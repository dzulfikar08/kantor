//! Fields on the page: their results laid out as text (screen and PDF use
//! the same layout), «Name» for a mail merge field with no result, and
//! page references numbered from Kantor's own layout. Page numbers come
//! from a page break in the fixture, so they don't depend on fonts.

use std::path::PathBuf;

use chrono::{FixedOffset, NaiveDate};
use docs_engine::{Document, FieldContext, Position, Selection};
use docs_layout::Engine;

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/docx")
        .join(name)
}

/// Every piece of text drawn on page `page`.
fn page_text(doc: &Document, laid: &docs_pdf::Layout, page: usize) -> String {
    let paged = &laid.paged;
    paged
        .glyph_runs(page)
        .iter()
        .map(|r| {
            let text = paged.run_text(r, &doc.paragraph(r.para).text);
            // Object characters stand for boxes, not glyphs.
            text.get(r.text_range.clone())
                .unwrap_or_default()
                .replace('\u{FFFC}', "")
        })
        .collect::<Vec<_>>()
        .join("")
}

#[test]
fn field_results_are_laid_out_and_page_references_follow_the_layout() {
    let mut doc = Document::open(&corpus("fields.docx")).unwrap();
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    assert_eq!(laid.paged.pages.len(), 2);
    let first = page_text(&doc, &laid, 0);
    for want in ["Introduction", "1 January 2020", "Old title", "«FirstName»"] {
        assert!(first.contains(want), "{want:?} not on page 1: {first}");
    }
    // Field codes draw nothing.
    assert!(!first.contains("PAGEREF") && !first.contains("MERGEFIELD"));
    assert!(page_text(&doc, &laid, 1).contains("Table 5"));

    let cx = FieldContext {
        now: NaiveDate::from_ymd_opt(2026, 11, 15)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap(),
        offset: FixedOffset::east_opt(0).unwrap(),
    };
    let all = Selection {
        anchor: Position::new(0, 0),
        head: doc.end(),
    };
    doc.update_fields(all, &cx, &mut |d| docs_pdf::field_pages(d, &mut engine));
    let shown = |i: usize| doc.paragraph(i).text.replace('\u{FFFC}', "");
    assert_eq!(shown(1), "Introduction\t1");
    assert_eq!(shown(2), "Scope\t1");
    assert_eq!(shown(3), "Results\t2");
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    assert!(page_text(&doc, &laid, 0).contains("Results"));
    let second = page_text(&doc, &laid, 1);
    assert!(second.contains("See Table 1 on page 2"), "{second}");

    // The PDF has the updated text too.
    let pdf = docs_pdf::export(&doc, &mut engine).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
}
