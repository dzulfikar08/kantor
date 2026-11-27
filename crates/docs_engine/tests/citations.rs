//! Citations from reference managers in `cite_*.docx`
//! (scripts/corpus/gen_docx_citations.py): found, shown as their text, and
//! saved back with their codes byte for byte after an edit elsewhere.

use std::collections::BTreeMap;
use std::io::{Cursor, Read as _};
use std::path::PathBuf;

use docs_engine::{CiteAt, CiteSource, Document, OBJECT, Selection};

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/docx")
        .join(name)
}

fn parts(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        let name = f.name().unwrap().into_owned();
        let mut v = Vec::new();
        f.read_to_end(&mut v).unwrap();
        out.insert(name, v);
    }
    out
}

/// Every element named `name` in `xml` (one that doesn't nest), as
/// written: start tag to end tag, or the empty element.
fn elements(xml: &str, name: &str) -> Vec<String> {
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = xml[at..].find(&open) {
        let start = at + i;
        at = start + open.len();
        // `<w:sdtPr>`, not `<w:sdtPrX>`.
        if !xml[at..].starts_with(['>', ' ', '/']) {
            continue;
        }
        let tag_end = start + xml[start..].find('>').unwrap() + 1;
        let end = if xml[..tag_end].ends_with("/>") {
            tag_end
        } else {
            tag_end + xml[tag_end..].find(&close).unwrap() + close.len()
        };
        out.push(xml[start..end].to_owned());
    }
    out
}

/// What must survive a save byte for byte: field codes (with `w:fldData`),
/// content control properties and citation controls.
fn codes(xml: &str) -> Vec<String> {
    let mut out = elements(xml, "w:instrText");
    out.extend(elements(xml, "w:fldData"));
    out.extend(elements(xml, "w:sdtPr"));
    out
}

fn shown(doc: &Document, i: usize) -> String {
    doc.paragraph(i).text.replace(OBJECT, "")
}

fn find(doc: &Document, text: &str) -> usize {
    (0..doc.len())
        .find(|&i| shown(doc, i).contains(text))
        .unwrap_or_else(|| panic!("no paragraph with {text:?}"))
}

/// Open, type in the paragraph meant for it, save to a copy; check the
/// codes and the other parts came through as they were. Returns the saved
/// document opened again.
fn round_trip(name: &str) -> Document {
    let src = corpus(name);
    let original = std::fs::read(&src).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join(name);
    std::fs::copy(&src, &copy).unwrap();
    let mut doc = Document::open(&copy).unwrap();
    assert!(
        doc.unsupported_content().is_empty(),
        "{name}: {:?}",
        doc.unsupported_content()
    );
    assert!(!doc.needs_save_as(), "{name} saves in place");
    let p = find(&doc, "Paragraf ini diubah");
    let end = docs_engine::Position::new(p, doc.paragraph(p).text.len());
    doc.insert_text(Selection::caret(end), " Sudah diubah.", None);
    doc.save().unwrap();
    let saved = std::fs::read(&copy).unwrap();
    let (before, after) = (parts(&original), parts(&saved));
    let xml =
        |p: &BTreeMap<String, Vec<u8>>| String::from_utf8(p["word/document.xml"].clone()).unwrap();
    assert_eq!(
        codes(&xml(&before)),
        codes(&xml(&after)),
        "{name}: field codes and content controls"
    );
    assert!(xml(&after).contains("Sudah diubah."));
    for (part, bytes) in &before {
        // Saves stamp the modification time into core.xml.
        if part == "word/document.xml" || part == "docProps/core.xml" {
            continue;
        }
        assert_eq!(
            after.get(part),
            Some(bytes),
            "{name}: {part} changed on save"
        );
    }
    let back = Document::open(&copy).unwrap();
    assert_eq!(back.paragraphs(), doc.paragraphs(), "{name}: same model");
    // Saved again unchanged: the same codes still.
    drop(dir);
    back
}

fn titles(c: &docs_engine::Citation) -> Vec<String> {
    c.items
        .iter()
        .map(|i| i.data["title"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[test]
fn zotero_citations_are_found_and_kept() {
    let doc = Document::open(&corpus("cite_zotero.docx")).unwrap();
    let cites = doc.citations();
    assert_eq!(cites.len(), 3);
    assert!(cites.iter().all(|c| c.source == CiteSource::Zotero));
    assert_eq!(cites[0].text, "(Nugroho, 2021)");
    assert_eq!(
        cites[1].text,
        "(Nugroho, 2021, hlm. 12; Santoso & Wijaya, 2019)"
    );
    // The code split over three runs reads as one JSON.
    assert_eq!(
        titles(&cites[1]),
        [
            "Pengaruh media sosial terhadap prestasi belajar mahasiswa",
            "Metodologi penelitian kuantitatif"
        ]
    );
    assert_eq!(cites[1].items[0].locator.as_deref(), Some("12"));
    assert_eq!(cites[1].items[0].label.as_deref(), Some("page"));
    assert_eq!(cites[1].items[0].id, "101");
    assert_eq!(
        cites[1].items[0].uris,
        ["http://zotero.org/users/1234567/items/NUGRH001"]
    );
    assert_eq!(cites[2].items[0].data["author"][2]["family"], "Hidayat");
    assert!(cites.iter().all(|c| !c.edited));
    let bibs = doc.bibliographies();
    assert_eq!(bibs.len(), 1);
    assert_eq!(bibs[0].source, CiteSource::Zotero);
    let CiteAt::Field { begin, end, .. } = bibs[0].at else {
        panic!("a field");
    };
    assert_eq!(end.para - begin.para, 2, "over three paragraphs");
    assert_eq!(
        bibs[0].json.as_ref().unwrap()["uncited"],
        serde_json::json!([])
    );
    // Shown as results.
    let p = find(&doc, "Penelitian sebelumnya");
    assert_eq!(
        shown(&doc, p),
        "Penelitian sebelumnya (Nugroho, 2021) menunjukkan pengaruh yang nyata \
         (Nugroho, 2021, hlm. 12; Santoso & Wijaya, 2019)."
    );
    let back = round_trip("cite_zotero.docx");
    assert_eq!(back.citations().len(), 3);
}

#[test]
fn mendeley_desktop_citations_are_found_and_kept() {
    let doc = Document::open(&corpus("cite_mendeley.docx")).unwrap();
    let cites = doc.citations();
    assert_eq!(cites.len(), 2);
    assert!(cites.iter().all(|c| c.source == CiteSource::Mendeley));
    assert_eq!(cites[0].text, "[1]");
    assert_eq!(cites[1].text, "[2, p. 50]");
    assert_eq!(cites[1].items[0].id, "ITEM-1");
    assert_eq!(cites[1].items[0].locator.as_deref(), Some("50"));
    assert_eq!(titles(&cites[0]), ["Metodologi penelitian kuantitatif"]);
    let bibs = doc.bibliographies();
    assert_eq!(bibs.len(), 1);
    assert_eq!(bibs[0].source, CiteSource::Mendeley);
    round_trip("cite_mendeley.docx");
}

#[test]
fn mendeley_cite_controls_show_their_text_and_are_kept() {
    let doc = Document::open(&corpus("cite_mendeley_cite.docx")).unwrap();
    assert!(
        !doc.preserved_content()
            .iter()
            .any(|p| p == "content controls"),
        "citations aren't placeholders: {:?}",
        doc.preserved_content()
    );
    let cites = doc.citations();
    assert_eq!(cites.len(), 2);
    assert!(cites.iter().all(|c| c.source == CiteSource::MendeleyCite));
    assert_eq!(cites[0].text, "(Nugroho, 2021)");
    assert_eq!(cites[1].text, "(Santoso & Wijaya, 2019)");
    assert!(matches!(cites[0].at, CiteAt::Control { .. }));
    assert_eq!(cites[1].items[0].data["publisher"], "Penerbit Andi");
    // The bibliography control is unwrapped: its entries are paragraphs.
    let bibs = doc.bibliographies();
    assert_eq!(bibs.len(), 1);
    let CiteAt::Paragraphs { first, last } = bibs[0].at else {
        panic!("paragraphs");
    };
    assert_eq!(last - first, 1);
    assert!(shown(&doc, first).starts_with("Nugroho, A. (2021)."));
    assert!(shown(&doc, last).starts_with("Santoso, B., & Wijaya, C. (2019)."));
    assert_eq!(shown(&doc, last + 1), "Lampiran");
    round_trip("cite_mendeley_cite.docx");
}

#[test]
fn word_and_endnote_citations_are_found_and_kept() {
    let doc = Document::open(&corpus("cite_word.docx")).unwrap();
    let sources = doc.word_sources();
    assert_eq!(sources.len(), 2);
    assert_eq!(sources[0].tag, "Nug21");
    assert_eq!(sources[0].data["type"], "article-journal");
    assert_eq!(sources[1].data["author"][1]["family"], "Wijaya");
    assert_eq!(sources[1].data["publisher-place"], "Yogyakarta");
    let cites = doc.citations();
    let kinds: Vec<CiteSource> = cites.iter().map(|c| c.source).collect();
    assert_eq!(
        kinds,
        [CiteSource::Word, CiteSource::Word, CiteSource::EndNote]
    );
    assert_eq!(cites[0].text, "(Nugroho, 2021)");
    assert_eq!(cites[1].items[0].id, "San19");
    assert_eq!(cites[1].items[0].locator.as_deref(), Some("23"));
    assert_eq!(cites[2].text, "(Rahmawati et al., 2022)");
    assert_eq!(cites[2].items[0].data["author"][0]["family"], "Rahmawati");
    assert_eq!(
        cites[2].items[0].data["container-title"],
        "Jurnal Teknologi Informasi"
    );
    let p = find(&doc, "Menurut penelitian");
    assert_eq!(doc.paragraph(p).text.matches(OBJECT).count(), 2);
    // Word's bibliography (nested controls, unwrapped) and EndNote's.
    let bibs: Vec<CiteSource> = doc.bibliographies().iter().map(|b| b.source).collect();
    assert_eq!(bibs, [CiteSource::Word, CiteSource::EndNote]);
    let h = find(&doc, "Daftar Pustaka");
    assert!(shown(&doc, h + 1).starts_with("Nugroho, A. (2021)."));
    round_trip("cite_word.docx");
}

#[test]
fn saving_unedited_keeps_every_part() {
    for name in [
        "cite_zotero.docx",
        "cite_mendeley.docx",
        "cite_mendeley_cite.docx",
        "cite_word.docx",
    ] {
        let doc = Document::open(&corpus(name)).unwrap();
        let bytes = doc.to_docx_bytes().unwrap();
        let (a, b) = (parts(&std::fs::read(corpus(name)).unwrap()), parts(&bytes));
        let xml = |p: &BTreeMap<String, Vec<u8>>| {
            String::from_utf8(p["word/document.xml"].clone()).unwrap()
        };
        assert_eq!(codes(&xml(&a)), codes(&xml(&b)), "{name}");
        assert_eq!(a.keys().collect::<Vec<_>>(), b.keys().collect::<Vec<_>>());
    }
}
