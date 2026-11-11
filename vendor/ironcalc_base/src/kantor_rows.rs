//! KANTOR PATCH (row heights): row heights in points with an exact undo.
//!
//! Upstream's `UserModel::set_rows_height` takes IronCalc's pixels, always
//! marks the row `customHeight`, keeps a hidden row hidden, and its undo only
//! restores the height (the row stays custom; a hidden row comes back with
//! height 0, as `get_row_height` reports hidden rows). This sets a row's
//! whole entry and records the old one (`Diff::SetRowEntry`), so undo puts
//! back exactly what the file had. See KANTOR_PATCHES.md §10.

use std::collections::HashMap;

use crate::{
    model::Model,
    types::Row,
    user_model::{UserModel, history::Diff},
};

/// Excel's largest row height, in points.
pub const MAX_ROW_HEIGHT: f64 = 409.0;

impl Model<'_> {
    /// Replace row `row`'s entry (1-based) on `sheet`, or remove it (`None`).
    pub(crate) fn kantor_set_row_entry(
        &mut self,
        sheet: u32,
        row: i32,
        entry: Option<Row>,
    ) -> Result<(), String> {
        let rows = &mut self.workbook.worksheet_mut(sheet)?.rows;
        let at = rows.iter().position(|r| r.r == row);
        match (at, entry) {
            (Some(i), Some(entry)) => rows[i] = entry,
            (Some(i), None) => {
                rows.remove(i);
            }
            (None, Some(entry)) => rows.push(entry),
            (None, None) => {}
        }
        Ok(())
    }
}

/// Whether a row entry says anything besides its height.
fn has_more_than_height(row: &Row) -> bool {
    row.s != 0 || row.custom_format || row.hidden || row.outline_level > 0 || row.collapsed
}

impl UserModel<'_> {
    /// Set the height of rows `row_start..=row_end` (1-based) on `sheet` as one
    /// undo step.
    ///
    /// * `Some(points)`: a custom height (`customHeight="1"`), which also shows
    ///   a hidden row, as in Excel.
    /// * `None`: automatic. The row's height follows its content again; an
    ///   entry kept only for its height is removed, others keep their style
    ///   and get `auto_points` (the fitted height, written as `ht`).
    ///
    /// Rows whose entry would not change are left out of the undo step;
    /// returns whether any changed (no undo step is recorded otherwise).
    pub fn kantor_set_rows_height(
        &mut self,
        sheet: u32,
        row_start: i32,
        row_end: i32,
        height: Option<f64>,
        auto_points: f64,
    ) -> Result<bool, String> {
        if row_start < 1 || row_end < row_start || row_end > crate::constants::LAST_ROW {
            return Err(format!("Rows {row_start}:{row_end} are not valid."));
        }
        if height.is_some_and(|h| !(h.is_finite() && h > 0.0 && h <= MAX_ROW_HEIGHT)) {
            return Err(format!("Row height must be between 0 and {MAX_ROW_HEIGHT} points."));
        }
        // One pass over the entries instead of a search per row.
        let index: HashMap<i32, Row> = self
            .model
            .workbook
            .worksheet(sheet)?
            .rows
            .iter()
            .filter(|r| (row_start..=row_end).contains(&r.r))
            .map(|r| (r.r, r.clone()))
            .collect();
        let mut diff_list = Vec::new();
        for row in row_start..=row_end {
            let old = index.get(&row).cloned();
            let new = match (height, &old) {
                (Some(h), Some(old)) => Some(Row {
                    height: h,
                    custom_height: true,
                    hidden: false,
                    ..old.clone()
                }),
                (Some(h), None) => Some(Row {
                    r: row,
                    height: h,
                    custom_format: false,
                    custom_height: true,
                    s: 0,
                    hidden: false,
                    outline_level: 0,
                    collapsed: false,
                }),
                (None, Some(old)) if has_more_than_height(old) => Some(Row {
                    height: auto_points,
                    custom_height: false,
                    ..old.clone()
                }),
                (None, _) => None,
            };
            if new == old {
                continue;
            }
            self.model.kantor_set_row_entry(sheet, row, new.clone())?;
            diff_list.push(Diff::SetRowEntry {
                sheet,
                row,
                new_value: new.map(Box::new),
                old_value: old.map(Box::new),
            });
        }
        let changed = !diff_list.is_empty();
        if changed {
            self.push_diff_list(diff_list);
        }
        Ok(changed)
    }
}
