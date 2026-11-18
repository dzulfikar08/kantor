//! `columns_long.docx` (scripts/corpus/gen_docx_more.py): 20 pages of
//! exact 14 pt lines in two, three and unequal columns, continuous section
//! breaks, footnotes, exact-height tables and pictures. Where lines land
//! doesn't depend on fonts, so the places below are LibreOffice 25.8's,
//! from its PDF of the file without its footnote references
//! (LibreOffice puts a page's footnotes across the whole page, Word and
//! Kantor under each column, so with notes the two differ by design).

use std::io::{Read as _, Write as _};
use std::path::PathBuf;

use docs_engine::Document;
use docs_layout::{Engine, PagedDocument};

fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/docx/columns_long.docx")
}

/// The fixture with its footnote references taken out.
fn without_notes(dir: &std::path::Path) -> PathBuf {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path()).unwrap()).unwrap();
    let out = dir.join("nonotes.docx");
    let mut w = zip::ZipWriter::new(std::fs::File::create(&out).unwrap());
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        let name = f.name().unwrap().into_owned();
        let mut data = Vec::new();
        f.read_to_end(&mut data).unwrap();
        if name == "word/document.xml" {
            let mut s = String::from_utf8(data).unwrap();
            let pre = r#"<w:r><w:rPr><w:vertAlign w:val="superscript"/></w:rPr><w:footnoteReference w:id=""#;
            while let Some(i) = s.find(pre) {
                let end = s[i..].find("</w:r>").unwrap() + i + "</w:r>".len();
                s.replace_range(i..end, "");
            }
            data = s.into_bytes();
        }
        w.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(&data).unwrap();
    }
    w.finish().unwrap();
    out
}

/// Page (1-based), left edge and top (pt) of the first line of the
/// paragraph whose text is `t`.
fn at(doc: &Document, paged: &PagedDocument, t: &str) -> (usize, f32, f32) {
    let pi = doc
        .paragraphs()
        .iter()
        .position(|p| p.text == t)
        .unwrap_or_else(|| panic!("no paragraph {t}"));
    for (n, page) in paged.pages.iter().enumerate() {
        if let Some(l) = page.lines.iter().find(|l| l.para == pi && !l.repeat) {
            return (n + 1, l.x * 0.75, l.top * 0.75);
        }
    }
    panic!("{t} not placed")
}

/// LibreOffice's page, left edge and top (pt) of the first line of each
/// column (and of the sections' last lines).
const LIBREOFFICE: &[(&str, usize, f32, f32)] = &[
    ("Long columns", 1, 72.0, 72.0),
    ("A 48", 1, 316.0, 100.0),
    ("A 91", 2, 72.0, 86.0),
    ("A 132", 2, 316.0, 72.0),
    ("A 181", 3, 72.0, 142.5),
    ("A 225", 3, 316.0, 72.0),
    ("A 267", 4, 72.0, 72.0),
    ("A 311", 4, 316.0, 72.0),
    ("A 360", 5, 72.0, 72.0),
    ("A 397", 5, 316.0, 72.0),
    ("A 446", 6, 72.0, 72.0),
    ("A 483", 6, 316.0, 72.0),
    ("A 532", 7, 72.0, 72.0),
    ("A 576", 7, 316.0, 72.0),
    ("A 618", 8, 72.0, 72.0),
    ("Between 5", 9, 72.0, 72.0),
    ("B 49", 9, 232.0, 86.0),
    ("B 97", 9, 392.0, 86.0),
    ("B 140", 10, 72.0, 72.0),
    ("B 182", 10, 232.0, 72.0),
    ("B 226", 10, 392.0, 72.0),
    ("B 275", 11, 72.0, 72.0),
    ("B 317", 11, 232.0, 72.0),
    ("B 361", 11, 392.0, 72.0),
    ("B 410", 12, 72.0, 72.0),
    ("B 451", 12, 232.0, 180.0),
    ("B 493", 12, 392.0, 72.0),
    ("B 542", 13, 72.0, 72.0),
    ("B 586", 13, 232.0, 72.0),
    ("B 628", 13, 392.0, 72.0),
    ("B 672", 14, 72.0, 72.0),
    ("B 721", 14, 232.0, 72.0),
    ("B 763", 14, 392.0, 72.0),
    ("B 807", 15, 72.0, 72.0),
    ("B 856", 15, 232.0, 72.0),
    ("B 900", 15, 392.0, 72.0),
    ("B end", 15, 392.0, 194.0),
    ("C 1", 16, 72.0, 72.0),
    ("C 50", 16, 238.0, 72.0),
    ("C 99", 17, 72.0, 72.0),
    ("C 141", 17, 238.0, 72.0),
    ("C 190", 18, 72.0, 72.0),
    ("C 232", 18, 238.0, 72.0),
    ("C 281", 19, 72.0, 72.0),
    ("C end", 19, 72.0, 460.0),
    ("Last 1", 20, 72.0, 72.0),
];

#[test]
fn long_columns_land_where_libreoffice_puts_them() {
    let dir = tempfile::tempdir().unwrap();
    let doc = Document::open(&without_notes(dir.path())).unwrap();
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let paged = &laid.paged;
    assert_eq!(paged.pages.len(), 20, "LibreOffice's page count");
    for &(t, page, x, top) in LIBREOFFICE {
        let (p, kx, ktop) = at(&doc, paged, t);
        assert!(
            p == page && (kx - x).abs() < 2.0 && (ktop - top).abs() < 1.5,
            "{t}: LibreOffice page {page} x {x} top {top}, Kantor page {p} x {kx} top {ktop}"
        );
    }
    // The two-column section's last page is balanced with one line more
    // on the left (Word's way; LibreOffice puts the odd line on the
    // right): A 657 ends the first column, A 658 starts the second.
    let (p, x, _) = at(&doc, paged, "A 657");
    assert_eq!((p, x), (8, 72.0));
    let (p, x, top) = at(&doc, paged, "A 658");
    assert!(p == 8 && x > 300.0 && (top - 72.0).abs() < 0.5);
    if let Ok(out) = std::env::var("KANTOR_COLS_PDF") {
        std::fs::write(out, docs_pdf::export(&doc, &mut engine).unwrap()).unwrap();
    }
}

#[test]
fn long_columns_put_footnotes_under_their_column() {
    let doc = Document::open(&path()).unwrap();
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let paged = &laid.paged;
    assert_eq!(paged.pages.len(), 21);
    let mut notes = 0;
    for (n, page) in paged.pages.iter().enumerate() {
        let boxes: Vec<[f32; 4]> = page.columns.iter().flatten().copied().collect();
        for nc in &page.note_columns {
            notes += nc.parts.len();
            if boxes.is_empty() {
                continue;
            }
            // Under a column of the page, as wide as it.
            assert!(
                boxes
                    .iter()
                    .any(|b| (b[0] - nc.x).abs() < 0.5 && (b[2] - nc.width).abs() < 0.5),
                "page {}: {:?} not under {boxes:?}",
                n + 1,
                (nc.x, nc.width)
            );
        }
        // The text stays above its column's notes.
        for nc in &page.note_columns {
            let top = nc.bottom - nc.height;
            for l in page.lines.iter().filter(|l| (l.x - nc.x).abs() < 0.5) {
                assert!(l.top + l.height <= top + 0.5, "page {}", n + 1);
            }
        }
    }
    assert_eq!(notes, doc.note_refs().len());
    // The unequal columns' notes: each laid out at its own column's width.
    let p = paged
        .pages
        .iter()
        .find(|p| {
            p.note_columns.len() > 1
                && (p.note_columns[0].width - p.note_columns[1].width).abs() > 50.0
        })
        .expect("a page of unequal columns with notes in both");
    let placed = &laid.furniture[paged.pages.iter().position(|q| std::ptr::eq(q, p)).unwrap()];
    for nc in &p.note_columns {
        assert!(placed.iter().any(|f| f.note.is_some()
            && (f.x - nc.x).abs() < 0.5
            && (f.layout.paged.setup.content_width() - nc.width).abs() < 0.5));
    }
}
