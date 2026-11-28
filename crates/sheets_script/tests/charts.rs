//! Charts from macros: `ChartObjects`, `Chart` (source data, type, title,
//! legend, axes), `SeriesCollection` / `Series`, `Workbook.Charts`,
//! `Shapes.AddChart2` with `ActiveChart`, `Chart.Export`; what a macro made
//! saved and read back, and undone in one step.

mod support;

use sheets_engine::drawing::{Drawing, DrawingKind};
use sheets_engine::{ChartType, LegendPos, Tab, Workbook};
use support::*;

/// Month / Sales / Costs on A1:C5 of Sheet1.
fn table() -> Workbook {
    let mut b = Workbook::new_empty().unwrap();
    let rows = [
        ["Month", "Sales", "Costs"],
        ["Jan", "120", "80"],
        ["Feb", "135", "90"],
        ["Mar", "150", "95"],
        ["Apr", "110", "70"],
    ];
    for (i, row) in rows.iter().enumerate() {
        for (j, v) in row.iter().enumerate() {
            b.set_input(0, i as u32, j as u32, v).unwrap();
        }
    }
    b
}

fn charts(book: &Workbook, sheet: u32) -> Vec<Drawing> {
    book.drawings(sheet)
        .into_iter()
        .filter(|d| matches!(*d.kind, DrawingKind::Chart { .. }))
        .collect()
}

fn xml_of(d: &Drawing) -> String {
    match &*d.kind {
        DrawingKind::Chart { xml, .. } => xml.to_string(),
        _ => unreachable!("a chart"),
    }
}

/// Runs `name` from `code`; `Debug.Print` lines trimmed (VBA pads numbers
/// with spaces).
fn run_lines(book: Workbook, code: &str, name: &str) -> (Workbook, Vec<String>) {
    let (book, outcome, printed) = run_printed(book, code, name);
    ok(&outcome);
    (book, printed.iter().map(|l| l.trim().to_owned()).collect())
}

#[test]
fn chart_object_from_a_table_saves_and_reads_back() {
    let code = r#"
Sub MakeChart()
    Dim co As ChartObject
    Set co = ActiveSheet.ChartObjects.Add(Left:=200, Top:=20, Width:=360, Height:=216)
    Debug.Print ActiveSheet.ChartObjects.Count
    With co.Chart
        .SetSourceData Source:=Range("A1:C5")
        .ChartType = xlLine
        .HasTitle = True
        .ChartTitle.Text = "Sales"
        .HasLegend = True
        .Legend.Position = xlLegendPositionBottom
        Debug.Print .ChartType & "|" & .SeriesCollection.Count & "|" & .SeriesCollection(1).Name
        Debug.Print .SeriesCollection(2).Formula
        Debug.Print .HasTitle & "|" & .ChartTitle.Caption & "|" & .Legend.Position
    End With
    Debug.Print co.Name & "|" & Round(co.Width) & "|" & Round(co.Left)
End Sub
"#;
    let (book, printed) = run_lines(table(), code, "MakeChart");
    assert_eq!(
        printed,
        vec![
            "1",
            "4|2|Sales",
            "=SERIES(Sheet1!$C$1,Sheet1!$A$2:$A$5,Sheet1!$C$2:$C$5,2)",
            "True|Sales|-4107",
            "Chart 1|360|200",
        ]
    );
    let found = charts(&book, 0);
    assert_eq!(found.len(), 1);
    let info = book.chart_info(found[0].key).unwrap();
    assert_eq!(info.spec.kind, ChartType::Line);
    assert_eq!(info.spec.title, "Sales");
    assert_eq!(info.spec.legend, LegendPos::Bottom);
    assert_eq!(info.series, vec!["Sales", "Costs"]);

    let (book, _dir) = reopen(book, "chart.xlsx");
    let found = charts(&book, 0);
    assert_eq!(found.len(), 1, "the chart is in the saved file");
    let xml = xml_of(&found[0]);
    assert!(xml.contains("lineChart"), "{xml}");
    assert!(xml.contains("Sheet1!$B$2:$B$5"), "{xml}");
    assert!(xml.contains("Sheet1!$A$2:$A$5"), "{xml}");
    let info = book.chart_info(found[0].key).unwrap();
    assert_eq!(info.spec.kind, ChartType::Line);
    assert_eq!(info.spec.title, "Sales");
    assert_eq!(info.spec.legend, LegendPos::Bottom);
    assert_eq!(info.series, vec!["Sales", "Costs"]);
}

#[test]
fn series_added_one_by_one() {
    let code = r#"
Sub Build()
    Dim ch As Chart, s As Series
    Set ch = Worksheets("Sheet1").ChartObjects.Add(10, 100, 300, 200).Chart
    Set s = ch.SeriesCollection.NewSeries
    s.Name = "=Sheet1!$B$1"
    s.Values = Range("B2:B5")
    s.XValues = Range("A2:A5")
    With ch.SeriesCollection.NewSeries
        .Name = "Plan"
        .Values = Array(100, 110, 120, 130)
        .XValues = "=Sheet1!$A$2:$A$5"
    End With
    Debug.Print ch.SeriesCollection.Count
    Debug.Print ch.SeriesCollection(1).Name
    Debug.Print ch.SeriesCollection("Plan").Formula
    Dim v
    v = ch.SeriesCollection(1).Values
    Debug.Print UBound(v) & " " & v(1) & " " & v(4)
    v = ch.SeriesCollection(2).XValues
    Debug.Print v(1) & " " & v(4)
    Dim x As Series, names As String
    For Each x In ch.SeriesCollection
        names = names & x.Name & ";"
    Next
    Debug.Print names
    ch.SeriesCollection(1).Delete
    Debug.Print ch.SeriesCollection.Count & " " & ch.SeriesCollection(1).Name
    ch.SeriesCollection(1).Formula = "=SERIES(Sheet1!$C$1,Sheet1!$A$2:$A$5,Sheet1!$C$2:$C$5,1)"
    Debug.Print ch.SeriesCollection(1).Name
End Sub
"#;
    let (book, printed) = run_lines(table(), code, "Build");
    assert_eq!(
        printed,
        vec![
            "2",
            "Sales",
            "=SERIES(\"Plan\",Sheet1!$A$2:$A$5,{100,110,120,130},2)",
            "4 120 110",
            "Jan Apr",
            "Sales;Plan;",
            "1 Plan",
            "Costs",
        ]
    );
    let found = charts(&book, 0);
    let xml = xml_of(&found[0]);
    assert!(xml.contains("Sheet1!$C$2:$C$5"), "{xml}");
    assert!(
        !xml.contains("<c:numLit>"),
        "the literal series was replaced: {xml}"
    );

    let (book, _dir) = reopen(book, "series.xlsx");
    let found = charts(&book, 0);
    assert_eq!(found.len(), 1);
    let info = book.chart_info(found[0].key).unwrap();
    assert_eq!(info.series, vec!["Costs"]);
    assert!(xml_of(&found[0]).contains("Sheet1!$C$1"));
}

#[test]
fn axes_and_collection_loops() {
    let code = r#"
Sub AxesDemo()
    Dim co As ChartObject
    Set co = ActiveSheet.ChartObjects.Add(250, 10, 360, 216)
    co.Chart.SetSourceData Range("A1:C5"), xlColumns
    With co.Chart.Axes(xlValue)
        .MinimumScale = 50
        .MaximumScale = 250
        .MajorUnit = 50
        .HasTitle = True
        .AxisTitle.Text = "Amount"
        Debug.Print .MinimumScaleIsAuto & " " & .MaximumScale & " " & .MajorUnit
    End With
    co.Chart.Axes(xlCategory).HasTitle = True
    co.Chart.Axes(xlCategory).AxisTitle.Caption = "Month"
    Debug.Print co.Chart.Axes(xlCategory).AxisTitle.Text
    co.Chart.Axes(xlValue).MaximumScaleIsAuto = True
    Debug.Print co.Chart.Axes(xlValue).MaximumScaleIsAuto
    ActiveSheet.ChartObjects.Add(250, 250, 200, 150).Chart.SetSourceData Range("A1:B5")
    Dim c As ChartObject
    For Each c In ActiveSheet.ChartObjects
        Debug.Print c.Name & " " & c.Chart.SeriesCollection.Count
    Next
    ActiveSheet.ChartObjects(2).Delete
    Debug.Print ActiveSheet.ChartObjects.Count
End Sub
"#;
    let (book, printed) = run_lines(table(), code, "AxesDemo");
    assert_eq!(
        printed,
        vec![
            "False 250 50",
            "Month",
            "True",
            "Chart 1 2",
            "Chart 2 1",
            "1"
        ]
    );
    let found = charts(&book, 0);
    assert_eq!(found.len(), 1);
    let info = book.chart_info(found[0].key).unwrap();
    assert_eq!(info.spec.y_axis.min, Some(50.0));
    assert_eq!(info.spec.y_axis.max, None);
    assert_eq!(info.spec.y_axis.major, Some(50.0));
    assert_eq!(info.spec.y_title, "Amount");
    assert_eq!(info.spec.x_title, "Month");

    let (book, _dir) = reopen(book, "axes.xlsx");
    let info = book.chart_info(charts(&book, 0)[0].key).unwrap();
    assert_eq!(info.spec.y_axis.min, Some(50.0));
    assert_eq!(info.spec.y_title, "Amount");
}

#[test]
fn undo_takes_the_chart_back_in_one_step() {
    let code = r#"
Sub MakeIt()
    Range("B2").Value = 999
    With ActiveSheet.ChartObjects.Add(200, 20, 300, 200).Chart
        .SetSourceData Range("A1:C5")
        .ChartType = xlBarClustered
        .HasTitle = True
        .ChartTitle.Text = "Undo me"
        .SeriesCollection.NewSeries.Values = Array(1, 2, 3, 4)
    End With
End Sub
"#;
    let (mut book, outcome) = run(table(), code, "MakeIt");
    ok(&outcome);
    assert_eq!(charts(&book, 0).len(), 1);
    assert_eq!(cell(&book, 0, "B2"), "999");
    assert!(book.undo().unwrap());
    assert!(charts(&book, 0).is_empty(), "one undo takes the chart away");
    assert_eq!(cell(&book, 0, "B2"), "120");
    assert!(book.redo().unwrap());
    let found = charts(&book, 0);
    assert_eq!(found.len(), 1);
    let info = book.chart_info(found[0].key).unwrap();
    assert_eq!(info.spec.kind, ChartType::Bar);
    assert_eq!(info.series.len(), 3);
}

#[test]
fn recorded_add_chart2_and_active_chart() {
    let code = r#"
Sub Recorded()
    Range("A1:C5").Select
    ActiveSheet.Shapes.AddChart2 201, xlColumnClustered
    ActiveChart.SetSourceData Source:=Range("Sheet1!$A$1:$C$5")
    ActiveChart.ChartTitle.Text = "Recorded"
    Debug.Print ActiveChart.ChartType & " " & ActiveChart.SeriesCollection.Count
    Debug.Print ActiveChart.ChartTitle.Text
End Sub
"#;
    let (book, printed) = run_lines(table(), code, "Recorded");
    assert_eq!(printed, vec!["51 2", "Recorded"]);
    let found = charts(&book, 0);
    assert_eq!(found.len(), 1);
    let info = book.chart_info(found[0].key).unwrap();
    assert_eq!(info.spec.kind, ChartType::Column);
    assert_eq!(info.spec.title, "Recorded");
}

#[test]
fn chart_sheets() {
    let code = r#"
Sub ChartSheets()
    Range("A1:C5").Select
    Dim ch As Chart
    Set ch = ThisWorkbook.Charts.Add
    ch.ChartType = xlBarClustered
    ch.Name = "Sales Chart"
    Debug.Print ThisWorkbook.Charts.Count & "|" & ThisWorkbook.Charts(1).Name & "|" & ch.SeriesCollection.Count
    Dim c As Chart
    For Each c In ThisWorkbook.Charts
        Debug.Print c.Name & "|" & c.ChartType
    Next
    Debug.Print ThisWorkbook.Charts("sales chart").HasLegend
End Sub
"#;
    let (book, printed) = run_lines(table(), code, "ChartSheets");
    assert_eq!(printed, vec!["1|Sales Chart|2", "Sales Chart|57", "True"]);
    let (book, _dir) = reopen(book, "sheets.xlsx");
    let k = book
        .tabs()
        .into_iter()
        .find_map(|t| match t {
            Tab::Chart(k) => Some(k),
            Tab::Sheet(_) => None,
        })
        .expect("a chart sheet");
    assert_eq!(book.chart_sheet_name(k).as_deref(), Some("Sales Chart"));
    let d = book.chart_sheet_drawings(k);
    let chart = d
        .iter()
        .find(|d| matches!(*d.kind, DrawingKind::Chart { .. }))
        .unwrap();
    let xml = xml_of(chart);
    assert!(xml.contains("<c:barDir val=\"bar\"/>"), "{xml}");
    assert!(xml.contains("Sheet1!$B$2:$B$5"), "{xml}");
}

#[test]
fn export_writes_a_png_in_the_workbook_folder() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.xlsx");
    table().save_as(&path).unwrap();
    let book = Workbook::open(&path).unwrap();
    let code = r#"
Sub ExportIt()
    Dim co As ChartObject
    Set co = ActiveSheet.ChartObjects.Add(200, 20, 360, 216)
    co.Chart.SetSourceData Range("A1:C5")
    Debug.Print co.Chart.Export("chart.png")
    On Error Resume Next
    co.Chart.Export "chart.gif"
    Debug.Print Err.Number
End Sub
"#;
    let (_book, printed) = run_lines(book, code, "ExportIt");
    assert_eq!(printed, vec!["True", "445"]);
    let png = std::fs::read(dir.path().join("chart.png")).unwrap();
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    // 360 × 216 points at 96 dpi.
    let w = u32::from_be_bytes(png[16..20].try_into().unwrap());
    let h = u32::from_be_bytes(png[20..24].try_into().unwrap());
    assert!(
        (478..=482).contains(&w) && (286..=290).contains(&h),
        "{w}×{h}"
    );
    assert!(!dir.path().join("chart.gif").exists());
}

#[test]
fn errors_are_excels() {
    let code = r#"
Sub Errors()
    Dim co As ChartObject
    Set co = ActiveSheet.ChartObjects.Add(200, 20, 300, 200)
    co.Chart.SetSourceData Range("A1:C5")
    On Error Resume Next
    co.Chart.ChartType = xlDoughnut
    Debug.Print Err.Number: Err.Clear
    Debug.Print co.Chart.Bogus
    Debug.Print Err.Number: Err.Clear
    Set co = ActiveSheet.ChartObjects(5)
    Debug.Print Err.Number: Err.Clear
    co.Chart.SeriesCollection(9).Name = "x"
    Debug.Print Err.Number: Err.Clear
    co.Chart.ChartType = xlPie
    co.Chart.Axes(xlValue).MinimumScale = 1
    Debug.Print Err.Number: Err.Clear
    co.Chart.SetSourceData Range("E1:F3")
    Debug.Print Err.Number: Err.Clear
    co.Chart.Legend.Position = xlLegendPositionCorner
    Debug.Print Err.Number: Err.Clear
End Sub
"#;
    let (book, printed) = run_lines(table(), code, "Errors");
    assert_eq!(printed, vec!["445", "438", "9", "9", "1004", "1004", "445"]);
    let info = book.chart_info(charts(&book, 0)[0].key).unwrap();
    assert_eq!(info.spec.kind, ChartType::Pie);
}
