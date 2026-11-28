//! `Worksheet.Shapes`, `Shape`, `Worksheet.Pictures` from macros.

mod support;

use std::path::Path;

use sheets_engine::Workbook;
use sheets_engine::drawing::{Drawing, DrawingKind};
use support::*;

/// A 1 × 1 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, 0x49, 0x48, 0x44, 0x52, 0, 0, 0,
    1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 0x0D, 0x49, 0x44, 0x41, 0x54,
    0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0, 0x1F, 0, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D,
    0x1D, 0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// A `width` × `height` 24-bit BMP, all blue.
fn bmp(width: u32, height: u32) -> Vec<u8> {
    let row = (width * 3).div_ceil(4) * 4;
    let size = 54 + row * height;
    let mut v = b"BM".to_vec();
    v.extend_from_slice(&size.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&54u32.to_le_bytes());
    v.extend_from_slice(&40u32.to_le_bytes());
    v.extend_from_slice(&(width as i32).to_le_bytes());
    v.extend_from_slice(&(height as i32).to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&24u16.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&(row * height).to_le_bytes());
    v.extend_from_slice(&2835u32.to_le_bytes());
    v.extend_from_slice(&2835u32.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    for _ in 0..height {
        for x in 0..row {
            v.push(if x % 3 == 0 && x < width * 3 { 0xFF } else { 0 });
        }
    }
    v
}

/// A path as a VBA string literal's content.
fn vba_path(p: &Path) -> String {
    p.display().to_string().replace('"', "\"\"")
}

/// A new workbook saved in a fresh folder and opened from there (so a
/// macro's files in that folder are allowed).
fn saved_book() -> (Workbook, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("book.xlsx");
    Workbook::new_empty().unwrap().save_as(&path).unwrap();
    (Workbook::open(&path).unwrap(), dir)
}

fn named(book: &Workbook, sheet: u32, name: &str) -> Drawing {
    book.drawings(sheet)
        .into_iter()
        .find(|d| match &*d.kind {
            DrawingKind::Shape { xml, .. } => xml.contains(&format!("name=\"{name}\"")),
            _ => d.name == name,
        })
        .unwrap_or_else(|| panic!("no object named {name}"))
}

#[test]
fn shapes_are_added_formatted_listed_and_deleted() {
    let code = r#"
Sub Build()
    Dim shp As Shape, tb As Shape, s As Shape
    Set shp = ActiveSheet.Shapes.AddShape(msoShapeRectangle, Range("C4").Left, Range("C4").Top, _
        Range("C4:E6").Width, Range("C4:E6").Height)
    shp.Name = "Go button"
    shp.TextFrame.Characters.Text = "Run report"
    shp.Fill.ForeColor.RGB = RGB(255, 0, 0)
    shp.Line.ForeColor.RGB = RGB(0, 0, 255)
    shp.Line.Weight = 2.5
    shp.OnAction = "Report"
    Set tb = ActiveSheet.Shapes.AddTextbox(msoTextOrientationHorizontal, 10, 10, 80, 30)
    tb.TextFrame2.TextRange.Text = "Note" & vbCr & "two"
    tb.Left = 20
    tb.Width = 90
    ActiveSheet.Shapes.AddShape msoShapeOval, 300, 50, 40, 40
    For Each s In ActiveSheet.Shapes
        Debug.Print s.Name & "|" & s.Type
    Next
    Debug.Print shp.Fill.ForeColor.RGB & "|" & shp.Line.ForeColor.RGB & "|" & shp.Line.Weight & "|" & shp.OnAction
    Debug.Print shp.TextFrame.Characters(1, 3).Text & "|" & Replace(tb.TextFrame.Characters.Text, vbLf, "/")
    Debug.Print Replace(tb.TextFrame2.TextRange.Text, vbCr, "/")
    Debug.Print Round(tb.Left, 2) & "|" & Round(tb.Width, 2) & "|" & (Round(shp.Top, 2) = Round(Range("C4").Top, 2)) _
        & "|" & (Round(shp.Height, 2) = Round(Range("C4:E6").Height, 2))
    Debug.Print shp.TopLeftCell.Address & "|" & shp.BottomRightCell.Address & "|" & shp.Placement
    shp.Placement = xlFreeFloating
    Debug.Print shp.Placement & "|" & (Round(shp.Left, 2) = Round(Range("C4").Left, 2))
    ActiveSheet.Shapes("Oval 3").Delete
    Debug.Print ActiveSheet.Shapes.Count & "|" & ActiveSheet.Shapes(2).Name
    On Error Resume Next
    Set s = ActiveSheet.Shapes("Nope")
    Debug.Print Err.Number
End Sub
"#;
    let (book, outcome, printed) = run_printed(sales(), code, "Build");
    ok(&outcome);
    assert_eq!(
        printed,
        [
            "Go button|1",
            "TextBox 2|17",
            "Oval 3|1",
            "255|16711680|2.5|Report",
            "Run|Note/two",
            "Note/two",
            "20|90|True|True",
            "$C$4|$F$7|1",
            "3|True",
            "2|TextBox 2",
            "9",
        ]
    );
    let all = book.drawings(0);
    assert_eq!(all.len(), 2);
    let go = named(&book, 0, "Go button");
    assert_eq!(book.shape_text(go.key).as_deref(), Some("Run report"));
    let xml = book.shape_xml(go.key).unwrap();
    assert!(xml.contains("<a:srgbClr val=\"FF0000\"/>"), "{xml}");
    assert!(xml.contains("<a:ln w=\"31750\""), "{xml}");
    assert_eq!(
        book.drawing_macro_now(go.key).as_deref(),
        Some("[0]!Report")
    );
    let tb = named(&book, 0, "TextBox 2");
    assert_eq!(book.shape_text(tb.key).as_deref(), Some("Note\ntwo"));
}

#[test]
fn pictures_and_shapes_survive_save_and_reopen() {
    let (book, dir) = saved_book();
    let logo = dir.path().join("logo.png");
    std::fs::write(&logo, PNG).unwrap();
    std::fs::write(dir.path().join("chart.bmp"), bmp(40, 20)).unwrap();
    let code = format!(
        r#"
Sub Pics()
    Dim p As Shape, b As Shape, r As Shape
    Set p = ActiveSheet.Shapes.AddPicture("{logo}", msoFalse, msoTrue, 10, 20, 60, 30)
    p.Name = "Logo"
    p.AlternativeText = "Company logo"
    Set b = ActiveSheet.Shapes.AddPicture("chart.bmp", False, True, 100, 0, -1, -1)
    Debug.Print Round(b.Width, 2) & "|" & Round(b.Height, 2) & "|" & b.Type & "|" & b.Name
    Set r = ActiveSheet.Shapes.AddShape(msoShapeRoundedRectangle, 0, 100, 100, 30)
    r.Name = "Start"
    r.TextFrame2.TextRange.Text = "Start here"
    r.Fill.ForeColor.RGB = RGB(0, 128, 0)
    r.OnAction = "Module1.Go"
    Debug.Print ActiveSheet.Pictures.Count & "|" & ActiveSheet.Pictures(1).Name
End Sub
"#,
        logo = vba_path(&logo)
    );
    let (book, outcome, printed) = run_printed(book, &code, "Pics");
    ok(&outcome);
    assert_eq!(printed, ["30|15|13|Picture 2", "2|Logo"]);

    let (back, _dir2) = reopen(book, "after.xlsx");
    let logo = named(&back, 0, "Logo");
    assert_eq!(logo.descr, "Company logo");
    match &*logo.kind {
        DrawingKind::Picture { data, .. } => assert_eq!(data.as_slice(), PNG),
        other => panic!("not a picture: {other:?}"),
    }
    let start = named(&back, 0, "Start");
    assert_eq!(back.shape_text(start.key).as_deref(), Some("Start here"));
    assert!(back.shape_xml(start.key).unwrap().contains("008000"));
    assert_eq!(
        back.drawing_macro(start.key).as_deref(),
        Some("[0]!Module1.Go")
    );
    assert_eq!(back.drawings(0).len(), 3);

    // The reopened file's objects work from a macro too.
    let code = r#"
Sub Check()
    With ActiveSheet.Shapes("Start")
        Debug.Print .OnAction & "|" & .TextFrame2.TextRange.Text & "|" & .Fill.ForeColor.RGB
        .Name = "Begin"
    End With
    Debug.Print ActiveSheet.Shapes("Logo").Type & "|" & ActiveSheet.Shapes("Logo").AlternativeText
    Debug.Print ActiveSheet.Shapes(3).Name
End Sub
"#;
    let (back, outcome, printed) = run_printed(back, code, "Check");
    ok(&outcome);
    assert_eq!(
        printed,
        ["Module1.Go|Start here|32768", "13|Company logo", "Begin",]
    );
    let begin = named(&back, 0, "Begin");
    assert_eq!(back.shape_text(begin.key).as_deref(), Some("Start here"));
}

#[test]
fn pictures_outside_the_workbook_folder_are_refused() {
    let (book, _dir) = saved_book();
    let elsewhere = tempfile::tempdir().unwrap();
    let file = elsewhere.path().join("secret.png");
    std::fs::write(&file, PNG).unwrap();
    let code = format!(
        "Sub Pic()\n    ActiveSheet.Shapes.AddPicture \"{}\", False, True, 0, 0, -1, -1\nEnd Sub\n",
        vba_path(&file)
    );
    let (book, outcome) = run(book, &code, "Pic");
    assert_eq!(err(&outcome).number, 1004);
    assert!(book.drawings(0).is_empty());
}

#[test]
fn undo_takes_back_everything_a_run_added() {
    let code = r#"
Sub Mess()
    Dim t As Shape
    With ActiveSheet.Shapes.AddShape(msoShapeIsoscelesTriangle, 10, 10, 50, 50)
        .Name = "Tri"
        .Fill.ForeColor.RGB = RGB(1, 2, 3)
        .Line.Visible = msoFalse
    End With
    Set t = ActiveSheet.Shapes.AddTextbox(msoTextOrientationHorizontal, 100, 10, 60, 20)
    t.TextFrame.Characters.Text = "Hi"
    Range("A1").Value = "changed"
    Debug.Print ActiveSheet.Shapes("Tri").AutoShapeType & "|" & ActiveSheet.Shapes("Tri").Line.Visible
End Sub
"#;
    let (mut book, outcome, printed) = run_printed(sales(), code, "Mess");
    ok(&outcome);
    assert_eq!(printed, ["7|0"]);
    assert_eq!(book.drawings(0).len(), 2);
    let tri = named(&book, 0, "Tri");
    let xml = book.shape_xml(tri.key).unwrap();
    assert!(xml.contains("prst=\"triangle\""), "{xml}");
    assert!(xml.contains("010203"), "{xml}");
    assert_eq!(cell(&book, 0, "A1"), "changed");
    assert!(book.undo().unwrap());
    assert!(book.drawings(0).is_empty());
    assert_eq!(cell(&book, 0, "A1"), "Name");
}
