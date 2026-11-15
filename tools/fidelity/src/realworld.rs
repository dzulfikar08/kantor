//! `realworld_probe IN OUTDIR`: one real-world file through Kantor's engines,
//! as `tools/realworld/sweep.py` needs it. Opens `IN`, saves it to
//! `OUTDIR/saved.{xlsx,docx,pptx}`, opens the saved file again, exports
//! `OUTDIR/out.pdf`, and prints one JSON object on stdout with each stage's
//! time and error (a panic is caught and reported as `panic: …`) and what the
//! model held before and after the round trip (cells, formulas, inputs that
//! changed; paragraphs and text; slides, shapes and text). For Docs and
//! Slides it also lays out the reopened copy and reports its page count as
//! `pages_after`, so the sweep can flag a save that changes pagination.
//!
//! The sweep runs it with a timeout and a memory cap, so a hang or an
//! out-of-memory kill shows up there, not here.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Result, bail};
use serde_json::{Value, json};

#[derive(Clone, Copy, PartialEq)]
enum App {
    Sheets,
    Docs,
    Slides,
}

fn app(path: &Path) -> Option<App> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    Some(match ext.as_str() {
        "xlsx" | "xlsm" | "xltx" | "xltm" | "xls" | "xlt" | "xlsb" | "ods" | "csv" => App::Sheets,
        "docx" | "docm" | "dotx" | "dotm" | "doc" | "dot" | "rtf" => App::Docs,
        "pptx" | "pptm" | "potx" | "ppsx" | "ppt" | "pps" | "pot" => App::Slides,
        _ => return None,
    })
}

/// Run `f`, timing it; an error or a panic becomes the stage's `error`.
fn stage<T>(f: impl FnOnce() -> Result<T>) -> (Option<T>, Value) {
    let start = Instant::now();
    let r = catch_unwind(AssertUnwindSafe(f));
    let ms = start.elapsed().as_millis() as u64;
    match r {
        Ok(Ok(v)) => (Some(v), json!({ "ms": ms, "error": null })),
        Ok(Err(e)) => (None, json!({ "ms": ms, "error": format!("{e:#}") })),
        Err(p) => {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "panic".into());
            (None, json!({ "ms": ms, "error": format!("panic: {msg}") }))
        }
    }
}

/// Every cell's input by `(sheet, row, col)`.
type Inputs = HashMap<(u32, u32, u32), String>;

fn sheet_inputs(wb: &sheets_engine::Workbook) -> Inputs {
    let mut out = HashMap::new();
    for s in 0..wb.sheet_names().len() as u32 {
        for (r, c) in wb.filled_cells(s) {
            out.insert((s, r, c), wb.input(s, r, c));
        }
    }
    out
}

fn sheet_counts(wb: &sheets_engine::Workbook, inputs: &Inputs) -> Value {
    json!({
        "sheets": wb.sheet_names().len(),
        "cells": inputs.len(),
        "formulas": inputs.values().filter(|v| v.starts_with('=')).count(),
    })
}

/// Inputs that differ between the two opens, with a few examples.
fn sheet_diff(before: &Inputs, after: &Inputs, names: &[String]) -> Value {
    let mut changed = 0usize;
    let mut examples = Vec::new();
    let mut keys: Vec<_> = before.keys().chain(after.keys()).collect();
    keys.sort_unstable();
    keys.dedup();
    for k in keys {
        let (a, b) = (before.get(k), after.get(k));
        if a != b {
            changed += 1;
            if examples.len() < 5 {
                let sheet = names.get(k.0 as usize).cloned().unwrap_or_default();
                examples.push(json!({
                    "cell": format!("{sheet}!R{}C{}", k.1 + 1, k.2 + 1),
                    "before": a.map(|s| truncate(s)),
                    "after": b.map(|s| truncate(s)),
                }));
            }
        }
    }
    json!({ "changed": changed, "examples": examples })
}

fn truncate(s: &str) -> String {
    s.chars().take(80).collect()
}

fn doc_text(doc: &docs_engine::Document) -> Vec<String> {
    doc.paragraphs().iter().map(|p| p.text.clone()).collect()
}

fn text_diff(before: &[String], after: &[String]) -> Value {
    let changed = before.iter().zip(after).filter(|(a, b)| a != b).count()
        + before.len().abs_diff(after.len());
    let first = before
        .iter()
        .zip(after)
        .find(|(a, b)| a != b)
        .map(|(a, b)| json!({ "before": truncate(a), "after": truncate(b) }));
    json!({ "changed": changed, "example": first })
}

fn slide_text(pres: &slides_engine::Presentation) -> Vec<String> {
    let mut out = Vec::new();
    for i in 0..pres.len() {
        for shape in &pres.slide(i).shapes {
            if let Some(t) = &shape.text {
                out.push(t.plain_text());
            }
        }
    }
    out
}

fn slide_counts(pres: &slides_engine::Presentation, text: &[String]) -> Value {
    json!({
        "slides": pres.len(),
        "shapes": (0..pres.len()).map(|i| pres.slide(i).shapes.len()).sum::<usize>(),
        "text_chars": text.iter().map(|t| t.chars().count()).sum::<usize>(),
    })
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, out] = args.as_slice() else {
        bail!("usage: realworld_probe <file> <out dir>");
    };
    let input = PathBuf::from(input);
    let out = PathBuf::from(out);
    std::fs::create_dir_all(&out)?;
    let Some(app) = app(&input) else {
        bail!("{}: not a format Kantor opens", input.display());
    };
    // Panics are reported in the JSON; keep their message on stderr short.
    std::panic::set_hook(Box::new(|info| {
        eprintln!("panic: {info}");
    }));
    let saved_ext = match app {
        App::Sheets => "xlsx",
        App::Docs => "docx",
        App::Slides => "pptx",
    };
    let saved = out.join(format!("saved.{saved_ext}"));
    let pdf = out.join("out.pdf");
    let _ = std::fs::remove_file(&saved);
    let _ = std::fs::remove_file(&pdf);
    let mut report = json!({ "file": input.display().to_string(), "app": saved_ext });
    let r = report.as_object_mut().expect("object");
    let mut engine: Option<docs_pdf::Engine> = None;

    match app {
        App::Sheets => {
            let (wb, s) = stage(|| sheets_engine::Workbook::open(&input));
            r.insert("open".into(), s);
            let Some(mut wb) = wb else {
                return print(report);
            };
            r.insert("import_notes".into(), json!(wb.import_notes()));
            let names = wb.sheet_names();
            let before = sheet_inputs(&wb);
            r.insert("before".into(), sheet_counts(&wb, &before));
            let (_, s) = stage(|| wb.save_as(&saved));
            r.insert("save".into(), s);
            r.insert("save_notes".into(), json!(wb.save_notes()));
            if saved.exists() {
                let (wb2, s) = stage(|| sheets_engine::Workbook::open(&saved));
                r.insert("reopen".into(), s);
                if let Some(wb2) = wb2 {
                    let after = sheet_inputs(&wb2);
                    r.insert("after".into(), sheet_counts(&wb2, &after));
                    let mut names2 = wb2.sheet_names();
                    if names2 != names {
                        r.insert("sheet_names_changed".into(), json!([names, names2]));
                        names2 = names.clone();
                    }
                    r.insert("diff".into(), sheet_diff(&before, &after, &names2));
                }
            }
            let (pages, s) = stage(|| {
                let print = sheets_pdf::Options::default()
                    .now()
                    .with_path(input.canonicalize().ok().as_deref());
                let (bytes, pages) = sheets_pdf::export(&mut wb, &print)?;
                docs_pdf::write_file(&pdf, &bytes)?;
                Ok(pages)
            });
            r.insert("pdf".into(), s);
            r.insert("pages".into(), json!(pages));
        }
        App::Docs => {
            let (doc, s) = stage(|| docs_engine::Document::open(&input));
            r.insert("open".into(), s);
            let Some(mut doc) = doc else {
                return print(report);
            };
            r.insert("import_notes".into(), json!(doc.import_notes()));
            let before = doc_text(&doc);
            r.insert(
                "before".into(),
                json!({ "paragraphs": before.len(),
                        "text_chars": before.iter().map(|t| t.chars().count()).sum::<usize>() }),
            );
            let (_, s) = stage(|| doc.save_as(&saved));
            r.insert("save".into(), s);
            if saved.exists() {
                let (doc2, s) = stage(|| docs_engine::Document::open(&saved));
                r.insert("reopen".into(), s);
                if let Some(doc2) = doc2 {
                    let after = doc_text(&doc2);
                    r.insert(
                        "after".into(),
                        json!({ "paragraphs": after.len(),
                                "text_chars": after.iter().map(|t| t.chars().count()).sum::<usize>() }),
                    );
                    r.insert("diff".into(), text_diff(&before, &after));
                    let (pages, s) = stage(|| {
                        let engine = engine.get_or_insert_with(docs_pdf::Engine::new);
                        Ok(docs_pdf::lay_out(&doc2, engine).paged.pages.len())
                    });
                    r.insert("pdf_after".into(), s);
                    r.insert("pages_after".into(), json!(pages));
                }
            }
            let (pages, s) = stage(|| {
                let engine = engine.get_or_insert_with(docs_pdf::Engine::new);
                let laid = docs_pdf::lay_out(&doc, engine);
                let bytes = docs_pdf::write(&docs_pdf::scene(&doc, &laid))?;
                docs_pdf::write_file(&pdf, &bytes)?;
                Ok(laid.paged.pages.len())
            });
            r.insert("pdf".into(), s);
            r.insert("pages".into(), json!(pages));
        }
        App::Slides => {
            let (pres, s) = stage(|| slides_engine::Presentation::open(&input));
            r.insert("open".into(), s);
            let Some(mut pres) = pres else {
                return print(report);
            };
            r.insert("import_notes".into(), json!(pres.import_notes()));
            let before = slide_text(&pres);
            r.insert("before".into(), slide_counts(&pres, &before));
            let (_, s) = stage(|| pres.save_as(&saved));
            r.insert("save".into(), s);
            if saved.exists() {
                let (pres2, s) = stage(|| slides_engine::Presentation::open(&saved));
                r.insert("reopen".into(), s);
                if let Some(pres2) = pres2 {
                    let after = slide_text(&pres2);
                    r.insert("after".into(), slide_counts(&pres2, &after));
                    r.insert("diff".into(), text_diff(&before, &after));
                    let (pages, s) = stage(|| {
                        let engine = engine.get_or_insert_with(docs_pdf::Engine::new);
                        Ok(slides_pdf::export(&pres2, engine)?.1)
                    });
                    r.insert("pdf_after".into(), s);
                    r.insert("pages_after".into(), json!(pages));
                }
            }
            let (pages, s) = stage(|| {
                let engine = engine.get_or_insert_with(docs_pdf::Engine::new);
                let (bytes, pages) = slides_pdf::export(&pres, engine)?;
                docs_pdf::write_file(&pdf, &bytes)?;
                Ok(pages)
            });
            r.insert("pdf".into(), s);
            r.insert("pages".into(), json!(pages));
        }
    }
    print(report)
}

fn print(report: Value) -> Result<()> {
    println!("{report}");
    Ok(())
}
