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

/// `drawings.docx`: groups, a canvas, a chart, turned objects, a shadow
/// and Tight wrapping.
#[test]
fn groups_charts_turns_shadows_and_tight_wrap() {
    use docs_pdf::floats::{FloatPrim, Painter};
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/docx/drawings.docx");
    let doc = Document::open(&path).unwrap();
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let painter = Painter::new(&doc);
    let items: Vec<&docs_pdf::FloatItem> = laid.floats.iter().flatten().collect();
    let by = |name: &str| {
        *items
            .iter()
            .find(|it| it.float(&doc).unwrap().name == name)
            .unwrap()
    };
    let prims = |name: &str| {
        let it = by(name);
        painter.prims(it.float(&doc).unwrap(), it.rect)
    };
    // The group: its rectangle (with a shadow), ellipse and picture, the
    // picture in the group's lower right quarter (child space is twice
    // the group's size).
    let g = prims("Group 1");
    assert!(!g.iter().any(|p| matches!(p, FloatPrim::Placeholder(..))));
    let rect = by("Group 1").rect;
    let pic = g
        .iter()
        .find_map(|p| match p {
            FloatPrim::Picture { clip, .. } => Some(*clip),
            _ => None,
        })
        .unwrap();
    assert!(
        (pic[0] - (rect[0] + rect[2] / 2.0)).abs() < 0.01,
        "{pic:?} in {rect:?}"
    );
    assert!((pic[2] - rect[2] / 2.0).abs() < 0.01);
    // The shadow: translucent black fills drawn before the shape.
    let shadow = g.iter().position(
        |p| matches!(p, FloatPrim::Path(d) if d.fill.is_some_and(|c| c.0[3] < 255 && c.0[0] == 0)),
    );
    let shape = g.iter().position(|p| {
        matches!(p, FloatPrim::Path(d) if d.fill.is_some_and(|c| c.0 == [0xBD, 0xD7, 0xEE, 255]))
    });
    assert!(shadow.unwrap() < shape.unwrap());
    // The canvas: background, shape and picture.
    let c = prims("Canvas 1");
    assert!(c.iter().any(|p| matches!(p, FloatPrim::Picture { .. })));
    assert!(c.len() >= 3);
    // The chart: drawn with its labels.
    let chart = by("Chart 1").chart.clone().expect("chart drawn");
    assert!(chart.prims.len() > 6);
    let shown: Vec<String> = chart.labels.iter().map(|l| l.layout.shown(0)).collect();
    for want in ["Sales", "Q1", "North", "Units"] {
        assert!(shown.iter().any(|s| s == want), "{want} in {shown:?}");
    }
    // The value axis title is turned.
    assert!(
        chart
            .labels
            .iter()
            .any(|l| l.layout.shown(0) == "Units" && l.rot.abs() == 90.0)
    );
    // The turned picture and text box.
    let tp = prims("Turned picture");
    assert!(
        tp.iter()
            .any(|p| matches!(p, FloatPrim::Picture { rot, .. } if (*rot - 30.0).abs() < 0.01))
    );
    let tb = by("Turned text box").float(&doc).unwrap();
    assert_eq!(docs_pdf::floats::text_turn(tb), 90.0);
    // Tight wrap: some line beside the triangle reaches past its box's
    // left edge (under the slanted side). Any line level with the triangle
    // counts: how many lines fit in a given height depends on the font
    // (CI's fallback fonts are taller), and Square wrap never crosses the
    // box's edge, so this still tells Tight from Square.
    let tri = by("Tight triangle");
    let page = laid
        .floats
        .iter()
        .position(|p| p.iter().any(|it| std::ptr::eq(it, tri)))
        .unwrap();
    let r = tri.rect;
    let past = laid.paged.pages[page].lines.iter().any(|l| {
        l.top + l.height > r[1] && l.top < r[1] + r[3] && l.x + l.width > r[0] + 10.0 && l.x < r[0]
    });
    assert!(past, "no line under the triangle's slope");
    // The PDF: chart labels and the turned text box's text are text.
    let scene = docs_pdf::scene(&doc, &laid);
    let pdf = docs_pdf::write(&scene).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
    let turned = scene
        .pages
        .iter()
        .flat_map(|p| p.front.iter().chain(&p.behind))
        .filter(|f| f.content_turn.is_some())
        .count();
    // The text box and the axis title.
    assert!(turned >= 2, "{turned}");
    let pics = scene
        .pages
        .iter()
        .flat_map(|p| p.front.iter().chain(&p.behind))
        .flat_map(|f| &f.pictures)
        .filter(|p| (p.turn - 30.0).abs() < 0.01)
        .count();
    assert_eq!(pics, 1);
}

/// `diagrams.docx`: the text of a group's shapes is laid out inside each
/// shape (a turned shape's text turned with it) and goes into the PDF.
#[test]
fn grouped_shapes_text_is_laid_out_and_exported() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/docx/diagrams.docx");
    let doc = Document::open(&path).unwrap();
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let it = laid
        .floats
        .iter()
        .flatten()
        .find(|it| it.float(&doc).unwrap().name == "Labelled group")
        .unwrap();
    assert_eq!(it.boxes.len(), 2);
    let r = it.rect;
    for b in &it.boxes {
        // Each shape's box lies in the group's box; its text starts inside.
        assert!(b.rect[0] >= r[0] - 0.5 && b.rect[0] + b.rect[2] <= r[0] + r[2] + 0.5);
        assert!(b.x >= b.rect[0] && b.y >= b.rect[1] - 0.5, "{:?}", b.rect);
    }
    let first = &it.boxes[0];
    assert_eq!(first.layout.shown(0), "Plan");
    assert_eq!(first.layout.shown(1), "first");
    assert_eq!(first.turn, 0.0);
    // The first box is the left fifth of the group (child space is twice
    // the group's size).
    assert!((first.rect[2] - r[2] / 5.0).abs() < 0.5, "{:?}", first.rect);
    let second = &it.boxes[1];
    assert_eq!(second.layout.shown(0), "Do");
    assert_eq!(second.turn, 90.0);
    let scene = docs_pdf::scene(&doc, &laid);
    let scenes: Vec<_> = scene.pages.iter().flat_map(|p| &p.front).collect();
    let with_text = scenes
        .iter()
        .filter(|f| {
            f.content
                .runs
                .iter()
                .any(|r| matches!(&*r.text, "first" | "Do"))
        })
        .count();
    assert_eq!(with_text, 2);
    assert!(scenes.iter().any(|f| f.content_turn.is_some_and(|t| t.0 == 90.0)));
    assert!(docs_pdf::write(&scene).unwrap().starts_with(b"%PDF"));
}

/// `diagrams.docx`: radar, bubble, stock and 3-D column charts are drawn
/// with Slides' chart code (not boxes), their titles as text.
#[test]
fn more_chart_types_are_drawn() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/docx/diagrams.docx");
    let doc = Document::open(&path).unwrap();
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    for (name, title) in [
        ("Radar chart", "Radar"),
        ("Bubble chart", "Bubbles"),
        ("Stock chart", "Prices"),
        ("3-D column chart", "Output"),
    ] {
        let it = laid
            .floats
            .iter()
            .flatten()
            .find(|it| it.float(&doc).unwrap().name == name)
            .unwrap_or_else(|| panic!("{name}"));
        let chart = it.chart.as_ref().unwrap_or_else(|| panic!("{name} drawn"));
        assert!(chart.prims.len() > 4, "{name}: {}", chart.prims.len());
        let shown: Vec<String> = chart.labels.iter().map(|l| l.layout.shown(0)).collect();
        assert!(shown.iter().any(|s| s == title), "{name}: {shown:?}");
    }
}
