//! Floating objects in `floats.docx` (scripts/corpus/gen_docx.py): read
//! with their placement, saved back byte for byte when untouched, and
//! moved, resized, rewrapped, put in line and deleted with undo.

use std::collections::BTreeMap;
use std::io::{Cursor, Read as _};
use std::path::PathBuf;

use docs_engine::float::{FloatAlign, Place, RelH, RelV, WrapSide};
use docs_engine::{Document, FloatKind, LayoutOption, Object, Wrap};

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/docx")
        .join(name)
}

fn parts(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        let name = f.name().unwrap().into_owned();
        let mut v = Vec::new();
        f.read_to_end(&mut v).unwrap();
        out.insert(name, v);
    }
    out
}

fn document_xml(bytes: &[u8]) -> String {
    String::from_utf8(parts(bytes).remove("word/document.xml").unwrap()).unwrap()
}

/// Every floating object: (paragraph, byte index, its XML as read).
fn floats(doc: &Document) -> Vec<(usize, usize, docs_engine::Float)> {
    let mut out = Vec::new();
    for (i, p) in doc.paragraphs().iter().enumerate() {
        for (at, o) in p.objects() {
            if let Object::Float(f) = o {
                out.push((i, at, (**f).clone()));
            }
        }
    }
    out
}

fn open() -> Document {
    Document::open(&corpus("floats.docx")).unwrap()
}

#[test]
fn reads_placement_kind_and_text_boxes() {
    let doc = open();
    assert!(
        !doc.has_unsupported_content(),
        "{:?}",
        doc.unsupported_content()
    );
    let f = floats(&doc);
    assert_eq!(f.len(), 5, "{f:?}");
    let by_name = |n: &str| f.iter().find(|x| x.2.name == n).unwrap().2.clone();
    let square = by_name("Square picture");
    assert!(matches!(square.kind, FloatKind::Picture { .. }));
    assert_eq!(square.at.h, (RelH::Margin, Place::Align(FloatAlign::End)));
    assert_eq!(square.at.v, (RelV::Paragraph, Place::Offset(0)));
    assert_eq!(square.at.wrap, Wrap::Square);
    assert_eq!(square.at.size_px(), (160.0, 120.0));
    let cropped = by_name("Cropped picture");
    let FloatKind::Picture { crop, rel_id, .. } = &cropped.kind else {
        panic!()
    };
    assert_eq!(*crop, [25000, 0, 25000, 0]);
    assert!(doc.image_bytes(rel_id).is_some());
    assert_eq!(cropped.at.wrap, Wrap::TopAndBottom);
    assert_eq!(cropped.at.v.0, RelV::Page);
    let tb = by_name("Text Box 1");
    assert_eq!(tb.kind, FloatKind::Shape);
    assert_eq!(tb.at.h, (RelH::Page, Place::Pct(55000)));
    assert_eq!(tb.at.side, WrapSide::Largest);
    let text = tb.text.as_ref().unwrap();
    let texts: Vec<_> = text
        .doc
        .paragraphs()
        .iter()
        .map(|p| p.text.clone())
        .collect();
    assert_eq!(texts, ["Text box", "with two paragraphs"]);
    // The text box uses the main document's styles.
    assert!(
        text.doc.styles.get("Heading1").is_some() || text.doc.styles.get("Heading 1").is_some()
    );
    let ellipse = by_name("Ellipse behind");
    assert!(ellipse.at.behind && ellipse.at.wrap == Wrap::None);
    assert_eq!(ellipse.at.layout_option(), LayoutOption::Behind);
    assert!(ellipse.geometry_xml().unwrap().contains("ellipse"));
    let arrow = by_name("Arrow");
    assert_eq!(arrow.at.h.0, RelH::Character);
    assert_eq!(arrow.at.v.0, RelV::Line);
    assert_eq!(arrow.at.layout_option(), LayoutOption::InFront);
    // Shown, not just kept: the status no longer lists them.
    assert!(
        !doc.preserved_content()
            .iter()
            .any(|p| p.contains("floating")),
        "{:?}",
        doc.preserved_content()
    );
}

#[test]
fn untouched_anchors_save_byte_for_byte() {
    let original = std::fs::read(corpus("floats.docx")).unwrap();
    let doc = open();
    let saved = doc.to_docx_bytes().unwrap();
    let before = document_xml(&original);
    let after = document_xml(&saved);
    for (_, _, f) in floats(&doc) {
        assert!(after.contains(&*f.xml), "{} changed", f.name);
        // As in the original (past its first tag, which also carries the
        // namespaces this generator declared on the run).
        let inner = &f.xml[f.xml.find('>').unwrap() + 1..];
        assert!(before.contains(inner), "{}", f.name);
    }
    // And they read back the same.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("again.docx");
    std::fs::write(&path, &saved).unwrap();
    let back = Document::open(&path).unwrap();
    let a: Vec<_> = floats(&doc).into_iter().map(|f| f.2.at).collect();
    let b: Vec<_> = floats(&back).into_iter().map(|f| f.2.at).collect();
    assert_eq!(a, b);
}

#[test]
fn move_resize_rewrap_save_and_undo() {
    let mut doc = open();
    let all = floats(&doc);
    let (para, index, square) = all
        .iter()
        .find(|f| f.2.name == "Square picture")
        .cloned()
        .unwrap();
    let others: Vec<_> = all
        .iter()
        .filter(|f| f.2.name != "Square picture")
        .collect();
    // Dragged: Word stores the new place from the column and paragraph.
    let mut at = square.at;
    at.h = (RelH::Column, Place::Offset(95250));
    at.v = (RelV::Paragraph, Place::Offset(190500));
    at.cx *= 2;
    at.cy *= 2;
    assert!(doc.set_float_placement(para, index, at));
    assert!(doc.set_layout_option(para, index, LayoutOption::Tight));
    assert_eq!(doc.float_at(para, index).unwrap().at.wrap, Wrap::Tight);
    let saved = doc.to_docx_bytes().unwrap();
    let xml = document_xml(&saved);
    // The others are as they were.
    for (_, _, f) in &others {
        assert!(xml.contains(&*f.xml), "{} changed", f.name);
    }
    assert!(!xml.contains(&*square.xml));
    assert!(xml.contains("<wp:posOffset>95250</wp:posOffset>"));
    assert!(xml.contains("<wp:posOffset>190500</wp:posOffset>"));
    assert!(xml.contains(&format!("<wp:extent cx=\"{}\" cy=\"{}\"/>", at.cx, at.cy)));
    assert!(xml.contains("<wp:wrapTight wrapText=\"bothSides\">"));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moved.docx");
    std::fs::write(&path, &saved).unwrap();
    let back = Document::open(&path).unwrap();
    let f = back.float_at(para, index).unwrap();
    assert_eq!(
        (f.at.h, f.at.v, f.at.cx, f.at.cy),
        (at.h, at.v, at.cx, at.cy)
    );
    assert_eq!(f.at.wrap, Wrap::Tight);
    // Undo twice: as read, saved as read.
    doc.undo();
    doc.undo();
    assert_eq!(doc.float_at(para, index).unwrap().at, square.at);
    let xml = document_xml(&doc.to_docx_bytes().unwrap());
    assert!(xml.contains(&*square.xml));
}

#[test]
fn in_line_and_back_and_delete() {
    let mut doc = open();
    let (para, index, pic) = floats(&doc)
        .into_iter()
        .find(|f| f.2.name == "Square picture")
        .unwrap();
    assert!(doc.can_be_inline(para, index));
    assert!(doc.set_layout_option(para, index, LayoutOption::Inline));
    let Some(Object::Image(img)) = doc.paragraph(para).object_at(index) else {
        panic!("not inline");
    };
    assert_eq!((img.cx, img.cy), (pic.at.cx, pic.at.cy));
    assert_eq!(doc.layout_option(para, index), Some(LayoutOption::Inline));
    // Floating again: in front of text, at the column and paragraph.
    assert!(doc.set_layout_option(para, index, LayoutOption::InFront));
    let f = doc.float_at(para, index).unwrap().clone();
    assert_eq!(f.at.layout_option(), LayoutOption::InFront);
    assert_eq!(f.at.h, (RelH::Column, Place::Offset(0)));
    // It saves as a valid anchor that reads back the same.
    let saved = doc.to_docx_bytes().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("inline.docx");
    std::fs::write(&path, &saved).unwrap();
    let back = Document::open(&path).unwrap();
    let g = back.float_at(para, index).unwrap();
    assert_eq!(g.at, f.at);
    assert!(matches!(g.kind, FloatKind::Picture { .. }));
    // Shapes stay floating.
    let shape = floats(&doc)
        .into_iter()
        .find(|f| f.2.name == "Ellipse behind")
        .unwrap();
    assert!(!doc.can_be_inline(shape.0, shape.1));
    assert!(!doc.set_layout_option(shape.0, shape.1, LayoutOption::Inline));
    // Deleted on its own; text deletion around it keeps it.
    let n = floats(&doc).len();
    let len = doc.paragraph(shape.0).text.len();
    let sel = docs_engine::Selection {
        anchor: docs_engine::Position::new(shape.0, 0),
        head: docs_engine::Position::new(shape.0, len),
    };
    doc.delete_selection(sel);
    assert_eq!(floats(&doc).len(), n);
    let (p, i, _) = floats(&doc)
        .into_iter()
        .find(|f| f.2.name == "Ellipse behind")
        .unwrap();
    assert!(doc.delete_float(p, i));
    assert_eq!(floats(&doc).len(), n - 1);
    doc.undo();
    assert_eq!(floats(&doc).len(), n);
}

#[test]
fn text_box_text_is_edited_and_saved() {
    let mut doc = open();
    let (para, index, tb) = floats(&doc)
        .into_iter()
        .find(|f| f.2.name == "Text Box 1")
        .unwrap();
    assert_eq!(tb.text_locked(), None);
    assert_eq!(
        tb.text_box_text().as_deref(),
        Some("Text box\nwith two paragraphs")
    );
    assert!(doc.set_text_box_text(para, index, "Edited\nin Kantor\nthree"));
    let saved = doc.to_docx_bytes().unwrap();
    let xml = document_xml(&saved);
    // The shape and its VML fallback both carry the new text.
    assert_eq!(xml.matches("in Kantor").count(), 2, "{xml}");
    assert!(!xml.contains("with two paragraphs"));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tb.docx");
    std::fs::write(&path, &saved).unwrap();
    let back = Document::open(&path).unwrap();
    let f = back.float_at(para, index).unwrap();
    assert_eq!(
        f.text_box_text().as_deref(),
        Some("Edited\nin Kantor\nthree")
    );
    // Its placement is untouched.
    assert_eq!(f.at, tb.at);
    doc.undo();
    let f = doc.float_at(para, index).unwrap();
    assert_eq!(
        f.text_box_text().as_deref(),
        Some("Text box\nwith two paragraphs")
    );
    assert!(!f.is_modified());
}

#[test]
fn groups_canvases_charts_and_wrap_polygons_are_read() {
    let path = corpus("drawings.docx");
    let doc = Document::open(&path).unwrap();
    assert!(
        doc.unsupported_content().is_empty(),
        "{:?}",
        doc.unsupported_content()
    );
    let fs = floats(&doc);
    let by = |name: &str| fs.iter().find(|f| f.2.name == name).unwrap().2.clone();
    let group = by("Group 1");
    assert_eq!(group.kind, FloatKind::Group { canvas: false });
    assert_eq!(group.picture_rels().len(), 1);
    // Its picture is loaded with the document.
    assert!(doc.image_bytes(&group.picture_rels()[0]).is_some());
    let canvas = by("Canvas 1");
    assert_eq!(canvas.kind, FloatKind::Group { canvas: true });
    let FloatKind::Chart { part, .. } = &by("Chart 1").kind else {
        panic!()
    };
    assert_eq!(part, "word/charts/chart1.xml");
    assert!(doc.part_xml(part).unwrap().contains("c:barChart"));
    let tri = by("Tight triangle");
    assert_eq!(tri.at.wrap, Wrap::Tight);
    let poly = tri.wrap_polygon().unwrap();
    assert_eq!(poly.len(), 4);
    assert_eq!(poly[0], (0.5, 0.0));
    // Switched to Square, the polygon no longer applies.
    let mut square = tri.clone();
    square.at.set_layout_option(LayoutOption::Square);
    assert!(square.wrap_polygon().is_none());
    // Saved untouched: every drawing byte for byte, the chart part kept.
    let mut doc = doc;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("d.docx");
    let end = doc.end();
    let c = doc.insert_text(docs_engine::Selection::caret(end), "!", None);
    doc.backspace(c);
    doc.save_as(&out).unwrap();
    let saved = std::fs::read(&out).unwrap();
    let xml = document_xml(&saved);
    for f in &fs {
        assert!(xml.contains(&*f.2.xml), "{} rewritten", f.2.name);
    }
    let p = parts(&saved);
    assert_eq!(
        p["word/charts/chart1.xml"],
        parts(&std::fs::read(&path).unwrap())["word/charts/chart1.xml"]
    );
}

/// `diagrams.docx`: a group whose shapes hold text.
#[test]
fn grouped_shapes_text_is_read_edited_and_saved() {
    let path = corpus("diagrams.docx");
    let mut doc = Document::open(&path).unwrap();
    let (para, index, g) = floats(&doc)
        .into_iter()
        .find(|f| f.2.name == "Labelled group")
        .unwrap();
    assert_eq!(g.kind, FloatKind::Group { canvas: false });
    assert!(g.has_text() && g.text.is_none());
    let texts: Vec<String> = (0..g.boxes.len())
        .map(|k| g.text_box_text_in(Some(k)).unwrap())
        .collect();
    assert_eq!(texts, ["Plan\nfirst", "Do"]);
    // Untouched: the drawing is written back byte for byte.
    let original = document_xml(&std::fs::read(&path).unwrap());
    let group_xml = |x: &str| {
        let (a, b) = (x.find("<wpg:wgp>").unwrap(), x.find("</wpg:wgp>").unwrap());
        x[a..b].to_owned()
    };
    assert_eq!(
        group_xml(&document_xml(&doc.to_docx_bytes().unwrap())),
        group_xml(&original)
    );
    // Editing the second shape's text changes only that one.
    assert!(doc.set_text_box_text_in(para, index, Some(1), "Do it\nnow"));
    let saved = doc.to_docx_bytes().unwrap();
    let xml = document_xml(&saved);
    assert!(xml.contains(">Do it<") && xml.contains(">now<"), "{xml}");
    assert!(xml.contains(">Plan<"));
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("g.docx");
    std::fs::write(&out, &saved).unwrap();
    let back = Document::open(&out).unwrap();
    let f = back.float_at(para, index).unwrap();
    assert_eq!(f.text_box_text_in(Some(0)).as_deref(), Some("Plan\nfirst"));
    assert_eq!(f.text_box_text_in(Some(1)).as_deref(), Some("Do it\nnow"));
    doc.undo();
    let f = doc.float_at(para, index).unwrap();
    assert_eq!(f.text_box_text_in(Some(1)).as_deref(), Some("Do"));
    assert!(!f.is_modified());
}

/// `diagrams.docx`: SmartArt is read with its data model and drawing
/// parts; parts edited with it are saved in place of the originals.
#[test]
fn smartart_parts_are_read_and_edited_parts_saved() {
    let path = corpus("diagrams.docx");
    let mut doc = Document::open(&path).unwrap();
    assert!(
        !doc.preserved_content().iter().any(|p| p == "SmartArt"),
        "{:?}",
        doc.preserved_content()
    );
    let (para, index, f) = floats(&doc)
        .into_iter()
        .find(|f| f.2.name == "Diagram 1")
        .unwrap();
    let FloatKind::SmartArt { data, drawings } = &f.kind else {
        panic!("{:?}", f.kind)
    };
    assert_eq!(data, "word/diagrams/data1.xml");
    assert_eq!(drawings.len(), 1);
    assert_eq!(drawings[0].1, "word/diagrams/drawing1.xml");
    let edited: std::sync::Arc<str> = "<dsp:drawing/>".into();
    assert!(doc.set_float_parts(
        para,
        index,
        vec![("word/diagrams/drawing1.xml".into(), edited.clone())]
    ));
    assert_eq!(
        doc.current_part_xml("word/diagrams/drawing1.xml")
            .as_deref(),
        Some("<dsp:drawing/>")
    );
    let saved = parts(&doc.to_docx_bytes().unwrap());
    assert_eq!(saved["word/diagrams/drawing1.xml"], b"<dsp:drawing/>");
    doc.undo();
    let saved = parts(&doc.to_docx_bytes().unwrap());
    let original = parts(&std::fs::read(&path).unwrap());
    assert_eq!(
        saved["word/diagrams/drawing1.xml"],
        original["word/diagrams/drawing1.xml"]
    );
}

/// A text box edited in place, as Word does: its text formatted like the
/// body's (runs, alignment, lists), one undo step, saved into the shape's
/// `w:txbxContent` and the VML fallback's.
#[test]
fn text_box_is_edited_in_place_with_formatting() {
    use docs_engine::{Align, BoxRef, ListKind, Position, Selection, Toggle};
    let mut doc = open();
    let (para, index, tb) = floats(&doc)
        .into_iter()
        .find(|f| f.2.name == "Text Box 1")
        .unwrap();
    let b = BoxRef {
        story: None,
        para,
        index,
        which: None,
    };
    let (edit, mut part) = doc.edit_text_box(b).unwrap();
    assert_eq!(part.paragraph(0).text, "Text box");
    // While it's edited the main document holds an empty box.
    assert!(!doc.part_changed(&edit, &part));
    let word = Selection {
        anchor: Position::new(0, 0),
        head: Position::new(0, 4),
    };
    assert!(part.toggle(word, Toggle::Bold));
    part.set_align(Selection::caret(Position::new(1, 0)), Align::Center);
    let end = part.end();
    let c = part.insert_text(Selection::caret(end), "\nFirst point", None);
    part.toggle_list(c, ListKind::Bullet);
    // Its own undo while it's open.
    part.undo();
    part.toggle_list(c, ListKind::Bullet);
    // Put back for layout and taken out again: nothing lost.
    doc.swap_part(&edit, &mut part);
    assert_eq!(
        doc.float_at(para, index)
            .unwrap()
            .text_box_text()
            .as_deref(),
        Some("Text box\nwith two paragraphs\nFirst point")
    );
    doc.swap_part(&edit, &mut part);
    assert!(doc.part_changed(&edit, &part));
    assert!(doc.finish_part(edit, part, Selection::default()));
    let f = doc.float_at(para, index).unwrap();
    assert!(f.is_modified() && f.at == tb.at);
    let saved = doc.to_docx_bytes().unwrap();
    let xml = document_xml(&saved);
    // Both the shape and its VML fallback, formatted.
    assert_eq!(xml.matches("First point").count(), 2, "{xml}");
    assert!(xml.matches("<w:jc w:val=\"center\"/>").count() >= 2);
    assert!(xml.contains("<w:b/>"), "{xml}");
    if let Ok(out) = std::env::var("KANTOR_TEXTBOX_OUT") {
        std::fs::write(out, &saved).unwrap();
    }
    let p = parts(&saved);
    let numbering = String::from_utf8(p["word/numbering.xml"].clone()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tb.docx");
    std::fs::write(&path, &saved).unwrap();
    let back = Document::open(&path).unwrap();
    let f = back.float_at(para, index).unwrap();
    let d = &f.text.as_ref().unwrap().doc;
    assert_eq!(d.len(), 3);
    assert!(
        d.resolve_run(d.paragraph(0), &d.paragraph(0).runs[0].props)
            .bold
    );
    assert_eq!(d.resolve_para(d.paragraph(1)).align, Align::Center);
    let num = d.paragraph(2).props.num.expect("a list item");
    assert!(
        numbering.contains(&format!("w:numId=\"{}\"", num.id)),
        "the list's definition is written: {numbering}"
    );
    // One undo step puts the box back as it was.
    doc.undo();
    let f = doc.float_at(para, index).unwrap();
    assert_eq!(
        f.text_box_text().as_deref(),
        Some("Text box\nwith two paragraphs")
    );
    assert!(!f.is_modified());
    doc.redo();
    assert_eq!(
        doc.float_at(para, index)
            .unwrap()
            .text_box_text()
            .as_deref(),
        Some("Text box\nwith two paragraphs\nFirst point")
    );

    // Unchanged: nothing recorded, nothing rewritten.
    let mut doc = open();
    let (edit, part) = doc.edit_text_box(b).unwrap();
    assert!(!doc.finish_part(edit, part, Selection::default()));
    assert!(!doc.float_at(para, index).unwrap().is_modified());
    assert!(!doc.is_dirty());
}

/// Typing in a text box with Track Changes on is tracked there.
#[test]
fn text_box_edits_are_tracked() {
    use docs_engine::{BoxRef, Position, Selection};
    let mut doc = open();
    let (para, index, _) = floats(&doc)
        .into_iter()
        .find(|f| f.2.name == "Text Box 1")
        .unwrap();
    doc.set_track_changes(true, Selection::default());
    let b = BoxRef {
        story: None,
        para,
        index,
        which: None,
    };
    let (edit, mut part) = doc.edit_text_box(b).unwrap();
    part.insert_text(Selection::caret(Position::new(0, 0)), "New ", None);
    assert!(doc.finish_part(edit, part, Selection::default()));
    let xml = document_xml(&doc.to_docx_bytes().unwrap());
    assert!(xml.contains("<w:ins "), "{xml}");
    assert_eq!(xml.matches(">New <").count(), 2, "{xml}");
}

/// A group's shape's text edited in place: only that shape's text box
/// is rewritten.
#[test]
fn group_shape_text_is_edited_in_place() {
    use docs_engine::{BoxRef, Position, Selection, Toggle};
    let mut doc = Document::open(&corpus("diagrams.docx")).unwrap();
    let (para, index, _) = floats(&doc)
        .into_iter()
        .find(|f| f.2.name == "Labelled group")
        .unwrap();
    let b = BoxRef {
        story: None,
        para,
        index,
        which: Some(1),
    };
    let (edit, mut part) = doc.edit_text_box(b).unwrap();
    let c = part.insert_text(Selection::caret(Position::new(0, 2)), " it", None);
    part.toggle(
        Selection {
            anchor: Position::new(0, 0),
            head: c.head,
        },
        Toggle::Italic,
    );
    assert!(doc.finish_part(edit, part, Selection::default()));
    let f = doc.float_at(para, index).unwrap();
    assert_eq!(f.text_box_text_in(Some(1)).as_deref(), Some("Do it"));
    assert!(f.boxes[1].edited && !f.boxes[0].edited);
    let xml = document_xml(&doc.to_docx_bytes().unwrap());
    assert!(xml.contains(">Do it<") && xml.contains("<w:i/>"), "{xml}");
    assert!(xml.contains(">Plan<"));
}
