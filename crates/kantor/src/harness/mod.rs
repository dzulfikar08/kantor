//! The tools Paparan AI's agent loop ([`kantor_ai::agent`]) works with:
//! files and Kantor's slides, docs and sheets tools, all confined to one
//! work folder. No shell: each tool calls the Rust functions behind
//! `kantor slides|docs|sheets build|list|edit|check|schema` and `kantor
//! convert` directly.
//!
//! The user's files that are in the folder when the job starts are kept as
//! they are: results go to new files (the skills' first rule, enforced).

mod keys;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use kantor_ai::Cancel;
use kantor_ai::agent::{Image, ToolOutput, ToolSpec, Tools, page_lines};
use serde_json::{Value, json};

use crate::agent::{
    docs_check, docs_edit, docs_list, docs_schema, sheets_author, sheets_check, sheets_list,
    sheets_schema, slides, slides_author, slides_check, slides_schema,
};
use crate::cli::render_png::{self, PngOptions};
use crate::cli::slides::write_file;

/// Which of Kantor's apps a file or a skill is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum App {
    Slides,
    Docs,
    Sheets,
}

impl App {
    pub const ALL: [App; 3] = [App::Slides, App::Docs, App::Sheets];

    pub fn name(self) -> &'static str {
        match self {
            App::Slides => "slides",
            App::Docs => "docs",
            App::Sheets => "sheets",
        }
    }

    fn from_name(s: &str) -> Option<App> {
        App::ALL.into_iter().find(|a| a.name() == s.trim())
    }

    /// The app that opens `path`, by its extension.
    pub fn of(path: &Path) -> Option<App> {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "pptx" | "ppt" | "odp" | "pps" | "ppsx" => Some(App::Slides),
            "docx" | "doc" | "rtf" | "odt" | "dot" | "ott" => Some(App::Docs),
            "xlsx" | "xlsm" | "xls" | "xlsb" | "ods" | "csv" | "tsv" => Some(App::Sheets),
            _ => None,
        }
    }

    /// The apps a skill is about, from its name (`paparan-deck`).
    pub fn for_skill(name: &str) -> Vec<App> {
        let n = name.to_ascii_lowercase();
        let mut out = Vec::new();
        if n.contains("deck") || n.contains("slide") {
            out.push(App::Slides);
        }
        if n.contains("doc") {
            out.push(App::Docs);
        }
        if n.contains("sheet") || n.contains("book") {
            out.push(App::Sheets);
        }
        if out.is_empty() {
            out = App::ALL.to_vec();
        }
        out
    }

    fn schema(self) -> Value {
        match self {
            App::Slides => slides_schema::deck(),
            App::Docs => docs_schema::document(),
            App::Sheets => sheets_schema::workbook(),
        }
    }

    /// The file type `build` writes.
    fn native_ext(self) -> &'static str {
        match self {
            App::Slides => "pptx",
            App::Docs => "docx",
            App::Sheets => "xlsx",
        }
    }
}

/// Pictures one `look` hands the model at most.
const MAX_LOOK: usize = 4;
/// The longest file `read_file` reads.
const MAX_READ: u64 = 8 * 1024 * 1024;

/// The document tools over one work folder.
pub struct DocTools {
    root: PathBuf,
    /// The apps whose keys go in the primer.
    apps: Vec<App>,
    /// Files that were there before the job (relative paths): never
    /// overwritten.
    originals: BTreeSet<PathBuf>,
    protect: bool,
    tmp: u32,
}

impl DocTools {
    /// Tools in `root` (which must exist). The files in it now are the
    /// user's originals and stay untouched unless `protect` is false.
    pub fn new(root: &Path, apps: Vec<App>, protect: bool) -> Result<Self> {
        let root = root
            .canonicalize()
            .with_context(|| format!("can't open the work folder {}", root.display()))?;
        if !root.is_dir() {
            bail!("{} is not a folder", root.display());
        }
        let mut originals = BTreeSet::new();
        for (rel, _) in walk(&root, 4) {
            originals.insert(rel);
        }
        Ok(Self {
            root,
            apps,
            originals,
            protect,
            tmp: 0,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `p` (relative to the work folder, or absolute inside it) as a path
    /// inside the work folder; anything else is refused.
    pub fn resolve(&self, p: &str) -> Result<PathBuf> {
        let p = p.trim();
        if p.is_empty() {
            bail!("give a path");
        }
        let path = Path::new(p);
        let rel = if path.is_absolute() {
            path.strip_prefix(&self.root)
                .map_err(|_| anyhow!("{p} is outside the work folder; use paths relative to it"))?
                .to_path_buf()
        } else {
            path.to_path_buf()
        };
        if rel
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
        {
            bail!("{p}: stay inside the work folder (no `..`)");
        }
        let full = self.root.join(&rel);
        // Through symbolic links, too.
        let mut probe = full.clone();
        while !probe.exists() {
            if !probe.pop() {
                break;
            }
        }
        if let Ok(real) = probe.canonicalize()
            && !real.starts_with(&self.root)
        {
            bail!("{p} leads outside the work folder");
        }
        Ok(full)
    }

    /// `path` as the model sees it (relative, with `/`).
    fn show(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// Refuse to write over one of the user's files.
    fn writable(&self, path: &Path) -> Result<()> {
        if !self.protect {
            return Ok(());
        }
        let rel = path.strip_prefix(&self.root).unwrap_or(path);
        let rel: PathBuf = rel
            .components()
            .filter(|c| !matches!(c, Component::CurDir))
            .collect();
        if self.originals.contains(&rel) {
            bail!(
                "{} is one of the user's files and stays as it is: write the result to a new file (e.g. {})",
                self.show(path),
                suggest_name(&rel)
            );
        }
        Ok(())
    }

    /// Picture paths in build or edit JSON (`path`, `logo`, `picture`)
    /// must stay in the work folder; relative ones start from `base`.
    fn check_paths(&self, v: &Value, base: &Path) -> Result<()> {
        match v {
            Value::Object(m) => {
                for (k, v) in m {
                    if matches!(k.as_str(), "path" | "logo" | "picture")
                        && let Some(s) = v.as_str()
                    {
                        let p = Path::new(s.trim());
                        let joined = if p.is_absolute() {
                            p.to_path_buf()
                        } else {
                            base.join(p)
                        };
                        let rel = joined
                            .strip_prefix(&self.root)
                            .map(|r| r.to_string_lossy().into_owned())
                            .map_err(|_| anyhow!("picture {s}: use a file in the work folder"))?;
                        self.resolve(&rel).with_context(|| format!("picture {s}"))?;
                    } else {
                        self.check_paths(v, base)?;
                    }
                }
                Ok(())
            }
            Value::Array(items) => items.iter().try_for_each(|i| self.check_paths(i, base)),
            _ => Ok(()),
        }
    }

    fn file_arg(&self, args: &Value, key: &str) -> Result<PathBuf> {
        let s = args
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("`{key}` is required"))?;
        self.resolve(s)
    }

    fn existing(&self, args: &Value, key: &str) -> Result<PathBuf> {
        let p = self.file_arg(args, key)?;
        if !p.is_file() {
            bail!(
                "{} doesn't exist (list_files shows what's there)",
                self.show(&p)
            );
        }
        Ok(p)
    }
}

/// `deck.pptx` → `deck-new.pptx`.
fn suggest_name(rel: &Path) -> String {
    let stem = rel
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = match rel.extension() {
        Some(e) => format!("{stem}-new.{}", e.to_string_lossy()),
        None => format!("{stem}-new"),
    };
    rel.with_file_name(name)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Files under `dir` (relative paths and sizes), `depth` folders deep,
/// hidden ones left out.
fn walk(dir: &Path, depth: usize) -> Vec<(PathBuf, u64)> {
    fn go(dir: &Path, rel: &Path, depth: usize, out: &mut Vec<(PathBuf, u64)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let rel = rel.join(&name);
            let path = e.path();
            if path.is_dir() {
                if depth > 0 {
                    go(&path, &rel, depth - 1, out);
                }
            } else if let Ok(m) = e.metadata() {
                out.push((rel, m.len()));
            }
        }
    }
    let mut out = Vec::new();
    go(dir, Path::new(""), depth, &mut out);
    out
}

fn size(n: u64) -> String {
    match n {
        0..1024 => format!("{n} B"),
        1024..1_048_576 => format!("{:.1} KB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.0),
    }
}

/// Changes whenever the file does (size, modified time).
fn version(path: &Path) -> String {
    let m = std::fs::metadata(path).ok();
    let len = m.as_ref().map_or(0, |m| m.len());
    let t = m
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    format!("{len}-{t}")
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn req_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("`{key}` is required (a string)"))
}

fn app_of(path: &Path) -> Result<App> {
    App::of(path).ok_or_else(|| {
        anyhow!(
            "{}: not a presentation, document or workbook (.pptx, .docx, .xlsx)",
            path.display()
        )
    })
}

/// Check findings as text: the counts, then errors and warnings (info
/// too when asked).
fn findings_text(text: &str, info: bool) -> String {
    text.lines()
        .filter(|l| {
            info || !(l.starts_with(char::is_whitespace) && l.trim_start().starts_with("info"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The check report of `path` as text, and its error count.
fn check_file(path: &Path, args: &Value, info: bool) -> Result<(String, usize)> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (text, errors) = match app_of(path)? {
        App::Slides => {
            let pres = slides_engine::Presentation::open(path)?;
            let only = match args.get("slide").and_then(Value::as_u64) {
                Some(n) if n >= 1 && (n as usize) <= pres.len() => Some(n as usize - 1),
                Some(n) => bail!("there is no slide {n}; the deck has {}", pres.len()),
                None => None,
            };
            let report = slides_check::check(&pres, &mut slides_scene::Engine::new(), only);
            (report.text(&name), usize::from(report.has_errors()))
        }
        App::Docs => {
            let doc = docs_edit::open(path)?;
            let report = docs_check::check(&doc);
            (report.text(&name), usize::from(report.has_errors()))
        }
        App::Sheets => {
            let book = open_book(path)?;
            let only = match args.get("sheet").filter(|v| !v.is_null()) {
                Some(v) => Some(crate::agent::sheets::sheet_index(
                    &book,
                    &json!({ "sheet": v }),
                    0,
                )?),
                None => None,
            };
            let report = sheets_check::check(&book, only);
            (report.text(&name), usize::from(report.has_errors()))
        }
    };
    Ok((findings_text(&text, info), errors))
}

fn open_book(path: &Path) -> Result<sheets_engine::Workbook> {
    let book = sheets_engine::Workbook::open_with_password(path, None)
        .with_context(|| format!("can't open {}", path.display()))?;
    for note in book.import_notes() {
        log::info!("{}: {note}", path.display());
    }
    Ok(book)
}

impl DocTools {
    fn list_files(&self, args: &Value) -> Result<ToolOutput> {
        let dir = match str_arg(args, "path") {
            Some(p) => self.resolve(p)?,
            None => self.root.clone(),
        };
        if !dir.is_dir() {
            bail!("{} is not a folder", self.show(&dir));
        }
        let files = walk(&dir, 3);
        if files.is_empty() {
            return Ok(ToolOutput::ok("(no files)"));
        }
        let mut out: Vec<String> = files
            .iter()
            .take(300)
            .map(|(p, n)| {
                let full = dir.join(p);
                let mark =
                    if self.protect && self.originals.contains(&PathBuf::from(self.show(&full))) {
                        "  (user's file: read-only)"
                    } else {
                        ""
                    };
                format!("{}  {}{mark}", self.show(&full), size(*n))
            })
            .collect();
        if files.len() > 300 {
            out.push(format!("… {} more", files.len() - 300));
        }
        Ok(ToolOutput::ok(out.join("\n")))
    }

    fn read_file(&self, args: &Value) -> Result<ToolOutput> {
        let path = self.existing(args, "path")?;
        let len = path.metadata().map(|m| m.len()).unwrap_or(0);
        if len > MAX_READ {
            bail!("{} is too large to read ({})", self.show(&path), size(len));
        }
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if matches!(ext.as_str(), "png" | "jpg" | "jpeg") {
            let bytes = std::fs::read(&path)?;
            let shown = self.show(&path);
            return Ok(
                ToolOutput::ok(format!("{shown}: a picture")).with_images(vec![Image {
                    file: shown.clone(),
                    page: None,
                    version: version(&path),
                    label: shown,
                    mime: if ext == "png" {
                        "image/png"
                    } else {
                        "image/jpeg"
                    },
                    bytes,
                }]),
            );
        }
        if App::of(&path).is_some() && !matches!(ext.as_str(), "csv" | "tsv") {
            bail!(
                "{} is a {} file: read it with `list` (and `look` for pictures)",
                self.show(&path),
                ext
            );
        }
        let bytes = std::fs::read(&path)?;
        if bytes.iter().take(8000).any(|&b| b == 0) {
            bail!("{} is a binary file", self.show(&path));
        }
        let text = String::from_utf8_lossy(&bytes);
        let n = |k: &str| args.get(k).and_then(Value::as_u64);
        Ok(ToolOutput::ok(page_lines(&text, n("offset"), n("limit")))
            .more("read on with read_file offset/limit (lines)"))
    }

    fn write_file(&self, args: &Value) -> Result<ToolOutput> {
        let path = self.file_arg(args, "path")?;
        let content = req_str(args, "content")?;
        self.writable(&path)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if path.extension().is_some_and(|e| e == "json")
            && let Err(e) = serde_json::from_str::<Value>(content)
        {
            // Written anyway (the model can fix it), but say so now.
            write_file(&path, content.as_bytes())?;
            return Ok(ToolOutput::error(format!(
                "wrote {} but it is not valid JSON: {e}",
                self.show(&path)
            )));
        }
        write_file(&path, content.as_bytes())?;
        Ok(ToolOutput::ok(format!(
            "wrote {} ({})",
            self.show(&path),
            size(content.len() as u64)
        )))
    }

    fn replace_in_file(&self, args: &Value) -> Result<ToolOutput> {
        let path = self.existing(args, "path")?;
        self.writable(&path)?;
        let old = req_str(args, "old")?;
        let new = req_str(args, "new")?;
        if old.is_empty() {
            bail!("`old` is empty");
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("{} is not a text file", self.show(&path)))?;
        let n = text.matches(old).count();
        let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
        if n == 0 {
            bail!(
                "`old` is not in {} (it must match exactly, spaces too; read_file shows the text)",
                self.show(&path)
            );
        }
        if n > 1 && !all {
            bail!(
                "`old` is in {} {n} times: give more context, or all: true",
                self.show(&path)
            );
        }
        let out = if all {
            text.replace(old, new)
        } else {
            text.replacen(old, new, 1)
        };
        write_file(&path, out.as_bytes())?;
        let mut msg = format!(
            "replaced {} in {}",
            if all { n } else { 1 },
            self.show(&path)
        );
        if path.extension().is_some_and(|e| e == "json")
            && let Err(e) = serde_json::from_str::<Value>(&out)
        {
            msg.push_str(&format!("; it is no longer valid JSON: {e}"));
            return Ok(ToolOutput::error(msg));
        }
        Ok(ToolOutput::ok(msg))
    }

    fn build(&mut self, args: &Value) -> Result<ToolOutput> {
        let source = self.existing(args, "source")?;
        let out = match str_arg(args, "output") {
            Some(o) => self.resolve(o)?,
            None => bail!("`output` is required: the .pptx, .docx or .xlsx to write"),
        };
        let app = app_of(&out)?;
        if out
            .extension()
            .is_none_or(|e| !e.eq_ignore_ascii_case(app.native_ext()))
            && !(app == App::Sheets && out.extension().is_some_and(|e| e == "xlsm"))
        {
            bail!("build writes .pptx, .docx or .xlsx files");
        }
        self.writable(&out)?;
        let text = std::fs::read_to_string(&source)
            .with_context(|| format!("can't read {}", self.show(&source)))?;
        let json: Value = serde_json::from_str(&text)
            .with_context(|| format!("{} is not valid JSON", self.show(&source)))?;
        let base = source.parent().unwrap_or(&self.root).to_path_buf();
        self.check_paths(&json, &base)?;
        let mut lines = Vec::new();
        match app {
            App::Slides => {
                let p = slides_author::build(&json, Some(&base))?;
                write_file(&out, &p.to_bytes()?)?;
                lines.push(format!("built {} ({} slides)", self.show(&out), p.len()));
            }
            App::Docs => {
                let built = crate::agent::docs_author::build(&json, Some(&base))?;
                write_file(&out, &built.bytes)?;
                lines.push(format!(
                    "built {} ({} page{})",
                    self.show(&out),
                    built.pages,
                    if built.pages == 1 { "" } else { "s" }
                ));
                lines.extend(built.warnings.iter().map(|w| format!("note: {w}")));
            }
            App::Sheets => {
                let book = sheets_author::build(&json, Some(&base))?;
                write_file(&out, &sheets_author::to_bytes(&book)?)?;
                let n = book.sheet_names().len();
                lines.push(format!(
                    "built {} ({n} sheet{})",
                    self.show(&out),
                    if n == 1 { "" } else { "s" }
                ));
            }
        }
        let (check, _) = check_file(&out, &json!({}), false)?;
        lines.push(format!("check: {check}"));
        let mut result = ToolOutput::ok(lines.join("\n"));
        if str_arg(args, "look") != Some("none") {
            match self.render(&out, None, true, None) {
                Ok(look) => {
                    result.text.push('\n');
                    result.text.push_str(&look.text);
                    result.images = look.images;
                }
                Err(e) => result
                    .text
                    .push_str(&format!("\n(no contact sheet: {e:#})")),
            }
        }
        Ok(result)
    }

    fn check(&self, args: &Value) -> Result<ToolOutput> {
        let path = self.existing(args, "file")?;
        let info = args.get("info").and_then(Value::as_bool).unwrap_or(false);
        let (text, _) = check_file(&path, args, info)?;
        Ok(ToolOutput::ok(text).more("check one slide (slide) or sheet (sheet) at a time"))
    }

    fn list(&self, args: &Value) -> Result<ToolOutput> {
        let path = self.existing(args, "file")?;
        let v = match app_of(&path)? {
            App::Slides => {
                let p = slides_engine::Presentation::open(&path)?;
                let mut out = slides::list(&p, 0);
                if let Some(m) = out.as_object_mut() {
                    m.remove("current_slide");
                    m.insert("size".into(), json!({"w": p.size.cx, "h": p.size.cy}));
                }
                if let Some(n) = args.get("slide").and_then(Value::as_u64) {
                    let one = out["slides"]
                        .get(n.saturating_sub(1) as usize)
                        .cloned()
                        .ok_or_else(|| anyhow!("there is no slide {n}"))?;
                    out["slides"] = json!([one]);
                }
                out
            }
            App::Docs => {
                let doc = docs_edit::open(&path)?;
                if args.get("text").and_then(Value::as_bool).unwrap_or(false) {
                    let n = |k: &str| args.get(k).and_then(Value::as_u64).map(|n| n as usize);
                    docs_list::text(&doc, n("start").unwrap_or(0), n("limit").unwrap_or(200))
                } else {
                    docs_list::outline(&doc)
                }
            }
            App::Sheets => {
                let book = open_book(&path)?;
                let sheet = match args.get("sheet") {
                    Some(Value::String(s)) => Some(s.clone()),
                    Some(Value::Number(n)) => Some(n.to_string()),
                    _ => None,
                };
                sheets_list::list(
                    &book,
                    &sheets_list::ListOpts {
                        sheet,
                        range: str_arg(args, "range").map(str::to_owned),
                        formulas: args
                            .get("formulas")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        max_rows: args
                            .get("max_rows")
                            .and_then(Value::as_u64)
                            .map(|n| n.min(u64::from(u32::MAX)) as u32),
                    },
                )?
            }
        };
        Ok(ToolOutput::ok(v.to_string()).more(
            "ask for less: slide (slides), text with start/limit (docs), sheet, range or max_rows (sheets)",
        ))
    }

    fn edit(&self, args: &Value) -> Result<ToolOutput> {
        let file = self.existing(args, "file")?;
        let out = match str_arg(args, "output") {
            Some(o) => self.resolve(o)?,
            None => file.clone(),
        };
        self.writable(&out)?;
        let ops = args.get("ops").and_then(Value::as_array).ok_or_else(|| {
            anyhow!("`ops` is required: [{{\"tool\": ..., \"args\": {{...}}}}, ...]")
        })?;
        let ops: Vec<(String, Value)> = ops
            .iter()
            .enumerate()
            .map(|(i, op)| {
                let tool = op
                    .get("tool")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("operation {} has no `tool`", i + 1))?;
                Ok((
                    tool.to_owned(),
                    op.get("args").cloned().unwrap_or(json!({})),
                ))
            })
            .collect::<Result<_>>()?;
        for (_, a) in &ops {
            self.check_paths(a, &self.root)?;
        }
        let app = app_of(&file)?;
        if app_of(&out)? != app {
            bail!("the output must be the same kind of file as the input");
        }
        let mut results = Vec::new();
        match app {
            App::Slides => {
                let mut p = slides_engine::Presentation::open(&file)?;
                if out == file && p.needs_save_as() {
                    bail!(
                        "{} can't be saved in place; give `output`",
                        self.show(&file)
                    );
                }
                for (n, (tool, a)) in ops.iter().enumerate() {
                    let tool = if tool.starts_with("slides_") {
                        tool.clone()
                    } else {
                        format!("slides_{tool}")
                    };
                    if !slides_author::EDIT_TOOLS.contains(&tool.as_str()) {
                        bail!(
                            "operation {}: {tool} is not a slides editing tool (one of {})",
                            n + 1,
                            slides_author::EDIT_TOOLS
                                .iter()
                                .map(|t| t.trim_start_matches("slides_"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                    }
                    let (what, r, _) = slides_author::run_tool(&mut p, &tool, a, Some(&self.root))
                        .with_context(|| format!("operation {} ({tool})", n + 1))?;
                    results.push(json!({"done": what, "result": r}));
                }
                write_file(&out, &p.to_bytes()?)?;
            }
            App::Docs => {
                let track = str_arg(args, "track").map(str::to_owned);
                let mut s = docs_edit::Session::open(&file, Some(self.root.clone()), track)?;
                if out == file && s.read_only() {
                    bail!(
                        "{} can't be saved in place; give `output`",
                        self.show(&file)
                    );
                }
                for (n, (tool, a)) in ops.iter().enumerate() {
                    let r = s
                        .run(tool, a)
                        .with_context(|| format!("operation {} ({tool})", n + 1))?;
                    results.push(r);
                }
                write_file(&out, &s.save()?)?;
            }
            App::Sheets => {
                let mut book = open_book(&file)?;
                for (n, (tool, a)) in ops.iter().enumerate() {
                    let (what, r) = sheets_author::run_tool(&mut book, tool, a, Some(&self.root))
                        .with_context(|| format!("operation {} ({tool})", n + 1))?;
                    results.push(json!({"done": what, "result": r}));
                }
                book.save_as(&out)?;
            }
        }
        let mut text: Vec<String> = results.iter().map(Value::to_string).collect();
        text.push(format!("saved {}", self.show(&out)));
        Ok(ToolOutput::ok(text.join("\n")))
    }

    fn schema(&self, args: &Value) -> Result<ToolOutput> {
        let kind = req_str(args, "kind")?;
        let app =
            App::from_name(kind).ok_or_else(|| anyhow!("`kind` is slides, docs or sheets"))?;
        let schema = app.schema();
        let wanted: Vec<&str> = args
            .get("defs")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if wanted.is_empty() {
            return Ok(ToolOutput::ok(keys::summary(app, &schema)));
        }
        let defs = schema.get("$defs").cloned().unwrap_or(json!({}));
        let mut out = serde_json::Map::new();
        for name in wanted {
            match defs.get(name) {
                Some(d) => {
                    out.insert(name.to_owned(), d.clone());
                }
                None => bail!(
                    "no definition {name}; there are: {}",
                    defs.as_object()
                        .map(|m| m.keys().cloned().collect::<Vec<_>>().join(", "))
                        .unwrap_or_default()
                ),
            }
        }
        Ok(ToolOutput::ok(Value::Object(out).to_string()).more("ask for fewer defs at a time"))
    }

    fn look(&mut self, args: &Value) -> Result<ToolOutput> {
        let file = self.existing(args, "file")?;
        let pages = str_arg(args, "pages").map(str::to_owned);
        let sheet = args
            .get("contact_sheet")
            .and_then(Value::as_bool)
            .unwrap_or(pages.is_none());
        let width = args
            .get("width")
            .and_then(Value::as_u64)
            .map(|w| w.clamp(160, 1200) as u32);
        self.render(&file, pages, sheet, width)
    }

    /// Draw `file` as PNGs into `shots/`: a contact sheet or the pages
    /// `pages` names; the pictures go to the model too (at most
    /// [`MAX_LOOK`]).
    fn render(
        &mut self,
        file: &Path,
        pages: Option<String>,
        sheet: bool,
        width: Option<u32>,
    ) -> Result<ToolOutput> {
        let app = app_of(file)?;
        let shots = self.root.join("shots");
        std::fs::create_dir_all(&shots)?;
        self.tmp += 1;
        let tmp = shots.join(format!(".look-{}-{}", std::process::id(), self.tmp));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp)?;
        let stem = file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        let target = tmp.join(format!("{stem}.png"));
        let opts = PngOptions {
            slides: pages,
            // Small enough to keep the model's picture tokens down.
            width: Some(width.unwrap_or(if sheet { 400 } else { 1000 })),
            sheet,
        };
        let result = render_to(app, file, &target, &opts);
        let mut made: Vec<PathBuf> = std::fs::read_dir(&tmp)
            .map(|d| d.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        made.sort();
        let mut images = Vec::new();
        let mut saved = Vec::new();
        let shown_file = self.show(file);
        let ver = version(file);
        let what = match app {
            App::Slides => "slide",
            App::Docs | App::Sheets => "page",
        };
        for (k, png) in made.iter().enumerate() {
            let Some(name) = png.file_name() else {
                continue;
            };
            let dest = shots.join(name);
            std::fs::rename(png, &dest)?;
            saved.push(self.show(&dest));
            if k >= MAX_LOOK {
                continue;
            }
            let name = name.to_string_lossy();
            let part = name
                .trim_end_matches(".png")
                .rsplit('-')
                .next()
                .unwrap_or_default()
                .trim_start_matches('0')
                .to_owned();
            let (page, label) = if part == "sheet" {
                ("sheet".to_owned(), format!("{shown_file} contact sheet"))
            } else {
                (part.clone(), format!("{shown_file} {what} {part}"))
            };
            images.push(Image {
                file: shown_file.clone(),
                page: Some(page),
                version: ver.clone(),
                label,
                mime: "image/png",
                bytes: std::fs::read(&dest)?,
            });
        }
        let _ = std::fs::remove_dir_all(&tmp);
        // The summary names the temporary folder: name shots/ instead.
        let summary = result?
            .replace(&tmp.display().to_string(), &shots.display().to_string())
            .replace(
                &format!("{}{}", self.root.display(), std::path::MAIN_SEPARATOR),
                "",
            )
            .replace('\\', "/");
        let mut text = format!("{summary}; saved {}", saved.join(", "));
        if saved.len() > MAX_LOOK {
            text.push_str(&format!(
                "; showing the first {MAX_LOOK}: ask for the others with pages"
            ));
        }
        Ok(ToolOutput::ok(text).with_images(images))
    }

    fn convert(&self, args: &Value) -> Result<ToolOutput> {
        let input = self.existing(args, "input")?;
        let output = self.file_arg(args, "output")?;
        self.writable(&output)?;
        if output
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("png"))
        {
            bail!("for pictures use `look` (they come back to you)");
        }
        if let Some(dir) = output.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let summary = crate::cli::convert::convert_file(&input, &output, true)?;
        Ok(ToolOutput::ok(format!(
            "{} -> {} ({summary})",
            self.show(&input),
            self.show(&output)
        )))
    }
}

/// Draw `file` as PNGs named after `target` (`deck-01.png`…, or
/// `deck-sheet.png`); returns the summary.
fn render_to(app: App, file: &Path, target: &Path, opts: &PngOptions) -> Result<String> {
    match app {
        App::Slides => {
            let pres = slides_engine::Presentation::open(file)?;
            render_png::convert(&pres, target, true, opts, true)
        }
        App::Docs => {
            let doc = docs_edit::open(file)?;
            render_png::convert_doc(&doc, target, true, opts, true)
        }
        App::Sheets => {
            let mut book = open_book(file)?;
            let print = sheets_pdf::Options::default().now().with_path(Some(file));
            let (pdf, _) = sheets_pdf::export(&mut book, &print)?;
            render_png::convert_pdf(pdf, target, true, opts, true)
        }
    }
}

/// `{"type": "object", …}` with these properties.
fn params(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required})
}

impl Tools for DocTools {
    fn specs(&self) -> Vec<ToolSpec> {
        let s = |d: &str| json!({"type": "string", "description": d});
        let i = |d: &str| json!({"type": "integer", "description": d});
        let b = |d: &str| json!({"type": "boolean", "description": d});
        vec![
            ToolSpec::new(
                "list_files",
                "List the files in the work folder (or a folder in it), with sizes.",
                params(
                    json!({"path": s("A folder in the work folder (default: all)")}),
                    &[],
                ),
            ),
            ToolSpec::new(
                "read_file",
                "Read a text file (JSON, CSV, Markdown…), or a PNG/JPEG as a picture. Long files: page with offset/limit (lines).",
                params(
                    json!({"path": s("File path"), "offset": i("First line (from 1)"), "limit": i("Lines to read")}),
                    &["path"],
                ),
            ),
            ToolSpec::new(
                "write_file",
                "Write a whole text file (creates folders). The user's own files can't be overwritten: use a new name.",
                params(
                    json!({"path": s("File path"), "content": s("The whole text")}),
                    &["path", "content"],
                ),
            ),
            ToolSpec::new(
                "replace_in_file",
                "Replace exact text in a text file (cheaper than writing it again). `old` must match once, or give all: true.",
                params(
                    json!({"path": s("File path"), "old": s("Text to find, exactly"), "new": s("Replacement"), "all": b("Replace every match")}),
                    &["path", "old", "new"],
                ),
            ),
            ToolSpec::new(
                "build",
                "Build a .pptx, .docx or .xlsx (by output's extension) from a JSON file, as `kantor slides|docs|sheets build` and the skill's build.sh: then checks it and shows its contact sheet.",
                params(
                    json!({
                        "source": s("The JSON file (deck.json, doc.json, book.json)"),
                        "output": s("The file to write: .pptx, .docx or .xlsx"),
                        "look": json!({"type": "string", "enum": ["sheet", "none"], "description": "Contact sheet after the build (default sheet)"}),
                    }),
                    &["source", "output"],
                ),
            ),
            ToolSpec::new(
                "check",
                "Check a .pptx, .docx or .xlsx as `kantor … check`: errors and warnings (info: true for info findings).",
                params(
                    json!({"file": s("File"), "slide": i("Slides: only this slide"), "sheet": s("Sheets: only this sheet (name or number)"), "info": b("Show info findings too")}),
                    &["file"],
                ),
            ),
            ToolSpec::new(
                "list",
                "What a file holds, as `kantor … list` (JSON). Slides: slides, shapes, notes. Docs: outline, or the text with text: true. Sheets: sheets, tables, column summaries, first rows.",
                params(
                    json!({
                        "file": s("File"),
                        "slide": i("Slides: only this slide"),
                        "text": b("Docs: paragraphs' text instead of the outline"),
                        "start": i("Docs text: first paragraph (from 0)"),
                        "limit": i("Docs text: paragraphs (default 200)"),
                        "sheet": s("Sheets: one sheet (name or number)"),
                        "range": s("Sheets: cells to show, e.g. Data!A1:H200"),
                        "formulas": b("Sheets: formulas instead of values"),
                        "max_rows": i("Sheets: rows per sheet (0: summaries only)"),
                    }),
                    &["file"],
                ),
            ),
            ToolSpec::new(
                "edit",
                "Change a file with editing tools, as `kantor … edit FILE -o OUT --ops`: ops is the list itself. Give output for a new file (required for the user's own files).",
                params(
                    json!({
                        "file": s("The .pptx, .docx or .xlsx to change"),
                        "output": s("Write the result here (default: in place)"),
                        "ops": {"type": "array", "description": "[{\"tool\": \"...\", \"args\": {...}}, ...]", "items": {"type": "object"}},
                        "track": s("Docs: record the changes as tracked changes by this author"),
                    }),
                    &["file", "ops"],
                ),
            ),
            ToolSpec::new(
                "schema",
                "JSON Schema of slides decks, docs or sheets workbooks: without defs, every definition's keys; with defs, those definitions in full.",
                params(
                    json!({
                        "kind": json!({"type": "string", "enum": ["slides", "docs", "sheets"]}),
                        "defs": {"type": "array", "items": {"type": "string"}, "description": "Definitions to show, e.g. [\"chart\", \"stat_cards\"]"},
                    }),
                    &["kind"],
                ),
            ),
            ToolSpec::new(
                "look",
                "See a .pptx, .docx or .xlsx as pictures (the PDF render; a workbook's printed pages), as `kantor convert --to png`: a contact sheet of all pages (default), or pages. Saved under shots/ too.",
                params(
                    json!({
                        "file": s("File"),
                        "pages": s("Slides or pages: 3, 2-5, 1,3,7-9 or 4- (at most 4 come back)"),
                        "contact_sheet": b("One numbered picture of all pages (default when no pages)"),
                        "width": i("Width of each page in px (default 1000, at most 1200; 400 in a contact sheet)"),
                    }),
                    &["file"],
                ),
            ),
            ToolSpec::new(
                "convert",
                "Convert a file as `kantor convert IN OUT`: .pptx/.docx/.xlsx to PDF, CSV/.xls/.ods to .xlsx, .xlsx to CSV, .doc/.odt to .docx, .ppt to .pptx.",
                params(
                    json!({"input": s("File to read"), "output": s("File to write")}),
                    &["input", "output"],
                ),
            ),
        ]
    }

    fn primer(&self) -> String {
        let mut out = String::from(PRIMER);
        for app in &self.apps {
            out.push_str("\n\n");
            out.push_str(&keys::summary(*app, &app.schema()));
        }
        let files = walk(&self.root, 2);
        out.push_str("\n\n# The work folder now\n");
        if files.is_empty() {
            out.push_str("(empty)");
        }
        for (p, n) in files.iter().take(40) {
            out.push_str(&format!(
                "{}  {}\n",
                p.to_string_lossy().replace('\\', "/"),
                size(*n)
            ));
        }
        if files.len() > 40 {
            out.push_str(&format!("… {} more (list_files)\n", files.len() - 40));
        }
        out
    }

    fn call(&mut self, name: &str, args: &Value, _cancel: &Cancel) -> ToolOutput {
        let r = match name {
            "list_files" => self.list_files(args),
            "read_file" => self.read_file(args),
            "write_file" => self.write_file(args),
            "replace_in_file" => self.replace_in_file(args),
            "build" => self.build(args),
            "check" => self.check(args),
            "list" => self.list(args),
            "edit" => self.edit(args),
            "schema" => self.schema(args),
            "look" => self.look(args),
            "convert" => self.convert(args),
            other => Err(anyhow!("there is no tool {other}")),
        };
        r.unwrap_or_else(|e| ToolOutput::error(format!("{e:#}")))
    }

    fn check_outputs(&mut self, outputs: &[String]) -> Result<Vec<String>, String> {
        let mut problems = Vec::new();
        let mut shown = Vec::new();
        for o in outputs {
            let path = match self.resolve(o) {
                Ok(p) => p,
                Err(e) => {
                    problems.push(format!("{o}: {e:#}"));
                    continue;
                }
            };
            if !path.is_file() {
                problems.push(format!("{o} doesn't exist"));
                continue;
            }
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if matches!(ext.as_str(), "pptx" | "docx" | "xlsx" | "xlsm") {
                match check_file(&path, &json!({}), false) {
                    Ok((_, 0)) => {}
                    Ok((text, _)) => {
                        let errors: Vec<&str> = text
                            .lines()
                            .filter(|l| {
                                l.starts_with(char::is_whitespace)
                                    && l.trim_start().starts_with("error")
                            })
                            .take(20)
                            .collect();
                        problems.push(format!(
                            "{o}: the check reports errors:\n{}",
                            errors.join("\n")
                        ));
                    }
                    Err(e) => problems.push(format!("{o} can't be opened: {e:#}")),
                }
            }
            shown.push(self.show(&path));
        }
        if problems.is_empty() {
            Ok(shown)
        } else {
            Err(problems.join("\n"))
        }
    }
}

/// How the skills' commands map to the tools.
const PRIMER: &str = "\
# Tools (there is no shell)
All paths are relative to the work folder; nothing outside it can be reached. The user's \
files that were there at the start are read-only: write results to new files.
The skill's commands map to tools:
- `kantor slides|docs|sheets build X.json -o OUT` and the skill's build.sh → `build` {source, output}: \
it builds, checks and returns the contact sheet in one step.
- `kantor … check FILE` → `check`; `kantor … list FILE [--range …]` → `list`; \
`kantor … edit FILE -o OUT --ops OPS.json` → `edit` {file, output, ops: [...]} (the ops inline).
- `kantor … schema | jq …` → `schema` {kind, defs}. The keys below come from the schemas: \
use them directly instead of exploring the schema.
- `kantor convert IN OUT` → `convert` (a PDF copy: output deck.pdf). `kantor convert … --to png \
[--contact-sheet] [--slide N | --page N]` → `look` {file, contact_sheet, pages}: the pictures \
come back to you.
- cat / jq / sed / ls → read_file, write_file, replace_in_file, list_files. Files under \
~/.claude/skills/… → read_skill_file.";
