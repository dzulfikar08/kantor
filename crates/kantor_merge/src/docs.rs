//! Word mail merge main documents: one `.docx` and/or `.pdf` per record,
//! or every record in one `.docx` (a section each), as Word's Finish &
//! Merge makes them.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use docs_engine::{Document, MergeData};

use crate::{Overwrite, Report, check_new, file_name, unique_path};

/// What to write.
#[derive(Clone, Debug, Default)]
pub struct DocsOut {
    /// The folder for one file per record (`None`: no such files).
    pub dir: Option<PathBuf>,
    /// One `.docx` per record.
    pub docx: bool,
    /// One `.pdf` per record.
    pub pdf: bool,
    /// File name pattern ([`file_name`]); default `TEMPLATE N`.
    pub name: Option<String>,
    /// Every record in this one `.docx` (or `.pdf`).
    pub single: Option<PathBuf>,
    pub overwrite: Overwrite,
}

/// Fields the document uses that `data` lacks (Word's "MergeField was
/// not found" errors).
pub fn missing_fields(doc: &Document, data: &MergeData) -> Vec<String> {
    doc.merge_field_names()
        .into_iter()
        .filter(|n| data.column(n).is_none())
        .collect()
}

/// Mail merge fields of Word's that Kantor doesn't fill in yet, found in
/// the body: (field type, Word's name for it).
pub fn unfilled_fields(doc: &Document) -> Vec<(&'static str, &'static str)> {
    let kinds: HashSet<String> = doc.fields().iter().map(|f| f.kind()).collect();
    [
        ("ADDRESSBLOCK", "Address Block"),
        ("GREETINGLINE", "Greeting Line"),
    ]
    .into_iter()
    .filter(|(k, _)| kinds.contains(*k))
    .collect()
}

/// The default file name pattern: the template's name and the record's
/// number in the merge.
pub fn default_pattern(template: Option<&Path>) -> String {
    let stem = template
        .and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Merged".into());
    format!("{stem} {{#}}")
}

/// Merge `doc` with `records` (indices into `data.rows`, in order) and
/// write what `out` asks for. `pdf` writes a document as PDF (see
/// [`pdf_writer`]).
pub fn merge(
    doc: &Document,
    data: &MergeData,
    records: &[usize],
    out: &DocsOut,
    pdf: &mut dyn FnMut(&Document, &Path) -> Result<()>,
) -> Result<Report> {
    if out.single.is_none() && (out.dir.is_none() || !(out.docx || out.pdf)) {
        bail!("nothing to write: give a folder with .docx or .pdf files, or one merged file");
    }
    let mut report = Report::default();
    for f in missing_fields(doc, data) {
        report.notes.push(format!(
            "the document's field {f:?} is not in the data source (it shows Word's error text)"
        ));
    }
    for (kind, name) in unfilled_fields(doc) {
        report.notes.push(format!(
            "{name} ({kind}) fields aren't filled in yet: they keep the template's text"
        ));
    }
    if records.is_empty() {
        bail!("no records to merge (check the filter)");
    }
    if let Some(dir) = &out.dir
        && !dir.is_dir()
    {
        std::fs::create_dir_all(dir)?;
    }
    if let Some(single) = &out.single {
        check_new(single, out.overwrite)?;
        if let Some(parent) = single.parent().filter(|p| !p.as_os_str().is_empty())
            && !parent.is_dir()
        {
            bail!("folder {} doesn't exist", parent.display());
        }
        let Some(mut one) = doc.merge_into_one(data, records) else {
            bail!("every record was skipped (SKIPIF)");
        };
        report.records = one.sections().len();
        write(&mut one, single, pdf)?;
        report.files.push(single.clone());
    }
    if let Some(dir) = &out.dir
        && (out.docx || out.pdf)
    {
        if !dir.is_dir() {
            std::fs::create_dir_all(dir)?;
        }
        let pattern = out
            .name
            .clone()
            .unwrap_or_else(|| default_pattern(doc.path()));
        let copies = doc.merge_all(data, records);
        if copies.len() < records.len() {
            report.notes.push(format!(
                "{} record(s) skipped or taken by NEXT fields",
                records.len() - copies.len()
            ));
        }
        let mut taken = HashSet::new();
        // Every name first: nothing is written when one exists.
        let mut plan: Vec<(PathBuf, usize)> = Vec::new();
        for (n, (row, _)) in copies.iter().enumerate() {
            for (want, ext) in [(out.docx, "docx"), (out.pdf, "pdf")] {
                if want {
                    let name = file_name(&pattern, data, *row, n + 1, ext);
                    let path = unique_path(dir, &name, &mut taken);
                    check_new(&path, out.overwrite)?;
                    plan.push((path, n));
                }
            }
        }
        report.records = copies.len();
        let mut docs: Vec<Document> = copies.into_iter().map(|(_, d)| d).collect();
        for (path, n) in plan {
            write(&mut docs[n], &path, pdf)?;
            report.files.push(path);
        }
    }
    Ok(report)
}

/// Write a document as `.docx` or (by extension) `.pdf`.
pub fn write(
    doc: &mut Document,
    path: &Path,
    pdf: &mut dyn FnMut(&Document, &Path) -> Result<()>,
) -> Result<()> {
    let is_pdf = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    if is_pdf {
        pdf(doc, path)
    } else {
        doc.save_as(path)
    }
}

/// A PDF writer laying documents out with `engine` (made on first use:
/// scanning the fonts takes a moment).
pub fn pdf_writer(
    engine: &mut Option<docs_pdf::Engine>,
) -> impl FnMut(&Document, &Path) -> Result<()> + '_ {
    move |doc, path| {
        let engine = engine.get_or_insert_with(docs_pdf::Engine::new);
        write_pdf(doc, path, engine)
    }
}

/// Lay `doc` out with `engine` and write it to `path` as PDF.
pub fn write_pdf(doc: &Document, path: &Path, engine: &mut docs_pdf::Engine) -> Result<()> {
    let laid = docs_pdf::lay_out(doc, engine);
    let bytes = docs_pdf::write(&docs_pdf::scene(doc, &laid))?;
    docs_pdf::write_file(path, &bytes)
}
