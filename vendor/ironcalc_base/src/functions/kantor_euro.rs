//! KANTOR PATCH (functions): EUROCONVERT.
//!
//! `EUROCONVERT(number, source, target, [full_precision],
//! [triangulation_precision])` converts between the euro and the currencies
//! it replaced, at the fixed rates of the EU Council, with each currency's
//! rounding (Microsoft's table). Between two national currencies the amount
//! goes through the euro ("triangulation"); `triangulation_precision` (3 or
//! more) rounds that euro amount to so many significant digits.

use crate::{
    calc_result::CalcResult,
    expressions::{parser::Node, token::Error, types::CellReferenceIndex},
    functions::math_and_trigonometry::mathematical::{round_to_digits, Rounding},
    model::Model,
    number_format::to_precision,
};

/// ISO code, units per euro, calculation precision (decimals).
const CURRENCIES: [(&str, f64, f64); 14] = [
    ("BEF", 40.3399, 0.0),
    ("LUF", 40.3399, 0.0),
    ("DEM", 1.95583, 2.0),
    ("ESP", 166.386, 0.0),
    ("FRF", 6.55957, 2.0),
    ("IEP", 0.787564, 2.0),
    ("ITL", 1936.27, 0.0),
    ("NLG", 2.20371, 2.0),
    ("ATS", 13.7603, 2.0),
    ("PTE", 200.482, 0.0),
    ("FIM", 5.94573, 2.0),
    ("GRD", 340.75, 0.0),
    ("SIT", 239.64, 2.0),
    ("EUR", 1.0, 2.0),
];

fn currency(code: &str) -> Option<(f64, f64)> {
    let code = code.trim().to_uppercase();
    CURRENCIES
        .iter()
        .find(|(c, _, _)| *c == code)
        .map(|(_, rate, precision)| (*rate, *precision))
}

/// The conversion on plain values: `None` for a code that isn't a currency
/// or a triangulation precision below 3.
pub(crate) fn euroconvert(
    number: f64,
    source: &str,
    target: &str,
    full_precision: bool,
    triangulation: Option<f64>,
) -> Option<f64> {
    let (from_rate, _) = currency(source)?;
    let (to_rate, to_precision_digits) = currency(target)?;
    let triangulation = match triangulation {
        Some(t) if t < 3.0 => return None,
        Some(t) => Some(t.trunc() as usize),
        None => None,
    };
    if source.trim().eq_ignore_ascii_case(target.trim()) {
        return Some(number);
    }
    let mut euros = number / from_rate;
    if let Some(digits) = triangulation {
        if source.trim().to_uppercase() != "EUR" {
            euros = to_precision(euros, digits);
        }
    }
    let result = euros * to_rate;
    if full_precision {
        return Some(result);
    }
    round_to_digits(
        to_precision(result, 15),
        to_precision_digits,
        Rounding::Nearest,
    )
}

impl Model<'_> {
    pub(crate) fn fn_euroconvert(&mut self, args: &[Node], cell: CellReferenceIndex) -> CalcResult {
        if args.len() < 3 || args.len() > 5 {
            return CalcResult::new_args_number_error(cell);
        }
        let number = match self.get_number(&args[0], cell) {
            Ok(f) => f,
            Err(e) => return e,
        };
        let source = match self.get_string(&args[1], cell) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let target = match self.get_string(&args[2], cell) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let full_precision = match args.get(3) {
            None | Some(Node::EmptyArgKind) => false,
            Some(node) => match self.get_boolean(node, cell) {
                Ok(b) => b,
                Err(e) => return e,
            },
        };
        let triangulation = match args.get(4) {
            None | Some(Node::EmptyArgKind) => None,
            Some(node) => match self.get_number(node, cell) {
                Ok(f) => Some(f),
                Err(e) => return e,
            },
        };
        match euroconvert(number, &source, &target, full_precision, triangulation) {
            Some(v) => CalcResult::Number(v),
            None => CalcResult::new_error(
                Error::VALUE,
                cell,
                "Invalid currency or triangulation precision".to_string(),
            ),
        }
    }
}
