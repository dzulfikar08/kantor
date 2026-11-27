//! Events and the calls a host routes back into the project:
//!
//! - `WithEvents` variables (MS-VBAL §5.2.3.1.3): a module-level variable of a class or
//!   document module declared `WithEvents` is a *sink*. When the object it holds raises event
//!   `E`, the module's `<variable>_<E>` procedure runs (with `Me` the instance that holds the
//!   variable). Sinks are registered when their variables come to exist: a document or standard
//!   module's when its variables are first used, a class instance's when it is created (kept
//!   weakly, so a sink doesn't keep its instance alive).
//! - `RaiseEvent` (§5.4.2.20) in a class raises one of its declared `Event`s to the sinks that
//!   hold this instance; arguments are passed `ByRef` where the handler says so, so a handler
//!   can set `Cancel`.
//! - Host events ([`HostCall::Event`]): the host raises an event of one of its objects
//!   (`Application.SheetChange`), delivered to sinks holding an object of that type name.
//! - [`HostCall::Proc`]: a handler by name (`Sheet1.Worksheet_Change`), run between two
//!   statements when the script's own edits fire an event.
//! - `Application.Run` ([`ScriptMember::Run`]): a procedure of the project by name.

use std::rc::{Rc, Weak};

use super::ast::{ArgExpr, Ident};
use super::compile::{Module, ModuleKind, Proc};
use super::interp::{ArgIn, Interp, Place, R};
use super::objects::{Instance, Scope};
use crate::object::*;

/// A sink as of now: its module and, for an instance, `Me` and its variables.
type LiveSink = (Rc<Module>, Option<(ObjectRef, Rc<Scope>)>);

/// A module whose `WithEvents` variables receive events.
pub(crate) struct Sink {
    pub module: Rc<Module>,
    /// For a class instance: the instance and its variables (weak); `None` for a document or
    /// standard module (its variables are the module's).
    pub owner: Option<(Weak<dyn Dispatch>, Weak<Scope>)>,
}

impl Interp<'_> {
    /// Registers the `WithEvents` variables of an instance just created.
    pub(crate) fn register_instance_sink(
        &mut self,
        m: &Rc<Module>,
        obj: &ObjectRef,
        fields: &Rc<Scope>,
    ) {
        if m.with_events.is_empty() {
            return;
        }
        self.st.sinks.retain(|s| match &s.owner {
            Some((o, _)) => o.strong_count() > 0,
            None => true,
        });
        self.st.sinks.push(Sink {
            module: m.clone(),
            owner: Some((Rc::downgrade(obj), Rc::downgrade(fields))),
        });
    }

    /// Registers a document or standard module's `WithEvents` variables.
    pub(crate) fn register_module_sink(&mut self, m: &Rc<Module>) {
        if !m.with_events.is_empty() {
            self.st.sinks.push(Sink {
                module: m.clone(),
                owner: None,
            });
        }
    }

    /// Calls `<var>_<event>` for every `WithEvents` variable holding an object `matches`
    /// accepts. `args` are re-read from their places before each handler, so a later handler
    /// sees what an earlier one set (`Cancel`).
    pub(crate) fn raise_to_sinks(
        &mut self,
        matches: &dyn Fn(&ObjectRef) -> bool,
        event: &str,
        args: &[(Value, Option<Place>)],
    ) -> R<()> {
        // Handlers may create instances (new sinks): go over the ones there now.
        let sinks: Vec<LiveSink> = self
            .st
            .sinks
            .iter()
            .filter_map(|s| match &s.owner {
                None => Some((s.module.clone(), None)),
                Some((o, f)) => Some((s.module.clone(), Some((o.upgrade()?, f.upgrade()?)))),
            })
            .collect();
        let event = event.to_lowercase();
        for (module, owner) in sinks {
            let scope = match &owner {
                Some((_, fields)) => fields.clone(),
                None => match self.st.scopes.get(module.idx).cloned().flatten() {
                    Some(s) => s,
                    None => continue,
                },
            };
            for var in module.with_events.clone() {
                let held = {
                    let scope = scope.borrow();
                    match scope.get(&var).map(|s| s.borrow().value.clone()) {
                        Some(Value::Object(Some(o))) => Some(o),
                        _ => None,
                    }
                };
                let Some(held) = held else { continue };
                if !matches(&held) {
                    continue;
                }
                let key = format!("{var}_{event}");
                let Some(p) = module.procs.get(key.as_str()).and_then(|g| g.sub.clone()) else {
                    continue;
                };
                let mut a = Vec::with_capacity(args.len());
                for (v, place) in args {
                    let value = match place {
                        Some(p) => self.read_place(p)?,
                        None => v.clone(),
                    };
                    a.push(ArgIn {
                        name: None,
                        value,
                        place: place.clone(),
                    });
                }
                self.call_proc(&module, &p, a, owner.clone(), None)?;
            }
        }
        Ok(())
    }

    /// `RaiseEvent Name(args)` inside a class: to the sinks holding this instance.
    pub(crate) fn raise_event_stmt(
        &mut self,
        name: &Ident,
        args: &[ArgExpr],
        module: &Rc<Module>,
        me: Option<ObjectRef>,
        f: &mut super::interp::Frame,
    ) -> R<()> {
        if !module.events.contains(&name.key) {
            return Err(ScriptError::syntax(format!(
                "Event not found: {}",
                name.text
            )));
        }
        let Some(me) = me else {
            return Err(ScriptError::syntax("Invalid use of RaiseEvent"));
        };
        let evaluated = self.eval_args_in(args, f)?;
        let args: Vec<(Value, Option<Place>)> =
            evaluated.into_iter().map(|a| (a.value, a.place)).collect();
        self.raise_to_sinks(
            &|o: &ObjectRef| Value::same_object(o, &me),
            &name.key,
            &args,
        )
    }

    /// A procedure by name, as `Application.Run` and hosts name them: `Macro1`,
    /// `Module1.Macro1`, `Sheet1.Worksheet_Change`, `'Book1.xlsm'!Macro1`. Any procedure of a
    /// standard or document module (private ones too, as Excel's `Run` allows).
    pub(crate) fn find_proc(&self, name: &str) -> Option<(Rc<Module>, Rc<Proc>)> {
        let name = match name.rfind('!') {
            Some(i) => &name[i + 1..],
            None => name,
        };
        let name = name.trim().trim_matches('\'').trim().to_lowercase();
        let (module, proc_name) = match name.rsplit_once('.') {
            Some((m, p)) => (Some(m.to_owned()), p.to_owned()),
            None => (None, name),
        };
        let mut candidates: Vec<&Rc<Module>> = self
            .modules
            .iter()
            .filter(|m| match &module {
                Some(k) => &*m.key == k.as_str(),
                None => matches!(m.kind, ModuleKind::Standard | ModuleKind::Document),
            })
            .filter(|m| matches!(m.kind, ModuleKind::Standard | ModuleKind::Document))
            .collect();
        candidates.sort_by_key(|m| m.kind != ModuleKind::Standard);
        candidates.into_iter().find_map(|m| {
            let g = m.procs.get(proc_name.as_str())?;
            let p = g.sub.clone().or_else(|| g.get.clone())?;
            Some((m.clone(), p))
        })
    }

    /// `Application.Run "Macro", args…`.
    pub(crate) fn app_run(&mut self, args: Vec<Arg>) -> R<Value> {
        let mut rest = Vec::new();
        let mut macro_name = None;
        for (i, a) in args.into_iter().enumerate() {
            match a.name.as_deref().map(str::to_ascii_lowercase).as_deref() {
                Some("macro") => macro_name = Some(a.value),
                _ if i == 0 && a.name.is_none() => macro_name = Some(a.value),
                _ => rest.push(a.value),
            }
        }
        let name = match macro_name {
            Some(v) => super::convert::to_str(&self.deref(v)?)?,
            None => return Err(ScriptError::new(449, "Argument not optional: Macro")),
        };
        let Some((m, p)) = self.find_proc(&name) else {
            return Err(ScriptError::new(
                1004,
                format!(
                    "Cannot run the macro '{name}'. The macro may not be available in this workbook or all macros may be disabled."
                ),
            ));
        };
        let a = rest
            .into_iter()
            .filter(|v| !v.is_missing())
            .map(ArgIn::value)
            .collect();
        self.call_proc(&m, &p, a, None, None)
    }

    /// Runs what the host asks for between statements. An error in a handler isn't the
    /// caller's to trap (VBA reports it in the handler): it stops the run.
    pub(crate) fn host_calls(&mut self) -> R<()> {
        let calls = self.host.take_calls();
        if calls.is_empty() {
            return Ok(());
        }
        self.run_host_calls(calls)
    }

    #[inline(never)]
    fn run_host_calls(&mut self, calls: Vec<HostCall>) -> R<()> {
        for call in calls {
            let r = match call {
                HostCall::Proc { name, args } => match self.find_proc(&name) {
                    Some((m, p)) => {
                        let a = args.into_iter().map(ArgIn::value).collect();
                        self.call_proc(&m, &p, a, None, None).map(|_| ())
                    }
                    None => Ok(()),
                },
                HostCall::Event {
                    source_type,
                    event,
                    args,
                } => {
                    let args: Vec<(Value, Option<Place>)> =
                        args.into_iter().map(|v| (v, None)).collect();
                    self.raise_to_sinks(
                        &|o: &ObjectRef| o.type_name().eq_ignore_ascii_case(&source_type),
                        &event,
                        &args,
                    )
                }
            };
            if let Err(mut e) = r {
                if e.is_trappable() {
                    e.kind = ErrorKind::Interrupted;
                }
                return Err(e);
            }
        }
        Ok(())
    }

    /// Whether instance `o` implements interface `iface` (lower case).
    pub(crate) fn implements(o: &ObjectRef, iface: &str) -> bool {
        o.as_any()
            .and_then(|a| a.downcast_ref::<Instance>())
            .is_some_and(|i| i.module.implements.iter().any(|x| &**x == iface))
    }
}
