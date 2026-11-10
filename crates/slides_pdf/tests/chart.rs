//! A chart generated here: drawn in the scene (no placeholder), its labels
//! written to the PDF as extractable text, and its part kept byte for
//! byte when the presentation is saved.

use std::collections::BTreeMap;
use std::io::{Cursor, Read as _, Write as _};

use slides_engine::Presentation;
use slides_scene::{Builder, Engine, Prim};

const CHART: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><c:chart><c:autoTitleDeleted val="1"/><c:plotArea><c:layout/><c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:varyColors val="0"/><c:ser><c:idx val="0"/><c:order val="0"/><c:tx><c:strRef><c:f>Sheet1!$B$1</c:f><c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>Wombat</c:v></c:pt></c:strCache></c:strRef></c:tx><c:dLbls><c:numFmt formatCode="0.0" sourceLinked="0"/><c:showLegendKey val="0"/><c:showVal val="1"/><c:showCatName val="0"/><c:showSerName val="0"/><c:showPercent val="0"/><c:showBubbleSize val="0"/></c:dLbls><c:cat><c:strRef><c:f>Sheet1!$A$2:$A$3</c:f><c:strCache><c:ptCount val="2"/><c:pt idx="0"><c:v>Quokka</c:v></c:pt><c:pt idx="1"><c:v>Zebra</c:v></c:pt></c:strCache></c:strRef></c:cat><c:val><c:numRef><c:f>Sheet1!$B$2:$B$3</c:f><c:numCache><c:formatCode>General</c:formatCode><c:ptCount val="2"/><c:pt idx="0"><c:v>7.5</c:v></c:pt><c:pt idx="1"><c:v>9</c:v></c:pt></c:numCache></c:numRef></c:val></c:ser><c:axId val="-11"/><c:axId val="-12"/></c:barChart><c:catAx><c:axId val="-11"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="b"/><c:crossAx val="-12"/></c:catAx><c:valAx><c:axId val="-12"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="l"/><c:majorGridlines/><c:crossAx val="-11"/><c:crossBetween val="between"/></c:valAx></c:plotArea><c:legend><c:legendPos val="b"/></c:legend></c:chart><c:externalData r:id="rId1"><c:autoUpdate val="0"/></c:externalData></c:chartSpace>"#;

const FRAME: &str = r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="30" name="Chart 1"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="914400" y="914400"/><a:ext cx="7315200" cy="4572000"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" r:id="rIdChart"/></a:graphicData></a:graphic></p:graphicFrame>"#;

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

/// A one-slide deck with a column chart.
fn deck(dir: &std::path::Path) -> std::path::PathBuf {
    let mut p = Presentation::new();
    let plain = dir.join("plain.pptx");
    p.save_as(&plain).unwrap();
    let mut all = parts(&std::fs::read(&plain).unwrap());
    let edit = |all: &mut BTreeMap<String, Vec<u8>>, k: &str, f: &dyn Fn(String) -> String| {
        let s = String::from_utf8(all[k].clone()).unwrap();
        all.insert(k.to_owned(), f(s).into_bytes());
    };
    edit(&mut all, "ppt/slides/slide1.xml", &|s| {
        s.replace("</p:spTree>", &format!("{FRAME}</p:spTree>"))
    });
    edit(&mut all, "ppt/slides/_rels/slide1.xml.rels", &|s| {
        s.replace(
            "</Relationships>",
            r#"<Relationship Id="rIdChart" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="../charts/chart1.xml"/></Relationships>"#,
        )
    });
    edit(&mut all, "[Content_Types].xml", &|s| {
        s.replace(
            "</Types>",
            r#"<Override PartName="/ppt/charts/chart1.xml" ContentType="application/vnd.openxmlformats-officedocument.drawingml.chart+xml"/></Types>"#,
        )
    });
    all.insert("ppt/charts/chart1.xml".into(), CHART.as_bytes().to_vec());
    let path = dir.join("chart.pptx");
    std::fs::write(&path, zip_parts(all)).unwrap();
    path
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
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

#[test]
fn chart_frames_are_drawn_with_text_labels() {
    let dir = tempfile::tempdir().unwrap();
    let pres = Presentation::open(&deck(dir.path())).unwrap();
    let frame = pres
        .slide(0)
        .shapes
        .iter()
        .find(|s| s.name == "Chart 1")
        .unwrap();
    assert_eq!(frame.chart_part(), Some("ppt/charts/chart1.xml"));
    // Drawn charts aren't listed as objects Kantor can't show.
    assert!(pres.unsupported_objects().is_empty());
    let mut engine = Engine::new();
    let scene = Builder::new().slide(&mut engine, &pres, 0, &Default::default());
    assert!(
        !scene
            .items
            .iter()
            .any(|i| matches!(i.prim, Prim::Unsupported { .. })),
        "no placeholder"
    );
    let words: Vec<String> = scene
        .items
        .iter()
        .filter_map(|i| match &i.prim {
            Prim::Text(t) => Some(t.runs.iter().map(|r| r.sample()).collect::<String>()),
            _ => None,
        })
        .collect();
    for w in ["Quokka", "Zebra", "Wombat", "7.5", "9.0"] {
        assert!(words.iter().any(|t| t == w), "{w} in {words:?}");
    }
    // Inside the frame (1 in = 96 px from the slide's corner, 8 × 5 in).
    for i in scene.items.iter().filter(|i| i.layer.is_some()) {
        let o = i.transform.apply(slides_scene::p(0.0, 0.0));
        assert!(
            o.x >= 95.0 && o.x <= 96.0 + 768.0 && o.y >= 95.0 && o.y <= 96.0 + 480.0,
            "{o:?}"
        );
    }

    let (pdf, _) = slides_pdf::export(&pres, &mut engine).unwrap();
    let chars = mapped_chars(&pdf);
    for c in "QuokaZebrWm7.59".chars() {
        assert!(chars.contains(c), "{c} in {chars:?}");
    }
}

#[test]
fn chart_parts_are_kept_on_save() {
    let dir = tempfile::tempdir().unwrap();
    let src = deck(dir.path());
    let mut pres = Presentation::open(&src).unwrap();
    // Draw it (charts are read-only), then save elsewhere.
    let mut engine = Engine::new();
    Builder::new().slide(&mut engine, &pres, 0, &Default::default());
    let out = dir.path().join("saved.pptx");
    pres.save_as(&out).unwrap();
    let before = parts(&std::fs::read(&src).unwrap());
    let after = parts(&std::fs::read(&out).unwrap());
    assert_eq!(
        after["ppt/charts/chart1.xml"],
        before["ppt/charts/chart1.xml"]
    );
    let slide = String::from_utf8(after["ppt/slides/slide1.xml"].clone()).unwrap();
    assert!(slide.contains(FRAME), "the frame as read");
}
