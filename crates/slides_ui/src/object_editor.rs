//! Editing a table's cells or a chart's data in a panel over the canvas
//! (double-click the table or chart, the toolbar's edit button, or the
//! palette).
//!
//! - Table: a grid of the cells' text; add or remove rows and columns next
//!   to the cell last clicked; fill that cell with a theme colour or none.
//! - Chart: categories down, series across (names in the top row); add a
//!   category at the end or remove the last one; add a series after the
//!   last one or remove the series of the current cell.
//!
//! *Apply* writes the typed text as one undo step (`Presentation::
//! edit_table` / `set_chart_data`); structure buttons apply at once.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::tokens::Elevation;
use kantor_ui::{components::section_label, metrics::*, tokens};
use slides_engine::{ChartData, Rgb, TableOp};

use crate::SlidesView;
use crate::draw::hsla;

/// What the panel edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Table,
    Chart,
}

pub(crate) struct ObjectEditor {
    target: Target,
    slide: usize,
    shape: usize,
    /// Inputs by row, then column. For charts row 0 is the series names
    /// (column 0 unused) and column 0 the categories.
    cells: Vec<Vec<Entity<InputState>>>,
    /// The cell last focused (row, column).
    current: (usize, usize),
    error: Option<SharedString>,
    _subs: Vec<Subscription>,
}

/// Text in a value cell: numbers with a dot or a comma.
fn parse_num(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    t.parse().ok().or_else(|| t.replace(',', ".").parse().ok())
}

fn fmt_num(v: Option<f64>) -> String {
    match v {
        Some(v) if v.fract() == 0.0 && v.abs() < 1e15 => format!("{}", v as i64),
        Some(v) => format!("{v}"),
        None => String::new(),
    }
}

impl SlidesView {
    /// What the selected shape is, if it can be edited here.
    pub(crate) fn object_target(&self) -> Option<(usize, Target)> {
        let i = self.selected?;
        if self.pres.table(self.current, i).is_some() {
            return Some((i, Target::Table));
        }
        if self.pres.chart_data(self.current, i).is_some() {
            return Some((i, Target::Chart));
        }
        None
    }

    /// Open the editor for the selected table or chart.
    pub fn edit_object_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.object_target() {
            Some((shape, target)) => self.open_object_editor(shape, target, window, cx),
            None => {
                self.status = "Select a table or a chart to edit its data".into();
                cx.notify();
            }
        }
    }

    pub(crate) fn open_object_editor(
        &mut self,
        shape: usize,
        target: Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let slide = self.current;
        let grid: Vec<Vec<String>> = match target {
            Target::Table => match self.pres.table_text(slide, shape) {
                Some(t) => t,
                None => return,
            },
            Target::Chart => {
                let Some(d) = self.pres.chart_data(slide, shape) else {
                    return;
                };
                let mut g = vec![
                    std::iter::once(String::new())
                        .chain(d.series.iter().map(|s| s.name.clone()))
                        .collect::<Vec<_>>(),
                ];
                for (i, c) in d.categories.iter().enumerate() {
                    g.push(
                        std::iter::once(c.clone())
                            .chain(
                                d.series
                                    .iter()
                                    .map(|s| fmt_num(s.values.get(i).copied().flatten())),
                            )
                            .collect(),
                    );
                }
                g
            }
        };
        let mut subs = Vec::new();
        let cells: Vec<Vec<Entity<InputState>>> = grid
            .into_iter()
            .enumerate()
            .map(|(r, row)| {
                row.into_iter()
                    .enumerate()
                    .map(|(c, text)| {
                        let input = cx.new(|cx| {
                            let mut s = InputState::new(window, cx);
                            s.set_value(text, window, cx);
                            s
                        });
                        subs.push(cx.subscribe_in(
                            &input,
                            window,
                            move |this, _, ev: &InputEvent, window, cx| match ev {
                                InputEvent::Focus => {
                                    if let Some(e) = &mut this.object_editor {
                                        e.current = (r, c);
                                        cx.notify();
                                    }
                                }
                                InputEvent::PressEnter { .. } => {
                                    this.apply_object_editor(false, window, cx);
                                }
                                _ => {}
                            },
                        ));
                        input
                    })
                    .collect()
            })
            .collect();
        let current = self
            .object_editor
            .as_ref()
            .filter(|e| e.slide == slide && e.shape == shape)
            .map_or((usize::from(target == Target::Chart), 0), |e| e.current);
        self.object_editor = Some(ObjectEditor {
            target,
            slide,
            shape,
            cells,
            current,
            error: None,
            _subs: subs,
        });
        cx.notify();
    }

    fn close_object_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.object_editor = None;
        self.focus(window, cx);
        cx.notify();
    }

    fn grid_text(&self, cx: &App) -> Vec<Vec<String>> {
        self.object_editor
            .as_ref()
            .map(|e| {
                e.cells
                    .iter()
                    .map(|row| row.iter().map(|i| i.read(cx).value().to_string()).collect())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Write what was typed (one undo step); `keep_open` leaves the panel
    /// open (before a structure change).
    fn apply_object_editor(
        &mut self,
        keep_open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(e) = &self.object_editor else {
            return false;
        };
        let (slide, shape, target) = (e.slide, e.shape, e.target);
        let typed = self.grid_text(cx);
        let result = match target {
            Target::Table => {
                let before = self.pres.table_text(slide, shape).unwrap_or_default();
                let ops: Vec<TableOp> = typed
                    .iter()
                    .enumerate()
                    .flat_map(|(r, row)| {
                        let before = &before;
                        row.iter()
                            .enumerate()
                            .filter(move |(c, t)| before.get(r).and_then(|b| b.get(*c)) != Some(*t))
                            .map(move |(c, t)| TableOp::Text {
                                row: r,
                                col: c,
                                text: t.clone(),
                            })
                    })
                    .collect();
                self.pres.edit_table(slide, shape, &ops)
            }
            Target::Chart => {
                let Some(old) = self.pres.chart_data(slide, shape) else {
                    return false;
                };
                let mut d = ChartData {
                    categories: typed.iter().skip(1).map(|r| r[0].clone()).collect(),
                    series: old.series.clone(),
                };
                for (k, s) in d.series.iter_mut().enumerate() {
                    s.name = typed[0].get(k + 1).cloned().unwrap_or_default();
                    s.values = typed
                        .iter()
                        .skip(1)
                        .map(|r| r.get(k + 1).and_then(|t| parse_num(t)))
                        .collect();
                }
                if d == old {
                    Ok(())
                } else {
                    self.pres.set_chart_data(slide, shape, &d)
                }
            }
        };
        match result {
            Ok(()) => {
                self.after_object_edit(window, cx);
                if !keep_open {
                    self.close_object_editor(window, cx);
                }
                true
            }
            Err(err) => {
                if let Some(e) = &mut self.object_editor {
                    e.error = Some(format!("{err:#}").into());
                }
                cx.notify();
                false
            }
        }
    }

    fn after_object_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.after_edit(window, cx);
    }

    /// A table structure change or fill at the current cell.
    fn table_op(
        &mut self,
        make: fn((usize, usize)) -> TableOp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.apply_object_editor(true, window, cx) {
            return;
        }
        let Some(e) = &self.object_editor else { return };
        let (slide, shape, current) = (e.slide, e.shape, e.current);
        let op = make(current);
        match self.pres.edit_table(slide, shape, &[op]) {
            Ok(()) => {
                self.after_object_edit(window, cx);
                self.open_object_editor(shape, Target::Table, window, cx);
            }
            Err(err) => {
                if let Some(e) = &mut self.object_editor {
                    e.error = Some(format!("{err:#}").into());
                }
                cx.notify();
            }
        }
    }

    fn fill_cell(&mut self, color: Option<Rgb>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.apply_object_editor(true, window, cx) {
            return;
        }
        let Some(e) = &self.object_editor else { return };
        let (slide, shape, (row, col)) = (e.slide, e.shape, e.current);
        if self
            .pres
            .edit_table(slide, shape, &[TableOp::Fill { row, col, color }])
            .is_ok()
        {
            self.after_object_edit(window, cx);
        }
        cx.notify();
    }

    /// Add a category at the end (or remove the last one).
    fn chart_rows(&mut self, add: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.apply_object_editor(true, window, cx) {
            return;
        }
        let Some(e) = &self.object_editor else { return };
        let (slide, shape) = (e.slide, e.shape);
        let Some(mut d) = self.pres.chart_data(slide, shape) else {
            return;
        };
        if add {
            d.categories
                .push(format!("Category {}", d.categories.len() + 1));
            for s in &mut d.series {
                s.values.push(None);
            }
        } else {
            if d.categories.len() <= 1 {
                return;
            }
            d.categories.pop();
            for s in &mut d.series {
                s.values.pop();
            }
        }
        match self.pres.set_chart_data(slide, shape, &d) {
            Ok(()) => {
                self.after_object_edit(window, cx);
                self.open_object_editor(shape, Target::Chart, window, cx);
            }
            Err(err) => {
                if let Some(e) = &mut self.object_editor {
                    e.error = Some(format!("{err:#}").into());
                }
                cx.notify();
            }
        }
    }

    /// Add a series after the last one, or remove the series of the
    /// current cell (the last one when the cell is a category label).
    fn chart_series_op(&mut self, add: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.apply_object_editor(true, window, cx) {
            return;
        }
        let Some(e) = &self.object_editor else { return };
        let (slide, shape, (_, col)) = (e.slide, e.shape, e.current);
        let Some(d) = self.pres.chart_data(slide, shape) else {
            return;
        };
        let res = if add {
            let name = format!("Series {}", d.series.len() + 1);
            self.pres.add_chart_series(slide, shape, &name)
        } else {
            let k = if col >= 1 && col <= d.series.len() {
                col - 1
            } else {
                d.series.len().saturating_sub(1)
            };
            self.pres.remove_chart_series(slide, shape, k)
        };
        match res {
            Ok(()) => {
                self.after_object_edit(window, cx);
                self.open_object_editor(shape, Target::Chart, window, cx);
                if let Some(e) = &mut self.object_editor {
                    let cols = e.cells.first().map_or(1, Vec::len);
                    e.current = (0, if add { cols - 1 } else { col.min(cols - 1) });
                }
            }
            Err(err) => {
                if let Some(e) = &mut self.object_editor {
                    e.error = Some(format!("{err:#}").into());
                }
                cx.notify();
            }
        }
    }

    pub(crate) fn render_object_editor(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let e = self.object_editor.as_ref()?;
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let tool =
            |id: &'static str,
             icon: Option<Lucide>,
             label: &'static str,
             tip: &'static str,
             run: fn(&mut SlidesView, &mut Window, &mut Context<SlidesView>)| {
                let v = view.clone();
                let mut b = Button::new(id).ghost().xsmall().tooltip(tip);
                b = match icon {
                    Some(i) => b.icon(i),
                    None => b.label(label),
                };
                b.on_click(move |_, window, cx| {
                    let _ = v.update(cx, |this, cx| run(this, window, cx));
                })
            };
        let mut bar = h_flex().gap(SPACE_1).flex_wrap();
        match e.target {
            Target::Table => {
                bar = bar
                    .child(tool(
                        "tbl-row-above",
                        None,
                        "+ Row above",
                        "Insert a row above the current cell",
                        |v, w, cx| {
                            v.table_op(
                                |(r, _)| TableOp::InsertRow {
                                    row: r,
                                    below: false,
                                },
                                w,
                                cx,
                            )
                        },
                    ))
                    .child(tool(
                        "tbl-row-below",
                        None,
                        "+ Row below",
                        "Insert a row below the current cell",
                        |v, w, cx| {
                            v.table_op(
                                |(r, _)| TableOp::InsertRow {
                                    row: r,
                                    below: true,
                                },
                                w,
                                cx,
                            )
                        },
                    ))
                    .child(tool(
                        "tbl-row-del",
                        None,
                        "− Row",
                        "Delete the current row",
                        |v, w, cx| v.table_op(|(r, _)| TableOp::DeleteRow(r), w, cx),
                    ))
                    .child(tool(
                        "tbl-col-left",
                        None,
                        "+ Column left",
                        "Insert a column left of the current cell",
                        |v, w, cx| {
                            v.table_op(
                                |(_, c)| TableOp::InsertCol {
                                    col: c,
                                    right: false,
                                },
                                w,
                                cx,
                            )
                        },
                    ))
                    .child(tool(
                        "tbl-col-right",
                        None,
                        "+ Column right",
                        "Insert a column right of the current cell",
                        |v, w, cx| {
                            v.table_op(
                                |(_, c)| TableOp::InsertCol {
                                    col: c,
                                    right: true,
                                },
                                w,
                                cx,
                            )
                        },
                    ))
                    .child(tool(
                        "tbl-col-del",
                        None,
                        "− Column",
                        "Delete the current column",
                        |v, w, cx| v.table_op(|(_, c)| TableOp::DeleteCol(c), w, cx),
                    ));
            }
            Target::Chart => {
                bar = bar
                    .child(tool(
                        "chart-row-add",
                        None,
                        "+ Category",
                        "Add a category at the end",
                        |v, w, cx| v.chart_rows(true, w, cx),
                    ))
                    .child(tool(
                        "chart-row-del",
                        None,
                        "− Last category",
                        "Remove the last category",
                        |v, w, cx| v.chart_rows(false, w, cx),
                    ))
                    .child(tool(
                        "chart-ser-add",
                        None,
                        "+ Series",
                        "Add a series after the last one",
                        |v, w, cx| v.chart_series_op(true, w, cx),
                    ))
                    .child(tool(
                        "chart-ser-del",
                        None,
                        "− Series",
                        "Remove the series of the current cell",
                        |v, w, cx| v.chart_series_op(false, w, cx),
                    ));
            }
        }
        // Cell fill swatches (tables): the slide's theme colours and none.
        let swatches = (e.target == Target::Table).then(|| {
            let mut row = h_flex()
                .gap(SPACE_1)
                .items_center()
                .child(div().text_color(t.fg_muted).mr(SPACE_2).child("Cell fill"));
            let none = {
                let v = view.clone();
                div()
                    .id("fill-none")
                    .size(px(18.))
                    .rounded(RADIUS_SM)
                    .border_1()
                    .border_color(t.border_strong)
                    .cursor_pointer()
                    .child(
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(t.fg_muted)
                            .text_size(FONT_SMALL)
                            .child("∅"),
                    )
                    .on_click(move |_, window, cx| {
                        let _ = v.update(cx, |this, cx| this.fill_cell(None, window, cx));
                    })
            };
            row = row.child(none);
            for (slot, rgb) in self.pres.theme_colors(self.current) {
                if !(slot.starts_with("accent") || slot == "bg2" || slot == "tx2") {
                    continue;
                }
                let v = view.clone();
                row = row.child(
                    div()
                        .id(SharedString::from(format!("fill-{slot}")))
                        .size(px(18.))
                        .rounded(RADIUS_SM)
                        .border_1()
                        .border_color(t.border)
                        .bg(hsla(rgb))
                        .cursor_pointer()
                        .on_click(move |_, window, cx| {
                            let _ = v.update(cx, |this, cx| this.fill_cell(Some(rgb), window, cx));
                        }),
                );
            }
            row
        });
        let cols = e.cells.iter().map(Vec::len).max().unwrap_or(0);
        let cell_w = px(if cols > 6 { 88. } else { 110. });
        let mut grid = v_flex().gap(SPACE_1);
        for (r, row) in e.cells.iter().enumerate() {
            let mut line = h_flex().gap(SPACE_1);
            for (c, input) in row.iter().enumerate() {
                let current = e.current == (r, c);
                let header = e.target == Target::Chart && (r == 0 || c == 0);
                let corner = e.target == Target::Chart && r == 0 && c == 0;
                let cell = div()
                    .w(cell_w)
                    .flex_none()
                    .rounded(RADIUS_SM)
                    .when(current, |d| d.border_1().border_color(t.accent))
                    .when(header, |d| d.bg(t.surface));
                line = line.child(if corner {
                    cell.child(div().px(SPACE_2).text_color(t.fg_dim).child("Category"))
                } else {
                    cell.child(Input::new(input).small())
                });
            }
            grid = grid.child(line);
        }
        let title = match e.target {
            Target::Table => "Edit table",
            Target::Chart => "Edit chart data",
        };
        let (va, vc) = (view.clone(), view.clone());
        let footer = h_flex()
            .gap(SPACE_2)
            .children(
                e.error
                    .clone()
                    .map(|m| div().flex_1().text_color(t.danger).child(m)),
            )
            .child(div().flex_1())
            .child(
                Button::new("object-cancel")
                    .ghost()
                    .xsmall()
                    .label("Close")
                    .on_click(move |_, window, cx| {
                        let _ = vc.update(cx, |this, cx| this.close_object_editor(window, cx));
                    }),
            )
            .child(
                Button::new("object-apply")
                    .primary()
                    .xsmall()
                    .label("Apply")
                    .tooltip("Enter")
                    .on_click(move |_, window, cx| {
                        let _ = va.update(cx, |this, cx| {
                            this.apply_object_editor(false, window, cx);
                        });
                    }),
            );
        let b = self.viewport.get();
        let panel = v_flex()
            .id("slides-object-editor")
            .occlude()
            .max_w(b.size.width - SPACE_7)
            .max_h(b.size.height - SPACE_7)
            .gap(SPACE_3)
            .p(SPACE_4)
            .bg(t.elevated)
            .border_1()
            .border_color(t.border)
            .rounded(RADIUS_LG)
            .shadow(t.shadow(Elevation::Popover))
            .text_size(FONT_SMALL)
            .text_color(t.fg)
            .child(section_label(title, cx))
            .child(bar)
            .children(swatches)
            .child(
                div()
                    .id("object-grid")
                    .overflow_scroll()
                    .max_h(px(360.))
                    .child(grid),
            )
            .child(footer);
        Some(deferred(
            anchored()
                .position(point(
                    b.origin.x + b.size.width - SPACE_4,
                    b.origin.y + SPACE_4,
                ))
                .anchor(Anchor::TopRight)
                .snap_to_window_with_margin(SPACE_4)
                .child(panel),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn numbers_in_cells() {
        assert_eq!(parse_num("12,5"), Some(12.5));
        assert_eq!(parse_num(" 3 "), Some(3.0));
        assert_eq!(parse_num(""), None);
        assert_eq!(parse_num("abc"), None);
        assert_eq!(fmt_num(Some(14.0)), "14");
        assert_eq!(fmt_num(Some(14.25)), "14.25");
    }
}
