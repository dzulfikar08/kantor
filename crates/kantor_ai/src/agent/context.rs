//! The conversation the agent resends each step, kept small.
//!
//! Every step sends the whole conversation again, so what stays in it is
//! paid for on every later step. Three rules keep it lean:
//! - a new picture of a file drops older pictures of the same page, and
//!   every older picture of the file when the file changed since; at most
//!   `max_images` pictures are kept (the oldest go first);
//! - long strings in older tool-call arguments (a whole deck.json passed to
//!   `write_file`) are replaced by a note once the step is over;
//! - large tool results are cut when they arrive, and cut further once
//!   they're a few steps old.
//!
//! Whatever is dropped is replaced by a one-line note, so the model knows
//! it can look again.

use base64::Engine as _;
use serde_json::{Value, json};

use super::Image;

/// A picture still in the conversation.
#[derive(Clone, Debug)]
struct Shown {
    msg: usize,
    part: usize,
    file: String,
    page: Option<String>,
    version: String,
    label: String,
}

/// The messages after the system prompt (OpenAI chat format), with the
/// step each was added in.
#[derive(Clone, Debug, Default)]
pub struct Context {
    messages: Vec<Value>,
    steps: Vec<u32>,
    shown: Vec<Shown>,
}

impl Context {
    /// A conversation starting with the user's request.
    pub fn new(task: &str) -> Self {
        let mut c = Self::default();
        c.push(0, json!({"role": "user", "content": task}));
        c
    }

    /// A conversation that goes on from an earlier job's (`history`, as
    /// [`super::Outcome::messages`] gave it) with the user's next request.
    /// Pictures in the history become notes (the model can look again),
    /// earlier reasoning is left out, and tool calls left without a result (a job stopped mid-step) get
    /// one, so the API takes the conversation.
    pub fn resume(history: Vec<Value>, task: &str) -> Self {
        let mut c = Self::default();
        let mut pending: Vec<String> = Vec::new();
        for mut m in history {
            let role = m.get("role").and_then(Value::as_str).unwrap_or_default();
            if role == "tool" {
                if let Some(id) = m.get("tool_call_id").and_then(Value::as_str) {
                    pending.retain(|p| p != id);
                }
            } else {
                c.close_calls(&mut pending);
            }
            if role == "assistant"
                && let Some(calls) = m.get("tool_calls").and_then(Value::as_array)
            {
                pending = calls
                    .iter()
                    .filter_map(|call| call.get("id").and_then(Value::as_str))
                    .map(str::to_owned)
                    .collect();
            }
            // Reasoning only helps the request it was made for.
            if let Some(o) = m.as_object_mut() {
                o.remove("reasoning_content");
            }
            if let Some(parts) = m.get_mut("content").and_then(Value::as_array_mut) {
                for p in parts {
                    if p.get("type").and_then(Value::as_str) == Some("image_url") {
                        *p = json!({"type": "text", "text": "[picture from an earlier request removed; look again if you need it]"});
                    }
                }
            }
            c.push(0, m);
        }
        c.close_calls(&mut pending);
        c.push(0, json!({"role": "user", "content": task}));
        c
    }

    /// Results for tool calls that got none.
    fn close_calls(&mut self, pending: &mut Vec<String>) {
        for id in pending.drain(..) {
            self.push(
                0,
                json!({"role": "tool", "tool_call_id": id, "content": "not run: the job was stopped"}),
            );
        }
    }

    pub fn messages(&self) -> &[Value] {
        &self.messages
    }

    pub fn into_messages(self) -> Vec<Value> {
        self.messages
    }

    pub fn push(&mut self, step: u32, message: Value) {
        self.messages.push(message);
        self.steps.push(step);
    }

    /// Pictures in the conversation now.
    pub fn image_count(&self) -> usize {
        self.shown.len()
    }

    /// Add `images` as one user message (they can't go in `tool` messages
    /// on most OpenAI-compatible servers), after dropping older pictures
    /// they replace, then keep at most `max_images`. Returns the labels of
    /// pictures dropped.
    pub fn push_images(
        &mut self,
        step: u32,
        intro: &str,
        images: Vec<Image>,
        max_images: usize,
    ) -> Vec<String> {
        let mut dropped = Vec::new();
        if images.is_empty() {
            return dropped;
        }
        // Only the newest `max_images` of this batch can stay anyway.
        let skip = images.len().saturating_sub(max_images.max(1));
        let images: Vec<Image> = images.into_iter().skip(skip).collect();
        for img in &images {
            let stale: Vec<usize> = self
                .shown
                .iter()
                .enumerate()
                .filter(|(_, s)| {
                    s.file == img.file && (s.page == img.page || s.version != img.version)
                })
                .map(|(i, _)| i)
                .collect();
            for i in stale.into_iter().rev() {
                let s = self.shown.remove(i);
                let note = if s.version != img.version {
                    format!(
                        "[picture of {} removed: the file changed since; a newer picture follows]",
                        s.label
                    )
                } else {
                    format!(
                        "[picture of {} removed: a newer picture of it follows]",
                        s.label
                    )
                };
                self.replace_part(s.msg, s.part, note);
                dropped.push(s.label);
            }
        }
        let msg = self.messages.len();
        let mut parts = vec![json!({"type": "text", "text": intro})];
        for img in images {
            parts.push(json!({"type": "text", "text": format!("{}:", img.label)}));
            let url = format!(
                "data:{};base64,{}",
                img.mime,
                base64::engine::general_purpose::STANDARD.encode(&img.bytes)
            );
            self.shown.push(Shown {
                msg,
                part: parts.len(),
                file: img.file,
                page: img.page,
                version: img.version,
                label: img.label,
            });
            parts.push(json!({"type": "image_url", "image_url": {"url": url}}));
        }
        self.push(step, json!({"role": "user", "content": parts}));
        while self.shown.len() > max_images {
            let s = self.shown.remove(0);
            let note = format!(
                "[picture of {} removed to save space; look again if you need it]",
                s.label
            );
            self.replace_part(s.msg, s.part, note);
            dropped.push(s.label);
        }
        dropped
    }

    fn replace_part(&mut self, msg: usize, part: usize, note: String) {
        if let Some(p) = self
            .messages
            .get_mut(msg)
            .and_then(|m| m.get_mut("content"))
            .and_then(|c| c.get_mut(part))
        {
            *p = json!({"type": "text", "text": note});
        }
    }

    /// Shorten what older steps (before `step`) sent: string values longer
    /// than `args_over` characters in tool-call arguments, and tool
    /// results longer than `result_over` characters from steps before
    /// `results_before`. Messages already short are left byte for byte as
    /// they were (a changed message ends the provider's prompt cache
    /// there).
    pub fn compact(
        &mut self,
        step: u32,
        args_over: usize,
        results_before: u32,
        result_over: usize,
    ) {
        for (m, &s) in self.messages.iter_mut().zip(&self.steps) {
            if s >= step {
                continue;
            }
            match m.get("role").and_then(Value::as_str) {
                Some("assistant") => {
                    let Some(calls) = m.get_mut("tool_calls").and_then(Value::as_array_mut) else {
                        continue;
                    };
                    for call in calls {
                        let Some(args) = call.pointer_mut("/function/arguments") else {
                            continue;
                        };
                        let Some(text) = args.as_str() else { continue };
                        if text.len() <= args_over {
                            continue;
                        }
                        let Ok(mut v) = serde_json::from_str::<Value>(text) else {
                            continue;
                        };
                        if shorten_strings(&mut v, args_over) {
                            *args = Value::String(v.to_string());
                        }
                    }
                }
                Some("tool") if s < results_before => {
                    let Some(text) = m.get("content").and_then(Value::as_str) else {
                        continue;
                    };
                    let n = text.chars().count();
                    if n > result_over && !text.contains(CUT_MARK) {
                        let keep = result_over / 2;
                        let short = format!(
                            "{}\n[{CUT_MARK} from {n} to {keep} characters to save space; run the tool again if you need the rest]",
                            cut(text, keep)
                        );
                        m["content"] = Value::String(short);
                    }
                }
                _ => {}
            }
        }
    }
}

const CUT_MARK: &str = "older result cut";

/// Replace strings longer than `over` characters (anywhere in `v`) by a
/// note; whether any was.
fn shorten_strings(v: &mut Value, over: usize) -> bool {
    match v {
        Value::String(s) if s.chars().count() > over && !s.starts_with("[left out:") => {
            *s = format!(
                "[left out: {} characters sent in an earlier step; read the file to see what it holds now]",
                s.chars().count()
            );
            true
        }
        Value::Array(items) => {
            let mut any = false;
            for i in items {
                any |= shorten_strings(i, over);
            }
            any
        }
        Value::Object(map) => {
            let mut any = false;
            for i in map.values_mut() {
                any |= shorten_strings(i, over);
            }
            any
        }
        _ => false,
    }
}

/// The first `n` characters of `text`, ending at a line break near the
/// end when there is one.
pub fn cut(text: &str, n: usize) -> &str {
    let end = text.char_indices().nth(n).map_or(text.len(), |(i, _)| i);
    let head = &text[..end];
    match head.rfind('\n') {
        Some(i) if i > end * 9 / 10 => &head[..i],
        _ => head,
    }
}

/// `text` cut to `max` characters with a note saying how much was left
/// out and how to see it (`hint`).
pub fn truncate_result(text: &str, max: usize, hint: Option<&str>) -> String {
    let n = text.chars().count();
    if n <= max {
        return text.to_owned();
    }
    let head = cut(text, max);
    let shown = head.chars().count();
    format!(
        "{head}\n[truncated: showed {shown} of {n} characters. {}]",
        hint.unwrap_or("Ask for less at a time (a range, a page, offset and limit).")
    )
}
