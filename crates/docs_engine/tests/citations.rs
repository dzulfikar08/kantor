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

// ── refreshing and styles ──────────────────────────────────────────────────

fn texts(doc: &Document) -> Vec<String> {
    doc.citations().into_iter().map(|c| c.text).collect()
}

fn instr_json(doc: &Document, i: usize) -> serde_json::Value {
    doc.citations()[i].json.clone().unwrap()
}

#[test]
fn zotero_refresh_and_restyle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("z.docx");
    std::fs::copy(corpus("cite_zotero.docx"), &path).unwrap();
    let mut doc = Document::open(&path).unwrap();
    let prefs = doc.citation_prefs(None);
    assert_eq!(prefs.style, "http://www.zotero.org/styles/apa");
    assert_eq!(prefs.locale.as_deref(), Some("id-ID"));
    assert_eq!(prefs.from, docs_engine::PrefsFrom::Zotero);

    // The fixture's texts are what APA in Indonesian gives.
    let (_, report) = doc.refresh_citations(Selection::default(), None).unwrap();
    assert_eq!(report.citations, 3);
    assert_eq!(report.bibliographies, 1);
    let t = texts(&doc);
    assert_eq!(t[0], "(Nugroho, 2021)");
    assert!(t[1].starts_with("(Nugroho, 2021, "), "{}", t[1]);
    assert!(t[1].ends_with("; Santoso & Wijaya, 2019)"), "{}", t[1]);
    assert_eq!(t[2], "(Rahmawati dkk., 2022)");
    // The code records the text Zotero would compare.
    for i in 0..3 {
        assert_eq!(instr_json(&doc, i)["properties"]["plainCitation"], t[i]);
    }
    // The bibliography: three entries, one paragraph each, sorted.
    let h = find(&doc, "Daftar Pustaka");
    assert!(shown(&doc, h + 1).starts_with("Nugroho, A. (2021). Pengaruh media sosial"));
    assert!(shown(&doc, h + 2).starts_with("Rahmawati, D., Pratama, E., & Hidayat, F. (2022)."));
    assert!(shown(&doc, h + 3).starts_with("Santoso, B., & Wijaya, C. (2019)."));
    assert_eq!(
        doc.paragraph(h + 3).props.style.as_deref(),
        Some("Bibliography")
    );
    // Titles in italics as runs.
    let p = doc.paragraph(h + 3);
    let italic: String = p
        .run_ranges()
        .filter(|(_, r)| r.props.italic == Some(true))
        .map(|(r, _)| p.text[r].to_owned())
        .collect();
    assert!(
        italic.contains("Metodologi penelitian kuantitatif"),
        "{italic:?}"
    );
    assert_eq!(doc.bibliographies().len(), 1);

    // IEEE: numbers, and the preference written the way Zotero keeps it.
    doc.set_citation_prefs("http://www.zotero.org/styles/ieee", Some("en-US"));
    doc.refresh_citations(Selection::default(), None).unwrap();
    let t = texts(&doc);
    assert_eq!(t[0], "[1]");
    assert!(t[1].starts_with("[1"), "{}", t[1]);
    assert!(t[1].contains('2'), "{}", t[1]);
    assert_eq!(t[2], "[3]");
    let h = find(&doc, "Daftar Pustaka");
    assert!(
        shown(&doc, h + 1).starts_with("[1]\t"),
        "{}",
        shown(&doc, h + 1)
    );
    assert_eq!(doc.bibliographies().len(), 1);
    doc.save().unwrap();
    let saved = parts(&std::fs::read(&path).unwrap());
    let custom = String::from_utf8(saved["docProps/custom.xml"].clone()).unwrap();
    assert!(custom.contains("ZOTERO_PREF_1"));
    assert!(
        custom.contains("http://www.zotero.org/styles/ieee"),
        "{custom}"
    );
    assert!(custom.contains("session id=&quot;Kq3vXz9P&quot;") || custom.contains("Kq3vXz9P"));
    let back = Document::open(&path).unwrap();
    assert_eq!(
        back.citation_prefs(None).style,
        "http://www.zotero.org/styles/ieee"
    );
    assert_eq!(texts(&back), t);
    assert!(back.unsupported_content().is_empty());

    // Undo brings the old texts back.
    let mut doc = back;
    doc.set_citation_prefs("http://www.zotero.org/styles/apa", Some("id-ID"));
    doc.refresh_citations(Selection::default(), None).unwrap();
    assert_eq!(texts(&doc)[0], "(Nugroho, 2021)");
    doc.undo();
    assert_eq!(texts(&doc)[0], "[1]");
}

#[test]
fn mendeley_desktop_refresh_uses_its_style() {
    let mut doc = Document::open(&corpus("cite_mendeley.docx")).unwrap();
    let prefs = doc.citation_prefs(None);
    assert_eq!(prefs.style, "http://www.zotero.org/styles/ieee");
    assert_eq!(prefs.from, docs_engine::PrefsFrom::Mendeley);
    doc.refresh_citations(Selection::default(), Some("en-US"))
        .unwrap();
    let t = texts(&doc);
    assert_eq!(t[0], "[1]");
    assert!(t[1].starts_with("[2"), "{}", t[1]);
    assert!(t[1].contains("50"), "{}", t[1]);
    assert_eq!(
        instr_json(&doc, 0)["mendeley"]["plainTextFormattedCitation"],
        "[1]"
    );
    let h = find(&doc, "Referensi");
    assert!(
        shown(&doc, h + 1).starts_with("[1]\tB. Santoso and C. Wijaya"),
        "{}",
        shown(&doc, h + 1)
    );
    // Restyling a Mendeley document keeps Mendeley's property.
    doc.set_citation_prefs("http://www.zotero.org/styles/apa", None);
    assert_eq!(
        doc.citation_prefs(None).from,
        docs_engine::PrefsFrom::Mendeley
    );
    let bytes = doc.to_docx_bytes().unwrap();
    let custom = String::from_utf8(parts(&bytes)["docProps/custom.xml"].clone()).unwrap();
    assert!(
        custom.contains("<vt:lpwstr>http://www.zotero.org/styles/apa</vt:lpwstr>"),
        "{custom}"
    );
    assert!(!custom.contains("ZOTERO_PREF"));
}

#[test]
fn mendeley_cite_refresh_rewrites_controls() {
    let mut doc = Document::open(&corpus("cite_mendeley_cite.docx")).unwrap();
    assert_eq!(
        doc.citation_prefs(None).from,
        docs_engine::PrefsFrom::Default
    );
    doc.set_citation_prefs("http://www.zotero.org/styles/ieee", None);
    doc.refresh_citations(Selection::default(), Some("en-US"))
        .unwrap();
    assert_eq!(texts(&doc), ["[1]", "[2]"]);
    let c = &doc.citations()[0];
    assert_eq!(
        c.json.as_ref().unwrap()["manualOverride"]["citeprocText"],
        "[1]"
    );
    let bibs = doc.bibliographies();
    assert_eq!(bibs.len(), 1);
    let CiteAt::Paragraphs { first, last } = bibs[0].at else {
        panic!("paragraphs");
    };
    assert_eq!(last - first, 1);
    assert!(
        shown(&doc, first).starts_with("[1]\tA. Nugroho"),
        "{}",
        shown(&doc, first)
    );
    assert_eq!(shown(&doc, last + 1), "Lampiran");
    // Saved and read again: the same.
    let bytes = doc.to_docx_bytes().unwrap();
    let xml = String::from_utf8(parts(&bytes)["word/document.xml"].clone()).unwrap();
    assert!(xml.contains("<w:tag w:val=\"MENDELEY_BIBLIOGRAPHY\"/>"));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.docx");
    std::fs::write(&path, &bytes).unwrap();
    let back = Document::open(&path).unwrap();
    assert_eq!(texts(&back), ["[1]", "[2]"]);
    assert_eq!(back.bibliographies().len(), 1);
}

#[test]
fn word_and_endnote_citations_are_left_alone() {
    let mut doc = Document::open(&corpus("cite_word.docx")).unwrap();
    let before = doc.paragraphs().to_vec();
    let (_, report) = doc.refresh_citations(Selection::default(), None).unwrap();
    assert_eq!(report.other, 3);
    assert_eq!(report.citations, 0);
    assert_eq!(doc.paragraphs(), before.as_slice());
}

#[test]
fn new_documents_get_preferences_and_a_source_list() {
    let mut doc = Document::new();
    doc.insert_text(Selection::default(), "Teks.", None);
    doc.set_citation_prefs("http://www.zotero.org/styles/apa", Some("id-ID"));
    let items = vec![
        serde_json::json!({"id": "nugroho2021", "type": "book", "title": "Satu", "issued": {"date-parts": [[2021]]}}),
        serde_json::json!({"id": "b", "type": "book", "title": "Dua"}),
    ];
    assert_eq!(doc.add_sources(items.clone()), 2);
    assert_eq!(
        doc.add_sources(vec![items[0].clone()]),
        0,
        "the same item again"
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.docx");
    doc.save_as(&path).unwrap();
    let saved = parts(&std::fs::read(&path).unwrap());
    let rels = String::from_utf8(saved["_rels/.rels"].clone()).unwrap();
    assert!(rels.contains("Target=\"docProps/custom.xml\""), "{rels}");
    let ct = String::from_utf8(saved["[Content_Types].xml"].clone()).unwrap();
    assert!(ct.contains("/docProps/custom.xml"));
    assert!(ct.contains("customXmlProperties+xml"));
    let doc_rels = String::from_utf8(saved["word/_rels/document.xml.rels"].clone()).unwrap();
    assert!(doc_rels.contains("relationships/customXml\""), "{doc_rels}");
    let back = Document::open(&path).unwrap();
    assert_eq!(back.sources(), items);
    assert_eq!(back.citation_prefs(None).locale.as_deref(), Some("id-ID"));
    // Saved again: the same parts, not a second list.
    let mut back = back;
    back.add_sources(vec![serde_json::json!({"id": "c", "title": "Tiga"})]);
    back.save().unwrap();
    let again = parts(&std::fs::read(&path).unwrap());
    assert_eq!(
        again
            .keys()
            .filter(|k| k.starts_with("customXml/item"))
            .count(),
        1
    );
    assert_eq!(Document::open(&path).unwrap().sources().len(), 3);
}

// ── inserting ──────────────────────────────────────────────────────────────

const BIB: &str = r#"
@article{nugroho2021,
  author = {Nugroho, Adi},
  title = {Pengaruh media sosial terhadap prestasi belajar mahasiswa},
  journal = {Jurnal Pendidikan Indonesia},
  year = {2021}, volume = {10}, number = {2}, pages = {115--130}
}
@book{santoso2019,
  author = {Santoso, Budi and Wijaya, Citra},
  title = {Metodologi penelitian kuantitatif},
  publisher = {Penerbit Andi}, address = {Yogyakarta}, year = {2019}
}
"#;

fn bib_items() -> Vec<docs_engine::CiteItem> {
    kantor_cite::import::from_bib(BIB)
        .unwrap()
        .into_iter()
        .map(docs_engine::CiteItem::new)
        .collect()
}

#[test]
fn citations_inserted_as_zotero_fields() {
    let mut doc = Document::new();
    let c = doc.insert_text(Selection::default(), "Hasil penelitian sebelumnya .", None);
    // Before the full stop.
    let at = docs_engine::Position::new(0, c.head.offset - 1);
    let items = bib_items();
    let after = doc
        .insert_citation(Selection::caret(at), &items, Some("id-ID"))
        .unwrap();
    assert_eq!(
        shown(&doc, 0),
        "Hasil penelitian sebelumnya (Nugroho, 2021; Santoso & Wijaya, 2019)."
    );
    let cites = doc.citations();
    assert_eq!(cites.len(), 1);
    assert_eq!(cites[0].source, CiteSource::Zotero);
    let j = cites[0].json.as_ref().unwrap();
    assert_eq!(j["citationID"].as_str().unwrap().len(), 8);
    assert_eq!(j["citationItems"][0]["id"], "nugroho2021");
    assert_eq!(
        j["citationItems"][1]["itemData"]["publisher"],
        "Penerbit Andi"
    );
    assert_eq!(
        j["properties"]["plainCitation"],
        "(Nugroho, 2021; Santoso & Wijaya, 2019)"
    );
    // The caret is after the citation; typing goes after it.
    doc.insert_text(after, " Lagi", None);
    assert!(
        shown(&doc, 0).ends_with("2019) Lagi."),
        "{}",
        shown(&doc, 0)
    );
    // The preferences Zotero's plug-in needs were written.
    assert_eq!(
        doc.citation_prefs(None).from,
        docs_engine::PrefsFrom::Zotero
    );
    assert_eq!(doc.citation_prefs(None).locale.as_deref(), Some("id-ID"));

    // A bibliography in a new paragraph.
    let end = doc.end();
    doc.newline(Selection::caret(end), None);
    let end = doc.end();
    doc.insert_bibliography(Selection::caret(end), Some("id-ID"))
        .unwrap();
    let bibs = doc.bibliographies();
    assert_eq!(bibs.len(), 1);
    assert_eq!(bibs[0].source, CiteSource::Zotero);
    assert!(
        shown(&doc, 1).starts_with("Nugroho, A. (2021)."),
        "{}",
        shown(&doc, 1)
    );
    assert!(shown(&doc, 2).starts_with("Santoso, B., & Wijaya, C. (2019)."));

    // IEEE: a new first citation shifts the numbers.
    doc.set_citation_prefs("http://www.zotero.org/styles/ieee", Some("en-US"));
    doc.refresh_citations(Selection::default(), None).unwrap();
    assert_eq!(texts(&doc), ["[1], [2]"]);
    let santoso = vec![items[1].clone()];
    doc.insert_citation(Selection::default(), &santoso, None)
        .unwrap();
    assert_eq!(texts(&doc), ["[1]", "[1], [2]"]);
    assert!(
        shown(&doc, 1).starts_with("[1]\tB. Santoso"),
        "{}",
        shown(&doc, 1)
    );

    // One undo step each.
    doc.undo();
    assert_eq!(texts(&doc), ["[1], [2]"]);

    // Saved and opened again: still Zotero fields Kantor and Zotero read.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("baru.docx");
    doc.save_as(&path).unwrap();
    let back = Document::open(&path).unwrap();
    assert_eq!(texts(&back), ["[1], [2]"]);
    assert_eq!(
        back.citations()[0].items[1].data["title"],
        "Metodologi penelitian kuantitatif"
    );
    let xml = String::from_utf8(parts(&std::fs::read(&path).unwrap())["word/document.xml"].clone())
        .unwrap();
    assert!(xml.contains("ADDIN ZOTERO_ITEM CSL_CITATION {"));
    assert!(xml.contains("ADDIN ZOTERO_BIBL {&quot;uncited&quot;:[],&quot;omitted&quot;:[],&quot;custom&quot;:[]} CSL_BIBLIOGRAPHY") || xml.contains(r#"ADDIN ZOTERO_BIBL {"uncited":[],"omitted":[],"custom":[]} CSL_BIBLIOGRAPHY"#));
}

#[test]
fn mendeley_documents_get_mendeley_fields() {
    let mut doc = Document::open(&corpus("cite_mendeley.docx")).unwrap();
    assert_eq!(doc.new_citation_source(), CiteSource::Mendeley);
    let p = find(&doc, "Paragraf ini diubah");
    let at = docs_engine::Position::new(p, doc.paragraph(p).text.len());
    doc.insert_citation(Selection::caret(at), &bib_items()[..1], Some("en-US"))
        .unwrap();
    let cites = doc.citations();
    assert_eq!(cites.len(), 3);
    assert!(cites.iter().all(|c| c.source == CiteSource::Mendeley));
    assert_eq!(cites[2].items[0].id, "ITEM-1");
    assert_eq!(cites[2].text, "[3]");
    let h = find(&doc, "Referensi");
    assert!(
        shown(&doc, h + 3).starts_with("[3]\tA. Nugroho"),
        "{}",
        shown(&doc, h + 3)
    );
    assert_eq!(
        doc.citation_prefs(None).from,
        docs_engine::PrefsFrom::Mendeley
    );
}
