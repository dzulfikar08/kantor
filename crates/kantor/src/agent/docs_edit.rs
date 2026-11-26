//! `kantor docs edit` (and the MCP `docs_edit` tool): tools that revise a
//! Word document. Some run on the docs engine (find and replace, styles of
//! paragraphs, comments, tracked changes, fields); others change the
//! package's XML (inserted blocks from the build JSON, moved sections,
//! style definitions, headers and footers, page setup). A session keeps the
//! document in whichever form the next tool needs.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use docs_engine::agent::NewParagraph;
use docs_engine::{Document, Position, Selection};
use serde_json::{Map, Value, json};

use super::docs_author::{Props, Writer, missing_styles, normalize};
use super::docs_list::{find_heading, section_end};
use super::docs_look::{
    Look, RunFmt, STYLE_KEYS, get, hf_spec, look_json, object, opt_bool, opt_str, page_spec,
};
use super::docs_pkg::{Pkg, attr_of, esc, insert_before, rel_type, wml_type};

/// The editing tools (without the `docs_` prefix).
pub const TOOLS: &[&str] = &[
    "get_paragraphs",
    "insert_paragraphs",
    "replace_paragraph",
    "delete_paragraphs",
    "find_replace",
    "format",
    "insert_blocks",
    "replace_blocks",
    "delete_blocks",
    "move_section",
    "set_style",
    "restyle",
    "add_comment",
    "resolve_comment",
    "accept_changes",
    "reject_changes",
    "update_fields",
    "set_header",
    "set_footer",
    "page_setup",
];

/// Marks the place blocks go while the package is edited.
const MARKER: &str = "KANTORMARK7F3A";

const SEL: Selection = Selection::caret(Position::new(0, 0));

/// Open a document (`.docx`, or a `.doc`, `.rtf`, `.odt` import).
pub fn open(path: &Path) -> Result<Document> {
    if !path.is_file() {
        bail!("{} is not a file", path.display());
    }
    Document::open(path)
}

/// `[{"tool", "args"}, …]` as (tool, args).
pub fn ops_of(list: &Value) -> Result<Vec<(String, Value)>> {
    let items = list
        .as_array()
        .context("--ops must hold an array of {\"tool\", \"args\"}")?;
    items
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
        .collect()
}

/// A document being edited.
pub struct Session {
    doc: Option<Document>,
    bytes: Vec<u8>,
    base: Option<PathBuf>,
    track: Option<String>,
    tracked_before: bool,
    imported: bool,
}

fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn idx(args: &Value, key: &str) -> Result<Option<usize>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .map(|n| Some(n as usize))
            .with_context(|| format!("`{key}` must be a paragraph id (a whole number)")),
    }
}

impl Session {
    pub fn open(path: &Path, base: Option<PathBuf>, track: Option<String>) -> Result<Self> {
        let doc = open(path)?;
        Self::from_doc(doc, base, track)
    }

    pub fn from_doc(
        mut doc: Document,
        base: Option<PathBuf>,
        track: Option<String>,
    ) -> Result<Self> {
        let imported = doc.is_imported();
        let tracked_before = doc.track_changes();
        if let Some(a) = &track {
            if a.trim().is_empty() {
                bail!("--track needs an author name");
            }
            doc.set_author(a.trim());
            doc.set_track_changes(true, SEL);
        }
        Ok(Self {
            doc: Some(doc),
            bytes: Vec::new(),
            base,
            track,
            tracked_before,
            imported,
        })
    }

    /// The file can't be saved over (an import).
    pub fn read_only(&self) -> bool {
        self.imported
    }

    fn doc(&mut self) -> Result<&mut Document> {
        if self.doc.is_none() {
            let read = docs_engine::docx::read(std::mem::take(&mut self.bytes))?;
            let mut d = Document::from_read(read, None);
            if let Some(a) = &self.track {
                d.set_author(a.trim());
            }
            self.doc = Some(d);
        }
        self.doc.as_mut().context("no document")
    }

    fn pkg(&mut self) -> Result<Pkg> {
        if let Some(d) = self.doc.take() {
            self.bytes = d.to_docx_bytes()?;
        }
        Pkg::read(&self.bytes)
    }

    fn set_pkg(&mut self, p: &Pkg) -> Result<()> {
        self.bytes = p.to_bytes()?;
        self.doc = None;
        Ok(())
    }

    /// The file as it is now.
    pub fn save(&mut self) -> Result<Vec<u8>> {
        let restore = self.track.is_some() && !self.tracked_before;
        let d = self.doc()?;
        if restore {
            d.set_track_changes(false, SEL);
        }
        let out = d.to_docx_bytes()?;
        Pkg::read(&out)?.to_bytes()
    }

    /// Run tool `tool` with `args`; returns what it did.
    pub fn run(&mut self, tool: &str, args: &Value) -> Result<Value> {
        let tool = tool.strip_prefix("docs_").unwrap_or(tool);
        let args = if args.is_null() {
            json!({})
        } else {
            args.clone()
        };
        if !args.is_object() {
            bail!("the arguments must be an object");
        }
        let mut out = match tool {
            "get_paragraphs" => super::docs::paragraphs(self.doc()?, &args)?,
            "insert_paragraphs" => {
                let d = self.doc()?;
                let (after, items) = super::docs::plan_insert(d, &args)?;
                let r = d
                    .insert_paragraphs(after, &items, SEL)
                    .map_err(anyhow::Error::msg)?;
                json!({"paragraphs": [r.start, r.end - 1]})
            }
            "replace_paragraph" => {
                let i = idx(&args, "index")?.context("`index` is required")?;
                let text = super::req_str(&args, "text")?;
                let r = self
                    .doc()?
                    .replace_paragraph_text(i, text, SEL)
                    .map_err(anyhow::Error::msg)?;
                json!({"paragraphs": [r.start, r.end - 1]})
            }
            "delete_paragraphs" => {
                let i = idx(&args, "index")?.context("`index` is required")?;
                let n = idx(&args, "count")?.unwrap_or(1);
                self.doc()?
                    .delete_paragraphs(i, n, SEL)
                    .map_err(anyhow::Error::msg)?;
                json!({"deleted": n})
            }
            "find_replace" | "format" => self.find_format(&args, tool == "format")?,
            "insert_blocks" => self.insert_blocks(&args)?,
            "replace_blocks" => {
                let (from, to) = self.range(&args)?;
                self.delete_range(from, to)?;
                let mut a = args.clone();
                if let Some(m) = a.as_object_mut() {
                    m.remove("from");
                    m.remove("to");
                    m.remove("heading");
                    if from == 0 {
                        m.insert("at_start".into(), json!(true));
                    } else {
                        m.insert("after".into(), json!(from - 1));
                    }
                }
                self.insert_blocks(&a)?
            }
            "delete_blocks" => {
                let (from, to) = self.range(&args)?;
                self.delete_range(from, to)?;
                json!({"deleted": [from, to]})
            }
            "move_section" => self.move_section(&args)?,
            "set_style" => {
                let (from, to) = self.range(&args)?;
                let name = super::req_str(&args, "style")?;
                let d = self.doc()?;
                let id = match d.find_style(name) {
                    Ok(id) => id,
                    Err(e) => {
                        // A built-in style the document lacks: add it.
                        if super::docs_look::builtin(name).is_none() {
                            bail!("{e}");
                        }
                        self.add_styles(
                            &args,
                            &[super::docs_look::builtin(name)
                                .map(|b| b.1)
                                .unwrap_or_default()],
                        )?;
                        self.doc()?.find_style(name).map_err(anyhow::Error::msg)?
                    }
                };
                let d = self.doc()?;
                d.set_style(super::docs_list::span(d, from, to), id.as_deref());
                json!({"paragraphs": [from, to], "style": name})
            }
            "restyle" => self.restyle(&args)?,
            "add_comment" => {
                let i = idx(&args, "paragraph")?.context("`paragraph` (its id) is required")?;
                let text = super::req_str(&args, "text")?;
                let author = opt_str_v(&args, "author")?
                    .map(str::to_owned)
                    .or_else(|| self.track.clone())
                    .unwrap_or_else(|| "Kantor".into());
                let d = self.doc()?;
                if i >= d.len() {
                    bail!("there is no paragraph {i} (the document has {})", d.len());
                }
                let sel = match opt_str_v(&args, "find")? {
                    Some(f) => {
                        let p = &d.paragraph(i).text;
                        let at = p
                            .find(f)
                            .with_context(|| format!("paragraph {i} doesn't contain \"{f}\""))?;
                        Selection {
                            anchor: Position::new(i, at),
                            head: Position::new(i, at + f.len()),
                        }
                    }
                    None => super::docs_list::span(d, i, i),
                };
                let id = d
                    .add_comment(sel, text, &author)
                    .context("can't comment there")?;
                json!({"comment": id})
            }
            "resolve_comment" => {
                let id = super::req_str(&args, "id")?;
                let done = super::opt_bool(&args, "done")?.unwrap_or(true);
                if !self.doc()?.set_comment_done(id, done, SEL) {
                    bail!("there is no comment {id}");
                }
                json!({"comment": id, "resolved": done})
            }
            "accept_changes" | "reject_changes" => {
                let accept = tool == "accept_changes";
                let has_range = args.get("from").is_some() || args.get("heading").is_some();
                let range = if has_range {
                    Some(self.range(&args)?)
                } else {
                    None
                };
                let d = self.doc()?;
                let before = d.changes().len();
                match range {
                    None => {
                        d.resolve_all(accept, SEL);
                    }
                    Some((a, b)) => {
                        let s = super::docs_list::span(d, a, b);
                        if accept {
                            d.accept_change(s);
                        } else {
                            d.reject_change(s);
                        }
                    }
                }
                let after = self.doc()?.changes().len();
                json!({"resolved": before.saturating_sub(after), "left": after})
            }
            "update_fields" => {
                let d = self.doc()?;
                let mut engine = docs_layout::Engine::new();
                super::docs_author::update_all_fields(
                    d,
                    &mut engine,
                    chrono::Local::now().naive_local(),
                );
                json!({"fields": d.fields().len()})
            }
            "set_header" | "set_footer" => self.set_hf(
                &args,
                if tool == "set_header" {
                    "header"
                } else {
                    "footer"
                },
            )?,
            "page_setup" => self.page_setup(&args)?,
            other => bail!(
                "{other} is not a docs editing tool (one of {})",
                TOOLS.join(", ")
            ),
        };
        if let Some(m) = out.as_object_mut() {
            m.insert("done".into(), json!(tool));
        }
        Ok(out)
    }

    /// Paragraphs `from..=to` from `from`/`to` ids or a `heading`'s
    /// section, widened to whole tables.
    fn range(&mut self, args: &Value) -> Result<(usize, usize)> {
        let d = self.doc()?;
        let (mut a, mut b) = match (opt_str_v(args, "heading")?, idx(args, "from")?) {
            (Some(h), _) => {
                let i = find_heading(d, h)?;
                (i, section_end(d, i) - 1)
            }
            (None, Some(f)) => (f, idx(args, "to")?.unwrap_or(f)),
            (None, None) => bail!("give `from` (and `to`) paragraph ids, or a `heading`"),
        };
        if b < a {
            bail!("`to` comes before `from`");
        }
        if b >= d.len() {
            bail!("there is no paragraph {b} (the document has {})", d.len());
        }
        if let Some(c) = d.paragraph(a).cell {
            a = d.table_range(c.table).start;
        }
        if let Some(c) = d.paragraph(b).cell {
            b = d.table_range(c.table).end - 1;
        }
        Ok((a, b))
    }

    fn delete_range(&mut self, a: usize, b: usize) -> Result<()> {
        let d = self.doc()?;
        let n = d.len();
        let sel = if b + 1 < n {
            Selection {
                anchor: Position::new(a, 0),
                head: Position::new(b + 1, 0),
            }
        } else if a > 0 {
            Selection {
                anchor: Position::new(a - 1, d.paragraph(a - 1).text.len()),
                head: Position::new(b, d.paragraph(b).text.len()),
            }
        } else {
            super::docs_list::span(d, a, b)
        };
        d.delete_selection(sel);
        Ok(())
    }

    /// The look inserted blocks and new styles take (`preset`, `accent`,
    /// `fonts`, `palette` in the arguments; default `report`).
    fn look(args: &Value) -> Result<Look> {
        let mut m = Map::new();
        for k in ["preset", "accent", "fonts", "palette", "labels"] {
            if let Some(v) = args.get(k) {
                m.insert(k.into(), v.clone());
            }
        }
        let lang = opt_str_v(args, "lang")?.unwrap_or("en-US");
        Look::read(&look_json(&m)?, lang)
    }

    /// Add style definitions `ids` from the arguments' look.
    fn add_styles(&mut self, args: &Value, ids: &[&str]) -> Result<()> {
        let look = Self::look(args)?;
        let mut pkg = self.pkg()?;
        let part = pkg.document_part()?;
        let styles_part = pkg
            .related(&part, "styles")
            .context("the document has no styles part")?;
        let mut st = pkg.text(&styles_part).unwrap_or_default();
        let wanted = ids.iter().map(|s| s.to_string()).collect();
        let add = missing_styles(&look, &st, &wanted);
        insert_before(&mut st, "</w:styles>", &add);
        pkg.set_text(&styles_part, st);
        self.set_pkg(&pkg)
    }

    /// Where `insert_blocks` puts its blocks: after paragraph id (`None`:
    /// at the start).
    fn place(&mut self, args: &Value) -> Result<Option<usize>> {
        let d = self.doc()?;
        let n = d.len();
        if opt_bool_v(args, "at_start")? == Some(true) {
            return Ok(None);
        }
        if let Some(i) = idx(args, "after")? {
            if i >= n {
                bail!("there is no paragraph {i} (the document has {n})");
            }
            return Ok(Some(i));
        }
        if let Some(i) = idx(args, "before")? {
            if i >= n {
                bail!("there is no paragraph {i} (the document has {n})");
            }
            return Ok(i.checked_sub(1));
        }
        if let Some(h) = opt_str_v(args, "after_heading")? {
            return Ok(Some(find_heading(d, h)?));
        }
        if let Some(h) = opt_str_v(args, "before_heading")? {
            return Ok(find_heading(d, h)?.checked_sub(1));
        }
        if let Some(h) = opt_str_v(args, "end_of_section")? {
            let i = find_heading(d, h)?;
            return Ok(Some(section_end(d, i) - 1));
        }
        Ok(Some(n - 1))
    }

    fn insert_blocks(&mut self, args: &Value) -> Result<Value> {
        let o = object(
            args,
            "insert_blocks' arguments",
            &[
                "blocks",
                "after",
                "before",
                "after_heading",
                "before_heading",
                "end_of_section",
                "at_start",
                "at_end",
                "preset",
                "accent",
                "fonts",
                "palette",
                "labels",
                "lang",
                "markdown",
            ],
        )?;
        let blocks: Vec<Map<String, Value>> = get(o, "blocks")
            .and_then(Value::as_array)
            .context("`blocks` must be an array of blocks (the build JSON's)")?
            .iter()
            .enumerate()
            .map(|(i, b)| normalize(b).with_context(|| format!("block {}", i + 1)))
            .collect::<Result<_>>()?;
        if blocks.is_empty() || blocks.len() > super::docs_author::MAX_BLOCKS {
            bail!("give 1 to {} blocks", super::docs_author::MAX_BLOCKS);
        }
        let look = Self::look(args)?;
        let md = opt_bool(o, "markdown")?.unwrap_or(true);
        let after = self.place(args)?;
        let before_len = self.doc()?.len();
        let r = {
            let d = self.doc()?;
            d.insert_paragraphs_untracked(
                after,
                &[NewParagraph {
                    text: MARKER.into(),
                    style: None,
                    list: None,
                    level: 0,
                }],
                SEL,
            )
            .map_err(anyhow::Error::msg)?
        };
        let at = r.start;
        let pkg = self.pkg()?;
        let date = now_iso();
        let track = self.track.clone();
        let mut w = Writer::for_edit(
            &look,
            pkg,
            self.base.as_deref(),
            md,
            track.as_deref().map(|a| (a, date.as_str())),
        )?;
        w.props = read_props(&w.pkg);
        w.collect(&blocks)?;
        for (i, b) in blocks.iter().enumerate() {
            w.block(b).with_context(|| format!("block {}", i + 1))?;
        }
        let body = w.body_xml();
        let warnings = std::mem::take(&mut w.warnings);
        let needs_update = body.contains("SEQ ") || body.contains("REF ") || body.contains("TOC ");
        let mut pkg = finish_edit(w)?;
        let part = pkg.document_part()?;
        let mut xml = pkg.text(&part).context("no document part")?;
        let (s, e) = marker_para(&xml).context("lost the insertion point")?;
        xml.replace_range(s..e, &body);
        pkg.set_text(&part, xml);
        self.set_pkg(&pkg)?;
        let d = self.doc()?;
        let added = d.len().saturating_sub(before_len);
        let last = (at + added.max(1) - 1).min(d.len() - 1);
        if needs_update {
            let cx = docs_engine::FieldContext {
                now: chrono::Local::now().naive_local(),
                offset: chrono::Offset::fix(&chrono::Utc),
            };
            let mut engine = docs_layout::Engine::new();
            let sel = super::docs_list::span(d, at.min(d.len() - 1), last);
            d.update_fields(sel, &cx, &mut |x| docs_pdf::field_pages(x, &mut engine));
        }
        Ok(json!({"paragraphs": [at, last], "notes": warnings}))
    }

    fn move_section(&mut self, args: &Value) -> Result<Value> {
        if self.track.is_some() {
            bail!("move_section can't be a tracked change yet: run it without --track");
        }
        let o = object(
            args,
            "move_section's arguments",
            &["heading", "before_heading", "after_section", "to_end"],
        )?;
        let h = opt_str(o, "heading")?.context("`heading` names the section to move")?;
        let d = self.doc()?;
        let a = find_heading(d, h)?;
        let b = section_end(d, a);
        let dest = if let Some(t) = opt_str(o, "before_heading")? {
            find_heading(d, t)?
        } else if let Some(t) = opt_str(o, "after_section")? {
            let i = find_heading(d, t)?;
            section_end(d, i)
        } else if opt_bool(o, "to_end")? == Some(true) {
            d.len()
        } else {
            bail!("say where: `before_heading`, `after_section` or `to_end`");
        };
        if dest > a && dest < b {
            bail!("the section can't move inside itself");
        }
        if dest == a || dest == b {
            return Ok(json!({"moved": 0}));
        }
        // Markers, highest place first so the lower ids hold.
        let mut places = vec![(b, 'B'), (a, 'A'), (dest, 'C')];
        places.sort_by_key(|x| std::cmp::Reverse(x.0));
        for (at, tag) in &places {
            let d = self.doc()?;
            d.insert_paragraphs_untracked(
                at.checked_sub(1),
                &[NewParagraph {
                    text: format!("{MARKER}{tag}"),
                    style: None,
                    list: None,
                    level: 0,
                }],
                SEL,
            )
            .map_err(anyhow::Error::msg)?;
        }
        let mut pkg = self.pkg()?;
        let part = pkg.document_part()?;
        let xml = pkg.text(&part).context("no document part")?;
        let find =
            |t: char| marker_para_tagged(&xml, &format!("{MARKER}{t}")).context("lost a marker");
        let (a0, a1) = find('A')?;
        let (b0, b1) = find('B')?;
        let (c0, c1) = find('C')?;
        let moved = xml[a1..b0].to_owned();
        let mut pieces: Vec<(usize, usize, String)> =
            vec![(a0, b1, String::new()), (c0, c1, moved)];
        pieces.sort_by_key(|p| std::cmp::Reverse(p.0));
        let mut out = xml.clone();
        for (s, e, with) in pieces {
            out.replace_range(s..e, &with);
        }
        pkg.set_text(&part, out);
        self.set_pkg(&pkg)?;
        Ok(json!({"moved": b - a}))
    }

    fn find_format(&mut self, args: &Value, format_only: bool) -> Result<Value> {
        let find = super::req_str(args, "find")?.to_owned();
        if find.is_empty() {
            bail!("`find` is empty");
        }
        let with = if format_only {
            None
        } else {
            Some(super::req_str(args, "replace")?.to_owned())
        };
        let opts = kantor_core::find::FindOptions {
            match_case: super::opt_bool(args, "match_case")?.unwrap_or(false),
            whole_word: super::opt_bool(args, "whole_word")?.unwrap_or(false),
        };
        let o = args.as_object().context("arguments")?;
        let fmt_keys = [
            "bold",
            "italic",
            "underline",
            "color",
            "size",
            "font",
            "style",
            "highlight",
        ];
        let wants_fmt = fmt_keys.iter().any(|k| o.contains_key(*k));
        if format_only && !wants_fmt {
            bail!("format needs a formatting key: {}", fmt_keys.join(", "));
        }
        // The formatting to apply, as the docs engine's run properties.
        let look = Self::look(args)?;
        let (rf, _) = look.formats(o)?;
        let char_style = match opt_str(o, "style")? {
            Some(s) => {
                let d = self.doc()?;
                let id = d
                    .styles
                    .styles
                    .iter()
                    .find(|st| {
                        st.kind == docs_engine::style::StyleKind::Character
                            && (st.name.eq_ignore_ascii_case(s) || st.id.eq_ignore_ascii_case(s))
                    })
                    .map(|st| st.id.clone())
                    .with_context(|| format!("no character style \"{s}\" in the document"))?;
                Some(id)
            }
            None => None,
        };
        if !wants_fmt && with.is_some() {
            let d = self.doc()?;
            let (n, _) = super::docs::find_replace(d, SEL, args)?;
            return Ok(json!({"replaced": n}));
        }
        let with = with.as_deref().map(super::docs::replacement);
        let n = super::docs::in_parts(self.doc()?, args, |d, _| {
            let ranges: Vec<(usize, std::ops::Range<usize>)> = d
                .paragraphs()
                .iter()
                .enumerate()
                .flat_map(|(i, p)| {
                    kantor_core::find::find_all(&p.text, &find, opts)
                        .into_iter()
                        .map(move |r| (i, r))
                })
                .collect();
            let n = ranges.len();
            for (i, r) in ranges.into_iter().rev() {
                let (start, end) = match &with {
                    Some(w) => {
                        d.replace_ranges(&[(i, r.clone())], w, SEL);
                        // Tracked: the deletion stays before the new text.
                        let p = &d.paragraph(i).text;
                        let s = p[r.start..]
                            .find(w.as_str())
                            .map_or(r.start, |k| r.start + k);
                        (s, s + w.len())
                    }
                    None => (r.start, r.end),
                };
                let sel = Selection {
                    anchor: Position::new(i, start),
                    head: Position::new(i, end),
                };
                let rf = rf.clone();
                let cs = char_style.clone();
                d.format(sel, move |p| apply_run(p, &rf, cs.as_deref()));
            }
            Ok(n)
        })?;
        let key = if with.is_some() {
            "replaced"
        } else {
            "formatted"
        };
        let mut m = Map::new();
        m.insert(key.into(), json!(n));
        Ok(Value::Object(m))
    }

    fn restyle(&mut self, args: &Value) -> Result<Value> {
        let o = args.as_object().context("arguments")?;
        let name = opt_str(o, "style")?
            .context("`style` names the style to change")?
            .to_owned();
        let mut keys: Vec<&str> = STYLE_KEYS.to_vec();
        keys.extend(["style", "preset", "accent", "fonts", "palette", "lang"]);
        object(args, "restyle's arguments", &keys)?;
        let look = Self::look(args)?;
        let mut def = o.clone();
        for k in [
            "style", "preset", "accent", "fonts", "palette", "lang", "type", "based_on", "next",
        ] {
            def.remove(k);
        }
        let (rf, pf) = look.formats(&def)?;
        let found = self.doc()?.find_style(&name);
        let mut pkg = self.pkg()?;
        let part = pkg.document_part()?;
        let sp = pkg
            .related(&part, "styles")
            .context("the document has no styles part")?;
        let mut st = pkg.text(&sp).unwrap_or_default();
        let id = match found {
            Ok(Some(id)) => id,
            Ok(None) => style_id_by_name(&st, "Normal").unwrap_or_else(|| "Normal".into()),
            Err(_) => {
                // A character style, or a new style.
                match style_id_by_name(&st, &name) {
                    Some(id) => id,
                    None => {
                        let kind = opt_str(o, "type")?.unwrap_or("paragraph");
                        if !["paragraph", "character"].contains(&kind) {
                            bail!("`type` must be paragraph or character");
                        }
                        let id = super::docs_look::custom_id(&name);
                        let based = match opt_str(o, "based_on")? {
                            Some(b) => style_id_by_name(&st, b)
                                .with_context(|| format!("no style \"{b}\" to base it on"))?,
                            None if kind == "paragraph" => {
                                style_id_by_name(&st, "Normal").unwrap_or_else(|| "Normal".into())
                            }
                            None => "DefaultParagraphFont".into(),
                        };
                        let el = format!(
                            "<w:style w:type=\"{kind}\" w:customStyle=\"1\" w:styleId=\"{}\"><w:name w:val=\"{}\"/><w:basedOn w:val=\"{}\"/><w:qFormat/></w:style>",
                            esc(&id),
                            esc(&name),
                            esc(&based)
                        );
                        insert_before(&mut st, "</w:styles>", &el);
                        id
                    }
                }
            }
        };
        let (s, e) =
            style_range(&st, &id).with_context(|| format!("no style {id} in styles.xml"))?;
        let el = st[s..e].to_owned();
        let new = restyled(&el, &rf, &pf);
        st.replace_range(s..e, &new);
        pkg.set_text(&sp, st);
        self.set_pkg(&pkg)?;
        Ok(json!({"style": name, "id": id}))
    }

    fn set_hf(&mut self, args: &Value, kind: &str) -> Result<Value> {
        let mut keys = super::docs_look::HF_KEYS.to_vec();
        keys.extend([
            "section", "none", "preset", "accent", "fonts", "palette", "lang",
        ]);
        let o = object(args, &format!("set_{kind}'s arguments"), &keys)?;
        let spec = if opt_bool(o, "none")? == Some(true) {
            None
        } else {
            let mut m = Map::new();
            for k in super::docs_look::HF_KEYS {
                if let Some(v) = o.get(*k) {
                    m.insert((*k).into(), v.clone());
                }
            }
            hf_spec(Some(&Value::Object(m)), kind)?
        };
        let section = idx(args, "section")?;
        let look = Self::look(args)?;
        let pkg = self.pkg()?;
        let mut w = Writer::for_edit(&look, pkg, None, false, None)?;
        w.props = read_props(&w.pkg);
        let part = w.pkg.document_part()?;
        let xml = w.pkg.text(&part).context("no document part")?;
        let sects = sect_prs(&xml);
        if sects.is_empty() {
            bail!("the document has no sections");
        }
        if let Some(n) = section
            && (n == 0 || n > sects.len())
        {
            bail!("there is no section {n} (the document has {})", sects.len());
        }
        let mut out = xml.clone();
        let mut changed = 0;
        for (k, (s, e)) in sects.iter().enumerate().rev() {
            if section.is_some_and(|n| n != k + 1) {
                continue;
            }
            let sp = &xml[*s..*e];
            let width = sect_width(sp);
            let rel = w.hf_rel(kind, spec.as_ref(), width);
            let tag = if kind == "header" {
                "headerReference"
            } else {
                "footerReference"
            };
            let new = with_reference(sp, tag, &rel);
            out.replace_range(*s..*e, &new);
            changed += 1;
        }
        let mut pkg = std::mem::take(&mut w.pkg);
        pkg.set_text(&part, out);
        let missing = missing_styles(
            &look,
            &pkg.text(&styles_of(&pkg)?).unwrap_or_default(),
            &w.used_styles,
        );
        if !missing.is_empty() {
            let sp = styles_of(&pkg)?;
            let mut st = pkg.text(&sp).unwrap_or_default();
            insert_before(&mut st, "</w:styles>", &missing);
            pkg.set_text(&sp, st);
        }
        self.set_pkg(&pkg)?;
        Ok(json!({"sections": changed}))
    }

    fn page_setup(&mut self, args: &Value) -> Result<Value> {
        let mut keys = super::docs_look::PAGE_KEYS.to_vec();
        keys.push("section");
        let o = object(args, "page_setup's arguments", &keys)?;
        let section = idx(args, "section")?;
        let mut po = o.clone();
        po.remove("section");
        let pv = Value::Object(po);
        let mut pkg = self.pkg()?;
        let part = pkg.document_part()?;
        let xml = pkg.text(&part).context("no document part")?;
        let sects = sect_prs(&xml);
        if let Some(n) = section
            && (n == 0 || n > sects.len())
        {
            bail!("there is no section {n} (the document has {})", sects.len());
        }
        let mut out = xml.clone();
        let mut changed = 0;
        for (k, (s, e)) in sects.iter().enumerate().rev() {
            if section.is_some_and(|n| n != k + 1) {
                continue;
            }
            let sp = &xml[*s..*e];
            let cur = sect_page(sp);
            let p = page_spec(&pv, cur)?;
            out.replace_range(*s..*e, &with_page(sp, &p));
            changed += 1;
        }
        pkg.set_text(&part, out);
        self.set_pkg(&pkg)?;
        Ok(json!({"sections": changed}))
    }
}

fn opt_str_v<'a>(v: &'a Value, k: &str) -> Result<Option<&'a str>> {
    super::opt_str(v, k)
}

fn opt_bool_v(v: &Value, k: &str) -> Result<Option<bool>> {
    super::opt_bool(v, k)
}

/// `rf` (and a character style) on a run's properties.
fn apply_run(p: &mut docs_engine::RunProps, rf: &RunFmt, style: Option<&str>) {
    if let Some(b) = rf.bold {
        docs_engine::Toggle::Bold.set(p, b);
    }
    if let Some(i) = rf.italic {
        docs_engine::Toggle::Italic.set(p, i);
    }
    if let Some(u) = rf.underline {
        docs_engine::Toggle::Underline.set(p, u);
    }
    if let Some(c) = &rf.color {
        p.color = docs_engine::Rgb::parse(c);
        p.extras.remove("color");
    }
    if let Some(s) = rf.size {
        p.size = Some(s);
        p.extras.remove("sz");
    }
    if let Some(f) = &rf.font {
        p.fonts = docs_engine::Fonts::family(f);
    }
    if let Some(h) = &rf.highlight {
        p.extras.remove("highlight");
        p.extras.push(&format!("<w:highlight w:val=\"{h}\"/>"));
    }
    if let Some(s) = style {
        p.style = Some(s.to_owned());
    }
}

/// Finish a writer that added blocks to a package: its list definitions,
/// footnotes and the styles the package lacks go in.
pub fn finish_edit(mut w: Writer) -> Result<Pkg> {
    let look = w.look;
    let mut pkg = std::mem::take(&mut w.pkg);
    let part = pkg.document_part()?;
    // Numbering.
    let numbering = match pkg.related(&part, "numbering") {
        Some(n) => n,
        None => {
            let name = "word/numbering.xml".to_owned();
            pkg.set_text(
                &name,
                format!(
                    "{}<w:numbering xmlns:w=\"{}\"></w:numbering>",
                    super::docs_pkg::XML_DECL,
                    super::docs_pkg::NS_W
                ),
            );
            pkg.set_override(&name, &wml_type("numbering"));
            pkg.add_rel(
                &part,
                &rel_type("numbering"),
                &super::docs_pkg::relative(&part, &name),
                false,
                "rId",
            );
            name
        }
    };
    let mut nx = pkg.text(&numbering).unwrap_or_default();
    let abs = format!(
        "{}{}",
        super::docs_look::abstract_list(w.nums.bullets_abs, true, &look.palette["bullet"]),
        super::docs_look::abstract_list(w.nums.numbers_abs, false, &look.palette["ink"])
    );
    let mut nums = format!(
        "<w:num w:numId=\"{}\"><w:abstractNumId w:val=\"{}\"/></w:num>",
        w.nums.bullets, w.nums.bullets_abs
    );
    for (id, start) in &w.lists {
        nums.push_str(&super::docs_look::num_xml(*id, w.nums.numbers_abs, *start));
    }
    match nx.find("<w:num ") {
        Some(i) => nx.insert_str(i, &abs),
        None => insert_before(&mut nx, "</w:numbering>", &abs),
    }
    if nx.trim_end().ends_with("/>") && !nx.contains("</w:numbering>") {
        let i = nx.rfind("/>").unwrap_or(nx.len());
        nx.replace_range(i.., "></w:numbering>");
    }
    insert_before(&mut nx, "</w:numbering>", &nums);
    pkg.set_text(&numbering, nx);
    // Footnotes.
    if !w.notes.is_empty() {
        match pkg.related(&part, "footnotes") {
            Some(f) => {
                let mut t = pkg.text(&f).unwrap_or_default();
                insert_before(&mut t, "</w:footnotes>", &w.notes.concat());
                pkg.set_text(&f, t);
            }
            None => {
                let name = "word/footnotes.xml";
                pkg.set_text(name, super::docs_author::footnotes_xml(&w.notes));
                pkg.set_override(name, &wml_type("footnotes"));
                pkg.add_rel(
                    &part,
                    &rel_type("footnotes"),
                    &super::docs_pkg::relative(&part, name),
                    false,
                    "rId",
                );
            }
        }
    }
    // Styles.
    let sp = styles_of(&pkg)?;
    let mut st = pkg.text(&sp).unwrap_or_default();
    let add = missing_styles(look, &st, &w.used_styles);
    insert_before(&mut st, "</w:styles>", &add);
    pkg.set_text(&sp, st);
    Ok(pkg)
}

fn styles_of(pkg: &Pkg) -> Result<String> {
    let part = pkg.document_part()?;
    pkg.related(&part, "styles")
        .context("the document has no styles part")
}

/// Document properties from `docProps` (title, company…).
fn read_props(pkg: &Pkg) -> Props {
    let core = pkg.text("docProps/core.xml").unwrap_or_default();
    let app = pkg.text("docProps/app.xml").unwrap_or_default();
    let el = |src: &str, name: &str| -> String {
        let open = format!("<{name}>");
        src.find(&open)
            .and_then(|i| {
                let s = &src[i + open.len()..];
                s.find("</").map(|e| s[..e].to_owned())
            })
            .map(|t| docs_engine::xml::inner_text(&format!("<x><t>{t}</t></x>"), &["t"]))
            .unwrap_or_default()
    };
    Props {
        title: el(&core, "dc:title"),
        subject: el(&core, "dc:subject"),
        author: el(&core, "dc:creator"),
        company: el(&app, "Company"),
        ..Props::default()
    }
}

/// The `<w:p>…</w:p>` holding `MARKER` (exactly).
fn marker_para(xml: &str) -> Option<(usize, usize)> {
    marker_para_tagged(xml, MARKER)
}

fn marker_para_tagged(xml: &str, text: &str) -> Option<(usize, usize)> {
    let pat = format!(">{text}<");
    let i = xml.find(&pat)?;
    let s = [xml[..i].rfind("<w:p>"), xml[..i].rfind("<w:p ")]
        .into_iter()
        .flatten()
        .max()?;
    let e = xml[i..].find("</w:p>")? + i + "</w:p>".len();
    if xml[s..i].contains("</w:p>") {
        return None;
    }
    Some((s, e))
}

/// The style element with id `id`: its byte range.
fn style_range(st: &str, id: &str) -> Option<(usize, usize)> {
    let i = st.find(&format!("w:styleId=\"{}\"", esc(id)))?;
    let s = st[..i].rfind("<w:style ")?;
    let e = st[i..].find("</w:style>")? + i + "</w:style>".len();
    Some((s, e))
}

/// The id of the style named `name` (or with that id).
fn style_id_by_name(st: &str, name: &str) -> Option<String> {
    let squash = |s: &str| s.replace(' ', "").to_ascii_lowercase();
    let want = squash(name);
    let mut rest = st;
    while let Some(i) = rest.find("<w:style ") {
        let end = rest[i..].find("</w:style>").map_or(rest.len(), |e| i + e);
        let el = &rest[i..end];
        let tag_end = el.find('>').unwrap_or(el.len());
        let id = attr_of(&el[..tag_end], "w:styleId").unwrap_or_default();
        let nm = el
            .find("<w:name ")
            .and_then(|k| attr_of(&el[k..k + el[k..].find('>').unwrap_or(0)], "w:val"))
            .unwrap_or_default();
        if squash(&id) == want || squash(&nm) == want {
            return Some(id);
        }
        rest = &rest[end..];
    }
    None
}

/// Top-level child elements of a fragment.
pub fn children(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let b = xml.as_bytes();
    let mut i = 0;
    while let Some(k) = xml[i..].find('<').map(|k| k + i) {
        let start = k;
        let mut depth = 0i32;
        let mut j = k;
        loop {
            let Some(lt) = xml[j..].find('<').map(|x| x + j) else {
                return out;
            };
            let gt = match xml[lt..].find('>') {
                Some(g) => lt + g,
                None => return out,
            };
            let closing = b.get(lt + 1) == Some(&b'/');
            let selfclose = b[gt - 1] == b'/';
            if closing {
                depth -= 1;
            } else if !selfclose {
                depth += 1;
            }
            j = gt + 1;
            if depth <= 0 {
                break;
            }
        }
        out.push(xml[start..j].to_owned());
        i = j;
    }
    out
}

/// The inside of element `name` (`w:rPr`) in `el`, and its range.
fn inner_of(el: &str, name: &str) -> Option<(usize, usize, String)> {
    let open = format!("<{name}>");
    if let Some(i) = el.find(&open) {
        let close = format!("</{name}>");
        let e = el[i..].find(&close)? + i;
        return Some((i, e + close.len(), el[i + open.len()..e].to_owned()));
    }
    let empty = format!("<{name}/>");
    let i = el.find(&empty)?;
    Some((i, i + empty.len(), String::new()))
}

/// Attributes of `<w:spacing …/>`-like elements merged: `new`'s win.
fn merge_attrs(old: &str, new: &str) -> String {
    let attrs = |t: &str| -> Vec<(String, String)> {
        let body = t
            .trim_start_matches('<')
            .trim_end_matches('>')
            .trim_end_matches('/');
        let mut v = Vec::new();
        let mut rest = body.split_once(char::is_whitespace).map_or("", |(_, r)| r);
        while let Some(eq) = rest.find("=\"") {
            let key = rest[..eq].trim().to_owned();
            let after = &rest[eq + 2..];
            let Some(end) = after.find('"') else { break };
            v.push((key, after[..end].to_owned()));
            rest = &after[end + 1..];
        }
        v
    };
    let name = docs_engine::model::element_name(new);
    let mut a = attrs(old);
    for (k, val) in attrs(new) {
        match a.iter_mut().find(|(x, _)| *x == k) {
            Some(slot) => slot.1 = val,
            None => a.push((k, val)),
        }
    }
    // hanging and firstLine exclude each other.
    if new.contains("w:hanging=") {
        a.retain(|(k, _)| k != "w:firstLine");
    }
    if new.contains("w:firstLine=") {
        a.retain(|(k, _)| k != "w:hanging");
    }
    let mut out = format!("<w:{name}");
    for (k, v) in a {
        out.push_str(&format!(" {k}=\"{v}\""));
    }
    out.push_str("/>");
    out
}

/// Property elements `new` over `old` (same names replaced; spacing and
/// indents merged), in schema order.
fn overlay_props(old: &str, new: &str, order: &[&str]) -> String {
    let mut have = children(old);
    for n in children(new) {
        let name = docs_engine::model::element_name(&n).to_owned();
        match have
            .iter()
            .position(|h| docs_engine::model::element_name(h) == name)
        {
            Some(i) if matches!(name.as_str(), "spacing" | "ind") => {
                have[i] = merge_attrs(&have[i], &n);
            }
            Some(i) => {
                have[i] = n;
                // b with bCs, i with iCs, sz with szCs: handled as their own names.
            }
            None => have.push(n),
        }
    }
    docs_engine::xml::ordered(order, have)
}

/// A style element with `rf` and `pf` over its properties.
fn restyled(el: &str, rf: &RunFmt, pf: &super::docs_look::ParaFmt) -> String {
    let mut el = el.to_owned();
    let pnew = pf.children(None);
    if !pnew.is_empty() {
        let merged = match inner_of(&el, "w:pPr") {
            Some((s, e, old)) => {
                let m = overlay_props(&old, &pnew, docs_engine::xml::PPR_ORDER);
                el.replace_range(s..e, &format!("<w:pPr>{m}</w:pPr>"));
                true
            }
            None => false,
        };
        if !merged {
            let ins = format!("<w:pPr>{pnew}</w:pPr>");
            let at = el
                .find("<w:rPr")
                .or_else(|| el.find("<w:tblPr"))
                .unwrap_or_else(|| el.rfind("</w:style>").unwrap_or(el.len()));
            el.insert_str(at, &ins);
        }
    }
    let rnew = rf.children();
    if !rnew.is_empty() {
        match inner_of(&el, "w:rPr") {
            Some((s, e, old)) => {
                let m = overlay_props(&old, &rnew, docs_engine::xml::RPR_ORDER);
                el.replace_range(s..e, &format!("<w:rPr>{m}</w:rPr>"));
            }
            None => {
                let ins = format!("<w:rPr>{rnew}</w:rPr>");
                let at = el
                    .find("<w:tblPr")
                    .unwrap_or_else(|| el.rfind("</w:style>").unwrap_or(el.len()));
                el.insert_str(at, &ins);
            }
        }
    }
    el
}

/// The byte ranges of the document's `w:sectPr` elements, in order.
fn sect_prs(xml: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(k) = xml[i..].find("<w:sectPr").map(|k| k + i) {
        let after = xml.as_bytes().get(k + 9).copied();
        if !matches!(after, Some(b'>') | Some(b' ') | Some(b'/')) {
            i = k + 9;
            continue;
        }
        let tag_end = match xml[k..].find('>') {
            Some(g) => k + g + 1,
            None => break,
        };
        let e = if xml[..tag_end].ends_with("/>") {
            tag_end
        } else {
            match xml[k..].find("</w:sectPr>") {
                Some(c) => k + c + "</w:sectPr>".len(),
                None => break,
            }
        };
        out.push((k, e));
        i = e;
    }
    out
}

fn num_attr(tag: &str, name: &str) -> Option<i64> {
    attr_of(tag, name)?.parse().ok()
}

fn tag_of<'a>(sp: &'a str, el: &str) -> Option<&'a str> {
    let i = sp
        .find(&format!("<{el} "))
        .or_else(|| sp.find(&format!("<{el}/")))?;
    let e = sp[i..].find('>')? + i + 1;
    Some(&sp[i..e])
}

/// A section's page setup.
fn sect_page(sp: &str) -> super::docs_look::PageSpec {
    let mut p = super::docs_look::A4;
    if let Some(t) = tag_of(sp, "w:pgSz") {
        p.width = num_attr(t, "w:w").unwrap_or(p.width);
        p.height = num_attr(t, "w:h").unwrap_or(p.height);
        p.landscape = attr_of(t, "w:orient").as_deref() == Some("landscape") || p.width > p.height;
    }
    if let Some(t) = tag_of(sp, "w:pgMar") {
        p.top = num_attr(t, "w:top").unwrap_or(p.top).abs();
        p.bottom = num_attr(t, "w:bottom").unwrap_or(p.bottom).abs();
        p.left = num_attr(t, "w:left").unwrap_or(p.left);
        p.right = num_attr(t, "w:right").unwrap_or(p.right);
        p.header = num_attr(t, "w:header").unwrap_or(p.header);
        p.footer = num_attr(t, "w:footer").unwrap_or(p.footer);
    }
    p
}

fn sect_width(sp: &str) -> i64 {
    sect_page(sp).text_width()
}

/// A sectPr with `p`'s size and margins.
fn with_page(sp: &str, p: &super::docs_look::PageSpec) -> String {
    let sz = format!(
        "<w:pgSz w:w=\"{}\" w:h=\"{}\"{}/>",
        p.width,
        p.height,
        if p.landscape {
            " w:orient=\"landscape\""
        } else {
            ""
        }
    );
    let gutter = tag_of(sp, "w:pgMar")
        .and_then(|t| num_attr(t, "w:gutter"))
        .unwrap_or(0);
    let mar = format!(
        "<w:pgMar w:top=\"{}\" w:right=\"{}\" w:bottom=\"{}\" w:left=\"{}\" w:header=\"{}\" w:footer=\"{}\" w:gutter=\"{gutter}\"/>",
        p.top, p.right, p.bottom, p.left, p.header, p.footer
    );
    let mut out = sp.to_owned();
    let mut put = |el: &str, new: &str, before: &[&str]| {
        if let Some(t) = tag_of(&out, el).map(str::to_owned) {
            out = out.replacen(&t, new, 1);
        } else {
            let at = before
                .iter()
                .filter_map(|b| out.find(&format!("<{b}")))
                .min()
                .unwrap_or_else(|| out.rfind("</w:sectPr>").unwrap_or(out.len()));
            out.insert_str(at, new);
        }
    };
    put(
        "w:pgSz",
        &sz,
        &[
            "w:pgMar",
            "w:paperSrc",
            "w:pgBorders",
            "w:lnNumType",
            "w:pgNumType",
            "w:cols",
            "w:titlePg",
            "w:docGrid",
        ],
    );
    put(
        "w:pgMar",
        &mar,
        &[
            "w:paperSrc",
            "w:pgBorders",
            "w:lnNumType",
            "w:pgNumType",
            "w:cols",
            "w:titlePg",
            "w:docGrid",
        ],
    );
    if out.ends_with("/>") && out.starts_with("<w:sectPr") && !out.contains("</w:sectPr>") {
        // `<w:sectPr/>` can't hold children: rebuild it.
        out = format!("<w:sectPr>{sz}{mar}</w:sectPr>");
    }
    out
}

/// A sectPr whose default `tag` (headerReference / footerReference)
/// points at `rel`.
fn with_reference(sp: &str, tag: &str, rel: &str) -> String {
    let mut out = if sp.ends_with("/>") && !sp.contains("</w:sectPr>") {
        "<w:sectPr></w:sectPr>".to_owned()
    } else {
        sp.to_owned()
    };
    // Drop the old default reference.
    let mut rest_from = 0;
    while let Some(i) = out[rest_from..]
        .find(&format!("<w:{tag} "))
        .map(|i| i + rest_from)
    {
        let e = out[i..].find("/>").map_or(out.len(), |e| i + e + 2);
        let t = &out[i..e];
        let ty = attr_of(t, "w:type").unwrap_or_else(|| "default".into());
        if ty == "default" {
            out.replace_range(i..e, "");
        } else {
            rest_from = e;
        }
    }
    let new = format!("<w:{tag} w:type=\"default\" r:id=\"{rel}\"/>");
    // Headers before footers, both first in the sectPr.
    let at = if tag == "headerReference" {
        out.find('>').map_or(0, |g| g + 1)
    } else {
        out.find("<w:footerReference")
            .or_else(|| {
                out.rfind("<w:headerReference")
                    .and_then(|h| out[h..].find("/>").map(|e| h + e + 2))
            })
            .unwrap_or_else(|| out.find('>').map_or(0, |g| g + 1))
    };
    out.insert_str(at, &new);
    out
}

#[cfg(test)]
mod tests {
    use super::super::docs_list::shown;
    use super::*;

    fn session(blocks: Value) -> Session {
        let built = super::super::docs_author::build(
            &json!({"properties": {"title": "Doc"}, "blocks": blocks}),
            None,
        )
        .unwrap();
        let doc = Document::from_read(docs_engine::docx::read(built.bytes).unwrap(), None);
        Session::from_doc(doc, None, None).unwrap()
    }

    fn texts(s: &mut Session) -> Vec<String> {
        let d = s.doc().unwrap();
        (0..d.len()).map(|i| shown(d, i)).collect()
    }

    #[std::prelude::v1::test]
    fn inserts_blocks_and_moves_sections() {
        let mut s = session(json!([{"h1": "One"}, "a", {"h1": "Two"}, "b", {"h1": "Three"}, "c"]));
        let out = s
            .run(
                "insert_blocks",
                &json!({"end_of_section": "One", "blocks": [{"h2": "New"}, "Text with a note[^N.]", {"bullets": ["x", "y"]},
                    {"type": "table", "header": ["H"], "rows": [["1"]], "caption": "T", "id": "t"}, "See {ref:t}."]}),
            )
            .unwrap();
        assert_eq!(out["done"], "insert_blocks");
        let t = texts(&mut s);
        let a = t.iter().position(|x| x == "a").unwrap();
        assert_eq!(t[a + 1], "New");
        assert!(
            t.iter()
                .any(|x| x.replace('\u{a0}', " ").starts_with("See Table 1")),
            "{t:?}"
        );
        assert!(!t.iter().any(|x| x.contains(MARKER)));
        s.run(
            "move_section",
            &json!({"heading": "Three", "before_heading": "One"}),
        )
        .unwrap();
        let t = texts(&mut s);
        assert_eq!(t[0], "Three");
        assert_eq!(t[1], "c");
        // Delete a section, restyle, headers, page setup.
        s.run("delete_blocks", &json!({"heading": "Two"})).unwrap();
        assert!(!texts(&mut s).iter().any(|x| x == "b"));
        s.run(
            "restyle",
            &json!({"style": "Heading 1", "color": "#112233", "space_before": 30}),
        )
        .unwrap();
        s.run("set_footer", &json!({"right": "Page {page} of {pages}"}))
            .unwrap();
        s.run("page_setup", &json!({"orientation": "landscape"}))
            .unwrap();
        let bytes = s.save().unwrap();
        let p = Pkg::read(&bytes).unwrap();
        let st = p.text("word/styles.xml").unwrap();
        let (a, b) = style_range(&st, "Heading1").unwrap();
        assert!(
            st[a..b].contains("112233") && st[a..b].contains("w:before=\"600\""),
            "{}",
            &st[a..b]
        );
        let d = p.text("word/document.xml").unwrap();
        assert!(d.contains("w:orient=\"landscape\""));
        assert!(
            p.parts
                .keys()
                .any(|k| k.starts_with("word/footer") && p.text(k).unwrap().contains("NUMPAGES"))
        );
    }

    #[std::prelude::v1::test]
    fn tracked_replace_comments_and_accepting() {
        let built = super::super::docs_author::build(
            &json!({"blocks": ["The cat sat.", "Another cat."]}),
            None,
        )
        .unwrap();
        let doc = Document::from_read(docs_engine::docx::read(built.bytes).unwrap(), None);
        let mut s = Session::from_doc(doc, None, Some("Reviewer".into())).unwrap();
        let out = s
            .run(
                "find_replace",
                &json!({"find": "cat", "replace": "dog", "bold": true}),
            )
            .unwrap();
        assert_eq!(out["replaced"], 2);
        assert!(!s.doc().unwrap().changes().is_empty());
        s.run(
            "add_comment",
            &json!({"paragraph": 0, "find": "sat", "text": "Check this"}),
        )
        .unwrap();
        assert_eq!(s.doc().unwrap().comments().len(), 1);
        let ins = s
            .run("insert_blocks", &json!({"blocks": ["Added."]}))
            .unwrap();
        assert_eq!(ins["done"], "insert_blocks");
        let r = s.run("accept_changes", &json!({})).unwrap();
        assert_eq!(r["left"], 0);
        let t = texts(&mut s);
        assert_eq!(t[0], "The dog sat.");
        assert!(t.iter().any(|x| x == "Added."));
        assert!(s.run("frobnicate", &json!({})).is_err());
    }

    /// `replace_paragraph`, `insert_paragraphs` and `delete_paragraphs`
    /// with `--track`: saved as `w:ins`/`w:del` by the author, listed, and
    /// rejecting gives the original back.
    #[std::prelude::v1::test]
    fn tracked_paragraph_ops() {
        let blocks = json!(["Intro", {"numbers": ["Satu", "Dua"]}, "Poin lama.", "Penutup"]);
        let original = ["Intro", "Satu", "Dua", "Poin lama.", "Penutup"];
        let open = |bytes: Vec<u8>, track: Option<&str>| {
            let doc = Document::from_read(docs_engine::docx::read(bytes).unwrap(), None);
            Session::from_doc(doc, None, track.map(str::to_owned)).unwrap()
        };
        let base = super::super::docs_author::build(&json!({"blocks": blocks}), None)
            .unwrap()
            .bytes;
        let ops = [
            ("replace_paragraph", json!({"index": 3, "text": "Diganti."})),
            (
                "insert_paragraphs",
                json!({"after_index": 2, "paragraphs": [{"text": "Tiga", "list": "number"}]}),
            ),
            ("delete_paragraphs", json!({"index": 4})),
            (
                "insert_paragraphs",
                json!({"after_index": 4, "paragraphs": [{"text": "Akhir"}]}),
            ),
        ];
        let accepted: [&[&str]; 4] = [
            &["Intro", "Satu", "Dua", "Diganti.", "Penutup"],
            &["Intro", "Satu", "Dua", "Tiga", "Poin lama.", "Penutup"],
            &["Intro", "Satu", "Dua", "Poin lama."],
            &["Intro", "Satu", "Dua", "Poin lama.", "Penutup", "Akhir"],
        ];
        for ((op, args), want) in ops.iter().zip(accepted) {
            let mut s = open(base.clone(), Some("AI"));
            s.run(op, args).unwrap();
            let bytes = s.save().unwrap();
            let xml = Pkg::read(&bytes)
                .unwrap()
                .text("word/document.xml")
                .unwrap();
            let by_ai = xml.matches("w:author=\"AI\"").count();
            assert!(by_ai >= 1, "{op}: no tracked change by AI");
            assert!(
                xml.contains("<w:ins ") || xml.contains("<w:del "),
                "{op}: no w:ins or w:del"
            );

            // Read back: listed, and resolving gives the right text.
            let mut back = open(bytes.clone(), None);
            let d = back.doc().unwrap();
            let n = d.changes().len();
            assert!(n >= 1, "{op}");
            let listed = super::super::docs_list::outline(d);
            assert_eq!(listed["tracked_change_count"], n, "{op}");
            assert_eq!(listed["tracked_changes"].as_array().unwrap().len(), n);
            back.run("reject_changes", &json!({})).unwrap();
            assert_eq!(texts(&mut back), original, "{op}: reject");
            let mut fwd = open(bytes, None);
            fwd.run("accept_changes", &json!({})).unwrap();
            assert_eq!(texts(&mut fwd), want, "{op}: accept");
            if *op == "insert_paragraphs" && want.contains(&"Tiga") {
                // The new item continues the list: 1. Satu 2. Dua 3. Tiga.
                let d = fwd.doc().unwrap();
                let num = |i: usize| d.styles.effective_num(d.paragraph(i)).map(|n| n.id);
                assert!(num(3).is_some());
                assert_eq!(num(3), num(1), "{op}: list continues");
            }
        }
    }

    /// `find_replace` goes through the body, the first-page and default
    /// headers and the footer, tracked or not; `\n` in the replacement is a
    /// line break in the same paragraph.
    #[std::prelude::v1::test]
    fn find_replace_covers_headers_and_footers() {
        use docs_engine::section::{HfKind, HfType};
        let mut base =
            Document::from_paragraphs(vec![docs_engine::Paragraph::new("Kepada $TUJUAN")]);
        base.set_title_page(0, true, SEL);
        let hf = [
            (HfKind::Header, HfType::First, "$KOPSURAT"),
            (HfKind::Header, HfType::Default, "Lanjutan $KOPSURAT"),
            (HfKind::Footer, HfType::Default, "$ALAMAT"),
        ];
        for (kind, ty, text) in hf {
            assert!(base.set_hf_text(0, kind, ty, text, None, SEL));
        }
        let bytes = base.to_docx_bytes().unwrap();
        let read = |b: Vec<u8>| Document::from_read(docs_engine::docx::read(b).unwrap(), None);
        for track in [None, Some("AI")] {
            let mut s =
                Session::from_doc(read(bytes.clone()), None, track.map(str::to_owned)).unwrap();
            let mut n = 0;
            for (find, with) in [
                ("$TUJUAN", "Bapak Kepala"),
                ("$KOPSURAT", "PT CONTOH\nJakarta"),
                ("$ALAMAT", "Jl. Merdeka 1"),
            ] {
                let out = s
                    .run("find_replace", &json!({"find": find, "replace": with}))
                    .unwrap();
                n += out["replaced"].as_u64().unwrap();
            }
            assert_eq!(n, 4, "{track:?}");
            // Only where asked.
            let none = s
                .run(
                    "find_replace",
                    &json!({"find": "Lanjutan", "replace": "x", "in": "body"}),
                )
                .unwrap();
            assert_eq!(none["replaced"], 0);
            assert!(
                s.run(
                    "find_replace",
                    &json!({"find": "a", "replace": "b", "in": "notes"})
                )
                .is_err()
            );

            let out = s.save().unwrap();
            let pkg = Pkg::read(&out).unwrap();
            let mut back = read(out);
            let parts: Vec<String> = back.stories().iter().map(|st| st.part.clone()).collect();
            assert_eq!(parts.len(), 3);
            let xml: String = parts.iter().map(|p| pkg.text(p).unwrap()).collect();
            assert!(xml.contains("<w:br/>"), "{track:?}: no line break");
            if track.is_some() {
                assert!(xml.contains("<w:ins ") && xml.contains("<w:del "));
                assert!(xml.contains("w:author=\"AI\""));
                assert!(back.part_changes() >= 3);
                back.resolve_all(true, SEL);
            }
            assert_eq!(back.paragraph(0).text, "Kepada Bapak Kepala");
            let shown = |kind, ty| {
                back.hf_text(0, kind, ty)
                    .0
                    .replace(docs_engine::LINE_BREAK, "|")
            };
            assert_eq!(shown(HfKind::Header, HfType::First), "PT CONTOH|Jakarta");
            assert_eq!(
                shown(HfKind::Header, HfType::Default),
                "Lanjutan PT CONTOH|Jakarta"
            );
            assert_eq!(shown(HfKind::Footer, HfType::Default), "Jl. Merdeka 1");
        }
    }

    #[std::prelude::v1::test]
    fn xml_helpers() {
        assert_eq!(
            children("<w:b/><w:sz w:val=\"2\"/><w:rFonts><x/></w:rFonts>").len(),
            3
        );
        assert_eq!(
            merge_attrs(
                "<w:spacing w:before=\"1\" w:after=\"2\"/>",
                "<w:spacing w:after=\"9\"/>"
            ),
            "<w:spacing w:before=\"1\" w:after=\"9\"/>"
        );
        let sp = "<w:sectPr><w:headerReference w:type=\"default\" r:id=\"rId9\"/><w:pgSz w:w=\"11906\" w:h=\"16838\"/></w:sectPr>";
        let n = with_reference(sp, "headerReference", "rId2");
        assert!(n.contains("rId2") && !n.contains("rId9"));
        let n = with_reference(sp, "footerReference", "rId3");
        assert!(n.find("rId9").unwrap() < n.find("rId3").unwrap());
        assert_eq!(
            sect_prs(&format!("<w:p><w:pPr>{sp}</w:pPr></w:p>{sp}")).len(),
            2
        );
        assert_eq!(sect_page(sp).width, 11906);
    }
}
