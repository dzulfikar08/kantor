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

/// Alt+F9: a field's code takes the place of its result in the layout
/// (the result stays in the text, hidden), page numbers too.
#[test]
fn field_codes_replace_results_in_the_layout() {
    let mut doc = docs_engine::Document::open(&corpus("fields.docx")).unwrap();
    let date = doc
        .fields()
        .into_iter()
        .find(|f| f.kind() == "DATE")
        .unwrap();
    let (paras, _, _) = docs_pdf::build::build(&doc);
    let lp = &paras[date.begin.para];
    assert!(lp.hidden.is_empty());
    assert!(!lp.objects.iter().any(|o| o.label.starts_with("{ DATE")));
    doc.set_field_codes(Some(docs_engine::CodeView {
        all: true,
        ids: Default::default(),
    }));
    let (paras, _, _) = docs_pdf::build::build(&doc);
    let lp = &paras[date.begin.para];
    let code = lp
        .objects
        .iter()
        .find(|o| o.index == date.begin.offset)
        .unwrap();
    assert_eq!(code.kind, docs_layout::ObjectKind::Text);
    assert!(code.label.starts_with("{ DATE"), "{}", code.label);
    assert!(code.label.ends_with(" }"));
    let (a, b) = date.result();
    assert!(lp.hidden.contains(&(a.offset..b.offset)), "{:?}", lp.hidden);
    // Only the field at the caret (Shift+F9).
    let mut one = docs_engine::CodeView::default();
    one.ids.insert(date.mark_id());
    doc.set_field_codes(Some(one));
    let (paras, _, _) = docs_pdf::build::build(&doc);
    let shown_codes = paras
        .iter()
        .flat_map(|p| p.objects.iter())
        .filter(|o| o.label.starts_with("{ "))
        .count();
    assert_eq!(shown_codes, 1);
    // Laid out, the hidden result takes (almost) no room.
    let mut engine = docs_layout::Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    assert!(!laid.paged.pages.is_empty());
}

/// A field's code in the codes view shows the codes nested in it.
#[test]
fn field_codes_show_nested_codes() {
    let mut doc = docs_engine::Document::open(&corpus("fields.docx")).unwrap();
    let date = doc
        .fields()
        .into_iter()
        .find(|f| f.kind() == "DATE")
        .unwrap();
    let cx = FieldContext {
        now: NaiveDate::from_ymd_opt(2026, 11, 15)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap(),
        offset: FixedOffset::east_opt(0).unwrap(),
    };
    let mut engine = Engine::new();
    let code = "IF { DATE \\@ \"yyyy\" } = \"2026\" \"yes\" \"no\"";
    assert!(doc.set_field_code(
        date.begin,
        code,
        Selection::caret(date.begin),
        &cx,
        &mut |d| docs_pdf::field_pages(d, &mut engine)
    ));
    doc.set_field_codes(Some(docs_engine::CodeView {
        all: true,
        ids: Default::default(),
    }));
    let (paras, _, _) = docs_pdf::build::build(&doc);
    let label = &paras[date.begin.para]
        .objects
        .iter()
        .find(|o| o.index == date.begin.offset)
        .unwrap()
        .label;
    assert_eq!(label, &format!("{{ {code} }}"));
}

/// Field codes typed in the text: the caret walks the code shown between
/// the braces, a click maps back to it, and a code edited this way keeps
/// the result until it's updated (Word: F9).
#[test]
fn codes_in_the_text_take_a_caret_and_keep_the_result() {
    let mut doc = Document::open(&corpus("fields.docx")).unwrap();
    doc.set_field_codes(Some(docs_engine::CodeView {
        all: true,
        ids: Default::default(),
    }));
    let f = doc
        .fields()
        .into_iter()
        .find(|f| f.kind() == "DATE")
        .unwrap();
    let code = f.shown_code();
    let label = format!("{{ {code} }}");
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let paged = &laid.paged;
    let (p, i) = (f.begin.para, f.begin.offset);
    let first = paged.label_caret(p, i, 2).unwrap();
    let last = paged.label_caret(p, i, label.len() - 2).unwrap();
    assert!(last.x > first.x + 5.0, "{first:?} {last:?}");
    assert_eq!(first.page, last.page);
    // A click at a caret's spot finds that byte again.
    for at in [2, 4, label.len() - 2] {
        let c = paged.label_caret(p, i, at).unwrap();
        let hit = paged.label_hit(p, i, c.page, c.x + 0.2, c.top + c.height / 2.0);
        assert_eq!(hit, Some(at), "{label:?} at {at}");
    }
    // Off the label: nothing.
    assert_eq!(
        paged.label_hit(p, i, first.page, first.x - 40.0, first.top),
        None
    );

    let before = doc.paragraph(p).text.clone();
    let new = code.replace("DATE", "TIME");
    assert!(doc.set_field_instr(f.begin, &new, Selection::caret(f.begin)));
    let g = doc
        .fields()
        .into_iter()
        .find(|g| g.begin == f.begin)
        .unwrap();
    assert_eq!(g.shown_code(), new.trim());
    assert_eq!(doc.paragraph(p).text, before, "the result stays until F9");
    doc.undo();
    let g = doc
        .fields()
        .into_iter()
        .find(|g| g.begin == f.begin)
        .unwrap();
    assert_eq!(g.shown_code(), code);
}

/// `STYLEREF` in a header shows each page's heading (the first on the
/// page, else the last before it), and `INDEX` has letter headings and
/// cross-references (tests/corpus/docx/styleref_index.docx from
/// scripts/corpus/gen_docx_more.py; pages from page breaks).
#[test]
fn style_refs_in_headers_follow_the_page_and_the_index_has_headings() {
    let mut doc = Document::open(&corpus("styleref_index.docx")).unwrap();
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    assert_eq!(laid.paged.pages.len(), 4);
    let header = |laid: &docs_pdf::Layout, page: usize| -> String {
        laid.furniture[page]
            .iter()
            .find(|p| p.kind == docs_engine::HfKind::Header)
            .map(|p| p.layout.paras[0].text.replace('\u{FFFC}', ""))
            .unwrap_or_default()
    };
    assert_eq!(header(&laid, 0), "Chapter: Alpha chapter");
    assert_eq!(
        header(&laid, 1),
        "Chapter: Alpha chapter",
        "no heading: the one before"
    );
    assert_eq!(header(&laid, 2), "Chapter: Beta chapter");
    assert_eq!(header(&laid, 3), "Chapter: Beta chapter");
    // The header itself keeps its saved result.
    let story = doc.stories()[laid.furniture[0][0].layout.story]
        .doc
        .paragraphs()[0]
        .text
        .replace('\u{FFFC}', "");
    assert_eq!(story, "Chapter: 0");

    let cx = FieldContext {
        now: NaiveDate::from_ymd_opt(2026, 11, 17)
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
    let last = doc.len() - 1;
    let index = doc.paragraph(last).text.replace('\u{FFFC}', "");
    let lines: Vec<&str> = index.split('\u{2028}').collect();
    assert_eq!(
        lines,
        [
            "A",
            "Apples, 1, 3",
            "Avocados, 3",
            "B",
            "Bananas, 2",
            "F",
            "Fruit. See Apples",
            "    Apples, 1",
            "Z",
            "Zebras, 1",
        ],
        "{index:?}"
    );
}
