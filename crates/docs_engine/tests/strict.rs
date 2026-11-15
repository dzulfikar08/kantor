//! Strict Open XML documents (ISO/IEC 29500 Strict): `tests/corpus/strict/report.docx`
//! is `docx/report.docx` as Strict (scripts/corpus/gen_strict.py): Strict
//! namespaces and relationship types, lengths with units ("1in", "90pt"),
//! font sizes in points, start/end alignment and indents. It opens as the
//! original does and saves as Transitional.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use docs_engine::{Document, Position, Selection};

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus")
        .join(name)
}

fn part(path: &Path, name: &str) -> String {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut s = String::new();
    zip.by_name(name).unwrap().read_to_string(&mut s).unwrap();
    s
}

#[test]
fn strict_document_opens_like_its_original() {
    let original = Document::open(&corpus("docx/report.docx")).unwrap();
    let mut strict = Document::open(&corpus("strict/report.docx")).unwrap();
    assert_eq!(strict.paragraphs(), original.paragraphs());
    assert_eq!(strict.unsupported_content(), original.unsupported_content());
    assert!(!strict.is_imported());

    // Saved in place as Transitional (Word's default), and reopened the same.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("report.docx");
    std::fs::copy(corpus("strict/report.docx"), &path).unwrap();
    let mut doc = Document::open(&path).unwrap();
    assert_eq!(doc.path(), Some(path.as_path()));
    let caret = Selection::caret(Position::new(0, 0));
    doc.insert_text(caret, "Edited. ", None);
    doc.save().unwrap();
    for name in [
        "_rels/.rels",
        "word/_rels/document.xml.rels",
        "word/document.xml",
        "word/styles.xml",
    ] {
        let xml = part(&path, name);
        assert!(!xml.contains("purl.oclc.org"), "{name}");
        assert!(!xml.contains("conformance"), "{name}");
    }
    // Page size and margins in twips again.
    let body = part(&path, "word/document.xml");
    assert!(
        body.contains(r#"<w:pgSz w:w="11906" w:h="16838"/>"#),
        "{body}"
    );
    assert!(body.contains(r#"w:top="1440""#), "{body}");
    let back = Document::open(&path).unwrap();
    strict.insert_text(caret, "Edited. ", None);
    assert_eq!(back.paragraphs(), strict.paragraphs());
}
