//! SmartArt in `diagrams.docx` (scripts/corpus/gen_docx.py): drawn from
//! Word's drawing part with its text, edited, saved to the drawing and the
//! data model, and exported.

use std::path::PathBuf;

use docs_engine::{Document, Object};
use docs_layout::Engine;

fn open() -> Document {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/docx/diagrams.docx");
    Document::open(&path).unwrap()
}

fn anchor(doc: &Document) -> (usize, usize) {
    doc.paragraphs()
        .iter()
        .enumerate()
        .find_map(|(i, p)| {
            p.objects().find_map(|(at, o)| match o {
                Object::Float(f) if f.name == "Diagram 1" => Some((i, at)),
                _ => None,
            })
        })
        .unwrap()
}

fn shown(doc: &Document) -> Vec<String> {
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(doc, &mut engine);
    let it = laid
        .floats
        .iter()
        .flatten()
        .find(|it| it.float(doc).unwrap().name == "Diagram 1")
        .unwrap();
    it.boxes
        .iter()
        .map(|b| {
            (0..b.layout.paras.len())
                .map(|k| b.layout.shown(k))
                .collect::<Vec<_>>()
                .join("|")
        })
        .collect()
}

#[test]
fn drawn_from_its_drawing_part() {
    let doc = open();
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let it = laid
        .floats
        .iter()
        .flatten()
        .find(|it| it.float(&doc).unwrap().name == "Diagram 1")
        .unwrap();
    // Five shapes filled in the theme's accent colour, not a placeholder.
    let draw = it.chart.as_ref().expect("SmartArt drawn");
    let filled = draw
        .prims
        .iter()
        .filter(|p| matches!(p, docs_pdf::floats::FloatPrim::Path(d) if d.fill.is_some()))
        .count();
    assert_eq!(filled, 5);
    assert_eq!(shown(&doc), ["Plan", "", "Ship", "Steps", "One|Two"]);
    // The first shape's text is inside the first fifth of the frame.
    let b = &it.boxes[0];
    assert!(b.x >= it.rect[0] && b.x < it.rect[0] + it.rect[2] / 5.0);
    let scene = docs_pdf::scene(&doc, &laid);
    let runs: Vec<String> = scene
        .pages
        .iter()
        .flat_map(|p| &p.front)
        .flat_map(|f| &f.content.runs)
        .map(|r| r.text.to_string())
        .collect();
    assert!(runs.iter().any(|t| t == "Ship"), "{runs:?}");
    assert!(docs_pdf::write(&scene).unwrap().starts_with(b"%PDF"));
}

#[test]
fn text_edited_and_saved_to_drawing_and_data() {
    let mut doc = open();
    let (para, index) = anchor(&doc);
    let f = doc.float_at(para, index).unwrap().clone();
    // A shape showing two items keeps two lines.
    assert_eq!(
        docs_pdf::smartart::shape_text(&doc, &f, 4).unwrap(),
        ("One\nTwo".to_owned(), None)
    );
    assert!(docs_pdf::smartart::set_shape_text(&doc, &f, 4, "Only one").is_err());
    let parts = docs_pdf::smartart::set_shape_text(&doc, &f, 0, "Design")
        .unwrap()
        .unwrap();
    assert_eq!(parts.len(), 2);
    assert!(doc.set_float_parts(para, index, parts));
    // The empty placeholder item gets text too.
    let f = doc.float_at(para, index).unwrap().clone();
    let parts = docs_pdf::smartart::set_shape_text(&doc, &f, 1, "Build")
        .unwrap()
        .unwrap();
    assert!(doc.set_float_parts(para, index, parts));
    let f = doc.float_at(para, index).unwrap().clone();
    let parts = docs_pdf::smartart::set_shape_text(&doc, &f, 4, "First\nSecond")
        .unwrap()
        .unwrap();
    assert!(doc.set_float_parts(para, index, parts));
    assert_eq!(
        shown(&doc),
        ["Design", "Build", "Ship", "Steps", "First|Second"]
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sa.docx");
    std::fs::write(&path, doc.to_docx_bytes().unwrap()).unwrap();
    let back = Document::open(&path).unwrap();
    assert_eq!(
        shown(&back),
        ["Design", "Build", "Ship", "Steps", "First|Second"]
    );
    let data = back.part_xml("word/diagrams/data1.xml").unwrap();
    for want in [">Design<", ">Build<", ">First<", ">Second<"] {
        assert!(data.contains(want), "{want} in {data}");
    }
    assert!(!data.contains(">Plan<") && !data.contains("phldr=\"1\""));
    // Undo goes back one edit at a time.
    doc.undo();
    assert_eq!(shown(&doc), ["Design", "Build", "Ship", "Steps", "One|Two"]);
}
