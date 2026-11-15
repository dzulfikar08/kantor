//! Tracked table rows (Word's `w:trPr/w:ins` and `w:trPr/w:del`).
//!
//! With Track Changes on, a row inserted in Kantor is marked inserted (its
//! `w:trPr` gets a `w:ins` by the user) and deleting a row marks it deleted
//! (`w:del`) with its text struck through, instead of removing it, as Word
//! does. Accepting an inserted row or rejecting a deleted one just drops
//! the mark; accepting a deleted row or rejecting an inserted one removes
//! the row (the whole table when it was the last one). Rows marked in files
//! from Word are read and resolved the same way.

use crate::document::{Document, Kind};
use crate::model::{CellRef, Paragraph, Position, Selection};
use crate::review::{RevKind, now_utc};
use crate::table::{Row, Tables};
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
    matches!(name, "ins" | "del")
}
