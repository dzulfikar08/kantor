//! Mail merge, as Word's Mailings tab: the toolbar's Mailings button shows
//! a bar with Select Recipients (a workbook or CSV file, read by the Sheets
//! engine), Edit Recipient List (sheet, header row, filter, sort, ticks),
//! Insert Merge Field, Highlight Merge Fields, Preview Results with the
//! record arrows, and Finish & Merge: Edit Individual Documents (one new
//! document, a section per record), Save documents (one `.docx` or `.pdf`
//! per record, named from a field) and Print Documents.
//!
//! The data source link is saved in the document as Word saves it
//! (`w:mailMerge`, a path relative to the document), so Word and Kantor
//! both find it again; filter and sort are part of it (Word's query).
//! Unticked recipients are only remembered while the document is open.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use docs_engine::{FilterOp, MailMergeLink, Query};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    Disableable as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_merge::Recipients;
use kantor_ui::i18n::{self, t, t_dyn, tf, tk};
use kantor_ui::tokens::Elevation;
use kantor_ui::{metrics::*, tokens};

use crate::DocsView;

/// Rows the recipient list shows (the rest are counted).
const LIST_ROWS: usize = 500;
/// Columns the recipient list shows.
const LIST_COLUMNS: usize = 6;

/// Filter Records' comparisons ([`FilterOp::label`], shown with `t_dyn`).
const _OP_LABELS: [&str; 10] = [
    tk("Equal to"),
    tk("Not equal to"),
    tk("Less than"),
    tk("Greater than"),
    tk("Less than or equal"),
    tk("Greater than or equal"),
    tk("Is blank"),
    tk("Is not blank"),
    tk("Contains"),
    tk("Does not contain"),
];

/// What Finish & Merge makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finish {
    /// One new document with every record (Word's Edit Individual
    /// Documents).
    Edit,
    /// One `.docx` per record in a folder.
    Docx,
    /// One `.pdf` per record in a folder.
    Pdf,
    /// Every record printed (one PDF to the system's viewer).
    Print,
}

/// Events for the workspace.
pub enum DocsEvent {
    /// Open this document in a new tab (Edit Individual Documents).
    OpenMerged(Box<docs_engine::Document>),
}

impl EventEmitter<DocsEvent> for DocsView {}

/// The Mailings bar's state.
pub(crate) struct Mailings {
    pub recipients: Option<Recipients>,
    /// Why the data source couldn't be read.
    pub error: Option<String>,
    pub query: Query,
    /// Unticked rows (indices into the data's rows).
    pub excluded: HashSet<usize>,
    /// The records that merge, in order.
    pub records: Vec<usize>,
    pub preview: bool,
    /// The record previewed (index into `records`).
    pub at: usize,
    pub highlight: bool,
    /// The recipient list is open.
    pub list_open: bool,
    /// Filter Records' line being written.
    pub filter_field: Option<String>,
    pub filter_op: FilterOp,
    filter_value: Entity<InputState>,
    header_row: Entity<InputState>,
    name_pattern: Entity<InputState>,
    _subs: Vec<Subscription>,
}

impl Mailings {
    fn new(window: &mut Window, cx: &mut Context<DocsView>) -> Self {
        let filter_value = cx.new(|cx| InputState::new(window, cx));
        let header_row = cx.new(|cx| InputState::new(window, cx));
        let name_pattern = cx.new(|cx| InputState::new(window, cx));
        let subs = vec![
            cx.subscribe_in(
                &filter_value,
                window,
                |this, _, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        this.add_filter_doc(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &header_row,
                window,
                |this, _, ev: &InputEvent, window, cx| {
                    if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        this.apply_header_row(window, cx);
                    }
                },
            ),
        ];
        Self {
            recipients: None,
            error: None,
            query: Query::default(),
            excluded: HashSet::new(),
            records: Vec::new(),
            preview: false,
            at: 0,
            highlight: false,
            list_open: false,
            filter_field: None,
            filter_op: FilterOp::Equal,
            filter_value,
            header_row,
            name_pattern,
            _subs: subs,
        }
    }

    /// Work the merged records out again (filter, sort, ticks).
    fn reselect(&mut self) {
        self.records = match &self.recipients {
            Some(r) => r
                .data
                .select(&self.query)
                .into_iter()
                .filter(|k| !self.excluded.contains(k))
                .collect(),
            None => Vec::new(),
        };
        self.at = self.at.min(self.records.len().saturating_sub(1));
    }

    /// The data's column names.
    pub fn fields(&self) -> Vec<String> {
        self.recipients
            .as_ref()
            .map(|r| r.data.fields.clone())
            .unwrap_or_default()
    }
}

/// `path` relative to `dir` when it is inside it (or beside it: `../`),
/// `/` separated; else absolute.
pub(crate) fn relative_source(path: &Path, dir: Option<&Path>) -> String {
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if let Some(dir) = dir.and_then(|d| d.canonicalize().ok()) {
        if let Ok(rest) = abs.strip_prefix(&dir) {
            return rest.to_string_lossy().replace('\\', "/");
        }
        if let Some(parent) = dir.parent()
            && let Ok(rest) = abs.strip_prefix(parent)
        {
            return format!("../{}", rest.to_string_lossy().replace('\\', "/"));
        }
    }
    abs.to_string_lossy().into_owned()
}

impl DocsView {
    /// A pane showing a merged document (Edit Individual Documents),
    /// untitled and unsaved.
    pub fn new_merged(
        doc: docs_engine::Document,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let n = doc.sections().len();
        Self::with_document(
            doc,
            tf(
                "Merged document: {} records, each in its own section",
                &[&n],
            ),
            window,
            cx,
        )
    }

    /// Show or hide the Mailings bar (toolbar button, palette). Showing it
    /// reads the document's own data source, as Word does on opening.
    pub fn toggle_mailings_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mailings.take().is_some() {
            self.doc.set_merge_view(None);
            self.relayout();
            cx.notify();
            return;
        }
        self.leave_part(window, cx);
        self.mailings = Some(Mailings::new(window, cx));
        self.open_linked_source(window, cx);
        cx.notify();
    }

    fn mailings_mut(&mut self, window: &mut Window, cx: &mut Context<Self>) -> &mut Mailings {
        if self.mailings.is_none() {
            self.mailings = Some(Mailings::new(window, cx));
        }
        self.mailings.as_mut().expect("just made")
    }

    /// Read the data source the document links to, if any.
    fn open_linked_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(link) = self.doc.mail_merge().cloned() else {
            return;
        };
        let dir = self
            .doc
            .path()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        let Some(path) = link.source_path(dir.as_deref()) else {
            return;
        };
        let query = link.parsed_query();
        let sheet = (!link.is_text_file() && !query.table.is_empty()).then(|| query.table.clone());
        let preview = link.preview;
        let active = link.active_record.unwrap_or(1).max(1) as usize - 1;
        self.load_recipients(&path, sheet.as_deref(), None, Some(query), window, cx);
        if let Some(m) = &mut self.mailings
            && m.recipients.is_some()
        {
            m.preview = preview;
            m.at = active.min(m.records.len().saturating_sub(1));
        }
        self.refresh_merge_view();
    }

    /// Select Recipients: ask for a workbook or CSV file.
    pub fn select_recipients_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mailings_mut(window, cx);
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("Select Recipients (workbook or CSV file)").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.load_recipients(&path, None, None, None, window, cx);
                if let Some(m) = &mut this.mailings
                    && m.recipients.is_some()
                {
                    m.list_open = true;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Read `path` (sheet, header row) as the recipients and link the
    /// document to it. `query`: the filter and sort to keep (default
    /// none).
    pub(crate) fn load_recipients(
        &mut self,
        path: &Path,
        sheet: Option<&str>,
        header_row: Option<u32>,
        query: Option<Query>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = Recipients::open(path, sheet, header_row);
        let dir = self
            .doc
            .path()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        let m = self.mailings_mut(window, cx);
        match result {
            Ok(r) => {
                let mut q = query.unwrap_or_default();
                q.table = r.sheet_name().to_owned();
                let source = relative_source(path, dir.as_deref());
                let sheet = (!r.is_text_file()).then(|| r.sheet_name().to_owned());
                let link = MailMergeLink::new(&source, sheet.as_deref(), &q);
                let row = r.header_row.to_string();
                m.header_row
                    .update(cx, |i, cx| i.set_value(row, window, cx));
                m.recipients = Some(r);
                m.error = None;
                m.query = q;
                m.excluded.clear();
                m.at = 0;
                m.reselect();
                let n = m.records.len();
                self.doc.set_mail_merge(Some(link));
                self.status = tf("{} recipients from {}", &[&n, &crate::display_name(path)]).into();
            }
            Err(e) => {
                m.error = Some(format!("{e:#}"));
                self.status = tf("Can't read the data source: {}", &[&format!("{e:#}")]).into();
            }
        }
        self.refresh_merge_view();
        self.relayout();
        cx.notify();
    }

    /// Read another sheet of the data source.
    fn pick_sheet(&mut self, sheet: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.mailings.as_ref().and_then(|m| m.recipients.clone()) else {
            return;
        };
        self.load_recipients(&r.path, Some(&sheet), None, None, window, cx);
    }

    fn apply_header_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = &self.mailings else {
            return;
        };
        let Some(r) = m.recipients.clone() else {
            return;
        };
        let typed = m.header_row.read(cx).value().trim().parse::<u32>().ok();
        let Some(row) = typed.filter(|&n| n >= 1 && n != r.header_row) else {
            return;
        };
        let sheet = r.sheet_name().to_owned();
        self.load_recipients(&r.path, Some(&sheet), Some(row), None, window, cx);
    }

    /// The filter or sort changed: records, preview and the saved query.
    fn query_changed(&mut self, cx: &mut Context<Self>) {
        let Some(m) = &mut self.mailings else {
            return;
        };
        m.reselect();
        let q = m.query.clone();
        if let Some(link) = self.doc.mail_merge().cloned() {
            self.doc.set_mail_merge(Some(link.with_query(&q)));
        }
        self.refresh_merge_view();
        self.relayout();
        cx.notify();
    }

    fn add_filter_doc(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = &mut self.mailings else {
            return;
        };
        let Some(field) = m.filter_field.clone() else {
            return;
        };
        let value = m.filter_value.read(cx).value().trim().to_owned();
        if m.filter_op.takes_value() && value.is_empty() {
            return;
        }
        m.query.filters.push(docs_engine::Filter {
            field,
            op: m.filter_op,
            value,
            or: false,
        });
        self.query_changed(cx);
    }

    fn clear_filter_doc(&mut self, cx: &mut Context<Self>) {
        if let Some(m) = &mut self.mailings {
            m.query.filters.clear();
        }
        self.query_changed(cx);
    }

    fn set_sort_doc(&mut self, field: Option<String>, asc: bool, cx: &mut Context<Self>) {
        if let Some(m) = &mut self.mailings {
            m.query.sort = field.map(|f| vec![(f, asc)]).unwrap_or_default();
        }
        self.query_changed(cx);
    }

    fn toggle_recipient(&mut self, row: usize, cx: &mut Context<Self>) {
        if let Some(m) = &mut self.mailings
            && !m.excluded.remove(&row)
        {
            m.excluded.insert(row);
        }
        self.query_changed(cx);
    }

    fn tick_all(&mut self, on: bool, cx: &mut Context<Self>) {
        if let Some(m) = &mut self.mailings {
            if on {
                m.excluded.clear();
            } else if let Some(r) = &m.recipients {
                m.excluded = (0..r.data.rows.len()).collect();
            }
        }
        self.query_changed(cx);
    }

    /// Insert Merge Field: `MERGEFIELD Name` at the caret.
    pub fn insert_merge_field_doc(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let code = docs_engine::merge::merge_field_code(name);
        self.insert_field_doc(&code, window, cx);
        self.refresh_merge_view();
        self.relayout();
        self.focus(window, cx);
        cx.notify();
    }

    /// Preview Results on or off.
    pub fn toggle_merge_preview_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let m = self.mailings_mut(window, cx);
        m.preview = !m.preview;
        let on = m.preview;
        self.save_preview_state();
        if on && self.mailings.as_ref().is_some_and(|m| m.records.is_empty()) {
            self.status = t("Select recipients first (Mailings ▸ Select Recipients)").into();
        }
        self.refresh_merge_view();
        self.relayout();
        cx.notify();
    }

    /// Show record `at` (index into the merged records), clamped.
    pub fn go_to_record_doc(&mut self, at: isize, cx: &mut Context<Self>) {
        let Some(m) = &mut self.mailings else {
            return;
        };
        let last = m.records.len().saturating_sub(1) as isize;
        m.at = at.clamp(0, last.max(0)) as usize;
        m.preview = true;
        self.save_preview_state();
        self.refresh_merge_view();
        self.relayout();
        cx.notify();
    }

    fn step_record(&mut self, by: isize, cx: &mut Context<Self>) {
        let at = self.mailings.as_ref().map_or(0, |m| m.at as isize);
        self.go_to_record_doc(at + by, cx);
    }

    /// Preview and the record shown go into the link, as Word saves them.
    fn save_preview_state(&mut self) {
        let Some(m) = &self.mailings else {
            return;
        };
        if let Some(mut link) = self.doc.mail_merge().cloned() {
            link.preview = m.preview;
            link.active_record = m.preview.then_some(m.at as u32 + 1);
            self.doc.set_mail_merge(Some(link));
        }
    }

    pub fn toggle_merge_highlight_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let m = self.mailings_mut(window, cx);
        m.highlight = !m.highlight;
        cx.notify();
    }

    /// The merge fields show the previewed record's values, or their own
    /// results (layout reads the document's merge view).
    pub(crate) fn refresh_merge_view(&mut self) {
        if self.part.is_some() {
            return;
        }
        let view = self.mailings.as_ref().and_then(|m| {
            let r = m.recipients.as_ref()?;
            (m.preview && !m.records.is_empty())
                .then(|| self.doc.merge_preview(&r.data, &m.records, m.at))
        });
        self.doc.set_merge_view(view);
    }

    /// Ranges of the merge fields to shade (Highlight Merge Fields).
    pub(crate) fn merge_highlights(&self) -> Vec<(docs_engine::Position, docs_engine::Position)> {
        match &self.mailings {
            Some(m) if m.highlight && self.part.is_none() => self.doc.merge_field_ranges(),
            _ => Vec::new(),
        }
    }

    /// Finish & Merge.
    pub fn finish_merge_doc(&mut self, what: Finish, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_part(window, cx);
        let Some((data, records)) = self.mailings.as_ref().and_then(|m| {
            m.recipients
                .as_ref()
                .map(|r| (r.data.clone(), m.records.clone()))
        }) else {
            self.status = t("Select recipients first (Mailings ▸ Select Recipients)").into();
            cx.notify();
            return;
        };
        if records.is_empty() {
            self.status = t("No recipients are selected").into();
            cx.notify();
            return;
        }
        let mut doc = self.doc.clone();
        doc.set_merge_view(None);
        match what {
            Finish::Edit | Finish::Print => {
                let Some(merged) = doc.merge_into_one(&data, &records) else {
                    self.status = t("Every record was skipped").into();
                    cx.notify();
                    return;
                };
                if what == Finish::Edit {
                    self.status =
                        tf("Merged {} records into a new document", &[&records.len()]).into();
                    cx.emit(DocsEvent::OpenMerged(Box::new(merged)));
                } else {
                    let laid = docs_pdf::lay_out(&merged, &mut self.engine.borrow_mut());
                    let scene = docs_pdf::scene(&merged, &laid);
                    let stem = std::path::Path::new(&self.name())
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let written = kantor_core::dirs::print_pdf_path(&stem)
                        .map_err(anyhow::Error::from)
                        .and_then(|path| {
                            let bytes = docs_pdf::write(&scene)?;
                            docs_pdf::write_file(&path, &bytes)?;
                            Ok(path)
                        });
                    match written {
                        Ok(path) => {
                            cx.open_with_system(&path);
                            self.status = tf(
                                "Opened {} merged records in the PDF viewer to print",
                                &[&records.len()],
                            )
                            .into();
                        }
                        Err(e) => self.status = tf("Print failed: {}", &[&format!("{e:#}")]).into(),
                    }
                }
                cx.notify();
            }
            Finish::Docx | Finish::Pdf => {
                let pattern = self
                    .mailings
                    .as_ref()
                    .map(|m| m.name_pattern.read(cx).value().trim().to_owned())
                    .filter(|p| !p.is_empty());
                let paths = cx.prompt_for_paths(PathPromptOptions {
                    files: false,
                    directories: true,
                    multiple: false,
                    prompt: Some(t("Save the merged documents in this folder").into()),
                });
                let template = doc.path().map(Path::to_path_buf);
                cx.spawn_in(window, async move |this, cx| {
                    let Ok(Ok(Some(paths))) = paths.await else {
                        return;
                    };
                    let Some(dir) = paths.into_iter().next() else {
                        return;
                    };
                    let _ = this.update(cx, |this, cx| {
                        let out = kantor_merge::docs::DocsOut {
                            dir: Some(dir.clone()),
                            docx: what == Finish::Docx,
                            pdf: what == Finish::Pdf,
                            name: pattern.or_else(|| {
                                Some(kantor_merge::docs::default_pattern(template.as_deref()))
                            }),
                            single: None,
                            overwrite: kantor_merge::Overwrite::Refuse,
                        };
                        let engine = this.engine.clone();
                        let result = kantor_merge::docs::merge(
                            &doc,
                            &data,
                            &records,
                            &out,
                            &mut |d, path| {
                                kantor_merge::docs::write_pdf(d, path, &mut engine.borrow_mut())
                            },
                        );
                        this.status = match result {
                            Ok(report) => tf(
                                "Saved {} merged documents in {}",
                                &[&report.files.len(), &crate::display_name(&dir)],
                            )
                            .into(),
                            Err(e) => tf("Merge failed: {}", &[&format!("{e:#}")]).into(),
                        };
                        cx.notify();
                    });
                })
                .detach();
            }
        }
    }

    /// The Mailings bar.
    pub(crate) fn render_mailings_bar(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let m = self.mailings.as_ref()?;
        let tok = tokens(cx);
        let view = cx.entity().downgrade();
        let has_data = m.recipients.is_some();
        let fields = m.fields();
        let used = self.doc.merge_field_names();
        let count = m.records.len();

        let button = |id: &'static str, label: &'static str, tip: &'static str, enabled: bool| {
            div()
                .id(id)
                .flex_none()
                .h(CONTROL_SM)
                .px(SPACE_3)
                .flex()
                .items_center()
                .rounded(RADIUS_MD)
                .border_1()
                .border_color(tok.border)
                .text_color(tok.fg)
                .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(tok.hover)))
                .when(!enabled, |d| d.opacity(0.5))
                .child(i18n::t(label))
                .tooltip(move |window, cx| Tooltip::new(i18n::t(tip)).build(window, cx))
        };
        let check = |id: &'static str, label: &'static str, on: bool, enabled: bool| {
            h_flex()
                .id(id)
                .flex_none()
                .h(CONTROL_SM)
                .px(SPACE_3)
                .gap(SPACE_2)
                .rounded(RADIUS_MD)
                .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(tok.hover)))
                .when(!enabled, |d| d.opacity(0.5))
                .child(check_box(on, &tok))
                .child(div().text_color(tok.fg).child(i18n::t(label)))
        };
        let select = {
            let v = view.clone();
            button(
                "mm-select",
                tk("Select Recipients…"),
                tk("Use a workbook or CSV file: its first row names the fields"),
                true,
            )
            .on_click(move |_, window, cx| {
                let _ = v.update(cx, |this, cx| this.select_recipients_doc(window, cx));
            })
        };
        let edit = {
            let v = view.clone();
            button(
                "mm-edit",
                tk("Edit Recipient List…"),
                tk("Sheet, header row, filter, sort and which recipients merge"),
                has_data,
            )
            .when(has_data, |d| {
                d.on_click(move |_, _, cx| {
                    let _ = v.update(cx, |this, cx| {
                        if let Some(m) = &mut this.mailings {
                            m.list_open = !m.list_open;
                        }
                        cx.notify();
                    });
                })
            })
        };
        let insert = {
            let v = view.clone();
            let names = if fields.is_empty() {
                used.clone()
            } else {
                fields.clone()
            };
            Button::new("mm-insert")
                .ghost()
                .xsmall()
                .label(i18n::t("Insert Merge Field"))
                .dropdown_caret(true)
                .disabled(names.is_empty())
                .tooltip(i18n::t("A field filled in from each recipient"))
                .dropdown_menu(move |mut menu, _, _| {
                    for n in &names {
                        let v = v.clone();
                        let name = n.clone();
                        menu = menu.item(PopupMenuItem::new(n.clone()).on_click(
                            move |_, window, cx| {
                                let _ = v.update(cx, |this, cx| {
                                    this.insert_merge_field_doc(&name, window, cx)
                                });
                            },
                        ));
                    }
                    menu
                })
        };
        let highlight = {
            let v = view.clone();
            check(
                "mm-highlight",
                tk("Highlight Merge Fields"),
                m.highlight,
                true,
            )
            .on_click(move |_, window, cx| {
                let _ = v.update(cx, |this, cx| this.toggle_merge_highlight_doc(window, cx));
            })
        };
        let preview = {
            let v = view.clone();
            check("mm-preview", tk("Preview Results"), m.preview, has_data).when(has_data, |d| {
                d.on_click(move |_, window, cx| {
                    let _ = v.update(cx, |this, cx| this.toggle_merge_preview_doc(window, cx));
                })
            })
        };
        let arrow = |id: &'static str, icon: Lucide, tip: &'static str, to: isize, rel: bool| {
            let v = view.clone();
            div()
                .id(id)
                .flex_none()
                .size(CONTROL_SM)
                .flex()
                .items_center()
                .justify_center()
                .rounded(RADIUS_MD)
                .when(has_data, |d| {
                    d.cursor_pointer()
                        .hover(|s| s.bg(tok.hover))
                        .on_click(move |_, _, cx| {
                            let _ = v.update(cx, |this, cx| {
                                if rel {
                                    this.step_record(to, cx)
                                } else {
                                    this.go_to_record_doc(to, cx)
                                }
                            });
                        })
                })
                .when(!has_data, |d| d.opacity(0.5))
                .child(Icon::new(icon).with_size(ICON_MD))
                .tooltip(move |window, cx| Tooltip::new(i18n::t(tip)).build(window, cx))
        };
        let record = if has_data && count > 0 {
            tf("Record {} of {}", &[&(m.at + 1), &count])
        } else if has_data {
            t("No recipients").to_owned()
        } else {
            t("No data source").to_owned()
        };
        let finish = {
            let v = view.clone();
            Button::new("mm-finish")
                .primary()
                .xsmall()
                .label(i18n::t("Finish & Merge"))
                .dropdown_caret(true)
                .disabled(!has_data || count == 0)
                .dropdown_menu(move |menu, _, _| {
                    let item = |label: &'static str, icon: Lucide, what: Finish| {
                        let v = v.clone();
                        PopupMenuItem::new(i18n::t(label)).icon(icon).on_click(
                            move |_, window, cx| {
                                let _ = v
                                    .update(cx, |this, cx| this.finish_merge_doc(what, window, cx));
                            },
                        )
                    };
                    menu.item(item(
                        tk("Edit Individual Documents"),
                        Lucide::FilePlus,
                        Finish::Edit,
                    ))
                    .item(item(
                        tk("Save as Documents (.docx), One per Recipient…"),
                        Lucide::FileText,
                        Finish::Docx,
                    ))
                    .item(item(
                        tk("Save as PDF, One per Recipient…"),
                        Lucide::FileDown,
                        Finish::Pdf,
                    ))
                    .item(item(
                        tk("Print Documents…"),
                        Lucide::Printer,
                        Finish::Print,
                    ))
                })
        };
        let names = h_flex()
            .id("mm-names")
            .flex_none()
            .gap(SPACE_2)
            .child(div().text_color(tok.fg_muted).child(t("File names")))
            .child(div().w(REM * 11.).child(Input::new(&m.name_pattern).small()))
            .tooltip(|window, cx| {
                Tooltip::new(t(
                    "{Field} is the recipient's value, {#} the record number; empty: the document's name",
                ))
                .build(window, cx)
            });
        let close = {
            let v = view.clone();
            div()
                .id("mm-close")
                .flex_none()
                .size(CONTROL_SM)
                .flex()
                .items_center()
                .justify_center()
                .rounded(RADIUS_MD)
                .cursor_pointer()
                .text_color(tok.fg_muted)
                .hover(|s| s.bg(tok.hover).text_color(tok.fg))
                .child(Icon::new(Lucide::X).with_size(ICON_SM))
                .tooltip(|window, cx| Tooltip::new(i18n::t("Close Mailings")).build(window, cx))
                .on_click(move |_, window, cx| {
                    let _ = v.update(cx, |this, cx| {
                        this.toggle_mailings_doc(window, cx);
                        this.focus(window, cx);
                    });
                })
        };
        let source = match (&m.recipients, &m.error) {
            (Some(r), _) => {
                let mut s = crate::display_name(&r.path);
                if !r.is_text_file() && r.sheets.len() > 1 {
                    s = format!("{s} · {}", r.sheet_name());
                }
                s
            }
            (None, Some(e)) => e.clone(),
            (None, None) => String::new(),
        };
        Some(
            h_flex()
                .id("docs-mailings-bar")
                .key_context(crate::POPOVER_CONTEXT)
                .on_action(|_: &crate::PopoverKey, _, _| {})
                .flex_none()
                .flex_wrap()
                .items_center()
                .gap(SPACE_3)
                .px(SPACE_6)
                .py(SPACE_2)
                .bg(tok.chrome)
                .border_b_1()
                .border_color(tok.border)
                .text_size(FONT_SMALL)
                .text_color(tok.fg)
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(tok.accent)
                        .child(t("Mailings")),
                )
                .child(select)
                .child(edit)
                .when(!source.is_empty(), |d| {
                    d.child(
                        div()
                            .max_w(REM * 14.)
                            .truncate()
                            .text_color(if m.error.is_some() && m.recipients.is_none() {
                                tok.danger
                            } else {
                                tok.fg_muted
                            })
                            .child(source),
                    )
                })
                .child(insert)
                .child(highlight)
                .child(preview)
                .child(
                    h_flex()
                        .flex_none()
                        .gap(px(2.))
                        .items_center()
                        .child(arrow(
                            "mm-first",
                            Lucide::ChevronsLeft,
                            tk("First record"),
                            0,
                            false,
                        ))
                        .child(arrow(
                            "mm-prev",
                            Lucide::ChevronLeft,
                            tk("Previous record"),
                            -1,
                            true,
                        ))
                        .child(div().px(SPACE_2).text_color(tok.fg_muted).child(record))
                        .child(arrow(
                            "mm-next",
                            Lucide::ChevronRight,
                            tk("Next record"),
                            1,
                            true,
                        ))
                        .child(arrow(
                            "mm-last",
                            Lucide::ChevronsRight,
                            tk("Last record"),
                            isize::MAX,
                            false,
                        )),
                )
                .child(div().flex_1())
                .child(names)
                .child(finish)
                .child(close),
        )
    }

    /// The recipient list (Edit Recipient List).
    pub(crate) fn render_recipients(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let m = self.mailings.as_ref().filter(|m| m.list_open)?;
        let r = m.recipients.as_ref()?;
        let tok = tokens(cx);
        let view = cx.entity().downgrade();
        let data = &r.data;
        let shown_rows = data.select(&m.query);
        let close = {
            let v = view.clone();
            move |_: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                let _ = v.update(cx, |this, cx| {
                    if let Some(m) = &mut this.mailings {
                        m.list_open = false;
                    }
                    this.focus(window, cx);
                    cx.notify();
                });
            }
        };
        let chip = |id: SharedString, label: String, on: bool| {
            div()
                .id(id)
                .flex_none()
                .h(CONTROL_SM)
                .px(SPACE_3)
                .flex()
                .items_center()
                .rounded(RADIUS_MD)
                .cursor_pointer()
                .when(on, |d| d.bg(tok.selected).text_color(tok.fg))
                .when(!on, |d| {
                    d.border_1()
                        .border_color(tok.border)
                        .text_color(tok.fg)
                        .hover(|s| s.bg(tok.hover))
                })
                .child(label)
        };
        let label = |s: &'static str| div().flex_none().text_color(tok.fg_muted).child(t(s));

        // Sheets and header row.
        let mut source = h_flex().flex_wrap().gap(SPACE_2).items_center();
        if !r.is_text_file() && r.sheets.len() > 1 {
            source = source.child(label(tk("Sheet")));
            for (k, s) in r.sheets.iter().enumerate() {
                let v = view.clone();
                let name = s.clone();
                source = source.child(
                    chip(format!("mm-sheet-{k}").into(), s.clone(), k == r.sheet).on_click(
                        move |_, window, cx| {
                            let _ =
                                v.update(cx, |this, cx| this.pick_sheet(name.clone(), window, cx));
                        },
                    ),
                );
            }
        }
        source = source
            .child(label(tk("Header row")))
            .child(div().w(REM * 4.).child(Input::new(&m.header_row).small()));

        // Sort.
        let (sort_field, asc) = m
            .query
            .sort
            .first()
            .cloned()
            .map_or((None, true), |(f, a)| (Some(f), a));
        let sort = {
            let v = view.clone();
            let names = data.fields.clone();
            let current = sort_field.clone();
            Button::new("mm-sort")
                .outline()
                .xsmall()
                .label(current.clone().unwrap_or_else(|| t("(none)").to_owned()))
                .dropdown_caret(true)
                .dropdown_menu(move |mut menu, _, _| {
                    let none = v.clone();
                    menu = menu.item(PopupMenuItem::new(t("(none)")).on_click(move |_, _, cx| {
                        let _ = none.update(cx, |this, cx| this.set_sort_doc(None, true, cx));
                    }));
                    for n in &names {
                        let v = v.clone();
                        let f = n.clone();
                        menu =
                            menu.item(PopupMenuItem::new(n.clone()).on_click(move |_, _, cx| {
                                let _ = v.update(cx, |this, cx| {
                                    this.set_sort_doc(Some(f.clone()), asc, cx)
                                });
                            }));
                    }
                    menu
                })
        };
        let direction = {
            let v = view.clone();
            let f = sort_field.clone();
            chip(
                "mm-sort-dir".into(),
                if asc { t("Ascending") } else { t("Descending") }.to_owned(),
                false,
            )
            .when(f.is_some(), |d| {
                d.on_click(move |_, _, cx| {
                    let _ = v.update(cx, |this, cx| this.set_sort_doc(f.clone(), !asc, cx));
                })
            })
        };

        // Filter.
        let field_btn = {
            let v = view.clone();
            let names = data.fields.clone();
            Button::new("mm-filter-field")
                .outline()
                .xsmall()
                .label(
                    m.filter_field
                        .clone()
                        .unwrap_or_else(|| t("Field").to_owned()),
                )
                .dropdown_caret(true)
                .dropdown_menu(move |mut menu, _, _| {
                    for n in &names {
                        let v = v.clone();
                        let f = n.clone();
                        menu =
                            menu.item(PopupMenuItem::new(n.clone()).on_click(move |_, _, cx| {
                                let _ = v.update(cx, |this, cx| {
                                    if let Some(m) = &mut this.mailings {
                                        m.filter_field = Some(f.clone());
                                    }
                                    cx.notify();
                                });
                            }));
                    }
                    menu
                })
        };
        let op_btn = {
            let v = view.clone();
            Button::new("mm-filter-op")
                .outline()
                .xsmall()
                .label(t_dyn(m.filter_op.label()).to_owned())
                .dropdown_caret(true)
                .dropdown_menu(move |mut menu, _, _| {
                    for op in FilterOp::ALL {
                        let v = v.clone();
                        menu =
                            menu.item(PopupMenuItem::new(t_dyn(op.label()).to_owned()).on_click(
                                move |_, _, cx| {
                                    let _ = v.update(cx, |this, cx| {
                                        if let Some(m) = &mut this.mailings {
                                            m.filter_op = op;
                                        }
                                        cx.notify();
                                    });
                                },
                            ));
                    }
                    menu
                })
        };
        let add_filter = {
            let v = view.clone();
            chip("mm-filter-add".into(), t("Add Filter").to_owned(), false).on_click(
                move |_, window, cx| {
                    let _ = v.update(cx, |this, cx| this.add_filter_doc(window, cx));
                },
            )
        };
        let clear_filter = {
            let v = view.clone();
            chip(
                "mm-filter-clear".into(),
                t("Clear Filters").to_owned(),
                false,
            )
            .on_click(move |_, _, cx| {
                let _ = v.update(cx, |this, cx| this.clear_filter_doc(cx));
            })
        };
        let filters: Vec<String> = m
            .query
            .filters
            .iter()
            .enumerate()
            .map(|(k, f)| {
                let join = match (k, f.or) {
                    (0, _) => String::new(),
                    (_, true) => format!("{} ", t("Or")),
                    (_, false) => format!("{} ", t("And")),
                };
                if f.op.takes_value() {
                    format!(
                        "{join}{} {} \"{}\"",
                        f.field,
                        t_dyn(f.op.label()).to_lowercase(),
                        f.value
                    )
                } else {
                    format!("{join}{} {}", f.field, t_dyn(f.op.label()).to_lowercase())
                }
            })
            .collect();

        // The table.
        let cols: Vec<(usize, &String)> =
            data.fields.iter().enumerate().take(LIST_COLUMNS).collect();
        let all_on = shown_rows.iter().all(|k| !m.excluded.contains(k));
        let cell = |s: String| {
            div()
                .w(REM * 9.)
                .flex_none()
                .truncate()
                .px(SPACE_2)
                .child(s)
        };
        let mut head = h_flex()
            .h(CONTROL_SM)
            .items_center()
            .border_b_1()
            .border_color(tok.border)
            .font_weight(FontWeight::SEMIBOLD)
            .child({
                let v = view.clone();
                div()
                    .id("mm-tick-all")
                    .flex_none()
                    .w(REM * 2.)
                    .cursor_pointer()
                    .child(check_box(all_on, &tok))
                    .on_click(move |_, _, cx| {
                        let _ = v.update(cx, |this, cx| this.tick_all(!all_on, cx));
                    })
            });
        for (_, name) in &cols {
            head = head.child(cell((*name).clone()));
        }
        let mut body = v_flex().id("mm-rows").max_h(REM * 18.).overflow_y_scroll();
        for &row in shown_rows.iter().take(LIST_ROWS) {
            let on = !m.excluded.contains(&row);
            let v = view.clone();
            let mut line = h_flex()
                .id(SharedString::from(format!("mm-row-{row}")))
                .min_h(CONTROL_SM)
                .items_center()
                .rounded(RADIUS_SM)
                .cursor_pointer()
                .hover(|s| s.bg(tok.hover))
                .when(!on, |d| d.text_color(tok.fg_dim))
                .child(div().flex_none().w(REM * 2.).child(check_box(on, &tok)))
                .on_click(move |_, _, cx| {
                    let _ = v.update(cx, |this, cx| this.toggle_recipient(row, cx));
                });
            for (c, _) in &cols {
                let text = data.rows[row]
                    .get(*c)
                    .map(|v| v.text.clone())
                    .unwrap_or_default();
                line = line.child(cell(text));
            }
            body = body.child(line);
        }
        if shown_rows.len() > LIST_ROWS {
            body = body.child(
                div()
                    .p(SPACE_2)
                    .text_color(tok.fg_muted)
                    .child(tf("{} more recipients", &[&(shown_rows.len() - LIST_ROWS)])),
            );
        }
        let summary = tf(
            "{} of {} recipients will merge",
            &[&m.records.len(), &data.rows.len()],
        );
        let ok = {
            let v = view.clone();
            div()
                .id("mm-list-ok")
                .flex_none()
                .h(CONTROL_SM)
                .px(SPACE_4)
                .flex()
                .items_center()
                .rounded(RADIUS_MD)
                .cursor_pointer()
                .bg(tok.accent)
                .text_color(tok.accent_fg)
                .child(t("OK"))
                .on_click(move |_, window, cx| {
                    let _ = v.update(cx, |this, cx| {
                        if let Some(m) = &mut this.mailings {
                            m.list_open = false;
                        }
                        this.focus(window, cx);
                        cx.notify();
                    });
                })
        };
        let panel = v_flex()
            .id("docs-mm-recipients")
            .key_context(crate::POPOVER_CONTEXT)
            .on_action(|_: &crate::PopoverKey, _, _| {})
            .occlude()
            .w(REM * 46.)
            .gap(SPACE_3)
            .p(SPACE_4)
            .bg(tok.elevated)
            .border_1()
            .border_color(tok.border)
            .rounded(RADIUS_LG)
            .shadow(tok.shadow(Elevation::Popover))
            .text_size(FONT_SMALL)
            .text_color(tok.fg)
            .child(
                h_flex()
                    .gap(SPACE_3)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t("Mail Merge Recipients")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(tok.fg_muted)
                            .child(crate::display_name(&r.path)),
                    ),
            )
            .child(source)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(SPACE_2)
                    .items_center()
                    .child(label(tk("Sort by")))
                    .child(sort)
                    .child(direction),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(SPACE_2)
                    .items_center()
                    .child(label(tk("Filter")))
                    .child(field_btn)
                    .child(op_btn)
                    .child(div().w(REM * 9.).child(Input::new(&m.filter_value).small()))
                    .child(add_filter)
                    .when(!filters.is_empty(), |d| d.child(clear_filter)),
            )
            .when(!filters.is_empty(), |d| {
                d.child(div().text_color(tok.fg_muted).child(filters.join("  ")))
            })
            .child(
                v_flex()
                    .id("mm-table")
                    .overflow_x_scroll()
                    .rounded(RADIUS_MD)
                    .border_1()
                    .border_color(tok.border)
                    .bg(tok.surface)
                    .p(SPACE_1)
                    .child(head)
                    .child(body),
            )
            .child(
                h_flex()
                    .items_center()
                    .child(div().flex_1().text_color(tok.fg_muted).child(summary))
                    .child(ok),
            );
        let at = {
            let b = self.viewport.get();
            point(b.origin.x + b.size.width / 2., b.origin.y + SPACE_7)
        };
        Some(kantor_ui::overlay::popover(
            anchored()
                .position(at)
                .anchor(Anchor::TopCenter)
                .snap_to_window_with_margin(SPACE_4)
                .child(panel.on_mouse_down_out(close)),
        ))
    }
}

fn check_box(on: bool, tok: &kantor_ui::Tokens) -> Div {
    div()
        .flex_none()
        .size(ICON_MD)
        .flex()
        .items_center()
        .justify_center()
        .rounded(RADIUS_SM)
        .border_1()
        .border_color(tok.border_strong)
        .when(on, |d| {
            d.bg(tok.accent)
                .border_color(tok.accent)
                .text_color(tok.accent_fg)
                .child(Icon::new(Lucide::Check).with_size(ICON_SM))
        })
}

/// The toolbar's Mailings button.
pub(crate) fn mailings_button(view: &DocsView, cx: &mut Context<DocsView>) -> Stateful<Div> {
    let tok = tokens(cx);
    let v = cx.entity().downgrade();
    let open = view.mailings.is_some();
    div()
        .id("docs-mailings")
        .flex_none()
        .size(CONTROL_SM)
        .flex()
        .items_center()
        .justify_center()
        .rounded(RADIUS_MD)
        .cursor_pointer()
        .when(open, |d| d.bg(tok.selected).text_color(tok.fg))
        .when(!open, |d| {
            d.text_color(tok.fg_muted)
                .hover(|s| s.bg(tok.hover).text_color(tok.fg))
        })
        .child(Icon::new(Lucide::Mail).with_size(ICON_MD))
        .tooltip(|window, cx| {
            Tooltip::new(i18n::t("Mailings: merge fields filled in from a list")).build(window, cx)
        })
        .on_click(move |_, window, cx| {
            let _ = v.update(cx, |this, cx| this.toggle_mailings_doc(window, cx));
        })
}

/// Where the merged documents of a template go by default.
pub(crate) fn _default_dir(doc: &docs_engine::Document) -> Option<PathBuf> {
    doc.path().and_then(Path::parent).map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn data_sources_are_linked_relative_to_the_document() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("data");
        std::fs::create_dir(&sub).unwrap();
        let file = sub.join("staff list.xlsx");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(
            relative_source(&file, Some(dir.path())),
            "data/staff list.xlsx"
        );
        let docs = dir.path().join("letters");
        std::fs::create_dir(&docs).unwrap();
        assert_eq!(
            relative_source(&file, Some(&docs)),
            "../data/staff list.xlsx"
        );
        assert!(relative_source(&file, None).ends_with("staff list.xlsx"));
    }
}
