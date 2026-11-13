//! `kantor convert`: convert documents without opening a window (no GPUI).
//!
//! ```text
//! kantor convert report.docx report.pdf
//! kantor convert letter.rtf letter.docx   (also .doc / .dot)
//! kantor convert deck.pptx deck.pdf
//! kantor convert old.ppt old.pptx
//! kantor convert --fit-width --landscape book.xlsx book.pdf
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
    \x20 .rtf .doc .dot (Word 97-2003)              -> .docx .pdf\n\
    \x20 .pptx .ppt .pps .pot                       -> .pdf (one page per slide,\n\
    \x20                                               hidden slides left out)\n\
    \x20 .ppt .pps .pot                             -> .pptx\n\
    \x20 .xlsx .xlsm .xls .xlt .xlsb .ods .csv .tsv -> .xlsx .csv .tsv\n\
    \x20 .xlsx .xlsm .xls .xlt .xlsb .ods .csv .tsv -> .pdf (printed as Excel prints:\n\
    \x20                                               every visible sheet, or --sheet)\n\
    \n\
    Options:\n\
    \x20 -o, --output-dir DIR   write each INPUT to DIR/NAME.FORMAT\n\
    \x20 -t, --to FORMAT        output format with -o: pdf, docx, xlsx, pptx, csv or tsv\n\
    \x20     --sheet N|NAME     sheet to write to .csv/.tsv or .pdf (1-based number\n\
    \x20                        or name; default: the active sheet for .csv/.tsv,\n\
    \x20                        every visible sheet for .pdf)\n\
    \x20 Spreadsheet to PDF (each overrides the sheets' own page setup):\n\
    \x20     --fit-width        fit each sheet to one page wide\n\
    \x20     --fit-page         fit each sheet on one page\n\
    \x20     --scale PERCENT    print at this scale (10-400)\n\
    \x20     --landscape        landscape orientation\n\
    \x20     --portrait         portrait orientation\n\
    \x20     --paper SIZE       a4, letter, legal, f4 (folio), a3, a5, or WxHmm / WxHin\n\
    \x20     --no-print-area    print each sheet's used range, not its print area\n\
    \x20 Presentation to PDF:\n\
    \x20     --notes            notes pages: each slide above its speaker notes (A4)\n\
    \x20     --handouts N       N slides per A4 page: 1, 2, 3 (with lines), 4, 6 or 9\n\
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
    /// Spreadsheet → PDF overrides.
    print: Print,
    /// Presentation → PDF layout (notes pages, handouts).
    slides: Option<slides_pdf::PrintLayout>,
}

/// Page setup given on the command line for spreadsheet → PDF.
#[derive(Debug, Default, PartialEq)]
struct Print {
    fit: Option<(u32, u32)>,
    scale: Option<u32>,
    landscape: Option<bool>,
    paper: Option<sheets_engine::Paper>,
    no_print_area: bool,
}

impl Print {
    fn given(&self) -> bool {
        *self != Print::default()
    }
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
    "xlsx", "xlsm", "xls", "xla", "xlt", "xlsb", "ods", "csv", "tsv", "tab",
];
const SHEET_OUTPUTS: &[&str] = &["xlsx", "csv", "tsv"];
const SLIDES_INPUTS: &[&str] = &["pptx", "ppt", "pps", "pot"];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    DocxToPdf,
    /// An imported document (.rtf, .doc, .dot) written as .docx.
    DocsToDocx,
    PptxToPdf,
    /// PowerPoint 97-2003 to `.pptx`.
    PptToPptx,
    SheetsToPdf,
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
    if matches!(i.as_str(), "rtf" | "doc" | "dot") {
        match o.as_str() {
            "pdf" => return Ok(Kind::DocxToPdf),
            "docx" => return Ok(Kind::DocsToDocx),
            _ => {}
        }
    }
    if SLIDES_INPUTS.contains(&i.as_str()) && o == "pdf" {
        return Ok(Kind::PptxToPdf);
    }
    if i != "pptx" && SLIDES_INPUTS.contains(&i.as_str()) && o == "pptx" {
        return Ok(Kind::PptToPptx);
    }
    if SHEET_INPUTS.contains(&i.as_str()) && SHEET_OUTPUTS.contains(&o.as_str()) {
        return Ok(Kind::Sheets);
    }
    if SHEET_INPUTS.contains(&i.as_str()) && o == "pdf" {
        return Ok(Kind::SheetsToPdf);
    }
    let later = i == "pdf" && matches!(o.as_str(), "png" | "jpg" | "jpeg");
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
            "--fit-width" => opts.print.fit = Some((1, 0)),
            "--fit-page" => opts.print.fit = Some((1, 1)),
            "--landscape" => opts.print.landscape = Some(true),
            "--portrait" => opts.print.landscape = Some(false),
            "--no-print-area" => opts.print.no_print_area = true,
            "--notes" => opts.slides = Some(slides_pdf::PrintLayout::Notes),
            "--handouts" => {
                let v = value(flag)?;
                match v.parse::<u8>() {
                    Ok(n) if slides_pdf::PrintLayout::HANDOUTS.contains(&n) => {
                        opts.slides = Some(slides_pdf::PrintLayout::Handouts(n))
                    }
                    _ => return usage(format!("--handouts {v}: use 1, 2, 3, 4, 6 or 9")),
                }
            }
            "--scale" => {
                let v = value(flag)?;
                match v.trim_end_matches('%').parse::<u32>() {
                    Ok(n) if (10..=400).contains(&n) => opts.print.scale = Some(n),
                    _ => return usage(format!("--scale {v}: give a percentage from 10 to 400")),
                }
            }
            "--paper" => {
                let v = value(flag)?;
                match sheets_engine::Paper::from_name(&v) {
                    Some(p) => opts.print.paper = Some(p),
                    None => {
                        return usage(format!(
                            "--paper {v}: use a4, letter, legal, f4, a3, a5, or WxHmm / WxHin"
                        ));
                    }
                }
            }
            "--" => only_files = true,
            _ => return usage(format!("unknown option {s}")),
        }
    }
    match (dir, to) {
        (Some(dir), Some(to)) => {
            if positional.is_empty() {
                return usage("no input files");
            }
            if !matches!(to.as_str(), "pdf" | "docx" | "xlsx" | "pptx" | "csv" | "tsv") {
                return usage(format!(
                    "--to {to}: Kantor converts to pdf, docx, xlsx, pptx, csv or tsv"
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
    let to_pdf = |opts: &Options| {
        opts.jobs
            .iter()
            .any(|(i, o)| kind(i, o).ok() == Some(Kind::SheetsToPdf))
    };
    if opts.sheet.is_some()
        && !to_pdf(&opts)
        && !opts
            .jobs
            .iter()
            .any(|(_, o)| matches!(ext(o).as_str(), "csv" | "tsv"))
    {
        return usage("--sheet only applies to .csv/.tsv output or spreadsheets to .pdf");
    }
    if opts.print.given() && !to_pdf(&opts) {
        return usage("page setup options only apply to spreadsheets converted to .pdf");
    }
    if opts.slides.is_some()
        && !opts
            .jobs
            .iter()
            .all(|(i, o)| kind(i, o).ok() == Some(Kind::PptxToPdf))
    {
        return usage("--notes and --handouts only apply to presentations converted to .pdf");
    }
    if opts.print.fit.is_some() && opts.print.scale.is_some() {
        return usage("--scale and --fit-width / --fit-page don't go together");
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
        Kind::DocsToDocx => docs_to_docx(input, output),
        Kind::PptxToPdf => pptx_to_pdf(input, output, engine, opts.slides),
        Kind::PptToPptx => ppt_to_pptx(input, output),
        Kind::Sheets => sheets(input, output, opts.sheet.as_deref()),
        Kind::SheetsToPdf => sheets_to_pdf(input, output, opts),
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
    import_notes(input, &doc);
    // The font scan happens once, for the first document.
    let engine = engine.get_or_insert_with(docs_pdf::Engine::new);
    let laid = docs_pdf::lay_out(&doc, engine);
    let bytes = docs_pdf::write(&docs_pdf::scene(&doc, &laid))?;
    docs_pdf::write_file(output, &bytes)?;
    let pages = laid.paged.pages.len().max(1);
    Ok(format!(
        "{pages} page{}, {}",
        if pages == 1 { "" } else { "s" },
        size(bytes.len())
    ))
}

fn import_notes(input: &Path, doc: &docs_engine::Document) {
    for note in doc.import_notes() {
        eprintln!("kantor convert: note: {}: {note}", input.display());
    }
}

fn docs_to_docx(input: &Path, output: &Path) -> Result<String> {
    let mut doc = docs_engine::Document::open(input).context("can't open the document")?;
    import_notes(input, &doc);
    doc.save_as(output)?;
    let written = std::fs::metadata(output).map(|m| m.len() as usize)?;
    let n = doc.len();
    Ok(format!(
        "{n} paragraph{}, {}",
        if n == 1 { "" } else { "s" },
        size(written)
    ))
}

fn pptx_to_pdf(
    input: &Path,
    output: &Path,
    engine: &mut Option<docs_pdf::Engine>,
    layout: Option<slides_pdf::PrintLayout>,
) -> Result<String> {
    let pres = slides_engine::Presentation::open(input).context("can't open the presentation")?;
    for note in pres.import_notes() {
        eprintln!("kantor convert: note: {}: {note}", input.display());
    }
    let engine = engine.get_or_insert_with(docs_pdf::Engine::new);
    let (bytes, pages) = match layout {
        None => slides_pdf::export(&pres, engine)?,
        Some(layout) => {
            let job = slides_pdf::print::job_headless(&pres, engine, layout);
            slides_pdf::print::write_job(&job)?
        }
    };
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

fn ppt_to_pptx(input: &Path, output: &Path) -> Result<String> {
    let mut pres =
        slides_engine::Presentation::open(input).context("can't open the presentation")?;
    for note in pres.import_notes() {
        eprintln!("kantor convert: note: {}: {note}", input.display());
    }
    pres.save_as(output)?;
    let written = std::fs::metadata(output).map(|m| m.len() as usize)?;
    let n = pres.len();
    Ok(format!(
        "{n} slide{}, {}",
        if n == 1 { "" } else { "s" },
        size(written)
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
        book.set_active_sheet(sheet_index(&names, want)?)?;
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

/// A sheet by name or 1-based number.
fn sheet_index(names: &[String], want: &str) -> Result<u32> {
    names
        .iter()
        .position(|n| n == want)
        .or_else(|| {
            want.parse::<usize>()
                .ok()
                .filter(|n| (1..=names.len()).contains(n))
                .map(|n| n - 1)
        })
        .map(|i| i as u32)
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
        })
}

fn sheets_to_pdf(input: &Path, output: &Path, opts: &Options) -> Result<String> {
    let mut book = sheets_engine::Workbook::open(input).context("can't open the workbook")?;
    for note in book.import_notes() {
        eprintln!("kantor convert: note: {}: {note}", input.display());
    }
    let names = book.sheet_names();
    let sheets = match opts.sheet.as_deref() {
        Some(want) => Some(vec![sheet_index(&names, want)?]),
        None => None,
    };
    let p = &opts.print;
    let print = sheets_pdf::Options {
        sheets,
        paper: p.paper,
        landscape: p.landscape,
        fit: p.fit,
        scale: p.scale,
        ignore_print_area: p.no_print_area,
        ..sheets_pdf::Options::default()
    }
    .now()
    .with_path(input.canonicalize().ok().as_deref());
    let (bytes, pages) = sheets_pdf::export(&mut book, &print)?;
    docs_pdf::write_file(output, &bytes)?;
    Ok(format!(
        "{pages} page{}, {}",
        if pages == 1 { "" } else { "s" },
        size(bytes.len())
    ))
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
        let o = parse(&args(&[
            "--sheet",
            "Sales",
            "--fit-width",
            "--landscape",
            "--paper",
            "F4",
            "a.xlsx",
            "b.pdf",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(o.print.fit, Some((1, 0)));
        assert_eq!(o.print.landscape, Some(true));
        assert_eq!(o.print.paper, Some(sheets_engine::Paper::FOLIO));
        let o = parse(&args(&["--handouts", "6", "a.pptx", "b.pdf"]))
            .unwrap()
            .unwrap();
        assert_eq!(o.slides, Some(slides_pdf::PrintLayout::Handouts(6)));
        let o = parse(&args(&["--notes", "a.pptx", "b.pdf"]))
            .unwrap()
            .unwrap();
        assert_eq!(o.slides, Some(slides_pdf::PrintLayout::Notes));
    }

    #[std::prelude::v1::test]
    fn usage_errors_say_what_is_wrong() {
        assert!(err(&["a.docx"]).contains("expected INPUT and OUTPUT"));
        assert!(err(&["a.docx", "b.docx", "c.pdf"]).contains("-o DIR --to FORMAT"));
        assert!(err(&["-o", "out", "a.docx"]).contains("needs --to"));
        assert!(err(&["--bogus", "a", "b"]).contains("unknown option --bogus"));
        assert!(err(&["--sheet"]).contains("needs a value"));
        assert!(err(&["--sheet", "1", "a.docx", "b.pdf"]).contains("only applies to .csv"));
        assert!(err(&["--fit-width", "a.xlsx", "b.csv"]).contains("only apply to spreadsheets"));
        assert!(err(&["--handouts", "5", "a.pptx", "b.pdf"]).contains("use 1, 2, 3, 4, 6 or 9"));
        assert!(err(&["--notes", "a.docx", "b.pdf"]).contains("only apply to presentations"));
        assert!(err(&["--paper", "b9", "a.xlsx", "b.pdf"]).contains("--paper b9"));
        assert!(err(&["--scale", "5", "a.xlsx", "b.pdf"]).contains("10 to 400"));
        assert!(err(&["--scale", "50", "--fit-page", "a.xlsx", "b.pdf"]).contains("don't go"));
        assert!(err(&["-o", "d", "-t", "odt", "a.docx"]).contains("pdf, docx, xlsx, pptx, csv or tsv"));
    }

    #[std::prelude::v1::test]
    fn unsupported_pairs_are_named() {
        assert_eq!(
            kind(Path::new("a.xlsx"), Path::new("a.pdf")).unwrap(),
            Kind::SheetsToPdf
        );
        assert_eq!(
            kind(Path::new("deck.PPTX"), Path::new("deck.pdf")).unwrap(),
            Kind::PptxToPdf
        );
        assert!(err(&["a.pptx", "a.png"]).contains(".pptx to .png is not supported ("));
        for ext in ["ppt", "PPS", "pot"] {
            let input = format!("old.{ext}");
            assert_eq!(
                kind(Path::new(&input), Path::new("old.pdf")).unwrap(),
                Kind::PptxToPdf
            );
            assert_eq!(
                kind(Path::new(&input), Path::new("new.pptx")).unwrap(),
                Kind::PptToPptx
            );
        }
        assert!(err(&["a.pptx", "b.pptx"]).contains(".pptx to .pptx is not supported"));
        let o = parse(&args(&["--notes", "a.ppt", "b.pdf"]))
            .unwrap()
            .unwrap();
        assert_eq!(o.slides, Some(slides_pdf::PrintLayout::Notes));
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
        assert_eq!(
            kind(Path::new("a.RTF"), Path::new("b.pdf")).unwrap(),
            Kind::DocxToPdf
        );
        assert_eq!(
            kind(Path::new("a.rtf"), Path::new("b.docx")).unwrap(),
            Kind::DocsToDocx
        );
        assert!(err(&["a.rtf", "a.xlsx"]).contains(".rtf to .xlsx is not supported ("));
        assert!(err(&["a.docx", "b.docx"]).contains(".docx to .docx is not supported ("));
        assert_eq!(
            kind(Path::new("old.DOC"), Path::new("b.pdf")).unwrap(),
            Kind::DocxToPdf
        );
        assert_eq!(
            kind(Path::new("t.dot"), Path::new("t.docx")).unwrap(),
            Kind::DocsToDocx
        );
        assert!(err(&["a.docx", "b.doc"]).contains("not supported ("));
    }
}
