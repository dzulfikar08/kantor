//! KANTOR PATCH (3-D references): `SUM(Jan:Mar!B2)`, `'Q 1:Q 4'!A1:B2`.
//! See KANTOR_PATCHES.md §29.
//!
//! A 3-D reference names the same cell or range on every sheet from one
//! sheet to another, by their order in the workbook. As in Excel:
//!
//! * only the functions that take one (SUM, AVERAGE, COUNT, MAX, …) read
//!   it, once per sheet; anywhere else it's `#VALUE!`;
//! * the sheets are kept by name, so a sheet inserted or moved between the
//!   two is counted and renaming either end renames it in the formula;
//! * deleting an end sheet moves that end one sheet inwards, deleting the
//!   only sheet left makes it `#REF!`;
//! * inserting or deleting rows and columns doesn't change it.

use crate::{
    calc_result::CalcResult,
    expressions::{
        parser::{
            stringify::{stringify_reference, DisplaceData},
            Node, Range3D, Reference,
        },
        token::Error,
        types::{CellReferenceIndex, CellReferenceRC},
        utils::quote_name,
    },
    functions::Function,
    model::Model,
};

use crate::constants::{LAST_COLUMN, LAST_ROW};

/// Whether `kind` reads 3-D references (Excel's list).
pub(crate) fn takes_3d(kind: &Function) -> bool {
    use Function::*;
    matches!(
        kind,
        Sum | Average
            | Averagea
            | Count
            | Counta
            | Max
            | MaxA
            | Min
            | MinA
            | Product
            | Stdev
            | StDevP
            | StDevS
            | Stdeva
            | Stdevpa
            | StDevPCompat
            | VarP
            | VarS
            | VarpA
            | VarA
            | VarCompat
            | VarPCompat
    )
}

/// The first and last sheet of `r` in workbook order.
fn span(r: &Range3D) -> (u32, u32) {
    (
        r.sheet_index1.min(r.sheet_index2),
        r.sheet_index1.max(r.sheet_index2),
    )
}

/// `Jan:Mar!B2`, quoted as one name when either needs it, as Excel writes
/// it. `context: None` gives the R1C1 (internal) form.
pub(crate) fn stringify_3d(r: &Range3D, context: Option<&CellReferenceRC>) -> String {
    let n1 = quote_name(&r.sheet_name1);
    let n2 = quote_name(&r.sheet_name2);
    let prefix = if n1 != r.sheet_name1 || n2 != r.sheet_name2 {
        format!(
            "'{}:{}'",
            r.sheet_name1.replace('\'', "''"),
            r.sheet_name2.replace('\'', "''")
        )
    } else {
        format!("{n1}:{n2}")
    };
    let full_row = !r.is_cell
        && r.absolute_row1
        && r.absolute_row2
        && r.row1 == 1
        && r.row2 == LAST_ROW;
    let full_column = !r.is_cell
        && r.absolute_column1
        && r.absolute_column2
        && r.column1 == 1
        && r.column2 == LAST_COLUMN;
    let end = |absolute_row, absolute_column, row, column| {
        stringify_reference(
            context,
            &DisplaceData::None,
            &Reference {
                sheet_name: &None,
                sheet_index: u32::MAX,
                absolute_row,
                absolute_column,
                row,
                column,
            },
            full_row,
            full_column,
        )
    };
    let s1 = end(r.absolute_row1, r.absolute_column1, r.row1, r.column1);
    if s1 == "#REF!" {
        return s1;
    }
    if r.is_cell {
        return format!("{prefix}!{s1}");
    }
    let s2 = end(r.absolute_row2, r.absolute_column2, r.row2, r.column2);
    if s2 == "#REF!" {
        return s2;
    }
    format!("{prefix}!{s1}:{s2}")
}

/// One range per sheet of `r`, as a function reads them.
fn per_sheet(r: &Range3D, sheet_names: &[String]) -> Vec<Node> {
    let (first, last) = span(r);
    (first..=last)
        .map(|sheet_index| Node::RangeKind {
            sheet_name: sheet_names.get(sheet_index as usize).cloned(),
            sheet_index,
            absolute_row1: r.absolute_row1,
            absolute_column1: r.absolute_column1,
            row1: r.row1,
            column1: r.column1,
            absolute_row2: r.absolute_row2,
            absolute_column2: r.absolute_column2,
            row2: r.row2,
            column2: r.column2,
        })
        .collect()
}

/// The sheets `r` reads (for partial recalculation).
pub(crate) fn sheets_of(r: &Range3D) -> std::ops::RangeInclusive<u32> {
    let (first, last) = span(r);
    first..=last
}

/// After a rename of sheet `index` to `new_name`.
pub(crate) fn rename_3d(r: &mut Range3D, index: u32, new_name: &str) {
    if r.sheet_index1 == index {
        r.sheet_name1 = new_name.to_string();
    }
    if r.sheet_index2 == index {
        r.sheet_name2 = new_name.to_string();
    }
}

/// Before sheet `index` is deleted (`names`: the sheets before): an end on
/// it moves one sheet inwards. `None`: nothing is left (`#REF!`); `Some(changed)`.
pub(crate) fn cut_3d(r: &mut Range3D, index: u32, names: &[String]) -> Option<bool> {
    let (first, last) = span(r);
    if first == last {
        return if first == index { None } else { Some(false) };
    }
    let mut changed = false;
    let inwards = |at: u32, towards: u32| if at < towards { at + 1 } else { at - 1 };
    if r.sheet_index1 == index {
        let to = inwards(r.sheet_index1, r.sheet_index2);
        r.sheet_name1 = names.get(to as usize).cloned().unwrap_or_default();
        r.sheet_index1 = to;
        changed = true;
    } else if r.sheet_index2 == index {
        let to = inwards(r.sheet_index2, r.sheet_index1);
        r.sheet_name2 = names.get(to as usize).cloned().unwrap_or_default();
        r.sheet_index2 = to;
        changed = true;
    }
    Some(changed)
}

impl Model<'_> {
    /// The arguments of `kind` with each 3-D reference spread over its
    /// sheets; `None` if there is none.
    pub(crate) fn kantor_spread_3d(&self, kind: &Function, args: &[Node]) -> Option<Vec<Node>> {
        if !args.iter().any(|a| matches!(a, Node::Range3DKind(_))) || !takes_3d(kind) {
            return None;
        }
        let names = self.workbook.get_worksheet_names();
        let mut out = Vec::with_capacity(args.len());
        for a in args {
            match a {
                Node::Range3DKind(r) => out.extend(per_sheet(r, &names)),
                other => out.push(other.clone()),
            }
        }
        Some(out)
    }

    /// A 3-D reference anywhere else (Excel: `#VALUE!`).
    pub(crate) fn kantor_3d_elsewhere(&self, cell: CellReferenceIndex) -> CalcResult {
        CalcResult::new_error(
            Error::VALUE,
            cell,
            "A 3-D reference can only be used in functions such as SUM".to_string(),
        )
    }
}
