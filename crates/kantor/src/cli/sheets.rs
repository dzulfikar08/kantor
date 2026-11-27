//! `kantor sheets`: build, list, edit and check workbooks without a window.
//!
//! ```text
//! kantor sheets build book.json -o book.xlsx
//! kantor sheets list book.xlsx [--sheet Data] [--range A1:H200] [--formulas] [--max-rows N]
//! kantor sheets edit book.xlsx to_numbers '{"range": "Data!B2:B500"}'
//! kantor sheets edit book.xlsx -o out.xlsx --ops ops.json
//! kantor sheets check book.xlsx [--json] [--sheet Data]
//! kantor sheets macros book.xlsm [--module Module1] [--json]
//! kantor sheets macros run book.xlsm --macro Macro1 [--out out.xlsm] [--input VALUE]...
//! kantor sheets schema > sheets-json.schema.json
//! ```
//!
//! The JSON is the MCP tool `sheets_edit`'s (see `agent/sheets_author.rs`
//! and docs/sheets-json.md).

use std::ffi::OsString;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};
use sheets_engine::Workbook;

use crate::agent::{sheets_author, sheets_check, sheets_list, sheets_macros, sheets_schema};

pub const USAGE: &str = "usage: kantor sheets build BOOK.json -o OUT.xlsx [-f] [--check [--json]]\n\
    \x20      kantor sheets list FILE [--sheet S] [--range R] [--formulas] [--max-rows N]\n\
    \x20      kantor sheets check FILE [--json] [--sheet S]\n\
    \x20      kantor sheets macros FILE [--module NAME] [--json]\n\
    \x20      kantor sheets macros run FILE --macro NAME [--out OUT.xlsm] [--input V]...\n\
    \x20                [--events] [--allow-path PATH]...\n\
    \x20      kantor sheets edit FILE [-o OUT.xlsx] [-f] TOOL ARGS\n\
    \x20      kantor sheets edit FILE [-o OUT.xlsx] [-f] --ops OPS.json\n\
    \x20      kantor sheets schema\n\
    \x20      kantor sheets merge TEMPLATE.xlsx DATA -o DIR [options]\n\
    \n\
    Build, read, change and check workbooks without opening a window, with\n\
    JSON (docs/sheets-json.md). Formulas stay formulas and are calculated;\n\
    the files open in Excel.\n\
    \n\
    \x20 build    write a .xlsx from a workbook description: sheets with rows or\n\
    \x20          columns of values and formulas, cells, number formats and\n\
    \x20          styles, column widths, frozen panes, tables, charts placed by\n\
    \x20          cell, conditional formats, validation, pivot tables, names.\n\
    \x20          BOOK.json may be - (standard input). The same JSON always\n\
    \x20          gives the same file. --check then checks it.\n\
    \x20 list     print the workbook as JSON: each sheet's used range, frozen\n\
    \x20          panes, merged cells, tables, charts and their series ranges,\n\
    \x20          conditional formats, validation, pivot tables, a summary of\n\
    \x20          each column, and a page of cells (values, formulas, number\n\
    \x20          formats; 30 rows unless --max-rows, a --range up to 500 rows;\n\
    \x20          \"next\" says where to continue).\n\
    \x20 check    audit the workbook: formula errors, circular references,\n\
    \x20          numbers and dates stored as text, inconsistent formulas,\n\
    \x20          numbers typed into formulas, typed totals, SUMs that miss\n\
    \x20          rows, formulas using empty cells, mixed formats and units,\n\
    \x20          duplicate rows, extra spaces, merged cells in data, volatile\n\
    \x20          functions, external links, broken names, hidden data, text\n\
    \x20          cut off or shown as #### (measured as the PDF draws it),\n\
    \x20          content outside the print area or split by page breaks,\n\
    \x20          unsorted bar charts. Each finding has a sheet and range.\n\
    \x20          Exit status 1 on errors.\n\
    \x20 macros   list the workbook's macros (its VBA project): modules and\n\
    \x20          their Subs and Functions with line numbers; --module NAME\n\
    \x20          prints a module's code.\n\
    \x20 macros run  run macro NAME (Macro1 or Module1.Macro1) of FILE and\n\
    \x20          save the result (in place unless --out). Only run macros\n\
    \x20          you trust: they change the workbook like a person would.\n\
    \x20          MsgBox prints to standard error and answers OK (or Yes);\n\
    \x20          InputBox takes the --input values in order and fails when\n\
    \x20          none is left. The workbook's VBA functions work in its\n\
    \x20          formulas. On a run-time error nothing is saved (exit 1).\n\
    \x20 edit     run editing tools on a file and save it (in place unless -o):\n\
    \x20          TOOL is one of the tools below, ARGS its JSON arguments (or\n\
    \x20          @FILE); --ops runs a list of {\"tool\": ..., \"args\": {...}}.\n\
    \x20          Each tool prints its result as one JSON line.\n\
    \x20 schema   print the JSON Schema of workbooks and tool arguments\n\
    \x20 merge    one workbook per record of a data file, {{Field}} filled in\n\
    \x20          (kantor sheets merge --help)\n\
    \n\
    Tools: write_range write_columns clear fill_down fill_right format\n\
    \x20 column_width autofit row_height freeze insert_rows delete_rows\n\
    \x20 insert_columns delete_columns hide add_sheet delete_sheet rename_sheet\n\
    \x20 move_sheet copy_sheet hide_sheet sort remove_duplicates trim\n\
    \x20 to_numbers to_dates split_column combine_columns find_replace\n\
    \x20 add_table add_chart add_conditional_format clear_conditional_formats\n\
    \x20 add_validation clear_validation define_name delete_name add_pivot\n\
    \x20 page_setup recalculate (kantor sheets schema has their arguments)\n\
    \n\
    See a sheet as a picture: kantor convert FILE.xlsx -o DIR --to png\n\
    (one PNG per printed page; --contact-sheet for one picture).\n\
    \n\
    Options:\n\
    \x20 -o, --output FILE   the .xlsx to write\n\
    \x20 -f, --force         overwrite an existing output file\n\
    \x20     --check         build: check the workbook it wrote\n\
    \x20     --json          check: findings as JSON; macros: the listing (and\n\
    \x20                     --module's code) as JSON\n\
    \x20     --module NAME   macros: the module whose code to print\n\
    \x20     --macro NAME    macros run: the macro to run\n\
    \x20     --out FILE      macros run: the workbook to write (.xlsm keeps\n\
    \x20                     the macros; same as -o)\n\
    \x20     --input V       macros run: an answer for the next InputBox\n\
    \x20                     (repeat for several)\n\
    \x20     --events        macros run: fire Workbook_Open first (and the save\n\
    \x20                     events when saving), as Excel does with macros on\n\
    \x20     --allow-path P  macros run: a file or folder the macro may open or\n\
    \x20                     save (Workbooks.Open, SaveAs); any other path is\n\
    \x20                     refused (repeat for several)\n\
    \x20     --sheet S       list, check: one sheet (name or number from 1)\n\
    \x20     --range R       list: the cells to show (A1:H200, Data!A1:H200)\n\
    \x20     --formulas      list: formulas in place of their values\n\
    \x20     --max-rows N    list: rows of cells per sheet (0: summaries only)\n\
    \x20 -h, --help          show this help\n\
    \n\
    Exit status: 0 on success, 1 when the work failed (or check found\n\
    errors), 2 for usage errors.";

/// A usage error (exit status 2).
struct Usage(String);

impl From<&str> for Usage {
    fn from(s: &str) -> Self {
        Usage(s.to_owned())
    }
}

#[derive(Default)]
struct Opts {
    output: Option<PathBuf>,
    force: bool,
    ops: Option<String>,
    check: bool,
    json: bool,
    sheet: Option<String>,
    range: Option<String>,
    formulas: bool,
    max_rows: Option<u32>,
    module: Option<String>,
    macro_name: Option<String>,
    inputs: Vec<String>,
    events: bool,
    allow_paths: Vec<PathBuf>,
    rest: Vec<String>,
}

fn parse(args: &[OsString]) -> Result<Opts, Usage> {
    let mut o = Opts::default();
    let mut it = args.iter();
    let value = |it: &mut std::slice::Iter<OsString>, flag: &str| -> Result<String, Usage> {
        it.next()
            .map(|v| v.to_string_lossy().into_owned())
            .ok_or_else(|| Usage(format!("{flag} needs a value")))
    };
    while let Some(a) = it.next() {
        let a = a
            .to_str()
            .ok_or_else(|| Usage("arguments must be valid UTF-8".into()))?;
        match a {
            "-o" | "--output" | "--out" => o.output = Some(PathBuf::from(value(&mut it, a)?)),
            "--macro" => o.macro_name = Some(value(&mut it, a)?),
            "--input" => o.inputs.push(value(&mut it, a)?),
            "--events" => o.events = true,
            "--allow-path" => o.allow_paths.push(PathBuf::from(value(&mut it, a)?)),
            "-f" | "--force" => o.force = true,
            "--check" => o.check = true,
            "--json" => o.json = true,
            "--formulas" => o.formulas = true,
            "--sheet" => o.sheet = Some(value(&mut it, a)?),
            "--range" => o.range = Some(value(&mut it, a)?),
            "--ops" => o.ops = Some(value(&mut it, a)?),
            "--module" => o.module = Some(value(&mut it, a)?),
            "--max-rows" => {
                let v = value(&mut it, a)?;
                o.max_rows = Some(
                    v.parse()
                        .map_err(|_| Usage(format!("--max-rows {v}: give a number")))?,
                );
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

fn is_xlsx(p: &Path) -> bool {
    p.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("xlsx") || e.eq_ignore_ascii_case("xlsm"))
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

fn open(path: &str) -> Result<Workbook> {
    let p = Path::new(path);
    if !p.is_file() {
        bail!("{path} is not a file");
    }
    let password = std::env::var("KANTOR_OFFICE_PASSWORD").ok();
    let book = Workbook::open_with_password(p, password.as_deref())
        .with_context(|| format!("can't open {path}"))?;
    for note in book.import_notes() {
        eprintln!("kantor sheets: note: {path}: {note}");
    }
    Ok(book)
}

/// Run `kantor sheets …`; returns the exit status.
pub fn main(args: &[OsString]) -> i32 {
    let Some(cmd) = args.first().and_then(|a| a.to_str()) else {
        eprintln!("{USAGE}");
        return 2;
    };
    if matches!(cmd, "-h" | "--help" | "help") || args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return 0;
    }
    let opts = match parse(&args[1..]) {
        Ok(o) => o,
        Err(Usage(msg)) => {
            eprintln!("kantor sheets: {msg}\n\n{USAGE}");
            return 2;
        }
    };
    let result = match cmd {
        "build" => build(&opts),
        "list" => list(&opts),
        "check" => match opts.rest.as_slice() {
            [file] => return check(Path::new(file), &opts),
            _ => Ok(Err("check takes one FILE".into())),
        },
        "edit" => edit(&opts),
        "macros" => macros(&opts),
        "schema" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&sheets_schema::workbook()).unwrap_or_default()
            );
            Ok(Ok(()))
        }
        other => Ok(Err(Usage(format!("unknown command {other}")))),
    };
    match result {
        Ok(Ok(())) if cmd == "build" && opts.check => match &opts.output {
            Some(out) => check(out, &opts),
            None => 0,
        },
        Ok(Ok(())) => 0,
        Ok(Err(Usage(msg))) => {
            eprintln!("kantor sheets {cmd}: {msg}\n\n{USAGE}");
            2
        }
        Err(e) => {
            eprintln!("kantor sheets {cmd}: {e:#}");
            1
        }
    }
}

type Outcome = Result<Result<(), Usage>>;

fn build(o: &Opts) -> Outcome {
    let [desc] = o.rest.as_slice() else {
        return Ok(Err("build takes one BOOK.json".into()));
    };
    let Some(out) = &o.output else {
        return Ok(Err("build needs -o OUT.xlsx".into()));
    };
    if !is_xlsx(out) {
        return Ok(Err("the output must be a .xlsx file".into()));
    }
    if out.exists() && !o.force {
        bail!("{} exists (use -f to overwrite)", out.display());
    }
    let json = read_json(desc)?;
    let base = if desc == "-" {
        std::env::current_dir().ok()
    } else {
        Path::new(desc)
            .canonicalize()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
    };
    let book = sheets_author::build(&json, base.as_deref())?;
    let bytes = sheets_author::to_bytes(&book)?;
    super::slides::write_file(out, &bytes)?;
    let n = book.sheet_names().len();
    let line = format!(
        "{desc} -> {} ({n} sheet{})",
        out.display(),
        if n == 1 { "" } else { "s" }
    );
    if o.check && o.json {
        eprintln!("{line}");
    } else {
        println!("{line}");
    }
    Ok(Ok(()))
}

fn list(o: &Opts) -> Outcome {
    let [file] = o.rest.as_slice() else {
        return Ok(Err("list takes one FILE".into()));
    };
    let book = open(file)?;
    let mut v = sheets_list::list(
        &book,
        &sheets_list::ListOpts {
            sheet: o.sheet.clone(),
            range: o.range.clone(),
            formulas: o.formulas,
            max_rows: o.max_rows,
        },
    )?;
    v["file"] = json!(file);
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(Ok(()))
}

fn macros(o: &Opts) -> Outcome {
    if o.rest.first().map(String::as_str) == Some("run") {
        return run_macro(o);
    }
    let [file] = o.rest.as_slice() else {
        return Ok(Err("macros takes one FILE".into()));
    };
    let book = open(file)?;
    let opts = sheets_macros::MacrosOpts {
        module: o.module.clone(),
        code: false,
    };
    if o.json {
        let mut v = sheets_macros::json(&book, &opts)?;
        v["file"] = json!(file);
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else if let Some(name) = &o.module {
        let v = sheets_macros::json(&book, &opts)?;
        print!("{}", v["modules"][0]["code"].as_str().unwrap_or_default());
        if let Some(e) = v["modules"][0]["error"].as_str() {
            eprintln!("kantor sheets macros: {name}: {e}");
        }
    } else {
        print!("{}", sheets_macros::text(&book)?);
    }
    Ok(Ok(()))
}

/// `kantor sheets macros run FILE --macro NAME`: run a macro headless and
/// save the workbook.
fn run_macro(o: &Opts) -> Outcome {
    let [_, file] = o.rest.as_slice() else {
        return Ok(Err("macros run takes one FILE".into()));
    };
    let Some(name) = &o.macro_name else {
        return Ok(Err("macros run needs --macro NAME".into()));
    };
    let out = o.output.clone().unwrap_or_else(|| PathBuf::from(file));
    if !is_xlsx(&out) {
        return Ok(Err("the output must be a .xlsm or .xlsx file".into()));
    }
    if o.output.is_some() && out.exists() && !o.force {
        bail!("{} exists (use -f to overwrite)", out.display());
    }
    let mut book = open(file)?;
    let macros = match sheets_script::MacroProject::of_book(&book) {
        None => bail!("{file} has no macros"),
        Some(Err(e)) => bail!("the workbook's VBA project can't be read: {e}"),
        Some(Ok(m)) => m,
    };
    let known = macros.macros();
    let wanted = name.to_ascii_lowercase();
    if !known.iter().any(|m| {
        m.name.to_ascii_lowercase() == wanted || m.full_name().to_ascii_lowercase() == wanted
    }) {
        let list: Vec<String> = known.iter().map(|m| m.full_name()).collect();
        bail!(
            "there is no macro {name} (macros: {})",
            if list.is_empty() {
                "none".to_owned()
            } else {
                list.join(", ")
            }
        );
    }
    // Running the file's macros trusts it: its VBA functions work in its
    // formulas too.
    sheets_script::enable_functions(&mut book, Some(&macros));
    book.recalculate();
    let sheet = book.active_sheet();
    let view = sheets_script::View {
        sheet,
        ..Default::default()
    };
    let mut ui = sheets_script::HeadlessUi::new(o.inputs.clone(), true);
    let mut allowed = Vec::new();
    for p in &o.allow_paths {
        let abs =
            std::path::absolute(p).with_context(|| format!("--allow-path {}", p.display()))?;
        allowed.push(abs);
    }
    ui.allowed_paths = allowed.clone();
    let options = sheets_script::RunOptions::default();
    let mut engine = sheets_script::MacroEngine::new(macros.clone());
    engine.set_paths(allowed, false);
    let mut book = book;
    let mut view = view;
    let mut extra = Vec::new();
    if o.events {
        let (b, outcome) = engine.fire(
            book,
            &sheets_script::Event::Open,
            view,
            Box::new(ui.clone()),
            &options,
            None,
        );
        book = b;
        if let Err(e) = &outcome.result {
            bail!("Workbook_Open: {e}; the workbook was not saved");
        }
        view = outcome.view;
        extra.extend(outcome.books);
    }
    let (b, outcome) = engine.run(book, name, view, Box::new(ui.clone()), &options);
    book = b;
    if let Err(e) = &outcome.result {
        bail!("{name}: {e}; the workbook was not saved");
    }
    let view = outcome.view;
    extra.extend(outcome.books);
    let unsaved = extra
        .iter()
        .filter(|b| matches!(b, sheets_script::BookOut::New { book } if book.is_dirty()))
        .count();
    if unsaved > 0 {
        eprintln!(
            "kantor sheets macros run: note: {unsaved} workbook(s) the macro opened or added were changed and not saved (the macro didn't save them)"
        );
    }
    if !engine.timers().is_empty() {
        eprintln!(
            "kantor sheets macros run: note: {} Application.OnTime schedule(s) didn't run (the command ends with the macro)",
            engine.timers().len()
        );
    }
    if o.events {
        let (b, outcome) = engine.fire(
            book,
            &sheets_script::Event::BeforeSave { save_as_ui: false },
            view,
            Box::new(ui.clone()),
            &options,
            None,
        );
        book = b;
        if let Err(e) = &outcome.result {
            bail!("Workbook_BeforeSave: {e}; the workbook was not saved");
        }
        if outcome.cancel {
            eprintln!("kantor sheets macros run: Workbook_BeforeSave cancelled the save");
            return Ok(Ok(()));
        }
    }
    book.set_user_functions(None);
    let saved = book.save_as(&out);
    if o.events {
        let (b, _) = engine.fire(
            book,
            &sheets_script::Event::AfterSave {
                success: saved.is_ok(),
            },
            view,
            Box::new(ui.clone()),
            &options,
            None,
        );
        book = b;
    }
    saved.with_context(|| format!("can't save {}", out.display()))?;
    for note in book.save_notes() {
        eprintln!("kantor sheets macros run: note: {note}");
    }
    println!("{file}: ran {name} -> {}", out.display());
    Ok(Ok(()))
}

fn check(file: &Path, o: &Opts) -> i32 {
    let book = match open(&file.display().to_string()) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("kantor sheets check: {e:#}");
            return 1;
        }
    };
    let only = match &o.sheet {
        None => None,
        Some(s) => {
            let v = match s.parse::<u64>() {
                Ok(n) => json!({"sheet": n}),
                Err(_) => json!({"sheet": s}),
            };
            match crate::agent::sheets::sheet_index(&book, &v, 0) {
                Ok(i) => Some(i),
                Err(e) => {
                    eprintln!("kantor sheets check: --sheet {s}: {e:#}");
                    return 2;
                }
            }
        }
    };
    let report = sheets_check::check(&book, only);
    if o.json {
        let mut v = report.json();
        v["file"] = json!(file.display().to_string());
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
                        tool.to_owned(),
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
            (file, vec![(tool.clone(), args)])
        }
        ([file, tool], None) => (file, vec![(tool.clone(), json!({}))]),
        _ => {
            return Ok(Err(
                "edit takes FILE and TOOL ARGS, or --ops OPS.json".into()
            ));
        }
    };
    for (tool, _) in &ops {
        if sheets_author::spec(tool).is_none() {
            return Ok(Err(Usage(format!(
                "{tool} is not a sheets editing tool (one of {})",
                sheets_author::TOOLS
                    .iter()
                    .map(|t| t.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))));
        }
    }
    let out = o.output.clone().unwrap_or_else(|| PathBuf::from(file));
    if !is_xlsx(&out) {
        return Ok(Err("the output must be a .xlsx file (give -o)".into()));
    }
    let in_place = o.output.is_none();
    if !in_place && out.exists() && !o.force {
        bail!("{} exists (use -f to overwrite)", out.display());
    }
    let mut book = open(file)?;
    if in_place && !is_xlsx(Path::new(file)) {
        bail!("{file} can't be saved in place (it is not a .xlsx); give -o NEW.xlsx");
    }
    let base = std::env::current_dir().ok();
    for (n, (tool, args)) in ops.iter().enumerate() {
        let (what, result) = sheets_author::run_tool(&mut book, tool, args, base.as_deref())
            .with_context(|| format!("operation {} ({tool})", n + 1))?;
        let mut line = result;
        if let Some(m) = line.as_object_mut() {
            m.insert("done".into(), json!(what));
        }
        println!("{line}");
    }
    book.save_as(&out)?;
    for note in book.save_notes() {
        eprintln!("kantor sheets: note: {}: {note}", out.display());
    }
    println!("saved {}", out.display());
    Ok(Ok(()))
}
