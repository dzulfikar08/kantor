//! PLS-SEM in Sheets (SmartPLS's job, see docs/decisions/0006-pls-sem.md).
//!
//! **Data ▸ PLS-SEM…** (toolbar, palette) takes the selected cells, or the
//! region around the active cell, with the header row naming the
//! indicators, and opens a *model tab*: a tab in the sheet tab bar, like a
//! chart sheet, that shows the model's path diagram instead of the grid.
//! Models are saved in the workbook (a custom XML part, see
//! `sheets_engine::pls_models`), so reopening the file brings the diagram
//! back; **Open Model…** adds one from a pls-json file.
//!
//! - [`diagram`]: the model with its layout, editing with undo, auto-layout
//!   and the geometry (no GPUI);
//! - [`labels`]: the numbers on the diagram and the result tables with
//!   their rule-of-thumb flags (no GPUI);
//! - [`data`]: the data range read from the workbook;
//! - `view`: the tab itself ([`PlsView`]): indicator list, canvas,
//!   settings, results; estimation on another thread with progress and
//!   Stop.
//!
//! Reports (.docx through the docs builder, .xlsx through the sheets
//! builder) and the AI summary live in the `kantor` crate: it registers
//! them as [`PlsHooks`].

pub mod data;
pub mod diagram;
pub mod labels;
mod view;

use std::path::Path;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_pls::Results;
use kantor_ui::i18n::{t, tf};
use kantor_ui::{metrics::*, tokens};
use sheets_engine::pls_models::PlsModel;
use sheets_engine::{CellRange, StyleChange};

pub use view::{PlsEvent, PlsView};

use crate::{SheetsEvent, SheetsView};

/// What the app adds: the report writer (`.docx` in English, or
/// Indonesian when the flag is set, and the results `.xlsx`, by the path's
/// extension) and the compact JSON summary Paparan AI reads.
pub struct PlsHooks {
    pub write: fn(&Results, &Path, bool) -> anyhow::Result<()>,
    pub summary: fn(&Results) -> String,
}

impl Global for PlsHooks {}

/// The workbook's model tabs.
#[derive(Default)]
pub(crate) struct PlsTabs {
    /// One view per model, in the workbook's order; `None` until read from
    /// the workbook.
    views: Option<Vec<(Entity<PlsView>, Subscription)>>,
    /// The model tab shown instead of the grid.
    pub(crate) shown: Option<usize>,
}

impl SheetsView {
    /// Read the workbook's models into views (once it has opened).
    pub(crate) fn pls_ensure(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pls.views.is_some() || self.busy.is_some() {
            return;
        }
        let models = self
            .table
            .read(cx)
            .delegate()
            .book
            .pls_models()
            .unwrap_or_default();
        let mut views = Vec::new();
        for m in models {
            let mut d = match diagram::Diagram::from_json(&m.json) {
                Ok(d) => d,
                Err(e) => {
                    log::warn!("PLS-SEM model {:?} can't be read: {e}", m.name);
                    continue;
                }
            };
            if d.layout().name.is_none() {
                d.set_name(&m.name);
            }
            views.push(self.pls_make_view(d, window, cx));
        }
        self.pls.views = Some(views);
    }

    fn pls_make_view(
        &mut self,
        d: diagram::Diagram,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<PlsView>, Subscription) {
        let (columns, cases) = {
            let book = &self.table.read(cx).delegate().book;
            let (sheet, range) = d.data_ref();
            match data::locate(book, sheet.unwrap_or_default(), range.unwrap_or_default()) {
                Some((s, r)) => {
                    let (rows, _) = book.used_size(s);
                    let bottom = r.bottom.min(rows.saturating_sub(1));
                    (
                        data::column_names(book, s, r),
                        Some(bottom.saturating_sub(r.top) as usize),
                    )
                }
                None => (Vec::new(), None),
            }
        };
        let me = cx.entity().downgrade();
        let view = cx.new(|cx| PlsView::new(me, d, columns, cases, window, cx));
        let sub = cx.subscribe_in(
            &view,
            window,
            |this, _, ev: &PlsEvent, window, cx| match ev {
                PlsEvent::Changed => this.pls_store(window, cx),
                PlsEvent::InsertTables(tables) => this.pls_insert_tables(tables, window, cx),
                PlsEvent::Explain(prompt, files) => cx.emit(SheetsEvent::ExplainPls {
                    prompt: prompt.clone(),
                    files: files.clone(),
                }),
            },
        );
        (view, sub)
    }

    /// Write the models into the workbook (saved with it).
    fn pls_store(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(views) = &self.pls.views else {
            return;
        };
        let models: Vec<PlsModel> = views
            .iter()
            .map(|(v, _)| {
                let v = v.read(cx);
                PlsModel::new(&v.name(), &v.diagram.to_json())
            })
            .collect();
        self.table.update(cx, |table, _| {
            table.delegate_mut().book.set_pls_models(models)
        });
        self.update_title(window, cx);
        cx.notify();
    }

    /// The data for a model: the selection, or the region around the active
    /// cell when one cell is selected. `(sheet, range, columns, cases)`.
    pub(crate) fn pls_data_range(&self, cx: &App) -> Option<(String, String, Vec<String>, usize)> {
        if self.on_chart_sheet(cx) {
            return None;
        }
        let sheet = self.grid_sheet(cx);
        let mut range = self.range(cx);
        let book = &self.table.read(cx).delegate().book;
        if range.top == range.bottom && range.left == range.right {
            range = book.current_region(sheet, range.top, range.left);
        }
        let (rows, cols) = book.used_size(sheet);
        range.bottom = range.bottom.min(rows.saturating_sub(1));
        range.right = range.right.min(cols.saturating_sub(1));
        if rows == 0 || range.bottom <= range.top || range.right < range.left {
            return None;
        }
        let columns = data::column_names(book, sheet, range);
        let name = book.sheet_names().get(sheet as usize)?.clone();
        Some((
            name,
            range.name(),
            columns,
            (range.bottom - range.top) as usize,
        ))
    }

    /// Data ▸ PLS-SEM…: a new model on the selected data, in its own tab.
    pub fn pls_new_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.blocked(cx) {
            return;
        }
        if self.editing.is_some() {
            self.commit_edit(None, window, cx);
        }
        self.pls_ensure(window, cx);
        let Some((sheet, range, _, _)) = self.pls_data_range(cx) else {
            self.status = t(
                "Select the survey data with its header row (one column per indicator), then choose PLS-SEM… again",
            )
            .into();
            cx.notify();
            return;
        };
        let n = self.pls.views.as_ref().map_or(0, Vec::len) + 1;
        let name = format!("{} {n}", t("Model"));
        let d = diagram::Diagram::new(&name, &sheet, &range);
        self.pls_add(d, window, cx);
    }

    fn pls_add(&mut self, d: diagram::Diagram, window: &mut Window, cx: &mut Context<Self>) {
        let made = self.pls_make_view(d, window, cx);
        let views = self.pls.views.get_or_insert_with(Vec::new);
        views.push(made);
        let k = views.len() - 1;
        self.pls_store(window, cx);
        self.select_pls_tab(k, window, cx);
    }

    /// Open Model…: a pls-json file (a model saved from here, or written for
    /// `kantor pls`) as a new model tab. A model without a data range takes
    /// the selected data.
    pub fn pls_open_model_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.blocked(cx) {
            return;
        }
        self.pls_ensure(window, cx);
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("Open a PLS-SEM model (.json)").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| this.pls_open_file(&path, window, cx));
        })
        .detach();
    }

    fn pls_open_file(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let read = std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|text| diagram::Diagram::from_json(&text));
        let mut d = match read {
            Ok(d) => d,
            Err(e) => {
                self.status = tf("Can't open the model: {}", &[&e]).into();
                cx.notify();
                return;
            }
        };
        if d.layout().name.is_none() {
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| t("Model").to_owned());
            d.set_name(&stem);
        }
        let known = {
            let book = &self.table.read(cx).delegate().book;
            let (s, r) = d.data_ref();
            s.zip(r)
                .is_some_and(|(s, r)| data::locate(book, s, r).is_some())
        };
        if !known {
            match self.pls_data_range(cx) {
                Some((sheet, range, _, _)) => d.set_data_ref(&sheet, &range),
                None => {
                    self.status =
                        t("Select the model's data with its header row first, then open the model")
                            .into();
                    cx.notify();
                    return;
                }
            }
        }
        self.pls_add(d, window, cx);
    }

    /// The model tabs: `(index, name)`.
    pub(crate) fn pls_tabs(&self, cx: &App) -> Vec<(usize, String)> {
        self.pls
            .views
            .iter()
            .flatten()
            .enumerate()
            .map(|(k, (v, _))| (k, v.read(cx).name()))
            .collect()
    }

    /// The model tab shown instead of the grid.
    pub(crate) fn shown_pls(&self) -> Option<Entity<PlsView>> {
        let k = self.pls.shown?;
        self.pls.views.as_ref()?.get(k).map(|(v, _)| v.clone())
    }

    /// Show model tab `k` in place of the grid.
    pub(crate) fn select_pls_tab(&mut self, k: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            self.commit_edit(None, window, cx);
        }
        self.chart_tab = None;
        self.pls.shown = Some(k);
        if let Some(v) = self.shown_pls() {
            v.update(cx, |v, cx| v.focus(window, cx));
        }
        cx.notify();
    }

    /// Leave the model tab for the grid.
    pub(crate) fn leave_pls(&mut self) -> bool {
        self.pls.shown.take().is_some()
    }

    /// Delete model tab `k` (the workbook keeps the others).
    pub(crate) fn pls_delete(&mut self, k: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(views) = &mut self.pls.views else {
            return;
        };
        if k >= views.len() {
            return;
        }
        views.remove(k);
        self.pls.shown = None;
        self.pls_store(window, cx);
        self.status = t("Model deleted").into();
        self.focus_grid(window, cx);
        cx.notify();
    }

    /// Insert as new sheet: the result tables on a new worksheet at the
    /// end, as one undo step.
    fn pls_insert_tables(
        &mut self,
        tables: &[labels::ResultTable],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cells = labels::sheet_cells(tables);
        let base = t("PLS Results");
        let name = {
            let book = &self.table.read(cx).delegate().book;
            (1..)
                .map(|n| {
                    if n == 1 {
                        base.to_owned()
                    } else {
                        format!("{base} {n}")
                    }
                })
                .find(|n| book.sheet_name_taken(n, None).is_ok())
                .unwrap_or_else(|| base.to_owned())
        };
        self.pls.shown = None;
        let res = self.tab_edit(t("Inserted the results").into(), window, cx, |b| {
            b.batch(|b| {
                let at = b.tabs().len();
                let s = b.insert_sheet(at)?;
                b.rename_sheet(s, &name)?;
                b.set_inputs(s, &cells.cells)?;
                let one = |r: u32, c: u32| CellRange {
                    top: r,
                    left: c,
                    bottom: r,
                    right: c,
                };
                for &(r, c0, c1) in &cells.bold {
                    let range = CellRange {
                        top: r,
                        left: c0,
                        bottom: r,
                        right: c1,
                    };
                    b.set_range_style(s, range, StyleChange::Bold(true))?;
                }
                for &(r, c) in &cells.numbers {
                    b.set_range_style_value(s, one(r, c), "num_fmt", "0.000")?;
                }
                for &(r, c) in &cells.flagged {
                    b.set_range_style(s, one(r, c), StyleChange::Fill(Some(0xFC_E4E4)))?;
                }
                b.set_column_width(s, 0, 0, 180.0)?;
                if cells.cols > 1 {
                    b.set_column_width(s, 1, cells.cols - 1, 90.0)?;
                }
                Ok(())
            })
        });
        if res.is_ok() {
            self.select_cell(0, 0, cx);
            self.load_selected_into_formula(window, cx);
            self.focus_grid(window, cx);
        }
    }

    /// The model tabs for the sheet tab bar (after the sheets).
    pub(crate) fn render_pls_tabs(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t_ = tokens(cx);
        let shown = self.pls.shown;
        self.pls_tabs(cx)
            .into_iter()
            .map(|(k, name)| {
                let view = cx.entity().downgrade();
                let menu_view = view.clone();
                let selected = shown == Some(k);
                div()
                    .id(("pls-tab", k))
                    .h(CONTROL_SM)
                    .px(SPACE_5)
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .rounded(RADIUS_SM)
                    .text_size(FONT_SMALL)
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .when_else(
                        selected,
                        |d| {
                            d.bg(t_.bg)
                                .border_1()
                                .border_color(t_.border_strong)
                                .text_color(t_.fg)
                        },
                        |d| d.text_color(t_.fg_muted).hover(|s| s.bg(t_.hover)),
                    )
                    .child(
                        gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Network)
                            .xsmall(),
                    )
                    .child(name)
                    .on_click(move |_, window, cx| {
                        let _ = view.update(cx, |this, cx| this.select_pls_tab(k, window, cx));
                    })
                    .context_menu(move |menu, _, _| {
                        let v = menu_view.clone();
                        menu.item(PopupMenuItem::new(t("Delete Model")).on_click(
                            move |_, window, cx| {
                                let _ = v.update(cx, |this, cx| this.pls_delete(k, window, cx));
                            },
                        ))
                    })
                    .into_any_element()
            })
            .collect()
    }
}
