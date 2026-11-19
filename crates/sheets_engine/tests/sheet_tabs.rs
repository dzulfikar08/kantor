//! Worksheet tabs: Insert, Delete, Rename, Move or Copy, Tab Color, Hide and
//! Unhide, with undo, saving and the chart sheets of
//! tests/corpus/xlsx/22_chart_sheets.xlsx.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use sheets_engine::{
    CellRange, ErrorStyle, Message, Tab, TabColor, Validation, ValidationOperator, ValidationType,
    Workbook,
};

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/xlsx")
        .join(name)
}

fn parts(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).unwrap();
        let mut b = Vec::new();
        e.read_to_end(&mut b).unwrap();
        out.insert(e.name().unwrap().into_owned(), b);
    }
    out
}

fn text(p: &BTreeMap<String, Vec<u8>>, name: &str) -> String {
    String::from_utf8(p[name].clone()).unwrap()
}

fn save(wb: &mut Workbook, dir: &Path, name: &str) -> BTreeMap<String, Vec<u8>> {
    let path = dir.join(name);
    wb.save_as(&path).unwrap();
    let problems = sheets_engine::package_problems(&std::fs::read(&path).unwrap());
    assert!(problems.is_empty(), "{name}: {problems:?}");
    parts(&path)
}

/// Parts that differ between two saves (docProps/core.xml holds the save
/// time and is skipped).
fn changed(a: &BTreeMap<String, Vec<u8>>, b: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
    let mut out: Vec<String> = a
        .keys()
        .chain(b.keys())
        .filter(|n| *n != "docProps/core.xml" && a.get(*n) != b.get(*n))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Tab names in order (chart sheets in brackets).
fn tab_names(wb: &Workbook) -> Vec<String> {
    let names = wb.sheet_names();
    wb.tabs()
        .into_iter()
        .map(|t| match t {
            Tab::Sheet(i) => names[i as usize].clone(),
            Tab::Chart(k) => format!("[{}]", wb.chart_sheet_name(k).unwrap()),
        })
        .collect()
}

/// `<sheet name=…>` of workbook.xml in order, with `state` when hidden.
fn listed(p: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
    let wb = text(p, "xl/workbook.xml");
    let doc = roxmltree::Document::parse(&wb).unwrap();
    doc.descendants()
        .filter(|n| n.has_tag_name("sheet"))
        .map(|n| {
            let name = n.attribute("name").unwrap().to_owned();
            match n.attribute("state") {
                Some(s) => format!("{name} ({s})"),
                None => name,
            }
        })
        .collect()
}

/// The worksheet part of the sheet called `name`.
fn sheet_part(p: &BTreeMap<String, Vec<u8>>, name: &str) -> String {
    let wb = text(p, "xl/workbook.xml");
    let doc = roxmltree::Document::parse(&wb).unwrap();
    let rid = doc
        .descendants()
        .find(|n| n.has_tag_name("sheet") && n.attribute("name") == Some(name))
        .and_then(|n| {
            n.attributes()
                .find(|a| a.name() == "id")
                .map(|a| a.value().to_owned())
        })
        .unwrap();
    let rels = text(p, "xl/_rels/workbook.xml.rels");
    let doc = roxmltree::Document::parse(&rels).unwrap();
    let target = doc
        .descendants()
        .find(|n| n.attribute("Id") == Some(rid.as_str()))
        .and_then(|n| n.attribute("Target"))
        .unwrap();
    let target = target.trim_start_matches('/').trim_start_matches("xl/");
    text(p, &format!("xl/{target}"))
}

#[test]
fn insert_rename_and_delete_follow_excel() {
    let mut wb = Workbook::new_empty().unwrap();
    assert_eq!(wb.sheet_names(), ["Sheet1"]);
    // Insert puts the new sheet where it is asked (before tab `at`).
    assert_eq!(wb.insert_sheet(1).unwrap(), 1);
    assert_eq!(wb.insert_sheet(0).unwrap(), 0);
    assert_eq!(tab_names(&wb), ["Sheet3", "Sheet1", "Sheet2"]);
    assert_eq!(wb.next_sheet_name(), "Sheet4");

    wb.set_input(1, 0, 0, "5").unwrap(); // Sheet1!A1
    wb.set_input(2, 0, 0, "=Sheet1!A1*2").unwrap();
    wb.set_input(2, 1, 0, "=SUM(Sheet3!A1:A3)+1").unwrap();

    // Excel's name rules.
    for bad in [
        "", "  ", "a/b", "a:b", "x[1]", "'quoted", "quoted'", "History",
    ] {
        assert!(wb.rename_sheet(1, bad).is_err(), "{bad:?}");
    }
    assert!(wb.rename_sheet(1, &"x".repeat(32)).is_err());
    assert!(wb.rename_sheet(1, "SHEET2").is_err(), "taken in any case");
    wb.rename_sheet(1, &"x".repeat(31)).unwrap();
    wb.rename_sheet(1, "Sales 2026").unwrap();
    assert_eq!(wb.input(2, 0, 0), "='Sales 2026'!A1*2");
    assert_eq!(wb.display_value(2, 0, 0), "10");
    // Undo: back to the earlier names, formulas too.
    wb.undo().unwrap();
    wb.undo().unwrap();
    assert_eq!(wb.sheet_names()[1], "Sheet1");
    assert_eq!(wb.input(2, 0, 0), "=Sheet1!A1*2");
    wb.redo().unwrap();
    wb.redo().unwrap();
    assert_eq!(wb.input(2, 0, 0), "='Sales 2026'!A1*2");

    // Delete: references to it are #REF!.
    assert!(wb.sheet_has_data(1));
    assert!(!wb.sheet_has_data(0));
    wb.delete_sheet(1).unwrap();
    assert_eq!(tab_names(&wb), ["Sheet3", "Sheet2"]);
    assert_eq!(wb.input(1, 0, 0), "=#REF!*2");
    assert_eq!(wb.display_value(1, 0, 0), "#REF!");
    wb.undo().unwrap();
    assert_eq!(tab_names(&wb), ["Sheet3", "Sales 2026", "Sheet2"]);
    assert_eq!(wb.display_value(2, 0, 0), "10");

    // The last shown worksheet can't go or be hidden.
    wb.set_sheet_hidden(0, true).unwrap();
    wb.set_sheet_hidden(1, true).unwrap();
    assert_eq!(tab_names(&wb), ["Sheet2"]);
    assert!(wb.delete_sheet(2).is_err());
    assert!(wb.set_sheet_hidden(2, true).is_err());
    let hidden: Vec<String> = wb.hidden_sheets().into_iter().map(|h| h.1).collect();
    assert_eq!(hidden, ["Sheet3", "Sales 2026"]);
    // A hidden one can be deleted.
    wb.delete_sheet(0).unwrap();
    wb.undo().unwrap();
    wb.set_sheet_hidden(1, false).unwrap();
    assert_eq!(tab_names(&wb), ["Sales 2026", "Sheet2"]);
}

#[test]
fn names_follow_a_rename_and_go_with_a_deleted_sheet() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::new_empty().unwrap();
    wb.insert_sheet(1).unwrap();
    wb.set_input(1, 0, 0, "4").unwrap();
    wb.set_input(0, 0, 0, "=Sheet2!A1").unwrap();
    let p = save(&mut wb, dir.path(), "a.xlsx");
    let mut wb = Workbook::open(dir.path().join("a.xlsx")).unwrap();
    let _ = p;
    wb.rename_sheet(1, "Data").unwrap();
    let p = save(&mut wb, dir.path(), "b.xlsx");
    assert_eq!(listed(&p), ["Sheet1", "Data"]);
    assert!(sheet_part(&p, "Sheet1").contains("<f>Data!A1</f>"));
    wb.delete_sheet(1).unwrap();
    let p = save(&mut wb, dir.path(), "c.xlsx");
    assert_eq!(listed(&p), ["Sheet1"]);
    assert!(sheet_part(&p, "Sheet1").contains("<f>#REF!</f>"));
    // Save → undo → save: the sheet comes back.
    wb.undo().unwrap();
    let p = save(&mut wb, dir.path(), "d.xlsx");
    assert_eq!(listed(&p), ["Sheet1", "Data"]);
    assert!(sheet_part(&p, "Sheet1").contains("<f>Data!A1</f>"));
    let back = Workbook::open(dir.path().join("d.xlsx")).unwrap();
    assert_eq!(back.display_value(0, 0, 0), "4");
}

fn list_rule() -> Validation {
    Validation {
        ime_mode: None,
        kind: ValidationType::List,
        operator: ValidationOperator::Between,
        allow_blank: true,
        dropdown: true,
        prompt: None,
        error: Some((
            ErrorStyle::Stop,
            Message {
                title: "Pick".into(),
                text: "From the list".into(),
            },
        )),
        formula1: Some("\"Yes,No\"".into()),
        formula2: None,
        ranges: Vec::new(),
    }
}

#[test]
fn a_copy_has_the_sheets_cells_formats_notes_and_rules() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::new_empty().unwrap();
    wb.insert_sheet(1).unwrap(); // Sheet2
    wb.set_input(0, 0, 0, "Total").unwrap();
    wb.set_input(0, 0, 1, "=B2*2").unwrap();
    wb.set_input(0, 1, 1, "21").unwrap();
    wb.set_column_width(0, 0, 0, 200.0).unwrap();
    wb.merge_range(0, CellRange::from_corners((3, 0), (3, 2)))
        .unwrap();
    wb.set_note(0, 0, 0, "Checked by Rina", "Rina").unwrap();
    wb.set_validation(0, &[CellRange::from_corners((5, 0), (9, 0))], list_rule())
        .unwrap();
    let cf = sheets_engine::CfRuleDef {
        num_fmt: None,
        border: None,
        spec: sheets_engine::CfSpec::CellValue {
            op: sheets_engine::CfCompare::GreaterThan,
            value: "7".into(),
            value2: String::new(),
        },
        format: sheets_engine::CfFormat::PRESETS[0].1,
        stop_if_true: false,
    };
    wb.add_cf_rule(0, "B1:B10", &cf).unwrap();

    // Before Sheet2 (tab 1).
    let copy = wb.copy_sheet(0, 1).unwrap();
    assert_eq!(copy, 1);
    assert_eq!(tab_names(&wb), ["Sheet1", "Sheet1 (2)", "Sheet2"]);
    assert_eq!(wb.display_value(1, 0, 1), "42");
    assert_eq!(wb.column_width(1, 0), wb.column_width(0, 0));
    assert!(wb.merge_at(1, 3, 1).is_some());
    assert_eq!(wb.note_at(1, 0, 0).unwrap().text, "Checked by Rina");
    assert_eq!(wb.sheet_validations(1).len(), 1);
    assert_eq!(wb.cf_rules(1, None).len(), 1);
    // A copy of the copy counts on.
    wb.copy_sheet(1, 99).unwrap();
    assert_eq!(
        tab_names(&wb),
        ["Sheet1", "Sheet1 (2)", "Sheet2", "Sheet1 (3)"]
    );
    wb.undo().unwrap();

    let p = save(&mut wb, dir.path(), "a.xlsx");
    assert_eq!(listed(&p), ["Sheet1", "Sheet1 (2)", "Sheet2"]);
    let back = Workbook::open(dir.path().join("a.xlsx")).unwrap();
    assert_eq!(back.note_at(1, 0, 0).unwrap().text, "Checked by Rina");
    assert_eq!(back.sheet_validations(1).len(), 1);
    assert_eq!(back.cf_rules(1, None).len(), 1);
    assert!(back.merge_at(1, 3, 1).is_some());
    assert_eq!(back.display_value(1, 0, 1), "42");

    // Undo the copy: gone, also from the next save.
    wb.undo().unwrap();
    assert_eq!(tab_names(&wb), ["Sheet1", "Sheet2"]);
    let p = save(&mut wb, dir.path(), "b.xlsx");
    assert_eq!(listed(&p), ["Sheet1", "Sheet2"]);
    assert_eq!(
        p.keys().filter(|k| k.contains("comments")).count(),
        1,
        "one sheet's notes"
    );
}

#[test]
fn move_hide_and_tab_colors_save() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::new_empty().unwrap();
    wb.insert_sheet(1).unwrap();
    wb.insert_sheet(2).unwrap();
    wb.move_tab(Tab::Sheet(0), 2).unwrap();
    assert_eq!(tab_names(&wb), ["Sheet2", "Sheet3", "Sheet1"]);
    wb.move_tab(Tab::Sheet(2), 0).unwrap();
    assert_eq!(tab_names(&wb), ["Sheet1", "Sheet2", "Sheet3"]);
    wb.undo().unwrap();
    assert_eq!(tab_names(&wb), ["Sheet2", "Sheet3", "Sheet1"]);

    let accent = TabColor::Theme { slot: 4, tint: 0.4 };
    wb.set_tab_color(0, Some(accent)).unwrap();
    wb.set_tab_color(1, Some(TabColor::Rgb(0xC00000))).unwrap();
    assert_eq!(wb.tab_color(0), Some(accent));
    assert_eq!(
        wb.tab_color_rgb(TabColor::Theme { slot: 4, tint: 0.0 }),
        0x4472C4
    );
    wb.set_sheet_hidden(1, true).unwrap();
    let p = save(&mut wb, dir.path(), "a.xlsx");
    assert_eq!(listed(&p), ["Sheet2", "Sheet3 (hidden)", "Sheet1"]);
    assert!(sheet_part(&p, "Sheet2").contains("<tabColor theme=\"4\" tint=\"0.4\"/>"));
    assert!(sheet_part(&p, "Sheet3").contains("<tabColor rgb=\"FFC00000\"/>"));
    let back = Workbook::open(dir.path().join("a.xlsx")).unwrap();
    assert_eq!(back.tab_color(0), Some(accent));
    assert_eq!(back.tab_color(1), Some(TabColor::Rgb(0xC00000)));
    assert_eq!(tab_names(&back), ["Sheet2", "Sheet1"]);

    // No Color, undone and redone.
    wb.set_tab_color(0, None).unwrap();
    assert_eq!(wb.tab_color(0), None);
    wb.undo().unwrap();
    assert_eq!(wb.tab_color(0), Some(accent));
    let p2 = save(&mut wb, dir.path(), "b.xlsx");
    assert!(changed(&p, &p2).is_empty(), "{:?}", changed(&p, &p2));
}

#[test]
fn worksheets_move_among_chart_sheets_and_charts_follow_a_rename() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(corpus("22_chart_sheets.xlsx")).unwrap();
    assert_eq!(
        tab_names(&wb),
        ["[Revenue chart]", "Data", "[Share]", "Notes"]
    );
    let plain = save(&mut wb, dir.path(), "plain.xlsx");

    // A worksheet can't take a chart sheet's name (shown or hidden).
    assert!(wb.rename_sheet(0, "share").is_err());
    assert!(wb.rename_sheet(0, "Hidden chart").is_err());

    // Rename Data: the charts still read it, and are saved with the name.
    wb.rename_sheet(0, "Sales data").unwrap();
    let r = wb.chart_reference("Data!$B$2:$B$5").unwrap();
    assert_eq!(r.shown, ["120", "95", "143", "88"]);
    let p = save(&mut wb, dir.path(), "renamed.xlsx");
    assert_eq!(
        listed(&p),
        [
            "Revenue chart",
            "Sales data",
            "Share",
            "Hidden chart (hidden)",
            "Notes"
        ]
    );
    let chart = text(&p, "xl/charts/chart1.xml");
    assert!(
        chart.contains("<c:f>'Sales data'!$B$2:$B$5</c:f>"),
        "{chart}"
    );
    assert!(!chart.contains("<c:f>Data!"));
    // Reopened, the chart reads its data.
    let back = Workbook::open(dir.path().join("renamed.xlsx")).unwrap();
    assert!(back.chart_reference("'Sales data'!$B$2:$B$5").is_some());
    // Undone and saved: as opened again.
    wb.undo().unwrap();
    let undone = save(&mut wb, dir.path(), "undone.xlsx");
    assert_eq!(listed(&undone), listed(&plain));
    assert!(text(&undone, "xl/charts/chart1.xml").contains("<c:f>Data!$B$2:$B$5</c:f>"));

    // Move Notes to the front: the chart sheets keep their places among
    // the others.
    wb.move_tab(Tab::Sheet(1), 0).unwrap();
    assert_eq!(
        tab_names(&wb),
        ["Notes", "[Revenue chart]", "Data", "[Share]"]
    );
    // Data (worksheet 1 now) to the end, past Share.
    wb.move_tab(Tab::Sheet(1), 3).unwrap();
    assert_eq!(
        tab_names(&wb),
        ["Notes", "[Revenue chart]", "[Share]", "Data"]
    );
    let p = save(&mut wb, dir.path(), "moved.xlsx");
    assert_eq!(
        listed(&p),
        [
            "Notes",
            "Revenue chart",
            "Share",
            "Hidden chart (hidden)",
            "Data"
        ]
    );
    let back = Workbook::open(dir.path().join("moved.xlsx")).unwrap();
    assert_eq!(tab_names(&back), tab_names(&wb));
    wb.undo().unwrap();
    wb.undo().unwrap();
    assert_eq!(
        tab_names(&wb),
        ["[Revenue chart]", "Data", "[Share]", "Notes"]
    );

    // Insert before Share, copy Data before Revenue chart, delete Notes.
    wb.insert_sheet(2).unwrap();
    wb.copy_sheet(0, 0).unwrap();
    assert_eq!(
        tab_names(&wb),
        [
            "Data (2)",
            "[Revenue chart]",
            "Data",
            "Sheet1",
            "[Share]",
            "Notes"
        ]
    );
    wb.delete_sheet(3).unwrap();
    let p = save(&mut wb, dir.path(), "edited.xlsx");
    assert_eq!(
        listed(&p),
        [
            "Data (2)",
            "Revenue chart",
            "Data",
            "Sheet1",
            "Share",
            "Hidden chart (hidden)"
        ]
    );
    let back = Workbook::open(dir.path().join("edited.xlsx")).unwrap();
    assert_eq!(tab_names(&back), tab_names(&wb));
    for _ in 0..3 {
        wb.undo().unwrap();
    }
    let p = save(&mut wb, dir.path(), "back.xlsx");
    assert_eq!(listed(&p), listed(&plain));
}

#[test]
fn notes_stay_with_their_sheet_through_tab_commands() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::new_empty().unwrap();
    wb.insert_sheet(1).unwrap();
    wb.set_note(1, 2, 2, "On Sheet2", "Bayu").unwrap();
    wb.set_note(0, 0, 0, "On Sheet1", "Bayu").unwrap();
    save(&mut wb, dir.path(), "a.xlsx");
    let mut wb = Workbook::open(dir.path().join("a.xlsx")).unwrap();
    wb.rename_sheet(1, "Second").unwrap();
    wb.move_tab(Tab::Sheet(1), 0).unwrap();
    assert_eq!(wb.note_at(0, 2, 2).unwrap().text, "On Sheet2");
    assert_eq!(wb.note_at(1, 0, 0).unwrap().text, "On Sheet1");
    let p = save(&mut wb, dir.path(), "b.xlsx");
    assert_eq!(listed(&p), ["Second", "Sheet1"]);
    let back = Workbook::open(dir.path().join("b.xlsx")).unwrap();
    assert_eq!(back.note_at(0, 2, 2).unwrap().text, "On Sheet2");
    assert_eq!(back.note_at(1, 0, 0).unwrap().text, "On Sheet1");
    // Deleting a sheet takes its notes; the other keeps its own.
    wb.delete_sheet(0).unwrap();
    let p = save(&mut wb, dir.path(), "c.xlsx");
    assert_eq!(listed(&p), ["Sheet1"]);
    let back = Workbook::open(dir.path().join("c.xlsx")).unwrap();
    assert_eq!(back.notes_now(0).len(), 1);
    assert_eq!(back.note_at(0, 0, 0).unwrap().text, "On Sheet1");
}

/// Names that pointed only at a deleted sheet's cells are saved as Excel
/// writes them, with the cell part (`#REF!$B$3:$B$6`), and read back.
#[test]
fn deleted_sheet_names_are_saved_with_their_cell_part() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(corpus("08_defined_names.xlsx")).unwrap();
    let inputs = wb.sheet_names().iter().position(|n| n == "Inputs").unwrap() as u32;
    wb.delete_sheet(inputs).unwrap();
    let p = save(&mut wb, dir.path(), "names.xlsx");
    let xml = text(&p, "xl/workbook.xml");
    for want in [
        r#"name="Prices">#REF!$B$3:$B$6</definedName>"#,
        r#"name="TaxRate">#REF!$B$1</definedName>"#,
        r#"name="Ten">10</definedName>"#,
    ] {
        assert!(xml.contains(want), "{want} not in {xml}");
    }
    let mut back = Workbook::open(dir.path().join("names.xlsx")).unwrap();
    assert_eq!(back.display_value(0, 0, 1), "#REF!");
    let p = save(&mut back, dir.path(), "again.xlsx");
    let xml = text(&p, "xl/workbook.xml");
    assert!(
        xml.contains(r#"name="Prices">#REF!$B$3:$B$6</definedName>"#),
        "{xml}"
    );
}
