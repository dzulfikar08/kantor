//! The sidebar: page thumbnails or the document's bookmarks.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::components::{IconButtonExt as _, icon_button};
use kantor_ui::i18n;
use kantor_ui::tokens::Elevation;
use kantor_ui::{metrics::*, tokens};
use pdf_engine::{LinkTarget, OutlineItem};

use crate::worker::PageJob;
use crate::{Panel, PdfView, SIDEBAR_W, THUMB_H, THUMB_W};

/// One thumbnail row: the box plus its page number, px.
const THUMB_ROW: f32 = THUMB_H + 34.0;

pub(crate) fn render(
    view: &PdfView,
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
        .child(
            div()
                .flex_1()
                .text_size(FONT_SMALL)
                .text_color(t.fg_muted)
                .pl(SPACE_3)
                .child(match panel {
                    Panel::Thumbnails => i18n::t("Pages"),
                    Panel::Bookmarks => i18n::t("Bookmarks"),
                }),
        );
    let body = match panel {
        Panel::Thumbnails => thumbnails(view, window, cx),
        Panel::Bookmarks => bookmarks(view, cx),
    };
    v_flex()
        .flex_none()
        .w(px(SIDEBAR_W))
        .h_full()
        .bg(t.chrome)
        .border_r_1()
        .border_color(t.border)
        .child(header)
        .child(div().flex_1().min_h_0().child(body))
        .into_any_element()
}

fn thumbnails(view: &PdfView, window: &mut Window, cx: &mut Context<PdfView>) -> AnyElement {
    let t = tokens(cx);
    let n = view.page_count();
    let sizes = view.doc().map(|d| d.sizes.clone()).unwrap_or_default();
    let turn = view.turn;
    let current = view.current_page();
    let weak = cx.entity().downgrade();
    let thumbs = view.thumbs.clone();
    let worker = view.worker.clone();
    let sf = window.scale_factor();
    uniform_list("pdf-thumb-list", n, move |range, _, _| {
        // Draw the thumbnails in view (replacing those asked before).
        let mut jobs = Vec::new();
        let boxes: Vec<(usize, f32, f32, f32)> = range
            .clone()
            .map(|i| {
                let (w, h) = sizes.get(i).copied().unwrap_or((612.0, 792.0));
                let (w, h) = turn.size(w, h);
                let k = (THUMB_W / w).min(THUMB_H / h);
                (i, w * k, h * k, k)
            })
            .collect();
        for &(i, _, _, k) in &boxes {
            if !thumbs.borrow().has(i, turn) {
                jobs.push(PageJob {
                    page: i,
                    scale: k * sf,
                    turn,
                });
            }
        }
        if let Some(w) = &worker {
            w.want_thumbs(jobs);
        }
        boxes
            .into_iter()
            .map(|(i, w, h, k)| {
                let image = thumbs.borrow_mut().get(i, turn, k * sf).map(|(i, _)| i);
                let on = i == current;
                let weak = weak.clone();
                v_flex()
                    .id(("pdf-thumb", i))
                    .h(px(THUMB_ROW))
                    .w_full()
                    .items_center()
                    .justify_center()
                    .gap(SPACE_2)
                    .cursor_pointer()
                    .when(!on, |d| d.hover(|d| d.bg(t.hover)))
                    .child(
                        div()
                            .w(px(w + 4.0))
                            .h(px(h + 4.0))
                            .rounded(RADIUS_SM)
                            .border_2()
                            .border_color(if on { t.accent } else { transparent_black() })
                            .child(
                                div()
                                    .size_full()
                                    .bg(t.paper)
                                    .border_1()
                                    .border_color(t.border_strong)
                                    .shadow(t.shadow(Elevation::Raised))
                                    .overflow_hidden()
                                    .children(image.map(|i| img(i).size_full())),
                            ),
                    )
                    .child(
                        div()
                            .text_size(FONT_CAPTION)
                            .text_color(if on { t.accent } else { t.fg_dim })
                            .when(on, |d| d.font_weight(FontWeight::SEMIBOLD))
                            .child(format!("{}", i + 1)),
                    )
                    .on_click(move |_, window, cx| {
                        let _ = weak.update(cx, |v, cx| {
                            v.go_to_page(i, cx);
                            v.focus(window, cx);
                        });
                    })
            })
            .collect()
    })
    .size_full()
    .into_any_element()
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
    for (i, it) in items.iter().enumerate() {
        path.push(i);
        let open = view.expanded.contains(path);
        out.push(Row {
            path: path.clone(),
            depth: path.len() - 1,
            title: it.title.clone(),
            target: it.target.clone(),
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
    let outline = view.doc().map(|d| d.outline.as_slice()).unwrap_or(&[]);
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
                            .text_color(t.fg)
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
