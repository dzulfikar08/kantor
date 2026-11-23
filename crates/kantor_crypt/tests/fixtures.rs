//! The password-protected corpus in tests/corpus/encrypted (made by its
//! make_fixtures.py with msoffcrypto-tool and LibreOffice 25.8; password
//! `kantor`): every kind decrypts to its plain package, wrong passwords are
//! told apart, and Kantor's own Agile output reads back.

use std::io::{Cursor, Read as _};
use std::path::PathBuf;

use kantor_crypt::{Error, Protection};

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/encrypted")
        .join(name)
}

fn read(name: &str) -> Vec<u8> {
    std::fs::read(corpus(name)).unwrap()
}

/// An entry of a zip, inflated.
fn entry(zip: &[u8], name: &str) -> String {
    let mut z = zip::ZipArchive::new(Cursor::new(zip)).unwrap();
    let mut s = String::new();
    z.by_name(name)
        .unwrap_or_else(|_| panic!("no {name}"))
        .read_to_string(&mut s)
        .unwrap();
    s
}

const OOXML: [(&str, &str); 8] = [
    ("agile.docx", "word/document.xml"),
    ("agile.xlsx", "xl/worksheets/sheet1.xml"),
    ("agile.pptx", "ppt/slides/slide1.xml"),
    ("standard.docx", "word/document.xml"),
    ("standard.xlsx", "xl/worksheets/sheet1.xml"),
    ("lo.docx", "word/document.xml"),
    ("lo.xlsx", "xl/sharedStrings.xml"),
    ("lo.pptx", "ppt/slides/slide1.xml"),
];

const ODF: [&str; 7] = [
    "aead.odt",
    "aead.ods",
    "aead.odp",
    "cbc.odt",
    "cbc.ods",
    "cbc.odp",
    "blowfish.odt",
];

#[test]
fn ooxml_files_decrypt() {
    for (name, part) in OOXML {
        let bytes = read(name);
        assert_eq!(
            kantor_crypt::protection(&bytes),
            Some(Protection::Ooxml),
            "{name}"
        );
        assert_eq!(
            kantor_crypt::file_protection(&corpus(name)),
            Some(Protection::Ooxml),
            "{name}"
        );
        let plain =
            kantor_crypt::decrypt(&bytes, "kantor").unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(entry(&plain, part).contains("Rahasia Kantor"), "{name}");
        assert_eq!(
            kantor_crypt::check_password(&bytes, "kantor"),
            Ok(()),
            "{name}"
        );
        assert_eq!(
            kantor_crypt::decrypt(&bytes, "kantor2"),
            Err(Error::WrongPassword),
            "{name}"
        );
        assert_eq!(
            kantor_crypt::unlock(bytes, None).unwrap_err(),
            Error::PasswordNeeded,
            "{name}"
        );
    }
}

#[test]
fn odf_files_decrypt() {
    for name in ODF {
        let bytes = read(name);
        assert_eq!(
            kantor_crypt::protection(&bytes),
            Some(Protection::Odf),
            "{name}"
        );
        assert_eq!(
            kantor_crypt::file_protection(&corpus(name)),
            Some(Protection::Odf),
            "{name}"
        );
        let plain =
            kantor_crypt::decrypt(&bytes, "kantor").unwrap_or_else(|e| panic!("{name}: {e}"));
        // A plain ODF package: mimetype first, readable content, a manifest
        // with no encryption data left.
        assert_eq!(&plain[30..38], b"mimetype", "{name}");
        assert!(
            entry(&plain, "content.xml").contains("Rahasia Kantor"),
            "{name}"
        );
        assert!(
            !entry(&plain, "META-INF/manifest.xml").contains("encryption-data"),
            "{name}"
        );
        assert_eq!(kantor_crypt::protection(&plain), None, "{name}");
        assert_eq!(
            kantor_crypt::check_password(&bytes, "kantor"),
            Ok(()),
            "{name}"
        );
        assert_eq!(
            kantor_crypt::check_password(&bytes, "salah"),
            Err(Error::WrongPassword),
            "{name}"
        );
        assert_eq!(
            kantor_crypt::decrypt(&bytes, "salah"),
            Err(Error::WrongPassword),
            "{name}"
        );
    }
}

#[test]
fn kantor_encrypts_what_it_reads() {
    let plain = kantor_crypt::decrypt(&read("agile.docx"), "kantor").unwrap();
    let enc = kantor_crypt::encrypt(&plain, "rahasia ✓").unwrap();
    assert_eq!(kantor_crypt::protection(&enc), Some(Protection::Ooxml));
    assert_eq!(kantor_crypt::decrypt(&enc, "rahasia ✓").unwrap(), plain);
    assert_eq!(
        kantor_crypt::decrypt(&enc, "rahasia"),
        Err(Error::WrongPassword)
    );
    // A fresh salt each time.
    assert_ne!(kantor_crypt::encrypt(&plain, "rahasia ✓").unwrap(), enc);
    if let Ok(dir) = std::env::var("KANTOR_CRYPT_OUT") {
        // CI checks these with msoffcrypto-tool and LibreOffice.
        std::fs::create_dir_all(&dir).unwrap();
        for (name, _) in OOXML.iter().filter(|(n, _)| n.starts_with("agile")) {
            let plain = kantor_crypt::decrypt(&read(name), "kantor").unwrap();
            let out = format!("{dir}/kantor-{name}");
            std::fs::write(&out, kantor_crypt::encrypt(&plain, "kantor").unwrap()).unwrap();
            std::fs::write(format!("{out}.plain"), &plain).unwrap();
        }
    }
}

#[test]
fn damaged_files_fail_cleanly() {
    for name in [
        "agile.xlsx",
        "standard.xlsx",
        "cbc.ods",
        "aead.ods",
        "blowfish.odt",
    ] {
        let bytes = read(name);
        for cut in (0..bytes.len()).step_by(bytes.len() / 23 + 1) {
            let _ = kantor_crypt::protection(&bytes[..cut]);
            let _ = kantor_crypt::decrypt(&bytes[..cut], "kantor");
        }
        // Flipped bytes in the middle: never a panic; where the format
        // authenticates the data (AES-GCM), an error.
        let mut bad = bytes.clone();
        let mid = bad.len() / 2;
        for b in &mut bad[mid..mid + 64] {
            *b ^= 0x5A;
        }
        let res = kantor_crypt::decrypt(&bad, "kantor");
        if name == "aead.ods" {
            assert!(res.is_err(), "{name}");
        }
    }
    assert_eq!(kantor_crypt::protection(b"plain text"), None);
    assert!(matches!(
        kantor_crypt::decrypt(b"plain", "x"),
        Err(Error::Damaged(_))
    ));
}
