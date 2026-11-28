//! Objects the VBA runtime provides itself: `Collection`, `Scripting.Dictionary`, the `Err`
//! object and instances of class modules. They implement [`Dispatch`], so a host or another
//! language binding can use them like any other object.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::ast::Ty;
use super::compile::Module;
use super::convert;
use crate::object::{Arg, Array, Dispatch, ScriptError, Value, find_arg, standard_message};

type R<T> = Result<T, ScriptError>;

/// A variable: its value, declared type and how it was declared.
#[derive(Debug)]
pub struct Var {
    pub value: Value,
    pub ty: Ty,
    /// A fixed-size array (`Dim a(10)`): can't be `ReDim`med or assigned.
    pub fixed: bool,
    /// `Dim x As New Class`: the class to create on first use.
    pub auto_new: Option<Rc<str>>,
}

/// A shared variable cell; `ByRef` arguments share the caller's cell.
pub type Slot = Rc<RefCell<Var>>;

/// Variables by lower-case name.
pub type Scope = RefCell<HashMap<Rc<str>, Slot>>;

pub fn new_slot(value: Value, ty: Ty) -> Slot {
    Rc::new(RefCell::new(Var {
        value,
        ty,
        fixed: false,
        auto_new: None,
    }))
}

fn is(name: &str, member: &str) -> bool {
    name.eq_ignore_ascii_case(member)
}

fn required<'a>(args: &'a [Arg], position: usize, name: &str) -> R<&'a Value> {
    find_arg(args, position, name).ok_or_else(|| ScriptError::runtime(449))
}

// ---- Collection ----

/// VBA's `Collection`: 1-based items with optional case-insensitive string keys.
#[derive(Default)]
pub struct Collection {
    items: RefCell<Vec<(Option<String>, Value)>>,
}

impl Collection {
    pub fn new() -> Collection {
        Collection::default()
    }

    pub fn len(&self) -> usize {
        self.items.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.borrow().is_empty()
    }

    fn position(&self, index: &Value) -> R<usize> {
        let items = self.items.borrow();
        match index {
            Value::String(key) => {
                let key = key.to_lowercase();
                items
                    .iter()
                    .position(|(k, _)| k.as_deref() == Some(key.as_str()))
                    .ok_or_else(|| ScriptError::runtime(5))
            }
            Value::Object(_) | Value::Array(_) | Value::Record(_) => Err(ScriptError::runtime(13)),
            other => {
                let i = convert::to_i64(other)?;
                if i >= 1 && (i as usize) <= items.len() {
                    Ok(i as usize - 1)
                } else {
                    Err(ScriptError::runtime(9))
                }
            }
        }
    }

    fn add(&self, args: &[Arg]) -> R<()> {
        let item = required(args, 0, "item")?.clone();
        let key = match find_arg(args, 1, "key") {
            None => None,
            Some(Value::String(s)) => Some(s.to_lowercase()),
            Some(Value::Object(_) | Value::Array(_)) => return Err(ScriptError::runtime(13)),
            Some(other) => Some(convert::to_str(other)?.to_lowercase()),
        };
        if let Some(k) = &key
            && self
                .items
                .borrow()
                .iter()
                .any(|(x, _)| x.as_ref() == Some(k))
        {
            return Err(ScriptError::runtime(457));
        }
        let before = find_arg(args, 2, "before").cloned();
        let after = find_arg(args, 3, "after").cloned();
        let at = match (before, after) {
            (Some(_), Some(_)) => return Err(ScriptError::runtime(5)),
            (Some(b), None) => self.position(&b)?,
            (None, Some(a)) => self.position(&a)? + 1,
            (None, None) => self.len(),
        };
        self.items.borrow_mut().insert(at, (key, item));
        Ok(())
    }
}

impl Dispatch for Collection {
    fn type_name(&self) -> String {
        "Collection".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> R<Value> {
        if name.is_empty() || is(name, "item") {
            let i = self.position(required(args, 0, "index")?)?;
            return Ok(self.items.borrow()[i].1.clone());
        }
        if is(name, "count") {
            return Ok(Value::Long(self.len() as i32));
        }
        if is(name, "add") {
            self.add(args)?;
            return Ok(Value::Empty);
        }
        if is(name, "remove") {
            let i = self.position(required(args, 0, "index")?)?;
            self.items.borrow_mut().remove(i);
            return Ok(Value::Empty);
        }
        if is(name, "_newenum") {
            return Ok(Value::array(Array::from_vec(0, self.enumerate()?)));
        }
        Err(ScriptError::no_member("Collection", name))
    }

    fn enumerate(&self) -> R<Vec<Value>> {
        Ok(self.items.borrow().iter().map(|(_, v)| v.clone()).collect())
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }
}

// ---- Scripting.Dictionary ----

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum DictKey {
    Empty,
    Num(u64),
    Bool(bool),
    Str(String),
    Obj(usize),
    Err(i32),
}

#[derive(Default)]
struct DictInner {
    entries: Vec<(Value, Value)>,
    index: HashMap<DictKey, usize>,
    text: bool,
}

impl DictInner {
    fn reindex(&mut self) {
        self.index.clear();
        for (i, (k, _)) in self.entries.iter().enumerate() {
            if let Ok(key) = dict_key(k, self.text) {
                self.index.insert(key, i);
            }
        }
    }
}

fn dict_key(v: &Value, text: bool) -> R<DictKey> {
    Ok(match v {
        Value::Empty => DictKey::Empty,
        Value::Boolean(b) => DictKey::Bool(*b),
        Value::String(s) => DictKey::Str(if text { s.to_lowercase() } else { s.clone() }),
        Value::Object(Some(o)) => DictKey::Obj(Rc::as_ptr(o) as *const () as usize),
        Value::Object(None) => DictKey::Obj(0),
        Value::Error(n) => DictKey::Err(*n),
        Value::Null | Value::Missing => return Err(ScriptError::runtime(94)),
        Value::Array(_) | Value::Record(_) => return Err(ScriptError::runtime(13)),
        other => {
            // Numeric keys compare by value whatever their subtype (1, 1&, 1# are one key).
            let f = other.as_f64().unwrap_or(0.0);
            DictKey::Num(if f == 0.0 { 0 } else { f.to_bits() })
        }
    })
}

/// `Scripting.Dictionary`: keys of any scalar or object type, kept in insertion order.
#[derive(Default)]
pub struct Dictionary {
    inner: RefCell<DictInner>,
}

impl Dictionary {
    pub fn new() -> Dictionary {
        Dictionary::default()
    }

    fn find(&self, key: &Value) -> R<Option<usize>> {
        let inner = self.inner.borrow();
        Ok(inner.index.get(&dict_key(key, inner.text)?).copied())
    }

    fn insert(&self, key: Value, item: Value) -> R<()> {
        let mut inner = self.inner.borrow_mut();
        let k = dict_key(&key, inner.text)?;
        let at = inner.entries.len();
        inner.entries.push((key, item));
        inner.index.insert(k, at);
        Ok(())
    }

    fn set_item(&self, key: &Value, item: Value) -> R<()> {
        match self.find(key)? {
            Some(i) => {
                self.inner.borrow_mut().entries[i].1 = item;
                Ok(())
            }
            None => self.insert(key.clone(), item),
        }
    }
}

impl Dispatch for Dictionary {
    fn type_name(&self) -> String {
        "Dictionary".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> R<Value> {
        if name.is_empty() || is(name, "item") {
            let key = required(args, 0, "key")?;
            return match self.find(key)? {
                Some(i) => Ok(self.inner.borrow().entries[i].1.clone()),
                None => {
                    // Reading a missing key adds it with an Empty item (Scripting runtime
                    // behaviour).
                    self.insert(key.clone(), Value::Empty)?;
                    Ok(Value::Empty)
                }
            };
        }
        if is(name, "add") {
            let key = required(args, 0, "key")?;
            let item = required(args, 1, "item")?.clone();
            if self.find(key)?.is_some() {
                return Err(ScriptError::runtime(457));
            }
            self.insert(key.clone(), item)?;
            return Ok(Value::Empty);
        }
        if is(name, "exists") {
            return Ok(Value::Boolean(
                self.find(required(args, 0, "key")?)?.is_some(),
            ));
        }
        if is(name, "count") {
            return Ok(Value::Long(self.inner.borrow().entries.len() as i32));
        }
        if is(name, "keys") || is(name, "items") {
            let keys = is(name, "keys");
            let inner = self.inner.borrow();
            let vals = inner
                .entries
                .iter()
                .map(|(k, v)| if keys { k.clone() } else { v.clone() })
                .collect();
            return Ok(Value::array(Array::from_vec(0, vals)));
        }
        if is(name, "remove") {
            let key = required(args, 0, "key")?;
            let Some(i) = self.find(key)? else {
                return Err(ScriptError::runtime(32811));
            };
            let mut inner = self.inner.borrow_mut();
            inner.entries.remove(i);
            inner.reindex();
            return Ok(Value::Empty);
        }
        if is(name, "removeall") {
            let mut inner = self.inner.borrow_mut();
            inner.entries.clear();
            inner.index.clear();
            return Ok(Value::Empty);
        }
        if is(name, "comparemode") {
            return Ok(Value::Long(i32::from(self.inner.borrow().text)));
        }
        if is(name, "_newenum") {
            return Ok(Value::array(Array::from_vec(0, self.enumerate()?)));
        }
        Err(ScriptError::no_member("Dictionary", name))
    }

    fn put(&self, name: &str, args: &[Arg], value: Value) -> R<()> {
        if name.is_empty() || is(name, "item") {
            return self.set_item(required(args, 0, "key")?, value);
        }
        if is(name, "key") {
            let old = required(args, 0, "key")?;
            let Some(i) = self.find(old)? else {
                return Err(ScriptError::runtime(32811));
            };
            if self.find(&value)?.is_some() {
                return Err(ScriptError::runtime(457));
            }
            let mut inner = self.inner.borrow_mut();
            inner.entries[i].0 = value;
            inner.reindex();
            return Ok(());
        }
        if is(name, "comparemode") {
            let mut inner = self.inner.borrow_mut();
            if !inner.entries.is_empty() {
                return Err(ScriptError::runtime(5));
            }
            inner.text = convert::to_i64(&value)? == 1;
            return Ok(());
        }
        Err(ScriptError::no_member("Dictionary", name))
    }

    fn enumerate(&self) -> R<Vec<Value>> {
        Ok(self
            .inner
            .borrow()
            .entries
            .iter()
            .map(|(k, _)| k.clone())
            .collect())
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }
}

// ---- Err ----

#[derive(Clone, Debug, Default)]
pub struct ErrState {
    pub number: i32,
    pub description: String,
    pub source: String,
    pub help_file: String,
    pub help_context: i32,
}

/// The `Err` object.
#[derive(Default)]
pub struct ErrObject {
    pub state: RefCell<ErrState>,
}

impl ErrObject {
    pub fn set(&self, e: &ScriptError, default_source: &str) {
        let mut s = self.state.borrow_mut();
        s.number = e.number;
        s.description = e.description.clone();
        s.source = if e.source.is_empty() {
            default_source.to_string()
        } else {
            e.source.clone()
        };
    }

    pub fn clear(&self) {
        *self.state.borrow_mut() = ErrState::default();
    }

    pub fn number(&self) -> i32 {
        self.state.borrow().number
    }
}

impl Dispatch for ErrObject {
    fn type_name(&self) -> String {
        "ErrObject".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> R<Value> {
        let s = self.state.borrow().clone();
        if name.is_empty() || is(name, "number") {
            return Ok(Value::Long(s.number));
        }
        if is(name, "description") {
            return Ok(Value::String(s.description));
        }
        if is(name, "source") {
            return Ok(Value::String(s.source));
        }
        if is(name, "helpfile") {
            return Ok(Value::String(s.help_file));
        }
        if is(name, "helpcontext") {
            return Ok(Value::Long(s.help_context));
        }
        if is(name, "lastdllerror") {
            return Ok(Value::Long(0));
        }
        if is(name, "clear") {
            self.clear();
            return Ok(Value::Empty);
        }
        if is(name, "raise") {
            let number = convert::to_i64(required(args, 0, "number")?)? as i32;
            let description = match find_arg(args, 2, "description") {
                Some(d) => convert::to_str(d)?,
                None => standard_message(number).to_string(),
            };
            let mut e = ScriptError::new(number, description);
            if let Some(src) = find_arg(args, 1, "source") {
                e.source = convert::to_str(src)?;
            }
            return Err(e);
        }
        Err(ScriptError::no_member("ErrObject", name))
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> R<()> {
        let mut s = self.state.borrow_mut();
        if name.is_empty() || is(name, "number") {
            s.number = convert::to_i64(&value)? as i32;
            if s.description.is_empty() && s.number != 0 {
                s.description = standard_message(s.number).to_string();
            }
        } else if is(name, "description") {
            s.description = convert::to_str(&value)?;
        } else if is(name, "source") {
            s.source = convert::to_str(&value)?;
        } else if is(name, "helpfile") {
            s.help_file = convert::to_str(&value)?;
        } else if is(name, "helpcontext") {
            s.help_context = convert::to_i64(&value)? as i32;
        } else {
            return Err(ScriptError::no_member("ErrObject", name));
        }
        Ok(())
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }
}

// ---- Class instances ----

/// An instance of a class module. The interpreter calls its procedures; through [`Dispatch`]
/// (for hosts and other languages) only its public variables are reachable.
pub struct Instance {
    pub module: Rc<Module>,
    pub fields: Rc<Scope>,
    /// A form module's instance: the form's controls and state.
    pub(crate) form: Option<Rc<super::forms::FormRt>>,
}

impl Instance {
    fn public_field(&self, name: &str) -> Option<Slot> {
        let key = name.to_lowercase();
        if !self.module.public_vars.contains(key.as_str()) {
            return None;
        }
        self.fields.borrow().get(key.as_str()).cloned()
    }
}

impl Dispatch for Instance {
    fn type_name(&self) -> String {
        self.module.name.clone()
    }

    fn get(&self, name: &str, args: &[Arg]) -> R<Value> {
        match self.public_field(name) {
            Some(slot) if args.is_empty() => Ok(slot.borrow().value.clone()),
            _ => Err(ScriptError::no_member(&self.module.name, name)),
        }
    }

    fn put(&self, name: &str, args: &[Arg], value: Value) -> R<()> {
        match self.public_field(name) {
            Some(slot) if args.is_empty() => {
                slot.borrow_mut().value = value;
                Ok(())
            }
            _ => Err(ScriptError::no_member(&self.module.name, name)),
        }
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_keys_and_positions() {
        let c = Collection::new();
        c.get("Add", &[Arg::new("a"), Arg::new("K1")]).unwrap();
        c.get("Add", &[Arg::new("b")]).unwrap();
        c.get("Add", &[Arg::new("first"), Arg::named("Before", 1)])
            .unwrap();
        assert_eq!(c.get("Count", &[]).unwrap(), Value::Long(3));
        assert_eq!(c.get("", &[Arg::new(1)]).unwrap(), Value::from("first"));
        assert_eq!(c.get("Item", &[Arg::new("k1")]).unwrap(), Value::from("a"));
        assert_eq!(
            c.get("Add", &[Arg::new("x"), Arg::new("k1")])
                .unwrap_err()
                .number,
            457
        );
        assert_eq!(c.get("Item", &[Arg::new(9)]).unwrap_err().number, 9);
        assert_eq!(c.get("Item", &[Arg::new("nope")]).unwrap_err().number, 5);
    }

    #[test]
    fn dictionary_semantics() {
        let d = Dictionary::new();
        d.get("Add", &[Arg::new(1), Arg::new("one")]).unwrap();
        // 1 (Long) and 1# (Double) are the same key; "1" is a different one.
        assert_eq!(
            d.get("Exists", &[Arg::new(1.0)]).unwrap(),
            Value::Boolean(true)
        );
        assert_eq!(
            d.get("Exists", &[Arg::new("1")]).unwrap(),
            Value::Boolean(false)
        );
        assert_eq!(
            d.get("Add", &[Arg::new(1), Arg::new("x")])
                .unwrap_err()
                .number,
            457
        );
        d.put("Item", &[Arg::new("b")], Value::from(2)).unwrap();
        assert_eq!(d.get("", &[Arg::new("b")]).unwrap(), Value::Long(2));
        assert_eq!(d.get("Item", &[Arg::new("missing")]).unwrap(), Value::Empty);
        assert_eq!(d.get("Count", &[]).unwrap(), Value::Long(3));
        d.get("Remove", &[Arg::new("missing")]).unwrap();
        assert_eq!(
            d.get("Remove", &[Arg::new("zzz")]).unwrap_err().number,
            32811
        );
        let keys = d.get("Keys", &[]).unwrap();
        let Value::Array(a) = keys else {
            panic!("Keys returns an array")
        };
        assert_eq!(a.values(), &[Value::Long(1), Value::from("b")]);
    }
}
