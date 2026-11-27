//! `Worksheet.Hyperlinks`, `Range.Hyperlinks` and `Hyperlink`: the links on
//! cells (Insert Link's), through the engine's cell link edits.
//!
//! A `Hyperlink` is kept by its sheet and cell and looked up on each
//! access. Kantor keeps a link per cell: `Hyperlinks.Add` with a range of
//! several cells links its first cell.

use std::rc::Rc;

use kantor_script::{Arg, Dispatch, ScriptError, Value};
use sheets_engine::CellRange;

use crate::range::{RangeObj, range_of};
use crate::session::Session;
use crate::values::*;

// MsoHyperlinkType.
pub const MSO_HYPERLINK_RANGE: i64 = 0;
pub const MSO_HYPERLINK_SHAPE: i64 = 1;
pub const MSO_HYPERLINK_INLINE_SHAPE: i64 = 2;

/// Every constant of this module by its VBA name.
pub const CONSTANTS: &[(&str, i64)] = &[
    ("msoHyperlinkRange", MSO_HYPERLINK_RANGE),
    ("msoHyperlinkShape", MSO_HYPERLINK_SHAPE),
    ("msoHyperlinkInlineShape", MSO_HYPERLINK_INLINE_SHAPE),
];

/// `Worksheet.Hyperlinks`.
pub fn sheet_hyperlinks(s: &Rc<Session>, sheet: u32) -> Value {
    obj(Rc::new(HyperlinksObj {
        s: s.clone(),
        sheet,
        scope: None,
    }))
}

/// `Range.Hyperlinks`: the links on the range's cells.
pub fn range_hyperlinks(r: &RangeObj) -> Value {
    obj(Rc::new(HyperlinksObj {
        s: r.s.clone(),
        sheet: r.sheet,
        scope: Some(r.areas.clone()),
    }))
}

/// `Range.ClearHyperlinks`: removes the links of the range's cells (their
/// text and look stay).
pub fn clear_hyperlinks(r: &RangeObj) -> Result<Value, ScriptError> {
    HyperlinksObj {
        s: r.s.clone(),
        sheet: r.sheet,
        scope: Some(r.areas.clone()),
    }
    .delete_all()?;
    Ok(Value::Empty)
}

/// `Hyperlinks(i)` on a collection got with arguments
/// (`ActiveSheet.Hyperlinks(1)`): the item, else the collection.
pub fn with_args(collection: Value, args: &[Arg]) -> Result<Value, ScriptError> {
    match (&collection, args.is_empty()) {
        (Value::Object(Some(o)), false) => o.get("", args),
        _ => Ok(collection),
    }
}

/// A link's `(address, sub-address, screen tip)`.
type Parts = (String, String, Option<String>);

/// The links of a sheet, or of some of its cells.
pub struct HyperlinksObj {
    s: Rc<Session>,
    sheet: u32,
    scope: Option<Vec<CellRange>>,
}

impl HyperlinksObj {
    /// The linked cells, by row then column.
    fn cells(&self) -> Result<Vec<(u32, u32)>, ScriptError> {
        let idx = self.s.sheet_index(self.sheet)?;
        let links = self.s.read()?.cell_links(idx);
        Ok(links
            .into_iter()
            .map(|l| (l.0, l.1))
            .filter(|&(r, c)| {
                self.scope
                    .as_ref()
                    .is_none_or(|areas| areas.iter().any(|a| a.contains(r, c)))
            })
            .collect())
    }

    fn link(&self, row: u32, col: u32) -> Value {
        obj(Rc::new(HyperlinkObj {
            s: self.s.clone(),
            sheet: self.sheet,
            row,
            col,
        }))
    }

    fn delete_all(&self) -> Result<(), ScriptError> {
        let cells = self.cells()?;
        let idx = self.s.sheet_index(self.sheet)?;
        let mut book = self.s.write()?;
        for (r, c) in cells {
            book.remove_cell_link(idx, r, c).map_err(engine)?;
        }
        Ok(())
    }

    /// `Add(Anchor, Address, [SubAddress], [ScreenTip], [TextToDisplay])`.
    fn add(&self, args: &[Arg]) -> Result<Value, ScriptError> {
        let anchor = need(args, 0, "Anchor")?;
        let r = match range_of(anchor) {
            Some(r) => r,
            None if anchor.is_object() => {
                return Err(ScriptError::unsupported("A hyperlink on a shape"));
            }
            None => return Err(ScriptError::runtime(13)),
        };
        let address = to_str(need(args, 1, "Address")?)?;
        let sub = arg_str(args, 2, "SubAddress")?.unwrap_or_default();
        let tip = arg_str(args, 3, "ScreenTip")?;
        let text = arg_str(args, 4, "TextToDisplay")?;
        let a = r.first();
        let (row, col) = (a.top, a.left);
        let idx = r.idx()?;
        let mut book = self.s.write()?;
        // An empty cell shows the address, as in Excel.
        let text = match text {
            Some(t) => Some(t),
            None if book.input(idx, row, col).is_empty() => Some(if address.trim().is_empty() {
                sub.clone()
            } else {
                address.clone()
            }),
            None => None,
        };
        book.set_cell_link(
            idx,
            row,
            col,
            &address,
            &sub,
            tip.as_deref(),
            text.as_deref(),
        )
        .map_err(|e| err1004(format!("Add method of Hyperlinks class failed ({e})")))?;
        drop(book);
        Ok(obj(Rc::new(HyperlinkObj {
            s: self.s.clone(),
            sheet: r.sheet,
            row,
            col,
        })))
    }
}

impl Dispatch for HyperlinksObj {
    fn type_name(&self) -> String {
        "Hyperlinks".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "" | "item" => {
                let i = to_i64(need(args, 0, "Index")?)?;
                let cells = self.cells()?;
                if i < 1 || i as usize > cells.len() {
                    return Err(ScriptError::runtime(9));
                }
                let (r, c) = cells[i as usize - 1];
                Ok(self.link(r, c))
            }
            "count" => Ok(Value::Long(self.cells()?.len() as i32)),
            "add" => self.add(args),
            "delete" => self.delete_all().map(|()| Value::Empty),
            "parent" => Ok(obj(crate::sheet::SheetObj::new(&self.s, self.sheet))),
            "application" => Ok(obj(Rc::new(crate::app::Application::new(&self.s)))),
            _ => Err(ScriptError::no_member("Hyperlinks", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        Ok(self
            .cells()?
            .into_iter()
            .map(|(r, c)| self.link(r, c))
            .collect())
    }
}

/// One cell's `Hyperlink`.
pub struct HyperlinkObj {
    s: Rc<Session>,
    sheet: u32,
    row: u32,
    col: u32,
}

impl HyperlinkObj {
    /// The sheet's index and the link: `(address, sub-address, screen tip)`.
    fn now(&self) -> Result<(u32, Parts), ScriptError> {
        let idx = self.s.sheet_index(self.sheet)?;
        let link = self.s.read()?.cell_link(idx, self.row, self.col);
        link.map(|l| (idx, l))
            .ok_or_else(|| ScriptError::new(424, "Object required (the hyperlink was deleted)"))
    }

    fn change(
        &self,
        what: &str,
        f: impl FnOnce(&mut Parts) -> Option<String>,
    ) -> Result<(), ScriptError> {
        let (idx, mut link) = self.now()?;
        let text = f(&mut link);
        self.s
            .write()?
            .set_cell_link(
                idx,
                self.row,
                self.col,
                &link.0,
                &link.1,
                link.2.as_deref(),
                text.as_deref(),
            )
            .map_err(|e| {
                err1004(format!(
                    "Unable to set the {what} property of the Hyperlink class ({e})"
                ))
            })
    }
}

impl Dispatch for HyperlinkObj {
    fn type_name(&self) -> String {
        "Hyperlink".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "address" => Ok(Value::String(self.now()?.1.0)),
            "subaddress" => Ok(Value::String(self.now()?.1.1)),
            "screentip" => Ok(Value::String(self.now()?.1.2.unwrap_or_default())),
            "texttodisplay" | "name" => {
                let (idx, _) = self.now()?;
                Ok(Value::String(
                    self.s.read()?.display_value(idx, self.row, self.col),
                ))
            }
            "emailsubject" => {
                let address = self.now()?.1.0;
                let subject = if address.to_ascii_lowercase().starts_with("mailto:") {
                    address.split_once('?').and_then(|(_, q)| {
                        q.split('&').find_map(|p| {
                            p.split_once('=')
                                .filter(|(k, _)| k.eq_ignore_ascii_case("subject"))
                                .map(|(_, v)| v.to_owned())
                        })
                    })
                } else {
                    None
                };
                Ok(Value::String(subject.unwrap_or_default()))
            }
            "type" => Ok(Value::Long(MSO_HYPERLINK_RANGE as i32)),
            "range" => {
                self.now()?;
                Ok(obj(RangeObj::new(
                    &self.s,
                    self.sheet,
                    CellRange::from_corners((self.row, self.col), (self.row, self.col)),
                )))
            }
            "parent" => Ok(obj(crate::sheet::SheetObj::new(&self.s, self.sheet))),
            "application" => Ok(obj(Rc::new(crate::app::Application::new(&self.s)))),
            "delete" => {
                let (idx, _) = self.now()?;
                self.s
                    .write()?
                    .remove_cell_link(idx, self.row, self.col)
                    .map_err(engine)?;
                Ok(Value::Empty)
            }
            "follow" => Err(ScriptError::unsupported(
                "Following a hyperlink from a macro (Hyperlink.Follow)",
            )),
            "shape" => Err(ScriptError::unsupported("A hyperlink on a shape")),
            "addtofavorites" | "createnewdocument" => {
                Err(ScriptError::unsupported(&format!("Hyperlink.{name}")))
            }
            _ => Err(ScriptError::no_member("Hyperlink", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "address" => {
                let v = to_str(&value)?;
                self.change("Address", |l| {
                    l.0 = v;
                    None
                })
            }
            "subaddress" => {
                let v = to_str(&value)?;
                self.change("SubAddress", |l| {
                    l.1 = v;
                    None
                })
            }
            "screentip" => {
                let v = to_str(&value)?;
                self.change("ScreenTip", |l| {
                    l.2 = Some(v);
                    None
                })
            }
            "texttodisplay" => {
                let v = to_str(&value)?;
                self.change("TextToDisplay", |_| Some(v))
            }
            "emailsubject" => Err(ScriptError::unsupported("Hyperlink.EmailSubject")),
            _ => Err(ScriptError::no_member("Hyperlink", name)),
        }
    }
}
