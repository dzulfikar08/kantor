//! Notes from macros: `Range.AddComment`, `Range.Comment`, `Range.NoteText`,
//! `Range.ClearComments`, `Worksheet.Comments` and the `Comment` object.
//!
//! Excel's `Comment` is a legacy note (what Kantor's New Note makes); its
//! `CommentThreaded` is a threaded comment. Kantor's engine models notes
//! (`sheets_engine::notes`) and only carries a file's threaded comments
//! through saves, so the threaded members fail with error 445.
//!
//! A `Comment` object is its sheet's `sheetId` and its cell, looked up again
//! on each use; every change goes through the engine's note edits (New /
//! Edit / Delete Note, Show/Hide Note), so it undoes and saves as they do.

use std::rc::Rc;

use kantor_script::{Arg, Dispatch, ScriptError, Value};
use sheets_engine::{CellNote, CellRange};

use crate::range::RangeObj;
use crate::session::Session;
use crate::values::*;

/// `Range.AddComment([Text])`: a new note on the range's top-left cell, by
/// `Application.UserName`. Error 1004 when the cell has one already, as in
/// Excel.
pub fn add_comment(r: &RangeObj, args: &[Arg]) -> Result<Value, ScriptError> {
    let text = arg_str(args, 0, "Text")?.unwrap_or_default();
    let a = r.first();
    let (row, col) = (a.top, a.left);
    let idx = r.idx()?;
    let author = r.s.user_name.borrow().clone();
    {
        let mut book = r.s.write()?;
        if book.note_at(idx, row, col).is_some() {
            return Err(err1004(
                "Application-defined or object-defined error (the cell already has a note)",
            ));
        }
        book.set_note(idx, row, col, &text, &author)
            .map_err(engine)?;
    }
    Ok(obj(CommentObj::new(&r.s, r.sheet, row, col)))
}

/// `Range.Comment`: the note on the range's top-left cell, or `Nothing`.
pub fn comment(r: &RangeObj) -> Result<Value, ScriptError> {
    let a = r.first();
    let idx = r.idx()?;
    let has = r.s.read()?.note_at(idx, a.top, a.left).is_some();
    Ok(if has {
        obj(CommentObj::new(&r.s, r.sheet, a.top, a.left))
    } else {
        Value::NOTHING
    })
}

/// `Range.ClearComments`: deletes the notes on every area of the range.
pub fn clear_comments(r: &RangeObj) -> Result<Value, ScriptError> {
    let idx = r.idx()?;
    let mut book = r.s.write()?;
    for a in &r.areas {
        book.delete_notes(idx, *a).map_err(engine)?;
    }
    Ok(Value::Empty)
}

/// `Range.NoteText([Text], [Start], [Length])` on the range's top-left
/// cell. Without `Text` it returns `Length` characters (to the end when
/// left out) from `Start` (1-based, default 1). With `Text` it replaces
/// those characters (from `Start` to the end when `Length` is left out),
/// making a note when the cell has none, and returns the note's text.
pub fn note_text(r: &RangeObj, args: &[Arg]) -> Result<Value, ScriptError> {
    let text = arg_str(args, 0, "Text")?;
    let start = arg_i64(args, 1, "Start")?.unwrap_or(1);
    let length = arg_i64(args, 2, "Length")?;
    if start < 1 || length.is_some_and(|l| l < 0) {
        return Err(ScriptError::runtime(5));
    }
    let a = r.first();
    let (row, col) = (a.top, a.left);
    let idx = r.idx()?;
    let current =
        r.s.read()?
            .note_at(idx, row, col)
            .map(|n| n.text)
            .unwrap_or_default();
    let chars: Vec<char> = current.chars().collect();
    let from = ((start - 1) as usize).min(chars.len());
    let to = match length {
        Some(l) => from.saturating_add(l as usize).min(chars.len()),
        None => chars.len(),
    };
    let Some(text) = text else {
        return Ok(Value::String(chars[from..to].iter().collect()));
    };
    let mut new: String = chars[..from].iter().collect();
    new.push_str(&text);
    new.extend(&chars[to..]);
    let author = r.s.user_name.borrow().clone();
    r.s.write()?
        .set_note(idx, row, col, &new, &author)
        .map_err(engine)?;
    Ok(Value::String(new))
}

/// `Worksheet.Comments`.
pub fn comments(s: &Rc<Session>, sheet: u32) -> Value {
    obj(Rc::new(CommentsObj {
        s: s.clone(),
        sheet,
    }))
}

/// `Range.AddCommentThreaded`, `Range.CommentThreaded`,
/// `Worksheet.CommentsThreaded`: Kantor keeps a file's threaded comments
/// but doesn't model them.
pub fn threaded() -> Result<Value, ScriptError> {
    Err(ScriptError::unsupported(
        "Threaded comments (CommentThreaded)",
    ))
}

/// The cells of the sheet with a note, in reading order.
fn note_cells(s: &Rc<Session>, sheet: u32) -> Result<Vec<(u32, u32)>, ScriptError> {
    let idx = s.sheet_index(sheet)?;
    Ok(s.read()?.note_cells(idx))
}

/// `Worksheet.Comments`: the sheet's notes in reading order (row by row).
pub struct CommentsObj {
    s: Rc<Session>,
    sheet: u32,
}

impl CommentsObj {
    fn comment(&self, cell: (u32, u32)) -> Value {
        obj(CommentObj::new(&self.s, self.sheet, cell.0, cell.1))
    }
}

impl Dispatch for CommentsObj {
    fn type_name(&self) -> String {
        "Comments".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "count" => Ok(Value::Long(note_cells(&self.s, self.sheet)?.len() as i32)),
            "" | "item" => {
                let i = to_i64(need(args, 0, "Index")?)?;
                let cells = note_cells(&self.s, self.sheet)?;
                let cell = usize::try_from(i - 1)
                    .ok()
                    .and_then(|k| cells.get(k).copied())
                    .ok_or_else(|| ScriptError::runtime(9))?;
                Ok(self.comment(cell))
            }
            "parent" => Ok(obj(crate::sheet::SheetObj::new(&self.s, self.sheet))),
            _ => Err(ScriptError::no_member("Comments", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        Ok(note_cells(&self.s, self.sheet)?
            .into_iter()
            .map(|c| self.comment(c))
            .collect())
    }
}

/// A note (Excel's `Comment`), by its sheet and cell.
pub struct CommentObj {
    s: Rc<Session>,
    sheet: u32,
    row: u32,
    col: u32,
}

impl CommentObj {
    pub fn new(s: &Rc<Session>, sheet: u32, row: u32, col: u32) -> Rc<CommentObj> {
        Rc::new(CommentObj {
            s: s.clone(),
            sheet,
            row,
            col,
        })
    }

    fn cell(&self) -> CellRange {
        CellRange::from_corners((self.row, self.col), (self.row, self.col))
    }

    /// The note now (error 1004 when it was deleted).
    fn note(&self) -> Result<(u32, CellNote), ScriptError> {
        let idx = self.s.sheet_index(self.sheet)?;
        let note = self.s.read()?.note_at(idx, self.row, self.col);
        note.map(|n| (idx, n)).ok_or_else(|| {
            err1004("Application-defined or object-defined error (the note was deleted)")
        })
    }

    /// `Comment.Text([Text], [Start], [Overwrite])`.
    fn text(&self, args: &[Arg]) -> Result<Value, ScriptError> {
        let (idx, note) = self.note()?;
        let Some(text) = arg_str(args, 0, "Text")? else {
            return Ok(Value::String(note.text));
        };
        let new = match arg_i64(args, 1, "Start")? {
            None => text,
            Some(start) => {
                if start < 1 {
                    return Err(ScriptError::runtime(5));
                }
                let overwrite = arg_bool(args, 2, "Overwrite")?.unwrap_or(false);
                let chars: Vec<char> = note.text.chars().collect();
                let from = ((start - 1) as usize).min(chars.len());
                let to = if overwrite {
                    (from + text.chars().count()).min(chars.len())
                } else {
                    from
                };
                let mut out: String = chars[..from].iter().collect();
                out.push_str(&text);
                out.extend(&chars[to..]);
                out
            }
        };
        let author = self.s.user_name.borrow().clone();
        self.s
            .write()?
            .set_note(idx, self.row, self.col, &new, &author)
            .map_err(engine)?;
        Ok(Value::String(new))
    }

    /// The next (or previous) note of the sheet in reading order, or
    /// `Nothing` past the last (first) one.
    fn step(&self, back: bool) -> Result<Value, ScriptError> {
        let cells = note_cells(&self.s, self.sheet)?;
        let me = (self.row, self.col);
        let found = if back {
            cells.iter().rev().find(|&&c| c < me).copied()
        } else {
            cells.iter().find(|&&c| c > me).copied()
        };
        Ok(match found {
            Some((row, col)) => obj(CommentObj::new(&self.s, self.sheet, row, col)),
            None => Value::NOTHING,
        })
    }

    fn set_visible(&self, shown: bool) -> Result<(), ScriptError> {
        let (idx, _) = self.note()?;
        self.s
            .write()?
            .show_notes(idx, self.cell(), shown)
            .map_err(engine)?;
        Ok(())
    }
}

impl Dispatch for CommentObj {
    fn type_name(&self) -> String {
        "Comment".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "text" => self.text(args),
            "author" => Ok(Value::String(self.note()?.1.author)),
            "visible" => Ok(Value::Boolean(self.note()?.1.shown)),
            "parent" => Ok(obj(RangeObj::new(&self.s, self.sheet, self.cell()))),
            "delete" => {
                let (idx, _) = self.note()?;
                self.s
                    .write()?
                    .delete_notes(idx, self.cell())
                    .map_err(engine)?;
                Ok(Value::Empty)
            }
            "next" => self.step(false),
            "previous" => self.step(true),
            "shape" => Err(ScriptError::unsupported("A note's shape (Comment.Shape)")),
            _ => Err(ScriptError::no_member("Comment", name)),
        }
    }

    fn put(&self, name: &str, args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "visible" => self.set_visible(to_bool(&value)?),
            "text" if args.is_empty() => {
                self.text(&[Arg::new(value)])?;
                Ok(())
            }
            _ => Err(ScriptError::no_member("Comment", name)),
        }
    }
}
