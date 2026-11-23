//! Encrypted OOXML packages ([MS-OFFCRYPTO] 2.3.4): Agile and Standard
//! encryption read, Agile written.

use std::io::{Cursor, Read as _, Write as _};

use crate::prim::{self, Aes, Hash};
use crate::{Error, damaged};

/// Office 2013's spin count.
pub const SPIN_COUNT: u32 = 100_000;
/// Segment size of the encrypted package (Agile).
const SEGMENT: usize = 4096;

const BLK_VERIFIER_INPUT: [u8; 8] = [0xfe, 0xa7, 0xd2, 0x76, 0x3b, 0x4b, 0x9e, 0x79];
const BLK_VERIFIER_VALUE: [u8; 8] = [0xd7, 0xaa, 0x0f, 0x6d, 0x30, 0x61, 0x34, 0x4e];
const BLK_KEY: [u8; 8] = [0x14, 0x6e, 0x0b, 0xe7, 0xab, 0xac, 0xd0, 0xd6];
const BLK_HMAC_KEY: [u8; 8] = [0x5f, 0xb2, 0xad, 0x01, 0x0c, 0xb9, 0xe1, 0xf6];
const BLK_HMAC_VALUE: [u8; 8] = [0xa0, 0x67, 0x7f, 0x02, 0xb2, 0x2c, 0x84, 0x33];

/// The two streams that matter.
struct Container {
    info: Vec<u8>,
    package: Vec<u8>,
}

fn read_container(bytes: &[u8]) -> Result<Container, Error> {
    let mut c = cfb::CompoundFile::open(Cursor::new(bytes))
        .map_err(|e| damaged(format!("compound file: {e}")))?;
    let mut read = |name: &str| -> Result<Vec<u8>, Error> {
        let mut s = c
            .open_stream(name)
            .map_err(|_| damaged(format!("no {name} stream")))?;
        let mut v = Vec::new();
        s.read_to_end(&mut v)
            .map_err(|e| damaged(format!("{name}: {e}")))?;
        Ok(v)
    };
    Ok(Container {
        info: read("/EncryptionInfo")?,
        package: read("/EncryptedPackage")?,
    })
}

enum Info {
    Agile(Agile),
    Standard(Standard),
}

fn parse_info(info: &[u8]) -> Result<Info, Error> {
    if info.len() < 8 {
        return Err(damaged("EncryptionInfo too short"));
    }
    let major = u16::from_le_bytes([info[0], info[1]]);
    let minor = u16::from_le_bytes([info[2], info[3]]);
    let flags = u32::from_le_bytes([info[4], info[5], info[6], info[7]]);
    match (major, minor) {
        (4, 4) => Agile::parse(&info[8..]).map(Info::Agile),
        (2..=4, 2) => {
            const F_CRYPTOAPI: u32 = 0x04;
            const F_EXTERNAL: u32 = 0x10;
            const F_AES: u32 = 0x20;
            if flags & F_EXTERNAL != 0 {
                return Err(Error::Unsupported("external encryption".into()));
            }
            if flags & F_AES == 0 || flags & F_CRYPTOAPI == 0 {
                return Err(Error::Unsupported("RC4 CryptoAPI encryption".into()));
            }
            Standard::parse(&info[8..]).map(Info::Standard)
        }
        (3 | 4, 3) => Err(Error::Unsupported("Extensible encryption".into())),
        _ => Err(Error::Unsupported(format!(
            "encryption version {major}.{minor}"
        ))),
    }
}

pub fn decrypt(bytes: &[u8], password: &str) -> Result<Vec<u8>, Error> {
    let c = read_container(bytes)?;
    match parse_info(&c.info)? {
        Info::Agile(a) => {
            let key = a.secret_key(password)?;
            a.check_integrity(&key, &c.package)?;
            a.decrypt_package(&key, &c.package)
        }
        Info::Standard(s) => {
            let key = s.key(password)?;
            s.decrypt_package(&key, &c.package)
        }
    }
}

pub fn check(bytes: &[u8], password: &str) -> Result<(), Error> {
    let c = read_container(bytes)?;
    match parse_info(&c.info)? {
        Info::Agile(a) => a.secret_key(password).map(|_| ()),
        Info::Standard(s) => s.key(password).map(|_| ()),
    }
}

/// The plain size at the start of an encrypted package.
fn package_size(package: &[u8]) -> Result<usize, Error> {
    let size = package
        .get(..8)
        .ok_or_else(|| damaged("EncryptedPackage too short"))?;
    let size = u64::from_le_bytes(size.try_into().unwrap());
    let size = usize::try_from(size).map_err(|_| damaged("EncryptedPackage size"))?;
    if size > package.len() - 8 {
        return Err(damaged("EncryptedPackage shorter than its size"));
    }
    Ok(size)
}

// ── Agile ─────────────────────────────────────────────────────────────────

/// Cipher parameters of `keyData` or of the password key encryptor.
#[derive(Debug, Clone)]
struct Params {
    salt: Vec<u8>,
    hash: Hash,
    block: usize,
    key_bytes: usize,
}

impl Params {
    fn parse(n: roxmltree::Node) -> Result<Params, Error> {
        let attr = |name: &str| {
            n.attribute(name)
                .ok_or_else(|| damaged(format!("EncryptionInfo: no {name}")))
        };
        let num = |name: &str| -> Result<usize, Error> {
            attr(name)?
                .trim()
                .parse()
                .map_err(|_| damaged(format!("EncryptionInfo: {name}")))
        };
        let cipher = attr("cipherAlgorithm")?;
        if cipher != "AES" {
            return Err(Error::Unsupported(format!("{cipher} encryption")));
        }
        let chaining = attr("cipherChaining")?;
        if chaining != "ChainingModeCBC" {
            return Err(Error::Unsupported(format!("{chaining} chaining")));
        }
        let hash_name = attr("hashAlgorithm")?;
        let hash = Hash::from_name(hash_name)
            .ok_or_else(|| Error::Unsupported(format!("{hash_name} hashing")))?;
        let block = num("blockSize")?;
        let key_bits = num("keyBits")?;
        if block != 16 || !matches!(key_bits, 128 | 192 | 256) {
            return Err(Error::Unsupported(format!("{key_bits}-bit AES")));
        }
        Ok(Params {
            salt: prim::b64(attr("saltValue")?)?,
            hash,
            block,
            key_bytes: key_bits / 8,
        })
    }

    /// H(salt + block key), cut or padded with 0x36 to the block size.
    fn iv(&self, block_key: &[u8]) -> Vec<u8> {
        fit(self.hash.digest(&[&self.salt, block_key]), self.block)
    }
}

/// Cut `v` to `len` bytes, or pad it with 0x36.
fn fit(mut v: Vec<u8>, len: usize) -> Vec<u8> {
    v.resize(len, 0x36);
    v
}

struct Agile {
    key_data: Params,
    hmac_key: Option<Vec<u8>>,
    hmac_value: Option<Vec<u8>>,
    pw: Params,
    spin: u32,
    verifier_input: Vec<u8>,
    verifier_value: Vec<u8>,
    key_value: Vec<u8>,
}

impl Agile {
    fn parse(xml: &[u8]) -> Result<Agile, Error> {
        let text = std::str::from_utf8(xml).map_err(|_| damaged("EncryptionInfo isn't UTF-8"))?;
        let text = text.trim_start_matches('\u{feff}');
        let doc = roxmltree::Document::parse(text)
            .map_err(|e| damaged(format!("EncryptionInfo XML: {e}")))?;
        let find = |name: &str| {
            doc.descendants()
                .find(|n| n.is_element() && n.tag_name().name() == name)
        };
        let key_data = Params::parse(find("keyData").ok_or_else(|| damaged("no keyData"))?)?;
        let integrity = find("dataIntegrity");
        let get = |n: Option<roxmltree::Node>, a: &str| -> Result<Option<Vec<u8>>, Error> {
            n.and_then(|n| n.attribute(a)).map(prim::b64).transpose()
        };
        let hmac_key = get(integrity, "encryptedHmacKey")?;
        let hmac_value = get(integrity, "encryptedHmacValue")?;
        // The password key encryptor (certificate ones are skipped).
        let ek = doc
            .descendants()
            .find(|n| {
                n.is_element()
                    && n.tag_name().name() == "encryptedKey"
                    && n.attribute("encryptedKeyValue").is_some()
                    && n.attribute("spinCount").is_some()
            })
            .ok_or_else(|| Error::Unsupported("certificate encryption".into()))?;
        let pw = Params::parse(ek)?;
        let spin: u32 = ek
            .attribute("spinCount")
            .unwrap_or_default()
            .parse()
            .map_err(|_| damaged("spinCount"))?;
        if spin > 10_000_000 {
            return Err(Error::Unsupported("spin count too large".into()));
        }
        let need = |a: &str| -> Result<Vec<u8>, Error> {
            prim::b64(ek.attribute(a).ok_or_else(|| damaged(format!("no {a}")))?)
        };
        Ok(Agile {
            key_data,
            hmac_key,
            hmac_value,
            spin,
            verifier_input: need("encryptedVerifierHashInput")?,
            verifier_value: need("encryptedVerifierHashValue")?,
            key_value: need("encryptedKeyValue")?,
            pw,
        })
    }

    /// The intermediate key of `password` (spun hash, MS-OFFCRYPTO 2.3.4.11).
    fn spun(&self, password: &str) -> Vec<u8> {
        spin_hash(self.pw.hash, &self.pw.salt, password, self.spin)
    }

    fn derive(&self, spun: &[u8], block_key: &[u8]) -> Vec<u8> {
        fit(self.pw.hash.digest(&[spun, block_key]), self.pw.key_bytes)
    }

    fn decrypt_with(&self, key: &[u8], data: &[u8]) -> Result<Vec<u8>, Error> {
        let mut out = data.to_vec();
        if !out.len().is_multiple_of(16) {
            return Err(damaged("encrypted key size"));
        }
        // The IV is the encryptor's salt (MS-OFFCRYPTO 2.3.4.11).
        Aes::new(key)?.cbc_decrypt(&fit(self.pw.salt.clone(), self.pw.block), &mut out)?;
        Ok(out)
    }

    /// Verify `password` and give back the package's secret key.
    fn secret_key(&self, password: &str) -> Result<Vec<u8>, Error> {
        let spun = self.spun(password);
        let input = self.decrypt_with(
            &self.derive(&spun, &BLK_VERIFIER_INPUT),
            &self.verifier_input,
        )?;
        let value = self.decrypt_with(
            &self.derive(&spun, &BLK_VERIFIER_VALUE),
            &self.verifier_value,
        )?;
        let salt_len = self.pw.salt.len();
        let want = self.pw.hash.digest(&[&input[..salt_len.min(input.len())]]);
        if value.len() < want.len() || value[..want.len()] != want[..] {
            return Err(Error::WrongPassword);
        }
        let key = self.decrypt_with(&self.derive(&spun, &BLK_KEY), &self.key_value)?;
        if key.len() < self.key_data.key_bytes {
            return Err(damaged("encrypted key size"));
        }
        Ok(key[..self.key_data.key_bytes].to_vec())
    }

    fn check_integrity(&self, key: &[u8], package: &[u8]) -> Result<(), Error> {
        let (Some(ek), Some(ev)) = (&self.hmac_key, &self.hmac_value) else {
            return Ok(());
        };
        let aes = Aes::new(key)?;
        let n = self.key_data.hash.size();
        let mut hk = ek.clone();
        aes.cbc_decrypt(&self.key_data.iv(&BLK_HMAC_KEY), &mut hk)?;
        let mut hv = ev.clone();
        aes.cbc_decrypt(&self.key_data.iv(&BLK_HMAC_VALUE), &mut hv)?;
        if hk.len() < n || hv.len() < n {
            return Err(damaged("data integrity sizes"));
        }
        let got = self.key_data.hash.hmac(&hk[..n], package);
        if got != hv[..n] {
            return Err(damaged("the data integrity check failed"));
        }
        Ok(())
    }

    fn decrypt_package(&self, key: &[u8], package: &[u8]) -> Result<Vec<u8>, Error> {
        let size = package_size(package)?;
        let aes = Aes::new(key)?;
        let data = &package[8..];
        let mut out = Vec::with_capacity(data.len());
        for (i, seg) in data.chunks(SEGMENT).enumerate() {
            let mut seg = seg.to_vec();
            // A short last segment (some writers don't pad it).
            let whole = seg.len() / 16 * 16;
            seg.truncate(whole);
            let iv = self.key_data.iv(&(i as u32).to_le_bytes());
            aes.cbc_decrypt(&iv, &mut seg)?;
            out.extend_from_slice(&seg);
            if out.len() >= size {
                break;
            }
        }
        if out.len() < size {
            return Err(damaged("EncryptedPackage shorter than its size"));
        }
        out.truncate(size);
        Ok(out)
    }
}

/// H(salt + password), then H(iterator + H) `spin` times.
fn spin_hash(hash: Hash, salt: &[u8], password: &str, spin: u32) -> Vec<u8> {
    let mut h = hash.digest(&[salt, &prim::utf16le(password)]);
    for i in 0..spin {
        h = hash.digest(&[&i.to_le_bytes(), &h]);
    }
    h
}

// ── Standard ─────────────────────────────────────────────────────────────

struct Standard {
    key_bits: usize,
    salt: Vec<u8>,
    verifier: Vec<u8>,
    verifier_hash: Vec<u8>,
}

impl Standard {
    fn parse(b: &[u8]) -> Result<Standard, Error> {
        let u32_at = |o: usize| -> Result<u32, Error> {
            b.get(o..o + 4)
                .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
                .ok_or_else(|| damaged("EncryptionInfo too short"))
        };
        let header_size = u32_at(0)? as usize;
        let h = 4;
        let alg = u32_at(h + 8)?;
        let alg_hash = u32_at(h + 12)?;
        let key_bits = u32_at(h + 16)? as usize;
        if !matches!(alg, 0x660E..=0x6610) {
            return Err(Error::Unsupported(format!("algorithm {alg:#x}")));
        }
        if alg_hash != 0x8004 && alg_hash != 0 {
            return Err(Error::Unsupported(format!("hash {alg_hash:#x}")));
        }
        if !matches!(key_bits, 128 | 192 | 256) {
            return Err(Error::Unsupported(format!("{key_bits}-bit AES")));
        }
        let v = h
            .checked_add(header_size)
            .ok_or_else(|| damaged("header size"))?;
        let salt_size = u32_at(v)? as usize;
        if salt_size != 16 {
            return Err(damaged("salt size"));
        }
        let take = |o: usize, n: usize| -> Result<Vec<u8>, Error> {
            b.get(o..o + n)
                .map(<[u8]>::to_vec)
                .ok_or_else(|| damaged("EncryptionVerifier too short"))
        };
        let salt = take(v + 4, 16)?;
        let verifier = take(v + 20, 16)?;
        // verifierHashSize (20) then the hash, AES-padded to 32 bytes.
        let verifier_hash = take(v + 40, 32)?;
        Ok(Standard {
            key_bits,
            salt,
            verifier,
            verifier_hash,
        })
    }

    /// The key of `password` (MS-OFFCRYPTO 2.3.4.7), checked against the
    /// verifier.
    fn key(&self, password: &str) -> Result<Vec<u8>, Error> {
        let h = spin_hash(Hash::Sha1, &self.salt, password, 50_000);
        let mut hfinal_in = h.clone();
        hfinal_in.extend(0u32.to_le_bytes());
        let hfinal = prim::sha1(&hfinal_in);
        let derive = |pad: u8| {
            let mut buf = [pad; 64];
            for (b, x) in buf.iter_mut().zip(&hfinal) {
                *b ^= x;
            }
            prim::sha1(&buf)
        };
        let mut x = derive(0x36);
        x.extend(derive(0x5c));
        let key = x[..self.key_bits / 8].to_vec();
        let aes = Aes::new(&key)?;
        let mut verifier = self.verifier.clone();
        aes.ecb_decrypt(&mut verifier)?;
        let mut hash = self.verifier_hash.clone();
        aes.ecb_decrypt(&mut hash)?;
        if prim::sha1(&verifier) != hash[..20] {
            return Err(Error::WrongPassword);
        }
        Ok(key)
    }

    fn decrypt_package(&self, key: &[u8], package: &[u8]) -> Result<Vec<u8>, Error> {
        let size = package_size(package)?;
        let mut data = package[8..].to_vec();
        data.truncate(data.len() / 16 * 16);
        Aes::new(key)?.ecb_decrypt(&mut data)?;
        if data.len() < size {
            return Err(damaged("EncryptedPackage shorter than its size"));
        }
        data.truncate(size);
        Ok(data)
    }
}

// ── writing ──────────────────────────────────────────────────────────────

/// A length-prefixed UTF-16LE string padded to 4 bytes (UNICODE-LP-P4).
fn lp_p4(s: &str) -> Vec<u8> {
    let u = prim::utf16le(s);
    let mut v = (u.len() as u32).to_le_bytes().to_vec();
    v.extend(&u);
    while !v.len().is_multiple_of(4) {
        v.push(0);
    }
    v
}

/// The `\x06DataSpaces` streams Office expects beside the encrypted package
/// ([MS-OFFCRYPTO] 2.1, 2.2).
fn data_spaces() -> Vec<(&'static str, Vec<u8>)> {
    let ver = |v: &mut Vec<u8>| {
        // Reader, updater and writer versions: 1.0 each.
        for _ in 0..3 {
            v.extend(1u16.to_le_bytes());
            v.extend(0u16.to_le_bytes());
        }
    };
    let mut version = lp_p4("Microsoft.Container.DataSpaces");
    ver(&mut version);

    let mut entry = 1u32.to_le_bytes().to_vec(); // one reference component
    entry.extend(0u32.to_le_bytes()); // a stream
    entry.extend(lp_p4("EncryptedPackage"));
    entry.extend(lp_p4("StrongEncryptionDataSpace"));
    let mut map = 8u32.to_le_bytes().to_vec();
    map.extend(1u32.to_le_bytes());
    map.extend((entry.len() as u32 + 4).to_le_bytes());
    map.extend(entry);

    let mut space = 8u32.to_le_bytes().to_vec();
    space.extend(1u32.to_le_bytes());
    space.extend(lp_p4("StrongEncryptionTransform"));

    let id = lp_p4("{FF9A3F03-56EF-4613-BDD5-5A41C1D07246}");
    let mut primary = ((8 + id.len()) as u32).to_le_bytes().to_vec();
    primary.extend(1u32.to_le_bytes()); // transform type
    primary.extend(id);
    primary.extend(lp_p4("Microsoft.Container.EncryptionTransform"));
    ver(&mut primary);
    primary.extend(0u32.to_le_bytes()); // EncryptionName (empty)
    primary.extend(0u32.to_le_bytes()); // EncryptionBlockSize
    primary.extend(0u32.to_le_bytes()); // CipherMode
    primary.extend(4u32.to_le_bytes()); // Reserved

    vec![
        ("/\u{6}DataSpaces/Version", version),
        ("/\u{6}DataSpaces/DataSpaceMap", map),
        (
            "/\u{6}DataSpaces/DataSpaceInfo/StrongEncryptionDataSpace",
            space,
        ),
        (
            "/\u{6}DataSpaces/TransformInfo/StrongEncryptionTransform/\u{6}Primary",
            primary,
        ),
    ]
}

fn pad16(mut v: Vec<u8>) -> Vec<u8> {
    while !v.len().is_multiple_of(16) {
        v.push(0);
    }
    v
}

pub fn encrypt(zip: &[u8], password: &str, spin: u32) -> Result<Vec<u8>, Error> {
    if !zip.starts_with(b"PK") {
        return Err(damaged("not a zip package"));
    }
    let hash = Hash::Sha512;
    let key_salt = prim::random(16);
    let pw_salt = prim::random(16);
    let secret = prim::random(32);
    let key_data = Params {
        salt: key_salt.clone(),
        hash,
        block: 16,
        key_bytes: 32,
    };

    // The package, in 4096-byte segments.
    let aes = Aes::new(&secret)?;
    let mut package = (zip.len() as u64).to_le_bytes().to_vec();
    for (i, seg) in zip.chunks(SEGMENT).enumerate() {
        let mut seg = pad16(seg.to_vec());
        aes.cbc_encrypt(&key_data.iv(&(i as u32).to_le_bytes()), &mut seg);
        package.extend(seg);
    }

    // Data integrity: an HMAC of the encrypted package.
    let hmac_key = prim::random(hash.size());
    let hmac_value = hash.hmac(&hmac_key, &package);
    let mut enc_hmac_key = hmac_key;
    aes.cbc_encrypt(&key_data.iv(&BLK_HMAC_KEY), &mut enc_hmac_key);
    let mut enc_hmac_value = hmac_value;
    aes.cbc_encrypt(&key_data.iv(&BLK_HMAC_VALUE), &mut enc_hmac_value);

    // The password key encryptor.
    let spun = spin_hash(hash, &pw_salt, password, spin);
    let derive = |blk: &[u8]| fit(hash.digest(&[&spun, blk]), 32);
    let pw_enc = |blk: &[u8], data: Vec<u8>| -> Result<Vec<u8>, Error> {
        let mut d = pad16(data);
        Aes::new(&derive(blk))?.cbc_encrypt(&pw_salt, &mut d);
        Ok(d)
    };
    let verifier = prim::random(16);
    let enc_input = pw_enc(&BLK_VERIFIER_INPUT, verifier.clone())?;
    let enc_value = pw_enc(&BLK_VERIFIER_VALUE, hash.digest(&[&verifier]))?;
    let enc_key = pw_enc(&BLK_KEY, secret)?;

    let b = prim::b64_encode;
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n\
<encryption xmlns=\"http://schemas.microsoft.com/office/2006/encryption\" \
xmlns:p=\"http://schemas.microsoft.com/office/2006/keyEncryptor/password\" \
xmlns:c=\"http://schemas.microsoft.com/office/2006/keyEncryptor/certificate\">\
<keyData saltSize=\"16\" blockSize=\"16\" keyBits=\"256\" hashSize=\"64\" \
cipherAlgorithm=\"AES\" cipherChaining=\"ChainingModeCBC\" hashAlgorithm=\"SHA512\" \
saltValue=\"{}\"/>\
<dataIntegrity encryptedHmacKey=\"{}\" encryptedHmacValue=\"{}\"/>\
<keyEncryptors><keyEncryptor uri=\"http://schemas.microsoft.com/office/2006/keyEncryptor/password\">\
<p:encryptedKey spinCount=\"{spin}\" saltSize=\"16\" blockSize=\"16\" keyBits=\"256\" \
hashSize=\"64\" cipherAlgorithm=\"AES\" cipherChaining=\"ChainingModeCBC\" \
hashAlgorithm=\"SHA512\" saltValue=\"{}\" encryptedVerifierHashInput=\"{}\" \
encryptedVerifierHashValue=\"{}\" encryptedKeyValue=\"{}\"/>\
</keyEncryptor></keyEncryptors></encryption>",
        b(&key_salt),
        b(&enc_hmac_key),
        b(&enc_hmac_value),
        b(&pw_salt),
        b(&enc_input),
        b(&enc_value),
        b(&enc_key),
    );
    let mut info = vec![4, 0, 4, 0, 0x40, 0, 0, 0];
    info.extend(xml.as_bytes());

    let io = |e: std::io::Error| damaged(format!("writing the compound file: {e}"));
    // Version 3 (512-byte sectors), as Office writes it: LibreOffice can't open
    // encrypted packages in version 4 compound files.
    let mut c = cfb::CompoundFile::create_with_version(cfb::Version::V3, Cursor::new(Vec::new()))
        .map_err(io)?;
    for storage in [
        "/\u{6}DataSpaces",
        "/\u{6}DataSpaces/DataSpaceInfo",
        "/\u{6}DataSpaces/TransformInfo",
        "/\u{6}DataSpaces/TransformInfo/StrongEncryptionTransform",
    ] {
        c.create_storage(storage).map_err(io)?;
    }
    for (name, data) in data_spaces() {
        c.create_stream(name)
            .map_err(io)?
            .write_all(&data)
            .map_err(io)?;
    }
    c.create_stream("/EncryptionInfo")
        .map_err(io)?
        .write_all(&info)
        .map_err(io)?;
    c.create_stream("/EncryptedPackage")
        .map_err(io)?
        .write_all(&package)
        .map_err(io)?;
    c.flush().map_err(io)?;
    Ok(c.into_inner().into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_zip() -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        w.start_file(
            "[Content_Types].xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        w.write_all(&vec![b'x'; 9000]).unwrap();
        w.finish().unwrap().into_inner()
    }

    #[std::prelude::v1::test]
    fn agile_round_trip() {
        let zip = tiny_zip();
        // A low spin count keeps the test fast; the format is the same.
        let enc = encrypt(&zip, "kantor", 1000).unwrap();
        assert_eq!(crate::protection(&enc), Some(crate::Protection::Ooxml));
        // A version 3 compound file, as Office writes (LibreOffice needs it).
        assert_eq!(&enc[0x1A..0x1C], &[3, 0]);
        assert_eq!(decrypt(&enc, "kantor").unwrap(), zip);
        assert_eq!(decrypt(&enc, "Kantor"), Err(Error::WrongPassword));
        assert_eq!(check(&enc, "kantor"), Ok(()));
        // Every truncation fails cleanly.
        for cut in (0..enc.len()).step_by(97) {
            let _ = decrypt(&enc[..cut], "kantor");
            let _ = crate::protection(&enc[..cut]);
        }
    }

    #[std::prelude::v1::test]
    fn data_spaces_match_the_spec() {
        let ds = data_spaces();
        // Sizes of Office's own streams.
        assert_eq!(ds[0].1.len(), 76);
        assert_eq!(ds[1].1.len(), 112);
        assert_eq!(&ds[1].1[8..12], &104u32.to_le_bytes());
        assert_eq!(ds[2].1.len(), 64);
        assert_eq!(&ds[3].1[..4], &88u32.to_le_bytes());
        assert_eq!(ds[3].1.len(), 200);
    }

    #[std::prelude::v1::test]
    fn rc4_and_extensible_are_unsupported() {
        let mut info = vec![4, 0, 2, 0, 0x04, 0, 0, 0];
        info.extend([0u8; 64]);
        assert!(matches!(parse_info(&info), Err(Error::Unsupported(_))));
        let info = [3u8, 0, 3, 0, 0, 0, 0, 0];
        assert!(matches!(parse_info(&info), Err(Error::Unsupported(_))));
        assert!(matches!(parse_info(&[1, 2]), Err(Error::Damaged(_))));
    }
}
