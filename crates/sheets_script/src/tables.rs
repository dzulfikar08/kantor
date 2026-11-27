//! `Worksheet.ListObjects` (Excel tables): `ListObjects`, `ListObject`,
//! `ListRows` / `ListRow`, `ListColumns` / `ListColumn`, the table's
//! `AutoFilter` and `Sort`, `Range.ListObject`, `Range.AutoFilter` on a
//! table's cells, and structured references in `Range("Table1[Amount]")`.
//!
//! Objects keep the table by name (shared with the objects made from them,
//! so a rename through one is seen by the others) and look it up on every
//! access. Edits go through the engine's table operations (Format as
//! Table, Table Design, Convert to Range, and `script_tables`' resize and
//! row/column insertion).

use std::any::Any;
use std::cell::RefCell;
use std::rc::{Rc, Weak};

use kantor_script::{Arg, Dispatch, ScriptError, Value};
use sheets_engine::{CellRange, FilterScope, TableInfo, TableOptions, TotalFunc};

use crate::consts::{
    XL_AND, XL_ASCENDING, XL_DESCENDING, XL_GUESS, XL_NO, XL_SORT_ON_VALUES, XL_TOP_TO_BOTTOM,
    XL_YES,
};
use crate::range::{RangeObj, key_column, range_of, sort_range};
use crate::session::Session;
use crate::sheet::SheetObj;
use crate::values::*;

// XlListObjectSourceType.
pub const XL_SRC_EXTERNAL: i64 = 0;
pub const XL_SRC_RANGE: i64 = 1;
pub const XL_SRC_XML: i64 = 2;
pub const XL_SRC_QUERY: i64 = 3;
pub const XL_SRC_MODEL: i64 = 4;
// XlTotalsCalculation.
pub const XL_TOTALS_CALCULATION_NONE: i64 = 0;
pub const XL_TOTALS_CALCULATION_SUM: i64 = 1;
pub const XL_TOTALS_CALCULATION_AVERAGE: i64 = 2;
pub const XL_TOTALS_CALCULATION_COUNT: i64 = 3;
pub const XL_TOTALS_CALCULATION_COUNT_NUMS: i64 = 4;
pub const XL_TOTALS_CALCULATION_MIN: i64 = 5;
pub const XL_TOTALS_CALCULATION_MAX: i64 = 6;
pub const XL_TOTALS_CALCULATION_STD_DEV: i64 = 7;
pub const XL_TOTALS_CALCULATION_VAR: i64 = 8;
pub const XL_TOTALS_CALCULATION_CUSTOM: i64 = 9;

/// Every constant of this module by its VBA name.
pub const CONSTANTS: &[(&str, i64)] = &[
    ("xlSrcExternal", XL_SRC_EXTERNAL),
    ("xlSrcRange", XL_SRC_RANGE),
    ("xlSrcXml", XL_SRC_XML),
    ("xlSrcQuery", XL_SRC_QUERY),
    ("xlSrcModel", XL_SRC_MODEL),
    ("xlTotalsCalculationNone", XL_TOTALS_CALCULATION_NONE),
    ("xlTotalsCalculationSum", XL_TOTALS_CALCULATION_SUM),
    ("xlTotalsCalculationAverage", XL_TOTALS_CALCULATION_AVERAGE),
    ("xlTotalsCalculationCount", XL_TOTALS_CALCULATION_COUNT),
    (
        "xlTotalsCalculationCountNums",
        XL_TOTALS_CALCULATION_COUNT_NUMS,
    ),
    ("xlTotalsCalculationMin", XL_TOTALS_CALCULATION_MIN),
    ("xlTotalsCalculationMax", XL_TOTALS_CALCULATION_MAX),
    ("xlTotalsCalculationStdDev", XL_TOTALS_CALCULATION_STD_DEV),
    ("xlTotalsCalculationVar", XL_TOTALS_CALCULATION_VAR),
    ("xlTotalsCalculationCustom", XL_TOTALS_CALCULATION_CUSTOM),
];

/// `Worksheet.ListObjects` (with arguments: `ListObjects("Table1")`).
pub fn sheet_list_objects(s: &Rc<Session>, sheet: u32, args: &[Arg]) -> Result<Value, ScriptError> {
    let all = ListObjectsObj {
        s: s.clone(),
        sheet,
    };
    if args.is_empty() {
        Ok(obj(Rc::new(all)))
    } else {
        all.item(need(args, 0, "Index")?)
    }
}

/// `Range.ListObject`: the table the range's first cell is in, or `Nothing`.
pub fn range_list_object(r: &RangeObj) -> Result<Value, ScriptError> {
    let idx = r.idx()?;
    let a = r.first();
    let t = r.s.read()?.table_at(idx, a.top, a.left);
    Ok(match t {
        Some(t) => list_object(&r.s, r.sheet, t.name),
        None => Value::NOTHING,
    })
}

/// `Range.AutoFilter [Field, Criteria1, Operator, Criteria2]` on a range
/// whose first cell is in a table: the table's own filter (fields count
/// from the table's first column). `None` when the range isn't in a table
/// (the sheet's AutoFilter then applies).
pub fn range_autofilter(r: &RangeObj, args: &[Arg]) -> Option<Result<Value, ScriptError>> {
    let idx = r.idx().ok()?;
    let a = r.first();
    let t = r.s.read().ok()?.table_at(idx, a.top, a.left)?;
    Some(table_autofilter(&r.s, idx, &t, args))
}

fn table_autofilter(
    s: &Rc<Session>,
    idx: u32,
    t: &TableInfo,
    args: &[Arg],
) -> Result<Value, ScriptError> {
    let fail = || err1004("AutoFilter method of Range class failed");
    let Some(field) = arg_i64(args, 0, "Field")? else {
        // No arguments: the filter buttons go on or off.
        s.write()?.toggle_filter(idx, t.area).map_err(engine)?;
        return Ok(Value::Boolean(true));
    };
    let col = i64::from(t.area.left) + field - 1;
    if field < 1 || col > i64::from(t.area.right) || t.header_rows == 0 {
        return Err(fail());
    }
    let c1 = arg(args, 1, "Criteria1").map(scalar).transpose()?;
    let op = arg_i64(args, 2, "Operator")?.unwrap_or(XL_AND);
    let c2 = arg(args, 3, "Criteria2").map(scalar).transpose()?;
    let kind = crate::range::filter_kind(c1, op, c2)?;
    let mut book = s.write()?;
    if !t.filter_buttons {
        book.toggle_filter(idx, t.area).map_err(engine)?;
    }
    book.set_column_filter(idx, &FilterScope::Table(t.name.clone()), col as u32, kind)
        .map_err(engine)?;
    Ok(Value::Boolean(true))
}

/// The table `name` (any case) and the `sheetId` of its sheet.
fn find_table(s: &Rc<Session>, name: &str) -> Option<(u32, TableInfo)> {
    let book = s.read().ok()?;
    let n = book.sheet_names().len() as u32;
    (0..n).find_map(|i| {
        book.tables(i)
            .into_iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
            .map(|t| (book.sheet_id(i), t))
    })
}

/// A structured reference as a range: `Table1` (its data), `Table1[Amount]`,
/// `Table1[#All]`, `Table1[#Headers]`, `Table1[#Data]`, `Table1[#Totals]`,
/// `Table1[[#All],[Amount]]`, `Table1[[#Headers],[#Data],[Amount]]`,
/// `Table1[[Region]:[Amount]]`. `None` for anything else (`[@Col]` needs a
/// row, which `Range` doesn't have).
pub fn structured_range(s: &Rc<Session>, text: &str) -> Option<Rc<RangeObj>> {
    let text = text.trim();
    let (name, spec) = match text.find('[') {
        Some(i) if text.ends_with(']') => (&text[..i], &text[i + 1..text.len() - 1]),
        Some(_) => return None,
        None => (text, ""),
    };
    if name.is_empty() || name.contains(['!', ':', ' ', '$', '\'']) {
        return None;
    }
    let (sheet, t) = find_table(s, name)?;
    // The items: `[a],[b]:[c]` or a single `a`.
    let mut items: Vec<String> = Vec::new();
    let mut range_sep = false;
    if spec.trim_start().starts_with('[') {
        let mut depth = 0;
        let mut cur = String::new();
        let mut quoted = false;
        for ch in spec.chars() {
            if quoted {
                cur.push(ch);
                quoted = false;
                continue;
            }
            match ch {
                '\'' if depth > 0 => quoted = true,
                '[' => {
                    if depth > 0 {
                        cur.push(ch);
                    }
                    depth += 1;
                }
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        items.push(std::mem::take(&mut cur));
                    } else {
                        cur.push(ch);
                    }
                }
                ':' if depth == 0 => range_sep = true,
                c if depth == 0 => {
                    if c != ',' && !c.is_whitespace() {
                        return None;
                    }
                }
                c => cur.push(c),
            }
        }
        if depth != 0 {
            return None;
        }
    } else if !spec.is_empty() {
        let mut cur = String::new();
        let mut quoted = false;
        for ch in spec.chars() {
            if quoted {
                cur.push(ch);
                quoted = false;
            } else if ch == '\'' {
                quoted = true;
            } else {
                cur.push(ch);
            }
        }
        items.push(cur);
    }
    let (data_top, data_bottom) = t.data_rows();
    let data_bottom = data_bottom.max(data_top);
    let (mut all, mut headers, mut data, mut totals) = (false, false, false, false);
    let mut columns: Vec<u32> = Vec::new();
    for item in &items {
        let item = item.trim();
        match item.to_ascii_lowercase().as_str() {
            "#all" => all = true,
            "#headers" => headers = true,
            "#data" => data = true,
            "#totals" => totals = true,
            "" => {}
            x if x.starts_with('#') || x.starts_with('@') => return None,
            _ => {
                let i = t
                    .columns
                    .iter()
                    .position(|c| c.eq_ignore_ascii_case(item))?;
                columns.push(t.area.left + i as u32);
            }
        }
    }
    let (top, bottom) = if all {
        (t.area.top, t.area.bottom)
    } else {
        let header = t.header_row();
        let total = t.totals_row();
        match (headers, data, totals) {
            (true, true, _) => (header?, data_bottom),
            (true, false, false) => (header?, header?),
            (false, true, true) => (data_top, total?),
            (false, false, true) => (total?, total?),
            _ => (data_top, data_bottom),
        }
    };
    let (left, right) = match columns.as_slice() {
        [] => (t.area.left, t.area.right),
        [c] => (*c, *c),
        [a, b] if range_sep => (*a.min(b), *a.max(b)),
        _ => return None,
    };
    Some(RangeObj::new(
        s,
        sheet,
        CellRange {
            top,
            left,
            bottom,
            right,
        },
    ))
}

/// A table as the objects below keep it: by name, shared so a rename seen
/// through one object is seen by the others made from it.
#[derive(Clone)]
struct Table {
    s: Rc<Session>,
    sheet: u32,
    name: Rc<RefCell<String>>,
}

impl Table {
    fn name(&self) -> String {
        self.name.borrow().clone()
    }

    fn gone(&self) -> ScriptError {
        ScriptError::new(
            424,
            format!("Object required (the table {} was deleted)", self.name()),
        )
    }

    fn info(&self) -> Result<(u32, TableInfo), ScriptError> {
        let idx = self.s.sheet_index(self.sheet)?;
        let name = self.name();
        let t = self
            .s
            .read()?
            .tables(idx)
            .into_iter()
            .find(|t| t.name.eq_ignore_ascii_case(&name));
        t.map(|t| (idx, t)).ok_or_else(|| self.gone())
    }

    fn range(&self, area: CellRange) -> Value {
        obj(RangeObj::new(&self.s, self.sheet, area))
    }

    /// The data rows, `None` when the table has none (or only its empty
    /// insert row).
    fn body(&self) -> Result<Option<CellRange>, ScriptError> {
        let (_, t) = self.info()?;
        if self.s.read()?.table_body_is_empty(&t.name) {
            return Ok(None);
        }
        let d = t.data_range();
        Ok((d.top <= d.bottom).then_some(d))
    }

    fn rows_count(&self) -> Result<u32, ScriptError> {
        Ok(self.body()?.map_or(0, |d| d.rows_count()))
    }

    fn options(&self) -> Result<TableOptions, ScriptError> {
        let name = self.name();
        let o = self.s.read()?.table_options(&name);
        o.ok_or_else(|| self.gone())
    }

    fn set_options(
        &self,
        what: &str,
        f: impl FnOnce(&mut TableOptions),
    ) -> Result<(), ScriptError> {
        let mut o = self.options()?;
        f(&mut o);
        let name = self.name();
        self.s.write()?.set_table_options(&name, o).map_err(|e| {
            err1004(format!(
                "Unable to set the {what} property of the ListObject class ({e})"
            ))
        })
    }

    fn list_object(&self) -> Value {
        obj(Rc::new(ListObjectObj { t: self.clone() }))
    }
}

pub fn list_object(s: &Rc<Session>, sheet: u32, name: String) -> Value {
    obj(Rc::new(ListObjectObj {
        t: Table {
            s: s.clone(),
            sheet,
            name: Rc::new(RefCell::new(name)),
        },
    }))
}

/// A table style by name (a `TableStyle` object's name too): built-in or the
/// workbook's own, in its own spelling; `None` for `""` (no style).
fn style_name(s: &Rc<Session>, v: &Value) -> Result<Option<String>, ScriptError> {
    let text = match v {
        Value::Object(Some(o)) => to_str(&o.get("Name", &[])?)?,
        other => to_str(other)?,
    };
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let custom = s.read()?.custom_table_styles();
    sheets_engine::table_style_names()
        .into_iter()
        .chain(custom)
        .find(|n| n.eq_ignore_ascii_case(text))
        .map(Some)
        .ok_or_else(|| err1004(format!("The table style '{text}' doesn't exist")))
}

/// `Worksheet.ListObjects`.
pub struct ListObjectsObj {
    s: Rc<Session>,
    sheet: u32,
}

impl ListObjectsObj {
    fn names(&self) -> Result<Vec<String>, ScriptError> {
        let idx = self.s.sheet_index(self.sheet)?;
        let tables = self.s.read()?.tables(idx);
        Ok(tables.into_iter().map(|t| t.name).collect())
    }

    fn item(&self, key: &Value) -> Result<Value, ScriptError> {
        let names = self.names()?;
        let name = match scalar(key)? {
            Value::String(n) => names.into_iter().find(|x| x.eq_ignore_ascii_case(&n)),
            v => {
                let i = to_i64(&v)?;
                if i >= 1 {
                    names.into_iter().nth(i as usize - 1)
                } else {
                    None
                }
            }
        }
        .ok_or_else(|| ScriptError::runtime(9))?;
        Ok(list_object(&self.s, self.sheet, name))
    }

    /// `Add([SourceType], [Source], [LinkSource], [XlListObjectHasHeaders],
    /// [Destination], [TableStyleName])`.
    fn add(&self, args: &[Arg]) -> Result<Value, ScriptError> {
        let source_type = arg_i64(args, 0, "SourceType")?.unwrap_or(XL_SRC_RANGE);
        if source_type != XL_SRC_RANGE {
            return Err(ScriptError::unsupported(
                "ListObjects.Add from an external source, XML, a query or the data model",
            ));
        }
        let idx = self.s.sheet_index(self.sheet)?;
        let range = match arg(args, 1, "Source") {
            Some(v) => {
                let r = match range_of(v) {
                    Some(r) => r,
                    None => crate::sheet::resolve(&self.s, self.sheet, &to_str(v)?)?,
                };
                if r.sheet != self.sheet {
                    return Err(err1004(
                        "A table's source range must be on the sheet the table is added to",
                    ));
                }
                let a = r.first();
                if a.is_single() {
                    self.s.read()?.table_suggestion(idx, a, (a.top, a.left))
                } else {
                    a
                }
            }
            None => {
                let sel = self.s.selection(self.sheet);
                self.s.read()?.table_suggestion(idx, sel.range, sel.active)
            }
        };
        let headers = match arg_i64(args, 3, "XlListObjectHasHeaders")?.unwrap_or(XL_GUESS) {
            XL_YES => true,
            XL_NO => false,
            _ => self.s.read()?.guess_header(idx, range),
        };
        let style = match arg(args, 5, "TableStyleName") {
            Some(v) => style_name(&self.s, v)?,
            None => Some(self.s.read()?.default_table_style()),
        };
        let name = self
            .s
            .write()?
            .create_table(
                idx,
                range,
                headers,
                style.as_deref().unwrap_or("TableStyleMedium2"),
            )
            .map_err(|e| err1004(format!("Method 'Add' of object 'ListObjects' failed ({e})")))?;
        let made = Table {
            s: self.s.clone(),
            sheet: self.sheet,
            name: Rc::new(RefCell::new(name)),
        };
        if style.is_none() {
            made.set_options("TableStyle", |o| o.style = None)?;
        }
        Ok(made.list_object())
    }
}

impl Dispatch for ListObjectsObj {
    fn type_name(&self) -> String {
        "ListObjects".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "" | "item" => self.item(need(args, 0, "Index")?),
            "count" => Ok(Value::Long(self.names()?.len() as i32)),
            "add" => self.add(args),
            "parent" => Ok(obj(SheetObj::new(&self.s, self.sheet))),
            "application" => Ok(obj(Rc::new(crate::app::Application::new(&self.s)))),
            _ => Err(ScriptError::no_member("ListObjects", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        Ok(self
            .names()?
            .into_iter()
            .map(|n| list_object(&self.s, self.sheet, n))
            .collect())
    }
}

/// `ListObject`: one table.
pub struct ListObjectObj {
    t: Table,
}

impl ListObjectObj {
    fn rows(&self) -> ListRowsObj {
        ListRowsObj { t: self.t.clone() }
    }

    fn columns(&self) -> ListColumnsObj {
        ListColumnsObj { t: self.t.clone() }
    }

    fn resize(&self, args: &[Arg]) -> Result<(), ScriptError> {
        let r = range_of(need(args, 0, "Range")?).ok_or_else(|| ScriptError::runtime(424))?;
        if r.sheet != self.t.sheet {
            return Err(err1004("Resize method of ListObject class failed"));
        }
        self.t.info()?;
        let name = self.t.name();
        self.t
            .s
            .write()?
            .resize_table(&name, r.first())
            .map_err(|e| err1004(format!("Resize method of ListObject class failed ({e})")))
    }

    /// `Unlist` (Convert to Range); with `clear`, `Delete` (the cells are
    /// cleared too, after showing the rows its filter hid).
    fn unlist(&self, clear: bool) -> Result<(), ScriptError> {
        let (idx, t) = self.t.info()?;
        let mut book = self.t.s.write()?;
        if clear
            && book
                .filter(idx, &FilterScope::Table(t.name.clone()))
                .is_some_and(|f| !f.columns.is_empty())
        {
            book.clear_filter(idx, &FilterScope::Table(t.name.clone()))
                .map_err(engine)?;
        }
        book.convert_table_to_range(&t.name).map_err(engine)?;
        if clear {
            book.clear_all(idx, t.area).map_err(engine)?;
        }
        Ok(())
    }
}

impl Dispatch for ListObjectObj {
    fn type_name(&self) -> String {
        "ListObject".into()
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "name" | "displayname" => {
                let (_, t) = self.t.info()?;
                Ok(Value::String(t.name))
            }
            "range" => {
                let (_, t) = self.t.info()?;
                Ok(self.t.range(t.area))
            }
            "databodyrange" => Ok(match self.t.body()? {
                Some(d) => self.t.range(d),
                None => Value::NOTHING,
            }),
            "headerrowrange" => {
                let (_, t) = self.t.info()?;
                Ok(match t.header_row() {
                    Some(r) => self
                        .t
                        .range(CellRange::from_corners((r, t.area.left), (r, t.area.right))),
                    None => Value::NOTHING,
                })
            }
            "totalsrowrange" => {
                let (_, t) = self.t.info()?;
                Ok(match t.totals_row() {
                    Some(r) => self
                        .t
                        .range(CellRange::from_corners((r, t.area.left), (r, t.area.right))),
                    None => Value::NOTHING,
                })
            }
            "insertrowrange" => {
                let (_, t) = self.t.info()?;
                Ok(if self.t.body()?.is_none() {
                    self.t.range(t.data_range())
                } else {
                    Value::NOTHING
                })
            }
            "showtotals" => Ok(Value::Boolean(self.t.info()?.1.totals_rows > 0)),
            "showheaders" => Ok(Value::Boolean(self.t.info()?.1.header_rows > 0)),
            "showautofilter" | "showautofilterdropdown" => {
                Ok(Value::Boolean(self.t.info()?.1.filter_buttons))
            }
            "tablestyle" => Ok(Value::String(self.t.info()?.1.style.unwrap_or_default())),
            "showtablestylerowstripes" => Ok(Value::Boolean(self.t.info()?.1.row_stripes)),
            "showtablestylecolumnstripes" => Ok(Value::Boolean(self.t.info()?.1.column_stripes)),
            "showtablestylefirstcolumn" => Ok(Value::Boolean(self.t.info()?.1.first_column)),
            "showtablestylelastcolumn" => Ok(Value::Boolean(self.t.info()?.1.last_column)),
            "listrows" if args.is_empty() => Ok(obj(Rc::new(self.rows()))),
            "listrows" => self.rows().get("", args),
            "listcolumns" if args.is_empty() => Ok(obj(Rc::new(self.columns()))),
            "listcolumns" => self.columns().get("", args),
            "autofilter" => {
                let (_, t) = self.t.info()?;
                Ok(if t.filter_buttons {
                    obj(Rc::new(TableAutoFilterObj { t: self.t.clone() }))
                } else {
                    Value::NOTHING
                })
            }
            "sort" => Ok(obj(Rc::new(TableSortObj { t: self.t.clone() }))),
            "sourcetype" => Ok(Value::Long(XL_SRC_RANGE as i32)),
            "active" => Ok(Value::Boolean(false)),
            "parent" => Ok(obj(SheetObj::new(&self.t.s, self.t.sheet))),
            "application" => Ok(obj(Rc::new(crate::app::Application::new(&self.t.s)))),
            "summary" | "alternativetext" | "comment" => Ok(Value::String(String::new())),
            "resize" => self.resize(args).map(|()| Value::Empty),
            "unlist" => self.unlist(false).map(|()| Value::Empty),
            "delete" => self.unlist(true).map(|()| Value::Empty),
            "querytable" | "tableobject" | "refresh" | "updatechanges" | "publish"
            | "exporttovisio" | "slicers" | "xmlmap" => Err(ScriptError::unsupported(&format!(
                "ListObject.{name} (tables linked to external data)"
            ))),
            _ => Err(ScriptError::no_member("ListObject", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "name" | "displayname" => {
                let new = to_str(&value)?.trim().to_owned();
                let old = self.t.name();
                self.t.info()?;
                self.t.s.write()?.rename_table(&old, &new).map_err(|e| {
                    err1004(format!(
                        "Unable to set the Name property of the ListObject class ({e})"
                    ))
                })?;
                *self.t.name.borrow_mut() = new;
                Ok(())
            }
            "showtotals" => {
                let on = to_bool(&value)?;
                self.t.set_options("ShowTotals", |o| o.total_row = on)
            }
            "showheaders" => {
                let on = to_bool(&value)?;
                self.t.set_options("ShowHeaders", |o| o.header_row = on)
            }
            "showautofilter" | "showautofilterdropdown" => {
                let on = to_bool(&value)?;
                let (idx, t) = self.t.info()?;
                if t.filter_buttons != on {
                    if t.header_rows == 0 {
                        return Err(err1004(
                            "Unable to set the ShowAutoFilter property of the ListObject class",
                        ));
                    }
                    self.t
                        .s
                        .write()?
                        .toggle_filter(idx, t.area)
                        .map_err(engine)?;
                }
                Ok(())
            }
            "tablestyle" => {
                let style = style_name(&self.t.s, &value)?;
                self.t.set_options("TableStyle", |o| o.style = style)
            }
            "showtablestylerowstripes" => {
                let on = to_bool(&value)?;
                self.t
                    .set_options("ShowTableStyleRowStripes", |o| o.banded_rows = on)
            }
            "showtablestylecolumnstripes" => {
                let on = to_bool(&value)?;
                self.t
                    .set_options("ShowTableStyleColumnStripes", |o| o.banded_columns = on)
            }
            "showtablestylefirstcolumn" => {
                let on = to_bool(&value)?;
                self.t
                    .set_options("ShowTableStyleFirstColumn", |o| o.first_column = on)
            }
            "showtablestylelastcolumn" => {
                let on = to_bool(&value)?;
                self.t
                    .set_options("ShowTableStyleLastColumn", |o| o.last_column = on)
            }
            "summary" | "alternativetext" | "comment" => Err(ScriptError::unsupported(
                "A table's alternative text or comment",
            )),
            _ => Err(ScriptError::no_member("ListObject", name)),
        }
    }
}

/// `ListObject.ListRows`: the data rows.
pub struct ListRowsObj {
    t: Table,
}

impl ListRowsObj {
    fn row(&self, index: u32) -> Value {
        obj(Rc::new(ListRowObj {
            t: self.t.clone(),
            index,
        }))
    }
}

impl Dispatch for ListRowsObj {
    fn type_name(&self) -> String {
        "ListRows".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "" | "item" => {
                let i = to_i64(need(args, 0, "Index")?)?;
                if i < 1 || i > i64::from(self.t.rows_count()?) {
                    return Err(ScriptError::runtime(9));
                }
                Ok(self.row(i as u32))
            }
            "count" => Ok(Value::Long(self.t.rows_count()? as i32)),
            "add" => {
                let position = arg_i64(args, 0, "Position")?;
                if position.is_some_and(|p| p < 1) {
                    return Err(err1004("Add method of ListRows class failed"));
                }
                self.t.info()?;
                let name = self.t.name();
                let row = self
                    .t
                    .s
                    .write()?
                    .add_table_row(&name, position.map(|p| p as u32))
                    .map_err(|e| err1004(format!("Add method of ListRows class failed ({e})")))?;
                let (top, _) = self.t.info()?.1.data_rows();
                Ok(self.row(row.saturating_sub(top) + 1))
            }
            "parent" => Ok(self.t.list_object()),
            _ => Err(ScriptError::no_member("ListRows", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        Ok((1..=self.t.rows_count()?).map(|i| self.row(i)).collect())
    }
}

/// `ListRow`: one data row, by its position.
pub struct ListRowObj {
    t: Table,
    index: u32,
}

impl Dispatch for ListRowObj {
    fn type_name(&self) -> String {
        "ListRow".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "range" => {
                let (_, t) = self.t.info()?;
                let (top, bottom) = t.data_rows();
                let row = top + self.index - 1;
                if row > bottom {
                    return Err(ScriptError::runtime(9));
                }
                Ok(self.t.range(CellRange::from_corners(
                    (row, t.area.left),
                    (row, t.area.right),
                )))
            }
            "index" => Ok(Value::Long(self.index as i32)),
            "delete" => {
                self.t.info()?;
                let name = self.t.name();
                self.t
                    .s
                    .write()?
                    .delete_table_row(&name, self.index)
                    .map_err(|e| err1004(format!("Delete method of ListRow class failed ({e})")))?;
                Ok(Value::Empty)
            }
            "parent" => Ok(self.t.list_object()),
            _ => Err(ScriptError::no_member("ListRow", name)),
        }
    }
}

/// `ListObject.ListColumns`.
pub struct ListColumnsObj {
    t: Table,
}

impl ListColumnsObj {
    fn column(&self, index: u32) -> Value {
        obj(Rc::new(ListColumnObj {
            t: self.t.clone(),
            index,
        }))
    }
}

impl Dispatch for ListColumnsObj {
    fn type_name(&self) -> String {
        "ListColumns".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "" | "item" => {
                let (_, t) = self.t.info()?;
                let i = match scalar(need(args, 0, "Index")?)? {
                    Value::String(n) => t
                        .columns
                        .iter()
                        .position(|c| c.eq_ignore_ascii_case(n.trim()))
                        .map(|i| i as i64 + 1)
                        .unwrap_or(0),
                    v => to_i64(&v)?,
                };
                if i < 1 || i as usize > t.columns.len() {
                    return Err(ScriptError::runtime(9));
                }
                Ok(self.column(i as u32))
            }
            "count" => Ok(Value::Long(self.t.info()?.1.columns.len() as i32)),
            "add" => {
                let position = arg_i64(args, 0, "Position")?;
                if position.is_some_and(|p| p < 1) {
                    return Err(err1004("Add method of ListColumns class failed"));
                }
                self.t.info()?;
                let name = self.t.name();
                let col = self
                    .t
                    .s
                    .write()?
                    .add_table_column(&name, position.map(|p| p as u32))
                    .map_err(|e| {
                        err1004(format!("Add method of ListColumns class failed ({e})"))
                    })?;
                let left = self.t.info()?.1.area.left;
                Ok(self.column(col.saturating_sub(left) + 1))
            }
            "parent" => Ok(self.t.list_object()),
            _ => Err(ScriptError::no_member("ListColumns", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        let n = self.t.info()?.1.columns.len() as u32;
        Ok((1..=n).map(|i| self.column(i)).collect())
    }
}

/// `ListColumn`: one column, by its position.
pub struct ListColumnObj {
    t: Table,
    index: u32,
}

impl ListColumnObj {
    /// The table and the column's sheet column.
    fn at(&self) -> Result<(u32, TableInfo, u32), ScriptError> {
        let (idx, t) = self.t.info()?;
        if self.index < 1 || self.index as usize > t.columns.len() {
            return Err(ScriptError::runtime(9));
        }
        let col = t.area.left + self.index - 1;
        Ok((idx, t, col))
    }
}

fn total_to_xl(f: TotalFunc) -> i64 {
    match f {
        TotalFunc::None => XL_TOTALS_CALCULATION_NONE,
        TotalFunc::Sum => XL_TOTALS_CALCULATION_SUM,
        TotalFunc::Average => XL_TOTALS_CALCULATION_AVERAGE,
        TotalFunc::Count => XL_TOTALS_CALCULATION_COUNT,
        TotalFunc::CountNumbers => XL_TOTALS_CALCULATION_COUNT_NUMS,
        TotalFunc::Min => XL_TOTALS_CALCULATION_MIN,
        TotalFunc::Max => XL_TOTALS_CALCULATION_MAX,
        TotalFunc::StdDev => XL_TOTALS_CALCULATION_STD_DEV,
        TotalFunc::Var => XL_TOTALS_CALCULATION_VAR,
    }
}

fn xl_to_total(n: i64) -> Result<TotalFunc, ScriptError> {
    Ok(match n {
        XL_TOTALS_CALCULATION_NONE => TotalFunc::None,
        XL_TOTALS_CALCULATION_SUM => TotalFunc::Sum,
        XL_TOTALS_CALCULATION_AVERAGE => TotalFunc::Average,
        XL_TOTALS_CALCULATION_COUNT => TotalFunc::Count,
        XL_TOTALS_CALCULATION_COUNT_NUMS => TotalFunc::CountNumbers,
        XL_TOTALS_CALCULATION_MIN => TotalFunc::Min,
        XL_TOTALS_CALCULATION_MAX => TotalFunc::Max,
        XL_TOTALS_CALCULATION_STD_DEV => TotalFunc::StdDev,
        XL_TOTALS_CALCULATION_VAR => TotalFunc::Var,
        XL_TOTALS_CALCULATION_CUSTOM => {
            return Err(ScriptError::unsupported(
                "A custom total row formula (xlTotalsCalculationCustom)",
            ));
        }
        _ => return Err(ScriptError::runtime(5)),
    })
}

impl Dispatch for ListColumnObj {
    fn type_name(&self) -> String {
        "ListColumn".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "name" => {
                let (_, t, _) = self.at()?;
                Ok(Value::String(t.columns[self.index as usize - 1].clone()))
            }
            "index" => Ok(Value::Long(self.index as i32)),
            "range" => {
                let (_, t, col) = self.at()?;
                Ok(self.t.range(CellRange::from_corners(
                    (t.area.top, col),
                    (t.area.bottom, col),
                )))
            }
            "databodyrange" => {
                let (_, _, col) = self.at()?;
                Ok(match self.t.body()? {
                    Some(d) => self
                        .t
                        .range(CellRange::from_corners((d.top, col), (d.bottom, col))),
                    None => Value::NOTHING,
                })
            }
            "total" => {
                let (_, t, col) = self.at()?;
                Ok(match t.totals_row() {
                    Some(r) => self.t.range(CellRange::from_corners((r, col), (r, col))),
                    None => Value::NOTHING,
                })
            }
            "totalscalculation" => {
                let (_, t, _) = self.at()?;
                let totals = self.t.s.read()?.table_totals(&t.name);
                Ok(Value::Long(total_to_xl(
                    totals
                        .get(self.index as usize - 1)
                        .copied()
                        .unwrap_or(TotalFunc::None),
                ) as i32))
            }
            "delete" => {
                self.at()?;
                let name = self.t.name();
                self.t
                    .s
                    .write()?
                    .delete_table_column(&name, self.index)
                    .map_err(|e| {
                        err1004(format!("Delete method of ListColumn class failed ({e})"))
                    })?;
                Ok(Value::Empty)
            }
            "parent" => Ok(self.t.list_object()),
            _ => Err(ScriptError::no_member("ListColumn", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "name" => {
                let new = to_str(&value)?;
                let (idx, t, col) = self.at()?;
                let header = t.header_row().ok_or_else(|| {
                    err1004("Unable to set the Name property of the ListColumn class (the table has no header row)")
                })?;
                // Typing in the header cell renames the column and the
                // references to it.
                self.t
                    .s
                    .write()?
                    .set_input(idx, header, col, &new)
                    .map_err(engine)
            }
            "totalscalculation" => {
                let f = xl_to_total(to_i64(&value)?)?;
                let (_, t, _) = self.at()?;
                self.t
                    .s
                    .write()?
                    .set_table_total(&t.name, self.index as usize - 1, f)
                    .map_err(|e| {
                        err1004(format!(
                            "Unable to set the TotalsCalculation property of the ListColumn class ({e})"
                        ))
                    })
            }
            _ => Err(ScriptError::no_member("ListColumn", name)),
        }
    }
}

/// `ListObject.AutoFilter`: the table's filter (while it has buttons).
pub struct TableAutoFilterObj {
    t: Table,
}

impl Dispatch for TableAutoFilterObj {
    fn type_name(&self) -> String {
        "AutoFilter".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        let (idx, t) = self.t.info()?;
        let scope = FilterScope::Table(t.name.clone());
        match name.to_ascii_lowercase().as_str() {
            "range" => {
                let f = self.t.s.read()?.filter(idx, &scope);
                Ok(match f {
                    Some(f) => self.t.range(f.area),
                    None => Value::NOTHING,
                })
            }
            "filtermode" => {
                let f = self.t.s.read()?.filter(idx, &scope);
                Ok(Value::Boolean(f.is_some_and(|f| !f.columns.is_empty())))
            }
            "showalldata" => {
                let mut book = self.t.s.write()?;
                if book.filter(idx, &scope).is_some() {
                    book.clear_filter(idx, &scope).map_err(engine)?;
                }
                Ok(Value::Empty)
            }
            "applyfilter" => {
                let mut book = self.t.s.write()?;
                if book.filter(idx, &scope).is_some() {
                    book.reapply_filter(idx, &scope).map_err(engine)?;
                }
                Ok(Value::Empty)
            }
            "sort" => Ok(obj(Rc::new(TableSortObj { t: self.t.clone() }))),
            "parent" => Ok(self.t.list_object()),
            _ => Err(ScriptError::no_member("AutoFilter", name)),
        }
    }
}

/// A table's `Sort` being set up (`SortFields.Add`, `Header`) until
/// `Apply`, for the run (the session) it was made in.
struct PendingTableSort {
    session: Weak<Session>,
    table: String,
    keys: Vec<(u32, bool)>,
    header: i64,
}

thread_local! {
    static SORTS: RefCell<Vec<PendingTableSort>> = const { RefCell::new(Vec::new()) };
}

/// A run on session `s` ended (the engine reuses the session): sorts set up
/// and never applied are dropped.
pub(crate) fn end_run(s: &Session) {
    SORTS.with(|all| {
        all.borrow_mut()
            .retain(|p| p.session.strong_count() > 0 && !std::ptr::eq(p.session.as_ptr(), s))
    });
}

fn with_sort<R>(t: &Table, f: impl FnOnce(&mut PendingTableSort) -> R) -> R {
    SORTS.with(|all| {
        let mut all = all.borrow_mut();
        // Runs that ended leave nothing behind.
        all.retain(|p| p.session.strong_count() > 0);
        let me = Rc::downgrade(&t.s);
        let key = t.name().to_lowercase();
        let i = match all
            .iter()
            .position(|p| Weak::ptr_eq(&p.session, &me) && p.table == key)
        {
            Some(i) => i,
            None => {
                all.push(PendingTableSort {
                    session: me,
                    table: key,
                    keys: Vec::new(),
                    header: XL_YES,
                });
                all.len() - 1
            }
        };
        f(&mut all[i])
    })
}

/// `ListObject.Sort`: sorts the data rows.
pub struct TableSortObj {
    t: Table,
}

impl Dispatch for TableSortObj {
    fn type_name(&self) -> String {
        "Sort".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "sortfields" => Ok(obj(Rc::new(TableSortFieldsObj { t: self.t.clone() }))),
            "header" => Ok(Value::Long(with_sort(&self.t, |p| p.header) as i32)),
            "matchcase" => Ok(Value::Boolean(false)),
            "orientation" => Ok(Value::Long(XL_TOP_TO_BOTTOM as i32)),
            "rng" => Ok(match self.t.body()? {
                Some(d) => self.t.range(d),
                None => Value::NOTHING,
            }),
            "apply" => {
                let keys = with_sort(&self.t, |p| p.keys.clone());
                if let Some(body) = self.t.body()? {
                    sort_range(&self.t.s, self.t.sheet, body, &keys, XL_NO)?;
                }
                Ok(Value::Empty)
            }
            "parent" => Ok(self.t.list_object()),
            _ => Err(ScriptError::no_member("Sort", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "header" => {
                let h = to_i64(&value)?;
                with_sort(&self.t, |p| p.header = h);
                Ok(())
            }
            "matchcase" | "orientation" | "sortmethod" => Ok(()),
            _ => Err(ScriptError::no_member("Sort", name)),
        }
    }
}

pub struct TableSortFieldsObj {
    t: Table,
}

impl Dispatch for TableSortFieldsObj {
    fn type_name(&self) -> String {
        "SortFields".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "clear" => {
                with_sort(&self.t, |p| p.keys.clear());
                Ok(Value::Empty)
            }
            "count" => Ok(Value::Long(with_sort(&self.t, |p| p.keys.len()) as i32)),
            "add" | "add2" => {
                let key = need(args, 0, "Key")?;
                let col = key_column(&self.t.s, self.t.sheet, key)?;
                let on = arg_i64(args, 1, "SortOn")?.unwrap_or(XL_SORT_ON_VALUES);
                if on != XL_SORT_ON_VALUES {
                    return Err(ScriptError::unsupported("Sorting by colour from a macro"));
                }
                let desc = arg_i64(args, 2, "Order")?.unwrap_or(XL_ASCENDING) == XL_DESCENDING;
                with_sort(&self.t, |p| p.keys.push((col, desc)));
                Ok(Value::Empty)
            }
            _ => Err(ScriptError::no_member("SortFields", name)),
        }
    }
}
