//! JavaScript macros on the Sheets object model: cells, formats, loops, a
//! chart, dialogs, errors, Stop; one undo step; the scripts saved in the
//! workbook and listed with the macros.

mod support;

use sheets_engine::drawing::DrawingKind;
use sheets_engine::scripts::{Script, ScriptLanguage};
use sheets_engine::{ChartType, Workbook};
use sheets_script::{HeadlessUi, MacroEngine, MacroProject, RunOptions, RunOutcome, View};
use support::*;

fn with_js(mut book: Workbook, source: &str) -> Workbook {
    book.set_scripts(vec![Script::new(
        "Report",
        ScriptLanguage::JavaScript,
        source,
    )]);
    book
}

struct Ran {
    book: Workbook,
    outcome: RunOutcome,
    printed: Vec<String>,
    shown: Vec<String>,
}

fn run_with(book: Workbook, name: &str, options: &RunOptions) -> Ran {
    let macros = MacroProject::of_book(&book)
        .expect("the workbook has macros")
        .expect("they can be read");
    let ui = HeadlessUi::new(vec!["7".into()], false);
    let printed = ui.printed.clone();
    let shown = ui.shown.clone();
    let mut engine = MacroEngine::new(macros);
    let (book, outcome) = engine.run(book, name, View::default(), Box::new(ui), options);
    let printed = printed.borrow().clone();
    let shown = shown.borrow().clone();
    Ran {
        book,
        outcome,
        printed,
        shown,
    }
}

fn run_js(book: Workbook, name: &str) -> Ran {
    run_with(book, name, &RunOptions::default())
}

const REPORT: &str = r##"
// Totals the Amount column and formats a small report.
function report() {
  const ws = ActiveSheet;
  let total = 0;
  for (let r = 2; r <= 5; r++) {
    total += ws.cells(r, 3).value;
  }
  ws.range("E1").value = "Total";
  ws.range("F1").value = total;
  ws.range("E1:F1").font.bold = true;
  ws.range("F1").numberFormat = "#,##0.00";
  const data = ws.range("A2:C5").value;
  ws.range("H1").value = data.length + "x" + data[0].length + " " + data[0][0];
  ws.range("A7:B8").value = [[1, 2], [3, 4]];
  ws.range("C7").formula = "=SUM(A7:B8)";
  let east = 0;
  for (const c of ws.range("B2:B5")) {
    if (c.value === "East") east++;
  }
  Range("H2").value = east;
  console.log("total", total, Range("C7").value);
}

function _helper() {}

function withArgs(a) { return a; }

function ask() {
  const yes = msgBox("Write it?", vbYesNo + vbQuestion, "Report");
  const n = InputBox("How many?");
  Range("J1").value = yes === vbYes ? Number(n) * 2 : -1;
}

function broken() {
  const r = Range("A1");
  r.noSuchThing = 1;
}

function spin() {
  for (;;) {}
}
"##;

#[test]
fn a_script_writes_cells_and_formats_in_one_undo_step() {
    let ran = run_js(with_js(sales(), REPORT), "report");
    ok(&ran.outcome);
    let mut book = ran.book;
    assert_eq!(cell(&book, 0, "E1"), "Total");
    assert_eq!(cell(&book, 0, "F1"), "700.00");
    assert!(book.cell_style(0, 0, 4).bold);
    assert!(book.cell_style(0, 0, 5).bold);
    assert_eq!(cell(&book, 0, "H1"), "4x3 Dewi");
    assert_eq!(cell(&book, 0, "B8"), "4");
    assert_eq!(cell(&book, 0, "C7"), "10");
    assert_eq!(cell(&book, 0, "H2"), "2");
    assert_eq!(ran.printed, vec!["total 700 10".to_owned()]);
    assert!(ran.outcome.changed);

    // One undo takes all of it back; the table is as it was.
    assert!(book.undo().unwrap());
    for a1 in ["E1", "F1", "H1", "A7", "B8", "C7", "H2"] {
        assert_eq!(cell(&book, 0, a1), "", "{a1}");
    }
    assert!(!book.cell_style(0, 0, 4).bold);
    assert_eq!(cell(&book, 0, "C2"), "300");
}

#[test]
fn script_macros_are_listed_and_qualified_names_run() {
    let book = with_js(sales(), REPORT);
    let macros = MacroProject::of_book(&book).unwrap().unwrap();
    let names: Vec<String> = macros.macros().iter().map(|m| m.full_name()).collect();
    assert_eq!(
        names,
        vec![
            "Report.report",
            "Report.ask",
            "Report.broken",
            "Report.spin"
        ]
    );
    let ran = run_js(book, "Report.report");
    ok(&ran.outcome);
    assert_eq!(cell(&ran.book, 0, "F1"), "700.00");
}

#[test]
fn dialogs_and_constants() {
    let ran = run_js(with_js(sales(), REPORT), "ask");
    ok(&ran.outcome);
    assert_eq!(ran.shown, vec!["Write it?".to_owned()]);
    assert_eq!(cell(&ran.book, 0, "J1"), "14");
}

#[test]
fn errors_name_the_module_function_and_line() {
    let ran = run_js(with_js(sales(), REPORT), "broken");
    let e = err(&ran.outcome);
    assert_eq!(e.number, 438, "{e}");
    assert_eq!(
        (e.module.as_str(), e.procedure.as_str(), e.line),
        ("Report", "broken", 37)
    );
    assert_eq!(e.title(), "Run-time error '438':");

    let ran = run_js(with_js(sales(), "function go() { null.x; }"), "go");
    let e = err(&ran.outcome);
    assert_eq!(e.number, 0);
    assert_eq!(e.title(), "Run-time error:");
    assert!(e.description.starts_with("TypeError"), "{e}");

    let ran = run_js(with_js(sales(), "function go( {"), "Report.go");
    assert_eq!(err(&ran.outcome).title(), "Compile error:");
}

#[test]
fn stop_ends_a_running_script() {
    let options = RunOptions::default();
    options.cancel.cancel();
    let ran = run_with(with_js(sales(), REPORT), "spin", &options);
    let e = err(&ran.outcome);
    assert_eq!(e.number, 18, "{e}");
    assert_eq!(e.title(), "Code execution has been interrupted");
}

#[test]
fn a_script_makes_a_chart() {
    let mut book = Workbook::new_empty().unwrap();
    let rows = [
        ["Month", "Sales", "Costs"],
        ["Jan", "120", "80"],
        ["Feb", "135", "90"],
        ["Mar", "150", "95"],
    ];
    for (i, row) in rows.iter().enumerate() {
        for (j, v) in row.iter().enumerate() {
            book.set_input(0, i as u32, j as u32, v).unwrap();
        }
    }
    let source = r#"
function makeChart() {
  const co = ActiveSheet.chartObjects.add({ left: 200, top: 20, width: 360, height: 216 });
  const ch = co.chart;
  ch.setSourceData(Range("A1:C4"));
  ch.chartType = xlLine;
  ch.hasTitle = true;
  ch.chartTitle.text = "Sales";
  print(ActiveSheet.chartObjects.count, ch.seriesCollection.count);
}
"#;
    let ran = run_js(with_js(book, source), "makeChart");
    ok(&ran.outcome);
    assert_eq!(ran.printed, vec!["1 2".to_owned()]);
    let charts: Vec<_> = ran
        .book
        .drawings(0)
        .into_iter()
        .filter(|d| matches!(*d.kind, DrawingKind::Chart { .. }))
        .collect();
    assert_eq!(charts.len(), 1);
    let info = ran.book.chart_info(charts[0].key).unwrap();
    assert_eq!(info.spec.kind, ChartType::Line);
    assert_eq!(info.spec.title, "Sales");
}

#[test]
fn scripts_are_saved_with_the_workbook() {
    let book = with_js(sales(), REPORT);
    let ran = run_js(book, "report");
    ok(&ran.outcome);
    let (book, _dir) = reopen(ran.book, "report.xlsx");
    let scripts = book.scripts().unwrap();
    assert_eq!(scripts.len(), 1);
    assert_eq!(scripts[0].name, "Report");
    assert_eq!(scripts[0].source, REPORT);
    assert_eq!(cell(&book, 0, "F1"), "700.00");
    // They run again from the reopened file.
    let (book, _dir2) = reopen(book, "again.xlsm");
    let mut book = book;
    book.set_input(0, 1, 2, "1000").unwrap();
    let ran = run_js(book, "report");
    ok(&ran.outcome);
    assert_eq!(cell(&ran.book, 0, "F1"), "1,400.00");
}
