//! Batch steps (kantor_pdf::batch plugins): "Add e-Meterai" and "Sign with
//! Provider". Both run on the file the earlier steps produced.
//!
//! ```json
//! {"type": "emeterai", "provider": "mekari", "doc_type": "Surat Perjanjian",
//!  "page": "last", "place": {"anchor": "bottom-right", "dx": 72, "dy": 72}}
//! {"type": "provider-sign", "provider": "privy", "signer": "a@b.id",
//!  "appearance": {"kind": "invisible"}}
//! ```

use std::sync::{Arc, Mutex};

use kantor_pdf::batch::{self, Applied, FileContext, Place as BoxPlace, Plugin};
use kantor_pdf::sign::Place;
use serde::{Deserialize, Serialize};

use crate::check::Kept;
use crate::emeterai::{self, EMeteraiProvider, StampRequest};
use crate::signer::{self, RemoteSigner, SignRequest, Signer};

pub const EMETERAI: &str = "emeterai";
pub const PROVIDER_SIGN: &str = "provider-sign";

/// Why a file was skipped (shown as they are; the app translates them).
pub mod why {
    pub const ENCRYPTED: &str = "Not stamped: the file is password-protected, and the provider needs the whole document (turn on \"Stamp an unprotected copy\" to stamp a copy without the password)";
    pub const NOT_OWNER: &str = "Not stamped: an unprotected copy needs the file's owner (permissions) password, not the password that opens it";
    pub const ENCRYPTED_SIGNED: &str = "Not stamped: the file is signed and password-protected; removing the password would break its signatures";
    pub const NO_PAGE: &str = "Not stamped: the file doesn't have the step's page";
    pub const SIGNED_REWRITE: &str = "Not stamped: the file is signed and this provider rebuilds files, which would break the signatures";
    pub const CERTIFIED: &str = "Not stamped: the file is certified and allows no changes";
    pub const SIGN_ENCRYPTED: &str =
        "Not signed: the file is password-protected, and this provider needs the whole document";
    pub const SIGN_RESTRICTED: &str = "Not signed: the file's security settings don't allow signing with the password that opens it (give the owner password)";
    pub const SIGN_CERTIFIED: &str =
        "Not signed: the file is certified and allows no changes, not even another signature";
    pub const SIGN_NO_PAGE: &str = "Not signed: the file doesn't have the step's page";
    /// Not a skip: said of a file stamped from an unprotected copy.
    pub const UNPROTECTED: &str = "Stamped an unprotected copy: the result has no password (adding it back would break the e-meterai)";
    pub const ALL: [&str; 11] = [
        ENCRYPTED,
        NOT_OWNER,
        ENCRYPTED_SIGNED,
        NO_PAGE,
        SIGNED_REWRITE,
        CERTIFIED,
        SIGN_ENCRYPTED,
        SIGN_RESTRICTED,
        SIGN_CERTIFIED,
        SIGN_NO_PAGE,
        UNPROTECTED,
    ];
}

/// The "emeterai" step's options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EMeteraiStep {
    pub provider: String,
    pub doc_type: String,
    /// `first`, `last` or a page number.
    pub page: String,
    pub place: BoxPlace,
    /// The box's side in points.
    pub size: f32,
    pub reason: String,
    pub location: String,
    /// Password-protected files: stamp a copy without the password (needs
    /// the owner password; see [`emeterai::unprotected_copy`]). Off, they
    /// are skipped.
    pub unprotected_copy: bool,
}

impl Default for EMeteraiStep {
    fn default() -> Self {
        EMeteraiStep {
            provider: String::new(),
            doc_type: String::new(),
            page: "last".into(),
            place: BoxPlace::default(),
            size: emeterai::SIZE_PT,
            reason: String::new(),
            location: String::new(),
            unprotected_copy: false,
        }
    }
}

impl EMeteraiStep {
    pub fn from_value(v: &serde_json::Value) -> Result<EMeteraiStep, String> {
        serde_json::from_value(v.clone()).map_err(|e| e.to_string())
    }

    pub fn to_step(&self) -> batch::Step {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        if let Some(m) = v.as_object_mut() {
            m.insert("type".into(), EMETERAI.into());
        }
        batch::Step::Plugin {
            kind: EMETERAI.into(),
            options: v,
        }
    }
}

/// Where a provider signature goes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Appearance {
    Invisible,
    Field {
        name: String,
    },
    Box {
        #[serde(default)]
        page: String,
        #[serde(default)]
        place: BoxPlace,
        #[serde(default = "default_w")]
        width: f32,
        #[serde(default = "default_h")]
        height: f32,
    },
}

fn default_w() -> f32 {
    180.0
}
fn default_h() -> f32 {
    60.0
}

/// The "provider-sign" step's options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderSignStep {
    pub provider: String,
    /// The signer's account at the provider.
    pub signer: String,
    pub appearance: Appearance,
    pub reason: String,
    pub location: String,
}

impl Default for ProviderSignStep {
    fn default() -> Self {
        ProviderSignStep {
            provider: String::new(),
            signer: String::new(),
            appearance: Appearance::Invisible,
            reason: String::new(),
            location: String::new(),
        }
    }
}

impl ProviderSignStep {
    pub fn from_value(v: &serde_json::Value) -> Result<ProviderSignStep, String> {
        serde_json::from_value(v.clone()).map_err(|e| e.to_string())
    }

    pub fn to_step(&self) -> batch::Step {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        if let Some(m) = v.as_object_mut() {
            m.insert("type".into(), PROVIDER_SIGN.into());
        }
        batch::Step::Plugin {
            kind: PROVIDER_SIGN.into(),
            options: v,
        }
    }
}

/// Makes a provider client by id (reads settings and the keychain, so it
/// runs when a file is processed, off the UI thread).
pub type Factory<T> = Arc<dyn Fn(&str) -> Result<Arc<T>, crate::Error> + Send + Sync>;

/// Provider clients by id: given, or made on first use.
struct Clients<T: ?Sized> {
    /// The ids steps may name.
    ids: Vec<String>,
    made: Mutex<Vec<Arc<T>>>,
    make: Option<Factory<T>>,
}

impl<T: ?Sized> Clients<T> {
    fn get(&self, id: &str, what: &str, id_of: fn(&T) -> &'static str) -> Result<Arc<T>, String> {
        if !self.ids.iter().any(|i| i == id) {
            return Err(format!(
                "the {what} provider \"{id}\" isn't set up in e-Sign Providers"
            ));
        }
        let mut made = self.made.lock().map_err(|_| "busy".to_string())?;
        if let Some(c) = made.iter().find(|c| id_of(c) == id) {
            return Ok(c.clone());
        }
        let make = self.make.as_ref().ok_or_else(|| {
            format!("the {what} provider \"{id}\" isn't set up in e-Sign Providers")
        })?;
        let c = make(id).map_err(|e| e.to_string())?;
        made.push(c.clone());
        Ok(c)
    }
}

/// Runs "emeterai" steps with one provider per id.
pub struct EMeteraiPlugin {
    clients: Clients<dyn EMeteraiProvider>,
}

impl EMeteraiPlugin {
    pub fn new(providers: Vec<Arc<dyn EMeteraiProvider>>) -> EMeteraiPlugin {
        EMeteraiPlugin {
            clients: Clients {
                ids: providers.iter().map(|p| p.info().id.to_string()).collect(),
                made: Mutex::new(providers),
                make: None,
            },
        }
    }

    /// Providers `ids`, made by `make` when a file first needs one.
    pub fn lazy(ids: Vec<String>, make: Factory<dyn EMeteraiProvider>) -> EMeteraiPlugin {
        EMeteraiPlugin {
            clients: Clients {
                ids,
                made: Mutex::new(Vec::new()),
                make: Some(make),
            },
        }
    }

    fn ids(&self) -> &[String] {
        &self.clients.ids
    }

    fn get(&self, id: &str) -> Result<Arc<dyn EMeteraiProvider>, String> {
        self.clients.get(id, "e-meterai", |p| p.info().id)
    }
}

impl Plugin for EMeteraiPlugin {
    fn kind(&self) -> &str {
        EMETERAI
    }

    fn check(&self, options: &serde_json::Value) -> Result<(), String> {
        let s = EMeteraiStep::from_value(options)?;
        if !self.ids().contains(&s.provider) {
            return Err(format!(
                "the e-meterai provider \"{}\" isn't set up in e-Sign Providers",
                s.provider
            ));
        }
        if s.doc_type.trim().is_empty() {
            return Err("choose the document type".into());
        }
        if !(emeterai::SIZE_MIN_PT..=emeterai::SIZE_MAX_PT).contains(&s.size) {
            return Err(format!(
                "the e-meterai must be {:.0} to {:.0} points wide",
                emeterai::SIZE_MIN_PT,
                emeterai::SIZE_MAX_PT
            ));
        }
        Ok(())
    }

    fn apply(
        &self,
        options: &serde_json::Value,
        file: &FileContext,
        bytes: &[u8],
    ) -> Result<Applied, String> {
        let s = EMeteraiStep::from_value(options)?;
        let p = self.get(&s.provider)?;
        // The provider gets the whole file: a password-protected one only
        // as an unprotected copy, when the step allows that and the batch's
        // password for the file is its owner password. Checked before
        // anything is sent.
        let plain;
        let mut unprotected = false;
        let bytes = if emeterai::is_protected(bytes) {
            if !s.unprotected_copy {
                return Ok(Applied::Skipped(why::ENCRYPTED.into()));
            }
            plain = match emeterai::unprotected_copy(bytes, file.password.unwrap_or("")) {
                Ok(b) => b,
                Err(crate::Error::OwnerPassword) => {
                    return Ok(Applied::Skipped(why::NOT_OWNER.into()));
                }
                Err(crate::Error::ProtectedSigned) => {
                    return Ok(Applied::Skipped(why::ENCRYPTED_SIGNED.into()));
                }
                Err(e) => return Err(e.to_string()),
            };
            unprotected = true;
            plain.as_slice()
        } else {
            bytes
        };
        let pdf = kantor_pdf::Pdf::from_bytes(bytes, None).map_err(|e| e.to_string())?;
        let sigs = kantor_pdf::sig::signatures(&pdf);
        if sigs.and_then(|s| s.docmdp) == Some(1) {
            return Ok(Applied::Skipped(why::CERTIFIED.into()));
        }
        if sigs.is_some() && p.info().rewrites_file {
            return Ok(Applied::Skipped(why::SIGNED_REWRITE.into()));
        }
        let Ok(index) = batch::one_page(&s.page, pdf.page_count()) else {
            return Ok(Applied::Skipped(why::NO_PAGE.into()));
        };
        let (rect, _) = batch::place_box(&pdf, index, &s.place, s.size, s.size)
            .ok_or_else(|| "the page can't be read".to_string())?;
        let mut req = StampRequest::new(file.name, index, rect, &s.doc_type);
        req.reason = s.reason.clone();
        req.location = s.location.clone();
        let (out, _) =
            emeterai::stamp_checked(p.as_ref(), bytes, &req, false).map_err(|e| e.to_string())?;
        Ok(if unprotected {
            Applied::Noted(out.pdf, why::UNPROTECTED.into())
        } else {
            Applied::Changed(out.pdf)
        })
    }
}

/// Runs "provider-sign" steps.
pub struct ProviderSignPlugin {
    clients: Clients<dyn RemoteSigner>,
}

impl ProviderSignPlugin {
    pub fn new(providers: Vec<Arc<dyn RemoteSigner>>) -> ProviderSignPlugin {
        ProviderSignPlugin {
            clients: Clients {
                ids: providers.iter().map(|p| p.info().id.to_string()).collect(),
                made: Mutex::new(providers),
                make: None,
            },
        }
    }

    /// Providers `ids`, made by `make` when a file first needs one.
    pub fn lazy(ids: Vec<String>, make: Factory<dyn RemoteSigner>) -> ProviderSignPlugin {
        ProviderSignPlugin {
            clients: Clients {
                ids,
                made: Mutex::new(Vec::new()),
                make: Some(make),
            },
        }
    }

    fn get(&self, id: &str) -> Result<Arc<dyn RemoteSigner>, String> {
        self.clients.get(id, "signing", |p| p.info().id)
    }
}

impl Plugin for ProviderSignPlugin {
    fn kind(&self) -> &str {
        PROVIDER_SIGN
    }

    fn last(&self) -> bool {
        true
    }

    fn check(&self, options: &serde_json::Value) -> Result<(), String> {
        let s = ProviderSignStep::from_value(options)?;
        if !self.clients.ids.contains(&s.provider) {
            return Err(format!(
                "the signing provider \"{}\" isn't set up in e-Sign Providers",
                s.provider
            ));
        }
        if s.signer.trim().is_empty() {
            return Err("type the signer's account at the provider".into());
        }
        Ok(())
    }

    fn apply(
        &self,
        options: &serde_json::Value,
        file: &FileContext,
        bytes: &[u8],
    ) -> Result<Applied, String> {
        let s = ProviderSignStep::from_value(options)?;
        let p = self.get(&s.provider)?;
        // A password-protected file opens with the batch's password for it
        // and stays protected (hash signing: the provider only sees a hash),
        // with the same permission rules as signing with a certificate.
        // Providers that take the whole document can't open it.
        if !p.info().hash_signing && emeterai::is_protected(bytes) {
            return Ok(Applied::Skipped(why::SIGN_ENCRYPTED.into()));
        }
        let pdf = match kantor_pdf::sign::open_to_sign(bytes, file.password) {
            Ok(pdf) => pdf,
            Err(kantor_pdf::Error::Invalid(e)) if e == kantor_pdf::sign::NOT_PERMITTED => {
                return Ok(Applied::Skipped(why::SIGN_RESTRICTED.into()));
            }
            Err(e) => return Err(e.to_string()),
        };
        if kantor_pdf::sig::signatures(&pdf).and_then(|s| s.docmdp) == Some(1) {
            return Ok(Applied::Skipped(why::SIGN_CERTIFIED.into()));
        }
        let mut rotate = 0;
        let place = match &s.appearance {
            Appearance::Invisible => Place::Invisible,
            Appearance::Field { name } => Place::Field(name.clone()),
            Appearance::Box {
                page,
                place,
                width,
                height,
            } => {
                let Ok(index) = batch::one_page(page, pdf.page_count()) else {
                    return Ok(Applied::Skipped(why::SIGN_NO_PAGE.into()));
                };
                let (rect, r) = batch::place_box(&pdf, index, place, *width, *height)
                    .ok_or_else(|| "the page can't be read".to_string())?;
                rotate = r;
                Place::New { page: index, rect }
            }
        };
        let mut req = SignRequest::new(file.name, place);
        req.reason = s.reason.clone();
        req.location = s.location.clone();
        req.rotate = rotate;
        req.password = file.password.map(str::to_owned);
        let who = Signer {
            id: s.signer.clone(),
            otp: None,
        };
        let (out, kept) =
            signer::sign(p.as_ref(), bytes, &req, &who, false).map_err(|e| e.to_string())?;
        debug_assert!(!matches!(kept, Kept::Broken(_)));
        Ok(Applied::Changed(out))
    }
}
