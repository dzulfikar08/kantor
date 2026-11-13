//! A small blocking WebDAV client for opening and saving files on a
//! self-hosted drive (Nextcloud, OpenCloud/OCIS, or any RFC 4918 server).
//!
//! Only plain WebDAV is spoken so the server stays swappable: `PROPFIND`
//! (depth 0/1), `GET`, `PUT` with `If-Match` on the ETag so a save can't
//! silently overwrite someone else's, `LOCK`/`UNLOCK`, `MKCOL`, `DELETE`.
//! It's blocking (same ureq + rustls stack as `kantor_ai` and
//! `kantor_update`): call it off the UI thread.

pub mod cache;
pub mod session;
pub mod sync;
mod xml;

use std::io::Read;
use std::time::Duration;

use base64::Engine as _;
use ureq::http::{self, Method};

pub use xml::{parse_lock_owner, parse_lock_token, parse_multistatus};

/// How long [`Client::get`] keeps retrying `425 Too Early`.
pub const TOO_EARLY_WAIT: Duration = Duration::from_secs(15);

/// How the client signs in.
#[derive(Clone)]
pub enum Auth {
    None,
    /// HTTP Basic: a user name with a password or, better, an app password
    /// (Nextcloud "Devices & sessions", OpenCloud "App tokens").
    Basic {
        user: String,
        password: String,
    },
    /// An OAuth/OIDC access token.
    Bearer(String),
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Auth::None => f.write_str("None"),
            Auth::Basic { user, .. } => write!(f, "Basic({user}, ***)"),
            Auth::Bearer(_) => f.write_str("Bearer(***)"),
        }
    }
}

impl Auth {
    fn header(&self) -> Option<String> {
        match self {
            Auth::None => None,
            Auth::Basic { user, password } => Some(format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))
            )),
            Auth::Bearer(t) => Some(format!("Bearer {t}")),
        }
    }
}

#[derive(Debug)]
pub enum Error {
    /// Couldn't reach the server, or the connection broke.
    Network(String),
    /// 401/403: wrong credentials or no permission.
    Auth(u16),
    NotFound,
    /// 412: the file changed on the server since it was opened (ETag
    /// mismatch), or `If-None-Match: *` found an existing file.
    Conflict,
    /// 423: someone else holds a lock.
    Locked,
    /// 425: the server is still processing the last upload of this file.
    TooEarly,
    /// Any other unexpected status.
    Http {
        status: u16,
        body: String,
    },
    /// The server replied with something that isn't valid WebDAV.
    Protocol(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Network(m) => write!(f, "network error: {m}"),
            Error::Auth(s) => write!(f, "not authorised (HTTP {s})"),
            Error::NotFound => f.write_str("not found"),
            Error::Conflict => f.write_str("the file was changed on the server (ETag mismatch)"),
            Error::Locked => f.write_str("the file is locked by someone else"),
            Error::TooEarly => f.write_str("the server is still processing the last upload"),
            Error::Http { status, body } => {
                write!(f, "HTTP {status}")?;
                if !body.is_empty() {
                    write!(f, ": {}", body.chars().take(200).collect::<String>())?;
                }
                Ok(())
            }
            Error::Protocol(m) => write!(f, "bad server reply: {m}"),
        }
    }
}

impl std::error::Error for Error {}

/// One item of a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// As the server sent it (percent-encoded, usually an absolute path).
    pub href: String,
    /// Decoded last segment.
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    /// Quoted, as sent (`"abc"`); pass it back unchanged to [`Client::put`].
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_type: Option<String>,
}

/// A downloaded file.
#[derive(Debug)]
pub struct Fetched {
    pub bytes: Vec<u8>,
    pub etag: Option<String>,
}

/// When a `PUT` may go through.
#[derive(Debug, Clone, Copy)]
pub enum Precondition<'a> {
    /// Only if the server's ETag still equals this one (a normal save).
    IfMatch(&'a str),
    /// Only if nothing exists there yet (Save As, upload of a new file).
    IfNoneMatch,
    /// Unconditionally (the user chose "overwrite theirs").
    Overwrite,
}

/// A held `LOCK`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockToken(pub String);

pub struct Client {
    agent: ureq::Agent,
    /// Ends with `/`, e.g. `https://drive.example/remote.php/dav/files/alice/`.
    base: String,
    auth: Option<String>,
}

/// Slowest link a transfer is planned for (128 kbit/s): after a PUT body is
/// written, the reply may only come once the data still queued in socket
/// buffers has crossed the link, so the wait allowed for it grows with the
/// upload's size at this rate.
pub const SLOWEST_LINK_BYTES_PER_SEC: u64 = 16 * 1024;

/// A read or write that makes no progress for this long fails the request.
/// Transfers have no fixed overall timeout: a 20 MB save on 500 kbit/s takes
/// about 6 minutes and is fine as long as bytes keep moving.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(120);

/// Upload progress: `(bytes sent, total)`.
pub type Progress<'a> = &'a mut dyn FnMut(u64, u64);

impl Client {
    /// `base` is the user's DAV root; paths given to the methods are relative
    /// to it, `/`-separated and unencoded (`Laporan/Q3 2026.xlsx`).
    pub fn new(base: &str, auth: Auth) -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .allow_non_standard_methods(true)
            // A redirected PUT must not be replayed somewhere else silently.
            .max_redirects(0)
            .timeout_connect(Some(Duration::from_secs(15)))
            // Progress-based: each read and write must move bytes within
            // STALL_TIMEOUT. PUT replies get a longer, size-based wait
            // (see `put_with_progress`).
            .timeout_per_read(Some(STALL_TIMEOUT))
            .timeout_per_write(Some(STALL_TIMEOUT))
            .user_agent(concat!("Kantor/", env!("CARGO_PKG_VERSION"), " (WebDAV)"))
            .build();
        let mut base = base.to_string();
        if !base.ends_with('/') {
            base.push('/');
        }
        Self {
            agent: config.into(),
            base,
            auth: auth.header(),
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// The URL of `path` under the base, each segment percent-encoded.
    pub fn url(&self, path: &str) -> String {
        let mut url = self.base.clone();
        let segs: Vec<String> = path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(percent_encode)
            .collect();
        url.push_str(&segs.join("/"));
        if path.ends_with('/') && !segs.is_empty() {
            url.push('/');
        }
        url
    }

    fn request(&self, method: &str, path: &str) -> http::request::Builder {
        let mut b = http::Request::builder()
            .method(Method::from_bytes(method.as_bytes()).expect("valid method"))
            .uri(self.url(path));
        if let Some(a) = &self.auth {
            b = b.header("authorization", a);
        }
        b
    }

    fn send(&self, req: http::request::Builder, body: &[u8]) -> Result<Reply, Error> {
        let req = req
            .body(body)
            .map_err(|e| Error::Protocol(format!("request: {e}")))?;
        let resp = self
            .agent
            .run(req)
            .map_err(|e| Error::Network(e.to_string()))?;
        Reply::read(resp)
    }

    /// The children of a folder (depth 1), without the folder itself.
    pub fn list(&self, dir: &str) -> Result<Vec<Entry>, Error> {
        let mut dir = dir.trim_end_matches('/').to_string();
        dir.push('/');
        let mut all = self.propfind(&dir, 1)?;
        // The first response is the folder itself.
        if !all.is_empty() {
            all.remove(0);
        }
        Ok(all)
    }

    /// One file's or folder's properties (depth 0).
    pub fn stat(&self, path: &str) -> Result<Entry, Error> {
        self.propfind(path, 0)?
            .into_iter()
            .next()
            .ok_or_else(|| Error::Protocol("empty PROPFIND reply".into()))
    }

    fn propfind(&self, path: &str, depth: u8) -> Result<Vec<Entry>, Error> {
        const BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/><d:getcontentlength/><d:getetag/><d:getlastmodified/><d:getcontenttype/></d:prop></d:propfind>"#;
        let text = self.propfind_raw(path, depth, BODY)?;
        parse_multistatus(&text)
    }

    /// A PROPFIND reply as text. Asks for gzip: Nextcloud shrinks a
    /// 500-entry listing 15× (208 KB to 14 KB), which matters on slow links;
    /// servers that don't compress just send it plain.
    fn propfind_raw(&self, path: &str, depth: u8, body: &str) -> Result<String, Error> {
        let req = self
            .request("PROPFIND", path)
            .header("depth", depth.to_string())
            .header("accept-encoding", "gzip")
            .header("content-type", "application/xml; charset=utf-8");
        let r = self.send(req, body.as_bytes())?;
        if r.status != 207 {
            return Err(r.error());
        }
        String::from_utf8(r.decoded()?)
            .map_err(|_| Error::Protocol("PROPFIND reply isn't UTF-8".into()))
    }

    /// Download a file with its current ETag.
    ///
    /// OpenCloud/OCIS post-process every upload asynchronously and answer
    /// `425 Too Early` until that's done (measured: right after a 2 MB PUT),
    /// so a 425 is retried with backoff for up to [`TOO_EARLY_WAIT`].
    pub fn get(&self, path: &str) -> Result<Fetched, Error> {
        let start = std::time::Instant::now();
        let mut pause = Duration::from_millis(50);
        loop {
            match self.try_get(path) {
                Err(Error::TooEarly) if start.elapsed() + pause < TOO_EARLY_WAIT => {
                    std::thread::sleep(pause);
                    pause = (pause * 2).min(Duration::from_secs(1));
                }
                other => return other,
            }
        }
    }

    /// One `GET`, without retrying `425 Too Early`.
    pub fn try_get(&self, path: &str) -> Result<Fetched, Error> {
        let r = self.send(self.request("GET", path), &[])?;
        if r.status != 200 {
            return Err(r.error());
        }
        Ok(Fetched {
            bytes: r.bytes,
            etag: r.etag,
        })
    }

    /// Upload `bytes` to `path`. Returns the new ETag when the server sends
    /// one (Nextcloud and OCIS do); otherwise `None`, and the caller can
    /// [`stat`](Self::stat). A `lock` you hold is sent in the `If` header.
    pub fn put(
        &self,
        path: &str,
        bytes: &[u8],
        cond: Precondition,
        lock: Option<&LockToken>,
    ) -> Result<Option<String>, Error> {
        self.put_with_progress(path, bytes, cond, lock, &mut |_, _| {})
    }

    /// [`put`](Self::put), reporting bytes handed to the connection.
    ///
    /// Timeouts follow progress, not the clock: every write must move bytes
    /// within [`STALL_TIMEOUT`], and the reply may take `STALL_TIMEOUT` plus
    /// the time the whole body needs at [`SLOWEST_LINK_BYTES_PER_SEC`]
    /// (data still queued in buffers when the last write returns).
    pub fn put_with_progress(
        &self,
        path: &str,
        bytes: &[u8],
        cond: Precondition,
        lock: Option<&LockToken>,
        progress: Progress,
    ) -> Result<Option<String>, Error> {
        let mut req = self
            .request("PUT", path)
            .header("content-type", "application/octet-stream")
            .header("content-length", bytes.len().to_string());
        match cond {
            Precondition::IfMatch(etag) => req = req.header("if-match", etag),
            Precondition::IfNoneMatch => req = req.header("if-none-match", "*"),
            Precondition::Overwrite => {}
        }
        if let Some(LockToken(t)) = lock {
            req = req.header("if", format!("(<{t}>)"));
        }
        let total = bytes.len() as u64;
        let mut reader = ProgressReader {
            data: bytes,
            pos: 0,
            progress,
        };
        let body = ureq::SendBody::from_reader(&mut reader);
        let req = req
            .body(body)
            .map_err(|e| Error::Protocol(format!("request: {e}")))?;
        let reply_wait = reply_wait(total);
        let req = self
            .agent
            .configure_request(req)
            .timeout_per_read(Some(reply_wait))
            .timeout_recv_response(Some(reply_wait))
            .build();
        let resp = self
            .agent
            .run(req)
            .map_err(|e| Error::Network(e.to_string()))?;
        let r = Reply::read(resp)?;
        match r.status {
            200 | 201 | 204 => Ok(r.etag),
            _ => Err(r.error()),
        }
    }

    /// Take an exclusive write lock for `timeout` (servers may shorten it).
    /// `owner` is shown to others who find the file locked (servers that
    /// know the account show its name instead).
    pub fn lock(&self, path: &str, timeout: Duration, owner: &str) -> Result<LockToken, Error> {
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:lockinfo xmlns:d="DAV:"><d:lockscope><d:exclusive/></d:lockscope><d:locktype><d:write/></d:locktype><d:owner>{}</d:owner></d:lockinfo>"#,
            xml_escape(owner)
        );
        let req = self
            .request("LOCK", path)
            .header("depth", "0")
            .header("timeout", format!("Second-{}", timeout.as_secs()))
            .header("content-type", "application/xml; charset=utf-8");
        let r = self.send(req, body.as_bytes())?;
        if !matches!(r.status, 200 | 201) {
            return Err(r.error());
        }
        let from_header = r.lock_token.as_deref().map(|t| {
            t.trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .to_string()
        });
        from_header
            .filter(|t| !t.is_empty())
            .or_else(|| parse_lock_token(&String::from_utf8_lossy(&r.bytes)))
            .map(LockToken)
            .ok_or_else(|| Error::Protocol("LOCK reply has no lock token".into()))
    }

    /// Extend a lock we hold (RFC 4918 §9.10.2: LOCK with no body and the
    /// token in `If`).
    pub fn refresh_lock(
        &self,
        path: &str,
        token: &LockToken,
        timeout: Duration,
    ) -> Result<(), Error> {
        let req = self
            .request("LOCK", path)
            .header("if", format!("(<{}>)", token.0))
            .header("timeout", format!("Second-{}", timeout.as_secs()));
        let r = self.send(req, &[])?;
        match r.status {
            200 | 201 => Ok(()),
            _ => Err(r.error()),
        }
    }

    pub fn unlock(&self, path: &str, token: &LockToken) -> Result<(), Error> {
        let req = self
            .request("UNLOCK", path)
            .header("lock-token", format!("<{}>", token.0));
        let r = self.send(req, &[])?;
        match r.status {
            200 | 204 => Ok(()),
            _ => Err(r.error()),
        }
    }

    /// Who holds the lock on `path`, if anyone (best effort: Nextcloud's
    /// display name, else the lock's `owner`).
    pub fn lock_owner(&self, path: &str) -> Result<Option<String>, Error> {
        const BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:nc="http://nextcloud.org/ns"><d:prop><d:lockdiscovery/><nc:lock-owner-displayname/><nc:lock-owner/></d:prop></d:propfind>"#;
        let text = self.propfind_raw(path, 0, BODY)?;
        Ok(parse_lock_owner(&text))
    }

    /// Create a folder. An existing one (405) is fine.
    pub fn mkcol(&self, path: &str) -> Result<(), Error> {
        let r = self.send(self.request("MKCOL", path), &[])?;
        match r.status {
            201 | 405 => Ok(()),
            _ => Err(r.error()),
        }
    }

    pub fn delete(&self, path: &str) -> Result<(), Error> {
        let r = self.send(self.request("DELETE", path), &[])?;
        match r.status {
            200 | 204 | 404 => Ok(()),
            _ => Err(r.error()),
        }
    }
}

/// How long a PUT may wait for its reply once the body is written.
pub fn reply_wait(body_len: u64) -> Duration {
    STALL_TIMEOUT + Duration::from_secs(body_len / SLOWEST_LINK_BYTES_PER_SEC)
}

struct ProgressReader<'a, 'p> {
    data: &'a [u8],
    pos: usize,
    progress: Progress<'p>,
}

impl Read for ProgressReader<'_, '_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = buf.len().min(self.data.len() - self.pos).min(64 * 1024);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        (self.progress)(self.pos as u64, self.data.len() as u64);
        Ok(n)
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

struct Reply {
    status: u16,
    etag: Option<String>,
    lock_token: Option<String>,
    gzip: bool,
    bytes: Vec<u8>,
}

impl Reply {
    fn read(resp: http::Response<ureq::Body>) -> Result<Self, Error> {
        let status = resp.status().as_u16();
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let etag = header("etag").or_else(|| header("oc-etag"));
        let lock_token = header("lock-token");
        let gzip = header("content-encoding").is_some_and(|e| e.eq_ignore_ascii_case("gzip"));
        let mut bytes = Vec::new();
        resp.into_body()
            .into_reader()
            .read_to_end(&mut bytes)
            .map_err(|e| Error::Network(e.to_string()))?;
        Ok(Self {
            status,
            etag,
            lock_token,
            gzip,
            bytes,
        })
    }

    /// The body with `Content-Encoding: gzip` undone.
    fn decoded(self) -> Result<Vec<u8>, Error> {
        if !self.gzip {
            return Ok(self.bytes);
        }
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(&self.bytes[..])
            .read_to_end(&mut out)
            .map_err(|e| Error::Protocol(format!("bad gzip reply: {e}")))?;
        Ok(out)
    }

    fn error(self) -> Error {
        status_error(self.status, &self.bytes)
    }
}

fn status_error(status: u16, body: &[u8]) -> Error {
    match status {
        401 | 403 => Error::Auth(status),
        404 => Error::NotFound,
        412 => Error::Conflict,
        423 => Error::Locked,
        425 => Error::TooEarly,
        _ => Error::Http {
            status,
            body: String::from_utf8_lossy(body).trim().to_string(),
        },
    }
}

/// Percent-encode one path segment: everything but RFC 3986 unreserved
/// characters and a few sub-delims servers accept unencoded.
fn percent_encode(seg: &str) -> String {
    let mut out = String::with_capacity(seg.len());
    for b in seg.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Decode `%XX` escapes (invalid UTF-8 becomes U+FFFD).
pub(crate) fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_encode_segments() {
        let c = Client::new("http://h:8080/remote.php/dav/files/admin", Auth::None);
        assert_eq!(c.base(), "http://h:8080/remote.php/dav/files/admin/");
        assert_eq!(
            c.url("Laporan/Q3 2026 (final).xlsx"),
            "http://h:8080/remote.php/dav/files/admin/Laporan/Q3%202026%20%28final%29.xlsx"
        );
        assert_eq!(
            c.url("été/"),
            "http://h:8080/remote.php/dav/files/admin/%C3%A9t%C3%A9/"
        );
        assert_eq!(c.url(""), c.base());
        assert_eq!(
            c.url("a//b#c?d"),
            "http://h:8080/remote.php/dav/files/admin/a/b%23c%3Fd"
        );
    }

    #[test]
    fn decode_round_trips() {
        for s in ["Q3 2026 (final).xlsx", "été", "100%", "a%2"] {
            assert_eq!(percent_decode(&percent_encode(s)), s);
        }
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("a%zz"), "a%zz");
    }

    #[test]
    fn reply_wait_grows_with_size() {
        assert_eq!(reply_wait(0), STALL_TIMEOUT);
        // 20 MB at the 128 kbit/s floor: about 21 minutes on top.
        assert_eq!(
            reply_wait(20_300_194),
            STALL_TIMEOUT + Duration::from_secs(20_300_194 / SLOWEST_LINK_BYTES_PER_SEC)
        );
        assert!(reply_wait(20_300_194) > Duration::from_secs(1200));
    }

    #[test]
    fn basic_auth_header() {
        let a = Auth::Basic {
            user: "admin".into(),
            password: "secret".into(),
        };
        assert_eq!(a.header().unwrap(), "Basic YWRtaW46c2VjcmV0");
        assert!(!format!("{a:?}").contains("secret"));
    }

    #[test]
    fn statuses_map_to_errors() {
        assert!(matches!(status_error(412, b""), Error::Conflict));
        assert!(matches!(status_error(423, b""), Error::Locked));
        assert!(matches!(status_error(425, b""), Error::TooEarly));
        assert!(matches!(status_error(401, b""), Error::Auth(401)));
        assert!(matches!(status_error(404, b""), Error::NotFound));
        assert!(matches!(
            status_error(507, b"full"),
            Error::Http { status: 507, .. }
        ));
    }
}
