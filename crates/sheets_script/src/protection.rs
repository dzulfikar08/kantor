//! `Range.Locked` and `Range.FormulaHidden`: the cell protection flags the
//! cells' styles keep (`Workbook::cell_protection` /
//! `set_range_protection`), saved as `<xf><protection>`.
//!
//! Kantor doesn't protect sheets: it carries a file's `<sheetProtection>`
//! through a save but has no protection of its own to turn on or check. So `Worksheet.Protect` / `Unprotect` and
//! `Workbook.Protect` / `Unprotect` are accepted and change nothing,
//! `ProtectContents` and friends read `False`, and macro writes are never
//! refused for protection. The flags set here still matter when the file
//! is protected in Excel.

use kantor_script::{ScriptError, Value};
use sheets_engine::{CellRange, Workbook};

use crate::range::RangeObj;
use crate::values::*;

/// Cells read at most for one `Locked` / `FormulaHidden` (beyond that,
/// the first ones decide).
const MAX_READ: u64 = 1_000_000;

/// `areas` with whole rows and columns cut to the used part of the sheet
/// (cells beyond it have the default style).
fn used_areas(book: &Workbook, idx: u32, areas: &[CellRange]) -> Vec<CellRange> {
    let (rows, cols) = book.used_size(idx);
    areas
        .iter()
        .filter_map(|a| {
            if a.is_whole_columns() || a.is_whole_rows() {
                a.clip(rows.max(1), cols.max(1))
            } else {
                Some(*a)
            }
        })
        .collect()
}

/// One flag of the range's cells: `True`/`False` when they agree, `Null`
/// when they differ.
fn flag(r: &RangeObj, hidden: bool) -> Result<Value, ScriptError> {
    let idx = r.idx()?;
    let book = r.s.read()?;
    let mut seen: Option<bool> = None;
    let mut read = 0u64;
    for a in used_areas(&book, idx, &r.areas) {
        for row in a.top..=a.bottom {
            for col in a.left..=a.right {
                let (locked, formula_hidden) = book.cell_protection(idx, row, col);
                let v = if hidden { formula_hidden } else { locked };
                match seen {
                    Some(s) if s != v => return Ok(Value::Null),
                    _ => seen = Some(v),
                }
                read += 1;
                if read >= MAX_READ {
                    return Ok(Value::Boolean(v));
                }
            }
        }
    }
    // Nothing in use: the default style's.
    Ok(Value::Boolean(seen.unwrap_or(!hidden)))
}

fn set_flag(r: &RangeObj, hidden: bool, value: &Value) -> Result<(), ScriptError> {
    let on = to_bool(value)?;
    let idx = r.idx()?;
    let mut book = r.s.write()?;
    for a in used_areas(&book, idx, &r.areas) {
        let (locked, formula_hidden) = if hidden {
            (None, Some(on))
        } else {
            (Some(on), None)
        };
        book.set_range_protection(idx, a, locked, formula_hidden)
            .map_err(engine)?;
    }
    Ok(())
}

/// `Range.Locked`.
pub fn locked(r: &RangeObj) -> Result<Value, ScriptError> {
    flag(r, false)
}

/// `Range.Locked = True | False`.
pub fn set_locked(r: &RangeObj, value: &Value) -> Result<(), ScriptError> {
    set_flag(r, false, value)
}

/// `Range.FormulaHidden`.
pub fn formula_hidden(r: &RangeObj) -> Result<Value, ScriptError> {
    flag(r, true)
}

/// `Range.FormulaHidden = True | False`.
pub fn set_formula_hidden(r: &RangeObj, value: &Value) -> Result<(), ScriptError> {
    set_flag(r, true, value)
}
