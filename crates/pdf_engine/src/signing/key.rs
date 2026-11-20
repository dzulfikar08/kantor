//! Private keys that sign: software keys (from a PKCS #12 file) with ring,
//! and the [`SigningKey`] trait hardware tokens and system stores implement.

use ring::rand::SystemRandom;
use ring::signature::{self as rs, EcdsaKeyPair, RsaKeyPair};

use super::Error;
use crate::signatures::crypto;
use crate::signatures::der::{self, Tlv};
use crate::signatures::x509::{Cert, PublicKey};

/// A digest for signing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hash {
    Sha256,
    Sha384,
    Sha512,
}

impl Hash {
    pub fn oid(self) -> &'static str {
        match self {
            Hash::Sha256 => crypto::SHA256,
            Hash::Sha384 => crypto::SHA384,
            Hash::Sha512 => crypto::SHA512,
        }
    }

    pub fn len(self) -> usize {
        match self {
            Hash::Sha256 => 32,
            Hash::Sha384 => 48,
            Hash::Sha512 => 64,
        }
    }

    pub fn digest(self, data: &[u8]) -> Vec<u8> {
        crypto::digest(self.oid(), data).unwrap_or_default()
    }

    /// DER DigestInfo prefix for PKCS #1 v1.5 (RFC 8017 §9.2 note 1).
    pub fn digest_info(self, hash: &[u8]) -> Vec<u8> {
        use super::asn1::{alg_null, octets, seq};
        seq(&[&alg_null(self.oid()), &octets(hash)])
    }
}

/// The kind of key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyType {
    Rsa { bits: usize },
    P256,
    P384,
}

impl KeyType {
    pub fn of(key: &PublicKey) -> Option<KeyType> {
        match key.alg.as_str() {
            crypto::RSA => {
                let bits = Tlv::parse(&key.key)
                    .ok()
                    .and_then(|t| t.reader().read().ok())
                    .map(|n| n.uint_bytes().len() * 8)
                    .unwrap_or(0);
                Some(KeyType::Rsa { bits })
            }
            crypto::EC_PUBLIC_KEY => match key.curve.as_deref() {
                Some(crypto::P256) => Some(KeyType::P256),
                Some(crypto::P384) => Some(KeyType::P384),
                _ => None,
            },
            _ => None,
        }
    }

    /// The digest that goes with the key.
    pub fn hash(self) -> Hash {
        match self {
            KeyType::P384 => Hash::Sha384,
            _ => Hash::Sha256,
        }
    }

    pub fn is_rsa(self) -> bool {
        matches!(self, KeyType::Rsa { .. })
    }
}

/// A private key that can sign. `sign` gets the message (not its hash)
/// and returns the signature as CMS stores it (ECDSA as a DER
/// `Ecdsa-Sig-Value`).
pub trait SigningKey: Send + Sync {
    fn key_type(&self) -> KeyType;
    fn sign(&self, msg: &[u8], hash: Hash, pss: bool) -> Result<Vec<u8>, Error>;
}

/// A key held in memory (read from a PKCS #12 file).
pub struct SoftKey {
    kind: Soft,
}

enum Soft {
    Rsa(RsaKeyPair),
    Ec(EcdsaKeyPair, KeyType),
}

impl SoftKey {
    /// Build the key from a PKCS #8 PrivateKeyInfo, checking it belongs to
    /// `cert`.
    pub fn from_pkcs8(pkcs8: &[u8], cert: &Cert) -> Result<SoftKey, Error> {
        let bad = |m: &str| Error::Key(m.to_string());
        let mut r = Tlv::parse(pkcs8)
            .and_then(|t| t.expect(der::SEQUENCE))
            .map_err(|_| bad("the private key is unreadable"))?
            .reader();
        let mut parse = || -> der::Result<(String, Vec<u8>, Vec<u8>)> {
            let _v = r.read_tag(der::INTEGER)?;
            let alg = r.read_tag(der::SEQUENCE)?;
            let mut a = alg.reader();
            let oid = a.read()?.oid()?;
            let params = if a.is_empty() {
                Vec::new()
            } else {
                a.read()?.raw.to_vec()
            };
            let key = r.read_tag(der::OCTET_STRING)?.body.to_vec();
            Ok((oid, params, key))
        };
        let (oid, params, key) = parse().map_err(|_| bad("the private key is unreadable"))?;
        let ktype = KeyType::of(&cert.key)
            .ok_or_else(|| Error::Unsupported(format!("{} keys", cert.key.describe())))?;
        match oid.as_str() {
            crypto::RSA => {
                let kp = RsaKeyPair::from_pkcs8(pkcs8)
                    .or_else(|_| RsaKeyPair::from_der(&key))
                    .map_err(|e| Error::Unsupported(format!("this RSA key ({e})")))?;
                if kp.public().as_ref() != cert.key.key.as_slice() {
                    return Err(bad("the private key doesn't belong to the certificate"));
                }
                Ok(SoftKey {
                    kind: Soft::Rsa(kp),
                })
            }
            crypto::EC_PUBLIC_KEY => {
                let curve = Tlv::parse(&params).ok().and_then(|t| t.oid().ok());
                // ECPrivateKey { version, privateKey OCTET STRING, … }
                let scalar = Tlv::parse(&key)
                    .ok()
                    .and_then(|t| {
                        let mut r = t.reader();
                        r.read_tag(der::INTEGER).ok()?;
                        Some(r.read_tag(der::OCTET_STRING).ok()?.body.to_vec())
                    })
                    .ok_or_else(|| bad("the private key is unreadable"))?;
                let (alg, len): (&'static rs::EcdsaSigningAlgorithm, usize) = match ktype {
                    KeyType::P256 => (&rs::ECDSA_P256_SHA256_ASN1_SIGNING, 32),
                    KeyType::P384 => (&rs::ECDSA_P384_SHA384_ASN1_SIGNING, 48),
                    KeyType::Rsa { .. } => {
                        return Err(bad("the private key doesn't belong to the certificate"));
                    }
                };
                if curve
                    .as_deref()
                    .is_some_and(|c| Some(c) != cert.key.curve.as_deref())
                {
                    return Err(bad("the private key doesn't belong to the certificate"));
                }
                let mut padded = vec![0u8; len.saturating_sub(scalar.len())];
                padded.extend_from_slice(&scalar);
                let kp = EcdsaKeyPair::from_private_key_and_public_key(
                    alg,
                    &padded,
                    &cert.key.key,
                    &SystemRandom::new(),
                )
                .map_err(|_| bad("the private key doesn't belong to the certificate"))?;
                Ok(SoftKey {
                    kind: Soft::Ec(kp, ktype),
                })
            }
            other => Err(Error::Unsupported(format!("keys of type {other}"))),
        }
    }
}

impl SigningKey for SoftKey {
    fn key_type(&self) -> KeyType {
        match &self.kind {
            Soft::Rsa(kp) => KeyType::Rsa {
                bits: kp.public().modulus_len() * 8,
            },
            Soft::Ec(_, t) => *t,
        }
    }

    fn sign(&self, msg: &[u8], hash: Hash, pss: bool) -> Result<Vec<u8>, Error> {
        let rng = SystemRandom::new();
        match &self.kind {
            Soft::Rsa(kp) => {
                let pad: &'static dyn rs::RsaEncoding = match (hash, pss) {
                    (Hash::Sha256, false) => &rs::RSA_PKCS1_SHA256,
                    (Hash::Sha384, false) => &rs::RSA_PKCS1_SHA384,
                    (Hash::Sha512, false) => &rs::RSA_PKCS1_SHA512,
                    (Hash::Sha256, true) => &rs::RSA_PSS_SHA256,
                    (Hash::Sha384, true) => &rs::RSA_PSS_SHA384,
                    (Hash::Sha512, true) => &rs::RSA_PSS_SHA512,
                };
                let mut sig = vec![0; kp.public().modulus_len()];
                kp.sign(pad, &rng, msg, &mut sig)
                    .map_err(|_| Error::Key("RSA signing failed".into()))?;
                Ok(sig)
            }
            Soft::Ec(kp, t) => {
                if hash != t.hash() {
                    return Err(Error::Unsupported(format!(
                        "{} with this key",
                        crypto::digest_label(hash.oid())
                    )));
                }
                Ok(kp
                    .sign(&rng, msg)
                    .map_err(|_| Error::Key("ECDSA signing failed".into()))?
                    .as_ref()
                    .to_vec())
            }
        }
    }
}
