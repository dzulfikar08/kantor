//! Tracked table rows (Word's `w:trPr/w:ins` and `w:trPr/w:del`).
//!
//! With Track Changes on, a row inserted in Kantor is marked inserted (its
//! `w:trPr` gets a `w:ins` by the user) and deleting a row marks it deleted
//! (`w:del`) with its text struck through, instead of removing it, as Word
//! does. Accepting an inserted row or rejecting a deleted one just drops
//! the mark; accepting a deleted row or rejecting an inserted one removes
//! the row (the whole table when it was the last one). Rows marked in files
//! from Word are read and resolved the same way.
//!
//! Columns work as Word's: an inserted column's cells are marked inserted
//! (`w:tcPr/w:cellIns`) and the widths it changed recorded as property
//! changes (`w:tcPrChange`, `w:tblGridChange`); deleting a column marks
//! its cells deleted (`w:cellDel`) with their text struck. Property
//! changes in files (`w:tblPrChange`, `w:trPrChange`, `w:tcPrChange`,
//! `w:tblGridChange`) are shown as one formatting change per table and
//! accepted (the record dropped) or rejected (the old properties back).
//! Merged-cell changes (`w:cellMerge`) aren't modelled.

use crate::document::{Document, Kind};
use crate::model::{CellRef, Paragraph, Position, Selection};
use crate::review::{RevKind, now_utc};
use crate::table::{Cell, Row, Table, Tables, strip_element};
use crate::xml;

/// A row's tracked insertion or deletion: kind, author, date.
pub(crate) fn row_rev(row: &Row) -> Option<(RevKind, String, String)> {
    let el = xml::parse(row.tr_pr.as_deref()?)?;
    el.children.iter().find_map(|c| {
        let kind = match c.name.as_str() {
            "ins" => RevKind::Insert,
            "del" => RevKind::Delete,
            _ => return None,
        };
        Some((
            kind,
            c.attr("author").unwrap_or_default().to_owned(),
            c.attr("date").unwrap_or_default().to_owned(),
        ))
    })
}

/// Put a `<w:ins …/>` / `<w:del …/>` on a row (replacing one it had).
pub(crate) fn set_row_rev(row: &mut Row, el: &str) {
    clear_row_rev(row);
    let tr = match row.tr_pr.as_deref() {
        None => format!("<w:trPr>{el}</w:trPr>"),
        Some(x) if !x.contains("</w:trPr>") => format!("<w:trPr>{el}</w:trPr>"),
        Some(x) => x.replacen("</w:trPr>", &format!("{el}</w:trPr>"), 1),
    };
    row.tr_pr = Some(tr.into());
}

/// Drop a row's `w:ins` / `w:del`.
pub(crate) fn clear_row_rev(row: &mut Row) {
    let Some(x) = row.tr_pr.as_deref() else {
        return;
    };
    let mut s = x.to_owned();
    for name in ["ins", "del"] {
        let pat = format!("<w:{name} ");
        while let Some(i) = s.find(&pat) {
            let end = s[i..].find("/>").map_or(s.len(), |e| i + e + 2);
            s.replace_range(i..end, "");
        }
        s = s.replace(&format!("<w:{name}/>"), "");
    }
    row.tr_pr = if s.replace(char::is_whitespace, "") == "<w:trPr></w:trPr>" {
        None
    } else {
        Some(s.into())
    };
}

impl Document {
    /// A row revision element by the user.
    pub(crate) fn row_rev_el(&self, kind: RevKind, id: u64) -> String {
        let name = if kind == RevKind::Insert {
            "ins"
        } else {
            "del"
        };
        format!(
            "<w:{name} w:id=\"{id}\" w:author=\"{}\" w:date=\"{}\"/>",
            xml::escape(&self.author),
            now_utc()
        )
    }

    /// The paragraph range `[first, last]` of table row `row` of `id`.
    pub(crate) fn row_paras(paras: &[Paragraph], id: u32, row: u32) -> Option<(usize, usize)> {
        let first = paras
            .iter()
            .position(|p| p.cell.is_some_and(|c| c.table == id && c.row == row))?;
        let n = paras[first..]
            .iter()
            .take_while(|p| p.cell.is_some_and(|c| c.table == id && c.row == row))
            .count();
        Some((first, first + n - 1))
    }

    /// Delete rows `rows` of table `id` with Track Changes on: each is
    /// marked deleted and its text struck through (rows the user inserted
    /// go at once, as in Word). Returns the selection afterwards.
    pub(crate) fn tracked_delete_rows(
        &mut self,
        id: u32,
        rows: &[usize],
        sel: Selection,
    ) -> Selection {
        let range = self.table_range(id);
        let mut t = self.tables[&id].clone();
        let mut ids = self.max_rev_id();
        let mut paras: Vec<Paragraph> = self.paragraphs[range.clone()].to_vec();
        let mut own = Vec::new();
        let mut end = Position::new(range.start, 0);
        for &r in rows {
            let Some(row) = t.rows.get_mut(r) else {
                continue;
            };
            match row_rev(row) {
                Some((RevKind::Insert, au, _)) if au == self.author => {
                    own.push(r);
                    continue;
                }
                Some((RevKind::Delete, _, _)) => continue,
                _ => {}
            }
            let Some((a, b)) = Self::row_paras(&paras, id, r as u32) else {
                continue;
            };
            // Each cell's text struck (cells can't merge, so this keeps
            // every paragraph).
            for (k, slot) in paras.iter_mut().enumerate().take(b + 1).skip(a) {
                let i = range.start + k;
                let p = &self.paragraphs[i];
                if p.text.is_empty() {
                    continue;
                }
                let (cut, _) = self.tracked_cut(
                    Position::new(i, 0),
                    Position::new(i, p.text.len()),
                    &mut ids,
                );
                if let Some(c) = cut.into_iter().next() {
                    *slot = c;
                }
            }
            ids += 1;
            let el = self.row_rev_el(RevKind::Delete, ids);
            set_row_rev(row, &el);
            end = Position::new(range.start + b, paras[b].text.len());
        }
        let mut tables = self.tables.clone();
        tables.insert(id, t);
        remove_rows(&mut paras, &mut tables, id, &own);
        let lone = paras.is_empty() && self.paragraphs.len() == range.len();
        if lone {
            paras.push(Paragraph::default());
        }
        let total = self.paragraphs.len() - range.len() + paras.len();
        let mut after = Selection::caret(end);
        if !own.is_empty() || after.head.para >= total {
            after = Selection::caret(Position::new(range.start.min(total - 1), 0));
        }
        let n = range.len();
        self.replace_with_tables(
            range.start,
            n,
            paras,
            Some(tables),
            (sel, after),
            Kind::Other,
            true,
        );
        self.clamp_selection(after)
    }
}

/// Remove rows `rows` of table `id` from `paras` (which hold the whole
/// table) and from `tables`, renumbering the rows after them; a table left
/// without rows is removed.
pub(crate) fn remove_rows(
    paras: &mut Vec<Paragraph>,
    tables: &mut Tables,
    id: u32,
    rows: &[usize],
) {
    let Some(t) = tables.get_mut(&id) else {
        return;
    };
    let mut rows = rows.to_vec();
    rows.sort_unstable();
    rows.dedup();
    for &r in rows.iter().rev() {
        if r >= t.rows.len() {
            continue;
        }
        t.rows.remove(r);
        paras.retain(|p| !p.cell.is_some_and(|c| c.table == id && c.row as usize == r));
        for p in paras.iter_mut() {
            if let Some(c) = &mut p.cell
                && c.table == id
                && c.row as usize > r
            {
                *c = CellRef {
                    row: c.row - 1,
                    ..*c
                };
            }
        }
    }
    if t.rows.is_empty() {
        tables.remove(&id);
        paras.retain(|p| !p.cell.is_some_and(|c| c.table == id));
    }
}

/// Resolve the tracked rows of the tables in `paras` (`first` is the
/// index of `paras[0]` in the document) that touch `start..=end` (all of
/// them when `None`), keeping `only` kinds when given. Returns whether a
/// row changed.
pub(crate) fn resolve_rows(
    paras: &mut Vec<Paragraph>,
    tables: &mut Tables,
    first: usize,
    span: Option<(Position, Position)>,
    accept: bool,
    only: Option<RevKind>,
) -> bool {
    let ids: Vec<u32> = {
        let mut v: Vec<u32> = paras
            .iter()
            .filter_map(|p| p.cell.map(|c| c.table))
            .collect();
        v.dedup();
        v
    };
    let mut changed = false;
    for id in ids {
        let Some(t) = tables.get(&id) else { continue };
        let mut gone = Vec::new();
        let mut clear = Vec::new();
        for (ri, row) in t.rows.iter().enumerate() {
            let Some((kind, _, _)) = row_rev(row) else {
                continue;
            };
            if only.is_some_and(|k| k != kind) {
                continue;
            }
            let Some((a, b)) = Document::row_paras(paras, id, ri as u32) else {
                continue;
            };
            if let Some((s, e)) = span {
                let rs = Position::new(first + a, 0);
                let re = Position::new(first + b, paras[b].text.len());
                if re < s || rs > e {
                    continue;
                }
            }
            if (kind == RevKind::Delete) == accept {
                gone.push(ri);
            } else {
                clear.push(ri);
            }
        }
        if gone.is_empty() && clear.is_empty() {
            continue;
        }
        changed = true;
        if let Some(t) = tables.get_mut(&id) {
            for &ri in &clear {
                clear_row_rev(&mut t.rows[ri]);
            }
        }
        remove_rows(paras, tables, id, &gone);
    }
    changed
}

/// Shared by the reader: whether `tr_pr` holds only revision marks Kantor
/// handles (`w:ins`, `w:del`).
pub(crate) fn is_row_mark(name: &str) -> bool {
    matches!(name, "ins" | "del" | "trPrChange")
}

// ── inserted and deleted cells, property changes ──────────────────────────

/// A tracked change to a table other than its text: rows or cells inserted
/// or deleted, or its properties changed (table, rows or cells: one
/// change per table, as Word's "Formatted Table" balloon).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TableChange {
    Row,
    Cell,
    Props,
}

/// The first element `name` in a property element's XML, as written.
fn el_in<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    crate::float::element_range(xml, name).map(|r| &xml[r])
}

/// A cell's tracked insertion or deletion (`w:cellIns` / `w:cellDel`):
/// kind, author, date.
pub(crate) fn cell_rev(cell: &Cell) -> Option<(RevKind, String, String)> {
    let x = cell.tc_pr.as_deref()?;
    // Only the cell's own (not the old properties of a `w:tcPrChange`).
    let own = match x.find("<w:tcPrChange") {
        Some(i) => &x[..i],
        None => x,
    };
    [("cellIns", RevKind::Insert), ("cellDel", RevKind::Delete)]
        .into_iter()
        .find_map(|(name, kind)| {
            let el = el_in(own, name)?;
            let (a, d) = crate::fmt_track::who_when(el);
            Some((kind, a, d))
        })
}

/// `xml` (a property element `<w:outer>…</w:outer>`, or none) with `el`
/// added at its end, before `before` when it has one.
fn add_to_props(xml: Option<&str>, outer: &str, el: &str, before: Option<&str>) -> String {
    let close = format!("</w:{outer}>");
    let x = match xml {
        Some(x) if x.contains(&close) => x.to_owned(),
        _ => return format!("<w:{outer}>{el}{close}"),
    };
    let at = before
        .and_then(|b| x.find(&format!("<w:{b}")))
        .or_else(|| x.rfind(&close))
        .unwrap_or(x.len());
    let mut out = x;
    out.insert_str(at, el);
    out
}

/// Put a `<w:cellIns …/>` / `<w:cellDel …/>` on a cell (replacing one).
pub(crate) fn set_cell_rev(cell: &mut Cell, el: &str) {
    clear_cell_rev(cell);
    let x = add_to_props(cell.tc_pr.as_deref(), "tcPr", el, Some("tcPrChange"));
    cell.tc_pr = Some(x.into());
}

/// Drop a cell's `w:cellIns` / `w:cellDel`.
pub(crate) fn clear_cell_rev(cell: &mut Cell) {
    let Some(x) = cell.tc_pr.as_deref() else {
        return;
    };
    let s = strip_element(&strip_element(x, "cellIns"), "cellDel");
    cell.tc_pr = Some(s.into());
}

/// The tracked change of a property element (`w:tblPrChange`,
/// `w:trPrChange`, `w:tcPrChange`), as written.
fn prop_change<'a>(xml: Option<&'a str>, name: &str) -> Option<&'a str> {
    el_in(xml?, name)
}

/// `xml` (`<w:outer>…</w:outer>`) with its change `change` resolved:
/// accepted, the record goes; rejected, the old properties it holds come
/// back, with the current `keep` elements (revision marks) kept.
fn resolve_prop(xml: &str, outer: &str, change: &str, accept: bool, keep: &[&str]) -> String {
    let Some(rec) = el_in(xml, change) else {
        return xml.to_owned();
    };
    if accept {
        return strip_element(xml, change);
    }
    // The old properties: the record's inner `<w:outer>`.
    let inner_start = rec.find('>').map_or(rec.len(), |i| i + 1);
    let old = crate::fmt_track::inner(&rec[inner_start..], outer)
        .unwrap_or_else(|| format!("<w:{outer}/>"));
    let mut out = if old.ends_with("/>") {
        format!("<w:{outer}></w:{outer}>")
    } else {
        old
    };
    for k in keep {
        if let Some(el) = el_in(xml, k).filter(|el| !rec.contains(*el)) {
            out = add_to_props(Some(&out), outer, el, None);
        }
    }
    out
}

/// `new` (`<w:outer>…</w:outer>`) with a change record holding `old`
/// (without revision marks), by `author` now. When `old` already has a
/// record it stays (it holds the properties before any change).
fn record_prop(old: Option<&str>, new: &str, outer: &str, change: &str, tag: &str) -> String {
    if let Some(rec) = prop_change(old, change) {
        let base = strip_element(new, change);
        return add_to_props(Some(&base), outer, rec, None);
    }
    let mut was = old.map_or(format!("<w:{outer}/>"), str::to_owned);
    for n in ["cellIns", "cellDel", "cellMerge", "ins", "del", change] {
        was = strip_element(&was, n);
    }
    if was.replace(char::is_whitespace, "") == format!("<w:{outer}></w:{outer}>") {
        was = format!("<w:{outer}/>");
    }
    let rec = format!("<w:{change} {tag}>{was}</w:{change}>");
    add_to_props(Some(new), outer, &rec, None)
}

/// Whether table `t` has tracked property changes (its own, its grid's,
/// a row's or a cell's): who and when (the first found).
pub(crate) fn props_changed(t: &Table) -> Option<(String, String)> {
    let mut recs = Vec::new();
    recs.extend(prop_change(t.tbl_pr.as_deref(), "tblPrChange"));
    recs.extend(t.grid_change.as_deref());
    for r in &t.rows {
        recs.extend(prop_change(r.tr_pr.as_deref(), "trPrChange"));
        for c in &r.cells {
            recs.extend(prop_change(c.tc_pr.as_deref(), "tcPrChange"));
        }
    }
    recs.first().map(|r| crate::fmt_track::who_when(r))
}

/// What table `t`'s property changes changed, for the balloon.
pub(crate) fn props_items(t: &Table) -> Vec<crate::fmt_track::FormatItem> {
    use crate::fmt_track::FormatItem as F;
    let mut out = Vec::new();
    if prop_change(t.tbl_pr.as_deref(), "tblPrChange").is_some() || t.grid_change.is_some() {
        out.push(F::TableProps);
    }
    if t.rows
        .iter()
        .any(|r| prop_change(r.tr_pr.as_deref(), "trPrChange").is_some())
    {
        out.push(F::RowProps);
    }
    if t.rows.iter().flat_map(|r| &r.cells).any(|c| {
        prop_change(c.tc_pr.as_deref(), "tcPrChange").is_some()
    }) {
        out.push(F::CellProps);
    }
    out
}

/// Accept or reject every property change of table `t`. Whether there
/// was one.
pub(crate) fn resolve_props(t: &mut Table, accept: bool) -> bool {
    let mut any = false;
    if let Some(x) = t.tbl_pr.as_deref()
        && prop_change(Some(x), "tblPrChange").is_some()
    {
        t.tbl_pr = Some(resolve_prop(x, "tblPr", "tblPrChange", accept, &[]).into());
        t.refresh();
        any = true;
    }
    if let Some(g) = t.grid_change.take() {
        if !accept && let Some(el) = xml::parse(&g) {
            let grid: Vec<u32> = el
                .child("tblGrid")
                .map(|tg| {
                    tg.children
                        .iter()
                        .filter(|c| c.name == "gridCol")
                        .map(|c| c.num("w").unwrap_or(0))
                        .collect()
                })
                .unwrap_or_default();
            if !grid.is_empty() {
                t.grid = grid;
            }
        }
        any = true;
    }
    for r in &mut t.rows {
        if let Some(x) = r.tr_pr.as_deref()
            && prop_change(Some(x), "trPrChange").is_some()
        {
            r.tr_pr = Some(resolve_prop(x, "trPr", "trPrChange", accept, &["ins", "del"]).into());
            r.refresh();
            any = true;
        }
        for c in &mut r.cells {
            if let Some(x) = c.tc_pr.as_deref()
                && prop_change(Some(x), "tcPrChange").is_some()
            {
                c.tc_pr = Some(
                    resolve_prop(x, "tcPr", "tcPrChange", accept, &["cellIns", "cellDel"]).into(),
                );
                c.refresh();
                any = true;
            }
        }
    }
    any
}

/// The paragraph range `[first, last]` of cell `cell` of row `row` of
/// table `id` in `paras`.
pub(crate) fn cell_paras(paras: &[Paragraph], id: u32, row: u32, cell: u32) -> Option<(usize, usize)> {
    let want = Some(CellRef {
        table: id,
        row,
        cell,
    });
    let first = paras.iter().position(|p| p.cell == want)?;
    let n = paras[first..].iter().take_while(|p| p.cell == want).count();
    Some((first, first + n - 1))
}

/// Remove cells `cells` (row, cell) of table `id` from `paras` (which hold
/// the whole table) and from `tables`: later cells of the row renumbered,
/// a row left without cells removed, and a grid column no row uses any
/// more dropped.
pub(crate) fn remove_cells(
    paras: &mut Vec<Paragraph>,
    tables: &mut Tables,
    id: u32,
    cells: &[(usize, usize)],
) {
    let Some(t) = tables.get_mut(&id) else {
        return;
    };
    let mut cells = cells.to_vec();
    cells.sort_unstable();
    cells.dedup();
    // Grid columns each row loses.
    let mut lost: Vec<Vec<usize>> = vec![Vec::new(); t.rows.len()];
    for &(r, c) in &cells {
        if let Some(cell) = t.rows.get(r).and_then(|row| row.cells.get(c)) {
            let g = t.grid_col(r, c);
            lost[r].extend(g..g + cell.span.max(1) as usize);
        }
    }
    let ncols = t.columns();
    let dead: Vec<usize> = (0..ncols)
        .filter(|g| !t.rows.is_empty() && lost.iter().all(|l| l.contains(g)))
        .collect();
    for &(r, c) in cells.iter().rev() {
        let Some(row) = t.rows.get_mut(r) else {
            continue;
        };
        if c >= row.cells.len() {
            continue;
        }
        row.cells.remove(c);
        paras.retain(|p| {
            !p.cell
                .is_some_and(|x| x.table == id && x.row as usize == r && x.cell as usize == c)
        });
        for p in paras.iter_mut() {
            if let Some(x) = &mut p.cell
                && x.table == id
                && x.row as usize == r
                && x.cell as usize > c
            {
                x.cell -= 1;
            }
        }
    }
    // (Unless the grid already lost them: a rejected `w:tblGridChange`.)
    let widest = t
        .rows
        .iter()
        .map(|r| r.cells.iter().map(|c| c.span.max(1) as usize).sum::<usize>())
        .max()
        .unwrap_or(0);
    for &g in dead.iter().rev() {
        if g < t.grid.len() && t.grid.len() > widest {
            t.grid.remove(g);
        }
    }
    let empty: Vec<usize> = (0..t.rows.len())
        .filter(|&r| t.rows[r].cells.is_empty())
        .collect();
    remove_rows(paras, tables, id, &empty);
}

/// Resolve the inserted and deleted cells of the tables in `paras`
/// (`first`: index of `paras[0]` in the document) that touch `span` (all
/// when `None`), only `only` kinds when given. Whether a cell changed.
pub(crate) fn resolve_cells(
    paras: &mut Vec<Paragraph>,
    tables: &mut Tables,
    first: usize,
    span: Option<(Position, Position)>,
    accept: bool,
    only: Option<RevKind>,
) -> bool {
    let mut ids: Vec<u32> = paras
        .iter()
        .filter_map(|p| p.cell.map(|c| c.table))
        .collect();
    ids.dedup();
    let mut changed = false;
    for id in ids {
        let Some(t) = tables.get(&id) else { continue };
        let mut gone = Vec::new();
        let mut clear = Vec::new();
        for (ri, row) in t.rows.iter().enumerate() {
            for (ci, cell) in row.cells.iter().enumerate() {
                let Some((kind, _, _)) = cell_rev(cell) else {
                    continue;
                };
                if only.is_some_and(|k| k != kind) {
                    continue;
                }
                let Some((a, b)) = cell_paras(paras, id, ri as u32, ci as u32) else {
                    continue;
                };
                if let Some((s, e)) = span {
                    let cs = Position::new(first + a, 0);
                    let ce = Position::new(first + b, paras[b].text.len());
                    if ce < s || cs > e {
                        continue;
                    }
                }
                if (kind == RevKind::Delete) == accept {
                    gone.push((ri, ci));
                } else {
                    clear.push((ri, ci));
                }
            }
        }
        if gone.is_empty() && clear.is_empty() {
            continue;
        }
        changed = true;
        if let Some(t) = tables.get_mut(&id) {
            for &(r, c) in &clear {
                clear_cell_rev(&mut t.rows[r].cells[c]);
            }
        }
        remove_cells(paras, tables, id, &gone);
    }
    changed
}

/// Resolve the property changes of the tables in `paras` that touch
/// `span` (all when `None`). Whether one changed.
pub(crate) fn resolve_table_props(
    paras: &[Paragraph],
    tables: &mut Tables,
    first: usize,
    span: Option<(Position, Position)>,
    accept: bool,
) -> bool {
    let mut ids: Vec<u32> = paras
        .iter()
        .filter_map(|p| p.cell.map(|c| c.table))
        .collect();
    ids.dedup();
    let mut changed = false;
    for id in ids {
        if let Some((s, e)) = span {
            let a = paras.iter().position(|p| p.cell.is_some_and(|c| c.table == id));
            let b = paras.iter().rposition(|p| p.cell.is_some_and(|c| c.table == id));
            let (Some(a), Some(b)) = (a, b) else { continue };
            let ts = Position::new(first + a, 0);
            let te = Position::new(first + b, paras[b].text.len());
            if te < s || ts > e {
                continue;
            }
        }
        if let Some(t) = tables.get_mut(&id) {
            changed |= resolve_props(t, accept);
        }
    }
    changed
}

impl Document {
    /// A cell revision element (`w:cellIns` / `w:cellDel`) by the user.
    pub(crate) fn cell_rev_el(&self, kind: RevKind, id: u64) -> String {
        let name = if kind == RevKind::Insert {
            "cellIns"
        } else {
            "cellDel"
        };
        format!(
            "<w:{name} w:id=\"{id}\" w:author=\"{}\" w:date=\"{}\"/>",
            xml::escape(&self.author),
            now_utc()
        )
    }

    /// With Track Changes on, mark the cells of table `t` (the edited copy
    /// of `old`) that a column insertion added (`added`: row, cell) as
    /// inserted, and record the widths it changed (the old cell and grid
    /// properties) as property changes, as Word does.
    pub(crate) fn track_inserted_cells(&self, old: &Table, t: &mut Table, added: &[(usize, usize)]) {
        let mut id = self.max_rev_id();
        let date = now_utc();
        let tag = |id: u64| {
            format!(
                "w:id=\"{id}\" w:author=\"{}\" w:date=\"{date}\"",
                xml::escape(&self.author)
            )
        };
        for (ri, row) in t.rows.iter_mut().enumerate() {
            let mut k_old = 0;
            for (ci, cell) in row.cells.iter_mut().enumerate() {
                if added.contains(&(ri, ci)) {
                    id += 1;
                    set_cell_rev(cell, &self.cell_rev_el(RevKind::Insert, id));
                    continue;
                }
                let was = old.rows.get(ri).and_then(|r| r.cells.get(k_old));
                k_old += 1;
                let Some(was) = was else { continue };
                if was.tc_pr == cell.tc_pr {
                    continue;
                }
                id += 1;
                let new = cell.tc_pr.as_deref().unwrap_or("<w:tcPr></w:tcPr>").to_owned();
                cell.tc_pr = Some(
                    record_prop(was.tc_pr.as_deref(), &new, "tcPr", "tcPrChange", &tag(id)).into(),
                );
            }
        }
        if t.grid != old.grid && t.grid_change.is_none() {
            id += 1;
            let cols: String = old
                .grid
                .iter()
                .map(|w| format!("<w:gridCol w:w=\"{w}\"/>"))
                .collect();
            t.grid_change =
                Some(format!("<w:tblGridChange w:id=\"{id}\"><w:tblGrid>{cols}</w:tblGrid></w:tblGridChange>").into());
        }
    }

    /// Delete grid column `col` of table `id` with Track Changes on: each
    /// cell in it is marked deleted (`w:cellDel`) and its text struck
    /// through; cells the user inserted go at once. Returns the selection
    /// afterwards.
    pub(crate) fn tracked_delete_column(&mut self, id: u32, col: usize, sel: Selection) -> Selection {
        let range = self.table_range(id);
        let mut t = self.tables[&id].clone();
        let mut ids = self.max_rev_id();
        let mut paras: Vec<Paragraph> = self.paragraphs[range.clone()].to_vec();
        let mut own = Vec::new();
        for ri in 0..t.rows.len() {
            let Some(ci) = (0..t.rows[ri].cells.len()).find(|&ci| t.grid_col(ri, ci) == col) else {
                continue;
            };
            match cell_rev(&t.rows[ri].cells[ci]) {
                Some((RevKind::Insert, au, _)) if au == self.author => {
                    own.push((ri, ci));
                    continue;
                }
                Some((RevKind::Delete, _, _)) => continue,
                _ => {}
            }
            let Some((a, b)) = cell_paras(&paras, id, ri as u32, ci as u32) else {
                continue;
            };
            for (k, slot) in paras.iter_mut().enumerate().take(b + 1).skip(a) {
                let i = range.start + k;
                let p = &self.paragraphs[i];
                if p.text.is_empty() {
                    continue;
                }
                let (cut, _) = self.tracked_cut(
                    Position::new(i, 0),
                    Position::new(i, p.text.len()),
                    &mut ids,
                );
                if let Some(c) = cut.into_iter().next() {
                    *slot = c;
                }
            }
            ids += 1;
            let el = self.cell_rev_el(RevKind::Delete, ids);
            set_cell_rev(&mut t.rows[ri].cells[ci], &el);
        }
        let mut tables = self.tables.clone();
        tables.insert(id, t);
        remove_cells(&mut paras, &mut tables, id, &own);
        let lone = paras.is_empty() && self.paragraphs.len() == range.len();
        if lone {
            paras.push(Paragraph::default());
        }
        let total = self.paragraphs.len() - range.len() + paras.len();
        let caret = sel.head.para.clamp(range.start, range.start + paras.len().max(1) - 1);
        let after = Selection::caret(Position::new(caret.min(total - 1), 0));
        let n = range.len();
        self.replace_with_tables(
            range.start,
            n,
            paras,
            Some(tables),
            (sel, after),
            Kind::Other,
            true,
        );
        self.clamp_selection(after)
    }
}

impl Row {
    /// Whether the row is a tracked insertion or deletion.
    pub fn tracked(&self) -> Option<RevKind> {
        row_rev(self).map(|r| r.0)
    }
}

impl Cell {
    /// Whether the cell is a tracked insertion or deletion.
    pub fn tracked(&self) -> Option<RevKind> {
        cell_rev(self).map(|r| r.0)
    }
}
