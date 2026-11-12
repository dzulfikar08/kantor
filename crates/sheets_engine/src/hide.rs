//! Hiding and unhiding rows and columns by hand, as Excel's Hide / Unhide
//! (Ctrl+9, Ctrl+Shift+9, Ctrl+0, Ctrl+Shift+0).
//!
//! Hidden rows and columns are what the file keeps (`<row hidden="1">`,
//! `<col hidden="1">`), and a row a filter hides is a hidden row too
//! (`filter.rs`). Excel tells them apart by where they are: while a filter
//! filters something, a hidden row among its data rows is *filtered out*,
//! and Unhide leaves it hidden (clearing or changing the filter shows it).
//! Kantor does the same: Unhide Rows skips the data rows of every filter
//! that has a condition, and says so. Hide works anywhere, filtered range or
//! not; the next Reapply of a filter shows the rows that match again, as in
//! Excel.
//!
//! Each command is one undo step; widths and heights are kept, so unhidden
//! rows and columns come back as they were.

use std::collections::BTreeSet;

use anyhow::{Result, anyhow, bail};

use crate::{MAX_COLS, MAX_ROWS, Workbook};

/// Most rows one Hide Rows may hide (each hidden row is an entry in the
/// sheet); a whole-column selection asks for a million.
const MAX_HIDE_ROWS: u32 = 65_536;

/// What an Unhide did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Unhidden {
    /// Rows or columns shown again.
    pub shown: u32,
    /// Hidden rows left hidden because a filter hides them.
    pub filtered: u32,
}

impl Workbook {
    /// Whether column `col` (0-based) of `sheet` is hidden.
    pub fn is_column_hidden(&self, sheet: u32, col: u32) -> bool {
        self.model
            .get_model()
            .is_column_hidden(sheet, col as i32 + 1)
            .unwrap_or(false)
    }

    /// Whether row `row` (0-based) of `sheet` is hidden (by hand or by a
    /// filter).
    pub fn is_row_hidden(&self, sheet: u32, row: u32) -> bool {
        self.model
            .get_model()
            .workbook
            .worksheet(sheet)
            .ok()
            .and_then(|ws| ws.rows.iter().find(|r| r.r == row as i32 + 1))
            .is_some_and(|r| r.hidden)
    }

    /// Hidden columns of `sheet` among `first..=last`, in order.
    pub fn hidden_columns(&self, sheet: u32, first: u32, last: u32) -> Vec<u32> {
        let Ok(ws) = self.model.get_model().workbook.worksheet(sheet) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for c in ws.cols.iter().filter(|c| c.hidden) {
            let lo = (c.min.max(1) as u32 - 1).max(first);
            let hi = (c.max.max(1) as u32 - 1).min(last);
            out.extend(lo..=hi.max(lo).min(hi));
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Hide rows `first..=last` (0-based) of `sheet`, as one undo step.
    /// Returns how many rows were hidden now.
    pub fn hide_rows(&mut self, sheet: u32, first: u32, last: u32) -> Result<u32> {
        if first > last || last >= MAX_ROWS {
            bail!("row out of range");
        }
        if last - first + 1 > MAX_HIDE_ROWS {
            bail!("select at most {MAX_HIDE_ROWS} rows to hide");
        }
        let hidden = self.hidden_rows(sheet);
        let changes: Vec<(i32, bool)> = (first..=last)
            .filter(|r| !hidden.contains(r))
            .map(|r| (r as i32 + 1, true))
            .collect();
        self.set_rows_hidden_steps(sheet, &changes)?;
        Ok(changes.len() as u32)
    }

    /// Show the hidden rows among `first..=last` (0-based) of `sheet` as one
    /// undo step, except rows a filter hides (see the module doc).
    pub fn unhide_rows(&mut self, sheet: u32, first: u32, last: u32) -> Result<Unhidden> {
        if first > last || last >= MAX_ROWS {
            bail!("row out of range");
        }
        let filtered = self.filtered_row_spans(sheet);
        let hidden: BTreeSet<u32> = self
            .hidden_rows(sheet)
            .into_iter()
            .filter(|r| (first..=last).contains(r))
            .collect();
        let mut done = Unhidden::default();
        let mut changes = Vec::new();
        for r in hidden {
            if filtered.iter().any(|&(a, b)| (a..=b).contains(&r)) {
                done.filtered += 1;
            } else {
                changes.push((r as i32 + 1, false));
            }
        }
        self.set_rows_hidden_steps(sheet, &changes)?;
        done.shown = changes.len() as u32;
        Ok(done)
    }

    fn set_rows_hidden_steps(&mut self, sheet: u32, changes: &[(i32, bool)]) -> Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        if self
            .model
            .kantor_set_rows_hidden(sheet, changes)
            .map_err(|e| anyhow!("{e}"))?
        {
            self.history.edited();
        }
        Ok(())
    }

    /// Hide (`hide`) or show columns `first..=last` (0-based) of `sheet`, as
    /// one undo step. Returns how many columns changed.
    pub fn set_columns_hidden(
        &mut self,
        sheet: u32,
        first: u32,
        last: u32,
        hide: bool,
    ) -> Result<u32> {
        if first > last || last >= MAX_COLS {
            bail!("column out of range");
        }
        let changes: Vec<(i32, bool)> = if hide {
            (first..=last)
                .filter(|&c| !self.is_column_hidden(sheet, c))
                .map(|c| (c as i32 + 1, true))
                .collect()
        } else {
            self.hidden_columns(sheet, first, last)
                .into_iter()
                .map(|c| (c as i32 + 1, false))
                .collect()
        };
        if changes.is_empty() {
            return Ok(0);
        }
        if self
            .model
            .kantor_set_columns_hidden(sheet, &changes)
            .map_err(|e| anyhow!("{e}"))?
        {
            self.history.edited();
        }
        Ok(changes.len() as u32)
    }
}

#[cfg(test)]
mod tests {
    use crate::{CellRange, FilterScope, Workbook};

    fn book(rows: u32) -> Workbook {
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(0, 0, 0, "Name").unwrap();
        wb.set_input(0, 0, 1, "Qty").unwrap();
        for r in 1..rows {
            wb.set_input(0, r, 0, &format!("n{}", r % 3)).unwrap();
            wb.set_input(0, r, 1, &r.to_string()).unwrap();
        }
        wb
    }

    #[test]
    fn hide_and_unhide_rows_are_one_undo_step_each() {
        let mut wb = book(10);
        assert_eq!(wb.hide_rows(0, 2, 4).unwrap(), 3);
        assert!((2..=4).all(|r| wb.is_row_hidden(0, r)));
        assert!(!wb.is_row_hidden(0, 5));
        // Hiding again changes nothing and adds no undo step.
        assert_eq!(wb.hide_rows(0, 3, 3).unwrap(), 0);
        wb.undo().unwrap();
        assert!((2..=4).all(|r| !wb.is_row_hidden(0, r)), "one step");
        wb.redo().unwrap();
        assert!(wb.is_row_hidden(0, 3));
        let done = wb.unhide_rows(0, 0, 9).unwrap();
        assert_eq!(done.shown, 3);
        assert!((2..=4).all(|r| !wb.is_row_hidden(0, r)));
        wb.undo().unwrap();
        assert!((2..=4).all(|r| wb.is_row_hidden(0, r)), "one step");
        // Only inside the range.
        assert_eq!(wb.unhide_rows(0, 3, 9).unwrap().shown, 2);
        assert!(wb.is_row_hidden(0, 2));
        assert!(wb.hide_rows(0, 0, super::MAX_HIDE_ROWS).is_err());
    }

    #[test]
    fn hidden_rows_and_columns_survive_save_with_their_sizes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hidden.xlsx");
        let mut wb = book(6);
        wb.set_column_width(0, 2, 2, 150.0).unwrap();
        wb.set_row_heights(0, 3, 3, crate::RowHeight::Points(40.0))
            .unwrap();
        wb.hide_rows(0, 3, 4).unwrap();
        assert_eq!(wb.set_columns_hidden(0, 1, 2, true).unwrap(), 2);
        assert_eq!(wb.column_width(0, 2), 0.0, "hidden: no width on screen");
        wb.save_as(&path).unwrap();
        let mut back = Workbook::open(&path).unwrap();
        assert!(back.is_row_hidden(0, 3) && back.is_row_hidden(0, 4));
        assert!(!back.is_row_hidden(0, 2));
        assert_eq!(back.hidden_columns(0, 0, 10), vec![1, 2]);
        // Unhidden, they come back as they were.
        back.unhide_rows(0, 0, 10).unwrap();
        assert_eq!(back.set_columns_hidden(0, 0, 10, false).unwrap(), 2);
        assert!((back.column_width(0, 2) - 150.0).abs() < 1.0);
        let spec = back.row_spec(0);
        let r3 = spec.rows.iter().find(|r| r.row == 3).unwrap();
        assert!((r3.points - 40.0).abs() < 0.01 && !r3.hidden);
    }

    #[test]
    fn unhiding_columns_is_one_undo_step() {
        let mut wb = book(3);
        assert_eq!(wb.set_columns_hidden(0, 0, 3, true).unwrap(), 4);
        assert_eq!(wb.hidden_columns(0, 0, 100), vec![0, 1, 2, 3]);
        assert_eq!(wb.set_columns_hidden(0, 2, 2, true).unwrap(), 0);
        assert_eq!(wb.set_columns_hidden(0, 1, 2, false).unwrap(), 2);
        assert_eq!(wb.hidden_columns(0, 0, 100), vec![0, 3]);
        wb.undo().unwrap();
        assert_eq!(wb.hidden_columns(0, 0, 100), vec![0, 1, 2, 3]);
        wb.undo().unwrap();
        assert!(wb.hidden_columns(0, 0, 100).is_empty(), "hide was one step");
        assert_eq!(wb.display_value(0, 2, 1), "2", "the edit before stays");
    }

    #[test]
    fn dragging_a_hidden_column_open_shows_it_in_one_step() {
        let mut wb = book(3);
        wb.set_columns_hidden(0, 1, 1, true).unwrap();
        assert!(wb.set_column_widths(0, &[(1, 120.0)]).unwrap());
        assert!(!wb.is_column_hidden(0, 1));
        assert!((wb.column_width(0, 1) - 120.0).abs() < 1.0);
        wb.undo().unwrap();
        assert!(wb.is_column_hidden(0, 1), "width and unhide undo together");
    }

    #[test]
    fn unhide_leaves_filtered_rows_to_the_filter() {
        let mut wb = book(10);
        wb.toggle_filter(0, CellRange::from_corners((0, 0), (8, 1)))
            .unwrap();
        // Keep only n0 (rows 3, 6).
        let values = wb.filter_values(0, &FilterScope::Sheet, 0);
        let keep: Vec<String> = values
            .iter()
            .map(|v| v.text.clone())
            .filter(|t| t == "n0")
            .collect();
        wb.set_column_filter(
            0,
            &FilterScope::Sheet,
            0,
            Some(crate::FilterKind::Values {
                values: keep,
                blanks: false,
                dates: Vec::new(),
            }),
        )
        .unwrap();
        assert!(wb.is_row_hidden(0, 1) && !wb.is_row_hidden(0, 3));
        // A row hidden by hand below the filtered data.
        wb.set_input(0, 12, 0, "below").unwrap();
        wb.hide_rows(0, 12, 12).unwrap();
        let done = wb.unhide_rows(0, 0, 20).unwrap();
        assert_eq!(done.shown, 1, "the hand-hidden row");
        assert!(done.filtered > 0);
        assert!(wb.is_row_hidden(0, 1), "still filtered out");
        assert!(!wb.is_row_hidden(0, 12));
        // Clearing the filter shows them.
        wb.clear_filter(0, &FilterScope::Sheet).unwrap();
        assert!(!wb.is_row_hidden(0, 1));
    }

    #[test]
    fn hiding_rows_under_a_merge_keeps_the_merge() {
        let mut wb = book(6);
        let merged = CellRange::from_corners((1, 3), (4, 4));
        wb.set_input(0, 1, 3, "Title").unwrap();
        wb.merge_range(0, merged).unwrap();
        wb.hide_rows(0, 2, 3).unwrap();
        assert_eq!(wb.merge_at(0, 1, 3), Some(merged));
        assert_eq!(wb.merge_at(0, 4, 4), Some(merged));
        wb.unhide_rows(0, 0, 5).unwrap();
        wb.set_columns_hidden(0, 4, 4, true).unwrap();
        assert_eq!(wb.merge_at(0, 4, 3), Some(merged));
        assert_eq!(wb.display_value(0, 1, 3), "Title");
    }
}
