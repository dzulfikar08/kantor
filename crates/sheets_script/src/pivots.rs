//! Pivot tables: `Worksheet.PivotTables`, `PivotTable`, `PivotFields` /
//! `RowFields` / `ColumnFields` / `PageFields` / `DataFields`,
//! `PivotField`, `PivotItems`, `PivotItem`, `Workbook.PivotCaches`,
//! `PivotCache` and `Workbook.RefreshAll`.
//!
//! Every change goes through the engine's PivotTable Fields list operations
//! (`Workbook::pivot_layout` → edit → `Workbook::set_pivot_layout`), Insert
//! PivotTable (`insert_pivot`) and Refresh (`refresh_pivot`,
//! `refresh_all_pivots`), as the grid's commands do, so undo and save work
//! as for changes made by hand. Objects keep the pivot table's sheet
//! (`sheetId`) and name, a field's name and an item's label, and look them
//! up again on each use.

use std::any::Any;
use std::cell::RefCell;
use std::rc::{Rc, Weak};

use kantor_script::{Arg, Dispatch, ScriptError, Value};
use sheets_engine::{
    AreaEntry, AxisEntry, CellRange, PivotDestination, PivotFunc, PivotInfo, PivotLayout,
    PivotSort, ReportLayout, Workbook,
};

use crate::addr;
use crate::consts::{XL_ASCENDING, XL_DESCENDING};
use crate::range::{RangeObj, range_of};
use crate::session::Session;
use crate::sheet::{SheetObj, sheet_by_name};
use crate::values::*;

// XlPivotTableSourceType
pub const XL_DATABASE: i64 = 1;
pub const XL_EXTERNAL: i64 = 2;
pub const XL_CONSOLIDATION: i64 = 3;
// XlPivotFieldOrientation
pub const XL_HIDDEN: i64 = 0;
pub const XL_ROW_FIELD: i64 = 1;
pub const XL_COLUMN_FIELD: i64 = 2;
pub const XL_PAGE_FIELD: i64 = 3;
pub const XL_DATA_FIELD: i64 = 4;
// XlConsolidationFunction
pub const XL_SUM: i64 = -4157;
pub const XL_COUNT: i64 = -4112;
pub const XL_AVERAGE: i64 = -4106;
pub const XL_MAX: i64 = -4136;
pub const XL_MIN: i64 = -4139;
pub const XL_PRODUCT: i64 = -4149;
pub const XL_COUNT_NUMS: i64 = -4113;
pub const XL_STDEV: i64 = -4155;
pub const XL_STDEVP: i64 = -4156;
pub const XL_VAR: i64 = -4164;
pub const XL_VARP: i64 = -4165;
// XlLayoutRowType
pub const XL_COMPACT_ROW: i64 = 0;
pub const XL_TABULAR_ROW: i64 = 1;
pub const XL_OUTLINE_ROW: i64 = 2;
// XlPivotFieldRepeatLabels
pub const XL_DO_NOT_REPEAT_LABELS: i64 = 1;
pub const XL_REPEAT_LABELS: i64 = 2;
/// `AutoSort xlManual`.
pub const XL_MANUAL: i64 = -4135;
// XlPivotTableMissingItems
pub const XL_MISSING_ITEMS_DEFAULT: i64 = -1;
pub const XL_MISSING_ITEMS_NONE: i64 = 0;
// XlPivotTableVersionList
pub const XL_PIVOT_TABLE_VERSION_2000: i64 = 0;
pub const XL_PIVOT_TABLE_VERSION_10: i64 = 1;
pub const XL_PIVOT_TABLE_VERSION_11: i64 = 2;
pub const XL_PIVOT_TABLE_VERSION_12: i64 = 3;
pub const XL_PIVOT_TABLE_VERSION_14: i64 = 4;
pub const XL_PIVOT_TABLE_VERSION_15: i64 = 5;
pub const XL_PIVOT_TABLE_VERSION_CURRENT: i64 = -1;

/// Every constant here by its VBA name, for the interpreter.
pub const CONSTANTS: &[(&str, i64)] = &[
    ("xlDatabase", XL_DATABASE),
    ("xlExternal", XL_EXTERNAL),
    ("xlConsolidation", XL_CONSOLIDATION),
    ("xlHidden", XL_HIDDEN),
    ("xlRowField", XL_ROW_FIELD),
    ("xlColumnField", XL_COLUMN_FIELD),
    ("xlPageField", XL_PAGE_FIELD),
    ("xlDataField", XL_DATA_FIELD),
    ("xlSum", XL_SUM),
    ("xlCount", XL_COUNT),
    ("xlAverage", XL_AVERAGE),
    ("xlMax", XL_MAX),
    ("xlMin", XL_MIN),
    ("xlProduct", XL_PRODUCT),
    ("xlCountNums", XL_COUNT_NUMS),
    ("xlStDev", XL_STDEV),
    ("xlStDevP", XL_STDEVP),
    ("xlVar", XL_VAR),
    ("xlVarP", XL_VARP),
    ("xlCompactRow", XL_COMPACT_ROW),
    ("xlTabularRow", XL_TABULAR_ROW),
    ("xlOutlineRow", XL_OUTLINE_ROW),
    ("xlDoNotRepeatLabels", XL_DO_NOT_REPEAT_LABELS),
    ("xlRepeatLabels", XL_REPEAT_LABELS),
    ("xlManual", XL_MANUAL),
    ("xlMissingItemsDefault", XL_MISSING_ITEMS_DEFAULT),
    ("xlMissingItemsNone", XL_MISSING_ITEMS_NONE),
    ("xlPivotTableVersion2000", XL_PIVOT_TABLE_VERSION_2000),
    ("xlPivotTableVersion10", XL_PIVOT_TABLE_VERSION_10),
    ("xlPivotTableVersion11", XL_PIVOT_TABLE_VERSION_11),
    ("xlPivotTableVersion12", XL_PIVOT_TABLE_VERSION_12),
    ("xlPivotTableVersion14", XL_PIVOT_TABLE_VERSION_14),
    ("xlPivotTableVersion15", XL_PIVOT_TABLE_VERSION_15),
    ("xlPivotTableVersionCurrent", XL_PIVOT_TABLE_VERSION_CURRENT),
];

fn func_of(n: i64) -> Option<PivotFunc> {
    Some(match n {
        XL_SUM => PivotFunc::Sum,
        XL_COUNT => PivotFunc::Count,
        XL_AVERAGE => PivotFunc::Average,
        XL_MAX => PivotFunc::Max,
        XL_MIN => PivotFunc::Min,
        XL_PRODUCT => PivotFunc::Product,
        XL_COUNT_NUMS => PivotFunc::CountNumbers,
        XL_STDEV => PivotFunc::StdDev,
        XL_STDEVP => PivotFunc::StdDevp,
        XL_VAR => PivotFunc::Var,
        XL_VARP => PivotFunc::Varp,
        _ => return None,
    })
}

fn func_code(f: PivotFunc) -> i64 {
    match f {
        PivotFunc::Sum => XL_SUM,
        PivotFunc::Count => XL_COUNT,
        PivotFunc::Average => XL_AVERAGE,
        PivotFunc::Max => XL_MAX,
        PivotFunc::Min => XL_MIN,
        PivotFunc::Product => XL_PRODUCT,
        PivotFunc::CountNumbers => XL_COUNT_NUMS,
        PivotFunc::StdDev => XL_STDEV,
        PivotFunc::StdDevp => XL_STDEVP,
        PivotFunc::Var => XL_VAR,
        PivotFunc::Varp => XL_VARP,
    }
}

/// An edit of the workbook: values brought up to date first (a pivot
/// table reads its source), marked stale after.
fn edit<R>(
    s: &Session,
    f: impl FnOnce(&mut Workbook) -> Result<R, ScriptError>,
) -> Result<R, ScriptError> {
    let r = {
        let mut guard = s.read()?;
        let book: &mut Workbook = &mut guard;
        f(book)
    };
    drop(s.write()?);
    r
}

/// `ThisWorkbook.RefreshAll`: every pivot table, from its source (one
/// step of the run's undo). Web formulas (`WEBSERVICE`, `IMAGE`) and
/// external connections are not refreshed: macros don't go online.
pub fn refresh_all(s: &Rc<Session>) -> Result<(), ScriptError> {
    edit(s, |book| {
        book.refresh_all_pivots().map(|_| ()).map_err(engine)
    })
}

/// `Worksheet.PivotTables` (the collection) or `PivotTables(index)`.
pub fn pivot_tables(s: &Rc<Session>, sheet: u32, args: &[Arg]) -> Result<Value, ScriptError> {
    let c = Rc::new(PivotTablesObj {
        s: s.clone(),
        sheet,
    });
    if args.is_empty() {
        Ok(obj(c))
    } else {
        c.get("", args)
    }
}

/// `Worksheet.PivotTableWizard`.
pub fn pivot_table_wizard() -> Result<Value, ScriptError> {
    Err(ScriptError::unsupported("PivotTableWizard"))
}

/// `Workbook.PivotCaches`.
pub fn pivot_caches(s: &Rc<Session>) -> Value {
    obj(Rc::new(PivotCachesObj { s: s.clone() }))
}

/// `Workbook.PivotCaches(index)` (arguments given).
pub fn pivot_caches_with(s: &Rc<Session>, args: &[Arg]) -> Result<Value, ScriptError> {
    PivotCachesObj { s: s.clone() }.get("", args)
}

/// An area typed as A1 (`A3`, `$A$1:$C$5`) or R1C1 (`R3C1`, `R1C1:R5C3`).
fn cell_area(text: &str) -> Option<CellRange> {
    addr::parse_area(text).or_else(|| addr::parse_area(&addr::r1c1_to_a1(text.trim(), 0, 0)))
}

/// A `SourceData` argument as Insert PivotTable takes it: a range with its
/// sheet (`Sheet1!$A$1:$C$5`), or a table or defined name as given.
fn source_text(s: &Rc<Session>, v: &Value) -> Result<String, ScriptError> {
    if let Some(r) = range_of(v) {
        let idx = r.idx()?;
        let name = s.read()?.sheet_names()[idx as usize].clone();
        return Ok(format!(
            "{}!{}",
            addr::sheet_ref(&name),
            addr::area_text(r.first(), true, true)
        ));
    }
    let text = to_str(v)?;
    let text = text.trim().trim_start_matches('=').trim();
    let (sheet, cells) = addr::split_sheet(text);
    let Some(area) = cell_area(cells) else {
        return Ok(text.to_owned());
    };
    let a = addr::area_text(area, true, true);
    Ok(match sheet {
        Some(n) => format!("{}!{a}", addr::sheet_ref(&n)),
        None => a,
    })
}

/// A source as Excel's `SourceData` reads it: `Sheet1!R1C1:R5C3`.
fn r1c1_source(text: &str) -> String {
    let (sheet, cells) = addr::split_sheet(text);
    match addr::parse_area(cells) {
        Some(a) => {
            let r = addr::area_text_r1c1(a, true, true, 0, 0);
            match sheet {
                Some(n) => format!("{}!{r}", addr::sheet_ref(&n)),
                None => r,
            }
        }
        None => text.to_owned(),
    }
}

/// A `TableDestination`: `(sheetId, row, col)` of its top-left cell.
fn destination(s: &Rc<Session>, v: &Value) -> Result<(u32, u32, u32), ScriptError> {
    if let Some(r) = range_of(v) {
        let a = r.first();
        return Ok((r.sheet, a.top, a.left));
    }
    let text = to_str(v)?;
    let bad = || err1004(format!("The PivotTable destination {text:?} is not valid"));
    let (sheet, cells) = addr::split_sheet(text.trim());
    let id = match sheet {
        Some(n) => sheet_by_name(s, &n).ok_or_else(bad)?,
        None => s.active.get(),
    };
    let area = cell_area(cells).ok_or_else(bad)?;
    Ok((id, area.top, area.left))
}

/// Insert PivotTable: `PivotCache.CreatePivotTable` / `PivotTables.Add`.
/// `dest_i`, `name_i`: the positions of `TableDestination` and `TableName`.
fn create_pivot(
    s: &Rc<Session>,
    source: &str,
    args: &[Arg],
    dest_i: usize,
    name_i: usize,
) -> Result<Value, ScriptError> {
    let table_name = arg_str(args, name_i, "TableName")?
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty());
    let active = s.sheet_index(s.active.get())?;
    let dest = match arg(args, dest_i, "TableDestination") {
        None => PivotDestination::NewSheet,
        Some(v) => {
            let (id, row, col) = destination(s, v)?;
            let sheet = s.sheet_index(id)?;
            if let Some(n) = &table_name
                && s.read()?
                    .pivot_tables(sheet)
                    .iter()
                    .any(|p| p.name.eq_ignore_ascii_case(n))
            {
                return Err(err1004(format!(
                    "A PivotTable report with the name {n:?} already exists on this sheet"
                )));
            }
            PivotDestination::Cell { sheet, row, col }
        }
    };
    let (id, name) = edit(s, |book| {
        let (sheet, row, col) = book.insert_pivot(source, active, dest).map_err(|e| {
            err1004(format!(
                "Unable to get the PivotTables property of the Worksheet class ({e})"
            ))
        })?;
        if let Some(n) = &table_name {
            book.rename_pivot(sheet, row, col, n).map_err(engine)?;
        }
        let name = book
            .pivot_at(sheet, row, col)
            .map(|p| p.name)
            .ok_or_else(|| err1004("The PivotTable was not made"))?;
        Ok((book.sheet_id(sheet), name))
    })?;
    Ok(obj(PivotTableObj::new(s, id, &name)))
}

// ------------------------------------------------------------ PivotCaches

/// The pivot tables of the workbook: `(sheetId, name, source)`.
fn all_pivots(s: &Session) -> Result<Vec<(u32, String, String)>, ScriptError> {
    let book = s.read()?;
    let mut out = Vec::new();
    for idx in 0..book.sheet_names().len() as u32 {
        for p in book.pivot_tables(idx) {
            let source = book
                .pivot_layout(idx, p.area.top, p.area.left)
                .map(|l| l.source)
                .unwrap_or_default();
            out.push((book.sheet_id(idx), p.name, source));
        }
    }
    Ok(out)
}

/// `Workbook.PivotCaches`: one cache per source (pivot tables on the same
/// source share theirs, as in Excel).
pub struct PivotCachesObj {
    s: Rc<Session>,
}

impl PivotCachesObj {
    fn caches(&self) -> Result<Vec<Rc<PivotCacheObj>>, ScriptError> {
        let mut seen: Vec<String> = Vec::new();
        let mut out = Vec::new();
        for (id, name, source) in all_pivots(&self.s)? {
            let key = source.to_lowercase();
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            out.push(Rc::new(PivotCacheObj {
                s: self.s.clone(),
                kind: CacheKind::Of(PivotTableObj::new(&self.s, id, &name)),
            }));
        }
        Ok(out)
    }
}

impl Dispatch for PivotCachesObj {
    fn type_name(&self) -> String {
        "PivotCaches".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "create" | "add" => {
                let kind = arg_i64(args, 0, "SourceType")?.unwrap_or(XL_DATABASE);
                if kind != XL_DATABASE {
                    return Err(ScriptError::unsupported(
                        "A PivotCache on external data or consolidation ranges",
                    ));
                }
                let source = source_text(&self.s, need(args, 1, "SourceData")?)?;
                Ok(obj(Rc::new(PivotCacheObj {
                    s: self.s.clone(),
                    kind: CacheKind::New(source),
                })))
            }
            "count" => Ok(Value::Long(self.caches()?.len() as i32)),
            "" | "item" => {
                let i = to_i64(need(args, 0, "Index")?)?;
                let all = self.caches()?;
                if i < 1 || i as usize > all.len() {
                    return Err(ScriptError::runtime(9));
                }
                Ok(obj(all[i as usize - 1].clone()))
            }
            "parent" => Ok(obj(Rc::new(crate::app::WorkbookObj::new(&self.s)))),
            _ => Err(ScriptError::no_member("PivotCaches", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        Ok(self.caches()?.into_iter().map(obj).collect())
    }
}

enum CacheKind {
    /// Made by `PivotCaches.Create`, no pivot table on it yet: its source.
    New(String),
    /// The cache of a pivot table.
    Of(Rc<PivotTableObj>),
}

pub struct PivotCacheObj {
    s: Rc<Session>,
    kind: CacheKind,
}

impl PivotCacheObj {
    fn source(&self) -> Result<String, ScriptError> {
        match &self.kind {
            CacheKind::New(src) => Ok(src.clone()),
            CacheKind::Of(pt) => Ok(pt.layout()?.source),
        }
    }
}

impl Dispatch for PivotCacheObj {
    fn type_name(&self) -> String {
        "PivotCache".into()
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "sourcedata" => Ok(Value::String(r1c1_source(&self.source()?))),
            "sourcetype" => Ok(Value::Long(XL_DATABASE as i32)),
            "createpivottable" => create_pivot(&self.s, &self.source()?, args, 0, 1),
            "refresh" => {
                if let CacheKind::Of(pt) = &self.kind {
                    pt.refresh()?;
                }
                Ok(Value::Empty)
            }
            "refreshonfileopen" | "backgroundquery" => Ok(Value::Boolean(false)),
            "enablerefresh" | "savedata" => Ok(Value::Boolean(true)),
            "missingitemslimit" => Ok(Value::Long(XL_MISSING_ITEMS_DEFAULT as i32)),
            "parent" => Ok(obj(Rc::new(crate::app::WorkbookObj::new(&self.s)))),
            _ => Err(ScriptError::no_member("PivotCache", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], _value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            // Accepted, nothing changes in Kantor.
            "refreshonfileopen" | "missingitemslimit" | "backgroundquery" | "enablerefresh"
            | "savedata" | "refreshperiod" => Ok(()),
            "sourcedata" => Err(ScriptError::unsupported(
                "Changing a PivotCache's SourceData from a macro",
            )),
            _ => Err(ScriptError::no_member("PivotCache", name)),
        }
    }
}

// ------------------------------------------------------------ PivotTables

pub struct PivotTablesObj {
    s: Rc<Session>,
    sheet: u32,
}

impl PivotTablesObj {
    fn names(&self) -> Result<Vec<String>, ScriptError> {
        let idx = self.s.sheet_index(self.sheet)?;
        Ok(self
            .s
            .read()?
            .pivot_tables(idx)
            .into_iter()
            .map(|p| p.name)
            .collect())
    }
}

impl Dispatch for PivotTablesObj {
    fn type_name(&self) -> String {
        "PivotTables".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "" | "item" => {
                let names = self.names()?;
                let fail =
                    || err1004("Unable to get the PivotTables property of the Worksheet class");
                let found = match scalar(need(args, 0, "Index")?)? {
                    Value::String(n) => names
                        .into_iter()
                        .find(|x| x.eq_ignore_ascii_case(&n))
                        .ok_or_else(fail)?,
                    v => {
                        let i = to_i64(&v)?;
                        if i < 1 || i as usize > names.len() {
                            return Err(fail());
                        }
                        names[i as usize - 1].clone()
                    }
                };
                Ok(obj(PivotTableObj::new(&self.s, self.sheet, &found)))
            }
            "count" => Ok(Value::Long(self.names()?.len() as i32)),
            "add" => {
                let cache = need(args, 0, "PivotCache")?;
                let source = cache
                    .as_object()
                    .and_then(|o| o.as_any())
                    .and_then(|a| a.downcast_ref::<PivotCacheObj>())
                    .ok_or_else(|| ScriptError::runtime(13))?
                    .source()?;
                create_pivot(&self.s, &source, args, 1, 2)
            }
            "parent" => Ok(obj(SheetObj::new(&self.s, self.sheet))),
            _ => Err(ScriptError::no_member("PivotTables", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        Ok(self
            .names()?
            .into_iter()
            .map(|n| obj(PivotTableObj::new(&self.s, self.sheet, &n)))
            .collect())
    }
}

// ------------------------------------------------------------- PivotTable

thread_local! {
    /// `ManualUpdate = True` pivot tables, by run and `(sheetId, name)`.
    static MANUAL: RefCell<Vec<(Weak<Session>, u32, String)>> = const { RefCell::new(Vec::new()) };
}

pub struct PivotTableObj {
    pub s: Rc<Session>,
    /// `sheetId`.
    pub sheet: u32,
    /// Shared with the fields and items made from it (a rename reaches
    /// them).
    name: Rc<RefCell<String>>,
}

impl PivotTableObj {
    pub fn new(s: &Rc<Session>, sheet: u32, name: &str) -> Rc<PivotTableObj> {
        Rc::new(PivotTableObj {
            s: s.clone(),
            sheet,
            name: Rc::new(RefCell::new(name.to_owned())),
        })
    }

    fn handle(&self) -> Rc<PivotTableObj> {
        Rc::new(PivotTableObj {
            s: self.s.clone(),
            sheet: self.sheet,
            name: self.name.clone(),
        })
    }

    fn missing(&self) -> ScriptError {
        err1004(format!(
            "The PivotTable {} is not on its sheet any more",
            self.name.borrow()
        ))
    }

    /// Its sheet's index and where it is now.
    fn locate(&self, book: &Workbook) -> Result<(u32, PivotInfo), ScriptError> {
        let idx = self.s.index_of(book, self.sheet)?;
        let name = self.name.borrow().clone();
        book.pivot_tables(idx)
            .into_iter()
            .find(|p| p.name.eq_ignore_ascii_case(&name))
            .map(|p| (idx, p))
            .ok_or_else(|| self.missing())
    }

    fn layout(&self) -> Result<PivotLayout, ScriptError> {
        let book = self.s.read()?;
        let (idx, p) = self.locate(&book)?;
        book.pivot_layout(idx, p.area.top, p.area.left)
            .ok_or_else(|| self.missing())
    }

    /// Change its fields as the PivotTable Fields list does (laid out
    /// again when anything changed).
    fn change<R>(
        &self,
        f: impl FnOnce(&mut PivotLayout) -> Result<R, ScriptError>,
    ) -> Result<R, ScriptError> {
        edit(&self.s, |book| {
            let (idx, p) = self.locate(book)?;
            let (row, col) = (p.area.top, p.area.left);
            let before = book
                .pivot_layout(idx, row, col)
                .ok_or_else(|| self.missing())?;
            let mut l = before.clone();
            let r = f(&mut l)?;
            if l != before {
                book.set_pivot_layout(idx, row, col, &l).map_err(engine)?;
            }
            Ok(r)
        })
    }

    fn refresh(&self) -> Result<(), ScriptError> {
        edit(&self.s, |book| {
            let (idx, p) = self.locate(book)?;
            book.refresh_pivot(idx, p.area.top, p.area.left)
                .map_err(engine)?;
            Ok(())
        })
    }

    fn field(&self, key: FKey) -> Value {
        obj(Rc::new(PivotFieldObj {
            pt: self.handle(),
            key: RefCell::new(key),
        }))
    }

    fn fields(&self, which: Which, args: &[Arg]) -> Result<Value, ScriptError> {
        let c = Rc::new(PivotFieldsObj {
            pt: self.handle(),
            which,
        });
        if args.is_empty() {
            Ok(obj(c))
        } else {
            c.get("", args)
        }
    }

    fn manual(&self) -> bool {
        let name = self.name.borrow().to_lowercase();
        MANUAL.with(|m| {
            let mut m = m.borrow_mut();
            m.retain(|(w, _, _)| w.strong_count() > 0);
            m.iter().any(|(w, sheet, n)| {
                std::ptr::eq(w.as_ptr(), Rc::as_ptr(&self.s)) && *sheet == self.sheet && *n == name
            })
        })
    }

    fn set_manual(&self, on: bool) {
        let name = self.name.borrow().to_lowercase();
        MANUAL.with(|m| {
            let mut m = m.borrow_mut();
            m.retain(|(w, sheet, n)| {
                w.strong_count() > 0
                    && !(std::ptr::eq(w.as_ptr(), Rc::as_ptr(&self.s))
                        && *sheet == self.sheet
                        && *n == name)
            });
            if on {
                m.push((Rc::downgrade(&self.s), self.sheet, name));
            }
        });
    }

    /// `TableRange1`, `TableRange2`, `DataBodyRange`, `RowRange`,
    /// `ColumnRange`, `PageRange`.
    fn range(&self, which: &str, prop: &str) -> Result<Value, ScriptError> {
        let (p, off, l) = {
            let book = self.s.read()?;
            let (idx, p) = self.locate(&book)?;
            let off = book.pivot_body_offsets(idx, p.area.top, p.area.left);
            let l = book
                .pivot_layout(idx, p.area.top, p.area.left)
                .ok_or_else(|| self.missing())?;
            (p, off, l)
        };
        let fail = || {
            err1004(format!(
                "Unable to get the {prop} property of the PivotTable class"
            ))
        };
        let a = p.area;
        let (_, fdr, fdc) = off.ok_or_else(fail)?;
        let r = match which {
            "tablerange1" => Some(a),
            "tablerange2" => Some(p.bounds),
            "pagerange" if p.bounds.top + 2 <= a.top => Some(CellRange {
                top: p.bounds.top,
                left: a.left,
                bottom: a.top - 2,
                right: a.left + 1,
            }),
            "databodyrange"
                if !l.values.is_empty() && a.top + fdr <= a.bottom && a.left + fdc <= a.right =>
            {
                Some(CellRange {
                    top: a.top + fdr,
                    left: a.left + fdc,
                    bottom: a.bottom,
                    right: a.right,
                })
            }
            "rowrange" if !l.rows.is_empty() && fdc > 0 => Some(CellRange {
                top: a.top + fdr.saturating_sub(1),
                left: a.left,
                bottom: a.bottom,
                right: a.left + fdc - 1,
            }),
            "columnrange" if !l.columns.is_empty() && fdr > 0 && a.left + fdc <= a.right => {
                Some(CellRange {
                    top: a.top,
                    left: a.left + fdc,
                    bottom: a.top + fdr - 1,
                    right: a.right,
                })
            }
            _ => None,
        };
        let r = r.ok_or_else(fail)?;
        Ok(obj(RangeObj::new(&self.s, self.sheet, r)))
    }

    /// `AddDataField Field, Caption, Function`.
    fn add_data_field(&self, args: &[Arg]) -> Result<Value, ScriptError> {
        let field = need(args, 0, "Field")?;
        let fail = || err1004("Unable to run the AddDataField method of the PivotTable class");
        let source = match field
            .as_object()
            .and_then(|o| o.as_any())
            .and_then(|a| a.downcast_ref::<PivotFieldObj>())
        {
            Some(pf) => match &*pf.key.borrow() {
                FKey::Source(n) => n.clone(),
                _ => return Err(fail()),
            },
            None => to_str(field)?,
        };
        let caption = arg_str(args, 1, "Caption")?.filter(|c| !c.trim().is_empty());
        let func = match arg_i64(args, 2, "Function")? {
            Some(n) => Some(func_of(n).ok_or_else(fail)?),
            None => None,
        };
        let k = self.change(|l| {
            let f = source_index(l, &source).ok_or_else(fail)?;
            l.add_value(f);
            let k = l.values.len() - 1;
            if let Some(func) = func {
                l.values[k].func = func;
            }
            if let Some(c) = &caption {
                set_caption(l, k, c)?;
            }
            Ok(k)
        })?;
        let l = self.layout()?;
        if k >= l.values.len() {
            return Err(fail());
        }
        Ok(self.field(FKey::Data(l.value_caption(k))))
    }

    /// `AddFields RowFields, ColumnFields, PageFields, AddToTable`.
    fn add_fields(&self, args: &[Arg]) -> Result<Value, ScriptError> {
        let list = |i: usize, n: &str| -> Result<Vec<String>, ScriptError> {
            Ok(match arg(args, i, n) {
                None => Vec::new(),
                Some(Value::Array(a)) => a
                    .values()
                    .iter()
                    .map(to_str)
                    .collect::<Result<Vec<String>, ScriptError>>()?,
                Some(v) => vec![to_str(v)?],
            })
        };
        let rows = list(0, "RowFields")?;
        let cols = list(1, "ColumnFields")?;
        let pages = list(2, "PageFields")?;
        let add = arg_bool(args, 3, "AddToTable")?.unwrap_or(false);
        let fail = |n: &str| {
            err1004(format!(
                "Unable to run the AddFields method ({n} is not a field)"
            ))
        };
        self.change(|l| {
            if !add {
                l.rows.retain(|e| *e == AxisEntry::Values);
                l.columns.retain(|e| *e == AxisEntry::Values);
                l.filters.clear();
                l.filter_item.clear();
            }
            for (names, area) in [(&rows, "Rows"), (&cols, "Columns"), (&pages, "Filters")] {
                for n in names {
                    let entry = if is_values_name(n) && area != "Filters" {
                        AreaEntry::Values
                    } else {
                        AreaEntry::Field(source_index(l, n).ok_or_else(|| fail(n.as_str()))?)
                    };
                    l.move_entry(entry, area, usize::MAX);
                }
            }
            Ok(())
        })?;
        Ok(Value::Boolean(true))
    }
}

fn source_index(l: &PivotLayout, name: &str) -> Option<usize> {
    l.fields
        .iter()
        .position(|f| f.name.eq_ignore_ascii_case(name.trim()))
}

fn value_index(l: &PivotLayout, caption: &str) -> Option<usize> {
    (0..l.values.len()).find(|&k| l.value_caption(k).eq_ignore_ascii_case(caption.trim()))
}

/// The `Σ Values` field's names.
fn is_values_name(name: &str) -> bool {
    let n = name.trim();
    n.eq_ignore_ascii_case("Data") || n.eq_ignore_ascii_case("Values") || n == "Σ Values"
}

fn has_values_entry(l: &PivotLayout) -> bool {
    l.rows.contains(&AxisEntry::Values) || l.columns.contains(&AxisEntry::Values)
}

/// Name value field `k` `caption` (Excel refuses a source field's name or
/// another value field's).
fn set_caption(l: &mut PivotLayout, k: usize, caption: &str) -> Result<(), ScriptError> {
    let caption = caption.trim();
    let clash = l
        .fields
        .iter()
        .any(|f| f.name.eq_ignore_ascii_case(caption))
        || (0..l.values.len()).any(|j| j != k && l.value_caption(j).eq_ignore_ascii_case(caption));
    if clash {
        return Err(err1004("PivotTable field name already exists."));
    }
    l.values[k].name = None;
    if l.value_caption(k) != caption {
        l.values[k].name = Some(caption.to_owned());
    }
    Ok(())
}

/// Clear field `f`'s filters: label, value and date filters, hidden items
/// and the report filter's item.
fn clear_field(l: &mut PivotLayout, f: usize) {
    let sf = &mut l.fields[f];
    sf.settings.filter = None;
    sf.settings.filter2 = None;
    for it in sf.items.iter_mut() {
        it.hidden = false;
    }
    if let Some(p) = l.filters.iter().position(|x| *x == f) {
        l.filter_item[p] = None;
    }
}

impl Dispatch for PivotTableObj {
    fn type_name(&self) -> String {
        "PivotTable".into()
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "" | "name" => {
                let (_, p) = {
                    let book = self.s.read()?;
                    self.locate(&book)?
                };
                Ok(Value::String(p.name))
            }
            "pivotfields" => self.fields(Which::All, args),
            "rowfields" => self.fields(Which::Rows, args),
            "columnfields" => self.fields(Which::Columns, args),
            "pagefields" => self.fields(Which::Pages, args),
            "datafields" => self.fields(Which::Data, args),
            "pivotcache" => Ok(obj(Rc::new(PivotCacheObj {
                s: self.s.clone(),
                kind: CacheKind::Of(self.handle()),
            }))),
            "sourcedata" => Ok(Value::String(r1c1_source(&self.layout()?.source))),
            "refreshtable" | "update" => {
                self.refresh()?;
                Ok(Value::Boolean(true))
            }
            "tablerange1" => self.range(&lower, "TableRange1"),
            "tablerange2" => self.range(&lower, "TableRange2"),
            "databodyrange" => self.range(&lower, "DataBodyRange"),
            "rowrange" => self.range(&lower, "RowRange"),
            "columnrange" => self.range(&lower, "ColumnRange"),
            "pagerange" => self.range(&lower, "PageRange"),
            "parent" => Ok(obj(SheetObj::new(&self.s, self.sheet))),
            "application" => Ok(obj(Rc::new(crate::app::Application::new(&self.s)))),
            "clearallfilters" => {
                self.change(|l| {
                    for f in 0..l.fields.len() {
                        clear_field(l, f);
                    }
                    Ok(())
                })?;
                Ok(Value::Empty)
            }
            "cleartable" => {
                self.change(|l| {
                    l.rows.clear();
                    l.columns.clear();
                    l.filters.clear();
                    l.filter_item.clear();
                    l.values.clear();
                    Ok(())
                })?;
                Ok(Value::Empty)
            }
            "manualupdate" => Ok(Value::Boolean(self.manual())),
            "adddatafield" => self.add_data_field(args),
            "addfields" => self.add_fields(args),
            "rowaxislayout" => {
                let form = match to_i64(need(args, 0, "RowLayout")?)? {
                    XL_COMPACT_ROW => ReportLayout::Compact,
                    XL_TABULAR_ROW => ReportLayout::Tabular,
                    XL_OUTLINE_ROW => ReportLayout::Outline,
                    _ => {
                        return Err(err1004(
                            "Unable to run the RowAxisLayout method of the PivotTable class",
                        ));
                    }
                };
                self.change(|l| {
                    l.layout = form;
                    Ok(())
                })?;
                Ok(Value::Empty)
            }
            "repeatalllabels" => {
                let on = to_i64(need(args, 0, "Repeat")?)? == XL_REPEAT_LABELS;
                self.change(|l| {
                    for f in l.fields.iter_mut() {
                        f.settings.repeat_labels = on;
                    }
                    Ok(())
                })?;
                Ok(Value::Empty)
            }
            "rowgrand" => Ok(Value::Boolean(self.layout()?.grand_total_column)),
            "columngrand" => Ok(Value::Boolean(self.layout()?.grand_total_row)),
            "allowmultiplefilters" => Ok(Value::Boolean(self.layout()?.multiple_filters)),
            "tablestyle2" => Ok(Value::String(self.layout()?.style.name.unwrap_or_default())),
            "showtablestylerowheaders" => Ok(Value::Boolean(self.layout()?.style.row_headers)),
            "showtablestylecolumnheaders" => {
                Ok(Value::Boolean(self.layout()?.style.column_headers))
            }
            "showtablestylerowstripes" => Ok(Value::Boolean(self.layout()?.style.banded_rows)),
            "showtablestylecolumnstripes" => {
                Ok(Value::Boolean(self.layout()?.style.banded_columns))
            }
            "pivottablewizard" => pivot_table_wizard(),
            "getpivotdata" => Err(ScriptError::unsupported("PivotTable.GetPivotData")),
            "pivotselect" => Err(ScriptError::unsupported("PivotTable.PivotSelect")),
            "showpages" => Err(ScriptError::unsupported("PivotTable.ShowPages")),
            "calculatedfields" => Err(ScriptError::unsupported(
                "PivotTable.CalculatedFields from a macro",
            )),
            _ => Err(ScriptError::no_member("PivotTable", name)),
        }
    }

    fn put(&self, name: &str, args: &[Arg], value: Value) -> Result<(), ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "name" => {
                let new = to_str(&value)?.trim().to_owned();
                edit(&self.s, |book| {
                    let (idx, p) = self.locate(book)?;
                    book.rename_pivot(idx, p.area.top, p.area.left, &new)
                        .map_err(|e| {
                            err1004(format!(
                                "Unable to set the Name property of the PivotTable class ({e})"
                            ))
                        })
                })?;
                let manual = self.manual();
                self.set_manual(false);
                *self.name.borrow_mut() = new;
                self.set_manual(manual);
                Ok(())
            }
            "manualupdate" => {
                // Kept; Kantor lays the pivot table out at each change.
                self.set_manual(to_bool(&value)?);
                Ok(())
            }
            "rowgrand" => {
                let on = to_bool(&value)?;
                self.change(|l| {
                    l.grand_total_column = on;
                    Ok(())
                })
            }
            "columngrand" => {
                let on = to_bool(&value)?;
                self.change(|l| {
                    l.grand_total_row = on;
                    Ok(())
                })
            }
            "allowmultiplefilters" => {
                let on = to_bool(&value)?;
                self.change(|l| {
                    l.multiple_filters = on;
                    if !on {
                        for f in &mut l.fields {
                            f.settings.filter2 = None;
                        }
                    }
                    Ok(())
                })
            }
            "tablestyle2" => {
                let style = to_str(&value)?.trim().to_owned();
                self.change(|l| {
                    l.style.name = (!style.is_empty()).then_some(style);
                    Ok(())
                })
            }
            "showtablestylerowheaders"
            | "showtablestylecolumnheaders"
            | "showtablestylerowstripes"
            | "showtablestylecolumnstripes" => {
                let on = to_bool(&value)?;
                self.change(|l| {
                    match lower.as_str() {
                        "showtablestylerowheaders" => l.style.row_headers = on,
                        "showtablestylecolumnheaders" => l.style.column_headers = on,
                        "showtablestylerowstripes" => l.style.banded_rows = on,
                        _ => l.style.banded_columns = on,
                    }
                    Ok(())
                })
            }
            // What a recorded Insert PivotTable sets: accepted, Kantor's
            // pivot tables look the same either way.
            "hasautoformat"
            | "displayerrorstring"
            | "displaynullstring"
            | "enabledrilldown"
            | "errorstring"
            | "mergelabels"
            | "nullstring"
            | "pagefieldorder"
            | "pagefieldwrapcount"
            | "preserveformatting"
            | "printtitles"
            | "repeatitemsoneachprintedpage"
            | "savedata"
            | "smallgrid"
            | "totalsannotation"
            | "compactrowindent"
            | "ingriddropzones"
            | "displayfieldcaptions"
            | "displaymemberpropertytooltips"
            | "displaycontexttooltips"
            | "showdrillindicators"
            | "printdrillindicators"
            | "sortusingcustomlists"
            | "fieldlistsortascending"
            | "showvaluesrow"
            | "calculatedmembersinfilters"
            | "viewcalculatedmembers"
            | "visualtotals"
            | "showpagemultipleitemlabel"
            | "displayimmediateitems"
            | "enablefieldlist"
            | "enablewizard"
            | "enablefielddialog"
            | "compactlayoutrowheader"
            | "compactlayoutcolumnheader"
            | "grandtotalname"
            | "showtablestylelastcolumn"
            | "location" => Ok(()),
            _ => match self.get(name, args)? {
                Value::Object(Some(o)) => o.put("", &[], value),
                _ => Err(ScriptError::no_member("PivotTable", name)),
            },
        }
    }
}

// ------------------------------------------------------------ PivotFields

/// A field as an object keeps it: a source field by name, a value field by
/// caption, or the `Σ Values` field.
#[derive(Clone, Debug, PartialEq)]
enum FKey {
    Source(String),
    Data(String),
    Values,
}

fn key_name(k: &FKey) -> String {
    match k {
        FKey::Source(n) | FKey::Data(n) => n.clone(),
        FKey::Values => "Values".to_owned(),
    }
}

fn key_is(k: &FKey, name: &str) -> bool {
    match k {
        FKey::Values => is_values_name(name),
        _ => key_name(k).eq_ignore_ascii_case(name.trim()),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Which {
    All,
    Rows,
    Columns,
    Pages,
    Data,
}

pub struct PivotFieldsObj {
    pt: Rc<PivotTableObj>,
    which: Which,
}

impl PivotFieldsObj {
    fn keys(&self, l: &PivotLayout) -> Vec<FKey> {
        let axis = |list: &[AxisEntry]| -> Vec<FKey> {
            list.iter()
                .map(|e| match e {
                    AxisEntry::Field(f) => {
                        FKey::Source(l.fields.get(*f).map(|x| x.name.clone()).unwrap_or_default())
                    }
                    AxisEntry::Values => FKey::Values,
                })
                .collect()
        };
        match self.which {
            Which::All => l
                .fields
                .iter()
                .map(|f| FKey::Source(f.name.clone()))
                .collect(),
            Which::Rows => axis(&l.rows),
            Which::Columns => axis(&l.columns),
            Which::Pages => l
                .filters
                .iter()
                .filter_map(|f| l.fields.get(*f))
                .map(|x| FKey::Source(x.name.clone()))
                .collect(),
            Which::Data => (0..l.values.len())
                .map(|k| FKey::Data(l.value_caption(k)))
                .collect(),
        }
    }
}

impl Dispatch for PivotFieldsObj {
    fn type_name(&self) -> String {
        "PivotFields".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "" | "item" => {
                let l = self.pt.layout()?;
                let keys = self.keys(&l);
                let fail =
                    || err1004("Unable to get the PivotFields property of the PivotTable class");
                let key = match scalar(need(args, 0, "Index")?)? {
                    Value::String(n) => {
                        let found = keys.into_iter().find(|k| key_is(k, &n));
                        // `PivotFields` also gives value fields and `Σ Values`.
                        let found = found.or_else(|| {
                            if self.which != Which::All {
                                return None;
                            }
                            if let Some(k) = value_index(&l, &n) {
                                Some(FKey::Data(l.value_caption(k)))
                            } else if is_values_name(&n) && has_values_entry(&l) {
                                Some(FKey::Values)
                            } else {
                                None
                            }
                        });
                        found.ok_or_else(fail)?
                    }
                    v => {
                        let i = to_i64(&v)?;
                        if i < 1 || i as usize > keys.len() {
                            return Err(fail());
                        }
                        keys[i as usize - 1].clone()
                    }
                };
                Ok(self.pt.field(key))
            }
            "count" => {
                let l = self.pt.layout()?;
                Ok(Value::Long(self.keys(&l).len() as i32))
            }
            "parent" => Ok(obj(self.pt.handle())),
            _ => Err(ScriptError::no_member("PivotFields", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        let l = self.pt.layout()?;
        Ok(self
            .keys(&l)
            .into_iter()
            .map(|k| self.pt.field(k))
            .collect())
    }
}

// ------------------------------------------------------------- PivotField

pub struct PivotFieldObj {
    pt: Rc<PivotTableObj>,
    key: RefCell<FKey>,
}

/// The area name of an orientation.
fn area_name(o: i64) -> Option<&'static str> {
    match o {
        XL_ROW_FIELD => Some("Rows"),
        XL_COLUMN_FIELD => Some("Columns"),
        XL_PAGE_FIELD => Some("Filters"),
        _ => None,
    }
}

/// Where source field `f` is: its orientation and its 0-based position
/// there.
fn place_of(l: &PivotLayout, f: usize) -> (i64, Option<usize>) {
    let e = AxisEntry::Field(f);
    if let Some(p) = l.rows.iter().position(|x| *x == e) {
        (XL_ROW_FIELD, Some(p))
    } else if let Some(p) = l.columns.iter().position(|x| *x == e) {
        (XL_COLUMN_FIELD, Some(p))
    } else if let Some(p) = l.filters.iter().position(|x| *x == f) {
        (XL_PAGE_FIELD, Some(p))
    } else if l.values.iter().any(|v| v.field == f) {
        (XL_DATA_FIELD, None)
    } else {
        (XL_HIDDEN, None)
    }
}

/// Where the `Σ Values` field is.
fn values_place(l: &PivotLayout) -> (i64, Option<usize>) {
    if let Some(p) = l.rows.iter().position(|x| *x == AxisEntry::Values) {
        (XL_ROW_FIELD, Some(p))
    } else if let Some(p) = l.columns.iter().position(|x| *x == AxisEntry::Values) {
        (XL_COLUMN_FIELD, Some(p))
    } else {
        (XL_HIDDEN, None)
    }
}

impl PivotFieldObj {
    fn key(&self) -> FKey {
        self.key.borrow().clone()
    }

    fn gone(&self) -> ScriptError {
        err1004(format!(
            "The PivotField {} is not in the PivotTable any more",
            key_name(&self.key.borrow())
        ))
    }

    fn not_for(&self, prop: &str) -> ScriptError {
        err1004(format!(
            "Unable to get the {prop} property of the PivotField class"
        ))
    }

    fn src(&self, l: &PivotLayout) -> Result<usize, ScriptError> {
        match &*self.key.borrow() {
            FKey::Source(n) => source_index(l, n).ok_or_else(|| self.gone()),
            _ => Err(self.gone()),
        }
    }

    fn data(&self, l: &PivotLayout) -> Result<usize, ScriptError> {
        match &*self.key.borrow() {
            FKey::Data(c) => value_index(l, c).ok_or_else(|| self.gone()),
            _ => Err(self.gone()),
        }
    }

    /// After value field `k` changed: keep it by its caption now.
    fn rekey(&self, k: usize) -> Result<(), ScriptError> {
        let l = self.pt.layout()?;
        if k < l.values.len() {
            *self.key.borrow_mut() = FKey::Data(l.value_caption(k));
        }
        Ok(())
    }

    fn source_name(&self) -> Result<String, ScriptError> {
        match self.key() {
            FKey::Source(n) => Ok(n),
            _ => Err(self.not_for("PivotItems")),
        }
    }

    fn items(&self, args: &[Arg]) -> Result<Value, ScriptError> {
        let c = Rc::new(PivotItemsObj {
            pt: self.pt.handle(),
            field: self.source_name()?,
        });
        if args.is_empty() {
            Ok(obj(c))
        } else {
            c.get("", args)
        }
    }

    fn orientation(&self) -> Result<i64, ScriptError> {
        let l = self.pt.layout()?;
        Ok(match self.key() {
            FKey::Source(_) => place_of(&l, self.src(&l)?).0,
            FKey::Data(_) => {
                self.data(&l)?;
                XL_DATA_FIELD
            }
            FKey::Values => values_place(&l).0,
        })
    }

    fn position(&self) -> Result<i64, ScriptError> {
        let l = self.pt.layout()?;
        let p = match self.key() {
            FKey::Source(_) => place_of(&l, self.src(&l)?).1,
            FKey::Data(_) => Some(self.data(&l)?),
            FKey::Values => values_place(&l).1,
        };
        p.map(|p| p as i64 + 1)
            .ok_or_else(|| self.not_for("Position"))
    }

    fn set_orientation(&self, o: i64) -> Result<(), ScriptError> {
        let fail = || err1004("Unable to set the Orientation property of the PivotField class");
        match self.key() {
            FKey::Source(_) => self.pt.change(|l| {
                let f = self.src(l)?;
                let (now, _) = place_of(l, f);
                match o {
                    XL_HIDDEN => {
                        if matches!(now, XL_DATA_FIELD | XL_HIDDEN) {
                            l.remove_field(f);
                        } else {
                            l.rows.retain(|e| *e != AxisEntry::Field(f));
                            l.columns.retain(|e| *e != AxisEntry::Field(f));
                            if let Some(p) = l.filters.iter().position(|x| *x == f) {
                                l.filters.remove(p);
                                l.filter_item.remove(p);
                            }
                        }
                    }
                    XL_DATA_FIELD => l.add_value(f),
                    _ => {
                        let area = area_name(o).ok_or_else(fail)?;
                        if now != o {
                            l.move_entry(AreaEntry::Field(f), area, usize::MAX);
                        }
                    }
                }
                Ok(())
            }),
            FKey::Data(_) => match o {
                XL_DATA_FIELD => Ok(()),
                XL_HIDDEN => self.pt.change(|l| {
                    let k = self.data(l)?;
                    l.remove_value(k);
                    Ok(())
                }),
                _ => Err(fail()),
            },
            FKey::Values => {
                let area = match o {
                    XL_ROW_FIELD | XL_COLUMN_FIELD => area_name(o).ok_or_else(fail)?,
                    _ => return Err(fail()),
                };
                self.pt.change(|l| {
                    if values_place(l).0 != o {
                        l.move_entry(AreaEntry::Values, area, usize::MAX);
                    }
                    Ok(())
                })
            }
        }
    }

    fn set_position(&self, p: i64) -> Result<(), ScriptError> {
        let fail = || err1004("Unable to set the Position property of the PivotField class");
        if p < 1 {
            return Err(fail());
        }
        let at = p as usize - 1;
        match self.key() {
            FKey::Source(_) => self.pt.change(|l| {
                let f = self.src(l)?;
                let (o, _) = place_of(l, f);
                let area = area_name(o).ok_or_else(fail)?;
                let len = match o {
                    XL_ROW_FIELD => l.rows.len(),
                    XL_COLUMN_FIELD => l.columns.len(),
                    _ => l.filters.len(),
                };
                if at >= len {
                    return Err(fail());
                }
                l.move_entry(AreaEntry::Field(f), area, at);
                Ok(())
            }),
            FKey::Data(_) => self.pt.change(|l| {
                let k = self.data(l)?;
                if at >= l.values.len() {
                    return Err(fail());
                }
                l.move_entry(AreaEntry::Value(k), "Values", at);
                Ok(())
            }),
            FKey::Values => self.pt.change(|l| {
                let (o, _) = values_place(l);
                let area = area_name(o).ok_or_else(fail)?;
                let len = if o == XL_ROW_FIELD {
                    l.rows.len()
                } else {
                    l.columns.len()
                };
                if at >= len {
                    return Err(fail());
                }
                l.move_entry(AreaEntry::Values, area, at);
                Ok(())
            }),
        }
    }

    fn current_page(&self) -> Result<Value, ScriptError> {
        let l = self.pt.layout()?;
        let f = match self.key() {
            FKey::Source(_) => self.src(&l)?,
            _ => return Err(self.not_for("CurrentPage")),
        };
        let p = l
            .filters
            .iter()
            .position(|x| *x == f)
            .ok_or_else(|| self.not_for("CurrentPage"))?;
        Ok(Value::String(
            l.filter_item[p]
                .clone()
                .unwrap_or_else(|| "(All)".to_owned()),
        ))
    }

    fn set_current_page(&self, want: &str) -> Result<(), ScriptError> {
        let fail = || err1004("Unable to set the CurrentPage property of the PivotField class");
        if !matches!(self.key(), FKey::Source(_)) {
            return Err(fail());
        }
        let want = want.trim();
        self.pt.change(|l| {
            let f = self.src(l)?;
            let p = l.filters.iter().position(|x| *x == f).ok_or_else(fail)?;
            let label = if want.eq_ignore_ascii_case("(All)") {
                None
            } else {
                Some(
                    l.fields[f]
                        .items
                        .iter()
                        .find(|it| it.label.eq_ignore_ascii_case(want))
                        .map(|it| it.label.clone())
                        .ok_or_else(fail)?,
                )
            };
            for it in l.fields[f].items.iter_mut() {
                it.hidden = false;
            }
            l.filter_item[p] = label;
            Ok(())
        })
    }

    fn auto_sort(&self, args: &[Arg]) -> Result<Value, ScriptError> {
        let fail = || err1004("Unable to run the AutoSort method of the PivotField class");
        let sort = match to_i64(need(args, 0, "Order")?)? {
            XL_ASCENDING => PivotSort::Ascending,
            XL_DESCENDING => PivotSort::Descending,
            XL_MANUAL => PivotSort::Manual,
            _ => return Err(fail()),
        };
        let by = arg_str(args, 1, "Field")?;
        self.pt.change(|l| {
            let f = self.src(l)?;
            let by_value = match &by {
                Some(n) if !n.trim().eq_ignore_ascii_case(&l.fields[f].name) => {
                    Some(value_index(l, n).ok_or_else(fail)?)
                }
                _ => None,
            };
            let st = &mut l.fields[f].settings;
            st.sort = sort;
            st.sort_by_value = by_value.filter(|_| sort != PivotSort::Manual);
            Ok(())
        })?;
        Ok(Value::Empty)
    }
}

impl Dispatch for PivotFieldObj {
    fn type_name(&self) -> String {
        "PivotField".into()
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "" | "name" | "value" | "caption" => {
                // Still there?
                let l = self.pt.layout()?;
                match self.key() {
                    FKey::Source(_) => {
                        self.src(&l)?;
                    }
                    FKey::Data(_) => {
                        self.data(&l)?;
                    }
                    FKey::Values => {}
                }
                Ok(Value::String(key_name(&self.key())))
            }
            "sourcename" => {
                let l = self.pt.layout()?;
                Ok(Value::String(match self.key() {
                    FKey::Source(_) => l.fields[self.src(&l)?].name.clone(),
                    FKey::Data(_) => {
                        let k = self.data(&l)?;
                        l.fields
                            .get(l.values[k].field)
                            .map(|f| f.name.clone())
                            .unwrap_or_default()
                    }
                    FKey::Values => String::new(),
                }))
            }
            "orientation" => Ok(Value::Long(self.orientation()? as i32)),
            "position" => Ok(Value::Long(self.position()? as i32)),
            "currentpage" => self.current_page(),
            "pivotitems" => self.items(args),
            "function" => {
                let l = self.pt.layout()?;
                if !matches!(self.key(), FKey::Data(_)) {
                    return Err(self.not_for("Function"));
                }
                let k = self.data(&l)?;
                Ok(Value::Long(func_code(l.values[k].func) as i32))
            }
            "numberformat" => {
                let l = self.pt.layout()?;
                if !matches!(self.key(), FKey::Data(_)) {
                    return Err(self.not_for("NumberFormat"));
                }
                let k = self.data(&l)?;
                Ok(Value::String(
                    l.values[k]
                        .num_fmt
                        .clone()
                        .unwrap_or_else(|| "General".to_owned()),
                ))
            }
            "allitemsvisible" => {
                let l = self.pt.layout()?;
                let f = self.src(&l)?;
                let page = l
                    .filters
                    .iter()
                    .position(|x| *x == f)
                    .and_then(|p| l.filter_item[p].clone());
                Ok(Value::Boolean(
                    page.is_none() && l.fields[f].items.iter().all(|it| !it.hidden),
                ))
            }
            "enablemultiplepageitems" => Ok(Value::Boolean(true)),
            "clearallfilters" | "clearmanualfilter" => {
                let all = name.eq_ignore_ascii_case("clearallfilters");
                self.pt.change(|l| {
                    let f = self.src(l)?;
                    if all {
                        clear_field(l, f);
                    } else {
                        for it in l.fields[f].items.iter_mut() {
                            it.hidden = false;
                        }
                    }
                    Ok(())
                })?;
                Ok(Value::Empty)
            }
            "autosort" => self.auto_sort(args),
            "parent" => Ok(obj(self.pt.handle())),
            _ => Err(ScriptError::no_member("PivotField", name)),
        }
    }

    fn put(&self, name: &str, args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "orientation" => self.set_orientation(to_i64(&value)?),
            "position" => self.set_position(to_i64(&value)?),
            "currentpage" => self.set_current_page(&to_str(&value)?),
            "function" => {
                let func = func_of(to_i64(&value)?).ok_or_else(|| {
                    err1004("Unable to set the Function property of the PivotField class")
                })?;
                if !matches!(self.key(), FKey::Data(_)) {
                    return Err(err1004(
                        "Unable to set the Function property of the PivotField class",
                    ));
                }
                let k = self.pt.change(|l| {
                    let k = self.data(l)?;
                    l.values[k].func = func;
                    Ok(k)
                })?;
                self.rekey(k)
            }
            "numberformat" => {
                let code = to_str(&value)?;
                if !matches!(self.key(), FKey::Data(_)) {
                    return Err(ScriptError::unsupported(
                        "NumberFormat of a row, column or page field",
                    ));
                }
                let k = self.pt.change(|l| {
                    let k = self.data(l)?;
                    l.values[k].num_fmt = (!code.trim().is_empty()
                        && !code.eq_ignore_ascii_case("General"))
                    .then(|| code.clone());
                    Ok(k)
                })?;
                self.rekey(k)
            }
            "name" | "caption" => {
                let caption = to_str(&value)?;
                if !matches!(self.key(), FKey::Data(_)) {
                    return Err(ScriptError::unsupported(
                        "Renaming a row, column or page field",
                    ));
                }
                let k = self.pt.change(|l| {
                    let k = self.data(l)?;
                    set_caption(l, k, &caption)?;
                    Ok(k)
                })?;
                self.rekey(k)
            }
            // Kantor's report filters always allow several items.
            "enablemultiplepageitems" => Ok(()),
            _ => match self.get(name, args)? {
                Value::Object(Some(o)) => o.put("", &[], value),
                _ => Err(ScriptError::no_member("PivotField", name)),
            },
        }
    }
}

// ------------------------------------------------------------- PivotItems

pub struct PivotItemsObj {
    pt: Rc<PivotTableObj>,
    /// The source field's name.
    field: String,
}

impl PivotItemsObj {
    fn labels(&self) -> Result<Vec<String>, ScriptError> {
        let l = self.pt.layout()?;
        let f = source_index(&l, &self.field).ok_or_else(|| {
            err1004(format!(
                "The PivotField {} is not in the PivotTable any more",
                self.field
            ))
        })?;
        Ok(l.fields[f]
            .items
            .iter()
            .map(|it| it.label.clone())
            .collect())
    }

    fn item(&self, label: String) -> Value {
        obj(Rc::new(PivotItemObj {
            pt: self.pt.handle(),
            field: self.field.clone(),
            name: label,
        }))
    }
}

impl Dispatch for PivotItemsObj {
    fn type_name(&self) -> String {
        "PivotItems".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "" | "item" => {
                let labels = self.labels()?;
                let fail =
                    || err1004("Unable to get the PivotItems property of the PivotField class");
                let label = match scalar(need(args, 0, "Index")?)? {
                    Value::String(n) => labels
                        .into_iter()
                        .find(|x| x.eq_ignore_ascii_case(n.trim()))
                        .ok_or_else(fail)?,
                    v => {
                        let i = to_i64(&v)?;
                        if i < 1 || i as usize > labels.len() {
                            return Err(fail());
                        }
                        labels[i as usize - 1].clone()
                    }
                };
                Ok(self.item(label))
            }
            "count" => Ok(Value::Long(self.labels()?.len() as i32)),
            "parent" => Ok(self.pt.field(FKey::Source(self.field.clone()))),
            _ => Err(ScriptError::no_member("PivotItems", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        Ok(self
            .labels()?
            .into_iter()
            .map(|label| self.item(label))
            .collect())
    }
}

pub struct PivotItemObj {
    pt: Rc<PivotTableObj>,
    field: String,
    name: String,
}

impl PivotItemObj {
    fn fail(&self) -> ScriptError {
        err1004("Unable to set the Visible property of the PivotItem class")
    }

    fn visible(&self) -> Result<bool, ScriptError> {
        let l = self.pt.layout()?;
        let f = source_index(&l, &self.field).ok_or_else(|| self.fail())?;
        let it = l.fields[f]
            .items
            .iter()
            .find(|it| it.label.eq_ignore_ascii_case(&self.name))
            .ok_or_else(|| self.fail())?;
        let page = l
            .filters
            .iter()
            .position(|x| *x == f)
            .and_then(|p| l.filter_item[p].clone());
        Ok(!it.hidden && page.is_none_or(|p| p.eq_ignore_ascii_case(&self.name)))
    }

    fn set_visible(&self, on: bool) -> Result<(), ScriptError> {
        self.pt.change(|l| {
            let f = source_index(l, &self.field).ok_or_else(|| self.fail())?;
            let k = l.fields[f]
                .items
                .iter()
                .position(|it| it.label.eq_ignore_ascii_case(&self.name))
                .ok_or_else(|| self.fail())?;
            // A report filter showing one item: that item, as hidden items.
            if let Some(p) = l.filters.iter().position(|x| *x == f)
                && let Some(cur) = l.filter_item[p].take()
            {
                for it in l.fields[f].items.iter_mut() {
                    it.hidden = !it.label.eq_ignore_ascii_case(&cur);
                }
            }
            let items = &mut l.fields[f].items;
            items[k].hidden = !on;
            if items.iter().all(|it| it.hidden) {
                return Err(self.fail());
            }
            Ok(())
        })
    }
}

impl Dispatch for PivotItemObj {
    fn type_name(&self) -> String {
        "PivotItem".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "" | "name" | "value" | "caption" | "sourcename" => {
                Ok(Value::String(self.name.clone()))
            }
            "visible" => Ok(Value::Boolean(self.visible()?)),
            "parent" => Ok(self.pt.field(FKey::Source(self.field.clone()))),
            _ => Err(ScriptError::no_member("PivotItem", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "visible" => self.set_visible(to_bool(&value)?),
            "name" | "caption" | "value" => Err(ScriptError::unsupported(
                "Renaming a PivotTable item from a macro",
            )),
            _ => Err(ScriptError::no_member("PivotItem", name)),
        }
    }
}
