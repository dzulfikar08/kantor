//! One chat completion, streamed, against any [`Api`].
//!
//! Every provider streams: Anthropic, OpenAI and Gemini as server-sent
//! events, Ollama as one JSON object per line. [`Client::complete`] calls
//! `on_text` with each piece and returns the whole text.

use std::io::{BufRead, BufReader};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};

use crate::AiError;
use crate::error::{error_message, truncate};
use crate::provider::{Api, ProviderConfig, ProviderId};
use crate::transport::{HttpRequest, Transport, map_io_error};

/// What to ask. `json` asks for a JSON object reply (natively where the API
/// supports it; the prompt always says so too).
#[derive(Clone, Debug, PartialEq)]
pub struct ChatRequest {
    pub system: String,
    pub user: String,
    pub json: bool,
    pub max_tokens: u32,
    /// The Kantor feature asking (`rewrite`, `formula`, …): sent to Paparan
    /// AI as `paparan_task` so its server-side prompt for that task is used.
    /// Other providers don't get it.
    pub task: Option<&'static str>,
    /// OpenAI-style tool definitions (`[{"type":"function",…}]`), for the
    /// OpenAI-compatible APIs.
    pub tools: Option<Value>,
    /// A whole conversation (OpenAI chat messages after the system one:
    /// user, assistant with `tool_calls`, `tool` results), for the
    /// OpenAI-compatible APIs; replaces `user`. The agent loop
    /// ([`crate::agent`]) sends these, and asks for token usage too.
    pub messages: Option<Vec<Value>>,
    /// For the OpenAI-compatible APIs with `messages`: the most tokens
    /// the reply may have (reasoning included), sent as `max_tokens`. The
    /// agent sets it when a budget is nearly spent; otherwise the server's
    /// own limit applies.
    pub max_output: Option<u32>,
    /// For the OpenAI-compatible APIs with `messages`: `reasoning_effort`
    /// (`low`, `medium`, `high`) for models that think before answering.
    pub reasoning_effort: Option<String>,
}

impl ChatRequest {
    pub fn new(system: impl Into<String>, user: impl Into<String>) -> Self {
        Self {
            system: system.into(),
            user: user.into(),
            json: false,
            max_tokens: 1024,
            task: None,
            tools: None,
            messages: None,
            max_output: None,
            reasoning_effort: None,
        }
    }

    /// Send `messages` instead of one user message (see
    /// [`ChatRequest::messages`]).
    pub fn messages(mut self, messages: Vec<Value>) -> Self {
        self.messages = Some(messages);
        self
    }

    /// Name the Kantor feature asking (see [`ChatRequest::task`]).
    pub fn task(mut self, task: &'static str) -> Self {
        self.task = Some(task);
        self
    }

    pub fn tools(mut self, tools: Value) -> Self {
        self.tools = Some(tools);
        self
    }

    pub fn json(mut self) -> Self {
        self.json = true;
        self
    }

    pub fn max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = n;
        self
    }
}

/// Shared cancel flag, checked between streamed pieces.
#[derive(Clone, Default, Debug)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

const ANTHROPIC_VERSION: &str = "2023-06-01";

/// A tool call the model asked for (OpenAI-compatible APIs), its arguments
/// as the JSON text streamed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Tokens one request used, as the server counted them (OpenAI-compatible
/// APIs send them in the last streamed chunk when asked).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Usage {
    /// Prompt tokens, cached ones included.
    pub input: u64,
    /// The part of `input` read from the provider's prompt cache.
    pub cached: u64,
    pub output: u64,
}

impl Usage {
    pub fn add(&mut self, other: Usage) {
        self.input += other.input;
        self.cached += other.cached;
        self.output += other.output;
    }

    /// From an OpenAI-style `usage` object (`prompt_tokens`,
    /// `completion_tokens`, `prompt_tokens_details.cached_tokens`;
    /// DeepSeek's `prompt_cache_hit_tokens`).
    pub fn from_json(v: &Value) -> Option<Usage> {
        let n = |p: &str| v.pointer(p).and_then(Value::as_u64);
        let input = n("/prompt_tokens").or_else(|| n("/input_tokens"))?;
        Some(Usage {
            input,
            cached: n("/prompt_tokens_details/cached_tokens")
                .or_else(|| n("/prompt_cache_hit_tokens"))
                .or_else(|| n("/input_tokens_details/cached_tokens"))
                .unwrap_or(0)
                .min(input),
            output: n("/completion_tokens")
                .or_else(|| n("/output_tokens"))
                .unwrap_or(0),
        })
    }
}

/// A whole streamed reply: its text and any tool calls.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reply {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    /// Thinking models' reasoning (`reasoning_content` / `reasoning`
    /// deltas), kept apart from the answer.
    pub reasoning: String,
    /// Token usage, when the server sent it.
    pub usage: Option<Usage>,
    /// Why the model stopped (`stop`, `tool_calls`, `length`), when sent.
    pub finish_reason: Option<String>,
}

/// The longest `Retry-After` waited out before trying once more; longer
/// waits are reported to the user.
const MAX_AUTO_WAIT_SECS: u64 = 10;

pub struct Client {
    config: ProviderConfig,
    transport: Arc<dyn Transport>,
}

impl Client {
    pub fn new(config: ProviderConfig, transport: Arc<dyn Transport>) -> Self {
        Self { config, transport }
    }

    pub fn config(&self) -> &ProviderConfig {
        &self.config
    }

    /// The HTTP request for `req` (public for tests and diagnostics).
    pub fn build_request(&self, req: &ChatRequest) -> HttpRequest {
        build_request(&self.config, req)
    }

    /// Send `req` and stream the reply. Blocking: call from a worker thread.
    pub fn complete(
        &self,
        req: &ChatRequest,
        cancel: &Cancel,
        on_text: &mut dyn FnMut(&str),
    ) -> Result<String, AiError> {
        let reply = self.complete_reply(req, cancel, on_text)?;
        if reply.text.trim().is_empty() {
            return Err(AiError::BadReply("the model sent an empty answer".into()));
        }
        Ok(reply.text)
    }

    /// [`Client::complete`], keeping tool calls too (a reply may be only
    /// tool calls).
    pub fn complete_reply(
        &self,
        req: &ChatRequest,
        cancel: &Cancel,
        on_text: &mut dyn FnMut(&str),
    ) -> Result<Reply, AiError> {
        if cancel.is_cancelled() {
            return Err(AiError::Cancelled);
        }
        let http = self.build_request(req);
        log::info!(
            "AI request: {} model {} ({} bytes)",
            self.config.provider.key(),
            self.config.model,
            http.body.len()
        );
        let paparan = self.config.provider == ProviderId::Paparan;
        let mut waited = false;
        let resp = loop {
            let resp = self.transport.send(&http)?;
            if cancel.is_cancelled() {
                return Err(AiError::Cancelled);
            }
            if paparan {
                crate::paparan::note_response(&resp);
            }
            if (200..300).contains(&resp.status) {
                break resp;
            }
            let err = if paparan {
                crate::paparan::error_from_response(resp)
            } else {
                let status = resp.status;
                AiError::Http {
                    status,
                    message: error_message(&resp.read_text()),
                }
            };
            // A short Retry-After is waited out once.
            if let AiError::RateLimited {
                retry_after: Some(secs),
                ..
            } = &err
                && *secs <= MAX_AUTO_WAIT_SECS
                && !waited
            {
                waited = true;
                wait(std::time::Duration::from_secs(*secs), cancel)?;
                continue;
            }
            return Err(err);
        };
        let host = http.url.split('?').next().unwrap_or_default().to_owned();
        read_reply(
            self.config.api(),
            BufReader::new(resp.body),
            cancel,
            on_text,
            &host,
        )
    }

    /// A tiny request that proves the key, address and model work.
    pub fn test_connection(&self, cancel: &Cancel) -> Result<String, AiError> {
        let req = ChatRequest::new(
            "You are a connection test. Reply with the single word OK.",
            "Reply with OK.",
        )
        .max_tokens(16);
        self.complete(&req, cancel, &mut |_| {})
    }
}

/// Sleep `d`, waking to check `cancel`.
pub(crate) fn wait(d: std::time::Duration, cancel: &Cancel) -> Result<(), AiError> {
    let end = std::time::Instant::now() + d;
    loop {
        if cancel.is_cancelled() {
            return Err(AiError::Cancelled);
        }
        let now = std::time::Instant::now();
        if now >= end {
            return Ok(());
        }
        std::thread::sleep((end - now).min(std::time::Duration::from_millis(100)));
    }
}

pub fn build_request(cfg: &ProviderConfig, req: &ChatRequest) -> HttpRequest {
    let base = cfg.base_url.trim_end_matches('/');
    let key = cfg.api_key.clone().unwrap_or_default();
    match cfg.api() {
        Api::Anthropic => {
            let body = json!({
                "model": cfg.model,
                "max_tokens": req.max_tokens,
                "system": req.system,
                "messages": [{"role": "user", "content": req.user}],
                "stream": true,
            });
            HttpRequest::post(
                format!("{base}/v1/messages"),
                vec![
                    ("x-api-key", key),
                    ("anthropic-version", ANTHROPIC_VERSION.to_owned()),
                ],
                body.to_string(),
            )
        }
        Api::OpenAi => {
            // No temperature / max_tokens: newer OpenAI models reject them,
            // and `response_format` isn't supported by every compatible
            // server, so JSON is asked for in the prompt only.
            let mut body = json!({
                "model": cfg.model,
                "messages": [
                    {"role": "system", "content": req.system},
                    {"role": "user", "content": req.user},
                ],
                "stream": true,
            });
            if let Some(messages) = &req.messages {
                let mut all = vec![json!({"role": "system", "content": req.system})];
                all.extend(messages.iter().cloned());
                body["messages"] = Value::Array(all);
                body["stream_options"] = json!({"include_usage": true});
                if let Some(n) = req.max_output {
                    body["max_tokens"] = json!(n);
                }
                if let Some(e) = req.reasoning_effort.as_deref().filter(|e| !e.is_empty()) {
                    body["reasoning_effort"] = json!(e);
                }
            }
            if let Some(tools) = &req.tools {
                body["tools"] = tools.clone();
            }
            let mut headers = Vec::new();
            if !key.is_empty() {
                headers.push(("authorization", format!("Bearer {key}")));
            }
            match cfg.provider {
                ProviderId::OpenRouter => headers.push(("x-title", "Kantor".to_owned())),
                ProviderId::Paparan => {
                    if let Some(task) = req.task {
                        body["paparan_task"] = json!(task);
                    }
                    headers.push((
                        "x-kantor-version",
                        crate::paparan::CLIENT_VERSION.to_owned(),
                    ));
                }
                _ => {}
            }
            HttpRequest::post(
                format!("{base}/chat/completions"),
                headers,
                body.to_string(),
            )
        }
        Api::Gemini => {
            let mut generation = json!({ "maxOutputTokens": req.max_tokens });
            if req.json {
                generation["responseMimeType"] = json!("application/json");
            }
            let body = json!({
                "systemInstruction": {"parts": [{"text": req.system}]},
                "contents": [{"role": "user", "parts": [{"text": req.user}]}],
                "generationConfig": generation,
            });
            HttpRequest::post(
                format!(
                    "{base}/v1beta/models/{}:streamGenerateContent?alt=sse",
                    cfg.model
                ),
                vec![("x-goog-api-key", key)],
                body.to_string(),
            )
        }
        Api::Ollama => {
            let mut body = json!({
                "model": cfg.model,
                "messages": [
                    {"role": "system", "content": req.system},
                    {"role": "user", "content": req.user},
                ],
                "stream": true,
                "options": {"num_predict": req.max_tokens},
            });
            if req.json {
                body["format"] = json!("json");
            }
            HttpRequest::post(format!("{base}/api/chat"), Vec::new(), body.to_string())
        }
    }
}

/// Read a streamed reply to the end, calling `on_text` per piece; an empty
/// answer is an error.
pub fn read_stream(
    api: Api,
    reader: impl BufRead,
    cancel: &Cancel,
    on_text: &mut dyn FnMut(&str),
    host: &str,
) -> Result<String, AiError> {
    let reply = read_reply(api, reader, cancel, on_text, host)?;
    if reply.text.trim().is_empty() {
        return Err(AiError::BadReply("the model sent an empty answer".into()));
    }
    Ok(reply.text)
}

/// Read a streamed reply to the end: its text (`on_text` per piece) and,
/// for the OpenAI-compatible APIs, tool calls assembled from their deltas.
/// Empty only if there's neither.
pub fn read_reply(
    api: Api,
    reader: impl BufRead,
    cancel: &Cancel,
    on_text: &mut dyn FnMut(&str),
    host: &str,
) -> Result<Reply, AiError> {
    let mut out = String::new();
    let mut calls: Vec<ToolCall> = Vec::new();
    let mut reasoning = String::new();
    let mut usage = None;
    let mut finish_reason = None;
    for line in reader.lines() {
        if cancel.is_cancelled() {
            return Err(AiError::Cancelled);
        }
        let line = line.map_err(|e| map_io_error(&e, host))?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let payload = if api == Api::Ollama {
            line
        } else {
            // SSE: only `data:` lines carry content (`event:` names repeat
            // the payload's own `type`).
            match line.strip_prefix("data:") {
                Some(p) => p.trim(),
                None => {
                    crate::paparan::note_stream_comment(line);
                    continue;
                }
            }
        };
        if payload == "[DONE]" {
            break;
        }
        let value: Value = serde_json::from_str(payload).map_err(|_| {
            AiError::BadReply(format!("unexpected data: {}", truncate(payload, 120)))
        })?;
        if api == Api::OpenAi {
            if let Some(deltas) = value
                .pointer("/choices/0/delta/tool_calls")
                .and_then(Value::as_array)
            {
                for d in deltas {
                    add_tool_delta(&mut calls, d);
                }
            }
            for key in ["reasoning_content", "reasoning"] {
                if let Some(t) = value
                    .pointer(&format!("/choices/0/delta/{key}"))
                    .and_then(Value::as_str)
                {
                    // Some servers send both keys with the same text.
                    reasoning.push_str(t);
                    break;
                }
            }
            if let Some(r) = value
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)
            {
                finish_reason = Some(r.to_owned());
            }
            if let Some(u) = value.get("usage").and_then(Usage::from_json) {
                usage = Some(u);
            }
        }
        match piece(api, &value)? {
            Piece::Text(t) => {
                if !t.is_empty() {
                    on_text(&t);
                    out.push_str(&t);
                }
            }
            Piece::Done => break,
            Piece::Skip => {}
        }
    }
    if out.trim().is_empty() && calls.is_empty() && reasoning.trim().is_empty() {
        return Err(AiError::BadReply("the model sent an empty answer".into()));
    }
    Ok(Reply {
        text: out,
        tool_calls: calls,
        reasoning,
        usage,
        finish_reason,
    })
}

/// Merge one streamed `tool_calls` delta: the first names the call (`id`,
/// `function.name`), later ones with the same `index` append arguments.
fn add_tool_delta(calls: &mut Vec<ToolCall>, d: &Value) {
    let index = d
        .get("index")
        .and_then(Value::as_u64)
        .map_or(calls.len().saturating_sub(1), |i| i as usize);
    while calls.len() <= index {
        calls.push(ToolCall::default());
    }
    let call = &mut calls[index];
    if let Some(id) = d.get("id").and_then(Value::as_str) {
        call.id = id.to_owned();
    }
    if let Some(name) = d.pointer("/function/name").and_then(Value::as_str) {
        call.name.push_str(name);
    }
    if let Some(args) = d.pointer("/function/arguments").and_then(Value::as_str) {
        call.arguments.push_str(args);
    }
}

enum Piece {
    Text(String),
    Done,
    Skip,
}

/// One streamed event's text, or an error the server sent mid-stream.
fn piece(api: Api, v: &Value) -> Result<Piece, AiError> {
    if let Some(err) = v.get("error") {
        let message = err
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| err.as_str())
            .unwrap_or("unknown error")
            .to_owned();
        // Paparan AI names its errors (`insufficient_credits`).
        if let Some(code) = err.get("code").and_then(Value::as_str) {
            return Err(crate::paparan::error_from_code(code, message, err));
        }
        let status = err.get("code").and_then(Value::as_u64).unwrap_or(500) as u16;
        return Err(AiError::Http { status, message });
    }
    let text = |s: Option<&str>| Piece::Text(s.unwrap_or_default().to_owned());
    Ok(match api {
        Api::Anthropic => match v.get("type").and_then(Value::as_str) {
            Some("content_block_delta") => text(v.pointer("/delta/text").and_then(Value::as_str)),
            Some("message_stop") => Piece::Done,
            _ => Piece::Skip,
        },
        Api::OpenAi => text(
            v.pointer("/choices/0/delta/content")
                .and_then(Value::as_str),
        ),
        Api::Gemini => {
            let parts = v
                .pointer("/candidates/0/content/parts")
                .and_then(Value::as_array);
            let joined: String = parts
                .into_iter()
                .flatten()
                // Skip "thought" parts of thinking models.
                .filter(|p| !p.get("thought").and_then(Value::as_bool).unwrap_or(false))
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect();
            Piece::Text(joined)
        }
        Api::Ollama => {
            let t = v
                .pointer("/message/content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if v.get("done").and_then(Value::as_bool) == Some(true) && t.is_empty() {
                Piece::Done
            } else {
                Piece::Text(t)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::Mutex;

    use super::*;
    use crate::{HttpResponse, ProviderId};

    fn cfg(provider: ProviderId, key: Option<&str>) -> ProviderConfig {
        let info = provider.info();
        ProviderConfig {
            provider,
            model: if info.default_model.is_empty() {
                "m".into()
            } else {
                info.default_model.into()
            },
            base_url: if info.default_base_url.is_empty() {
                "http://127.0.0.1:1234/v1".into()
            } else {
                info.default_base_url.into()
            },
            api_key: key.map(str::to_owned),
        }
    }

    fn body(r: &HttpRequest) -> Value {
        serde_json::from_str(&r.body).unwrap()
    }

    fn header<'a>(r: &'a HttpRequest, name: &str) -> Option<&'a str> {
        r.headers
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn anthropic_request() {
        let req = ChatRequest::new("sys", "hi").max_tokens(200);
        let r = build_request(&cfg(ProviderId::Anthropic, Some("k1")), &req);
        assert_eq!(r.url, "https://api.anthropic.com/v1/messages");
        assert_eq!(header(&r, "x-api-key"), Some("k1"));
        assert_eq!(header(&r, "anthropic-version"), Some(ANTHROPIC_VERSION));
        let b = body(&r);
        assert_eq!(b["model"], "claude-haiku-5-5");
        assert_eq!(b["system"], "sys");
        assert_eq!(b["max_tokens"], 200);
        assert_eq!(b["messages"][0]["role"], "user");
        assert_eq!(b["messages"][0]["content"], "hi");
        assert_eq!(b["stream"], true);
    }

    #[test]
    fn openai_compatible_request() {
        let req = ChatRequest::new("sys", "hi").json();
        let r = build_request(&cfg(ProviderId::OpenRouter, Some("k2")), &req);
        assert_eq!(r.url, "https://openrouter.ai/api/v1/chat/completions");
        assert_eq!(header(&r, "authorization"), Some("Bearer k2"));
        let b = body(&r);
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["messages"][1]["content"], "hi");
        assert!(b.get("response_format").is_none());
        assert!(b.get("temperature").is_none());

        let r = build_request(&cfg(ProviderId::Custom, None), &req);
        assert_eq!(r.url, "http://127.0.0.1:1234/v1/chat/completions");
        assert_eq!(header(&r, "authorization"), None, "no empty bearer");
    }

    #[test]
    fn gemini_request() {
        let req = ChatRequest::new("sys", "hi").json();
        let r = build_request(&cfg(ProviderId::Gemini, Some("g")), &req);
        assert_eq!(
            r.url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
        );
        assert_eq!(header(&r, "x-goog-api-key"), Some("g"));
        assert!(!r.url.contains("key="), "key goes in a header, not the URL");
        let b = body(&r);
        assert_eq!(b["systemInstruction"]["parts"][0]["text"], "sys");
        assert_eq!(b["contents"][0]["parts"][0]["text"], "hi");
        assert_eq!(
            b["generationConfig"]["responseMimeType"],
            "application/json"
        );
    }

    #[test]
    fn ollama_request() {
        let req = ChatRequest::new("sys", "hi").json();
        let r = build_request(&cfg(ProviderId::Ollama, None), &req);
        assert_eq!(r.url, "http://localhost:11434/api/chat");
        assert!(r.headers.is_empty());
        let b = body(&r);
        assert_eq!(b["format"], "json");
        assert_eq!(b["model"], "llama3.2");
    }

    fn read(api: Api, s: &str) -> Result<String, AiError> {
        let mut pieces = Vec::new();
        let out = read_stream(
            api,
            Cursor::new(s.as_bytes().to_vec()),
            &Cancel::default(),
            &mut |t| pieces.push(t.to_owned()),
            "test",
        )?;
        assert_eq!(pieces.concat(), out);
        Ok(out)
    }

    #[test]
    fn parses_anthropic_stream() {
        let s = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{}}\n\n\
                 event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n\
                 event: ping\ndata: {\"type\":\"ping\"}\n\n\
                 event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n\
                 event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        assert_eq!(read(Api::Anthropic, s).unwrap(), "Hello");
        let err = "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n";
        assert!(matches!(
            read(Api::Anthropic, err),
            Err(AiError::Http { message, .. }) if message == "Overloaded"
        ));
    }

    #[test]
    fn parses_openai_stream() {
        let s = "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n\
                 data: {\"choices\":[{\"delta\":{\"content\":\"=SUM\"}}]}\n\n\
                 : keep-alive comment\n\n\
                 data: {\"choices\":[{\"delta\":{\"content\":\"(A1)\"}}]}\n\n\
                 data: [DONE]\n\n";
        assert_eq!(read(Api::OpenAi, s).unwrap(), "=SUM(A1)");
    }

    #[test]
    fn parses_gemini_stream() {
        let s = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"thinking\",\"thought\":true},{\"text\":\"{\\\"a\\\":\"}],\"role\":\"model\"}}]}\r\n\r\n\
                 data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"1}\"}]}}]}\r\n\r\n";
        assert_eq!(read(Api::Gemini, s).unwrap(), "{\"a\":1}");
    }

    #[test]
    fn parses_ollama_stream() {
        let s = "{\"message\":{\"role\":\"assistant\",\"content\":\"O\"},\"done\":false}\n\
                 {\"message\":{\"role\":\"assistant\",\"content\":\"K\"},\"done\":false}\n\
                 {\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done\":true}\n";
        assert_eq!(read(Api::Ollama, s).unwrap(), "OK");
        assert!(matches!(
            read(Api::Ollama, "{\"error\":\"model not found\"}\n"),
            Err(AiError::Http { .. })
        ));
    }

    #[test]
    fn empty_and_garbage_replies_are_errors() {
        assert!(matches!(
            read(Api::OpenAi, "data: [DONE]\n"),
            Err(AiError::BadReply(_))
        ));
        assert!(matches!(
            read(Api::Ollama, "<html>proxy error</html>\n"),
            Err(AiError::BadReply(_))
        ));
    }

    /// Records the request and answers with a canned response.
    struct Mock {
        status: u16,
        reply: &'static str,
        seen: Mutex<Vec<HttpRequest>>,
    }

    impl Transport for Mock {
        fn send(&self, req: &HttpRequest) -> Result<HttpResponse, AiError> {
            self.seen.lock().unwrap().push(req.clone());
            Ok(HttpResponse::new(
                self.status,
                Cursor::new(self.reply.as_bytes()),
            ))
        }
    }

    #[test]
    fn parses_tool_call_deltas() {
        let s = "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"add_slide\",\"arguments\":\"\"}}]}}]}\n\n\
                 data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"title\\\":\"}}]}}]}\n\n\
                 data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"call_2\",\"function\":{\"name\":\"notes\",\"arguments\":\"{}\"}}]}}]}\n\n\
                 data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"Hi\\\"}\"}}]}}]}\n\n\
                 data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n\
                 data: [DONE]\n\n";
        let reply = read_reply(
            Api::OpenAi,
            Cursor::new(s.as_bytes().to_vec()),
            &Cancel::default(),
            &mut |_| {},
            "test",
        )
        .unwrap();
        assert_eq!(reply.text, "");
        assert_eq!(
            reply.tool_calls,
            vec![
                ToolCall {
                    id: "call_1".into(),
                    name: "add_slide".into(),
                    arguments: "{\"title\":\"Hi\"}".into(),
                },
                ToolCall {
                    id: "call_2".into(),
                    name: "notes".into(),
                    arguments: "{}".into(),
                },
            ]
        );
        // Text-only callers still see an empty answer as an error.
        assert!(matches!(read(Api::OpenAi, s), Err(AiError::BadReply(_))));
    }

    #[test]
    fn usage_reasoning_and_finish_reason_are_read() {
        let s = "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"hmm \"}}]}\n\n\
                 data: {\"choices\":[{\"delta\":{\"reasoning\":\"ok\"}}]}\n\n\
                 data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"},\"finish_reason\":\"stop\"}]}\n\n\
                 data: {\"choices\":[],\"usage\":{\"prompt_tokens\":120,\"completion_tokens\":7,\"prompt_tokens_details\":{\"cached_tokens\":100}}}\n\n\
                 data: [DONE]\n\n";
        let reply = read_reply(
            Api::OpenAi,
            Cursor::new(s.as_bytes().to_vec()),
            &Cancel::default(),
            &mut |_| {},
            "test",
        )
        .unwrap();
        assert_eq!(reply.text, "Hi");
        assert_eq!(reply.reasoning, "hmm ok");
        assert_eq!(reply.finish_reason.as_deref(), Some("stop"));
        assert_eq!(
            reply.usage,
            Some(Usage {
                input: 120,
                cached: 100,
                output: 7
            })
        );
        // DeepSeek's own field for cache hits.
        assert_eq!(
            Usage::from_json(
                &json!({"prompt_tokens": 10, "prompt_cache_hit_tokens": 4, "completion_tokens": 1})
            ),
            Some(Usage {
                input: 10,
                cached: 4,
                output: 1
            })
        );
        // Reasoning alone isn't an empty answer.
        let only =
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"x\"}}]}\n\ndata: [DONE]\n\n";
        assert!(
            read_reply(
                Api::OpenAi,
                Cursor::new(only.as_bytes().to_vec()),
                &Cancel::default(),
                &mut |_| {},
                "test",
            )
            .is_ok()
        );
    }

    #[test]
    fn conversations_are_sent_after_the_system_prompt() {
        let req = ChatRequest::new("sys", "ignored").messages(vec![
            json!({"role": "user", "content": "make a deck"}),
            json!({"role": "assistant", "content": null, "tool_calls": []}),
        ]);
        let b = body(&build_request(&cfg(ProviderId::Custom, None), &req));
        let m = b["messages"].as_array().unwrap();
        assert_eq!(m.len(), 3);
        assert_eq!(m[0]["content"], "sys");
        assert_eq!(m[1]["content"], "make a deck");
        assert_eq!(b["stream_options"]["include_usage"], true);
        // No output cap or reasoning effort unless asked for.
        assert!(b.get("max_tokens").is_none());
        assert!(b.get("reasoning_effort").is_none());

        let mut req = req;
        req.max_output = Some(3_000);
        req.reasoning_effort = Some("low".into());
        let b = body(&build_request(&cfg(ProviderId::Custom, None), &req));
        assert_eq!(b["max_tokens"], 3_000);
        assert_eq!(b["reasoning_effort"], "low");
    }

    #[test]
    fn tools_are_sent_to_openai_compatible_apis() {
        let req = ChatRequest::new("s", "u")
            .tools(json!([{"type": "function", "function": {"name": "f"}}]));
        let r = build_request(&cfg(ProviderId::OpenAi, Some("k")), &req);
        assert_eq!(body(&r)["tools"][0]["function"]["name"], "f");
    }

    #[test]
    fn complete_through_a_mock_transport() {
        let mock = Arc::new(Mock {
            status: 200,
            reply: "{\"message\":{\"content\":\"OK\"},\"done\":true}\n",
            seen: Mutex::default(),
        });
        let client = Client::new(cfg(ProviderId::Ollama, None), mock.clone());
        assert_eq!(client.test_connection(&Cancel::default()).unwrap(), "OK");
        assert_eq!(mock.seen.lock().unwrap().len(), 1);

        let failing = Arc::new(Mock {
            status: 401,
            reply: r#"{"error":{"message":"Incorrect API key provided"}}"#,
            seen: Mutex::default(),
        });
        let client = Client::new(cfg(ProviderId::OpenAi, Some("bad")), failing);
        let err = client.test_connection(&Cancel::default()).unwrap_err();
        assert_eq!(
            err,
            AiError::Http {
                status: 401,
                message: "Incorrect API key provided".into()
            }
        );
    }

    #[test]
    fn cancelled_before_sending() {
        let mock = Arc::new(Mock {
            status: 200,
            reply: "",
            seen: Mutex::default(),
        });
        let client = Client::new(cfg(ProviderId::Ollama, None), mock.clone());
        let cancel = Cancel::default();
        cancel.cancel();
        assert_eq!(client.test_connection(&cancel), Err(AiError::Cancelled));
        assert!(mock.seen.lock().unwrap().is_empty());
    }
}
