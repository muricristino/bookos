use crate::aa::{self, Book};
use crate::chrome::Chrome;
use crate::{fetch_book, file_name, home};
use anyhow::{Context, Result, anyhow};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Cursor, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tiny_http::{Header, Request, Response, Server};

#[derive(Clone)]
enum Job {
    Working(Book),
    Done(PathBuf),
    Failed(String),
}

struct State {
    chrome: Mutex<Option<Chrome>>,
    books: Mutex<HashMap<String, Book>>,
    details: Mutex<HashMap<String, aa::Details>>,
    jobs: Mutex<HashMap<String, Job>>,
    show: bool,
}

type Reply = Response<Box<dyn Read + Send>>;

fn cache_dir(sub: &str) -> PathBuf {
    home().join(".cache/bookos").join(sub)
}

pub fn run(bind: &str, port: u16, show: bool) -> Result<()> {
    let server = Server::http((bind, port)).map_err(|e| anyhow!("could not listen on {bind}:{port}: {e}"))?;
    let state = Arc::new(State { chrome: Mutex::new(None), books: Mutex::new(HashMap::new()), details: Mutex::new(HashMap::new()), jobs: Mutex::new(HashMap::new()), show });
    eprintln!("bookos is serving on http://{bind}:{port}");
    for request in server.incoming_requests() {
        let state = state.clone();
        thread::spawn(move || handle(&state, request));
    }
    Ok(())
}

fn handle(state: &Arc<State>, request: Request) {
    let url = request.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    let params = parse_query(query);
    let reply = if path == "/" {
        Ok(json_reply(200, &json!({ "name": "bookos", "version": env!("CARGO_PKG_VERSION") })))
    } else if path == "/search" {
        search(state, &params)
    } else if let Some(rest) = path.strip_prefix("/cover/") {
        let width = params.get("w").and_then(|w| w.parse().ok()).unwrap_or(300u32).clamp(60, 1236);
        cover(state, rest.trim_end_matches(".jpg"), width)
    } else if let Some(md5) = path.strip_prefix("/book/") {
        book_details(state, md5)
    } else if let Some(md5) = path.strip_prefix("/download/") {
        download(state, md5)
    } else {
        Ok(json_reply(404, &json!({ "error": "not found" })))
    };
    let reply = reply.unwrap_or_else(|e| {
        eprintln!("{path}: {e:#}");
        json_reply(500, &json!({ "error": format!("{e:#}") }))
    });
    let _ = request.respond(reply);
}

fn parse_query(query: &str) -> HashMap<String, String> {
    query.split('&').filter_map(|pair| pair.split_once('=')).map(|(k, v)| (decode(k), decode(v))).collect()
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                Ok(b) => {
                    out.push(b);
                    i += 2;
                }
                Err(_) => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
}

fn json_reply(status: u16, body: &Value) -> Reply {
    let data = body.to_string().into_bytes();
    let len = data.len();
    Response::new(status.into(), vec![header("Content-Type", "application/json; charset=utf-8")], Box::new(Cursor::new(data)), Some(len), None)
}

fn valid_md5(md5: &str) -> bool {
    md5.len() == 32 && md5.chars().all(|c| c.is_ascii_hexdigit())
}

fn with_chrome<T>(state: &State, f: impl Fn(&Chrome) -> Result<T>) -> Result<T> {
    let mut guard = state.chrome.lock().map_err(|_| anyhow!("Chrome lock poisoned"))?;
    for attempt in 0..2 {
        if guard.is_none() {
            *guard = Some(Chrome::open(state.show)?);
        }
        match f(guard.as_ref().expect("chrome is open")) {
            Ok(v) => return Ok(v),
            Err(e) if attempt == 0 => {
                eprintln!("retrying with a fresh Chrome: {e:#}");
                *guard = None;
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

fn search(state: &State, params: &HashMap<String, String>) -> Result<Reply> {
    let query = params.get("q").map(|q| q.trim()).unwrap_or("");
    if query.is_empty() {
        return Ok(json_reply(400, &json!({ "error": "missing q" })));
    }
    let param = |k: &str| params.get(k).map(String::as_str).filter(|v| !v.is_empty());
    let filters = aa::Filters {
        ext: param("format"),
        lang: param("lang"),
        field: param("by").filter(|f| ["author", "title"].contains(f)),
        page: param("page").and_then(|p| p.parse().ok()).unwrap_or(1),
    };
    let max = param("n").and_then(|n| n.parse().ok()).unwrap_or(20usize);
    let mut books = with_chrome(state, |c| aa::search(c, query, filters))?;
    books.truncate(max);
    if let Some(width) = param("w").and_then(|w| w.parse().ok()) {
        prefetch_covers(&books, width);
    }
    let mut known = state.books.lock().map_err(|_| anyhow!("lock poisoned"))?;
    let results: Vec<Value> = books
        .iter()
        .map(|b| {
            known.insert(b.md5.clone(), b.clone());
            let mut v = serde_json::to_value(b).unwrap_or(Value::Null);
            v["cover_url"] = if b.cover.is_empty() { Value::Null } else { json!(format!("/cover/{}.jpg", b.md5)) };
            v["download_url"] = json!(format!("/download/{}", b.md5));
            v
        })
        .collect();
    Ok(json_reply(200, &json!({ "query": query, "results": results })))
}

fn book_details(state: &State, md5: &str) -> Result<Reply> {
    if !valid_md5(md5) {
        return Ok(json_reply(400, &json!({ "error": "bad md5" })));
    }
    let cached = state.details.lock().map_err(|_| anyhow!("lock poisoned"))?.get(md5).cloned();
    let details = match cached {
        Some(d) => d,
        None => {
            let mut d = with_chrome(state, |c| aa::details(c, md5))?;
            if let Some(known) = state.books.lock().map_err(|_| anyhow!("lock poisoned"))?.get(md5)
                && d.book.cover.is_empty()
            {
                d.book.cover = known.cover.clone();
            }
            state.books.lock().map_err(|_| anyhow!("lock poisoned"))?.entry(md5.to_string()).or_insert_with(|| d.book.clone());
            state.details.lock().map_err(|_| anyhow!("lock poisoned"))?.insert(md5.to_string(), d.clone());
            d
        }
    };
    let mut v = serde_json::to_value(&details)?;
    v["cover_url"] = if details.book.cover.is_empty() { Value::Null } else { json!(format!("/cover/{md5}.jpg")) };
    v["download_url"] = json!(format!("/download/{md5}"));
    Ok(json_reply(200, &v))
}

fn book_for(state: &State, md5: &str) -> Result<Book> {
    if let Some(b) = state.books.lock().map_err(|_| anyhow!("lock poisoned"))?.get(md5) {
        return Ok(b.clone());
    }
    let book = with_chrome(state, |c| aa::lookup(c, md5))?;
    state.books.lock().map_err(|_| anyhow!("lock poisoned"))?.insert(md5.to_string(), book.clone());
    Ok(book)
}

fn cover(state: &State, md5: &str, width: u32) -> Result<Reply> {
    if !valid_md5(md5) {
        return Ok(json_reply(400, &json!({ "error": "bad md5" })));
    }
    let dir = cache_dir("covers");
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{md5}_{width}.jpg"));
    if !path.exists() {
        let book = book_for(state, md5)?;
        if book.cover.is_empty() {
            return Ok(json_reply(404, &json!({ "error": "no cover" })));
        }
        save_cover(&book, width, &path)?;
    }
    let file = File::open(&path)?;
    let len = file.metadata()?.len() as usize;
    Ok(Response::new(200.into(), vec![header("Content-Type", "image/jpeg"), header("Cache-Control", "max-age=604800")], Box::new(file), Some(len), None))
}

fn prefetch_covers(books: &[Book], width: u32) {
    let queue: Vec<Book> = books.iter().filter(|b| !b.cover.is_empty() && valid_md5(&b.md5)).cloned().collect();
    let queue = Arc::new(Mutex::new(queue));
    for _ in 0..8 {
        let queue = queue.clone();
        thread::spawn(move || {
            while let Some(book) = queue.lock().ok().and_then(|mut q| q.pop()) {
                let path = cache_dir("covers").join(format!("{}_{width}.jpg", book.md5));
                if !path.exists() {
                    let _ = fs::create_dir_all(cache_dir("covers")).map_err(anyhow::Error::from).and_then(|_| save_cover(&book, width, &path));
                }
            }
        });
    }
}

fn save_cover(book: &Book, width: u32, path: &std::path::Path) -> Result<()> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(30))).build().into();
    let mut resp = agent.get(&book.cover).header("User-Agent", "Mozilla/5.0").call().context("could not fetch the cover")?;
    let raw = resp.body_mut().with_config().limit(20 * 1024 * 1024).read_to_vec()?;
    let img = image::load_from_memory(&raw).context("the cover is not an image")?.grayscale();
    let height = (img.height() as u64 * width as u64 / img.width().max(1) as u64).max(1) as u32;
    let img = img.resize_exact(width, height, FilterType::Lanczos3);
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 85).encode_image(&img)?;
    let part = path.with_extension("part");
    fs::write(&part, out)?;
    fs::rename(&part, path)?;
    Ok(())
}

fn content_type(ext: &str) -> &'static str {
    match ext {
        "epub" => "application/epub+zip",
        "pdf" => "application/pdf",
        "mobi" | "azw" | "azw3" => "application/x-mobipocket-ebook",
        "txt" => "text/plain; charset=utf-8",
        "djvu" => "image/vnd.djvu",
        "fb2" => "application/x-fictionbook+xml",
        _ => "application/octet-stream",
    }
}

fn encode_filename(name: &str) -> String {
    name.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn download(state: &Arc<State>, md5: &str) -> Result<Reply> {
    let md5 = md5.to_lowercase();
    if !valid_md5(&md5) {
        return Ok(json_reply(400, &json!({ "error": "bad md5" })));
    }
    let job = state.jobs.lock().map_err(|_| anyhow!("lock poisoned"))?.get(&md5).cloned();
    match job {
        Some(Job::Done(path)) if path.exists() => {
            let file = File::open(&path)?;
            let len = file.metadata()?.len() as usize;
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("book").to_string();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
            let headers = vec![
                header("Content-Type", content_type(&ext)),
                header("Content-Disposition", &format!("attachment; filename*=UTF-8''{}", encode_filename(&name))),
                header("X-Bookos-Md5", &md5),
            ];
            Ok(Response::new(200.into(), headers, Box::new(file), Some(len), None))
        }
        Some(Job::Working(book)) => {
            let part = cache_dir("downloads").join(format!("{}.part", file_name(&book)));
            let bytes = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
            Ok(json_reply(202, &json!({ "state": "working", "bytes": bytes, "size": book.size, "title": book.title })))
        }
        Some(Job::Failed(e)) => {
            state.jobs.lock().map_err(|_| anyhow!("lock poisoned"))?.remove(&md5);
            Ok(json_reply(502, &json!({ "state": "failed", "error": e })))
        }
        _ => {
            let book = book_for(state, &md5)?;
            let size = book.size.clone();
            let title = book.title.clone();
            state.jobs.lock().map_err(|_| anyhow!("lock poisoned"))?.insert(md5.clone(), Job::Working(book.clone()));
            let state = state.clone();
            let id = md5.clone();
            thread::spawn(move || {
                let result = (|| {
                    let mut guard = state.chrome.lock().map_err(|_| anyhow!("Chrome lock poisoned"))?;
                    fetch_book(&book, &cache_dir("downloads"), None, &mut guard, state.show)
                })();
                let job = match result {
                    Ok(path) => Job::Done(path),
                    Err(e) => Job::Failed(format!("{e:#}")),
                };
                if let Ok(mut jobs) = state.jobs.lock() {
                    jobs.insert(id, job);
                }
            });
            Ok(json_reply(202, &json!({ "state": "working", "bytes": 0, "size": size, "title": title })))
        }
    }
}
