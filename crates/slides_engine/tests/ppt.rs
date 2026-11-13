//! PowerPoint 97-2003 import: the synthetic decks in `tests/corpus/ppt`
//! (made by `make_fixtures.py`: python-pptx decks converted by
//! LibreOffice) open with their text, formatting, shapes, pictures,
//! tables, notes and slide size, and damaged files fail cleanly.

use std::io::{Cursor, Read as _};
use std::path::{Path, PathBuf};

use slides_engine::frame::FrameContent;
use slides_engine::*;

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/ppt")
        .join(name)
}

fn open(name: &str) -> Presentation {
    let p = Presentation::open(&corpus(name)).unwrap();
    if let Some(dir) = std::env::var_os("KANTOR_PPT_DUMP") {
        let out = Path::new(&dir).join(name).with_extension("pptx");
        std::fs::write(out, p.to_bytes().unwrap()).unwrap();
    }
    p
}

fn texts(p: &Presentation, slide: usize) -> Vec<String> {
    p.slide(slide)
        .shapes
        .iter()
        .filter_map(|s| s.text.as_ref().map(|t| t.plain_text()))
        .collect()
}

fn shape_with<'a>(p: &'a Presentation, slide: usize, text: &str) -> &'a Shape {
    p.slide(slide)
        .shapes
        .iter()
        .find(|s| {
            s.text
                .as_ref()
                .is_some_and(|t| t.plain_text().contains(text))
        })
        .unwrap_or_else(|| panic!("no shape with {text:?} on slide {slide}"))
}

const CM: i64 = 360_000;

fn near(a: i64, b: i64) -> bool {
    // Master units are 1/576 in (1587.5 EMU).
    (a - b).abs() <= 2000
}

fn streams(name: &str) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let bytes = std::fs::read(corpus(name)).unwrap();
    let mut cf = cfb::CompoundFile::open(Cursor::new(bytes)).unwrap();
    let mut read = |n: &str| {
        let mut v = Vec::new();
        cf.open_stream(n).unwrap().read_to_end(&mut v).unwrap();
        v
    };
    (
        read("PowerPoint Document"),
        read("Current User"),
        read("Pictures"),
    )
}

#[test]
fn opens_as_an_unsaved_import() {
    let p = open("basic.ppt");
    assert_valid(&p.to_bytes().unwrap());
    assert!(p.is_imported());
    assert_eq!(p.path(), None);
    assert_eq!(p.source_path(), Some(corpus("basic.ppt").as_path()));
    assert_eq!(p.title(), "basic.ppt");
    assert_eq!(p.suggested_file_name(), "basic.pptx");
    assert!(p.needs_save_as());
    assert!(!p.is_dirty());
    assert_eq!(p.len(), 7);
    // python-pptx's template is 4:3.
    assert_eq!((p.size.cx, p.size.cy), (9_144_000, 6_858_000));
}

fn assert_valid(bytes: &[u8]) {
    let problems = package_problems(bytes);
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn slide_size_and_background() {
    let p = open("wide.ppt");
    assert_eq!((p.size.cx, p.size.cy), (12_192_000, 6_858_000));
    let bg = p.slide(0).bg.or(p.masters[0].bg);
    assert_eq!(bg.map(|c| c.hex()), Some("FFF4D6".into()));
    let t = shape_with(&p, 0, "Widescreen");
    let run = &t.text.as_ref().unwrap().paras[0].runs[0];
    assert_eq!(run.props.size, Some(4000));
    let x = t.xfrm.unwrap();
    assert!(near(x.x, 2 * CM) && near(x.y, 2 * CM), "{x:?}");
}

#[test]
fn text_and_formatting() {
    let p = open("basic.ppt");
    assert_eq!(
        texts(&p, 0),
        ["Quarterly Review", "Prepared with Kantor fixtures"]
    );
    // LibreOffice writes the title slide's title as a plain title.
    let title = &p.slide(0).shapes[0];
    assert_eq!(
        title.ph.as_ref().and_then(|ph| ph.kind.as_deref()),
        Some("title")
    );
    let body = shape_with(&p, 1, "First level bullet");
    let paras = &body.text.as_ref().unwrap().paras;
    let levels: Vec<u8> = paras.iter().map(|p| p.props.level).collect();
    assert_eq!(levels, [0, 1, 2, 0]);
    let last = &paras[3];
    assert_eq!(last.text, "Bold italic underlined red 28pt Courier");
    let run = |text: &str| {
        let mut at = 0;
        for r in &last.runs {
            if last.text[at..at + r.len].contains(text) {
                return &r.props;
            }
            at += r.len;
        }
        panic!("no run {text:?}");
    };
    assert_eq!(run("Bold").bold, Some(true));
    assert_eq!(run("italic").italic, Some(true));
    assert_eq!(run("underlined").underline, Some(true));
    let red = run("red 28pt");
    assert_eq!(red.size, Some(2800));
    assert_eq!(red.color.map(|c| c.hex()), Some("C01010".into()));
    let bytes = p.to_bytes().unwrap();
    let xml = part_text(&bytes, "ppt/slides/slide2.xml");
    assert!(xml.contains("typeface=\"Courier New\""), "{xml}");
}

fn part_text(bytes: &[u8], name: &str) -> String {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut s = String::new();
    zip.by_name(name).unwrap().read_to_string(&mut s).unwrap();
    s
}

#[test]
fn unicode_and_alignment() {
    let p = open("basic.ppt");
    let t = shape_with(&p, 5, "Selamat datang");
    let paras = &t.text.as_ref().unwrap().paras;
    assert_eq!(paras[0].text, "Selamat datang di rapat tahunan");
    assert_eq!(paras[1].text, "日本語のテキスト 中文 한국어");
    assert_eq!(paras[2].text, "Right aligned — “quotes” € 100");
    assert_eq!(paras[1].props.align, Some(Align::Center));
    assert_eq!(paras[2].props.align, Some(Align::Right));
}

#[test]
fn shapes_positions_and_geometry() {
    let p = open("basic.ppt");
    let s = &p.slide(2).shapes;
    let geom = |i: usize| match &s[i].kind {
        ShapeKind::Shape { geom, .. } => geom.clone(),
        k => panic!("{k:?}"),
    };
    assert_eq!(geom(0), Geom::Rect);
    assert_eq!(geom(1), Geom::RoundRect);
    assert_eq!(geom(2), Geom::Ellipse);
    assert_eq!(geom(3), Geom::Other("triangle".into()));
    assert_eq!(geom(4), Geom::Other("rightArrow".into()));
    assert_eq!(geom(5), Geom::Other("star5".into()));
    assert_eq!(geom(6), Geom::Line);
    assert_eq!(geom(7), Geom::Other("rtTriangle".into()));

    let rect = s[0].xfrm.unwrap();
    assert!(near(rect.x, CM) && near(rect.y, CM), "{rect:?}");
    assert!(near(rect.cx, 6 * CM) && near(rect.cy, 3 * CM), "{rect:?}");
    assert_eq!(s[0].look.fill.map(|c| c.hex()), Some("208040".into()));
    let (line_color, line_w) = s[0].look.line.unwrap();
    assert_eq!(line_color.hex(), "000080");
    assert!((line_w - 38_100).abs() < 200, "{line_w}");
    assert_eq!(texts(&p, 2), ["Green box"]);

    let arrow = s[4].xfrm.unwrap();
    assert_eq!(arrow.rot, 30 * 60_000);
    assert!(near(arrow.x, 7 * CM) && near(arrow.cx, 6 * CM), "{arrow:?}");
    assert!(s[7].xfrm.unwrap().flip_h);
    let line = s[6].xfrm.unwrap();
    assert!(near(line.x, CM) && near(line.y, 12 * CM), "{line:?}");
    assert!(near(line.cx, 11 * CM) && near(line.cy, 4 * CM), "{line:?}");
}

#[test]
fn pictures() {
    let p = open("basic.ppt");
    let pics: Vec<_> = p
        .slide(3)
        .shapes
        .iter()
        .filter_map(|s| match &s.kind {
            ShapeKind::Picture { part, .. } => Some((part.clone().unwrap(), s.xfrm.unwrap())),
            _ => None,
        })
        .collect();
    assert_eq!(pics.len(), 2);
    let png = p.part_bytes(&pics[0].0).unwrap();
    assert!(png.starts_with(b"\x89PNG"), "{}", pics[0].0);
    let jpeg = p.part_bytes(&pics[1].0).unwrap();
    assert!(jpeg.starts_with(&[0xFF, 0xD8]), "{}", pics[1].0);
    assert!(near(pics[0].1.x, 2 * CM) && near(pics[0].1.cx, 8 * CM));
    assert!(p.import_notes().is_empty(), "{:?}", p.import_notes());
}

#[test]
fn tables_become_tables() {
    let p = open("basic.ppt");
    let table = p
        .slide(4)
        .shapes
        .iter()
        .find_map(|s| match &s.kind {
            ShapeKind::Other { frame, .. } => match frame.get() {
                Some(FrameContent::Table(t)) => Some(t.clone()),
                _ => None,
            },
            _ => None,
        })
        .expect("a table");
    assert_eq!(table.cols.len(), 3);
    assert_eq!(table.rows.len(), 3);
    let cell = |r: usize, c: usize| {
        table.rows[r].cells[c]
            .text
            .as_ref()
            .map(|t| t.plain_text())
            .unwrap_or_default()
    };
    assert_eq!(cell(0, 0), "R1C1");
    assert_eq!(cell(1, 2), "R2C3");
    assert_eq!(cell(2, 1), "R3C2");
    assert!(table.rows[0].cells[0].fill.is_some());
}

#[test]
fn speaker_notes_and_hidden_slides() {
    let p = open("basic.ppt");
    assert_eq!(p.notes_text(0), "Welcome everyone.\nSecond notes line.");
    assert_eq!(p.notes_text(1), "Talk about formatting.");
    assert_eq!(p.notes_text(2), "");
    assert_eq!(p.notes_text(5), "Catatan pembicara: terima kasih");
    let hidden: Vec<bool> = p.slides().iter().map(|s| s.hidden).collect();
    assert_eq!(hidden, [false, false, false, false, false, false, true]);
}

#[test]
fn show_and_template_files_open_too() {
    let dir = tempfile::tempdir().unwrap();
    for ext in ["pps", "pot", "PPT"] {
        let path = dir.path().join(format!("deck.{ext}"));
        std::fs::copy(corpus("wide.ppt"), &path).unwrap();
        let p = Presentation::open(&path).unwrap();
        assert!(p.is_imported(), "{ext}");
        assert_eq!(p.suggested_file_name(), "deck.pptx");
    }
}

#[test]
fn save_writes_a_new_pptx() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("deck.ppt");
    std::fs::copy(corpus("basic.ppt"), &src).unwrap();
    let before = std::fs::read(&src).unwrap();
    let mut p = Presentation::open(&src).unwrap();
    assert!(p.save().is_err());
    let err = p.save_as(&src).unwrap_err().to_string();
    assert!(err.contains(".pptx"), "{err}");
    let out = dir.path().join("deck.pptx");
    p.save_as(&out).unwrap();
    assert_eq!(
        std::fs::read(&src).unwrap(),
        before,
        "the .ppt is untouched"
    );
    assert!(!p.is_imported());
    assert_eq!(p.path(), Some(out.as_path()));
    let q = Presentation::open(&out).unwrap();
    assert_eq!(q.len(), 7);
    assert_eq!(texts(&q, 0), texts(&p, 0));
    assert_eq!(q.notes_text(0), "Welcome everyone.\nSecond notes line.");
}

#[test]
fn recovered_imports_stay_imports() {
    let p = open("basic.ppt");
    let bytes = p.to_bytes().unwrap();
    let r = Presentation::open_recovered(bytes, Some(corpus("basic.ppt")), Vec::new()).unwrap();
    assert_eq!(r.path(), None);
    assert!(r.needs_save_as());
    assert_eq!(r.title(), "basic.ppt");
    assert_eq!(r.suggested_file_name(), "basic.pptx");
}

#[test]
fn damaged_files_give_errors() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = std::fs::read(corpus("basic.ppt")).unwrap();
    for (name, data) in [
        ("empty.ppt", Vec::new()),
        ("text.ppt", b"just some text".to_vec()),
        ("truncated.ppt", bytes[..bytes.len() / 3].to_vec()),
        ("header-only.ppt", bytes[..512].to_vec()),
    ] {
        let path = dir.path().join(name);
        std::fs::write(&path, data).unwrap();
        let err = format!("{:#}", Presentation::open(&path).err().expect(name));
        assert!(err.contains("can't be opened"), "{name}: {err}");
    }
}

#[test]
fn encrypted_and_old_files_are_refused() {
    let (doc, mut user, pics) = streams("basic.ppt");
    user[12..16].copy_from_slice(&0xF3D1_C4DFu32.to_le_bytes());
    let err = ppt::convert_streams(&doc, &user, &pics).err().unwrap();
    assert!(err.to_string().contains("password-protected"), "{err}");

    let (doc, mut user, pics) = streams("basic.ppt");
    // docFileVersion (after size, token, offset and the name length).
    user[22..24].copy_from_slice(&0x03F3u16.to_le_bytes());
    let err = ppt::convert_streams(&doc, &user, &pics).err().unwrap();
    assert!(err.to_string().contains("PowerPoint 95"), "{err}");
}

/// Flip bytes all over the document stream: every result is a
/// presentation or an error, never a panic.
#[test]
fn corrupted_streams_never_panic() {
    let (doc, user, pics) = streams("basic.ppt");
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for round in 0..300 {
        let mut d = doc.clone();
        for _ in 0..1 + round % 8 {
            let i = (next() % d.len() as u64) as usize;
            d[i] = next() as u8;
        }
        if let Ok(imp) = ppt::convert_streams(&d, &user, &pics) {
            let _ = Presentation::from_bytes(imp.pptx);
        }
    }
    // Damaged pictures and a cut-off stream.
    let mut p = pics.clone();
    for b in p.iter_mut().step_by(7) {
        *b = !*b;
    }
    let _ = ppt::convert_streams(&doc, &user, &p);
    for cut in [0, 8, 100, doc.len() / 2, doc.len() - 1] {
        let _ = ppt::convert_streams(&doc[..cut], &user, &pics);
    }
}
