//! Partial recalculation gives what a full one gives: random formula graphs,
//! random edits, the values after each partial evaluation compared with a
//! full evaluation of the same workbook.

use crate::expressions::types::Area;
use crate::UserModel;

const ROWS: i32 = 14;
const COLUMNS: i32 = 6;
const SHEETS: u32 = 2;

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }
}

fn column_name(column: i32) -> char {
    (b'A' + (column - 1) as u8) as char
}

/// Formulas read columns before `lim` only (`lim` past the last column:
/// any), so that a formula in column `lim` can't be on a loop.
fn column(rng: &mut Rng, lim: i32) -> char {
    column_name(1 + rng.below((lim - 1).min(COLUMNS) as u64) as i32)
}

fn cell_ref(rng: &mut Rng, lim: i32) -> String {
    let dollar = |rng: &mut Rng| if rng.below(4) == 0 { "$" } else { "" };
    let sheet = if rng.below(5) == 0 { "Sheet2!" } else { "" };
    let d1 = dollar(rng);
    let c = column(rng, lim);
    format!(
        "{sheet}{d1}{c}{}{}",
        dollar(rng),
        1 + rng.below(ROWS as u64)
    )
}

fn range_ref(rng: &mut Rng, lim: i32) -> String {
    // (Whole columns are few: criteria functions read every row of them,
    // which is slow in a debug build.)
    match rng.below(30) {
        0 => format!("{}:{}", column(rng, lim.min(4)), column(rng, lim.min(4))),
        1..=5 => {
            let c = column(rng, lim);
            format!("{c}$1:{c}{}", 1 + rng.below(ROWS as u64))
        }
        _ => {
            let a = cell_ref(rng, lim);
            let b = cell_ref(rng, lim);
            // Both ends on one sheet.
            let b = b.trim_start_matches("Sheet2!").to_string();
            format!("{a}:{b}")
        }
    }
}

fn formula(rng: &mut Rng, depth: u32, lim: i32) -> String {
    if lim <= 1 {
        return format!("{}*2", rng.below(20));
    }
    if depth > 2 {
        return match rng.below(3) {
            1 => format!("{}", rng.below(20)),
            _ => cell_ref(rng, lim),
        };
    }
    let sub = |rng: &mut Rng| formula(rng, depth + 1, lim);
    match rng.below(18) {
        0..=2 => cell_ref(rng, lim),
        3 => format!("SUM({})", range_ref(rng, lim)),
        4 => format!("{}+{}", sub(rng), sub(rng)),
        5 => format!("{}*{}", sub(rng), sub(rng)),
        6 => format!("IF({}>5,{},{})", sub(rng), sub(rng), sub(rng)),
        7 => format!("COUNTIF({},\">3\")", range_ref(rng, lim)),
        8 if lim > 3 => format!("IFERROR(VLOOKUP({},A1:C{ROWS},2,FALSE),-1)", sub(rng)),
        9 if lim > 2 => format!("IFERROR(MATCH({},B$1:B$20,0),0)", sub(rng)),
        // (INDEX and names may return arrays: typed in, they become
        // dynamic arrays, which are always evaluated in full.)
        10 => format!("SUMPRODUCT({})", range_ref(rng, lim)),
        11 if lim > 2 => format!("SUM(INDIRECT(\"A1:B\"&MAX(1,MIN({ROWS},{}))))", sub(rng)),
        12 if lim > 3 => format!("SUM(OFFSET(A1,1,1,2,2))+{}", sub(rng)),
        13 => format!("ROWS({})+COLUMN()", range_ref(rng, lim)),
        14 => format!("SUBTOTAL(9,{})", range_ref(rng, lim)),
        15 => format!("MAX({},{})", range_ref(rng, lim), sub(rng)),
        16 => format!("{}&\"x\"", sub(rng)),
        _ => format!("SUMIF({},\">2\")", range_ref(rng, lim)),
    }
}

fn input(rng: &mut Rng, lim: i32) -> String {
    match rng.below(10) {
        0..=4 => format!("={}", formula(rng, 0, lim)),
        5 => String::new(),
        6 => ["x", "apple", "TRUE"][rng.below(3) as usize].to_string(),
        _ => format!("{}", rng.below(12)),
    }
}

fn values(model: &UserModel) -> Vec<String> {
    let mut out = Vec::new();
    let sheets = model.get_model().workbook.worksheets.len() as u32;
    for sheet in 0..sheets {
        for row in 1..=ROWS + 4 {
            for column in 1..=COLUMNS {
                out.push(model.get_formatted_cell_value(sheet, row, column).unwrap());
            }
        }
    }
    out
}

/// After a partial evaluation, every value is what a full one makes.
fn check(model: &mut UserModel, step: &str) {
    let partial = values(model);
    model.evaluate();
    let full = values(model);
    for (i, (p, f)) in partial.iter().zip(&full).enumerate() {
        let sheet = i as i32 / ((ROWS + 4) * COLUMNS);
        let rest = i as i32 % ((ROWS + 4) * COLUMNS);
        let (row, column) = (rest / COLUMNS + 1, rest % COLUMNS + 1);
        assert_eq!(
            p,
            f,
            "after {step}: Sheet{} {}{row} = {:?}",
            sheet + 1,
            column_name(column),
            model.get_cell_content(sheet as u32, row, column)
        );
    }
}

fn random_workbook(seed: u64, acyclic: bool) -> (UserModel<'static>, Rng) {
    let mut rng = Rng(seed);
    let mut model = UserModel::new_empty("partial", "en", "UTC", "en").unwrap();
    model.new_sheet().unwrap();
    for sheet in 0..SHEETS {
        for row in 1..=ROWS {
            for column in 1..=COLUMNS {
                let lim = if acyclic { column } else { COLUMNS + 1 };
                let value = input(&mut rng, lim);
                model.set_user_input(sheet, row, column, &value).unwrap();
            }
        }
    }
    model.evaluate();
    (model, rng)
}

fn edit(model: &mut UserModel, rng: &mut Rng, acyclic: bool) -> String {
    let sheet = rng.below(SHEETS as u64) as u32;
    let row = 1 + rng.below(ROWS as u64) as i32;
    let column = 1 + rng.below(COLUMNS as u64) as i32;
    match rng.below(8) {
        0..=3 => {
            let lim = if acyclic { column } else { COLUMNS + 1 };
            let value = input(rng, lim);
            let _ = model.set_user_input(sheet, row, column, &value);
            format!("Sheet{} {}{row} := {value}", sheet + 1, column_name(column))
        }
        4 => {
            let area = Area {
                sheet,
                row,
                column,
                width: 1 + rng.below(2) as i32,
                height: 1,
            };
            let to = (row + 1 + rng.below(4) as i32).min(ROWS + 4);
            let _ = model.auto_fill_rows(&area, to);
            format!(
                "fill Sheet{} {}{row} down to {to}",
                sheet + 1,
                column_name(column)
            )
        }
        5 => {
            let area = Area {
                sheet,
                row,
                column,
                width: 2,
                height: 2,
            };
            let _ = model.range_clear_contents(&area);
            format!("clear Sheet{} {}{row}:+2", sheet + 1, column_name(column))
        }
        6 => {
            let _ = model.undo();
            "undo".to_string()
        }
        _ => {
            let _ = model.redo();
            "redo".to_string()
        }
    }
}

/// Automatic calculation: every edit evaluates what it changed.
#[test]
fn partial_recalculation_matches_full_after_each_edit() {
    for seed in 0..25 {
        let (mut model, mut rng) = random_workbook(seed, seed % 2 == 0);
        for _ in 0..20 {
            let step = edit(&mut model, &mut rng, seed % 2 == 0);
            check(&mut model, &format!("seed {seed}, {step}"));
        }
    }
}

/// Manual calculation: several edits, then one partial evaluation. No
/// loops: with one, a full evaluation decides (see `kantor_evaluate_only`).
#[test]
fn partial_recalculation_matches_full_after_many_edits() {
    let mut partial = 0;
    for seed in 100..125 {
        let (mut model, mut rng) = random_workbook(seed, true);
        model.pause_evaluation();
        for round in 0..6 {
            let mut steps = Vec::new();
            for _ in 0..1 + rng.below(4) {
                steps.push(edit(&mut model, &mut rng, true));
            }
            if model.kantor_evaluate_changes() {
                partial += 1;
            }
            check(&mut model, &format!("seed {seed} round {round}: {steps:?}"));
        }
    }
    assert!(partial > 75, "only {partial} partial evaluations");
}

/// A long chain and a fill: what a fill changes is all that runs.
#[test]
fn a_fill_evaluates_only_its_dependents() {
    let mut model = UserModel::new_empty("chain", "en", "UTC", "en").unwrap();
    model.pause_evaluation();
    for row in 1..=2000 {
        model.set_user_input(0, row, 1, &row.to_string()).unwrap();
        // A running total, and a column nothing reads.
        let total = if row == 1 {
            "=A1".to_string()
        } else {
            format!("=B{}+A{row}", row - 1)
        };
        model.set_user_input(0, row, 2, &total).unwrap();
        model
            .set_user_input(0, row, 4, &format!("=A{row}*2"))
            .unwrap();
    }
    model.set_user_input(0, 1, 3, "=B1*10").unwrap();
    model.evaluate();
    let evaluations = model.get_model().kantor_evaluations();
    model
        .auto_fill_rows(
            &Area {
                sheet: 0,
                row: 1,
                column: 3,
                width: 1,
                height: 1,
            },
            1000,
        )
        .unwrap();
    assert_eq!(model.get_model().kantor_evaluations(), evaluations + 1);
    // The 999 new cells, nothing else.
    assert_eq!(model.get_model().kantor_last_partial(), Some(999));
    assert_eq!(
        model.get_formatted_cell_value(0, 1000, 3).unwrap(),
        "5005000"
    );
    // The fill changed C only: B and D were read, not run again. Change a
    // value under them behind the model's back and nothing notices.
    check(&mut model, "fill");
    model.set_user_input(0, 5, 1, "100").unwrap();
    assert!(model.kantor_evaluate_changes());
    // B5:B2000, D5 and C5:C1000.
    assert_eq!(
        model.get_model().kantor_last_partial(),
        Some(1996 + 1 + 996)
    );
    assert_eq!(
        model.get_formatted_cell_value(0, 2000, 2).unwrap(),
        "2001095"
    );
    assert_eq!(model.get_formatted_cell_value(0, 5, 4).unwrap(), "200");
    check(&mut model, "edit at the top of the chain");
}

/// Automatic calculation: a typed value reaches the formulas that read it.
#[test]
fn a_typed_value_updates_its_dependents() {
    let mut model = UserModel::new_empty("typed", "en", "UTC", "en").unwrap();
    model.set_user_input(0, 1, 1, "1").unwrap();
    model.set_user_input(0, 1, 2, "=A1*2").unwrap();
    model.set_user_input(0, 2, 2, "=SUM(B1,A:A)").unwrap();
    model.set_user_input(0, 1, 1, "5").unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 1, 2).unwrap(), "10");
    assert_eq!(model.get_formatted_cell_value(0, 2, 2).unwrap(), "15");
    assert!(model.get_model().kantor_last_partial().is_some());
    model.undo().unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 2, 2).unwrap(), "3");
    model.redo().unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 2, 2).unwrap(), "15");
}
