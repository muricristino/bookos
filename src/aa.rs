use crate::chrome::{Chrome, Tab};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::thread::sleep;
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Book {
    pub md5: String,
    pub title: String,
    pub author: String,
    pub publisher: String,
    pub year: String,
    pub lang: String,
    pub ext: String,
    pub size: String,
    pub kind: String,
    pub sources: Vec<String>,
}

const SEARCH_JS: &str = r#"JSON.stringify([...document.querySelectorAll('a.js-vim-focus[href^="/md5/"]')].map(a => {
  const info = a.parentElement.parentElement;
  const icon = n => { const s = info.querySelector('[class*="mdi--' + n + '"]'); return s ? s.parentElement.textContent.trim() : '' };
  const meta = info.querySelector('div.text-gray-800') || [...info.querySelectorAll('div')].find(d => / · /.test(d.textContent) && /\d(KB|MB|GB)/.test(d.textContent));
  return { md5: a.getAttribute('href').slice(5), title: a.textContent.trim(), author: icon('user-edit'), publisher: icon('company'), meta: meta ? meta.textContent.trim() : '' };
}))"#;

const SLOW_JS: &str = r#"JSON.stringify((() => {
  const links = [...document.querySelectorAll('a[href^="http"]')];
  const a = links.find(a => /download now/i.test(a.textContent)) || document.querySelector('p.text-xl a[href^="http"]');
  if (a) return { url: a.href };
  const t = (document.body ? document.body.innerText : '').replace(/\s+/g, ' ');
  const w = t.match(/please wait[^.!]*/i);
  const i = Math.max(t.search(/Slow Partner Server/i), t.search(/Download from partner/i), 0);
  return { wait: w ? w[0].trim() : '', text: t.slice(i, i + 400) };
})())"#;

fn domains() -> Vec<String> {
    let list = crate::config("domain").unwrap_or_else(|| "annas-archive.gl,annas-archive.pk,annas-archive.gd".into());
    list.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

fn open<'a>(chrome: &'a Chrome, path: &str) -> Result<Tab<'a>> {
    let mut errors = Vec::new();
    for host in domains() {
        match chrome.tab(&format!("https://{host}{path}")) {
            Ok(tab) => return Ok(tab),
            Err(e) => {
                eprintln!("  {host}: {e}");
                errors.push(format!("{host}: {e}"));
            }
        }
    }
    bail!("no Anna's Archive domain answered ({})", errors.join("; "))
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn parse_meta(book: &mut Book, meta: &str) {
    for part in meta.split('·').map(|p| p.replace('\u{a0}', " ").trim().to_string()) {
        let is_size = ["KB", "MB", "GB"].iter().any(|u| part.ends_with(u)) && part.starts_with(|c: char| c.is_ascii_digit());
        if let Some(src) = part.strip_prefix("🚀/") {
            book.sources = src.split('/').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        } else if part.contains('[') && part.contains(']') && book.lang.is_empty() {
            book.lang = part
                .split('[')
                .skip(1)
                .filter_map(|s| s.split(']').next())
                .collect::<Vec<_>>()
                .join(",");
        } else if is_size {
            book.size = part;
        } else if part.len() == 4 && part.chars().all(|c| c.is_ascii_digit()) {
            book.year = part;
        } else if !part.is_empty() && part.len() <= 6 && part.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) && book.ext.is_empty() {
            book.ext = part.to_lowercase();
        } else if ["Book", "Journal", "Magazine", "Comic", "Standard", "Musical", "Other"].iter().any(|k| part.contains(k)) {
            book.kind = part.trim_start_matches(|c: char| !c.is_ascii_alphabetic()).to_string();
        }
    }
}

pub fn search(chrome: &Chrome, query: &str, ext: Option<&str>, lang: Option<&str>) -> Result<Vec<Book>> {
    let mut url = format!("/search?q={}", encode(query));
    if let Some(e) = ext {
        url.push_str(&format!("&ext={}", encode(&e.to_lowercase())));
    }
    if let Some(l) = lang {
        url.push_str(&format!("&lang={}", encode(&l.to_lowercase())));
    }
    let mut raw = Value::Null;
    for attempt in 0..2 {
        let tab = open(chrome, &url)?;
        raw = tab.eval_json(SEARCH_JS)?;
        if raw.as_array().is_some_and(|a| !a.is_empty()) {
            break;
        }
        if attempt == 1 {
            let title = tab.eval("document.title")?;
            eprintln!("  the page came back with no results ({})", title.as_str().unwrap_or("").trim());
        }
        sleep(Duration::from_secs(2));
    }
    let mut books = Vec::new();
    for item in raw.as_array().cloned().unwrap_or_default() {
        let s = |k: &str| item[k].as_str().unwrap_or("").trim().to_string();
        let mut book = Book { md5: s("md5"), title: s("title"), author: s("author"), publisher: s("publisher"), ..Default::default() };
        if book.md5.len() != 32 || books.iter().any(|b: &Book| b.md5 == book.md5) {
            continue;
        }
        parse_meta(&mut book, &s("meta"));
        if ext.is_some_and(|e| !book.ext.eq_ignore_ascii_case(e)) {
            continue;
        }
        if lang.is_some_and(|l| !book.lang.is_empty() && !book.lang.to_lowercase().contains(&l.to_lowercase())) {
            continue;
        }
        books.push(book);
    }
    Ok(books)
}

pub fn lookup(chrome: &Chrome, md5: &str) -> Result<Book> {
    let books = search(chrome, md5, None, None)?;
    books.into_iter().find(|b| b.md5 == md5).with_context(|| format!("Anna's Archive did not find md5 {md5}"))
}

pub fn fast_link(md5: &str, key: &str) -> Result<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut errors = Vec::new();
    for host in domains() {
        let url = format!("https://{host}/dyn/api/fast_download.json?md5={md5}&key={}", encode(key));
        let v: Value = match agent.get(&url).call().map_err(anyhow::Error::from).and_then(|r| Ok(r.into_body().read_json()?)) {
            Ok(v) => v,
            Err(e) => {
                errors.push(format!("{host}: {e}"));
                continue;
            }
        };
        match v["download_url"].as_str() {
            Some(u) => return Ok(u.to_string()),
            None => bail!("{}", v["error"].as_str().unwrap_or("no download_url in the response")),
        }
    }
    bail!("{}", errors.join("; "))
}

pub fn slow_link(chrome: &Chrome, md5: &str, server: u32) -> Result<String> {
    let tab = open(chrome, &format!("/slow_download/{md5}/0/{server}"))?;
    let t = Instant::now();
    let mut last = String::new();
    let mut waited = Instant::now();
    loop {
        sleep(Duration::from_secs(2));
        let v = match tab.eval_json(SLOW_JS) {
            Ok(v) => v,
            Err(_) if t.elapsed() < Duration::from_secs(300) => continue,
            Err(e) => return Err(e),
        };
        if let Some(u) = v["url"].as_str() {
            if !last.is_empty() {
                eprintln!();
            }
            return Ok(u.to_string());
        }
        let wait = v["wait"].as_str().unwrap_or("");
        if !wait.is_empty() {
            waited = Instant::now();
            if wait != last {
                eprint!("\r  {wait}     ");
                last = wait.to_string();
            }
        }
        if waited.elapsed() > Duration::from_secs(30) || t.elapsed() > Duration::from_secs(300) {
            if !last.is_empty() {
                eprintln!();
            }
            bail!("the download link did not show up: {}", v["text"].as_str().unwrap_or("").trim());
        }
    }
}
