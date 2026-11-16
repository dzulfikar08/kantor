//! Merge Cells and Split Cells…, as Word's.
//!
//! Merging joins a rectangle of cells into one: the top-left cell's
//! properties, `w:gridSpan` for the columns it covers and `w:vMerge`
//! (`restart` on top, continued below) for its rows, with the text of
//! every cell that has some, in reading order. A selection that cuts
//! through a merged cell grows to take all of it, as in Word.
//!
//! Splitting divides a cell into rows × columns: columns into equal
//! widths (new grid columns where the old ones don't line up, the cells
//! above and below spanning them), rows by adding rows whose other cells
//! continue the ones above (`w:vMerge`), or by dividing a vertically
//! merged cell's rows. Its paragraphs go one per new cell.
//!
//! With Track Changes on, a merge down a column is recorded as Word
//! records it (`w:cellMerge` on each cell, the moved text as an insertion
//! in the top cell and a deletion where it was); a merge across columns
//! marks the row's cells merged with `w:hMerge` (first `restart`, the
//! others continuing it) with the properties before in `w:tcPrChange`, so
//! rejecting it brings the cells back and accepting it writes Word's
//! `w:gridSpan`. A split is recorded as the cells and rows it adds
//! (`w:cellIns`, `w:trPr/w:ins`) and the properties it changed
//! (`w:tcPrChange`, `w:tblGridChange`); the text stays in the first cell.

use crate::document::{Document, Kind};
use crate::edit::Grid;
use crate::model::{CellRef, Paragraph, Position, Selection};
use crate::page::set_child_ranked;
use crate::review::{RevKind, now_utc};
use crate::table::{Cell, Row, Table, VMerge, strip_element};
use crate::xml;

/// Most grid columns a table can have (Word's limit).
pub const MAX_COLUMNS: usize = 63;
/// Most rows Split Cells makes at once.
pub const MAX_SPLIT_ROWS: usize = 99;

pub const NOT_IN_TABLE: &str = "The caret isn't in a table";
pub const SELECT_CELLS: &str = "Select two or more cells to merge";
pub const NOT_RECTANGLE: &str = "These cells can't be merged";
pub const ROWS_DIVIDE: &str = "The number of rows must divide the rows the merged cell spans";
pub const TOO_MANY_COLUMNS: &str = "A table can have at most 63 columns";

/// `w:tcPr` children in schema order.
const TCPR_ORDER: &[&str] = &[
    "cnfStyle",
    "tcW",
    "gridSpan",
    "hMerge",
    "vMerge",
    "tcBorders",
    "shd",
    "noWrap",
    "tcMar",
    "textDirection",
    "tcFitText",
    "vAlign",
    "hideMark",
    "headers",
    "cellIns",
    "cellDel",
    "cellMerge",
    "tcPrChange",
];

fn rank(name: &str) -> usize {
    TCPR_ORDER
        .iter()
        .position(|n| *n == name)
        .unwrap_or(TCPR_ORDER.len())
}

/// `tc_pr` with its own child `name` set to `new` (or removed), in schema
/// order; every other byte kept.
pub(crate) fn set_tc_child(tc_pr: &str, name: &str, new: Option<String>) -> String {
    set_child_ranked(tc_pr, name, rank, |_| true, new)
}

/// `cell` with span, merge state and width set (in its `w:tcPr` too).
fn shape(cell: &Cell, span: usize, vmerge: VMerge, width: u32) -> Cell {
    let mut x = cell
        .tc_pr
        .as_deref()
        .unwrap_or("<w:tcPr></w:tcPr>")
        .to_owned();
    x = set_tc_child(&x, "hMerge", None);
    x = set_tc_child(&x, "cellMerge", None);
    x = set_tc_child(
        &x,
        "gridSpan",
        (span > 1).then(|| format!("<w:gridSpan w:val=\"{span}\"/>")),
    );
    x = set_tc_child(&x, "vMerge", vmerge.element());
    x = set_tc_child(
        &x,
        "tcW",
        Some(format!("<w:tcW w:w=\"{width}\" w:type=\"dxa\"/>")),
    );
    let mut out = cell.clone();
    out.tc_pr = Some(x.into());
    out.refresh();
    out
}

/// Grid columns `[start, end)` of cell `c` in row `r`.
fn cols(t: &Table, r: usize, c: usize) -> (usize, usize) {
    let g = t.grid_col(r, c);
    (g, g + t.rows[r].cells[c].span.max(1) as usize)
}

/// The cell of row `r` that starts at grid column `g`.
fn cell_starting(t: &Table, r: usize, g: usize) -> Option<usize> {
    let row = t.rows.get(r)?;
    (0..row.cells.len()).find(|&c| t.grid_col(r, c) == g)
}

/// Rows `[top, bottom]` of the vertical merge cell `(r, c)` is part of.
fn group(t: &Table, r: usize, c: usize) -> (usize, usize) {
    let g = t.grid_col(r, c);
    let continues = |row: usize| {
        cell_starting(t, row, g).is_some_and(|ci| t.rows[row].cells[ci].vmerge == VMerge::Continue)
    };
    let mut top = r;
    while top > 0 && continues(top) && cell_starting(t, top - 1, g).is_some() {
        top -= 1;
    }
    let mut bottom = r;
    if t.rows[r].cells[c].vmerge != VMerge::None {
        while bottom + 1 < t.rows.len() && continues(bottom + 1) {
            bottom += 1;
        }
    }
    (top, bottom)
}

/// A cell by (row, cell index).
type At = (usize, usize);

/// A rectangle of the grid: rows `r0..=r1`, grid columns `g0..g1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    r0: usize,
    r1: usize,
    g0: usize,
    g1: usize,
}

/// The rectangle cells `a` and `b` (row, cell) span, grown until no
/// merged cell crosses its edge; with each row's cells in it (first
/// index, count). `None` when a row has no cells there.
fn rect(t: &Table, a: (usize, usize), b: (usize, usize)) -> Option<(Rect, Vec<(usize, usize)>)> {
    let (ca, cb) = (cols(t, a.0, a.1), cols(t, b.0, b.1));
    let mut r = Rect {
        r0: a.0.min(b.0),
        r1: a.0.max(b.0),
        g0: ca.0.min(cb.0),
        g1: ca.1.max(cb.1),
    };
    loop {
        let before = r;
        for ri in r.r0..=r.r1 {
            for ci in 0..t.rows[ri].cells.len() {
                let (s, e) = cols(t, ri, ci);
                if e <= r.g0 || s >= r.g1 {
                    continue;
                }
                r.g0 = r.g0.min(s);
                r.g1 = r.g1.max(e);
                let (top, bottom) = group(t, ri, ci);
                r.r0 = r.r0.min(top);
                r.r1 = r.r1.max(bottom);
            }
        }
        if r == before {
            break;
        }
    }
    let mut runs = Vec::new();
    for ri in r.r0..=r.r1 {
        let k0 = cell_starting(t, ri, r.g0)?;
        let mut k = k0;
        let mut g = r.g0;
        while g < r.g1 {
            let c = t.rows[ri].cells.get(k)?;
            g += c.span.max(1) as usize;
            k += 1;
        }
        if g != r.g1 {
            return None;
        }
        runs.push((k0, k - k0));
    }
    Some((r, runs))
}

/// Grid column edges in twips (`widths.len() + 1` of them).
fn edges(widths: &[u32]) -> Vec<u64> {
    let mut out = vec![0u64];
    for w in widths {
        out.push(out.last().copied().unwrap_or(0) + *w as u64);
    }
    out
}

fn has_text(paras: &[Paragraph]) -> bool {
    paras.iter().any(|p| !p.text.is_empty())
}

/// Merge `rect` of `(t, grid)` untracked: the new table and grid, and the
/// merged cell (row, cell).
fn merged(
    t: &Table,
    grid: &Grid,
    id: u32,
    cw: u32,
    rect: Rect,
    runs: &[(usize, usize)],
) -> (Table, Grid, (usize, usize)) {
    let widths = t.column_widths(cw);
    let w: u32 = widths[rect.g0..rect.g1.min(widths.len())].iter().sum();
    let span = rect.g1 - rect.g0;
    let mut nt = t.clone();
    let mut ng = grid.clone();
    // Every cell's text, in reading order; the top-left's when none has any.
    let mut text: Vec<Paragraph> = Vec::new();
    for (i, &(k0, n)) in runs.iter().enumerate() {
        for ps in &grid[rect.r0 + i][k0..k0 + n] {
            if has_text(ps) {
                text.extend(ps.iter().cloned());
            }
        }
    }
    if text.is_empty() {
        text = grid[rect.r0][runs[0].0].clone();
    }
    for (i, &(k0, n)) in runs.iter().enumerate().rev() {
        let r = rect.r0 + i;
        let vm = match (rect.r1 > rect.r0, i) {
            (false, _) => VMerge::None,
            (true, 0) => VMerge::Restart,
            (true, _) => VMerge::Continue,
        };
        let cell = shape(&t.rows[r].cells[k0], span, vm, w);
        nt.rows[r].cells.splice(k0..k0 + n, [cell]);
        let paras = if i == 0 {
            std::mem::take(&mut text)
        } else {
            let cref = CellRef {
                table: id,
                row: r as u32,
                cell: k0 as u32,
            };
            vec![Document::cell_paragraph(cref, grid[r][k0].first())]
        };
        ng[r].splice(k0..k0 + n, [paras]);
    }
    (nt, ng, (rect.r0, runs[0].0))
}

/// Split cell `(r, c)` of `(t, grid)` into `rows` × `columns`: the new
/// table and grid and the first new cell.
#[allow(clippy::too_many_arguments)]
fn split(
    t: &Table,
    grid: &Grid,
    id: u32,
    cw: u32,
    (r, c): (usize, usize),
    rows: usize,
    columns: usize,
    keep_text: bool,
) -> Result<Split, &'static str> {
    let (top, bottom) = group(t, r, c);
    let g = t.grid_col(r, c);
    let ci_top = cell_starting(t, top, g).unwrap_or(c);
    let h = bottom - top + 1;
    if rows > 1 && h > 1 && h % rows != 0 {
        return Err(ROWS_DIVIDE);
    }
    let span = t.rows[top].cells[ci_top].span.max(1) as usize;
    let widths = t.column_widths(cw);
    let xs = edges(&widths);
    let (x0, x1) = (xs[g], xs[(g + span).min(xs.len() - 1)]);
    let pts: Vec<u64> = (0..=columns)
        .map(|k| x0 + (x1 - x0) * k as u64 / columns as u64)
        .collect();
    let mut bounds = xs.clone();
    for p in &pts[1..columns] {
        if !bounds.iter().any(|b| b.abs_diff(*p) <= 1) {
            bounds.push(*p);
        }
    }
    bounds.sort_unstable();
    if bounds.len() - 1 > MAX_COLUMNS {
        return Err(TOO_MANY_COLUMNS);
    }
    let idx = |x: u64| {
        bounds
            .iter()
            .position(|b| b.abs_diff(x) <= 1)
            .unwrap_or(bounds.len() - 1)
    };
    let mut nt = t.clone();
    let mut ng = grid.clone();
    nt.grid = bounds.windows(2).map(|w| (w[1] - w[0]) as u32).collect();
    // Cells across new grid columns span them.
    for (ri, row) in t.rows.iter().enumerate() {
        for (ci, cell) in row.cells.iter().enumerate() {
            let (a, b) = cols(t, ri, ci);
            let (a, b) = (xs[a.min(xs.len() - 1)], xs[b.min(xs.len() - 1)]);
            let ns = idx(b) - idx(a);
            if ns != cell.span.max(1) as usize && ns > 0 {
                nt.rows[ri].cells[ci] = shape(cell, ns, cell.vmerge, (b - a) as u32);
            }
        }
    }
    let src = t.rows[top].cells[ci_top].clone();
    let piece = |k: usize, vm: VMerge| {
        let ns = (idx(pts[k + 1]) - idx(pts[k])).max(1);
        let mut cell = shape(&src, ns, vm, (pts[k + 1] - pts[k]) as u32);
        if let Some(x) = cell.tc_pr.as_deref() {
            let x = strip_element(&strip_element(x, "cellIns"), "cellDel");
            cell.tc_pr = Some(x.into());
        }
        cell
    };
    let like = grid[top][ci_top].first().cloned();
    let empty = |row: usize, cell: usize| {
        let cref = CellRef {
            table: id,
            row: row as u32,
            cell: cell as u32,
        };
        vec![Document::cell_paragraph(cref, like.as_ref())]
    };
    // The visible new cells, in reading order, get the paragraphs.
    let text = grid[top][ci_top].clone();
    let slots = rows * columns;
    let mut fill: Vec<Vec<Paragraph>> = vec![Vec::new(); slots];
    let mut added: Vec<At> = Vec::new();
    let mut added_rows: Vec<usize> = Vec::new();
    if text.len() <= 1 || keep_text {
        fill[0] = text;
    } else {
        for (i, p) in text.into_iter().enumerate() {
            fill[i.min(slots - 1)].push(p);
        }
    }
    let mut fill = fill.into_iter();
    if rows > 1 && h == 1 {
        // New rows below; the row's other cells continue down into them.
        let base: Vec<Cell> = nt.rows[top].cells.clone();
        let mut cells = Vec::new();
        let mut paras = Vec::new();
        for (ci, cell) in base.iter().enumerate() {
            if ci == ci_top {
                for k in 0..columns {
                    if k > 0 {
                        added.push((top, cells.len()));
                    }
                    cells.push(piece(k, VMerge::None));
                    let p = fill.next().unwrap_or_default();
                    paras.push(if p.is_empty() { empty(top, ci) } else { p });
                }
            } else {
                let vm = match cell.vmerge {
                    VMerge::None => VMerge::Restart,
                    v => v,
                };
                let w = cell.width.unwrap_or(0);
                cells.push(if vm == cell.vmerge {
                    cell.clone()
                } else {
                    shape(cell, cell.span.max(1) as usize, vm, w)
                });
                paras.push(ng[top][ci].clone());
            }
        }
        let src_row = &t.rows[top];
        let new_rows: Vec<(Row, Vec<Vec<Paragraph>>)> = (1..rows)
            .map(|_| {
                let mut row = Row {
                    tr_pr: src_row.tr_pr.as_deref().map(|x| {
                        strip_element(&strip_element(&strip_element(x, "tblHeader"), "ins"), "del")
                            .into()
                    }),
                    tbl_pr_ex: src_row.tbl_pr_ex.clone(),
                    height: src_row.height,
                    header: false,
                    cant_split: src_row.cant_split,
                    cells: Vec::new(),
                };
                let mut ps = Vec::new();
                for (ci, cell) in base.iter().enumerate() {
                    if ci == ci_top {
                        for k in 0..columns {
                            row.cells.push(piece(k, VMerge::None));
                            let p = fill.next().unwrap_or_default();
                            ps.push(if p.is_empty() { empty(top, ci) } else { p });
                        }
                    } else {
                        let w = cell.width.unwrap_or(0);
                        row.cells
                            .push(shape(cell, cell.span.max(1) as usize, VMerge::Continue, w));
                        ps.push(empty(top, ci));
                    }
                }
                (row, ps)
            })
            .collect();
        nt.rows[top].cells = cells;
        ng[top] = paras;
        for (k, (row, ps)) in new_rows.into_iter().enumerate() {
            added_rows.push(top + 1 + k);
            nt.rows.insert(top + 1 + k, row);
            ng.insert(top + 1 + k, ps);
        }
    } else {
        let per = if rows > 1 { h / rows } else { h };
        for i in 0..h {
            let row = top + i;
            let Some(ci) = cell_starting(t, row, g) else {
                continue;
            };
            let vm = if rows == 1 {
                t.rows[row].cells[ci].vmerge
            } else if per == 1 {
                VMerge::None
            } else if i % per == 0 {
                VMerge::Restart
            } else {
                VMerge::Continue
            };
            let visible = i % per == 0;
            let mut paras = Vec::new();
            for k in 0..columns {
                let p = if visible {
                    fill.next().unwrap_or_default()
                } else if k == 0 && i > 0 {
                    ng[row][ci].clone()
                } else {
                    Vec::new()
                };
                paras.push(if p.is_empty() { empty(row, ci) } else { p });
            }
            added.extend((1..columns).map(|k| (row, ci + k)));
            nt.rows[row]
                .cells
                .splice(ci..ci + 1, (0..columns).map(|k| piece(k, vm)));
            ng[row].splice(ci..ci + 1, paras);
        }
    }
    Ok(Split {
        table: nt,
        grid: ng,
        focus: (top, ci_top),
        added,
        added_rows,
    })
}

/// What [`split`] made: the table and its text, the first new cell, and
/// the cells (row, cell) and rows it added.
struct Split {
    table: Table,
    grid: Grid,
    focus: At,
    added: Vec<At>,
    added_rows: Vec<usize>,
}

impl Document {
    /// The cells the selection's ends are in: table id and (row, cell) of
    /// each.
    fn cell_ends(&self, sel: Selection) -> Option<(u32, At, At)> {
        let a = self.cell_at(sel.anchor)?;
        let b = self.cell_at(sel.head)?;
        (a.table == b.table).then_some((
            a.table,
            (a.row as usize, a.cell as usize),
            (b.row as usize, b.cell as usize),
        ))
    }

    /// Whether Merge Cells applies: the selection covers more than one cell
    /// of a table.
    pub fn can_merge_cells(&self, sel: Selection) -> bool {
        let sel = self.clamp_selection(sel);
        self.cell_ends(sel).is_some_and(|(id, a, b)| {
            rect(&self.tables[&id], a, b)
                .is_some_and(|(_, runs)| runs.len() > 1 || runs.first().is_some_and(|r| r.1 > 1))
        })
    }

    /// Merge the selected cells (one undo step). Returns the caret after
    /// and whether the merge went in untracked with Track Changes on.
    pub fn merge_cells(&mut self, sel: Selection) -> Result<(Selection, bool), &'static str> {
        let sel = self.clamp_selection(sel);
        let Some((id, a, b)) = self.cell_ends(sel) else {
            return Err(if self.cell_at(sel.head).is_some() {
                NOT_RECTANGLE
            } else {
                NOT_IN_TABLE
            });
        };
        let t = self.tables[&id].clone();
        let Some((rect, runs)) = rect(&t, a, b) else {
            return Err(NOT_RECTANGLE);
        };
        if runs.len() == 1 && runs[0].1 == 1 {
            return Err(SELECT_CELLS);
        }
        let (range, grid) = self.grid(id);
        let (nt, ng, focus, untracked) = if self.track {
            let (nt, ng) = self.tracked_merge(&t, grid, id, range.start, rect, &runs);
            (nt, ng, (rect.r0, runs[0].0), false)
        } else {
            let (nt, ng, focus) = merged(&t, &grid, id, self.content_width(), rect, &runs);
            (nt, ng, focus, self.track)
        };
        Ok((self.commit_table(id, range, nt, ng, focus, sel), untracked))
    }

    /// Split the cell at the caret (the selected cells merged first, as
    /// Word's "Merge cells before split") into `rows` × `columns` (one undo
    /// step). Returns the caret after and whether it went in untracked with
    /// Track Changes on.
    pub fn split_cells(
        &mut self,
        sel: Selection,
        rows: usize,
        columns: usize,
    ) -> Result<(Selection, bool), &'static str> {
        let sel = self.clamp_selection(sel);
        let Some((id, a, b)) = self.cell_ends(sel) else {
            return Err(NOT_IN_TABLE);
        };
        let rows = rows.clamp(1, MAX_SPLIT_ROWS);
        let columns = columns.clamp(1, MAX_COLUMNS);
        let (range, grid) = self.grid(id);
        let mut t = self.tables[&id].clone();
        let mut grid = grid;
        let mut at = b;
        if a != b {
            let Some((rect, runs)) = rect(&t, a, b) else {
                return Err(NOT_RECTANGLE);
            };
            if runs.len() > 1 || runs[0].1 > 1 {
                let (nt, ng, focus) = merged(&t, &grid, id, self.content_width(), rect, &runs);
                (t, grid, at) = (nt, ng, focus);
            }
        }
        if rows == 1 && columns == 1 && a == b {
            return Ok((sel, false));
        }
        // Tracked as the cells and rows it adds, unless cells were merged
        // first (that goes in untracked, as Word's).
        let tracked = self.track && a == b;
        let mut sp = split(
            &t,
            &grid,
            id,
            self.content_width(),
            at,
            rows,
            columns,
            tracked,
        )?;
        if tracked {
            let el = |kind, id| self.row_rev_el(kind, id);
            let mut id_now = self.max_rev_id();
            for &r in &sp.added_rows {
                id_now += 1;
                crate::track_table::set_row_rev(
                    &mut sp.table.rows[r],
                    &el(RevKind::Insert, id_now),
                );
            }
            let old = self.tables[&id].clone();
            self.track_inserted_cells_in(&old, &mut sp.table, &sp.added, &sp.added_rows, id_now);
        }
        let untracked = self.track && !tracked;
        Ok((
            self.commit_table(id, range, sp.table, sp.grid, sp.focus, sel),
            untracked,
        ))
    }

    /// Put table `id` (paragraphs `range`) back as `t` with `grid`'s text:
    /// one undo step, the caret in cell `focus`.
    fn commit_table(
        &mut self,
        id: u32,
        range: std::ops::Range<usize>,
        t: Table,
        grid: Grid,
        focus: (usize, usize),
        sel: Selection,
    ) -> Selection {
        let out = Self::flatten(id, grid);
        let cell = CellRef {
            table: id,
            row: focus.0 as u32,
            cell: focus.1 as u32,
        };
        let caret = Self::first_of(&out, cell).unwrap_or(0) + range.start;
        let after = Selection::caret(Position::new(caret, 0));
        let mut tables = self.tables.clone();
        tables.insert(id, t);
        self.replace_with_tables(
            range.start,
            range.len(),
            out,
            Some(tables),
            (sel, after),
            Kind::Other,
            true,
        );
        after
    }

    /// A merge down one column with Track Changes on: each cell whose
    /// merge state changes gets a `w:cellMerge` (new state, old state), and
    /// the text of the cells below is struck where it is and inserted at
    /// the end of the top cell, as Word records it.
    fn tracked_merge(
        &self,
        t: &Table,
        mut grid: Grid,
        id: u32,
        first: usize,
        rect: Rect,
        runs: &[(usize, usize)],
    ) -> (Table, Grid) {
        let mut nt = t.clone();
        let mut ids = self.max_rev_id();
        let date = now_utc();
        let author = xml::escape(&self.author);
        for (i, &(k, _)) in runs.iter().enumerate() {
            if runs.len() < 2 {
                break;
            }
            let r = rect.r0 + i;
            let cell = &t.rows[r].cells[k];
            let now = if i == 0 {
                VMerge::Restart
            } else {
                VMerge::Continue
            };
            // A cell merged before (tracked) keeps its first state.
            let was = crate::track_table::cell_merge(cell)
                .and_then(xml::parse)
                .map_or(cell.vmerge, |el| {
                    VMerge::from_annotation(el.attr("vMergeOrig"))
                });
            if cell.vmerge == now && crate::track_table::cell_merge(cell).is_none() {
                continue;
            }
            let x = cell.tc_pr.as_deref().unwrap_or("<w:tcPr></w:tcPr>");
            let x = set_tc_child(x, "vMerge", None);
            let rec = if was == now {
                None
            } else {
                ids += 1;
                let orig = was
                    .annotation()
                    .map(|o| format!(" w:vMergeOrig=\"{o}\""))
                    .unwrap_or_default();
                Some(format!(
                    "<w:cellMerge w:id=\"{ids}\" w:vMerge=\"{}\"{orig} w:author=\"{author}\" w:date=\"{date}\"/>",
                    now.annotation().unwrap_or("cont")
                ))
            };
            let x = match rec {
                Some(rec) => set_tc_child(&x, "cellMerge", Some(rec)),
                None => set_tc_child(
                    &set_tc_child(&x, "cellMerge", None),
                    "vMerge",
                    now.element(),
                ),
            };
            let c = &mut nt.rows[r].cells[k];
            c.tc_pr = Some(x.into());
            c.refresh();
        }
        // Across columns: the row's cells marked merged (`w:hMerge`), the
        // properties before recorded (`w:tcPrChange`).
        let tag = |id: u64| format!("w:id=\"{id}\" w:author=\"{author}\" w:date=\"{date}\"");
        for (i, &(k0, n)) in runs.iter().enumerate() {
            if n < 2 {
                continue;
            }
            let r = rect.r0 + i;
            for k in k0..k0 + n {
                let old = t.rows[r].cells[k].tc_pr.as_deref();
                let c = &mut nt.rows[r].cells[k];
                let x = c.tc_pr.as_deref().unwrap_or("<w:tcPr></w:tcPr>");
                let state = if k == k0 {
                    "<w:hMerge w:val=\"restart\"/>"
                } else {
                    "<w:hMerge/>"
                };
                let x = set_tc_child(x, "hMerge", Some(state.into()));
                ids += 1;
                let x = crate::track_table::record_prop(old, &x, "tcPr", "tcPrChange", &tag(ids));
                c.tc_pr = Some(x.into());
                c.refresh();
            }
        }
        // The other cells' text moves to the first: struck there, inserted
        // at the end of the first cell.
        let ins = {
            ids += 1;
            self.new_rev(RevKind::Insert, ids)
        };
        let mut copies: Vec<Paragraph> = Vec::new();
        let others = runs.iter().enumerate().flat_map(|(i, &(k0, n))| {
            (k0..k0 + n)
                .filter(move |&k| i > 0 || k > k0)
                .map(move |k| (i, k))
        });
        for (i, k) in others {
            let r = rect.r0 + i;
            let Some((a, b)) =
                crate::track_table::cell_paras(&self.paragraphs[first..], id, r as u32, k as u32)
            else {
                continue;
            };
            let own: Vec<Paragraph> = self.paragraphs[first + a..=first + b].to_vec();
            let visible: Vec<Paragraph> = own.iter().map(without_deleted).collect();
            if !has_text(&visible) {
                continue;
            }
            let last = &self.paragraphs[first + b];
            let (struck, _) = self.tracked_cut(
                Position::new(first + a, 0),
                Position::new(first + b, last.text.len()),
                &mut ids,
            );
            grid[r][k] = struck;
            copies.extend(visible);
        }
        if copies.is_empty() {
            return (nt, grid);
        }
        let mark = format!("{}/>", ins.tag.trim_end_matches('>').trim_end_matches('/'));
        for p in &mut copies {
            crate::track::strip_mark(p, &["ins", "del", "moveFrom", "moveTo"]);
            for run in &mut p.runs {
                if run.len > 0 {
                    run.rev = Some(ins.clone());
                }
            }
        }
        let n = copies.len();
        for p in copies.iter_mut().take(n - 1) {
            crate::track::set_mark_rev(p, &mark);
        }
        let k0 = runs[0].0;
        let top = &mut grid[rect.r0][k0];
        if top.len() == 1 && top[0].text.is_empty() {
            *top = copies;
        } else {
            if let Some(last) = top.last_mut() {
                crate::track::set_mark_rev(last, &mark);
            }
            top.extend(copies);
        }
        (nt, grid)
    }
}

/// `p` without its text marked deleted.
fn without_deleted(p: &Paragraph) -> Paragraph {
    let mut out = p.clone();
    let gone: Vec<std::ops::Range<usize>> = p
        .run_ranges()
        .filter(|(_, r)| r.rev.as_ref().is_some_and(|v| v.kind == RevKind::Delete))
        .map(|(range, _)| range)
        .collect();
    for range in gone.into_iter().rev() {
        out.delete(range);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tc_pr_children_go_in_schema_order() {
        let x = "<w:tcPr><w:tcW w:w=\"10\" w:type=\"dxa\"/><w:vAlign w:val=\"top\"/></w:tcPr>";
        let y = set_tc_child(x, "gridSpan", Some("<w:gridSpan w:val=\"2\"/>".into()));
        let y = set_tc_child(&y, "vMerge", VMerge::Restart.element());
        assert_eq!(
            y,
            "<w:tcPr><w:tcW w:w=\"10\" w:type=\"dxa\"/><w:gridSpan w:val=\"2\"/><w:vMerge w:val=\"restart\"/><w:vAlign w:val=\"top\"/></w:tcPr>"
        );
    }

    #[test]
    fn merge_rectangles_grow_over_merged_cells() {
        // Row 0: a(span 2) b; row 1: c d e.
        let mut t = Table::new(2, 3, 9000);
        t.rows[0].cells.remove(1);
        t.rows[0].cells[0] = Table::new_cell(6000, 2);
        // Selecting row 1's second cell and row 0's first takes all of a.
        let (r, runs) = rect(&t, (1, 1), (0, 0)).unwrap();
        assert_eq!((r.r0, r.r1, r.g0, r.g1), (0, 1, 0, 2));
        assert_eq!(runs, vec![(0, 1), (0, 2)]);
    }
}
