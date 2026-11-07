//! The Slides toolbar: slides (new / duplicate / delete), insert (text box,
//! rectangle, ellipse, picture), B / I / U for the text being edited, and
//! Present.

use docs_engine::Toggle;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{button::Button, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::components::icon_button;
use kantor_ui::{metrics::*, tokens};
use slides_engine::Geom;

use crate::SlidesView;

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
            "New slide · Ctrl+M",
            false,
            |v, w, cx| v.new_slide_doc(w, cx),
            cx,
        ))
        .child(tool(
            "slides-duplicate",
            Lucide::CopyPlus,
            "Duplicate slide · Ctrl+D",
            false,
            |v, w, cx| v.duplicate_slide_doc(w, cx),
            cx,
        ))
        .child(tool(
            "slides-delete",
            Lucide::Delete,
            "Delete slide",
            false,
            |v, w, cx| v.delete_slide_doc(w, cx),
            cx,
        ))
        .child(separator(cx))
        .child(tool(
            "slides-text-box",
            Lucide::Type,
            "Insert text box",
            false,
            |v, w, cx| v.insert_text_box_doc(w, cx),
            cx,
        ))
        .child(tool(
            "slides-rect",
            Lucide::Square,
            "Insert rectangle",
            false,
            |v, w, cx| v.insert_shape_doc(Geom::Rect, w, cx),
            cx,
        ))
        .child(tool(
            "slides-ellipse",
            Lucide::Circle,
            "Insert ellipse",
            false,
            |v, w, cx| v.insert_shape_doc(Geom::Ellipse, w, cx),
            cx,
        ))
        .child(tool(
            "slides-image",
            Lucide::ImagePlus,
            "Insert picture (PNG or JPEG)",
            false,
            |v, w, cx| v.insert_image_doc(w, cx),
            cx,
        ))
        .child(separator(cx))
        .child(tool(
            "slides-bold",
            Lucide::Bold,
            "Bold · Ctrl+B",
            view.is_on(Toggle::Bold),
            |v, w, cx| v.toggle_doc(Toggle::Bold, w, cx),
            cx,
        ))
        .child(tool(
            "slides-italic",
            Lucide::Italic,
            "Italic · Ctrl+I",
            view.is_on(Toggle::Italic),
            |v, w, cx| v.toggle_doc(Toggle::Italic, w, cx),
            cx,
        ))
        .child(tool(
            "slides-underline",
            Lucide::Underline,
            "Underline · Ctrl+U",
            view.is_on(Toggle::Underline),
            |v, w, cx| v.toggle_doc(Toggle::Underline, w, cx),
            cx,
        ))
        .child(div().flex_1())
        .child(tool(
            "slides-present",
            Lucide::Presentation,
            "Present · F5 (Shift+F5 from this slide)",
            false,
            |v, w, cx| v.present_doc(true, w, cx),
            cx,
        ))
}
