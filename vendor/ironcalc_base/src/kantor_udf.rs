//! KANTOR PATCH (user functions, §37): functions the host defines (VBA
//! `Public Function`s of a workbook's macros used in cell formulas), and
//! formulas evaluated without a cell to hold them (VBA's
//! `WorksheetFunction.Sum(...)` and `Evaluate`).
//!
//! A name that is neither a built-in function nor a LAMBDA defined name
//! parses as `Node::NamedFunctionKind` and evaluates to `#NAME?` upstream.
//! With user functions set ([`Model::kantor_set_user_functions`]), such a
//! call asks them first: its arguments are evaluated (ranges read cell by
//! cell, so they are dependencies like any other read) and handed over as
//! [`KantorValue`]s. The formula text is untouched.

use std::sync::Arc;

use crate::calc_result::CalcResult;
use crate::expressions::parser::{ArrayNode, Node};
use crate::expressions::token::{get_error_by_name, Error};
use crate::expressions::types::{CellReferenceIndex, CellReferenceRC};
use crate::{Model, UserModel};

/// A value passed to or returned by a user function, or the value of a
/// formula evaluated with [`Model::kantor_evaluate_formula`].
#[derive(Clone, Debug, PartialEq)]
pub enum KantorValue {
    /// An empty cell or an omitted argument.
    Empty,
    Number(f64),
    Text(String),
    Bool(bool),
    /// An error value by its name (`#DIV/0!`, `#N/A`).
    Error(String),
    /// A range's values or an array, by rows.
    Array(Vec<Vec<KantorValue>>),
}

/// A cell or range by sheet and position: where a range argument came from,
/// or the cell whose formula calls a user function. Rows and columns are
/// 1-based.
#[derive(Clone, Debug, PartialEq)]
pub struct KantorRangeRef {
    /// The sheet's index.
    pub sheet: u32,
    pub sheet_name: String,
    pub row: i32,
    pub column: i32,
    pub rows: i32,
    pub columns: i32,
}

/// An argument of a user function: its value and, for a reference, the
/// range it names (VBA passes a `Range` object).
#[derive(Clone, Debug, PartialEq)]
pub struct KantorArg {
    pub value: KantorValue,
    pub range: Option<KantorRangeRef>,
}

/// Functions the host defines. `call` returns `None` when it has no
/// function `name` (the formula shows `#NAME?`).
pub trait KantorUserFunctions: Send + Sync {
    fn call(&self, name: &str, args: Vec<KantorValue>) -> Option<KantorValue>;

    /// [`Self::call`] with where reference arguments came from and the
    /// calling cell (`Application.Caller`). The engine calls this one.
    fn call_with(
        &self,
        name: &str,
        args: Vec<KantorArg>,
        _caller: &KantorRangeRef,
    ) -> Option<KantorValue> {
        self.call(name, args.into_iter().map(|a| a.value).collect())
    }
}

/// Cells read from one range argument at most.
const MAX_RANGE_CELLS: i64 = 4_000_000;

/// Ranges bigger than this (whole columns or rows) are clipped to the
/// sheet's used area.
const CLIP_CELLS: i64 = 65_536;

impl Model<'_> {
    /// Sets (or with `None`, removes) the host's functions. The caller
    /// recalculates for formulas that use them to change.
    pub fn kantor_set_user_functions(&mut self, functions: Option<Arc<dyn KantorUserFunctions>>) {
        self.kantor_udfs = functions;
    }

    /// Calls a user function for `NamedFunctionKind` (`None`: there is none).
    pub(crate) fn kantor_call_user_function(
        &mut self,
        name: &str,
        args: &[Node],
        cell: CellReferenceIndex,
    ) -> Option<CalcResult> {
        let functions = self.kantor_udfs.clone()?;
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            let result = self.evaluate_node_in_context(arg, cell);
            let range = match &result {
                CalcResult::Range { left, right } if left.sheet == right.sheet => Some(
                    self.kantor_range_ref(
                        left.sheet,
                        left.row.min(right.row),
                        left.column.min(right.column),
                        (left.row - right.row).abs() + 1,
                        (left.column - right.column).abs() + 1,
                    ),
                ),
                _ => None,
            };
            values.push(KantorArg {
                value: self.kantor_value_of(result),
                range,
            });
        }
        let caller = self.kantor_range_ref(cell.sheet, cell.row, cell.column, 1, 1);
        let result = functions.call_with(name, values, &caller)?;
        Some(self.kantor_calc_result(result, cell))
    }

    fn kantor_range_ref(
        &self,
        sheet: u32,
        row: i32,
        column: i32,
        rows: i32,
        columns: i32,
    ) -> KantorRangeRef {
        KantorRangeRef {
            sheet,
            sheet_name: self
                .workbook
                .worksheets
                .get(sheet as usize)
                .map(|w| w.get_name())
                .unwrap_or_default(),
            row,
            column,
            rows,
            columns,
        }
    }

    fn kantor_value_of(&mut self, result: CalcResult) -> KantorValue {
        match result {
            CalcResult::Number(n) => KantorValue::Number(n),
            CalcResult::String(s) => KantorValue::Text(s),
            CalcResult::Boolean(b) => KantorValue::Bool(b),
            CalcResult::Error { error, .. } => KantorValue::Error(error.to_string()),
            CalcResult::EmptyCell | CalcResult::EmptyArg => KantorValue::Empty,
            CalcResult::Array(rows) => KantorValue::Array(
                rows.iter()
                    .map(|r| r.iter().map(array_value).collect())
                    .collect(),
            ),
            CalcResult::Range { left, right } => {
                if left.sheet != right.sheet {
                    return KantorValue::Error(Error::VALUE.to_string());
                }
                let (top, bottom) = (left.row.min(right.row), left.row.max(right.row));
                let (first, last) = (left.column.min(right.column), left.column.max(right.column));
                // A big range (a whole column or row): only its used part.
                let size = i64::from(bottom - top + 1) * i64::from(last - first + 1);
                let (bottom, last) = match self.workbook.worksheet(left.sheet) {
                    Ok(ws) if size > CLIP_CELLS => {
                        let d = ws.dimension();
                        (
                            bottom.min(d.max_row.max(top)),
                            last.min(d.max_column.max(first)),
                        )
                    }
                    Ok(_) => (bottom, last),
                    Err(_) => return KantorValue::Error(Error::REF.to_string()),
                };
                let cells = i64::from(bottom - top + 1) * i64::from(last - first + 1);
                if cells > MAX_RANGE_CELLS {
                    return KantorValue::Error(Error::VALUE.to_string());
                }
                let mut rows = Vec::with_capacity((bottom - top + 1) as usize);
                for row in top..=bottom {
                    let mut out = Vec::with_capacity((last - first + 1) as usize);
                    for column in first..=last {
                        let v = self.evaluate_cell(CellReferenceIndex {
                            sheet: left.sheet,
                            row,
                            column,
                        });
                        out.push(match v {
                            CalcResult::Range { .. } | CalcResult::Array(_) => {
                                KantorValue::Error(Error::VALUE.to_string())
                            }
                            v => self.kantor_value_of(v),
                        });
                    }
                    rows.push(out);
                }
                KantorValue::Array(rows)
            }
            CalcResult::Lambda(_) => KantorValue::Error(Error::VALUE.to_string()),
        }
    }

    fn kantor_calc_result(&mut self, value: KantorValue, cell: CellReferenceIndex) -> CalcResult {
        match value {
            KantorValue::Empty => CalcResult::Number(0.0),
            KantorValue::Number(n) if n.is_finite() => CalcResult::Number(n),
            KantorValue::Number(_) => {
                CalcResult::new_error(Error::NUM, cell, "Not a finite number".to_string())
            }
            KantorValue::Text(s) => CalcResult::String(s),
            KantorValue::Bool(b) => CalcResult::Boolean(b),
            KantorValue::Error(name) => CalcResult::new_error(
                get_error_by_name(&name, self.language).unwrap_or(Error::VALUE),
                cell,
                format!("User function returned {name}"),
            ),
            // An array result: its first value (no spilling from user
            // functions).
            KantorValue::Array(rows) => {
                match rows.into_iter().next().and_then(|r| r.into_iter().next()) {
                    Some(v) => self.kantor_calc_result(v, cell),
                    None => CalcResult::new_error(Error::VALUE, cell, "Empty array".to_string()),
                }
            }
        }
    }

    /// Evaluates `formula` (English, with or without `=`) as if it were in
    /// cell (`sheet`, `row`, `column`) (1-based), without storing it. A
    /// range result comes back as its values. Reads stored values only (as
    /// conditional formatting does): the caller keeps the workbook
    /// calculated.
    pub fn kantor_evaluate_formula(
        &mut self,
        sheet: u32,
        row: i32,
        column: i32,
        formula: &str,
    ) -> Result<KantorValue, String> {
        let body = formula.trim();
        let body = body.strip_prefix('=').unwrap_or(body);
        let context = CellReferenceRC {
            sheet: self
                .workbook
                .worksheets
                .get(sheet as usize)
                .ok_or("no such sheet")?
                .get_name(),
            row,
            column,
        };
        let node = match self.parse_internal_formula(body, &context) {
            Node::ParseErrorKind { message, .. } => return Err(message),
            node => node,
        };
        let stored_only = self.evaluation.stored_only;
        self.evaluation.stored_only = true;
        let cell = CellReferenceIndex { sheet, row, column };
        let result = self.evaluate_node_in_context(&node, cell);
        let value = self.kantor_value_of(result);
        self.evaluation.stored_only = stored_only;
        Ok(value)
    }

    /// The value of a cell as stored (1-based), errors by name.
    pub fn kantor_cell_value(&mut self, sheet: u32, row: i32, column: i32) -> KantorValue {
        let stored_only = self.evaluation.stored_only;
        self.evaluation.stored_only = true;
        let result = self.evaluate_cell(CellReferenceIndex { sheet, row, column });
        let value = match result {
            CalcResult::Range { .. } | CalcResult::Array(_) => {
                KantorValue::Error(Error::VALUE.to_string())
            }
            v => self.kantor_value_of(v),
        };
        self.evaluation.stored_only = stored_only;
        value
    }
}

fn array_value(n: &ArrayNode) -> KantorValue {
    match n {
        ArrayNode::Boolean(b) => KantorValue::Bool(*b),
        ArrayNode::Number(x) => KantorValue::Number(*x),
        ArrayNode::String(s) => KantorValue::Text(s.clone()),
        ArrayNode::Error(e) => KantorValue::Error(e.to_string()),
        ArrayNode::Empty => KantorValue::Empty,
    }
}

impl UserModel<'_> {
    /// [`Model::kantor_set_user_functions`].
    pub fn kantor_set_user_functions(&mut self, functions: Option<Arc<dyn KantorUserFunctions>>) {
        self.model.kantor_set_user_functions(functions);
    }

    /// [`Model::kantor_evaluate_formula`].
    pub fn kantor_evaluate_formula(
        &mut self,
        sheet: u32,
        row: i32,
        column: i32,
        formula: &str,
    ) -> Result<KantorValue, String> {
        self.model
            .kantor_evaluate_formula(sheet, row, column, formula)
    }

    /// Copies the selected area (as `copy_to_clipboard`) and pastes it with
    /// its top-left at (`row`, `column`) of sheet `target` (as
    /// `paste_from_clipboard`; with `cut`, a move): VBA's `Range.Copy
    /// Destination:=…`, without a clipboard in between.
    pub fn kantor_copy_selection_to(
        &mut self,
        target: u32,
        row: i32,
        column: i32,
        cut: bool,
    ) -> Result<(), String> {
        let clip = self.copy_to_clipboard()?;
        self.set_selected_sheet(target)?;
        self.set_selected_cell(row, column)?;
        self.paste_from_clipboard(clip.sheet, clip.range, &clip.data, cut)
    }

    /// [`Model::kantor_cell_value`].
    pub fn kantor_cell_value(&mut self, sheet: u32, row: i32, column: i32) -> KantorValue {
        self.model.kantor_cell_value(sheet, row, column)
    }
}
