//! The Macro Editor's path: code typed into a workbook (a new VBA project,
//! or an edited one, and scripts), saved as `.xlsm`, opened again and run.

mod support;

use sheets_engine::Workbook;
use sheets_engine::vba::ModuleKind;
use sheets_script::code::{self, CodeKind, CodeModule};
use sheets_script::{HeadlessUi, MacroEngine, MacroProject, RunOptions, View};
use support::*;

const BASICS: &str = "Option Explicit\n\nSub Basics()\n    Dim i As Long\n    For i = 1 To 3\n        Cells(i, 1).Value = i * 10\n    Next i\n    Range(\"B1\").Value = \"done\"\n    Debug.Print \"ran\"\nEnd Sub\n";

fn run(book: Workbook, name: &str) -> (Workbook, sheets_script::RunOutcome) {
    let macros = MacroProject::of_book(&book)
        .expect("the workbook has macros")
        .expect("they can be read");
    let mut engine = MacroEngine::new(macros);
    engine.run(
        book,
        name,
        View::default(),
        Box::new(HeadlessUi::new(Vec::new(), false)),
        &RunOptions::default(),
    )
}

#[test]
fn new_workbook_new_module_saved_as_xlsm_runs_after_reopening() {
    let mut book = Workbook::new_empty().unwrap();
    book.insert_sheet(1).unwrap();
    // A workbook without a project shows ThisWorkbook and a sheet module each.
    let mut modules = code::code_modules(&book).unwrap();
    let names: Vec<&str> = modules.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["ThisWorkbook", "Sheet1", "Sheet2"]);
    assert!(!book.has_macros());
    // Nothing typed: nothing changes.
    assert!(!code::apply(&mut book, &modules).unwrap());
    assert!(!book.has_macros());

    // Insert Module, paste code.
    let name = code::free_name(&modules, CodeKind::Standard);
    assert_eq!(name, "Module1");
    modules.push(CodeModule::new(&name, CodeKind::Standard, BASICS));
    assert!(code::diagnostics(modules.last().unwrap()).is_empty());
    assert_eq!(
        code::macro_at(modules.last().unwrap(), 5).as_deref(),
        Ok("Module1.Basics")
    );
    assert!(code::apply(&mut book, &modules).unwrap());
    assert!(book.has_macros());
    assert!(book.is_dirty());
    // Applying the same again changes nothing.
    assert!(!code::apply(&mut book, &modules).unwrap());

    // F5 in the editor: the live project is the edited one.
    let (book, outcome) = run(book, "Module1.Basics");
    ok(&outcome);
    assert_eq!(cell(&book, 0, "A3"), "30");

    // Save as .xlsm, open again, run from the macro list.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Book1.xlsm");
    let mut book = book;
    book.save_as(&path).unwrap();
    let ct = package_text(&path, "[Content_Types].xml").unwrap();
    assert!(ct.contains("macroEnabled.main+xml"), "{ct}");
    assert!(ct.contains("vbaProject"), "{ct}");
    let wb = package_text(&path, "xl/workbook.xml").unwrap();
    assert!(wb.contains("codeName=\"ThisWorkbook\""), "{wb}");
    let s2 = package_text(&path, "xl/worksheets/sheet2.xml").unwrap();
    assert!(s2.contains("codeName=\"Sheet2\""), "{s2}");

    let mut book = Workbook::open(&path).unwrap();
    assert!(book.has_macros());
    assert!(!book.vba_edited());
    let p = book.vba_project().unwrap().unwrap();
    assert_eq!(p.module("Module1").unwrap().kind, ModuleKind::Standard);
    assert_eq!(book.sheet_code_names().len(), 2);
    let macros = MacroProject::of_book(&book).unwrap().unwrap();
    assert!(
        macros
            .macros()
            .iter()
            .any(|m| m.full_name() == "Module1.Basics")
    );
    for r in 0..3 {
        book.set_input(0, r, 0, "").unwrap();
    }
    let (book, outcome) = run(book, "Module1.Basics");
    ok(&outcome);
    assert_eq!(cell(&book, 0, "A2"), "20");
    assert_eq!(cell(&book, 0, "B1"), "done");

    // Edit it again in the reopened file: the change reads back.
    let mut book = book;
    let mut modules = code::code_modules(&book).unwrap();
    let m = modules.iter_mut().find(|m| m.name == "Module1").unwrap();
    m.code = m.code.replace("i * 10", "i * 100");
    assert!(code::apply(&mut book, &modules).unwrap());
    let (book, outcome) = run(book, "Basics");
    ok(&outcome);
    assert_eq!(cell(&book, 0, "A3"), "300");
}

#[test]
fn a_sheet_module_handles_events_after_saving() {
    let mut book = Workbook::new_empty().unwrap();
    let mut modules = code::code_modules(&book).unwrap();
    let sheet = modules.iter_mut().find(|m| m.name == "Sheet1").unwrap();
    assert_eq!(sheet.sheet.as_deref(), Some("Sheet1"));
    sheet.code =
        "Public Sub Hello()\n    Me.Range(\"C1\").Value = \"from sheet\"\nEnd Sub\n".into();
    code::apply(&mut book, &modules).unwrap();
    let (book, _dir) = reopen(book, "events.xlsm");
    let (book, outcome) = run(book, "Sheet1.Hello");
    ok(&outcome);
    assert_eq!(cell(&book, 0, "C1"), "from sheet");
}

#[test]
fn xlsx_keeps_scripts_but_not_vba() {
    let mut book = Workbook::new_empty().unwrap();
    let mut modules = code::code_modules(&book).unwrap();
    modules.push(CodeModule::new("Module1", CodeKind::Standard, BASICS));
    modules.push(CodeModule::new(
        "Report",
        CodeKind::JavaScript,
        "function go() {\n  Range('D1').value = 7;\n}\n",
    ));
    code::apply(&mut book, &modules).unwrap();
    let (book, _dir) = reopen(book, "plain.xlsx");
    assert!(!book.has_macros());
    let after = code::code_modules(&book).unwrap();
    assert!(
        after
            .iter()
            .any(|m| m.name == "Report" && m.kind == CodeKind::JavaScript)
    );
    assert!(!after.iter().any(|m| m.name == "Module1"));
    let (book, outcome) = run(book, "Report.go");
    ok(&outcome);
    assert_eq!(cell(&book, 0, "D1"), "7");
}

#[test]
fn removing_every_module_but_the_documents_keeps_a_project() {
    let mut book = Workbook::new_empty().unwrap();
    let mut modules = code::code_modules(&book).unwrap();
    modules.push(CodeModule::new("Module1", CodeKind::Standard, BASICS));
    modules.push(CodeModule::new(
        "Counter",
        CodeKind::Class,
        "Public N As Long\n",
    ));
    code::apply(&mut book, &modules).unwrap();
    let (mut book, _dir) = reopen(book, "two.xlsm");
    let mut modules = code::code_modules(&book).unwrap();
    let kinds: Vec<CodeKind> = modules.iter().map(|m| m.kind).collect();
    assert_eq!(
        kinds,
        [
            CodeKind::Document,
            CodeKind::Document,
            CodeKind::Standard,
            CodeKind::Class
        ]
    );
    modules.retain(|m| m.kind == CodeKind::Document);
    assert!(code::apply(&mut book, &modules).unwrap());
    let p = book.vba_project().unwrap().unwrap();
    assert_eq!(p.modules().len(), 2);
    // A compile error shows at its line.
    let broken = CodeModule::new(
        "Module9",
        CodeKind::Standard,
        "Sub A()\n    If 1 Then\nEnd Sub\n",
    );
    let d = code::diagnostics(&broken);
    assert!(d.iter().any(|d| d.fatal), "{d:?}");
}
