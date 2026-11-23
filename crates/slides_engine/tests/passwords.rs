//! Password-protected presentations (tests/corpus/encrypted, password
//! `kantor`): opening asks for the password, a wrong one is told apart, and
//! saves stay encrypted with it.

use std::path::PathBuf;

use slides_engine::Presentation;

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/encrypted")
        .join(name)
}

fn crypt_error(e: &anyhow::Error) -> Option<&kantor_crypt::Error> {
    e.downcast_ref::<kantor_crypt::Error>()
}

fn has_text(p: &Presentation, text: &str) -> bool {
    (0..p.len()).any(|i| {
        p.slide(i).shapes.iter().any(|s| {
            s.text
                .as_ref()
                .is_some_and(|t| t.plain_text().contains(text))
        })
    })
}

#[test]
fn encrypted_pptx_opens_with_its_password() {
    for name in ["agile.pptx", "lo.pptx"] {
        let path = corpus(name);
        let e = Presentation::open(&path).map(|_| ()).unwrap_err();
        assert_eq!(
            crypt_error(&e),
            Some(&kantor_crypt::Error::PasswordNeeded),
            "{name}: {e:#}"
        );
        let e = Presentation::open_with_password(&path, Some("salah"))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            crypt_error(&e),
            Some(&kantor_crypt::Error::WrongPassword),
            "{name}: {e:#}"
        );
        let p = Presentation::open_with_password(&path, Some("kantor"))
            .unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert!(has_text(&p, "Rahasia Kantor"), "{name}");
        assert_eq!(p.password(), Some("kantor"));
        assert_eq!(p.path(), Some(path.as_path()), "{name}");
    }
}

#[test]
fn saves_stay_encrypted() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = Presentation::open_with_password(&corpus("agile.pptx"), Some("kantor")).unwrap();
    let out = dir.path().join("copy.pptx");
    p.save_as(&out).unwrap();
    assert_eq!(
        kantor_crypt::file_protection(&out),
        Some(kantor_crypt::Protection::Ooxml)
    );
    let again = Presentation::open_with_password(&out, Some("kantor")).unwrap();
    assert!(has_text(&again, "Rahasia Kantor"));

    p.set_password(None);
    let plain = dir.path().join("plain.pptx");
    p.save_as(&plain).unwrap();
    assert_eq!(kantor_crypt::file_protection(&plain), None);
    assert!(has_text(
        &Presentation::open(&plain).unwrap(),
        "Rahasia Kantor"
    ));
}

#[test]
fn encrypted_odp_imports_and_keeps_the_password() {
    for name in ["aead.odp", "cbc.odp"] {
        let path = corpus(name);
        let e = Presentation::open(&path).map(|_| ()).unwrap_err();
        assert_eq!(
            crypt_error(&e),
            Some(&kantor_crypt::Error::PasswordNeeded),
            "{name}: {e:#}"
        );
        let p = Presentation::open_with_password(&path, Some("kantor"))
            .unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert!(p.is_imported(), "{name}");
        assert!(has_text(&p, "Rahasia Kantor"), "{name}");
        assert_eq!(p.password(), Some("kantor"));
    }
}
