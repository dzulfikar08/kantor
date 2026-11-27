//! A mock spreadsheet host: `Range`, `Cells`, `ActiveSheet`, `WorksheetFunction` over a
//! `HashMap` of cells, plus recorded `MsgBox`/`InputBox`/`Debug.Print`.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use kantor_script::vba::{ModuleKind, Project};
use kantor_script::{
    Arg, Array, CancelToken, Dispatch, Host, ObjectRef, ScriptError, Value, find_arg,
};

pub type Grid = Rc<RefCell<HashMap<(i32, i32), Value>>>;

fn col_number(letters: &str) -> i32 {
    letters.chars().fold(0, |n, c| {
        n * 26 + (c.to_ascii_uppercase() as i32 - 'A' as i32 + 1)
    })
}

fn col_letters(mut c: i32) -> String {
    let mut s = String::new();
    while c > 0 {
        let r = (c - 1) % 26;
        s.insert(0, (b'A' + r as u8) as char);
        c = (c - 1) / 26;
    }
    s
}

/// "B12" -> (12, 2).
pub fn parse_ref(s: &str) -> Option<(i32, i32)> {
    let s = s.replace('$', "");
    let split = s.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = s.split_at(split);
    if letters.is_empty() || !letters.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some((digits.parse().ok()?, col_number(letters)))
}

pub struct RangeObj {
    pub grid: Grid,
    pub r1: i32,
    pub c1: i32,
    pub r2: i32,
    pub c2: i32,
}

impl RangeObj {
    pub fn new(grid: &Grid, r1: i32, c1: i32, r2: i32, c2: i32) -> Rc<RangeObj> {
        Rc::new(RangeObj {
            grid: grid.clone(),
            r1,
            c1,
            r2,
            c2,
        })
    }

    pub fn parse(grid: &Grid, text: &str) -> Result<Rc<RangeObj>, ScriptError> {
        let bad = || ScriptError::new(1004, format!("Method 'Range' failed: {text}"));
        match text.split_once(':') {
            Some((a, b)) => {
                let (r1, c1) = parse_ref(a).ok_or_else(bad)?;
                let (r2, c2) = parse_ref(b).ok_or_else(bad)?;
                Ok(RangeObj::new(grid, r1, c1, r2, c2))
            }
            None => {
                let (r, c) = parse_ref(text).ok_or_else(bad)?;
                Ok(RangeObj::new(grid, r, c, r, c))
            }
        }
    }

    fn cell(&self, r: i32, c: i32) -> Value {
        self.grid
            .borrow()
            .get(&(r, c))
            .cloned()
            .unwrap_or(Value::Empty)
    }

    fn set(&self, r: i32, c: i32, v: Value) {
        if v.is_empty() {
            self.grid.borrow_mut().remove(&(r, c));
        } else {
            self.grid.borrow_mut().insert((r, c), v);
        }
    }

    fn value(&self) -> Value {
        if self.r1 == self.r2 && self.c1 == self.c2 {
            return self.cell(self.r1, self.c1);
        }
        let rows = (self.r1..=self.r2)
            .map(|r| (self.c1..=self.c2).map(|c| self.cell(r, c)).collect())
            .collect();
        Value::array(Array::from_rows(1, rows))
    }
}

fn int(v: &Value) -> Result<i32, ScriptError> {
    v.as_f64()
        .map(|f| f as i32)
        .ok_or_else(|| ScriptError::runtime(13))
}

impl Dispatch for RangeObj {
    fn type_name(&self) -> String {
        "Range".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        let obj = |r: Rc<RangeObj>| Ok(Value::object(r as ObjectRef));
        match lower.as_str() {
            "" | "cells" | "item" if !args.is_empty() => {
                let r = int(find_arg(args, 0, "RowIndex").unwrap_or(&Value::Long(1)))?;
                let c = int(find_arg(args, 1, "ColumnIndex").unwrap_or(&Value::Long(1)))?;
                let (r, c) = (self.r1 + r - 1, self.c1 + c - 1);
                obj(RangeObj::new(&self.grid, r, c, r, c))
            }
            "" | "value" | "value2" => Ok(self.value()),
            "text" => Ok(Value::String(kantor_script::vba::convert::display(
                &self.value(),
            ))),
            "row" => Ok(Value::Long(self.r1)),
            "column" => Ok(Value::Long(self.c1)),
            "count" => Ok(Value::Long(
                (self.r2 - self.r1 + 1) * (self.c2 - self.c1 + 1),
            )),
            "address" => Ok(Value::String(format!(
                "${}${}",
                col_letters(self.c1),
                self.r1
            ))),
            "cells" => obj(RangeObj::new(
                &self.grid, self.r1, self.c1, self.r2, self.c2,
            )),
            "offset" => {
                let dr = find_arg(args, 0, "RowOffset").map_or(Ok(0), int)?;
                let dc = find_arg(args, 1, "ColumnOffset").map_or(Ok(0), int)?;
                obj(RangeObj::new(
                    &self.grid,
                    self.r1 + dr,
                    self.c1 + dc,
                    self.r2 + dr,
                    self.c2 + dc,
                ))
            }
            "resize" => {
                let rows = find_arg(args, 0, "RowSize").map_or(Ok(self.r2 - self.r1 + 1), int)?;
                let cols =
                    find_arg(args, 1, "ColumnSize").map_or(Ok(self.c2 - self.c1 + 1), int)?;
                obj(RangeObj::new(
                    &self.grid,
                    self.r1,
                    self.c1,
                    self.r1 + rows - 1,
                    self.c1 + cols - 1,
                ))
            }
            "clearcontents" => {
                for r in self.r1..=self.r2 {
                    for c in self.c1..=self.c2 {
                        self.set(r, c, Value::Empty);
                    }
                }
                Ok(Value::Empty)
            }
            "copy" => {
                let dest =
                    find_arg(args, 0, "Destination").ok_or_else(|| ScriptError::runtime(449))?;
                let Some(o) = dest.as_object() else {
                    return Err(ScriptError::runtime(424));
                };
                let (r0, c0) = match o.get("Row", &[])? {
                    Value::Long(r) => (r, int(&o.get("Column", &[])?)?),
                    _ => return Err(ScriptError::runtime(13)),
                };
                for r in self.r1..=self.r2 {
                    for c in self.c1..=self.c2 {
                        let v = self.cell(r, c);
                        self.set(r0 + r - self.r1, c0 + c - self.c1, v);
                    }
                }
                Ok(Value::Empty)
            }
            _ => Err(ScriptError::no_member("Range", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "" | "value" | "value2" => {
                match value {
                    Value::Array(a) => {
                        for (i, row) in a.rows().into_iter().enumerate() {
                            for (j, v) in row.into_iter().enumerate() {
                                self.set(self.r1 + i as i32, self.c1 + j as i32, v);
                            }
                        }
                    }
                    v => {
                        for r in self.r1..=self.r2 {
                            for c in self.c1..=self.c2 {
                                self.set(r, c, v.clone());
                            }
                        }
                    }
                }
                Ok(())
            }
            _ => Err(ScriptError::no_member("Range", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        let mut out = Vec::new();
        for r in self.r1..=self.r2 {
            for c in self.c1..=self.c2 {
                out.push(Value::object(
                    RangeObj::new(&self.grid, r, c, r, c) as ObjectRef
                ));
            }
        }
        Ok(out)
    }
}

pub struct Sheet {
    pub grid: Grid,
    pub name: RefCell<String>,
}

impl Dispatch for Sheet {
    fn type_name(&self) -> String {
        "Worksheet".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "name" => Ok(Value::String(self.name.borrow().clone())),
            "range" | "cells" => sheet_member(&self.grid, name, args),
            _ => Err(ScriptError::no_member("Worksheet", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "name" => {
                *self.name.borrow_mut() = value.as_str().unwrap_or_default().to_string();
                Ok(())
            }
            _ => Err(ScriptError::no_member("Worksheet", name)),
        }
    }
}

pub struct WorksheetFunction;

impl Dispatch for WorksheetFunction {
    fn type_name(&self) -> String {
        "WorksheetFunction".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        let mut nums = Vec::new();
        for a in args {
            let vals = match &a.value {
                Value::Object(Some(o)) => match o.get("Value", &[])? {
                    Value::Array(arr) => arr.values().to_vec(),
                    v => vec![v],
                },
                Value::Array(arr) => arr.values().to_vec(),
                v => vec![v.clone()],
            };
            nums.extend(
                vals.iter()
                    .filter_map(|v| if v.is_number() { v.as_f64() } else { None }),
            );
        }
        match name.to_ascii_lowercase().as_str() {
            "sum" => Ok(Value::Double(nums.iter().sum())),
            "max" => Ok(Value::Double(nums.iter().cloned().fold(f64::MIN, f64::max))),
            _ => Err(ScriptError::no_member("WorksheetFunction", name)),
        }
    }
}

fn sheet_member(grid: &Grid, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
    match name.to_ascii_lowercase().as_str() {
        "range" => {
            let first = find_arg(args, 0, "Cell1").ok_or_else(|| ScriptError::runtime(449))?;
            match first {
                Value::String(s) => Ok(Value::object(RangeObj::parse(grid, s)? as ObjectRef)),
                _ => Err(ScriptError::runtime(13)),
            }
        }
        _ => {
            if args.is_empty() {
                return Ok(Value::object(
                    RangeObj::new(grid, 1, 1, 1_048_576, 16_384) as ObjectRef
                ));
            }
            let r = int(find_arg(args, 0, "RowIndex").ok_or_else(|| ScriptError::runtime(449))?)?;
            let c = match find_arg(args, 1, "ColumnIndex") {
                Some(Value::String(s)) => col_number(s),
                Some(v) => int(v)?,
                None => 1,
            };
            Ok(Value::object(RangeObj::new(grid, r, c, r, c) as ObjectRef))
        }
    }
}

/// Excel's `Application`: the unqualified globals.
pub struct App {
    pub grid: Grid,
    pub sheet: Rc<Sheet>,
    pub screen_updating: Rc<RefCell<bool>>,
}

impl App {
    /// A `Range` object for an address, as a host would pass one to a UDF.
    pub fn get_range(&self, addr: &str) -> Value {
        Value::object(RangeObj::parse(&self.grid, addr).expect("valid range") as ObjectRef)
    }
}

impl Dispatch for App {
    fn type_name(&self) -> String {
        "Application".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "range" | "cells" => sheet_member(&self.grid, name, args),
            "activesheet" => Ok(Value::object(self.sheet.clone() as ObjectRef)),
            "application" => Ok(Value::object(Rc::new(App {
                grid: self.grid.clone(),
                sheet: self.sheet.clone(),
                screen_updating: self.screen_updating.clone(),
            }) as ObjectRef)),
            "worksheetfunction" => Ok(Value::object(Rc::new(WorksheetFunction) as ObjectRef)),
            "screenupdating" => Ok(Value::Boolean(*self.screen_updating.borrow())),
            "evaluate" => match find_arg(args, 0, "Name") {
                Some(Value::String(s)) => {
                    Ok(Value::object(RangeObj::parse(&self.grid, s)? as ObjectRef))
                }
                _ => Err(ScriptError::runtime(13)),
            },
            _ => Err(ScriptError::no_member("Application", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "screenupdating" => {
                *self.screen_updating.borrow_mut() = matches!(value, Value::Boolean(true));
                Ok(())
            }
            _ => Err(ScriptError::no_member("Application", name)),
        }
    }
}

pub struct MockHost {
    pub grid: Grid,
    pub app: Rc<App>,
    pub printed: Vec<String>,
    pub prompts: Vec<String>,
    pub answers: VecDeque<i32>,
    pub inputs: VecDeque<Option<String>>,
    pub cancel: CancelToken,
    pub now: f64,
}

impl MockHost {
    pub fn new() -> MockHost {
        let grid: Grid = Rc::new(RefCell::new(HashMap::new()));
        let sheet = Rc::new(Sheet {
            grid: grid.clone(),
            name: RefCell::new("Sheet1".into()),
        });
        MockHost {
            app: Rc::new(App {
                grid: grid.clone(),
                sheet,
                screen_updating: Rc::new(RefCell::new(true)),
            }),
            grid,
            printed: Vec::new(),
            prompts: Vec::new(),
            answers: VecDeque::new(),
            inputs: VecDeque::new(),
            cancel: CancelToken::new(),
            // 2024-03-15 14:30:00
            now: kantor_script::date::day_number(2024, 3, 15) as f64 + 14.5 / 24.0,
        }
    }

    pub fn cell(&self, addr: &str) -> Value {
        let (r, c) = parse_ref(addr).expect("valid address");
        self.grid
            .borrow()
            .get(&(r, c))
            .cloned()
            .unwrap_or(Value::Empty)
    }

    pub fn set(&self, addr: &str, v: impl Into<Value>) {
        let (r, c) = parse_ref(addr).expect("valid address");
        self.grid.borrow_mut().insert((r, c), v.into());
    }
}

impl Host for MockHost {
    fn globals(&mut self) -> Option<ObjectRef> {
        Some(self.app.clone())
    }

    fn constant(&mut self, name: &str) -> Option<Value> {
        match name.to_ascii_lowercase().as_str() {
            "xlup" => Some(Value::Long(-4162)),
            "xltoleft" => Some(Value::Long(-4159)),
            _ => None,
        }
    }

    fn msg_box(&mut self, prompt: &str, _buttons: i32, _title: &str) -> Result<i32, ScriptError> {
        self.prompts.push(prompt.to_string());
        Ok(self.answers.pop_front().unwrap_or(1))
    }

    fn input_box(
        &mut self,
        prompt: &str,
        _title: &str,
        default: &str,
    ) -> Result<Option<String>, ScriptError> {
        self.prompts.push(prompt.to_string());
        Ok(self.inputs.pop_front().unwrap_or(Some(default.to_string())))
    }

    fn debug_print(&mut self, line: &str) {
        self.printed.push(line.to_string());
    }

    fn now(&mut self) -> f64 {
        self.now
    }

    fn cancelled(&mut self) -> bool {
        self.cancel.is_cancelled()
    }
}

/// A project with one standard module.
pub fn project(src: &str) -> Project {
    let mut p = Project::new();
    if let Err(e) = p.add_module("Module1", ModuleKind::Standard, src) {
        panic!("module failed to load: {e}");
    }
    p
}

/// Runs `proc` in a one-module project with a fresh mock host.
pub fn run(src: &str, proc: &str) -> (Value, MockHost) {
    let mut p = project(src);
    let mut host = MockHost::new();
    let v = match p.run(proc, &[], &mut host) {
        Ok(v) => v,
        Err(e) => panic!("{proc} failed: {e}"),
    };
    (v, host)
}

/// Runs a function that returns a string.
pub fn run_str(src: &str, proc: &str) -> String {
    match run(src, proc).0 {
        Value::String(s) => s,
        other => panic!("{proc} returned {other:?}"),
    }
}
