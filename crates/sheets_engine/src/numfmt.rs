//! Cell values shown the way Excel shows them, through `kantor_numfmt` (adapted from
//! GridCraft): fractions, elapsed time, conditions, colours, text sections, fill, spacing and
//! locale tags (`[$Rp-421]#,##0`, `[$-421]d mmmm yyyy`), which IronCalc's formatter gets wrong
//! or rejects.
//!
//! Format codes are compiled once per style index (see `look.rs`), because the grid formats
//! every visible cell on every frame. A code `kantor_numfmt` finds invalid (one Excel rejects,
//! or one longer than 255 characters) falls back to IronCalc's formatter.

use std::collections::HashMap;

use ironcalc_base::Model;
use ironcalc_base::cell::CellValue;
use ironcalc_base::types::CellType;
use kantor_numfmt::{DateSystem, NumberFormat as Code};

use crate::format::Rgb;

/// A cell's value as the grid draws it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Display {
    pub text: String,
    /// Colour from a `[Red]` / `[Color10]` section, `0xRRGGBB`.
    pub color: Option<Rgb>,
    /// Repeat character from `*x` and its byte position in `text`: the character is repeated
    /// there to fill the cell's width (`_("Rp"* #,##0_)` puts `Rp` at the left edge).
    pub fill: Option<(char, usize)>,
}

impl Display {
    fn plain(text: String) -> Self {
        Self {
            text,
            color: None,
            fill: None,
        }
    }
}

/// The format code a style shows numbers with. A built-in `numFmtId` the workbook does not
/// define shows as Excel's en-US built-in (14 is `m/d/yyyy`, which IronCalc's table has as
/// `mm-dd-yy`); otherwise the style's code.
fn style_code(model: &Model<'_>, style_index: i32) -> String {
    let styles = &model.workbook.styles;
    let Some(xf) = styles.cell_xfs.get(style_index.max(0) as usize) else {
        return "general".to_owned();
    };
    let id = xf.num_fmt_id;
    if let Some(f) = styles.num_fmts.iter().find(|f| f.num_fmt_id == id) {
        return f.format_code.clone();
    }
    u32::try_from(id)
        .ok()
        .and_then(kantor_numfmt::builtin_format)
        .map(str::to_owned)
        .or_else(|| styles.get_style(style_index).ok().map(|s| s.num_fmt))
        .unwrap_or_else(|| "general".to_owned())
}

/// The compiled number format of a style.
pub(crate) fn compile(model: &Model<'_>, style_index: i32) -> Code {
    Code::parse(&style_code(model, style_index))
}

/// Compiled formats by style index, for one-off passes over a model (CSV export).
#[derive(Default)]
pub(crate) struct Formats(HashMap<i32, Code>);

impl Formats {
    pub(crate) fn get(&mut self, model: &Model<'_>, style_index: i32) -> &Code {
        self.0
            .entry(style_index)
            .or_insert_with(|| compile(model, style_index))
    }
}

/// How the cell at 1-based `(row, col)` shows its value with format `fmt`.
pub(crate) fn display_cell(
    model: &Model<'_>,
    sheet: u32,
    row: i32,
    col: i32,
    fmt: &Code,
) -> Display {
    let Ok(ws) = model.workbook.worksheet(sheet) else {
        return Display::default();
    };
    let Some(cell) = ws.cell(row, col) else {
        return Display::default();
    };
    if !fmt.is_valid() {
        return Display::plain(
            model
                .get_formatted_cell_value(sheet, row, col)
                .unwrap_or_default(),
        );
    }
    let kind = cell.get_type();
    let shown = match cell.value(&model.workbook.shared_strings, language()) {
        CellValue::None => return Display::default(),
        CellValue::Number(n) => fmt.display_number(n, DateSystem::D1900),
        // Errors (and an unevaluated formula's `#ERROR!`) show as they are.
        CellValue::String(s) if kind != CellType::Text || !fmt.formats_text() => {
            return Display::plain(s);
        }
        CellValue::String(s) => fmt.display_text(&s),
        CellValue::Boolean(b) => {
            let names = &language().booleans;
            return Display::plain(if b { &names.r#true } else { &names.r#false }.clone());
        }
    };
    Display {
        color: shown
            .color
            .and_then(|c| c.rgb())
            .map(|[r, g, b]| (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)),
        fill: shown.fill,
        text: shown.text,
    }
}

/// The cell at 1-based `(row, col)`, formatted with its own style, for callers without a
/// [`crate::Workbook`] cache.
pub(crate) fn display_model_cell(model: &Model<'_>, sheet: u32, row: i32, col: i32) -> Display {
    let style = model
        .workbook
        .worksheet(sheet)
        .ok()
        .and_then(|ws| ws.cell(row, col).map(|c| c.get_style()))
        .unwrap_or(0);
    display_cell(model, sheet, row, col, &compile(model, style))
}

pub(crate) fn language() -> &'static ironcalc_base::language::Language {
    ironcalc_base::language::get_language(crate::LANGUAGE).expect("IronCalc has English")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CellRange, Workbook};

    /// A workbook with `value` in A1 shown with format `code`.
    fn shown(value: &str, code: &str) -> Display {
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(0, 0, 0, value).unwrap();
        let a1 = CellRange::from_corners((0, 0), (0, 0));
        wb.model
            .update_range_style(&a1.area(0), "num_fmt", code)
            .unwrap();
        wb.history.edited();
        wb.display(0, 0, 0)
    }

    #[test]
    fn excel_formats_in_the_grid() {
        let cases = [
            ("1.5", "# ?/?", "1 1/2"),
            ("1.5", "[h]:mm:ss", "36:00:00"),
            ("1500000", "[>=1000]#,##0,\"K\";0", "1,500K"),
            ("1500000", "[$Rp-421]#,##0", "Rp1.500.000"),
            ("1234.5", "[$-421]#,##0.00", "1.234,50"),
            (
                "45886",
                "[$-421]dddd, d mmmm yyyy",
                "Minggu, 17 Agustus 2025",
            ),
            ("1234", "#,##0_);(#,##0)", "1,234 "),
            ("Budi", "\"Nama: \"@", "Nama: Budi"),
            ("Budi", "0.00", "Budi"),
            ("TRUE", "0.00", "TRUE"),
            ("=1/0", "0.00;;;\"x\"@", "#DIV/0!"),
            ("", "0.00", ""),
        ];
        for (value, code, want) in cases {
            assert_eq!(shown(value, code).text, want, "{value:?} with {code:?}");
        }
    }

    #[test]
    fn colour_and_fill() {
        let red = shown("-5", "0.00;[Red]-0.00");
        assert_eq!((red.text.as_str(), red.color), ("-5.00", Some(0xFF0000)));
        assert_eq!(shown("5", "0.00;[Red]-0.00").color, None);
        let acc = shown(
            "1500",
            "_(\"Rp\"* #,##0_);_(\"Rp\"* \\(#,##0\\);_(\"Rp\"* \"-\"_);_(@_)",
        );
        assert_eq!((acc.text.as_str(), acc.fill), (" Rp1,500 ", Some((' ', 3))));
    }

    #[test]
    fn invalid_codes_fall_back_to_ironcalc() {
        // `B3` before a date is a code Excel rejects; IronCalc still shows something.
        let code = "B3dd";
        assert!(!Code::parse(code).is_valid());
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(0, 0, 0, "45886").unwrap();
        let a1 = CellRange::from_corners((0, 0), (0, 0));
        wb.model
            .update_range_style(&a1.area(0), "num_fmt", code)
            .unwrap();
        wb.history.edited();
        let ironcalc = wb.model.get_formatted_cell_value(0, 1, 1).unwrap();
        assert_eq!(wb.display_value(0, 0, 0), ironcalc);
    }
}
