//! Identities whose keys stay outside Kantor: PKCS #11 tokens and smart
//! cards, and the macOS/Windows system store (see the kantor_keystore
//! crate for the foreign calls).

use std::sync::Arc;

use kantor_keystore::{HashAlg, Mech, pkcs11, system};

use super::key::{Hash, KeyType, SigningKey};
use super::{Error, Identity, Source};
use crate::signatures::x509::Cert;

impl From<kantor_keystore::Error> for Error {
    fn from(e: kantor_keystore::Error) -> Self {
        match e {
            kantor_keystore::Error::WrongPin => Error::WrongPassword,
            kantor_keystore::Error::Locked => Error::Locked,
            kantor_keystore::Error::Cancelled => Error::Key("cancelled".into()),
            kantor_keystore::Error::Failed(m) => Error::Key(m),
        }
    }
}

fn mech(kt: KeyType, hash: Hash, pss: bool) -> Mech {
    let h = match hash {
        Hash::Sha256 => HashAlg::Sha256,
        Hash::Sha384 => HashAlg::Sha384,
        Hash::Sha512 => HashAlg::Sha512,
    };
    match kt {
        KeyType::Rsa { .. } if pss => Mech::RsaPss(h),
        KeyType::Rsa { .. } => Mech::RsaPkcs1(h),
        KeyType::P256 | KeyType::P384 => Mech::Ecdsa(h),
    }
}

/// A certificate offered for signing, before its key is unlocked.
pub struct Candidate {
    pub cert: Arc<Cert>,
    /// Other certificates found beside it (possible issuers).
    pub others: Vec<Arc<Cert>>,
    pub place: Place,
}

pub enum Place {
    Token {
        module: Arc<pkcs11::Module>,
        cert: pkcs11::TokenCert,
    },
    System(Arc<system::SystemCert>),
}

impl Candidate {
    /// Where it is, for the picker ("Token SafeNet", "System").
    pub fn source(&self) -> Source {
        match &self.place {
            Place::Token { module, cert } => Source::Token {
                module: module.path().to_string(),
                token: cert.token.clone(),
            },
            Place::System(_) => Source::System,
        }
    }

    /// The token wants the PIN on its own keypad.
    pub fn pin_pad(&self) -> bool {
        matches!(&self.place, Place::Token { cert, .. } if cert.pin_pad)
    }

    /// Unlock the key (tokens: log in with `pin`; the system store asks
    /// on its own).
    pub fn unlock(&self, pin: Option<&str>) -> Result<Identity, Error> {
        let kt = KeyType::of(&self.cert.key)
            .ok_or_else(|| Error::Unsupported(format!("{} keys", self.cert.key.describe())))?;
        let key: Box<dyn SigningKey> = match &self.place {
            Place::Token { module, cert } => Box::new(TokenSigner {
                key: module.key(cert, pin)?,
                kt,
            }),
            Place::System(s) => Box::new(SystemSigner {
                cert: s.clone(),
                kt,
            }),
        };
        Ok(Identity {
            cert: self.cert.clone(),
            chain: self.others.clone(),
            key,
            source: self.source(),
        })
    }
}

fn candidates(found: Vec<(Arc<Cert>, Place)>) -> Vec<Candidate> {
    let all: Vec<Arc<Cert>> = found.iter().map(|(c, _)| c.clone()).collect();
    found
        .into_iter()
        .filter(|(c, _)| !c.is_ca())
        .map(|(cert, place)| Candidate {
            others: all.iter().filter(|o| !o.same(&cert)).cloned().collect(),
            cert,
            place,
        })
        .collect()
}

/// The certificates on the tokens a PKCS #11 module sees.
pub fn token_candidates(module_path: &str) -> Result<Vec<Candidate>, Error> {
    let module = pkcs11::Module::load(module_path)?;
    let found = module
        .certificates()?
        .into_iter()
        .filter_map(|tc| {
            let cert = Cert::parse(&tc.der).ok()?;
            Some((
                Arc::new(cert),
                Place::Token {
                    module: module.clone(),
                    cert: tc,
                },
            ))
        })
        .collect();
    Ok(candidates(found))
}

/// Certificates with keys in the system store (macOS keychain, Windows
/// Personal store); empty elsewhere.
pub fn system_candidates() -> Result<Vec<Candidate>, Error> {
    let found = system::certificates()?
        .into_iter()
        .filter_map(|s| {
            let cert = Cert::parse(&s.der).ok()?;
            Some((Arc::new(cert), Place::System(Arc::new(s))))
        })
        .collect();
    Ok(candidates(found))
}

pub use kantor_keystore::pkcs11::AVAILABLE as TOKEN_MODULES;
pub use kantor_keystore::pkcs11::common_modules;
pub use kantor_keystore::system::AVAILABLE as SYSTEM_STORE;

struct TokenSigner {
    key: pkcs11::TokenKey,
    kt: KeyType,
}

impl SigningKey for TokenSigner {
    fn key_type(&self) -> KeyType {
        self.kt
    }

    fn sign(&self, msg: &[u8], hash: Hash, pss: bool) -> Result<Vec<u8>, Error> {
        Ok(self.key.sign(mech(self.kt, hash, pss), &hash.digest(msg))?)
    }
}

struct SystemSigner {
    cert: Arc<system::SystemCert>,
    kt: KeyType,
}

impl SigningKey for SystemSigner {
    fn key_type(&self) -> KeyType {
        self.kt
    }

    fn sign(&self, msg: &[u8], hash: Hash, pss: bool) -> Result<Vec<u8>, Error> {
        Ok(self
            .cert
            .sign(mech(self.kt, hash, pss), &hash.digest(msg))?)
    }
}
