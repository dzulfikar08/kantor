//! `.docx` round trips and the unsupported-content rules.

use std::io::{Cursor, Read as _, Write as _};
use std::path::Path;

use docs_engine::{
    Align, Document, LINE_BREAK, ListKind, OBJECT, Object, PAGE_BREAK, Paragraph, Position, Rgb,
    RunProps, Selection, TableEdit, Toggle,
};

const W_NS: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";

/// A minimal package around a `document.xml` body, the way Word lays it out,
/// plus an extra custom part that the model knows nothing about.
fn package(body: &str, extra_settings: &str) -> Vec<u8> {
    package_with(body, extra_settings, "", "", Vec::new())
}

/// [`package`] with extra document relationships, content types and parts.
fn package_with(
    body: &str,
    extra_settings: &str,
    extra_rels: &str,
    extra_types: &str,
    extra_parts: Vec<(&str, Vec<u8>)>,
) -> Vec<u8> {
    let content_types = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/>{extra_types}</Types>"#
    );
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    let doc_rels = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings" Target="settings.xml"/>{extra_rels}</Relationships>"#
    );
    let styles = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles {W_NS}><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:asciiTheme="minorHAnsi" w:hAnsiTheme="minorHAnsi"/><w:sz w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="240" w:after="0"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:sz w:val="32"/><w:color w:val="2F5496" w:themeColor="accent1" w:themeShade="BF"/></w:rPr></w:style><w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:left w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:bottom w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:right w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideH w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideV w:val="single" w:sz="4" w:space="0" w:color="auto"/></w:tblBorders></w:tblPr></w:style></w:styles>"#
    );
    let settings = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:settings {W_NS}><w:zoom w:percent="100"/>{extra_settings}</w:settings>"#
    );
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" {W_NS} xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture" xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math" mc:Ignorable="w14"><w:body>{body}<w:sectPr w:rsidR="00AB12CD"><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1080" w:bottom="1440" w:left="1080" w:header="720" w:footer="720" w:gutter="0"/><w:cols w:space="720"/><w:docGrid w:linePitch="360"/></w:sectPr></w:body></w:document>"#
    );
    let mut parts: Vec<(&str, Vec<u8>)> = vec![
        ("[Content_Types].xml", content_types.into_bytes()),
        ("_rels/.rels", rels.as_bytes().to_vec()),
        ("word/_rels/document.xml.rels", doc_rels.into_bytes()),
        ("word/document.xml", document.into_bytes()),
        ("word/styles.xml", styles.into_bytes()),
        ("word/settings.xml", settings.into_bytes()),
        (
            "customXml/item1.xml",
            b"<custom>keep me &amp; my bytes</custom>".to_vec(),
        ),
    ];
    parts.extend(extra_parts);
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in parts {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&data).unwrap();
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
    let body = r#"<w:p><w:r><w:t>Before</w:t></w:r></w:p><w:p><w:r><w:t>item</w:t></w:r><w:ins w:id="1" w:author="a"><w:r><w:t>new</w:t></w:r></w:ins></w:p><w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> REF _Ref1 \h </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Table 1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#;
    let dir = tempfile::tempdir().unwrap();
    let original = package(body, "<w:trackRevisions/>");
    let path = write_temp(dir.path(), "complex.docx", &original);
    let mut doc = Document::open(&path).unwrap();
    let u = doc.unsupported_content().to_vec();
    assert_eq!(
        u,
        ["fields that depend on document content (contents, cross-references…)"],
        "tracked changes and the track changes switch are kept, not unsupported"
    );
    assert!(doc.track_changes(), "the switch is read: Kantor tracks edits");
    assert_eq!(doc.changes().len(), 1);
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
    // The copy keeps the tracked insertion (its own tag around the run) and
    // the field as they were.
    let xml = String::from_utf8(zip_entry(
        &std::fs::read(&copy).unwrap(),
        "word/document.xml",
    ))
    .unwrap();
    assert!(xml.contains(
        r#"<w:ins w:id="1" w:author="a"><w:r><w:t xml:space="preserve">new</w:t></w:r></w:ins>"#
    ));
    assert!(xml.contains(r#"<w:instrText xml:space="preserve"> REF _Ref1 \h </w:instrText>"#));
}

#[test]
fn comments_are_kept_and_other_content_is_carried() {
    let body = r#"<w:p><w:commentRangeStart w:id="0"/><w:r><w:t>a</w:t></w:r><w:commentRangeEnd w:id="0"/><w:r><w:commentReference w:id="0"/></w:r><w:r><w:drawing/></w:r><w:hyperlink r:id="rId9"><w:r><w:rPr><w:rStyle w:val="Hyperlink"/></w:rPr><w:t>link</w:t></w:r></w:hyperlink></w:p><w:p><w:r><w:t>x</w:t><w:br w:type="page"/><w:t>y</w:t></w:r></w:p>"#;
    let read = docs_engine::docx::read(package(body, "")).unwrap();
    assert!(read.unsupported.is_empty(), "{:?}", read.unsupported);
    assert!(
        read.preserved.iter().any(|x| x == "drawings"),
        "{:?}",
        read.preserved
    );
    let p = &read.paragraphs;
    assert!(p[0].text.ends_with("link"));
    let link = p[0].runs.last().unwrap();
    assert_eq!(link.link.as_deref(), Some(r#"<w:hyperlink r:id="rId9">"#));
    assert_eq!(link.props.style.as_deref(), Some("Hyperlink"));
    assert_eq!(p[1].text, format!("x{PAGE_BREAK}y"));
}

/// Open, save untouched, reopen: the same model, and every listed XML
/// fragment written back byte for byte.
fn round_trip(bytes: Vec<u8>, verbatim: &[&str]) -> (Document, Document, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = write_temp(dir.path(), "rt.docx", &bytes);
    let mut doc = Document::open(&path).unwrap();
    assert!(
        doc.unsupported_content().is_empty(),
        "{:?}",
        doc.unsupported_content()
    );
    // Make it dirty without changing it, then save in place.
    let end = doc.end();
    let c = doc.insert_text(Selection::caret(end), "!", None);
    doc.backspace(c);
    doc.save().unwrap();
    let saved = std::fs::read(&path).unwrap();
    let xml = String::from_utf8(zip_entry(&saved, "word/document.xml")).unwrap();
    for v in verbatim {
        assert!(xml.contains(v), "{v} not kept in\n{xml}");
    }
    let back = Document::open(&path).unwrap();
    assert!(back.unsupported_content().is_empty());
    assert_eq!(
        back.paragraphs(),
        doc.paragraphs(),
        "same model after a save"
    );
    (doc, back, xml)
}

const NUMBERING: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:abstractNum w:abstractNumId="0"><w:multiLevelType w:val="hybridMultilevel"/><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl><w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="lowerRoman"/><w:lvlText w:val="%2."/><w:lvlJc w:val="right"/><w:pPr><w:ind w:left="1440" w:hanging="180"/></w:pPr></w:lvl></w:abstractNum><w:abstractNum w:abstractNumId="1"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="&#xF0B7;"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr><w:rPr><w:rFonts w:ascii="Symbol" w:hAnsi="Symbol" w:hint="default"/></w:rPr></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num><w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num></w:numbering>"#;

fn list_package(body: &str) -> Vec<u8> {
    package_with(
        body,
        "",
        r#"<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/>"#,
        r#"<Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/>"#,
        vec![("word/numbering.xml", NUMBERING.as_bytes().to_vec())],
    )
}

fn num_p(text: &str, id: u32, level: u8) -> String {
    format!(
        r#"<w:p><w:pPr><w:pStyle w:val="ListParagraph"/><w:numPr><w:ilvl w:val="{level}"/><w:numId w:val="{id}"/></w:numPr><w:contextualSpacing/></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"#
    )
}

#[test]
fn lists_read_render_and_round_trip() {
    let body = [
        num_p("one", 1, 0),
        num_p("one-a", 1, 1),
        num_p("two", 1, 0),
        "<w:p><w:r><w:t>plain</w:t></w:r></w:p>".into(),
        num_p("dot", 2, 0),
    ]
    .concat();
    let (doc, _, _) = round_trip(list_package(&body), &["<w:numId w:val=\"1\"/>"]);
    let m: Vec<_> = doc
        .list_markers()
        .into_iter()
        .map(|m| m.unwrap_or_default())
        .collect();
    assert_eq!(m, ["1.", "i.", "2.", "", "•"]);
    assert_eq!(doc.list_kind(4), Some(ListKind::Bullet));
    let r = doc.resolve_para(doc.paragraph(1));
    assert_eq!((r.indent_left_pt, r.first_line_pt), (72.0, -9.0));

    // A new list adds definitions; the original part's bytes stay in place.
    let dir = tempfile::tempdir().unwrap();
    let path = write_temp(dir.path(), "list.docx", &list_package(&body));
    let mut doc = Document::open(&path).unwrap();
    // Next to a numbered list, numbering continues it.
    doc.toggle_list(Selection::caret(Position::new(3, 0)), ListKind::Numbered);
    assert_eq!(doc.list_markers()[3].as_deref(), Some("3."));
    // After a bulleted one, a new numbered list starts at 1.
    let end = doc.end();
    let c = doc.insert_text(
        Selection::caret(end),
        "
fresh",
        None,
    );
    doc.toggle_list(c, ListKind::Numbered);
    assert_eq!(
        doc.list_markers()[5].as_deref(),
        Some("1."),
        "a new list restarts"
    );
    doc.save().unwrap();
    let saved = std::fs::read(&path).unwrap();
    let numbering = String::from_utf8(zip_entry(&saved, "word/numbering.xml")).unwrap();
    let head = &NUMBERING[..NUMBERING.find("<w:num ").unwrap()];
    assert!(
        numbering.starts_with(head),
        "original definitions untouched"
    );
    assert!(numbering.contains("<w:num w:numId=\"3\">"));
    let back = Document::open(&path).unwrap();
    assert_eq!(back.list_markers()[3].as_deref(), Some("3."));
    assert_eq!(back.list_markers()[4].as_deref(), Some("•"));
    assert_eq!(back.list_markers()[5].as_deref(), Some("1."));
}

#[test]
fn new_document_with_a_list_gets_a_numbering_part() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new-list.docx");
    let mut doc = Document::new();
    let c = doc.insert_text(Selection::default(), "a\nb", None);
    doc.toggle_list(
        Selection {
            anchor: Position::new(0, 0),
            head: c.head,
        },
        ListKind::Bullet,
    );
    doc.save_as(&path).unwrap();
    let back = Document::open(&path).unwrap();
    assert!(back.unsupported_content().is_empty());
    assert_eq!(back.list_kind(0), Some(ListKind::Bullet));
    assert_eq!(back.list_kind(1), Some(ListKind::Bullet));
    // A second save writes into the package (and keeps the list).
    let mut back = back;
    back.insert_text(Selection::caret(Position::new(1, 1)), "c", None);
    back.save().unwrap();
    let again = Document::open(&path).unwrap();
    assert_eq!(again.list_kind(1), Some(ListKind::Bullet));
    assert_eq!(again.paragraph(1).text, "bc");
}

const TABLE: &str = r#"<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/><w:tblW w:w="0" w:type="auto"/><w:tblLook w:val="04A0" w:firstRow="1" w:lastRow="0" w:firstColumn="1" w:lastColumn="0" w:noHBand="0" w:noVBand="1"/></w:tblPr><w:tblGrid><w:gridCol w:w="3000"/><w:gridCol w:w="3000"/><w:gridCol w:w="3000"/></w:tblGrid><w:tr><w:trPr><w:tblHeader/></w:trPr><w:tc><w:tcPr><w:tcW w:w="6000" w:type="dxa"/><w:gridSpan w:val="2"/><w:shd w:val="clear" w:color="auto" w:fill="D9E2F3"/></w:tcPr><w:p><w:r><w:t>Wide</w:t></w:r></w:p></w:tc><w:tc><w:tcPr><w:tcW w:w="3000" w:type="dxa"/><w:vMerge w:val="restart"/></w:tcPr><w:p><w:r><w:t>Tall</w:t></w:r></w:p><w:p><w:r><w:t>two paras</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:tcPr><w:tcW w:w="3000" w:type="dxa"/></w:tcPr><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc><w:tc><w:tcPr><w:tcW w:w="3000" w:type="dxa"/><w:tcBorders><w:top w:val="nil"/></w:tcBorders></w:tcPr><w:p/></w:tc><w:tc><w:tcPr><w:tcW w:w="3000" w:type="dxa"/><w:vMerge/></w:tcPr><w:p/></w:tc></w:tr></w:tbl>"#;

#[test]
fn tables_read_and_round_trip() {
    let body = format!("<w:p><w:r><w:t>Before</w:t></w:r></w:p>{TABLE}<w:p/>");
    let (doc, back, _) = round_trip(
        package(&body, ""),
        &[
            r#"<w:tblPr><w:tblStyle w:val="TableGrid"/><w:tblW w:w="0" w:type="auto"/>"#,
            r#"<w:trPr><w:tblHeader/></w:trPr>"#,
            r#"<w:shd w:val="clear" w:color="auto" w:fill="D9E2F3"/>"#,
            r#"<w:gridCol w:w="3000"/><w:gridCol w:w="3000"/><w:gridCol w:w="3000"/>"#,
        ],
    );
    let t = doc.table(1).unwrap();
    assert_eq!(t.rows.len(), 2);
    assert_eq!(t.rows[0].cells[0].span, 2);
    assert_eq!(t.rows[0].cells[1].vmerge, docs_engine::VMerge::Restart);
    assert_eq!(t.rows[1].cells[2].vmerge, docs_engine::VMerge::Continue);
    assert!(t.rows[0].header);
    assert_eq!(t.style.as_deref(), Some("TableGrid"));
    assert!(t.rows[1].cells[1].borders.top.is_some_and(|b| !b.on));
    assert_eq!(back.table(1), doc.table(1));
    let texts: Vec<_> = doc.paragraphs().iter().map(|p| p.text.as_str()).collect();
    assert_eq!(
        texts,
        ["Before", "Wide", "Tall", "two paras", "a", "", "", ""]
    );
    // The table style's spacing applies inside cells.
    assert_eq!(doc.resolve_para(doc.paragraph(1)).space_after_pt, 0.0);
    assert_eq!(doc.resolve_para(doc.paragraph(0)).space_after_pt, 8.0);

    // Editing structure: a row below, saved and read back.
    let dir = tempfile::tempdir().unwrap();
    let path = write_temp(dir.path(), "t.docx", &package(&body, ""));
    let mut doc = Document::open(&path).unwrap();
    let c = doc
        .edit_table(Selection::caret(Position::new(4, 0)), TableEdit::RowBelow)
        .unwrap();
    doc.insert_text(c, "new row", None);
    doc.save().unwrap();
    let back = Document::open(&path).unwrap();
    let t = back.table(1).unwrap();
    assert_eq!(t.rows.len(), 3);
    assert_eq!(
        t.rows[2].cells[2].vmerge,
        docs_engine::VMerge::None,
        "no merge copied"
    );
    assert!(
        back.paragraphs()
            .iter()
            .any(|p| p.text == "new row" && p.cell.unwrap().row == 2)
    );
}

/// A 2×1 PNG.
fn tiny_png() -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend(2u32.to_be_bytes());
    png.extend(1u32.to_be_bytes());
    png.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
    png
}

const DRAWING: &str = r#"<w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="952500" cy="476250"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:docPr id="5" name="Picture 5" descr="Chart of Q3"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="0" name="q3.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="rId7"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="952500" cy="476250"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing>"#;

#[test]
fn images_read_insert_and_round_trip() {
    let body = format!(
        "<w:p><w:r><w:t>See </w:t></w:r><w:r>{DRAWING}</w:r><w:r><w:t> here</w:t></w:r></w:p>"
    );
    let bytes = package_with(
        &body,
        "",
        r#"<Relationship Id="rId7" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/>"#,
        r#"<Default Extension="png" ContentType="image/png"/>"#,
        vec![("word/media/image1.png", tiny_png())],
    );
    let (doc, _, _) = round_trip(bytes.clone(), &[DRAWING]);
    let p = doc.paragraph(0);
    assert_eq!(p.text, format!("See {OBJECT} here"));
    let Some(Object::Image(img)) = p.object_at(4) else {
        panic!("image expected")
    };
    assert_eq!(img.size_px(), (100.0, 50.0));
    assert_eq!(img.descr, "Chart of Q3");
    assert_eq!(
        doc.image_bytes(&img.rel_id).as_deref(),
        Some(tiny_png().as_slice())
    );

    // Insert a second picture, save in place, read it back.
    let dir = tempfile::tempdir().unwrap();
    let path = write_temp(dir.path(), "img.docx", &bytes);
    let mut doc = Document::open(&path).unwrap();
    let c = doc
        .insert_image(Selection::caret(Position::new(0, 0)), tiny_png(), "dot.png")
        .unwrap();
    assert_eq!(c.head.offset, OBJECT.len_utf8());
    doc.save().unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert_eq!(zip_entry(&saved, "word/media/image1.png"), tiny_png());
    assert_eq!(
        zip_entry(&saved, "word/media/kantor_image1.png"),
        tiny_png()
    );
    let rels = String::from_utf8(zip_entry(&saved, "word/_rels/document.xml.rels")).unwrap();
    assert!(rels.contains(r#"Id="rIdKantorImage1""#), "{rels}");
    let back = Document::open(&path).unwrap();
    assert!(back.unsupported_content().is_empty());
    let objects: Vec<_> = back.paragraph(0).objects().collect();
    assert_eq!(objects.len(), 2);
    let Object::Image(new) = objects[0].1 else {
        panic!()
    };
    assert_eq!(
        back.image_bytes(&new.rel_id).as_deref(),
        Some(tiny_png().as_slice())
    );
    assert_eq!(new.doc_pr, 6, "unique drawing id");

    // A new document: the picture part, relationship and content type.
    let path = dir.path().join("new-img.docx");
    let mut doc = Document::new();
    doc.insert_image(Selection::default(), tiny_png(), "dot.png")
        .unwrap();
    doc.save_as(&path).unwrap();
    let saved = std::fs::read(&path).unwrap();
    let ct = String::from_utf8(zip_entry(&saved, "[Content_Types].xml")).unwrap();
    assert!(ct.contains(r#"Extension="png""#), "{ct}");
    let back = Document::open(&path).unwrap();
    assert!(back.unsupported_content().is_empty());
    let Some((_, Object::Image(img))) = back.paragraph(0).objects().next() else {
        panic!("image expected")
    };
    assert!(back.image_bytes(&img.rel_id).is_some());
}

#[test]
fn opaque_elements_are_kept_byte_for_byte() {
    let anchor = r#"<w:drawing><wp:anchor distT="0" distB="0" distL="114300" distR="114300" simplePos="0" relativeHeight="1" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="column"><wp:posOffset>0</wp:posOffset></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx="952500" cy="952500"/><wp:wrapSquare wrapText="bothSides"/><wp:docPr id="9" name="Shape"/></wp:anchor></w:drawing>"#;
    let sdt_block = r#"<w:sdt><w:sdtPr><w:docPartObj><w:docPartGallery w:val="Cover Pages"/></w:docPartObj></w:sdtPr><w:sdtContent><w:p><w:r><w:t>Cover</w:t></w:r></w:p></w:sdtContent></w:sdt>"#;
    let sdt_inline = r#"<w:sdt><w:sdtPr><w:alias w:val="Name"/></w:sdtPr><w:sdtContent><w:r><w:t>Budi</w:t></w:r></w:sdtContent></w:sdt>"#;
    let field = r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>3</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#;
    let simple = r#"<w:fldSimple w:instr=" DATE "><w:r><w:t>7/11/2026</w:t></w:r></w:fldSimple>"#;
    let math = r#"<m:oMath><m:r><m:t>x²</m:t></m:r></m:oMath>"#;
    let bookmark = r#"<w:bookmarkStart w:id="3" w:name="intro"/>"#;
    let body = format!(
        "{sdt_block}<w:p>{bookmark}<w:r><w:t>Hi </w:t></w:r>{sdt_inline}<w:r><w:t> p.</w:t></w:r>{field}<w:r><w:rPr><w:highlight w:val=\"yellow\"/><w:b/></w:rPr><w:t>x</w:t></w:r><w:r>{anchor}</w:r>{simple}{math}<w:bookmarkEnd w:id=\"3\"/></w:p><w:p><w:pPr><w:keepNext/><w:tabs><w:tab w:val=\"left\" w:pos=\"720\"/></w:tabs></w:pPr><w:r><w:t>end</w:t></w:r></w:p>"
    );
    let (doc, _, xml) = round_trip(
        package(&body, ""),
        &[
            anchor,
            sdt_block,
            sdt_inline,
            field,
            simple,
            math,
            bookmark,
            "<w:bookmarkEnd w:id=\"3\"/>",
            "<w:highlight w:val=\"yellow\"/>",
            "<w:keepNext/><w:tabs><w:tab w:val=\"left\" w:pos=\"720\"/></w:tabs>",
        ],
    );
    assert!(
        xml.contains("<w:rPr><w:b/><w:highlight w:val=\"yellow\"/></w:rPr>"),
        "schema order"
    );
    let p = doc.paragraph(0);
    assert!(p.is_block_object(), "the cover page is its own block");
    let labels: Vec<_> = doc
        .paragraph(1)
        .objects()
        .map(|(_, o)| o.label().to_owned())
        .collect();
    assert_eq!(
        labels,
        [
            "",
            "Budi",
            "3",
            "Floating image",
            "7/11/2026",
            "Equation",
            ""
        ]
    );
    // Deleting text around a bookmark keeps the bookmark.
    let mut doc = doc;
    let end = doc.paragraph(1).text.len();
    doc.delete_selection(Selection {
        anchor: Position::new(1, 0),
        head: Position::new(1, end),
    });
    let left: Vec<_> = doc
        .paragraph(1)
        .objects()
        .map(|(_, o)| o.is_marker())
        .collect();
    assert_eq!(left, [true, true]);
    assert_eq!(
        doc.text_in(
            Position::new(1, 0),
            Position::new(1, doc.paragraph(1).text.len())
        ),
        ""
    );
}

#[test]
fn not_a_docx_is_an_error() {
    assert!(docs_engine::docx::read(b"hello".to_vec()).is_err());
    let doc = Document::from_paragraphs(vec![Paragraph::new("x")]);
    assert_eq!(doc.title(), "Untitled.docx");
}

#[test]
fn page_setup_changes_only_pgsz_and_pgmar_attributes() {
    use docs_engine::{MarginPreset, Orientation, PaperSize};
    let dir = tempfile::tempdir().unwrap();
    let original = package(WORD_BODY, "");
    let path = write_temp(dir.path(), "page.docx", &original);
    let mut doc = Document::open(&path).unwrap();
    let before = doc.page;
    let page = doc
        .page
        .with_paper(PaperSize::A4)
        .with_orientation(Orientation::Landscape)
        .with_margins(MarginPreset::Narrow);
    doc.set_page(page, Selection::default());
    assert!(doc.is_dirty());
    // One undo step, and redo brings it back.
    doc.undo();
    assert_eq!(doc.page, before);
    assert!(!doc.is_dirty());
    doc.redo();
    assert_eq!(doc.page, page);
    doc.save().unwrap();

    let saved = std::fs::read(&path).unwrap();
    let old = String::from_utf8(zip_entry(&original, "word/document.xml")).unwrap();
    let new = String::from_utf8(zip_entry(&saved, "word/document.xml")).unwrap();
    let sect = |s: &str| {
        let a = s.find("<w:sectPr").unwrap();
        let b = s[a..].find("</w:sectPr>").unwrap() + a + "</w:sectPr>".len();
        s[a..b].to_owned()
    };
    assert_eq!(
        sect(&new),
        sect(&old)
            .replace(
                r#"<w:pgSz w:w="12240" w:h="15840"/>"#,
                r#"<w:pgSz w:w="16838" w:h="11906" w:orient="landscape"/>"#
            )
            .replace(
                r#"w:top="1440" w:right="1080" w:bottom="1440" w:left="1080""#,
                r#"w:top="720" w:right="720" w:bottom="720" w:left="720""#
            )
    );
    for part in [
        "word/styles.xml",
        "word/settings.xml",
        "customXml/item1.xml",
    ] {
        assert_eq!(
            zip_entry(&saved, part),
            zip_entry(&original, part),
            "{part}"
        );
    }
    let back = Document::open(&path).unwrap();
    assert_eq!(back.page, page);
    assert_eq!(back.page.orientation(), Orientation::Landscape);
}

#[test]
fn new_document_saves_its_page_setup() {
    use docs_engine::{Orientation, PaperSize};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f4.docx");
    let mut doc = Document::new();
    assert_eq!(PaperSize::of(&doc.page), Some(PaperSize::A4));
    let page = doc
        .page
        .with_paper(PaperSize::F4)
        .with_orientation(Orientation::Landscape);
    doc.set_page(page, Selection::default());
    doc.save_as(&path).unwrap();
    let back = Document::open(&path).unwrap();
    assert_eq!(back.page, page);
    let xml = String::from_utf8(zip_entry(
        &std::fs::read(&path).unwrap(),
        "word/document.xml",
    ))
    .unwrap();
    assert!(xml.contains("orient=\"landscape\""), "{xml}");
}

#[test]
fn ruler_indents_are_one_undo_step() {
    let mut doc = Document::from_paragraphs(vec![Paragraph::new("a"), Paragraph::new("b")]);
    let all = Selection {
        anchor: Position::new(0, 0),
        head: Position::new(1, 1),
    };
    doc.set_indent(
        all,
        docs_engine::Indent {
            left: Some(720),
            right: None,
            first: Some(-360),
        },
    );
    for p in doc.paragraphs() {
        assert_eq!(p.props.indent.left, Some(720));
        assert_eq!(p.props.indent.first, Some(-360));
        assert_eq!(p.props.indent.right, Some(0));
    }
    doc.undo();
    assert!(doc.paragraphs().iter().all(|p| p.props.indent.is_empty()));
}

#[test]
fn recovery_copy_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let original = package(WORD_BODY, "");
    let path = write_temp(dir.path(), "report.docx", &original);
    let mut doc = Document::open(&path).unwrap();
    let rev = doc.revision();
    let end = Position::new(0, doc.paragraph(0).text.len());
    let c = doc.insert_text(Selection::caret(end), " d", None);
    let r1 = doc.revision();
    assert!(r1 > rev);
    // Typing more of the same word merges into one undo step but is still a change.
    doc.insert_text(c, "raft", None);
    assert!(doc.revision() > r1);

    let copy = doc.to_docx_bytes().unwrap();
    assert!(doc.is_dirty(), "a recovery copy isn't a save");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        original,
        "original untouched"
    );

    let back = Document::open_recovered(copy, Some(path.clone()), Vec::new()).unwrap();
    assert!(back.is_dirty());
    assert_eq!(back.path(), Some(path.as_path()));
    assert_eq!(back.paragraph(0).text, "Laporan Q3 & Q4 draft");
    assert_eq!(&back.paragraphs()[1..], &doc.paragraphs()[1..]);
    assert!(!back.needs_save_as());
    // The parts Kantor doesn't model came through the copy.
    let mut back = back;
    back.save().unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert_eq!(
        zip_entry(&saved, "customXml/item1.xml"),
        zip_entry(&original, "customXml/item1.xml")
    );

    // A document whose original has content Kantor can't keep still never
    // overwrites it after a restore.
    let u = vec!["tracked changes".to_owned()];
    let restored =
        Document::open_recovered(doc.to_docx_bytes().unwrap(), Some(path.clone()), u).unwrap();
    assert!(restored.needs_save_as());
    let mut restored = restored;
    assert!(restored.save().is_err());

    // An untitled document.
    let mut new = Document::new();
    new.insert_text(Selection::default(), "notes", None);
    let back = Document::open_recovered(new.to_docx_bytes().unwrap(), None, Vec::new()).unwrap();
    assert!(back.is_dirty() && back.path().is_none());
    assert_eq!(back.paragraph(0).text, "notes");
}
