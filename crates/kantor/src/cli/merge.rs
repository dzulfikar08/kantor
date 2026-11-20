//! `kantor docs merge`, `kantor slides merge`, `kantor sheets merge`: mail
//! merge without a window (see `kantor_merge`).
//!
//! ```text
//! kantor docs merge offer.docx staff.xlsx -o out/ --pdf --name "{Name} - Offer"
//! kantor docs merge offer.docx -o out/        (its own data source, as Word saved it)
//! kantor docs merge labels.docx staff.csv --single labels-merged.docx
//! kantor slides merge certificate.pptx people.xlsx -o certs/ --pdf
//! kantor sheets merge invoice.xlsx orders.xlsx --sheet Orders -o invoices/
//! ```

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use docs_engine::{Document, Query};
use kantor_merge::docs::DocsOut;
use kantor_merge::placeholders::FilesOut;
use kantor_merge::{Overwrite, Recipients, Report};

/// Which kind of template.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum App {
    Docs,
    Slides,
    Sheets,
}

impl App {
    fn word(self) -> &'static str {
        match self {
            Self::Docs => "docs",
            Self::Slides => "slides",
            Self::Sheets => "sheets",
        }
    }

    /// The template's own format.
    fn native(self) -> &'static str {
        match self {
            Self::Docs => "docx",
            Self::Slides => "pptx",
            Self::Sheets => "xlsx",
        }
    }
}

pub fn usage(app: App) -> String {
    let (template, fields, extra) = match app {
        App::Docs => (
            "TEMPLATE.docx",
            "Word mail merge fields (MERGEFIELD, with IF, NEXT, NEXTIF, SKIPIF,\n\
             MERGEREC, MERGESEQ and the \\* \\# \\@ \\b \\f switches), as Word's\n\
             Mailings tab writes them. Without DATA the template's own data\n\
             source is used (with its filter and sort), as Word saved it.",
            "\x20     --single FILE      every record in one .docx (a section each), or .pdf\n",
        ),
        App::Slides => (
            "TEMPLATE.pptx",
            "{{Field}} placeholders in the slides' text (shapes, tables, groups).",
            "",
        ),
        App::Sheets => (
            "TEMPLATE.xlsx",
            "{{Field}} placeholders in the cells (a cell holding only a number\n\
             or date field gets the number).",
            "",
        ),
    };
    let native = app.native();
    let data = if app == App::Docs { "[DATA]" } else { "DATA" };
    format!(
        "usage: kantor {} merge {template} {data} -o DIR [options]\n\
         \n\
         Fill a template in once per record of DATA (.xlsx .xlsm .xls .xlsb\n\
         .ods .csv .tsv; the first row names the fields) and write one file\n\
         per record. The template holds {fields}\n\
         \n\
         Options:\n\
         \x20 -o, --output-dir DIR   folder for the files (made when missing)\n\
         \x20     --{native}             write .{native} files (the default)\n\
         \x20     --pdf              write .pdf files (with --{native}: both)\n\
         \x20     --name PATTERN     file names: {{Field}} is the record's value, {{#}}\n\
         \x20                        its number (default: \"TEMPLATE {{#}}\"); the\n\
         \x20                        extension is added\n\
         {extra}\
         \x20     --sheet N|NAME     the data's sheet (1-based number or name;\n\
         \x20                        default: the first)\n\
         \x20     --header-row N     the row naming the fields (default 1)\n\
         \x20     --where COND       only records where COND holds: Field=Value,\n\
         \x20                        Field!=Value, Field<Value, >, <=, >=, Field~Text\n\
         \x20                        (contains), Field= (blank), Field!= (not blank);\n\
         \x20                        repeat for more (all must hold)\n\
         \x20     --sort FIELD[:desc] order of the records (repeat for more keys)\n\
         \x20 -f, --force            overwrite existing files\n\
         \x20 -h, --help             show this help\n\
         \n\
         Exit status: 0 on success, 1 when the merge failed, 2 for usage errors.",
        app.word()
    )
}

/// A merge to run (the command line's or the MCP tool's).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    pub template: PathBuf,
    pub data: Option<PathBuf>,
    pub sheet: Option<String>,
    pub header_row: Option<u32>,
    pub filters: Vec<String>,
    pub sort: Vec<String>,
    pub dir: Option<PathBuf>,
    pub native: bool,
    pub pdf: bool,
    pub name: Option<String>,
    pub single: Option<PathBuf>,
    pub force: bool,
}

struct Usage(String);

fn parse(app: App, args: &[OsString]) -> Result<Option<Request>, Usage> {
    let mut r = Request::default();
    let mut positional = Vec::new();
    let mut it = args.iter();
    let native_flag = format!("--{}", app.native());
    while let Some(arg) = it.next() {
        let s = arg.to_str().unwrap_or_default();
        if !s.starts_with('-') || s == "-" {
            positional.push(PathBuf::from(arg));
            continue;
        }
        let (flag, inline) = match s.split_once('=') {
            Some((f, v)) if f.starts_with("--") && f != "--where" => (f, Some(v.to_owned())),
            _ => (s, None),
        };
        let (flag, inline) = if let Some(v) = s.strip_prefix("--where=") {
            ("--where", Some(v.to_owned()))
        } else {
            (flag, inline)
        };
        let mut value = |name: &str| -> Result<String, Usage> {
            match inline.clone() {
                Some(v) => Ok(v),
                None => match it.next() {
                    Some(v) => Ok(v.to_string_lossy().into_owned()),
                    None => Err(Usage(format!("{name} needs a value"))),
                },
            }
        };
        match flag {
            "-h" | "--help" => return Ok(None),
            "-f" | "--force" => r.force = true,
            "-o" | "--output-dir" => r.dir = Some(PathBuf::from(value(flag)?)),
            "--pdf" => r.pdf = true,
            f if f == native_flag => r.native = true,
            "--name" => r.name = Some(value(flag)?),
            "--single" if app == App::Docs => r.single = Some(PathBuf::from(value(flag)?)),
            "--sheet" => r.sheet = Some(value(flag)?),
            "--header-row" => {
                let v = value(flag)?;
                r.header_row =
                    Some(v.parse().ok().filter(|n| *n >= 1).ok_or_else(|| {
                        Usage(format!("--header-row takes a row number, not {v:?}"))
                    })?);
            }
            "--where" => r.filters.push(value(flag)?),
            "--sort" => r.sort.push(value(flag)?),
            _ => return Err(Usage(format!("unknown option {s}"))),
        }
    }
    let mut pos = positional.into_iter();
    r.template = pos
        .next()
        .ok_or_else(|| Usage(format!("give the template ({})", app.native())))?;
    r.data = pos.next();
    if let Some(extra) = pos.next() {
        return Err(Usage(format!("unexpected argument {}", extra.display())));
    }
    if r.dir.is_none() && r.single.is_none() {
        return Err(Usage("give an output folder with -o".into()));
    }
    if r.dir.is_none() && (r.pdf || r.native || r.name.is_some()) {
        return Err(Usage(format!(
            "--pdf, --{} and --name are for the files in -o DIR",
            app.native()
        )));
    }
    if r.data.is_none() && app != App::Docs {
        return Err(Usage("give the data file (.xlsx, .csv…)".into()));
    }
    Ok(Some(r))
}

/// Run `kantor APP merge ARGS…`; returns the exit status.
pub fn main(app: App, args: &[OsString]) -> i32 {
    let req = match parse(app, args) {
        Ok(Some(r)) => r,
        Ok(None) => {
            println!("{}", usage(app));
            return 0;
        }
        Err(Usage(msg)) => {
            eprintln!("kantor {} merge: {msg}\n\n{}", app.word(), usage(app));
            return 2;
        }
    };
    let mut engine = None;
    match run(app, &req, &mut engine) {
        Ok(report) => {
            for note in &report.notes {
                eprintln!("kantor {} merge: note: {note}", app.word());
            }
            for f in &report.files {
                println!("{}", f.display());
            }
            println!(
                "{} record{} merged, {} file{} written",
                report.records,
                if report.records == 1 { "" } else { "s" },
                report.files.len(),
                if report.files.len() == 1 { "" } else { "s" }
            );
            0
        }
        Err(e) => {
            eprintln!("kantor {} merge: {e:#}", app.word());
            1
        }
    }
}

/// The records and their data for a request: the data file's (or the
/// Docs template's own linked data source), filtered and sorted.
fn recipients(
    req: &Request,
    link: Option<&docs_engine::MailMergeLink>,
    doc_dir: Option<&Path>,
) -> Result<(Recipients, Vec<usize>)> {
    let mut query = Query::default();
    let (path, sheet) = match (&req.data, link) {
        (Some(p), _) => (p.clone(), req.sheet.clone()),
        (None, Some(l)) => {
            let path = l
                .source_path(doc_dir)
                .context("the template's data source has no file")?;
            query = l.parsed_query();
            let sheet = req.sheet.clone().or_else(|| {
                (!l.is_text_file() && !query.table.is_empty()).then(|| query.table.clone())
            });
            (path, sheet)
        }
        (None, None) => bail!("the template has no data source: give the data file"),
    };
    let rec = Recipients::open(&path, sheet.as_deref(), req.header_row)?;
    for w in &req.filters {
        query.filters.push(kantor_merge::parse_where(w)?);
    }
    if !req.sort.is_empty() {
        query.sort.clear();
        for s in &req.sort {
            let (f, asc) = match s.rsplit_once(':') {
                Some((f, d)) if d.eq_ignore_ascii_case("desc") => (f, false),
                Some((f, d)) if d.eq_ignore_ascii_case("asc") => (f, true),
                _ => (s.as_str(), true),
            };
            query.sort.push((f.trim().to_owned(), asc));
        }
    }
    let rows = kantor_merge::select(&rec.data, &query)?;
    Ok((rec, rows))
}

/// Run a merge request.
pub fn run(app: App, req: &Request, engine: &mut Option<docs_pdf::Engine>) -> Result<Report> {
    if !req.template.is_file() {
        bail!("no such file: {}", req.template.display());
    }
    let ext = req
        .template
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let overwrite = if req.force {
        Overwrite::Allow
    } else {
        Overwrite::Refuse
    };
    let native = req.native || !req.pdf;
    match app {
        App::Docs => {
            if !matches!(
                ext.as_str(),
                "docx" | "docm" | "dotx" | "doc" | "dot" | "rtf"
            ) {
                bail!("{} is not a Word document", req.template.display());
            }
            let doc = Document::open(&req.template).context("can't open the template")?;
            let dir = req
                .template
                .canonicalize()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf));
            let (rec, rows) = recipients(req, doc.mail_merge(), dir.as_deref())?;
            let out = DocsOut {
                dir: req.dir.clone(),
                docx: req.dir.is_some() && native,
                pdf: req.dir.is_some() && req.pdf,
                name: req.name.clone(),
                single: req.single.clone(),
                overwrite,
            };
            kantor_merge::docs::merge(
                &doc,
                &rec.data,
                &rows,
                &out,
                &mut kantor_merge::docs::pdf_writer(engine),
            )
        }
        App::Slides | App::Sheets => {
            let want = app.native();
            if ext != want {
                bail!("the template must be a .{want} file");
            }
            let (rec, rows) = recipients(req, None, None)?;
            let out = FilesOut {
                dir: req.dir.clone().context("give an output folder")?,
                native,
                pdf: req.pdf,
                name: req.name.clone(),
                overwrite,
            };
            if app == App::Slides {
                kantor_merge::placeholders::merge_slides(
                    &req.template,
                    &rec.data,
                    &rows,
                    &out,
                    engine,
                )
            } else {
                kantor_merge::placeholders::merge_sheets(&req.template, &rec.data, &rows, &out)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<OsString> {
        s.iter().map(OsString::from).collect()
    }

    fn parsed(app: App, s: &[&str]) -> Request {
        match parse(app, &args(s)) {
            Ok(Some(r)) => r,
            Ok(None) => panic!("help"),
            Err(Usage(m)) => panic!("{m}"),
        }
    }

    fn err(app: App, s: &[&str]) -> String {
        match parse(app, &args(s)) {
            Err(Usage(m)) => m,
            _ => panic!("no error for {s:?}"),
        }
    }

    #[std::prelude::v1::test]
    fn command_lines() {
        let r = parsed(
            App::Docs,
            &[
                "offer.docx",
                "staff.xlsx",
                "-o",
                "out",
                "--pdf",
                "--name",
                "{Name} - Offer",
                "--where",
                "Dept=R&D",
                "--where=City=Jakarta",
                "--sort",
                "Name:desc",
            ],
        );
        assert_eq!(r.template, PathBuf::from("offer.docx"));
        assert_eq!(r.data, Some(PathBuf::from("staff.xlsx")));
        assert!(r.pdf && !r.native);
        assert_eq!(r.filters, vec!["Dept=R&D", "City=Jakarta"]);
        assert_eq!(r.sort, vec!["Name:desc"]);
        let r = parsed(App::Docs, &["t.docx", "--single", "all.docx"]);
        assert_eq!(r.single, Some(PathBuf::from("all.docx")));
        assert!(r.data.is_none());
        let r = parsed(
            App::Slides,
            &["c.pptx", "d.csv", "-o", "x", "--pptx", "--pdf"],
        );
        assert!(r.native && r.pdf);
        assert!(err(App::Slides, &["c.pptx", "d.csv", "--single", "a"]).contains("unknown option"));
        assert!(err(App::Docs, &["t.docx", "d.csv"]).contains("-o"));
        assert!(err(App::Sheets, &["t.xlsx", "-o", "x"]).contains("data file"));
        assert!(err(App::Docs, &["t.docx", "--single", "a.docx", "--pdf"]).contains("-o DIR"));
    }
}
