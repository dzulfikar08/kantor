//! Formatting of Excel 97-2003 imports (`.xls`, `.xlt`), against the
//! fixtures `scripts/corpus/gen_xls.py` makes (tests/corpus/xls): LibreOffice
//! conversions of openpyxl workbooks, and a file xlwt writes directly.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use sheets_engine::{CellRange, HAlign, LineStyle, Paper, SourceFormat, VAlign, Workbook};

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/xls")
        .join(name)
}

/// A part of a saved `.xlsx`, as text.
fn part(path: &Path, name: &str) -> String {
    let file = std::fs::File::open(path).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    let mut s = String::new();
    zip.by_name(name)
        .unwrap_or_else(|_| panic!("no {name}"))
        .read_to_string(&mut s)
        .unwrap();
    s
}

#[test]
fn cell_formats() {
    let wb = Workbook::open(corpus("formatted.xls")).unwrap();
    assert_eq!(wb.source_format(), SourceFormat::Xls);
    assert!(wb.import_notes().is_empty(), "{:?}", wb.import_notes());
    assert!(!wb.is_dirty());

    // Title: Arial 16 bold white on a custom-palette blue, centred.
    let a1 = wb.cell_style(0, 0, 0);
    assert!(a1.bold);
    assert_eq!(a1.font_size, 16);
    assert_eq!(a1.font_name.as_deref(), Some("Arial"));
    assert_eq!(a1.font_color, Some(0xFFFFFF));
    assert_eq!(a1.fill, Some(0x1F4E78));
    assert_eq!((a1.align, a1.valign), (HAlign::Center, VAlign::Center));

    // Header: a colour not in the default palette (PALETTE record), and a
    // medium bottom border in another custom colour.
    let b2 = wb.cell_style(0, 1, 1);
    assert_eq!(b2.fill, Some(0xDDEBF7));
    assert_eq!(b2.font_color, Some(0x1F4E78));
    let bottom = b2.borders.bottom.unwrap();
    assert_eq!(bottom.style, LineStyle::Medium);
    assert_eq!(bottom.color, Some(0x1F4E78));

    // Custom number formats (FORMAT records) and built-in ones.
    assert_eq!(wb.display_value(0, 2, 1), "1,200.50");
    assert_eq!(wb.display_value(0, 4, 1), "-150.00");
    assert_eq!(wb.cell_style(0, 4, 1).num_fmt, "#,##0.00;[RED]\\-#,##0.00");
    assert_eq!(wb.display_value(0, 5, 1), "Rp 1,851");
    assert_eq!(wb.display_value(0, 5, 5), "37.5%");
    assert_eq!(wb.display_value(0, 7, 3), "15/03/2024");
    assert_eq!(wb.display_value(0, 15, 0), "1.235E+03");
    // Formulas still come from calamine.
    assert_eq!(wb.input(0, 5, 1), "=SUM(B3:B5)");

    // Thin box, then thin top + double bottom.
    let b3 = wb.cell_style(0, 2, 1);
    for edge in [
        b3.borders.left,
        b3.borders.right,
        b3.borders.top,
        b3.borders.bottom,
    ] {
        assert_eq!(edge.unwrap().style, LineStyle::Thin);
    }
    let b6 = wb.cell_style(0, 5, 1);
    assert_eq!(b6.borders.top.unwrap().style, LineStyle::Thin);
    assert_eq!(b6.borders.bottom.unwrap().style, LineStyle::Double);

    // Border styles and colours of their own.
    let e8 = wb.cell_style(0, 7, 4);
    let b = e8.borders;
    assert_eq!(
        (b.left.unwrap().style, b.left.unwrap().color),
        (LineStyle::Dashed, Some(0xFF0000))
    );
    assert_eq!(
        (b.right.unwrap().style, b.right.unwrap().color),
        (LineStyle::Dotted, Some(0x00B050))
    );
    assert_eq!(
        (b.top.unwrap().style, b.top.unwrap().color),
        (LineStyle::Thick, Some(0x7030A0))
    );
    assert_eq!(b.bottom.unwrap().style, LineStyle::Hair);

    // Alignment: wrap + top, indent, rotation, right.
    let f4 = wb.cell_style(0, 3, 5);
    assert!(f4.wrap);
    assert_eq!(f4.valign, VAlign::Top);
    let a5 = wb.cell_style(0, 4, 0);
    assert_eq!(a5.indent, 2);
    assert!(a5.italic && a5.underline && a5.strikethrough);
    assert_eq!(wb.cell_style(0, 7, 0).rotation, 45);
    assert_eq!(wb.cell_style(0, 7, 1).rotation, 90);
    assert_eq!(wb.cell_style(0, 7, 2).align, HAlign::Right);

    // Fonts of their own.
    let f8 = wb.cell_style(0, 7, 5);
    assert_eq!(f8.font_name.as_deref(), Some("Courier New"));
    assert_eq!(f8.font_size, 9);
    assert_eq!(f8.font_color, Some(0xC00000));

    // Unformatted cells keep the workbook default.
    let plain = wb.cell_style(0, 2, 5);
    assert_eq!(plain.fill, None);
    assert_eq!(plain.font_name, None);
    assert!(!plain.bold);
}

#[test]
fn layout() {
    let wb = Workbook::open(corpus("formatted.xls")).unwrap();
    assert_eq!(
        wb.merges_in(0, CellRange::all()),
        vec![
            CellRange::from_corners((0, 0), (0, 5)),
            CellRange::from_corners((9, 0), (10, 1)),
        ]
    );
    // The merged block's fill covers the whole block.
    assert_eq!(wb.cell_style(0, 10, 1).fill, Some(0xFFFF00));

    // Widths 24 and 14 characters (openpyxl), as Excel stores them.
    let w = |c| wb.column_width(0, c);
    assert!(w(0) > w(1) && w(5) > w(0), "{} {} {}", w(0), w(1), w(5));
    assert!((w(1) / w(0) - 14.0 / 24.0).abs() < 0.01);
    assert!(wb.is_column_hidden(0, 6));
    assert!(!wb.is_column_hidden(0, 5));

    let rows = wb.row_spec(0);
    let row = |r: u32| rows.rows.iter().find(|e| e.row == r).copied();
    assert_eq!(row(0).unwrap().points, 30.0);
    assert!(row(0).unwrap().custom);
    assert_eq!(row(7).unwrap().points, 48.0);
    assert!(wb.is_row_hidden(0, 11));
    assert!(!wb.is_row_hidden(0, 10));
    assert_eq!(rows.default_points, 15.0);

    // "Secret" is hidden. (Headless LibreOffice drops view settings such as
    // gridlines and frozen panes: those are checked on the xlwt file.)
    assert!(!wb.sheet_hidden(1));
    assert!(wb.sheet_hidden(2));
}

#[test]
fn page_setup_and_print_names() {
    let wb = Workbook::open(corpus("formatted.xls")).unwrap();
    let p = wb.print_settings(0);
    assert!(p.setup.paper.same_as(&Paper::A4));
    assert!(p.setup.landscape);
    assert_eq!(p.setup.fit, Some((1, 0)));
    assert_eq!((p.margins.left, p.margins.top), (0.5, 1.0));
    assert_eq!(p.print_area, vec![CellRange::from_corners((0, 0), (15, 5))]);
    assert_eq!(p.title_rows, Some((0, 1)));
    // A sheet that says nothing prints portrait.
    assert!(!wb.print_settings(1).setup.landscape);
}

#[test]
fn names_and_formulas_using_them() {
    let wb = Workbook::open(corpus("formatted.xls")).unwrap();
    // =Rate*100 with Rate = Report!$F$6 (0.375): a live formula.
    assert_eq!(wb.input(1, 2, 1), "=Rate*100");
    assert_eq!(wb.display_value(1, 2, 1), "37.5");
    assert_eq!(wb.input(1, 1, 1), "=Report!B6*2");
}

#[test]
fn saved_xlsx_keeps_links_names_tabs_and_page_setup() {
    let mut wb = Workbook::open(corpus("formatted.xls")).unwrap();
    assert!(wb.path().is_none(), "imports save as a new file");
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("formatted.xlsx");
    wb.save_as(&out).unwrap();

    let book = part(&out, "xl/workbook.xml");
    assert!(book.contains("name=\"Rate\""), "{book}");
    assert!(book.contains("Report!$F$6"), "{book}");
    assert!(book.contains("name=\"Local\" localSheetId=\"0\""), "{book}");
    assert!(book.contains("_xlnm.Print_Area"), "{book}");
    assert!(book.contains("_xlnm.Print_Titles"), "{book}");
    assert!(book.contains("state=\"hidden\""), "{book}");

    let report = part(&out, "xl/worksheets/sheet1.xml");
    assert!(report.contains("<hyperlink ref=\"A14\""), "{report}");
    assert!(report.contains("orientation=\"landscape\""), "{report}");
    assert!(report.contains("paperSize=\"9\""), "{report}");
    assert!(report.contains("fitToPage=\"1\""), "{report}");
    assert!(report.contains("<mergeCell ref=\"A1:F1\"/>"), "{report}");
    let rels = part(&out, "xl/worksheets/_rels/sheet1.xml.rels");
    assert!(rels.contains("https://kantor.paparanai.id/"), "{rels}");

    let plain = part(&out, "xl/worksheets/sheet2.xml");
    assert!(plain.contains("<tabColor rgb=\"FFFF0000\""), "{plain}");

    // And the saved file opens with the same look.
    let back = Workbook::open(&out).unwrap();
    assert_eq!(back.cell_style(0, 1, 1).fill, Some(0xDDEBF7));
    assert_eq!(back.display_value(0, 5, 1), "Rp 1,851");
}

#[test]
fn xlt_templates_open_like_xls() {
    let wb = Workbook::open(corpus("formatted.xlt")).unwrap();
    assert_eq!(wb.source_format(), SourceFormat::Xls);
    assert!(wb.path().is_none());
    assert_eq!(wb.suggested_file_name(), "formatted.xlsx");
    assert_eq!(wb.cell_style(0, 1, 1).fill, Some(0xDDEBF7));
}

#[test]
fn frozen_panes_active_tab_and_patterns() {
    let wb = Workbook::open(corpus("xlwt.xls")).unwrap();
    assert_eq!(wb.frozen(0), (1, 1));
    assert_eq!(wb.active_sheet(), 1);
    assert!(!wb.is_dirty());

    let head = wb.cell_style(0, 0, 1);
    assert!(head.bold);
    assert_eq!(head.font_color, Some(0xFFFFFF));
    assert_eq!(head.fill, Some(0x000080));
    // A 25 % grey pattern, black on white, shows as one mixed colour.
    assert_eq!(wb.cell_style(0, 1, 4).fill, Some(0xBFBFBF));
    assert!(wb.show_gridlines(0));
    assert!(!wb.show_gridlines(1));
    assert!(wb.column_width(0, 0) > wb.column_width(0, 1));

    let p = wb.print_settings(0);
    assert!(p.setup.landscape);
    assert!(p.setup.paper.same_as(&Paper::A4));
    assert_eq!(p.row_breaks, vec![20]);
    assert_eq!(p.header_footer.odd_header, "&CQuarterly");
    assert_eq!(p.header_footer.odd_footer, "&RPage &P");
    assert!(p.options.horizontal_centered);
}

#[test]
fn date_system_1904() {
    let wb = Workbook::open(corpus("dates1904.xls")).unwrap();
    // Stored as 1904 serial 43903; shown as the same date.
    assert_eq!(wb.display_value(0, 0, 0), "2024-03-15");
    // Plain numbers are not dates and don't move.
    assert_eq!(wb.display_value(0, 1, 0), "2.5");
}

#[test]
fn malformed_xls_imports_values_or_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    // Not a compound file at all.
    let junk = dir.path().join("junk.xls");
    std::fs::write(&junk, b"definitely not a spreadsheet").unwrap();
    assert!(Workbook::open(&junk).is_err());

    // Every record of the real file with its payload damaged: never a panic.
    let bytes = std::fs::read(corpus("formatted.xls")).unwrap();
    for (i, step) in [(0x1Fusize, 97usize), (0x55, 131), (0xAA, 173)] {
        let mut b = bytes.clone();
        let mut at = 512 + i;
        while at < b.len() {
            b[at] ^= 0x5A;
            at += step;
        }
        let path = dir.path().join(format!("bad{i}.xls"));
        std::fs::write(&path, &b).unwrap();
        let _ = Workbook::open(&path);
    }
}

#[test]
fn ods_brings_values_without_formatting() {
    // The ODS copy of the same workbook: values and formulas, no formats yet.
    let wb = Workbook::open(corpus("formatted.ods")).unwrap();
    assert_eq!(wb.source_format(), SourceFormat::Ods);
    assert_eq!(wb.display_value(0, 0, 0), "Quarterly sales, East region");
    assert_eq!(wb.input(0, 5, 1), "=SUM(B3:B5)");
    assert_eq!(wb.cell_style(0, 0, 0).fill, None);
}
