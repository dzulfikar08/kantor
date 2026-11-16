//! Single sign-on for cloud accounts (the workspace side of
//! `kantor_cloud::oidc`): signing in from Cloud Accounts, building clients
//! that send bearer tokens, and telling the user when a sign-in has ended.
//! The refresh token is kept in the keychain under the account id, like an
//! app password (`kantor_ai::keys::CLOUD`, same file fallback).

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use kantor_ai::keys::{CLOUD, KeyLocation};
use kantor_cloud::oidc::{self, DonePage, TokenSource};
use kantor_cloud::session::Remote;
use kantor_cloud::{Auth, Client, Error};
use kantor_core::settings::CloudAccount;
use kantor_ui::i18n::{t, tf};

/// What a sign-in in progress asks the UI to do.
pub enum SsoStep {
    /// Open this address in the system browser.
    OpenBrowser(String),
    /// Show the device code: go to `uri`, enter `code` (`open`: the address
    /// with the code filled in, opened in the browser too).
    DeviceCode {
        uri: String,
        code: String,
        open: Option<String>,
    },
}

/// A sign-in's request: the drive, and what the user typed.
pub struct SsoRequest {
    pub server: String,
    pub issuer: String,
    pub client_id: String,
    /// Use the device code instead of the browser on this computer.
    pub device: bool,
    /// Signing in again to this existing account.
    pub again: Option<CloudAccount>,
}

/// The server's address as typed, with a scheme.
pub fn normalize_server(server: &str) -> String {
    let s = server.trim().trim_end_matches('/');
    if s.starts_with("http://") || s.starts_with("https://") {
        s.to_owned()
    } else {
        format!("https://{s}")
    }
}

/// The drive's origin (`https://drive.example.go.id`) from its address.
fn origin(server: &str) -> String {
    let s = normalize_server(server);
    for marker in ["/remote.php/", "/dav/", "/webdav"] {
        if let Some(i) = s.find(marker) {
            return s[..i].to_owned();
        }
    }
    s
}

fn done_page() -> DonePage {
    DonePage {
        title: "Kantor".into(),
        ok: t("You're signed in. You can close this tab and go back to Kantor.").into(),
        failed: t("Sign-in didn't finish. Go back to Kantor and try again.").into(),
    }
}

fn describe(e: &Error) -> String {
    match e {
        Error::Auth(_) => t(
            "The drive refused the sign-in. Ask your administrator whether Kantor may use single sign-on on this drive.",
        )
        .into(),
        Error::NotFound => t(
            "This drive doesn't say where to sign in. Enter your organisation's sign-in server.",
        )
        .into(),
        e => tf("Sign-in failed: {}", &[&e.to_string()]),
    }
}

/// Sign in and make the account (worker thread). Returns the account and
/// where its refresh token was stored.
pub fn sign_in(
    req: SsoRequest,
    cancel: &AtomicBool,
    step: &dyn Fn(SsoStep),
) -> Result<(CloudAccount, KeyLocation), String> {
    let server = normalize_server(&req.server);
    let origin = origin(&server);
    // Where to sign in: the issuer typed, else what the drive says.
    let (endpoints, suggested) = if req.issuer.trim().is_empty() {
        let d = oidc::discover_server(&origin).map_err(|e| describe(&e))?;
        (d.endpoints, d.client_id)
    } else {
        let issuer = normalize_server(&req.issuer);
        (
            oidc::discover_issuer(&issuer).map_err(|e| describe(&e))?,
            None,
        )
    };
    let client_id = Some(req.client_id.trim().to_owned())
        .filter(|c| !c.is_empty())
        .or(suggested)
        .unwrap_or_else(|| oidc::DEFAULT_CLIENT_ID.to_owned());
    let tokens = if req.device {
        let show = |start: &oidc::DeviceStart| {
            step(SsoStep::DeviceCode {
                uri: start.verification_uri.clone(),
                code: start.user_code.clone(),
                open: start.verification_uri_complete.clone(),
            })
        };
        oidc::sign_in_device(&endpoints, &client_id, &show, cancel).map_err(|e| match e {
            Error::NotFound => t(
                "This sign-in server has no sign-in with a code. Use the browser sign-in instead.",
            )
            .to_owned(),
            e => describe(&e),
        })?
    } else {
        let open = |url: &str| step(SsoStep::OpenBrowser(url.to_owned()));
        oidc::sign_in_browser(&endpoints, &client_id, &open, cancel, &done_page())
            .map_err(|e| describe(&e))?
    };
    let refresh = tokens.refresh_token.clone().ok_or_else(|| {
        t("The sign-in server gave no refresh token, so Kantor can't stay signed in. Ask your administrator to allow offline access for Kantor.")
            .to_owned()
    })?;
    // Who the drive says this is, and its WebDAV root.
    let (user, display) = oidc::whoami(&origin, &tokens.access_token).map_err(|e| describe(&e))?;
    let claims = tokens.id_token.as_deref().and_then(oidc::jwt_claims);
    let login = claims
        .as_ref()
        .and_then(|c| c["preferred_username"].as_str().map(str::to_owned))
        .or(display)
        .unwrap_or_else(|| user.clone());
    let account = match req.again {
        Some(mut a) => {
            a.user = user;
            a.issuer = endpoints.issuer.clone();
            a.client_id = client_id.clone();
            a.name = if login == a.user {
                String::new()
            } else {
                login
            };
            a
        }
        None => CloudAccount {
            id: super::accounts::account_id(&origin, &user),
            server: origin.clone(),
            name: if login == user { String::new() } else { login },
            user,
            auth: "oidc".into(),
            issuer: endpoints.issuer.clone(),
            client_id: client_id.clone(),
            dav_url: String::new(),
        },
    };
    // Check the drive takes the token for files too, before keeping it.
    let id = account.id.clone();
    let source = TokenSource::new(endpoints, client_id, refresh.clone(), {
        let id = id.clone();
        move |r| {
            let _ = CLOUD.save(&id, r);
        }
    })
    .with_access(&tokens);
    let source = oidc::replace_source(&id, source);
    let client = Client::new(&account.dav_root(), Auth::Source(source));
    client.list("").map_err(|e| describe(&e))?;
    let location = CLOUD.save(&id, &refresh)?;
    Ok((account, location))
}

/// A client for a single sign-on account (no network: tokens are fetched
/// on the first request). Worker thread (the keychain may block).
pub fn remote(account: &CloudAccount) -> Result<Remote, String> {
    let source = oidc::shared_source(&account.id, || {
        let refresh = CLOUD.load(&account.id).ok_or(Error::Auth(401))?;
        let id = account.id.clone();
        Ok(TokenSource::lazy(
            account.issuer.clone(),
            client_id(account),
            refresh,
            move |r| {
                let _ = CLOUD.save(&id, r);
            },
        ))
    })
    .map_err(|_| ended_message(account))?;
    if source.needs_sign_in() {
        return Err(ended_message(account));
    }
    Ok(Remote {
        client: Client::new(&account.dav_root(), Auth::Source(source)),
        account: account.id.clone(),
        owner: format!(
            "{} (Kantor)",
            if account.name.is_empty() {
                &account.user
            } else {
                &account.name
            }
        ),
    })
}

fn client_id(account: &CloudAccount) -> String {
    if account.client_id.is_empty() {
        oidc::DEFAULT_CLIENT_ID.to_owned()
    } else {
        account.client_id.clone()
    }
}

/// Whether the account's sign-in has ended this run (the provider refused
/// its refresh token).
pub fn ended(account: &CloudAccount) -> bool {
    account.is_sso() && oidc::source_of(&account.id).is_some_and(|s| s.needs_sign_in())
}

/// What to tell the user when the sign-in has ended.
pub fn ended_message(account: &CloudAccount) -> String {
    tf(
        "Your sign-in to {} has ended. Your edits are kept on this computer. Sign in again in Cloud Accounts, then choose Sync Now.",
        &[&account.label()],
    )
}

/// Forget a removed account's sign-in: tell the provider (best effort) and
/// drop the token source (worker thread).
pub fn sign_out(account: &CloudAccount) {
    if let Some(source) = oidc::source_of(&account.id)
        && let Ok(ep) = source.endpoints()
    {
        oidc::revoke(&ep, source.client_id(), &source.refresh_token());
    } else if let Some(refresh) = CLOUD.load(&account.id)
        && let Ok(ep) = oidc::endpoints_cached(&account.issuer)
    {
        oidc::revoke(&ep, &client_id(account), &refresh);
    }
    oidc::forget_source(&account.id);
}

/// A shared cancel flag for a sign-in in progress.
pub type Cancel = Arc<AtomicBool>;

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn server_addresses() {
        assert_eq!(
            normalize_server("drive.example.go.id/"),
            "https://drive.example.go.id"
        );
        assert_eq!(
            origin("https://d.example/remote.php/dav/files/budi/"),
            "https://d.example"
        );
        assert_eq!(origin("http://127.0.0.1:8080"), "http://127.0.0.1:8080");
    }
}
