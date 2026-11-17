//! Pattern fills and a chart inside a group, in
//! tests/corpus/pptx/patterns.pptx (`scripts/corpus/gen_pptx_patterns.py`).

use std::path::PathBuf;

use slides_engine::{Layer, Paint as Fill, Presentation};
use slides_scene::{Builder, Engine, Paint, Prim};

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/pptx")
        .join(name)
}

fn index(p: &Presentation, name: &str) -> usize {
    p.slide(0)
        .shapes
        .iter()
        .position(|s| s.name == name)
        .unwrap_or_else(|| panic!("no {name}"))
}

fn patterns(items: &[slides_scene::Item], layer: Layer) -> Vec<(String, [u8; 4], [u8; 4])> {
    items
        .iter()
        .filter(|i| i.layer == Some(layer))
        .filter_map(|i| match &i.prim {
            Prim::Fill {
                paint: Paint::Pattern { fg, bg, bits, .. },
                ..
            } => Some((
                kantor_ooxml::pattern::from_bits(bits)
                    .map(|(p, _)| p.prst.to_owned())
                    .unwrap_or_default(),
                fg.0,
                bg.0,
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn pattern_fills_and_a_grouped_chart_are_drawn() {
    let p = Presentation::open(&corpus("patterns.pptx")).unwrap();
    let mut engine = Engine::new();
    let scene = Builder::new().slide(&mut engine, &p, 0, &Default::default());
    let boxed = Layer::Slide(index(&p, "Pattern box"));
    assert_eq!(
        patterns(&scene.items, boxed),
        [(
            "dkUpDiag".to_owned(),
            [0x44, 0x72, 0xC4, 255],
            [255, 255, 255, 255]
        )]
    );
    let oval = Layer::Slide(index(&p, "Pattern oval"));
    assert_eq!(patterns(&scene.items, oval)[0].0, "smCheck");
    // The chart inside the group: four patterned bars.
    let group = Layer::Slide(index(&p, "Chart group"));
    let bars = patterns(&scene.items, group);
    assert_eq!(bars.len(), 4, "{bars:?}");
    assert!(
        bars.iter()
            .all(|b| b.0 == "ltUpDiag" && b.1 == [0x54, 0x82, 0x35, 255])
    );
}

#[test]
fn the_fill_menus_pattern_is_drawn_and_saved() {
    let mut p = Presentation::open(&corpus("patterns.pptx")).unwrap();
    let k = index(&p, "Pattern oval");
    p.set_fill(
        (0, k),
        Fill::Pattern {
            prst: "lgGrid".into(),
            fg: Box::new(Fill::Scheme("accent2".into())),
            bg: Box::new(Fill::Rgb(slides_engine::Rgb([255, 255, 255]))),
        },
    )
    .unwrap();
    let mut engine = Engine::new();
    let scene = Builder::new().slide(&mut engine, &p, 0, &Default::default());
    let got = patterns(&scene.items, Layer::Slide(k));
    assert_eq!(got[0].0, "lgGrid");
    assert_eq!(got[0].2, [255, 255, 255, 255]);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("patterns");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("saved.pptx");
    p.save_as(&path).unwrap();
    let back = Presentation::open(&path).unwrap();
    let mut engine = Engine::new();
    let scene = Builder::new().slide(&mut engine, &back, 0, &Default::default());
    let got = patterns(&scene.items, Layer::Slide(index(&back, "Pattern oval")));
    assert_eq!(got[0].0, "lgGrid");
    let mut zip = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
    let mut xml = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("ppt/slides/slide1.xml").unwrap(), &mut xml)
        .unwrap();
    assert!(
        xml.contains(
            "<a:pattFill prst=\"lgGrid\"><a:fgClr><a:schemeClr val=\"accent2\"/></a:fgClr>"
        ),
        "{xml}"
    );
}
