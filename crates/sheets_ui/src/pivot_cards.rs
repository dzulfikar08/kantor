//! The PivotTable Fields panel's settings cards (see [`crate::pivot`]):
//!
//! * **Field Settings** of a row or column field: subtotals (Automatic,
//!   None, Custom with the functions picked), layout (form, repeat item
//!   labels, a blank line after each item), sorting (A to Z, Z to A, by a
//!   value field) and one label, value (Top 10 too) or date filter.
//! * **Grouping**: numbers (Starting at / Ending at / By) or dates (the
//!   levels, the number of days), as Excel's Grouping dialog; **Group
//!   Selection** reuses the item picker; **Ungroup** is a menu item.
//! * **Calculated Field**: a name and a formula over the fields' names.
//! * **Calculated Item** (a row or column field's menu): a name and a
//!   formula over the field's items; its existing calculated items can be
//!   picked to change or delete.
//! * **PivotTable Styles**: the gallery of the 84 built-in styles drawn by
//!   the engine, with the style options.
//!
//! Every Apply is one undo step in the engine.

use gpui_kit::component::{
    Disableable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, t_dyn, tf, tk};
use kantor_ui::{metrics::*, tokens};
use sheets_engine::{
    DateLevel, FieldSubtotals, FilterFamily, PIVOT_FILTER_TYPES, PivotFieldFilter, PivotFunc,
    PivotGrouping, PivotLayout, PivotSort, PivotStyleOptions, ReportLayout, filter_arity,
    pivot_style_names,
};

use crate::SheetsView;

/// Field Settings of field `field`, being edited.
pub(crate) struct FieldCard {
    pub field: usize,
    pub settings: sheets_engine::PivotFieldSettings,
    /// The filter's family while one is being chosen (`None`: no filter).
    pub family: Option<FilterFamily>,
    pub v1: Entity<InputState>,
    pub v2: Entity<InputState>,
    /// Which of the field's two filters is being edited (with "Allow
    /// multiple filters per field"): the editor shows `settings.filter`,
    /// and while the second is edited the two are swapped (`other` holds
    /// the other's typed values).
    pub second: bool,
    pub other: (String, String),
    _subs: Vec<Subscription>,
}

/// The Grouping card of field `field`.
pub(crate) struct GroupCard {
    pub field: usize,
    pub dates: bool,
    pub levels: Vec<DateLevel>,
    pub auto_start: bool,
    pub auto_end: bool,
    pub start: Entity<InputState>,
    pub end: Entity<InputState>,
    pub by: Entity<InputState>,
    pub days: Entity<InputState>,
    pub error: Option<String>,
    _subs: Vec<Subscription>,
}

/// The Insert Calculated Field card, or Insert Calculated Item of field
/// `item_field`.
pub(crate) struct CalcCard {
    pub item_field: Option<usize>,
    pub name: Entity<InputState>,
    pub formula: Entity<InputState>,
    pub error: Option<String>,
    _subs: Vec<Subscription>,
}

/// Excel's names for the date grouping levels and Show Values As, shown
/// through `t_dyn` (checked in the tests).
pub(crate) const LEVEL_LABELS: [&str; 7] = [
    tk("Seconds"),
    tk("Minutes"),
    tk("Hours"),
    tk("Days"),
    tk("Months"),
    tk("Quarters"),
    tk("Years"),
];

fn form_label(l: ReportLayout) -> &'static str {
    match l {
        ReportLayout::Compact => i18n::t("Show item labels in compact form"),
        ReportLayout::Outline => i18n::t("Show item labels in outline form"),
        ReportLayout::Tabular => i18n::t("Show item labels in tabular form"),
    }
}

fn sort_label(s: PivotSort) -> &'static str {
    match s {
        PivotSort::Manual => i18n::t("Manual (data source order)"),
        PivotSort::Ascending => i18n::t("Ascending (A to Z)"),
        PivotSort::Descending => i18n::t("Descending (Z to A)"),
    }
}

fn family_label(f: Option<FilterFamily>) -> &'static str {
    match f {
        None => i18n::t("No filter"),
        Some(FilterFamily::Label) => i18n::t("Label Filter"),
        Some(FilterFamily::Value) => i18n::t("Value Filter"),
        Some(FilterFamily::Top) => i18n::t("Top 10"),
        Some(FilterFamily::Date) => i18n::t("Date Filter"),
    }
}

/// The filter types of a family, with Excel's wording.
fn types_of(f: FilterFamily) -> impl Iterator<Item = (&'static str, &'static str)> {
    PIVOT_FILTER_TYPES
        .iter()
        .filter(move |(_, _, fam)| *fam == f)
        .map(|(k, label, _)| (*k, *label))
}

fn type_label(kind: &str) -> String {
    PIVOT_FILTER_TYPES
        .iter()
        .find(|(k, _, _)| *k == kind)
        .map_or_else(|| kind.to_owned(), |(_, l, _)| t_dyn(l).to_owned())
}

/// A serial date as `2026-01-05`.
fn iso(serial: f64) -> String {
    let days = serial.floor() as i64 - 25_569;
    // Days from 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// A typed date (`2026-01-05`, `1/5/2026`) as a serial date.
fn serial_of(s: &str) -> Option<f64> {
    let s = s.trim();
    let civil = |y: i64, m: i64, d: i64| -> Option<f64> {
        if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
            return None;
        }
        let y = if m <= 2 { y - 1 } else { y };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        Some((era * 146_097 + doe - 719_468 + 25_569) as f64)
    };
    let p: Vec<&str> = s.split(['-', '/']).collect();
    let n = |x: &str| x.trim().parse::<i64>().ok();
    match p[..] {
        [y, m, d] if s.contains('-') => civil(n(y)?, n(m)?, n(d)?),
        [m, d, y] => civil(n(y)?, n(m)?, n(d)?),
        _ => s.parse().ok(),
    }
}

fn number_text(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}

impl SheetsView {
    fn panel_layout(&self) -> Option<PivotLayout> {
        self.pivot_panel.as_ref().and_then(|p| p.layout.clone())
    }

    /// Close every card of the panel.
    pub(crate) fn close_pivot_cards(&mut self) {
        if let Some(p) = self.pivot_panel.as_mut() {
            p.settings = None;
            p.picker = None;
            p.field_card = None;
            p.group_card = None;
            p.calc_card = None;
        }
    }

    /// Run `op` on the panel's pivot table (`(sheet, row, col)` of its
    /// body) and report how it went: `done` on success.
    pub(crate) fn pivot_op(
        &mut self,
        done: String,
        op: impl FnOnce(&mut sheets_engine::Workbook, u32, u32, u32) -> anyhow::Result<()>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.editing.is_some() || self.blocked(cx) {
            return false;
        }
        let Some((sheet, row, col)) = self.panel_anchor(cx) else {
            return false;
        };
        let result = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let res = op(&mut grid.book, sheet, row, col);
            grid.resize();
            table.refresh(cx);
            res
        });
        let ok = result.is_ok();
        self.status = match result {
            Ok(()) => done.into(),
            Err(e) => format!("{e:#}").into(),
        };
        self.after_pivot_edit(window, cx);
        ok
    }

    // ---------------------------------------------------------- field settings

    pub(crate) fn open_field_card(
        &mut self,
        field: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(l) = self.panel_layout() else { return };
        let Some(f) = l.fields.get(field) else { return };
        let settings = f.settings.clone();
        let input = |cx: &mut Context<Self>, window: &mut Window, v: &str| {
            let e = cx.new(|cx| InputState::new(window, cx));
            e.update(cx, |i, cx| i.set_value(v.to_owned(), window, cx));
            e
        };
        let (a, b) = settings
            .filter
            .as_ref()
            .map(|x| (x.value1.clone(), x.value2.clone()))
            .unwrap_or_default();
        let v1 = input(cx, window, &a);
        let v2 = input(cx, window, &b);
        let enter = |this: &mut Self,
                     _: &Entity<InputState>,
                     e: &InputEvent,
                     w: &mut Window,
                     cx: &mut Context<Self>| {
            if let InputEvent::PressEnter { .. } = e {
                this.apply_field_card(w, cx);
            }
        };
        let subs = vec![
            cx.subscribe_in(&v1, window, enter),
            cx.subscribe_in(&v2, window, enter),
        ];
        let family = settings
            .filter
            .as_ref()
            .and_then(|f| sheets_engine::filter_family(&f.kind));
        self.close_pivot_cards();
        if let Some(p) = self.pivot_panel.as_mut() {
            let other = settings
                .filter2
                .as_ref()
                .map(|x| (x.value1.clone(), x.value2.clone()))
                .unwrap_or_default();
            p.field_card = Some(FieldCard {
                field,
                settings,
                family,
                v1,
                v2,
                second: false,
                other,
                _subs: subs,
            });
        }
        cx.notify();
    }

    /// Edit the field's other filter (Filter 1 / Filter 2).
    fn switch_field_filter(&mut self, second: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self
            .pivot_panel
            .as_mut()
            .and_then(|p| p.field_card.as_mut())
        else {
            return;
        };
        if c.second == second {
            return;
        }
        let now = (
            c.v1.read(cx).value().to_string(),
            c.v2.read(cx).value().to_string(),
        );
        if let Some(f) = c.settings.filter.as_mut() {
            (f.value1, f.value2) = now.clone();
        }
        if c.family.is_none() {
            c.settings.filter = None;
        }
        std::mem::swap(&mut c.settings.filter, &mut c.settings.filter2);
        let (a, b) = std::mem::replace(&mut c.other, now);
        c.second = second;
        c.family = c
            .settings
            .filter
            .as_ref()
            .and_then(|f| sheets_engine::filter_family(&f.kind));
        let (v1, v2) = (c.v1.clone(), c.v2.clone());
        v1.update(cx, |i, cx| i.set_value(a, window, cx));
        v2.update(cx, |i, cx| i.set_value(b, window, cx));
        cx.notify();
    }

    fn edit_field_card(&mut self, f: impl FnOnce(&mut FieldCard), cx: &mut Context<Self>) {
        if let Some(c) = self
            .pivot_panel
            .as_mut()
            .and_then(|p| p.field_card.as_mut())
        {
            f(c);
        }
        cx.notify();
    }

    fn apply_field_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(card) = self.pivot_panel.as_mut().and_then(|p| p.field_card.take()) else {
            return;
        };
        let mut settings = card.settings.clone();
        if let Some(f) = settings.filter.as_mut() {
            f.value1 = card.v1.read(cx).value().trim().to_owned();
            f.value2 = card.v2.read(cx).value().trim().to_owned();
        }
        if card.family.is_none() {
            settings.filter = None;
        }
        if let Some(f) = settings.filter2.as_mut() {
            f.value1 = card.other.0.trim().to_owned();
            f.value2 = card.other.1.trim().to_owned();
        }
        if card.second {
            std::mem::swap(&mut settings.filter, &mut settings.filter2);
        }
        if settings.filter.is_none() {
            settings.filter = settings.filter2.take();
        }
        let field = card.field;
        let Some(mut layout) = self.panel_layout() else {
            return;
        };
        if field >= layout.fields.len() {
            return;
        }
        layout.fields[field].settings = settings;
        let before = self.panel_layout();
        if before.as_ref() == Some(&layout) {
            self.focus_grid(window, cx);
            return;
        }
        if !self.apply_layout(layout, window, cx)
            && let Some(p) = self.pivot_panel.as_mut()
        {
            // Keep the card open to fix what was refused.
            p.field_card = Some(card);
        }
    }

    pub(crate) fn render_field_card(
        &self,
        l: &PivotLayout,
        c: &FieldCard,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let Some(field) = l.fields.get(c.field) else {
            return div();
        };
        let s = &c.settings;
        let custom = matches!(s.subtotals, FieldSubtotals::Custom(_));
        let picked = match &s.subtotals {
            FieldSubtotals::Custom(v) => v.clone(),
            _ => Vec::new(),
        };
        let radio = |id: &'static str, label: &'static str, on: bool, value: FieldSubtotals| {
            let v = view.clone();
            Button::new(id)
                .xsmall()
                .label(label)
                .when_else(on, |b| b.primary(), |b| b.outline())
                .on_click(move |_, _, cx| {
                    let value = value.clone();
                    let _ = v.update(cx, |this, cx| {
                        this.edit_field_card(|c| c.settings.subtotals = value, cx)
                    });
                })
        };
        let mut funcs = h_flex().flex_wrap().gap(SPACE_2);
        if custom {
            for f in PivotFunc::ALL {
                let v = view.clone();
                let on = picked.contains(&f);
                funcs = funcs.child(
                    Checkbox::new(SharedString::from(format!("pivot-fs-func-{}", f.label())))
                        .label(t_dyn(f.label()))
                        .checked(on)
                        .on_click(move |on, _, cx| {
                            let on = *on;
                            let _ = v.update(cx, |this, cx| {
                                this.edit_field_card(
                                    |c| {
                                        if let FieldSubtotals::Custom(list) =
                                            &mut c.settings.subtotals
                                        {
                                            list.retain(|x| *x != f);
                                            if on {
                                                list.push(f);
                                            }
                                        }
                                    },
                                    cx,
                                )
                            });
                        }),
                );
            }
        }
        let form_view = view.clone();
        let form = s.form;
        let sort = s.sort;
        let by_value = s.sort_by_value;
        let sort_view = view.clone();
        let by_view = view.clone();
        let values: Vec<String> = (0..l.values.len()).map(|k| l.value_caption(k)).collect();
        let by_label = match by_value {
            Some(k) => values.get(k).cloned().unwrap_or_default(),
            None => tf("{} (labels)", &[&field.name]),
        };
        let field_name = field.name.clone();
        let values_by = values.clone();
        let rep_view = view.clone();
        let blank_view = view.clone();
        // Filter.
        let fam_view = view.clone();
        let family = c.family;
        let dates = field.dates;
        let current = s.filter.clone();
        let kind_view = view.clone();
        let measure_view = view.clone();
        let top_view = view.clone();
        let kind = current.as_ref().map(|f| f.kind.clone()).unwrap_or_default();
        let arity = if family.is_some() {
            filter_arity(&kind)
        } else {
            0
        };
        let measure_label = current
            .as_ref()
            .and_then(|f| f.value_field)
            .and_then(|k| values.get(k).cloned())
            .unwrap_or_else(|| i18n::t("Choose a value field").to_owned());
        let values_m = values.clone();
        let top = current.as_ref().is_none_or(|f| f.top);
        let ok = view.clone();
        let cancel = view.clone();
        crate::pivot::card(cx)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(SharedString::from(tf(
                        "Field Settings · {}",
                        &[&field.name],
                    ))),
            )
            .child(kantor_ui::components::section_label(
                i18n::t("Subtotals"),
                cx,
            ))
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .child(radio(
                        "pivot-fs-auto",
                        i18n::t("Automatic"),
                        s.subtotals == FieldSubtotals::Automatic,
                        FieldSubtotals::Automatic,
                    ))
                    .child(radio(
                        "pivot-fs-none",
                        i18n::t("None"),
                        s.subtotals == FieldSubtotals::None,
                        FieldSubtotals::None,
                    ))
                    .child(radio(
                        "pivot-fs-custom",
                        i18n::t("Custom"),
                        custom,
                        FieldSubtotals::Custom(if custom {
                            picked.clone()
                        } else {
                            vec![PivotFunc::Sum]
                        }),
                    )),
            )
            .when(custom, |d| d.child(funcs))
            .child(kantor_ui::components::section_label(i18n::t("Layout"), cx))
            .child(
                Button::new("pivot-fs-form")
                    .outline()
                    .small()
                    .label(form_label(form))
                    .dropdown_caret(true)
                    .dropdown_menu(move |mut m, _, _| {
                        for r in [
                            ReportLayout::Compact,
                            ReportLayout::Outline,
                            ReportLayout::Tabular,
                        ] {
                            let v = form_view.clone();
                            m = m.item(
                                PopupMenuItem::new(form_label(r))
                                    .checked(r == form)
                                    .on_click(move |_, _, cx| {
                                        let _ = v.update(cx, |this, cx| {
                                            this.edit_field_card(|c| c.settings.form = r, cx)
                                        });
                                    }),
                            );
                        }
                        m
                    }),
            )
            .child(
                Checkbox::new("pivot-fs-repeat")
                    .label(i18n::t("Repeat item labels"))
                    .checked(s.repeat_labels)
                    .disabled(form == ReportLayout::Compact)
                    .on_click(move |on, _, cx| {
                        let on = *on;
                        let _ = rep_view.update(cx, |this, cx| {
                            this.edit_field_card(|c| c.settings.repeat_labels = on, cx)
                        });
                    }),
            )
            .child(
                Checkbox::new("pivot-fs-blank")
                    .label(i18n::t("Insert blank line after each item label"))
                    .checked(s.blank_line)
                    .on_click(move |on, _, cx| {
                        let on = *on;
                        let _ = blank_view.update(cx, |this, cx| {
                            this.edit_field_card(|c| c.settings.blank_line = on, cx)
                        });
                    }),
            )
            .child(kantor_ui::components::section_label(i18n::t("Sort"), cx))
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .flex_wrap()
                    .child(
                        Button::new("pivot-fs-sort")
                            .outline()
                            .small()
                            .label(sort_label(sort))
                            .dropdown_caret(true)
                            .dropdown_menu(move |mut m, _, _| {
                                for o in [
                                    PivotSort::Manual,
                                    PivotSort::Ascending,
                                    PivotSort::Descending,
                                ] {
                                    let v = sort_view.clone();
                                    m = m.item(
                                        PopupMenuItem::new(sort_label(o))
                                            .checked(o == sort)
                                            .on_click(move |_, _, cx| {
                                                let _ = v.update(cx, |this, cx| {
                                                    this.edit_field_card(
                                                        |c| c.settings.sort = o,
                                                        cx,
                                                    )
                                                });
                                            }),
                                    );
                                }
                                m
                            }),
                    )
                    .when(sort != PivotSort::Manual, |d| {
                        d.child(
                            Button::new("pivot-fs-sort-by")
                                .outline()
                                .small()
                                .label(SharedString::from(tf("by {}", &[&by_label])))
                                .dropdown_caret(true)
                                .dropdown_menu(move |mut m, _, _| {
                                    let choices =
                                        std::iter::once((tf("{} (labels)", &[&field_name]), None))
                                            .chain(
                                                values_by
                                                    .iter()
                                                    .cloned()
                                                    .enumerate()
                                                    .map(|(k, n)| (n, Some(k))),
                                            );
                                    for (label, k) in choices {
                                        let v = by_view.clone();
                                        m = m.item(
                                            PopupMenuItem::new(label)
                                                .checked(k == by_value)
                                                .on_click(move |_, _, cx| {
                                                    let _ = v.update(cx, |this, cx| {
                                                        this.edit_field_card(
                                                            |c| c.settings.sort_by_value = k,
                                                            cx,
                                                        )
                                                    });
                                                }),
                                        );
                                    }
                                    m
                                }),
                        )
                    }),
            )
            .child(kantor_ui::components::section_label(i18n::t("Filter"), cx))
            .when(l.multiple_filters, |d| {
                let pick = |second: bool, label: &'static str| {
                    let v = view.clone();
                    Button::new(if second {
                        "pivot-fs-filter2"
                    } else {
                        "pivot-fs-filter1"
                    })
                    .xsmall()
                    .label(label)
                    .when_else(c.second == second, |b| b.primary(), |b| b.outline())
                    .on_click(move |_, window, cx| {
                        let _ =
                            v.update(cx, |this, cx| this.switch_field_filter(second, window, cx));
                    })
                };
                d.child(
                    h_flex()
                        .gap(SPACE_2)
                        .flex_wrap()
                        .child(pick(false, i18n::t("Filter 1")))
                        .child(pick(true, i18n::t("Filter 2")))
                        .child(
                            div()
                                .text_size(FONT_SMALL)
                                .text_color(t.fg_muted)
                                .child(i18n::t("one by label or date, one by value")),
                        ),
                )
            })
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .flex_wrap()
                    .child(
                        Button::new("pivot-fs-family")
                            .outline()
                            .small()
                            .label(family_label(family))
                            .dropdown_caret(true)
                            .dropdown_menu(move |mut m, _, _| {
                                let mut fams = vec![None, Some(FilterFamily::Label)];
                                if dates {
                                    fams.push(Some(FilterFamily::Date));
                                }
                                fams.push(Some(FilterFamily::Value));
                                fams.push(Some(FilterFamily::Top));
                                for fam in fams {
                                    let v = fam_view.clone();
                                    m = m.item(
                                        PopupMenuItem::new(family_label(fam))
                                            .checked(fam == family)
                                            .on_click(move |_, _, cx| {
                                                let _ = v.update(cx, |this, cx| {
                                                    this.edit_field_card(
                                                        |c| {
                                                            c.family = fam;
                                                            c.settings.filter =
                                                                fam.map(|f| PivotFieldFilter {
                                                                    kind: types_of(f)
                                                                        .next()
                                                                        .map(|x| x.0)
                                                                        .unwrap_or_default()
                                                                        .to_owned(),
                                                                    value_field: matches!(
                                                                        f,
                                                                        FilterFamily::Value
                                                                            | FilterFamily::Top
                                                                    )
                                                                    .then_some(0),
                                                                    value1: if f
                                                                        == FilterFamily::Top
                                                                    {
                                                                        "10".into()
                                                                    } else {
                                                                        String::new()
                                                                    },
                                                                    value2: String::new(),
                                                                    top: true,
                                                                });
                                                        },
                                                        cx,
                                                    )
                                                });
                                            }),
                                    );
                                }
                                m
                            }),
                    )
                    .when_some(family, |d, fam| {
                        let kind_now = kind.clone();
                        d.child(
                            Button::new("pivot-fs-kind")
                                .outline()
                                .small()
                                .label(type_label(&kind))
                                .dropdown_caret(true)
                                .dropdown_menu(move |mut m, _, _| {
                                    for (k, label) in types_of(fam) {
                                        let v = kind_view.clone();
                                        m = m.item(
                                            PopupMenuItem::new(t_dyn(label))
                                                .checked(k == kind_now)
                                                .on_click(move |_, _, cx| {
                                                    let _ = v.update(cx, |this, cx| {
                                                        this.edit_field_card(
                                                            |c| {
                                                                if let Some(f) =
                                                                    c.settings.filter.as_mut()
                                                                {
                                                                    f.kind = k.to_owned();
                                                                }
                                                            },
                                                            cx,
                                                        )
                                                    });
                                                }),
                                        );
                                    }
                                    m
                                }),
                        )
                    }),
            )
            .when(matches!(family, Some(FilterFamily::Top)), |d| {
                d.child(
                    Button::new("pivot-fs-top")
                        .outline()
                        .small()
                        .label(if top {
                            i18n::t("Top")
                        } else {
                            i18n::t("Bottom")
                        })
                        .dropdown_caret(true)
                        .dropdown_menu(move |mut m, _, _| {
                            for (label, on) in [(i18n::t("Top"), true), (i18n::t("Bottom"), false)]
                            {
                                let v = top_view.clone();
                                m = m.item(PopupMenuItem::new(label).checked(on == top).on_click(
                                    move |_, _, cx| {
                                        let _ = v.update(cx, |this, cx| {
                                            this.edit_field_card(
                                                |c| {
                                                    if let Some(f) = c.settings.filter.as_mut() {
                                                        f.top = on;
                                                    }
                                                },
                                                cx,
                                            )
                                        });
                                    },
                                ));
                            }
                            m
                        }),
                )
            })
            .when(
                matches!(family, Some(FilterFamily::Value | FilterFamily::Top)),
                |d| {
                    d.child(
                        Button::new("pivot-fs-measure")
                            .outline()
                            .small()
                            .label(SharedString::from(measure_label.clone()))
                            .dropdown_caret(true)
                            .dropdown_menu(move |mut m, _, _| {
                                for (k, n) in values_m.iter().enumerate() {
                                    let v = measure_view.clone();
                                    m = m.item(PopupMenuItem::new(n.clone()).on_click(
                                        move |_, _, cx| {
                                            let _ = v.update(cx, |this, cx| {
                                                this.edit_field_card(
                                                    |c| {
                                                        if let Some(f) = c.settings.filter.as_mut()
                                                        {
                                                            f.value_field = Some(k);
                                                        }
                                                    },
                                                    cx,
                                                )
                                            });
                                        },
                                    ));
                                }
                                m
                            }),
                    )
                },
            )
            .when(arity >= 1, |d| d.child(Input::new(&c.v1).small()))
            .when(arity >= 2, |d| {
                d.child(
                    div()
                        .text_size(FONT_SMALL)
                        .text_color(t.fg_muted)
                        .child(i18n::t("and")),
                )
                .child(Input::new(&c.v2).small())
            })
            .when(
                matches!(family, Some(FilterFamily::Date)) && arity > 0,
                |d| {
                    d.child(
                        div()
                            .text_size(FONT_SMALL)
                            .text_color(t.fg_muted)
                            .child(i18n::t("Dates such as 2026-01-31 or 1/31/2026")),
                    )
                },
            )
            .child(card_buttons("pivot-fs", ok, cancel, |this, w, cx| {
                this.apply_field_card(w, cx)
            }))
    }

    // ---------------------------------------------------------- grouping

    pub(crate) fn open_group_card(
        &mut self,
        field: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(l) = self.panel_layout() else { return };
        // A level field (Years) stands for its base field.
        let field = l
            .fields
            .get(field)
            .and_then(|f| f.group_base)
            .filter(|b| l.fields.get(*b).is_some_and(|f| f.grouping.is_some()))
            .unwrap_or(field);
        let Some(f) = l.fields.get(field) else { return };
        let dates = f.dates;
        let (start, end, by, levels, days, auto_start, auto_end) = match &f.grouping {
            Some(PivotGrouping::Numbers {
                start,
                end,
                by,
                auto_start,
                auto_end,
            }) => (*start, *end, *by, vec![], 1, *auto_start, *auto_end),
            Some(PivotGrouping::Dates {
                start,
                end,
                levels,
                days,
                auto_start,
                auto_end,
            }) => (
                *start,
                *end,
                1.0,
                levels.clone(),
                *days,
                *auto_start,
                *auto_end,
            ),
            None => (
                f.min.unwrap_or(0.0),
                if dates {
                    f.max.unwrap_or(0.0).floor() + 1.0
                } else {
                    f.max.unwrap_or(0.0)
                },
                if dates { 1.0 } else { 10.0 },
                vec![DateLevel::Months],
                7,
                true,
                true,
            ),
        };
        let shown = |x: f64| if dates { iso(x) } else { number_text(x) };
        let input = |cx: &mut Context<Self>, window: &mut Window, v: String| {
            let e = cx.new(|cx| InputState::new(window, cx));
            e.update(cx, |i, cx| i.set_value(v, window, cx));
            e
        };
        let start_i = input(cx, window, shown(start));
        let end_i = input(cx, window, shown(end));
        let by_i = input(cx, window, number_text(by));
        let days_i = input(cx, window, days.to_string());
        let enter = |this: &mut Self,
                     _: &Entity<InputState>,
                     e: &InputEvent,
                     w: &mut Window,
                     cx: &mut Context<Self>| {
            if let InputEvent::PressEnter { .. } = e {
                this.apply_group_card(w, cx);
            }
        };
        let subs = [&start_i, &end_i, &by_i, &days_i]
            .into_iter()
            .map(|e| cx.subscribe_in(e, window, enter))
            .collect();
        self.close_pivot_cards();
        if let Some(p) = self.pivot_panel.as_mut() {
            p.group_card = Some(GroupCard {
                field,
                dates,
                levels,
                auto_start,
                auto_end,
                start: start_i,
                end: end_i,
                by: by_i,
                days: days_i,
                error: None,
                _subs: subs,
            });
        }
        cx.notify();
    }

    fn edit_group_card(&mut self, f: impl FnOnce(&mut GroupCard), cx: &mut Context<Self>) {
        if let Some(c) = self
            .pivot_panel
            .as_mut()
            .and_then(|p| p.group_card.as_mut())
        {
            f(c);
        }
        cx.notify();
    }

    fn apply_group_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self
            .pivot_panel
            .as_ref()
            .and_then(|p| p.group_card.as_ref())
        else {
            return;
        };
        let text = |e: &Entity<InputState>| e.read(cx).value().trim().to_owned();
        let read = |s: &str| {
            if c.dates {
                serial_of(s)
            } else {
                s.parse::<f64>().ok()
            }
        };
        let (start, end) = (read(&text(&c.start)), read(&text(&c.end)));
        let grouping = match (start, end) {
            (Some(start), Some(end)) if c.dates => Ok(PivotGrouping::Dates {
                start,
                end,
                auto_start: c.auto_start,
                auto_end: c.auto_end,
                levels: c.levels.clone(),
                days: text(&c.days).parse().unwrap_or(1),
            }),
            (Some(start), Some(end)) => match text(&c.by).parse::<f64>() {
                Ok(by) => Ok(PivotGrouping::Numbers {
                    start,
                    end,
                    by,
                    auto_start: c.auto_start,
                    auto_end: c.auto_end,
                }),
                Err(_) => Err(i18n::t("By must be a number").to_owned()),
            },
            _ if c.dates => Err(i18n::t("Type the dates such as 2026-01-31").to_owned()),
            _ => Err(i18n::t("Starting at and Ending at must be numbers").to_owned()),
        };
        let field = c.field;
        let grouping = match grouping {
            Ok(g) => g,
            Err(e) => {
                self.edit_group_card(|c| c.error = Some(e), cx);
                return;
            }
        };
        let name = self.panel_layout().map(|l| l.name).unwrap_or_default();
        let ok = self.pivot_op(
            tf("{} grouped", &[&name]),
            move |book, s, r, c| book.group_pivot_field(s, r, c, field, &grouping),
            window,
            cx,
        );
        if ok {
            if let Some(p) = self.pivot_panel.as_mut() {
                p.group_card = None;
            }
        } else {
            let e = self.status.to_string();
            self.edit_group_card(|c| c.error = Some(e), cx);
        }
    }

    pub(crate) fn render_group_card(
        &self,
        l: &PivotLayout,
        c: &GroupCard,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let name = l
            .fields
            .get(c.field)
            .map(|f| f.name.clone())
            .unwrap_or_default();
        let auto = |id: &'static str, on: bool, start: bool| {
            let v = view.clone();
            Checkbox::new(id)
                .label(i18n::t("Auto"))
                .checked(on)
                .on_click(move |on, _, cx| {
                    let on = *on;
                    let _ = v.update(cx, |this, cx| {
                        this.edit_group_card(
                            |c| {
                                if start {
                                    c.auto_start = on
                                } else {
                                    c.auto_end = on
                                }
                            },
                            cx,
                        )
                    });
                })
        };
        let mut levels = h_flex().flex_wrap().gap(SPACE_2);
        if c.dates {
            for (level, label) in DateLevel::ALL.into_iter().zip(LEVEL_LABELS) {
                let v = view.clone();
                levels = levels.child(
                    Checkbox::new(SharedString::from(format!("pivot-group-{label}")))
                        .label(t_dyn(label))
                        .checked(c.levels.contains(&level))
                        .on_click(move |on, _, cx| {
                            let on = *on;
                            let _ = v.update(cx, |this, cx| {
                                this.edit_group_card(
                                    |c| {
                                        c.levels.retain(|x| *x != level);
                                        if on {
                                            c.levels.push(level);
                                        }
                                    },
                                    cx,
                                )
                            });
                        }),
                );
            }
        }
        let only_days = c.levels == [DateLevel::Days];
        let ok = view.clone();
        let cancel = view.clone();
        crate::pivot::card(cx)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(SharedString::from(tf("Grouping · {}", &[&name]))),
            )
            .child(kantor_ui::components::section_label(
                i18n::t("Starting at"),
                cx,
            ))
            .child(
                h_flex()
                    .gap(SPACE_3)
                    .child(div().flex_1().child(Input::new(&c.start).small()))
                    .child(auto("pivot-group-auto-start", c.auto_start, true)),
            )
            .child(kantor_ui::components::section_label(
                i18n::t("Ending at"),
                cx,
            ))
            .child(
                h_flex()
                    .gap(SPACE_3)
                    .child(div().flex_1().child(Input::new(&c.end).small()))
                    .child(auto("pivot-group-auto-end", c.auto_end, false)),
            )
            .when(!c.dates, |d| {
                d.child(kantor_ui::components::section_label(i18n::t("By"), cx))
                    .child(Input::new(&c.by).small())
            })
            .when(c.dates, |d| {
                d.child(kantor_ui::components::section_label(i18n::t("By"), cx))
                    .child(levels)
                    .child(kantor_ui::components::section_label(
                        i18n::t("Number of days"),
                        cx,
                    ))
                    .child(
                        div()
                            .when(!only_days, |d| d.opacity(0.5))
                            .child(Input::new(&c.days).small().disabled(!only_days)),
                    )
            })
            .when_some(c.error.clone(), |d, e| {
                d.child(
                    div()
                        .text_size(FONT_SMALL)
                        .text_color(t.danger)
                        .child(SharedString::from(e)),
                )
            })
            .child(card_buttons("pivot-group", ok, cancel, |this, w, cx| {
                this.apply_group_card(w, cx)
            }))
    }

    /// Ungroup field `field` (all its grouping).
    pub(crate) fn ungroup_field(
        &mut self,
        field: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self.panel_layout().map(|l| l.name).unwrap_or_default();
        self.pivot_op(
            tf("{} ungrouped", &[&name]),
            move |book, s, r, c| book.ungroup_pivot_field(s, r, c, field, None),
            window,
            cx,
        );
    }

    /// Group the items picked in the item picker (Group Selection).
    pub(crate) fn apply_group_selection(
        &mut self,
        field: usize,
        labels: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut made = String::new();
        let ok = self.pivot_op(
            String::new(),
            |book, s, r, c| {
                made = book.group_pivot_items(s, r, c, field, &labels)?;
                Ok(())
            },
            window,
            cx,
        );
        if ok {
            self.status = tf("{} made", &[&made]).into();
        }
    }

    // ---------------------------------------------------------- calculated field

    pub(crate) fn open_calc_card(
        &mut self,
        existing: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(l) = self.panel_layout() else { return };
        let (name, formula) = match existing.and_then(|k| l.fields.get(k)) {
            Some(f) => (f.name.clone(), f.formula.clone().unwrap_or_default()),
            None => {
                let n = (1..)
                    .map(|k| format!("{}{k}", i18n::t("Field")))
                    .find(|n| !l.fields.iter().any(|f| f.name.eq_ignore_ascii_case(n)))
                    .unwrap_or_default();
                (n, "= 0".to_owned())
            }
        };
        let name_i = cx.new(|cx| InputState::new(window, cx));
        name_i.update(cx, |i, cx| i.set_value(name, window, cx));
        let formula_i = cx.new(|cx| InputState::new(window, cx));
        formula_i.update(cx, |i, cx| {
            i.set_value(formula, window, cx);
            i.focus(window, cx);
        });
        let enter = |this: &mut Self,
                     _: &Entity<InputState>,
                     e: &InputEvent,
                     w: &mut Window,
                     cx: &mut Context<Self>| {
            if let InputEvent::PressEnter { .. } = e {
                this.apply_calc_card(w, cx);
            }
        };
        let subs = vec![
            cx.subscribe_in(&name_i, window, enter),
            cx.subscribe_in(&formula_i, window, enter),
        ];
        self.close_pivot_cards();
        if let Some(p) = self.pivot_panel.as_mut() {
            p.calc_card = Some(CalcCard {
                item_field: None,
                name: name_i,
                formula: formula_i,
                error: None,
                _subs: subs,
            });
        }
        cx.notify();
    }

    /// Insert Calculated Item in field `field` (a row or column field).
    pub(crate) fn open_calc_item_card(
        &mut self,
        field: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(l) = self.panel_layout() else { return };
        let Some(f) = l.fields.get(field) else { return };
        let name = (1..)
            .map(|k| format!("{}{k}", i18n::t("Formula")))
            .find(|n| !f.items.iter().any(|i| i.label.eq_ignore_ascii_case(n)))
            .unwrap_or_default();
        self.open_calc_card(None, window, cx);
        if let Some(c) = self.pivot_panel.as_mut().and_then(|p| p.calc_card.as_mut()) {
            c.item_field = Some(field);
            let (n, fo) = (c.name.clone(), c.formula.clone());
            n.update(cx, |i, cx| i.set_value(name, window, cx));
            fo.update(cx, |i, cx| i.set_value("= 0", window, cx));
        }
        cx.notify();
    }

    fn apply_calc_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.pivot_panel.as_ref().and_then(|p| p.calc_card.as_ref()) else {
            return;
        };
        let name = c.name.read(cx).value().trim().to_owned();
        let formula = c.formula.read(cx).value().trim().to_owned();
        let n = name.clone();
        let item_field = c.item_field;
        let ok = self.pivot_op(
            tf("{} saved", &[&name]),
            move |book, s, r, col| match item_field {
                Some(f) => book.set_pivot_calculated_item(s, r, col, f, &n, &formula),
                None => book.set_pivot_calculated_field(s, r, col, &n, &formula),
            },
            window,
            cx,
        );
        if ok {
            if let Some(p) = self.pivot_panel.as_mut() {
                p.calc_card = None;
            }
        } else {
            let e = self.status.to_string();
            if let Some(c) = self.pivot_panel.as_mut().and_then(|p| p.calc_card.as_mut()) {
                c.error = Some(e);
            }
            cx.notify();
        }
    }

    /// Delete the calculated item named in the Calculated Item card.
    fn delete_calc_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.pivot_panel.as_ref().and_then(|p| p.calc_card.as_ref()) else {
            return;
        };
        let Some(field) = c.item_field else { return };
        let name = c.name.read(cx).value().trim().to_owned();
        let n = name.clone();
        let ok = self.pivot_op(
            tf("{} deleted", &[&name]),
            move |book, s, r, col| book.delete_pivot_calculated_item(s, r, col, field, &n),
            window,
            cx,
        );
        if ok && let Some(p) = self.pivot_panel.as_mut() {
            p.calc_card = None;
        }
        cx.notify();
    }

    /// Delete calculated field `field`.
    pub(crate) fn delete_calc_field(
        &mut self,
        field: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self
            .panel_layout()
            .and_then(|l| l.fields.get(field).map(|f| f.name.clone()))
        else {
            return;
        };
        let n = name.clone();
        self.pivot_op(
            tf("{} deleted", &[&name]),
            move |book, s, r, c| book.delete_pivot_calculated_field(s, r, c, &n),
            window,
            cx,
        );
    }

    pub(crate) fn render_calc_card(
        &self,
        l: &PivotLayout,
        c: &CalcCard,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let mut fields = h_flex().flex_wrap().gap(SPACE_1);
        // The names the formula can use: fields, or the field's items.
        let names: Vec<String> = match c.item_field.and_then(|f| l.fields.get(f)) {
            Some(f) => f.items.iter().map(|i| i.label.clone()).collect(),
            None => l
                .fields
                .iter()
                .filter(|f| f.formula.is_none() && f.group_base.is_none())
                .map(|f| f.name.clone())
                .collect(),
        };
        let calc_items: Vec<(String, String)> = c
            .item_field
            .and_then(|f| l.fields.get(f))
            .map(|f| f.calc_items.clone())
            .unwrap_or_default();
        for (k, name) in names.into_iter().enumerate() {
            let v = view.clone();
            fields = fields.child(
                Button::new(("pivot-calc-field", k))
                    .xsmall()
                    .ghost()
                    .label(SharedString::from(name.clone()))
                    .on_click(move |_, w, cx| {
                        let name = name.clone();
                        let _ = v.update(cx, |this, cx| {
                            let Some(c) =
                                this.pivot_panel.as_ref().and_then(|p| p.calc_card.as_ref())
                            else {
                                return;
                            };
                            let quoted = if name.chars().all(|ch| ch.is_alphanumeric() || ch == '_')
                            {
                                name
                            } else {
                                format!("'{}'", name.replace('\'', "''"))
                            };
                            let input = c.formula.clone();
                            input.update(cx, |i, cx| {
                                let now = i.value().to_string();
                                let sep = if now.ends_with(['=', ' ', '(', '+', '-', '*', '/', ','])
                                {
                                    ""
                                } else {
                                    " "
                                };
                                i.set_value(format!("{now}{sep}{quoted}"), w, cx);
                                i.focus(w, cx);
                            });
                        });
                    }),
            );
        }
        // A calculated item's existing ones: click to change or delete.
        let mut existing = h_flex().flex_wrap().gap(SPACE_1);
        for (k, (name, formula)) in calc_items.iter().enumerate() {
            let v = view.clone();
            let (name, formula) = (name.clone(), formula.clone());
            existing = existing.child(
                Button::new(("pivot-calc-item", k))
                    .xsmall()
                    .outline()
                    .label(SharedString::from(name.clone()))
                    .on_click(move |_, w, cx| {
                        let _ = v.update(cx, |this, cx| {
                            let Some(c) =
                                this.pivot_panel.as_ref().and_then(|p| p.calc_card.as_ref())
                            else {
                                return;
                            };
                            let (n, f) = (c.name.clone(), c.formula.clone());
                            n.update(cx, |i, cx| i.set_value(name.clone(), w, cx));
                            f.update(cx, |i, cx| i.set_value(formula.clone(), w, cx));
                        });
                    }),
            );
        }
        let typed = c.name.read(cx).value().trim().to_owned();
        let deletable = calc_items
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case(&typed));
        let del = view.clone();
        let item_mode = c.item_field.is_some();
        let (title, hint) = match c.item_field.and_then(|f| l.fields.get(f)) {
            Some(f) => (
                tf("Insert Calculated Item in \"{}\"", &[&f.name]),
                i18n::t("Items · click to insert into the formula"),
            ),
            None => (
                i18n::t("Insert Calculated Field").to_owned(),
                i18n::t("Fields · click to insert into the formula"),
            ),
        };
        let ok = view.clone();
        let cancel = view.clone();
        crate::pivot::card(cx)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(SharedString::from(title)),
            )
            .child(kantor_ui::components::section_label(i18n::t("Name"), cx))
            .child(Input::new(&c.name).small())
            .child(kantor_ui::components::section_label(i18n::t("Formula"), cx))
            .child(Input::new(&c.formula).small())
            .child(
                div()
                    .text_size(FONT_SMALL)
                    .text_color(t.fg_muted)
                    .child(hint),
            )
            .child(fields)
            .when(!calc_items.is_empty(), |d| {
                d.child(kantor_ui::components::section_label(
                    i18n::t("Calculated items · click to change"),
                    cx,
                ))
                .child(existing)
            })
            .when(item_mode, |d| {
                d.child(
                    h_flex().child(
                        Button::new("pivot-calc-item-delete")
                            .xsmall()
                            .ghost()
                            .icon(gpui_kit::assets::IconName::Trash)
                            .label(i18n::t("Delete"))
                            .disabled(!deletable)
                            .on_click(move |_, w, cx| {
                                let _ = del.update(cx, |this, cx| this.delete_calc_item(w, cx));
                            }),
                    ),
                )
            })
            .when_some(c.error.clone(), |d, e| {
                d.child(
                    div()
                        .text_size(FONT_SMALL)
                        .text_color(t.danger)
                        .child(SharedString::from(e)),
                )
            })
            .child(card_buttons("pivot-calc", ok, cancel, |this, w, cx| {
                this.apply_calc_card(w, cx)
            }))
    }

    // ---------------------------------------------------------- styles

    pub(crate) fn render_pivot_styles(
        &self,
        l: &PivotLayout,
        locked: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let view = cx.entity().downgrade();
        let opts = l.style.clone();
        let current = opts.name.clone();
        let toggle = view.clone();
        let label: SharedString = match &current {
            Some(n) => style_label(n).into(),
            None => i18n::t("None").into(),
        };
        let option = |id: &'static str,
                      label: &'static str,
                      on: bool,
                      set: fn(&mut PivotStyleOptions, bool)| {
            let v = view.clone();
            Checkbox::new(id)
                .label(label)
                .checked(on)
                .disabled(locked)
                .on_click(move |on, w, cx| {
                    let on = *on;
                    let _ = v.update(cx, |this, cx| {
                        this.edit_layout(|l| set(&mut l.style, on), w, cx)
                    });
                })
        };
        v_flex()
            .gap(SPACE_3)
            .child(kantor_ui::components::section_label(
                i18n::t("PivotTable Style"),
                cx,
            ))
            .child(
                Button::new("pivot-style")
                    .outline()
                    .small()
                    .label(label)
                    .dropdown_caret(true)
                    .disabled(locked)
                    .on_click(move |_, _, cx| {
                        let _ = toggle.update(cx, |this, cx| {
                            if let Some(p) = this.pivot_panel.as_mut() {
                                p.styles_open = !p.styles_open;
                            }
                            cx.notify();
                        });
                    }),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(SPACE_2)
                    .child(option(
                        "pivot-style-rowh",
                        i18n::t("Row Headers"),
                        opts.row_headers,
                        |s, on| s.row_headers = on,
                    ))
                    .child(option(
                        "pivot-style-colh",
                        i18n::t("Column Headers"),
                        opts.column_headers,
                        |s, on| s.column_headers = on,
                    ))
                    .child(option(
                        "pivot-style-rowb",
                        i18n::t("Banded Rows"),
                        opts.banded_rows,
                        |s, on| s.banded_rows = on,
                    ))
                    .child(option(
                        "pivot-style-colb",
                        i18n::t("Banded Columns"),
                        opts.banded_columns,
                        |s, on| s.banded_columns = on,
                    )),
            )
    }

    /// The style gallery, as a card at the top of the panel.
    pub(crate) fn render_style_gallery(&self, l: &PivotLayout, cx: &mut Context<Self>) -> Div {
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let opts = l.style.clone();
        let current = opts.name.clone();
        let close = view.clone();
        let new_view = view.clone();
        let out = crate::pivot::card(cx).child(
            h_flex()
                .child(
                    div()
                        .flex_1()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(i18n::t("PivotTable Style")),
                )
                .child(
                    Button::new("pivot-style-new")
                        .ghost()
                        .xsmall()
                        .icon(gpui_kit::assets::IconName::Plus)
                        .label(i18n::t("New PivotTable Style…"))
                        .on_click(move |_, w, cx| {
                            let _ = new_view.update(cx, |this, cx| this.new_pivot_style_doc(w, cx));
                        }),
                )
                .child(
                    Button::new("pivot-styles-close")
                        .ghost()
                        .xsmall()
                        .label(i18n::t("Close"))
                        .on_click(move |_, _, cx| {
                            let _ = close.update(cx, |this, cx| {
                                if let Some(p) = this.pivot_panel.as_mut() {
                                    p.styles_open = false;
                                }
                                cx.notify();
                            });
                        }),
                ),
        );
        let book = &self.table.read(cx).delegate().book;
        let mut grid = h_flex().flex_wrap().gap(SPACE_2);
        let none_view = view.clone();
        grid = grid.child(
            div()
                .id("pivot-style-none")
                .p(px(3.))
                .rounded(RADIUS_SM)
                .border_2()
                .border_color(if current.is_none() {
                    t.accent
                } else {
                    gpui::transparent_black()
                })
                .hover(|s| s.bg(t.hover))
                .cursor_pointer()
                .child(
                    div()
                        .w(px(40.))
                        .h(px(47.))
                        .border_1()
                        .border_color(t.border)
                        .bg(gpui::white())
                        .text_size(FONT_CAPTION)
                        .text_color(t.fg_dim)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(i18n::t("None")),
                )
                .on_click(move |_, w, cx| {
                    let _ = none_view.update(cx, |this, cx| {
                        this.edit_layout(|l| l.style.name = None, w, cx)
                    });
                }),
        );
        let custom = book.custom_pivot_styles();
        let names = custom.iter().cloned().chain(pivot_style_names());
        for (i, name) in names.enumerate() {
            let Some(cells) = book.pivot_style_preview(&name, &opts) else {
                continue;
            };
            let is_custom = custom.contains(&name);
            let (menu_name, mv) = (name.clone(), view.clone());
            let selected = current.as_deref() == Some(name.as_str());
            let v = view.clone();
            let tip: SharedString = style_label(&name).into();
            let mut pic = v_flex().gap(px(1.));
            for r in &cells {
                let mut line = h_flex().gap(px(1.));
                for c in r {
                    line = line.child(
                        div()
                            .w(px(9.))
                            .h(px(5.))
                            .bg(c.fill.map(crate::look::hsla).unwrap_or(gpui::white()))
                            .when_some(c.top.as_ref().and_then(|b| b.color), |d, col| {
                                d.border_t_1().border_color(crate::look::hsla(col))
                            })
                            .when_some(c.bottom.as_ref().and_then(|b| b.color), |d, col| {
                                d.border_b_1().border_color(crate::look::hsla(col))
                            }),
                    );
                }
                pic = pic.child(line);
            }
            let n2 = name.clone();
            grid = grid.child(
                div()
                    .id(("pivot-style", i))
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
                        gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                    })
                    .child(pic)
                    .on_click(move |_, w, cx| {
                        let n = n2.clone();
                        let _ = v.update(cx, |this, cx| {
                            this.edit_layout(|l| l.style.name = Some(n), w, cx)
                        });
                    })
                    .context_menu(move |menu, _, _| {
                        crate::table_style_ui::style_menu_for(
                            menu, &menu_name, is_custom, true, &mv,
                        )
                    }),
            );
        }
        out.child(
            div()
                .id("pivot-styles")
                .max_h(px(300.))
                .overflow_y_scroll()
                .p(SPACE_2)
                .rounded(RADIUS_SM)
                .border_1()
                .border_color(t.border)
                .bg(t.bg)
                .child(grid),
        )
    }
}

/// `PivotStyleMedium9` → `Medium 9`.
pub(crate) fn style_label(name: &str) -> String {
    for (family, label) in [
        ("Light", tk("Light")),
        ("Medium", tk("Medium")),
        ("Dark", tk("Dark")),
    ] {
        if let Some(n) = name.strip_prefix(&format!("PivotStyle{family}")) {
            return format!("{} {n}", i18n::t(label));
        }
    }
    name.to_owned()
}

/// Cancel and Apply buttons for a card.
fn card_buttons(
    id: &'static str,
    ok: WeakEntity<SheetsView>,
    cancel: WeakEntity<SheetsView>,
    apply: fn(&mut SheetsView, &mut Window, &mut Context<SheetsView>),
) -> Div {
    h_flex()
        .justify_end()
        .gap(SPACE_3)
        .child(
            Button::new(SharedString::from(format!("{id}-cancel")))
                .ghost()
                .small()
                .label(i18n::t("Cancel"))
                .on_click(move |_, _, cx| {
                    let _ = cancel.update(cx, |this, cx| {
                        this.close_pivot_cards();
                        cx.notify();
                    });
                }),
        )
        .child(
            Button::new(SharedString::from(format!("{id}-ok")))
                .primary()
                .small()
                .label(i18n::t("Apply"))
                .on_click(move |_, w, cx| {
                    let _ = ok.update(cx, |this, cx| apply(this, w, cx));
                }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Engine names shown through `t_dyn`: Show Values As and the filter types
    /// (`sheets_engine::ShowAs::label`, `PIVOT_FILTER_TYPES`).
    const ENGINE_LABELS: &[&str] = &[
        tk("No Calculation"),
        tk("% of Grand Total"),
        tk("% of Column Total"),
        tk("% of Row Total"),
        tk("% Of"),
        tk("% of Parent Row Total"),
        tk("% of Parent Column Total"),
        tk("% of Parent Total"),
        tk("Difference From"),
        tk("% Difference From"),
        tk("Running Total In"),
        tk("% Running Total In"),
        tk("Rank Smallest to Largest"),
        tk("Rank Largest to Smallest"),
        tk("Index"),
        tk("Equals"),
        tk("Does Not Equal"),
        tk("Begins With"),
        tk("Ends With"),
        tk("Contains"),
        tk("Greater Than"),
        tk("Less Than"),
        tk("Between"),
        tk("Not Between"),
        tk("Top 10 (Items)"),
        tk("Top 10 (Percent)"),
        tk("Top 10 (Sum)"),
        tk("Before"),
        tk("After"),
        tk("Tomorrow"),
        tk("Today"),
        tk("Yesterday"),
        tk("Next Week"),
        tk("This Week"),
        tk("Last Week"),
        tk("Next Month"),
        tk("This Month"),
        tk("Last Month"),
        tk("Next Quarter"),
        tk("This Quarter"),
        tk("Last Quarter"),
        tk("Next Year"),
        tk("This Year"),
        tk("Last Year"),
        tk("Year to Date"),
        tk("Quarter 1"),
        tk("Quarter 2"),
        tk("Quarter 3"),
        tk("Quarter 4"),
        tk("January"),
        tk("February"),
        tk("March"),
        tk("April"),
        tk("May"),
        tk("June"),
        tk("July"),
        tk("August"),
        tk("September"),
        tk("October"),
        tk("November"),
        tk("December"),
        tk("Does Not Begin With"),
        tk("Does Not End With"),
        tk("Does Not Contain"),
        tk("Greater Than Or Equal To"),
        tk("Less Than Or Equal To"),
        tk("Before Or Equal To"),
        tk("After Or Equal To"),
    ];

    #[std::prelude::v1::test]
    fn dates_read_and_shown() {
        assert_eq!(iso(46027.0), "2026-01-05");
        assert_eq!(serial_of("2026-01-05"), Some(46027.0));
        assert_eq!(serial_of("1/5/2026"), Some(46027.0));
        assert_eq!(serial_of("nope"), None);
    }

    /// Names from `sheets_engine` shown through `t_dyn`.
    #[std::prelude::v1::test]
    fn engine_names_are_translated() {
        for s in LEVEL_LABELS {
            assert!(i18n::indonesian(s).is_some(), "{s}");
        }
        for s in sheets_engine::ShowAs::ALL {
            assert!(ENGINE_LABELS.contains(&s.label()), "{}", s.label());
        }
        for (_, label, _) in PIVOT_FILTER_TYPES {
            assert!(ENGINE_LABELS.contains(label), "{label}");
        }
        for s in ENGINE_LABELS {
            assert!(i18n::indonesian(s).is_some(), "{s}");
        }
    }
}
