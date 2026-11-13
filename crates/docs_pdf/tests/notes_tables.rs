//! Table rows across pages and footnotes/endnotes, from a synthetic
//! `.docx`, laid out and exported: the page on screen and in the PDF come
//! from this same layout.

use std::io::{Cursor, Write as _};

use docs_engine::{Document, NoteKind};
use docs_layout::Engine;

const W_NS: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";
const R_NS: &str =
    "xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"";

fn cell(text: &str) -> String {
    let runs: Vec<String> = text
        .split('\n')
        .map(|l| format!("<w:t xml:space=\"preserve\">{l}</w:t>"))
        .collect();
    format!(
        "<w:tc><w:tcPr><w:tcW w:w=\"4500\" w:type=\"dxa\"/></w:tcPr><w:p><w:pPr><w:spacing w:after=\"0\"/></w:pPr><w:r>{}</w:r></w:p></w:tc>",
        runs.join("<w:br/>")
    )
}

fn row(text: &str, tr_pr: &str) -> String {
    format!("<w:tr>{tr_pr}{}{}</w:tr>", cell(text), cell("·"))
}

/// A table with a header row, 30 short rows, a 70-line row and a
/// 12-line row that can't split; a paragraph with a footnote; an endnote.
fn package() -> Vec<u8> {
    let border =
        |s: &str| format!("<w:{s} w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>");
    let borders: String = ["top", "left", "bottom", "right", "insideH", "insideV"]
        .iter()
        .map(|s| border(s))
        .collect();
    let mut rows = vec![row("Item header", "<w:trPr><w:tblHeader/></w:trPr>")];
    for i in 0..30 {
        rows.push(row(&format!("Row {i}"), ""));
    }
    let tall: Vec<String> = (0..70).map(|k| format!("Tall line {k}")).collect();
    rows.push(row(&tall.join("\n"), ""));
    let kept: Vec<String> = (0..12).map(|k| format!("Kept line {k}")).collect();
    rows.push(row(&kept.join("\n"), "<w:trPr><w:cantSplit/></w:trPr>"));
    let body = format!(
        "<w:p><w:r><w:t xml:space=\"preserve\">Opening claim</w:t></w:r><w:r><w:rPr><w:vertAlign w:val=\"superscript\"/></w:rPr><w:footnoteReference w:id=\"1\"/></w:r><w:r><w:t xml:space=\"preserve\"> and more</w:t></w:r><w:r><w:endnoteReference w:id=\"1\"/></w:r></w:p>\
         <w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblBorders>{borders}</w:tblBorders></w:tblPr><w:tblGrid><w:gridCol w:w=\"4500\"/><w:gridCol w:w=\"4500\"/></w:tblGrid>{}</w:tbl>\
         <w:p><w:r><w:t>After the table.</w:t></w:r></w:p>",
        rows.concat()
    );
    let note = |kind: &str, text: &str| {
        format!(
            "<w:{kind} w:id=\"1\"><w:p><w:pPr><w:spacing w:after=\"0\"/></w:pPr><w:r><w:rPr><w:vertAlign w:val=\"superscript\"/></w:rPr><w:{kind}Ref/></w:r><w:r><w:rPr><w:sz w:val=\"20\"/></w:rPr><w:t xml:space=\"preserve\"> {text}</w:t></w:r></w:p></w:{kind}>"
        )
    };
    let footnotes = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<w:footnotes {W_NS} {R_NS}>{}</w:footnotes>",
        note("footnote", "Quagmire footnote text.")
    );
    let endnotes = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<w:endnotes {W_NS} {R_NS}>{}</w:endnotes>",
        note("endnote", "Zephyr endnote text.")
    );
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<w:document {W_NS} {R_NS}><w:body>{body}<w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/><w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" w:header=\"708\" w:footer=\"708\" w:gutter=\"0\"/></w:sectPr></w:body></w:document>"
    );
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/></Relationships>";
    let doc_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes\" Target=\"footnotes.xml\"/><Relationship Id=\"rId4\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/endnotes\" Target=\"endnotes.xml\"/></Relationships>";
    let types = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/><Override PartName=\"/word/footnotes.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml\"/><Override PartName=\"/word/endnotes.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml\"/></Types>";
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in [
        ("[Content_Types].xml", types.to_owned()),
        ("_rels/.rels", rels.to_owned()),
        ("word/_rels/document.xml.rels", doc_rels.to_owned()),
        ("word/document.xml", document),
        ("word/footnotes.xml", footnotes),
        ("word/endnotes.xml", endnotes),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(data.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn open() -> (tempfile::TempDir, Document) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tables_notes.docx");
    std::fs::write(&path, package()).unwrap();
    let doc = Document::open(&path).unwrap();
    (dir, doc)
}

/// Index of the first paragraph whose text starts with `start`.
fn para(doc: &Document, start: &str) -> usize {
    doc.paragraphs()
        .iter()
        .position(|p| p.text.starts_with(start))
        .unwrap()
}

#[test]
fn rows_break_and_headers_repeat() {
    let (_dir, doc) = open();
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let paged = &laid.paged;
    let header = para(&doc, "Item header");
    let tall = para(&doc, "Tall line 0");
    let kept = para(&doc, "Kept line 0");
    // The tall row starts on the page row 49 ends on and goes on.
    let pages_of = |pi: usize| -> Vec<usize> {
        paged
            .pages
            .iter()
            .enumerate()
            .filter(|(_, p)| p.lines.iter().any(|l| l.para == pi && !l.repeat))
            .map(|(i, _)| i)
            .collect()
    };
    let tall_pages = pages_of(tall);
    assert!(tall_pages.len() >= 2, "{tall_pages:?}");
    assert_eq!(
        tall_pages[0],
        pages_of(tall - 2)[0],
        "no gap before the tall row"
    );
    // The row that can't split sits on one page.
    assert_eq!(pages_of(kept).len(), 1);
    // Every page after the first that shows the table starts with the
    // repeated header row.
    for (i, p) in paged.pages.iter().enumerate().skip(1) {
        let table_lines: Vec<_> = p
            .lines
            .iter()
            .filter(|l| doc.paragraph(l.para).cell.is_some())
            .collect();
        if table_lines.is_empty() {
            continue;
        }
        let first = p
            .lines
            .iter()
            .min_by(|a, b| a.top.total_cmp(&b.top))
            .unwrap();
        assert_eq!(first.para, header, "page {i}");
        assert!(first.repeat);
    }
    // The PDF has the same pages.
    let pdf = docs_pdf::write(&docs_pdf::scene(&doc, &laid)).unwrap();
    let pages = String::from_utf8_lossy(&pdf).matches("/Type/Page").count()
        - String::from_utf8_lossy(&pdf).matches("/Type/Pages").count();
    assert_eq!(pages, paged.pages.len());
}

#[test]
fn footnotes_at_page_bottom_and_endnotes_at_the_end() {
    let (_dir, doc) = open();
    assert_eq!(
        doc.note_text(NoteKind::Footnote, "1"),
        "Quagmire footnote text."
    );
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let paged = &laid.paged;
    let first = &paged.pages[0];
    assert_eq!(first.notes, vec![(0, "Opening claim".len())]);
    let foot: Vec<_> = laid.furniture[0]
        .iter()
        .filter(|p| p.note == Some((NoteKind::Footnote, "1".to_owned())))
        .collect();
    assert_eq!(foot.len(), 1);
    let s = first.setup;
    let [_, y, _, h] = foot[0].rect();
    assert!(
        (y + h - (s.height - s.margin_bottom)).abs() < 0.5,
        "at the bottom of the text area"
    );
    // No body line runs into it.
    for l in &first.lines {
        assert!(l.top + l.height <= y + 0.5);
    }
    // The separator line above it.
    assert!(
        first
            .rules
            .iter()
            .any(|r| r.rect[1] < y && r.rect[1] > y - 20.0 && r.rect[3] < 1.0)
    );
    // The note's number is drawn: its glyph run is a label.
    assert!(
        foot[0]
            .layout
            .paged
            .glyph_runs(0)
            .iter()
            .any(|r| matches!(r.source, docs_layout::RunSource::Mark(_)))
    );
    // The endnote follows the last text.
    let last = laid.furniture.len() - 1;
    assert!(
        laid.furniture[last]
            .iter()
            .any(|p| p.note == Some((NoteKind::Endnote, "1".to_owned())))
    );
    // Both notes are in the PDF.
    let pdf = docs_pdf::write(&docs_pdf::scene(&doc, &laid)).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.pdf");
    docs_pdf::write_file(&file, &pdf).unwrap();
    if let Ok(out) = std::process::Command::new("pdftotext")
        .arg(&file)
        .arg("-")
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("Quagmire footnote text."), "{text}");
        assert!(text.contains("Zephyr endnote text."), "{text}");
    }
}
