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

use std::collections::HashMap;

use crate::{CellRange, MAX_COLS, MAX_ROWS, Workbook};

/// The top-left cells of the frozen panes of each worksheet of a package
/// (`sheetView@topLeftCell` where the pane is frozen), by `sheetId`, when
/// not A1. `skeletons`: [`crate::preserve::worksheet_skeletons`].
pub(crate) fn read_panes(
    skeletons: &std::result::Result<Vec<(u32, String)>, String>,
) -> HashMap<u32, (u32, u32)> {
    let mut out = HashMap::new();
    let Ok(list) = skeletons else {
        return out;
    };
    for (id, xml) in list {
        // Cheap test first: most sheets have no frozen panes.
        if !xml.contains("frozen") {
            continue;
        }
        let Ok(doc) = roxmltree::Document::parse(xml) else {
            continue;
        };
        let Some(view) = el(doc.root_element(), "sheetViews").and_then(|v| el(v, "sheetView"))
        else {
            continue;
        };
        let frozen = el(view, "pane")
            .is_some_and(|p| matches!(p.attribute("state"), Some("frozen" | "frozenSplit")));
        if let (true, Some(cell)) = (frozen, view.attribute("topLeftCell").and_then(parse_a1))
            && cell != (0, 0)
        {
            out.insert(*id, cell);
        }
    }
    out
}

fn el<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

/// `"B10"` -> `(9, 1)` (`$` signs allowed).
fn parse_a1(s: &str) -> Option<(u32, u32)> {
    let s = s.replace('$', "");
    let split = s.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = s.split_at(split);
    if letters.is_empty() || !letters.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let col = letters.chars().try_fold(0u32, |n, c| {
        n.checked_mul(26)?
            .checked_add(c.to_ascii_uppercase() as u32 - 'A' as u32 + 1)
    })?;
    let row: u32 = digits.parse().ok()?;
    (row >= 1 && col >= 1 && row <= MAX_ROWS && col <= MAX_COLS).then(|| (row - 1, col - 1))
}

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
/// next filled cell, or the sheet's edge when there is none. Lines that
/// aren't `shown` (hidden rows and columns, rows a filter hides) are passed
/// over as if they weren't there, as in Excel.
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
    shown: impl Fn(u32) -> bool,
) -> u32 {
    let next = |mut i: u32| loop {
        i = if forward {
            (i < last).then(|| i + 1)?
        } else {
            i.checked_sub(1)?
        };
        if shown(i) {
            return Some(i);
        }
    };
    // The far edge: the last shown index (or `from` if none past it is).
    let edge = || {
        let mut i = last;
        while i > from && !shown(i) {
            i -= 1;
        }
        i
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
            return edge();
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
pub(crate) fn is_date_code(code: &str) -> bool {
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
            let hidden = self.hidden_row_set(sheet);
            let origin = self.frozen_origin(sheet).0;
            let row = data_edge(
                row,
                step.forward(),
                MAX_ROWS - 1,
                rows,
                |r| self.is_filled(sheet, r, col),
                |r| r >= origin && !hidden.contains(&r),
            );
            (row, col)
        } else {
            let col = data_edge(
                col,
                step.forward(),
                MAX_COLS - 1,
                cols,
                |c| self.is_filled(sheet, row, c),
                |c| !self.col_skipped(sheet, c),
            );
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
        self.check_pivots(sheet, true, true, row, count)?;
        self.model
            .insert_rows(sheet, row as i32 + 1, count as i32)
            .map_err(|e| anyhow!("{e}"))?;
        let sheet_id = self.sheet_id(sheet);
        self.history.moved(crate::shift::Op::InsertRows {
            sheet: sheet_id,
            at: row,
            count,
        });
        self.sync_pivot_grids(true);
        Ok(())
    }

    /// Delete rows `first..=last` (one undo step).
    pub fn delete_rows(&mut self, sheet: u32, first: u32, last: u32) -> Result<()> {
        if first > last || last >= MAX_ROWS {
            bail!("row out of range");
        }
        self.check_pivots(sheet, true, false, first, last - first + 1)?;
        self.model
            .delete_rows(sheet, first as i32 + 1, (last - first + 1) as i32)
            .map_err(|e| anyhow!("{e}"))?;
        let sheet_id = self.sheet_id(sheet);
        self.history.moved(crate::shift::Op::DeleteRows {
            sheet: sheet_id,
            at: first,
            count: last - first + 1,
        });
        self.sync_pivot_grids(true);
        Ok(())
    }

    /// Insert `count` empty columns before `col` (one undo step).
    pub fn insert_columns(&mut self, sheet: u32, col: u32, count: u32) -> Result<()> {
        if count == 0 || col >= MAX_COLS {
            bail!("column out of range");
        }
        self.check_pivots(sheet, false, true, col, count)?;
        self.model
            .insert_columns(sheet, col as i32 + 1, count as i32)
            .map_err(|e| anyhow!("{e}"))?;
        // Columns inserted inside a table got names (`Column1`...): their
        // header cells show them, as in Excel.
        let mut steps = 1;
        for t in self.tables(sheet) {
            let Some(header) = t.header_row() else {
                continue;
            };
            for c in col..col + count {
                if c > t.area.left && c <= t.area.right && !self.is_filled(sheet, header, c) {
                    let name = t.columns[(c - t.area.left) as usize].clone();
                    if self
                        .model
                        .set_user_input(sheet, header as i32 + 1, c as i32 + 1, &name)
                        .is_ok()
                    {
                        steps += 1;
                    }
                }
            }
        }
        let sheet_id = self.sheet_id(sheet);
        self.history.moved_steps(
            crate::shift::Op::InsertColumns {
                sheet: sheet_id,
                at: col,
                count,
            },
            steps,
        );
        self.sync_pivot_grids(true);
        Ok(())
    }

    /// Delete columns `first..=last` (one undo step).
    pub fn delete_columns(&mut self, sheet: u32, first: u32, last: u32) -> Result<()> {
        if first > last || last >= MAX_COLS {
            bail!("column out of range");
        }
        self.check_pivots(sheet, false, false, first, last - first + 1)?;
        self.model
            .delete_columns(sheet, first as i32 + 1, (last - first + 1) as i32)
            .map_err(|e| anyhow!("{e}"))?;
        let sheet_id = self.sheet_id(sheet);
        self.history.moved(crate::shift::Op::DeleteColumns {
            sheet: sheet_id,
            at: first,
            count: last - first + 1,
        });
        self.sync_pivot_grids(true);
        Ok(())
    }

    /// Frozen `(rows, columns)` at the top / left of `sheet`.
    pub fn frozen(&self, sheet: u32) -> (u32, u32) {
        let m = self.model.get_model();
        let rows = m.get_frozen_rows_count(sheet).unwrap_or(0).max(0) as u32;
        let cols = m.get_frozen_columns_count(sheet).unwrap_or(0).max(0) as u32;
        (rows, cols)
    }

    /// Freeze the first `rows` rows and `cols` columns (0 unfreezes). Saved
    /// to `.xlsx` as a frozen pane. Like Excel's Freeze Panes it is not an
    /// undo step, but the workbook has unsaved changes. Returns whether
    /// anything changed.
    pub fn set_frozen(&mut self, sheet: u32, rows: u32, cols: u32) -> Result<bool> {
        self.set_frozen_at(sheet, (0, 0), (rows, cols))
    }

    /// Excel's Freeze Panes on a scrolled sheet: the panes show rows
    /// `top..split.0` and columns `left..split.1` (`origin` is `(top,
    /// left)`, the cell at the panes' top-left); the rows above and columns
    /// left of `origin` are out of sight until the panes are unfrozen, as in
    /// Excel. [`Workbook::frozen`] still counts from `A1` (`split`). Saved as
    /// the sheet view's `topLeftCell` with the pane's `xSplit`/`ySplit`
    /// counted from it. Not an undo step (see [`Workbook::set_frozen`]).
    pub fn set_frozen_at(
        &mut self,
        sheet: u32,
        origin: (u32, u32),
        split: (u32, u32),
    ) -> Result<bool> {
        let (rows, cols) = split;
        if rows >= MAX_ROWS || cols >= MAX_COLS {
            bail!("too many frozen rows or columns");
        }
        // An origin only applies to the way that is frozen, inside it.
        let origin = (
            if origin.0 < rows { origin.0 } else { 0 },
            if origin.1 < cols { origin.1 } else { 0 },
        );
        if self.frozen(sheet) == split && self.frozen_origin(sheet) == origin {
            return Ok(false);
        }
        if self.frozen(sheet) != split {
            self.model
                .kantor_set_frozen(sheet, rows as i32, cols as i32)
                .map_err(|e| anyhow!("{e}"))?;
        }
        let id = self.sheet_id(sheet);
        if origin == (0, 0) {
            self.panes.remove(&id);
        } else {
            self.panes.insert(id, origin);
        }
        self.history.view_changed = true;
        self.history.view_changes += 1;
        Ok(true)
    }

    /// The top-left cell of `sheet`'s frozen panes: `(0, 0)` unless they
    /// were frozen on a scrolled sheet ([`Workbook::set_frozen_at`]).
    pub fn frozen_origin(&self, sheet: u32) -> (u32, u32) {
        let (rows, cols) = self.frozen(sheet);
        let (top, left) = self
            .panes
            .get(&self.sheet_id(sheet))
            .copied()
            .unwrap_or_default();
        (
            if top < rows { top } else { 0 },
            if left < cols { left } else { 0 },
        )
    }

    /// Whether keyboard moves pass over row `row`: hidden by hand or by a
    /// filter, or above frozen panes frozen on a scrolled sheet.
    pub fn row_skipped(&self, sheet: u32, row: u32) -> bool {
        row < self.frozen_origin(sheet).0 || self.hidden_row_set(sheet).contains(&row)
    }

    /// Whether keyboard moves pass over column `col`: hidden, or left of
    /// frozen panes frozen on a scrolled sheet.
    pub fn col_skipped(&self, sheet: u32, col: u32) -> bool {
        col < self.frozen_origin(sheet).1 || self.is_column_hidden(sheet, col)
    }

    /// The rows of `sheet` that are hidden (by hand or by a filter), for the
    /// current state.
    pub fn hidden_row_set(&self, sheet: u32) -> std::sync::Arc<std::collections::HashSet<u32>> {
        if let Some(set) = self.cached(|c| c.hidden_rows.get(&sheet).cloned()) {
            return set;
        }
        let set: std::collections::HashSet<u32> =
            match self.model.get_model().workbook.worksheet(sheet) {
                Ok(ws) => ws
                    .rows
                    .iter()
                    .filter(|r| r.hidden && r.r >= 1)
                    .map(|r| r.r as u32 - 1)
                    .collect(),
                Err(_) => Default::default(),
            };
        let set = std::sync::Arc::new(set);
        self.cached(|c| c.hidden_rows.insert(sheet, set.clone()));
        set
    }

    /// The nearest cell to `(row, col)` that keyboard moves can land on:
    /// itself, or the next shown row below / column right (else above /
    /// left), as Excel does after a jump to a hidden cell.
    pub fn nearest_shown(&self, sheet: u32, (row, col): (u32, u32)) -> (u32, u32) {
        let near = |at: u32, last: u32, skip: &dyn Fn(u32) -> bool| {
            (at..=last)
                .find(|&i| !skip(i))
                .or_else(|| (0..at).rev().find(|&i| !skip(i)))
                .unwrap_or(at)
        };
        let hidden = self.hidden_row_set(sheet);
        let top = self.frozen_origin(sheet).0;
        let row = near(row, MAX_ROWS - 1, &|r| r < top || hidden.contains(&r));
        let col = near(col, MAX_COLS - 1, &|c| self.col_skipped(sheet, c));
        (row, col)
    }

    /// Save patches for panes frozen on a scrolled sheet (IronCalc writes
    /// frozen panes counted from A1).
    pub(crate) fn pane_patches(&self, out: &mut crate::preserve::Patches) {
        for sheet in 0..self.sheet_names().len() as u32 {
            let origin = self.frozen_origin(sheet);
            if origin != (0, 0) {
                let id = self.sheet_id(sheet);
                out.sheets.entry(id).or_default().frozen_view = Some((origin, self.frozen(sheet)));
            }
        }
    }

    /// After opening: panes the file froze on a scrolled sheet. IronCalc
    /// read their splits counted from the sheet view's `topLeftCell`; make
    /// them count from A1 and remember the top-left cell.
    pub(crate) fn apply_read_panes(&mut self, panes: &HashMap<u32, (u32, u32)>) {
        for sheet in 0..self.sheet_names().len() as u32 {
            let id = self.sheet_id(sheet);
            let Some(&(top, left)) = panes.get(&id) else {
                continue;
            };
            let (rows, cols) = self.frozen(sheet);
            let top = if rows > 0 { top } else { 0 };
            let left = if cols > 0 { left } else { 0 };
            let split = (rows + top, cols + left);
            if (top, left) == (0, 0) || split.0 >= MAX_ROWS || split.1 >= MAX_COLS {
                continue;
            }
            if self
                .model
                .kantor_set_frozen(sheet, split.0 as i32, split.1 as i32)
                .is_ok()
            {
                self.panes.insert(id, (top, left));
            }
        }
    }

    /// Status-bar numbers for `range`. Small ranges are read cell by cell,
    /// large ones (whole columns) by walking the cells that exist.
    pub fn range_stats(&self, sheet: u32, range: CellRange) -> RangeStats {
        let cached = self.cached(|c| {
            c.stats
                .filter(|(s, r, _)| (*s, *r) == (sheet, range))
                .map(|(_, _, stats)| stats)
        });
        if let Some(stats) = cached {
            return stats;
        }
        let stats = self.count_range(sheet, range);
        self.cached(|c| c.stats = Some((sheet, range, stats)));
        stats
    }

    fn count_range(&self, sheet: u32, range: CellRange) -> RangeStats {
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
        // A hidden column dragged open is shown, in the same undo step; it
        // is shown first, so undoing the width restores its real one.
        let shown: Vec<(i32, bool)> = changed
            .iter()
            .filter(|&&(col, w)| w >= 1.0 && self.is_column_hidden(sheet, col))
            .map(|&(col, _)| (col as i32 + 1, false))
            .collect();
        if !shown.is_empty()
            && self
                .model
                .kantor_set_columns_hidden(sheet, &shown)
                .map_err(|e| anyhow!("{e}"))?
        {
            steps += 1;
        }
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
        assert_eq!(data_edge(0, true, last, end, &f, |_| true), 2);
        // At a block's end: to the next block's start.
        assert_eq!(data_edge(2, true, last, end, &f, |_| true), 5);
        // From a blank: the next filled cell.
        assert_eq!(data_edge(3, true, last, end, &f, |_| true), 5);
        // Past the data: the edge.
        assert_eq!(data_edge(6, true, last, end, &f, |_| true), 9);
        assert_eq!(data_edge(8, true, last, 7, &f, |_| true), 9);
        // Backwards.
        assert_eq!(data_edge(6, false, last, end, &f, |_| true), 5);
        assert_eq!(data_edge(5, false, last, end, &f, |_| true), 2);
        assert_eq!(data_edge(4, false, last, end, &f, |_| true), 2);
        assert_eq!(data_edge(0, false, last, end, &f, |_| true), 0);
        assert_eq!(data_edge(9, true, last, end, &f, |_| true), 9);
        // Empty line: straight to either edge.
        let empty = line("");
        assert_eq!(
            data_edge(3, true, 1_000_000, 0, &empty, |_| true),
            1_000_000
        );
        assert_eq!(data_edge(3, false, 1_000_000, 0, &empty, |_| true), 0);
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

    /// A big fill is one undo step that restores exactly, and IronCalc's
    /// sync queue (a copy of every diff list) is off: on a sheet without
    /// conditional formatting nothing ever emptied it.
    #[test]
    fn big_fill_undoes_in_one_step_and_keeps_no_queue() {
        let mut wb = book(&[(0, 0, "5"), (0, 1, "=A1*2"), (0, 2, "=SUM($A$1:A1)")]);
        let before = (wb.input(0, 1999, 1), wb.used_size(0));
        wb.fill(
            0,
            &Fill::down(CellRange::from_corners((0, 0), (1999, 2))).unwrap(),
        )
        .unwrap();
        assert_eq!(wb.input(0, 1999, 1), "=A2000*2");
        assert_eq!(wb.input(0, 1999, 2), "=SUM($A$1:A2000)");
        assert_eq!(wb.display_value(0, 1999, 2), "10000");
        let empty = ironcalc_base::UserModel::new_empty("x", "en", "UTC", "en")
            .unwrap()
            .flush_send_queue();
        assert_eq!(wb.model.flush_send_queue(), empty);
        wb.undo().unwrap();
        assert_eq!((wb.input(0, 1999, 1), wb.used_size(0)), before);
        assert_eq!(wb.input(0, 0, 1), "=A1*2");
        wb.redo().unwrap();
        assert_eq!(wb.input(0, 1999, 2), "=SUM($A$1:A2000)");
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
    fn data_edge_passes_over_hidden_lines() {
        //            0123456789
        let cells = "xx.x..x...";
        let f = line(cells);
        let end = cells.len() as u32;
        // 2 hidden: the block 0..=3 is one block, as Excel sees it.
        let shown = |i: u32| i != 2;
        assert_eq!(data_edge(0, true, 9, end, &f, shown), 3);
        assert_eq!(data_edge(3, false, 9, end, &f, shown), 0);
        // A hidden filled cell is no stop: 3 hidden, 1 -> 6.
        let shown = |i: u32| i != 3 && i != 2;
        assert_eq!(data_edge(1, true, 9, end, &f, shown), 6);
        // The edge is the last shown index.
        let shown = |i: u32| i < 8;
        assert_eq!(data_edge(6, true, 9, end, &f, shown), 7);
        let shown = |i: u32| i > 0;
        assert_eq!(data_edge(1, false, 9, end, &f, shown), 1);
        // Nothing shown that way: stay.
        assert_eq!(data_edge(1, false, 9, end, &f, |i| i > 0), 1);
    }

    #[test]
    fn jumps_skip_hidden_and_filtered_rows_and_hidden_columns() {
        use crate::FilterScope;
        let mut wb = book(&[
            (0, 0, "h"),
            (1, 0, "a"),
            (2, 0, "b"),
            (3, 0, "a"),
            (4, 0, "b"),
            (5, 0, "a"),
            (0, 1, "x"),
            (0, 2, "y"),
            (0, 4, "z"),
        ]);
        // Hide rows 2..=4 by hand: Ctrl+Down from A2 lands on A6.
        wb.hide_rows(0, 2, 4).unwrap();
        assert_eq!(wb.jump(0, (1, 0), Step::Down), (5, 0));
        assert_eq!(wb.jump(0, (5, 0), Step::Up), (0, 0));
        assert!(wb.row_skipped(0, 3) && !wb.row_skipped(0, 5));
        wb.undo().unwrap();
        // Filtered-out rows too: keep only "a" (rows 1, 3, 5).
        wb.toggle_filter(0, CellRange::from_corners((0, 0), (5, 0)))
            .unwrap();
        wb.set_column_filter(
            0,
            &FilterScope::Sheet,
            0,
            Some(crate::FilterKind::Values {
                values: vec!["a".into()],
                blanks: false,
                dates: Vec::new(),
            }),
        )
        .unwrap();
        assert!(wb.row_skipped(0, 2) && wb.row_skipped(0, 4));
        assert_eq!(wb.jump(0, (1, 0), Step::Down), (5, 0));
        assert_eq!(wb.nearest_shown(0, (2, 0)), (3, 0));
        // Hidden blank column D between C and E: C1 and E1 are one block.
        assert_eq!(
            wb.jump(0, (0, 2), Step::Right),
            (0, 4),
            "D blank: next block"
        );
        wb.set_columns_hidden(0, 3, 3, true).unwrap();
        assert_eq!(wb.jump(0, (0, 0), Step::Right), (0, 4));
        // With C hidden too, a hidden filled cell is no stop either.
        wb.set_columns_hidden(0, 2, 2, true).unwrap();
        assert!(wb.col_skipped(0, 2));
        assert_eq!(wb.jump(0, (0, 4), Step::Left), (0, 0));
    }

    #[test]
    fn panes_frozen_on_a_scrolled_sheet_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scrolled.xlsx");
        let mut wb = book(&[(0, 0, "x"), (30, 5, "y")]);
        // Scrolled to C11, freeze at E15: rows 11..14 and columns C..D stay.
        assert!(wb.set_frozen_at(0, (10, 2), (14, 4)).unwrap());
        assert!(!wb.set_frozen_at(0, (10, 2), (14, 4)).unwrap(), "no change");
        assert_eq!(wb.frozen(0), (14, 4));
        assert_eq!(wb.frozen_origin(0), (10, 2));
        // Rows above and columns left of the panes are out of reach.
        assert!(wb.row_skipped(0, 9) && !wb.row_skipped(0, 10));
        assert!(wb.col_skipped(0, 1) && !wb.col_skipped(0, 2));
        assert_eq!(wb.jump(0, (12, 3), Step::Up), (10, 3));
        assert_eq!(wb.nearest_shown(0, (0, 0)), (10, 2));
        assert!(wb.is_dirty());
        wb.save_as(&path).unwrap();
        let xml = String::from_utf8(
            crate::preserve::read_part(&std::fs::read(&path).unwrap(), "xl/worksheets/sheet1.xml")
                .unwrap(),
        )
        .unwrap();
        assert!(xml.contains("topLeftCell=\"C11\""), "{xml}");
        assert!(
            xml.contains("<pane xSplit=\"2\" ySplit=\"4\" topLeftCell=\"E15\" activePane=\"bottomRight\" state=\"frozen\"/>"),
            "{xml}"
        );
        let mut back = Workbook::open(&path).unwrap();
        assert_eq!(back.frozen(0), (14, 4));
        assert_eq!(back.frozen_origin(0), (10, 2));
        assert!(!back.is_dirty());
        // Saved twice (the saved file is now the original): still right.
        back.set_input(0, 1, 1, "z").unwrap();
        back.save_as(&path).unwrap();
        let again = Workbook::open(&path).unwrap();
        assert_eq!(
            (again.frozen(0), again.frozen_origin(0)),
            ((14, 4), (10, 2))
        );
        // Freeze Top Row then: the origin goes, the file has a plain pane.
        back.set_frozen(0, 1, 0).unwrap();
        assert_eq!(back.frozen_origin(0), (0, 0));
        back.save_as(&path).unwrap();
        let plain = Workbook::open(&path).unwrap();
        assert_eq!((plain.frozen(0), plain.frozen_origin(0)), ((1, 0), (0, 0)));
        // Undo doesn't touch panes, as in Excel.
        back.undo().unwrap();
        assert_eq!(back.frozen(0), (1, 0));
    }

    #[test]
    fn frozen_panes_are_saved_but_not_undone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frozen.xlsx");
        let mut wb = book(&[(0, 0, "x")]);
        wb.save_as(&path).unwrap();
        assert!(!wb.is_dirty());
        assert_eq!(wb.frozen(0), (0, 0));
        assert!(wb.set_frozen(0, 1, 2).unwrap());
        assert!(!wb.set_frozen(0, 1, 2).unwrap(), "no change");
        assert_eq!(wb.frozen(0), (1, 2));
        assert!(wb.is_dirty(), "freezing is a change to save");
        // As in Excel, Ctrl+Z doesn't unfreeze: it undoes the edit before.
        wb.undo().unwrap();
        assert_eq!(wb.frozen(0), (1, 2));
        assert_eq!(wb.display_value(0, 0, 0), "");
        wb.redo().unwrap();
        wb.save_as(&path).unwrap();
        assert!(!wb.is_dirty());
        let reopened = Workbook::open(&path).unwrap();
        assert_eq!(reopened.frozen(0), (1, 2));
        assert_eq!(reopened.display_value(0, 0, 0), "x");
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
