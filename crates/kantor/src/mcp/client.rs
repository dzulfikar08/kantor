//! The running app as the MCP server's [`Backend`]: tool calls go over
//! the instance's socket (see [`crate::ipc`]). If no Kantor is running,
//! the first call starts one (unless `--no-launch`) and waits for it.

use std::io::{BufReader, Write as _};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::protocol::Backend;
use crate::ipc;

/// How long a freshly started Kantor gets to open its socket.
const LAUNCH_WAIT: Duration = Duration::from_secs(20);

struct Conn {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

pub struct AppClient {
    conn: Option<Conn>,
    next_id: u64,
    launch: bool,
}

impl AppClient {
    pub fn new(launch: bool) -> Self {
        Self {
            conn: None,
            next_id: 1,
            launch,
        }
    }

    fn dir() -> Result<PathBuf, String> {
        ipc::socket_dir()
            .ok_or_else(|| "can't tell where Kantor's socket is: HOME is not set".into())
    }

    /// Connect to the newest running instance.
    fn find() -> Option<Conn> {
        let dir = Self::dir().ok()?;
        ipc::sockets_in(&dir).into_iter().find_map(|path| {
            let stream = ipc::connect(&path).ok()?;
            // Above the app's own reply timeout, so its message arrives.
            stream
                .set_read_timeout(Some(ipc::REPLY_TIMEOUT + Duration::from_secs(10)))
                .ok()?;
            Some(Conn {
                reader: BufReader::new(stream.try_clone().ok()?),
                writer: stream,
            })
        })
    }

    fn connect(&mut self) -> Result<&mut Conn, String> {
        if self.conn.is_none() {
            self.conn = match Self::find() {
                Some(conn) => Some(conn),
                None if self.launch => Some(Self::launch()?),
                None => {
                    return Err(
                        "Kantor isn't running. Ask the user to start Kantor, then try again."
                            .into(),
                    );
                }
            };
        }
        Ok(self.conn.as_mut().expect("just connected"))
    }

    /// Start Kantor (detached, its Home screen and last session) and wait
    /// for its socket.
    fn launch() -> Result<Conn, String> {
        use std::os::unix::process::CommandExt as _;
        let exe =
            std::env::current_exe().map_err(|e| format!("can't find the kantor binary: {e}"))?;
        log::info!("starting {}", exe.display());
        std::process::Command::new(&exe)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|e| format!("can't start Kantor ({}): {e}", exe.display()))?;
        let start = Instant::now();
        while start.elapsed() < LAUNCH_WAIT {
            std::thread::sleep(Duration::from_millis(200));
            if let Some(conn) = Self::find() {
                return Ok(conn);
            }
        }
        Err("started Kantor, but it didn't become ready within 20 seconds (no display?)".into())
    }

    fn round_trip(&mut self, tool: &str, args: &Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let conn = self.connect()?;
        let line = json!({"id": id, "tool": tool, "args": args}).to_string();
        let sent = writeln!(conn.writer, "{line}").and_then(|()| conn.writer.flush());
        if let Err(e) = sent {
            self.conn = None;
            return Err(format!("lost the connection to Kantor ({e}); try again"));
        }
        let reply = match ipc::read_line(&mut conn.reader, ipc::MAX_MESSAGE) {
            Ok(Some(reply)) => reply,
            Ok(None) => {
                self.conn = None;
                return Err("Kantor closed the connection (did it quit?)".into());
            }
            Err(e) => {
                self.conn = None;
                return Err(format!("no answer from Kantor: {e}"));
            }
        };
        let reply: Value =
            serde_json::from_str(&reply).map_err(|e| format!("bad answer from Kantor: {e}"))?;
        if let Some(error) = reply.get("error").and_then(Value::as_str) {
            return Err(error.to_owned());
        }
        Ok(reply.get("result").cloned().unwrap_or(Value::Null))
    }
}

impl Backend for AppClient {
    fn call(&mut self, tool: &str, args: Value) -> Result<Value, String> {
        // A connection to an instance that has quit since the last call
        // fails on first use; one fresh connection is tried (that instance
        // is gone, so nothing runs twice). A timeout is not retried.
        let had_conn = self.conn.is_some();
        match self.round_trip(tool, &args) {
            Err(e) if had_conn && self.conn.is_none() && !e.starts_with("no answer") => {
                self.round_trip(tool, &args)
            }
            other => other,
        }
    }
}
