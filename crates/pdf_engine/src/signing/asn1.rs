//! DER encoders for what signing writes (CMS, time-stamp requests).

use crate::signatures::der::{self, Tlv, encode};
use crate::signatures::x509::Cert;

pub use crate::signatures::der::{ctx, encode_oid as oid};

pub fn seq(parts: &[&[u8]]) -> Vec<u8> {
    encode(der::SEQUENCE, &parts.concat())
}

/// A DER SET OF: elements sorted by their encodings.
pub fn set_of(mut items: Vec<Vec<u8>>) -> Vec<u8> {
    items.sort();
    encode(der::SET, &items.concat())
}

/// Context-specific `[n]` constructed, around `body` (EXPLICIT, or IMPLICIT
/// of a constructed type).
pub fn tagged(n: u8, body: &[u8]) -> Vec<u8> {
    encode(der::ctx(n), body)
}

pub fn octets(b: &[u8]) -> Vec<u8> {
    encode(der::OCTET_STRING, b)
}

pub fn null() -> Vec<u8> {
    vec![der::NULL, 0]
}

pub fn boolean(v: bool) -> Vec<u8> {
    vec![der::BOOLEAN, 1, if v { 0xff } else { 0 }]
}

/// A non-negative INTEGER from big-endian magnitude bytes.
pub fn uint(b: &[u8]) -> Vec<u8> {
    let mut b = b;
    while b.len() > 1 && b[0] == 0 {
        b = &b[1..];
    }
    let mut body = Vec::with_capacity(b.len() + 1);
    if b.is_empty() || b[0] & 0x80 != 0 {
        body.push(0);
    }
    body.extend_from_slice(b);
    encode(der::INTEGER, &body)
}

pub fn int(v: u64) -> Vec<u8> {
    uint(&v.to_be_bytes())
}

/// GeneralizedTime `YYYYMMDDHHMMSSZ`.
pub fn generalized_time(t: i64) -> Vec<u8> {
    let (y, mo, d, h, mi, s) = der::civil(t);
    encode(
        der::GENERALIZED_TIME,
        format!("{y:04}{mo:02}{d:02}{h:02}{mi:02}{s:02}Z").as_bytes(),
    )
}

/// An AlgorithmIdentifier with no parameters.
pub fn alg(o: &str) -> Vec<u8> {
    seq(&[&oid(o)])
}

/// An AlgorithmIdentifier with NULL parameters.
pub fn alg_null(o: &str) -> Vec<u8> {
    seq(&[&oid(o), &null()])
}

/// The certificate's serial number INTEGER exactly as written in it.
pub fn serial_of(cert: &Cert) -> Vec<u8> {
    (|| -> der::Result<Vec<u8>> {
        let mut t = Tlv::parse(cert.tbs())?.reader();
        t.read_opt(der::ctx(0))?;
        Ok(t.read_tag(der::INTEGER)?.raw.to_vec())
    })()
    .unwrap_or_else(|_| uint(&cert.serial))
}

/// IssuerAndSerialNumber.
pub fn issuer_serial(cert: &Cert) -> Vec<u8> {
    seq(&[&cert.issuer, &serial_of(cert)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers() {
        assert_eq!(int(0), vec![2, 1, 0]);
        assert_eq!(int(127), vec![2, 1, 127]);
        assert_eq!(int(128), vec![2, 2, 0, 128]);
        assert_eq!(uint(&[0, 0, 1, 0]), vec![2, 2, 1, 0]);
    }

    #[test]
    fn sets_sort() {
        assert_eq!(
            set_of(vec![vec![2, 1, 5], vec![2, 1, 3]]),
            vec![0x31, 6, 2, 1, 3, 2, 1, 5]
        );
    }
}
