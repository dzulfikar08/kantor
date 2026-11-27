//! What a VBA function used in a cell formula sees: a read-only `Range` for
//! each reference argument (`Function Total(r As Range)`) and
//! `Application.Caller` (the calling cell).
//!
//! The engine evaluates the function while it recalculates, so the function
//! can't reach the workbook: a range argument carries the values the engine
//! read for it (the same cells the formula depends on). Reading inside them
//! works (`Value`, `Cells`, `Item`, `Rows`, `Columns`, `Count`, `Offset` and
//! `Resize` inside the block, `For Each`, `Address`, `Row`, `Column`);
//! reading outside them, and any write, fails, and the cell shows `#VALUE!`
//! as Excel shows for a function that writes.

use std::rc::Rc;

use kantor_script::{Arg, Array, Dispatch, ObjectRef, ScriptError, Value};
use sheets_engine::CellRange;
use sheets_engine::script::{KantorRangeRef, KantorValue};

use crate::addr;
use crate::values::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Cells,
    Rows,
    Columns,
}

/// A read-only range inside a UDF.
pub struct UdfRange {
    sheet: Rc<str>,
    /// 1-based top-left and size.
    row: i32,
    col: i32,
    rows: i32,
    cols: i32,
    /// The values the engine read, with their top-left.
    values: Rc<Vec<Vec<KantorValue>>>,
    base: (i32, i32),
    /// The size of the argument the values are for.
    block: (i32, i32),
    mode: Mode,
}

fn read_only() -> ScriptError {
    err1004("A function used in a formula can't change cells")
}

impl UdfRange {
    pub fn new(r: &KantorRangeRef, values: Option<KantorValue>) -> UdfRange {
        let values = match values {
            Some(KantorValue::Array(rows)) => rows,
            Some(v) => vec![vec![v]],
            None => Vec::new(),
        };
        UdfRange {
            sheet: Rc::from(r.sheet_name.as_str()),
            row: r.row,
            col: r.column,
            rows: r.rows.max(1),
            cols: r.columns.max(1),
            values: Rc::new(values),
            base: (r.row, r.column),
            block: (r.rows.max(1), r.columns.max(1)),
            mode: Mode::Cells,
        }
    }

    fn sub(&self, row: i32, col: i32, rows: i32, cols: i32) -> Value {
        Value::object(Rc::new(UdfRange {
            sheet: self.sheet.clone(),
            row,
            col,
            rows,
            cols,
            values: self.values.clone(),
            base: self.base,
            block: self.block,
            mode: Mode::Cells,
        }))
    }

    fn with_mode(&self, mode: Mode) -> Value {
        Value::object(Rc::new(UdfRange {
            sheet: self.sheet.clone(),
            values: self.values.clone(),
            mode,
            ..*self
        }))
    }

    fn cell(&self, row: i32, col: i32) -> Result<Value, ScriptError> {
        let (r, c) = (row - self.base.0, col - self.base.1);
        if r < 0 || c < 0 || r >= self.block.0 || c >= self.block.1 {
            return Err(outside());
        }
        // Past the used part of a whole column or row: empty.
        Ok(self
            .values
            .get(r as usize)
            .and_then(|x| x.get(c as usize))
            .map_or(Value::Empty, |v| from_cell(v.clone(), false)))
    }

    fn value(&self) -> Result<Value, ScriptError> {
        if self.rows == 1 && self.cols == 1 {
            return self.cell(self.row, self.col);
        }
        let mut rows = Vec::with_capacity(self.rows as usize);
        for r in 0..self.rows {
            let mut out = Vec::with_capacity(self.cols as usize);
            for c in 0..self.cols {
                out.push(self.cell(self.row + r, self.col + c)?);
            }
            rows.push(out);
        }
        Ok(Value::array(Array::from_rows(1, rows)))
    }

    /// `Item(i[, j])` / `Cells(i, j)`, 1-based from the top-left.
    fn item(&self, args: &[Arg]) -> Result<Value, ScriptError> {
        let i = to_i64(need(args, 0, "RowIndex")?)? as i32;
        let j = match arg(args, 1, "ColumnIndex") {
            Some(v) if !v.is_missing() => Some(to_i64(v)? as i32),
            _ => None,
        };
        match (self.mode, j) {
            (Mode::Rows, None) => Ok(self.sub(self.row + i - 1, self.col, 1, self.cols)),
            (Mode::Columns, None) => Ok(self.sub(self.row, self.col + i - 1, self.rows, 1)),
            (_, None) => {
                let k = i - 1;
                let cols = self.cols.max(1);
                Ok(self.sub(self.row + k / cols, self.col + k % cols, 1, 1))
            }
            (_, Some(j)) => Ok(self.sub(self.row + i - 1, self.col + j - 1, 1, 1)),
        }
    }

    fn area(&self) -> CellRange {
        CellRange::from_corners(
            ((self.row - 1).max(0) as u32, (self.col - 1).max(0) as u32),
            (
                (self.row + self.rows - 2).max(0) as u32,
                (self.col + self.cols - 2).max(0) as u32,
            ),
        )
    }
}

fn outside() -> ScriptError {
    err1004("A function used in a formula can only read the cells passed to it")
}

impl Dispatch for UdfRange {
    fn type_name(&self) -> String {
        "Range".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "" if !args.is_empty() => self.item(args),
            "" | "value" | "value2" => self.value(),
            "text" => Ok(Value::String(to_str(&self.value()?)?)),
            "item" | "cells" if !args.is_empty() => self.item(args),
            "cells" => Ok(self.with_mode(Mode::Cells)),
            "rows" if args.is_empty() => Ok(self.with_mode(Mode::Rows)),
            "columns" if args.is_empty() => Ok(self.with_mode(Mode::Columns)),
            "rows" => self.with_mode_obj(Mode::Rows).item(args),
            "columns" => self.with_mode_obj(Mode::Columns).item(args),
            "count" | "countlarge" => Ok(Value::Long(match self.mode {
                Mode::Rows => self.rows,
                Mode::Columns => self.cols,
                Mode::Cells => self.rows * self.cols,
            })),
            "row" => Ok(Value::Long(self.row)),
            "column" => Ok(Value::Long(self.col)),
            "address" => {
                let row_abs = arg_bool(args, 0, "RowAbsolute")?.unwrap_or(true);
                let col_abs = arg_bool(args, 1, "ColumnAbsolute")?.unwrap_or(true);
                let external = arg_bool(args, 3, "External")?.unwrap_or(false);
                let text = addr::area_text(self.area(), row_abs, col_abs);
                Ok(Value::String(if external {
                    format!("{}!{text}", addr::sheet_ref(&self.sheet))
                } else {
                    text
                }))
            }
            "offset" => {
                let r = arg_i64(args, 0, "RowOffset")?.unwrap_or(0) as i32;
                let c = arg_i64(args, 1, "ColumnOffset")?.unwrap_or(0) as i32;
                Ok(self.sub(self.row + r, self.col + c, self.rows, self.cols))
            }
            "resize" => {
                let r = arg_i64(args, 0, "RowSize")?.map_or(self.rows, |v| v as i32);
                let c = arg_i64(args, 1, "ColumnSize")?.map_or(self.cols, |v| v as i32);
                Ok(self.sub(self.row, self.col, r.max(1), c.max(1)))
            }
            "worksheet" | "parent" => Ok(Value::object(Rc::new(UdfSheet {
                name: self.sheet.clone(),
            }))),
            "hasformula" => Ok(Value::Boolean(false)),
            _ => Err(ScriptError::no_member("Range", name)),
        }
    }

    fn put(&self, _name: &str, _args: &[Arg], _value: Value) -> Result<(), ScriptError> {
        Err(read_only())
    }

    fn call(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "clear" | "clearcontents" | "delete" | "insert" | "copy" | "cut" | "select"
            | "activate" | "pastespecial" | "sort" | "autofilter" | "merge" => Err(read_only()),
            _ => self.get(name, args),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        let mut out = Vec::new();
        match self.mode {
            Mode::Rows => {
                for r in 0..self.rows {
                    out.push(self.sub(self.row + r, self.col, 1, self.cols));
                }
            }
            Mode::Columns => {
                for c in 0..self.cols {
                    out.push(self.sub(self.row, self.col + c, self.rows, 1));
                }
            }
            Mode::Cells => {
                for r in 0..self.rows {
                    for c in 0..self.cols {
                        out.push(self.sub(self.row + r, self.col + c, 1, 1));
                    }
                }
            }
        }
        Ok(out)
    }
}

impl UdfRange {
    fn with_mode_obj(&self, mode: Mode) -> UdfRange {
        UdfRange {
            sheet: self.sheet.clone(),
            values: self.values.clone(),
            mode,
            ..*self
        }
    }
}

/// The sheet of a UDF's range: its name only.
struct UdfSheet {
    name: Rc<str>,
}

impl Dispatch for UdfSheet {
    fn type_name(&self) -> String {
        "Worksheet".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "name" | "" => Ok(Value::String(self.name.to_string())),
            _ => Err(outside()),
        }
    }

    fn put(&self, _name: &str, _args: &[Arg], _value: Value) -> Result<(), ScriptError> {
        Err(read_only())
    }
}

/// `Application` inside a UDF: `Caller`, `Volatile`; anything else is
/// out of reach.
pub struct UdfApp {
    pub caller: KantorRangeRef,
}

impl Dispatch for UdfApp {
    fn type_name(&self) -> String {
        "Application".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "caller" | "thiscell" => Ok(Value::object(Rc::new(UdfRange::new(&self.caller, None)))),
            "volatile" => Ok(Value::Empty),
            "name" | "" => Ok(Value::string("Microsoft Excel")),
            "application" => Ok(Value::object(Rc::new(UdfApp {
                caller: self.caller.clone(),
            }))),
            _ => Err(ScriptError::no_member("Application", name)),
        }
    }
}

/// The globals inside a UDF: `Application` only.
pub struct UdfGlobals {
    pub app: ObjectRef,
}

impl Dispatch for UdfGlobals {
    fn type_name(&self) -> String {
        "Global".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "application" => Ok(Value::Object(Some(self.app.clone()))),
            _ => Err(ScriptError::no_member("Global", name)),
        }
    }
}
