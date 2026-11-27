//! Page setup, page breaks, PDF export and AutoFit from macros: the engine
//! state they leave, what a save keeps, one undo step per run.

mod support;

use sheets_engine::{CellRange, Paper, PrintComments, Workbook};
use support::*;

const SETUP: &str = r#"
Sub Setup()
    With ActiveSheet.PageSetup
        .Orientation = xlLandscape
        .PaperSize = xlPaperA4
        .PrintArea = "$A$1:$C$5"
        .PrintTitleRows = "$1:$1"
        .Zoom = False
        .FitToPagesWide = 1
        .FitToPagesTall = False
        .LeftMargin = Application.InchesToPoints(0.5)
        .RightMargin = Application.InchesToPoints(0.5)
        .TopMargin = Application.CentimetersToPoints(2.54)
        .BottomMargin = 54
        .HeaderMargin = Application.InchesToPoints(0.3)
        .FooterMargin = Application.InchesToPoints(0.3)
        .CenterHorizontally = True
        .CenterVertically = False
        .PrintGridlines = True
        .PrintHeadings = False
        .LeftHeader = "&D"
        .CenterHeader = "Sales report"
        .RightFooter = "Page &P of &N"
        .FirstPageNumber = xlAutomatic
        .Order = xlOverThenDown
        .BlackAndWhite = False
        .Draft = False
        .PrintComments = xlPrintSheetEnd
    End With
End Sub

Sub ReadBack()
    With ActiveSheet.PageSetup
        Debug.Print CStr(.Orientation) & "|" & CStr(.PaperSize) & "|" & .PrintArea & "|" & .PrintTitleRows
        Debug.Print CStr(.Zoom) & "|" & CStr(.FitToPagesWide) & "|" & CStr(.FitToPagesTall)
        Debug.Print CStr(.LeftMargin) & "|" & CStr(.TopMargin) & "|" & CStr(.BottomMargin)
        Debug.Print .LeftHeader & "|" & .CenterHeader & "|" & .RightFooter & "|" & CStr(.FirstPageNumber)
        Debug.Print .CenterHorizontally & "|" & .PrintGridlines & "|" & CStr(.Order)
    End With
End Sub
"#;

fn check_setup(book: &Workbook) {
    let p = book.print_settings(0);
    assert!(p.setup.landscape);
    assert!(p.setup.paper.same_as(&Paper::A4));
    assert_eq!(p.print_area, vec![CellRange::from_corners((0, 0), (4, 2))]);
    assert_eq!(p.title_rows, Some((0, 0)));
    assert_eq!(p.title_cols, None);
    assert_eq!(p.setup.fit, Some((1, 0)));
    assert!((p.margins.left - 0.5).abs() < 1e-9);
    assert!((p.margins.right - 0.5).abs() < 1e-9);
    assert!((p.margins.top - 1.0).abs() < 1e-9);
    assert!((p.margins.bottom - 0.75).abs() < 1e-9);
    assert!((p.margins.header - 0.3).abs() < 1e-9);
    assert!(p.options.horizontal_centered);
    assert!(!p.options.vertical_centered);
    assert!(p.options.grid_lines);
    assert_eq!(p.header_footer.odd_header, "&L&D&CSales report");
    assert_eq!(p.header_footer.odd_footer, "&RPage &P of &N");
    assert_eq!(p.setup.first_page_number, None);
    assert!(p.setup.over_then_down);
    assert_eq!(p.setup.comments, PrintComments::AtEnd);
}

#[test]
fn page_setup_from_a_macro_is_kept_and_saved() {
    let (book, outcome) = run(sales(), SETUP, "Setup");
    ok(&outcome);
    check_setup(&book);

    let (book, outcome, printed) = run_printed(book, SETUP, "ReadBack");
    ok(&outcome);
    assert_eq!(
        printed,
        [
            "2|9|$A$1:$C$5|$1:$1",
            "False|1|False",
            "36|72|54",
            "&D|Sales report|Page &P of &N|-4105",
            "True|True|2",
        ]
    );

    let (reopened, dir) = reopen(book, "setup.xlsx");
    check_setup(&reopened);
    let xml = package_text(&dir.path().join("setup.xlsx"), "xl/worksheets/sheet1.xml").unwrap();
    assert!(xml.contains("orientation=\"landscape\""), "{xml}");
    assert!(xml.contains("paperSize=\"9\""), "{xml}");
    assert!(xml.contains("fitToHeight=\"0\""), "{xml}");
    assert!(xml.contains("fitToPage=\"1\""), "{xml}");
    let book_xml = package_text(&dir.path().join("setup.xlsx"), "xl/workbook.xml").unwrap();
    assert!(book_xml.contains("_xlnm.Print_Area"), "{book_xml}");
    assert!(book_xml.contains("_xlnm.Print_Titles"), "{book_xml}");
}

#[test]
fn zoom_and_even_pages() {
    let code = r#"
Sub Go()
    With Worksheets(1).PageSetup
        .FitToPagesWide = 2
        .Zoom = 80
        .OddAndEvenPagesHeaderFooter = True
        .EvenPage.CenterFooter.Text = "Even &P"
        .DifferentFirstPageHeaderFooter = True
        .FirstPage.LeftHeader.Text = "Cover"
        .PrintArea = ""
        .PrintTitleColumns = "$A:$B"
    End With
    Debug.Print CStr(Worksheets(1).PageSetup.Zoom) & "|" & Worksheets(1).PageSetup.PrintTitleColumns
End Sub
"#;
    let (book, outcome, printed) = run_printed(sales(), code, "Go");
    ok(&outcome);
    assert_eq!(printed, ["80|$A:$B"]);
    let p = book.print_settings(0);
    assert_eq!(p.setup.fit, None);
    assert_eq!(p.setup.scale, 80);
    assert!(p.header_footer.different_odd_even);
    assert_eq!(p.header_footer.even_footer, "&CEven &P");
    assert!(p.header_footer.different_first);
    assert_eq!(p.header_footer.first_header, "&LCover");
    assert!(p.print_area.is_empty());
    assert_eq!(p.title_cols, Some((0, 1)));
}

#[test]
fn bad_page_setup_values_fail_with_1004() {
    for line in [".Zoom = 5", ".PaperSize = 9999", ".Orientation = 7"] {
        let code = format!(
            "Sub Go()\n    With ActiveSheet.PageSetup\n        {line}\n    End With\nEnd Sub\n"
        );
        let (_, outcome) = run(sales(), &code, "Go");
        assert_eq!(err(&outcome).number, 1004, "{line}");
    }
    let code = "Sub Go()\n    ActiveSheet.PageSetup.NoSuchThing = 1\nEnd Sub\n";
    let (_, outcome) = run(sales(), code, "Go");
    assert_eq!(err(&outcome).number, 438);
}

#[test]
fn page_breaks_added_listed_and_reset() {
    let code = r#"
Sub Breaks()
    ActiveSheet.HPageBreaks.Add Before:=Range("A10")
    ActiveSheet.VPageBreaks.Add Before:=Range("C1")
    Rows(20).PageBreak = xlPageBreakManual
    Debug.Print CStr(ActiveSheet.HPageBreaks.Count) & "|" & ActiveSheet.HPageBreaks(1).Location.Address
    Debug.Print CStr(ActiveSheet.VPageBreaks.Count) & "|" & CStr(Rows(20).PageBreak)
End Sub

Sub Reset()
    ActiveSheet.ResetAllPageBreaks
End Sub
"#;
    let (book, outcome, printed) = run_printed(sales(), code, "Breaks");
    ok(&outcome);
    assert_eq!(printed, ["2|$A$10", "1|-4135"]);
    let p = book.print_settings(0);
    assert_eq!(p.row_breaks, vec![9, 19]);
    assert_eq!(p.col_breaks, vec![2]);

    let (book, outcome) = run(book, code, "Reset");
    ok(&outcome);
    let p = book.print_settings(0);
    assert!(p.row_breaks.is_empty() && p.col_breaks.is_empty());
}

#[test]
fn undo_takes_a_page_setup_run_back_in_one_step() {
    let code = r#"
Sub Go()
    With ActiveSheet.PageSetup
        .Orientation = xlLandscape
        .PaperSize = xlPaperLegal
        .CenterFooter = "&P"
        .PrintArea = "A1:B3"
    End With
    ActiveSheet.HPageBreaks.Add Range("A4")
End Sub
"#;
    let before = sales().print_settings(0);
    let (mut book, outcome) = run(sales(), code, "Go");
    ok(&outcome);
    assert!(book.print_settings(0).setup.landscape);
    assert!(book.undo().unwrap());
    assert_eq!(book.print_settings(0), before);
}

/// A workbook saved in a fresh folder and opened from there, so macros
/// have a folder to write in.
fn saved_sales() -> (Workbook, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Report.xlsx");
    let mut book = sales();
    book.save_as(&path).unwrap();
    (Workbook::open(&path).unwrap(), dir)
}

fn vba_path(p: &std::path::Path) -> String {
    p.display().to_string().replace('"', "\"\"")
}

#[test]
fn export_as_fixed_format_writes_pdfs_in_the_workbook_folder() {
    let (book, dir) = saved_sales();
    let sheet_pdf = dir.path().join("sheet.pdf");
    let code = format!(
        r#"
Sub Export()
    ActiveSheet.ExportAsFixedFormat Type:=xlTypePDF, Filename:="{}", Quality:=xlQualityStandard, IncludeDocProperties:=True, IgnorePrintAreas:=False, OpenAfterPublish:=False
    ThisWorkbook.ExportAsFixedFormat xlTypePDF
    Range("A1:B3").ExportAsFixedFormat Type:=xlTypePDF, Filename:="range"
End Sub
"#,
        vba_path(&sheet_pdf)
    );
    let (_, outcome) = run(book, &code, "Export");
    ok(&outcome);
    for name in ["sheet.pdf", "Report.pdf", "range.pdf"] {
        let bytes = std::fs::read(dir.path().join(name)).unwrap();
        assert!(bytes.starts_with(b"%PDF"), "{name}");
    }
}

#[test]
fn export_outside_the_folder_and_printing_are_refused() {
    let (book, dir) = saved_sales();
    let other = tempfile::tempdir().unwrap();
    let outside = other.path().join("out.pdf");
    let code = format!(
        "Sub Go()\n    ActiveSheet.ExportAsFixedFormat xlTypePDF, \"{}\"\nEnd Sub\n",
        vba_path(&outside)
    );
    let (book, outcome) = run(book, &code, "Go");
    assert_eq!(err(&outcome).number, 1004);
    assert!(!outside.exists());

    let (book, outcome) = run(book, "Sub Go()\n    ActiveSheet.PrintOut\nEnd Sub\n", "Go");
    assert_eq!(err(&outcome).number, 445);
    let code = "Sub Go()\n    ActiveSheet.ExportAsFixedFormat xlTypeXPS, \"x.xps\"\nEnd Sub\n";
    let (_, outcome) = run(book, code, "Go");
    assert_eq!(err(&outcome).number, 445);
    assert!(!dir.path().join("x.xps").exists());
}

#[test]
fn autofit_widens_a_column_for_long_text() {
    let mut book = sales();
    set(
        &mut book,
        0,
        "A6",
        "A much longer customer name than the others",
    );
    let narrow = book.column_width(0, 0);
    let code = r#"
Sub Fit()
    Columns("A:C").AutoFit
    Range("E1").Value = "Wrapped text that goes on" & vbLf & "and on"
    Range("E1").WrapText = True
    Rows(1).RowHeight = 40
    Rows(1).AutoFit
End Sub
"#;
    let (mut book, outcome) = run(book, code, "Fit");
    ok(&outcome);
    let wide = book.column_width(0, 0);
    assert!(wide > narrow + 50.0, "{narrow} -> {wide}");
    // The short "Amount" column fits too, narrower than the long one.
    assert!(book.column_width(0, 2) < wide);
    // Row 1 follows its content again (no custom height).
    assert!(!book.row_spec(0).rows.iter().any(|r| r.row == 0 && r.custom));
    assert!(book.undo().unwrap());
    assert_eq!(book.column_width(0, 0), narrow);
}

#[test]
fn autofit_columns_of_a_range_look_only_at_its_rows() {
    let mut book = sales();
    set(
        &mut book,
        0,
        "A6",
        "A much longer customer name than the others",
    );
    let code = "Sub Fit()\n    Range(\"A1:A5\").Columns.AutoFit\nEnd Sub\n";
    let (book, outcome) = run(book, code, "Fit");
    ok(&outcome);
    let short = book.column_width(0, 0);
    let mut long = sales();
    set(
        &mut long,
        0,
        "A6",
        "A much longer customer name than the others",
    );
    let (long, outcome) = run(long, "Sub Fit()\n    Columns(1).AutoFit\nEnd Sub\n", "Fit");
    ok(&outcome);
    assert!(long.column_width(0, 0) > short + 50.0);
}
