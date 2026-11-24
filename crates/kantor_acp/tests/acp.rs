//! End-to-end tests against a fake ACP agent, in two forms:
//! - in process, over pipes, where each test plays the agent step by step;
//! - as a real subprocess: this test binary started again with
//!   `KANTOR_ACP_FAKE_AGENT=<mode>`, for spawning, exit codes, stderr and
//!   process-group shutdown.
//!
//! No network, no real agent. `harness = false`, so `main` is ours.

use std::io::{BufRead, BufReader, PipeReader, PipeWriter, Write};
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use futures::StreamExt;
use kantor_acp::{
    AcpAgent, AcpError, AcpEvent, AgentCommand, ClientOptions, Content, DiskFs, Events, McpServer,
    PermissionChoice, PromptBlock, SessionUpdate, StopReason, ToolKind, ToolStatus,
};
use serde_json::{Value, json};

const FAKE_ENV: &str = "KANTOR_ACP_FAKE_AGENT";

fn main() {
    if let Ok(mode) = std::env::var(FAKE_ENV) {
        std::process::exit(fake_agent(&mode));
    }
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut tests: Vec<(&str, fn())> = vec![
        ("in_process_full_turn", in_process_full_turn as fn()),
        (
            "cancel_answers_pending_permissions",
            cancel_answers_pending_permissions as fn(),
        ),
        ("crash_mid_turn", crash_mid_turn as fn()),
        (
            "refuses_fs_and_terminal_by_default",
            refuses_fs_and_terminal_by_default as fn(),
        ),
        ("fs_handler_when_enabled", fs_handler_when_enabled as fn()),
        ("auth_required", auth_required as fn()),
        (
            "permission_without_listener_is_cancelled",
            permission_without_listener_is_cancelled as fn(),
        ),
        (
            "wrong_choice_keeps_request_pending",
            wrong_choice_keeps_request_pending as fn(),
        ),
        ("subprocess_full_turn", subprocess_full_turn as fn()),
        (
            "subprocess_crash_reports_exit_code",
            subprocess_crash_reports_exit_code as fn(),
        ),
    ];
    #[cfg(unix)]
    tests.push((
        "drop_ends_the_process_group",
        drop_ends_the_process_group as fn(),
    ));
    let filter = std::env::args().skip(1).find(|a| !a.starts_with('-'));
    let mut failed = Vec::new();
    let mut ran = 0;
    for (name, test) in tests {
        if filter.as_deref().is_some_and(|f| !name.contains(f)) {
            continue;
        }
        ran += 1;
        println!("test {name} ...");
        match std::panic::catch_unwind(test) {
            Ok(()) => println!("test {name} ... ok"),
            Err(_) => {
                println!("test {name} ... FAILED");
                failed.push(name);
            }
        }
    }
    println!("{} passed, {} failed", ran - failed.len(), failed.len());
    if !failed.is_empty() {
        eprintln!("failed: {failed:?}");
        std::process::exit(1);
    }
}

// ---- helpers -------------------------------------------------------------

const WAIT: Duration = Duration::from_secs(30);

/// The events on a std channel, for timeouts.
struct Ev(mpsc::Receiver<AcpEvent>);

fn forward(mut events: Events) -> Ev {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        while let Some(e) = futures::executor::block_on(events.next()) {
            if tx.send(e).is_err() {
                break;
            }
        }
    });
    Ev(rx)
}

impl Ev {
    fn next(&self) -> AcpEvent {
        self.0.recv_timeout(WAIT).expect("an event")
    }

    fn update(&self) -> (String, SessionUpdate) {
        match self.next() {
            AcpEvent::Update { session, update } => (session, update),
            other => panic!("expected an update, got {other:?}"),
        }
    }
}

/// The agent's end of an in-process connection.
struct Fake {
    rx: BufReader<PipeReader>,
    tx: Option<PipeWriter>,
}

impl Fake {
    /// The client's next message; `None` when it closed its output.
    fn try_recv(&mut self) -> Option<Value> {
        let mut line = String::new();
        self.rx.read_line(&mut line).expect("read");
        if line.is_empty() {
            return None;
        }
        Some(serde_json::from_str(&line).expect("client sends JSON"))
    }

    fn recv(&mut self) -> Value {
        self.try_recv().expect("a message from the client")
    }

    fn send(&mut self, v: Value) {
        let w = self.tx.as_mut().expect("not crashed");
        writeln!(w, "{v}").unwrap();
        w.flush().unwrap();
    }

    fn send_raw(&mut self, line: &str) {
        let w = self.tx.as_mut().expect("not crashed");
        writeln!(w, "{line}").unwrap();
        w.flush().unwrap();
    }

    fn reply(&mut self, req: &Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": req["id"], "result": result}));
    }

    fn crash(&mut self) {
        self.tx = None;
    }
}

fn pair_with(options: ClientOptions) -> (AcpAgent, Events, Fake) {
    let (client_r, agent_w) = std::io::pipe().unwrap();
    let (agent_r, client_w) = std::io::pipe().unwrap();
    let (agent, events) = AcpAgent::connect(client_r, client_w, options).unwrap();
    let fake = Fake {
        rx: BufReader::new(agent_r),
        tx: Some(agent_w),
    };
    (agent, events, fake)
}

fn pair() -> (AcpAgent, Ev, Fake) {
    let (agent, events, fake) = pair_with(ClientOptions::default());
    (agent, forward(events), fake)
}

fn init_result() -> Value {
    json!({
        "protocolVersion": 1,
        "agentCapabilities": {"loadSession": false, "promptCapabilities": {"embeddedContext": true}},
        "authMethods": [{"id": "login", "name": "Log in", "description": null}],
        "agentInfo": {"name": "fake-agent", "version": "1.0"}
    })
}

fn update(session: &str, update: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": session, "update": update}})
}

fn permission_request(id: &str, session: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/request_permission",
        "params": {
            "sessionId": session,
            "toolCall": {"toolCallId": "t1", "title": "Edit report.docx", "kind": "edit"},
            "options": [
                {"optionId": "once", "name": "Allow", "kind": "allow_once"},
                {"optionId": "always", "name": "Always allow", "kind": "allow_always"},
                {"optionId": "no", "name": "Reject", "kind": "reject_once"}
            ]
        }
    })
}

/// Runs `f` on another thread while the test answers as the agent.
fn call<T: Send + 'static>(
    agent: &AcpAgent,
    f: impl FnOnce(AcpAgent) -> T + Send + 'static,
) -> thread::JoinHandle<T> {
    let agent = agent.clone();
    thread::spawn(move || f(agent))
}

/// initialize + session/new, answered by `fake`; returns the session id.
fn ready(agent: &AcpAgent, fake: &mut Fake) -> String {
    let h = call(agent, |a| a.initialize());
    let req = fake.recv();
    fake.reply(&req, init_result());
    h.join().unwrap().unwrap();
    let h = call(agent, |a| a.new_session(&std::env::temp_dir(), &[]));
    let req = fake.recv();
    assert_eq!(req["method"], "session/new");
    fake.reply(&req, json!({"sessionId": "s1"}));
    h.join().unwrap().unwrap().id
}

// ---- in process ----------------------------------------------------------

fn in_process_full_turn() {
    let (agent, ev, mut fake) = pair();
    assert!(agent.is_running());
    assert_eq!(agent.pid(), None);

    let h = call(&agent, |a| a.initialize());
    let req = fake.recv();
    assert_eq!(req["method"], "initialize");
    assert_eq!(req["params"]["protocolVersion"], 1);
    assert_eq!(
        req["params"]["clientCapabilities"],
        json!({"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false})
    );
    assert_eq!(req["params"]["clientInfo"]["name"], "kantor");
    fake.reply(&req, init_result());
    let info = h.join().unwrap().unwrap();
    assert_eq!(info.auth_methods[0].id, "login");
    assert_eq!(info.name.as_deref(), Some("fake-agent"));
    assert!(info.prompt_embedded_context);

    let dir = std::env::temp_dir();
    let cwd = dir.clone();
    let h = call(&agent, move |a| {
        a.new_session(&cwd, &[McpServer::kantor("/opt/kantor/kantor")])
    });
    let req = fake.recv();
    assert_eq!(req["method"], "session/new");
    assert_eq!(req["params"]["cwd"], json!(dir));
    let mcp = &req["params"]["mcpServers"][0];
    assert_eq!(mcp["name"], "kantor");
    assert_eq!(mcp["command"], "/opt/kantor/kantor");
    assert_eq!(mcp["args"], json!(["mcp", "--no-launch"]));
    assert!(mcp["env"].is_array());
    fake.reply(
        &req,
        json!({"sessionId": "s1", "modes": {"currentModeId": "ask", "availableModes": [
            {"id": "ask", "name": "Ask"}, {"id": "auto", "name": "Auto-edit"}]}}),
    );
    let session = h.join().unwrap().unwrap();
    assert_eq!(session.id, "s1");
    assert_eq!(session.modes.as_ref().unwrap().available.len(), 2);

    agent
        .prompt(
            "s1",
            vec![
                PromptBlock::text("Bold the title"),
                PromptBlock::file(&dir.join("report.docx")),
            ],
        )
        .unwrap();
    let prompt = fake.recv();
    assert_eq!(prompt["method"], "session/prompt");
    assert_eq!(prompt["params"]["sessionId"], "s1");
    assert_eq!(
        prompt["params"]["prompt"][0],
        json!({"type": "text", "text": "Bold the title"})
    );
    assert_eq!(prompt["params"]["prompt"][1]["type"], "resource_link");
    assert_eq!(prompt["params"]["prompt"][1]["name"], "report.docx");
    assert!(
        prompt["params"]["prompt"][1]["uri"]
            .as_str()
            .unwrap()
            .starts_with("file:///")
    );

    fake.send(update(
        "s1",
        json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "Looking"}}),
    ));
    fake.send(update(
        "s1",
        json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "Sure"}}),
    ));
    fake.send(update(
        "s1",
        json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Edit report.docx",
               "kind": "edit", "status": "pending"}),
    ));
    fake.send(permission_request("perm-1", "s1"));

    assert_eq!(
        ev.update(),
        (
            "s1".to_owned(),
            SessionUpdate::AgentThoughtChunk(Content::Text("Looking".into()))
        )
    );
    assert_eq!(
        ev.update().1,
        SessionUpdate::AgentMessageChunk(Content::Text("Sure".into()))
    );
    let SessionUpdate::ToolCall(mut tool) = ev.update().1 else {
        panic!("tool call")
    };
    assert_eq!(tool.kind, ToolKind::Edit);
    let AcpEvent::Permission(p) = ev.next() else {
        panic!("permission")
    };
    assert_eq!(p.session, "s1");
    assert_eq!(p.tool_call.id, "t1");
    assert_eq!(p.options.len(), 3);

    agent
        .answer_permission(p.id, PermissionChoice::AllowAlways)
        .unwrap();
    let answer = fake.recv();
    assert_eq!(answer["id"], "perm-1");
    assert_eq!(
        answer["result"],
        json!({"outcome": {"outcome": "selected", "optionId": "always"}})
    );
    assert!(matches!(
        agent.answer_permission(p.id, PermissionChoice::AllowOnce),
        Err(AcpError::UnknownPermission(_))
    ));

    fake.send(update(
        "s1",
        json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed"}),
    ));
    fake.send(update(
        "s1",
        json!({"sessionUpdate": "plan", "entries": [{"content": "Bold title", "priority": "high", "status": "completed"}]}),
    ));
    fake.send(update(
        "s1",
        json!({"sessionUpdate": "current_mode_update", "currentModeId": "auto"}),
    ));
    fake.reply(&prompt, json!({"stopReason": "end_turn"}));

    let SessionUpdate::ToolCallUpdate(u) = ev.update().1 else {
        panic!("tool call update")
    };
    tool.apply(&u);
    assert_eq!(tool.status, ToolStatus::Completed);
    assert_eq!(tool.title, "Edit report.docx");
    assert!(matches!(ev.update().1, SessionUpdate::Plan(entries) if entries.len() == 1));
    assert_eq!(ev.update().1, SessionUpdate::CurrentMode("auto".into()));
    match ev.next() {
        AcpEvent::TurnEnded { session, result } => {
            assert_eq!(session, "s1");
            assert_eq!(result.unwrap(), StopReason::EndTurn);
        }
        other => panic!("expected the turn's end, got {other:?}"),
    }

    // Dropping the last handle closes the agent's input; the agent then
    // exits (closes its output) and the client reports it.
    drop(agent);
    assert!(fake.try_recv().is_none());
    fake.crash();
    assert!(matches!(ev.next(), AcpEvent::Exited { code: None, .. }));
}

fn cancel_answers_pending_permissions() {
    let (agent, ev, mut fake) = pair();
    let s = ready(&agent, &mut fake);
    agent
        .prompt(&s, vec![PromptBlock::text("Rewrite it")])
        .unwrap();
    let prompt = fake.recv();
    fake.send(permission_request("p9", &s));
    let AcpEvent::Permission(p) = ev.next() else {
        panic!("permission")
    };

    agent.cancel(&s).unwrap();
    let cancel = fake.recv();
    assert_eq!(cancel["method"], "session/cancel");
    assert!(cancel.get("id").is_none());
    assert_eq!(cancel["params"]["sessionId"], "s1");
    let answer = fake.recv();
    assert_eq!(answer["id"], "p9");
    assert_eq!(answer["result"]["outcome"], json!({"outcome": "cancelled"}));
    fake.reply(&prompt, json!({"stopReason": "cancelled"}));

    match ev.next() {
        AcpEvent::TurnEnded { result, .. } => assert_eq!(result.unwrap(), StopReason::Cancelled),
        other => panic!("expected the turn's end, got {other:?}"),
    }
    assert!(matches!(
        agent.answer_permission(p.id, PermissionChoice::AllowOnce),
        Err(AcpError::UnknownPermission(_))
    ));
}

fn crash_mid_turn() {
    let (agent, ev, mut fake) = pair();
    let s = ready(&agent, &mut fake);
    agent.prompt(&s, vec![PromptBlock::text("Go")]).unwrap();
    let _prompt = fake.recv();
    let pending = call(&agent, |a| a.set_mode("s1", "auto"));
    let set_mode = fake.recv();
    assert_eq!(set_mode["method"], "session/set_mode");
    assert_eq!(set_mode["params"]["modeId"], "auto");
    fake.send(update(
        "s1",
        json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "Work"}}),
    ));
    fake.crash();

    assert!(matches!(ev.update().1, SessionUpdate::AgentMessageChunk(_)));
    match ev.next() {
        AcpEvent::TurnEnded { session, result } => {
            assert_eq!(session, "s1");
            assert!(matches!(result, Err(AcpError::Exited)));
        }
        other => panic!("expected the turn's end, got {other:?}"),
    }
    assert!(matches!(ev.next(), AcpEvent::Exited { code: None, .. }));
    assert!(matches!(pending.join().unwrap(), Err(AcpError::Exited)));
    assert!(!agent.is_running());
    assert!(matches!(
        agent.prompt("s1", vec![PromptBlock::text("again")]),
        Err(AcpError::Exited)
    ));
    assert!(matches!(agent.initialize(), Err(AcpError::Exited)));
}

fn refuses_fs_and_terminal_by_default() {
    let (agent, ev, mut fake) = pair();
    let _s = ready(&agent, &mut fake);
    fake.send_raw("npm warn exec The following package was not found and will be installed");
    fake.send(
        json!({"jsonrpc": "2.0", "id": 7, "method": "fs/read_text_file",
                     "params": {"sessionId": "s1", "path": "/etc/passwd"}}),
    );
    let r = fake.recv();
    assert_eq!(r["id"], 7);
    assert_eq!(r["error"]["code"], -32601);
    fake.send(
        json!({"jsonrpc": "2.0", "id": 8, "method": "fs/write_text_file",
                     "params": {"sessionId": "s1", "path": "/tmp/x", "content": "x"}}),
    );
    assert_eq!(fake.recv()["error"]["code"], -32601);
    fake.send(
        json!({"jsonrpc": "2.0", "id": "t", "method": "terminal/create",
                     "params": {"sessionId": "s1", "command": "rm"}}),
    );
    let r = fake.recv();
    assert_eq!(r["id"], "t");
    assert_eq!(r["error"]["code"], -32601);
    fake.send(json!({"jsonrpc": "2.0", "method": "_vendor/something", "params": {}}));
    fake.send(update(
        "s1",
        json!({"sessionUpdate": "usage_update", "used": 10, "size": 100}),
    ));
    assert!(matches!(ev.update().1, SessionUpdate::Other { kind, .. } if kind == "usage_update"));
}

fn fs_handler_when_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
    let options = ClientOptions {
        fs: Some(Arc::new(DiskFs::new(dir.path(), false))),
        ..ClientOptions::default()
    };
    let (agent, _events, mut fake) = pair_with(options);
    let h = call(&agent, |a| a.initialize());
    let req = fake.recv();
    assert_eq!(
        req["params"]["clientCapabilities"]["fs"],
        json!({"readTextFile": true, "writeTextFile": false})
    );
    fake.reply(&req, init_result());
    h.join().unwrap().unwrap();

    fake.send(
        json!({"jsonrpc": "2.0", "id": 1, "method": "fs/read_text_file",
                     "params": {"sessionId": "s1", "path": file, "line": 2, "limit": 1}}),
    );
    let r = fake.recv();
    assert_eq!(r["result"]["content"], "two\n");
    fake.send(
        json!({"jsonrpc": "2.0", "id": 2, "method": "fs/write_text_file",
                     "params": {"sessionId": "s1", "path": file, "content": "x"}}),
    );
    assert_eq!(fake.recv()["error"]["code"], -32601);
    let outside: PathBuf = std::env::temp_dir().join("kantor-acp-outside.txt");
    fake.send(
        json!({"jsonrpc": "2.0", "id": 3, "method": "fs/read_text_file",
                     "params": {"sessionId": "s1", "path": outside}}),
    );
    assert_eq!(fake.recv()["error"]["code"], -32603);
}

fn auth_required() {
    let (agent, _ev, mut fake) = pair();
    let h = call(&agent, |a| a.initialize());
    let req = fake.recv();
    fake.reply(&req, init_result());
    h.join().unwrap().unwrap();

    let h = call(&agent, |a| a.new_session(&std::env::temp_dir(), &[]));
    let req = fake.recv();
    fake.send(json!({"jsonrpc": "2.0", "id": req["id"],
                     "error": {"code": -32000, "message": "Authentication required"}}));
    assert!(matches!(h.join().unwrap(), Err(AcpError::AuthRequired(_))));

    let h = call(&agent, |a| a.authenticate("login"));
    let req = fake.recv();
    assert_eq!(req["method"], "authenticate");
    assert_eq!(req["params"]["methodId"], "login");
    fake.reply(&req, json!({}));
    h.join().unwrap().unwrap();

    // Another error code stays a plain RPC error.
    let h = call(&agent, |a| a.set_mode("s1", "x"));
    let req = fake.recv();
    fake.send(json!({"jsonrpc": "2.0", "id": req["id"],
                     "error": {"code": -32602, "message": "unknown mode"}}));
    assert!(matches!(h.join().unwrap(), Err(AcpError::Rpc(e)) if e.code == -32602));

    // A wrong protocol version is refused.
    let h = call(&agent, |a| a.initialize());
    let req = fake.recv();
    let mut v2 = init_result();
    v2["protocolVersion"] = json!(2);
    fake.reply(&req, v2);
    assert!(matches!(h.join().unwrap(), Err(AcpError::Protocol(_))));
}

fn permission_without_listener_is_cancelled() {
    let (agent, events, mut fake) = pair_with(ClientOptions::default());
    drop(events);
    fake.send(permission_request("p1", "s1"));
    let r = fake.recv();
    assert_eq!(r["id"], "p1");
    assert_eq!(r["result"]["outcome"]["outcome"], "cancelled");
    drop(agent);
}

fn wrong_choice_keeps_request_pending() {
    let (agent, ev, mut fake) = pair();
    let mut perm = permission_request("p1", "s1");
    // Only "allow once" and "reject always" on offer.
    perm["params"]["options"] = json!([
        {"optionId": "y", "name": "Yes", "kind": "allow_once"},
        {"optionId": "n", "name": "No", "kind": "reject_always"}
    ]);
    fake.send(perm);
    let AcpEvent::Permission(p) = ev.next() else {
        panic!("permission")
    };
    assert!(matches!(
        agent.answer_permission(p.id, PermissionChoice::AllowAlways),
        Err(AcpError::ChoiceNotOffered(_))
    ));
    agent
        .answer_permission(p.id, PermissionChoice::Reject)
        .unwrap();
    let r = fake.recv();
    assert_eq!(
        r["result"]["outcome"],
        json!({"outcome": "selected", "optionId": "n"})
    );
}

// ---- subprocess ----------------------------------------------------------

fn fake_command(mode: &str) -> AgentCommand {
    AgentCommand {
        label: "Fake agent".to_owned(),
        program: std::env::current_exe().unwrap(),
        args: Vec::new(),
        env: vec![(FAKE_ENV.to_owned(), mode.to_owned())],
    }
}

fn subprocess_full_turn() {
    let dir = std::env::temp_dir();
    let (agent, events) =
        AcpAgent::spawn(&fake_command("normal"), &dir, ClientOptions::default()).unwrap();
    let ev = forward(events);
    assert!(agent.pid().is_some());
    let info = agent.initialize().unwrap();
    assert_eq!(info.name.as_deref(), Some("fake-agent"));
    let session = agent
        .new_session(&dir, &[McpServer::kantor("/opt/kantor/kantor")])
        .unwrap();
    assert_eq!(session.id, "s1");
    agent
        .prompt(&session.id, vec![PromptBlock::text("Hello")])
        .unwrap();

    assert_eq!(
        ev.update().1,
        SessionUpdate::AgentMessageChunk(Content::Text("Hi from the fake agent".into()))
    );
    let AcpEvent::Permission(p) = ev.next() else {
        panic!("permission")
    };
    agent
        .answer_permission(p.id, PermissionChoice::AllowOnce)
        .unwrap();
    let SessionUpdate::ToolCallUpdate(u) = ev.update().1 else {
        panic!("tool call update")
    };
    assert_eq!(u.status, Some(ToolStatus::Completed));
    // The fake echoes the option it got as the tool's output.
    assert_eq!(u.raw_output, Some(json!({"selected": "once"})));
    match ev.next() {
        AcpEvent::TurnEnded { result, .. } => assert_eq!(result.unwrap(), StopReason::EndTurn),
        other => panic!("expected the turn's end, got {other:?}"),
    }

    agent.shutdown();
    assert!(matches!(ev.next(), AcpEvent::Exited { .. }));
    assert!(!agent.is_running());
}

fn subprocess_crash_reports_exit_code() {
    let dir = std::env::temp_dir();
    let (agent, events) =
        AcpAgent::spawn(&fake_command("crash"), &dir, ClientOptions::default()).unwrap();
    let ev = forward(events);
    agent.initialize().unwrap();
    let s = agent.new_session(&dir, &[]).unwrap();
    agent.prompt(&s.id, vec![PromptBlock::text("Go")]).unwrap();
    assert!(matches!(ev.update().1, SessionUpdate::AgentMessageChunk(_)));
    match ev.next() {
        AcpEvent::TurnEnded { result, .. } => assert!(matches!(result, Err(AcpError::Exited))),
        other => panic!("expected the turn's end, got {other:?}"),
    }
    match ev.next() {
        AcpEvent::Exited { code, stderr_tail } => {
            assert_eq!(code, Some(3));
            assert!(stderr_tail.contains("fake agent: boom"), "{stderr_tail:?}");
        }
        other => panic!("expected the exit, got {other:?}"),
    }
}

/// `ps` state of `pid`: `None` when gone; zombies count as gone.
#[cfg(unix)]
fn alive(pid: u32) -> bool {
    let out = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout);
    let stat = stat.trim();
    !stat.is_empty() && !stat.starts_with('Z')
}

#[cfg(unix)]
fn drop_ends_the_process_group() {
    let dir = std::env::temp_dir();
    let (agent, events) =
        AcpAgent::spawn(&fake_command("grandchild"), &dir, ClientOptions::default()).unwrap();
    let ev = forward(events);
    let pid = agent.pid().unwrap();
    let info = agent.initialize().unwrap();
    let grandchild: u32 = info.version.unwrap().parse().unwrap();
    assert!(alive(pid));
    assert!(alive(grandchild));

    drop(agent);
    assert!(matches!(ev.next(), AcpEvent::Exited { .. }));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while (alive(pid) || alive(grandchild)) && std::time::Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
    assert!(!alive(pid), "the agent outlived its handle");
    assert!(!alive(grandchild), "the agent's child outlived it");
}

// ---- the fake agent (subprocess side) --------------------------------------

fn fake_agent(mode: &str) -> i32 {
    if mode == "sleep" {
        thread::sleep(Duration::from_secs(120));
        return 0;
    }
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut out = std::io::stdout().lock();
    let mut send = |v: Value| {
        writeln!(out, "{v}").unwrap();
        out.flush().unwrap();
    };
    // Noise before the first message must not confuse the client.
    send(json!("not a JSON-RPC message"));
    let mut grandchild = None;
    while let Some(Ok(line)) = lines.next() {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let reply = |result: Value| json!({"jsonrpc": "2.0", "id": msg["id"], "result": result});
        match msg["method"].as_str().unwrap_or_default() {
            "initialize" => {
                let mut result = init_result();
                if mode == "grandchild" {
                    let child = std::process::Command::new(std::env::current_exe().unwrap())
                        .env(FAKE_ENV, "sleep")
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                        .unwrap();
                    result["agentInfo"]["version"] = json!(child.id().to_string());
                    grandchild = Some(child);
                }
                send(reply(result));
            }
            "session/new" => send(reply(json!({"sessionId": "s1"}))),
            "session/prompt" => {
                send(update(
                    "s1",
                    json!({"sessionUpdate": "agent_message_chunk",
                           "content": {"type": "text", "text": "Hi from the fake agent"}}),
                ));
                if mode == "crash" {
                    eprintln!("fake agent: boom");
                    return 3;
                }
                send(permission_request("p1", "s1"));
                let selected = loop {
                    let Some(Ok(line)) = lines.next() else {
                        return 1;
                    };
                    let Ok(answer) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    if answer["id"] == "p1" && answer.get("method").is_none() {
                        break answer["result"]["outcome"]["optionId"].clone();
                    }
                };
                send(update(
                    "s1",
                    json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1",
                           "status": "completed", "rawOutput": {"selected": selected}}),
                ));
                send(reply(json!({"stopReason": "end_turn"})));
            }
            _ => {}
        }
    }
    drop(grandchild);
    0
}
