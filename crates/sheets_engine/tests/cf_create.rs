//! Conditional formatting rules made and managed in Kantor: each change is
//! one undo step, the grid shows it, a save writes rules and dxfs Excel can
//! read (read back the same), and sheets nobody touched keep the file's
//! markup byte for byte.

#[path = "support/pivot_check.rs"]
mod pivot_check;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use pivot_check::{el, els, parts};
use roxmltree::Document;
use sheets_engine::{CfCompare, CfFormat, CfPeriod, CfRuleDef, CfSpec, CfTextOp, Workbook};

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cf_create");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/xlsx")
        .join(name)
}

fn rule(spec: CfSpec) -> CfRuleDef {
    CfRuleDef {
        spec,
        format: CfFormat::PRESETS[0].1,
        stop_if_true: false,
    }
}

/// Numbers 1..=10 in A1:A10, names in B1:B10, dates in C1:C3.
fn data_book() -> Workbook {
    let mut wb = Workbook::new_empty().unwrap();
    let names = [
        "Ani", "Budi", "Citra", "Ani", "Dewi", "Eko", "Budi", "Fajar", "Gita", "Hadi",
    ];
    let mut cells = Vec::new();
    for i in 0..10u32 {
        cells.push((i, 0, (i + 1).to_string()));
        cells.push((i, 1, names[i as usize].to_string()));
    }
    cells.push((0, 2, "=TODAY()".into()));
    wb.set_inputs(0, &cells).unwrap();
    wb
}

/// The checks Excel makes first on conditional formats: each rule has a
/// type and a unique priority, dxf ids exist, formulas have no `=`.
fn check_cf(path: &Path) -> Vec<(String, String)> {
    let p = parts(path);
    assert!(sheets_engine::package_problems(&std::fs::read(path).unwrap()).is_empty());
    let styles = Document::parse(&p["xl/styles.xml"]).unwrap();
    let dxfs = styles
        .descendants()
        .find(|n| n.tag_name().name() == "dxfs")
        .map(|d| els(d, "dxf").count())
        .unwrap_or(0);
    let mut out = Vec::new();
    for (name, xml) in &p {
        if !(name.starts_with("xl/worksheets/sheet") && name.ends_with(".xml")) {
            continue;
        }
        let doc = Document::parse(xml).unwrap();
        let mut priorities = HashSet::new();
        for cf in els(doc.root_element(), "conditionalFormatting") {
            assert!(cf.attribute("sqref").is_some());
            for r in els(cf, "cfRule") {
                let ty = r.attribute("type").expect("type").to_owned();
                let pr = r.attribute("priority").expect("priority");
                assert!(priorities.insert(pr.to_owned()), "priority {pr} twice");
                if let Some(d) = r.attribute("dxfId") {
                    assert!(d.parse::<usize>().unwrap() < dxfs, "dxfId {d} of {dxfs}");
                }
                for f in els(r, "formula") {
                    assert!(!f.text().unwrap_or_default().starts_with('='));
                }
                out.push((ty, cf.attribute("sqref").unwrap().to_owned()));
            }
        }
    }
    out
}

#[test]
fn highlight_rules_show_undo_and_save() {
    let mut wb = data_book();
    let gt = rule(CfSpec::CellValue {
        op: CfCompare::GreaterThan,
        value: "7".into(),
        value2: String::new(),
    });
    wb.add_cf_rule(0, "A1:A10", &gt).unwrap();
    let c = wb.conditional(0, 8, 0).expect("A9 > 7");
    assert_eq!(c.style.as_ref().unwrap().fill, Some(0xFFC7CE));
    assert!(wb.conditional(0, 2, 0).is_none_or(|c| c.style.is_none()));
    assert!(wb.undo().unwrap());
    assert!(wb.cf_rules(0, None).is_empty());
    assert!(wb.redo().unwrap());
    assert_eq!(wb.cf_rules(0, None).len(), 1);

    let between = rule(CfSpec::CellValue {
        op: CfCompare::Between,
        value: "2".into(),
        value2: "4".into(),
    });
    wb.add_cf_rule(0, "A1:A10", &between).unwrap();
    wb.add_cf_rule(
        0,
        "B1:B10",
        &rule(CfSpec::Text {
            op: CfTextOp::Contains,
            text: "an".into(),
        }),
    )
    .unwrap();
    wb.add_cf_rule(0, "B1:B10", &rule(CfSpec::Duplicates { unique: false }))
        .unwrap();
    wb.add_cf_rule(
        0,
        "C1:C3",
        &rule(CfSpec::Dates {
            period: CfPeriod::Today,
        }),
    )
    .unwrap();
    wb.add_cf_rule(
        0,
        "A1:A10",
        &rule(CfSpec::TopBottom {
            bottom: false,
            rank: 10,
            percent: true,
        }),
    )
    .unwrap();
    wb.add_cf_rule(
        0,
        "A1:B10",
        &rule(CfSpec::Formula {
            formula: "=$A1=5".into(),
        }),
    )
    .unwrap();
    // Duplicates: Ani and Budi.
    assert!(
        wb.conditional(0, 3, 1)
            .and_then(|c| c.style.clone())
            .is_some()
    );
    // Text contains "an" (any case): Ani, Fajar? no: "Ani" has "an".
    let list = wb.cf_rules(0, None);
    assert_eq!(list.len(), 7);
    assert_eq!(list[0].summary, "Formula: =$A1=5");
    assert_eq!(list[6].summary, "Cell value greater than 7");

    let path = scratch("rules.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let types: Vec<String> = check_cf(&path).into_iter().map(|t| t.0).collect();
    for t in [
        "cellIs",
        "containsText",
        "duplicateValues",
        "timePeriod",
        "top10",
        "expression",
    ] {
        assert!(types.iter().any(|x| x == t), "{t} missing: {types:?}");
    }
    let back = Workbook::open(&path).unwrap();
    let summaries = |w: &Workbook| {
        w.cf_rules(0, None)
            .into_iter()
            .map(|r| (r.summary, r.range, r.format))
            .collect::<Vec<_>>()
    };
    assert_eq!(summaries(&back), summaries(&wb));
}

#[test]
fn bars_scales_icons_save_and_read_back() {
    let mut wb = data_book();
    let plain = |spec| CfRuleDef {
        spec,
        format: CfFormat::default(),
        stop_if_true: false,
    };
    wb.add_cf_rule(
        0,
        "A1:A10",
        &plain(CfSpec::DataBar {
            color: 0x638EC6,
            gradient: true,
        }),
    )
    .unwrap();
    wb.add_cf_rule(
        0,
        "A1:A10",
        &plain(CfSpec::ColorScale {
            colors: vec![0xF8696B, 0xFFEB84, 0x63BE7B],
        }),
    )
    .unwrap();
    wb.add_cf_rule(
        0,
        "A1:A10",
        &plain(CfSpec::IconSet {
            set: "3Arrows".into(),
            reverse: false,
            show_value: true,
        }),
    )
    .unwrap();
    let top = wb.conditional(0, 9, 0).unwrap();
    assert!(top.bar.is_some() && top.icon.is_some());
    let path = scratch("visual.xlsx");
    wb.save_as(&path).unwrap();
    let mut types: Vec<String> = check_cf(&path).into_iter().map(|t| t.0).collect();
    types.sort();
    assert_eq!(types, vec!["colorScale", "dataBar", "iconSet"]);
    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.cf_rules(0, None).len(), 3);
    let mut back = back;
    let c = back.conditional(0, 9, 0).unwrap();
    assert!(c.bar.is_some() && c.icon.is_some() && c.style.is_some());
}

#[test]
fn manage_rules_priority_stop_and_delete() {
    let mut wb = data_book();
    let a = rule(CfSpec::CellValue {
        op: CfCompare::GreaterThan,
        value: "5".into(),
        value2: String::new(),
    });
    let mut b = rule(CfSpec::CellValue {
        op: CfCompare::LessThan,
        value: "8".into(),
        value2: String::new(),
    });
    b.format = CfFormat::PRESETS[2].1;
    wb.add_cf_rule(0, "A1:A10", &a).unwrap();
    wb.add_cf_rule(0, "A1:A10", &b).unwrap();
    // b is first (added last), its green fill wins on 6 and 7.
    assert_eq!(
        wb.conditional(0, 5, 0)
            .unwrap()
            .style
            .as_ref()
            .unwrap()
            .fill,
        Some(0xC6EFCE)
    );
    let list = wb.cf_rules(0, None);
    let a_index = list[1].index;
    wb.move_cf_rule(0, a_index, true).unwrap();
    assert_eq!(
        wb.conditional(0, 5, 0)
            .unwrap()
            .style
            .as_ref()
            .unwrap()
            .fill,
        Some(0xFFC7CE)
    );
    assert_eq!(wb.cf_rules(0, None)[0].summary, "Cell value greater than 5");
    // Stop if true on the first.
    let first = wb.cf_rules(0, None)[0].clone();
    let mut def = first.def.clone().unwrap();
    def.stop_if_true = true;
    wb.update_cf_rule(0, first.index, &first.range, &def)
        .unwrap();
    assert!(wb.cf_rules(0, None)[0].stop_if_true);
    let path = scratch("managed.xlsx");
    wb.save_as(&path).unwrap();
    check_cf(&path);
    let back = Workbook::open(&path).unwrap();
    let l = back.cf_rules(0, None);
    assert_eq!(l[0].summary, "Cell value greater than 5");
    assert!(l[0].stop_if_true);
    // Clear rules from the selection.
    assert_eq!(
        wb.clear_cf_rules(
            0,
            Some(sheets_engine::CellRange::from_corners((0, 0), (0, 0)))
        )
        .unwrap(),
        2
    );
    assert!(wb.cf_rules(0, None).is_empty());
    assert!(wb.undo().unwrap());
    assert_eq!(wb.cf_rules(0, None).len(), 2);
}

#[test]
fn bad_input_is_refused() {
    let mut wb = data_book();
    assert!(
        wb.add_cf_rule(0, "nowhere", &rule(CfSpec::Blanks { not: false }))
            .is_err()
    );
    assert!(
        wb.add_cf_rule(
            0,
            "A1",
            &rule(CfSpec::Formula {
                formula: "=SUM(".into()
            })
        )
        .is_err()
    );
    assert!(wb.cf_rules(0, None).is_empty());
    assert!(!wb.can_undo() || wb.undo().is_ok());
}

#[test]
fn untouched_sheets_and_dxfs_stay_byte_identical() {
    let src = corpus("18_conditional_formatting.xlsx");
    let mut plain = Workbook::open(&src).unwrap();
    let plain_path = scratch("plain18.xlsx");
    plain.save_as(&plain_path).unwrap();
    let mut wb = Workbook::open(&src).unwrap();
    let before = wb.cf_rules(1, None).len();
    wb.add_cf_rule(
        1,
        "A1:A5",
        &rule(CfSpec::CellValue {
            op: CfCompare::EqualTo,
            value: "1".into(),
            value2: String::new(),
        }),
    )
    .unwrap();
    let path = scratch("edited18.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    check_cf(&path);
    let a = parts(&plain_path);
    let b = parts(&path);
    assert_eq!(a["xl/worksheets/sheet1.xml"], b["xl/worksheets/sheet1.xml"]);
    // The file's dxfs come first, unchanged; the new one is appended.
    let dxfs = |x: &str| -> Vec<String> {
        let d = Document::parse(x).unwrap();
        let list = d
            .descendants()
            .find(|n| n.tag_name().name() == "dxfs")
            .unwrap();
        els(list, "dxf").map(|n| x[n.range()].to_owned()).collect()
    };
    let (da, db) = (dxfs(&a["xl/styles.xml"]), dxfs(&b["xl/styles.xml"]));
    assert_eq!(db.len(), da.len() + 1);
    assert_eq!(&db[..da.len()], &da[..]);
    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.cf_rules(1, None).len(), before + 1);
    assert_eq!(back.cf_rules(0, None), plain.cf_rules(0, None));
    let doc = Document::parse(&b["xl/worksheets/sheet2.xml"]).unwrap();
    assert!(el(doc.root_element(), "conditionalFormatting").is_some());
}
