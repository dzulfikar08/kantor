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

/// `word/document.xml` of a saved file.
fn document_xml(path: &std::path::Path) -> String {
    let bytes = std::fs::read(path).unwrap();
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut s = String::new();
    std::io::Read::read_to_string(&mut z.by_name("word/document.xml").unwrap(), &mut s).unwrap();
    s
}

#[test]
fn inline_smartart_made_floating_and_back_as_word_writes_it() {
    use docs_engine::LayoutOption;
    use docs_engine::float::{Place, RelH, RelV};
    let mut doc = open();
    let (pi, index) = find(&doc, "Diagram 2");
    let f = doc.float_at(pi, index).unwrap().clone();
    assert!(doc.can_be_inline(pi, index));
    // Square, where it stood in its line (from the column and paragraph).
    assert!(doc.set_layout_option_from(pi, index, LayoutOption::Square, (914400, 12700)));
    let now = doc.float_at(pi, index).unwrap();
    assert!(!now.inline);
    assert!(matches!(now.kind, FloatKind::SmartArt { .. }));
    assert_eq!(now.parts, f.parts);
    assert_eq!(doc.layout_option(pi, index), Some(LayoutOption::Square));
    assert_eq!(now.at.h, (RelH::Column, Place::Offset(914400)));
    assert_eq!(now.at.v, (RelV::Paragraph, Place::Offset(12700)));
    assert_eq!((now.at.cx, now.at.cy), (f.at.cx, f.at.cy));
    assert_eq!(now.at.dist, [0, 0, 114300, 114300]);
    assert!(now.at.allow_overlap && now.at.in_cell && !now.at.behind);
    let xml = now.write_xml();
    for want in [
        "<wp:anchor distT=\"0\" distB=\"0\" distL=\"114300\" distR=\"114300\" simplePos=\"0\"",
        "locked=\"0\" layoutInCell=\"1\" allowOverlap=\"1\"",
        "<wp:simplePos x=\"0\" y=\"0\"/>",
        "<wp:positionH relativeFrom=\"column\"><wp:posOffset>914400</wp:posOffset></wp:positionH>",
        "<wp:positionV relativeFrom=\"paragraph\"><wp:posOffset>12700</wp:posOffset></wp:positionV>",
        "<wp:wrapSquare wrapText=\"bothSides\"/><wp:docPr ",
        "</a:graphic></wp:anchor></w:drawing>",
    ] {
        assert!(xml.contains(want), "{want}\n{xml}");
    }
    assert!(!xml.contains("inline"), "{xml}");
    // Laid out as a floating object now, not a box in the line.
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    assert!(
        laid.paged.pages[0]
            .floats
            .iter()
            .any(|pf| (pf.para, pf.index) == (pi, index))
    );
    assert!(
        !laid
            .paged
            .objects(0)
            .iter()
            .any(|o| (o.para, o.index) == (pi, index))
    );
    // Each wrap Word offers.
    for (o, want) in [
        (LayoutOption::Tight, "<wp:wrapTight wrapText=\"bothSides\">"),
        (LayoutOption::Through, "<wp:wrapThrough wrapText=\"bothSides\">"),
        (LayoutOption::TopAndBottom, "<wp:wrapTopAndBottom/>"),
        (LayoutOption::Behind, "<wp:wrapNone/>"),
        (LayoutOption::InFront, "<wp:wrapNone/>"),
    ] {
        assert!(doc.set_layout_option(pi, index, o));
        let xml = doc.float_at(pi, index).unwrap().write_xml().into_owned();
        assert!(xml.contains(want), "{o:?}: {xml}");
        assert_eq!(
            xml.contains("behindDoc=\"1\""),
            o == LayoutOption::Behind,
            "{o:?}"
        );
        assert_eq!(doc.layout_option(pi, index), Some(o));
    }
    // Saved and read back: floating SmartArt, drawn.
    let dir = std::env::temp_dir().join(format!("kantor-sa-float-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("floating.docx");
    doc.save_as(&out).unwrap();
    let back = Document::open(&out).unwrap();
    let (bp, bi) = find(&back, "Diagram 2");
    let b = back.float_at(bp, bi).unwrap();
    assert!(!b.inline);
    assert!(matches!(b.kind, FloatKind::SmartArt { ref drawings, .. } if !drawings.is_empty()));
    assert_eq!(back.layout_option(bp, bi), Some(LayoutOption::InFront));
    let laid = docs_pdf::lay_out(&back, &mut engine);
    let it = laid
        .floats
        .iter()
        .flatten()
        .find(|it| (it.para, it.index) == (bp, bi))
        .expect("drawn");
    assert!(it.chart.is_some());
    // Back in line: Word's inline frame, no position or wrap.
    assert!(doc.set_layout_option(pi, index, LayoutOption::Inline));
    let now = doc.float_at(pi, index).unwrap();
    assert!(now.inline);
    assert_eq!(doc.layout_option(pi, index), Some(LayoutOption::Inline));
    let xml = now.write_xml();
    assert!(
        xml.starts_with("<w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\""),
        "{xml}"
    );
    assert!(xml.contains(&format!("<wp:extent cx=\"{}\" cy=\"{}\"/>", f.at.cx, f.at.cy)));
    for gone in ["anchor", "wrap", "position", "simplePos", "relativeHeight"] {
        assert!(!xml.contains(gone), "{gone}: {xml}");
    }
    let out2 = dir.join("inline.docx");
    doc.save_as(&out2).unwrap();
    let back = Document::open(&out2).unwrap();
    let (bp, bi) = find(&back, "Diagram 2");
    assert!(back.float_at(bp, bi).unwrap().inline);
    assert!(document_xml(&out2).contains("<wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\""));
    // Undo walks back through every step to the SmartArt as read.
    while doc.can_undo() {
        doc.undo();
    }
    let undone = doc.float_at(pi, index).unwrap();
    assert!(undone.inline);
    assert_eq!(undone.write_xml(), f.write_xml());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn floating_smartart_resized_then_put_in_line_keeps_its_size() {
    use docs_engine::LayoutOption;
    let mut doc = open();
    let (pi, index) = find(&doc, "Diagram 1");
    assert!(doc.set_layout_option(pi, index, LayoutOption::InFront));
    let mut at = doc.float_at(pi, index).unwrap().at;
    at.cx = 2 * 914400;
    at.cy = 914400;
    at.h.1 = docs_engine::float::Place::Offset(457200);
    assert!(doc.set_float_placement(pi, index, at));
    assert!(doc.set_layout_option(pi, index, LayoutOption::Inline));
    let f = doc.float_at(pi, index).unwrap();
    assert!(f.inline);
    assert_eq!((f.at.cx, f.at.cy), (2 * 914400, 914400));
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let o = laid
        .paged
        .objects(0)
        .into_iter()
        .find(|o| (o.para, o.index) == (pi, index))
        .expect("in its line again");
    assert!((o.whole[2] - 192.0).abs() < 0.5, "{:?}", o.whole);
}

#[test]
fn inline_smartart_dragged_moves_in_the_text() {
    let mut doc = open();
    let (pi, index) = find(&doc, "Diagram 2");
    let text = doc.paragraph(pi).text.clone();
    let len = docs_engine::OBJECT.len_utf8();
    // Later in its own paragraph: after "sits".
    let to = text.find(" in this line").unwrap();
    let sel = doc
        .move_object(pi, index, docs_engine::Position::new(pi, to))
        .unwrap();
    assert_eq!(sel.anchor, docs_engine::Position::new(pi, to - len));
    assert_eq!(find(&doc, "Diagram 2"), (pi, to - len));
    let now = doc.paragraph(pi).text.clone();
    assert!(now.contains(&format!("sits{} in this line", docs_engine::OBJECT)), "{now}");
    assert!(now.contains("Accent List  sits") || now.contains("Accent List sits"));
    // Onto itself: nothing to do.
    assert!(
        doc.move_object(pi, to - len, docs_engine::Position::new(pi, to - len))
            .is_none()
    );
    // To another paragraph (the first one's start), then undone in one step.
    let sel = doc
        .move_object(pi, to - len, docs_engine::Position::new(0, 0))
        .unwrap();
    assert_eq!(sel.anchor, docs_engine::Position::new(0, 0));
    assert_eq!(find(&doc, "Diagram 2"), (0, 0));
    assert!(!doc.paragraph(pi).text.contains(docs_engine::OBJECT));
    doc.undo();
    assert_eq!(find(&doc, "Diagram 2"), (pi, to - len));
    doc.undo();
    assert_eq!(doc.paragraph(pi).text, text);
    // Saved after a move: the drawing as read, at its new place.
    doc.redo();
    let dir = std::env::temp_dir().join(format!("kantor-sa-move-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("moved.docx");
    doc.save_as(&out).unwrap();
    let xml = document_xml(&out);
    let i = xml.find("sits").unwrap();
    assert!(xml[i..].trim_start_matches("sits").contains("<wp:inline"));
    let back = Document::open(&out).unwrap();
    let (bp, bi) = find(&back, "Diagram 2");
    assert_eq!((bp, bi), (pi, to - len));
    assert!(back.float_at(bp, bi).unwrap().inline);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn moving_with_track_changes_leaves_a_deletion_and_an_insertion() {
    let mut doc = open();
    doc.set_track_changes(true, Selection::default());
    let (pi, index) = find(&doc, "Diagram 2");
    let sel = doc
        .move_object(pi, index, docs_engine::Position::new(pi, 0))
        .unwrap();
    assert_eq!(sel.anchor.offset, 0);
    let p = doc.paragraph(pi);
    let revs: Vec<_> = p
        .runs
        .iter()
        .filter(|r| r.object.as_ref().is_some_and(|o| matches!(o, Object::Float(_))))
        .map(|r| r.rev.as_ref().map(|r| (r.kind, r.tag.clone())))
        .collect();
    assert_eq!(revs.len(), 2, "{revs:?}");
    let (ins, del) = (revs[0].clone().unwrap(), revs[1].clone().unwrap());
    assert_eq!(ins.0, docs_engine::RevKind::Insert);
    assert_eq!(del.0, docs_engine::RevKind::Delete);
    assert_ne!(ins.1, del.1);
    // The inserted copy moves again outright (it's the user's own).
    let sel = doc
        .move_object(pi, 0, docs_engine::Position::new(0, 0))
        .unwrap();
    assert_eq!(sel.anchor, docs_engine::Position::new(0, 0));
    let floats_in = |p: usize| {
        doc.paragraph(p)
            .objects()
            .filter(|(_, o)| matches!(o, Object::Float(_)))
            .count()
    };
    assert_eq!(floats_in(pi), 1);
}
