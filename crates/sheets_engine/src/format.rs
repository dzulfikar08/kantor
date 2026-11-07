//! Cell formatting: font flags, horizontal alignment, number formats, column
//! widths and row heights, over IronCalc's style API.
//!
//! Every mutating method here is **one** IronCalc history entry (one undo
//! step), so each calls `history.edited()` exactly once.

use anyhow::{Result, anyhow, bail};
use ironcalc_base::types::{Cell, CellType, HorizontalAlignment, Style};

use crate::{CellRange, MAX_COLS, MAX_ROWS, Workbook};

/// Styling more cells than this at once is refused: IronCalc stores a style
/// per cell, so a huge range would create that many cells (whole-column and
/// whole-row styles are not exposed yet).
pub const MAX_STYLED_CELLS: u64 = 250_000;

/// Smallest column width / row height we store, in pixels.
const MIN_SIZE: f64 = 4.0;

/// Horizontal alignment of a cell's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HAlign {
    /// Depends on the value, as in Excel: numbers right, booleans and errors
    /// centred, text left.
    #[default]
    General,
    Left,
    Center,
    Right,
}

impl HAlign {
    fn ironcalc(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Left => "left",
            Self::Center => "center",
            Self::Right => "right",
        }
    }

    fn from_ironcalc(h: &HorizontalAlignment) -> Self {
        match h {
            HorizontalAlignment::Left => Self::Left,
            HorizontalAlignment::Right => Self::Right,
            HorizontalAlignment::Center | HorizontalAlignment::CenterContinuous => Self::Center,
            // Fill, justify and distributed are drawn like their closest kin.
            HorizontalAlignment::Justify | HorizontalAlignment::Distributed => Self::Left,
            HorizontalAlignment::Fill | HorizontalAlignment::General => Self::General,
        }
    }
}

/// The number formats offered in the toolbar. Any other format code a file
/// brings along is kept and shown as [`NumberFormat::Custom`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberFormat {
    General,
    /// Thousands separators, 2 decimals: `1,234.50`.
    Number,
    /// `12.50%`.
    Percent,
    /// Indonesian rupiah, no decimals: `Rp1,250,000`.
    CurrencyIdr,
    /// US dollars: `$1,234.50`.
    CurrencyUsd,
    /// ISO date: `2026-11-07`.
    Date,
    /// A format code not in this list.
    Custom,
}

impl NumberFormat {
    /// The formats a user can pick (everything but `Custom`).
    pub const ALL: [NumberFormat; 6] = [
        Self::General,
        Self::Number,
        Self::Percent,
        Self::CurrencyIdr,
        Self::CurrencyUsd,
        Self::Date,
    ];

    /// The Excel format code. `Custom` has none.
    pub fn code(self) -> Option<&'static str> {
        Some(match self {
            Self::General => "general",
            Self::Number => "#,##0.00",
            Self::Percent => "0.00%",
            Self::CurrencyIdr => "\"Rp\"#,##0",
            Self::CurrencyUsd => "\"$\"#,##0.00",
            Self::Date => "yyyy-mm-dd",
            Self::Custom => return None,
        })
    }

    /// Which entry a format code is (case-insensitive for `General`).
    pub fn from_code(code: &str) -> Self {
        if code.is_empty() || code.eq_ignore_ascii_case("general") {
            return Self::General;
        }
        Self::ALL
            .into_iter()
            .find(|f| f.code() == Some(code))
            .unwrap_or(Self::Custom)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Number => "Number",
            Self::Percent => "Percent",
            Self::CurrencyIdr => "Rupiah (Rp)",
            Self::CurrencyUsd => "Dollar ($)",
            Self::Date => "Date",
            Self::Custom => "Custom",
        }
    }
}

/// The formatting of one cell, as the grid and toolbar need it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CellStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub align: HAlign,
    /// The raw Excel format code (`general`, `#,##0.00`, …).
    pub num_fmt: String,
}

impl CellStyle {
    fn from_ironcalc(s: &Style) -> Self {
        Self {
            bold: s.font.b,
            italic: s.font.i,
            underline: s.font.u,
            strikethrough: s.font.strike,
            align: s
                .alignment
                .as_ref()
                .map(|a| HAlign::from_ironcalc(&a.horizontal))
                .unwrap_or_default(),
            num_fmt: s.num_fmt.clone(),
        }
    }

    pub fn number_format(&self) -> NumberFormat {
        NumberFormat::from_code(&self.num_fmt)
    }
}

/// One formatting change, applied to a whole range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleChange {
    Bold(bool),
    Italic(bool),
    Underline(bool),
    Strikethrough(bool),
    Align(HAlign),
    /// `Custom` is rejected: it has no code to apply.
    NumberFormat(NumberFormat),
}

impl StyleChange {
    fn path_and_value(self) -> Result<(&'static str, &'static str)> {
        let flag = |b: bool| if b { "true" } else { "false" };
        Ok(match self {
            Self::Bold(b) => ("font.b", flag(b)),
            Self::Italic(b) => ("font.i", flag(b)),
            Self::Underline(b) => ("font.u", flag(b)),
            Self::Strikethrough(b) => ("font.strike", flag(b)),
            Self::Align(a) => ("alignment.horizontal", a.ironcalc()),
            Self::NumberFormat(f) => (
                "num_fmt",
                f.code().ok_or_else(|| anyhow!("no format code to apply"))?,
            ),
        })
    }
}

/// What kind of value a cell shows, for `General` alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Number,
    Text,
    Boolean,
    Error,
}

impl Workbook {
    /// The formatting of a cell (including row and column styles).
    pub fn cell_style(&self, sheet: u32, row: u32, col: u32) -> CellStyle {
        // `Model::get_style_for_cell` rather than `UserModel::get_cell_style`:
        // the latter also looks up four neighbours for borders, which the
        // grid does not draw yet, and this runs for every visible cell.
        self.model
            .get_model()
            .get_style_for_cell(sheet, row as i32 + 1, col as i32 + 1)
            .map(|s| CellStyle::from_ironcalc(&s))
            .unwrap_or_default()
    }

    /// The kind of value a cell shows (a formula counts as its result; an
    /// empty cell counts as text, though IronCalc calls it a number).
    pub fn value_kind(&self, sheet: u32, row: u32, col: u32) -> ValueKind {
        let model = self.model.get_model();
        let (r, c) = (row as i32 + 1, col as i32 + 1);
        let empty = match model.workbook.worksheet(sheet).map(|ws| ws.cell(r, c)) {
            Ok(Some(cell)) => matches!(*cell, Cell::EmptyCell { .. }),
            _ => true,
        };
        if empty {
            return ValueKind::Text;
        }
        match model.get_cell_type(sheet, r, c) {
            Ok(CellType::Number) => ValueKind::Number,
            Ok(CellType::LogicalValue) => ValueKind::Boolean,
            Ok(CellType::ErrorValue) => ValueKind::Error,
            _ => ValueKind::Text,
        }
    }

    /// How a cell's text is drawn: its alignment, with `General` resolved by
    /// value kind the way Excel does. Never returns `General`.
    pub fn display_align(&self, sheet: u32, row: u32, col: u32) -> HAlign {
        match self.cell_style(sheet, row, col).align {
            HAlign::General => match self.value_kind(sheet, row, col) {
                ValueKind::Number => HAlign::Right,
                ValueKind::Boolean | ValueKind::Error => HAlign::Center,
                ValueKind::Text => HAlign::Left,
            },
            other => other,
        }
    }

    /// Apply one formatting change to every cell of `range`, as one undo step.
    pub fn set_range_style(
        &mut self,
        sheet: u32,
        range: CellRange,
        change: StyleChange,
    ) -> Result<()> {
        if range.bottom >= MAX_ROWS || range.right >= MAX_COLS {
            bail!("range out of bounds");
        }
        let cells = (range.bottom - range.top + 1) as u64 * (range.right - range.left + 1) as u64;
        if cells > MAX_STYLED_CELLS {
            bail!("too many cells to format at once ({cells}); select fewer");
        }
        let (path, value) = change.path_and_value()?;
        self.model
            .update_range_style(&range.area(sheet), path, value)
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(())
    }

    /// Width of a column in pixels (IronCalc's unit; the default is 90).
    pub fn column_width(&self, sheet: u32, col: u32) -> f64 {
        self.model
            .get_column_width(sheet, col as i32 + 1)
            .unwrap_or(90.0)
    }

    /// Set the width (pixels) of columns `first..=last`, as one undo step.
    pub fn set_column_width(
        &mut self,
        sheet: u32,
        first: u32,
        last: u32,
        width: f64,
    ) -> Result<()> {
        if first > last || last >= MAX_COLS {
            bail!("column out of range");
        }
        if !width.is_finite() {
            bail!("invalid column width");
        }
        self.model
            .set_columns_width(
                sheet,
                first as i32 + 1,
                last as i32 + 1,
                width.max(MIN_SIZE),
            )
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(())
    }

    /// Height of a row in pixels (IronCalc's unit; the default is 25).
    pub fn row_height(&self, sheet: u32, row: u32) -> f64 {
        self.model
            .get_row_height(sheet, row as i32 + 1)
            .unwrap_or(25.0)
    }

    /// Set the height (pixels) of rows `first..=last`, as one undo step.
    pub fn set_row_height(&mut self, sheet: u32, first: u32, last: u32, height: f64) -> Result<()> {
        if first > last || last >= MAX_ROWS {
            bail!("row out of range");
        }
        if !height.is_finite() {
            bail!("invalid row height");
        }
        self.model
            .set_rows_height(
                sheet,
                first as i32 + 1,
                last as i32 + 1,
                height.max(MIN_SIZE),
            )
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a1() -> CellRange {
        CellRange::from_corners((0, 0), (0, 0))
    }

    #[test]
    fn number_formats_display() {
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(0, 0, 0, "1234.5").unwrap();
        // IronCalc rounds halves to even, so 1234.5 shows as Rp1,234 (Excel: Rp1,235).
        let cases = [
            (NumberFormat::Number, "1,234.50"),
            (NumberFormat::Percent, "123450.00%"),
            (NumberFormat::CurrencyIdr, "Rp1,234"),
            (NumberFormat::CurrencyUsd, "$1,234.50"),
            (NumberFormat::General, "1234.5"),
        ];
        for (fmt, shown) in cases {
            wb.set_range_style(0, a1(), StyleChange::NumberFormat(fmt))
                .unwrap();
            assert_eq!(wb.display_value(0, 0, 0), shown, "{fmt:?}");
            assert_eq!(wb.cell_style(0, 0, 0).number_format(), fmt);
        }
        // 46333 = 2026-11-07 in Excel's 1900 date system.
        wb.set_input(0, 1, 0, "46333").unwrap();
        wb.set_range_style(
            0,
            CellRange::from_corners((1, 0), (1, 0)),
            StyleChange::NumberFormat(NumberFormat::Date),
        )
        .unwrap();
        assert_eq!(wb.display_value(0, 1, 0), "2026-11-07");
        assert!(
            wb.set_range_style(0, a1(), StyleChange::NumberFormat(NumberFormat::Custom))
                .is_err()
        );
        assert_eq!(NumberFormat::from_code("0.000"), NumberFormat::Custom);
        assert_eq!(NumberFormat::from_code("General"), NumberFormat::General);
    }

    #[test]
    fn range_style_is_one_undo_step() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.xlsx");
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(0, 0, 0, "x").unwrap();
        wb.save_as(&path).unwrap();

        let r = CellRange::from_corners((0, 0), (2, 1));
        wb.set_range_style(0, r, StyleChange::Bold(true)).unwrap();
        assert!(wb.is_dirty());
        for (row, col) in [(0, 0), (2, 1), (1, 0)] {
            assert!(wb.cell_style(0, row, col).bold);
        }
        assert!(!wb.cell_style(0, 3, 0).bold);

        assert!(wb.undo().unwrap());
        assert!(!wb.cell_style(0, 0, 0).bold);
        assert!(!wb.cell_style(0, 2, 1).bold);
        assert!(!wb.is_dirty(), "one undo returns to the saved state");
        assert!(wb.redo().unwrap());
        assert!(wb.cell_style(0, 2, 1).bold);
        assert!(wb.is_dirty());

        wb.set_range_style(0, r, StyleChange::Bold(false)).unwrap();
        assert!(!wb.cell_style(0, 0, 0).bold);
        assert!(wb.undo().unwrap());
        assert!(wb.cell_style(0, 0, 0).bold);
    }

    #[test]
    fn flags_and_alignment() {
        let mut wb = Workbook::new_empty().unwrap();
        for change in [
            StyleChange::Italic(true),
            StyleChange::Underline(true),
            StyleChange::Strikethrough(true),
            StyleChange::Align(HAlign::Center),
        ] {
            wb.set_range_style(0, a1(), change).unwrap();
        }
        let s = wb.cell_style(0, 0, 0);
        assert!(s.italic && s.underline && s.strikethrough && !s.bold);
        assert_eq!(s.align, HAlign::Center);
        wb.set_range_style(0, a1(), StyleChange::Align(HAlign::General))
            .unwrap();
        assert_eq!(wb.cell_style(0, 0, 0).align, HAlign::General);
    }

    #[test]
    fn general_alignment_follows_value() {
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(0, 0, 0, "12").unwrap();
        wb.set_input(0, 0, 1, "text").unwrap();
        wb.set_input(0, 0, 2, "TRUE").unwrap();
        wb.set_input(0, 0, 3, "=1/0").unwrap();
        wb.set_input(0, 0, 4, "=A1*2").unwrap();
        assert_eq!(wb.display_align(0, 0, 0), HAlign::Right);
        assert_eq!(wb.display_align(0, 0, 1), HAlign::Left);
        assert_eq!(wb.display_align(0, 0, 2), HAlign::Center);
        assert_eq!(wb.display_align(0, 0, 3), HAlign::Center);
        assert_eq!(wb.display_align(0, 0, 4), HAlign::Right);
        assert_eq!(wb.display_align(0, 5, 5), HAlign::Left, "empty");
        wb.set_range_style(0, a1(), StyleChange::Align(HAlign::Left))
            .unwrap();
        assert_eq!(wb.display_align(0, 0, 0), HAlign::Left);
    }

    #[test]
    fn sizes_undo() {
        let mut wb = Workbook::new_empty().unwrap();
        let default_w = wb.column_width(0, 1);
        wb.set_column_width(0, 1, 2, 150.0).unwrap();
        assert_eq!(wb.column_width(0, 1), 150.0);
        assert_eq!(wb.column_width(0, 2), 150.0);
        assert_eq!(wb.column_width(0, 3), default_w);
        wb.set_row_height(0, 4, 4, 40.0).unwrap();
        assert_eq!(wb.row_height(0, 4), 40.0);

        assert!(wb.undo().unwrap());
        assert_eq!(wb.row_height(0, 4), wb.row_height(0, 5));
        assert!(wb.undo().unwrap());
        assert_eq!(wb.column_width(0, 1), default_w);
        assert_eq!(wb.column_width(0, 2), default_w);
        assert!(!wb.is_dirty());

        assert!(wb.set_column_width(0, 0, MAX_COLS, 10.0).is_err());
        assert!(wb.set_row_height(0, 3, 2, 10.0).is_err());
    }

    #[test]
    fn refuses_huge_ranges() {
        let mut wb = Workbook::new_empty().unwrap();
        let r = CellRange::from_corners((0, 0), (MAX_ROWS - 1, 1));
        assert!(wb.set_range_style(0, r, StyleChange::Bold(true)).is_err());
        assert!(!wb.can_undo());
        assert!(!wb.is_dirty());
    }

    #[test]
    fn styles_and_sizes_round_trip_xlsx() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fmt.xlsx");
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(0, 0, 0, "Title").unwrap();
        wb.set_input(0, 1, 0, "1250000").unwrap();
        wb.set_input(0, 1, 1, "0.125").unwrap();
        wb.set_range_style(0, a1(), StyleChange::Bold(true))
            .unwrap();
        wb.set_range_style(0, a1(), StyleChange::Italic(true))
            .unwrap();
        wb.set_range_style(0, a1(), StyleChange::Underline(true))
            .unwrap();
        wb.set_range_style(0, a1(), StyleChange::Strikethrough(true))
            .unwrap();
        wb.set_range_style(0, a1(), StyleChange::Align(HAlign::Center))
            .unwrap();
        let b = |r, c| CellRange::from_corners((r, c), (r, c));
        wb.set_range_style(
            0,
            b(1, 0),
            StyleChange::NumberFormat(NumberFormat::CurrencyIdr),
        )
        .unwrap();
        wb.set_range_style(0, b(1, 1), StyleChange::NumberFormat(NumberFormat::Percent))
            .unwrap();
        wb.set_column_width(0, 0, 0, 180.0).unwrap();
        wb.set_row_height(0, 0, 0, 40.0).unwrap();
        wb.save_as(&path).unwrap();

        let back = Workbook::open(&path).unwrap();
        let s = back.cell_style(0, 0, 0);
        assert!(s.bold && s.italic && s.underline && s.strikethrough);
        assert_eq!(s.align, HAlign::Center);
        assert_eq!(
            back.cell_style(0, 1, 0).number_format(),
            NumberFormat::CurrencyIdr
        );
        assert_eq!(back.display_value(0, 1, 0), "Rp1,250,000");
        assert_eq!(back.display_value(0, 1, 1), "12.50%");
        assert!(!back.cell_style(0, 1, 1).bold);
        assert!(
            (back.column_width(0, 0) - 180.0).abs() < 1.0,
            "{}",
            back.column_width(0, 0)
        );
        assert!((back.column_width(0, 1) - wb.column_width(0, 1)).abs() < 1.0);
        assert!(
            (back.row_height(0, 0) - 40.0).abs() < 1.0,
            "{}",
            back.row_height(0, 0)
        );
    }
}
