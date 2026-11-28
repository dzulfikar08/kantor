//! `kantor docs`: write and revise Word documents without a window.
//!
//! ```text
//! kantor docs build doc.json -o doc.docx
//! kantor docs list doc.docx [--text]
//! kantor docs check doc.docx [--json]
//! kantor docs edit doc.docx [-o out.docx] TOOL ARGS | --ops ops.json
//! kantor docs schema > docs-json.schema.json
//! ```
//!
//! The JSON is described in docs/docs-json.md.

use std::ffi::OsString;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

use crate::agent::docs_author;

pub const USAGE: &str = "usage: kantor docs build DOC.json -o OUT.docx [-f] [--check [--json]]\n\
    \x20      kantor docs list FILE.docx [--text [--start N] [--limit N]]\n\
    \x20      kantor docs check FILE.docx [--json]\n\
    \x20      kantor docs edit FILE.docx [-o OUT.docx] [-f] [--track AUTHOR] TOOL ARGS\n\
    \x20      kantor docs edit FILE.docx [-o OUT.docx] [-f] [--track AUTHOR] --ops OPS.json\n\
    \x20      kantor docs schema\n\
    \x20      kantor docs merge TEMPLATE.docx [DATA] -o DIR [options]\n\
    \n\
    Write and revise Word documents without opening a window, with JSON\n\
    (docs/docs-json.md).\n\
    \n\
    \x20 build    write a .docx from a document description: a preset look\n\
    \x20          (report, proposal, letter) with your accent colour and\n\
    \x20          fonts, real styles, numbered headings, cover, table of\n\
    \x20          contents, lists, tables, pictures, charts, callouts,\n\
    \x20          captions, cross-references, footnotes, sections (landscape\n\
    \x20          pages), headers and footers with page numbers. Fields (the\n\
    \x20          table of contents, page references) are updated from\n\
    \x20          Kantor's layout. DOC.json may be - (standard input);\n\
    \x20          relative picture paths start from its folder. The same JSON\n\
    \x20          always gives the same file. --check then checks it.\n\
    \x20 list     the document's outline as JSON: sections, the headings\n\
    \x20          tree with paragraph ids, styles used and direct formatting,\n\
    \x20          tables, pictures, charts, fields, comments, tracked changes,\n\
    \x20          footnotes, pages. --text prints the paragraphs (id, style,\n\
    \x20          text), --start/--limit page through long documents.\n\
    \x20 check    lay the document out as Kantor draws it (PDF, PNG) and report\n\
    \x20          problems with paragraph ids and pages: empty or skipped\n\
    \x20          heading levels, hand-made headings, direct formatting, broken\n\
    \x20          references, stale table of contents, caption numbers,\n\
    \x20          pictures without alt text, tables without a header row, text\n\
    \x20          past cells or the page, headings alone at a page bottom,\n\
    \x20          mixed fonts, placeholder text, double spaces, open comments\n\
    \x20          and changes, low contrast, fonts not installed. Exit status 1\n\
    \x20          when there are errors.\n\
    \x20 edit     run tools on a file and save it (in place unless -o): the\n\
    \x20          MCP docs tools (get_paragraphs, insert_paragraphs,\n\
    \x20          replace_paragraph, delete_paragraphs, find_replace) and\n\
    \x20          insert_blocks, replace_blocks, delete_blocks, move_section,\n\
    \x20          set_style, restyle, format, add_comment, resolve_comment,\n\
    \x20          accept_changes, reject_changes, update_fields, set_header,\n\
    \x20          set_footer, page_setup (docs/docs-json.md). ARGS is JSON or\n\
    \x20          @FILE; --ops runs [{\"tool\", \"args\"}, …]. --track AUTHOR\n\
    \x20          makes the edits tracked changes by AUTHOR.\n\
    \x20 schema   print the JSON Schema of documents and blocks\n\
    \x20 merge    one document per record of a workbook or CSV file (kantor\n\
    \x20          docs merge --help)\n\
    \n\
    See the pages: kantor convert FILE.docx -o DIR --to png [--contact-sheet].\n\
    \n\
    Options:\n\
    \x20 -o, --output FILE   the .docx to write\n\
    \x20 -f, --force         overwrite an existing output file\n\
    \x20     --check         build: check the document it wrote\n\
    \x20     --json          check: findings as JSON\n\
    \x20     --text          list: the paragraphs\n\
    \x20     --track AUTHOR  edit: tracked changes by AUTHOR\n\
    \x20 -h, --help          show this help\n\
    \n\
    Exit status: 0 on success, 1 when the work failed, 2 for usage errors.";

/// A usage error (exit status 2).
struct Usage(String);

impl From<&str> for Usage {
    fn from(s: &str) -> Self {
        Usage(s.to_owned())
    }
}

fn is_docx(p: &Path) -> bool {
    p.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("docx"))
}

/// Read JSON from a file, `-` (standard input), or `@FILE`.
pub fn read_json(arg: &str) -> Result<Value> {
    let text = if arg == "-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .context("reading standard input")?;
        s
    } else {
        let path = arg.strip_prefix('@').unwrap_or(arg);
        std::fs::read_to_string(path).with_context(|| format!("can't read {path}"))?
    };
    serde_json::from_str(&text).with_context(|| format!("{arg} is not valid JSON"))
}

#[derive(Default)]
struct Opts {
    output: Option<PathBuf>,
    force: bool,
    ops: Option<String>,
    check: bool,
    json: bool,
    text: bool,
    start: Option<usize>,
    limit: Option<usize>,
    track: Option<String>,
    rest: Vec<String>,
}

fn parse(args: &[OsString]) -> Result<Opts, Usage> {
    let mut o = Opts::default();
    let mut it = args.iter();
    let num = |v: Option<&OsString>, what: &str| -> Result<usize, Usage> {
        v.and_then(|v| v.to_str())
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| Usage(format!("{what} needs a number")))
    };
    while let Some(a) = it.next() {
        let a = a
            .to_str()
            .ok_or_else(|| Usage("arguments must be valid UTF-8".into()))?;
        match a {
            "-o" | "--output" => {
                let v = it.next().ok_or(Usage::from("-o needs a file"))?;
                o.output = Some(PathBuf::from(v));
            }
            "-f" | "--force" => o.force = true,
            "--check" => o.check = true,
            "--json" => o.json = true,
            "--text" => o.text = true,
            // kantor docs bench (see cli/docs_bench.rs).
            "--verify" => {}
            "--start" => o.start = Some(num(it.next(), "--start")?),
            "--limit" => o.limit = Some(num(it.next(), "--limit")?),
            "--track" => {
                let v = it
                    .next()
                    .ok_or(Usage::from("--track needs an author name"))?;
                o.track = Some(v.to_string_lossy().into_owned());
            }
            "--ops" => {
                let v = it.next().ok_or(Usage::from("--ops needs a file"))?;
                o.ops = Some(v.to_string_lossy().into_owned());
            }
            "-" => o.rest.push(a.to_owned()),
            s if s.starts_with('-') && !s.starts_with("-{") => {
                return Err(Usage(format!("unknown option {s}")));
            }
            s => o.rest.push(s.to_owned()),
        }
    }
    Ok(o)
}

/// Run `kantor docs …`; returns the exit status.
pub fn main(args: &[OsString]) -> i32 {
    let Some(cmd) = args.first().and_then(|a| a.to_str()) else {
        eprintln!("{USAGE}");
        return 2;
    };
    if cmd == "merge" {
        return super::merge::main(super::merge::App::Docs, &args[1..]);
    }
    if matches!(cmd, "-h" | "--help" | "help") || args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return 0;
    }
    let opts = match parse(&args[1..]) {
        Ok(o) => o,
        Err(Usage(msg)) => {
            eprintln!("kantor docs: {msg}\n\n{USAGE}");
            return 2;
        }
    };
    if cmd == "bench" {
        let [file] = opts.rest.as_slice() else {
            eprintln!("kantor docs bench: bench takes one FILE.docx");
            return 2;
        };
        let verify = args.iter().any(|a| a == "--verify");
        return match super::docs_bench::run(Path::new(file), opts.limit.unwrap_or(30), verify) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("kantor docs bench: {e:#}");
                1
            }
        };
    }
    if cmd == "check" {
        let [file] = opts.rest.as_slice() else {
            eprintln!("kantor docs check: check takes one FILE.docx\n\n{USAGE}");
            return 2;
        };
        return check(Path::new(file), opts.json);
    }
    let result = match cmd {
        "build" => build(&opts),
        "list" => list(&opts),
        "edit" => edit(&opts),
        "schema" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&crate::agent::docs_schema::document())
                    .unwrap_or_default()
            );
            Ok(Ok(()))
        }
        other => Ok(Err(Usage(format!("unknown command {other}")))),
    };
    match result {
        Ok(Ok(())) if cmd == "build" && opts.check => match &opts.output {
            Some(out) => check(out, opts.json),
            None => 0,
        },
        Ok(Ok(())) => 0,
        Ok(Err(Usage(msg))) => {
            eprintln!("kantor docs {cmd}: {msg}\n\n{USAGE}");
            2
        }
        Err(e) => {
            eprintln!("kantor docs {cmd}: {e:#}");
            1
        }
    }
}

type Outcome = Result<Result<(), Usage>>;

fn build(o: &Opts) -> Outcome {
    let [doc] = o.rest.as_slice() else {
        return Ok(Err("build takes one DOC.json".into()));
    };
    let Some(out) = &o.output else {
        return Ok(Err("build needs -o OUT.docx".into()));
    };
    if !is_docx(out) {
        return Ok(Err("the output must be a .docx file".into()));
    }
    if out.exists() && !o.force {
        bail!("{} exists (use -f to overwrite)", out.display());
    }
    let json = read_json(doc)?;
    let base = if doc == "-" {
        std::env::current_dir().ok()
    } else {
        Path::new(doc)
            .canonicalize()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
    };
    let built = docs_author::build(&json, base.as_deref())?;
    super::slides::write_file(out, &built.bytes)?;
    let line = format!(
        "{doc} -> {} ({} page{})",
        out.display(),
        built.pages,
        if built.pages == 1 { "" } else { "s" }
    );
    let log = |s: &str| {
        if o.check && o.json {
            eprintln!("{s}");
        } else {
            println!("{s}");
        }
    };
    log(&line);
    for w in &built.warnings {
        log(&format!("note: {w}"));
    }
    Ok(Ok(()))
}

fn list(o: &Opts) -> Outcome {
    let [file] = o.rest.as_slice() else {
        return Ok(Err("list takes one FILE.docx".into()));
    };
    let doc = crate::agent::docs_edit::open(Path::new(file))?;
    let out = if o.text {
        crate::agent::docs_list::text(&doc, o.start.unwrap_or(0), o.limit.unwrap_or(500))
    } else {
        crate::agent::docs_list::outline(&doc)
    };
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(Ok(()))
}

fn check(file: &Path, json: bool) -> i32 {
    let doc = match crate::agent::docs_edit::open(file) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("kantor docs check: {}: {e:#}", file.display());
            return 1;
        }
    };
    let report = crate::agent::docs_check::check(&doc);
    if json {
        let mut v = report.json();
        v["file"] = serde_json::json!(file.display().to_string());
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
    } else {
        print!("{}", report.text(&file.display().to_string()));
    }
    i32::from(report.has_errors())
}

fn edit(o: &Opts) -> Outcome {
    let (file, ops) = match (o.rest.as_slice(), &o.ops) {
        ([file], Some(ops)) => {
            let list = read_json(ops)?;
            (file, crate::agent::docs_edit::ops_of(&list)?)
        }
        ([file, tool, args], None) => {
            let args = if args.starts_with('@') || args == "-" {
                read_json(args)?
            } else {
                serde_json::from_str(args).context("ARGS is not valid JSON")?
            };
            (file, vec![(tool.clone(), args)])
        }
        ([file, tool], None) => (file, vec![(tool.clone(), serde_json::json!({}))]),
        _ => {
            return Ok(Err(
                "edit takes FILE.docx and TOOL ARGS, or --ops OPS.json".into()
            ));
        }
    };
    let out = o.output.clone().unwrap_or_else(|| PathBuf::from(file));
    if !is_docx(&out) {
        return Ok(Err("the output must be a .docx file (give -o)".into()));
    }
    if o.output.is_some() && out.exists() && !o.force {
        bail!("{} exists (use -f to overwrite)", out.display());
    }
    let base = std::env::current_dir().ok();
    let mut session =
        crate::agent::docs_edit::Session::open(Path::new(file), base, o.track.clone())?;
    if o.output.is_none() && session.read_only() {
        bail!("{file} can't be saved in place (it isn't a .docx); give -o NEW.docx");
    }
    for (n, (tool, args)) in ops.iter().enumerate() {
        let result = session
            .run(tool, args)
            .with_context(|| format!("operation {} ({tool})", n + 1))?;
        println!("{result}");
    }
    let bytes = session.save()?;
    super::slides::write_file(&out, &bytes)?;
    println!("saved {}", out.display());
    Ok(Ok(()))
}
