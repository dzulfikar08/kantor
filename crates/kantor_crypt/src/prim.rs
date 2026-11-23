//! Primitives: hashes, AES and Blowfish block modes, key derivation.
//! Block modes are written out here (they are a few lines each) so the
//! crate only needs the block ciphers.

use aes::cipher::{BlockCipherDecrypt, BlockCipherEncrypt, KeyInit};
use base64::Engine as _;
use sha1::Digest as _;

use crate::{Error, damaged};

/// A hash algorithm by its MS-OFFCRYPTO / XML-DSig name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hash {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl Hash {
    pub fn from_name(name: &str) -> Option<Hash> {
        Some(match name.to_ascii_uppercase().replace('-', "").as_str() {
            "SHA1" => Hash::Sha1,
            "SHA256" => Hash::Sha256,
            "SHA384" => Hash::Sha384,
            "SHA512" => Hash::Sha512,
            _ => return None,
        })
    }

    pub fn len(self) -> usize {
        match self {
            Hash::Sha1 => 20,
            Hash::Sha256 => 32,
            Hash::Sha384 => 48,
            Hash::Sha512 => 64,
        }
    }

    /// The hash of the parts, one after the other.
    pub fn digest(self, parts: &[&[u8]]) -> Vec<u8> {
        fn run<D: sha1::Digest>(parts: &[&[u8]]) -> Vec<u8> {
            let mut d = D::new();
            for p in parts {
                d.update(p);
            }
            d.finalize().to_vec()
        }
        match self {
            Hash::Sha1 => run::<sha1::Sha1>(parts),
            Hash::Sha256 => run::<sha2::Sha256>(parts),
            Hash::Sha384 => run::<sha2::Sha384>(parts),
            Hash::Sha512 => run::<sha2::Sha512>(parts),
        }
    }

    pub fn hmac(self, key: &[u8], data: &[u8]) -> Vec<u8> {
        use hmac::{Hmac, Mac};
        fn run<D>(key: &[u8], data: &[u8]) -> Vec<u8>
        where
            D: hmac::EagerHash,
            Hmac<D>: hmac::KeyInit + Mac,
        {
            let mut m = <Hmac<D> as hmac::KeyInit>::new_from_slice(key)
                .expect("HMAC takes keys of any length");
            m.update(data);
            m.finalize().into_bytes().to_vec()
        }
        match self {
            Hash::Sha1 => run::<sha1::Sha1>(key, data),
            Hash::Sha256 => run::<sha2::Sha256>(key, data),
            Hash::Sha384 => run::<sha2::Sha384>(key, data),
            Hash::Sha512 => run::<sha2::Sha512>(key, data),
        }
    }
}

pub fn sha1(data: &[u8]) -> Vec<u8> {
    sha1::Sha1::digest(data).to_vec()
}

/// AES with a 128, 192 or 256-bit key.
pub enum Aes {
    A128(aes::Aes128),
    A192(aes::Aes192),
    A256(aes::Aes256),
}

impl Aes {
    pub fn new(key: &[u8]) -> Result<Aes, Error> {
        let bad = |_| damaged("AES key size");
        Ok(match key.len() {
            16 => Aes::A128(aes::Aes128::new_from_slice(key).map_err(bad)?),
            24 => Aes::A192(aes::Aes192::new_from_slice(key).map_err(bad)?),
            32 => Aes::A256(aes::Aes256::new_from_slice(key).map_err(bad)?),
            n => return Err(Error::Unsupported(format!("{}-bit AES keys", n * 8))),
        })
    }

    fn enc(&self, b: &mut [u8]) {
        let mut blk = aes::Block::try_from(&b[..]).expect("16-byte block");
        match self {
            Aes::A128(c) => c.encrypt_block(&mut blk),
            Aes::A192(c) => c.encrypt_block(&mut blk),
            Aes::A256(c) => c.encrypt_block(&mut blk),
        }
        b.copy_from_slice(&blk);
    }

    fn dec(&self, b: &mut [u8]) {
        let mut blk = aes::Block::try_from(&b[..]).expect("16-byte block");
        match self {
            Aes::A128(c) => c.decrypt_block(&mut blk),
            Aes::A192(c) => c.decrypt_block(&mut blk),
            Aes::A256(c) => c.decrypt_block(&mut blk),
        }
        b.copy_from_slice(&blk);
    }

    /// CBC decryption in place (`data` a whole number of blocks).
    pub fn cbc_decrypt(&self, iv: &[u8], data: &mut [u8]) -> Result<(), Error> {
        if iv.len() != 16 || data.len() % 16 != 0 {
            return Err(damaged("AES-CBC block size"));
        }
        let mut prev = [0u8; 16];
        prev.copy_from_slice(iv);
        for chunk in data.chunks_exact_mut(16) {
            let mut c = [0u8; 16];
            c.copy_from_slice(chunk);
            self.dec(chunk);
            for (x, p) in chunk.iter_mut().zip(prev) {
                *x ^= p;
            }
            prev = c;
        }
        Ok(())
    }

    /// CBC encryption in place (`data` a whole number of blocks).
    pub fn cbc_encrypt(&self, iv: &[u8], data: &mut [u8]) {
        debug_assert!(iv.len() == 16 && data.len() % 16 == 0);
        let mut prev = [0u8; 16];
        prev.copy_from_slice(&iv[..16]);
        for chunk in data.chunks_exact_mut(16) {
            for (x, p) in chunk.iter_mut().zip(prev) {
                *x ^= p;
            }
            self.enc(chunk);
            prev.copy_from_slice(chunk);
        }
    }

    /// ECB decryption in place (`data` a whole number of blocks).
    pub fn ecb_decrypt(&self, data: &mut [u8]) -> Result<(), Error> {
        if data.len() % 16 != 0 {
            return Err(damaged("AES-ECB block size"));
        }
        for chunk in data.chunks_exact_mut(16) {
            self.dec(chunk);
        }
        Ok(())
    }
}

/// Blowfish in OpenOffice.org's "CFB" stream mode: 64-bit cipher feedback,
/// one byte at a time (rtl_Cipher_ModeStream).
pub fn blowfish_cfb_decrypt(key: &[u8], iv: &[u8], data: &mut [u8]) -> Result<(), Error> {
    let bf: blowfish::Blowfish =
        blowfish::Blowfish::new_from_slice(key).map_err(|_| damaged("Blowfish key size"))?;
    if iv.len() != 8 {
        return Err(damaged("Blowfish IV size"));
    }
    let mut reg = [0u8; 8];
    reg.copy_from_slice(iv);
    let mut k = 0;
    for byte in data.iter_mut() {
        if k == 0 {
            let mut blk = blowfish::cipher::Block::<blowfish::Blowfish>::try_from(&reg[..])
                .expect("8-byte block");
            bf.encrypt_block(&mut blk);
            reg.copy_from_slice(&blk);
        }
        let c = *byte;
        *byte ^= reg[k];
        reg[k] = c;
        k = (k + 1) % 8;
    }
    Ok(())
}

/// AES-GCM decryption (`data` = ciphertext followed by the 16-byte tag).
/// A wrong key fails the tag: [`Error::WrongPassword`].
pub fn aes_gcm_decrypt(key: &[u8], iv: &[u8], data: &[u8]) -> Result<Vec<u8>, Error> {
    use aes_gcm::aead::Aead as _;
    if key.len() != 32 {
        return Err(Error::Unsupported(format!(
            "{}-bit AES-GCM keys",
            key.len() * 8
        )));
    }
    if iv.len() != 12 {
        return Err(Error::Unsupported(format!(
            "{}-byte AES-GCM nonces",
            iv.len()
        )));
    }
    let cipher = aes_gcm::Aes256Gcm::new_from_slice(key).map_err(|_| damaged("AES-GCM key"))?;
    let nonce = aes_gcm::Nonce::<aes_gcm::aes::cipher::consts::U12>::try_from(iv)
        .map_err(|_| damaged("AES-GCM nonce"))?;
    cipher
        .decrypt(&nonce, data)
        .map_err(|_| Error::WrongPassword)
}

pub fn pbkdf2_sha1(password: &[u8], salt: &[u8], rounds: u32, len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password, salt, rounds, &mut out);
    out
}

pub fn argon2id(
    password: &[u8],
    salt: &[u8],
    t_cost: u32,
    m_cost: u32,
    lanes: u32,
    len: usize,
) -> Result<Vec<u8>, Error> {
    // Bounds that keep a hostile file from asking for gigabytes or hours.
    if m_cost > 4 * 1024 * 1024 || t_cost > 64 || lanes == 0 || lanes > 64 {
        return Err(Error::Unsupported("Argon2 parameters too large".into()));
    }
    let params = argon2::Params::new(m_cost, t_cost, lanes, Some(len))
        .map_err(|e| damaged(format!("Argon2 parameters: {e}")))?;
    let a = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = vec![0u8; len];
    a.hash_password_into(password, salt, &mut out)
        .map_err(|e| damaged(format!("Argon2: {e}")))?;
    Ok(out)
}

pub fn b64(s: &str) -> Result<Vec<u8>, Error> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|_| damaged("base64 value"))
}

pub fn b64_encode(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

pub fn random(len: usize) -> Vec<u8> {
    use ring::rand::SecureRandom as _;
    let mut out = vec![0u8; len];
    ring::rand::SystemRandom::new()
        .fill(&mut out)
        .expect("the system's random number generator works");
    out
}

/// The password as UTF-16LE (MS-OFFCRYPTO).
pub fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn hashes_match_known_values() {
        assert_eq!(Hash::Sha1.digest(&[b"a", b"bc"]), sha1(b"abc"),);
        assert_eq!(
            b64_encode(&Hash::Sha256.digest(&[b"abc"])),
            "ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0="
        );
    }

    #[std::prelude::v1::test]
    fn aes_cbc_round_trips() {
        let key = [7u8; 32];
        let iv = [9u8; 16];
        let aes = Aes::new(&key).unwrap();
        let mut data = *b"0123456789abcdef0123456789abcdef";
        aes.cbc_encrypt(&iv, &mut data);
        assert_ne!(&data, b"0123456789abcdef0123456789abcdef");
        aes.cbc_decrypt(&iv, &mut data).unwrap();
        assert_eq!(&data, b"0123456789abcdef0123456789abcdef");
        assert!(aes.cbc_decrypt(&iv, &mut [0u8; 15]).is_err());
        assert!(Aes::new(&[0u8; 7]).is_err());
    }

    #[std::prelude::v1::test]
    fn pbkdf2_matches_rfc6070() {
        // RFC 6070, test vector 2.
        assert_eq!(
            pbkdf2_sha1(b"password", b"salt", 2, 20),
            [
                0xea, 0x6c, 0x01, 0x4d, 0xc7, 0x2d, 0x6f, 0x8c, 0xcd, 0x1e, 0xd9, 0x2a, 0xce, 0x1d,
                0x41, 0xf0, 0xd8, 0xde, 0x89, 0x57
            ]
        );
    }

    #[std::prelude::v1::test]
    fn base64_reads_with_line_breaks() {
        assert_eq!(b64("YW\nJj").unwrap(), b"abc");
        assert!(b64("!!").is_err());
    }
}
