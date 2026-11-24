//! JSON-RPC 2.0 framing: one JSON object per line (ACP's stdio transport).

use serde_json::{Value, json};

use crate::types::RpcError;

/// JSON-RPC error codes.
pub(crate) const METHOD_NOT_FOUND: i64 = -32601;
pub(crate) const INVALID_PARAMS: i64 = -32602;
pub(crate) const INTERNAL_ERROR: i64 = -32603;

/// A message from the agent.
#[derive(Debug)]
pub(crate) enum Incoming {
    /// The answer to one of our requests (our ids are integers).
    Response {
        id: u64,
        result: Result<Value, RpcError>,
    },
    /// The agent asks the client something; `id` is echoed back verbatim.
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
}

/// Parses one line; `None` for blank lines, non-JSON output (some agents and
/// `npx` print banners on stdout) and messages that aren't JSON-RPC.
pub(crate) fn parse_line(line: &str) -> Option<Incoming> {
    let line = line.trim();
    if !line.starts_with('{') {
        return None;
    }
    let mut msg: Value = serde_json::from_str(line).ok()?;
    let obj = msg.as_object_mut()?;
    let params = obj.remove("params").unwrap_or(Value::Null);
    let id = obj.remove("id");
    if let Some(method) = obj.get("method").and_then(Value::as_str) {
        let method = method.to_owned();
        return Some(match id {
            Some(id) if !id.is_null() => Incoming::Request { id, method, params },
            _ => Incoming::Notification { method, params },
        });
    }
    let id = id?.as_u64()?;
    let result = if let Some(error) = obj.remove("error") {
        Err(serde_json::from_value(error.clone()).unwrap_or(RpcError {
            code: INTERNAL_ERROR,
            message: "malformed error".to_owned(),
            data: Some(error),
        }))
    } else {
        Ok(obj.remove("result").unwrap_or(Value::Null))
    };
    Some(Incoming::Response { id, result })
}

pub(crate) fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

pub(crate) fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

pub(crate) fn response(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

pub(crate) fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_three_kinds() {
        let Some(Incoming::Response { id: 3, result }) =
            parse_line(r#"{"jsonrpc":"2.0","id":3,"result":{"a":1}}"#)
        else {
            panic!("response")
        };
        assert_eq!(result.unwrap()["a"], 1);

        let Some(Incoming::Response { id: 4, result }) = parse_line(
            r#"{"jsonrpc":"2.0","id":4,"error":{"code":-32000,"message":"Authentication required"}}"#,
        ) else {
            panic!("error response")
        };
        assert_eq!(result.unwrap_err().code, -32000);

        let Some(Incoming::Request { id, method, .. }) = parse_line(
            r#"{"jsonrpc":"2.0","id":"p1","method":"session/request_permission","params":{}}"#,
        ) else {
            panic!("request")
        };
        assert_eq!(id, "p1");
        assert_eq!(method, "session/request_permission");

        let Some(Incoming::Notification { method, params }) =
            parse_line(r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s"}}"#)
        else {
            panic!("notification")
        };
        assert_eq!(method, "session/update");
        assert_eq!(params["sessionId"], "s");
    }

    #[test]
    fn skips_noise() {
        assert!(parse_line("").is_none());
        assert!(parse_line("npm warn exec The following package was not found").is_none());
        assert!(parse_line("{not json").is_none());
        assert!(parse_line(r#"{"jsonrpc":"2.0","id":"x","result":null}"#).is_none());
    }
}
