//! What the MCP tools do to documents, written against the engines
//! (`Workbook`, `Document`, `Presentation`) so it can be tested without a
//! window. `workspace/agent.rs` finds the document, runs these inside the
//! views' `agent_edit` hooks (undo step, redraw, highlight) and answers.

pub mod docs;
pub mod docs_author;
pub mod docs_check;
pub mod docs_edit;
pub mod docs_inline;
pub mod docs_list;
pub mod docs_look;
pub mod docs_pkg;
pub mod docs_schema;
pub mod sheets;
pub mod sheets_author;
pub mod sheets_chart;
pub mod sheets_check;
pub mod sheets_clean;
pub mod sheets_list;
pub mod sheets_objects;
pub mod sheets_schema;
pub mod slides;
pub mod slides_author;
pub mod slides_chart;
pub mod slides_check;
pub mod slides_components;
pub mod slides_icons;
pub mod slides_schema;
pub mod slides_style;

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
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

/// An absolute path argument (`~/` allowed).
pub fn path_arg(text: &str) -> Result<PathBuf> {
    let text = text.trim();
    let path = match text.strip_prefix("~/") {
        Some(rest) => kantor_core::dirs::user_home_dir()
            .context("HOME is not set")?
            .join(rest),
        None => PathBuf::from(text),
    };
    if !path.is_absolute() {
        bail!("give an absolute path (got \"{text}\")");
    }
    Ok(path)
}
