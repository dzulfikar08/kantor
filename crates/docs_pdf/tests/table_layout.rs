//! Table alignment, indent and merged cells, from synthetic `.docx`
//! packages: positions after layout, edits, save and reopen.

use std::io::{Cursor, Write as _};

use docs_engine::{Document, Position, Selection, TableAlign, TableProps};
use docs_layout::Block;

const W_NS: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";

/// A package whose body is `body` (A4, 2.54 cm margins: 9026 twips wide).
pub fn package(body: &str) -> Vec<u8> {
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<w:document {W_NS}><w:body>{body}<w:p/><w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/><w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" w:header=\"708\" w:footer=\"708\" w:gutter=\"0\"/></w:sectPr></w:body></w:document>"
    );
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/></Relationships>";
    let types = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/></Types>";
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in [
        ("[Content_Types].xml", types.to_owned()),
        ("_rels/.rels", rels.to_owned()),
        ("word/document.xml", document),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(data.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn open(body: &str) -> (tempfile::TempDir, Document) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.docx");
    std::fs::write(&path, package(body)).unwrap();
    let doc = Document::open(&path).unwrap();
    (dir, doc)
}

fn reopen(dir: &tempfile::TempDir, doc: &mut Document) -> Document {
    let path = dir.path().join("saved.docx");
    doc.save_as(&path).unwrap();
    Document::open(&path).unwrap()
}

/// A 2 × 2 table of 2000-twip columns with `tbl_pr` children.
fn small_table(tbl_pr: &str) -> String {
    let cell = |t: &str| {
        format!(
            "<w:tc><w:tcPr><w:tcW w:w=\"2000\" w:type=\"dxa\"/></w:tcPr><w:p><w:r><w:t>{t}</w:t></w:r></w:p></w:tc>"
        )
    };
    format!(
        "<w:tbl><w:tblPr><w:tblW w:w=\"4000\" w:type=\"dxa\"/>{tbl_pr}</w:tblPr><w:tblGrid><w:gridCol w:w=\"2000\"/><w:gridCol w:w=\"2000\"/></w:tblGrid><w:tr>{}{}</w:tr><w:tr>{}{}</w:tr></w:tbl>",
        cell("a"),
        cell("b"),
        cell("c"),
        cell("d")
    )
}

fn table_x(doc: &Document) -> f32 {
    let (_, _, blocks) = docs_pdf::build::build(doc);
    blocks
        .iter()
        .find_map(|b| match b {
            Block::Table(t) => Some(t.x),
            _ => None,
        })
        .unwrap()
}

/// Twips to layout px.
fn px(tw: f32) -> f32 {
    tw / 15.0
}

#[test]
fn centred_and_right_tables_sit_between_the_margins() {
    let content = 9026.0;
    for (pr, want) in [
        ("", 0.0),
        ("<w:tblInd w:w=\"720\" w:type=\"dxa\"/>", 720.0),
        ("<w:jc w:val=\"center\"/>", (content - 4000.0) / 2.0),
        ("<w:jc w:val=\"right\"/>", content - 4000.0),
        ("<w:jc w:val=\"end\"/>", content - 4000.0),
        // The indent applies to left-aligned tables only, as in Word.
        (
            "<w:jc w:val=\"center\"/><w:tblInd w:w=\"720\" w:type=\"dxa\"/>",
            (content - 4000.0) / 2.0,
        ),
    ] {
        let (_d, doc) = open(&small_table(pr));
        let x = table_x(&doc);
        assert!((x - px(want)).abs() < 0.5, "{pr}: {x} vs {}", px(want));
    }
}

#[test]
fn centred_table_lines_are_placed_centred() {
    let (_d, doc) = open(&small_table("<w:jc w:val=\"center\"/>"));
    let mut engine = docs_layout::Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let page = &laid.paged.pages[0];
    let a = page.lines.iter().find(|l| l.para == 0).unwrap();
    let left = laid.paged.setup.margin_left;
    // The first cell's text starts past the centred table's left edge.
    let edge = left + px((9026.0 - 4000.0) / 2.0);
    assert!(a.x > edge && a.x < edge + px(400.0), "{} vs {edge}", a.x);
}

#[test]
fn table_properties_are_set_undone_and_saved() {
    let (dir, mut doc) = open(&small_table(""));
    let sel = Selection::caret(Position::new(0, 0));
    assert_eq!(
        doc.table_props(sel),
        Some(TableProps {
            align: TableAlign::Left,
            indent: 0
        })
    );
    doc.set_table_props(
        sel,
        TableProps {
            align: TableAlign::Center,
            indent: 0,
        },
    )
    .unwrap();
    assert!((table_x(&doc) - px((9026.0 - 4000.0) / 2.0)).abs() < 0.5);
    doc.undo();
    assert!(table_x(&doc).abs() < 0.5);
    doc.redo();
    let mut back = reopen(&dir, &mut doc);
    assert_eq!(back.table_props(sel).unwrap().align, TableAlign::Center);
    // Left with an indent; the jc stays explicit once the file had one.
    back.set_table_props(
        sel,
        TableProps {
            align: TableAlign::Left,
            indent: 567,
        },
    )
    .unwrap();
    let again = reopen(&dir, &mut back);
    let t = again
        .table(again.paragraphs()[0].cell.unwrap().table)
        .unwrap();
    let pr = t.tbl_pr.as_deref().unwrap();
    assert!(
        pr.contains("<w:jc w:val=\"left\"/><w:tblInd w:w=\"567\""),
        "{pr}"
    );
    assert!((table_x(&again) - px(567.0)).abs() < 0.5);
}

#[test]
fn tracked_table_properties_record_the_old_ones() {
    let (_dir, mut doc) = open(&small_table(""));
    let sel = Selection::caret(Position::new(0, 0));
    doc.set_track_changes(true, sel);
    doc.set_table_props(
        sel,
        TableProps {
            align: TableAlign::Right,
            indent: 0,
        },
    )
    .unwrap();
    let id = doc.paragraphs()[0].cell.unwrap().table;
    let pr = doc.table(id).unwrap().tbl_pr.clone().unwrap();
    assert!(pr.contains("<w:tblPrChange"), "{pr}");
    assert_eq!(doc.changes().len(), 1);
    doc.resolve_all(false, sel);
    assert_eq!(doc.table_props(sel).unwrap().align, TableAlign::Left);
}

#[test]
fn tracked_merges_are_shaded_and_drawn_merged() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/docx/tracked_merge.docx");
    let doc = Document::open(&path).unwrap();
    let (_, _, blocks) = docs_pdf::build::build(&doc);
    let t = blocks
        .iter()
        .find_map(|b| match b {
            Block::Table(t) => Some(t),
            _ => None,
        })
        .unwrap();
    let first = &t.rows[0].cells[0];
    assert_eq!(first.merge, docs_layout::Merge::Restart);
    assert_eq!(first.fill, Some(docs_pdf::build::MERGED_FILL));
    assert_eq!(t.rows[1].cells[0].merge, docs_layout::Merge::Continue);
    assert_eq!(t.rows[0].cells[1].fill, None);
}
