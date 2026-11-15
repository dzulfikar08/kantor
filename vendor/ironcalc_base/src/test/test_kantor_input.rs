#![allow(clippy::unwrap_used)]
//! KANTOR PATCH (typed times): typed times and dates with times are stored
//! as numbers with a time format, as Excel does.

use crate::test::util::new_empty_model;

fn typed(input: &str) -> (f64, String) {
    let mut model = new_empty_model();
    model.set_user_input(0, 1, 1, input.to_string()).unwrap();
    model.evaluate();
    let cell = model
        .workbook
        .worksheet(0)
        .unwrap()
        .cell(1, 1)
        .map(|c| c.into_owned());
    let style = model.get_style_for_cell(0, 1, 1).unwrap();
    let v = match model.get_cell_value_by_index(0, 1, 1).unwrap() {
        crate::cell::CellValue::Number(n) => n,
        other => panic!("{input:?} stored as {other:?} ({cell:?})"),
    };
    (v, style.num_fmt)
}

fn secs(v: f64) -> i64 {
    (v * 86_400.0).round() as i64
}

#[test]
fn typed_times_are_numbers() {
    let (v, f) = typed("9:30");
    assert_eq!((secs(v), f.as_str()), (34_200, "h:mm"));
    let (v, f) = typed("9:30 PM");
    assert_eq!((secs(v), f.as_str()), (77_400, "h:mm AM/PM"));
    let (v, f) = typed("14:05:30");
    assert_eq!((secs(v), f.as_str()), (50_730, "h:mm:ss"));
    let (v, f) = typed("27:15");
    assert_eq!((secs(v), f.as_str()), (98_100, "[h]:mm:ss"));
}

#[test]
fn typed_date_and_time() {
    // 2026-11-16 is serial 46342.
    let (v, f) = typed("11/16/2026 9:30");
    assert_eq!(v.trunc(), 46342.0);
    assert_eq!(secs(v.fract()), 34_200);
    assert_eq!(f, "mm/dd/yyyy h:mm");
    let (v, _) = typed("2026-11-16 14:05");
    assert_eq!(v.trunc(), 46342.0);
}

#[test]
fn time_arithmetic_and_display() {
    let mut model = new_empty_model();
    model.set_user_input(0, 1, 1, "9:30".to_string()).unwrap();
    model.set_user_input(0, 2, 1, "17:15".to_string()).unwrap();
    model.set_user_input(0, 3, 1, "=A2-A1".to_string()).unwrap();
    model.evaluate();
    assert_eq!(model.get_formatted_cell_value(0, 1, 1).unwrap(), "9:30");
    assert_eq!(model.get_formatted_cell_value(0, 3, 1).unwrap(), "7:45");
}

#[test]
fn a_cell_that_shows_times_keeps_its_format() {
    let mut model = new_empty_model();
    model
        .set_user_input(0, 1, 1, "9:30:00 AM".to_string())
        .unwrap();
    model.set_user_input(0, 1, 1, "14:00".to_string()).unwrap();
    let style = model.get_style_for_cell(0, 1, 1).unwrap();
    assert_eq!(style.num_fmt, "h:mm:ss AM/PM");
    // A time typed over a date takes the time's format.
    model
        .set_user_input(0, 2, 1, "11/16/2026".to_string())
        .unwrap();
    model.set_user_input(0, 2, 1, "8:00".to_string()).unwrap();
    assert_eq!(model.get_style_for_cell(0, 2, 1).unwrap().num_fmt, "h:mm");
}

#[test]
fn not_times() {
    let mut model = new_empty_model();
    for text in ["9:5", "ab:cd", "Room 12:30a-b", "12:61", "item", "spam"] {
        model.set_user_input(0, 1, 1, text.to_string()).unwrap();
        model.evaluate();
        assert_eq!(
            model.get_cell_value_by_index(0, 1, 1).unwrap(),
            crate::cell::CellValue::String(text.to_string()),
            "{text}"
        );
    }
}

#[test]
fn indonesian_input() {
    let model = new_empty_model();
    let p = |s: &str| {
        model
            .kantor_parse_input_in(s, &["$", "€", "Rp"], true)
            .unwrap()
    };
    let (v, f) = p("16/11/2026 9:30");
    assert_eq!(v.trunc(), 46342.0);
    assert_eq!(secs(v.fract()), 34_200);
    assert_eq!(f.as_deref(), Some("dd/mm/yyyy h:mm"));
    let (v, f) = p("16/11/2026");
    assert_eq!((v, f.as_deref()), (46342.0, Some("dd/mm/yyyy")));
    let (v, f) = p("16-Agu-2026");
    assert_eq!(f.as_deref(), Some("dd-mmm-yyyy"));
    assert_eq!(v, p("16/08/2026").0);
    let (v, _) = p("1 Mei 2026".replace(' ', "-").as_str());
    assert_eq!(v, p("01/05/2026").0);
    let (v, f) = p("9.30".replace('.', ":").as_str());
    assert_eq!((secs(v), f.as_deref()), (34_200, Some("h:mm")));
    // Numbers are still numbers.
    assert_eq!(p("12.5").0, 12.5);
}
