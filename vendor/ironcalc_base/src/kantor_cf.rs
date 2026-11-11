//! KANTOR PATCH (conditional formatting): formulas of conditional formatting
//! rules, parsed once and evaluated cell by cell by the caller.
//!
//! Upstream evaluates every rule over every cell of its range after each
//! `evaluate()` (`Model::evaluate_conditional_formatting`) and keeps a result
//! per cell. Kantor evaluates rules lazily, for the cells on screen, with its
//! own caches; it turns the eager pass off with
//! [`set_eager_conditional_formatting`] and uses this API for the formulas
//! (`expression` rules, `cellIs` operands, `cfvo` formulas).
//!
//! Evaluation reads stored values only: a formula cell the rule refers to
//! gives its last value and is never recalculated, so painting can't change
//! the workbook (in manual calculation a pending formula stays pending).

use std::sync::atomic::{AtomicBool, Ordering};

use crate::calc_result::CalcResult;
use crate::expressions::parser::Node;
use crate::expressions::types::{CellReferenceIndex, CellReferenceRC};
use crate::{Model, UserModel};

static EAGER: AtomicBool = AtomicBool::new(true);

/// Whether `Model::evaluate_conditional_formatting` (run on load, after each
/// evaluation and on theme changes) computes anything. On by default, as
/// upstream; off, its results stay empty.
pub fn set_eager_conditional_formatting(on: bool) {
    EAGER.store(on, Ordering::Relaxed);
}

pub(crate) fn eager_conditional_formatting() -> bool {
    EAGER.load(Ordering::Relaxed)
}

/// A parsed conditional formatting formula, relative to the cell it was
/// parsed for.
#[derive(Clone)]
pub struct CfFormula(Node);

/// The value of a [`CfFormula`] at one cell.
#[derive(Clone, Debug, PartialEq)]
pub enum CfValue {
    Number(f64),
    Text(String),
    Bool(bool),
    Error,
    /// An empty cell, or a reference to one.
    Empty,
    /// A range or array (the caller decides; Excel takes the top-left value
    /// in most places, and rules treat it as not matching).
    Other,
}

impl Model<'_> {
    /// Parses a rule formula (English, with or without a leading `=`) as if
    /// written in (`sheet`, `row`, `column`) (1-based). `None` if it doesn't
    /// parse.
    pub fn cf_parse(&mut self, sheet: u32, formula: &str, row: i32, column: i32) -> Option<CfFormula> {
        let body = formula.trim();
        let body = body.strip_prefix('=').unwrap_or(body);
        if body.is_empty() {
            return None;
        }
        let context = CellReferenceRC {
            sheet: self.workbook.worksheets.get(sheet as usize)?.get_name(),
            row,
            column,
        };
        match self.parse_internal_formula(body, &context) {
            Node::ParseErrorKind { .. } => None,
            node => Some(CfFormula(node)),
        }
    }

    /// Evaluates `formula` for cell (`sheet`, `row`, `column`) (1-based):
    /// relative references are taken from that cell. Reads stored values only.
    pub fn cf_evaluate(&mut self, formula: &CfFormula, sheet: u32, row: i32, column: i32) -> CfValue {
        let stored_only = self.evaluation.stored_only;
        self.evaluation.stored_only = true;
        let result = self.evaluate_node_in_context(&formula.0, CellReferenceIndex { sheet, row, column });
        let result = match result {
            CalcResult::Range { left, right } if left == right => self.evaluate_cell(left),
            r => r,
        };
        self.evaluation.stored_only = stored_only;
        match result {
            CalcResult::Number(n) => CfValue::Number(n),
            CalcResult::String(s) => CfValue::Text(s),
            CalcResult::Boolean(b) => CfValue::Bool(b),
            CalcResult::Error { .. } => CfValue::Error,
            CalcResult::EmptyCell | CalcResult::EmptyArg => CfValue::Empty,
            _ => CfValue::Other,
        }
    }
}

impl UserModel<'_> {
    /// [`Model::cf_parse`].
    pub fn cf_parse(&mut self, sheet: u32, formula: &str, row: i32, column: i32) -> Option<CfFormula> {
        self.model.cf_parse(sheet, formula, row, column)
    }

    /// [`Model::cf_evaluate`].
    pub fn cf_evaluate(&mut self, formula: &CfFormula, sheet: u32, row: i32, column: i32) -> CfValue {
        self.model.cf_evaluate(formula, sheet, row, column)
    }
}
