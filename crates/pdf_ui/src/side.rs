//! The sidebar: page thumbnails or the document's bookmarks.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::components::{IconButtonExt as _, icon_button};
use kantor_ui::i18n;
use kantor_ui::{metrics::*, tokens};
use pdf_engine::{LinkTarget, OutlineItem};

use crate::grid::{Cells, ListKind, page_list};
use crate::{Panel, PdfView, SIDEBAR_W};

pub(crate) fn render(
    view: &mut PdfView,
    panel: Panel,
    window: &mut Window,
    cx: &mut Context<PdfView>,
) -> AnyElement {
    let t = tokens(cx);
    let tab = |id: &'static str, icon: Lucide, tip: &'static str, which: Panel| {
        let v = cx.entity().downgrade();
        icon_button(id, icon, panel == which, cx)
            .tip(tip, None)
            .on_click(move |_, _, cx| {
                let _ = v.update(cx, |v, cx| {
                    v.panel = Some(which);
                    cx.notify();
                });
            })
    };
    let header = h_flex()
        .flex_none()
        .h(CONTROL_LG)
        .px(SPACE_4)
        .gap(SPACE_2)
        .border_b_1()
        .border_color(t.border)
        .child(tab(
            "pdf-thumbs",
            Lucide::GalleryVerticalEnd,
            i18n::t("Page thumbnails"),
            Panel::Thumbnails,
        ))
        .child(tab(
            "pdf-bookmarks",
            Lucide::Bookmark,
            i18n::t("Bookmarks"),
            Panel::Bookmarks,
        ))
        .child(tab(
            "pdf-comments-tab",
            Lucide::MessageSquareText,
            i18n::t("Comments"),
            Panel::Comments,
        ))
        .when(view.sigs.signed() || panel == Panel::Signatures, |d| {
            d.child(tab(
                "pdf-signatures",
                Lucide::Signature,
                i18n::t("Signatures"),
                Panel::Signatures,
            ))
        })
        .child(
            div()
                .flex_1()
                .text_size(FONT_SMALL)
                .text_color(t.fg_muted)
                .pl(SPACE_3)
                .child(match panel {
                    Panel::Thumbnails => i18n::t("Pages"),
                    Panel::Bookmarks => i18n::t("Bookmarks"),
                    Panel::Comments => i18n::t("Comments"),
                    Panel::Signatures => i18n::t("Signatures"),
                }),
        );
    let body = match panel {
        Panel::Thumbnails => {
            let el = thumbnails(view, window, cx);
            view.thumb_followed = Some(view.current_page());
            el
        }
        Panel::Bookmarks => bookmarks(view, cx),
        Panel::Comments => view.render_comments(cx),
        Panel::Signatures => crate::signatures::panel(view, cx),
    };
    v_flex()
        .flex_none()
        // Comments and signatures need room for names, times and details.
        .w(px(match panel {
            Panel::Comments => 280.0,
            Panel::Signatures => SIDEBAR_W * 1.6,
            _ => SIDEBAR_W,
        }))
        .h_full()
        .bg(t.chrome)
        .border_r_1()
        .border_color(t.border)
        .child(header)
        .child(div().flex_1().min_h_0().child(body))
        .into_any_element()
}

fn thumbnails(view: &PdfView, window: &mut Window, cx: &mut Context<PdfView>) -> AnyElement {
    let cells = Cells::new(view, ListKind::Sidebar, window, cx);
    let n = cells.len();
    let current = view.current_page();
    let scroll = view.thumb_scroll.clone();
    if view.thumb_followed != Some(current) && view.page_sel.len() <= 1 {
        scroll.scroll_to_item(current, ScrollStrategy::Center);
    }
    let list = uniform_list("pdf-thumb-list", n, move |range, _, cx| {
        // Draw the thumbnails in view (replacing those asked before).
        cells.request(range.clone());
        range.map(|i| cells.cell(i, cx)).collect()
    })
    .track_scroll(&scroll)
    .size_full()
    .into_any_element();
    page_list(list, ListKind::Sidebar, view, cx).into_any_element()
}

/// A visible bookmark row.
struct Row {
    path: Vec<usize>,
    depth: usize,
    title: String,
    target: Option<LinkTarget>,
    has_children: bool,
    open: bool,
}

fn rows(items: &[OutlineItem], view: &PdfView, path: &mut Vec<usize>, out: &mut Vec<Row>) {
    let base = view.doc().map(|d| d.model.base().id);
    for (i, it) in items.iter().enumerate() {
        path.push(i);
        let open = view.expanded.contains(path);
        // Where the bookmark's page is now (none when it was deleted).
        let target = match (it.target.clone(), base) {
            (Some(t), Some(b)) => view.map_target(b, t),
            _ => None,
        };
        out.push(Row {
            path: path.clone(),
            depth: path.len() - 1,
            title: it.title.clone(),
            target,
            has_children: !it.children.is_empty(),
            open,
        });
        if open {
            rows(&it.children, view, path, out);
        }
        path.pop();
    }
}

fn bookmarks(view: &PdfView, cx: &mut Context<PdfView>) -> AnyElement {
    let t = tokens(cx);
    let outline = view.outline();
    if outline.is_empty() {
        return div()
            .p(SPACE_6)
            .text_size(FONT_SMALL)
            .text_color(t.fg_muted)
            .child(i18n::t("This document has no bookmarks."))
            .into_any_element();
    }
    let mut list = Vec::new();
    rows(outline, view, &mut Vec::new(), &mut list);
    let weak = cx.entity().downgrade();
    let count = list.len();
    let list = std::rc::Rc::new(list);
    uniform_list("pdf-bookmark-list", count, move |range, _, _| {
        range
            .map(|i| {
                let r = &list[i];
                let (w1, w2) = (weak.clone(), weak.clone());
                let path = r.path.clone();
                let target = r.target.clone();
                let toggle = r.has_children.then(|| {
                    let path = path.clone();
                    div()
                        .id(("pdf-bm-toggle", i))
                        .flex_none()
                        .size(px(16.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(t.fg_muted)
                        .child(
                            gpui_kit::component::Icon::new(if r.open {
                                Lucide::ChevronDown
                            } else {
                                Lucide::ChevronRight
                            })
                            .with_size(ICON_SM),
                        )
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            cx.stop_propagation();
                            let _ = w1.update(cx, |v, cx| {
                                if !v.expanded.remove(&path) {
                                    v.expanded.insert(path.clone());
                                }
                                cx.notify();
                            });
                        })
                });
                h_flex()
                    .id(("pdf-bm", i))
                    .h(px(26.))
                    .w_full()
                    .pl(px(8.0 + 14.0 * r.depth as f32))
                    .pr(SPACE_4)
                    .gap(SPACE_2)
                    .cursor_pointer()
                    .hover(|d| d.bg(t.hover))
                    .child(match toggle {
                        Some(tg) => tg.into_any_element(),
                        None => div().flex_none().w(px(16.)).into_any_element(),
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(FONT_SMALL)
                            .text_color(if r.target.is_some() || r.has_children {
                                t.fg
                            } else {
                                t.fg_dim
                            })
                            .child(r.title.clone()),
                    )
                    .on_click(move |_, window, cx| {
                        let Some(target) = target.clone() else {
                            return;
                        };
                        let _ = w2.update(cx, |v, cx| {
                            v.follow(target, window, cx);
                        });
                    })
            })
            .collect()
    })
    .size_full()
    .into_any_element()
}
