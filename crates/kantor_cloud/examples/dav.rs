//! A tiny WebDAV command line over `kantor_cloud`.
//!
//! ```text
//! export KANTOR_DAV_URL=http://localhost:8080/remote.php/dav/files/admin/
//! export KANTOR_DAV_USER=admin KANTOR_DAV_PASS=app-password
//! cargo run -p kantor_cloud --example dav -- ls [dir]
//! cargo run -p kantor_cloud --example dav -- get <remote> [local]
//! cargo run -p kantor_cloud --example dav -- put <local> <remote> [--etag E | --new | --force]
//! cargo run -p kantor_cloud --example dav -- open <remote>      # GET, then open in sheets/docs engine
//! cargo run -p kantor_cloud --example dav -- lock <remote> | unlock <remote> <token>
//! ```

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use kantor_cloud::{Auth, Client, LockToken, Precondition};

fn client() -> Result<Client> {
    let url = std::env::var("KANTOR_DAV_URL").context("set KANTOR_DAV_URL")?;
    let auth = match std::env::var("KANTOR_DAV_TOKEN") {
        Ok(t) => Auth::Bearer(t),
        Err(_) => match std::env::var("KANTOR_DAV_USER") {
            Ok(user) => Auth::Basic {
                user,
                password: std::env::var("KANTOR_DAV_PASS").unwrap_or_default(),
            },
            Err(_) => Auth::None,
        },
    };
    Ok(Client::new(&url, auth))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).map(String::as_str);
    let c = client()?;
    match arg(0) {
        Some("ls") => {
            let t = Instant::now();
            let entries = c.list(arg(1).unwrap_or(""))?;
            for e in &entries {
                println!(
                    "{:>1} {:>10} {:<24} {}",
                    if e.is_dir { "d" } else { "-" },
                    e.size.map(|s| s.to_string()).unwrap_or_default(),
                    e.etag.as_deref().unwrap_or(""),
                    e.name
                );
            }
            eprintln!("{} entries in {:?}", entries.len(), t.elapsed());
        }
        Some("get") => {
            let remote = arg(1).context("get <remote> [local]")?;
            let t = Instant::now();
            let f = c.get(remote)?;
            let local = arg(2).unwrap_or_else(|| remote.rsplit('/').next().unwrap_or(remote));
            std::fs::write(local, &f.bytes)?;
            eprintln!(
                "{} bytes, etag {:?}, {:?} -> {local}",
                f.bytes.len(),
                f.etag,
                t.elapsed()
            );
        }
        Some("put") => {
            let (local, remote) = (
                arg(1).context("put <local> <remote>")?,
                arg(2).context("put <local> <remote>")?,
            );
            let bytes = std::fs::read(local)?;
            let cond = match (arg(3), arg(4)) {
                (Some("--etag"), Some(e)) => Precondition::IfMatch(e),
                (Some("--new"), _) => Precondition::IfNoneMatch,
                (Some("--force"), _) | (None, _) => Precondition::Overwrite,
                _ => bail!("expected --etag E, --new or --force"),
            };
            let t = Instant::now();
            let etag = c.put(remote, &bytes, cond, None)?;
            eprintln!(
                "{} bytes, new etag {etag:?}, {:?}",
                bytes.len(),
                t.elapsed()
            );
        }
        Some("open") => {
            let remote = arg(1).context("open <remote>")?;
            let t = Instant::now();
            let f = c.get(remote)?;
            let fetched = t.elapsed();
            let t = Instant::now();
            let summary = open_bytes(remote, f.bytes)?;
            println!("{summary}");
            eprintln!(
                "download {fetched:?}, open {:?}, etag {:?}",
                t.elapsed(),
                f.etag
            );
        }
        Some("lock") => {
            let remote = arg(1).context("lock <remote>")?;
            let tok = c.lock(remote, Duration::from_secs(1800))?;
            println!("{}", tok.0);
        }
        Some("unlock") => {
            let (remote, tok) = (
                arg(1).context("unlock <remote> <token>")?,
                arg(2).context("unlock <remote> <token>")?,
            );
            c.unlock(remote, &LockToken(tok.to_string()))?;
        }
        _ => bail!("usage: dav ls|get|put|open|lock|unlock ... (see the source header)"),
    }
    Ok(())
}

/// Open downloaded bytes the way the app would. Docs reads straight from
/// memory; Sheets opens by path (its reader is path-based), so the bytes go
/// to a temporary file first, which is also what an offline copy would be.
fn open_bytes(name: &str, bytes: Vec<u8>) -> Result<String> {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "docx" => {
            let read = docs_engine::docx::read(bytes)?;
            let doc = docs_engine::Document::from_read(read, None);
            let words: usize = doc
                .paragraphs()
                .iter()
                .map(|p| p.text.split_whitespace().count())
                .sum();
            Ok(format!(
                "docx: {} paragraphs, {words} words",
                doc.paragraphs().len()
            ))
        }
        "xlsx" | "xlsm" | "ods" | "xls" | "csv" => {
            let dir = tempfile::tempdir()?;
            let path = dir.path().join(format!("download.{ext}"));
            std::fs::write(&path, &bytes)?;
            let wb = sheets_engine::Workbook::open(&path)?;
            Ok(format!("{ext}: sheets {:?}", wb.sheet_names()))
        }
        _ => bail!("don't know how to open .{ext}"),
    }
}
