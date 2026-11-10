//! KANTOR PATCH: a cell of a row of packed cells, with its column, in 32
//! bytes instead of the 56 of a `(column, Cell)`.
//!
//! A `Cell` is as large as its largest kind, the anchor of an array formula,
//! and a row of cells pays that for every cell. Sheets of data are mostly
//! numbers, shared strings and plain formulas with their value, which need
//! far less. A row holding nothing else keeps them like this; a row with an
//! array formula or a spill cell in it stays a row of cells.

use crate::expressions::token::Error;
use crate::types::{Cell, ErrorDetails, FormulaValue};

/// A cell of a row of packed cells and its column (`c`). Its style is `s`.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Packed {
    Empty {
        c: i32,
        s: i32,
    },
    Boolean {
        c: i32,
        s: i32,
        v: bool,
    },
    Number {
        c: i32,
        s: i32,
        v: f64,
    },
    Error {
        c: i32,
        s: i32,
        ei: Error,
    },
    SharedString {
        c: i32,
        s: i32,
        si: i32,
    },
    /// A formula (`Cell::CellFormula`) whose value is no value yet.
    Unevaluated {
        c: i32,
        s: i32,
        f: i32,
    },
    FormulaBoolean {
        c: i32,
        s: i32,
        f: i32,
        v: bool,
    },
    FormulaNumber {
        c: i32,
        s: i32,
        f: i32,
        v: f64,
    },
    /// The text without the spare room a `String` may have.
    FormulaText {
        c: i32,
        s: i32,
        f: i32,
        v: Box<str>,
    },
    FormulaError {
        c: i32,
        s: i32,
        f: i32,
        ei: Error,
        d: Box<ErrorDetails>,
    },
}

impl Packed {
    /// The cell packed, or the cell back if it cannot be.
    pub(super) fn pack(c: i32, cell: Cell) -> Result<Packed, Cell> {
        Ok(match cell {
            Cell::EmptyCell { s } => Packed::Empty { c, s },
            Cell::BooleanCell { v, s } => Packed::Boolean { c, s, v },
            Cell::NumberCell { v, s } => Packed::Number { c, s, v },
            Cell::ErrorCell { ei, s } => Packed::Error { c, s, ei },
            Cell::SharedString { si, s } => Packed::SharedString { c, s, si },
            Cell::CellFormula { f, s, v } => match v {
                FormulaValue::Unevaluated => Packed::Unevaluated { c, s, f },
                FormulaValue::Boolean(v) => Packed::FormulaBoolean { c, s, f, v },
                FormulaValue::Number(v) => Packed::FormulaNumber { c, s, f, v },
                FormulaValue::Text(v) => Packed::FormulaText {
                    c,
                    s,
                    f,
                    v: v.into_boxed_str(),
                },
                FormulaValue::Error { ei, d } => Packed::FormulaError { c, s, f, ei, d },
            },
            Cell::ArrayFormula { .. } | Cell::SpillCell { .. } => return Err(cell),
        })
    }

    /// Whether the cell is a formula.
    pub(super) fn is_formula(&self) -> bool {
        matches!(
            self,
            Packed::Unevaluated { .. }
                | Packed::FormulaBoolean { .. }
                | Packed::FormulaNumber { .. }
                | Packed::FormulaText { .. }
                | Packed::FormulaError { .. }
        )
    }

    /// Whether [`Packed::pack`] takes the cell.
    pub(super) fn packs(cell: &Cell) -> bool {
        !matches!(cell, Cell::ArrayFormula { .. } | Cell::SpillCell { .. })
    }

    /// The cell this is.
    pub(super) fn cell(&self) -> Cell {
        match self {
            Packed::Empty { s, .. } => Cell::EmptyCell { s: *s },
            Packed::Boolean { s, v, .. } => Cell::BooleanCell { v: *v, s: *s },
            Packed::Number { s, v, .. } => Cell::NumberCell { v: *v, s: *s },
            Packed::Error { s, ei, .. } => Cell::ErrorCell {
                ei: ei.clone(),
                s: *s,
            },
            Packed::SharedString { s, si, .. } => Cell::SharedString { si: *si, s: *s },
            Packed::Unevaluated { s, f, .. } => formula(*f, *s, FormulaValue::Unevaluated),
            Packed::FormulaBoolean { s, f, v, .. } => formula(*f, *s, FormulaValue::Boolean(*v)),
            Packed::FormulaNumber { s, f, v, .. } => formula(*f, *s, FormulaValue::Number(*v)),
            Packed::FormulaText { s, f, v, .. } => {
                formula(*f, *s, FormulaValue::Text(v.to_string()))
            }
            Packed::FormulaError { s, f, ei, d, .. } => formula(
                *f,
                *s,
                FormulaValue::Error {
                    ei: ei.clone(),
                    d: d.clone(),
                },
            ),
        }
    }

    /// The cell this is, taking it apart.
    pub(super) fn into_cell(self) -> Cell {
        match self {
            Packed::FormulaText { s, f, v, .. } => formula(f, s, FormulaValue::Text(v.into())),
            Packed::FormulaError { s, f, ei, d, .. } => {
                formula(f, s, FormulaValue::Error { ei, d })
            }
            other => other.cell(),
        }
    }

    #[inline]
    pub(super) fn column(&self) -> i32 {
        match self {
            Packed::Empty { c, .. }
            | Packed::Boolean { c, .. }
            | Packed::Number { c, .. }
            | Packed::Error { c, .. }
            | Packed::SharedString { c, .. }
            | Packed::Unevaluated { c, .. }
            | Packed::FormulaBoolean { c, .. }
            | Packed::FormulaNumber { c, .. }
            | Packed::FormulaText { c, .. }
            | Packed::FormulaError { c, .. } => *c,
        }
    }

    pub(super) fn set_style(&mut self, style: i32) {
        match self {
            Packed::Empty { s, .. }
            | Packed::Boolean { s, .. }
            | Packed::Number { s, .. }
            | Packed::Error { s, .. }
            | Packed::SharedString { s, .. }
            | Packed::Unevaluated { s, .. }
            | Packed::FormulaBoolean { s, .. }
            | Packed::FormulaNumber { s, .. }
            | Packed::FormulaText { s, .. }
            | Packed::FormulaError { s, .. } => *s = style,
        }
    }
}

fn formula(f: i32, s: i32, v: FormulaValue) -> Cell {
    Cell::CellFormula { f, s, v }
}
