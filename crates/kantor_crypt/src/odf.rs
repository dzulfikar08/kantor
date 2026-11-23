//! Encrypted OpenDocument packages (ODF 1.2 part 3, §3.8 and 4.8; ODF 1.4
//! "wholesome" encryption): each encrypted entry is deflated, then
//! encrypted with a key derived from the password, and described in
//! `META-INF/manifest.xml`; or the whole package is one deflated and
//! encrypted `encrypted-package` entry.

use std::io::{Cursor, Read, Seek, Write as _};

use crate::prim::{self, Aes, Hash};
use crate::{Error, damaged};

const MANIFEST: &str = "META-INF/manifest.xml";
const WHOLE: &str = "encrypted-package";
/// Larger manifests aren't Office files.
const MAX_MANIFEST: u64 = 16 << 20;

/// Whether the zip in `r` is an encrypted ODF package (central directory
/// and manifest only).
pub fn sniff<R: Read + Seek>(r: &mut R) -> bool {
    let Ok(mut z) = zip::ZipArchive::new(r) else {
        return false;
    };
    if z.index_for_name(WHOLE).is_some() {
        return true;
    }
    let Ok(m) = z.by_name(MANIFEST) else {
        return false;
    };
    if m.size() > MAX_MANIFEST {
        return false;
    }
    let mut s = String::new();
    m.take(MAX_MANIFEST).read_to_string(&mut s).is_ok() && s.contains("encryption-data")
}

/// How one entry is encrypted.
#[derive(Debug, Clone)]
struct Enc {
    path: String,
    size: Option<usize>,
    checksum: Option<(Checksum, Vec<u8>)>,
    algorithm: Algorithm,
    iv: Vec<u8>,
    start_key: Hash,
    kdf: Kdf,
    key_size: usize,
    salt: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Checksum {
    /// The hash of the first 1024 bytes.
    First1k(Hash),
    /// The hash of everything.
    Whole(Hash),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Algorithm {
    BlowfishCfb,
    AesCbc,
    AesGcm,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kdf {
    Pbkdf2 { rounds: u32 },
    Argon2id { t: u32, m: u32, lanes: u32 },
}

const MANIFEST_NS: &str = "urn:oasis:names:tc:opendocument:xmlns:manifest:1.0";

/// A manifest attribute, or one of that name in any namespace (LibreOffice
/// puts the Argon2 parameters in its loext namespace).
fn m<'a>(n: roxmltree::Node<'a, '_>, a: &str) -> Option<&'a str> {
    n.attribute((MANIFEST_NS, a))
        .or_else(|| n.attributes().find(|x| x.name() == a).map(|x| x.value()))
}

fn parse_manifest(xml: &str) -> Result<Vec<Enc>, Error> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| damaged(format!("manifest: {e}")))?;
    let mut out = Vec::new();
    for entry in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "file-entry")
    {
        let Some(data) = entry
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == "encryption-data")
        else {
            continue;
        };
        let path = m(entry, "full-path")
            .ok_or_else(|| damaged("manifest entry without a path"))?
            .to_owned();
        let size = m(entry, "size").and_then(|s| s.trim().parse().ok());
        let child = |name: &str| {
            data.children()
                .find(|n| n.is_element() && n.tag_name().name() == name)
        };
        let alg = child("algorithm").ok_or_else(|| damaged("no encryption algorithm"))?;
        let alg_name = m(alg, "algorithm-name").unwrap_or_default();
        let algorithm = match alg_name {
            "Blowfish CFB" => Algorithm::BlowfishCfb,
            "http://www.w3.org/2001/04/xmlenc#aes256-cbc"
            | "http://www.w3.org/2001/04/xmlenc#aes192-cbc"
            | "http://www.w3.org/2001/04/xmlenc#aes128-cbc" => Algorithm::AesCbc,
            "http://www.w3.org/2009/xmlenc11#aes256-gcm" => Algorithm::AesGcm,
            other => return Err(Error::Unsupported(format!("{other} encryption"))),
        };
        let iv = prim::b64(m(alg, "initialisation-vector").unwrap_or_default())?;
        let start_key = match child("start-key-generation") {
            None => Hash::Sha1,
            Some(n) => {
                let name = m(n, "start-key-generation-name").unwrap_or_default();
                let short = name.rsplit('#').next().unwrap_or(name);
                Hash::from_name(short)
                    .ok_or_else(|| Error::Unsupported(format!("{name} start keys")))?
            }
        };
        let kd = child("key-derivation").ok_or_else(|| damaged("no key derivation"))?;
        let kd_name = m(kd, "key-derivation-name").unwrap_or_default();
        let num = |a: &str, default: u32| -> Result<u32, Error> {
            m(kd, a).map_or(Ok(default), |v| {
                v.trim()
                    .parse()
                    .map_err(|_| damaged(format!("manifest: {a}")))
            })
        };
        let kdf = if kd_name == "PBKDF2" {
            Kdf::Pbkdf2 {
                rounds: num("iteration-count", 1024)?,
            }
        } else if kd_name.ends_with("argon2id") {
            Kdf::Argon2id {
                t: num("argon2-iterations", 3)?,
                m: num("argon2-memory", 65536)?,
                lanes: num("argon2-lanes", 4)?,
            }
        } else {
            return Err(Error::Unsupported(format!("{kd_name} key derivation")));
        };
        if let Kdf::Pbkdf2 { rounds } = kdf
            && rounds > 10_000_000
        {
            return Err(Error::Unsupported("too many PBKDF2 rounds".into()));
        }
        let key_size = num("key-size", 16)? as usize;
        if !(1..=64).contains(&key_size) {
            return Err(damaged("manifest: key size"));
        }
        let salt = prim::b64(m(kd, "salt").unwrap_or_default())?;
        let checksum = match (m(data, "checksum-type"), m(data, "checksum")) {
            (Some(t), Some(c)) => {
                let t = t.rsplit('#').next().unwrap_or(t).to_ascii_lowercase();
                let kind = match t.as_str() {
                    "sha1/1k" | "sha1-1k" => Some(Checksum::First1k(Hash::Sha1)),
                    "sha256-1k" => Some(Checksum::First1k(Hash::Sha256)),
                    "sha1" => Some(Checksum::Whole(Hash::Sha1)),
                    "sha256" => Some(Checksum::Whole(Hash::Sha256)),
                    _ => None,
                };
                match kind {
                    Some(k) => Some((k, prim::b64(c)?)),
                    None => None,
                }
            }
            _ => None,
        };
        out.push(Enc {
            path,
            size,
            checksum,
            algorithm,
            iv,
            start_key,
            kdf,
            key_size,
            salt,
        });
    }
    Ok(out)
}

impl Enc {
    fn key(&self, password: &str) -> Result<Vec<u8>, Error> {
        let start = self.start_key.digest(&[password.as_bytes()]);
        match self.kdf {
            Kdf::Pbkdf2 { rounds } => {
                Ok(prim::pbkdf2_sha1(&start, &self.salt, rounds, self.key_size))
            }
            Kdf::Argon2id { t, m, lanes } => {
                prim::argon2id(&start, &self.salt, t, m, lanes, self.key_size)
            }
        }
    }

    /// The entry's plain contents (or, with `check_only`, just whether the
    /// password is right).
    fn open(&self, key: &[u8], data: &[u8], check_only: bool) -> Result<Vec<u8>, Error> {
        let mut d = data.to_vec();
        match self.algorithm {
            Algorithm::BlowfishCfb => prim::blowfish_cfb_decrypt(key, &self.iv, &mut d)?,
            Algorithm::AesCbc => {
                if !d.len().is_multiple_of(16) {
                    return Err(damaged(format!("{}: not whole AES blocks", self.path)));
                }
                Aes::new(key)?.cbc_decrypt(&self.iv, &mut d)?;
                // W3C XML Encryption padding: the last byte counts the pad.
                let pad = usize::from(*d.last().unwrap_or(&0));
                if (1..=16).contains(&pad) && pad <= d.len() {
                    d.truncate(d.len() - pad);
                } else if self.checksum.is_none() {
                    return Err(Error::WrongPassword);
                }
            }
            Algorithm::AesGcm => {
                // W3C XML Encryption 1.1: the nonce, then the ciphertext and
                // the tag (the nonce repeats the manifest's IV).
                let n = self.iv.len();
                if d.len() < n + 16 {
                    return Err(damaged(format!("{}: too short", self.path)));
                }
                d = prim::aes_gcm_decrypt(key, &d[..n], &d[n..])?;
            }
        }
        if let Some((kind, want)) = &self.checksum {
            let got = match kind {
                Checksum::First1k(h) => h.digest(&[&d[..d.len().min(1024)]]),
                Checksum::Whole(h) => h.digest(&[&d]),
            };
            if &got != want {
                return Err(Error::WrongPassword);
            }
        }
        if check_only {
            return Ok(Vec::new());
        }
        // Deflated before encryption, except entries that were stored
        // (pictures): those are their size already.
        match inflate(&d, self.size) {
            Some(plain) => Ok(plain),
            None if self.size.is_none_or(|s| s == d.len()) => Ok(d),
            None if self.checksum.is_none() => Err(Error::WrongPassword),
            None => Err(damaged(format!("{} can't be inflated", self.path))),
        }
    }
}

/// Raw deflate, checked against the expected size when there is one.
fn inflate(data: &[u8], size: Option<usize>) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(size.unwrap_or(data.len() * 3));
    let limit = size.map_or(1 << 31, |s| s as u64 + 1);
    flate2::read::DeflateDecoder::new(data)
        .take(limit)
        .read_to_end(&mut out)
        .ok()?;
    if size.is_some_and(|s| s != out.len()) {
        return None;
    }
    Some(out)
}

/// Read an entry's stored bytes (encrypted entries are stored).
fn raw_entry<R: Read + Seek>(z: &mut zip::ZipArchive<R>, name: &str) -> Result<Vec<u8>, Error> {
    let i = z
        .index_for_name(name)
        .ok_or_else(|| damaged(format!("{name} is missing")))?;
    let stored = z
        .by_index_raw(i)
        .map(|f| f.compression() == zip::CompressionMethod::Stored)
        .map_err(|e| damaged(format!("{name}: {e}")))?;
    let mut v = Vec::new();
    let res = if stored {
        z.by_index_raw(i)
            .and_then(|mut f| Ok(f.read_to_end(&mut v)?))
    } else {
        z.by_index(i).and_then(|mut f| Ok(f.read_to_end(&mut v)?))
    };
    res.map_err(|e| damaged(format!("{name}: {e}")))?;
    Ok(v)
}

fn read_entry<R: Read + Seek>(z: &mut zip::ZipArchive<R>, i: usize) -> Result<Vec<u8>, Error> {
    let mut v = Vec::new();
    z.by_index(i)
        .and_then(|mut f| Ok(f.read_to_end(&mut v)?))
        .map_err(|e| damaged(format!("entry {i}: {e}")))?;
    Ok(v)
}

/// The plain package (`None` with `check_only`, once the password checks).
pub fn decrypt(bytes: &[u8], password: &str, check_only: bool) -> Result<Option<Vec<u8>>, Error> {
    let mut z =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| damaged(format!("zip: {e}")))?;
    let manifest = {
        let i = z
            .index_for_name(MANIFEST)
            .ok_or_else(|| damaged("no manifest"))?;
        let b = read_entry(&mut z, i)?;
        String::from_utf8(b).map_err(|_| damaged("manifest isn't UTF-8"))?
    };
    let encs = parse_manifest(&manifest)?;
    if encs.is_empty() {
        return Err(damaged("no encrypted entries"));
    }

    // ODF 1.4: one encrypted package holding the real one.
    if let Some(whole) = encs.iter().find(|e| e.path == WHOLE) {
        let key = whole.key(password)?;
        let data = raw_entry(&mut z, WHOLE)?;
        let inner = whole.open(&key, &data, check_only)?;
        if check_only {
            return Ok(None);
        }
        if !inner.starts_with(b"PK") {
            return Err(damaged("the encrypted package isn't a zip"));
        }
        return Ok(Some(inner));
    }

    // Per entry: every entry has its own salt and IV. Check the password on
    // the smallest one first (cheap, and a wrong password stops there).
    let mut order: Vec<&Enc> = encs.iter().collect();
    order.sort_by_key(|e| e.size.unwrap_or(usize::MAX));
    let mut plain: std::collections::HashMap<&str, Vec<u8>> = Default::default();
    for e in order {
        let data = match raw_entry(&mut z, &e.path) {
            Ok(d) => d,
            // Listed but not there (LibreOffice lists some it leaves out).
            Err(_) if z.index_for_name(&e.path).is_none() => continue,
            Err(err) => return Err(err),
        };
        let key = e.key(password)?;
        let p = e.open(&key, &data, check_only)?;
        if check_only {
            return Ok(None);
        }
        plain.insert(e.path.as_str(), p);
    }

    // A new zip: the plain entries, the others as they were, and the
    // manifest without its encryption data.
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let deflated = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let io = |e: zip::result::ZipError| damaged(format!("writing the package: {e}"));
    for i in 0..z.len() {
        let (name, is_dir) = {
            let f = z
                .by_index_raw(i)
                .map_err(|e| damaged(format!("entry {i}: {e}")))?;
            (
                f.name()
                    .map_err(|e| damaged(format!("entry {i}: {e}")))?
                    .into_owned(),
                f.is_dir(),
            )
        };
        if is_dir {
            w.add_directory(name, stored).map_err(io)?;
            continue;
        }
        let data = if name == MANIFEST {
            strip_encryption_data(&manifest).into_bytes()
        } else if let Some(p) = plain.remove(name.as_str()) {
            p
        } else {
            read_entry(&mut z, i)?
        };
        let opts = if name == "mimetype" { stored } else { deflated };
        w.start_file(name, opts).map_err(io)?;
        w.write_all(&data)
            .map_err(|e| damaged(format!("writing the package: {e}")))?;
    }
    Ok(Some(w.finish().map_err(io)?.into_inner()))
}

/// The manifest without its `encryption-data` elements.
fn strip_encryption_data(xml: &str) -> String {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return xml.to_owned();
    };
    let mut cut: Vec<std::ops::Range<usize>> = doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "encryption-data")
        .map(|n| n.range())
        .collect();
    cut.sort_by_key(|r| r.start);
    let mut out = String::with_capacity(xml.len());
    let mut at = 0;
    for r in cut {
        if r.start < at {
            continue;
        }
        out.push_str(&xml[at..r.start]);
        at = r.end;
    }
    out.push_str(&xml[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn manifest_loses_its_encryption_data() {
        let xml = r#"<m:manifest xmlns:m="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><m:file-entry m:full-path="content.xml"><m:encryption-data m:checksum="x"><m:algorithm/></m:encryption-data></m:file-entry></m:manifest>"#;
        let out = strip_encryption_data(xml);
        assert!(!out.contains("encryption-data"));
        assert!(out.contains(r#"m:full-path="content.xml"></m:file-entry>"#));
    }

    #[std::prelude::v1::test]
    fn unknown_algorithms_are_unsupported() {
        let xml = r#"<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><manifest:file-entry manifest:full-path="content.xml"><manifest:encryption-data><manifest:algorithm manifest:algorithm-name="urn:rot13" manifest:initialisation-vector=""/><manifest:key-derivation manifest:key-derivation-name="PBKDF2" manifest:salt=""/></manifest:encryption-data></manifest:file-entry></manifest:manifest>"#;
        assert!(matches!(parse_manifest(xml), Err(Error::Unsupported(_))));
    }

    #[std::prelude::v1::test]
    fn inflate_checks_the_size() {
        let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(b"hello hello hello").unwrap();
        let d = e.finish().unwrap();
        assert_eq!(inflate(&d, Some(17)).unwrap(), b"hello hello hello");
        assert!(inflate(&d, Some(3)).is_none());
        assert!(inflate(b"\xff\xff\xff", None).is_none());
    }
}
