//! Writing CMS SignedData (RFC 5652) the way PAdES wants it: detached,
//! with content-type, message-digest and ESS signing-certificate-v2 signed
//! attributes, and a signature time stamp as an unsigned attribute.

use super::Error;
use super::asn1::{alg, alg_null, generalized_time, int, octets, oid, seq, set_of, tagged};
use super::key::{Hash, KeyType, SigningKey};
use crate::signatures::cms::{
    ATTR_CONTENT_TYPE, ATTR_MESSAGE_DIGEST, ATTR_SIGNING_CERT_V2, ATTR_SIGNING_TIME,
    ATTR_TIMESTAMP_TOKEN,
};
use crate::signatures::crypto;
use crate::signatures::x509::Cert;

pub const DATA: &str = "1.2.840.113549.1.7.1";
const SIGNED_DATA: &str = "1.2.840.113549.1.7.2";

pub fn attribute(o: &str, values: Vec<Vec<u8>>) -> Vec<u8> {
    seq(&[&oid(o), &set_of(values)])
}

/// ESS signing-certificate-v2 (RFC 5035) naming `cert` by its SHA-256.
pub fn signing_certificate_v2(cert: &Cert) -> Vec<u8> {
    let hash = crypto::sha256(&cert.der);
    // GeneralNames { directoryName [4] EXPLICIT Name }
    let general_names = seq(&[&tagged(4, &cert.issuer)]);
    let issuer_serial = seq(&[&general_names, &super::asn1::serial_of(cert)]);
    // hashAlgorithm is left out: SHA-256 is the default.
    let ess_cert_id = seq(&[&octets(&hash), &issuer_serial]);
    let v = seq(&[&seq(&[&ess_cert_id])]);
    attribute(ATTR_SIGNING_CERT_V2, vec![v])
}

/// The signature AlgorithmIdentifier for a key, digest and padding.
pub fn signature_algorithm(kt: KeyType, hash: Hash, pss: bool) -> Vec<u8> {
    match kt {
        KeyType::Rsa { .. } if pss => {
            let h = alg_null(hash.oid());
            let mgf = seq(&[&oid("1.2.840.113549.1.1.8"), &h]);
            let params = seq(&[
                &tagged(0, &h),
                &tagged(1, &mgf),
                &tagged(2, &int(hash.len() as u64)),
            ]);
            seq(&[&oid("1.2.840.113549.1.1.10"), &params])
        }
        KeyType::Rsa { .. } => alg_null(match hash {
            Hash::Sha256 => "1.2.840.113549.1.1.11",
            Hash::Sha384 => "1.2.840.113549.1.1.12",
            Hash::Sha512 => "1.2.840.113549.1.1.13",
        }),
        KeyType::P256 | KeyType::P384 => alg(match hash {
            Hash::Sha256 => "1.2.840.10045.4.3.2",
            Hash::Sha384 => "1.2.840.10045.4.3.3",
            Hash::Sha512 => "1.2.840.10045.4.3.4",
        }),
    }
}

/// A signer about to sign: everything but the unsigned attributes.
pub struct Signed {
    signed_attrs: Vec<u8>,
    sig_alg: Vec<u8>,
    hash: Hash,
    /// The signature value.
    pub signature: Vec<u8>,
}

/// Make the signed attributes for content with digest `digest` and sign
/// them with `key`.
pub fn sign_attributes(
    key: &dyn SigningKey,
    cert: &Cert,
    content_type: &str,
    digest: &[u8],
    hash: Hash,
    pss: bool,
    signing_time: Option<i64>,
) -> Result<Signed, Error> {
    let pss = pss && key.key_type().is_rsa();
    let mut attrs = vec![
        attribute(ATTR_CONTENT_TYPE, vec![oid(content_type)]),
        attribute(ATTR_MESSAGE_DIGEST, vec![octets(digest)]),
        signing_certificate_v2(cert),
    ];
    if let Some(t) = signing_time {
        attrs.push(attribute(ATTR_SIGNING_TIME, vec![generalized_time(t)]));
    }
    let set = set_of(attrs);
    let signature = key.sign(&set, hash, pss)?;
    Ok(Signed {
        signed_attrs: set,
        sig_alg: signature_algorithm(key.key_type(), hash, pss),
        hash,
        signature,
    })
}

/// The ContentInfo around SignedData. `content` is the encapsulated content
/// (a TSTInfo for time-stamp tokens), `None` for a detached signature;
/// `unsigned` are extra unsigned attributes (a time-stamp token).
pub fn assemble(
    s: &Signed,
    cert: &Cert,
    chain: &[&Cert],
    content_type: &str,
    content: Option<&[u8]>,
    unsigned: Vec<Vec<u8>>,
) -> Vec<u8> {
    let mut signed = s.signed_attrs.clone();
    signed[0] = crate::signatures::der::ctx(0);
    let mut si = vec![
        int(1),
        super::asn1::issuer_serial(cert),
        alg(s.hash.oid()),
        signed,
        s.sig_alg.clone(),
        octets(&s.signature),
    ];
    if !unsigned.is_empty() {
        let mut u = set_of(unsigned);
        u[0] = crate::signatures::der::ctx(1);
        si.push(u);
    }
    let si = seq(&si.iter().map(Vec::as_slice).collect::<Vec<_>>());
    let mut certs: Vec<u8> = cert.der.clone();
    for c in chain {
        if c.der != cert.der {
            certs.extend_from_slice(&c.der);
        }
    }
    let eci = match content {
        Some(c) => seq(&[&oid(content_type), &tagged(0, &octets(c))]),
        None => seq(&[&oid(content_type)]),
    };
    let version = if content_type == DATA { 1 } else { 3 };
    let sd = seq(&[
        &int(version),
        &set_of(vec![alg(s.hash.oid())]),
        &eci,
        &tagged(0, &certs),
        &crate::signatures::der::encode(crate::signatures::der::SET, &si),
    ]);
    seq(&[&oid(SIGNED_DATA), &tagged(0, &sd)])
}

/// The unsigned attribute carrying a signature time-stamp token.
pub fn timestamp_attribute(token: &[u8]) -> Vec<u8> {
    attribute(ATTR_TIMESTAMP_TOKEN, vec![token.to_vec()])
}
