//! The PDF toolbar: sidebar, page box, zoom, rotate view, undo, Organize
//! Pages, the Tools menu, find and print.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::Input;
use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::components::{IconButtonExt as _, icon_button};
use kantor_ui::i18n::{self, tf, tk};
use kantor_ui::{metrics::*, tokens};

use crate::{Panel, PdfView, Zoom, layout};

type Run = fn(&mut PdfView, &mut Window, &mut Context<PdfView>);

fn tool(
    id: &'static str,
    icon: Lucide,
    (tip, keys): (&'static str, Option<&'static str>),
    active: bool,
    run: Run,
    cx: &mut Context<PdfView>,
) -> Button {
    let view = cx.entity().downgrade();
    icon_button(id, icon, active, cx)
        .tip(tip, keys)
        .on_click(move |_, window, cx| {
            let _ = view.update(cx, |v, cx| {
                run(v, window, cx);
                v.focus(window, cx);
            });
        })
}

fn separator(cx: &App) -> Div {
    div()
        .flex_none()
        .w(px(1.))
        .h(ICON_LG)
        .mx(SPACE_2)
        .bg(tokens(cx).border)
}

pub(crate) fn render(view: &PdfView, cx: &mut Context<PdfView>) -> Div {
    let t = tokens(cx);
    let n = view.page_count();
    let zoom_label = match view.zoom() {
        Zoom::FitWidth => i18n::t("Fit Width").to_owned(),
        Zoom::FitPage => i18n::t("Fit Page").to_owned(),
        Zoom::Percent(_) => format!("{:.0}%", view.zoom_value() * 100.0),
    };
    let weak = cx.entity().downgrade();
    h_flex()
        .flex_none()
        .h(CONTROL_LG + SPACE_3)
        .px(SPACE_4)
        .gap(SPACE_1)
        .bg(t.chrome)
        .border_b_1()
        .border_color(t.border)
        .child(tool(
            "pdf-sidebar",
            Lucide::PanelLeft,
            (i18n::t("Sidebar: thumbnails and bookmarks"), Some("f4")),
            view.panel().is_some(),
            |v, _, cx| {
                v.panel = match v.panel {
                    Some(_) => None,
                    None => Some(Panel::Thumbnails),
                };
                cx.notify();
            },
            cx,
        ))
        .child(separator(cx))
        .child(tool(
            "pdf-prev",
            Lucide::ChevronUp,
            (i18n::t("Previous page"), None),
            false,
            |v, _, cx| {
                let p = v.current_page().saturating_sub(1);
                v.go_to_page(p, cx)
            },
            cx,
        ))
        .child(tool(
            "pdf-next",
            Lucide::ChevronDown,
            (i18n::t("Next page"), None),
            false,
            |v, _, cx| {
                let p = (v.current_page() + 1).min(v.page_count().saturating_sub(1));
                v.go_to_page(p, cx)
            },
            cx,
        ))
        .child(
            div()
                .w(px(52.))
                .child(Input::new(&view.page_box).xsmall().cleanable(false)),
        )
        .child(
            div()
                .px(SPACE_3)
                .text_size(FONT_SMALL)
                .text_color(t.fg_muted)
                .child(tf("of {}", &[&n])),
        )
        .child(separator(cx))
        .child(tool(
            "pdf-zoom-out",
            Lucide::ZoomOut,
            (i18n::t("Zoom out"), Some("ctrl--")),
            false,
            |v, _, cx| v.zoom_step(false, None, cx),
            cx,
        ))
        .child(
            Button::new("pdf-zoom")
                .ghost()
                .xsmall()
                .label(zoom_label)
                .dropdown_caret(true)
                .tooltip(i18n::t("Zoom"))
                .dropdown_menu(move |menu, _, _| zoom_items(menu, &weak)),
        )
        .child(tool(
            "pdf-zoom-in",
            Lucide::ZoomIn,
            (i18n::t("Zoom in"), Some("ctrl-=")),
            false,
            |v, _, cx| v.zoom_step(true, None, cx),
            cx,
        ))
        .child(tool(
            "pdf-fit-width",
            Lucide::MoveHorizontal,
            (i18n::t("Fit Width"), Some("ctrl-2")),
            view.zoom() == Zoom::FitWidth,
            |v, _, cx| v.set_zoom(Zoom::FitWidth, None, cx),
            cx,
        ))
        .child(tool(
            "pdf-fit-page",
            Lucide::Maximize,
            (i18n::t("Fit Page"), Some("ctrl-0")),
            view.zoom() == Zoom::FitPage,
            |v, _, cx| v.set_zoom(Zoom::FitPage, None, cx),
            cx,
        ))
        .child(separator(cx))
        .child(tool(
            "pdf-rotate-ccw",
            Lucide::RotateCcw,
            (i18n::t("Rotate counterclockwise"), Some("ctrl-[")),
            false,
            |v, _, cx| v.rotate(false, cx),
            cx,
        ))
        .child(tool(
            "pdf-rotate-cw",
            Lucide::RotateCw,
            (i18n::t("Rotate clockwise"), Some("ctrl-]")),
            false,
            |v, _, cx| v.rotate(true, cx),
            cx,
        ))
        .child(separator(cx))
        .child(tool(
            "pdf-undo",
            Lucide::Undo,
            (i18n::t("Undo"), Some("ctrl-z")),
            false,
            |v, w, cx| v.undo_doc(w, cx),
            cx,
        ))
        .child(tool(
            "pdf-redo",
            Lucide::Redo,
            (i18n::t("Redo"), Some("ctrl-y")),
            false,
            |v, w, cx| v.redo_doc(w, cx),
            cx,
        ))
        .child(tool(
            "pdf-organize",
            Lucide::LayoutGrid,
            (i18n::t("Organize Pages"), Some("ctrl-shift-o")),
            view.is_organizing(),
            |v, w, cx| v.toggle_organize(w, cx),
            cx,
        ))
        .child({
            let weak = cx.entity().downgrade();
            Button::new("pdf-tools")
                .ghost()
                .xsmall()
                .icon(Lucide::Wrench)
                .label(i18n::t("Tools"))
                .dropdown_caret(true)
                .tooltip(i18n::t("Page tools"))
                .dropdown_menu(move |menu, window, cx| tool_items(menu, &weak, window, cx))
        })
        .child(div().flex_1())
        .child(tool(
            "pdf-find",
            Lucide::Search,
            (i18n::t("Find"), Some("ctrl-f")),
            false,
            |v, w, cx| v.open_find(false, w, cx),
            cx,
        ))
        .child(tool(
            "pdf-print",
            Lucide::Printer,
            (i18n::t("Print"), Some("ctrl-p")),
            false,
            |v, w, cx| v.print_doc(w, cx),
            cx,
        ))
}

/// The Tools menu: page tools on the whole document or the selected pages.
fn tool_items(
    menu: PopupMenu,
    view: &WeakEntity<PdfView>,
    _: &mut Window,
    _: &mut Context<PopupMenu>,
) -> PopupMenu {
    let item = |label: &'static str, run: Run| {
        let v = view.clone();
        PopupMenuItem::new(i18n::t(label)).on_click(move |_, window, cx| {
            let _ = v.update(cx, |v, cx| run(v, window, cx));
        })
    };
    menu.item(item(tk("Rotate Pages Left"), |v, _, cx| {
        v.rotate_pages(false, cx)
    }))
    .item(item(tk("Rotate Pages Right"), |v, _, cx| {
        v.rotate_pages(true, cx)
    }))
    .item(item(tk("Delete Pages"), |v, _, cx| v.delete_pages(cx)))
    .item(item(tk("Extract Pages…"), |v, w, cx| {
        v.extract_pages(w, cx)
    }))
    .separator()
    .item(item(tk("Insert Pages from File…"), |v, w, cx| {
        v.insert_from_file(true, w, cx)
    }))
    .item(item(tk("Insert Blank Page"), |v, _, cx| {
        v.insert_blank_page(true, cx)
    }))
    .separator()
    .item(item(tk("Combine Files…"), |v, w, cx| {
        v.combine_files(w, cx)
    }))
    .item(item(tk("Split Document…"), |v, w, cx| v.split_doc(w, cx)))
    .separator()
    .item(item(tk("Add Page Numbers…"), |v, w, cx| {
        v.add_page_numbers(w, cx)
    }))
    .item(item(tk("Watermark…"), |v, w, cx| v.add_watermark(w, cx)))
    .item(item(tk("Reduce File Size…"), |v, w, cx| {
        v.reduce_file_size(w, cx)
    }))
}

fn zoom_items(menu: PopupMenu, view: &WeakEntity<PdfView>) -> PopupMenu {
    let item = |label: String, zoom: Zoom| {
        let v = view.clone();
        PopupMenuItem::new(label).on_click(move |_, window, cx| {
            let _ = v.update(cx, |v, cx| {
                v.set_zoom(zoom, None, cx);
                v.focus(window, cx);
            });
        })
    };
    let mut menu = menu
        .item(item(i18n::t("Fit Width").into(), Zoom::FitWidth))
        .item(item(i18n::t("Fit Page").into(), Zoom::FitPage))
        .item(item(i18n::t("Actual Size").into(), Zoom::Percent(1.0)))
        .separator();
    for &z in layout::STEPS {
        menu = menu.item(item(format!("{:.0}%", z * 100.0), Zoom::Percent(z)));
    }
    menu
}
