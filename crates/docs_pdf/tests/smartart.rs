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

/// A part of the Slides corpus's SmartArt deck (Kantor's layouts:
/// data3/drawing3 are a Basic Process).
fn pptx_part(name: &str) -> String {
    use std::io::Read as _;
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/pptx/smartart.pptx");
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut s = String::new();
    zip.by_name(name).unwrap().read_to_string(&mut s).unwrap();
    s
}

#[test]
fn items_added_and_removed_and_laid_out_again() {
    let mut doc = open();
    let (para, index) = anchor(&doc);
    let f = doc.float_at(para, index).unwrap().clone();
    // The fixture's own layout isn't one Kantor lays out: refused.
    assert_eq!(
        docs_pdf::smartart::add_item(&doc, &f, 0, true).unwrap_err(),
        slides_engine::smartart_layout::LAYOUT_REFUSAL
    );
    // Give it a Basic Process (data model and drawing).
    let docs_engine::FloatKind::SmartArt { data, drawings, .. } = &f.kind else {
        panic!()
    };
    assert_eq!(drawings.len(), 1);
    let process = vec![
        (data.clone(), pptx_part("ppt/diagrams/data3.xml").into()),
        (
            drawings[0].1.clone(),
            pptx_part("ppt/diagrams/drawing3.xml").into(),
        ),
    ];
    assert!(doc.set_float_parts(para, index, process));
    let f = doc.float_at(para, index).unwrap().clone();
    assert!(docs_pdf::smartart::can_relayout(&doc, &f));
    let before = shown(&doc);
    let first = before.iter().position(|t| !t.is_empty()).unwrap();
    let parts = docs_pdf::smartart::add_item(&doc, &f, first, true).unwrap();
    assert!(doc.set_float_parts(para, index, parts));
    // One more box (and arrow), the new one empty.
    let added = shown(&doc);
    assert!(added.len() > before.len(), "{before:?} → {added:?}");
    assert!(
        added.iter().filter(|t| t.is_empty()).count()
            > before.iter().filter(|t| t.is_empty()).count()
    );
    let f = doc.float_at(para, index).unwrap().clone();
    let parts = docs_pdf::smartart::remove_item(&doc, &f, first).unwrap();
    assert!(doc.set_float_parts(para, index, parts));
    assert!(!shown(&doc).contains(&before[first]));
    // Resized: laid out again for the new size (one undo step), its
    // boxes inside the frame.
    let before_resize = shown(&doc);
    let f = doc.float_at(para, index).unwrap().clone();
    let mut at = f.at;
    at.cx /= 2;
    let parts = docs_pdf::smartart::relaid(&doc, &f, (at.cx, at.cy)).unwrap();
    assert!(doc.set_float_placement_parts(para, index, at, parts));
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let it = laid
        .floats
        .iter()
        .flatten()
        .find(|it| it.float(&doc).unwrap().name == "Diagram 1")
        .unwrap();
    for b in &it.boxes {
        assert!(
            b.rect[0] + b.rect[2] <= it.rect[0] + it.rect[2] + 1.0,
            "{:?} in {:?}",
            b.rect,
            it.rect
        );
    }
    // Saved and read back.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("items.docx");
    std::fs::write(&path, doc.to_docx_bytes().unwrap()).unwrap();
    let back = Document::open(&path).unwrap();
    assert_eq!(shown(&back), shown(&doc));
    if let Ok(out) = std::env::var("KANTOR_SA_OUT") {
        std::fs::copy(&path, PathBuf::from(out).join("smartart-items.docx")).unwrap();
    }
    // The resize and the new layout are one step.
    doc.undo();
    let f = doc.float_at(para, index).unwrap();
    assert_eq!(f.at.cx, at.cx * 2);
    assert_eq!(shown(&doc), before_resize);
}

/// SmartArt laid out again smaller: its text shrinks to fit its shapes
/// (Word's autofit), and grows back when it's made bigger again.
#[test]
fn text_shrinks_to_fit_when_laid_out_again() {
    let mut doc = open();
    let (para, index) = anchor(&doc);
    let f = doc.float_at(para, index).unwrap().clone();
    let docs_engine::FloatKind::SmartArt { data, drawings, .. } = &f.kind else {
        panic!()
    };
    let drawing = drawings[0].1.clone();
    let process = vec![
        (data.clone(), pptx_part("ppt/diagrams/data3.xml").into()),
        (
            drawing.clone(),
            pptx_part("ppt/diagrams/drawing3.xml").into(),
        ),
    ];
    assert!(doc.set_float_parts(para, index, process));
    let f = doc.float_at(para, index).unwrap().clone();
    let full = f.at;
    let mut small = full;
    small.cx /= 5;
    small.cy /= 5;
    let parts = docs_pdf::smartart::relaid(&doc, &f, (small.cx, small.cy)).unwrap();
    let xml = parts.iter().find(|p| p.0 == drawing).unwrap().1.clone();
    assert!(xml.contains("fontScale=\""), "{xml}");
    assert!(doc.set_float_placement_parts(para, index, small, parts));
    if let Ok(out) = std::env::var("KANTOR_SA_FIT_OUT") {
        std::fs::write(out, doc.to_docx_bytes().unwrap()).unwrap();
    }
    // Every text fits its box.
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let it = laid
        .floats
        .iter()
        .flatten()
        .find(|it| it.float(&doc).unwrap().name == "Diagram 1")
        .unwrap();
    for b in it.boxes.iter().filter(|b| !b.layout.shown(0).is_empty()) {
        assert!(
            b.layout.height <= b.rect[3] + 0.5,
            "{} in {:?}",
            b.layout.height,
            b.rect
        );
    }
    // Made big again: the text is as written.
    let f = doc.float_at(para, index).unwrap().clone();
    let parts = docs_pdf::smartart::relaid(&doc, &f, (full.cx, full.cy)).unwrap();
    let xml = parts.iter().find(|p| p.0 == drawing).unwrap().1.clone();
    assert!(!xml.contains("fontScale=\""), "{xml}");
}

/// A Labeled Hierarchy in Word: laid out by Kantor, labels added.
#[test]
fn labeled_hierarchy_takes_labels() {
    let mut doc = open();
    let (para, index) = anchor(&doc);
    let f = doc.float_at(para, index).unwrap().clone();
    let docs_engine::FloatKind::SmartArt { data, drawings, .. } = &f.kind else {
        panic!()
    };
    let labeled = vec![
        (data.clone(), pptx_part("ppt/diagrams/data9.xml").into()),
        (
            drawings[0].1.clone(),
            pptx_part("ppt/diagrams/drawing9.xml").into(),
        ),
    ];
    assert!(doc.set_float_parts(para, index, labeled));
    let f = doc.float_at(para, index).unwrap().clone();
    assert!(docs_pdf::smartart::can_relayout(&doc, &f));
    let before = shown(&doc);
    let staff = before.iter().position(|t| t == "Staff").unwrap();
    let parts = docs_pdf::smartart::add_item(&doc, &f, staff, true).unwrap();
    assert!(doc.set_float_parts(para, index, parts));
    assert_eq!(shown(&doc).len(), before.len() + 1);
    let f = doc.float_at(para, index).unwrap().clone();
    let ceo = shown(&doc).iter().position(|t| t == "CEO").unwrap();
    assert_eq!(
        docs_pdf::smartart::add_item(&doc, &f, ceo, true).unwrap_err(),
        slides_engine::smartart_layout::TOP_ALONE
    );
}

/// Word's Change Layout: the fixture's SmartArt (a layout Kantor doesn't
/// lay out) as a Basic Pyramid, then a Basic Matrix: same items and text,
/// laid out by Kantor (items can be added), the layout part rewritten;
/// one undo step each.
#[test]
fn layout_changed_keeping_items() {
    let mut doc = open();
    let (para, index) = anchor(&doc);
    let f = doc.float_at(para, index).unwrap().clone();
    let docs_engine::FloatKind::SmartArt {
        layout: Some(layout),
        ..
    } = &f.kind
    else {
        panic!("a layout part")
    };
    let layout = layout.clone();
    let before = shown(&doc);
    let texts = |v: &[String]| {
        let mut t: Vec<String> = v
            .iter()
            .flat_map(|s| s.split('|').map(str::to_owned))
            .filter(|s| !s.is_empty())
            .collect();
        t.sort();
        t
    };
    let parts = docs_pdf::smartart::change_layout(&doc, &f, slides_engine::SmartArtFamily::Pyramid)
        .unwrap();
    assert!(doc.set_float_parts(para, index, parts));
    let pyramid = shown(&doc);
    assert_eq!(texts(&pyramid), texts(&before), "{before:?} → {pyramid:?}");
    let f = doc.float_at(para, index).unwrap().clone();
    assert!(docs_pdf::smartart::can_relayout(&doc, &f));
    let parts =
        docs_pdf::smartart::change_layout(&doc, &f, slides_engine::SmartArtFamily::Matrix).unwrap();
    assert!(doc.set_float_parts(para, index, parts));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("layout.docx");
    std::fs::write(&path, doc.to_docx_bytes().unwrap()).unwrap();
    if let Ok(out) = std::env::var("KANTOR_SA_OUT") {
        std::fs::copy(&path, PathBuf::from(out).join("smartart-layout.docx")).unwrap();
    }
    let back = Document::open(&path).unwrap();
    assert_eq!(shown(&back), shown(&doc));
    assert!(
        back.current_part_xml(&layout)
            .unwrap()
            .contains("layout/matrix3")
    );
    doc.undo();
    assert_eq!(shown(&doc), pyramid);
    doc.undo();
    assert_eq!(shown(&doc), before);
}

/// Word's Organization Chart *Layout* and *Add Assistant*: the Slides
/// corpus's org chart (slide 11: an assistant, Left Hanging and Both
/// branches) in the fixture's SmartArt object.
#[test]
fn org_chart_layouts_and_assistants() {
    use slides_engine::OrgLayout;
    let mut doc = open();
    let (para, index) = anchor(&doc);
    let f = doc.float_at(para, index).unwrap().clone();
    let docs_engine::FloatKind::SmartArt { data, drawings, .. } = &f.kind else {
        panic!()
    };
    let org = vec![
        (data.clone(), pptx_part("ppt/diagrams/data11.xml").into()),
        (
            drawings[0].1.clone(),
            pptx_part("ppt/diagrams/drawing11.xml").into(),
        ),
    ];
    assert!(doc.set_float_parts(para, index, org));
    let f = doc.float_at(para, index).unwrap().clone();
    // The drawing shape showing text `t`.
    let at = |doc: &Document, t: &str| {
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
            .find(|b| b.layout.paras.len() == 1 && b.layout.shown(0) == t)
            .unwrap_or_else(|| panic!("{t}: {:?}", shown(doc)))
            .which
    };
    let sales = at(&doc, "Sales");
    assert_eq!(
        docs_pdf::smartart::org_item(&doc, &f, sales),
        Some(Some(OrgLayout::LeftHanging))
    );
    let parts =
        docs_pdf::smartart::set_org_layout(&doc, &f, sales, OrgLayout::RightHanging).unwrap();
    assert!(doc.set_float_parts(para, index, parts));
    let f = doc.float_at(para, index).unwrap().clone();
    assert_eq!(
        docs_pdf::smartart::org_item(&doc, &f, at(&doc, "Sales")),
        Some(Some(OrgLayout::RightHanging))
    );
    let n = shown(&doc).len();
    let parts = docs_pdf::smartart::add_assistant(&doc, &f, at(&doc, "Ops")).unwrap();
    assert!(doc.set_float_parts(para, index, parts));
    // A new box (its connector has no text).
    assert_eq!(shown(&doc).len(), n + 1);
    // Not an Organization Chart: no Layout, no assistants.
    let labeled = vec![
        (data.clone(), pptx_part("ppt/diagrams/data9.xml").into()),
        (
            drawings[0].1.clone(),
            pptx_part("ppt/diagrams/drawing9.xml").into(),
        ),
    ];
    assert!(doc.set_float_parts(para, index, labeled));
    let f = doc.float_at(para, index).unwrap().clone();
    let ceo = at(&doc, "CEO");
    assert_eq!(docs_pdf::smartart::org_item(&doc, &f, ceo), None);
    assert_eq!(
        docs_pdf::smartart::add_assistant(&doc, &f, ceo).unwrap_err(),
        slides_engine::smartart_layout::ASSISTANT_ORG
    );
}
