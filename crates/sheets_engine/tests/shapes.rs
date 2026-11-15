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
        let DrawingKind::Shape { xml } = &*d.kind else {
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
