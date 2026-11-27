//! `Application.OnTime`, `Application.Wait` and `Application.Undo`.
//!
//! - `OnTime EarliestTime, Procedure[, LatestTime][, Schedule]` keeps a
//!   schedule in the run's app; the engine keeps it between runs
//!   ([`crate::MacroEngine::due`]), and the host runs the macro when it's
//!   due while the workbook is open. `Schedule:=False` removes the schedule
//!   with the same time and procedure (error 1004 when there is none, as in
//!   Excel).
//! - `Wait Time` sleeps until then in short slices, so Stop (the run's
//!   cancel flag) ends it with error 18.
//! - `Undo` takes back the user's last action, only from a macro that
//!   hasn't changed anything yet (an event handler rejecting an edit).

use std::rc::Rc;
use std::time::Duration;

use kantor_script::{Arg, ScriptError, Value};

use crate::engine::Timer;
use crate::session::Session;
use crate::values::*;

/// The members above; `None` for any other.
pub fn application_member(
    s: &Rc<Session>,
    lower: &str,
    args: &[Arg],
) -> Option<Result<Value, ScriptError>> {
    Some(match lower {
        "ontime" => on_time(s, args),
        "wait" => wait(s, args),
        "undo" => undo(s),
        _ => return None,
    })
}

fn date_arg(v: &Value) -> Result<f64, ScriptError> {
    match scalar(v)? {
        Value::Date(d) => Ok(d),
        Value::String(t) => {
            kantor_script::vba::convert::to_f64(&Value::String(t.clone())).or_else(|_| {
                kantor_script::vba::convert::convert(
                    &Value::String(t),
                    kantor_script::vba::convert::Scalar::Date,
                )
                .and_then(|d| kantor_script::vba::convert::to_f64(&d))
            })
        }
        v => to_f64(&v),
    }
}

fn on_time(s: &Rc<Session>, args: &[Arg]) -> Result<Value, ScriptError> {
    let at = date_arg(need(args, 0, "EarliestTime")?)?;
    let name = to_str(need(args, 1, "Procedure")?)?;
    let latest = match arg(args, 2, "LatestTime") {
        Some(v) if !v.is_missing() => Some(date_arg(v)?),
        _ => None,
    };
    let schedule = arg_bool(args, 3, "Schedule")?.unwrap_or(true);
    let this = crate::books::this(s);
    let app = this
        .app
        .borrow()
        .clone()
        .ok_or_else(|| ScriptError::unsupported("Application.OnTime here"))?;
    let mut timers = app.timers.borrow_mut();
    if schedule {
        timers.push(Timer {
            at,
            name: crate::assigned_macro(&name).unwrap_or(name),
            latest,
        });
        return Ok(Value::Empty);
    }
    let wanted = crate::assigned_macro(&name).unwrap_or(name);
    // Times compare to the second (a schedule made with Now + TimeValue).
    let same_time = |t: &Timer| (t.at - at).abs() < 0.5 / 86_400.0;
    match timers
        .iter()
        .position(|t| same_time(t) && t.name.eq_ignore_ascii_case(&wanted))
    {
        Some(i) => {
            timers.remove(i);
            Ok(Value::Empty)
        }
        None => Err(err1004("Method 'OnTime' of object '_Application' failed")),
    }
}

fn wait(s: &Rc<Session>, args: &[Arg]) -> Result<Value, ScriptError> {
    let until = date_arg(need(args, 0, "Time")?)?;
    let cancel = s.cancel.borrow().clone();
    loop {
        let now = crate::run::local_now();
        if now >= until {
            return Ok(Value::Boolean(true));
        }
        if cancel.is_cancelled() {
            return Err(ScriptError::interrupted(
                18,
                "Code execution has been interrupted",
            ));
        }
        let left = Duration::from_secs_f64(((until - now) * 86_400.0).max(0.0));
        std::thread::sleep(left.min(Duration::from_millis(50)));
    }
}

fn undo(s: &Rc<Session>) -> Result<Value, ScriptError> {
    let this = crate::books::this(s);
    let mut book = this.write()?;
    if book.undo_mark() == this.run_mark.get() && book.can_undo() {
        book.revert_edit().map_err(engine)?;
        // Undo stays one step: nothing to fold for the reverted edit.
        this.run_mark.set(book.undo_mark());
    }
    Ok(Value::Empty)
}
