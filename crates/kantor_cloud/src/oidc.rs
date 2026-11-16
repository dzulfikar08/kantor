//! Single sign-on for cloud accounts: OpenID Connect with the
//! organisation's identity provider (Keycloak, OpenCloud's own IdP, Azure
//! AD, …) instead of an app password.
//!
//! - **Discovery**: the server's own `/.well-known/openid-configuration`
//!   (OpenCloud serves its issuer's there), else the `openIdConnect`
//!   settings in its web client's `/config.json` (OpenCloud, ownCloud
//!   Infinite Scale), else an issuer URL the user types (Nextcloud with
//!   `user_oidc` doesn't publish its provider).
//! - **Sign-in**: Authorization Code with PKCE (S256) through the system
//!   browser and a one-shot listener on `127.0.0.1` (RFC 8252), or the
//!   Device Authorization Grant (RFC 8628) when no browser can reach this
//!   computer.
//! - **Use**: WebDAV requests carry `Authorization: Bearer`; the access
//!   token is refreshed shortly before it expires, and once more when the
//!   server refuses it ([`TokenSource`]). A refresh the provider refuses
//!   (session ended, password changed, access revoked) leaves the account
//!   needing a new sign-in; work stays in the working copy meanwhile.
//!
//! Blocking, like the rest of the crate: run it off the UI thread.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::Deserialize;

use crate::{Error, percent_decode, percent_encode};

/// What Kantor asks for: an id token, and a refresh token that outlives
/// the browser session (`offline_access`).
pub const SCOPE: &str = "openid profile email offline_access";

/// The scope without `offline_access`, for providers that refuse offline
/// tokens to this user or client: the refresh token then lasts as long as
/// the provider's session.
pub const SCOPE_ONLINE: &str = "openid profile email";

/// Whether a token or device error means the provider won't give offline
/// tokens (Keycloak: `not_allowed`, "Offline tokens not allowed").
pub fn offline_refused(e: &Error) -> bool {
    match e {
        Error::Http { body, .. } => {
            let b = body.to_ascii_lowercase();
            b.contains("offline") || b.starts_with("invalid_scope")
        }
        _ => false,
    }
}

/// The client id Kantor registers as by default (a public client with
/// PKCE, loopback redirects and the device grant).
pub const DEFAULT_CLIENT_ID: &str = "kantor";

/// OpenCloud's built-in IdP knows its desktop client under this id
/// (loopback redirects allowed); used when the server is OpenCloud and the
/// user didn't give another.
pub const OPENCLOUD_DESKTOP_CLIENT_ID: &str = "OpenCloudDesktop";

/// How long the browser sign-in may take.
pub const BROWSER_TIMEOUT: Duration = Duration::from_secs(300);

/// Refresh this long before the access token expires (at most half its
/// lifetime: a provider handing out one-minute tokens would otherwise get a
/// refresh before every request).
const REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// The provider's endpoints (from its discovery document).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Endpoints {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(default)]
    pub device_authorization_endpoint: Option<String>,
    #[serde(default)]
    pub revocation_endpoint: Option<String>,
}

/// What discovery found for a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    pub endpoints: Endpoints,
    /// A client id the server suggests (OpenCloud's desktop client, or the
    /// one in its web config when it is not the web client's own).
    pub client_id: Option<String>,
}

/// Tokens from the provider.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub id_token: Option<String>,
}

impl std::fmt::Debug for TokenSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenSource(***)")
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(5)
        // A drive creating the account on its first bearer request (Nextcloud
        // user_oidc provisioning) can take a while.
        .timeout_global(Some(Duration::from_secs(90)))
        .user_agent(concat!("Kantor/", env!("CARGO_PKG_VERSION"), " (OIDC)"))
        .tls_config(crate::tls::tls_config())
        .build()
        .into()
}

/// GET `url` as JSON: `(status, body)`.
fn get_json(url: &str) -> Result<(u16, serde_json::Value), Error> {
    let mut resp = agent()
        .get(url)
        .header("accept", "application/json")
        .call()
        .map_err(|e| Error::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    let body = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| Error::Network(e.to_string()))?;
    Ok((
        status,
        serde_json::from_str(&body).unwrap_or(serde_json::Value::Null),
    ))
}

/// POST a form: `(status, JSON body)`.
fn post_form(url: &str, form: &[(&str, &str)]) -> Result<(u16, serde_json::Value), Error> {
    let body = form_body(form);
    let mut resp = agent()
        .post(url)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "application/json")
        .send(body.as_bytes())
        .map_err(|e| Error::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    let text = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| Error::Network(e.to_string()))?;
    Ok((
        status,
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Null),
    ))
}

/// `a=1&b=x%20y`.
pub fn form_body(form: &[(&str, &str)]) -> String {
    form.iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// The provider's discovery document under `issuer`.
pub fn discover_issuer(issuer: &str) -> Result<Endpoints, Error> {
    let issuer = issuer.trim().trim_end_matches('/');
    let url = if issuer.ends_with("/.well-known/openid-configuration") {
        issuer.to_owned()
    } else {
        format!("{issuer}/.well-known/openid-configuration")
    };
    let start = Instant::now();
    let (status, json) = get_json(&url)?;
    log::debug!(
        "oidc discovery {url}: HTTP {status} after {:.2} s",
        start.elapsed().as_secs_f64()
    );
    if status != 200 {
        return Err(if status == 404 {
            Error::NotFound
        } else {
            Error::Http {
                status,
                body: String::new(),
            }
        });
    }
    serde_json::from_value(json)
        .map_err(|e| Error::Protocol(format!("not an OpenID Connect discovery document: {e}")))
}

/// Find the sign-in provider of a drive (see the module docs). `NotFound`
/// when the server doesn't tell (Nextcloud): ask for the issuer.
pub fn discover_server(server: &str) -> Result<Discovered, Error> {
    let origin = server.trim().trim_end_matches('/');
    // OpenCloud / oCIS answer for their issuer here.
    let last = match discover_issuer(origin) {
        Ok(endpoints) => {
            let client_id = opencloud_client(origin);
            return Ok(Discovered {
                endpoints,
                client_id,
            });
        }
        Err(e @ Error::Network(_)) => return Err(e),
        Err(e) => e,
    };
    // Their web client's settings name the issuer (an external IdP).
    if let Ok((200, config)) = get_json(&format!("{origin}/config.json")) {
        let oidc = &config["openIdConnect"];
        let issuer = oidc["metadata_url"]
            .as_str()
            .or_else(|| oidc["authority"].as_str());
        if let Some(issuer) = issuer {
            let endpoints = discover_issuer(issuer)?;
            return Ok(Discovered {
                endpoints,
                client_id: Some(OPENCLOUD_DESKTOP_CLIENT_ID.to_owned()),
            });
        }
    }
    Err(last)
}

/// OpenCloud's desktop client id when `origin` looks like OpenCloud/oCIS.
fn opencloud_client(origin: &str) -> Option<String> {
    let (status, json) = get_json(&format!("{origin}/config.json")).ok()?;
    (status == 200 && json["openIdConnect"].is_object())
        .then(|| OPENCLOUD_DESKTOP_CLIENT_ID.to_owned())
}

/// A PKCE pair (RFC 7636, S256).
#[derive(Debug, Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

/// `n` random bytes, base64url without padding.
pub fn random_token(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes)
        .expect("system random source");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

impl Pkce {
    pub fn new() -> Self {
        Self::from_verifier(random_token(32))
    }

    pub fn from_verifier(verifier: String) -> Self {
        let digest = ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes());
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest.as_ref());
        Self {
            verifier,
            challenge,
        }
    }
}

impl Default for Pkce {
    fn default() -> Self {
        Self::new()
    }
}

/// The address to send the browser to.
pub fn authorize_url(
    ep: &Endpoints,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    pkce: &Pkce,
    scope: &str,
) -> String {
    let sep = if ep.authorization_endpoint.contains('?') {
        '&'
    } else {
        '?'
    };
    format!(
        "{}{sep}{}",
        ep.authorization_endpoint,
        form_body(&[
            ("response_type", "code"),
            ("client_id", client_id),
            ("redirect_uri", redirect_uri),
            ("scope", scope),
            ("state", state),
            ("code_challenge", &pkce.challenge),
            ("code_challenge_method", "S256"),
        ])
    )
}

/// The one-shot listener the browser comes back to.
pub struct Loopback {
    listener: TcpListener,
    pub redirect_uri: String,
}

/// What the page in the browser says once the sign-in is done.
#[derive(Debug, Clone)]
pub struct DonePage {
    pub title: String,
    pub ok: String,
    pub failed: String,
}

impl Default for DonePage {
    fn default() -> Self {
        Self {
            title: "Kantor".into(),
            ok: "You're signed in. You can close this tab and go back to Kantor.".into(),
            failed: "Sign-in didn't finish. Go back to Kantor and try again.".into(),
        }
    }
}

impl Loopback {
    /// Listen on a free port of 127.0.0.1.
    pub fn bind() -> Result<Self, Error> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|e| Error::Network(format!("can't listen for the sign-in: {e}")))?;
        let port = listener
            .local_addr()
            .map_err(|e| Error::Network(e.to_string()))?
            .port();
        listener
            .set_nonblocking(true)
            .map_err(|e| Error::Network(e.to_string()))?;
        Ok(Self {
            listener,
            redirect_uri: format!("http://127.0.0.1:{port}/"),
        })
    }

    /// Wait for the browser to bring the code back (for `state`), until
    /// `timeout` or `cancel`.
    pub fn wait_for_code(
        &self,
        state: &str,
        timeout: Duration,
        cancel: &AtomicBool,
        page: &DonePage,
    ) -> Result<String, Error> {
        let until = Instant::now() + timeout;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Protocol("sign-in cancelled".into()));
            }
            if Instant::now() > until {
                return Err(Error::Protocol("sign-in timed out".into()));
            }
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if let Some(result) = answer(stream, state, page) {
                        return result;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(Error::Network(e.to_string())),
            }
        }
    }
}

/// Read one request on the loopback listener. `None`: not the redirect
/// (a favicon request), keep waiting.
fn answer(mut stream: TcpStream, state: &str, page: &DonePage) -> Option<Result<String, Error>> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line).ok()?;
    let target = line.split_whitespace().nth(1)?.to_owned();
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
    let params = parse_query(query);
    let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let result = if let Some(err) = get("error") {
        let desc = get("error_description").unwrap_or_default();
        Some(Err(Error::Protocol(
            format!("the sign-in was refused: {err} {desc}")
                .trim()
                .to_owned(),
        )))
    } else if let Some(code) = get("code") {
        if get("state").as_deref() == Some(state) {
            Some(Ok(code))
        } else {
            Some(Err(Error::Protocol(
                "sign-in reply for another request (state)".into(),
            )))
        }
    } else {
        None
    };
    let (status, text) = match &result {
        Some(Ok(_)) => ("200 OK", page.ok.as_str()),
        Some(Err(_)) => ("400 Bad Request", page.failed.as_str()),
        None => ("404 Not Found", ""),
    };
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title></head>\
         <body style=\"font-family:sans-serif;margin:4em auto;max-width:32em\"><p>{}</p></body></html>",
        html_escape(&page.title),
        html_escape(text)
    );
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{html}",
        html.len()
    );
    let _ = stream.flush();
    result
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `a=1&b=x%20y` → pairs (`+` is a space).
pub fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (
                percent_decode(&k.replace('+', " ")),
                percent_decode(&v.replace('+', " ")),
            )
        })
        .collect()
}

/// An error reply from the token endpoint as an [`Error`].
fn token_error(status: u16, json: &serde_json::Value) -> Error {
    let code = json["error"].as_str().unwrap_or("");
    match code {
        // The refresh token or code is no longer valid: sign in again.
        "invalid_grant" | "invalid_token" | "unauthorized_client" => Error::Auth(401),
        _ => Error::Http {
            status,
            body: format!(
                "{code} {}",
                json["error_description"].as_str().unwrap_or("")
            )
            .trim()
            .to_owned(),
        },
    }
}

fn tokens_from(status: u16, json: serde_json::Value) -> Result<Tokens, Error> {
    if status != 200 {
        return Err(token_error(status, &json));
    }
    let t: Tokens =
        serde_json::from_value(json).map_err(|e| Error::Protocol(format!("token reply: {e}")))?;
    if t.access_token.is_empty() {
        return Err(Error::Protocol(
            "token reply without an access token".into(),
        ));
    }
    Ok(t)
}

/// Trade the browser's code for tokens.
pub fn exchange_code(
    ep: &Endpoints,
    client_id: &str,
    code: &str,
    redirect_uri: &str,
    pkce: &Pkce,
) -> Result<Tokens, Error> {
    let (status, json) = post_form(
        &ep.token_endpoint,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id),
            ("code_verifier", &pkce.verifier),
        ],
    )?;
    tokens_from(status, json)
}

/// A new access token from a refresh token.
pub fn refresh(ep: &Endpoints, client_id: &str, refresh_token: &str) -> Result<Tokens, Error> {
    let (status, json) = post_form(
        &ep.token_endpoint,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
        ],
    )?;
    refresh_reply(status, json)
}

/// A refresh's reply. Besides `invalid_grant`, OpenCloud's IdP answers an
/// expired or revoked refresh token with `invalid_request`: for a refresh
/// that also means signing in again.
fn refresh_reply(status: u16, json: serde_json::Value) -> Result<Tokens, Error> {
    if matches!(status, 400 | 401) && json["error"].as_str() == Some("invalid_request") {
        return Err(Error::Auth(401));
    }
    tokens_from(status, json)
}

/// Tell the provider a refresh token is no longer wanted (RFC 7009).
/// `NotFound`: the provider has no revocation endpoint.
///
/// Keycloak ends the session the token belongs to: an offline token's
/// offline session, or a session token's browser session (the SSO login
/// itself). See [`retire_replaced`] before revoking a token that a new
/// sign-in replaces.
pub fn revoke(ep: &Endpoints, client_id: &str, refresh_token: &str) -> Result<(), Error> {
    let Some(url) = &ep.revocation_endpoint else {
        return Err(Error::NotFound);
    };
    let start = Instant::now();
    let (status, json) = post_form(
        url,
        &[
            ("token", refresh_token),
            ("token_type_hint", "refresh_token"),
            ("client_id", client_id),
        ],
    )?;
    log::debug!(
        "oidc revoke: HTTP {status} after {:.2} s",
        start.elapsed().as_secs_f64()
    );
    // RFC 7009: 200 also for a token that was no longer valid.
    if status == 200 {
        Ok(())
    } else {
        Err(token_error(status, &json))
    }
}

/// The provider session (`sid`) a token belongs to, when the token is a
/// JWT that says so: Keycloak's id, access, refresh and offline tokens do.
pub fn session_id(token: &str) -> Option<String> {
    jwt_claims(token)?["sid"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// The session a fresh sign-in's tokens belong to.
fn tokens_session(t: &Tokens) -> Option<String> {
    t.id_token
        .as_deref()
        .and_then(session_id)
        .or_else(|| t.refresh_token.as_deref().and_then(session_id))
        .or_else(|| session_id(&t.access_token))
}

/// What became of the refresh token a new sign-in replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Replaced {
    /// Revoked: its own session ended at the provider.
    Revoked,
    /// Kept: it belongs to the same provider session as the new sign-in
    /// (the browser was still signed in), so revoking it would end the new
    /// sign-in too. There is one session either way.
    SameSession,
    /// Kept: the tokens don't say which session they belong to, so
    /// revoking could end the new sign-in.
    Unknown,
    /// The provider refused the revocation or couldn't be reached (the old
    /// session then ends when it times out).
    Failed(String),
}

/// After "Sign in again" (or adding an account that was already there),
/// with the new sign-in checked and stored: revoke the `old` refresh token
/// when it belongs to another provider session than the `new` sign-in, so
/// the old sign-in's session doesn't stay usable.
///
/// Measured on Keycloak 26.8 (2026-11-16): revoking a token ends only the
/// session it belongs to. A sign-in in a browser that is still signed in
/// reuses that browser session (same `sid`) for session tokens; revoking
/// the old token then ends the new sign-in as well, hence the `sid`
/// comparison. Offline sign-ins get a session of their own each time.
/// Revoking *before* the new sign-in would sign the browser out (the
/// password is asked again) and lose the old token if the user cancels.
pub fn retire_replaced(ep: &Endpoints, client_id: &str, old: &str, new: &Tokens) -> Replaced {
    if new.refresh_token.as_deref() == Some(old) {
        return Replaced::SameSession;
    }
    let (old_sid, new_sid) = (session_id(old), tokens_session(new));
    let r = match (&old_sid, &new_sid) {
        (Some(o), Some(n)) if o == n => Replaced::SameSession,
        (Some(_), Some(_)) => match revoke(ep, client_id, old) {
            Ok(()) => Replaced::Revoked,
            Err(e) => Replaced::Failed(e.to_string()),
        },
        _ => Replaced::Unknown,
    };
    log::info!(
        "replaced sign-in: {r:?} (old session known: {}, new: {})",
        old_sid.is_some(),
        new_sid.is_some()
    );
    r
}

/// A device sign-in in progress (RFC 8628): show `user_code` and
/// `verification_uri` (or open `verification_uri_complete`).
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    #[serde(default = "default_interval")]
    pub interval: u64,
    #[serde(default = "default_expiry")]
    pub expires_in: u64,
}

fn default_interval() -> u64 {
    5
}

fn default_expiry() -> u64 {
    600
}

/// Start a device sign-in. `NotFound` when the provider has no device
/// endpoint.
/// `pkce` is sent too (Keycloak asks for it when the client requires
/// PKCE; other providers ignore it): pass the same one to [`device_wait`].
pub fn device_start(
    ep: &Endpoints,
    client_id: &str,
    pkce: &Pkce,
    scope: &str,
) -> Result<DeviceStart, Error> {
    let url = ep
        .device_authorization_endpoint
        .as_deref()
        .ok_or(Error::NotFound)?;
    let (status, json) = post_form(
        url,
        &[
            ("client_id", client_id),
            ("scope", scope),
            ("code_challenge", &pkce.challenge),
            ("code_challenge_method", "S256"),
        ],
    )?;
    if status != 200 {
        return Err(token_error(status, &json));
    }
    serde_json::from_value(json).map_err(|e| Error::Protocol(format!("device reply: {e}")))
}

/// A device sign-in ready to show: the codes and the PKCE pair to finish
/// with. Falls back to a session token's scope when offline tokens are
/// refused at the start.
pub fn device_begin(
    ep: &Endpoints,
    client_id: &str,
) -> Result<(DeviceStart, Pkce, &'static str), Error> {
    let pkce = Pkce::new();
    match device_start(ep, client_id, &pkce, SCOPE) {
        Err(e) if offline_refused(&e) => {
            let start = device_start(ep, client_id, &pkce, SCOPE_ONLINE)?;
            Ok((start, pkce, SCOPE_ONLINE))
        }
        r => r.map(|s| (s, pkce, SCOPE)),
    }
}

/// Device sign-in from start to tokens: `show` gets the code to show (a
/// second one when the provider refuses offline tokens only once the user
/// approved: the sign-in then starts again for a session token).
pub fn sign_in_device(
    ep: &Endpoints,
    client_id: &str,
    show: &dyn Fn(&DeviceStart),
    cancel: &AtomicBool,
) -> Result<Tokens, Error> {
    let (start, pkce, scope) = device_begin(ep, client_id)?;
    show(&start);
    match device_wait(ep, client_id, &start, &pkce, cancel) {
        Err(e) if scope == SCOPE && offline_refused(&e) => {
            log::info!("offline tokens refused; a new code for a session token");
            let pkce = Pkce::new();
            let start = device_start(ep, client_id, &pkce, SCOPE_ONLINE)?;
            show(&start);
            device_wait(ep, client_id, &start, &pkce, cancel)
        }
        r => r,
    }
}

/// Wait for the user to approve a device sign-in, polling as the provider
/// asks, until it expires or `cancel`.
pub fn device_wait(
    ep: &Endpoints,
    client_id: &str,
    start: &DeviceStart,
    pkce: &Pkce,
    cancel: &AtomicBool,
) -> Result<Tokens, Error> {
    let until = Instant::now() + Duration::from_secs(start.expires_in.max(30));
    let mut interval = Duration::from_secs(start.interval.clamp(1, 60));
    loop {
        // Sleep in small steps so a cancel is quick.
        let wake = Instant::now() + interval;
        while Instant::now() < wake {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Protocol("sign-in cancelled".into()));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if Instant::now() > until {
            return Err(Error::Protocol("the code expired; start again".into()));
        }
        let (status, json) = post_form(
            &ep.token_endpoint,
            &[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", &start.device_code),
                ("client_id", client_id),
                ("code_verifier", &pkce.verifier),
            ],
        )?;
        if status == 200 {
            return tokens_from(status, json);
        }
        match json["error"].as_str() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval += Duration::from_secs(5),
            Some("access_denied") => {
                return Err(Error::Protocol("the sign-in was refused".into()));
            }
            Some("expired_token") => {
                return Err(Error::Protocol("the code expired; start again".into()));
            }
            _ => return Err(token_error(status, &json)),
        }
    }
}

/// The claims of a JWT (id token), unverified: only for showing who
/// signed in. Never used to decide access.
pub fn jwt_claims(jwt: &str) -> Option<serde_json::Value> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Who the drive says the token belongs to: `(user id, display name)` from
/// its OCS API (Nextcloud and OpenCloud both answer). The id names the
/// WebDAV root (`/remote.php/dav/files/<id>/`).
pub fn whoami(origin: &str, access_token: &str) -> Result<(String, Option<String>), Error> {
    let url = format!(
        "{}/ocs/v2.php/cloud/user?format=json",
        origin.trim_end_matches('/')
    );
    let start = Instant::now();
    let mut resp = agent()
        .get(&url)
        .header("authorization", &format!("Bearer {access_token}"))
        .header("ocs-apirequest", "true")
        .header("accept", "application/json")
        .call()
        .map_err(|e| Error::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().unwrap_or_default();
    // A drive that creates the account on its first bearer request
    // (Nextcloud user_oidc copying the skeleton files) takes seconds here.
    log::debug!(
        "oidc whoami: HTTP {status} after {:.2} s",
        start.elapsed().as_secs_f64()
    );
    match status {
        200 => {}
        401 | 403 => return Err(Error::Auth(status)),
        404 => return Err(Error::NotFound),
        _ => {
            return Err(Error::Http {
                status,
                body: text.chars().take(200).collect(),
            });
        }
    }
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| Error::Protocol(format!("user reply: {e}")))?;
    let data = &json["ocs"]["data"];
    let id = data["id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Protocol("user reply without an id".into()))?;
    let name = data["display-name"]
        .as_str()
        .or_else(|| data["displayname"].as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    Ok((id.to_owned(), name))
}

/// Gives the client a valid bearer token for each request.
pub trait BearerSource: Send + Sync {
    /// The access token to send (refreshed when it is about to expire).
    fn token(&self) -> Result<String, Error>;
    /// The server refused `token`: drop it so the next [`Self::token`]
    /// refreshes.
    fn rejected(&self, token: &str);
}

struct Held {
    refresh: String,
    access: Option<(String, Instant)>,
}

/// Access tokens for one account, refreshed from its refresh token.
/// `persist` stores a rotated refresh token (the keychain).
pub struct TokenSource {
    /// The provider's issuer; its endpoints are looked up at the first
    /// refresh when not given (so making a source needs no network).
    issuer: String,
    endpoints: Mutex<Option<Endpoints>>,
    client_id: String,
    held: Mutex<Held>,
    persist: Box<dyn Fn(&str) + Send + Sync>,
    /// The provider refused the refresh token: sign in again.
    expired: AtomicBool,
}

impl TokenSource {
    pub fn new(
        endpoints: Endpoints,
        client_id: String,
        refresh_token: String,
        persist: impl Fn(&str) + Send + Sync + 'static,
    ) -> Self {
        Self {
            issuer: endpoints.issuer.clone(),
            endpoints: Mutex::new(Some(endpoints)),
            client_id,
            held: Mutex::new(Held {
                refresh: refresh_token,
                access: None,
            }),
            persist: Box::new(persist),
            expired: AtomicBool::new(false),
        }
    }

    /// A source for `issuer`, whose endpoints are discovered when the first
    /// token is needed.
    pub fn lazy(
        issuer: String,
        client_id: String,
        refresh_token: String,
        persist: impl Fn(&str) + Send + Sync + 'static,
    ) -> Self {
        Self {
            issuer,
            endpoints: Mutex::new(None),
            client_id,
            held: Mutex::new(Held {
                refresh: refresh_token,
                access: None,
            }),
            persist: Box::new(persist),
            expired: AtomicBool::new(false),
        }
    }

    /// The provider's endpoints (discovered once).
    pub fn endpoints(&self) -> Result<Endpoints, Error> {
        let mut e = self.endpoints.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ep) = e.as_ref() {
            return Ok(ep.clone());
        }
        let ep = endpoints_cached(&self.issuer)?;
        *e = Some(ep.clone());
        Ok(ep)
    }

    /// The refresh token held now (to revoke it).
    pub fn refresh_token(&self) -> String {
        self.held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .refresh
            .clone()
    }

    /// Start with the access token of a fresh sign-in.
    pub fn with_access(self, tokens: &Tokens) -> Self {
        if let Ok(mut h) = self.held.lock() {
            h.access = Some((tokens.access_token.clone(), expiry(tokens.expires_in)));
        }
        self
    }

    /// Whether the provider ended the sign-in (a new one is needed).
    pub fn needs_sign_in(&self) -> bool {
        self.expired.load(Ordering::Relaxed)
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }
}

/// When to stop using an access token that lives `expires_in` seconds.
fn expiry(expires_in: Option<u64>) -> Instant {
    // No lifetime given: assume five minutes, a common default.
    let life = Duration::from_secs(expires_in.unwrap_or(300));
    Instant::now() + life - REFRESH_MARGIN.min(life / 2)
}

impl BearerSource for TokenSource {
    fn token(&self) -> Result<String, Error> {
        let mut h = self.held.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((t, until)) = &h.access
            && Instant::now() < *until
        {
            return Ok(t.clone());
        }
        if self.expired.load(Ordering::Relaxed) {
            return Err(Error::Auth(401));
        }
        let endpoints = self.endpoints()?;
        let start = Instant::now();
        let refreshed = refresh(&endpoints, &self.client_id, &h.refresh);
        log::debug!(
            "oidc token refresh: {} after {:.2} s",
            if refreshed.is_ok() { "ok" } else { "failed" },
            start.elapsed().as_secs_f64()
        );
        match refreshed {
            Ok(t) => {
                if let Some(r) = t.refresh_token.as_ref().filter(|r| **r != h.refresh) {
                    h.refresh = r.clone();
                    (self.persist)(r);
                }
                h.access = Some((t.access_token.clone(), expiry(t.expires_in)));
                Ok(t.access_token)
            }
            Err(Error::Auth(s)) => {
                log::warn!("cloud sign-in ended: the provider refused the refresh token");
                self.expired.store(true, Ordering::Relaxed);
                Err(Error::Auth(s))
            }
            Err(e) => Err(e),
        }
    }

    fn rejected(&self, token: &str) {
        let mut h = self.held.lock().unwrap_or_else(|e| e.into_inner());
        if h.access.as_ref().is_some_and(|(t, _)| t == token) {
            h.access = None;
        }
    }
}

/// Token sources by account id, shared by every client of the account so
/// one refresh serves them all.
static SOURCES: Mutex<Vec<(String, Arc<TokenSource>)>> = Mutex::new(Vec::new());

/// The account's token source, made by `make` the first time.
pub fn shared_source(
    account: &str,
    make: impl FnOnce() -> Result<TokenSource, Error>,
) -> Result<Arc<TokenSource>, Error> {
    let mut all = SOURCES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, s)) = all.iter().find(|(a, _)| a == account) {
        return Ok(s.clone());
    }
    let s = Arc::new(make()?);
    all.push((account.to_owned(), s.clone()));
    Ok(s)
}

/// Put a fresh sign-in's source in place of the account's old one.
pub fn replace_source(account: &str, source: TokenSource) -> Arc<TokenSource> {
    let mut all = SOURCES.lock().unwrap_or_else(|e| e.into_inner());
    all.retain(|(a, _)| a != account);
    let s = Arc::new(source);
    all.push((account.to_owned(), s.clone()));
    s
}

/// Forget an account's source (account removed).
pub fn forget_source(account: &str) {
    SOURCES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|(a, _)| a != account);
}

/// The account's source, if one was made this run.
pub fn source_of(account: &str) -> Option<Arc<TokenSource>> {
    SOURCES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(a, _)| a == account)
        .map(|(_, s)| s.clone())
}

/// Endpoints of an issuer, discovered once per run.
pub fn endpoints_cached(issuer: &str) -> Result<Endpoints, Error> {
    static CACHE: Mutex<Vec<(String, Endpoints)>> = Mutex::new(Vec::new());
    if let Some((_, e)) = CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(i, _)| i == issuer)
    {
        return Ok(e.clone());
    }
    let e = discover_issuer(issuer)?;
    CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((issuer.to_owned(), e.clone()));
    Ok(e)
}

/// How a sign-in went: the tokens, the provider's endpoints and the client
/// id used.
pub struct SignedIn {
    pub tokens: Tokens,
    pub endpoints: Endpoints,
    pub client_id: String,
}

/// Browser sign-in: `open` gets the address to open in the system browser.
/// When the provider refuses offline tokens, signs in once more without
/// asking for them (the browser is signed in by then, so it comes straight
/// back).
pub fn sign_in_browser(
    endpoints: &Endpoints,
    client_id: &str,
    open: &dyn Fn(&str),
    cancel: &AtomicBool,
    page: &DonePage,
) -> Result<Tokens, Error> {
    match browser_once(endpoints, client_id, open, cancel, page, SCOPE) {
        Err(e) if offline_refused(&e) => {
            log::info!("offline tokens refused; signing in for a session token");
            browser_once(endpoints, client_id, open, cancel, page, SCOPE_ONLINE)
        }
        r => r,
    }
}

/// One browser sign-in asking for `scope` ([`SCOPE`] or [`SCOPE_ONLINE`]),
/// without the fallback of [`sign_in_browser`].
pub fn browser_once(
    endpoints: &Endpoints,
    client_id: &str,
    open: &dyn Fn(&str),
    cancel: &AtomicBool,
    page: &DonePage,
    scope: &str,
) -> Result<Tokens, Error> {
    let loopback = Loopback::bind()?;
    let pkce = Pkce::new();
    let state = random_token(16);
    open(&authorize_url(
        endpoints,
        client_id,
        &loopback.redirect_uri,
        &state,
        &pkce,
        scope,
    ));
    let code = loopback.wait_for_code(&state, BROWSER_TIMEOUT, cancel, page)?;
    exchange_code(endpoints, client_id, &code, &loopback.redirect_uri, &pkce)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_lived_access_tokens_are_used_before_refreshing() {
        // Keycloak realms with one-minute tokens: used for 30 s, not refreshed
        // before every request.
        let now = Instant::now();
        let e = expiry(Some(60));
        assert!(
            e >= now + Duration::from_secs(30) && e <= Instant::now() + Duration::from_secs(30)
        );
        // An hour: refreshed a minute early.
        assert!(expiry(Some(3600)) >= now + Duration::from_secs(3540));
        assert!(expiry(Some(0)) <= Instant::now());
    }

    #[test]
    fn pkce_matches_rfc_7636_example() {
        // RFC 7636 appendix B.
        let p = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
        assert_eq!(p.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let a = Pkce::new();
        let b = Pkce::new();
        assert_ne!(a.verifier, b.verifier);
        assert!(a.verifier.len() >= 43);
    }

    #[test]
    fn authorize_url_carries_pkce_and_state() {
        let ep = Endpoints {
            issuer: "https://sso.example/realms/pemda".into(),
            authorization_endpoint: "https://sso.example/realms/pemda/protocol/openid-connect/auth"
                .into(),
            token_endpoint: "https://sso.example/t".into(),
            device_authorization_endpoint: None,
            revocation_endpoint: None,
        };
        let p = Pkce::from_verifier("v".repeat(43));
        let url = authorize_url(&ep, "kantor", "http://127.0.0.1:5000/", "s1", &p, SCOPE);
        assert!(url.starts_with("https://sso.example/realms/pemda/protocol/openid-connect/auth?response_type=code&client_id=kantor&redirect_uri=http%3A%2F%2F127.0.0.1%3A5000%2F"));
        assert!(url.contains("&state=s1&"));
        assert!(url.contains(&format!("code_challenge={}", p.challenge)));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("scope=openid%20profile%20email%20offline_access"));
    }

    #[test]
    fn loopback_returns_the_code_for_our_state_only() {
        let lb = Loopback::bind().unwrap();
        let port = lb
            .redirect_uri
            .trim_start_matches("http://127.0.0.1:")
            .trim_end_matches('/')
            .to_owned();
        let hit = move |path: &str| {
            let mut s = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
            write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
            let mut out = String::new();
            let _ = std::io::Read::read_to_string(&mut s, &mut out);
            out
        };
        let cancel = AtomicBool::new(false);
        let h = std::thread::spawn({
            let hit = hit.clone();
            move || {
                std::thread::sleep(Duration::from_millis(150));
                let f = hit("/favicon.ico");
                let ok = hit("/?state=xyz&code=abc%2B1&session_state=q");
                (f, ok)
            }
        });
        let code = lb
            .wait_for_code(
                "xyz",
                Duration::from_secs(10),
                &cancel,
                &DonePage::default(),
            )
            .unwrap();
        assert_eq!(code, "abc+1");
        let (f, ok) = h.join().unwrap();
        assert!(f.starts_with("HTTP/1.1 404"));
        assert!(ok.starts_with("HTTP/1.1 200") && ok.contains("signed in"));

        // Another request's state, or a refusal, is an error.
        let lb = Loopback::bind().unwrap();
        let port = lb
            .redirect_uri
            .trim_start_matches("http://127.0.0.1:")
            .trim_end_matches('/')
            .to_owned();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            let mut s = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
            write!(s, "GET /?error=access_denied&state=xyz HTTP/1.1\r\n\r\n").unwrap();
        });
        assert!(
            lb.wait_for_code(
                "xyz",
                Duration::from_secs(10),
                &cancel,
                &DonePage::default()
            )
            .is_err()
        );
        // Cancelled.
        let lb = Loopback::bind().unwrap();
        cancel.store(true, Ordering::Relaxed);
        assert!(
            lb.wait_for_code(
                "xyz",
                Duration::from_secs(10),
                &cancel,
                &DonePage::default()
            )
            .is_err()
        );
    }

    #[test]
    fn jwt_claims_are_read_for_display() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"preferred_username":"budi","name":"Budi Santoso"}"#);
        let c = jwt_claims(&format!("h.{payload}.s")).unwrap();
        assert_eq!(c["preferred_username"], "budi");
        assert!(jwt_claims("nope").is_none());
    }

    fn jwt_with_sid(sid: &str) -> String {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(format!(r#"{{"typ":"Offline","sid":"{sid}"}}"#));
        format!("h.{payload}.s")
    }

    #[test]
    fn a_replaced_token_of_the_same_session_is_never_revoked() {
        // No revocation endpoint: any attempt to revoke shows as Failed.
        let ep = Endpoints {
            issuer: "https://sso.example/realms/pemda".into(),
            authorization_endpoint: "https://sso.example/a".into(),
            token_endpoint: "https://sso.example/t".into(),
            device_authorization_endpoint: None,
            revocation_endpoint: None,
        };
        assert_eq!(session_id(&jwt_with_sid("s1")).as_deref(), Some("s1"));
        assert_eq!(session_id("opaque-token"), None);
        let new = |refresh: &str, id: Option<&str>| Tokens {
            access_token: "a".into(),
            refresh_token: Some(refresh.into()),
            id_token: id.map(jwt_with_sid),
            ..Default::default()
        };
        let old = jwt_with_sid("s1");
        // The browser was still signed in: same session, kept.
        assert_eq!(
            retire_replaced(&ep, "kantor", &old, &new(&jwt_with_sid("s1"), Some("s1"))),
            Replaced::SameSession
        );
        // The id token says the session even when the refresh token is opaque.
        assert_eq!(
            retire_replaced(&ep, "kantor", &old, &new("opaque", Some("s1"))),
            Replaced::SameSession
        );
        // Can't tell: kept.
        assert_eq!(
            retire_replaced(&ep, "kantor", "opaque-old", &new("opaque", None)),
            Replaced::Unknown
        );
        // Another session: revoked (here: tried, no endpoint).
        assert!(matches!(
            retire_replaced(&ep, "kantor", &old, &new(&jwt_with_sid("s2"), Some("s2"))),
            Replaced::Failed(_)
        ));
    }

    #[test]
    fn dead_refresh_tokens_mean_sign_in_again() {
        let r = refresh_reply(
            400,
            serde_json::json!({"error": "invalid_request", "error_description": "token is expired"}),
        );
        assert!(matches!(r, Err(Error::Auth(401))));
        let r = refresh_reply(400, serde_json::json!({"error": "invalid_grant"}));
        assert!(matches!(r, Err(Error::Auth(401))));
        let r = refresh_reply(503, serde_json::json!({}));
        assert!(matches!(r, Err(Error::Http { status: 503, .. })));
    }

    #[test]
    fn offline_refusals_are_recognised() {
        let e = token_error(
            400,
            &serde_json::json!({"error": "not_allowed", "error_description": "Offline tokens not allowed for the user or client"}),
        );
        assert!(offline_refused(&e));
        let e = token_error(400, &serde_json::json!({"error": "invalid_scope"}));
        assert!(offline_refused(&e));
        let e = token_error(400, &serde_json::json!({"error": "invalid_request"}));
        assert!(!offline_refused(&e));
    }

    #[test]
    fn token_errors_that_mean_sign_in_again() {
        let e = token_error(400, &serde_json::json!({"error": "invalid_grant"}));
        assert!(matches!(e, Error::Auth(401)));
        let e = token_error(
            400,
            &serde_json::json!({"error": "invalid_request", "error_description": "x"}),
        );
        assert!(matches!(e, Error::Http { status: 400, .. }));
    }

    #[test]
    fn queries_decode() {
        assert_eq!(
            parse_query("a=1&b=x+y&c=%2F"),
            vec![
                ("a".into(), "1".into()),
                ("b".into(), "x y".into()),
                ("c".into(), "/".into())
            ]
        );
        assert_eq!(form_body(&[("a", "x y"), ("b", "/")]), "a=x%20y&b=%2F");
    }
}
