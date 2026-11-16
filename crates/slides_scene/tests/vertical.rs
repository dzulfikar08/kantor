//! East Asian vertical text (`eaVert`) in tests/corpus/pptx/frames.pptx:
//! CJK characters stand upright, Latin text is turned with the line, as
//! PowerPoint draws it.

use std::path::PathBuf;

use slides_engine::frame::{Vert, upright_in_vertical};
use slides_engine::{Position, Presentation, Selection};
use slides_scene::{Builder, Engine, Item, Prim};

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/pptx")
        .join(name)
}

/// Text items of shape `k` on the slide: (text, upright?).
fn texts(items: &[Item], k: usize) -> Vec<(String, bool)> {
    items
        .iter()
        .filter(|i| i.layer == Some(slides_engine::Layer::Slide(k)))
        .filter_map(|i| match &i.prim {
            Prim::Text(t) => Some((
                t.runs.iter().map(|r| r.sample()).collect::<String>(),
                i.transform.is_upright(),
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn cjk_stands_upright_and_latin_turns_in_ea_vertical_text() {
    let mut p = Presentation::open(&corpus("frames.pptx")).unwrap();
    let k = p
        .slide(2)
        .shapes
        .iter()
        .position(|s| s.name == "EaVert")
        .unwrap();
    assert_eq!(
        p.slide(2).shapes[k].text.as_ref().unwrap().body.vert,
        Some(Vert::EaDown)
    );
    let mut engine = Engine::new();
    let scene = Builder::new().slide(&mut engine, &p, 2, &Default::default());
    let t = texts(&scene.items, k);
    // 縦書き: three characters, each its own upright item.
    assert_eq!(t.len(), 3, "{t:?}");
    assert!(t.iter().all(|(_, up)| *up), "{t:?}");
    let chars: String = t.iter().map(|x| x.0.as_str()).collect();
    assert_eq!(chars, "縦書き");
    // Latin text in it turns with the line; the ideographs stay upright
    // and run down the box in order.
    let end = Position::new(0, "縦書き".len());
    p.insert_text((2, k), Selection::caret(end), "AB", None)
        .unwrap();
    let scene = Builder::new().slide(&mut engine, &p, 2, &Default::default());
    let t = texts(&scene.items, k);
    assert!(t.contains(&("AB".into(), false)), "{t:?}");
    let ups: Vec<&Item> = scene
        .items
        .iter()
        .filter(|i| {
            i.layer == Some(slides_engine::Layer::Slide(k))
                && i.transform.is_upright()
                && matches!(i.prim, Prim::Text(_))
        })
        .collect();
    assert_eq!(ups.len(), 3);
    let ys: Vec<f32> = ups
        .iter()
        .map(|i| i.transform.apply(slides_scene::p(0.0, 0.0)).y)
        .collect();
    // Upright glyphs keep the layout's origin: compare where each lands.
    let centres: Vec<f32> = ups
        .iter()
        .map(|i| {
            let Prim::Text(t) = &i.prim else {
                unreachable!()
            };
            let g = &t.runs[0].run.glyphs[0];
            i.transform
                .apply(slides_scene::p(t.origin.x + g.x, t.origin.y + g.y))
                .y
        })
        .collect();
    assert!(
        centres.windows(2).all(|w| w[1] > w[0]),
        "down the box: {centres:?} {ys:?}"
    );
}

#[test]
fn which_characters_stand_upright() {
    for c in ['縦', 'か', 'カ', '한', '。', 'Ａ', '１'] {
        assert!(upright_in_vertical(c), "{c}");
    }
    for c in ['A', '1', 'ー', '「', '」', '（', '〜', 'é'] {
        assert!(!upright_in_vertical(c), "{c}");
    }
    assert_eq!(Vert::parse(Some("mongolianVert")), Some(Vert::Mongolian));
    assert_eq!(Vert::parse(Some("wordArtVertRtl")), Some(Vert::Stacked));
    assert_eq!(Vert::parse(Some("vert")), Some(Vert::Down));
}

/// frames.pptx with its eaVert box made `vert` (as read from the slide).
fn frames_with(vert: &str) -> Presentation {
    use std::io::{Read as _, Write as _};
    let bytes = std::fs::read(corpus("frames.pptx")).unwrap();
    let mut zin = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for i in 0..zin.len() {
        let mut f = zin.by_index(i).unwrap();
        let name = f.name().unwrap().into_owned();
        let mut v = Vec::new();
        f.read_to_end(&mut v).unwrap();
        if name == "ppt/slides/slide3.xml" {
            v = String::from_utf8(v)
                .unwrap()
                .replace("vert=\"eaVert\"", &format!("vert=\"{vert}\""))
                .into_bytes();
        }
        out.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        out.write_all(&v).unwrap();
    }
    Presentation::from_bytes(out.finish().unwrap().into_inner()).unwrap()
}

/// Where the turned (Latin) runs "AB" and "CD" land across the box.
fn line_xs(vert: &str) -> (f32, f32) {
    let mut p = frames_with(vert);
    let k = p
        .slide(2)
        .shapes
        .iter()
        .position(|s| s.name == "EaVert")
        .unwrap();
    let end = Position::new(0, "縦書き".len());
    p.insert_text((2, k), Selection::caret(end), "AB\nCD", None)
        .unwrap();
    let mut engine = Engine::new();
    let scene = Builder::new().slide(&mut engine, &p, 2, &Default::default());
    let x = |want: &str| {
        scene
            .items
            .iter()
            .filter(|i| i.layer == Some(slides_engine::Layer::Slide(k)))
            .find_map(|i| match &i.prim {
                Prim::Text(t) if t.runs.iter().any(|r| r.sample() == want) => {
                    let r = t.runs.iter().find(|r| r.sample() == want).unwrap();
                    Some(
                        i.transform
                            .apply(slides_scene::p(t.origin.x, t.origin.y + r.run.baseline))
                            .x,
                    )
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("{want} drawn"))
    };
    (x("AB"), x("CD"))
}

#[test]
fn mongolian_vertical_lines_stack_left_to_right() {
    assert_eq!(
        frames_with("mongolianVert")
            .slide(2)
            .shapes
            .iter()
            .find(|s| s.name == "EaVert")
            .unwrap()
            .text
            .as_ref()
            .unwrap()
            .body
            .vert,
        Some(Vert::Mongolian)
    );
    // eaVert: the second line left of the first; mongolianVert: right.
    let (a, b) = line_xs("eaVert");
    assert!(b < a, "eaVert {a} {b}");
    let (a, b) = line_xs("mongolianVert");
    assert!(b > a, "mongolianVert {a} {b}");
}
