//! Excel 2010 extras of conditional formatting (x14): a data bar's lengths,
//! border and axis are drawn, and a sheet whose rules are edited in Kantor
//! keeps them, and its custom icon sets, in the saved file.

#[path = "support/pivot_check.rs"]
mod pivot_check;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use pivot_check::parts;
use sheets_engine::{CfFormat, CfRuleDef, CfSpec, Workbook};

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cf_x14");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn plain(spec: CfSpec) -> CfRuleDef {
    CfRuleDef {
        num_fmt: None,
        border: None,
        spec,
        format: CfFormat::default(),
        stop_if_true: false,
    }
}

/// `path` with its first sheet's XML changed by `edit`, written to `to`.
fn patch_sheet(path: &Path, to: &Path, edit: impl Fn(String) -> String) {
    let mut zin = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut zout = zip::ZipWriter::new(std::fs::File::create(to).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    for i in 0..zin.len() {
        let mut f = zin.by_index(i).unwrap();
        let name = f.name().unwrap().to_string();
        let mut data = Vec::new();
        f.read_to_end(&mut data).unwrap();
        if name == "xl/worksheets/sheet1.xml" {
            data = edit(String::from_utf8(data).unwrap()).into_bytes();
        }
        zout.start_file(&name, opts).unwrap();
        zout.write_all(&data).unwrap();
    }
    zout.finish().unwrap();
}

const CUSTOM_ICONS: &str = r#"<x14:conditionalFormatting xmlns:xm="http://schemas.microsoft.com/office/excel/2006/main"><x14:cfRule type="iconSet" priority="7" id="{00000000-0000-0000-0000-0000000000AA}"><x14:iconSet iconSet="3Flags" custom="1"><x14:cfvo type="percent"><xm:f>0</xm:f></x14:cfvo><x14:cfvo type="percent"><xm:f>33</xm:f></x14:cfvo><x14:cfvo type="percent"><xm:f>67</xm:f></x14:cfvo><x14:cfIcon iconSet="3Symbols" iconId="0"/><x14:cfIcon iconSet="3Flags" iconId="1"/><x14:cfIcon iconSet="3Stars" iconId="2"/></x14:iconSet></x14:cfRule><xm:sqref>B1:B10</xm:sqref></x14:conditionalFormatting>"#;

/// A file whose data bar on A1:A10 has 10-80 % lengths, a green border and
/// no axis, and whose B1:B10 has a custom icon set.
fn fixture(tag: &str) -> PathBuf {
    let mut wb = Workbook::new_empty().unwrap();
    let cells: Vec<(u32, u32, String)> = (0..10u32)
        .flat_map(|i| [(i, 0, (i + 1).to_string()), (i, 1, (i * 10).to_string())])
        .collect();
    wb.set_inputs(0, &cells).unwrap();
    wb.add_cf_rule(
        0,
        "A1:A10",
        &plain(CfSpec::DataBar {
            color: 0x638EC6,
            gradient: false,
        }),
    )
    .unwrap();
    let kantor = scratch(&format!("kantor-{tag}.xlsx"));
    wb.save_as(&kantor).unwrap();
    let file = scratch(&format!("excel2010-{tag}.xlsx"));
    patch_sheet(&kantor, &file, |x| {
        let x = x.replacen(
            r#"<x14:dataBar minLength="0" maxLength="100" gradient="0">"#,
            r#"<x14:dataBar minLength="10" maxLength="80" gradient="0" border="1" axisPosition="none">"#,
            1,
        );
        let x = x.replacen(
            "<x14:negativeFillColor",
            r#"<x14:borderColor rgb="FF00B050"/><x14:negativeFillColor"#,
            1,
        );
        assert!(x.contains(r#"border="1""#), "{x}");
        x.replacen(
            "</x14:conditionalFormattings>",
            &format!("{CUSTOM_ICONS}</x14:conditionalFormattings>"),
            1,
        )
    });
    file
}

#[test]
fn data_bar_look_from_the_file() {
    let mut wb = Workbook::open(fixture("look")).unwrap();
    let lowest = wb.conditional(0, 0, 0).unwrap().bar.unwrap();
    let highest = wb.conditional(0, 9, 0).unwrap().bar.unwrap();
    // 1 of 0..10 (the automatic minimum is 0): 10 % + 10 % of 70 %.
    assert!((lowest.to - 0.17).abs() < 1e-4, "{lowest:?}");
    assert!((highest.to - 0.8).abs() < 1e-4, "{highest:?}");
    assert_eq!(highest.border, Some(0x00B050));
    // The custom icon set shows its icons.
    assert!(wb.conditional(0, 9, 1).unwrap().icon.is_some());
}

#[test]
fn edited_sheet_keeps_excel_2010_extras() {
    let mut wb = Workbook::open(fixture("edited")).unwrap();
    // Another rule on the same sheet: its rules are now saved from the model.
    wb.add_cf_rule(
        0,
        "C1:C10",
        &plain(CfSpec::DataBar {
            color: 0xFF0000,
            gradient: true,
        }),
    )
    .unwrap();
    let path = scratch("edited.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let sheet = &parts(&path)["xl/worksheets/sheet1.xml"];
    assert!(
        sheet.contains(r#"minLength="10" maxLength="80""#),
        "{sheet}"
    );
    assert!(
        sheet.contains(r#"border="1""#) && sheet.contains("FF00B050"),
        "{sheet}"
    );
    assert!(sheet.contains(r#"axisPosition="none""#), "{sheet}");
    assert!(sheet.contains(r#"custom="1""#) && sheet.contains("<xm:sqref>B1:B10</xm:sqref>"));
    assert!(sheet.contains("C1:C10"), "the new rule is there");
    assert!(roxmltree::Document::parse(sheet).is_ok());
    // Read back: the look and the icon set are still there.
    let mut back = Workbook::open(&path).unwrap();
    assert_eq!(
        back.conditional(0, 9, 0).unwrap().bar.unwrap().border,
        Some(0x00B050)
    );
    assert!(back.conditional(0, 9, 1).unwrap().icon.is_some());
    // Undone, saved again: still kept.
    wb.undo().unwrap();
    let undone = scratch("undone.xlsx");
    wb.save_as(&undone).unwrap();
    let sheet = &parts(&undone)["xl/worksheets/sheet1.xml"];
    assert!(
        sheet.contains(r#"border="1""#) && sheet.contains(r#"custom="1""#),
        "{sheet}"
    );
}

#[test]
fn a_changed_bar_drops_the_file_look() {
    let mut wb = Workbook::open(fixture("changed")).unwrap();
    let rules = wb.cf_rules(0, None);
    let bar = rules
        .iter()
        .find(|r| r.range == "A1:A10")
        .expect("the data bar");
    let mut def = bar.def.clone().expect("editable");
    def.spec = CfSpec::DataBar {
        color: 0x00FF00,
        gradient: false,
    };
    wb.update_cf_rule(0, bar.index, "A1:A10", &def).unwrap();
    let b = wb.conditional(0, 9, 0).unwrap().bar.unwrap();
    assert_eq!((b.to, b.border), (1.0, None), "Kantor's look");
    let path = scratch("changed.xlsx");
    wb.save_as(&path).unwrap();
    let sheet = &parts(&path)["xl/worksheets/sheet1.xml"];
    assert!(!sheet.contains("FF00B050"), "{sheet}");
}
