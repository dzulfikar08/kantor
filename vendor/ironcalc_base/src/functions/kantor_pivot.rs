//! KANTOR PATCH (pivot): GETPIVOTDATA.
//!
//! The model knows nothing of pivot tables: their cells are ordinary cells.
//! The caller (Kantor's `sheets_engine::pivot`) describes each pivot table's
//! value cells in [`crate::Model::kantor_pivots`] — which data field a cell
//! shows and for which field/item pairs — and keeps that list up to date when
//! pivot tables move or are refreshed. `GETPIVOTDATA(data_field, pivot_table,
//! [field1, item1], ...)` finds the pivot table `pivot_table` points into, then
//! the value cell whose data field and pairs are exactly the ones given
//! (fields and items in any order, case-insensitive), and returns that cell's
//! value. Nothing matches: `#REF!`, as in Excel.

use crate::{
    calc_result::CalcResult, expressions::parser::Node, expressions::token::Error,
    expressions::types::CellReferenceIndex, model::Model,
};

/// A value cell of a pivot table.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PivotValueCell {
    /// The names its data field answers to, lower case (its caption, e.g.
    /// `sum of revenue`, and the source field, `revenue`).
    pub data: Vec<String>,
    /// `(field, item)` pairs it is for, lower case, sorted. Empty for the
    /// grand total.
    pub items: Vec<(String, String)>,
    /// 1-based.
    pub row: i32,
    pub column: i32,
}

/// One pivot table: where it is (1-based, inclusive) and its value cells.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PivotGrid {
    pub sheet: u32,
    pub top: i32,
    pub left: i32,
    pub bottom: i32,
    pub right: i32,
    pub cells: Vec<PivotValueCell>,
}

impl Model<'_> {
    /// `=GETPIVOTDATA(data_field, pivot_table, [field1, item1], ...)`.
    pub(crate) fn fn_getpivotdata(
        &mut self,
        args: &[Node],
        cell: CellReferenceIndex,
    ) -> CalcResult {
        if args.len() < 2 || !args.len().is_multiple_of(2) {
            return CalcResult::new_args_number_error(cell);
        }
        let data = match self.get_string(&args[0], cell) {
            Ok(s) => s.trim().to_lowercase(),
            Err(e) => return e,
        };
        let at = match self.get_reference(&args[1], cell) {
            Ok(r) => r.left,
            Err(e) => return e,
        };
        let mut pairs = Vec::with_capacity(args.len() / 2 - 1);
        for pair in args[2..].chunks(2) {
            let field = match self.get_string(&pair[0], cell) {
                Ok(s) => s.trim().to_lowercase(),
                Err(e) => return e,
            };
            let item = match self.get_string(&pair[1], cell) {
                Ok(s) => s.trim().to_lowercase(),
                Err(e) => return e,
            };
            pairs.push((field, item));
        }
        pairs.sort();
        let not_found =
            |message: &str| CalcResult::new_error(Error::REF, cell, message.to_string());
        let Some(grid) = self.kantor_pivots.iter().find(|g| {
            g.sheet == at.sheet
                && (g.top..=g.bottom).contains(&at.row)
                && (g.left..=g.right).contains(&at.column)
        }) else {
            return not_found("Not a reference to a PivotTable");
        };
        let found = grid
            .cells
            .iter()
            .find(|c| c.items == pairs && c.data.contains(&data))
            .map(|c| (c.row, c.column));
        let Some((row, column)) = found else {
            return not_found("No such value in the PivotTable");
        };
        let target = CellReferenceIndex {
            sheet: grid.sheet,
            row,
            column,
        };
        match self.evaluate_cell(target) {
            CalcResult::EmptyCell | CalcResult::EmptyArg => CalcResult::Number(0.0),
            r => r,
        }
    }
}
