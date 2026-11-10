//! `kantor pdf <tool>`: PDF page tools on the command line, with no window.
//! The work is done by [`kantor_pdf`]; this module parses arguments, reads
//! and writes files, and prints one line per result.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use kantor_pdf::{
    ImageOptions, Orientation, PageNumberOptions, PageRanges, PageSize, Pdf, Position,
    WatermarkOptions,
};

/// A failed run: the message and the exit code (2 for usage, 1 otherwise).
#[derive(Debug)]
pub struct Failure {
    pub message: String,
    pub code: i32,
}

impl Failure {
    fn usage(message: impl Into<String>) -> Failure {
        Failure {
            message: message.into(),
            code: 2,
        }
    }
}

impl From<kantor_pdf::Error> for Failure {
    fn from(e: kantor_pdf::Error) -> Self {
        Failure {
            message: e.to_string(),
            code: 1,
        }
    }
}

impl From<kantor_pdf::RangeError> for Failure {
    fn from(e: kantor_pdf::RangeError) -> Self {
        Failure::usage(e.to_string())
    }
}

type Outcome = Result<String, Failure>;

/// A tool: its arguments (after the tool name) and usage text.
type Tool = fn(&[String], &str) -> Outcome;

pub const USAGE: &str = "usage: kantor pdf <tool> [options]\n\
\n\
PDF tools that run without opening a window. Pages are numbered from 1.\n\
\n\
tools:\n\
\x20 merge         join PDFs into one (optionally only some pages of each)\n\
\x20 split         split into several PDFs: by ranges, every N pages, or one per page\n\
\x20 extract       keep only some pages, in the order given\n\
\x20 delete        remove pages\n\
\x20 reorder       put the pages in a new order\n\
\x20 rotate        turn pages by 90, 180 or 270 degrees\n\
\x20 info          page count, page sizes, title/author/producer, encryption\n\
\x20 images        make a PDF from JPEG/PNG images (one page each)\n\
\x20 watermark     stamp text across pages\n\
\x20 page-numbers  number the pages\n\
\x20 compress      make the file smaller without losing quality\n\
\n\
Run `kantor pdf <tool> --help` for a tool's options.\n\
\n\
Page ranges: 1-3,5,8- (8 to the end), -3 (1 to 3), 5-2 (counting down),\n\
last, last-1 (every page, reversed), all.\n\
Options for every tool that writes a PDF:\n\
\x20 -o, --output PATH    where to write (default: next to the input, with a suffix)\n\
\x20 -f, --force          overwrite PATH if it exists\n\
\x20     --password PW    open an encrypted PDF ('' for one with no open password;\n\
\x20                      or set KANTOR_PDF_PASSWORD). Results are not encrypted.";

const MERGE: &str = "usage: kantor pdf merge FILE[:PAGES]... -o OUT.pdf [-f]\n\
\n\
Join PDFs in the order given. Add :PAGES to a file to take only those pages,\n\
in that order: `kantor pdf merge cover.pdf report.pdf:2- -o out.pdf`.\n\
The first file's outline and metadata are kept.";

const SPLIT: &str = "usage: kantor pdf split FILE (--ranges R | --every N | --each) [-o DIR] [-f]\n\
\n\
\x20 --ranges R   one output per comma-separated item: 1-3,4-6,7- gives three files\n\
\x20 --every N    one output per N pages\n\
\x20 --each       one output per page (same as --every 1)\n\
\x20 -o DIR       folder for the parts (made if missing; default: next to FILE)\n\
\n\
Parts are named FILE-PAGES.pdf, like report-1-3.pdf and report-7.pdf.";

const EXTRACT: &str = "usage: kantor pdf extract FILE PAGES [-o OUT.pdf] [-f]\n\
\n\
Keep only PAGES, in the order written; a page may repeat (`3,1-2,3`).\n\
Default output: FILE-extracted.pdf.";

const DELETE: &str = "usage: kantor pdf delete FILE PAGES [-o OUT.pdf] [-f]\n\
\n\
Remove PAGES (`2,5-7`, `last`). Default output: FILE-deleted.pdf.";

const REORDER: &str = "usage: kantor pdf reorder FILE ORDER [-o OUT.pdf] [-f]\n\
\n\
ORDER lists every page once, like `3,1,2,4-`; `last-1` reverses the PDF.\n\
Default output: FILE-reordered.pdf. To pick or repeat pages, use extract.";

const ROTATE: &str = "usage: kantor pdf rotate FILE DEGREES [--pages PAGES] [-o OUT.pdf] [-f]\n\
\n\
Turn pages clockwise by DEGREES: 90, 180, 270 (or -90, left, right).\n\
Adds to any rotation a page already has. --pages limits it (default: all).\n\
Default output: FILE-rotated.pdf.";

const INFO: &str = "usage: kantor pdf info FILE [--json] [--password PW]\n\
\n\
Show the page count, each page's size (as displayed, in points and mm),\n\
title, author, subject, creator, producer, PDF version and encryption.";

const IMAGES: &str = "usage: kantor pdf images IMAGE... -o OUT.pdf [options]\n\
\n\
One page per JPEG or PNG, in the order given. JPEGs are embedded unchanged;\n\
PNGs losslessly (with transparency). Photos are turned upright (EXIF).\n\
\x20 --page-size S      fit (page = image size, default), a4 or letter\n\
\x20 --orientation O    auto (default; landscape for wide images), portrait, landscape\n\
\x20 --margin PT        space around each image in points (default 0; 36 = 0.5 in)";

const WATERMARK: &str = "usage: kantor pdf watermark FILE --text TEXT [options] [-o OUT.pdf] [-f]\n\
\n\
\x20 --text TEXT       the watermark (Latin characters; Helvetica)\n\
\x20 --size PT         font size (default 48)\n\
\x20 --opacity A       0 to 1, or a percentage like 30% (default 0.3)\n\
\x20 --rotation DEG    counter-clockwise; negative turns clockwise (default 45)\n\
\x20 --position P      center (default), top-left, top-center, top-right,\n\
\x20                   bottom-left, bottom-center, bottom-right\n\
\x20 --color HEX       like 808080 or #c00 (default 808080)\n\
\x20 --pages PAGES     which pages (default: all)\n\
Default output: FILE-watermarked.pdf.";

const PAGE_NUMBERS: &str = "usage: kantor pdf page-numbers FILE [options] [-o OUT.pdf] [-f]\n\
\n\
\x20 --format F        {n} is the number, {total} the last number (default \"{n}\";\n\
\x20                   e.g. \"{n} of {total}\", \"Page {n}\")\n\
\x20 --position P      bottom-center (default), bottom-left, bottom-right,\n\
\x20                   top-left, top-center, top-right\n\
\x20 --start N         number on the first numbered page (default 1)\n\
\x20 --pages PAGES     which pages to number (default: all)\n\
\x20 --size PT         font size (default 12)\n\
\x20 --margin PT       distance from the page edge (default 30)\n\
Default output: FILE-numbered.pdf.";

const COMPRESS: &str = "usage: kantor pdf compress FILE [-o OUT.pdf] [-f]\n\
\n\
Lossless: removes unused objects, merges identical streams, compresses\n\
uncompressed streams and packs objects into object streams. Images are\n\
not re-encoded. If that doesn't make the file smaller, the output is a copy.\n\
Default output: FILE-compressed.pdf.";

/// Parsed arguments for one tool.
#[derive(Debug, Default)]
struct Args {
    positional: Vec<String>,
    values: Vec<(&'static str, String)>,
    flags: Vec<&'static str>,
}

impl Args {
    /// Parse `args` given the tool's value options and flag options (long
    /// names). `-o`, `-f` and `-h` are short for output, force and help.
    fn parse(
        args: &[String],
        value_opts: &[&'static str],
        flag_opts: &[&'static str],
        usage: &str,
    ) -> Result<Args, Failure> {
        let mut out = Args::default();
        let mut iter = args.iter();
        let mut only_positional = false;
        while let Some(arg) = iter.next() {
            if only_positional || arg == "-" || !arg.starts_with('-') || is_negative_number(arg) {
                out.positional.push(arg.clone());
                continue;
            }
            if arg == "--" {
                only_positional = true;
                continue;
            }
            let (name, inline) = match arg.split_once('=') {
                Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
                _ => (arg.as_str(), None),
            };
            let long = match name {
                "-o" => "--output",
                "-f" => "--force",
                "-h" => "--help",
                n => n,
            };
            let long = long.strip_prefix("--").unwrap_or("");
            if let Some(&key) = value_opts.iter().find(|k| **k == long) {
                let value = match inline {
                    Some(v) => v,
                    None => iter.next().cloned().ok_or_else(|| {
                        Failure::usage(format!("{name} needs a value\n\n{usage}"))
                    })?,
                };
                out.values.retain(|(k, _)| *k != key);
                out.values.push((key, value));
            } else if let Some(&key) = flag_opts.iter().find(|k| **k == long) {
                if inline.is_some() {
                    return Err(Failure::usage(format!("{name} takes no value\n\n{usage}")));
                }
                out.flags.push(key);
            } else {
                return Err(Failure::usage(format!("unknown option {name}\n\n{usage}")));
            }
        }
        Ok(out)
    }

    fn value(&self, key: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }

    fn flag(&self, key: &str) -> bool {
        self.flags.contains(&key)
    }

    fn parse_value<T: std::str::FromStr>(
        &self,
        key: &str,
        what: &str,
    ) -> Result<Option<T>, Failure> {
        self.value(key)
            .map(|v| {
                v.trim()
                    .parse::<T>()
                    .map_err(|_| Failure::usage(format!("--{key}: \"{v}\" is not {what}")))
            })
            .transpose()
    }

    fn password(&self) -> Option<String> {
        self.value("password")
            .map(str::to_owned)
            .or_else(|| std::env::var("KANTOR_PDF_PASSWORD").ok())
    }

    /// Exactly `n` positional arguments, named for the error.
    fn expect(&self, names: &[&str], usage: &str) -> Result<(), Failure> {
        if self.positional.len() < names.len() {
            let missing = names[self.positional.len()..].join(" ");
            return Err(Failure::usage(format!("missing {missing}\n\n{usage}")));
        }
        if self.positional.len() > names.len() {
            return Err(Failure::usage(format!(
                "unexpected argument \"{}\"\n\n{usage}",
                self.positional[names.len()]
            )));
        }
        Ok(())
    }
}

fn is_negative_number(s: &str) -> bool {
    s.len() > 1 && s.starts_with('-') && s[1..].bytes().all(|b| b.is_ascii_digit())
}

const WRITE_OPTS: [&str; 2] = ["output", "password"];
const WRITE_FLAGS: [&str; 2] = ["force", "help"];

fn opts<'a>(extra: &[&'a str], base: &[&'a str]) -> Vec<&'a str> {
    base.iter().chain(extra).copied().collect()
}

/// Run `kantor pdf ARGS`; returns the exit code.
pub fn main(args: Vec<OsString>) -> i32 {
    let args: Result<Vec<String>, _> = args
        .into_iter()
        .map(|a| {
            a.into_string()
                .map_err(|a| a.to_string_lossy().into_owned())
        })
        .collect();
    let result = match args {
        Ok(args) => run(&args),
        Err(bad) => Err(Failure::usage(format!(
            "the argument \"{bad}\" is not valid UTF-8"
        ))),
    };
    match result {
        Ok(summary) => {
            print!("{summary}");
            if !summary.is_empty() && !summary.ends_with('\n') {
                println!();
            }
            0
        }
        Err(f) => {
            eprintln!("kantor pdf: {}", f.message);
            f.code
        }
    }
}

/// Run a tool and return what to print.
pub fn run(args: &[String]) -> Outcome {
    let Some((tool, rest)) = args.split_first() else {
        return Err(Failure::usage(USAGE));
    };
    let (usage, handler): (&str, Tool) = match tool.as_str() {
        "-h" | "--help" | "help" => return Ok(format!("{USAGE}\n")),
        "merge" => (MERGE, merge),
        "split" => (SPLIT, split),
        "extract" => (EXTRACT, extract),
        "delete" => (DELETE, delete),
        "reorder" => (REORDER, reorder),
        "rotate" => (ROTATE, rotate),
        "info" => (INFO, info),
        "images" | "images-to-pdf" | "img2pdf" => (IMAGES, images),
        "watermark" => (WATERMARK, watermark),
        "page-numbers" | "number" | "numbers" => (PAGE_NUMBERS, page_numbers),
        "compress" => (COMPRESS, compress),
        other => {
            return Err(Failure::usage(format!(
                "unknown tool \"{other}\"\n\n{USAGE}"
            )));
        }
    };
    if rest.iter().any(|a| a == "-h" || a == "--help") {
        return Ok(format!("{usage}\n"));
    }
    handler(rest, usage)
}

/// `report.pdf` + `rotated` → `report-rotated.pdf` in the same folder.
fn suffixed(input: &Path, suffix: &str) -> PathBuf {
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".into());
    input.with_file_name(format!("{stem}-{suffix}.pdf"))
}

fn output_path(args: &Args, input: &Path, suffix: &str) -> PathBuf {
    args.value("output")
        .map(PathBuf::from)
        .unwrap_or_else(|| suffixed(input, suffix))
}

fn required_output(args: &Args, usage: &str) -> Result<PathBuf, Failure> {
    args.value("output")
        .map(PathBuf::from)
        .ok_or_else(|| Failure::usage(format!("give the output file with -o OUT.pdf\n\n{usage}")))
}

fn check_overwrite(path: &Path, force: bool) -> Result<(), Failure> {
    if path.exists() && !force {
        return Err(Failure {
            message: format!("{} already exists (use -f to overwrite)", path.display()),
            code: 1,
        });
    }
    if path.is_dir() {
        return Err(Failure {
            message: format!("{} is a folder, not a file", path.display()),
            code: 1,
        });
    }
    Ok(())
}

/// Write via a temporary file in the same folder, so a failed write never
/// leaves half a file (or clobbers the input when writing over it).
fn write_file(path: &Path, bytes: &[u8]) -> Result<(), Failure> {
    let io_err = |e: std::io::Error| Failure {
        message: format!("can't write {}: {e}", path.display()),
        code: 1,
    };
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.kantor-{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(io_err)?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io_err(e)
    })
}

fn open(path: &Path, args: &Args) -> Result<(Pdf, usize), Failure> {
    let bytes = kantor_pdf::read(path)?;
    let pdf = Pdf::from_bytes(&bytes, args.password().as_deref())
        .map_err(|e| Failure::from(e).prefixed(path))?;
    Ok((pdf, bytes.len()))
}

impl Failure {
    fn prefixed(mut self, path: &Path) -> Failure {
        self.message = format!("{}: {}", path.display(), self.message);
        self
    }
}

fn save(pdf: &mut Pdf, path: &Path) -> Result<usize, Failure> {
    let bytes = pdf.to_bytes()?;
    write_file(path, &bytes)?;
    Ok(bytes.len())
}

fn ranges(s: &str) -> Result<PageRanges, Failure> {
    Ok(PageRanges::parse(s)?)
}

/// `1 page` / `3 pages`.
fn pages(n: u32) -> String {
    format!("{n} page{}", if n == 1 { "" } else { "s" })
}

pub fn human_size(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{:.1} MB", b / KB / KB)
    }
}

/// Load the input, apply `edit`, write the output, and describe it.
fn edit_one(
    rest: &[String],
    usage: &str,
    extra_values: &[&'static str],
    positional: &[&str],
    suffix: &str,
    edit: impl FnOnce(&mut Pdf, &Args) -> Result<String, Failure>,
) -> Outcome {
    let args = Args::parse(rest, &opts(extra_values, &WRITE_OPTS), &WRITE_FLAGS, usage)?;
    args.expect(positional, usage)?;
    let input = PathBuf::from(&args.positional[0]);
    let output = output_path(&args, &input, suffix);
    check_overwrite(&output, args.flag("force"))?;
    let (mut pdf, _) = open(&input, &args)?;
    let what = edit(&mut pdf, &args).map_err(|f| f.prefixed(&input))?;
    let pages_out = pdf.page_count();
    save(&mut pdf, &output)?;
    Ok(format!(
        "{}: {what} → {} ({})",
        input.display(),
        output.display(),
        pages(pages_out)
    ))
}

fn merge(rest: &[String], usage: &str) -> Outcome {
    let args = Args::parse(rest, &WRITE_OPTS, &WRITE_FLAGS, usage)?;
    if args.positional.is_empty() {
        return Err(Failure::usage(format!("give the PDFs to merge\n\n{usage}")));
    }
    let output = required_output(&args, usage)?;
    check_overwrite(&output, args.flag("force"))?;
    let mut docs = Vec::new();
    let mut total = 0;
    for spec in &args.positional {
        // `file.pdf:1-3`, unless a file with that whole name exists.
        let (path, range) = match spec.rsplit_once(':') {
            Some((p, r)) if !Path::new(spec).exists() && !p.is_empty() => {
                (PathBuf::from(p), Some(r))
            }
            _ => (PathBuf::from(spec), None),
        };
        let (mut pdf, _) = open(&path, &args)?;
        if let Some(r) = range {
            let r = ranges(r).map_err(|f| f.prefixed(&path))?;
            kantor_pdf::extract(&mut pdf, &r).map_err(|e| Failure::from(e).prefixed(&path))?;
        }
        total += pdf.page_count();
        docs.push(pdf);
    }
    let count = docs.len();
    let base = docs.remove(0);
    let mut merged = kantor_pdf::merge(base, docs)?;
    save(&mut merged, &output)?;
    Ok(format!(
        "merged {count} file{} → {} ({})",
        if count == 1 { "" } else { "s" },
        output.display(),
        pages(total)
    ))
}

fn split(rest: &[String], usage: &str) -> Outcome {
    let args = Args::parse(
        rest,
        &["output", "password", "ranges", "every"],
        &["force", "help", "each"],
        usage,
    )?;
    args.expect(&["FILE"], usage)?;
    let input = PathBuf::from(&args.positional[0]);
    let modes = [
        args.value("ranges").is_some(),
        args.value("every").is_some(),
        args.flag("each"),
    ];
    if modes.iter().filter(|m| **m).count() != 1 {
        return Err(Failure::usage(format!(
            "choose one of --ranges, --every or --each\n\n{usage}"
        )));
    }
    let (pdf, _) = open(&input, &args)?;
    let total = pdf.page_count();
    let groups = if let Some(r) = args.value("ranges") {
        ranges(r)?.resolve_groups(total).map_err(Failure::from)?
    } else {
        let n = if args.flag("each") {
            1
        } else {
            args.parse_value::<u32>("every", "a whole number")?
                .unwrap_or(1)
        };
        kantor_pdf::every(total, n)?
    };
    let dir = match args.value("output") {
        Some(d) => PathBuf::from(d),
        None => input
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
    };
    if dir.is_file() {
        return Err(Failure::usage(format!(
            "-o for split is a folder, and {} is a file",
            dir.display()
        )));
    }
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "part".into());
    let width = total.to_string().len();
    let names: Vec<PathBuf> = groups
        .iter()
        .map(|g| {
            let label = match (g.first(), g.last()) {
                (Some(a), Some(b)) if g.len() > 1 && g.windows(2).all(|w| w[1] == w[0] + 1) => {
                    format!("{a:0width$}-{b:0width$}")
                }
                (Some(a), _) if g.len() == 1 => format!("{a:0width$}"),
                _ => kantor_pdf::range::compact(g),
            };
            dir.join(format!("{stem}-{label}.pdf"))
        })
        .collect();
    let mut unique = names.clone();
    unique.sort();
    unique.dedup();
    if unique.len() != names.len() {
        return Err(Failure::usage(
            "two parts would have the same name; list each range once",
        ));
    }
    // Check every name before writing any part.
    for name in &names {
        check_overwrite(name, args.flag("force"))?;
    }
    std::fs::create_dir_all(&dir).map_err(|e| Failure {
        message: format!("can't make {}: {e}", dir.display()),
        code: 1,
    })?;
    let mut i = 0;
    kantor_pdf::split(&pdf, &groups, |mut part| {
        save(&mut part.pdf, &names[i]).map_err(|f| kantor_pdf::Error::Invalid(f.message))?;
        i += 1;
        Ok(())
    })?;
    let shown = |p: &PathBuf| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let list = if names.len() <= 3 {
        names.iter().map(shown).collect::<Vec<_>>().join(", ")
    } else {
        format!("{} … {}", shown(&names[0]), shown(&names[names.len() - 1]))
    };
    Ok(format!(
        "{}: split {} into {} files in {} ({list})",
        input.display(),
        pages(total),
        names.len(),
        dir.display()
    ))
}

fn extract(rest: &[String], usage: &str) -> Outcome {
    edit_one(
        rest,
        usage,
        &[],
        &["FILE", "PAGES"],
        "extracted",
        |pdf, args| {
            let total = pdf.page_count();
            kantor_pdf::extract(pdf, &ranges(&args.positional[1])?)?;
            Ok(format!("kept {} of {total}", pages(pdf.page_count())))
        },
    )
}

fn delete(rest: &[String], usage: &str) -> Outcome {
    edit_one(
        rest,
        usage,
        &[],
        &["FILE", "PAGES"],
        "deleted",
        |pdf, args| {
            let n = kantor_pdf::delete(pdf, &ranges(&args.positional[1])?)?;
            Ok(format!("deleted {}", pages(n)))
        },
    )
}

fn reorder(rest: &[String], usage: &str) -> Outcome {
    edit_one(
        rest,
        usage,
        &[],
        &["FILE", "ORDER"],
        "reordered",
        |pdf, args| {
            let order = ranges(&args.positional[1])?;
            kantor_pdf::reorder(pdf, &order)?;
            Ok(format!("reordered to {order}"))
        },
    )
}

fn parse_degrees(s: &str) -> Result<i64, Failure> {
    let d = match s.to_ascii_lowercase().trim() {
        "right" | "cw" => 90,
        "left" | "ccw" => -90,
        other => other.trim_end_matches('°').parse::<i64>().map_err(|_| {
            Failure::usage(format!(
                "\"{s}\" is not an angle (use 90, 180, 270, -90, left or right)"
            ))
        })?,
    };
    if d % 90 != 0 {
        return Err(Failure::usage(format!(
            "rotation must be a multiple of 90 degrees, not {d}"
        )));
    }
    Ok(d)
}

fn rotate(rest: &[String], usage: &str) -> Outcome {
    edit_one(
        rest,
        usage,
        &["pages"],
        &["FILE", "DEGREES"],
        "rotated",
        |pdf, args| {
            let degrees = parse_degrees(&args.positional[1])?;
            let which = match args.value("pages") {
                Some(p) => ranges(p)?,
                None => PageRanges::all(),
            };
            let n = kantor_pdf::rotate(pdf, &which, degrees)?;
            let turn = match degrees.rem_euclid(360) {
                0 => "by 0° (no change)".to_string(),
                90 => "90° clockwise".to_string(),
                180 => "180°".to_string(),
                _ => "90° counter-clockwise".to_string(),
            };
            Ok(format!("rotated {} {turn}", pages(n)))
        },
    )
}

fn info(rest: &[String], usage: &str) -> Outcome {
    let args = Args::parse(rest, &["password"], &["json", "help"], usage)?;
    args.expect(&["FILE"], usage)?;
    let input = PathBuf::from(&args.positional[0]);
    let bytes = kantor_pdf::read(&input)?;
    let info = kantor_pdf::info(&bytes, args.password().as_deref())
        .map_err(|e| Failure::from(e).prefixed(&input))?;
    if args.flag("json") {
        let pages: Vec<_> = info
            .pages
            .iter()
            .map(|p| {
                serde_json::json!({
                    "width": p.width, "height": p.height,
                    "rotation": p.rotation, "paper": p.paper_name(),
                })
            })
            .collect();
        let v = serde_json::json!({
            "file": input.display().to_string(),
            "file_size": info.file_size,
            "version": info.version,
            "encrypted": info.encrypted,
            "readable": info.readable,
            "page_count": info.pages.len(),
            "pages": pages,
            "title": info.title, "author": info.author, "subject": info.subject,
            "creator": info.creator, "producer": info.producer,
        });
        return Ok(format!(
            "{}\n",
            serde_json::to_string_pretty(&v).unwrap_or_default()
        ));
    }
    let mut out = String::new();
    let _ = writeln!(out, "File:       {}", input.display());
    let _ = writeln!(out, "Size:       {}", human_size(info.file_size));
    if !info.version.is_empty() {
        let _ = writeln!(out, "PDF:        {}", info.version);
    }
    let _ = writeln!(
        out,
        "Encrypted:  {}",
        match (info.encrypted, info.readable) {
            (false, _) => "no",
            (true, true) => "yes (opened)",
            (true, false) => "yes (pass --password to read pages and metadata)",
        }
    );
    if !info.readable {
        return Ok(out);
    }
    let _ = writeln!(out, "Pages:      {}", info.pages.len());
    for (key, value) in [
        ("Title", &info.title),
        ("Author", &info.author),
        ("Subject", &info.subject),
        ("Creator", &info.creator),
        ("Producer", &info.producer),
    ] {
        if let Some(v) = value {
            let _ = writeln!(out, "{:<12}{v}", format!("{key}:"));
        }
    }
    // Group runs of pages with the same size: `1-3   A4  595 x 842 pt`.
    let mut i = 0;
    let describe = |p: &kantor_pdf::PageInfo| {
        let mm = |pt: f32| (pt * 25.4 / 72.0).round();
        let mut s = format!(
            "{} x {} pt ({} x {} mm)",
            trim_num(p.width),
            trim_num(p.height),
            mm(p.width),
            mm(p.height)
        );
        if let Some(name) = p.paper_name() {
            s = format!("{name}, {s}");
        }
        if p.rotation != 0 {
            s += &format!(", rotated {}°", p.rotation);
        }
        s
    };
    let _ = writeln!(out, "Page sizes:");
    while i < info.pages.len() {
        let d = describe(&info.pages[i]);
        let mut j = i;
        while j + 1 < info.pages.len() && describe(&info.pages[j + 1]) == d {
            j += 1;
        }
        let label = if i == j {
            format!("{}", i + 1)
        } else {
            format!("{}-{}", i + 1, j + 1)
        };
        let _ = writeln!(out, "  {label:<10}{d}");
        i = j + 1;
    }
    Ok(out)
}

fn trim_num(v: f32) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn images(rest: &[String], usage: &str) -> Outcome {
    let args = Args::parse(
        rest,
        &["output", "page-size", "orientation", "margin"],
        &WRITE_FLAGS,
        usage,
    )?;
    if args.positional.is_empty() {
        return Err(Failure::usage(format!("give the images\n\n{usage}")));
    }
    let output = required_output(&args, usage)?;
    check_overwrite(&output, args.flag("force"))?;
    let opts = ImageOptions {
        page_size: args
            .value("page-size")
            .map(str::parse::<PageSize>)
            .transpose()?
            .unwrap_or(PageSize::Fit),
        orientation: args
            .value("orientation")
            .map(str::parse::<Orientation>)
            .transpose()?
            .unwrap_or(Orientation::Auto),
        margin: args
            .parse_value::<f32>("margin", "a number of points")?
            .unwrap_or(0.0),
    };
    let files = args
        .positional
        .iter()
        .map(|p| Ok((p.clone(), kantor_pdf::read(Path::new(p))?)))
        .collect::<Result<Vec<_>, kantor_pdf::Error>>()?;
    let mut pdf = kantor_pdf::images_to_pdf(&files, &opts)?;
    let size = save(&mut pdf, &output)?;
    Ok(format!(
        "{} image{} → {} ({}, {})",
        files.len(),
        if files.len() == 1 { "" } else { "s" },
        output.display(),
        pages(pdf.page_count()),
        human_size(size)
    ))
}

fn parse_opacity(s: &str) -> Result<f32, Failure> {
    let bad = || Failure::usage(format!("--opacity: \"{s}\" is not 0 to 1 or a percentage"));
    let v = match s.trim().strip_suffix('%') {
        Some(p) => p.trim().parse::<f32>().map_err(|_| bad())? / 100.0,
        None => {
            let v = s.trim().parse::<f32>().map_err(|_| bad())?;
            // `30` reads as 30%.
            if v > 1.0 { v / 100.0 } else { v }
        }
    };
    if !(0.0..=1.0).contains(&v) {
        return Err(bad());
    }
    Ok(v)
}

fn parse_color(s: &str) -> Result<[f32; 3], Failure> {
    let hex = s.trim().trim_start_matches('#');
    let bad = || {
        Failure::usage(format!(
            "--color: \"{s}\" is not a hex colour like 808080 or #c00"
        ))
    };
    let digits: Vec<u8> = hex
        .chars()
        .map(|c| c.to_digit(16).map(|d| d as u8))
        .collect::<Option<_>>()
        .ok_or_else(bad)?;
    let rgb = match digits.len() {
        3 => [digits[0] * 17, digits[1] * 17, digits[2] * 17],
        6 => [
            digits[0] * 16 + digits[1],
            digits[2] * 16 + digits[3],
            digits[4] * 16 + digits[5],
        ],
        _ => return Err(bad()),
    };
    Ok(rgb.map(|c| f32::from(c) / 255.0))
}

fn watermark(rest: &[String], usage: &str) -> Outcome {
    let extra = [
        "text", "size", "opacity", "rotation", "position", "color", "pages",
    ];
    edit_one(
        rest,
        usage,
        &extra,
        &["FILE"],
        "watermarked",
        |pdf, args| {
            let text = args
                .value("text")
                .ok_or_else(|| Failure::usage(format!("give the text with --text\n\n{usage}")))?;
            let mut o = WatermarkOptions {
                text: text.to_string(),
                ..Default::default()
            };
            if let Some(v) = args.parse_value::<f32>("size", "a number of points")? {
                o.font_size = v;
            }
            if let Some(v) = args.value("opacity") {
                o.opacity = parse_opacity(v)?;
            }
            if let Some(v) = args.parse_value::<f32>("rotation", "a number of degrees")? {
                o.rotation = v;
            }
            if let Some(v) = args.value("position") {
                o.position = v.parse::<Position>()?;
            }
            if let Some(v) = args.value("color") {
                o.color = parse_color(v)?;
            }
            let which = match args.value("pages") {
                Some(p) => ranges(p)?,
                None => PageRanges::all(),
            };
            let n = kantor_pdf::add_watermark(pdf, &o, &which)?;
            Ok(format!("watermarked {} with \"{}\"", pages(n), o.text))
        },
    )
}

fn page_numbers(rest: &[String], usage: &str) -> Outcome {
    let extra = ["format", "position", "start", "pages", "size", "margin"];
    edit_one(rest, usage, &extra, &["FILE"], "numbered", |pdf, args| {
        let mut o = PageNumberOptions::default();
        if let Some(v) = args.value("format") {
            o.format = v.to_string();
        }
        if let Some(v) = args.value("position") {
            o.position = v.parse::<Position>()?;
        }
        if let Some(v) = args.parse_value::<i64>("start", "a whole number")? {
            o.start = v;
        }
        if let Some(v) = args.parse_value::<f32>("size", "a number of points")? {
            o.font_size = v;
        }
        if let Some(v) = args.parse_value::<f32>("margin", "a number of points")? {
            o.margin = v;
        }
        let which = match args.value("pages") {
            Some(p) => ranges(p)?,
            None => PageRanges::all(),
        };
        let n = kantor_pdf::add_page_numbers(pdf, &o, &which)?;
        let last = o.start + i64::from(n) - 1;
        Ok(format!("numbered {} ({}–{last})", pages(n), o.start))
    })
}

fn compress(rest: &[String], usage: &str) -> Outcome {
    let args = Args::parse(rest, &WRITE_OPTS, &WRITE_FLAGS, usage)?;
    args.expect(&["FILE"], usage)?;
    let input = PathBuf::from(&args.positional[0]);
    let output = output_path(&args, &input, "compressed");
    check_overwrite(&output, args.flag("force"))?;
    let original = kantor_pdf::read(&input)?;
    let pdf = Pdf::from_bytes(&original, args.password().as_deref())
        .map_err(|e| Failure::from(e).prefixed(&input))?;
    let (bytes, report) = kantor_pdf::compress(pdf, &original)?;
    write_file(&output, &bytes)?;
    let detail = if report.kept_original {
        " (already compact; wrote an unchanged copy)".to_string()
    } else {
        format!(" (−{:.1}%)", report.saved_percent())
    };
    Ok(format!(
        "{}: {} → {}{detail} → {}",
        input.display(),
        human_size(report.before),
        human_size(report.after),
        output.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_short_long_and_inline_options() {
        let a = Args::parse(
            &s(&["in.pdf", "-o", "out.pdf", "--pages=1-2", "-f", "-90"]),
            &["output", "pages"],
            &["force"],
            "",
        )
        .unwrap();
        assert_eq!(a.positional, ["in.pdf", "-90"]);
        assert_eq!(a.value("output"), Some("out.pdf"));
        assert_eq!(a.value("pages"), Some("1-2"));
        assert!(a.flag("force"));
    }

    #[test]
    fn rejects_unknown_and_valueless_options() {
        let e = Args::parse(&s(&["--nope"]), &[], &[], "U").unwrap_err();
        assert!(e.message.starts_with("unknown option --nope"));
        assert_eq!(e.code, 2);
        let e = Args::parse(&s(&["-o"]), &["output"], &[], "U").unwrap_err();
        assert!(e.message.contains("-o needs a value"));
        let a = Args::parse(&s(&["--", "-o"]), &["output"], &[], "").unwrap();
        assert_eq!(a.positional, ["-o"]);
    }

    #[test]
    fn degrees_opacity_colour() {
        assert_eq!(parse_degrees("left").unwrap(), -90);
        assert_eq!(parse_degrees("270").unwrap(), 270);
        assert!(parse_degrees("45").is_err());
        assert_eq!(parse_opacity("30%").unwrap(), 0.3);
        assert_eq!(parse_opacity("0.5").unwrap(), 0.5);
        assert_eq!(parse_opacity("30").unwrap(), 0.3);
        assert!(parse_opacity("150%").is_err());
        assert_eq!(parse_color("#fff").unwrap(), [1.0, 1.0, 1.0]);
        assert_eq!(parse_color("ff0000").unwrap(), [1.0, 0.0, 0.0]);
        assert!(parse_color("red").is_err());
    }

    #[test]
    fn default_output_names() {
        assert_eq!(
            suffixed(Path::new("/a/report.pdf"), "rotated"),
            Path::new("/a/report-rotated.pdf")
        );
        assert_eq!(human_size(1536), "1.5 KB");
    }

    #[test]
    fn unknown_tool_lists_the_tools() {
        let e = run(&s(&["frobnicate"])).unwrap_err();
        assert!(e.message.contains("unknown tool \"frobnicate\""));
        assert!(e.message.contains("page-numbers"));
        assert!(run(&s(&["rotate", "--help"])).unwrap().contains("DEGREES"));
    }
}
