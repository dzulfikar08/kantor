//! The running app as the MCP server's [`Backend`]: tool calls go over
//! the instance's socket (see [`crate::ipc`]). If no Kantor is running,
//! the first call starts one (unless `--no-launch`) and waits for it.

use std::io::{BufReader, Write as _};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::protocol::Backend;
use crate::ipc;

/// How long a freshly started Kantor gets to open its socket.
const LAUNCH_WAIT: Duration = Duration::from_secs(20);

struct Conn {
    reader: BufReader<ipc::Stream>,
    writer: ipc::Stream,
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
        let exe = super::exe_path().map_err(|e| format!("can't find the kantor binary: {e}"))?;
        log::info!("starting {}", exe.display());
        let mut cmd = std::process::Command::new(&exe);
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            cmd.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP: outlives this
            // `kantor mcp`, and the agent's Ctrl+C doesn't reach it.
            cmd.creation_flags(0x0000_0008 | 0x0000_0200);
        }
        cmd.spawn()
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

/// `slides_build`: write the deck's `.pptx` here, without the app.
pub fn build_deck(args: &Value) -> Result<Value, String> {
    let run = || -> anyhow::Result<Value> {
        use anyhow::{Context as _, bail};
        let deck = args.get("deck").context("`deck` is required")?;
        let path = crate::agent::path_arg(crate::agent::req_str(args, "path")?)?;
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pptx"))
        {
            bail!("`path` must end in .pptx");
        }
        let base = crate::agent::opt_str(args, "base_dir")?
            .map(crate::agent::path_arg)
            .transpose()?;
        let overwrite = crate::agent::opt_bool(args, "overwrite")?.unwrap_or(false);
        if path.exists() && !overwrite {
            bail!(
                "{} exists; pass overwrite=true to replace it",
                path.display()
            );
        }
        let p = crate::agent::slides_author::build(deck, base.as_deref())?;
        let bytes = p.to_bytes()?;
        crate::cli::slides::write_file(&path, &bytes)?;
        Ok(json!({"path": path, "slides": p.len(), "bytes": bytes.len()}))
    };
    run().map_err(|e| format!("{e:#}"))
}

impl Backend for AppClient {
    fn call(&mut self, tool: &str, args: Value) -> Result<Value, String> {
        if tool == "slides_build" {
            let mut out = build_deck(&args)?;
            if args.get("open").and_then(Value::as_bool) == Some(true) {
                let opened = self.call("open_document", json!({"path": out["path"]}))?;
                out["opened"] = opened;
            }
            return Ok(out);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn slides_build_writes_a_deck_and_keeps_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deck.pptx");
        let args = json!({
            "path": path,
            "deck": {"units": "in", "slides": [{"title": "Hello", "elements": [
                {"type": "shape", "geometry": "chevron", "x": 1, "y": 2, "w": 3, "h": 1, "text": "Go"}
            ]}]},
        });
        let out = build_deck(&args).unwrap();
        assert_eq!(out["slides"], 1);
        let first = std::fs::read(&path).unwrap();
        assert!(build_deck(&args).unwrap_err().contains("overwrite"));
        let mut again = args.clone();
        again["overwrite"] = json!(true);
        build_deck(&again).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), first);
        let mut bad = args.clone();
        bad["path"] = json!("relative.pptx");
        assert!(build_deck(&bad).is_err());
    }

    #[std::prelude::v1::test]
    fn every_slides_editing_tool_is_offered() {
        for t in crate::agent::slides_author::EDIT_TOOLS {
            assert!(super::super::tools::exists(t), "{t}");
        }
        assert!(super::super::tools::exists("slides_build"));
    }
}
