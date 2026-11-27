//! A VBA interpreter.
//!
//! ```
//! use kantor_script::vba::{ModuleKind, Project};
//! use kantor_script::{NullHost, Value};
//!
//! let mut project = Project::new();
//! project
//!     .add_module("Module1", ModuleKind::Standard, "Function Twice(x)\r\n  Twice = x * 2\r\nEnd Function\r\n")
//!     .unwrap();
//! let v = project.call_function("Twice", &[Value::Long(21)], &mut NullHost).unwrap();
//! assert_eq!(v, Value::Long(42));
//! ```
//!
//! Source goes in per module ([`Project::add_module`]); [`Project::run`] runs a macro and
//! [`Project::call_function`] calls a function (a worksheet UDF later). Module-level variables
//! keep their values between runs until [`Project::reset`] or an `End` statement, as in VBA.
//! Global names the project doesn't define come from the [`Host`].
//!
//! UserForms: [`Project::add_form`] gives a form module its designer; `UserForm1.Show` then
//! shows it through [`Host::form_event`] and runs its event handlers (see [`crate::forms`]).
//!
//! The rules follow the VBA language specification (MS-VBAL); comments cite its sections.

pub mod ast;
pub mod builtins;
pub mod compile;
pub mod convert;
pub mod format;
mod forms;
mod interp;
pub mod lexer;
pub mod objects;
pub mod parser;
pub mod preprocess;

use std::rc::Rc;

pub use ast::ProcKind;
pub use compile::ModuleKind;
pub use objects::{Collection, Dictionary, ErrObject};

use compile::Module;
use interp::{ArgIn, Interp, State};

use crate::object::{ErrorKind, Host, Location, ScriptError, Value};

/// A procedure, as listed for a macro picker.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcInfo {
    pub module: String,
    pub name: String,
    pub kind: ProcKind,
    pub public: bool,
    pub params: usize,
    pub required_params: usize,
    /// A public `Sub` without required parameters in a standard or document module: what Excel
    /// lists under Macros.
    pub is_macro: bool,
    pub line: u32,
}

/// A set of modules that run together (a workbook's VBA project).
pub struct Project {
    modules: Vec<Rc<Module>>,
    state: State,
}

impl Default for Project {
    fn default() -> Project {
        Project::new()
    }
}

impl Project {
    pub fn new() -> Project {
        Project {
            modules: Vec::new(),
            state: State::new(),
        }
    }

    /// Adds a module, or replaces the module with the same name. Fails with a syntax error
    /// (with the line) when the module's structure doesn't parse; single bad or unsupported
    /// lines only fail when they run.
    pub fn add_module(
        &mut self,
        name: &str,
        kind: ModuleKind,
        source: &str,
    ) -> Result<(), ScriptError> {
        let existing = self
            .modules
            .iter()
            .position(|m| m.name.eq_ignore_ascii_case(name));
        let idx = existing.unwrap_or(self.modules.len());
        let with_module = |mut e: ScriptError| {
            let line = e.location.as_ref().map_or(0, |l| l.line);
            let procedure = e
                .location
                .as_ref()
                .map(|l| l.procedure.clone())
                .unwrap_or_default();
            e.location = Some(Location {
                module: name.to_string(),
                procedure,
                line,
            });
            e
        };
        let def = parser::parse_module(source).map_err(with_module)?;
        let module = compile::compile_module(idx, name, kind, def).map_err(with_module)?;
        match existing {
            Some(i) => {
                self.modules[i] = Rc::new(module);
                self.state.reset();
            }
            None => self.modules.push(Rc::new(module)),
        }
        Ok(())
    }

    pub fn module_names(&self) -> Vec<String> {
        self.modules.iter().map(|m| m.name.clone()).collect()
    }

    /// Every procedure, in module and source order.
    pub fn procedures(&self) -> Vec<ProcInfo> {
        let mut out = Vec::new();
        for m in &self.modules {
            for p in &m.order {
                let required = p.params.iter().filter(|x| !x.optional).count();
                out.push(ProcInfo {
                    module: m.name.clone(),
                    name: p.name.text.to_string(),
                    kind: p.kind,
                    public: p.public,
                    params: p.params.len(),
                    required_params: required,
                    is_macro: p.kind == ProcKind::Sub
                        && p.public
                        && required == 0
                        && matches!(m.kind, ModuleKind::Standard | ModuleKind::Document),
                    line: p.line,
                });
            }
        }
        out
    }

    /// The designer of form module `design.name` (its controls): what `UserForm1.Show` shows.
    /// A form module without one shows an empty form.
    pub fn add_form(&mut self, design: crate::forms::FormDesign) {
        self.state
            .forms
            .insert(design.name.to_lowercase(), Rc::new(design));
    }

    /// Makes a constant visible to every module (the object model's `xlUp`, `xlValues`…).
    pub fn register_constant(&mut self, name: &str, value: Value) {
        self.state.constants.insert(name.to_lowercase(), value);
    }

    /// Stops a run after this many statements (error 18, not trappable). `None`: no limit.
    pub fn set_step_limit(&mut self, limit: Option<u64>) {
        self.state.max_steps = limit;
    }

    /// The deepest procedure nesting before "Out of stack space" (error 28).
    pub fn set_max_call_depth(&mut self, depth: usize) {
        self.state.max_depth = depth;
    }

    /// Clears module-level variables, statics and `Err` (the editor's Reset button).
    pub fn reset(&mut self) {
        self.state.reset();
    }

    fn find(
        &self,
        name: &str,
        function_only: bool,
    ) -> Result<(Rc<Module>, Rc<compile::Proc>), ScriptError> {
        let not_found = || ScriptError::new(35, format!("Macro not found: {name}"));
        let (module, proc_name) = match name.rsplit_once('.') {
            Some((m, p)) => (Some(m.to_lowercase()), p.to_lowercase()),
            None => (None, name.to_lowercase()),
        };
        let candidates = self.modules.iter().filter(|m| match &module {
            Some(key) => &*m.key == key,
            None => matches!(m.kind, ModuleKind::Standard | ModuleKind::Document),
        });
        let mut ordered: Vec<&Rc<Module>> = candidates.collect();
        ordered.sort_by_key(|m| m.kind != ModuleKind::Standard);
        for m in ordered {
            if let Some(g) = m.procs.get(proc_name.as_str()) {
                let p = if function_only {
                    g.sub.as_ref().filter(|p| p.kind == ProcKind::Function)
                } else {
                    g.sub.as_ref().or(g.get.as_ref())
                };
                if let Some(p) = p {
                    if matches!(m.kind, ModuleKind::Class | ModuleKind::Form) {
                        return Err(ScriptError::new(
                            35,
                            format!("{name} is in a class module; create an instance to call it"),
                        ));
                    }
                    return Ok((m.clone(), p.clone()));
                }
            }
        }
        Err(not_found())
    }

    fn invoke(
        &mut self,
        module: Rc<Module>,
        proc: Rc<compile::Proc>,
        args: &[Value],
        host: &mut dyn Host,
    ) -> Result<Value, ScriptError> {
        self.state.steps = 0;
        let args = args.iter().cloned().map(ArgIn::value).collect();
        let (result, halted) = {
            let mut it = Interp {
                modules: &self.modules,
                st: &mut self.state,
                host,
                depth: 0,
            };
            let mut r = it.call_proc(&module, &proc, args, None, None);
            // Forms shown modeless stay up once the macro is done; their events run now.
            if r.is_ok()
                && let Err(e) = it.run_modeless()
            {
                r = Err(e);
            }
            it.flush_print();
            match r {
                Err(e) if e.kind == ErrorKind::Halt => (Ok(Value::Empty), true),
                other => (other, false),
            }
        };
        if halted {
            // `End` resets every module-level variable (VBA reference, "End statement").
            self.state.reset();
        }
        result
    }

    /// Runs a procedure by name (`"Macro1"` or `"Module1.Macro1"`). Arguments are passed by
    /// value; a Function's result is returned.
    pub fn run(
        &mut self,
        name: &str,
        args: &[Value],
        host: &mut dyn Host,
    ) -> Result<Value, ScriptError> {
        let (m, p) = self.find(name, false)?;
        self.invoke(m, p, args, host)
    }

    /// Calls a public Function of a standard module, as a worksheet formula calls a UDF.
    pub fn call_function(
        &mut self,
        name: &str,
        args: &[Value],
        host: &mut dyn Host,
    ) -> Result<Value, ScriptError> {
        let (m, p) = self.find(name, true)?;
        if !p.public || m.kind != ModuleKind::Standard {
            return Err(ScriptError::new(
                35,
                format!("{name} is not a public function of a standard module"),
            ));
        }
        self.invoke(m, p, args, host)
    }
}
