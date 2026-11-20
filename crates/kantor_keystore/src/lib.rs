//! Signing keys that live outside Kantor: PKCS #11 tokens and smart cards
//! ([`pkcs11`], any platform), the macOS keychain and the Windows
//! certificate store ([`system`]).
//!
//! This crate only talks to the foreign code: it lists certificates (as
//! DER) and signs a digest. Building the signature (CMS, PDF) is
//! `pdf_engine::signing`'s job. PINs and passwords are passed straight
//! through and never kept.

pub mod pkcs11;
pub mod system;

use std::fmt;

/// Why a key operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Wrong PIN.
    WrongPin,
    /// Locked after too many wrong PINs.
    Locked,
    /// The user cancelled a system prompt.
    Cancelled,
    Failed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::WrongPin => f.write_str("the PIN is wrong"),
            Error::Locked => f.write_str("the token is locked (too many wrong PINs)"),
            Error::Cancelled => f.write_str("cancelled"),
            Error::Failed(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for Error {}

/// A digest algorithm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashAlg {
    Sha256,
    Sha384,
    Sha512,
}

impl HashAlg {
    pub fn size(self) -> usize {
        match self {
            HashAlg::Sha256 => 32,
            HashAlg::Sha384 => 48,
            HashAlg::Sha512 => 64,
        }
    }

    /// The DER DigestInfo prefix for PKCS #1 v1.5 (RFC 8017 §9.2).
    pub fn digest_info_prefix(self) -> &'static [u8] {
        match self {
            HashAlg::Sha256 => &[
                0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
                0x01, 0x05, 0x00, 0x04, 0x20,
            ],
            HashAlg::Sha384 => &[
                0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
                0x02, 0x05, 0x00, 0x04, 0x30,
            ],
            HashAlg::Sha512 => &[
                0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
                0x03, 0x05, 0x00, 0x04, 0x40,
            ],
        }
    }
}

/// How to sign. The input is always the digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mech {
    /// RSA PKCS #1 v1.5.
    RsaPkcs1(HashAlg),
    /// RSASSA-PSS, MGF1 with the same digest, salt as long as the digest.
    RsaPss(HashAlg),
    /// ECDSA; the result is a DER Ecdsa-Sig-Value.
    Ecdsa(HashAlg),
}

impl Mech {
    pub fn hash(self) -> HashAlg {
        match self {
            Mech::RsaPkcs1(h) | Mech::RsaPss(h) | Mech::Ecdsa(h) => h,
        }
    }
}

/// `r || s` to a DER `Ecdsa-Sig-Value`.
pub fn ecdsa_raw_to_der(raw: &[u8]) -> Vec<u8> {
    fn int(b: &[u8]) -> Vec<u8> {
        let mut b = b;
        while b.len() > 1 && b[0] == 0 {
            b = &b[1..];
        }
        let mut v = vec![0x02];
        let pad = b.first().is_some_and(|x| x & 0x80 != 0);
        v.push((b.len() + pad as usize) as u8);
        if pad {
            v.push(0);
        }
        v.extend_from_slice(b);
        v
    }
    let (r, s) = raw.split_at(raw.len() / 2);
    let body = [int(r), int(s)].concat();
    let mut out = vec![0x30];
    if body.len() < 0x80 {
        out.push(body.len() as u8);
    } else {
        out.extend([0x81, body.len() as u8]);
    }
    out.extend(body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ecdsa_der() {
        let der = ecdsa_raw_to_der(&[0x80, 1, 0, 2]);
        assert_eq!(der, vec![0x30, 8, 2, 3, 0, 0x80, 1, 2, 1, 2]);
    }
}
