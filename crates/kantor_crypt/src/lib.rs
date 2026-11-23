//! Password-protected Office files.
//!
//! * **OOXML** (`.docx`, `.xlsx`, `.pptx` and their macro/template forms)
//!   encrypted as [MS-OFFCRYPTO] describes: a compound file holding an
//!   `EncryptionInfo` and an `EncryptedPackage` stream. *Agile* encryption
//!   (Office 2010 and later: AES with SHA-1/256/384/512, CBC) and *Standard*
//!   encryption (Office 2007: AES-128/192/256 ECB, SHA-1) are read; files
//!   are written with Agile encryption the way Office 2013 and later write
//!   them (AES-256, SHA-512, 100,000 spins, data integrity HMAC). RC4
//!   CryptoAPI and Extensible encryption are [`Error::Unsupported`].
//! * **OpenDocument** (`.odt`, `.ods`, `.odp`, …) encrypted per entry
//!   (Blowfish CFB with a SHA-1 start key, as OpenOffice.org wrote it; AES-256
//!   CBC with a SHA-256 start key, as LibreOffice 3.4 to 24.2 write it) or as
//!   one `encrypted-package` (ODF 1.4: AES-256-GCM with Argon2id, LibreOffice
//!   24.8 and later). Kantor never writes OpenDocument, so only reading.
//!
//! [`decrypt`] gives back the plain package: a normal `.docx`/`.xlsx`/`.pptx`
//! zip, or an ODF zip with every entry decrypted and no encryption data left
//! in its manifest. Malformed files give [`Error::Damaged`], never a panic.
//!
//! [MS-OFFCRYPTO]: https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-offcrypto/

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

mod cfbscan;
mod odf;
mod ooxml;
mod prim;

/// How a file is protected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protection {
    /// An encrypted OOXML package (MS-OFFCRYPTO compound file).
    Ooxml,
    /// An OpenDocument package with encrypted entries.
    Odf,
}

/// Why a protected file can't be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The file is protected and no password was given.
    PasswordNeeded,
    /// The password doesn't open the file.
    WrongPassword,
    /// Encrypted in a way Kantor can't read (what, in a few words).
    Unsupported(String),
    /// The file is damaged or isn't what it claims (what, in a few words).
    Damaged(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::PasswordNeeded => f.write_str("the file is password-protected"),
            Error::WrongPassword => f.write_str("the password is incorrect"),
            Error::Unsupported(what) => write!(f, "encrypted in a way Kantor can't read ({what})"),
            Error::Damaged(what) => write!(f, "the encrypted file is damaged ({what})"),
        }
    }
}

impl std::error::Error for Error {}

pub(crate) fn damaged(what: impl Into<String>) -> Error {
    Error::Damaged(what.into())
}

/// The password Excel uses for files that are encrypted without one being
/// asked for ("VelvetSweatshop"); [`unlock`] tries it before asking.
pub const DEFAULT_EXCEL_PASSWORD: &str = "VelvetSweatshop";

/// Whether `bytes` are a protected Office file (looks at the container only;
/// no key derivation).
pub fn protection(bytes: &[u8]) -> Option<Protection> {
    sniff(&mut std::io::Cursor::new(bytes))
}

/// [`protection`] of the file at `path`. Reads only what it needs: the
/// compound file's header and directory sectors, or a zip's central
/// directory and `META-INF/manifest.xml`; never the whole file. `None` for
/// plain files and on read errors.
pub fn file_protection(path: &Path) -> Option<Protection> {
    let mut f = std::fs::File::open(path).ok()?;
    sniff(&mut f)
}

fn sniff<R: Read + Seek>(r: &mut R) -> Option<Protection> {
    let mut head = [0u8; 8];
    r.seek(SeekFrom::Start(0)).ok()?;
    r.read_exact(&mut head).ok()?;
    if head == cfbscan::MAGIC {
        return cfbscan::has_encrypted_package(r).then_some(Protection::Ooxml);
    }
    if head.starts_with(b"PK") {
        r.seek(SeekFrom::Start(0)).ok()?;
        return odf::sniff(r).then_some(Protection::Odf);
    }
    None
}

/// The plain package inside a protected file.
pub fn decrypt(bytes: &[u8], password: &str) -> Result<Vec<u8>, Error> {
    match protection(bytes) {
        Some(Protection::Ooxml) => ooxml::decrypt(bytes, password),
        Some(Protection::Odf) => {
            odf::decrypt(bytes, password, false).map(|p| p.unwrap_or_default())
        }
        None => Err(damaged("not an encrypted Office file")),
    }
}

/// Check `password` without decrypting the whole file (key derivation and
/// the password verifier, or one entry's checksum).
pub fn check_password(bytes: &[u8], password: &str) -> Result<(), Error> {
    match protection(bytes) {
        Some(Protection::Ooxml) => ooxml::check(bytes, password),
        Some(Protection::Odf) => odf::decrypt(bytes, password, true).map(|_| ()),
        None => Err(damaged("not an encrypted Office file")),
    }
}

/// Encrypt a plain OOXML package (a `.docx`/`.xlsx`/`.pptx` zip) with Agile
/// encryption (AES-256-CBC, SHA-512, 100,000 spins, with the data integrity
/// HMAC), as Office 2013 and later do.
pub fn encrypt(zip: &[u8], password: &str) -> Result<Vec<u8>, Error> {
    ooxml::encrypt(zip, password, ooxml::SPIN_COUNT)
}

/// What the engines open: `bytes` unchanged when they aren't protected;
/// otherwise the plain package for `password`. With no password, an OOXML
/// file encrypted with Excel's default password opens as it is (as in
/// Excel); any other protected file is [`Error::PasswordNeeded`].
pub fn unlock(bytes: Vec<u8>, password: Option<&str>) -> Result<Unlocked, Error> {
    let Some(kind) = protection(&bytes) else {
        return Ok(Unlocked {
            bytes,
            protected: false,
        });
    };
    let plain = match password {
        Some(pw) => decrypt(&bytes, pw)?,
        None if kind == Protection::Ooxml => match decrypt(&bytes, DEFAULT_EXCEL_PASSWORD) {
            // Saved without asking, so it's saved again without a password.
            Ok(plain) => {
                return Ok(Unlocked {
                    bytes: plain,
                    protected: false,
                });
            }
            Err(Error::WrongPassword) => return Err(Error::PasswordNeeded),
            Err(e) => return Err(e),
        },
        None => return Err(Error::PasswordNeeded),
    };
    Ok(Unlocked {
        bytes: plain,
        protected: true,
    })
}

/// A file's contents after [`unlock`].
#[derive(Debug)]
pub struct Unlocked {
    /// The plain package (or the file as it was).
    pub bytes: Vec<u8>,
    /// Whether a password was needed (so the document should keep it).
    pub protected: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn plain_files_are_not_protected() {
        assert_eq!(protection(b""), None);
        assert_eq!(protection(b"PK\x03\x04garbage"), None);
        assert_eq!(protection(&cfbscan::MAGIC), None);
        let u = unlock(b"hello".to_vec(), None).unwrap();
        assert!(!u.protected);
        assert_eq!(u.bytes, b"hello");
    }

    #[std::prelude::v1::test]
    fn errors_read_well() {
        assert_eq!(
            Error::WrongPassword.to_string(),
            "the password is incorrect"
        );
        assert!(Error::Unsupported("RC4".into()).to_string().contains("RC4"));
    }
}
