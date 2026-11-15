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
