//! A workbook's macros (its VBA project, and its JavaScript and Python
//! scripts) as JSON and text: `kantor sheets
//! macros`, the MCP tool `sheets_macros` and Paparan AI's `macros`. Kantor
//! tools don't run macros (only the user does, in Sheets after Enable
//! Content or with `kantor sheets macros run`): this is for reading them, so
//! the assistant (or the
//! user) can explain what one does or rebuild it with formulas.

use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use sheets_engine::Workbook;
use sheets_engine::scripts::{Script, ScriptLanguage};
use sheets_engine::vba::{Module, ReferenceKind, VbaProject};

/// What the JSON says about running them.
pub const NOTE: &str = "AI tools don't run macros: a macro is code from the file, and only the user runs it (Kantor Sheets after Enable Content, or kantor sheets macros run). Read the code to explain what a macro does, or rebuild it with formulas and sheets editing tools.";

/// What to list.
#[derive(Clone, Debug, Default)]
pub struct MacrosOpts {
    /// The source of this module (any case).
    pub module: Option<String>,
    /// The source of every module.
    pub code: bool,
}

/// The workbook's VBA project: `Ok(None)` when it has no macros.
pub fn project(book: &Workbook) -> Result<Option<VbaProject>> {
    match book.vba_project() {
        None => Ok(None),
        Some(Ok(p)) => Ok(Some(p)),
        Some(Err(e)) => Err(anyhow!("the workbook's VBA project can't be read: {e}")),
    }
}

fn module_json(m: &Module, code: bool) -> Value {
    let procedures: Vec<Value> = m
        .procedures()
        .iter()
        .map(|p| {
            json!({
                "name": p.name,
                "kind": p.kind.keyword(),
                "private": p.private,
                "arguments": p.has_arguments,
                "macro": p.is_macro(),
                "line": p.line + 1,
                "end_line": p.end_line + 1,
            })
        })
        .collect();
    let mut v = json!({
        "name": m.name,
        "kind": m.kind.key(),
        "lines": m.code.lines().count(),
        "procedures": procedures,
    });
    if code {
        v["code"] = json!(m.code);
        v["attributes"] = json!(m.attributes);
    }
    if let Some(e) = &m.error {
        v["error"] = json!(e);
    }
    v
}

/// The workbook's JavaScript and Python scripts (stored in the workbook).
fn scripts(book: &Workbook) -> Result<Vec<Script>> {
    book.scripts()
        .map_err(|e| anyhow!("the workbook's scripts can't be read: {e}"))
}

fn script_json(s: &Script, code: bool) -> Value {
    let keyword = match s.language {
        ScriptLanguage::JavaScript => "function",
        ScriptLanguage::Python => "def",
    };
    let procedures: Vec<Value> = sheets_script::scripts::functions(s)
        .iter()
        .map(|f| {
            json!({
                "name": f.name,
                "kind": keyword,
                "private": f.name.starts_with('_'),
                "arguments": f.params > 0,
                "macro": f.is_macro(),
                "line": f.line + 1,
                "end_line": f.end_line + 1,
            })
        })
        .collect();
    let mut v = json!({
        "name": s.name,
        "kind": s.language.key(),
        "language": s.language.label(),
        "lines": s.source.lines().count(),
        "procedures": procedures,
    });
    if code {
        v["code"] = json!(s.source);
    }
    v
}

/// The project as JSON (see [`MacrosOpts`]): the VBA modules, then the
/// JavaScript and Python scripts (`kind` `javascript` / `python`). Line
/// numbers count from 1 in each module's `code` (attribute lines left out,
/// as the Visual Basic Editor shows it).
pub fn json(book: &Workbook, opts: &MacrosOpts) -> Result<Value> {
    let p = project(book)?;
    let scripts = scripts(book)?;
    if p.is_none() && scripts.is_empty() {
        if let Some(m) = &opts.module {
            return Err(anyhow!("the workbook has no macros (no module {m})"));
        }
        return Ok(json!({"has_macros": false, "modules": []}));
    }
    if let Some(name) = &opts.module {
        let known = p.as_ref().is_some_and(|p| p.module(name).is_some())
            || scripts.iter().any(|s| s.name.eq_ignore_ascii_case(name));
        if !known {
            let names: Vec<&str> = p
                .iter()
                .flat_map(|p| p.modules().iter().map(|m| m.name.as_str()))
                .chain(scripts.iter().map(|s| s.name.as_str()))
                .collect();
            return Err(anyhow!(
                "there is no module {name} (modules: {})",
                names.join(", ")
            ));
        }
    }
    let wanted = |n: &str| {
        opts.module
            .as_deref()
            .is_none_or(|m| n.eq_ignore_ascii_case(m))
    };
    let code = opts.code || opts.module.is_some();
    let mut modules: Vec<Value> = p
        .iter()
        .flat_map(|p| p.modules().iter())
        .filter(|m| wanted(&m.name))
        .map(|m| module_json(m, code))
        .collect();
    modules.extend(
        scripts
            .iter()
            .filter(|s| wanted(&s.name))
            .map(|s| script_json(s, code)),
    );
    let mut v = json!({
        "has_macros": true,
        "modules": modules,
        "note": NOTE,
    });
    if let Some(p) = &p {
        let references: Vec<Value> = p
            .references
            .iter()
            .map(|r| {
                json!({
                    "name": r.name,
                    "kind": match r.kind {
                        ReferenceKind::Registered => "library",
                        ReferenceKind::Project => "project",
                        ReferenceKind::Control => "control",
                    },
                    "libid": r.libid,
                })
            })
            .collect();
        v["project"] = json!(p.name);
        v["codepage"] = json!(p.codepage);
        v["references"] = json!(references);
    }
    Ok(v)
}

/// The listing as text: modules and their procedures, then the scripts and
/// their functions.
pub fn text(book: &Workbook) -> Result<String> {
    let p = project(book)?;
    let scripts = scripts(book)?;
    if p.is_none() && scripts.is_empty() {
        return Ok("no macros\n".into());
    }
    let mut out = String::new();
    if let Some(p) = &p {
        out.push_str(&format!(
            "VBA project {} (code page {})\n",
            p.name, p.codepage
        ));
        for m in p.modules() {
            let lines = m.code.lines().count();
            out.push_str(&format!("\n{} ({}", m.name, m.kind.label()));
            if lines == 0 {
                out.push_str(", no code)\n");
            } else {
                out.push_str(&format!(", {lines} lines)\n"));
            }
            if let Some(e) = &m.error {
                out.push_str(&format!("  can't read its code: {e}\n"));
            }
            for p in m.procedures() {
                let mut head = format!(
                    "{}{} {}{}",
                    if p.private { "Private " } else { "" },
                    p.kind.keyword(),
                    p.name,
                    if p.has_arguments { "(…)" } else { "()" }
                );
                if p.is_macro() && m.kind != sheets_engine::vba::ModuleKind::Class {
                    head.push_str("  [macro]");
                }
                out.push_str(&format!(
                    "  {head:<48} lines {}-{}\n",
                    p.line + 1,
                    p.end_line + 1
                ));
            }
        }
    }
    if !scripts.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("Scripts (stored in the workbook)\n");
        for s in &scripts {
            out.push_str(&format!(
                "\n{} ({}, {} lines)\n",
                s.name,
                s.language.label(),
                s.source.lines().count()
            ));
            let keyword = match s.language {
                ScriptLanguage::JavaScript => "function",
                ScriptLanguage::Python => "def",
            };
            for f in sheets_script::scripts::functions(s) {
                let mut head = format!(
                    "{keyword} {}{}",
                    f.name,
                    if f.params > 0 { "(…)" } else { "()" }
                );
                if f.is_macro() {
                    head.push_str("  [macro]");
                }
                out.push_str(&format!(
                    "  {head:<48} lines {}-{}\n",
                    f.line + 1,
                    f.end_line + 1
                ));
            }
        }
    }
    out.push_str("\nkantor sheets macros FILE --module NAME prints a module's code.\n");
    Ok(out)
}

/// The tool's arguments (`module`, `code`) as options.
pub fn opts_from(args: &Value) -> MacrosOpts {
    MacrosOpts {
        module: args
            .get("module")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned),
        code: args.get("code").and_then(Value::as_bool).unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheets_engine::vba::ModuleKind;
    use sheets_engine::vba::synth::Builder;
    use std::io::{Cursor, Read, Write};

    /// A workbook with `vba` as its project, saved as `.xlsm` and opened.
    pub(crate) fn book_with(vba: &[u8], dir: &std::path::Path) -> Workbook {
        let plain = dir.join("plain.xlsx");
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(0, 0, 0, "x").unwrap();
        wb.save_as(&plain).unwrap();
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&plain).unwrap()).unwrap();
        let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        for i in 0..zip.len() {
            let mut e = zip.by_index(i).unwrap();
            let name = e.name().unwrap().into_owned();
            let mut data = Vec::new();
            e.read_to_end(&mut data).unwrap();
            let data = match name.as_str() {
                "[Content_Types].xml" => String::from_utf8(data)
                    .unwrap()
                    .replace(
                        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
                        "application/vnd.ms-excel.sheet.macroEnabled.main+xml",
                    )
                    .replace(
                        "</Types>",
                        "<Default Extension=\"bin\" ContentType=\"application/vnd.ms-office.vbaProject\"/></Types>",
                    )
                    .into_bytes(),
                "xl/_rels/workbook.xml.rels" => String::from_utf8(data)
                    .unwrap()
                    .replace(
                        "</Relationships>",
                        "<Relationship Id=\"rId99\" Type=\"http://schemas.microsoft.com/office/2006/relationships/vbaProject\" Target=\"vbaProject.bin\"/></Relationships>",
                    )
                    .into_bytes(),
                _ => data,
            };
            out.start_file(name.as_str(), opts).unwrap();
            out.write_all(&data).unwrap();
        }
        out.start_file("xl/vbaProject.bin", opts).unwrap();
        out.write_all(vba).unwrap();
        let path = dir.join("book.xlsm");
        std::fs::write(&path, out.finish().unwrap().into_inner()).unwrap();
        Workbook::open(&path).unwrap()
    }

    #[std::prelude::v1::test]
    fn lists_modules_and_code() {
        let vba = Builder::new(1252)
            .module(
                "Module1",
                ModuleKind::Standard,
                "Sub Totals()\n    Range(\"B1\").Value = 3\nEnd Sub\n\nFunction Twice(x)\n    Twice = x * 2\nEnd Function\n",
            )
            .module("ThisWorkbook", ModuleKind::Document, "")
            .build()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let book = book_with(&vba, dir.path());

        let v = json(&book, &MacrosOpts::default()).unwrap();
        assert_eq!(v["has_macros"], true);
        assert_eq!(v["modules"].as_array().unwrap().len(), 2);
        let m = &v["modules"][0];
        assert_eq!(m["name"], "Module1");
        assert_eq!(m["kind"], "standard");
        assert!(m.get("code").is_none());
        assert_eq!(m["procedures"][0]["name"], "Totals");
        assert_eq!(m["procedures"][0]["macro"], true);
        assert_eq!(m["procedures"][0]["line"], 1);
        assert_eq!(m["procedures"][0]["end_line"], 3);
        assert_eq!(m["procedures"][1]["kind"], "Function");
        assert_eq!(m["procedures"][1]["arguments"], true);
        assert_eq!(v["references"][0]["name"], "stdole");

        let one = json(
            &book,
            &MacrosOpts {
                module: Some("module1".into()),
                code: false,
            },
        )
        .unwrap();
        assert_eq!(one["modules"].as_array().unwrap().len(), 1);
        assert!(
            one["modules"][0]["code"]
                .as_str()
                .unwrap()
                .contains("Range(\"B1\").Value = 3")
        );
        assert!(
            json(
                &book,
                &MacrosOpts {
                    module: Some("Nope".into()),
                    code: false
                }
            )
            .is_err()
        );

        let t = text(&book).unwrap();
        assert!(t.contains("Module1 (Module, 7 lines)"), "{t}");
        assert!(t.contains("Sub Totals()  [macro]"), "{t}");
        assert!(t.contains("ThisWorkbook (Document, no code)"), "{t}");

        let plain = Workbook::new_empty().unwrap();
        assert_eq!(
            json(&plain, &MacrosOpts::default()).unwrap()["has_macros"],
            false
        );
    }

    #[std::prelude::v1::test]
    fn lists_scripts() {
        let mut book = Workbook::new_empty().unwrap();
        book.set_scripts(vec![
            Script::new(
                "Report",
                ScriptLanguage::JavaScript,
                "function go() {\n  Range('A1').value = 1;\n}\nfunction twice(x) { return x * 2; }\n",
            ),
            Script::new("Tools", ScriptLanguage::Python, "def tidy():\n    pass\n"),
        ]);
        let v = json(&book, &MacrosOpts::default()).unwrap();
        assert_eq!(v["has_macros"], true);
        assert!(v.get("project").is_none());
        let m = &v["modules"][0];
        assert_eq!(m["name"], "Report");
        assert_eq!(m["kind"], "javascript");
        assert_eq!(m["procedures"][0]["name"], "go");
        assert_eq!(m["procedures"][0]["macro"], true);
        assert_eq!(m["procedures"][0]["end_line"], 3);
        assert_eq!(m["procedures"][1]["arguments"], true);
        assert_eq!(v["modules"][1]["kind"], "python");
        let one = json(
            &book,
            &MacrosOpts {
                module: Some("tools".into()),
                code: false,
            },
        )
        .unwrap();
        assert_eq!(one["modules"][0]["code"], "def tidy():\n    pass\n");
        let t = text(&book).unwrap();
        assert!(t.contains("Report (JavaScript, 4 lines)"), "{t}");
        assert!(t.contains("function go()  [macro]"), "{t}");
        assert!(t.contains("def tidy()  [macro]"), "{t}");
    }
}
