//! Signing in to Microsoft OneDrive and Google Drive: OAuth 2.0
//! authorization code with PKCE through the system browser and a one-shot
//! listener on this computer (RFC 8252, the desktop-app pattern), as for
//! single sign-on ([`crate::oidc`]). The refresh token then feeds a
//! [`TokenSource`] like any other sign-in.
//!
//! Kantor's app registrations (client ids) come from the build:
//! `KANTOR_MS_CLIENT_ID` (and optionally `KANTOR_MS_TENANT`, default
//! `common`: personal and work or school accounts), `KANTOR_GOOGLE_CLIENT_ID`
//! and `KANTOR_GOOGLE_CLIENT_SECRET` (a Google desktop client's secret is
//! not secret: it ships in every copy). The same variables set when Kantor
//! runs take precedence, for testing a registration without a new build.
//! A provider without a client id isn't offered. See
//! `docs/cloud-providers.md` for the registrations.

use std::sync::atomic::AtomicBool;

use crate::Error;
use crate::oidc::{
    BROWSER_TIMEOUT, DonePage, Endpoints, Loopback, Pkce, Tokens, authorize_url, form_body,
    post_form, random_token, tokens_from,
};

/// A drive that signs in with OAuth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    OneDrive,
    GoogleDrive,
}

/// Kantor's registration with a provider.
#[derive(Clone, PartialEq, Eq)]
pub struct App {
    pub client_id: String,
    pub client_secret: Option<String>,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "App({}, secret: ***)", self.client_id)
    }
}

/// A setting from the environment at run time, else from the build.
fn setting(name: &str, built: Option<&'static str>) -> Option<String> {
    std::env::var(name)
        .ok()
        .or_else(|| built.map(str::to_owned))
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

impl Provider {
    pub const ALL: [Provider; 2] = [Provider::OneDrive, Provider::GoogleDrive];

    /// The id kept in settings (`provider = "onedrive"`).
    pub fn id(self) -> &'static str {
        match self {
            Provider::OneDrive => "onedrive",
            Provider::GoogleDrive => "gdrive",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.id() == id)
    }

    /// The product name (not translated).
    pub fn name(self) -> &'static str {
        match self {
            Provider::OneDrive => "OneDrive",
            Provider::GoogleDrive => "Google Drive",
        }
    }

    /// Kantor's registration, if this build (or the environment) has one.
    pub fn app(self) -> Option<App> {
        match self {
            Provider::OneDrive => Some(App {
                client_id: setting("KANTOR_MS_CLIENT_ID", option_env!("KANTOR_MS_CLIENT_ID"))?,
                client_secret: None,
            }),
            Provider::GoogleDrive => Some(App {
                client_id: setting(
                    "KANTOR_GOOGLE_CLIENT_ID",
                    option_env!("KANTOR_GOOGLE_CLIENT_ID"),
                )?,
                client_secret: setting(
                    "KANTOR_GOOGLE_CLIENT_SECRET",
                    option_env!("KANTOR_GOOGLE_CLIENT_SECRET"),
                ),
            }),
        }
    }

    pub fn available(self) -> bool {
        self.app().is_some()
    }

    /// Where to sign in.
    pub fn endpoints(self) -> Endpoints {
        match self {
            Provider::OneDrive => {
                let tenant = setting("KANTOR_MS_TENANT", option_env!("KANTOR_MS_TENANT"))
                    .unwrap_or_else(|| "common".into());
                let base = format!("https://login.microsoftonline.com/{tenant}");
                Endpoints {
                    issuer: format!("{base}/v2.0"),
                    authorization_endpoint: format!("{base}/oauth2/v2.0/authorize"),
                    token_endpoint: format!("{base}/oauth2/v2.0/token"),
                    device_authorization_endpoint: None,
                    // Microsoft has no RFC 7009 endpoint for these tokens.
                    revocation_endpoint: None,
                }
            }
            Provider::GoogleDrive => Endpoints {
                issuer: "https://accounts.google.com".into(),
                authorization_endpoint: "https://accounts.google.com/o/oauth2/v2/auth".into(),
                token_endpoint: "https://oauth2.googleapis.com/token".into(),
                device_authorization_endpoint: None,
                revocation_endpoint: Some("https://oauth2.googleapis.com/revoke".into()),
            },
        }
    }

    /// What Kantor asks for: the user's files and a refresh token.
    pub fn scope(self) -> &'static str {
        match self {
            Provider::OneDrive => "offline_access Files.ReadWrite User.Read openid profile",
            // Only files Kantor creates or opens (no security assessment).
            Provider::GoogleDrive => {
                "openid email profile https://www.googleapis.com/auth/drive.file"
            }
        }
    }

    /// The address the browser comes back to, for the listener's `port`.
    /// Microsoft matches `http://localhost` registered for desktop apps
    /// with any port; Google's desktop clients take `127.0.0.1`.
    pub fn redirect_uri(self, port: u16) -> String {
        match self {
            Provider::OneDrive => format!("http://localhost:{port}"),
            Provider::GoogleDrive => format!("http://127.0.0.1:{port}"),
        }
    }

    /// Extra authorization parameters: Google gives a refresh token only
    /// with `access_type=offline` (and again on a repeat sign-in only with
    /// `prompt=consent`); both let the user pick the account.
    fn extra(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Provider::OneDrive => &[("prompt", "select_account")],
            Provider::GoogleDrive => &[("access_type", "offline"), ("prompt", "consent")],
        }
    }

    /// The browser address for a sign-in.
    pub fn authorize_url(
        self,
        ep: &Endpoints,
        app: &App,
        redirect_uri: &str,
        state: &str,
        pkce: &Pkce,
    ) -> String {
        let mut url = authorize_url(ep, &app.client_id, redirect_uri, state, pkce, self.scope());
        url.push('&');
        url.push_str(&form_body(self.extra()));
        url
    }
}

/// Trade the browser's code for tokens (with the client secret if the
/// registration has one).
pub fn exchange_code(
    ep: &Endpoints,
    app: &App,
    code: &str,
    redirect_uri: &str,
    pkce: &Pkce,
) -> Result<Tokens, Error> {
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", app.client_id.as_str()),
        ("code_verifier", pkce.verifier.as_str()),
    ];
    if let Some(s) = &app.client_secret {
        form.push(("client_secret", s.as_str()));
    }
    let (status, json) = post_form(&ep.token_endpoint, &form)?;
    tokens_from(status, json)
}

/// Browser sign-in with `provider` at `ep` (its [`Provider::endpoints`],
/// or a test server's): `open` gets the address to open in the browser.
pub fn sign_in(
    provider: Provider,
    app: &App,
    ep: &Endpoints,
    open: &dyn Fn(&str),
    cancel: &AtomicBool,
    page: &DonePage,
) -> Result<Tokens, Error> {
    let loopback = Loopback::bind()?;
    let redirect = provider.redirect_uri(loopback.port());
    let pkce = Pkce::new();
    let state = random_token(16);
    open(&provider.authorize_url(ep, app, &redirect, &state, &pkce));
    let code = loopback.wait_for_code(&state, BROWSER_TIMEOUT, cancel, page)?;
    exchange_code(ep, app, &code, &redirect, &pkce)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_round_trip_and_ask_for_offline_access() {
        for p in Provider::ALL {
            assert_eq!(Provider::from_id(p.id()), Some(p));
        }
        assert!(Provider::OneDrive.scope().contains("offline_access"));
        assert!(Provider::OneDrive.scope().contains("Files.ReadWrite"));
        assert!(Provider::GoogleDrive.scope().ends_with("/auth/drive.file"));
        let ep = Provider::GoogleDrive.endpoints();
        let app = App {
            client_id: "cid".into(),
            client_secret: Some("s".into()),
        };
        let url = Provider::GoogleDrive.authorize_url(
            &ep,
            &app,
            "http://127.0.0.1:5/",
            "st",
            &Pkce::from_verifier("v".repeat(43)),
        );
        assert!(url.contains("access_type=offline"), "{url}");
        assert!(url.contains("prompt=consent"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(
            !url.contains("client_secret"),
            "the secret never goes to the browser"
        );
        assert_eq!(
            Provider::OneDrive.redirect_uri(4321),
            "http://localhost:4321"
        );
        assert!(
            Provider::OneDrive
                .endpoints()
                .token_endpoint
                .ends_with("/oauth2/v2.0/token")
        );
    }
}
