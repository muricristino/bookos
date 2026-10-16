use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

const CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

pub fn profile_dir() -> PathBuf {
    crate::home().join(".config/bookos/chrome")
}

pub fn kill_all() {
    let _ = Command::new("pkill")
        .arg("-f")
        .arg(format!("^/Applications/Google Chrome.*--user-data-dir={}", profile_dir().display()))
        .status();
}

fn user_agent() -> String {
    let out = Command::new(CHROME).arg("--version").output().ok();
    let version = out.map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    let major = version.split_whitespace().last().and_then(|v| v.split('.').next()).unwrap_or("141").to_string();
    format!("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36")
}

pub struct Chrome {
    child: Child,
    port: u16,
    http: ureq::Agent,
    visible: bool,
}

pub struct Tab<'a> {
    chrome: &'a Chrome,
    id: String,
}

impl Chrome {
    // The DDoS-Guard in front of Anna's Archive flags any page with a CDP session attached while
    // its challenge runs, so pages are opened through the HTTP endpoint and only attached to after.
    pub fn open(visible: bool) -> Result<Chrome> {
        kill_all();
        let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
        let mut cmd = Command::new(CHROME);
        cmd.arg(format!("--user-data-dir={}", profile_dir().display()))
            .arg(format!("--remote-debugging-port={port}"))
            .arg(format!("--user-agent={}", user_agent()))
            .args(["--no-first-run", "--no-default-browser-check", "--lang=en-US"]);
        if visible {
            cmd.arg("--window-size=1100,900");
        } else {
            cmd.arg("--headless=new");
        }
        let child = cmd.arg("about:blank").stdout(Stdio::null()).stderr(Stdio::null()).spawn().context("could not start Chrome")?;
        let http: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(10)))
            .http_status_as_error(false)
            .build()
            .into();
        let chrome = Chrome { child, port, http, visible };
        let t = Instant::now();
        while chrome.get("/json/version").is_null() {
            if t.elapsed() > Duration::from_secs(20) {
                bail!("Chrome did not respond on port {port}");
            }
            sleep(Duration::from_millis(200));
        }
        Ok(chrome)
    }

    fn get(&self, path: &str) -> Value {
        let url = format!("http://127.0.0.1:{}{path}", self.port);
        let resp = if path.starts_with("/json/new") { self.http.put(&url).send_empty() } else { self.http.get(&url).call() };
        resp.ok().and_then(|r| r.into_body().read_json().ok()).unwrap_or(Value::Null)
    }

    fn title(&self, id: &str) -> Option<String> {
        let list = self.get("/json/list");
        list.as_array()?.iter().find(|p| p["id"] == id).map(|p| p["title"].as_str().unwrap_or("").to_string())
    }

    pub fn tab(&self, url: &str) -> Result<Tab<'_>> {
        let page = self.get(&format!("/json/new?{}", url.replace('%', "%25").replace('&', "%26").replace('#', "%23")));
        let id = page["id"].as_str().context("Chrome did not open the tab")?.to_string();
        let tab = Tab { chrome: self, id };
        tab.wait_loaded()?;
        let title = self.title(&tab.id).unwrap_or_default();
        let http_error = title.split_whitespace().next().is_some_and(|w| w.len() == 3 && (w.starts_with('4') || w.starts_with('5')) && w.chars().all(|c| c.is_ascii_digit()));
        if http_error {
            bail!("the site answered {title}");
        }
        Ok(tab)
    }
}

impl Drop for Chrome {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Tab<'_> {
    fn wait_loaded(&self) -> Result<()> {
        let limit = Duration::from_secs(if self.chrome.visible { 300 } else { 60 });
        let t = Instant::now();
        let mut warned = false;
        loop {
            sleep(Duration::from_millis(700));
            let title = self.chrome.title(&self.id).unwrap_or_default();
            let lower = title.to_lowercase();
            let checking = lower.contains("ddos") || lower.contains("checking your browser") || title.is_empty() || title.starts_with("http");
            if !checking {
                break;
            }
            if t.elapsed() > Duration::from_secs(8) && !warned {
                eprintln!("  passing the site's browser check…");
                warned = true;
            }
            if t.elapsed() > limit {
                if self.chrome.visible {
                    bail!("the site's browser check did not finish");
                }
                bail!("the site asked for a manual check; run again with --show and solve it in the Chrome window");
            }
        }
        let t = Instant::now();
        while self.eval("document.readyState")?.as_str() != Some("complete") && t.elapsed() < Duration::from_secs(30) {
            sleep(Duration::from_millis(300));
        }
        Ok(())
    }

    pub fn eval(&self, expression: &str) -> Result<Value> {
        let ws_url = format!("ws://127.0.0.1:{}/devtools/page/{}", self.chrome.port, self.id);
        let (mut ws, _) = tungstenite::connect(&ws_url).context("could not talk to the Chrome tab")?;
        let msg = json!({"id": 1, "method": "Runtime.evaluate", "params": {"expression": expression, "returnByValue": true}});
        ws.send(tungstenite::Message::text(msg.to_string()))?;
        loop {
            let text = ws.read()?.into_text()?;
            let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            if v["id"] == 1 {
                let _ = ws.close(None);
                if let Some(e) = v["result"]["exceptionDetails"].as_object() {
                    bail!("page script error: {}", e.get("text").and_then(|t| t.as_str()).unwrap_or("?"));
                }
                return Ok(v["result"]["result"]["value"].clone());
            }
        }
    }

    pub fn eval_json(&self, expression: &str) -> Result<Value> {
        let s = self.eval(expression)?;
        Ok(serde_json::from_str(s.as_str().unwrap_or("null"))?)
    }
}

impl Drop for Tab<'_> {
    fn drop(&mut self) {
        let _ = self.chrome.get(&format!("/json/close/{}", self.id));
    }
}
