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

/// Where a note's box is (its VML `x:Anchor`), relative to the note's
/// cell so it moves with it: each edge is a column (or row) counted from
/// the note's cell and an offset into it in Excel px.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NoteBox {
    pub left: (i64, u32),
    pub top: (i64, u32),
    pub right: (i64, u32),
    pub bottom: (i64, u32),
}

impl NoteBox {
    /// The box Excel gives a new note: right of the cell, about two columns
    /// wide and four rows high, starting a row up.
    pub fn default_for(row: u32) -> Self {
        let up = i64::from(row.min(1));
        NoteBox {
            left: (1, 15),
            top: (-up, 10),
            right: (3, 15),
            bottom: (3, 4),
        }
    }

    /// From an `x:Anchor` (left column, offset, top row, offset, right
    /// column, offset, bottom row, offset) of the note on `(row, col)`.
    pub fn from_anchor(row: u32, col: u32, a: [u32; 8]) -> Self {
        let c = |v: u32| i64::from(v) - i64::from(col);
        let r = |v: u32| i64::from(v) - i64::from(row);
        NoteBox {
            left: (c(a[0]), a[1]),
            top: (r(a[2]), a[3]),
            right: (c(a[4]), a[5]),
            bottom: (r(a[6]), a[7]),
        }
    }

    /// The `x:Anchor` numbers for the note on `(row, col)` (clamped to the
    /// sheet).
    pub fn anchor(&self, row: u32, col: u32) -> [u32; 8] {
        let c = |d: i64| (i64::from(col) + d).clamp(0, i64::from(crate::MAX_COLS) - 1) as u32;
        let r = |d: i64| (i64::from(row) + d).clamp(0, i64::from(crate::MAX_ROWS) - 1) as u32;
        [
            c(self.left.0),
            self.left.1,
            r(self.top.0),
            self.top.1,
            c(self.right.0),
            self.right.1,
            r(self.bottom.0),
            self.bottom.1,
        ]
    }

    /// As a drawing anchor (offsets in EMU), to place it with a sheet's
    /// geometry like a picture.
    pub fn drawing_anchor(&self, row: u32, col: u32) -> crate::drawing::Anchor {
        use crate::drawing::{Anchor, EditAs, Marker};
        let a = self.anchor(row, col);
        let m = |col: u32, cx: u32, row: u32, ry: u32| Marker {
            col,
            col_off: i64::from(cx) * 9525,
            row,
            row_off: i64::from(ry) * 9525,
        };
        Anchor::TwoCell {
            from: m(a[0], a[1], a[2], a[3]),
            to: m(a[4], a[5], a[6], a[7]),
            edit_as: EditAs::TwoCell,
        }
    }

    /// From a drawing anchor (see [`NoteBox::drawing_anchor`]).
    pub fn from_drawing_anchor(row: u32, col: u32, a: &crate::drawing::Anchor) -> Option<Self> {
        let crate::drawing::Anchor::TwoCell { from, to, .. } = a else {
            return None;
        };
        let px = |emu: i64| (emu.max(0) as f64 / 9525.0).round() as u32;
        Some(Self::from_anchor(
            row,
            col,
            [
                from.col,
                px(from.col_off),
                from.row,
                px(from.row_off),
                to.col,
                px(to.col_off),
                to.row,
                px(to.row_off),
            ],
        ))
    }
}

/// A run of a note's formatted text (a comment's `<r>`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct NoteRun {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// Hundredths of a point.
    pub size: Option<u32>,
    /// `RRGGBB`.
    pub color: Option<u32>,
    pub font: Option<String>,
    /// The other run properties as the file has them (`<family>`,
    /// `<charset>`, a theme or indexed `<color>` ...), written back as is.
    pub other: String,
}

impl NoteRun {
    /// Whether `runs` are what Kantor writes for a note's plain text
    /// anyway: the bold `Author:` run (if any), then plain runs, in one
    /// font and size.
    pub fn is_default(runs: &[NoteRun], author: &str) -> bool {
        let head = format!("{author}:");
        runs.iter().enumerate().all(|(i, r)| {
            let author_run = i == 0 && !author.is_empty() && r.text == head;
            r.bold == author_run
                && !r.italic
                && !r.underline
                && r.color.is_none()
                && r.size == runs[0].size
                && r.font == runs[0].font
        })
    }
}

/// A note on a cell.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
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
    /// Where its box is (`None`: Excel's default place).
    pub place: Option<NoteBox>,
    /// Its text with formatting, when the file has more than Excel's
    /// default (their texts joined are `text`); `None`: plain text.
    pub runs: Option<Arc<[NoteRun]>>,
}

impl CellNote {
    /// Its box, or Excel's default one.
    pub fn place_or_default(&self) -> NoteBox {
        self.place.unwrap_or_else(|| NoteBox::default_for(self.row))
    }
}

/// The notes on a copied or cut range (see [`Workbook::note_clip`]).
#[derive(Debug, Clone)]
pub struct NoteClip {
    sheet_id: u32,
    range: crate::CellRange,
    notes: Vec<CellNote>,
    cut: bool,
}

impl NoteClip {
    /// Whether any copied cell has a note.
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    /// Whether it was cut (the notes move).
    pub fn is_cut(&self) -> bool {
        self.cut
    }
}

/// What a package says of a note besides its text (read at open).
#[derive(Debug, Clone, Default)]
pub(crate) struct NoteExtra {
    /// Shown all the time (`<x:Visible/>`).
    pub shown: bool,
    /// Its box's `x:Anchor`.
    pub anchor: Option<[u32; 8]>,
    /// Its formatted text, when it has formatting of its own (see
    /// [`CellNote::runs`]).
    pub runs: Option<Arc<[NoteRun]>>,
}

/// A note to write, at its current cell.
#[derive(Debug, Clone)]
pub(crate) struct NoteSave {
    pub row: u32,
    pub col: u32,
    pub author: String,
    pub text: String,
    pub shown: bool,
    /// The box's `x:Anchor` when it has its own place.
    pub anchor: Option<[u32; 8]>,
    pub runs: Option<Arc<[NoteRun]>>,
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
                let extra = self.note_extras.get(&(id, r.top, r.left));
                let runs = extra.and_then(|e| e.runs.clone()).filter(|runs| {
                    runs.iter().map(|r| r.text.as_str()).collect::<String>() == c.text
                });
                Some(CellNote {
                    row: r.top,
                    col: r.left,
                    author: c.author_name.clone(),
                    text: c.text.clone(),
                    shown: extra.is_some_and(|e| e.shown),
                    place: extra
                        .and_then(|e| e.anchor)
                        .map(|a| NoteBox::from_anchor(r.top, r.left, a)),
                    runs,
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
                runs: None,
                ..n.clone()
            },
            None => CellNote {
                row,
                col,
                author: author.to_owned(),
                text: text.to_owned(),
                ..CellNote::default()
            },
        };
        let mut list: Vec<CellNote> = now.values().cloned().collect();
        list.retain(|n| (n.row, n.col) != (row, col));
        list.push(note);
        self.put_notes(sheet, list);
        Ok(())
    }

    /// New Note / Edit Note with formatted text: the cell's note holds
    /// `runs` (their texts joined are its text). A new note gets `author`.
    /// One undo step (none when nothing changes).
    pub fn set_note_runs(
        &mut self,
        sheet: u32,
        row: u32,
        col: u32,
        runs: Vec<NoteRun>,
        author: &str,
    ) -> Result<()> {
        self.check_note_cell(sheet, row, col)?;
        let text: String = runs.iter().map(|r| r.text.as_str()).collect();
        let runs: Arc<[NoteRun]> = runs.into();
        let now = self.notes_now(sheet);
        let note = match now.get(&(row, col)) {
            Some(n) if n.text == text && n.runs.as_deref() == Some(&*runs) => return Ok(()),
            Some(n) => CellNote {
                text,
                runs: Some(runs),
                ..n.clone()
            },
            None => CellNote {
                row,
                col,
                author: author.to_owned(),
                text,
                runs: Some(runs),
                ..CellNote::default()
            },
        };
        let mut list: Vec<CellNote> = now.values().cloned().collect();
        list.retain(|n| (n.row, n.col) != (row, col));
        list.push(note);
        self.put_notes(sheet, list);
        Ok(())
    }

    /// Move or resize the box of the note on `(row, col)` (dragged as in
    /// Excel). One undo step; nothing when it's where it was.
    pub fn set_note_place(&mut self, sheet: u32, row: u32, col: u32, place: NoteBox) -> Result<()> {
        let now = self.notes_now(sheet);
        let Some(n) = now.get(&(row, col)) else {
            bail!("This cell has no note");
        };
        if n.place_or_default() == place {
            return Ok(());
        }
        let mut list: Vec<CellNote> = now.values().cloned().collect();
        for m in &mut list {
            if (m.row, m.col) == (row, col) {
                m.place = Some(place);
            }
        }
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

    /// The sheets' notes are the lists given (at the cells they are on
    /// now), in the current history state: for an edit that made its state
    /// already (sort, fill, paste) and moves or copies notes too, so they
    /// undo as one step. Sheets whose notes didn't change are left out.
    pub(crate) fn notes_with_edit(&mut self, lists: Vec<(u32, Vec<CellNote>)>) {
        let lists: Vec<(u32, Vec<CellNote>)> = lists
            .into_iter()
            .map(|(sheet, mut list)| {
                list.sort_by_key(|n| (n.row, n.col));
                (self.sheet_id(sheet), list)
            })
            .collect();
        if lists.is_empty() {
            return;
        }
        self.change_edits(|e, layout| {
            for (id, list) in lists {
                e.notes.insert(id, (list, layout.clone()));
            }
        });
    }

    /// The notes of `sheet` now, as a list.
    pub(crate) fn note_list(&self, sheet: u32) -> Vec<CellNote> {
        self.notes_now(sheet).values().cloned().collect()
    }

    /// What Copy (or Cut) takes of the notes on `range` of `sheet`, for
    /// [`Workbook::paste_tsv_with_notes`].
    pub fn note_clip(&self, sheet: u32, range: crate::CellRange, cut: bool) -> NoteClip {
        let mut notes: Vec<CellNote> = self
            .notes_now(sheet)
            .values()
            .filter(|n| range.contains(n.row, n.col))
            .cloned()
            .collect();
        notes.sort_by_key(|n| (n.row, n.col));
        NoteClip {
            sheet_id: self.sheet_id(sheet),
            range,
            notes,
            cut,
        }
    }

    /// Paste (see [`Workbook::paste_tsv`]) with the notes of the copied
    /// cells, as Excel's Paste: the pasted cells get the copied cells'
    /// notes (and lose their own); after Cut, the notes leave the cut
    /// cells. One undo step.
    pub fn paste_tsv_with_notes(
        &mut self,
        sheet: u32,
        row: u32,
        col: u32,
        text: &str,
        clip: &NoteClip,
    ) -> Result<Option<crate::CellRange>> {
        let source = self.sheet_index_of(clip.sheet_id);
        let mut target_list = self.note_list(sheet);
        let mut source_list = source.map(|s| self.note_list(s));
        let Some(range) = self.paste_tsv(sheet, row, col, text)? else {
            return Ok(None);
        };
        let mut lists = Vec::new();
        // Cut: the notes leave where they were (before landing, so a move
        // within the sheet keeps them).
        if clip.cut
            && let (Some(s), Some(list)) = (source, source_list.as_mut())
        {
            let before = list.len();
            list.retain(|n| !clip.range.contains(n.row, n.col));
            if s == sheet {
                target_list.retain(|n| !clip.range.contains(n.row, n.col));
            } else if list.len() != before {
                lists.push((s, std::mem::take(list)));
            }
        }
        let before = target_list.clone();
        target_list.retain(|n| !range.contains(n.row, n.col));
        for n in &clip.notes {
            let r = row + (n.row - clip.range.top);
            let c = col + (n.col - clip.range.left);
            if range.contains(r, c) {
                target_list.push(CellNote {
                    row: r,
                    col: c,
                    ..n.clone()
                });
            }
        }
        if target_list != before || !lists.is_empty() {
            lists.push((sheet, target_list));
        }
        self.notes_with_edit(lists);
        Ok(Some(range))
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
                    anchor: n.place.map(|p| p.anchor(n.row, n.col)),
                    runs: n.runs.clone(),
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

/// A `<comment>`'s `<text>` for `n`: its runs when it has them, else
/// [`text_xml`].
pub(crate) fn note_text_xml(n: &NoteSave) -> String {
    let Some(runs) = &n.runs else {
        return text_xml(&n.author, &n.text);
    };
    let mut out = String::from("<text>");
    for r in runs.iter() {
        out.push_str("<r><rPr>");
        if r.bold {
            out.push_str("<b/>");
        }
        if r.italic {
            out.push_str("<i/>");
        }
        if r.underline {
            out.push_str("<u/>");
        }
        out.push_str(&format!("<sz val=\"{}\"/>", fmt_pt(r.size.unwrap_or(900))));
        match r.color {
            Some(c) => out.push_str(&format!("<color rgb=\"FF{c:06X}\"/>")),
            None if !r.other.contains("<color") => out.push_str("<color indexed=\"81\"/>"),
            None => {}
        }
        out.push_str(&format!(
            "<rFont val=\"{}\"/>",
            esc(r.font.as_deref().unwrap_or("Tahoma"))
        ));
        out.push_str(&r.other);
        out.push_str(&format!(
            "</rPr><t xml:space=\"preserve\">{}</t></r>",
            esc(&r.text)
        ));
    }
    out.push_str("</text>");
    out
}

/// Hundredths of a point as `sz` writes them (`9`, `10.5`).
fn fmt_pt(h: u32) -> String {
    if h.is_multiple_of(100) {
        (h / 100).to_string()
    } else {
        format!("{}", h as f64 / 100.0)
    }
}

/// The runs of a comment's `<text>` (`None`: not runs, e.g. a plain
/// `<t>`). Properties Kantor doesn't edit go into `other` as written.
pub(crate) fn read_runs(text: roxmltree::Node, src: &str) -> Option<Vec<NoteRun>> {
    let mut out = Vec::new();
    for r in text.children().filter(|c| c.is_element()) {
        match r.tag_name().name() {
            "r" => {}
            "rPh" | "phoneticPr" => continue,
            _ => return None,
        }
        let mut run = NoteRun::default();
        for c in r.children().filter(|c| c.is_element()) {
            match c.tag_name().name() {
                "t" => run.text.push_str(c.text().unwrap_or_default()),
                "rPr" => {
                    for p in c.children().filter(|c| c.is_element()) {
                        let val = p.attribute("val");
                        let on = val.is_none_or(|v| v != "0" && v != "false");
                        match p.tag_name().name() {
                            "b" => run.bold = on,
                            "i" => run.italic = on,
                            "u" if val.is_none_or(|v| v == "single") => run.underline = true,
                            "sz" => {
                                run.size = val
                                    .and_then(|v| v.parse::<f64>().ok())
                                    .map(|v| (v * 100.0).round() as u32)
                            }
                            "rFont" => run.font = val.map(str::to_owned),
                            "color"
                                if p.attribute("rgb").is_some() && p.attributes().count() == 1 =>
                            {
                                let v = p.attribute("rgb").unwrap_or_default();
                                run.color =
                                    u32::from_str_radix(&v[v.len().saturating_sub(6)..], 16).ok();
                            }
                            // Excel's default note colour is Automatic.
                            "color" if p.attribute("indexed") == Some("81") => {}
                            _ => run.other.push_str(&src[p.range()]),
                        }
                    }
                }
                _ => {}
            }
        }
        out.push(run);
    }
    (!out.is_empty()).then_some(out)
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
            note_text_xml(n)
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
    // The box: its own place, else right of the cell, 2 columns wide and
    // 4 rows high.
    let a = n
        .anchor
        .unwrap_or_else(|| NoteBox::default_for(n.row).anchor(n.row, n.col));
    let anchor = anchor_text(a);
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

/// An `x:Anchor`'s text.
pub(crate) fn anchor_text(a: [u32; 8]) -> String {
    a.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
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
