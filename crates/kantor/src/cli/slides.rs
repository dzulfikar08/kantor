//! `kantor slides`: build and edit presentations without a window.
//!
//! ```text
//! kantor slides build deck.json -o deck.pptx
//! kantor slides list deck.pptx
//! kantor slides edit deck.pptx add_shape '{"slide": 2, "type": "shape", ...}'
//! kantor slides edit deck.pptx -o out.pptx --ops ops.json
//! kantor slides schema > slides-deck.schema.json
//! ```
//!
//! The JSON is the MCP slides tools' (see `agent/slides_author.rs` and
//! docs/slides-json.md).

use std::ffi::OsString;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};
use slides_engine::Presentation;

use crate::agent::{slides, slides_author, slides_schema};

pub const USAGE: &str = "usage: kantor slides build DECK.json -o OUT.pptx [-f]\n\
    \x20      kantor slides list FILE.pptx\n\
    \x20      kantor slides edit FILE.pptx [-o OUT.pptx] [-f] TOOL ARGS\n\
    \x20      kantor slides edit FILE.pptx [-o OUT.pptx] [-f] --ops OPS.json\n\
    \x20      kantor slides schema\n\
    \x20      kantor slides merge TEMPLATE.pptx DATA -o DIR [options]\n\
    \n\
    Build and change presentations without opening a window, with the JSON\n\
    of Kantor's MCP slides tools (docs/slides-json.md).\n\
    \n\
    \x20 build    write a .pptx from a deck description: slide size, theme\n\
    \x20          fonts and colours, then each slide's layout, title, bullets,\n\
    \x20          background, elements (shapes, text boxes, lines, pictures,\n\
    \x20          tables, charts) and notes. DECK.json may be - (standard input);\n\
    \x20          relative picture paths start from its folder. The same deck\n\
    \x20          always gives the same file.\n\
    \x20 list     print the slides and their shapes (id, name, kind, geometry,\n\
    \x20          position and size in EMU, text; charts: type, series) as JSON\n\
    \x20 edit     run slides tools on a file and save it (in place unless -o).\n\
    \x20          TOOL is a tool name with or without slides_ (add_shape,\n\
    \x20          add_picture, add_table, add_chart, add_elements, add_slide,\n\
    \x20          set_background, update_shape, delete_shape, order_shape,\n\
    \x20          group_shapes, set_shape_text, set_notes, delete_slide);\n\
    \x20          ARGS is its JSON arguments, or @FILE. --ops runs a list of\n\
    \x20          {\"tool\": ..., \"args\": {...}}. Relative picture paths start\n\
    \x20          from the current folder.\n\
    \x20 schema   print the JSON Schema of decks and elements\n\
    \x20 merge    one presentation per record of a workbook or CSV file,\n\
    \x20          {{Field}} in the slides' text filled in (kantor slides\n\
    \x20          merge --help)\n\
    \n\
    Options:\n\
    \x20 -o, --output FILE   the .pptx to write\n\
    \x20 -f, --force         overwrite an existing output file\n\
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

/// Write `bytes` to `path` through a temporary file beside it, so a
/// failed write leaves any old file as it was.
pub fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.kantor-{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes).with_context(|| format!("can't write {}", path.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        anyhow::anyhow!("can't write {}: {e}", path.display())
    })
}

fn is_pptx(p: &Path) -> bool {
    p.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pptx"))
}

/// Read JSON from a file, `-` (standard input), or `@FILE`.
fn read_json(arg: &str) -> Result<Value> {
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

/// Options shared by the subcommands.
#[derive(Default)]
struct Opts {
    output: Option<PathBuf>,
    force: bool,
    ops: Option<String>,
    rest: Vec<String>,
}

fn parse(args: &[OsString]) -> Result<Opts, Usage> {
    let mut o = Opts::default();
    let mut it = args.iter();
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

/// Run `kantor slides …`; returns the exit status.
pub fn main(args: &[OsString]) -> i32 {
    let Some(cmd) = args.first().and_then(|a| a.to_str()) else {
        eprintln!("{USAGE}");
        return 2;
    };
    if cmd == "merge" {
        return super::merge::main(super::merge::App::Slides, &args[1..]);
    }
    if matches!(cmd, "-h" | "--help" | "help") || args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return 0;
    }
    let opts = match parse(&args[1..]) {
        Ok(o) => o,
        Err(Usage(msg)) => {
            eprintln!("kantor slides: {msg}\n\n{USAGE}");
            return 2;
        }
    };
    let result = match cmd {
        "build" => build(&opts),
        "list" => list(&opts),
        "edit" => edit(&opts),
        "schema" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&slides_schema::deck()).unwrap_or_default()
            );
            Ok(Ok(()))
        }
        other => Ok(Err(Usage(format!("unknown command {other}")))),
    };
    match result {
        Ok(Ok(())) => 0,
        Ok(Err(Usage(msg))) => {
            eprintln!("kantor slides {cmd}: {msg}\n\n{USAGE}");
            2
        }
        Err(e) => {
            eprintln!("kantor slides {cmd}: {e:#}");
            1
        }
    }
}

type Outcome = Result<Result<(), Usage>>;

fn build(o: &Opts) -> Outcome {
    let [deck] = o.rest.as_slice() else {
        return Ok(Err("build takes one DECK.json".into()));
    };
    let Some(out) = &o.output else {
        return Ok(Err("build needs -o OUT.pptx".into()));
    };
    if !is_pptx(out) {
        return Ok(Err("the output must be a .pptx file".into()));
    }
    if out.exists() && !o.force {
        bail!("{} exists (use -f to overwrite)", out.display());
    }
    let json = read_json(deck)?;
    let base = if deck == "-" {
        std::env::current_dir().ok()
    } else {
        Path::new(deck)
            .canonicalize()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
    };
    let p = slides_author::build(&json, base.as_deref())?;
    let bytes = p.to_bytes()?;
    write_file(out, &bytes)?;
    println!("{deck} -> {} ({} slides)", out.display(), p.len());
    Ok(Ok(()))
}

fn open(path: &str) -> Result<Presentation> {
    let path = Path::new(path);
    if !path.is_file() {
        bail!("{} is not a file", path.display());
    }
    Presentation::open(path)
}

fn list(o: &Opts) -> Outcome {
    let [file] = o.rest.as_slice() else {
        return Ok(Err("list takes one FILE.pptx".into()));
    };
    let p = open(file)?;
    let mut out = slides::list(&p, 0);
    if let Some(m) = out.as_object_mut() {
        m.remove("current_slide");
        m.insert("size".into(), json!({"w": p.size.cx, "h": p.size.cy}));
    }
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(Ok(()))
}

/// `add_shape` → `slides_add_shape`.
fn tool_name(t: &str) -> String {
    if t.starts_with("slides_") {
        t.to_owned()
    } else {
        format!("slides_{t}")
    }
}

fn edit(o: &Opts) -> Outcome {
    let (file, ops) = match (o.rest.as_slice(), &o.ops) {
        ([file], Some(ops)) => {
            let list = read_json(ops)?;
            let Some(items) = list.as_array() else {
                bail!("--ops must hold an array of {{\"tool\", \"args\"}}");
            };
            let ops = items
                .iter()
                .enumerate()
                .map(|(i, op)| {
                    let tool = op
                        .get("tool")
                        .and_then(Value::as_str)
                        .with_context(|| format!("operation {} has no `tool`", i + 1))?;
                    Ok((
                        tool_name(tool),
                        op.get("args").cloned().unwrap_or(json!({})),
                    ))
                })
                .collect::<Result<Vec<_>>>()?;
            (file, ops)
        }
        ([file, tool, args], None) => {
            let args = if args.starts_with('@') || args == "-" {
                read_json(args)?
            } else {
                serde_json::from_str(args).context("ARGS is not valid JSON")?
            };
            (file, vec![(tool_name(tool), args)])
        }
        ([file, tool], None) => (file, vec![(tool_name(tool), json!({}))]),
        _ => {
            return Ok(Err(
                "edit takes FILE.pptx and TOOL ARGS, or --ops OPS.json".into()
            ));
        }
    };
    for (tool, _) in &ops {
        if !slides_author::EDIT_TOOLS.contains(&tool.as_str()) {
            return Ok(Err(Usage(format!(
                "{tool} is not a slides editing tool (one of {})",
                slides_author::EDIT_TOOLS
                    .iter()
                    .map(|t| t.trim_start_matches("slides_"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))));
        }
    }
    let out = o.output.clone().unwrap_or_else(|| PathBuf::from(file));
    if !is_pptx(&out) {
        return Ok(Err("the output must be a .pptx file (give -o)".into()));
    }
    let in_place = o.output.is_none();
    if !in_place && out.exists() && !o.force {
        bail!("{} exists (use -f to overwrite)", out.display());
    }
    let mut p = open(file)?;
    if in_place && p.needs_save_as() {
        bail!(
            "{file} can't be saved in place ({}); give -o NEW.pptx",
            if p.is_imported() {
                "it is not a .pptx".to_owned()
            } else {
                p.unsupported_content().join(", ")
            }
        );
    }
    let base = std::env::current_dir().ok();
    for (n, (tool, args)) in ops.iter().enumerate() {
        let args = if args.is_null() {
            json!({})
        } else {
            args.clone()
        };
        let (what, result, _) = slides_author::run_tool(&mut p, tool, &args, base.as_deref())
            .with_context(|| format!("operation {} ({tool})", n + 1))?;
        let mut line = result;
        if let Some(m) = line.as_object_mut() {
            m.insert("done".into(), json!(what));
        }
        println!("{line}");
    }
    write_file(&out, &p.to_bytes()?)?;
    println!("saved {}", out.display());
    Ok(Ok(()))
}
