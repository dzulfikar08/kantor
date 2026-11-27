//! OneDrive and Google Drive accounts (the workspace side of
//! `kantor_cloud::oauth`): signing in from Cloud Accounts and building
//! clients. Like single sign-on, the refresh token is kept in the keychain
//! under the account id (`kantor_ai::keys::CLOUD`), never in settings.

use std::sync::atomic::AtomicBool;

use kantor_ai::keys::{CLOUD, KeyLocation};
use kantor_cloud::oauth::{self, Provider};
use kantor_cloud::oidc::{self, TokenSource};
use kantor_cloud::session::Remote;
use kantor_cloud::{Auth, Client, Error, gdrive, graph};
use kantor_core::settings::CloudAccount;
use kantor_ui::i18n::{t, tf};

use super::sso::{self, SsoStep};

/// The account's provider.
pub fn provider(account: &CloudAccount) -> Option<Provider> {
    Provider::from_id(&account.provider)
}

/// The providers this build can sign in to (a client id is set).
pub fn available() -> Vec<Provider> {
    Provider::ALL
        .into_iter()
        .filter(|p| p.available())
        .collect()
}

fn not_built(p: Provider) -> String {
    tf("This copy of Kantor can't sign in to {}.", &[&p.name()])
}

fn describe(p: Provider, e: &Error) -> String {
    match e {
        Error::Auth(_) => tf("{} refused the sign-in. Try again.", &[&p.name()]),
        e => tf("Sign-in failed: {}", &[&e.to_string()]),
    }
}

/// A client for `provider` with `auth`.
fn client(p: Provider, account_id: &str, auth: Auth) -> Client {
    match p {
        Provider::OneDrive => Client::onedrive(auth),
        Provider::GoogleDrive => Client::google_drive(auth, gdrive::shared_ids(account_id)),
    }
}

/// A safe id for the account (`onedrive-budi_contoso.com`).
fn account_id(p: Provider, login: &str) -> String {
    let login: String = login
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-._".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{}-{login}", p.id())
}

fn persist(id: &str) -> impl Fn(&str) + Send + Sync + 'static {
    let id = id.to_owned();
    move |r| {
        let _ = CLOUD.save(&id, r);
    }
}

/// Sign in with `p` in the browser and make the account (worker thread).
/// `again`: the account being signed in again.
pub fn sign_in(
    p: Provider,
    again: Option<CloudAccount>,
    cancel: &AtomicBool,
    step: &dyn Fn(SsoStep),
) -> Result<(CloudAccount, KeyLocation), String> {
    let app = p.app().ok_or_else(|| not_built(p))?;
    let ep = p.endpoints();
    let open = |url: &str| step(SsoStep::OpenBrowser(url.to_owned()));
    let tokens = oauth::sign_in(p, &app, &ep, &open, cancel, &sso::done_page())
        .map_err(|e| describe(p, &e))?;
    let refresh = tokens.refresh_token.clone().ok_or_else(|| {
        tf(
            "{} gave no refresh token, so Kantor can't stay signed in. Try again.",
            &[&p.name()],
        )
    })?;
    step(SsoStep::Progress(
        t("Signed in. Opening your drive…").into(),
    ));
    // Who this is.
    let probe = client(p, "", Auth::Bearer(tokens.access_token.clone()));
    let (_, login, name) = match p {
        Provider::OneDrive => graph::whoami_with(&probe, graph::ME),
        Provider::GoogleDrive => gdrive::whoami(&probe),
    }
    .map_err(|e| describe(p, &e))?;
    let id = account_id(p, &login);
    let mut account = again.unwrap_or_default();
    account.id = id.clone();
    account.server = String::new();
    account.dav_url = String::new();
    account.user = login;
    account.name = name;
    account.auth = "oauth".into();
    account.provider = p.id().into();
    account.issuer = ep.issuer.clone();
    account.client_id = app.client_id.clone();
    let source = TokenSource::new(ep, app.client_id.clone(), refresh.clone(), persist(&id))
        .with_client_secret(app.client_secret.clone())
        .with_access(&tokens);
    let source = oidc::replace_source(&id, source);
    // The drive takes the token for files too.
    client(p, &id, Auth::Source(source))
        .list("")
        .map_err(|e| describe(p, &e))?;
    step(SsoStep::Progress(
        t("Saving the sign-in in the system keychain… If your computer asks for the keyring password, enter it.").into(),
    ));
    let location = CLOUD.save(&id, &refresh)?;
    Ok((account, location))
}

/// A client for a OneDrive or Google Drive account (no network: the token
/// is refreshed at the first request). Worker thread (the keychain may
/// block).
pub fn remote(account: &CloudAccount) -> Result<Remote, String> {
    let p = provider(account)
        .ok_or_else(|| t("This account's drive isn't known to this copy of Kantor.").to_owned())?;
    let app = p.app().ok_or_else(|| not_built(p))?;
    let source = oidc::shared_source(&account.id, || {
        let refresh = CLOUD.load(&account.id).ok_or(Error::Auth(401))?;
        Ok(TokenSource::new(
            p.endpoints(),
            app.client_id.clone(),
            refresh,
            persist(&account.id),
        )
        .with_client_secret(app.client_secret.clone()))
    })
    .map_err(|_| sso::ended_message(account))?;
    if source.needs_sign_in() {
        return Err(sso::ended_message(account));
    }
    Ok(Remote {
        client: client(p, &account.id, Auth::Source(source)),
        account: account.id.clone(),
        owner: format!("{} (Kantor)", account.user),
    })
}

/// Forget a removed account's sign-in: revoke it where the provider
/// allows (Google), drop the token source (worker thread).
pub fn sign_out(account: &CloudAccount) {
    if let Some(p) = provider(account) {
        let refresh = oidc::source_of(&account.id)
            .map(|s| s.refresh_token())
            .or_else(|| CLOUD.load(&account.id));
        if let Some(refresh) = refresh {
            let _ = oidc::revoke(&p.endpoints(), &account.client_id, &refresh);
        }
    }
    oidc::forget_source(&account.id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn account_ids_are_safe_names() {
        assert_eq!(
            account_id(Provider::OneDrive, "budi@contoso.com"),
            "onedrive-budi_contoso.com"
        );
        assert_eq!(account_id(Provider::GoogleDrive, "a b@x"), "gdrive-a_b_x");
    }
}
