//! The client side of one ACP connection.

use std::collections::HashMap;
use std::fmt;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::{Duration, Instant};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use serde_json::{Value, json};

use crate::PROTOCOL_VERSION;
use crate::fs::FsHandler;
use crate::preset::AgentCommand;
use crate::process::{self, AgentProcess};
use crate::types::{
    AcpError, AcpEvent, AgentInfo, McpServer, PermissionChoice, PermissionId, PermissionOption,
    PermissionRequest, PromptBlock, Session, SessionId, SessionUpdate, StopReason, resolve_choice,
};
use crate::wire::{self, INTERNAL_ERROR, INVALID_PARAMS, Incoming, METHOD_NOT_FOUND};

/// The agent's events, in the order it sent them. Ends after
/// [`AcpEvent::Exited`].
pub type Events = UnboundedReceiver<AcpEvent>;

/// How long the agent gets to exit after SIGTERM before SIGKILL.
const GRACE: Duration = Duration::from_millis(300);
/// The stderr kept for [`AcpEvent::Exited`].
const STDERR_TAIL: usize = 8 * 1024;

#[derive(Clone)]
pub struct ClientOptions {
    /// Answers the agent's `fs/*` requests; `None` (the default) offers no
    /// file access at all.
    pub fs: Option<Arc<dyn FsHandler>>,
    /// Sent as `clientInfo`.
    pub client_name: String,
    pub client_version: String,
}

impl Default for ClientOptions {
    fn default() -> Self {
        ClientOptions {
            fs: None,
            client_name: "kantor".to_owned(),
            client_version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

impl fmt::Debug for ClientOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientOptions")
            .field("fs", &self.fs.is_some())
            .field("client_name", &self.client_name)
            .field("client_version", &self.client_version)
            .finish()
    }
}

/// A running ACP agent. Clones share it; the process (and everything it
/// started) ends when the last clone is dropped, or on [`Self::shutdown`].
#[derive(Clone)]
pub struct AcpAgent {
    inner: Arc<Inner>,
}

impl fmt::Debug for AcpAgent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AcpAgent")
            .field("pid", &self.pid())
            .field("running", &self.is_running())
            .finish()
    }
}

struct Inner {
    shared: Arc<Shared>,
    options: ClientOptions,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Off the caller's thread: ending the process waits up to GRACE.
        let shared = self.shared.clone();
        let spawned = std::thread::Builder::new()
            .name("acp-shutdown".to_owned())
            .spawn(move || shared.shutdown());
        if spawned.is_err() {
            self.shared.shutdown();
        }
    }
}

/// Who waits for the answer to one of our requests.
enum Waiter {
    /// A blocking call.
    Reply(mpsc::Sender<Result<Value, AcpError>>),
    /// `session/prompt`: the answer becomes [`AcpEvent::TurnEnded`].
    Turn(SessionId),
}

struct PendingPermission {
    rpc_id: Value,
    session: SessionId,
    options: Vec<PermissionOption>,
}

#[derive(Default)]
struct State {
    waiters: HashMap<u64, Waiter>,
    permissions: HashMap<u64, PendingPermission>,
    closed: bool,
}

#[derive(Default)]
struct StderrTail {
    bytes: Vec<u8>,
    done: bool,
}

struct Shared {
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    state: Mutex<State>,
    next_id: AtomicU64,
    next_permission: AtomicU64,
    events: UnboundedSender<AcpEvent>,
    fs: Option<Arc<dyn FsHandler>>,
    process: Mutex<Option<AgentProcess>>,
    stderr: Arc<Mutex<StderrTail>>,
    has_stderr: bool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl AcpAgent {
    /// Starts `command` in `cwd` (the folder of the open document, say).
    /// Nothing is sent yet; call [`Self::initialize`] next.
    pub fn spawn(
        command: &AgentCommand,
        cwd: &Path,
        options: ClientOptions,
    ) -> Result<(AcpAgent, Events), AcpError> {
        let (process, pipes) = process::spawn(command, cwd).map_err(AcpError::Spawn)?;
        log::info!("acp: started {} (pid {})", command.label, process.id());
        Self::start(
            Box::new(pipes.stdin),
            pipes.stdout,
            Some(pipes.stderr),
            Some(process),
            options,
        )
    }

    /// An agent on any pair of streams (tests, an agent behind a socket).
    pub fn connect<R, W>(
        reader: R,
        writer: W,
        options: ClientOptions,
    ) -> Result<(AcpAgent, Events), AcpError>
    where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
    {
        Self::start(Box::new(writer), reader, None, None, options)
    }

    fn start<R: Read + Send + 'static>(
        writer: Box<dyn Write + Send>,
        reader: R,
        stderr: Option<std::process::ChildStderr>,
        process: Option<AgentProcess>,
        options: ClientOptions,
    ) -> Result<(AcpAgent, Events), AcpError> {
        let (tx, rx) = unbounded();
        let shared = Arc::new(Shared {
            writer: Mutex::new(Some(writer)),
            state: Mutex::new(State::default()),
            next_id: AtomicU64::new(1),
            next_permission: AtomicU64::new(1),
            events: tx,
            fs: options.fs.clone(),
            process: Mutex::new(process),
            stderr: Arc::new(Mutex::new(StderrTail::default())),
            has_stderr: stderr.is_some(),
        });
        if let Some(stderr) = stderr {
            let tail = shared.stderr.clone();
            std::thread::Builder::new()
                .name("acp-stderr".to_owned())
                .spawn(move || drain_stderr(stderr, &tail))
                .map_err(AcpError::Spawn)?;
        }
        let reader_shared = shared.clone();
        let agent = AcpAgent {
            inner: Arc::new(Inner { shared, options }),
        };
        // If this fails, dropping `agent` ends the process.
        std::thread::Builder::new()
            .name("acp-reader".to_owned())
            .spawn(move || reader_shared.read_loop(reader))
            .map_err(AcpError::Spawn)?;
        Ok((agent, rx))
    }

    fn shared(&self) -> &Shared {
        &self.inner.shared
    }

    /// The agent's process id (`None` for [`Self::connect`]).
    pub fn pid(&self) -> Option<u32> {
        lock(&self.shared().process).as_ref().map(AgentProcess::id)
    }

    /// False once the agent has exited or was shut down.
    pub fn is_running(&self) -> bool {
        !lock(&self.shared().state).closed
    }

    /// The handshake. Offers file access only when [`ClientOptions::fs`]
    /// is set, and never a terminal. Blocks until the agent answers (the
    /// first start through `npx` downloads the adapter first).
    pub fn initialize(&self) -> Result<AgentInfo, AcpError> {
        let options = &self.inner.options;
        let (read, write) = options
            .fs
            .as_ref()
            .map_or((false, false), |h| (h.can_read(), h.can_write()));
        let v = self.shared().request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "clientCapabilities": {
                    "fs": {"readTextFile": read, "writeTextFile": write},
                    "terminal": false,
                },
                "clientInfo": {
                    "name": options.client_name,
                    "title": "Kantor",
                    "version": options.client_version,
                },
            }),
        )?;
        let info = AgentInfo::from_value(&v);
        if info.protocol_version != PROTOCOL_VERSION {
            return Err(AcpError::Protocol(format!(
                "the agent speaks ACP version {}, Kantor speaks {PROTOCOL_VERSION}",
                info.protocol_version
            )));
        }
        Ok(info)
    }

    /// Signs in with one of [`AgentInfo::auth_methods`]. Blocks while the
    /// agent does it (it may open a browser).
    pub fn authenticate(&self, method_id: &str) -> Result<(), AcpError> {
        self.shared()
            .request("authenticate", json!({"methodId": method_id}))
            .map(drop)
    }

    /// A new conversation working in `cwd` (absolute), with `mcp_servers`
    /// (normally [`McpServer::kantor_current`]). Fails with
    /// [`AcpError::AuthRequired`] when the user must sign in first.
    pub fn new_session(&self, cwd: &Path, mcp_servers: &[McpServer]) -> Result<Session, AcpError> {
        let v = self.shared().request(
            "session/new",
            json!({
                "cwd": cwd,
                "mcpServers": mcp_servers.iter().map(McpServer::to_json).collect::<Vec<_>>(),
            }),
        )?;
        Session::from_value(&v).ok_or_else(|| AcpError::Protocol("no sessionId".to_owned()))
    }

    /// Switches the session to one of [`Session::modes`].
    pub fn set_mode(&self, session: &str, mode_id: &str) -> Result<(), AcpError> {
        self.shared()
            .request(
                "session/set_mode",
                json!({"sessionId": session, "modeId": mode_id}),
            )
            .map(drop)
    }

    /// Sends the user's message and returns at once; the reply streams in
    /// as [`AcpEvent::Update`]s and ends with [`AcpEvent::TurnEnded`].
    pub fn prompt(&self, session: &str, blocks: Vec<PromptBlock>) -> Result<(), AcpError> {
        self.shared()
            .send_request(
                "session/prompt",
                json!({
                    "sessionId": session,
                    "prompt": blocks.iter().map(PromptBlock::to_json).collect::<Vec<_>>(),
                }),
                Waiter::Turn(session.to_owned()),
            )
            .map(drop)
    }

    /// Stops the session's current turn: the agent ends it with
    /// [`StopReason::Cancelled`]. Permission requests of the session still
    /// waiting for the user are answered "cancelled".
    pub fn cancel(&self, session: &str) -> Result<(), AcpError> {
        let shared = self.shared();
        log::debug!("acp: → session/cancel");
        shared.write(&wire::notification(
            "session/cancel",
            json!({"sessionId": session}),
        ))?;
        let pending: Vec<PendingPermission> = {
            let mut state = lock(&shared.state);
            let ids: Vec<u64> = state
                .permissions
                .iter()
                .filter(|(_, p)| p.session == session)
                .map(|(id, _)| *id)
                .collect();
            ids.iter()
                .filter_map(|id| state.permissions.remove(id))
                .collect()
        };
        for p in pending {
            shared.answer(&p.rpc_id, None)?;
        }
        Ok(())
    }

    /// The user's answer to an [`AcpEvent::Permission`]. A choice the agent
    /// didn't offer is an error and leaves the request pending.
    pub fn answer_permission(
        &self,
        id: PermissionId,
        choice: PermissionChoice,
    ) -> Result<(), AcpError> {
        let shared = self.shared();
        let pending = lock(&shared.state)
            .permissions
            .remove(&id.0)
            .ok_or(AcpError::UnknownPermission(id))?;
        match resolve_choice(&pending.options, &choice) {
            Ok(option) => shared.answer(&pending.rpc_id, option),
            Err(e) => {
                lock(&shared.state).permissions.insert(id.0, pending);
                Err(e)
            }
        }
    }

    /// Ends the agent now: closes its input, then signals its process group
    /// (Unix) or ends its process tree (Windows). Waits up to 300 ms.
    /// [`AcpEvent::Exited`] follows.
    pub fn shutdown(&self) {
        self.shared().shutdown();
    }
}

impl Shared {
    fn write(&self, msg: &Value) -> Result<(), AcpError> {
        let mut line = serde_json::to_vec(msg).map_err(|e| AcpError::Protocol(e.to_string()))?;
        line.push(b'\n');
        let mut writer = lock(&self.writer);
        let Some(w) = writer.as_mut() else {
            return Err(AcpError::Exited);
        };
        w.write_all(&line)
            .and_then(|()| w.flush())
            .map_err(AcpError::Io)
    }

    fn send_request(&self, method: &str, params: Value, waiter: Waiter) -> Result<u64, AcpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut state = lock(&self.state);
            if state.closed {
                return Err(AcpError::Exited);
            }
            state.waiters.insert(id, waiter);
        }
        log::debug!("acp: → {method} #{id}");
        if let Err(e) = self.write(&wire::request(id, method, params)) {
            lock(&self.state).waiters.remove(&id);
            return Err(e);
        }
        Ok(id)
    }

    fn request(&self, method: &str, params: Value) -> Result<Value, AcpError> {
        let (tx, rx) = mpsc::channel();
        self.send_request(method, params, Waiter::Reply(tx))?;
        rx.recv().unwrap_or(Err(AcpError::Exited))
    }

    /// Answers a `session/request_permission`: `Some(option id)` or the
    /// cancelled outcome.
    fn answer(&self, rpc_id: &Value, option: Option<String>) -> Result<(), AcpError> {
        let outcome = match option {
            Some(id) => json!({"outcome": "selected", "optionId": id}),
            None => json!({"outcome": "cancelled"}),
        };
        self.write(&wire::response(rpc_id, json!({"outcome": outcome})))
    }

    fn emit(&self, event: AcpEvent) -> bool {
        self.events.unbounded_send(event).is_ok()
    }

    fn shutdown(&self) {
        // EOF on its stdin: well-behaved agents exit by themselves.
        lock(&self.writer).take();
        if let Some(p) = lock(&self.process).as_mut() {
            p.stop(GRACE);
        }
    }

    fn read_loop<R: Read>(&self, reader: R) {
        let mut reader = BufReader::new(reader);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let line = String::from_utf8_lossy(&buf);
            match wire::parse_line(&line) {
                Some(msg) => self.handle(msg),
                None if !line.trim().is_empty() => {
                    log::debug!("acp: skipped a non-JSON-RPC line ({} bytes)", buf.len());
                }
                None => {}
            }
        }
        self.closed();
    }

    fn handle(&self, msg: Incoming) {
        match msg {
            Incoming::Response { id, result } => {
                let Some(waiter) = lock(&self.state).waiters.remove(&id) else {
                    log::debug!("acp: answer to unknown request #{id}");
                    return;
                };
                let result = result.map_err(AcpError::from_rpc);
                match waiter {
                    Waiter::Reply(tx) => {
                        let _ = tx.send(result);
                    }
                    Waiter::Turn(session) => {
                        let result = result.and_then(|v| {
                            v["stopReason"]
                                .as_str()
                                .map(StopReason::from_wire)
                                .ok_or_else(|| AcpError::Protocol("no stopReason".to_owned()))
                        });
                        self.emit(AcpEvent::TurnEnded { session, result });
                    }
                }
            }
            Incoming::Notification { method, params } => {
                if method == "session/update" {
                    let Some(session) = params["sessionId"].as_str() else {
                        return;
                    };
                    self.emit(AcpEvent::Update {
                        session: session.to_owned(),
                        update: SessionUpdate::from_value(&params["update"]),
                    });
                } else {
                    log::debug!("acp: ignored notification {method}");
                }
            }
            Incoming::Request { id, method, params } => {
                log::debug!("acp: ← {method}");
                let reply = match method.as_str() {
                    "session/request_permission" => {
                        self.permission(id, &params);
                        return;
                    }
                    "fs/read_text_file" => self.read_text_file(&params),
                    "fs/write_text_file" => self.write_text_file(&params),
                    _ => Err((METHOD_NOT_FOUND, format!("{method} is not supported"))),
                };
                let msg = match reply {
                    Ok(result) => wire::response(&id, result),
                    Err((code, message)) => wire::error_response(&id, code, &message),
                };
                let _ = self.write(&msg);
            }
        }
    }

    fn permission(&self, rpc_id: Value, params: &Value) {
        let id = PermissionId(self.next_permission.fetch_add(1, Ordering::Relaxed));
        let Some(request) = PermissionRequest::parse(id, params) else {
            let _ = self.write(&wire::error_response(
                &rpc_id,
                INVALID_PARAMS,
                "malformed permission request",
            ));
            return;
        };
        lock(&self.state).permissions.insert(
            id.0,
            PendingPermission {
                rpc_id: rpc_id.clone(),
                session: request.session.clone(),
                options: request.options.clone(),
            },
        );
        if !self.emit(AcpEvent::Permission(request)) {
            // Nobody listens: don't leave the agent waiting.
            lock(&self.state).permissions.remove(&id.0);
            let _ = self.answer(&rpc_id, None);
        }
    }

    fn fs_handler(&self, write: bool) -> Result<&dyn FsHandler, (i64, String)> {
        match &self.fs {
            Some(h) if (if write { h.can_write() } else { h.can_read() }) => Ok(&**h),
            _ => Err((METHOD_NOT_FOUND, "file access is off".to_owned())),
        }
    }

    fn read_text_file(&self, params: &Value) -> Result<Value, (i64, String)> {
        let fs = self.fs_handler(false)?;
        let path = params["path"]
            .as_str()
            .ok_or((INVALID_PARAMS, "no path".to_owned()))?;
        let content = fs
            .read_text_file(
                Path::new(path),
                params["line"].as_u64(),
                params["limit"].as_u64(),
            )
            .map_err(|e| (INTERNAL_ERROR, e))?;
        Ok(json!({"content": content}))
    }

    fn write_text_file(&self, params: &Value) -> Result<Value, (i64, String)> {
        let fs = self.fs_handler(true)?;
        let (Some(path), Some(content)) = (params["path"].as_str(), params["content"].as_str())
        else {
            return Err((INVALID_PARAMS, "path and content needed".to_owned()));
        };
        fs.write_text_file(Path::new(path), content)
            .map_err(|e| (INTERNAL_ERROR, e))?;
        Ok(Value::Null)
    }

    /// The agent's output ended: fail what waits, end the process, report.
    fn closed(&self) {
        let (waiters, _permissions) = {
            let mut state = lock(&self.state);
            state.closed = true;
            (
                std::mem::take(&mut state.waiters),
                std::mem::take(&mut state.permissions),
            )
        };
        let mut waiters: Vec<(u64, Waiter)> = waiters.into_iter().collect();
        waiters.sort_by_key(|(id, _)| *id);
        for (_, waiter) in waiters {
            match waiter {
                Waiter::Reply(tx) => {
                    let _ = tx.send(Err(AcpError::Exited));
                }
                Waiter::Turn(session) => {
                    self.emit(AcpEvent::TurnEnded {
                        session,
                        result: Err(AcpError::Exited),
                    });
                }
            }
        }
        lock(&self.writer).take();
        let code = lock(&self.process).as_mut().and_then(|p| p.stop(GRACE));
        let stderr_tail = if self.has_stderr {
            // The process is gone, so stderr ends too; give the drain thread
            // a moment to read the last of it.
            let deadline = Instant::now() + Duration::from_millis(500);
            while !lock(&self.stderr).done && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            String::from_utf8_lossy(&lock(&self.stderr).bytes)
                .trim()
                .to_owned()
        } else {
            String::new()
        };
        log::info!("acp: agent exited (code {code:?})");
        self.emit(AcpEvent::Exited { code, stderr_tail });
    }
}

/// Keeps the last [`STDERR_TAIL`] bytes of the agent's stderr. Never logged:
/// it may echo what the user asked.
fn drain_stderr(mut stderr: std::process::ChildStderr, tail: &Mutex<StderrTail>) {
    let mut chunk = [0u8; 4096];
    loop {
        match stderr.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut t = lock(tail);
                t.bytes.extend_from_slice(&chunk[..n]);
                let len = t.bytes.len();
                if len > STDERR_TAIL {
                    t.bytes.drain(..len - STDERR_TAIL);
                }
            }
        }
    }
    lock(tail).done = true;
}
