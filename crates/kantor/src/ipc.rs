//! The local endpoint `kantor mcp` talks to: one Unix domain socket per
//! running Kantor, in a private directory.
//!
//! ```text
//! Linux   $XDG_RUNTIME_DIR/kantor/kantor-<pid>.sock   (else ~/.local/state/kantor/run/)
//! macOS   ~/Library/Application Support/Kantor/run/kantor-<pid>.sock
//! ```
//!
//! (`KANTOR_SOCKET_DIR` overrides the directory, for tests.) The directory is
//! `0700` and the socket `0600`, so only the user can connect. Messages are
//! one JSON object per line:
//!
//! ```text
//! → {"id": 1, "tool": "list_documents", "args": {}}
//! ← {"id": 1, "result": …}   or   {"id": 1, "error": "…"}
//! ```
//!
//! Socket I/O runs on background threads; each request is handed to the
//! GPUI main thread through a channel ([`Request`]) and answered from there,
//! so a slow or stuck client never blocks the UI.

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use serde_json::{Value, json};

/// Longest message either side accepts (a large `write_range` fits).
pub const MAX_MESSAGE: usize = 8 * 1024 * 1024;
/// How long a client waits for the app to answer one request.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(60);
/// Clients connected at once.
const MAX_CLIENTS: usize = 8;

const PREFIX: &str = "kantor-";
const SUFFIX: &str = ".sock";

/// Where running instances put their sockets.
pub fn socket_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("KANTOR_SOCKET_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return home.map(|h| h.join("Library/Application Support/Kantor/run"));
    }
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        Some(run) => Some(PathBuf::from(run).join("kantor")),
        None => home.map(|h| h.join(".local/state/kantor/run")),
    }
}

/// Sockets in `dir`, newest first.
pub fn sockets_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with(PREFIX) && name.ends_with(SUFFIX)
        })
        .map(|e| {
            let modified = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (modified, e.path())
        })
        .collect();
    found.sort_by_key(|a| std::cmp::Reverse(a.0));
    found.into_iter().map(|(_, p)| p).collect()
}

/// Connect to `path`; a socket nobody listens on any more (a crashed
/// instance) is removed.
pub fn connect(path: &Path) -> io::Result<UnixStream> {
    match UnixStream::connect(path) {
        Ok(s) => Ok(s),
        Err(e) => {
            if e.kind() == io::ErrorKind::ConnectionRefused {
                let _ = fs::remove_file(path);
            }
            Err(e)
        }
    }
}

/// Read one `\n`-terminated line of at most `max` bytes. `Ok(None)` at end
/// of input; an `InvalidData` error for a longer line or invalid UTF-8.
pub fn read_line(reader: &mut impl BufRead, max: usize) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    let n = reader.take(max as u64 + 1).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
    } else if buf.len() > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("message longer than {} MB", max / (1024 * 1024)),
        ));
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "message is not UTF-8"))
}

/// One tool call for the main thread. Answer it with [`Request::respond`];
/// dropping it answers with an error.
pub struct Request {
    pub tool: String,
    pub args: Value,
    reply: mpsc::SyncSender<Result<Value, String>>,
}

impl Request {
    pub fn respond(self, result: Result<Value, String>) {
        // The client may have given up waiting; nothing to do then.
        let _ = self.reply.send(result);
    }
}

/// The app's endpoint. The socket file is removed on drop (and by
/// [`Server::remove`] when quitting, since `exit` skips destructors).
pub struct Server {
    pub path: PathBuf,
}

impl Server {
    pub fn remove(&self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.remove();
    }
}

/// Create the private directory for sockets.
fn make_private_dir(dir: &Path) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    // An existing directory may have been made with looser permissions.
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

/// Start listening in `dir`. Requests arrive on the returned channel.
pub fn serve(dir: &Path) -> io::Result<(Server, UnboundedReceiver<Request>)> {
    make_private_dir(dir)?;
    let path = dir.join(format!("{PREFIX}{}{SUFFIX}", std::process::id()));
    let _ = fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    let (tx, rx) = unbounded();
    let own = path.clone();
    let dir = dir.to_path_buf();
    std::thread::Builder::new()
        .name("kantor-ipc".into())
        .spawn(move || {
            // Sockets left by instances that crashed.
            for other in sockets_in(&dir).into_iter().filter(|p| *p != own) {
                let _ = connect(&other);
            }
            accept_loop(listener, tx)
        })?;
    Ok((Server { path }, rx))
}

fn accept_loop(listener: UnixListener, tx: UnboundedSender<Request>) {
    let clients = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else {
            continue;
        };
        if clients.load(Ordering::SeqCst) >= MAX_CLIENTS {
            let _ = writeln!(
                stream,
                "{}",
                json!({"id": null, "error": "too many clients connected to Kantor"})
            );
            continue;
        }
        clients.fetch_add(1, Ordering::SeqCst);
        let (client_tx, count) = (tx.clone(), clients.clone());
        let spawned = std::thread::Builder::new()
            .name("kantor-ipc-client".into())
            .spawn(move || {
                if let Err(e) = serve_client(stream, &client_tx) {
                    log::debug!("MCP client: {e}");
                }
                count.fetch_sub(1, Ordering::SeqCst);
            });
        if spawned.is_err() {
            clients.fetch_sub(1, Ordering::SeqCst);
            log::warn!("can't start a thread for an MCP client");
        }
        if tx.is_closed() {
            break;
        }
    }
}

/// Answer one client's requests until it disconnects.
fn serve_client(stream: UnixStream, tx: &UnboundedSender<Request>) -> io::Result<()> {
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    loop {
        let line = match read_line(&mut reader, MAX_MESSAGE) {
            Ok(Some(line)) => line,
            Ok(None) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                writeln!(writer, "{}", json!({"id": null, "error": e.to_string()}))?;
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        if line.trim().is_empty() {
            continue;
        }
        let reply = answer(&line, tx);
        writeln!(writer, "{reply}")?;
        writer.flush()?;
    }
}

/// The response line for one request line.
fn answer(line: &str, tx: &UnboundedSender<Request>) -> Value {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return json!({"id": null, "error": format!("invalid JSON: {e}")}),
    };
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let Some(tool) = msg.get("tool").and_then(Value::as_str) else {
        return json!({"id": id, "error": "missing \"tool\""});
    };
    let args = msg.get("args").cloned().unwrap_or_else(|| json!({}));
    let (reply, wait) = mpsc::sync_channel(1);
    let request = Request {
        tool: tool.to_owned(),
        args,
        reply,
    };
    if tx.unbounded_send(request).is_err() {
        return json!({"id": id, "error": "Kantor is quitting"});
    }
    match wait.recv_timeout(REPLY_TIMEOUT) {
        Ok(Ok(result)) => json!({"id": id, "result": result}),
        Ok(Err(error)) => json!({"id": id, "error": error}),
        Err(mpsc::RecvTimeoutError::Timeout) => json!({
            "id": id,
            "error": "Kantor didn't answer within a minute (busy, or a dialog is open); the request may still run"
        }),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            json!({"id": id, "error": "Kantor dropped the request (its window may be closed)"})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt as _;

    #[std::prelude::v1::test]
    fn reads_bounded_lines() {
        let mut r = io::Cursor::new(b"one\r\ntwo\nthree".to_vec());
        assert_eq!(read_line(&mut r, 10).unwrap().as_deref(), Some("one"));
        assert_eq!(read_line(&mut r, 10).unwrap().as_deref(), Some("two"));
        assert_eq!(read_line(&mut r, 10).unwrap().as_deref(), Some("three"));
        assert_eq!(read_line(&mut r, 10).unwrap(), None);
        let mut long = io::Cursor::new(b"0123456789abc\n".to_vec());
        assert!(read_line(&mut long, 10).is_err());
    }

    /// A socket round trip with a stand-in for the main thread.
    #[std::prelude::v1::test]
    fn serves_requests_over_the_socket() {
        let dir = tempfile::tempdir().unwrap();
        let (server, mut rx) = serve(dir.path()).unwrap();
        let mode = fs::metadata(&server.path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let mode = fs::metadata(dir.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        std::thread::spawn(move || {
            futures::executor::block_on(async {
                while let Some(req) = rx.next().await {
                    let result = match req.tool.as_str() {
                        "echo" => Ok(req.args.clone()),
                        other => Err(format!("unknown tool {other}")),
                    };
                    req.respond(result);
                }
            })
        });
        assert_eq!(sockets_in(dir.path()), vec![server.path.clone()]);
        let stream = connect(&server.path).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        let mut ask = |line: &str| {
            writeln!(writer, "{line}").unwrap();
            let reply = read_line(&mut reader, MAX_MESSAGE).unwrap().unwrap();
            serde_json::from_str::<Value>(&reply).unwrap()
        };
        let r = ask(r#"{"id":1,"tool":"echo","args":{"a":[1,2]}}"#);
        assert_eq!(r, json!({"id": 1, "result": {"a": [1, 2]}}));
        let r = ask(r#"{"id":2,"tool":"nope"}"#);
        assert_eq!(r["error"], "unknown tool nope");
        let r = ask("not json");
        assert!(r["error"].as_str().unwrap().starts_with("invalid JSON"));
        let path = server.path.clone();
        drop(server);
        assert!(!path.exists());
    }
}
