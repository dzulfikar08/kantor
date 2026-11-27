//! The tree-walking runtime: frames, name resolution, places (assignable locations), calls with
//! real `ByRef`, and the statement loop with `On Error` handling.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;

use super::ast::*;
use super::builtins;
use super::compile::*;
use super::convert::{self, Arith, Scalar};
use super::objects::*;
use crate::object::*;

pub(crate) type R<T> = Result<T, ScriptError>;

/// A module's evaluated constants and enum members, with their visibility.
type ConstMap = Rc<RefCell<HashMap<Rc<str>, (Value, bool)>>>;

/// Project-wide run-time state that outlives one `run` (module variables keep their values
/// between macro runs until the project is reset, as in VBA).
pub(crate) struct State {
    pub scopes: Vec<Option<Rc<Scope>>>,
    pub consts: Vec<Option<ConstMap>>,
    pub statics: HashMap<(usize, Rc<str>, u8), Rc<Scope>>,
    pub err: Rc<ErrObject>,
    pub rnd_seed: u32,
    pub print_line: String,
    pub steps: u64,
    pub max_steps: Option<u64>,
    pub max_depth: usize,
    pub constants: HashMap<String, Value>,
}

impl State {
    pub fn new() -> State {
        State {
            scopes: Vec::new(),
            consts: Vec::new(),
            statics: HashMap::new(),
            err: Rc::new(ErrObject::default()),
            rnd_seed: 0x50000,
            print_line: String::new(),
            steps: 0,
            max_steps: None,
            max_depth: 100,
            constants: HashMap::new(),
        }
    }

    /// Forgets module variables and statics (VBA's reset, or the `End` statement).
    pub fn reset(&mut self) {
        self.scopes.clear();
        self.consts.clear();
        self.statics.clear();
        self.err.clear();
        self.print_line.clear();
    }
}

#[derive(Clone)]
pub(crate) enum Root {
    Slot(Slot),
    /// A property of an object (`obj.Name(args)`; `""` is the default member).
    Prop {
        obj: ObjectRef,
        name: Rc<str>,
        args: Vec<Arg>,
    },
    /// A `Property Let`/`Set` in a standard module.
    Proc {
        module: Rc<Module>,
        group: ProcGroup,
        args: Vec<Arg>,
    },
}

#[derive(Clone)]
pub(crate) enum Step {
    Index(Vec<i32>),
    Field(Rc<str>),
}

/// An assignable location: a variable, an element or field inside one, or a property.
#[derive(Clone)]
pub(crate) struct Place {
    root: Root,
    path: Vec<Step>,
}

impl Place {
    fn slot(s: Slot) -> Place {
        Place {
            root: Root::Slot(s),
            path: Vec::new(),
        }
    }

    fn prop(obj: ObjectRef, name: &str, args: Vec<Arg>) -> Place {
        Place {
            root: Root::Prop {
                obj,
                name: Rc::from(name),
                args,
            },
            path: Vec::new(),
        }
    }

    fn with(mut self, step: Step) -> Place {
        self.path.push(step);
        self
    }
}

/// An argument for a VBA procedure: its value and, for variables, where to write `ByRef`
/// changes back.
pub(crate) struct ArgIn {
    pub name: Option<Rc<str>>,
    pub value: Value,
    pub place: Option<Place>,
}

impl ArgIn {
    pub fn value(value: Value) -> ArgIn {
        ArgIn {
            name: None,
            value,
            place: None,
        }
    }
}

enum Temp {
    None,
    For { to: Value, step: Value },
    Each { items: Vec<Value>, pos: usize },
    Case(Value),
}

enum WithTarget {
    Value(Value),
    Place(Place),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Handler {
    None,
    ResumeNext,
    GoTo(usize),
}

/// A procedure activation.
pub(crate) struct Frame {
    pub module: Rc<Module>,
    pub proc: Rc<Proc>,
    locals: HashMap<Rc<str>, Slot>,
    result: Option<Slot>,
    /// `Me` and the instance's variables, in a class module.
    me: Option<(ObjectRef, Rc<Scope>)>,
    temps: Vec<Temp>,
    withs: Vec<WithTarget>,
    handler: Handler,
    in_handler: bool,
    err_pc: usize,
    gosubs: Vec<usize>,
    pub erl: u32,
}

impl Frame {
    fn new(module: Rc<Module>, proc: Rc<Proc>, me: Option<(ObjectRef, Rc<Scope>)>) -> Frame {
        let temps = (0..proc.slots).map(|_| Temp::None).collect();
        Frame {
            module,
            proc,
            locals: HashMap::new(),
            result: None,
            me,
            temps,
            withs: Vec::new(),
            handler: Handler::None,
            in_handler: false,
            err_pc: 0,
            gosubs: Vec::new(),
            erl: 0,
        }
    }
}

enum Flow {
    Next,
    Jump(usize),
    Exit,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    Get,
    Call,
}

pub(crate) enum Binding {
    Slot(Slot),
    /// The function's own name inside it: the return value.
    Result(Slot),
    Const(Value),
    Proc(Rc<Module>, ProcGroup),
    Builtin(Rc<str>),
    Module(Rc<Module>),
    Enum,
    Declare(String),
    Host(ObjectRef),
    Unknown,
}

/// What a declared type name refers to.
pub(crate) enum Named {
    Udt(Rc<Module>, Rc<TypeDef>),
    Class(Rc<Module>),
    Collection,
    Dictionary,
    /// A user `Enum` or a library enumeration (`VbMsgBoxResult`, `XlDirection`): a Long.
    Enum,
    Other,
}

const VBA_LIBRARIES: &[&str] = &[
    "vba",
    "strings",
    "math",
    "datetime",
    "conversion",
    "information",
    "interaction",
    "financial",
    "filesystem",
];

fn as_instance(o: &ObjectRef) -> Option<&Instance> {
    o.as_any()?.downcast_ref::<Instance>()
}

fn index_of(v: &Value) -> R<i32> {
    let n = convert::to_i64(v)?;
    i32::try_from(n).map_err(|_| ScriptError::runtime(9))
}

fn navigate<'v>(v: &'v Value, path: &[Step]) -> R<&'v Value> {
    let mut cur = v;
    for step in path {
        cur = match (step, cur) {
            (Step::Index(ix), Value::Array(a)) => {
                a.get(ix).ok_or_else(|| ScriptError::runtime(9))?
            }
            (Step::Field(n), Value::Record(r)) => {
                r.field(n).ok_or_else(|| ScriptError::runtime(438))?
            }
            (Step::Index(_), _) => return Err(ScriptError::runtime(13)),
            (Step::Field(_), _) => return Err(ScriptError::runtime(424)),
        };
    }
    Ok(cur)
}

fn navigate_mut<'v>(v: &'v mut Value, path: &[Step]) -> R<&'v mut Value> {
    let mut cur = v;
    for step in path {
        cur = match (step, cur) {
            (Step::Index(ix), Value::Array(a)) => Rc::make_mut(a)
                .get_mut(ix)
                .ok_or_else(|| ScriptError::runtime(9))?,
            (Step::Field(n), Value::Record(r)) => Rc::make_mut(r)
                .field_mut(n)
                .ok_or_else(|| ScriptError::runtime(438))?,
            (Step::Index(_), _) => return Err(ScriptError::runtime(13)),
            (Step::Field(_), _) => return Err(ScriptError::runtime(424)),
        };
    }
    Ok(cur)
}

pub(crate) fn scalar_of(ty: &Ty) -> Option<Scalar> {
    Some(match ty {
        Ty::Boolean => Scalar::Boolean,
        Ty::Byte => Scalar::Byte,
        Ty::Integer => Scalar::Integer,
        Ty::Long => Scalar::Long,
        Ty::LongLong => Scalar::LongLong,
        Ty::Single => Scalar::Single,
        Ty::Double => Scalar::Double,
        Ty::Currency => Scalar::Currency,
        Ty::Date => Scalar::Date,
        Ty::String | Ty::FixedString(_) => Scalar::String,
        _ => return None,
    })
}

fn fixed_string(s: String, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    let len = out.chars().count();
    out.extend(std::iter::repeat_n(' ', n - len));
    out
}

pub(crate) struct Interp<'a> {
    pub modules: &'a [Rc<Module>],
    pub st: &'a mut State,
    pub host: &'a mut dyn Host,
    pub depth: usize,
}

impl Interp<'_> {
    // ---- Budget ----

    fn tick(&mut self) -> R<()> {
        self.st.steps += 1;
        if let Some(max) = self.st.max_steps
            && self.st.steps > max
        {
            return Err(ScriptError::interrupted(
                18,
                format!("The macro was stopped after {max} steps"),
            ));
        }
        if self.st.steps.is_multiple_of(1024) && self.host.cancelled() {
            return Err(ScriptError::interrupted(
                18,
                "Code execution has been interrupted",
            ));
        }
        Ok(())
    }

    // ---- Modules ----

    pub fn module_named(&self, key: &str) -> Option<Rc<Module>> {
        self.modules.iter().find(|m| &*m.key == key).cloned()
    }

    fn module_frame(&self, m: &Rc<Module>) -> Frame {
        Frame::new(m.clone(), m.init.clone(), None)
    }

    /// The variables of a standard or document module, created on first use.
    fn module_scope(&mut self, m: &Rc<Module>) -> R<Rc<Scope>> {
        if self.st.scopes.len() <= m.idx {
            self.st.scopes.resize(m.idx + 1, None);
        }
        if let Some(s) = &self.st.scopes[m.idx] {
            return Ok(s.clone());
        }
        let scope = Rc::new(Scope::default());
        self.st.scopes[m.idx] = Some(scope.clone());
        let mut frame = self.module_frame(m);
        for d in &m.vars {
            let slot = self.make_var(d, &mut frame)?;
            scope.borrow_mut().insert(d.name.key.clone(), slot);
        }
        Ok(scope)
    }

    /// The constants and enum members of a module, evaluated on first use.
    fn module_consts(&mut self, m: &Rc<Module>) -> R<ConstMap> {
        if self.st.consts.len() <= m.idx {
            self.st.consts.resize(m.idx + 1, None);
        }
        if let Some(c) = &self.st.consts[m.idx] {
            return Ok(c.clone());
        }
        let map = Rc::new(RefCell::new(HashMap::new()));
        self.st.consts[m.idx] = Some(map.clone());
        let mut frame = self.module_frame(m);
        for e in &m.enums {
            let mut next = Value::Long(0);
            for (name, expr) in &e.members {
                let v = match expr {
                    Some(x) => convert::convert(&self.eval_value(x, &mut frame)?, Scalar::Long)?,
                    None => next.clone(),
                };
                next = convert::arith(Arith::Add, &v, &Value::Long(1))?;
                map.borrow_mut().insert(name.key.clone(), (v, e.public));
            }
        }
        for (public, c) in &m.consts {
            let v = self.eval_value(&c.value, &mut frame)?;
            let v = self.coerce(v, &c.ty, false)?;
            map.borrow_mut().insert(c.name.key.clone(), (v, *public));
        }
        Ok(map)
    }

    fn static_scope(&mut self, m: &Module, p: &Proc) -> Rc<Scope> {
        let key = (m.idx, p.name.key.clone(), p.kind as u8);
        self.st.statics.entry(key).or_default().clone()
    }

    /// What a declared type name refers to.
    pub fn resolve_named(&self, name: &str) -> Named {
        match name {
            "collection" | "vba.collection" => return Named::Collection,
            "dictionary" | "scripting.dictionary" => return Named::Dictionary,
            _ => {}
        }
        let short = name.rsplit('.').next().unwrap_or(name);
        for m in self.modules {
            if let Some(t) = m.types.get(short) {
                return Named::Udt(m.clone(), t.clone());
            }
        }
        for m in self.modules {
            if &*m.key == short && matches!(m.kind, ModuleKind::Class | ModuleKind::Form) {
                return Named::Class(m.clone());
            }
        }
        if self
            .modules
            .iter()
            .any(|m| m.enums.iter().any(|e| &*e.name.key == short))
            || ["vb", "xl", "mso"].iter().any(|p| short.starts_with(p))
        {
            return Named::Enum;
        }
        Named::Other
    }

    // ---- Variables ----

    pub fn default_value(&mut self, ty: &Ty) -> R<Value> {
        Ok(match ty {
            Ty::Variant => Value::Empty,
            Ty::Boolean => Value::Boolean(false),
            Ty::Byte => Value::Byte(0),
            Ty::Integer => Value::Integer(0),
            Ty::Long => Value::Long(0),
            Ty::LongLong => Value::LongLong(0),
            Ty::Single => Value::Single(0.0),
            Ty::Double => Value::Double(0.0),
            Ty::Currency => Value::Currency(0),
            Ty::Date => Value::Date(0.0),
            Ty::String => Value::String(String::new()),
            Ty::FixedString(n) => Value::String(" ".repeat(*n)),
            Ty::Object => Value::Object(None),
            Ty::Array(_) => Value::array(Array::unallocated()),
            Ty::Named(n) => match self.resolve_named(n) {
                Named::Udt(m, t) => self.new_record(&m, &t)?,
                Named::Enum => Value::Long(0),
                _ => Value::Object(None),
            },
        })
    }

    fn new_record(&mut self, m: &Rc<Module>, t: &TypeDef) -> R<Value> {
        if self.depth > self.st.max_depth {
            return Err(ScriptError::runtime(28));
        }
        self.depth += 1;
        let mut frame = self.module_frame(m);
        let mut fields = Vec::with_capacity(t.fields.len());
        for d in &t.fields {
            let slot = self.make_var(d, &mut frame);
            match slot {
                Ok(s) => {
                    let v = std::mem::take(&mut s.borrow_mut().value);
                    fields.push((d.name.text.to_string(), v));
                }
                Err(e) => {
                    self.depth -= 1;
                    return Err(e);
                }
            }
        }
        self.depth -= 1;
        Ok(Value::Record(Rc::new(Record {
            type_name: t.name.text.to_string(),
            fields,
        })))
    }

    fn eval_bounds(&mut self, bounds: &[Bounds], f: &mut Frame) -> R<Vec<(i32, i32)>> {
        let mut out = Vec::with_capacity(bounds.len());
        for b in bounds {
            let lower = match &b.lower {
                Some(e) => index_of(&self.eval_value(e, f)?)?,
                None => f.module.base,
            };
            let upper = index_of(&self.eval_value(&b.upper, f)?)?;
            out.push((lower, upper));
        }
        Ok(out)
    }

    fn alloc_array(&mut self, bounds: &[(i32, i32)], elem: &Ty) -> R<Value> {
        let fill = self.default_value(elem)?;
        Ok(Value::array(Array::new(bounds, fill)?))
    }

    fn make_var(&mut self, d: &VarDecl, f: &mut Frame) -> R<Slot> {
        let (ty, value, fixed) = match &d.array {
            None => (d.ty.clone(), self.default_value(&d.ty)?, false),
            Some(b) if b.is_empty() => (
                Ty::Array(Box::new(d.ty.clone())),
                Value::array(Array::unallocated()),
                false,
            ),
            Some(b) => {
                let bounds = self.eval_bounds(b, f)?;
                (
                    Ty::Array(Box::new(d.ty.clone())),
                    self.alloc_array(&bounds, &d.ty)?,
                    true,
                )
            }
        };
        let auto_new = match (&d.ty, d.auto_new) {
            (Ty::Named(n), true) => Some(n.clone()),
            _ => None,
        };
        Ok(Rc::new(RefCell::new(Var {
            value,
            ty,
            fixed,
            auto_new,
        })))
    }

    fn read_slot(&mut self, s: &Slot) -> R<Value> {
        let auto = {
            let v = s.borrow();
            match (&v.value, &v.auto_new) {
                (Value::Object(None), Some(class)) => Some(class.clone()),
                _ => return Ok(v.value.clone()),
            }
        };
        if let Some(class) = auto {
            let obj = self.new_object(&class)?;
            s.borrow_mut().value = obj.clone();
            return Ok(obj);
        }
        Ok(Value::Empty)
    }

    /// Converts a value for a variable of type `ty` (Let: plain values; Set: objects).
    pub fn coerce(&mut self, value: Value, ty: &Ty, set: bool) -> R<Value> {
        if set {
            return match ty {
                Ty::Variant | Ty::Object => Ok(value),
                Ty::Named(n) => match self.resolve_named(n) {
                    Named::Udt(..) | Named::Enum => Err(ScriptError::runtime(13)),
                    _ => Ok(value),
                },
                _ => Err(ScriptError::runtime(13)),
            };
        }
        match ty {
            Ty::Variant => Ok(value),
            Ty::FixedString(n) => Ok(Value::String(fixed_string(convert::to_str(&value)?, *n))),
            Ty::Object => Err(ScriptError::runtime(91)),
            Ty::Named(n) => match self.resolve_named(n) {
                Named::Udt(_, t) => match &value {
                    Value::Record(r) if r.type_name.eq_ignore_ascii_case(&t.name.text) => Ok(value),
                    _ => Err(ScriptError::runtime(13)),
                },
                Named::Enum => convert::convert(&value, Scalar::Long),
                _ => {
                    if value.is_object() {
                        Ok(value)
                    } else {
                        Err(ScriptError::runtime(91))
                    }
                }
            },
            Ty::Array(elem) => match value {
                Value::Array(mut a) => {
                    if let Some(s) = scalar_of(elem) {
                        let arr = Rc::make_mut(&mut a);
                        for v in arr.values_mut() {
                            *v = convert::convert(v, s)?;
                        }
                    }
                    Ok(Value::Array(a))
                }
                _ => Err(ScriptError::runtime(13)),
            },
            other => match scalar_of(other) {
                Some(s) => {
                    if value.is_object() {
                        return Err(ScriptError::runtime(13));
                    }
                    convert::convert(&value, s)
                }
                None => Ok(value),
            },
        }
    }

    fn assign_slot(&mut self, s: &Slot, value: Value, set: bool) -> R<()> {
        let (ty, fixed) = {
            let v = s.borrow();
            (v.ty.clone(), v.fixed)
        };
        if fixed {
            return Err(ScriptError::syntax("Can't assign to array"));
        }
        let value = self.coerce(value, &ty, set)?;
        let old = std::mem::replace(&mut s.borrow_mut().value, value);
        self.release(old);
        Ok(())
    }

    /// Runs `Class_Terminate` when the last reference to a class instance goes away.
    pub fn release(&mut self, v: Value) {
        let Value::Object(Some(o)) = v else {
            return;
        };
        if Rc::strong_count(&o) != 1 {
            return;
        }
        let Some(inst) = as_instance(&o) else {
            return;
        };
        let module = inst.module.clone();
        let fields = inst.fields.clone();
        let Some(p) = module
            .procs
            .get("class_terminate")
            .and_then(|g| g.sub.clone())
        else {
            return;
        };
        // Errors in Class_Terminate are ignored, as VBA reports them only in the debugger.
        let _ = self.call_proc(&module, &p, Vec::new(), Some((o.clone(), fields)), None);
    }

    fn release_frame(&mut self, mut f: Frame) {
        f.withs.clear();
        for (_, slot) in f.locals.drain() {
            if Rc::strong_count(&slot) == 1 {
                let v = std::mem::take(&mut slot.borrow_mut().value);
                self.release(v);
            }
        }
    }

    // ---- Name resolution ----

    fn implicit_local(&mut self, id: &Ident, f: &mut Frame) -> R<Slot> {
        if f.module.explicit {
            return Err(ScriptError::syntax(format!(
                "Variable not defined: {}",
                id.text
            )));
        }
        let ty = f.module.deftype(&id.text);
        let value = self.default_value(&ty)?;
        let slot = new_slot(value, ty);
        f.locals.insert(id.key.clone(), slot.clone());
        Ok(slot)
    }

    /// A member of module `m` (`inside`: looked up from code in that module).
    fn module_member(
        &mut self,
        m: &Rc<Module>,
        key: &str,
        f: &Frame,
        inside: bool,
    ) -> R<Option<Binding>> {
        let scope = match m.kind {
            ModuleKind::Class | ModuleKind::Form => {
                if inside {
                    f.me.as_ref().map(|(_, s)| s.clone())
                } else {
                    None
                }
            }
            _ => Some(self.module_scope(m)?),
        };
        if let Some(scope) = scope
            && (inside || m.public_vars.contains(key))
            && let Some(s) = scope.borrow().get(key)
        {
            return Ok(Some(Binding::Slot(s.clone())));
        }
        let consts = self.module_consts(m)?;
        if let Some((v, public)) = consts.borrow().get(key)
            && (inside || *public)
        {
            return Ok(Some(Binding::Const(v.clone())));
        }
        if let Some(g) = m.procs.get(key)
            && (inside || g.is_public())
        {
            return Ok(Some(Binding::Proc(m.clone(), g.clone())));
        }
        if let Some(lib) = m.declares.get(key) {
            return Ok(Some(Binding::Declare(lib.clone())));
        }
        Ok(None)
    }

    pub fn lookup(&mut self, id: &Ident, f: &Frame) -> R<Binding> {
        let key = &*id.key;
        if let Some(s) = f.locals.get(key) {
            return Ok(Binding::Slot(s.clone()));
        }
        if let Some(r) = &f.result
            && f.proc.name.key == id.key
        {
            return Ok(Binding::Result(r.clone()));
        }
        let current = f.module.clone();
        if let Some(b) = self.module_member(&current, key, f, true)? {
            return Ok(b);
        }
        let modules = self.modules;
        for m in modules {
            if m.idx != current.idx
                && m.kind == ModuleKind::Standard
                && let Some(b) = self.module_member(m, key, f, false)?
            {
                return Ok(b);
            }
        }
        if let Some(m) = self.module_named(key) {
            return Ok(Binding::Module(m));
        }
        if modules
            .iter()
            .any(|m| m.enums.iter().any(|e| e.name.key == id.key))
        {
            return Ok(Binding::Enum);
        }
        if builtins::is_builtin(key) {
            return Ok(Binding::Builtin(id.key.clone()));
        }
        if let Some(v) = builtins::constant(key) {
            return Ok(Binding::Const(v));
        }
        if let Some(v) = self.st.constants.get(key) {
            return Ok(Binding::Const(v.clone()));
        }
        if let Some(v) = self.host.constant(&id.text) {
            return Ok(Binding::Const(v));
        }
        if let Some(g) = self.host.globals() {
            return Ok(Binding::Host(g));
        }
        Ok(Binding::Unknown)
    }

    // ---- Expressions ----

    pub fn eval(&mut self, e: &Expr, f: &mut Frame) -> R<Value> {
        match e {
            Expr::Lit(v) => Ok(v.clone()),
            Expr::Name(id) => self.eval_name(id, f),
            Expr::Member(base, name) => self.member(base, name, &[], f, Access::Get),
            Expr::Bang(base, key) => {
                let obj = self.eval(base, f)?;
                self.index_value(obj, vec![Arg::new(key.text.to_string())], Access::Get)
            }
            Expr::Call(target, args) => self.eval_call(target, args, f, Access::Get),
            Expr::Paren(x) => self.eval(x, f),
            Expr::Unary(UnOp::Neg, x) => {
                let v = self.eval_value(x, f)?;
                convert::negate(&v)
            }
            Expr::Unary(UnOp::Not, x) => {
                let v = self.eval_value(x, f)?;
                convert::not(&v)
            }
            Expr::Binary(BinOp::Is, a, b) => {
                let a = self.eval(a, f)?;
                let b = self.eval(b, f)?;
                match (a, b) {
                    (Value::Object(x), Value::Object(y)) => Ok(Value::Boolean(match (x, y) {
                        (None, None) => true,
                        (Some(x), Some(y)) => Value::same_object(&x, &y),
                        _ => false,
                    })),
                    _ => Err(ScriptError::runtime(424)),
                }
            }
            Expr::Binary(op, a, b) => {
                let a = self.eval_value(a, f)?;
                let b = self.eval_value(b, f)?;
                convert::binary(*op, &a, &b, f.module.compare_text)
            }
            Expr::New(name) => self.new_object(name),
            Expr::TypeOf(x, name) => {
                let v = self.eval(x, f)?;
                let short = name.rsplit('.').next().unwrap_or(name);
                match v {
                    Value::Object(None) => Ok(Value::Boolean(false)),
                    Value::Object(Some(o)) => Ok(Value::Boolean(
                        short == "object" || o.type_name().eq_ignore_ascii_case(short),
                    )),
                    _ => Err(ScriptError::runtime(424)),
                }
            }
            Expr::With(depth) => match f.withs.get(*depth) {
                Some(WithTarget::Value(v)) => Ok(v.clone()),
                Some(WithTarget::Place(p)) => {
                    let p = p.clone();
                    self.read_place(&p)
                }
                None => Err(ScriptError::runtime(91)),
            },
            Expr::Me => self.me(f),
            Expr::Evaluate(text) => {
                let g = self
                    .host
                    .globals()
                    .ok_or_else(|| ScriptError::unsupported("[ ] evaluation"))?;
                g.call("Evaluate", &[Arg::new(text.as_str())])
            }
        }
    }

    fn me(&mut self, f: &Frame) -> R<Value> {
        if let Some((o, _)) = &f.me {
            return Ok(Value::Object(Some(o.clone())));
        }
        if f.module.kind == ModuleKind::Document
            && let Some(g) = self.host.globals()
        {
            return g.get(&f.module.name, &[]);
        }
        Err(ScriptError::syntax("Invalid use of Me keyword"))
    }

    /// Evaluates and replaces an object by its default member (MS-VBAL §5.6.2.3, Let-coercion
    /// of objects): what operators and `Let` assignments see.
    pub fn eval_value(&mut self, e: &Expr, f: &mut Frame) -> R<Value> {
        let v = self.eval(e, f)?;
        self.deref(v)
    }

    pub fn deref(&mut self, mut v: Value) -> R<Value> {
        for _ in 0..8 {
            match v {
                Value::Object(Some(o)) => v = self.obj_get(&o, "", Vec::new(), Access::Get)?,
                Value::Object(None) => return Err(ScriptError::runtime(91)),
                other => return Ok(other),
            }
        }
        Err(ScriptError::runtime(13))
    }

    fn eval_name(&mut self, id: &Ident, f: &mut Frame) -> R<Value> {
        match self.lookup(id, f)? {
            Binding::Slot(s) | Binding::Result(s) => self.read_slot(&s),
            Binding::Const(v) => Ok(v),
            Binding::Proc(m, g) => {
                let me = self.me_for(&m, f);
                self.call_group(&m, &g, Vec::new(), me, Access::Get)
            }
            Binding::Builtin(k) => self.builtin(&k, Vec::new(), f),
            Binding::Module(_) | Binding::Enum => Err(ScriptError::syntax(format!(
                "Expected variable or procedure, not '{}'",
                id.text
            ))),
            Binding::Declare(lib) => Err(ScriptError::unsupported(&format!(
                "Calling the DLL function {} (\"{}\")",
                id.text, lib
            ))),
            Binding::Host(g) => match g.get(&id.text, &[]) {
                Err(e) if e.number == 438 => {
                    let s = self.implicit_local(id, f)?;
                    self.read_slot(&s)
                }
                r => r,
            },
            Binding::Unknown => {
                let s = self.implicit_local(id, f)?;
                self.read_slot(&s)
            }
        }
    }

    /// `Me` for an unqualified call to a procedure of module `m`.
    fn me_for(&self, m: &Module, f: &Frame) -> Option<(ObjectRef, Rc<Scope>)> {
        if m.idx == f.module.idx {
            f.me.clone()
        } else {
            None
        }
    }

    /// Evaluates arguments for a host object or builtin: values only, objects kept.
    fn eval_args(&mut self, args: &[ArgExpr], f: &mut Frame) -> R<Vec<Arg>> {
        let mut out = Vec::with_capacity(args.len());
        for a in args {
            let value = match &a.value {
                Some(e) => self.eval(e, f)?,
                None => Value::Missing,
            };
            out.push(Arg {
                name: a.name.as_ref().map(|n| n.text.to_string()),
                value,
            });
        }
        Ok(out)
    }

    /// Evaluates arguments for a VBA procedure, keeping where variables live for `ByRef`.
    fn eval_args_in(&mut self, args: &[ArgExpr], f: &mut Frame) -> R<Vec<ArgIn>> {
        let mut out = Vec::with_capacity(args.len());
        for a in args {
            let name = a.name.as_ref().map(|n| n.key.clone());
            let (value, place) = match &a.value {
                None => (Value::Missing, None),
                Some(e) => match self.try_place(e, f)? {
                    Some(p) => (self.read_place(&p)?, Some(p)),
                    None => (self.eval(e, f)?, None),
                },
            };
            out.push(ArgIn { name, value, place });
        }
        Ok(out)
    }

    fn indices(&mut self, args: &[Arg]) -> R<Vec<i32>> {
        let mut out = Vec::with_capacity(args.len());
        for a in args {
            let v = self.deref(a.value.clone())?;
            out.push(index_of(&v)?);
        }
        Ok(out)
    }

    /// `value(args)`: an array element, or the default member of an object.
    fn index_value(&mut self, v: Value, args: Vec<Arg>, access: Access) -> R<Value> {
        if args.is_empty() {
            return Ok(v);
        }
        match v {
            Value::Array(a) => {
                let ix = self.indices(&args)?;
                a.get(&ix).cloned().ok_or_else(|| ScriptError::runtime(9))
            }
            Value::Object(Some(o)) => self.obj_get(&o, "", args, access),
            Value::Object(None) => Err(ScriptError::runtime(91)),
            _ => Err(ScriptError::runtime(13)),
        }
    }

    pub fn eval_call(
        &mut self,
        target: &Expr,
        args: &[ArgExpr],
        f: &mut Frame,
        access: Access,
    ) -> R<Value> {
        match target {
            Expr::Name(id) => match self.lookup(id, f)? {
                Binding::Slot(s) => {
                    let v = self.read_slot(&s)?;
                    let a = self.eval_args(args, f)?;
                    self.index_value(v, a, access)
                }
                Binding::Result(s) => {
                    if args.is_empty() && access == Access::Get {
                        return self.read_slot(&s);
                    }
                    // `Foo(x)` inside Function Foo is a recursive call.
                    let m = f.module.clone();
                    let g = m.procs.get(&id.key).cloned().unwrap_or_default();
                    let a = self.eval_args_in(args, f)?;
                    let me = f.me.clone();
                    self.call_group(&m, &g, a, me, access)
                }
                Binding::Const(v) => {
                    let a = self.eval_args(args, f)?;
                    self.index_value(v, a, access)
                }
                Binding::Proc(m, g) => {
                    let a = self.eval_args_in(args, f)?;
                    let me = self.me_for(&m, f);
                    self.call_group(&m, &g, a, me, access)
                }
                Binding::Builtin(k) => {
                    let a = self.builtin_args(&k, args, f)?;
                    self.builtin(&k, a, f)
                }
                Binding::Declare(lib) => Err(ScriptError::unsupported(&format!(
                    "Calling the DLL function {} (\"{}\")",
                    id.text, lib
                ))),
                Binding::Module(_) | Binding::Enum => Err(ScriptError::syntax(format!(
                    "Expected procedure, not '{}'",
                    id.text
                ))),
                Binding::Host(g) => {
                    let a = self.eval_args(args, f)?;
                    let r = if access == Access::Call {
                        g.call(&id.text, &a)
                    } else {
                        g.get(&id.text, &a)
                    };
                    match r {
                        Err(e) if e.number == 438 => Err(ScriptError::new(
                            35,
                            format!("Sub or Function not defined: {}", id.text),
                        )),
                        r => r,
                    }
                }
                Binding::Unknown => Err(ScriptError::new(
                    35,
                    format!("Sub or Function not defined: {}", id.text),
                )),
            },
            Expr::Member(base, name) => self.member(base, name, args, f, access),
            other => {
                let v = self.eval(other, f)?;
                let a = self.eval_args(args, f)?;
                self.index_value(v, a, access)
            }
        }
    }

    /// `base.name(args)`.
    fn member(
        &mut self,
        base: &Expr,
        name: &Ident,
        args: &[ArgExpr],
        f: &mut Frame,
        access: Access,
    ) -> R<Value> {
        if let Expr::Name(id) = base {
            match self.lookup(id, f)? {
                Binding::Module(m) => return self.module_qualified(&m, name, args, f, access),
                Binding::Enum => {
                    let modules = self.modules;
                    for m in modules {
                        if m.enums.iter().any(|e| e.name.key == id.key) {
                            let consts = self.module_consts(m)?;
                            if let Some((v, _)) = consts.borrow().get(&name.key) {
                                return Ok(v.clone());
                            }
                        }
                    }
                    return Err(ScriptError::syntax(format!(
                        "Method or data member not found: {}",
                        name.text
                    )));
                }
                Binding::Unknown | Binding::Host(_) if VBA_LIBRARIES.contains(&&*id.key) => {
                    if builtins::is_builtin(&name.key) {
                        let a = self.builtin_args(&name.key, args, f)?;
                        return self.builtin(&name.key, a, f);
                    }
                    if let Some(v) = builtins::constant(&name.key) {
                        return Ok(v);
                    }
                }
                _ => {}
            }
        }
        let obj = self.eval(base, f)?;
        match obj {
            Value::Object(Some(o)) => {
                if as_instance(&o).is_some() {
                    let a = self.eval_args_in(args, f)?;
                    return self.instance_member(&o, &name.key, a, access);
                }
                let a = self.eval_args(args, f)?;
                if access == Access::Call {
                    o.call(&name.text, &a)
                } else {
                    o.get(&name.text, &a)
                }
            }
            Value::Object(None) => Err(ScriptError::runtime(91)),
            Value::Record(r) => {
                let field = r.field(&name.text).cloned().ok_or_else(|| {
                    ScriptError::syntax(format!("Method or data member not found: {}", name.text))
                })?;
                let a = self.eval_args(args, f)?;
                self.index_value(field, a, access)
            }
            _ => Err(ScriptError::runtime(424)),
        }
    }

    fn module_qualified(
        &mut self,
        m: &Rc<Module>,
        name: &Ident,
        args: &[ArgExpr],
        f: &mut Frame,
        access: Access,
    ) -> R<Value> {
        if matches!(m.kind, ModuleKind::Class | ModuleKind::Form) {
            return Err(ScriptError::runtime(424));
        }
        let inside = m.idx == f.module.idx;
        match self.module_member(m, &name.key, f, inside)? {
            Some(Binding::Slot(s)) => {
                let v = self.read_slot(&s)?;
                let a = self.eval_args(args, f)?;
                self.index_value(v, a, access)
            }
            Some(Binding::Const(v)) => Ok(v),
            Some(Binding::Proc(pm, g)) => {
                let a = self.eval_args_in(args, f)?;
                self.call_group(&pm, &g, a, None, access)
            }
            Some(Binding::Declare(lib)) => Err(ScriptError::unsupported(&format!(
                "Calling the DLL function {} (\"{}\")",
                name.text, lib
            ))),
            _ => Err(ScriptError::syntax(format!(
                "Method or data member not found: {}.{}",
                m.name, name.text
            ))),
        }
    }

    /// A member of a class instance, from outside it (public members only).
    fn instance_member(
        &mut self,
        o: &ObjectRef,
        key: &str,
        args: Vec<ArgIn>,
        access: Access,
    ) -> R<Value> {
        let Some(inst) = as_instance(o) else {
            return Err(ScriptError::runtime(438));
        };
        let module = inst.module.clone();
        let fields = inst.fields.clone();
        let key: Rc<str> = if key.is_empty() {
            match &module.default_member {
                Some(d) => d.clone(),
                None => return Err(ScriptError::no_member(&module.name, "")),
            }
        } else {
            Rc::from(key)
        };
        if let Some(g) = module.procs.get(&key)
            && g.is_public()
        {
            let g = g.clone();
            return self.call_group(&module, &g, args, Some((o.clone(), fields)), access);
        }
        if module.public_vars.contains(&key) {
            let slot = fields.borrow().get(&key).cloned();
            if let Some(s) = slot {
                let v = self.read_slot(&s)?;
                let a = args
                    .into_iter()
                    .map(|a| Arg {
                        name: None,
                        value: a.value,
                    })
                    .collect();
                return self.index_value(v, a, access);
            }
        }
        Err(ScriptError::no_member(&module.name, &key))
    }

    /// Reads a property of any object (VBA instances included).
    pub fn obj_get(
        &mut self,
        o: &ObjectRef,
        name: &str,
        args: Vec<Arg>,
        access: Access,
    ) -> R<Value> {
        if as_instance(o).is_some() {
            let a = args
                .into_iter()
                .map(|a| ArgIn {
                    name: a.name.map(|n| Rc::from(n.to_lowercase())),
                    value: a.value,
                    place: None,
                })
                .collect();
            return self.instance_member(o, &name.to_lowercase(), a, access);
        }
        if access == Access::Call {
            o.call(name, &args)
        } else {
            o.get(name, &args)
        }
    }

    /// Writes a property of any object (VBA instances included).
    pub fn obj_put(
        &mut self,
        o: &ObjectRef,
        name: &str,
        args: Vec<Arg>,
        value: Value,
        set: bool,
    ) -> R<()> {
        let Some(inst) = as_instance(o) else {
            return if set {
                o.put_ref(name, &args, value)
            } else {
                o.put(name, &args, value)
            };
        };
        let module = inst.module.clone();
        let fields = inst.fields.clone();
        let key: Rc<str> = if name.is_empty() {
            match &module.default_member {
                Some(d) => d.clone(),
                None => return Err(ScriptError::no_member(&module.name, "")),
            }
        } else {
            Rc::from(name.to_lowercase())
        };
        if let Some(g) = module.procs.get(&key).filter(|g| g.is_public()).cloned() {
            let a = args.into_iter().map(|a| ArgIn::value(a.value)).collect();
            return self.call_property_put(&module, &g, a, value, set, Some((o.clone(), fields)));
        }
        if module.public_vars.contains(&key) {
            let slot = fields.borrow().get(&key).cloned();
            if let Some(s) = slot {
                if args.is_empty() {
                    return self.assign_slot(&s, value, set);
                }
                let ix = self.indices(&args)?;
                return self.write_place(&Place::slot(s).with(Step::Index(ix)), value, set);
            }
        }
        Err(ScriptError::no_member(&module.name, &key))
    }

    fn call_property_put(
        &mut self,
        m: &Rc<Module>,
        g: &ProcGroup,
        args: Vec<ArgIn>,
        value: Value,
        set: bool,
        me: Option<(ObjectRef, Rc<Scope>)>,
    ) -> R<()> {
        let p = if set {
            g.set.clone().or_else(|| g.let_.clone())
        } else {
            g.let_.clone().or_else(|| g.set.clone())
        };
        let Some(p) = p else {
            return Err(ScriptError::runtime(451));
        };
        self.call_proc(m, &p, args, me, Some(value))?;
        Ok(())
    }

    // ---- Objects ----

    pub fn new_object(&mut self, name: &str) -> R<Value> {
        match self.resolve_named(name) {
            Named::Class(m) => self.instantiate(&m),
            Named::Collection => Ok(Value::object(Rc::new(Collection::new()))),
            Named::Dictionary => Ok(Value::object(Rc::new(Dictionary::new()))),
            Named::Udt(..) | Named::Enum => Err(ScriptError::syntax("Invalid use of New keyword")),
            Named::Other => match self.host.create_object(name) {
                Some(r) => Ok(Value::Object(Some(r?))),
                None => Err(ScriptError {
                    number: 429,
                    ..ScriptError::unsupported(&format!("Creating '{name}' objects"))
                }),
            },
        }
    }

    pub fn create_object(&mut self, prog_id: &str) -> R<Value> {
        let lower = prog_id.to_lowercase();
        match lower.as_str() {
            "scripting.dictionary" => return Ok(Value::object(Rc::new(Dictionary::new()))),
            "vba.collection" => return Ok(Value::object(Rc::new(Collection::new()))),
            _ => {}
        }
        match self.host.create_object(prog_id) {
            Some(r) => Ok(Value::Object(Some(r?))),
            None => Err(ScriptError {
                number: 429,
                ..ScriptError::unsupported(&format!("CreateObject(\"{prog_id}\")"))
            }),
        }
    }

    fn instantiate(&mut self, m: &Rc<Module>) -> R<Value> {
        if m.kind == ModuleKind::Form {
            return Err(ScriptError::unsupported(&format!("UserForms ({})", m.name)));
        }
        let fields = Rc::new(Scope::default());
        let mut frame = self.module_frame(m);
        for d in &m.vars {
            let slot = self.make_var(d, &mut frame)?;
            fields.borrow_mut().insert(d.name.key.clone(), slot);
        }
        let obj: ObjectRef = Rc::new(Instance {
            module: m.clone(),
            fields: fields.clone(),
        });
        if let Some(p) = m.procs.get("class_initialize").and_then(|g| g.sub.clone()) {
            self.call_proc(m, &p, Vec::new(), Some((obj.clone(), fields)), None)?;
        }
        Ok(Value::Object(Some(obj)))
    }

    /// The items `For Each` visits.
    pub fn enumerate(&mut self, v: Value) -> R<Vec<Value>> {
        match v {
            Value::Array(a) => Ok(a.values().to_vec()),
            Value::Object(Some(o)) => {
                if let Some(inst) = as_instance(&o) {
                    let module = inst.module.clone();
                    let Some(key) = module.enumerator.clone() else {
                        return Err(ScriptError::runtime(438));
                    };
                    let e = self.instance_member(&o, &key, Vec::new(), Access::Get)?;
                    return self.enumerate(e);
                }
                o.enumerate()
            }
            Value::Object(None) => Err(ScriptError::runtime(91)),
            _ => Err(ScriptError::runtime(424)),
        }
    }

    // ---- Places ----

    /// The place an argument expression names, if it's a variable, an array element or a
    /// field of a user-defined type: those are passed `ByRef` for real.
    fn try_place(&mut self, e: &Expr, f: &mut Frame) -> R<Option<Place>> {
        self.place_of(e, f, true)
    }

    fn place_of(&mut self, e: &Expr, f: &mut Frame, implicit: bool) -> R<Option<Place>> {
        match e {
            Expr::Name(id) => Ok(match self.lookup(id, f)? {
                Binding::Slot(s) | Binding::Result(s) => Some(Place::slot(s)),
                Binding::Unknown if implicit && !f.module.explicit => {
                    Some(Place::slot(self.implicit_local(id, f)?))
                }
                _ => None,
            }),
            Expr::Call(t, args) if args.iter().all(|a| a.value.is_some() && a.name.is_none()) => {
                let Some(p) = self.place_of(t, f, false)? else {
                    return Ok(None);
                };
                if !self.place_is(&p, |v| matches!(v, Value::Array(_)))? {
                    return Ok(None);
                }
                let a = self.eval_args(args, f)?;
                let ix = self.indices(&a)?;
                Ok(Some(p.with(Step::Index(ix))))
            }
            Expr::Member(base, name) => {
                if let Expr::Name(id) = &**base
                    && let Binding::Module(m) = self.lookup(id, f)?
                {
                    let inside = m.idx == f.module.idx;
                    return Ok(match self.module_member(&m, &name.key, f, inside)? {
                        Some(Binding::Slot(s)) => Some(Place::slot(s)),
                        _ => None,
                    });
                }
                let Some(p) = self.place_of(base, f, false)? else {
                    return Ok(None);
                };
                if self.place_is(&p, |v| matches!(v, Value::Record(_)))? {
                    Ok(Some(p.with(Step::Field(name.key.clone()))))
                } else {
                    Ok(None)
                }
            }
            Expr::With(depth) => Ok(match f.withs.get(*depth) {
                Some(WithTarget::Place(p)) => Some(p.clone()),
                _ => None,
            }),
            _ => Ok(None),
        }
    }

    fn place_is(&mut self, p: &Place, test: fn(&Value) -> bool) -> R<bool> {
        match &p.root {
            Root::Slot(s) => {
                let v = s.borrow();
                Ok(navigate(&v.value, &p.path).map(test).unwrap_or(false))
            }
            _ => Ok(false),
        }
    }

    pub fn read_place(&mut self, p: &Place) -> R<Value> {
        match &p.root {
            Root::Slot(s) => {
                if p.path.is_empty() {
                    return self.read_slot(s);
                }
                let v = s.borrow();
                navigate(&v.value, &p.path).cloned()
            }
            Root::Prop { obj, name, args } => {
                let v = self.obj_get(obj, name, args.clone(), Access::Get)?;
                navigate(&v, &p.path).cloned()
            }
            Root::Proc {
                module,
                group,
                args,
            } => {
                let a = args.iter().map(|a| ArgIn::value(a.value.clone())).collect();
                let v = self.call_group(module, group, a, None, Access::Get)?;
                navigate(&v, &p.path).cloned()
            }
        }
    }

    fn field_type(&self, type_name: &str, field: &str) -> Ty {
        if let Named::Udt(_, t) = self.resolve_named(&type_name.to_lowercase())
            && let Some(d) = t.fields.iter().find(|d| &*d.name.key == field)
        {
            return match &d.array {
                Some(_) => Ty::Array(Box::new(d.ty.clone())),
                None => d.ty.clone(),
            };
        }
        Ty::Variant
    }

    /// The declared type at a place inside a variable.
    fn place_type(&self, s: &Slot, path: &[Step]) -> Ty {
        let var = s.borrow();
        let mut ty = var.ty.clone();
        let mut cur = Some(&var.value);
        for step in path {
            ty = match (step, &ty) {
                (Step::Index(_), Ty::Array(elem)) => (**elem).clone(),
                (Step::Index(_), _) => Ty::Variant,
                (Step::Field(f), _) => match cur {
                    Some(Value::Record(r)) => self.field_type(&r.type_name, f),
                    _ => Ty::Variant,
                },
            };
            cur = cur.and_then(|v| navigate(v, std::slice::from_ref(step)).ok());
        }
        ty
    }

    pub fn write_place(&mut self, p: &Place, value: Value, set: bool) -> R<()> {
        match &p.root {
            Root::Slot(s) if p.path.is_empty() => self.assign_slot(s, value, set),
            Root::Slot(s) => {
                let ty = self.place_type(s, &p.path);
                let value = self.coerce(value, &ty, set)?;
                let old = {
                    let mut var = s.borrow_mut();
                    let target = navigate_mut(&mut var.value, &p.path)?;
                    std::mem::replace(target, value)
                };
                self.release(old);
                Ok(())
            }
            Root::Prop { obj, name, args } if p.path.is_empty() => {
                self.obj_put(obj, name, args.clone(), value, set)
            }
            Root::Prop { obj, name, args } => {
                let mut whole = self.obj_get(obj, name, args.clone(), Access::Get)?;
                *navigate_mut(&mut whole, &p.path)? = value;
                self.obj_put(obj, name, args.clone(), whole, false)
            }
            Root::Proc {
                module,
                group,
                args,
            } => {
                let a = args.iter().map(|a| ArgIn::value(a.value.clone())).collect();
                self.call_property_put(module, group, a, value, set, None)
            }
        }
    }

    /// The place an assignment target names.
    fn resolve_place(&mut self, e: &Expr, f: &mut Frame) -> R<Place> {
        match e {
            Expr::Name(id) => self.name_place(id, Vec::new(), f),
            Expr::Call(t, args) => match &**t {
                Expr::Name(id) => {
                    let a = self.eval_args(args, f)?;
                    self.name_place(id, a, f)
                }
                Expr::Member(base, name) => {
                    let a = self.eval_args(args, f)?;
                    self.member_place(base, name, a, f)
                }
                other => {
                    let inner = self.resolve_place(other, f)?;
                    let a = self.eval_args(args, f)?;
                    self.indexed_place(inner, a)
                }
            },
            Expr::Member(base, name) => self.member_place(base, name, Vec::new(), f),
            Expr::Bang(base, key) => {
                let obj = self.eval(base, f)?;
                match obj {
                    Value::Object(Some(o)) => {
                        Ok(Place::prop(o, "", vec![Arg::new(key.text.to_string())]))
                    }
                    Value::Object(None) => Err(ScriptError::runtime(91)),
                    _ => Err(ScriptError::runtime(424)),
                }
            }
            Expr::With(depth) => match f.withs.get(*depth) {
                Some(WithTarget::Place(p)) => Ok(p.clone()),
                _ => Err(ScriptError::syntax("Invalid assignment target")),
            },
            _ => Err(ScriptError::syntax("Invalid assignment target")),
        }
    }

    /// `inner(args)` as a place: an element when `inner` holds an array, else the default
    /// member of the object it holds.
    fn indexed_place(&mut self, inner: Place, args: Vec<Arg>) -> R<Place> {
        if args.is_empty() {
            return Ok(inner);
        }
        let current = self.read_place(&inner)?;
        match current {
            Value::Array(_) => {
                let ix = self.indices(&args)?;
                Ok(inner.with(Step::Index(ix)))
            }
            Value::Object(Some(o)) => Ok(Place::prop(o, "", args)),
            Value::Object(None) => Err(ScriptError::runtime(91)),
            _ => Err(ScriptError::runtime(13)),
        }
    }

    fn name_place(&mut self, id: &Ident, args: Vec<Arg>, f: &mut Frame) -> R<Place> {
        match self.lookup(id, f)? {
            Binding::Slot(s) | Binding::Result(s) => {
                if args.is_empty() {
                    return Ok(Place::slot(s));
                }
                let is_array = matches!(s.borrow().ty, Ty::Array(_));
                if is_array {
                    let ix = self.indices(&args)?;
                    return Ok(Place::slot(s).with(Step::Index(ix)));
                }
                self.indexed_place(Place::slot(s), args)
            }
            Binding::Proc(m, g) if g.let_.is_some() || g.set.is_some() => {
                if m.idx == f.module.idx
                    && let Some((o, _)) = &f.me
                {
                    return Ok(Place::prop(o.clone(), &id.key, args));
                }
                Ok(Place {
                    root: Root::Proc {
                        module: m,
                        group: g,
                        args,
                    },
                    path: Vec::new(),
                })
            }
            Binding::Builtin(k) if &*k == "err" => Ok(Place::prop(self.st.err.clone(), "", args)),
            Binding::Builtin(k) if matches!(&*k, "date" | "time" | "now") => {
                Err(ScriptError::unsupported("Setting the system date or time"))
            }
            Binding::Host(g) => {
                if args.is_empty() {
                    match g.get(&id.text, &[]) {
                        Err(e) if e.number == 438 => Ok(Place::slot(self.implicit_local(id, f)?)),
                        Err(e) => Err(e),
                        Ok(Value::Object(Some(o))) => Ok(Place::prop(o, "", Vec::new())),
                        Ok(_) => Ok(Place::prop(g, &id.text, Vec::new())),
                    }
                } else {
                    // `Cells(1, 2) = 5`: the default member of the object `Cells(1, 2)`.
                    match g.get(&id.text, &args)? {
                        Value::Object(Some(o)) => Ok(Place::prop(o, "", Vec::new())),
                        _ => Ok(Place::prop(g, &id.text, args)),
                    }
                }
            }
            Binding::Unknown => {
                let s = self.implicit_local(id, f)?;
                if args.is_empty() {
                    Ok(Place::slot(s))
                } else {
                    self.indexed_place(Place::slot(s), args)
                }
            }
            _ => Err(ScriptError::syntax(format!(
                "Can't assign to '{}'",
                id.text
            ))),
        }
    }

    fn member_place(
        &mut self,
        base: &Expr,
        name: &Ident,
        args: Vec<Arg>,
        f: &mut Frame,
    ) -> R<Place> {
        if let Expr::Name(id) = base
            && let Binding::Module(m) = self.lookup(id, f)?
        {
            let inside = m.idx == f.module.idx;
            return match self.module_member(&m, &name.key, f, inside)? {
                Some(Binding::Slot(s)) => self.indexed_place(Place::slot(s), args),
                Some(Binding::Proc(pm, g)) => Ok(Place {
                    root: Root::Proc {
                        module: pm,
                        group: g,
                        args,
                    },
                    path: Vec::new(),
                }),
                _ => Err(ScriptError::syntax(format!(
                    "Method or data member not found: {}",
                    name.text
                ))),
            };
        }
        if let Some(p) = self.place_of(base, f, false)?
            && self.place_is(&p, |v| matches!(v, Value::Record(_)))?
        {
            let p = p.with(Step::Field(name.key.clone()));
            return self.indexed_place(p, args);
        }
        let obj = self.eval(base, f)?;
        match obj {
            Value::Object(Some(o)) => Ok(Place::prop(o, &name.text, args)),
            Value::Object(None) => Err(ScriptError::runtime(91)),
            _ => Err(ScriptError::runtime(424)),
        }
    }

    // ---- Calls ----

    pub fn call_group(
        &mut self,
        m: &Rc<Module>,
        g: &ProcGroup,
        args: Vec<ArgIn>,
        me: Option<(ObjectRef, Rc<Scope>)>,
        _access: Access,
    ) -> R<Value> {
        let p = g.sub.clone().or_else(|| g.get.clone());
        match p {
            Some(p) => self.call_proc(m, &p, args, me, None),
            None => Err(ScriptError::runtime(451)),
        }
    }

    pub fn call_proc(
        &mut self,
        m: &Rc<Module>,
        p: &Rc<Proc>,
        args: Vec<ArgIn>,
        me: Option<(ObjectRef, Rc<Scope>)>,
        let_value: Option<Value>,
    ) -> R<Value> {
        if self.depth >= self.st.max_depth {
            return Err(ScriptError::runtime(28));
        }
        if m.kind == ModuleKind::Form {
            return Err(ScriptError::unsupported(&format!("UserForms ({})", m.name)));
        }
        let mut frame = Frame::new(m.clone(), p.clone(), me);
        self.depth += 1;
        let (writebacks, result) = self.enter(&mut frame, args, let_value);
        self.depth -= 1;
        let value = match frame.result.take() {
            Some(s) => std::mem::take(&mut s.borrow_mut().value),
            None => Value::Empty,
        };
        self.release_frame(frame);
        for (temp, place) in writebacks {
            let v = std::mem::take(&mut temp.borrow_mut().value);
            let set = v.is_object();
            self.write_place(&place, v, set)?;
        }
        result?;
        Ok(value)
    }

    /// Binds arguments, creates locals and runs the body. Returns the `ByRef` write-backs
    /// even when it fails, so the caller still sees what changed.
    fn enter(
        &mut self,
        f: &mut Frame,
        args: Vec<ArgIn>,
        let_value: Option<Value>,
    ) -> (Vec<(Slot, Place)>, R<()>) {
        let mut writebacks = Vec::new();
        let r = self
            .bind_params(f, args, let_value, &mut writebacks)
            .and_then(|()| self.init_locals(f))
            .and_then(|()| self.run_frame(f));
        (writebacks, r)
    }

    fn bind_params(
        &mut self,
        f: &mut Frame,
        args: Vec<ArgIn>,
        let_value: Option<Value>,
        writebacks: &mut Vec<(Slot, Place)>,
    ) -> R<()> {
        let proc = f.proc.clone();
        let params = &proc.params;
        let (normal, value_param) = match (&let_value, params.split_last()) {
            (Some(_), Some((last, rest))) => (rest, Some(last)),
            (Some(_), None) => return Err(ScriptError::runtime(450)),
            _ => (params.as_slice(), None),
        };
        let paramarray_at = normal.iter().position(|p| p.paramarray);
        let mut assigned: Vec<Option<ArgIn>> = (0..normal.len()).map(|_| None).collect();
        let mut extra = Vec::new();
        let mut pos = 0;
        for a in args {
            if let Some(n) = &a.name {
                let Some(i) = normal.iter().position(|p| p.name.key == *n) else {
                    return Err(ScriptError::new(
                        448,
                        format!("Named argument not found: {n}"),
                    ));
                };
                assigned[i] = Some(a);
                continue;
            }
            match paramarray_at {
                Some(pa) if pos >= pa => extra.push(a.value),
                _ if pos < normal.len() => assigned[pos] = Some(a),
                _ => return Err(ScriptError::runtime(450)),
            }
            pos += 1;
        }
        for (i, p) in normal.iter().enumerate() {
            if p.paramarray {
                let arr = Value::array(Array::from_vec(0, std::mem::take(&mut extra)));
                f.locals
                    .insert(p.name.key.clone(), new_slot(arr, p.ty.clone()));
                continue;
            }
            let slot = match assigned[i].take() {
                Some(a) if !a.value.is_missing() => self.bind_arg(p, a, writebacks)?,
                _ => {
                    if !p.optional {
                        return Err(ScriptError::new(
                            449,
                            format!("Argument not optional: {}", p.name.text),
                        ));
                    }
                    let value = match &p.default {
                        Some(d) => {
                            let module = f.module.clone();
                            let mut mf = self.module_frame(&module);
                            let v = self.eval_value(d, &mut mf)?;
                            self.coerce(v, &p.ty, false)?
                        }
                        None if p.ty == Ty::Variant => Value::Missing,
                        None => self.default_value(&p.ty)?,
                    };
                    new_slot(value, p.ty.clone())
                }
            };
            f.locals.insert(p.name.key.clone(), slot);
        }
        if let (Some(p), Some(v)) = (value_param, let_value) {
            let set = v.is_object();
            let v = self.coerce_arg(v, &p.ty, set)?;
            f.locals
                .insert(p.name.key.clone(), new_slot(v, p.ty.clone()));
        }
        Ok(())
    }

    /// An argument converted for a parameter: objects stay objects for Variant/Object
    /// parameters and become their default value for typed ones.
    fn coerce_arg(&mut self, v: Value, ty: &Ty, is_object: bool) -> R<Value> {
        if is_object {
            if matches!(ty, Ty::Variant | Ty::Object)
                || matches!(ty, Ty::Named(n) if !matches!(self.resolve_named(n), Named::Udt(..) | Named::Enum))
            {
                return Ok(v);
            }
            let v = self.deref(v)?;
            return self.coerce(v, ty, false);
        }
        if v.is_missing() {
            return Ok(v);
        }
        self.coerce(v, ty, false)
    }

    fn bind_arg(&mut self, p: &Param, a: ArgIn, writebacks: &mut Vec<(Slot, Place)>) -> R<Slot> {
        let is_object = a.value.is_object();
        if !p.byval
            && let Some(place) = a.place
        {
            if place.path.is_empty()
                && let Root::Slot(s) = &place.root
                && (p.ty == Ty::Variant || s.borrow().ty == p.ty)
            {
                return Ok(s.clone());
            }
            let v = self.coerce_arg(a.value, &p.ty, is_object)?;
            let temp = new_slot(v, p.ty.clone());
            writebacks.push((temp.clone(), place));
            return Ok(temp);
        }
        let v = self.coerce_arg(a.value, &p.ty, is_object)?;
        Ok(new_slot(v, p.ty.clone()))
    }

    fn init_locals(&mut self, f: &mut Frame) -> R<()> {
        let proc = f.proc.clone();
        let module = f.module.clone();
        if !proc.statics.is_empty() || (proc.is_static && !proc.locals.is_empty()) {
            let scope = self.static_scope(&module, &proc);
            let decls = proc
                .statics
                .iter()
                .chain(proc.locals.iter().filter(|_| proc.is_static));
            for d in decls {
                let existing = scope.borrow().get(&d.name.key).cloned();
                let slot = match existing {
                    Some(s) => s,
                    None => {
                        let s = self.make_var(d, f)?;
                        scope.borrow_mut().insert(d.name.key.clone(), s.clone());
                        s
                    }
                };
                f.locals.insert(d.name.key.clone(), slot);
            }
        }
        if !proc.is_static {
            for d in &proc.locals {
                if f.locals.contains_key(&d.name.key) {
                    continue;
                }
                let slot = self.make_var(d, f)?;
                f.locals.insert(d.name.key.clone(), slot);
            }
        }
        for c in &proc.consts {
            let v = self.eval_value(&c.value, f)?;
            let v = self.coerce(v, &c.ty, false)?;
            f.locals
                .insert(c.name.key.clone(), new_slot(v, c.ty.clone()));
        }
        if matches!(proc.kind, ProcKind::Function | ProcKind::PropertyGet) {
            let v = self.default_value(&proc.ret)?;
            f.result = Some(new_slot(v, proc.ret.clone()));
        }
        Ok(())
    }

    // ---- Statements ----

    fn run_frame(&mut self, f: &mut Frame) -> R<()> {
        let proc = f.proc.clone();
        let mut pc = 0usize;
        while pc < proc.ops.len() {
            self.tick()?;
            match self.exec_op(&proc.ops[pc], pc, f) {
                Ok(Flow::Next) => pc += 1,
                Ok(Flow::Jump(t)) => pc = t,
                Ok(Flow::Exit) => break,
                Err(mut e) => {
                    if e.location.is_none() {
                        e.location = Some(Location {
                            module: f.module.name.clone(),
                            procedure: proc.name.text.to_string(),
                            line: proc.lines[pc],
                        });
                    }
                    if !e.is_trappable() || f.in_handler {
                        return Err(e);
                    }
                    match f.handler {
                        Handler::None => return Err(e),
                        Handler::ResumeNext => {
                            self.st.err.set(&e, "VBAProject");
                            f.erl = proc.erl[pc];
                            pc += 1;
                        }
                        Handler::GoTo(t) => {
                            self.st.err.set(&e, "VBAProject");
                            f.erl = proc.erl[pc];
                            f.in_handler = true;
                            f.err_pc = pc;
                            pc = t;
                        }
                    }
                }
            }
        }
        if f.in_handler {
            // Leaving an error handler resets Err (VBA reference, "Err object").
            self.st.err.clear();
        }
        Ok(())
    }

    fn exec_op(&mut self, op: &Op, pc: usize, f: &mut Frame) -> R<Flow> {
        match op {
            Op::Exec(kind) => {
                self.exec_stmt(kind, f)?;
                Ok(Flow::Next)
            }
            Op::Branch { cond, when, target } => {
                let c = self.truth(cond, f)?;
                Ok(if c == *when {
                    Flow::Jump(*target)
                } else {
                    Flow::Next
                })
            }
            Op::Jump(t) => Ok(Flow::Jump(*t)),
            Op::For(op) => self.for_init(op, f),
            Op::Next(op) => self.for_next(op, f),
            Op::Each(op) => self.each_init(op, f),
            Op::EachNext(op) => self.each_next(op, f),
            Op::Select { test, slot } => {
                let v = self.eval_value(test, f)?;
                f.temps[*slot] = Temp::Case(v);
                Ok(Flow::Next)
            }
            Op::Case {
                slot,
                clauses,
                next,
            } => {
                let test = match &f.temps[*slot] {
                    Temp::Case(v) => v.clone(),
                    _ => Value::Empty,
                };
                Ok(if self.case_matches(&test, clauses, f)? {
                    Flow::Next
                } else {
                    Flow::Jump(*next)
                })
            }
            Op::WithEnter { depth, object } => {
                let target = self.with_target(object, f)?;
                f.withs.truncate(*depth);
                while f.withs.len() < *depth {
                    f.withs.push(WithTarget::Value(Value::Empty));
                }
                f.withs.push(target);
                Ok(Flow::Next)
            }
            Op::WithLeave(depth) => {
                f.withs.truncate(*depth);
                Ok(Flow::Next)
            }
            Op::OnError(mode) => {
                self.st.err.clear();
                match mode {
                    ErrMode::ResumeNext => f.handler = Handler::ResumeNext,
                    ErrMode::GoTo(t) => f.handler = Handler::GoTo(*t),
                    ErrMode::Disable => f.handler = Handler::None,
                    ErrMode::Reset => f.in_handler = false,
                }
                Ok(Flow::Next)
            }
            Op::Resume(to) => {
                if !f.in_handler {
                    return Err(ScriptError::runtime(20));
                }
                f.in_handler = false;
                self.st.err.clear();
                Ok(Flow::Jump(match to {
                    ResumeTo::Here => f.err_pc,
                    ResumeTo::Next => f.err_pc + 1,
                    ResumeTo::Label(t) => *t,
                }))
            }
            Op::GoSub(t) => {
                f.gosubs.push(pc + 1);
                Ok(Flow::Jump(*t))
            }
            Op::Return => f
                .gosubs
                .pop()
                .map(Flow::Jump)
                .ok_or_else(|| ScriptError::runtime(3)),
            Op::Exit => Ok(Flow::Exit),
        }
    }

    /// A condition's truth; Null counts as False.
    fn truth(&mut self, cond: &Expr, f: &mut Frame) -> R<bool> {
        let v = self.eval_value(cond, f)?;
        if v.is_null() {
            return Ok(false);
        }
        convert::to_bool(&v)
    }

    fn with_target(&mut self, e: &Expr, f: &mut Frame) -> R<WithTarget> {
        if let Some(p) = self.place_of(e, f, false)?
            && self.place_is(&p, |v| matches!(v, Value::Record(_)))?
        {
            return Ok(WithTarget::Place(p));
        }
        let v = self.eval(e, f)?;
        if matches!(v, Value::Object(None)) {
            return Err(ScriptError::runtime(91));
        }
        Ok(WithTarget::Value(v))
    }

    fn case_matches(&mut self, test: &Value, clauses: &[CaseClause], f: &mut Frame) -> R<bool> {
        let text = f.module.compare_text;
        for c in clauses {
            let hit = match c {
                CaseClause::Value(e) => {
                    let v = self.eval_value(e, f)?;
                    convert::compare(test, &v, text)? == Some(Ordering::Equal)
                }
                CaseClause::Range(lo, hi) => {
                    let lo = self.eval_value(lo, f)?;
                    let hi = self.eval_value(hi, f)?;
                    matches!(
                        convert::compare(test, &lo, text)?,
                        Some(Ordering::Greater | Ordering::Equal)
                    ) && matches!(
                        convert::compare(test, &hi, text)?,
                        Some(Ordering::Less | Ordering::Equal)
                    )
                }
                CaseClause::Is(op, e) => {
                    let v = self.eval_value(e, f)?;
                    matches!(convert::binary(*op, test, &v, text)?, Value::Boolean(true))
                }
            };
            if hit {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn for_continue(cur: &Value, to: &Value, step: &Value) -> R<bool> {
        let c = convert::to_f64(cur)?;
        let t = convert::to_f64(to)?;
        Ok(if convert::to_f64(step)? >= 0.0 {
            c <= t
        } else {
            c >= t
        })
    }

    fn for_init(&mut self, op: &ForOp, f: &mut Frame) -> R<Flow> {
        let from = self.eval_value(&op.from, f)?;
        let to = self.eval_value(&op.to, f)?;
        let step = match &op.step {
            Some(s) => self.eval_value(s, f)?,
            None => Value::Integer(1),
        };
        convert::to_f64(&from)?;
        convert::to_f64(&to)?;
        convert::to_f64(&step)?;
        let place = self.resolve_place(&op.var, f)?;
        self.write_place(&place, from, false)?;
        let cur = self.read_place(&place)?;
        let go = Self::for_continue(&cur, &to, &step)?;
        f.temps[op.slot] = Temp::For { to, step };
        Ok(if go { Flow::Next } else { Flow::Jump(op.exit) })
    }

    fn for_next(&mut self, op: &NextOp, f: &mut Frame) -> R<Flow> {
        let (to, step) = match &f.temps[op.slot] {
            Temp::For { to, step } => (to.clone(), step.clone()),
            _ => return Err(ScriptError::runtime(92)),
        };
        let place = self.resolve_place(&op.var, f)?;
        let cur = self.read_place(&place)?;
        let next = convert::arith(Arith::Add, &cur, &step)?;
        self.write_place(&place, next, false)?;
        let cur = self.read_place(&place)?;
        if Self::for_continue(&cur, &to, &step)? {
            Ok(Flow::Jump(op.body))
        } else {
            f.temps[op.slot] = Temp::None;
            Ok(Flow::Next)
        }
    }

    fn assign_loop_var(&mut self, var: &Expr, v: Value, f: &mut Frame) -> R<()> {
        let place = self.resolve_place(var, f)?;
        let set = v.is_object();
        self.write_place(&place, v, set)
    }

    fn each_init(&mut self, op: &EachOp, f: &mut Frame) -> R<Flow> {
        let coll = self.eval(&op.collection, f)?;
        let items = self.enumerate(coll)?;
        let Some(first) = items.first().cloned() else {
            return Ok(Flow::Jump(op.exit));
        };
        f.temps[op.slot] = Temp::Each { items, pos: 1 };
        self.assign_loop_var(&op.var, first, f)?;
        Ok(Flow::Next)
    }

    fn each_next(&mut self, op: &NextOp, f: &mut Frame) -> R<Flow> {
        let next = match &mut f.temps[op.slot] {
            Temp::Each { items, pos } => {
                let v = items.get(*pos).cloned();
                *pos += 1;
                v
            }
            _ => return Err(ScriptError::runtime(92)),
        };
        match next {
            Some(v) => {
                self.assign_loop_var(&op.var, v, f)?;
                Ok(Flow::Jump(op.body))
            }
            None => {
                f.temps[op.slot] = Temp::None;
                Ok(Flow::Next)
            }
        }
    }

    fn exec_stmt(&mut self, kind: &StmtKind, f: &mut Frame) -> R<()> {
        match kind {
            StmtKind::Assign { set, target, value } => {
                let v = if *set {
                    let v = self.eval(value, f)?;
                    if !v.is_object() {
                        return Err(ScriptError::runtime(424));
                    }
                    v
                } else {
                    self.eval_value(value, f)?
                };
                let place = self.resolve_place(target, f)?;
                self.write_place(&place, v, *set)
            }
            StmtKind::Call { target, args } => {
                let (t, a): (&Expr, &[ArgExpr]) = match target {
                    Expr::Call(t, a) if args.is_empty() => (t, a),
                    _ => (target, args),
                };
                self.eval_call(t, a, f, Access::Call)?;
                Ok(())
            }
            StmtKind::ReDim { preserve, items } => {
                for item in items {
                    self.redim(item, *preserve, f)?;
                }
                Ok(())
            }
            StmtKind::Erase(targets) => {
                for t in targets {
                    let place = self.resolve_place(t, f)?;
                    let Root::Slot(s) = &place.root else {
                        return Err(ScriptError::runtime(13));
                    };
                    if !place.path.is_empty() {
                        return Err(ScriptError::runtime(13));
                    }
                    let (ty, fixed, bounds) = {
                        let v = s.borrow();
                        let bounds: Vec<(i32, i32)> = match &v.value {
                            Value::Array(a) => {
                                a.dims().iter().map(|d| (d.lower, d.upper())).collect()
                            }
                            _ => return Err(ScriptError::runtime(13)),
                        };
                        (v.ty.clone(), v.fixed, bounds)
                    };
                    let elem = match ty {
                        Ty::Array(e) => *e,
                        _ => Ty::Variant,
                    };
                    let fresh = if fixed {
                        self.alloc_array(&bounds, &elem)?
                    } else {
                        Value::array(Array::unallocated())
                    };
                    s.borrow_mut().value = fresh;
                }
                Ok(())
            }
            StmtKind::MidAssign {
                target,
                start,
                length,
                value,
            } => {
                let place = self.resolve_place(target, f)?;
                let current: Vec<char> = convert::to_str(&self.read_place(&place)?)?
                    .chars()
                    .collect();
                let start = convert::to_i64(&self.eval_value(start, f)?)?;
                let repl: Vec<char> = convert::to_str(&self.eval_value(value, f)?)?
                    .chars()
                    .collect();
                if start < 1 || start as usize > current.len() {
                    return Err(ScriptError::runtime(5));
                }
                let at = start as usize - 1;
                let mut n = repl.len().min(current.len() - at);
                if let Some(l) = length {
                    let l = convert::to_i64(&self.eval_value(l, f)?)?;
                    if l < 0 {
                        return Err(ScriptError::runtime(5));
                    }
                    n = n.min(l as usize);
                }
                let mut out = current;
                out[at..at + n].copy_from_slice(&repl[..n]);
                self.write_place(&place, Value::String(out.into_iter().collect()), false)
            }
            StmtKind::Pad {
                right,
                target,
                value,
            } => {
                let place = self.resolve_place(target, f)?;
                let current = self.read_place(&place)?;
                if matches!(current, Value::Record(_)) {
                    return Err(ScriptError::unsupported("LSet between user-defined types"));
                }
                let width = convert::to_str(&current)?.chars().count();
                let v: Vec<char> = convert::to_str(&self.eval_value(value, f)?)?
                    .chars()
                    .collect();
                let v: String = v.into_iter().take(width).collect();
                let pad = " ".repeat(width - v.chars().count());
                let out = if *right {
                    format!("{pad}{v}")
                } else {
                    format!("{v}{pad}")
                };
                self.write_place(&place, Value::String(out), false)
            }
            StmtKind::Error(e) => {
                let n = convert::to_i64(&self.eval_value(e, f)?)?;
                Err(ScriptError::runtime(n as i32))
            }
            StmtKind::Print(items) => self.print(items, f),
            StmtKind::Assert(e) => {
                if self.truth(e, f)? {
                    Ok(())
                } else {
                    Err(ScriptError::interrupted(0, "Debug.Assert failed"))
                }
            }
            StmtKind::Stop => Err(ScriptError::interrupted(
                0,
                "Execution stopped at a Stop statement",
            )),
            StmtKind::End => Err(ScriptError {
                kind: ErrorKind::Halt,
                ..ScriptError::new(0, "End")
            }),
            StmtKind::Unsupported(feature) => Err(ScriptError::unsupported(feature)),
            StmtKind::Invalid(msg) => Err(ScriptError::syntax(msg.clone())),
            _ => Ok(()),
        }
    }

    fn redim(&mut self, item: &ReDimItem, preserve: bool, f: &mut Frame) -> R<()> {
        let bounds = self.eval_bounds(&item.bounds, f)?;
        let place = match &item.target {
            Expr::Name(id) => match self.lookup(id, f)? {
                Binding::Slot(s) | Binding::Result(s) => Place::slot(s),
                Binding::Unknown | Binding::Host(_) => {
                    // ReDim of an undeclared name declares a dynamic array.
                    let elem = item
                        .ty
                        .clone()
                        .unwrap_or_else(|| f.module.deftype(&id.text));
                    let slot = new_slot(
                        Value::array(Array::unallocated()),
                        Ty::Array(Box::new(elem)),
                    );
                    f.locals.insert(id.key.clone(), slot.clone());
                    Place::slot(slot)
                }
                _ => return Err(ScriptError::syntax(format!("Can't ReDim '{}'", id.text))),
            },
            other => self.resolve_place(other, f)?,
        };
        if let Root::Slot(s) = &place.root
            && place.path.is_empty()
        {
            let elem = {
                let v = s.borrow();
                if v.fixed {
                    return Err(ScriptError::runtime(10));
                }
                match &v.ty {
                    Ty::Array(e) => (**e).clone(),
                    Ty::Variant => item.ty.clone().unwrap_or(Ty::Variant),
                    _ => return Err(ScriptError::runtime(13)),
                }
            };
            // Take the array out so ReDim Preserve resizes it in place.
            let mut current = std::mem::take(&mut s.borrow_mut().value);
            let r = self.redim_value(&mut current, &bounds, &elem, preserve);
            s.borrow_mut().value = current;
            return r;
        }
        let elem = match &place.root {
            Root::Slot(s) => match self.place_type(s, &place.path) {
                Ty::Array(e) => *e,
                _ => item.ty.clone().unwrap_or(Ty::Variant),
            },
            _ => item.ty.clone().unwrap_or(Ty::Variant),
        };
        let mut current = self.read_place(&place)?;
        self.redim_value(&mut current, &bounds, &elem, preserve)?;
        self.write_place(&place, current, false)
    }

    /// Reallocates an array value; on error it's left unchanged.
    fn redim_value(
        &mut self,
        v: &mut Value,
        bounds: &[(i32, i32)],
        elem: &Ty,
        preserve: bool,
    ) -> R<()> {
        if preserve && let Value::Array(a) = v {
            let fill = self.default_value(elem)?;
            return Rc::make_mut(a).resize_last(bounds, fill);
        }
        *v = self.alloc_array(bounds, elem)?;
        Ok(())
    }

    fn print_text(v: &Value) -> String {
        match v {
            Value::Byte(_)
            | Value::Integer(_)
            | Value::Long(_)
            | Value::LongLong(_)
            | Value::Single(_)
            | Value::Double(_)
            | Value::Currency(_) => {
                let s = convert::to_str(v).unwrap_or_default();
                if s.starts_with('-') {
                    format!("{s} ")
                } else {
                    format!(" {s} ")
                }
            }
            other => convert::display(other),
        }
    }

    fn print(&mut self, items: &[PrintItem], f: &mut Frame) -> R<()> {
        for item in items {
            match item {
                PrintItem::Expr(e) => {
                    let v = self.eval_value(e, f)?;
                    self.st.print_line.push_str(&Self::print_text(&v));
                }
                PrintItem::Semi => {}
                PrintItem::Comma => {
                    let col = self.st.print_line.chars().count();
                    let target = (col / 14 + 1) * 14;
                    self.st
                        .print_line
                        .extend(std::iter::repeat_n(' ', target - col));
                }
                PrintItem::Spc(e) => {
                    let n = convert::to_i64(&self.eval_value(e, f)?)?.max(0) as usize;
                    self.st.print_line.extend(std::iter::repeat_n(' ', n));
                }
                PrintItem::Tab(e) => {
                    let col = self.st.print_line.chars().count();
                    let target = match e {
                        Some(e) => (convert::to_i64(&self.eval_value(e, f)?)?.max(1) as usize) - 1,
                        None => (col / 14 + 1) * 14,
                    };
                    if target > col {
                        self.st
                            .print_line
                            .extend(std::iter::repeat_n(' ', target - col));
                    }
                }
            }
        }
        if !matches!(items.last(), Some(PrintItem::Semi | PrintItem::Comma)) {
            let line = std::mem::take(&mut self.st.print_line);
            self.host.debug_print(&line);
        }
        Ok(())
    }

    /// Writes out a pending `Debug.Print` line that ended with `;`.
    pub fn flush_print(&mut self) {
        if !self.st.print_line.is_empty() {
            let line = std::mem::take(&mut self.st.print_line);
            self.host.debug_print(&line);
        }
    }

    // ---- Builtin plumbing ----

    fn builtin_args(&mut self, key: &str, args: &[ArgExpr], f: &mut Frame) -> R<Vec<Value>> {
        let raw = self.eval_args(args, f)?;
        let mut out: Vec<Value> = Vec::new();
        let names = builtins::params(key);
        for a in raw {
            match a.name {
                None => out.push(a.value),
                Some(n) => {
                    let Some(i) =
                        names.and_then(|ns| ns.iter().position(|p| p.eq_ignore_ascii_case(&n)))
                    else {
                        return Err(ScriptError::new(
                            448,
                            format!("Named argument not found: {n}"),
                        ));
                    };
                    if out.len() <= i {
                        out.resize(i + 1, Value::Missing);
                    }
                    out[i] = a.value;
                }
            }
        }
        if !builtins::keeps_objects(key) {
            for v in &mut out {
                if v.is_object() {
                    let taken = std::mem::take(v);
                    *v = self.deref(taken)?;
                }
            }
        }
        Ok(out)
    }

    /// Calls a VBA instance member or a host object member by name (`CallByName`).
    pub fn call_by_name(
        &mut self,
        o: &ObjectRef,
        name: &str,
        kind: i64,
        mut args: Vec<Arg>,
    ) -> R<Value> {
        match kind {
            4 | 8 => {
                let Some(v) = args.pop() else {
                    return Err(ScriptError::runtime(449));
                };
                self.obj_put(o, name, args, v.value, kind == 8)?;
                Ok(Value::Empty)
            }
            1 => self.obj_get(o, name, args, Access::Call),
            _ => self.obj_get(o, name, args, Access::Get),
        }
    }
}
