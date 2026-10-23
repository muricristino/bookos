use crate::aa::{Book, Filters};
use crate::{config, home};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SUBJECTS: &[(&str, &str, &str, &str)] = &[
    ("brazilian-fiction", "Brazilian fiction", "brazilian_fiction", "editions"),
    ("fantasy", "Fantasy", "fantasy", "rating"),
    ("mystery", "Mystery and crime", "detective_and_mystery_stories", "rating"),
    ("russian-classics", "Russian classics", "russian_fiction", "editions"),
];
const MAX_AGE: u64 = 24 * 3600;

#[derive(Clone, Debug)]
pub enum Source {
    Newest,
    Subject(String, String),
    Author(String),
    List(Vec<(String, String)>),
}

#[derive(Clone, Debug)]
pub struct Shelf {
    pub id: String,
    pub title: String,
    pub source: Source,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Stocked {
    pub id: String,
    pub title: String,
    pub built: u64,
    pub books: Vec<Book>,
}

pub type Search<'a> = &'a dyn Fn(&str, Filters) -> Result<Vec<Book>>;

fn lang() -> String {
    config("store_lang").unwrap_or_else(|| "pt".into())
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn cache_path(id: &str) -> PathBuf {
    home().join(".cache/bookos/store").join(format!("{id}.json"))
}

fn lists_path() -> PathBuf {
    home().join(".config/bookos/shelves")
}

pub fn fold(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' | 'Á' | 'À' | 'Â' | 'Ã' | 'Ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' | 'Í' | 'Ì' | 'Î' | 'Ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' | 'Ó' | 'Ò' | 'Ô' | 'Õ' | 'Ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' | 'Ú' | 'Ù' | 'Û' | 'Ü' => 'u',
            'ç' | 'Ç' => 'c',
            'ñ' | 'Ñ' => 'n',
            c if c.is_alphanumeric() => c.to_ascii_lowercase(),
            _ => ' ',
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn slug(s: &str) -> String {
    fold(s).replace(' ', "-")
}

fn words(s: &str) -> Vec<String> {
    const SKIP: &[&str] = &["the", "and", "dos", "das", "uma", "com", "para", "por", "del", "los", "las"];
    fold(s).split(' ').filter(|w| w.len() >= 3 && !SKIP.contains(w)).map(String::from).collect()
}

fn title_matches(found: &str, wanted: &str) -> bool {
    let found = format!(" {} ", fold(found));
    let wanted = words(wanted);
    if wanted.is_empty() {
        return false;
    }
    let need = wanted.len().min(2);
    wanted.iter().take(3).filter(|w| found.contains(&format!(" {w} "))).count() >= need
}

fn author_matches(found: &str, wanted: &str) -> bool {
    let found = fold(found);
    words(wanted).iter().any(|w| found.contains(w.as_str()))
}

fn human_name(name: &str) -> String {
    match name.split_once(", ") {
        Some((last, first)) if !first.contains(',') => format!("{first} {last}"),
        _ => name.to_string(),
    }
}

pub fn library() -> Vec<(String, String)> {
    let names: Vec<String> = match config("ssh") {
        Some(host) => std::process::Command::new("ssh")
            .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5", &host, "find /mnt/us/documents -name '*.epub' -o -name '*.azw3' -o -name '*.mobi' -o -name '*.pdf'"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(String::from).collect())
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let names = if names.is_empty() {
        std::fs::read_dir(home().join(".cache/bookos/downloads"))
            .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect())
            .unwrap_or_default()
    } else {
        names
    };
    names.iter().filter_map(|p| parse_entry(p)).collect()
}

fn unflip(title: &str) -> String {
    for article in ["A", "O", "As", "Os", "The", "Um", "Uma"] {
        if let Some(rest) = title.strip_suffix(&format!(", {article}")) {
            return format!("{article} {rest}");
        }
    }
    title.to_string()
}

fn parse_entry(path: &str) -> Option<(String, String)> {
    let path = path.strip_prefix("/mnt/us/documents/").unwrap_or(path);
    let mut parts: Vec<&str> = path.split('/').collect();
    let file = parts.pop()?;
    let stem = file.rsplit_once('.')?.0;
    let folder = parts.last().copied().unwrap_or("");
    let clean = |t: &str| unflip(t.split(" (").next().unwrap_or(t).split(" [").next().unwrap_or(t).replace('_', ":").trim());
    if parts.len() == 1 && folder.contains(", ") && !folder.contains('&') {
        let (title, _) = stem.rsplit_once(" - ")?;
        let author = human_name(&folder.replace("_", "."));
        return Some((author, clean(title)));
    }
    if parts.is_empty() || folder == "bookos" {
        let (author, title) = stem.split_once(" - ")?;
        let year = title.rsplit_once(" (").map(|(_, y)| y.trim_end_matches(')')).unwrap_or("");
        if author.chars().any(|c| c.is_ascii_digit()) || !(year.len() == 4 && year.chars().all(|c| c.is_ascii_digit()) || title.contains(" [")) {
            return None;
        }
        return Some((human_name(author.trim()), clean(title)));
    }
    None
}

fn read_lists() -> Vec<Shelf> {
    let text = std::fs::read_to_string(lists_path()).unwrap_or_default();
    let mut shelves: Vec<Shelf> = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(title) = line.strip_prefix('#') {
            let title = title.trim().to_string();
            shelves.push(Shelf { id: slug(&title), title, source: Source::List(Vec::new()) });
        } else if let Some(Shelf { source: Source::List(items), .. }) = shelves.last_mut() {
            let (title, author) = line.split_once('|').unwrap_or((line, ""));
            items.push((title.trim().to_string(), author.trim().to_string()));
        }
    }
    shelves
}

pub fn shelves() -> Vec<Shelf> {
    let mut list = vec![Shelf { id: "new".into(), title: "New arrivals".into(), source: Source::Newest }];
    for (id, title, subject, sort) in SUBJECTS {
        list.push(Shelf { id: id.to_string(), title: title.to_string(), source: Source::Subject(subject.to_string(), sort.to_string()) });
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (author, _) in library() {
        *counts.entry(author).or_default() += 1;
    }
    let mut authors: Vec<(String, usize)> = counts.into_iter().collect();
    authors.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (author, _) in authors.into_iter().take(4) {
        list.push(Shelf { id: format!("more-{}", slug(&author)), title: format!("More by {author}"), source: Source::Author(author) });
    }
    list.extend(read_lists());
    list
}

pub fn find(id: &str) -> Result<Shelf> {
    let all = shelves();
    let ids: Vec<String> = all.iter().map(|s| s.id.clone()).collect();
    all.into_iter().find(|s| s.id == id).with_context(|| format!("no shelf called {id}; the shelves are: {}", ids.join(", ")))
}

pub fn cached(id: &str) -> Option<Stocked> {
    serde_json::from_str(&std::fs::read_to_string(cache_path(id)).ok()?).ok()
}

pub fn is_fresh(stocked: &Stocked) -> bool {
    now().saturating_sub(stocked.built) < MAX_AGE
}

fn open_library(subject: &str, sort: &str, lang: &str) -> Result<Vec<(String, String)>> {
    let code = match lang {
        "pt" => "por",
        "en" => "eng",
        "es" => "spa",
        "fr" => "fre",
        other => other,
    };
    let url = format!(
        "https://openlibrary.org/search.json?subject={subject}&language={code}&sort={sort}&limit=40&fields=title,author_name,editions,editions.title,editions.language"
    );
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(40))).build().into();
    let v: Value = agent.get(&url).header("User-Agent", "bookos (github.com/muricristino/bookos)").call().context("could not reach Open Library")?.into_body().read_json()?;
    Ok(v["docs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            let edition = d["editions"]["docs"].as_array().and_then(|e| e.first()).and_then(|e| e["title"].as_str());
            let title = edition.or(d["title"].as_str())?.to_string();
            let author = d["author_name"].as_array().and_then(|a| a.first()).and_then(|a| a.as_str()).unwrap_or("").to_string();
            Some((title, author))
        })
        .collect())
}

fn looks_english(title: &str) -> bool {
    fold(title).split(' ').filter(|w| ["the", "and", "of", "to", "in", "with", "from"].contains(w)).count() >= 2
}

fn resolve(search: Search, lang: &str, title: &str, author: &str) -> Result<Option<Book>> {
    let filters = Filters { ext: Some("epub"), lang: Some(lang), ..Default::default() };
    let books = search(&format!("{title} {author}"), filters)?;
    let mut matching = books.into_iter().filter(|b| title_matches(&b.title, title) && (lang == "en" || !looks_english(&b.title)));
    let first = matching.next();
    if author.is_empty() {
        return Ok(first);
    }
    Ok(first.into_iter().chain(matching).find(|b| author_matches(&b.author, author)))
}

pub fn build(shelf: &Shelf, search: Search, max: usize) -> Result<Stocked> {
    let lang = lang();
    let owned: Vec<String> = library().iter().map(|(_, t)| fold(t)).collect();
    let mut books: Vec<Book> = Vec::new();
    let add = |b: Book, books: &mut Vec<Book>| {
        let key = fold(&b.title);
        if !owned.contains(&key) && !books.iter().any(|x| fold(&x.title) == key || x.md5 == b.md5) {
            books.push(b);
        }
    };
    match &shelf.source {
        Source::Newest => {
            let filters = Filters { ext: Some("epub"), lang: Some(&lang), sort: Some("newest_added"), ..Default::default() };
            for b in search("", filters)? {
                add(b, &mut books);
            }
        }
        Source::Author(author) => {
            let filters = Filters { ext: Some("epub"), lang: Some(&lang), field: Some("author"), ..Default::default() };
            for b in search(author, filters)? {
                add(b, &mut books);
            }
        }
        Source::Subject(subject, sort) => {
            for (title, author) in open_library(subject, sort, &lang)? {
                if books.len() >= max {
                    break;
                }
                eprintln!("  {title} — {author}");
                if let Some(b) = resolve(search, &lang, &title, &author)? {
                    add(b, &mut books);
                }
            }
        }
        Source::List(items) => {
            if items.is_empty() {
                bail!("the shelf {} in {} has no books", shelf.title, lists_path().display());
            }
            for (title, author) in items {
                if books.len() >= max {
                    break;
                }
                eprintln!("  {title} — {author}");
                if let Some(b) = resolve(search, &lang, title, author)? {
                    add(b, &mut books);
                }
            }
        }
    }
    books.truncate(max);
    let stocked = Stocked { id: shelf.id.clone(), title: shelf.title.clone(), built: now(), books };
    let path = cache_path(&shelf.id);
    std::fs::create_dir_all(path.parent().context("bad cache path")?)?;
    std::fs::write(&path, serde_json::to_string_pretty(&stocked)?)?;
    Ok(stocked)
}

pub fn age(stocked: &Stocked) -> String {
    let secs = now().saturating_sub(stocked.built);
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86400 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86400),
    }
}
