//! JavaScript and Python macros on the same object model as VBA.
//!
//! A workbook's scripts (`sheets_engine::scripts`, stored in the file) are
//! modules like VBA's: their top-level functions without parameters are
//! macros, listed with the VBA ones and run the same way (a run is one undo
//! step, on the same macro thread, with the same dialogs and Stop). The
//! languages bind to the objects of this crate through
//! [`kantor_script::Dispatch`]: `kantor_script_js` (QuickJS) and
//! `kantor_script_py` (RustPython, with the `python` feature). The host
//! ([`crate::run::SheetsHost`]) supplies the globals (`Range`, `Cells`,
//! `ActiveSheet`…), the constants (`xlUp`) and which members are methods
//! ([`is_method`]), since JavaScript and Python read a property when it is
//! named but call a method.
//!
//! Unlike the VBA project, scripts keep no state between runs: each run
//! evaluates the modules afresh.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use kantor_script::binding::FunctionInfo;
use kantor_script::{Host, ScriptError, Value};
use sheets_engine::scripts::{Script, ScriptLanguage};

use crate::run::{MacroInfo, MacroProject};

/// The top-level functions of a script.
pub fn functions(script: &Script) -> Vec<FunctionInfo> {
    match script.language {
        ScriptLanguage::JavaScript => kantor_script_js::functions(&script.source),
        ScriptLanguage::Python => python_functions(&script.source),
    }
}

#[cfg(feature = "python")]
fn python_functions(source: &str) -> Vec<FunctionInfo> {
    kantor_script_py::functions(source)
}

#[cfg(not(feature = "python"))]
fn python_functions(source: &str) -> Vec<FunctionInfo> {
    kantor_script::binding::python_functions(source)
}

/// The scripts' macros: top-level functions without parameters whose name
/// doesn't start with `_`.
pub fn macros(scripts: &[Script]) -> Vec<MacroInfo> {
    scripts
        .iter()
        .flat_map(|s| {
            functions(s)
                .into_iter()
                .filter(FunctionInfo::is_macro)
                .map(|f| MacroInfo {
                    module: s.name.clone(),
                    name: f.name,
                })
        })
        .collect()
}

/// Which script runs macro `name` (`Report.go`, or `go`): its language and
/// the qualified name. `None`: a VBA macro (a bare name the VBA project
/// defines is VBA's, as are names of no script).
pub(crate) fn find(macros: &MacroProject, name: &str) -> Option<(ScriptLanguage, String)> {
    let (module, func) = kantor_script::binding::split_target(name);
    if let Some(m) = module {
        let s = macros
            .scripts
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(m))?;
        let real = function_named(s, func).unwrap_or_else(|| func.to_owned());
        return Some((s.language, format!("{}.{real}", s.name)));
    }
    if !macros.modules.is_empty() {
        let (project, _) = macros.compile();
        if project
            .procedures()
            .iter()
            .any(|p| p.name.eq_ignore_ascii_case(func))
        {
            return None;
        }
    }
    macros.scripts.iter().find_map(|s| {
        function_named(s, func).map(|real| (s.language, format!("{}.{real}", s.name)))
    })
}

/// The function of `script` named `name`: that exact name, else one
/// differing only in case (a macro name typed as `Go` for `go`).
fn function_named(script: &Script, name: &str) -> Option<String> {
    let found = functions(script);
    found
        .iter()
        .find(|f| f.name == name)
        .or_else(|| found.iter().find(|f| f.name.eq_ignore_ascii_case(name)))
        .map(|f| f.name.clone())
}

/// Runs `target` (`Module.function`) of the scripts in `language`, with
/// `host` (the run's [`crate::run::SheetsHost`]).
pub(crate) fn run(
    macros: &MacroProject,
    language: ScriptLanguage,
    target: &str,
    host: Rc<RefCell<dyn Host>>,
    cancel: &kantor_script::CancelToken,
) -> Result<Value, ScriptError> {
    let modules: Vec<&Script> = macros
        .scripts
        .iter()
        .filter(|s| s.language == language)
        .collect();
    match language {
        ScriptLanguage::JavaScript => {
            let modules: Vec<kantor_script_js::Module> = modules
                .iter()
                .map(|s| kantor_script_js::Module {
                    name: s.name.clone(),
                    source: s.source.clone(),
                })
                .collect();
            let options = kantor_script_js::Options {
                cancel: cancel.clone(),
                ..Default::default()
            };
            kantor_script_js::run(&modules, target, &[], host, &options)
        }
        ScriptLanguage::Python => run_python(&modules, target, host, cancel),
    }
}

#[cfg(feature = "python")]
fn run_python(
    modules: &[&Script],
    target: &str,
    host: Rc<RefCell<dyn Host>>,
    cancel: &kantor_script::CancelToken,
) -> Result<Value, ScriptError> {
    let modules: Vec<kantor_script_py::Module> = modules
        .iter()
        .map(|s| kantor_script_py::Module {
            name: s.name.clone(),
            source: s.source.clone(),
        })
        .collect();
    let options = kantor_script_py::Options {
        cancel: cancel.clone(),
        ..Default::default()
    };
    kantor_script_py::run(&modules, target, &[], host, &options)
}

#[cfg(not(feature = "python"))]
fn run_python(
    _modules: &[&Script],
    _target: &str,
    _host: Rc<RefCell<dyn Host>>,
    _cancel: &kantor_script::CancelToken,
) -> Result<Value, ScriptError> {
    Err(ScriptError::unsupported(
        "Running Python macros in this build",
    ))
}

/// A constant of the object model (`xlUp`), any case.
pub(crate) fn constant(name: &str) -> Option<Value> {
    static TABLE: OnceLock<HashMap<String, i64>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        crate::run::all_constants()
            .map(|(n, v)| (n.to_ascii_lowercase(), v))
            .collect()
    });
    table
        .get(&name.to_ascii_lowercase())
        .map(|v| Value::Long(*v as i32))
}

/// Whether member `member` of a `type_name` object is a method (called with
/// parentheses in JavaScript and Python) rather than a property. Members
/// that need arguments are methods anyway (the bindings find out); this
/// lists the ones that work without arguments and act, or take optional
/// arguments that change what they give (`Offset`, `Address`).
pub(crate) fn is_method(type_name: &str, member: &str) -> bool {
    if type_name == "WorksheetFunction" {
        return true;
    }
    let m = member.to_ascii_lowercase();
    METHODS.binary_search(&m.as_str()).is_ok()
}

/// Members that are methods on every object that has them (sorted).
const METHODS: &[&str] = &[
    "activate",
    "add",
    "addaboveaverage",
    "addchart",
    "addchart2",
    "addcolorscale",
    "addcomment",
    "addcommentthreaded",
    "addconnector",
    "adddatabar",
    "adddatafield",
    "addfields",
    "addformcontrol",
    "addiconsetcondition",
    "addlabel",
    "addline",
    "addpicture",
    "addpolyline",
    "address",
    "addresslocal",
    "addshape",
    "addtextbox",
    "addtop10",
    "adduniquevalues",
    "apply",
    "applydatalabels",
    "autofill",
    "autofilter",
    "autofit",
    "borderaround",
    "bringtofront",
    "calculate",
    "calculatefull",
    "characters",
    "clear",
    "clearallfilters",
    "clearcomments",
    "clearcontents",
    "clearformats",
    "clearhyperlinks",
    "clearnotes",
    "clearoutline",
    "close",
    "copy",
    "copypicture",
    "cut",
    "delete",
    "duplicate",
    "evaluate",
    "export",
    "exportasfixedformat",
    "fillacross",
    "filldown",
    "fillleft",
    "fillright",
    "fillup",
    "find",
    "findnext",
    "findprevious",
    "flip",
    "follow",
    "followhyperlink",
    "getpivotdata",
    "goto",
    "group",
    "incrementleft",
    "incrementrotation",
    "incrementtop",
    "inputbox",
    "insert",
    "insertindent",
    "intersect",
    "justify",
    "merge",
    "modify",
    "move",
    "offset",
    "ontime",
    "paste",
    "pastespecial",
    "printout",
    "printpreview",
    "protect",
    "quit",
    "refresh",
    "refreshall",
    "refreshtable",
    "removeduplicates",
    "removesubtotal",
    "replace",
    "resize",
    "run",
    "save",
    "saveas",
    "savecopyas",
    "scaleheight",
    "scalewidth",
    "select",
    "sendtoback",
    "setfirstpriority",
    "setlastpriority",
    "setsourcedata",
    "showalldata",
    "sort",
    "specialcells",
    "subtotal",
    "texttocolumns",
    "undo",
    "ungroup",
    "union",
    "unmerge",
    "unprotect",
    "volatile",
    "wait",
    "zorder",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn methods_are_sorted_and_found() {
        let mut sorted = METHODS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, METHODS);
        assert!(is_method("Range", "ClearContents"));
        assert!(is_method("Range", "offset"));
        assert!(!is_method("Range", "Value"));
        assert!(is_method("WorksheetFunction", "Sum"));
        assert_eq!(constant("XLUP"), Some(Value::Long(-4162)));
    }
}
