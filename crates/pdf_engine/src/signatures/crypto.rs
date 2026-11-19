//! Digests and signature checks, with ring (already in Kantor's tree
//! through rustls): RSA PKCS#1 v1.5 and PSS, ECDSA P-256/P-384, SHA-1/2.

use ring::{digest as rd, signature as rs};

use super::der::{self, Tlv};
use super::x509::PublicKey;

pub const SHA1: &str = "1.3.14.3.2.26";
pub const SHA256: &str = "2.16.840.1.101.3.4.2.1";
pub const SHA384: &str = "2.16.840.1.101.3.4.2.2";
pub const SHA512: &str = "2.16.840.1.101.3.4.2.3";

pub const RSA: &str = "1.2.840.113549.1.1.1";
const RSA_PSS: &str = "1.2.840.113549.1.1.10";
pub const EC_PUBLIC_KEY: &str = "1.2.840.10045.2.1";
pub const P256: &str = "1.2.840.10045.3.1.7";
pub const P384: &str = "1.3.132.0.34";
pub const P521: &str = "1.3.132.0.35";

/// An AlgorithmIdentifier: OID and the raw parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlgId {
    pub oid: String,
    pub params: Vec<u8>,
}

impl AlgId {
    pub fn parse(t: Tlv<'_>) -> der::Result<AlgId> {
        let mut r = t.reader();
        let oid = r.read()?.oid()?;
        let params = if r.is_empty() {
            Vec::new()
        } else {
            r.read()?.raw.to_vec()
        };
        Ok(AlgId { oid, params })
    }

    /// The digest a signature algorithm names ("sha256WithRSA" → SHA-256);
    /// `None` for bare key algorithms (rsaEncryption).
    pub fn digest(&self) -> Option<&'static str> {
        Some(match self.oid.as_str() {
            "1.2.840.113549.1.1.5" | "1.2.840.10045.4.1" | "1.3.14.3.2.29" => SHA1,
            "1.2.840.113549.1.1.11" | "1.2.840.10045.4.3.2" => SHA256,
            "1.2.840.113549.1.1.12" | "1.2.840.10045.4.3.3" => SHA384,
            "1.2.840.113549.1.1.13" | "1.2.840.10045.4.3.4" => SHA512,
            RSA_PSS => pss_hash(&self.params).unwrap_or(SHA1),
            _ => return None,
        })
    }
}

/// The hash of RSASSA-PSS parameters (SHA-1 when absent).
fn pss_hash(params: &[u8]) -> Option<&'static str> {
    let seq = Tlv::parse(params).ok()?;
    let mut r = seq.reader();
    let Ok(Some(h)) = r.read_opt(der::ctx(0)) else {
        return Some(SHA1);
    };
    let alg = AlgId::parse(h.reader().read().ok()?).ok()?;
    digest_name(&alg.oid)
}

pub fn digest_name(oid: &str) -> Option<&'static str> {
    [SHA1, SHA256, SHA384, SHA512]
        .into_iter()
        .find(|&d| d == oid)
}

/// "SHA-256" for a digest OID.
pub fn digest_label(oid: &str) -> String {
    match oid {
        SHA1 => "SHA-1".into(),
        SHA256 => "SHA-256".into(),
        SHA384 => "SHA-384".into(),
        SHA512 => "SHA-512".into(),
        "2.16.840.1.101.3.4.2.4" => "SHA-224".into(),
        "1.2.840.113549.2.5" => "MD5".into(),
        o => o.into(),
    }
}

fn ring_digest(oid: &str) -> Option<&'static rd::Algorithm> {
    Some(match oid {
        SHA1 => &rd::SHA1_FOR_LEGACY_USE_ONLY,
        SHA256 => &rd::SHA256,
        SHA384 => &rd::SHA384,
        SHA512 => &rd::SHA512,
        _ => return None,
    })
}

/// A digest that can be fed in pieces.
pub struct Hasher(rd::Context);

impl Hasher {
    pub fn new(oid: &str) -> Option<Hasher> {
        ring_digest(oid).map(|a| Hasher(rd::Context::new(a)))
    }

    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    pub fn finish(self) -> Vec<u8> {
        self.0.finish().as_ref().to_vec()
    }
}

pub fn digest(oid: &str, data: &[u8]) -> Option<Vec<u8>> {
    ring_digest(oid).map(|a| rd::digest(a, data).as_ref().to_vec())
}

pub fn sha256(data: &[u8]) -> Vec<u8> {
    rd::digest(&rd::SHA256, data).as_ref().to_vec()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerifyError {
    /// The algorithm (or key/hash pairing) isn't supported.
    Unsupported(String),
    /// The signature doesn't match.
    Bad,
}

/// Check `sig` over `msg` with `key`. `sig_alg` is the signature
/// algorithm; when it names only the key type (CMS often writes plain
/// rsaEncryption), `digest` says which hash was used.
pub fn verify(
    key: &PublicKey,
    sig_alg: &AlgId,
    digest: Option<&str>,
    msg: &[u8],
    sig: &[u8],
) -> Result<(), VerifyError> {
    let hash = sig_alg
        .digest()
        .or(digest.and_then(digest_name))
        .ok_or_else(|| VerifyError::Unsupported(sig_alg.oid.clone()))?;
    let unsupported =
        || VerifyError::Unsupported(format!("{} with {}", key.describe(), digest_label(hash)));
    let alg: &dyn rs::VerificationAlgorithm = match key.alg.as_str() {
        RSA if sig_alg.oid == RSA_PSS => match hash {
            SHA256 => &rs::RSA_PSS_2048_8192_SHA256,
            SHA384 => &rs::RSA_PSS_2048_8192_SHA384,
            SHA512 => &rs::RSA_PSS_2048_8192_SHA512,
            _ => return Err(unsupported()),
        },
        RSA => match hash {
            SHA1 => &rs::RSA_PKCS1_1024_8192_SHA1_FOR_LEGACY_USE_ONLY,
            SHA256 => &rs::RSA_PKCS1_1024_8192_SHA256_FOR_LEGACY_USE_ONLY,
            SHA384 => &rs::RSA_PKCS1_2048_8192_SHA384,
            SHA512 => &rs::RSA_PKCS1_1024_8192_SHA512_FOR_LEGACY_USE_ONLY,
            _ => return Err(unsupported()),
        },
        EC_PUBLIC_KEY => match (key.curve.as_deref(), hash) {
            (Some(P256), SHA256) => &rs::ECDSA_P256_SHA256_ASN1,
            (Some(P256), SHA384) => &rs::ECDSA_P256_SHA384_ASN1,
            (Some(P384), SHA256) => &rs::ECDSA_P384_SHA256_ASN1,
            (Some(P384), SHA384) => &rs::ECDSA_P384_SHA384_ASN1,
            _ => return Err(unsupported()),
        },
        _ => return Err(unsupported()),
    };
    rs::UnparsedPublicKey::new(alg, &key.key)
        .verify(msg, sig)
        .map_err(|_| VerifyError::Bad)
}
