//! The formula bar in Indonesian: a date whose format writes the month as a
//! name (`15-Des-2026`) is edited with the Indonesian name
//! (`15 Desember 2026`), any other date as Indonesia's short date
//! (`15/12/2026`), and pressing Enter keeps the same date and format.
//! Typed Indonesian month names give dates. Its own test binary: the input
//! and display locales are global (every test here sets the same ones).

use sheets_engine::{CellRange, ValueKind, Workbook};

/// 15 December 2026.
const DEC_15: f64 = 46_371.0;

fn indonesian() -> Workbook {
    sheets_engine::set_input_locale("id");
    sheets_engine::set_number_locale(kantor_numfmt::DisplayLocale::Indonesian);
    Workbook::new_empty().unwrap()
}

/// Enter on the formula bar text of `row`: same value, format and text.
fn round_trip(wb: &mut Workbook, row: u32) {
    let state = |wb: &Workbook| {
        (
            wb.input_for_editing(0, row, 0),
            wb.number_at(0, row, 0),
            wb.cell_style(0, row, 0).num_fmt.clone(),
            wb.display(0, row, 0).text,
        )
    };
    let before = state(wb);
    let typed = sheets_engine::delocalize_typed(&before.0).into_owned();
    wb.set_input(0, row, 0, &typed).unwrap();
    assert_eq!(state(wb), before, "row {row}");
}

#[test]
fn indonesian_date_formula_bar_text() {
    let mut wb = indonesian();
    wb.set_input(0, 0, 0, "15-Des-2026").unwrap();
    wb.set_input(0, 1, 0, "16/11/2026 14:05").unwrap();
    wb.set_input(0, 2, 0, "15/12/2026").unwrap();
    assert_eq!(wb.display(0, 0, 0).text, "15-Des-2026");
    assert_eq!(wb.input(0, 0, 0), "15 Desember 2026");
    assert_eq!(wb.input_for_editing(0, 0, 0), "15 Desember 2026");
    assert_eq!(wb.input(0, 1, 0), "16/11/2026 14:05:00");
    assert_eq!(wb.input(0, 2, 0), "15/12/2026");
    for row in 0..3 {
        round_trip(&mut wb, row);
    }
}

#[test]
fn indonesian_month_name_formats() {
    let mut wb = indonesian();
    for (row, (code, shown)) in [
        ("d mmmm yyyy", "15 Desember 2026"),
        ("d-mmm-yy", "15-Des-26"),
        ("mmmm d, yyyy", "Desember 15, 2026"),
        ("dddd, d mmmm yyyy", "Selasa, 15 Desember 2026"),
        ("mmm-yy", "Des-26"),
        ("[$-409]d mmmm yyyy", "15 December 2026"),
    ]
    .into_iter()
    .enumerate()
    {
        let row = row as u32;
        wb.set_input(0, row, 0, "15/12/2026").unwrap();
        let cell = CellRange::from_corners((row, 0), (row, 0));
        wb.set_range_style_value(0, cell, "num_fmt", code).unwrap();
        assert_eq!(wb.display(0, row, 0).text, shown, "{code}");
        assert_eq!(wb.input(0, row, 0), "15 Desember 2026", "{code}");
        round_trip(&mut wb, row);
        assert_eq!(wb.number_at(0, row, 0), Some((DEC_15, true)), "{code}");
    }
    // With a time.
    wb.set_input(0, 9, 0, "15/12/2026 14:05").unwrap();
    let cell = CellRange::from_corners((9, 0), (9, 0));
    wb.set_range_style_value(0, cell, "num_fmt", "d mmmm yyyy h:mm")
        .unwrap();
    assert_eq!(wb.input(0, 9, 0), "15 Desember 2026 14:05:00");
    round_trip(&mut wb, 9);
}

#[test]
fn typed_indonesian_month_names() {
    let mut wb = indonesian();
    for (row, (typed, serial, code)) in [
        ("15 Desember 2026", DEC_15, "d mmmm yyyy"),
        ("15 desember 2026", DEC_15, "d mmmm yyyy"),
        ("15 DES 2026", DEC_15, "d mmm yyyy"),
        // With `-` or `/` IronCalc's own reader gives a two-digit day `dd`.
        ("15-des-26", DEC_15, "dd-mmm-yy"),
        ("Desember 15, 2026", DEC_15, "mmmm d, yyyy"),
        ("17 Agustus 2026", DEC_15 - 120.0, "d mmmm yyyy"),
        ("17 Agt 2026", DEC_15 - 120.0, "d mmm yyyy"),
        ("17/Agu/2026", DEC_15 - 120.0, "dd/mmm/yyyy"),
        ("1 Mei 2026", DEC_15 - 228.0, "d mmmm yyyy"),
        ("2 Okt 2026", DEC_15 - 74.0, "d mmm yyyy"),
        ("15 December 2026", DEC_15, "d mmmm yyyy"),
    ]
    .into_iter()
    .enumerate()
    {
        let row = row as u32;
        wb.set_input(0, row, 0, typed).unwrap();
        assert_eq!(wb.number_at(0, row, 0), Some((serial, true)), "{typed}");
        assert_eq!(wb.cell_style(0, row, 0).num_fmt, code, "{typed}");
        round_trip(&mut wb, row);
    }
    // Shown with the Indonesian names, edited back as such.
    assert_eq!(wb.display(0, 0, 0).text, "15 Desember 2026");
    assert_eq!(wb.display(0, 7, 0).text, "17/Agu/2026");
    assert_eq!(wb.input(0, 7, 0), "17 Agustus 2026");
    // A time after a month-name date.
    wb.set_input(0, 20, 0, "15 Desember 2026 14:05").unwrap();
    assert_eq!(wb.cell_style(0, 20, 0).num_fmt, "d mmmm yyyy h:mm");
    assert_eq!(wb.input(0, 20, 0), "15 Desember 2026 14:05:00");
    round_trip(&mut wb, 20);
}

#[test]
fn indonesian_text_with_month_names_stays_text() {
    let mut wb = indonesian();
    for (row, text) in [
        "Mei Ling",
        "Mar 2026 report",
        "Juni",
        "Maret Agustus",
        "Des 2026 akhir",
    ]
    .into_iter()
    .enumerate()
    {
        wb.set_input(0, row as u32, 0, text).unwrap();
        assert_eq!(wb.value_kind(0, row as u32, 0), ValueKind::Text, "{text}");
        assert_eq!(wb.input(0, row as u32, 0), text);
    }
}
