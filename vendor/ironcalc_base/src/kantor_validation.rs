//! KANTOR PATCH (data validation): what Kantor's data validation needs on
//! top of the conditional formatting formulas (`kantor_cf`).
//!
//! * A list validation's source is a range (`$A$1:$A$5`, `Lists!A:A`, a
//!   defined name, `INDIRECT(...)`, `OFFSET(...)`) or an array; the caller
//!   reads the cells itself ([`Model::cf_evaluate_ref`]).
//! * A value the user types is checked the way `set_user_input` would store
//!   it (number formats, booleans, error names), without storing it
//!   ([`Model::kantor_input_value`]).

use crate::calc_result::CalcResult;
use crate::expressions::parser::ArrayNode;
use crate::expressions::token::get_error_by_name;
use crate::expressions::types::CellReferenceIndex;
use crate::formatter::format::parse_formatted_number;
use crate::{CfFormula, CfValue, Model, UserModel};

/// The value of a formula that may be a range or an array.
#[derive(Clone, Debug, PartialEq)]
pub enum CfRef {
    Value(CfValue),
    /// A range of cells, 1-based and inclusive (whole columns run to the
    /// last row).
    Range {
        sheet: u32,
        top: i32,
        left: i32,
        bottom: i32,
        right: i32,
    },
    /// An array, row by row.
    Array(Vec<Vec<CfValue>>),
}

fn array_value(v: &ArrayNode) -> CfValue {
    match v {
        ArrayNode::Boolean(b) => CfValue::Bool(*b),
        ArrayNode::Number(n) => CfValue::Number(*n),
        ArrayNode::String(s) => CfValue::Text(s.clone()),
        ArrayNode::Error(_) => CfValue::Error,
        ArrayNode::Empty => CfValue::Empty,
    }
}

impl Model<'_> {
    /// Evaluates `formula` (see [`Model::cf_evaluate`]) for cell (`sheet`,
    /// `row`, `column`) (1-based), keeping a range or an array as such.
    /// Reads stored values only.
    pub fn cf_evaluate_ref(&mut self, formula: &CfFormula, sheet: u32, row: i32, column: i32) -> CfRef {
        let stored_only = self.evaluation.stored_only;
        self.evaluation.stored_only = true;
        let result =
            self.evaluate_node_in_context(formula.node(), CellReferenceIndex { sheet, row, column });
        self.evaluation.stored_only = stored_only;
        match result {
            CalcResult::Range { left, right } if left.sheet == right.sheet => CfRef::Range {
                sheet: left.sheet,
                top: left.row.min(right.row),
                left: left.column.min(right.column),
                bottom: left.row.max(right.row),
                right: left.column.max(right.column),
            },
            CalcResult::Array(rows) => {
                CfRef::Array(rows.iter().map(|r| r.iter().map(array_value).collect()).collect())
            }
            CalcResult::Number(n) => CfRef::Value(CfValue::Number(n)),
            CalcResult::String(s) => CfRef::Value(CfValue::Text(s)),
            CalcResult::Boolean(b) => CfRef::Value(CfValue::Bool(b)),
            CalcResult::Error { .. } => CfRef::Value(CfValue::Error),
            CalcResult::EmptyCell | CalcResult::EmptyArg => CfRef::Value(CfValue::Empty),
            _ => CfRef::Value(CfValue::Other),
        }
    }

    /// The value `set_user_input` would store for `input` (a quote prefix
    /// keeps text as text); `None` for a formula.
    pub fn kantor_input_value(&self, input: &str) -> Option<CfValue> {
        if input.is_empty() {
            return Some(CfValue::Empty);
        }
        if let Some(text) = input.strip_prefix('\'') {
            return Some(CfValue::Text(text.to_owned()));
        }
        if self.formula_without_prefix(input).is_some() {
            return None;
        }
        let mut currencies = vec!["$", "€"];
        let currency = &self.locale.currency.symbol;
        if !currencies.iter().any(|e| e == currency) {
            currencies.push(currency);
        }
        if let Ok((v, _)) = parse_formatted_number(input, &currencies, self.locale) {
            return Some(CfValue::Number(v));
        }
        if let Ok(b) = input.to_lowercase().parse::<bool>() {
            return Some(CfValue::Bool(b));
        }
        if get_error_by_name(&input.to_uppercase(), self.language).is_some() {
            return Some(CfValue::Error);
        }
        Some(CfValue::Text(input.to_owned()))
    }
}

impl UserModel<'_> {
    /// [`Model::cf_evaluate_ref`].
    pub fn cf_evaluate_ref(&mut self, formula: &CfFormula, sheet: u32, row: i32, column: i32) -> CfRef {
        self.model.cf_evaluate_ref(formula, sheet, row, column)
    }

    /// [`Model::kantor_input_value`].
    pub fn kantor_input_value(&self, input: &str) -> Option<CfValue> {
        self.model.kantor_input_value(input)
    }
}
