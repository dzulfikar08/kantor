//! Events, the live project, other workbooks and VBA functions with
//! `Range` arguments, on real workbooks through `MacroEngine`.

use std::path::PathBuf;

use sheets_engine::Workbook;
use sheets_engine::vba::ModuleKind;
use sheets_script::run::ModuleSource;
use sheets_script::{
    BookOut, Event, EventKind, HeadlessUi, MacroEngine, MacroProject, RunOptions, RunOutcome, View,
};

fn modules(list: &[(&str, ModuleKind, &str)]) -> MacroProject {
    MacroProject {
        modules: list
            .iter()
            .map(|(name, kind, src)| ModuleSource::new(name, *kind, &src.replace('\n', "\r\n")))
            .collect(),
        ..Default::default()
    }
}

fn engine(list: &[(&str, ModuleKind, &str)]) -> MacroEngine {
    MacroEngine::new(modules(list))
}

fn ui() -> HeadlessUi {
    HeadlessUi::new(Vec::new(), false)
}

fn opts() -> RunOptions {
    RunOptions {
        step_limit: Some(5_000_000),
        ..Default::default()
    }
}

fn ok(o: &RunOutcome) {
    if let Err(e) = &o.result {
        panic!("macro failed: {e}");
    }
}

fn cell(book: &Workbook, sheet: u32, a1: &str) -> String {
    let (r, c) = sheets_script::addr::parse_cell(a1).unwrap();
    book.display_value(sheet, r, c)
}

fn set(book: &mut Workbook, sheet: u32, a1: &str, v: &str) {
    let (r, c) = sheets_script::addr::parse_cell(a1).unwrap();
    book.set_input(sheet, r, c, v).unwrap();
}

fn area(a1: &str) -> sheets_engine::CellRange {
    sheets_script::addr::parse_area(a1).unwrap()
}

/// A user edit of `a1` followed by its `Change` event, folded together.
fn edit(
    engine: &mut MacroEngine,
    book: Workbook,
    a1: &str,
    v: &str,
    ui: HeadlessUi,
) -> (Workbook, RunOutcome) {
    let mut book = book;
    let mark = book.undo_mark();
    set(&mut book, 0, a1, v);
    engine.fire(
        book,
        &Event::Change {
            sheet: 0,
            areas: vec![area(a1)],
        },
        View::default(),
        Box::new(ui),
        &opts(),
        Some(mark),
    )
}

#[test]
fn worksheet_change_validates_input_and_undoes_it() {
    let sheet = r#"
Private Sub Worksheet_Change(ByVal Target As Range)
    If Target.Column = 1 And Not IsNumeric(Target.Value) Then
        Application.EnableEvents = False
        Application.Undo
        Application.EnableEvents = True
        MsgBox "Numbers only in column A"
    ElseIf Target.Column = 1 Then
        Application.EnableEvents = False
        Target.Offset(0, 1).Value = Target.Value * 2
        Application.EnableEvents = True
    End If
End Sub
"#;
    let mut e = engine(&[("Sheet1", ModuleKind::Document, sheet)]);
    assert!(e.index().wants(EventKind::Change));
    assert!(!e.index().wants(EventKind::BeforeSave));
    let mut book = Workbook::new_empty().unwrap();
    set(&mut book, 0, "A1", "5");
    let ui1 = ui();
    let shown = ui1.shown.clone();
    let (book, out) = edit(&mut e, book, "A1", "abc", ui1);
    ok(&out);
    assert_eq!(cell(&book, 0, "A1"), "5");
    assert_eq!(shown.borrow().as_slice(), ["Numbers only in column A"]);
    let (mut book, out) = edit(&mut e, book, "A2", "21", ui());
    ok(&out);
    assert_eq!(cell(&book, 0, "B2"), "42");
    // The edit and what its handler did undo together.
    book.undo().unwrap();
    assert_eq!(cell(&book, 0, "A2"), "");
    assert_eq!(cell(&book, 0, "B2"), "");
}

#[test]
fn macro_writes_fire_change_and_enable_events_stops_recursion() {
    // Handlers nest on the native stack: as on the desktop's macro thread.
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(macro_writes_fire_change)
        .unwrap()
        .join()
        .unwrap();
}

fn macro_writes_fire_change() {
    let module = r#"
Public Calls As Long
Sub Fill()
    Range("A1").Value = 1
    Range("A2:A3").Value = 2
    Range("C1").Value = Calls
End Sub
Sub Forever()
    Range("E1").Value = 1
End Sub
"#;
    let sheet = r#"
Private Sub Worksheet_Change(ByVal Target As Range)
    If Target.Column = 1 Then
        Calls = Calls + 1
        Application.EnableEvents = False
        Target.Offset(0, 1).Value = "seen " & Target.Address(False, False)
        Application.EnableEvents = True
    ElseIf Target.Column = 5 Then
        ' No guard: each write fires Change again.
        Target.Value = Target.Value + 1
    End If
End Sub
"#;
    let mut e = engine(&[
        ("Module1", ModuleKind::Standard, module),
        ("Sheet1", ModuleKind::Document, sheet),
    ]);
    let (book, out) = e.run(
        Workbook::new_empty().unwrap(),
        "Fill",
        View::default(),
        Box::new(ui()),
        &opts(),
    );
    ok(&out);
    assert_eq!(cell(&book, 0, "B1"), "seen A1");
    assert_eq!(cell(&book, 0, "B2"), "seen A2:A3");
    assert_eq!(cell(&book, 0, "C1"), "2");
    let (_, out) = e.run(book, "Forever", View::default(), Box::new(ui()), &opts());
    assert_eq!(out.result.unwrap_err().number, 28);
}

#[test]
fn cancel_stops_double_click_close_and_save() {
    let sheet = r#"
Private Sub Worksheet_BeforeDoubleClick(ByVal Target As Range, Cancel As Boolean)
    If Target.Address = "$B$2" Then
        Cancel = True
        Target.Value = "clicked"
    End If
End Sub

Private Sub Worksheet_BeforeRightClick(ByVal Target As Range, Cancel As Boolean)
    Cancel = True
End Sub
"#;
    let this = r#"
Private Sub Workbook_BeforeClose(Cancel As Boolean)
    Cancel = (Sheet1.Range("A1").Value = "keep")
End Sub

Private Sub Workbook_BeforeSave(ByVal SaveAsUI As Boolean, Cancel As Boolean)
    If SaveAsUI Then Cancel = True
End Sub
"#;
    let mut e = engine(&[
        ("Sheet1", ModuleKind::Document, sheet),
        ("ThisWorkbook", ModuleKind::Document, this),
    ]);
    let book = Workbook::new_empty().unwrap();
    let fire = |e: &mut MacroEngine, book: Workbook, ev: Event| {
        e.fire(book, &ev, View::default(), Box::new(ui()), &opts(), None)
    };
    let (book, out) = fire(
        &mut e,
        book,
        Event::BeforeDoubleClick {
            sheet: 0,
            row: 1,
            col: 1,
        },
    );
    ok(&out);
    assert!(out.cancel);
    assert_eq!(cell(&book, 0, "B2"), "clicked");
    let (book, out) = fire(
        &mut e,
        book,
        Event::BeforeDoubleClick {
            sheet: 0,
            row: 0,
            col: 0,
        },
    );
    assert!(!out.cancel);
    let (book, out) = fire(
        &mut e,
        book,
        Event::BeforeRightClick {
            sheet: 0,
            range: area("C3"),
        },
    );
    assert!(out.cancel);
    let (mut book, out) = fire(&mut e, book, Event::BeforeClose);
    assert!(!out.cancel);
    set(&mut book, 0, "A1", "keep");
    let (book, out) = fire(&mut e, book, Event::BeforeClose);
    ok(&out);
    assert!(out.cancel);
    let (book, out) = fire(&mut e, book, Event::BeforeSave { save_as_ui: true });
    assert!(out.cancel);
    let (_, out) = fire(&mut e, book, Event::BeforeSave { save_as_ui: false });
    assert!(!out.cancel);
}

#[test]
fn module_variables_last_between_runs_until_end() {
    let module = r#"
Dim n As Long
Public Total As Double
Sub Bump()
    n = n + 1
    Total = Total + 0.5
    Range("A1").Value = n
    Range("A2").Value = Total
End Sub
Sub Halt()
    End
End Sub
Sub Fails()
    n = 100
    Err.Raise 1004
End Sub
"#;
    let mut e = engine(&[("Module1", ModuleKind::Standard, module)]);
    let mut book = Workbook::new_empty().unwrap();
    for _ in 0..2 {
        let (b, out) = e.run(book, "Bump", View::default(), Box::new(ui()), &opts());
        ok(&out);
        book = b;
    }
    assert_eq!(cell(&book, 0, "A1"), "2");
    assert_eq!(cell(&book, 0, "A2"), "1");
    let (b, out) = e.run(book, "Halt", View::default(), Box::new(ui()), &opts());
    ok(&out);
    let (b, _) = e.run(b, "Bump", View::default(), Box::new(ui()), &opts());
    assert_eq!(cell(&b, 0, "A1"), "1");
    // An unhandled error ends the run like the error dialog's End.
    let (b, out) = e.run(b, "Fails", View::default(), Box::new(ui()), &opts());
    assert!(out.result.is_err());
    let (b, _) = e.run(b, "Bump", View::default(), Box::new(ui()), &opts());
    assert_eq!(cell(&b, 0, "A1"), "1");
}

#[test]
fn application_events_through_with_events_and_objects_kept_between_runs() {
    let this = r#"
Private WithEvents App As Application
Public Kept As Worksheet

Private Sub Workbook_Open()
    Set App = Application
    Set Kept = Sheet1
End Sub

Private Sub App_SheetChange(ByVal Sh As Object, ByVal Target As Range)
    Application.EnableEvents = False
    Sh.Range("Z1").Value = "app " & Sh.Name & " " & Target.Address
    Application.EnableEvents = True
End Sub
"#;
    let module = r#"
Sub UseKept()
    ThisWorkbook.Kept.Range("Y1").Value = "kept " & ThisWorkbook.Kept.Name
End Sub
"#;
    let mut e = engine(&[
        ("ThisWorkbook", ModuleKind::Document, this),
        ("Sheet1", ModuleKind::Document, ""),
        ("Module1", ModuleKind::Standard, module),
    ]);
    // No handler for Change until Workbook_Open hooks the Application, but
    // the index knows a WithEvents variable handles SheetChange.
    assert!(e.index().wants(EventKind::Change));
    let book = Workbook::new_empty().unwrap();
    let (book, out) = e.fire(
        book,
        &Event::Open,
        View::default(),
        Box::new(ui()),
        &opts(),
        None,
    );
    ok(&out);
    let (book, out) = edit(&mut e, book, "C3", "x", ui());
    ok(&out);
    assert_eq!(cell(&book, 0, "Z1"), "app Sheet1 $C$3");
    let (book, out) = e.run(book, "UseKept", View::default(), Box::new(ui()), &opts());
    ok(&out);
    assert_eq!(cell(&book, 0, "Y1"), "kept Sheet1");
}

#[test]
fn raise_event_and_implements_in_a_workbook() {
    let counter = r#"
Public Event Reached(ByVal n As Long)
Private n As Long
Public Sub Add()
    n = n + 1
    If n Mod 2 = 0 Then RaiseEvent Reached(n)
End Sub
"#;
    let listener = r#"
Implements IReport
Private WithEvents c As Counter
Private logText As String
Public Sub Listen(x As Counter)
    Set c = x
End Sub
Private Sub c_Reached(ByVal n As Long)
    logText = logText & n & " "
End Sub
Private Function IReport_Text() As String
    IReport_Text = Trim(logText)
End Function
"#;
    let report = "Public Function Text() As String\nEnd Function\n";
    let module = r#"
Sub Go()
    Dim c As New Counter, l As New Listener, r As IReport, i As Long
    l.Listen c
    For i = 1 To 6
        c.Add
    Next
    Set r = l
    Range("A1").Value = r.Text
End Sub
"#;
    let mut e = engine(&[
        ("Counter", ModuleKind::Class, counter),
        ("Listener", ModuleKind::Class, listener),
        ("IReport", ModuleKind::Class, report),
        ("Module1", ModuleKind::Standard, module),
    ]);
    let (book, out) = e.run(
        Workbook::new_empty().unwrap(),
        "Go",
        View::default(),
        Box::new(ui()),
        &opts(),
    );
    ok(&out);
    assert_eq!(cell(&book, 0, "A1"), "2 4 6");
}

#[test]
fn application_run_with_arguments() {
    let module = r#"
Function Add2(a, b)
    Add2 = a + b
End Function
Sub Note(text As String)
    Range("C1").Value = text
End Sub
Sub Go()
    Range("A1").Value = Application.Run("Add2", 3, 4)
    Range("A2").Value = Application.Run("'Book1.xlsm'!Module1.Add2", 1, 1)
    Run "Note", "hello"
End Sub
"#;
    let mut e = engine(&[("Module1", ModuleKind::Standard, module)]);
    let (book, out) = e.run(
        Workbook::new_empty().unwrap(),
        "Go",
        View::default(),
        Box::new(ui()),
        &opts(),
    );
    ok(&out);
    assert_eq!(cell(&book, 0, "A1"), "7");
    assert_eq!(cell(&book, 0, "A2"), "2");
    assert_eq!(cell(&book, 0, "C1"), "hello");
}

#[test]
fn on_time_schedules_on_a_fake_clock() {
    let module = r#"
Function At() As Date
    At = DateSerial(2030, 1, 2) + TimeSerial(9, 0, 0)
End Function
Sub Plan()
    Application.OnTime At, "Tick"
    Application.OnTime At + TimeSerial(0, 0, 30), "Module1.Tick"
    Application.OnTime At + TimeSerial(1, 0, 0), "Tick"
    Application.OnTime At + TimeSerial(1, 0, 0), "Tick", , False
End Sub
Sub Tick()
    Range("A1").Value = Range("A1").Value + 1
End Sub
Sub CancelMissing()
    Application.OnTime At + 5, "Tick", , False
End Sub
"#;
    let mut e = engine(&[("Module1", ModuleKind::Standard, module)]);
    let (book, out) = e.run(
        Workbook::new_empty().unwrap(),
        "Plan",
        View::default(),
        Box::new(ui()),
        &opts(),
    );
    ok(&out);
    assert_eq!(e.timers().len(), 2);
    // 2030-01-02 09:00 as an OLE date.
    let at = 47_485.0 + 9.0 / 24.0;
    assert_eq!(e.due(at - 1.0 / 1440.0), None);
    assert_eq!(e.due(at).as_deref(), Some("Tick"));
    let (book, out) = e.run(book, "Tick", View::default(), Box::new(ui()), &opts());
    ok(&out);
    assert_eq!(e.due(at + 10.0 / 86_400.0), None);
    let name = e.due(at + 31.0 / 86_400.0).unwrap();
    assert_eq!(name, "Module1.Tick");
    let (book, out) = e.run(book, &name, View::default(), Box::new(ui()), &opts());
    ok(&out);
    assert_eq!(cell(&book, 0, "A1"), "2");
    assert!(e.timers().is_empty());
    let (_, out) = e.run(
        book,
        "CancelMissing",
        View::default(),
        Box::new(ui()),
        &opts(),
    );
    assert_eq!(out.result.unwrap_err().number, 1004);
}

/// A folder of its own under the system's temporary folder.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let p =
            std::env::temp_dir().join(format!("kantor-vba-events-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn vba_path(p: &std::path::Path) -> String {
    p.display().to_string().replace('"', "\"\"")
}

#[test]
fn workbooks_open_add_save_as_and_close_round_trip() {
    let dir = TempDir::new("books");
    let data = dir.0.join("Data.xlsx");
    {
        let mut d = Workbook::new_empty().unwrap();
        set(&mut d, 0, "A1", "41");
        d.save_as(&data).unwrap();
    }
    let out_xlsx = dir.0.join("Summary.xlsx");
    let out_csv = dir.0.join("Summary.csv");
    let module = format!(
        r#"
Sub Go()
    Dim src As Workbook, dst As Workbook
    Set src = Workbooks.Open("{data}")
    ThisWorkbook.Sheets(1).Range("B1").Value = "active is " & ActiveWorkbook.Name
    ThisWorkbook.Sheets(1).Range("A1").Value = Workbooks("Data.xlsx").Sheets(1).Range("A1").Value + 1
    ThisWorkbook.Sheets(1).Range("A2").Value = Workbooks.Count
    Set dst = Workbooks.Add
    dst.Sheets(1).Range("A1").Value = "made by " & ThisWorkbook.Name
    dst.SaveAs Filename:="{xlsx}", FileFormat:=51
    dst.SaveAs "{csv_stem}", 6
    ThisWorkbook.Sheets(1).Range("A3").Value = dst.Name
    dst.Close SaveChanges:=False
    src.Close False
    ThisWorkbook.Sheets(1).Range("A4").Value = Workbooks.Count
End Sub
"#,
        data = vba_path(&data),
        xlsx = vba_path(&out_xlsx),
        csv_stem = vba_path(&dir.0.join("Summary")),
    );
    let mut e = engine(&[("Module1", ModuleKind::Standard, &module)]);
    e.set_paths(vec![dir.0.clone()], false);
    let (book, out) = e.run(
        Workbook::new_empty().unwrap(),
        "Go",
        View::default(),
        Box::new(ui()),
        &opts(),
    );
    ok(&out);
    assert_eq!(cell(&book, 0, "A1"), "42");
    assert_eq!(cell(&book, 0, "A2"), "2");
    assert_eq!(cell(&book, 0, "A3"), "Summary.csv");
    assert_eq!(cell(&book, 0, "A4"), "1");
    assert_eq!(cell(&book, 0, "B1"), "active is Data.xlsx");
    assert!(out.books.is_empty(), "{:?}", out.books);
    let saved = Workbook::open(&out_xlsx).unwrap();
    assert_eq!(cell(&saved, 0, "A1"), "made by Untitled.xlsx");
    let csv = std::fs::read_to_string(&out_csv).unwrap();
    assert!(csv.contains("made by"), "{csv}");
}

#[test]
fn opened_and_added_workbooks_come_back_to_be_shown() {
    let module = "Sub Go()\n    Workbooks.Add\n    ActiveSheet.Range(\"A1\").Value = 5\nEnd Sub\n";
    let mut e = engine(&[("Module1", ModuleKind::Standard, module)]);
    let (book, out) = e.run(
        Workbook::new_empty().unwrap(),
        "Go",
        View::default(),
        Box::new(ui()),
        &opts(),
    );
    ok(&out);
    // The new workbook was active: unqualified ranges went there.
    assert_eq!(cell(&book, 0, "A1"), "");
    match out.books.as_slice() {
        [BookOut::New { book }] => assert_eq!(cell(book, 0, "A1"), "5"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_path_guard_refuses_paths_not_allowed() {
    let dir = TempDir::new("guard");
    let other = TempDir::new("guard-other");
    let secret = other.0.join("Secret.xlsx");
    Workbook::new_empty().unwrap().save_as(&secret).unwrap();
    let module = format!(
        r#"
Sub OpenIt()
    Workbooks.Open "{secret}"
End Sub
Sub SaveIt()
    ThisWorkbook.SaveCopyAs "{copy}"
End Sub
"#,
        secret = vba_path(&secret),
        copy = vba_path(&other.0.join("Copy.xlsx")),
    );
    let mut e = engine(&[("Module1", ModuleKind::Standard, &module)]);
    e.set_paths(vec![dir.0.clone()], false);
    let (book, out) = e.run(
        Workbook::new_empty().unwrap(),
        "OpenIt",
        View::default(),
        Box::new(ui()),
        &opts(),
    );
    let err = out.result.unwrap_err();
    assert_eq!(err.number, 1004);
    assert!(
        err.description.contains("didn't let"),
        "{}",
        err.description
    );
    let (_, out) = e.run(book, "SaveIt", View::default(), Box::new(ui()), &opts());
    assert_eq!(out.result.unwrap_err().number, 1004);
    assert!(!other.0.join("Copy.xlsx").exists());
}

#[test]
fn udf_gets_a_read_only_range_and_its_caller() {
    let macros = modules(&[(
        "Module1",
        ModuleKind::Standard,
        r#"
Public Function SumBig(r As Range, limit As Double) As Double
    Dim c As Range
    For Each c In r
        If c.Value > limit Then SumBig = SumBig + c.Value
    Next
End Function

Public Function Addr(r As Range) As String
    Addr = r.Address
End Function

Public Function SizeOf(r As Range) As String
    SizeOf = r.Count & " " & r.Rows.Count & "x" & r.Columns.Count
End Function

Public Function PickSecond(r As Range) As String
    PickSecond = r.Cells(2, 1).Value & " " & r.Offset(1, 0).Resize(1, 1).Value
End Function

Public Function Who() As String
    Who = Application.Caller.Address(False, False)
End Function

Public Function Sneaky(r As Range) As String
    r.Value = 1
    Sneaky = "wrote"
End Function
"#,
    )]);
    let mut book = Workbook::new_empty().unwrap();
    for (a1, v) in [("A1", "4"), ("A2", "10"), ("A3", "12")] {
        set(&mut book, 0, a1, v);
    }
    set(&mut book, 0, "C1", "=SumBig(A1:A3, 5)");
    set(&mut book, 0, "C2", "=Addr(A1:A3)");
    set(&mut book, 0, "D2", "=SizeOf(A1:A3)");
    set(&mut book, 0, "E2", "=PickSecond(A1:A3)");
    set(&mut book, 0, "F2", "=Who()");
    set(&mut book, 0, "C3", "=Sneaky(A1)");
    sheets_script::enable_functions(&mut book, Some(&macros));
    book.recalculate();
    assert_eq!(cell(&book, 0, "C1"), "22");
    let got: Vec<String> = ["C2", "D2", "E2", "F2"]
        .iter()
        .map(|a| cell(&book, 0, a))
        .collect();
    assert_eq!(got, ["$A$1:$A$3", "3 3x1", "10 10", "F2"]);
    assert_eq!(cell(&book, 0, "C3"), "#VALUE!");
    set(&mut book, 0, "A1", "100");
    assert_eq!(cell(&book, 0, "C1"), "122");
}
