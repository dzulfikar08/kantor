//! KANTOR PATCH (partial recalculation): after an edit, evaluate only the
//! formulas whose value may have changed, the edited cells' dependents, and
//! read every other formula's stored value. See KANTOR_PATCHES.md §19.
//!
//! The dependents are found from the parsed formulas, not from a graph kept
//! across edits: a sheet's formula cells are grouped into runs (consecutive
//! rows of one column with the same shared formula, which is what a filled-
//! down column is), and each run's references are translated over the run
//! at once. Starting from the changed rectangles, every run whose references
//! meet a changed rectangle has the rows that read it marked, and those rows
//! are changed rectangles in turn, until nothing new is marked.
//!
//! Whenever the answer could be wrong this gives up and the caller
//! evaluates everything (`Model::evaluate`): dynamic arrays and array
//! formulas (spills move), dynamic links (HYPERLINK), changes that are not
//! cells (sheets, names, tables), and too much work. Formulas whose
//! references can't be read off the formula (OFFSET, INDIRECT, defined
//! names, tables, ranges built with `:` from functions...) and volatile ones
//! are evaluated every time, with their dependents.

use std::collections::{HashSet, VecDeque};

use crate::evaluation::{CellKey, KeyBuild};
use crate::expressions::parser::Node;
use crate::expressions::types::CellReferenceIndex;
use crate::functions::Function;
use crate::kantor_changes::KantorChanges;
use crate::model::Model;

/// Run-reference checks allowed for one partial evaluation before it gives
/// up and everything is evaluated (a full pass on the 90k-row test workbook
/// is ~1 s; this is a few tens of milliseconds).
const MAX_WORK: u64 = 30_000_000;

/// The formula cells a partial evaluation runs. Most reads in it are of
/// other cells (a SUMIFS reads a whole column), so each sheet's bounds are
/// checked before the set.
pub(crate) struct OnlyCells {
    cells: HashSet<CellKey, KeyBuild>,
    /// Per sheet: first row, first column, last row, last column.
    bounds: Vec<Option<(i32, i32, i32, i32)>>,
}

impl OnlyCells {
    fn new(cells: &[CellReferenceIndex]) -> Self {
        let mut bounds: Vec<Option<(i32, i32, i32, i32)>> = Vec::new();
        for c in cells {
            let s = c.sheet as usize;
            if bounds.len() <= s {
                bounds.resize(s + 1, None);
            }
            let b = bounds[s].get_or_insert((c.row, c.column, c.row, c.column));
            *b = (
                b.0.min(c.row),
                b.1.min(c.column),
                b.2.max(c.row),
                b.3.max(c.column),
            );
        }
        OnlyCells {
            cells: cells.iter().map(|c| (c.sheet, c.row, c.column)).collect(),
            bounds,
        }
    }

    #[inline]
    pub(crate) fn contains(&self, key: &CellKey) -> bool {
        let &(sheet, row, column) = key;
        match self.bounds.get(sheet as usize) {
            Some(Some((r1, c1, r2, c2))) => {
                (*r1..=*r2).contains(&row)
                    && (*c1..=*c2).contains(&column)
                    && self.cells.contains(key)
            }
            _ => false,
        }
    }
}

/// A rectangle of one sheet, 1-based and inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    sheet: u32,
    row1: i32,
    column1: i32,
    row2: i32,
    column2: i32,
}

/// One end of a reference: absolute, or an offset from the formula's cell.
#[derive(Clone, Copy, Debug)]
struct End {
    value: i32,
    absolute: bool,
}

impl End {
    fn at(self, origin: i32) -> i32 {
        if self.absolute {
            self.value
        } else {
            origin + self.value
        }
    }
}

/// A reference of a formula, as written (relative to its cell).
#[derive(Clone, Copy, Debug)]
struct Precedent {
    sheet: u32,
    row1: End,
    column1: End,
    row2: End,
    column2: End,
}

/// What a shared formula reads.
#[derive(Default)]
struct Reads {
    precedents: Vec<Precedent>,
    /// It reads what can't be told from the formula, or is volatile:
    /// evaluated every time.
    unknown: bool,
}

fn reads_of(node: &Node, reads: &mut Reads) {
    match node {
        Node::ReferenceKind {
            sheet_index,
            absolute_row,
            absolute_column,
            row,
            column,
            ..
        } => {
            let row = End {
                value: *row,
                absolute: *absolute_row,
            };
            let column = End {
                value: *column,
                absolute: *absolute_column,
            };
            reads.precedents.push(Precedent {
                sheet: *sheet_index,
                row1: row,
                column1: column,
                row2: row,
                column2: column,
            });
        }
        Node::RangeKind {
            sheet_index,
            absolute_row1,
            absolute_column1,
            row1,
            column1,
            absolute_row2,
            absolute_column2,
            row2,
            column2,
            ..
        } => reads.precedents.push(Precedent {
            sheet: *sheet_index,
            row1: End {
                value: *row1,
                absolute: *absolute_row1,
            },
            column1: End {
                value: *column1,
                absolute: *absolute_column1,
            },
            row2: End {
                value: *row2,
                absolute: *absolute_row2,
            },
            column2: End {
                value: *column2,
                absolute: *absolute_column2,
            },
        }),
        Node::BooleanKind(_)
        | Node::NumberKind(_)
        | Node::StringKind(_)
        | Node::ErrorKind(_)
        | Node::EmptyArgKind
        | Node::ArrayKind(_)
        | Node::WrongReferenceKind { .. }
        | Node::WrongRangeKind { .. }
        | Node::ParseErrorKind { .. }
        | Node::NamedVariableKind { .. } => {}
        Node::OpConcatenateKind { left, right }
        | Node::OpSumKind { left, right, .. }
        | Node::OpProductKind { left, right, .. }
        | Node::OpPowerKind { left, right }
        | Node::CompareKind { left, right, .. } => {
            reads_of(left, reads);
            reads_of(right, reads);
        }
        Node::UnaryKind { right, .. } => reads_of(right, reads),
        Node::ImplicitIntersection { child, .. } => reads_of(child, reads),
        Node::FunctionKind { kind, args } => {
            if matches!(
                kind,
                Function::Rand
                    | Function::Randbetween
                    | Function::Randarray
                    | Function::Now
                    | Function::Today
                    | Function::Offset
                    | Function::Indirect
                    | Function::Cell
                    | Function::Info
                    | Function::Sheets
                    | Function::Subtotal
                    | Function::Aggregate
                    | Function::Getpivotdata
                    | Function::Hyperlink
            ) {
                reads.unknown = true;
            }
            for arg in args {
                reads_of(arg, reads);
            }
        }
        Node::LambdaDefKind { body, .. } => reads_of(body, reads),
        Node::LambdaCallKind { lambda, args } => {
            reads_of(lambda, reads);
            for arg in args {
                reads_of(arg, reads);
            }
        }
        // A range between what functions return, names, tables, spills,
        // functions defined elsewhere: what they read isn't in the formula.
        Node::OpRangeKind { .. }
        | Node::NamedFunctionKind { .. }
        | Node::DefinedNameKind(_)
        | Node::TableNameKind(_)
        | Node::TableRefKind(_)
        | Node::SpillRangeOperator { .. } => reads.unknown = true,
    }
}

/// Consecutive rows of one column holding the same shared formula.
struct Run {
    sheet: u32,
    column: i32,
    first_row: i32,
    last_row: i32,
    /// Index into `Plan::reads` (one entry per sheet and shared formula).
    reads: usize,
    /// Rows marked to be evaluated, one bit each, from `first_row`.
    marked: Vec<u64>,
}

impl Run {
    /// Marks rows `from..=to` (clipped to the run) and returns the stretches
    /// that were not marked yet.
    fn mark(&mut self, from: i32, to: i32, out: &mut Vec<(i32, i32)>) {
        let from = from.max(self.first_row);
        let to = to.min(self.last_row);
        if from > to {
            return;
        }
        if self.marked.is_empty() {
            let len = (self.last_row - self.first_row + 1) as usize;
            self.marked = vec![0; len.div_ceil(64)];
        }
        let mut start = None;
        for row in from..=to {
            let i = (row - self.first_row) as usize;
            let (word, bit) = (i / 64, 1u64 << (i % 64));
            if self.marked[word] & bit == 0 {
                self.marked[word] |= bit;
                if start.is_none() {
                    start = Some(row);
                }
            } else if let Some(s) = start.take() {
                out.push((s, row - 1));
            }
        }
        if let Some(s) = start {
            out.push((s, to));
        }
    }

    fn is_marked(&self, row: i32) -> bool {
        let i = (row - self.first_row) as usize;
        self.marked
            .get(i / 64)
            .is_some_and(|word| word & (1u64 << (i % 64)) != 0)
    }
}

/// The rows of a run whose reference `p` meets `rect`, as an interval (maybe
/// empty).
fn rows_reading(run: &Run, p: &Precedent, rect: &Rect) -> (i32, i32) {
    const NONE: (i32, i32) = (1, 0);
    if p.sheet != rect.sheet {
        return NONE;
    }
    let (a, b) = (p.column1.at(run.column), p.column2.at(run.column));
    if a.min(b) > rect.column2 || a.max(b) < rect.column1 {
        return NONE;
    }
    let (x1, x2) = (rect.row1, rect.row2);
    let all = (run.first_row, run.last_row);
    match (p.row1.absolute, p.row2.absolute) {
        (true, true) => {
            let (lo, hi) = (
                p.row1.value.min(p.row2.value),
                p.row1.value.max(p.row2.value),
            );
            if lo <= x2 && hi >= x1 {
                all
            } else {
                NONE
            }
        }
        (false, false) => {
            let lo = p.row1.value.min(p.row2.value);
            let hi = p.row1.value.max(p.row2.value);
            // Row r reads r + lo ..= r + hi.
            (x1 - hi, x2 - lo)
        }
        (absolute_first, _) => {
            // One end fixed at `fixed`, the other at r + d.
            let (fixed, d) = if absolute_first {
                (p.row1.value, p.row2.value)
            } else {
                (p.row2.value, p.row1.value)
            };
            if (x1..=x2).contains(&fixed) {
                all
            } else if fixed < x1 {
                (x1 - d, i32::MAX)
            } else {
                (i32::MIN, x2 - d)
            }
        }
    }
}

impl Model<'_> {
    /// KANTOR PATCH (partial recalculation): evaluates what `changes` (made
    /// since the model was last evaluated) can have changed, or everything
    /// when that can't be told for sure. Returns whether it evaluated only
    /// part of the workbook.
    pub fn kantor_evaluate_changed(&mut self, changes: &KantorChanges) -> bool {
        if let Some(cells) = self.kantor_dirty_cells(changes) {
            let count = cells.len();
            if self.kantor_evaluate_only(cells) {
                self.kantor_last_partial = Some(count);
                return true;
            }
        }
        self.evaluate();
        false
    }

    /// The formula cells to evaluate after `changes`, in natural order, or
    /// `None` when everything should be.
    fn kantor_dirty_cells(&self, changes: &KantorChanges) -> Option<Vec<CellReferenceIndex>> {
        if changes.all || !changes.sheets.is_empty() {
            return None;
        }
        if !self.evaluation.anchor_order.is_empty() || !self.links.is_empty() {
            return None;
        }
        // Runs, built in natural order: a column's open run is extended by
        // the next row with the same formula.
        let mut reads: Vec<Reads> = Vec::new();
        let mut runs: Vec<Run> = Vec::new();
        let mut pending: Vec<(usize, i32)> = Vec::new();
        for (sheet, worksheet) in self.workbook.worksheets.iter().enumerate() {
            if worksheet.shared_formulas.is_empty() {
                continue;
            }
            let sheet = sheet as u32;
            let parsed = self.parsed_formulas.get(sheet as usize)?;
            // By column, the run that ended last in it; by formula index,
            // its entry in `reads` (`usize::MAX`: none yet).
            let mut open: Vec<usize> = Vec::new();
            let mut formula_reads: Vec<usize> = vec![usize::MAX; parsed.len()];
            let mut array = false;
            worksheet
                .sheet_data
                .kantor_for_each_formula(|row, column, formula| {
                    let Some((f, unevaluated)) = formula else {
                        array = true;
                        return;
                    };
                    let known = formula_reads.get(f as usize).copied().unwrap_or(usize::MAX);
                    let index = if known != usize::MAX {
                        known
                    } else {
                        let mut r = Reads::default();
                        match parsed.get(f as usize) {
                            Some((node, _)) => reads_of(node, &mut r),
                            None => r.unknown = true,
                        }
                        reads.push(r);
                        if let Some(slot) = formula_reads.get_mut(f as usize) {
                            *slot = reads.len() - 1;
                        }
                        reads.len() - 1
                    };
                    let column_index = column as usize;
                    if open.len() <= column_index {
                        open.resize(column_index + 1, usize::MAX);
                    }
                    let extends = Some(open[column_index]).filter(|&i| {
                        i != usize::MAX && {
                            let run = &runs[i];
                            run.reads == index && run.last_row + 1 == row
                        }
                    });
                    let i = match extends {
                        Some(i) => {
                            runs[i].last_row = row;
                            i
                        }
                        None => {
                            open[column_index] = runs.len();
                            runs.push(Run {
                                sheet,
                                column,
                                first_row: row,
                                last_row: row,
                                reads: index,
                                marked: Vec::new(),
                            });
                            runs.len() - 1
                        }
                    };
                    // A formula with no value yet (typed in manual mode).
                    if unevaluated {
                        pending.push((i, row));
                    }
                });
            if array {
                return None;
            }
        }
        // Runs by sheet they read, to look at only those for a rectangle.
        let sheet_count = self.workbook.worksheets.len();
        let mut readers: Vec<Vec<usize>> = vec![Vec::new(); sheet_count];
        for (i, run) in runs.iter().enumerate() {
            let mut sheets: Vec<u32> = reads[run.reads]
                .precedents
                .iter()
                .map(|p| p.sheet)
                .collect();
            sheets.sort_unstable();
            sheets.dedup();
            for s in sheets {
                if let Some(list) = readers.get_mut(s as usize) {
                    list.push(i);
                }
            }
        }

        let mut queue: VecDeque<Rect> = VecDeque::new();
        let mark = |run: &mut Run, from: i32, to: i32, queue: &mut VecDeque<Rect>| {
            let mut fresh = Vec::new();
            run.mark(from, to, &mut fresh);
            for (a, b) in fresh {
                queue.push_back(Rect {
                    sheet: run.sheet,
                    row1: a,
                    column1: run.column,
                    row2: b,
                    column2: run.column,
                });
            }
        };
        for (i, row) in pending {
            mark(&mut runs[i], row, row, &mut queue);
        }
        // What was edited, and the formulas there.
        let seeds = changes
            .cells
            .iter()
            .map(|&(sheet, row1, column1, row2, column2)| Rect {
                sheet,
                row1,
                column1,
                row2,
                column2,
            });
        for seed in seeds {
            queue.push_back(seed);
            for run in runs.iter_mut().filter(|r| {
                r.sheet == seed.sheet && (seed.column1..=seed.column2).contains(&r.column)
            }) {
                mark(run, seed.row1, seed.row2, &mut queue);
            }
        }
        // Formulas evaluated every time.
        for run in runs.iter_mut().filter(|r| reads[r.reads].unknown) {
            let (first, last) = (run.first_row, run.last_row);
            mark(run, first, last, &mut queue);
        }
        // Dependents, until there are no more.
        let mut work: u64 = 0;
        while let Some(rect) = queue.pop_front() {
            let Some(list) = readers.get(rect.sheet as usize) else {
                continue;
            };
            for &i in list {
                let run = &mut runs[i];
                for p in &reads[run.reads].precedents {
                    work += 1;
                    let (from, to) = rows_reading(run, p, &rect);
                    if from <= to {
                        mark(run, from, to, &mut queue);
                    }
                }
            }
            if work > MAX_WORK {
                return None;
            }
        }
        let mut cells: Vec<CellReferenceIndex> = Vec::new();
        for run in runs.iter().filter(|r| !r.marked.is_empty()) {
            for row in run.first_row..=run.last_row {
                if run.is_marked(row) {
                    cells.push(CellReferenceIndex {
                        sheet: run.sheet,
                        row,
                        column: run.column,
                    });
                }
            }
        }
        cells.sort_unstable_by_key(|c| (c.sheet, c.row, c.column));
        Some(cells)
    }

    /// Evaluates `cells` (formula cells, natural order); any other formula
    /// read gives its stored value. False if the pass had to be abandoned
    /// (then nothing can be trusted and everything must be evaluated).
    fn kantor_evaluate_only(&mut self, cells: Vec<CellReferenceIndex>) -> bool {
        self.kantor_evaluations += 1;
        let dirty = OnlyCells::new(&cells);
        let state = &mut self.evaluation;
        state.cells.clear();
        state.stack.clear();
        state.seen.clear();
        state.seen_ranges.clear();
        state.restart = None;
        state.depth = 0;
        state.unwinding = false;
        state.circular.clear();
        state.lookups.clear();
        state.cycle_reads = 0;
        state.record_reads = false;
        state.in_pass = true;
        state.kantor_only = Some(dirty);
        self.clear_variable_stack();
        self.clear_lambdas();
        let mut ok = true;
        for cell in cells {
            self.evaluation.root = Some(cell);
            self.evaluate_cell(cell);
            if self.evaluation.restart.is_some() {
                self.evaluation.restart = None;
                ok = false;
                break;
            }
        }
        let state = &mut self.evaluation;
        state.in_pass = false;
        state.root = None;
        state.kantor_only = None;
        state.lookups.clear();
        state.seen = Default::default();
        state.seen_ranges = Vec::new();
        // A formula may have started to spill: its cells are read by
        // formulas that were not evaluated. And which cells of a loop end up
        // `#CIRC!` depends on where the evaluation entered it, which a full
        // evaluation decides.
        if ok
            && (!self.evaluation.circular.is_empty()
                || self.evaluation.cycle_reads > 0
                || !self.dynamic_anchors_in_natural_order().is_empty())
        {
            ok = false;
        }
        if ok {
            self.evaluate_conditional_formatting();
        }
        ok
    }
}

#[cfg(test)]
mod tests;
