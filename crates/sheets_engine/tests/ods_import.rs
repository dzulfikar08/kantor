//! Notes, data validation and conditional formats of OpenDocument imports
//! (`.ods`): the LibreOffice conversion `scripts/corpus/gen_xls.py` makes
//! (tests/corpus/xls/features.ods), and small hand-written packages for what
//! LibreOffice doesn't write.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use sheets_engine::{CellRange, ErrorStyle, ValidationOperator, ValidationType, Workbook};

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

fn summaries(wb: &Workbook, sheet: u32) -> Vec<(String, String)> {
    wb.cf_rules(sheet, None)
        .into_iter()
        .map(|r| (r.range, r.summary))
        .collect()
}

#[test]
fn ods_notes_round_trip() {
    let mut wb = Workbook::open(corpus("features.ods")).unwrap();
    assert!(wb.import_notes().is_empty(), "{:?}", wb.import_notes());
    let notes = wb.notes(0);
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert_eq!((notes[0].row, notes[0].col), (1, 0));
    assert_eq!(notes[0].author, "Ann");
    assert_eq!(notes[0].text, "Bought weekly");
    assert_eq!((notes[1].row, notes[1].col), (2, 2));
    assert_eq!(notes[1].author, "Budi");
    assert_eq!(notes[1].text, "Price went up\nin February");
    // The note's text is not the cell's.
    assert_eq!(wb.display_value(0, 1, 0), "Rice");
    assert_eq!(wb.display_value(0, 2, 2), "99");
    let other = wb.notes(1);
    assert_eq!(other.len(), 1);
    assert_eq!((other[0].row, other[0].col), (1, 1));
    assert_eq!(other[0].text, "On the second sheet");
    assert!(wb.note_at(0, 1, 0).is_some());

    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("notes.xlsx");
    wb.save_as(&out).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let bytes = std::fs::read(&out).unwrap();
    assert!(sheets_engine::package_problems(&bytes).is_empty());
    let c1 = part(&out, "xl/comments1.xml");
    assert!(c1.contains("ref=\"A2\""), "{c1}");
    assert!(c1.contains("Price went up\nin February"), "{c1}");
    assert!(c1.contains("<author>Budi</author>"), "{c1}");

    let back = Workbook::open(&out).unwrap();
    let texts: Vec<String> = back.notes(0).into_iter().map(|n| n.text).collect();
    assert_eq!(texts, ["Bought weekly", "Price went up\nin February"]);
    assert_eq!(back.notes(1).len(), 1);
}

#[test]
fn ods_data_validation_round_trip() {
    let mut wb = Workbook::open(corpus("features.ods")).unwrap();
    let list = wb.sheet_validations(0);
    assert_eq!(list.len(), 7, "{list:?}");
    let at = |col: u32| list.iter().find(|v| v.ranges[0].left == col).unwrap();
    let f = at(5);
    assert_eq!(f.kind, ValidationType::List);
    assert_eq!(f.formula1.as_deref(), Some("\"Open,Closed,Late\""));
    assert_eq!(f.ranges, vec![CellRange::from_corners((1, 5), (4, 5))]);
    assert!(f.dropdown);
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
    assert_eq!(
        (error.title.as_str(), error.text.as_str()),
        ("Too many", "Use 0 to 100")
    );
    let c = at(2);
    assert_eq!(
        (c.kind, c.operator),
        (ValidationType::Decimal, ValidationOperator::GreaterThan)
    );
    assert_eq!(c.error.as_ref().unwrap().0, ErrorStyle::Warning);
    let d = at(3);
    assert_eq!(d.kind, ValidationType::Date);
    assert_eq!(d.formula1.as_deref(), Some("DATE(2024,1,1)"));
    assert_eq!(d.formula2.as_deref(), Some("DATE(2024,12,31)"));
    let e = at(4);
    assert_eq!(
        (e.kind, e.operator, e.formula1.as_deref()),
        (
            ValidationType::TextLength,
            ValidationOperator::Equal,
            Some("4")
        )
    );
    let i = at(8);
    assert_eq!(i.kind, ValidationType::Custom);
    assert_eq!(i.formula1.as_deref(), Some("ISNUMBER(I1)"));

    // A whole column: one range, an information alert, blanks allowed.
    let scales = wb.sheet_validations(2);
    assert_eq!(scales.len(), 1);
    let h = &scales[0];
    assert_eq!(
        h.ranges,
        vec![CellRange::from_corners((0, 7), (1_048_575, 7))]
    );
    assert_eq!(
        (h.operator, h.formula1.as_deref()),
        (ValidationOperator::GreaterThanOrEqual, Some("1"))
    );
    assert!(h.allow_blank);
    assert_eq!(h.error.as_ref().unwrap().0, ErrorStyle::Information);

    // The rules work.
    let items = wb.validation_list(0, 2, 5).unwrap();
    assert_eq!(
        items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(),
        ["Open", "Closed", "Late"]
    );
    assert_eq!(wb.validation_list(0, 2, 6).unwrap().len(), 3);
    assert!(wb.invalid_cell(0, 3, 1));
    assert!(!wb.invalid_cell(0, 1, 1));

    // Saved as .xlsx with a row inserted above: written, moved, reopened.
    wb.insert_rows(0, 0, 1).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("dv.xlsx");
    wb.save_as(&out).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let sheet = part(&out, "xl/worksheets/sheet1.xml");
    assert!(sheet.contains("<dataValidations count=\"7\">"), "{sheet}");
    assert!(sheet.contains("sqref=\"F3:F6\""), "{sheet}");
    assert!(part(&out, "xl/worksheets/sheet3.xml").contains("sqref=\"H1:H1048576\""));
    let back = Workbook::open(&out).unwrap();
    assert_eq!(back.sheet_validations(0).len(), 7);
    assert_eq!(back.sheet_validations(2).len(), 1);
}

#[test]
fn ods_conditional_formats_round_trip() {
    let mut wb = Workbook::open(corpus("features.ods")).unwrap();
    // LibreOffice's list, not the cell styles' copies of it: four rules,
    // in file order.
    let rules = wb.cf_rules(0, None);
    let ranges: Vec<&str> = rules.iter().map(|r| r.range.as_str()).collect();
    assert_eq!(ranges, ["B2:B5", "C2:C5", "F2:F5", "A2:A5"]);
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

    let fill = |wb: &mut Workbook, s, r, c| {
        wb.conditional(s, r, c)
            .and_then(|x| x.style.as_ref().and_then(|s| s.fill))
    };
    assert_eq!(fill(&mut wb, 0, 1, 1), Some(0xC6EFCE));
    assert_eq!(fill(&mut wb, 0, 2, 1), None);
    assert_eq!(fill(&mut wb, 0, 4, 5), Some(0xFFC7CE));
    assert!(
        wb.conditional(0, 4, 0)
            .and_then(|x| x.style.clone())
            .unwrap()
            .underline
    );

    // Colour scales, a data bar, icon sets and the other kinds.
    let scales = summaries(&wb, 2);
    let kinds: Vec<&str> = scales.iter().map(|(r, _)| r.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "A1:A10", "B1:B10", "C1:C10", "D1:D10", "E1:E10", "F1:F10", "F1:F10", "F1:F10",
            "G1:G10", "H1:H10", "I1:I3"
        ],
        "{scales:?}"
    );
    let rules = wb.cf_rules(2, None);
    // A1 is the lowest of the two-colour scale, A10 the highest.
    let a1 = wb.conditional(2, 0, 0).unwrap();
    assert_eq!(a1.style.as_ref().unwrap().fill, Some(0xF8696B));
    let a10 = wb.conditional(2, 9, 0).unwrap();
    assert_eq!(a10.style.as_ref().unwrap().fill, Some(0x63BE7B));
    // C10 has the longest bar.
    let bar = |wb: &mut Workbook, r| wb.conditional(2, r, 2).unwrap().bar.unwrap();
    let width = |b: sheets_engine::DataBar| b.to - b.from;
    assert!(width(bar(&mut wb, 9)) > width(bar(&mut wb, 1)));
    assert!(wb.conditional(2, 9, 3).unwrap().icon.is_some());
    // The rating hides the value.
    let e = wb.conditional(2, 9, 4).unwrap();
    assert!(!e.show_value);
    // "ap" in apple and apricot; "banana" begins with b; apple twice.
    let f1 = wb.conditional(2, 0, 5).unwrap().style.clone().unwrap();
    assert_eq!(f1.fill, Some(0xFFEB9C));
    assert_eq!(f1.font_color, Some(0x9C0006));
    assert!(wb.conditional(2, 1, 5).unwrap().style.clone().unwrap().bold);
    assert!(
        wb.conditional(2, 6, 5)
            .and_then(|x| x.style.clone())
            .is_none()
    );
    // Top 3 of G (80, 90, 100); above the average of H (6..10).
    assert!(
        wb.conditional(2, 9, 6)
            .unwrap()
            .style
            .clone()
            .unwrap()
            .italic
    );
    assert!(
        wb.conditional(2, 6, 6)
            .and_then(|x| x.style.clone())
            .is_none()
    );
    let h10 = wb.conditional(2, 9, 7).unwrap().style.clone().unwrap();
    assert_eq!(h10.font_color, Some(0x0000FF));
    assert!(
        rules[10].summary.to_lowercase().contains("today"),
        "{:?}",
        rules[10].summary
    );

    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("cf.xlsx");
    wb.save_as(&out).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let bytes = std::fs::read(&out).unwrap();
    assert!(sheets_engine::package_problems(&bytes).is_empty());
    let sheet = part(&out, "xl/worksheets/sheet1.xml");
    assert_eq!(
        sheet.matches("<conditionalFormatting").count(),
        4,
        "{sheet}"
    );
    let sheet3 = part(&out, "xl/worksheets/sheet3.xml");
    for kind in [
        "colorScale",
        "dataBar",
        "iconSet",
        "top10",
        "aboveAverage",
        "duplicateValues",
    ] {
        assert!(sheet3.contains(kind), "{kind}: {sheet3}");
    }
    let back = Workbook::open(&out).unwrap();
    assert_eq!(summaries(&back, 0), summaries(&wb, 0));
    assert_eq!(summaries(&back, 2), summaries(&wb, 2));
}

const NS: &str = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:of="urn:oasis:names:tc:opendocument:xmlns:of:1.2" xmlns:calcext="urn:org:documentfoundation:names:experimental:calc:xmlns:calcext:1.0" office:version="1.3""#;

/// A one-table `.ods` with `body` as its rows, `validations` before them
/// and `styles` as the automatic styles.
fn write_ods(path: &Path, styles: &str, validations: &str, body: &str) {
    let content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {NS}><office:automatic-styles>{styles}</office:automatic-styles><office:body><office:spreadsheet>{validations}<table:table table:name="One"><table:table-column table:number-columns-repeated="3"/>{body}</table:table></office:spreadsheet></office:body></office:document-content>"#
    );
    let manifest = r#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/></manifest:manifest>"#;
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.oasis.opendocument.spreadsheet")
        .unwrap();
    zip.start_file("META-INF/manifest.xml", stored).unwrap();
    zip.write_all(manifest.as_bytes()).unwrap();
    zip.start_file("content.xml", stored).unwrap();
    zip.write_all(content.as_bytes()).unwrap();
    zip.finish().unwrap();
}

fn num(v: i32) -> String {
    format!(
        r#"<table:table-cell office:value-type="float" office:value="{v}"><text:p>{v}</text:p></table:table-cell>"#
    )
}

/// Files from other programs carry conditions only as `style:map`s on the
/// cells' styles; conditions with no Excel counterpart are left out with a
/// note, and the file still opens.
#[test]
fn ods_style_maps_and_odd_conditions() {
    let styles = r##"<style:style style:name="Hot" style:family="table-cell"><style:text-properties fo:font-weight="bold" fo:color="#c00000"/></style:style>
<style:style style:name="Cool" style:family="table-cell"><style:table-cell-properties fo:background-color="#ddebf7"/></style:style>
<style:style style:name="ce1" style:family="table-cell"><style:map style:condition="cell-content()&gt;=5" style:apply-style-name="Hot" style:base-cell-address="One.A1"/><style:map style:condition="is-true-formula([.A1]=[.B1])" style:apply-style-name="Cool" style:base-cell-address="One.A1"/></style:style>"##;
    let validations = r#"<table:content-validations><table:content-validation table:name="v1" table:condition="of:cell-content-is-whole-number() and cell-content-is-not-between(1;[.$C$1])" table:allow-empty-cell="true" table:base-cell-address="One.B1"/><table:content-validation table:name="v2" table:condition="of:cell-content-is-in-list(&quot;a,b&quot;;&quot;c&quot;)" table:base-cell-address="One.C2"/><table:content-validation table:name="v3" table:condition="of:cell-content-text-length-is-between(2,5)" table:display-list="none" table:base-cell-address="One.C3"><table:help-message table:title="Short"><text:p>Two to <text:span>five</text:span></text:p><text:p>characters</text:p></table:help-message></table:content-validation></table:content-validations>"#;
    let row = |a: i32, b: i32, extra: &str| {
        format!(
            r#"<table:table-row><table:table-cell table:style-name="ce1" office:value-type="float" office:value="{a}"><text:p>{a}</text:p></table:table-cell><table:table-cell table:content-validation-name="v1" office:value-type="float" office:value="{b}"><text:p>{b}</text:p></table:table-cell>{extra}</table:table-row>"#
        )
    };
    let note = r#"<table:table-cell table:content-validation-name="v3" office:value-type="string"><office:annotation><dc:creator> Siti </dc:creator><dc:date>2024-01-01T00:00:00</dc:date><text:p>First<text:s text:c="2"/>line<text:line-break/>second</text:p><text:p><text:span>third</text:span></text:p></office:annotation><text:p>xy</text:p></table:table-cell>"#;
    let body = [
        row(7, 3, &num(9)),
        row(
            2,
            2,
            r#"<table:table-cell table:content-validation-name="v2"/>"#,
        ),
        row(5, 1, note),
    ]
    .concat();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("maps.ods");
    write_ods(&path, styles, validations, &body);

    let mut wb = Workbook::open(&path).unwrap();
    // The list item with a comma can't be an Excel list.
    assert_eq!(
        wb.import_notes(),
        ["1 data validation rule not imported (condition not supported)"]
    );
    assert_eq!(
        summaries(&wb, 0),
        [
            (
                "A1:A3".to_owned(),
                "Cell value greater than or equal to 5".to_owned()
            ),
            ("A1:A3".to_owned(), "Formula: =A1=B1".to_owned()),
        ]
    );
    let a1 = wb.conditional(0, 0, 0).unwrap().style.clone().unwrap();
    assert!(a1.bold);
    assert_eq!(a1.font_color, Some(0xC00000));
    assert_eq!(a1.fill, None);
    let a2 = wb.conditional(0, 1, 0).unwrap().style.clone().unwrap();
    assert_eq!(a2.fill, Some(0xDDEBF7));
    assert!(!a2.bold);

    let list = wb.sheet_validations(0);
    assert_eq!(list.len(), 2, "{list:?}");
    let v1 = &list[0];
    assert_eq!(
        (v1.kind, v1.operator),
        (ValidationType::Whole, ValidationOperator::NotBetween)
    );
    assert_eq!(
        (v1.formula1.as_deref(), v1.formula2.as_deref()),
        (Some("1"), Some("$C$1"))
    );
    assert_eq!(v1.ranges, vec![CellRange::from_corners((0, 1), (2, 1))]);
    assert!(v1.allow_blank);
    assert!(v1.error.is_none());
    let v3 = &list[1];
    assert_eq!(
        (v3.kind, v3.operator),
        (ValidationType::TextLength, ValidationOperator::Between)
    );
    assert!(!v3.dropdown);
    let p = v3.prompt.as_ref().unwrap();
    assert_eq!(
        (p.title.as_str(), p.text.as_str()),
        ("Short", "Two to five\ncharacters")
    );

    let notes = wb.notes(0);
    assert_eq!(notes.len(), 1);
    assert_eq!((notes[0].row, notes[0].col), (2, 2));
    assert_eq!(notes[0].author, "Siti");
    assert_eq!(notes[0].text, "First  line\nsecond\nthird");
    assert_eq!(wb.display_value(0, 2, 2), "xy");

    let out = dir.path().join("maps.xlsx");
    wb.save_as(&out).unwrap();
    let back = Workbook::open(&out).unwrap();
    assert_eq!(summaries(&back, 0), summaries(&wb, 0));
    assert_eq!(back.sheet_validations(0).len(), 2);
    assert_eq!(back.notes(0)[0].text, "First  line\nsecond\nthird");
}

/// A formula rule relative to a base cell other than its range's top-left
/// is moved to read the same cells; unknown kinds are left out.
#[test]
fn ods_rules_rebased_and_unknown_kinds_skipped() {
    let styles = r##"<style:style style:name="Mark" style:family="table-cell"><style:table-cell-properties fo:background-color="#ffeb9c"/></style:style>"##;
    let rows: String = (1..=3)
        .map(|v| {
            format!(
                "<table:table-row>{}{}</table:table-row>",
                num(v),
                num(v * 2)
            )
        })
        .collect();
    let cf = r#"<calcext:conditional-formats><calcext:conditional-format calcext:target-range-address="One.B2:One.B3"><calcext:condition calcext:apply-style-name="Mark" calcext:value="formula-is([.A1]&gt;1)" calcext:base-cell-address="One.B1"/><calcext:condition calcext:apply-style-name="Mark" calcext:value="above-equal-average" calcext:base-cell-address="One.B2"/><calcext:condition calcext:apply-style-name="Mark" calcext:value="begins-with([.A1])" calcext:base-cell-address="One.B2"/></calcext:conditional-format></calcext:conditional-formats>"#;
    let body = format!("{rows}{cf}");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rebase.ods");
    write_ods(&path, styles, "", &body);
    let mut wb = Workbook::open(&path).unwrap();
    assert_eq!(
        wb.import_notes(),
        ["2 conditional formatting rules not imported (condition not supported)"]
    );
    // Written for B1 reading A1, so for B2 it reads A2.
    assert_eq!(
        summaries(&wb, 0),
        [("B2:B3".to_owned(), "Formula: =A2>1".to_owned())]
    );
    assert!(wb.conditional(0, 1, 1).unwrap().style.is_some());
}
