//! The native side of the bridge (`N` in `prelude.js`) and value conversions.

use std::rc::Rc;

use kantor_script::binding::{self, Member};
use kantor_script::{Arg, Array, ObjectRef, Record, ScriptError, Value, date};
use rquickjs::{Class, Ctx, Function, Object};

use crate::{Bridge, Handle, Js};

const PRELUDE: &str = include_str!("prelude.js");

/// A conversion that failed: a script-level error (type mismatch) or a JavaScript exception.
pub(crate) enum Failed {
    Script(ScriptError),
    Js(rquickjs::Error),
}

impl From<rquickjs::Error> for Failed {
    fn from(e: rquickjs::Error) -> Failed {
        Failed::Js(e)
    }
}

impl From<ScriptError> for Failed {
    fn from(e: ScriptError) -> Failed {
        Failed::Script(e)
    }
}

/// `e` as a `KantorError`.
fn kantor_error<'js>(ctx: &Ctx<'js>, e: &ScriptError) -> rquickjs::Result<Js<'js>> {
    let f: Function = kit(ctx)?.get("error")?;
    f.call((e.number, e.description.clone(), e.source.clone()))
}

/// Throws `e` as a `KantorError`.
fn throw(ctx: &Ctx<'_>, e: &ScriptError) -> rquickjs::Error {
    match kantor_error(ctx, e) {
        Ok(v) => ctx.throw(v),
        Err(err) => err,
    }
}

fn fail(ctx: &Ctx<'_>, e: Failed) -> rquickjs::Error {
    match e {
        Failed::Script(e) => throw(ctx, &e),
        Failed::Js(e) => e,
    }
}

/// The prelude's helpers (`__kantor`).
fn kit<'js>(ctx: &Ctx<'js>) -> rquickjs::Result<Object<'js>> {
    ctx.globals().get("__kantor")
}

impl Bridge {
    /// Error 18 when the host or the cancel token says stop.
    fn check(&self, ctx: &Ctx<'_>) -> rquickjs::Result<()> {
        let stop = self.cancel.is_cancelled()
            || self
                .host
                .try_borrow_mut()
                .map(|mut h| h.cancelled())
                .unwrap_or(false);
        if stop {
            self.cancel.cancel();
            return Err(throw(
                ctx,
                &ScriptError::interrupted(18, kantor_script::object::standard_message(18)),
            ));
        }
        Ok(())
    }

    fn is_method(&self, obj: &ObjectRef, name: &str) -> bool {
        self.host
            .try_borrow_mut()
            .map(|mut h| h.is_method(&obj.type_name(), name))
            .unwrap_or(false)
    }
}

/// Positional (`undefined` is an omitted argument) and named arguments.
fn arguments<'js>(
    ctx: &Ctx<'js>,
    positional: &rquickjs::Array<'js>,
    named: Option<Object<'js>>,
) -> Result<Vec<Arg>, Failed> {
    let mut out = Vec::with_capacity(positional.len());
    for i in 0..positional.len() {
        let v: Js = positional.get(i)?;
        out.push(Arg {
            name: None,
            value: from_js(ctx, &v, Value::Missing)?,
        });
    }
    if let Some(named) = named {
        for prop in named.props::<String, Js>() {
            let (k, v) = prop?;
            out.push(Arg {
                name: Some(k),
                value: from_js(ctx, &v, Value::Missing)?,
            });
        }
    }
    Ok(out)
}

fn handle(h: &Class<'_, Handle>) -> ObjectRef {
    h.borrow().0.clone()
}

/// Defines `N`, evaluates the prelude with it.
pub(crate) fn install<'js>(
    ctx: &Ctx<'js>,
    bridge: &Rc<Bridge>,
    globals: Option<ObjectRef>,
) -> rquickjs::Result<()> {
    let n = Object::new(ctx.clone())?;

    let b = bridge.clone();
    n.set(
        "member",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, h: Class<'js, Handle>, name: String, method: Js<'js>| {
                b.check(&ctx)?;
                let obj = handle(&h);
                let is_method = b.is_method(&obj, &name);
                match binding::member(&obj, &name, is_method) {
                    Ok(Member::Method) => Ok(method),
                    Ok(Member::Value(v)) => to_js(&ctx, &v).map_err(|e| fail(&ctx, e)),
                    Err(e) => Err(throw(&ctx, &e)),
                }
            },
        )?,
    )?;

    for (key, as_call) in [("get", false), ("call", true)] {
        let b = bridge.clone();
        n.set(
            key,
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>,
                      h: Class<'js, Handle>,
                      name: String,
                      positional: rquickjs::Array<'js>,
                      named: Option<Object<'js>>| {
                    b.check(&ctx)?;
                    let obj = handle(&h);
                    let args = arguments(&ctx, &positional, named).map_err(|e| fail(&ctx, e))?;
                    let r = if as_call {
                        obj.call(&name, &args)
                    } else {
                        obj.get(&name, &args)
                    };
                    match r {
                        Ok(v) => to_js(&ctx, &v).map_err(|e| fail(&ctx, e)),
                        Err(e) => Err(throw(&ctx, &e)),
                    }
                },
            )?,
        )?;
    }

    let b = bridge.clone();
    n.set(
        "put",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, h: Class<'js, Handle>, name: String, value: Js<'js>| {
                b.check(&ctx)?;
                let obj = handle(&h);
                let value = from_js(&ctx, &value, Value::Empty).map_err(|e| fail(&ctx, e))?;
                let r = if value.is_object() {
                    obj.put_ref(&name, &[], value)
                } else {
                    obj.put(&name, &[], value)
                };
                r.map_err(|e| throw(&ctx, &e))
            },
        )?,
    )?;

    let b = bridge.clone();
    n.set(
        "items",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, h: Class<'js, Handle>| {
            b.check(&ctx)?;
            let items = handle(&h).enumerate().map_err(|e| throw(&ctx, &e))?;
            let mut out = Vec::with_capacity(items.len());
            for v in &items {
                out.push(to_js(&ctx, v).map_err(|e| fail(&ctx, e))?);
            }
            Ok::<_, rquickjs::Error>(out)
        })?,
    )?;

    n.set(
        "typeName",
        Function::new(ctx.clone(), |h: Class<'js, Handle>| handle(&h).type_name())?,
    )?;

    n.set(
        "primitive",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, h: Class<'js, Handle>| {
            let obj = handle(&h);
            match obj.get("", &[]) {
                Ok(v) if !v.is_object() && !matches!(v, Value::Array(_) | Value::Record(_)) => {
                    to_js(&ctx, &v).map_err(|e| fail(&ctx, e))
                }
                _ => rquickjs::String::from_str(ctx.clone(), &format!("[{}]", obj.type_name()))
                    .map(|s| s.into_value()),
            }
        })?,
    )?;

    n.set(
        "errorText",
        Function::new(ctx.clone(), |code: i32| error_text(code).to_owned())?,
    )?;

    n.set(
        "typeOf",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, v: Js<'js>| {
            match from_js(&ctx, &v, Value::Empty) {
                Ok(v) => type_name(&v),
                Err(_) => "Object".to_owned(),
            }
        })?,
    )?;

    let b = bridge.clone();
    n.set(
        "print",
        Function::new(ctx.clone(), move |line: String| {
            if let Ok(mut h) = b.host.try_borrow_mut() {
                h.debug_print(&line);
            }
        })?,
    )?;

    let b = bridge.clone();
    n.set(
        "msgBox",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, prompt: String, buttons: i32, title: String| {
                b.check(&ctx)?;
                let r = match b.host.try_borrow_mut() {
                    Ok(mut h) => h.msg_box(&prompt, buttons, &title),
                    Err(_) => Ok(1),
                };
                r.map_err(|e| throw(&ctx, &e))
            },
        )?,
    )?;

    let b = bridge.clone();
    n.set(
        "inputBox",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, prompt: String, title: String, default: String| {
                b.check(&ctx)?;
                let r = match b.host.try_borrow_mut() {
                    Ok(mut h) => h.input_box(&prompt, &title, &default),
                    Err(_) => Ok(None),
                };
                match r {
                    Ok(Some(s)) => {
                        rquickjs::String::from_str(ctx.clone(), &s).map(|s| s.into_value())
                    }
                    Ok(None) => Ok(Js::new_null(ctx.clone())),
                    Err(e) => Err(throw(&ctx, &e)),
                }
            },
        )?,
    )?;

    if let Some(g) = &globals {
        n.set("globals", Class::instance(ctx.clone(), Handle(g.clone()))?)?;
        n.set(
            "globalNames",
            crate::GLOBAL_NAMES
                .iter()
                .map(|s| (*s).to_owned())
                .collect::<Vec<_>>(),
        )?;

        let b = bridge.clone();
        let g2 = g.clone();
        n.set(
            "lookup",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, name: String, method: Js<'js>| {
                    b.check(&ctx)?;
                    if let Some(v) = constant(&b, &name) {
                        return to_js(&ctx, &v).map_err(|e| fail(&ctx, e));
                    }
                    let is_method = b.is_method(&g2, &name);
                    match binding::member(&g2, &name, is_method) {
                        Ok(Member::Method) => Ok(method),
                        Ok(Member::Value(v)) => to_js(&ctx, &v).map_err(|e| fail(&ctx, e)),
                        Err(e) if e.number == 438 => Ok(Js::new_undefined(ctx.clone())),
                        Err(e) => Err(throw(&ctx, &e)),
                    }
                },
            )?,
        )?;

        let b = bridge.clone();
        let g3 = g.clone();
        n.set(
            "known",
            Function::new(ctx.clone(), move |name: String| {
                if constant(&b, &name).is_some() || b.is_method(&g3, &name) {
                    return true;
                }
                match g3.get(&name, &[]) {
                    Ok(_) => true,
                    Err(e) => binding::needs_arguments(&e),
                }
            })?,
        )?;
    }

    let prelude: Function = ctx.eval(PRELUDE)?;
    prelude.call::<_, ()>((n,))?;
    Ok(())
}

/// A constant of the host's object model, or VBA's message box constants.
fn constant(b: &Bridge, name: &str) -> Option<Value> {
    if let Some(v) = binding::vb_constant(name) {
        return Some(v);
    }
    b.host.try_borrow_mut().ok()?.constant(name)
}

/// Excel's text for an error value.
pub(crate) fn error_text(code: i32) -> &'static str {
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

/// A [`Value`] as JavaScript.
pub(crate) fn to_js<'js>(ctx: &Ctx<'js>, v: &Value) -> Result<Js<'js>, Failed> {
    let c = ctx.clone();
    Ok(match v {
        Value::Empty | Value::Null | Value::Object(None) => Js::new_null(c),
        Value::Missing => Js::new_undefined(c),
        Value::Boolean(b) => Js::new_bool(c, *b),
        Value::Byte(n) => Js::new_int(c, i32::from(*n)),
        Value::Integer(n) => Js::new_int(c, i32::from(*n)),
        Value::Long(n) => Js::new_int(c, *n),
        Value::LongLong(n) => Js::new_number(c, *n as f64),
        Value::Single(n) => Js::new_number(c, f64::from(*n)),
        Value::Double(n) => Js::new_number(c, *n),
        Value::Currency(n) => Js::new_number(c, *n as f64 / 10_000.0),
        Value::String(s) => rquickjs::String::from_str(c, s)?.into_value(),
        Value::Date(d) => {
            let (y, m, day) = date::ymd(*d);
            let (hh, mm, ss) = date::hms(*d);
            let f: Function = kit(ctx)?.get("date")?;
            f.call((y as f64, m, day, hh, mm, ss))?
        }
        Value::Error(code) => {
            let f: Function = kit(ctx)?.get("cellError")?;
            f.call((*code,))?
        }
        Value::Array(a) => array_to_js(ctx, a, 0, &mut Vec::new())?,
        Value::Record(r) => {
            let o = Object::new(c)?;
            for (k, v) in &r.fields {
                o.set(k.as_str(), to_js(ctx, v)?)?;
            }
            o.into_value()
        }
        Value::Object(Some(o)) => {
            let f: Function = kit(ctx)?.get("wrap")?;
            let h = Class::instance(c, Handle(o.clone()))?;
            f.call((h,))?
        }
    })
}

/// Dimension `dim` of `a` as nested arrays (rows of a 2-D array first).
fn array_to_js<'js>(
    ctx: &Ctx<'js>,
    a: &Array,
    dim: usize,
    index: &mut Vec<i32>,
) -> Result<Js<'js>, Failed> {
    let Some(d) = a.dims().get(dim) else {
        return to_js(ctx, a.get(index).unwrap_or(&Value::Empty));
    };
    let out = rquickjs::Array::new(ctx.clone())?;
    for (k, i) in (d.lower..=d.upper()).enumerate() {
        index.push(i);
        let v = array_to_js(ctx, a, dim + 1, index)?;
        index.pop();
        out.set(k, v)?;
    }
    Ok(out.into_value())
}

/// A whole number when it is one, as VBA's numbers from cells come.
fn number(f: f64) -> Value {
    if f.fract() == 0.0 && f >= f64::from(i32::MIN) && f <= f64::from(i32::MAX) {
        Value::Long(f as i32)
    } else {
        Value::Double(f)
    }
}

/// JavaScript as a [`Value`]; `undefined` becomes `undefined` (an omitted argument is
/// [`Value::Missing`], a property set to it is [`Value::Empty`]).
pub(crate) fn from_js<'js>(ctx: &Ctx<'js>, v: &Js<'js>, undefined: Value) -> Result<Value, Failed> {
    if v.is_undefined() {
        return Ok(undefined);
    }
    if v.is_null() {
        return Ok(Value::Empty);
    }
    if let Some(b) = v.as_bool() {
        return Ok(Value::Boolean(b));
    }
    if let Some(i) = v.as_int() {
        return Ok(Value::Long(i));
    }
    if let Some(f) = v.as_float() {
        return Ok(number(f));
    }
    if let Some(s) = v.as_string() {
        return Ok(Value::String(s.to_string()?));
    }
    if let Some(b) = v.as_big_int() {
        return Ok(Value::LongLong(b.clone().to_i64()?));
    }
    if let Some(arr) = v.as_array() {
        let mut items = Vec::with_capacity(arr.len());
        for i in 0..arr.len() {
            let x: Js = arr.get(i)?;
            items.push(x);
        }
        // An array of arrays is a 2-D array (rows), as `Range.value` takes it.
        if !items.is_empty() && items.iter().all(|x| x.is_array()) {
            let mut rows = Vec::with_capacity(items.len());
            for row in &items {
                let row = row.as_array().cloned().unwrap_or_else(|| unreachable!());
                let mut cells = Vec::with_capacity(row.len());
                for i in 0..row.len() {
                    let x: Js = row.get(i)?;
                    cells.push(from_js(ctx, &x, Value::Empty)?);
                }
                rows.push(cells);
            }
            return Ok(Value::array(Array::from_rows(0, rows)));
        }
        let mut out = Vec::with_capacity(items.len());
        for x in &items {
            out.push(from_js(ctx, x, Value::Empty)?);
        }
        return Ok(Value::array(Array::from_vec(0, out)));
    }
    if let Some(obj) = v.as_object() {
        let classify: Function = kit(ctx)?.get("classify")?;
        let tag: Js = classify.call((v.clone(),))?;
        if let Ok(h) = Class::<Handle>::from_value(&tag) {
            return Ok(Value::Object(Some(handle(&h))));
        }
        if let Some(t) = tag.as_array() {
            let kind: String = t.get(0)?;
            match kind.as_str() {
                "d" => {
                    let n = |i: usize| -> rquickjs::Result<f64> { t.get::<f64>(i) };
                    let day = date::day_number_normalised(n(1)? as i64, n(2)? as i64, n(3)? as i64);
                    let secs = n(4)? * 3600.0 + n(5)? * 60.0 + n(6)? + n(7)? / 1000.0;
                    return Ok(Value::Date(date::join(day, secs)));
                }
                "e" => return Ok(Value::Error(t.get::<i32>(1)?)),
                _ => {
                    return Err(Failed::Script(ScriptError::new(
                        13,
                        "Type mismatch (an invalid Date)",
                    )));
                }
            }
        }
        if v.is_function() {
            return Err(Failed::Script(ScriptError::new(
                13,
                "Type mismatch (a function isn't a value the workbook can take)",
            )));
        }
        let mut fields = Vec::new();
        for prop in obj.props::<String, Js>() {
            let (k, x) = prop?;
            fields.push((k, from_js(ctx, &x, Value::Empty)?));
        }
        return Ok(Value::Record(Rc::new(Record {
            type_name: "Object".to_owned(),
            fields,
        })));
    }
    Err(Failed::Script(ScriptError::runtime(13)))
}
