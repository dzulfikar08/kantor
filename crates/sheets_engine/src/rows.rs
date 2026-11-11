//! Row heights, as Excel keeps them: a sheet default
//! (`<sheetFormatPr defaultRowHeight>`), and per row a height in points that
//! is either custom (`customHeight="1"`, set by the user) or automatic (the
//! height Excel fitted to the content, `ht` without `customHeight`), or
//! hidden.
//!
//! The grid decides what an automatic row looks like (it fits the row to its
//! wrapped text and larger fonts, see `sheets_ui`); the engine stores custom
//! heights, which are saved and undone like any edit.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use ironcalc_base::types::Cell;

use crate::{CellStyle, MAX_ROWS, Workbook};

/// Excel's default row height in points (Calibri 11).
pub const DEFAULT_ROW_POINTS: f64 = 15.0;

/// Excel's largest row height in points.
pub const MAX_ROW_POINTS: f64 = ironcalc_base::kantor_rows::MAX_ROW_HEIGHT;

/// One row the sheet says something about.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RowEntry {
    /// 0-based row.
    pub row: u32,
    /// Height in points: the user's (custom) or the last fitted one (auto).
    pub points: f64,
    pub custom: bool,
    pub hidden: bool,
}

/// The row heights of a sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct RowSpec {
    /// Height of a row the sheet says nothing about, in points.
    pub default_points: f64,
    /// Rows with an entry, sorted by row.
    pub rows: Vec<RowEntry>,
}

/// A new height for rows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RowHeight {
    /// A custom height in points (shows hidden rows).
    Points(f64),
    /// Follow the content again; the value is the fitted height in points,
    /// written as the row's `ht` when the row has other formatting to keep.
    Auto(f64),
}

/// A cell that can make an automatic row taller: wrapped text or a font
/// larger than the workbook's default.
#[derive(Debug, Clone)]
pub struct FitCell {
    pub col: u32,
    pub text: String,
    pub style: Arc<CellStyle>,
}

impl Workbook {
    /// The default row height of `sheet` in points.
    pub fn default_row_points(&self, sheet: u32) -> f64 {
        self.model
            .get_model()
            .workbook
            .worksheet(sheet)
            .ok()
            .and_then(|ws| ws.default_row_height)
            .unwrap_or(DEFAULT_ROW_POINTS)
    }

    /// Every row height `sheet` keeps, in one pass over its row entries
    /// (IronCalc looks rows up by scanning them, so asking row by row would
    /// be quadratic on a big sheet).
    pub fn row_spec(&self, sheet: u32) -> RowSpec {
        let default_points = self.default_row_points(sheet);
        let mut rows: Vec<RowEntry> = match self.model.get_model().workbook.worksheet(sheet) {
            Ok(ws) => ws
                .rows
                .iter()
                .filter(|r| r.r >= 1 && (r.r as u32) <= MAX_ROWS)
                .filter(|r| r.custom_height || r.hidden || r.height != default_points)
                .map(|r| RowEntry {
                    row: r.r as u32 - 1,
                    points: if r.height.is_finite() && r.height >= 0.0 {
                        r.height
                    } else {
                        default_points
                    },
                    custom: r.custom_height,
                    hidden: r.hidden,
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        rows.sort_by_key(|r| r.row);
        rows.dedup_by_key(|r| r.row);
        RowSpec {
            default_points,
            rows,
        }
    }

    /// Set the height of rows `first..=last` (0-based), as one undo step.
    /// Returns whether anything changed.
    pub fn set_row_heights(
        &mut self,
        sheet: u32,
        first: u32,
        last: u32,
        height: RowHeight,
    ) -> Result<bool> {
        if first > last || last >= MAX_ROWS {
            bail!("row out of range");
        }
        let (points, auto) = match height {
            RowHeight::Points(p) => {
                if !p.is_finite() || p <= 0.0 {
                    bail!("invalid row height");
                }
                (Some(p.min(MAX_ROW_POINTS)), 0.0)
            }
            RowHeight::Auto(p) => (None, p.clamp(0.0, MAX_ROW_POINTS)),
        };
        let changed = self
            .model
            .kantor_set_rows_height(sheet, first as i32 + 1, last as i32 + 1, points, auto)
            .map_err(|e| anyhow!("{e}"))?;
        if changed {
            self.history.edited();
        }
        Ok(changed)
    }

    /// The cells of `row` that can make it taller than a plain row (wrapped
    /// text, a larger font), with their displayed text. Merged cells don't
    /// count, as in Excel.
    pub fn row_fit_cells(&self, sheet: u32, row: u32) -> Vec<FitCell> {
        let model = self.model.get_model();
        let Ok(ws) = model.workbook.worksheet(sheet) else {
            return Vec::new();
        };
        let default_size = self.default_font_size();
        let cols: Vec<u32> = ws
            .sheet_data
            .cells_in_row(row as i32 + 1)
            .into_iter()
            .filter(|(c, cell)| *c >= 1 && !matches!(**cell, Cell::EmptyCell { .. }))
            .map(|(c, _)| c as u32 - 1)
            .collect();
        cols.into_iter()
            .filter_map(|col| {
                let style = self.cell_style(sheet, row, col);
                if !style.wrap && style.font_size <= default_size {
                    return None;
                }
                if self.merge_at(sheet, row, col).is_some() {
                    return None;
                }
                let text = self.display(sheet, row, col).text;
                (!text.is_empty()).then_some(FitCell { col, text, style })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_heights_and_auto_undo_exactly() {
        let mut wb = Workbook::new_empty().unwrap();
        assert_eq!(wb.row_spec(0).rows, vec![]);
        assert_eq!(wb.row_spec(0).default_points, DEFAULT_ROW_POINTS);
        assert!(
            wb.set_row_heights(0, 2, 3, RowHeight::Points(30.0))
                .unwrap()
        );
        let spec = wb.row_spec(0);
        assert_eq!(
            spec.rows,
            vec![
                RowEntry {
                    row: 2,
                    points: 30.0,
                    custom: true,
                    hidden: false
                },
                RowEntry {
                    row: 3,
                    points: 30.0,
                    custom: true,
                    hidden: false
                },
            ]
        );
        assert!(wb.is_dirty());
        // Back to automatic: the entries go (they held only a height).
        assert!(wb.set_row_heights(0, 2, 3, RowHeight::Auto(15.0)).unwrap());
        assert_eq!(wb.row_spec(0).rows, vec![]);
        // Nothing to change: no undo step.
        assert!(!wb.set_row_heights(0, 2, 3, RowHeight::Auto(15.0)).unwrap());
        assert!(wb.undo().unwrap());
        assert_eq!(wb.row_spec(0).rows.len(), 2);
        assert!(wb.undo().unwrap());
        assert_eq!(wb.row_spec(0).rows, vec![]);
        assert!(!wb.is_dirty());
        assert!(wb.redo().unwrap());
        assert_eq!(wb.row_spec(0).rows[0].points, 30.0);
    }

    #[test]
    fn bad_heights_are_refused() {
        let mut wb = Workbook::new_empty().unwrap();
        assert!(wb.set_row_heights(0, 0, 0, RowHeight::Points(0.0)).is_err());
        assert!(
            wb.set_row_heights(0, 0, 0, RowHeight::Points(f64::NAN))
                .is_err()
        );
        assert!(wb.set_row_heights(0, 3, 2, RowHeight::Points(20.0)).is_err());
        // Taller than Excel allows: capped.
        wb.set_row_heights(0, 0, 0, RowHeight::Points(1000.0))
            .unwrap();
        assert_eq!(wb.row_spec(0).rows[0].points, MAX_ROW_POINTS);
    }

    #[test]
    fn fit_cells_are_wrapped_or_large() {
        use crate::StyleChange;
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(0, 0, 0, "plain").unwrap();
        wb.set_input(0, 0, 1, "wrapped text").unwrap();
        wb.set_input(0, 0, 2, "big").unwrap();
        let one = |c| crate::CellRange::from_corners((0, c), (0, c));
        wb.set_range_style(0, one(1), StyleChange::Wrap(true))
            .unwrap();
        wb.set_range_style(0, one(2), StyleChange::FontSizeDelta(10))
            .unwrap();
        let cols: Vec<u32> = wb.row_fit_cells(0, 0).iter().map(|c| c.col).collect();
        assert_eq!(cols, vec![1, 2]);
        assert!(wb.row_fit_cells(0, 5).is_empty());
    }
}
