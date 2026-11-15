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
    assert_eq!(Vert::parse(Some("mongolianVert")), Some(Vert::EaDown));
    assert_eq!(Vert::parse(Some("wordArtVertRtl")), Some(Vert::Stacked));
    assert_eq!(Vert::parse(Some("vert")), Some(Vert::Down));
}
