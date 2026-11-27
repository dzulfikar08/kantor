//! Tables (`ListObjects`) from macros: made, grown, totalled, resized,
//! filtered and sorted through the engine's table operations, saved and
//! undone.

mod support;

use sheets_engine::TotalFunc;
use support::*;

const BUILD: &str = r#"
Sub BuildTable()
    Dim ws As Worksheet, lo As ListObject
    Set ws = ActiveSheet
    Set lo = ws.ListObjects.Add(xlSrcRange, ws.Range("A1:C5"), , xlYes, , "TableStyleMedium9")
    lo.Name = "SalesTbl"
    lo.ListRows.Add.Range.Value = Array("Eko", "West", 120)
    Debug.Print lo.Name & " " & lo.ListRows.Count & " " & lo.ListColumns.Count
    Debug.Print WorksheetFunction.Sum(lo.ListColumns("Amount").DataBodyRange)
    lo.ShowTotals = True
    lo.ListColumns("Amount").TotalsCalculation = xlTotalsCalculationSum
    Debug.Print lo.TotalsRowRange.Address & " " & lo.ListColumns(3).Total.Value
    lo.Resize ws.Range("A1:D7")
    lo.ListColumns(4).Name = "Bonus"
    lo.ListColumns("Bonus").DataBodyRange.Formula = "=SalesTbl[@Amount]/10"
    Debug.Print lo.Range.Address & " " & lo.TableStyle & " " & lo.ShowTotals
End Sub
"#;

#[test]
fn a_table_is_made_grown_totalled_and_resized() {
    let (book, outcome, printed) = run_printed(sales(), BUILD, "BuildTable");
    ok(&outcome);
    assert_eq!(
        printed,
        vec![
            "SalesTbl 5 3",
            "820",
            "$A$7:$C$7 820",
            "$A$1:$D$7 TableStyleMedium9 True",
        ]
    );
    let t = &book.tables(0)[0];
    assert_eq!(t.name, "SalesTbl");
    assert_eq!(t.area.name(), "A1:D7");
    assert_eq!(t.columns, vec!["Name", "Region", "Amount", "Bonus"]);
    assert_eq!(t.totals_rows, 1);
    assert_eq!(cell(&book, 0, "A6"), "Eko");
    assert_eq!(cell(&book, 0, "C6"), "120");
    assert_eq!(cell(&book, 0, "A7"), "Total");
    assert_eq!(input(&book, 0, "C7"), "=SUBTOTAL(109,SalesTbl[Amount])");
    assert_eq!(cell(&book, 0, "D2"), "30");
}

const FILTER: &str = r#"
Sub FilterAndSort()
    Dim lo As ListObject
    BuildTable
    Set lo = ActiveSheet.ListObjects("SalesTbl")
    lo.Range.AutoFilter Field:=2, Criteria1:="East"
    Debug.Print lo.ListColumns("Amount").Total.Value & " " & lo.AutoFilter.FilterMode
End Sub

Sub ShowAndSort()
    Dim lo As ListObject
    Set lo = ActiveSheet.ListObjects(1)
    lo.AutoFilter.ShowAllData
    With lo.Sort
        .SortFields.Clear
        .SortFields.Add Key:=lo.ListColumns("Amount").Range, SortOn:=xlSortOnValues, Order:=xlDescending
        .Header = xlYes
        .Apply
    End With
    Debug.Print lo.ListColumns("Amount").Total.Value
End Sub
"#;

#[test]
fn the_table_filter_and_sort_work_on_its_data() {
    let code = [BUILD, FILTER].concat();
    let (book, outcome, printed) = run_printed(sales(), &code, "FilterAndSort");
    ok(&outcome);
    assert_eq!(printed.last().map(String::as_str), Some("500 True"));
    // Budi, Citra and Eko (West, North, West) are hidden.
    let hidden: Vec<bool> = (1..=5).map(|r| book.row_hidden(0, r)).collect();
    assert_eq!(hidden, vec![false, true, false, true, true]);

    let (book, outcome, printed) = run_printed(book, &code, "ShowAndSort");
    ok(&outcome);
    assert_eq!(printed, vec!["820"]);
    assert!((1..=6).all(|r| !book.row_hidden(0, r)));
    let names: Vec<String> = (2..=6).map(|r| cell(&book, 0, &format!("A{r}"))).collect();
    assert_eq!(names, vec!["Dewi", "Andi", "Budi", "Eko", "Citra"]);
    // The calculated column moved with its rows.
    assert_eq!(cell(&book, 0, "D3"), "20");
    assert_eq!(cell(&book, 0, "D6"), "5");
}

#[test]
fn the_table_survives_save_and_reopen() {
    let (book, outcome) = run(sales(), BUILD, "BuildTable");
    ok(&outcome);
    let (back, _dir) = reopen(book, "table.xlsx");
    let t = &back.tables(0)[0];
    assert_eq!(t.name, "SalesTbl");
    assert_eq!(t.area.name(), "A1:D7");
    assert_eq!(t.style.as_deref(), Some("TableStyleMedium9"));
    assert_eq!(t.totals_rows, 1);
    assert_eq!(back.table_totals("SalesTbl")[2], TotalFunc::Sum);
    assert_eq!(cell(&back, 0, "C7"), "820");
    assert_eq!(cell(&back, 0, "D6"), "12");

    // The reopened table works from a macro too.
    let (_, outcome, printed) = run_printed(
        back,
        r#"
Sub Again()
    With ActiveSheet.ListObjects("salestbl")
        Debug.Print .Name & " " & .ListRows.Count & " " & .DataBodyRange.Address
    End With
End Sub
"#,
        "Again",
    );
    ok(&outcome);
    assert_eq!(printed, vec!["SalesTbl 5 $A$2:$D$6"]);
}

#[test]
fn one_undo_takes_the_whole_run_back() {
    let (mut book, outcome) = run(sales(), BUILD, "BuildTable");
    ok(&outcome);
    assert_eq!(book.tables(0).len(), 1);
    assert!(book.undo().unwrap());
    assert!(book.tables(0).is_empty());
    assert_eq!(cell(&book, 0, "A6"), "");
    assert_eq!(cell(&book, 0, "A7"), "");
    assert_eq!(cell(&book, 0, "D1"), "");
    assert_eq!(cell(&book, 0, "C5"), "50");
}

#[test]
fn rows_and_columns_are_added_deleted_and_listed() {
    let code = r#"
Sub EditTable()
    Dim lo As ListObject, r As ListRow, c As ListColumn, n As String
    Set lo = ActiveSheet.ListObjects.Add(xlSrcRange, Range("A1:C5"), , xlYes)
    Debug.Print lo.Name & " " & ActiveSheet.ListObjects.Count & " " & ActiveSheet.ListObjects(1).Name
    Set r = lo.ListRows.Add(1)
    r.Range.Value = Array("Fajar", "South", 75)
    Debug.Print lo.ListRows.Count & " " & lo.ListRows(1).Range.Cells(1, 1).Value & " " & r.Index
    lo.ListRows(3).Delete
    Set c = lo.ListColumns.Add
    c.Name = "Note"
    Debug.Print lo.ListColumns.Count & " " & c.Index & " " & lo.ListColumns(4).Name & " " & lo.HeaderRowRange.Address
    lo.ListColumns("Region").Delete
    Debug.Print lo.Range.Address & " " & lo.ListColumns(2).Name
    For Each c In lo.ListColumns
        n = n & c.Name & ","
    Next
    Debug.Print n
    Debug.Print TypeName(Range("B3").ListObject) & " " & (Range("H1").ListObject Is Nothing)
    lo.ShowTableStyleRowStripes = False
    lo.ShowTableStyleFirstColumn = True
    lo.TableStyle = "tablestylelight9"
    Debug.Print lo.TableStyle & " " & lo.ShowTableStyleRowStripes & " " & lo.ShowTableStyleFirstColumn
End Sub

Sub Unlist()
    ActiveSheet.ListObjects(1).Unlist
    Debug.Print ActiveSheet.ListObjects.Count
End Sub
"#;
    let (book, outcome, printed) = run_printed(sales(), code, "EditTable");
    ok(&outcome);
    assert_eq!(
        printed,
        vec![
            "Table1 1 Table1",
            "5 Fajar 1",
            "4 4 Note $A$1:$D$1",
            "$A$1:$C$5 Amount",
            "Name,Amount,Note,",
            "ListObject True",
            "TableStyleLight9 False True",
        ]
    );
    let t = &book.tables(0)[0];
    assert_eq!(t.columns, vec!["Name", "Amount", "Note"]);
    assert!(!t.row_stripes && t.first_column);
    let names: Vec<String> = (2..=5).map(|r| cell(&book, 0, &format!("A{r}"))).collect();
    assert_eq!(names, vec!["Fajar", "Dewi", "Andi", "Citra"]);
    assert_eq!(cell(&book, 0, "B2"), "75");

    let (book, outcome, printed) = run_printed(book, code, "Unlist");
    ok(&outcome);
    assert_eq!(printed, vec!["0"]);
    assert!(book.tables(0).is_empty());
    assert_eq!(cell(&book, 0, "A2"), "Fajar");
}

#[test]
fn structured_references_name_table_ranges() {
    let code = r#"
Sub Refs()
    ActiveSheet.ListObjects.Add(xlSrcRange, Range("A1:C5"), , xlYes).Name = "SalesTbl"
    Debug.Print Range("SalesTbl[Amount]").Address
    Debug.Print Range("SalesTbl[[#All],[Amount]]").Address
    Debug.Print Range("SalesTbl[#Headers]").Address
    Debug.Print Range("SalesTbl[[Region]:[Amount]]").Address
    Debug.Print Range("SalesTbl").Address
    Dim lo As ListObject
    Set lo = ActiveSheet.ListObjects("SalesTbl")
    lo.Sort.SortFields.Clear
    lo.Sort.SortFields.Add2 Key:=Range("SalesTbl[[#All],[Name]]"), SortOn:=xlSortOnValues, Order:=xlAscending
    lo.Sort.Header = xlYes
    lo.Sort.Apply
End Sub
"#;
    let (book, outcome, printed) = run_printed(sales(), code, "Refs");
    ok(&outcome);
    assert_eq!(
        printed,
        vec![
            "$C$2:$C$5",
            "$C$1:$C$5",
            "$A$1:$C$1",
            "$B$2:$C$5",
            "$A$2:$C$5"
        ]
    );
    let names: Vec<String> = (2..=5).map(|r| cell(&book, 0, &format!("A{r}"))).collect();
    assert_eq!(names, vec!["Andi", "Budi", "Citra", "Dewi"]);
}

#[test]
fn errors_are_excels() {
    let code = r#"
Sub Missing()
    Debug.Print ActiveSheet.ListObjects("Nope").Name
End Sub

Sub Overlap()
    ActiveSheet.ListObjects.Add xlSrcRange, Range("A1:C5"), , xlYes
    ActiveSheet.ListObjects.Add xlSrcRange, Range("B2:D6"), , xlYes
End Sub

Sub NoColumn()
    ActiveSheet.ListObjects.Add xlSrcRange, Range("A1:C5"), , xlYes
    Debug.Print ActiveSheet.ListObjects(1).ListColumns(9).Name
End Sub

Sub External()
    ActiveSheet.ListObjects.Add xlSrcExternal, "DSN=x", , xlYes
End Sub

Sub Unknown()
    ActiveSheet.ListObjects.Add xlSrcRange, Range("A1:C5"), , xlYes
    ActiveSheet.ListObjects(1).Frobnicate
End Sub
"#;
    for (name, number) in [
        ("Missing", 9),
        ("Overlap", 1004),
        ("NoColumn", 9),
        ("External", 445),
        ("Unknown", 438),
    ] {
        let (_, outcome) = run(sales(), code, name);
        assert_eq!(err(&outcome).number, number, "{name}");
    }
}
