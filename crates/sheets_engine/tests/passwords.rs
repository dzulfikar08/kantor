//! Password-protected workbooks (tests/corpus/encrypted, password
//! `kantor`): opening asks for the password, a wrong one is told apart, and
//! `.xlsx` saves stay encrypted with it.

use std::path::PathBuf;

use sheets_engine::Workbook;

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/encrypted")
        .join(name)
}

fn crypt_error(e: &anyhow::Error) -> Option<&kantor_crypt::Error> {
    e.downcast_ref::<kantor_crypt::Error>()
}

#[test]
fn encrypted_xlsx_opens_with_its_password() {
    for name in ["agile.xlsx", "standard.xlsx", "lo.xlsx"] {
        let path = corpus(name);
        let e = Workbook::open(&path).map(|_| ()).unwrap_err();
        assert_eq!(
            crypt_error(&e),
            Some(&kantor_crypt::Error::PasswordNeeded),
            "{name}: {e:#}"
        );
        let e = Workbook::open_with_password(&path, Some("salah"))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            crypt_error(&e),
            Some(&kantor_crypt::Error::WrongPassword),
            "{name}: {e:#}"
        );
        let book = Workbook::open_with_password(&path, Some("kantor"))
            .unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert_eq!(book.input(0, 0, 0), "Rahasia Kantor", "{name}");
        assert_eq!(book.password(), Some("kantor"));
        assert_eq!(book.path(), Some(path.as_path()), "{name}");
    }
}

#[test]
fn saves_stay_encrypted() {
    let dir = tempfile::tempdir().unwrap();
    let mut book = Workbook::open_with_password(&corpus("agile.xlsx"), Some("kantor")).unwrap();
    book.set_input(0, 2, 0, "baru").unwrap();
    let out = dir.path().join("copy.xlsx");
    book.save_as(&out).unwrap();
    assert_eq!(
        kantor_crypt::file_protection(&out),
        Some(kantor_crypt::Protection::Ooxml)
    );
    // The next save carries parts over from memory, not the encrypted file.
    book.set_input(0, 3, 0, "lagi").unwrap();
    book.save().unwrap();
    let again = Workbook::open_with_password(&out, Some("kantor")).unwrap();
    assert_eq!(again.input(0, 0, 0), "Rahasia Kantor");
    assert_eq!(again.input(0, 3, 0), "lagi");

    // CSV output is never encrypted.
    let csv = dir.path().join("data.csv");
    book.save_as(&csv).unwrap();
    assert!(
        std::fs::read_to_string(&csv)
            .unwrap()
            .contains("Rahasia Kantor")
    );

    let mut plain = Workbook::open_with_password(&out, Some("kantor")).unwrap();
    plain.set_password(None);
    let unprotected = dir.path().join("plain.xlsx");
    plain.save_as(&unprotected).unwrap();
    assert_eq!(kantor_crypt::file_protection(&unprotected), None);
    assert_eq!(
        Workbook::open(&unprotected).unwrap().input(0, 0, 0),
        "Rahasia Kantor"
    );
}

#[test]
fn encrypted_ods_imports_and_keeps_the_password() {
    for name in ["aead.ods", "cbc.ods"] {
        let path = corpus(name);
        let e = Workbook::open(&path).map(|_| ()).unwrap_err();
        assert_eq!(
            crypt_error(&e),
            Some(&kantor_crypt::Error::PasswordNeeded),
            "{name}: {e:#}"
        );
        let book = Workbook::open_with_password(&path, Some("kantor"))
            .unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert_eq!(book.input(0, 0, 0), "Rahasia Kantor", "{name}");
        assert_eq!(book.password(), Some("kantor"));
        assert!(book.path().is_none(), "an import saves with Save As");
        assert_eq!(book.source_path(), Some(path.as_path()), "{name}");
    }
}
