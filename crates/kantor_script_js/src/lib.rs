//! JavaScript macros: QuickJS (through rquickjs) bound to `kantor_script`'s object layer.
//!
//! [`run`] evaluates a workbook's JavaScript modules in a fresh runtime and calls one function.
//! The host's objects ([`kantor_script::Dispatch`]) appear as JavaScript objects:
//!
//! - Naming a member reads a property (`r.value`, `ws.name`) or gives a method to call
//!   (`r.clear()`, `r.offset(1, 0)`, `ws.range("A1")`), as [`kantor_script::binding::member`]
//!   decides. Member names are case-insensitive, as in VBA: `Range`, `range` and `RANGE` are one
//!   member; the idiomatic spelling is camelCase.
//! - Assigning sets a property: `r.value = 5`, `r.font.bold = true`.
//! - Calling an object takes its default member: `Worksheets("Data")`, `ws.cells(2, 1)`.
//! - `for (const ws of ActiveWorkbook.worksheets)` enumerates a collection.
//! - Named arguments (VBA's `Copy Destination:=x`) go in a trailing plain object:
//!   `r.copy({ destination: ws.range("D1") })`.
//! - Values: numbers, strings, booleans; an empty cell is `null`; dates are `Date`s (local
//!   time); Excel errors are `CellError`s (`CVErr(2042)` is `#N/A`); a 2-D `Range.value` is an
//!   array of rows. A failing member throws a `KantorError` with VBA's `number`.
//! - Globals: `Application`, `ActiveWorkbook`, `ActiveSheet`, `ActiveCell`, `Selection`,
//!   `ThisWorkbook`, `Workbooks`, `Worksheets`, `Sheets`, `Range`, `Cells`, `Rows`, `Columns`,
//!   `WorksheetFunction` (and their camelCase spellings), the host's constants (`xlUp`),
//!   `MsgBox`/`msgBox`, `InputBox`/`inputBox`, `CVErr`, `TypeName`, and `print`/`console.log`
//!   for the Immediate output.
//!
//! The runtime has no file, network or process access (QuickJS has none unless the embedder
//! adds it), a memory limit and a stack limit; [`Options::cancel`] stops a run (QuickJS's
//! interrupt handler checks it). Each run starts afresh: unlike VBA's module variables, globals
//! don't last from one run to the next.

mod convert;
mod scan;

use std::cell::RefCell;
use std::rc::Rc;

use kantor_script::{CancelToken, ErrorKind, Host, Location, ObjectRef, ScriptError, Value};
use rquickjs::class::{JsClass, Readable, Trace, Tracer};
use rquickjs::context::EvalOptions;
use rquickjs::function::Args;
use rquickjs::{Context, Ctx, JsLifetime, Runtime};

pub use scan::functions;

type Js<'js> = rquickjs::Value<'js>;

/// One module of JavaScript: `name` is what errors call it (`Report.js`).
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
    /// Bytes the runtime may allocate.
    pub memory_limit: usize,
    /// Bytes of native stack JavaScript may use: keep it well under the thread's stack.
    pub max_stack: usize,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            cancel: CancelToken::new(),
            memory_limit: 1 << 30,
            max_stack: 512 << 10,
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

/// A host object held by a JavaScript value.
pub(crate) struct Handle(pub(crate) ObjectRef);

impl<'js> Trace<'js> for Handle {
    fn trace<'a>(&self, _tracer: Tracer<'a, 'js>) {}
}

// SAFETY: `Handle` holds no JavaScript value, so it is the same type for every `'js`.
#[allow(unsafe_code)]
unsafe impl<'js> JsLifetime<'js> for Handle {
    type Changed<'to> = Handle;
}

impl<'js> JsClass<'js> for Handle {
    const NAME: &'static str = "KantorObject";
    type Mutable = Readable;

    fn constructor(_ctx: &Ctx<'js>) -> rquickjs::Result<Option<rquickjs::Constructor<'js>>> {
        Ok(None)
    }
}

/// What the native functions share during a run.
pub(crate) struct Bridge {
    pub(crate) host: Rc<RefCell<dyn Host>>,
    pub(crate) cancel: CancelToken,
    /// The module file names (`Report.js`), for error locations.
    pub(crate) files: Vec<(String, String)>,
}

/// Runs function `target` (`go`, or `Report.go`) of `modules` with `args`, after evaluating
/// every module (the target's module last, so its definitions win). Returns what the function
/// returns.
pub fn run(
    modules: &[Module],
    target: &str,
    args: &[Value],
    host: Rc<RefCell<dyn Host>>,
    options: &Options,
) -> Result<Value, ScriptError> {
    let (module, func) = kantor_script::binding::split_target(target);
    if let Some(m) = module
        && !modules.iter().any(|x| x.name.eq_ignore_ascii_case(m))
    {
        return Err(ScriptError::new(
            35,
            format!("There is no JavaScript module {m}"),
        ));
    }
    let mut ordered: Vec<&Module> = modules
        .iter()
        .filter(|m| module.is_none_or(|t| !m.name.eq_ignore_ascii_case(t)))
        .collect();
    ordered.extend(
        modules
            .iter()
            .filter(|m| module.is_some_and(|t| m.name.eq_ignore_ascii_case(t))),
    );
    let rt = Runtime::new().map_err(|e| internal(&e))?;
    rt.set_memory_limit(options.memory_limit);
    rt.set_max_stack_size(options.max_stack);
    let stop = options.cancel.clone();
    rt.set_interrupt_handler(Some(Box::new(move || stop.is_cancelled())));
    let context = Context::full(&rt).map_err(|e| internal(&e))?;
    let globals = host.borrow_mut().globals();
    let bridge = Rc::new(Bridge {
        host,
        cancel: options.cancel.clone(),
        files: modules
            .iter()
            .map(|m| (format!("{}.js", m.name), m.name.clone()))
            .collect(),
    });
    context.with(|ctx| {
        let fail = |e: rquickjs::Error| caught(&ctx, e, &bridge);
        convert::install(&ctx, &bridge, globals).map_err(fail)?;
        for m in &ordered {
            ctx.eval_with_options::<(), _>(
                m.source.as_bytes(),
                script_options(format!("{}.js", m.name)),
            )
            .map_err(fail)?;
        }
        let global = ctx.globals();
        let defined = global.contains_key(func).map_err(fail)?;
        let f: Option<Js> = if defined {
            Some(global.get(func).map_err(fail)?)
        } else {
            None
        };
        let Some(f) = f.as_ref().and_then(|f| f.as_function()).cloned() else {
            return Err(ScriptError::new(
                35,
                format!("Sub or Function not defined ({func})"),
            ));
        };
        let mut call = Args::new(ctx.clone(), args.len());
        for a in args {
            let v = convert::to_js(&ctx, a).map_err(|e| failed(&ctx, e, &bridge))?;
            call.push_arg(v).map_err(fail)?;
        }
        let r: Js = call.apply(&f).map_err(fail)?;
        if r.is_promise() {
            return Ok(Value::Empty);
        }
        convert::from_js(&ctx, &r, Value::Empty).map_err(|e| failed(&ctx, e, &bridge))
    })
}

/// Evaluates `source` as one module named `name` and returns its completion value (for tests
/// and the REPL-like uses of the bridge).
pub fn eval(
    name: &str,
    source: &str,
    host: Rc<RefCell<dyn Host>>,
    options: &Options,
) -> Result<Value, ScriptError> {
    let wrapped = format!(
        "function __kantor_eval() {{ return eval({}); }}",
        js_string(source)
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

/// `s` as a JavaScript string literal.
fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn internal(e: &rquickjs::Error) -> ScriptError {
    ScriptError::new(51, format!("JavaScript engine: {e}"))
}

fn failed(ctx: &Ctx<'_>, e: convert::Failed, bridge: &Bridge) -> ScriptError {
    match e {
        convert::Failed::Script(e) => e,
        convert::Failed::Js(e) => caught(ctx, e, bridge),
    }
}

/// The pending exception (or another failure) as a [`ScriptError`].
pub(crate) fn caught(ctx: &Ctx<'_>, e: rquickjs::Error, bridge: &Bridge) -> ScriptError {
    if bridge.cancel.is_cancelled() {
        // Clear what the interrupt left.
        let _ = ctx.catch();
        return ScriptError::interrupted(18, kantor_script::object::standard_message(18));
    }
    if !matches!(e, rquickjs::Error::Exception) {
        return internal(&e);
    }
    let v = ctx.catch();
    exception(&v, bridge)
}

fn exception(v: &Js<'_>, bridge: &Bridge) -> ScriptError {
    let Some(obj) = v.as_object() else {
        let text = v
            .as_string()
            .and_then(|s| s.to_string().ok())
            .unwrap_or_else(|| format!("{v:?}"));
        return ScriptError::new(0, format!("Uncaught {text}"));
    };
    let get = |k: &str| -> Option<String> {
        obj.get::<_, Option<rquickjs::Coerced<String>>>(k)
            .ok()
            .flatten()
            .map(|c| c.0)
    };
    let name = get("name").unwrap_or_default();
    let message = get("message").unwrap_or_default();
    let number = obj.get::<_, Option<i32>>("number").ok().flatten();
    let stack = get("stack").unwrap_or_default();
    let mut err = match (name.as_str(), number) {
        (_, Some(n)) => {
            let d = get("description").unwrap_or(message);
            ScriptError::new(n, d).with_source(get("source").unwrap_or_default())
        }
        ("SyntaxError", None) => ScriptError::syntax(format!("SyntaxError: {message}")),
        ("InternalError", None) if message.contains("out of memory") => {
            ScriptError::new(7, format!("Out of memory ({message})"))
        }
        ("InternalError" | "RangeError", None) if message.contains("stack") => {
            ScriptError::new(28, format!("Out of stack space ({message})"))
        }
        (_, None) if name.is_empty() => ScriptError::new(0, message),
        (_, None) => ScriptError::new(0, format!("{name}: {message}")),
    };
    err.location = location(&stack, &bridge.files);
    if err.kind == ErrorKind::Runtime && err.location.is_none() && name == "SyntaxError" {
        err.kind = ErrorKind::Syntax;
    }
    err
}

/// The first frame of `stack` in one of the modules: `at go (Report.js:12:5)`.
pub(crate) fn location(stack: &str, files: &[(String, String)]) -> Option<Location> {
    for line in stack.lines() {
        let l = line.trim();
        let l = l.strip_prefix("at ").unwrap_or(l);
        let (func, place) = match l.strip_suffix(')').and_then(|x| x.rsplit_once(" (")) {
            Some((f, p)) => (f.trim(), p),
            None => ("", l),
        };
        let mut parts = place.split(':');
        let file = parts.next().unwrap_or_default();
        let Some((_, module)) = files.iter().find(|(f, _)| f == file) else {
            continue;
        };
        let line: u32 = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        let procedure = if func.starts_with('<') || func == "anonymous" {
            String::new()
        } else {
            func.to_owned()
        };
        return Some(Location {
            module: module.clone(),
            procedure,
            line,
        });
    }
    None
}

/// Options to evaluate module file `file` as a sloppy-mode script, as browsers run scripts.
#[allow(clippy::field_reassign_with_default)]
fn script_options(file: String) -> EvalOptions {
    let mut opts = EvalOptions::default();
    opts.strict = false;
    opts.filename = Some(file);
    opts
}

#[cfg(test)]
mod tests;
