//! Insert Chart and the Chart panel.
//!
//! * Insert Chart (toolbar, palette): a chart of the picked type from the
//!   selection (else the data around the active cell), placed beside the
//!   data and selected; moving, resizing and deleting work as for a file's
//!   charts ([`crate::drawings`]).
//! * The Chart panel beside the grid, for the selected chart made in Kantor:
//!   type, title, legend position, series from rows or columns, data range,
//!   delete. Each change is one undo step (`sheets_engine::chart_edit`).
//!   Charts from the file can be moved, resized and deleted, not restyled.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{self, Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::{metrics::*, tokens};
use sheets_engine::drawing::DrawingKey;
use sheets_engine::{ChartSpec, ChartType, LegendPos, NEW_CHART_BASE};

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
                self.status = format!("{} chart inserted", kind.label()).into();
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
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("No title"));
        let range = cx.new(|cx| InputState::new(window, cx).placeholder("e.g. A1:C13"));
        let subs = vec![
            cx.subscribe_in(&title, window, |this, _, e: &InputEvent, w, cx| {
                if let InputEvent::PressEnter { .. } | InputEvent::Blur = e {
                    this.apply_chart_inputs(w, cx);
                }
            }),
            cx.subscribe_in(&range, window, |this, _, e: &InputEvent, w, cx| {
                if let InputEvent::PressEnter { .. } = e {
                    this.apply_chart_inputs(w, cx);
                }
            }),
        ];
        self.chart_panel = Some(ChartPanel {
            loaded: None,
            title,
            range,
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

    /// The selected chart made in Kantor and its spec.
    fn selected_chart(&self, cx: &App) -> Option<(DrawingKey, ChartSpec)> {
        // The selected chart, else the one the panel shows (clicking in the
        // panel deselects the chart on the grid).
        let key = match self.drawings.borrow().selected {
            Some(k) if k.index >= NEW_CHART_BASE => k,
            Some(_) => return None,
            None => self.chart_panel.as_ref()?.loaded?,
        };
        let spec = self.table.read(cx).delegate().book.chart_spec(key)?;
        Some((key, spec))
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
        let (title, range) = (p.title.clone(), p.range.clone());
        if let Some((_, spec)) = sel {
            title.update(cx, |i, cx| i.set_value(spec.title.clone(), window, cx));
            range.update(cx, |i, cx| i.set_value(spec.data.name(), window, cx));
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
        let Some((key, mut spec)) = self.selected_chart(cx) else {
            return;
        };
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
        self.edit_chart(window, cx, |s| {
            s.title = title;
            s.data = dialog::parse_cells(&range)
                .filter(|r| !r.is_single())
                .ok_or("Data: type the chart's cells, such as A1:C13")?;
            Ok(())
        });
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
                    .child("Chart"),
            )
            .child(
                Button::new("chart-panel-close")
                    .ghost()
                    .xsmall()
                    .icon(Lucide::X)
                    .tooltip("Close")
                    .on_click(move |_, _, cx| {
                        let _ = close.update(cx, |this, cx| {
                            this.chart_panel = None;
                            cx.notify();
                        });
                    }),
            );
        let file_chart = self
            .drawings
            .borrow()
            .selected
            .is_some_and(|k| k.index < NEW_CHART_BASE);
        let body: AnyElement = match self.selected_chart(cx) {
            Some((_, spec)) => {
                let tv = view.clone();
                let current = spec.kind;
                let kind = Button::new("chart-kind")
                    .outline()
                    .small()
                    .label(current.label())
                    .dropdown_caret(true)
                    .dropdown_menu(move |mut m, _, _| {
                        for k in ChartType::ALL {
                            let v = tv.clone();
                            m = m.item(PopupMenuItem::new(k.label()).checked(k == current).on_click(
                                move |_, w, cx| {
                                    let _ = v.update(cx, |this, cx| {
                                        this.edit_chart(w, cx, |s| {
                                            s.kind = k;
                                            Ok(())
                                        })
                                    });
                                },
                            ));
                        }
                        m
                    });
                let lv = view.clone();
                let legend = spec.legend;
                let legend_button = Button::new("chart-legend")
                    .outline()
                    .small()
                    .label(legend.label())
                    .dropdown_caret(true)
                    .dropdown_menu(move |mut m, _, _| {
                        for l in LegendPos::ALL {
                            let v = lv.clone();
                            m = m.item(PopupMenuItem::new(l.label()).checked(l == legend).on_click(
                                move |_, w, cx| {
                                    let _ = v.update(cx, |this, cx| {
                                        this.edit_chart(w, cx, |s| {
                                            s.legend = l;
                                            Ok(())
                                        })
                                    });
                                },
                            ));
                        }
                        m
                    });
                let dv = view.clone();
                let av = view.clone();
                v_flex()
                    .gap(SPACE_4)
                    .child(row("Type", kind, cx))
                    .child(row("Title", Input::new(&p.title).small(), cx))
                    .child(row("Legend", legend_button, cx))
                    .child(row(
                        "Series in",
                        choices(
                            "chart-series",
                            &[(false, "Columns"), (true, "Rows")],
                            spec.by_rows,
                            &view,
                            |this, v| this.pending_chart_rows = Some(v),
                        ),
                        cx,
                    ))
                    .child(row("Data", Input::new(&p.range).small(), cx))
                    .child(note("Press Enter in a box to apply it.", false, cx))
                    .children(p.error.clone().map(|e| note(e, true, cx)))
                    .child(
                        h_flex()
                            .gap(SPACE_3)
                            .child(
                                Button::new("chart-apply")
                                    .small()
                                    .outline()
                                    .label("Apply")
                                    .on_click(move |_, w, cx| {
                                        let _ = av.update(cx, |this, cx| this.apply_chart_inputs(w, cx));
                                    }),
                            )
                            .child(
                                Button::new("chart-delete")
                                    .small()
                                    .outline()
                                    .icon(Lucide::Trash)
                                    .label("Delete Chart")
                                    .on_click(move |_, w, cx| {
                                        let _ = dv.update(cx, |this, cx| {
                                            this.drawing_key("delete", w, cx);
                                        });
                                    }),
                            ),
                    )
                    .into_any_element()
            }
            None => div()
                .text_size(FONT_SMALL)
                .text_color(t.fg_muted)
                .child(if file_chart {
                    "This chart came with the file: it can be moved, resized and deleted, but Kantor can't change its type or data yet."
                } else {
                    "Select a chart made with Insert Chart to change it."
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
                    this.focus_grid(w, cx);
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

/// The toolbar's Insert Chart menu.
pub(crate) fn chart_menu(mut menu: PopupMenu, view: &WeakEntity<SheetsView>) -> PopupMenu {
    for (i, k) in ChartType::ALL.into_iter().enumerate() {
        if i == 2 || i == 4 || i == 6 || i == 7 || i == 9 {
            menu = menu.separator();
        }
        let v = view.clone();
        menu = menu.item(PopupMenuItem::new(format!("{} Chart", k.label())).on_click(
            move |_, w, cx| {
                let _ = v.update(cx, |this, cx| this.insert_chart_doc(k, w, cx));
            },
        ));
    }
    let v = view.clone();
    menu.separator()
        .item(PopupMenuItem::new("Chart Panel").on_click(move |_, w, cx| {
            let _ = v.update(cx, |this, cx| this.chart_options_doc(w, cx));
        }))
}
