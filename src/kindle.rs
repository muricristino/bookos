use crate::config;
use anyhow::{Context, Result, bail};
use lettre::message::header::ContentType;
use lettre::message::{Attachment, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};
use std::fs;
use std::path::{Path, PathBuf};

const EMAIL_FORMATS: &[&str] = &["epub", "pdf", "txt", "doc", "docx", "rtf", "htm", "html", "jpg", "jpeg", "png", "gif", "bmp"];
const USB_FORMATS: &[&str] = &["mobi", "azw", "azw3", "kfx", "prc", "pdf", "txt"];
const EMAIL_LIMIT: u64 = 50 * 1024 * 1024;
const KEYRING_SERVICE: &str = "bookos";

fn ext(file: &Path) -> String {
    file.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase()
}

pub fn usb_volume() -> Option<PathBuf> {
    fs::read_dir("/Volumes").ok()?.flatten().map(|e| e.path()).find(|p| {
        p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.to_lowercase().contains("kindle")) && p.join("documents").is_dir()
    })
}

pub fn send(file: &Path, via: Option<&str>) -> Result<()> {
    let ext = ext(file);
    let via = match via {
        Some(v) => v.to_string(),
        None if usb_volume().is_some() && USB_FORMATS.contains(&ext.as_str()) => "usb".into(),
        None => "email".into(),
    };
    match via.as_str() {
        "usb" => usb(file, &ext),
        "email" => email(file, &ext),
        other => bail!("unknown way to send: {other} (use email or usb)"),
    }
}

fn usb(file: &Path, ext: &str) -> Result<()> {
    if !USB_FORMATS.contains(&ext) {
        bail!("a Kindle over USB doesn't read .{ext}; use {} or send it by email", USB_FORMATS.join(", "));
    }
    let volume = usb_volume().context("no Kindle mounted in /Volumes; plug it in over USB")?;
    let dest = volume.join("documents").join(file.file_name().context("bad file name")?);
    fs::copy(file, &dest).with_context(|| format!("could not copy to {}", dest.display()))?;
    println!("copied to {}", dest.display());
    println!("eject the Kindle and the book shows up in the library");
    Ok(())
}

fn smtp_server(user: &str) -> Result<(String, u16)> {
    if let Some(s) = config("smtp") {
        return Ok(match s.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.parse().context("bad port in smtp=")?),
            None => (s, 587),
        });
    }
    let domain = user.rsplit('@').next().unwrap_or("").to_lowercase();
    let host = match domain.as_str() {
        "gmail.com" | "googlemail.com" => "smtp.gmail.com",
        "outlook.com" | "hotmail.com" | "live.com" => "smtp-mail.outlook.com",
        "icloud.com" | "me.com" | "mac.com" => "smtp.mail.me.com",
        "yahoo.com" => "smtp.mail.yahoo.com",
        _ => bail!("set the SMTP server: bookos config smtp smtp.example.com:587"),
    };
    Ok((host.into(), 587))
}

pub fn password_entry(user: &str) -> Result<keyring::Entry> {
    Ok(keyring::Entry::new(KEYRING_SERVICE, user)?)
}

fn password(user: &str) -> Result<String> {
    if let Ok(p) = std::env::var("BOOKOS_SMTP_PASS") {
        return Ok(p);
    }
    password_entry(user)?.get_password().with_context(|| format!("no SMTP password for {user}; set it with: bookos config smtp_pass"))
}

fn email(file: &Path, ext: &str) -> Result<()> {
    if !EMAIL_FORMATS.contains(&ext) {
        bail!(
            "Send to Kindle by email doesn't take {} files; send an epub or pdf instead, or plug the Kindle in over USB and use --via usb",
            if ext.is_empty() { "these".to_string() } else { format!(".{ext}") }
        );
    }
    let size = fs::metadata(file)?.len();
    if size > EMAIL_LIMIT {
        bail!("the file is {} MB; Send to Kindle by email takes up to 50 MB", size / 1_048_576);
    }
    let to = config("kindle").context("set your Send to Kindle address: bookos config kindle you@kindle.com")?;
    if let Some(command) = config("mail_command") {
        return run_mail_command(&command, &to, file);
    }
    let user = config("smtp_user").context("set the email account that sends: bookos config smtp_user you@gmail.com")?;
    let (host, port) = smtp_server(&user)?;
    let pass = password(&user)?;

    let name = file.file_name().and_then(|n| n.to_str()).context("bad file name")?.to_string();
    let title = file.file_stem().and_then(|n| n.to_str()).unwrap_or(&name).to_string();
    let mime = match ext {
        "epub" => "application/epub+zip",
        "pdf" => "application/pdf",
        "txt" => "text/plain; charset=utf-8",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "rtf" => "application/rtf",
        "htm" | "html" => "text/html; charset=utf-8",
        "png" => "image/png",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        _ => "image/jpeg",
    };
    let message = Message::builder()
        .from(user.parse().context("bad smtp_user address")?)
        .to(to.parse().context("bad kindle address")?)
        .subject(&title)
        .multipart(
            MultiPart::mixed()
                .singlepart(SinglePart::plain(String::from("Sent by bookos")))
                .singlepart(Attachment::new(name).body(fs::read(file)?, ContentType::parse(mime)?)),
        )?;
    let builder = if port == 465 { SmtpTransport::relay(&host)? } else { SmtpTransport::starttls_relay(&host)? };
    let transport = builder.port(port).credentials(Credentials::new(user.clone(), pass)).build();
    eprintln!("sending to {to} through {host}…");
    transport.send(&message).with_context(|| format!("{host}:{port} refused the email"))?;
    println!("sent to {to}");
    println!("if it doesn't show up, add {user} to the approved senders in Amazon's Personal Document Settings");
    Ok(())
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn run_mail_command(command: &str, to: &str, file: &Path) -> Result<()> {
    let subject = file.file_stem().and_then(|n| n.to_str()).unwrap_or("book");
    let command = command
        .replace("{to}", &quote(to))
        .replace("{subject}", &quote(subject))
        .replace("{file}", &quote(&file.to_string_lossy()));
    eprintln!("sending to {to} with mail_command…");
    let status = std::process::Command::new("sh").arg("-c").arg(&command).status().context("could not run mail_command")?;
    if !status.success() {
        bail!("mail_command failed ({status})");
    }
    println!("sent to {to}");
    Ok(())
}
