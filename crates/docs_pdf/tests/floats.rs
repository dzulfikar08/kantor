//! Floating objects of `floats.docx` (scripts/corpus/gen_docx.py) laid out
//! and exported: on their page, the text wrapping around them, drawn
//! behind or in front of it, their text boxes' text in the PDF.

use std::path::PathBuf;

use docs_engine::{Document, Object};
use docs_layout::Engine;

fn open() -> Document {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/docx/floats.docx");
    Document::open(&path).unwrap()
}

#[test]
fn laid_out_drawn_and_exported() {
    let doc = open();
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let items: Vec<_> = laid.floats.iter().flatten().collect();
    assert_eq!(items.len(), 5);
    let name = |it: &docs_pdf::FloatItem| it.float(&doc).unwrap().name.clone();
    // All five on the first page; behind ones first, then by z-order.
    let first: Vec<String> = laid.floats[0].iter().map(name).collect();
    assert_eq!(
        first,
        [
            "Ellipse behind",
            "Square picture",
            "Cropped picture",
            "Text Box 1",
            "Arrow"
        ]
    );
    let setup = laid.paged.page_setup(0);
    let square = &laid.floats[0][1];
    // Right-aligned in the margins, at its paragraph's top.
    assert!((square.rect[0] + square.rect[2] - (setup.width - setup.margin_right)).abs() < 0.01);
    // No line runs into it.
    for l in &laid.paged.pages[0].lines {
        let r = square.rect;
        let beside = l.top < r[1] + r[3] && l.top + l.height > r[1];
        if beside {
            assert!(l.x + l.width <= r[0] - 12.0 + 0.01, "{l:?} into {r:?}");
        }
    }
    // The cropped picture is fixed to the page: 600 px down.
    let cropped = &laid.floats[0][2];
    assert_eq!(cropped.rect[1], 600.0);
    // The text box is laid out, inset in its box.
    let tb = &laid.floats[0][3];
    let (l, x, y) = tb.text.as_ref().unwrap();
    assert!(*x > tb.rect[0] && *y > tb.rect[1]);
    assert_eq!(l.paras.len(), 2);
    // The scene: one behind, four in front; the cropped picture clipped to
    // its box and drawn twice as wide (a quarter cut off each side).
    let scene = docs_pdf::scene(&doc, &laid);
    let page = &scene.pages[0];
    assert_eq!(page.behind.len(), 1);
    assert_eq!(page.front.len(), 4);
    assert!(!page.behind[0].paths.is_empty());
    let pic = &page.front[1].pictures[0];
    assert_eq!(pic.clip, Some(cropped.rect));
    assert!((pic.rect[2] - 2.0 * cropped.rect[2]).abs() < 0.01);
    assert!(!page.front[2].content.runs.is_empty());
    let pdf = docs_pdf::write(&scene).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
}

#[test]
fn objects_in_line_take_their_place_in_the_text() {
    let mut doc = open();
    let (para, index) = doc
        .paragraphs()
        .iter()
        .enumerate()
        .find_map(|(i, p)| {
            p.objects().find_map(|(at, o)| match o {
                Object::Float(f) if f.name == "Square picture" => Some((i, at)),
                _ => None,
            })
        })
        .unwrap();
    assert!(doc.set_layout_option(para, index, docs_engine::LayoutOption::Inline));
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    assert_eq!(laid.floats.iter().flatten().count(), 4);
    let o = laid.paged.objects(0);
    assert!(o.iter().any(|o| o.para == para && o.index == index));
}

#[test]
fn shapes_take_their_fill_and_outline() {
    let doc = open();
    let painter = docs_pdf::floats::Painter::new(&doc);
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let tb = &laid.floats[0][3];
    let prims = painter.prims(tb.float(&doc).unwrap(), tb.rect);
    let docs_pdf::floats::FloatPrim::Path(p) = &prims[0] else {
        panic!("{prims:?}")
    };
    assert_eq!(
        p.fill,
        Some(docs_layout::Rgba([0xFF, 0xF4, 0xD6, 255])),
        "{prims:?}"
    );
    assert_eq!(
        p.stroke.as_ref().unwrap().0,
        docs_layout::Rgba([0xC0, 0x8A, 0x00, 255])
    );
}
