//! SmartArt laid out by Kantor (tests/corpus/pptx/smartart.pptx from
//! scripts/corpus/gen_pptx_smartart.py): resized SmartArt laid out again,
//! items added and removed in lists, processes, cycles and hierarchies, refused in
//! other layouts; saved and read back.

use std::collections::BTreeMap;
use std::io::{Cursor, Read as _};
use std::path::PathBuf;

use slides_engine::{AddPlace, Presentation, Selection, ShapeRef, SmartArtFamily, text_ops};

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/pptx/smartart.pptx")
}

fn parts(bytes: &[u8]) -> BTreeMap<String, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        let name = f.name().unwrap().into_owned();
        let mut v = Vec::new();
        f.read_to_end(&mut v).unwrap();
        out.insert(name, String::from_utf8_lossy(&v).into_owned());
    }
    out
}

fn frame(p: &Presentation, slide: usize) -> ShapeRef {
    let i = p
        .slide(slide)
        .shapes
        .iter()
        .position(|s| s.smartart().is_some())
        .expect("SmartArt");
    (slide, i).into()
}

/// The drawing's shapes' texts and boxes.
fn drawn(p: &Presentation, f: ShapeRef) -> Vec<(String, slides_engine::Xfrm)> {
    let g = &p.shape(f).unwrap().smartart().unwrap().group;
    g.children
        .iter()
        .map(|c| {
            (
                c.text.as_ref().map(|t| t.plain_text()).unwrap_or_default(),
                c.xfrm.unwrap(),
            )
        })
        .collect()
}

fn find(p: &Presentation, f: ShapeRef, text: &str) -> ShapeRef {
    let i = drawn(p, f)
        .iter()
        .position(|(t, _)| t == text)
        .unwrap_or_else(|| panic!("{text} in {:?}", drawn(p, f)));
    f.child(i).unwrap()
}

#[test]
fn families_are_recognised() {
    let p = Presentation::open(&corpus()).unwrap();
    let fam: Vec<Option<SmartArtFamily>> =
        (0..5).map(|s| p.smartart_family(frame(&p, s))).collect();
    assert_eq!(
        fam,
        [
            Some(SmartArtFamily::Blocks),
            Some(SmartArtFamily::Bullets),
            Some(SmartArtFamily::Process),
            Some(SmartArtFamily::Cycle),
            None
        ]
    );
}

#[test]
fn resized_smartart_is_laid_out_again_not_scaled() {
    let mut p = Presentation::open(&corpus()).unwrap();
    let f = frame(&p, 0);
    let mut x = p.shape(f).unwrap().xfrm.unwrap();
    // Much wider and lower: the five blocks go on one row, still 10:6.
    x.cx *= 3;
    x.cy /= 2;
    p.set_xfrm(f, x, false).unwrap();
    let d = drawn(&p, f);
    assert_eq!(d.len(), 5);
    assert!(d.iter().all(|(_, b)| b.y == d[0].1.y), "{d:?}");
    for (_, b) in &d {
        assert!((b.cy as f64 / b.cx as f64 - 0.6).abs() < 0.01, "{b:?}");
        assert!(b.x >= 0 && b.x + b.cx <= x.cx);
    }
    // The drawing's own space is the frame's new size.
    let g = &p.shape(f).unwrap().smartart().unwrap().group;
    assert_eq!((g.child_box.cx, g.child_box.cy), (x.cx, x.cy));
    // Saved: the drawing part has the new boxes.
    let bytes = p.to_bytes().unwrap();
    let back = Presentation::from_bytes(bytes).unwrap();
    assert_eq!(drawn(&back, frame(&back, 0)), d);
    // A layout Kantor doesn't know scales as before.
    let h = frame(&p, 4);
    let before = drawn(&p, h);
    let mut hx = p.shape(h).unwrap().xfrm.unwrap();
    hx.cx *= 2;
    p.set_xfrm(h, hx, false).unwrap();
    assert_eq!(drawn(&p, h), before);
}

#[test]
fn process_items_are_added_typed_into_and_deleted() {
    let mut p = Presentation::open(&corpus()).unwrap();
    let f = frame(&p, 2);
    let build = find(&p, f, "Build");
    let new = p.smartart_add_shape(build, AddPlace::After).unwrap();
    let d = drawn(&p, f);
    // Four steps and three arrows, left to right in order.
    let steps: Vec<&(String, slides_engine::Xfrm)> =
        d.iter().filter(|(_, b)| b.cx > 1_000_000).collect();
    assert_eq!(steps.len(), 4, "{d:?}");
    assert_eq!(d.len(), 7, "{d:?}");
    let order: Vec<&str> = {
        let mut s = steps.clone();
        s.sort_by_key(|(_, b)| b.x);
        s.iter().map(|(t, _)| t.as_str()).collect()
    };
    assert_eq!(order, ["Plan", "Build", "", "Ship"]);
    // The new shape takes text, saved into the data model.
    assert!(p.smartart_text_editable(new));
    let end = text_ops::end(p.text_paras(new).unwrap());
    p.insert_text(new, Selection::caret(end), "Test", None)
        .unwrap();
    let saved = parts(&p.to_bytes().unwrap());
    let data = &saved["ppt/diagrams/data3.xml"];
    assert!(data.contains("<a:t>Test</a:t>"), "{data}");
    let drawing = &saved["ppt/diagrams/drawing3.xml"];
    assert!(drawing.contains("<a:t>Test</a:t>"));
    assert_eq!(drawing.matches("prst=\"rightArrow\"").count(), 3);
    // Read back: the new item is there, in order.
    let back = Presentation::from_bytes(p.to_bytes().unwrap()).unwrap();
    let bf = frame(&back, 2);
    let mut steps: Vec<(String, slides_engine::Xfrm)> = drawn(&back, bf)
        .into_iter()
        .filter(|(t, _)| !t.is_empty())
        .collect();
    steps.sort_by_key(|(_, b)| b.x);
    let names: Vec<&str> = steps.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(names, ["Plan", "Build", "Test", "Ship"]);
    // Delete "Plan": three steps and two arrows left.
    let plan = find(&p, f, "Plan");
    p.delete_shape(plan).unwrap();
    assert_eq!(drawn(&p, f).len(), 5);
    let saved = parts(&p.to_bytes().unwrap());
    let data = &saved["ppt/diagrams/data3.xml"];
    assert!(!data.contains("<a:t>Plan</a:t>"), "{data}");
    // Undo twice: back to the file's three steps.
    p.undo();
    p.undo();
    p.undo();
    let d = drawn(&p, f);
    assert_eq!(d.len(), 5);
}

#[test]
fn cycles_bullets_and_blocks_take_new_items() {
    let mut p = Presentation::open(&corpus()).unwrap();
    // Cycle: a fifth season, with its arrow.
    let f = frame(&p, 3);
    p.smartart_add_shape(find(&p, f, "Winter"), AddPlace::Before)
        .unwrap();
    assert_eq!(drawn(&p, f).len(), 10);
    // Bullet list: a new header between North and South.
    let b = frame(&p, 1);
    p.smartart_add_shape(find(&p, b, "North"), AddPlace::After)
        .unwrap();
    let d = drawn(&p, b);
    let y = |t: &str| d.iter().find(|(x, _)| x == t).unwrap().1.y;
    let blank = d.iter().find(|(t, _)| t.is_empty()).unwrap().1.y;
    assert!(y("Oslo\nBergen") < blank && blank < y("South"), "{d:?}");
    // One level only: Below is refused.
    assert!(
        p.smartart_add_shape(find(&p, b, "South"), AddPlace::Below)
            .is_err()
    );
    // Blocks: the frame alone adds after the last item.
    let k = frame(&p, 0);
    p.smartart_add_shape(k, AddPlace::After).unwrap();
    assert_eq!(drawn(&p, k).len(), 6);
    // Other layouts refuse, with a message.
    let h = frame(&p, 4);
    let e = p
        .smartart_add_shape(find(&p, h, "Team"), AddPlace::After)
        .unwrap_err();
    assert!(e.to_string().contains("can't lay out"), "{e}");
    assert!(p.delete_shape(find(&p, h, "Team")).is_err());
    // All of it saves as valid parts and reads back. For a look (and the
    // Office validator): KANTOR_SMARTART_OUT=/path/out.pptx.
    let bytes = p.to_bytes().unwrap();
    if let Ok(out) = std::env::var("KANTOR_SMARTART_OUT") {
        std::fs::write(out, &bytes).unwrap();
    }
    let back = Presentation::from_bytes(bytes).unwrap();
    assert_eq!(drawn(&back, frame(&back, 3)).len(), 10);
    assert_eq!(drawn(&back, frame(&back, 0)).len(), 6);
}

#[test]
fn smartart_shapes_turn_and_keep_their_turn_when_laid_out() {
    let mut p = Presentation::open(&corpus()).unwrap();
    let f = frame(&p, 2);
    let build = find(&p, f, "Build");
    let at = p.placed(build).unwrap().xfrm;
    p.set_xfrm(build, at.with_degrees(30.0), false).unwrap();
    assert_eq!(p.shape(build).unwrap().xfrm.unwrap().rot, 30 * 60_000);
    let saved = parts(&p.to_bytes().unwrap());
    let drawing = &saved["ppt/diagrams/drawing3.xml"];
    // The shape and its text box turned; PowerPoint's custom angle on its
    // presentation item.
    assert!(drawing.contains("<a:xfrm rot=\"1800000\">"), "{drawing}");
    assert!(
        drawing.contains("<dsp:txXfrm rot=\"1800000\">"),
        "{drawing}"
    );
    assert!(saved["ppt/diagrams/data3.xml"].contains("custAng=\"1800000\""));
    // Laid out again (resized), the turn stays.
    let mut x = p.shape(f).unwrap().xfrm.unwrap();
    x.cx = x.cx * 3 / 2;
    p.set_xfrm(f, x, false).unwrap();
    let b = find(&p, f, "Build");
    assert_eq!(p.shape(b).unwrap().xfrm.unwrap().rot, 30 * 60_000);
}

/// Item boxes of a hierarchy SmartArt by text (connectors have none).
fn items(p: &Presentation, f: ShapeRef) -> BTreeMap<String, slides_engine::Xfrm> {
    drawn(p, f)
        .into_iter()
        .filter(|(t, _)| !t.is_empty())
        .collect()
}

#[test]
fn hierarchies_lay_out_as_trees() {
    let mut p = Presentation::open(&corpus()).unwrap();
    let fam: Vec<Option<SmartArtFamily>> =
        (5..8).map(|s| p.smartart_family(frame(&p, s))).collect();
    assert_eq!(
        fam,
        [
            Some(SmartArtFamily::OrgChart),
            Some(SmartArtFamily::Hierarchy),
            Some(SmartArtFamily::HorizontalHierarchy)
        ]
    );
    for s in 5..8 {
        let f = frame(&p, s);
        let mut x = p.shape(f).unwrap().xfrm.unwrap();
        x.cx = x.cx * 3 / 2;
        p.set_xfrm(f, x, false).unwrap();
        let b = items(&p, f);
        let mid = |t: &str| (b[t].x + b[t].cx / 2, b[t].y + b[t].cy / 2);
        if s == 7 {
            // Left to right: CEO, then CFO and CTO, then Dev and Ops.
            assert!(
                mid("CEO").0 < mid("CTO").0 && mid("CTO").0 < mid("Dev").0,
                "{b:?}"
            );
            assert!(mid("Dev").1 < mid("Ops").1);
        } else {
            assert!(
                mid("CEO").1 < mid("CTO").1 && mid("CTO").1 < mid("Dev").1,
                "{b:?}"
            );
            // CTO centred over Dev and Ops.
            assert!(
                (mid("CTO").0 - (mid("Dev").0 + mid("Ops").0) / 2).abs() < 2,
                "{b:?}"
            );
        }
        for v in b.values() {
            assert!(v.x >= 0 && v.x + v.cx <= x.cx + 1, "{v:?}");
        }
    }
}

#[test]
fn hierarchy_shapes_are_added_above_below_and_deleted() {
    let mut p = Presentation::open(&corpus()).unwrap();
    let f = frame(&p, 5);
    let n0 = drawn(&p, f).len();
    // Below CFO: a new box under it, with a connector.
    let new = p
        .smartart_add_shape(find(&p, f, "CFO"), AddPlace::Below)
        .unwrap();
    let end = text_ops::end(p.text_paras(new).unwrap());
    p.insert_text(new, Selection::caret(end), "Audit", None)
        .unwrap();
    let b = items(&p, f);
    assert!(b["Audit"].y > b["CFO"].y, "{b:?}");
    assert_eq!(drawn(&p, f).len(), n0 + 2);
    // Above CEO: a new top box.
    let top = p
        .smartart_add_shape(find(&p, f, "CEO"), AddPlace::Above)
        .unwrap();
    let end = text_ops::end(p.text_paras(top).unwrap());
    p.insert_text(top, Selection::caret(end), "Board", None)
        .unwrap();
    let b = items(&p, f);
    assert!(b["Board"].y < b["CEO"].y, "{b:?}");
    assert_eq!(drawn(&p, f).len(), n0 + 4);
    // Delete CTO: Dev and Ops move up beside CFO.
    p.delete_shape(find(&p, f, "CTO")).unwrap();
    let b = items(&p, f);
    assert!(!b.contains_key("CTO"));
    assert_eq!(b["Dev"].y, b["CFO"].y, "{b:?}");
    // Saved: the data model has the new items and the tree, and reads back
    // the same.
    let bytes = p.to_bytes().unwrap();
    if let Ok(out) = std::env::var("KANTOR_SMARTART_TREE_OUT") {
        std::fs::write(out, &bytes).unwrap();
    }
    let saved = parts(&bytes);
    let data = &saved["ppt/diagrams/data6.xml"];
    assert!(data.contains("<a:t>Audit</a:t>") && data.contains("<a:t>Board</a:t>"));
    assert!(!data.contains("<a:t>CTO</a:t>"));
    let back = Presentation::from_bytes(bytes).unwrap();
    assert_eq!(items(&back, frame(&back, 5)), b);
    // Hierarchies with a background and a text box per item: both copied.
    let h = frame(&p, 6);
    let n = drawn(&p, h).len();
    p.smartart_add_shape(find(&p, h, "Ops"), AddPlace::After)
        .unwrap();
    assert_eq!(drawn(&p, h).len(), n + 3);
}

/// The `fontScale` of each shape's text (100 000 = as written).
fn scales(p: &Presentation, f: ShapeRef) -> Vec<(String, u32)> {
    let g = &p.shape(f).unwrap().smartart().unwrap().group;
    g.children
        .iter()
        .filter_map(|c| {
            let t = c.text.as_ref().filter(|t| !t.is_empty())?;
            Some((t.plain_text(), t.body.font_scale.unwrap_or(100_000)))
        })
        .collect()
}

#[test]
fn text_shrinks_to_fit_when_laid_out_smaller_and_grows_back() {
    let mut p = Presentation::open(&corpus()).unwrap();
    let f = frame(&p, 0);
    assert!(scales(&p, f).iter().all(|(_, s)| *s == 100_000));
    let a = find(&p, f, "A");
    let end = text_ops::end(p.text_paras(a).unwrap());
    p.insert_text(
        a,
        Selection::caret(end),
        " is a much longer item name",
        None,
    )
    .unwrap();
    let orig = p.shape(f).unwrap().xfrm.unwrap();
    // A third of the size: that text no longer fits as written.
    let mut x = orig;
    x.cx /= 3;
    x.cy /= 3;
    p.set_xfrm(f, x, false).unwrap();
    let small = scales(&p, f);
    assert!(small.iter().all(|(_, s)| *s < 100_000), "{small:?}");
    // The blocks' text is one size, as Office keeps it.
    assert!(small.iter().all(|(_, s)| *s == small[0].1), "{small:?}");
    let bytes = p.to_bytes().unwrap();
    let saved = parts(&bytes);
    let drawing = &saved["ppt/diagrams/drawing1.xml"];
    assert!(
        drawing.contains(&format!("<a:normAutofit fontScale=\"{}\"/>", small[0].1)),
        "{drawing}"
    );
    let back = Presentation::from_bytes(bytes).unwrap();
    assert_eq!(scales(&back, frame(&back, 0)), small);
    // Back to its size: the text is as written again.
    p.set_xfrm(f, orig, false).unwrap();
    assert!(
        scales(&p, f).iter().all(|(_, s)| *s == 100_000),
        "{:?}",
        scales(&p, f)
    );
    // Undo goes back to the small layout with its small text.
    p.undo();
    assert_eq!(scales(&p, f), small);
}

#[test]
fn labeled_hierarchy_lays_out_bands_and_takes_shapes() {
    let mut p = Presentation::open(&corpus()).unwrap();
    let f = frame(&p, 8);
    assert_eq!(p.smartart_family(f), Some(SmartArtFamily::LabeledHierarchy));
    let mut x = p.shape(f).unwrap().xfrm.unwrap();
    x.cx = x.cx * 3 / 2;
    p.set_xfrm(f, x, false).unwrap();
    let b = items(&p, f);
    let mid = |t: &str| (b[t].x + b[t].cx / 2, b[t].y + b[t].cy / 2);
    // Bands across the whole width, one under another, top down.
    for l in ["Board", "Managers", "Staff"] {
        assert_eq!((b[l].x, b[l].cx), (0, x.cx), "{l}: {b:?}");
    }
    assert!(b["Board"].y < b["Managers"].y && b["Managers"].y < b["Staff"].y);
    // Each level of the tree in its band, right of the labels.
    let inside = |item: &str, band: &str| {
        let (_, y) = mid(item);
        y > b[band].y && y < b[band].y + b[band].cy
    };
    assert!(inside("CEO", "Board"), "{b:?}");
    assert!(inside("CTO", "Managers") && inside("CFO", "Managers"));
    assert!(inside("Dev", "Staff") && inside("Ops", "Staff"));
    for t in ["CEO", "CFO", "CTO", "Dev", "Ops"] {
        assert!(b[t].x >= x.cx / 5, "{t} right of the labels: {b:?}");
        assert!(b[t].x + b[t].cx <= x.cx + 1);
    }
    // The bands' text stays at their left.
    let g = &p.shape(f).unwrap().smartart().unwrap().group;
    let k = g
        .children
        .iter()
        .position(|c| c.text.as_ref().is_some_and(|t| t.plain_text() == "Board"))
        .unwrap();
    let tb = g.text_boxes[k].unwrap();
    assert!(tb.x == 0 && tb.cx <= x.cx / 4, "{tb:?}");

    // A new label after Staff: a fourth band, under it.
    let n0 = drawn(&p, f).len();
    let new = p
        .smartart_add_shape(find(&p, f, "Staff"), AddPlace::After)
        .unwrap();
    let end = text_ops::end(p.text_paras(new).unwrap());
    p.insert_text(new, Selection::caret(end), "Interns", None)
        .unwrap();
    let b = items(&p, f);
    assert!(
        b["Interns"].y > b["Staff"].y && b["Interns"].cx == x.cx,
        "{b:?}"
    );
    assert_eq!(drawn(&p, f).len(), n0 + 1);
    // Below Dev: a fourth level, in the fourth band.
    let qa = p
        .smartart_add_shape(find(&p, f, "Dev"), AddPlace::Below)
        .unwrap();
    let end = text_ops::end(p.text_paras(qa).unwrap());
    p.insert_text(qa, Selection::caret(end), "QA", None)
        .unwrap();
    let b = items(&p, f);
    let (_, y) = (b["QA"].x, b["QA"].y + b["QA"].cy / 2);
    assert!(
        y > b["Interns"].y && y < b["Interns"].y + b["Interns"].cy,
        "{b:?}"
    );
    // Refused: shapes above or below a label, beside the top, deleting a
    // top with two shapes under it.
    let e = p
        .smartart_add_shape(find(&p, f, "Board"), AddPlace::Below)
        .unwrap_err();
    assert_eq!(e.to_string(), slides_engine::smartart_layout::LABEL_LEVELS);
    let e = p
        .smartart_add_shape(find(&p, f, "CEO"), AddPlace::After)
        .unwrap_err();
    assert_eq!(e.to_string(), slides_engine::smartart_layout::TOP_ALONE);
    let e = p.delete_shape(find(&p, f, "CEO")).unwrap_err();
    assert_eq!(e.to_string(), slides_engine::smartart_layout::TOP_KEEP);
    // A label deleted: the others close up.
    p.delete_shape(find(&p, f, "Managers")).unwrap();
    let b = items(&p, f);
    assert!(!b.contains_key("Managers"));
    // Saved and read back the same.
    let bytes = p.to_bytes().unwrap();
    if let Ok(out) = std::env::var("KANTOR_SMARTART_LABEL_OUT") {
        std::fs::write(out, &bytes).unwrap();
    }
    let back = Presentation::from_bytes(bytes).unwrap();
    assert_eq!(items(&back, frame(&back, 8)), b);
}
