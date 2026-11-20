//! HTTP for providers: blocking ureq with rustls (run it off the UI thread),
//! behind a small trait so tests can record what was sent. Header values
//! and bodies can hold credentials and documents: `Debug` shows neither.

use std::fmt;
use std::io::Read as _;
use std::time::Duration;

use crate::Error;

/// Largest response read (stamped PDFs come back whole).
const MAX_BODY: u64 = 200 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
        }
    }
}

/// One request.
#[derive(Clone)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(n, _)| n.as_str()).collect();
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("url", &self.url.split('?').next().unwrap_or(""))
            .field("headers", &names)
            .field("body_len", &self.body.len())
            .finish()
    }
}

impl Request {
    pub fn new(method: Method, url: impl Into<String>) -> Request {
        Request {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn get(url: impl Into<String>) -> Request {
        Request::new(Method::Get, url)
    }

    pub fn post(url: impl Into<String>) -> Request {
        Request::new(Method::Post, url)
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Request {
        self.headers.push((name.to_string(), value.into()));
        self
    }

    pub fn bearer(self, token: &str) -> Request {
        self.header("Authorization", format!("Bearer {token}"))
    }

    pub fn json(mut self, value: &serde_json::Value) -> Request {
        self.body = serde_json::to_vec(value).unwrap_or_default();
        self.header("Content-Type", "application/json")
    }

    pub fn form(mut self, fields: &[(&str, &str)]) -> Request {
        let enc: Vec<String> = fields
            .iter()
            .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
            .collect();
        self.body = enc.join("&").into_bytes();
        self.header("Content-Type", "application/x-www-form-urlencoded")
    }

    pub fn multipart(mut self, m: Multipart) -> Request {
        let (ct, body) = m.finish();
        self.body = body;
        self.header("Content-Type", ct)
    }
}

/// A response, read whole.
#[derive(Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .field("body_len", &self.body.len())
            .finish()
    }
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The body as JSON.
    pub fn json(&self) -> Result<serde_json::Value, Error> {
        serde_json::from_slice(&self.body).map_err(|_| {
            Error::Provider(format!(
                "the provider's answer isn't JSON (HTTP {}): {}",
                self.status,
                snippet(&self.body)
            ))
        })
    }

    /// A non-2xx answer as an error with the provider's message.
    pub fn check(self) -> Result<Response, Error> {
        if self.ok() {
            return Ok(self);
        }
        let msg = serde_json::from_slice::<serde_json::Value>(&self.body)
            .ok()
            .and_then(|v| crate::json::message(&v))
            .unwrap_or_else(|| snippet(&self.body));
        Err(match self.status {
            401 | 403 => Error::Auth(msg),
            402 => Error::Quota(msg),
            _ => Error::Provider(format!("HTTP {}: {msg}", self.status)),
        })
    }
}

/// The first 200 characters of a body, for messages.
pub fn snippet(body: &[u8]) -> String {
    let s = String::from_utf8_lossy(body);
    let s = s.trim();
    let mut out: String = s.chars().take(200).collect();
    if s.chars().count() > 200 {
        out.push('…');
    }
    out
}

/// Sends requests.
pub trait Transport: Send + Sync {
    fn send(&self, req: &Request) -> Result<Response, Error>;
}

/// The real transport: ureq with rustls, Mozilla's and the system's roots.
pub struct Ureq {
    agent: ureq::Agent,
}

impl Default for Ureq {
    fn default() -> Self {
        Self::new()
    }
}

impl Ureq {
    pub fn new() -> Ureq {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_per_read(Some(Duration::from_secs(120)))
            .timeout_global(Some(Duration::from_secs(600)))
            .user_agent(concat!("Kantor/", env!("CARGO_PKG_VERSION")))
            .build();
        Ureq {
            agent: config.into(),
        }
    }
}

impl Transport for Ureq {
    fn send(&self, req: &Request) -> Result<Response, Error> {
        let host = req.url.split('?').next().unwrap_or("").to_string();
        let map = |e: ureq::Error| match e {
            ureq::Error::Timeout(_) => Error::Network(format!("{host} didn't answer in time")),
            ureq::Error::HostNotFound => Error::Network(format!("host not found ({host})")),
            ureq::Error::ConnectionFailed => Error::Network(format!("connection failed ({host})")),
            other => Error::Network(other.to_string()),
        };
        let resp = match req.method {
            Method::Get => {
                let mut b = self.agent.get(&req.url);
                for (n, v) in &req.headers {
                    b = b.header(n.as_str(), v.as_str());
                }
                b.call().map_err(map)?
            }
            Method::Post | Method::Put => {
                let mut b = if req.method == Method::Post {
                    self.agent.post(&req.url)
                } else {
                    self.agent.put(&req.url)
                };
                for (n, v) in &req.headers {
                    b = b.header(n.as_str(), v.as_str());
                }
                b.send(&req.body[..]).map_err(map)?
            }
        };
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .filter_map(|(n, v)| Some((n.as_str().to_string(), v.to_str().ok()?.to_string())))
            .collect();
        let mut body = Vec::new();
        resp.into_body()
            .into_reader()
            .take(MAX_BODY)
            .read_to_end(&mut body)
            .map_err(|e| Error::Network(e.to_string()))?;
        Ok(Response {
            status,
            headers,
            body,
        })
    }
}

/// A `multipart/form-data` body.
pub struct Multipart {
    boundary: String,
    body: Vec<u8>,
}

impl Default for Multipart {
    fn default() -> Self {
        Self::new()
    }
}

impl Multipart {
    pub fn new() -> Multipart {
        let r = ring::rand::SystemRandom::new();
        let mut b = [0u8; 12];
        let _ = ring::rand::SecureRandom::fill(&r, &mut b);
        let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
        Multipart {
            boundary: format!("----KantorBoundary{hex}"),
            body: Vec::new(),
        }
    }

    pub fn text(mut self, name: &str, value: &str) -> Multipart {
        self.body.extend_from_slice(
            format!(
                "--{}\r\nContent-Disposition: form-data; name=\"{}\"\r\n\r\n",
                self.boundary,
                quote(name)
            )
            .as_bytes(),
        );
        self.body.extend_from_slice(value.as_bytes());
        self.body.extend_from_slice(b"\r\n");
        self
    }

    pub fn file(mut self, name: &str, file_name: &str, content_type: &str, data: &[u8]) -> Self {
        self.body.extend_from_slice(
            format!(
                "--{}\r\nContent-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
                self.boundary,
                quote(name),
                quote(file_name),
                content_type
            )
            .as_bytes(),
        );
        self.body.extend_from_slice(data);
        self.body.extend_from_slice(b"\r\n");
        self
    }

    fn finish(mut self) -> (String, Vec<u8>) {
        self.body
            .extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        (
            format!("multipart/form-data; boundary={}", self.boundary),
            self.body,
        )
    }
}

fn quote(s: &str) -> String {
    s.replace(['"', '\r', '\n'], "_")
}

pub fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Parse a `multipart/form-data` body (the mock server and tests).
pub fn parse_multipart(content_type: &str, body: &[u8]) -> Vec<Part> {
    let Some(boundary) = content_type
        .split(';')
        .find_map(|p| p.trim().strip_prefix("boundary="))
    else {
        return Vec::new();
    };
    let delim = format!("--{boundary}");
    let mut parts = Vec::new();
    let mut rest = body;
    while let Some(start) = find(rest, delim.as_bytes()) {
        rest = &rest[start + delim.len()..];
        if rest.starts_with(b"--") {
            break;
        }
        rest = rest.strip_prefix(b"\r\n").unwrap_or(rest);
        let Some(end) = find(rest, delim.as_bytes()) else {
            break;
        };
        let chunk = &rest[..end];
        let chunk = chunk.strip_suffix(b"\r\n").unwrap_or(chunk);
        if let Some(h) = find(chunk, b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&chunk[..h]).to_string();
            let data = chunk[h + 4..].to_vec();
            let attr = |key: &str| {
                let pat = format!("{key}=\"");
                head.find(&pat).and_then(|i| {
                    let s = &head[i + pat.len()..];
                    s.find('"').map(|j| s[..j].to_string())
                })
            };
            parts.push(Part {
                name: attr(" name").or_else(|| attr(";name")).unwrap_or_default(),
                file_name: attr("filename"),
                data,
            });
        }
        rest = &rest[end..];
    }
    parts
}

/// One part of a multipart body.
#[derive(Clone, Debug)]
pub struct Part {
    pub name: String,
    pub file_name: Option<String>,
    pub data: Vec<u8>,
}

pub(crate) fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_round_trip() {
        let req = Request::post("http://x/upload").multipart(
            Multipart::new().text("namadoc", "4b").file(
                "file",
                "a.pdf",
                "application/pdf",
                b"%PDF-1.7\r\n\r\nbody",
            ),
        );
        let ct = req
            .headers
            .iter()
            .find(|(n, _)| n == "Content-Type")
            .unwrap()
            .1
            .clone();
        let parts = parse_multipart(&ct, &req.body);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].name, "namadoc");
        assert_eq!(parts[0].data, b"4b");
        assert_eq!(parts[1].file_name.as_deref(), Some("a.pdf"));
        assert_eq!(parts[1].data, b"%PDF-1.7\r\n\r\nbody");
    }

    #[test]
    fn debug_hides_secrets() {
        let r = Request::post("https://x.example/token?secret=1")
            .bearer("abc-secret")
            .json(&serde_json::json!({"password": "pw-secret"}));
        let s = format!("{r:?}");
        assert!(!s.contains("secret"), "{s}");
    }

    #[test]
    fn url_encoding() {
        assert_eq!(url_encode("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
    }
}
