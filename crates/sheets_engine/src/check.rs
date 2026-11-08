//! Checking a formula before it goes into a cell (the AI formula assistant):
//! it must parse with IronCalc, and evaluating it in a scratch copy of the
//! workbook must not give `#NAME?` (unknown function) or `#ERROR!`.

use std::collections::HashMap;

use ironcalc_base::UserModel;
use ironcalc_base::expressions::parser::{Node, new_parser_english};
use ironcalc_base::expressions::types::CellReferenceRC;

use crate::{LANGUAGE, Workbook};

/// Result of evaluating a formula in a scratch copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormulaPreview {
    /// The value it shows, formatted as in the grid.
    pub value: String,
    /// Set when the value is a spreadsheet error worth mentioning
    /// (`#DIV/0!`, `#N/A`, `#REF!`…); the formula is still usable.
    pub warning: Option<String>,
}

impl Workbook {
    /// Parse `formula` (with its `=`) as if it were in `(row, col)` of `sheet`.
    pub fn parse_formula(
        &self,
        sheet: u32,
        row: u32,
        col: u32,
        formula: &str,
    ) -> Result<(), String> {
        let body = formula
            .trim()
            .strip_prefix('=')
            .ok_or("a formula starts with =")?;
        if body.trim().is_empty() {
            return Err("the formula is empty".into());
        }
        let names = self.sheet_names();
        let context = CellReferenceRC {
            sheet: names.get(sheet as usize).cloned().unwrap_or_default(),
            row: row as i32 + 1,
            column: col as i32 + 1,
        };
        let mut parser = new_parser_english(names, Vec::new(), HashMap::new());
        match parser.parse(body, &context) {
            Node::ParseErrorKind {
                message, position, ..
            } => Err(format!("{message} (at character {})", position + 1)),
            _ => Ok(()),
        }
    }

    /// The workbook as bytes, for [`evaluate_in_copy`] on another thread.
    pub fn snapshot(&self) -> Vec<u8> {
        self.model.to_bytes()
    }
}

/// Put `formula` in `(row, col)` of `sheet` in a copy of the workbook made
/// from [`Workbook::snapshot`], recalculate and return the value. The real
/// workbook is untouched. Can take a moment on big workbooks: run it off the
/// UI thread.
pub fn evaluate_in_copy(
    snapshot: &[u8],
    sheet: u32,
    row: u32,
    col: u32,
    formula: &str,
) -> Result<FormulaPreview, String> {
    let mut model = UserModel::from_bytes(snapshot, LANGUAGE)?;
    let (r, c) = (row as i32 + 1, col as i32 + 1);
    model.set_user_input(sheet, r, c, formula)?;
    model.evaluate();
    let value = model.get_formatted_cell_value(sheet, r, c)?;
    let warning = match value.as_str() {
        "#NAME?" => return Err("it uses a function or name Kantor doesn't know (#NAME?)".into()),
        "#ERROR!" => return Err("it can't be calculated (#ERROR!)".into()),
        "#CIRC!" => return Err("it refers to its own cell (circular reference)".into()),
        v if v.starts_with('#') => Some(format!("Gives {v} with the current data")),
        _ => None,
    };
    Ok(FormulaPreview { value, warning })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> Workbook {
        let mut b = Workbook::new_empty().unwrap();
        b.set_input(0, 0, 0, "Region").unwrap();
        b.set_input(0, 0, 1, "Sales").unwrap();
        b.set_input(0, 1, 0, "Jakarta").unwrap();
        b.set_input(0, 1, 1, "100").unwrap();
        b.set_input(0, 2, 0, "Bandung").unwrap();
        b.set_input(0, 2, 1, "50").unwrap();
        b.set_input(0, 3, 0, "Jakarta").unwrap();
        b.set_input(0, 3, 1, "25").unwrap();
        b
    }

    #[test]
    fn parses_good_and_rejects_broken_formulas() {
        let b = book();
        assert!(
            b.parse_formula(0, 0, 3, "=SUMIF(A2:A4,\"Jakarta\",B2:B4)")
                .is_ok()
        );
        assert!(b.parse_formula(0, 0, 3, "=SUM(A1:A3").is_err());
        assert!(b.parse_formula(0, 0, 3, "SUM(A1)").is_err());
        assert!(b.parse_formula(0, 0, 3, "=").is_err());
    }

    #[test]
    fn evaluates_in_a_copy_only() {
        let b = book();
        let snap = b.snapshot();
        let p = evaluate_in_copy(&snap, 0, 0, 3, "=SUMIF(A2:A4,\"Jakarta\",B2:B4)").unwrap();
        assert_eq!(p.value, "125");
        assert_eq!(p.warning, None);
        assert_eq!(b.input(0, 0, 3), "", "real workbook untouched");
        assert!(!b.can_redo());

        let p = evaluate_in_copy(&snap, 0, 0, 3, "=1/0").unwrap();
        assert!(p.warning.is_some());
        assert!(evaluate_in_copy(&snap, 0, 0, 3, "=NOSUCHFUNCTION(A1)").is_err());
        assert!(evaluate_in_copy(&snap, 0, 0, 3, "=D1+1").is_err());
    }
}
