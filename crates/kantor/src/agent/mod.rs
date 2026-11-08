//! What the MCP tools do to documents, written against the engines
//! (`Workbook`, `Document`, `Presentation`) so it can be tested without a
//! window. `workspace/agent.rs` finds the document, runs these inside the
//! views' `agent_edit` hooks (undo step, redraw, highlight) and answers.

pub mod docs;
pub mod sheets;
pub mod slides;

use anyhow::{Result, bail};
use serde_json::Value;

/// Longest text accepted for one cell, paragraph or shape.
pub const MAX_TEXT: usize = 32_767;

fn get<'a>(args: &'a Value, key: &str) -> Option<&'a Value> {
    args.get(key).filter(|v| !v.is_null())
}

pub fn opt_str<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>> {
    match get(args, key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => bail!("`{key}` must be a string"),
    }
}

pub fn req_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    opt_str(args, key)?.ok_or_else(|| anyhow::anyhow!("`{key}` is required"))
}

pub fn opt_int(args: &Value, key: &str) -> Result<Option<i64>> {
    match get(args, key) {
        None => Ok(None),
        Some(v) => match v.as_i64() {
            Some(n) => Ok(Some(n)),
            None => bail!("`{key}` must be a whole number"),
        },
    }
}

pub fn req_int(args: &Value, key: &str) -> Result<i64> {
    opt_int(args, key)?.ok_or_else(|| anyhow::anyhow!("`{key}` is required"))
}

/// A whole number of at least `min`.
pub fn opt_count(args: &Value, key: &str, min: i64) -> Result<Option<u64>> {
    match opt_int(args, key)? {
        Some(n) if n < min => bail!("`{key}` must be at least {min}"),
        other => Ok(other.map(|n| n as u64)),
    }
}

pub fn opt_bool(args: &Value, key: &str) -> Result<Option<bool>> {
    match get(args, key) {
        None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => bail!("`{key}` must be true or false"),
    }
}

/// Reject text longer than [`MAX_TEXT`].
pub fn check_len(what: &str, text: &str) -> Result<()> {
    if text.chars().count() > MAX_TEXT {
        bail!("{what} is longer than {MAX_TEXT} characters");
    }
    Ok(())
}

/// The character offset of byte `offset` in `text`.
pub fn char_offset(text: &str, offset: usize) -> usize {
    text.get(..offset.min(text.len()))
        .map_or(0, |s| s.chars().count())
}
