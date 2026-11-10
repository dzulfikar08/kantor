// KANTOR PATCH (fidelity): what the patched reader and writer keep.
#![allow(clippy::unwrap_used)]

use std::io::Cursor;

use ironcalc::export::save_xlsx_to_writer;
use ironcalc::import::load_from_xlsx_bytes;
use ironcalc_base::types::{
    Alignment, BorderItem, BorderStyle, CellProtection, Color, HorizontalAlignment, Row,
};
use ironcalc_base::Model;

const STYLES: [BorderStyle; 13] = [
    BorderStyle::Thin,
    BorderStyle::Medium,
    BorderStyle::Thick,
    BorderStyle::Double,
    BorderStyle::Dotted,
    BorderStyle::SlantDashDot,
    BorderStyle::MediumDashed,
    BorderStyle::MediumDashDotDot,
    BorderStyle::MediumDashDot,
    BorderStyle::Hair,
    BorderStyle::Dashed,
    BorderStyle::DashDot,
    BorderStyle::DashDotDot,
];

fn round_trip(model: &Model) -> Model<'static> {
    let bytes = save_xlsx_to_writer(model, Cursor::new(Vec::new()))
        .unwrap()
        .into_inner();
    let workbook = load_from_xlsx_bytes(&bytes, "t", "en", "UTC").unwrap();
    Model::from_workbook(workbook, "en").unwrap()
}

#[test]
fn styles_survive_a_round_trip() {
    let mut model = Model::new_empty("t", "en", "UTC", "en").unwrap();
    for (i, style) in STYLES.iter().enumerate() {
        let mut s = model.get_style_for_cell(0, i as i32 + 1, 1).unwrap();
        let item = Some(BorderItem {
            style: style.clone(),
            color: Color::Rgb("#1F4E79".to_string()),
        });
        s.border.left = item.clone();
        s.border.diagonal = item;
        s.border.diagonal_up = i % 2 == 0;
        s.border.diagonal_down = i % 3 == 0;
        model.set_cell_style(0, i as i32 + 1, 1, &s).unwrap();
    }
    let alignments = [
        (1, 0, false),
        (3, 45, false),
        (0, 120, true),
        (2, 255, false),
    ];
    for (i, (indent, text_rotation, shrink_to_fit)) in alignments.iter().enumerate() {
        let mut s = model.get_style_for_cell(0, i as i32 + 1, 2).unwrap();
        s.alignment = Some(Alignment {
            horizontal: HorizontalAlignment::Right,
            indent: *indent,
            text_rotation: *text_rotation,
            shrink_to_fit: *shrink_to_fit,
            ..Default::default()
        });
        model.set_cell_style(0, i as i32 + 1, 2, &s).unwrap();
    }
    let protections = [(false, false), (false, true), (true, true)];
    for (i, (locked, hidden)) in protections.iter().enumerate() {
        let mut s = model.get_style_for_cell(0, i as i32 + 1, 3).unwrap();
        s.protection = Some(CellProtection {
            locked: *locked,
            hidden: *hidden,
        });
        model.set_cell_style(0, i as i32 + 1, 3, &s).unwrap();
    }

    let again = round_trip(&model);
    for row in 1..=STYLES.len() as i32 {
        assert_eq!(
            again.get_style_for_cell(0, row, 1).unwrap().border,
            model.get_style_for_cell(0, row, 1).unwrap().border,
            "row {row}"
        );
    }
    for row in 1..=alignments.len() as i32 {
        assert_eq!(
            again.get_style_for_cell(0, row, 2).unwrap().alignment,
            model.get_style_for_cell(0, row, 2).unwrap().alignment,
        );
    }
    for row in 1..=protections.len() as i32 {
        assert_eq!(
            again.get_style_for_cell(0, row, 3).unwrap().protection,
            model.get_style_for_cell(0, row, 3).unwrap().protection,
        );
    }
    // A plain cell still has no protection record (Excel's default).
    assert_eq!(again.get_style_for_cell(0, 9, 9).unwrap().protection, None);
}

#[test]
fn outline_levels_survive_a_round_trip() {
    let mut model = Model::new_empty("t", "en", "UTC", "en").unwrap();
    model.set_user_input(0, 1, 1, "x".to_string()).unwrap();
    let ws = model.workbook.worksheet_mut(0).unwrap();
    for (r, level, hidden, collapsed) in [
        (2, 1, false, false),
        (3, 2, true, false),
        (4, 0, false, true),
    ] {
        ws.rows.push(Row {
            r,
            height: 15.0,
            custom_format: false,
            custom_height: false,
            s: 0,
            hidden,
            outline_level: level,
            collapsed,
        });
    }
    ws.set_column_width(3, 100.0).unwrap();
    ws.cols[0].outline_level = 1;
    ws.cols[0].collapsed = true;

    let again = round_trip(&model);
    let a = &model.workbook.worksheets[0];
    let b = &again.workbook.worksheets[0];
    let outline = |rows: &[Row]| -> Vec<(i32, u8, bool, bool)> {
        let mut v: Vec<_> = rows
            .iter()
            .map(|r| (r.r, r.outline_level, r.hidden, r.collapsed))
            .collect();
        v.sort();
        v
    };
    assert_eq!(outline(&a.rows), outline(&b.rows));
    assert_eq!(b.cols[0].outline_level, 1);
    assert!(b.cols[0].collapsed);
}
