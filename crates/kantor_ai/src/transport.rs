//! The HTTP layer behind a trait, so request building and stream parsing are
//! tested without a network ([`Transport`] mocks).

use std::fmt;
use std::io::Read;
use std::time::Duration;

use crate::AiError;

/// A JSON POST. Header values may hold the API key: the `Debug` output
/// redacts them.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: String,
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(n, _)| *n).collect();
        f.debug_struct("HttpRequest")
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

pub struct HttpResponse {
    pub status: u16,
    pub body: Box<dyn Read + Send>,
}

pub trait Transport: Send + Sync {
    fn post(&self, req: &HttpRequest) -> Result<HttpResponse, AiError>;
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
    fn post(&self, req: &HttpRequest) -> Result<HttpResponse, AiError> {
        let mut builder = self.agent.post(&req.url);
        for (name, value) in &req.headers {
            builder = builder.header(*name, value);
        }
        let resp = builder
            .header("content-type", "application/json")
            .send(req.body.as_str())
            .map_err(|e| map_ureq_error(e, &req.url))?;
        let status = resp.status().as_u16();
        let body = resp.into_body().into_reader();
        Ok(HttpResponse {
            status,
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
        let r = HttpRequest {
            url: "https://x.example/v1?key=secret".into(),
            headers: vec![("authorization", "Bearer sk-secret".into())],
            body: "{}".into(),
        };
        let s = format!("{r:?}");
        assert!(!s.contains("secret"), "{s}");
        assert!(s.contains("authorization"));
    }
}
