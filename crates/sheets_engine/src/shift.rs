//! Rows and columns inserted or deleted since a file was opened, and how they
//! move cell references in the parts `preserve` carries from it.
//!
//! IronCalc shifts what it models (cells, formulas, merges, names...). The
//! parts and elements carried verbatim from the original (data validation,
//! tables, sparklines, comments, drawings, charts...) still point at the cells
//! of the original; [`Moves`] maps those positions to the saved workbook, the
//! way Excel adjusts them: a reference after an insertion moves, one inside a
//! deleted block is gone (`#REF!`, or the item is dropped), a range loses its
//! deleted part and grows when rows are inserted strictly inside it, and a
//! whole column or row stays whole.
//!
//! Undo and redo move between states that each have their own list of moves
//! from the opened file ([`Layout`]); the moves between any two states are
//! found through their common ancestor.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use crate::{MAX_COLS, MAX_ROWS};

/// An insertion or deletion of whole rows or columns. Sheets are identified
/// by `sheetId`; positions are 0-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    InsertRows { sheet: u32, at: u32, count: u32 },
    DeleteRows { sheet: u32, at: u32, count: u32 },
    InsertColumns { sheet: u32, at: u32, count: u32 },
    DeleteColumns { sheet: u32, at: u32, count: u32 },
}

impl Op {
    fn inverse(self) -> Op {
        match self {
            Op::InsertRows { sheet, at, count } => Op::DeleteRows { sheet, at, count },
            Op::DeleteRows { sheet, at, count } => Op::InsertRows { sheet, at, count },
            Op::InsertColumns { sheet, at, count } => Op::DeleteColumns { sheet, at, count },
            Op::DeleteColumns { sheet, at, count } => Op::InsertColumns { sheet, at, count },
        }
    }

    fn sheet(self) -> u32 {
        match self {
            Op::InsertRows { sheet, .. }
            | Op::DeleteRows { sheet, .. }
            | Op::InsertColumns { sheet, .. }
            | Op::DeleteColumns { sheet, .. } => sheet,
        }
    }
}

/// The moves that lead from the opened file to one state of the workbook: a
/// persistent list (states share their common beginning).
#[derive(Clone, Default)]
pub(crate) struct Layout(Option<Arc<Link>>);

pub(crate) struct Link {
    op: Op,
    prev: Layout,
    depth: usize,
}

impl Layout {
    pub fn with(&self, op: Op) -> Layout {
        Layout(Some(Arc::new(Link {
            op,
            prev: self.clone(),
            depth: self.depth() + 1,
        })))
    }

    fn depth(&self) -> usize {
        self.0.as_ref().map_or(0, |l| l.depth)
    }

    fn same(&self, other: &Layout) -> bool {
        match (&self.0, &other.0) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    fn up(&self) -> (Op, Layout) {
        let link = self.0.as_ref().expect("not the root");
        (link.op, link.prev.clone())
    }

    /// The moves from the state `self` belongs to, to the state of `to`.
    pub fn moves_to(&self, to: &Layout) -> Moves {
        let (mut a, mut b) = (self.clone(), to.clone());
        let mut back = Vec::new(); // undone, from `self` upwards
        let mut forward = Vec::new(); // done, from `to` upwards
        while a.depth() > b.depth() {
            let (op, prev) = a.up();
            back.push(op.inverse());
            a = prev;
        }
        while b.depth() > a.depth() {
            let (op, prev) = b.up();
            forward.push(op);
            b = prev;
        }
        while !a.same(&b) {
            let (op, prev) = a.up();
            back.push(op.inverse());
            a = prev;
            let (op, prev) = b.up();
            forward.push(op);
            b = prev;
        }
        forward.reverse();
        back.extend(forward);
        Moves(back)
    }
}

/// Layouts by history state (see `Revisions`).
#[derive(Default)]
pub(crate) struct Layouts {
    by_state: HashMap<u64, Layout>,
}

impl Layouts {
    pub fn get(&self, state: u64) -> Layout {
        self.by_state.get(&state).cloned().unwrap_or_default()
    }

    pub fn set(&mut self, state: u64, layout: Layout) {
        if layout.0.is_none() {
            self.by_state.remove(&state);
        } else {
            self.by_state.insert(state, layout);
        }
    }
}

/// A cell range, 0-based and inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Area {
    pub top: u32,
    pub left: u32,
    pub bottom: u32,
    pub right: u32,
}

/// Rows and columns inserted or deleted, in order.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Moves(Vec<Op>);

impl Moves {
    #[cfg(test)]
    pub fn new(ops: Vec<Op>) -> Self {
        Moves(ops)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether anything moved on the sheet.
    pub fn touches(&self, sheet: u32) -> bool {
        self.0.iter().any(|op| op.sheet() == sheet)
    }

    /// Where a cell of `sheet` is now; `None` if it was deleted.
    pub fn cell(&self, sheet: u32, row: u32, col: u32) -> Option<(u32, u32)> {
        let a = self.area(
            sheet,
            Area {
                top: row,
                left: col,
                bottom: row,
                right: col,
            },
        )?;
        Some((a.top, a.left))
    }

    /// Where a range of `sheet` is now; `None` if all of it was deleted.
    pub fn area(&self, sheet: u32, area: Area) -> Option<Area> {
        self.0
            .iter()
            .try_fold(area, |a, op| apply_area(*op, sheet, a))
    }

    /// Where a grid line crossing (the top-left corner of cell `row`, `col`)
    /// is now, as `(row, col)`: what a drawing's anchor follows. A corner in
    /// a deleted block moves to where the block was.
    pub fn point(&self, sheet: u32, row: u32, col: u32) -> (u32, u32) {
        let line = |x: u32, at: u32, count: u32, insert: bool, max: u32| {
            if insert {
                if x >= at { (x + count).min(max) } else { x }
            } else if x >= at + count {
                x - count
            } else {
                x.min(at)
            }
        };
        self.0
            .iter()
            .filter(|op| op.sheet() == sheet)
            .fold((row, col), |(r, c), op| match *op {
                Op::InsertRows { at, count, .. } => (line(r, at, count, true, MAX_ROWS), c),
                Op::DeleteRows { at, count, .. } => (line(r, at, count, false, MAX_ROWS), c),
                Op::InsertColumns { at, count, .. } => (r, line(c, at, count, true, MAX_COLS)),
                Op::DeleteColumns { at, count, .. } => (r, line(c, at, count, false, MAX_COLS)),
            })
    }

    /// A space-separated list of A1 ranges (`sqref`), moved; `None` if
    /// nothing of it is left.
    pub fn sqref(&self, sheet: u32, sqref: &str) -> Option<String> {
        let mut out = Vec::new();
        for part in sqref.split_whitespace() {
            match parse_range(part) {
                Some(r) => {
                    if let Some(moved) = self.range(sheet, &r) {
                        out.push(moved);
                    }
                }
                None => out.push(part.to_owned()),
            }
        }
        (!out.is_empty()).then(|| out.join(" "))
    }

    fn range(&self, sheet: u32, r: &RangeRef) -> Option<String> {
        let moved = self.area(sheet, r.area())?;
        Some(r.with(moved).to_string())
    }

    /// A formula with its references moved. Unqualified references are on
    /// `home`; `sheets` maps sheet names (as the formula writes them,
    /// case-insensitively) to sheet ids. References to unknown sheets, names,
    /// strings and functions are left alone; deleted references become
    /// `#REF!`.
    pub fn formula<'a>(
        &self,
        formula: &'a str,
        home: Option<u32>,
        sheets: &HashMap<String, u32>,
    ) -> Cow<'a, str> {
        if self.is_empty() {
            return Cow::Borrowed(formula);
        }
        let mut out = String::with_capacity(formula.len());
        let mut changed = false;
        let mut last = 0;
        for token in references(formula) {
            let sheet = match &token.sheet {
                Some(name) => sheets.get(&name.to_lowercase()).copied(),
                None => home,
            };
            let Some(sheet) = sheet else { continue };
            if !self.touches(sheet) {
                continue;
            }
            let text = match self.area(sheet, token.range.area()) {
                Some(a) => token.range.with(a).to_string(),
                None => "#REF!".to_owned(),
            };
            let span = &formula[token.range_span.clone()];
            if text != span {
                out.push_str(&formula[last..token.range_span.start]);
                out.push_str(&text);
                last = token.range_span.end;
                changed = true;
            }
        }
        if !changed {
            return Cow::Borrowed(formula);
        }
        out.push_str(&formula[last..]);
        Cow::Owned(out)
    }
}

impl Moves {
    /// A conditional formatting formula of a rule on `area` (of `home`),
    /// moved. Its relative references are relative to the top-left cell of
    /// the area; when that cell was deleted, the area's new top-left is a
    /// cell further down or right, and the formula is first re-based on it
    /// (as Excel does), then moved like any formula.
    pub fn cf_formula<'a>(
        &self,
        formula: &'a str,
        home: u32,
        sheets: &HashMap<String, u32>,
        area: Area,
    ) -> Cow<'a, str> {
        if !self.touches(home) {
            return Cow::Borrowed(formula);
        }
        let first =
            |rows: bool, lo: u32, hi: u32| (lo..=hi).find(|&x| self.line_kept(home, x, rows));
        let (Some(top), Some(left)) = (
            first(true, area.top, area.bottom),
            first(false, area.left, area.right),
        ) else {
            return self.formula(formula, Some(home), sheets);
        };
        let (dr, dc) = (top - area.top, left - area.left);
        if dr == 0 && dc == 0 {
            return self.formula(formula, Some(home), sheets);
        }
        let mut rebased = String::with_capacity(formula.len());
        let mut last = 0;
        for token in references(formula) {
            if token.sheet.is_some() {
                continue;
            }
            let shift = |(r, c, ar, ac): (u32, u32, bool, bool)| {
                (
                    if ar { r } else { (r + dr).min(MAX_ROWS - 1) },
                    if ac { c } else { (c + dc).min(MAX_COLS - 1) },
                    ar,
                    ac,
                )
            };
            let moved = RangeRef {
                kind: token.range.kind,
                a: shift(token.range.a),
                b: shift(token.range.b),
            };
            rebased.push_str(&formula[last..token.range_span.start]);
            rebased.push_str(&moved.to_string());
            last = token.range_span.end;
        }
        rebased.push_str(&formula[last..]);
        Cow::Owned(self.formula(&rebased, Some(home), sheets).into_owned())
    }

    /// Whether row (`rows`) or column `x` of `sheet` is still there.
    fn line_kept(&self, sheet: u32, x: u32, rows: bool) -> bool {
        let mut x = x;
        for op in self.0.iter().filter(|op| op.sheet() == sheet) {
            match (*op, rows) {
                (Op::InsertRows { at, count, .. }, true)
                | (Op::InsertColumns { at, count, .. }, false) => {
                    if x >= at {
                        x += count;
                    }
                }
                (Op::DeleteRows { at, count, .. }, true)
                | (Op::DeleteColumns { at, count, .. }, false) => {
                    if (at..at + count).contains(&x) {
                        return false;
                    }
                    if x >= at + count {
                        x -= count;
                    }
                }
                _ => {}
            }
        }
        true
    }
}

fn apply_area(op: Op, sheet: u32, a: Area) -> Option<Area> {
    if op.sheet() != sheet {
        return Some(a);
    }
    match op {
        Op::InsertRows { at, count, .. } => {
            let (top, bottom) = insert(a.top, a.bottom, at, count, MAX_ROWS)?;
            Some(Area { top, bottom, ..a })
        }
        Op::DeleteRows { at, count, .. } => {
            let (top, bottom) = delete(a.top, a.bottom, at, count, MAX_ROWS)?;
            Some(Area { top, bottom, ..a })
        }
        Op::InsertColumns { at, count, .. } => {
            let (left, right) = insert(a.left, a.right, at, count, MAX_COLS)?;
            Some(Area { left, right, ..a })
        }
        Op::DeleteColumns { at, count, .. } => {
            let (left, right) = delete(a.left, a.right, at, count, MAX_COLS)?;
            Some(Area { left, right, ..a })
        }
    }
}

/// `count` lines inserted before line `at`, applied to lines `lo..=hi`.
fn insert(lo: u32, hi: u32, at: u32, count: u32, max: u32) -> Option<(u32, u32)> {
    if lo == 0 && hi == max - 1 {
        return Some((lo, hi)); // a whole column / row stays whole
    }
    let shift = |x: u32| if x >= at { x + count } else { x };
    let (lo, hi) = (shift(lo), shift(hi));
    if lo >= max {
        return None; // pushed off the sheet
    }
    Some((lo, hi.min(max - 1)))
}

/// Lines `at..at + count` deleted, applied to lines `lo..=hi`.
fn delete(lo: u32, hi: u32, at: u32, count: u32, max: u32) -> Option<(u32, u32)> {
    if lo == 0 && hi == max - 1 {
        return Some((lo, hi));
    }
    let end = at + count; // first line after the deleted block
    if lo >= at && hi < end {
        return None;
    }
    let map = |x: u32| {
        if x >= end {
            x - count
        } else if x >= at {
            at
        } else {
            x
        }
    };
    let lo = map(lo);
    // The last line, if deleted, becomes the last line before the block.
    let hi = if hi >= at && hi < end {
        at - 1
    } else {
        map(hi)
    };
    Some((lo, hi))
}

// ------------------------------------------------------------------ A1 references

/// An A1 cell, range, whole-column or whole-row reference, with its `$`s.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RangeRef {
    kind: RefKind,
    /// `(row, col)`, 0-based, and whether each is absolute.
    a: (u32, u32, bool, bool),
    b: (u32, u32, bool, bool),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum RefKind {
    Cell,
    Range,
    Columns,
    Rows,
}

impl RangeRef {
    pub fn area(&self) -> Area {
        Area {
            top: self.a.0.min(self.b.0),
            left: self.a.1.min(self.b.1),
            bottom: self.a.0.max(self.b.0),
            right: self.a.1.max(self.b.1),
        }
    }

    fn with(&self, area: Area) -> RangeRef {
        RangeRef {
            kind: self.kind,
            a: (area.top, area.left, self.a.2, self.a.3),
            b: (area.bottom, area.right, self.b.2, self.b.3),
        }
    }
}

impl std::fmt::Display for RangeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let d = |abs: bool| if abs { "$" } else { "" };
        let cell = |f: &mut std::fmt::Formatter<'_>, (r, c, ar, ac): (u32, u32, bool, bool)| {
            write!(f, "{}{}{}{}", d(ac), crate::column_name(c), d(ar), r + 1)
        };
        match self.kind {
            RefKind::Cell => cell(f, self.a),
            RefKind::Range => {
                cell(f, self.a)?;
                f.write_str(":")?;
                cell(f, self.b)
            }
            RefKind::Columns => write!(
                f,
                "{}{}:{}{}",
                d(self.a.3),
                crate::column_name(self.a.1),
                d(self.b.3),
                crate::column_name(self.b.1)
            ),
            RefKind::Rows => write!(
                f,
                "{}{}:{}{}",
                d(self.a.2),
                self.a.0 + 1,
                d(self.b.2),
                self.b.0 + 1
            ),
        }
    }
}

/// `$A$1`, `B2:C3`, `A:B`, `$3:$4`, or `None`.
pub(crate) fn parse_range(s: &str) -> Option<RangeRef> {
    let (len, r) = scan_range(s.as_bytes(), 0)?;
    (len == s.len()).then_some(r)
}

/// `[$]letters`: (column, absolute, bytes read).
fn scan_col(b: &[u8], i: usize) -> Option<(u32, bool, usize)> {
    let abs = b.get(i) == Some(&b'$');
    let start = i + usize::from(abs);
    let mut j = start;
    let mut col: u32 = 0;
    while j < b.len() && b[j].is_ascii_uppercase() && j - start < 3 {
        col = col * 26 + u32::from(b[j] - b'A' + 1);
        j += 1;
    }
    (j > start && (1..=MAX_COLS).contains(&col)).then(|| (col - 1, abs, j - i))
}

/// `[$]digits`: (row, absolute, bytes read).
fn scan_row(b: &[u8], i: usize) -> Option<(u32, bool, usize)> {
    let abs = b.get(i) == Some(&b'$');
    let start = i + usize::from(abs);
    let mut j = start;
    while j < b.len() && b[j].is_ascii_digit() && j - start < 7 {
        j += 1;
    }
    if j == start || b[start] == b'0' {
        return None;
    }
    let row: u32 = std::str::from_utf8(&b[start..j]).ok()?.parse().ok()?;
    (row <= MAX_ROWS).then(|| (row - 1, abs, j - i))
}

fn scan_cell(b: &[u8], i: usize) -> Option<((u32, u32, bool, bool), usize)> {
    let (col, ac, n1) = scan_col(b, i)?;
    let (row, ar, n2) = scan_row(b, i + n1)?;
    Some(((row, col, ar, ac), n1 + n2))
}

/// The longest reference at `i`: (bytes read, reference).
fn scan_range(b: &[u8], i: usize) -> Option<(usize, RangeRef)> {
    if let Some((a, n)) = scan_cell(b, i) {
        if b.get(i + n) == Some(&b':')
            && let Some((c, m)) = scan_cell(b, i + n + 1)
        {
            return Some((
                n + 1 + m,
                RangeRef {
                    kind: RefKind::Range,
                    a,
                    b: c,
                },
            ));
        }
        return Some((
            n,
            RangeRef {
                kind: RefKind::Cell,
                a,
                b: a,
            },
        ));
    }
    if let Some((c1, a1, n)) = scan_col(b, i)
        && b.get(i + n) == Some(&b':')
        && let Some((c2, a2, m)) = scan_col(b, i + n + 1)
    {
        return Some((
            n + 1 + m,
            RangeRef {
                kind: RefKind::Columns,
                a: (0, c1, false, a1),
                b: (MAX_ROWS - 1, c2, false, a2),
            },
        ));
    }
    if let Some((r1, a1, n)) = scan_row(b, i)
        && b.get(i + n) == Some(&b':')
        && let Some((r2, a2, m)) = scan_row(b, i + n + 1)
    {
        return Some((
            n + 1 + m,
            RangeRef {
                kind: RefKind::Rows,
                a: (r1, 0, a1, false),
                b: (r2, MAX_COLS - 1, a2, false),
            },
        ));
    }
    None
}

pub(crate) struct Token {
    /// The sheet name, unquoted.
    pub(crate) sheet: Option<String>,
    pub(crate) range: RangeRef,
    /// Where the reference (without the sheet) is in the formula.
    range_span: std::ops::Range<usize>,
}

fn is_name_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || c >= 0x80
}

/// The references in a formula, skipping strings, functions, names and
/// structured references.
pub(crate) fn references(formula: &str) -> Vec<Token> {
    let b = formula.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => {
                // A string: skip to its end ("" is an escaped quote).
                i += 1;
                while i < b.len() {
                    if b[i] == b'"' {
                        if b.get(i + 1) == Some(&b'"') {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i += 1;
            }
            b'[' => {
                // Structured reference or external workbook: skip it.
                let mut depth = 0;
                while i < b.len() {
                    match b[i] {
                        b'[' => depth += 1,
                        b']' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                i += 1;
            }
            b'\'' => {
                // A quoted sheet name.
                let mut name = String::new();
                let mut j = i + 1;
                let mut closed = false;
                while j < b.len() {
                    if b[j] == b'\'' {
                        if b.get(j + 1) == Some(&b'\'') {
                            name.push('\'');
                            j += 2;
                            continue;
                        }
                        closed = true;
                        break;
                    }
                    let ch = formula[j..].chars().next().unwrap_or_default();
                    name.push(ch);
                    j += ch.len_utf8();
                }
                if closed && b.get(j + 1) == Some(&b'!') {
                    let at = j + 2;
                    if let Some((n, range)) = scan_range(b, at)
                        && !b.get(at + n).is_some_and(|&c| is_name_byte(c) || c == b'(')
                    {
                        out.push(Token {
                            sheet: Some(name),
                            range,
                            range_span: at..at + n,
                        });
                        i = at + n;
                        continue;
                    }
                    i = at;
                    continue;
                }
                i = j + 1;
            }
            c if is_name_byte(c) || c == b'$' => {
                // A word: a sheet name with `!`, a reference, a name or a function.
                let start = i;
                let mut j = i;
                while j < b.len() && (is_name_byte(b[j]) || b[j] == b'$') {
                    j += 1;
                }
                if b.get(j) == Some(&b'!') && b[start] != b'$' {
                    let at = j + 1;
                    if let Some((n, range)) = scan_range(b, at)
                        && !b.get(at + n).is_some_and(|&c| is_name_byte(c) || c == b'(')
                    {
                        out.push(Token {
                            sheet: Some(formula[start..j].to_owned()),
                            range,
                            range_span: at..at + n,
                        });
                        i = at + n;
                        continue;
                    }
                    i = at;
                    continue;
                }
                let preceded = start > 0 && (is_name_byte(b[start - 1]) || b[start - 1] == b'!');
                if !preceded
                    && let Some((n, range)) = scan_range(b, start)
                    && !b
                        .get(start + n)
                        .is_some_and(|&c| is_name_byte(c) || c == b'(' || c == b'!')
                {
                    out.push(Token {
                        sheet: None,
                        range,
                        range_span: start..start + n,
                    });
                    i = start + n;
                    continue;
                }
                i = j.max(start + 1);
            }
            _ => i += 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(sheet: u32, at: u32, count: u32) -> Op {
        Op::InsertRows { sheet, at, count }
    }

    fn del_rows(sheet: u32, at: u32, count: u32) -> Op {
        Op::DeleteRows { sheet, at, count }
    }

    fn sheets() -> HashMap<String, u32> {
        HashMap::from([("data".to_owned(), 1), ("my sheet".to_owned(), 2)])
    }

    #[test]
    fn ranges_move_like_excel() {
        let m = Moves::new(vec![rows(1, 4, 2)]); // two rows before row 5
        assert_eq!(
            m.sqref(1, "A1:B3 C5 D2:D9").as_deref(),
            Some("A1:B3 C7 D2:D11")
        );
        assert_eq!(m.sqref(2, "C5").as_deref(), Some("C5"), "other sheet");
        // Inserting at the first row moves the range, inside it grows it.
        assert_eq!(m.sqref(1, "A5:A6").as_deref(), Some("A7:A8"));
        let m = Moves::new(vec![del_rows(1, 1, 2)]); // rows 2-3 deleted
        assert_eq!(m.sqref(1, "A1:A10 B2:B3 C4").as_deref(), Some("A1:A8 C2"));
        assert_eq!(m.sqref(1, "B2:B3"), None);
        assert_eq!(m.sqref(1, "A3:A5").as_deref(), Some("A2:A3"));
        assert_eq!(m.sqref(1, "A:A").as_deref(), Some("A:A"));
        let m = Moves::new(vec![Op::DeleteColumns {
            sheet: 1,
            at: 0,
            count: 1,
        }]);
        assert_eq!(m.sqref(1, "A1 B1:D4 $C:$D").as_deref(), Some("A1:C4 $B:$C"));
    }

    #[test]
    fn formulas_move_their_references() {
        let m = Moves::new(vec![rows(1, 0, 1), del_rows(2, 4, 1)]);
        let f = |s: &str, home| m.formula(s, home, &sheets()).into_owned();
        assert_eq!(f("Data!$B$2:$B$7", None), "Data!$B$3:$B$8");
        assert_eq!(
            f("SUM(A1,Data!C3)+'My Sheet'!A5", Some(2)),
            "SUM(A1,Data!C4)+'My Sheet'!#REF!"
        );
        assert_eq!(f("A1+B2", Some(1)), "A2+B3");
        assert_eq!(
            f("\"A1\"&LOG10(A1)&Rate&Sales[A1]", Some(1)),
            "\"A1\"&LOG10(A2)&Rate&Sales[A1]"
        );
        assert_eq!(f("Other!A1", Some(1)), "Other!A1", "unknown sheet");
        assert_eq!(f("(Data!$A$1,Data!$A$3)", None), "(Data!$A$2,Data!$A$4)");
        assert_eq!(f("Data!A:A", None), "Data!A:A");
    }

    #[test]
    fn layouts_find_the_moves_between_states() {
        let root = Layout::default();
        let a = root.with(rows(1, 0, 1));
        let b = a.with(rows(1, 5, 2));
        let c = a.with(del_rows(1, 3, 1));
        assert_eq!(
            root.moves_to(&b),
            Moves::new(vec![rows(1, 0, 1), rows(1, 5, 2)])
        );
        assert_eq!(b.moves_to(&a), Moves::new(vec![del_rows(1, 5, 2)]));
        assert_eq!(
            b.moves_to(&c),
            Moves::new(vec![del_rows(1, 5, 2), del_rows(1, 3, 1)])
        );
        assert!(b.moves_to(&b).is_empty());
        // Moving there and back is the identity for what wasn't deleted.
        let there = root.moves_to(&b);
        let back = b.moves_to(&root);
        let cell = there.cell(1, 10, 0).unwrap();
        assert_eq!(back.cell(1, cell.0, cell.1), Some((10, 0)));
    }
}
