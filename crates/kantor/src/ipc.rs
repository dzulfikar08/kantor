//! The local endpoint `kantor mcp` talks to: one per running Kantor, in a
//! private directory.
//!
//! ```text
//! Linux    $XDG_RUNTIME_DIR/kantor/kantor-<pid>.sock   (else ~/.local/state/kantor/run/)
//! macOS    ~/Library/Application Support/Kantor/run/kantor-<pid>.sock
//! Windows  %LOCALAPPDATA%\Kantor\run\kantor-<pid>.port
//! ```
//!
//! (`KANTOR_SOCKET_DIR` overrides the directory, for tests.) On Linux and
//! macOS the endpoint is a Unix domain socket; the directory is `0700` and the
//! socket `0600`, so only the user can connect. On Windows it is a TCP socket
//! on 127.0.0.1 with an ephemeral port: the `.port` file holds the address and
//! a random token, and a client must send that token as its first line before
//! anything else (other local users can reach the port, but can't read the
//! file in the user's profile). After that, messages are one JSON object per
//! line on every platform:
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
const SUFFIX: &str = endpoint::SUFFIX;

pub use endpoint::Stream;

/// Where running instances put their endpoints.
pub fn socket_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("KANTOR_SOCKET_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    if cfg!(windows) {
        return kantor_core::session::state_dir().map(|d| d.join("run"));
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
pub fn connect(path: &Path) -> io::Result<Stream> {
    match endpoint::connect(path) {
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

/// Start listening in `dir`. Requests arrive on the returned channel.
pub fn serve(dir: &Path) -> io::Result<(Server, UnboundedReceiver<Request>)> {
    endpoint::make_private_dir(dir)?;
    let path = dir.join(format!("{PREFIX}{}{SUFFIX}", std::process::id()));
    let _ = fs::remove_file(&path);
    let listener = endpoint::Listener::bind(&path)?;
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

fn accept_loop(listener: endpoint::Listener, tx: UnboundedSender<Request>) {
    let clients = Arc::new(AtomicUsize::new(0));
    loop {
        let Ok(mut stream) = listener.accept() else {
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
        let auth = listener.token().to_owned();
        let spawned = std::thread::Builder::new()
            .name("kantor-ipc-client".into())
            .spawn(move || {
                if let Err(e) = serve_client(stream, &client_tx, &auth) {
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
fn serve_client(stream: Stream, tx: &UnboundedSender<Request>, token: &str) -> io::Result<()> {
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    if !token.is_empty() {
        // Windows: the token from the endpoint file comes first, within a few
        // seconds, or the connection is dropped without a word.
        reader
            .get_ref()
            .set_read_timeout(Some(Duration::from_secs(5)))?;
        let line = read_line(&mut reader, 256)?.unwrap_or_default();
        if !endpoint::same_token(line.trim(), token) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "wrong token",
            ));
        }
        reader.get_ref().set_read_timeout(None)?;
    }
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

/// Unix domain sockets.
#[cfg(unix)]
mod endpoint {
    use std::fs;
    use std::io;
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;

    pub const SUFFIX: &str = ".sock";
    pub type Stream = UnixStream;

    /// Create the private directory for sockets.
    pub fn make_private_dir(dir: &Path) -> io::Result<()> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        // An existing directory may have been made with looser permissions.
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
    }

    pub struct Listener(UnixListener);

    impl Listener {
        pub fn bind(path: &Path) -> io::Result<Self> {
            let listener = UnixListener::bind(path)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
            Ok(Self(listener))
        }

        pub fn accept(&self) -> io::Result<Stream> {
            self.0.accept().map(|(s, _)| s)
        }

        /// No token: the socket's permissions keep other users out.
        pub fn token(&self) -> &str {
            ""
        }
    }

    pub fn connect(path: &Path) -> io::Result<Stream> {
        UnixStream::connect(path)
    }

    pub fn same_token(_: &str, _: &str) -> bool {
        true
    }
}

/// TCP on 127.0.0.1 plus a token in a file only the user can read (std has
/// no named pipes or Unix sockets on Windows, and Kantor forbids `unsafe`).
#[cfg(windows)]
mod endpoint {
    use std::fs;
    use std::io::{self, Write as _};
    use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
    use std::path::Path;
    use std::time::Duration;

    pub const SUFFIX: &str = ".port";
    pub type Stream = TcpStream;

    /// `%LOCALAPPDATA%` is private to the user already (profile ACLs).
    pub fn make_private_dir(dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)
    }

    pub struct Listener {
        listener: TcpListener,
        token: String,
    }

    fn random_token() -> io::Result<String> {
        use ring::rand::SecureRandom as _;
        let mut bytes = [0u8; 32];
        ring::rand::SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| io::Error::other("no random numbers"))?;
        Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }

    impl Listener {
        /// Listen on an ephemeral loopback port and write `ADDR TOKEN` to
        /// `path` (through a temporary file, so a client never reads half).
        pub fn bind(path: &Path) -> io::Result<Self> {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
            let token = random_token()?;
            let tmp = path.with_extension("tmp");
            {
                let mut f = fs::File::create(&tmp)?;
                writeln!(f, "{} {token}", listener.local_addr()?)?;
                f.sync_all()?;
            }
            fs::rename(&tmp, path)?;
            Ok(Self { listener, token })
        }

        pub fn accept(&self) -> io::Result<Stream> {
            let (stream, peer) = self.listener.accept()?;
            if !peer.ip().is_loopback() {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "not local"));
            }
            Ok(stream)
        }

        pub fn token(&self) -> &str {
            &self.token
        }
    }

    pub fn connect(path: &Path) -> io::Result<Stream> {
        let text = fs::read_to_string(path)?;
        let mut parts = text.split_whitespace();
        let addr: SocketAddr = parts
            .next()
            .and_then(|a| a.parse().ok())
            .filter(|a: &SocketAddr| a.ip().is_loopback())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad endpoint file"))?;
        let token = parts
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad endpoint file"))?;
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
        stream.set_nodelay(true)?;
        writeln!(stream, "{token}")?;
        Ok(stream)
    }

    /// Compare without stopping at the first difference.
    pub fn same_token(got: &str, want: &str) -> bool {
        got.len() == want.len()
            && got
                .bytes()
                .zip(want.bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0
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
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(&server.path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            let mode = fs::metadata(dir.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
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

    /// Windows: a client without the endpoint file's token gets nothing.
    #[cfg(windows)]
    #[std::prelude::v1::test]
    fn refuses_clients_without_the_token() {
        let dir = tempfile::tempdir().unwrap();
        let (server, _rx) = serve(dir.path()).unwrap();
        let text = fs::read_to_string(&server.path).unwrap();
        let addr = text.split_whitespace().next().unwrap();
        let mut stream = std::net::TcpStream::connect(addr).unwrap();
        writeln!(stream, "not-the-token").unwrap();
        writeln!(stream, r#"{{"id":1,"tool":"echo"}}"#).unwrap();
        let mut reader = BufReader::new(stream);
        // Closed without an answer.
        assert!(matches!(
            read_line(&mut reader, MAX_MESSAGE),
            Ok(None) | Err(_)
        ));
        assert!(endpoint::same_token("abc", "abc"));
        assert!(!endpoint::same_token("abd", "abc"));
        assert!(!endpoint::same_token("ab", "abc"));
    }
}
