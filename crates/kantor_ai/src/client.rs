//! One chat completion, streamed, against any [`Api`].
//!
//! Every provider streams: Anthropic, OpenAI and Gemini as server-sent
//! events, Ollama as one JSON object per line. [`Client::complete`] calls
//! `on_text` with each piece and returns the whole text.

use std::io::{BufRead, BufReader, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};

use crate::AiError;
use crate::error::{error_message, truncate};
use crate::provider::{Api, ProviderConfig};
use crate::transport::{HttpRequest, Transport, map_io_error};

/// What to ask. `json` asks for a JSON object reply (natively where the API
/// supports it; the prompt always says so too).
#[derive(Clone, Debug, PartialEq)]
pub struct ChatRequest {
    pub system: String,
    pub user: String,
    pub json: bool,
    pub max_tokens: u32,
}

impl ChatRequest {
    pub fn new(system: impl Into<String>, user: impl Into<String>) -> Self {
        Self {
            system: system.into(),
            user: user.into(),
            json: false,
            max_tokens: 1024,
        }
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
        let resp = self.transport.post(&http)?;
        if cancel.is_cancelled() {
            return Err(AiError::Cancelled);
        }
        if !(200..300).contains(&resp.status) {
            let mut body = String::new();
            let _ = resp.body.take(64 * 1024).read_to_string(&mut body);
            return Err(AiError::Http {
                status: resp.status,
                message: error_message(&body),
            });
        }
        let host = http.url.split('?').next().unwrap_or_default().to_owned();
        read_stream(
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
            HttpRequest {
                url: format!("{base}/v1/messages"),
                headers: vec![
                    ("x-api-key", key),
                    ("anthropic-version", ANTHROPIC_VERSION.to_owned()),
                ],
                body: body.to_string(),
            }
        }
        Api::OpenAi => {
            // No temperature / max_tokens: newer OpenAI models reject them,
            // and `response_format` isn't supported by every compatible
            // server, so JSON is asked for in the prompt only.
            let body = json!({
                "model": cfg.model,
                "messages": [
                    {"role": "system", "content": req.system},
                    {"role": "user", "content": req.user},
                ],
                "stream": true,
            });
            let mut headers = Vec::new();
            if !key.is_empty() {
                headers.push(("authorization", format!("Bearer {key}")));
            }
            if cfg.provider == crate::ProviderId::OpenRouter {
                headers.push(("x-title", "Kantor".to_owned()));
            }
            HttpRequest {
                url: format!("{base}/chat/completions"),
                headers,
                body: body.to_string(),
            }
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
            HttpRequest {
                url: format!(
                    "{base}/v1beta/models/{}:streamGenerateContent?alt=sse",
                    cfg.model
                ),
                headers: vec![("x-goog-api-key", key)],
                body: body.to_string(),
            }
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
            HttpRequest {
                url: format!("{base}/api/chat"),
                headers: Vec::new(),
                body: body.to_string(),
            }
        }
    }
}

/// Read a streamed reply to the end, calling `on_text` per piece.
pub fn read_stream(
    api: Api,
    reader: impl BufRead,
    cancel: &Cancel,
    on_text: &mut dyn FnMut(&str),
    host: &str,
) -> Result<String, AiError> {
    let mut out = String::new();
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
                None => continue,
            }
        };
        if payload == "[DONE]" {
            break;
        }
        let value: Value = serde_json::from_str(payload).map_err(|_| {
            AiError::BadReply(format!("unexpected data: {}", truncate(payload, 120)))
        })?;
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
    if out.trim().is_empty() {
        return Err(AiError::BadReply("the model sent an empty answer".into()));
    }
    Ok(out)
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
        fn post(&self, req: &HttpRequest) -> Result<HttpResponse, AiError> {
            self.seen.lock().unwrap().push(req.clone());
            Ok(HttpResponse {
                status: self.status,
                body: Box::new(Cursor::new(self.reply.as_bytes())),
            })
        }
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
