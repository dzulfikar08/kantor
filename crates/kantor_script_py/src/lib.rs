//! Python macros: RustPython bound to `kantor_script`'s object layer.
//!
//! [`run`] runs a workbook's Python modules in a fresh interpreter and calls one function. The
//! host's objects ([`kantor_script::Dispatch`]) appear as Python objects:
//!
//! - Naming a member reads a property (`r.Value`, `ws.Name`) or gives a method to call
//!   (`r.Clear()`, `r.Offset(1, 0)`, `ws.Range("A1")`), as [`kantor_script::binding::member`]
//!   decides. Member names are case-insensitive, as in VBA, and underscores are dropped, so
//!   `r.number_format` is `NumberFormat`: VBA's spelling and snake_case both work.
//! - Assigning sets a property: `r.Value = 5`, `r.Font.Bold = True`.
//! - Calling an object, or indexing it, takes its default member: `Worksheets("Data")`,
//!   `ws.Cells(2, 1)`, `ws.Cells[2, 1]`.
//! - `for ws in ActiveWorkbook.Worksheets:` enumerates a collection; `len()` is its `Count`.
//! - Named arguments are keyword arguments: `r.Copy(Destination=ws.Range("D1"))`.
//! - Values: `int`, `float`, `str`, `bool`; an empty cell is `None`; dates are [`Date`]s (a
//!   `float` of Excel's serial with `.year`, `.month`…); Excel errors are `CellError`s; a 2-D
//!   `Range.Value` is a list of rows. A failing member raises `KantorError` with VBA's
//!   `number`.
//! - Globals (looked up each time they are used): `Application`, `ActiveWorkbook`,
//!   `ActiveSheet`, `ActiveCell`, `Selection`, `ThisWorkbook`, `Workbooks`, `Worksheets`,
//!   `Sheets`, `Range`, `Cells`, `Rows`, `Columns`, `WorksheetFunction` (and snake_case
//!   spellings, except `range`, which stays Python's), the host's constants (`xlUp`), `MsgBox`,
//!   `InputBox`, `CVErr`, `TypeName`, and `print` for the Immediate output.
//!
//! The interpreter has no standard library on disk and none of the modules that reach the
//! file system, network or processes (RustPython's `host_env` feature is off: no `os`,
//! `socket`, `subprocess`, `open`): the built-in types and functions and a few built-in
//! modules (`time`, `itertools`, `functools`'s core…). Each module is importable by its name
//! from the others (`import Tools`). [`Options::cancel`] stops a run. Each run starts afresh.
//!
//! [`Date`]: https://docs.python.org/3/library/functions.html#float

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use kantor_script::binding::{self, Member};
use kantor_script::{Arg, Array, CancelToken, Host, Location, ObjectRef, Record, ScriptError, Value};
use rustpython_vm::builtins::{PyBaseExceptionRef, PyDict, PyDictRef, PyFloat, PyType, PyTypeRef};
use rustpython_vm::scope::Scope;
use rustpython_vm::{AsObject, Interpreter, PyObjectRef, PyResult, Settings, VirtualMachine};

pub use kantor_script::binding::python_functions as functions;

const PRELUDE: &str = include_str!("prelude.py");

/// One module of Python: `name` is its module name (`import Report`) and what errors call it
/// (`Report.py`).
#[derive(Clone, Debug)]
pub struct Module {
    pub name: String,
    pub source: String,
}

/// How a run goes.
#[derive(Clone)]
pub struct Options {
    /// Stops the run (error 18) when set, even inside a loop that calls nothing.
    pub cancel: CancelToken,
    /// Python's recursion limit (`sys.setrecursionlimit`): the interpreter recurses on the
    /// native stack, so keep it within the thread's stack.
    pub recursion_limit: usize,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            cancel: CancelToken::new(),
            recursion_limit: 400,
        }
    }
}

/// The names [`run`] defines as globals, read from the host's globals object when used.
pub const GLOBAL_NAMES: &[&str] = &[
    "Application",
    "ActiveWorkbook",
    "ActiveSheet",
    "ActiveCell",
    "ActiveChart",
    "Selection",
    "ThisWorkbook",
    "Workbooks",
    "Worksheets",
    "Sheets",
    "Charts",
    "Range",
    "Cells",
    "Rows",
    "Columns",
    "WorksheetFunction",
    "Evaluate",
    "Intersect",
    "Union",
];

/// The prelude's classes Rust converts values with.
struct Classes {
    obj: PyTypeRef,
    live: PyTypeRef,
    date: PyTypeRef,
    cell_error: PyTypeRef,
    kantor_error: PyTypeRef,
    describe: PyObjectRef,
}

/// What the native functions share during a run.
struct Bridge {
    host: Rc<RefCell<dyn Host>>,
    cancel: CancelToken,
    /// Host objects Python holds, by handle.
    objects: RefCell<HashMap<i64, ObjectRef>>,
    next: Cell<i64>,
    classes: RefCell<Option<Rc<Classes>>>,
    /// Module file names (`Report.py`) and module names, for error locations.
    files: Vec<(String, String)>,
}

impl Bridge {
    fn classes(&self) -> Rc<Classes> {
        match self.classes.borrow().as_ref() {
            Some(c) => c.clone(),
            None => unreachable!("the prelude ran first"),
        }
    }

    fn object(&self, h: i64, vm: &VirtualMachine) -> PyResult<ObjectRef> {
        self.objects
            .borrow()
            .get(&h)
            .cloned()
            .ok_or_else(|| vm.new_runtime_error("the object is gone"))
    }

    fn handle(&self, o: ObjectRef) -> i64 {
        let h = self.next.get();
        self.next.set(h + 1);
        self.objects.borrow_mut().insert(h, o);
        h
    }

    /// `KeyboardInterrupt` when the host or the cancel token says stop.
    fn check(&self, vm: &VirtualMachine) -> PyResult<()> {
        let stop = self.cancel.is_cancelled()
            || self
                .host
                .try_borrow_mut()
                .map(|mut h| h.cancelled())
                .unwrap_or(false);
        if stop {
            self.cancel.cancel();
            return Err(interrupt(vm));
        }
        Ok(())
    }

    fn is_method(&self, obj: &ObjectRef, name: &str) -> bool {
        self.host
            .try_borrow_mut()
            .map(|mut h| h.is_method(&obj.type_name(), name))
            .unwrap_or(false)
    }

    /// `e` as a Python `KantorError`.
    fn error(&self, e: &ScriptError, vm: &VirtualMachine) -> PyBaseExceptionRef {
        let cls = self.classes().kantor_error.clone();
        let args = vec![
            vm.ctx.new_int(e.number).into(),
            vm.ctx.new_str(e.description.as_str()).into(),
            vm.ctx.new_str(e.source.as_str()).into(),
        ];
        match vm.invoke_exception(&cls, args) {
            Ok(x) => x,
            Err(x) => x,
        }
    }
}

fn interrupt(vm: &VirtualMachine) -> PyBaseExceptionRef {
    vm.new_exception_msg(
        vm.ctx.exceptions.keyboard_interrupt.to_owned(),
        kantor_script::object::standard_message(18).into(),
    )
}

/// Runs function `target` (`go`, or `Report.go`) of `modules` with `args`. Every module is
/// run first (in order; the target's module last), each in its own namespace and importable
/// by its name. A bare name is looked up in the module that defines it.
pub fn run(
    modules: &[Module],
    target: &str,
    args: &[Value],
    host: Rc<RefCell<dyn Host>>,
    options: &Options,
) -> Result<Value, ScriptError> {
    let (module, func) = binding::split_target(target);
    let home = match module {
        Some(m) => modules.iter().find(|x| x.name.eq_ignore_ascii_case(m)),
        None => modules
            .iter()
            .find(|x| functions(&x.source).iter().any(|f| f.name == func))
            .or_else(|| modules.last()),
    };
    let Some(home) = home else {
        return Err(ScriptError::new(
            35,
            match module {
                Some(m) => format!("There is no Python module {m}"),
                None => format!("Sub or Function not defined ({func})"),
            },
        ));
    };
    let mut ordered: Vec<&Module> = modules
        .iter()
        .filter(|m| !std::ptr::eq(*m, home))
        .collect();
    ordered.push(home);

    let settings = Settings {
        isolated: true,
        ignore_environment: true,
        install_signal_handlers: false,
        import_site: false,
        user_site_directory: false,
        ..Default::default()
    };
    let (tx, rx) = rustpython_vm::signal::user_signal_channel();
    let interp = Interpreter::with_init(settings, move |vm| vm.set_user_signal_channel(rx));

    // Stop: a watcher sends the interpreter an interrupt it raises between instructions
    // (again and again, until the run ends: another interpreter may take the signal).
    let done = Arc::new(AtomicBool::new(false));
    let watcher = {
        let done = done.clone();
        let cancel = options.cancel.clone();
        std::thread::Builder::new()
            .name("python-stop".into())
            .spawn(move || {
                while !done.load(Ordering::Relaxed) {
                    if cancel.is_cancelled() {
                        let _ = tx.send(Box::new(|vm: &VirtualMachine| -> PyResult<()> {
                            Err(interrupt(vm))
                        }));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            })
            .ok()
    };

    let bridge = Rc::new(Bridge {
        host,
        cancel: options.cancel.clone(),
        objects: RefCell::new(HashMap::new()),
        next: Cell::new(1),
        classes: RefCell::new(None),
        files: modules
            .iter()
            .map(|m| (format!("{}.py", m.name), m.name.clone()))
            .collect(),
    });
    let result = interp.enter(|vm| {
        run_in(
            vm,
            &bridge,
            &ordered,
            &home.name,
            func,
            args,
            options.recursion_limit,
        )
    });
    done.store(true, Ordering::Relaxed);
    if let Some(w) = watcher {
        let _ = w.join();
    }
    drop(interp);
    result
}

fn run_in(
    vm: &VirtualMachine,
    bridge: &Rc<Bridge>,
    ordered: &[&Module],
    home: &str,
    func: &str,
    args: &[Value],
    recursion_limit: usize,
) -> Result<Value, ScriptError> {
    let fail = |e: PyBaseExceptionRef| caught(vm, bridge, e);
    let exports = install(vm, bridge).map_err(fail)?;
    if let Ok(sys) = vm.import("sys", 0) {
        let _ = vm.call_method(&sys, "setrecursionlimit", (recursion_limit,));
    }
    let sys_modules = vm.sys_module.get_attr("modules", vm).map_err(fail)?;
    // Every module is importable before any runs (`import Tools` at the top of another).
    let mut namespaces = Vec::with_capacity(ordered.len());
    for m in ordered {
        let globals = vm.ctx.new_dict();
        for (k, v) in exports.items_vec() {
            globals.set_item(&*k, v, vm).map_err(fail)?;
        }
        globals
            .set_item("__name__", vm.ctx.new_str(m.name.as_str()).into(), vm)
            .map_err(fail)?;
        let module = vm.new_module(&m.name, globals.clone(), None);
        sys_modules
            .set_item(m.name.as_str(), module.into(), vm)
            .map_err(fail)?;
        namespaces.push(globals);
    }
    let mut home_globals = None;
    for (m, globals) in ordered.iter().zip(namespaces) {
        let scope = Scope::with_builtins(None, globals.clone(), vm);
        vm.run_string(scope, &m.source, &format!("{}.py", m.name))
            .map_err(fail)?;
        if m.name == home {
            home_globals = Some(globals);
        }
    }
    let Some(globals) = home_globals else {
        return Err(ScriptError::new(35, format!("There is no Python module {home}")));
    };
    let f = globals.get_item_opt(func, vm).map_err(fail)?;
    let Some(f) = f.filter(|f| f.is_callable()) else {
        return Err(ScriptError::new(
            35,
            format!("Sub or Function not defined ({func})"),
        ));
    };
    let mut call = Vec::with_capacity(args.len());
    for a in args {
        call.push(to_py(vm, bridge, a).map_err(fail)?);
    }
    let r = f.call(call, vm).map_err(fail)?;
    from_py(vm, bridge, &r, Value::Empty).map_err(fail)
}

/// Evaluates `source` (an expression, or statements) as module `name` and returns its value
/// (for tests).
pub fn eval(
    name: &str,
    source: &str,
    host: Rc<RefCell<dyn Host>>,
    options: &Options,
) -> Result<Value, ScriptError> {
    let wrapped = format!(
        "def __kantor_eval():\n    return eval({})\n",
        py_string(source)
    );
    run(
        &[Module {
            name: name.to_owned(),
            source: wrapped,
        }],
        "__kantor_eval",
        &[],
        host,
        options,
    )
}

/// `s` as a Python string literal.
fn py_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// The native module (`_k`), the prelude run with it, and the prelude's exports.
fn install(vm: &VirtualMachine, bridge: &Rc<Bridge>) -> PyResult<PyDictRef> {
    let k = vm.ctx.new_dict();
    let set = |name: &str, f: PyObjectRef| k.set_item(name, f, vm);

    let b = bridge.clone();
    set(
        "member",
        vm.new_function(
            "member",
            move |h: i64, name: String, method: PyObjectRef, vm: &VirtualMachine| -> PyResult {
                b.check(vm)?;
                let obj = b.object(h, vm)?;
                let is_method = b.is_method(&obj, &name);
                match binding::member(&obj, &name, is_method) {
                    Ok(Member::Method) => Ok(method),
                    Ok(Member::Value(v)) => to_py(vm, &b, &v),
                    Err(e) => Err(b.error(&e, vm)),
                }
            },
        )
        .into(),
    )?;

    for (key, as_call) in [("get", false), ("call", true)] {
        let b = bridge.clone();
        let name: &'static str = key;
        set(
            key,
            vm.new_function(
                name,
                move |h: i64,
                      name: String,
                      positional: Vec<PyObjectRef>,
                      named: Vec<PyObjectRef>,
                      vm: &VirtualMachine|
                      -> PyResult {
                    b.check(vm)?;
                    let obj = b.object(h, vm)?;
                    let args = arguments(vm, &b, &positional, &named)?;
                    let r = if as_call {
                        obj.call(&name, &args)
                    } else {
                        obj.get(&name, &args)
                    };
                    match r {
                        Ok(v) => to_py(vm, &b, &v),
                        Err(e) => Err(b.error(&e, vm)),
                    }
                },
            )
            .into(),
        )?;
    }

    let b = bridge.clone();
    set(
        "put",
        vm.new_function(
            "put",
            move |h: i64,
                  name: String,
                  value: PyObjectRef,
                  index: Vec<PyObjectRef>,
                  vm: &VirtualMachine|
                  -> PyResult<()> {
                b.check(vm)?;
                let obj = b.object(h, vm)?;
                let value = from_py(vm, &b, &value, Value::Empty)?;
                let args = arguments(vm, &b, &index, &[])?;
                let r = if value.is_object() {
                    obj.put_ref(&name, &args, value)
                } else {
                    obj.put(&name, &args, value)
                };
                r.map_err(|e| b.error(&e, vm))
            },
        )
        .into(),
    )?;

    let b = bridge.clone();
    set(
        "items",
        vm.new_function(
            "items",
            move |h: i64, vm: &VirtualMachine| -> PyResult {
                b.check(vm)?;
                let obj = b.object(h, vm)?;
                let items = obj.enumerate().map_err(|e| b.error(&e, vm))?;
                let mut out = Vec::with_capacity(items.len());
                for v in &items {
                    out.push(to_py(vm, &b, v)?);
                }
                Ok(vm.ctx.new_list(out).into())
            },
        )
        .into(),
    )?;

    let b = bridge.clone();
    set(
        "release",
        vm.new_function("release", move |h: i64, _vm: &VirtualMachine| {
            b.objects.borrow_mut().remove(&h);
        })
        .into(),
    )?;

    let b = bridge.clone();
    set(
        "type_name",
        vm.new_function(
            "type_name",
            move |h: i64, vm: &VirtualMachine| -> PyResult<String> {
                Ok(b.object(h, vm)?.type_name())
            },
        )
        .into(),
    )?;

    let b = bridge.clone();
    set(
        "same",
        vm.new_function(
            "same",
            move |a: i64, c: i64, vm: &VirtualMachine| -> PyResult<bool> {
                Ok(Value::same_object(&b.object(a, vm)?, &b.object(c, vm)?))
            },
        )
        .into(),
    )?;

    let b = bridge.clone();
    set(
        "identity",
        vm.new_function(
            "identity",
            move |h: i64, vm: &VirtualMachine| -> PyResult<i64> {
                let o = b.object(h, vm)?;
                Ok(Rc::as_ptr(&o).cast::<u8>() as usize as i64)
            },
        )
        .into(),
    )?;

    let b = bridge.clone();
    set(
        "primitive",
        vm.new_function(
            "primitive",
            move |h: i64, vm: &VirtualMachine| -> PyResult {
                let obj = b.object(h, vm)?;
                match obj.get("", &[]) {
                    Ok(v) if !v.is_object() && !matches!(v, Value::Array(_) | Value::Record(_)) => {
                        to_py(vm, &b, &v)
                    }
                    _ => Ok(vm.ctx.new_str(format!("<{}>", obj.type_name())).into()),
                }
            },
        )
        .into(),
    )?;

    set(
        "error_text",
        vm.new_function("error_text", |code: i32, _vm: &VirtualMachine| {
            error_text(code).to_owned()
        })
            .into(),
    )?;

    let b = bridge.clone();
    set(
        "type_of",
        vm.new_function(
            "type_of",
            move |v: PyObjectRef, vm: &VirtualMachine| -> String {
                match from_py(vm, &b, &v, Value::Empty) {
                    Ok(v) => type_name(&v),
                    Err(_) => "Object".to_owned(),
                }
            },
        )
        .into(),
    )?;

    let b = bridge.clone();
    set(
        "print",
        vm.new_function("print", move |line: String, _vm: &VirtualMachine| {
            if let Ok(mut h) = b.host.try_borrow_mut() {
                h.debug_print(&line);
            }
        })
        .into(),
    )?;

    let b = bridge.clone();
    set(
        "msg_box",
        vm.new_function(
            "msg_box",
            move |prompt: String, buttons: i32, title: String, vm: &VirtualMachine| -> PyResult<i32> {
                b.check(vm)?;
                let r = match b.host.try_borrow_mut() {
                    Ok(mut h) => h.msg_box(&prompt, buttons, &title),
                    Err(_) => Ok(1),
                };
                r.map_err(|e| b.error(&e, vm))
            },
        )
        .into(),
    )?;

    let b = bridge.clone();
    set(
        "input_box",
        vm.new_function(
            "input_box",
            move |prompt: String, title: String, default: String, vm: &VirtualMachine| -> PyResult {
                b.check(vm)?;
                let r = match b.host.try_borrow_mut() {
                    Ok(mut h) => h.input_box(&prompt, &title, &default),
                    Err(_) => Ok(None),
                };
                match r {
                    Ok(Some(s)) => Ok(vm.ctx.new_str(s).into()),
                    Ok(None) => Ok(vm.ctx.none()),
                    Err(e) => Err(b.error(&e, vm)),
                }
            },
        )
        .into(),
    )?;

    let globals = bridge
        .host
        .try_borrow_mut()
        .ok()
        .and_then(|mut h| h.globals());
    match &globals {
        Some(g) => {
            let h = bridge.handle(g.clone());
            set("globals", vm.ctx.new_int(h).into())?;
            let names: Vec<PyObjectRef> = GLOBAL_NAMES
                .iter()
                .map(|n| vm.ctx.new_str(*n).into())
                .collect();
            set("global_names", vm.ctx.new_list(names).into())?;
        }
        None => {
            set("globals", vm.ctx.none())?;
        }
    }

    // The prelude, with `_k` as its native side.
    let prelude = vm.ctx.new_dict();
    let module = vm.new_module("_kantor", k.clone(), None);
    prelude.set_item("_k", module.into(), vm)?;
    prelude.set_item("__name__", vm.ctx.new_str("_kantor_prelude").into(), vm)?;
    let scope = Scope::with_builtins(None, prelude.clone(), vm);
    vm.run_string(scope, PRELUDE, "<kantor>")?;
    let class = |name: &str| -> PyResult<PyTypeRef> {
        prelude
            .get_item(name, vm)?
            .downcast::<PyType>()
            .map_err(|_| vm.new_type_error(format!("{name} is not a class")))
    };
    *bridge.classes.borrow_mut() = Some(Rc::new(Classes {
        obj: class("_Obj")?,
        live: class("_Live")?,
        date: class("Date")?,
        cell_error: class("CellError")?,
        kantor_error: class("KantorError")?,
        describe: prelude.get_item("_describe", vm)?,
    }));

    // The object model's constants (`xlUp`) and VBA's (`vbYesNo`) for every module.
    let exports = prelude
        .get_item("_exports", vm)?
        .downcast::<PyDict>()
        .map_err(|_| vm.new_type_error("_exports is not a dict"))?;
    let constants = bridge
        .host
        .try_borrow_mut()
        .map(|mut h| h.constants())
        .unwrap_or_default();
    for (name, v) in constants
        .into_iter()
        .chain(binding::vb_constants().into_iter().map(|(n, v)| (n.to_owned(), v)))
    {
        if exports.get_item_opt(name.as_str(), vm)?.is_none() {
            exports.set_item(name.as_str(), to_py(vm, bridge, &v)?, vm)?;
        }
    }
    Ok(exports)
}

/// Positional (`None` is an omitted argument) and named (`(name, value)` pairs) arguments.
fn arguments(
    vm: &VirtualMachine,
    b: &Bridge,
    positional: &[PyObjectRef],
    named: &[PyObjectRef],
) -> PyResult<Vec<Arg>> {
    let mut out = Vec::with_capacity(positional.len() + named.len());
    for v in positional {
        out.push(Arg {
            name: None,
            value: from_py(vm, b, v, Value::Missing)?,
        });
    }
    for pair in named {
        let pair: Vec<PyObjectRef> = pair.try_to_value(vm)?;
        let [k, v] = pair.as_slice() else {
            return Err(vm.new_type_error("a named argument is a (name, value) pair"));
        };
        let k: String = k.try_to_value(vm)?;
        out.push(Arg {
            name: Some(k.replace('_', "")),
            value: from_py(vm, b, v, Value::Missing)?,
        });
    }
    Ok(out)
}

/// A [`Value`] as Python.
fn to_py(vm: &VirtualMachine, b: &Bridge, v: &Value) -> PyResult {
    Ok(match v {
        Value::Empty | Value::Null | Value::Missing | Value::Object(None) => vm.ctx.none(),
        Value::Boolean(x) => vm.ctx.new_bool(*x).into(),
        Value::Byte(n) => vm.ctx.new_int(*n).into(),
        Value::Integer(n) => vm.ctx.new_int(*n).into(),
        Value::Long(n) => vm.ctx.new_int(*n).into(),
        Value::LongLong(n) => vm.ctx.new_int(*n).into(),
        Value::Single(n) => vm.ctx.new_float(f64::from(*n)).into(),
        Value::Double(n) => vm.ctx.new_float(*n).into(),
        Value::Currency(n) => vm.ctx.new_float(*n as f64 / 10_000.0).into(),
        Value::String(s) => vm.ctx.new_str(s.as_str()).into(),
        Value::Date(d) => {
            let cls = b.classes().date.clone();
            cls.as_object().call(vec![vm.ctx.new_float(*d).into()], vm)?
        }
        Value::Error(code) => {
            let cls = b.classes().cell_error.clone();
            cls.as_object().call(vec![vm.ctx.new_int(*code).into()], vm)?
        }
        Value::Array(a) => array_to_py(vm, b, a, 0, &mut Vec::new())?,
        Value::Record(r) => {
            let d = vm.ctx.new_dict();
            for (k, x) in &r.fields {
                d.set_item(k.as_str(), to_py(vm, b, x)?, vm)?;
            }
            d.into()
        }
        Value::Object(Some(o)) => {
            let h = b.handle(o.clone());
            let cls = b.classes().obj.clone();
            cls.as_object().call(vec![vm.ctx.new_int(h).into()], vm)?
        }
    })
}

/// Dimension `dim` of `a` as nested lists (rows of a 2-D array first).
fn array_to_py(
    vm: &VirtualMachine,
    b: &Bridge,
    a: &Array,
    dim: usize,
    index: &mut Vec<i32>,
) -> PyResult {
    let Some(d) = a.dims().get(dim) else {
        return to_py(vm, b, a.get(index).unwrap_or(&Value::Empty));
    };
    let mut out = Vec::with_capacity(d.len);
    for i in d.lower..=d.upper() {
        index.push(i);
        out.push(array_to_py(vm, b, a, dim + 1, index)?);
        index.pop();
    }
    Ok(vm.ctx.new_list(out).into())
}

fn is_sequence(vm: &VirtualMachine, o: &PyObjectRef) -> bool {
    o.fast_isinstance(vm.ctx.types.list_type) || o.fast_isinstance(vm.ctx.types.tuple_type)
}

fn mismatch(vm: &VirtualMachine, b: &Bridge, what: &str) -> PyBaseExceptionRef {
    b.error(
        &ScriptError::new(13, format!("Type mismatch ({what} isn't a value the workbook can take)")),
        vm,
    )
}

/// Python as a [`Value`]; `None` becomes `none` ([`Value::Missing`] for an argument,
/// [`Value::Empty`] otherwise).
fn from_py(vm: &VirtualMachine, b: &Bridge, o: &PyObjectRef, none: Value) -> PyResult<Value> {
    if vm.is_none(o) {
        return Ok(none);
    }
    if o.fast_isinstance(vm.ctx.types.bool_type) {
        return Ok(Value::Boolean(o.is(&vm.ctx.true_value)));
    }
    if o.fast_isinstance(vm.ctx.types.int_type) {
        return Ok(match o.try_to_value::<i64>(vm) {
            Ok(n) => match i32::try_from(n) {
                Ok(n) => Value::Long(n),
                Err(_) => Value::LongLong(n),
            },
            Err(_) => return Err(mismatch(vm, b, "an integer this large")),
        });
    }
    let classes = b.classes();
    if let Some(f) = o.downcast_ref::<PyFloat>() {
        return Ok(if o.fast_isinstance(&classes.date) {
            Value::Date(f.to_f64())
        } else {
            Value::Double(f.to_f64())
        });
    }
    if o.fast_isinstance(vm.ctx.types.str_type) {
        return Ok(Value::String(o.try_to_value::<String>(vm)?));
    }
    if is_sequence(vm, o) {
        let items: Vec<PyObjectRef> = o.try_to_value(vm)?;
        // A list of lists is a 2-D array (rows), as `Range.Value` takes it.
        if !items.is_empty() && items.iter().all(|x| is_sequence(vm, x)) {
            let mut rows = Vec::with_capacity(items.len());
            for row in &items {
                let cells: Vec<PyObjectRef> = row.try_to_value(vm)?;
                let mut out = Vec::with_capacity(cells.len());
                for c in &cells {
                    out.push(from_py(vm, b, c, Value::Empty)?);
                }
                rows.push(out);
            }
            return Ok(Value::array(Array::from_rows(0, rows)));
        }
        let mut out = Vec::with_capacity(items.len());
        for x in &items {
            out.push(from_py(vm, b, x, Value::Empty)?);
        }
        return Ok(Value::array(Array::from_vec(0, out)));
    }
    if o.fast_isinstance(&classes.obj) {
        let h: i64 = o.get_attr("_h", vm)?.try_to_value(vm)?;
        return Ok(Value::Object(Some(b.object(h, vm)?)));
    }
    if o.fast_isinstance(&classes.live) {
        let now = vm.call_method(o, "_now", ())?;
        return from_py(vm, b, &now, none);
    }
    if o.fast_isinstance(&classes.cell_error) {
        let code: i32 = o.get_attr("code", vm)?.try_to_value(vm)?;
        return Ok(Value::Error(code));
    }
    if o.fast_isinstance(vm.ctx.types.dict_type) {
        let items: Vec<PyObjectRef> = vm.call_method(o, "items", ())?.try_to_value(vm)?;
        let mut fields = Vec::with_capacity(items.len());
        for pair in &items {
            let pair: Vec<PyObjectRef> = pair.try_to_value(vm)?;
            if let [k, v] = pair.as_slice() {
                let k: String = k.str(vm)?.to_string();
                fields.push((k, from_py(vm, b, v, Value::Empty)?));
            }
        }
        return Ok(Value::Record(Rc::new(Record {
            type_name: "dict".to_owned(),
            fields,
        })));
    }
    Err(mismatch(vm, b, &format!("a {}", o.class().name())))
}

/// Excel's text for an error value.
fn error_text(code: i32) -> &'static str {
    match code {
        2000 => "#NULL!",
        2007 => "#DIV/0!",
        2015 => "#VALUE!",
        2023 => "#REF!",
        2029 => "#NAME?",
        2036 => "#NUM!",
        2042 => "#N/A",
        2043 => "#GETTING_DATA",
        2045 => "#SPILL!",
        2047 => "#BLOCKED!",
        2048 => "#UNKNOWN!",
        2049 => "#FIELD!",
        2050 => "#CALC!",
        _ => "#ERROR!",
    }
}

/// VBA's `TypeName` for a value.
fn type_name(v: &Value) -> String {
    match v {
        Value::Empty | Value::Missing => "Empty",
        Value::Null => "Null",
        Value::Boolean(_) => "Boolean",
        Value::Byte(_) => "Byte",
        Value::Integer(_) => "Integer",
        Value::Long(_) => "Long",
        Value::LongLong(_) => "LongLong",
        Value::Single(_) => "Single",
        Value::Double(_) => "Double",
        Value::Currency(_) => "Currency",
        Value::Date(_) => "Date",
        Value::String(_) => "String",
        Value::Error(_) => "Error",
        Value::Array(_) => "Variant()",
        Value::Record(_) => "Object",
        Value::Object(None) => "Nothing",
        Value::Object(Some(o)) => return o.type_name(),
    }
    .to_owned()
}

/// A Python exception as a [`ScriptError`], through the prelude's `_describe`.
fn caught(vm: &VirtualMachine, b: &Bridge, e: PyBaseExceptionRef) -> ScriptError {
    if b.cancel.is_cancelled() {
        return ScriptError::interrupted(18, kantor_script::object::standard_message(18));
    }
    let described = b
        .classes
        .borrow()
        .as_ref()
        .map(|c| c.describe.clone())
        .and_then(|d| d.call(vec![PyObjectRef::from(e.clone())], vm).ok())
        .and_then(|s| s.try_to_value::<String>(vm).ok());
    let Some(text) = described else {
        let name = e.class().name().to_string();
        return ScriptError::new(0, format!("{name} (in the Python bridge)"));
    };
    let mut parts = text.split('\u{1f}');
    let number = parts.next().unwrap_or_default();
    let kind = parts.next().unwrap_or_default();
    let message = parts.next().unwrap_or_default();
    let frames = parts.next().unwrap_or_default();
    let mut err = match number.parse::<i32>() {
        Ok(n) => ScriptError::new(n, message),
        Err(_) if kind == "SyntaxError" || kind == "IndentationError" || kind == "TabError" => {
            ScriptError::syntax(format!("{kind}: {message}"))
        }
        Err(_) if kind == "MemoryError" => ScriptError::new(7, format!("Out of memory ({message})")),
        Err(_) if kind == "RecursionError" => {
            ScriptError::new(28, format!("Out of stack space ({message})"))
        }
        Err(_) if message.is_empty() => ScriptError::new(0, kind),
        Err(_) => ScriptError::new(0, format!("{kind}: {message}")),
    };
    err.location = frames
        .split('\u{1e}')
        .rev()
        .filter_map(|f| {
            let mut p = f.split('\u{1d}');
            let file = p.next()?;
            let line: u32 = p.next()?.parse().ok()?;
            let func = p.next().unwrap_or_default();
            let (_, module) = b.files.iter().find(|(f, _)| f == file)?;
            Some(Location {
                module: module.clone(),
                procedure: if func.starts_with('<') {
                    String::new()
                } else {
                    func.to_owned()
                },
                line,
            })
        })
        .next();
    err
}

#[cfg(test)]
mod tests;
