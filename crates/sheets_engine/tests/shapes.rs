//! Shapes and text boxes on worksheets (`23_shapes.xlsx`): read as their
//! elements, text edited and saved in place (the rest of the drawing byte
//! for byte), inserted from Insert ▸ Shapes, copied with their sheet,
//! undone across saves.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use sheets_engine::drawing::{Drawing, DrawingKind};
use sheets_engine::{ShapePreset, Workbook};

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/xlsx")
        .join(name)
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("shapes");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn parts(path: &Path) -> BTreeMap<String, String> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).unwrap();
        let mut b = Vec::new();
        e.read_to_end(&mut b).unwrap();
        out.insert(
            e.name().unwrap().into_owned(),
            String::from_utf8_lossy(&b).into_owned(),
        );
    }
    out
}

fn shapes(wb: &Workbook, sheet: u32) -> Vec<Drawing> {
    wb.drawings(sheet)
        .into_iter()
        .filter(|d| matches!(*d.kind, DrawingKind::Shape { .. }))
        .collect()
}

fn named(wb: &Workbook, sheet: u32, name: &str) -> Drawing {
    wb.drawings(sheet)
        .into_iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("no {name}"))
}

#[test]
fn shapes_are_read_with_their_text() {
    let wb = Workbook::open(corpus("23_shapes.xlsx")).unwrap();
    let names: Vec<String> = shapes(&wb, 0).iter().map(|d| d.name.clone()).collect();
    for n in [
        "Revenue box",
        "Gradient oval",
        "Dashed frame",
        "Arrow line",
        "Turned arrow",
        "Notes box",
        "Pair",
        "Button 1",
        "Note callout",
        "Hidden shape",
    ] {
        assert!(names.iter().any(|x| x == n), "{n} missing from {names:?}");
    }
    assert!(named(&wb, 0, "Hidden shape").hidden);
    // A form control's shape is shown, not edited.
    assert!(!named(&wb, 0, "Button 1").editable);
    assert!(named(&wb, 0, "Revenue box").editable);
    let text = |n: &str| wb.shape_text(named(&wb, 0, n).key);
    assert_eq!(text("Revenue box").as_deref(), Some("Revenue"));
    assert_eq!(
        text("Dashed frame").as_deref(),
        Some("First line\nSecond line")
    );
    // Connectors and groups have no text to edit.
    assert_eq!(text("Arrow line"), None);
    assert_eq!(text("Pair"), None);
    assert_eq!(text("Button 1"), None);
    // Each element parses on its own.
    for d in shapes(&wb, 0) {
        let DrawingKind::Shape { xml, .. } = &*d.kind else {
            unreachable!()
        };
        roxmltree::Document::parse(xml).unwrap_or_else(|e| panic!("{}: {e}", d.name));
    }
}

#[test]
fn untouched_shapes_save_byte_for_byte() {
    let mut wb = Workbook::open(corpus("23_shapes.xlsx")).unwrap();
    let path = scratch("untouched.xlsx");
    wb.save_as(&path).unwrap();
    let before = parts(&corpus("23_shapes.xlsx"));
    let after = parts(&path);
    assert_eq!(
        before["xl/drawings/drawing1.xml"],
        after["xl/drawings/drawing1.xml"]
    );
}

#[test]
fn edited_text_saves_in_place_and_undoes_across_saves() {
    let mut wb = Workbook::open(corpus("23_shapes.xlsx")).unwrap();
    let key = named(&wb, 0, "Revenue box").key;
    wb.set_shape_text(key, "Sales\nUp 5%").unwrap();
    assert_eq!(wb.shape_text(key).as_deref(), Some("Sales\nUp 5%"));
    // The same text again: no undo step.
    wb.set_shape_text(key, "Sales\nUp 5%").unwrap();
    let path = scratch("edited.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let original = parts(&corpus("23_shapes.xlsx"))["xl/drawings/drawing1.xml"].clone();
    let saved = parts(&path)["xl/drawings/drawing1.xml"].clone();
    // The bold white 14 pt formatting stays on both lines.
    assert_eq!(saved.matches("sz=\"1400\" b=\"1\"").count(), 2);
    // Everything but that shape is as it was.
    let other = |x: &str| x[x.find("Gradient oval").unwrap()..].to_owned();
    assert_eq!(other(&original), other(&saved));
    let back = Workbook::open(&path).unwrap();
    let k2 = named(&back, 0, "Revenue box").key;
    assert_eq!(back.shape_text(k2).as_deref(), Some("Sales\nUp 5%"));
    // Undone after the save: the next save has the old text again.
    assert!(wb.undo().unwrap());
    assert_eq!(wb.shape_text(key).as_deref(), Some("Revenue"));
    wb.save_as(&path).unwrap();
    let back = Workbook::open(&path).unwrap();
    let k3 = named(&back, 0, "Revenue box").key;
    assert_eq!(back.shape_text(k3).as_deref(), Some("Revenue"));
    // A moved shape keeps its edited text.
    assert!(wb.redo().unwrap());
    let d = named(&wb, 0, "Revenue box");
    let moved = match d.anchor {
        sheets_engine::drawing::Anchor::TwoCell { from, to, edit_as } => {
            sheets_engine::drawing::Anchor::TwoCell {
                from: sheets_engine::drawing::Marker {
                    row: from.row + 2,
                    ..from
                },
                to: sheets_engine::drawing::Marker {
                    row: to.row + 2,
                    ..to
                },
                edit_as,
            }
        }
        a => a,
    };
    wb.place_drawing(key, moved).unwrap();
    wb.save_as(&path).unwrap();
    let back = Workbook::open(&path).unwrap();
    let d = named(&back, 0, "Revenue box");
    assert_eq!(back.shape_text(d.key).as_deref(), Some("Sales\nUp 5%"));
    assert_eq!(d.anchor, moved);
}

#[test]
fn inserted_shapes_save_and_go_with_undo() {
    let mut wb = Workbook::open(corpus("23_shapes.xlsx")).unwrap();
    let other = 1;
    let tb = wb.insert_shape(other, ShapePreset::TextBox, 2, 1).unwrap();
    wb.set_shape_text(tb, "Hello").unwrap();
    let arrow = wb.insert_shape(other, ShapePreset::Arrow, 8, 1).unwrap();
    let callout = wb.insert_shape(0, ShapePreset::Callout, 25, 1).unwrap();
    assert_eq!(wb.shape_text(arrow), None);
    assert_eq!(wb.shape_text(callout).as_deref(), Some(""));
    let path = scratch("inserted.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let back = Workbook::open(&path).unwrap();
    let names: Vec<String> = shapes(&back, other)
        .iter()
        .map(|d| d.name.clone())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    let t = shapes(&back, other)
        .into_iter()
        .find(|d| d.name.starts_with("TextBox"))
        .unwrap();
    assert_eq!(back.shape_text(t.key).as_deref(), Some("Hello"));
    // Ids are unique in each drawing.
    let p = parts(&path);
    for (n, x) in p
        .iter()
        .filter(|(n, _)| n.starts_with("xl/drawings/drawing") && n.ends_with(".xml"))
    {
        let mut ids: Vec<&str> = x
            .split("cNvPr id=\"")
            .skip(1)
            .map(|s| s.split('"').next().unwrap())
            .collect();
        let all = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all, "{n}");
    }
    assert_eq!(shapes(&back, 0).len(), 11);
    // Text edited after a save goes to the same object; deleted after a
    // save, it's gone from the next.
    wb.set_shape_text(tb, "Hello again").unwrap();
    wb.delete_drawing(arrow).unwrap();
    wb.save_as(&path).unwrap();
    let back = Workbook::open(&path).unwrap();
    let list = shapes(&back, other);
    assert_eq!(list.len(), 1);
    assert_eq!(back.shape_text(list[0].key).as_deref(), Some("Hello again"));
    // All undone: nothing of them in the next save.
    while wb.undo().unwrap() {}
    wb.save_as(&path).unwrap();
    let back = Workbook::open(&path).unwrap();
    assert!(shapes(&back, other).is_empty());
    assert_eq!(shapes(&back, 0).len(), 10);
}

#[test]
fn a_copied_sheet_brings_its_shapes() {
    let mut wb = Workbook::open(corpus("23_shapes.xlsx")).unwrap();
    let copy = wb.copy_sheet(0, 1).unwrap();
    let names: Vec<String> = shapes(&wb, copy).iter().map(|d| d.name.clone()).collect();
    // Hidden ones aren't copied (as with charts and pictures).
    assert!(names.contains(&"Revenue box".to_owned()));
    assert!(names.contains(&"Pair".to_owned()));
    assert!(!names.contains(&"Hidden shape".to_owned()));
    let k = shapes(&wb, copy)
        .into_iter()
        .find(|d| d.name == "Revenue box")
        .unwrap()
        .key;
    wb.set_shape_text(k, "Copy").unwrap();
    // The original keeps its text.
    assert_eq!(
        wb.shape_text(named(&wb, 0, "Revenue box").key).as_deref(),
        Some("Revenue")
    );
    let path = scratch("copied.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let back = Workbook::open(&path).unwrap();
    let c = back
        .sheet_names()
        .iter()
        .position(|n| n == "Shapes (2)")
        .unwrap() as u32;
    let list = shapes(&back, c);
    assert_eq!(list.len(), names.len());
    let r = list.iter().find(|d| d.name == "Revenue box").unwrap();
    assert_eq!(back.shape_text(r.key).as_deref(), Some("Copy"));
    // The copied group's shapes get ids of their own.
    assert_unique_ids(&path);
    // Every copied object is editable (unique ids).
    assert!(list.iter().all(|d| d.editable), "{list:?}");
}

/// `cNvPr` ids are unique in each drawing of `path`.
fn assert_unique_ids(path: &Path) {
    for (n, x) in parts(path)
        .iter()
        .filter(|(n, _)| n.starts_with("xl/drawings/drawing") && n.ends_with(".xml"))
    {
        let mut ids: Vec<&str> = x
            .split("cNvPr id=\"")
            .skip(1)
            .map(|s| s.split('"').next().unwrap())
            .collect();
        let all = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all, "{n}: {x}");
    }
}

#[test]
fn formatted_shapes_save_in_place() {
    use sheets_engine::shape_format::{set_fill, set_outline, set_text_anchor};
    use sheets_engine::{OutlineEdit, ShapeFill, TextAnchor};
    let mut wb = Workbook::open(corpus("23_shapes.xlsx")).unwrap();
    let frame = named(&wb, 0, "Dashed frame").key;
    wb.edit_shape(frame, |x| {
        set_fill(
            x,
            ShapeFill::Solid {
                rgb: [0xFF, 0xF2, 0xCC],
                transparency: 30,
            },
        )
    })
    .unwrap();
    wb.edit_shape(frame, |x| set_text_anchor(x, TextAnchor::Middle))
        .unwrap();
    let line = named(&wb, 0, "Arrow line").key;
    wb.edit_shape(line, |x| {
        set_outline(
            x,
            &OutlineEdit {
                color: Some(Some([0xC0, 0, 0])),
                width: Some(38_100),
                dash: Some("sysDot"),
                head: Some("none"),
                tail: Some("stealth"),
            },
        )
    })
    .unwrap();
    // Form controls can't be formatted.
    let button = named(&wb, 0, "Button 1").key;
    assert!(
        wb.edit_shape(button, |x| set_fill(x, ShapeFill::None))
            .is_err()
    );
    let path = scratch("formatted.xlsx");
    wb.save_as(&path).unwrap();
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let original = parts(&corpus("23_shapes.xlsx"))["xl/drawings/drawing1.xml"].clone();
    let saved = parts(&path)["xl/drawings/drawing1.xml"].clone();
    roxmltree::Document::parse(&saved).unwrap();
    assert!(saved.contains("<a:srgbClr val=\"FFF2CC\"><a:alpha val=\"70000\"/></a:srgbClr>"));
    assert!(saved.contains("<a:prstDash val=\"sysDot\"/>"));
    assert!(saved.contains("<a:tailEnd type=\"stealth\"/>"));
    // Untouched shapes stay byte for byte.
    for n in [
        "Revenue box",
        "Gradient oval",
        "Turned arrow",
        "Notes box",
        "Pair",
    ] {
        let el = |x: &str| {
            let at = x.find(&format!("name=\"{n}\"")).unwrap();
            let start = x[..at].rfind("<xdr:twoCellAnchor").unwrap();
            let end = start + x[start..].find("</xdr:twoCellAnchor>").unwrap();
            x[start..end].to_owned()
        };
        assert_eq!(el(&original), el(&saved), "{n}");
    }
    let back = Workbook::open(&path).unwrap();
    let DrawingKind::Shape { xml, .. } = &*named(&back, 0, "Dashed frame").kind else {
        panic!()
    };
    assert!(xml.contains("anchor=\"ctr\""));
    // One undo step each.
    assert!(wb.undo().unwrap());
    let DrawingKind::Shape { xml, .. } = &*named(&wb, 0, "Arrow line").kind else {
        panic!()
    };
    assert!(!xml.contains("stealth"));
}

const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, 0x49, 0x48, 0x44, 0x52, 0, 0, 0,
    1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 0x0D, 0x49, 0x44, 0x41, 0x54,
    0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0, 0x1F, 0, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D,
    0x1D, 0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// The image a drawing's `r:embed` id reaches, in a saved package.
fn embedded(p: &BTreeMap<String, String>, drawing: &str, rid: &str) -> Option<String> {
    let (dir, file) = drawing.rsplit_once('/').unwrap();
    let rels = roxmltree::Document::parse(&p[&format!("{dir}/_rels/{file}.rels")]).unwrap();
    let t = rels
        .descendants()
        .find(|r| r.attribute("Id") == Some(rid))?
        .attribute("Target")?
        .to_owned();
    let part = format!("xl/{}", t.trim_start_matches("../"));
    p.contains_key(&part).then_some(part)
}

#[test]
fn picture_fills_and_shadows_save() {
    use sheets_engine::shape_format as fmt;
    let mut wb = Workbook::open(corpus("23_shapes.xlsx")).unwrap();
    let key = named(&wb, 0, "Revenue box").key;
    wb.set_shape_picture(key, PNG.to_vec(), "png").unwrap();
    wb.edit_shape(key, |x| {
        fmt::set_shadow(x, Some(sheets_engine::ShadowPreset::BottomRight))
    })
    .unwrap();
    let DrawingKind::Shape { xml, media } = &*named(&wb, 0, "Revenue box").kind else {
        panic!()
    };
    assert!(xml.contains("<a:blipFill"), "{xml}");
    assert!(xml.contains("dir=\"2700000\""), "{xml}");
    assert!(
        media
            .iter()
            .any(|(_, ext, d)| ext == "png" && &d[..] == PNG)
    );
    assert!(wb.set_shape_picture(key, vec![1, 2], "tiff").is_err());
    // A new shape too.
    let new = wb.insert_shape(0, ShapePreset::Rectangle, 2, 8).unwrap();
    wb.set_shape_picture(new, PNG.to_vec(), "png").unwrap();
    let path = scratch("picture-fill.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let p = parts(&path);
    let drawing = p
        .iter()
        .find(|(n, x)| n.starts_with("xl/drawings/drawing") && x.contains("Revenue box"))
        .map(|(n, _)| n.clone())
        .unwrap();
    let doc = roxmltree::Document::parse(&p[&drawing]).unwrap();
    let embeds: Vec<String> = doc
        .descendants()
        .filter(|n| n.has_tag_name("blip"))
        .filter_map(|n| {
            n.attributes()
                .find(|a| a.name() == "embed")
                .map(|a| a.value().to_owned())
        })
        .collect();
    assert!(embeds.len() >= 2, "{embeds:?}");
    for rid in &embeds {
        assert!(embedded(&p, &drawing, rid).is_some(), "{rid}");
    }
    // Opened again: the fill is there with its picture.
    let back = Workbook::open(&path).unwrap();
    let DrawingKind::Shape { media, .. } = &*named(&back, 0, "Revenue box").kind else {
        panic!()
    };
    assert!(media.iter().any(|(_, _, d)| &d[..] == PNG));
    // Saved again: no second copy of the relationship.
    let path2 = scratch("picture-fill2.xlsx");
    let mut back = back;
    back.save_as(&path2).unwrap();
    assert!(sheets_engine::package_problems(&std::fs::read(&path2).unwrap()).is_empty());
}

#[test]
fn linked_text_and_equations_show_and_stay() {
    let mut wb = Workbook::open(corpus("23_shapes.xlsx")).unwrap();
    let key = named(&wb, 0, "Revenue box").key;
    wb.set_input(0, 0, 0, "Live value").unwrap();
    // `=A1` typed into the shape, as Excel saves it; and an equation.
    wb.edit_shape(key, |x| {
        let start = x.find("<a:p>").unwrap();
        let end = x.rfind("</a:p>").unwrap() + "</a:p>".len();
        let mut out = x.to_owned();
        out.replace_range(
            start..end,
            "<a:p><a:fld id=\"{AB12CD34-0000-4000-8000-000000000001}\" type=\"TxLink\"><a:rPr lang=\"en-US\"/><a:pPr/><a:t>old</a:t></a:fld></a:p><a:p><mc:AlternateContent xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" xmlns:a14=\"http://schemas.microsoft.com/office/drawing/2010/main\"><mc:Choice Requires=\"a14\"><a14:m><m:oMathPara xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\"><m:oMath><m:sSup><m:e><m:r><m:t>x</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup></m:oMath></m:oMathPara></a14:m></mc:Choice><mc:Fallback><a:r><a:rPr lang=\"en-US\"/><a:t>x^2</a:t></a:r></mc:Fallback></mc:AlternateContent></a:p>",
        );
        Ok(out.replacen("textlink=\"\"", "textlink=\"$A$1\"", 1))
    })
    .unwrap();
    let DrawingKind::Shape { xml, .. } = &*named(&wb, 0, "Revenue box").kind else {
        panic!()
    };
    assert!(xml.contains("<a:t>Live value</a:t>"), "{xml}");
    // Text with fields isn't edited as plain lines (that would lose them).
    assert!(wb.shape_text(key).is_none());
    assert!(wb.set_shape_text(key, "plain").is_err());
    // A row inserted above: the link follows its cell.
    wb.insert_rows(0, 0, 1).unwrap();
    wb.set_input(0, 1, 0, "Moved value").unwrap();
    let DrawingKind::Shape { xml, .. } = &*named(&wb, 0, "Revenue box").kind else {
        panic!()
    };
    assert!(xml.contains("<a:t>Moved value</a:t>"), "{xml}");
    let path = scratch("linked.xlsx");
    wb.save_as(&path).unwrap();
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let p = parts(&path);
    let d = p.values().find(|x| x.contains("Revenue box")).unwrap();
    assert!(
        d.contains("type=\"TxLink\"") && d.contains("<a14:m>"),
        "{d}"
    );
    assert!(
        d.contains("textlink=\"$A$2\""),
        "the link follows its cell: {d}"
    );
}
