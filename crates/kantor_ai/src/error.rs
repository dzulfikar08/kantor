//! Errors shown to the user, worded so they say what to do next.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiError {
    /// AI features are off, or no provider is chosen.
    Disabled,
    /// The provider needs an API key and none is stored.
    MissingKey {
        provider: &'static str,
    },
    /// A required setting (base URL, model) is empty.
    MissingSetting(&'static str),
    /// The server answered with an error status.
    Http {
        status: u16,
        message: String,
    },
    /// The server couldn't be reached.
    Network(String),
    Timeout,
    Cancelled,
    /// The reply wasn't what was asked for.
    BadReply(String),
    /// The model answered but declined (e.g. "can't be done with a formula").
    NoAnswer(String),
    /// The request is too large to send (selection too long).
    TooLarge(String),
}

impl fmt::Display for AiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => write!(
                f,
                "AI features are off. Turn them on and choose a provider in AI Settings."
            ),
            Self::MissingKey { provider } => {
                write!(f, "No API key for {provider}. Add one in AI Settings.")
            }
            Self::MissingSetting(what) => write!(f, "Set the {what} in AI Settings."),
            Self::Http { status, message } => {
                let hint = match status {
                    401 | 403 => "The API key was rejected; check it in AI Settings.",
                    404 => "Model or address not found; check the model name and base URL.",
                    429 => "Rate limited or out of credit; try again later.",
                    500..=599 => "The provider had a problem; try again later.",
                    _ => "The provider refused the request.",
                };
                if message.is_empty() {
                    write!(f, "{hint} (HTTP {status})")
                } else {
                    write!(f, "{hint} (HTTP {status}: {message})")
                }
            }
            Self::Network(msg) => write!(f, "Can't reach the provider: {msg}"),
            Self::Timeout => write!(f, "The provider took too long to answer."),
            Self::Cancelled => write!(f, "Cancelled."),
            Self::BadReply(msg) => write!(f, "The model's answer couldn't be used: {msg}"),
            Self::NoAnswer(msg) => write!(f, "{msg}"),
            Self::TooLarge(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for AiError {}

/// The most useful message in an error body: `error.message` (Anthropic,
/// OpenAI, Gemini), `error` as a string (Ollama) or `message`; else the
/// start of the body.
pub fn error_message(body: &str) -> String {
    let json: Option<serde_json::Value> = serde_json::from_str(body.trim()).ok();
    let found = json.as_ref().and_then(|v| {
        // Gemini wraps errors in a one-element array when streaming.
        let v = v.as_array().and_then(|a| a.first()).unwrap_or(v);
        v.pointer("/error/message")
            .or_else(|| v.get("error").filter(|e| e.is_string()))
            .or_else(|| v.get("message"))
            .and_then(|m| m.as_str())
            .map(str::to_owned)
    });
    let msg = found.unwrap_or_else(|| body.trim().to_owned());
    truncate(&msg, 300)
}

pub(crate) fn truncate(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_from_each_provider() {
        let anthropic = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
        assert_eq!(error_message(anthropic), "invalid x-api-key");
        let ollama = r#"{"error":"model \"llama9\" not found, try pulling it first"}"#;
        assert_eq!(
            error_message(ollama),
            "model \"llama9\" not found, try pulling it first"
        );
        let gemini = r#"[{"error":{"code":400,"message":"API key not valid."}}]"#;
        assert_eq!(error_message(gemini), "API key not valid.");
        assert_eq!(error_message("Bad Gateway"), "Bad Gateway");
    }

    #[test]
    fn http_errors_say_what_to_do() {
        let e = AiError::Http {
            status: 401,
            message: "invalid key".into(),
        };
        assert!(e.to_string().contains("API key was rejected"));
        assert!(e.to_string().contains("invalid key"));
    }
}
