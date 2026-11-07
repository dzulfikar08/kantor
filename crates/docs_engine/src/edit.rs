//! Editing commands for lists, tables and pictures. Each is one undo step.

use std::sync::Arc;

use crate::document::{Document, Kind, NewMedia};
use crate::docx::Media;
use crate::model::*;
use crate::numbering::ListKind;
use crate::object::{EMU_PER_PX, ImageFormat, image_dimensions};
use crate::table::{Row, Table, VMerge, strip_element};

/// A table structure change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableEdit {
    RowAbove,
    RowBelow,
    ColumnLeft,
    ColumnRight,
    DeleteRow,
    DeleteColumn,
    DeleteTable,
}

/// Cell contents: rows × cells × paragraphs.
type Grid = Vec<Vec<Vec<Paragraph>>>;

impl Document {
    // ── lists ──────────────────────────────────────────────────────────────

    /// The kind of list paragraph `i` is in.
    pub fn list_kind(&self, i: usize) -> Option<ListKind> {
        let p = self.paragraphs.get(i)?;
        let r = self.styles.effective_num(p)?;
        let link = |id: &str| self.styles.get(id).and_then(|s| s.para.num).map(|n| n.id);
        self.numbering.kind(r.id, &link)
    }

    /// Bulleted / numbered list on for every selected paragraph, or off if
    /// they all already are that kind. Joins an adjacent list of the same
    /// kind, else starts a new one.
    pub fn toggle_list(&mut self, sel: Selection, kind: ListKind) {
        let sel = self.clamp_selection(sel);
        let (start, end) = sel.ordered();
        let range = start.para..=end.para;
        if range.clone().all(|i| self.list_kind(i) == Some(kind)) {
            let out: Vec<Paragraph> = range
                .clone()
                .map(|i| {
                    let mut p = self.paragraphs[i].clone();
                    let mut q = p.clone();
                    q.props.num = None;
                    p.props.num = self
                        .styles
                        .effective_num(&q)
                        .map(|_| NumRef { id: 0, level: 0 });
                    p.props.indent = Indent::default();
                    p
                })
                .collect();
            self.replace(start.para, out.len(), out, (sel, sel), Kind::Other, true);
            return;
        }
        let cell = self.paragraphs[start.para].cell;
        let neighbour = |i: usize| {
            (self.paragraphs.get(i)?.cell == cell && self.list_kind(i) == Some(kind))
                .then(|| self.styles.effective_num(&self.paragraphs[i]))
                .flatten()
        };
        let id = start
            .para
            .checked_sub(1)
            .and_then(neighbour)
            .or_else(|| neighbour(end.para + 1))
            .map(|r| r.id)
            .unwrap_or_else(|| self.numbering.new_list(kind));
        let out: Vec<Paragraph> = range
            .map(|i| {
                let mut p = self.paragraphs[i].clone();
                let level = self.styles.effective_num(&p).map_or(0, |r| r.level);
                p.props.num = Some(NumRef { id, level });
                p.props.indent = Indent::default();
                p
            })
            .collect();
        self.replace(start.para, out.len(), out, (sel, sel), Kind::Other, true);
    }

    /// Move the selected list paragraphs `delta` levels deeper (0–8).
    /// Returns `false` (no change) when none is a list paragraph.
    pub fn change_list_level(&mut self, sel: Selection, delta: i32) -> bool {
        let sel = self.clamp_selection(sel);
        let (start, end) = sel.ordered();
        let mut changed = false;
        let out: Vec<Paragraph> = (start.para..=end.para)
            .map(|i| {
                let mut p = self.paragraphs[i].clone();
                if let Some(r) = self.styles.effective_num(&p) {
                    let level = (r.level as i32 + delta).clamp(0, 8) as u8;
                    if level != r.level {
                        changed = true;
                        p.props.num = Some(NumRef { id: r.id, level });
                    }
                }
                p
            })
            .collect();
        if changed {
            self.replace(start.para, out.len(), out, (sel, sel), Kind::Other, true);
        }
        changed
    }

    /// Enter: a new paragraph, except in an empty list item, which leaves
    /// the list instead.
    pub fn newline(&mut self, sel: Selection, props: Option<RunProps>) -> Selection {
        let sel = self.clamp_selection(sel);
        if sel.is_empty() {
            let p = &self.paragraphs[sel.head.para];
            let empty =
                p.text.chars().all(|c| c == OBJECT) && p.objects().all(|(_, o)| o.is_marker());
            if empty && self.styles.effective_num(p).is_some() {
                self.remove_list_at(sel.head.para, sel);
                return sel;
            }
        }
        self.insert_text(sel, "\n", props)
    }

    // ── tables ─────────────────────────────────────────────────────────────

    /// Width available to body text, in twips.
    pub fn content_width(&self) -> u32 {
        self.page
            .width
            .saturating_sub(self.page.margin_left + self.page.margin_right)
            .max(1440)
    }

    /// A new paragraph for a cell, formatted like `like` (or plain, with no
    /// spacing after, Word's table text default).
    fn cell_paragraph(cell: CellRef, like: Option<&Paragraph>) -> Paragraph {
        let mut p = match like {
            Some(l) => {
                let mut p = Paragraph::styled("", l.runs[0].props.clone());
                p.props = l.props.clone();
                p.props.num = None;
                p.props.extras.remove("sectPr");
                p
            }
            None => {
                let mut p = Paragraph::default();
                p.props.after = Some(0);
                p.props.line = Some(LineSpacing {
                    line: 240,
                    rule: LineRule::Auto,
                });
                p
            }
        };
        p.cell = Some(cell);
        p
    }

    /// Insert a `rows` × `cols` table after the caret's paragraph (or in
    /// place of an empty one). Returns the caret in the first cell, or
    /// `None` inside a table (nested tables aren't supported).
    pub fn insert_table(&mut self, sel: Selection, rows: usize, cols: usize) -> Option<Selection> {
        let sel = self.clamp_selection(sel);
        let at = sel.head.para;
        if self.paragraphs[at].cell.is_some() {
            return None;
        }
        let (rows, cols) = (rows.clamp(1, 63), cols.clamp(1, 63));
        let id = self.tables.keys().max().map_or(1, |m| m + 1);
        let table = Table::new(rows, cols, self.content_width());
        let mut paras = Vec::with_capacity(rows * cols + 2);
        for r in 0..rows {
            for c in 0..cols {
                let cell = CellRef {
                    table: id,
                    row: r as u32,
                    cell: c as u32,
                };
                paras.push(Self::cell_paragraph(cell, None));
            }
        }
        let current = self.paragraphs[at].clone();
        let empty = current.text.is_empty();
        let (out, first) = if empty {
            // Before the empty paragraph, which stays as the one after.
            let mut out = paras;
            out.push(current);
            (out, at)
        } else {
            let mut out = vec![current];
            out.extend(paras);
            let next_is_body = self
                .paragraphs
                .get(at + 1)
                .is_some_and(|p| p.cell.is_none());
            if !next_is_body {
                out.push(Paragraph::default());
            }
            (out, at + 1)
        };
        let mut tables = self.tables.clone();
        tables.insert(id, table);
        let after = Selection::caret(Position::new(first, 0));
        self.replace_with_tables(at, 1, out, Some(tables), (sel, after), Kind::Other, true);
        Some(after)
    }

    /// The cell of the caret.
    pub fn cell_at(&self, pos: Position) -> Option<CellRef> {
        self.paragraphs.get(pos.para)?.cell
    }

    /// A table's paragraphs grouped by row and cell.
    fn grid(&self, id: u32) -> (std::ops::Range<usize>, Grid) {
        let range = self.table_range(id);
        let t = &self.tables[&id];
        let mut grid: Grid = t
            .rows
            .iter()
            .map(|r| vec![Vec::new(); r.cells.len()])
            .collect();
        for p in &self.paragraphs[range.clone()] {
            let c = p.cell.expect("table paragraph");
            if let Some(cell) = grid
                .get_mut(c.row as usize)
                .and_then(|r| r.get_mut(c.cell as usize))
            {
                cell.push(p.clone());
            }
        }
        (range, grid)
    }

    /// Flatten a grid back into paragraphs with renumbered cell refs.
    fn flatten(id: u32, grid: Grid) -> Vec<Paragraph> {
        let mut out = Vec::new();
        for (r, row) in grid.into_iter().enumerate() {
            for (c, cell) in row.into_iter().enumerate() {
                let cref = CellRef {
                    table: id,
                    row: r as u32,
                    cell: c as u32,
                };
                if cell.is_empty() {
                    out.push(Self::cell_paragraph(cref, None));
                }
                for mut p in cell {
                    p.cell = Some(cref);
                    out.push(p);
                }
            }
        }
        out
    }

    /// Index of the first paragraph of a cell in `paras`.
    fn first_of(paras: &[Paragraph], cell: CellRef) -> Option<usize> {
        paras.iter().position(|p| p.cell == Some(cell))
    }

    /// Change the structure of the table at the caret. Returns the new
    /// selection, or an error message (no table, merged cells).
    pub fn edit_table(&mut self, sel: Selection, edit: TableEdit) -> Result<Selection, String> {
        let sel = self.clamp_selection(sel);
        let Some(at) = self.cell_at(sel.head) else {
            return Err("The caret isn't in a table".into());
        };
        let id = at.table;
        let (range, mut grid) = self.grid(id);
        let mut t = self.tables[&id].clone();
        let (r, c) = (at.row as usize, at.cell as usize);
        let focus: (usize, usize);
        match edit {
            TableEdit::DeleteTable => {
                return Ok(self.remove_table(id, range, sel));
            }
            TableEdit::RowAbove | TableEdit::RowBelow => {
                let src = &t.rows[r];
                let mut row = Row {
                    tr_pr: src
                        .tr_pr
                        .as_deref()
                        .map(|x| strip_element(x, "tblHeader").into()),
                    tbl_pr_ex: src.tbl_pr_ex.clone(),
                    height: src.height,
                    header: false,
                    cells: Vec::new(),
                };
                let mut cells = Vec::new();
                for (ci, cell) in src.cells.iter().enumerate() {
                    let mut nc = cell.clone();
                    nc.vmerge = VMerge::None;
                    nc.tc_pr = cell
                        .tc_pr
                        .as_deref()
                        .map(|x| strip_element(x, "vMerge").into());
                    row.cells.push(nc);
                    let like = grid[r].get(ci).and_then(|ps| ps.first());
                    cells.push(vec![Self::cell_paragraph(
                        CellRef {
                            table: id,
                            row: 0,
                            cell: ci as u32,
                        },
                        like,
                    )]);
                }
                let ri = if edit == TableEdit::RowAbove {
                    r
                } else {
                    r + 1
                };
                t.rows.insert(ri, row);
                grid.insert(ri, cells);
                focus = (ri, 0);
            }
            TableEdit::DeleteRow => {
                if t.rows.len() <= 1 {
                    return Ok(self.remove_table(id, range, sel));
                }
                // A merge that started here continues from the next row.
                let removed = t.rows.remove(r);
                grid.remove(r);
                if let Some(next) = t.rows.get_mut(r) {
                    for (ci, cell) in removed.cells.iter().enumerate() {
                        if cell.vmerge == VMerge::Restart
                            && let Some(n) = next.cells.get_mut(ci)
                            && n.vmerge == VMerge::Continue
                        {
                            n.vmerge = VMerge::Restart;
                            n.tc_pr = n.tc_pr.as_deref().map(|x| {
                                strip_element(x, "vMerge")
                                    .replacen(
                                        "<w:tcPr>",
                                        "<w:tcPr><w:vMerge w:val=\"restart\"/>",
                                        1,
                                    )
                                    .into()
                            });
                        }
                    }
                }
                focus = (r.min(t.rows.len() - 1), 0);
            }
            TableEdit::ColumnLeft | TableEdit::ColumnRight | TableEdit::DeleteColumn => {
                if t.has_merges() {
                    return Err(
                        "Column changes in tables with merged cells aren't supported yet".into(),
                    );
                }
                let total: u32 = t.column_widths(self.content_width()).iter().sum();
                let mut widths = t.column_widths(self.content_width());
                if edit == TableEdit::DeleteColumn {
                    if widths.len() <= 1 {
                        return Ok(self.remove_table(id, range, sel));
                    }
                    widths.remove(c);
                    for (row, cells) in t.rows.iter_mut().zip(grid.iter_mut()) {
                        if c < row.cells.len() {
                            row.cells.remove(c);
                            cells.remove(c);
                        }
                    }
                    focus = (r, c.min(widths.len() - 1));
                } else {
                    let ci = if edit == TableEdit::ColumnLeft {
                        c
                    } else {
                        c + 1
                    };
                    let w = widths.get(c).copied().unwrap_or(1440);
                    widths.insert(ci.min(widths.len()), w);
                    for (ri, (row, cells)) in t.rows.iter_mut().zip(grid.iter_mut()).enumerate() {
                        let like = cells.get(c).and_then(|ps| ps.first()).cloned();
                        let at = ci.min(row.cells.len());
                        row.cells.insert(at, Table::new_cell(w, 1));
                        cells.insert(
                            at,
                            vec![Self::cell_paragraph(
                                CellRef {
                                    table: id,
                                    row: ri as u32,
                                    cell: at as u32,
                                },
                                like.as_ref(),
                            )],
                        );
                    }
                    focus = (r, ci);
                }
                // Keep the table's width: scale the columns to the old total.
                let sum: u32 = widths.iter().sum::<u32>().max(1);
                let widths: Vec<u32> = widths
                    .iter()
                    .map(|w| ((*w as u64 * total as u64) / sum as u64) as u32)
                    .collect();
                for row in &mut t.rows {
                    for (cell, w) in row.cells.iter_mut().zip(&widths) {
                        cell.width = Some(*w);
                        cell.tc_pr = Some(with_tcw(cell.tc_pr.as_deref(), *w).into());
                    }
                }
                t.grid = widths;
            }
        }
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
        Ok(after)
    }

    fn remove_table(
        &mut self,
        id: u32,
        range: std::ops::Range<usize>,
        sel: Selection,
    ) -> Selection {
        let mut tables = self.tables.clone();
        tables.remove(&id);
        let at = range.start;
        // Keep a paragraph where the table was if nothing else is left.
        let lone = self.paragraphs.len() == range.len();
        let out = if lone {
            vec![Paragraph::default()]
        } else {
            Vec::new()
        };
        let caret = Position::new(
            at.min(self.paragraphs.len() - range.len() + out.len() - 1),
            0,
        );
        let after = Selection::caret(caret);
        self.replace_with_tables(
            at,
            range.len(),
            out,
            Some(tables),
            (sel, after),
            Kind::Other,
            true,
        );
        self.clamp_selection(after)
    }

    /// The selection Tab (or Shift+Tab) moves to: the next (previous)
    /// cell's text. Tab in the last cell adds a row first.
    pub fn next_cell(&mut self, sel: Selection, forward: bool) -> Option<Selection> {
        let sel = self.clamp_selection(sel);
        let at = self.cell_at(sel.head)?;
        let range = self.table_range(at.table);
        let next = if forward {
            self.paragraphs[sel.head.para..range.end]
                .iter()
                .position(|p| p.cell != Some(at))
                .map(|k| sel.head.para + k)
        } else {
            let first = self.paragraphs[range.start..=sel.head.para]
                .iter()
                .rposition(|p| p.cell != Some(at))
                .map(|k| range.start + k);
            first.map(|last_of_prev| {
                let c = self.paragraphs[last_of_prev].cell;
                let mut i = last_of_prev;
                while i > range.start && self.paragraphs[i - 1].cell == c {
                    i -= 1;
                }
                i
            })
        };
        let start = match next {
            Some(i) => i,
            None if forward => {
                let t = &self.tables[&at.table];
                // Down to the last row, then add one below it.
                let last_row = t.rows.len() - 1;
                let last = CellRef {
                    table: at.table,
                    row: last_row as u32,
                    cell: 0,
                };
                let p = Self::first_of(&self.paragraphs[range.clone()], last)? + range.start;
                return self
                    .edit_table(Selection::caret(Position::new(p, 0)), TableEdit::RowBelow)
                    .ok();
            }
            None => return None,
        };
        let cell = self.paragraphs[start].cell;
        let mut end = start;
        while end + 1 < self.paragraphs.len() && self.paragraphs[end + 1].cell == cell {
            end += 1;
        }
        Some(Selection {
            anchor: Position::new(start, 0),
            head: Position::new(end, self.paragraphs[end].text.len()),
        })
    }

    // ── pictures ───────────────────────────────────────────────────────────

    /// Insert a PNG or JPEG at the selection, scaled down to fit the text
    /// width. `name` is the file name (for alt text).
    pub fn insert_image(
        &mut self,
        sel: Selection,
        bytes: Vec<u8>,
        name: &str,
    ) -> anyhow::Result<Selection> {
        let format = ImageFormat::sniff(&bytes);
        if format == ImageFormat::Other {
            anyhow::bail!("{name} isn't a PNG or JPEG picture");
        }
        let (w, h) = image_dimensions(&bytes)
            .ok_or_else(|| anyhow::anyhow!("Can't read the size of {name}"))?;
        let sel = self.clamp_selection(sel);
        let (start, _) = sel.ordered();
        // Fit the column: the page's text width, or the cell's.
        let mut max_px = self.content_width() as f32 / 15.0;
        if let Some(c) = self.paragraphs[start.para].cell
            && let Some(t) = self.tables.get(&c.table)
        {
            let widths = t.column_widths(self.content_width());
            let g = t.grid_col(c.row as usize, c.cell as usize);
            let span = t.rows[c.row as usize].cells[c.cell as usize].span.max(1) as usize;
            let cw: u32 = widths.iter().skip(g).take(span).sum();
            max_px = (cw.saturating_sub(216) as f32 / 15.0).max(16.0);
        }
        let scale = (max_px / w as f32).min(1.0);
        let (wpx, hpx) = (w as f32 * scale, h as f32 * scale);
        let n = (1..)
            .find(|n| {
                let id = format!("rIdKantorImage{n}");
                !self.media.contains_key(&id)
                    && !self.package.as_ref().is_some_and(|p| p.has_rel(&id))
            })
            .expect("infinite ids");
        let rel_id = format!("rIdKantorImage{n}");
        let part = format!("word/media/kantor_image{n}.{}", format.extension());
        let doc_pr = self.max_doc_pr() + 1;
        let bytes: Arc<[u8]> = bytes.into();
        let image = Image {
            rel_id: rel_id.clone(),
            cx: (wpx * EMU_PER_PX).round() as i64,
            cy: (hpx * EMU_PER_PX).round() as i64,
            descr: name.to_owned(),
            xml: None,
            doc_pr,
            part: part.clone(),
        };
        self.media.insert(
            rel_id.clone(),
            Media {
                part: part.clone(),
                bytes: Some(bytes.clone()),
            },
        );
        self.new_media.push(NewMedia {
            rel_id,
            part,
            bytes,
        });
        Ok(self.insert_object(sel, Object::Image(Arc::new(image))))
    }

    /// Replace the selection with one object.
    pub fn insert_object(&mut self, sel: Selection, object: Object) -> Selection {
        let sel = self.clamp_selection(sel);
        let (start, _) = sel.ordered();
        let after_delete = if sel.is_empty() {
            sel
        } else {
            self.delete_selection(sel)
        };
        let at = after_delete.head;
        let mut p = self.paragraphs[at.para].clone();
        let props = p.props_at(at.offset);
        p.insert_object(at.offset, object, props);
        let after = Selection::caret(Position::new(at.para, at.offset + OBJECT.len_utf8()));
        let _ = start;
        self.replace(
            at.para,
            1,
            vec![p],
            (after_delete, after),
            Kind::Other,
            true,
        );
        after
    }

    /// Largest `wp:docPr` id in the document (new pictures need unique ids).
    fn max_doc_pr(&self) -> u32 {
        let mut max = 0;
        for p in &self.paragraphs {
            for (_, o) in p.objects() {
                match o {
                    Object::Image(i) => {
                        max = max.max(i.doc_pr);
                        if let Some(x) = &i.xml {
                            max = max.max(doc_pr_ids(x));
                        }
                    }
                    Object::Opaque(o) => max = max.max(doc_pr_ids(&o.xml)),
                }
            }
        }
        max
    }
}

/// Largest `docPr id="…"` in an XML fragment.
fn doc_pr_ids(xml: &str) -> u32 {
    let mut max = 0;
    let mut rest = xml;
    while let Some(i) = rest.find("docPr ") {
        rest = &rest[i + 6..];
        let tag = &rest[..rest.find('>').unwrap_or(rest.len())];
        if let Some(j) = tag
            .find(" id=\"")
            .or_else(|| tag.starts_with("id=\"").then_some(0))
        {
            let v = &tag[j..];
            let v = &v[v.find('"').map_or(0, |k| k + 1)..];
            if let Some(n) = v.split('"').next().and_then(|n| n.parse::<u32>().ok()) {
                max = max.max(n);
            }
        }
    }
    max
}

/// A `w:tcPr` with its `w:tcW` set to `w` twips.
fn with_tcw(tc_pr: Option<&str>, w: u32) -> String {
    let tcw = format!("<w:tcW w:w=\"{w}\" w:type=\"dxa\"/>");
    match tc_pr {
        Some(x) => {
            let stripped = strip_element(x, "tcW");
            if stripped.starts_with("<w:tcPr/>") {
                format!("<w:tcPr>{tcw}</w:tcPr>")
            } else {
                match stripped.find('>') {
                    Some(i) => {
                        let mut s = stripped.clone();
                        s.insert_str(i + 1, &tcw);
                        s
                    }
                    None => format!("<w:tcPr>{tcw}</w:tcPr>"),
                }
            }
        }
        None => format!("<w:tcPr>{tcw}</w:tcPr>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(p: usize, o: usize) -> Selection {
        Selection::caret(Position::new(p, o))
    }

    fn texts(d: &Document) -> Vec<&str> {
        d.paragraphs().iter().map(|p| p.text.as_str()).collect()
    }

    #[test]
    fn lists_toggle_continue_end_and_indent() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("one"), Paragraph::new("x")]);
        d.toggle_list(sel(0, 0), ListKind::Numbered);
        assert_eq!(d.list_kind(0), Some(ListKind::Numbered));
        let c = d.newline(sel(0, 3), None);
        assert_eq!(d.list_kind(1), Some(ListKind::Numbered), "Enter continues");
        assert_eq!(
            d.list_markers()[..2],
            [Some("1.".into()), Some("2.".into())]
        );
        assert!(d.change_list_level(c, 1));
        assert_eq!(d.list_markers()[1].as_deref(), Some("a."));
        d.change_list_level(c, -1);
        // Enter on the empty item ends the list.
        d.newline(c, None);
        assert_eq!(d.list_kind(1), None);
        assert_eq!(texts(&d), ["one", "", "x"]);
        // Toggling again on a list paragraph switches it off; undo restores.
        d.toggle_list(sel(0, 0), ListKind::Numbered);
        assert_eq!(d.list_kind(0), None);
        d.undo();
        assert_eq!(d.list_kind(0), Some(ListKind::Numbered));
        // Backspace at the start of a list item removes the number.
        d.backspace(sel(0, 0));
        assert_eq!(d.list_kind(0), None);
        assert_eq!(texts(&d), ["one", "", "x"]);
        d.toggle_list(sel(2, 0), ListKind::Bullet);
        assert_eq!(d.list_markers()[2].as_deref(), Some("•"));
    }

    #[test]
    fn tables_insert_navigate_and_edit() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("before")]);
        let c = d.insert_table(sel(0, 6), 2, 2).unwrap();
        assert_eq!(d.len(), 1 + 4 + 1, "table and a paragraph after it");
        assert_eq!(c.head, Position::new(1, 0));
        let c = d.insert_text(c, "a", None);
        let n = d.next_cell(c, true).unwrap();
        assert_eq!(n.head.para, 2);
        d.insert_text(n, "b", None);
        // Tab in the last cell adds a row.
        let last = d.next_cell(sel(4, 0), true).unwrap();
        assert_eq!(d.table(1).unwrap().rows.len(), 3);
        assert_eq!(d.cell_at(last.head).unwrap().row, 2);
        // Typing across cells never merges them.
        d.delete_selection(Selection {
            anchor: Position::new(1, 0),
            head: Position::new(2, 1),
        });
        assert_eq!(d.len(), 1 + 6 + 1);
        assert!(d.paragraphs()[1..7].iter().all(|p| p.text.is_empty()));
        // Backspace at a cell start doesn't join it with the previous cell.
        d.insert_text(sel(2, 0), "z", None);
        d.backspace(sel(2, 0));
        assert_eq!(d.paragraph(2).text, "z");
        d.edit_table(sel(2, 0), TableEdit::ColumnRight).unwrap();
        assert_eq!(d.table(1).unwrap().columns(), 3);
        let total: u32 = d.table(1).unwrap().grid.iter().sum();
        assert!(total.abs_diff(d.content_width()) < 4, "width kept: {total}");
        d.edit_table(sel(2, 0), TableEdit::DeleteRow).unwrap();
        assert_eq!(d.table(1).unwrap().rows.len(), 2);
        d.undo();
        d.undo();
        assert_eq!(d.table(1).unwrap().columns(), 2);
        assert_eq!(d.table(1).unwrap().rows.len(), 3);
        d.edit_table(sel(2, 0), TableEdit::DeleteTable).unwrap();
        assert_eq!(texts(&d), ["before", ""]);
        // Not inside a table.
        assert!(d.insert_table(sel(0, 0), 1, 1).is_some());
        assert!(d.insert_table(sel(1, 0), 1, 1).is_none());
    }

    #[test]
    fn selecting_over_a_whole_table_removes_it() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("a"), Paragraph::new("b")]);
        d.insert_table(sel(0, 1), 1, 2);
        assert_eq!(d.len(), 4);
        d.delete_selection(Selection {
            anchor: Position::new(0, 1),
            head: Position::new(3, 0),
        });
        assert_eq!(texts(&d), ["ab"]);
    }

    #[test]
    fn images_insert_as_one_character() {
        // 1×1 PNG header is enough for the size.
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend(2000u32.to_be_bytes());
        png.extend(1000u32.to_be_bytes());
        png.extend([8, 6, 0, 0, 0]);
        let mut d = Document::from_paragraphs(vec![Paragraph::new("ab")]);
        let c = d.insert_image(sel(0, 1), png, "pic.png").unwrap();
        assert_eq!(c.head.offset, 1 + OBJECT.len_utf8());
        let Some(Object::Image(img)) = d.paragraph(0).object_at(1) else {
            panic!("no image");
        };
        let (w, h) = img.size_px();
        assert!(
            (w - d.content_width() as f32 / 15.0).abs() < 1.0,
            "scaled to the text width"
        );
        assert!((w / h - 2.0).abs() < 0.01);
        assert!(d.image_bytes(&img.rel_id).is_some());
        assert_eq!(
            d.text_in(Position::new(0, 0), Position::new(0, c.head.offset + 1)),
            "ab"
        );
        d.backspace(c);
        assert_eq!(d.paragraph(0).text, "ab");
        assert!(
            d.insert_image(sel(0, 0), b"GIF89a".to_vec(), "x.gif")
                .is_err()
        );
    }

    #[test]
    fn helpers() {
        assert_eq!(
            doc_pr_ids("<wp:docPr id=\"7\" name=\"x\"/><wp:docPr id=\"3\"/>"),
            7
        );
        assert_eq!(
            with_tcw(
                Some("<w:tcPr><w:tcW w:w=\"1\" w:type=\"dxa\"/></w:tcPr>"),
                5
            ),
            "<w:tcPr><w:tcW w:w=\"5\" w:type=\"dxa\"/></w:tcPr>"
        );
    }
}
