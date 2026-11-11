//! Sorting a range or a table by one or more columns, as Excel does.
//!
//! Order (ascending): numbers (dates are numbers), then text (any case),
//! then FALSE before TRUE, then errors; blanks always last, descending too.
//! The sort is stable. Rows hidden by a filter stay where they are and only
//! the visible rows are sorted among themselves, as Excel does under a filter.
//!
//! Cells move with their formulas as they are: IronCalc keeps a formula's
//! references relative to its cell, so a reference to a cell in the same row
//! follows the row and absolute references stay put, which is what Excel's
//! sort does (`UserModel::kantor_permute_rows`, vendor/ironcalc_base §12).

use std::cmp::Ordering;

use anyhow::{Result, anyhow, bail};
use ironcalc_base::Model;
use ironcalc_base::types::{Cell, FormulaValue, SpillValue};

use crate::{CellRange, Workbook};

/// A cell's value as sorting and filtering compare it.
#[derive(Debug, Clone, PartialEq)]
pub enum CellValue {
    Number(f64),
    /// Lower case, for comparing.
    Text(String),
    Bool(bool),
    Error,
    Blank,
}

/// The value of cell `(row, col)` (0-based).
pub(crate) fn cell_value(model: &Model, sheet: u32, row: u32, col: u32) -> CellValue {
    let Ok(ws) = model.workbook.worksheet(sheet) else {
        return CellValue::Blank;
    };
    let Some(cell) = ws.sheet_data.cell(row as i32 + 1, col as i32 + 1) else {
        return CellValue::Blank;
    };
    let text = |s: &str| {
        if s.is_empty() {
            CellValue::Blank
        } else {
            CellValue::Text(s.to_lowercase())
        }
    };
    match cell.as_ref() {
        Cell::EmptyCell { .. } => CellValue::Blank,
        Cell::BooleanCell { v, .. } => CellValue::Bool(*v),
        Cell::NumberCell { v, .. } => CellValue::Number(*v),
        Cell::ErrorCell { .. } => CellValue::Error,
        Cell::SharedString { si, .. } => model
            .workbook
            .shared_strings
            .get(*si as usize)
            .map_or(CellValue::Blank, |s| text(s)),
        Cell::CellFormula { v, .. } | Cell::ArrayFormula { v, .. } => match v {
            FormulaValue::Number(n) => CellValue::Number(*n),
            FormulaValue::Text(s) => text(s),
            FormulaValue::Boolean(b) => CellValue::Bool(*b),
            FormulaValue::Error { .. } => CellValue::Error,
            FormulaValue::Unevaluated => CellValue::Blank,
        },
        Cell::SpillCell { v, .. } => match v {
            SpillValue::Number(n) => CellValue::Number(*n),
            SpillValue::Text(s) => text(s),
            SpillValue::Boolean(b) => CellValue::Bool(*b),
            SpillValue::Error(_) => CellValue::Error,
        },
    }
}

fn rank(v: &CellValue) -> u8 {
    match v {
        CellValue::Number(_) => 0,
        CellValue::Text(_) => 1,
        CellValue::Bool(_) => 2,
        CellValue::Error => 3,
        CellValue::Blank => 4,
    }
}

/// Ascending order of two values (blanks last).
pub(crate) fn compare_values(a: &CellValue, b: &CellValue) -> Ordering {
    match (a, b) {
        (CellValue::Number(x), CellValue::Number(y)) => x.total_cmp(y),
        (CellValue::Text(x), CellValue::Text(y)) => x.cmp(y),
        (CellValue::Bool(x), CellValue::Bool(y)) => x.cmp(y),
        _ => rank(a).cmp(&rank(b)),
    }
}

/// One sort level: a column (absolute, 0-based) and its direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortKey {
    pub col: u32,
    pub descending: bool,
}

fn compare_keyed(a: &[CellValue], b: &[CellValue], keys: &[SortKey]) -> Ordering {
    for (i, k) in keys.iter().enumerate() {
        let (x, y) = (&a[i], &b[i]);
        // Blanks last both ways.
        let o = match (matches!(x, CellValue::Blank), matches!(y, CellValue::Blank)) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            _ if k.descending => compare_values(y, x),
            _ => compare_values(x, y),
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    Ordering::Equal
}

impl Workbook {
    /// Sort the rows of `range` on `sheet` by `keys` (columns inside the
    /// range), first key first. With `header`, the range's first row stays
    /// on top. Whole columns are sorted down to the last used row. One undo
    /// step; returns the number of rows sorted.
    pub fn sort_range(
        &mut self,
        sheet: u32,
        range: CellRange,
        keys: &[SortKey],
        header: bool,
    ) -> Result<usize> {
        if keys.is_empty() {
            bail!("nothing to sort by");
        }
        let (rows, cols) = self.used_size(sheet);
        let mut range = range
            .clip(rows.max(1), cols.max(1))
            .ok_or_else(|| anyhow!("nothing to sort"))?;
        if keys.iter().any(|k| k.col < range.left || k.col > range.right) {
            bail!("the sort column is outside the range");
        }
        if header {
            range.top += 1;
        }
        if range.top >= range.bottom {
            return Ok(0);
        }
        let hidden = self.hidden_rows(sheet);
        let visible: Vec<u32> = (range.top..=range.bottom)
            .filter(|r| !hidden.contains(r))
            .collect();
        let model = self.model.get_model();
        let mut rows: Vec<(u32, Vec<CellValue>)> = visible
            .iter()
            .map(|&r| {
                (
                    r,
                    keys.iter()
                        .map(|k| cell_value(model, sheet, r, k.col))
                        .collect(),
                )
            })
            .collect();
        rows.sort_by(|a, b| compare_keyed(&a.1, &b.1, keys));
        // Destination of each row: visible slots get the sorted rows, hidden
        // rows keep theirs.
        let mut order: Vec<i32> = (range.top..=range.bottom).map(|r| r as i32 + 1).collect();
        for (slot, (src, _)) in visible.iter().zip(&rows) {
            order[(slot - range.top) as usize] = *src as i32 + 1;
        }
        let count = rows.len();
        if order
            .iter()
            .enumerate()
            .all(|(i, &r)| r == (range.top + i as u32) as i32 + 1)
        {
            return Ok(count);
        }
        self.model.pause_evaluation();
        let r = self
            .model
            .kantor_permute_rows(
                sheet,
                range.top as i32 + 1,
                range.left as i32 + 1,
                range.right as i32 + 1,
                &order,
            )
            .map_err(|e| anyhow!("{e}"));
        self.end_batch(u32::from(r.is_ok()));
        r?;
        Ok(count)
    }

    /// Sort a table's data rows by one of its columns (absolute).
    pub fn sort_table(
        &mut self,
        sheet: u32,
        table: &str,
        col: u32,
        descending: bool,
    ) -> Result<usize> {
        let t = self
            .tables(sheet)
            .into_iter()
            .find(|t| t.name == table)
            .ok_or_else(|| anyhow!("no table {table}"))?;
        let range = t.data_range();
        if range.top > range.bottom {
            return Ok(0);
        }
        self.sort_range(sheet, range, &[SortKey { col, descending }], false)
    }

    /// Whether the first row of `range` looks like a header (Excel's guess):
    /// text over a column whose second row is not text, or bold over plain.
    pub fn guess_header(&self, sheet: u32, range: CellRange) -> bool {
        if range.top >= range.bottom {
            return false;
        }
        if let Some(t) = self.table_at(sheet, range.top, range.left) {
            return t.header_row() == Some(range.top);
        }
        let model = self.model.get_model();
        let mut text_over_other = false;
        for c in range.left..=range.right.min(range.left + 50) {
            let a = cell_value(model, sheet, range.top, c);
            let b = cell_value(model, sheet, range.top + 1, c);
            if matches!(a, CellValue::Text(_)) && !matches!(b, CellValue::Text(_) | CellValue::Blank)
            {
                text_over_other = true;
            }
            if self.cell_style(sheet, range.top, c).bold
                && !self.cell_style(sheet, range.top + 1, c).bold
            {
                return true;
            }
        }
        text_over_other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excel_order() {
        let mut v = vec![
            CellValue::Blank,
            CellValue::Bool(true),
            CellValue::Text("b".into()),
            CellValue::Error,
            CellValue::Number(10.0),
            CellValue::Bool(false),
            CellValue::Text("a".into()),
            CellValue::Number(-1.0),
        ];
        v.sort_by(compare_values);
        assert_eq!(
            v,
            [
                CellValue::Number(-1.0),
                CellValue::Number(10.0),
                CellValue::Text("a".into()),
                CellValue::Text("b".into()),
                CellValue::Bool(false),
                CellValue::Bool(true),
                CellValue::Error,
                CellValue::Blank,
            ]
        );
    }
}
