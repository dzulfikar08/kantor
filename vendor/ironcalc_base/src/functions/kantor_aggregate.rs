//! KANTOR PATCH (functions): AGGREGATE, and what it shares with SUBTOTAL.
//!
//! `AGGREGATE(function_num, options, ref1, [ref2], ...)` (functions 1–13) and
//! `AGGREGATE(function_num, options, array, k)` (functions 14–19).
//!
//! Options (Excel's table):
//!
//! | option | nested SUBTOTAL/AGGREGATE | hidden rows | error values |
//! |---|---|---|---|
//! | 0 (default) | ignored | counted | counted |
//! | 1 | ignored | ignored | counted |
//! | 2 | ignored | counted | ignored |
//! | 3 | ignored | ignored | ignored |
//! | 4 | counted | counted | counted |
//! | 5 | counted | ignored | counted |
//! | 6 | counted | counted | ignored |
//! | 7 | counted | ignored | ignored |
//!
//! Rows hidden by a filter count as hidden. As in Excel, hidden columns are
//! never skipped (AGGREGATE is meant for columns of data). Functions 1–13
//! take references (or plain values); an array computed by the formula gives
//! #VALUE! there, as in Excel. Functions 14–19 take any array.

use std::collections::{HashMap, HashSet};

use crate::{
    calc_result::CalcResult,
    expressions::{
        parser::{ArrayNode, Node},
        token::Error,
        types::CellReferenceIndex,
    },
    model::Model,
};

use super::Function;

/// Whether `node` calls SUBTOTAL or AGGREGATE anywhere: such a cell is a
/// "nested subtotal" that SUBTOTAL and AGGREGATE (options 0–3) skip.
pub(crate) fn has_subtotal(node: &Node) -> bool {
    match node {
        Node::FunctionKind { kind, args } => {
            matches!(kind, Function::Subtotal | Function::Aggregate)
                || args.iter().any(has_subtotal)
        }
        Node::OpRangeKind { left, right }
        | Node::OpConcatenateKind { left, right }
        | Node::OpSumKind { left, right, .. }
        | Node::OpProductKind { left, right, .. }
        | Node::OpPowerKind { left, right }
        | Node::CompareKind { left, right, .. } => has_subtotal(left) || has_subtotal(right),
        Node::LambdaDefKind { body, .. } => has_subtotal(body),
        Node::LambdaCallKind { lambda, args } => {
            has_subtotal(lambda) || args.iter().any(has_subtotal)
        }
        Node::NamedFunctionKind { args, .. } => args.iter().any(has_subtotal),
        Node::ImplicitIntersection { child, .. } | Node::SpillRangeOperator { child } => {
            has_subtotal(child)
        }
        Node::UnaryKind { right, .. } => has_subtotal(right),
        _ => false,
    }
}

/// One value AGGREGATE looks at.
enum Item {
    Number(f64),
    Other,
    Error(CalcResult),
}

#[derive(Clone, Copy)]
struct Options {
    skip_nested: bool,
    skip_hidden: bool,
    skip_errors: bool,
}

impl<'a> Model<'a> {
    /// Whether the cell's formula contains SUBTOTAL or AGGREGATE.
    pub(crate) fn cell_has_subtotal(&self, sheet: u32, row: i32, column: i32) -> bool {
        let Some(worksheet) = self.workbook.worksheets.get(sheet as usize) else {
            return false;
        };
        let Some(formula) = worksheet.cell(row, column).and_then(|c| c.get_formula()) else {
            return false;
        };
        self.parsed_formulas
            .get(sheet as usize)
            .and_then(|f| f.get(formula as usize))
            .is_some_and(|(node, _)| has_subtotal(node))
    }

    /// The hidden rows of a sheet (hidden by hand or by a filter).
    pub(crate) fn hidden_rows(&self, sheet: u32) -> HashSet<i32> {
        match self.workbook.worksheet(sheet) {
            Ok(ws) => ws.rows.iter().filter(|r| r.hidden).map(|r| r.r).collect(),
            Err(_) => HashSet::new(),
        }
    }

    /// The last row of `left..right` worth reading (the rest is empty), with
    /// the empty tail put on record for dynamic arrays.
    fn used_last_row(&mut self, left: CellReferenceIndex, right: CellReferenceIndex) -> i32 {
        let max_row = match self.used_dimension(left.sheet) {
            Ok(d) => d.max_row,
            Err(_) => return right.row,
        };
        if right.row > max_row {
            let first = max_row.max(left.row - 1) + 1;
            self.record_seen_empty_range(left.sheet, first, left.column, right.row, right.column);
            max_row
        } else {
            right.row
        }
    }

    fn aggregate_items(
        &mut self,
        args: &[Node],
        cell: CellReferenceIndex,
        options: Options,
        arrays_allowed: bool,
    ) -> Result<Vec<Item>, CalcResult> {
        let mut items = Vec::new();
        let push_value = |items: &mut Vec<Item>, value: CalcResult| match value {
            CalcResult::Number(f) => items.push(Item::Number(f)),
            error @ CalcResult::Error { .. } => {
                if !options.skip_errors {
                    items.push(Item::Error(error));
                }
            }
            CalcResult::EmptyCell | CalcResult::EmptyArg => {}
            _ => items.push(Item::Other),
        };
        for arg in args {
            if options.skip_nested
                && matches!(
                    arg,
                    Node::FunctionKind {
                        kind: Function::Subtotal | Function::Aggregate,
                        ..
                    }
                )
            {
                continue;
            }
            match self.evaluate_node_with_reference(arg, cell) {
                CalcResult::Range { left, right } => {
                    if left.sheet != right.sheet {
                        return Err(CalcResult::new_error(
                            Error::VALUE,
                            cell,
                            "Ranges are in different sheets".to_string(),
                        ));
                    }
                    let hidden = if options.skip_hidden {
                        self.hidden_rows(left.sheet)
                    } else {
                        HashSet::new()
                    };
                    let last_row = self.used_last_row(left, right);
                    for row in left.row..=last_row {
                        if hidden.contains(&row) {
                            continue;
                        }
                        for column in left.column..=right.column {
                            if options.skip_nested
                                && self.cell_has_subtotal(left.sheet, row, column)
                            {
                                continue;
                            }
                            let value = self.evaluate_cell(CellReferenceIndex {
                                sheet: left.sheet,
                                row,
                                column,
                            });
                            push_value(&mut items, value);
                        }
                    }
                }
                CalcResult::Array(array) => {
                    if !arrays_allowed {
                        return Err(CalcResult::new_error(
                            Error::VALUE,
                            cell,
                            "AGGREGATE functions 1 to 13 need references".to_string(),
                        ));
                    }
                    for value in array.into_iter().flatten() {
                        let value = match value {
                            ArrayNode::Number(f) => CalcResult::Number(f),
                            ArrayNode::Error(error) => {
                                CalcResult::new_error(error, cell, String::new())
                            }
                            ArrayNode::Empty => CalcResult::EmptyCell,
                            _ => CalcResult::Boolean(false),
                        };
                        push_value(&mut items, value);
                    }
                }
                CalcResult::Lambda(_) => {
                    return Err(CalcResult::new_error(
                        Error::VALUE,
                        cell,
                        "Unexpected LAMBDA".to_string(),
                    ))
                }
                CalcResult::String(text) => match self.cast_number(&text) {
                    Some(f) => items.push(Item::Number(f)),
                    None => push_value(
                        &mut items,
                        CalcResult::new_error(Error::VALUE, cell, "Not a number".to_string()),
                    ),
                },
                CalcResult::Boolean(b) => items.push(Item::Number(if b { 1.0 } else { 0.0 })),
                value => push_value(&mut items, value),
            }
        }
        Ok(items)
    }

    pub(crate) fn fn_aggregate(&mut self, args: &[Node], cell: CellReferenceIndex) -> CalcResult {
        if args.len() < 3 {
            return CalcResult::new_args_number_error(cell);
        }
        let function = match self.get_number(&args[0], cell) {
            Ok(f) => f.trunc(),
            Err(e) => return e,
        };
        if !(1.0..=19.0).contains(&function) {
            return CalcResult::new_error(Error::VALUE, cell, "Invalid function_num".to_string());
        }
        let function = function as u8;
        let option = if matches!(args[1], Node::EmptyArgKind) {
            0.0
        } else {
            match self.get_number(&args[1], cell) {
                Ok(f) => f.trunc(),
                Err(e) => return e,
            }
        };
        if !(0.0..=7.0).contains(&option) {
            return CalcResult::new_error(Error::VALUE, cell, "Invalid options".to_string());
        }
        let option = option as u8;
        let options = Options {
            skip_nested: option <= 3,
            skip_hidden: matches!(option, 1 | 3 | 5 | 7),
            skip_errors: matches!(option, 2 | 3 | 6 | 7),
        };
        if function >= 14 {
            if args.len() != 4 {
                return CalcResult::new_error(
                    Error::VALUE,
                    cell,
                    "AGGREGATE functions 14 to 19 need an array and k".to_string(),
                );
            }
            let k = match self.get_number(&args[3], cell) {
                Ok(f) => f,
                Err(e) => return e,
            };
            let items = match self.aggregate_items(&args[2..3], cell, options, true) {
                Ok(items) => items,
                Err(e) => return e,
            };
            let mut values = match numbers(items) {
                Ok(v) => v,
                Err(e) => return e,
            };
            return aggregate_k(function, &mut values, k, cell);
        }
        let items = match self.aggregate_items(&args[2..], cell, options, false) {
            Ok(items) => items,
            Err(e) => return e,
        };
        match function {
            2 => {
                let count = items
                    .iter()
                    .filter(|i| matches!(i, Item::Number(_)))
                    .count();
                CalcResult::Number(count as f64)
            }
            3 => CalcResult::Number(items.len() as f64),
            _ => match numbers(items) {
                Ok(mut values) => aggregate_values(function, &mut values, cell),
                Err(e) => e,
            },
        }
    }
}

/// The numbers among `items`, or the first error.
fn numbers(items: Vec<Item>) -> Result<Vec<f64>, CalcResult> {
    let mut values = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Item::Number(f) => values.push(f),
            Item::Other => {}
            Item::Error(e) => return Err(e),
        }
    }
    Ok(values)
}

fn div0(cell: CellReferenceIndex) -> CalcResult {
    CalcResult::new_error(Error::DIV, cell, "Division by 0!".to_string())
}

fn num_error(cell: CellReferenceIndex, message: &str) -> CalcResult {
    CalcResult::new_error(Error::NUM, cell, message.to_string())
}

/// Sample (`ddof` 1) or population (`ddof` 0) variance.
fn variance(values: &[f64], ddof: f64) -> Option<f64> {
    let n = values.len() as f64;
    if n - ddof <= 0.0 {
        return None;
    }
    let mean = values.iter().sum::<f64>() / n;
    Some(values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - ddof))
}

/// Functions 1 and 4–13 of AGGREGATE (and GROUPBY's own aggregations).
pub(crate) fn aggregate_values(
    function: u8,
    values: &mut [f64],
    cell: CellReferenceIndex,
) -> CalcResult {
    let n = values.len();
    match function {
        1 => {
            if n == 0 {
                return div0(cell);
            }
            CalcResult::Number(values.iter().sum::<f64>() / n as f64)
        }
        2 => CalcResult::Number(n as f64),
        // MAX and MIN of no numbers are 0.
        4 => CalcResult::Number(values.iter().copied().reduce(f64::max).unwrap_or(0.0)),
        5 => CalcResult::Number(values.iter().copied().reduce(f64::min).unwrap_or(0.0)),
        6 => {
            if n == 0 {
                return CalcResult::Number(0.0);
            }
            CalcResult::Number(values.iter().product())
        }
        7 | 8 | 10 | 11 => {
            let ddof = if matches!(function, 7 | 10) { 1.0 } else { 0.0 };
            match variance(values, ddof) {
                Some(v) if function <= 8 => CalcResult::Number(v.sqrt()),
                Some(v) => CalcResult::Number(v),
                None => div0(cell),
            }
        }
        9 => CalcResult::Number(values.iter().sum()),
        12 => {
            if n == 0 {
                return num_error(cell, "No numbers");
            }
            values.sort_by(f64::total_cmp);
            let median = if n % 2 == 1 {
                values[n / 2]
            } else {
                (values[n / 2 - 1] + values[n / 2]) / 2.0
            };
            CalcResult::Number(median)
        }
        13 => {
            // The first value, in order, of those that appear most often.
            let mut counts: HashMap<u64, usize> = HashMap::new();
            for v in values.iter() {
                *counts.entry(v.to_bits()).or_default() += 1;
            }
            let mut best: Option<(f64, usize)> = None;
            for v in values.iter() {
                let c = counts[&v.to_bits()];
                if c > 1 && best.is_none_or(|(_, b)| c > b) {
                    best = Some((*v, c));
                }
            }
            match best {
                Some((v, _)) => CalcResult::Number(v),
                None => CalcResult::new_error(Error::NA, cell, "No value repeats".to_string()),
            }
        }
        _ => CalcResult::new_error(Error::VALUE, cell, "Invalid function_num".to_string()),
    }
}

/// Functions 14–19: LARGE, SMALL, PERCENTILE.INC, QUARTILE.INC,
/// PERCENTILE.EXC, QUARTILE.EXC.
fn aggregate_k(function: u8, values: &mut [f64], k: f64, cell: CellReferenceIndex) -> CalcResult {
    let n = values.len();
    if n == 0 {
        return num_error(cell, "No numbers");
    }
    values.sort_by(f64::total_cmp);
    match function {
        14 | 15 => {
            let k = k.trunc();
            if k < 1.0 || k > n as f64 {
                return num_error(cell, "k out of range");
            }
            let k = k as usize;
            CalcResult::Number(if function == 14 {
                values[n - k]
            } else {
                values[k - 1]
            })
        }
        16 | 17 => {
            let p = if function == 17 {
                let q = k.trunc();
                if !(0.0..=4.0).contains(&q) {
                    return num_error(cell, "quart out of range");
                }
                q / 4.0
            } else {
                k
            };
            if !(0.0..=1.0).contains(&p) {
                return num_error(cell, "k out of range");
            }
            CalcResult::Number(interpolate(values, p * (n - 1) as f64))
        }
        18 | 19 => {
            let p = if function == 19 {
                let q = k.trunc();
                if !(1.0..=3.0).contains(&q) {
                    return num_error(cell, "quart out of range");
                }
                q / 4.0
            } else {
                k
            };
            let rank = p * (n + 1) as f64 - 1.0;
            if p <= 0.0 || p >= 1.0 || rank < 0.0 || rank > (n - 1) as f64 {
                return num_error(cell, "k out of range");
            }
            CalcResult::Number(interpolate(values, rank))
        }
        _ => CalcResult::new_error(Error::VALUE, cell, "Invalid function_num".to_string()),
    }
}

/// The value at 0-based fractional `rank` of sorted `values`.
fn interpolate(values: &[f64], rank: f64) -> f64 {
    let low = rank.floor() as usize;
    let fraction = rank - rank.floor();
    match values.get(low + 1) {
        Some(next) if fraction > 0.0 => values[low] + fraction * (next - values[low]),
        _ => values[low],
    }
}
