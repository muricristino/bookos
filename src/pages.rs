use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

#[derive(Default, Clone, Copy)]
pub struct Query<'a> {
    pub isbn: Option<&'a str>,
    pub title: Option<&'a str>,
    pub author: Option<&'a str>,
    pub lang: Option<&'a str>,
    pub publisher: Option<&'a str>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Edition {
    pub pages: u32,
    pub title: String,
    pub publisher: String,
    pub year: String,
    pub isbn: String,
}

#[derive(Serialize, Debug)]
pub struct Found {
    pub pages: u32,
    pub editions: Vec<Edition>,
}

const OPEN_LIBRARY: &str = "https://openlibrary.org";

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(20))).http_status_as_error(false).build().into()
}

fn get(agent: &ureq::Agent, path: &str) -> Option<Value> {
    let mut resp = agent.get(&format!("{OPEN_LIBRARY}{path}")).header("User-Agent", "bookos (github.com/muricristino/bookos)").call().ok()?;
    if resp.status() != 200 {
        return None;
    }
    resp.body_mut().with_config().limit(32 * 1024 * 1024).read_json().ok()
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

fn ol_lang(lang: &str) -> String {
    let lang = lang.split([',', '-', '_']).next().unwrap_or("").trim().to_lowercase();
    match lang.as_str() {
        "pt" => "por",
        "en" => "eng",
        "es" => "spa",
        "fr" => "fre",
        "it" => "ita",
        "de" => "ger",
        "ru" => "rus",
        "ja" => "jpn",
        other => return other.to_string(),
    }
    .to_string()
}

fn normalize(s: &str) -> String {
    let folded: String = s
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        })
        .collect();
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn clean_title(title: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in title.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let out = out.split([':', ';']).next().unwrap_or("").trim().to_string();
    if out.is_empty() { title.trim().to_string() } else { out }
}

pub fn clean_author(author: &str) -> String {
    let first = author.split([';', '&']).next().unwrap_or("").trim();
    let first = first.split('[').next().unwrap_or("").trim();
    match first.split_once(',') {
        Some((last, given)) => format!("{} {}", given.trim(), last.trim()),
        None => first.to_string(),
    }
}

fn edition(e: &Value) -> Option<Edition> {
    let pages = e["number_of_pages"].as_u64().filter(|&p| p >= 20)? as u32;
    let first = |k: &str| e[k].as_array().and_then(|a| a.first()).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let date = e["publish_date"].as_str().unwrap_or("");
    let year = date.split(|c: char| !c.is_ascii_digit()).find(|p| p.len() == 4).unwrap_or("").to_string();
    let isbn = if first("isbn_13").is_empty() { first("isbn_10") } else { first("isbn_13") };
    Some(Edition { pages, title: e["title"].as_str().unwrap_or("").to_string(), publisher: first("publishers"), year, isbn })
}

fn in_lang(e: &Value, lang: &str) -> bool {
    e["languages"].as_array().is_some_and(|a| a.iter().any(|l| l["key"].as_str().is_some_and(|k| k.ends_with(&format!("/{lang}")))))
}

fn work_editions(agent: &ureq::Agent, work: &str, lang: Option<&str>) -> Vec<Edition> {
    let Some(v) = get(agent, &format!("{work}/editions.json?limit=1000")) else {
        return Vec::new();
    };
    v["entries"].as_array().into_iter().flatten().filter(|e| lang.is_none_or(|l| in_lang(e, l))).filter_map(edition).collect()
}

pub fn find(q: Query) -> Result<Found> {
    let agent = agent();
    let lang = q.lang.filter(|l| !l.is_empty()).map(ol_lang);
    let mut exact = None;
    let mut works = Vec::new();
    if let Some(isbn) = q.isbn.map(|i| i.replace(['-', ' '], "")).filter(|i| i.len() == 10 || i.len() == 13)
        && let Some(v) = get(&agent, &format!("/isbn/{isbn}.json"))
    {
        exact = edition(&v);
        if let Some(w) = v["works"].as_array().and_then(|a| a.first()).and_then(|w| w["key"].as_str()) {
            works.push(w.to_string());
        }
    }
    if works.is_empty() {
        let title = q.title.map(clean_title).unwrap_or_default();
        if title.is_empty() {
            bail!("give an isbn or a title");
        }
        let author = q.author.map(clean_author).unwrap_or_default();
        let wanted = normalize(&title);
        for text in [format!("{title} {author}"), title.clone()] {
            let mut path = format!("/search.json?q={}&fields=key,title,editions,editions.title&limit=8", encode(text.trim()));
            if let Some(l) = q.lang.filter(|l| !l.is_empty()) {
                path.push_str(&format!("&lang={}", encode(l)));
            }
            let Some(v) = get(&agent, &path) else { continue };
            for doc in v["docs"].as_array().into_iter().flatten() {
                let titles = std::iter::once(doc["title"].as_str().unwrap_or("")).chain(doc["editions"]["docs"].as_array().into_iter().flatten().filter_map(|e| e["title"].as_str()));
                let matches = titles.map(normalize).any(|t| !t.is_empty() && (t == wanted || t.starts_with(&wanted) || wanted.starts_with(&t)));
                if let Some(key) = doc["key"].as_str().filter(|_| matches)
                    && !works.iter().any(|w| w == key)
                {
                    works.push(key.to_string());
                }
            }
            if !works.is_empty() {
                break;
            }
        }
    }
    let mut editions: Vec<Edition> = Vec::new();
    for work in works.iter().take(3) {
        editions.extend(work_editions(&agent, work, lang.as_deref()));
    }
    if editions.is_empty() && exact.is_none() {
        bail!("Open Library has no page count for this book");
    }
    let mut seen = std::collections::HashSet::new();
    editions.retain(|e| seen.insert((e.pages, normalize(&e.publisher), e.year.clone())));
    let mut sorted: Vec<u32> = editions.iter().map(|e| e.pages).collect();
    sorted.sort_unstable();
    let median = sorted.get(sorted.len() / 2).copied().unwrap_or(0) as i64;
    let publisher = q.publisher.map(normalize).filter(|p| p.len() >= 3);
    let same_publisher = |e: &Edition| {
        let p = normalize(&e.publisher);
        publisher.as_ref().is_some_and(|want| !p.is_empty() && (p.contains(want.as_str()) || want.contains(p.as_str())))
    };
    editions.sort_by_key(|e| (!same_publisher(e), (e.pages as i64 - median).abs(), std::cmp::Reverse(e.year.clone())));
    if let Some(e) = exact {
        editions.retain(|x| !(x.pages == e.pages && x.isbn == e.isbn));
        editions.insert(0, e);
    }
    Ok(Found { pages: editions[0].pages, editions })
}
