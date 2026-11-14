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
fn ods_brings_values_and_formatting() {
    // The ODS copy of the same workbook.
    let mut wb = Workbook::open(corpus("formatted.ods")).unwrap();
    assert_eq!(wb.source_format(), SourceFormat::Ods);
    // (`=Rate*100` keeps its value: calamine doesn't bring ODS names.)
    assert_eq!(wb.import_notes(), ["1 formula kept as values"]);
    assert_eq!(wb.display_value(0, 0, 0), "Quarterly sales, East region");
    assert_eq!(wb.input(0, 5, 1), "=SUM(B3:B5)");

    // Fonts, fills, alignment.
    let a1 = wb.cell_style(0, 0, 0);
    assert!(a1.bold);
    assert_eq!(a1.font_size, 16);
    assert_eq!(a1.font_name.as_deref(), Some("Arial"));
    assert_eq!(a1.font_color, Some(0xFFFFFF));
    assert_eq!(a1.fill, Some(0x1F4E78));
    assert_eq!((a1.align, a1.valign), (HAlign::Center, VAlign::Center));
    let b2 = wb.cell_style(0, 1, 1);
    assert_eq!(b2.fill, Some(0xDDEBF7));
    let bottom = b2.borders.bottom.unwrap();
    assert_eq!(
        (bottom.style, bottom.color),
        (LineStyle::Medium, Some(0x1F4E78))
    );
    assert!(b2.borders.top.is_none());

    // Number formats (number:*-style with their maps).
    assert_eq!(wb.display_value(0, 2, 1), "1,200.50");
    assert_eq!(wb.display_value(0, 4, 1), "-150.00");
    assert_eq!(wb.display_value(0, 5, 1), "Rp 1,851");
    assert_eq!(wb.display_value(0, 5, 5), "37.5%");
    assert_eq!(wb.display_value(0, 7, 3), "15/03/2024");
    assert_eq!(wb.display_value(0, 15, 0), "1.235E+03");

    // Borders, wrap, indent, rotation, strikethrough.
    let b3 = wb.cell_style(0, 2, 1);
    assert_eq!(b3.borders.left.unwrap().style, LineStyle::Thin);
    assert_eq!(
        wb.cell_style(0, 5, 1).borders.bottom.unwrap().style,
        LineStyle::Double
    );
    let e8 = wb.cell_style(0, 7, 4).borders;
    assert_eq!(e8.left.unwrap().color, Some(0xFF0000));
    assert_eq!(e8.top.unwrap().style, LineStyle::Thick);
    let f4 = wb.cell_style(0, 3, 5);
    assert!(f4.wrap);
    assert_eq!(f4.valign, VAlign::Top);
    let a5 = wb.cell_style(0, 4, 0);
    assert_eq!(a5.indent, 2);
    assert!(a5.italic && a5.underline && a5.strikethrough);
    assert_eq!(wb.cell_style(0, 7, 0).rotation, 45);
    assert_eq!(wb.cell_style(0, 7, 1).rotation, 90);
    assert_eq!(wb.cell_style(0, 7, 2).align, HAlign::Right);
    let f8 = wb.cell_style(0, 7, 5);
    assert_eq!(f8.font_name.as_deref(), Some("Courier New"));
    assert_eq!(f8.font_size, 9);
    assert_eq!(f8.font_color, Some(0xC00000));
    let plain = wb.cell_style(0, 2, 5);
    assert_eq!(plain.fill, None);
    assert!(!plain.bold);

    // Layout: merges, widths, heights, hidden column and row, sheets.
    assert_eq!(
        wb.merges_in(0, CellRange::all()),
        vec![
            CellRange::from_corners((0, 0), (0, 5)),
            CellRange::from_corners((9, 0), (10, 1)),
        ]
    );
    let w = |c| wb.column_width(0, c);
    assert!(
        (w(1) / w(0) - 14.0 / 24.0).abs() < 0.02,
        "{} {}",
        w(0),
        w(1)
    );
    assert!(wb.is_column_hidden(0, 6));
    assert!(!wb.is_column_hidden(0, 5));
    let rows = wb.row_spec(0);
    let row = |r: u32| rows.rows.iter().find(|e| e.row == r).copied();
    assert!((row(0).unwrap().points - 30.0).abs() < 0.1);
    assert!((row(7).unwrap().points - 48.0).abs() < 0.1);
    assert!(wb.is_row_hidden(0, 11));
    assert!(!wb.is_row_hidden(0, 10));
    assert!(!wb.sheet_hidden(1));
    assert!(wb.sheet_hidden(2));

    // Saved as .xlsx and opened again, it looks the same.
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("from-ods.xlsx");
    wb.save_as(&out).unwrap();
    let bytes = std::fs::read(&out).unwrap();
    assert!(sheets_engine::package_problems(&bytes).is_empty());
    let back = Workbook::open(&out).unwrap();
    assert_eq!(back.cell_style(0, 0, 0).fill, Some(0x1F4E78));
    assert_eq!(back.display_value(0, 5, 1), "Rp 1,851");
    assert!(back.is_column_hidden(0, 6));
    assert_eq!(back.merges_in(0, CellRange::all()).len(), 2);
}

#[test]
fn xls_notes_round_trip() {
    let mut wb = Workbook::open(corpus("features.xls")).unwrap();
    let notes = wb.notes(0);
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert_eq!((notes[0].row, notes[0].col), (1, 0));
    assert_eq!(notes[0].author, "Ann");
    assert_eq!(notes[0].text, "Bought weekly");
    assert_eq!((notes[1].row, notes[1].col), (2, 2));
    assert_eq!(notes[1].author, "Budi");
    assert_eq!(notes[1].text, "Price went up\nin February");
    let other = wb.notes(1);
    assert_eq!(other.len(), 1);
    assert_eq!((other[0].row, other[0].col), (1, 1));
    assert_eq!(other[0].text, "On the second sheet");

    // Saved as .xlsx with comment parts and their boxes; a row inserted
    // above moves them.
    wb.insert_rows(0, 0, 1).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("notes.xlsx");
    wb.save_as(&out).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let bytes = std::fs::read(&out).unwrap();
    assert!(sheets_engine::package_problems(&bytes).is_empty());
    let c1 = part(&out, "xl/comments1.xml");
    assert!(c1.contains("ref=\"A3\""), "{c1}");
    assert!(c1.contains("ref=\"C4\""), "{c1}");
    assert!(c1.contains("Price went up\nin February"), "{c1}");
    assert!(c1.contains("<author>Budi</author>"), "{c1}");
    let sheet1 = part(&out, "xl/worksheets/sheet1.xml");
    assert!(sheet1.contains("<legacyDrawing r:id="), "{sheet1}");
    let vml = part(&out, "xl/drawings/vmlDrawing1.vml");
    assert!(vml.contains("<x:Row>2</x:Row>"), "{vml}");
    let c2 = part(&out, "xl/comments2.xml");
    assert!(c2.contains("ref=\"B2\""), "{c2}");

    // Reopened: the notes are there (the saved file carries them on).
    let back = Workbook::open(&out).unwrap();
    let texts: Vec<String> = back.notes(0).into_iter().map(|n| n.text).collect();
    assert!(texts.contains(&"Bought weekly".to_owned()), "{texts:?}");
    let mut back = back;
    let again = dir.path().join("again.xlsx");
    back.save_as(&again).unwrap();
    assert!(part(&again, "xl/comments1.xml").contains("Bought weekly"));
}

#[test]
fn xls_data_validation_round_trip() {
    use sheets_engine::{ErrorStyle, ValidationOperator, ValidationType};
    let mut wb = Workbook::open(corpus("features.xls")).unwrap();
    assert!(wb.import_notes().is_empty(), "{:?}", wb.import_notes());
    let list = wb.sheet_validations(0);
    assert_eq!(list.len(), 7, "{list:?}");
    let at = |col: u32| list.iter().find(|v| v.ranges[0].left == col).unwrap();
    let f = at(5);
    assert_eq!(f.kind, ValidationType::List);
    assert_eq!(f.formula1.as_deref(), Some("\"Open,Closed,Late\""));
    assert_eq!(f.ranges, vec![CellRange::from_corners((1, 5), (4, 5))]);
    assert_eq!(at(6).formula1.as_deref(), Some("$H$1:$H$3"));
    let b = at(1);
    assert_eq!(
        (b.kind, b.operator),
        (ValidationType::Whole, ValidationOperator::Between)
    );
    assert_eq!(
        (b.formula1.as_deref(), b.formula2.as_deref()),
        (Some("0"), Some("100"))
    );
    let prompt = b.prompt.as_ref().unwrap();
    assert_eq!(
        (prompt.title.as_str(), prompt.text.as_str()),
        ("Quantity", "A whole number from 0 to 100")
    );
    let (style, error) = b.error.as_ref().unwrap();
    assert_eq!(*style, ErrorStyle::Stop);
    assert_eq!(error.text, "Use 0 to 100");
    let c = at(2);
    assert_eq!(
        (c.kind, c.operator),
        (ValidationType::Decimal, ValidationOperator::GreaterThan)
    );
    assert_eq!(c.error.as_ref().unwrap().0, ErrorStyle::Warning);
    assert_eq!(at(3).formula2.as_deref(), Some("DATE(2024,12,31)"));
    assert_eq!(at(4).kind, ValidationType::TextLength);
    assert_eq!(at(8).formula1.as_deref(), Some("ISNUMBER(I1)"));
    // The rules work: the drop-down, and a typed value is checked.
    let items = wb.validation_list(0, 2, 5).unwrap();
    assert_eq!(
        items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(),
        ["Open", "Closed", "Late"]
    );
    let items = wb.validation_list(0, 2, 6).unwrap();
    assert_eq!(items.len(), 3);
    // Tea's quantity (150) is over 100; Rice's (5) is fine.
    assert!(wb.invalid_cell(0, 3, 1));
    assert!(!wb.invalid_cell(0, 1, 1));

    // Saved as .xlsx: the rules are written, and moved with a row inserted.
    wb.insert_rows(0, 0, 1).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("dv.xlsx");
    wb.save_as(&out).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let sheet = part(&out, "xl/worksheets/sheet1.xml");
    assert!(sheet.contains("<dataValidations count=\"7\">"), "{sheet}");
    assert!(sheet.contains("sqref=\"F3:F6\""), "{sheet}");
    let back = Workbook::open(&out).unwrap();
    assert_eq!(back.sheet_validations(0).len(), 7);
}

#[test]
fn xls_conditional_formats_round_trip() {
    let mut wb = Workbook::open(corpus("features.xls")).unwrap();
    let rules = wb.cf_rules(0, None);
    assert_eq!(rules.len(), 4);
    let by = |range: &str| rules.iter().find(|r| r.range == range).unwrap();
    assert_eq!(by("B2:B5").summary, "Cell value between 1 and 10");
    assert_eq!(by("B2:B5").format.fill, Some(0xC6EFCE));
    let c = by("C2:C5");
    assert_eq!(c.summary, "Cell value greater than =$I$1");
    assert!(c.format.bold);
    assert_eq!(c.format.font, Some(0x9C0006));
    let f = by("F2:F5");
    assert_eq!(f.format.fill, Some(0xFFC7CE));
    assert!(f.format.italic);
    let a = by("A2:A5");
    assert_eq!(a.summary, "Formula: =AND($B2<0,LEN(A2)>3)");
    assert!(a.format.underline);
    assert_eq!(a.format.font, Some(0x0000FF));

    // What the cells show: B2 (5) is between 1 and 10, F5 is "Late", A5's
    // row has a negative quantity.
    let fill = |wb: &mut Workbook, r, c| {
        wb.conditional(0, r, c)
            .and_then(|x| x.style.as_ref().and_then(|s| s.fill))
    };
    assert_eq!(fill(&mut wb, 1, 1), Some(0xC6EFCE));
    assert_eq!(fill(&mut wb, 2, 1), None);
    assert_eq!(fill(&mut wb, 4, 5), Some(0xFFC7CE));
    let a5 = wb
        .conditional(0, 4, 0)
        .and_then(|x| x.style.clone())
        .unwrap();
    assert!(a5.underline);
    assert!(
        wb.conditional(0, 3, 0)
            .and_then(|x| x.style.clone())
            .is_none()
    );

    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("cf.xlsx");
    wb.save_as(&out).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let sheet = part(&out, "xl/worksheets/sheet1.xml");
    assert_eq!(
        sheet.matches("<conditionalFormatting").count(),
        4,
        "{sheet}"
    );
    let back = Workbook::open(&out).unwrap();
    assert_eq!(back.cf_rules(0, None).len(), 4);
}

fn cf_ranges(wb: &Workbook) -> Vec<String> {
    wb.cf_rules(0, None).into_iter().map(|r| r.range).collect()
}

/// The first `CF` in the file is Excel's priority 1: listed first (Manage
/// Rules), applied first, saved as `priority="1"`; Move Up / Move Down,
/// Delete and Stop If True act on the rule picked in that list.
#[test]
fn xls_conditional_formats_keep_their_priority_order() {
    let mut wb = Workbook::open(corpus("features.xls")).unwrap();
    // As LibreOffice and Excel list them (file order).
    assert_eq!(cf_ranges(&wb), ["B2:B5", "C2:C5", "F2:F5", "A2:A5"]);

    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("order.xlsx");
    wb.save_as(&out).unwrap();
    let sheet = part(&out, "xl/worksheets/sheet1.xml");
    let priority = |range: &str| {
        let block = &sheet[sheet.find(&format!("sqref=\"{range}\"")).unwrap()..];
        let block = &block[..block.find("</conditionalFormatting>").unwrap()];
        let at = block.find("priority=\"").unwrap() + 10;
        block[at..at + 1].to_owned()
    };
    assert_eq!(
        ["B2:B5", "C2:C5", "F2:F5", "A2:A5"].map(priority),
        ["1", "2", "3", "4"]
    );
    assert_eq!(
        cf_ranges(&Workbook::open(&out).unwrap()),
        cf_ranges(&wb)
    );

    // Move the last one up, then the first one down.
    let a = wb.cf_rules(0, None)[3].index;
    wb.move_cf_rule(0, a, true).unwrap();
    assert_eq!(cf_ranges(&wb), ["B2:B5", "C2:C5", "A2:A5", "F2:F5"]);
    let b = wb.cf_rules(0, None)[0].index;
    wb.move_cf_rule(0, b, false).unwrap();
    assert_eq!(cf_ranges(&wb), ["C2:C5", "B2:B5", "A2:A5", "F2:F5"]);
    // At either end nothing moves.
    let top = wb.cf_rules(0, None)[0].index;
    wb.move_cf_rule(0, top, true).unwrap();
    assert_eq!(cf_ranges(&wb)[0], "C2:C5");

    // Stop If True on the second one shown.
    let second = wb.cf_rules(0, None)[1].clone();
    let mut def = second.def.clone().unwrap();
    def.stop_if_true = true;
    wb.update_cf_rule(0, second.index, &second.range, &def)
        .unwrap();
    let stops: Vec<bool> = wb
        .cf_rules(0, None)
        .iter()
        .map(|r| r.stop_if_true)
        .collect();
    assert_eq!(stops, [false, true, false, false]);
    assert_eq!(cf_ranges(&wb)[1], "B2:B5");

    // Delete the third one shown.
    let third = wb.cf_rules(0, None)[2].index;
    wb.delete_cf_rule(0, third).unwrap();
    assert_eq!(cf_ranges(&wb), ["C2:C5", "B2:B5", "F2:F5"]);
    assert!(wb.undo().unwrap());
    assert_eq!(cf_ranges(&wb), ["C2:C5", "B2:B5", "A2:A5", "F2:F5"]);
}
