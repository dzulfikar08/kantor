//! A model that plays back a script, for tests without a network (here and
//! in the crates that bring the tools).

use std::collections::VecDeque;
use std::sync::Mutex;

use serde_json::Value;

use super::{Model, StepOptions};
use crate::AiError;
use crate::client::{Cancel, Reply, ToolCall, Usage};

/// One scripted turn.
pub enum Turn {
    Reply(Reply),
    Fail(AiError),
}

/// Replies in order; records what each step was sent.
#[derive(Default)]
pub struct Scripted {
    turns: Mutex<VecDeque<Turn>>,
    /// The messages sent on each step.
    pub seen: Mutex<Vec<Vec<Value>>>,
    /// The system prompt last sent.
    pub system: Mutex<String>,
    /// The tools and options each step was sent.
    pub tools: Mutex<Vec<Value>>,
    pub opts: Mutex<Vec<StepOptions>>,
}

impl Scripted {
    pub fn new(turns: Vec<Turn>) -> Self {
        Self {
            turns: Mutex::new(turns.into()),
            ..Self::default()
        }
    }

    /// The messages sent on step `n` (from 1).
    pub fn sent(&self, n: usize) -> Vec<Value> {
        self.seen
            .lock()
            .map(|s| s.get(n - 1).cloned().unwrap_or_default())
            .unwrap_or_default()
    }

    /// The names of the tools offered on step `n` (from 1).
    pub fn tools_on(&self, n: usize) -> Vec<String> {
        self.tools
            .lock()
            .ok()
            .and_then(|t| t.get(n - 1).cloned())
            .and_then(|t| t.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|t| t.pointer("/function/name").and_then(Value::as_str))
            .map(str::to_owned)
            .collect()
    }

    /// The options of step `n` (from 1).
    pub fn opts_on(&self, n: usize) -> StepOptions {
        self.opts
            .lock()
            .ok()
            .and_then(|o| o.get(n - 1).cloned())
            .unwrap_or_default()
    }

    pub fn steps(&self) -> usize {
        self.seen.lock().map(|s| s.len()).unwrap_or(0)
    }
}

/// A turn of text only.
pub fn text(t: &str) -> Turn {
    Turn::Reply(Reply {
        text: t.to_owned(),
        usage: Some(Usage {
            input: 1000,
            cached: 0,
            output: 50,
        }),
        ..Reply::default()
    })
}

/// A turn calling tools: `(name, arguments JSON)`.
pub fn calls(list: &[(&str, Value)]) -> Turn {
    Turn::Reply(Reply {
        tool_calls: list
            .iter()
            .enumerate()
            .map(|(i, (name, args))| ToolCall {
                id: format!("c{i}"),
                name: (*name).to_owned(),
                arguments: args.to_string(),
            })
            .collect(),
        usage: Some(Usage {
            input: 1000,
            cached: 200,
            output: 100,
        }),
        ..Reply::default()
    })
}

/// A turn calling one tool.
pub fn call(name: &str, args: Value) -> Turn {
    calls(&[(name, args)])
}

impl Model for Scripted {
    fn complete(
        &self,
        system: &str,
        messages: &[Value],
        tools: &Value,
        cancel: &Cancel,
        on_text: &mut dyn FnMut(&str),
    ) -> Result<Reply, AiError> {
        self.complete_with(
            system,
            messages,
            tools,
            &StepOptions::default(),
            cancel,
            on_text,
        )
    }

    fn complete_with(
        &self,
        system: &str,
        messages: &[Value],
        tools: &Value,
        opts: &StepOptions,
        cancel: &Cancel,
        on_text: &mut dyn FnMut(&str),
    ) -> Result<Reply, AiError> {
        if let Ok(mut t) = self.tools.lock() {
            t.push(tools.clone());
        }
        if let Ok(mut o) = self.opts.lock() {
            o.push(opts.clone());
        }
        if cancel.is_cancelled() {
            return Err(AiError::Cancelled);
        }
        if let Ok(mut s) = self.seen.lock() {
            s.push(messages.to_vec());
        }
        if let Ok(mut s) = self.system.lock() {
            *s = system.to_owned();
        }
        let turn = self.turns.lock().ok().and_then(|mut t| t.pop_front());
        match turn {
            Some(Turn::Reply(r)) => {
                if !r.text.is_empty() {
                    on_text(&r.text);
                }
                Ok(r)
            }
            Some(Turn::Fail(e)) => Err(e),
            None => Err(AiError::BadReply("the script ran out".into())),
        }
    }
}
