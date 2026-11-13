//! KANTOR PATCH (changes since last asked): what edits, undos and redos
//! changed, for callers that keep answers worked out from many cells (the
//! range-wide numbers of conditional formatting) and want to keep them
//! across edits that don't touch them. See KANTOR_PATCHES.md §16.
//!
//! [`UserModel`](crate::UserModel) puts every change on its `send_queue`
//! (upstream: for syncing a remote copy). Kantor never sends it, so the
//! queue held a second copy of every diff for the whole session;
//! [`UserModel::kantor_take_changes`](crate::UserModel::kantor_take_changes)
//! empties it into a [`KantorChanges`] summary instead.
//!
//! Formula results change without a diff, when the model is evaluated:
//! [`Model::kantor_evaluations`](crate::Model::kantor_evaluations) counts
//! evaluations.

use crate::user_model::history::{Diff, QueueDiffs};

/// More rectangles than this on one sheet are summed up as the whole sheet.
const MAX_RECTS_PER_SHEET: usize = 64;

/// What changed, by sheet index. Rows and columns are 1-based, inclusive.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct KantorChanges {
    /// Cells whose value or formula may have changed: `(sheet, row1,
    /// column1, row2, column2)`.
    pub cells: Vec<(u32, i32, i32, i32, i32)>,
    /// Sheets where anything may have changed or moved (rows inserted,
    /// sorted, conditional formatting rules edited...).
    pub sheets: Vec<u32>,
    /// Anything may have changed (sheets added, removed or renamed, defined
    /// names, locale...).
    pub all: bool,
    /// Something changed that doesn't change values (styles, sizes, hidden
    /// rows, frozen panes...).
    pub restyled: bool,
}

impl KantorChanges {
    /// Nothing changed.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty() && self.sheets.is_empty() && !self.all && !self.restyled
    }

    /// Whether values in `sheet`'s rectangle (1-based, inclusive) may have
    /// changed.
    pub fn touches(&self, sheet: u32, row1: i32, col1: i32, row2: i32, col2: i32) -> bool {
        self.all
            || self.sheets.contains(&sheet)
            || self.cells.iter().any(|&(s, r1, c1, r2, c2)| {
                s == sheet && r1 <= row2 && row1 <= r2 && c1 <= col2 && col1 <= c2
            })
    }

    /// Whether `sheet` as a whole changed (or everything did).
    pub fn sheet_changed(&self, sheet: u32) -> bool {
        self.all || self.sheets.contains(&sheet)
    }

    /// Whether values anywhere may have changed.
    pub fn any_values(&self) -> bool {
        self.all || !self.sheets.is_empty() || !self.cells.is_empty()
    }

    /// Add what `other` says.
    pub fn merge(&mut self, other: KantorChanges) {
        self.all |= other.all;
        self.restyled |= other.restyled;
        for s in other.sheets {
            self.sheet(s);
        }
        for (s, r1, c1, r2, c2) in other.cells {
            self.rect(s, r1, c1, r2, c2);
        }
    }

    fn sheet(&mut self, sheet: u32) {
        if !self.sheets.contains(&sheet) {
            self.sheets.push(sheet);
        }
        self.cells.retain(|c| c.0 != sheet);
    }

    fn rect(&mut self, sheet: u32, row1: i32, col1: i32, row2: i32, col2: i32) {
        if self.all || self.sheets.contains(&sheet) {
            return;
        }
        // Grow a rectangle this one touches or continues (a fill, typing
        // down a column), else add it.
        for c in self.cells.iter_mut().filter(|c| c.0 == sheet) {
            let rows_meet = row1 <= c.3 + 1 && c.1 <= row2 + 1;
            let cols_meet = col1 <= c.4 + 1 && c.2 <= col2 + 1;
            let same_cols = c.2 == col1 && c.4 == col2;
            let same_rows = c.1 == row1 && c.3 == row2;
            if (same_cols && rows_meet) || (same_rows && cols_meet) {
                c.1 = c.1.min(row1);
                c.2 = c.2.min(col1);
                c.3 = c.3.max(row2);
                c.4 = c.4.max(col2);
                return;
            }
        }
        if self.cells.iter().filter(|c| c.0 == sheet).count() >= MAX_RECTS_PER_SHEET {
            self.sheet(sheet);
            return;
        }
        self.cells.push((sheet, row1, col1, row2, col2));
    }

    /// KANTOR PATCH (partial recalculation): add one diff list.
    pub(crate) fn add_list(&mut self, list: &[Diff]) {
        for diff in list {
            self.diff(diff);
        }
    }

    pub(crate) fn add(&mut self, queue: &[QueueDiffs]) {
        for q in queue {
            for diff in &q.list {
                self.diff(diff);
            }
        }
    }

    fn diff(&mut self, diff: &Diff) {
        match diff {
            Diff::SetCellValue {
                sheet, row, column, ..
            } => self.rect(*sheet, *row, *column, *row, *column),
            // An array formula's anchor and the cells it covers.
            Diff::SetArrayValue {
                sheet,
                row,
                column,
                width,
                height,
                ..
            }
            | Diff::RangeClearContents {
                sheet,
                row,
                column,
                width,
                height,
                ..
            }
            | Diff::RangeClearAll {
                sheet,
                row,
                column,
                width,
                height,
                ..
            } => self.rect(
                *sheet,
                *row,
                *column,
                *row + (*height).max(1) - 1,
                *column + (*width).max(1) - 1,
            ),
            Diff::KantorPermuteRows {
                sheet,
                top,
                left,
                right,
                order,
            } => self.rect(*sheet, *top, *left, *top + order.len() as i32 - 1, *right),
            // Cells move, tables and rules change shape.
            Diff::InsertRows { sheet, .. }
            | Diff::DeleteRows { sheet, .. }
            | Diff::InsertColumns { sheet, .. }
            | Diff::DeleteColumns { sheet, .. }
            | Diff::MoveColumns { sheet, .. }
            | Diff::MoveRows { sheet, .. }
            | Diff::SetMergedCells { sheet, .. }
            | Diff::AddConditionalFormatting { sheet, .. }
            | Diff::DeleteConditionalFormatting { sheet, .. }
            | Diff::UpdateConditionalFormatting { sheet, .. }
            | Diff::SwapConditionalFormattingPriority { sheet, .. }
            | Diff::KantorSetSheetFilter { sheet, .. } => self.sheet(*sheet),
            // No value changes.
            Diff::CellClearFormatting { .. }
            | Diff::SetCellStyle { .. }
            | Diff::ApplyNamedStyle { .. }
            | Diff::SetColumnWidth { .. }
            | Diff::SetColumnHidden { .. }
            | Diff::SetRowHeight { .. }
            | Diff::SetRowHidden { .. }
            | Diff::SetRowEntry { .. }
            | Diff::KantorRowEntries { .. }
            | Diff::SetColumnStyle { .. }
            | Diff::SetRowStyle { .. }
            | Diff::DeleteColumnStyle { .. }
            | Diff::DeleteRowStyle { .. }
            | Diff::SetFrozenRowsCount { .. }
            | Diff::SetFrozenColumnsCount { .. }
            | Diff::SetSheetColor { .. }
            | Diff::SetSheetState { .. }
            | Diff::SetShowGridLines { .. }
            | Diff::SetCellLink { .. }
            | Diff::SetWorkbookName { .. } => self.restyled = true,
            // Sheets added, removed, moved or renamed; tables (structured
            // references); names, locale, styles, theme.
            Diff::DeleteSheet { .. }
            | Diff::NewSheet { .. }
            | Diff::DuplicateSheet { .. }
            | Diff::RenameSheet { .. }
            | Diff::MoveSheet { .. }
            | Diff::KantorSetTable { .. }
            | Diff::KantorRenameTableColumn { .. }
            | Diff::SetTheme { .. }
            | Diff::CreateDefinedName { .. }
            | Diff::DeleteDefinedName { .. }
            | Diff::UpdateDefinedName { .. }
            | Diff::SetLocale { .. }
            | Diff::SetTimezone { .. }
            | Diff::CreateNamedStyle { .. }
            | Diff::DeleteNamedStyle { .. }
            | Diff::UpdateNamedStyle { .. } => self.all = true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_grow_and_collapse() {
        let mut c = KantorChanges::default();
        for row in 1..=10 {
            c.rect(0, row, 3, row, 3);
        }
        assert_eq!(c.cells, vec![(0, 1, 3, 10, 3)]);
        assert!(c.touches(0, 5, 1, 5, 5));
        assert!(!c.touches(0, 11, 3, 20, 3));
        assert!(!c.touches(1, 5, 3, 5, 3));
        for i in 0..MAX_RECTS_PER_SHEET as i32 {
            c.rect(0, 100 + 2 * i, 10 + 2 * i, 100 + 2 * i, 10 + 2 * i);
        }
        assert!(c.sheets.contains(&0));
        assert!(c.touches(0, 1000, 1000, 1000, 1000));
    }
}
