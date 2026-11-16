//! A workbook shaped like the one from the Nextcloud demo (a title in A1
//! over empty, styled `<c t="n"></c>` placeholders in B1:F1, `#,##0`
//! amounts): the title's neighbours count as empty for text overflow, and
//! with the Indonesian display locale the amounts show `109.600.000`
//! while the saved file keeps Excel's invariant format codes. Its own test
//! binary: the display locale is set once per process.

use std::io::Write as _;

use sheets_engine::Workbook;

const NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn demo_xlsx() -> Vec<u8> {
    let sheet = format!(
        "<worksheet xmlns=\"{NS}\"><sheetData>\
<row r=\"1\"><c r=\"A1\" s=\"1\" t=\"inlineStr\"><is><t>RENCANA ANGGARAN KEGIATAN TAHUN 2027</t></is></c>\
<c r=\"B1\" s=\"1\" t=\"n\"></c><c r=\"C1\" s=\"1\" t=\"n\"></c><c r=\"D1\" s=\"1\" t=\"n\"/></row>\
<row r=\"2\"><c r=\"A2\" t=\"n\"><v>109600000</v></c><c r=\"B2\" s=\"2\"><v>109600000</v></c>\
<c r=\"C2\" s=\"3\"><v>1234.5</v></c><c r=\"D2\"><f>B2</f><v></v></c></row>\
</sheetData></worksheet>"
    );
    let styles = format!(
        "<styleSheet xmlns=\"{NS}\"><numFmts count=\"1\"><numFmt numFmtId=\"164\" formatCode=\"#,##0.00\"/></numFmts>\
<fonts count=\"2\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font><font><b/><sz val=\"14\"/><name val=\"Calibri\"/></font></fonts>\
<fills count=\"2\"><fill><patternFill patternType=\"none\"/></fill><fill><patternFill patternType=\"gray125\"/></fill></fills>\
<borders count=\"1\"><border><left/><right/><top/><bottom/><diagonal/></border></borders>\
<cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>\
<cellXfs count=\"4\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>\
<xf numFmtId=\"0\" fontId=\"1\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>\
<xf numFmtId=\"3\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>\
<xf numFmtId=\"164\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/></cellXfs></styleSheet>"
    );
    let parts = [
        (
            "[Content_Types].xml",
            "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
<Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>\
<Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>\
</Types>"
                .to_owned(),
        ),
        (
            "_rels/.rels",
            format!("<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"{R}/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>"),
        ),
        (
            "xl/workbook.xml",
            format!("<workbook xmlns=\"{NS}\" xmlns:r=\"{R}\"><sheets><sheet name=\"Anggaran\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>"),
        ),
        (
            "xl/_rels/workbook.xml.rels",
            format!("<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"{R}/worksheet\" Target=\"worksheets/sheet1.xml\"/><Relationship Id=\"rId2\" Type=\"{R}/styles\" Target=\"styles.xml\"/></Relationships>"),
        ),
        ("xl/styles.xml", styles),
        ("xl/worksheets/sheet1.xml", sheet),
    ];
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    for (name, data) in parts {
        zip.start_file(name, opts).unwrap();
        zip.write_all(data.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[test]
fn demo_workbook_overflow_and_indonesian_numbers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Anggaran Kegiatan 2027.xlsx");
    std::fs::write(&path, demo_xlsx()).unwrap();
    let mut book = Workbook::open(&path).unwrap();

    // Overflow: only A1 shows something in row 1; the styled `t="n"`
    // placeholders are empty, so the title may spill over them.
    assert_eq!(book.content_columns_in_row(0, 0), vec![0]);
    assert_eq!(book.display_value(0, 0, 1), "");
    // Typing in a neighbour makes it count (and stops the spill).
    book.set_input(0, 0, 2, "x").unwrap();
    assert_eq!(book.content_columns_in_row(0, 0), vec![0, 2]);
    book.set_input(0, 0, 2, "").unwrap();
    assert_eq!(book.content_columns_in_row(0, 0), vec![0]);
    // Row 2: numbers and a formula all count.
    book.recalculate();
    assert_eq!(book.content_columns_in_row(0, 1), vec![0, 1, 2, 3]);

    // English separators until the app sets the Indonesian locale.
    assert_eq!(book.display_value(0, 1, 1), "109,600,000");
    sheets_engine::set_number_locale(kantor_numfmt::DisplayLocale::Indonesian);
    assert_eq!(book.display_value(0, 1, 1), "109.600.000");
    assert_eq!(book.display_value(0, 1, 2), "1.234,50");
    assert_eq!(book.display_value(0, 1, 0), "109600000");
    // A formula in General: no group separator.
    assert_eq!(book.display_value(0, 1, 3), "109600000");
    // Editing shows the decimal comma; typing it back stores the same value.
    assert_eq!(book.input_for_editing(0, 1, 2), "1234,5");
    let typed = sheets_engine::delocalize_typed("2.500,75");
    book.set_input(0, 1, 2, &typed).unwrap();
    assert_eq!(book.input(0, 1, 2), "2500.75");
    assert_eq!(book.display_value(0, 1, 2), "2.500,75");
    // Agent results are data: en-US.
    assert_eq!(
        sheets_engine::numbers_invariant(|| book.display_value(0, 1, 1)),
        "109,600,000"
    );

    // The file keeps Excel's invariant codes and values.
    let out = dir.path().join("saved.xlsx");
    book.save_as(&out).unwrap();
    let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
    let mut styles = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("xl/styles.xml").unwrap(), &mut styles).unwrap();
    assert!(styles.contains("#,##0.00"), "{styles}");
    assert!(!styles.contains("#.##0"), "{styles}");
    let mut sheet = String::new();
    std::io::Read::read_to_string(
        &mut zip.by_name("xl/worksheets/sheet1.xml").unwrap(),
        &mut sheet,
    )
    .unwrap();
    assert!(sheet.contains("<v>2500.75</v>"), "{sheet}");

    // CSV export is file content: en-US whatever the display locale.
    let csv = dir.path().join("out.csv");
    book.save_as(&csv).unwrap();
    let text = std::fs::read_to_string(&csv).unwrap();
    assert!(text.contains("\"109,600,000\""), "{text}");
}
