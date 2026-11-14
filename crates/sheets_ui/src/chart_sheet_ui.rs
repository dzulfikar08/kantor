//! Chart sheets (a tab that is one chart): the bar over the chart, its tab's
//! right-click menu and the Rename prompt.
//!
//! Showing a chart sheet selects its chart, as Excel does, so the Chart
//! panel (`chart_ui`, the same one embedded charts use) edits it; the bar
//! opens the panel. The tab menu has Excel's Rename, Delete, Move Left /
//! Move Right, Hide and Unhide; each is one undo step
//! (`sheets_engine::chart_sheet`).

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{self, Input, InputEvent, InputState},
    menu::{PopupMenu, PopupMenuItem},
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::{metrics::*, tokens};
use sheets_engine::Tab;

use crate::SheetsView;

/// The Rename prompt for a chart sheet's tab.
pub(crate) struct ChartRename {
    k: usize,
    input: Entity<InputState>,
    _sub: Subscription,
}

impl SheetsView {
    /// The first chart of chart sheet `k`, which showing it selects.
    pub(crate) fn chart_sheet_chart(
        &self,
        k: usize,
        cx: &App,
    ) -> Option<sheets_engine::drawing::DrawingKey> {
        self.table
            .read(cx)
            .delegate()
            .book
            .chart_sheet_drawings(k)
            .into_iter()
            .find(|d| d.editable && !d.hidden)
            .map(|d| d.key)
    }

    /// Run a chart sheet tab command on the workbook (one undo step) and
    /// say what happened.
    fn chart_tab_edit(
        &mut self,
        done: String,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut sheets_engine::Workbook) -> anyhow::Result<()>,
    ) -> bool {
        if self.editing.is_some() {
            self.commit_edit(None, window, cx);
        }
        if self.blocked(cx) {
            return false;
        }
        let res = self.table.update(cx, |table, cx| {
            let r = f(&mut table.delegate_mut().book);
            table.refresh(cx);
            r
        });
        let ok = res.is_ok();
        self.status = match res {
            Ok(()) => done.into(),
            Err(e) => format!("{e:#}").into(),
        };
        // A chart sheet deleted or hidden while shown: back to the grid.
        if self.chart_tab.is_some() && !self.on_chart_sheet(cx) {
            self.leave_chart_sheet(window, cx);
        }
        self.update_title(window, cx);
        cx.notify();
        ok
    }

    /// Show the grid's sheet again (the chart sheet shown went away).
    pub(crate) fn leave_chart_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.chart_tab = None;
        self.drawings.borrow_mut().selected = None;
        self.focus_grid(window, cx);
        cx.notify();
    }

    fn chart_tab_name(&self, k: usize, cx: &App) -> String {
        self.table
            .read(cx)
            .delegate()
            .book
            .chart_sheet_name(k)
            .unwrap_or_default()
    }

    pub(crate) fn delete_chart_tab(
        &mut self,
        k: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self.chart_tab_name(k, cx);
        self.chart_tab_edit(
            format!("Deleted chart sheet \u{201c}{name}\u{201d} (Undo brings it back)"),
            window,
            cx,
            |b| b.delete_chart_sheet(k),
        );
    }

    pub(crate) fn hide_chart_tab(&mut self, k: usize, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.chart_tab_name(k, cx);
        self.chart_tab_edit(format!("Hid \u{201c}{name}\u{201d}"), window, cx, |b| {
            b.set_chart_sheet_hidden(k, true)
        });
    }

    pub(crate) fn unhide_chart_tab(
        &mut self,
        k: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self.chart_tab_name(k, cx);
        if self.chart_tab_edit(format!("Unhid \u{201c}{name}\u{201d}"), window, cx, |b| {
            b.set_chart_sheet_hidden(k, false)
        }) {
            // As Excel: the sheet unhidden is shown.
            self.select_chart_sheet(k, window, cx);
        }
    }

    /// Move chart sheet `k`'s tab one place left (`-1`) or right (`1`).
    pub(crate) fn move_chart_tab(
        &mut self,
        k: usize,
        by: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tabs = self.table.read(cx).delegate().book.tabs();
        let Some(at) = tabs.iter().position(|t| *t == Tab::Chart(k)) else {
            return;
        };
        let Some(to) = at.checked_add_signed(by).filter(|to| *to < tabs.len()) else {
            return;
        };
        self.chart_tab_edit("Moved".into(), window, cx, |b| b.move_chart_sheet(k, to));
    }

    /// Open the Rename prompt for chart sheet `k`.
    pub(crate) fn rename_chart_tab(
        &mut self,
        k: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let name = self.chart_tab_name(k, cx);
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Sheet name"));
        input.update(cx, |i, cx| i.set_value(name, window, cx));
        let sub = cx.subscribe_in(&input, window, Self::on_chart_rename_input);
        // Focused once the menu that asked for it has closed (closing it
        // gives focus back to what had it).
        let focus = input.clone();
        window.defer(cx, move |window, cx| {
            focus.update(cx, |i, cx| i.focus(window, cx));
        });
        self.chart_rename = Some(ChartRename {
            k,
            input,
            _sub: sub,
        });
        cx.notify();
    }

    fn on_chart_rename_input(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::PressEnter { .. } => {
                let Some(k) = self.chart_rename.as_ref().map(|r| r.k) else {
                    return;
                };
                let name = input.read(cx).value().trim().to_owned();
                if self.chart_tab_edit(
                    format!("Renamed to \u{201c}{name}\u{201d}"),
                    window,
                    cx,
                    |b| b.rename_chart_sheet(k, &name),
                ) {
                    self.close_chart_rename(window, cx);
                }
            }
            InputEvent::Blur => self.close_chart_rename(window, cx),
            _ => {}
        }
    }

    fn close_chart_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chart_rename.take().is_some() {
            self.refocus(window, cx);
            cx.notify();
        }
    }

    /// Focus what shows: the chart sheet or the grid.
    pub(crate) fn refocus(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chart_tab.is_some() {
            window.focus(&self.chart_focus, cx);
        } else {
            self.focus_grid(window, cx);
        }
    }

    /// The Rename prompt, over the sheet tabs.
    pub(crate) fn render_chart_rename(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let r = self.chart_rename.as_ref()?;
        let t = tokens(cx);
        Some(
            div()
                .absolute()
                .bottom(CONTROL_LG + SPACE_3)
                .left(SPACE_6)
                .w(px(300.))
                .p(SPACE_4)
                .rounded(RADIUS_MD)
                .bg(t.surface)
                .border_1()
                .border_color(t.border_strong)
                .shadow_md()
                .on_action(
                    cx.listener(|this, _: &input::Escape, w, cx| this.close_chart_rename(w, cx)),
                )
                .child(
                    div()
                        .text_size(FONT_SMALL)
                        .text_color(t.fg_muted)
                        .mb(SPACE_3)
                        .child("Rename chart sheet"),
                )
                .child(
                    h_flex()
                        .gap(SPACE_3)
                        .child(div().flex_1().child(Input::new(&r.input).small()))
                        .child(
                            div()
                                .text_size(FONT_SMALL)
                                .text_color(t.fg_dim)
                                .child("Enter to apply"),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The bar over a chart sheet: its name and the Chart panel button.
    pub(crate) fn render_chart_sheet_bar(
        &self,
        name: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let open = self.chart_panel.is_some();
        h_flex()
            .flex_none()
            .h(CONTROL_LG)
            .px(SPACE_4)
            .gap(SPACE_3)
            .bg(t.chrome)
            .border_b_1()
            .border_color(t.border)
            .text_size(FONT_SMALL)
            .text_color(t.fg_muted)
            .child(div().flex_1().min_w_0().truncate().child(format!(
                "Chart sheet \u{201c}{name}\u{201d} \u{b7} its data is on the worksheets"
            )))
            .child(
                Button::new("chart-sheet-panel")
                    .xsmall()
                    .when_else(open, |b| b.primary(), |b| b.ghost())
                    .icon(Lucide::ChartColumn)
                    .label("Chart")
                    .tooltip("Change the chart's type, titles, legend, colours and data")
                    .on_click(move |_, w, cx| {
                        let _ = view.update(cx, |this, cx| this.chart_options_doc(w, cx));
                    }),
            )
    }
}

/// The right-click menu of chart sheet `k`'s tab, as Excel's.
pub(crate) fn chart_tab_menu(
    menu: PopupMenu,
    k: usize,
    first: bool,
    last: bool,
    hidden: &[(usize, String)],
    view: &WeakEntity<SheetsView>,
) -> PopupMenu {
    let item =
        |label: &'static str,
         f: fn(&mut SheetsView, usize, &mut Window, &mut Context<SheetsView>)| {
            let view = view.clone();
            PopupMenuItem::new(label).on_click(move |_, window, cx| {
                let _ = view.update(cx, |v, cx| f(v, k, window, cx));
            })
        };
    let menu = menu
        .item(item("Rename", SheetsView::rename_chart_tab))
        .item(item("Delete", SheetsView::delete_chart_tab))
        .separator()
        .item(item("Move Left", |v, k, w, cx| v.move_chart_tab(k, -1, w, cx)).disabled(first))
        .item(item("Move Right", |v, k, w, cx| v.move_chart_tab(k, 1, w, cx)).disabled(last))
        .separator()
        .item(item("Hide", SheetsView::hide_chart_tab));
    unhide_items(menu, hidden, view)
}

/// "Unhide “name”" for each hidden chart sheet (Excel's Unhide… list).
pub(crate) fn unhide_items(
    mut menu: PopupMenu,
    hidden: &[(usize, String)],
    view: &WeakEntity<SheetsView>,
) -> PopupMenu {
    for (k, name) in hidden {
        let (k, view) = (*k, view.clone());
        menu = menu.item(
            PopupMenuItem::new(format!("Unhide \u{201c}{name}\u{201d}")).on_click(
                move |_, window, cx| {
                    let _ = view.update(cx, |v, cx| v.unhide_chart_tab(k, window, cx));
                },
            ),
        );
    }
    menu
}
