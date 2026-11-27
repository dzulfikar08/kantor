//! `Range`: cells of one sheet (one or more areas), as a collection of
//! cells, rows (`.Rows`) or columns (`.Columns`).

use std::any::Any;
use std::cell::RefMut;
use std::rc::Rc;

use kantor_script::{Arg, Array, Dispatch, ObjectRef, ScriptError, Value};
use sheets_engine::script::KantorValue;
use sheets_engine::{CellRange, MAX_COLS, MAX_ROWS, Workbook};

use crate::addr;
use crate::consts::*;
use crate::format::{BordersObj, FontObj, InteriorObj};
use crate::session::{FindState, Selection, Session};
use crate::sheet::SheetObj;
use crate::values::*;

/// What `Count`, `Item` and `For Each` go over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Cells,
    Rows,
    Columns,
}

pub struct RangeObj {
    pub s: Rc<Session>,
    pub sheet: u32,
    pub areas: Vec<CellRange>,
    pub mode: Mode,
}

impl RangeObj {
    pub fn new(s: &Rc<Session>, sheet: u32, area: CellRange) -> Rc<RangeObj> {
        Rc::new(RangeObj {
            s: s.clone(),
            sheet,
            areas: vec![area],
            mode: Mode::Cells,
        })
    }

    pub fn multi(s: &Rc<Session>, sheet: u32, areas: Vec<CellRange>) -> Rc<RangeObj> {
        Rc::new(RangeObj {
            s: s.clone(),
            sheet,
            areas,
            mode: Mode::Cells,
        })
    }

    pub fn first(&self) -> CellRange {
        self.areas[0]
    }

    fn with_mode(&self, mode: Mode) -> Rc<RangeObj> {
        Rc::new(RangeObj {
            s: self.s.clone(),
            sheet: self.sheet,
            areas: self.areas.clone(),
            mode,
        })
    }

    fn at(&self, area: CellRange) -> Value {
        obj(RangeObj::new(&self.s, self.sheet, area))
    }

    pub fn idx(&self) -> Result<u32, ScriptError> {
        self.s.sheet_index(self.sheet)
    }

    /// The cells of an area inside the used part of the sheet (whole rows
    /// and columns are mostly empty).
    fn used(&self, book: &Workbook, idx: u32, area: CellRange) -> Option<CellRange> {
        let (rows, cols) = book.used_size(idx);
        area.clip(rows.max(1), cols.max(1))
    }

    /// The cell `(row, col)` (1-based, may lie outside) relative to the
    /// first area's top-left.
    fn cell_at(&self, row: i64, col: i64) -> Result<CellRange, ScriptError> {
        let a = self.first();
        let r = i64::from(a.top) + row - 1;
        let c = i64::from(a.left) + col - 1;
        if r < 0 || c < 0 || r >= i64::from(MAX_ROWS) || c >= i64::from(MAX_COLS) {
            return Err(err1004("Application-defined or object-defined error"));
        }
        Ok(CellRange::from_corners(
            (r as u32, c as u32),
            (r as u32, c as u32),
        ))
    }

    fn count(&self) -> i64 {
        self.areas
            .iter()
            .map(|a| match self.mode {
                Mode::Cells => i64::from(a.rows_count()) * i64::from(a.cols_count()),
                Mode::Rows => i64::from(a.rows_count()),
                Mode::Columns => i64::from(a.cols_count()),
            })
            .sum()
    }

    /// `Item(i)` / `Item(row, col)` / `Cells(row, col)`.
    fn item(&self, args: &[Arg]) -> Result<Value, ScriptError> {
        let a = self.first();
        let row = need(args, 0, "RowIndex")?;
        let col = arg(args, 1, "ColumnIndex");
        if col.is_none()
            && let Value::String(s) = scalar(row)?
        {
            return match self.mode {
                // `Columns("B")`, `Columns("B:D")`, `Rows("2:4")`.
                Mode::Columns | Mode::Rows => {
                    let text = if s.contains(':') {
                        s.clone()
                    } else {
                        format!("{s}:{s}")
                    };
                    let r = addr::parse_area(&text)
                        .filter(|r| r.is_whole_columns() || r.is_whole_rows())
                        .ok_or_else(|| ScriptError::runtime(13))?;
                    let r = if r.is_whole_columns() {
                        CellRange::from_corners((a.top, r.left), (a.bottom, r.right))
                    } else {
                        CellRange::from_corners((r.top, a.left), (r.bottom, a.right))
                    };
                    Ok(obj(Rc::new(RangeObj {
                        s: self.s.clone(),
                        sheet: self.sheet,
                        areas: vec![r],
                        mode: self.mode,
                    })))
                }
                // `Range("A1:C3")("B2")`: an address relative to the range.
                Mode::Cells => self.relative_range(&s),
            };
        }
        let i = to_i64(row)?;
        match (self.mode, col) {
            (Mode::Rows, None) => {
                let r = i64::from(a.top) + i - 1;
                if r < 0 || r >= i64::from(MAX_ROWS) {
                    return Err(err1004("Application-defined or object-defined error"));
                }
                Ok(self.at(CellRange::from_corners(
                    (r as u32, a.left),
                    (r as u32, a.right),
                )))
            }
            (Mode::Columns, None) => {
                let c = i64::from(a.left) + i - 1;
                if c < 0 || c >= i64::from(MAX_COLS) {
                    return Err(err1004("Application-defined or object-defined error"));
                }
                Ok(self.at(CellRange::from_corners(
                    (a.top, c as u32),
                    (a.bottom, c as u32),
                )))
            }
            (_, None) => {
                // Row-major within the first area's width.
                let w = i64::from(a.cols_count());
                let k = i - 1;
                if k < 0 {
                    return Err(err1004("Application-defined or object-defined error"));
                }
                Ok(self.at(self.cell_at(k / w + 1, k % w + 1)?))
            }
            (_, Some(c)) => {
                let c = match scalar(c)? {
                    Value::String(s) => match addr::parse_col(&s) {
                        Some(n) => i64::from(n) - i64::from(a.left) + 1,
                        None => return Err(ScriptError::runtime(13)),
                    },
                    v => to_i64(&v)?,
                };
                Ok(self.at(self.cell_at(i, c)?))
            }
        }
    }

    /// `rng.Range("A1")`: an address taken relative to this range.
    fn relative_range(&self, text: &str) -> Result<Value, ScriptError> {
        let a = self.first();
        let r = addr::parse_area(text)
            .ok_or_else(|| err1004(format!("Method 'Range' failed: {text}")))?;
        let moved = CellRange {
            top: r.top + a.top,
            bottom: (r.bottom + a.top).min(MAX_ROWS - 1),
            left: r.left + a.left,
            right: (r.right + a.left).min(MAX_COLS - 1),
        };
        Ok(self.at(moved))
    }

    /// Values (`formulas`: inputs as typed) of the first area.
    fn read(&self, what: Read) -> Result<Value, ScriptError> {
        let idx = self.idx()?;
        let mut book = self.s.read()?;
        let a = self.first();
        if a.is_single() {
            return Ok(read_cell(&mut book, idx, a.top, a.left, what));
        }
        // A whole column or row: its used part.
        let a = match (
            a.is_whole_columns() || a.is_whole_rows(),
            self.used(&book, idx, a),
        ) {
            (true, Some(u)) => u,
            (true, None) => CellRange::from_corners((a.top, a.left), (a.top, a.left)),
            (false, _) => a,
        };
        if u64::from(a.rows_count()) * u64::from(a.cols_count()) > 20_000_000 {
            return Err(ScriptError::runtime(7));
        }
        let rows = (a.top..=a.bottom)
            .map(|r| {
                (a.left..=a.right)
                    .map(|c| read_cell(&mut book, idx, r, c, what))
                    .collect()
            })
            .collect();
        Ok(Value::array(Array::from_rows(1, rows)))
    }

    /// Write `v` to every area: an array cell by cell, anything else to
    /// each cell.
    fn write(&self, v: &Value, what: Write) -> Result<(), ScriptError> {
        let idx = self.idx()?;
        let v = match v {
            Value::Object(Some(_)) => scalar(v)?,
            v => v.clone(),
        };
        let mut book = self.s.write()?;
        for &area in &self.areas {
            let area = if area.is_whole_columns() || area.is_whole_rows() {
                match self.used(&book, idx, area) {
                    Some(u) => u,
                    None => continue,
                }
            } else {
                area
            };
            match &v {
                Value::Array(arr) => {
                    let rows = arr.rows();
                    for (i, r) in (area.top..=area.bottom).enumerate() {
                        let row = if rows.len() == 1 {
                            rows.first()
                        } else {
                            rows.get(i)
                        };
                        for (j, c) in (area.left..=area.right).enumerate() {
                            let x = match row {
                                Some(row) if row.len() == 1 => row.first(),
                                Some(row) => row.get(j),
                                None => None,
                            };
                            let x = x.cloned().unwrap_or(Value::Error(2042));
                            write_cell(&mut book, idx, r, c, &x, what)?;
                        }
                    }
                }
                v => {
                    // One A1 formula over several cells is written as Excel
                    // does: relative references move with each cell.
                    let shifted = match (v, what) {
                        (Value::String(f), Write::Value | Write::Formula)
                            if f.starts_with('=')
                                && (area.top != area.bottom || area.left != area.right) =>
                        {
                            Some(Value::String(addr::a1_to_r1c1(f, area.top, area.left)))
                        }
                        _ => None,
                    };
                    let (v, what) = match &shifted {
                        Some(r1c1) => (r1c1, Write::FormulaR1C1),
                        None => (v, what),
                    };
                    for r in area.top..=area.bottom {
                        for c in area.left..=area.right {
                            write_cell(&mut book, idx, r, c, v, what)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn address(&self, args: &[Arg]) -> Result<Value, ScriptError> {
        let abs_row = arg_bool(args, 0, "RowAbsolute")?.unwrap_or(true);
        let abs_col = arg_bool(args, 1, "ColumnAbsolute")?.unwrap_or(true);
        let style = arg_i64(args, 2, "ReferenceStyle")?.unwrap_or(XL_A1);
        let external = arg_bool(args, 3, "External")?.unwrap_or(false);
        let relative = match arg(args, 4, "RelativeTo") {
            Some(v) => range_of(v).map(|r| (r.first().top, r.first().left)),
            None => None,
        };
        let parts: Vec<String> = self
            .areas
            .iter()
            .map(|a| {
                if style == XL_R1C1 {
                    let (r, c) = relative.unwrap_or((0, 0));
                    addr::area_text_r1c1(
                        *a,
                        abs_row || relative.is_none(),
                        abs_col || relative.is_none(),
                        r,
                        c,
                    )
                } else {
                    addr::area_text(*a, abs_row, abs_col)
                }
            })
            .collect();
        let mut text = parts.join(",");
        if external {
            let idx = self.idx()?;
            let book = self.s.read()?;
            let name = book.sheet_names()[idx as usize].clone();
            let file = self.s.file_name.borrow().clone();
            let sheet = format!("[{file}]{name}");
            text = format!("{}!{text}", addr::sheet_ref(&sheet));
        }
        Ok(Value::String(text))
    }

    fn select(&self) -> Result<(), ScriptError> {
        let a = self.first();
        self.s.active.set(self.sheet);
        self.s.set_selection(
            self.sheet,
            Selection {
                range: a,
                active: (a.top, a.left),
            },
        );
        Ok(())
    }

    fn activate(&self) -> Result<(), ScriptError> {
        let a = self.first();
        let sel = self.s.selection(self.sheet);
        self.s.active.set(self.sheet);
        if self.s.active.get() == self.sheet && sel.range.contains(a.top, a.left) && a.is_single() {
            self.s.set_selection(
                self.sheet,
                Selection {
                    range: sel.range,
                    active: (a.top, a.left),
                },
            );
            return Ok(());
        }
        self.select()
    }

    /// `End(xlUp)` and the other directions, as Ctrl+arrow.
    fn end(&self, dir: i64) -> Result<Value, ScriptError> {
        let idx = self.idx()?;
        let book = self.s.read()?;
        let a = self.first();
        let (mut r, mut c) = (a.top, a.left);
        let filled = |r: u32, c: u32| book.is_filled(idx, r, c);
        let (dr, dc): (i64, i64) = match dir {
            XL_UP => (-1, 0),
            XL_DOWN => (1, 0),
            XL_TO_LEFT => (0, -1),
            XL_TO_RIGHT => (0, 1),
            _ => return Err(err1004("End: unknown direction")),
        };
        let step = |r: u32, c: u32| -> Option<(u32, u32)> {
            let nr = i64::from(r) + dr;
            let nc = i64::from(c) + dc;
            (nr >= 0 && nc >= 0 && nr < i64::from(MAX_ROWS) && nc < i64::from(MAX_COLS))
                .then_some((nr as u32, nc as u32))
        };
        let (rows, cols) = book.used_size(idx);
        let past_used = |r: u32, c: u32| (dr > 0 && r >= rows) || (dc > 0 && c >= cols);
        match step(r, c) {
            None => {}
            Some((nr, nc)) if filled(r, c) && filled(nr, nc) => {
                // To the last filled cell of this run.
                (r, c) = (nr, nc);
                while let Some((nr, nc)) = step(r, c) {
                    if !filled(nr, nc) {
                        break;
                    }
                    (r, c) = (nr, nc);
                }
            }
            Some(_) => {
                // To the next filled cell, or the sheet's edge.
                while let Some((nr, nc)) = step(r, c) {
                    (r, c) = (nr, nc);
                    if filled(r, c) {
                        break;
                    }
                    if past_used(r, c) {
                        // Nothing further: the edge.
                        if dr > 0 {
                            r = MAX_ROWS - 1;
                        }
                        if dc > 0 {
                            c = MAX_COLS - 1;
                        }
                        break;
                    }
                }
            }
        }
        drop(book);
        Ok(self.at(CellRange::from_corners((r, c), (r, c))))
    }

    fn special_cells(&self, kind: i64, value: Option<i64>) -> Result<Value, ScriptError> {
        let idx = self.idx()?;
        let mut book = self.s.read()?;
        let (rows, cols) = book.used_size(idx);
        if kind == XL_CELL_TYPE_LAST_CELL {
            let r = rows.max(1) - 1;
            let c = cols.max(1) - 1;
            drop(book);
            return Ok(self.at(CellRange::from_corners((r, c), (r, c))));
        }
        // A single cell means the used range, as in Excel.
        let areas: Vec<CellRange> = if self.areas.len() == 1 && self.first().is_single() {
            vec![CellRange::from_corners(
                (0, 0),
                (rows.max(1) - 1, cols.max(1) - 1),
            )]
        } else {
            self.areas.clone()
        };
        let mut found: Vec<CellRange> = Vec::new();
        for a in areas {
            let Some(a) = a.clip(rows.max(1), cols.max(1)) else {
                continue;
            };
            for r in a.top..=a.bottom {
                let mut run: Option<(u32, u32)> = None;
                for c in a.left..=a.right {
                    let formula = book.cell_formula(idx, r, c).is_some();
                    let filled = book.is_filled(idx, r, c);
                    let mut hit = match kind {
                        XL_CELL_TYPE_CONSTANTS => filled && !formula,
                        XL_CELL_TYPE_FORMULAS => formula,
                        XL_CELL_TYPE_BLANKS => !filled,
                        XL_CELL_TYPE_VISIBLE => {
                            !book.is_row_hidden(idx, r) && !book.is_column_hidden(idx, c)
                        }
                        _ => return Err(err1004("SpecialCells: this cell type is not supported")),
                    };
                    if hit && let Some(mask) = value.filter(|_| kind != XL_CELL_TYPE_BLANKS) {
                        let v = book.cell_value(idx, r, c);
                        let bit = match v {
                            KantorValue::Number(_) => 1,
                            KantorValue::Text(_) | KantorValue::Empty => 2,
                            KantorValue::Bool(_) => 4,
                            KantorValue::Error(_) => 16,
                            KantorValue::Array(_) => 0,
                        };
                        hit = mask & bit != 0;
                    }
                    match (hit, run) {
                        (true, None) => run = Some((c, c)),
                        (true, Some((s, _))) => run = Some((s, c)),
                        (false, Some((s, e))) => {
                            found.push(CellRange::from_corners((r, s), (r, e)));
                            run = None;
                        }
                        (false, None) => {}
                    }
                }
                if let Some((s, e)) = run {
                    found.push(CellRange::from_corners((r, s), (r, e)));
                }
            }
        }
        if found.is_empty() {
            return Err(err1004("No cells were found."));
        }
        // Rows of the same columns one under the other make one area.
        let mut merged: Vec<CellRange> = Vec::new();
        for f in found {
            if let Some(last) = merged.last_mut()
                && last.left == f.left
                && last.right == f.right
                && last.bottom + 1 == f.top
            {
                last.bottom = f.bottom;
                continue;
            }
            merged.push(f);
        }
        drop(book);
        Ok(obj(RangeObj::multi(&self.s, self.sheet, merged)))
    }

    fn find(&self, args: &[Arg], next: Option<bool>) -> Result<Value, ScriptError> {
        let state = match next {
            None => {
                let what = need(args, 0, "What")?;
                let what = match scalar(what)? {
                    Value::Date(d) => kantor_script::vba::convert::date_to_string(d),
                    v => to_str(&v)?,
                };
                let look_in = arg_i64(args, 2, "LookIn")?.unwrap_or(XL_FORMULAS);
                let look_at = arg_i64(args, 3, "LookAt")?.unwrap_or(XL_PART);
                let order = arg_i64(args, 4, "SearchOrder")?.unwrap_or(XL_BY_ROWS);
                let st = FindState {
                    what,
                    formulas: look_in == XL_FORMULAS,
                    whole: look_at == XL_WHOLE,
                    match_case: arg_bool(args, 6, "MatchCase")?.unwrap_or(false),
                    by_columns: order == XL_BY_COLUMNS,
                };
                *self.s.find.borrow_mut() = Some(st.clone());
                st
            }
            Some(_) => self
                .s
                .find
                .borrow()
                .clone()
                .ok_or_else(|| err1004("FindNext needs a Find first"))?,
        };
        let backwards = arg_i64(args, 5, "SearchDirection")?.unwrap_or(XL_NEXT) == XL_PREVIOUS
            || next == Some(true);
        let after_arg = match next {
            None => arg(args, 1, "After"),
            Some(_) => arg(args, 0, "After"),
        };
        let after = after_arg.and_then(range_of).map(|r| {
            let a = r.first();
            (a.top, a.left)
        });
        let idx = self.idx()?;
        let book = self.s.read()?;
        let (rows, cols) = book.used_size(idx);
        let mut area = self.first();
        if area.is_single() && self.areas.len() == 1 {
            area = CellRange::from_corners((0, 0), (MAX_ROWS - 1, MAX_COLS - 1));
        }
        let Some(area) = area.clip(rows.max(1), cols.max(1)) else {
            return Ok(Value::NOTHING);
        };
        let (h, w) = (area.rows_count() as u64, area.cols_count() as u64);
        let total = h * w;
        let pos_of = |r: u32, c: u32| -> u64 {
            let (i, j) = (u64::from(r - area.top), u64::from(c - area.left));
            if state.by_columns {
                j * h + i
            } else {
                i * w + j
            }
        };
        let cell_of = |k: u64| -> (u32, u32) {
            let (i, j) = if state.by_columns {
                (k % h, k / h)
            } else {
                (k / w, k % w)
            };
            (area.top + i as u32, area.left + j as u32)
        };
        // The search starts after `After` (default: the area's first cell)
        // and wraps around.
        let start = match after {
            Some((r, c)) if area.contains(r, c) => pos_of(r, c),
            _ if backwards => 0,
            _ => total - 1,
        };
        let needle = if state.match_case {
            state.what.clone()
        } else {
            state.what.to_lowercase()
        };
        for n in 1..=total {
            let k = if backwards {
                (start + total - n % total) % total
            } else {
                (start + n) % total
            };
            let (r, c) = cell_of(k);
            if !book.is_filled(idx, r, c) {
                continue;
            }
            let text = if state.formulas {
                book.input(idx, r, c)
            } else {
                book.display_value(idx, r, c)
            };
            let text = if state.match_case {
                text
            } else {
                text.to_lowercase()
            };
            let hit = if state.whole {
                text == needle
            } else {
                text.contains(&needle)
            };
            if hit {
                drop(book);
                return Ok(self.at(CellRange::from_corners((r, c), (r, c))));
            }
        }
        Ok(Value::NOTHING)
    }

    fn replace(&self, args: &[Arg]) -> Result<Value, ScriptError> {
        let what = to_str(need(args, 0, "What")?)?;
        let with = to_str(need(args, 1, "Replacement")?)?;
        let whole = arg_i64(args, 2, "LookAt")?.unwrap_or(XL_PART) == XL_WHOLE;
        let match_case = arg_bool(args, 4, "MatchCase")?.unwrap_or(false);
        if what.is_empty() {
            return Ok(Value::Boolean(false));
        }
        let idx = self.idx()?;
        let mut book = self.s.write()?;
        let (rows, cols) = book.used_size(idx);
        let areas: Vec<CellRange> = if self.areas.len() == 1 && self.first().is_single() {
            vec![CellRange::all()]
        } else {
            self.areas.clone()
        };
        for a in areas {
            let Some(a) = a.clip(rows.max(1), cols.max(1)) else {
                continue;
            };
            for r in a.top..=a.bottom {
                for c in a.left..=a.right {
                    if !book.is_filled(idx, r, c) {
                        continue;
                    }
                    let input = book.input(idx, r, c);
                    let new = replace_text(&input, &what, &with, whole, match_case);
                    if new != input {
                        book.set_input(idx, r, c, &new).map_err(engine)?;
                    }
                }
            }
        }
        Ok(Value::Boolean(true))
    }

    /// `Range.Sort Key1, Order1, Key2, Type, Order2, Key3, Order3, Header…`.
    fn sort(&self, args: &[Arg]) -> Result<(), ScriptError> {
        let idx = self.idx()?;
        let mut range = self.first();
        if range.is_single() {
            let book = self.s.read()?;
            range = book.current_region(idx, range.top, range.left);
        }
        let mut keys = Vec::new();
        for (key, order, kpos, opos) in [
            ("Key1", "Order1", 0, 1),
            ("Key2", "Order2", 2, 4),
            ("Key3", "Order3", 5, 6),
        ] {
            let Some(k) = arg(args, kpos, key) else {
                continue;
            };
            let col = key_column(&self.s, self.sheet, k)?;
            let desc = arg_i64(args, opos, order)?.unwrap_or(XL_ASCENDING) == XL_DESCENDING;
            keys.push((col, desc));
        }
        let header = arg_i64(args, 7, "Header")?.unwrap_or(XL_NO);
        sort_range(&self.s, self.sheet, range, &keys, header)
    }

    /// `AutoFilter [Field, Criteria1, Operator, Criteria2]`.
    fn autofilter(&self, args: &[Arg]) -> Result<Value, ScriptError> {
        if let Some(r) = crate::tables::range_autofilter(self, args) {
            return r;
        }
        let idx = self.idx()?;
        let mut range = self.first();
        let field = arg_i64(args, 0, "Field")?;
        let mut book = self.s.write()?;
        let existing = book
            .filters(idx)
            .into_iter()
            .find(|f| f.scope == sheets_engine::FilterScope::Sheet);
        let Some(field) = field else {
            // No arguments: turn the filter on or off.
            if range.is_single() {
                range = book.current_region(idx, range.top, range.left);
            }
            book.toggle_filter(idx, range).map_err(engine)?;
            return Ok(Value::Boolean(true));
        };
        let info = match existing {
            Some(f) => f,
            None => {
                if range.is_single() {
                    range = book.current_region(idx, range.top, range.left);
                }
                book.toggle_filter(idx, range).map_err(engine)?;
                book.filters(idx)
                    .into_iter()
                    .find(|f| f.scope == sheets_engine::FilterScope::Sheet)
                    .ok_or_else(|| err1004("AutoFilter method of Range class failed"))?
            }
        };
        let col = info.area.left as i64 + field - 1;
        if field < 1 || col > i64::from(info.area.right) {
            return Err(err1004("AutoFilter method of Range class failed"));
        }
        let c1 = arg(args, 1, "Criteria1").map(scalar).transpose()?;
        let op = arg_i64(args, 2, "Operator")?.unwrap_or(XL_AND);
        let c2 = arg(args, 3, "Criteria2").map(scalar).transpose()?;
        let kind = filter_kind(c1, op, c2)?;
        book.set_column_filter(idx, &sheets_engine::FilterScope::Sheet, col as u32, kind)
            .map_err(engine)?;
        Ok(Value::Boolean(true))
    }

    fn remove_duplicates(&self, args: &[Arg]) -> Result<(), ScriptError> {
        let a = self.first();
        let columns: Vec<u32> = match arg(args, 0, "Columns") {
            None => (0..a.cols_count()).collect(),
            Some(v) => match scalar(v)? {
                Value::Array(arr) => arr
                    .values()
                    .iter()
                    .map(|x| to_i64(x).map(|n| (n.max(1) - 1) as u32))
                    .collect::<Result<_, _>>()?,
                v => vec![(to_i64(&v)?.max(1) - 1) as u32],
            },
        };
        let header = arg_i64(args, 1, "Header")?.unwrap_or(XL_NO) == XL_YES;
        crate::fit::remove_duplicates(self, &columns, header)
    }

    fn clear(&self, what: Clear) -> Result<(), ScriptError> {
        let idx = self.idx()?;
        let mut book = self.s.write()?;
        for &a in &self.areas {
            let a = if a.is_whole_columns() || a.is_whole_rows() {
                match self.used(&book, idx, a) {
                    Some(u) => u,
                    None => continue,
                }
            } else {
                a
            };
            match what {
                Clear::Contents => book.clear_range(idx, a),
                Clear::Formats => book.clear_formats(idx, a),
                Clear::All => book.clear_all(idx, a),
            }
            .map_err(engine)?;
        }
        Ok(())
    }

    fn delete(&self, shift: Option<i64>) -> Result<(), ScriptError> {
        let idx = self.idx()?;
        let a = self.first();
        let mut book = self.s.write()?;
        if a.is_whole_rows() {
            return book.delete_rows(idx, a.top, a.bottom).map_err(engine);
        }
        if a.is_whole_columns() {
            return book.delete_columns(idx, a.left, a.right).map_err(engine);
        }
        let (rows, cols) = book.used_size(idx);
        let up = match shift {
            Some(XL_SHIFT_UP) => true,
            Some(XL_SHIFT_TO_LEFT) => false,
            // Excel decides by the range's shape.
            _ => a.cols_count() >= a.rows_count(),
        };
        if up {
            let below = a.bottom + 1;
            if below < rows {
                let src = CellRange::from_corners((below, a.left), (rows - 1, a.right));
                book.copy_cells(idx, src, idx, a.top, a.left, true)
                    .map_err(engine)?;
            } else {
                book.clear_all(idx, a).map_err(engine)?;
            }
        } else {
            let right = a.right + 1;
            if right < cols {
                let src = CellRange::from_corners((a.top, right), (a.bottom, cols - 1));
                book.copy_cells(idx, src, idx, a.top, a.left, true)
                    .map_err(engine)?;
            } else {
                book.clear_all(idx, a).map_err(engine)?;
            }
        }
        Ok(())
    }

    fn insert(&self, shift: Option<i64>) -> Result<(), ScriptError> {
        let idx = self.idx()?;
        let a = self.first();
        let mut book = self.s.write()?;
        if a.is_whole_rows() {
            return book.insert_rows(idx, a.top, a.rows_count()).map_err(engine);
        }
        if a.is_whole_columns() {
            return book
                .insert_columns(idx, a.left, a.cols_count())
                .map_err(engine);
        }
        let (rows, cols) = book.used_size(idx);
        let down = match shift {
            Some(XL_SHIFT_DOWN) => true,
            Some(XL_SHIFT_TO_RIGHT) => false,
            _ => a.cols_count() >= a.rows_count(),
        };
        if down {
            if a.top < rows {
                let h = a.rows_count();
                if rows - 1 + h >= MAX_ROWS {
                    return Err(err1004("Insert method of Range class failed"));
                }
                let src = CellRange::from_corners((a.top, a.left), (rows - 1, a.right));
                book.copy_cells(idx, src, idx, a.top + h, a.left, true)
                    .map_err(engine)?;
            }
        } else if a.left < cols {
            let w = a.cols_count();
            if cols - 1 + w >= MAX_COLS {
                return Err(err1004("Insert method of Range class failed"));
            }
            let src = CellRange::from_corners((a.top, a.left), (a.bottom, cols - 1));
            book.copy_cells(idx, src, idx, a.top, a.left + w, true)
                .map_err(engine)?;
        }
        Ok(())
    }

    /// `Copy [Destination]` / `Cut [Destination]`.
    fn copy(&self, args: &[Arg], cut: bool) -> Result<Value, ScriptError> {
        let src = self.first();
        match arg(args, 0, "Destination") {
            Some(d) => {
                let dest = range_of(d).ok_or_else(|| ScriptError::runtime(424))?;
                let didx = dest.idx()?;
                let idx = self.idx()?;
                let d = dest.first();
                let mut book = self.s.write()?;
                book.copy_cells(idx, src, didx, d.top, d.left, cut)
                    .map_err(engine)?;
                self.s.clip.set(None);
            }
            None => {
                self.s.clip.set(Some(crate::session::Clip {
                    sheet_id: self.sheet,
                    range: src,
                    cut,
                }));
            }
        }
        Ok(Value::Boolean(true))
    }

    /// `PasteSpecial [Paste], [Operation], [SkipBlanks], [Transpose]`.
    fn paste_special(&self, args: &[Arg]) -> Result<Value, ScriptError> {
        let what = arg_i64(args, 0, "Paste")?.unwrap_or(XL_PASTE_ALL);
        let clip = self
            .s
            .clip
            .get()
            .ok_or_else(|| err1004("PasteSpecial method of Range class failed"))?;
        paste(&self.s, clip, self.sheet, self.first(), what)?;
        Ok(Value::Boolean(true))
    }

    fn merge(&self, across: bool) -> Result<(), ScriptError> {
        let idx = self.idx()?;
        let a = self.first();
        let mut book = self.s.write()?;
        if across {
            for r in a.top..=a.bottom {
                book.merge_range(idx, CellRange::from_corners((r, a.left), (r, a.right)))
                    .map_err(engine)?;
            }
            return Ok(());
        }
        book.merge_range(idx, a).map_err(engine)
    }

    fn style_value(&self, path: &str, value: &str) -> Result<(), ScriptError> {
        set_style(&self.s, self.sheet, &self.areas, path, value)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Read {
    Value,
    Value2,
    Formula,
    FormulaR1C1,
    Text,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Write {
    Value,
    Formula,
    FormulaR1C1,
}

#[derive(Clone, Copy)]
enum Clear {
    Contents,
    Formats,
    All,
}

fn read_cell(book: &mut Workbook, idx: u32, r: u32, c: u32, what: Read) -> Value {
    match what {
        Read::Value | Read::Value2 => {
            let v = book.cell_value(idx, r, c);
            let date = what == Read::Value
                && matches!(v, KantorValue::Number(_))
                && book.is_date_format(idx, r, c);
            from_cell(v, date)
        }
        Read::Formula => Value::String(book.input(idx, r, c)),
        Read::FormulaR1C1 => {
            let input = book.input(idx, r, c);
            Value::String(if input.starts_with('=') {
                addr::a1_to_r1c1(&input, r, c)
            } else {
                input
            })
        }
        Read::Text => Value::String(book.display_value(idx, r, c)),
    }
}

fn write_cell(
    book: &mut RefMut<'_, Workbook>,
    idx: u32,
    r: u32,
    c: u32,
    v: &Value,
    what: Write,
) -> Result<(), ScriptError> {
    let (input, date_fmt) = cell_input(v)?;
    let cell = CellRange::from_corners((r, c), (r, c));
    let Some(mut input) = input else {
        if book.is_filled(idx, r, c) {
            book.clear_range(idx, cell).map_err(engine)?;
        }
        return Ok(());
    };
    if what == Write::FormulaR1C1 && input.starts_with('=') {
        input = addr::r1c1_to_a1(&input, r, c);
    }
    book.set_input(idx, r, c, &input).map_err(engine)?;
    if let Some(fmt) = date_fmt {
        let style = book.cell_style(idx, r, c);
        if style.num_fmt.eq_ignore_ascii_case("general") {
            book.set_range_style_value(idx, cell, "num_fmt", fmt)
                .map_err(engine)?;
        }
    }
    Ok(())
}

fn replace_text(text: &str, what: &str, with: &str, whole: bool, match_case: bool) -> String {
    if whole {
        let same = if match_case {
            text == what
        } else {
            text.to_lowercase() == what.to_lowercase()
        };
        return if same {
            with.to_owned()
        } else {
            text.to_owned()
        };
    }
    if match_case {
        return text.replace(what, with);
    }
    // Case-insensitive, keeping the rest of the text as it is.
    let lower = text.to_lowercase();
    let needle = what.to_lowercase();
    if lower.len() != text.len() {
        return text.replace(what, with);
    }
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(k) = lower[i..].find(&needle) {
        out.push_str(&text[i..i + k]);
        out.push_str(with);
        i += k + needle.len();
    }
    out.push_str(&text[i..]);
    out
}

/// The `Range` inside an argument (`None` for anything else).
pub fn range_of(v: &Value) -> Option<Rc<RangeObj>> {
    let o = v.as_object()?;
    let any = o.as_any()?;
    any.downcast_ref::<RangeObj>().map(|r| {
        Rc::new(RangeObj {
            s: r.s.clone(),
            sheet: r.sheet,
            areas: r.areas.clone(),
            mode: r.mode,
        })
    })
}

/// A sort key's absolute column: a `Range` or an address on `sheet`.
pub fn key_column(s: &Rc<Session>, _sheet: u32, k: &Value) -> Result<u32, ScriptError> {
    if let Some(r) = range_of(k) {
        return Ok(r.first().left);
    }
    let text = to_str(k)?;
    let (_, cells) = addr::split_sheet(&text);
    let _ = s;
    addr::parse_area(cells)
        .map(|r| r.left)
        .ok_or_else(|| err1004("The sort reference is not valid."))
}

/// Sort `range` by `(column, descending)` keys.
pub fn sort_range(
    s: &Rc<Session>,
    sheet: u32,
    range: CellRange,
    keys: &[(u32, bool)],
    header: i64,
) -> Result<(), ScriptError> {
    let idx = s.sheet_index(sheet)?;
    let mut book = s.read()?;
    let header = match header {
        XL_YES => true,
        XL_GUESS => book.guess_header(idx, range),
        _ => false,
    };
    let keys: Vec<(u32, bool)> = if keys.is_empty() {
        vec![(range.left, false)]
    } else {
        keys.to_vec()
    };
    let levels: Vec<sheets_engine::SortLevel> = keys
        .iter()
        .map(|&(col, descending)| sheets_engine::SortLevel {
            col,
            descending,
            on: sheets_engine::SortOn::Values,
        })
        .collect();
    if levels
        .iter()
        .any(|l| l.col < range.left || l.col > range.right)
    {
        return Err(err1004(
            "The sort reference is not valid. Make sure that it's within the data you want to sort.",
        ));
    }
    book.sort_range_by(idx, range, &levels, header)
        .map_err(engine)?;
    drop(book);
    // Formulas that read the sorted cells are recalculated before the next
    // read.
    s.write()?;
    Ok(())
}

pub(crate) fn filter_kind(
    c1: Option<Value>,
    op: i64,
    c2: Option<Value>,
) -> Result<Option<sheets_engine::FilterKind>, ScriptError> {
    use sheets_engine::{Condition, FilterKind, FilterOp};
    let Some(c1) = c1 else {
        return Ok(None);
    };
    if op == XL_FILTER_VALUES {
        let values: Vec<String> = match &c1 {
            Value::Array(a) => a.values().iter().map(to_str).collect::<Result<_, _>>()?,
            v => vec![to_str(v)?],
        };
        let blanks = values.iter().any(|v| v.is_empty() || v == "=");
        return Ok(Some(FilterKind::Values {
            values: values
                .into_iter()
                .filter(|v| !v.is_empty() && v != "=")
                .collect(),
            blanks,
            dates: Vec::new(),
        }));
    }
    if op == XL_TOP10_ITEMS
        || op == XL_BOTTOM10_ITEMS
        || op == XL_TOP10_PERCENT
        || op == XL_BOTTOM10_PERCENT
    {
        return Ok(Some(FilterKind::Top10 {
            top: op == XL_TOP10_ITEMS || op == XL_TOP10_PERCENT,
            percent: op == XL_TOP10_PERCENT || op == XL_BOTTOM10_PERCENT,
            value: to_f64(&c1)?,
        }));
    }
    let cond = |v: &Value| -> Result<Condition, ScriptError> {
        let text = to_str(v)?;
        let (op, rest) = ["<>", ">=", "<=", "=", ">", "<"]
            .iter()
            .find_map(|p| text.strip_prefix(p).map(|r| (*p, r)))
            .unwrap_or(("=", text.as_str()));
        Ok(Condition {
            op: match op {
                "<>" => FilterOp::NotEqual,
                ">=" => FilterOp::GreaterThanOrEqual,
                "<=" => FilterOp::LessThanOrEqual,
                ">" => FilterOp::GreaterThan,
                "<" => FilterOp::LessThan,
                _ => FilterOp::Equal,
            },
            value: rest.to_owned(),
        })
    };
    let mut conditions = vec![cond(&c1)?];
    if let Some(c2) = &c2 {
        conditions.push(cond(c2)?);
    }
    // One equality condition is a value list in Excel's terms.
    if conditions.len() == 1
        && conditions[0].op == FilterOp::Equal
        && !conditions[0].value.contains(['*', '?'])
    {
        let v = conditions.remove(0).value;
        return Ok(Some(FilterKind::Values {
            blanks: v.is_empty(),
            values: if v.is_empty() { Vec::new() } else { vec![v] },
            dates: Vec::new(),
        }));
    }
    Ok(Some(FilterKind::Custom {
        and: op != XL_OR,
        conditions,
    }))
}

/// Paste `clip` onto `sheet` at `target` (its top-left; a bigger target
/// takes formats repeated, as Excel).
pub fn paste(
    s: &Rc<Session>,
    clip: crate::session::Clip,
    sheet: u32,
    target: CellRange,
    what: i64,
) -> Result<(), ScriptError> {
    let src_idx = s.sheet_index(clip.sheet_id)?;
    let idx = s.sheet_index(sheet)?;
    let src = clip.range;
    let dst = CellRange {
        top: target.top,
        left: target.left,
        bottom: (target.top + src.rows_count() - 1).min(MAX_ROWS - 1),
        right: (target.left + src.cols_count() - 1).min(MAX_COLS - 1),
    };
    match what {
        XL_PASTE_ALL | XL_PASTE_ALL_EXCEPT_BORDERS | XL_PASTE_ALL_USING_SOURCE_THEME => {
            let mut book = s.write()?;
            book.copy_cells(src_idx, src, idx, target.top, target.left, clip.cut)
                .map_err(engine)?;
            if clip.cut {
                s.clip.set(None);
            }
        }
        XL_PASTE_VALUES
        | XL_PASTE_VALUES_AND_NUMBER_FORMATS
        | XL_PASTE_FORMULAS
        | XL_PASTE_FORMULAS_AND_NUMBER_FORMATS => {
            let mut book = s.read()?;
            let formulas = matches!(
                what,
                XL_PASTE_FORMULAS | XL_PASTE_FORMULAS_AND_NUMBER_FORMATS
            );
            let cells: Vec<Vec<Value>> = (src.top..=src.bottom)
                .map(|r| {
                    (src.left..=src.right)
                        .map(|c| {
                            if formulas {
                                Value::String(book.input(src_idx, r, c))
                            } else {
                                from_cell(book.cell_value(src_idx, r, c), false)
                            }
                        })
                        .collect()
                })
                .collect();
            drop(book);
            let mut book = s.write()?;
            for (i, row) in cells.iter().enumerate() {
                for (j, v) in row.iter().enumerate() {
                    let (r, c) = (dst.top + i as u32, dst.left + j as u32);
                    if r > dst.bottom || c > dst.right {
                        continue;
                    }
                    // Values paste as constants, formulas as typed (their
                    // references aren't moved).
                    let v = match (formulas, v) {
                        (false, Value::String(t)) if t.starts_with('=') => {
                            Value::String(format!("'{t}"))
                        }
                        _ => v.clone(),
                    };
                    write_cell(&mut book, idx, r, c, &v, Write::Value)?;
                }
            }
            if matches!(
                what,
                XL_PASTE_VALUES_AND_NUMBER_FORMATS | XL_PASTE_FORMULAS_AND_NUMBER_FORMATS
            ) {
                for (i, r) in (src.top..=src.bottom).enumerate() {
                    for (j, c) in (src.left..=src.right).enumerate() {
                        let fmt = book.cell_style(src_idx, r, c).num_fmt.clone();
                        let cell = CellRange::from_corners(
                            (dst.top + i as u32, dst.left + j as u32),
                            (dst.top + i as u32, dst.left + j as u32),
                        );
                        book.set_range_style_value(idx, cell, "num_fmt", &fmt)
                            .map_err(engine)?;
                    }
                }
            }
        }
        XL_PASTE_FORMATS => {
            let target = if target.is_single() { dst } else { target };
            let mut book = s.write()?;
            book.paste_formats(src_idx, src, idx, target)
                .map_err(engine)?;
        }
        XL_PASTE_COLUMN_WIDTHS => {
            let mut book = s.write()?;
            for (j, c) in (src.left..=src.right).enumerate() {
                let w = book.column_width(src_idx, c);
                let to = dst.left + j as u32;
                if to < MAX_COLS {
                    book.set_column_width(idx, to, to, w).map_err(engine)?;
                }
            }
        }
        _ => return Err(ScriptError::unsupported("This PasteSpecial option")),
    }
    Ok(())
}

/// Set style `path` to `value` on `areas` of sheet `sheet` (whole rows or
/// columns: their used part).
pub fn set_style(
    s: &Rc<Session>,
    sheet: u32,
    areas: &[CellRange],
    path: &str,
    value: &str,
) -> Result<(), ScriptError> {
    let idx = s.sheet_index(sheet)?;
    let mut book = s.write()?;
    let (rows, cols) = book.used_size(idx);
    for &a in areas {
        let a = if a.is_whole_columns() || a.is_whole_rows() {
            match a.clip(rows.max(1), cols.max(1)) {
                Some(u) => u,
                None => continue,
            }
        } else {
            a
        };
        book.set_range_style_value(idx, a, path, value)
            .map_err(engine)?;
    }
    Ok(())
}

fn halign_value(n: i64) -> Option<&'static str> {
    Some(match n {
        XL_GENERAL => "general",
        XL_LEFT => "left",
        XL_CENTER => "center",
        XL_RIGHT => "right",
        XL_JUSTIFY => "justify",
        XL_FILL => "fill",
        XL_CENTER_ACROSS_SELECTION => "centerContinuous",
        XL_DISTRIBUTED => "distributed",
        _ => return None,
    })
}

fn valign_value(n: i64) -> Option<&'static str> {
    Some(match n {
        XL_TOP => "top",
        XL_CENTER => "center",
        XL_BOTTOM => "bottom",
        XL_JUSTIFY => "justify",
        XL_DISTRIBUTED => "distributed",
        _ => return None,
    })
}

impl Dispatch for RangeObj {
    fn type_name(&self) -> String {
        "Range".into()
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "" | "item" if !args.is_empty() => self.item(args),
            "cells" if !args.is_empty() => self.with_mode(Mode::Cells).item(args),
            "" | "value" => self.read(Read::Value),
            "value2" => self.read(Read::Value2),
            "formula" | "formulalocal" | "formula2" | "formulaarray" => self.read(Read::Formula),
            "formular1c1" | "formular1c1local" | "formula2r1c1" => self.read(Read::FormulaR1C1),
            "text" => self.read(Read::Text),
            "cells" => Ok(obj(self.with_mode(Mode::Cells))),
            "rows" if !args.is_empty() => self.with_mode(Mode::Rows).item(args),
            "columns" if !args.is_empty() => self.with_mode(Mode::Columns).item(args),
            "rows" => Ok(obj(self.with_mode(Mode::Rows))),
            "columns" => Ok(obj(self.with_mode(Mode::Columns))),
            "count" | "countlarge" => Ok(match self.count() {
                n if n <= i64::from(i32::MAX) => Value::Long(n as i32),
                n => Value::Double(n as f64),
            }),
            "row" => Ok(Value::Long(self.first().top as i32 + 1)),
            "column" => Ok(Value::Long(self.first().left as i32 + 1)),
            "address" | "addresslocal" => self.address(args),
            "offset" => {
                let a = self.first();
                let dr = arg_i64(args, 0, "RowOffset")?.unwrap_or(0);
                let dc = arg_i64(args, 1, "ColumnOffset")?.unwrap_or(0);
                let top = i64::from(a.top) + dr;
                let left = i64::from(a.left) + dc;
                let bottom = i64::from(a.bottom) + dr;
                let right = i64::from(a.right) + dc;
                if top < 0
                    || left < 0
                    || bottom >= i64::from(MAX_ROWS)
                    || right >= i64::from(MAX_COLS)
                {
                    return Err(err1004("Application-defined or object-defined error"));
                }
                Ok(self.at(CellRange {
                    top: top as u32,
                    left: left as u32,
                    bottom: bottom as u32,
                    right: right as u32,
                }))
            }
            "resize" => {
                let a = self.first();
                let rows = arg_i64(args, 0, "RowSize")?.unwrap_or(i64::from(a.rows_count()));
                let cols = arg_i64(args, 1, "ColumnSize")?.unwrap_or(i64::from(a.cols_count()));
                if rows < 1 || cols < 1 {
                    return Err(err1004("Application-defined or object-defined error"));
                }
                let bottom = i64::from(a.top) + rows - 1;
                let right = i64::from(a.left) + cols - 1;
                if bottom >= i64::from(MAX_ROWS) || right >= i64::from(MAX_COLS) {
                    return Err(err1004("Application-defined or object-defined error"));
                }
                Ok(self.at(CellRange {
                    top: a.top,
                    left: a.left,
                    bottom: bottom as u32,
                    right: right as u32,
                }))
            }
            "end" => self.end(to_i64(need(args, 0, "Direction")?)?),
            "currentregion" => {
                let a = self.first();
                let idx = self.idx()?;
                let r = self.s.read()?.current_region(idx, a.top, a.left);
                Ok(self.at(r))
            }
            "entirerow" => {
                let a = self.first();
                Ok(obj(Rc::new(RangeObj {
                    s: self.s.clone(),
                    sheet: self.sheet,
                    areas: vec![CellRange::rows(a.top, a.bottom)],
                    mode: Mode::Rows,
                })))
            }
            "entirecolumn" => {
                let a = self.first();
                Ok(obj(Rc::new(RangeObj {
                    s: self.s.clone(),
                    sheet: self.sheet,
                    areas: vec![CellRange::columns(a.left, a.right)],
                    mode: Mode::Columns,
                })))
            }
            "areas" => match arg_i64(args, 0, "Index")? {
                Some(i) => {
                    let a = usize::try_from(i - 1)
                        .ok()
                        .and_then(|i| self.areas.get(i))
                        .ok_or_else(|| ScriptError::runtime(9))?;
                    Ok(self.at(*a))
                }
                None => Ok(obj(Rc::new(AreasObj {
                    range: RangeObj::multi(&self.s, self.sheet, self.areas.clone()),
                }))),
            },
            "range" => {
                let a = need(args, 0, "Cell1")?;
                if let Some(b) = arg(args, 1, "Cell2") {
                    let (x, y) = (range_of(a), range_of(b));
                    if let (Some(x), Some(y)) = (x, y) {
                        let (p, q) = (x.first(), y.first());
                        return Ok(self.at(CellRange {
                            top: p.top.min(q.top),
                            left: p.left.min(q.left),
                            bottom: p.bottom.max(q.bottom),
                            right: p.right.max(q.right),
                        }));
                    }
                }
                self.relative_range(&to_str(a)?)
            }
            "worksheet" | "parent" => Ok(obj(Rc::new(SheetObj {
                s: self.s.clone(),
                id: self.sheet,
            }))),
            "application" => Ok(obj(Rc::new(crate::app::Application::new(&self.s)))),
            "hasformula" => {
                let idx = self.idx()?;
                let book = self.s.read()?;
                let (mut some, mut all) = (false, true);
                for a in &self.areas {
                    for r in a.top..=a.bottom.min(a.top + 10_000) {
                        for c in a.left..=a.right.min(a.left + 1000) {
                            if book.cell_formula(idx, r, c).is_some() {
                                some = true;
                            } else {
                                all = false;
                            }
                        }
                    }
                }
                Ok(if all {
                    Value::Boolean(true)
                } else if some {
                    Value::Null
                } else {
                    Value::Boolean(false)
                })
            }
            "font" => Ok(obj(Rc::new(FontObj {
                s: self.s.clone(),
                sheet: self.sheet,
                areas: self.areas.clone(),
            }))),
            "interior" => Ok(obj(Rc::new(InteriorObj {
                s: self.s.clone(),
                sheet: self.sheet,
                areas: self.areas.clone(),
            }))),
            "borders" => {
                let edge = arg_i64(args, 0, "Index")?;
                Ok(obj(Rc::new(BordersObj {
                    s: self.s.clone(),
                    sheet: self.sheet,
                    areas: self.areas.clone(),
                    edge,
                })))
            }
            "numberformat" | "numberformatlocal" => {
                let a = self.first();
                let idx = self.idx()?;
                let fmt = self
                    .s
                    .read()?
                    .cell_style(idx, a.top, a.left)
                    .num_fmt
                    .clone();
                Ok(Value::String(if fmt.eq_ignore_ascii_case("general") {
                    "General".into()
                } else {
                    fmt
                }))
            }
            "horizontalalignment" | "verticalalignment" | "wraptext" | "indentlevel" => {
                let a = self.first();
                let idx = self.idx()?;
                let st = self.s.read()?.cell_style(idx, a.top, a.left);
                Ok(match lower.as_str() {
                    "horizontalalignment" => Value::Long(match (st.align, st.align_detail) {
                        (_, sheets_engine::AlignDetail::CenterAcross) => {
                            XL_CENTER_ACROSS_SELECTION as i32
                        }
                        (_, sheets_engine::AlignDetail::Fill) => XL_FILL as i32,
                        (_, sheets_engine::AlignDetail::Justify) => XL_JUSTIFY as i32,
                        (_, sheets_engine::AlignDetail::Distributed) => XL_DISTRIBUTED as i32,
                        (sheets_engine::HAlign::Left, _) => XL_LEFT as i32,
                        (sheets_engine::HAlign::Center, _) => XL_CENTER as i32,
                        (sheets_engine::HAlign::Right, _) => XL_RIGHT as i32,
                        (sheets_engine::HAlign::General, _) => XL_GENERAL as i32,
                    }),
                    "verticalalignment" => Value::Long(match st.valign {
                        sheets_engine::VAlign::Top => XL_TOP as i32,
                        sheets_engine::VAlign::Center => XL_CENTER as i32,
                        sheets_engine::VAlign::Bottom => XL_BOTTOM as i32,
                    }),
                    "indentlevel" => Value::Long(st.indent as i32),
                    _ => Value::Boolean(st.wrap),
                })
            }
            "mergecells" => {
                let a = self.first();
                let idx = self.idx()?;
                Ok(Value::Boolean(
                    self.s.read()?.merge_at(idx, a.top, a.left).is_some(),
                ))
            }
            "mergearea" => {
                let a = self.first();
                let idx = self.idx()?;
                let m = self.s.read()?.merge_at(idx, a.top, a.left).unwrap_or(a);
                Ok(self.at(m))
            }
            "columnwidth" => {
                let a = self.first();
                let idx = self.idx()?;
                let book = self.s.read()?;
                let px = book.column_width(idx, a.left);
                let d = book.column_scale(idx).digit_px;
                let chars = if px <= 0.0 {
                    0.0
                } else {
                    (((px - 5.0) / d * 100.0 + 0.5).trunc() / 100.0).max(0.0)
                };
                Ok(Value::Double(chars))
            }
            "rowheight" => {
                let a = self.first();
                let idx = self.idx()?;
                let book = self.s.read()?;
                Ok(Value::Double(book.row_points(idx, a.top)))
            }
            "width" | "height" => {
                let a = self.first();
                let idx = self.idx()?;
                let book = self.s.read()?;
                Ok(Value::Double(if lower == "width" {
                    (a.left..=a.right.min(a.left + 1000))
                        .map(|c| book.column_width(idx, c) * 0.75)
                        .sum()
                } else {
                    (a.top..=a.bottom.min(a.top + 10_000))
                        .map(|r| book.row_points(idx, r))
                        .sum()
                }))
            }
            "left" | "top" => {
                let a = self.first();
                let idx = self.idx()?;
                let book = self.s.read()?;
                Ok(Value::Double(if lower == "left" {
                    (0..a.left).map(|c| book.column_width(idx, c) * 0.75).sum()
                } else {
                    (0..a.top.min(100_000))
                        .map(|r| book.row_points(idx, r))
                        .sum()
                }))
            }
            "hidden" => {
                let a = self.first();
                let idx = self.idx()?;
                let book = self.s.read()?;
                Ok(Value::Boolean(
                    if a.is_whole_columns() || self.mode == Mode::Columns {
                        (a.left..=a.right).all(|c| book.is_column_hidden(idx, c))
                    } else {
                        (a.top..=a.bottom.min(a.top + 100_000)).all(|r| book.is_row_hidden(idx, r))
                    },
                ))
            }
            "locked" => crate::protection::locked(self),
            "formulahidden" => crate::protection::formula_hidden(self),
            "pagebreak" => crate::page_setup::range_page_break(self),
            "exportasfixedformat" => crate::page_setup::export_pdf_range(self, args),
            "formatconditions" => crate::cond_formats::format_conditions(self, args),
            "characters" => crate::characters::characters(self, args),
            "validation" => Ok(crate::validation::validation(self)),
            "name" => Err(err1004("This range has no name")),
            "find" => self.find(args, None),
            "findnext" => self.find(args, Some(false)),
            "findprevious" => self.find(args, Some(true)),
            "specialcells" => {
                let kind = to_i64(need(args, 0, "Type")?)?;
                let value = arg_i64(args, 1, "Value")?;
                self.special_cells(kind, value)
            }
            "next" => {
                let a = self.first();
                Ok(self.at(self.cell_at(1, i64::from(a.cols_count()) + 1)?))
            }
            "previous" => self.at_checked(1, 0),
            "dependents" | "precedents" | "directdependents" | "directprecedents" => {
                Err(ScriptError::unsupported("Range.Dependents and Precedents"))
            }
            "pivottable" => Ok(Value::NOTHING),
            "listobject" => crate::tables::range_list_object(self),
            "hyperlinks" => {
                crate::hyperlinks::with_args(crate::hyperlinks::range_hyperlinks(self), args)
            }
            "comment" => crate::comments::comment(self),
            "addcomment" => crate::comments::add_comment(self, args),
            "notetext" => crate::comments::note_text(self, args),
            "addcommentthreaded" | "commentthreaded" => crate::comments::threaded(),
            // Methods (also as statements, see `call`).
            "select" => self.select().map(|()| Value::Boolean(true)),
            "activate" => self.activate().map(|()| Value::Boolean(true)),
            "clear" => self.clear(Clear::All).map(|()| Value::Boolean(true)),
            "clearcontents" => self.clear(Clear::Contents).map(|()| Value::Boolean(true)),
            "clearformats" => self.clear(Clear::Formats).map(|()| Value::Boolean(true)),
            "clearcomments" | "clearnotes" => crate::comments::clear_comments(self),
            "clearhyperlinks" => crate::hyperlinks::clear_hyperlinks(self),
            "clearoutline" => Ok(Value::Boolean(true)),
            "delete" => self
                .delete(arg_i64(args, 0, "Shift")?)
                .map(|()| Value::Boolean(true)),
            "insert" => self
                .insert(arg_i64(args, 0, "Shift")?)
                .map(|()| Value::Boolean(true)),
            "copy" => self.copy(args, false),
            "cut" => self.copy(args, true),
            "pastespecial" => self.paste_special(args),
            "replace" => self.replace(args),
            "sort" => self.sort(args).map(|()| Value::Long(0)),
            "autofilter" => self.autofilter(args),
            "removeduplicates" => self.remove_duplicates(args).map(|()| Value::Empty),
            "autofit" => crate::fit::autofit(self),
            "merge" => self
                .merge(arg_bool(args, 0, "Across")?.unwrap_or(false))
                .map(|()| Value::Empty),
            "unmerge" => {
                let idx = self.idx()?;
                let mut book = self.s.write()?;
                for a in &self.areas {
                    book.unmerge_range(idx, *a).map_err(engine)?;
                }
                Ok(Value::Empty)
            }
            "calculate" => {
                self.s.read()?;
                Ok(Value::Empty)
            }
            "borderaround" => {
                let line = arg_i64(args, 0, "LineStyle")?.unwrap_or(XL_CONTINUOUS);
                let weight = arg_i64(args, 1, "Weight")?.unwrap_or(XL_THIN);
                let color = arg_i64(args, 3, "Color")?.map(bgr_to_rgb);
                crate::format::draw_border(
                    &self.s,
                    self.sheet,
                    &self.areas,
                    sheets_engine::BorderEdges::Outline,
                    line,
                    weight,
                    color,
                )?;
                Ok(Value::Boolean(true))
            }
            "filldown" | "fillright" => {
                let a = self.first();
                let idx = self.idx()?;
                let mut book = self.s.write()?;
                let fill = if lower == "filldown" {
                    sheets_engine::Fill::down(a)
                } else {
                    sheets_engine::Fill::right(a)
                };
                if let Some(fill) = fill {
                    book.fill(idx, &fill).map_err(engine)?;
                }
                Ok(Value::Boolean(true))
            }
            "show" | "showdetail" => Ok(Value::Boolean(true)),
            "printout" | "printpreview" => crate::page_setup::print_out(),
            "group" | "ungroup" => Err(ScriptError::unsupported("Outline grouping from a macro")),
            _ => Err(ScriptError::no_member("Range", name)),
        }
    }

    fn put(&self, name: &str, args: &[Arg], value: Value) -> Result<(), ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "" | "value" | "value2" if args.is_empty() => self.write(&value, Write::Value),
            "formula" | "formulalocal" | "formula2" | "formulaarray" => {
                self.write(&value, Write::Formula)
            }
            "formular1c1" | "formular1c1local" | "formula2r1c1" => {
                self.write(&value, Write::FormulaR1C1)
            }
            "numberformat" | "numberformatlocal" => {
                let mut fmt = to_str(&value)?;
                if fmt.eq_ignore_ascii_case("general") {
                    fmt = "general".into();
                }
                self.style_value("num_fmt", &fmt)
            }
            "horizontalalignment" => {
                let v = halign_value(to_i64(&value)?).ok_or_else(|| {
                    err1004("Unable to set the HorizontalAlignment property of the Range class")
                })?;
                self.style_value("alignment.horizontal", v)
            }
            "verticalalignment" => {
                let v = valign_value(to_i64(&value)?).ok_or_else(|| {
                    err1004("Unable to set the VerticalAlignment property of the Range class")
                })?;
                self.style_value("alignment.vertical", v)
            }
            "wraptext" => self.style_value(
                "alignment.wrap_text",
                if to_bool(&value)? { "true" } else { "false" },
            ),
            "indentlevel" => self.style_value("alignment.indent", &to_i64(&value)?.to_string()),
            "orientation" => {
                let n = to_i64(&value)?;
                let rot = match n {
                    XL_UPWARD => 90,
                    XL_DOWNWARD => 180,
                    XL_VERTICAL => 255,
                    XL_HORIZONTAL => 0,
                    n if (0..=90).contains(&n) => n,
                    n if (-90..0).contains(&n) => 90 - n,
                    _ => return Err(err1004("Unable to set the Orientation property")),
                };
                self.style_value("alignment.text_rotation", &rot.to_string())
            }
            "mergecells" => {
                if to_bool(&value)? {
                    self.merge(false)
                } else {
                    let idx = self.idx()?;
                    let mut book = self.s.write()?;
                    for a in &self.areas {
                        book.unmerge_range(idx, *a).map_err(engine)?;
                    }
                    Ok(())
                }
            }
            "columnwidth" => {
                let chars = to_f64(&value)?;
                if !(0.0..=255.0).contains(&chars) {
                    return Err(err1004(
                        "Unable to set the ColumnWidth property of the Range class",
                    ));
                }
                let idx = self.idx()?;
                let mut book = self.s.write()?;
                let scale = book.column_scale(idx);
                for a in &self.areas {
                    if chars == 0.0 {
                        book.set_columns_hidden(idx, a.left, a.right, true)
                            .map_err(engine)?;
                        continue;
                    }
                    let stored =
                        ((chars * scale.digit_px + 5.0) / scale.digit_px * 256.0).trunc() / 256.0;
                    let px = scale.px(stored);
                    book.set_column_width(idx, a.left, a.right, px)
                        .map_err(engine)?;
                }
                Ok(())
            }
            "rowheight" => {
                let pts = to_f64(&value)?;
                if !(0.0..=409.5).contains(&pts) {
                    return Err(err1004(
                        "Unable to set the RowHeight property of the Range class",
                    ));
                }
                let idx = self.idx()?;
                let mut book = self.s.write()?;
                for a in &self.areas {
                    if pts == 0.0 {
                        book.hide_rows(idx, a.top, a.bottom).map_err(engine)?;
                    } else {
                        book.set_row_heights(
                            idx,
                            a.top,
                            a.bottom,
                            sheets_engine::RowHeight::Points(pts),
                        )
                        .map_err(engine)?;
                    }
                }
                Ok(())
            }
            "hidden" => {
                let hide = to_bool(&value)?;
                let idx = self.idx()?;
                let mut book = self.s.write()?;
                for a in &self.areas {
                    if a.is_whole_columns() || self.mode == Mode::Columns {
                        book.set_columns_hidden(idx, a.left, a.right, hide)
                            .map_err(engine)?;
                    } else if a.is_whole_rows() || self.mode == Mode::Rows {
                        if hide {
                            book.hide_rows(idx, a.top, a.bottom).map_err(engine)?;
                        } else {
                            book.unhide_rows(idx, a.top, a.bottom).map_err(engine)?;
                        }
                    } else {
                        return Err(err1004(
                            "Unable to set the Hidden property of the Range class",
                        ));
                    }
                }
                Ok(())
            }
            "name" => {
                let n = to_str(&value)?;
                let idx = self.idx()?;
                let mut book = self.s.write()?;
                let sheet = book.sheet_names()[idx as usize].clone();
                let refers = format!(
                    "{}!{}",
                    addr::sheet_ref(&sheet),
                    addr::area_text(self.first(), true, true)
                );
                book.set_defined_name(&n, None, &refers).map_err(engine)
            }
            "locked" => crate::protection::set_locked(self, &value),
            "formulahidden" => crate::protection::set_formula_hidden(self, &value),
            "pagebreak" => crate::page_setup::set_range_page_break(self, &value),
            "style" | "shrinktofit" | "addindent" | "readingorder" => Ok(()),
            // `Cells(1, 2) = x`, `Range("A1").Offset(1) = x`: the member's
            // range gets the value.
            _ => {
                let target = self.get(name, args)?;
                match target {
                    Value::Object(Some(o)) => o.put("", &[], value),
                    _ => Err(ScriptError::no_member("Range", name)),
                }
            }
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        let mut out = Vec::new();
        for a in &self.areas {
            // `For Each c In Columns("A")` visits the used cells.
            let a = if (a.is_whole_columns() || a.is_whole_rows()) && self.mode == Mode::Cells {
                let idx = self.idx()?;
                let book = self.s.read()?;
                match self.used(&book, idx, *a) {
                    Some(u) => u,
                    None => continue,
                }
            } else {
                *a
            };
            match self.mode {
                Mode::Cells => {
                    if u64::from(a.rows_count()) * u64::from(a.cols_count()) > 5_000_000 {
                        return Err(ScriptError::runtime(7));
                    }
                    for r in a.top..=a.bottom {
                        for c in a.left..=a.right {
                            out.push(self.at(CellRange::from_corners((r, c), (r, c))));
                        }
                    }
                }
                Mode::Rows => {
                    for r in a.top..=a.bottom {
                        let mut row = RangeObj::new(
                            &self.s,
                            self.sheet,
                            CellRange::from_corners((r, a.left), (r, a.right)),
                        );
                        if let Some(x) = Rc::get_mut(&mut row) {
                            x.mode = Mode::Rows;
                        }
                        out.push(obj(row));
                    }
                }
                Mode::Columns => {
                    for c in a.left..=a.right {
                        let mut col = RangeObj::new(
                            &self.s,
                            self.sheet,
                            CellRange::from_corners((a.top, c), (a.bottom, c)),
                        );
                        if let Some(x) = Rc::get_mut(&mut col) {
                            x.mode = Mode::Columns;
                        }
                        out.push(obj(col));
                    }
                }
            }
        }
        Ok(out)
    }
}

impl RangeObj {
    fn at_checked(&self, dr: i64, dc: i64) -> Result<Value, ScriptError> {
        Ok(self.at(self.cell_at(dr, dc)?))
    }
}

/// `Range.Areas`.
pub struct AreasObj {
    range: Rc<RangeObj>,
}

impl Dispatch for AreasObj {
    fn type_name(&self) -> String {
        "Areas".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "count" => Ok(Value::Long(self.range.areas.len() as i32)),
            "" | "item" => self.range.get("Areas", args),
            _ => Err(ScriptError::no_member("Areas", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        Ok(self
            .range
            .areas
            .iter()
            .map(|a| obj(RangeObj::new(&self.range.s, self.range.sheet, *a)))
            .collect())
    }
}

/// The intersection of two ranges on the same sheet.
pub fn intersect(a: &RangeObj, b: &RangeObj) -> Vec<CellRange> {
    if a.sheet != b.sheet {
        return Vec::new();
    }
    let mut out = Vec::new();
    for x in &a.areas {
        for y in &b.areas {
            if x.intersects(y) {
                out.push(CellRange {
                    top: x.top.max(y.top),
                    left: x.left.max(y.left),
                    bottom: x.bottom.min(y.bottom),
                    right: x.right.min(y.right),
                });
            }
        }
    }
    out
}

pub fn as_object(r: Rc<RangeObj>) -> ObjectRef {
    r as ObjectRef
}

#[cfg(test)]
mod tests {
    use super::replace_text;

    #[test]
    fn replacing_text() {
        assert_eq!(
            replace_text("Apple pie", "apple", "Pear", false, false),
            "Pear pie"
        );
        assert_eq!(
            replace_text("Apple pie", "apple", "Pear", false, true),
            "Apple pie"
        );
        assert_eq!(replace_text("x", "X", "y", true, false), "y");
        assert_eq!(replace_text("xx", "X", "y", true, false), "xx");
    }
}
