//! Conditional formats, fill patterns, `Characters` and border edges set
//! by macros: the engine's rules and styles after a run, what a saved file
//! keeps, and one undo step per run.

mod support;
use support::*;

use sheets_engine::{BorderLine, CellRange, CfCompare, CfSpec, CfTextOp, CfValueKind, LineStyle};

fn rules(book: &sheets_engine::Workbook) -> Vec<sheets_engine::CfRuleInfo> {
    book.cf_rules(0, None)
}

const HIGHLIGHT: &str = r#"
Sub Highlight()
    Dim r As Range
    Set r = Range("C2:C5")
    r.FormatConditions.Delete
    r.FormatConditions.Add Type:=xlCellValue, Operator:=xlGreater, Formula1:="=200"
    r.FormatConditions(r.FormatConditions.Count).SetFirstPriority
    With r.FormatConditions(1)
        .Font.Bold = True
        .Font.Color = RGB(156, 0, 6)
        .Interior.Color = RGB(255, 199, 206)
        .Borders(xlBottom).LineStyle = xlContinuous
        .Borders(xlBottom).Color = RGB(192, 0, 0)
        .NumberFormat = "0.0"
        .StopIfTrue = False
    End With
    With r.FormatConditions(1)
        Debug.Print r.FormatConditions.Count & "|" & .Type & "|" & .Operator & "|" & .Formula1 & "|" & TypeName(r.FormatConditions(1))
        Debug.Print .Interior.Color & "|" & .Font.Bold & "|" & .Priority
    End With
End Sub
"#;

#[test]
fn values_over_200_red_and_bold() {
    let (book, outcome, printed) = run_printed(sales(), HIGHLIGHT, "Highlight");
    ok(&outcome);
    assert_eq!(printed[0], "1|1|5|=200|FormatCondition");
    // RGB(255, 199, 206) as VBA's &HBBGGRR.
    assert_eq!(printed[1], format!("{}|True|1", 0xCEC7FF));
    let list = rules(&book);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].range, "C2:C5");
    let def = list[0].def.clone().unwrap();
    assert_eq!(
        def.spec,
        CfSpec::CellValue {
            op: CfCompare::GreaterThan,
            value: "200".into(),
            value2: String::new(),
        }
    );
    assert_eq!(def.format.fill, Some(0xFFC7CE));
    assert_eq!(def.format.font, Some(0x9C0006));
    assert!(def.format.bold);
    assert_eq!(def.num_fmt.as_deref(), Some("0.0"));
    let sides = book.cf_border_sides(def.border.as_ref().unwrap());
    assert_eq!(sides[0], None);
    assert_eq!(
        sides[3],
        Some(BorderLine {
            style: LineStyle::Thin,
            color: Some(0xC00000),
        })
    );

    // The file keeps the rule, its formula and its colours.
    let (book, _dir) = reopen(book, "highlight.xlsx");
    let list = rules(&book);
    assert_eq!(list.len(), 1);
    let def = list[0].def.clone().unwrap();
    assert!(matches!(
        def.spec,
        CfSpec::CellValue { op: CfCompare::GreaterThan, ref value, .. } if value == "200"
    ));
    assert_eq!(def.format.fill, Some(0xFFC7CE));
    assert_eq!(def.format.font, Some(0x9C0006));
    assert!(def.format.bold);
}

const RULES: &str = r#"
Sub Rules()
    With Range("A2:C5").FormatConditions.Add(Type:=xlExpression, Formula1:="=$C2>100")
        .Interior.Color = RGB(198, 239, 206)
    End With
    Dim cs As Object
    Set cs = Range("C2:C5").FormatConditions.AddColorScale(ColorScaleType:=3)
    cs.ColorScaleCriteria(1).Type = xlConditionValueLowestValue
    cs.ColorScaleCriteria(1).FormatColor.Color = RGB(248, 105, 107)
    cs.ColorScaleCriteria(2).Type = xlConditionValuePercentile
    cs.ColorScaleCriteria(2).Value = 40
    cs.ColorScaleCriteria(2).FormatColor.Color = RGB(255, 235, 132)
    cs.ColorScaleCriteria(3).Type = xlConditionValueHighestValue
    cs.ColorScaleCriteria(3).FormatColor.Color = RGB(99, 190, 123)
    Dim db As Object
    Set db = Range("C2:C5").FormatConditions.AddDatabar
    db.BarColor.Color = RGB(0, 138, 239)
    db.BarFillType = xlDataBarFillSolid
    db.MinPoint.Modify newtype:=xlConditionValueNumber, newvalue:=0
    db.MaxPoint.Modify newtype:=xlConditionValueHighestValue
    Dim fc As Object
    For Each fc In Range("A2:C5").FormatConditions
        Debug.Print fc.Type & " " & TypeName(fc)
    Next
    Debug.Print Range("C2:C5").FormatConditions.Count & " " & Range("A2:A5").FormatConditions.Count
    Debug.Print cs.ColorScaleCriteria(2).Type & " " & cs.ColorScaleCriteria(2).Value & " " & db.MinPoint.Type
End Sub

Sub Clear()
    Dim i As Long
    For i = Range("C2:C5").FormatConditions.Count To 1 Step -1
        If Range("C2:C5").FormatConditions(i).Type = xlDatabar Then Range("C2:C5").FormatConditions(i).Delete
    Next
    Debug.Print Range("C2:C5").FormatConditions.Count
    Range("A2:C5").FormatConditions.Delete
    Debug.Print Range("A2:C5").FormatConditions.Count
End Sub
"#;

#[test]
fn expression_rule_colour_scale_and_data_bar() {
    let (book, outcome, printed) = run_printed(sales(), RULES, "Rules");
    ok(&outcome);
    // Add puts each new rule last.
    assert_eq!(
        printed,
        [
            "2 FormatCondition",
            "3 ColorScale",
            "4 Databar",
            "3 1",
            "5 40 0"
        ]
    );
    let list = rules(&book);
    assert_eq!(list.len(), 3);
    let def = |i: usize| list[i].def.clone().unwrap();
    assert_eq!(list[0].range, "A2:C5");
    assert_eq!(
        def(0).spec,
        CfSpec::Formula {
            formula: "=$C2>100".into()
        }
    );
    assert_eq!(def(0).format.fill, Some(0xC6EFCE));
    assert_eq!(
        def(1).spec,
        CfSpec::ColorScale {
            colors: vec![0xF8696B, 0xFFEB84, 0x63BE7B]
        }
    );
    let points = book.cf_color_scale(0, list[1].index).unwrap();
    assert_eq!(points[0].0.kind, CfValueKind::Lowest);
    assert_eq!(points[1].0.kind, CfValueKind::Percentile);
    assert_eq!(points[1].0.value, "40");
    assert_eq!(points[2].0.kind, CfValueKind::Highest);
    match def(2).spec {
        CfSpec::DataBar {
            color,
            gradient,
            bar,
        } => {
            assert_eq!(color, 0x008AEF);
            assert!(!gradient);
            assert_eq!(bar.min.kind, CfValueKind::Number);
            assert_eq!(bar.min.value, "0");
            assert_eq!(bar.max.kind, CfValueKind::Highest);
        }
        other => panic!("not a data bar: {other:?}"),
    }

    // Saved and opened again: the three rules, the formula and the colours.
    let (reopened, _dir) = reopen(book, "rules.xlsx");
    let list = rules(&reopened);
    assert_eq!(list.len(), 3);
    assert_eq!(
        list[0].def.as_ref().unwrap().spec,
        CfSpec::Formula {
            formula: "=$C2>100".into()
        }
    );
    assert_eq!(list[0].format.fill, Some(0xC6EFCE));
    let points = reopened.cf_color_scale(0, list[1].index).unwrap();
    assert_eq!(points[1].0.value, "40");
    assert_eq!(points[2].1, 0x63BE7B);

    // Listing and deleting.
    let (book, outcome, printed) = run_printed(reopened, RULES, "Clear");
    ok(&outcome);
    assert_eq!(printed, ["2", "0"]);
    assert!(rules(&book).is_empty());
}

#[test]
fn top_average_unique_and_text_rules() {
    let code = r#"
Sub More()
    With Range("C2:C5").FormatConditions.AddTop10
        .TopBottom = xlTop10Bottom
        .Rank = 2
        .Font.Italic = True
    End With
    With Range("C2:C5").FormatConditions.AddAboveAverage
        .AboveBelow = xlBelowAverage
        .Interior.ColorIndex = 6
    End With
    With Range("B2:B5").FormatConditions.AddUniqueValues
        .DupeUnique = xlDuplicate
        .Font.Color = RGB(255, 0, 0)
    End With
    Range("A2:A5").FormatConditions.Add Type:=xlTextString, String:="an", TextOperator:=xlContains
    Range("A2:A5").FormatConditions(1).Interior.Color = RGB(255, 255, 0)
    Range("C2:C5").FormatConditions(2).SetFirstPriority
    Debug.Print Range("C2:C5").FormatConditions(1).Type & " " & Range("C2:C5").FormatConditions(1).Priority
End Sub
"#;
    let (book, outcome, printed) = run_printed(sales(), code, "More");
    ok(&outcome);
    assert_eq!(printed, ["12 1"]);
    let list = rules(&book);
    assert_eq!(list.len(), 4);
    let spec_of = |want: fn(&CfSpec) -> bool| {
        list.iter()
            .find(|r| r.def.as_ref().is_some_and(|d| want(&d.spec)))
            .unwrap()
            .clone()
    };
    let top = spec_of(|s| matches!(s, CfSpec::TopBottom { .. }));
    assert_eq!(
        top.def.as_ref().unwrap().spec,
        CfSpec::TopBottom {
            bottom: true,
            rank: 2,
            percent: false
        }
    );
    assert!(top.format.italic);
    let avg = spec_of(|s| matches!(s, CfSpec::Average { .. }));
    assert_eq!(
        avg.def.as_ref().unwrap().spec,
        CfSpec::Average {
            below: true,
            equal: false,
            std_dev: 0
        }
    );
    assert_eq!(avg.format.fill, Some(0xFFFF00));
    // Below average was moved first.
    assert_eq!(list[0].index, avg.index);
    let dup = spec_of(|s| matches!(s, CfSpec::Duplicates { .. }));
    assert_eq!(
        dup.def.as_ref().unwrap().spec,
        CfSpec::Duplicates { unique: false }
    );
    assert_eq!(dup.format.font, Some(0xFF0000));
    let text = spec_of(|s| matches!(s, CfSpec::Text { .. }));
    assert_eq!(
        text.def.as_ref().unwrap().spec,
        CfSpec::Text {
            op: CfTextOp::Contains,
            text: "an".into()
        }
    );
    assert_eq!(text.range, "A2:A5");
}

#[test]
fn icon_set_from_the_workbook_icon_sets() {
    let code = r#"
Sub Icons()
    With Range("C2:C5").FormatConditions.AddIconSetCondition
        .IconSet = ActiveWorkbook.IconSets(xl3Arrows)
        .ShowIconOnly = False
        .IconCriteria(2).Type = xlConditionValuePercent
        .IconCriteria(2).Value = 30
        .IconCriteria(3).Operator = xlGreater
        Debug.Print TypeName(Range("C2:C5").FormatConditions(1)) & " " & .IconCriteria.Count & " " & .IconSet.ID
    End With
End Sub
"#;
    let (book, outcome, printed) = run_printed(sales(), code, "Icons");
    ok(&outcome);
    assert_eq!(printed, ["IconSetCondition 3 1"]);
    let list = rules(&book);
    match &list[0].def.as_ref().unwrap().spec {
        CfSpec::IconSet {
            set,
            show_value,
            steps,
            ..
        } => {
            assert_eq!(set, "3Arrows");
            assert!(*show_value);
            assert_eq!(steps[1].value.kind, CfValueKind::Percent);
            assert_eq!(steps[1].value.value, "30");
            assert!(!steps[2].gte);
        }
        other => panic!("not an icon set: {other:?}"),
    }
}

#[test]
fn modify_applies_to_range_and_errors() {
    let code = r#"
Sub Grow()
    Range("C2:C5").FormatConditions.Add Type:=xlCellValue, Operator:=xlBetween, Formula1:="100", Formula2:="=$E$1"
    Range("C2:C5").FormatConditions(1).ModifyAppliesToRange Range("C2:C10")
    Debug.Print Range("C3").FormatConditions(1).AppliesTo.Address & " " & Range("C3").FormatConditions(1).Formula2
    On Error Resume Next
    Range("C2:C5").FormatConditions.Add Type:=xlErrorsCondition
    Debug.Print Err.Number
    Err.Clear
    Debug.Print Range("C2:C5").FormatConditions(5).Type
    Debug.Print Err.Number
End Sub
"#;
    let (book, outcome, printed) = run_printed(sales(), code, "Grow");
    ok(&outcome);
    assert_eq!(printed[0], "$C$2:$C$10 =$E$1");
    assert_eq!(printed[1], "445");
    assert_eq!(printed.last().unwrap(), "9");
    assert_eq!(rules(&book)[0].range, "C2:C10");
}

#[test]
fn interior_patterns() {
    let code = r#"
Sub Fill()
    With Range("A1:C1").Interior
        .Pattern = xlSolid
        .PatternColorIndex = xlAutomatic
        .Color = RGB(255, 192, 0)
    End With
    Debug.Print Range("A1").Interior.Pattern & " " & Range("D1").Interior.Pattern & " " & Range("A1").Interior.PatternColorIndex
    Range("A1").Interior.Pattern = xlNone
    On Error Resume Next
    Range("A2").Interior.Pattern = xlGray50
    Debug.Print Err.Number
End Sub
"#;
    let (book, outcome, printed) = run_printed(sales(), code, "Fill");
    ok(&outcome);
    assert_eq!(printed, ["1 -4142 -4105", "445"]);
    assert_eq!(book.cell_style(0, 0, 0).fill, None);
    assert_eq!(book.cell_style(0, 0, 1).fill, Some(0xFFC000));
    assert_eq!(book.cell_style(0, 1, 0).fill, None);
    let (book, _dir) = reopen(book, "fill.xlsx");
    assert_eq!(book.cell_style(0, 0, 2).fill, Some(0xFFC000));
    assert_eq!(book.cell_style(0, 0, 0).fill, None);
}

#[test]
fn characters_text_and_whole_cell_font() {
    let code = r#"
Sub Rich()
    Range("A7").Value = "Total sales"
    Debug.Print Range("A7").Characters(1, 5).Text & "|" & Range("A7").Characters(7).Text & "|" & Range("A7").Characters(Start:=7, Length:=3).Count
    Range("A7").Characters(1, 5).Text = "Gross"
    With Range("A7").Characters(Start:=1, Length:=11).Font
        .Bold = True
        .Color = RGB(192, 0, 0)
    End With
    ' What the cell has already: accepted for part of the text.
    With Range("A7").Characters(Start:=1, Length:=5).Font
        .Bold = True
        .Superscript = False
    End With
    On Error Resume Next
    Range("A7").Characters(1, 5).Font.Italic = True
    Debug.Print Err.Number
End Sub
"#;
    let mut start = sales();
    start.set_input(0, 6, 0, "Old").unwrap();
    let (mut book, outcome, printed) = run_printed(start, code, "Rich");
    ok(&outcome);
    assert_eq!(printed, ["Total|sales|3", "445"]);
    assert_eq!(input(&book, 0, "A7"), "Gross sales");
    let st = book.cell_style(0, 6, 0);
    assert!(st.bold);
    assert!(!st.italic);
    assert_eq!(st.font_color, Some(0xC00000));
    // One undo takes the whole run back.
    assert!(book.undo().unwrap());
    assert_eq!(input(&book, 0, "A7"), "Old");
    assert!(!book.cell_style(0, 6, 0).bold);
}

#[test]
fn one_border_edge_removed_leaves_the_others() {
    let code = r#"
Sub Edges()
    With Range("B2:C4").Borders
        .LineStyle = xlContinuous
        .Weight = xlThin
    End With
    Range("B2:C4").Borders(xlEdgeLeft).LineStyle = xlNone
    Range("B2:C4").Borders(xlInsideHorizontal).LineStyle = xlNone
    Debug.Print Range("B2").Borders(xlEdgeLeft).LineStyle & " " & Range("B2").Borders(xlEdgeTop).LineStyle
End Sub
"#;
    let (book, outcome, printed) = run_printed(sales(), code, "Edges");
    ok(&outcome);
    assert_eq!(printed, ["-4142 1"]);
    let b = |r: u32, c: u32| book.cell_style(0, r, c).borders;
    // B2: top and right stay, left and the inside line under it went.
    let b2 = b(1, 1);
    assert!(b2.left.is_none());
    assert!(b2.top.is_some());
    assert!(b2.right.is_some());
    assert!(b2.bottom.is_none());
    // C3: the vertical lines stay, the horizontal ones went.
    let c3 = b(2, 2);
    assert!(c3.left.is_some() && c3.right.is_some());
    assert!(c3.top.is_none() && c3.bottom.is_none());
    // B4 keeps the bottom edge; A2 has no right side facing B2.
    assert!(b(3, 1).bottom.is_some());
    assert!(b(3, 1).left.is_none());
    assert!(b(1, 0).right.is_none());
    assert!(b(3, 2).right.is_some());
}

#[test]
fn diagonal_borders_are_kept() {
    let code = r#"
Sub Diag()
    With Range("E2:E3").Borders(xlDiagonalDown)
        .LineStyle = xlContinuous
        .Color = RGB(255, 0, 0)
    End With
    Range("E2:E3").Borders(xlDiagonalUp).LineStyle = xlDash
    Range("E3").Borders(xlDiagonalUp).LineStyle = xlNone
    Debug.Print Range("E2").Borders(xlDiagonalUp).LineStyle & " " & Range("E3").Borders(xlDiagonalUp).LineStyle & " " & Range("E3").Borders(xlDiagonalDown).LineStyle
End Sub
"#;
    let (book, outcome, printed) = run_printed(sales(), code, "Diag");
    ok(&outcome);
    // A cell has one diagonal line style for both directions.
    assert_eq!(printed, ["-4115 -4142 -4115"]);
    let (up, down, line) = book.cell_diagonal(0, 1, 4);
    assert!(up && down);
    assert_eq!(line.map(|l| l.style), Some(LineStyle::Dashed));
    let (up, down, _) = book.cell_diagonal(0, 2, 4);
    assert!(!up && down);
    // Saved: the file has them.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("diag.xlsx");
    let mut book = book;
    book.save_as(&path).unwrap();
    let styles = package_text(&path, "xl/styles.xml").unwrap();
    assert!(styles.contains("diagonalUp=\"1\""));
    assert!(styles.contains("diagonalDown=\"1\""));
    let book = sheets_engine::Workbook::open(&path).unwrap();
    let (up, down, line) = book.cell_diagonal(0, 1, 4);
    assert!(up && down);
    assert_eq!(line.map(|l| l.style), Some(LineStyle::Dashed));
}

#[test]
fn undo_takes_rules_and_borders_back_in_one_step() {
    let code = r#"
Sub Format()
    Range("C2:C5").FormatConditions.Add Type:=xlCellValue, Operator:=xlLess, Formula1:="100"
    Range("C2:C5").FormatConditions(1).Interior.Color = RGB(255, 0, 0)
    Range("C2:C5").FormatConditions.AddDatabar
    Range("A1:C1").Borders(xlEdgeBottom).LineStyle = xlDouble
    Range("A1:C1").Borders(xlEdgeBottom).LineStyle = xlNone
End Sub
"#;
    let mut start = sales();
    start
        .set_range_border(
            0,
            CellRange::from_corners((0, 0), (0, 2)),
            sheets_engine::BorderEdges::Top,
            LineStyle::Thin,
            None,
        )
        .unwrap();
    let (mut book, outcome) = run(start, code, "Format");
    ok(&outcome);
    assert_eq!(rules(&book).len(), 2);
    assert!(book.cell_style(0, 0, 0).borders.bottom.is_none());
    assert!(book.cell_style(0, 0, 0).borders.top.is_some());
    assert!(book.undo().unwrap());
    assert!(rules(&book).is_empty());
    assert!(book.cell_style(0, 0, 0).borders.top.is_some());
}
