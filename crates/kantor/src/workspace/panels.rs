//! Left sidebar (open documents, recent files, create) and right inspector
//! (details of the focused document). Both are resizable by dragging their edge.

use gpui_kit::component::{ActiveTheme as _, IconName, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::components::{dot, icon_button, section_label};
use kantor_ui::metrics::*;
use kantor_ui::tokens;

use super::doc::DocView;
use super::{AppKind, Drag, Workspace};
use crate::actions::*;

/// Width of the invisible drag strip on a panel edge.
const HANDLE: Pixels = px(6.);

impl Workspace {
    fn panel_header(
        &self,
        title: &'static str,
        close_id: &'static str,
        icon: IconName,
        on_close: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = tokens(cx);
        let entity = cx.entity().downgrade();
        h_flex()
            .flex_none()
            .h(TITLE_BAR_HEIGHT)
            .px(SPACE_5)
            .gap(SPACE_3)
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .flex_1()
                    .text_size(FONT_SMALL)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.fg)
                    .child(title),
            )
            .child(
                icon_button(close_id, icon, true, cx).on_click(move |_, window, cx| {
                    let _ = entity.update(cx, |ws, cx| on_close(ws, window, cx));
                }),
            )
    }

    fn sidebar_row(&self, id: impl Into<ElementId>, selected: bool, cx: &App) -> Stateful<Div> {
        let t = tokens(cx);
        div()
            .id(id)
            .h(ROW_HEIGHT)
            .px(SPACE_4)
            .flex()
            .items_center()
            .gap(SPACE_4)
            .rounded(RADIUS_MD)
            .text_size(FONT_SMALL)
            .cursor_pointer()
            .when_else(
                selected,
                |d| d.bg(t.selected).text_color(t.fg),
                |d| {
                    d.text_color(t.fg_muted)
                        .hover(|s| s.bg(t.hover).text_color(t.fg))
                },
            )
    }

    pub(super) fn render_sidebar(&mut self, cx: &mut Context<Self>) -> Div {
        let t = tokens(cx);
        let entity = cx.entity().downgrade();
        let header = self.panel_header(
            "Explorer",
            "hide-sidebar",
            IconName::PanelLeft,
            |ws, window, cx| ws.toggle_sidebar(&ToggleSidebar, window, cx),
            cx,
        );

        // Open documents
        let mut open = Vec::new();
        for (ix, tab) in self.tabs.iter().enumerate() {
            for pane in tab.layout.panes() {
                let Some(doc) = self.docs.get(&pane) else {
                    continue;
                };
                let v = &doc.view;
                let (name, dirty) = (v.doc_name(cx), v.is_dirty(cx));
                let app_color = v.kind().color(&t);
                let selected = self.active == Some(ix) && tab.focused == pane;
                let entity = entity.clone();
                open.push(
                    self.sidebar_row(("open-doc", pane as usize), selected, cx)
                        .child(dot(if dirty { t.warning } else { app_color }))
                        .child(div().flex_1().min_w_0().truncate().child(name))
                        .on_click(move |_, window, cx| {
                            let _ = entity.update(cx, |ws, cx| {
                                if let Some(tab) = ws.tabs.get_mut(ix) {
                                    tab.focused = pane;
                                }
                                ws.activate(Some(ix), window, cx);
                            });
                        }),
                );
            }
        }
        if open.is_empty() {
            open.push(
                div()
                    .id("open-empty")
                    .px(SPACE_4)
                    .text_size(FONT_SMALL)
                    .text_color(t.fg_dim)
                    .child("No open documents"),
            );
        }

        // Recent files
        let recent: Vec<_> = self
            .recent
            .iter()
            .filter(|p| p.is_file())
            .take(8)
            .cloned()
            .enumerate()
            .map(|(i, path)| {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let folder = path
                    .parent()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let entity = entity.clone();
                self.sidebar_row(("recent", i), false, cx)
                    .child(dot(t.fg_dim))
                    .child(div().flex_1().min_w_0().truncate().child(name))
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(90.))
                            .truncate()
                            .text_size(FONT_CAPTION)
                            .text_color(t.fg_dim)
                            .child(folder),
                    )
                    .on_click(move |_, window, cx| {
                        let path = path.clone();
                        let _ = entity.update(cx, |ws, cx| ws.open_path(path, window, cx));
                    })
            })
            .collect();

        // Create
        let create: Vec<_> = [
            (AppKind::Sheets, "New spreadsheet", t.sheets, true),
            (AppKind::Docs, "New document", t.docs, true),
            (AppKind::Slides, "New presentation · soon", t.slides, false),
        ]
        .into_iter()
        .map(|(app, label, color, ready)| {
            let entity = entity.clone();
            self.sidebar_row(("create", app as usize), false, cx)
                .when(!ready, |d| d.text_color(t.fg_dim))
                .child(dot(color))
                .child(label)
                .on_click(move |_, window, cx| {
                    let _ = entity.update(cx, |ws, cx| ws.launch(app, None, window, cx));
                })
        })
        .collect();

        let section = |label: &'static str, rows: Vec<Stateful<Div>>, cx: &App| {
            v_flex()
                .gap(SPACE_1)
                .child(
                    div()
                        .px(SPACE_4)
                        .pb(SPACE_2)
                        .child(section_label(label, cx)),
                )
                .children(rows)
        };
        let has_recent = !recent.is_empty();
        let open_section = section("Open", open, cx);
        let recent_section = section("Recent", recent, cx);
        let create_section = section("Create", create, cx);

        let handle = div()
            .id("sidebar-handle")
            .absolute()
            .top_0()
            .bottom_0()
            .right(-HANDLE / 2.)
            .w(HANDLE)
            .cursor(CursorStyle::ResizeLeftRight)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|ws, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let drag = Drag::Sidebar {
                        origin_x: ev.position.x,
                        origin_w: ws.ui.sidebar_width,
                    };
                    ws.start_drag(drag, cx);
                }),
            );

        v_flex()
            .relative()
            .size_full()
            .bg(t.chrome)
            .border_r_1()
            .border_color(t.border)
            .child(header)
            .child(
                v_flex()
                    .id("sidebar-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(SPACE_3)
                    .gap(SPACE_7)
                    .child(open_section)
                    .when(has_recent, |d| d.child(recent_section))
                    .child(create_section),
            )
            .child(handle)
    }

    pub(super) fn render_inspector(&mut self, cx: &mut Context<Self>) -> Div {
        let t = tokens(cx);
        let header = self.panel_header(
            "Inspector",
            "hide-inspector",
            IconName::PanelRight,
            |ws, window, cx| ws.toggle_inspector(&ToggleInspector, window, cx),
            cx,
        );
        let mono_family = cx.theme().mono_font_family.clone();
        let field = |label: &'static str, value: String, mono: bool| {
            v_flex()
                .gap(SPACE_1)
                .child(
                    div()
                        .text_size(FONT_CAPTION)
                        .text_color(t.fg_dim)
                        .child(label),
                )
                .child(
                    div()
                        .text_size(FONT_SMALL)
                        .text_color(if value.is_empty() { t.fg_dim } else { t.fg })
                        .when(mono, |d| d.font_family(mono_family.clone()))
                        .child(if value.is_empty() {
                            "—".to_owned()
                        } else {
                            value
                        }),
                )
        };

        let body = match self.focused_view() {
            Some(DocView::Docs(view)) => {
                let d = view.read(cx).info();
                let folder = d
                    .path
                    .as_deref()
                    .and_then(|p| p.parent())
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "Not saved yet".to_owned());
                let state = match (d.dirty, d.unsupported.is_empty()) {
                    (true, _) => "Unsaved changes",
                    (false, true) => "Saved",
                    (false, false) => "Opened with unsupported content",
                };
                v_flex()
                    .gap(SPACE_7)
                    .child(
                        v_flex()
                            .gap(SPACE_5)
                            .child(section_label("Paragraph", cx))
                            .child(field("Style", d.style, false))
                            .child(field("Font", d.font, false)),
                    )
                    .child(
                        v_flex()
                            .gap(SPACE_5)
                            .child(section_label("Document", cx))
                            .child(field("File", d.name, false))
                            .child(field("Folder", folder, false))
                            .child(field(
                                "Length",
                                format!(
                                    "{} pages · {} words · {} characters · {} paragraphs",
                                    d.pages, d.words, d.chars, d.paragraphs
                                ),
                                false,
                            ))
                            .child(field("State", state.to_owned(), false))
                            .when(!d.unsupported.is_empty(), |c| {
                                c.child(field(
                                    "Not shown or kept",
                                    format!(
                                        "{}. Save writes a new copy; the original is never overwritten.",
                                        d.unsupported.join(", ")
                                    ),
                                    false,
                                ))
                            }),
                    )
            }
            Some(DocView::Sheets(view)) => {
                let v = view.read(cx);
                let cell = v.cell_info(cx);
                let book = v.book_info(cx);
                let folder = book
                    .path
                    .as_deref()
                    .and_then(|p| p.parent())
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "Not saved yet".to_owned());
                let sheet = book
                    .sheets
                    .get(book.active_sheet)
                    .cloned()
                    .unwrap_or_default();
                let is_formula = cell.input.starts_with('=');
                v_flex()
                    .gap(SPACE_7)
                    .child(
                        v_flex()
                            .gap(SPACE_5)
                            .child(section_label("Cell", cx))
                            .child(field(
                                "Address",
                                format!("{} · {sheet}", cell.address),
                                false,
                            ))
                            .child(field(
                                if is_formula { "Formula" } else { "Content" },
                                cell.input,
                                is_formula,
                            ))
                            .child(field("Value", cell.value, false)),
                    )
                    .child(
                        v_flex()
                            .gap(SPACE_5)
                            .child(section_label("Workbook", cx))
                            .child(field("File", book.name, false))
                            .child(field("Folder", folder, false))
                            .child(field(
                                "Sheets",
                                format!("{} ({})", book.sheets.len(), book.sheets.join(", ")),
                                false,
                            ))
                            .child(field(
                                "Used range",
                                format!("{} rows × {} columns", book.used.0, book.used.1),
                                false,
                            ))
                            .child(field(
                                "State",
                                if book.dirty {
                                    "Unsaved changes".to_owned()
                                } else {
                                    "Saved".to_owned()
                                },
                                false,
                            )),
                    )
            }
            None => v_flex().child(
                div()
                    .text_size(FONT_SMALL)
                    .text_color(t.fg_dim)
                    .child("Open a document to see its details here."),
            ),
        };

        let handle = div()
            .id("inspector-handle")
            .absolute()
            .top_0()
            .bottom_0()
            .left(-HANDLE / 2.)
            .w(HANDLE)
            .cursor(CursorStyle::ResizeLeftRight)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|ws, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let drag = Drag::Inspector {
                        origin_x: ev.position.x,
                        origin_w: ws.ui.inspector_width,
                    };
                    ws.start_drag(drag, cx);
                }),
            );

        v_flex()
            .relative()
            .size_full()
            .bg(t.chrome)
            .border_l_1()
            .border_color(t.border)
            .child(header)
            .child(
                v_flex()
                    .id("inspector-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(SPACE_6)
                    .child(body),
            )
            .child(handle)
    }
}
