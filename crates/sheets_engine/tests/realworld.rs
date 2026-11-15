//! What the real-world sweep (tools/realworld, docs/realworld-sweep.md)
//! found in public files, each rebuilt here as a minimal synthetic package:
//! files from other writers that Kantor refused, panicked on or took
//! minutes to open.

use std::io::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use sheets_engine::Workbook;

const ODS_MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/></manifest:manifest>"#;

/// A zip with these parts, in this order.
fn write_zip(path: &Path, parts: &[(&str, &[u8])]) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    for (name, data) in parts {
        zip.start_file(*name, opts).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
}

const CT: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>"#;
const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
const WORKBOOK: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>"#;

fn workbook_rels(styles: bool) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>{}</Relationships>"#,
        if styles {
            r#"<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>"#
        } else {
            ""
        }
    )
}

fn sheet(data: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{data}</sheetData></worksheet>"#
    )
}

fn xlsx(path: &Path, sheet_data: &str, styles: Option<&str>) {
    let rels = workbook_rels(styles.is_some());
    let sheet = sheet(sheet_data);
    let mut parts: Vec<(&str, &[u8])> = vec![
        ("[Content_Types].xml", CT.as_bytes()),
        ("_rels/.rels", ROOT_RELS.as_bytes()),
        ("xl/workbook.xml", WORKBOOK.as_bytes()),
        ("xl/_rels/workbook.xml.rels", rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
    ];
    if let Some(s) = styles {
        parts.push(("xl/styles.xml", s.as_bytes()));
    }
    write_zip(path, &parts);
}

/// Opens, saves and opens again; the cells' inputs must survive.
fn round_trip(path: &Path, cells: &[(u32, u32, &str)]) {
    let mut wb = Workbook::open(path).unwrap();
    for (r, c, v) in cells {
        assert_eq!(wb.input(0, *r, *c), *v, "R{}C{}", r + 1, c + 1);
    }
    let out = path.with_file_name("saved.xlsx");
    wb.save_as(&out).unwrap();
    let back = Workbook::open(&out).unwrap();
    for (r, c, v) in cells {
        assert_eq!(back.input(0, *r, *c), *v, "saved R{}C{}", r + 1, c + 1);
    }
}

/// The styles part is optional: generators that write values only leave it
/// out, and Excel opens their files ("Zip Error: specified file not found").
#[test]
fn xlsx_without_styles_part_opens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bare.xlsx");
    xlsx(
        &path,
        r#"<row r="1"><c r="A1" t="inlineStr"><is><t>Name</t></is></c><c r="B1"><v>42</v></c></row>"#,
        None,
    );
    round_trip(&path, &[(0, 0, "Name"), (0, 1, "42")]);
}

/// A styles part without `<fills>`, `<borders>`, `<cellStyleXfs>` or
/// `<cellStyles>`, or with a format pointing at a font that isn't there,
/// panicked (index out of bounds).
#[test]
fn xlsx_with_partial_styles_opens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("partial.xlsx");
    let styles = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts><cellXfs count="2"><xf numFmtId="0" fontId="0"/><xf numFmtId="0" fontId="1" fillId="7" borderId="3"/></cellXfs></styleSheet>"#;
    xlsx(
        &path,
        r#"<row r="1"><c r="A1" s="0"><v>1.5</v></c><c r="B1" s="1"><v>2</v></c></row>"#,
        Some(styles),
    );
    round_trip(&path, &[(0, 0, "1.5"), (0, 1, "2")]);
    // A format pointing past the fonts, fills and borders there are (LibreOffice
    // test files, Excel repairs them) uses the first of each.
    let wb = Workbook::open(&path).unwrap();
    assert_eq!(wb.cell_style(0, 0, 1).fill, None);
}

/// `r` is optional on rows and cells (ECMA-376 §18.3.1.4, §18.3.1.73): the
/// next row or column along. Inline-string writers leave it out.
#[test]
fn xlsx_rows_and_cells_without_references() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("no-refs.xlsx");
    xlsx(
        &path,
        concat!(
            r#"<row><c t="inlineStr"><is><t>a</t></is></c><c><v>2</v></c></row>"#,
            r#"<row r="3"><c r="B3"><v>3</v></c><c><v>4</v></c></row>"#,
            r#"<row><c><v>5</v></c></row>"#,
        ),
        None,
    );
    round_trip(
        &path,
        &[
            (0, 0, "a"),
            (0, 1, "2"),
            (1, 0, ""),
            (2, 1, "3"),
            (2, 2, "4"),
            (3, 0, "5"),
        ],
    );
}

/// Strict Open XML isn't read yet: say so instead of a parser error.
#[test]
fn strict_xlsx_is_named() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("strict.xlsx");
    let rels = ROOT_RELS.replace(
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument",
        "http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument",
    );
    write_zip(
        &path,
        &[
            ("[Content_Types].xml", CT.as_bytes()),
            ("_rels/.rels", rels.as_bytes()),
        ],
    );
    let err = Workbook::open(&path).err().expect("refused").to_string();
    assert!(err.contains("Strict Open XML"), "{err}");
}

/// An `.xlsb` whose sheet names a relationship that isn't there panicked in
/// calamine; the file is refused instead, and Kantor keeps running.
#[test]
fn xlsb_reader_panic_is_an_error() {
    fn record(out: &mut Vec<u8>, ty: u32, data: &[u8]) {
        for v in [ty, data.len() as u32] {
            let mut v = v;
            loop {
                let b = (v & 0x7f) as u8;
                v >>= 7;
                if v == 0 {
                    out.push(b);
                    break;
                }
                out.push(b | 0x80);
            }
        }
        out.extend_from_slice(data);
    }
    fn wide(out: &mut Vec<u8>, s: &str) {
        let units: Vec<u16> = s.encode_utf16().collect();
        out.extend_from_slice(&(units.len() as u32).to_le_bytes());
        for u in units {
            out.extend_from_slice(&u.to_le_bytes());
        }
    }
    let mut sheet = Vec::new();
    sheet.extend_from_slice(&0u32.to_le_bytes()); // visible
    sheet.extend_from_slice(&1u32.to_le_bytes()); // tab id
    wide(&mut sheet, "rId9"); // not in the relationships
    wide(&mut sheet, "Sheet1");
    let mut bin = Vec::new();
    record(&mut bin, 0x0083, &[]); // BrtBeginBook
    record(&mut bin, 0x008F, &[]); // BrtBeginBundleShs
    record(&mut bin, 0x009C, &sheet); // BrtBundleSh
    record(&mut bin, 0x0090, &[]); // BrtEndBundleShs
    record(&mut bin, 0x0084, &[]); // BrtEndBook
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.bin"/></Relationships>"#;
    let root = ROOT_RELS.replace("workbook.xml", "workbook.bin");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken.xlsb");
    write_zip(
        &path,
        &[
            ("[Content_Types].xml", CT.as_bytes()),
            ("_rels/.rels", root.as_bytes()),
            ("xl/_rels/workbook.bin.rels", rels.as_bytes()),
            ("xl/workbook.bin", &bin),
        ],
    );
    // Opening must return (an error or, if calamine copes one day, a book).
    let _ = Workbook::open(&path);
}

/// LibreOffice ends each row with one empty cell repeated to the last
/// column, taking the column's default cell style. Those were styled one by
/// one (thousands of cells a row): a 3,000-row file took two minutes.
#[test]
fn ods_column_default_runs_open_quickly() {
    let ns = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" office:version="1.3""#;
    let styles = r##"<style:style style:name="ce1" style:family="table-cell"><style:table-cell-properties fo:background-color="#ddebf7"/></style:style>"##;
    let mut rows = String::new();
    for r in 1..=3000 {
        rows.push_str(&format!(
            r#"<table:table-row><table:table-cell office:value-type="float" office:value="{r}"><text:p>{r}</text:p></table:table-cell><table:table-cell table:number-columns-repeated="16383"/></table:table-row>"#
        ));
    }
    let content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {ns}><office:automatic-styles>{styles}</office:automatic-styles><office:body><office:spreadsheet><table:table table:name="One"><table:table-column table:number-columns-repeated="16384" table:default-cell-style-name="ce1"/>{rows}<table:table-row table:number-rows-repeated="1045576"><table:table-cell table:number-columns-repeated="16384"/></table:table-row></table:table></office:spreadsheet></office:body></office:document-content>"#
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runs.ods");
    write_zip(
        &path,
        &[
            (
                "mimetype",
                b"application/vnd.oasis.opendocument.spreadsheet",
            ),
            ("META-INF/manifest.xml", ODS_MANIFEST.as_bytes()),
            ("content.xml", content.as_bytes()),
        ],
    );
    let start = Instant::now();
    let mut wb = Workbook::open(&path).unwrap();
    // Was ~2 minutes in a release build; generous for debug builds and CI.
    assert!(
        start.elapsed() < Duration::from_secs(30),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(wb.input(0, 2999, 0), "3000");
    // The empty cells still show the column's fill, through the column.
    let fill = |wb: &Workbook, r, c| wb.cell_style(0, r, c).fill;
    assert_eq!(fill(&wb, 5, 7), fill(&wb, 0, 0));
    assert!(fill(&wb, 5, 7).is_some());
    let out = dir.path().join("runs.xlsx");
    wb.save_as(&out).unwrap();
    assert_eq!(Workbook::open(&out).unwrap().input(0, 2999, 0), "3000");
}

/// A one-or-more-table `.ods` with this `content.xml` body.
fn write_ods(path: &Path, spreadsheet: &str) {
    let ns = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" office:version="1.3""#;
    let content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {ns}><office:body><office:spreadsheet>{spreadsheet}</office:spreadsheet></office:body></office:document-content>"#
    );
    write_zip(
        path,
        &[
            (
                "mimetype",
                b"application/vnd.oasis.opendocument.spreadsheet",
            ),
            ("META-INF/manifest.xml", ODS_MANIFEST.as_bytes()),
            ("content.xml", content.as_bytes()),
        ],
    );
}

/// LibreOffice names a sheet linked from another file after its URL
/// (`'file:///…/Report.xlsm'#Cover`), which Excel doesn't allow: the whole
/// file was refused. The sheet is renamed, with an import note.
#[test]
fn ods_sheet_names_excel_does_not_allow_are_renamed() {
    let cell = |v: &str| {
        format!(
            r#"<table:table-row><table:table-cell office:value-type="string"><text:p>{v}</text:p></table:table-cell></table:table-row>"#
        )
    };
    let body = format!(
        r#"<table:table table:name="Data">{}</table:table><table:table table:name="'file:///G:/Reports/New%20Report.xlsm'#Cover_Sheet">{}</table:table>"#,
        cell("one"),
        cell("two")
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("links.ods");
    write_ods(&path, &body);
    let wb = Workbook::open(&path).unwrap();
    let names = wb.sheet_names();
    assert_eq!(names[0], "Data");
    assert!(names[1].chars().count() <= 31, "{names:?}");
    assert!(!names[1].contains(['/', ':']), "{names:?}");
    assert_eq!(wb.input(1, 0, 0), "two");
    let notes: Vec<String> = wb.import_notes();
    assert!(
        notes.iter().any(|n| n.contains("1 sheet renamed")),
        "{notes:?}"
    );
}

/// Whole-row formatting from LibreOffice (a styled empty run to the last
/// column) becomes the rows' style, not 16,000 styled cells per row.
#[test]
fn ods_styled_runs_to_the_last_column_are_row_styles() {
    let ns = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" office:version="1.3""#;
    let styles = r##"<style:style style:name="ce1" style:family="table-cell"><style:table-cell-properties fo:background-color="#fff2cc"/></style:style>"##;
    let mut rows = String::new();
    for r in 1..=2000 {
        rows.push_str(&format!(
            r#"<table:table-row><table:table-cell table:style-name="ce1" office:value-type="float" office:value="{r}"><text:p>{r}</text:p></table:table-cell><table:table-cell table:number-columns-repeated="16383" table:style-name="ce1"/></table:table-row>"#
        ));
    }
    rows.push_str(r#"<table:table-row table:number-rows-repeated="3"><table:table-cell table:number-columns-repeated="16384" table:style-name="ce1"/></table:table-row>"#);
    let content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {ns}><office:automatic-styles>{styles}</office:automatic-styles><office:body><office:spreadsheet><table:table table:name="One"><table:table-column table:number-columns-repeated="16384"/>{rows}</table:table></office:spreadsheet></office:body></office:document-content>"#
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rows.ods");
    write_zip(
        &path,
        &[
            (
                "mimetype",
                b"application/vnd.oasis.opendocument.spreadsheet",
            ),
            ("META-INF/manifest.xml", ODS_MANIFEST.as_bytes()),
            ("content.xml", content.as_bytes()),
        ],
    );
    let start = Instant::now();
    let mut wb = Workbook::open(&path).unwrap();
    assert!(
        start.elapsed() < Duration::from_secs(30),
        "{:?}",
        start.elapsed()
    );
    let fill = wb.cell_style(0, 0, 0).fill;
    assert!(fill.is_some());
    for (r, c) in [(0, 1), (0, 900), (1999, 16000), (2001, 5)] {
        assert_eq!(wb.cell_style(0, r, c).fill, fill, "R{}C{}", r + 1, c + 1);
    }
    // Rows past the formatted ones stay plain.
    assert_eq!(wb.cell_style(0, 2100, 0).fill, None);
    let out = dir.path().join("rows.xlsx");
    wb.save_as(&out).unwrap();
    let back = Workbook::open(&out).unwrap();
    assert_eq!(back.cell_style(0, 1500, 3000).fill, fill);
}
