//! MCP over stdio: newline-delimited JSON-RPC 2.0 messages on stdin/stdout
//! (logs go to stderr). Tool calls go to a [`Backend`].
//!
//! Dual-era, as the 2026-07-28 revision describes: a request whose
//! `params._meta` names a protocol version is served statelessly by that
//! revision (`server/discover`, `resultType` on results, error -32022 for
//! versions we don't speak); `initialize` starts a legacy session
//! (2025-11-25 and earlier). Both offer `ping`, `tools/list`, `tools/call`.

use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

use super::tools;
use crate::ipc;

/// Handshake-based (legacy) revisions this server speaks, newest first. It
/// uses only what they all share (tools with text results).
pub const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
/// Stateless (modern) revisions: the version is on every request.
pub const MODERN_VERSIONS: [&str; 1] = ["2026-07-28"];
const VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const UNSUPPORTED_VERSION: i64 = -32022;

const INSTRUCTIONS: &str = "Kantor is the office suite (Sheets, Docs, Slides) the user has open; \
these tools read and change the documents in it live, while the user watches. Start with \
list_documents. Tools act on the document in front unless you pass `document` (an id from \
list_documents); call activate_document so the user sees the one you work on. Every change is \
one undo step the user can take back with Ctrl+Z, and the changed cells, paragraphs or slide \
are selected. Nothing is written to disk until save_document.";

/// Runs tool calls (the running app, or a stand-in in tests).
pub trait Backend {
    fn call(&mut self, tool: &str, args: Value) -> Result<Value, String>;
}

pub struct Server<B> {
    backend: B,
}

fn server_info() -> Value {
    json!({"name": "kantor", "title": "Kantor", "version": env!("CARGO_PKG_VERSION")})
}

fn error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

fn result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// The version to answer `initialize` with: the client's if we speak it,
/// else our newest.
fn negotiate(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|r| PROTOCOL_VERSIONS.iter().find(|v| **v == r))
        .unwrap_or(&PROTOCOL_VERSIONS[0])
}

/// A tool result as MCP content: JSON text, or the error text with `isError`.
fn tool_result(r: Result<Value, String>) -> Value {
    match r {
        Ok(v) => {
            let text = match &v {
                Value::String(s) => s.clone(),
                other => serde_json::to_string_pretty(other).unwrap_or_default(),
            };
            json!({"content": [{"type": "text", "text": text}], "isError": false})
        }
        Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
    }
}

impl<B: Backend> Server<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    /// The reply to one message, if it needs one (notifications and
    /// responses don't).
    pub fn handle(&mut self, line: &str) -> Option<Value> {
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => return Some(error(Value::Null, PARSE_ERROR, format!("parse error: {e}"))),
        };
        let Some(obj) = msg.as_object() else {
            return Some(error(
                Value::Null,
                INVALID_REQUEST,
                "expected one JSON-RPC message object (batches aren't supported)",
            ));
        };
        let id = obj.get("id").cloned();
        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            // A response to something we never send, or junk.
            return id
                .filter(|_| !obj.contains_key("result") && !obj.contains_key("error"))
                .map(|id| error(id, INVALID_REQUEST, "missing method"));
        };
        let Some(id) = id else {
            // Notifications (`notifications/initialized`, cancellations).
            return None;
        };
        let params = obj.get("params").cloned().unwrap_or_else(|| json!({}));
        let modern = params
            .get("_meta")
            .and_then(|m| m.get(VERSION_KEY))
            .map(|v| v.as_str().unwrap_or_default().to_owned());
        if let Some(v) = &modern
            && !MODERN_VERSIONS.contains(&v.as_str())
        {
            let supported: Vec<&str> = MODERN_VERSIONS
                .iter()
                .chain(&PROTOCOL_VERSIONS)
                .copied()
                .collect();
            return Some(json!({"jsonrpc": "2.0", "id": id, "error": {
                "code": UNSUPPORTED_VERSION,
                "message": "Unsupported protocol version",
                "data": {"supported": supported, "requested": v},
            }}));
        }
        let mut reply = self.dispatch(id, method, &params);
        if modern.is_some()
            && let Some(r) = reply.get_mut("result").and_then(Value::as_object_mut)
        {
            r.insert("resultType".into(), json!("complete"));
        }
        Some(reply)
    }

    fn dispatch(&mut self, id: Value, method: &str, params: &Value) -> Value {
        match method {
            "server/discover" => {
                let supported: Vec<&str> = MODERN_VERSIONS
                    .iter()
                    .chain(&PROTOCOL_VERSIONS)
                    .copied()
                    .collect();
                result(
                    id,
                    json!({
                        "supportedVersions": supported,
                        "capabilities": {"tools": {"listChanged": false}},
                        "_meta": {"io.modelcontextprotocol/serverInfo": server_info()},
                        "instructions": INSTRUCTIONS,
                    }),
                )
            }
            "initialize" => {
                let requested = params.get("protocolVersion").and_then(Value::as_str);
                result(
                    id,
                    json!({
                        "protocolVersion": negotiate(requested),
                        "capabilities": {"tools": {"listChanged": false}},
                        "serverInfo": server_info(),
                        "instructions": INSTRUCTIONS,
                    }),
                )
            }
            "ping" => result(id, json!({})),
            "tools/list" => result(id, json!({"tools": tools::list()})),
            "tools/call" => {
                let Some(name) = params.get("name").and_then(Value::as_str) else {
                    return error(id, INVALID_PARAMS, "missing tool name");
                };
                if !tools::exists(name) {
                    return error(id, INVALID_PARAMS, format!("unknown tool: {name}"));
                }
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                if !args.is_object() {
                    return error(id, INVALID_PARAMS, "arguments must be an object");
                }
                result(id, tool_result(self.backend.call(name, args)))
            }
            other => error(id, METHOD_NOT_FOUND, format!("method not found: {other}")),
        }
    }

    /// Serve until stdin closes.
    pub fn run(&mut self, input: &mut impl BufRead, output: &mut impl Write) -> io::Result<()> {
        loop {
            let line = match ipc::read_line(input, ipc::MAX_MESSAGE) {
                Ok(Some(line)) => line,
                Ok(None) => return Ok(()),
                Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                    let reply = error(Value::Null, PARSE_ERROR, e.to_string());
                    writeln!(output, "{reply}")?;
                    output.flush()?;
                    continue;
                }
                Err(e) => return Err(e),
            };
            if line.trim().is_empty() {
                continue;
            }
            if let Some(reply) = self.handle(&line) {
                writeln!(output, "{reply}")?;
                output.flush()?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Echoes the tool and its arguments; `fail` errors.
    struct Fake;

    impl Backend for Fake {
        fn call(&mut self, tool: &str, args: Value) -> Result<Value, String> {
            match args.get("fail") {
                Some(_) => Err("it failed".into()),
                None => Ok(json!({"tool": tool, "args": args})),
            }
        }
    }

    fn ask(server: &mut Server<Fake>, msg: Value) -> Value {
        server.handle(&msg.to_string()).expect("a reply")
    }

    #[std::prelude::v1::test]
    fn initializes_with_a_known_version() {
        let mut s = Server::new(Fake);
        let r = ask(
            &mut s,
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                   "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                              "clientInfo": {"name": "t", "version": "1"}}}),
        );
        assert_eq!(r["id"], 1);
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(r["result"]["serverInfo"]["name"], "kantor");
        assert!(r["result"]["capabilities"]["tools"].is_object());
        let r = ask(
            &mut s,
            json!({"jsonrpc": "2.0", "id": "x", "method": "initialize",
                   "params": {"protocolVersion": "1999-01-01"}}),
        );
        assert_eq!(r["id"], "x");
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
        let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        assert_eq!(s.handle(&note.to_string()), None);
    }

    #[std::prelude::v1::test]
    fn lists_and_calls_tools() {
        let mut s = Server::new(Fake);
        let r = ask(
            &mut s,
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        );
        let tools = r["result"]["tools"].as_array().unwrap();
        assert!(tools.iter().any(|t| t["name"] == "sheets_write_range"));
        for t in tools {
            assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
            assert!(!t["description"].as_str().unwrap().is_empty());
        }
        let r = ask(
            &mut s,
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                   "params": {"name": "list_documents", "arguments": {}}}),
        );
        assert_eq!(r["result"]["isError"], false);
        let text = r["result"]["content"][0]["text"].as_str().unwrap();
        let inner: Value = serde_json::from_str(text).unwrap();
        assert_eq!(inner["tool"], "list_documents");
        let r = ask(
            &mut s,
            json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
                   "params": {"name": "list_documents", "arguments": {"fail": true}}}),
        );
        assert_eq!(r["result"]["isError"], true);
        assert_eq!(r["result"]["content"][0]["text"], "it failed");
    }

    #[std::prelude::v1::test]
    fn reports_protocol_errors() {
        let mut s = Server::new(Fake);
        let r = s.handle("{not json").unwrap();
        assert_eq!(r["error"]["code"], PARSE_ERROR);
        let r = ask(
            &mut s,
            json!([{"jsonrpc": "2.0", "id": 1, "method": "ping"}]),
        );
        assert_eq!(r["error"]["code"], INVALID_REQUEST);
        let r = ask(
            &mut s,
            json!({"jsonrpc": "2.0", "id": 5, "method": "resources/list"}),
        );
        assert_eq!(r["error"]["code"], METHOD_NOT_FOUND);
        let r = ask(
            &mut s,
            json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {"name": "rm_rf"}}),
        );
        assert_eq!(r["error"]["code"], INVALID_PARAMS);
        let r = ask(&mut s, json!({"jsonrpc": "2.0", "id": 7, "method": "ping"}));
        assert_eq!(r["result"], json!({}));
        assert_eq!(s.handle(r#"{"jsonrpc":"2.0","id":9,"result":{}}"#), None);
    }

    #[std::prelude::v1::test]
    fn serves_stateless_requests() {
        let mut s = Server::new(Fake);
        let meta = json!({VERSION_KEY: "2026-07-28", "io.modelcontextprotocol/clientInfo": {"name": "t", "version": "1"}});
        let r = ask(
            &mut s,
            json!({"jsonrpc": "2.0", "id": "d", "method": "server/discover", "params": {"_meta": meta}}),
        );
        assert_eq!(r["result"]["resultType"], "complete");
        assert_eq!(r["result"]["supportedVersions"][0], "2026-07-28");
        assert_eq!(
            r["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
            "kantor"
        );
        let r = ask(
            &mut s,
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                   "params": {"_meta": meta, "name": "list_documents", "arguments": {}}}),
        );
        assert_eq!(r["result"]["resultType"], "complete");
        assert_eq!(r["result"]["isError"], false);
        let r = ask(
            &mut s,
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list",
                   "params": {"_meta": {VERSION_KEY: "1900-01-01"}}}),
        );
        assert_eq!(r["error"]["code"], UNSUPPORTED_VERSION);
        assert_eq!(r["error"]["data"]["requested"], "1900-01-01");
    }

    #[std::prelude::v1::test]
    fn runs_over_streams() {
        let mut s = Server::new(Fake);
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            "\n\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
            "\n"
        );
        let mut out = Vec::new();
        s.run(&mut io::Cursor::new(input), &mut out).unwrap();
        let lines: Vec<&str> = std::str::from_utf8(&out).unwrap().lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[1].contains(r#""id":2"#));
    }
}
