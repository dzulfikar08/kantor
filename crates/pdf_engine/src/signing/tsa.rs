//! RFC 3161 time stamps from a time-stamp authority (TSA) over HTTP.

use super::Error;
use super::asn1::{alg, boolean, int, octets, seq};
use super::key::Hash;
use crate::signatures::Fetch;
use crate::signatures::cms::{CT_TST_INFO, SignedData, TstInfo};
use crate::signatures::der::{self, Tlv};

/// A TimeStampReq for `hashed` (a digest made with `hash`).
pub fn request(hash: Hash, hashed: &[u8], nonce: u64) -> Vec<u8> {
    let imprint = seq(&[&alg(hash.oid()), &octets(hashed)]);
    seq(&[&int(1), &imprint, &int(nonce), &boolean(true)])
}

fn nonce() -> u64 {
    use ring::rand::SecureRandom;
    let mut b = [0u8; 8];
    let _ = ring::rand::SystemRandom::new().fill(&mut b);
    u64::from_be_bytes(b) >> 1
}

/// Ask the TSA at `url` to stamp `data`; returns the token (a CMS
/// ContentInfo) after checking it stamps the right digest.
pub fn stamp(fetch: &dyn Fetch, url: &str, hash: Hash, data: &[u8]) -> Result<Vec<u8>, Error> {
    let hashed = hash.digest(data);
    let req = request(hash, &hashed, nonce());
    let body = fetch
        .post(url, "application/timestamp-query", &req)
        .ok_or_else(|| Error::Tsa(format!("no answer from the time-stamp server {url}")))?;
    token_from_response(&body, &hashed)
}

/// The token in a TimeStampResp, checked against `hashed`.
pub fn token_from_response(body: &[u8], hashed: &[u8]) -> Result<Vec<u8>, Error> {
    let bad = || Error::Tsa("the time-stamp server's answer is unreadable".into());
    let resp = Tlv::parse(body)
        .and_then(|t| t.expect(der::SEQUENCE))
        .map_err(|_| bad())?;
    let mut r = resp.reader();
    let status = r.read_tag(der::SEQUENCE).map_err(|_| bad())?;
    let code = status
        .reader()
        .read_tag(der::INTEGER)
        .and_then(|t| t.small_int())
        .map_err(|_| bad())?;
    if code > 1 {
        return Err(Error::Tsa(format!(
            "the time-stamp server refused the request (status {code})"
        )));
    }
    let token = r.read_tag(der::SEQUENCE).map_err(|_| bad())?.raw.to_vec();
    let sd = SignedData::parse(&token).map_err(|_| bad())?;
    if sd.content_type != CT_TST_INFO {
        return Err(bad());
    }
    let info = sd
        .content
        .as_deref()
        .and_then(|c| TstInfo::parse(c).ok())
        .ok_or_else(bad)?;
    if info.hashed != hashed {
        return Err(Error::Tsa(
            "the time-stamp server stamped something else".into(),
        ));
    }
    Ok(token)
}
