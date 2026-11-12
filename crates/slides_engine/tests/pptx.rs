//! `.pptx` round trips: new packages are valid, unchanged parts are copied
//! byte for byte, edits survive a reopen, and unmodelled XML is kept.

use std::collections::BTreeMap;
use std::io::{Cursor, Read as _, Write as _};

use slides_engine::render::LineSpacing;
use slides_engine::*;

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

fn text(bytes: &[u8], part: &str) -> String {
    String::from_utf8(
        parts(bytes)
            .remove(part)
            .unwrap_or_else(|| panic!("no {part}")),
    )
    .unwrap()
}

/// Rewrite a package with some parts replaced or added.
fn with_parts(bytes: &[u8], changes: &[(&str, String)]) -> Vec<u8> {
    let mut all = parts(bytes);
    for (k, v) in changes {
        all.insert((*k).to_owned(), v.clone().into_bytes());
    }
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    // Content types first, like Office.
    let ct = all.remove("[Content_Types].xml").unwrap();
    zip.start_file("[Content_Types].xml", opts).unwrap();
    zip.write_all(&ct).unwrap();
    for (k, v) in all {
        zip.start_file(k.as_str(), opts).unwrap();
        zip.write_all(&v).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn assert_valid(bytes: &[u8]) {
    let problems = package_problems(bytes);
    assert!(problems.is_empty(), "{problems:#?}");
}

/// A 2×1 red PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x7B, 0x40, 0xE8,
    0xDD, 0x00, 0x00, 0x00, 0x0F, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0,
    0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x56, 0xC7, 0x2F, 0x0D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn title_ref(p: &Presentation, slide: usize) -> ShapeRef {
    let i = p
        .slide(slide)
        .shapes
        .iter()
        .position(|s| s.ph.as_ref().is_some_and(|ph| ph.is_title()))
        .expect("title placeholder");
    (slide, i)
}

#[test]
fn new_presentation_is_a_valid_package() {
    let p = Presentation::new();
    assert_eq!(p.size, SlideSize::WIDE);
    assert_eq!(p.len(), 1);
    assert_eq!(p.layouts.len(), 2);
    assert_eq!(p.layout_of(0).name, "Title Slide");
    assert!(!p.is_dirty());
    assert!(p.needs_save_as(), "untitled");
    assert!(p.unsupported_content().is_empty());
    let bytes = p.to_bytes().unwrap();
    assert_valid(&bytes);
    // Placeholders inherit their position and show a prompt.
    let list = p.render_list(0, true);
    let title = list
        .iter()
        .find(|r| r.shape.ph.as_ref().is_some_and(|p| p.is_title()))
        .unwrap();
    assert_eq!(title.xfrm, Xfrm::new(1524000, 1122363, 9144000, 2387600));
    let t = title.text.as_ref().unwrap();
    assert!(t.prompt);
    assert_eq!(t.paras[0].text, "Click to add title");
    assert_eq!(
        t.paras[0].base.size_pt, 60.0,
        "layout lstStyle overrides master 44 pt"
    );
    assert_eq!(t.paras[0].align, Align::Center);
    assert_eq!(t.paras[0].base.font, "Calibri Light");
    assert!(
        p.render_list(0, false)
            .iter()
            .all(|r| r.text.as_ref().is_none_or(|t| !t.prompt))
    );
}

/// Empty placeholders are flagged for the editor's dashed outline; prompts
/// are display-only and never reach the file. Once the placeholder has
/// text, both go away.
#[test]
fn prompts_are_display_only() {
    let mut p = Presentation::new();
    let list = p.render_list(0, true);
    let empty: Vec<_> = list.iter().filter(|r| r.is_empty_placeholder()).collect();
    assert_eq!(empty.len(), 2, "title and subtitle");
    let sub = empty
        .iter()
        .find(|r| r.shape.ph.as_ref().is_some_and(|p| !p.is_title()))
        .unwrap();
    assert_eq!(
        sub.text.as_ref().unwrap().paras[0].text,
        "Click to add subtitle"
    );
    let bytes = p.to_bytes().unwrap();
    let xml = String::from_utf8_lossy(&parts(&bytes)["ppt/slides/slide1.xml"]).into_owned();
    assert!(!xml.contains("Click to add"), "{xml}");

    let t = title_ref(&p, 0);
    p.insert_text(t, Selection::default(), "Hello", None)
        .unwrap();
    let list = p.render_list(0, true);
    let title = list.iter().find(|r| r.layer == Layer::Slide(t.1)).unwrap();
    assert!(!title.is_empty_placeholder());
    let tt = title.text.as_ref().unwrap();
    assert!(!tt.prompt);
    assert_eq!(tt.paras[0].text, "Hello");

    let i = p.new_slide(Some(0)).unwrap();
    let list = p.render_list(i, true);
    let texts: Vec<_> = list
        .iter()
        .filter(|r| r.is_empty_placeholder())
        .map(|r| r.text.as_ref().unwrap().paras[0].text.clone())
        .collect();
    assert!(
        texts.contains(&"Click to add title".to_owned()),
        "{texts:?}"
    );
    assert!(texts.contains(&"Click to add text".to_owned()), "{texts:?}");
}

#[test]
fn unchanged_save_copies_every_part() {
    let p = Presentation::new();
    let a = p.to_bytes().unwrap();
    let b = Presentation::from_bytes(a.clone())
        .unwrap()
        .to_bytes()
        .unwrap();
    assert_eq!(parts(&a), parts(&b));
}

#[test]
fn edits_survive_save_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deck.pptx");
    let mut p = Presentation::new();
    let original = parts(&p.to_bytes().unwrap());

    let t = title_ref(&p, 0);
    let sel = p
        .insert_text(t, Selection::default(), "Quarterly review", None)
        .unwrap();
    p.toggle(
        t,
        Selection {
            anchor: Position::new(0, 0),
            head: sel.head,
        },
        Toggle::Italic,
    )
    .unwrap();
    let s1 = p.new_slide(Some(0)).unwrap();
    assert_eq!(p.layout_of(s1).name, "Title and Content");
    let tb = p.add_text_box(s1).unwrap();
    p.insert_text((s1, tb), Selection::default(), "Line one\nLine two", None)
        .unwrap();
    let rect = p.add_geometry(s1, Geom::Rect).unwrap();
    let oval = p.add_geometry(s1, Geom::Ellipse).unwrap();
    p.set_xfrm((s1, oval), Xfrm::new(100, 200, 300, 400), false)
        .unwrap();
    let pic = p.add_picture(s1, PNG.to_vec()).unwrap();
    let dup = p.duplicate_slide(s1).unwrap();
    p.delete_shape((dup, rect)).unwrap();
    p.move_slide(dup, 0).unwrap();
    assert!(p.is_dirty());
    p.save_as(&path).unwrap();
    assert!(!p.is_dirty());

    let bytes = std::fs::read(&path).unwrap();
    assert_valid(&bytes);
    let saved = parts(&bytes);
    // Parts Kantor doesn't model are byte-identical.
    for part in [
        "ppt/theme/theme1.xml",
        "ppt/slideMasters/slideMaster1.xml",
        "ppt/slideLayouts/slideLayout1.xml",
        "ppt/slideLayouts/slideLayout2.xml",
        "docProps/core.xml",
        "ppt/presProps.xml",
    ] {
        assert_eq!(saved[part], original[part], "{part}");
    }

    let q = Presentation::open(&path).unwrap();
    assert_eq!(q.len(), 3);
    // Slide order: duplicate (moved first), title, content.
    let title = &q.slide(1).shapes[title_ref(&q, 1).1];
    let tp = &title.text.as_ref().unwrap().paras[0];
    assert_eq!(tp.text, "Quarterly review");
    assert_eq!(tp.runs[0].props.italic, Some(true));
    let content = q.slide(2);
    let texts: Vec<_> = content
        .shapes
        .iter()
        .filter_map(|s| s.text.as_ref().map(|t| t.plain_text()))
        .collect();
    assert!(
        texts.contains(&"Line one\nLine two".to_owned()),
        "{texts:?}"
    );
    assert!(
        content
            .shapes
            .iter()
            .any(|s| s.xfrm == Some(Xfrm::new(100, 200, 300, 400)))
    );
    let picture = &content.shapes[pic];
    let ShapeKind::Picture {
        part: Some(part), ..
    } = &picture.kind
    else {
        panic!("{:?}", picture.kind)
    };
    assert_eq!(q.part_bytes(part).unwrap().as_slice(), PNG);
    assert_eq!(
        picture.xfrm.unwrap().cx,
        picture.xfrm.unwrap().cy * 2,
        "aspect kept"
    );
    assert_eq!(
        q.slide(0).shapes.len(),
        content.shapes.len() - 1,
        "duplicate lost the rect"
    );
    // Shape ids are unique per slide.
    for s in q.slides() {
        let mut ids: Vec<u32> = s.shapes.iter().map(|x| x.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), s.shapes.len());
    }
    // Saving the reopened file unchanged copies everything.
    let again = q.to_bytes().unwrap();
    assert_eq!(parts(&again), saved);
}

const SLIDE_EXTRAS: &str = r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="9" name="Table 8"/><p:cNvGraphicFramePr><a:graphicFrameLocks noGrp="1"/></p:cNvGraphicFramePr><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="10" y="20"/><a:ext cx="30" cy="40"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblGrid><a:gridCol w="30"/></a:tblGrid><a:tr h="40"><a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>cell</a:t></a:r></a:p></a:txBody></a:tc></a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#;

/// The template's title slide with a table, a transition and a notes slide.
fn rich_deck() -> Vec<u8> {
    let base = Presentation::new().to_bytes().unwrap();
    let slide = text(&base, "ppt/slides/slide1.xml")
        .replace("</p:spTree>", &format!("{SLIDE_EXTRAS}</p:spTree>"))
        .replace(
            "</p:clrMapOvr>",
            "</p:clrMapOvr><p:transition spd=\"slow\"><p:fade/></p:transition>",
        );
    let rels = text(&base, "ppt/slides/_rels/slide1.xml.rels").replace(
        "</Relationships>",
        "<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide\" Target=\"../notesSlides/notesSlide1.xml\"/></Relationships>",
    );
    let ct = text(&base, "[Content_Types].xml").replace(
        "</Types>",
        "<Override PartName=\"/ppt/notesSlides/notesSlide1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.notesSlide+xml\"/></Types>",
    );
    let notes = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:notes xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld></p:notes>"#;
    let notes_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="../slides/slide1.xml"/></Relationships>"#;
    with_parts(
        &base,
        &[
            ("ppt/slides/slide1.xml", slide),
            ("ppt/slides/_rels/slide1.xml.rels", rels),
            ("[Content_Types].xml", ct),
            ("ppt/notesSlides/notesSlide1.xml", notes.into()),
            (
                "ppt/notesSlides/_rels/notesSlide1.xml.rels",
                notes_rels.into(),
            ),
        ],
    )
}

#[test]
fn unmodelled_slide_content_is_kept_verbatim() {
    let deck = rich_deck();
    assert_valid(&deck);
    let mut p = Presentation::from_bytes(deck.clone()).unwrap();
    assert!(p.unsupported_content().is_empty());
    let table = p
        .slide(0)
        .shapes
        .iter()
        .position(|s| matches!(s.kind, ShapeKind::Other { .. }))
        .unwrap();
    assert_eq!(
        p.slide(0).shapes[table].xfrm,
        Some(Xfrm::new(10, 20, 30, 40))
    );
    assert!(
        p.set_xfrm((0, table), Xfrm::new(0, 0, 1, 1), false)
            .is_err(),
        "tables don't move yet"
    );
    let t = title_ref(&p, 0);
    p.insert_text(t, Selection::default(), "Hello & welcome", None)
        .unwrap();
    let out = p.to_bytes().unwrap();
    assert_valid(&out);
    let slide = text(&out, "ppt/slides/slide1.xml");
    assert!(slide.contains(SLIDE_EXTRAS), "table kept verbatim");
    assert!(slide.contains("<p:transition spd=\"slow\"><p:fade/></p:transition>"));
    assert!(slide.contains("<a:t>Hello &amp; welcome</a:t>"));
    let before = parts(&deck);
    let after = parts(&out);
    assert_eq!(
        before["ppt/notesSlides/notesSlide1.xml"],
        after["ppt/notesSlides/notesSlide1.xml"]
    );
    assert_eq!(
        before["ppt/slides/_rels/slide1.xml.rels"],
        after["ppt/slides/_rels/slide1.xml.rels"]
    );

    // Deleting the slide drops its notes too, and the package stays valid.
    p.new_slide(Some(0)).unwrap();
    p.delete_slide(0).unwrap();
    let out = p.to_bytes().unwrap();
    assert_valid(&out);
    let after = parts(&out);
    assert!(!after.contains_key("ppt/slides/slide1.xml"));
    assert!(!after.contains_key("ppt/notesSlides/notesSlide1.xml"));
    let q = Presentation::from_bytes(out).unwrap();
    assert_eq!(q.len(), 1);
    assert_eq!(q.slide(0).part, "ppt/slides/slide2.xml");
}

#[test]
fn sections_force_save_as_after_reordering() {
    let base = Presentation::new();
    let mut p = base;
    p.new_slide(Some(0)).unwrap();
    let bytes = p.to_bytes().unwrap();
    let pres = text(&bytes, "ppt/presentation.xml");
    let ids: Vec<String> = pres
        .match_indices("<p:sldId id=\"")
        .map(|(i, _)| pres[i + 13..].split('"').next().unwrap().to_owned())
        .collect();
    let sections = format!(
        "<p:extLst><p:ext uri=\"{{521415D9-36F7-43E2-AB2F-B90AF26B5E84}}\"><p14:sectionLst xmlns:p14=\"http://schemas.microsoft.com/office/powerpoint/2010/main\"><p14:section name=\"A\" id=\"{{00000000-0000-0000-0000-000000000001}}\"><p14:sldIdLst>{}</p14:sldIdLst></p14:section></p14:sectionLst></p:ext></p:extLst></p:presentation>",
        ids.iter()
            .map(|i| format!("<p14:sldId id=\"{i}\"/>"))
            .collect::<String>()
    );
    let with_sections = with_parts(
        &bytes,
        &[(
            "ppt/presentation.xml",
            pres.replace("</p:presentation>", &sections),
        )],
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sections.pptx");
    std::fs::write(&path, &with_sections).unwrap();

    let mut p = Presentation::open(&path).unwrap();
    assert!(p.unsupported_content().is_empty());
    assert!(!p.needs_save_as());
    // Editing text keeps sections; reordering doesn't.
    let t = title_ref(&p, 0);
    p.insert_text(t, Selection::default(), "x", None).unwrap();
    p.save().unwrap();
    assert!(text(&std::fs::read(&path).unwrap(), "ppt/presentation.xml").contains("sectionLst"));
    p.move_slide(1, 0).unwrap();
    assert!(p.has_unsupported_content());
    assert!(p.needs_save_as());
    assert!(p.save().is_err(), "never overwrites");
    assert!(p.save_as(&path).is_err(), "not even through Save As");
    let copy = dir.path().join("copy.pptx");
    p.save_as(&copy).unwrap();
    let out = std::fs::read(&copy).unwrap();
    assert_valid(&out);
    let pres = text(&out, "ppt/presentation.xml");
    assert!(!pres.contains("sectionLst") && !pres.contains("<p:extLst>"));
    assert!(roxmltree::Document::parse(&pres).is_ok());
}

#[test]
fn typing_coalesces_and_undo_tracks_dirty() {
    let mut p = Presentation::new();
    let t = title_ref(&p, 0);
    let mut sel = Selection::default();
    for ch in ["H", "e", "l", "l", "o"] {
        sel = p.insert_text(t, sel, ch, None).unwrap();
    }
    sel = p.insert_text(t, sel, " ", None).unwrap();
    for ch in ["y", "o", "u"] {
        sel = p.insert_text(t, sel, ch, None).unwrap();
    }
    let text_of = |p: &Presentation| p.text_paras(t).unwrap()[0].text.clone();
    assert_eq!(text_of(&p), "Hello you");
    let f = p.undo().unwrap();
    assert_eq!(f.slide, 0);
    assert_eq!(text_of(&p), "Hello", "a space starts the next step");
    p.undo();
    assert_eq!(text_of(&p), "");
    assert!(!p.is_dirty(), "back to the saved state");
    p.redo();
    assert_eq!(text_of(&p), "Hello");
    assert!(p.is_dirty());
    // Nudges merge into one step.
    let s = p.add_text_box(0).unwrap();
    let x0 = p.slide(0).shapes[s].xfrm.unwrap();
    for i in 1..=5 {
        p.set_xfrm(
            (0, s),
            Xfrm {
                x: x0.x + i * 10,
                ..x0
            },
            true,
        )
        .unwrap();
    }
    p.undo();
    assert_eq!(p.slide(0).shapes[s].xfrm, Some(x0));
    let _ = sel;
}

#[test]
fn bold_from_the_master_toggles_off() {
    let base = Presentation::new().to_bytes().unwrap();
    let master = text(&base, "ppt/slideMasters/slideMaster1.xml").replace(
        "<a:defRPr sz=\"4400\" kern=\"1200\">",
        "<a:defRPr sz=\"4400\" b=\"1\" kern=\"1200\">",
    );
    let mut p = Presentation::from_bytes(with_parts(
        &base,
        &[("ppt/slideMasters/slideMaster1.xml", master)],
    ))
    .unwrap();
    let s = p.new_slide(Some(0)).unwrap();
    let t = title_ref(&p, s);
    let sel = p
        .insert_text(t, Selection::default(), "Bold title", None)
        .unwrap();
    let all = Selection {
        anchor: Position::new(0, 0),
        head: sel.head,
    };
    assert!(
        p.is_on(t, all, Toggle::Bold),
        "inherited from the master title style"
    );
    p.toggle(t, all, Toggle::Bold).unwrap();
    assert!(!p.is_on(t, all, Toggle::Bold));
    assert_eq!(p.text_paras(t).unwrap()[0].runs[0].props.bold, Some(false));
}

#[test]
fn locked_slides_refuse_edits_but_are_kept() {
    let base = Presentation::new().to_bytes().unwrap();
    let slide = text(&base, "ppt/slides/slide1.xml").replace(
        "xmlns:p=",
        "xmlns:pml=\"http://schemas.openxmlformats.org/presentationml/2006/main\" xmlns:p=",
    );
    // Same namespace bound twice is fine; now rename the element prefix.
    let slide = slide
        .replace("<p:sld ", "<pml:sld ")
        .replace("</p:sld>", "</pml:sld>");
    let deck = with_parts(&base, &[("ppt/slides/slide1.xml", slide.clone())]);
    let mut p = Presentation::from_bytes(deck).unwrap();
    // `p:` is still bound correctly here, so the slide is editable.
    assert!(p.slide(0).locked.is_none());
    let weird = slide.replace(
        "xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"",
        "xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:x=\"http://schemas.openxmlformats.org/drawingml/2006/main\"",
    );
    let weird = weird.replace("<a:", "<x:").replace("</a:", "</x:").replace(
        "xmlns:x=\"http://schemas.openxmlformats.org/drawingml/2006/main\" ",
        "",
    );
    let weird = weird.replace("xmlns:a=", "xmlns:x=");
    let deck = with_parts(&base, &[("ppt/slides/slide1.xml", weird.clone())]);
    p = Presentation::from_bytes(deck).unwrap();
    assert!(p.slide(0).locked.is_some(), "a: prefix not bound");
    assert!(p.add_text_box(0).is_err());
    p.new_slide(Some(0)).unwrap();
    let out = p.to_bytes().unwrap();
    assert_valid(&out);
    assert_eq!(text(&out, "ppt/slides/slide1.xml"), weird);
}

fn body_ref(p: &Presentation, slide: usize) -> ShapeRef {
    let i = p
        .slide(slide)
        .shapes
        .iter()
        .position(|s| s.ph.as_ref().is_some_and(|ph| !ph.is_title()))
        .expect("body placeholder");
    (slide, i)
}

fn markers(p: &Presentation, r: ShapeRef) -> Vec<Option<String>> {
    let list = p.render_list(r.0, false);
    let shape = list.iter().find(|x| x.layer == Layer::Slide(r.1)).unwrap();
    shape
        .text
        .as_ref()
        .unwrap()
        .paras
        .iter()
        .map(|p| p.marker.as_ref().map(|m| m.text.clone()))
        .collect()
}

fn all(p: &Presentation, r: ShapeRef) -> Selection {
    let paras = p.text_paras(r).unwrap();
    Selection {
        anchor: Position::default(),
        head: text_ops::end(paras),
    }
}

/// Bullets come from the master's body style; levels, numbering and
/// bullet toggles are written as DrawingML and survive a reopen.
#[test]
fn bullets_levels_and_numbering_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lists.pptx");
    let mut p = Presentation::new();
    let s = p.new_slide(Some(0)).unwrap();
    let body = body_ref(&p, s);
    p.insert_text(body, Selection::default(), "One\nTwo\nThree", None)
        .unwrap();
    // Inherited: "•", marL 228600 (18 pt), hanging 18 pt, 10 pt before, 90 % lines.
    assert_eq!(markers(&p, body), vec![Some("•".to_owned()); 3]);
    let list = p.render_list(s, false);
    let t = list
        .iter()
        .find(|x| x.layer == Layer::Slide(body.1))
        .unwrap()
        .text
        .clone()
        .unwrap();
    assert_eq!(t.paras[0].indent_pt, 18.0);
    assert_eq!(t.paras[0].first_line_pt, -18.0);
    assert_eq!(t.paras[0].space_before_pt, 0.0, "first paragraph");
    assert_eq!(t.paras[1].space_before_pt, 10.0);
    assert_eq!(t.paras[1].line, Some(LineSpacing::Multiple(0.9)));

    // Tab on "Two": level 2 (smaller text from lvl2pPr).
    let two = Selection::caret(Position::new(1, 0));
    p.change_level(body, two, 1).unwrap();
    assert_eq!(p.text_paras(body).unwrap()[1].props.level, 1);
    assert!(p.is_list(body, all(&p, body), false));
    // Numbered: levels count separately.
    p.toggle_list(body, all(&p, body), true).unwrap();
    assert!(p.is_list(body, all(&p, body), true));
    assert_eq!(
        markers(&p, body),
        [Some("1."), Some("1."), Some("2.")].map(|m| m.map(str::to_owned))
    );
    // "Three" off: an explicit buNone over the inherited bullet.
    let three = Selection::caret(Position::new(2, 0));
    p.toggle_list(body, three, true).unwrap();
    assert_eq!(markers(&p, body)[2], None);
    p.change_level(body, two, -1).unwrap();

    // A text box has no bullets; turning them on adds a hanging indent.
    let tb = p.add_text_box(s).unwrap();
    p.insert_text((s, tb), Selection::default(), "a\nb", None)
        .unwrap();
    assert_eq!(markers(&p, (s, tb)), vec![None, None]);
    p.toggle_list((s, tb), all(&p, (s, tb)), false).unwrap();
    assert_eq!(markers(&p, (s, tb)), vec![Some("•".to_owned()); 2]);
    // Undo and redo the toggle.
    p.undo().unwrap();
    assert_eq!(markers(&p, (s, tb)), vec![None, None]);
    p.redo().unwrap();

    p.save_as(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_valid(&bytes);
    let xml = text(&bytes, &p.slide(s).part);
    assert!(
        xml.contains(r#"<a:buAutoNum type="arabicPeriod"/>"#),
        "{xml}"
    );
    assert!(xml.contains("<a:buNone/>"), "{xml}");
    assert!(
        xml.contains(r#"<a:pPr marL="342900" indent="-342900"><a:buChar char="•"/></a:pPr>"#),
        "{xml}"
    );
    let q = Presentation::open(&path).unwrap();
    assert_eq!(
        markers(&q, body),
        [Some("1."), Some("2."), None].map(|m| m.map(str::to_owned))
    );
    assert_eq!(markers(&q, (s, tb)), vec![Some("•".to_owned()); 2]);
}

/// Paragraph properties Kantor doesn't model (spacing, tab stops, other
/// attributes) are kept when the bullet changes; a symbol bullet font is
/// dropped with the symbol it drew.
#[test]
fn bullet_changes_keep_unmodelled_paragraph_xml() {
    let base = Presentation::new().to_bytes().unwrap();
    let para = r#"<a:p><a:pPr marL="285750" indent="-285750" fontAlgn="base"><a:lnSpc><a:spcPct val="150000"/></a:lnSpc><a:buFont typeface="Wingdings" pitchFamily="2" charset="2"/><a:buChar char="§"/><a:tabLst><a:tab pos="914400" algn="l"/></a:tabLst></a:pPr><a:r><a:rPr lang="en-US"/><a:t>Point</a:t></a:r></a:p>"#;
    let shape = format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="TextBox 8"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="3000000" cy="1000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/>{para}</p:txBody></p:sp>"#
    );
    let slide =
        text(&base, "ppt/slides/slide1.xml").replace("</p:spTree>", &format!("{shape}</p:spTree>"));
    let mut p =
        Presentation::from_bytes(with_parts(&base, &[("ppt/slides/slide1.xml", slide)])).unwrap();
    let r = (0, p.slide(0).shapes.len() - 1);
    // Wingdings § is a square bullet.
    assert_eq!(markers(&p, r), vec![Some("■".to_owned())]);
    let unchanged = p.to_bytes().unwrap();
    assert!(text(&unchanged, "ppt/slides/slide1.xml").contains(para));
    p.toggle_list(r, all(&p, r), true).unwrap();
    let xml = text(&p.to_bytes().unwrap(), "ppt/slides/slide1.xml");
    assert!(
        xml.contains(r#"<a:pPr marL="285750" indent="-285750" fontAlgn="base"><a:lnSpc><a:spcPct val="150000"/></a:lnSpc><a:buAutoNum type="arabicPeriod"/><a:tabLst><a:tab pos="914400" algn="l"/></a:tabLst></a:pPr>"#),
        "{xml}"
    );
}

/// Notes are created with their part, relationships, content type and a
/// notes master; edits are undoable and survive save, reopen and edit.
#[test]
fn speaker_notes_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.pptx");
    let mut p = Presentation::new();
    assert_eq!(p.notes_text(0), "");
    assert_eq!(p.notes_locked(0), None);
    p.set_notes_text(0, "Say hello").unwrap();
    p.set_notes_text(0, "Say hello\nThen bye").unwrap();
    assert_eq!(p.notes_text(0), "Say hello\nThen bye");
    // Typing in the notes is one undo step.
    p.undo().unwrap();
    assert_eq!(p.notes_text(0), "");
    p.redo().unwrap();
    assert_eq!(p.notes_text(0), "Say hello\nThen bye");
    // Duplicating the slide copies its notes into a new part.
    let dup = p.duplicate_slide(0).unwrap();
    assert_eq!(p.notes_text(dup), "Say hello\nThen bye");
    p.save_as(&path).unwrap();

    let bytes = std::fs::read(&path).unwrap();
    assert_valid(&bytes);
    let all = parts(&bytes);
    let masters: Vec<_> = all
        .keys()
        .filter(|k| k.starts_with("ppt/notesMasters/notesMaster"))
        .collect();
    assert_eq!(masters, ["ppt/notesMasters/notesMaster1.xml"]);
    assert!(all.contains_key("ppt/notesSlides/notesSlide1.xml"));
    assert!(all.contains_key("ppt/notesSlides/notesSlide2.xml"));
    let pres = text(&bytes, "ppt/presentation.xml");
    let a = pres.find("</p:sldMasterIdLst>").unwrap();
    let b = pres.find("<p:notesMasterIdLst>").unwrap();
    let c = pres.find("<p:sldIdLst>").unwrap();
    assert!(a < b && b < c, "schema order: {pres}");
    let notes = text(&bytes, "ppt/notesSlides/notesSlide1.xml");
    assert!(notes.contains("<a:t>Then bye</a:t>"), "{notes}");
    let ct = text(&bytes, "[Content_Types].xml");
    assert!(ct.contains("/ppt/notesSlides/notesSlide1.xml"));
    assert!(ct.contains("notesMaster+xml"));

    // Reopen, edit, save again: the notes part is rewritten in place.
    let mut q = Presentation::open(&path).unwrap();
    assert_eq!(q.notes_text(0), "Say hello\nThen bye");
    q.set_notes_text(0, "Say hello!\nThen bye").unwrap();
    q.save().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_valid(&bytes);
    let r = Presentation::open(&path).unwrap();
    assert_eq!(r.notes_text(0), "Say hello!\nThen bye");
    assert_eq!(r.notes_text(1), "Say hello\nThen bye");
    assert_eq!(
        parts(&bytes)
            .keys()
            .filter(|k| k.starts_with("ppt/notesMasters/notesMaster") && k.ends_with(".xml"))
            .count(),
        1
    );
}

/// Notes without a notes text box are kept but can't be edited.
#[test]
fn notes_without_a_body_are_read_only() {
    let mut p = Presentation::from_bytes(rich_deck()).unwrap();
    assert!(p.notes_locked(0).is_some());
    assert!(p.set_notes_text(0, "x").is_err());
    assert!(!p.has_unsupported_content());
}

/// Rotation and flips are written to `a:xfrm` and read back.
#[test]
fn rotation_round_trips() {
    let mut p = Presentation::new();
    let r = p.add_geometry(0, Geom::Rect).unwrap();
    let x = p.slide(0).shapes[r].xfrm.unwrap();
    let turned = Xfrm {
        flip_h: true,
        ..x.with_degrees(-30.0)
    };
    assert_eq!(turned.rot, 330 * 60_000);
    p.set_xfrm((0, r), turned, false).unwrap();
    let bytes = p.to_bytes().unwrap();
    let xml = text(&bytes, "ppt/slides/slide1.xml");
    assert!(
        xml.contains(r#"<a:xfrm rot="19800000" flipH="1">"#),
        "{xml}"
    );
    let q = Presentation::from_bytes(bytes).unwrap();
    assert_eq!(q.slide(0).shapes[r].xfrm, Some(turned));
}

/// Fill and outline edits replace only the fill and the outline's colour
/// and width in `p:spPr`; dashes, effects and the rest are kept.
#[test]
fn fill_and_line_edits_keep_the_rest_of_sppr() {
    let base = Presentation::new().to_bytes().unwrap();
    let styled = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="Styled"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="100" cy="100"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill><a:ln w="12700" cap="rnd"><a:solidFill><a:srgbClr val="00FF00"/></a:solidFill><a:prstDash val="dash"/></a:ln><a:effectLst/></p:spPr></p:sp>"#;
    let bare = r#"<p:sp><p:nvSpPr><p:cNvPr id="10" name="Bare"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="100" cy="100"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:sp>"#;
    let slide = text(&base, "ppt/slides/slide1.xml")
        .replace("</p:spTree>", &format!("{styled}{bare}</p:spTree>"));
    let mut p =
        Presentation::from_bytes(with_parts(&base, &[("ppt/slides/slide1.xml", slide)])).unwrap();
    let n = p.slide(0).shapes.len();
    let (title, styled_i, bare_i) = (title_ref(&p, 0), n - 2, n - 1);
    let accent2 = p
        .theme_colors(0)
        .into_iter()
        .find(|(k, _)| *k == "accent2")
        .unwrap()
        .1;
    p.set_fill((0, styled_i), Paint::Scheme("accent2".into()))
        .unwrap();
    p.set_line((0, styled_i), None, Some(38_100)).unwrap();
    assert_eq!(p.slide(0).shapes[styled_i].look.fill, Some(accent2));
    p.set_line((0, bare_i), Some(Paint::Rgb(Rgb([1, 2, 3]))), None)
        .unwrap();
    p.set_fill((0, bare_i), Paint::None).unwrap();
    // The title placeholder has `<p:spPr/>`: moved and filled at once.
    p.set_xfrm(title, Xfrm::new(1, 2, 3, 4), false).unwrap();
    p.set_fill(title, Paint::Rgb(Rgb([0x12, 0x34, 0x56])))
        .unwrap();

    let bytes = p.to_bytes().unwrap();
    assert_valid(&bytes);
    let xml = text(&bytes, "ppt/slides/slide1.xml");
    assert!(xml.contains(r#"<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:schemeClr val="accent2"/></a:solidFill><a:ln w="38100" cap="rnd"><a:solidFill><a:srgbClr val="00FF00"/></a:solidFill><a:prstDash val="dash"/></a:ln><a:effectLst/></p:spPr>"#), "{xml}");
    assert!(xml.contains(r#"<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/><a:ln w="12700"><a:solidFill><a:srgbClr val="010203"/></a:solidFill></a:ln></p:spPr>"#), "{xml}");
    assert!(xml.contains(r#"<p:spPr><a:xfrm><a:off x="1" y="2"/><a:ext cx="3" cy="4"/></a:xfrm><a:solidFill><a:srgbClr val="123456"/></a:solidFill></p:spPr>"#), "{xml}");
    // Undo restores the original shape exactly.
    for _ in 0..6 {
        p.undo().unwrap();
    }
    let xml = text(&p.to_bytes().unwrap(), "ppt/slides/slide1.xml");
    assert!(xml.contains(styled) && xml.contains(bare), "{xml}");
    let q = Presentation::from_bytes(bytes).unwrap();
    assert_eq!(q.slide(0).shapes[styled_i].look.fill, Some(accent2));
    assert_eq!(
        q.slide(0).shapes[styled_i].look.line.map(|l| l.1),
        Some(38_100)
    );
}

/// Shapes copy within a presentation (offset when they'd cover the
/// original) and into another one (pictures take their media along);
/// pastes are undoable and the packages stay valid.
#[test]
fn copy_paste_and_duplicate_shapes() {
    let mut a = Presentation::new();
    let pic = a.add_picture(0, PNG.to_vec()).unwrap();
    let rect = a.add_geometry(0, Geom::Rect).unwrap();
    a.insert_text((0, rect), Selection::default(), "Box", None)
        .unwrap();
    a.set_fill((0, rect), Paint::Scheme("accent2".into()))
        .unwrap();
    let a_bytes = a.to_bytes().unwrap();
    let mut a = Presentation::from_bytes(a_bytes).unwrap();

    // Duplicate: same slide, offset, new id, same picture part.
    let dup = a.duplicate_shape((0, pic)).unwrap();
    let (orig, copy) = (&a.slide(0).shapes[pic], &a.slide(0).shapes[dup]);
    assert_ne!(orig.id, copy.id);
    let (ox, cx_) = (orig.xfrm.unwrap(), copy.xfrm.unwrap());
    assert_eq!((cx_.x - ox.x, cx_.y - ox.y), (228_600, 228_600));
    let bytes = a.to_bytes().unwrap();
    assert_valid(&bytes);
    assert_eq!(
        parts(&bytes)
            .keys()
            .filter(|k| k.starts_with("ppt/media/"))
            .count(),
        1,
        "the copy shares the picture"
    );

    // Into another presentation, on a slide of its own.
    let clip = a.copy_shapes(0, &[pic, rect]).unwrap();
    assert_eq!(clip.text, "Box");
    let mut b = Presentation::new();
    let s = b.new_slide(Some(0)).unwrap();
    let before = b.slide(s).shapes.len();
    let added = b.paste_shapes(s, &clip).unwrap();
    assert_eq!(added, vec![before, before + 1]);
    let ids: Vec<u32> = b.slide(s).shapes.iter().map(|x| x.id).collect();
    let mut uniq = ids.clone();
    uniq.dedup();
    assert_eq!(ids.len(), uniq.len(), "unique ids: {ids:?}");
    assert_eq!(b.text_paras((s, before + 1)).unwrap()[0].text, "Box");
    // Not offset: nothing there on this slide.
    assert_eq!(b.slide(s).shapes[before].xfrm, a.slide(0).shapes[pic].xfrm);
    let bytes = b.to_bytes().unwrap();
    assert_valid(&bytes);
    let xml = text(&bytes, &b.slide(s).part);
    assert!(xml.contains(r#"<a:schemeClr val="accent2"/>"#), "{xml}");
    assert!(
        parts(&bytes)
            .keys()
            .any(|k| k.starts_with("ppt/media/image"))
    );
    let reopened = Presentation::from_bytes(bytes).unwrap();
    assert!(reopened.slide(s).shapes[before].is_picture());

    b.undo().unwrap();
    assert_eq!(b.slide(s).shapes.len(), before);

    // Tables, charts and groups aren't copied yet.
    let c = Presentation::from_bytes(rich_deck()).unwrap();
    let table = c.slide(0).shapes.len() - 1;
    assert!(c.copy_shapes(0, &[table]).is_err());
}

/// Adjusted presets, custom geometry and connectors are read for display
/// and written back exactly as they were (unchanged or moved).
#[test]
fn shape_geometry_reads_and_round_trips() {
    let base = Presentation::new().to_bytes().unwrap();
    let arrow = r#"<p:sp><p:nvSpPr><p:cNvPr id="20" name="Arrow"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="100" y="100"/><a:ext cx="2000000" cy="1000000"/></a:xfrm><a:prstGeom prst="rightArrow"><a:avLst><a:gd name="adj1" fmla="val 80000"/><a:gd name="adj2" fmla="val 20000"/></a:avLst></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Go</a:t></a:r></a:p></p:txBody></p:sp>"#;
    let custom = r#"<p:sp><p:nvSpPr><p:cNvPr id="21" name="Free"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1000" cy="1000"/></a:xfrm><a:custGeom><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:rect l="l" t="t" r="r" b="b"/><a:pathLst><a:path w="10" h="10"><a:moveTo><a:pt x="0" y="10"/></a:moveTo><a:lnTo><a:pt x="5" y="0"/></a:lnTo><a:lnTo><a:pt x="10" y="10"/></a:lnTo><a:close/></a:path></a:pathLst></a:custGeom></p:spPr></p:sp>"#;
    let elbow = r#"<p:cxnSp><p:nvCxnSpPr><p:cNvPr id="22" name="Elbow"/><p:cNvCxnSpPr><a:stCxn id="20" idx="3"/><a:endCxn id="21" idx="1"/></p:cNvCxnSpPr><p:nvPr/></p:nvCxnSpPr><p:spPr><a:xfrm flipV="1"><a:off x="10" y="20"/><a:ext cx="3000" cy="4000"/></a:xfrm><a:prstGeom prst="bentConnector3"><a:avLst><a:gd name="adj1" fmla="val 25000"/></a:avLst></a:prstGeom><a:ln w="12700"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:ln></p:spPr></p:cxnSp>"#;
    let straight = r#"<p:cxnSp><p:nvCxnSpPr><p:cNvPr id="23" name="Straight"/><p:cNvCxnSpPr/><p:nvPr/></p:nvCxnSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="10" cy="10"/></a:xfrm><a:prstGeom prst="straightConnector1"><a:avLst/></a:prstGeom></p:spPr></p:cxnSp>"#;
    let slide = text(&base, "ppt/slides/slide1.xml").replace(
        "</p:spTree>",
        &format!("{arrow}{custom}{elbow}{straight}</p:spTree>"),
    );
    let deck = with_parts(&base, &[("ppt/slides/slide1.xml", slide.clone())]);
    let mut p = Presentation::from_bytes(deck.clone()).unwrap();
    let n = p.slide(0).shapes.len();
    let shapes = &p.slide(0).shapes[n - 4..];

    let g = shapes[0].kind.geometry().unwrap();
    assert_eq!(g.preset_name(), Some("rightArrow"));
    let o = geom::outline(&g, 2_000_000.0, 1_000_000.0);
    let b = o.bounds().unwrap();
    assert!((b.r - 2_000_000.0).abs() < 1.0 && b.l.abs() < 1.0);
    assert!(matches!(
        &shapes[1].kind,
        ShapeKind::Shape { geom: Geom::Other(o), geometry: Some(Geometry::Custom(_)), .. } if o == "custom"
    ));
    let tri = geom::outline(&shapes[1].kind.geometry().unwrap(), 1000.0, 1000.0);
    assert_eq!(tri.paths[0].cmds[1], geom::PathCmd::LineTo((500.0, 0.0)));
    // Connectors are shapes with their own geometry and outline.
    let Some(Geometry::Preset { name, adjust }) = shapes[2].kind.geometry() else {
        panic!("{:?}", shapes[2].kind)
    };
    assert_eq!((name.as_str(), adjust.len()), ("bentConnector3", 1));
    assert_eq!(shapes[2].look.line.map(|l| l.0), Some(Rgb([0xFF, 0, 0])));
    assert!(shapes[2].xfrm.unwrap().flip_v);
    assert!(matches!(
        shapes[3].kind,
        ShapeKind::Shape {
            geom: Geom::Line,
            ..
        }
    ));
    let line = shapes[3].kind.geometry().unwrap();
    assert!(line.is_basic());
    assert_eq!(
        geom::outline(&line, 10.0, 10.0).paths[0].flatten(1.0),
        [(vec![(0.0, 0.0), (10.0, 10.0)], false)]
    );
    // Unchanged: every part byte for byte.
    assert_eq!(parts(&p.to_bytes().unwrap()), parts(&deck));
    // Moved: only the offsets change; geometry XML is kept verbatim.
    for i in n - 4..n {
        let x = p.slide(0).shapes[i].xfrm.unwrap();
        p.set_xfrm((0, i), Xfrm { x: x.x + 7, ..x }, false).unwrap();
    }
    let out = p.to_bytes().unwrap();
    assert_valid(&out);
    let xml = text(&out, "ppt/slides/slide1.xml");
    for (src, from, to) in [
        (arrow, r#"<a:off x="100" "#, r#"<a:off x="107" "#),
        (
            custom,
            r#"<a:off x="0" y="0"/><a:ext cx="1000""#,
            r#"<a:off x="7" y="0"/><a:ext cx="1000""#,
        ),
        (elbow, r#"<a:off x="10" "#, r#"<a:off x="17" "#),
        (
            straight,
            r#"<a:off x="0" y="0"/><a:ext cx="10""#,
            r#"<a:off x="7" y="0"/><a:ext cx="10""#,
        ),
    ] {
        let moved = src.replace(from, to);
        assert_ne!(moved, src);
        assert!(xml.contains(&moved), "{moved}\n{xml}");
    }
    // Reopened: the same geometry.
    let q = Presentation::from_bytes(out).unwrap();
    for i in n - 4..n {
        assert_eq!(q.slide(0).shapes[i].kind, p.slide(0).shapes[i].kind);
    }
    // A copy keeps the adjustments too.
    let clip = p.copy_shapes(0, &[n - 4]).unwrap();
    p.paste_shapes(0, &clip).unwrap();
    let last = p.slide(0).shapes.last().unwrap();
    assert_eq!(last.kind, p.slide(0).shapes[n - 4].kind);
}

#[test]
fn recovery_copy_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deck.pptx");
    std::fs::write(&path, Presentation::new().to_bytes().unwrap()).unwrap();
    let original = std::fs::read(&path).unwrap();
    let mut p = Presentation::open(&path).unwrap();
    let rev = p.revision();
    let title = title_ref(&p, 0);
    let sel = p
        .insert_text(title, Selection::default(), "Q3", None)
        .unwrap();
    let r1 = p.revision();
    assert!(r1 > rev);
    p.insert_text(title, sel, "x", None).unwrap();
    assert!(
        p.revision() > r1,
        "merged typing still changes the revision"
    );
    p.add_text_box(0).unwrap();

    let copy = p.to_bytes().unwrap();
    assert!(p.is_dirty(), "a recovery copy isn't a save");
    assert_eq!(std::fs::read(&path).unwrap(), original);

    let mut back =
        Presentation::open_recovered(copy.clone(), Some(path.clone()), Vec::new()).unwrap();
    assert!(back.is_dirty());
    assert_eq!(back.path(), Some(path.as_path()));
    assert_eq!(back.title(), "deck.pptx");
    assert_eq!(back.slide(0).shapes.len(), p.slide(0).shapes.len());
    assert_eq!(back.text_paras(title).unwrap()[0].text, "Q3x");
    assert!(!back.needs_save_as());
    back.save().unwrap();
    assert!(!back.is_dirty());
    let again = Presentation::open(&path).unwrap();
    assert_eq!(again.text_paras(title).unwrap()[0].text, "Q3x");

    // Content Kantor can't keep still sends Save to Save As.
    let mut restricted =
        Presentation::open_recovered(copy, Some(path.clone()), vec!["custom slide shows".into()])
            .unwrap();
    assert!(restricted.needs_save_as());
    assert!(restricted.save().is_err());
}

#[cfg(unix)]
#[test]
fn saving_keeps_the_file_permissions() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.pptx");
    let mut p = slides_engine::Presentation::new();
    p.save_as(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    p.save().unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o644);
}
