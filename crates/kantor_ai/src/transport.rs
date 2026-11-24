//! The HTTP layer behind a trait, so request building and stream parsing are
//! tested without a network ([`Transport`] mocks).

use std::fmt;
use std::io::Read;
use std::time::Duration;

use crate::AiError;

/// The HTTP method of an [`HttpRequest`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Method {
    /// No body is sent.
    Get,
    /// The body is sent as JSON.
    #[default]
    Post,
}

/// A JSON POST (or a GET). Header values may hold the API key: the `Debug`
/// output redacts them.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: String,
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(n, _)| *n).collect();
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &redact_url(&self.url))
            .field("headers", &names)
            .field("body_len", &self.body.len())
            .finish()
    }
}

/// Drop the query string (Gemini can carry `key=` there).
fn redact_url(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

impl HttpRequest {
    /// A JSON POST of `body`.
    pub fn post(url: String, headers: Vec<(&'static str, String)>, body: String) -> Self {
        Self {
            method: Method::Post,
            url,
            headers,
            body,
        }
    }

    /// A GET (no body).
    pub fn get(url: String, headers: Vec<(&'static str, String)>) -> Self {
        Self {
            method: Method::Get,
            url,
            headers,
            body: String::new(),
        }
    }
}

pub struct HttpResponse {
    pub status: u16,
    /// Response headers, names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Box<dyn Read + Send>,
}

impl HttpResponse {
    /// A response with no headers (tests, mocks).
    pub fn new(status: u16, body: impl Read + Send + 'static) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Box::new(body),
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers
            .push((name.to_ascii_lowercase(), value.to_owned()));
        self
    }

    /// The first header called `name` (any case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The body as text, at most 64 KiB (error replies).
    pub fn read_text(self) -> String {
        let mut text = String::new();
        let _ = self.body.take(64 * 1024).read_to_string(&mut text);
        text
    }
}

pub trait Transport: Send + Sync {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, AiError>;
}

/// The real transport: `ureq` with rustls, blocking (run it off the UI thread).
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl Default for UreqTransport {
    fn default() -> Self {
        Self::new(Duration::from_secs(120))
    }
}

impl UreqTransport {
    /// `read_timeout` bounds each wait for data (first token included);
    /// local models loading into memory can take a while.
    pub fn new(read_timeout: Duration) -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_per_read(Some(read_timeout))
            .timeout_global(Some(Duration::from_secs(300)))
            .user_agent(concat!("Kantor/", env!("CARGO_PKG_VERSION")))
            .build();
        Self {
            agent: config.into(),
        }
    }
}

impl Transport for UreqTransport {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, AiError> {
        let resp = match req.method {
            Method::Post => {
                let mut builder = self.agent.post(&req.url);
                for (name, value) in &req.headers {
                    builder = builder.header(*name, value);
                }
                builder
                    .header("content-type", "application/json")
                    .send(req.body.as_str())
            }
            Method::Get => {
                let mut builder = self.agent.get(&req.url);
                for (name, value) in &req.headers {
                    builder = builder.header(*name, value);
                }
                builder.call()
            }
        }
        .map_err(|e| map_ureq_error(e, &req.url))?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .filter_map(|(n, v)| {
                Some((n.as_str().to_ascii_lowercase(), v.to_str().ok()?.to_owned()))
            })
            .collect();
        let body = resp.into_body().into_reader();
        Ok(HttpResponse {
            status,
            headers,
            body: Box::new(body),
        })
    }
}

fn map_ureq_error(e: ureq::Error, url: &str) -> AiError {
    let host = redact_url(url);
    match e {
        ureq::Error::Timeout(_) => AiError::Timeout,
        ureq::Error::HostNotFound => AiError::Network(format!("host not found ({host})")),
        ureq::Error::ConnectionFailed => AiError::Network(format!("connection failed ({host})")),
        ureq::Error::Io(io) => map_io_error(&io, host),
        other => AiError::Network(other.to_string()),
    }
}

pub(crate) fn map_io_error(io: &std::io::Error, host: &str) -> AiError {
    use std::io::ErrorKind;
    match io.kind() {
        ErrorKind::TimedOut | ErrorKind::WouldBlock => AiError::Timeout,
        ErrorKind::ConnectionRefused => AiError::Network(format!(
            "nothing is listening at {host}; is the server running?"
        )),
        _ => AiError::Network(io.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_hides_keys() {
        let r = HttpRequest::post(
            "https://x.example/v1?key=secret".into(),
            vec![("authorization", "Bearer sk-secret".into())],
            "{}".into(),
        );
        let s = format!("{r:?}");
        assert!(!s.contains("secret"), "{s}");
        assert!(s.contains("authorization"));
    }

    #[test]
    fn response_headers_any_case() {
        let r = HttpResponse::new(200, std::io::empty())
            .with_header("X-Paparan-Credits-Remaining", "12");
        assert_eq!(r.header("x-paparan-credits-remaining"), Some("12"));
        assert_eq!(r.header("X-PAPARAN-CREDITS-REMAINING"), Some("12"));
        assert_eq!(r.header("retry-after"), None);
    }
}
