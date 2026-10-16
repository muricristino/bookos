use anyhow::{Result, bail};
use std::time::Duration;

const MIRRORS: [&str; 5] = ["libgen.li", "libgen.gl", "libgen.la", "libgen.bz", "libgen.vg"];

fn mirrors() -> Vec<String> {
    match crate::config("libgen") {
        Some(list) => list.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        None => MIRRORS.iter().map(|s| s.to_string()).collect(),
    }
}

pub fn link(md5: &str) -> Result<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(6)))
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut errors = Vec::new();
    for host in mirrors() {
        let url = format!("https://{host}/ads.php?md5={md5}");
        let body = match agent.get(&url).header("User-Agent", "Mozilla/5.0").call() {
            Ok(r) if r.status() == 200 => r.into_body().read_to_string().unwrap_or_default(),
            Ok(r) => {
                errors.push(format!("{host}: HTTP {}", r.status()));
                continue;
            }
            Err(e) => {
                errors.push(format!("{host}: {e}"));
                continue;
            }
        };
        let Some(start) = body.find("get.php?md5=") else {
            errors.push(format!("{host}: no download link on the page"));
            continue;
        };
        let href: String = body[start..].chars().take_while(|c| *c != '"' && *c != '\'').collect();
        return Ok(format!("https://{host}/{}", href.replace("&amp;", "&")));
    }
    bail!("{}", errors.join("; "))
}
