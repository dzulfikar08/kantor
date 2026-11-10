//! MCP tool calls (from `kantor mcp`, through [`crate::ipc`]) run here on
//! the main thread: find the document, run the tool through the view's
//! `agent_*` hooks (so edits are undoable, mark the document unsaved and
//! redraw like the user's own) and answer with JSON.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use gpui_kit::*;
use kantor_core::layout::PaneId;
use kantor_core::session::Session;
use serde_json::{Value, json};
use sheets_engine::CellRange;
use slides_engine::Focus;

use super::doc::DocView;
use super::{AppKind, Workspace};
use crate::agent::{self, docs, sheets, slides};

/// How long an agent's message stays in the status bar.
const NOTICE_TIME: Duration = Duration::from_secs(6);

fn kind_name(kind: AppKind) -> &'static str {
    match kind {
        AppKind::Sheets => "sheets",
        AppKind::Docs => "docs",
        AppKind::Slides => "slides",
    }
}

/// An absolute path from an argument (`~/` expanded).
fn path_arg(text: &str) -> Result<PathBuf> {
    let text = text.trim();
    let path = match text.strip_prefix("~/") {
        Some(rest) => {
            PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(rest)
        }
        None => PathBuf::from(text),
    };
    if !path.is_absolute() {
        bail!("give an absolute path (got \"{text}\")");
    }
    Ok(path)
}

/// Extensions each app can save to.
fn saves_as(kind: AppKind) -> &'static [&'static str] {
    match kind {
        AppKind::Sheets => &["xlsx", "csv", "tsv"],
        AppKind::Docs => &["docx"],
        AppKind::Slides => &["pptx"],
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

impl Workspace {
    /// Run one tool call; errors are messages for the agent.
    pub fn agent_call(
        &mut self,
        tool: &str,
        args: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let args = if args.is_null() { json!({}) } else { args };
        if !args.is_object() {
            return Err("arguments must be an object".into());
        }
        self.dispatch(tool, &args, window, cx)
            .map_err(|e| format!("{e:#}"))
    }

    fn dispatch(
        &mut self,
        tool: &str,
        args: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value> {
        match tool {
            "list_documents" => Ok(self.list_documents(cx)),
            "open_document" => self.agent_open(args, window, cx),
            "new_document" => {
                let kind = match agent::req_str(args, "kind")? {
                    "sheets" => AppKind::Sheets,
                    "docs" => AppKind::Docs,
                    "slides" => AppKind::Slides,
                    other => bail!("`kind` must be sheets, docs or slides, not \"{other}\""),
                };
                self.launch(kind, None, window, cx);
                let pane = self
                    .focused_pane()
                    .context("the new document didn't open")?;
                self.notice(format!("Agent: new {}", kind.name()), cx);
                Ok(self.doc_json(pane, cx))
            }
            "save_document" => self.agent_save(args, window, cx),
            "activate_document" => {
                let id = agent::req_int(args, "document")?;
                let pane = self.agent_pane(Some(id))?;
                self.show_pane(pane, window, cx);
                Ok(self.doc_json(pane, cx))
            }
            "get_selection" => {
                let (_, view) = self.agent_target(args, None)?;
                Ok(match view {
                    DocView::Sheets(e) => {
                        let v = e.read(cx);
                        v.agent_ready()?;
                        let (sheet, range, active) = v.agent_selection(cx);
                        v.agent_read(cx, |book, _| sheets::selection(book, sheet, range, active))
                    }
                    DocView::Docs(e) => e.read(cx).agent_read(docs::selection),
                    DocView::Slides(e) => e
                        .read(cx)
                        .agent_read(|p, current| json!({"slide": current + 1, "of": p.len()})),
                })
            }
            t if t.starts_with("sheets_") => self.sheets_tool(t, args, window, cx),
            t if t.starts_with("docs_") => self.docs_tool(t, args, window, cx),
            t if t.starts_with("slides_") => self.slides_tool(t, args, window, cx),
            "hello" => Ok(json!({"app": "kantor", "version": env!("CARGO_PKG_VERSION")})),
            other => bail!("unknown tool {other}"),
        }
    }

    /// Palette: put the `claude mcp add …` command on the clipboard.
    pub(super) fn copy_mcp_setup(&mut self, cx: &mut Context<Self>) {
        #[cfg(unix)]
        {
            let command = crate::mcp::claude_code_command();
            cx.write_to_clipboard(ClipboardItem::new_string(command.clone()));
            self.notice(format!("Copied: {command}  (run it in a terminal; `kantor mcp --print-config` has more)"), cx);
        }
        #[cfg(not(unix))]
        self.notice("The MCP server needs Linux or macOS".into(), cx);
    }

    // ── documents ──────────────────────────────────────────────────────────

    fn focused_pane(&self) -> Option<PaneId> {
        Some(self.tabs.get(self.active?)?.focused)
    }

    /// Documents in tab order.
    fn all_panes(&self) -> Vec<PaneId> {
        self.tabs.iter().flat_map(|t| t.layout.panes()).collect()
    }

    fn doc_json(&self, pane: PaneId, cx: &App) -> Value {
        let Some(doc) = self.docs.get(&pane) else {
            return Value::Null;
        };
        let v = &doc.view;
        json!({
            "id": pane,
            "kind": kind_name(v.kind()),
            "name": v.doc_name(cx),
            "path": v.path(cx),
            "unsaved_changes": v.is_dirty(cx),
            "in_front": self.focused_pane() == Some(pane),
        })
    }

    fn list_documents(&self, cx: &App) -> Value {
        let docs: Vec<Value> = self
            .all_panes()
            .into_iter()
            .map(|p| self.doc_json(p, cx))
            .collect();
        let hint = if docs.is_empty() {
            "No documents are open; use open_document or new_document."
        } else {
            "Tools act on the document in front unless given `document`."
        };
        json!({"documents": docs, "note": hint})
    }

    fn agent_pane(&self, id: Option<i64>) -> Result<PaneId> {
        match id {
            Some(id) => {
                let pane = u64::try_from(id).ok().filter(|p| self.docs.contains_key(p));
                pane.with_context(|| format!("no open document has id {id}; see list_documents"))
            }
            None => self.focused_pane().context(
                "no document is in front (Kantor shows its Home screen); pass `document`, or use open_document / new_document",
            ),
        }
    }

    /// The document a tool works on, checked to be of app `kind`.
    fn agent_target(&self, args: &Value, kind: Option<AppKind>) -> Result<(PaneId, DocView)> {
        let pane = self.agent_pane(agent::opt_int(args, "document")?)?;
        let view = self.docs[&pane].view.clone();
        if let Some(kind) = kind
            && view.kind() != kind
        {
            bail!(
                "document {pane} is a {} document; this tool works on {} documents",
                view.kind().name(),
                kind.name()
            );
        }
        Ok((pane, view))
    }

    /// Bring `pane`'s tab to front and focus it.
    fn show_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ti) = self.tabs.iter().position(|t| t.layout.contains(pane)) {
            self.tabs[ti].focused = pane;
            self.activate(Some(ti), window, cx);
        }
    }

    /// Show `text` in the status bar for a few seconds, whichever document
    /// is in front.
    pub(super) fn notice(&mut self, text: String, cx: &mut Context<Self>) {
        self.notice_id += 1;
        let id = self.notice_id;
        self.notice = Some(text.into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(NOTICE_TIME).await;
            let _ = this.update(cx, |ws, cx| {
                if ws.notice_id == id {
                    ws.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// After an agent edit: the notice, naming the document when it isn't
    /// the one in front.
    fn edited(&mut self, pane: PaneId, what: &str, cx: &mut Context<Self>) {
        let text = if self.focused_pane() == Some(pane) {
            format!("Agent: {what}")
        } else {
            let name = self
                .docs
                .get(&pane)
                .map(|d| d.view.doc_name(cx))
                .unwrap_or_default();
            format!("Agent: {what} in {name}")
        };
        self.notice(text, cx);
    }

    fn agent_open(
        &mut self,
        args: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value> {
        let path = path_arg(agent::req_str(args, "path")?)?;
        if !path.is_file() {
            bail!("{} is not a file", path.display());
        }
        let path = path.canonicalize().unwrap_or(path);
        let Some(kind) = AppKind::for_path(&path) else {
            bail!(
                "Kantor can't open {} (use .xlsx, .csv, .ods, .xls, .docx or .pptx)",
                path.display()
            );
        };
        self.launch(kind, Some(path.clone()), window, cx);
        let pane = self.focused_pane().context("the document didn't open")?;
        let opened = self.docs.get(&pane).and_then(|d| d.view.path(cx));
        if opened.as_deref().is_none_or(|p| !same_file(p, &path)) {
            // The view fell back to an empty document; its status says why.
            let why = self
                .docs
                .get(&pane)
                .map(|d| d.view.status(cx))
                .unwrap_or_default();
            if let Some(ti) = self.active {
                self.remove_pane(ti, pane, window, cx);
            }
            bail!("couldn't open {}: {why}", path.display());
        }
        self.notice(format!("Agent: opened {}", path.display()), cx);
        Ok(self.doc_json(pane, cx))
    }

    fn agent_save(
        &mut self,
        args: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value> {
        let (pane, view) = self.agent_target(args, None)?;
        let path = match agent::opt_str(args, "path")? {
            None => None,
            Some(text) => {
                let mut path = path_arg(text)?;
                let allowed = saves_as(view.kind());
                match path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_ascii_lowercase)
                {
                    None => {
                        path.set_extension(allowed[0]);
                    }
                    Some(ext) if allowed.contains(&ext.as_str()) => {}
                    Some(ext) => bail!(
                        "a {} document can't be saved as .{ext}; use .{}",
                        view.kind().name(),
                        allowed.join(" or .")
                    ),
                }
                let own = view.path(cx).is_some_and(|p| same_file(&p, &path));
                if path.exists() && !own && !agent::opt_bool(args, "overwrite")?.unwrap_or(false) {
                    bail!(
                        "{} already exists; pass overwrite=true to replace it",
                        path.display()
                    );
                }
                if !path.parent().is_some_and(Path::is_dir) {
                    bail!("the folder for {} doesn't exist", path.display());
                }
                Some(path)
            }
        };
        let status = match &view {
            DocView::Sheets(e) => e.update(cx, |v, cx| v.agent_save(path.as_deref(), window, cx)),
            DocView::Docs(e) => e.update(cx, |v, cx| v.agent_save(path.as_deref(), window, cx)),
            DocView::Slides(e) => e.update(cx, |v, cx| v.agent_save(path.as_deref(), window, cx)),
        }?;
        if let Some(p) = view.path(cx) {
            Session::touch_recent(&mut self.recent, &p);
        }
        self.persist(cx);
        self.edited(pane, &status, cx);
        let mut out = self.doc_json(pane, cx);
        out["status"] = json!(status);
        Ok(out)
    }

    // ── Sheets ─────────────────────────────────────────────────────────────

    fn sheets_tool(
        &mut self,
        tool: &str,
        args: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value> {
        let (pane, view) = self.agent_target(args, Some(AppKind::Sheets))?;
        let DocView::Sheets(e) = view else {
            unreachable!("checked by agent_target")
        };
        e.read(cx).agent_ready()?;
        let sheet = e
            .read(cx)
            .agent_read(cx, |book, shown| sheets::sheet_index(book, args, shown))?;
        let name = e.read(cx).agent_read(cx, |book, _| {
            book.sheet_names()
                .get(sheet as usize)
                .cloned()
                .unwrap_or_default()
        });
        // `(what, range to show)` and the edit, for the writing tools.
        type Edit = Box<dyn FnOnce(&mut sheets_engine::Workbook) -> Result<()>>;
        let (what, show, edit): (String, Option<CellRange>, Edit) = match tool {
            "sheets_list_sheets" => {
                return Ok(e.read(cx).agent_read(cx, sheets::list_sheets));
            }
            "sheets_read_range" => {
                return e
                    .read(cx)
                    .agent_read(cx, |book, _| sheets::read_range(book, sheet, args));
            }
            "sheets_select" => {
                let range = sheets::range_arg(args, "range")?;
                self.show_pane(pane, window, cx);
                e.update(cx, |v, cx| v.agent_select(sheet, range, window, cx))?;
                return Ok(json!({"selected": range.name(), "sheet": name}));
            }
            "sheets_write_range" => {
                let plan = sheets::plan_write(args)?;
                let what = format!("wrote {}", plan.range.name());
                let cells = plan.cells;
                (
                    what,
                    Some(plan.range),
                    Box::new(move |b| b.set_inputs(sheet, &cells)),
                )
            }
            "sheets_format" => {
                let plan = sheets::plan_format(args)?;
                let range = plan.range;
                let what = format!("formatted {}", range.name());
                let edit: Edit = Box::new(move |b| {
                    for c in plan.changes {
                        b.set_range_style(sheet, range, c)?;
                    }
                    if let Some(preset) = plan.borders {
                        b.set_range_borders(sheet, range, preset)?;
                    }
                    match plan.merge {
                        Some(true) => b.merge_range(sheet, range)?,
                        Some(false) => {
                            b.unmerge_range(sheet, range)?;
                        }
                        None => {}
                    }
                    Ok(())
                });
                (what, Some(range), edit)
            }
            "sheets_insert_rows" => {
                let row =
                    sheets::row_arg(args, "before_row")?.context("`before_row` is required")?;
                let count = agent::opt_count(args, "count", 1)?.unwrap_or(1).min(10_000) as u32;
                let range = CellRange::rows(row, row + count - 1);
                let what = format!("inserted {count} row(s) at {}", row + 1);
                (
                    what,
                    Some(range),
                    Box::new(move |b| b.insert_rows(sheet, row, count)),
                )
            }
            "sheets_delete_rows" => {
                let first =
                    sheets::row_arg(args, "first_row")?.context("`first_row` is required")?;
                let last = sheets::row_arg(args, "last_row")?.unwrap_or(first);
                if last < first {
                    bail!("`last_row` is before `first_row`");
                }
                let what = format!("deleted rows {}–{}", first + 1, last + 1);
                let show = CellRange::rows(first, first);
                (
                    what,
                    Some(show),
                    Box::new(move |b| b.delete_rows(sheet, first, last)),
                )
            }
            "sheets_insert_columns" => {
                let col = sheets::column_arg(args, "before_column")?
                    .context("`before_column` is required")?;
                let count = agent::opt_count(args, "count", 1)?.unwrap_or(1).min(1_000) as u32;
                let range = CellRange::columns(col, col + count - 1);
                let what = format!(
                    "inserted columns {}",
                    sheets::columns_name(col, col + count - 1)
                );
                (
                    what,
                    Some(range),
                    Box::new(move |b| b.insert_columns(sheet, col, count)),
                )
            }
            "sheets_delete_columns" => {
                let first = sheets::column_arg(args, "first_column")?
                    .context("`first_column` is required")?;
                let last = sheets::column_arg(args, "last_column")?.unwrap_or(first);
                if last < first {
                    bail!("`last_column` is before `first_column`");
                }
                let what = format!("deleted columns {}", sheets::columns_name(first, last));
                let show = CellRange::columns(first, first);
                (
                    what,
                    Some(show),
                    Box::new(move |b| b.delete_columns(sheet, first, last)),
                )
            }
            other => bail!("unknown tool {other}"),
        };
        let show = show.map(|r| (sheet, r));
        e.update(cx, |v, cx| v.agent_edit(&what, show, window, cx, edit))?;
        self.edited(pane, &what, cx);
        let mut out = json!({"done": what, "sheet": name});
        if tool == "sheets_write_range" {
            // Formulas' results, so the agent sees errors such as #NAME?.
            out["values"] = e.read(cx).agent_read(cx, |book, _| {
                let range = show.map(|s| s.1).expect("write shows its range");
                let args = json!({"range": range.name(), "include": "values", "max_cells": 400});
                sheets::read_range(book, sheet, &args).map_or(Value::Null, |r| {
                    if r.get("truncated").is_some() {
                        Value::Null
                    } else {
                        r["values"].clone()
                    }
                })
            });
        }
        if self.focused_pane() != Some(pane) {
            out["note"] =
                json!("This document isn't in front; activate_document shows it to the user.");
        }
        Ok(out)
    }

    // ── Docs ───────────────────────────────────────────────────────────────

    fn docs_tool(
        &mut self,
        tool: &str,
        args: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value> {
        let (pane, view) = self.agent_target(args, Some(AppKind::Docs))?;
        let DocView::Docs(e) = view else {
            unreachable!("checked by agent_target")
        };
        if tool == "docs_get_paragraphs" {
            return e.read(cx).agent_read(|doc, _| docs::paragraphs(doc, args));
        }
        let (what, out) = match tool {
            "docs_insert_paragraphs" => {
                let (after, items) = e
                    .read(cx)
                    .agent_read(|doc, _| docs::plan_insert(doc, args))?;
                let range = e.update(cx, |v, cx| {
                    v.agent_edit("inserted paragraphs", window, cx, |doc, sel| {
                        let range = doc
                            .insert_paragraphs(after, &items, sel)
                            .map_err(anyhow::Error::msg)?;
                        let end = doc.paragraph(range.end - 1).text.len();
                        let shown = docs_engine::Selection {
                            anchor: docs_engine::Position::new(range.start, 0),
                            head: docs_engine::Position::new(range.end - 1, end),
                        };
                        Ok((range, shown))
                    })
                })?;
                let what = format!("inserted {} paragraph(s) at {}", range.len(), range.start);
                (
                    what,
                    json!({"first_index": range.start, "count": range.len()}),
                )
            }
            "docs_replace_paragraph" => {
                let index = usize::try_from(agent::req_int(args, "index")?)
                    .context("`index` must be 0 or more")?;
                let text = agent::req_str(args, "text")?;
                agent::check_len("`text`", text)?;
                let range = e.update(cx, |v, cx| {
                    v.agent_edit("replaced a paragraph", window, cx, |doc, sel| {
                        let range = doc
                            .replace_paragraph_text(index, text, sel)
                            .map_err(anyhow::Error::msg)?;
                        let end = doc.paragraph(range.end - 1).text.len();
                        let shown = docs_engine::Selection {
                            anchor: docs_engine::Position::new(range.start, 0),
                            head: docs_engine::Position::new(range.end - 1, end),
                        };
                        Ok((range, shown))
                    })
                })?;
                (
                    format!("replaced paragraph {index}"),
                    json!({"first_index": range.start, "count": range.len()}),
                )
            }
            "docs_delete_paragraphs" => {
                let index = usize::try_from(agent::req_int(args, "index")?)
                    .context("`index` must be 0 or more")?;
                let count = agent::opt_count(args, "count", 1)?.unwrap_or(1) as usize;
                e.update(cx, |v, cx| {
                    v.agent_edit("deleted paragraphs", window, cx, |doc, sel| {
                        doc.delete_paragraphs(index, count, sel)
                            .map_err(anyhow::Error::msg)?;
                        let at = index.min(doc.len() - 1);
                        Ok((
                            (),
                            docs_engine::Selection::caret(docs_engine::Position::new(at, 0)),
                        ))
                    })
                })?;
                (
                    format!("deleted {count} paragraph(s) at {index}"),
                    json!({"paragraphs_now": e.read(cx).agent_read(|d, _| d.len())}),
                )
            }
            "docs_find_replace" => {
                let n = e.update(cx, |v, cx| {
                    v.agent_edit("replaced text", window, cx, |doc, sel| {
                        docs::find_replace(doc, sel, args)
                    })
                })?;
                if n == 0 {
                    return Ok(json!({"replaced": 0}));
                }
                (format!("replaced {n} match(es)"), json!({"replaced": n}))
            }
            other => bail!("unknown tool {other}"),
        };
        self.edited(pane, &what, cx);
        let mut out = out;
        out["done"] = json!(what);
        if self.focused_pane() != Some(pane) {
            out["note"] =
                json!("This document isn't in front; activate_document shows it to the user.");
        }
        Ok(out)
    }

    // ── Slides ─────────────────────────────────────────────────────────────

    fn slides_tool(
        &mut self,
        tool: &str,
        args: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value> {
        let (pane, view) = self.agent_target(args, Some(AppKind::Slides))?;
        let DocView::Slides(e) = view else {
            unreachable!("checked by agent_target")
        };
        let (what, out) = match tool {
            "slides_list" => return Ok(e.read(cx).agent_read(slides::list)),
            "slides_go_to" => {
                let i = e
                    .read(cx)
                    .agent_read(|p, _| slides::slide_arg(p, args, "slide"))?;
                self.show_pane(pane, window, cx);
                e.update(cx, |v, cx| v.agent_go_to(i, cx));
                return Ok(json!({"slide": i + 1}));
            }
            "slides_add_slide" => {
                let plan = e.read(cx).agent_read(|p, _| slides::plan_add(p, args))?;
                let i = e.update(cx, |v, cx| {
                    v.agent_edit("added a slide", window, cx, |p, _| {
                        let i = p.add_slide_with_text(
                            plan.after,
                            plan.layout,
                            plan.title.as_deref(),
                            &plan.body,
                        )?;
                        Ok((i, Focus::slide(i)))
                    })
                })?;
                (format!("added slide {}", i + 1), json!({"slide": i + 1}))
            }
            "slides_set_shape_text" => {
                let i = e
                    .read(cx)
                    .agent_read(|p, _| slides::slide_arg(p, args, "slide"))?;
                let id =
                    u32::try_from(agent::req_int(args, "shape_id")?).context("bad `shape_id`")?;
                let text = agent::req_str(args, "text")?;
                agent::check_len("`text`", text)?;
                e.update(cx, |v, cx| {
                    v.agent_edit("changed text", window, cx, |p, _| {
                        let ix = p.shape_index(i, id).with_context(|| {
                            format!("slide {} has no shape with id {id}; see slides_list", i + 1)
                        })?;
                        p.set_shape_text((i, ix), text)?;
                        Ok(((), Focus::shape(i, ix)))
                    })
                })?;
                (format!("changed shape {id} on slide {}", i + 1), json!({}))
            }
            "slides_delete_slide" => {
                let i = e
                    .read(cx)
                    .agent_read(|p, _| slides::slide_arg(p, args, "slide"))?;
                if e.read(cx).agent_read(|p, _| p.len()) == 1 {
                    bail!("that is the only slide; add another before deleting it");
                }
                e.update(cx, |v, cx| {
                    v.agent_edit("deleted a slide", window, cx, |p, _| {
                        p.delete_slide(i)?;
                        let next = i.min(p.len() - 1);
                        Ok(((), Focus::slide(next)))
                    })
                })?;
                (
                    format!("deleted slide {}", i + 1),
                    json!({"slides_now": e.read(cx).agent_read(|p, _| p.len())}),
                )
            }
            "slides_set_notes" => {
                let i = e
                    .read(cx)
                    .agent_read(|p, _| slides::slide_arg(p, args, "slide"))?;
                let text = agent::req_str(args, "text")?;
                agent::check_len("`text`", text)?;
                e.update(cx, |v, cx| {
                    v.agent_edit("changed speaker notes", window, cx, |p, _| {
                        p.set_notes_text(i, text)?;
                        Ok(((), Focus::slide(i)))
                    })
                })?;
                (format!("changed the notes of slide {}", i + 1), json!({}))
            }
            other => bail!("unknown tool {other}"),
        };
        self.edited(pane, &what, cx);
        let mut out = out;
        out["done"] = json!(what);
        if self.focused_pane() != Some(pane) {
            out["note"] =
                json!("This document isn't in front; activate_document shows it to the user.");
        }
        Ok(out)
    }
}
