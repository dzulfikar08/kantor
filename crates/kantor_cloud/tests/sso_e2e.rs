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
//! cargo test -p kantor_cloud --test sso_e2e -- --ignored --nocapture
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
