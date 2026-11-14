//! The Slides toolbar: slides (new / duplicate / delete), insert (text box,
//! rectangle, ellipse, picture), B / I / U and lists for the text being
//! edited, and Present.

use docs_engine::Toggle;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::components::icon_button;
use kantor_ui::i18n::{self, tf};
use kantor_ui::{metrics::*, tokens};
use slides_engine::Geom;
use slides_pdf::PrintLayout;

use crate::SlidesView;
use crate::paint_menu::PaintKind;

type Run = fn(&mut SlidesView, &mut Window, &mut Context<SlidesView>);

fn tool(
    id: &'static str,
    icon: Lucide,
    tip: &'static str,
    active: bool,
    run: Run,
    cx: &mut Context<SlidesView>,
) -> Button {
    let view = cx.entity().downgrade();
    icon_button(id, icon, active, cx)
        .tooltip(tip)
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

pub(crate) fn render(view: &SlidesView, cx: &mut Context<SlidesView>) -> Div {
    let t = tokens(cx);
    h_flex()
        .flex_none()
        .h(CONTROL_LG + SPACE_3)
        .px(SPACE_4)
        .gap(SPACE_1)
        .bg(t.chrome)
        .border_b_1()
        .border_color(t.border)
        .child(tool(
            "slides-new",
            Lucide::SquarePlus,
            i18n::t("New slide · Ctrl+M"),
            false,
            |v, w, cx| v.new_slide_doc(w, cx),
            cx,
        ))
        .child(tool(
            "slides-duplicate",
            Lucide::CopyPlus,
            i18n::t("Duplicate slide · Ctrl+D"),
            false,
            |v, w, cx| v.duplicate_slide_doc(w, cx),
            cx,
        ))
        .child(tool(
            "slides-delete",
            Lucide::Delete,
            i18n::t("Delete slide"),
            false,
            |v, w, cx| v.delete_slide_doc(w, cx),
            cx,
        ))
        .child(separator(cx))
        .child(tool(
            "slides-text-box",
            Lucide::Type,
            i18n::t("Insert text box"),
            false,
            |v, w, cx| v.insert_text_box_doc(w, cx),
            cx,
        ))
        .child(tool(
            "slides-rect",
            Lucide::Square,
            i18n::t("Insert rectangle"),
            false,
            |v, w, cx| v.insert_shape_doc(Geom::Rect, w, cx),
            cx,
        ))
        .child(tool(
            "slides-ellipse",
            Lucide::Circle,
            i18n::t("Insert ellipse"),
            false,
            |v, w, cx| v.insert_shape_doc(Geom::Ellipse, w, cx),
            cx,
        ))
        .child(tool(
            "slides-image",
            Lucide::ImagePlus,
            i18n::t("Insert picture (PNG or JPEG)"),
            false,
            |v, w, cx| v.insert_image_doc(w, cx),
            cx,
        ))
        .when(view.object_target().is_some(), |d| {
            d.child(tool(
                "slides-edit-object",
                Lucide::SquarePen,
                i18n::t("Edit table or chart data"),
                false,
                |v, w, cx| v.edit_object_doc(w, cx),
                cx,
            ))
        })
        .child(crate::paint_menu::paint_button(view, PaintKind::Fill, cx))
        .child(crate::paint_menu::paint_button(view, PaintKind::Line, cx))
        .child(separator(cx))
        .child(tool(
            "slides-bold",
            Lucide::Bold,
            i18n::t("Bold · Ctrl+B"),
            view.is_on(Toggle::Bold),
            |v, w, cx| v.toggle_doc(Toggle::Bold, w, cx),
            cx,
        ))
        .child(tool(
            "slides-italic",
            Lucide::Italic,
            i18n::t("Italic · Ctrl+I"),
            view.is_on(Toggle::Italic),
            |v, w, cx| v.toggle_doc(Toggle::Italic, w, cx),
            cx,
        ))
        .child(tool(
            "slides-underline",
            Lucide::Underline,
            i18n::t("Underline · Ctrl+U"),
            view.is_on(Toggle::Underline),
            |v, w, cx| v.toggle_doc(Toggle::Underline, w, cx),
            cx,
        ))
        .child(separator(cx))
        .child(tool(
            "slides-bullets",
            Lucide::List,
            i18n::t("Bulleted list · Tab / Shift+Tab change the level"),
            view.is_list(false),
            |v, w, cx| v.toggle_list_doc(false, w, cx),
            cx,
        ))
        .child(tool(
            "slides-numbering",
            Lucide::ListOrdered,
            i18n::t("Numbered list"),
            view.is_list(true),
            |v, w, cx| v.toggle_list_doc(true, w, cx),
            cx,
        ))
        .child(div().flex_1())
        .child(tool(
            "slides-notes",
            Lucide::NotepadText,
            i18n::t("Speaker notes"),
            view.notes_open(),
            |v, w, cx| v.toggle_notes_doc(w, cx),
            cx,
        ))
        .child(print_button(cx))
        .child(tool(
            "slides-present",
            Lucide::Presentation,
            i18n::t("Present · F5 (Shift+F5 from this slide)"),
            false,
            |v, w, cx| v.present_doc(true, w, cx),
            cx,
        ))
}

/// Export as PDF and the print layouts (slides, notes pages, handouts).
fn print_button(cx: &mut Context<SlidesView>) -> impl IntoElement {
    let view = cx.entity().downgrade();
    Button::new("slides-print")
        .ghost()
        .xsmall()
        .icon(Lucide::Printer)
        .dropdown_caret(true)
        .tooltip(i18n::t("Print and export"))
        .dropdown_menu(move |menu, _, _| print_items(menu, &view))
}

fn print_items(menu: PopupMenu, view: &WeakEntity<SlidesView>) -> PopupMenu {
    let export = view.clone();
    let mut menu = menu
        .item(
            PopupMenuItem::new(i18n::t("Export as PDF…"))
                .icon(Lucide::FileDown)
                .on_click(move |_, window, cx| {
                    let _ = export.update(cx, |v, cx| v.export_pdf_doc(window, cx));
                }),
        )
        .separator();
    let layouts = [PrintLayout::Slides, PrintLayout::Notes]
        .into_iter()
        .chain(PrintLayout::HANDOUTS.map(PrintLayout::Handouts));
    for layout in layouts {
        let v = view.clone();
        let label = match layout {
            PrintLayout::Slides => i18n::t("Print slides…").to_owned(),
            PrintLayout::Notes => i18n::t("Print notes pages…").to_owned(),
            PrintLayout::Handouts(1) => i18n::t("Print handouts: 1 slide per page…").to_owned(),
            PrintLayout::Handouts(n) => tf("Print handouts: {} slides per page…", &[&n]),
        };
        let item = PopupMenuItem::new(label).on_click(move |_, window, cx| {
            let _ = v.update(cx, |v, cx| v.print_layout_doc(layout, window, cx));
        });
        menu = menu.item(if layout == PrintLayout::Slides {
            item.icon(Lucide::Printer)
        } else {
            item
        });
    }
    menu
}
