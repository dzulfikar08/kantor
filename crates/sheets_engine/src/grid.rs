//! Grid manipulation: Ctrl+arrow jumps, the fill handle and Ctrl+D / Ctrl+R,
//! inserting and deleting rows and columns, frozen panes, and the numbers
//! behind the status bar's Sum / Average / Count.
//!
//! The decisions (where a jump lands, which way a fill goes, what the status
//! bar shows) are pure functions; the [`Workbook`] methods apply them. Fills
//! themselves are IronCalc's auto-fill, which extends series (`1, 2` → `3, 4`,
//! dates as numbers, `Item 1` → `Item 2`), repeats everything else and shifts
//! relative references in formulas, recording one undo step.

use anyhow::{Result, anyhow, bail};
use ironcalc_base::cell::CellValue;
use ironcalc_base::types::Cell;

use crate::{CellRange, MAX_COLS, MAX_ROWS, Workbook};

/// An arrow-key direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Up,
    Down,
    Left,
    Right,
}

impl Step {
    pub fn is_vertical(self) -> bool {
        matches!(self, Step::Up | Step::Down)
    }

    fn forward(self) -> bool {
        matches!(self, Step::Down | Step::Right)
    }
}

/// Where Ctrl+arrow lands along one line of cells (a row or a column), like
/// Excel: from inside a block of filled cells, its last cell; otherwise the
/// next filled cell, or the sheet's edge when there is none.
///
/// `from` and the result are indices along the line, `last` is the line's
/// last index, and no index at or past `data_end` is filled (so scanning
/// blanks towards the edge stops early).
pub fn data_edge(
    from: u32,
    forward: bool,
    last: u32,
    data_end: u32,
    filled: impl Fn(u32) -> bool,
) -> u32 {
    let next = |i: u32| {
        if forward {
            (i < last).then(|| i + 1)
        } else {
            i.checked_sub(1)
        }
    };
    let Some(first) = next(from) else {
        return from;
    };
    if filled(from) && filled(first) {
        let mut i = first;
        while let Some(j) = next(i) {
            if !filled(j) {
                break;
            }
            i = j;
        }
        return i;
    }
    let mut i = first;
    loop {
        if forward && i >= data_end {
            return last;
        }
        if filled(i) {
            return i;
        }
        match next(i) {
            Some(j) => i = j,
            None => return i,
        }
    }
}

/// Which way a fill extends its source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillDirection {
    Down,
    Up,
    Right,
    Left,
}

/// A fill: extend `source` in `direction` up to row or column `to` (inclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fill {
    pub source: CellRange,
    pub direction: FillDirection,
    pub to: u32,
}

impl Fill {
    /// The fill handle of `source` dragged to the cell `pointer`: the fill
    /// goes along the axis the pointer left the source furthest on, and
    /// covers whole rows or columns of the source. `None` while the pointer
    /// is inside the source.
    pub fn from_drag(source: CellRange, pointer: (u32, u32)) -> Option<Fill> {
        let (row, col) = pointer;
        let down = row.saturating_sub(source.bottom);
        let up = source.top.saturating_sub(row);
        let right = col.saturating_sub(source.right);
        let left = source.left.saturating_sub(col);
        let vertical = down.max(up);
        let horizontal = right.max(left);
        if vertical == 0 && horizontal == 0 {
            return None;
        }
        let (direction, to) = if vertical >= horizontal {
            if down > 0 {
                (FillDirection::Down, row)
            } else {
                (FillDirection::Up, row)
            }
        } else if right > 0 {
            (FillDirection::Right, col)
        } else {
            (FillDirection::Left, col)
        };
        Some(Fill {
            source,
            direction,
            to,
        })
    }

    /// Ctrl+D: copy the top row of `range` down through it, or, for a single
    /// row, the row above into it. `None` on the first row.
    pub fn down(range: CellRange) -> Option<Fill> {
        if range.rows_count() > 1 {
            let source = CellRange {
                bottom: range.top,
                ..range
            };
            return Some(Fill {
                source,
                direction: FillDirection::Down,
                to: range.bottom,
            });
        }
        let above = range.top.checked_sub(1)?;
        Some(Fill {
            source: CellRange {
                top: above,
                bottom: above,
                ..range
            },
            direction: FillDirection::Down,
            to: range.bottom,
        })
    }

    /// Ctrl+R: like [`Fill::down`], from the left column (or the column to
    /// the left of a one-column range).
    pub fn right(range: CellRange) -> Option<Fill> {
        if range.cols_count() > 1 {
            let source = CellRange {
                right: range.left,
                ..range
            };
            return Some(Fill {
                source,
                direction: FillDirection::Right,
                to: range.right,
            });
        }
        let left = range.left.checked_sub(1)?;
        Some(Fill {
            source: CellRange {
                left,
                right: left,
                ..range
            },
            direction: FillDirection::Right,
            to: range.right,
        })
    }

    /// Source and filled cells together: the selection after the fill.
    pub fn target(&self) -> CellRange {
        let s = self.source;
        match self.direction {
            FillDirection::Down => CellRange {
                bottom: self.to,
                ..s
            },
            FillDirection::Up => CellRange { top: self.to, ..s },
            FillDirection::Right => CellRange {
                right: self.to,
                ..s
            },
            FillDirection::Left => CellRange { left: self.to, ..s },
        }
    }
}

/// Whether a number format code shows a date (has day or year parts
/// outside quoted text and `[...]` sections).
fn is_date_code(code: &str) -> bool {
    let mut quoted = false;
    let mut bracket = false;
    for ch in code.chars() {
        match ch {
            '"' => quoted = !quoted,
            '[' if !quoted => bracket = true,
            ']' if !quoted => bracket = false,
            'd' | 'D' | 'y' | 'Y' if !quoted && !bracket => return true,
            _ => {}
        }
    }
    false
}

/// What the status bar sums up for a selection.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct RangeStats {
    /// Non-empty cells.
    pub count: usize,
    /// Cells whose value is a number (formulas included).
    pub numbers: usize,
    pub sum: f64,
}

impl RangeStats {
    pub fn add_number(&mut self, n: f64) {
        self.count += 1;
        self.numbers += 1;
        self.sum += n;
    }

    /// A non-empty cell that is not a number (text, logical, error).
    pub fn add_other(&mut self) {
        self.count += 1;
    }

    pub fn average(&self) -> Option<f64> {
        (self.numbers > 0).then(|| self.sum / self.numbers as f64)
    }

    /// Excel's status bar: nothing for fewer than two filled cells, the
    /// count alone when none is a number, else average, count and sum.
    pub fn summary(&self) -> Option<String> {
        if self.count < 2 {
            return None;
        }
        Some(match self.average() {
            None => format!("Count: {}", self.count),
            Some(avg) => format!(
                "Average: {}   Count: {}   Sum: {}",
                format_stat(avg),
                self.count,
                format_stat(self.sum)
            ),
        })
    }
}

/// A status-bar number: thousands separators and at most ten decimals,
/// trailing zeros dropped (`1234.5` → `1,234.5`).
pub fn format_stat(v: f64) -> String {
    if !v.is_finite() {
        return "—".to_owned();
    }
    let s = format!("{:.10}", v);
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        &s
    };
    let (sign, s) = match s.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", s),
    };
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    let mut grouped = String::with_capacity(int.len() + int.len() / 3);
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    // `-0` after rounding a tiny negative number.
    let sign = if grouped == "0" && frac.is_none() {
        ""
    } else {
        sign
    };
    match frac {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
}

impl Workbook {
    /// Whether a cell holds anything (a value or a formula; not just a style).
    pub fn is_filled(&self, sheet: u32, row: u32, col: u32) -> bool {
        let Ok(ws) = self.model.get_model().workbook.worksheet(sheet) else {
            return false;
        };
        match ws.cell(row as i32 + 1, col as i32 + 1) {
            Some(cell) => !matches!(*cell, Cell::EmptyCell { .. }),
            None => false,
        }
    }

    /// Where Ctrl+`step` from `(row, col)` lands (see [`data_edge`]).
    pub fn jump(&self, sheet: u32, (row, col): (u32, u32), step: Step) -> (u32, u32) {
        let (rows, cols) = self.used_size(sheet);
        if step.is_vertical() {
            let row = data_edge(row, step.forward(), MAX_ROWS - 1, rows, |r| {
                self.is_filled(sheet, r, col)
            });
            (row, col)
        } else {
            let col = data_edge(col, step.forward(), MAX_COLS - 1, cols, |c| {
                self.is_filled(sheet, row, c)
            });
            (row, col)
        }
    }

    /// Apply a fill (one undo step) and return the range it covers.
    pub fn fill(&mut self, sheet: u32, fill: &Fill) -> Result<CellRange> {
        let target = fill.target();
        if target.bottom >= MAX_ROWS || target.right >= MAX_COLS {
            bail!("fill goes past the sheet's edge");
        }
        let area = fill.source.area(sheet);
        let to = fill.to as i32 + 1;
        let res = match fill.direction {
            FillDirection::Down | FillDirection::Up => self.model.auto_fill_rows(&area, to),
            FillDirection::Right | FillDirection::Left => self.model.auto_fill_columns(&area, to),
        };
        let series = self.number_series(sheet, fill);
        res.map_err(|e| anyhow!("{e}"))?;
        let mut steps = 1;
        for (line, values) in series {
            match self.write_numbers(sheet, line, &values) {
                Ok(()) => steps += 1,
                Err(err) => log::warn!("fill series: {err:#}"),
            }
        }
        self.history.edited_steps(steps);
        Ok(target)
    }

    /// Series IronCalc's auto-fill misses: lines of the source holding typed
    /// numbers shown with a format (dates above all; IronCalc reads their
    /// formatted text and only copies them). Each entry is a line of the
    /// fill's new cells and their values, in top-to-bottom / left-to-right
    /// order. Two or more numbers in steps of the same size continue; one date
    /// counts up by a day, as in Excel.
    fn number_series(&self, sheet: u32, fill: &Fill) -> Vec<(CellRange, Vec<f64>)> {
        let Ok(ws) = self.model.get_model().workbook.worksheet(sheet) else {
            return Vec::new();
        };
        let s = fill.source;
        let vertical = matches!(fill.direction, FillDirection::Down | FillDirection::Up);
        let lines = if vertical {
            s.left..=s.right
        } else {
            s.top..=s.bottom
        };
        let mut out = Vec::new();
        for line in lines {
            let cells: Vec<(u32, u32)> = if vertical {
                (s.top..=s.bottom).map(|r| (r, line)).collect()
            } else {
                (s.left..=s.right).map(|c| (line, c)).collect()
            };
            let values: Option<Vec<f64>> = cells
                .iter()
                .map(
                    |&(r, c)| match ws.cell(r as i32 + 1, c as i32 + 1).as_deref() {
                        Some(Cell::NumberCell { v, .. }) => Some(*v),
                        _ => None,
                    },
                )
                .collect();
            let Some(values) = values else { continue };
            let formatted = cells.iter().all(|&(r, c)| {
                !matches!(
                    self.cell_style(sheet, r, c).number_format(),
                    crate::NumberFormat::General
                )
            });
            if !formatted {
                continue; // IronCalc's own series handle plain numbers
            }
            let step = match values.as_slice() {
                [_] => {
                    let (r, c) = cells[0];
                    let date = self.cell_style(sheet, r, c).number_format()
                        == crate::NumberFormat::Date
                        || is_date_code(&self.cell_style(sheet, r, c).num_fmt);
                    if !date {
                        continue;
                    }
                    1.0
                }
                [first, second, ..] => {
                    let step = second - first;
                    let even = values
                        .windows(2)
                        .all(|w| ((w[1] - w[0]) - step).abs() <= 1e-9 * step.abs().max(1.0));
                    if !even {
                        continue;
                    }
                    step
                }
                [] => continue,
            };
            let (first, last) = (values[0], values[values.len() - 1]);
            let (range, new): (CellRange, Vec<f64>) = match fill.direction {
                FillDirection::Down => (
                    CellRange::from_corners((s.bottom + 1, line), (fill.to, line)),
                    (1..=fill.to - s.bottom)
                        .map(|k| last + step * k as f64)
                        .collect(),
                ),
                FillDirection::Right => (
                    CellRange::from_corners((line, s.right + 1), (line, fill.to)),
                    (1..=fill.to - s.right)
                        .map(|k| last + step * k as f64)
                        .collect(),
                ),
                FillDirection::Up => (
                    CellRange::from_corners((fill.to, line), (s.top - 1, line)),
                    (1..=s.top - fill.to)
                        .rev()
                        .map(|k| first - step * k as f64)
                        .collect(),
                ),
                FillDirection::Left => (
                    CellRange::from_corners((line, fill.to), (line, s.left - 1)),
                    (1..=s.left - fill.to)
                        .rev()
                        .map(|k| first - step * k as f64)
                        .collect(),
                ),
            };
            out.push((range, new));
        }
        out
    }

    /// Write numbers into a one-row or one-column range as one IronCalc
    /// step, keeping the cells' formats.
    fn write_numbers(&mut self, sheet: u32, line: CellRange, values: &[f64]) -> Result<()> {
        let sep = if line.cols_count() > 1 { "\t" } else { "\n" };
        let text = values
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(sep);
        self.model
            .set_selected_sheet(sheet)
            .and_then(|()| {
                self.model
                    .set_selected_cell(line.top as i32 + 1, line.left as i32 + 1)
            })
            .and_then(|()| self.model.paste_csv_string(&line.area(sheet), &text))
            .map_err(|e| anyhow!("{e}"))
    }

    /// Insert `count` empty rows before `row` (one undo step).
    pub fn insert_rows(&mut self, sheet: u32, row: u32, count: u32) -> Result<()> {
        if count == 0 || row >= MAX_ROWS {
            bail!("row out of range");
        }
        self.model
            .insert_rows(sheet, row as i32 + 1, count as i32)
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(())
    }

    /// Delete rows `first..=last` (one undo step).
    pub fn delete_rows(&mut self, sheet: u32, first: u32, last: u32) -> Result<()> {
        if first > last || last >= MAX_ROWS {
            bail!("row out of range");
        }
        self.model
            .delete_rows(sheet, first as i32 + 1, (last - first + 1) as i32)
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(())
    }

    /// Insert `count` empty columns before `col` (one undo step).
    pub fn insert_columns(&mut self, sheet: u32, col: u32, count: u32) -> Result<()> {
        if count == 0 || col >= MAX_COLS {
            bail!("column out of range");
        }
        self.model
            .insert_columns(sheet, col as i32 + 1, count as i32)
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(())
    }

    /// Delete columns `first..=last` (one undo step).
    pub fn delete_columns(&mut self, sheet: u32, first: u32, last: u32) -> Result<()> {
        if first > last || last >= MAX_COLS {
            bail!("column out of range");
        }
        self.model
            .delete_columns(sheet, first as i32 + 1, (last - first + 1) as i32)
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(())
    }

    /// Frozen `(rows, columns)` at the top / left of `sheet`.
    pub fn frozen(&self, sheet: u32) -> (u32, u32) {
        let m = self.model.get_model();
        let rows = m.get_frozen_rows_count(sheet).unwrap_or(0).max(0) as u32;
        let cols = m.get_frozen_columns_count(sheet).unwrap_or(0).max(0) as u32;
        (rows, cols)
    }

    /// Freeze the first `rows` rows and `cols` columns (0 unfreezes), as one
    /// undo step. Saved to `.xlsx` as a frozen pane. Returns whether
    /// anything changed.
    pub fn set_frozen(&mut self, sheet: u32, rows: u32, cols: u32) -> Result<bool> {
        if rows >= MAX_ROWS || cols >= MAX_COLS {
            bail!("too many frozen rows or columns");
        }
        let (old_rows, old_cols) = self.frozen(sheet);
        let mut steps = 0;
        if rows != old_rows {
            self.model
                .set_frozen_rows_count(sheet, rows as i32)
                .map_err(|e| anyhow!("{e}"))?;
            steps += 1;
        }
        if cols != old_cols {
            if let Err(e) = self.model.set_frozen_columns_count(sheet, cols as i32) {
                if steps > 0 {
                    let _ = self.model.undo();
                }
                return Err(anyhow!("{e}"));
            }
            steps += 1;
        }
        if steps > 0 {
            self.history.edited_steps(steps);
        }
        Ok(steps > 0)
    }

    /// Status-bar numbers for `range`. Small ranges are read cell by cell,
    /// large ones (whole columns) by walking the cells that exist.
    pub fn range_stats(&self, sheet: u32, range: CellRange) -> RangeStats {
        let mut stats = RangeStats::default();
        let model = self.model.get_model();
        let Ok(ws) = model.workbook.worksheet(sheet) else {
            return stats;
        };
        let (rows, cols) = self.used_size(sheet);
        let Some(range) = range.clip(rows, cols) else {
            return stats;
        };
        let mut add = |row: i32, col: i32| {
            match ws.cell(row, col).as_deref() {
                None | Some(Cell::EmptyCell { .. }) => return,
                Some(_) => {}
            }
            match model.get_cell_value_by_index(sheet, row, col) {
                Ok(CellValue::Number(n)) => stats.add_number(n),
                Ok(CellValue::None) => {}
                Ok(CellValue::String(s)) if s.is_empty() => {}
                _ => stats.add_other(),
            }
        };
        if u64::from(range.rows_count()) * u64::from(range.cols_count()) <= 20_000 {
            for row in range.top..=range.bottom {
                for col in range.left..=range.right {
                    add(row as i32 + 1, col as i32 + 1);
                }
            }
        } else {
            for (row, col, _) in ws.sheet_data.cells() {
                if row >= 1 && col >= 1 && range.contains(row as u32 - 1, col as u32 - 1) {
                    add(row, col);
                }
            }
        }
        stats
    }

    /// Set several columns' widths (pixels) as one undo step; columns whose
    /// width would not change are skipped. Returns whether any changed.
    pub fn set_column_widths(&mut self, sheet: u32, widths: &[(u32, f64)]) -> Result<bool> {
        let changed: Vec<(u32, f64)> = widths
            .iter()
            .copied()
            .filter(|&(col, w)| (self.column_width(sheet, col) - w).abs() >= 0.5)
            .collect();
        for &(col, w) in &changed {
            if col >= MAX_COLS || !w.is_finite() {
                bail!("invalid column width");
            }
        }
        let mut steps = 0;
        for &(col, w) in &changed {
            let res =
                self.model
                    .set_columns_width(sheet, col as i32 + 1, col as i32 + 1, w.max(4.0));
            if let Err(e) = res {
                for _ in 0..steps {
                    let _ = self.model.undo();
                }
                return Err(anyhow!("{e}"));
            }
            steps += 1;
        }
        if steps > 0 {
            self.history.edited_steps(steps);
        }
        Ok(steps > 0)
    }

    /// The width (pixels) of columns nobody has resized.
    pub fn default_column_width(&self, sheet: u32) -> f64 {
        self.column_width(sheet, MAX_COLS - 1)
    }

    /// The displayed text of every filled cell in column `col`, with whether
    /// it is bold (for fitting the column's width to its content).
    pub fn column_texts(&self, sheet: u32, col: u32) -> Vec<(String, bool)> {
        let model = self.model.get_model();
        let Ok(ws) = model.workbook.worksheet(sheet) else {
            return Vec::new();
        };
        let col1 = col as i32 + 1;
        let mut rows: Vec<i32> = ws
            .sheet_data
            .cells()
            .filter(|(_, c, cell)| *c == col1 && !matches!(**cell, Cell::EmptyCell { .. }))
            .map(|(r, _, _)| r)
            .collect();
        rows.sort_unstable();
        rows.into_iter()
            .filter(|r| *r >= 1)
            .map(|r| {
                let row = r as u32 - 1;
                (
                    self.display_value(sheet, row, col),
                    self.cell_style(sheet, row, col).bold,
                )
            })
            .filter(|(text, _)| !text.is_empty())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(cells: &str) -> impl Fn(u32) -> bool + '_ {
        move |i| cells.as_bytes().get(i as usize) == Some(&b'x')
    }

    #[test]
    fn data_edge_like_excel() {
        //            0123456789
        let cells = "xxx..xx...";
        let f = line(cells);
        let last = 9;
        let end = cells.len() as u32;
        // Inside a block: to its end.
        assert_eq!(data_edge(0, true, last, end, &f), 2);
        // At a block's end: to the next block's start.
        assert_eq!(data_edge(2, true, last, end, &f), 5);
        // From a blank: the next filled cell.
        assert_eq!(data_edge(3, true, last, end, &f), 5);
        // Past the data: the edge.
        assert_eq!(data_edge(6, true, last, end, &f), 9);
        assert_eq!(data_edge(8, true, last, 7, &f), 9);
        // Backwards.
        assert_eq!(data_edge(6, false, last, end, &f), 5);
        assert_eq!(data_edge(5, false, last, end, &f), 2);
        assert_eq!(data_edge(4, false, last, end, &f), 2);
        assert_eq!(data_edge(0, false, last, end, &f), 0);
        assert_eq!(data_edge(9, true, last, end, &f), 9);
        // Empty line: straight to either edge.
        let empty = line("");
        assert_eq!(data_edge(3, true, 1_000_000, 0, &empty), 1_000_000);
        assert_eq!(data_edge(3, false, 1_000_000, 0, &empty), 0);
    }

    #[test]
    fn fill_from_drag() {
        let src = CellRange::from_corners((1, 1), (2, 2)); // B2:C3
        assert_eq!(Fill::from_drag(src, (2, 2)), None);
        assert_eq!(Fill::from_drag(src, (1, 1)), None);
        let down = Fill::from_drag(src, (6, 1)).unwrap();
        assert_eq!(down.direction, FillDirection::Down);
        assert_eq!(down.target().name(), "B2:C7");
        let up = Fill::from_drag(src, (0, 2)).unwrap();
        assert_eq!(up.direction, FillDirection::Up);
        assert_eq!(up.target().name(), "B1:C3");
        let right = Fill::from_drag(src, (3, 6)).unwrap();
        assert_eq!(right.direction, FillDirection::Right);
        assert_eq!(right.target().name(), "B2:G3");
        let left = Fill::from_drag(src, (2, 0)).unwrap();
        assert_eq!(left.direction, FillDirection::Left);
        assert_eq!(left.target().name(), "A2:C3");
        // Diagonal: the longer way wins, ties go down.
        assert_eq!(
            Fill::from_drag(src, (4, 4)).unwrap().direction,
            FillDirection::Down
        );
    }

    #[test]
    fn fill_down_and_right_ranges() {
        let r = CellRange::from_corners((0, 0), (3, 1));
        let d = Fill::down(r).unwrap();
        assert_eq!(d.source.name(), "A1:B1");
        assert_eq!(d.target(), r);
        let one_row = CellRange::from_corners((4, 0), (4, 2));
        let d = Fill::down(one_row).unwrap();
        assert_eq!(d.source.name(), "A4:C4");
        assert_eq!(d.target().name(), "A4:C5");
        assert_eq!(Fill::down(CellRange::from_corners((0, 0), (0, 0))), None);
        let r = Fill::right(CellRange::from_corners((2, 3), (2, 3))).unwrap();
        assert_eq!(r.source.name(), "C3");
        assert_eq!(r.target().name(), "C3:D3");
        assert_eq!(Fill::right(CellRange::from_corners((0, 0), (5, 0))), None);
    }

    #[test]
    fn stats_summary() {
        let mut s = RangeStats::default();
        s.add_number(1.0);
        assert_eq!(s.summary(), None, "one cell shows nothing");
        s.add_other();
        assert_eq!(
            s.summary().unwrap(),
            "Average: 1   Count: 2   Sum: 1",
            "text counts but is not summed"
        );
        s.add_number(1234.5);
        assert_eq!(s.average(), Some(617.75));
        assert_eq!(
            s.summary().unwrap(),
            "Average: 617.75   Count: 3   Sum: 1,235.5"
        );
        let mut t = RangeStats::default();
        t.add_other();
        t.add_other();
        assert_eq!(t.summary().unwrap(), "Count: 2");
    }

    #[test]
    fn stat_numbers() {
        assert_eq!(format_stat(0.0), "0");
        assert_eq!(format_stat(-0.0), "0");
        assert_eq!(format_stat(1234567.0), "1,234,567");
        assert_eq!(format_stat(-1234.25), "-1,234.25");
        assert_eq!(format_stat(1.0 / 3.0), "0.3333333333");
        assert_eq!(format_stat(0.1 + 0.2), "0.3");
        assert_eq!(format_stat(123.0), "123");
        assert_eq!(format_stat(f64::NAN), "—");
    }

    fn book(cells: &[(u32, u32, &str)]) -> Workbook {
        let mut wb = Workbook::new_empty().unwrap();
        for (r, c, v) in cells {
            wb.set_input(0, *r, *c, v).unwrap();
        }
        wb
    }

    #[test]
    fn fill_extends_series_and_shifts_references() {
        let mut wb = book(&[
            (0, 0, "1"),
            (1, 0, "2"),
            (0, 1, "Item 1"),
            (1, 1, "Item 2"),
            (0, 2, "=A1*10"),
            (1, 2, "=A2*10"),
            (0, 3, "x"),
            (1, 3, "x"),
            (0, 4, "=$A$1+A1"),
            (1, 4, "=$A$1+A2"),
        ]);
        let src = CellRange::from_corners((0, 0), (1, 4));
        let fill = Fill::from_drag(src, (4, 4)).unwrap();
        let target = wb.fill(0, &fill).unwrap();
        assert_eq!(target.name(), "A1:E5");
        let col = |wb: &Workbook, c| (0..5).map(|r| wb.input(0, r, c)).collect::<Vec<_>>();
        assert_eq!(col(&wb, 0), ["1", "2", "3", "4", "5"]);
        assert_eq!(col(&wb, 1)[4], "Item 5");
        assert_eq!(col(&wb, 2)[4], "=A5*10");
        assert_eq!(wb.display_value(0, 4, 2), "50");
        assert_eq!(col(&wb, 3)[4], "x");
        assert_eq!(col(&wb, 4)[4], "=$A$1+A5");
        // One undo step takes the whole fill back.
        assert!(wb.undo().unwrap());
        assert_eq!(wb.input(0, 2, 0), "");
        assert_eq!(wb.input(0, 1, 0), "2");
    }

    #[test]
    fn date_codes() {
        assert!(is_date_code("yyyy-mm-dd"));
        assert!(is_date_code("d/m/yy"));
        assert!(!is_date_code("#,##0.00"));
        assert!(!is_date_code("\"Rp\"#,##0"));
        assert!(!is_date_code("[Red]0.00"));
    }

    #[test]
    fn fill_date_series() {
        let mut wb = book(&[(0, 0, "1/30/2026"), (1, 0, "1/31/2026")]);
        let fill = Fill::from_drag(CellRange::from_corners((0, 0), (1, 0)), (3, 0)).unwrap();
        wb.fill(0, &fill).unwrap();
        assert_eq!(
            wb.display_value(0, 2, 0),
            "2/01/2026",
            "typed dates get m/dd/yyyy"
        );
        assert_eq!(wb.display_value(0, 3, 0), "2/02/2026");
        // Upwards.
        let fill = Fill::from_drag(CellRange::from_corners((2, 0), (3, 0)), (0, 0)).unwrap();
        wb.set_input(0, 0, 0, "").unwrap();
        wb.set_input(0, 1, 0, "").unwrap();
        wb.fill(0, &fill).unwrap();
        assert_eq!(wb.display_value(0, 0, 0), "1/30/2026");
        // One date counts up by a day; one undo takes the fill back.
        let mut wb = book(&[(0, 0, "1/30/2026")]);
        let fill = Fill::from_drag(CellRange::from_corners((0, 0), (0, 0)), (2, 0)).unwrap();
        wb.fill(0, &fill).unwrap();
        assert_eq!(
            wb.display_value(0, 2, 0),
            "2/01/2026",
            "typed dates get m/dd/yyyy"
        );
        wb.undo().unwrap();
        assert_eq!(wb.input(0, 1, 0), "");
        assert_eq!(wb.input(0, 0, 0), "1/30/2026");
    }

    #[test]
    fn fill_down_and_right_copy() {
        let mut wb = book(&[(0, 0, "5"), (0, 1, "=A1+1")]);
        let target = wb
            .fill(
                0,
                &Fill::down(CellRange::from_corners((0, 0), (2, 1))).unwrap(),
            )
            .unwrap();
        assert_eq!(target.name(), "A1:B3");
        assert_eq!(wb.input(0, 2, 0), "5", "Ctrl+D copies, no series");
        assert_eq!(wb.input(0, 2, 1), "=A3+1");
        wb.fill(
            0,
            &Fill::right(CellRange::from_corners((0, 2), (0, 2))).unwrap(),
        )
        .unwrap();
        assert_eq!(wb.input(0, 0, 2), "=B1+1");
    }

    #[test]
    fn insert_and_delete_shift_formulas() {
        let mut wb = book(&[(0, 0, "1"), (1, 0, "2"), (2, 0, "=A1+A2")]);
        wb.insert_rows(0, 1, 2).unwrap();
        assert_eq!(wb.input(0, 3, 0), "2");
        assert_eq!(wb.input(0, 4, 0), "=A1+A4");
        assert_eq!(wb.display_value(0, 4, 0), "3");
        wb.delete_rows(0, 1, 2).unwrap();
        assert_eq!(wb.input(0, 2, 0), "=A1+A2");
        wb.insert_columns(0, 0, 1).unwrap();
        assert_eq!(wb.input(0, 2, 1), "=B1+B2");
        wb.delete_columns(0, 0, 0).unwrap();
        assert_eq!(wb.input(0, 2, 0), "=A1+A2");
        // Each is one undo step.
        wb.undo().unwrap();
        assert_eq!(wb.input(0, 2, 1), "=B1+B2");
        assert!(wb.delete_rows(0, 3, 1).is_err());
    }

    #[test]
    fn jump_to_data_edges() {
        let wb = book(&[(0, 0, "a"), (1, 0, "b"), (2, 0, "c"), (6, 0, "d")]);
        assert_eq!(wb.jump(0, (0, 0), Step::Down), (2, 0));
        assert_eq!(wb.jump(0, (2, 0), Step::Down), (6, 0));
        assert_eq!(wb.jump(0, (6, 0), Step::Down), (MAX_ROWS - 1, 0));
        assert_eq!(wb.jump(0, (6, 0), Step::Up), (2, 0));
        assert_eq!(wb.jump(0, (0, 0), Step::Right), (0, MAX_COLS - 1));
        assert_eq!(wb.jump(0, (0, 5), Step::Left), (0, 0));
    }

    #[test]
    fn frozen_panes_undo_as_one_and_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frozen.xlsx");
        let mut wb = book(&[(0, 0, "head")]);
        assert_eq!(wb.frozen(0), (0, 0));
        assert!(wb.set_frozen(0, 1, 2).unwrap());
        assert!(!wb.set_frozen(0, 1, 2).unwrap(), "no change, no undo step");
        assert_eq!(wb.frozen(0), (1, 2));
        wb.save_as(&path).unwrap();
        let reopened = Workbook::open(&path).unwrap();
        assert_eq!(reopened.frozen(0), (1, 2));

        assert!(wb.undo().unwrap());
        assert_eq!(wb.frozen(0), (0, 0), "both counts undo as one step");
        assert!(wb.redo().unwrap());
        assert_eq!(wb.frozen(0), (1, 2));
        assert!(wb.undo().unwrap());
        assert!(wb.undo().unwrap());
        assert_eq!(wb.input(0, 0, 0), "", "and the step before is intact");
    }

    #[test]
    fn stats_of_a_range() {
        let wb = book(&[
            (0, 0, "10"),
            (1, 0, "=A1*2"),
            (2, 0, "text"),
            (3, 0, "TRUE"),
            (0, 5, "1000"),
        ]);
        let s = wb.range_stats(0, CellRange::columns(0, 0));
        assert_eq!((s.count, s.numbers, s.sum), (4, 2, 30.0));
        let all = wb.range_stats(0, CellRange::all());
        assert_eq!(all.sum, 1030.0);
    }

    #[test]
    fn column_widths_undo_as_one() {
        let mut wb = book(&[]);
        let default = wb.default_column_width(0);
        assert!(
            wb.set_column_widths(0, &[(0, 50.0), (2, 200.0), (3, default)])
                .unwrap()
        );
        assert_eq!(wb.column_width(0, 0), 50.0);
        assert_eq!(wb.column_width(0, 2), 200.0);
        assert!(!wb.set_column_widths(0, &[(0, 50.0)]).unwrap());
        wb.undo().unwrap();
        assert_eq!(wb.column_width(0, 0), default);
        assert_eq!(wb.column_width(0, 2), default);
        assert!(!wb.can_undo());
    }

    #[test]
    fn texts_of_a_column() {
        let wb = book(&[(3, 1, "wide text"), (0, 1, "1"), (0, 0, "other")]);
        assert_eq!(
            wb.column_texts(0, 1),
            [("1".to_owned(), false), ("wide text".to_owned(), false)]
        );
    }
}
