//! Single sign-on against a real identity provider and drive (Keycloak +
//! Nextcloud with `user_oidc` bearer validation, or OpenCloud). Ignored by
//! default; run with:
//!
//! ```text
//! KANTOR_SSO_SERVER    the drive (http://172.17.0.1:28481)
//! KANTOR_SSO_ISSUER    the issuer, when the drive doesn't publish it (Nextcloud)
//! KANTOR_SSO_CLIENT    client id (default: what discovery suggests, else `kantor`)
//! KANTOR_SSO_USER / _PASS   an account at the provider
//! KANTOR_SSO_BROWSER   a command run as `<cmd> <url> <user> <pass>` that signs in
//!                      in a (headless) browser and leaves it at the redirect
//! cargo test -p kantor_cloud --test sso_e2e -- --ignored --nocapture --test-threads 1
//! ```
//!
//! "Sign in again" tests (Keycloak only) also need:
//!
//! ```text
//! KANTOR_SSO_KC_ADMIN       admin:password of Keycloak's master realm (counts sessions)
//! KANTOR_SSO_SHARED_BROWSER like KANTOR_SSO_BROWSER, but every sign-in goes through one
//!                           browser that stays open (its SSO cookie is kept), as a
//!                           user's browser does; the shared-session test is skipped without it
//! ```

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use kantor_cloud::oidc::{self, BearerSource, DonePage, TokenSource};
use kantor_cloud::{Auth, Client, Error, Precondition};

struct Env {
    server: String,
    issuer: Option<String>,
    client: Option<String>,
    user: String,
    pass: String,
    browser: String,
}

fn env() -> Option<Env> {
    let v = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
    Some(Env {
        server: v("KANTOR_SSO_SERVER")?,
        issuer: v("KANTOR_SSO_ISSUER"),
        client: v("KANTOR_SSO_CLIENT"),
        user: v("KANTOR_SSO_USER")?,
        pass: v("KANTOR_SSO_PASS")?,
        browser: v("KANTOR_SSO_BROWSER")?,
    })
}

fn browse(e: &Env, url: &str) {
    let mut parts = e.browser.split_whitespace();
    let out = std::process::Command::new(parts.next().unwrap())
        .args(parts)
        .args([url, &e.user, &e.pass])
        .output()
        .expect("browser helper");
    eprintln!("browser: {}", String::from_utf8_lossy(&out.stdout).trim());
}

fn endpoints(e: &Env) -> (oidc::Endpoints, String) {
    let (ep, suggested) = match &e.issuer {
        Some(i) => (oidc::discover_issuer(i).unwrap(), None),
        None => {
            let d = oidc::discover_server(&e.server).unwrap();
            (d.endpoints, d.client_id)
        }
    };
    let client = e
        .client
        .clone()
        .or(suggested)
        .unwrap_or_else(|| oidc::DEFAULT_CLIENT_ID.into());
    (ep, client)
}

/// Sign in, find the user, then list, upload and download over WebDAV
/// with bearer tokens that the source refreshes.
fn use_the_drive(e: &Env, ep: oidc::Endpoints, client: String, tokens: oidc::Tokens) {
    let refresh = tokens.refresh_token.clone().expect("a refresh token");
    let (id, name) = oidc::whoami(&e.server, &tokens.access_token).expect("whoami");
    eprintln!("signed in as {id} ({name:?})");
    let stored = Arc::new(Mutex::new(None::<String>));
    let keep = stored.clone();
    let source = Arc::new(
        TokenSource::new(ep, client, refresh, move |r| {
            *keep.lock().unwrap() = Some(r.into())
        })
        .with_access(&tokens),
    );
    let base = format!(
        "{}/remote.php/dav/files/{id}/",
        e.server.trim_end_matches('/')
    );
    let c = Client::new(&base, Auth::Source(source.clone()));
    c.list("").expect("listing with a bearer token");
    let name = format!("kantor-sso-{}.txt", std::process::id());
    c.put(&name, b"hello", Precondition::IfNoneMatch, None)
        .expect("upload with a bearer token");
    assert_eq!(c.get(&name).expect("download").bytes, b"hello");
    // Force a refresh: drop the access token as if the server refused it.
    let t1 = source.token().unwrap();
    source.rejected(&t1);
    let t2 = source.token().unwrap();
    assert_ne!(t1, t2, "a refreshed access token");
    c.list("").expect("listing after a refresh");
    let _ = c.delete(&name);
    // A made-up refresh token: the provider refuses it, the sign-in ends.
    let (ep2, client2) = endpoints(e);
    let bad = Arc::new(TokenSource::new(ep2, client2, "not-a-token".into(), |_| {}));
    let c2 = Client::new(&base, Auth::Source(bad.clone()));
    assert!(matches!(c2.list(""), Err(Error::Auth(_))));
    assert!(bad.needs_sign_in());
}

#[test]
#[ignore]
fn browser_sign_in_then_webdav() {
    let Some(e) = env() else {
        eprintln!("KANTOR_SSO_* not set: skipped");
        return;
    };
    let (ep, client) = endpoints(&e);
    let cancel = AtomicBool::new(false);
    let tokens = oidc::sign_in_browser(
        &ep,
        &client,
        &|url| {
            let url = url.to_owned();
            let e2 = Env {
                browser: e.browser.clone(),
                user: e.user.clone(),
                pass: e.pass.clone(),
                server: String::new(),
                issuer: None,
                client: None,
            };
            std::thread::spawn(move || browse(&e2, &url));
        },
        &cancel,
        &DonePage::default(),
    )
    .expect("browser sign-in");
    use_the_drive(&e, ep, client, tokens);
}

#[test]
#[ignore]
fn device_sign_in_then_webdav() {
    let Some(e) = env() else {
        eprintln!("KANTOR_SSO_* not set: skipped");
        return;
    };
    let (ep, client) = endpoints(&e);
    if ep.device_authorization_endpoint.is_none() {
        eprintln!("the provider has no device endpoint: skipped");
        return;
    }
    let (browser, user, pass) = (e.browser.clone(), e.user.clone(), e.pass.clone());
    let show = move |start: &oidc::DeviceStart| {
        eprintln!("code {} at {}", start.user_code, start.verification_uri);
        let url = start
            .verification_uri_complete
            .clone()
            .unwrap_or_else(|| start.verification_uri.clone());
        let e2 = Env {
            browser: browser.clone(),
            user: user.clone(),
            pass: pass.clone(),
            server: String::new(),
            issuer: None,
            client: None,
        };
        std::thread::spawn(move || browse(&e2, &url));
    };
    let cancel = AtomicBool::new(false);
    let tokens = oidc::sign_in_device(&ep, &client, &show, &cancel).expect("device sign-in");
    use_the_drive(&e, ep, client, tokens);
}

// ── Sign in again ─────────────────────────────────────────────────────────

/// Keycloak's admin API for the test realm: counts the user's sessions.
struct KcAdmin {
    base: String,
    realm: String,
    user: String,
    pass: String,
}

impl KcAdmin {
    /// From an issuer like `http://host/realms/pemda` and KANTOR_SSO_KC_ADMIN.
    fn new(issuer: &str) -> Option<Self> {
        let (user, pass) = std::env::var("KANTOR_SSO_KC_ADMIN")
            .ok()?
            .split_once(':')
            .map(|(u, p)| (u.to_owned(), p.to_owned()))?;
        let (base, realm) = issuer.trim_end_matches('/').split_once("/realms/")?;
        Some(Self {
            base: base.to_owned(),
            realm: realm.to_owned(),
            user,
            pass,
        })
    }

    fn agent() -> ureq::Agent {
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into()
    }

    fn token(&self) -> String {
        let body = oidc::form_body(&[
            ("grant_type", "password"),
            ("client_id", "admin-cli"),
            ("username", &self.user),
            ("password", &self.pass),
        ]);
        let mut r = Self::agent()
            .post(&format!(
                "{}/realms/master/protocol/openid-connect/token",
                self.base
            ))
            .header("content-type", "application/x-www-form-urlencoded")
            .send(body.as_bytes())
            .unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&r.body_mut().read_to_string().unwrap()).unwrap();
        v["access_token"].as_str().expect("admin token").to_owned()
    }

    fn call(&self, method: &str, path: &str) -> serde_json::Value {
        let url = format!("{}/admin/realms/{}/{path}", self.base, self.realm);
        let auth = format!("Bearer {}", self.token());
        let a = Self::agent();
        let mut r = match method {
            "GET" => a.get(&url).header("authorization", &auth).call(),
            "POST" => a.post(&url).header("authorization", &auth).send_empty(),
            _ => a.delete(&url).header("authorization", &auth).call(),
        }
        .unwrap();
        let text = r.body_mut().read_to_string().unwrap_or_default();
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
    }

    fn ids(&self, user: &str, client: &str) -> (String, String) {
        let u = self.call("GET", &format!("users?username={user}&exact=true"));
        let c = self.call("GET", &format!("clients?clientId={client}"));
        (
            u[0]["id"].as_str().expect("user").to_owned(),
            c[0]["id"].as_str().expect("client").to_owned(),
        )
    }

    /// End all of the user's sessions, offline ones too (their consent).
    fn sign_out_everywhere(&self, user: &str, client: &str) {
        let (uid, _) = self.ids(user, client);
        self.call("POST", &format!("users/{uid}/logout"));
        self.call("DELETE", &format!("users/{uid}/consents/{client}"));
    }

    /// The user's sessions with `client` (browser and offline), by id.
    fn sessions(&self, user: &str, client: &str) -> Vec<String> {
        let (uid, cid) = self.ids(user, client);
        let mut ids = Vec::new();
        for s in [
            self.call("GET", &format!("users/{uid}/sessions")),
            self.call("GET", &format!("users/{uid}/offline-sessions/{cid}")),
        ] {
            for s in s.as_array().into_iter().flatten() {
                let with_client = s["clients"]
                    .as_object()
                    .is_some_and(|c| c.values().any(|v| v == client));
                if with_client && let Some(id) = s["id"].as_str() {
                    ids.push(id.to_owned());
                }
            }
        }
        ids
    }
}

/// A browser sign-in through `browser` (a helper command) asking for `scope`.
fn sign_in_with(
    e: &Env,
    browser: &str,
    ep: &oidc::Endpoints,
    client: &str,
    scope: &str,
) -> oidc::Tokens {
    let cancel = AtomicBool::new(false);
    let (browser, user, pass) = (browser.to_owned(), e.user.clone(), e.pass.clone());
    oidc::browser_once(
        ep,
        client,
        &|url| {
            let e2 = Env {
                browser: browser.clone(),
                user: user.clone(),
                pass: pass.clone(),
                server: String::new(),
                issuer: None,
                client: None,
            };
            let url = url.to_owned();
            std::thread::spawn(move || browse(&e2, &url));
        },
        &cancel,
        &DonePage::default(),
        scope,
    )
    .expect("browser sign-in")
}

/// The drive takes the new sign-in: who it is, and its files.
fn drive_works(e: &Env, ep: &oidc::Endpoints, client: &str, t: &oidc::Tokens) {
    let fresh = oidc::refresh(ep, client, t.refresh_token.as_deref().unwrap())
        .expect("the new sign-in refreshes");
    let (id, _) = oidc::whoami(&e.server, &fresh.access_token).expect("whoami");
    let base = format!(
        "{}/remote.php/dav/files/{id}/",
        e.server.trim_end_matches('/')
    );
    Client::new(&base, Auth::Bearer(fresh.access_token))
        .list("")
        .expect("listing with the new sign-in");
}

fn keycloak(e: &Env) -> Option<(KcAdmin, oidc::Endpoints, String)> {
    let (ep, client) = endpoints(e);
    let Some(admin) = KcAdmin::new(&ep.issuer) else {
        eprintln!("not Keycloak, or KANTOR_SSO_KC_ADMIN not set: skipped");
        return None;
    };
    admin.sign_out_everywhere(&e.user, &client);
    assert!(admin.sessions(&e.user, &client).is_empty());
    Some((admin, ep, client))
}

/// "Sign in again" from another browser (or after the browser was closed):
/// the old sign-in's session is revoked, the new one works, and Keycloak
/// shows one Kantor session.
#[test]
#[ignore]
fn sign_in_again_ends_the_old_session() {
    let Some(e) = env() else {
        eprintln!("KANTOR_SSO_* not set: skipped");
        return;
    };
    let Some((admin, ep, client)) = keycloak(&e) else {
        return;
    };
    let old = sign_in_with(&e, &e.browser, &ep, &client, oidc::SCOPE);
    let new = sign_in_with(&e, &e.browser, &ep, &client, oidc::SCOPE);
    let before = admin.sessions(&e.user, &client);
    eprintln!("sessions after two sign-ins: {before:?}");
    assert_eq!(before.len(), 2, "each sign-in has its own offline session");
    let old_refresh = old.refresh_token.clone().unwrap();
    assert_eq!(
        oidc::retire_replaced(&ep, &client, &old_refresh, &new),
        oidc::Replaced::Revoked
    );
    let after = admin.sessions(&e.user, &client);
    eprintln!("sessions after Sign in again: {after:?}");
    assert_eq!(after.len(), 1, "exactly one Kantor session left");
    assert_eq!(
        after[0],
        oidc::session_id(new.refresh_token.as_deref().unwrap()).unwrap()
    );
    assert!(
        matches!(
            oidc::refresh(&ep, &client, &old_refresh),
            Err(Error::Auth(_))
        ),
        "the old refresh token is no longer valid"
    );
    drive_works(&e, &ep, &client, &new);
}

/// "Sign in again" in a browser that is still signed in, with session
/// tokens (the provider gives no offline tokens): the new sign-in shares
/// the old one's session, so the old token is kept, and the new sign-in
/// keeps working. Revoking the old token would have ended it (checked last).
#[test]
#[ignore]
fn sign_in_again_in_a_signed_in_browser_keeps_the_new_session() {
    let Some(e) = env() else {
        eprintln!("KANTOR_SSO_* not set: skipped");
        return;
    };
    let Some(shared) = std::env::var("KANTOR_SSO_SHARED_BROWSER")
        .ok()
        .filter(|s| !s.is_empty())
    else {
        eprintln!("KANTOR_SSO_SHARED_BROWSER not set: skipped");
        return;
    };
    let Some((admin, ep, client)) = keycloak(&e) else {
        return;
    };
    let old = sign_in_with(&e, &shared, &ep, &client, oidc::SCOPE_ONLINE);
    let new = sign_in_with(&e, &shared, &ep, &client, oidc::SCOPE_ONLINE);
    let old_refresh = old.refresh_token.clone().unwrap();
    assert_eq!(
        oidc::session_id(&old_refresh),
        oidc::session_id(new.refresh_token.as_deref().unwrap()),
        "the browser's session was reused"
    );
    assert_eq!(
        oidc::retire_replaced(&ep, &client, &old_refresh, &new),
        oidc::Replaced::SameSession
    );
    let after = admin.sessions(&e.user, &client);
    eprintln!("sessions after Sign in again: {after:?}");
    assert_eq!(after.len(), 1, "exactly one Kantor session");
    drive_works(&e, &ep, &client, &new);
    // Why: revoking the old token ends the shared session, new sign-in included.
    oidc::revoke(&ep, &client, &old_refresh).unwrap();
    assert!(matches!(
        oidc::refresh(&ep, &client, new.refresh_token.as_deref().unwrap()),
        Err(Error::Auth(_))
    ));
    admin.sign_out_everywhere(&e.user, &client);
}
