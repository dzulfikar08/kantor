//! PKCS #12 (.p12/.pfx) files: a certificate, its chain and the private
//! key, protected by a password (RFC 7292).
//!
//! Reads what OpenSSL, Windows, macOS, Java and the Indonesian CAs write:
//! PBES2 (PBKDF2 with AES-CBC or 3DES) and the older PKCS #12 PBE schemes
//! (SHA-1 with 3DES or RC2), with the integrity MAC checked first so a
//! wrong password is reported as such. The password is never stored.

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockDecryptMut, InnerIvInit, KeyIvInit};
use ring::{digest as rd, hmac, pbkdf2};

use super::Error;
use super::key::SoftKey;
use crate::signatures::der::{self, Tlv};
use crate::signatures::x509::Cert;

const DATA: &str = "1.2.840.113549.1.7.1";
const ENCRYPTED_DATA: &str = "1.2.840.113549.1.7.6";
const KEY_BAG: &str = "1.2.840.113549.1.12.10.1.1";
const SHROUDED_KEY_BAG: &str = "1.2.840.113549.1.12.10.1.2";
const CERT_BAG: &str = "1.2.840.113549.1.12.10.1.3";
const X509_CERT: &str = "1.2.840.113549.1.9.22.1";
const LOCAL_KEY_ID: &str = "1.2.840.113549.1.9.21";
const FRIENDLY_NAME: &str = "1.2.840.113549.1.9.20";
const PBES2: &str = "1.2.840.113549.1.5.13";
const PBKDF2: &str = "1.2.840.113549.1.5.12";
const PBMAC1: &str = "1.2.840.113549.1.5.14";

/// What a PKCS #12 file holds.
pub struct P12 {
    /// The signer's certificate (the one with the private key).
    pub cert: Cert,
    /// The other certificates in the file (its chain, usually).
    pub chain: Vec<Cert>,
    pub key: SoftKey,
    /// The friendly name the file gives the key, if any.
    pub name: Option<String>,
}

struct Bag {
    kind: BagKind,
    local_id: Option<Vec<u8>>,
    name: Option<String>,
}

enum BagKind {
    Key(Vec<u8>),
    Cert(Cert),
}

/// Open a PKCS #12 file with `password`.
pub fn open(data: &[u8], password: &str) -> Result<P12, Error> {
    let bad = || Error::Key("not a readable PKCS #12 (.p12/.pfx) file".into());
    let pfx = Tlv::parse(data)
        .and_then(|t| t.expect(der::SEQUENCE))
        .map_err(|_| bad())?;
    let mut r = pfx.reader();
    r.read_tag(der::INTEGER).map_err(|_| bad())?;
    let auth = r.read_tag(der::SEQUENCE).map_err(|_| bad())?;
    let content = data_content(auth).ok_or_else(bad)?;
    let mac = r.read_opt(der::SEQUENCE).map_err(|_| bad())?;
    // Which password encoding the file uses: as given, or (OpenSSL's
    // quirk for empty passwords) none at all.
    let mut absent = false;
    if let Some(mac) = mac {
        if !check_mac(mac, &content, password, false)? {
            if password.is_empty() && check_mac(mac, &content, password, true)? {
                absent = true;
            } else {
                return Err(Error::WrongPassword);
            }
        }
    }
    let mut bags = Vec::new();
    let infos = Tlv::parse(&content)
        .and_then(|t| t.reader().all())
        .map_err(|_| bad())?;
    for ci in infos {
        let mut c = ci.reader();
        let ctype = c.read().and_then(|t| t.oid()).map_err(|_| bad())?;
        let safe = match ctype.as_str() {
            DATA => data_content(ci).ok_or_else(bad)?,
            ENCRYPTED_DATA => {
                let ed = c
                    .read_tag(der::ctx(0))
                    .and_then(|t| t.reader().read_tag(der::SEQUENCE))
                    .map_err(|_| bad())?;
                let mut e = ed.reader();
                e.read_tag(der::INTEGER).map_err(|_| bad())?;
                let eci = e.read_tag(der::SEQUENCE).map_err(|_| bad())?;
                let mut ec = eci.reader();
                ec.read().map_err(|_| bad())?;
                let alg = ec.read_tag(der::SEQUENCE).map_err(|_| bad())?;
                let Some(enc) = ec.read().ok() else {
                    continue;
                };
                let enc = enc.octets().map_err(|_| bad())?;
                decrypt(alg, &enc, password, absent)?
            }
            _ => continue,
        };
        read_bags(&safe, password, absent, &mut bags)?;
    }
    pair(bags)
}

/// The bytes of a ContentInfo of type data.
fn data_content(ci: Tlv<'_>) -> Option<Vec<u8>> {
    let mut r = ci.reader();
    if r.read().ok()?.oid().ok()? != DATA {
        return None;
    }
    r.read_tag(der::ctx(0))
        .ok()?
        .reader()
        .read()
        .ok()?
        .octets()
        .ok()
}

fn read_bags(safe: &[u8], password: &str, absent: bool, out: &mut Vec<Bag>) -> Result<(), Error> {
    let bad = || Error::Key("not a readable PKCS #12 (.p12/.pfx) file".into());
    let list = Tlv::parse(safe)
        .and_then(|t| t.reader().all())
        .map_err(|_| bad())?;
    for b in list {
        let mut r = b.reader();
        let id = r.read().and_then(|t| t.oid()).map_err(|_| bad())?;
        let value = r
            .read_tag(der::ctx(0))
            .and_then(|t| t.reader().read())
            .map_err(|_| bad())?;
        let mut local_id = None;
        let mut name = None;
        if let Ok(Some(attrs)) = r.read_opt(der::SET) {
            for a in attrs.reader().all().unwrap_or_default() {
                let mut ar = a.reader();
                let Ok(oid) = ar.read().and_then(|t| t.oid()) else {
                    continue;
                };
                let Some(v) = ar
                    .read_tag(der::SET)
                    .ok()
                    .and_then(|s| s.reader().read().ok())
                else {
                    continue;
                };
                match oid.as_str() {
                    LOCAL_KEY_ID => local_id = Some(v.body.to_vec()),
                    FRIENDLY_NAME => name = Some(v.text()),
                    _ => {}
                }
            }
        }
        let kind = match id.as_str() {
            KEY_BAG => BagKind::Key(value.raw.to_vec()),
            SHROUDED_KEY_BAG => {
                let mut e = value.reader();
                let alg = e.read_tag(der::SEQUENCE).map_err(|_| bad())?;
                let enc = e.read().and_then(|t| t.octets()).map_err(|_| bad())?;
                BagKind::Key(decrypt(alg, &enc, password, absent)?)
            }
            CERT_BAG => {
                let mut c = value.reader();
                if c.read().and_then(|t| t.oid()).ok().as_deref() != Some(X509_CERT) {
                    continue;
                }
                let der_cert = c
                    .read_tag(der::ctx(0))
                    .and_then(|t| t.reader().read())
                    .and_then(|t| t.octets())
                    .map_err(|_| bad())?;
                match Cert::parse(&der_cert) {
                    Ok(cert) => BagKind::Cert(cert),
                    Err(_) => continue,
                }
            }
            _ => continue,
        };
        out.push(Bag {
            kind,
            local_id,
            name,
        });
    }
    Ok(())
}

/// Match the key with its certificate.
fn pair(bags: Vec<Bag>) -> Result<P12, Error> {
    let mut keys = Vec::new();
    let mut certs = Vec::new();
    for b in bags {
        match b.kind {
            BagKind::Key(k) => keys.push((k, b.local_id, b.name)),
            BagKind::Cert(c) => certs.push((c, b.local_id, b.name)),
        }
    }
    if keys.is_empty() {
        return Err(Error::Key(
            "the file has no private key (only certificates)".into(),
        ));
    }
    let mut last_err = None;
    for (k, kid, kname) in &keys {
        // The certificate with the same local key id first, then any.
        let mut order: Vec<usize> = (0..certs.len()).collect();
        order.sort_by_key(|&i| (kid.is_none() || certs[i].1 != *kid, certs[i].0.is_ca()));
        for i in order {
            match SoftKey::from_pkcs8(k, &certs[i].0) {
                Ok(key) => {
                    let (cert, _, cname) = certs.remove(i);
                    return Ok(P12 {
                        cert,
                        chain: certs.into_iter().map(|c| c.0).collect(),
                        key,
                        name: kname.clone().or(cname),
                    });
                }
                Err(e) => last_err = Some(e),
            }
        }
    }
    Err(last_err
        .unwrap_or_else(|| Error::Key("the file has no certificate for its private key".into())))
}

// --- Integrity MAC ---

fn check_mac(mac: Tlv<'_>, content: &[u8], password: &str, absent: bool) -> Result<bool, Error> {
    let bad = || Error::Key("the file's integrity check is unreadable".into());
    let mut r = mac.reader();
    let di = r.read_tag(der::SEQUENCE).map_err(|_| bad())?;
    let mut d = di.reader();
    let alg = d.read_tag(der::SEQUENCE).map_err(|_| bad())?;
    let alg_oid = alg
        .reader()
        .read()
        .and_then(|t| t.oid())
        .map_err(|_| bad())?;
    let want = d.read_tag(der::OCTET_STRING).map_err(|_| bad())?.body;
    let salt = r.read_tag(der::OCTET_STRING).map_err(|_| bad())?.body;
    let iter = match r.read_opt(der::INTEGER) {
        Ok(Some(i)) => i.small_int().map_err(|_| bad())?,
        _ => 1,
    };
    if alg_oid == PBMAC1 {
        return Err(Error::Unsupported(
            "PKCS #12 files protected with PBMAC1".into(),
        ));
    }
    let (halg, len): (hmac::Algorithm, usize) = match alg_oid.as_str() {
        "1.3.14.3.2.26" => (hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, 20),
        "2.16.840.1.101.3.4.2.1" => (hmac::HMAC_SHA256, 32),
        "2.16.840.1.101.3.4.2.2" => (hmac::HMAC_SHA384, 48),
        "2.16.840.1.101.3.4.2.3" => (hmac::HMAC_SHA512, 64),
        o => return Err(Error::Unsupported(format!("PKCS #12 integrity digest {o}"))),
    };
    let key = p12_kdf(&alg_oid, password, absent, salt, iter, 3, len)?;
    let k = hmac::Key::new(halg, &key);
    Ok(hmac::verify(&k, content, want).is_ok())
}

/// The PKCS #12 key derivation (RFC 7292 appendix B.2).
fn p12_kdf(
    hash_oid: &str,
    password: &str,
    absent: bool,
    salt: &[u8],
    iterations: u64,
    id: u8,
    n: usize,
) -> Result<Vec<u8>, Error> {
    let (alg, v): (&'static rd::Algorithm, usize) = match hash_oid {
        "1.3.14.3.2.26" => (&rd::SHA1_FOR_LEGACY_USE_ONLY, 64),
        "2.16.840.1.101.3.4.2.1" => (&rd::SHA256, 64),
        "2.16.840.1.101.3.4.2.2" => (&rd::SHA384, 128),
        "2.16.840.1.101.3.4.2.3" => (&rd::SHA512, 128),
        o => return Err(Error::Unsupported(format!("PKCS #12 digest {o}"))),
    };
    if iterations == 0 || iterations > 10_000_000 {
        return Err(Error::Key("bad PKCS #12 iteration count".into()));
    }
    // The password as a NUL-terminated big-endian BMPString.
    let mut pw = Vec::new();
    if !absent {
        for u in password.encode_utf16() {
            pw.extend_from_slice(&u.to_be_bytes());
        }
        pw.extend_from_slice(&[0, 0]);
    }
    let fill = |src: &[u8]| -> Vec<u8> {
        if src.is_empty() {
            return Vec::new();
        }
        let len = v * src.len().div_ceil(v);
        src.iter().copied().cycle().take(len).collect()
    };
    let mut i = fill(salt);
    i.extend(fill(&pw));
    let d = vec![id; v];
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let mut a = {
            let mut c = rd::Context::new(alg);
            c.update(&d);
            c.update(&i);
            c.finish().as_ref().to_vec()
        };
        for _ in 1..iterations {
            a = rd::digest(alg, &a).as_ref().to_vec();
        }
        out.extend_from_slice(&a);
        if out.len() >= n {
            break;
        }
        // I_j = (I_j + B + 1) mod 2^(v*8)
        let b: Vec<u8> = a.iter().copied().cycle().take(v).collect();
        for chunk in i.chunks_mut(v) {
            let mut carry = 1u16;
            for k in (0..v).rev() {
                let s = chunk[k] as u16 + b[k] as u16 + carry;
                chunk[k] = s as u8;
                carry = s >> 8;
            }
        }
    }
    out.truncate(n);
    Ok(out)
}

// --- Decryption ---

fn decrypt(alg: Tlv<'_>, data: &[u8], password: &str, absent: bool) -> Result<Vec<u8>, Error> {
    let bad = || Error::Key("the file's encryption is unreadable".into());
    let mut r = alg.reader();
    let oid = r.read().and_then(|t| t.oid()).map_err(|_| bad())?;
    let params = r.read().map_err(|_| bad())?;
    // Without a MAC, bad padding is the only sign of a wrong password.
    let wrong = || Error::WrongPassword;
    if oid == PBES2 {
        let mut p = params.reader();
        let kdf = p.read_tag(der::SEQUENCE).map_err(|_| bad())?;
        let scheme = p.read_tag(der::SEQUENCE).map_err(|_| bad())?;
        let mut k = kdf.reader();
        if k.read().and_then(|t| t.oid()).map_err(|_| bad())? != PBKDF2 {
            return Err(Error::Unsupported(
                "this PKCS #12 key derivation (only PBKDF2 is supported)".into(),
            ));
        }
        let kp = k.read_tag(der::SEQUENCE).map_err(|_| bad())?;
        let mut kp = kp.reader();
        let salt = kp.read_tag(der::OCTET_STRING).map_err(|_| bad())?.body;
        let iter = kp
            .read_tag(der::INTEGER)
            .and_then(|t| t.small_int())
            .map_err(|_| bad())?;
        let _key_len = kp.read_opt(der::INTEGER).map_err(|_| bad())?;
        let prf = match kp.read_opt(der::SEQUENCE).map_err(|_| bad())? {
            Some(a) => a.reader().read().and_then(|t| t.oid()).map_err(|_| bad())?,
            None => "1.2.840.113549.2.7".into(),
        };
        let prf = match prf.as_str() {
            "1.2.840.113549.2.7" => pbkdf2::PBKDF2_HMAC_SHA1,
            "1.2.840.113549.2.9" => pbkdf2::PBKDF2_HMAC_SHA256,
            "1.2.840.113549.2.10" => pbkdf2::PBKDF2_HMAC_SHA384,
            "1.2.840.113549.2.11" => pbkdf2::PBKDF2_HMAC_SHA512,
            o => return Err(Error::Unsupported(format!("PBKDF2 with {o}"))),
        };
        let mut s = scheme.reader();
        let cipher = s.read().and_then(|t| t.oid()).map_err(|_| bad())?;
        let iv = s
            .read_tag(der::OCTET_STRING)
            .map_err(|_| bad())?
            .body
            .to_vec();
        let key_len = match cipher.as_str() {
            "2.16.840.1.101.3.4.1.2" => 16,
            "2.16.840.1.101.3.4.1.22" => 24,
            "2.16.840.1.101.3.4.1.42" => 32,
            "1.2.840.113549.3.7" => 24,
            o => return Err(Error::Unsupported(format!("PKCS #12 cipher {o}"))),
        };
        let iter = std::num::NonZeroU32::new(iter.min(u32::MAX as u64) as u32).ok_or_else(bad)?;
        let mut key = vec![0u8; key_len];
        pbkdf2::derive(prf, iter, salt, password.as_bytes(), &mut key);
        return cbc_decrypt(&cipher, &key, &iv, data).ok_or_else(wrong);
    }
    // PKCS #12 PBE: SHA-1 with 3DES or RC2.
    let (key_len, cipher) = match oid.as_str() {
        "1.2.840.113549.1.12.1.3" => (24, "3des"),
        "1.2.840.113549.1.12.1.4" => (16, "2des"),
        "1.2.840.113549.1.12.1.5" => (16, "rc2-128"),
        "1.2.840.113549.1.12.1.6" => (5, "rc2-40"),
        o => return Err(Error::Unsupported(format!("PKCS #12 encryption {o}"))),
    };
    let mut p = params.reader();
    let salt = p.read_tag(der::OCTET_STRING).map_err(|_| bad())?.body;
    let iter = p
        .read_tag(der::INTEGER)
        .and_then(|t| t.small_int())
        .map_err(|_| bad())?;
    let sha1 = "1.3.14.3.2.26";
    let mut key = p12_kdf(sha1, password, absent, salt, iter, 1, key_len)?;
    let iv = p12_kdf(sha1, password, absent, salt, iter, 2, 8)?;
    let out = match cipher {
        "3des" => cbc_decrypt("1.2.840.113549.3.7", &key, &iv, data),
        "2des" => {
            let k1 = key[..8].to_vec();
            key.extend_from_slice(&k1);
            cbc_decrypt("1.2.840.113549.3.7", &key, &iv, data)
        }
        "rc2-128" => rc2_decrypt(&key, 128, &iv, data),
        _ => rc2_decrypt(&key, 40, &iv, data),
    };
    out.ok_or_else(wrong)
}

fn cbc_decrypt(cipher: &str, key: &[u8], iv: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    match cipher {
        "2.16.840.1.101.3.4.1.2" => cbc::Decryptor::<aes::Aes128>::new_from_slices(key, iv)
            .ok()?
            .decrypt_padded_vec_mut::<Pkcs7>(data)
            .ok(),
        "2.16.840.1.101.3.4.1.22" => cbc::Decryptor::<aes::Aes192>::new_from_slices(key, iv)
            .ok()?
            .decrypt_padded_vec_mut::<Pkcs7>(data)
            .ok(),
        "2.16.840.1.101.3.4.1.42" => cbc::Decryptor::<aes::Aes256>::new_from_slices(key, iv)
            .ok()?
            .decrypt_padded_vec_mut::<Pkcs7>(data)
            .ok(),
        "1.2.840.113549.3.7" => cbc::Decryptor::<des::TdesEde3>::new_from_slices(key, iv)
            .ok()?
            .decrypt_padded_vec_mut::<Pkcs7>(data)
            .ok(),
        _ => None,
    }
}

fn rc2_decrypt(key: &[u8], bits: usize, iv: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    let c = rc2::Rc2::new_with_eff_key_len(key, bits);
    cbc::Decryptor::<rc2::Rc2>::inner_iv_slice_init(c, iv)
        .ok()?
        .decrypt_padded_vec_mut::<Pkcs7>(data)
        .ok()
}
