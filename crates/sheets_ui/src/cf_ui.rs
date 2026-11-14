//! Conditional Formatting in the Sheets view: the toolbar menu (Highlight
//! Cells Rules, Top/Bottom Rules, Data Bars, Color Scales, Icon Sets, New
//! Rule…, Clear Rules, Manage Rules…), the rule card (new or edit, any rule
//! type, format presets or a custom fill/font) and the Manage Rules card
//! (rules of the selection or the sheet: edit, delete, move up/down, stop if
//! true). Rules are evaluated by `sheets_engine::cond_format` and changed
//! through `sheets_engine::cf_edit`, one undo step each.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    Disableable as _, Sizable as _,
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
use sheets_engine::{
    COLOR_SCALES, CfCompare, CfFormat, CfPeriod, CfRuleDef, CfSpec, CfTextOp, DATA_BAR_COLORS,
    ICON_SETS, sqref_of,
};

use crate::SheetsView;
use crate::dialog::{self, choices, note, row};

actions!(sheets, [NewConditionalRule, ManageConditionalRules]);

/// The rule types of New Formatting Rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    CellValue,
    Text,
    Dates,
    Duplicates,
    TopBottom,
    Average,
    Blanks,
    Formula,
    DataBar,
    ColorScale,
    IconSet,
}

impl Kind {
    const ALL: [Kind; 11] = [
        Kind::CellValue,
        Kind::Text,
        Kind::Dates,
        Kind::Duplicates,
        Kind::TopBottom,
        Kind::Average,
        Kind::Blanks,
        Kind::Formula,
        Kind::DataBar,
        Kind::ColorScale,
        Kind::IconSet,
    ];

    fn label(self) -> &'static str {
        match self {
            Kind::CellValue => t("Cell value"),
            Kind::Text => t("Specific text"),
            Kind::Dates => t("Dates occurring"),
            Kind::Duplicates => t("Duplicate or unique values"),
            Kind::TopBottom => t("Top or bottom ranked values"),
            Kind::Average => t("Above or below average"),
            Kind::Blanks => t("Blanks"),
            Kind::Formula => t("Use a formula"),
            Kind::DataBar => t("Data bar"),
            Kind::ColorScale => t("Color scale"),
            Kind::IconSet => t("Icon set"),
        }
    }

    fn has_format(self) -> bool {
        !matches!(self, Kind::DataBar | Kind::ColorScale | Kind::IconSet)
    }
}

/// Fill and font colours offered for a custom format.
const SWATCHES: [u32; 12] = [
    0xFFC7CE, 0xFFEB9C, 0xC6EFCE, 0xDDEBF7, 0xFCE4D6, 0xEDEDED, 0x9C0006, 0x9C5700, 0x006100,
    0x1F4E78, 0xC00000, 0x000000,
];

/// The new / edit rule card.
pub(crate) struct CfDialog {
    sheet: u32,
    /// Editing the rule stored at this index (else a new rule).
    edit: Option<usize>,
    kind: Kind,
    compare: CfCompare,
    text_op: CfTextOp,
    period: CfPeriod,
    unique: bool,
    bottom: bool,
    percent: bool,
    below: bool,
    not_blank: bool,
    bar_color: u32,
    gradient: bool,
    scale: usize,
    icon_set: usize,
    reverse: bool,
    icons_only: bool,
    format: CfFormat,
    stop: bool,
    range: Entity<InputState>,
    value: Entity<InputState>,
    value2: Entity<InputState>,
    text: Entity<InputState>,
    rank: Entity<InputState>,
    formula: Entity<InputState>,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

/// The Manage Rules card.
pub(crate) struct CfManager {
    sheet: u32,
    /// Rules of the whole sheet, else of the selection.
    whole_sheet: bool,
    error: Option<String>,
}

type Pick = (Kind, Box<dyn Fn(&mut CfDialog)>);
/// A dropdown entry: label, checked, what it sets.
type MenuChoice = (String, bool, Box<dyn Fn(&mut CfDialog)>);

impl SheetsView {
    fn selection_sqref(&self, cx: &App) -> (u32, String) {
        let grid = self.table.read(cx).delegate();
        (grid.sheet, sqref_of(&[grid.range()]))
    }

    /// Open the rule card for a new rule of `kind` (`set` adjusts the
    /// defaults: Greater Than, Top 10%...).
    pub(crate) fn open_cf_dialog(
        &mut self,
        kind: Kind,
        set: impl FnOnce(&mut CfDialog),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let (sheet, range) = self.selection_sqref(cx);
        let mut d = self.new_cf_dialog(sheet, kind, &range, window, cx);
        set(&mut d);
        d.value.update(cx, |i, cx| i.focus(window, cx));
        if kind == Kind::Formula {
            d.formula.update(cx, |i, cx| i.focus(window, cx));
        } else if kind == Kind::Text {
            d.text.update(cx, |i, cx| i.focus(window, cx));
        }
        self.cf_dialog = Some(d);
        cx.notify();
    }

    fn new_cf_dialog(
        &mut self,
        sheet: u32,
        kind: Kind,
        range_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> CfDialog {
        let mk = |window: &mut Window, cx: &mut Context<Self>, v: &str, ph: &str| {
            let e = cx.new(|cx| InputState::new(window, cx).placeholder(ph.to_owned()));
            e.update(cx, |i, cx| i.set_value(v.to_owned(), window, cx));
            e
        };
        let range = mk(window, cx, range_text, t("e.g. A2:A20"));
        let value = mk(window, cx, "", t("a number, text or =formula"));
        let value2 = mk(window, cx, "", t("and"));
        let text = mk(window, cx, "", t("text to look for"));
        let rank = mk(window, cx, "10", "");
        let formula = mk(window, cx, "", t("e.g. =$B2>100"));
        let subs = [&range, &value, &value2, &text, &rank, &formula]
            .into_iter()
            .map(|e| {
                cx.subscribe_in(e, window, |this, _, ev: &InputEvent, w, cx| match ev {
                    InputEvent::Change => {
                        if let Some(d) = this.cf_dialog.as_mut() {
                            d.error = None;
                        }
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => this.cf_dialog_ok(w, cx),
                    _ => {}
                })
            })
            .collect();
        CfDialog {
            sheet,
            edit: None,
            kind,
            compare: CfCompare::GreaterThan,
            text_op: CfTextOp::Contains,
            period: CfPeriod::Today,
            unique: false,
            bottom: false,
            percent: false,
            below: false,
            not_blank: false,
            bar_color: DATA_BAR_COLORS[0].1,
            gradient: true,
            scale: 0,
            icon_set: 0,
            reverse: false,
            icons_only: false,
            format: CfFormat::PRESETS[0].1,
            stop: false,
            range,
            value,
            value2,
            text,
            rank,
            formula,
            error: None,
            _subs: subs,
        }
    }

    /// Edit the rule stored at `index` of `sheet` (Manage Rules).
    fn edit_cf_rule(
        &mut self,
        sheet: u32,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let info = {
            let grid = self.table.read(cx).delegate();
            grid.book
                .cf_rules(sheet, None)
                .into_iter()
                .find(|r| r.index == index)
        };
        let Some(info) = info else { return };
        let Some(def) = info.def.clone() else {
            if let Some(m) = self.cf_manager.as_mut() {
                m.error = Some(
                    t("Kantor can't edit this kind of rule yet (it can delete or move it)").into(),
                );
            }
            cx.notify();
            return;
        };
        let mut d = self.new_cf_dialog(sheet, Kind::CellValue, &info.range, window, cx);
        d.edit = Some(index);
        d.format = def.format;
        d.stop = def.stop_if_true;
        let set = |e: &Entity<InputState>, v: &str, window: &mut Window, cx: &mut Context<Self>| {
            e.update(cx, |i, cx| i.set_value(v.to_owned(), window, cx));
        };
        match &def.spec {
            CfSpec::CellValue { op, value, value2 } => {
                d.kind = Kind::CellValue;
                d.compare = *op;
                set(&d.value, value, window, cx);
                set(&d.value2, value2, window, cx);
            }
            CfSpec::Text { op, text } => {
                d.kind = Kind::Text;
                d.text_op = *op;
                set(&d.text, text, window, cx);
            }
            CfSpec::Dates { period } => {
                d.kind = Kind::Dates;
                d.period = *period;
            }
            CfSpec::Duplicates { unique } => {
                d.kind = Kind::Duplicates;
                d.unique = *unique;
            }
            CfSpec::TopBottom {
                bottom,
                rank,
                percent,
            } => {
                d.kind = Kind::TopBottom;
                d.bottom = *bottom;
                d.percent = *percent;
                set(&d.rank, &rank.to_string(), window, cx);
            }
            CfSpec::Average { below } => {
                d.kind = Kind::Average;
                d.below = *below;
            }
            CfSpec::Blanks { not } => {
                d.kind = Kind::Blanks;
                d.not_blank = *not;
            }
            CfSpec::Formula { formula } => {
                d.kind = Kind::Formula;
                set(&d.formula, formula, window, cx);
            }
            CfSpec::DataBar { color, gradient } => {
                d.kind = Kind::DataBar;
                d.bar_color = *color;
                d.gradient = *gradient;
            }
            CfSpec::ColorScale { colors } => {
                d.kind = Kind::ColorScale;
                d.scale = COLOR_SCALES
                    .iter()
                    .position(|(_, c)| *c == colors.as_slice())
                    .unwrap_or(0);
            }
            CfSpec::IconSet {
                set: s,
                reverse,
                show_value,
            } => {
                d.kind = Kind::IconSet;
                d.icon_set = ICON_SETS.iter().position(|(_, n)| n == s).unwrap_or(0);
                d.reverse = *reverse;
                d.icons_only = !show_value;
            }
        }
        self.cf_dialog = Some(d);
        cx.notify();
    }

    fn close_cf_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cf_dialog.take().is_some() {
            if self.cf_manager.is_none() {
                self.focus_grid(window, cx);
            }
            cx.notify();
        }
    }

    fn cf_spec(d: &CfDialog, cx: &App) -> Result<CfSpec, String> {
        let text = |e: &Entity<InputState>| e.read(cx).value().to_string();
        Ok(match d.kind {
            Kind::CellValue => CfSpec::CellValue {
                op: d.compare,
                value: text(&d.value),
                value2: text(&d.value2),
            },
            Kind::Text => CfSpec::Text {
                op: d.text_op,
                text: text(&d.text),
            },
            Kind::Dates => CfSpec::Dates { period: d.period },
            Kind::Duplicates => CfSpec::Duplicates { unique: d.unique },
            Kind::TopBottom => CfSpec::TopBottom {
                bottom: d.bottom,
                rank: text(&d.rank)
                    .trim()
                    .parse()
                    .map_err(|_| t("Rank: type a whole number").to_owned())?,
                percent: d.percent,
            },
            Kind::Average => CfSpec::Average { below: d.below },
            Kind::Blanks => CfSpec::Blanks { not: d.not_blank },
            Kind::Formula => CfSpec::Formula {
                formula: text(&d.formula),
            },
            Kind::DataBar => CfSpec::DataBar {
                color: d.bar_color,
                gradient: d.gradient,
            },
            Kind::ColorScale => CfSpec::ColorScale {
                colors: COLOR_SCALES[d.scale].1.to_vec(),
            },
            Kind::IconSet => CfSpec::IconSet {
                set: ICON_SETS[d.icon_set].1.to_owned(),
                reverse: d.reverse,
                show_value: !d.icons_only,
            },
        })
    }

    fn cf_dialog_ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.cf_dialog.as_ref() else {
            return;
        };
        let spec = Self::cf_spec(d, cx);
        let range = d.range.read(cx).value().to_string();
        let (sheet, edit) = (d.sheet, d.edit);
        let def = spec.map(|spec| CfRuleDef {
            format: if d.kind.has_format() {
                d.format
            } else {
                CfFormat::default()
            },
            stop_if_true: d.stop && d.kind.has_format(),
            spec,
        });
        let res = def.and_then(|def| {
            self.table.update(cx, |table, cx| {
                let grid = table.delegate_mut();
                let r = match edit {
                    Some(i) => grid.book.update_cf_rule(sheet, i, &range, &def),
                    None => grid.book.add_cf_rule(sheet, &range, &def),
                };
                table.refresh(cx);
                r.map_err(|e| error_text(&e))
            })
        });
        match res {
            Ok(()) => {
                self.status = if edit.is_some() {
                    t("Rule changed").into()
                } else {
                    t("Conditional formatting rule added").into()
                };
                self.close_cf_dialog(window, cx);
                self.update_title(window, cx);
            }
            Err(e) => {
                if let Some(d) = self.cf_dialog.as_mut() {
                    d.error = Some(e);
                }
                cx.notify();
            }
        }
    }

    /// Add a rule without a card (data bars, colour scales, icon sets).
    fn quick_cf(&mut self, spec: CfSpec, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let (sheet, range) = self.selection_sqref(cx);
        let def = CfRuleDef {
            spec,
            format: CfFormat::default(),
            stop_if_true: false,
        };
        let res = self.table.update(cx, |table, cx| {
            let r = table.delegate_mut().book.add_cf_rule(sheet, &range, &def);
            table.refresh(cx);
            r
        });
        self.status = match res {
            Ok(()) => tf("Conditional formatting added to {}", &[&range]).into(),
            Err(e) => error_text(&e).into(),
        };
        self.update_title(window, cx);
        cx.notify();
    }

    /// Clear Rules from the selection, or from the whole sheet.
    pub fn clear_cf_doc(&mut self, whole_sheet: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let res = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let within = (!whole_sheet).then(|| grid.range());
            let sheet = grid.sheet;
            let r = grid.book.clear_cf_rules(sheet, within);
            table.refresh(cx);
            r
        });
        self.status = match res {
            Ok(0) => t("No conditional formatting rules here").into(),
            Ok(1) => t("1 rule cleared").into(),
            Ok(n) => tf("{} rules cleared", &[&n]).into(),
            Err(e) => error_text(&e).into(),
        };
        self.update_title(window, cx);
        cx.notify();
    }

    /// Open Manage Rules for the selection.
    pub fn manage_cf_doc(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let sheet = self.table.read(cx).delegate().sheet;
        self.cf_manager = Some(CfManager {
            sheet,
            whole_sheet: false,
            error: None,
        });
        cx.notify();
    }

    /// New Rule… for the selection.
    pub fn new_cf_rule_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_cf_dialog(Kind::CellValue, |_| {}, window, cx);
    }

    fn manage_cf(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut sheets_engine::Workbook, u32) -> anyhow::Result<()>,
    ) {
        let Some(sheet) = self.cf_manager.as_ref().map(|m| m.sheet) else {
            return;
        };
        let res = self.table.update(cx, |table, cx| {
            let r = f(&mut table.delegate_mut().book, sheet);
            table.refresh(cx);
            r
        });
        if let Some(m) = self.cf_manager.as_mut() {
            m.error = res.err().map(|e| error_text(&e));
        }
        self.update_title(window, cx);
        cx.notify();
    }

    pub(crate) fn render_cf_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.cf_dialog.as_ref()?;
        let view = cx.entity().downgrade();
        let kind_view = view.clone();
        let current = d.kind;
        let kind = Button::new("cf-kind")
            .outline()
            .small()
            .label(current.label())
            .dropdown_caret(true)
            .dropdown_menu(move |mut m, _, _| {
                for k in Kind::ALL {
                    let v = kind_view.clone();
                    m = m.item(
                        PopupMenuItem::new(k.label())
                            .checked(k == current)
                            .on_click(move |_, _, cx| {
                                let _ = v.update(cx, |this, cx| {
                                    if let Some(d) = this.cf_dialog.as_mut() {
                                        d.kind = k;
                                    }
                                    cx.notify();
                                });
                            }),
                    );
                }
                m
            });
        let title = if d.edit.is_some() {
            t("Edit Formatting Rule")
        } else {
            t("New Formatting Rule")
        };
        let mut card = dialog::card("cf-rule", title, px(470.), cx)
            .key_context("KantorCfDialog")
            .on_action(cx.listener(|this, _: &input::Escape, w, cx| this.close_cf_dialog(w, cx)))
            .child(row(t("Applies to"), Input::new(&d.range).small(), cx))
            .child(row(t("Rule type"), kind, cx))
            .child(self.render_cf_kind(d, &view, cx));
        if d.kind.has_format() {
            card = card.child(self.render_cf_format(d, &view, cx));
        }
        card = card
            .children(d.error.clone().map(|e| note(e, true, cx)))
            .child(dialog::buttons(
                "cf-buttons",
                t("OK"),
                true,
                &view,
                Self::cf_dialog_ok,
                Self::close_cf_dialog,
            ));
        Some(dialog::place(card).into_any_element())
    }

    fn render_cf_kind(&self, d: &CfDialog, view: &WeakEntity<Self>, cx: &mut Context<Self>) -> Div {
        let menu_button = |id: &'static str, label: String, items: Vec<MenuChoice>| {
            let v = view.clone();
            let items = std::rc::Rc::new(items);
            Button::new(id)
                .outline()
                .small()
                .label(label)
                .dropdown_caret(true)
                .dropdown_menu(move |mut m, _, _| {
                    for (i, (label, checked, _)) in items.iter().enumerate() {
                        let v = v.clone();
                        let items = items.clone();
                        m = m.item(
                            PopupMenuItem::new(label.clone())
                                .checked(*checked)
                                .on_click(move |_, _, cx| {
                                    let items = items.clone();
                                    let _ = v.update(cx, |this, cx| {
                                        if let Some(d) = this.cf_dialog.as_mut() {
                                            (items[i].2)(d);
                                        }
                                        cx.notify();
                                    });
                                }),
                        );
                    }
                    m
                })
        };
        let mut out = v_flex().gap(SPACE_4);
        match d.kind {
            Kind::CellValue => {
                let items = CfCompare::ALL
                    .into_iter()
                    .map(|c| {
                        (
                            t_dyn(c.label()).to_owned(),
                            c == d.compare,
                            Box::new(move |d: &mut CfDialog| d.compare = c)
                                as Box<dyn Fn(&mut CfDialog)>,
                        )
                    })
                    .collect();
                out = out.child(row(
                    t("Cell value is"),
                    menu_button("cf-compare", t_dyn(d.compare.label()).to_owned(), items),
                    cx,
                ));
                out = out.child(row(t("Value"), Input::new(&d.value).small(), cx));
                if d.compare.two_values() {
                    out = out.child(row(t("And"), Input::new(&d.value2).small(), cx));
                }
            }
            Kind::Text => {
                let items = CfTextOp::ALL
                    .into_iter()
                    .map(|c| {
                        (
                            t_dyn(c.label()).to_owned(),
                            c == d.text_op,
                            Box::new(move |d: &mut CfDialog| d.text_op = c)
                                as Box<dyn Fn(&mut CfDialog)>,
                        )
                    })
                    .collect();
                out = out
                    .child(row(
                        t("Text"),
                        menu_button("cf-textop", t_dyn(d.text_op.label()).to_owned(), items),
                        cx,
                    ))
                    .child(row(t("Looking for"), Input::new(&d.text).small(), cx));
            }
            Kind::Dates => {
                let items = CfPeriod::ALL
                    .into_iter()
                    .map(|p| {
                        (
                            t_dyn(p.label()).to_owned(),
                            p == d.period,
                            Box::new(move |d: &mut CfDialog| d.period = p)
                                as Box<dyn Fn(&mut CfDialog)>,
                        )
                    })
                    .collect();
                out = out.child(row(
                    t("A date occurring"),
                    menu_button("cf-period", t_dyn(d.period.label()).to_owned(), items),
                    cx,
                ));
            }
            Kind::Duplicates => {
                out = out.child(row(
                    t("Format"),
                    choices(
                        "cf-dup",
                        &[(false, t("Duplicate values")), (true, t("Unique values"))],
                        d.unique,
                        view,
                        |this, v| {
                            if let Some(d) = this.cf_dialog.as_mut() {
                                d.unique = v;
                            }
                        },
                    ),
                    cx,
                ));
            }
            Kind::TopBottom => {
                let pv = view.clone();
                out = out
                    .child(row(
                        t("Format the"),
                        choices(
                            "cf-top",
                            &[(false, t("Top")), (true, t("Bottom"))],
                            d.bottom,
                            view,
                            |this, v| {
                                if let Some(d) = this.cf_dialog.as_mut() {
                                    d.bottom = v;
                                }
                            },
                        ),
                        cx,
                    ))
                    .child(row(
                        t("Number"),
                        h_flex()
                            .gap(SPACE_4)
                            .child(div().w(px(80.)).child(Input::new(&d.rank).small()))
                            .child(
                                Checkbox::new("cf-percent")
                                    .label(t("% of the selected range"))
                                    .checked(d.percent)
                                    .on_click(move |on, _, cx| {
                                        let on = *on;
                                        let _ = pv.update(cx, |this, cx| {
                                            if let Some(d) = this.cf_dialog.as_mut() {
                                                d.percent = on;
                                            }
                                            cx.notify();
                                        });
                                    }),
                            ),
                        cx,
                    ));
            }
            Kind::Average => {
                out = out.child(row(
                    t("Values"),
                    choices(
                        "cf-avg",
                        &[(false, t("Above average")), (true, t("Below average"))],
                        d.below,
                        view,
                        |this, v| {
                            if let Some(d) = this.cf_dialog.as_mut() {
                                d.below = v;
                            }
                        },
                    ),
                    cx,
                ));
            }
            Kind::Blanks => {
                out = out.child(row(
                    t("Cells that are"),
                    choices(
                        "cf-blank",
                        &[(false, t("Blank")), (true, t("Not blank"))],
                        d.not_blank,
                        view,
                        |this, v| {
                            if let Some(d) = this.cf_dialog.as_mut() {
                                d.not_blank = v;
                            }
                        },
                    ),
                    cx,
                ));
            }
            Kind::Formula => {
                out = out
                    .child(row(t("Format where"), Input::new(&d.formula).small(), cx))
                    .child(note(
                        t("The formula is true for the cells to format; references are relative to the first cell of the range."),
                        false,
                        cx,
                    ));
            }
            Kind::DataBar => {
                let items = DATA_BAR_COLORS
                    .iter()
                    .map(|(n, c)| {
                        let c = *c;
                        (
                            t(n).to_owned(),
                            c == d.bar_color,
                            Box::new(move |d: &mut CfDialog| d.bar_color = c)
                                as Box<dyn Fn(&mut CfDialog)>,
                        )
                    })
                    .collect();
                let name = DATA_BAR_COLORS
                    .iter()
                    .find(|(_, c)| *c == d.bar_color)
                    .map(|(n, _)| t(n).to_owned())
                    .unwrap_or_else(|| format!("#{:06X}", d.bar_color));
                out = out
                    .child(row(t("Bar colour"), menu_button("cf-bar", name, items), cx))
                    .child(row(
                        t("Fill"),
                        choices(
                            "cf-grad",
                            &[(true, t("Gradient")), (false, t("Solid"))],
                            d.gradient,
                            view,
                            |this, v| {
                                if let Some(d) = this.cf_dialog.as_mut() {
                                    d.gradient = v;
                                }
                            },
                        ),
                        cx,
                    ));
            }
            Kind::ColorScale => {
                let items = COLOR_SCALES
                    .iter()
                    .enumerate()
                    .map(|(i, (n, _))| {
                        (
                            t(n).to_owned(),
                            i == d.scale,
                            Box::new(move |d: &mut CfDialog| d.scale = i)
                                as Box<dyn Fn(&mut CfDialog)>,
                        )
                    })
                    .collect();
                out = out.child(row(
                    t("Colours"),
                    h_flex()
                        .gap(SPACE_4)
                        .child(menu_button(
                            "cf-scale",
                            t(COLOR_SCALES[d.scale].0).to_owned(),
                            items,
                        ))
                        .child(scale_strip(COLOR_SCALES[d.scale].1)),
                    cx,
                ));
            }
            Kind::IconSet => {
                let items = ICON_SETS
                    .iter()
                    .enumerate()
                    .map(|(i, (n, _))| {
                        (
                            t(n).to_owned(),
                            i == d.icon_set,
                            Box::new(move |d: &mut CfDialog| d.icon_set = i)
                                as Box<dyn Fn(&mut CfDialog)>,
                        )
                    })
                    .collect();
                let rv = view.clone();
                let ov = view.clone();
                out = out
                    .child(row(
                        t("Icons"),
                        menu_button("cf-icons", t(ICON_SETS[d.icon_set].0).to_owned(), items),
                        cx,
                    ))
                    .child(row(
                        "",
                        h_flex()
                            .gap(SPACE_6)
                            .child(
                                Checkbox::new("cf-reverse")
                                    .label(t("Reverse icon order"))
                                    .checked(d.reverse)
                                    .on_click(move |on, _, cx| {
                                        let on = *on;
                                        let _ = rv.update(cx, |this, cx| {
                                            if let Some(d) = this.cf_dialog.as_mut() {
                                                d.reverse = on;
                                            }
                                            cx.notify();
                                        });
                                    }),
                            )
                            .child(
                                Checkbox::new("cf-icons-only")
                                    .label(t("Show icon only"))
                                    .checked(d.icons_only)
                                    .on_click(move |on, _, cx| {
                                        let on = *on;
                                        let _ = ov.update(cx, |this, cx| {
                                            if let Some(d) = this.cf_dialog.as_mut() {
                                                d.icons_only = on;
                                            }
                                            cx.notify();
                                        });
                                    }),
                            ),
                        cx,
                    ))
                    .child(note(
                        t("Thresholds split the range in equal percentages, as Excel does."),
                        false,
                        cx,
                    ));
            }
        }
        out
    }

    fn render_cf_format(
        &self,
        d: &CfDialog,
        view: &WeakEntity<Self>,
        cx: &mut Context<Self>,
    ) -> Div {
        let pv = view.clone();
        let current = d.format;
        let preset_label = CfFormat::PRESETS
            .iter()
            .find(|(_, f)| *f == current)
            .map(|(n, _)| t(n).to_owned())
            .unwrap_or_else(|| t("Custom format").to_owned());
        let presets = Button::new("cf-preset")
            .outline()
            .small()
            .label(preset_label)
            .dropdown_caret(true)
            .dropdown_menu(move |mut m, _, _| {
                for (name, f) in CfFormat::PRESETS {
                    let v = pv.clone();
                    m = m.item(PopupMenuItem::new(t(name)).checked(f == current).on_click(
                        move |_, _, cx| {
                            let _ = v.update(cx, |this, cx| {
                                if let Some(d) = this.cf_dialog.as_mut() {
                                    d.format = f;
                                }
                                cx.notify();
                            });
                        },
                    ));
                }
                m
            });
        let swatch_row = |id: &'static str, font: bool| {
            let mut r = h_flex().gap(SPACE_1).flex_wrap();
            let cur = if font { current.font } else { current.fill };
            for (i, c) in std::iter::once(None)
                .chain(SWATCHES.iter().map(|c| Some(*c)))
                .enumerate()
            {
                let v = view.clone();
                let t = tokens(cx);
                r = r.child(
                    div()
                        .id((id, i))
                        .size(px(18.))
                        .rounded(RADIUS_SM)
                        .border_2()
                        .border_color(if cur == c { t.accent } else { t.border })
                        .bg(c.map(crate::look::hsla).unwrap_or(t.surface))
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
                                if let Some(d) = this.cf_dialog.as_mut() {
                                    if font {
                                        d.format.font = c;
                                    } else {
                                        d.format.fill = c;
                                    }
                                }
                                cx.notify();
                            });
                        }),
                );
            }
            r
        };
        let check =
            |id: &'static str, label: &'static str, on: bool, set: fn(&mut CfFormat, bool)| {
                let v = view.clone();
                Checkbox::new(id)
                    .label(label)
                    .checked(on)
                    .on_click(move |on, _, cx| {
                        let on = *on;
                        let _ = v.update(cx, |this, cx| {
                            if let Some(d) = this.cf_dialog.as_mut() {
                                set(&mut d.format, on);
                            }
                            cx.notify();
                        });
                    })
            };
        let sv = view.clone();
        v_flex()
            .gap(SPACE_3)
            .child(kantor_ui::components::section_label(t("Format"), cx))
            .child(h_flex().gap(SPACE_4).child(presets).child(format_chip(
                &current,
                "AaBbCc 123",
                cx,
            )))
            .child(row(t("Fill"), swatch_row("cf-fill", false), cx))
            .child(row(t("Font colour"), swatch_row("cf-font", true), cx))
            .child(
                h_flex()
                    .gap(SPACE_6)
                    .child(check("cf-bold", t("Bold"), current.bold, |f, v| f.bold = v))
                    .child(check("cf-italic", t("Italic"), current.italic, |f, v| {
                        f.italic = v
                    }))
                    .child(check(
                        "cf-underline",
                        t("Underline"),
                        current.underline,
                        |f, v| f.underline = v,
                    ))
                    .child(check(
                        "cf-strike",
                        t("Strikethrough"),
                        current.strike,
                        |f, v| f.strike = v,
                    )),
            )
            .child(
                Checkbox::new("cf-stop")
                    .label(t("Stop if true (rules below don't apply to these cells)"))
                    .checked(d.stop)
                    .on_click(move |on, _, cx| {
                        let on = *on;
                        let _ = sv.update(cx, |this, cx| {
                            if let Some(d) = this.cf_dialog.as_mut() {
                                d.stop = on;
                            }
                            cx.notify();
                        });
                    }),
            )
    }

    pub(crate) fn render_cf_manager(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.cf_dialog.is_some() {
            return None;
        }
        let m = self.cf_manager.as_ref()?;
        let tok = tokens(cx);
        let view = cx.entity().downgrade();
        let (rules, sel) = {
            let grid = self.table.read(cx).delegate();
            let sel = grid.range();
            let within = (!m.whole_sheet).then_some(sel);
            (grid.book.cf_rules(m.sheet, within), sel)
        };
        let mut list = v_flex()
            .id("cf-list")
            .max_h(px(330.))
            .overflow_y_scroll()
            .rounded(RADIUS_SM)
            .border_1()
            .border_color(tok.border)
            .bg(tok.bg);
        if rules.is_empty() {
            list = list.child(div().p(SPACE_5).child(note(
                if m.whole_sheet {
                    t("This sheet has no conditional formatting rules.")
                } else {
                    t("No rules apply to the selected cells.")
                },
                false,
                cx,
            )));
        }
        let n = rules.len();
        for (k, r) in rules.into_iter().enumerate() {
            let idx = r.index;
            let (ev, dv, uv, wv, sv) = (
                view.clone(),
                view.clone(),
                view.clone(),
                view.clone(),
                view.clone(),
            );
            let def = r.def.clone();
            let range = r.range.clone();
            list = list.child(
                h_flex()
                    .px(SPACE_4)
                    .py(SPACE_3)
                    .gap(SPACE_4)
                    .when(k > 0, |d| d.border_t_1().border_color(tok.border))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(SPACE_1)
                            .child(div().truncate().child(SharedString::from(summary_text(&r))))
                            .child(
                                div()
                                    .text_size(FONT_SMALL)
                                    .text_color(tok.fg_muted)
                                    .child(SharedString::from(tf("Applies to {}", &[&r.range]))),
                            ),
                    )
                    .child(format_chip(&r.format, "AaBb", cx))
                    .child(
                        Checkbox::new(("cf-row-stop", k))
                            .label(t("Stop"))
                            .checked(r.stop_if_true)
                            .disabled(def.as_ref().is_none_or(|d| !d.spec.has_format()))
                            .on_click(move |on, w, cx| {
                                let on = *on;
                                let def = def.clone();
                                let range = range.clone();
                                let _ = sv.update(cx, |this, cx| {
                                    if let Some(mut def) = def {
                                        def.stop_if_true = on;
                                        this.manage_cf(w, cx, |b, s| {
                                            b.update_cf_rule(s, idx, &range, &def)
                                        });
                                    }
                                });
                            }),
                    )
                    .child(
                        Button::new(("cf-row-up", k))
                            .ghost()
                            .xsmall()
                            .icon(Lucide::ChevronUp)
                            .tooltip(t("Move up (applies sooner)"))
                            .disabled(k == 0)
                            .on_click(move |_, w, cx| {
                                let _ = uv.update(cx, |this, cx| {
                                    this.manage_cf(w, cx, |b, s| b.move_cf_rule(s, idx, true))
                                });
                            }),
                    )
                    .child(
                        Button::new(("cf-row-down", k))
                            .ghost()
                            .xsmall()
                            .icon(Lucide::ChevronDown)
                            .tooltip(t("Move down"))
                            .disabled(k + 1 == n)
                            .on_click(move |_, w, cx| {
                                let _ = wv.update(cx, |this, cx| {
                                    this.manage_cf(w, cx, |b, s| b.move_cf_rule(s, idx, false))
                                });
                            }),
                    )
                    .child(
                        Button::new(("cf-row-edit", k))
                            .ghost()
                            .xsmall()
                            .label(t("Edit"))
                            .on_click(move |_, w, cx| {
                                let _ = ev.update(cx, |this, cx| {
                                    let sheet = this.cf_manager.as_ref().map_or(0, |m| m.sheet);
                                    this.edit_cf_rule(sheet, idx, w, cx)
                                });
                            }),
                    )
                    .child(
                        Button::new(("cf-row-delete", k))
                            .ghost()
                            .xsmall()
                            .icon(Lucide::Trash)
                            .tooltip(t("Delete rule"))
                            .on_click(move |_, w, cx| {
                                let _ = dv.update(cx, |this, cx| {
                                    this.manage_cf(w, cx, |b, s| b.delete_cf_rule(s, idx))
                                });
                            }),
                    ),
            );
        }
        let nv = view.clone();
        let cv = view.clone();
        let card = dialog::card("cf-manage", t("Conditional Formatting Rules"), px(600.), cx)
            .key_context("KantorCfManager")
            .on_action(cx.listener(|this, _: &input::Escape, w, cx| {
                this.cf_manager = None;
                this.focus_grid(w, cx);
                cx.notify();
            }))
            .child(
                h_flex()
                    .gap(SPACE_4)
                    .child(note(t("Show rules for"), false, cx))
                    .child(choices(
                        "cf-scope",
                        &[(false, t("Current selection")), (true, t("This sheet"))],
                        m.whole_sheet,
                        &view,
                        |this, v| {
                            if let Some(m) = this.cf_manager.as_mut() {
                                m.whole_sheet = v;
                            }
                        },
                    ))
                    .child(div().flex_1())
                    .child(note(tf("Selection {}", &[&sel.name()]), false, cx)),
            )
            .child(note(
                t("The first rule applies first; a rule with Stop ends the list for its cells."),
                false,
                cx,
            ))
            .child(list)
            .children(m.error.clone().map(|e| note(e, true, cx)))
            .child(
                h_flex()
                    .gap(SPACE_3)
                    .child(
                        Button::new("cf-manage-new")
                            .small()
                            .outline()
                            .label(t("New Rule…"))
                            .on_click(move |_, w, cx| {
                                let _ = nv.update(cx, |this, cx| this.new_cf_rule_doc(w, cx));
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("cf-manage-close")
                            .small()
                            .primary()
                            .label(t("Close"))
                            .on_click(move |_, w, cx| {
                                let _ = cv.update(cx, |this, cx| {
                                    this.cf_manager = None;
                                    this.focus_grid(w, cx);
                                    cx.notify();
                                });
                            }),
                    ),
            );
        Some(dialog::place(card).into_any_element())
    }

    pub(crate) fn cf_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(
            cx.listener(|this, _: &NewConditionalRule, w, cx| this.new_cf_rule_doc(w, cx)),
        )
        .on_action(cx.listener(|this, _: &ManageConditionalRules, w, cx| this.manage_cf_doc(w, cx)))
    }
}

/// Strings from `sheets_engine::cf_edit` shown in the UI (labels, preset and
/// colour names, error messages); translated where shown with `t`/`t_dyn`.
#[cfg(test)]
const ENGINE_KEYS: &[&str] = &[
    tk("greater than"),
    tk("less than"),
    tk("between"),
    tk("not between"),
    tk("equal to"),
    tk("not equal to"),
    tk("greater than or equal to"),
    tk("less than or equal to"),
    tk("containing"),
    tk("not containing"),
    tk("beginning with"),
    tk("ending with"),
    tk("Yesterday"),
    tk("Today"),
    tk("Tomorrow"),
    tk("In the last 7 days"),
    tk("Last week"),
    tk("This week"),
    tk("Next week"),
    tk("Last month"),
    tk("This month"),
    tk("Next month"),
    tk("Light Red Fill with Dark Red Text"),
    tk("Yellow Fill with Dark Yellow Text"),
    tk("Green Fill with Dark Green Text"),
    tk("Light Red Fill"),
    tk("Red Text"),
    tk("Blue"),
    tk("Green"),
    tk("Red"),
    tk("Orange"),
    tk("Light Blue"),
    tk("Purple"),
    tk("Green - Yellow - Red"),
    tk("Red - Yellow - Green"),
    tk("Green - White - Red"),
    tk("Red - White - Green"),
    tk("Blue - White - Red"),
    tk("Red - White - Blue"),
    tk("White - Red"),
    tk("Red - White"),
    tk("Green - White"),
    tk("White - Green"),
    tk("Green - Yellow"),
    tk("Yellow - Green"),
    tk("3 Arrows"),
    tk("3 Arrows (Gray)"),
    tk("3 Triangles"),
    tk("4 Arrows"),
    tk("5 Arrows"),
    tk("3 Traffic Lights"),
    tk("4 Traffic Lights"),
    tk("3 Signs"),
    tk("3 Symbols (Circled)"),
    tk("3 Symbols"),
    tk("3 Flags"),
    tk("5 Ratings"),
    tk("Type the value to compare with"),
    tk("Type the text to look for"),
    tk("Give a number of items (1-1000) or a percentage (1-100)"),
    tk("Type a formula, such as $B2>100"),
    tk("A colour scale has two or three colours"),
    tk("Type the cells it applies to, such as A2:A20"),
    tk("Errors"),
    tk("No errors"),
    tk("Date occurring"),
];

/// An engine error for the UI, translated when it's one of the known
/// messages.
fn error_text(e: &anyhow::Error) -> String {
    let s = format!("{e:#}");
    t_dyn(&s).to_owned()
}

/// The Manage Rules line for a rule, in the interface language (the
/// engine's `summary` is English).
fn summary_text(r: &sheets_engine::CfRuleInfo) -> String {
    let Some(def) = r.def.as_ref() else {
        let s = r.summary.as_str();
        let inner =
            |pre: &str, post: &str| s.strip_prefix(pre)?.strip_suffix(post).map(str::to_owned);
        if let Some(v) = inner("Text equal to \"", "\"") {
            return tf("Text equal to \"{}\"", &[&v]);
        }
        if let Some(n) = inner("Icon set (", " icons)") {
            return tf("Icon set ({} icons)", &[&n]);
        }
        if let Some(n) = inner("Rating (", " steps)") {
            return tf("Rating ({} steps)", &[&n]);
        }
        if let Some(n) = s.strip_suffix("-color scale") {
            return tf("{}-color scale", &[&n]);
        }
        return t_dyn(s).to_owned();
    };
    match &def.spec {
        CfSpec::CellValue { op, value, value2 } if op.two_values() => {
            let op = t_dyn(op.label());
            tf("Cell value {} {} and {}", &[&op, value, value2])
        }
        CfSpec::CellValue { op, value, .. } => tf("Cell value {} {}", &[&t_dyn(op.label()), value]),
        CfSpec::Text { op, text } => tf("Text {} \"{}\"", &[&t_dyn(op.label()), text]),
        CfSpec::Dates { period } => {
            let p = t_dyn(period.label()).to_lowercase();
            tf("Date occurring: {}", &[&p])
        }
        CfSpec::Duplicates { unique: false } => t("Duplicate values").to_owned(),
        CfSpec::Duplicates { unique: true } => t("Unique values").to_owned(),
        CfSpec::TopBottom {
            bottom,
            rank,
            percent,
        } => {
            let n = format!("{rank}{}", if *percent { "%" } else { "" });
            if *bottom {
                tf("Bottom {}", &[&n])
            } else {
                tf("Top {}", &[&n])
            }
        }
        CfSpec::Average { below: false } => t("Above average").to_owned(),
        CfSpec::Average { below: true } => t("Below average").to_owned(),
        CfSpec::Blanks { not: false } => t("Blank cells").to_owned(),
        CfSpec::Blanks { not: true } => t("Cells with something in them").to_owned(),
        CfSpec::Formula { formula } => tf("Formula: {}", &[formula]),
        CfSpec::DataBar { .. } => t("Data bar").to_owned(),
        CfSpec::ColorScale { colors } => tf("{}-color scale", &[&colors.len()]),
        CfSpec::IconSet { .. } => r.summary.clone(),
    }
}

/// A small sample of a format.
fn format_chip(f: &CfFormat, text: &'static str, cx: &App) -> Div {
    let t = tokens(cx);
    let mut d = div()
        .flex_none()
        .px(SPACE_3)
        .h(CONTROL_SM)
        .flex()
        .items_center()
        .rounded(RADIUS_SM)
        .border_1()
        .border_color(t.border)
        .bg(f.fill.map(crate::look::hsla).unwrap_or(gpui::white()))
        .text_size(FONT_SMALL)
        .text_color(f.font.map(crate::look::hsla).unwrap_or(gpui::black()));
    if f.bold {
        d = d.font_weight(FontWeight::BOLD);
    }
    if f.italic {
        d = d.italic();
    }
    if f.underline {
        d = d.underline();
    }
    if f.strike {
        d = d.line_through();
    }
    d.child(text)
}

/// A colour scale as a strip.
fn scale_strip(colors: &[u32]) -> Div {
    let mut d = h_flex().h(px(14.)).w(px(84.));
    for c in colors {
        d = d.child(div().flex_1().h_full().bg(crate::look::hsla(*c)));
    }
    d
}

/// The toolbar's Conditional Formatting menu.
pub(crate) fn cf_menu(
    menu: PopupMenu,
    view: &WeakEntity<SheetsView>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let open = |label: &'static str, pick: fn() -> Pick, view: &WeakEntity<SheetsView>| {
        let v = view.clone();
        PopupMenuItem::new(t_dyn(label)).on_click(move |_, w, cx| {
            let (kind, set) = pick();
            let _ = v.update(cx, |this, cx| this.open_cf_dialog(kind, |d| set(d), w, cx));
        })
    };
    let hv = view.clone();
    let tv = view.clone();
    let bv = view.clone();
    let sv = view.clone();
    let iv = view.clone();
    let menu = menu
        .submenu(t("Highlight Cells Rules"), window, cx, move |m, _, _| {
            m.item(open(
                tk("Greater Than…"),
                || {
                    (
                        Kind::CellValue,
                        Box::new(|d| d.compare = CfCompare::GreaterThan),
                    )
                },
                &hv,
            ))
            .item(open(
                tk("Less Than…"),
                || {
                    (
                        Kind::CellValue,
                        Box::new(|d| d.compare = CfCompare::LessThan),
                    )
                },
                &hv,
            ))
            .item(open(
                tk("Between…"),
                || {
                    (
                        Kind::CellValue,
                        Box::new(|d| d.compare = CfCompare::Between),
                    )
                },
                &hv,
            ))
            .item(open(
                tk("Equal To…"),
                || {
                    (
                        Kind::CellValue,
                        Box::new(|d| d.compare = CfCompare::EqualTo),
                    )
                },
                &hv,
            ))
            .item(open(
                tk("Text that Contains…"),
                || (Kind::Text, Box::new(|_| {})),
                &hv,
            ))
            .item(open(
                tk("A Date Occurring…"),
                || (Kind::Dates, Box::new(|_| {})),
                &hv,
            ))
            .item(open(
                tk("Duplicate Values…"),
                || (Kind::Duplicates, Box::new(|_| {})),
                &hv,
            ))
        })
        .submenu(t("Top/Bottom Rules"), window, cx, move |m, _, _| {
            m.item(open(
                tk("Top 10 Items…"),
                || (Kind::TopBottom, Box::new(|_| {})),
                &tv,
            ))
            .item(open(
                tk("Top 10%…"),
                || (Kind::TopBottom, Box::new(|d| d.percent = true)),
                &tv,
            ))
            .item(open(
                tk("Bottom 10 Items…"),
                || (Kind::TopBottom, Box::new(|d| d.bottom = true)),
                &tv,
            ))
            .item(open(
                tk("Bottom 10%…"),
                || {
                    (
                        Kind::TopBottom,
                        Box::new(|d| {
                            d.bottom = true;
                            d.percent = true;
                        }),
                    )
                },
                &tv,
            ))
            .item(open(
                tk("Above Average…"),
                || (Kind::Average, Box::new(|_| {})),
                &tv,
            ))
            .item(open(
                tk("Below Average…"),
                || (Kind::Average, Box::new(|d| d.below = true)),
                &tv,
            ))
        })
        .submenu(t("Data Bars"), window, cx, move |mut m, _, _| {
            for gradient in [true, false] {
                m = m.label(if gradient {
                    t("Gradient Fill")
                } else {
                    t("Solid Fill")
                });
                for (name, color) in DATA_BAR_COLORS {
                    let v = bv.clone();
                    m = m.item(PopupMenuItem::new(tf("{} Data Bar", &[&t(name)])).on_click(
                        move |_, w, cx| {
                            let _ = v.update(cx, |this, cx| {
                                this.quick_cf(CfSpec::DataBar { color, gradient }, w, cx)
                            });
                        },
                    ));
                }
            }
            m
        })
        .submenu(t("Color Scales"), window, cx, move |mut m, _, _| {
            for (name, colors) in COLOR_SCALES {
                let v = sv.clone();
                m = m.item(
                    PopupMenuItem::new(tf("{} Color Scale", &[&t(name)])).on_click(
                        move |_, w, cx| {
                            let _ = v.update(cx, |this, cx| {
                                this.quick_cf(
                                    CfSpec::ColorScale {
                                        colors: colors.to_vec(),
                                    },
                                    w,
                                    cx,
                                )
                            });
                        },
                    ),
                );
            }
            m
        })
        .submenu(t("Icon Sets"), window, cx, move |mut m, _, _| {
            for (name, set) in ICON_SETS {
                let v = iv.clone();
                m = m.item(PopupMenuItem::new(t(name)).on_click(move |_, w, cx| {
                    let _ = v.update(cx, |this, cx| {
                        this.quick_cf(
                            CfSpec::IconSet {
                                set: set.to_owned(),
                                reverse: false,
                                show_value: true,
                            },
                            w,
                            cx,
                        )
                    });
                }));
            }
            m
        })
        .separator();
    let nv = view.clone();
    let c1 = view.clone();
    let c2 = view.clone();
    let mv = view.clone();
    menu.item(
        PopupMenuItem::new(t("New Rule…")).on_click(move |_, w, cx| {
            let _ = nv.update(cx, |this, cx| this.new_cf_rule_doc(w, cx));
        }),
    )
    .item(
        PopupMenuItem::new(t("Clear Rules from Selected Cells")).on_click(move |_, w, cx| {
            let _ = c1.update(cx, |this, cx| this.clear_cf_doc(false, w, cx));
        }),
    )
    .item(
        PopupMenuItem::new(t("Clear Rules from Entire Sheet")).on_click(move |_, w, cx| {
            let _ = c2.update(cx, |this, cx| this.clear_cf_doc(true, w, cx));
        }),
    )
    .item(
        PopupMenuItem::new(t("Manage Rules…")).on_click(move |_, w, cx| {
            let _ = mv.update(cx, |this, cx| this.manage_cf_doc(w, cx));
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn engine_strings_have_indonesian() {
        let missing: Vec<&str> = ENGINE_KEYS
            .iter()
            .copied()
            .chain(DATA_BAR_COLORS.iter().map(|(n, _)| *n))
            .chain(COLOR_SCALES.iter().map(|(n, _)| *n))
            .chain(ICON_SETS.iter().map(|(n, _)| *n))
            .chain(CfFormat::PRESETS.iter().map(|(n, _)| *n))
            .chain(CfCompare::ALL.iter().map(|c| c.label()))
            .chain(CfTextOp::ALL.iter().map(|c| c.label()))
            .chain(CfPeriod::ALL.iter().map(|c| c.label()))
            .filter(|k| kantor_ui::i18n::indonesian(k).is_none())
            .collect();
        assert!(missing.is_empty(), "no Indonesian for {missing:?}");
    }
}
