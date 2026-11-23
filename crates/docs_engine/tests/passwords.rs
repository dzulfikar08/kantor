//! Password-protected documents (tests/corpus/encrypted, password
//! `kantor`): opening asks for the password, a wrong one is told apart, and
//! saves stay encrypted with it.

use std::path::PathBuf;

use docs_engine::Document;

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/encrypted")
        .join(name)
}

fn crypt_error(e: &anyhow::Error) -> Option<&kantor_crypt::Error> {
    e.downcast_ref::<kantor_crypt::Error>()
}

fn has_text(doc: &Document, text: &str) -> bool {
    doc.paragraphs().iter().any(|p| p.text.contains(text))
}

#[test]
fn encrypted_docx_opens_with_its_password() {
    for name in ["agile.docx", "standard.docx", "lo.docx"] {
        let path = corpus(name);
        let e = Document::open(&path).map(|_| ()).unwrap_err();
        assert_eq!(
            crypt_error(&e),
            Some(&kantor_crypt::Error::PasswordNeeded),
            "{name}: {e:#}"
        );
        let e = Document::open_with_password(&path, Some("salah"))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            crypt_error(&e),
            Some(&kantor_crypt::Error::WrongPassword),
            "{name}: {e:#}"
        );

        let doc = Document::open_with_password(&path, Some("kantor"))
            .unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert!(has_text(&doc, "Rahasia Kantor"), "{name}");
        assert_eq!(doc.password(), Some("kantor"));
        // A .docx keeps its file: Save writes it (encrypted).
        assert_eq!(doc.path(), Some(path.as_path()), "{name}");
        assert!(!doc.is_imported());
        assert!(!docs_engine::is_import_path(&path), "{name}");
    }
}

#[test]
fn saves_stay_encrypted() {
    let dir = tempfile::tempdir().unwrap();
    let mut doc = Document::open_with_password(&corpus("agile.docx"), Some("kantor")).unwrap();
    let out = dir.path().join("copy.docx");
    doc.save_as(&out).unwrap();
    assert_eq!(
        kantor_crypt::file_protection(&out),
        Some(kantor_crypt::Protection::Ooxml)
    );
    let again = Document::open_with_password(&out, Some("kantor")).unwrap();
    assert!(has_text(&again, "Rahasia Kantor"));
    // Save over it (the second save reads the package back from memory).
    let mut again = again;
    again.save().unwrap();
    assert!(Document::open_with_password(&out, Some("kantor")).is_ok());

    // Without the password the copy is a plain .docx.
    doc.set_password(None);
    let plain = dir.path().join("plain.docx");
    doc.save_as(&plain).unwrap();
    assert_eq!(kantor_crypt::file_protection(&plain), None);
    assert!(has_text(&Document::open(&plain).unwrap(), "Rahasia Kantor"));
}

#[test]
fn encrypted_odt_imports_and_keeps_the_password() {
    for name in ["aead.odt", "cbc.odt", "blowfish.odt"] {
        let path = corpus(name);
        let e = Document::open(&path).map(|_| ()).unwrap_err();
        assert_eq!(
            crypt_error(&e),
            Some(&kantor_crypt::Error::PasswordNeeded),
            "{name}: {e:#}"
        );
        let doc = Document::open_with_password(&path, Some("kantor"))
            .unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert!(doc.is_imported(), "{name}");
        assert!(has_text(&doc, "Rahasia Kantor"), "{name}");
        assert_eq!(doc.password(), Some("kantor"));
        assert!(docs_engine::is_import_path(&path), "{name}");
    }
    let dir = tempfile::tempdir().unwrap();
    let mut doc = Document::open_with_password(&corpus("aead.odt"), Some("kantor")).unwrap();
    let out = dir.path().join("from-odt.docx");
    doc.save_as(&out).unwrap();
    assert_eq!(
        kantor_crypt::file_protection(&out),
        Some(kantor_crypt::Protection::Ooxml)
    );
}
