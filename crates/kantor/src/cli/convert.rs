//! `kantor convert`: convert documents without opening a window (no GPUI).
//!
//! ```text
//! kantor convert report.docx report.pdf
//! kantor convert deck.pptx deck.pdf
//! kantor convert budget.ods budget.xlsx
//! kantor convert --sheet Summary book.xlsx summary.csv
//! kantor convert -o out/ --to pdf a.docx b.docx
//! ```

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};

pub const USAGE: &str = "usage: kantor convert [options] INPUT OUTPUT\n\
    \x20      kantor convert [options] -o DIR --to FORMAT INPUT...\n\
    \n\
    Convert documents without opening a window. Formats follow the file\n\
    extensions.\n\
    \n\
    Supported conversions:\n\
    \x20 .docx                                      -> .pdf\n\
    \x20 .pptx                                      -> .pdf (one page per slide,\n\
    \x20                                               hidden slides left out)\n\
    \x20 .xlsx .xlsm .xls .xlsb .ods .csv .tsv      -> .xlsx .csv .tsv\n\
    \n\
    Options:\n\
    \x20 -o, --output-dir DIR   write each INPUT to DIR/NAME.FORMAT\n\
    \x20 -t, --to FORMAT        output format with -o: pdf, xlsx, csv or tsv\n\
    \x20     --sheet N|NAME     sheet to write to .csv/.tsv (1-based number or\n\
    \x20                        name; default: the workbook's active sheet)\n\
    \x20 -f, --force            overwrite existing output files\n\
    \x20 -h, --help             show this help\n\
    \n\
    Exit status: 0 when every file converted, 1 when one failed, 2 for\n\
    usage errors (including unsupported conversions).";

/// What the command line asked for.
#[derive(Debug, Default, PartialEq)]
struct Options {
    jobs: Vec<(PathBuf, PathBuf)>,
    sheet: Option<String>,
    force: bool,
}

/// A usage error (exit status 2).
#[derive(Debug)]
struct Usage(String);

fn usage<T>(msg: impl Into<String>) -> Result<T, Usage> {
    Err(Usage(msg.into()))
}

/// Lower-case extension without the dot.
fn ext(p: &Path) -> String {
    p.extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

const SHEET_INPUTS: &[&str] = &[
    "xlsx", "xlsm", "xls", "xla", "xlsb", "ods", "csv", "tsv", "tab",
];
const SHEET_OUTPUTS: &[&str] = &["xlsx", "csv", "tsv"];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    DocxToPdf,
    PptxToPdf,
    Sheets,
}

/// Which converter handles `input` → `output`, or why none does.
fn kind(input: &Path, output: &Path) -> Result<Kind, Usage> {
    let (i, o) = (ext(input), ext(output));
    let show = |e: &str| {
        if e.is_empty() {
            "(no extension)".to_owned()
        } else {
            format!(".{e}")
        }
    };
    if i == "docx" && o == "pdf" {
        return Ok(Kind::DocxToPdf);
    }
    if i == "pptx" && o == "pdf" {
        return Ok(Kind::PptxToPdf);
    }
    if SHEET_INPUTS.contains(&i.as_str()) && SHEET_OUTPUTS.contains(&o.as_str()) {
        return Ok(Kind::Sheets);
    }
    let later = SHEET_INPUTS.contains(&i.as_str()) && o == "pdf"
        || i == "pdf" && matches!(o.as_str(), "png" | "jpg" | "jpeg");
    let why = if later {
        "is not supported yet"
    } else {
        "is not supported"
    };
    usage(format!(
        "converting {} to {} {why} ({})",
        show(&i),
        show(&o),
        input.display()
    ))
}

fn parse(args: &[OsString]) -> Result<Option<Options>, Usage> {
    let mut opts = Options::default();
    let mut positional = Vec::new();
    let mut dir = None;
    let mut to = None;
    let mut it = args.iter();
    let mut only_files = false;
    while let Some(arg) = it.next() {
        let s = arg.to_str().unwrap_or_default();
        if only_files || !s.starts_with('-') || s == "-" {
            positional.push(PathBuf::from(arg));
            continue;
        }
        let (flag, inline) = match s.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_owned())),
            _ => (s, None),
        };
        let mut value = |name: &str| -> Result<String, Usage> {
            match inline.clone() {
                Some(v) => Ok(v),
                None => match it.next() {
                    Some(v) => Ok(v.to_string_lossy().into_owned()),
                    None => usage(format!("{name} needs a value")),
                },
            }
        };
        match flag {
            "-h" | "--help" => return Ok(None),
            "-f" | "--force" => opts.force = true,
            "-o" | "--output-dir" => dir = Some(PathBuf::from(value(flag)?)),
            "-t" | "--to" => to = Some(value(flag)?.trim_start_matches('.').to_lowercase()),
            "--sheet" => opts.sheet = Some(value(flag)?),
            "--" => only_files = true,
            _ => return usage(format!("unknown option {s}")),
        }
    }
    match (dir, to) {
        (Some(dir), Some(to)) => {
            if positional.is_empty() {
                return usage("no input files");
            }
            if !matches!(to.as_str(), "pdf" | "xlsx" | "csv" | "tsv") {
                return usage(format!(
                    "--to {to}: Kantor converts to pdf, xlsx, csv or tsv"
                ));
            }
            for input in positional {
                let name = input
                    .file_stem()
                    .ok_or_else(|| Usage(format!("{} has no file name", input.display())))?;
                let mut out = dir.join(name);
                out.set_extension(&to);
                opts.jobs.push((input, out));
            }
        }
        (Some(_), None) => return usage("-o DIR needs --to FORMAT"),
        (None, Some(_)) => return usage("--to FORMAT goes with -o DIR"),
        (None, None) => match <[PathBuf; 2]>::try_from(positional) {
            Ok([input, output]) => opts.jobs.push((input, output)),
            Err(p) if p.len() < 2 => return usage("expected INPUT and OUTPUT"),
            Err(_) => return usage("more than one input: use -o DIR --to FORMAT"),
        },
    }
    for (input, output) in &opts.jobs {
        kind(input, output)?;
    }
    if opts.sheet.is_some()
        && !opts
            .jobs
            .iter()
            .any(|(_, o)| matches!(ext(o).as_str(), "csv" | "tsv"))
    {
        return usage("--sheet only applies to .csv/.tsv output");
    }
    Ok(Some(opts))
}

/// Run `kantor convert ARGS…`; returns the exit status.
pub fn main(args: &[OsString]) -> i32 {
    let opts = match parse(args) {
        Ok(Some(o)) => o,
        Ok(None) => {
            println!("{USAGE}");
            return 0;
        }
        Err(Usage(msg)) => {
            eprintln!("kantor convert: {msg}\n\n{USAGE}");
            return 2;
        }
    };
    let mut engine = None;
    let mut failed = 0;
    for (input, output) in &opts.jobs {
        match convert(input, output, &opts, &mut engine) {
            Ok(summary) => println!("{} -> {} ({summary})", input.display(), output.display()),
            Err(e) => {
                eprintln!("kantor convert: {}: {e:#}", input.display());
                failed += 1;
            }
        }
    }
    i32::from(failed > 0)
}

fn convert(
    input: &Path,
    output: &Path,
    opts: &Options,
    engine: &mut Option<docs_pdf::Engine>,
) -> Result<String> {
    if !input.is_file() {
        bail!("no such file");
    }
    if output.exists() {
        if same_file(input, output) {
            bail!("the output is the input file");
        }
        if !opts.force {
            bail!("{} exists (use -f to overwrite)", output.display());
        }
        if output.is_dir() {
            bail!("{} is a folder", output.display());
        }
    }
    if let Some(dir) = output.parent().filter(|d| !d.as_os_str().is_empty())
        && !dir.is_dir()
    {
        bail!("folder {} doesn't exist", dir.display());
    }
    let kind = kind(input, output).map_err(|Usage(m)| anyhow!(m))?;
    match kind {
        Kind::DocxToPdf => docx_to_pdf(input, output, engine),
        Kind::PptxToPdf => pptx_to_pdf(input, output, engine),
        Kind::Sheets => sheets(input, output, opts.sheet.as_deref()),
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn docx_to_pdf(
    input: &Path,
    output: &Path,
    engine: &mut Option<docs_pdf::Engine>,
) -> Result<String> {
    let doc = docs_engine::Document::open(input).context("can't open the document")?;
    // The font scan happens once, for the first document.
    let engine = engine.get_or_insert_with(docs_pdf::Engine::new);
    let paged = docs_pdf::lay_out(&doc, engine);
    let bytes = docs_pdf::write(&docs_pdf::scene(&doc, &paged))?;
    docs_pdf::write_file(output, &bytes)?;
    if doc.preserved_content().iter().any(|c| c.contains("header")) {
        eprintln!(
            "kantor convert: note: {}: headers and footers are not in the PDF yet",
            input.display()
        );
    }
    let pages = paged.pages.len().max(1);
    Ok(format!(
        "{pages} page{}, {}",
        if pages == 1 { "" } else { "s" },
        size(bytes.len())
    ))
}

fn pptx_to_pdf(
    input: &Path,
    output: &Path,
    engine: &mut Option<docs_pdf::Engine>,
) -> Result<String> {
    let pres = slides_engine::Presentation::open(input).context("can't open the presentation")?;
    let engine = engine.get_or_insert_with(docs_pdf::Engine::new);
    let (bytes, pages) = slides_pdf::export(&pres, engine)?;
    docs_pdf::write_file(output, &bytes)?;
    let hidden = (0..pres.len()).filter(|&i| pres.slide(i).hidden).count();
    let what = pres.unsupported_objects();
    if !what.is_empty() {
        eprintln!(
            "kantor convert: note: {}: not in the PDF yet: {}",
            input.display(),
            what.join(", ")
        );
    }
    Ok(format!(
        "{pages} page{}{}, {}",
        if pages == 1 { "" } else { "s" },
        match hidden {
            0 => String::new(),
            1 => ", 1 hidden slide left out".to_owned(),
            n => format!(", {n} hidden slides left out"),
        },
        size(bytes.len())
    ))
}

fn sheets(input: &Path, output: &Path, sheet: Option<&str>) -> Result<String> {
    // Opening keeps the file's cached formula values (no recalculation of
    // large workbooks); imports are evaluated by the importer.
    let mut book = sheets_engine::Workbook::open(input).context("can't open the workbook")?;
    for note in book.import_notes() {
        eprintln!("kantor convert: note: {}: {note}", input.display());
    }
    let names = book.sheet_names();
    if let Some(want) = sheet {
        let index = names
            .iter()
            .position(|n| n == want)
            .or_else(|| {
                want.parse::<usize>()
                    .ok()
                    .filter(|n| (1..=names.len()).contains(n))
                    .map(|n| n - 1)
            })
            .ok_or_else(|| {
                anyhow!(
                    "no sheet {want:?}; the sheets are {}",
                    names
                        .iter()
                        .enumerate()
                        .map(|(i, n)| format!("{} {n:?}", i + 1))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        book.set_active_sheet(index as u32)?;
    }
    let delimited = matches!(ext(output).as_str(), "csv" | "tsv");
    book.save_as(output)?;
    for note in book.save_notes() {
        eprintln!("kantor convert: note: {}: {note}", output.display());
    }
    let written = std::fs::metadata(output).map(|m| m.len() as usize)?;
    if delimited {
        let active = book.active_sheet() as usize;
        let name = names.get(active).map_or("", String::as_str);
        let left = names.len().saturating_sub(1);
        let rest = match left {
            0 => String::new(),
            1 => ", 1 other sheet not written".to_owned(),
            n => format!(", {n} other sheets not written"),
        };
        Ok(format!("sheet {name:?}{rest}, {}", size(written)))
    } else {
        let n = names.len();
        Ok(format!(
            "{n} sheet{}, {}",
            if n == 1 { "" } else { "s" },
            size(written)
        ))
    }
}

fn size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<OsString> {
        s.iter().map(OsString::from).collect()
    }

    fn err(s: &[&str]) -> String {
        parse(&args(s)).unwrap_err().0
    }

    #[std::prelude::v1::test]
    fn two_files_or_a_folder_and_a_format() {
        let o = parse(&args(&["a.docx", "b.pdf"])).unwrap().unwrap();
        assert_eq!(o.jobs, vec![("a.docx".into(), "b.pdf".into())]);
        assert!(!o.force);
        let o = parse(&args(&[
            "-f",
            "-o",
            "out",
            "--to=.PDF",
            "x/a.docx",
            "b.docx",
        ]))
        .unwrap()
        .unwrap();
        assert!(o.force);
        assert_eq!(
            o.jobs,
            vec![
                ("x/a.docx".into(), "out/a.pdf".into()),
                ("b.docx".into(), "out/b.pdf".into())
            ]
        );
        assert!(parse(&args(&["--help"])).unwrap().is_none());
        let o = parse(&args(&["--sheet", "2", "a.ods", "b.csv"]))
            .unwrap()
            .unwrap();
        assert_eq!(o.sheet.as_deref(), Some("2"));
    }

    #[std::prelude::v1::test]
    fn usage_errors_say_what_is_wrong() {
        assert!(err(&["a.docx"]).contains("expected INPUT and OUTPUT"));
        assert!(err(&["a.docx", "b.docx", "c.pdf"]).contains("-o DIR --to FORMAT"));
        assert!(err(&["-o", "out", "a.docx"]).contains("needs --to"));
        assert!(err(&["--bogus", "a", "b"]).contains("unknown option --bogus"));
        assert!(err(&["--sheet"]).contains("needs a value"));
        assert!(err(&["--sheet", "1", "a.docx", "b.pdf"]).contains("only applies to .csv"));
        assert!(err(&["-o", "d", "-t", "odt", "a.docx"]).contains("pdf, xlsx, csv or tsv"));
    }

    #[std::prelude::v1::test]
    fn unsupported_pairs_are_named() {
        assert!(err(&["a.xlsx", "a.pdf"]).contains(".xlsx to .pdf is not supported yet"));
        assert_eq!(
            kind(Path::new("deck.PPTX"), Path::new("deck.pdf")).unwrap(),
            Kind::PptxToPdf
        );
        assert!(err(&["a.pptx", "a.png"]).contains(".pptx to .png is not supported ("));
        assert!(err(&["a.pdf", "a.png"]).contains("not supported yet"));
        assert!(err(&["a.docx", "a.xlsx"]).contains(".docx to .xlsx is not supported ("));
        assert!(err(&["a", "b.pdf"]).contains("(no extension)"));
        assert_eq!(
            kind(Path::new("A.DOCX"), Path::new("b.Pdf")).unwrap(),
            Kind::DocxToPdf
        );
        assert_eq!(
            kind(Path::new("a.ods"), Path::new("b.tsv")).unwrap(),
            Kind::Sheets
        );
    }
}
