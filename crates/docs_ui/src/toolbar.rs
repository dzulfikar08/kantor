//! The Docs toolbar: B / I / U, paragraph styles, alignment, lists,
//! insert table / picture, table tools (when the caret is in a table) and
//! zoom, each showing the state at the caret.

use docs_engine::{Align, ListKind, TableEdit, Toggle};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, tooltip::Tooltip};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::components::icon_button;
use kantor_ui::{metrics::*, tokens};

use crate::DocsView;

type Run = fn(&mut DocsView, &mut Window, &mut Context<DocsView>);

fn tool(
    id: &'static str,
    icon: Lucide,
    tip: &'static str,
    active: bool,
    run: Run,
    cx: &mut Context<DocsView>,
) -> Stateful<Div> {
    let view = cx.entity().downgrade();
    icon_button(id, icon, active, cx)
        .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
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

pub(crate) fn render(view: &DocsView, cx: &mut Context<DocsView>) -> Div {
    let t = tokens(cx);
    let style = view.current_style();
    let align = view.current_align();
    let is_style = |s: Option<&str>| style.as_deref() == s;
    let zoom_label = format!("{:.0}%", view.zoom() * 100.0);
    let list = view.current_list();
    let in_table = view.in_table();
    let table_button = {
        let v = cx.entity().downgrade();
        icon_button("docs-table", Lucide::Table, false, cx)
            .tooltip(|window, cx| Tooltip::new("Insert table").build(window, cx))
            .on_click(move |ev, _, cx| {
                let at = ev.position();
                let _ = v.update(cx, |v, cx| v.toggle_table_picker(at, cx));
            })
    };
    let reset = {
        let view = cx.entity().downgrade();
        div()
            .id("docs-zoom-reset")
            .flex_none()
            .h(CONTROL_SM)
            .px(SPACE_3)
            .flex()
            .items_center()
            .rounded(RADIUS_MD)
            .text_size(FONT_SMALL)
            .text_color(t.fg_muted)
            .cursor_pointer()
            .hover(|s| s.bg(t.hover).text_color(t.fg))
            .tooltip(|window, cx| Tooltip::new("Actual size  Ctrl+0").build(window, cx))
            .child(zoom_label)
            .on_click(move |_, window, cx| {
                let _ = view.update(cx, |v, cx| {
                    v.set_zoom(1.0, cx);
                    v.focus(window, cx);
                });
            })
    };
    h_flex()
        .flex_none()
        .h(CONTROL_LG + SPACE_3)
        .px(SPACE_4)
        .gap(SPACE_1)
        .bg(t.chrome)
        .border_b_1()
        .border_color(t.border)
        .child(tool(
            "docs-bold",
            Lucide::Bold,
            "Bold  Ctrl+B",
            view.is_on(Toggle::Bold),
            |v, w, cx| v.toggle_doc(Toggle::Bold, w, cx),
            cx,
        ))
        .child(tool(
            "docs-italic",
            Lucide::Italic,
            "Italic  Ctrl+I",
            view.is_on(Toggle::Italic),
            |v, w, cx| v.toggle_doc(Toggle::Italic, w, cx),
            cx,
        ))
        .child(tool(
            "docs-underline",
            Lucide::Underline,
            "Underline  Ctrl+U",
            view.is_on(Toggle::Underline),
            |v, w, cx| v.toggle_doc(Toggle::Underline, w, cx),
            cx,
        ))
        .child(separator(cx))
        .child(tool(
            "docs-normal",
            Lucide::Pilcrow,
            "Normal text  Ctrl+Alt+0",
            is_style(None),
            |v, w, cx| v.set_style_doc(None, w, cx),
            cx,
        ))
        .child(tool(
            "docs-h1",
            Lucide::Heading1,
            "Heading 1  Ctrl+Alt+1",
            is_style(Some("Heading1")),
            |v, w, cx| v.set_style_doc(Some("Heading1"), w, cx),
            cx,
        ))
        .child(tool(
            "docs-h2",
            Lucide::Heading2,
            "Heading 2  Ctrl+Alt+2",
            is_style(Some("Heading2")),
            |v, w, cx| v.set_style_doc(Some("Heading2"), w, cx),
            cx,
        ))
        .child(tool(
            "docs-h3",
            Lucide::Heading3,
            "Heading 3  Ctrl+Alt+3",
            is_style(Some("Heading3")),
            |v, w, cx| v.set_style_doc(Some("Heading3"), w, cx),
            cx,
        ))
        .child(separator(cx))
        .child(tool(
            "docs-left",
            Lucide::TextAlignStart,
            "Align left  Ctrl+L",
            align == Align::Left,
            |v, w, cx| v.set_align_doc(Align::Left, w, cx),
            cx,
        ))
        .child(tool(
            "docs-center",
            Lucide::TextAlignCenter,
            "Center  Ctrl+E",
            align == Align::Center,
            |v, w, cx| v.set_align_doc(Align::Center, w, cx),
            cx,
        ))
        .child(tool(
            "docs-right",
            Lucide::TextAlignEnd,
            "Align right  Ctrl+R",
            align == Align::Right,
            |v, w, cx| v.set_align_doc(Align::Right, w, cx),
            cx,
        ))
        .child(tool(
            "docs-justify",
            Lucide::TextAlignJustify,
            "Justify  Ctrl+J",
            align == Align::Justify,
            |v, w, cx| v.set_align_doc(Align::Justify, w, cx),
            cx,
        ))
        .child(separator(cx))
        .child(tool(
            "docs-bullets",
            Lucide::List,
            "Bulleted list  Ctrl+Shift+8",
            list == Some(ListKind::Bullet),
            |v, w, cx| v.toggle_list_doc(ListKind::Bullet, w, cx),
            cx,
        ))
        .child(tool(
            "docs-numbered",
            Lucide::ListOrdered,
            "Numbered list  Ctrl+Shift+7",
            list == Some(ListKind::Numbered),
            |v, w, cx| v.toggle_list_doc(ListKind::Numbered, w, cx),
            cx,
        ))
        .child(tool(
            "docs-outdent",
            Lucide::ListIndentDecrease,
            "Decrease list level  Shift+Tab",
            false,
            |v, w, cx| v.list_level_doc(-1, w, cx),
            cx,
        ))
        .child(tool(
            "docs-indent",
            Lucide::ListIndentIncrease,
            "Increase list level  Tab",
            false,
            |v, w, cx| v.list_level_doc(1, w, cx),
            cx,
        ))
        .child(separator(cx))
        .child(table_button)
        .child(tool(
            "docs-image",
            Lucide::ImagePlus,
            "Insert picture (PNG or JPEG)",
            false,
            |v, w, cx| v.insert_image_doc(w, cx),
            cx,
        ))
        .when(in_table, |bar| {
            bar.child(separator(cx))
                .child(tool(
                    "docs-row-below",
                    Lucide::BetweenHorizontalEnd,
                    "Insert row below",
                    false,
                    |v, w, cx| v.table_edit_doc(TableEdit::RowBelow, w, cx),
                    cx,
                ))
                .child(tool(
                    "docs-col-right",
                    Lucide::BetweenVerticalEnd,
                    "Insert column right",
                    false,
                    |v, w, cx| v.table_edit_doc(TableEdit::ColumnRight, w, cx),
                    cx,
                ))
                .child(tool(
                    "docs-del-row",
                    Lucide::Rows3,
                    "Delete row",
                    false,
                    |v, w, cx| v.table_edit_doc(TableEdit::DeleteRow, w, cx),
                    cx,
                ))
                .child(tool(
                    "docs-del-col",
                    Lucide::Columns3,
                    "Delete column",
                    false,
                    |v, w, cx| v.table_edit_doc(TableEdit::DeleteColumn, w, cx),
                    cx,
                ))
                .child(tool(
                    "docs-del-table",
                    Lucide::Trash,
                    "Delete table",
                    false,
                    |v, w, cx| v.table_edit_doc(TableEdit::DeleteTable, w, cx),
                    cx,
                ))
        })
        .child(div().flex_1())
        .child(tool(
            "docs-zoom-out",
            Lucide::ZoomOut,
            "Zoom out  Ctrl+-",
            false,
            |v, _, cx| v.zoom_by(1.0 / ZOOM_STEP, cx),
            cx,
        ))
        .child(reset)
        .child(tool(
            "docs-zoom-in",
            Lucide::ZoomIn,
            "Zoom in  Ctrl+=",
            false,
            |v, _, cx| v.zoom_by(ZOOM_STEP, cx),
            cx,
        ))
        .child(separator(cx))
        .child(crate::page_menu::page_button(view, cx))
}
