//! Custom table styles (`24_table_styles.xlsx`, scripts/corpus/
//! gen_table_styles_xlsx.py): read and drawn, kept byte for byte when
//! untouched, and New / Modify / Duplicate / Delete Table Style with
//! undo, written into styles.xml and read back the same, across saves of
//! one session.

#[path = "support/pivot_check.rs"]
mod pivot_check;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pivot_check::{el, els, parts};
use roxmltree::Document;
use sheets_engine::format::{BorderLine, LineStyle};
use sheets_engine::{CellRange, TableElement, TableElementFormat, TableStyleSpec, Workbook};

const PURPLE: u32 = 0x7030A0;

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("table_styles");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/xlsx")
        .join(name)
}

/// `<tableStyles>` of a saved package, verbatim.
fn table_styles_xml(p: &BTreeMap<String, String>) -> String {
    let s = &p["xl/styles.xml"];
    let a = s.find("<tableStyles").expect("tableStyles");
    let end = s[a..]
        .find("</tableStyles>")
        .map(|e| a + e + "</tableStyles>".len())
        .unwrap_or_else(|| a + s[a..].find("/>").unwrap() + 2);
    s[a..end].to_owned()
}

/// The dxfs of a styles part, as text.
fn dxfs(p: &BTreeMap<String, String>) -> Vec<String> {
    let xml = &p["xl/styles.xml"];
    let doc = Document::parse(xml).unwrap();
    match el(doc.root_element(), "dxfs") {
        Some(d) => els(d, "dxf").map(|n| xml[n.range()].to_owned()).collect(),
        None => Vec::new(),
    }
}

/// The dxf element `kind` of style `name` points to, in a saved package.
fn element_dxf(p: &BTreeMap<String, String>, name: &str, kind: &str) -> Option<String> {
    let xml = &p["xl/styles.xml"];
    let doc = Document::parse(xml).unwrap();
    let ts = el(doc.root_element(), "tableStyles")?;
    let style = els(ts, "tableStyle").find(|s| s.attribute("name") == Some(name))?;
    let id: usize = els(style, "tableStyleElement")
        .find(|e| e.attribute("type") == Some(kind))?
        .attribute("dxfId")?
        .parse()
        .ok()?;
    dxfs(p).get(id).cloned()
}

fn assert_same_except_core(a: &BTreeMap<String, String>, b: &BTreeMap<String, String>) {
    assert_eq!(
        a.keys().collect::<Vec<_>>(),
        b.keys().collect::<Vec<_>>(),
        "parts differ"
    );
    for (name, text) in a {
        // docProps/core.xml holds the save time, which may tick over between the two saves.
        if name == "docProps/core.xml" {
            continue;
        }
        assert_eq!(Some(text), b.get(name), "{name} changed");
    }
}

#[test]
fn file_styles_draw_their_elements() {
    let wb = Workbook::open(corpus("24_table_styles.xlsx")).unwrap();
    assert_eq!(wb.custom_table_styles(), vec!["Brand Purple".to_owned()]);
    assert_eq!(wb.default_table_style(), "Brand Purple");
    let head = wb.table_cell_look(0, 0, 1).unwrap();
    assert_eq!(
        (head.fill, head.font_color, head.bold),
        (Some(PURPLE), Some(0xFFFFFF), true)
    );
    // First row stripe is 2 rows, the second stripe 1: rows 2-3 banded,
    // row 4 not, rows 5-6 banded.
    let band = Some(0xE4DFEC);
    let fills: Vec<_> = (1..=5)
        .map(|r| wb.table_cell_look(0, r, 1).unwrap().fill)
        .collect();
    assert_eq!(fills, vec![band, band, None, band, band]);
    // First column italic; inside lines from the whole table.
    let first = wb.table_cell_look(0, 2, 0).unwrap();
    assert!(first.italic);
    assert_eq!(first.bottom.unwrap().color, Some(PURPLE));
    // Total row: double line above, bold; its last cell red.
    let total = wb.table_cell_look(0, 6, 1).unwrap();
    assert!(total.bold);
    assert_eq!(total.top.unwrap().style, LineStyle::Double);
    assert_eq!(
        wb.table_cell_look(0, 6, 2).unwrap().font_color,
        Some(0xFF0000)
    );
    assert_eq!(wb.table_cell_look(0, 6, 1).unwrap().font_color, None);
}

#[test]
fn untouched_file_styles_are_kept_byte_for_byte() {
    let src = corpus("24_table_styles.xlsx");
    let orig = parts(&src);
    let mut wb = Workbook::open(&src).unwrap();
    // An edit elsewhere (a cell), so the save isn't a no-op.
    wb.set_input(1, 4, 0, "x").unwrap();
    let out = scratch("untouched.xlsx");
    wb.save_as(&out).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let saved = parts(&out);
    assert_eq!(table_styles_xml(&saved), table_styles_xml(&orig));
    for (name, kind) in [
        ("Brand Purple", "wholeTable"),
        ("Brand Purple", "headerRow"),
        ("Brand Purple", "firstRowStripe"),
        ("Brand Purple", "lastTotalCell"),
        ("Pivot Only", "headerRow"),
    ] {
        assert_eq!(
            element_dxf(&saved, name, kind),
            element_dxf(&orig, name, kind),
            "{name} {kind}"
        );
    }
    // The conditional format's dxf stays first.
    assert_eq!(dxfs(&saved)[0], dxfs(&orig)[0]);
    let back = Workbook::open(&out).unwrap();
    assert_eq!(
        back.table_cell_look(0, 0, 1),
        Workbook::open(&src).unwrap().table_cell_look(0, 0, 1)
    );
    // A second save of the same session writes the same.
    let again = scratch("untouched2.xlsx");
    wb.save_as(&again).unwrap();
    assert_same_except_core(&saved, &parts(&again));
}

fn green_spec(name: &str) -> TableStyleSpec {
    let mut spec = TableStyleSpec::new(name);
    spec.set_format(
        TableElement::HeaderRow,
        TableElementFormat {
            fill: Some(0x00B050),
            font: Some(0xFFFFFF),
            bold: Some(true),
            ..Default::default()
        },
    );
    spec.set_format(
        TableElement::WholeTable,
        TableElementFormat {
            bottom: Some(BorderLine {
                style: LineStyle::Medium,
                color: Some(0x00B050),
            }),
            horizontal: Some(BorderLine {
                style: LineStyle::Dashed,
                color: Some(0x00B050),
            }),
            ..Default::default()
        },
    );
    spec.set_format(
        TableElement::FirstRowStripe,
        TableElementFormat {
            fill: Some(0xE2EFDA),
            ..Default::default()
        },
    );
    spec.set_size(TableElement::FirstRowStripe, 3);
    spec
}

/// Region / Units on A1:B9 of a new workbook, as a table.
fn table_book() -> Workbook {
    let mut wb = Workbook::new_empty().unwrap();
    let mut cells = vec![(0, 0, "Region".to_owned()), (0, 1, "Units".to_owned())];
    for r in 1..9 {
        cells.push((r, 0, format!("R{r}")));
        cells.push((r, 1, (r * 3).to_string()));
    }
    wb.set_inputs(0, &cells).unwrap();
    wb.create_table(
        0,
        CellRange::from_corners((0, 0), (8, 1)),
        true,
        "TableStyleMedium2",
    )
    .unwrap();
    wb
}

#[test]
fn new_style_applies_saves_and_reads_back() {
    let mut wb = table_book();
    assert_eq!(wb.next_table_style_name(), "Table Style 1");
    wb.add_table_style(&green_spec("Table Style 1"), true)
        .unwrap();
    assert_eq!(wb.default_table_style(), "Table Style 1");
    assert!(
        wb.add_table_style(&green_spec("table style 1"), false)
            .is_err()
    );
    assert!(
        wb.add_table_style(&green_spec("TableStyleMedium2"), false)
            .is_err()
    );
    let mut o = wb.table_options("Table1").unwrap();
    o.style = Some("Table Style 1".into());
    wb.set_table_options("Table1", o).unwrap();
    let head = wb.table_cell_look(0, 0, 0).unwrap();
    assert_eq!((head.fill, head.bold), (Some(0x00B050), true));
    // Stripes of 3 rows, then 1.
    let fills: Vec<_> = (1..=5)
        .map(|r| wb.table_cell_look(0, r, 1).unwrap().fill)
        .collect();
    let g = Some(0xE2EFDA);
    assert_eq!(fills, vec![g, g, g, None, g]);
    assert_eq!(
        wb.table_cell_look(0, 2, 0).unwrap().bottom.unwrap().style,
        LineStyle::Dashed
    );

    let out = scratch("new_style.xlsx");
    wb.save_as(&out).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert!(sheets_engine::package_problems(&std::fs::read(&out).unwrap()).is_empty());
    let p = parts(&out);
    let ts = table_styles_xml(&p);
    assert!(ts.contains("defaultTableStyle=\"Table Style 1\""), "{ts}");
    assert!(ts.contains("type=\"firstRowStripe\" size=\"3\""), "{ts}");
    let back = Workbook::open(&out).unwrap();
    assert_eq!(back.custom_table_styles(), vec!["Table Style 1".to_owned()]);
    assert_eq!(back.default_table_style(), "Table Style 1");
    assert_eq!(
        back.table_style_spec("Table Style 1").unwrap(),
        wb.table_style_spec("Table Style 1").unwrap()
    );
    for r in 0..9 {
        for c in 0..2 {
            assert_eq!(
                back.table_cell_look(0, r, c),
                wb.table_cell_look(0, r, c),
                "{r},{c}"
            );
        }
    }
}

#[test]
fn edits_undo_one_step_each() {
    let mut wb = table_book();
    wb.add_table_style(&green_spec("Mine"), false).unwrap();
    let mut o = wb.table_options("Table1").unwrap();
    o.style = Some("Mine".into());
    wb.set_table_options("Table1", o).unwrap();
    // Modify: rename and recolour; the table follows the new name.
    let mut spec = wb.table_style_spec("Mine").unwrap();
    spec.name = "Ours".into();
    spec.set_format(
        TableElement::HeaderRow,
        TableElementFormat {
            fill: Some(0xC00000),
            ..Default::default()
        },
    );
    wb.modify_table_style("Mine", &spec, true).unwrap();
    assert_eq!(wb.custom_table_styles(), vec!["Ours".to_owned()]);
    assert_eq!(
        wb.table_options("Table1").unwrap().style.as_deref(),
        Some("Ours")
    );
    assert_eq!(wb.table_cell_look(0, 0, 0).unwrap().fill, Some(0xC00000));
    assert!(wb.undo().unwrap());
    assert_eq!(wb.custom_table_styles(), vec!["Mine".to_owned()]);
    assert_eq!(
        wb.table_options("Table1").unwrap().style.as_deref(),
        Some("Mine")
    );
    assert_eq!(wb.table_cell_look(0, 0, 0).unwrap().fill, Some(0x00B050));
    assert!(wb.redo().unwrap());
    assert_eq!(wb.table_cell_look(0, 0, 0).unwrap().fill, Some(0xC00000));
    // Duplicate (as the dialog does it: a copy under a free name).
    let copy = wb.duplicate_table_style_name("Ours");
    assert_eq!(copy, "Ours 2");
    let mut dup = wb.table_style_spec("Ours").unwrap();
    dup.name = copy.clone();
    wb.add_table_style(&dup, false).unwrap();
    // A built-in one can be duplicated too.
    let medium = wb.table_style_spec("TableStyleMedium2").unwrap();
    assert!(!medium.format(TableElement::HeaderRow).is_empty());
    // Delete: the table falls back to the default style.
    wb.delete_table_style("Ours").unwrap();
    assert_eq!(wb.custom_table_styles(), vec![copy]);
    assert_eq!(
        wb.table_options("Table1").unwrap().style.as_deref(),
        Some("TableStyleMedium2")
    );
    assert!(wb.undo().unwrap());
    assert_eq!(
        wb.table_options("Table1").unwrap().style.as_deref(),
        Some("Ours")
    );
    assert_eq!(wb.default_table_style(), "Ours");
}

#[test]
fn file_style_modified_keeps_the_others_and_the_conditional_format() {
    let src = corpus("24_table_styles.xlsx");
    let orig = parts(&src);
    let mut wb = Workbook::open(&src).unwrap();
    let mut spec = wb.table_style_spec("Brand Purple").unwrap();
    spec.set_format(
        TableElement::HeaderRow,
        TableElementFormat {
            fill: Some(0x305496),
            font: Some(0xFFFFFF),
            bold: Some(true),
            ..Default::default()
        },
    );
    wb.modify_table_style("Brand Purple", &spec, true).unwrap();
    let out = scratch("modified.xlsx");
    wb.save_as(&out).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let p = parts(&out);
    // The pivot style is as the file had it, with its dxf.
    assert_eq!(
        element_dxf(&p, "Pivot Only", "headerRow"),
        element_dxf(&orig, "Pivot Only", "headerRow")
    );
    assert!(table_styles_xml(&p).contains(
        "<tableStyle name=\"Pivot Only\" table=\"0\" count=\"1\"><tableStyleElement type=\"headerRow\" dxfId=\"7\"/></tableStyle>"
    ));
    assert_eq!(dxfs(&p)[0], dxfs(&orig)[0]);
    let mut back = Workbook::open(&out).unwrap();
    assert_eq!(back.table_cell_look(0, 0, 1).unwrap().fill, Some(0x305496));
    assert_eq!(
        back.table_cell_look(0, 6, 2).unwrap().font_color,
        Some(0xFF0000)
    );
    assert!(back.table_cell_look(0, 2, 0).unwrap().italic);
    // The conditional format still has its red fill.
    let cf = back.conditional(0, 1, 2);
    assert!(cf.is_some_and(|c| c.style.as_ref().is_some_and(|s| s.fill == Some(0xFFC7CE))));
}

#[test]
fn save_edit_save_and_save_undo_save() {
    let src = corpus("24_table_styles.xlsx");
    let mut wb = Workbook::open(&src).unwrap();
    wb.add_table_style(&green_spec("Green"), false).unwrap();
    let first = scratch("session1.xlsx");
    wb.save_as(&first).unwrap();
    let p1 = parts(&first);
    let count1 = dxfs(&p1).len();
    // Saving again without edits writes the same (no dxfs pile up).
    let second = scratch("session2.xlsx");
    wb.save_as(&second).unwrap();
    assert_same_except_core(&p1, &parts(&second));
    // Edit, save: the new style's dxfs replace the old ones.
    let mut spec = wb.table_style_spec("Green").unwrap();
    spec.set_format(
        TableElement::TotalRow,
        TableElementFormat {
            italic: Some(true),
            ..Default::default()
        },
    );
    wb.modify_table_style("Green", &spec, false).unwrap();
    let third = scratch("session3.xlsx");
    wb.save_as(&third).unwrap();
    let p3 = parts(&third);
    assert_eq!(dxfs(&p3).len(), count1 + 1);
    assert!(element_dxf(&p3, "Green", "totalRow").is_some());
    // Undo the edit and the new style, save: as the opened file again.
    assert!(wb.undo().unwrap());
    assert!(wb.undo().unwrap());
    assert!(wb.custom_table_styles() == vec!["Brand Purple".to_owned()]);
    let fourth = scratch("session4.xlsx");
    wb.save_as(&fourth).unwrap();
    let p4 = parts(&fourth);
    let orig = parts(&src);
    assert_eq!(table_styles_xml(&p4), table_styles_xml(&orig));
    assert_eq!(dxfs(&p4).len(), dxfs(&orig).len());
    let back = Workbook::open(&fourth).unwrap();
    assert_eq!(back.custom_table_styles(), vec!["Brand Purple".to_owned()]);
    assert_eq!(back.table_cell_look(0, 0, 1).unwrap().fill, Some(PURPLE));
}

#[test]
fn styles_without_custom_ones_stay_as_ironcalc_writes_them() {
    let mut wb = table_book();
    let a = scratch("plain_a.xlsx");
    wb.save_as(&a).unwrap();
    assert!(!parts(&a)["xl/styles.xml"].contains("<tableStyles"));
    // A style made and undone leaves nothing behind.
    wb.add_table_style(&green_spec("Gone"), false).unwrap();
    let b = scratch("plain_b.xlsx");
    wb.save_as(&b).unwrap();
    assert!(parts(&b)["xl/styles.xml"].contains("name=\"Gone\""));
    wb.undo().unwrap();
    let c = scratch("plain_c.xlsx");
    wb.save_as(&c).unwrap();
    assert_eq!(parts(&a)["xl/styles.xml"], parts(&c)["xl/styles.xml"]);
}

/// A table's own formats (`headerRowDxfId`, a column's `dataDxfId`) are
/// drawn over its style and under the cells' own formatting, as in Excel.
#[test]
fn table_dxfs_layer_between_style_and_cells() {
    use std::io::{Read, Write};
    let mut wb = Workbook::new_empty().unwrap();
    for (c, h) in ["Item", "Qty"].iter().enumerate() {
        wb.set_input(0, 0, c as u32, h).unwrap();
        wb.set_input(0, 1, c as u32, "1").unwrap();
        wb.set_input(0, 2, c as u32, "2").unwrap();
    }
    wb.create_table(0, CellRange::from_corners((0, 0), (2, 1)), true, "TableStyleMedium2")
        .unwrap();
    // B3's own fill: it wins over the column's.
    wb.set_range_style(
        0,
        CellRange::from_corners((2, 1), (2, 1)),
        sheets_engine::StyleChange::Fill(Some(0x00FF00)),
    )
    .unwrap();
    let path = scratch("dxfs-kantor.xlsx");
    wb.save_as(&path).unwrap();
    let file = scratch("dxfs-excel.xlsx");
    {
        let mut zin = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let mut zout = zip::ZipWriter::new(std::fs::File::create(&file).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        let n_dxfs = {
            let mut s = String::new();
            zin.by_name("xl/styles.xml").unwrap().read_to_string(&mut s).unwrap();
            s.matches("<dxf>").count()
        };
        for i in 0..zin.len() {
            let mut f = zin.by_index(i).unwrap();
            let name = f.name().unwrap().to_string();
            let mut data = String::new();
            let mut bytes = Vec::new();
            f.read_to_end(&mut bytes).unwrap();
            if name == "xl/styles.xml" || name.starts_with("xl/tables/") {
                data = String::from_utf8(bytes.clone()).unwrap();
            }
            let out = if name == "xl/styles.xml" {
                let mine = "<dxf><font><i/><color rgb=\"FFFF0000\"/></font></dxf><dxf><fill><patternFill><bgColor rgb=\"FFFFFF00\"/></patternFill></fill></dxf>";
                let d = if data.contains("</dxfs>") {
                    data.replacen("</dxfs>", &format!("{mine}</dxfs>"), 1)
                } else {
                    data.replacen("<dxfs count=\"0\"/>", &format!("<dxfs count=\"2\">{mine}</dxfs>"), 1)
                };
                assert!(d.contains(mine), "{d}");
                d.into_bytes()
            } else if name.starts_with("xl/tables/") {
                data.replacen("<table ", &format!("<table headerRowDxfId=\"{n_dxfs}\" "), 1)
                    .replacen(
                        "name=\"Qty\"",
                        &format!("name=\"Qty\" dataDxfId=\"{}\"", n_dxfs + 1),
                        1,
                    )
                    .into_bytes()
            } else {
                bytes
            };
            zout.start_file(&name, opts).unwrap();
            zout.write_all(&out).unwrap();
        }
        zout.finish().unwrap();
    }
    let wb = Workbook::open(&file).unwrap();
    // Header: the style's fill and white bold font; the table's dxf makes it
    // italic red.
    let h = wb.table_cell_look(0, 0, 0).unwrap();
    assert!(h.italic && h.bold, "{h:?}");
    assert_eq!(h.font_color, Some(0xFF0000));
    assert!(h.fill.is_some());
    // Qty's data cells: the column's yellow over the stripes.
    assert_eq!(wb.table_cell_look(0, 1, 1).unwrap().fill, Some(0xFFFF00));
    let item = wb.table_cell_look(0, 1, 0).unwrap();
    assert_ne!(item.fill, Some(0xFFFF00));
    assert!(!item.italic, "the header's format stays on the header");
    // B3 keeps its own green (drawn over the table's).
    assert_eq!(wb.cell_style(0, 2, 1).fill, Some(0x00FF00));
}
