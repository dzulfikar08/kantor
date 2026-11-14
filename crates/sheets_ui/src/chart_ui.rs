//! Insert Chart and the Chart panel.
//!
//! * Insert Chart (toolbar, palette): a chart of the picked type from the
//!   selection (else the data around the active cell), placed beside the
//!   data and selected; moving, resizing and deleting work as for a file's
//!   charts ([`crate::drawings`]).
//! * The Chart panel beside the grid, for the selected chart (made in
//!   Kantor or from the file): type, title, legend position, axis titles,
//!   series colours, series from rows or columns, data range, delete. Each
//!   change is one undo step (`sheets_engine::chart_edit`; a file's chart
//!   part is changed in place, `sheets_engine::file_chart`). Combo, 3-D and
//!   other types Kantor can't make keep their type and data range.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    Disableable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{self, Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, t_dyn, tf};
use kantor_ui::{metrics::*, tokens};
use sheets_engine::drawing::DrawingKey;
use sheets_engine::{ChartInfo, ChartSpec, ChartType, LegendPos, SeriesColor};

use crate::SheetsView;
use crate::dialog::{self, choices, note, row};

actions!(sheets, [InsertChart, ChartOptions]);

const PANEL_WIDTH: Pixels = px(290.);

/// The Chart panel.
pub(crate) struct ChartPanel {
    /// The chart the inputs were filled for.
    loaded: Option<DrawingKey>,
    title: Entity<InputState>,
    range: Entity<InputState>,
    x_title: Entity<InputState>,
    y_title: Entity<InputState>,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl SheetsView {
    /// Insert a chart of `kind` from the selection.
    pub fn insert_chart_doc(
        &mut self,
        kind: ChartType,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let (r, c) = self.selected;
        let res = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
            let data = grid.book.table_suggestion(sheet, grid.range(), (r, c));
            let res = grid.book.insert_chart(sheet, data, kind);
            table.refresh(cx);
            res
        });
        match res {
            Ok(key) => {
                self.drawings.borrow_mut().selected = Some(key);
                self.status = tf("{} chart inserted", &[&t_dyn(kind.label())]).into();
                self.open_chart_panel(window, cx);
            }
            Err(e) => self.status = format!("{e:#}").into(),
        }
        self.update_title(window, cx);
        cx.notify();
    }

    fn open_chart_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chart_panel.is_some() {
            return;
        }
        let title = cx.new(|cx| InputState::new(window, cx).placeholder(i18n::t("No title")));
        let range = cx.new(|cx| InputState::new(window, cx).placeholder(i18n::t("e.g. A1:C13")));
        let x_title = cx.new(|cx| InputState::new(window, cx).placeholder(i18n::t("No title")));
        let y_title = cx.new(|cx| InputState::new(window, cx).placeholder(i18n::t("No title")));
        let mut subs = Vec::new();
        for input in [&title, &x_title, &y_title] {
            subs.push(
                cx.subscribe_in(input, window, |this, _, e: &InputEvent, w, cx| {
                    if let InputEvent::PressEnter { .. } | InputEvent::Blur = e {
                        this.apply_chart_inputs(w, cx);
                    }
                }),
            );
        }
        subs.push(
            cx.subscribe_in(&range, window, |this, _, e: &InputEvent, w, cx| {
                if let InputEvent::PressEnter { .. } = e {
                    this.apply_chart_inputs(w, cx);
                }
            }),
        );
        self.chart_panel = Some(ChartPanel {
            loaded: None,
            title,
            range,
            x_title,
            y_title,
            error: None,
            _subs: subs,
        });
    }

    /// Show or hide the Chart panel.
    pub fn chart_options_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chart_panel.take().is_none() {
            self.open_chart_panel(window, cx);
        }
        cx.notify();
    }

    /// The selected chart and what the panel shows for it.
    fn selected_chart(&self, cx: &App) -> Option<(DrawingKey, ChartInfo)> {
        // The selected chart, else the one the panel shows (clicking in the
        // panel deselects the chart on the grid).
        let key = match self.drawings.borrow().selected {
            Some(k) => k,
            None => self.chart_panel.as_ref()?.loaded?,
        };
        let info = self.table.read(cx).delegate().book.chart_info(key)?;
        Some((key, info))
    }

    /// `spec`'s data range as typed: with its sheet when it's another one.
    fn data_text(&self, key: DrawingKey, spec: &ChartSpec, cx: &App) -> String {
        let book = &self.table.read(cx).delegate().book;
        match spec.data_sheet.filter(|id| *id != key.sheet_id) {
            Some(id) => {
                let names = book.sheet_names();
                let name = (0..names.len() as u32)
                    .find(|&s| book.sheet_id(s) == id)
                    .and_then(|s| names.get(s as usize).cloned())
                    .unwrap_or_default();
                format!(
                    "{}!{}",
                    sheets_engine::quote_sheet_name(&name),
                    spec.data.name()
                )
            }
            None => spec.data.name(),
        }
    }

    /// Fill the panel's inputs for the selected chart (from `render`, which
    /// has the window).
    pub(crate) fn sync_chart_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.chart_panel.as_ref() else {
            return;
        };
        let sel = self.selected_chart(cx);
        let key = sel.as_ref().map(|s| s.0);
        if p.loaded == key {
            return;
        }
        let (title, range, xt, yt) = (
            p.title.clone(),
            p.range.clone(),
            p.x_title.clone(),
            p.y_title.clone(),
        );
        if let Some((key, info)) = sel {
            let spec = &info.spec;
            let data = self.data_text(key, spec, cx);
            title.update(cx, |i, cx| i.set_value(spec.title.clone(), window, cx));
            range.update(cx, |i, cx| i.set_value(data, window, cx));
            xt.update(cx, |i, cx| i.set_value(spec.x_title.clone(), window, cx));
            yt.update(cx, |i, cx| i.set_value(spec.y_title.clone(), window, cx));
        }
        if let Some(p) = self.chart_panel.as_mut() {
            p.loaded = key;
            p.error = None;
        }
    }

    fn edit_chart(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut ChartSpec) -> Result<(), String>,
    ) {
        let Some((key, info)) = self.selected_chart(cx) else {
            return;
        };
        let mut spec = info.spec;
        let res = f(&mut spec).and_then(|()| {
            self.table.update(cx, |table, cx| {
                let r = table.delegate_mut().book.set_chart(key, spec);
                table.refresh(cx);
                r.map_err(|e| format!("{e:#}"))
            })
        });
        if let Some(p) = self.chart_panel.as_mut() {
            p.error = res.err();
        }
        self.update_title(window, cx);
        cx.notify();
    }

    fn apply_chart_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.chart_panel.as_ref() else {
            return;
        };
        let title = p.title.read(cx).value().to_string();
        let range = p.range.read(cx).value().to_string();
        let x_title = p.x_title.read(cx).value().to_string();
        let y_title = p.y_title.read(cx).value().to_string();
        let Some((key, info)) = self.selected_chart(cx) else {
            return;
        };
        // An unchanged range stays as the chart has it (a file's chart whose
        // series Kantor can't read back as one block keeps them).
        let range_changed = range.trim() != self.data_text(key, &info.spec, cx);
        let sheet = if range_changed {
            match self.chart_data_sheet(key, &range, cx) {
                Ok(s) => Some(s),
                Err(e) => {
                    if let Some(p) = self.chart_panel.as_mut() {
                        p.error = Some(e);
                    }
                    cx.notify();
                    return;
                }
            }
        } else {
            None
        };
        self.edit_chart(window, cx, |s| {
            s.title = title;
            s.x_title = x_title;
            s.y_title = y_title;
            if let Some(sheet) = sheet {
                s.data_sheet = sheet;
                s.data = dialog::parse_cells(&range)
                    .filter(|r| !r.is_single())
                    .ok_or(i18n::t("Data: type the chart's cells, such as A1:C13"))?;
            }
            Ok(())
        });
    }

    /// The sheet a typed data range names (`None`: the chart's own sheet).
    fn chart_data_sheet(
        &self,
        key: DrawingKey,
        text: &str,
        cx: &App,
    ) -> Result<Option<u32>, String> {
        let t = text.trim().trim_start_matches('=');
        let Some((name, _)) = t.rsplit_once('!') else {
            // A chart sheet has no cells: its data stays on its sheet.
            if sheets_engine::drawing::chart_sheet_of_key(key.sheet_id).is_some() {
                return Ok(self.selected_chart(cx).and_then(|(_, i)| i.spec.data_sheet));
            }
            return Ok(None);
        };
        let name = name.trim();
        let name = name
            .strip_prefix('\'')
            .and_then(|n| n.strip_suffix('\''))
            .map_or_else(|| name.to_owned(), |n| n.replace("''", "'"));
        let book = &self.table.read(cx).delegate().book;
        let i = book
            .sheet_names()
            .iter()
            .position(|n| n.eq_ignore_ascii_case(&name))
            .ok_or_else(|| tf("Data: there's no sheet called {}", &[&name]))?;
        let id = book.sheet_id(i as u32);
        Ok((id != key.sheet_id).then_some(id))
    }

    pub(crate) fn render_chart_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.chart_panel.as_ref()?;
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
                    .child(i18n::t("Chart")),
            )
            .child(
                Button::new("chart-panel-close")
                    .ghost()
                    .xsmall()
                    .icon(Lucide::X)
                    .tooltip(i18n::t("Close"))
                    .on_click(move |_, _, cx| {
                        let _ = close.update(cx, |this, cx| {
                            this.chart_panel = None;
                            cx.notify();
                        });
                    }),
            );
        let a_chart = self.drawings.borrow().selected.is_some();
        let body: AnyElement = match self.selected_chart(cx) {
            Some((key, info)) => {
                let spec = info.spec.clone();
                let tv = view.clone();
                let current = spec.kind;
                let kind: AnyElement = if info.fixed {
                    Button::new("chart-kind")
                        .outline()
                        .small()
                        .label(t_dyn(&info.kind_label).to_owned())
                        .disabled(true)
                        .tooltip(i18n::t("Kantor can't change this chart's type"))
                        .into_any_element()
                } else {
                    Button::new("chart-kind")
                        .outline()
                        .small()
                        .label(t_dyn(&info.kind_label).to_owned())
                        .dropdown_caret(true)
                        .dropdown_menu(move |mut m, _, _| {
                            for k in ChartType::ALL {
                                let v = tv.clone();
                                m = m.item(
                                    PopupMenuItem::new(t_dyn(k.label()))
                                        .checked(k == current)
                                        .on_click(move |_, w, cx| {
                                            let _ = v.update(cx, |this, cx| {
                                                this.edit_chart(w, cx, |s| {
                                                    s.kind = k;
                                                    Ok(())
                                                })
                                            });
                                        }),
                                );
                            }
                            m
                        })
                        .into_any_element()
                };
                let lv = view.clone();
                let legend = spec.legend;
                let legend_button = Button::new("chart-legend")
                    .outline()
                    .small()
                    .label(t_dyn(legend.label()))
                    .dropdown_caret(true)
                    .dropdown_menu(move |mut m, _, _| {
                        for l in LegendPos::ALL {
                            let v = lv.clone();
                            m = m.item(
                                PopupMenuItem::new(t_dyn(l.label()))
                                    .checked(l == legend)
                                    .on_click(move |_, w, cx| {
                                        let _ = v.update(cx, |this, cx| {
                                            this.edit_chart(w, cx, |s| {
                                                s.legend = l;
                                                Ok(())
                                            })
                                        });
                                    }),
                            );
                        }
                        m
                    });
                let colours = series_colours(&info, &view, cx);
                // A chart sheet's chart goes with its sheet (tab menu).
                let on_chart_sheet =
                    sheets_engine::drawing::chart_sheet_of_key(key.sheet_id).is_some();
                let dv = view.clone();
                let av = view.clone();
                v_flex()
                    .gap(SPACE_4)
                    .child(row(i18n::t("Type"), kind, cx))
                    .child(row(i18n::t("Title"), Input::new(&p.title).small(), cx))
                    .child(row(i18n::t("Legend"), legend_button, cx))
                    .when(info.has_axes, |d| {
                        d.child(row(i18n::t("Horizontal axis"), Input::new(&p.x_title).small(), cx))
                            .child(row(i18n::t("Vertical axis"), Input::new(&p.y_title).small(), cx))
                    })
                    .child(colours)
                    .when(!info.fixed, |d| {
                        d.child(row(
                            i18n::t("Series in"),
                            choices(
                                "chart-series",
                                &[(false, i18n::t("Columns")), (true, i18n::t("Rows"))],
                                spec.by_rows,
                                &view,
                                |this, v| this.pending_chart_rows = Some(v),
                            ),
                            cx,
                        ))
                        .child(row(i18n::t("Data"), Input::new(&p.range).small(), cx))
                    })
                    .when(info.fixed, |d| {
                        let msg = i18n::t("Kantor can't change the type or data of a {} chart; titles, legend and colours can be changed.");
                        d.child(note(
                            i18n::fill(msg, &[&t_dyn(&info.kind_label)]),
                            false,
                            cx,
                        ))
                    })
                    .when(info.data_unknown && !info.fixed, |d| {
                        d.child(note(
                            i18n::t("This chart's series don't come from one block of cells: a new data range replaces them."),
                            false,
                            cx,
                        ))
                    })
                    .child(note(i18n::t("Press Enter in a box to apply it."), false, cx))
                    .children(p.error.clone().map(|e| note(e, true, cx)))
                    .child(
                        h_flex()
                            .gap(SPACE_3)
                            .child(
                                Button::new("chart-apply")
                                    .small()
                                    .outline()
                                    .label(i18n::t("Apply"))
                                    .on_click(move |_, w, cx| {
                                        let _ = av.update(cx, |this, cx| this.apply_chart_inputs(w, cx));
                                    }),
                            )
                            .when(!on_chart_sheet, |d| d.child(
                                Button::new("chart-delete")
                                    .small()
                                    .outline()
                                    .icon(Lucide::Trash)
                                    .label(i18n::t("Delete Chart"))
                                    .on_click(move |_, w, cx| {
                                        let _ = dv.update(cx, |this, cx| {
                                            this.drawing_key("delete", w, cx);
                                        });
                                    }),
                            )),
                    )
                    .into_any_element()
            }
            None => div()
                .text_size(FONT_SMALL)
                .text_color(t.fg_muted)
                .child(if a_chart {
                    i18n::t("Kantor can't change this object: select a chart.")
                } else {
                    i18n::t("Select a chart to change it.")
                })
                .into_any_element(),
        };
        Some(
            v_flex()
                .id("chart-panel")
                .key_context("KantorChartPanel")
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
                    this.refocus(w, cx);
                    cx.notify();
                }))
                .child(header)
                .child(body)
                .into_any_element(),
        )
    }

    /// Apply a choice made with the window-less `choices` buttons.
    pub(crate) fn apply_pending_chart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(rows) = self.pending_chart_rows.take() {
            self.edit_chart(window, cx, |s| {
                s.by_rows = rows;
                Ok(())
            });
        }
    }

    pub(crate) fn chart_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &InsertChart, w, cx| {
            this.insert_chart_doc(ChartType::Column, w, cx)
        }))
        .on_action(cx.listener(|this, _: &ChartOptions, w, cx| this.chart_options_doc(w, cx)))
    }
}

/// Excel's standard colours, offered for series.
const SERIES_SWATCHES: [[u8; 3]; 10] = [
    [0xC0, 0x00, 0x00],
    [0xFF, 0x00, 0x00],
    [0xFF, 0xC0, 0x00],
    [0xFF, 0xFF, 0x00],
    [0x92, 0xD0, 0x50],
    [0x00, 0xB0, 0x50],
    [0x00, 0xB0, 0xF0],
    [0x00, 0x70, 0xC0],
    [0x00, 0x20, 0x60],
    [0x70, 0x30, 0xA0],
];

/// Each series' name and a row of swatches (automatic first).
fn series_colours(info: &ChartInfo, view: &WeakEntity<SheetsView>, cx: &App) -> Div {
    let t = tokens(cx);
    let mut list = v_flex()
        .gap(SPACE_2)
        .child(kantor_ui::components::section_label(
            i18n::t("Series colours"),
            cx,
        ));
    for (i, name) in info.series.iter().enumerate().take(24) {
        let cur = info.spec.colors.get(i).copied().unwrap_or_default();
        let mut sw = h_flex().gap(SPACE_1).flex_wrap();
        let options = std::iter::once(SeriesColor::Automatic)
            .chain(SERIES_SWATCHES.iter().map(|c| SeriesColor::Rgb(*c)));
        for (k, c) in options.enumerate() {
            let v = view.clone();
            let n = info.series.len();
            sw = sw.child(
                div()
                    .id(("chart-colour", i * 16 + k))
                    .size(px(16.))
                    .rounded(RADIUS_SM)
                    .border_2()
                    .border_color(if cur == c { t.accent } else { t.border })
                    .bg(match c {
                        SeriesColor::Rgb([r, g, b]) => crate::look::hsla(
                            (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b),
                        ),
                        _ => t.surface,
                    })
                    .when(c == SeriesColor::Automatic, |d| {
                        d.child(
                            div()
                                .text_size(FONT_CAPTION)
                                .text_color(t.fg_dim)
                                .flex()
                                .justify_center()
                                .child("A"),
                        )
                    })
                    .tooltip(move |w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(match c {
                            SeriesColor::Rgb(_) => i18n::t("Colour"),
                            _ => i18n::t("Automatic"),
                        })
                        .build(w, cx)
                    })
                    .cursor_pointer()
                    .on_click(move |_, w, cx| {
                        let _ = v.update(cx, |this, cx| {
                            this.edit_chart(w, cx, |s| {
                                if s.colors.len() < n {
                                    s.colors.resize(n, SeriesColor::Automatic);
                                }
                                s.colors[i] = c;
                                Ok(())
                            })
                        });
                    }),
            );
        }
        let label = if name.trim().is_empty() {
            tf("Series {}", &[&(i + 1)])
        } else {
            name.clone()
        };
        list = list.child(
            v_flex()
                .gap(SPACE_1)
                .child(
                    div()
                        .text_size(FONT_SMALL)
                        .text_color(t.fg_muted)
                        .truncate()
                        .child(label),
                )
                .child(sw),
        );
    }
    list
}

/// A chart type's item in the Insert Chart menu.
fn menu_label(k: ChartType) -> &'static str {
    match k {
        ChartType::Column => i18n::t("Column Chart"),
        ChartType::StackedColumn => i18n::t("Stacked Column Chart"),
        ChartType::Bar => i18n::t("Bar Chart"),
        ChartType::StackedBar => i18n::t("Stacked Bar Chart"),
        ChartType::Line => i18n::t("Line Chart"),
        ChartType::StackedLine => i18n::t("Stacked Line Chart"),
        ChartType::Pie => i18n::t("Pie Chart"),
        ChartType::Area => i18n::t("Area Chart"),
        ChartType::StackedArea => i18n::t("Stacked Area Chart"),
        ChartType::Scatter => i18n::t("Scatter Chart"),
        ChartType::Other => i18n::t("Other Chart"),
    }
}

/// The toolbar's Insert Chart menu.
pub(crate) fn chart_menu(mut menu: PopupMenu, view: &WeakEntity<SheetsView>) -> PopupMenu {
    for (i, k) in ChartType::ALL.into_iter().enumerate() {
        if i == 2 || i == 4 || i == 6 || i == 7 || i == 9 {
            menu = menu.separator();
        }
        let v = view.clone();
        menu = menu.item(PopupMenuItem::new(menu_label(k)).on_click(move |_, w, cx| {
            let _ = v.update(cx, |this, cx| this.insert_chart_doc(k, w, cx));
        }));
    }
    let v = view.clone();
    menu.separator().item(
        PopupMenuItem::new(i18n::t("Chart Panel")).on_click(move |_, w, cx| {
            let _ = v.update(cx, |this, cx| this.chart_options_doc(w, cx));
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kantor_ui::i18n::{indonesian, tk};

    /// Names from `sheets_engine` shown here through `t_dyn`: chart types
    /// (`ChartType::label`, a file chart's `kind_label`) and legend places.
    const ENGINE_LABELS: &[&str] = &[
        tk("Column"),
        tk("Stacked Column"),
        tk("Bar"),
        tk("Stacked Bar"),
        tk("Line"),
        tk("Stacked Line"),
        tk("Pie"),
        tk("Area"),
        tk("Stacked Area"),
        tk("Scatter"),
        tk("Other"),
        tk("Combo"),
        tk("100% Stacked Column"),
        tk("100% Stacked Bar"),
        tk("100% Stacked Line"),
        tk("100% Stacked Area"),
        tk("Doughnut"),
        tk("3-D Column"),
        tk("3-D Line"),
        tk("3-D Area"),
        tk("3-D Pie"),
        tk("Pie of Pie"),
        tk("Radar"),
        tk("Bubble"),
        tk("Stock"),
        tk("Surface"),
        tk("Right"),
        tk("Top"),
        tk("Bottom"),
        tk("Left"),
    ];

    #[std::prelude::v1::test]
    fn engine_labels_are_translated() {
        for k in ChartType::ALL {
            assert!(ENGINE_LABELS.contains(&k.label()), "{}", k.label());
        }
        for l in LegendPos::ALL {
            assert!(indonesian(l.label()).is_some(), "{}", l.label());
        }
        for s in ENGINE_LABELS {
            assert!(indonesian(s).is_some(), "{s}");
        }
    }
}
