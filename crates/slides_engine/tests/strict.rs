//! Strict Open XML presentations (ISO/IEC 29500 Strict): `tests/corpus/strict/deck.pptx`
//! is `pptx/deck.pptx` as Strict (scripts/corpus/gen_strict.py): Strict
//! namespaces and relationship types, DrawingML and PresentationML
//! percentages as "35%". It opens as the original does and saves as
//! Transitional.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use slides_engine::Presentation;

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus")
        .join(name)
}

fn parts(path: &Path) -> Vec<(String, String)> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut out = Vec::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        let name = f.name().unwrap().to_string();
        if name.ends_with(".xml") || name.ends_with(".rels") {
            let mut s = String::new();
            f.read_to_string(&mut s).unwrap();
            out.push((name, s));
        }
    }
    out
}

/// Shapes compare by what was read (their equality also asks for the same
/// source part, which two opened files never share).
fn read<T: std::fmt::Debug>(x: &T) -> String {
    format!("{x:?}")
}

fn same(a: &Presentation, b: &Presentation) {
    assert_eq!(a.size, b.size);
    assert_eq!(a.slides().len(), b.slides().len());
    for (i, (x, y)) in a.slides().iter().zip(b.slides()).enumerate() {
        assert_eq!(read(&x.shapes), read(&y.shapes), "slide {i}");
        assert_eq!(read(&x.background), read(&y.background), "slide {i}");
    }
    assert_eq!(a.layouts.len(), b.layouts.len());
    for (x, y) in a.layouts.iter().zip(&b.layouts) {
        assert_eq!(read(&x.shapes), read(&y.shapes), "{}", x.part);
    }
    for (x, y) in a.masters.iter().zip(&b.masters) {
        assert_eq!(read(&x.shapes), read(&y.shapes), "{}", x.part);
        assert_eq!(read(&x.background), read(&y.background), "{}", x.part);
    }
}

#[test]
fn strict_presentation_opens_like_its_original() {
    let original = Presentation::open(&corpus("pptx/deck.pptx")).unwrap();
    let mut strict = Presentation::open(&corpus("strict/deck.pptx")).unwrap();
    same(&original, &strict);
    assert!(!strict.is_imported());

    // Saved in place as Transitional (PowerPoint's default), and reopened the same.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deck.pptx");
    std::fs::copy(corpus("strict/deck.pptx"), &path).unwrap();
    let mut p = Presentation::open(&path).unwrap();
    p.set_notes_text(0, "Edited.").unwrap();
    p.save().unwrap();
    for (name, xml) in parts(&path) {
        assert!(!xml.contains("purl.oclc.org"), "{name}");
        assert!(!xml.contains("conformance"), "{name}");
        assert!(!xml.contains("%\""), "{name} keeps a Strict percentage");
    }
    let back = Presentation::open(&path).unwrap();
    strict.set_notes_text(0, "Edited.").unwrap();
    same(&strict, &back);
}
