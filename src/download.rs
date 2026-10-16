use anyhow::{Context, Result, bail};
use md5::{Digest, Md5};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}

pub fn fetch(url: &str, dest: &Path, expected_md5: &str) -> Result<()> {
    let part = PathBuf::from(format!("{}.part", dest.display()));
    let mut hasher = Md5::new();
    let mut have = 0u64;
    if let Ok(mut f) = File::open(&part) {
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            have += n as u64;
        }
    }

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent.get(url).header("User-Agent", "Mozilla/5.0");
    if have > 0 {
        req = req.header("Range", format!("bytes={have}-"));
    }
    let resp = req.call().context("could not connect to the file server")?;
    let status = resp.status().as_u16();
    let mut file = match status {
        206 if have > 0 => {
            eprintln!("  resuming from {}", mb(have));
            OpenOptions::new().append(true).open(&part)?
        }
        200 => {
            have = 0;
            hasher = Md5::new();
            File::create(&part)?
        }
        416 if have > 0 => OpenOptions::new().append(true).open(&part)?,
        _ => bail!("the server answered HTTP {status}"),
    };
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .map(|len| if status == 206 { len + have } else { len });

    if status != 416 {
        let mut body = resp.into_body();
        let mut reader = body.as_reader();
        let mut buf = vec![0u8; 1 << 16];
        let start = Instant::now();
        let start_have = have;
        let mut last = Instant::now() - Duration::from_secs(1);
        loop {
            let n = reader.read(&mut buf).context("the connection dropped; run again to resume")?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])?;
            hasher.update(&buf[..n]);
            have += n as u64;
            if last.elapsed() >= Duration::from_millis(400) {
                let speed = (have - start_have) as f64 / start.elapsed().as_secs_f64().max(0.001);
                let of = total.map(|t| format!(" / {}", mb(t))).unwrap_or_default();
                eprint!("\r  {}{of}  {}/s     ", mb(have), mb(speed as u64));
                last = Instant::now();
            }
        }
        eprintln!("\r  {}  done                    ", mb(have));
    }
    file.flush()?;
    drop(file);

    if let Some(t) = total
        && have < t
    {
        bail!("the download stopped at {} of {}; run again to resume", mb(have), mb(t));
    }
    let got = format!("{:x}", hasher.finalize());
    if got != expected_md5 {
        fs::remove_file(&part)?;
        bail!("the downloaded file does not match the md5 (got {got}); discarded");
    }
    fs::rename(&part, dest)?;
    Ok(())
}
