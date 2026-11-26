//! The formula bar in Indonesian: a date shown with month names in the grid
//! (`15-Des-2026`) is edited as Indonesia's short date (`15/12/2026`), not
//! with English month names, and pressing Enter keeps the same date and
//! format. Its own test binary: the input and display locales are global.

use sheets_engine::Workbook;

#[test]
fn indonesian_date_formula_bar_text() {
    sheets_engine::set_input_locale("id");
    sheets_engine::set_number_locale(kantor_numfmt::DisplayLocale::Indonesian);
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_input(0, 0, 0, "15-Des-2026").unwrap();
    wb.set_input(0, 1, 0, "16/11/2026 14:05").unwrap();
    assert_eq!(wb.display(0, 0, 0).text, "15-Des-2026");
    assert_eq!(wb.input(0, 0, 0), "15/12/2026");
    assert_eq!(wb.input_for_editing(0, 0, 0), "15/12/2026");
    assert_eq!(wb.input(0, 1, 0), "16/11/2026 14:05:00");
    for row in 0..2 {
        let (before, shown) = (wb.input_for_editing(0, row, 0), wb.display(0, row, 0).text);
        let typed = sheets_engine::delocalize_typed(&before).into_owned();
        wb.set_input(0, row, 0, &typed).unwrap();
        assert_eq!(wb.input(0, row, 0), before, "row {row}");
        assert_eq!(wb.display(0, row, 0).text, shown, "row {row}");
    }
}
