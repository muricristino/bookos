# bookos

A command-line tool to search and download books from [Anna's Archive](https://annas-archive.gl) and Libgen, and send them to your Kindle.

![bookos demo](assets/demo.gif)

[Watch in full quality (MP4)](assets/demo.mp4)

```
$ bookos search dom casmurro -f epub -l pt -n 3
  1  Dom Casmurro (Prazer de Ler) (Portuguese Edition)
     de Assis, Machado · 2016 · pt · epub · 0.5MB · lgli/zlib
     6929bd3a14ab735fbd2c7a190d8728fb
  2  Dom Casmurro
     De Assis, Machado · 2000 · pt · epub · 0.3MB · lgli/lgrs/zlib
     db3ba212ff208c9ebad4b3bd88bba98e
  ...

download with: bookos download <number>

$ bookos download 1
Dom Casmurro (Prazer de Ler) (Portuguese Edition)
de Assis, Machado
aa-slow: getting the link from server #1…
  Please wait 52 seconds to download this file
aa-slow: downloading…
  0.4 MB  done
saved to ~/Downloads/de Assis, Machado - Dom Casmurro (Prazer de Ler) (Portuguese Edition) (2016).epub
```

## What it does

- **Search** goes through Anna's Archive, which indexes Libgen (.rs and .li), Z-Library, Sci-Hub, the Internet Archive lending library, DuXiu, HathiTrust and direct uploads. Each result shows title, author, publisher, year, language, format, size, the collections that hold the file, and its md5.
- **Download** identifies a file by its md5 (the same across all these libraries) and tries the sources in order until one works:
  1. `aa-fast`: the official Anna's Archive fast download API. Needs a membership secret key; skipped otherwise.
  2. `aa-slow`: the free "slow partner server" download. It runs through Chrome, waits out the queue countdown, grabs the signed download link and fetches the file over plain HTTP.
  3. `libgen`: the Libgen.li family of mirrors (`ads.php` → `get.php`).
- Every download is **checked against its md5**. A mismatching file is discarded.
- Interrupted downloads **resume**: the partial file is kept as `<name>.part`. Run the same command again to continue it with an HTTP `Range` request.
- Files are saved as `Author - Title (Year).ext`.

## Requirements

- macOS
- Google Chrome installed at `/Applications/Google Chrome.app`
- Rust 1.88 or newer (edition 2024), to build it

## Install

```
cargo install --git https://github.com/muricristino/bookos
```

Or from source:

```
git clone https://github.com/muricristino/bookos
cd bookos
cargo build --release
cp target/release/bookos ~/.local/bin/
```

## Usage

### Search

```
bookos search <terms...> [-f FORMAT] [-l LANG] [-n MAX] [--json]
```

| Flag | Description |
|---|---|
| `-f, --format` | File format: `pdf`, `epub`, `mobi`, `azw3`, `djvu`… |
| `-l, --lang` | Language code: `en`, `pt`, `es`… |
| `-n, --max` | How many results to show (default 10) |
| `--json` | Print results as JSON |

The terms can be a title, an author, an ISBN, or an md5. The results are saved to `~/.config/bookos/last-search.json`, so you can download by number afterwards.

```
bookos search the left hand of darkness
bookos search 9788525054463
bookos search machado de assis -f epub -l pt -n 20
```

### Download

```
bookos download <number|md5> [-d DIR] [--source SOURCE]
```

| Flag | Description |
|---|---|
| `-d, --dir` | Destination directory (default `~/Downloads`, or `dir` from the config) |
| `--source` | Use only this source: `aa-fast`, `aa-slow` or `libgen` (a comma-separated list also works) |

```
bookos download 2                                   # result #2 from the last search
bookos download 7efc25320899bd1151b6ef1c8d41b6cc    # any md5, looked up on Anna's Archive
bookos download 1 -d ~/Books --source libgen
```

### Send to Kindle

```
bookos kindle <file|number|md5> [--via email|usb|ssh] [-d DIR]
bookos download <number|md5> --kindle
```

A number or md5 is downloaded first. With a Kindle plugged in over USB and a format it reads (`mobi`, `azw3`, `pdf`, `txt`), the file is copied into its `documents` folder. Otherwise it goes by email to your Send to Kindle address, which takes `epub`, `pdf`, `txt`, `docx`, `rtf`, `html` and images up to 50 MB (not `mobi` or `azw3` anymore).

One-time setup for email:

```
bookos config kindle you@kindle.com       # Amazon › Manage Content and Devices › Preferences › Personal Document Settings
bookos config smtp_user you@gmail.com     # the account that sends; for Gmail use an app password
bookos config smtp_pass                   # asks for the password and keeps it in the macOS Keychain
```

Add the `smtp_user` address to the **Approved Personal Document E-mail List** on that same Amazon page, or Amazon drops the email. The SMTP server is guessed for Gmail, Outlook, iCloud and Yahoo; set others with `bookos config smtp smtp.example.com:587`. `BOOKOS_SMTP_PASS` overrides the Keychain.

To send with another mail tool instead of SMTP (for example an Outlook account that needs OAuth), set `mail_command`. `{to}`, `{subject}` and `{file}` are replaced, shell-quoted:

```
bookos config mail_command "mymailer --to {to} --subject {subject} --attach {file}"
```

On a jailbroken Kindle with an ssh server (for example the dropbear that ships with KOReader), `--via ssh` copies any format straight into `/mnt/us/documents` over Wi-Fi:

```
bookos config ssh kindle                  # a Host in ~/.ssh/config, or root@192.168.0.20
bookos kindle book.epub --via ssh
```

Amazon sometimes answers with a "verify your request" email. Confirm it from the terminal with the link in it:

```
bookos verify https://www.amazon.com/sendtokindle/verification/confirm/…
```

### Config

```
bookos config                  # print the config
bookos config dir              # print one key
bookos config dir ~/Books      # set a key
```

### Global flag

| Flag | Description |
|---|---|
| `--show` | Show the Chrome window instead of running it headless (see below) |

## Configuration

The optional file `~/.config/bookos/config` holds plain `key=value` lines:

```
key=your-anna's-archive-secret-key
dir=~/Books
sources=aa-fast,aa-slow,libgen
domain=annas-archive.gl,annas-archive.pk,annas-archive.gd
libgen=libgen.li,libgen.gl,libgen.la,libgen.bz,libgen.vg
kindle=you@kindle.com
smtp_user=you@gmail.com
smtp=smtp.gmail.com:587
```

| Key | Default | Description |
|---|---|---|
| `key` | none | Anna's Archive membership secret key (from your account page). Enables `aa-fast`. The `AA_KEY` environment variable also works. |
| `dir` | `~/Downloads` | Where downloads go |
| `sources` | `aa-fast,aa-slow,libgen` | Source order |
| `domain` | `annas-archive.gl,annas-archive.pk,annas-archive.gd` | Anna's Archive domains, tried in order |
| `libgen` | `libgen.li,libgen.gl,libgen.la,libgen.bz,libgen.vg` | Libgen mirrors, tried in order |
| `kindle` | none | Your Send to Kindle email address |
| `smtp_user` | none | Email account that sends to the Kindle |
| `ssh` | none | ssh host of a jailbroken Kindle, used by `--via ssh` |
| `mail_command` | none | Shell command that sends the email instead of SMTP |
| `smtp` | guessed from `smtp_user` | SMTP server as `host:port` (465 uses TLS, anything else STARTTLS) |

## How the Chrome part works

Anna's Archive sits behind DDoS-Guard. Its JavaScript browser check blocks `curl`, plain HTTP clients and the usual automation tools: Playwright, `headless_chrome`, and anything with a DevTools session attached while the check runs. Its cookies don't work outside the browser either, because the client is fingerprinted.

So bookos starts your real Google Chrome with a dedicated profile in `~/.config/bookos/chrome`, headless by default. It opens pages through Chrome's DevTools HTTP endpoint and only attaches over WebSocket once the page has loaded, to read the results with a small script. The profile keeps the DDoS-Guard cookies, so after the first run (about 20 s) searches take a few seconds. Only page loads go through Chrome. File downloads use plain HTTP.

If DDoS-Guard asks for a manual check, run the same command with `--show`. A Chrome window opens, you solve the check there, and bookos continues. The profile remembers it afterwards.

The `aa-fast` API (`/dyn/api/fast_download.json`) is not behind the browser check, so with a member key, downloads don't need Chrome at all.

## Troubleshooting

- **`the site answered 502 Bad Gateway`**: Anna's Archive is overloaded. bookos falls through to the next domain on its own. You can change the list with `domain=` in the config.
- **`the site asked for a manual check`**: run again with `--show` and solve the check in the window.
- **Libgen times out**: some ISPs and networks block the Libgen mirrors. Put working mirrors in `libgen=`, or rely on the Anna's Archive sources.
- **Slow downloads**: the free slow servers have a queue (often up to a minute) and usually deliver around 100 KB/s. A membership key (`key=`) switches to the fast servers.
- **`the downloaded file does not match the md5`**: the server sent something else, often an error page. The file was discarded. Try again or use another `--source`.
- **Download stopped halfway**: run the same command again to resume from the `.part` file.

## Legal

bookos is a client for publicly reachable websites. Many files on these libraries are under copyright. You are responsible for complying with the copyright law of your jurisdiction.
