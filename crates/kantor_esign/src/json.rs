//! Small helpers for the providers' JSON answers.

use serde_json::Value;

use crate::Error;

/// The value at a `/`-separated path (`data/attributes/id`; numbers index
/// arrays).
pub fn at<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = v;
    for key in path.split('/').filter(|k| !k.is_empty()) {
        cur = match cur {
            Value::Object(m) => m.get(key)?,
            Value::Array(a) => a.get(key.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// A string (or number, as text) at `path`.
pub fn str_at(v: &Value, path: &str) -> Option<String> {
    match at(v, path)? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// A string at `path`, or an error naming what's missing.
pub fn need(v: &Value, path: &str) -> Result<String, Error> {
    str_at(v, path).ok_or_else(|| {
        Error::Provider(format!(
            "the provider's answer has no \"{path}\": {}",
            crate::http::snippet(v.to_string().as_bytes())
        ))
    })
}

/// The human message of an error answer, under the names providers use.
pub fn message(v: &Value) -> Option<String> {
    for p in [
        "message",
        "error_description",
        "error/message",
        "errors/0/detail",
        "errors/0/message",
        "errors/0/title",
        "error",
        "result/desc",
        "desc",
        "detail",
    ] {
        if let Some(s) = str_at(v, p) {
            return Some(s);
        }
    }
    None
}
