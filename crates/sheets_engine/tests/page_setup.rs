//! Page Setup made in Kantor: undo/redo, the saved `pageSetup`,
//! `pageMargins`, `printOptions`, `headerFooter`, `fitToPage` and print
//! names, read back the same; sheets nobody edited stay byte for byte.

#[path = "support/pivot_check.rs"]
mod pivot_check;

use std::path::{Path, PathBuf};

use pivot_check::{el, parts};
use roxmltree::Document;
use sheets_engine::{
    CellRange, HeaderFooter, Margins, PageSetup, Paper, PrintOptions, SheetPrint, Workbook,
};

/// Children of `<worksheet>` in schema order (CT_Worksheet), the part the
/// tests check.
const ORDER: [&str; 22] = [
    "sheetPr",
    "dimension",
    "sheetViews",
    "sheetFormatPr",
    "cols",
    "sheetData",
    "sheetProtection",
    "autoFilter",
    "mergeCells",
    "conditionalFormatting",
    "dataValidations",
    "hyperlinks",
    "printOptions",
    "pageMargins",
    "pageSetup",
    "headerFooter",
    "rowBreaks",
    "colBreaks",
    "drawing",
    "legacyDrawing",
    "tableParts",
    "extLst",
];

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("page_setup");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/xlsx")
        .join(name)
}

fn assert_schema_order(xml: &str) {
    let doc = Document::parse(xml).unwrap();
    let mut last = 0;
    for c in doc.root_element().children().filter(|c| c.is_element()) {
        let name = c.tag_name().name();
        if let Some(i) = ORDER.iter().position(|n| *n == name) {
            assert!(i >= last, "{name} is out of schema order in {xml}");
            last = i;
        }
    }
}

fn settings() -> SheetPrint {
    SheetPrint {
        setup: PageSetup {
            paper: Paper::FOLIO,
            landscape: true,
            scale: 100,
            fit: Some((1, 0)),
            over_then_down: true,
            first_page_number: Some(3),
            black_and_white: false,
        },
        margins: Margins::NARROW,
        options: PrintOptions {
            grid_lines: true,
            headings: true,
            horizontal_centered: true,
            vertical_centered: false,
        },
        header_footer: HeaderFooter {
            odd_header: "&L&F&C&\"Arial,Bold\"Sales & Co&R&D &T".into(),
            odd_footer: "&CPage &P of &N".into(),
            scale_with_doc: true,
            align_with_margins: true,
            ..HeaderFooter::default()
        },
        row_breaks: Vec::new(),
        col_breaks: Vec::new(),
        print_area: vec![CellRange::from_corners((1, 1), (9, 3))],
        title_rows: Some((0, 1)),
        title_cols: Some((0, 0)),
    }
}

fn data_book() -> Workbook {
    let mut wb = Workbook::new_empty().unwrap();
    let cells: Vec<(u32, u32, String)> = (0..12)
        .flat_map(|r| (0..4).map(move |c| (r, c, format!("{}", r * 10 + c))))
        .collect();
    wb.set_inputs(0, &cells).unwrap();
    wb
}

#[test]
fn page_setup_undoes_saves_and_reads_back() {
    let mut wb = data_book();
    let before = wb.print_settings(0);
    let want = settings();
    wb.set_print_settings(0, want.clone()).unwrap();
    assert_eq!(wb.print_settings(0), want);
    assert!(wb.undo().unwrap());
    assert_eq!(wb.print_settings(0), before);
    assert!(wb.redo().unwrap());
    assert_eq!(wb.print_settings(0), want);

    let path = scratch("new.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let p = parts(&path);
    let sheet = &p["xl/worksheets/sheet1.xml"];
    assert_schema_order(sheet);
    let doc = Document::parse(sheet).unwrap();
    let root = doc.root_element();
    let setup = el(root, "pageSetup").expect("pageSetup");
    assert_eq!(setup.attribute("paperSize"), Some("14"));
    assert_eq!(setup.attribute("orientation"), Some("landscape"));
    assert_eq!(setup.attribute("fitToHeight"), Some("0"));
    assert_eq!(setup.attribute("fitToWidth"), None, "1 is the default");
    assert_eq!(setup.attribute("pageOrder"), Some("overThenDown"));
    assert_eq!(setup.attribute("firstPageNumber"), Some("3"));
    assert_eq!(setup.attribute("useFirstPageNumber"), Some("1"));
    let fit = el(el(root, "sheetPr").unwrap(), "pageSetUpPr").unwrap();
    assert_eq!(fit.attribute("fitToPage"), Some("1"));
    let m = el(root, "pageMargins").unwrap();
    assert_eq!(m.attribute("left"), Some("0.25"));
    assert_eq!(m.attribute("top"), Some("0.75"));
    let o = el(root, "printOptions").unwrap();
    assert_eq!(o.attribute("gridLines"), Some("1"));
    assert_eq!(o.attribute("headings"), Some("1"));
    assert_eq!(o.attribute("horizontalCentered"), Some("1"));
    let hf = el(root, "headerFooter").unwrap();
    assert_eq!(
        el(hf, "oddHeader").unwrap().text(),
        Some("&L&F&C&\"Arial,Bold\"Sales & Co&R&D &T")
    );
    let wbx = Document::parse(&p["xl/workbook.xml"]).unwrap();
    let names: Vec<(String, String)> = wbx
        .descendants()
        .filter(|n| n.tag_name().name() == "definedName")
        .map(|n| {
            (
                n.attribute("name").unwrap().to_owned(),
                n.text().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    assert!(names.contains(&("_xlnm.Print_Area".into(), "Sheet1!$B$2:$D$10".into())));
    assert!(
        names.contains(&(
            "_xlnm.Print_Titles".into(),
            "Sheet1!$A:$A,Sheet1!$1:$2".into()
        )),
        "{names:?}"
    );
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());

    // Read back: the same settings, both parts of the print titles.
    let mut back = Workbook::open(&path).unwrap();
    assert_eq!(back.print_settings(0), want);
    // A save without edits keeps the union (IronCalc alone keeps only its
    // first part).
    let again = scratch("again.xlsx");
    back.save_as(&again).unwrap();
    let back2 = Workbook::open(&again).unwrap();
    assert_eq!(back2.print_settings(0), want);
}

#[test]
fn print_area_follows_inserted_rows() {
    let mut wb = data_book();
    wb.set_print_settings(0, settings()).unwrap();
    wb.insert_rows(0, 0, 2).unwrap();
    let p = wb.print_settings(0);
    assert_eq!(p.print_area, vec![CellRange::from_corners((3, 1), (11, 3))]);
    assert_eq!(p.title_rows, Some((2, 3)));
    let path = scratch("moved.xlsx");
    wb.save_as(&path).unwrap();
    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.print_settings(0).print_area, p.print_area);
}

#[test]
fn clearing_settings_removes_elements_and_names() {
    let src = corpus("16_print_protect_outline.xlsx");
    let mut wb = Workbook::open(&src).unwrap();
    let mut p = wb.print_settings(0);
    assert!(!p.print_area.is_empty());
    p.print_area.clear();
    p.title_rows = None;
    p.options = PrintOptions::default();
    p.header_footer = HeaderFooter {
        scale_with_doc: true,
        align_with_margins: true,
        ..HeaderFooter::default()
    };
    wb.set_print_settings(0, p.clone()).unwrap();
    let path = scratch("cleared.xlsx");
    wb.save_as(&path).unwrap();
    let parts = parts(&path);
    let sheet = &parts["xl/worksheets/sheet1.xml"];
    assert_schema_order(sheet);
    assert!(!sheet.contains("<printOptions"));
    assert!(!sheet.contains("<headerFooter"));
    // The manual page break and comment boxes are still carried.
    assert!(sheet.contains("<rowBreaks"));
    assert!(sheet.contains("<legacyDrawing"));
    let wbx = &parts["xl/workbook.xml"];
    assert!(!wbx.contains("_xlnm.Print_Area"), "{wbx}");
    assert!(!wbx.contains("_xlnm.Print_Titles"));
    assert!(wbx.contains("Totals"), "other names stay: {wbx}");
    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.print_settings(0).print_area, Vec::new());
}

#[test]
fn untouched_sheets_stay_byte_identical() {
    let src = corpus("16_print_protect_outline.xlsx");
    // A plain save, and a save with the third sheet's page setup changed.
    let mut plain = Workbook::open(&src).unwrap();
    let plain_path = scratch("plain.xlsx");
    plain.save_as(&plain_path).unwrap();
    let mut edited = Workbook::open(&src).unwrap();
    let mut p = edited.print_settings(2);
    p.setup.paper = Paper::A4;
    p.setup.landscape = true;
    p.margins = Margins::WIDE;
    edited.set_print_settings(2, p.clone()).unwrap();
    let edited_path = scratch("edited.xlsx");
    edited.save_as(&edited_path).unwrap();
    let a = parts(&plain_path);
    let b = parts(&edited_path);
    for (name, text) in &a {
        if name == "xl/worksheets/sheet3.xml" || name == "xl/workbook.xml" {
            continue;
        }
        assert_eq!(Some(text), b.get(name), "{name} changed");
    }
    // The original's print setup of sheet 1 is still what the file had.
    let s1 = &b["xl/worksheets/sheet1.xml"];
    assert!(s1.contains(r#"<pageSetup paperSize="9" fitToHeight="0" orientation="landscape"/>"#));
    assert_schema_order(&b["xl/worksheets/sheet3.xml"]);
    assert_eq!(Workbook::open(&edited_path).unwrap().print_settings(2), p);
}

#[test]
fn printer_settings_link_is_kept() {
    // A sheet whose <pageSetup> links printer settings keeps the link.
    let src = corpus("16_print_protect_outline.xlsx");
    let mut wb = Workbook::open(&src).unwrap();
    let mut p = wb.print_settings(0);
    p.setup.scale = 85;
    p.setup.fit = None;
    wb.set_print_settings(0, p.clone()).unwrap();
    let path = scratch("scale.xlsx");
    wb.save_as(&path).unwrap();
    let parts = parts(&path);
    let doc = Document::parse(&parts["xl/worksheets/sheet1.xml"]).unwrap();
    let root = doc.root_element();
    let setup = el(root, "pageSetup").unwrap();
    assert_eq!(setup.attribute("scale"), Some("85"));
    assert_eq!(setup.attribute("fitToHeight"), None);
    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.print_settings(0).setup, p.setup);
}

#[test]
fn header_sections_split_and_join() {
    use sheets_engine::{join_sections, split_sections};
    let s = split_sections("&LLeft&CMid &P&RRight && more");
    assert_eq!(
        s,
        ["Left".to_owned(), "Mid &P".into(), "Right && more".into()]
    );
    assert_eq!(
        join_sections(&s[0], &s[1], &s[2]),
        "&LLeft&CMid &P&RRight && more"
    );
    assert_eq!(split_sections("Plain")[1], "Plain");
    assert_eq!(Paper::FOLIO.code(), Some(14));
    assert_eq!(Paper::mm(100.0, 100.0).code(), None);
}
