//! Tab strip (doubles as the titlebar) and status bar.

use gpui_kit::component::{IconName, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_theme::catalog::ThemeId;
use kantor_ui::components::{IconButtonExt as _, dot, icon_button, kbd};
use kantor_ui::metrics::*;
use kantor_ui::tokens;

use super::Workspace;
use crate::actions::*;

/// Below this window width the tab strip drops its text labels.
const COMPACT_WINDOW: Pixels = px(900.);

impl Workspace {
    pub(super) fn render_tab_strip(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = tokens(cx);
        let compact = window.viewport_size().width < COMPACT_WINDOW;
        let entity = cx.entity().downgrade();

        let sidebar_toggle = (!self.ui.sidebar_open).then(|| {
            let entity = entity.clone();
            icon_button("show-sidebar", IconName::PanelLeft, false, cx)
                .tip("Show sidebar", Some("Ctrl+Shift+E"))
                .on_click(move |_, window, cx| {
                    let _ =
                        entity.update(cx, |ws, cx| ws.toggle_sidebar(&ToggleSidebar, window, cx));
                })
        });

        let home = {
            let entity = entity.clone();
            let selected = self.active.is_none();
            div()
                .id("tab-home")
                .flex_none()
                .size(TAB_HEIGHT)
                .flex()
                .items_center()
                .justify_center()
                .rounded(RADIUS_MD)
                .cursor_pointer()
                .when_else(
                    selected,
                    |d| d.bg(t.bg).border_1().border_color(t.border_strong),
                    |d| d.hover(|s| s.bg(t.hover)),
                )
                .child(img(self.logo.clone()).size(ICON_LG + SPACE_1))
                .on_click(move |_, window, cx| {
                    let _ = entity.update(cx, |ws, cx| ws.go_home(&GoHome, window, cx));
                })
        };

        let mut tabs = Vec::with_capacity(self.tabs.len());
        for (ix, tab) in self.tabs.iter().enumerate() {
            let selected = self.active == Some(ix);
            let panes = tab.layout.panes();
            let Some(doc) = self.docs.get(&tab.focused) else {
                continue;
            };
            let name = SharedString::from(doc.view.doc_name(cx));
            let app_color = doc.view.kind().color(&t);
            let dirty = panes
                .iter()
                .any(|p| self.docs.get(p).is_some_and(|d| d.view.is_dirty(cx)));
            let extra = panes.len().saturating_sub(1);
            let group = SharedString::from(format!("tab-{}", tab.id));
            let (on_select, on_close) = (entity.clone(), entity.clone());
            tabs.push(
                div()
                    .id(("tab", tab.id as usize))
                    .group(group.clone())
                    .flex_none()
                    .h(TAB_HEIGHT)
                    .pl(SPACE_5)
                    .pr(SPACE_2)
                    .flex()
                    .items_center()
                    .gap(SPACE_3)
                    .rounded(RADIUS_MD)
                    .text_size(FONT_SMALL)
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .when_else(
                        selected,
                        |d| {
                            d.bg(t.bg)
                                .border_1()
                                .border_color(t.border_strong)
                                .text_color(t.fg)
                        },
                        |d| d.text_color(t.fg_muted).hover(|s| s.bg(t.hover)),
                    )
                    .child(dot(if dirty { t.warning } else { app_color }))
                    .child(
                        div()
                            .w(if compact { px(96.) } else { TAB_TITLE_WIDTH })
                            .truncate()
                            .child(name),
                    )
                    .when(extra > 0, |d| {
                        d.child(
                            div()
                                .px(SPACE_2)
                                .rounded(RADIUS_SM)
                                .bg(t.surface)
                                .text_size(FONT_CAPTION)
                                .text_color(t.fg_muted)
                                .child(format!("+{extra}")),
                        )
                    })
                    .child(
                        div()
                            .id(("tab-close", tab.id as usize))
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(RADIUS_SM)
                            .text_color(t.fg_dim)
                            .hover(|s| s.bg(t.hover).text_color(t.fg))
                            .when(!selected, |d| {
                                d.invisible().group_hover(group, |s| s.visible())
                            })
                            .child("×")
                            .on_click(move |_, window, cx| {
                                cx.stop_propagation();
                                let _ = on_close.update(cx, |ws, cx| ws.close_tab(ix, window, cx));
                            }),
                    )
                    .on_click(move |_, window, cx| {
                        let _ = on_select.update(cx, |ws, cx| ws.activate(Some(ix), window, cx));
                    }),
            );
        }

        let new_tab = {
            let entity = entity.clone();
            icon_button("tab-new", IconName::Plus, false, cx)
                .tip("New spreadsheet", Some("Ctrl+N"))
                .on_click(move |_, window, cx| {
                    let _ =
                        entity.update(cx, |ws, cx| ws.new_spreadsheet(&NewSpreadsheet, window, cx));
                })
        };

        let in_doc = self.active.is_some();
        let split_right = {
            let entity = entity.clone();
            icon_button("split-right", IconName::LayoutDashboard, false, cx)
                .tip("Split right", Some("Ctrl+\\"))
                .on_click(move |_, window, cx| {
                    let _ = entity.update(cx, |ws, cx| ws.split_right(&SplitRight, window, cx));
                })
        };
        let split_down = {
            let entity = entity.clone();
            icon_button("split-down", IconName::PanelBottom, false, cx)
                .tip("Split down", Some("Ctrl+Alt+\\"))
                .on_click(move |_, window, cx| {
                    let _ = entity.update(cx, |ws, cx| ws.split_down(&SplitDown, window, cx));
                })
        };
        let inspector_toggle = {
            let entity = entity.clone();
            icon_button(
                "toggle-inspector",
                IconName::PanelRight,
                self.ui.inspector_open,
                cx,
            )
            .tip(
                if self.ui.inspector_open {
                    "Hide inspector"
                } else {
                    "Show inspector"
                },
                Some("Ctrl+Shift+I"),
            )
            .on_click(move |_, window, cx| {
                let _ = entity.update(cx, |ws, cx| {
                    ws.toggle_inspector(&ToggleInspector, window, cx)
                });
            })
        };

        let file_menu = self.render_file_menu(cx);

        let palette_hint = div()
            .id("palette-hint")
            .flex_none()
            .h(TAB_HEIGHT)
            .px(SPACE_4)
            .flex()
            .items_center()
            .gap(SPACE_3)
            .rounded(RADIUS_MD)
            .text_size(FONT_SMALL)
            .text_color(t.fg_muted)
            .cursor_pointer()
            .hover(|s| s.bg(t.hover))
            .when(!compact, |d| d.child("Commands"))
            .child(kbd("Ctrl K", cx))
            .on_click(move |_, window, cx| {
                let _ = entity.update(cx, |ws, cx| ws.toggle_palette(&TogglePalette, window, cx));
            });

        h_flex()
            .size_full()
            .px(SPACE_4)
            .gap(SPACE_2)
            .bg(t.chrome)
            .border_b_1()
            .border_color(t.border)
            .children(sidebar_toggle)
            .child(home)
            .child(
                h_flex()
                    .id("tab-list")
                    .flex_1()
                    .min_w_0()
                    .gap(SPACE_2)
                    .overflow_x_scroll()
                    .children(tabs)
                    .child(new_tab),
            )
            .when(in_doc, |d| d.child(split_right).child(split_down))
            .child(inspector_toggle)
            .child(file_menu)
            .child(palette_hint)
    }

    pub(super) fn render_status_bar(&mut self, cx: &mut Context<Self>) -> Div {
        let t = tokens(cx);
        let (app, color, detail, status, summary, calc, pivot) = match self.focused_view() {
            Some(view) => {
                let kind = view.kind();
                (
                    kind.name(),
                    kind.color(&t),
                    view.location(cx),
                    view.status(cx),
                    view.selection_summary(cx),
                    view.calc_status(cx),
                    view.pivot_status(cx),
                )
            }
            None => (
                "Home",
                t.accent,
                String::new(),
                self.message.clone(),
                None,
                None,
                None,
            ),
        };
        let status = self.notice.clone().unwrap_or(status);
        let cloud_chip = self.render_cloud_chip(cx);
        let panes = self
            .active
            .and_then(|i| self.tabs.get(i))
            .map_or(0, |tab| tab.layout.panes().len());
        let theme = match &crate::theme::current(cx).theme {
            ThemeId::FollowOmarchy => {
                kantor_theme::theme_name().unwrap_or_else(|| "default".to_owned())
            }
            ThemeId::Omarchy(name) | ThemeId::Ghostty(name) => name.clone(),
        };
        h_flex()
            .size_full()
            .px(SPACE_6)
            .gap(SPACE_6)
            .bg(t.chrome)
            .border_t_1()
            .border_color(t.border)
            .text_size(FONT_CAPTION)
            .text_color(t.fg_muted)
            .child(
                h_flex()
                    .gap(SPACE_3)
                    .child(dot(color))
                    .child(div().text_color(t.fg).child(app)),
            )
            .when(!detail.is_empty(), |d| d.child(div().child(detail)))
            // A cloud document: synced, uploading, offline, read-only…
            .children(cloud_chip)
            // The active cell is in a pivot table.
            .when_some(pivot, |d, pivot| {
                let ws = cx.entity().downgrade();
                d.child(
                    div()
                        .id("status-pivot")
                        .cursor_pointer()
                        .hover(|s| s.bg(t.accent.opacity(0.24)))
                        .on_click(move |_, window, cx| {
                            let _ = ws.update(cx, |ws, cx| {
                                ws.with_sheets(window, cx, |v, w, cx| v.pivot_fields_doc(w, cx))
                            });
                        })
                        .flex_none()
                        .px(SPACE_3)
                        .rounded(RADIUS_SM)
                        .bg(t.accent.opacity(0.14))
                        .text_color(t.accent)
                        .child(pivot),
                )
            })
            .when(panes > 1, |d| {
                d.child(div().child(format!("{panes} panes")))
            })
            .child(div().flex_1().min_w_0().truncate().child(status))
            // Excel's Sum / Average / Count for a selected range.
            .when_some(summary, |d, summary| {
                d.child(div().flex_none().text_color(t.fg).child(summary))
            })
            // Manual calculation, like Excel's "Calculate" in its status bar.
            .when_some(calc, |d, calc| {
                d.child(div().flex_none().text_color(t.warning).child(calc))
            })
            .when_some(self.updates.status_text(), |d, text| {
                d.child(
                    div()
                        .id("update-notice")
                        .flex_none()
                        .text_color(t.accent)
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .on_click(cx.listener(|ws, _, window, cx| ws.update_clicked(window, cx)))
                        .child(text),
                )
            })
            // Autosave switch, shown with a document in front.
            .when(self.focused_view().is_some(), |d| {
                let on = self.autosave_enabled();
                d.child(
                    div()
                        .id("autosave-toggle")
                        .flex_none()
                        .px(SPACE_3)
                        .rounded(RADIUS_SM)
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover))
                        .when(on, |d| d.text_color(t.fg))
                        .on_click(cx.listener(|ws, _, _, cx| ws.toggle_autosave(cx)))
                        .child(if on { "Autosave On" } else { "Autosave Off" }),
                )
            })
            .child(div().child(theme))
            .child(
                div()
                    .text_color(t.fg_dim)
                    .child(concat!("v", env!("CARGO_PKG_VERSION"))),
            )
    }
}
