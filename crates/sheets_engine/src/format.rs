//! Cell formatting: font flags, horizontal alignment, number formats, column
//! widths and row heights, over IronCalc's style API.
//!
//! Every mutating method here is **one** IronCalc history entry (one undo
//! step), so each calls `history.edited()` exactly once.

use anyhow::{Result, anyhow, bail};
use std::borrow::Cow;
use std::sync::Arc;

use ironcalc_base::types::{
    BorderItem, BorderStyle, Cell, CellType, Color, HorizontalAlignment, Style, Theme,
    VerticalAlignment,
};

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
///
/// Colours are resolved (theme colours with their tint, indexed colours) to
/// `0xRRGGBB`; `None` means automatic (no fill, the default text colour).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CellStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub align: HAlign,
    pub valign: VAlign,
    /// Wrap text onto several lines inside the cell.
    pub wrap: bool,
    /// The raw Excel format code (`general`, `#,##0.00`, …).
    pub num_fmt: String,
    /// Background of a solid (or patterned, drawn solid) fill.
    pub fill: Option<Rgb>,
    /// Text colour; `None` also when it is the workbook default font's colour.
    pub font_color: Option<Rgb>,
    /// Font size in points (IronCalc keeps whole points).
    pub font_size: i32,
    /// Font family, or `None` for the workbook's default font (which the grid
    /// draws in the UI font).
    pub font_name: Option<Arc<str>>,
    pub borders: Borders,
}

impl CellStyle {
    pub(crate) fn from_ironcalc(s: &Style, defaults: &FontDefaults, theme: &Theme) -> Self {
        let alignment = s.alignment.as_ref();
        let font_color = resolve(&s.font.color, theme).filter(|c| Some(*c) != defaults.color);
        let font_name = (!s.font.name.eq_ignore_ascii_case(&defaults.name))
            .then(|| Arc::<str>::from(s.font.name.as_str()));
        let line = |b: &Option<BorderItem>| {
            b.as_ref().map(|b| BorderLine {
                style: LineStyle::from_ironcalc(&b.style),
                color: resolve(&b.color, theme),
            })
        };
        Self {
            bold: s.font.b,
            italic: s.font.i,
            underline: s.font.u,
            strikethrough: s.font.strike,
            align: alignment
                .map(|a| HAlign::from_ironcalc(&a.horizontal))
                .unwrap_or_default(),
            valign: alignment
                .map(|a| VAlign::from_ironcalc(&a.vertical))
                .unwrap_or_default(),
            wrap: alignment.is_some_and(|a| a.wrap_text),
            num_fmt: s.num_fmt.clone(),
            fill: resolve(&s.fill.color, theme),
            font_color,
            font_size: s.font.sz,
            font_name,
            borders: Borders {
                left: line(&s.border.left),
                right: line(&s.border.right),
                top: line(&s.border.top),
                bottom: line(&s.border.bottom),
            },
        }
    }

    pub fn number_format(&self) -> NumberFormat {
        NumberFormat::from_code(&self.num_fmt)
    }
}

/// A colour as `0xRRGGBB`.
pub type Rgb = u32;

/// Parse `#RRGGBB` (IronCalc's resolved form).
pub fn parse_rgb(hex: &str) -> Option<Rgb> {
    let digits = hex.strip_prefix('#')?;
    (digits.len() == 6)
        .then(|| u32::from_str_radix(digits, 16).ok())
        .flatten()
}

fn resolve(color: &Color, theme: &Theme) -> Option<Rgb> {
    match color {
        Color::None => None,
        c => parse_rgb(&c.to_rgb(theme)),
    }
}

/// What "default" means for fonts: the workbook's first font (Excel's
/// "Normal" style font).
#[derive(Debug, Clone, Default)]
pub(crate) struct FontDefaults {
    pub name: String,
    pub size: i32,
    pub color: Option<Rgb>,
}

/// Vertical alignment of a cell's text. Excel's default is the bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VAlign {
    Top,
    Center,
    #[default]
    Bottom,
}

impl VAlign {
    fn from_ironcalc(v: &VerticalAlignment) -> Self {
        match v {
            VerticalAlignment::Top => Self::Top,
            VerticalAlignment::Center
            | VerticalAlignment::Justify
            | VerticalAlignment::Distributed => Self::Center,
            VerticalAlignment::Bottom => Self::Bottom,
        }
    }

    fn ironcalc(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Center => "center",
            Self::Bottom => "bottom",
        }
    }
}

/// How a border edge is drawn. Hair lines are drawn thin; thin dashed,
/// dash-dot and dash-dot-dot lines dotted (files keep their real style).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineStyle {
    Thin,
    Medium,
    Thick,
    Double,
    Dotted,
    /// Medium dashed, dash-dot and dash-dot-dot.
    MediumDashed,
}

impl LineStyle {
    fn from_ironcalc(s: &BorderStyle) -> Self {
        match s {
            BorderStyle::Thin | BorderStyle::Hair => Self::Thin,
            BorderStyle::Medium => Self::Medium,
            BorderStyle::Thick => Self::Thick,
            BorderStyle::Double => Self::Double,
            BorderStyle::Dotted
            | BorderStyle::Dashed
            | BorderStyle::DashDot
            | BorderStyle::DashDotDot => Self::Dotted,
            BorderStyle::SlantDashDot
            | BorderStyle::MediumDashed
            | BorderStyle::MediumDashDot
            | BorderStyle::MediumDashDotDot => Self::MediumDashed,
        }
    }

    /// Line width in pixels on screen.
    pub fn width(self) -> f32 {
        match self {
            Self::Thin | Self::Dotted => 1.0,
            Self::Medium | Self::MediumDashed => 2.0,
            Self::Thick | Self::Double => 3.0,
        }
    }

    /// Which line wins where two cells' borders meet, as in Excel: the
    /// heavier one.
    fn weight(self) -> u8 {
        match self {
            Self::Dotted => 1,
            Self::Thin => 2,
            Self::MediumDashed => 3,
            Self::Medium => 4,
            Self::Double => 5,
            Self::Thick => 6,
        }
    }
}

/// One border edge. `color: None` is automatic (black).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BorderLine {
    pub style: LineStyle,
    pub color: Option<Rgb>,
}

impl BorderLine {
    /// The edge two neighbouring cells share: `mine` (this cell's side) or
    /// `theirs` (the neighbour's side facing it), whichever is heavier; on a
    /// tie, the coloured one, then this cell's.
    pub fn shared(mine: Option<Self>, theirs: Option<Self>) -> Option<Self> {
        match (mine, theirs) {
            (Some(a), Some(b)) => {
                let key = |l: &Self| (l.style.weight(), l.color.is_some());
                Some(if key(&b) > key(&a) { b } else { a })
            }
            (a, b) => a.or(b),
        }
    }
}

/// A cell's own border edges (before sharing with neighbours).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Borders {
    pub left: Option<BorderLine>,
    pub right: Option<BorderLine>,
    pub top: Option<BorderLine>,
    pub bottom: Option<BorderLine>,
}

/// Which borders a border command draws over a range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderPreset {
    /// Every edge of every cell.
    All,
    /// The outline of the range.
    Outline,
    /// Remove every border in the range (and the neighbours' edges facing it).
    None,
}

impl BorderPreset {
    fn ironcalc(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Outline => "Outer",
            Self::None => "None",
        }
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
    VAlign(VAlign),
    Wrap(bool),
    /// `Custom` is rejected: it has no code to apply.
    NumberFormat(NumberFormat),
    /// `None` removes the fill.
    Fill(Option<Rgb>),
    /// `None` goes back to automatic.
    FontColor(Option<Rgb>),
    /// Grow (or shrink) every cell's font by this many points.
    FontSizeDelta(i32),
}

impl StyleChange {
    fn path_and_value(self) -> Result<(&'static str, Cow<'static, str>)> {
        let flag = |b: bool| Cow::Borrowed(if b { "true" } else { "false" });
        let color = |c: Option<Rgb>| match c {
            Some(c) => Cow::Owned(format!("#{:06X}", c & 0xFF_FFFF)),
            None => Cow::Borrowed(""),
        };
        Ok(match self {
            Self::Bold(b) => ("font.b", flag(b)),
            Self::Italic(b) => ("font.i", flag(b)),
            Self::Underline(b) => ("font.u", flag(b)),
            Self::Strikethrough(b) => ("font.strike", flag(b)),
            Self::Align(a) => ("alignment.horizontal", a.ironcalc().into()),
            Self::VAlign(a) => ("alignment.vertical", a.ironcalc().into()),
            Self::Wrap(b) => ("alignment.wrap_text", flag(b)),
            Self::NumberFormat(f) => (
                "num_fmt",
                f.code()
                    .ok_or_else(|| anyhow!("no format code to apply"))?
                    .into(),
            ),
            Self::Fill(c) => ("fill.fg_color", color(c)),
            Self::FontColor(c) => ("font.color", color(c)),
            Self::FontSizeDelta(d) => ("font.size_delta", d.to_string().into()),
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
    /// The formatting of a cell (including row and column styles). Cached
    /// per style index, so this is cheap enough for every visible cell.
    pub fn cell_style(&self, sheet: u32, row: u32, col: u32) -> Arc<CellStyle> {
        let index = self.style_index(sheet, row, col);
        self.style_by_index(index)
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
        self.display_align_of(&self.cell_style(sheet, row, col), sheet, row, col)
    }

    /// [`Self::display_align`] for a cell whose style is already at hand.
    pub fn display_align_of(&self, style: &CellStyle, sheet: u32, row: u32, col: u32) -> HAlign {
        match style.align {
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
        check_style_range(range)?;
        let (path, value) = change.path_and_value()?;
        self.model
            .update_range_style(&range.area(sheet), path, &value)
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(())
    }

    /// Draw `preset` borders (thin, automatic colour) over `range`, as one
    /// undo step.
    pub fn set_range_borders(
        &mut self,
        sheet: u32,
        range: CellRange,
        preset: BorderPreset,
    ) -> Result<()> {
        check_style_range(range)?;
        // `BorderArea`'s fields are private to IronCalc; it is built through
        // its serde form.
        let area: ironcalc_base::BorderArea = serde_json::from_value(serde_json::json!({
            "item": { "style": "thin", "color": "#000000" },
            "type": preset.ironcalc(),
        }))?;
        self.model
            .set_area_with_border(&range.area(sheet), &area)
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(())
    }

    /// Merge `range` into one cell, centred ("Merge & Center"), as one undo
    /// step. Like IronCalc (and unlike Excel, which asks first), it refuses
    /// when more than one of the cells has content.
    pub fn merge_range(&mut self, sheet: u32, range: CellRange) -> Result<()> {
        if range.is_single() {
            bail!("select more than one cell to merge");
        }
        check_style_range(range)?;
        self.model
            .merge_cells_center(&range.area(sheet))
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(())
    }

    /// Unmerge every merged cell touching `range`, as one undo step. Returns
    /// whether there was any.
    pub fn unmerge_range(&mut self, sheet: u32, range: CellRange) -> Result<bool> {
        if self.merges_in(sheet, range).is_empty() {
            return Ok(false);
        }
        self.model
            .unmerge_cells(&range.area(sheet))
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(true)
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

fn check_style_range(range: CellRange) -> Result<()> {
    if range.bottom >= MAX_ROWS || range.right >= MAX_COLS {
        bail!("range out of bounds");
    }
    let cells = range.rows_count() as u64 * range.cols_count() as u64;
    if cells > MAX_STYLED_CELLS {
        bail!("too many cells to format at once ({cells}); select fewer");
    }
    Ok(())
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
        // Halves round away from zero, as in Excel.
        let cases = [
            (NumberFormat::Number, "1,234.50"),
            (NumberFormat::Percent, "123450.00%"),
            (NumberFormat::CurrencyIdr, "Rp1,235"),
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
