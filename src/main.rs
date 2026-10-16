mod aa;
mod chrome;
mod download;
mod libgen;

use aa::Book;
use anyhow::{Context, Result, bail};
use chrome::Chrome;
use clap::{Parser, Subcommand};
use std::env;
use std::path::PathBuf;
use std::process::exit;

#[derive(Parser)]
#[command(
    name = "bookos",
    version,
    about = "Search and download books from Anna's Archive and Libgen",
    after_help = "Config in ~/.config/bookos/config (key=value):
  key=<Anna's Archive member secret key>          enables fast downloads
  dir=~/Books                                     where to save (default ~/Downloads)
  sources=aa-fast,aa-slow,libgen                  source order
  domain=annas-archive.gl,annas-archive.pk        Anna's Archive domains, in order
  libgen=libgen.li,libgen.gl                      Libgen mirrors"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
    /// Show the Chrome window (to solve the site's browser check by hand)
    #[arg(long, global = true)]
    show: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Search Anna's Archive (which indexes Libgen, Z-Lib, Sci-Hub, Internet Archive…)
    Search {
        terms: Vec<String>,
        /// Format: pdf, epub, mobi, azw3, djvu…
        #[arg(short, long)]
        format: Option<String>,
        /// Language: en, pt, es…
        #[arg(short, long)]
        lang: Option<String>,
        /// How many results to show
        #[arg(short = 'n', long, default_value_t = 10)]
        max: usize,
        #[arg(long)]
        json: bool,
    },
    /// Download by result number from the last search, or by md5
    Download {
        target: String,
        /// Destination directory
        #[arg(short, long)]
        dir: Option<PathBuf>,
        /// Only this source: aa-fast, aa-slow or libgen
        #[arg(long)]
        source: Option<String>,
    },
}

pub fn home() -> PathBuf {
    PathBuf::from(env::var("HOME").unwrap_or_default())
}

pub fn config(key: &str) -> Option<String> {
    std::fs::read_to_string(home().join(".config/bookos/config"))
        .ok()?
        .lines()
        .find_map(|l| l.split_once('=').filter(|(k, _)| k.trim() == key).map(|(_, v)| v.trim().to_string()))
        .filter(|v| !v.is_empty())
}

fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(p),
    }
}

fn last_search_path() -> PathBuf {
    home().join(".config/bookos/last-search.json")
}

unsafe extern "C" {
    fn signal(sig: i32, handler: extern "C" fn(i32)) -> usize;
}

extern "C" fn on_signal(_: i32) {
    chrome::kill_all();
    exit(130)
}

fn print_books(books: &[Book]) {
    for (i, b) in books.iter().enumerate() {
        println!("{:>3}  {}", i + 1, b.title);
        let mut info: Vec<&str> = Vec::new();
        for f in [&b.author, &b.year, &b.lang, &b.ext, &b.size] {
            if !f.is_empty() {
                info.push(f);
            }
        }
        let sources = b.sources.join("/");
        if !sources.is_empty() {
            info.push(&sources);
        }
        println!("     {}", info.join(" · "));
        println!("     {}", b.md5);
    }
}

fn clean(s: &str) -> String {
    let s: String = s.chars().map(|c| if "/\\:*?\"<>|".contains(c) || c.is_control() { ' ' } else { c }).collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    s.chars().take(110).collect::<String>().trim().to_string()
}

fn file_name(b: &Book) -> String {
    let author = b.author.split(';').next().unwrap_or("").trim();
    let mut name = clean(&b.title);
    if name.is_empty() {
        name = b.md5.clone();
    }
    if !author.is_empty() {
        name = format!("{} - {name}", clean(author));
    }
    if !b.year.is_empty() {
        name = format!("{name} ({})", b.year);
    }
    let ext = if b.ext.is_empty() { "bin" } else { &b.ext };
    format!("{name}.{ext}")
}

fn resolve_target(target: &str, chrome: &mut Option<Chrome>, show: bool) -> Result<Book> {
    let saved: Vec<Book> = std::fs::read_to_string(last_search_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    if let Ok(n) = target.parse::<usize>()
        && target.len() < 32
    {
        return saved.get(n.wrapping_sub(1)).cloned().with_context(|| format!("there is no item {n} in the last search"));
    }
    let md5 = target.to_lowercase();
    if md5.len() != 32 || !md5.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("use a result number from the last search or a 32-character md5");
    }
    if let Some(b) = saved.into_iter().find(|b| b.md5 == md5) {
        return Ok(b);
    }
    eprintln!("looking up {md5}…");
    let c = open_chrome(chrome, show)?;
    aa::lookup(c, &md5)
}

fn open_chrome(chrome: &mut Option<Chrome>, show: bool) -> Result<&Chrome> {
    if chrome.is_none() {
        *chrome = Some(Chrome::open(show)?);
    }
    Ok(chrome.as_ref().unwrap())
}

fn download(target: &str, dir: Option<PathBuf>, source: Option<String>, show: bool) -> Result<()> {
    let mut chrome: Option<Chrome> = None;
    let book = resolve_target(target, &mut chrome, show)?;
    let dir = dir.or_else(|| config("dir").map(|p| expand(&p))).unwrap_or_else(|| home().join("Downloads"));
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(file_name(&book));
    if dest.exists() {
        println!("already downloaded: {}", dest.display());
        return Ok(());
    }
    println!("{}", book.title);
    if !book.author.is_empty() {
        println!("{}", book.author);
    }

    let key = config("key").or_else(|| env::var("AA_KEY").ok());
    let order = source.or_else(|| config("sources")).unwrap_or_else(|| "aa-fast,aa-slow,libgen".into());
    let mut errors = Vec::new();
    for source in order.split(',').map(str::trim) {
        let links: Vec<Result<String>> = match source {
            "aa-fast" => match &key {
                Some(k) => vec![aa::fast_link(&book.md5, k)],
                None => continue,
            },
            "libgen" => vec![libgen::link(&book.md5)],
            "aa-slow" => {
                let c = match open_chrome(&mut chrome, show) {
                    Ok(c) => c,
                    Err(e) => {
                        errors.push(format!("aa-slow: {e}"));
                        continue;
                    }
                };
                let mut tried = Vec::new();
                for server in 0..3 {
                    eprintln!("aa-slow: getting the link from server #{}…", server + 1);
                    let link = aa::slow_link(c, &book.md5, server);
                    let ok = link.is_ok();
                    tried.push(link);
                    if ok {
                        break;
                    }
                }
                tried
            }
            other => {
                errors.push(format!("unknown source: {other}"));
                continue;
            }
        };
        for link in links {
            let url = match link {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("{source}: {e}");
                    errors.push(format!("{source}: {e}"));
                    continue;
                }
            };
            eprintln!("{source}: downloading…");
            match download::fetch(&url, &dest, &book.md5) {
                Ok(()) => {
                    println!("saved to {}", dest.display());
                    return Ok(());
                }
                Err(e) => {
                    eprintln!("{source}: {e}");
                    errors.push(format!("{source}: {e}"));
                }
            }
        }
    }
    if key.is_none() && errors.is_empty() {
        bail!("no source available");
    }
    bail!("could not download:\n  {}", errors.join("\n  "))
}

fn search(terms: &[String], format: Option<String>, lang: Option<String>, max: usize, json: bool, show: bool) -> Result<()> {
    let query = terms.join(" ");
    if query.trim().is_empty() {
        bail!("say what to search for: bookos search <title, author, isbn…>");
    }
    let chrome = Chrome::open(show)?;
    let mut books = aa::search(&chrome, &query, format.as_deref(), lang.as_deref())?;
    drop(chrome);
    books.truncate(max);
    std::fs::create_dir_all(home().join(".config/bookos"))?;
    std::fs::write(last_search_path(), serde_json::to_string_pretty(&books)?)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&books)?);
    } else if books.is_empty() {
        println!("nothing found");
    } else {
        print_books(&books);
        println!("\ndownload with: bookos download <number>");
    }
    Ok(())
}

fn main() {
    unsafe {
        signal(1, on_signal);
        signal(2, on_signal);
        signal(15, on_signal);
    }
    let cli = Cli::parse();
    let result = match cli.cmd {
        Cmd::Search { terms, format, lang, max, json } => search(&terms, format, lang, max, json, cli.show),
        Cmd::Download { target, dir, source } => download(&target, dir, source, cli.show),
    };
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        exit(1);
    }
}
