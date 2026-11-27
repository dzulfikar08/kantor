//! The note toolbar: formatting, lists, links, the view mode and the
//! conversions (Word, PDF, slides).

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::Button;
use gpui_kit::component::h_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::components::{IconButtonExt as _, icon_button};
use kantor_ui::i18n;
use kantor_ui::{metrics::*, tokens};
use notes_engine::Editor;

use crate::{Export, Mode, NotesView};

type Run = fn(&mut NotesView, &mut Window, &mut Context<NotesView>);

fn tool(
    id: &'static str,
    icon: Lucide,
    (tip, keys): (&'static str, Option<&'static str>),
    active: bool,
    run: Run,
    cx: &mut Context<NotesView>,
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

fn edit(v: &mut NotesView, cx: &mut Context<NotesView>, f: impl FnOnce(&mut Editor)) {
    v.type_or_command(f, cx);
}

pub(crate) fn render(view: &NotesView, cx: &mut Context<NotesView>) -> Div {
    let t = tokens(cx);
    let mode = view.mode();
    let editing = mode != Mode::Reading;
    let mut bar = h_flex()
        .flex_none()
        .h(CONTROL_LG + SPACE_3)
        .px(SPACE_4)
        .gap(SPACE_1)
        .bg(t.chrome)
        .border_b_1()
        .border_color(t.border);
    if editing {
        bar = bar
            .child(tool(
                "notes-h1",
                Lucide::Heading1,
                (i18n::t("Heading 1"), Some("ctrl-1")),
                false,
                |v, _, cx| edit(v, cx, |e| e.set_heading(1)),
                cx,
            ))
            .child(tool(
                "notes-h2",
                Lucide::Heading2,
                (i18n::t("Heading 2"), Some("ctrl-2")),
                false,
                |v, _, cx| edit(v, cx, |e| e.set_heading(2)),
                cx,
            ))
            .child(tool(
                "notes-h3",
                Lucide::Heading3,
                (i18n::t("Heading 3"), Some("ctrl-3")),
                false,
                |v, _, cx| edit(v, cx, |e| e.set_heading(3)),
                cx,
            ))
            .child(separator(cx))
            .child(tool(
                "notes-bold",
                Lucide::Bold,
                (i18n::t("Bold"), Some("ctrl-b")),
                false,
                |v, _, cx| edit(v, cx, |e| e.toggle_wrap("**")),
                cx,
            ))
            .child(tool(
                "notes-italic",
                Lucide::Italic,
                (i18n::t("Italic"), Some("ctrl-i")),
                false,
                |v, _, cx| edit(v, cx, |e| e.toggle_wrap("*")),
                cx,
            ))
            .child(tool(
                "notes-strike",
                Lucide::Strikethrough,
                (i18n::t("Strikethrough"), Some("ctrl-shift-x")),
                false,
                |v, _, cx| edit(v, cx, |e| e.toggle_wrap("~~")),
                cx,
            ))
            .child(tool(
                "notes-code",
                Lucide::Code,
                (i18n::t("Code"), Some("ctrl-`")),
                false,
                |v, _, cx| edit(v, cx, |e| e.toggle_wrap("`")),
                cx,
            ))
            .child(tool(
                "notes-link",
                Lucide::Link,
                (i18n::t("Link"), Some("ctrl-k")),
                false,
                |v, _, cx| edit(v, cx, Editor::insert_link),
                cx,
            ))
            .child(separator(cx))
            .child(tool(
                "notes-bullets",
                Lucide::List,
                (i18n::t("Bulleted list"), None),
                false,
                |v, _, cx| edit(v, cx, |e| e.toggle_line_prefix("- ")),
                cx,
            ))
            .child(tool(
                "notes-numbers",
                Lucide::ListOrdered,
                (i18n::t("Numbered list"), None),
                false,
                |v, _, cx| edit(v, cx, |e| e.toggle_line_prefix("1. ")),
                cx,
            ))
            .child(tool(
                "notes-tasks",
                Lucide::ListChecks,
                (i18n::t("Task list"), None),
                false,
                |v, _, cx| edit(v, cx, |e| e.toggle_line_prefix("- [ ] ")),
                cx,
            ))
            .child(tool(
                "notes-quote",
                Lucide::TextQuote,
                (i18n::t("Quote"), None),
                false,
                |v, _, cx| edit(v, cx, |e| e.toggle_line_prefix("> ")),
                cx,
            ));
    }
    bar.child(div().flex_1())
        .child(tool(
            "notes-source",
            Lucide::FileCode,
            (i18n::t("Source mode: every line as typed"), None),
            mode == Mode::Source,
            |v, _, cx| {
                let m = if v.mode() == Mode::Source {
                    Mode::Live
                } else {
                    Mode::Source
                };
                v.set_mode(m, cx)
            },
            cx,
        ))
        .child(tool(
            "notes-reading",
            Lucide::BookOpen,
            (i18n::t("Reading view"), Some("ctrl-e")),
            mode == Mode::Reading,
            |v, _, cx| {
                let m = if v.mode() == Mode::Reading {
                    Mode::Live
                } else {
                    Mode::Reading
                };
                v.set_mode(m, cx)
            },
            cx,
        ))
        .child(separator(cx))
        .child(tool(
            "notes-word",
            Lucide::FileText,
            (i18n::t("Save as Word document"), None),
            false,
            |v, _, cx| v.export(Export::Word, cx),
            cx,
        ))
        .child(tool(
            "notes-pdf",
            Lucide::FileDown,
            (i18n::t("Save as PDF"), None),
            false,
            |v, _, cx| v.export(Export::Pdf, cx),
            cx,
        ))
        .child(tool(
            "notes-slides",
            Lucide::Presentation,
            (i18n::t("Make slides from this note"), None),
            false,
            |v, _, cx| v.export(Export::Slides, cx),
            cx,
        ))
}
