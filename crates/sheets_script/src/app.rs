//! `Application` and the global names (`Range`, `Cells`, `ActiveSheet`…),
//! `Workbook` / `Workbooks`, `WorksheetFunction`, `ActiveWindow` and
//! `Names`.

use std::rc::Rc;

use kantor_script::{Arg, Dispatch, ScriptError, Value};
use sheets_engine::script::KantorValue;
use sheets_engine::{CalcMode, CellRange};

use crate::addr;
use crate::consts::*;
use crate::range::{RangeObj, intersect, range_of};
use crate::session::{Selection, Session};
use crate::sheet::{SheetObj, SheetsObj, range_on, resolve};
use crate::values::*;

/// What `Application` and the globals share.
fn common(s: &Rc<Session>, lower: &str, args: &[Arg]) -> Option<Result<Value, ScriptError>> {
    let active = s.active.get();
    Some(match lower {
        "range" => range_on(s, active, args),
        "cells" | "rows" | "columns" => SheetObj::new(s, active).get(lower, args),
        "activesheet" => Ok(obj(SheetObj::new(s, active))),
        "activecell" => {
            let (r, c) = s.selection(active).active;
            Ok(obj(RangeObj::new(
                s,
                active,
                CellRange::from_corners((r, c), (r, c)),
            )))
        }
        "selection" => Ok(obj(RangeObj::new(s, active, s.selection(active).range))),
        "activeworkbook" | "thisworkbook" => Ok(obj(Rc::new(WorkbookObj::new(s)))),
        "workbooks" if args.is_empty() => Ok(obj(Rc::new(WorkbooksObj { s: s.clone() }))),
        "workbooks" => WorkbooksObj { s: s.clone() }.get("", args),
        "worksheets" | "sheets" if args.is_empty() => Ok(obj(Rc::new(SheetsObj { s: s.clone() }))),
        "worksheets" | "sheets" => SheetsObj { s: s.clone() }.get("", args),
        "application" => Ok(obj(Rc::new(Application::new(s)))),
        "worksheetfunction" => Ok(obj(Rc::new(WorksheetFunction {
            s: s.clone(),
            late: false,
        }))),
        "evaluate" => evaluate(s, active, need_or(args)),
        "intersect" => {
            let mut ranges = Vec::new();
            for a in args {
                match range_of(&a.value) {
                    Some(r) => ranges.push(r),
                    None if a.value.is_missing() => {}
                    None => return Some(Err(ScriptError::runtime(13))),
                }
            }
            let Some(first) = ranges.first().cloned() else {
                return Some(Err(ScriptError::runtime(449)));
            };
            let mut acc = first;
            for r in &ranges[1..] {
                let areas = intersect(&acc, r);
                if areas.is_empty() {
                    return Some(Ok(Value::NOTHING));
                }
                acc = RangeObj::multi(s, acc.sheet, areas);
            }
            Ok(obj(acc))
        }
        "union" => {
            let mut areas = Vec::new();
            let mut sheet = None;
            for a in args {
                let Some(r) = range_of(&a.value) else {
                    if a.value.is_missing() {
                        continue;
                    }
                    return Some(Err(ScriptError::runtime(13)));
                };
                if sheet.is_some_and(|s| s != r.sheet) {
                    return Some(Err(err1004("Method 'Union' of object '_Global' failed")));
                }
                sheet = Some(r.sheet);
                areas.extend(r.areas.iter().copied());
            }
            match sheet {
                Some(id) => Ok(obj(RangeObj::multi(s, id, areas))),
                None => Err(ScriptError::runtime(449)),
            }
        }
        "names" if args.is_empty() => Ok(obj(Rc::new(NamesObj { s: s.clone() }))),
        "names" => NamesObj { s: s.clone() }.get("", args),
        "activewindow" => Ok(obj(Rc::new(WindowObj { s: s.clone() }))),
        "calculate" | "calculatefull" | "calculatefullrebuild" => {
            let r = s.write().map(|mut b| b.recalculate());
            Ok(match r {
                Ok(()) => Value::Empty,
                Err(e) => return Some(Err(e)),
            })
        }
        "goto" => {
            let Some(r) = arg(args, 0, "Reference") else {
                return Some(Err(ScriptError::runtime(449)));
            };
            let r = match range_of(r) {
                Some(r) => r,
                None => match to_str(r).and_then(|t| resolve(s, active, &t)) {
                    Ok(r) => r,
                    Err(e) => return Some(Err(e)),
                },
            };
            s.active.set(r.sheet);
            let a = r.first();
            s.set_selection(
                r.sheet,
                Selection {
                    range: a,
                    active: (a.top, a.left),
                },
            );
            Ok(Value::Empty)
        }
        _ => {
            // Document modules by name: `Sheet1.Range(...)`, `ThisWorkbook`.
            if let Some(id) = s.code_name_sheet(lower) {
                return Some(Ok(obj(SheetObj::new(s, id))));
            }
            return None;
        }
    })
}

fn need_or(args: &[Arg]) -> &Value {
    arg(args, 0, "Name").unwrap_or(&Value::Missing)
}

/// The object whose members scripts use unqualified (`Range`, `Cells`,
/// `ActiveSheet`), like Excel's `_Global`.
pub struct Globals {
    pub s: Rc<Session>,
}

impl Dispatch for Globals {
    fn type_name(&self) -> String {
        "Global".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        common(&self.s, &lower, args).unwrap_or_else(|| Err(ScriptError::no_member("Global", name)))
    }

    fn put(&self, name: &str, args: &[Arg], value: Value) -> Result<(), ScriptError> {
        let lower = name.to_ascii_lowercase();
        match common(&self.s, &lower, args) {
            Some(Ok(Value::Object(Some(o)))) => o.put("", &[], value),
            Some(Ok(_)) => Err(ScriptError::runtime(450)),
            Some(Err(e)) => Err(e),
            None => Err(ScriptError::no_member("Global", name)),
        }
    }
}

pub struct Application {
    s: Rc<Session>,
}

impl Application {
    pub fn new(s: &Rc<Session>) -> Application {
        Application { s: s.clone() }
    }
}

impl Dispatch for Application {
    fn type_name(&self) -> String {
        "Application".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        if let Some(r) = common(&self.s, &lower, args) {
            return r;
        }
        let st = self.s.settings.borrow();
        Ok(match lower.as_str() {
            "" | "name" => Value::string("Microsoft Excel"),
            "screenupdating" => Value::Boolean(st.screen_updating),
            "displayalerts" => Value::Boolean(st.display_alerts),
            "enableevents" => Value::Boolean(st.enable_events),
            "statusbar" => st.status_bar.clone(),
            "displaystatusbar" => Value::Boolean(st.display_status_bar),
            "cursor" => Value::Long(st.cursor),
            "cutcopymode" => Value::Long(match self.s.clip.get() {
                Some(c) if c.cut => 2,
                Some(_) => 1,
                None => 0,
            }),
            "calculation" => {
                drop(st);
                Value::Long(match self.s.read()?.calc_mode() {
                    CalcMode::Automatic => XL_CALCULATION_AUTOMATIC as i32,
                    CalcMode::Manual => XL_CALCULATION_MANUAL as i32,
                })
            }
            "username" => Value::String(self.s.user_name.borrow().clone()),
            "version" => Value::string("16.0"),
            "build" => Value::Long(0),
            "operatingsystem" => Value::string(std::env::consts::OS),
            "pathseparator" => Value::string(std::path::MAIN_SEPARATOR.to_string()),
            "decimalseparator" => Value::string("."),
            "thousandsseparator" => Value::string(","),
            "usesystemseparators" | "interactive" | "visible" => Value::Boolean(true),
            "ready" => Value::Boolean(true),
            "caller" => {
                return Err(ScriptError::unsupported("Application.Caller"));
            }
            "volatile" => Value::Empty,
            "inputbox" => {
                drop(st);
                return input_box(&self.s, args);
            }
            "wait" | "ontime" | "onkey" => {
                return Err(ScriptError::unsupported(&format!(
                    "Application.{name} (timers and keys)"
                )));
            }
            "run" => return Err(ScriptError::unsupported("Application.Run")),
            "quit" => return Err(ScriptError::unsupported("Application.Quit")),
            "getopenfilename" | "getsaveasfilename" | "filedialog" | "dialogs" => {
                return Err(ScriptError::unsupported("File dialogs from a macro"));
            }
            "sendkeys" => return Err(ScriptError::unsupported("SendKeys")),
            _ => {
                drop(st);
                // `Application.Sum(...)`: a worksheet function that returns
                // an error value instead of raising one.
                if is_function(name) {
                    return worksheet_function(&self.s, name, args, true);
                }
                return Err(ScriptError::no_member("Application", name));
            }
        })
    }

    fn put(&self, name: &str, args: &[Arg], value: Value) -> Result<(), ScriptError> {
        let lower = name.to_ascii_lowercase();
        {
            let mut st = self.s.settings.borrow_mut();
            match lower.as_str() {
                "screenupdating" => {
                    st.screen_updating = to_bool(&value)?;
                    return Ok(());
                }
                "displayalerts" => {
                    st.display_alerts = to_bool(&value)?;
                    return Ok(());
                }
                "enableevents" => {
                    st.enable_events = to_bool(&value)?;
                    return Ok(());
                }
                "statusbar" => {
                    st.status_bar = scalar(&value)?;
                    return Ok(());
                }
                "displaystatusbar" => {
                    st.display_status_bar = to_bool(&value)?;
                    return Ok(());
                }
                "cursor" => {
                    st.cursor = to_i64(&value)? as i32;
                    return Ok(());
                }
                "cutcopymode" => {
                    if !to_bool(&value).unwrap_or(false) {
                        self.s.clip.set(None);
                    }
                    return Ok(());
                }
                "interactive"
                | "visible"
                | "enableanimations"
                | "printcommunication"
                | "askToUpdateLinks"
                | "asktoupdatelinks"
                | "displayformulabar"
                | "displayfullscreen"
                | "windowstate"
                | "caption"
                | "iteration"
                | "maxiterations"
                | "maxchange"
                | "calculatebeforesave"
                | "enablecancelkey"
                | "displaycommentindicator"
                | "movedirectionafterenter"
                | "usesystemseparators"
                | "decimalseparator"
                | "thousandsseparator" => {
                    return Ok(());
                }
                _ => {}
            }
        }
        match lower.as_str() {
            "calculation" => {
                let mode = match to_i64(&value)? {
                    XL_CALCULATION_MANUAL => CalcMode::Manual,
                    _ => CalcMode::Automatic,
                };
                let mut book = self.s.write()?;
                book.set_calc_mode(mode);
                if mode == CalcMode::Automatic && book.needs_recalc() {
                    book.recalculate();
                }
                Ok(())
            }
            _ => match common(&self.s, &lower, args) {
                Some(Ok(Value::Object(Some(o)))) => o.put("", &[], value),
                Some(Ok(_)) => Err(ScriptError::runtime(450)),
                Some(Err(e)) => Err(e),
                None => Err(ScriptError::no_member("Application", name)),
            },
        }
    }
}

/// Whether the engine has a function `name`.
pub fn is_function(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    sheets_engine::formula::function_names()
        .binary_search(&upper)
        .is_ok()
}

/// `WorksheetFunction.Name(args…)`: the engine's function evaluated on the
/// active sheet. With `late` (`Application.Name`), an Excel error comes back
/// as an error value; otherwise it raises error 1004.
pub fn worksheet_function(
    s: &Rc<Session>,
    name: &str,
    args: &[Arg],
    late: bool,
) -> Result<Value, ScriptError> {
    if !is_function(name) {
        return Err(ScriptError::no_member("WorksheetFunction", name));
    }
    let mut parts = Vec::with_capacity(args.len());
    for a in args {
        if a.name.is_some() {
            return Err(ScriptError::runtime(448));
        }
        parts.push(match range_of(&a.value) {
            Some(r) => range_ref(s, &r)?,
            None if a.value.is_missing() => String::new(),
            None => formula_literal(&scalar(&a.value)?)?,
        });
    }
    let formula = format!("{}({})", name.to_ascii_uppercase(), parts.join(","));
    let active = s.active.get();
    let idx = s.sheet_index(active)?;
    let result = s
        .read()?
        .evaluate_formula(idx, 0, 0, &formula)
        .map_err(|e| {
            err1004(format!(
                "Unable to get the {name} property of the WorksheetFunction class ({e})"
            ))
        })?;
    match result {
        KantorValue::Error(e) if !late => Err(err1004(format!(
            "Unable to get the {name} property of the WorksheetFunction class ({e})"
        ))),
        KantorValue::Array(rows) if rows.len() == 1 && rows[0].len() == 1 => {
            Ok(from_cell(rows[0][0].clone(), false))
        }
        KantorValue::Empty => Ok(Value::Double(0.0)),
        v => Ok(from_cell(v, false)),
    }
}

/// `'Sheet 1'!$A$1:$B$2` for a range argument (its first area).
fn range_ref(s: &Rc<Session>, r: &RangeObj) -> Result<String, ScriptError> {
    let idx = s.sheet_index(r.sheet)?;
    let name = s.read()?.sheet_names()[idx as usize].clone();
    let a = r.first();
    Ok(format!(
        "{}!{}",
        addr::sheet_ref(&name),
        addr::area_text(a, true, true)
    ))
}

/// `Evaluate("A1:B2")`, `Evaluate("SUM(A1:A3)")`, `[A1]`.
pub fn evaluate(s: &Rc<Session>, sheet: u32, what: &Value) -> Result<Value, ScriptError> {
    if what.is_missing() {
        return Err(ScriptError::runtime(449));
    }
    let text = to_str(what)?;
    let text = text.trim();
    // A reference or a name: the range.
    if let Ok(r) = resolve(s, sheet, text) {
        return Ok(obj(r));
    }
    let idx = s.sheet_index(sheet)?;
    let v = s
        .read()?
        .evaluate_formula(idx, 0, 0, text)
        .map_err(|_| ScriptError::runtime(13))?;
    Ok(match v {
        KantorValue::Array(rows) if rows.len() == 1 && rows[0].len() == 1 => {
            from_cell(rows[0][0].clone(), false)
        }
        v => from_cell(v, false),
    })
}

pub struct WorksheetFunction {
    s: Rc<Session>,
    late: bool,
}

impl Dispatch for WorksheetFunction {
    fn type_name(&self) -> String {
        "WorksheetFunction".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        worksheet_function(&self.s, name, args, self.late)
    }
}

pub struct WorkbookObj {
    s: Rc<Session>,
}

impl WorkbookObj {
    pub fn new(s: &Rc<Session>) -> WorkbookObj {
        WorkbookObj { s: s.clone() }
    }
}

impl Dispatch for WorkbookObj {
    fn type_name(&self) -> String {
        "Workbook".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "" | "name" => Ok(Value::String(self.s.file_name.borrow().clone())),
            "fullname" => Ok(Value::String(self.s.full_name.borrow().clone())),
            "path" => Ok(Value::String(self.s.folder.borrow().clone())),
            "codename" => Ok(Value::string("ThisWorkbook")),
            "worksheets" | "sheets" | "names" | "activesheet" | "application" => {
                common(&self.s, &lower, args).unwrap_or_else(|| Err(ScriptError::runtime(438)))
            }
            "parent" => Ok(obj(Rc::new(Application::new(&self.s)))),
            "saved" => Ok(Value::Boolean(!self.s.read()?.is_dirty())),
            "readonly" | "hasvbproject" => Ok(Value::Boolean(lower == "hasvbproject")),
            "save" => {
                self.s.save_requested.set(true);
                Ok(Value::Empty)
            }
            "activate" | "protect" | "unprotect" => Ok(Value::Empty),
            "saveas" | "savecopyas" => Err(ScriptError::unsupported(
                "Saving under another name from a macro",
            )),
            "close" => Err(ScriptError::unsupported(
                "Closing the workbook from a macro",
            )),
            "refreshall" => Err(ScriptError::unsupported("RefreshAll from a macro")),
            "printout" | "exportasfixedformat" => Err(ScriptError::unsupported(
                "Printing or exporting from a macro",
            )),
            "windows" => Ok(obj(Rc::new(WindowObj { s: self.s.clone() }))),
            _ => Err(ScriptError::no_member("Workbook", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], _value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            // `ThisWorkbook.Saved = True` only changes what Excel asks on
            // close.
            "saved" => Ok(()),
            _ => Err(ScriptError::no_member("Workbook", name)),
        }
    }
}

pub struct WorkbooksObj {
    s: Rc<Session>,
}

impl Dispatch for WorkbooksObj {
    fn type_name(&self) -> String {
        "Workbooks".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "count" => Ok(Value::Long(1)),
            "" | "item" => {
                let key = scalar(need(args, 0, "Index")?)?;
                let ok = match &key {
                    Value::String(n) => {
                        let me = self.s.file_name.borrow();
                        n.eq_ignore_ascii_case(&me)
                            || std::path::Path::new(n.as_str())
                                .file_stem()
                                .zip(std::path::Path::new(me.as_str()).file_stem())
                                .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
                    }
                    v => to_i64(v)? == 1,
                };
                if !ok {
                    return Err(ScriptError::runtime(9));
                }
                Ok(obj(Rc::new(WorkbookObj::new(&self.s))))
            }
            "open" | "add" | "opentext" | "openxml" => Err(ScriptError::unsupported(
                "Opening or creating other workbooks from a macro",
            )),
            _ => Err(ScriptError::no_member("Workbooks", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        Ok(vec![obj(Rc::new(WorkbookObj::new(&self.s)))])
    }
}

/// `ActiveWindow`: freezing panes, gridlines and zoom; scrolling is
/// accepted and does nothing.
pub struct WindowObj {
    s: Rc<Session>,
}

impl Dispatch for WindowObj {
    fn type_name(&self) -> String {
        "Window".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        let id = self.s.active.get();
        let idx = self.s.sheet_index(id)?;
        match name.to_ascii_lowercase().as_str() {
            "" | "item" if !args.is_empty() => Ok(obj(Rc::new(WindowObj { s: self.s.clone() }))),
            "freezepanes" => {
                let (r, c) = self.s.read()?.frozen(idx);
                Ok(Value::Boolean(r > 0 || c > 0))
            }
            "splitrow" => Ok(Value::Long(self.s.read()?.frozen(idx).0 as i32)),
            "splitcolumn" => Ok(Value::Long(self.s.read()?.frozen(idx).1 as i32)),
            "displaygridlines" => Ok(Value::Boolean(self.s.read()?.show_gridlines(idx))),
            "zoom" => Ok(Value::Long(i32::from(self.s.read()?.zoom(idx)))),
            "scrollrow" | "scrollcolumn" => Ok(Value::Long(1)),
            "activesheet" | "activecell" | "selection" => {
                common(&self.s, &name.to_ascii_lowercase(), args)
                    .unwrap_or_else(|| Err(ScriptError::runtime(438)))
            }
            "smallscroll" | "largescroll" | "scrollworkbooktabs" | "activate" => Ok(Value::Empty),
            "windowstate" => Ok(Value::Long(XL_MAXIMIZED as i32)),
            "caption" => Ok(Value::String(self.s.file_name.borrow().clone())),
            "displayheadings"
            | "displayformulas"
            | "displayzeros"
            | "displayworkbooktabs"
            | "displayhorizontalscrollbar"
            | "displayverticalscrollbar"
            | "visible" => Ok(Value::Boolean(true)),
            "view" => Ok(Value::Long(XL_NORMAL_VIEW as i32)),
            _ => Err(ScriptError::no_member("Window", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        let id = self.s.active.get();
        let idx = self.s.sheet_index(id)?;
        match name.to_ascii_lowercase().as_str() {
            "freezepanes" => {
                let (rows, cols) = if to_bool(&value)? {
                    self.s.selection(id).active
                } else {
                    (0, 0)
                };
                self.s
                    .write()?
                    .set_frozen(idx, rows, cols)
                    .map_err(engine)?;
                Ok(())
            }
            "splitrow" | "splitcolumn" => {
                let n = to_i64(&value)?.max(0) as u32;
                let mut book = self.s.write()?;
                let (r, c) = book.frozen(idx);
                let (r, c) = if name.eq_ignore_ascii_case("splitrow") {
                    (n, c)
                } else {
                    (r, n)
                };
                book.set_frozen(idx, r, c).map_err(engine)?;
                Ok(())
            }
            "displaygridlines" => {
                let show = to_bool(&value)?;
                self.s
                    .write()?
                    .set_show_gridlines(idx, show)
                    .map_err(engine)
            }
            "zoom" => {
                let z = to_f64(&value)?.round().clamp(10.0, 400.0) as u16;
                self.s.write()?.set_zoom(idx, z);
                Ok(())
            }
            "view" => {
                let page = to_i64(&value)? == XL_PAGE_BREAK_PREVIEW;
                self.s.write()?.set_break_preview(idx, page);
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// `Names` / `ThisWorkbook.Names`: the workbook's defined names.
pub struct NamesObj {
    s: Rc<Session>,
}

impl Dispatch for NamesObj {
    fn type_name(&self) -> String {
        "Names".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "count" => Ok(Value::Long(self.s.read()?.defined_names().len() as i32)),
            "" | "item" => {
                let names = self.s.read()?.defined_names();
                let key = scalar(need(args, 0, "Index")?)?;
                let found = match &key {
                    Value::String(n) => names.iter().find(|d| d.name.eq_ignore_ascii_case(n)),
                    v => usize::try_from(to_i64(v)? - 1)
                        .ok()
                        .and_then(|i| names.get(i)),
                };
                let d = found.ok_or_else(|| ScriptError::runtime(9))?;
                Ok(obj(Rc::new(NameObj {
                    s: self.s.clone(),
                    name: d.name.clone(),
                    scope: d.scope,
                })))
            }
            "add" => {
                let n = to_str(need(args, 0, "Name")?)?;
                let refers = match arg(args, 1, "RefersTo") {
                    Some(v) => match range_of(v) {
                        Some(r) => range_ref(&self.s, &r)?,
                        None => to_str(v)?,
                    },
                    None => {
                        let r = arg(args, 4, "RefersToR1C1")
                            .ok_or_else(|| ScriptError::runtime(449))?;
                        let t = to_str(r)?;
                        addr::r1c1_to_a1(&t, 0, 0)
                    }
                };
                let refers = refers.trim_start_matches('=').to_owned();
                self.s
                    .write()?
                    .set_defined_name(&n, None, &refers)
                    .map_err(|e| err1004(format!("Method 'Add' of object 'Names' failed: {e}")))?;
                Ok(obj(Rc::new(NameObj {
                    s: self.s.clone(),
                    name: n,
                    scope: None,
                })))
            }
            _ => Err(ScriptError::no_member("Names", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        let names = self.s.read()?.defined_names();
        Ok(names
            .into_iter()
            .map(|d| {
                obj(Rc::new(NameObj {
                    s: self.s.clone(),
                    name: d.name,
                    scope: d.scope,
                }))
            })
            .collect())
    }
}

pub struct NameObj {
    s: Rc<Session>,
    name: String,
    scope: Option<u32>,
}

impl NameObj {
    fn formula(&self) -> Result<String, ScriptError> {
        let names = self.s.read()?.defined_names();
        names
            .into_iter()
            .find(|d| d.scope == self.scope && d.name.eq_ignore_ascii_case(&self.name))
            .map(|d| d.formula)
            .ok_or_else(|| err1004("The name was deleted"))
    }
}

impl Dispatch for NameObj {
    fn type_name(&self) -> String {
        "Name".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "name" | "namelocal" => Ok(Value::String(self.name.clone())),
            "" | "refersto" | "value" | "referstolocal" => {
                Ok(Value::String(format!("={}", self.formula()?)))
            }
            "referstorange" => {
                let f = self.formula()?;
                let sheet = self.s.active.get();
                Ok(obj(resolve(&self.s, sheet, &f)?))
            }
            "visible" => Ok(Value::Boolean(true)),
            "delete" => {
                self.s
                    .write()?
                    .delete_defined_name(&self.name, self.scope)
                    .map_err(engine)?;
                Ok(Value::Empty)
            }
            _ => Err(ScriptError::no_member("Name", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "refersto" | "value" => {
                let f = match range_of(&value) {
                    Some(r) => range_ref(&self.s, &r)?,
                    None => to_str(&value)?,
                };
                self.s
                    .write()?
                    .set_defined_name(&self.name, self.scope, f.trim_start_matches('='))
                    .map_err(engine)
            }
            "visible" | "comment" => Ok(()),
            _ => Err(ScriptError::no_member("Name", name)),
        }
    }
}

/// `Application.InputBox(Prompt, Title, Default, Left, Top, HelpFile,
/// HelpContextID, Type)`: `False` when cancelled; `Type` 1 a number, 2 text
/// (the default), 4 a Boolean, 8 a `Range`, 0 a formula.
fn input_box(s: &Rc<Session>, args: &[Arg]) -> Result<Value, ScriptError> {
    use crate::run::{InputBoxRequest, InputKind};
    let prompt = arg_str(args, 0, "Prompt")?.unwrap_or_default();
    let title = arg_str(args, 1, "Title")?.unwrap_or_else(|| "Input".to_owned());
    let default = match arg(args, 2, "Default") {
        Some(v) => match range_of(v) {
            Some(r) => addr::area_text(r.first(), true, true),
            None => to_str(v)?,
        },
        None => String::new(),
    };
    let ty = arg_i64(args, 7, "Type")?.unwrap_or(2);
    let kind = match ty {
        0 => InputKind::Formula,
        1 => InputKind::Number,
        4 => InputKind::Boolean,
        8 => InputKind::Range,
        _ => InputKind::Text,
    };
    let Some(answer) = s.input_box(&InputBoxRequest {
        prompt,
        title,
        default,
        kind,
    })?
    else {
        return Ok(Value::Boolean(false));
    };
    let bad = |what: &str| err1004(format!("The value entered is not a valid {what}"));
    Ok(match kind {
        InputKind::Text => Value::String(answer),
        InputKind::Formula => Value::String(answer),
        InputKind::Number => {
            let t = answer.trim().trim_start_matches('=');
            Value::Double(
                kantor_script::vba::convert::parse_number(t).ok_or_else(|| bad("number"))?,
            )
        }
        InputKind::Boolean => match answer.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Value::Boolean(true),
            "false" | "0" | "no" => Value::Boolean(false),
            _ => return Err(bad("Boolean")),
        },
        InputKind::Range => {
            let t = answer.trim().trim_start_matches('=');
            obj(resolve(s, s.active.get(), t).map_err(|_| bad("reference"))?)
        }
    })
}
