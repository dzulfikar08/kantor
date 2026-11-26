//! Dates with month names in English (the default input locale): the
//! formula bar writes a date whose format shows the month's name with the
//! English name (`15 December 2026`, `December 15, 2026`), typing such text
//! gives a date, and pressing Enter on the formula bar text keeps the date
//! and the cell's format. Its own test binary: the input locale is global.

use sheets_engine::{CellRange, ValueKind, Workbook};

/// 15 December 2026.
const DEC_15: f64 = 46_371.0;

fn formatted(code: &str, typed: &str) -> Workbook {
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_input(0, 0, 0, typed).unwrap();
    let a1 = CellRange::from_corners((0, 0), (0, 0));
    wb.set_range_style_value(0, a1, "num_fmt", code).unwrap();
    wb
}

/// Enter on the formula bar text: same value, same format, same text.
fn round_trip(wb: &mut Workbook) {
    let before = (
        wb.input_for_editing(0, 0, 0),
        wb.number_at(0, 0, 0),
        wb.cell_style(0, 0, 0).num_fmt.clone(),
        wb.display(0, 0, 0).text,
    );
    let typed = sheets_engine::delocalize_typed(&before.0).into_owned();
    wb.set_input(0, 0, 0, &typed).unwrap();
    let after = (
        wb.input_for_editing(0, 0, 0),
        wb.number_at(0, 0, 0),
        wb.cell_style(0, 0, 0).num_fmt.clone(),
        wb.display(0, 0, 0).text,
    );
    assert_eq!(after, before);
}

#[test]
fn month_name_formats_show_month_names() {
    for (code, shown, input) in [
        ("d mmmm yyyy", "15 December 2026", "15 December 2026"),
        ("d-mmm-yy", "15-Dec-26", "15 December 2026"),
        ("dd mmm yyyy", "15 Dec 2026", "15 December 2026"),
        ("mmmm d, yyyy", "December 15, 2026", "December 15, 2026"),
        (
            "dddd, mmmm d, yyyy",
            "Tuesday, December 15, 2026",
            "December 15, 2026",
        ),
        ("mmm-yy", "Dec-26", "December 15, 2026"),
        ("[$-421]d mmmm yyyy", "15 Desember 2026", "15 December 2026"),
    ] {
        let mut wb = formatted(code, "12/15/2026");
        assert_eq!(wb.display(0, 0, 0).text, shown, "{code}");
        assert_eq!(wb.input(0, 0, 0), input, "{code}");
        round_trip(&mut wb);
        assert_eq!(wb.number_at(0, 0, 0), Some((DEC_15, true)), "{code}");
    }
}

#[test]
fn month_name_date_with_time() {
    let mut wb = formatted("d mmmm yyyy h:mm", "12/15/2026 14:05");
    assert_eq!(wb.display(0, 0, 0).text, "15 December 2026 14:05");
    assert_eq!(wb.input(0, 0, 0), "15 December 2026 2:05:00 PM");
    round_trip(&mut wb);
    let mut wb = formatted("mmm d, yyyy h:mm AM/PM", "12/15/2026 14:05");
    assert_eq!(wb.input(0, 0, 0), "December 15, 2026 2:05:00 PM");
    round_trip(&mut wb);
}

#[test]
fn numeric_date_formats_keep_the_short_date() {
    for code in ["m/d/yyyy", "yyyy-mm-dd", "dd/mm/yy"] {
        let mut wb = formatted(code, "12/15/2026");
        assert_eq!(wb.input(0, 0, 0), "12/15/2026", "{code}");
        round_trip(&mut wb);
    }
}

#[test]
fn typed_month_names_give_dates() {
    for (typed, code, shown) in [
        ("15 December 2026", "d mmmm yyyy", "15 December 2026"),
        ("15 december 2026", "d mmmm yyyy", "15 December 2026"),
        ("December 15, 2026", "mmmm d, yyyy", "December 15, 2026"),
        ("Dec 15, 2026", "mmm d, yyyy", "Dec 15, 2026"),
        ("15 Dec 2026", "d mmm yyyy", "15 Dec 2026"),
        ("15-dec-2026", "d-mmm-yyyy", "15-Dec-2026"),
        ("15/Dec/26", "d/mmm/yy", "15/Dec/26"),
    ] {
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(0, 0, 0, typed).unwrap();
        assert_eq!(wb.number_at(0, 0, 0), Some((DEC_15, true)), "{typed}");
        assert_eq!(wb.cell_style(0, 0, 0).num_fmt, code, "{typed}");
        assert_eq!(wb.display(0, 0, 0).text, shown, "{typed}");
        round_trip(&mut wb);
    }
    // Without a year: this year, as in Excel.
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_input(0, 0, 0, "15 Dec").unwrap();
    wb.set_input(0, 0, 1, "=YEAR(A1)=YEAR(TODAY())").unwrap();
    wb.set_input(0, 0, 2, "=DAY(A1)*100+MONTH(A1)").unwrap();
    assert_eq!(wb.display(0, 0, 1).text, "TRUE");
    assert_eq!(wb.display(0, 0, 2).text, "1512");
    assert_eq!(wb.cell_style(0, 0, 0).num_fmt, "d mmm");
    // Month and year: the 1st.
    wb.set_input(0, 1, 0, "December 2026").unwrap();
    assert_eq!(wb.number_at(0, 1, 0), Some((DEC_15 - 14.0, true)));
    // Into a cell that already shows dates: its format stays.
    let mut wb = formatted("yyyy-mm-dd", "1/1/2026");
    wb.set_input(0, 0, 0, "December 15, 2026").unwrap();
    assert_eq!(wb.number_at(0, 0, 0), Some((DEC_15, true)));
    assert_eq!(wb.display(0, 0, 0).text, "2026-12-15");
}

#[test]
fn text_with_month_names_stays_text() {
    let mut wb = Workbook::new_empty().unwrap();
    for (row, text) in [
        "Mei Ling",
        "Mar 2026 report",
        "May",
        "March madness",
        "15 Desember 2026",
        "Dec 15 2026 2026",
        "Room 15 May",
    ]
    .into_iter()
    .enumerate()
    {
        wb.set_input(0, row as u32, 0, text).unwrap();
        assert_eq!(wb.value_kind(0, row as u32, 0), ValueKind::Text, "{text}");
        assert_eq!(wb.input(0, row as u32, 0), text);
    }
}
