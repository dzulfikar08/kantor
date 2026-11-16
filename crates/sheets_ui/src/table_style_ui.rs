//! New Table Style / Modify Table Style, as Excel has them: a name, the
//! list of table elements with a preview, Format… for the selected element
//! (font, border and fill, from the same swatches and toggles as the
//! toolbar and Conditional Formatting), Clear, the stripe size of stripes,
//! and "Set as default table style for this document". The style gallery's
//! right-click menu has Modify, Duplicate, Delete and Set As Default.
//!
//! Every change to the workbook is one undo step
//! (`sheets_engine::table_style_edit`).

use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{self, Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{t, t_dyn, tf, tk};
use kantor_ui::{metrics::*, tokens};
use sheets_engine::format::{BorderLine, LineStyle, Rgb};
use sheets_engine::{TableCellLook, TableElement, TableElementFormat, TableStyleSpec};

use crate::SheetsView;
use crate::dialog::{self, note, row};
use crate::look::hsla;
use crate::toolbar::SWATCHES;

actions!(sheets, [NewTableStyle]);

/// Texts of `sheets_engine` shown here, translated where shown.
#[cfg(test)]
const ENGINE_TEXTS: &[&str] = &[
    tk("Whole Table"),
    tk("First Column Stripe"),
    tk("Second Column Stripe"),
    tk("First Row Stripe"),
    tk("Second Row Stripe"),
    tk("Last Column"),
    tk("First Column"),
    tk("Header Row"),
    tk("Total Row"),
    tk("First Header Cell"),
    tk("Last Header Cell"),
    tk("First Total Cell"),
    tk("Last Total Cell"),
    tk("Type a name for the table style"),
    tk("A table style name has at most 255 characters"),
    tk("A table style with this name already exists"),
];

/// The New / Modify Table Style card.
pub(crate) struct TableStyleDialog {
    /// Modify: the style's name; `None` for New and Duplicate.
    editing: Option<String>,
    name: Entity<InputState>,
    spec: TableStyleSpec,
    element: TableElement,
    default: bool,
    /// Format… open for the selected element.
    format: Option<FormatDraft>,
    error: Option<String>,
    _sub: Subscription,
}

/// Format Cells for one table element.
#[derive(Clone)]
struct FormatDraft {
    f: TableElementFormat,
    /// The line the border buttons draw.
    pen: LineStyle,
    pen_color: Rgb,
}

/// Border buttons of Format…: an edge, or a preset.
#[derive(Clone, Copy, PartialEq)]
enum Edge {
    Outline,
    Inside,
    None,
    Top,
    Bottom,
    Left,
    Right,
    InsideHorizontal,
    InsideVertical,
}

impl Edge {
    const PRESETS: [(Edge, &'static str); 3] = [
        (Edge::None, tk("None")),
        (Edge::Outline, tk("Outline")),
        (Edge::Inside, tk("Inside")),
    ];
    const EDGES: [(Edge, &'static str); 6] = [
        (Edge::Top, tk("Top")),
        (Edge::Bottom, tk("Bottom")),
        (Edge::Left, tk("Left")),
        (Edge::Right, tk("Right")),
        (Edge::InsideHorizontal, tk("Inside Horizontal")),
        (Edge::InsideVertical, tk("Inside Vertical")),
    ];

    fn slot(self, f: &mut TableElementFormat) -> Option<&mut Option<BorderLine>> {
        Some(match self {
            Edge::Top => &mut f.top,
            Edge::Bottom => &mut f.bottom,
            Edge::Left => &mut f.left,
            Edge::Right => &mut f.right,
            Edge::InsideHorizontal => &mut f.horizontal,
            Edge::InsideVertical => &mut f.vertical,
            _ => return None,
        })
    }

    fn apply(self, d: &mut FormatDraft) {
        let pen = Some(BorderLine {
            style: d.pen,
            color: Some(d.pen_color),
        });
        let f = &mut d.f;
        match self {
            Edge::None => {
                for e in [&mut f.top, &mut f.bottom, &mut f.left, &mut f.right] {
                    *e = None;
                }
                f.horizontal = None;
                f.vertical = None;
            }
            Edge::Outline => {
                for e in [&mut f.top, &mut f.bottom, &mut f.left, &mut f.right] {
                    *e = pen;
                }
            }
            Edge::Inside => {
                f.horizontal = pen;
                f.vertical = pen;
            }
            e => {
                let slot = e.slot(f).expect("an edge");
                *slot = if *slot == pen { None } else { pen };
            }
        }
    }
}

/// Line styles of Format… ▸ Border, with their names.
const LINES: [(LineStyle, &str); 8] = [
    (LineStyle::Thin, tk("Thin")),
    (LineStyle::Hair, tk("Hair")),
    (LineStyle::Dotted, tk("Dotted")),
    (LineStyle::Dashed, tk("Dashed")),
    (LineStyle::DashDot, tk("Dash Dot")),
    (LineStyle::Medium, tk("Medium")),
    (LineStyle::Thick, tk("Thick")),
    (LineStyle::Double, tk("Double")),
];

impl SheetsView {
    /// Table Design ▸ New Table Style…
    pub fn new_table_style_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.table.read(cx).delegate().book.next_table_style_name();
        self.open_table_style(None, TableStyleSpec::new(&name), false, window, cx);
    }

    /// Modify… for custom style `name`.
    pub(crate) fn modify_table_style_ui(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (spec, default) = {
            let book = &self.table.read(cx).delegate().book;
            (
                book.table_style_spec(name),
                book.default_table_style() == name,
            )
        };
        if let Some(spec) = spec {
            self.open_table_style(Some(name.to_owned()), spec, default, window, cx);
        }
    }

    /// Duplicate… for style `name` (custom or built-in): New Table Style
    /// with a copy of it.
    pub(crate) fn duplicate_table_style_ui(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let spec = {
            let book = &self.table.read(cx).delegate().book;
            book.table_style_spec(name).map(|mut s| {
                s.name = book.duplicate_table_style_name(name);
                s
            })
        };
        if let Some(spec) = spec {
            self.open_table_style(None, spec, false, window, cx);
        }
    }

    fn open_table_style(
        &mut self,
        editing: Option<String>,
        spec: TableStyleSpec,
        default: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let name = cx.new(|cx| InputState::new(window, cx));
        name.update(cx, |i, cx| {
            i.set_value(spec.name.clone(), window, cx);
            i.focus(window, cx);
        });
        let sub = cx.subscribe_in(&name, window, |this, _, e: &InputEvent, w, cx| match e {
            InputEvent::Change => {
                if let Some(d) = this.table_style_dialog.as_mut() {
                    d.error = None;
                }
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.table_style_ok(w, cx),
            _ => {}
        });
        self.table_style_dialog = Some(TableStyleDialog {
            editing,
            name,
            spec,
            element: TableElement::WholeTable,
            default,
            format: None,
            error: None,
            _sub: sub,
        });
        cx.notify();
    }

    fn close_table_style(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.table_style_dialog.take().is_some() {
            self.focus_grid(window, cx);
            cx.notify();
        }
    }

    fn table_style_ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.table_style_dialog.as_ref() else {
            return;
        };
        if d.format.is_some() {
            self.format_ok(window, cx);
            return;
        }
        let mut spec = d.spec.clone();
        spec.name = d.name.read(cx).value().trim().to_string();
        let (editing, default) = (d.editing.clone(), d.default);
        let res = self.with_book(cx, |book| match &editing {
            Some(old) => book.modify_table_style(old, &spec, default),
            None => book.add_table_style(&spec, default),
        });
        match res {
            Ok(()) => {
                self.table_style_dialog = None;
                self.status = if editing.is_some() {
                    tf("Table style {} changed", &[&spec.name])
                } else {
                    tf("Table style {} added", &[&spec.name])
                }
                .into();
                self.focus_grid(window, cx);
                self.load_selected_into_formula(window, cx);
                self.update_title(window, cx);
                cx.notify();
            }
            Err(e) => {
                if let Some(d) = self.table_style_dialog.as_mut() {
                    d.error = Some(t_dyn(&format!("{e:#}")).to_owned());
                }
                cx.notify();
            }
        }
    }

    /// Run `f` on the workbook and repaint the grid.
    fn with_book<R>(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut sheets_engine::Workbook) -> R,
    ) -> R {
        self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let r = f(&mut grid.book);
            grid.resize();
            table.refresh(cx);
            r
        })
    }

    /// Delete for custom style `name`.
    pub(crate) fn delete_table_style_ui(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let res = self.with_book(cx, |b| b.delete_table_style(name));
        self.status = match res {
            Ok(()) => tf("Table style {} deleted", &[&name]),
            Err(e) => t_dyn(&format!("{e:#}")).to_owned(),
        }
        .into();
        self.load_selected_into_formula(window, cx);
        self.update_title(window, cx);
        cx.notify();
    }

    /// Set As Default for style `name`.
    pub(crate) fn default_table_style_ui(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let res = self.with_book(cx, |b| b.set_default_table_style(name));
        self.status = match res {
            Ok(()) => tf("New tables get {}", &[&crate::table_ui::style_label(name)]),
            Err(e) => t_dyn(&format!("{e:#}")).to_owned(),
        }
        .into();
        self.update_title(window, cx);
        cx.notify();
    }

    fn open_format(&mut self, cx: &mut Context<Self>) {
        if let Some(d) = self.table_style_dialog.as_mut() {
            d.format = Some(FormatDraft {
                f: d.spec.format(d.element),
                pen: LineStyle::Thin,
                pen_color: 0x000000,
            });
        }
        cx.notify();
    }

    fn format_ok(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.table_style_dialog.as_mut()
            && let Some(f) = d.format.take()
        {
            d.spec.set_format(d.element, f.f);
        }
        cx.notify();
    }

    fn format_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.table_style_dialog.as_mut() {
            Some(d) if d.format.is_some() => {
                d.format = None;
                cx.notify();
            }
            _ => self.close_table_style(window, cx),
        }
    }

    pub(crate) fn render_table_style_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.table_style_dialog.as_ref()?;
        let view = cx.entity().downgrade();
        let card = match &d.format {
            Some(f) => self.render_element_format(d, f, &view, cx),
            None => self.render_style_main(d, &view, cx),
        };
        let card = card
            .key_context("KantorTableStyleDialog")
            .on_action(cx.listener(|this, _: &input::Escape, w, cx| this.format_cancel(w, cx)));
        Some(dialog::place(card).into_any_element())
    }

    fn render_style_main(
        &self,
        d: &TableStyleDialog,
        view: &WeakEntity<Self>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let tk_ = tokens(cx);
        let title = if d.editing.is_some() {
            t("Modify Table Style")
        } else {
            t("New Table Style")
        };
        // Element list.
        let mut list = v_flex()
            .id("ts-elements")
            .w(px(200.))
            .p(SPACE_1)
            .rounded(RADIUS_SM)
            .border_1()
            .border_color(tk_.border)
            .bg(tk_.bg);
        for (i, e) in TableElement::ALL.into_iter().enumerate() {
            let v = view.clone();
            let selected = e == d.element;
            let formatted = !d.spec.format(e).is_empty();
            list = list.child(
                div()
                    .id(("ts-element", i))
                    .px(SPACE_2)
                    .py(px(2.))
                    .rounded(RADIUS_SM)
                    .when(selected, |s| s.bg(tk_.accent).text_color(tk_.accent_fg))
                    .when(!selected, |s| s.hover(|s| s.bg(tk_.hover)))
                    .when(formatted, |s| s.font_weight(FontWeight::SEMIBOLD))
                    .cursor_pointer()
                    .child(t_dyn(e.label()).to_owned())
                    .on_click(move |ev, w, cx| {
                        let double = ev.click_count() > 1;
                        let _ = v.update(cx, |this, cx| {
                            if let Some(d) = this.table_style_dialog.as_mut() {
                                d.element = e;
                            }
                            if double {
                                this.open_format(cx);
                            }
                            let _ = w;
                            cx.notify();
                        });
                    }),
            );
        }
        // Preview.
        let cells = self
            .table
            .read(cx)
            .delegate()
            .book
            .table_style_spec_preview(&d.spec);
        let preview = v_flex()
            .gap(SPACE_2)
            .child(kantor_ui::components::section_label(t("Preview"), cx))
            .child(
                div()
                    .p(SPACE_3)
                    .rounded(RADIUS_SM)
                    .border_1()
                    .border_color(tk_.border)
                    .bg(gpui::white())
                    .child(preview_picture(&cells, px(30.), px(16.), true)),
            );
        // Format, Clear, stripe size.
        let fv = view.clone();
        let cv = view.clone();
        let element = d.element;
        let mut tools = h_flex()
            .gap(SPACE_3)
            .child(
                Button::new("ts-format")
                    .small()
                    .outline()
                    .label(t("Format…"))
                    .on_click(move |_, _, cx| {
                        let _ = fv.update(cx, |this, cx| this.open_format(cx));
                    }),
            )
            .child(
                Button::new("ts-clear")
                    .small()
                    .ghost()
                    .label(t("Clear"))
                    .on_click(move |_, _, cx| {
                        let _ = cv.update(cx, |this, cx| {
                            if let Some(d) = this.table_style_dialog.as_mut() {
                                d.spec.set_format(element, TableElementFormat::default());
                            }
                            cx.notify();
                        });
                    }),
            );
        if element.has_size() {
            let current = d.spec.size(element);
            let sv = view.clone();
            tools = tools.child(
                h_flex()
                    .gap(SPACE_2)
                    .child(note(t("Stripe Size"), false, cx))
                    .child(
                        Button::new("ts-size")
                            .small()
                            .outline()
                            .label(current.to_string())
                            .dropdown_caret(true)
                            .dropdown_menu(move |mut m, _, _| {
                                for n in 1..=9u32 {
                                    let v = sv.clone();
                                    m = m.item(
                                        PopupMenuItem::new(n.to_string())
                                            .checked(n == current)
                                            .on_click(move |_, _, cx| {
                                                let _ = v.update(cx, |this, cx| {
                                                    if let Some(d) =
                                                        this.table_style_dialog.as_mut()
                                                    {
                                                        d.spec.set_size(element, n);
                                                    }
                                                    cx.notify();
                                                });
                                            }),
                                    );
                                }
                                m
                            }),
                    ),
            );
        }
        let dv = view.clone();
        dialog::card("table-style", title, px(560.), cx)
            .child(row(t("Name"), Input::new(&d.name).small(), cx))
            .child(
                h_flex()
                    .items_start()
                    .gap(SPACE_5)
                    .child(
                        v_flex()
                            .gap(SPACE_2)
                            .child(kantor_ui::components::section_label(t("Table Element"), cx))
                            .child(list),
                    )
                    .child(preview),
            )
            .child(tools)
            .child(note(
                tf(
                    "Element formatting: {}",
                    &[&summary(&d.spec.format(element))],
                ),
                false,
                cx,
            ))
            .child(
                Checkbox::new("ts-default")
                    .label(t("Set as default table style for this document"))
                    .checked(d.default)
                    .on_click(move |on, _, cx| {
                        let on = *on;
                        let _ = dv.update(cx, |this, cx| {
                            if let Some(d) = this.table_style_dialog.as_mut() {
                                d.default = on;
                            }
                            cx.notify();
                        });
                    }),
            )
            .children(d.error.clone().map(|e| note(e, true, cx)))
            .child(dialog::buttons(
                "ts-buttons",
                t("OK"),
                true,
                view,
                Self::table_style_ok,
                Self::close_table_style,
            ))
    }

    fn render_element_format(
        &self,
        d: &TableStyleDialog,
        f: &FormatDraft,
        view: &WeakEntity<Self>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let cur = &f.f;
        let check = |id: &'static str,
                     label: &'static str,
                     on: bool,
                     set: fn(&mut TableElementFormat, bool)| {
            let v = view.clone();
            Checkbox::new(id)
                .label(label)
                .checked(on)
                .on_click(move |on, _, cx| {
                    let on = *on;
                    let _ = v.update(cx, |this, cx| {
                        if let Some(f) = draft(this) {
                            set(&mut f.f, on);
                        }
                        cx.notify();
                    });
                })
        };
        let font = v_flex()
            .gap(SPACE_3)
            .child(kantor_ui::components::section_label(t("Font"), cx))
            .child(
                h_flex()
                    .gap(SPACE_6)
                    .child(check(
                        "tsf-bold",
                        t("Bold"),
                        cur.bold == Some(true),
                        |f, v| f.bold = v.then_some(true),
                    ))
                    .child(check(
                        "tsf-italic",
                        t("Italic"),
                        cur.italic == Some(true),
                        |f, v| f.italic = v.then_some(true),
                    ))
                    .child(check(
                        "tsf-underline",
                        t("Underline"),
                        cur.underline == Some(true),
                        |f, v| f.underline = v.then_some(true),
                    ))
                    .child(check(
                        "tsf-strike",
                        t("Strikethrough"),
                        cur.strike == Some(true),
                        |f, v| f.strike = v.then_some(true),
                    )),
            )
            .child(row(
                t("Colour"),
                swatches("tsf-font", cur.font, true, view, |d, c| d.f.font = c, cx),
                cx,
            ));
        // Border: the pen, presets, edges and a picture of the cell.
        let pv = view.clone();
        let pen = f.pen;
        let pen_label = LINES
            .iter()
            .find(|(l, _)| *l == pen)
            .map_or("", |(_, n)| *n);
        let pen_button = Button::new("tsf-pen")
            .small()
            .outline()
            .label(t_dyn(pen_label).to_owned())
            .dropdown_caret(true)
            .dropdown_menu(move |mut m, _, _| {
                for (l, n) in LINES {
                    let v = pv.clone();
                    m = m.item(
                        PopupMenuItem::new(t_dyn(n).to_owned())
                            .checked(l == pen)
                            .on_click(move |_, _, cx| {
                                let _ = v.update(cx, |this, cx| {
                                    if let Some(f) = draft(this) {
                                        f.pen = l;
                                    }
                                    cx.notify();
                                });
                            }),
                    );
                }
                m
            });
        let edge_button = |id: (&'static str, usize), e: Edge, label: &'static str, on: bool| {
            let v = view.clone();
            Button::new(id)
                .small()
                .label(t_dyn(label).to_owned())
                .when_else(on, |b| b.primary(), |b| b.outline())
                .on_click(move |_, _, cx| {
                    let _ = v.update(cx, |this, cx| {
                        if let Some(f) = draft(this) {
                            e.apply(f);
                        }
                        cx.notify();
                    });
                })
        };
        let mut presets = h_flex().gap(SPACE_2);
        for (i, (e, label)) in Edge::PRESETS.into_iter().enumerate() {
            presets = presets.child(edge_button(("tsf-preset", i), e, label, false));
        }
        let mut edges = h_flex().gap(SPACE_2).flex_wrap();
        let mut probe = cur.clone();
        for (i, (e, label)) in Edge::EDGES.into_iter().enumerate() {
            let on = e.slot(&mut probe).is_some_and(|s| s.is_some());
            edges = edges.child(edge_button(("tsf-edge", i), e, label, on));
        }
        let border = v_flex()
            .gap(SPACE_3)
            .child(kantor_ui::components::section_label(t("Border"), cx))
            .child(
                h_flex()
                    .gap(SPACE_4)
                    .child(row(t("Line"), pen_button, cx))
                    .child(border_picture(cur, cx)),
            )
            .child(row(
                t("Line colour"),
                swatches(
                    "tsf-pen-color",
                    Some(f.pen_color),
                    false,
                    view,
                    |d, c| d.pen_color = c.unwrap_or(0),
                    cx,
                ),
                cx,
            ))
            .child(presets)
            .child(edges);
        let fill = v_flex()
            .gap(SPACE_3)
            .child(kantor_ui::components::section_label(t("Fill"), cx))
            .child(swatches(
                "tsf-fill",
                cur.fill,
                true,
                view,
                |d, c| d.f.fill = c,
                cx,
            ));
        let clear = view.clone();
        dialog::card(
            "table-style-format",
            tf("Format Cells · {}", &[&t_dyn(d.element.label())]),
            px(560.),
            cx,
        )
        .child(font)
        .child(border)
        .child(fill)
        .child(
            h_flex()
                .justify_between()
                .child(
                    Button::new("tsf-clear")
                        .small()
                        .ghost()
                        .label(t("Clear"))
                        .on_click(move |_, _, cx| {
                            let _ = clear.update(cx, |this, cx| {
                                if let Some(f) = draft(this) {
                                    f.f = TableElementFormat::default();
                                }
                                cx.notify();
                            });
                        }),
                )
                .child(dialog::buttons(
                    "tsf-buttons",
                    t("OK"),
                    true,
                    view,
                    Self::format_ok,
                    Self::format_cancel,
                )),
        )
    }
}

/// The Format… draft of the open dialog.
fn draft(this: &mut SheetsView) -> Option<&mut FormatDraft> {
    this.table_style_dialog.as_mut()?.format.as_mut()
}

/// A row of colour swatches (with "no colour" first when `none`).
fn swatches(
    id: &'static str,
    current: Option<Rgb>,
    none: bool,
    view: &WeakEntity<SheetsView>,
    set: fn(&mut FormatDraft, Option<Rgb>),
    cx: &App,
) -> Div {
    let t = tokens(cx);
    let mut r = h_flex().gap(SPACE_1).flex_wrap();
    let colors = none
        .then_some(None)
        .into_iter()
        .chain(SWATCHES.iter().flatten().map(|c| Some(*c)));
    for (i, c) in colors.enumerate() {
        let v = view.clone();
        r = r.child(
            div()
                .id((id, i))
                .size(px(18.))
                .rounded(RADIUS_SM)
                .border_2()
                .border_color(if current == c { t.accent } else { t.border })
                .bg(c.map(hsla).unwrap_or(t.surface))
                .when(c.is_none(), |d| {
                    d.child(
                        div()
                            .text_size(FONT_CAPTION)
                            .text_color(t.fg_dim)
                            .flex()
                            .justify_center()
                            .child("–"),
                    )
                })
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    let _ = v.update(cx, |this, cx| {
                        if let Some(f) = draft(this) {
                            set(f, c);
                        }
                        cx.notify();
                    });
                }),
        );
    }
    r
}

/// A 2 × 2 block of cells with the element's borders, as Format Cells
/// ▸ Border previews it.
fn border_picture(f: &TableElementFormat, cx: &App) -> Div {
    let t = tokens(cx);
    let color = |l: &Option<BorderLine>| l.map(|l| hsla(l.color.unwrap_or(0)));
    let w = |l: &Option<BorderLine>| px(l.map_or(1., |l| l.style.width()));
    let cell = |top: &Option<BorderLine>,
                bottom: &Option<BorderLine>,
                left: &Option<BorderLine>,
                right: &Option<BorderLine>| {
        let faint = t.border;
        div()
            .size(px(28.))
            .bg(f.fill.map(hsla).unwrap_or(gpui::white()))
            .border_t(w(top))
            .border_b(w(bottom))
            .border_l(w(left))
            .border_r(w(right))
            .border_color(faint)
            .when_some(color(top), |d, c| d.border_color(c))
            .when_some(color(bottom), |d, c| d.border_color(c))
            .when_some(color(left), |d, c| d.border_color(c))
            .when_some(color(right), |d, c| d.border_color(c))
    };
    let (h, v) = (&f.horizontal, &f.vertical);
    v_flex()
        .child(
            h_flex()
                .child(cell(&f.top, h, &f.left, v))
                .child(cell(&f.top, h, v, &f.right)),
        )
        .child(
            h_flex()
                .child(cell(h, &f.bottom, &f.left, v))
                .child(cell(h, &f.bottom, v, &f.right)),
        )
}

/// A table style drawn small: each cell's fill and lines, and (`text`) a
/// dash of text in its font colour (thicker when bold).
pub(crate) fn preview_picture(
    cells: &[Vec<TableCellLook>],
    w: Pixels,
    h: Pixels,
    text: bool,
) -> Div {
    let mut pic = v_flex();
    for r in cells {
        let mut line = h_flex();
        for c in r {
            let ink = c.font_color.map(hsla).unwrap_or(gpui::black());
            line = line.child(
                div()
                    .w(w)
                    .h(h)
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(c.fill.map(hsla).unwrap_or(gpui::white()))
                    .when_some(c.bottom.as_ref(), |d, b| {
                        d.border_b(px(b.style.width().min(2.)))
                            .border_color(hsla(b.color.unwrap_or(0)))
                    })
                    .when_some(c.right.as_ref(), |d, b| {
                        d.border_r(px(b.style.width().min(2.)))
                            .border_color(hsla(b.color.unwrap_or(0)))
                    })
                    .when_some(c.top.as_ref(), |d, b| {
                        d.border_t(px(b.style.width().min(2.)))
                            .border_color(hsla(b.color.unwrap_or(0)))
                    })
                    .when_some(c.left.as_ref(), |d, b| {
                        d.border_l(px(b.style.width().min(2.)))
                            .border_color(hsla(b.color.unwrap_or(0)))
                    })
                    .when(text, |d| {
                        d.child(div().w(w * 0.5).h(px(if c.bold { 2. } else { 1. })).bg(ink))
                    }),
            );
        }
        pic = pic.child(line);
    }
    pic
}

/// "Bold; Fill; Borders": what an element format sets.
fn summary(f: &TableElementFormat) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if f.bold == Some(true) {
        parts.push(t("Bold"));
    }
    if f.italic == Some(true) {
        parts.push(t("Italic"));
    }
    if f.underline == Some(true) {
        parts.push(t("Underline"));
    }
    if f.strike == Some(true) {
        parts.push(t("Strikethrough"));
    }
    if f.font.is_some() {
        parts.push(t("Font colour"));
    }
    if f.fill.is_some() {
        parts.push(t("Fill"));
    }
    if [f.left, f.right, f.top, f.bottom, f.horizontal, f.vertical]
        .iter()
        .any(|l| l.is_some())
    {
        parts.push(t("Borders"));
    }
    if parts.is_empty() {
        t("none").to_owned()
    } else {
        parts.join("; ")
    }
}

/// The right-click menu of a style in the gallery: Modify, Duplicate,
/// Delete (custom styles), Set As Default.
pub(crate) fn style_menu(
    menu: PopupMenu,
    name: &str,
    custom: bool,
    view: &WeakEntity<SheetsView>,
) -> PopupMenu {
    type Run = fn(&mut SheetsView, &str, &mut Window, &mut Context<SheetsView>);
    let item = |label: &'static str, run: Run| {
        let v = view.clone();
        let n = name.to_owned();
        PopupMenuItem::new(t(label)).on_click(move |_, w, cx| {
            let _ = v.update(cx, |this, cx| run(this, &n, w, cx));
        })
    };
    let mut menu = menu;
    if custom {
        menu = menu.item(item(tk("Modify…"), |v, n, w, cx| {
            v.modify_table_style_ui(n, w, cx)
        }));
    }
    menu = menu.item(item(tk("Duplicate…"), |v, n, w, cx| {
        v.duplicate_table_style_ui(n, w, cx)
    }));
    if custom {
        menu = menu.item(item(tk("Delete"), |v, n, w, cx| {
            v.delete_table_style_ui(n, w, cx)
        }));
    }
    menu.separator()
        .item(item(tk("Set As Default"), |v, n, w, cx| {
            v.default_table_style_ui(n, w, cx)
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn engine_texts_have_indonesian() {
        for e in ENGINE_TEXTS {
            assert!(kantor_ui::i18n::indonesian(e).is_some(), "{e:?}");
        }
        for e in TableElement::ALL {
            assert!(ENGINE_TEXTS.contains(&e.label()), "{e:?}");
        }
    }
}
