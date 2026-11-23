//! OpenDocument presentation import: the synthetic decks in
//! `tests/corpus/odp` (made by `make_fixtures.py`: python-pptx decks
//! converted by LibreOffice) and small hand-written packages open with
//! their text, formatting, shapes, pictures, tables, notes, masters and
//! slide size, and damaged files fail cleanly.

use std::io::{Cursor, Read as _, Write as _};
use std::path::{Path, PathBuf};

use slides_engine::frame::FrameContent;
use slides_engine::*;

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/odp")
        .join(name)
}

fn open(name: &str) -> Presentation {
    let p = Presentation::open(&corpus(name)).unwrap();
    if let Some(dir) = std::env::var_os("KANTOR_ODP_DUMP") {
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
        .filter(|t| !t.is_empty())
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
    // LibreOffice keeps 1/100 mm.
    (a - b).abs() <= 1000
}

fn assert_valid(bytes: &[u8]) {
    let problems = package_problems(bytes);
    assert!(problems.is_empty(), "{problems:#?}");
}

fn part_text(bytes: &[u8], name: &str) -> String {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut s = String::new();
    zip.by_name(name).unwrap().read_to_string(&mut s).unwrap();
    s
}

#[test]
fn opens_as_an_unsaved_import() {
    let p = open("basic.odp");
    assert_valid(&p.to_bytes().unwrap());
    assert!(p.is_imported());
    assert_eq!(p.path(), None);
    assert_eq!(p.source_path(), Some(corpus("basic.odp").as_path()));
    assert_eq!(p.title(), "basic.odp");
    assert_eq!(p.suggested_file_name(), "basic.pptx");
    assert!(p.needs_save_as());
    assert!(!p.is_dirty());
    assert_eq!(p.len(), 7);
    // python-pptx's template is 4:3.
    assert_eq!((p.size.cx, p.size.cy), (9_144_000, 6_858_000));
    assert!(p.import_notes().is_empty(), "{:?}", p.import_notes());
}

#[test]
fn slide_size_background_and_templates() {
    for name in ["wide.odp", "wide.otp"] {
        let p = open(name);
        assert_eq!((p.size.cx, p.size.cy), (12_192_000, 6_858_000), "{name}");
        let bg = p.slide(0).bg.or(p.masters[0].bg);
        assert_eq!(bg.map(|c| c.hex()), Some("FFF4D6".into()), "{name}");
        let t = shape_with(&p, 0, "Widescreen");
        let run = &t.text.as_ref().unwrap().paras[0].runs[0];
        assert_eq!(run.props.size, Some(4000));
        let x = t.xfrm.unwrap();
        assert!(near(x.x, 2 * CM) && near(x.y, 2 * CM), "{x:?}");
    }
    assert_eq!(open("wide.otp").suggested_file_name(), "wide.pptx");
}

#[test]
fn text_and_formatting() {
    let p = open("basic.odp");
    assert_eq!(
        texts(&p, 0),
        ["Quarterly Review", "Prepared with Kantor fixtures"]
    );
    let title = &p.slide(0).shapes[0];
    assert_eq!(
        title.ph.as_ref().and_then(|ph| ph.kind.as_deref()),
        Some("ctrTitle")
    );
    let sub = shape_with(&p, 0, "Prepared");
    assert_eq!(
        sub.ph.as_ref().and_then(|ph| ph.kind.as_deref()),
        Some("subTitle")
    );
    let body = shape_with(&p, 1, "First level bullet");
    assert!(body.ph.is_some());
    let paras = &body.text.as_ref().unwrap().paras;
    let levels: Vec<u8> = paras.iter().map(|p| p.props.level).collect();
    assert_eq!(levels, [0, 1, 0, 0]);
    let last = &paras[2];
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
    // Bullets per level, numbering, and the link.
    assert!(xml.contains("<a:buChar"), "{xml}");
    assert!(xml.contains("<a:buAutoNum type=\"arabicPeriod\""), "{xml}");
    assert!(xml.contains("<a:hlinkClick r:id="), "{xml}");
    let rels = part_text(&bytes, "ppt/slides/_rels/slide2.xml.rels");
    assert!(
        rels.contains("https://kantor.paparanai.id/") && rels.contains("TargetMode=\"External\""),
        "{rels}"
    );
}

#[test]
fn unicode_and_alignment() {
    let p = open("basic.odp");
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
    let p = open("basic.odp");
    let s = &p.slide(2).shapes;
    let geom = |i: usize| match &s[i].kind {
        ShapeKind::Shape { geom, .. } => geom.clone(),
        k => panic!("{k:?}"),
    };
    assert_eq!(geom(0), Geom::Rect);
    assert_eq!(geom(1), Geom::RoundRect);
    assert_eq!(geom(2), Geom::Ellipse);
    assert_eq!(geom(3), Geom::Other("rightArrow".into()));
    assert_eq!(geom(4), Geom::Other("star5".into()));
    assert_eq!(geom(5), Geom::Line);
    assert_eq!(geom(6), Geom::Other("rtTriangle".into()));

    let rect = s[0].xfrm.unwrap();
    assert!(near(rect.x, CM) && near(rect.y, CM), "{rect:?}");
    assert!(near(rect.cx, 6 * CM) && near(rect.cy, 3 * CM), "{rect:?}");
    assert_eq!(s[0].look.fill.map(|c| c.hex()), Some("208040".into()));
    let (line_color, line_w) = s[0].look.line.unwrap();
    assert_eq!(line_color.hex(), "000080");
    assert!((line_w - 38_100).abs() < 400, "{line_w}");
    assert_eq!(texts(&p, 2), ["Green box"]);

    let arrow = s[3].xfrm.unwrap();
    assert!((arrow.rot - 30 * 60_000).abs() < 100, "{arrow:?}");
    assert!(near(arrow.x, 7 * CM) && near(arrow.cx, 6 * CM), "{arrow:?}");
    assert!(s[6].xfrm.unwrap().flip_h);
    let line = s[5].xfrm.unwrap();
    assert!(near(line.x, CM) && near(line.y, 12 * CM), "{line:?}");
    assert!(near(line.cx, 11 * CM) && near(line.cy, 4 * CM), "{line:?}");
    // The group stays a group.
    assert_eq!(s[7].group().map(|g| g.children.len()), Some(2));

    let bytes = p.to_bytes().unwrap();
    let xml = part_text(&bytes, "ppt/slides/slide3.xml");
    // Gradient, dashed line with an arrowhead.
    assert!(xml.contains("<a:gradFill"), "{xml}");
    assert!(xml.contains("<a:prstDash"), "{xml}");
    assert!(xml.contains("<a:tailEnd type=\"triangle\""), "{xml}");
}

#[test]
fn pictures() {
    let p = open("basic.odp");
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
}

#[test]
fn tables_become_tables() {
    let p = open("basic.odp");
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
    assert_eq!(cell(2, 0), "Merged");
    assert!(table.rows[0].cells[0].fill.is_some());
    let bytes = p.to_bytes().unwrap();
    let xml = part_text(&bytes, "ppt/slides/slide5.xml");
    assert!(
        xml.contains("gridSpan=\"2\"") && xml.contains("hMerge=\"1\""),
        "{xml}"
    );
}

#[test]
fn speaker_notes_and_hidden_slides() {
    let p = open("basic.odp");
    assert_eq!(p.notes_text(0), "Welcome everyone.");
    assert_eq!(p.notes_text(1), "Talk about formatting.");
    assert_eq!(p.notes_text(2), "");
    assert_eq!(p.notes_text(5), "Catatan pembicara: terima kasih");
    let hidden: Vec<bool> = p.slides().iter().map(|s| s.hidden).collect();
    assert_eq!(hidden, [false, false, false, false, false, false, true]);
}

#[test]
fn save_writes_a_new_pptx() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("deck.odp");
    std::fs::copy(corpus("basic.odp"), &src).unwrap();
    let before = std::fs::read(&src).unwrap();
    let mut p = Presentation::open(&src).unwrap();
    assert!(p.save().is_err());
    let out = dir.path().join("deck.pptx");
    p.save_as(&out).unwrap();
    assert_eq!(
        std::fs::read(&src).unwrap(),
        before,
        "the .odp is untouched"
    );
    assert!(!p.is_imported());
    let q = Presentation::open(&out).unwrap();
    assert_eq!(q.len(), 7);
    assert_eq!(texts(&q, 0), texts(&p, 0));
    assert_eq!(q.notes_text(0), "Welcome everyone.");
}

#[test]
fn found_by_content_and_extension() {
    let bytes = std::fs::read(corpus("basic.odp")).unwrap();
    assert!(odp::is_odp(&bytes));
    assert!(odp::is_odp(&std::fs::read(corpus("wide.otp")).unwrap()));
    assert!(!odp::is_odp(b"PK\x03\x04 not really"));
    assert!(odp::is_odp_path(Path::new("a/Deck.ODP")));
    assert!(odp::is_odp_path(Path::new("t.otp")));
    assert!(!odp::is_odp_path(Path::new("t.ods")));
    // An .odp under another name is still read as one.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deck.pptx");
    std::fs::write(&path, &bytes).unwrap();
    let p = Presentation::open(&path).unwrap();
    assert!(p.is_imported());
    assert_eq!(p.len(), 7);
}

#[test]
fn damaged_files_give_errors() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = std::fs::read(corpus("basic.odp")).unwrap();
    for (name, data) in [
        ("empty.odp", Vec::new()),
        ("text.odp", b"just some text".to_vec()),
        ("truncated.odp", bytes[..bytes.len() / 3].to_vec()),
        ("header-only.odp", bytes[..100].to_vec()),
    ] {
        let path = dir.path().join(name);
        std::fs::write(&path, data).unwrap();
        let err = format!("{:#}", Presentation::open(&path).err().expect(name));
        assert!(err.contains("can't be opened"), "{name}: {err}");
    }
    // A spreadsheet named .odp.
    let ods = package(
        "application/vnd.oasis.opendocument.spreadsheet",
        "<office:document-content xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\"/>",
        "",
        &[],
    );
    let err = odp::convert(&ods).err().unwrap().to_string();
    assert!(err.contains("not an OpenDocument presentation"), "{err}");
    // Password-protected.
    let enc = package(
        MIME,
        "<x/>",
        "",
        &[(
            "META-INF/manifest.xml",
            b"<manifest:manifest><manifest:encryption-data/></manifest:manifest>".to_vec(),
        )],
    );
    let err = odp::convert(&enc).err().unwrap().to_string();
    assert!(err.contains("password-protected"), "{err}");
    // Bad XML.
    let bad = package(MIME, "<office:document-content", "", &[]);
    assert!(odp::convert(&bad).is_err());
}

// ── hand-written packages ──────────────────────────────────────────────

const MIME: &str = "application/vnd.oasis.opendocument.presentation";

const NSS: &str = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:smil="urn:oasis:names:tc:opendocument:xmlns:smil-compatible:1.0" xmlns:anim="urn:oasis:names:tc:opendocument:xmlns:animation:1.0""#;

/// A package: `mimetype` first and stored, then the parts.
fn package(mime: &str, content: &str, styles: &str, extra: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    z.start_file("mimetype", stored).unwrap();
    z.write_all(mime.as_bytes()).unwrap();
    z.start_file("content.xml", stored).unwrap();
    z.write_all(content.as_bytes()).unwrap();
    if !styles.is_empty() {
        z.start_file("styles.xml", stored).unwrap();
        z.write_all(styles.as_bytes()).unwrap();
    }
    for (name, data) in extra {
        z.start_file(*name, stored).unwrap();
        z.write_all(data).unwrap();
    }
    z.finish().unwrap().into_inner()
}

/// A native LibreOffice-style deck: master with outline list styles,
/// background and a page number frame; slides using them.
fn native() -> Vec<u8> {
    let styles = format!(
        r##"<office:document-styles {NSS}><office:styles>
<draw:hatch draw:name="H" draw:style="single" draw:color="#ff0000" draw:distance="0.1cm" draw:rotation="450"/>
<style:default-style style:family="graphic"><style:text-properties fo:font-size="18pt" style:font-name="Liberation Sans"/></style:default-style>
<style:style style:name="M-title" style:family="presentation"><style:paragraph-properties fo:text-align="center"/><style:text-properties fo:font-size="40pt" fo:color="#123456" fo:font-family="Georgia"/></style:style>
<style:style style:name="M-subtitle" style:family="presentation"><style:text-properties fo:font-size="24pt"/></style:style>
<style:style style:name="M-outline1" style:family="presentation"><style:graphic-properties><text:list-style style:name="M-outline1">
<text:list-level-style-bullet text:level="1" text:bullet-char="●"><style:list-level-properties text:space-before="0.3cm" text:min-label-width="0.9cm"/><style:text-properties fo:font-family="OpenSymbol" fo:font-size="45%"/></text:list-level-style-bullet>
<text:list-level-style-number text:level="2" style:num-format="a" style:num-suffix=")"><style:list-level-properties text:space-before="1.5cm" text:min-label-width="0.9cm"/></text:list-level-style-number>
</text:list-style></style:graphic-properties><style:text-properties fo:font-size="32pt" fo:color="#202020"/></style:style>
<style:style style:name="M-outline2" style:family="presentation" style:parent-style-name="M-outline1"><style:text-properties fo:font-size="28pt"/></style:style>
</office:styles><office:automatic-styles>
<style:page-layout style:name="PM1"><style:page-layout-properties fo:page-width="28cm" fo:page-height="15.75cm"/></style:page-layout>
<style:style style:name="Mdp1" style:family="drawing-page"><style:drawing-page-properties draw:fill="solid" draw:fill-color="#e0f0ff"/></style:style>
<style:style style:name="Mgr1" style:family="graphic"><style:graphic-properties draw:fill="hatch" draw:fill-hatch-name="H" draw:stroke="none"/></style:style>
</office:automatic-styles><office:master-styles>
<style:master-page style:name="M" style:page-layout-name="PM1" draw:style-name="Mdp1">
<draw:rect draw:style-name="Mgr1" svg:x="0cm" svg:y="14cm" svg:width="28cm" svg:height="1.75cm"/>
<draw:frame presentation:style-name="M-title" svg:x="1cm" svg:y="1cm" svg:width="26cm" svg:height="3cm" presentation:class="title" presentation:placeholder="true"><draw:text-box/></draw:frame>
<draw:frame presentation:style-name="M-outline1" svg:x="1cm" svg:y="4.5cm" svg:width="26cm" svg:height="9cm" presentation:class="outline" presentation:placeholder="true"><draw:text-box/></draw:frame>
<draw:frame svg:x="24cm" svg:y="14.5cm" svg:width="3cm" svg:height="1cm" presentation:class="page-number"><draw:text-box><text:p><text:page-number>&lt;number&gt;</text:page-number></text:p></draw:text-box></draw:frame>
<draw:frame svg:x="8cm" svg:y="14.5cm" svg:width="12cm" svg:height="1cm" presentation:class="footer"><draw:text-box><text:p><presentation:footer/></text:p></draw:text-box></draw:frame>
</style:master-page></office:master-styles></office:document-styles>"##
    );
    let content = format!(
        r##"<office:document-content {NSS}><office:automatic-styles>
<style:style style:name="dp1" style:family="drawing-page"><style:drawing-page-properties presentation:display-page-number="true" presentation:display-footer="true"/></style:style>
<style:style style:name="dp2" style:family="drawing-page"><style:drawing-page-properties presentation:visibility="hidden" draw:fill="solid" draw:fill-color="#000000" smil:type="fade"/></style:style>
<style:style style:name="gr1" style:family="graphic"><style:graphic-properties draw:fill="gradient" draw:fill-gradient-name="G" draw:stroke="dash" svg:stroke-color="#00ff00" draw:shadow="visible" draw:shadow-offset-x="0.2cm" draw:shadow-offset-y="0.2cm"/></style:style>
<style:style style:name="T1" style:family="text"><style:text-properties fo:font-weight="bold" style:text-position="super 58%"/></style:style>
</office:automatic-styles><office:body><office:presentation>
<presentation:footer-decl presentation:name="ftr1">Company confidential</presentation:footer-decl>
<draw:page draw:name="Intro" draw:style-name="dp1" draw:master-page-name="M" presentation:use-footer-name="ftr1">
<draw:frame presentation:style-name="M-title" svg:x="1cm" svg:y="1cm" svg:width="26cm" svg:height="3cm" presentation:class="title"><draw:text-box><text:p>Native <text:span text:style-name="T1">deck</text:span></text:p></draw:text-box></draw:frame>
<draw:frame presentation:style-name="M-outline1" svg:x="1cm" svg:y="4.5cm" svg:width="26cm" svg:height="9cm" presentation:class="outline"><draw:text-box>
<text:list><text:list-item><text:p>Point one</text:p><text:list><text:list-item><text:p>Sub point</text:p></text:list-item></text:list></text:list-item>
<text:list-header><text:p>No bullet</text:p></text:list-header></text:list>
</draw:text-box></draw:frame>
<draw:custom-shape draw:style-name="gr1" svg:x="2cm" svg:y="2cm" svg:width="4cm" svg:height="2cm" draw:transform="rotate (-1.5707963267949) translate (2cm 6cm)"><text:p>Turned</text:p>
<draw:enhanced-geometry svg:viewBox="0 0 21600 21600" draw:type="mso-spt100" draw:enhanced-path="M 0 0 L 21600 10800 0 21600 Z N"/></draw:custom-shape>
<draw:frame draw:name="Chart 1" svg:x="10cm" svg:y="5cm" svg:width="8cm" svg:height="6cm"><draw:object xlink:href="./Object 1"/><draw:image xlink:href="./ObjectReplacements/Object 1"/></draw:frame>
<presentation:notes><draw:frame presentation:class="notes" svg:x="1cm" svg:y="1cm" svg:width="10cm" svg:height="10cm"><draw:text-box><text:p>Native notes</text:p></draw:text-box></draw:frame></presentation:notes>
</draw:page>
<draw:page draw:name="page2" draw:style-name="dp2" draw:master-page-name="M">
<draw:frame presentation:style-name="M-title" svg:x="1cm" svg:y="1cm" svg:width="26cm" svg:height="3cm" presentation:class="title" presentation:placeholder="true"><draw:text-box/></draw:frame>
<draw:line svg:x1="1cm" svg:y1="1cm" svg:x2="5cm" svg:y2="3cm"/>
</draw:page>
</office:presentation></office:body></office:document-content>"##
    );
    let styles = styles.replace(
        "<office:styles>",
        "<office:styles><draw:gradient draw:name=\"G\" draw:style=\"radial\" draw:start-color=\"#ffffff\" draw:end-color=\"#0000ff\"/>",
    );
    package(
        MIME,
        &content,
        &styles,
        &[
            (
                "Object 1/content.xml",
                b"<office:document-content><office:body><office:chart><chart:chart/></office:chart></office:body></office:document-content>".to_vec(),
            ),
            ("ObjectReplacements/Object 1", b"VCLMTF svm picture".to_vec()),
        ],
    )
}

#[test]
fn native_masters_lists_and_decorations() {
    let imported = odp::convert(&native()).unwrap();
    assert_valid(&imported.pptx);
    assert_eq!(
        imported.notes,
        [
            "1 chart left out",
            "slide transitions and animations were left out"
        ]
    );
    let p = Presentation::from_bytes(imported.pptx.clone()).unwrap();
    assert_eq!(p.len(), 2);
    assert_eq!((p.size.cx, p.size.cy), (10_080_000, 5_670_000));
    assert_eq!(p.masters[0].bg.map(|c| c.hex()), Some("E0F0FF".into()));
    assert!(p.slides()[1].hidden);
    assert!(!p.slides()[0].hidden);
    assert_eq!(p.slide(1).bg.map(|c| c.hex()), Some("000000".into()));
    assert_eq!(p.notes_text(0), "Native notes");

    let slide = part_text(&imported.pptx, "ppt/slides/slide1.xml");
    // Title placeholder, its master style's size, colour and font written
    // out; the superscript bold run.
    assert!(slide.contains("<p:ph type=\"title\"/>"), "{slide}");
    assert!(slide.contains("sz=\"4000\""), "{slide}");
    assert!(slide.contains("val=\"123456\""), "{slide}");
    assert!(slide.contains("typeface=\"Georgia\""), "{slide}");
    assert!(
        slide.contains("b=\"1\"") && slide.contains("baseline=\"30000\""),
        "{slide}"
    );
    // Outline levels: bullet, then "a)" numbering at level 2 with its
    // size, and a list header without a bullet.
    assert!(slide.contains("<a:buChar char=\"●\"/>"), "{slide}");
    assert!(
        slide.contains("<a:buAutoNum type=\"alphaLcParenR\"/>"),
        "{slide}"
    );
    assert!(slide.contains("sz=\"2800\""), "{slide}");
    let body = shape_with(&p, 0, "Point one");
    let paras = &body.text.as_ref().unwrap().paras;
    let levels: Vec<u8> = paras.iter().map(|p| p.props.level).collect();
    assert_eq!(levels, [0, 1, 0]);
    // marL = space before + label width; hanging by the label width.
    assert!(
        slide.contains("marL=\"432000\" indent=\"-324000\""),
        "{slide}"
    );
    // The custom shape: its own path, turned 90°, gradient, dashed
    // outline, shadow.
    assert!(slide.contains("<a:custGeom>"), "{slide}");
    assert!(slide.contains("rot=\"5400000\""), "{slide}");
    assert!(slide.contains("<a:path path=\"circle\">"), "{slide}");
    assert!(slide.contains("<a:prstDash"), "{slide}");
    assert!(slide.contains("<a:outerShdw"), "{slide}");
    // Slide number and footer from the master, as the slide asks.
    assert!(slide.contains("type=\"sldNum\""), "{slide}");
    assert!(slide.contains("type=\"slidenum\"><a:rPr"), "{slide}");
    assert!(slide.contains("Company confidential"), "{slide}");
    let turned = shape_with(&p, 0, "Turned").xfrm.unwrap();
    assert!(near(turned.x, -CM) && near(turned.y, 7 * CM), "{turned:?}");

    // The master: hatched background object as a pattern, text styles.
    let master = part_text(&imported.pptx, "ppt/slideMasters/slideMaster1.xml");
    assert!(master.contains("<a:pattFill prst="), "{master}");
    assert!(
        master.contains("<p:titleStyle><a:lvl1pPr algn=\"ctr\""),
        "{master}"
    );
    assert!(master.contains("<a:lvl2pPr"), "{master}");
    // Theme fonts from the title and outline styles.
    let theme = part_text(&imported.pptx, "ppt/theme/theme1.xml");
    assert!(
        theme.contains("<a:majorFont><a:latin typeface=\"Georgia\""),
        "{theme}"
    );

    // The hidden slide: an empty title placeholder and a line; no slide
    // number (it doesn't ask).
    let s2 = part_text(&imported.pptx, "ppt/slides/slide2.xml");
    assert!(s2.contains("show=\"0\""), "{s2}");
    assert!(s2.contains("<p:ph type=\"title\"/>"), "{s2}");
    assert!(s2.contains("<p:cxnSp>"), "{s2}");
    assert!(!s2.contains("sldNum"), "{s2}");
}

#[test]
fn svg_pictures_get_a_png_copy() {
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20" fill="#ff0000"/></svg>"##;
    let content = format!(
        r#"<office:document-content {NSS}><office:body><office:presentation>
<draw:page draw:name="p"><draw:frame svg:x="1cm" svg:y="1cm" svg:width="4cm" svg:height="2cm"><draw:image xlink:href="Pictures/a.svg"/></draw:frame>
<draw:frame svg:x="1cm" svg:y="5cm" svg:width="4cm" svg:height="2cm"><draw:image xlink:href="Pictures/missing.png"/></draw:frame></draw:page>
</office:presentation></office:body></office:document-content>"#
    );
    let imported = odp::convert(&package(
        MIME,
        &content,
        "",
        &[("Pictures/a.svg", svg.to_vec())],
    ))
    .unwrap();
    assert_valid(&imported.pptx);
    assert_eq!(
        imported.notes,
        ["1 picture in a format Kantor can't show left out"]
    );
    let slide = part_text(&imported.pptx, "ppt/slides/slide1.xml");
    assert!(slide.contains("<asvg:svgBlip"), "{slide}");
    let p = Presentation::from_bytes(imported.pptx).unwrap();
    let part = p
        .slide(0)
        .shapes
        .iter()
        .find_map(|s| match &s.kind {
            ShapeKind::Picture { part, .. } => part.clone(),
            _ => None,
        })
        .unwrap();
    assert!(
        p.part_bytes(&part).unwrap().starts_with(b"\x89PNG"),
        "{part}"
    );
}
