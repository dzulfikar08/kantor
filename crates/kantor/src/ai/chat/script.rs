//! A model that plays a script instead of calling a provider, so the chat
//! panel can be shown and checked without an account or a network
//! (`KANTOR_CHAT_SCRIPT=script.json kantor`). The tools are the real ones:
//! a scripted `build` builds, `look` renders.
//!
//! The script is a JSON array of turns:
//!
//! ```json
//! [{"delay_ms": 800, "text": "Building the deck…",
//!   "calls": [{"name": "build", "args": {"source": "deck.json", "output": "deck.pptx"}}]}]
//! ```

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use kantor_ai::agent::Model;
use kantor_ai::{AiError, Cancel, Reply, ToolCall, Usage};
use serde_json::Value;

/// The environment variable naming the script.
pub const SCRIPT_ENV: &str = "KANTOR_CHAT_SCRIPT";

pub struct Played {
    turns: Mutex<VecDeque<Value>>,
    step: Mutex<u64>,
}

impl Played {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("can't read the script {}: {e}", path.display()))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("bad script: {e}"))?;
        let turns = v.as_array().ok_or("the script must be a JSON array")?;
        Ok(Self {
            turns: Mutex::new(turns.iter().cloned().collect()),
            step: Mutex::new(0),
        })
    }
}

/// Sleep `d`, or stop early when cancelled.
fn pause(d: Duration, cancel: &Cancel) -> Result<(), AiError> {
    let end = Instant::now() + d;
    loop {
        if cancel.is_cancelled() {
            return Err(AiError::Cancelled);
        }
        let now = Instant::now();
        if now >= end {
            return Ok(());
        }
        std::thread::sleep((end - now).min(Duration::from_millis(50)));
    }
}

impl Model for Played {
    fn complete(
        &self,
        _system: &str,
        _messages: &[Value],
        _tools: &Value,
        cancel: &Cancel,
        on_text: &mut dyn FnMut(&str),
    ) -> Result<Reply, AiError> {
        let turn = self
            .turns
            .lock()
            .ok()
            .and_then(|mut t| t.pop_front())
            .ok_or_else(|| AiError::BadReply("the script ran out".into()))?;
        let step = self.step.lock().map_or(1, |mut s| {
            *s += 1;
            *s
        });
        let delay = turn.get("delay_ms").and_then(Value::as_u64).unwrap_or(400);
        pause(Duration::from_millis(delay), cancel)?;
        let text = turn
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        // Streamed a word at a time, like a provider.
        for word in text.split_inclusive(' ') {
            on_text(word);
            pause(Duration::from_millis(30), cancel)?;
        }
        let tool_calls = turn
            .get("calls")
            .and_then(Value::as_array)
            .map(|calls| {
                calls
                    .iter()
                    .enumerate()
                    .map(|(i, c)| ToolCall {
                        id: format!("s{step}_{i}"),
                        name: c
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        arguments: c.get("args").cloned().unwrap_or(Value::Null).to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let input = 6_000 + 2_500 * step;
        Ok(Reply {
            text,
            tool_calls,
            usage: Some(Usage {
                input,
                cached: input / 2,
                output: 350,
            }),
            ..Reply::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn plays_the_turns_in_order() {
        let m = Played::parse(
            r#"[{"delay_ms": 0, "text": "Halo dunia", "calls": [{"name": "look", "args": {"file": "a.pptx"}}]}]"#,
        )
        .unwrap();
        let mut streamed = String::new();
        let r = m
            .complete("", &[], &Value::Null, &Cancel::default(), &mut |t| {
                streamed.push_str(t)
            })
            .unwrap();
        assert_eq!(streamed, "Halo dunia");
        assert_eq!(r.tool_calls[0].name, "look");
        assert_eq!(r.tool_calls[0].arguments, r#"{"file":"a.pptx"}"#);
        assert!(
            m.complete("", &[], &Value::Null, &Cancel::default(), &mut |_| {})
                .is_err()
        );
        assert!(Played::parse("{}").is_err());
    }
}
