//! Notes (Excel's cell comments, "Notes" since Excel 365): reading them,
//! New / Edit / Delete Note and Show/Hide Note, and saving them.
//!
//! IronCalc keeps a sheet's notes in its model (`Worksheet::comments`, read
//! from an `.xlsx` or brought in from an `.xls`) but neither moves nor
//! writes them. The model's notes are where the opened file had them; they
//! follow inserted and deleted rows and columns through the layout. Once a
//! sheet's notes are edited in Kantor, its whole list lives in the history
//! state's [`crate::edits::Edits`] (with the layout it was made in), so the
//! edits undo and redo with the cells.
//!
//! Saving: an `.xlsx`'s own notes are carried with its parts (`preserve`),
//! moved with their cells. A sheet whose notes were edited this session,
//! and every sheet of a workbook with no package to carry from (new or
//! imported), gets its notes written from the current state: a comments
//! part and the VML note boxes Excel needs to show them (`preserve::
//! write_notes`), keeping the file's markup of notes that didn't change.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{Result, bail};

use crate::Workbook;
use crate::shift::{Layout, parse_range};

/// A sheet's notes by `(row, col)` (0-based) of the cell they are on now.
pub type SheetNotes = std::collections::HashMap<(u32, u32), CellNote>;

/// A note on a cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellNote {
    /// 0-based. From [`Workbook::notes_now`] and [`Workbook::note_at`]: the
    /// cell it is on now; from [`Workbook::notes`]: where the opened file
    /// had it.
    pub row: u32,
    pub col: u32,
    pub author: String,
    /// The note's text. Excel's notes start with `Author:` and a line break
    /// in the text itself (bold in Excel).
    pub text: String,
    /// Shown all the time (Show/Hide Note), not only while the pointer is
    /// over its cell.
    pub shown: bool,
}

/// A note to write, at its current cell.
#[derive(Debug, Clone)]
pub(crate) struct NoteSave {
    pub row: u32,
    pub col: u32,
    pub author: String,
    pub text: String,
    pub shown: bool,
}

impl Workbook {
    /// The notes of `sheet` (an index), as the opened file had them.
    pub fn notes(&self, sheet: u32) -> Vec<CellNote> {
        let Ok(ws) = self.model.get_model().workbook.worksheet(sheet) else {
            return Vec::new();
        };
        let id = ws.sheet_id;
        ws.comments
            .iter()
            .filter_map(|c| {
                let r = parse_range(&c.cell_ref)?.area();
                Some(CellNote {
                    row: r.top,
                    col: r.left,
                    author: c.author_name.clone(),
                    text: c.text.clone(),
                    shown: self.shown_notes.contains(&(id, r.top, r.left)),
                })
            })
            .collect()
    }

    /// The notes of `sheet` (an index) by the cell they are on now: moved
    /// with inserted and deleted rows and columns since the file was opened
    /// (a note whose cell was deleted is gone), with the notes added,
    /// edited and deleted in Kantor.
    pub fn notes_now(&self, sheet: u32) -> Arc<SheetNotes> {
        if let Some(n) = self.cached(|c| c.notes.get(&sheet).cloned()) {
            return n;
        }
        let id = self.sheet_id(sheet);
        let edits = self.edits();
        // The model's notes are where the opened file had them (IronCalc
        // doesn't move them): the root layout.
        let (list, base) = match edits.notes.get(&id) {
            Some((list, layout)) => (list.clone(), layout.clone()),
            None => (self.notes(sheet), Layout::default()),
        };
        let mut map = SheetNotes::new();
        if !list.is_empty() {
            let moves = base.moves_to(&self.history.layout());
            for mut n in list {
                if let Some((row, col)) = moves.cell(id, n.row, n.col) {
                    n.row = row;
                    n.col = col;
                    map.entry((row, col)).or_insert(n);
                }
            }
        }
        let map = Arc::new(map);
        self.cached(|c| c.notes.insert(sheet, map.clone()));
        map
    }

    /// The note on the cell at `(row, col)` of `sheet` now, if any.
    pub fn note_at(&self, sheet: u32, row: u32, col: u32) -> Option<CellNote> {
        self.notes_now(sheet).get(&(row, col)).cloned()
    }

    /// The cells of `sheet` with a note, in reading order (row by row).
    pub fn note_cells(&self, sheet: u32) -> Vec<(u32, u32)> {
        let mut cells: Vec<(u32, u32)> = self.notes_now(sheet).keys().copied().collect();
        cells.sort_unstable();
        cells
    }

    /// Next Note / Previous Note from `(row, col)`: the next cell with a
    /// note in reading order, wrapping round, as Excel goes through them.
    pub fn next_note(&self, sheet: u32, from: (u32, u32), back: bool) -> Option<(u32, u32)> {
        let cells = self.note_cells(sheet);
        if back {
            cells
                .iter()
                .rev()
                .find(|&&c| c < from)
                .or(cells.last())
                .copied()
        } else {
            cells.iter().find(|&&c| c > from).or(cells.first()).copied()
        }
    }

    /// New Note / Edit Note: the cell's note holds `text`. A new note gets
    /// `author`; an edited one keeps its own. One undo step (none when the
    /// text is what the note already has).
    pub fn set_note(
        &mut self,
        sheet: u32,
        row: u32,
        col: u32,
        text: &str,
        author: &str,
    ) -> Result<()> {
        self.check_note_cell(sheet, row, col)?;
        let now = self.notes_now(sheet);
        let note = match now.get(&(row, col)) {
            Some(n) if n.text == text => return Ok(()),
            Some(n) => CellNote {
                text: text.to_owned(),
                ..n.clone()
            },
            None => CellNote {
                row,
                col,
                author: author.to_owned(),
                text: text.to_owned(),
                shown: false,
            },
        };
        let mut list: Vec<CellNote> = now.values().cloned().collect();
        list.retain(|n| (n.row, n.col) != (row, col));
        list.push(note);
        self.put_notes(sheet, list);
        Ok(())
    }

    /// Delete Note on the cells of `range`. One undo step; `false` when
    /// none of them had a note.
    pub fn delete_notes(&mut self, sheet: u32, range: crate::CellRange) -> Result<bool> {
        let now = self.notes_now(sheet);
        let mut list: Vec<CellNote> = now.values().cloned().collect();
        let before = list.len();
        list.retain(|n| !range.contains(n.row, n.col));
        if list.len() == before {
            return Ok(false);
        }
        self.put_notes(sheet, list);
        Ok(true)
    }

    /// Show/Hide Note: the notes on the cells of `range` are shown all the
    /// time (`true`) or only while the pointer is over their cell. One undo
    /// step; `false` when nothing changed.
    pub fn show_notes(&mut self, sheet: u32, range: crate::CellRange, shown: bool) -> Result<bool> {
        let now = self.notes_now(sheet);
        let mut changed = false;
        let list: Vec<CellNote> = now
            .values()
            .map(|n| {
                let mut n = n.clone();
                if range.contains(n.row, n.col) && n.shown != shown {
                    n.shown = shown;
                    changed = true;
                }
                n
            })
            .collect();
        if changed {
            self.put_notes(sheet, list);
        }
        Ok(changed)
    }

    fn check_note_cell(&self, sheet: u32, row: u32, col: u32) -> Result<()> {
        if self.model.get_model().workbook.worksheet(sheet).is_err() {
            bail!("No sheet {sheet}");
        }
        if row >= crate::MAX_ROWS || col >= crate::MAX_COLS {
            bail!("No cell there");
        }
        Ok(())
    }

    /// The sheet's notes are `list` (at the cells they are on now).
    fn put_notes(&mut self, sheet: u32, mut list: Vec<CellNote>) {
        list.sort_by_key(|n| (n.row, n.col));
        let id = self.sheet_id(sheet);
        self.own_edit(|e, layout| {
            e.notes.insert(id, (list, layout.clone()));
        });
    }

    /// Notes a save writes from the current state: every sheet's when there
    /// is no package to carry them from (new and imported workbooks), else
    /// the sheets whose notes were edited this session (an empty list
    /// removes the sheet's notes from the file).
    pub(crate) fn note_patches(&self, out: &mut crate::preserve::Patches) {
        // An `.xls` import's original package holds its drawings only, so
        // its notes are written from the model like a new workbook's.
        let carried = self.origin.xlsx.is_some() && !self.origin.format.is_imported();
        let mut ids: HashSet<u32> = self.edits().notes.keys().copied().collect();
        ids.extend(self.touched.notes.iter().copied());
        for sheet in 0..self.sheet_names().len() as u32 {
            let id = self.sheet_id(sheet);
            if carried && !ids.contains(&id) {
                continue;
            }
            let mut list: Vec<NoteSave> = self
                .notes_now(sheet)
                .values()
                .map(|n| NoteSave {
                    row: n.row,
                    col: n.col,
                    author: n.author.clone(),
                    text: n.text.clone(),
                    shown: n.shown,
                })
                .collect();
            list.sort_by_key(|n| (n.row, n.col));
            if carried || !list.is_empty() {
                out.notes.push((id, list));
            }
        }
    }
}

pub(crate) fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Excel's run properties for note text (Tahoma 9, the note colour).
const RPR: &str =
    "<sz val=\"9\"/><color indexed=\"81\"/><rFont val=\"Tahoma\"/><family val=\"2\"/>";

/// A `<comment>`'s `<text>`: Excel's bold `Author:` run when the text
/// starts with it, then the rest.
pub(crate) fn text_xml(author: &str, text: &str) -> String {
    let head = format!("{author}:");
    let mut out = String::from("<text>");
    let rest = match text.strip_prefix(&head) {
        Some(rest) if !author.is_empty() => {
            out.push_str(&format!("<r><rPr><b/>{RPR}</rPr><t>{}</t></r>", esc(&head)));
            rest
        }
        _ => text,
    };
    if !rest.is_empty() || out.len() == "<text>".len() {
        out.push_str(&format!(
            "<r><rPr>{RPR}</rPr><t xml:space=\"preserve\">{}</t></r>",
            esc(rest)
        ));
    }
    out.push_str("</text>");
    out
}

/// The `comments` part of `notes`.
pub(crate) fn comments_xml(notes: &[NoteSave]) -> String {
    let mut authors: Vec<&str> = Vec::new();
    for n in notes {
        if !authors.contains(&n.author.as_str()) {
            authors.push(&n.author);
        }
    }
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<comments xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><authors>",
    );
    for a in &authors {
        out.push_str(&format!("<author>{}</author>", esc(a)));
    }
    out.push_str("</authors><commentList>");
    for n in notes {
        let author = authors.iter().position(|a| *a == n.author).unwrap_or(0);
        out.push_str(&format!(
            "<comment ref=\"{}\" authorId=\"{author}\">{}</comment>",
            crate::cell_name(n.row, n.col),
            text_xml(&n.author, &n.text)
        ));
    }
    out.push_str("</commentList></comments>");
    out
}

/// The VML namespaces and Excel's note shape type.
pub(crate) const VML_HEAD: &str = "<xml xmlns:v=\"urn:schemas-microsoft-com:vml\" xmlns:o=\"urn:schemas-microsoft-com:office:office\" xmlns:x=\"urn:schemas-microsoft-com:office:excel\">";
pub(crate) const VML_SHAPETYPE: &str = "<v:shapetype id=\"_x0000_t202\" coordsize=\"21600,21600\" o:spt=\"202\" path=\"m,l,21600r21600,l21600,xe\">\
<v:stroke joinstyle=\"miter\"/><v:path gradientshapeok=\"t\" o:connecttype=\"rect\"/></v:shapetype>";

/// The VML note box (`<v:shape>`) of `n`, with shape id `_x0000_s{id}`.
pub(crate) fn vml_shape(n: &NoteSave, id: usize, z: usize) -> String {
    // The box: right of the cell, 2 columns wide and 4 rows high.
    let anchor = format!(
        "{}, 15, {}, 10, {}, 15, {}, 4",
        n.col + 1,
        n.row.saturating_sub(1),
        n.col + 3,
        n.row + 3
    );
    let (visibility, visible) = if n.shown {
        ("visible", "<x:Visible/>")
    } else {
        ("hidden", "")
    };
    format!(
        "<v:shape id=\"_x0000_s{id}\" type=\"#_x0000_t202\" style=\"position:absolute;margin-left:59.25pt;margin-top:1.5pt;width:108pt;height:59.25pt;z-index:{z};visibility:{visibility}\" fillcolor=\"#ffffe1\" o:insetmode=\"auto\">\
<v:fill color2=\"#ffffe1\"/><v:shadow on=\"t\" color=\"black\" obscured=\"t\"/><v:path o:connecttype=\"none\"/>\
<v:textbox style=\"mso-direction-alt:auto\"><div style=\"text-align:left\"></div></v:textbox>\
<x:ClientData ObjectType=\"Note\"><x:MoveWithCells/><x:SizeWithCells/><x:Anchor>{anchor}</x:Anchor><x:AutoFill>False</x:AutoFill><x:Row>{}</x:Row><x:Column>{}</x:Column>{visible}</x:ClientData></v:shape>",
        n.row, n.col
    )
}

/// The VML drawing with a note box per note (Excel shows a note from its
/// box; without one it drops the note). `block` numbers the drawing's
/// shape ids (1024 per block), unique in the workbook.
pub(crate) fn vml_xml(notes: &[NoteSave], block: usize) -> String {
    let mut out = format!(
        "{VML_HEAD}<o:shapelayout v:ext=\"edit\"><o:idmap v:ext=\"edit\" data=\"{block}\"/></o:shapelayout>{VML_SHAPETYPE}"
    );
    for (i, n) in notes.iter().enumerate() {
        out.push_str(&vml_shape(n, 1024 * block + 1 + i, i + 1));
    }
    out.push_str("</xml>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn author_run_is_bold() {
        let x = text_xml("Dewi", "Dewi:\nCheck this & that");
        assert!(x.contains("<b/>"));
        assert!(x.contains("<t>Dewi:</t>"));
        assert!(x.contains(">\nCheck this &amp; that</t>"));
        let plain = text_xml("Dewi", "No author line");
        assert!(!plain.contains("<b/>"));
        assert!(text_xml("", "").contains("<t xml:space=\"preserve\"></t>"));
    }
}
