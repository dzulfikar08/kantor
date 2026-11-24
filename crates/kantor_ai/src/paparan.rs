//! Paparan AI: Kantor's hosted provider. No API key: the user signs in with
//! a Paparan account (a device code confirmed in the browser) and pays for
//! credits; the chat endpoint is OpenAI-compatible ([`crate::Client`] with
//! [`crate::ProviderId::Paparan`]).
//!
//! Endpoints, under [`api_base`] (`https://app.paparanai.id/api/kantor`,
//! `KANTOR_PAPARAN_URL` overrides it for testing):
//! - `POST device/start`, `POST device/token`: sign in ([`sign_in`]);
//! - `POST device/revoke`: sign out;
//! - `GET me`: account, plan and credit balance ([`Me`]);
//! - `POST v1/chat/completions`: chat, answering with the balance left in
//!   `X-Paparan-Credits-Remaining` ([`note_response`]).
//!
//! The token lives in the keychain like an API key (entry `paparan` of
//! [`keys::AI`]) and is never logged.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::AiError;
use crate::client::Cancel;
use crate::error::{error_message, truncate};
use crate::keys::{self, KeyLocation};
use crate::transport::{HttpRequest, HttpResponse, Transport};

/// The Kantor API's address when `KANTOR_PAPARAN_URL` isn't set.
pub const DEFAULT_URL: &str = "https://app.paparanai.id/api/kantor/";

/// Models Paparan AI offers; the server picks the underlying model.
pub const MODELS: &[&str] = &["paparan-auto", "paparan-fast", "paparan-smart"];
pub const DEFAULT_MODEL: &str = "paparan-auto";

/// Sent as `client_version` and `X-Kantor-Version`.
pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The keychain entry (in [`keys::AI`]) holding the sign-in token.
pub const TOKEN_NAME: &str = "paparan";

/// The balance left after a chat request.
pub const CREDITS_HEADER: &str = "x-paparan-credits-remaining";
/// Optional: whether that balance is low (`1`/`true`).
pub const CREDITS_LOW_HEADER: &str = "x-paparan-credits-low";

/// The Kantor API's address without a trailing slash.
pub fn api_base() -> String {
    api_base_from(std::env::var("KANTOR_PAPARAN_URL").ok().as_deref())
}

fn api_base_from(env: Option<&str>) -> String {
    env.map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_URL)
        .trim_end_matches('/')
        .to_owned()
}

/// The OpenAI-compatible base (`…/v1`, `/chat/completions` appended).
pub fn chat_base() -> String {
    format!("{}/v1", api_base())
}

/// The stored sign-in token. Can block (keychain): off the UI thread.
pub fn load_token() -> Option<String> {
    keys::AI.load(TOKEN_NAME)
}

pub fn save_token(token: &str) -> Result<KeyLocation, String> {
    keys::AI.save(TOKEN_NAME, token)
}

pub fn delete_token() {
    keys::AI.delete(TOKEN_NAME)
}

/// The name this computer shows in the account's device list.
pub fn device_name() -> String {
    let host = ["HOSTNAME", "COMPUTERNAME"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .chain(std::fs::read_to_string("/etc/hostname").ok())
        .map(|h| h.trim().to_owned())
        .find(|h| !h.is_empty());
    match host {
        Some(h) => format!("Kantor on {h}"),
        None => format!("Kantor ({})", std::env::consts::OS),
    }
}

// ── Replies ─────────────────────────────────────────────────────────────

/// `POST device/start`'s answer.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct DeviceStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    /// Seconds between polls.
    #[serde(default = "default_interval")]
    pub interval: u64,
    /// Seconds the code stays valid.
    #[serde(default = "default_expires")]
    pub expires_in: u64,
}

fn default_interval() -> u64 {
    5
}

fn default_expires() -> u64 {
    600
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Account {
    pub id: String,
    pub email: String,
    pub name: String,
}

impl Account {
    fn from_json(v: &Value) -> Self {
        let s = |k: &str| match v.get(k) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => String::new(),
        };
        Self {
            id: s("id"),
            email: s("email"),
            name: s("name"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Credits {
    pub remaining: f64,
    /// What a credit is called ("credits", "IDR", …).
    pub unit: String,
    pub low: bool,
}

impl Credits {
    /// "1,250 credits" (whole numbers without decimals).
    pub fn display(&self) -> String {
        let n = format_amount(self.remaining);
        if self.unit.is_empty() {
            n
        } else {
            format!("{n} {}", self.unit)
        }
    }
}

/// `1250.0` → `1,250`; `3.5` → `3.5`.
pub fn format_amount(n: f64) -> String {
    let neg = n < 0.0;
    let n = (n.abs() * 100.0).round() / 100.0;
    let whole = n.trunc() as u64;
    let frac = n - n.trunc();
    let digits = whole.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if frac > 0.005 {
        let f = format!("{frac:.2}");
        out.push_str(f.trim_start_matches('0').trim_end_matches('0'));
    }
    if neg { format!("-{out}") } else { out }
}

/// `GET me`'s answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Me {
    pub account: Account,
    /// The plan's name.
    pub plan: String,
    pub credits: Credits,
    pub topup_url: Option<String>,
    pub manage_url: Option<String>,
}

impl Me {
    pub fn from_json(v: &Value) -> Self {
        let plan = match v.get("plan") {
            Some(Value::String(s)) => s.clone(),
            Some(p) => p
                .get("name")
                .or_else(|| p.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            None => String::new(),
        };
        let c = v.get("credits").cloned().unwrap_or(Value::Null);
        let url = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        Self {
            account: Account::from_json(v.get("account").unwrap_or(&Value::Null)),
            plan,
            credits: Credits {
                remaining: number(c.get("remaining")).unwrap_or(0.0),
                unit: c
                    .get("unit")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                low: c.get("low").and_then(Value::as_bool).unwrap_or(false),
            },
            topup_url: url("topup_url"),
            manage_url: url("manage_url"),
        }
    }
}

/// A number sent as a number or a string.
fn number(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// `POST device/token`'s answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Poll {
    Pending,
    Denied,
    Expired,
    Ok { token: String, account: Account },
}

/// Why a sign-in didn't finish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInError {
    /// The user declined in the browser.
    Denied,
    /// The code ran out before it was confirmed.
    Expired,
    Ai(AiError),
}

impl From<AiError> for SignInError {
    fn from(e: AiError) -> Self {
        Self::Ai(e)
    }
}

impl std::fmt::Display for SignInError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Denied => write!(f, "The sign-in was declined."),
            Self::Expired => write!(f, "The sign-in code expired. Sign in again."),
            Self::Ai(e) => write!(f, "{e}"),
        }
    }
}

// ── Credits seen on chat replies ────────────────────────────────────────

/// The balance a chat reply reported.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CreditsSeen {
    pub remaining: f64,
    /// `None` when the server didn't say.
    pub low: Option<bool>,
}

static LAST_CREDITS: Mutex<Option<CreditsSeen>> = Mutex::new(None);

/// The balance in a response's headers.
pub fn credits_from(resp: &HttpResponse) -> Option<CreditsSeen> {
    let remaining = resp.header(CREDITS_HEADER)?.trim().parse::<f64>().ok()?;
    let low = resp
        .header(CREDITS_LOW_HEADER)
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"));
    Some(CreditsSeen { remaining, low })
}

/// Remember the balance a response reported (see [`take_credits`]).
pub fn note_response(resp: &HttpResponse) {
    if let Some(c) = credits_from(resp)
        && let Ok(mut last) = LAST_CREDITS.lock()
    {
        *last = Some(c);
    }
}

/// The balance reported since the last call, if any.
pub fn take_credits() -> Option<CreditsSeen> {
    LAST_CREDITS.lock().ok().and_then(|mut l| l.take())
}

// ── Errors ──────────────────────────────────────────────────────────────

/// The error a non-2xx response stands for. Paparan answers
/// `{"error": {"code", "message", "topup_url"?}}`.
pub fn error_from_response(resp: HttpResponse) -> AiError {
    let status = resp.status;
    let retry_after = resp
        .header("retry-after")
        .and_then(|v| v.trim().parse::<u64>().ok());
    let body = resp.read_text();
    let json: Value = serde_json::from_str(body.trim()).unwrap_or(Value::Null);
    let err = json.get("error").cloned().unwrap_or(Value::Null);
    let code = err.get("code").and_then(Value::as_str).unwrap_or_default();
    let message = error_message(&body);
    match (status, code) {
        (401, _) => AiError::SignedOut,
        (402, _) | (_, "insufficient_credits") => AiError::InsufficientCredits {
            message,
            topup_url: topup_url(&err),
        },
        (429, _) => AiError::RateLimited {
            retry_after,
            message,
        },
        _ => AiError::Http {
            status,
            message: truncate(&message, 300),
        },
    }
}

/// An error named in a streamed event (`{"error": {"code": "…"}}`).
pub fn error_from_code(code: &str, message: String, err: &Value) -> AiError {
    match code {
        "insufficient_credits" => AiError::InsufficientCredits {
            message,
            topup_url: topup_url(err),
        },
        "unauthorized" | "invalid_token" | "signed_out" => AiError::SignedOut,
        "rate_limited" => AiError::RateLimited {
            retry_after: None,
            message,
        },
        _ => AiError::Http {
            status: 500,
            message,
        },
    }
}

fn topup_url(err: &Value) -> Option<String> {
    err.get("topup_url")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

// ── The account API ─────────────────────────────────────────────────────

/// Sign-in, sign-out and balance calls.
pub struct Paparan {
    base: String,
    transport: Arc<dyn Transport>,
}

impl Paparan {
    /// Against [`api_base`].
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self::with_base(api_base(), transport)
    }

    pub fn with_base(base: impl Into<String>, transport: Arc<dyn Transport>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_owned(),
            transport,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.base)
    }

    fn call(&self, req: HttpRequest) -> Result<Value, AiError> {
        let resp = self.transport.send(&req)?;
        note_response(&resp);
        if !(200..300).contains(&resp.status) {
            return Err(error_from_response(resp));
        }
        let body = resp.read_text();
        if body.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(body.trim())
            .map_err(|_| AiError::BadReply(format!("unexpected answer: {}", truncate(&body, 120))))
    }

    fn bearer(token: &str) -> Vec<(&'static str, String)> {
        vec![("authorization", format!("Bearer {token}"))]
    }

    /// Start a sign-in: the code to show and the page to open.
    pub fn start(&self, device_name: &str) -> Result<DeviceStart, AiError> {
        let body = json!({
            "client": "kantor",
            "client_version": CLIENT_VERSION,
            "device_name": device_name,
        });
        let v = self.call(HttpRequest::post(
            self.url("device/start"),
            Vec::new(),
            body.to_string(),
        ))?;
        serde_json::from_value(v).map_err(|e| AiError::BadReply(e.to_string()))
    }

    /// Whether the code was confirmed yet.
    pub fn poll(&self, device_code: &str) -> Result<Poll, AiError> {
        let body = json!({ "device_code": device_code });
        let v = self.call(HttpRequest::post(
            self.url("device/token"),
            Vec::new(),
            body.to_string(),
        ))?;
        Ok(match v.get("status").and_then(Value::as_str) {
            Some("ok") => {
                let token = v
                    .get("token")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                    .ok_or_else(|| AiError::BadReply("no token in the sign-in answer".into()))?
                    .to_owned();
                Poll::Ok {
                    token,
                    account: Account::from_json(v.get("account").unwrap_or(&Value::Null)),
                }
            }
            Some("denied") => Poll::Denied,
            Some("expired") => Poll::Expired,
            _ => Poll::Pending,
        })
    }

    /// End the sign-in on the server.
    pub fn revoke(&self, token: &str) -> Result<(), AiError> {
        self.call(HttpRequest::post(
            self.url("device/revoke"),
            Self::bearer(token),
            "{}".into(),
        ))
        .map(|_| ())
    }

    pub fn me(&self, token: &str) -> Result<Me, AiError> {
        let v = self.call(HttpRequest::get(self.url("me"), Self::bearer(token)))?;
        Ok(Me::from_json(&v))
    }
}

/// A finished sign-in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedIn {
    pub token: String,
    pub account: Account,
}

/// Run the device sign-in: start, hand the code to `on_code` (show it and
/// open the page), then poll until it's confirmed, declined, expired or
/// cancelled. `sleep` waits between polls (and returns `Cancelled` when
/// asked to stop); tests pass one that doesn't sleep.
pub fn sign_in(
    api: &Paparan,
    device_name: &str,
    cancel: &Cancel,
    on_code: &mut dyn FnMut(&DeviceStart),
    sleep: &mut dyn FnMut(Duration, &Cancel) -> Result<(), AiError>,
) -> Result<SignedIn, SignInError> {
    let start = api.start(device_name)?;
    on_code(&start);
    let mut interval = Duration::from_secs(start.interval.clamp(1, 60));
    // Counted in waits, so a test's sleep needn't pass real time.
    let mut left = Duration::from_secs(start.expires_in.max(1));
    loop {
        if cancel.is_cancelled() {
            return Err(AiError::Cancelled.into());
        }
        if left.is_zero() {
            return Err(SignInError::Expired);
        }
        let step = interval.min(left);
        sleep(step, cancel)?;
        left = left.saturating_sub(step);
        match api.poll(&start.device_code) {
            Ok(Poll::Pending) => {}
            Ok(Poll::Ok { token, account }) => return Ok(SignedIn { token, account }),
            Ok(Poll::Denied) => return Err(SignInError::Denied),
            Ok(Poll::Expired) => return Err(SignInError::Expired),
            // Polling too fast: slow down, as OAuth's `slow_down`.
            Err(AiError::RateLimited { retry_after, .. }) => {
                interval =
                    Duration::from_secs(retry_after.unwrap_or(interval.as_secs() + 5).clamp(1, 60));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// The real wait for [`sign_in`]: sleeps in short steps, watching `cancel`.
pub fn sleep_unless_cancelled(d: Duration, cancel: &Cancel) -> Result<(), AiError> {
    let end = Instant::now() + d;
    loop {
        if cancel.is_cancelled() {
            return Err(AiError::Cancelled);
        }
        let now = Instant::now();
        if now >= end {
            return Ok(());
        }
        std::thread::sleep((end - now).min(Duration::from_millis(100)));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io::Cursor;

    use super::*;
    use crate::client::{ChatRequest, Client};
    use crate::provider::{ProviderConfig, ProviderId};
    use crate::transport::Method;

    /// A canned response: status, headers, body.
    type Canned = (u16, Vec<(&'static str, &'static str)>, String);

    fn ok(body: &str) -> Canned {
        (200, Vec::new(), body.to_owned())
    }

    fn st(status: u16, body: &str) -> Canned {
        (status, Vec::new(), body.to_owned())
    }

    fn hd(status: u16, headers: &[(&'static str, &'static str)], body: &str) -> Canned {
        (status, headers.to_vec(), body.to_owned())
    }

    /// Answers each request with the next canned response, recording it.
    #[derive(Default)]
    struct Script {
        replies: Mutex<VecDeque<Canned>>,
        seen: Mutex<Vec<HttpRequest>>,
    }

    impl Script {
        fn new(replies: Vec<Canned>) -> Arc<Self> {
            Arc::new(Self {
                replies: Mutex::new(replies.into()),
                seen: Mutex::default(),
            })
        }

        fn seen(&self) -> Vec<HttpRequest> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl Transport for Script {
        fn send(&self, req: &HttpRequest) -> Result<HttpResponse, AiError> {
            self.seen.lock().unwrap().push(req.clone());
            let (status, headers, body) = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("no more scripted replies");
            let mut r = HttpResponse::new(status, Cursor::new(body.into_bytes()));
            for (n, v) in headers {
                r = r.with_header(n, v);
            }
            Ok(r)
        }
    }

    const START: &str = r#"{"device_code":"dev-1","user_code":"ABCD-EFGH","verification_url":"https://app.paparanai.id/device?code=ABCD-EFGH","interval":5,"expires_in":20}"#;

    fn no_sleep(
        slept: &mut Vec<Duration>,
    ) -> impl FnMut(Duration, &Cancel) -> Result<(), AiError> + '_ {
        move |d, _| {
            slept.push(d);
            Ok(())
        }
    }

    #[test]
    fn base_url_and_override() {
        assert_eq!(api_base_from(None), "https://app.paparanai.id/api/kantor");
        assert_eq!(
            api_base_from(Some("http://127.0.0.1:8787/api/kantor/")),
            "http://127.0.0.1:8787/api/kantor"
        );
        assert_eq!(
            api_base_from(Some("  ")),
            "https://app.paparanai.id/api/kantor"
        );
    }

    #[test]
    fn device_flow_polls_until_ok() {
        let t = Script::new(vec![
            ok(START),
            ok(r#"{"status":"pending"}"#),
            hd(
                429,
                &[("Retry-After", "9")],
                r#"{"error":{"code":"slow_down","message":"slow down"}}"#,
            ),
            ok(r#"{"status":"pending"}"#),
            ok(
                r#"{"status":"ok","token":"tok-secret","account":{"id":42,"email":"a@b.id","name":"Ana"}}"#,
            ),
        ]);
        let api = Paparan::with_base("https://p.example/api/kantor/", t.clone());
        let mut shown = None;
        let mut slept = Vec::new();
        let done = sign_in(
            &api,
            "Kantor on test",
            &Cancel::default(),
            &mut |s| shown = Some(s.user_code.clone()),
            &mut no_sleep(&mut slept),
        )
        .unwrap();
        assert_eq!(shown.as_deref(), Some("ABCD-EFGH"));
        assert_eq!(done.token, "tok-secret");
        assert_eq!(
            done.account,
            Account {
                id: "42".into(),
                email: "a@b.id".into(),
                name: "Ana".into()
            }
        );
        // The 429 slowed polling down to its Retry-After; the last wait
        // is what's left of the code's 20 s.
        assert_eq!(slept, [5, 5, 9, 1].map(Duration::from_secs).to_vec());
        let seen = t.seen();
        assert_eq!(seen[0].url, "https://p.example/api/kantor/device/start");
        let start: Value = serde_json::from_str(&seen[0].body).unwrap();
        assert_eq!(start["client"], "kantor");
        assert_eq!(start["client_version"], CLIENT_VERSION);
        assert_eq!(start["device_name"], "Kantor on test");
        assert_eq!(seen[1].url, "https://p.example/api/kantor/device/token");
        assert_eq!(seen[1].body, r#"{"device_code":"dev-1"}"#);
        assert!(!format!("{:?}", seen).contains("tok-secret"));
    }

    #[test]
    fn device_flow_denied_expired_cancelled() {
        let api = |replies: Vec<Canned>| {
            Paparan::with_base("https://p.example/api/kantor", Script::new(replies))
        };
        let run = |api: &Paparan, cancel: &Cancel| {
            let mut slept = Vec::new();
            sign_in(api, "d", cancel, &mut |_| {}, &mut no_sleep(&mut slept))
        };
        let denied = api(vec![ok(START), ok(r#"{"status":"denied"}"#)]);
        assert_eq!(run(&denied, &Cancel::default()), Err(SignInError::Denied));

        let expired = api(vec![ok(START), ok(r#"{"status":"expired"}"#)]);
        assert_eq!(run(&expired, &Cancel::default()), Err(SignInError::Expired));

        // expires_in 20 at 5 s a poll: four polls, then the code is given up on.
        let pending = r#"{"status":"pending"}"#;
        let ran_out = api(vec![
            ok(START),
            ok(pending),
            ok(pending),
            ok(pending),
            ok(pending),
        ]);
        assert_eq!(run(&ran_out, &Cancel::default()), Err(SignInError::Expired));

        let cancel = Cancel::default();
        let cancelled = api(vec![ok(START), ok(pending)]);
        let mut slept = Vec::new();
        let mut sleep = |d: Duration, c: &Cancel| -> Result<(), AiError> {
            slept.push(d);
            c.cancel();
            Ok(())
        };
        assert_eq!(
            sign_in(&cancelled, "d", &cancel, &mut |_| {}, &mut sleep),
            Err(SignInError::Ai(AiError::Cancelled))
        );

        let refused = api(vec![st(
            503,
            r#"{"error":{"code":"unavailable","message":"Down for maintenance"}}"#,
        )]);
        assert_eq!(
            run(&refused, &Cancel::default()),
            Err(SignInError::Ai(AiError::Http {
                status: 503,
                message: "Down for maintenance".into()
            }))
        );
    }

    #[test]
    fn me_and_revoke() {
        let t = Script::new(vec![
            ok(
                r#"{"account":{"id":"u1","email":"a@b.id","name":"Ana"},"plan":{"name":"Starter"},"credits":{"remaining":1250,"unit":"credits","low":false},"topup_url":"https://app.paparanai.id/topup","manage_url":"https://app.paparanai.id/account"}"#,
            ),
            ok(""),
        ]);
        let api = Paparan::with_base("https://p.example/api/kantor", t.clone());
        let me = api.me("tok").unwrap();
        assert_eq!(me.account.email, "a@b.id");
        assert_eq!(me.plan, "Starter");
        assert_eq!(me.credits.display(), "1,250 credits");
        assert!(!me.credits.low);
        assert_eq!(
            me.topup_url.as_deref(),
            Some("https://app.paparanai.id/topup")
        );
        api.revoke("tok").unwrap();
        let seen = t.seen();
        assert_eq!(seen[0].method, Method::Get);
        assert_eq!(seen[0].url, "https://p.example/api/kantor/me");
        assert_eq!(seen[1].url, "https://p.example/api/kantor/device/revoke");
        for r in &seen {
            assert!(r.headers.contains(&("authorization", "Bearer tok".into())));
        }
        // A plan sent as a plain string.
        let me =
            Me::from_json(&json!({"plan": "Free", "credits": {"remaining": "3.5", "low": true}}));
        assert_eq!(me.plan, "Free");
        assert_eq!(me.credits.display(), "3.5");
        assert!(me.credits.low);
    }

    #[test]
    fn amounts() {
        assert_eq!(format_amount(0.0), "0");
        assert_eq!(format_amount(999.0), "999");
        assert_eq!(format_amount(1000.0), "1,000");
        assert_eq!(format_amount(1234567.0), "1,234,567");
        assert_eq!(format_amount(12.25), "12.25");
        assert_eq!(format_amount(-5.0), "-5");
    }

    fn paparan_client(t: Arc<Script>) -> Client {
        let cfg = ProviderConfig {
            provider: ProviderId::Paparan,
            model: DEFAULT_MODEL.into(),
            base_url: "https://p.example/api/kantor/v1".into(),
            api_key: Some("tok".into()),
        };
        Client::new(cfg, t)
    }

    const OK_STREAM: &str =
        "data: {\"choices\":[{\"delta\":{\"content\":\"Halo\"}}]}\n\ndata: [DONE]\n\n";

    #[test]
    fn chat_request_and_credits_header() {
        let t = Script::new(vec![hd(
            200,
            &[
                ("X-Paparan-Credits-Remaining", "97.5"),
                ("X-Paparan-Credits-Low", "true"),
            ],
            OK_STREAM,
        )]);
        let client = paparan_client(t.clone());
        let req = ChatRequest::new("sys", "hi").task("rewrite");
        let _ = take_credits();
        assert_eq!(
            client
                .complete(&req, &Cancel::default(), &mut |_| {})
                .unwrap(),
            "Halo"
        );
        assert_eq!(
            take_credits(),
            Some(CreditsSeen {
                remaining: 97.5,
                low: Some(true)
            })
        );
        assert_eq!(take_credits(), None, "taken once");
        let seen = t.seen();
        assert_eq!(
            seen[0].url,
            "https://p.example/api/kantor/v1/chat/completions"
        );
        assert!(
            seen[0]
                .headers
                .contains(&("authorization", "Bearer tok".into()))
        );
        let body: Value = serde_json::from_str(&seen[0].body).unwrap();
        assert_eq!(body["paparan_task"], "rewrite");
        assert_eq!(body["model"], "paparan-auto");
        assert_eq!(body["stream"], true);

        // Other providers never get the task.
        let other = crate::client::build_request(
            &ProviderConfig {
                provider: ProviderId::OpenAi,
                model: "m".into(),
                base_url: "https://x/v1".into(),
                api_key: Some("k".into()),
            },
            &req,
        );
        let body: Value = serde_json::from_str(&other.body).unwrap();
        assert!(body.get("paparan_task").is_none());
    }

    #[test]
    fn credit_header_parsing() {
        let r = |h: &[(&str, &str)]| {
            let mut r = HttpResponse::new(200, std::io::empty());
            for (n, v) in h {
                r = r.with_header(n, v);
            }
            credits_from(&r)
        };
        assert_eq!(r(&[]), None);
        assert_eq!(r(&[("x-paparan-credits-remaining", "abc")]), None);
        assert_eq!(
            r(&[("X-Paparan-Credits-Remaining", " 12 ")]),
            Some(CreditsSeen {
                remaining: 12.0,
                low: None
            })
        );
        assert_eq!(
            r(&[
                ("x-paparan-credits-remaining", "0"),
                ("x-paparan-credits-low", "1")
            ]),
            Some(CreditsSeen {
                remaining: 0.0,
                low: Some(true)
            })
        );
    }

    #[test]
    fn status_mapping() {
        let err = |status: u16, headers: &[(&'static str, &'static str)], body: &str| {
            let t = Script::new(vec![hd(status, headers, body)]);
            paparan_client(t)
                .complete(&ChatRequest::new("s", "u"), &Cancel::default(), &mut |_| {})
                .unwrap_err()
        };
        assert_eq!(
            err(
                402,
                &[],
                r#"{"error":{"code":"insufficient_credits","message":"Saldo habis","topup_url":"https://app.paparanai.id/topup"}}"#
            ),
            AiError::InsufficientCredits {
                message: "Saldo habis".into(),
                topup_url: Some("https://app.paparanai.id/topup".into())
            }
        );
        assert_eq!(
            err(
                401,
                &[],
                r#"{"error":{"code":"unauthorized","message":"bad token"}}"#
            ),
            AiError::SignedOut
        );
        assert_eq!(
            err(
                429,
                &[("Retry-After", "120")],
                r#"{"error":{"code":"rate_limited","message":"Slow down"}}"#
            ),
            AiError::RateLimited {
                retry_after: Some(120),
                message: "Slow down".into()
            }
        );
        assert_eq!(
            err(500, &[], "Internal error"),
            AiError::Http {
                status: 500,
                message: "Internal error".into()
            }
        );
        // An error sent mid-stream.
        let t = Script::new(vec![ok(
            "data: {\"error\":{\"code\":\"insufficient_credits\",\"message\":\"Out\"}}\n\n",
        )]);
        assert!(matches!(
            paparan_client(t).complete(
                &ChatRequest::new("s", "u"),
                &Cancel::default(),
                &mut |_| {}
            ),
            Err(AiError::InsufficientCredits {
                topup_url: None,
                ..
            })
        ));
    }

    #[test]
    fn short_retry_after_is_waited_out_once() {
        let t = Script::new(vec![
            hd(
                429,
                &[("retry-after", "0")],
                r#"{"error":{"code":"rate_limited","message":"busy"}}"#,
            ),
            ok(OK_STREAM),
        ]);
        let client = paparan_client(t.clone());
        assert_eq!(
            client
                .complete(&ChatRequest::new("s", "u"), &Cancel::default(), &mut |_| {})
                .unwrap(),
            "Halo"
        );
        assert_eq!(t.seen().len(), 2);

        let t = Script::new(vec![
            hd(429, &[("retry-after", "0")], "{}"),
            hd(429, &[("retry-after", "0")], "{}"),
        ]);
        assert!(matches!(
            paparan_client(t.clone()).complete(
                &ChatRequest::new("s", "u"),
                &Cancel::default(),
                &mut |_| {}
            ),
            Err(AiError::RateLimited {
                retry_after: Some(0),
                ..
            })
        ));
        assert_eq!(t.seen().len(), 2, "retried only once");
    }
}
