//! `kantor mcp` end to end: the real binary on stdio, talking to a
//! stand-in for the running app on a socket in a temporary directory.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn kantor_mcp(dir: &std::path::Path, extra: &[&str]) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_kantor"))
        .arg("mcp")
        .args(extra)
        .env("KANTOR_SOCKET_DIR", dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start kantor mcp")
}

/// Send JSON-RPC lines, close stdin, return the reply lines.
fn converse(mut child: std::process::Child, msgs: &[Value]) -> Vec<Value> {
    let mut stdin = child.stdin.take().unwrap();
    for m in msgs {
        writeln!(stdin, "{m}").unwrap();
    }
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).expect("stdout carries only JSON"))
        .collect()
}

fn call(id: u64, name: &str, args: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
           "params": {"name": name, "arguments": args}})
}

fn text(reply: &Value) -> &str {
    reply["result"]["content"][0]["text"].as_str().unwrap()
}

#[test]
fn forwards_tool_calls_to_the_app() {
    let dir = tempfile::tempdir().unwrap();
    let listener = UnixListener::bind(dir.path().join("kantor-1.sock")).unwrap();
    // The stand-in app: answers list_documents, fails anything else.
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut writer = stream.try_clone().unwrap();
        for line in BufReader::new(stream).lines() {
            let req: Value = serde_json::from_str(&line.unwrap()).unwrap();
            let reply = match req["tool"].as_str() {
                Some("list_documents") => {
                    json!({"id": req["id"], "result": {"documents": [{"id": 7, "kind": "sheets"}]}})
                }
                _ => json!({"id": req["id"], "error": format!("no {}", req["tool"])}),
            };
            writeln!(writer, "{reply}").unwrap();
        }
    });
    let replies = converse(
        kantor_mcp(dir.path(), &[]),
        &[
            json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
                   "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                              "clientInfo": {"name": "test", "version": "0"}}}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            call(2, "list_documents", json!({})),
            call(3, "sheets_read_range", json!({"range": "A1"})),
        ],
    );
    assert_eq!(replies.len(), 4);
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    assert!(replies[1]["result"]["tools"].as_array().unwrap().len() > 20);
    assert_eq!(replies[2]["result"]["isError"], false);
    let docs: Value = serde_json::from_str(text(&replies[2])).unwrap();
    assert_eq!(docs["documents"][0]["id"], 7);
    assert_eq!(replies[3]["result"]["isError"], true);
    assert_eq!(text(&replies[3]), "no \"sheets_read_range\"");
}

#[test]
fn says_when_kantor_is_not_running() {
    let dir = tempfile::tempdir().unwrap();
    let replies = converse(
        kantor_mcp(dir.path(), &["--no-launch"]),
        &[call(1, "list_documents", json!({}))],
    );
    assert_eq!(replies[0]["result"]["isError"], true);
    assert!(text(&replies[0]).contains("isn't running"));
}

#[test]
fn prints_setup() {
    let out = Command::new(env!("CARGO_BIN_EXE_kantor"))
        .args(["mcp", "--print-config"])
        .output()
        .unwrap();
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("claude mcp add kantor -- "), "{s}");
    assert!(s.contains("\"args\": ["), "{s}");
}
