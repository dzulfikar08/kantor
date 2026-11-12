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

// ── groups, tables, SmartArt ────────────────────────────────────────────

const DIAGRAM_DRAWING: &str =
    "http://schemas.microsoft.com/office/2007/relationships/diagramDrawing";

/// A long cell text in a narrow column: the row has to grow.
const LONG: &str = "many words that wrap over several lines in this narrow cell";

fn frame_shapes() -> String {
    let cell = |t: &str, attrs: &str| {
        format!(
            r#"<a:tc{attrs}><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>{t}</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc>"#
        )
    };
    let row = |cells: String| format!(r#"<a:tr h="370840">{cells}</a:tr>"#);
    let table = format!(
        r#"<a:tbl><a:tblPr firstRow="1" bandRow="1"><a:tableStyleId>{{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}}</a:tableStyleId></a:tblPr><a:tblGrid><a:gridCol w="1828800"/><a:gridCol w="1828800"/><a:gridCol w="1828800"/></a:tblGrid>{}{}{}</a:tbl>"#,
        row(cell("Head", "") + &cell("Hb", "") + &cell("Hc", "")),
        row(cell("Merged", r#" gridSpan="2""#)
            + &cell("", r#" hMerge="1""#)
            + &cell("Tall", r#" rowSpan="2""#)),
        row(cell(LONG, "") + &cell("Body", "") + &cell("", r#" vMerge="1""#)),
    );
    format!(
        r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="30" name="Group"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm flipH="1"><a:off x="914400" y="914400"/><a:ext cx="1828800" cy="914400"/><a:chOff x="0" y="0"/><a:chExt cx="914400" cy="457200"/></a:xfrm><a:solidFill><a:srgbClr val="FF00FF"/></a:solidFill></p:grpSpPr><p:sp><p:nvSpPr><p:cNvPr id="31" name="Child"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="457200" cy="457200"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:grpFill/></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>Grouped</a:t></a:r></a:p></p:txBody></p:sp><p:grpSp><p:nvGrpSpPr><p:cNvPr id="32" name="Inner"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm rot="5400000"><a:off x="457200" y="0"/><a:ext cx="457200" cy="457200"/><a:chOff x="0" y="0"/><a:chExt cx="100" cy="100"/></a:xfrm></p:grpSpPr><p:sp><p:nvSpPr><p:cNvPr id="33" name="Leaf"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="50" cy="50"/></a:xfrm><a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="00FFFF"/></a:solidFill></p:spPr></p:sp></p:grpSp></p:grpSp>
<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="35" name="Table"/><p:cNvGraphicFramePr><a:graphicFrameLocks noGrp="1"/></p:cNvGraphicFramePr><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="914400" y="2743200"/><a:ext cx="5486400" cy="1112520"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table">{table}</a:graphicData></a:graphic></p:graphicFrame>
<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="40" name="Diagram"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="7315200" y="3657600"/><a:ext cx="2743200" cy="1371600"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram"><dgm:relIds xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" r:dm="rIdDm"/></a:graphicData></a:graphic></p:graphicFrame>
<p:sp><p:nvSpPr><p:cNvPr id="41" name="Star"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="7315200" y="914400"/><a:ext cx="1828800" cy="1828800"/></a:xfrm><a:prstGeom prst="star5"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>Star</a:t></a:r></a:p></p:txBody></p:sp>"#
    )
}

/// A one-slide deck with a flipped group (nested turned group, `grpFill`),
/// a styled table with merged cells, SmartArt with its cached drawing and
/// a star with text.
fn frames_deck(dir: &std::path::Path) -> std::path::PathBuf {
    let plain = dir.join("plain.pptx");
    Presentation::new().save_as(&plain).unwrap();
    let mut all = parts(&std::fs::read(&plain).unwrap());
    let edit = |all: &mut BTreeMap<String, Vec<u8>>, k: &str, f: &dyn Fn(String) -> String| {
        let s = String::from_utf8(all[k].clone()).unwrap();
        all.insert(k.to_owned(), f(s).into_bytes());
    };
    edit(&mut all, "ppt/slides/slide1.xml", &|s| {
        s.replace("</p:spTree>", &format!("{}</p:spTree>", frame_shapes()))
    });
    edit(&mut all, "ppt/slides/_rels/slide1.xml.rels", &|s| {
        s.replace(
            "</Relationships>",
            &format!(
                r#"<Relationship Id="rIdDm" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramData" Target="../diagrams/data1.xml"/><Relationship Id="rIdDr" Type="{DIAGRAM_DRAWING}" Target="../diagrams/drawing1.xml"/></Relationships>"#
            ),
        )
    });
    all.insert(
        "ppt/diagrams/data1.xml".into(),
        br#"<dgm:dataModel xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><dgm:ptLst/><dgm:extLst><a:ext uri="http://schemas.microsoft.com/office/drawing/2008/diagram"><dsp:dataModelExt xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram" relId="rIdDr" minVer="http://schemas.openxmlformats.org/drawingml/2006/diagram"/></a:ext></dgm:extLst></dgm:dataModel>"#.to_vec(),
    );
    all.insert(
        "ppt/diagrams/drawing1.xml".into(),
        br#"<dsp:drawing xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><dsp:spTree><dsp:nvGrpSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvGrpSpPr/></dsp:nvGrpSpPr><dsp:grpSpPr/><dsp:sp modelId="{1}"><dsp:nvSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvSpPr/></dsp:nvSpPr><dsp:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1371600" cy="1371600"/></a:xfrm><a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="00FF00"/></a:solidFill></dsp:spPr><dsp:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>Smart</a:t></a:r></a:p></dsp:txBody><dsp:txXfrm><a:off x="200000" y="200000"/><a:ext cx="971600" cy="971600"/></dsp:txXfrm></dsp:sp></dsp:spTree></dsp:drawing>"#.to_vec(),
    );
    let path = dir.join("frames.pptx");
    std::fs::write(&path, zip_parts(all)).unwrap();
    path
}

fn solid_fills(scene: &slides_scene::SlideScene) -> Vec<([u8; 4], slides_engine::Layer)> {
    scene
        .items
        .iter()
        .filter_map(|it| match &it.prim {
            Prim::Fill {
                paint: slides_scene::Paint::Solid(c),
                ..
            } => Some((c.0, it.layer?)),
            _ => None,
        })
        .collect()
}

#[test]
fn groups_tables_and_smartart_are_drawn() {
    use slides_engine::{FrameContent, Layer, ShapeKind};
    let dir = tempfile::tempdir().unwrap();
    let path = frames_deck(dir.path());
    let pres = Presentation::open(&path).unwrap();
    let shapes = &pres.slide(0).shapes;
    let at = |name: &str| shapes.iter().position(|s| s.name == name).unwrap();
    let (group, table, diagram) = (at("Group"), at("Table"), at("Diagram"));
    // Nothing left that Kantor keeps but can't draw.
    assert!(
        pres.unsupported_objects().is_empty(),
        "{:?}",
        pres.unsupported_objects()
    );

    let mut engine = Engine::new();
    let scene = Builder::new().slide(&mut engine, &pres, 0, &Default::default());
    let fills = solid_fills(&scene);
    // The group's children: the grpFill child shows the group's fill, the
    // nested group's ellipse its own; both belong to the group.
    assert!(fills.contains(&([255, 0, 255, 255], Layer::Slide(group))));
    assert!(fills.contains(&([0, 255, 255, 255], Layer::Slide(group))));
    // The flipped group puts its first child on the right half.
    let child = scene
        .items
        .iter()
        .find(|it| matches!(&it.prim, Prim::Fill { paint: slides_scene::Paint::Solid(c), .. } if c.0 == [255, 0, 255, 255]))
        .unwrap();
    let left = child.transform.apply(slides_scene::p(0.0, 0.0)).x;
    let right = child.transform.apply(slides_scene::p(96.0, 0.0)).x;
    assert!((left.min(right) - 192.0).abs() < 0.5, "{left} {right}");

    // Table style (built in: the file only names it): accent 1 header,
    // banded body rows (40 % tint, then 20 %).
    let table_fills: Vec<[u8; 4]> = fills
        .iter()
        .filter(|f| f.1 == Layer::Slide(table))
        .map(|f| f.0)
        .collect();
    assert_eq!(table_fills.len(), 7, "{table_fills:?}");
    assert_eq!(table_fills[0], [0x44, 0x72, 0xC4, 255]);
    assert!(
        table_fills.contains(&[180, 198, 231, 255]),
        "{table_fills:?}"
    );
    assert!(
        table_fills.contains(&[217, 226, 243, 255]),
        "{table_fills:?}"
    );
    // Borders: white lines between cells.
    assert!(scene.items.iter().any(|it| it.layer == Some(Layer::Slide(table))
        && matches!(&it.prim, Prim::Stroke { stroke, .. } if stroke.color.0 == [255, 255, 255, 255])));
    // The long cell's row grew: the table is taller than its rows say.
    let bottom = scene
        .items
        .iter()
        .filter(|it| it.layer == Some(Layer::Slide(table)))
        .filter_map(|it| match &it.prim {
            Prim::Fill { path, .. } => {
                let b = path.transformed(&it.transform).bounds()?;
                Some(b[1] + b[3])
            }
            _ => None,
        })
        .fold(0.0f32, f32::max);
    assert!(bottom > 288.0 + 3.0 * 38.93 + 10.0, "{bottom}");
    // The merged cells' text is there once each.
    let texts = |layer| {
        scene
            .items
            .iter()
            .filter(|it| it.layer == Some(layer))
            .filter_map(|it| match &it.prim {
                Prim::Text(t) => Some(
                    t.runs
                        .iter()
                        .map(|r| r.sample().to_owned())
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let cells = texts(Layer::Slide(table));
    assert_eq!(
        cells.iter().filter(|t| t.contains("Merged")).count(),
        1,
        "{cells:?}"
    );
    assert_eq!(cells.len(), 7, "{cells:?}");

    // SmartArt: the cached drawing's shape and its text box.
    let ShapeKind::Other { frame, .. } = &shapes[diagram].kind else {
        panic!()
    };
    let Some(FrameContent::Diagram(d)) = frame.get() else {
        panic!("{frame:?}")
    };
    let drawing = pres.diagram_drawing(0, d).expect("drawing part");
    assert_eq!(drawing.children.len(), 1);
    assert!(drawing.text_boxes[0].is_some());
    assert!(fills.contains(&([0, 255, 0, 255], Layer::Slide(diagram))));
    assert_eq!(texts(Layer::Slide(diagram)).concat(), "Smart");

    // Text in a star goes in its text rectangle, inside the default insets.
    let star = at("Star");
    let list = pres.render_list(0, false);
    let r = list.iter().find(|r| r.layer == Layer::Slide(star)).unwrap();
    let ins = r.text.as_ref().unwrap().insets;
    assert!(
        ins[0] > 91_440 + 300_000 && ins[1] > 45_720 + 300_000,
        "{ins:?}"
    );

    // Saved unchanged, the slide is byte for byte the same.
    let before = parts(&std::fs::read(&path).unwrap());
    let after = parts(&pres.to_bytes().unwrap());
    for k in [
        "ppt/slides/slide1.xml",
        "ppt/diagrams/drawing1.xml",
        "ppt/diagrams/data1.xml",
    ] {
        assert_eq!(before[k], after[k], "{k}");
    }
}

#[test]
fn pdf_has_table_group_and_smartart_text() {
    let dir = tempfile::tempdir().unwrap();
    let pres = Presentation::open(&frames_deck(dir.path())).unwrap();
    let mut engine = Engine::new();
    let (pdf, pages) = slides_pdf::export(&pres, &mut engine).unwrap();
    assert_eq!(pages, 1);
    let chars = mapped_chars(&pdf);
    for c in "HeadMergedTallBodyGroupedSmartStar".chars() {
        assert!(chars.contains(c), "{c} in {chars:?}");
    }
}

// ── print layouts ───────────────────────────────────────────────────────

fn page_count(pdf: &[u8]) -> usize {
    let all = all_text(pdf);
    count(all.as_bytes(), b"/Type/Page") - count(all.as_bytes(), b"/Type/Pages")
}

#[test]
fn notes_pages_and_handouts_print_on_a4() {
    use slides_pdf::{PrintLayout, print};
    let mut pres = Presentation::new();
    for _ in 0..4 {
        pres.new_slide(None).unwrap();
    }
    assert_eq!(pres.len(), 5);
    pres.set_notes_text(0, "Mention Quokka budget").unwrap();
    let builder = Builder::new();
    let mut engine = Engine::new();

    let job = print::job(&pres, &builder, &mut engine, PrintLayout::Notes);
    assert_eq!(job.pages(), 5);
    assert!(!job.notes[0].is_empty() && job.notes[1].is_empty());
    let (pdf, pages) = print::write_job(&job).unwrap();
    assert_eq!((pages, page_count(&pdf)), (5, 5));
    // A4 is 595.28 × 841.89 pt.
    let all = all_text(&pdf);
    assert!(all.contains("/MediaBox[0 0 595.2"), "A4 page");
    let chars = mapped_chars(&pdf);
    for c in "MentionQukabdg".chars() {
        assert!(chars.contains(c), "{c} in {chars:?}");
    }

    for (n, want) in [(1, 5), (2, 3), (3, 2), (4, 2), (6, 1), (9, 1)] {
        let job = print::job(&pres, &builder, &mut engine, PrintLayout::Handouts(n));
        let (pdf, pages) = print::write_job(&job).unwrap();
        assert_eq!((pages, page_count(&pdf)), (want, want), "{n} per page");
    }

    // Full-page slides are the plain export.
    let job = print::job(&pres, &builder, &mut engine, PrintLayout::Slides);
    let (pdf, pages) = print::write_job(&job).unwrap();
    assert_eq!((pages, page_count(&pdf)), (5, 5));
}

#[test]
fn edited_chart_data_and_table_text_are_drawn() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/pptx/deck.pptx");
    let mut pres = Presentation::open(&path).unwrap();
    let find = |p: &Presentation, chart: bool| {
        (0..p.len())
            .find_map(|i| {
                (0..p.slide(i).shapes.len())
                    .find(|&k| {
                        if chart {
                            p.chart_data(i, k).is_some()
                        } else {
                            p.table(i, k).is_some()
                        }
                    })
                    .map(|k| (i, k))
            })
            .unwrap()
    };
    let (s, k) = find(&pres, true);
    let mut d = pres.chart_data(s, k).unwrap();
    d.categories.push("Zq".into());
    for ser in &mut d.series {
        ser.values.push(Some(30.0));
    }
    pres.set_chart_data(s, k, &d).unwrap();
    let (ts, tk) = find(&pres, false);
    pres.edit_table(
        ts,
        tk,
        &[slides_engine::TableOp::Text {
            row: 1,
            col: 0,
            text: "Kalimantan".into(),
        }],
    )
    .unwrap();
    let mut engine = Engine::new();
    let (pdf, _) = slides_pdf::export(&pres, &mut engine).unwrap();
    let chars = mapped_chars(&pdf);
    for c in "ZqKlimntn".chars() {
        assert!(chars.contains(c), "{c} in {chars:?}");
    }
}
