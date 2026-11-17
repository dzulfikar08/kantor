//! `columns.docx` (scripts/corpus/gen_docx_more.py): section columns laid
//! out as Word and LibreOffice do. Every line is exactly 14 pt and too
//! short to wrap, so where each one lands doesn't depend on fonts; the
//! expected pages are LibreOffice 25.8's (3 pages, the same lines in the
//! same columns).

use std::path::PathBuf;

use docs_engine::{Columns, ColumnsApply, Document, Position, Selection};
use docs_layout::{Engine, PagedDocument};

fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/docx/columns.docx")
}

/// Paragraph index of the one whose text is `t`.
fn para(doc: &Document, t: &str) -> usize {
    doc.paragraphs()
        .iter()
        .position(|p| p.text.trim_start_matches(docs_engine::COLUMN_BREAK) == t)
        .unwrap_or_else(|| panic!("no paragraph {t}"))
}

/// Page, left edge and top of paragraph `t`'s first line.
fn at(doc: &Document, paged: &PagedDocument, t: &str) -> (usize, f32, f32) {
    let pi = para(doc, t);
    for (n, page) in paged.pages.iter().enumerate() {
        if let Some(l) = page.lines.iter().find(|l| l.para == pi) {
            return (n, l.x, l.top);
        }
    }
    panic!("{t} not placed")
}

#[test]
fn columns_land_where_libreoffice_puts_them() {
    let doc = Document::open(&path()).unwrap();
    let s = doc.sections();
    assert_eq!(s.len(), 4);
    assert_eq!(
        s.iter().map(|s| s.props.columns.count()).collect::<Vec<_>>(),
        vec![1, 2, 1, 2]
    );
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let paged = &laid.paged;
    assert_eq!(paged.pages.len(), 3, "LibreOffice's page count");
    let left = paged.pages[0].setup.margin_left;

    // Page 1: items 1-20, then the column break before 21; 21-65 fill
    // column two (46 lines, under the first section's 3).
    let (p, x, _) = at(&doc, paged, "Item 20");
    assert_eq!((p, x), (0, left));
    let i21 = para(&doc, "Item 21");
    let line21 = paged.pages[0]
        .lines
        .iter()
        .find(|l| l.para == i21 && l.line == 1)
        .expect("Item 21 after its break");
    let (x2, top21) = (line21.x, line21.top);
    assert!(x2 > left + 200.0);
    assert_eq!(top21, at(&doc, paged, "Item 1").2, "column two starts level with one");
    assert_eq!(at(&doc, paged, "Item 66").0, 0);
    // Page 2: balanced before the continuous break, 67-98 | 99-130.
    let (p, x, top67) = at(&doc, paged, "Item 67");
    assert_eq!((p, x, top67), (1, left, paged.pages[1].setup.margin_top));
    assert_eq!(at(&doc, paged, "Item 98").1, left);
    let (p, x, top) = at(&doc, paged, "Item 99");
    assert_eq!((p, x, top), (1, x2, top67));
    // The one-column section after them sits under the longer column.
    let (p, x, after) = at(&doc, paged, "After 1");
    let below98 = at(&doc, paged, "Item 98").2 + 14.0 * 96.0 / 72.0;
    assert_eq!((p, x), (1, left));
    assert!((after - below98).abs() < 0.5, "{after} {below98}");
    // Page 3 (next page): a narrow left column, then a wide one.
    assert_eq!(at(&doc, paged, "Left 49").1, left);
    let (p, x, _) = at(&doc, paged, "Left 50");
    assert_eq!(p, 2);
    let wide = paged.pages[2]
        .lines
        .iter()
        .find(|l| l.x == x)
        .expect("a line in the wide column");
    assert!(x > left + 150.0 && wide.width > 300.0);

    // Lines between the columns of the first section only (no w:sep on
    // the last), on pages 1 and 2; page 2's as long as its columns.
    let seps = |n: usize| {
        paged.pages[n]
            .rules
            .iter()
            .filter(|r| r.rect[2] < 1.0 && r.rect[3] > 20.0)
            .count()
    };
    assert_eq!((seps(0), seps(1), seps(2)), (1, 1, 0));

    // The PDF has the same pages.
    let pdf = docs_pdf::export(&doc, &mut engine).unwrap();
    let count = |n: &[u8]| pdf.windows(n.len()).filter(|w| *w == n).count();
    assert_eq!(count(b"/Type/Page") - count(b"/Type/Pages"), 3);
}

#[test]
fn saving_keeps_the_column_markup() {
    let dir = std::env::temp_dir().join(format!("kantor-columns-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut doc = Document::open(&path()).unwrap();
    // An edit elsewhere: the sections' w:cols stay as they were.
    let p = para(&doc, "Item 5");
    doc.insert_text(Selection::caret(Position::new(p, 0)), "New ", None);
    let out = dir.join("edited.docx");
    doc.save_as(&out).unwrap();
    let xml = |path: &std::path::Path| {
        let f = std::fs::File::open(path).unwrap();
        let mut z = zip::ZipArchive::new(f).unwrap();
        let mut s = String::new();
        std::io::Read::read_to_string(&mut z.by_name("word/document.xml").unwrap(), &mut s)
            .unwrap();
        s
    };
    let (before, after) = (xml(&path()), xml(&out));
    for cols in [
        r#"<w:cols w:num="2" w:sep="1" w:space="720"/>"#,
        r#"<w:cols w:num="2" w:space="720" w:equalWidth="0"><w:col w:w="2600" w:space="720"/><w:col w:w="5706"/></w:cols>"#,
    ] {
        assert_eq!(before.matches(cols).count(), after.matches(cols).count(), "{cols}");
    }
    assert!(after.contains(r#"<w:br w:type="column"/>"#));

    // Three columns for the last section, saved and read back.
    let last = doc.paragraphs().len() - 1;
    doc.set_columns(
        Selection::caret(Position::new(last, 0)),
        &Columns::equal(3, 567),
        ColumnsApply::ThisSection,
    );
    let out = dir.join("three.docx");
    doc.save_as(&out).unwrap();
    let back = Document::open(&out).unwrap();
    assert_eq!(back.section_columns(last), Columns::equal(3, 567));
    assert_eq!(back.section_columns(0).count(), 1);
    assert_eq!(back.sections()[1].props.columns.count(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}
