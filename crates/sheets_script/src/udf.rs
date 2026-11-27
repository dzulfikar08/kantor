//! VBA `Public Function`s of standard modules used in cell formulas
//! (`=MyTax(A2)`), for the engine's user functions (`KANTOR_PATCHES.md`
//! §37).
//!
//! The engine may recalculate on any thread (the UI thread, a background
//! recalculation, a macro's worker), and the interpreter isn't `Send`, so
//! each thread compiles the project once into a thread-local cache keyed by
//! the project's id. A function gets its arguments as values (a range as a
//! 2-D array), runs with no workbook objects (`Range(...)` inside a UDF is
//! an error, which shows as `#VALUE!`) and a step budget, and its result
//! goes back to the cell; any error is `#VALUE!`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use kantor_script::vba::Project;
use kantor_script::{Array, NullHost, Value};
use sheets_engine::script::{KantorValue, UserFunctions};

use crate::run::{MacroProject, is_udf};
use crate::values::{from_cell, to_kantor};

/// Statements one call may run.
const UDF_STEPS: u64 = 5_000_000;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static PROJECTS: RefCell<HashMap<u64, Option<Project>>> = RefCell::new(HashMap::new());
}

/// A workbook's VBA functions as the engine's user functions.
pub struct VbaFunctions {
    id: u64,
    macros: MacroProject,
}

impl VbaFunctions {
    pub fn new(macros: MacroProject) -> Arc<VbaFunctions> {
        Arc::new(VbaFunctions {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            macros,
        })
    }
}

impl Drop for VbaFunctions {
    fn drop(&mut self) {
        // This thread's copy; other threads' stay until they end.
        let id = self.id;
        let _ = PROJECTS.try_with(|p| p.borrow_mut().remove(&id));
    }
}

fn arg_value(v: KantorValue) -> Value {
    match v {
        KantorValue::Array(rows) => Value::array(Array::from_rows(
            1,
            rows.into_iter()
                .map(|r| r.into_iter().map(|x| from_cell(x, false)).collect())
                .collect(),
        )),
        v => from_cell(v, false),
    }
}

impl UserFunctions for VbaFunctions {
    fn call(&self, name: &str, args: Vec<KantorValue>) -> Option<KantorValue> {
        PROJECTS.with(|projects| {
            let mut projects = projects.try_borrow_mut().ok()?;
            let project = projects.entry(self.id).or_insert_with(|| {
                let (mut p, _) = self.macros.compile();
                p.set_step_limit(Some(UDF_STEPS));
                Some(p)
            });
            let project = project.as_mut()?;
            if !is_udf(project, name) {
                return None;
            }
            let args: Vec<Value> = args.into_iter().map(arg_value).collect();
            Some(match project.call_function(name, &args, &mut NullHost) {
                Ok(v) => match v {
                    // A function that returns nothing shows 0, as in Excel.
                    Value::Empty => KantorValue::Number(0.0),
                    v => to_kantor(&v),
                },
                Err(e) => {
                    log::debug!("{name}: {e}");
                    KantorValue::Error("#VALUE!".to_owned())
                }
            })
        })
    }
}

/// Turns a workbook's VBA functions on (`Some`) or off in its formulas.
/// Doesn't recalculate.
pub fn enable_functions(book: &mut sheets_engine::Workbook, macros: Option<&MacroProject>) {
    match macros {
        Some(m) => book.set_user_functions(Some(VbaFunctions::new(m.clone()))),
        None => book.set_user_functions(None),
    }
}
