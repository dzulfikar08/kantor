//! The signed update manifest.
//!
//! One JSON file per channel, signed with Ed25519 over its exact bytes; the
//! detached signature (`<manifest>.sig`) is the base64 of the 64-byte
//! signature. CI writes both with `scripts/update-manifest.sh`.
//!
//! ```json
//! {
//!   "channel": "stable",
//!   "version": "0.2.1",
//!   "build": 0,
//!   "date": "2026-11-10T08:00:00Z",
//!   "notes_url": "https://github.com/dzulfikar08/kantor/releases/tag/v0.2.1",
//!   "assets": {
//!     "linux-x86_64-appimage": { "url": "https://…", "sha256": "…", "size": 52000000 },
//!     …
//!   }
//! }
//! ```
//!
//! Asset keys: `macos-universal-app` (a tar.gz holding the notarised, stapled
//! Kantor.app), `linux-{x86_64,aarch64}-appimage`, `linux-{x86_64,aarch64}-tarball`.

use std::collections::BTreeMap;

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::version::BuildId;

/// The key the app trusts: base64 of the raw 32-byte Ed25519 public key.
/// A build may override it at compile time (`KANTOR_UPDATE_PUBKEY`) to test
/// against a throwaway key; release builds never set it.
pub const PUBLIC_KEY: &str = match option_env!("KANTOR_UPDATE_PUBKEY") {
    Some(k) => k,
    None => "2sy5SODzbFlquPqJV9yDrc5bYyaZ+UY4X29N2GkB9tE=",
};

/// Manifests are small; anything bigger is not one.
pub const MAX_MANIFEST: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub channel: String,
    pub version: String,
    /// Nightly build stamp (`YYYYMMDDHHMMSS`, UTC); 0 for releases.
    #[serde(default)]
    pub build: u64,
    pub date: String,
    pub notes_url: String,
    pub assets: BTreeMap<String, Asset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub url: String,
    /// Lowercase hex SHA-256 of the file.
    pub sha256: String,
    pub size: u64,
}

impl Manifest {
    pub fn build_id(&self) -> Result<BuildId, Error> {
        BuildId::new(&self.version, self.build).map_err(Error::Manifest)
    }
}

/// Decode a base64 Ed25519 public key.
pub fn decode_public_key(b64: &str) -> Result<[u8; 32], Error> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|_| Error::Manifest("public key is not base64".into()))?;
    bytes
        .try_into()
        .map_err(|_| Error::Manifest("public key is not 32 bytes".into()))
}

/// Check `sig` (base64 text) over the exact `bytes`, then parse and validate
/// them. Nothing is parsed before the signature checks out.
pub fn verify(bytes: &[u8], sig: &[u8], public_key: &[u8; 32]) -> Result<Manifest, Error> {
    let sig_text = std::str::from_utf8(sig).map_err(|_| Error::Signature)?;
    let sig = base64::engine::general_purpose::STANDARD
        .decode(sig_text.trim())
        .map_err(|_| Error::Signature)?;
    if sig.len() != 64 {
        return Err(Error::Signature);
    }
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public_key)
        .verify(bytes, &sig)
        .map_err(|_| Error::Signature)?;
    let m: Manifest =
        serde_json::from_slice(bytes).map_err(|e| Error::Manifest(format!("invalid JSON: {e}")))?;
    m.build_id()?;
    for (key, a) in &m.assets {
        let hex_ok = a.sha256.len() == 64
            && a.sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !hex_ok {
            return Err(Error::Manifest(format!("{key}: bad sha256")));
        }
        if !(a.url.starts_with("https://") || a.url.starts_with("http://")) {
            return Err(Error::Manifest(format!("{key}: URL must be http(s)")));
        }
        if a.size == 0 {
            return Err(Error::Manifest(format!("{key}: size 0")));
        }
    }
    Ok(m)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair as _};

    pub(crate) fn keypair(seed: u8) -> Ed25519KeyPair {
        Ed25519KeyPair::from_seed_unchecked(&[seed; 32]).unwrap()
    }

    pub(crate) fn public(k: &Ed25519KeyPair) -> [u8; 32] {
        k.public_key().as_ref().try_into().unwrap()
    }

    pub(crate) fn sign(k: &Ed25519KeyPair, bytes: &[u8]) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD
            .encode(k.sign(bytes).as_ref())
            .into_bytes()
    }

    const SAMPLE: &str = r#"{
  "channel": "stable",
  "version": "0.2.1",
  "date": "2026-11-10T08:00:00Z",
  "notes_url": "https://github.com/dzulfikar08/kantor/releases/tag/v0.2.1",
  "assets": {
    "linux-x86_64-appimage": {
      "url": "https://github.com/dzulfikar08/kantor/releases/download/v0.2.1/Kantor-0.2.1-x86_64.AppImage",
      "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
      "size": 52000000
    }
  }
}
"#;

    #[test]
    fn embedded_key_decodes() {
        decode_public_key(PUBLIC_KEY).unwrap();
        assert!(decode_public_key("AAAA").is_err());
    }

    #[test]
    fn good_signature() {
        let k = keypair(7);
        let m = verify(SAMPLE.as_bytes(), &sign(&k, SAMPLE.as_bytes()), &public(&k)).unwrap();
        assert_eq!(m.version, "0.2.1");
        assert_eq!(m.build, 0);
        assert_eq!(m.assets["linux-x86_64-appimage"].size, 52000000);
    }

    #[test]
    fn bad_signature() {
        let k = keypair(7);
        let mut sig = base64::engine::general_purpose::STANDARD
            .decode(sign(&k, SAMPLE.as_bytes()))
            .unwrap();
        sig[10] ^= 1;
        let sig = base64::engine::general_purpose::STANDARD.encode(sig);
        assert!(matches!(
            verify(SAMPLE.as_bytes(), sig.as_bytes(), &public(&k)),
            Err(Error::Signature)
        ));
        for junk in [&b""[..], b"not base64!", b"AAAA"] {
            assert!(matches!(
                verify(SAMPLE.as_bytes(), junk, &public(&k)),
                Err(Error::Signature)
            ));
        }
    }

    #[test]
    fn wrong_key() {
        let (k, other) = (keypair(7), keypair(8));
        assert!(matches!(
            verify(
                SAMPLE.as_bytes(),
                &sign(&k, SAMPLE.as_bytes()),
                &public(&other)
            ),
            Err(Error::Signature)
        ));
    }

    #[test]
    fn tampered_bytes() {
        let k = keypair(7);
        let sig = sign(&k, SAMPLE.as_bytes());
        let tampered = SAMPLE.replace("0.2.1\"", "9.9.9\"");
        assert!(matches!(
            verify(tampered.as_bytes(), &sig, &public(&k)),
            Err(Error::Signature)
        ));
        // Even whitespace counts: the signature covers the exact bytes.
        let spaced = format!("{SAMPLE} ");
        assert!(matches!(
            verify(spaced.as_bytes(), &sig, &public(&k)),
            Err(Error::Signature)
        ));
    }

    #[test]
    fn signed_but_invalid() {
        let k = keypair(7);
        for (bad, why) in [
            (SAMPLE.replace("e3b0c442", "E3B0C442"), "uppercase hex"),
            (SAMPLE.replace("https://github", "file://github"), "scheme"),
            (SAMPLE.replace("52000000", "0"), "size"),
            (SAMPLE.replace("\"0.2.1\"", "\"latest\""), "version"),
            ("[]".to_owned(), "not an object"),
        ] {
            let r = verify(bad.as_bytes(), &sign(&k, bad.as_bytes()), &public(&k));
            assert!(matches!(r, Err(Error::Manifest(_))), "{why}: {r:?}");
        }
    }
}
