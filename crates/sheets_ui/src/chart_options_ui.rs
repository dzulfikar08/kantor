//! The Chart panel's Data Labels and Axis sections (Excel's Format Data
//! Labels and Format Axis, the common options): what labels show, where
//! they sit and their number format, for all series or one; an axis'
//! bounds, major unit, logarithmic scale, reverse order, number format and
//! display units. Each change is one undo step through `set_chart`.

use gpui_kit::component::{
    Sizable as _,
    button::Button,
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, t_dyn, tf};
use kantor_ui::metrics::*;
use sheets_engine::{AxisOptions, ChartInfo, ChartSpec, DataLabels, DisplayUnits};

use crate::SheetsView;
use crate::dialog::{self, row};

/// The labels the panel shows for `target` (all series: the first one's).
fn labels_of(spec: &ChartSpec, target: Option<usize>) -> DataLabels {
    spec.labels
        .get(target.unwrap_or(0))
        .cloned()
        .unwrap_or_default()
}

/// Set the labels of `target` (all series when `None`) to `l`.
fn put_labels(spec: &mut ChartSpec, n: usize, target: Option<usize>, l: &DataLabels) {
    if spec.labels.len() < n {
        spec.labels.resize(n, DataLabels::default());
    }
    match target {
        Some(i) if i < n => spec.labels[i] = l.clone(),
        _ => spec.labels.iter_mut().for_each(|x| *x = l.clone()),
    }
}

fn axis_of(spec: &mut ChartSpec, horizontal: bool) -> &mut AxisOptions {
    if horizontal {
        &mut spec.x_axis
    } else {
        &mut spec.y_axis
    }
}

fn num_text(v: Option<f64>) -> String {
    v.map(|v| format!("{v}")).unwrap_or_default()
}

/// What the option boxes hold, applied to a spec.
type ApplyOptions = Box<dyn FnOnce(&mut ChartSpec)>;

impl SheetsView {
    /// Fill the option boxes for the panel's series and axis.
    pub(crate) fn fill_option_inputs(
        &mut self,
        info: &ChartInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = self.chart_panel.as_ref() else {
            return;
        };
        let l = labels_of(&info.spec, p.label_target);
        let a = if p.horizontal_axis {
            &info.spec.x_axis
        } else {
            &info.spec.y_axis
        };
        let set = |input: &Entity<InputState>, v: String, window: &mut Window, cx: &mut App| {
            input.update(cx, |i, cx| i.set_value(v, window, cx));
        };
        set(&p.label_fmt, l.num_fmt.unwrap_or_default(), window, cx);
        set(&p.ax_min, num_text(a.min), window, cx);
        set(&p.ax_max, num_text(a.max), window, cx);
        set(&p.ax_major, num_text(a.major), window, cx);
        set(&p.ax_fmt, a.num_fmt.clone().unwrap_or_default(), window, cx);
    }

    /// The option boxes as a change to a spec (or what's wrong in them).
    pub(crate) fn read_option_inputs(
        &self,
        info: &ChartInfo,
        cx: &App,
    ) -> Result<ApplyOptions, String> {
        let Some(p) = self.chart_panel.as_ref() else {
            return Ok(Box::new(|_| {}));
        };
        let text = |e: &Entity<InputState>| e.read(cx).value().trim().to_owned();
        let number = |e: &Entity<InputState>, what: &str| -> Result<Option<f64>, String> {
            let t = text(e);
            if t.is_empty() || t.eq_ignore_ascii_case("auto") || t == i18n::t("Auto") {
                return Ok(None);
            }
            dialog::parse_number(&t).map(Some).ok_or_else(|| {
                tf(
                    "{}: type a number, or leave it empty for Auto",
                    &[&t_dyn(what)],
                )
            })
        };
        // The targets the boxes were filled for (a target picked since
        // hasn't refilled them yet).
        let (target, horizontal) = p.loaded_targets;
        let value_axis = if horizontal {
            info.x_value
        } else {
            info.y_value
        };
        let mut axis = if horizontal {
            info.spec.x_axis.clone()
        } else {
            info.spec.y_axis.clone()
        };
        if value_axis {
            axis.min = number(&p.ax_min, "Minimum")?;
            axis.max = number(&p.ax_max, "Maximum")?;
            axis.major = number(&p.ax_major, "Major unit")?;
            if let (Some(a), Some(b)) = (axis.min, axis.max)
                && a >= b
            {
                return Err(i18n::t("The minimum must be less than the maximum.").into());
            }
            if axis.major.is_some_and(|u| u <= 0.0) {
                return Err(i18n::t("The major unit must be more than 0.").into());
            }
            if axis.log_base.is_some() && axis.min.is_some_and(|m| m <= 0.0) {
                return Err(i18n::t(
                    "Negative or zero values can't be plotted on a logarithmic scale.",
                )
                .into());
            }
        }
        let fmt = text(&p.ax_fmt);
        axis.num_fmt = (!fmt.is_empty()).then_some(fmt);
        let label_fmt = text(&p.label_fmt);
        let n = info.series.len();
        let mut labels = labels_of(&info.spec, target);
        let fmt_changed = labels.num_fmt.as_deref().unwrap_or("") != label_fmt;
        labels.num_fmt = (!label_fmt.is_empty()).then_some(label_fmt);
        let has_axes = info.has_axes;
        Ok(Box::new(move |s| {
            if has_axes {
                *axis_of(s, horizontal) = axis;
            }
            if fmt_changed {
                put_labels(s, n, target, &labels);
            }
        }))
    }

    /// Change the labels of the panel's series.
    fn edit_labels(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut DataLabels) + 'static,
    ) {
        let Some(p) = self.chart_panel.as_ref() else {
            return;
        };
        let target = p.label_target;
        let n = self.selected_chart_series(cx);
        self.edit_chart(window, cx, move |s| {
            let mut l = labels_of(s, target);
            f(&mut l);
            put_labels(s, n, target, &l);
            Ok(())
        });
    }

    fn selected_chart_series(&self, cx: &App) -> usize {
        let Some(key) = self
            .drawings
            .borrow()
            .selected
            .or_else(|| self.chart_panel.as_ref().and_then(|p| p.loaded_key()))
        else {
            return 0;
        };
        self.table
            .read(cx)
            .delegate()
            .book
            .chart_info(key)
            .map_or(0, |i| i.series.len())
    }

    /// Change the options of the panel's axis.
    fn edit_axis(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut AxisOptions) + 'static,
    ) {
        let Some(horizontal) = self.chart_panel.as_ref().map(|p| p.horizontal_axis) else {
            return;
        };
        self.edit_chart(window, cx, move |s| {
            f(axis_of(s, horizontal));
            Ok(())
        });
    }

    /// The Data Labels section.
    pub(crate) fn render_label_options(
        &self,
        info: &ChartInfo,
        view: &WeakEntity<SheetsView>,
        cx: &App,
    ) -> Div {
        let Some(p) = self.chart_panel.as_ref() else {
            return div();
        };
        let target = p.label_target;
        let l = labels_of(&info.spec, target);
        let n = info.series.len();
        let name = |i: usize| {
            info.series
                .get(i)
                .filter(|s| !s.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| tf("Series {}", &[&(i + 1)]))
        };
        let names: Vec<String> = (0..n).map(name).collect();
        let tv = view.clone();
        let target_label = match target {
            Some(i) => name(i),
            None => i18n::t("All Series").to_owned(),
        };
        let series_button = Button::new("chart-label-series")
            .outline()
            .small()
            .label(target_label)
            .dropdown_caret(true)
            .dropdown_menu(move |mut m, _, _| {
                let opts = std::iter::once((None, i18n::t("All Series").to_owned()))
                    .chain(names.iter().cloned().enumerate().map(|(i, s)| (Some(i), s)));
                for (t, label) in opts {
                    let v = tv.clone();
                    m = m.item(PopupMenuItem::new(label).checked(t == target).on_click(
                        move |_, _, cx| {
                            let _ = v.update(cx, |this, cx| {
                                if let Some(p) = this.chart_panel.as_mut() {
                                    p.label_target = t;
                                }
                                cx.notify();
                            });
                        },
                    ));
                }
                m
            });
        let check =
            |id: &'static str, label: &'static str, on: bool, set: fn(&mut DataLabels, bool)| {
                let v = view.clone();
                Checkbox::new(id)
                    .label(label)
                    .checked(on)
                    .on_click(move |on, w, cx| {
                        let on = *on;
                        let _ =
                            v.update(cx, |this, cx| this.edit_labels(w, cx, move |l| set(l, on)));
                    })
            };
        let pos_view = view.clone();
        let current = l.pos;
        let positions = info.label_positions.clone();
        let pos_button = Button::new("chart-label-pos")
            .outline()
            .small()
            .label(current.map_or(i18n::t("Default"), |p| t_dyn(p.label())))
            .dropdown_caret(true)
            .dropdown_menu(move |mut m, _, _| {
                let opts = std::iter::once(None).chain(positions.iter().copied().map(Some));
                for pos in opts {
                    let v = pos_view.clone();
                    let label = pos.map_or(i18n::t("Default"), |p| t_dyn(p.label()));
                    m = m.item(PopupMenuItem::new(label).checked(pos == current).on_click(
                        move |_, w, cx| {
                            let _ = v.update(cx, |this, cx| {
                                this.edit_labels(w, cx, move |l| l.pos = pos)
                            });
                        },
                    ));
                }
                m
            });
        v_flex()
            .gap(SPACE_2)
            .child(kantor_ui::components::section_label(
                i18n::t("Data Labels"),
                cx,
            ))
            .when(n > 1, |d| {
                d.child(row(i18n::t("Series"), series_button, cx))
            })
            .child(
                h_flex()
                    .gap(SPACE_4)
                    .flex_wrap()
                    .child(check(
                        "chart-label-val",
                        i18n::t("Value"),
                        l.value,
                        |l, on| l.value = on,
                    ))
                    .child(check(
                        "chart-label-cat",
                        i18n::t("Category Name"),
                        l.category,
                        |l, on| l.category = on,
                    ))
                    .child(check(
                        "chart-label-ser",
                        i18n::t("Series Name"),
                        l.series,
                        |l, on| l.series = on,
                    ))
                    .when(info.percent_labels, |d| {
                        d.child(check(
                            "chart-label-pct",
                            i18n::t("Percentage"),
                            l.percent,
                            |l, on| l.percent = on,
                        ))
                    }),
            )
            .when(!info.label_positions.is_empty(), |d| {
                d.child(row(i18n::t("Label Position"), pos_button, cx))
            })
            .child(row(
                i18n::t("Number format"),
                Input::new(&p.label_fmt).small(),
                cx,
            ))
    }

    /// The Axis section.
    pub(crate) fn render_axis_options(
        &self,
        info: &ChartInfo,
        view: &WeakEntity<SheetsView>,
        cx: &App,
    ) -> Div {
        let Some(p) = self.chart_panel.as_ref() else {
            return div();
        };
        let horizontal = p.horizontal_axis;
        let value = if horizontal {
            info.x_value
        } else {
            info.y_value
        };
        let a = if horizontal {
            &info.spec.x_axis
        } else {
            &info.spec.y_axis
        };
        let av = view.clone();
        let axis_button = Button::new("chart-axis-which")
            .outline()
            .small()
            .label(if horizontal {
                i18n::t("Horizontal axis")
            } else {
                i18n::t("Vertical axis")
            })
            .dropdown_caret(true)
            .dropdown_menu(move |mut m, _, _| {
                for (h, label) in [
                    (false, i18n::t("Vertical axis")),
                    (true, i18n::t("Horizontal axis")),
                ] {
                    let v = av.clone();
                    m = m.item(PopupMenuItem::new(label).checked(h == horizontal).on_click(
                        move |_, _, cx| {
                            let _ = v.update(cx, |this, cx| {
                                if let Some(p) = this.chart_panel.as_mut() {
                                    p.horizontal_axis = h;
                                }
                                cx.notify();
                            });
                        },
                    ));
                }
                m
            });
        let rv = view.clone();
        let reverse = Checkbox::new("chart-axis-reverse")
            .label(if value {
                i18n::t("Values in reverse order")
            } else {
                i18n::t("Categories in reverse order")
            })
            .checked(a.reverse)
            .on_click(move |on, w, cx| {
                let on = *on;
                let _ = rv.update(cx, |this, cx| {
                    this.edit_axis(w, cx, move |a| a.reverse = on)
                });
            });
        let lv = view.clone();
        let log = Checkbox::new("chart-axis-log")
            .label(i18n::t("Logarithmic scale"))
            .checked(a.log_base.is_some())
            .on_click(move |on, w, cx| {
                let on = *on;
                let _ = lv.update(cx, |this, cx| {
                    this.edit_axis(w, cx, move |a| {
                        a.log_base = on.then_some(10.0);
                        // Excel drops bounds that can't be on a log scale.
                        if on {
                            a.min = a.min.filter(|m| *m > 0.0);
                            a.max = a.max.filter(|m| *m > 0.0);
                            a.major = None;
                        }
                    })
                });
            });
        let uv = view.clone();
        let units = a.units;
        let units_button = Button::new("chart-axis-units")
            .outline()
            .small()
            .label(t_dyn(units.label()))
            .dropdown_caret(true)
            .dropdown_menu(move |mut m, _, _| {
                for u in DisplayUnits::ALL {
                    let v = uv.clone();
                    m = m.item(
                        PopupMenuItem::new(t_dyn(u.label()))
                            .checked(u == units)
                            .on_click(move |_, w, cx| {
                                let _ = v.update(cx, |this, cx| {
                                    this.edit_axis(w, cx, move |a| a.units = u)
                                });
                            }),
                    );
                }
                m
            });
        v_flex()
            .gap(SPACE_2)
            .child(kantor_ui::components::section_label(
                i18n::t("Axis Options"),
                cx,
            ))
            .child(row(i18n::t("Axis"), axis_button, cx))
            .when(value, |d| {
                d.child(row(i18n::t("Minimum"), Input::new(&p.ax_min).small(), cx))
                    .child(row(i18n::t("Maximum"), Input::new(&p.ax_max).small(), cx))
                    .child(row(
                        i18n::t("Major unit"),
                        Input::new(&p.ax_major).small(),
                        cx,
                    ))
                    .child(row(i18n::t("Display units"), units_button, cx))
                    .child(log)
            })
            .child(reverse)
            .child(row(
                i18n::t("Number format"),
                Input::new(&p.ax_fmt).small(),
                cx,
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kantor_ui::i18n::{indonesian, tk};
    use sheets_engine::LabelPos;

    /// Engine names shown through `t_dyn` here.
    const ENGINE_LABELS: &[&str] = &[
        tk("Center"),
        tk("Inside End"),
        tk("Inside Base"),
        tk("Outside End"),
        tk("Left"),
        tk("Right"),
        tk("Above"),
        tk("Below"),
        tk("Best Fit"),
        tk("None"),
        tk("Hundreds"),
        tk("Thousands"),
        tk("10000"),
        tk("100000"),
        tk("Millions"),
        tk("10000000"),
        tk("100000000"),
        tk("Billions"),
        tk("Trillions"),
        tk("Custom"),
        tk("Minimum"),
        tk("Maximum"),
        tk("Major unit"),
    ];

    #[std::prelude::v1::test]
    fn engine_labels_are_translated() {
        for p in LabelPos::ALL {
            assert!(ENGINE_LABELS.contains(&p.label()), "{}", p.label());
        }
        for u in DisplayUnits::ALL {
            assert!(ENGINE_LABELS.contains(&u.label()), "{}", u.label());
        }
        for s in ENGINE_LABELS {
            assert!(indonesian(s).is_some(), "{s}");
        }
    }
}
