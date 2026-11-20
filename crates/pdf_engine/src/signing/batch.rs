//! "Sign with Certificate" as a batch step (`{"type": "sign", …}` in
//! `kantor_pdf::batch` files): the same options as the Sign with
//! Certificate dialog and `kantor pdf sign`, run on every file after the
//! batch's other steps, so the signature covers everything the batch did.
//!
//! The batch file names the certificate (a .p12 file, a token's PKCS #11
//! module, or the system store) but never holds its password or PIN: the
//! caller asks for it once when the batch starts, unlocks the key with
//! [`unlock`] and registers a [`SignPlugin`] holding the unlocked key for
//! that run.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use kantor_pdf::annot::Art;
use kantor_pdf::batch::{self, Applied, Batch, FileContext, Place as BoxPlace, Plugin, Step};
use kantor_pdf::sign::{Look, Place};
use serde::{Deserialize, Serialize};

use super::{Error, Identity, Level, LookText, Options, stores};
use crate::signatures::{Fetch, TrustStore};

/// The step's `type` in batch files.
pub const KIND: &str = "sign";

/// Where the signing certificate is. Never holds a password or PIN.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Certificate {
    /// A .p12/.pfx file (relative paths are from the batch file's folder).
    File(String),
    /// A USB token or smart card through its PKCS #11 module.
    Token {
        module: String,
        /// Pick the certificate whose name contains this (when the token
        /// holds several).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        name: String,
    },
    /// The macOS keychain or the Windows certificate store.
    System {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        name: String,
    },
}

impl Default for Certificate {
    fn default() -> Self {
        Certificate::File(String::new())
    }
}

/// How much PAdES, as batch files write it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LevelName {
    #[default]
    #[serde(rename = "B-B")]
    BB,
    #[serde(rename = "B-T")]
    BT,
    #[serde(rename = "B-LT")]
    BLt,
    #[serde(rename = "B-LTA")]
    BLta,
}

impl LevelName {
    pub fn level(self) -> Level {
        match self {
            LevelName::BB => Level::B,
            LevelName::BT => Level::T,
            LevelName::BLt => Level::Lt,
            LevelName::BLta => Level::Lta,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            LevelName::BB => "B-B",
            LevelName::BT => "B-T",
            LevelName::BLt => "B-LT",
            LevelName::BLta => "B-LTA",
        }
    }
}

/// Where the signature shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Appearance {
    /// No box (listed in the signature panel only).
    Invisible,
    /// An existing empty signature field, by name.
    Field { name: String },
    /// A new box on one page, placed as the stamp step places pictures.
    Box {
        /// `first`, `last` or a page number.
        #[serde(default = "last_page")]
        page: String,
        #[serde(default)]
        place: BoxPlace,
        #[serde(default = "box_width")]
        width: f32,
        #[serde(default = "box_height")]
        height: f32,
    },
}

fn last_page() -> String {
    "last".into()
}

fn box_width() -> f32 {
    180.0
}

fn box_height() -> f32 {
    60.0
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance::Invisible
    }
}

/// The sign step's options.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SignStep {
    pub certificate: Certificate,
    pub level: LevelName,
    /// Time-stamp server (B-T and up).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tsa: String,
    /// Certify with DocMDP permission 1, 2 or 3 (none: an approval
    /// signature).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certify: Option<u8>,
    pub appearance: Appearance,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub location: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub contact: String,
    /// A PNG or JPEG beside the text of a visible signature (a path, or
    /// `my-stamps/NAME`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub image: String,
    /// RSASSA-PSS instead of PKCS #1 v1.5 (RSA keys).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pss: bool,
}

/// Keys a sign step must not have: secrets never go in batch files.
const SECRET_KEYS: [&str; 4] = ["password", "pin", "passphrase", "secret"];

impl SignStep {
    pub fn from_value(v: &serde_json::Value) -> Result<SignStep, String> {
        if let Some(o) = v.as_object()
            && let Some(k) = SECRET_KEYS.iter().find(|k| o.contains_key(**k))
        {
            return Err(format!(
                "\"{k}\" doesn't belong in a batch file: the password or PIN is asked for when the batch runs"
            ));
        }
        let s: SignStep = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
        s.validate()?;
        Ok(s)
    }

    pub fn to_step(&self) -> Step {
        let mut options = serde_json::to_value(self).unwrap_or_default();
        if let Some(o) = options.as_object_mut() {
            o.remove("type");
        }
        Step::Plugin {
            kind: KIND.into(),
            options,
        }
    }

    /// Check the options (what [`SignStep::from_value`] checks).
    pub fn validate(&self) -> Result<(), String> {
        match &self.certificate {
            Certificate::File(p) if p.trim().is_empty() => {
                return Err("choose the certificate file (.p12 or .pfx)".into());
            }
            Certificate::Token { module, .. } if module.trim().is_empty() => {
                return Err("give the token's PKCS #11 module".into());
            }
            _ => {}
        }
        if self.certify.is_some_and(|c| !(1..=3).contains(&c)) {
            return Err("\"certify\" must be 1, 2 or 3".into());
        }
        if self.level != LevelName::BB && self.tsa.trim().is_empty() {
            return Err(format!(
                "{} needs a time-stamp server (\"tsa\")",
                self.level.name()
            ));
        }
        match &self.appearance {
            Appearance::Field { name } if name.trim().is_empty() => {
                return Err("give the signature field's name".into());
            }
            Appearance::Box {
                page,
                width,
                height,
                ..
            } => {
                if !(20.0..=1000.0).contains(width) || !(10.0..=1000.0).contains(height) {
                    return Err(
                        "the signature box must be 20 to 1000 points wide and 10 to 1000 high"
                            .into(),
                    );
                }
                // A page number is checked against each file; the words here.
                let p = page.trim().to_ascii_lowercase();
                if !(p == "first" || p == "last" || p.parse::<u32>().is_ok_and(|n| n >= 1)) {
                    return Err(format!("\"{page}\" is not first, last or a page number"));
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// The batch's sign step, if it has one: its index and options.
pub fn find(batch: &Batch) -> Option<(usize, Result<SignStep, String>)> {
    batch.steps.iter().enumerate().find_map(|(i, s)| match s {
        Step::Plugin { kind, options } if kind == KIND => Some((i, SignStep::from_value(options))),
        _ => None,
    })
}

/// The certificate file's path (relative paths from `base`).
pub fn certificate_path(spec: &str, base: &Path) -> PathBuf {
    let p = Path::new(spec);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// Whether unlocking asks the user for something: a .p12 file's password
/// or a token's PIN (`None` for the system store, which asks itself).
pub fn secret_kind(c: &Certificate) -> Option<&'static str> {
    match c {
        Certificate::File(_) => Some("password"),
        Certificate::Token { .. } => Some("PIN"),
        Certificate::System { .. } => None,
    }
}

fn pick(found: Vec<stores::Candidate>, name: &str, from: &str) -> Result<stores::Candidate, Error> {
    let want = name.trim().to_lowercase();
    let mut found: Vec<_> = found
        .into_iter()
        .filter(|c| {
            want.is_empty()
                || c.cert
                    .subject_name()
                    .display_name()
                    .to_lowercase()
                    .contains(&want)
        })
        .collect();
    match found.len() {
        0 => Err(Error::Key(format!(
            "no signing certificate found in {from}"
        ))),
        1 => Ok(found.remove(0)),
        _ => Err(Error::Key(format!(
            "several certificates in {from}; name one in the step: {}",
            found
                .iter()
                .map(|c| format!("\"{}\"", c.cert.subject_name().display_name()))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Unlock the step's certificate with `secret` (the .p12 password or the
/// token PIN; `None` for a PIN pad or the system store).
pub fn unlock(c: &Certificate, base: &Path, secret: Option<&str>) -> Result<Identity, Error> {
    match c {
        Certificate::File(spec) => {
            let path = certificate_path(spec, base);
            let data =
                std::fs::read(&path).map_err(|e| Error::Key(format!("{}: {e}", path.display())))?;
            Identity::from_p12(&data, secret.unwrap_or(""), &path)
        }
        Certificate::Token { module, name } => {
            let cand = pick(stores::token_candidates(module)?, name, module)?;
            if secret.is_none() && !cand.pin_pad() {
                return Err(Error::WrongPassword);
            }
            cand.unlock(secret)
        }
        Certificate::System { name } => {
            if !stores::SYSTEM_STORE {
                return Err(Error::Unsupported(
                    "the system certificate store on this system (use a .p12 file or a token)"
                        .into(),
                ));
            }
            pick(stores::system_candidates()?, name, "the system store")?.unlock(None)
        }
    }
}

/// The sign step for one run, with its key unlocked.
pub struct SignPlugin {
    id: Identity,
    /// The batch file's folder (relative picture paths).
    base: PathBuf,
    my_stamps: Option<PathBuf>,
    /// Network for time stamps and LTV data; `None`: offline (B-B only).
    fetch: Option<Arc<dyn Fetch + Send>>,
    trust: TrustStore,
    words: LookText,
    pictures: Mutex<HashMap<String, Arc<Vec<u8>>>>,
}

impl SignPlugin {
    pub fn new(id: Identity, base: &Path, my_stamps: Option<&Path>) -> SignPlugin {
        SignPlugin {
            id,
            base: base.to_path_buf(),
            my_stamps: my_stamps.map(Path::to_path_buf),
            fetch: None,
            trust: TrustStore::standard(),
            words: LookText::default(),
            pictures: Mutex::default(),
        }
    }

    /// Allow the network (time stamps, LTV data).
    pub fn online(mut self, fetch: Arc<dyn Fetch + Send>) -> SignPlugin {
        self.fetch = Some(fetch);
        self
    }

    /// Roots for building the chain for LTV data.
    pub fn trust(mut self, trust: TrustStore) -> SignPlugin {
        self.trust = trust;
        self
    }

    /// The visible signature's wording (translations).
    pub fn words(mut self, words: LookText) -> SignPlugin {
        self.words = words;
        self
    }

    /// The signer's name.
    pub fn signer(&self) -> String {
        self.id.name()
    }

    fn picture(&self, spec: &str) -> Result<Arc<Vec<u8>>, String> {
        if let Ok(m) = self.pictures.lock()
            && let Some(p) = m.get(spec)
        {
            return Ok(p.clone());
        }
        let path = batch::resolve_path(spec, &self.base, self.my_stamps.as_deref());
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let ok = bytes.starts_with(b"\x89PNG") || bytes.starts_with(&[0xFF, 0xD8]);
        if !ok {
            return Err(format!(
                "{}: the signature's picture must be a PNG or JPEG",
                path.display()
            ));
        }
        let bytes = Arc::new(bytes);
        if let Ok(mut m) = self.pictures.lock() {
            m.insert(spec.to_owned(), bytes.clone());
        }
        Ok(bytes)
    }
}

/// Why a file wasn't signed (the results show them; the app translates
/// them as they are, so they take no arguments).
pub mod why {
    pub const ENCRYPTED: &str = "Not signed: password-protected PDFs can't be signed yet";
    pub const NO_CHANGES: &str =
        "Not signed: the file is certified and allows no changes, not even another signature";
    pub const CERTIFY_SIGNED: &str = "Not signed: the file is already signed, and only the first signature can certify a document";
    pub const NO_FIELD: &str = "Not signed: the file has no signature field with the step's name";
    pub const FIELD_SIGNED: &str = "Not signed: the step's signature field is already signed";
    pub const CERTIFIED_BOX: &str = "Not signed: the file is certified, and a new visible signature would change its pages (sign an empty signature field, or invisibly)";
    pub const NO_PAGE: &str = "Not signed: the file doesn't have the step's page";
    /// All of them.
    pub const ALL: [&str; 7] = [
        ENCRYPTED,
        NO_CHANGES,
        CERTIFY_SIGNED,
        NO_FIELD,
        FIELD_SIGNED,
        CERTIFIED_BOX,
        NO_PAGE,
    ];
}

fn skip(why: &str) -> Result<Applied, String> {
    Ok(Applied::Skipped(why.to_owned()))
}

impl Plugin for SignPlugin {
    fn kind(&self) -> &str {
        KIND
    }

    fn last(&self) -> bool {
        true
    }

    fn check(&self, options: &serde_json::Value) -> Result<(), String> {
        let s = SignStep::from_value(options)?;
        if s.level != LevelName::BB && self.fetch.is_none() {
            return Err(Error::Offline.to_string());
        }
        if !s.image.is_empty() && !matches!(s.appearance, Appearance::Invisible) {
            self.picture(&s.image)?;
        }
        let now = crate::signatures::now();
        if self.id.cert.not_after < now {
            return Err(format!(
                "the certificate expired on {}",
                crate::signatures::format_time(self.id.cert.not_after)
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
        let s = SignStep::from_value(options)?;
        if file.password.is_some() {
            return skip(why::ENCRYPTED);
        }
        let pdf = kantor_pdf::Pdf::from_bytes(bytes, None).map_err(|e| e.to_string())?;
        if pdf.was_encrypted() {
            return skip(why::ENCRYPTED);
        }
        let sigs = kantor_pdf::sig::signatures(&pdf);
        let certified = sigs.and_then(|s| s.docmdp);
        if certified == Some(1) {
            return skip(why::NO_CHANGES);
        }
        if s.certify.is_some() && sigs.is_some() {
            return skip(why::CERTIFY_SIGNED);
        }
        let mut rotate = 0;
        let place = match &s.appearance {
            Appearance::Invisible => Place::Invisible,
            Appearance::Field { name } => {
                match kantor_pdf::sign::signature_fields(&pdf)
                    .into_iter()
                    .find(|f| f.name == *name)
                {
                    None => return skip(why::NO_FIELD),
                    Some(f) if f.signed => {
                        return skip(why::FIELD_SIGNED);
                    }
                    Some(_) => Place::Field(name.clone()),
                }
            }
            Appearance::Box {
                page,
                place,
                width,
                height,
            } => {
                if certified.is_some() {
                    return skip(why::CERTIFIED_BOX);
                }
                let index = match batch::one_page(page, pdf.page_count()) {
                    Ok(i) => i,
                    Err(_) => return skip(why::NO_PAGE),
                };
                let (rect, r) = batch::place_box(&pdf, index, place, *width, *height)
                    .ok_or_else(|| "the page can't be read".to_string())?;
                rotate = r;
                Place::New { page: index, rect }
            }
        };
        let nonempty = |v: &str| (!v.trim().is_empty()).then(|| v.trim().to_owned());
        let now = crate::signatures::now();
        let mut o = Options::new(place.clone());
        o.reason = nonempty(&s.reason);
        o.location = nonempty(&s.location);
        o.contact = nonempty(&s.contact);
        o.certify = s.certify;
        o.tsa = nonempty(&s.tsa);
        o.level = s.level.level();
        o.pss = s.pss;
        o.fetch = self.fetch.as_deref().map(|f| f as &dyn Fetch);
        o.trust = self.trust.clone();
        o.now = now;
        o.rotate = rotate;
        if !matches!(place, Place::Invisible) {
            let art = match s.image.as_str() {
                "" => None,
                spec => Some(Art::Image(self.picture(spec)?)),
            };
            o.look = Some(Look {
                lines: super::look_lines(
                    &self.id.name(),
                    now,
                    o.reason.as_deref(),
                    o.location.as_deref(),
                    &self.words,
                ),
                art,
            });
        }
        super::sign(bytes, &self.id, &o)
            .map(Applied::Changed)
            .map_err(|e| e.to_string())
    }
}
