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
        num_fmt: None,
        border: None,
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
        num_fmt: None,
        border: None,
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
            bar: Default::default(),
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
            steps: Vec::new(),
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

/// Manage Rules showing the selection's rules: Move Up/Down passes the
/// rules that aren't shown (Excel), which keep their own order.
#[test]
fn moving_a_shown_rule_skips_rules_not_shown() {
    let mut wb = data_book();
    let gt = |v: &str| {
        rule(CfSpec::CellValue {
            op: CfCompare::GreaterThan,
            value: v.into(),
            value2: String::new(),
        })
    };
    // Added last = first in the list: D (A1:A10), C (C1:C3), B (C1:C3), A (A1:A10).
    wb.add_cf_rule(0, "A1:A10", &gt("1")).unwrap();
    wb.add_cf_rule(0, "C1:C3", &gt("2")).unwrap();
    wb.add_cf_rule(0, "C1:C3", &gt("3")).unwrap();
    wb.add_cf_rule(0, "A1:A10", &gt("4")).unwrap();
    let order = |wb: &Workbook| -> Vec<String> {
        wb.cf_rules(0, None)
            .iter()
            .map(|r| r.summary.rsplit(' ').next().unwrap().to_owned())
            .collect()
    };
    assert_eq!(order(&wb), ["4", "3", "2", "1"]);
    let sel = Some(sheets_engine::CellRange::from_corners((0, 0), (9, 0)));
    let shown = wb.cf_rules(0, sel);
    assert_eq!(shown.len(), 2);
    // "1" up: past the hidden "2" and "3" and above the shown "4".
    let one = shown[1].index;
    wb.move_cf_rule_within(0, one, true, sel).unwrap();
    assert_eq!(order(&wb), ["1", "4", "3", "2"]);
    // One undo step puts it back.
    wb.undo().unwrap();
    assert_eq!(order(&wb), ["4", "3", "2", "1"]);
    // "4" down: below "1", hidden ones keep their order above it.
    let four = wb.cf_rules(0, sel)[0].index;
    wb.move_cf_rule_within(0, four, false, sel).unwrap();
    assert_eq!(order(&wb), ["3", "2", "1", "4"]);
    // Already last of the shown rules: nothing.
    let four = wb.cf_rules(0, sel)[1].index;
    wb.move_cf_rule_within(0, four, false, sel).unwrap();
    assert_eq!(order(&wb), ["3", "2", "1", "4"]);
}

#[test]
fn text_equal_to_number_formats_and_borders() {
    let mut wb = data_book();
    let mut def = rule(CfSpec::Text {
        op: CfTextOp::Equals,
        text: "Ani".into(),
    });
    def.num_fmt = Some("0.00%".into());
    def.border = Some(sheets_engine::outline_border(0xC00000));
    wb.add_cf_rule(0, "B1:B10", &def).unwrap();
    // "Ani" (B1, B4) gets the fill; "Budi" doesn't.
    assert!(
        wb.conditional(0, 0, 1)
            .and_then(|c| c.style.clone())
            .is_some()
    );
    assert!(wb.conditional(0, 1, 1).is_none_or(|c| c.style.is_none()));
    let rules = wb.cf_rules(0, None);
    assert_eq!(rules[0].def.as_ref(), Some(&def));

    let path = scratch("text_equal_numfmt_border.xlsx");
    wb.save_as(&path).unwrap();
    check_cf(&path);
    // Excel has no "equal to" text rule: it's saved as the formula Excel
    // uses for one (`B1="Ani"`), with its number format and borders.
    let mut back = Workbook::open(&path).unwrap();
    let rules = back.cf_rules(0, None);
    let read = rules[0].def.clone().unwrap();
    assert_eq!(read.num_fmt, def.num_fmt);
    assert_eq!(read.border, def.border);
    // Kantor reads that formula back as the rule it was made as.
    assert_eq!(read.spec, def.spec);
    assert_eq!(rules[0].summary, "Text equal to \"Ani\"");
    assert!(
        back.conditional(0, 3, 1)
            .and_then(|c| c.style.clone())
            .is_some()
    );

    // Quotes in the text survive; other formulas stay formulas.
    let quoted = CfSpec::Text {
        op: CfTextOp::Equals,
        text: "say \"hi\"".into(),
    };
    back.add_cf_rule(0, "C1:C3", &rule(quoted.clone())).unwrap();
    let other = CfSpec::Formula {
        formula: "=B1=\"a\"&\"b\"".into(),
    };
    back.add_cf_rule(0, "B1:B10", &rule(other.clone())).unwrap();
    let elsewhere = CfSpec::Formula {
        formula: "=C1=\"Ani\"".into(),
    };
    back.add_cf_rule(0, "B1:B10", &rule(elsewhere.clone()))
        .unwrap();
    back.save_as(&path).unwrap();
    let again = Workbook::open(&path).unwrap();
    let specs: Vec<CfSpec> = again
        .cf_rules(0, None)
        .into_iter()
        .filter_map(|r| r.def.map(|d| d.spec))
        .collect();
    for want in [&def.spec, &quoted, &other, &elsewhere] {
        assert!(specs.contains(want), "{want:?} not in {specs:?}");
    }
}

/// Manage Rules shows an icon set's icons and a rating's levels, lowest
/// first as cells get them (reversed sets reversed), and no icons for other
/// rules.
#[test]
fn rule_list_carries_icons() {
    let mut wb = data_book();
    let plain = |spec| CfRuleDef {
        num_fmt: None,
        border: None,
        spec,
        format: CfFormat::default(),
        stop_if_true: false,
    };
    let icons = |set: &str, reverse: bool| CfSpec::IconSet {
        set: set.into(),
        reverse,
        show_value: true,
        steps: Vec::new(),
    };
    wb.add_cf_rule(0, "A1:A10", &plain(icons("3Arrows", false)))
        .unwrap();
    wb.add_cf_rule(0, "B1:B10", &plain(icons("3Arrows", true)))
        .unwrap();
    wb.add_cf_rule(0, "C1:C10", &plain(icons("4Rating", false)))
        .unwrap();
    wb.add_cf_rule(
        0,
        "D1:D10",
        &plain(CfSpec::ColorScale {
            colors: vec![0xF8696B, 0x63BE7B],
        }),
    )
    .unwrap();
    let rules = wb.cf_rules(0, None);
    let of = |range: &str| {
        rules
            .iter()
            .find(|r| r.range == range)
            .unwrap()
            .icons
            .clone()
    };
    let arrows = of("A1:A10");
    assert_eq!(arrows.len(), 3);
    assert!(arrows.iter().all(Option::is_some));
    assert_eq!(arrows[0], wb.set_icon("3Arrows", 0));
    assert_eq!(arrows[2], wb.set_icon("3Arrows", 2));
    let mut reversed = of("B1:B10");
    reversed.reverse();
    assert_eq!(reversed, arrows);
    let rating = of("C1:C10");
    assert_eq!(rating.len(), 4);
    assert!(rating.iter().all(Option::is_some));
    assert_ne!(rating[0], rating[3]);
    assert!(of("D1:D10").is_empty());
}
