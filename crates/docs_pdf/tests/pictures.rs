//! `pictures.docx` (scripts/corpus/gen_docx_more.py): inline pictures cut
//! to their shapes with their outlines, and SmartArt picture fills taken
//! from the drawing part's own relationships, in the scene the screen and
//! the PDF draw from; saved unchanged.

use std::io::Read as _;
use std::path::PathBuf;

use docs_engine::{Document, Selection};
use docs_layout::Engine;
use docs_pdf::Picture;

fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/docx/pictures.docx")
}

fn open() -> Document {
    Document::open(&path()).unwrap()
}

#[test]
fn inline_pictures_are_cut_to_their_shapes_and_outlined() {
    let doc = open();
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let scene = docs_pdf::scene(&doc, &laid);
    let page = &scene.pages[0];
    // The header's oval first, then the body's six inline pictures.
    let pics: Vec<&Picture> = page.pictures.iter().collect();
    assert_eq!(pics.len(), 7, "{}", pics.len());
    let cut = pics.iter().filter(|p| p.shape.is_some()).count();
    // Header oval, oval, rounded, hexagon, star, turned oval.
    assert_eq!(cut, 6);
    // The bordered rectangle isn't cut, but has its border.
    assert_eq!(pics.iter().filter(|p| p.shape.is_none()).count(), 1);
    let turned: Vec<_> = pics.iter().filter(|p| p.turn != 0.0).collect();
    assert_eq!(turned.len(), 1);
    assert!((turned[0].turn - 30.0).abs() < 0.01);
    assert_eq!(turned[0].clip, Some(turned[0].rect));
    // Outlines: header oval, oval, rounded, hexagon (dashed), border,
    // turned oval; the star has none.
    assert_eq!(page.outlines.len(), 6);
    assert_eq!(
        page.outlines
            .iter()
            .filter(|d| d.stroke.as_ref().is_some_and(|s| s.2.is_some()))
            .count(),
        1
    );
    let red = page
        .outlines
        .iter()
        .find(|d| d.stroke.as_ref().unwrap().0.0 == [0xC0, 0, 0, 255])
        .expect("the oval's red outline");
    // 3 pt = 4 px, round the oval inside its picture's box.
    assert!((red.stroke.as_ref().unwrap().1 - 4.0).abs() < 0.01);
    let oval = pics
        .iter()
        .filter(|p| p.shape.is_some() && p.turn == 0.0)
        .nth(1)
        .unwrap();
    let [x, y, w, h] = oval.rect;
    for s in &red.segs {
        if let docs_pdf::floats::Seg::Move(p) | docs_pdf::floats::Seg::Line(p) = s {
            assert!(
                p[0] >= x - 0.01 && p[0] <= x + w + 0.01,
                "{p:?} {:?}",
                oval.rect
            );
            assert!(p[1] >= y - 0.01 && p[1] <= y + h + 0.01);
        }
    }
}

#[test]
fn smartart_picture_fills_come_from_the_drawing_part() {
    let doc = open();
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    for n in 1..=3 {
        let key = docs_engine::float::diagram_media_key(
            &format!("word/diagrams/drawing{n}.xml"),
            "rIdImg1",
        );
        assert!(doc.image_bytes(&key).is_some(), "{key}");
        let name = format!("Diagram {n}");
        let it = laid
            .floats
            .iter()
            .flatten()
            .find(|it| it.float(&doc).unwrap().name == name)
            .unwrap();
        let draw = it.chart.as_ref().expect("SmartArt drawn");
        let pics: Vec<_> = draw
            .prims
            .iter()
            .filter_map(|p| match p {
                docs_pdf::floats::FloatPrim::Picture { rel_id, shape, .. } => Some((rel_id, shape)),
                _ => None,
            })
            .collect();
        assert_eq!(pics.len(), 1, "{name}");
        assert_eq!(pics[0].0, &key);
        assert!(pics[0].1.is_some(), "{name}: cut to its shape");
    }
    // In the PDF's scene: three cut pictures among the floats (and the
    // floating oval).
    let scene = docs_pdf::scene(&doc, &laid);
    let cut = scene
        .pages
        .iter()
        .flat_map(|p| p.front.iter().chain(&p.behind))
        .flat_map(|f| &f.pictures)
        .filter(|p| p.shape.is_some())
        .count();
    assert_eq!(cut, 4);
    let pdf = docs_pdf::export(&doc, &mut engine).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
}

/// Every zip entry's bytes, by name.
fn entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut out = Vec::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let mut v = Vec::new();
        f.read_to_end(&mut v).unwrap();
        out.push((f.name().unwrap().to_string(), v));
    }
    out.sort();
    out
}

#[test]
fn saved_unchanged() {
    let mut doc = open();
    let dir = std::env::temp_dir().join(format!("kantor-pictures-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("pictures.docx");
    // An edit elsewhere: the pictures and SmartArt parts stay byte for byte.
    doc.insert_text(Selection::default(), "Edited. ", None);
    doc.save_as(&out).unwrap();
    let before = entries(&std::fs::read(path()).unwrap());
    let after = entries(&std::fs::read(&out).unwrap());
    for (name, bytes) in &before {
        if name == "word/document.xml" {
            continue;
        }
        let saved = after.iter().find(|(n, _)| n == name);
        assert_eq!(saved.map(|s| &s.1), Some(bytes), "{name}");
    }
    // Every drawing in the body is written back as read.
    let doc_xml = |e: &[(String, Vec<u8>)]| {
        let x = &e.iter().find(|(n, _)| n == "word/document.xml").unwrap().1;
        String::from_utf8(x.clone()).unwrap()
    };
    let (a, b) = (doc_xml(&before), doc_xml(&after));
    assert!(b.contains("Edited. "));
    let drawings = |x: &str| {
        x.match_indices("<w:drawing>")
            .map(|(i, _)| {
                let end = x[i..].find("</w:drawing>").unwrap();
                x[i..i + end].to_owned()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(drawings(&a).len(), 10);
    assert_eq!(drawings(&a), drawings(&b));
    std::fs::remove_dir_all(&dir).ok();
}
