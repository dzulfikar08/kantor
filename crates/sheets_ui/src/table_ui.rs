//! Format as Table (Ctrl+T, Ctrl+L) and the Table Design panel.
//!
//! * The Format as Table card: the range (the selection, else the data
//!   around the active cell; typed references work), "My table has headers"
//!   and a gallery of the built-in Light / Medium / Dark styles drawn by the
//!   same code that paints tables on the grid.
//! * Table Design is a side panel beside the grid (like PivotTable Fields)
//!   for the table at the active cell: name (Rename), style, the style
//!   options (header row, total row, banded rows and columns, first and last
//!   column, filter button), the total row function of each column, and
//!   Convert to Range. Every change is one undo step
//!   (`sheets_engine::table_edit`).

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{self, Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, t, t_dyn, tf, tk};
use kantor_ui::{metrics::*, tokens};
use sheets_engine::{TableOptions, TotalFunc, table_style_names};

use crate::dialog::{self, note, row};
use crate::{GRID_CONTEXT, SheetsView};

actions!(sheets, [FormatAsTable, TableDesign, ConvertTableToRange]);

const PANEL_WIDTH: Pixels = px(300.);
const DEFAULT_STYLE: &str = "TableStyleMedium2";

/// Texts of `sheets_engine` (table edits, total row functions) shown here,
/// translated where shown.
#[cfg(test)]
const ENGINE_TEXTS: &[&str] = &[
    tk("Select the cells of the table, not whole rows or columns"),
    tk("Too many columns"),
    tk("It overlaps a PivotTable"),
    tk("Tables can't contain merged cells"),
    tk("It overlaps the sheet's filter: remove the filter first"),
    tk("No room for a header row"),
    tk("No room for a data row"),
    tk("No room for a total row"),
    tk("A table needs a data row"),
    tk("The row above the table must be empty to show the header row"),
    tk("Turn on the total row first"),
    tk("A table name starts with a letter and has no spaces or symbols"),
    tk("Sum"),
    tk("Average"),
    tk("Count"),
    tk("Count Numbers"),
    tk("Max"),
];

pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-t"),
            FormatAsTable,
            Some(GRID_CONTEXT),
        ),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-l"),
            FormatAsTable,
            Some(GRID_CONTEXT),
        ),
    ]);
}

/// The Format as Table card.
pub(crate) struct TableDialog {
    sheet: u32,
    range: Entity<InputState>,
    headers: bool,
    style: String,
    error: Option<String>,
    _sub: Subscription,
}

/// The Table Design panel.
pub(crate) struct TablePanel {
    /// Renaming: the name being typed.
    rename: Option<(Entity<InputState>, Subscription)>,
    error: Option<String>,
}

impl SheetsView {
    /// Open Format as Table for the selection or the data around the
    /// active cell.
    pub fn format_as_table_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let (sheet, suggestion, in_table) = {
            let grid = self.table.read(cx).delegate();
            let (r, c) = self.selected;
            (
                grid.sheet,
                grid.book.table_suggestion(grid.sheet, grid.range(), (r, c)),
                grid.book.table_at(grid.sheet, r, c).is_some(),
            )
        };
        if in_table {
            // Excel opens the style gallery for an existing table.
            self.table_panel = Some(TablePanel {
                rename: None,
                error: None,
            });
            self.status =
                t("This cell is already in a table: its options are in Table Design").into();
            cx.notify();
            return;
        }
        let range = cx.new(|cx| InputState::new(window, cx));
        range.update(cx, |i, cx| {
            i.set_value(dialog::range_text(&suggestion), window, cx);
            i.focus(window, cx);
        });
        let sub = cx.subscribe_in(&range, window, |this, _, e: &InputEvent, w, cx| match e {
            InputEvent::Change => {
                if let Some(d) = this.table_dialog.as_mut() {
                    d.error = None;
                }
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.table_dialog_ok(w, cx),
            _ => {}
        });
        self.table_dialog = Some(TableDialog {
            sheet,
            range,
            headers: true,
            style: DEFAULT_STYLE.into(),
            error: None,
            _sub: sub,
        });
        cx.notify();
    }

    fn close_table_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.table_dialog.take().is_some() {
            self.focus_grid(window, cx);
            cx.notify();
        }
    }

    fn table_dialog_ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.table_dialog.as_ref() else {
            return;
        };
        let text = d.range.read(cx).value().to_string();
        let (sheet, headers, style) = (d.sheet, d.headers, d.style.clone());
        let Some(range) = dialog::parse_cells(&text) else {
            if let Some(d) = self.table_dialog.as_mut() {
                d.error = Some(t("Type the table's cells, such as $A$1:$D$20").into());
            }
            cx.notify();
            return;
        };
        let res = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let res = grid.book.create_table(sheet, range, headers, &style);
            grid.resize();
            table.refresh(cx);
            res
        });
        match res {
            Ok(name) => {
                self.table_dialog = None;
                self.status = tf("{} created", &[&name]).into();
                self.table_panel = Some(TablePanel {
                    rename: None,
                    error: None,
                });
                self.focus_grid(window, cx);
                self.after_table_edit(window, cx);
            }
            Err(e) => {
                if let Some(d) = self.table_dialog.as_mut() {
                    d.error = Some(t_dyn(&format!("{e:#}")).to_owned());
                }
                cx.notify();
            }
        }
    }

    fn after_table_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load_selected_into_formula(window, cx);
        self.update_title(window, cx);
        cx.notify();
    }

    /// Show or hide Table Design.
    pub fn table_design_doc(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.table_panel = match self.table_panel {
            Some(_) => None,
            None => Some(TablePanel {
                rename: None,
                error: None,
            }),
        };
        cx.notify();
    }

    /// The table at the active cell: its name.
    fn active_table(&self, cx: &App) -> Option<String> {
        let grid = self.table.read(cx).delegate();
        let (r, c) = self.selected;
        grid.book.table_at(grid.sheet, r, c).map(|t| t.name)
    }

    /// Run a table edit on the active table and report it.
    fn edit_table(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: &str,
        f: impl FnOnce(&mut sheets_engine::Workbook, &str) -> anyhow::Result<()>,
    ) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let Some(name) = self.active_table(cx) else {
            self.status = t("Select a cell in a table first").into();
            cx.notify();
            return;
        };
        let res = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let res = f(&mut grid.book, &name);
            grid.resize();
            table.refresh(cx);
            res
        });
        match res {
            Ok(()) => {
                self.status = done.to_owned().into();
                if let Some(p) = self.table_panel.as_mut() {
                    p.error = None;
                }
            }
            Err(e) => {
                let msg = t_dyn(&format!("{e:#}")).to_owned();
                self.status = msg.clone().into();
                if let Some(p) = self.table_panel.as_mut() {
                    p.error = Some(msg);
                }
            }
        }
        self.after_table_edit(window, cx);
    }

    fn set_table_option(
        &mut self,
        f: impl FnOnce(&mut TableOptions),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_table(window, cx, t("Table changed"), |book, name| {
            let mut o = book
                .table_options(name)
                .ok_or_else(|| anyhow::anyhow!("no table {name}"))?;
            f(&mut o);
            book.set_table_options(name, o)
        });
    }

    /// Convert to Range for the table at the active cell.
    pub fn convert_table_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_table(window, cx, t("Table converted to a range"), |book, name| {
            book.convert_table_to_range(name)
        });
    }

    fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.active_table(cx) else {
            return;
        };
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |i, cx| {
            i.set_value(name, window, cx);
            i.focus(window, cx);
        });
        let sub = cx.subscribe_in(&input, window, |this, input, e: &InputEvent, w, cx| {
            if let InputEvent::PressEnter { .. } = e {
                let new = input.read(cx).value().to_string();
                if let Some(p) = this.table_panel.as_mut() {
                    p.rename = None;
                }
                this.edit_table(w, cx, t("Table renamed"), |book, name| {
                    book.rename_table(name, &new)
                });
                this.focus_grid(w, cx);
            }
        });
        if let Some(p) = self.table_panel.as_mut() {
            p.rename = Some((input, sub));
        }
        cx.notify();
    }

    pub(crate) fn render_table_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.table_dialog.as_ref()?;
        let view = cx.entity().downgrade();
        let hv = view.clone();
        let gallery = self.style_gallery(
            &d.style,
            &view,
            |this, style, _, _| {
                if let Some(d) = this.table_dialog.as_mut() {
                    d.style = style;
                }
            },
            cx,
        );
        let card = dialog::card("table-insert", t("Format as Table"), px(430.), cx)
            .key_context("KantorTableDialog")
            .on_action(cx.listener(|this, _: &input::Escape, w, cx| this.close_table_dialog(w, cx)))
            .child(row(
                t("Where is the data?"),
                Input::new(&d.range).small(),
                cx,
            ))
            .child(
                Checkbox::new("table-headers")
                    .label(t("My table has headers"))
                    .checked(d.headers)
                    .on_click(move |on, _, cx| {
                        let on = *on;
                        let _ = hv.update(cx, |this, cx| {
                            if let Some(d) = this.table_dialog.as_mut() {
                                d.headers = on;
                            }
                            cx.notify();
                        });
                    }),
            )
            .child(kantor_ui::components::section_label(
                tf("Style · {}", &[&style_label(&d.style)]),
                cx,
            ))
            .child(gallery)
            .children(d.error.clone().map(|e| note(e, true, cx)))
            .child(dialog::buttons(
                "table-buttons",
                t("Create"),
                true,
                &view,
                Self::table_dialog_ok,
                Self::close_table_dialog,
            ));
        Some(dialog::place(card).into_any_element())
    }

    /// The built-in styles as small pictures; `pick` gets the clicked name.
    fn style_gallery(
        &self,
        current: &str,
        view: &WeakEntity<Self>,
        pick: fn(&mut SheetsView, String, &mut Window, &mut Context<SheetsView>),
        cx: &App,
    ) -> Stateful<Div> {
        let t = tokens(cx);
        let book = &self.table.read(cx).delegate().book;
        let mut grid = h_flex().flex_wrap().gap(SPACE_2);
        for (i, name) in table_style_names().into_iter().enumerate() {
            let Some(cells) = book.table_style_preview(&name) else {
                continue;
            };
            let selected = name == current;
            let v = view.clone();
            let label: SharedString = style_label(&name).into();
            let mut pic = v_flex().gap(px(1.));
            for r in &cells {
                let mut line = h_flex().gap(px(1.));
                for c in r {
                    line = line.child(
                        div()
                            .w(px(9.))
                            .h(px(5.))
                            .bg(c.fill.map(crate::look::hsla).unwrap_or(gpui::white()))
                            .when_some(c.bottom.as_ref().and_then(|b| b.color), |d, col| {
                                d.border_b_1().border_color(crate::look::hsla(col))
                            }),
                    );
                }
                pic = pic.child(line);
            }
            let name2 = name.clone();
            grid = grid.child(
                div()
                    .id(("table-style", i))
                    .p(px(3.))
                    .rounded(RADIUS_SM)
                    .border_2()
                    .border_color(if selected {
                        t.accent
                    } else {
                        gpui::transparent_black()
                    })
                    .hover(|s| s.bg(t.hover))
                    .cursor_pointer()
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(window, cx)
                    })
                    .child(pic)
                    .on_click(move |_, w, cx| {
                        let n = name2.clone();
                        let _ = v.update(cx, |this, cx| {
                            pick(this, n, w, cx);
                            cx.notify();
                        });
                    }),
            );
        }
        div()
            .id("table-styles")
            .max_h(px(220.))
            .overflow_y_scroll()
            .p(SPACE_2)
            .rounded(RADIUS_SM)
            .border_1()
            .border_color(t.border)
            .bg(t.bg)
            .child(grid)
    }

    pub(crate) fn render_table_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.table_panel.as_ref()?;
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let close = view.clone();
        let header = h_flex()
            .flex_none()
            .gap(SPACE_3)
            .child(
                div()
                    .flex_1()
                    .text_size(FONT_EMPHASIS)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(i18n::t("Table Design")),
            )
            .child(
                Button::new("table-panel-close")
                    .ghost()
                    .xsmall()
                    .icon(Lucide::X)
                    .tooltip(i18n::t("Close"))
                    .on_click(move |_, _, cx| {
                        let _ = close.update(cx, |this, cx| {
                            this.table_panel = None;
                            cx.notify();
                        });
                    }),
            );
        let (info, opts, totals) = {
            let grid = self.table.read(cx).delegate();
            let (r, c) = self.selected;
            let info = grid.book.table_at(grid.sheet, r, c);
            let opts = info.as_ref().and_then(|i| grid.book.table_options(&i.name));
            let totals = info
                .as_ref()
                .map(|i| grid.book.table_totals(&i.name))
                .unwrap_or_default();
            (info, opts, totals)
        };
        let body: AnyElement = match (info, opts) {
            (Some(info), Some(opts)) => {
                let mut b = v_flex().gap(SPACE_4);
                // Name.
                let name_row = match &p.rename {
                    Some((input, _)) => h_flex()
                        .gap(SPACE_2)
                        .child(div().flex_1().child(Input::new(input).small()))
                        .child(note(i18n::t("Enter to rename"), false, cx)),
                    None => {
                        let rv = view.clone();
                        h_flex()
                            .gap(SPACE_3)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(SharedString::from(info.name.clone())),
                            )
                            .child(
                                Button::new("table-rename")
                                    .small()
                                    .outline()
                                    .label(i18n::t("Rename"))
                                    .on_click(move |_, w, cx| {
                                        let _ = rv.update(cx, |this, cx| this.start_rename(w, cx));
                                    }),
                            )
                    }
                };
                b = b
                    .child(kantor_ui::components::section_label(i18n::t("Table name"), cx))
                    .child(name_row)
                    .child(note(info.area.name(), false, cx));
                // Options.
                let check = |id: &'static str,
                             label: &'static str,
                             on: bool,
                             set: fn(&mut TableOptions, bool)| {
                    let v = view.clone();
                    Checkbox::new(id)
                        .label(label)
                        .checked(on)
                        .on_click(move |on, w, cx| {
                            let on = *on;
                            let _ = v.update(cx, |this, cx| {
                                this.set_table_option(|o| set(o, on), w, cx)
                            });
                        })
                };
                b = b
                    .child(kantor_ui::components::section_label(i18n::t("Table style options"), cx))
                    .child(
                        h_flex()
                            .items_start()
                            .gap(SPACE_6)
                            .child(
                                v_flex()
                                    .gap(SPACE_3)
                                    .child(check("to-header", i18n::t("Header Row"), opts.header_row, |o, v| o.header_row = v))
                                    .child(check("to-total", i18n::t("Total Row"), opts.total_row, |o, v| o.total_row = v))
                                    .child(check("to-brows", i18n::t("Banded Rows"), opts.banded_rows, |o, v| o.banded_rows = v))
                                    .child(check("to-bcols", i18n::t("Banded Columns"), opts.banded_columns, |o, v| o.banded_columns = v)),
                            )
                            .child(
                                v_flex()
                                    .gap(SPACE_3)
                                    .child(check("to-first", i18n::t("First Column"), opts.first_column, |o, v| o.first_column = v))
                                    .child(check("to-last", i18n::t("Last Column"), opts.last_column, |o, v| o.last_column = v))
                                    .child(check("to-filter", i18n::t("Filter Button"), opts.filter_button, |o, v| o.filter_button = v)),
                            ),
                    );
                // Total row functions.
                if opts.total_row {
                    let mut list = v_flex().gap(SPACE_2);
                    for (i, (col, f)) in info.columns.iter().zip(totals.iter()).enumerate() {
                        let v = view.clone();
                        let current = *f;
                        list = list.child(row(
                            SharedString::from(col.clone()),
                            Button::new(("table-total", i))
                                .outline()
                                .small()
                                .label(i18n::t(current.label()))
                                .dropdown_caret(true)
                                .dropdown_menu(move |mut m, _, _| {
                                    for func in TotalFunc::ALL {
                                        let v = v.clone();
                                        m = m.item(
                                            PopupMenuItem::new(i18n::t(func.label()))
                                                .checked(func == current)
                                                .on_click(move |_, w, cx| {
                                                    let _ = v.update(cx, |this, cx| {
                                                        this.edit_table(w, cx, i18n::t("Total changed"), |book, name| {
                                                            book.set_table_total(name, i, func)
                                                        })
                                                    });
                                                }),
                                        );
                                    }
                                    m
                                }),
                            cx,
                        ));
                    }
                    b = b
                        .child(kantor_ui::components::section_label(i18n::t("Total row"), cx))
                        .child(list);
                }
                let style = opts.style.clone().unwrap_or_default();
                b = b
                    .child(kantor_ui::components::section_label(
                        i18n::tf("Style · {}", &[&style_label(&style)]),
                        cx,
                    ))
                    .child(self.style_gallery(
                        &style,
                        &view,
                        |this, s, w, cx| this.set_table_option(|o| o.style = Some(s), w, cx),
                        cx,
                    ));
                let cv = view.clone();
                b = b.child(
                    Button::new("table-convert")
                        .small()
                        .outline()
                        .label(i18n::t("Convert to Range"))
                        .on_click(move |_, w, cx| {
                            let _ = cv.update(cx, |this, cx| this.convert_table_doc(w, cx));
                        }),
                );
                b.children(p.error.clone().map(|e| note(e, true, cx)))
                    .into_any_element()
            }
            _ => div()
                .text_size(FONT_SMALL)
                .text_color(t.fg_muted)
                .child(i18n::t("Select a cell in a table to change it, or make one with Format as Table (Ctrl+T)."))
                .into_any_element(),
        };
        Some(
            v_flex()
                .id("table-panel")
                .key_context("KantorTablePanel")
                .flex_none()
                .w(PANEL_WIDTH)
                .h_full()
                .gap(SPACE_4)
                .p(SPACE_5)
                .bg(t.chrome)
                .border_l_1()
                .border_color(t.border)
                .text_size(FONT_BODY)
                .text_color(t.fg)
                .overflow_y_scroll()
                .on_action(cx.listener(|this, _: &input::Escape, w, cx| {
                    if let Some(p) = this.table_panel.as_mut() {
                        p.rename = None;
                    }
                    this.focus_grid(w, cx);
                    cx.notify();
                }))
                .child(header)
                .child(body)
                .into_any_element(),
        )
    }

    pub(crate) fn table_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(
            cx.listener(|this, _: &FormatAsTable, w, cx| this.format_as_table_doc(w, cx)),
        )
        .on_action(cx.listener(|this, _: &TableDesign, w, cx| this.table_design_doc(w, cx)))
        .on_action(
            cx.listener(|this, _: &ConvertTableToRange, w, cx| this.convert_table_doc(w, cx)),
        )
    }
}

/// `TableStyleMedium2` → `Medium 2`.
fn style_label(name: &str) -> String {
    for (family, label) in [
        ("Light", tk("Light")),
        ("Medium", tk("Medium")),
        ("Dark", tk("Dark")),
    ] {
        if let Some(n) = name.strip_prefix(&format!("TableStyle{family}")) {
            return format!("{} {n}", t(label));
        }
    }
    if name.is_empty() {
        t("None").into()
    } else {
        name.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn engine_texts_have_indonesian() {
        for e in ENGINE_TEXTS {
            assert!(kantor_ui::i18n::indonesian(e).is_some(), "{e:?}");
        }
    }
}
