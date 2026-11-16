//! Excel 2007 conditional formats in `.xls` (`CONDFMT12`/`CF12`, `CFEX`):
//! `tests/corpus/xls/cf12.xls`, written by `scripts/corpus/gen_xls.py
//! --cf12` from the [MS-XLS] byte layouts (see its docstring for each
//! column's rule). Rules come in as Kantor's own kinds, in priority order,
//! draw, and save to `.xlsx` as Excel writes them.

use std::path::{Path, PathBuf};

use sheets_engine::{IconShape, Workbook};

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/xls")
        .join(name)
}

fn open() -> Workbook {
    Workbook::open(corpus("cf12.xls")).unwrap()
}

#[test]
fn every_rule_is_imported_in_priority_order() {
    let wb = open();
    assert!(
        wb.import_notes().iter().all(|n| !n.contains("conditional")),
        "{:?}",
        wb.import_notes()
    );
    let rules = wb.cf_rules(0, None);
    let got: Vec<(String, String)> = rules
        .iter()
        .map(|r| (r.range.clone(), r.summary.clone()))
        .collect();
    let ranges: Vec<&str> = got.iter().map(|(r, _)| r.as_str()).collect();
    // The Excel 97 rule (no priority) first, then by CF12 / CFEX priority.
    assert_eq!(
        ranges,
        [
            "I1:I10", "A1:A10", "B1:B10", "C1:C10", "D1:D10", "E1:E10", "F1:F10", "G1:G10",
            "H1:H10", "J1:J10", "K1:K10", "L1:L10", "M1:M10", "M1:M10"
        ],
        "{got:#?}"
    );
    let summary = |range: &str| {
        got.iter()
            .find(|(r, _)| r == range)
            .map(|(_, s)| s.clone())
            .unwrap()
    };
    assert_eq!(summary("I1:I10"), "Cell value greater than 5");
    assert_eq!(summary("J1:J10"), "Cell value between 3 and 6");
    assert!(
        rules
            .iter()
            .find(|r| r.range == "J1:J10")
            .unwrap()
            .stop_if_true
    );
    for (range, kind) in [
        ("A1:A10", "Color Scale"),
        ("B1:B10", "Data Bar"),
        ("C1:C10", "Icon Set"),
        ("E1:E10", "containing \"ap\""),
        ("F1:F10", "Top 3"),
        ("G1:G10", "blank"),
        ("H1:H10", "Duplicate"),
        ("L1:L10", "Yesterday"),
    ] {
        assert!(
            summary(range).to_lowercase().contains(&kind.to_lowercase()),
            "{range}: {}",
            summary(range)
        );
    }
}

#[test]
fn the_rules_draw() {
    let mut wb = open();
    let fill = |wb: &mut Workbook, r: u32, c: u32| {
        wb.conditional(0, r, c)
            .and_then(|x| x.style.as_ref().and_then(|s| s.fill))
    };
    // A: colour scale from F8696B (10) through yellow (the 50th
    // percentile) to the theme colour.
    assert_eq!(fill(&mut wb, 0, 0), Some(0xF8696B));
    let top = fill(&mut wb, 9, 0).unwrap();
    assert_ne!(top, 0xF8696B);
    // B: a data bar, longest on the highest value.
    let bar = |wb: &mut Workbook, r: u32| wb.conditional(0, r, 1).and_then(|x| x.bar);
    let (b1, b10) = (bar(&mut wb, 0).unwrap(), bar(&mut wb, 9).unwrap());
    assert_eq!(b10.color, 0x638EC6);
    assert!(b10.to - b10.from > b1.to - b1.from);
    // C: traffic lights by percent: 1 red, 5 yellow, 10 green.
    let icon = |wb: &mut Workbook, r: u32, c: u32| wb.conditional(0, r, c).and_then(|x| x.icon);
    let lights: Vec<_> = [0, 4, 9]
        .iter()
        .map(|r| icon(&mut wb, *r, 2).unwrap().color)
        .collect();
    assert_eq!(lights.len(), 3);
    assert!(lights[0] != lights[1] && lights[1] != lights[2]);
    // D: arrows reversed (low values get the up arrow), icon only; 4 is
    // not above the strict "> 4" threshold.
    assert_eq!(icon(&mut wb, 0, 3).unwrap().shape, IconShape::ArrowUp);
    assert_eq!(icon(&mut wb, 9, 3).unwrap().shape, IconShape::ArrowDown);
    assert_eq!(icon(&mut wb, 3, 3), icon(&mut wb, 0, 3));
    assert_eq!(icon(&mut wb, 4, 3).unwrap().shape, IconShape::ArrowRight);
    // E: "apple", "grape", "papaya" and "apricot" contain "ap".
    let red = Some(0xFF0000);
    let hits: Vec<u32> = (0..10).filter(|r| fill(&mut wb, *r, 4) == red).collect();
    assert_eq!(hits, [0, 2, 3, 5]);
    // F: top 3 of 1..10.
    let yellow = Some(0xFFFF00);
    let hits: Vec<u32> = (0..10).filter(|r| fill(&mut wb, *r, 5) == yellow).collect();
    assert_eq!(hits, [7, 8, 9]);
    // G: blanks are rows 2, 5, 8.
    let hits: Vec<u32> = (0..10).filter(|r| fill(&mut wb, *r, 6) == red).collect();
    assert_eq!(hits, [1, 4, 7]);
    // H: 2 and 4 are duplicated.
    let hits: Vec<u32> = (0..10).filter(|r| fill(&mut wb, *r, 7) == yellow).collect();
    assert_eq!(hits, [1, 2, 4, 5]);
    // I: > 5; J: between 3 and 6.
    let hits: Vec<u32> = (0..10).filter(|r| fill(&mut wb, *r, 8) == red).collect();
    assert_eq!(hits, [5, 6, 7, 8, 9]);
    let hits: Vec<u32> = (0..10).filter(|r| fill(&mut wb, *r, 9) == red).collect();
    assert_eq!(hits, [2, 3, 4, 5]);
    // K: a rating.
    assert!(icon(&mut wb, 9, 10).is_some());
    // M: even numbers filled, and flags on every cell (the CF12 the CFEX
    // put in the same block).
    let hits: Vec<u32> = (0..10)
        .filter(|r| fill(&mut wb, *r, 12) == yellow)
        .collect();
    assert_eq!(hits, [1, 3, 5, 7, 9]);
    assert!((0..10).all(|r| icon(&mut wb, r, 12).is_some()));
}

#[test]
fn saved_as_xlsx_the_rules_are_excel_2007_rules() {
    let mut wb = open();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("cf12.xlsx");
    wb.save_as(&out).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
    let mut sheet = String::new();
    std::io::Read::read_to_string(
        &mut zip.by_name("xl/worksheets/sheet1.xml").unwrap(),
        &mut sheet,
    )
    .unwrap();
    for kind in [
        "type=\"colorScale\"",
        "type=\"dataBar\"",
        "type=\"iconSet\"",
        "iconSet=\"3TrafficLights1\"",
        "iconSet=\"3Arrows\"",
        "showValue=\"0\"",
        "reverse=\"1\"",
        "type=\"containsText\"",
        "type=\"top10\"",
        "type=\"containsBlanks\"",
        "type=\"duplicateValues\"",
        "timePeriod=\"yesterday\"",
        "stopIfTrue=\"1\"",
    ] {
        assert!(sheet.contains(kind), "{kind} missing: {sheet}");
    }
    let mut back = Workbook::open(&out).unwrap();
    assert_eq!(back.cf_rules(0, None).len(), 14);
    // The reversed arrows stay reversed.
    let icon = |wb: &mut Workbook, r: u32| wb.conditional(0, r, 3).and_then(|x| x.icon);
    assert_eq!(icon(&mut back, 0).unwrap().shape, IconShape::ArrowUp);
}

/// Prints the conditional formats Kantor reads from the `.xls` files named
/// in `KANTOR_CF12_FILES` (`:`-separated; files written by Excel, kept out
/// of the repository), for checking the reader against real records.
#[test]
#[ignore]
fn probe_files() {
    let Ok(files) = std::env::var("KANTOR_CF12_FILES") else {
        return;
    };
    for f in files.split(':') {
        let mut wb = Workbook::open(f).unwrap();
        println!("{f}: notes {:?}", wb.import_notes());
        for sheet in 0..wb.sheet_names().len() as u32 {
            for r in wb.cf_rules(sheet, None) {
                println!(
                    "  sheet {sheet} {} {} stop={}",
                    r.range, r.summary, r.stop_if_true
                );
            }
            let _ = wb.conditional(sheet, 0, 0);
        }
    }
}
