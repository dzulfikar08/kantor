//! Text boxes with `<a:bodyPr wrap="none">` (python-pptx's default for
//! `add_textbox`): every paragraph keeps its own line, placed in the box by
//! its alignment, and overflows the box's edges when it's wider. A
//! `wrap="square"` box with the same paragraphs is the reference. Checks
//! positions relative to the box, so nothing depends on the machine's fonts.

use std::collections::BTreeMap;
use std::io::{Cursor, Read as _, Write as _};

use slides_engine::Presentation;
use slides_scene::{Builder, Engine, Prim, SlideScene, p};

/// Alignment and text of each paragraph.
const PARAS: [(&str, &str); 6] = [
    ("l", "Left line"),
    ("ctr", "Centre"),
    ("r", "Right edge text"),
    (
        "ctr",
        "A centred line much longer than the narrow box it sits in",
    ),
    ("ctr", "中文居中"),
    (
        "r",
        "Long right-aligned line that overflows on the left side",
    ),
];

/// Box left edges (px); both boxes are 288 px wide, 9.6 px insets.
const NONE_X: f32 = 384.0;
const SQUARE_X: f32 = 960.0;
const BOX_W: f32 = 288.0;
const INSET: f32 = 9.6;

fn text_box(id: u32, x_emu: i64, wrap: &str) -> String {
    let paras: String = PARAS
        .iter()
        .map(|(algn, t)| {
            format!(
                r#"<a:p><a:pPr algn="{algn}"/><a:r><a:rPr lang="en-US" sz="1800"><a:solidFill><a:srgbClr val="000000"/></a:solidFill></a:rPr><a:t>{t}</a:t></a:r></a:p>"#
            )
        })
        .collect();
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="Box {id}"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="{x_emu}" y="914400"/><a:ext cx="2743200" cy="3657600"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/></p:spPr><p:txBody><a:bodyPr wrap="{wrap}"><a:spAutoFit/></a:bodyPr><a:lstStyle/>{paras}</p:txBody></p:sp>"#
    )
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

fn zip_parts(mut all: BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    let ct = all.remove("[Content_Types].xml").unwrap();
    zip.start_file("[Content_Types].xml", opts).unwrap();
    zip.write_all(&ct).unwrap();
    for (k, v) in all {
        zip.start_file(k.as_str(), opts).unwrap();
        zip.write_all(&v).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// One slide holding the unwrapped box (left) and the wrapped one (right).
fn deck(dir: &std::path::Path) -> std::path::PathBuf {
    let mut pres = Presentation::new();
    pres.new_slide(Some(0)).unwrap();
    let plain = dir.join("plain.pptx");
    pres.save_as(&plain).unwrap();
    let mut all = parts(&std::fs::read(&plain).unwrap());
    let k = "ppt/slides/slide1.xml";
    let s = String::from_utf8(all[k].clone()).unwrap();
    let boxes = text_box(40, 3_657_600, "none") + &text_box(41, 9_144_000, "square");
    assert!(s.contains("</p:spTree>"));
    all.insert(
        k.into(),
        s.replace("</p:spTree>", &format!("{boxes}</p:spTree>"))
            .into_bytes(),
    );
    let path = dir.join("nowrap.pptx");
    std::fs::write(&path, zip_parts(all)).unwrap();
    path
}

/// A drawn line: paragraph, baseline and its left / right edges (slide px).
#[derive(Debug, Clone, Copy)]
struct Line {
    para: usize,
    baseline: f32,
    x0: f32,
    x1: f32,
}

/// The lines of the text drawn inside `[left, left + BOX_W)` (by where the
/// text item's origin is).
fn lines(scene: &SlideScene, left: f32) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    for item in &scene.items {
        let Prim::Text(t) = &item.prim else { continue };
        let o = item.transform.apply(t.origin);
        if !(o.x >= left && o.x < left + BOX_W) {
            continue;
        }
        for r in &t.runs {
            let run = &r.run;
            for g in &run.glyphs {
                let a = item
                    .transform
                    .apply(p(t.origin.x + g.x, t.origin.y + run.baseline));
                let b = item
                    .transform
                    .apply(p(t.origin.x + g.x + g.advance, t.origin.y + run.baseline));
                match out
                    .iter_mut()
                    .find(|l| l.para == run.para && (l.baseline - a.y).abs() < 0.5)
                {
                    Some(l) => {
                        l.x0 = l.x0.min(a.x);
                        l.x1 = l.x1.max(b.x);
                    }
                    None => out.push(Line {
                        para: run.para,
                        baseline: a.y,
                        x0: a.x,
                        x1: b.x,
                    }),
                }
            }
        }
    }
    out.sort_by(|a, b| a.baseline.total_cmp(&b.baseline));
    out
}

fn close(a: f32, b: f32, what: &str, lines: &[Line]) {
    assert!((a - b).abs() < 1.0, "{what}: {a} vs {b}\n{lines:#?}");
}

#[test]
fn unwrapped_paragraphs_keep_their_alignment() {
    let dir = tempfile::tempdir().unwrap();
    let pres = Presentation::open(&deck(dir.path())).unwrap();
    let mut engine = Engine::new();
    let scene = Builder::new().slide(&mut engine, &pres, 0, &Default::default());

    // wrap="none": one line per paragraph, every one drawn.
    let (l, r) = (NONE_X + INSET, NONE_X + BOX_W - INSET);
    let mid = (l + r) / 2.0;
    let none = lines(&scene, NONE_X);
    let paras: Vec<usize> = none.iter().map(|x| x.para).collect();
    assert_eq!(paras, vec![0, 1, 2, 3, 4, 5], "{none:#?}");
    let by = |i: usize| none[i];
    close(by(0).x0, l, "left line starts at the inset", &none);
    close(
        (by(1).x0 + by(1).x1) / 2.0,
        mid,
        "short centred line",
        &none,
    );
    close(by(2).x1, r, "right line ends at the inset", &none);
    let long = by(3);
    close((long.x0 + long.x1) / 2.0, mid, "long centred line", &none);
    assert!(
        long.x0 < l && long.x1 > r,
        "overflows both sides: {none:#?}"
    );
    close((by(4).x0 + by(4).x1) / 2.0, mid, "CJK centred line", &none);
    let long_r = by(5);
    close(long_r.x1, r, "long right line ends at the inset", &none);
    assert!(long_r.x0 < l, "overflows on the left: {none:#?}");

    // wrap="square": the long lines wrap inside the box; short ones sit
    // where they do without wrapping.
    let (l, r) = (SQUARE_X + INSET, SQUARE_X + BOX_W - INSET);
    let mid = (l + r) / 2.0;
    let square = lines(&scene, SQUARE_X);
    for para in 0..PARAS.len() {
        assert!(square.iter().any(|x| x.para == para), "{square:#?}");
    }
    let count = |para| square.iter().filter(|x| x.para == para).count();
    assert!(count(3) > 1 && count(5) > 1, "long lines wrap: {square:#?}");
    for x in &square {
        // A wrapped line may end in its (hanging) space.
        assert!(x.x0 > l - 1.0 && x.x1 < r + 12.0, "inside the box: {x:?}");
    }
    let first = |para| *square.iter().find(|x| x.para == para).unwrap();
    close(first(0).x0, l, "square: left", &square);
    close(
        (first(1).x0 + first(1).x1) / 2.0,
        mid,
        "square: centred",
        &square,
    );
    close(first(2).x1, r, "square: right", &square);
}

#[test]
fn pdf_draws_every_unwrapped_line_on_the_page() {
    let dir = tempfile::tempdir().unwrap();
    let pres = Presentation::open(&deck(dir.path())).unwrap();
    let mut engine = Engine::new();
    // The PDF draws these scenes.
    let d = slides_pdf::deck(&pres, &Builder::new(), &mut engine, false);
    let none = lines(&d.slides[0], NONE_X);
    assert_eq!(none.len(), PARAS.len(), "{none:#?}");
    for x in &none {
        assert!(x.x0 >= 0.0 && x.x1 <= d.width, "on the page: {x:?}");
    }
    let pdf = slides_pdf::write(&d).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
    assert!(pdf.len() > 1000);
}
