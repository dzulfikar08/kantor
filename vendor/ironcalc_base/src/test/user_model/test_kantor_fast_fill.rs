#![allow(clippy::unwrap_used)]
//! KANTOR PATCH (fast fill): a fill gives formula cells their source's
//! shared formula instead of typing the translated text into each one. It
//! must leave exactly what typing the text leaves, and undo/redo the same.

use crate::expressions::types::Area;
use crate::test::util::new_empty_model;
use crate::UserModel;

/// Formulas the fill copies (all but the defined name), and some it can't
/// (the name; references that fall off the sheet when filling up or left).
const FORMULAS: &[&str] = &[
    "=A10*2",
    "=$A$2+A9",
    "=A$2+$A3",
    "=SUM(A:A)",
    "=SUM($A$1:A10)",
    "=Sheet2!A1+1",
    "=IF(A10>5,\"big\",\"small\")&\"!\"",
    "=0.1+0.2",
    "=-A10^2%",
    "=OFFSET(A1,1,0)",
    "=ROW()*COLUMN()",
    "=COUNTIF(A1:A30,\">=3\")",
    "=x+1",
    "=A1",
    "={1,2,3}+0",
    "=\"text\"",
];

fn setup() -> UserModel<'static> {
    let mut model = UserModel::from_model(new_empty_model());
    model.new_sheet().unwrap();
    model.set_selected_sheet(0).unwrap();
    model.new_defined_name("x", None, "Sheet1!$A$1").unwrap();
    for row in 1..=30 {
        model.set_user_input(0, row, 1, &row.to_string()).unwrap();
        model
            .set_user_input(1, row, 1, &(row * 10).to_string())
            .unwrap();
    }
    model
}

/// Contents and values of a rectangle, and the R1C1 formula of each cell.
fn snapshot(model: &UserModel, rows: (i32, i32), columns: (i32, i32)) -> Vec<String> {
    let ws = &model.get_model().workbook.worksheets[0];
    let mut out = Vec::new();
    for row in rows.0..=rows.1 {
        for column in columns.0..=columns.1 {
            let rc = ws
                .cell(row, column)
                .and_then(|c| c.get_formula())
                .map(|f| ws.shared_formulas[f as usize].clone())
                .unwrap_or_default();
            out.push(format!(
                "{row},{column}: {} = {} [{rc}]",
                model.get_cell_content(0, row, column).unwrap(),
                model.get_formatted_cell_value(0, row, column).unwrap(),
            ));
        }
    }
    out
}

/// What the fill should leave: the source's formula translated to each
/// target and typed in.
fn typed(model: &mut UserModel, targets: impl Iterator<Item = ((i32, i32), (i32, i32))>) {
    for ((source_row, source_column), (row, column)) in targets {
        let text = model
            .get_model()
            .extend_to(0, source_row, source_column, row, column)
            .unwrap();
        model.set_user_input(0, row, column, &text).unwrap();
    }
}

#[test]
fn fill_down_and_up_match_typing() {
    let width = FORMULAS.len() as i32;
    for to_row in [25, 1] {
        let mut filled = setup();
        let mut reference = setup();
        for (i, f) in FORMULAS.iter().enumerate() {
            filled.set_user_input(0, 10, 2 + i as i32, f).unwrap();
            reference.set_user_input(0, 10, 2 + i as i32, f).unwrap();
        }
        let area = Area {
            sheet: 0,
            row: 10,
            column: 2,
            width,
            height: 1,
        };
        filled.auto_fill_rows(&area, to_row).unwrap();
        let rows: Vec<i32> = if to_row > 10 {
            (11..=to_row).collect()
        } else {
            (to_row..10).collect()
        };
        typed(
            &mut reference,
            rows.iter()
                .flat_map(|&row| (0..width).map(move |i| ((10, 2 + i), (row, 2 + i)))),
        );
        filled.evaluate();
        reference.evaluate();
        let span = (to_row.min(10), to_row.max(10));
        assert_eq!(
            snapshot(&filled, span, (2, 1 + width)),
            snapshot(&reference, span, (2, 1 + width)),
            "fill to row {to_row}"
        );
        // Down, every formula is copied but the name's and the array one
        // (not a single value); up, some references also fall off the top.
        let copied = filled.kantor_fill_copies;
        let targets = rows.len() * FORMULAS.len();
        if to_row > 10 {
            assert_eq!(copied, rows.len() * (FORMULAS.len() - 2));
        } else {
            assert!(copied > 0 && copied < targets - rows.len(), "{copied}");
        }
    }
}

#[test]
fn fill_right_and_left_match_typing() {
    let height = FORMULAS.len() as i32;
    let rewritten: Vec<String> = FORMULAS
        .iter()
        .map(|f| f.replace("A10", "J1").replace("A9", "I1"))
        .collect();
    for to_column in [16, 1] {
        let mut filled = setup();
        let mut reference = setup();
        for (i, f) in rewritten.iter().enumerate() {
            filled.set_user_input(0, 2 + i as i32, 10, f).unwrap();
            reference.set_user_input(0, 2 + i as i32, 10, f).unwrap();
        }
        let area = Area {
            sheet: 0,
            row: 2,
            column: 10,
            width: 1,
            height,
        };
        filled.auto_fill_columns(&area, to_column).unwrap();
        let columns: Vec<i32> = if to_column > 10 {
            (11..=to_column).collect()
        } else {
            (to_column..10).collect()
        };
        typed(
            &mut reference,
            columns
                .iter()
                .flat_map(|&column| (0..height).map(move |i| ((2 + i, 10), (2 + i, column)))),
        );
        filled.evaluate();
        reference.evaluate();
        let span = (to_column.min(10), to_column.max(10));
        assert_eq!(
            snapshot(&filled, (2, 1 + height), span),
            snapshot(&reference, (2, 1 + height), span),
            "fill to column {to_column}"
        );
        assert!(filled.kantor_fill_copies > 0);
    }
}

#[test]
fn fill_undo_and_redo_restore_exactly() {
    let mut model = setup();
    for (i, f) in FORMULAS.iter().enumerate() {
        model.set_user_input(0, 10, 2 + i as i32, f).unwrap();
    }
    // Something in the way, for undo to bring back.
    model.set_user_input(0, 15, 3, "old").unwrap();
    model.evaluate();
    let span = (1, 40);
    let columns = (1, 2 + FORMULAS.len() as i32);
    let before = snapshot(&model, span, columns);
    let area = Area {
        sheet: 0,
        row: 10,
        column: 2,
        width: FORMULAS.len() as i32,
        height: 1,
    };
    model.auto_fill_rows(&area, 40).unwrap();
    model.evaluate();
    let after = snapshot(&model, span, columns);
    assert_ne!(before, after);
    model.undo().unwrap();
    model.evaluate();
    assert_eq!(snapshot(&model, span, columns), before);
    model.redo().unwrap();
    model.evaluate();
    assert_eq!(snapshot(&model, span, columns), after);
}

#[test]
fn changes_are_summed_up_without_the_send_queue() {
    let mut model = setup();
    model.kantor_stop_send_queue();
    let _ = model.kantor_take_changes();
    model.set_user_input(0, 10, 2, "=A10").unwrap();
    model
        .auto_fill_rows(
            &Area {
                sheet: 0,
                row: 10,
                column: 2,
                width: 1,
                height: 1,
            },
            20,
        )
        .unwrap();
    let changes = model.kantor_take_changes();
    assert!(changes.touches(0, 20, 2, 20, 2));
    assert!(!changes.touches(0, 21, 2, 30, 2));
    assert!(model.kantor_take_changes().is_empty());
    model.undo().unwrap();
    assert!(model.kantor_take_changes().touches(0, 15, 2, 15, 2));
    let nothing = UserModel::from_model(new_empty_model()).flush_send_queue();
    assert_eq!(model.flush_send_queue(), nothing);
}

// KANTOR PATCH (fill undo records): copied formulas are recorded by their
// source's position, not their text; undo and redo stay exact.

/// Fills `FORMULAS` (a column of them at B10 or a row at B10) each way,
/// then undoes and redoes three times; every state must be what the fill
/// and the undo left the first time.
#[test]
fn fill_records_undo_and_redo_every_way() {
    let n = FORMULAS.len() as i32;
    let cases: [(bool, i32); 4] = [(true, 40), (true, 3), (false, 2 + n + 6), (false, 1)];
    for (down, to) in cases {
        let mut model = setup();
        for (i, f) in FORMULAS.iter().enumerate() {
            let (row, column) = if down {
                (10, 2 + i as i32)
            } else {
                (10 + i as i32, 2)
            };
            model.set_user_input(0, row, column, f).unwrap();
        }
        model.set_user_input(0, 5, 3, "old").unwrap();
        model.set_user_input(0, 12, 1, "old").unwrap();
        model.evaluate();
        let rows = (1, 10 + n + 1);
        let columns = (1, 2 + n + 6);
        let before = snapshot(&model, rows, columns);
        if down {
            let area = Area {
                sheet: 0,
                row: 10,
                column: 2,
                width: n,
                height: 1,
            };
            model.auto_fill_rows(&area, to).unwrap();
        } else {
            let area = Area {
                sheet: 0,
                row: 10,
                column: 2,
                width: 1,
                height: n,
            };
            model.auto_fill_columns(&area, to).unwrap();
        }
        assert!(model.kantor_fill_copies > 0);
        model.evaluate();
        let after = snapshot(&model, rows, columns);
        assert_ne!(before, after);
        for _ in 0..3 {
            model.undo().unwrap();
            model.evaluate();
            assert_eq!(snapshot(&model, rows, columns), before, "undo {down} {to}");
            model.redo().unwrap();
            model.evaluate();
            assert_eq!(snapshot(&model, rows, columns), after, "redo {down} {to}");
        }
    }
}

/// With the send queue on (upstream's default, for a remote copy), the
/// fill's diff lists replayed on a copy of the model leave the same cells.
#[test]
fn fill_records_replay_on_a_remote_copy() {
    let mut model = setup();
    for (i, f) in FORMULAS.iter().enumerate() {
        model.set_user_input(0, 10, 2 + i as i32, f).unwrap();
    }
    let _ = model.flush_send_queue();
    let mut remote = UserModel::from_model(
        crate::Model::from_bytes(&model.get_model().to_bytes(), "en").unwrap(),
    );
    let area = Area {
        sheet: 0,
        row: 10,
        column: 2,
        width: FORMULAS.len() as i32,
        height: 1,
    };
    model.auto_fill_rows(&area, 25).unwrap();
    model.undo().unwrap();
    model.redo().unwrap();
    remote
        .apply_external_diffs(&model.flush_send_queue())
        .unwrap();
    model.evaluate();
    remote.evaluate();
    let span = (1, 30);
    let columns = (1, 2 + FORMULAS.len() as i32);
    assert_eq!(snapshot(&remote, span, columns), snapshot(&model, span, columns));
}
