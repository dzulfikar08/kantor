//! KANTOR PATCH: fast lookups.
//!
//! VLOOKUP, HLOOKUP, MATCH, LOOKUP, XLOOKUP and XMATCH used to read their
//! whole search vector, cell by cell, on every call, and then scan it from the
//! start. A sheet with a lookup per row over another sheet's column did that
//! work once per row: quadratic. Over a whole column (`A:B`) it was worse, as
//! VLOOKUP and MATCH walked all 1,048,576 rows.
//!
//! Here a search vector is read once per evaluation pass and kept, keyed by its
//! range. Only the part of the range inside the sheet's used area is read; the
//! rest is known to be empty and stays virtual (`SearchVector::len`), so every
//! search sees exactly the vector the old code built. Exact-match searches use
//! a hash index built on first use, with the same notion of equality as the
//! linear scans they replace (see `ExactMode`). Lookup values the index cannot
//! answer (text with wildcards, small numbers, booleans, blanks, errors) fall
//! back to scanning, which `linear_find` also implements as the reference the
//! tests compare against.
//!
//! When a vector may be reused, so that results stay the same:
//!
//! * Only during a pass (`Evaluation::in_pass`). The cache is emptied when a
//!   pass starts and when it ends, so nothing outlives an evaluation, and no
//!   edit to the workbook can happen while it is in use.
//! * Only if building it read nothing that can change later in the pass: no
//!   cell found on a cycle, no pending restart, no unwind (see `Model::search_vector`).
//!   The formula cells it read are evaluated and their values are final for
//!   the pass.
//! * Not if a cell being evaluated right now (on the evaluation stack) lies in
//!   the range: reading it again would close a cycle, and the old code
//!   reported `#CIRC!` there.
//! * A dynamic array that writes or removes spill cells drops every vector of
//!   its sheet; a CSE array drops the vectors its area overlaps.
//!
//! Without the cache, a later reader would also put its own reads on record
//! (`Evaluation::seen`); with it, only the reader that built the vector does.
//! A spill that contradicts those reads restarts the pass all the same, and the
//! pass that completes is unchanged: the vector was built after every spill
//! into it, or a restart would have followed.
//!
//! The used area of a sheet (`Worksheet::dimension`, a walk over every cell)
//! is kept for the pass in the same way, for `clip_to_used_area` and SUMIFS.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use crate::calc_result::CalcResult;
use crate::evaluation::{CellState, FormulaCell, KeyBuild};
use crate::expressions::types::CellReferenceIndex;
use crate::model::Model;
use crate::number_format::to_excel_precision;
use crate::types::Cell;
use crate::worksheet::WorksheetDimension;

use super::util::{compare_values, from_wildcard_to_regex, result_matches_regex, values_are_equal};

static EMPTY: CalcResult = CalcResult::EmptyCell;

/// `(sheet, row1, column1, row2, column2)` of a search vector: one row or one
/// column.
pub(crate) type VectorKey = (u32, i32, i32, i32, i32);

/// How an exact-match search decides that an element matches.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ExactMode {
    /// VLOOKUP, HLOOKUP and MATCH: text is matched as a wildcard pattern,
    /// case-insensitively (`from_wildcard_to_regex` of the lowercase text,
    /// against the lowercase element); anything else with `compare_values`
    /// (VLOOKUP, HLOOKUP) or `values_are_equal` (MATCH).
    Wildcard { compare: bool },
    /// XLOOKUP and XMATCH exact match: `compare_values(element, lookup) == 0`.
    Compare,
}

/// A hashable stand-in for a value, such that two values have the same key if
/// and only if they are equal, within a group of values where that holds.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Key {
    Text(String),
    Number(u64),
}

/// Values with a key go into `keyed`, the first and last position of each key;
/// the rest are listed in `others`, in order, and are scanned. The key
/// functions are chosen so that a value with a key never equals a value
/// without one.
#[derive(Default, Debug)]
struct KeyIndex {
    keyed: HashMap<Key, (usize, usize)>,
    others: Vec<usize>,
}

/// The key of a value under `compare_values(a, b) == 0`: numbers are compared
/// once rounded to 15 significant digits, text by its uppercase form, and an
/// empty cell equals 0, "" and FALSE. Numbers of magnitude 1 or more that
/// round differently are at least 1e-15 apart, more than the tolerance; the
/// smaller ones, blanks, empty text and booleans get no key.
fn compare_key(value: &CalcResult) -> Option<Key> {
    match value {
        CalcResult::Number(n) => {
            let rounded = to_excel_precision(*n, 15);
            if rounded.is_finite() && rounded.abs() >= 1.0 {
                Some(Key::Number(rounded.to_bits()))
            } else {
                None
            }
        }
        CalcResult::String(s) if !s.is_empty() => Some(Key::Text(s.to_uppercase())),
        _ => None,
    }
}

/// The key of a value under `values_are_equal`: numbers within `f64::EPSILON`
/// of each other, text by its uppercase form, an empty cell only equal to
/// another. From 2 up, distinct numbers are at least `f64::EPSILON` apart, so
/// they are equal only if they are the same number.
fn equal_key(value: &CalcResult) -> Option<Key> {
    match value {
        CalcResult::Number(n) => {
            if n.is_finite() && n.abs() >= 2.0 {
                Some(Key::Number(n.to_bits()))
            } else {
                None
            }
        }
        CalcResult::String(s) => Some(Key::Text(s.to_uppercase())),
        _ => None,
    }
}

/// Whether a text lookup is a plain string under `from_wildcard_to_regex`:
/// `*`, `?` and `~` (the escape) are the only characters it does not take
/// literally.
fn is_plain_text(text: &str) -> bool {
    !text.contains(['*', '?', '~'])
}

/// One search vector: the values of a single row or column of cells, in order.
pub(crate) struct SearchVector {
    /// The cells inside the sheet's used area, read.
    values: Vec<CalcResult>,
    /// The length of the whole vector. The positions from `values.len()` on lie
    /// beyond the used area and are empty cells.
    len: usize,
    /// The length up to the last cell that is not empty: what approximate
    /// (binary) searches look at.
    sorted_len: usize,
    /// Lowercase text to first and last position, for text lookups in
    /// `ExactMode::Wildcard`.
    lowercase: OnceLock<HashMap<String, (usize, usize)>>,
    compare: OnceLock<KeyIndex>,
    equal: OnceLock<KeyIndex>,
    /// Whether exact-match searches may build and use the indexes. A vector
    /// that is used once is scanned instead.
    indexed: bool,
}

impl SearchVector {
    pub(crate) fn new(values: Vec<CalcResult>, len: usize, indexed: bool) -> SearchVector {
        debug_assert!(values.len() <= len);
        let sorted_len = values
            .iter()
            .rposition(|v| !matches!(v, CalcResult::EmptyCell))
            .map_or(0, |p| p + 1);
        SearchVector {
            values,
            len,
            sorted_len,
            lowercase: OnceLock::new(),
            compare: OnceLock::new(),
            equal: OnceLock::new(),
            indexed,
        }
    }

    /// The length approximate matches search: the vector without its trailing
    /// empty cells.
    pub(crate) fn sorted_len(&self) -> usize {
        self.sorted_len
    }

    /// The value at `index`, which must be below `len()`.
    pub(crate) fn get(&self, index: usize) -> &CalcResult {
        self.values.get(index).unwrap_or(&EMPTY)
    }

    /// The whole vector, empty cells included.
    pub(crate) fn to_vec(&self) -> Vec<CalcResult> {
        let mut all = self.values.clone();
        all.resize(self.len, CalcResult::EmptyCell);
        all
    }

    /// The first (or with `from_last` the last) position whose value matches
    /// `lookup` in `mode`, the same answer as `linear_find`.
    pub(crate) fn find_exact(
        &self,
        lookup: &CalcResult,
        mode: ExactMode,
        from_last: bool,
    ) -> Option<usize> {
        if !self.indexed {
            return self.linear_find(lookup, mode, from_last);
        }
        match (mode, lookup) {
            (ExactMode::Wildcard { .. }, CalcResult::String(text)) => {
                if !is_plain_text(text) {
                    return self.linear_find(lookup, mode, from_last);
                }
                // Only text can match text here, and empty cells never do.
                let index = self.lowercase.get_or_init(|| {
                    let mut index: HashMap<String, (usize, usize)> = HashMap::new();
                    for (position, value) in self.values.iter().enumerate() {
                        if let CalcResult::String(s) = value {
                            index
                                .entry(s.to_lowercase())
                                .and_modify(|(_, last)| *last = position)
                                .or_insert((position, position));
                        }
                    }
                    index
                });
                index
                    .get(&text.to_lowercase())
                    .map(|&(first, last)| if from_last { last } else { first })
            }
            (ExactMode::Wildcard { compare: false }, _) => {
                let index = self.equal.get_or_init(|| self.key_index(equal_key));
                self.find_in(index, equal_key(lookup), from_last, |x| {
                    values_are_equal(x, lookup)
                })
            }
            (ExactMode::Wildcard { compare: true }, _) | (ExactMode::Compare, _) => {
                let index = self.compare.get_or_init(|| self.key_index(compare_key));
                self.find_in(index, compare_key(lookup), from_last, |x| {
                    compare_values(x, lookup) == 0
                })
            }
        }
    }

    fn key_index(&self, key: fn(&CalcResult) -> Option<Key>) -> KeyIndex {
        let mut index = KeyIndex::default();
        for (position, value) in self.values.iter().enumerate() {
            match key(value) {
                Some(k) => {
                    index
                        .keyed
                        .entry(k)
                        .and_modify(|(_, last)| *last = position)
                        .or_insert((position, position));
                }
                None => index.others.push(position),
            }
        }
        index
    }

    fn find_in(
        &self,
        index: &KeyIndex,
        key: Option<Key>,
        from_last: bool,
        matches: impl Fn(&CalcResult) -> bool,
    ) -> Option<usize> {
        if let Some(key) = key {
            // A value with a key equals only values with the same key. The
            // empty cells past the used area have none.
            return index
                .keyed
                .get(&key)
                .map(|&(first, last)| if from_last { last } else { first });
        }
        let in_tail = self.len > self.values.len() && matches(&EMPTY);
        let mut others = index.others.iter().map(|&p| (p, &self.values[p]));
        if from_last {
            if in_tail {
                return Some(self.len - 1);
            }
            others.rev().find(|(_, v)| matches(v)).map(|(p, _)| p)
        } else {
            others
                .find(|(_, v)| matches(v))
                .map(|(p, _)| p)
                .or(if in_tail {
                    Some(self.values.len())
                } else {
                    None
                })
        }
    }

    /// The reference implementation of `find_exact`: the scans the lookup
    /// functions used to run, over the whole vector.
    pub(crate) fn linear_find(
        &self,
        lookup: &CalcResult,
        mode: ExactMode,
        from_last: bool,
    ) -> Option<usize> {
        let matches: Box<dyn Fn(&CalcResult) -> bool> = match (mode, lookup) {
            (ExactMode::Wildcard { .. }, CalcResult::String(s)) => {
                match from_wildcard_to_regex(&s.to_lowercase(), true) {
                    Ok(reg) => Box::new(move |x| result_matches_regex(x, &reg)),
                    Err(_) => Box::new(|_| false),
                }
            }
            (ExactMode::Wildcard { compare: false }, _) => {
                Box::new(|x| values_are_equal(x, lookup))
            }
            _ => Box::new(|x| compare_values(x, lookup) == 0),
        };
        let in_tail = self.len > self.values.len() && matches(&EMPTY);
        let found = self.values.iter().enumerate();
        if from_last {
            if in_tail {
                return Some(self.len - 1);
            }
            found.rev().find(|(_, v)| matches(v)).map(|(p, _)| p)
        } else {
            found
                .clone()
                .find(|(_, v)| matches(v))
                .map(|(p, _)| p)
                .or(if in_tail {
                    Some(self.values.len())
                } else {
                    None
                })
        }
    }
}

/// `binary_search_on_array` over a search vector without its trailing empty
/// cells.
///
/// KANTOR PATCH (Excel compatibility): upstream searched the whole range, and
/// read the empty cells below the data as 0 or "" (`compare_values`), so an
/// approximate VLOOKUP or MATCH over `A:B`, or over a range longer than its
/// data, could land on a blank row. Excel ignores them.
pub(crate) fn binary_search_on_vector(target: &CalcResult, vector: &SearchVector) -> i32 {
    let mut l = 0;
    let mut r = vector.sorted_len();
    while l < r {
        let m = (l + r) / 2;
        match compare_values(vector.get(m), target) {
            -1 => {
                l = m + 1;
            }
            1 => {
                r = m;
            }
            _ => {
                return m as i32;
            }
        }
    }
    if l == 0 {
        return -2;
    }
    (l - 1) as i32
}

/// What one evaluation pass keeps: the search vectors read so far and the used
/// area of the sheets.
#[derive(Default)]
pub(crate) struct LookupCache {
    vectors: HashMap<VectorKey, Arc<SearchVector>, KeyBuild>,
    dimensions: HashMap<u32, WorksheetDimension, KeyBuild>,
    /// For `evaluated_search_vector`: the offset of the first cell found not
    /// evaluated yet, where the next attempt starts looking.
    pending: HashMap<VectorKey, i32, KeyBuild>,
}

impl LookupCache {
    pub(crate) fn clear(&mut self) {
        self.vectors.clear();
        self.dimensions.clear();
        self.pending.clear();
    }

    /// A dynamic array of `sheet` wrote or removed spill cells: forget all the
    /// sheet's vectors and its used area.
    pub(crate) fn sheet_changed(&mut self, sheet: u32) {
        self.vectors.retain(|key, _| key.0 != sheet);
        self.pending.retain(|key, _| key.0 != sheet);
        self.dimensions.remove(&sheet);
    }

    /// A CSE array of `sheet` filled the rectangle `(row1, column1)` to
    /// `(row2, column2)`, creating every cell of it: forget the vectors that
    /// overlap it and grow the used area to cover it.
    pub(crate) fn area_written(
        &mut self,
        sheet: u32,
        row1: i32,
        column1: i32,
        row2: i32,
        column2: i32,
    ) {
        let apart = |key: &VectorKey| {
            key.0 != sheet || key.3 < row1 || key.1 > row2 || key.4 < column1 || key.2 > column2
        };
        self.vectors.retain(|key, _| apart(key));
        self.pending.retain(|key, _| apart(key));
        if let Some(d) = self.dimensions.get_mut(&sheet) {
            d.min_row = d.min_row.min(row1);
            d.max_row = d.max_row.max(row2);
            d.min_column = d.min_column.min(column1);
            d.max_column = d.max_column.max(column2);
        }
    }
}

impl<'a> Model<'a> {
    /// The used area of a sheet, kept for the rest of the pass during one.
    pub(crate) fn used_dimension(&mut self, sheet: u32) -> Result<WorksheetDimension, String> {
        if self.evaluation.in_pass {
            if let Some(d) = self.evaluation.lookups.dimensions.get(&sheet) {
                return Ok(copy_dimension(d));
            }
        }
        let dimension = self
            .workbook
            .worksheet(sheet)
            .map_err(|_| format!("Invalid worksheet index: '{sheet}'"))?
            .dimension();
        if self.evaluation.in_pass {
            self.evaluation
                .lookups
                .dimensions
                .insert(sheet, copy_dimension(&dimension));
        }
        Ok(dimension)
    }

    /// The search vector `key` if this pass has read it already and it can be
    /// handed out again (see the module documentation).
    pub(crate) fn cached_search_vector(&self, key: VectorKey) -> Option<Arc<SearchVector>> {
        if !self.evaluation.in_pass {
            return None;
        }
        let vector = self.evaluation.lookups.vectors.get(&key)?;
        // A cell being evaluated would be read as a cycle: the vector is built
        // afresh, the way the old code did, to find it. (Defensive: such a
        // cell was read as a cycle when the vector was built, and a vector
        // built with a cycle is not kept.)
        let (sheet, row1, column1, row2, column2) = key;
        let reads_a_running_cell = self.evaluation.stack.iter().any(|c| {
            c.sheet == sheet
                && (row1..=row2).contains(&c.row)
                && (column1..=column2).contains(&c.column)
        });
        if reads_a_running_cell {
            return None;
        }
        Some(Arc::clone(vector))
    }

    /// The search vector `key`, if reading it evaluates nothing: every cell
    /// is a constant, empty, or a formula (or a spill cell of one) evaluated
    /// already in this pass. `None` otherwise, and outside a pass.
    ///
    /// For a function that reads only some cells of a range (the later
    /// criteria ranges of SUMIFS): reading the rest then evaluates nothing it
    /// would not have, and their values are final. The extra reads also go on
    /// record (`Evaluation::seen`), where only a spill could make anything of
    /// them, so this is only done in a workbook without dynamic arrays.
    pub(crate) fn evaluated_search_vector(&mut self, key: VectorKey) -> Option<Arc<SearchVector>> {
        if let Some(vector) = self.cached_search_vector(key) {
            return Some(vector);
        }
        if !self.evaluation.in_pass
            || !self.evaluation.lookup_cache_enabled
            || !self.evaluation.anchor_order.is_empty()
        {
            return None;
        }
        let (sheet, row1, column1, row2, column2) = key;
        let dimension = self.used_dimension(sheet).ok()?;
        let is_column = column1 == column2 && row1 != row2;
        let last = if is_column {
            row2.min(dimension.max_row) - row1
        } else {
            column2.min(dimension.max_column) - column1
        };
        let start = self
            .evaluation
            .lookups
            .pending
            .get(&key)
            .copied()
            .unwrap_or(0);
        for offset in start..=last {
            let (row, column) = if is_column {
                (row1 + offset, column1)
            } else {
                (row1, column1 + offset)
            };
            let position = CellReferenceIndex { sheet, row, column };
            let evaluated = |position: CellReferenceIndex| {
                let key = (position.sheet, position.row, position.column);
                self.evaluation.cells.get(&key) == Some(&CellState::Evaluated)
                    // KANTOR PATCH (partial recalculation): read as stored.
                    || self
                        .evaluation
                        .kantor_only
                        .as_ref()
                        .is_some_and(|only| !only.contains(&key))
            };
            let ready = match self.fetch_cell(position).as_deref() {
                None => true,
                Some(Cell::SpillCell { a, .. }) => evaluated(CellReferenceIndex {
                    sheet,
                    row: a.0,
                    column: a.1,
                }),
                Some(cell) => FormulaCell::of(cell).is_none() || evaluated(position),
            };
            if !ready {
                self.evaluation.lookups.pending.insert(key, offset);
                return None;
            }
        }
        self.evaluation.lookups.pending.remove(&key);
        Some(self.search_vector(sheet, row1, column1, row2, column2))
    }

    /// The values of the single row or column `(row1, column1)` to
    /// `(row2, column2)` of `sheet`, as a search vector. Every cell is read as
    /// `evaluate_cell` would read it; the cells past the sheet's used area are
    /// put on record as read empty in one go and left virtual. During a pass the
    /// vector is kept and handed out again while that gives the same values.
    pub(crate) fn search_vector(
        &mut self,
        sheet: u32,
        row1: i32,
        column1: i32,
        row2: i32,
        column2: i32,
    ) -> Arc<SearchVector> {
        let key = (sheet, row1, column1, row2, column2);
        if let Some(vector) = self.cached_search_vector(key) {
            return vector;
        }
        let in_pass = self.evaluation.in_pass;
        let is_column = column1 == column2 && row1 != row2;
        let len = if is_column {
            row2 - row1 + 1
        } else {
            column2 - column1 + 1
        }
        .max(0) as usize;
        // The part inside the used area. A sheet that does not exist reads as
        // empty, as `evaluate_cell` reads it.
        let read = match self.used_dimension(sheet) {
            Ok(d) => {
                if is_column {
                    let last = row2.min(d.max_row).max(row1 - 1);
                    self.record_seen_empty_range(sheet, last + 1, column1, row2, column2);
                    (last - row1 + 1) as usize
                } else {
                    let last = column2.min(d.max_column).max(column1 - 1);
                    self.record_seen_empty_range(sheet, row1, last + 1, row2, column2);
                    (last - column1 + 1) as usize
                }
            }
            Err(_) => len,
        };
        let cycles_before = self.evaluation.cycle_reads;
        let mut values = Vec::with_capacity(read);
        for offset in 0..read as i32 {
            let (row, column) = if is_column {
                (row1 + offset, column1)
            } else {
                (row1, column1 + offset)
            };
            values.push(self.evaluate_cell(CellReferenceIndex { sheet, row, column }));
        }
        let reusable = in_pass
            && self.evaluation.lookup_cache_enabled
            && self.evaluation.restart.is_none()
            && !self.evaluation.unwinding
            && self.evaluation.cycle_reads == cycles_before;
        let vector = Arc::new(SearchVector::new(values, len, reusable));
        if reusable {
            self.evaluation
                .lookups
                .vectors
                .insert(key, Arc::clone(&vector));
        }
        vector
    }
}

fn copy_dimension(d: &WorksheetDimension) -> WorksheetDimension {
    WorksheetDimension {
        min_row: d.min_row,
        max_row: d.max_row,
        min_column: d.min_column,
        max_column: d.max_column,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expressions::token::Error;

    fn error(error: Error) -> CalcResult {
        CalcResult::Error {
            error,
            origin: CellReferenceIndex {
                sheet: 0,
                row: 1,
                column: 1,
            },
            message: String::new(),
        }
    }

    /// A small deterministic generator, so the test needs no dependency.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 33
        }
        fn pick<'b, T>(&mut self, items: &'b [T]) -> &'b T {
            &items[self.next() as usize % items.len()]
        }
    }

    fn pool() -> Vec<CalcResult> {
        let texts = [
            "", "a", "A", "b", "B", "ab", "Ab", "1", "2", "10", "1.0", "TRUE", "straße", "STRASSE",
            "ß", "ss", "x*", "x?", "~", "a~b", "İ", "i̇", " a", "a ",
        ];
        let numbers = [
            0.0,
            -0.0,
            1e-20,
            2e-20,
            0.1,
            0.1 + 0.2,
            0.3,
            0.5,
            1.0,
            1.0 - f64::EPSILON / 2.0,
            1.0 + f64::EPSILON,
            1.000000000000001,
            1.0000000000000002,
            2.0,
            2.0 - f64::EPSILON,
            -1.0,
            -3.5,
            10.0,
            // Equal once rounded to 15 digits, but not the same number.
            1e9 + 1.0 / 3.0,
            1e9 + 0.333333,
            1e300,
            f64::MAX,
        ];
        let mut values: Vec<CalcResult> = texts
            .iter()
            .map(|s| CalcResult::String(s.to_string()))
            .collect();
        values.extend(numbers.iter().map(|n| CalcResult::Number(*n)));
        values.extend([
            CalcResult::Boolean(true),
            CalcResult::Boolean(false),
            CalcResult::EmptyCell,
            error(Error::NA),
            error(Error::DIV),
            error(Error::VALUE),
        ]);
        values
    }

    /// The indexes give the answer of the scans they replace, for every mode,
    /// direction and kind of value, with and without empty cells past the
    /// used area.
    #[test]
    fn index_matches_linear_scan() {
        let pool = pool();
        let mut random = Lcg(7);
        let modes = [
            ExactMode::Wildcard { compare: true },
            ExactMode::Wildcard { compare: false },
            ExactMode::Compare,
        ];
        for round in 0..300 {
            let size = (random.next() % 40) as usize;
            let values: Vec<CalcResult> = (0..size).map(|_| random.pick(&pool).clone()).collect();
            let tail = if round % 3 == 0 { 5 } else { 0 };
            let indexed = SearchVector::new(values.clone(), size + tail, true);
            let scanned = SearchVector::new(values, size + tail, false);
            let mut lookups = pool.clone();
            lookups.extend(
                ["x*", "*", "?", "a*", "~*", "A?", "~~"].map(|s| CalcResult::String(s.to_string())),
            );
            for lookup in &lookups {
                for mode in modes {
                    for from_last in [false, true] {
                        assert_eq!(
                            indexed.find_exact(lookup, mode, from_last),
                            scanned.find_exact(lookup, mode, from_last),
                            "round {round}, {mode:?}, from_last {from_last}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn binary_search_ignores_trailing_blanks() {
        let data = vec![
            CalcResult::Number(1.0),
            CalcResult::EmptyCell,
            CalcResult::Number(5.0),
        ];
        let mut values = data.clone();
        values.extend([CalcResult::EmptyCell, CalcResult::EmptyCell]);
        // Two blank cells in the used area and three past it.
        let vector = SearchVector::new(values, 8, true);
        assert_eq!(vector.sorted_len(), 3);
        for target in [-1.0, 0.0, 1.0, 3.0, 5.0, 9.0] {
            let target = CalcResult::Number(target);
            assert_eq!(
                binary_search_on_vector(&target, &vector),
                crate::functions::binary_search::binary_search_on_array(&target, &data)
            );
        }
        let blank = SearchVector::new(vec![CalcResult::EmptyCell], 4, true);
        assert_eq!(
            binary_search_on_vector(&CalcResult::Number(1.0), &blank),
            -2
        );
    }
}
