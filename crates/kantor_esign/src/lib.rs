//! e-meterai stamping and certified electronic signatures through licensed
//! Indonesian providers' APIs, behind provider-neutral traits.
//!
//! - [`EMeteraiProvider`]: Peruri's e-Meterai (Rp 10.000) as sold by its
//!   official distributors. The user places the meterai on a page; the
//!   provider stamps and returns the PDF.
//! - [`RemoteSigner`]: a certified signature by a Komdigi-licensed PSrE.
//!   Hash-based services plug into Kantor's own PAdES signer as an
//!   [`pdf_engine::signing::ExternalSigner`] (Kantor writes the signature
//!   field, the provider returns the CMS); others take the whole document.
//!
//! Nothing goes online unless the user allowed that provider in e-Sign
//! Providers ([`kantor_core::settings::EsignProviderSettings::allow_network`]).
//! Credentials live in the OS keychain ([`creds`]). Which providers publish
//! their API, and how each was implemented, is recorded in
//! `docs/decisions/0004-esign-providers.md`.

pub mod batch;
pub mod check;
pub mod creds;
pub mod emeterai;
pub mod http;
pub mod json;
pub mod mock;
pub mod providers;
pub mod signer;
pub mod util;

#[cfg(test)]
mod tests;

use std::fmt;
use std::sync::Arc;

use kantor_core::settings::EsignProviderSettings;

pub use creds::Credentials;
pub use emeterai::{DocType, EMeteraiProvider, StampRequest, Stamped};
pub use providers::{PROVIDERS, find};
pub use signer::{RemoteSigner, SignRequest, Signer};

/// Why a provider call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The provider isn't set up (no credentials) in e-Sign Providers.
    NotConfigured(String),
    /// The user hasn't allowed Kantor to contact this provider.
    NetworkOff(String),
    /// This provider is documented but Kantor can't call it yet (the text
    /// says why: its API isn't public).
    NotImplemented(String),
    /// The credentials were refused.
    Auth(String),
    /// No e-meterai quota or balance left.
    Quota(String),
    /// The provider needs a one-time password (OTP) sent to the signer.
    OtpRequired(String),
    /// Couldn't reach the provider.
    Network(String),
    /// The provider answered with an error, or something Kantor didn't
    /// expect.
    Provider(String),
    /// The PDF can't be used (encrypted, damaged…).
    Pdf(String),
    /// The provider's result would break earlier signatures.
    BreaksSignatures(Vec<String>),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotConfigured(p) => write!(
                f,
                "{p} isn't set up: add your provider account in e-Sign Providers"
            ),
            Error::NetworkOff(p) => write!(
                f,
                "Kantor isn't allowed to send documents to {p}: turn it on in e-Sign Providers"
            ),
            Error::NotImplemented(w) => write!(f, "not available yet: {w}"),
            Error::Auth(m) => write!(f, "the provider refused the credentials: {m}"),
            Error::Quota(m) => write!(f, "no e-meterai quota left: {m}"),
            Error::OtpRequired(m) => write!(f, "a one-time password (OTP) is needed: {m}"),
            Error::Network(m) | Error::Provider(m) | Error::Pdf(m) => f.write_str(m),
            Error::BreaksSignatures(f2) => write!(
                f,
                "the provider rebuilt the file, which breaks the earlier signature(s): {}",
                f2.join(", ")
            ),
        }
    }
}

impl std::error::Error for Error {}

impl From<kantor_pdf::Error> for Error {
    fn from(e: kantor_pdf::Error) -> Self {
        Error::Pdf(e.to_string())
    }
}

/// What a provider offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    EMeterai,
    Signing,
}

/// How much of a provider Kantor can call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    /// Built from the provider's public API documentation and tested
    /// against a mock of it; works once an account exists.
    Implemented,
    /// Listed so users find it, but its API documentation isn't public:
    /// calls fail with [`Error::NotImplemented`] and this reason.
    Stub(&'static str),
}

/// One credential field of a provider (shown in e-Sign Providers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    /// Typed masked and never shown again.
    pub secret: bool,
    pub optional: bool,
}

/// A provider Kantor knows.
#[derive(Clone, Copy, Debug)]
pub struct Info {
    /// Stable id used in settings, the keychain, the CLI and batch files.
    pub id: &'static str,
    pub name: &'static str,
    pub kinds: &'static [Kind],
    pub support: Support,
    /// Its public API documentation.
    pub docs: &'static str,
    pub production_url: &'static str,
    /// Empty when there is no public sandbox.
    pub sandbox_url: &'static str,
    pub fields: &'static [Field],
    /// e-meterai: the provider rebuilds the PDF instead of adding an
    /// incremental update (Kantor warns before sending a signed file).
    pub rewrites_file: bool,
    /// Signing: the API signs a hash (Kantor builds the PDF signature).
    pub hash_signing: bool,
}

impl Info {
    pub fn offers(&self, k: Kind) -> bool {
        self.kinds.contains(&k)
    }

    pub fn implemented(&self) -> bool {
        self.support == Support::Implemented
    }
}

/// A provider set up by the user: its settings and credentials, and the
/// transport to reach it.
#[derive(Clone)]
pub struct Account {
    pub info: &'static Info,
    pub settings: EsignProviderSettings,
    pub creds: Credentials,
    pub transport: Arc<dyn http::Transport>,
}

impl fmt::Debug for Account {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Account")
            .field("id", &self.info.id)
            .field("settings", &self.settings)
            .field("creds", &self.creds)
            .finish()
    }
}

impl Account {
    /// The account for provider `id` from the saved settings and keychain
    /// (blocks on the keychain: call off the UI thread).
    pub fn load(id: &str, all: &[EsignProviderSettings]) -> Result<Account, Error> {
        let info = find(id).ok_or_else(|| Error::NotConfigured(id.to_string()))?;
        let settings = all
            .iter()
            .find(|s| s.id == info.id)
            .cloned()
            .ok_or_else(|| Error::NotConfigured(info.name.to_string()))?;
        Ok(Account {
            info,
            settings,
            creds: creds::load(info.id),
            transport: Arc::new(http::Ureq::new()),
        })
    }

    /// The server to call: the override, else sandbox or production.
    pub fn base_url(&self) -> String {
        let s = self.settings.base_url.trim();
        let url = if !s.is_empty() {
            s
        } else if self.settings.sandbox && !self.info.sandbox_url.is_empty() {
            self.info.sandbox_url
        } else {
            self.info.production_url
        };
        url.trim_end_matches('/').to_string()
    }

    /// Fails unless the provider is implemented, set up and allowed online.
    pub fn ready(&self) -> Result<(), Error> {
        if let Support::Stub(why) = self.info.support {
            return Err(Error::NotImplemented(format!("{}: {why}", self.info.name)));
        }
        if !self.creds.missing(self.info.fields).is_empty() {
            return Err(Error::NotConfigured(self.info.name.to_string()));
        }
        if !self.settings.allow_network {
            return Err(Error::NetworkOff(self.info.name.to_string()));
        }
        Ok(())
    }

    /// Send a request (after [`Account::ready`]).
    pub fn send(&self, req: http::Request) -> Result<http::Response, Error> {
        self.ready()?;
        log::debug!("{}: {:?}", self.info.id, req);
        self.transport.send(&req)
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url(), path)
    }

    /// A credential field (empty string when missing).
    pub fn cred(&self, key: &str) -> String {
        self.creds.get(key).unwrap_or_default().to_string()
    }
}

/// The providers that offer `kind` and are set up and allowed online, from
/// settings alone (no keychain call): what the PDF menu enables.
pub fn configured(all: &[EsignProviderSettings], kind: Kind) -> Vec<&'static Info> {
    all.iter()
        .filter(|s| s.allow_network)
        .filter_map(|s| find(&s.id))
        .filter(|i| i.offers(kind) && i.implemented())
        .collect()
}
