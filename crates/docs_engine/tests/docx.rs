//! `.docx` round trips and the unsupported-content rules.

use std::io::{Cursor, Read as _, Write as _};
use std::path::Path;

use docs_engine::{
    Align, Document, LINE_BREAK, Paragraph, Position, Rgb, RunProps, Selection, Toggle,
};

const W_NS: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";

/// A minimal package around a `document.xml` body, the way Word lays it out,
/// plus an extra custom part that the model knows nothing about.
fn package(body: &str, extra_settings: &str) -> Vec<u8> {
    let content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/></Types>"#;
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    let doc_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings" Target="settings.xml"/></Relationships>"#;
    let styles = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles {W_NS}><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:asciiTheme="minorHAnsi" w:hAnsiTheme="minorHAnsi"/><w:sz w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="240" w:after="0"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:sz w:val="32"/><w:color w:val="2F5496" w:themeColor="accent1" w:themeShade="BF"/></w:rPr></w:style></w:styles>"#
    );
    let settings = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:settings {W_NS}><w:zoom w:percent="100"/>{extra_settings}</w:settings>"#
    );
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" {W_NS} xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" mc:Ignorable="w14"><w:body>{body}<w:sectPr w:rsidR="00AB12CD"><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1080" w:bottom="1440" w:left="1080" w:header="720" w:footer="720" w:gutter="0"/><w:cols w:space="720"/><w:docGrid w:linePitch="360"/></w:sectPr></w:body></w:document>"#
    );
    let parts: Vec<(&str, String)> = vec![
        ("[Content_Types].xml", content_types.into()),
        ("_rels/.rels", rels.into()),
        ("word/_rels/document.xml.rels", doc_rels.into()),
        ("word/document.xml", document),
        ("word/styles.xml", styles),
        ("word/settings.xml", settings),
        (
            "customXml/item1.xml",
            "<custom>keep me &amp; my bytes</custom>".into(),
        ),
    ];
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in parts {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(data.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// A body like Word writes: rsids, proofing marks, `_GoBack`, language tags.
const WORD_BODY: &str = r#"<w:p w:rsidR="00A1" w14:paraId="1A2B3C4D"><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Laporan</w:t></w:r><w:r><w:t xml:space="preserve"> Q3 &amp; Q4</w:t></w:r></w:p><w:p><w:pPr><w:jc w:val="both"/><w:spacing w:before="120" w:after="60" w:line="360" w:lineRule="auto"/><w:rPr><w:lang w:val="id-ID"/></w:rPr></w:pPr><w:bookmarkStart w:id="0" w:name="_GoBack"/><w:bookmarkEnd w:id="0"/><w:r><w:rPr><w:rFonts w:ascii="Arial" w:hAnsi="Arial"/><w:b/><w:bCs/><w:lang w:val="id-ID"/></w:rPr><w:t>Tebal</w:t></w:r><w:proofErr w:type="spellStart"/><w:r><w:rPr><w:i/><w:u w:val="single"/><w:color w:val="C0392B"/><w:sz w:val="28"/><w:szCs w:val="28"/></w:rPr><w:t xml:space="preserve"> miring</w:t></w:r><w:proofErr w:type="spellEnd"/><w:r><w:tab/><w:t>tab</w:t><w:br/><w:t>baris</w:t><w:lastRenderedPageBreak/></w:r></w:p><w:p><w:pPr><w:rPr><w:b/></w:rPr></w:pPr></w:p>"#;

fn zip_entry(bytes: &[u8], name: &str) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = Vec::new();
    zip.by_name(name).unwrap().read_to_end(&mut out).unwrap();
    out
}

fn write_temp(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn reads_word_style_body() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_temp(dir.path(), "word.docx", &package(WORD_BODY, ""));
    let doc = Document::open(&path).unwrap();
    assert!(
        doc.unsupported_content().is_empty(),
        "{:?}",
        doc.unsupported_content()
    );
    let p = doc.paragraphs();
    assert_eq!(p.len(), 3);
    assert_eq!(p[0].text, "Laporan Q3 & Q4");
    assert_eq!(p[0].props.style.as_deref(), Some("Heading1"));
    assert_eq!(p[1].text, format!("Tebal miring\ttab{LINE_BREAK}baris"));
    assert_eq!(p[1].props.align, Some(Align::Justify));
    assert_eq!(p[1].props.before, Some(120));
    assert_eq!(p[1].runs[0].props.bold, Some(true));
    assert_eq!(p[1].runs[0].props.fonts.ascii.as_deref(), Some("Arial"));
    let miring = &p[1].runs[1].props;
    assert_eq!(miring.italic, Some(true));
    assert_eq!(miring.underline, Some(true));
    assert_eq!(miring.color, Some(Rgb([0xC0, 0x39, 0x2B])));
    assert_eq!(miring.size, Some(28));
    // The empty paragraph keeps its mark formatting for typing.
    assert_eq!(p[2].runs[0].props.bold, Some(true));
    // Letter page from sectPr.
    assert_eq!((doc.page.width, doc.page.margin_left), (12240, 1080));
    // Styles resolve through basedOn; the heading is 16 pt.
    let r = doc.resolved_at(Position::new(0, 1));
    assert_eq!(r.size_pt, 16.0);
    assert_eq!(r.font, "Calibri");
}

#[test]
fn save_edits_body_and_keeps_every_other_part() {
    let dir = tempfile::tempdir().unwrap();
    let original = package(WORD_BODY, "");
    let path = write_temp(dir.path(), "keep.docx", &original);
    let mut doc = Document::open(&path).unwrap();
    let end = Position::new(0, doc.paragraph(0).text.len());
    let caret = doc.insert_text(Selection::caret(end), " final", None);
    doc.toggle(
        Selection {
            anchor: Position::new(0, 0),
            head: caret.head,
        },
        Toggle::Italic,
    );
    assert!(doc.is_dirty());
    doc.save().unwrap();
    assert!(!doc.is_dirty());

    let saved = std::fs::read(&path).unwrap();
    for part in [
        "word/styles.xml",
        "word/settings.xml",
        "customXml/item1.xml",
        "[Content_Types].xml",
        "word/_rels/document.xml.rels",
    ] {
        assert_eq!(
            zip_entry(&saved, part),
            zip_entry(&original, part),
            "{part} changed"
        );
    }
    let xml = String::from_utf8(zip_entry(&saved, "word/document.xml")).unwrap();
    assert!(
        xml.contains(r#"<w:sectPr w:rsidR="00AB12CD"><w:pgSz w:w="12240""#),
        "section properties kept verbatim"
    );
    assert!(xml.contains("mc:Ignorable=\"w14\""), "root tag kept");

    let again = Document::open(&path).unwrap();
    assert!(again.unsupported_content().is_empty());
    assert_eq!(again.paragraph(0).text, "Laporan Q3 & Q4 final");
    assert_eq!(again.paragraph(0).runs[0].props.italic, Some(true));
    // Everything else reads back identically.
    assert_eq!(&again.paragraphs()[1..], &doc.paragraphs()[1..]);
    assert_eq!(again.page, doc.page);
}

#[test]
fn new_document_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.docx");
    let mut doc = Document::new();
    let mut c = doc.insert_text(Selection::default(), "Judul", None);
    doc.set_style(c, Some("Heading1"));
    c = doc.insert_text(c, "\n", None);
    let bold = RunProps {
        bold: Some(true),
        ..RunProps::default()
    };
    c = doc.insert_text(c, "Halo ", Some(bold));
    c = doc.insert_text(c, "dunia\tdengan tab", Some(RunProps::default()));
    doc.set_align(c, Align::Center);
    doc.insert_text(c, "\n", None);
    assert!(doc.needs_save_as());
    doc.save_as(&path).unwrap();
    assert!(!doc.is_dirty());
    assert!(!doc.needs_save_as());

    let back = Document::open(&path).unwrap();
    assert!(
        back.unsupported_content().is_empty(),
        "{:?}",
        back.unsupported_content()
    );
    let texts: Vec<_> = back.paragraphs().iter().map(|p| p.text.as_str()).collect();
    assert_eq!(texts, vec!["Judul", "Halo dunia\tdengan tab", ""]);
    assert_eq!(back.paragraph(0).props.style.as_deref(), Some("Heading1"));
    assert_eq!(back.paragraph(1).props.align, Some(Align::Center));
    assert_eq!(back.paragraph(1).runs[0].props.bold, Some(true));
    assert_eq!(back.resolved_at(Position::new(0, 1)).size_pt, 16.0);
    assert_eq!(back.resolved_at(Position::new(1, 1)).size_pt, 11.0);
    assert_eq!(back.paragraphs(), doc.paragraphs());
    assert_eq!(back.page, doc.page);

    // A second save goes into the package written by the first.
    let mut back = back;
    back.insert_text(Selection::caret(Position::new(2, 0)), "lagi", None);
    back.save().unwrap();
    let third = Document::open(&path).unwrap();
    assert_eq!(third.paragraph(2).text, "lagi");
}

#[test]
fn unsupported_content_never_overwrites_the_original() {
    let body = r#"<w:p><w:r><w:t>Before</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:rPr><w:highlight w:val="yellow"/></w:rPr><w:t>item</w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:ins w:id="1" w:author="a"><w:r><w:t>new</w:t></w:r></w:ins></w:p>"#;
    let dir = tempfile::tempdir().unwrap();
    let original = package(body, "<w:trackRevisions/>");
    let path = write_temp(dir.path(), "complex.docx", &original);
    let mut doc = Document::open(&path).unwrap();
    let u = doc.unsupported_content().to_vec();
    for want in [
        "tables",
        "lists",
        "<w:highlight>",
        "fields (page numbers, contents…)",
        "tracked changes",
        "track changes switched on",
    ] {
        assert!(u.iter().any(|x| x == want), "{want} missing from {u:?}");
    }
    assert!(doc.needs_save_as());
    assert_eq!(doc.suggested_file_name(), "complex (Kantor).docx");
    doc.insert_text(Selection::default(), "x", None);

    assert!(doc.save().is_err(), "Save must not overwrite");
    assert!(
        doc.save_as(&path).is_err(),
        "Save As onto the original must fail"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        original,
        "original untouched"
    );

    let copy = dir.path().join("copy.docx");
    doc.save_as(&copy).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        original,
        "original untouched"
    );
    assert!(!doc.has_unsupported_content(), "the copy is all Kantor's");
    assert_eq!(doc.path(), Some(copy.as_path()));
    let back = Document::open(&copy).unwrap();
    assert_eq!(back.paragraph(0).text, "xBefore");
}

#[test]
fn headers_images_and_comments_are_unsupported() {
    let body = r#"<w:p><w:commentRangeStart w:id="0"/><w:r><w:t>a</w:t></w:r><w:commentRangeEnd w:id="0"/><w:r><w:drawing/></w:r><w:hyperlink r:id="rId9" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:r><w:t>link</w:t></w:r></w:hyperlink></w:p><w:p><w:r><w:br w:type="page"/></w:r></w:p>"#;
    let bytes = package(body, "");
    // Swap the sectPr for one with a header reference.
    let read = docs_engine::docx::read(bytes).unwrap();
    let u = read.unsupported;
    for want in [
        "comments",
        "images or drawings",
        "hyperlinks",
        "page or column breaks",
    ] {
        assert!(u.iter().any(|x| x == want), "{want} missing from {u:?}");
    }
    let header_body = WORD_BODY.to_owned()
        + r#"<w:p><w:pPr><w:sectPr><w:headerReference w:type="default" r:id="rId8" xmlns:r="x"/></w:sectPr></w:pPr></w:p>"#;
    let read = docs_engine::docx::read(package(&header_body, "")).unwrap();
    assert!(
        read.unsupported.iter().any(|x| x == "section breaks"),
        "{:?}",
        read.unsupported
    );
}

#[test]
fn not_a_docx_is_an_error() {
    assert!(docs_engine::docx::read(b"hello".to_vec()).is_err());
    let doc = Document::from_paragraphs(vec![Paragraph::new("x")]);
    assert_eq!(doc.title(), "Untitled.docx");
}
