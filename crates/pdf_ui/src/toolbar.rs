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

use crate::markup::{COLORS, Tool};
use crate::{Mode, Panel, PdfView, Zoom, layout};

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
        .child(separator(cx))
        .child(
            Button::new("pdf-comment-mode")
                .xsmall()
                .icon(Lucide::MessageSquare)
                .label(i18n::t("Comment"))
                .when(view.mode() == Mode::Comment, |b| b.primary())
                .when(view.mode() != Mode::Comment, |b| b.ghost())
                .tooltip(i18n::t("Highlight, notes, text boxes, drawing and stamps"))
                .on_click({
                    let v = cx.entity().downgrade();
                    move |_, window, cx| {
                        let _ = v.update(cx, |v, cx| {
                            v.set_mode(Mode::Comment, cx);
                            v.focus(window, cx);
                        });
                    }
                }),
        )
        .child(
            Button::new("pdf-fill-sign-mode")
                .xsmall()
                .icon(Lucide::Signature)
                .label(i18n::t("Fill & Sign"))
                .when(view.mode() == Mode::FillSign, |b| b.primary())
                .when(view.mode() != Mode::FillSign, |b| b.ghost())
                .tooltip(i18n::t(
                    "Fill in forms, add text, dates, check marks, signatures and stamps",
                ))
                .on_click({
                    let v = cx.entity().downgrade();
                    move |_, window, cx| {
                        let _ = v.update(cx, |v, cx| {
                            v.set_mode(Mode::FillSign, cx);
                            v.focus(window, cx);
                        });
                    }
                }),
        )
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
    .separator()
    .item(item(tk("Batch…"), |v, w, cx| v.open_batch(w, cx)))
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

fn tool_btn(
    id: &'static str,
    icon: Lucide,
    tip: &'static str,
    which: Tool,
    view: &PdfView,
    cx: &mut Context<PdfView>,
) -> Button {
    let active = view.markup.tool == which;
    let v = cx.entity().downgrade();
    icon_button(id, icon, active, cx)
        .tip(tip, None)
        .on_click(move |_, window, cx| {
            let which = which.clone();
            let _ = v.update(cx, |v, cx| {
                v.set_tool(which, cx);
                v.focus(window, cx);
            });
        })
}

/// The second row in Comment and Fill & Sign modes: the tools, then the
/// colour, line width, font and size, opacity, and what the selected
/// annotation offers.
pub(crate) fn render_markup(view: &PdfView, cx: &mut Context<PdfView>) -> Div {
    let t = tokens(cx);
    let weak = cx.entity().downgrade();
    let mut row = h_flex()
        .flex_none()
        .h(CONTROL_LG + SPACE_2)
        .px(SPACE_4)
        .gap(SPACE_1)
        .bg(t.chrome)
        .border_b_1()
        .border_color(t.border)
        .child(tool_btn(
            "pdf-t-select",
            Lucide::MousePointer2,
            tk("Select"),
            Tool::Select,
            view,
            cx,
        ));
    match view.mode() {
        Mode::FillSign => {
            row = row
                .child(tool_btn(
                    "pdf-t-fstext",
                    Lucide::Type,
                    tk("Add Text"),
                    Tool::FsText,
                    view,
                    cx,
                ))
                .child(tool_btn(
                    "pdf-t-fsdate",
                    Lucide::Calendar,
                    tk("Add Date"),
                    Tool::FsDate,
                    view,
                    cx,
                ))
                .child(tool_btn(
                    "pdf-t-fscheck",
                    Lucide::Check,
                    tk("Check Mark"),
                    Tool::FsCheck,
                    view,
                    cx,
                ))
                .child(tool_btn(
                    "pdf-t-fscross",
                    Lucide::X,
                    tk("Cross Mark"),
                    Tool::FsCross,
                    view,
                    cx,
                ))
                .child(separator(cx))
                .child({
                    let w = weak.clone();
                    Button::new("pdf-sign")
                        .ghost()
                        .xsmall()
                        .icon(Lucide::Signature)
                        .label(i18n::t("Sign"))
                        .dropdown_caret(true)
                        .tooltip(i18n::t("Add a signature or a saved stamp"))
                        .dropdown_menu(move |menu, _, _| crate::sign::sign_menu(menu, &w))
                });
        }
        _ => {
            row = row
                .child(tool_btn(
                    "pdf-t-hl",
                    Lucide::Highlighter,
                    tk("Highlight Text"),
                    Tool::Highlight,
                    view,
                    cx,
                ))
                .child(tool_btn(
                    "pdf-t-ul",
                    Lucide::Underline,
                    tk("Underline Text"),
                    Tool::Underline,
                    view,
                    cx,
                ))
                .child(tool_btn(
                    "pdf-t-so",
                    Lucide::Strikethrough,
                    tk("Strikethrough Text"),
                    Tool::StrikeOut,
                    view,
                    cx,
                ))
                .child(tool_btn(
                    "pdf-t-note",
                    Lucide::StickyNote,
                    tk("Add Sticky Note"),
                    Tool::Note,
                    view,
                    cx,
                ))
                .child(tool_btn(
                    "pdf-t-text",
                    Lucide::Type,
                    tk("Add Text Box"),
                    Tool::TextBox,
                    view,
                    cx,
                ))
                .child(separator(cx))
                .child(tool_btn(
                    "pdf-t-pen",
                    Lucide::PenLine,
                    tk("Draw"),
                    Tool::Pen,
                    view,
                    cx,
                ))
                .child(tool_btn(
                    "pdf-t-rect",
                    Lucide::Square,
                    tk("Rectangle"),
                    Tool::Rect,
                    view,
                    cx,
                ))
                .child(tool_btn(
                    "pdf-t-ellipse",
                    Lucide::Circle,
                    tk("Ellipse"),
                    Tool::Ellipse,
                    view,
                    cx,
                ))
                .child(tool_btn(
                    "pdf-t-line",
                    Lucide::Minus,
                    tk("Line"),
                    Tool::Line,
                    view,
                    cx,
                ))
                .child(tool_btn(
                    "pdf-t-arrow",
                    Lucide::MoveUpRight,
                    tk("Arrow"),
                    Tool::Arrow,
                    view,
                    cx,
                ))
                .child(tool_btn(
                    "pdf-t-poly",
                    Lucide::Pentagon,
                    tk("Polygon"),
                    Tool::Polygon,
                    view,
                    cx,
                ));
        }
    }
    row = row.child(
        Button::new("pdf-add-stamp")
            .ghost()
            .xsmall()
            .icon(Lucide::Stamp)
            .label(i18n::t("Add Image / Stamp…"))
            .on_click({
                let w = weak.clone();
                move |_, window, cx| {
                    let _ = w.update(cx, |v, cx| v.add_image_stamp(None, window, cx));
                }
            }),
    );
    row = row.child(separator(cx));
    // Colours.
    for (i, (c, name)) in COLORS.iter().enumerate() {
        let c = *c;
        let w = weak.clone();
        let current = view.markup.color == c;
        row = row.child(
            div()
                .id(("pdf-color", i))
                .size(px(16.))
                .rounded_full()
                .bg(crate::markup::hsla(c, 1.0))
                .border_2()
                .border_color(if current { t.accent } else { t.border })
                .cursor_pointer()
                .tooltip({
                    let name = i18n::t(name);
                    move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(name).build(window, cx)
                    }
                })
                .on_click(move |_, _, cx| {
                    let _ = w.update(cx, |v, cx| v.set_color(c, cx));
                }),
        );
    }
    let sel = view.markup.sel.clone().and_then(|s| view.find_mark(&s));
    let is_text = matches!(
        view.markup.tool,
        Tool::TextBox | Tool::FsText | Tool::FsDate
    ) || sel
        .as_ref()
        .is_some_and(|a| matches!(a.kind, kantor_pdf::annot::Kind::FreeText { .. }));
    let is_stamp = sel
        .as_ref()
        .is_some_and(|a| matches!(a.kind, kantor_pdf::annot::Kind::Stamp { .. }));
    row = row.child(separator(cx));
    if is_text {
        let w = weak.clone();
        let w2 = weak.clone();
        row = row
            .child(
                Button::new("pdf-font")
                    .ghost()
                    .xsmall()
                    .label(match view.markup.font {
                        kantor_pdf::annot::Font::Helvetica => "Helvetica",
                        kantor_pdf::annot::Font::Courier => "Courier",
                    })
                    .dropdown_caret(true)
                    .tooltip(i18n::t("Font"))
                    .dropdown_menu(move |menu, _, _| {
                        let (a, b) = (w.clone(), w.clone());
                        menu.item(PopupMenuItem::new("Helvetica").on_click(move |_, _, cx| {
                            let _ = a.update(cx, |v, cx| {
                                v.set_font(kantor_pdf::annot::Font::Helvetica, cx)
                            });
                        }))
                        .item(PopupMenuItem::new("Courier").on_click(move |_, _, cx| {
                            let _ = b.update(cx, |v, cx| {
                                v.set_font(kantor_pdf::annot::Font::Courier, cx)
                            });
                        }))
                    }),
            )
            .child(
                Button::new("pdf-font-size")
                    .ghost()
                    .xsmall()
                    .label(format!("{} pt", view.markup.font_size))
                    .dropdown_caret(true)
                    .tooltip(i18n::t("Font size"))
                    .dropdown_menu(move |mut menu, _, _| {
                        for s in [
                            8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 24.0, 36.0, 48.0, 72.0,
                        ] {
                            let w = w2.clone();
                            menu = menu.item(PopupMenuItem::new(format!("{s} pt")).on_click(
                                move |_, _, cx| {
                                    let _ = w.update(cx, |v, cx| v.set_font_size(s, cx));
                                },
                            ));
                        }
                        menu
                    }),
            );
    } else {
        let w = weak.clone();
        row = row.child(
            Button::new("pdf-width")
                .ghost()
                .xsmall()
                .label(format!("{} pt", view.markup.width))
                .dropdown_caret(true)
                .tooltip(i18n::t("Line width"))
                .dropdown_menu(move |mut menu, _, _| {
                    for s in [0.5, 1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0] {
                        let w = w.clone();
                        menu = menu.item(PopupMenuItem::new(format!("{s} pt")).on_click(
                            move |_, _, cx| {
                                let _ = w.update(cx, |v, cx| v.set_width(s, cx));
                            },
                        ));
                    }
                    menu
                }),
        );
    }
    let w = weak.clone();
    row = row.child(
        Button::new("pdf-opacity")
            .ghost()
            .xsmall()
            .label(format!("{:.0}%", view.markup.opacity * 100.0))
            .dropdown_caret(true)
            .tooltip(i18n::t("Opacity"))
            .dropdown_menu(move |mut menu, _, _| {
                for s in [1.0, 0.85, 0.7, 0.5, 0.3] {
                    let w = w.clone();
                    menu = menu.item(PopupMenuItem::new(format!("{:.0}%", s * 100.0)).on_click(
                        move |_, _, cx| {
                            let _ = w.update(cx, |v, cx| v.set_opacity(s, cx));
                        },
                    ));
                }
                menu
            }),
    );
    if is_stamp {
        row = row
            .child(separator(cx))
            .child(tool(
                "pdf-stamp-ccw",
                Lucide::RotateCcw,
                (i18n::t("Rotate Left"), None),
                false,
                |v, _, cx| v.turn_stamp(90.0, cx),
                cx,
            ))
            .child(tool(
                "pdf-stamp-cw",
                Lucide::RotateCw,
                (i18n::t("Rotate Right"), None),
                false,
                |v, _, cx| v.turn_stamp(-90.0, cx),
                cx,
            ));
    }
    if sel.is_some() {
        let (w1, w2) = (weak.clone(), weak.clone());
        row = row
            .child(
                Button::new("pdf-place-pages")
                    .ghost()
                    .xsmall()
                    .label(i18n::t("Place on Pages…"))
                    .on_click(move |_, window, cx| {
                        let _ = w1.update(cx, |v, cx| v.place_on_pages(window, cx));
                    }),
            )
            .child(
                Button::new("pdf-del-mark")
                    .ghost()
                    .xsmall()
                    .icon(Lucide::Trash)
                    .tooltip(i18n::t("Delete Comment"))
                    .on_click(move |_, _, cx| {
                        let _ = w2.update(cx, |v, cx| {
                            v.delete_mark(cx);
                        });
                    }),
            );
    }
    row = row.child(div().flex_1());
    if view.mode() == Mode::FillSign {
        row = row.child(
            div()
                .text_size(FONT_SMALL)
                .text_color(t.fg_dim)
                .child(i18n::t(
                    "Signatures added here are pictures, not certificate-based digital signatures",
                )),
        );
    }
    row.child(tool(
        "pdf-comments-panel",
        Lucide::MessageSquareText,
        (i18n::t("Comments"), None),
        view.panel() == Some(Panel::Comments),
        |v, _, cx| v.toggle_panel(Panel::Comments, cx),
        cx,
    ))
}
