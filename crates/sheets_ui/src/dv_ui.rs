//! Data Validation… for the selection: Excel's three tabs (Settings, Input
//! Message, Error Alert) in a card over the grid. OK puts the rule on the
//! cells it applies to (they leave any rule they had), Clear All removes
//! validation from them; one undo step each (`sheets_engine::validation_edit`).
//! The grid side (drop-down lists, input messages, alerts) is in
//! [`crate::data_validation`].

use gpui_kit::component::{
    Sizable as _,
    button::Button,
    checkbox::Checkbox,
    h_flex,
    input::{self, Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::metrics::*;
use sheets_engine::{
    ErrorStyle, Message, Validation, ValidationOperator, ValidationType, list_source,
    list_source_text, sqref_of, validation_operand, validation_operand_text,
};

use crate::SheetsView;
use crate::dialog::{self, choices, note, row};

actions!(sheets, [DataValidation]);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Settings,
    Input,
    Error,
}

const KINDS: [(ValidationType, &str); 8] = [
    (ValidationType::Any, "Any value"),
    (ValidationType::Whole, "Whole number"),
    (ValidationType::Decimal, "Decimal"),
    (ValidationType::List, "List"),
    (ValidationType::Date, "Date"),
    (ValidationType::Time, "Time"),
    (ValidationType::TextLength, "Text length"),
    (ValidationType::Custom, "Custom"),
];

const OPERATORS: [(ValidationOperator, &str); 8] = [
    (ValidationOperator::Between, "between"),
    (ValidationOperator::NotBetween, "not between"),
    (ValidationOperator::Equal, "equal to"),
    (ValidationOperator::NotEqual, "not equal to"),
    (ValidationOperator::GreaterThan, "greater than"),
    (ValidationOperator::LessThan, "less than"),
    (
        ValidationOperator::GreaterThanOrEqual,
        "greater than or equal to",
    ),
    (ValidationOperator::LessThanOrEqual, "less than or equal to"),
];

fn label<T: PartialEq + Copy>(list: &[(T, &'static str)], v: T) -> &'static str {
    list.iter().find(|(k, _)| *k == v).map_or("", |(_, l)| l)
}

fn compares(kind: ValidationType) -> bool {
    matches!(
        kind,
        ValidationType::Whole
            | ValidationType::Decimal
            | ValidationType::Date
            | ValidationType::Time
            | ValidationType::TextLength
    )
}

fn two(op: ValidationOperator) -> bool {
    matches!(
        op,
        ValidationOperator::Between | ValidationOperator::NotBetween
    )
}

/// The Data Validation card.
pub(crate) struct DvDialog {
    sheet: u32,
    tab: Tab,
    kind: ValidationType,
    op: ValidationOperator,
    allow_blank: bool,
    dropdown: bool,
    show_prompt: bool,
    show_error: bool,
    style: ErrorStyle,
    applies: Entity<InputState>,
    value1: Entity<InputState>,
    value2: Entity<InputState>,
    source: Entity<InputState>,
    formula: Entity<InputState>,
    prompt_title: Entity<InputState>,
    prompt_text: Entity<InputState>,
    error_title: Entity<InputState>,
    error_text: Entity<InputState>,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl SheetsView {
    /// Open Data Validation… for the selection (showing the rule of the
    /// active cell, if it has one).
    pub fn data_validation_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let (sheet, sel, current) = self.table.update(cx, |table, _| {
            let grid = table.delegate_mut();
            let sel = grid.range();
            let sheet = grid.sheet;
            let (r, c) = (sel.top, sel.left);
            (
                sheet,
                sel,
                grid.book.validation(sheet, r, c).map(|v| (*v).clone()),
            )
        });
        let v = current.unwrap_or(Validation {
            kind: ValidationType::Any,
            operator: ValidationOperator::Between,
            allow_blank: true,
            dropdown: true,
            prompt: None,
            error: Some((ErrorStyle::Stop, Message::default())),
            formula1: None,
            formula2: None,
            ranges: Vec::new(),
        });
        let mk = |window: &mut Window, cx: &mut Context<Self>, value: String, ph: &str| {
            let e = cx.new(|cx| InputState::new(window, cx).placeholder(ph.to_owned()));
            e.update(cx, |i, cx| i.set_value(value, window, cx));
            e
        };
        let bound = |f: &Option<String>| {
            f.as_deref()
                .map(|f| validation_operand_text(v.kind, f))
                .unwrap_or_default()
        };
        let is_list = v.kind == ValidationType::List;
        let is_custom = v.kind == ValidationType::Custom;
        let applies = mk(window, cx, sqref_of(&[sel]), "e.g. A2:A20");
        let value1 = mk(
            window,
            cx,
            if compares(v.kind) {
                bound(&v.formula1)
            } else {
                String::new()
            },
            "a value or =formula",
        );
        let value2 = mk(
            window,
            cx,
            if compares(v.kind) {
                bound(&v.formula2)
            } else {
                String::new()
            },
            "a value or =formula",
        );
        let source = mk(
            window,
            cx,
            if is_list {
                v.formula1
                    .as_deref()
                    .map(list_source_text)
                    .unwrap_or_default()
            } else {
                String::new()
            },
            "Yes, No, Maybe   or   =$H$1:$H$9",
        );
        let formula = mk(
            window,
            cx,
            if is_custom {
                v.formula1
                    .as_deref()
                    .map(|f| format!("={f}"))
                    .unwrap_or_default()
            } else {
                String::new()
            },
            "e.g. =ISNUMBER(A2)",
        );
        let pm = v.prompt.clone().unwrap_or_default();
        let (style, em) = v
            .error
            .clone()
            .unwrap_or((ErrorStyle::Stop, Message::default()));
        let prompt_title = mk(window, cx, pm.title, "");
        let prompt_text = mk(window, cx, pm.text, "");
        let error_title = mk(window, cx, em.title, "");
        let error_text = mk(window, cx, em.text, "");
        let subs = [
            &applies,
            &value1,
            &value2,
            &source,
            &formula,
            &prompt_title,
            &prompt_text,
            &error_title,
            &error_text,
        ]
        .into_iter()
        .map(|e| {
            cx.subscribe_in(e, window, |this, _, ev: &InputEvent, w, cx| match ev {
                InputEvent::Change => {
                    if let Some(d) = this.dv_dialog.as_mut() {
                        d.error = None;
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.dv_ok(w, cx),
                _ => {}
            })
        })
        .collect();
        applies.update(cx, |i, cx| i.focus(window, cx));
        self.dv_dialog = Some(DvDialog {
            sheet,
            tab: Tab::Settings,
            kind: v.kind,
            op: v.operator,
            allow_blank: v.allow_blank,
            dropdown: v.dropdown,
            show_prompt: v.prompt.is_some(),
            show_error: v.error.is_some(),
            style,
            applies,
            value1,
            value2,
            source,
            formula,
            prompt_title,
            prompt_text,
            error_title,
            error_text,
            error: None,
            _subs: subs,
        });
        cx.notify();
    }

    fn close_dv(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dv_dialog.take().is_some() {
            self.focus_grid(window, cx);
            cx.notify();
        }
    }

    fn dv_rule(
        d: &DvDialog,
        cx: &App,
    ) -> Result<(Vec<sheets_engine::CellRange>, Validation), String> {
        let text = |e: &Entity<InputState>| e.read(cx).value().to_string();
        let ranges = dialog::parse_ranges(&text(&d.applies).replace(' ', ","))
            .filter(|r| !r.is_empty())
            .ok_or("Applies to: type the cells, such as A2:A20")?;
        let (formula1, formula2) = match d.kind {
            ValidationType::Any => (None, None),
            ValidationType::List => (
                Some(list_source(&text(&d.source)).map_err(|e| format!("Source: {e}"))?),
                None,
            ),
            ValidationType::Custom => {
                let f = text(&d.formula);
                let f = f.trim().trim_start_matches('=').trim();
                if f.is_empty() {
                    return Err("Formula: type a formula that is TRUE for valid values".into());
                }
                (Some(f.to_owned()), None)
            }
            k => {
                let a = validation_operand(k, &text(&d.value1)).map_err(|e| format!("{e}"))?;
                let b = if two(d.op) {
                    Some(validation_operand(k, &text(&d.value2)).map_err(|e| format!("{e}"))?)
                } else {
                    None
                };
                (Some(a), b)
            }
        };
        let msg = |t: &Entity<InputState>, m: &Entity<InputState>| Message {
            title: text(t),
            text: text(m),
        };
        let prompt = msg(&d.prompt_title, &d.prompt_text);
        Ok((
            ranges,
            Validation {
                kind: d.kind,
                operator: d.op,
                allow_blank: d.allow_blank,
                dropdown: d.dropdown,
                prompt: (d.show_prompt && !(prompt.title.is_empty() && prompt.text.is_empty()))
                    .then_some(prompt),
                error: d
                    .show_error
                    .then(|| (d.style, msg(&d.error_title, &d.error_text))),
                formula1,
                formula2,
                ranges: Vec::new(),
            },
        ))
    }

    fn dv_ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.dv_dialog.as_ref() else {
            return;
        };
        let sheet = d.sheet;
        let res = Self::dv_rule(d, cx).and_then(|(ranges, v)| {
            self.table.update(cx, |table, cx| {
                let r = table.delegate_mut().book.set_validation(sheet, &ranges, v);
                table.refresh(cx);
                r.map_err(|e| format!("{e:#}"))
            })
        });
        self.after_dv(res, "Data validation set", window, cx);
    }

    fn dv_clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.dv_dialog.as_ref() else {
            return;
        };
        let sheet = d.sheet;
        let text = d.applies.read(cx).value().to_string();
        let res = dialog::parse_ranges(&text.replace(' ', ","))
            .ok_or_else(|| "Applies to: type the cells, such as A2:A20".to_owned())
            .and_then(|ranges| {
                self.table.update(cx, |table, cx| {
                    let r = table.delegate_mut().book.clear_validation(sheet, &ranges);
                    table.refresh(cx);
                    r.map_err(|e| format!("{e:#}"))
                })
            });
        self.after_dv(res, "Data validation cleared", window, cx);
    }

    fn after_dv(
        &mut self,
        res: Result<(), String>,
        done: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match res {
            Ok(()) => {
                self.status = done.to_owned().into();
                self.close_dv(window, cx);
                self.update_title(window, cx);
            }
            Err(e) => {
                if let Some(d) = self.dv_dialog.as_mut() {
                    d.error = Some(e);
                }
                cx.notify();
            }
        }
    }

    pub(crate) fn render_dv_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.dv_dialog.as_ref()?;
        let view = cx.entity().downgrade();
        let tabs = choices(
            "dv-tab",
            &[
                (Tab::Settings, "Settings"),
                (Tab::Input, "Input Message"),
                (Tab::Error, "Error Alert"),
            ],
            d.tab,
            &view,
            |this, t| {
                if let Some(d) = this.dv_dialog.as_mut() {
                    d.tab = t;
                }
            },
        );
        let check =
            |id: &'static str, label: &'static str, on: bool, set: fn(&mut DvDialog, bool)| {
                let v = view.clone();
                Checkbox::new(id)
                    .label(label)
                    .checked(on)
                    .on_click(move |on, _, cx| {
                        let on = *on;
                        let _ = v.update(cx, |this, cx| {
                            if let Some(d) = this.dv_dialog.as_mut() {
                                set(d, on);
                            }
                            cx.notify();
                        });
                    })
            };
        let body = match d.tab {
            Tab::Settings => {
                let kv = view.clone();
                let current = d.kind;
                let kind = Button::new("dv-kind")
                    .outline()
                    .small()
                    .label(label(&KINDS, d.kind))
                    .dropdown_caret(true)
                    .dropdown_menu(move |mut m, _, _| {
                        for (k, l) in KINDS {
                            let v = kv.clone();
                            m = m.item(PopupMenuItem::new(l).checked(k == current).on_click(
                                move |_, _, cx| {
                                    let _ = v.update(cx, |this, cx| {
                                        if let Some(d) = this.dv_dialog.as_mut() {
                                            d.kind = k;
                                        }
                                        cx.notify();
                                    });
                                },
                            ));
                        }
                        m
                    });
                let mut b = v_flex().gap(SPACE_4).child(row("Allow", kind, cx));
                if compares(d.kind) {
                    let ov = view.clone();
                    let cur = d.op;
                    b = b.child(row(
                        "Data",
                        Button::new("dv-op")
                            .outline()
                            .small()
                            .label(label(&OPERATORS, d.op))
                            .dropdown_caret(true)
                            .dropdown_menu(move |mut m, _, _| {
                                for (o, l) in OPERATORS {
                                    let v = ov.clone();
                                    m = m.item(PopupMenuItem::new(l).checked(o == cur).on_click(
                                        move |_, _, cx| {
                                            let _ = v.update(cx, |this, cx| {
                                                if let Some(d) = this.dv_dialog.as_mut() {
                                                    d.op = o;
                                                }
                                                cx.notify();
                                            });
                                        },
                                    ));
                                }
                                m
                            }),
                        cx,
                    ));
                    let (l1, l2) = match d.op {
                        ValidationOperator::Between | ValidationOperator::NotBetween => {
                            ("Minimum", "Maximum")
                        }
                        ValidationOperator::GreaterThan
                        | ValidationOperator::GreaterThanOrEqual => ("Minimum", ""),
                        ValidationOperator::LessThan | ValidationOperator::LessThanOrEqual => {
                            ("Maximum", "")
                        }
                        _ => ("Value", ""),
                    };
                    b = b.child(row(l1, Input::new(&d.value1).small(), cx));
                    if two(d.op) {
                        b = b.child(row(l2, Input::new(&d.value2).small(), cx));
                    }
                    let hint = match d.kind {
                        ValidationType::Date => "Dates as 2026-01-31 or 31/01/2026, or =formula",
                        ValidationType::Time => "Times as 9:30, or =formula",
                        ValidationType::TextLength => "Number of characters",
                        _ => "A number, or =formula such as =$B$1",
                    };
                    b = b.child(note(hint, false, cx));
                }
                match d.kind {
                    ValidationType::List => {
                        b = b
                            .child(row("Source", Input::new(&d.source).small(), cx))
                            .child(note(
                                "Items separated by commas, or =cells such as =$H$1:$H$9 (another sheet: =Lists!$A$1:$A$9)",
                                false,
                                cx,
                            ));
                    }
                    ValidationType::Custom => {
                        b = b.child(row("Formula", Input::new(&d.formula).small(), cx)).child(
                            note(
                                "TRUE for a valid value; references are relative to the first cell.",
                                false,
                                cx,
                            ),
                        );
                    }
                    _ => {}
                }
                let mut opts = h_flex().gap(SPACE_6);
                if d.kind != ValidationType::Any {
                    opts = opts.child(check("dv-blank", "Ignore blank", d.allow_blank, |d, v| {
                        d.allow_blank = v
                    }));
                }
                if d.kind == ValidationType::List {
                    opts = opts.child(check(
                        "dv-dropdown",
                        "In-cell dropdown",
                        d.dropdown,
                        |d, v| d.dropdown = v,
                    ));
                }
                b.child(opts)
                    .child(row("Applies to", Input::new(&d.applies).small(), cx))
            }
            Tab::Input => v_flex()
                .gap(SPACE_4)
                .child(check(
                    "dv-show-prompt",
                    "Show input message when the cell is selected",
                    d.show_prompt,
                    |d, v| d.show_prompt = v,
                ))
                .child(row("Title", Input::new(&d.prompt_title).small(), cx))
                .child(row("Input message", Input::new(&d.prompt_text).small(), cx)),
            Tab::Error => v_flex()
                .gap(SPACE_4)
                .child(check(
                    "dv-show-error",
                    "Show error alert after invalid data is entered",
                    d.show_error,
                    |d, v| d.show_error = v,
                ))
                .child(row(
                    "Style",
                    choices(
                        "dv-style",
                        &[
                            (ErrorStyle::Stop, "Stop"),
                            (ErrorStyle::Warning, "Warning"),
                            (ErrorStyle::Information, "Information"),
                        ],
                        d.style,
                        &view,
                        |this, s| {
                            if let Some(d) = this.dv_dialog.as_mut() {
                                d.style = s;
                            }
                        },
                    ),
                    cx,
                ))
                .child(row("Title", Input::new(&d.error_title).small(), cx))
                .child(row("Error message", Input::new(&d.error_text).small(), cx))
                .child(note(
                    match d.style {
                        ErrorStyle::Stop => "Stop: the value can't stay (Retry / Cancel).",
                        ErrorStyle::Warning => "Warning: asks \"Continue?\" (Yes / No / Cancel).",
                        ErrorStyle::Information => {
                            "Information: tells, then keeps the value (OK / Cancel)."
                        }
                    },
                    false,
                    cx,
                )),
        };
        let clear = view.clone();
        let card = dialog::card("data-validation", "Data Validation", px(480.), cx)
            .key_context("KantorDvDialog")
            .on_action(cx.listener(|this, _: &input::Escape, w, cx| this.close_dv(w, cx)))
            .child(tabs)
            .child(v_flex().min_h(px(230.)).child(body))
            .children(d.error.clone().map(|e| note(e, true, cx)))
            .child(
                h_flex()
                    .child(
                        Button::new("dv-clear")
                            .small()
                            .outline()
                            .label("Clear All")
                            .on_click(move |_, w, cx| {
                                let _ = clear.update(cx, |this, cx| this.dv_clear(w, cx));
                            }),
                    )
                    .child(div().flex_1())
                    .child(dialog::buttons(
                        "dv-buttons",
                        "OK",
                        true,
                        &view,
                        Self::dv_ok,
                        Self::close_dv,
                    )),
            );
        Some(dialog::place(card).into_any_element())
    }

    pub(crate) fn dv_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(
            cx.listener(|this, _: &DataValidation, w, cx| this.data_validation_doc(w, cx)),
        )
    }
}
