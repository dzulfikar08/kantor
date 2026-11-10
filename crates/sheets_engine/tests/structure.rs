//! Carried parts follow inserted and deleted rows and columns (the
//! `preserve` and `shift` modules), on files of the corpus.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use sheets_engine::Workbook;

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/xlsx")
        .join(name)
}

fn parts(path: &Path) -> BTreeMap<String, String> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).unwrap();
        let mut s = String::new();
        if e.read_to_string(&mut s).is_ok() {
            out.insert(e.name().unwrap().into_owned(), s);
        }
    }
    out
}

fn save(wb: &mut Workbook, dir: &Path, name: &str) -> BTreeMap<String, String> {
    let path = dir.join(name);
    wb.save_as(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        sheets_engine::package_problems(&bytes),
        Vec::<String>::new()
    );
    // Kantor (IronCalc) reads it back.
    Workbook::open(&path).unwrap();
    parts(&path)
}

fn sheet_index(wb: &Workbook, name: &str) -> u32 {
    wb.sheet_names().iter().position(|n| n == name).unwrap() as u32
}

#[test]
fn validations_and_sparklines_follow_inserted_rows() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(corpus("15_sparklines_cf_dv.xlsx")).unwrap();
    let s = sheet_index(&wb, "Metrics");
    wb.insert_rows(s, 3, 2).unwrap(); // before row 4
    let p = save(&mut wb, dir.path(), "a.xlsx");
    let x = &p["xl/worksheets/sheet1.xml"];
    assert!(x.contains(r#"sqref="H2:H11""#), "validation grows: {x}");
    assert!(x.contains("<xm:sqref>F2</xm:sqref>"), "above: unchanged");
    assert!(x.contains("<xm:sqref>F6</xm:sqref>"), "below: moved");
    assert!(x.contains("<xm:f>Metrics!B6:E6</xm:f>"), "data range moved");
    assert!(x.contains("<xm:f>Metrics!B2:E2</xm:f>"));
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());

    // Undone: back where they were. Saving again moves nothing twice.
    wb.undo().unwrap();
    let p = save(&mut wb, dir.path(), "b.xlsx");
    let x = &p["xl/worksheets/sheet1.xml"];
    assert!(x.contains(r#"sqref="H2:H9""#) && x.contains("<xm:sqref>F4</xm:sqref>"));
    // Redo after that save: relative to the file saved last.
    wb.redo().unwrap();
    let p = save(&mut wb, dir.path(), "c.xlsx");
    let x = &p["xl/worksheets/sheet1.xml"];
    assert!(x.contains(r#"sqref="H2:H11""#) && x.contains("<xm:sqref>F6</xm:sqref>"));
}

#[test]
fn deleted_cells_take_their_extras_with_them() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(corpus("15_sparklines_cf_dv.xlsx")).unwrap();
    let s = sheet_index(&wb, "Metrics");
    wb.delete_rows(s, 1, 8).unwrap(); // rows 2-9: every validation and sparkline
    let p = save(&mut wb, dir.path(), "a.xlsx");
    let x = &p["xl/worksheets/sheet1.xml"];
    assert!(!x.contains("<dataValidation"), "{x}");
    assert!(!x.contains("<x14:sparkline>"), "{x}");
    assert!(
        wb.save_notes()
            .iter()
            .any(|n| n.contains("<dataValidations> dropped")),
        "{:?}",
        wb.save_notes()
    );
}

#[test]
fn charts_drawings_and_chart_sheets_follow_rows() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(corpus("14_charts_images.xlsx")).unwrap();
    let s = sheet_index(&wb, "Data");
    wb.insert_rows(s, 0, 1).unwrap();
    let p = save(&mut wb, dir.path(), "a.xlsx");
    let chart = &p["xl/charts/chart1.xml"];
    assert!(chart.contains("<c:f>Data!$B$3:$B$8</c:f>"), "{chart}");
    assert!(!chart.contains("<c:f>Data!$B$1</c:f>"));
    assert!(chart.contains("<c:f>Data!$B$2</c:f>"), "series name moved");
    let sheet_chart = &p["xl/charts/chart2.xml"];
    assert!(sheet_chart.contains("<c:f>Data!$C$3:$C$8</c:f>"));
    // The chart was anchored at E2 (row index 1).
    let drawing = &p["xl/drawings/drawing1.xml"];
    assert!(
        drawing.contains("<xdr:row>2</xdr:row>"),
        "anchor moved: {drawing}"
    );
    // The chart sheet stays between Data and After.
    let wbx = &p["xl/workbook.xml"];
    let (data, trend, after) = (
        wbx.find("name=\"Data\"").unwrap(),
        wbx.find("name=\"Trend\"").unwrap(),
        wbx.find("name=\"After\"").unwrap(),
    );
    assert!(data < trend && trend < after, "{wbx}");
    assert_eq!(wb.sheet_names(), ["Data", "After"]);
}

#[test]
fn comments_and_page_breaks_follow_deleted_rows() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(corpus("16_print_protect_outline.xlsx")).unwrap();
    let s = sheet_index(&wb, "Print");
    wb.delete_rows(s, 1, 1).unwrap(); // row 2: the note on B2 goes
    let p = save(&mut wb, dir.path(), "a.xlsx");
    let comments = p
        .iter()
        .find(|(k, _)| k.starts_with("xl/comments"))
        .map(|(_, v)| v)
        .unwrap();
    assert!(!comments.contains("ref=\"B2\""), "{comments}");
    assert!(comments.contains("ref=\"C2\""), "C3 moved up: {comments}");
    let vml = p
        .iter()
        .find(|(k, _)| k.ends_with(".vml"))
        .map(|(_, v)| v)
        .unwrap();
    assert!(!vml.contains("<x:Row>1</x:Row>") || vml.matches("<x:Row>").count() == 1);
    assert!(vml.contains("<x:Row>1</x:Row>"), "C3's box moved: {vml}");
    let x = &p["xl/worksheets/sheet1.xml"];
    assert!(x.contains(r#"<brk id="19""#), "page break moved: {x}");
}

#[test]
fn tables_follow_rows_and_renamed_headers() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(corpus("13_tables.xlsx")).unwrap();
    let s = sheet_index(&wb, "Sales");
    wb.insert_rows(s, 3, 1).unwrap(); // inside the table
    wb.set_input(s, 0, 1, "Qty").unwrap(); // header "Units"
    let p = save(&mut wb, dir.path(), "a.xlsx");
    let t = &p["xl/tables/table1.xml"];
    assert!(t.contains(r#"ref="A1:D9""#), "{t}");
    assert!(t.contains(r#"<autoFilter ref="A1:D8""#), "{t}");
    assert!(t.contains(r#"name="Qty""#), "{t}");
    assert!(t.contains("[[#This Row],Qty]*[[#This Row],Price]"), "{t}");
    // (IronCalc can't compute this-row references: #ERROR! values are noted.)
    assert!(
        !wb.save_notes().iter().any(|n| n.contains("table")),
        "{:?}",
        wb.save_notes()
    );

    // A column inserted inside the table: Excel would have to rebuild it.
    wb.insert_columns(s, 2, 1).unwrap();
    let p = save(&mut wb, dir.path(), "b.xlsx");
    assert!(!p.contains_key("xl/tables/table1.xml"));
    assert!(
        p.contains_key("xl/tables/table2.xml"),
        "the other sheet's table stays"
    );
    assert!(
        wb.save_notes()
            .iter()
            .any(|n| n.contains("columns were inserted or deleted inside it")),
        "{:?}",
        wb.save_notes()
    );
}

#[test]
fn filter_database_name_stays_hidden_and_moves() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(corpus("13_tables.xlsx")).unwrap();
    let s = sheet_index(&wb, "Filtered");
    wb.insert_rows(s, 0, 1).unwrap();
    let p = save(&mut wb, dir.path(), "a.xlsx");
    let x = &p["xl/worksheets/sheet3.xml"];
    assert!(x.contains(r#"<autoFilter ref="A2:C11""#), "{x}");
    let wbx = &p["xl/workbook.xml"];
    let name = wbx
        .split("<definedName ")
        .find(|d| d.contains("_xlnm._FilterDatabase"))
        .unwrap();
    assert!(name.contains(r#"hidden="1""#), "{name}");
    assert!(name.contains("$A$2:$C$11"), "{name}");
}
