//! `Range.Characters(Start, Length)`: part of a cell's text.
//!
//! Kantor keeps one font per cell (no rich text runs in cells), so
//! `Characters(...).Font` formats the whole cell when the characters are
//! the whole text; for part of it, setting a font property to what the
//! cell already has is accepted (recorded macros set every property) and
//! anything else is error 445.

use std::rc::Rc;

use kantor_script::{Arg, Dispatch, ScriptError, Value};
use sheets_engine::CellRange;

use crate::format::FontObj;
use crate::range::RangeObj;
use crate::session::Session;
use crate::values::*;

/// `Range.Characters(Start, Length)` (the first cell of the range).
pub fn characters(r: &RangeObj, args: &[Arg]) -> Result<Value, ScriptError> {
    let start = arg_i64(args, 0, "Start")?.unwrap_or(1);
    let length = arg_i64(args, 1, "Length")?;
    if start < 1 || length.is_some_and(|n| n < 0) {
        return Err(err1004(
            "Unable to get the Characters property of the Range class",
        ));
    }
    let a = r.first();
    Ok(obj(Rc::new(CharactersObj {
        s: r.s.clone(),
        sheet: r.sheet,
        row: a.top,
        col: a.left,
        start: start as usize,
        length: length.map(|n| n as usize),
    })))
}

pub struct CharactersObj {
    pub s: Rc<Session>,
    pub sheet: u32,
    pub row: u32,
    pub col: u32,
    /// 1-based.
    pub start: usize,
    /// `None`: to the end.
    pub length: Option<usize>,
}

impl CharactersObj {
    /// The cell's text as shown.
    fn text(&self) -> Result<Vec<char>, ScriptError> {
        let idx = self.s.sheet_index(self.sheet)?;
        let book = self.s.read()?;
        Ok(book
            .display_value(idx, self.row, self.col)
            .chars()
            .collect())
    }

    /// The characters' place in `text` (char indexes).
    fn span(&self, len: usize) -> (usize, usize) {
        let from = (self.start - 1).min(len);
        let to = match self.length {
            Some(n) => from.saturating_add(n).min(len),
            None => len,
        };
        (from, to)
    }

    /// Replace the characters by `with`.
    fn replace(&self, with: &str) -> Result<(), ScriptError> {
        let text = self.text()?;
        let (from, to) = self.span(text.len());
        let mut out: String = text[..from].iter().collect();
        out.push_str(with);
        out.extend(&text[to..]);
        let idx = self.s.sheet_index(self.sheet)?;
        let mut book = self.s.write()?;
        if out.is_empty() {
            let cell = CellRange::from_corners((self.row, self.col), (self.row, self.col));
            if book.is_filled(idx, self.row, self.col) {
                book.clear_range(idx, cell).map_err(engine)?;
            }
            return Ok(());
        }
        book.set_input(idx, self.row, self.col, &out)
            .map_err(engine)
    }

    /// Whether the characters are the cell's whole text.
    fn whole(&self) -> Result<bool, ScriptError> {
        let len = self.text()?.len();
        let (from, to) = self.span(len);
        Ok(from == 0 && to == len)
    }
}

impl Dispatch for CharactersObj {
    fn type_name(&self) -> String {
        "Characters".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "text" | "caption" | "" => {
                let text = self.text()?;
                let (from, to) = self.span(text.len());
                Ok(Value::String(text[from..to].iter().collect()))
            }
            "count" => {
                let text = self.text()?;
                let (from, to) = self.span(text.len());
                Ok(Value::Long((to - from) as i32))
            }
            "font" => Ok(obj(Rc::new(CharFontObj {
                font: FontObj {
                    s: self.s.clone(),
                    sheet: self.sheet,
                    areas: vec![CellRange::from_corners(
                        (self.row, self.col),
                        (self.row, self.col),
                    )],
                },
                whole: self.whole()?,
            }))),
            "insert" => {
                self.replace(&to_str(need(args, 0, "String")?)?)?;
                Ok(Value::Boolean(true))
            }
            "delete" => {
                self.replace("")?;
                Ok(Value::Boolean(true))
            }
            "phoneticcharacters" => Ok(Value::String(String::new())),
            _ => Err(ScriptError::no_member("Characters", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "text" | "caption" | "" => self.replace(&to_str(&value)?),
            "phoneticcharacters" => Ok(()),
            _ => Err(ScriptError::no_member("Characters", name)),
        }
    }
}

/// `Characters(...).Font`: the cell's font, changed only for the whole text.
pub struct CharFontObj {
    pub font: FontObj,
    pub whole: bool,
}

/// Whether `value` is what font property `current` already is.
fn same(current: &Value, value: &Value) -> bool {
    match current {
        Value::Boolean(b) => to_bool(value).is_ok_and(|v| v == *b),
        Value::String(s) => to_str(value).is_ok_and(|v| v.eq_ignore_ascii_case(s)),
        other => match (other.as_f64(), to_f64(value)) {
            (Some(a), Ok(b)) => a == b,
            _ => false,
        },
    }
}

impl Dispatch for CharFontObj {
    fn type_name(&self) -> String {
        "Font".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        self.font.get(name, args)
    }

    fn put(&self, name: &str, args: &[Arg], value: Value) -> Result<(), ScriptError> {
        if self.whole {
            return self.font.put(name, args, value);
        }
        let lower = name.to_ascii_lowercase();
        // What Kantor's fonts don't have is accepted and changes nothing,
        // as on a whole cell.
        if matches!(
            lower.as_str(),
            "superscript"
                | "subscript"
                | "shadow"
                | "outlinefont"
                | "themecolor"
                | "tintandshade"
                | "themefont"
        ) {
            return Ok(());
        }
        match self.font.get(name, args) {
            Ok(current) if same(&current, &value) => Ok(()),
            Err(e) if e.number == 438 => Err(e),
            _ => Err(ScriptError::unsupported(
                "Formatting part of a cell's text (Kantor keeps one font per cell)",
            )),
        }
    }
}
