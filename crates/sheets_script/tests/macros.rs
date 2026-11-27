//! Realistic macros against real workbooks: each test writes a module,
//! runs a macro headless and checks the cells (and that one undo step
//! takes the whole run back).

use sheets_engine::Workbook;
use sheets_engine::vba::ModuleKind;
use sheets_script::run::ModuleSource;
use sheets_script::{
    HeadlessUi, MacroError, MacroProject, RunOptions, RunOutcome, Selection, View, run_headless,
};

fn project(code: &str) -> MacroProject {
    MacroProject {
        modules: vec![ModuleSource::new(
            "Module1",
            ModuleKind::Standard,
            &code.replace('\n', "\r\n"),
        )],
        ..Default::default()
    }
}

fn run_with(
    book: Workbook,
    code: &str,
    name: &str,
    inputs: &[&str],
) -> (Workbook, RunOutcome, Vec<String>, Vec<String>) {
    let ui = HeadlessUi::new(inputs.iter().map(|s| s.to_string()).collect(), false);
    let shown = ui.shown.clone();
    let printed = ui.printed.clone();
    let (book, outcome) = run_headless(
        book,
        &project(code),
        name,
        View::default(),
        ui,
        &RunOptions {
            step_limit: Some(50_000_000),
            ..Default::default()
        },
    );
    let shown = shown.borrow().clone();
    let printed = printed.borrow().clone();
    (book, outcome, shown, printed)
}

fn run(book: Workbook, code: &str, name: &str) -> (Workbook, RunOutcome) {
    let (book, outcome, _, _) = run_with(book, code, name, &[]);
    (book, outcome)
}

fn ok(outcome: &RunOutcome) {
    if let Err(e) = &outcome.result {
        panic!("macro failed: {e}");
    }
}

fn err(outcome: &RunOutcome) -> &MacroError {
    outcome.result.as_ref().expect_err("the macro should fail")
}

fn cell(book: &Workbook, sheet: u32, a1: &str) -> String {
    let (r, c) = sheets_script::addr::parse_cell(a1).unwrap();
    book.display_value(sheet, r, c)
}

fn input(book: &Workbook, sheet: u32, a1: &str) -> String {
    let (r, c) = sheets_script::addr::parse_cell(a1).unwrap();
    book.input(sheet, r, c)
}

fn set(book: &mut Workbook, sheet: u32, a1: &str, v: &str) {
    let (r, c) = sheets_script::addr::parse_cell(a1).unwrap();
    book.set_input(sheet, r, c, v).unwrap();
}

/// A small table: Name, Region, Amount.
fn sales() -> Workbook {
    let mut b = Workbook::new_empty().unwrap();
    let rows = [
        ["Name", "Region", "Amount"],
        ["Dewi", "East", "300"],
        ["Budi", "West", "150"],
        ["Andi", "East", "200"],
        ["Citra", "North", "50"],
    ];
    for (i, row) in rows.iter().enumerate() {
        for (j, v) in row.iter().enumerate() {
            b.set_input(0, i as u32, j as u32, v).unwrap();
        }
    }
    b
}

#[test]
fn loops_fill_rows_and_the_last_row_idiom_finds_the_end() {
    let code = r##"
Sub FillRows()
    Dim i As Long, lastRow As Long
    For i = 1 To 10
        Cells(i, 1).Value = i
        Cells(i, 2).Formula = "=A" & i & "*2"
    Next i
    lastRow = Cells(Rows.Count, "A").End(xlUp).Row
    Range("D1").Value = lastRow
    Range("D2").Value = WorksheetFunction.Sum(Range("B1:B" & lastRow))
    Range("D3").Value = Application.WorksheetFunction.CountIf(Range("A:A"), ">5")
    Range("D4").Value = Range("A1").End(xlDown).Address
    Range("D5").Value = Range("A1").End(xlToRight).Column
End Sub
"##;
    let (book, outcome) = run(Workbook::new_empty().unwrap(), code, "FillRows");
    ok(&outcome);
    assert_eq!(cell(&book, 0, "A10"), "10");
    assert_eq!(input(&book, 0, "B3"), "=A3*2");
    assert_eq!(cell(&book, 0, "B10"), "20");
    assert_eq!(cell(&book, 0, "D1"), "10");
    assert_eq!(cell(&book, 0, "D2"), "110");
    assert_eq!(cell(&book, 0, "D3"), "5");
    assert_eq!(cell(&book, 0, "D4"), "$A$10");
    assert_eq!(cell(&book, 0, "D5"), "2");
    assert!(outcome.changed);
}

#[test]
fn one_undo_takes_the_whole_run_back() {
    let mut book = sales();
    set(&mut book, 0, "E1", "keep");
    let code = r##"
Sub Mess()
    Dim c As Range
    For Each c In Range("C2:C5")
        c.Value = c.Value * 10
    Next
    Range("A1:C1").Font.Bold = True
    Worksheets.Add After:=Worksheets(Worksheets.Count)
    ActiveSheet.Name = "Report"
    Worksheets("Report").Range("A1").Value = "made"
    Worksheets(1).Rows(3).Delete
End Sub
"##;
    let (mut book, outcome) = run(book, code, "Mess");
    ok(&outcome);
    assert_eq!(book.sheet_names(), ["Sheet1", "Report"]);
    assert_eq!(cell(&book, 0, "C2"), "3000");
    assert_eq!(cell(&book, 0, "A3"), "Andi");
    assert_eq!(cell(&book, 1, "A1"), "made");
    assert!(book.cell_style(0, 0, 0).bold);
    assert_eq!(outcome.view.sheet, 1);
    // One step back: everything as before the run.
    assert!(book.undo().unwrap());
    assert_eq!(book.sheet_names(), ["Sheet1"]);
    assert_eq!(cell(&book, 0, "C2"), "300");
    assert_eq!(cell(&book, 0, "A3"), "Budi");
    assert_eq!(cell(&book, 0, "E1"), "keep");
    assert!(!book.cell_style(0, 0, 0).bold);
    assert!(book.redo().unwrap());
    assert_eq!(cell(&book, 0, "C2"), "3000");
}

#[test]
fn copy_and_paste_values_and_formats() {
    let mut book = sales();
    set(&mut book, 0, "D2", "=C2*2");
    let code = r##"
Sub CopyStuff()
    Range("D2").Font.Italic = True
    Range("A1:D2").Copy Destination:=Range("F1")
    Range("D2").Copy
    Range("H5").PasteSpecial Paste:=xlPasteValues
    Range("D2").Copy
    Range("H6").PasteSpecial xlPasteFormats
    Application.CutCopyMode = False
    Range("A5:C5").Cut Range("A7")
    Dim v As Variant
    v = Range("A2:C3").Value
    Range("J1").Resize(2, 3).Value = v
    Range("N1").Value = UBound(v, 1) & "x" & UBound(v, 2)
End Sub
"##;
    let (book, outcome) = run(book, code, "CopyStuff");
    ok(&outcome);
    assert_eq!(cell(&book, 0, "F1"), "Name");
    // The copied formula moved with the cells.
    assert_eq!(input(&book, 0, "I2"), "=H2*2");
    assert_eq!(input(&book, 0, "H5"), "600");
    assert!(book.cell_style(0, 5, 7).italic);
    assert_eq!(cell(&book, 0, "A5"), "");
    assert_eq!(cell(&book, 0, "A7"), "Citra");
    assert_eq!(cell(&book, 0, "K2"), "West");
    assert_eq!(cell(&book, 0, "N1"), "2x3");
}

#[test]
fn find_and_replace() {
    let code = r##"
Sub FindIt()
    Dim f As Range, first As String, n As Long
    Set f = Range("A1:C5").Find(What:="east", LookIn:=xlValues, LookAt:=xlWhole, MatchCase:=False)
    If Not f Is Nothing Then
        first = f.Address
        Do
            n = n + 1
            f.Offset(0, 1).Value = f.Offset(0, 1).Value + 1
            Set f = Range("A1:C5").FindNext(f)
        Loop While Not f Is Nothing And f.Address <> first
    End If
    Range("E1").Value = n
    Range("E2").Value = first
    Range("B1:B5").Replace What:="West", Replacement:="Barat", LookAt:=xlWhole
    If Range("A1:C5").Find("Nobody") Is Nothing Then Range("E3").Value = "none"
End Sub
"##;
    let (book, outcome) = run(sales(), code, "FindIt");
    ok(&outcome);
    assert_eq!(cell(&book, 0, "E1"), "2");
    assert_eq!(cell(&book, 0, "E2"), "$B$2");
    assert_eq!(cell(&book, 0, "C2"), "301");
    assert_eq!(cell(&book, 0, "C4"), "201");
    assert_eq!(cell(&book, 0, "B3"), "Barat");
    assert_eq!(cell(&book, 0, "E3"), "none");
}

#[test]
fn sort_both_ways() {
    let code = r##"
Sub SortIt()
    Range("A1:C5").Sort Key1:=Range("C1"), Order1:=xlDescending, Header:=xlYes
End Sub

Sub SortRecorded()
    ActiveSheet.Sort.SortFields.Clear
    ActiveSheet.Sort.SortFields.Add Key:=Range("A2:A5"), SortOn:=xlSortOnValues, Order:=xlAscending
    With ActiveSheet.Sort
        .SetRange Range("A1:C5")
        .Header = xlYes
        .Apply
    End With
End Sub
"##;
    let (book, outcome) = run(sales(), code, "SortIt");
    ok(&outcome);
    let col = |b: &Workbook, c: &str| {
        (2..=5)
            .map(|r| cell(b, 0, &format!("{c}{r}")))
            .collect::<Vec<_>>()
    };
    assert_eq!(col(&book, "C"), ["300", "200", "150", "50"]);
    assert_eq!(cell(&book, 0, "A1"), "Name");
    let (book, outcome) = run(book, code, "SortRecorded");
    ok(&outcome);
    assert_eq!(col(&book, "A"), ["Andi", "Budi", "Citra", "Dewi"]);
}

#[test]
fn autofilter_hides_rows_and_comes_off() {
    let code = r##"
Sub FilterEast()
    Range("A1").AutoFilter Field:=2, Criteria1:="East"
    Range("E1").Value = ActiveSheet.AutoFilterMode
    Range("E2").Value = Rows(3).Hidden
    Range("E3").Value = Rows(2).Hidden
End Sub

Sub FilterBig()
    Range("A1:C5").AutoFilter Field:=3, Criteria1:=">=200"
End Sub

Sub Unfilter()
    If ActiveSheet.FilterMode Then ActiveSheet.ShowAllData
    Range("E4").Value = Rows(3).Hidden
    ActiveSheet.AutoFilterMode = False
    Range("E5").Value = ActiveSheet.AutoFilterMode
End Sub
"##;
    let (book, outcome) = run(sales(), code, "FilterEast");
    ok(&outcome);
    assert_eq!(cell(&book, 0, "E1"), "TRUE");
    assert_eq!(cell(&book, 0, "E2"), "TRUE");
    assert_eq!(cell(&book, 0, "E3"), "FALSE");
    let (book, outcome) = run(book, code, "Unfilter");
    ok(&outcome);
    assert_eq!(cell(&book, 0, "E4"), "FALSE");
    assert_eq!(cell(&book, 0, "E5"), "FALSE");
    let (book, outcome) = run(book, code, "FilterBig");
    ok(&outcome);
    assert!(book.is_row_hidden(0, 2));
    assert!(book.is_row_hidden(0, 4));
    assert!(!book.is_row_hidden(0, 1));
}

#[test]
fn formatting() {
    let code = r##"
Sub Pretty()
    With Range("A1:C1")
        .Font.Bold = True
        .Font.Size = 14
        .Font.Color = RGB(255, 0, 0)
        .Interior.Color = RGB(255, 255, 0)
        .HorizontalAlignment = xlCenter
    End With
    Range("C2:C5").NumberFormat = "#,##0.00"
    Columns("A").ColumnWidth = 20
    Rows(1).RowHeight = 30
    Range("A1:C5").Borders.LineStyle = xlContinuous
    Range("A2").WrapText = True
    Range("A7:C7").Merge
    Range("F1").Value = Range("A1").Font.Bold
    Range("F2").Value = Range("C2").Text
    Range("F3").Value = Range("A7").MergeCells
    Range("F4").Value = Range("A1").Interior.Color = RGB(255, 255, 0)
    Range("F5").Value = Columns("A").ColumnWidth
    Range("F6").Value = Range("C2").NumberFormat
End Sub
"##;
    let (book, outcome) = run(sales(), code, "Pretty");
    ok(&outcome);
    let st = book.cell_style(0, 0, 0);
    assert!(st.bold);
    assert_eq!(st.font_size, 14);
    assert_eq!(st.font_color, Some(0xFF0000));
    assert_eq!(st.fill, Some(0xFFFF00));
    assert_eq!(st.align, sheets_engine::HAlign::Center);
    assert!(book.cell_style(0, 1, 0).wrap);
    assert!(book.cell_style(0, 1, 1).borders.left.is_some());
    assert_eq!(cell(&book, 0, "C2"), "300.00");
    assert_eq!(cell(&book, 0, "F1"), "TRUE");
    // Text "300.00" written as a value is the number 300, as in Excel.
    assert_eq!(cell(&book, 0, "F2"), "300");
    assert_eq!(cell(&book, 0, "F3"), "TRUE");
    assert_eq!(cell(&book, 0, "F4"), "TRUE");
    assert_eq!(cell(&book, 0, "F5"), "20");
    assert_eq!(cell(&book, 0, "F6"), "#,##0.00");
    assert!(book.merge_at(0, 6, 1).is_some());
    assert!((book.row_points(0, 0) - 30.0).abs() < 0.01);
}

#[test]
fn sheets_are_added_renamed_and_found() {
    let code = r##"
Sub SheetWork()
    Dim ws As Worksheet, names As String
    Set ws = Worksheets.Add(After:=Worksheets(1))
    ws.Name = "Data"
    Worksheets.Add Before:=Worksheets(1)
    ActiveSheet.Name = "First"
    For Each ws In Worksheets
        names = names & ws.Name & ","
    Next
    Worksheets("Data").Range("A1").Value = names
    Worksheets("Data").Range("A2").Value = Worksheets.Count
    Worksheets("Data").Range("A3").Value = Worksheets("Data").Index
    Sheets("Sheet1").Activate
    Range("B1").Value = ActiveSheet.Name
End Sub
"##;
    let (book, outcome) = run(Workbook::new_empty().unwrap(), code, "SheetWork");
    ok(&outcome);
    assert_eq!(book.sheet_names(), ["First", "Sheet1", "Data"]);
    assert_eq!(cell(&book, 2, "A1"), "First,Sheet1,Data,");
    assert_eq!(cell(&book, 2, "A2"), "3");
    assert_eq!(cell(&book, 2, "A3"), "3");
    assert_eq!(cell(&book, 1, "B1"), "Sheet1");
    assert_eq!(outcome.view.sheet, 1);
}

#[test]
fn worksheet_functions_and_their_errors() {
    let code = r##"
Sub Lookups()
    Range("E1").Value = WorksheetFunction.VLookup("Andi", Range("A1:C5"), 3, False)
    Range("E2").Value = WorksheetFunction.Match("Budi", Range("A:A"), 0)
    Range("E3").Value = WorksheetFunction.Max(Range("C2:C5"))
    Range("E4").Value = Application.Max(1, 7, 3)
    Dim v As Variant
    v = Application.VLookup("Nobody", Range("A1:C5"), 3, False)
    If IsError(v) Then Range("E5").Value = "missing"
    On Error Resume Next
    v = WorksheetFunction.VLookup("Nobody", Range("A1:C5"), 3, False)
    Range("E6").Value = Err.Number
    On Error GoTo 0
    Range("E7").Value = Evaluate("SUM(C2:C5)")
    Range("E8").Value = [C2] + 1
End Sub
"##;
    let (book, outcome) = run(sales(), code, "Lookups");
    ok(&outcome);
    assert_eq!(cell(&book, 0, "E1"), "200");
    assert_eq!(cell(&book, 0, "E2"), "3");
    assert_eq!(cell(&book, 0, "E3"), "300");
    assert_eq!(cell(&book, 0, "E4"), "7");
    assert_eq!(cell(&book, 0, "E5"), "missing");
    assert_eq!(cell(&book, 0, "E6"), "1004");
    assert_eq!(cell(&book, 0, "E7"), "700");
    assert_eq!(cell(&book, 0, "E8"), "301");
}

#[test]
fn runtime_errors_have_numbers_and_lines() {
    let code = r##"
Sub BadRange()
    Range("A1").Value = 1
    Range("NotARange!!").Value = 2
End Sub

Sub BadSheet()
    Worksheets("Missing").Activate
End Sub

Sub NoObject()
    Dim r As Range
    r.Value = 1
End Sub

Sub Mismatch()
    Range("A1").Value = "x"
    Range("A2").Value = Range("A1").Value * 2
End Sub
"##;
    let (book, outcome) = run(Workbook::new_empty().unwrap(), code, "BadRange");
    let e = err(&outcome);
    assert_eq!(e.number, 1004);
    assert_eq!(e.module, "Module1");
    assert_eq!(e.line, 4);
    assert_eq!(e.title(), "Run-time error '1004':");
    // What ran before the error stays (and undoes as one step).
    assert_eq!(cell(&book, 0, "A1"), "1");
    let (_, outcome) = run(book, code, "BadSheet");
    assert_eq!(err(&outcome).number, 9);
    let (_, outcome) = run(Workbook::new_empty().unwrap(), code, "NoObject");
    assert_eq!(err(&outcome).number, 91);
    let (_, outcome) = run(Workbook::new_empty().unwrap(), code, "Mismatch");
    assert_eq!(err(&outcome).number, 13);
}

#[test]
fn dialogs_headless() {
    let code = r##"
Sub Ask()
    Dim name As String, n As Variant, answer As VbMsgBoxResult
    name = InputBox("Your name?", "Hello", "nobody")
    n = Application.InputBox("How many?", Type:=1)
    answer = MsgBox("Hi " & name & ", " & n, vbYesNo + vbQuestion, "Greeting")
    Range("A1").Value = name
    Range("A2").Value = n * 2
    Range("A3").Value = answer = vbYes
    Debug.Print "done"; n
End Sub
"##;
    let (book, outcome, shown, printed) =
        run_with(Workbook::new_empty().unwrap(), code, "Ask", &["Sari", "21"]);
    ok(&outcome);
    assert_eq!(cell(&book, 0, "A1"), "Sari");
    assert_eq!(cell(&book, 0, "A2"), "42");
    assert_eq!(cell(&book, 0, "A3"), "TRUE");
    assert_eq!(shown, ["Hi Sari, 21"]);
    assert_eq!(printed.len(), 1);
    assert!(printed[0].starts_with("done"), "{printed:?}");
    // No input left: a clear error.
    let (_, outcome, _, _) = run_with(Workbook::new_empty().unwrap(), code, "Ask", &[]);
    assert!(err(&outcome).description.contains("--input"));
}

#[test]
fn selection_and_active_cell_follow_the_macro() {
    let code = r##"
Sub Walk()
    Range("B2").Select
    ActiveCell.Value = "here"
    ActiveCell.Offset(1, 0).Select
    Selection.Value = "below"
    Range("C3:D4").Select
    Selection.Interior.ColorIndex = 6
End Sub
"##;
    let (book, outcome) = run(Workbook::new_empty().unwrap(), code, "Walk");
    ok(&outcome);
    assert_eq!(cell(&book, 0, "B2"), "here");
    assert_eq!(cell(&book, 0, "B3"), "below");
    assert_eq!(book.cell_style(0, 3, 3).fill, Some(0xFFFF00));
    assert_eq!(
        outcome.view.selection,
        Selection {
            range: sheets_engine::CellRange::from_corners((2, 2), (3, 3)),
            active: (2, 2),
        }
    );
}

#[test]
fn formulas_in_r1c1_and_special_cells() {
    let mut book = Workbook::new_empty().unwrap();
    for r in 0..4 {
        book.set_input(0, r, 0, &(r + 1).to_string()).unwrap();
    }
    let code = r##"
Sub R1C1()
    Range("B1:B4").FormulaR1C1 = "=RC[-1]*10"
    Range("C1").Value = "'" & Range("B2").Formula
    Range("C2").Value = "'" & Range("B2").FormulaR1C1
    Range("C3").Value = Range("A1:B4").SpecialCells(xlCellTypeFormulas).Count
    Range("C4").Value = Cells.SpecialCells(xlCellTypeLastCell).Address
    Range("C5").Value = Range("A1").CurrentRegion.Address
    Range("C6").Value = Range("B3").HasFormula
End Sub
"##;
    let (book, outcome) = run(book, code, "R1C1");
    ok(&outcome);
    assert_eq!(input(&book, 0, "B3"), "=A3*10");
    assert_eq!(cell(&book, 0, "B4"), "40");
    assert_eq!(cell(&book, 0, "C1"), "=A2*10");
    assert_eq!(cell(&book, 0, "C2"), "=RC[-1]*10");
    assert_eq!(cell(&book, 0, "C3"), "4");
    assert_eq!(cell(&book, 0, "C4"), "$C$4");
    assert_eq!(cell(&book, 0, "C5"), "$A$1:$C$4");
    assert_eq!(cell(&book, 0, "C6"), "TRUE");
}

#[test]
fn remove_duplicates_and_delete_shift() {
    let mut book = Workbook::new_empty().unwrap();
    for (r, v) in ["k", "a", "b", "a", "c", "b"].iter().enumerate() {
        book.set_input(0, r as u32, 0, v).unwrap();
    }
    book.set_input(0, 0, 3, "x").unwrap();
    book.set_input(0, 1, 3, "y").unwrap();
    book.set_input(0, 2, 3, "z").unwrap();
    let code = r##"
Sub Dedupe()
    Range("A1:A6").RemoveDuplicates Columns:=1, Header:=xlYes
    Range("D1").Delete Shift:=xlUp
    Range("E1").Insert Shift:=xlDown
End Sub
"##;
    let (book, outcome) = run(book, code, "Dedupe");
    ok(&outcome);
    let col: Vec<String> = (1..=6).map(|r| cell(&book, 0, &format!("A{r}"))).collect();
    assert_eq!(col, ["k", "a", "b", "c", "", ""]);
    assert_eq!(cell(&book, 0, "D1"), "y");
    assert_eq!(cell(&book, 0, "D2"), "z");
    assert_eq!(cell(&book, 0, "D3"), "");
}

#[test]
fn a_cancelled_run_stops_with_error_18() {
    let code = r##"
Sub Forever()
    Do
        Range("A1").Value = Range("A1").Value + 1
    Loop
End Sub
"##;
    let ui = HeadlessUi::new(Vec::new(), false);
    let (book, outcome) = run_headless(
        Workbook::new_empty().unwrap(),
        &project(code),
        "Forever",
        View::default(),
        ui,
        &RunOptions {
            step_limit: Some(20_000),
            ..Default::default()
        },
    );
    let e = err(&outcome);
    assert_eq!(e.number, 18);
    // Where it stopped, as for any other error.
    assert_eq!(
        (e.module.as_str(), e.procedure.as_str()),
        ("Module1", "Forever")
    );
    assert!((3..=5).contains(&e.line), "line {}", e.line);
    assert!(cell(&book, 0, "A1").parse::<i64>().unwrap() > 100);
}

#[test]
fn vba_functions_in_cell_formulas() {
    let macros = project(
        r#"
Public Function MyTax(amount As Double) As Double
    MyTax = amount * 0.1
End Function

Public Function Joined(r As Variant) As String
    Dim v As Variant, s As String
    For Each v In r
        s = s & v
    Next
    Joined = s
End Function
"#,
    );
    let mut book = sales();
    set(&mut book, 0, "D2", "=MyTax(C2)");
    set(&mut book, 0, "D3", "=Joined(A2:A3)");
    // Macros off: Excel's #NAME?.
    book.recalculate();
    assert_eq!(cell(&book, 0, "D2"), "#NAME?");
    sheets_script::enable_functions(&mut book, Some(&macros));
    book.recalculate();
    assert_eq!(cell(&book, 0, "D2"), "30");
    assert_eq!(cell(&book, 0, "D3"), "DewiBudi");
    // The formula text is as written.
    assert_eq!(input(&book, 0, "D2"), "=MyTax(C2)");
    set(&mut book, 0, "C2", "1000");
    assert_eq!(cell(&book, 0, "D2"), "100");
    sheets_script::enable_functions(&mut book, None);
    book.recalculate();
    assert_eq!(cell(&book, 0, "D2"), "#NAME?");
}

#[test]
fn an_interrupt_reads_once() {
    let e = MacroError {
        number: 18,
        description: "Code execution has been interrupted".into(),
        kind: kantor_script::ErrorKind::Interrupted,
        module: "Module1".into(),
        procedure: "Spin".into(),
        line: 4,
    };
    assert_eq!(
        e.to_string(),
        "Code execution has been interrupted (in Module1.Spin, line 4)"
    );
}
