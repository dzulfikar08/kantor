//! Pivot tables from macros: made with `PivotCaches.Create` /
//! `CreatePivotTable`, arranged through `PivotFields`, filtered, refreshed,
//! saved and undone.

mod support;

use sheets_engine::AxisEntry;
use support::*;

/// What the macro recorder writes for Insert PivotTable on a new sheet,
/// then Region to Rows and Amount to Values.
const MAKE: &str = r#"
Sub MakePivot()
    Dim ws As Worksheet, pc As PivotCache, pt As PivotTable
    Set ws = Worksheets.Add(After:=Worksheets(Worksheets.Count))
    ws.Name = "Report"
    Set pc = ThisWorkbook.PivotCaches.Create(SourceType:=xlDatabase, SourceData:= _
        "Sheet1!R1C1:R5C3", Version:=6)
    Set pt = pc.CreatePivotTable(TableDestination:="Report!R3C1", _
        TableName:="SalesPivot", DefaultVersion:=6)
    With pt
        .ColumnGrand = True
        .RowGrand = True
        .HasAutoFormat = True
        .DisplayErrorString = False
        .InGridDropZones = False
        .RowAxisLayout xlCompactRow
    End With
    With pt.PivotCache
        .RefreshOnFileOpen = False
        .MissingItemsLimit = xlMissingItemsDefault
    End With
    With pt.PivotFields("Region")
        .Orientation = xlRowField
        .Position = 1
    End With
    pt.AddDataField pt.PivotFields("Amount"), "Sum of Amount", xlSum
End Sub

Function Pivot() As PivotTable
    Set Pivot = Worksheets("Report").PivotTables("SalesPivot")
End Function
"#;

fn code(rest: &str) -> String {
    format!("{MAKE}\n{rest}")
}

/// The pivot table's body on `Report` (sheet 1) as `[label, value]` rows.
fn body(book: &sheets_engine::Workbook, from: u32, to: u32) -> Vec<(String, String)> {
    (from..=to)
        .map(|r| {
            (
                cell(book, 1, &format!("A{r}")),
                cell(book, 1, &format!("B{r}")),
            )
        })
        .collect()
}

fn rows(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

#[test]
fn recorded_insert_pivot_table_builds_it() {
    let (book, outcome, printed) = run_printed(
        sales(),
        &code(
            r#"
Sub Go()
    MakePivot
    Dim pt As PivotTable
    Set pt = Pivot()
    Debug.Print pt.Name
    Debug.Print pt.TableRange1.Address
    Debug.Print pt.DataBodyRange.Address
    Debug.Print pt.RowRange.Address
    Debug.Print pt.PivotCache.SourceData
    Debug.Print Worksheets("Report").PivotTables.Count
    Debug.Print pt.DataFields(1).Name & " " & pt.DataFields(1).Function
    Debug.Print pt.PivotFields("Region").Orientation & " " & pt.PivotFields("Region").Position
End Sub
"#,
        ),
        "Go",
    );
    ok(&outcome);
    assert_eq!(book.sheet_names(), ["Sheet1", "Report"]);
    assert_eq!(
        body(&book, 3, 7),
        rows(&[
            ("Row Labels", "Sum of Amount"),
            ("East", "500"),
            ("North", "50"),
            ("West", "150"),
            ("Grand Total", "700"),
        ])
    );
    assert_eq!(
        printed,
        [
            "SalesPivot",
            "$A$3:$B$7",
            "$B$4:$B$7",
            "$A$3:$A$7",
            "Sheet1!R1C1:R5C3",
            "1",
            "Sum of Amount -4157",
            "1 1",
        ]
    );
    let p = &book.pivot_tables(1)[0];
    assert_eq!(p.name, "SalesPivot");
    let l = book.pivot_layout(1, 2, 0).unwrap();
    assert_eq!(l.source, "Sheet1!$A$1:$C$5");
    assert_eq!(l.rows, [AxisEntry::Field(1)]);
    assert_eq!(l.values.len(), 1);
}

#[test]
fn refresh_table_and_refresh_all_pick_up_new_source_values() {
    let (mut book, outcome) = run(sales(), MAKE, "MakePivot");
    ok(&outcome);
    // Budi (West) sells more.
    set(&mut book, 0, "C3", "1150");
    // Not refreshed yet, as in Excel.
    assert_eq!(cell(&book, 1, "B6"), "150");
    let (mut book, outcome) = run(
        book,
        &code("Sub R()\n    Pivot().RefreshTable\nEnd Sub\n"),
        "R",
    );
    ok(&outcome);
    assert_eq!(cell(&book, 1, "B6"), "1150");
    assert_eq!(cell(&book, 1, "B7"), "1700");
    // Andi (East) too; RefreshAll and PivotCache.Refresh.
    set(&mut book, 0, "C4", "1000");
    let (mut book, outcome) = run(
        book,
        &code("Sub R()\n    ThisWorkbook.RefreshAll\nEnd Sub\n"),
        "R",
    );
    ok(&outcome);
    assert_eq!(cell(&book, 1, "B4"), "1300");
    assert_eq!(cell(&book, 1, "B7"), "2500");
    set(&mut book, 0, "C5", "100");
    let (book, outcome) = run(
        book,
        &code("Sub R()\n    Pivot().PivotCache.Refresh\nEnd Sub\n"),
        "R",
    );
    ok(&outcome);
    assert_eq!(cell(&book, 1, "B5"), "100");
    assert_eq!(cell(&book, 1, "B7"), "2550");
}

#[test]
fn page_field_current_page() {
    let (book, outcome, printed) = run_printed(
        sales(),
        &code(
            r#"
Sub Go()
    MakePivot
    With Pivot()
        .PivotFields("Region").Orientation = xlPageField
        .PivotFields("Name").Orientation = xlRowField
        .PivotFields("Region").CurrentPage = "East"
        Debug.Print .PivotFields("Region").CurrentPage
        Debug.Print .PageFields.Count & " " & .RowFields(1).Name
    End With
End Sub
"#,
        ),
        "Go",
    );
    ok(&outcome);
    assert_eq!(printed, ["East", "1 Name"]);
    assert_eq!(cell(&book, 1, "A1"), "Region");
    assert_eq!(cell(&book, 1, "B1"), "East");
    assert_eq!(
        body(&book, 3, 6),
        rows(&[
            ("Row Labels", "Sum of Amount"),
            ("Andi", "200"),
            ("Dewi", "300"),
            ("Grand Total", "500"),
        ])
    );
    // "(All)" clears it.
    let (book, outcome, printed) = run_printed(
        book,
        &code(
            r#"
Sub All()
    Pivot().PivotFields("Region").CurrentPage = "(All)"
    Debug.Print Pivot().PivotFields("Region").CurrentPage
End Sub
"#,
        ),
        "All",
    );
    ok(&outcome);
    assert_eq!(printed, ["(All)"]);
    assert_eq!(cell(&book, 1, "B1"), "(All)");
    assert_eq!(cell(&book, 1, "A8"), "Grand Total");
    assert_eq!(cell(&book, 1, "B8"), "700");
}

#[test]
fn hiding_items_and_clearing_filters() {
    let (book, outcome, printed) = run_printed(
        sales(),
        &code(
            r#"
Sub Go()
    MakePivot
    Pivot().PivotFields("Region").PivotItems("West").Visible = False
    Dim it As PivotItem
    For Each it In Pivot().PivotFields("Region").PivotItems
        Debug.Print it.Name & " " & it.Visible
    Next
End Sub
"#,
        ),
        "Go",
    );
    ok(&outcome);
    assert_eq!(printed, ["East True", "North True", "West False"]);
    assert_eq!(
        body(&book, 4, 6),
        rows(&[("East", "500"), ("North", "50"), ("Grand Total", "550")])
    );
    let (book, outcome) = run(
        book,
        &code("Sub C()\n    Pivot().ClearAllFilters\nEnd Sub\n"),
        "C",
    );
    ok(&outcome);
    assert_eq!(
        body(&book, 6, 7),
        rows(&[("West", "150"), ("Grand Total", "700")])
    );
    // Hiding every item fails, as in Excel.
    let (_, outcome) = run(
        book,
        &code(
            r#"
Sub H()
    With Pivot().PivotFields("Region")
        .PivotItems("East").Visible = False
        .PivotItems("North").Visible = False
        .PivotItems("West").Visible = False
    End With
End Sub
"#,
        ),
        "H",
    );
    assert_eq!(err(&outcome).number, 1004);
}

#[test]
fn for_each_over_fields() {
    let (_, outcome, printed) = run_printed(
        sales(),
        &code(
            r#"
Sub Go()
    MakePivot
    Dim pf As PivotField
    For Each pf In Pivot().PivotFields
        Debug.Print pf.Name & " " & pf.Orientation
    Next
    For Each pf In Pivot().RowFields
        Debug.Print "row: " & pf.Name
    Next
    For Each pf In Pivot().DataFields
        Debug.Print "data: " & pf.Name & " of " & pf.SourceName
    Next
    Debug.Print Pivot().PivotFields.Count & " " & Pivot().ColumnFields.Count
End Sub
"#,
        ),
        "Go",
    );
    ok(&outcome);
    assert_eq!(
        printed,
        [
            "Name 0",
            "Region 1",
            "Amount 4",
            "row: Region",
            "data: Sum of Amount of Amount",
            "3 0",
        ]
    );
}

#[test]
fn value_field_function_and_caption() {
    let (book, outcome, printed) = run_printed(
        sales(),
        &code(
            r#"
Sub Go()
    MakePivot
    With Pivot().PivotFields("Sum of Amount")
        .Function = xlAverage
        Debug.Print .Name
        .Caption = "Mean sale"
        Debug.Print .Name
    End With
    Debug.Print Pivot().DataFields(1).Function
End Sub
"#,
        ),
        "Go",
    );
    ok(&outcome);
    assert_eq!(printed, ["Average of Amount", "Mean sale", "-4106"]);
    assert_eq!(cell(&book, 1, "B3"), "Mean sale");
    assert_eq!(cell(&book, 1, "B4"), "250");
}

#[test]
fn range_sources_add_fields_and_orientations() {
    let (book, outcome, printed) = run_printed(
        sales(),
        r#"
Sub Go()
    Dim ws As Worksheet, pc As PivotCache, pt As PivotTable
    Set ws = Worksheets.Add(After:=Worksheets(1))
    ws.Name = "Report"
    Set pc = ActiveWorkbook.PivotCaches.Create(xlDatabase, Worksheets("Sheet1").Range("A1:C5"))
    Set pt = Worksheets("Report").PivotTables.Add(pc, Worksheets("Report").Range("A3"), "PT2")
    pt.AddFields RowFields:="Region"
    pt.PivotFields("Amount").Orientation = xlDataField
    Debug.Print pt.DataFields(1).Name
    pt.PivotFields("Region").Orientation = xlColumnField
    Debug.Print pt.ColumnFields(1).Name & " " & pt.RowFields.Count
    pt.PivotFields("Region").Orientation = xlHidden
    Debug.Print pt.ColumnFields.Count
    Debug.Print ThisWorkbook.PivotCaches.Count
End Sub
"#,
        "Go",
    );
    ok(&outcome);
    assert_eq!(printed, ["Sum of Amount", "Region 0", "0", "1"]);
    let l = book.pivot_layout(1, 2, 0).unwrap();
    assert!(l.rows.is_empty() && l.columns.is_empty());
    assert_eq!(l.values.len(), 1);
    assert_eq!(book.pivot_tables(1)[0].name, "PT2");
}

#[test]
fn save_and_reopen_keeps_the_pivot_and_its_filter() {
    let (book, outcome) = run(
        sales(),
        &code(
            r#"
Sub Go()
    MakePivot
    Pivot().PivotFields("Region").PivotItems("West").Visible = False
End Sub
"#,
        ),
        "Go",
    );
    ok(&outcome);
    let (back, _dir) = reopen(book, "pivot.xlsx");
    let p = &back.pivot_tables(1)[0];
    assert_eq!(p.name, "SalesPivot");
    assert_eq!(p.area.name(), "A3:B6");
    let l = back.pivot_layout(1, 2, 0).unwrap();
    assert_eq!(l.rows, [AxisEntry::Field(1)]);
    assert_eq!(l.values.len(), 1);
    let west = l.fields[1]
        .items
        .iter()
        .find(|i| i.label == "West")
        .unwrap();
    assert!(west.hidden);
    assert_eq!(
        body(&back, 4, 6),
        rows(&[("East", "500"), ("North", "50"), ("Grand Total", "550")])
    );
    // And a macro still finds and refreshes it.
    let (back, outcome) = run(
        back,
        &code("Sub R()\n    Pivot().PivotFields(\"Region\").ClearAllFilters\nEnd Sub\n"),
        "R",
    );
    ok(&outcome);
    assert_eq!(cell(&back, 1, "B7"), "700");
}

#[test]
fn undo_takes_a_created_pivot_away_in_one_step() {
    let (mut book, outcome) = run(
        sales(),
        r#"
Sub Go()
    Dim pt As PivotTable
    Set pt = ThisWorkbook.PivotCaches.Create(xlDatabase, "Sheet1!A1:C5").CreatePivotTable("Sheet1!R1C6", "Quick")
    pt.PivotFields("Region").Orientation = xlRowField
    pt.AddDataField pt.PivotFields("Amount"), "Total", xlSum
    pt.Name = "Renamed"
End Sub
"#,
        "Go",
    );
    ok(&outcome);
    assert_eq!(book.pivot_tables(0).len(), 1);
    assert_eq!(book.pivot_tables(0)[0].name, "Renamed");
    assert_eq!(cell(&book, 0, "G1"), "Total");
    assert_eq!(cell(&book, 0, "G5"), "700");
    assert!(book.undo().unwrap());
    assert!(book.pivot_tables(0).is_empty());
    assert_eq!(cell(&book, 0, "F1"), "");
    assert_eq!(cell(&book, 0, "G5"), "");
    assert_eq!(cell(&book, 0, "C5"), "50");
}

#[test]
fn errors_like_excel() {
    let (book, outcome) = run(
        sales(),
        &code(
            "Sub E()\n    MakePivot\n    Debug.Print Pivot().PivotFields(\"Nope\").Name\nEnd Sub\n",
        ),
        "E",
    );
    assert_eq!(err(&outcome).number, 1004);
    let (book, outcome) = run(
        book,
        "Sub E()\n    Debug.Print Worksheets(1).PivotTables(\"Nope\").Name\nEnd Sub\n",
        "E",
    );
    assert_eq!(err(&outcome).number, 1004);
    let (book, outcome) = run(
        book,
        &code(
            "Sub E()\n    Pivot().PivotFields(\"Region\").PivotItems(\"South\").Visible = False\nEnd Sub\n",
        ),
        "E",
    );
    assert_eq!(err(&outcome).number, 1004);
    let (book, outcome) = run(
        book,
        &code("Sub E()\n    Pivot().PivotFields(\"Region\").CurrentPage = \"East\"\nEnd Sub\n"),
        "E",
    );
    assert_eq!(err(&outcome).number, 1004, "Region is a row field");
    let (_, outcome) = run(
        book,
        "Sub E()\n    ActiveSheet.PivotTableWizard\nEnd Sub\n",
        "E",
    );
    assert_eq!(err(&outcome).number, 445);
}
