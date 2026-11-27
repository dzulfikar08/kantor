//! A workbook's code as an editor (the Macro Editor, Alt+F11) shows and
//! changes it: the VBA project's modules and the JavaScript and Python
//! scripts, as one list ([`code_modules`]); written back with [`apply`];
//! checked before a run ([`diagnostics`]); the macro under the caret
//! ([`macro_at`]).
//!
//! Writing goes through `kantor_vba` (the project is rewritten without its
//! compiled caches, so Excel compiles it from source) and
//! `Workbook::set_vba_project` / `set_scripts`: the workbook becomes
//! unsaved, and a save as `.xlsm` writes the project. A workbook without a
//! project shows `ThisWorkbook` and a module per sheet, empty; the project
//! is made (with those document modules, as Excel's always has) when a
//! module gets code.

use kantor_script::vba::{ModuleKind as ScriptKind, Project};
use sheets_engine::Workbook;
use sheets_engine::scripts::{Script, ScriptLanguage};
use sheets_engine::vba::{ModuleKind, ModuleSpec, VbaProject};

pub use kantor_script::vba::parser::Diagnostic;

/// What a module is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CodeKind {
    /// `ThisWorkbook`, a sheet's module.
    Document,
    Standard,
    Class,
    /// A UserForm's code (its designer is kept; not edited here).
    Form,
    JavaScript,
    Python,
}

impl CodeKind {
    pub fn is_vba(self) -> bool {
        !matches!(self, CodeKind::JavaScript | CodeKind::Python)
    }

    fn vba(self) -> Option<ModuleKind> {
        Some(match self {
            CodeKind::Document => ModuleKind::Document,
            CodeKind::Standard => ModuleKind::Standard,
            CodeKind::Class => ModuleKind::Class,
            CodeKind::Form => ModuleKind::Form,
            CodeKind::JavaScript | CodeKind::Python => return None,
        })
    }

    fn of_vba(kind: ModuleKind) -> CodeKind {
        match kind {
            ModuleKind::Document => CodeKind::Document,
            ModuleKind::Standard => CodeKind::Standard,
            ModuleKind::Class => CodeKind::Class,
            ModuleKind::Form => CodeKind::Form,
        }
    }

    fn language(self) -> Option<ScriptLanguage> {
        match self {
            CodeKind::JavaScript => Some(ScriptLanguage::JavaScript),
            CodeKind::Python => Some(ScriptLanguage::Python),
            _ => None,
        }
    }

    /// Whether modules of this kind can be renamed and removed (document
    /// modules belong to sheets; forms to their designers).
    pub fn can_rename(self) -> bool {
        !matches!(self, CodeKind::Document | CodeKind::Form)
    }
}

/// One module as the editor holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeModule {
    pub name: String,
    pub kind: CodeKind,
    /// The code as the Visual Basic Editor shows it (no `Attribute`
    /// lines), lines ending in `\n`.
    pub code: String,
    /// For a document module: the sheet it belongs to (`None`: the
    /// workbook's, or not known).
    pub sheet: Option<String>,
}

impl CodeModule {
    pub fn new(name: &str, kind: CodeKind, code: &str) -> CodeModule {
        CodeModule {
            name: name.to_owned(),
            kind,
            code: code.to_owned(),
            sheet: None,
        }
    }
}

/// The workbook's modules in the editor's order: `ThisWorkbook`, the sheet
/// modules, standard modules, class modules, UserForms, then scripts. A
/// workbook without a VBA project gets empty `ThisWorkbook` and sheet
/// modules. `Err`: the project or the scripts can't be read (editing them
/// would lose them).
pub fn code_modules(book: &Workbook) -> Result<Vec<CodeModule>, String> {
    let scripts = book.scripts()?;
    let sheets = book.sheet_names();
    let mut out: Vec<CodeModule> = Vec::new();
    match book.vba_project() {
        Some(Ok(p)) => {
            let named = book.sheet_code_names();
            let docs: Vec<_> = p
                .modules()
                .iter()
                .filter(|m| m.kind == ModuleKind::Document)
                .collect();
            let is_book = |name: &str| name.eq_ignore_ascii_case("ThisWorkbook");
            // Sheet modules by the file's code names, else in order.
            let mut by_order = sheets.iter();
            for m in docs.iter().filter(|m| is_book(&m.name)) {
                out.push(CodeModule::new(&m.name, CodeKind::Document, &m.code));
            }
            for m in docs.iter().filter(|m| !is_book(&m.name)) {
                let sheet = if named.is_empty() {
                    by_order.next().cloned()
                } else {
                    named
                        .iter()
                        .find(|(_, c)| c.eq_ignore_ascii_case(&m.name))
                        .map(|(s, _)| s.clone())
                };
                let mut c = CodeModule::new(&m.name, CodeKind::Document, &m.code);
                c.sheet = sheet;
                out.push(c);
            }
            for kind in [ModuleKind::Standard, ModuleKind::Class, ModuleKind::Form] {
                for m in p.modules().iter().filter(|m| m.kind == kind) {
                    if let Some(e) = &m.error {
                        return Err(format!("module {} can't be read: {e}", m.name));
                    }
                    out.push(CodeModule::new(&m.name, CodeKind::of_vba(kind), &m.code));
                }
            }
        }
        Some(Err(e)) => return Err(e),
        None => {
            out.push(CodeModule::new("ThisWorkbook", CodeKind::Document, ""));
            let named = book.sheet_code_names();
            for (i, s) in sheets.iter().enumerate() {
                let name = named
                    .iter()
                    .find(|(n, _)| n == s)
                    .map(|(_, c)| c.clone())
                    .unwrap_or_else(|| format!("Sheet{}", i + 1));
                let mut c = CodeModule::new(&name, CodeKind::Document, "");
                c.sheet = Some(s.clone());
                out.push(c);
            }
        }
    }
    for s in scripts {
        let kind = match s.language {
            ScriptLanguage::JavaScript => CodeKind::JavaScript,
            ScriptLanguage::Python => CodeKind::Python,
        };
        out.push(CodeModule::new(&s.name, kind, &s.source));
    }
    Ok(out)
}

/// Whether `name` can name a new module in `modules`: a VBA module name
/// (letter, then letters, digits or `_`, ≤ 31 characters) no other module
/// has (any case). `None`: fine; `Some(why)`.
pub fn name_problem(modules: &[CodeModule], name: &str, except: Option<usize>) -> Option<String> {
    if !sheets_engine::vba::valid_module_name(name) {
        return Some(
            "A module name starts with a letter and has only letters, digits and _ (up to 31)"
                .into(),
        );
    }
    let taken = modules
        .iter()
        .enumerate()
        .any(|(i, m)| Some(i) != except && m.name.eq_ignore_ascii_case(name));
    taken.then(|| "Another module has that name".into())
}

/// A free name `Module1`, `Class2`… for a new module of `kind`.
pub fn free_name(modules: &[CodeModule], kind: CodeKind) -> String {
    let stem = match kind {
        CodeKind::Class => "Class",
        CodeKind::JavaScript => "Script",
        CodeKind::Python => "PyScript",
        _ => "Module",
    };
    (1..)
        .map(|n| format!("{stem}{n}"))
        .find(|n| !modules.iter().any(|m| m.name.eq_ignore_ascii_case(n)))
        .unwrap_or_default()
}

/// Writes `modules` into `book`: the VBA project (rewritten, made, or
/// removed when nothing is left) and the scripts. Only what changed is set,
/// so an editor can call it after every change. `Ok(true)`: something
/// changed.
pub fn apply(book: &mut Workbook, modules: &[CodeModule]) -> Result<bool, String> {
    let mut changed = false;
    // VBA.
    let specs: Vec<ModuleSpec> = modules
        .iter()
        .filter_map(|m| Some(ModuleSpec::new(&m.name, m.kind.vba()?, &m.code)))
        .collect();
    let current = match book.vba_project() {
        Some(Ok(p)) => Some(p),
        Some(Err(e)) => return Err(e),
        None => None,
    };
    match current {
        Some(p) => {
            if !same_project(&p, &specs) {
                let bytes = if specs.is_empty() {
                    None
                } else {
                    Some(p.with_modules(&specs).map_err(|e| e.to_string())?)
                };
                book.set_vba_project(bytes);
                changed = true;
            }
        }
        None => {
            let has_code = specs
                .iter()
                .any(|s| s.kind != ModuleKind::Document || !s.code.trim().is_empty());
            if has_code {
                let bytes = sheets_engine::vba::new_project(&specs).map_err(|e| e.to_string())?;
                book.set_vba_project(Some(bytes));
                changed = true;
            }
        }
    }
    // Scripts.
    let scripts: Vec<Script> = modules
        .iter()
        .filter_map(|m| Some(Script::new(&m.name, m.kind.language()?, &m.code)))
        .collect();
    if book.scripts()? != scripts {
        book.set_scripts(scripts);
        changed = true;
    }
    Ok(changed)
}

/// Whether `specs` are `p`'s modules as they are (names, order, code).
fn same_project(p: &VbaProject, specs: &[ModuleSpec]) -> bool {
    let norm = |s: &str| s.replace("\r\n", "\n").trim_end_matches('\n').to_owned();
    p.modules().len() == specs.len()
        && specs.iter().all(|s| {
            p.module(&s.name)
                .is_some_and(|m| m.name == s.name && norm(&m.code) == norm(&s.code))
        })
}

/// The problems to show in `module` before it runs (VBA: lines that don't
/// parse, statements Kantor doesn't run, the error that keeps the module
/// from compiling). Lines count from 1 in [`CodeModule::code`].
pub fn diagnostics(module: &CodeModule) -> Vec<Diagnostic> {
    let kind = match module.kind {
        CodeKind::Document => ScriptKind::Document,
        CodeKind::Standard => ScriptKind::Standard,
        CodeKind::Class => ScriptKind::Class,
        CodeKind::Form => ScriptKind::Form,
        CodeKind::JavaScript | CodeKind::Python => return Vec::new(),
    };
    Project::check(kind, &module.code)
}

/// The macro the caret is in (`line`, 0-based): `Module.Macro` for a public
/// `Sub` without arguments (or a script's top-level function without
/// parameters). `Err(name)`: the caret is in procedure `name`, which isn't
/// a macro (it has arguments, or is a Function or Private); `Err("")`: in
/// no procedure.
pub fn macro_at(module: &CodeModule, line: usize) -> Result<String, String> {
    match module.kind.language() {
        Some(language) => {
            let script = Script::new(&module.name, language, &module.code);
            let f = crate::scripts::functions(&script)
                .into_iter()
                .find(|f| f.line <= line && line <= f.end_line)
                .ok_or_else(String::new)?;
            if f.is_macro() {
                Ok(format!("{}.{}", module.name, f.name))
            } else {
                Err(f.name)
            }
        }
        None => {
            let p = sheets_engine::vba::scan_procedures(&module.code)
                .into_iter()
                .find(|p| p.line <= line && line <= p.end_line)
                .ok_or_else(String::new)?;
            if p.is_macro() && module.kind != CodeKind::Class && module.kind != CodeKind::Form {
                Ok(format!("{}.{}", module.name, p.name))
            } else {
                Err(p.name)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_macros() {
        let m = CodeModule::new(
            "Module1",
            CodeKind::Standard,
            "Option Explicit\n\nSub Go()\n    MsgBox 1\nEnd Sub\n\nFunction F(x)\nEnd Function\n",
        );
        assert_eq!(macro_at(&m, 3).as_deref(), Ok("Module1.Go"));
        assert_eq!(macro_at(&m, 6), Err("F".into()));
        assert_eq!(macro_at(&m, 0), Err(String::new()));
        let js = CodeModule::new(
            "Report",
            CodeKind::JavaScript,
            "function go() {\n  Range('A1').Value = 1;\n}\n",
        );
        assert_eq!(macro_at(&js, 1).as_deref(), Ok("Report.go"));
    }

    #[test]
    fn names() {
        let list = vec![
            CodeModule::new("ThisWorkbook", CodeKind::Document, ""),
            CodeModule::new("Module1", CodeKind::Standard, ""),
        ];
        assert_eq!(free_name(&list, CodeKind::Standard), "Module2");
        assert_eq!(free_name(&list, CodeKind::Class), "Class1");
        assert!(name_problem(&list, "module1", None).is_some());
        assert!(name_problem(&list, "Module1", Some(1)).is_none());
        assert!(name_problem(&list, "2nd", None).is_some());
        assert!(name_problem(&list, "Tools_2", None).is_none());
    }
}
