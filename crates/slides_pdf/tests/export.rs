//! PDF export of a presentation generated here: a new deck saved by
//! slides_engine, its slide XML extended with a gradient background, a
//! gradient shape, a dashed connector with arrowheads, a turned text box,
//! a cropped picture and a hidden slide. Checks the PDF's structure by
//! reading it back, so nothing depends on which fonts the machine has.

use std::collections::BTreeMap;
use std::io::{Cursor, Read as _, Write as _};

use slides_engine::Presentation;
use slides_scene::{Builder, Engine, Prim};

/// A `w` × `h` PNG of one colour.
fn png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let mut c = flate2::Crc::new();
        c.update(kind);
        c.update(data);
        out.extend(kind);
        out.extend(data);
        out.extend(c.sum().to_be_bytes());
    }
    let row: Vec<u8> = std::iter::once(0)
        .chain(rgb.into_iter().cycle().take(3 * w as usize))
        .collect();
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    z.write_all(&row.repeat(h as usize)).unwrap();
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend(w.to_be_bytes());
    ihdr.extend(h.to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z.finish().unwrap());
    chunk(&mut out, b"IEND", &[]);
    out
}

const BG: &str = r#"<p:bg><p:bgPr><a:gradFill rotWithShape="1"><a:gsLst><a:gs pos="0"><a:srgbClr val="1F3864"/></a:gs><a:gs pos="100000"><a:srgbClr val="2E75B6"/></a:gs></a:gsLst><a:lin ang="5400000" scaled="0"/></a:gradFill><a:effectLst/></p:bgPr></p:bg>"#;

fn shapes() -> String {
    String::from(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="20" name="Grad"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="914400" y="914400"/><a:ext cx="3657600" cy="1828800"/></a:xfrm><a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom><a:gradFill><a:gsLst><a:gs pos="0"><a:srgbClr val="FFFFFF"/></a:gs><a:gs pos="50000"><a:srgbClr val="FFC000"><a:alpha val="60000"/></a:srgbClr></a:gs><a:gs pos="100000"><a:srgbClr val="C00000"/></a:gs></a:gsLst><a:path path="circle"><a:fillToRect l="50000" t="50000" r="50000" b="50000"/></a:path></a:gradFill></p:spPr></p:sp>
<p:cxnSp><p:nvCxnSpPr><p:cNvPr id="21" name="Arrow"/><p:cNvCxnSpPr/><p:nvPr/></p:nvCxnSpPr><p:spPr><a:xfrm><a:off x="5486400" y="914400"/><a:ext cx="2743200" cy="914400"/></a:xfrm><a:prstGeom prst="straightConnector1"><a:avLst/></a:prstGeom><a:ln w="38100" cap="rnd"><a:solidFill><a:srgbClr val="00B050"/></a:solidFill><a:prstDash val="dash"/><a:headEnd type="oval"/><a:tailEnd type="triangle" w="lg" len="lg"/></a:ln></p:spPr></p:cxnSp>
<p:sp><p:nvSpPr><p:cNvPr id="22" name="Turned"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm rot="2700000"><a:off x="914400" y="3657600"/><a:ext cx="3657600" cy="914400"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/></p:spPr><p:txBody><a:bodyPr wrap="square"/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" sz="2800"><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill></a:rPr><a:t>Turned words</a:t></a:r></a:p></p:txBody></p:sp>
<p:pic><p:nvPicPr><p:cNvPr id="23" name="Pic"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rIdPic"><a:alphaModFix amt="50000"/></a:blip><a:srcRect l="50000"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm flipH="1"><a:off x="7315200" y="3657600"/><a:ext cx="1828800" cy="1828800"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>"#,
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

/// The test deck: two slides, the second hidden.
fn deck(dir: &std::path::Path) -> std::path::PathBuf {
    let mut p = Presentation::new();
    p.new_slide(Some(0)).unwrap();
    let plain = dir.join("plain.pptx");
    p.save_as(&plain).unwrap();
    let mut all = parts(&std::fs::read(&plain).unwrap());
    let edit = |all: &mut BTreeMap<String, Vec<u8>>, k: &str, f: &dyn Fn(String) -> String| {
        let s = String::from_utf8(all[k].clone()).unwrap();
        all.insert(k.to_owned(), f(s).into_bytes());
    };
    edit(&mut all, "ppt/slides/slide1.xml", &|s| {
        assert!(s.contains("<p:cSld><p:spTree>"), "{s}");
        s.replace("<p:cSld><p:spTree>", &format!("<p:cSld>{BG}<p:spTree>"))
            .replace("</p:spTree>", &format!("{}</p:spTree>", shapes()))
    });
    edit(&mut all, "ppt/slides/slide2.xml", &|s| {
        s.replacen("<p:sld ", "<p:sld show=\"0\" ", 1)
    });
    edit(&mut all, "ppt/slides/_rels/slide1.xml.rels", &|s| {
        s.replace(
            "</Relationships>",
            r#"<Relationship Id="rIdPic" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/></Relationships>"#,
        )
    });
    edit(&mut all, "[Content_Types].xml", &|s| {
        if s.contains("Extension=\"png\"") {
            s
        } else {
            s.replacen(
                "<Default ",
                r#"<Default Extension="png" ContentType="image/png"/><Default "#,
                1,
            )
        }
    });
    // Layouts get a plain red background: slides without their own inherit it.
    let layouts: Vec<String> = all
        .keys()
        .filter(|k| k.starts_with("ppt/slideLayouts/slideLayout") && k.ends_with(".xml"))
        .cloned()
        .collect();
    for l in layouts {
        edit(&mut all, &l, &|s| {
            let at = s.find("<p:cSld").unwrap();
            let end = at + s[at..].find('>').unwrap() + 1;
            format!(
                "{}<p:bg><p:bgPr><a:solidFill><a:srgbClr val=\"FF0000\"/></a:solidFill><a:effectLst/></p:bgPr></p:bg>{}",
                &s[..end],
                &s[end..]
            )
        });
    }
    all.insert("ppt/media/image1.png".into(), png(4, 2, [200, 30, 30]));
    let path = dir.join("deck.pptx");
    std::fs::write(&path, zip_parts(all)).unwrap();
    path
}

/// Every stream of the PDF, inflated when it's Flate-compressed.
fn streams(pdf: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = find(&pdf[at..], b"stream") {
        let start = at + i + b"stream".len();
        if at + i >= 3 && &pdf[at + i - 3..at + i] == b"end" {
            at = start;
            continue;
        }
        let body = start
            + match pdf.get(start..start + 2) {
                Some(b"\r\n") => 2,
                _ => 1,
            };
        let Some(end) = find(&pdf[body..], b"endstream") else {
            break;
        };
        let raw = &pdf[body..body + end];
        let mut inflated = Vec::new();
        let ok = flate2::read::ZlibDecoder::new(raw)
            .read_to_end(&mut inflated)
            .is_ok();
        out.push(if ok && !inflated.is_empty() {
            inflated
        } else {
            raw.to_vec()
        });
        at = body + end;
    }
    out
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn count(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len()).filter(|w| *w == needle).count()
}

/// Code points the ToUnicode CMaps map glyphs to.
fn mapped_chars(pdf: &[u8]) -> String {
    let mut chars = String::new();
    for s in streams(pdf) {
        let text = String::from_utf8_lossy(&s);
        if !text.contains("begincmap") {
            continue;
        }
        for block in text.split("beginbfchar").skip(1) {
            let block = block.split("endbfchar").next().unwrap_or_default();
            for line in block.lines() {
                let Some(hex) = line.split('<').nth(2) else {
                    continue;
                };
                let hex = hex.trim_end_matches(|c: char| c == '>' || c.is_whitespace());
                let units: Vec<u16> = (0..hex.len() / 4)
                    .filter_map(|k| u16::from_str_radix(&hex[k * 4..k * 4 + 4], 16).ok())
                    .collect();
                chars.extend(char::decode_utf16(units).filter_map(Result::ok));
            }
        }
    }
    chars
}

/// PDF objects (and inflated streams) as one searchable text.
fn all_text(pdf: &[u8]) -> String {
    let mut s = String::from_utf8_lossy(pdf).into_owned();
    for st in streams(pdf) {
        s.push_str(&String::from_utf8_lossy(&st));
    }
    s
}

#[test]
fn scene_has_backgrounds_gradients_arrowheads_and_turned_text() {
    let dir = tempfile::tempdir().unwrap();
    let pres = Presentation::open(&deck(dir.path())).unwrap();
    assert!(!pres.slide(0).hidden && pres.slide(1).hidden);
    let mut engine = Engine::new();
    let scene = Builder::new().slide(&mut engine, &pres, 0, &Default::default());
    assert_eq!((scene.width, scene.height), (1280.0, 720.0));
    // The slide's own gradient background comes first.
    let Prim::Fill { paint, .. } = &scene.items[0].prim else {
        panic!("{:?}", scene.items[0].prim)
    };
    let slides_scene::Paint::Linear { from, to, stops } = paint else {
        panic!("{paint:?}")
    };
    assert_eq!(stops.len(), 2);
    assert!(
        (from.y - 0.0).abs() < 0.01 && (to.y - 720.0).abs() < 0.01,
        "top → bottom"
    );
    // The hidden slide has none: its layout's.
    let hidden = Builder::new().slide(&mut engine, &pres, 1, &Default::default());
    assert!(hidden.hidden);
    assert!(
        matches!(
            &hidden.items[0].prim,
            Prim::Fill {
                paint: slides_scene::Paint::Solid(c),
                ..
            } if c.0 == [255, 0, 0, 255]
        ),
        "{:?}",
        hidden.items[0].prim
    );
    assert_eq!(hidden.background, slides_engine::Rgb([255, 0, 0]));
    let radial = scene.items.iter().any(|i| {
        matches!(
            &i.prim,
            Prim::Fill {
                paint: slides_scene::Paint::Radial { stops, .. },
                ..
            } if stops.len() == 3 && stops[1].color.0[3] == 153
        )
    });
    assert!(radial, "path gradient with a translucent middle stop");
    let dashed = scene.items.iter().find_map(|i| match &i.prim {
        Prim::Stroke { stroke, .. } if stroke.dash.is_some() => Some(stroke.clone()),
        _ => None,
    });
    let dashed = dashed.expect("dashed connector");
    assert_eq!(dashed.width, 4.0);
    assert_eq!(dashed.dash.unwrap(), vec![16.0, 12.0], "dash = 4:3 × width");
    // Two arrowheads, filled with the line colour, at the connector's ends.
    let heads: Vec<_> = scene
        .items
        .iter()
        .filter_map(|i| match &i.prim {
            Prim::Fill {
                path,
                paint: slides_scene::Paint::Solid(c),
                ..
            } if c.0 == [0x00, 0xB0, 0x50, 255] => {
                let b = path.transformed(&i.transform).bounds().unwrap();
                Some(((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0))
            }
            _ => None,
        })
        .collect();
    assert_eq!(heads.len(), 2, "{heads:?}");
    // The connector runs from (576, 96) to (864, 192) px.
    let near = |(x, y): (f32, f32), (ex, ey): (f32, f32), tol: f32| {
        (x - ex).abs() < tol && (y - ey).abs() < tol
    };
    assert!(
        near(heads[0], (576.0, 96.0), 1.0),
        "oval on the start: {heads:?}"
    );
    assert!(
        near(heads[1], (864.0, 192.0), 20.0),
        "triangle at the end: {heads:?}"
    );
    // Text turns with its shape (45°) and isn't flipped.
    let text = scene
        .items
        .iter()
        .find(|i| matches!(i.prim, Prim::Text(_)) && i.layer.is_some())
        .unwrap();
    let (a, flip) = text.transform.angle_flip();
    assert!((a.to_degrees() - 45.0).abs() < 0.01 && !flip);
    // The picture: half cropped away, half transparent, flipped.
    let pic = scene
        .items
        .iter()
        .find_map(|i| match &i.prim {
            Prim::Picture(p) => Some((p.clone(), i.transform)),
            _ => None,
        })
        .unwrap();
    assert_eq!(pic.0.crop, [0.5, 0.0, 0.0, 0.0]);
    assert!((pic.0.alpha - 0.5).abs() < 0.01);
    assert_eq!(pic.0.image_rect(), [-192.0, 0.0, 384.0, 192.0]);
    assert!(pic.1.angle_flip().1);
}

#[test]
fn pdf_has_one_page_per_shown_slide_fonts_shadings_and_text() {
    let dir = tempfile::tempdir().unwrap();
    let pres = Presentation::open(&deck(dir.path())).unwrap();
    let mut engine = Engine::new();
    let (pdf, pages) = slides_pdf::export(&pres, &mut engine).unwrap();
    assert_eq!(pages, 1, "the hidden slide is left out");
    let all = all_text(&pdf);
    assert_eq!(
        count(all.as_bytes(), b"/Type/Page") - count(all.as_bytes(), b"/Type/Pages"),
        1
    );
    // 13.333 × 7.5 in = 960 × 540 pt.
    assert!(all.contains("/MediaBox[0 0 960 540]"), "page size");
    // Gradients are shadings: axial (background) and radial (the ellipse).
    assert!(all.contains("/ShadingType 2"), "linear shading");
    assert!(all.contains("/ShadingType 3"), "radial shading");
    // Fonts are embedded with ToUnicode; the turned text can be extracted.
    assert!(
        all.contains("/FontFile2") || all.contains("/FontFile3"),
        "embedded font"
    );
    assert!(all.contains("/ToUnicode"));
    let chars = mapped_chars(&pdf);
    for c in "Turnedwords".chars() {
        assert!(chars.contains(c), "{c} in {chars:?}");
    }
    // A dash pattern and a picture.
    assert!(all.contains("]0 d"), "dash operator");
    assert!(all.contains("/Subtype/Image") || all.contains("/Subtype /Image"));
    // With hidden slides asked for: two pages.
    let d = slides_pdf::deck(&pres, &Builder::new(), &mut engine, true);
    let bytes = slides_pdf::write(&d).unwrap();
    let all = all_text(&bytes);
    assert_eq!(
        count(all.as_bytes(), b"/Type/Page") - count(all.as_bytes(), b"/Type/Pages"),
        2
    );
}
