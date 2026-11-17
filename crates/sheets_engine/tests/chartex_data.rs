//! Charts of the Office 2016 types in 25_chartex.xlsx (written by
//! scripts/corpus/gen_chartex_xlsx.py; one per sheet, in
//! `mc:AlternateContent`): their data range in the Chart panel, changed
//! and saved; moved and deleted like any chart.

#[path = "support/pivot_check.rs"]
mod pivot_check;

use std::path::{Path, PathBuf};

use pivot_check::parts;
use sheets_engine::drawing::{Anchor, DrawingKind};
use sheets_engine::{CellRange, ChartType, LegendPos, Workbook};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/xlsx/25_chartex.xlsx")
}

fn save(
    wb: &mut Workbook,
    dir: &Path,
    name: &str,
) -> (PathBuf, std::collections::BTreeMap<String, String>) {
    let path = dir.join(name);
    wb.save_as(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        sheets_engine::package_problems(&bytes),
        Vec::<String>::new()
    );
    if let Ok(dir) = std::env::var("KANTOR_PIVOT_SAMPLES") {
        std::fs::copy(&path, Path::new(&dir).join(name)).unwrap();
    }
    let p = parts(&path);
    (path, p)
}

#[test]
fn the_chart_panel_shows_and_changes_their_data_range() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(fixture()).unwrap();
    let labels = [
        "Waterfall",
        "Funnel",
        "Treemap",
        "Sunburst",
        "Histogram",
        "Pareto",
        "Box & Whisker",
    ];
    let ranges = [
        "A1:B7", "A1:B6", "A1:C7", "A1:D7", "A1:A21", "A1:B9", "A1:C11",
    ];
    for (k, (label, range)) in labels.iter().zip(ranges).enumerate() {
        let d = wb.drawings(k as u32);
        let info = wb
            .chart_info(d[0].key)
            .unwrap_or_else(|| panic!("sheet {k}"));
        assert_eq!(info.kind_label, *label);
        assert!(!info.data_unknown, "{label}");
        assert_eq!(info.spec.data.name(), range, "{label}");
    }
    // The funnel (sheet 1): its first four stages only.
    let key = wb.drawings(1)[0].key;
    let mut spec = wb.chart_info(key).unwrap().spec;
    spec.data = CellRange::from_corners((0, 0), (4, 1));
    wb.set_chart(key, spec.clone()).unwrap();
    let info = wb.chart_info(key).unwrap();
    assert_eq!(info.spec.data.name(), "A1:B5");
    let xml = match &*wb.drawings(1)[0].kind {
        DrawingKind::Chart { xml, .. } => xml.to_string(),
        _ => unreachable!(),
    };
    assert!(xml.contains("<cx:f>Funnel!$A$2:$A$5</cx:f>"), "{xml}");
    assert!(
        xml.contains("<cx:lvl ptCount=\"4\" formatCode=\"General\">"),
        "{xml}"
    );
    // Series colours aren't Kantor's to change on these.
    let mut other = spec.clone();
    other.colors = vec![sheets_engine::SeriesColor::Rgb([1, 2, 3])];
    let err = wb.set_chart(key, other).unwrap_err().to_string();
    assert!(err.contains("can't change that on a Funnel chart"), "{err}");
    // The treemap (sheet 2) over more rows: its two category columns grow.
    wb.set_input(2, 7, 0, "West").unwrap();
    wb.set_input(2, 7, 1, "Eta").unwrap();
    wb.set_input(2, 7, 2, "8").unwrap();
    let tkey = wb.drawings(2)[0].key;
    let mut tspec = wb.chart_info(tkey).unwrap().spec;
    tspec.data = CellRange::from_corners((0, 0), (7, 2));
    wb.set_chart(tkey, tspec).unwrap();
    // Too few columns for the categories and values.
    let mut narrow = wb.chart_info(tkey).unwrap().spec;
    narrow.data = CellRange::from_corners((0, 0), (7, 1));
    let err = wb.set_chart(tkey, narrow).unwrap_err().to_string();
    assert!(err.contains("needs 3 columns"), "{err}");
    // Moved like any chart.
    let moved = Anchor::TwoCell {
        from: sheets_engine::drawing::Marker {
            col: 6,
            col_off: 0,
            row: 2,
            row_off: 0,
        },
        to: sheets_engine::drawing::Marker {
            col: 12,
            col_off: 0,
            row: 16,
            row_off: 0,
        },
        edit_as: sheets_engine::drawing::EditAs::TwoCell,
    };
    wb.place_drawing(key, moved).unwrap();
    // The box & whisker (sheet 6) deleted.
    let bkey = wb.drawings(6)[0].key;
    wb.delete_drawing(bkey).unwrap();
    let (path, p) = save(&mut wb, dir.path(), "chartex-range.xlsx");
    let part = &p["xl/charts/chartEx2.xml"];
    assert!(part.contains("<cx:f>Funnel!$B$2:$B$5</cx:f>"), "{part}");
    let tree = &p["xl/charts/chartEx3.xml"];
    assert!(tree.contains("<cx:f>Treemap!$A$2:$B$8</cx:f>"), "{tree}");
    assert!(tree.contains(">Eta</cx:pt>"), "{tree}");
    // Both the choice's and the fallback's anchor moved.
    let drawing = &p["xl/drawings/drawing2.xml"];
    assert_eq!(
        drawing.matches("<xdr:col>6</xdr:col>").count(),
        2,
        "{drawing}"
    );
    assert!(drawing.contains("<mc:AlternateContent"), "{drawing}");
    assert!(!p["xl/drawings/drawing7.xml"].contains("AlternateContent"));
    let back = Workbook::open(&path).unwrap();
    assert_eq!(
        back.chart_info(back.drawings(1)[0].key)
            .unwrap()
            .spec
            .data
            .name(),
        "A1:B5"
    );
    assert_eq!(
        back.chart_info(back.drawings(2)[0].key)
            .unwrap()
            .spec
            .data
            .name(),
        "A1:C8"
    );
    assert!(back.drawings(6).is_empty());
    match back.drawings(1)[0].anchor {
        Anchor::TwoCell { from, .. } => assert_eq!((from.row, from.col), (2, 6)),
        a => panic!("{a:?}"),
    }
    // Undone (delete, move, treemap range, three cells, funnel range): the
    // next save is as the file was.
    for _ in 0..7 {
        wb.undo().unwrap();
    }
    let (_, p2) = save(&mut wb, dir.path(), "chartex-undone.xlsx");
    assert!(p2["xl/charts/chartEx2.xml"].contains("<cx:f>Funnel!$B$2:$B$6</cx:f>"));
    assert!(p2["xl/drawings/drawing7.xml"].contains("AlternateContent"));
}

#[test]
fn title_legend_type_and_rows_change() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(fixture()).unwrap();
    // The waterfall (sheet 0): a new title, no legend, then a funnel.
    let key = wb.drawings(0)[0].key;
    let info = wb.chart_info(key).unwrap();
    assert_eq!(info.spec.kind, ChartType::Waterfall);
    assert_eq!(info.spec.title, "Cash flow");
    assert_eq!(info.spec.legend, LegendPos::Top);
    let mut spec = info.spec.clone();
    spec.title = "Money & more".into();
    spec.legend = LegendPos::None;
    spec.kind = ChartType::Funnel;
    wb.set_chart(key, spec.clone()).unwrap();
    let info = wb.chart_info(key).unwrap();
    assert_eq!(info.spec, spec);
    assert_eq!(info.kind_label, "Funnel");
    // Every Office 2016 type in turn (the Pareto line comes and goes).
    for kind in ChartType::EX {
        spec.kind = kind;
        wb.set_chart(key, spec.clone()).unwrap();
        assert_eq!(wb.chart_info(key).unwrap().spec.kind, kind);
    }
    spec.kind = ChartType::Pareto;
    wb.set_chart(key, spec.clone()).unwrap();
    // Not a classic type.
    let mut bad = spec.clone();
    bad.kind = ChartType::Column;
    assert!(wb.set_chart(key, bad).is_err());
    // The box & whisker (sheet 6) laid out with series in rows.
    for (r, row) in [
        ["Day", "Mon", "Tue"],
        ["Morning", "12", "10"],
        ["Evening", "20", "30"],
    ]
    .iter()
    .enumerate()
    {
        for (c, v) in row.iter().enumerate() {
            wb.set_input(6, 20 + r as u32, c as u32, v).unwrap();
        }
    }
    let bkey = wb.drawings(6)[0].key;
    let mut b = wb.chart_info(bkey).unwrap().spec;
    b.by_rows = true;
    b.data = CellRange::from_corners((20, 0), (22, 2));
    wb.set_chart(bkey, b).unwrap();
    let bi = wb.chart_info(bkey).unwrap();
    assert!(bi.spec.by_rows);
    assert_eq!(bi.spec.data.name(), "A21:C23");
    let (path, p) = save(&mut wb, dir.path(), "chartex-styled.xlsx");
    let part = &p["xl/charts/chartEx1.xml"];
    assert!(part.contains("<cx:v>Money &amp; more</cx:v>"), "{part}");
    assert!(!part.contains("<cx:legend"), "{part}");
    assert!(part.contains("layoutId=\"paretoLine\""), "{part}");
    assert!(part.contains("<cx:units unit=\"percentage\"/>"), "{part}");
    let boxes = &p["xl/charts/chartEx7.xml"];
    assert!(
        boxes.contains("<cx:f>'Box and whisker'!$B$22:$C$22</cx:f>"),
        "{boxes}"
    );
    assert!(
        boxes.contains("<cx:f>'Box and whisker'!$B$21:$C$21</cx:f>"),
        "{boxes}"
    );
    let back = Workbook::open(&path).unwrap();
    let i = back.chart_info(back.drawings(0)[0].key).unwrap();
    assert_eq!(
        (i.spec.kind, i.spec.title.as_str()),
        (ChartType::Pareto, "Money & more")
    );
    assert!(
        back.chart_info(back.drawings(6)[0].key)
            .unwrap()
            .spec
            .by_rows
    );
}

#[test]
fn rows_inserted_move_their_references() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(fixture()).unwrap();
    wb.insert_rows(1, 0, 2).unwrap();
    let (_, p) = save(&mut wb, dir.path(), "chartex-rows.xlsx");
    let part = &p["xl/charts/chartEx2.xml"];
    assert!(part.contains("<cx:f>Funnel!$B$4:$B$8</cx:f>"), "{part}");
}
