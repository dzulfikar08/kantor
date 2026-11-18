//! `smartart_inline.docx` (scripts/corpus/gen_docx_more.py): SmartArt in
//! line with the text (`wp:inline`) drawn in its line on screen and in the
//! PDF, resized and saved; and a top-and-bottom floating picture that
//! doesn't fit at the bottom of a page moving to the next one with its
//! paragraph, as Word moves it.

use std::path::PathBuf;

use docs_engine::{Document, FloatKind, Object, Selection};
use docs_layout::{Engine, ObjectKind};

fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/docx/smartart_inline.docx")
}

fn open() -> Document {
    Document::open(&path()).unwrap()
}

/// (paragraph, byte index) of the inline SmartArt named `name`.
fn find(doc: &Document, name: &str) -> (usize, usize) {
    for (pi, p) in doc.paragraphs().iter().enumerate() {
        for (index, o) in p.objects() {
            if let Object::Float(f) = o
                && f.name == name
            {
                return (pi, index);
            }
        }
    }
    panic!("{name} not found");
}

#[test]
fn inline_smartart_is_read_as_an_inline_object() {
    let doc = open();
    for name in ["Diagram 1", "Diagram 2"] {
        let (pi, index) = find(&doc, name);
        let f = doc.float_at(pi, index).unwrap();
        assert!(f.inline, "{name}");
        assert!(matches!(f.kind, FloatKind::SmartArt { .. }), "{name}");
        let o = doc.paragraph(pi).object_at(index).unwrap();
        // It takes room in the line and goes with deleted text.
        assert!(!o.is_zero_width());
        assert!(!o.stays_on_delete());
        assert_eq!(
            doc.layout_option(pi, index),
            Some(docs_engine::LayoutOption::Inline)
        );
    }
}

#[test]
fn inline_smartart_is_drawn_in_its_line() {
    let doc = open();
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let page = 0;
    let objects = laid.paged.objects(page);
    for (name, w_in) in [("Diagram 1", 6.0), ("Diagram 2", 2.5)] {
        let (pi, index) = find(&doc, name);
        // A box of its extent in the line (96 px an inch).
        let o = objects
            .iter()
            .find(|o| o.para == pi && o.index == index)
            .unwrap_or_else(|| panic!("{name}: no box in the line"));
        assert_eq!(o.kind, ObjectKind::Drawing);
        assert!(
            (o.whole[2] - w_in * 96.0).abs() < 0.5,
            "{name}: {:?}",
            o.whole
        );
        assert!(
            (o.whole[3] - w_in * 48.0).abs() < 0.5,
            "{name}: {:?}",
            o.whole
        );
        // Drawn into that box as floating SmartArt is, picture included.
        let it = laid.floats[page]
            .iter()
            .find(|it| it.para == pi && it.index == index)
            .unwrap_or_else(|| panic!("{name}: not drawn"));
        assert_eq!(it.rect, o.whole, "{name}");
        let draw = it.chart.as_ref().expect("SmartArt drawn");
        assert!(
            draw.prims
                .iter()
                .any(|p| matches!(p, docs_pdf::floats::FloatPrim::Picture { .. }))
        );
        assert!(!it.boxes.is_empty(), "{name}: its text laid out");
        // Not placed as a floating object by the layout.
        assert!(
            !laid.paged.pages[page]
                .floats
                .iter()
                .any(|pf| pf.para == pi && pf.index == index)
        );
    }
    // The small one shares its line with text: the line holds glyphs
    // on both sides of it.
    let (pi, index) = find(&doc, "Diagram 2");
    let o = objects
        .iter()
        .find(|o| o.para == pi && o.index == index)
        .unwrap();
    let runs = laid.paged.glyph_runs(page);
    let base = o.whole[1] + o.whole[3];
    let beside = |left: bool| {
        runs.iter().filter(|r| r.para == pi).any(|r| {
            (r.baseline - base).abs() < 12.0
                && r.glyphs.iter().any(|g| {
                    if left {
                        g.x < o.whole[0]
                    } else {
                        g.x > o.whole[0] + o.whole[2]
                    }
                })
        })
    };
    assert!(beside(true) && beside(false));
    // The PDF draws its pictures (one cut picture each) among the floats.
    let scene = docs_pdf::scene(&doc, &laid);
    let cut = scene.pages[page]
        .front
        .iter()
        .flat_map(|f| &f.pictures)
        .filter(|p| p.shape.is_some())
        .count();
    assert_eq!(cut, 2);
    let pdf = docs_pdf::export(&doc, &mut engine).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
}

#[test]
fn inline_smartart_resizes_in_place_and_saves() {
    let mut doc = open();
    let (pi, index) = find(&doc, "Diagram 1");
    let f = doc.float_at(pi, index).unwrap().clone();
    let mut at = f.at;
    at.cx /= 2;
    at.cy /= 2;
    // A move is ignored: it stays in its line.
    at.h.1 = docs_engine::float::Place::Offset(914400);
    assert!(doc.set_float_placement(pi, index, at));
    let now = doc.float_at(pi, index).unwrap();
    assert_eq!((now.at.cx, now.at.cy), (f.at.cx / 2, f.at.cy / 2));
    assert_eq!(now.at.h, f.at.h);
    let xml = now.write_xml();
    assert!(xml.contains(&format!("cx=\"{}\" cy=\"{}\"", f.at.cx / 2, f.at.cy / 2)));
    assert!(xml.contains("<wp:inline distT=\"0\""), "{xml}");
    assert!(!xml.contains("anchor") && !xml.contains("wrap"), "{xml}");
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let o = laid
        .paged
        .objects(0)
        .into_iter()
        .find(|o| o.para == pi && o.index == index)
        .unwrap();
    assert!((o.whole[2] - 288.0).abs() < 0.5, "{:?}", o.whole);
    // Saved and read back: still inline, at its new size.
    let dir = std::env::temp_dir().join(format!("kantor-sa-inline-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("resized.docx");
    doc.save_as(&out).unwrap();
    let back = Document::open(&out).unwrap();
    let (pi, index) = find(&back, "Diagram 1");
    let b = back.float_at(pi, index).unwrap();
    assert!(b.inline);
    assert_eq!((b.at.cx, b.at.cy), (f.at.cx / 2, f.at.cy / 2));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn inline_smartart_saved_unchanged_and_deleted_with_text() {
    let mut doc = open();
    let before = std::fs::read(path()).unwrap();
    let dir = std::env::temp_dir().join(format!("kantor-sa-inline-rt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("same.docx");
    doc.insert_text(Selection::default(), "Edited. ", None);
    doc.save_as(&out).unwrap();
    let read = |b: &[u8]| {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(b.to_vec())).unwrap();
        let mut s = String::new();
        std::io::Read::read_to_string(&mut z.by_name("word/document.xml").unwrap(), &mut s)
            .unwrap();
        s
    };
    let (a, b) = (read(&before), read(&std::fs::read(&out).unwrap()));
    let drawings = |x: &str| {
        x.match_indices("<w:drawing>")
            .map(|(i, _)| {
                let end = x[i..].find("</w:drawing>").unwrap();
                x[i..i + end].to_owned()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(drawings(&a).len(), 3);
    assert_eq!(drawings(&a), drawings(&b));
    std::fs::remove_dir_all(&dir).ok();
    // Deleting the line's text deletes the SmartArt in it.
    let (pi, index) = find(&doc, "Diagram 2");
    let len = doc.paragraph(pi).text.len();
    let sel = Selection {
        anchor: docs_engine::Position::new(pi, 0),
        head: docs_engine::Position::new(pi, len),
    };
    doc.delete_selection(sel);
    assert!(
        doc.paragraph(pi)
            .objects()
            .all(|(_, o)| !matches!(o, Object::Float(_))),
        "{index}"
    );
}

#[test]
fn top_and_bottom_object_moves_to_the_next_page_with_its_paragraph() {
    let doc = open();
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let pages = &laid.paged.pages;
    assert_eq!(pages.len(), 2, "{}", pages.len());
    let (pi, index) = find(&doc, "Top and bottom picture");
    let on = |p: usize| {
        pages[p]
            .floats
            .iter()
            .find(|pf| (pf.para, pf.index) == (pi, index))
    };
    assert!(on(0).is_none(), "left on page 1");
    let pf = on(1).expect("on page 2");
    let setup = &pages[1].setup;
    // At the top of the next page's text, inside the page.
    assert!((pf.rect[1] - setup.margin_top).abs() < 0.5, "{:?}", pf.rect);
    assert!(pf.rect[1] + pf.rect[3] <= setup.height - setup.margin_bottom);
    // Its paragraph's text went with it, below it.
    let line = pages[1]
        .lines
        .iter()
        .find(|l| l.para == pi)
        .expect("anchor paragraph on page 2");
    assert!(line.top >= pf.rect[1] + pf.rect[3] - 0.5);
    assert!(!pages[0].lines.iter().any(|l| l.para == pi));
    // The paragraph before it stays on page 1.
    assert!(pages[0].lines.iter().any(|l| l.para == pi - 1));
}
