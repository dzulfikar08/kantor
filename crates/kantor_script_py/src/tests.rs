//! The Python bridge against a small object model: one workbook of sheets with cells.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use kantor_script::{
    Arg, Array, CancelToken, Dispatch, ErrorKind, Host, ObjectRef, ScriptError, Value, find_arg,
};

use crate::{Module, Options, eval, run};

#[derive(Default)]
struct Sheet {
    name: String,
    cells: RefCell<HashMap<(i32, i32), Value>>,
}

struct SheetObj(Rc<Sheet>);

struct RangeObj {
    sheet: Rc<Sheet>,
    row: i32,
    col: i32,
}

struct Sheets(Vec<Rc<Sheet>>);

struct Globals {
    sheets: Vec<Rc<Sheet>>,
    /// The arguments `Record` was called with, as `name=value` texts.
    recorded: Rc<RefCell<Vec<String>>>,
}

fn obj(o: impl Dispatch + 'static) -> Value {
    Value::object(Rc::new(o))
}

fn num(v: &Value) -> Option<i32> {
    v.as_f64().map(|f| f as i32)
}

/// `A1`-style address of one cell.
fn parse(a: &str) -> Option<(i32, i32)> {
    let a = a.trim();
    let letters: String = a.chars().take_while(char::is_ascii_alphabetic).collect();
    let col = letters
        .to_ascii_uppercase()
        .bytes()
        .fold(0i32, |n, b| n * 26 + i32::from(b - b'A' + 1));
    let row: i32 = a[letters.len()..].parse().ok()?;
    (col > 0).then_some((row, col))
}

impl Dispatch for RangeObj {
    fn type_name(&self) -> String {
        "Range".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "" | "item" if !args.is_empty() => {
                let r = find_arg(args, 0, "RowIndex").and_then(num).unwrap_or(1);
                let c = find_arg(args, 1, "ColumnIndex").and_then(num).unwrap_or(1);
                Ok(obj(RangeObj {
                    sheet: self.sheet.clone(),
                    row: self.row + r - 1,
                    col: self.col + c - 1,
                }))
            }
            "" | "value" => Ok(self
                .sheet
                .cells
                .borrow()
                .get(&(self.row, self.col))
                .cloned()
                .unwrap_or_default()),
            "row" => Ok(Value::Long(self.row)),
            "column" => Ok(Value::Long(self.col)),
            "worksheet" | "parent" => Ok(obj(SheetObj(self.sheet.clone()))),
            "offset" => {
                let r = find_arg(args, 0, "RowOffset").and_then(num).unwrap_or(0);
                let c = find_arg(args, 1, "ColumnOffset").and_then(num).unwrap_or(0);
                Ok(obj(RangeObj {
                    sheet: self.sheet.clone(),
                    row: self.row + r,
                    col: self.col + c,
                }))
            }
            "clear" => {
                self.sheet.cells.borrow_mut().remove(&(self.row, self.col));
                Ok(Value::Boolean(true))
            }
            "fail" => Err(ScriptError::new(
                1004,
                "Application-defined or object-defined error",
            )),
            _ => Err(ScriptError::no_member("Range", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "" | "value" => {
                self.sheet
                    .cells
                    .borrow_mut()
                    .insert((self.row, self.col), value);
                Ok(())
            }
            _ => Err(ScriptError::no_member("Range", name)),
        }
    }
}

impl Dispatch for SheetObj {
    fn type_name(&self) -> String {
        "Worksheet".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "name" => Ok(Value::from(self.0.name.as_str())),
            "range" => {
                let a = find_arg(args, 0, "Cell1").ok_or_else(|| ScriptError::runtime(449))?;
                let (row, col) = a
                    .as_str()
                    .and_then(parse)
                    .ok_or_else(|| ScriptError::new(1004, "Method 'Range' failed"))?;
                Ok(obj(RangeObj {
                    sheet: self.0.clone(),
                    row,
                    col,
                }))
            }
            "cells" => {
                let base = RangeObj {
                    sheet: self.0.clone(),
                    row: 1,
                    col: 1,
                };
                if args.is_empty() {
                    Ok(obj(base))
                } else {
                    base.get("", args)
                }
            }
            _ => Err(ScriptError::no_member("Worksheet", name)),
        }
    }
}

impl Dispatch for Sheets {
    fn type_name(&self) -> String {
        "Sheets".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "count" => Ok(Value::Long(self.0.len() as i32)),
            "" | "item" => {
                let a = find_arg(args, 0, "Index").ok_or_else(|| ScriptError::runtime(449))?;
                let found = match a {
                    Value::String(s) => self.0.iter().find(|x| x.name.eq_ignore_ascii_case(s)),
                    v => num(v).and_then(|i| self.0.get((i - 1) as usize)),
                };
                found
                    .map(|s| obj(SheetObj(s.clone())))
                    .ok_or_else(|| ScriptError::runtime(9))
            }
            _ => Err(ScriptError::no_member("Sheets", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        Ok(self.0.iter().map(|s| obj(SheetObj(s.clone()))).collect())
    }
}

impl Dispatch for Globals {
    fn type_name(&self) -> String {
        "Global".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        let active = SheetObj(self.sheets[0].clone());
        match name.to_ascii_lowercase().as_str() {
            "range" | "cells" => active.get(name, args),
            "activesheet" => Ok(obj(active)),
            "application" => Ok(obj(Globals {
                sheets: self.sheets.clone(),
                recorded: self.recorded.clone(),
            })),
            "worksheets" | "sheets" if args.is_empty() => Ok(obj(Sheets(self.sheets.clone()))),
            "worksheets" | "sheets" => Sheets(self.sheets.clone()).get("", args),
            "record" if args.is_empty() => Err(ScriptError::runtime(449)),
            "record" => {
                let mut r = self.recorded.borrow_mut();
                for a in args {
                    r.push(format!(
                        "{}={:?}",
                        a.name.as_deref().unwrap_or("_"),
                        a.value
                    ));
                }
                Ok(Value::Long(args.len() as i32))
            }
            _ => Err(ScriptError::no_member("Global", name)),
        }
    }
}

#[derive(Default)]
struct TestHost {
    globals: Option<ObjectRef>,
    printed: Vec<String>,
    boxes: Vec<(String, i32, String)>,
    polls: u32,
    /// `cancelled()` says yes after this many polls.
    stop_after: Option<u32>,
}

impl Host for TestHost {
    fn globals(&mut self) -> Option<ObjectRef> {
        self.globals.clone()
    }

    fn constant(&mut self, name: &str) -> Option<Value> {
        name.eq_ignore_ascii_case("xlUp")
            .then_some(Value::Long(-4162))
    }

    fn msg_box(&mut self, prompt: &str, buttons: i32, title: &str) -> Result<i32, ScriptError> {
        self.boxes.push((prompt.into(), buttons, title.into()));
        Ok(if buttons & 7 == 4 { 6 } else { 1 })
    }

    fn input_box(
        &mut self,
        _prompt: &str,
        _title: &str,
        _default: &str,
    ) -> Result<Option<String>, ScriptError> {
        Ok(Some("42".into()))
    }

    fn debug_print(&mut self, line: &str) {
        self.printed.push(line.into());
    }

    fn is_method(&mut self, _type_name: &str, member: &str) -> bool {
        matches!(member.to_ascii_lowercase().as_str(), "clear" | "offset")
    }

    fn cancelled(&mut self) -> bool {
        self.polls += 1;
        self.stop_after.is_some_and(|n| self.polls > n)
    }
}

struct World {
    sheets: Vec<Rc<Sheet>>,
    recorded: Rc<RefCell<Vec<String>>>,
    host: Rc<RefCell<TestHost>>,
}

fn world() -> World {
    let sheets: Vec<Rc<Sheet>> = ["Data", "Report"]
        .iter()
        .map(|n| {
            Rc::new(Sheet {
                name: (*n).into(),
                ..Default::default()
            })
        })
        .collect();
    let recorded = Rc::new(RefCell::new(Vec::new()));
    let host = TestHost {
        globals: Some(Rc::new(Globals {
            sheets: sheets.clone(),
            recorded: recorded.clone(),
        })),
        ..Default::default()
    };
    World {
        sheets,
        recorded,
        host: Rc::new(RefCell::new(host)),
    }
}

impl World {
    fn cell(&self, row: i32, col: i32) -> Value {
        self.sheets[0]
            .cells
            .borrow()
            .get(&(row, col))
            .cloned()
            .unwrap_or_default()
    }

    fn run(&self, source: &str, target: &str) -> Result<Value, ScriptError> {
        run(
            &[Module {
                name: "Main".into(),
                source: source.into(),
            }],
            target,
            &[],
            self.host.clone(),
            &Options::default(),
        )
    }

    fn eval(&self, source: &str) -> Value {
        eval("Main", source, self.host.clone(), &Options::default())
            .unwrap_or_else(|e| panic!("{source}: {e}"))
    }
}

#[test]
fn values_convert_both_ways() {
    let w = world();
    assert_eq!(w.eval("1 + 1"), Value::Long(2));
    assert_eq!(w.eval("1.5"), Value::Double(1.5));
    assert_eq!(w.eval("2 ** 40"), Value::LongLong(1 << 40));
    assert_eq!(w.eval("'é' + 'x'"), Value::from("éx"));
    assert_eq!(w.eval("3 > 2"), Value::Boolean(true));
    assert_eq!(w.eval("None"), Value::Empty);
    assert_eq!(
        w.eval("[[1, 'a'], (True, None)]"),
        Value::array(Array::from_rows(
            0,
            vec![
                vec![Value::Long(1), Value::from("a")],
                vec![Value::Boolean(true), Value::Empty]
            ]
        ))
    );
    assert_eq!(
        w.eval("[1, 2.5]"),
        Value::array(Array::from_vec(0, vec![Value::Long(1), Value::Double(2.5)]))
    );
    assert_eq!(w.eval("Date.of(2024, 1, 15, 18)"), Value::Date(45306.75));
    assert_eq!(w.eval("CVErr(2042)"), Value::Error(2042));
    assert_eq!(w.eval("str(CVErr(2007))"), Value::from("#DIV/0!"));

    // Into Python and back.
    let source = "def ident(x):\n    return x\n\ndef kind(x):\n    return type(x).__name__ + ':' + (str(len(x)) + ',' + str(len(x[0])) if isinstance(x, list) else repr(x))\n\ndef parts(d):\n    return d.year * 10000 + d.month * 100 + d.day + d.hour / 100\n";
    let m = [Module {
        name: "Main".into(),
        source: source.into(),
    }];
    let call = |f: &str, v: Value| run(&m, f, &[v], w.host.clone(), &Options::default());
    for v in [
        Value::Date(45306.75),
        Value::Error(2042),
        Value::from("x"),
        Value::Double(2.25),
        Value::Boolean(false),
        Value::Empty,
    ] {
        assert_eq!(call("ident", v.clone()).unwrap(), v);
    }
    assert_eq!(
        call("parts", Value::Date(45306.75)).unwrap(),
        Value::Double(20240115.18)
    );
    let grid = Value::array(Array::from_rows(
        1,
        vec![
            vec![Value::Long(1), Value::Long(2), Value::Long(3)],
            vec![Value::Long(4), Value::Long(5), Value::Long(6)],
        ],
    ));
    assert_eq!(call("kind", grid).unwrap(), Value::from("list:2,3"));
}

#[test]
fn objects_properties_methods_and_default_members() {
    let w = world();
    let src = r#"
def go():
    Range("A1").Value = 5
    ActiveSheet.range("B1").value = Range("A1").Value * 2
    Range("C1").VALUE = int(Range("A1")) + 1
    Cells(2, 1).Value = ActiveSheet.Name
    r = Range("A1").Offset(2, 1)
    r.value = str(r.row) + ":" + str(r.column)
    Range("D1").Value = "x"
    Range("D1").Clear()
    Worksheets("Report").Range("A1").Value = Worksheets.Count
    active_sheet.cells[3, 4].value = len(Worksheets)
    Range("F1").Value = TypeName(ActiveSheet) + "/" + TypeName(1.5)
    return Range("B1").Value
"#;
    assert_eq!(w.run(src, "go").unwrap(), Value::Long(10));
    assert_eq!(w.cell(1, 1), Value::Long(5));
    assert_eq!(w.cell(1, 3), Value::Long(6));
    assert_eq!(w.cell(2, 1), Value::from("Data"));
    assert_eq!(w.cell(3, 2), Value::from("3:2"));
    assert_eq!(w.cell(1, 4), Value::Empty);
    assert_eq!(w.cell(3, 4), Value::Long(2));
    assert_eq!(w.cell(1, 6), Value::from("Worksheet/Double"));
    assert_eq!(
        w.sheets[1].cells.borrow().get(&(1, 1)),
        Some(&Value::Long(2))
    );
}

#[test]
fn collections_iterate_and_keyword_arguments_pass() {
    let w = world();
    let src = r#"
def names():
    return ",".join(ws.Name for ws in Worksheets)

def named():
    return Application.Record(1, None, destination=Range("A1"), skip=True)
"#;
    assert_eq!(w.run(src, "names").unwrap(), Value::from("Data,Report"));
    assert_eq!(w.run(src, "named").unwrap(), Value::Long(4));
    assert_eq!(
        *w.recorded.borrow(),
        vec![
            "_=Long(1)".to_owned(),
            "_=Missing".to_owned(),
            "destination=Object(Range)".to_owned(),
            "skip=Boolean(true)".to_owned()
        ]
    );
}

#[test]
fn errors_carry_numbers_and_places() {
    let w = world();
    let src = "def go():\n    r = Range(\"A1\")\n    return r.nope\n\ndef caught():\n    try:\n        Range(\"A1\").fail()\n    except KantorError as e:\n        return str(e.number) + ' ' + e.description\n\ndef typo():\n    undefined_thing()\n";
    let e = w.run(src, "go").unwrap_err();
    assert_eq!(e.number, 438, "{e}");
    assert_eq!(e.kind, ErrorKind::Runtime);
    let loc = e.location.expect("a location");
    assert_eq!(
        (loc.module.as_str(), loc.procedure.as_str(), loc.line),
        ("Main", "go", 3)
    );
    assert_eq!(
        w.run(src, "caught").unwrap(),
        Value::from("1004 Application-defined or object-defined error")
    );
    let e = w.run(src, "typo").unwrap_err();
    assert!(e.description.starts_with("NameError"), "{e}");
    let e = w.run("def go(:\n    pass\n", "go").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Syntax, "{e}");
    let e = w.run("def go():\n    pass\n", "missing").unwrap_err();
    assert_eq!(e.number, 35);
}

#[test]
fn dialogs_output_constants_and_no_files() {
    let w = world();
    let src = r#"
def go():
    a = MsgBox("Go on?", vbYesNo + vbQuestion, "Ask")
    b = InputBox("Number?")
    print("answer", a, b, [1, 2], ActiveSheet)
    print(xlUp)
    return a

def files():
    try:
        open("x.txt", "w")
    except Exception as e:
        return type(e).__name__
    return "opened"

def modules():
    out = []
    for name in ("os", "socket", "subprocess"):
        try:
            __import__(name)
            out.append(name)
        except ImportError:
            pass
    return ",".join(out)
"#;
    assert_eq!(w.run(src, "go").unwrap(), Value::Long(6));
    assert_eq!(
        w.host.borrow().boxes,
        vec![("Go on?".into(), 36, "Ask".into())]
    );
    assert_eq!(
        w.host.borrow().printed,
        vec![
            "answer 6 42 [1, 2] <Worksheet>".to_owned(),
            "-4162".to_owned()
        ]
    );
    assert_ne!(w.run(src, "files").unwrap(), Value::from("opened"));
    assert_eq!(w.run(src, "modules").unwrap(), Value::from(""));
}

#[test]
fn runs_stop_when_cancelled() {
    let w = world();
    let cancel = CancelToken::new();
    let stopper = cancel.clone();
    let t = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(200));
        stopper.cancel();
    });
    let options = Options {
        cancel,
        ..Options::default()
    };
    let e = run(
        &[Module {
            name: "Main".into(),
            source: "def go():\n    n = 0\n    while True:\n        n += 1\n".into(),
        }],
        "go",
        &[],
        w.host.clone(),
        &options,
    )
    .unwrap_err();
    t.join().unwrap();
    assert_eq!((e.number, e.kind), (18, ErrorKind::Interrupted));

    // The host says stop: `except Exception` doesn't keep it going.
    let w = world();
    w.host.borrow_mut().stop_after = Some(50);
    let e = w
        .run(
            "def go():\n    while True:\n        try:\n            Range('A1').Value = 1\n        except Exception:\n            pass\n",
            "go",
        )
        .unwrap_err();
    assert_eq!((e.number, e.kind), (18, ErrorKind::Interrupted));
}

#[test]
fn modules_import_each_other() {
    let w = world();
    let modules = [
        Module {
            name: "Tools".into(),
            source: "def double(x):\n    return x * 2\n\ndef go():\n    return 1\n".into(),
        },
        Module {
            name: "Main".into(),
            source: "import Tools\n\ndef go():\n    return Tools.double(21)\n".into(),
        },
    ];
    let call = |t: &str| run(&modules, t, &[], w.host.clone(), &Options::default());
    assert_eq!(call("Main.go").unwrap(), Value::Long(42));
    assert_eq!(call("Tools.go").unwrap(), Value::Long(1));
    assert_eq!(call("Nope.go").unwrap_err().number, 35);
}
