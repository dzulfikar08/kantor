//! A small blocking client for opening and saving files on a drive: a
//! self-hosted WebDAV drive (Nextcloud, OpenCloud/OCIS, or any RFC 4918
//! server), Microsoft OneDrive (Microsoft Graph, see [`graph`]) or Google
//! Drive (Drive API v3, see [`gdrive`]). The same [`Client`] methods work
//! for all three, addressed by `/`-separated paths, so the sync rules
//! ([`session`], [`sync`]) and the working copies ([`cache`]) are shared.
//!
//! Only plain WebDAV is spoken so the server stays swappable: `PROPFIND`
//! (depth 0/1), `GET`, `PUT` with `If-Match` on the ETag so a save can't
//! silently overwrite someone else's, `LOCK`/`UNLOCK`, `MKCOL`, `DELETE`.
//! It's blocking (same ureq + rustls stack as `kantor_ai` and
//! `kantor_update`): call it off the UI thread.

pub mod cache;
pub mod chunked;
pub mod gdrive;
pub mod graph;
pub mod oauth;
pub mod oidc;
pub mod session;
pub mod sync;
pub mod tls;
mod xml;

use std::io::Read;
use std::time::Duration;

use base64::Engine as _;
use ureq::http::{self, Method};

pub use xml::{
    parse_display_name, parse_lock_owner, parse_lock_token, parse_lock_user, parse_multistatus,
    parse_user_search,
};

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
    /// OIDC access tokens from a source that refreshes them
    /// ([`oidc::TokenSource`]): each request gets a valid one, and a
    /// request the server refuses with 401 is sent once more with a fresh
    /// token.
    Source(std::sync::Arc<dyn oidc::BearerSource>),
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Auth::None => f.write_str("None"),
            Auth::Basic { user, .. } => write!(f, "Basic({user}, ***)"),
            Auth::Bearer(_) => f.write_str("Bearer(***)"),
            Auth::Source(_) => f.write_str("Source(***)"),
        }
    }
}

impl Auth {
    /// The `Authorization` header for the next request (a source may
    /// refresh its token first, or fail when the sign-in ended).
    fn header(&self) -> Result<Option<String>, Error> {
        Ok(match self {
            Auth::None => None,
            Auth::Basic { user, password } => Some(format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))
            )),
            Auth::Bearer(t) => Some(format!("Bearer {t}")),
            Auth::Source(s) => Some(format!("Bearer {}", s.token()?)),
        })
    }

    /// The server refused `header`: a source drops that token. Whether
    /// trying again can help.
    fn rejected(&self, header: Option<&str>) -> bool {
        match (self, header.and_then(|h| h.strip_prefix("Bearer "))) {
            (Auth::Source(s), Some(t)) => {
                s.rejected(t);
                true
            }
            _ => false,
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
    /// The user stopped a sign-in: Cancel in Kantor, declined consent in
    /// the browser (`access_denied`), or never came back (timed out). Not
    /// an error to show in red.
    Cancelled,
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
            Error::Cancelled => f.write_str("the sign-in was cancelled"),
        }
    }
}

impl std::error::Error for Error {}

/// One item of a listing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
    /// `oc:checksums` (Nextcloud, OpenCloud): `SHA1:… MD5:… ADLER32:…`.
    /// OpenCloud computes them for every file; Nextcloud keeps the one a
    /// client sent with `OC-Checksum` (Kantor sends SHA1).
    pub checksums: Option<String>,
}

impl Entry {
    /// The server's SHA1 of the content (lowercase hex), if it keeps one.
    pub fn sha1(&self) -> Option<String> {
        sha1_in(self.checksums.as_deref()?)
    }
}

/// The SHA1 in an `oc:checksums` value (`SHA1:ab… MD5:…`), lowercase.
pub fn sha1_in(checksums: &str) -> Option<String> {
    checksums
        .split_whitespace()
        .find_map(|c| {
            let (kind, hex) = c.split_once(':')?;
            kind.eq_ignore_ascii_case("sha1")
                .then(|| hex.to_ascii_lowercase())
        })
        .filter(|h| h.len() == 40 && h.chars().all(|c| c.is_ascii_hexdigit()))
}

/// SHA1 of `bytes` as lowercase hex, the form `OC-Checksum` takes.
pub fn sha1_hex(bytes: &[u8]) -> String {
    let d = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, bytes);
    d.as_ref().iter().map(|b| format!("{b:02x}")).collect()
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

/// Who holds a lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockOwner {
    /// The account id, when the server says it.
    pub user: Option<String>,
    /// The lock's owner text ("bob (Kantor)").
    pub text: String,
}

/// A held `LOCK`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockToken(pub String);

/// Which kind of drive a [`Client`] talks to.
#[derive(Clone)]
pub(crate) enum Backend {
    Dav,
    Graph,
    Drive(gdrive::Drive),
}

#[derive(Clone)]
pub struct Client {
    backend: Backend,
    agent: ureq::Agent,
    /// The agent's settings, for uploads that get an agent of their own
    /// (see [`Client::exec_watched`]).
    config: ureq::config::Config,
    /// Ends with `/`, e.g. `https://drive.example/remote.php/dav/files/alice/`.
    base: String,
    auth: Auth,
}

/// Slowest link a transfer is planned for (128 kbit/s): after a PUT body is
/// written, the reply may only come once the data still queued in socket
/// buffers has crossed the link, so the wait allowed for it grows with the
/// upload's size at this rate.
pub const SLOWEST_LINK_BYTES_PER_SEC: u64 = 16 * 1024;

/// A read or write that makes no progress for this long fails the request.
/// Transfers have no fixed overall timeout: a 20 MB save on 500 kbit/s takes
/// about 6 minutes and is fine as long as bytes keep moving.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// While an upload waits for its reply (data still crossing the link, or
/// the server assembling the file), the server is asked every this often,
/// on a new connection, whether it's still there.
pub const PROBE_EVERY: Duration = Duration::from_secs(10);

/// How long one of those checks may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Failed checks in a row that mean the link is gone.
const PROBES_TO_GIVE_UP: u32 = 2;

/// Upload progress: `(bytes sent, total)`.
pub type Progress<'a> = &'a mut dyn FnMut(u64, u64);

impl Client {
    /// `base` is the user's DAV root; paths given to the methods are relative
    /// to it, `/`-separated and unencoded (`Laporan/Q3 2026.xlsx`).
    pub fn new(base: &str, auth: Auth) -> Self {
        Self::with_backend(base, auth, Backend::Dav, "WebDAV")
    }

    fn with_backend(base: &str, auth: Auth, backend: Backend, kind: &str) -> Self {
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
            .user_agent(format!("Kantor/{} ({kind})", env!("CARGO_PKG_VERSION")))
            .tls_config(tls::tls_config())
            .build();
        let mut base = base.to_string();
        if !base.ends_with('/') {
            base.push('/');
        }
        Self {
            backend,
            agent: config.clone().into(),
            config,
            base,
            auth,
        }
    }

    /// A client for the signed-in user's OneDrive (Microsoft Graph).
    pub fn onedrive(auth: Auth) -> Self {
        Self::graph_at(graph::API, auth)
    }

    /// [`onedrive`](Self::onedrive) with another Graph drive address
    /// (`…/v1.0/me/drive`; a test server).
    pub fn graph_at(drive: &str, auth: Auth) -> Self {
        Self::with_backend(drive, auth, Backend::Graph, "OneDrive")
    }

    /// A client for the user's Google Drive. `ids` remembers which file id
    /// each path stands for (share it per account: see
    /// [`gdrive::shared_ids`]).
    pub fn google_drive(auth: Auth, ids: std::sync::Arc<gdrive::Ids>) -> Self {
        Self::google_drive_at(gdrive::API, gdrive::UPLOAD, auth, ids)
    }

    /// [`google_drive`](Self::google_drive) with other API addresses (a
    /// test server): `api` like `https://www.googleapis.com/drive/v3/`,
    /// `upload` like `https://www.googleapis.com/upload/drive/v3/`.
    pub fn google_drive_at(
        api: &str,
        upload: &str,
        auth: Auth,
        ids: std::sync::Arc<gdrive::Ids>,
    ) -> Self {
        let drive = gdrive::Drive::new(upload, ids);
        Self::with_backend(api, auth, Backend::Drive(drive), "Google Drive")
    }

    /// Upload `bytes` to `path` through an upload session (OneDrive,
    /// Google Drive). `url`: the session to resume, if any; set to the new
    /// session's address as soon as there is one, so a failed upload can
    /// be resumed later. Returns the new version tag.
    pub fn upload_session(
        &self,
        path: &str,
        bytes: &[u8],
        cond: Precondition,
        url: &mut Option<String>,
        progress: Progress,
    ) -> Result<Option<String>, Error> {
        match &self.backend {
            Backend::Graph => self.graph_upload_session(path, bytes, cond, url, progress),
            Backend::Drive(d) => d.upload_session(self, path, bytes, cond, url, progress),
            Backend::Dav => Err(Error::Protocol("WebDAV has no upload sessions".into())),
        }
    }

    /// The same client without credentials (pre-authorised addresses:
    /// Graph's download and upload-session URLs refuse a token).
    pub(crate) fn anonymous(&self) -> Client {
        Client {
            auth: Auth::None,
            ..self.clone()
        }
    }

    /// A JSON API request (Graph, Drive). `429` and `503` (throttling) are
    /// tried again after the server's `Retry-After`, three times at most.
    pub(crate) fn api_send(
        &self,
        make: &dyn Fn() -> http::request::Builder,
        body: &[u8],
    ) -> Result<Reply, Error> {
        let mut tries = 0u32;
        loop {
            let r = self.send(make(), body)?;
            if matches!(r.status, 429 | 503) && tries < 3 {
                tries += 1;
                let wait = r.retry_after.unwrap_or(1 << tries).min(30);
                log::info!("{} asks to wait {wait} s (HTTP {})", self.base, r.status);
                std::thread::sleep(Duration::from_secs(wait));
                continue;
            }
            return Ok(r);
        }
    }

    /// Whether the drive takes WebDAV locks (OneDrive and Google Drive
    /// don't offer them to apps: saves are guarded by versions only).
    pub fn has_locks(&self) -> bool {
        matches!(self.backend, Backend::Dav)
    }

    /// Files at least this big go through a resumable upload.
    pub fn resumable_min(&self) -> usize {
        match self.backend {
            // Graph's single PUT is meant for small files.
            Backend::Graph => graph::SESSION_MIN,
            _ => chunked::RESUMABLE_MIN,
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
        http::Request::builder()
            .method(Method::from_bytes(method.as_bytes()).expect("valid method"))
            .uri(self.url(path))
    }

    fn send(&self, req: http::request::Builder, body: &[u8]) -> Result<Reply, Error> {
        self.exec(req, body, None, None)
    }

    /// Whether the server answers at all (any HTTP status), on a new
    /// connection within [`PROBE_TIMEOUT`].
    pub fn reachable(&self) -> bool {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(PROBE_TIMEOUT))
            .tls_config(tls::tls_config())
            .build()
            .into();
        let mut b = http::Request::builder()
            .method(Method::HEAD)
            .uri(self.base.as_str());
        // Only whether it answers: no token refresh for this.
        if let Auth::Basic { .. } | Auth::Bearer(_) = &self.auth
            && let Ok(Some(a)) = self.auth.header()
        {
            b = b.header("authorization", a);
        }
        match b.body(()) {
            Ok(req) => agent.run(req).is_ok(),
            Err(_) => false,
        }
    }

    /// Run a request. A pooled keep-alive connection the server or a proxy
    /// already closed (after a dropped link) fails at once with a broken
    /// pipe or reset: the request is sent once more on a fresh connection.
    /// `wait`: how long the reply may take (uploads); `progress`: report
    /// the body's bytes as they go, offset by `.1`, of `.2`.
    ///
    /// Requests with a `wait` (uploads, assembling a chunked upload) are
    /// watched: see [`exec_watched`](Self::exec_watched).
    fn exec(
        &self,
        req: http::request::Builder,
        body: &[u8],
        wait: Option<Duration>,
        progress: Option<(Progress<'_>, u64, u64)>,
    ) -> Result<Reply, Error> {
        let (parts, ()) = req
            .body(())
            .map_err(|e| Error::Protocol(format!("request: {e}")))?
            .into_parts();
        let start = std::time::Instant::now();
        let what = log::log_enabled!(log::Level::Debug)
            .then(|| format!("{} {}", parts.method, parts.uri.path()));
        let r = match wait {
            Some(wait) => self.exec_watched(parts, body, wait, progress),
            None => self.exec_parts(&parts, body, None, progress),
        };
        // Every request's time (with any token refresh it needed), to find
        // pauses like a first sign-in's: RUST_LOG=kantor_cloud=debug.
        if let Some(what) = what {
            let outcome = match &r {
                Ok(reply) => format!("HTTP {}", reply.status),
                Err(e) => format!("error: {e}"),
            };
            log::debug!(
                "dav {what}: {outcome} after {:.2} s",
                start.elapsed().as_secs_f64()
            );
        }
        r
    }

    /// Run an upload on its own thread and tell a dead link from a slow
    /// one. While the body is being sent, [`STALL_TIMEOUT`] without a byte
    /// moving fails it. Once all of it is handed over, the reply may take
    /// long (bytes still queued on the way, a big file being assembled), so
    /// the server is checked every [`PROBE_EVERY`] on a new connection, and
    /// [`PROBES_TO_GIVE_UP`] failed checks in a row fail it.
    ///
    /// The upload gets an agent (connection) of its own, and giving up
    /// shuts it down: the socket closes at once, so the abandoned request
    /// can't come back to life when the link does and compete with the
    /// retry for the same part (it did: data a dead link had held was
    /// delivered later, and two copies of the same 5 MiB part crossed a
    /// 500 kbit/s link side by side). Parts that arrived stay on the
    /// server.
    fn exec_watched(
        &self,
        parts: http::request::Parts,
        body: &[u8],
        wait: Duration,
        mut progress: Option<(Progress<'_>, u64, u64)>,
    ) -> Result<Reply, Error> {
        use std::sync::mpsc::{RecvTimeoutError, channel};
        use std::time::Instant;

        enum Event {
            Sent(u64),
            Done(Result<Reply, Error>),
        }
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let what = format!(
            "{} …{}",
            parts.method,
            parts
                .uri
                .path()
                .rsplit('/')
                .take(2)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("/")
        );
        let (tx, rx) = channel::<Event>();
        let agent: ureq::Agent = self.config.clone().into();
        let mut me = self.clone();
        me.agent = agent.clone();
        let data: std::sync::Arc<[u8]> = std::sync::Arc::from(body);
        let len = data.len() as u64;
        let started = Instant::now();
        log::info!("dav #{id} {what}: start, {len} bytes");
        let worker_what = what.clone();
        std::thread::Builder::new()
            .name("kantor-dav".into())
            .spawn(move || {
                let sent_tx = tx.clone();
                let mut report = move |sent: u64, _: u64| {
                    let _ = sent_tx.send(Event::Sent(sent));
                };
                let r = me.exec_parts(&parts, &data, Some(wait), Some((&mut report, 0, len)));
                log::info!(
                    "dav #{id} {worker_what}: request ended after {:.1} s: {}",
                    started.elapsed().as_secs_f64(),
                    match &r {
                        Ok(reply) => format!("HTTP {}", reply.status),
                        Err(e) => e.to_string(),
                    }
                );
                let _ = tx.send(Event::Done(r));
            })
            .map_err(|e| Error::Network(format!("can't start a request: {e}")))?;
        let give_up = |sent: u64, why: &str| {
            log::warn!(
                "dav #{id} {what}: given up after {:.1} s, {sent} of {len} bytes handed over: {why}",
                started.elapsed().as_secs_f64()
            );
            agent.clone().shutdown();
        };
        let mut sent = 0u64;
        let mut moved = Instant::now();
        let mut next_probe: Option<Instant> = None;
        let mut failed_probes = 0;
        loop {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(Event::Sent(s)) => {
                    if s != sent {
                        sent = s;
                        moved = Instant::now();
                    }
                    if let Some((report, base, total)) = progress.as_mut() {
                        report(*base + s, *total);
                    }
                    continue;
                }
                Ok(Event::Done(r)) => {
                    log::info!(
                        "dav #{id} {what}: done after {:.1} s",
                        started.elapsed().as_secs_f64()
                    );
                    return r;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Error::Network("the request ended without an answer".into()));
                }
            }
            if sent < len {
                if moved.elapsed() >= STALL_TIMEOUT {
                    give_up(sent, "no data moving");
                    return Err(Error::Network(format!(
                        "no data moved for {} s",
                        STALL_TIMEOUT.as_secs()
                    )));
                }
                continue;
            }
            // Everything is handed over: wait for the reply while the
            // server still answers.
            let now = Instant::now();
            let due = *next_probe.get_or_insert(moved + PROBE_EVERY);
            if now < due {
                continue;
            }
            if self.reachable() {
                failed_probes = 0;
            } else {
                failed_probes += 1;
                if failed_probes >= PROBES_TO_GIVE_UP {
                    give_up(sent, "server checks failed");
                    return Err(Error::Network(
                        "the connection was lost while waiting for the server's reply".into(),
                    ));
                }
            }
            next_probe = Some(Instant::now() + PROBE_EVERY);
        }
    }

    /// [`exec`](Self::exec) on the calling thread.
    fn exec_parts(
        &self,
        parts: &http::request::Parts,
        body: &[u8],
        wait: Option<Duration>,
        mut progress: Option<(Progress<'_>, u64, u64)>,
    ) -> Result<Reply, Error> {
        // Once more on a stale pooled connection, and once more with a
        // fresh token when a token source's token was refused.
        let (mut stale_retried, mut auth_retried) = (false, false);
        loop {
            let header = self.auth.header()?;
            let mut b = http::Request::builder()
                .method(parts.method.clone())
                .uri(parts.uri.clone());
            for (k, v) in &parts.headers {
                if k != http::header::AUTHORIZATION {
                    b = b.header(k, v);
                }
            }
            if let Some(h) = &header {
                b = b.header("authorization", h);
            }
            let result = match progress.as_mut() {
                Some((report, base, total)) => {
                    let (base, total) = (*base, *total);
                    let mut f = |sent: u64, _: u64| report(base + sent, total);
                    let mut reader = ProgressReader {
                        data: body,
                        pos: 0,
                        progress: &mut f,
                    };
                    let req = b
                        .body(ureq::SendBody::from_reader(&mut reader))
                        .map_err(|e| Error::Protocol(format!("request: {e}")))?;
                    self.run_configured(req, wait)
                }
                None => {
                    let req = b
                        .body(body)
                        .map_err(|e| Error::Protocol(format!("request: {e}")))?;
                    self.run_configured(req, wait)
                }
            };
            match result {
                Err(e) if !stale_retried && stale_connection(&e) => stale_retried = true,
                Err(e) => return Err(Error::Network(e.to_string())),
                Ok(resp) => {
                    let reply = Reply::read(resp)?;
                    if reply.status == 401 && !auth_retried && self.auth.rejected(header.as_deref())
                    {
                        auth_retried = true;
                        continue;
                    }
                    return Ok(reply);
                }
            }
        }
    }

    fn run_configured<B: ureq::AsSendBody>(
        &self,
        req: http::Request<B>,
        wait: Option<Duration>,
    ) -> Result<http::Response<ureq::Body>, ureq::Error> {
        match wait {
            Some(w) => {
                let req = self
                    .agent
                    .configure_request(req)
                    .timeout_per_read(Some(w))
                    .timeout_recv_response(Some(w))
                    .build();
                self.agent.run(req)
            }
            None => self.agent.run(req),
        }
    }

    /// The children of a folder (depth 1), without the folder itself.
    pub fn list(&self, dir: &str) -> Result<Vec<Entry>, Error> {
        match &self.backend {
            Backend::Dav => {}
            Backend::Graph => return self.graph_list(dir),
            Backend::Drive(d) => return d.list(self, dir),
        }
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
        match &self.backend {
            Backend::Dav => {}
            Backend::Graph => return self.graph_stat(path),
            Backend::Drive(d) => return d.stat(self, path),
        }
        self.propfind(path, 0)?
            .into_iter()
            .next()
            .ok_or_else(|| Error::Protocol("empty PROPFIND reply".into()))
    }

    fn propfind(&self, path: &str, depth: u8) -> Result<Vec<Entry>, Error> {
        const BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns"><d:prop><d:resourcetype/><d:getcontentlength/><d:getetag/><d:getlastmodified/><d:getcontenttype/><oc:checksums/></d:prop></d:propfind>"#;
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
        match &self.backend {
            Backend::Dav => {}
            Backend::Graph => return self.graph_get(path),
            Backend::Drive(d) => return d.get(self, path),
        }
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
        match &self.backend {
            Backend::Dav => {}
            Backend::Graph => return self.graph_put(path, bytes, cond, progress),
            Backend::Drive(d) => return d.put(self, path, bytes, cond, progress),
        }
        // Nextcloud keeps this and reports it in `oc:checksums`, so a later
        // check can tell "same content, new ETag" without a download.
        let mut req = self
            .request("PUT", path)
            .header("content-type", "application/octet-stream")
            .header("content-length", bytes.len().to_string())
            .header("oc-checksum", format!("SHA1:{}", sha1_hex(bytes)));
        match cond {
            Precondition::IfMatch(etag) => req = req.header("if-match", etag),
            Precondition::IfNoneMatch => req = req.header("if-none-match", "*"),
            Precondition::Overwrite => {}
        }
        if let Some(LockToken(t)) = lock {
            req = req.header("if", format!("(<{t}>)"));
        }
        let total = bytes.len() as u64;
        let r = self.exec(
            req,
            bytes,
            Some(reply_wait(total)),
            Some((progress, 0, total)),
        )?;
        match r.status {
            200 | 201 | 204 => Ok(r.etag),
            _ => Err(r.error()),
        }
    }

    /// Take an exclusive write lock for `timeout` (servers may shorten it).
    /// `owner` is shown to others who find the file locked (servers that
    /// know the account show its name instead).
    pub fn lock(&self, path: &str, timeout: Duration, owner: &str) -> Result<LockToken, Error> {
        if !self.has_locks() {
            return Err(no_locks());
        }
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
        if !self.has_locks() {
            return Err(no_locks());
        }
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
        if !self.has_locks() {
            return Ok(());
        }
        let req = self
            .request("UNLOCK", path)
            .header("lock-token", format!("<{}>", token.0));
        let r = self.send(req, &[])?;
        match r.status {
            200 | 204 => Ok(()),
            _ => Err(r.error()),
        }
    }

    /// Who holds the lock on `path`, if anyone: the account id when the
    /// server says it, and the lock's owner text.
    pub fn lock_holder(&self, path: &str) -> Result<Option<LockOwner>, Error> {
        if !self.has_locks() {
            return Ok(None);
        }
        const BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:nc="http://nextcloud.org/ns"><d:prop><d:lockdiscovery/><nc:lock-owner-displayname/><nc:lock-owner/></d:prop></d:propfind>"#;
        let text = self.propfind_raw(path, 0, BODY)?;
        Ok(parse_lock_owner(&text).map(|owner| LockOwner {
            user: parse_lock_user(&text),
            text: owner,
        }))
    }

    /// An account's display name ("Bob Santoso"). Tried in order: the user
    /// search people get when sharing (Nextcloud autocomplete, OpenCloud
    /// Graph search; both work for ordinary users), then the user APIs
    /// (Nextcloud OCS, OpenCloud Graph; often admin-only). `None` when the
    /// server doesn't tell this user.
    pub fn display_name(&self, user: &str) -> Option<String> {
        if !self.has_locks() {
            return None;
        }
        let origin = self.origin();
        let enc = percent_encode(user);
        let searches = [
            format!("{origin}/ocs/v2.php/core/autocomplete/get?search={enc}&format=json"),
            format!("{origin}/graph/v1.0/users?$search=%22{enc}%22"),
        ];
        for url in searches {
            let req = http::Request::builder()
                .method(Method::GET)
                .uri(&url)
                .header("ocs-apirequest", "true")
                .header("accept", "application/json");
            if let Ok(r) = self.send(req, &[])
                && r.status == 200
                && let Some(name) = parse_user_search(&String::from_utf8_lossy(&r.bytes), user)
            {
                return Some(name);
            }
        }
        let urls = [
            format!("{origin}/ocs/v2.php/cloud/users/{enc}?format=json"),
            format!("{origin}/graph/v1.0/users/{enc}"),
        ];
        for url in urls {
            let req = http::Request::builder()
                .method(Method::GET)
                .uri(&url)
                .header("ocs-apirequest", "true")
                .header("accept", "application/json");
            let Ok(r) = self.send(req, &[]) else {
                continue;
            };
            if r.status == 200
                && let Some(name) = parse_display_name(&String::from_utf8_lossy(&r.bytes))
            {
                return Some(name);
            }
        }
        None
    }

    /// The server's address without the WebDAV path
    /// (`https://drive.example.go.id` from `…/remote.php/dav/files/budi/`).
    pub fn origin(&self) -> String {
        for marker in ["/remote.php/", "/dav/", "/webdav"] {
            if let Some(i) = self.base.find(marker) {
                return self.base[..i].to_owned();
            }
        }
        let after_scheme = self.base.find("://").map_or(0, |i| i + 3);
        match self.base[after_scheme..].find('/') {
            Some(i) => self.base[..after_scheme + i].to_owned(),
            None => self.base.trim_end_matches('/').to_owned(),
        }
    }

    /// Who holds the lock on `path`, if anyone (best effort: Nextcloud's
    /// display name, else the lock's `owner`).
    pub fn lock_owner(&self, path: &str) -> Result<Option<String>, Error> {
        if !self.has_locks() {
            return Ok(None);
        }
        const BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:nc="http://nextcloud.org/ns"><d:prop><d:lockdiscovery/><nc:lock-owner-displayname/><nc:lock-owner/></d:prop></d:propfind>"#;
        let text = self.propfind_raw(path, 0, BODY)?;
        Ok(parse_lock_owner(&text))
    }

    /// Create a folder. An existing one (405) is fine.
    pub fn mkcol(&self, path: &str) -> Result<(), Error> {
        match &self.backend {
            Backend::Dav => {}
            Backend::Graph => return self.graph_mkdir(path),
            Backend::Drive(d) => return d.mkdir(self, path),
        }
        let r = self.send(self.request("MKCOL", path), &[])?;
        match r.status {
            201 | 405 => Ok(()),
            _ => Err(r.error()),
        }
    }

    pub fn delete(&self, path: &str) -> Result<(), Error> {
        match &self.backend {
            Backend::Dav => {}
            Backend::Graph => return self.graph_delete(path),
            Backend::Drive(d) => return d.delete(self, path),
        }
        let r = self.send(self.request("DELETE", path), &[])?;
        match r.status {
            200 | 204 | 404 => Ok(()),
            _ => Err(r.error()),
        }
    }
}

/// What `lock` says on a drive without WebDAV locks.
fn no_locks() -> Error {
    Error::Http {
        status: 501,
        body: "this drive doesn't offer locks".into(),
    }
}

/// A connection that was already dead when reused: the request never
/// reached the server, so sending it again is safe.
fn stale_connection(e: &ureq::Error) -> bool {
    let s = e.to_string().to_ascii_lowercase();
    [
        "broken pipe",
        "connection reset",
        "peer disconnected",
        "connection aborted",
    ]
    .iter()
    .any(|p| s.contains(p))
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
    /// The server speaks TUS (`Tus-Resumable` header).
    tus: bool,
    upload_offset: Option<u64>,
    location: Option<String>,
    /// `Range` (Google's resumable uploads: what the server has).
    range: Option<String>,
    /// `Retry-After` in seconds (throttling).
    retry_after: Option<u64>,
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
        let tus = header("tus-resumable").is_some() || header("tus-version").is_some();
        let upload_offset = header("upload-offset").and_then(|o| o.trim().parse().ok());
        let location = header("location");
        let range = header("range");
        let retry_after = header("retry-after").and_then(|v| v.trim().parse().ok());
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
            tus,
            upload_offset,
            location,
            range,
            retry_after,
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

    /// The body as JSON (Graph, Drive replies).
    fn json(&self) -> Result<serde_json::Value, Error> {
        serde_json::from_slice(&self.bytes).map_err(|e| Error::Protocol(format!("JSON reply: {e}")))
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
pub(crate) fn percent_encode(seg: &str) -> String {
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
    fn sha1_from_checksums() {
        let h = "c98cd2493c3f31ea0cfc1b1cf9df6184c64fb89d";
        assert_eq!(sha1_in(&format!("SHA1:{h}")).as_deref(), Some(h));
        // OpenCloud lists several; case differs between servers.
        let upper = h.to_ascii_uppercase();
        assert_eq!(
            sha1_in(&format!("MD5:abc sha1:{upper} ADLER32:1234")).as_deref(),
            Some(h)
        );
        assert_eq!(sha1_in("MD5:abc"), None);
        assert_eq!(sha1_in("SHA1:nothex"), None);
        assert_eq!(sha1_hex(b"hello checksum\n"), h);
    }

    #[test]
    fn origins() {
        let c = |u: &str| Client::new(u, Auth::None).origin();
        assert_eq!(
            c("https://d.example/remote.php/dav/files/bob/"),
            "https://d.example"
        );
        assert_eq!(
            c("https://d.example/nc/remote.php/webdav/"),
            "https://d.example/nc"
        );
        assert_eq!(
            c("http://127.0.0.1:9200/dav/files/admin/"),
            "http://127.0.0.1:9200"
        );
        assert_eq!(c("https://x.example/files/"), "https://x.example");
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
        assert_eq!(
            a.header().unwrap().as_deref(),
            Some("Basic YWRtaW46c2VjcmV0")
        );
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
