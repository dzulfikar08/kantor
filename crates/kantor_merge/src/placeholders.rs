//! `{{Field}}` templates: a presentation's text (certificates, name
//! cards) or a workbook's cells (invoices, payslips) filled in per record,
//! one file each.
//!
//! In a presentation every `{{Field}}` in a slide's text (shapes, tables,
//! groups) becomes the record's value, keeping the formatting of the run
//! where it starts, even when PowerPoint split the placeholder across
//! runs. In a workbook every text cell's placeholders are filled; a cell
//! that is just one placeholder of a number or date takes the number, so
//! formulas and number formats work on it.

use std::collections::HashSet;
use std::io::{Cursor, Read as _, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use docs_engine::{MergeData, MergeValue};

use crate::{Overwrite, Report, check_new, file_name, unique_path};

/// `text` with each `{{Field}}` replaced by `value(Field)` (`None`: kept
/// as is). Returns the text and the names that had no value.
pub fn fill(text: &str, value: &dyn Fn(&str) -> Option<String>) -> (String, Vec<String>) {
    let mut out = String::with_capacity(text.len());
    let mut missing = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        match after.find("}}") {
            Some(j) if !after[..j].contains("{{") => {
                let name = after[..j].trim();
                match value(name) {
                    Some(v) => out.push_str(&v),
                    None => {
                        missing.push(name.to_owned());
                        out.push_str(&rest[i..i + 2 + j + 2]);
                    }
                }
                rest = &after[j + 2..];
            }
            _ => {
                out.push_str("{{");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    (out, missing)
}

/// The `{{Field}}` names in `text`.
pub fn names_in(text: &str) -> Vec<String> {
    fill(text, &|_| None).1
}

/// A record's value for a placeholder name (`None`: no such column).
fn lookup(data: &MergeData, row: usize) -> impl Fn(&str) -> Option<String> + '_ {
    move |name: &str| data.value(row, name).map(|v| v.text)
}

// ── DrawingML text (presentations) ─────────────────────────────────────────

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        let Some(semi) = after.find(';') else {
            out.push_str(after);
            return out;
        };
        let ent = &after[1..semi];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e if e.starts_with("#x") => u32::from_str_radix(&e[2..], 16)
                .ok()
                .and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => out.push(c),
            None => out.push_str(&after[..=semi]),
        }
        rest = &after[semi + 1..];
    }
    out.push_str(rest);
    out
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A part's XML with the `{{Field}}`s of each `a:p` paragraph filled in.
/// A placeholder split across runs (`{{Na` `me}}`) is filled into the run
/// it starts in; the rest of it goes from the following runs.
pub fn fill_drawingml(xml: &str, value: &dyn Fn(&str) -> Option<String>) -> (String, Vec<String>) {
    let mut out = String::with_capacity(xml.len());
    let mut missing = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest
        .find("<a:p>")
        .into_iter()
        .chain(rest.find("<a:p "))
        .min()
    {
        let Some(end) = rest[i..].find("</a:p>").map(|e| i + e + 6) else {
            break;
        };
        out.push_str(&rest[..i]);
        let (p, m) = fill_paragraph(&rest[i..end], value);
        out.push_str(&p);
        missing.extend(m);
        rest = &rest[end..];
    }
    out.push_str(rest);
    (out, missing)
}

/// One `a:p`: its `a:t` texts joined, placeholders found and filled.
fn fill_paragraph(p: &str, value: &dyn Fn(&str) -> Option<String>) -> (String, Vec<String>) {
    // Text elements: (content start, content end, unescaped text).
    let mut texts: Vec<(usize, usize, String)> = Vec::new();
    let mut from = 0;
    while let Some(k) = p[from..].find("<a:t") {
        let k = from + k;
        let tag_end = match p[k..].find('>') {
            Some(e) => k + e,
            None => break,
        };
        let name_end = &p[k + 4..k + 5];
        if !(name_end == ">" || name_end == " " || name_end == "/") {
            from = k + 4;
            continue;
        }
        if p[..=tag_end].ends_with("/>") {
            from = tag_end + 1;
            continue;
        }
        let Some(close) = p[tag_end..].find("</a:t>").map(|c| tag_end + c) else {
            break;
        };
        texts.push((tag_end + 1, close, unescape(&p[tag_end + 1..close])));
        from = close + 6;
    }
    let joined: String = texts.iter().map(|t| t.2.as_str()).collect();
    if !joined.contains("{{") {
        return (p.to_owned(), Vec::new());
    }
    let (filled, missing) = fill(&joined, value);
    if filled == joined {
        return (p.to_owned(), missing);
    }
    // Map each placeholder (in joined) to its replacement, then rebuild
    // the texts: a character of `joined` stays in its element; a
    // placeholder's text goes into the element it starts in.
    let mut new_texts: Vec<String> = vec![String::new(); texts.len()];
    let owner: Vec<usize> = texts
        .iter()
        .enumerate()
        .flat_map(|(k, t)| std::iter::repeat_n(k, t.2.len()))
        .collect();
    let mut pos = 0;
    while pos < joined.len() {
        let rest = &joined[pos..];
        if rest.starts_with("{{")
            && let Some(j) = rest[2..].find("}}")
            && !rest[2..2 + j].contains("{{")
        {
            let whole = &rest[..2 + j + 2];
            let (v, _) = fill(whole, value);
            new_texts[owner[pos]].push_str(&v);
            pos += whole.len();
            continue;
        }
        let c = rest.chars().next().expect("not at the end");
        new_texts[owner[pos]].push(c);
        pos += c.len_utf8();
    }
    let mut out = String::with_capacity(p.len());
    let mut last = 0;
    for (k, (a, b, _)) in texts.iter().enumerate() {
        out.push_str(&p[last..*a]);
        out.push_str(&escape(&new_texts[k]));
        last = *b;
    }
    out.push_str(&p[last..]);
    // Text that now starts or ends with spaces keeps them.
    (out, missing)
}

/// The `{{Field}}` names on a presentation's slides.
pub fn slides_names(template: &Path) -> Result<Vec<String>> {
    let bytes = std::fs::read(template)?;
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes.as_slice()))?;
    let mut names: Vec<String> = Vec::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i)?;
        let name = f.name().unwrap_or_default().into_owned();
        if !slide_part(&name) {
            continue;
        }
        let mut xml = String::new();
        f.read_to_string(&mut xml)?;
        let (_, found) = fill_drawingml(&xml, &|_| None);
        for n in found {
            if !names.contains(&n) {
                names.push(n);
            }
        }
    }
    Ok(names)
}

fn slide_part(name: &str) -> bool {
    name.starts_with("ppt/slides/slide") && name.ends_with(".xml")
}

/// A presentation with the record's values in its slides.
pub fn fill_pptx(
    bytes: &[u8],
    value: &dyn Fn(&str) -> Option<String>,
) -> Result<(Vec<u8>, Vec<String>)> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let mut missing = Vec::new();
    for i in 0..zip.len() {
        let name = zip.by_index(i)?.name().unwrap_or_default().into_owned();
        if !slide_part(&name) {
            out.raw_copy_file(zip.by_index_raw(i)?)?;
            continue;
        }
        let mut xml = String::new();
        zip.by_index(i)?.read_to_string(&mut xml)?;
        let (filled, m) = fill_drawingml(&xml, value);
        for n in m {
            if !missing.contains(&n) {
                missing.push(n);
            }
        }
        out.start_file(
            name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )?;
        out.write_all(filled.as_bytes())?;
    }
    Ok((out.finish()?.into_inner(), missing))
}

/// What a placeholder merge writes: `.pptx`/`.xlsx` and/or `.pdf`, one per
/// record, in `dir`.
#[derive(Clone, Debug, Default)]
pub struct FilesOut {
    pub dir: PathBuf,
    /// The template's own format (`.pptx`, `.xlsx`).
    pub native: bool,
    pub pdf: bool,
    pub name: Option<String>,
    pub overwrite: Overwrite,
}

/// A record and its output files (path, and whether it is a PDF).
type Planned = (usize, Vec<(PathBuf, bool)>);

fn names_for(
    template: &Path,
    data: &MergeData,
    records: &[usize],
    out: &FilesOut,
    native: &str,
) -> Result<Vec<Planned>> {
    if !(out.native || out.pdf) {
        bail!("nothing to write: ask for .{native} or .pdf files");
    }
    if records.is_empty() {
        bail!("no records to merge (check the filter)");
    }
    if !out.dir.is_dir() {
        std::fs::create_dir_all(&out.dir)?;
    }
    let pattern = out
        .name
        .clone()
        .unwrap_or_else(|| crate::docs::default_pattern(Some(template)));
    let mut taken = HashSet::new();
    let mut plan = Vec::new();
    for (n, &row) in records.iter().enumerate() {
        let mut files = Vec::new();
        for (want, ext, pdf) in [(out.native, native, false), (out.pdf, "pdf", true)] {
            if want {
                let name = file_name(&pattern, data, row, n + 1, ext);
                let path = unique_path(&out.dir, &name, &mut taken);
                check_new(&path, out.overwrite)?;
                files.push((path, pdf));
            }
        }
        plan.push((row, files));
    }
    Ok(plan)
}

/// One presentation per record from a `{{Field}}` template.
pub fn merge_slides(
    template: &Path,
    data: &MergeData,
    records: &[usize],
    out: &FilesOut,
    engine: &mut Option<docs_pdf::Engine>,
) -> Result<Report> {
    let bytes =
        std::fs::read(template).with_context(|| format!("can't read {}", template.display()))?;
    // Fails early on files that aren't presentations.
    slides_engine::Presentation::open(template).context("can't open the presentation")?;
    let plan = names_for(template, data, records, out, "pptx")?;
    let mut report = Report::default();
    let scratch = tempfile::tempdir()?;
    for (row, files) in plan {
        let (filled, missing) = fill_pptx(&bytes, &lookup(data, row))?;
        for m in missing {
            let note = format!("no field {m:?} in the data source: {{{{{m}}}}} is left as it is");
            if !report.notes.contains(&note) {
                report.notes.push(note);
            }
        }
        let tmp = scratch.path().join("merged.pptx");
        std::fs::write(&tmp, &filled)?;
        for (path, pdf) in files {
            if pdf {
                let pres = slides_engine::Presentation::open(&tmp)?;
                let engine = engine.get_or_insert_with(docs_pdf::Engine::new);
                let (pdf, _) = slides_pdf::export(&pres, engine)?;
                docs_pdf::write_file(&path, &pdf)?;
            } else {
                std::fs::write(&path, &filled)?;
            }
            report.files.push(path);
        }
        report.records += 1;
    }
    Ok(report)
}

// ── workbooks ─────────────────────────────────────────────────────────────

/// The `{{Field}}` names in a workbook's cells.
pub fn sheets_names(template: &Path) -> Result<Vec<String>> {
    let book = sheets_engine::Workbook::open(template).context("can't open the workbook")?;
    let mut names: Vec<String> = Vec::new();
    for s in 0..book.sheet_names().len() as u32 {
        for (_, _, text) in book.text_cells(s) {
            for n in names_in(&text) {
                if !names.contains(&n) {
                    names.push(n);
                }
            }
        }
    }
    Ok(names)
}

/// What a cell gets: a lone placeholder of a number or date keeps the
/// number (typed as such), anything else is text (typed with a leading
/// apostrophe so it stays text).
fn cell_input(text: &str, data: &MergeData, row: usize) -> (String, Vec<String>) {
    let t = text.trim();
    if let Some(name) = t.strip_prefix("{{").and_then(|r| r.strip_suffix("}}"))
        && !name.contains("{{")
        && let Some(MergeValue {
            number: Some(n), ..
        }) = data.value(row, name.trim())
    {
        return (format!("{n}"), Vec::new());
    }
    let (filled, missing) = fill(text, &lookup(data, row));
    let input = if filled.is_empty() {
        filled
    } else {
        format!("'{filled}")
    };
    (input, missing)
}

/// One workbook per record from a `{{Field}}` template.
pub fn merge_sheets(
    template: &Path,
    data: &MergeData,
    records: &[usize],
    out: &FilesOut,
) -> Result<Report> {
    sheets_engine::Workbook::open(template).context("can't open the workbook")?;
    let plan = names_for(template, data, records, out, "xlsx")?;
    let mut report = Report::default();
    for (row, files) in plan {
        let mut book = sheets_engine::Workbook::open(template)?;
        for s in 0..book.sheet_names().len() as u32 {
            let cells: Vec<(u32, u32, String)> = book
                .text_cells(s)
                .into_iter()
                .filter(|(_, _, t)| t.contains("{{"))
                .collect();
            let mut inputs = Vec::new();
            for (r, c, text) in cells {
                let (input, missing) = cell_input(&text, data, row);
                for m in missing {
                    let note =
                        format!("no field {m:?} in the data source: {{{{{m}}}}} is left as it is");
                    if !report.notes.contains(&note) {
                        report.notes.push(note);
                    }
                }
                inputs.push((r, c, input));
            }
            if !inputs.is_empty() {
                book.set_inputs(s, &inputs)?;
            }
        }
        for (path, pdf) in files {
            if pdf {
                let print = sheets_pdf::Options::default().now();
                let (bytes, _) = sheets_pdf::export(&mut book, &print)?;
                docs_pdf::write_file(&path, &bytes)?;
            } else {
                book.save_as(&path)?;
            }
            report.files.push(path);
        }
        report.records += 1;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_fill_across_runs() {
        let v = |n: &str| match n {
            "Name" => Some("Zoë & 张".to_owned()),
            "Course" => Some("Rust".to_owned()),
            _ => None,
        };
        assert_eq!(fill("Dear {{ Name }}!", &v).0, "Dear Zoë & 张!");
        assert_eq!(
            fill("{{Nope}} {{", &v),
            ("{{Nope}} {{".to_owned(), vec!["Nope".to_owned()])
        );
        let xml = r#"<p:sp><a:p><a:r><a:rPr b="1"/><a:t>Awarded to {{Na</a:t></a:r><a:r><a:t>me}} for </a:t></a:r><a:r><a:t xml:space="preserve">{{Course}}</a:t></a:r></a:p><a:p><a:r><a:t>&lt;{{Name}}&gt;</a:t></a:r></a:p></p:sp>"#;
        let (out, missing) = fill_drawingml(xml, &v);
        assert!(missing.is_empty());
        assert_eq!(
            out,
            r#"<p:sp><a:p><a:r><a:rPr b="1"/><a:t>Awarded to Zoë &amp; 张</a:t></a:r><a:r><a:t> for </a:t></a:r><a:r><a:t xml:space="preserve">Rust</a:t></a:r></a:p><a:p><a:r><a:t>&lt;Zoë &amp; 张&gt;</a:t></a:r></a:p></p:sp>"#
        );
    }

    #[test]
    fn lone_number_placeholders_stay_numbers() {
        let data = MergeData {
            fields: vec!["Total".into(), "Phone".into()],
            rows: vec![vec![
                MergeValue {
                    text: "1,250".into(),
                    number: Some(1250.0),
                    date: None,
                },
                MergeValue::text("0812"),
            ]],
        };
        assert_eq!(cell_input("{{Total}}", &data, 0).0, "1250");
        assert_eq!(cell_input("{{Phone}}", &data, 0).0, "'0812");
        assert_eq!(cell_input("Total: {{Total}}", &data, 0).0, "'Total: 1,250");
    }
}
