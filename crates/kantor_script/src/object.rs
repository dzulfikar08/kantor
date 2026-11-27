//! The host-neutral object layer: values, objects and the host.
//!
//! The shapes follow OLE Automation, which VBA, Office's object models and Windows Script Host
//! all share: a [`Value`] is a VARIANT, a [`Dispatch`] object is an IDispatch (members found by
//! case-insensitive name, optional named arguments, member `""` is the default member,
//! DISPID_VALUE), and errors carry an Automation error number and description. Nothing here
//! knows VBA syntax or coercion rules; those live in [`crate::vba`]. A JavaScript or Python
//! binding wraps the same [`ObjectRef`]s (see `docs/scripting.md`).

use std::any::Any;
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A shared reference to an object. Objects use interior mutability: every method takes `&self`.
pub type ObjectRef = Rc<dyn Dispatch>;

/// A dynamically typed value (an OLE VARIANT).
///
/// Arrays and records have value semantics: cloning a `Value` shares the storage until one copy
/// is changed (`Rc::make_mut`). Objects have reference semantics.
#[derive(Clone, Default)]
pub enum Value {
    /// Uninitialised (VT_EMPTY): `0` in numeric contexts, `""` in string contexts.
    #[default]
    Empty,
    /// No valid data (VT_NULL); propagates through most operators.
    Null,
    /// An optional argument that was left out (VT_ERROR with DISP_E_PARAMNOTFOUND).
    Missing,
    Boolean(bool),
    Byte(u8),
    /// 16-bit integer (VBA `Integer`).
    Integer(i16),
    /// 32-bit integer (VBA `Long`).
    Long(i32),
    /// 64-bit integer (VBA `LongLong`).
    LongLong(i64),
    Single(f32),
    Double(f64),
    /// Fixed point, scaled by 10 000 (VBA `Currency`).
    Currency(i64),
    /// OLE Automation date: days since 1899-12-30, the fraction is the time of day.
    Date(f64),
    String(String),
    /// An error value (VT_ERROR), e.g. an Excel cell error (`CVErr(2007)` is `#DIV/0!`).
    Error(i32),
    Array(Rc<Array>),
    /// A user-defined type (VBA `Type ... End Type`) or another structured value.
    Record(Rc<Record>),
    /// An object reference; `None` is `Nothing`.
    Object(Option<ObjectRef>),
}

impl Value {
    /// `Nothing`.
    pub const NOTHING: Value = Value::Object(None);

    pub fn string(s: impl Into<String>) -> Value {
        Value::String(s.into())
    }

    pub fn object(o: ObjectRef) -> Value {
        Value::Object(Some(o))
    }

    pub fn array(a: Array) -> Value {
        Value::Array(Rc::new(a))
    }

    pub fn is_object(&self) -> bool {
        matches!(self, Value::Object(_))
    }

    pub fn as_object(&self) -> Option<&ObjectRef> {
        match self {
            Value::Object(Some(o)) => Some(o),
            _ => None,
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Value::Empty)
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn is_missing(&self) -> bool {
        matches!(self, Value::Missing)
    }

    /// True for the numeric variants (Byte to Currency, not Date or Boolean).
    pub fn is_number(&self) -> bool {
        matches!(
            self,
            Value::Byte(_)
                | Value::Integer(_)
                | Value::Long(_)
                | Value::LongLong(_)
                | Value::Single(_)
                | Value::Double(_)
                | Value::Currency(_)
        )
    }

    /// The value as `f64` for the numeric, Boolean and Date variants, without any string parsing.
    pub fn as_f64(&self) -> Option<f64> {
        Some(match self {
            Value::Boolean(b) => {
                if *b {
                    -1.0
                } else {
                    0.0
                }
            }
            Value::Byte(n) => f64::from(*n),
            Value::Integer(n) => f64::from(*n),
            Value::Long(n) => f64::from(*n),
            Value::LongLong(n) => *n as f64,
            Value::Single(n) => f64::from(*n),
            Value::Double(n) | Value::Date(n) => *n,
            Value::Currency(n) => *n as f64 / 10_000.0,
            _ => return None,
        })
    }

    /// The string inside a `String` value.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// Whether two object references point at the same object (`Is`).
    pub fn same_object(a: &ObjectRef, b: &ObjectRef) -> bool {
        std::ptr::addr_eq(Rc::as_ptr(a), Rc::as_ptr(b))
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Empty => write!(f, "Empty"),
            Value::Null => write!(f, "Null"),
            Value::Missing => write!(f, "Missing"),
            Value::Boolean(b) => write!(f, "Boolean({b})"),
            Value::Byte(n) => write!(f, "Byte({n})"),
            Value::Integer(n) => write!(f, "Integer({n})"),
            Value::Long(n) => write!(f, "Long({n})"),
            Value::LongLong(n) => write!(f, "LongLong({n})"),
            Value::Single(n) => write!(f, "Single({n})"),
            Value::Double(n) => write!(f, "Double({n})"),
            Value::Currency(n) => write!(f, "Currency({})", *n as f64 / 10_000.0),
            Value::Date(n) => write!(f, "Date({n})"),
            Value::String(s) => write!(f, "String({s:?})"),
            Value::Error(n) => write!(f, "Error({n})"),
            Value::Array(a) => write!(f, "Array({a:?})"),
            Value::Record(r) => write!(f, "Record({r:?})"),
            Value::Object(None) => write!(f, "Nothing"),
            Value::Object(Some(o)) => write!(f, "Object({})", o.type_name()),
        }
    }
}

/// Structural equality; objects compare by identity. This is Rust equality for tests and hosts,
/// not a language's `=` operator.
impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Empty, Value::Empty)
            | (Value::Null, Value::Null)
            | (Value::Missing, Value::Missing) => true,
            (Value::Boolean(a), Value::Boolean(b)) => a == b,
            (Value::Byte(a), Value::Byte(b)) => a == b,
            (Value::Integer(a), Value::Integer(b)) => a == b,
            (Value::Long(a), Value::Long(b)) => a == b,
            (Value::LongLong(a), Value::LongLong(b)) => a == b,
            (Value::Single(a), Value::Single(b)) => a == b,
            (Value::Double(a), Value::Double(b)) => a == b,
            (Value::Currency(a), Value::Currency(b)) => a == b,
            (Value::Date(a), Value::Date(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Error(a), Value::Error(b)) => a == b,
            (Value::Array(a), Value::Array(b)) => a == b,
            (Value::Record(a), Value::Record(b)) => a == b,
            (Value::Object(None), Value::Object(None)) => true,
            (Value::Object(Some(a)), Value::Object(Some(b))) => Value::same_object(a, b),
            _ => false,
        }
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Value {
        Value::Boolean(v)
    }
}

impl From<i16> for Value {
    fn from(v: i16) -> Value {
        Value::Integer(v)
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Value {
        Value::Long(v)
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Value {
        Value::LongLong(v)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Value {
        Value::Double(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Value {
        Value::String(v.to_string())
    }
}

impl From<String> for Value {
    fn from(v: String) -> Value {
        Value::String(v)
    }
}

impl From<ObjectRef> for Value {
    fn from(v: ObjectRef) -> Value {
        Value::Object(Some(v))
    }
}

/// One dimension of an [`Array`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dim {
    pub lower: i32,
    pub len: usize,
}

impl Dim {
    pub fn upper(&self) -> i32 {
        self.lower + self.len as i32 - 1
    }
}

/// A multi-dimensional array with per-dimension lower bounds (an OLE SAFEARRAY).
///
/// Elements are stored column-major (the first index varies fastest), the SAFEARRAY layout, so
/// enumeration order matches VBA's `For Each` over a 2-D array. An array with no dimensions is
/// an unallocated dynamic array (`Dim a()` before `ReDim`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Array {
    dims: Vec<Dim>,
    data: Vec<Value>,
}

impl Array {
    /// An unallocated dynamic array.
    pub fn unallocated() -> Array {
        Array::default()
    }

    /// An array with the given `(lower, upper)` bounds, filled with `fill`. An upper bound of
    /// `lower - 1` makes an empty dimension; smaller is "Subscript out of range".
    pub fn new(bounds: &[(i32, i32)], fill: Value) -> Result<Array, ScriptError> {
        let mut dims = Vec::with_capacity(bounds.len());
        let mut total: usize = 1;
        for &(lower, upper) in bounds {
            let len = i64::from(upper) - i64::from(lower) + 1;
            if len < 0 {
                return Err(ScriptError::runtime(9));
            }
            let len = len as usize;
            total = total
                .checked_mul(len)
                .filter(|t| *t <= 1 << 28)
                .ok_or_else(|| ScriptError::runtime(7))?;
            dims.push(Dim { lower, len });
        }
        Ok(Array {
            dims,
            data: vec![fill; total],
        })
    }

    /// A one-dimensional array starting at `lower`.
    pub fn from_vec(lower: i32, items: Vec<Value>) -> Array {
        Array {
            dims: vec![Dim {
                lower,
                len: items.len(),
            }],
            data: items,
        }
    }

    /// A two-dimensional array from rows (`rows[r][c]`), both bounds starting at `lower`. Rows
    /// shorter than the longest are padded with Empty. This is the shape of `Range.Value`.
    pub fn from_rows(lower: i32, rows: Vec<Vec<Value>>) -> Array {
        let nrows = rows.len();
        let ncols = rows.iter().map(Vec::len).max().unwrap_or(0);
        let mut data = vec![Value::Empty; nrows * ncols];
        for (r, row) in rows.into_iter().enumerate() {
            for (c, v) in row.into_iter().enumerate() {
                data[r + c * nrows] = v;
            }
        }
        Array {
            dims: vec![Dim { lower, len: nrows }, Dim { lower, len: ncols }],
            data,
        }
    }

    pub fn dims(&self) -> &[Dim] {
        &self.dims
    }

    pub fn rank(&self) -> usize {
        self.dims.len()
    }

    pub fn is_allocated(&self) -> bool {
        !self.dims.is_empty()
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Elements in storage (column-major) order.
    pub fn values(&self) -> &[Value] {
        &self.data
    }

    pub fn values_mut(&mut self) -> &mut [Value] {
        &mut self.data
    }

    pub fn into_values(self) -> Vec<Value> {
        self.data
    }

    /// The storage index of an element, or `None` when an index is out of range.
    pub fn offset(&self, index: &[i32]) -> Option<usize> {
        if index.len() != self.dims.len() {
            return None;
        }
        let mut offset = 0usize;
        let mut stride = 1usize;
        for (i, d) in index.iter().zip(&self.dims) {
            let k = i64::from(*i) - i64::from(d.lower);
            if k < 0 || k as usize >= d.len {
                return None;
            }
            offset += k as usize * stride;
            stride *= d.len;
        }
        Some(offset)
    }

    pub fn get(&self, index: &[i32]) -> Option<&Value> {
        self.offset(index).map(|o| &self.data[o])
    }

    pub fn get_mut(&mut self, index: &[i32]) -> Option<&mut Value> {
        self.offset(index).map(move |o| &mut self.data[o])
    }

    /// Rows of a two-dimensional array (`rows[r][c]`); a one-dimensional array is one row.
    pub fn rows(&self) -> Vec<Vec<Value>> {
        match self.dims.as_slice() {
            [d] => vec![self.data[..d.len].to_vec()],
            [r, c] => (0..r.len)
                .map(|i| {
                    (0..c.len)
                        .map(|j| self.data[i + j * r.len].clone())
                        .collect()
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Resizes keeping the elements, as `ReDim Preserve`: only the last dimension may change
    /// size (its lower bound may move), the rank must stay. New elements are `fill`.
    pub fn resize_last(&mut self, bounds: &[(i32, i32)], fill: Value) -> Result<(), ScriptError> {
        if !self.is_allocated() {
            *self = Array::new(bounds, fill)?;
            return Ok(());
        }
        if bounds.len() != self.dims.len() {
            return Err(ScriptError::runtime(9));
        }
        let last = bounds.len() - 1;
        for (i, (&(lower, upper), d)) in bounds.iter().zip(&self.dims).enumerate() {
            if i < last && (lower != d.lower || upper != d.upper()) {
                return Err(ScriptError::runtime(9));
            }
        }
        let (lower, upper) = bounds[last];
        let len = i64::from(upper) - i64::from(lower) + 1;
        if len < 0 {
            return Err(ScriptError::runtime(9));
        }
        let inner: usize = self.dims[..last].iter().map(|d| d.len).product();
        self.data.resize(inner * len as usize, fill);
        self.dims[last] = Dim {
            lower,
            len: len as usize,
        };
        Ok(())
    }
}

/// A structured value: a VBA user-defined type, or a record a host hands out.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Record {
    pub type_name: String,
    pub fields: Vec<(String, Value)>,
}

impl Record {
    pub fn field(&self, name: &str) -> Option<&Value> {
        self.fields
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }

    pub fn field_mut(&mut self, name: &str) -> Option<&mut Value> {
        self.fields
            .iter_mut()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }
}

/// One argument of a member call: positional (`name` is `None`) or named (`Destination:=x`).
/// Named arguments always follow the positional ones. An omitted positional argument
/// (`Foo 1, , 3`) is [`Value::Missing`].
#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    pub name: Option<String>,
    pub value: Value,
}

impl Arg {
    pub fn new(value: impl Into<Value>) -> Arg {
        Arg {
            name: None,
            value: value.into(),
        }
    }

    pub fn named(name: &str, value: impl Into<Value>) -> Arg {
        Arg {
            name: Some(name.to_string()),
            value: value.into(),
        }
    }
}

/// The argument at positional `position` or named `name` (case-insensitive). Omitted
/// ([`Value::Missing`]) arguments count as absent.
pub fn find_arg<'a>(args: &'a [Arg], position: usize, name: &str) -> Option<&'a Value> {
    let found = args
        .iter()
        .find(|a| {
            a.name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
        .or_else(|| args.iter().filter(|a| a.name.is_none()).nth(position))?;
    if found.value.is_missing() {
        None
    } else {
        Some(&found.value)
    }
}

/// An object, IDispatch-style. Member names are case-insensitive; `""` is the default member
/// (`Range("A1") = 5` puts the default member of the `Range`).
///
/// - [`get`](Dispatch::get): property read or function call whose value is used.
/// - [`put`](Dispatch::put): property write with a plain value (VBA `Let`).
/// - [`put_ref`](Dispatch::put_ref): property write with an object reference (VBA `Set`).
/// - [`call`](Dispatch::call): method call as a statement; the value may be ignored.
///
/// An unknown member should fail with [`ScriptError::no_member`] (error 438): the interpreter
/// uses that number to fall back to other meanings of a name.
pub trait Dispatch {
    /// The type name scripts see (`TypeName(x)`, `TypeOf x Is Range`).
    fn type_name(&self) -> String;

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError>;

    fn put(&self, name: &str, _args: &[Arg], _value: Value) -> Result<(), ScriptError> {
        Err(ScriptError::no_member(&self.type_name(), name))
    }

    fn put_ref(&self, name: &str, args: &[Arg], value: Value) -> Result<(), ScriptError> {
        self.put(name, args, value)
    }

    fn call(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        self.get(name, args)
    }

    /// The items `For Each` visits.
    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        Err(ScriptError::runtime(438))
    }

    /// Downcasting for objects an interpreter implements itself.
    fn as_any(&self) -> Option<&dyn Any> {
        None
    }

    /// The code name of the document this object is (`Sheet1`, `ThisWorkbook`): the public
    /// members of that document module are members of the object too.
    fn document_module(&self) -> Option<String> {
        None
    }

    /// A member the script runtime answers itself instead of the object (Excel's
    /// `Application.Run`, which calls a procedure of the running project).
    fn script_member(&self, _name: &str) -> Option<ScriptMember> {
        None
    }
}

/// Members of host objects that the language runtime implements ([`Dispatch::script_member`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptMember {
    /// `Run(Macro, Arg1, Arg2…)`: call a procedure of the project by name (`"Macro1"`,
    /// `"Module1.Macro1"`, `"'Book.xlsm'!Macro1"`) and return its result.
    Run,
}

/// Something the host asks the script runtime to run between two statements: the event
/// handlers for what the script just did (a cell it changed fires `Worksheet_Change`).
#[derive(Clone, Debug, PartialEq)]
pub enum HostCall {
    /// A procedure by name (`"Sheet1.Worksheet_Change"`), arguments by value. Skipped when the
    /// project has no such procedure.
    Proc { name: String, args: Vec<Value> },
    /// An event of a host object, for the `WithEvents` variables holding an object of type
    /// `source_type` (`"Application"`): each one's `<variable>_<event>` procedure runs.
    Event {
        source_type: String,
        event: String,
        args: Vec<Value>,
    },
}

/// Where a script runs: global names, UI and output, time and cancellation.
///
/// Every method has a default, so a host implements only what it offers.
pub trait Host {
    /// The object whose members are visible without qualification, like Excel's `Application`
    /// global members (`Range`, `Cells`, `ActiveSheet`, `Worksheets`). Names the script doesn't
    /// define itself are looked up here (`get`/`put`/`call` with the bare name).
    fn globals(&mut self) -> Option<ObjectRef> {
        None
    }

    /// A named constant from the host's object model (`xlUp`, `xlValues`).
    fn constant(&mut self, _name: &str) -> Option<Value> {
        None
    }

    /// `CreateObject(class)` / `New class` for classes the language runtime doesn't provide.
    fn create_object(&mut self, _class: &str) -> Option<Result<ObjectRef, ScriptError>> {
        None
    }

    /// A message box; returns the button pressed (1 = OK, 2 = Cancel, 6 = Yes, 7 = No).
    fn msg_box(&mut self, _prompt: &str, _buttons: i32, _title: &str) -> Result<i32, ScriptError> {
        Ok(1)
    }

    /// An input box; `None` when cancelled.
    fn input_box(
        &mut self,
        _prompt: &str,
        _title: &str,
        _default: &str,
    ) -> Result<Option<String>, ScriptError> {
        Ok(None)
    }

    /// One line of `Debug.Print` / `console.log` output.
    fn debug_print(&mut self, _line: &str) {}

    /// Shows a UserForm, or updates the one showing, and waits for the user's next step (see
    /// [`crate::forms`]). Called again after each step until the form closes. The default has
    /// no window to show one in.
    fn form_event(
        &mut self,
        view: &crate::forms::FormView,
    ) -> Result<crate::forms::FormInput, ScriptError> {
        Err(ScriptError::new(
            445,
            format!(
                "The macro shows the UserForm {}, and there is no window to show it in",
                view.design.name
            ),
        ))
    }

    /// The form [`Host::form_event`] was showing is hidden or unloaded.
    fn form_closed(&mut self, _name: &str) {}

    /// The local date and time as an OLE date. The default is UTC.
    fn now(&mut self) -> f64 {
        crate::date::now_utc()
    }

    /// Polled while a script runs; returning true stops it with error 18 (user interrupt).
    fn cancelled(&mut self) -> bool {
        false
    }

    /// Asked between statements: procedures to run now (event handlers for changes the
    /// script made). Keep it cheap: it is called before every statement.
    fn take_calls(&mut self) -> Vec<HostCall> {
        Vec::new()
    }
}

/// A host with no globals and no UI.
#[derive(Default)]
pub struct NullHost;

impl Host for NullHost {}

/// A flag the UI thread sets to stop a running script; a [`Host`] returns it from
/// [`Host::cancelled`].
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> CancelToken {
        CancelToken::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn reset(&self) {
        self.0.store(false, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// What kind of failure a [`ScriptError`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// A run-time error a script can trap (`On Error`).
    Runtime,
    /// A feature Kantor doesn't run (file I/O, `Declare`, `Shell`, UserForms). Trappable like a
    /// run-time error; the description names the feature.
    Unsupported,
    /// The source doesn't parse, or a name/label doesn't exist (VBA's compile errors). Not
    /// trappable.
    Syntax,
    /// Stopped by the host or the step budget. Not trappable.
    Interrupted,
    /// Internal: the `End` statement. [`crate::vba::Project::run`] turns it into success.
    Halt,
}

/// Where an error happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub module: String,
    pub procedure: String,
    pub line: u32,
}

/// An error with an Automation error number (`Err.Number`) and description.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptError {
    pub number: i32,
    pub description: String,
    /// `Err.Source`: the object or project that raised it.
    pub source: String,
    pub kind: ErrorKind,
    pub location: Option<Location>,
}

impl ScriptError {
    pub fn new(number: i32, description: impl Into<String>) -> ScriptError {
        ScriptError {
            number,
            description: description.into(),
            source: String::new(),
            kind: ErrorKind::Runtime,
            location: None,
        }
    }

    /// A run-time error with the standard VBA description for `number`.
    pub fn runtime(number: i32) -> ScriptError {
        ScriptError::new(number, standard_message(number))
    }

    /// "Object doesn't support this property or method" (438), naming the member.
    pub fn no_member(type_name: &str, member: &str) -> ScriptError {
        let what = if member.is_empty() {
            "a default member".to_string()
        } else {
            format!("'{member}'")
        };
        ScriptError::new(
            438,
            format!("Object doesn't support this property or method ({type_name} has no {what})"),
        )
    }

    /// A feature Kantor doesn't run.
    pub fn unsupported(feature: &str) -> ScriptError {
        ScriptError {
            kind: ErrorKind::Unsupported,
            ..ScriptError::new(445, format!("{feature} is not supported in Kantor"))
        }
    }

    pub fn syntax(description: impl Into<String>) -> ScriptError {
        ScriptError {
            kind: ErrorKind::Syntax,
            ..ScriptError::new(0, description)
        }
    }

    pub fn interrupted(number: i32, description: impl Into<String>) -> ScriptError {
        ScriptError {
            kind: ErrorKind::Interrupted,
            ..ScriptError::new(number, description)
        }
    }

    /// Whether `On Error` can trap it.
    pub fn is_trappable(&self) -> bool {
        matches!(self.kind, ErrorKind::Runtime | ErrorKind::Unsupported)
    }

    pub fn with_source(mut self, source: impl Into<String>) -> ScriptError {
        self.source = source.into();
        self
    }
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ErrorKind::Syntax => write!(f, "Compile error: {}", self.description)?,
            _ => write!(f, "Run-time error {}: {}", self.number, self.description)?,
        }
        if let Some(loc) = &self.location {
            if loc.procedure.is_empty() {
                write!(f, " ({} line {})", loc.module, loc.line)?;
            } else {
                write!(f, " ({}.{} line {})", loc.module, loc.procedure, loc.line)?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for ScriptError {}

/// The standard description of a VBA run-time error number (the messages VBA shows; see
/// "Trappable errors" in the VBA language reference).
pub fn standard_message(number: i32) -> &'static str {
    match number {
        3 => "Return without GoSub",
        5 => "Invalid procedure call or argument",
        6 => "Overflow",
        7 => "Out of memory",
        9 => "Subscript out of range",
        10 => "This array is fixed or temporarily locked",
        11 => "Division by zero",
        13 => "Type mismatch",
        14 => "Out of string space",
        16 => "Expression too complex",
        17 => "Can't perform requested operation",
        18 => "User interrupt occurred",
        20 => "Resume without error",
        28 => "Out of stack space",
        35 => "Sub or Function not defined",
        51 => "Internal error",
        91 => "Object variable or With block variable not set",
        92 => "For loop not initialized",
        93 => "Invalid pattern string",
        94 => "Invalid use of Null",
        424 => "Object required",
        429 => "ActiveX component can't create object",
        438 => "Object doesn't support this property or method",
        445 => "Object doesn't support this action",
        448 => "Named argument not found",
        449 => "Argument not optional",
        450 => "Wrong number of arguments or invalid property assignment",
        451 => {
            "Property let procedure not defined and property get procedure did not return an object"
        }
        457 => "This key is already associated with an element of this collection",
        458 => "Variable uses an Automation type not supported in Visual Basic",
        32811 => "Element not found",
        _ => "Application-defined or object-defined error",
    }
}
