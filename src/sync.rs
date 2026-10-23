use crate::{config, expand, home};
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BOOK_EXTS: &[&str] = &["epub", "pdf", "mobi", "azw", "azw3", "txt", "fb2", "djvu", "cbz", "doc", "docx", "rtf"];
const SKIP: &[&str] = &["spidercat.azw3", "JAILBROKEN.txt", "My Clippings.txt"];
const KINDLE_ROOT: &str = "/mnt/us/documents";

pub struct Plan {
    pub to_drive: Vec<String>,
    pub to_kindle: Vec<String>,
    pub drop_drive: Vec<String>,
    pub drop_kindle: Vec<String>,
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn is_book(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    BOOK_EXTS.contains(&ext.as_str())
        && !SKIP.contains(&name)
        && !name.starts_with('.')
        && !rel.starts_with("dictionaries/")
        && !rel.split('/').any(|p| p.ends_with(".sdr") || p.starts_with('.'))
}

fn ssh(host: &str, command: &str) -> Command {
    let mut c = Command::new("ssh");
    c.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", host, command]);
    c
}

fn kindle_files(host: &str) -> Result<BTreeSet<String>> {
    let out = ssh(host, &format!("cd {KINDLE_ROOT} && find . -type f"))
        .output()
        .context("could not run ssh")?;
    if !out.status.success() {
        bail!("ssh to {host} failed; is the Kindle awake and on Wi-Fi?");
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim_start_matches("./").to_string())
        .filter(|l| is_book(l))
        .collect())
}

fn drive_files(root: &Path) -> BTreeSet<String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeSet<String>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if let Ok(rel) = path.strip_prefix(root) {
                let rel = rel.to_string_lossy().to_string();
                if is_book(&rel) {
                    out.insert(rel);
                }
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(root, root, &mut out);
    out
}

fn state_path() -> PathBuf {
    home().join(".config/bookos/sync-state")
}

fn load_state() -> BTreeSet<String> {
    fs::read_to_string(state_path()).map(|s| s.lines().map(String::from).collect()).unwrap_or_default()
}

fn save_state(files: &BTreeSet<String>) -> Result<()> {
    let body: String = files.iter().map(|f| format!("{f}\n")).collect();
    fs::write(state_path(), body)?;
    Ok(())
}

pub fn drive_dir() -> Result<PathBuf> {
    let dir = config("sync_dir").context("set the synced folder: bookos config sync_dir \"~/Library/CloudStorage/GoogleDrive-<you>/My Drive/Kindle\"")?;
    Ok(expand(&dir))
}

fn plan(kindle: &BTreeSet<String>, drive: &BTreeSet<String>, state: &BTreeSet<String>) -> Plan {
    let mut p = Plan { to_drive: vec![], to_kindle: vec![], drop_drive: vec![], drop_kindle: vec![] };
    for f in kindle.difference(drive) {
        if state.contains(f) { p.drop_kindle.push(f.clone()) } else { p.to_drive.push(f.clone()) }
    }
    for f in drive.difference(kindle) {
        if state.contains(f) { p.drop_drive.push(f.clone()) } else { p.to_kindle.push(f.clone()) }
    }
    p
}

fn pull(host: &str, rel: &str, root: &Path) -> Result<()> {
    let dest = root.join(rel);
    fs::create_dir_all(dest.parent().context("bad path")?)?;
    let part = dest.with_extension("part");
    let status = ssh(host, &format!("cat {}", quote(&format!("{KINDLE_ROOT}/{rel}"))))
        .stdout(fs::File::create(&part)?)
        .status()?;
    if !status.success() {
        let _ = fs::remove_file(&part);
        bail!("could not read {rel} from the Kindle");
    }
    fs::rename(&part, &dest)?;
    Ok(())
}

fn push(host: &str, rel: &str, root: &Path) -> Result<()> {
    let dest = quote(&format!("{KINDLE_ROOT}/{rel}"));
    let dir = quote(&format!("{KINDLE_ROOT}/{}", rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("")));
    let status = ssh(host, &format!("mkdir -p {dir} && cat > {dest}.part && mv {dest}.part {dest}"))
        .stdin(Stdio::from(fs::File::open(root.join(rel))?))
        .status()?;
    if !status.success() {
        bail!("could not copy {rel} to the Kindle");
    }
    Ok(())
}

pub fn sync(dry_run: bool, force: bool) -> Result<()> {
    let host = config("ssh").context("set the Kindle's ssh host: bookos config ssh kindle")?;
    let root = drive_dir()?;
    fs::create_dir_all(&root).with_context(|| format!("could not create {}", root.display()))?;
    let kindle = kindle_files(&host)?;
    let drive = drive_files(&root);
    let state = load_state();
    let p = plan(&kindle, &drive, &state);
    let drops = p.drop_drive.len() + p.drop_kindle.len();
    if !force && drops > 5 && drops * 4 > state.len() {
        bail!("{drops} books would be deleted ({} on the Kindle, {} in the folder); run with --force if that is right", p.drop_kindle.len(), p.drop_drive.len());
    }
    let mut report: BTreeMap<&str, usize> = BTreeMap::new();
    let mut failed = BTreeSet::new();
    for f in &p.to_drive {
        println!("kindle → folder  {f}");
        if !dry_run {
            if let Err(e) = pull(&host, f, &root) {
                eprintln!("  {e}");
                failed.insert(f.clone());
            }
        }
        *report.entry("copied to the folder").or_default() += 1;
    }
    for f in &p.to_kindle {
        println!("folder → kindle  {f}");
        if !dry_run {
            if let Err(e) = push(&host, f, &root) {
                eprintln!("  {e}");
                failed.insert(f.clone());
            }
        }
        *report.entry("copied to the Kindle").or_default() += 1;
    }
    for f in &p.drop_kindle {
        println!("delete on kindle {f}");
        if !dry_run {
            let _ = ssh(&host, &format!("rm -f {}", quote(&format!("{KINDLE_ROOT}/{f}")))).status();
        }
        *report.entry("deleted on the Kindle").or_default() += 1;
    }
    for f in &p.drop_drive {
        println!("delete in folder {f}");
        if !dry_run {
            let _ = fs::remove_file(root.join(f));
        }
        *report.entry("deleted in the folder").or_default() += 1;
    }
    if dry_run {
        println!("dry run: nothing changed");
        return Ok(());
    }
    let mut now: BTreeSet<String> = kindle.union(&drive).cloned().collect();
    for f in p.drop_kindle.iter().chain(&p.drop_drive).chain(&failed) {
        now.remove(f);
    }
    save_state(&now)?;
    if report.is_empty() {
        println!("in sync: {} books", now.len());
    } else {
        let parts: Vec<String> = report.iter().map(|(k, v)| format!("{v} {k}")).collect();
        println!("{} · {} books in sync", parts.join(", "), now.len());
    }
    Ok(())
}
