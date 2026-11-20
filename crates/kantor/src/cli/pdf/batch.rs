//! `kantor pdf stamp`, `kantor pdf fill` and `kantor pdf batch`: the same
//! steps on many PDFs, with [`kantor_pdf::batch`] (the PDF window's Batch
//! dialog saves the step files `batch` runs).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use kantor_pdf::batch::{
    self, Anchor, Batch, Existing, Fill, Output, Place, Plugins, Prepared, Stamp, Status, Step,
};

use super::{Args, Failure, Outcome, parse_opacity};

pub const STAMP: &str = "usage: kantor pdf stamp FILE|FOLDER... --image PICTURE [options] [-o OUT]\n\
\n\
Place a picture (a company stamp PNG with transparency, or a JPEG) on the\n\
pages of every PDF given, as a stamp annotation. Digitally signed files keep\n\
their signatures valid (the stamp is added as an incremental update) unless\n\
their certification allows no changes; password-protected files stay protected.\n\
\n\
\x20 --image FILE      the picture (PNG or JPEG)\n\
\x20 --page PAGES      all (default), first, last, or a range like 1-3,5\n\
\x20 --at PLACE        top-left, top-center, top-right, center-left, center,\n\
\x20                   center-right, bottom-left, bottom-center, bottom-right\n\
\x20                   (default), or X,Y: the picture's top-left corner in points\n\
\x20                   from the page's top-left\n\
\x20 --margin PT       distance from the page edges for --at (default 36; DX,DY)\n\
\x20 --width PT        the picture's width in points (default: its own size, at\n\
\x20                   most half the page)\n\
\x20 --opacity A       0 to 1 or a percentage (default 1)\n\
\x20 --signature       mark it as a Fill & Sign signature picture\n\
\x20 -o OUT            the result file (one input), or a folder for the results\n\
\x20                   (default: next to each input, named FILE-stamped.pdf)\n\
\x20 -f, --force       replace results that exist\n\
\x20 --skip-existing   leave results that exist alone and go on\n\
\x20 --password PW     for encrypted inputs (else they're opened without one)";

pub const FILL: &str = "usage: kantor pdf fill FILE|FOLDER... (--data TABLE | --set NAME=VALUE...) [-o OUT]\n\
\x20      kantor pdf fill FILE --list\n\
\n\
Fill in the form fields of every PDF given. The table is a CSV, JSON or\n\
spreadsheet (.xlsx, .xls, .ods; first sheet) with one row per PDF: a `file`\n\
column with the PDF's name (with or without .pdf) and one column per field,\n\
headed with the field's name. A table without a file column gives the same\n\
values to every PDF. Check boxes take yes/no (ya/tidak, 1/0, x); radio\n\
buttons and lists take an option's name.\n\
\n\
\x20 --data FILE       the table of values\n\
\x20 --key COLUMN      the table's file name column (default: file or filename)\n\
\x20 --set NAME=VALUE  a value for every PDF (repeat it; a table row wins)\n\
\x20 --list            show the form's fields, their kinds and options\n\
\x20 -o OUT            the result file (one input), or a folder for the results\n\
\x20                   (default: next to each input, named FILE-filled.pdf)\n\
\x20 -f, --force / --skip-existing / --password PW   as for stamp\n\
\n\
Signed files are filled with an incremental update when their certification\n\
allows form filling.";

pub const BATCH: &str = "usage: kantor pdf batch STEPS.json FILE|FOLDER... [-o FOLDER] [options]\n\
\n\
Run the steps of a batch file (saved by the PDF window's Tools > Batch…\n\
dialog, or written by hand) on every PDF given; a folder means the PDFs in\n\
it. Results go where the batch file says (next to each original with a\n\
suffix, or a folder), or into -o FOLDER.\n\
\n\
\x20 -o FOLDER         the folder for the results\n\
\x20 --suffix S        added to each result's name (default: the batch file's)\n\
\x20 -f, --force       replace results that exist\n\
\x20 --skip-existing   leave results that exist alone and go on\n\
\x20 --password PW     for encrypted inputs\n\
\n\
A batch file:\n\
\x20 {\"version\": 1, \"output\": {\"suffix\": \"-stamped\"}, \"steps\": [\n\
\x20   {\"type\": \"stamp\", \"image\": \"stamp.png\", \"pages\": \"last\",\n\
\x20    \"place\": {\"anchor\": \"bottom-right\", \"dx\": 36, \"dy\": 36}, \"width\": 120},\n\
\x20   {\"type\": \"fill\", \"data\": \"values.csv\"},\n\
\x20   {\"type\": \"page-numbers\", \"format\": \"Page {n} of {total}\"},\n\
\x20   {\"type\": \"watermark\", \"text\": \"COPY\"},\n\
\x20   {\"type\": \"flatten\"}]}\n\
Step types: stamp, signature (a signature picture; image may be\n\
my-stamps/NAME for one of My Stamps), fill, page-numbers, watermark, flatten.\n\
Relative paths are from the batch file's folder. On digitally signed files,\n\
page numbers, watermarks and flatten are skipped (they would invalidate the\n\
signature), as are steps the certification doesn't allow.\n\
\n\
Exit status: 0 when no file failed (skipped files are listed), 1 otherwise.";

/// My Stamps (the PDF window keeps them there).
fn my_stamps() -> Option<PathBuf> {
    kantor_core::settings::config_dir().map(|d| d.join("stamps"))
}

/// The input PDFs: files as given, folders as the PDFs in them.
fn inputs(args: &[String]) -> Result<Vec<PathBuf>, Failure> {
    let mut out = Vec::new();
    for a in args {
        let p = PathBuf::from(a);
        if p.is_dir() {
            let found = batch::pdfs_in(&p);
            if found.is_empty() {
                return Err(Failure {
                    message: format!("{}: no PDFs in this folder", p.display()),
                    code: 1,
                });
            }
            out.extend(found);
        } else {
            out.push(p);
        }
    }
    if out.is_empty() {
        return Err(Failure::usage("give the PDFs (files or folders)"));
    }
    Ok(out)
}

/// Where results go: `-o FILE.pdf` for a single input, `-o FOLDER`, or
/// next to each input with `suffix`.
enum Target {
    File(PathBuf),
    Output(Output),
}

fn target(args: &Args, files: &[PathBuf], suffix: &str) -> Result<Target, Failure> {
    match args.value("output") {
        Some(o) => {
            let p = PathBuf::from(o);
            let is_file =
                !p.is_dir() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
            if is_file {
                if files.len() > 1 {
                    return Err(Failure::usage(
                        "with several PDFs, -o is a folder for the results",
                    ));
                }
                Ok(Target::File(p))
            } else {
                Ok(Target::Output(Output {
                    suffix: args.value("suffix").unwrap_or("").to_owned(),
                    folder: Some(p),
                }))
            }
        }
        None => Ok(Target::Output(Output {
            suffix: args.value("suffix").unwrap_or(suffix).to_owned(),
            folder: None,
        })),
    }
}

/// Run `prepared` on `files` and describe each result.
fn execute(prepared: &Prepared, files: &[PathBuf], target: Target, args: &Args) -> Outcome {
    let password = args.password();
    let steps = &prepared.batch.steps;
    let existing = if args.flag("force") {
        Existing::Replace
    } else {
        Existing::Skip
    };
    let output = match target {
        Target::File(out) => {
            super::check_overwrite(&out, args.flag("force"))?;
            let input = &files[0];
            let bytes = kantor_pdf::read(input)?;
            let name = input
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let run = batch::run_file(prepared, &name, &bytes, password.as_deref());
            let mut r = batch::FileResult {
                input: input.clone(),
                output: None,
                status: Status::Skipped,
                notes: run.notes,
            };
            r.status = match run.outcome {
                batch::Outcome::Done { bytes, incremental } => {
                    super::write_file(&out, &bytes)?;
                    r.output = Some(out);
                    Status::Done { incremental }
                }
                batch::Outcome::Skipped => Status::Skipped,
                batch::Outcome::Failed(e) => Status::Failed(e),
            };
            let failed = matches!(r.status, Status::Failed(_));
            let text = r.describe(steps);
            return if failed {
                Err(Failure {
                    message: text,
                    code: 1,
                })
            } else {
                Ok(text)
            };
        }
        Target::Output(o) => o,
    };
    let clash: Vec<PathBuf> = batch::existing_outputs(&output, files);
    if !clash.is_empty() && !args.flag("force") && !args.flag("skip-existing") {
        let list: Vec<String> = clash
            .iter()
            .take(5)
            .map(|p| p.display().to_string())
            .collect();
        return Err(Failure {
            message: format!(
                "{} result file{} exist{} already ({}{}); use -f to replace or --skip-existing",
                clash.len(),
                if clash.len() == 1 { "" } else { "s" },
                if clash.len() == 1 { "s" } else { "" },
                list.join(", "),
                if clash.len() > 5 { ", …" } else { "" }
            ),
            code: 1,
        });
    }
    let cancel = AtomicBool::new(false);
    let results = batch::run(
        prepared,
        files,
        &output,
        existing,
        password.as_deref(),
        &cancel,
        |_, r| println!("{}", r.describe(steps)),
    );
    let count = |f: fn(&Status) -> bool| results.iter().filter(|r| f(&r.status)).count();
    let done = count(|s| matches!(s, Status::Done { .. }));
    let skipped = count(|s| matches!(s, Status::Skipped));
    let failed = count(|s| matches!(s, Status::Failed(_)));
    let summary = format!("{done} done, {skipped} skipped, {failed} failed");
    if failed > 0 {
        Err(Failure {
            message: summary,
            code: 1,
        })
    } else {
        Ok(summary)
    }
}

fn parse_place(at: Option<&str>, margin: Option<&str>) -> Result<Place, Failure> {
    let (dx, dy) = match margin {
        Some(m) => {
            let v: Vec<f32> = m
                .split(',')
                .map(|p| p.trim().parse::<f32>())
                .collect::<Result<_, _>>()
                .map_err(|_| Failure::usage(format!("--margin: \"{m}\" is not PT or DX,DY")))?;
            match v[..] {
                [d] => (d, d),
                [x, y] => (x, y),
                _ => {
                    return Err(Failure::usage(format!(
                        "--margin: \"{m}\" is not PT or DX,DY"
                    )));
                }
            }
        }
        None => (36.0, 36.0),
    };
    let Some(at) = at else {
        return Ok(Place::Anchor {
            anchor: Anchor::BottomRight,
            dx,
            dy,
        });
    };
    if let Some((x, y)) = at.split_once(',')
        && let (Ok(x), Ok(y)) = (x.trim().parse::<f32>(), y.trim().parse::<f32>())
    {
        return Ok(Place::Point { x, y });
    }
    let anchor = at
        .parse::<Anchor>()
        .map_err(|e| Failure::usage(e.to_string()))?;
    Ok(Place::Anchor { anchor, dx, dy })
}

pub fn stamp(rest: &[String], usage: &str) -> Outcome {
    let args = Args::parse(
        rest,
        &[
            "output", "password", "image", "page", "pages", "at", "margin", "width", "opacity",
            "suffix",
        ],
        &["force", "help", "signature", "skip-existing"],
        usage,
    )?;
    let image = args
        .value("image")
        .ok_or_else(|| Failure::usage(format!("give the picture with --image\n\n{usage}")))?;
    let files = inputs(&args.positional)?;
    let st = Stamp {
        image: std::path::absolute(image)
            .unwrap_or_else(|_| PathBuf::from(image))
            .to_string_lossy()
            .into_owned(),
        pages: args
            .value("page")
            .or(args.value("pages"))
            .unwrap_or("all")
            .to_owned(),
        place: parse_place(args.value("at"), args.value("margin"))?,
        width: args
            .parse_value::<f32>("width", "a number of points")?
            .unwrap_or(0.0),
        opacity: args
            .value("opacity")
            .map(parse_opacity)
            .transpose()?
            .unwrap_or(1.0),
    };
    let step = if args.flag("signature") {
        Step::Signature(st)
    } else {
        Step::Stamp(st)
    };
    let b = Batch {
        steps: vec![step],
        ..Batch::default()
    };
    let prepared = Prepared::new(
        &b,
        Path::new("."),
        my_stamps().as_deref(),
        &Plugins::default(),
    )?;
    let target = target(&args, &files, "-stamped")?;
    execute(&prepared, &files, target, &args)
}

/// The values after each `--set` (the argument parser keeps one per name).
fn sets(rest: &[String]) -> (Vec<String>, Vec<(String, String)>, Option<Failure>) {
    let mut others = Vec::new();
    let mut sets = Vec::new();
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let value = if a == "--set" {
            it.next().cloned()
        } else if let Some(v) = a.strip_prefix("--set=") {
            Some(v.to_owned())
        } else {
            others.push(a.clone());
            continue;
        };
        match value.as_deref().and_then(|v| v.split_once('=')) {
            Some((k, v)) => sets.push((k.trim().to_owned(), v.to_owned())),
            None => {
                return (others, sets, Some(Failure::usage("--set needs NAME=VALUE")));
            }
        }
    }
    (others, sets, None)
}

pub fn fill(rest: &[String], usage: &str) -> Outcome {
    let (rest, sets, bad) = sets(rest);
    if let Some(f) = bad {
        return Err(f);
    }
    let args = Args::parse(
        &rest,
        &["output", "password", "data", "key", "suffix"],
        &["force", "help", "list", "skip-existing"],
        usage,
    )?;
    if args.flag("list") {
        args.expect(&["FILE"], usage)?;
        return list_fields(Path::new(&args.positional[0]), &args);
    }
    if args.value("data").is_none() && sets.is_empty() {
        return Err(Failure::usage(format!(
            "give the values with --data TABLE or --set NAME=VALUE\n\n{usage}"
        )));
    }
    let files = inputs(&args.positional)?;
    let step = Step::Fill(Fill {
        data: args.value("data").map(|d| {
            std::path::absolute(d)
                .unwrap_or_else(|_| PathBuf::from(d))
                .to_string_lossy()
                .into_owned()
        }),
        key: args.value("key").map(str::to_owned),
        values: sets.into_iter().collect(),
    });
    let b = Batch {
        steps: vec![step],
        ..Batch::default()
    };
    let prepared = Prepared::new(&b, Path::new("."), None, &Plugins::default())?;
    let target = target(&args, &files, "-filled")?;
    execute(&prepared, &files, target, &args)
}

fn list_fields(path: &Path, args: &Args) -> Outcome {
    let (pdf, _) = super::open(path, args)?;
    let Some(form) = kantor_pdf::form::read_form(&pdf) else {
        return Ok(format!("{}: no form fields", path.display()));
    };
    use kantor_pdf::form::{FieldKind, FieldValue};
    let mut out = format!("{}: {} fields\n", path.display(), form.fields.len());
    for f in &form.fields {
        let kind = match &f.kind {
            FieldKind::Text { .. } => "text",
            FieldKind::Check => "check box",
            FieldKind::Radio => "radio buttons",
            FieldKind::Combo { .. } => "drop-down",
            FieldKind::List { .. } => "list",
            FieldKind::Push => "button",
            FieldKind::Signature => "signature",
        };
        let value = match &f.value {
            FieldValue::Text(t) if !t.is_empty() => format!(" = {t:?}"),
            FieldValue::State(s) => format!(" = {s}"),
            FieldValue::Choice(c) if !c.is_empty() => format!(" = {}", c.join("; ")),
            _ => String::new(),
        };
        let mut options: Vec<String> = f.options.iter().map(|(e, _)| e.clone()).collect();
        if matches!(f.kind, FieldKind::Radio | FieldKind::Check) {
            options = f
                .widgets
                .iter()
                .filter_map(|w| w.on_state.clone())
                .collect();
        }
        let options = if options.is_empty() {
            String::new()
        } else {
            format!(" [{}]", options.join(", "))
        };
        out.push_str(&format!("  {}  ({kind}){options}{value}\n", f.name));
    }
    if form.xfa_only() {
        out.push_str("  (an XFA-only form: Kantor can't fill it)\n");
    }
    Ok(out)
}

pub fn run_batch(rest: &[String], usage: &str) -> Outcome {
    let args = Args::parse(
        rest,
        &["output", "password", "suffix"],
        &["force", "help", "skip-existing"],
        usage,
    )?;
    let Some((steps_file, files)) = args.positional.split_first() else {
        return Err(Failure::usage(format!(
            "give the batch file and the PDFs\n\n{usage}"
        )));
    };
    let steps_path = PathBuf::from(steps_file);
    let text = std::fs::read_to_string(&steps_path).map_err(|e| Failure {
        message: format!("{}: {e}", steps_path.display()),
        code: 1,
    })?;
    let b = Batch::from_json(&text).map_err(|e| Failure::from(e).prefixed(&steps_path))?;
    let base = steps_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let files = inputs(files)?;
    let prepared = Prepared::new(&b, base, my_stamps().as_deref(), &Plugins::default())
        .map_err(|e| Failure::from(e).prefixed(&steps_path))?;
    let target = match args.value("output") {
        Some(o) => Target::Output(Output {
            suffix: args
                .value("suffix")
                .map(str::to_owned)
                .unwrap_or_else(|| b.output.suffix.clone()),
            folder: Some(PathBuf::from(o)),
        }),
        None => {
            let mut o = b.output.clone();
            if let Some(s) = args.value("suffix") {
                o.suffix = s.to_owned();
            }
            if o.folder.is_none() && o.suffix.is_empty() {
                return Err(Failure::usage(
                    "the results would replace the originals: give -o FOLDER or --suffix",
                ));
            }
            Target::Output(o)
        }
    };
    execute(&prepared, &files, target, &args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kantor_pdf::testing::{sample_form_pdf, sample_pdf, sample_png};

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn p(path: &Path) -> String {
        path.display().to_string()
    }

    #[test]
    fn places() {
        assert_eq!(
            parse_place(Some("top-left"), Some("10,20")).unwrap(),
            Place::Anchor {
                anchor: Anchor::TopLeft,
                dx: 10.0,
                dy: 20.0
            }
        );
        assert_eq!(
            parse_place(Some("100, 50"), None).unwrap(),
            Place::Point { x: 100.0, y: 50.0 }
        );
        assert!(parse_place(Some("upstairs"), None).is_err());
    }

    #[test]
    fn stamp_fill_and_batch_files() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("a.pdf"), sample_pdf(2)).unwrap();
        std::fs::write(d.join("b.pdf"), sample_pdf(3)).unwrap();
        std::fs::write(d.join("logo.png"), sample_png(40, 40, true)).unwrap();
        // One file to a named result.
        let out = d.join("one.pdf");
        let r = stamp(
            &s(&[
                &p(&d.join("a.pdf")),
                "--image",
                &p(&d.join("logo.png")),
                "--page",
                "last",
                "-o",
                &p(&out),
            ]),
            STAMP,
        )
        .unwrap();
        assert!(r.contains("done →"), "{r}");
        assert!(out.exists());
        // A folder of PDFs into another folder; again refuses to replace.
        let res = d.join("res");
        std::fs::create_dir(&res).unwrap();
        let src = d.join("src");
        std::fs::create_dir(&src).unwrap();
        std::fs::copy(d.join("a.pdf"), src.join("a.pdf")).unwrap();
        std::fs::copy(d.join("b.pdf"), src.join("b.pdf")).unwrap();
        let args = s(&[
            &p(&src),
            "--image",
            &p(&d.join("logo.png")),
            "--at",
            "top-left",
            "-o",
            &p(&res),
        ]);
        assert_eq!(stamp(&args, STAMP).unwrap(), "2 done, 0 skipped, 0 failed");
        assert!(res.join("b.pdf").exists());
        let e = stamp(&args, STAMP).unwrap_err();
        assert!(e.message.contains("exist already"), "{}", e.message);
        let mut skip = args.clone();
        skip.push("--skip-existing".into());
        assert_eq!(stamp(&skip, STAMP).unwrap(), "0 done, 2 skipped, 0 failed");
        // Fill with --set; --list shows the fields.
        std::fs::write(d.join("form.pdf"), sample_form_pdf()).unwrap();
        let r = fill(
            &s(&[
                &p(&d.join("form.pdf")),
                "--set",
                "name=Budi",
                "--set=agree=yes",
            ]),
            FILL,
        )
        .unwrap();
        assert!(r.contains("1 done"), "{r}");
        let list = fill(&s(&[&p(&d.join("form-filled.pdf")), "--list"]), FILL).unwrap();
        assert!(list.contains("name  (text) = \"Budi\""), "{list}");
        assert!(
            list.contains("size  (radio buttons) [S, M, L] = M"),
            "{list}"
        );
        // A batch file with relative paths.
        std::fs::write(
            d.join("steps.json"),
            r#"{"steps": [{"type": "stamp", "image": "logo.png", "pages": "first"},
                          {"type": "page-numbers"}], "output": {"suffix": "-b"}}"#,
        )
        .unwrap();
        let r = run_batch(
            &s(&[&p(&d.join("steps.json")), &p(&d.join("b.pdf"))]),
            BATCH,
        )
        .unwrap();
        assert!(r.starts_with("1 done"), "{r}");
        assert!(d.join("b-b.pdf").exists());
        let e = run_batch(&s(&[&p(&d.join("steps.json"))]), BATCH).unwrap_err();
        assert_eq!(e.code, 2);
    }

    #[test]
    fn batch_stamping_keeps_signatures_valid_for_the_checker() {
        let fixtures = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../pdf_engine/tests/fixtures/signatures"
        );
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("logo.png"), sample_png(40, 40, true)).unwrap();
        let out = d.join("out");
        let r = stamp(
            &s(&[
                &format!("{fixtures}/valid.pdf"),
                &format!("{fixtures}/unsigned.pdf"),
                "--image",
                &p(&d.join("logo.png")),
                "-o",
                &p(&out),
            ]),
            STAMP,
        )
        .unwrap();
        assert_eq!(r, "2 done, 0 skipped, 0 failed");
        let root = format!("{fixtures}/test-root.pem");
        let (text, code) = super::super::verify(&s(&[
            &p(&out.join("valid.pdf")),
            "--trust",
            &root,
            "--no-system-roots",
        ]));
        assert_eq!(code, 0, "{text}");
        assert!(text.contains("Stamp added on page 1"), "{text}");
        let original = std::fs::read(format!("{fixtures}/valid.pdf")).unwrap();
        assert!(
            std::fs::read(out.join("valid.pdf"))
                .unwrap()
                .starts_with(&original)
        );
    }
}
