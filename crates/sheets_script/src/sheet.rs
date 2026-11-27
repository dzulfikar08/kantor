//! `Worksheet`, the `Worksheets` / `Sheets` collection and `Worksheet.Sort`.

use std::any::Any;
use std::rc::Rc;

use kantor_script::{Arg, Dispatch, ScriptError, Value};
use sheets_engine::{CellRange, Tab};

use crate::addr;
use crate::consts::*;
use crate::range::{RangeObj, key_column, range_of, sort_range};
use crate::session::{PendingSort, Selection, Session};
use crate::values::*;

pub struct SheetObj {
    pub s: Rc<Session>,
    /// `sheetId`: stays the same when sheets are added, moved or renamed.
    pub id: u32,
}

impl SheetObj {
    pub fn new(s: &Rc<Session>, id: u32) -> Rc<SheetObj> {
        Rc::new(SheetObj { s: s.clone(), id })
    }

    fn idx(&self) -> Result<u32, ScriptError> {
        self.s.sheet_index(self.id)
    }

    fn range(&self, area: CellRange) -> Value {
        obj(RangeObj::new(&self.s, self.id, area))
    }
}

/// `Range("A1")`, `Range("A1:B2")`, `Range("Data!A1")`, a defined name,
/// `Range("A1,C3")`, `Range(cell1, cell2)` on sheet `sheet`.
pub fn range_on(s: &Rc<Session>, sheet: u32, args: &[Arg]) -> Result<Value, ScriptError> {
    let a = need(args, 0, "Cell1")?;
    if let Some(b) = arg(args, 1, "Cell2") {
        let corner = |v: &Value| -> Result<(u32, CellRange), ScriptError> {
            if let Some(r) = range_of(v) {
                return Ok((r.sheet, r.first()));
            }
            let text = to_str(v)?;
            let r = resolve(s, sheet, &text)?;
            Ok((r.sheet, r.first()))
        };
        let (sa, ra) = corner(a)?;
        let (_, rb) = corner(b)?;
        return Ok(obj(RangeObj::new(
            s,
            sa,
            CellRange {
                top: ra.top.min(rb.top),
                left: ra.left.min(rb.left),
                bottom: ra.bottom.max(rb.bottom),
                right: ra.right.max(rb.right),
            },
        )));
    }
    if let Some(r) = range_of(a) {
        return Ok(obj(r));
    }
    let text = to_str(a)?;
    Ok(obj(resolve(s, sheet, &text)?))
}

/// An address or a defined name as a range (on `sheet` unless it names
/// another).
pub fn resolve(s: &Rc<Session>, sheet: u32, text: &str) -> Result<Rc<RangeObj>, ScriptError> {
    let bad = || {
        err1004(format!(
            "Method 'Range' of object '_Global' failed ({text})"
        ))
    };
    let mut areas = Vec::new();
    let mut on = None;
    for part in split_top_commas(text) {
        let (sheet_name, cells) = addr::split_sheet(part);
        let id = match sheet_name {
            Some(n) => sheet_by_name(s, &n).ok_or_else(bad)?,
            None => sheet,
        };
        if on.is_some_and(|o| o != id) {
            return Err(bad());
        }
        on = Some(id);
        match addr::parse_area(cells) {
            Some(r) => areas.push(r),
            None => {
                // A defined name (sheet-level first).
                let r = defined_name(s, id, cells).ok_or_else(bad)?;
                if areas.is_empty() && r.areas.len() > 1 {
                    return Ok(r);
                }
                on = Some(r.sheet);
                areas.extend(r.areas.iter().copied());
            }
        }
    }
    if areas.is_empty() {
        return Err(bad());
    }
    Ok(RangeObj::multi(s, on.unwrap_or(sheet), areas))
}

fn split_top_commas(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut quote) = (0, false);
    for (i, c) in text.char_indices() {
        match c {
            '\'' => quote = !quote,
            ',' if !quote => {
                out.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&text[start..]);
    out
}

pub fn sheet_by_name(s: &Rc<Session>, name: &str) -> Option<u32> {
    let book = s.read().ok()?;
    let names = book.sheet_names();
    names
        .iter()
        .position(|n| n.eq_ignore_ascii_case(name))
        .map(|i| book.sheet_id(i as u32))
}

/// A defined name that refers to cells.
fn defined_name(s: &Rc<Session>, sheet: u32, name: &str) -> Option<Rc<RangeObj>> {
    let idx = s.sheet_index(sheet).ok()?;
    let names = s.read().ok()?.defined_names();
    let found = names
        .iter()
        .find(|n| n.scope == Some(idx) && n.name.eq_ignore_ascii_case(name))
        .or_else(|| {
            names
                .iter()
                .find(|n| n.scope.is_none() && n.name.eq_ignore_ascii_case(name))
        })?;
    let formula = found.formula.trim_start_matches('=').to_owned();
    if formula.contains('(') {
        return None;
    }
    resolve(s, sheet, &formula).ok()
}

impl Dispatch for SheetObj {
    fn type_name(&self) -> String {
        "Worksheet".into()
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "name" | "" => {
                let idx = self.idx()?;
                Ok(Value::String(
                    self.s.read()?.sheet_names()[idx as usize].clone(),
                ))
            }
            "codename" => {
                let codes = self.s.code_names.borrow();
                Ok(Value::String(
                    codes
                        .iter()
                        .find(|(_, id)| *id == self.id)
                        .map(|(c, _)| c.clone())
                        .unwrap_or_default(),
                ))
            }
            "index" => Ok(Value::Long(self.idx()? as i32 + 1)),
            "range" => range_on(&self.s, self.id, args),
            "cells" if !args.is_empty() => {
                RangeObj::new(&self.s, self.id, CellRange::all()).get("Cells", args)
            }
            "cells" => Ok(self.range(CellRange::all())),
            "rows" => {
                let all = RangeObj::new(&self.s, self.id, CellRange::all());
                all.get("Rows", args)
            }
            "columns" => {
                let all = RangeObj::new(&self.s, self.id, CellRange::all());
                all.get("Columns", args)
            }
            "usedrange" => {
                let idx = self.idx()?;
                let book = self.s.read()?;
                let filled = book.filled_cells(idx);
                let r = if filled.is_empty() {
                    CellRange::from_corners((0, 0), (0, 0))
                } else {
                    let top = filled.iter().map(|c| c.0).min().unwrap_or(0);
                    let bottom = filled.iter().map(|c| c.0).max().unwrap_or(0);
                    let left = filled.iter().map(|c| c.1).min().unwrap_or(0);
                    let right = filled.iter().map(|c| c.1).max().unwrap_or(0);
                    CellRange {
                        top,
                        left,
                        bottom,
                        right,
                    }
                };
                drop(book);
                Ok(self.range(r))
            }
            "visible" => {
                let idx = self.idx()?;
                Ok(Value::Long(if self.s.read()?.sheet_hidden(idx) {
                    XL_SHEET_HIDDEN as i32
                } else {
                    XL_SHEET_VISIBLE as i32
                }))
            }
            "parent" => Ok(obj(Rc::new(crate::app::WorkbookObj::new(&self.s)))),
            "application" => Ok(obj(Rc::new(crate::app::Application::new(&self.s)))),
            "next" | "previous" => {
                let idx = self.idx()?;
                let book = self.s.read()?;
                let n = book.sheet_names().len() as u32;
                let to = if lower == "next" {
                    idx + 1
                } else {
                    idx.wrapping_sub(1)
                };
                if to >= n {
                    return Ok(Value::NOTHING);
                }
                let id = book.sheet_id(to);
                drop(book);
                Ok(obj(SheetObj::new(&self.s, id)))
            }
            "autofiltermode" => {
                let idx = self.idx()?;
                Ok(Value::Boolean(
                    self.s
                        .read()?
                        .filters(idx)
                        .iter()
                        .any(|f| f.scope == sheets_engine::FilterScope::Sheet),
                ))
            }
            "filtermode" => {
                let idx = self.idx()?;
                Ok(Value::Boolean(
                    self.s
                        .read()?
                        .filters(idx)
                        .iter()
                        .any(|f| !f.columns.is_empty()),
                ))
            }
            "autofilter" => {
                let idx = self.idx()?;
                let f = self
                    .s
                    .read()?
                    .filters(idx)
                    .into_iter()
                    .find(|f| f.scope == sheets_engine::FilterScope::Sheet);
                Ok(match f {
                    Some(f) => obj(Rc::new(AutoFilterObj {
                        s: self.s.clone(),
                        sheet: self.id,
                        area: f.area,
                    })),
                    None => Value::NOTHING,
                })
            }
            "sort" => Ok(obj(Rc::new(SortObj::new(&self.s, self.id)))),
            "protectcontents" | "protectdrawingobjects" | "protectscenarios" => {
                Ok(Value::Boolean(false))
            }
            "type" => Ok(Value::Long(XL_WORKSHEET as i32)),
            "standardwidth" => {
                let idx = self.idx()?;
                let book = self.s.read()?;
                let d = book.column_scale(idx).digit_px;
                let px = book.default_column_width(idx);
                Ok(Value::Double(
                    ((px - 5.0) / d * 100.0 + 0.5).trunc() / 100.0,
                ))
            }
            "standardheight" => {
                let idx = self.idx()?;
                Ok(Value::Double(self.s.read()?.default_row_points(idx)))
            }
            "tab" => Ok(Value::NOTHING),
            "shapes" | "chartobjects" | "listobjects" | "pivottables" | "comments"
            | "hyperlinks" | "buttons" => Err(ScriptError::unsupported(&format!(
                "Worksheet.{name} from a macro"
            ))),
            // Methods.
            "activate" | "select" => {
                self.s.active.set(self.id);
                Ok(Value::Empty)
            }
            "delete" => {
                let idx = self.idx()?;
                self.s.write()?.delete_sheet(idx).map_err(engine)?;
                Ok(Value::Boolean(true))
            }
            "copy" | "move" => {
                let at = place(&self.s, args)?;
                let idx = self.idx()?;
                let mut book = self.s.write()?;
                if lower == "copy" {
                    let Some(at) = at else {
                        return Err(ScriptError::unsupported(
                            "Copying a sheet to a new workbook",
                        ));
                    };
                    let new = book.copy_sheet(idx, at).map_err(engine)?;
                    let id = book.sheet_id(new);
                    drop(book);
                    self.s.active.set(id);
                } else {
                    let Some(at) = at else {
                        return Err(ScriptError::unsupported("Moving a sheet to a new workbook"));
                    };
                    let tabs = book.tabs();
                    let from = tabs.iter().position(|t| *t == Tab::Sheet(idx));
                    let to = match from {
                        Some(f) if f < at => at - 1,
                        _ => at,
                    };
                    book.move_tab(Tab::Sheet(idx), to).map_err(engine)?;
                }
                Ok(Value::Empty)
            }
            "protect" | "unprotect" => {
                // Kantor doesn't protect sheets: accepted, nothing changes.
                Ok(Value::Empty)
            }
            "calculate" => {
                self.s.read()?;
                Ok(Value::Empty)
            }
            "paste" => {
                let clip = self
                    .s
                    .clip
                    .get()
                    .ok_or_else(|| err1004("Paste method of Worksheet class failed"))?;
                let target = match arg(args, 0, "Destination").and_then(range_of) {
                    Some(r) => (r.sheet, r.first()),
                    None => (self.id, self.s.selection(self.id).range),
                };
                crate::range::paste(&self.s, clip, target.0, target.1, XL_PASTE_ALL)?;
                Ok(Value::Empty)
            }
            "showalldata" => {
                let idx = self.idx()?;
                let mut book = self.s.write()?;
                if book
                    .filters(idx)
                    .iter()
                    .any(|f| f.scope == sheets_engine::FilterScope::Sheet)
                {
                    book.clear_filter(idx, &sheets_engine::FilterScope::Sheet)
                        .map_err(engine)?;
                }
                Ok(Value::Empty)
            }
            "printout" | "printpreview" | "exportasfixedformat" => Err(ScriptError::unsupported(
                "Printing or exporting from a macro",
            )),
            "evaluate" => crate::app::evaluate(&self.s, self.id, need(args, 0, "Name")?),
            _ => Err(ScriptError::no_member("Worksheet", name)),
        }
    }

    fn put(&self, name: &str, args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "name" => {
                let n = to_str(&value)?;
                let idx = self.idx()?;
                self.s
                    .write()?
                    .rename_sheet(idx, &n)
                    .map_err(|e| err1004(format!("Unable to set the Name property: {e}")))
            }
            "visible" => {
                let hidden = match scalar(&value)? {
                    Value::Boolean(b) => !b,
                    v => to_i64(&v)? != XL_SHEET_VISIBLE,
                };
                let idx = self.idx()?;
                self.s
                    .write()?
                    .set_sheet_hidden(idx, hidden)
                    .map_err(|e| err1004(format!("Unable to set the Visible property: {e}")))
            }
            "autofiltermode" => {
                if to_bool(&value)? {
                    return Ok(());
                }
                let idx = self.idx()?;
                let mut book = self.s.write()?;
                if let Some(f) = book
                    .filters(idx)
                    .into_iter()
                    .find(|f| f.scope == sheets_engine::FilterScope::Sheet)
                {
                    book.toggle_filter(idx, f.area).map_err(engine)?;
                }
                Ok(())
            }
            "enablecalculation" | "enableselection" | "scrollarea" | "displaypagebreaks"
            | "enableautofilter" | "enableoutlining" | "displayrighttoleft" => Ok(()),
            _ => {
                let target = self.get(name, args)?;
                match target {
                    Value::Object(Some(o)) => o.put("", &[], value),
                    _ => Err(ScriptError::no_member("Worksheet", name)),
                }
            }
        }
    }
}

/// The tab position a `Before:=` / `After:=` argument asks for (`None`:
/// neither, a new workbook in Excel).
fn place(s: &Rc<Session>, args: &[Arg]) -> Result<Option<usize>, ScriptError> {
    let target = |v: &Value| -> Result<Option<usize>, ScriptError> {
        let o = v.as_object().ok_or_else(|| ScriptError::runtime(424))?;
        let id = o
            .as_any()
            .and_then(|a| a.downcast_ref::<SheetObj>())
            .map(|w| w.id)
            .ok_or_else(|| ScriptError::runtime(13))?;
        let idx = s.sheet_index(id)?;
        let book = s.read()?;
        Ok(book.tabs().iter().position(|t| *t == Tab::Sheet(idx)))
    };
    if let Some(b) = arg(args, 0, "Before") {
        return Ok(Some(target(b)?.unwrap_or(0)));
    }
    if let Some(a) = arg(args, 1, "After") {
        let n = s.read()?.tabs().len();
        return Ok(Some(target(a)?.map_or(n, |p| p + 1)));
    }
    Ok(None)
}

/// `Worksheets` / `Sheets` (worksheets only: chart sheets aren't in the
/// object model here).
pub struct SheetsObj {
    pub s: Rc<Session>,
}

impl SheetsObj {
    fn item(&self, key: &Value) -> Result<Value, ScriptError> {
        let book = self.s.read()?;
        let names = book.sheet_names();
        let idx = match scalar(key)? {
            Value::String(n) => names
                .iter()
                .position(|x| x.eq_ignore_ascii_case(&n))
                .ok_or_else(|| ScriptError::runtime(9))?,
            Value::Array(_) => {
                return Err(ScriptError::unsupported("Selecting several sheets"));
            }
            v => {
                let i = to_i64(&v)?;
                if i < 1 || i as usize > names.len() {
                    return Err(ScriptError::runtime(9));
                }
                i as usize - 1
            }
        };
        let id = book.sheet_id(idx as u32);
        drop(book);
        Ok(obj(SheetObj::new(&self.s, id)))
    }
}

impl Dispatch for SheetsObj {
    fn type_name(&self) -> String {
        "Sheets".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "" | "item" => self.item(need(args, 0, "Index")?),
            "count" => Ok(Value::Long(self.s.read()?.sheet_names().len() as i32)),
            "add" | "add2" => {
                let count = arg_i64(args, 2, "Count")?.unwrap_or(1).max(1);
                // New sheets go before the active one unless told otherwise.
                let at = match place(&self.s, args)? {
                    Some(at) => at,
                    None => {
                        let active = self.s.sheet_index(self.s.active.get())?;
                        let book = self.s.read()?;
                        book.tabs()
                            .iter()
                            .position(|t| *t == Tab::Sheet(active))
                            .unwrap_or(0)
                    }
                };
                let mut last = None;
                for k in 0..count {
                    let mut book = self.s.write()?;
                    let idx = book.insert_sheet(at + k as usize).map_err(engine)?;
                    last = Some(book.sheet_id(idx));
                }
                let id = last.ok_or_else(|| ScriptError::runtime(5))?;
                // The new sheet becomes the active one.
                self.s.active.set(id);
                self.s.set_selection(id, Selection::cell(0, 0));
                Ok(obj(SheetObj::new(&self.s, id)))
            }
            "visible" => Ok(Value::Boolean(true)),
            "select" => Ok(Value::Empty),
            "parent" => Ok(obj(Rc::new(crate::app::WorkbookObj::new(&self.s)))),
            "copy" | "delete" | "move" | "printout" => Err(ScriptError::unsupported(
                "Copying, moving or deleting all sheets at once",
            )),
            _ => Err(ScriptError::no_member("Sheets", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        let book = self.s.read()?;
        let n = book.sheet_names().len() as u32;
        let ids: Vec<u32> = (0..n).map(|i| book.sheet_id(i)).collect();
        drop(book);
        Ok(ids
            .into_iter()
            .map(|id| obj(SheetObj::new(&self.s, id)))
            .collect())
    }
}

/// `Worksheet.AutoFilter` (the sheet's filter).
pub struct AutoFilterObj {
    s: Rc<Session>,
    sheet: u32,
    area: CellRange,
}

impl Dispatch for AutoFilterObj {
    fn type_name(&self) -> String {
        "AutoFilter".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "range" => Ok(obj(RangeObj::new(&self.s, self.sheet, self.area))),
            "filtermode" => {
                let idx = self.s.sheet_index(self.sheet)?;
                Ok(Value::Boolean(self.s.read()?.filters(idx).iter().any(
                    |f| f.scope == sheets_engine::FilterScope::Sheet && !f.columns.is_empty(),
                )))
            }
            "showalldata" => {
                let idx = self.s.sheet_index(self.sheet)?;
                self.s
                    .write()?
                    .clear_filter(idx, &sheets_engine::FilterScope::Sheet)
                    .map_err(engine)?;
                Ok(Value::Empty)
            }
            "applyfilter" => {
                let idx = self.s.sheet_index(self.sheet)?;
                self.s
                    .write()?
                    .reapply_filter(idx, &sheets_engine::FilterScope::Sheet)
                    .map_err(engine)?;
                Ok(Value::Empty)
            }
            _ => Err(ScriptError::no_member("AutoFilter", name)),
        }
    }
}

/// `Worksheet.Sort`: `SortFields.Clear`, `SortFields.Add Key, , Order`,
/// `SetRange`, `Header`, `Apply` (what recorded macros write).
pub struct SortObj {
    s: Rc<Session>,
    sheet: u32,
}

impl SortObj {
    fn new(s: &Rc<Session>, sheet: u32) -> SortObj {
        SortObj {
            s: s.clone(),
            sheet,
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut PendingSort) -> R) -> R {
        let mut all = self.s.sorts.borrow_mut();
        let p = all.entry(self.sheet).or_insert_with(|| PendingSort {
            header: XL_NO,
            ..Default::default()
        });
        f(p)
    }
}

impl Dispatch for SortObj {
    fn type_name(&self) -> String {
        "Sort".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "sortfields" => Ok(obj(Rc::new(SortFieldsObj {
                sort: SortObj::new(&self.s, self.sheet),
            }))),
            "setrange" => {
                let r = range_of(need(args, 0, "Rng")?).ok_or_else(|| ScriptError::runtime(424))?;
                self.with(|p| p.range = Some(r.first()));
                Ok(Value::Empty)
            }
            "header" => Ok(Value::Long(self.with(|p| p.header) as i32)),
            "matchcase" => Ok(Value::Boolean(false)),
            "orientation" => Ok(Value::Long(XL_TOP_TO_BOTTOM as i32)),
            "rng" => Ok(match self.with(|p| p.range) {
                Some(r) => obj(RangeObj::new(&self.s, self.sheet, r)),
                None => Value::NOTHING,
            }),
            "apply" => {
                let p = self.with(|p| p.clone());
                let range = p
                    .range
                    .ok_or_else(|| err1004("Apply method of Sort class failed"))?;
                sort_range(&self.s, self.sheet, range, &p.keys, p.header)?;
                Ok(Value::Empty)
            }
            _ => Err(ScriptError::no_member("Sort", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "header" => {
                let h = to_i64(&value)?;
                self.with(|p| p.header = h);
                Ok(())
            }
            "matchcase" | "orientation" | "sortmethod" => Ok(()),
            _ => Err(ScriptError::no_member("Sort", name)),
        }
    }
}

pub struct SortFieldsObj {
    sort: SortObj,
}

impl Dispatch for SortFieldsObj {
    fn type_name(&self) -> String {
        "SortFields".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "clear" => {
                self.sort.with(|p| p.keys.clear());
                Ok(Value::Empty)
            }
            "count" => Ok(Value::Long(self.sort.with(|p| p.keys.len()) as i32)),
            "add" | "add2" => {
                let key = need(args, 0, "Key")?;
                let col = key_column(&self.sort.s, self.sort.sheet, key)?;
                let on = arg_i64(args, 1, "SortOn")?.unwrap_or(XL_SORT_ON_VALUES);
                if on != XL_SORT_ON_VALUES {
                    return Err(ScriptError::unsupported("Sorting by colour from a macro"));
                }
                let desc = arg_i64(args, 2, "Order")?.unwrap_or(XL_ASCENDING) == XL_DESCENDING;
                self.sort.with(|p| p.keys.push((col, desc)));
                Ok(Value::Empty)
            }
            _ => Err(ScriptError::no_member("SortFields", name)),
        }
    }
}
