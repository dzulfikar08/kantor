//! Sheets: "Ask AI for a formula…" and "Explain formula".
//!
//! Sends the column headers, up to 20 sample rows near the selection and the
//! active cell's address (never the whole sheet). A suggested formula is
//! parsed with IronCalc and evaluated in a scratch copy before Insert is
//! offered; Insert is one undo step.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{self, Input, InputEvent, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ai::prompt::{self, FormulaReply, Lang};
use kantor_ai::sample::{SheetSample, sample_sheet};
use kantor_ai::{AiError, Cancel};
use kantor_core::settings::Settings;
use kantor_ui::metrics::*;
use sheets_engine::check::{FormulaPreview, evaluate_in_copy};
use sheets_ui::{AiTarget, SheetsView};

use super::widgets::{Status, card, footer, note, result_box, status_line};
use super::{AI_PANEL, ClosePanel, PanelEvent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormulaMode {
    Ask,
    Explain,
}

enum State {
    Idle,
    Asking {
        chars: usize,
    },
    Ready {
        reply: FormulaReply,
        preview: FormulaPreview,
    },
    Explained(String),
    Failed {
        message: String,
        formula: Option<String>,
    },
}

pub struct FormulaPanel {
    sheets: WeakEntity<SheetsView>,
    target: AiTarget,
    sample: SheetSample,
    mode: FormulaMode,
    /// The active cell's formula (Explain).
    formula: String,
    input: Entity<InputState>,
    /// The request the shown result answers.
    asked: Option<String>,
    state: State,
    cancel: Option<Cancel>,
    enabled: bool,
    provider: &'static str,
    focus: FocusHandle,
    _subscription: Subscription,
}

impl EventEmitter<PanelEvent> for FormulaPanel {}

impl FormulaPanel {
    pub fn new(
        sheets: Entity<SheetsView>,
        mode: FormulaMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let ai = Settings::load().ai;
        let (target, sample, formula) = {
            let view = sheets.read(cx);
            let target = view.ai_target(cx);
            view.read_book(cx, |book, sheet| {
                let names = book.sheet_names();
                let sample = sample_sheet(
                    names.get(sheet as usize).map_or("", String::as_str),
                    book.used_size(sheet),
                    (target.row, target.col),
                    sheets_engine::column_name,
                    |r, c| book.display_value(sheet, r, c),
                );
                (target, sample, book.input(sheet, target.row, target.col))
            })
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Describe it, e.g. total Sales where Region is Jakarta")
        });
        let subscription =
            cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.enter(window, cx);
                }
            });
        let mut this = Self {
            sheets: sheets.downgrade(),
            target,
            sample,
            mode,
            formula,
            input,
            asked: None,
            state: State::Idle,
            cancel: None,
            enabled: super::client_allowed(&ai),
            provider: super::provider_label(&ai),
            focus: cx.focus_handle(),
            _subscription: subscription,
        };
        if mode == FormulaMode::Explain && this.enabled {
            this.explain(cx);
        }
        this
    }

    pub fn focus_first(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == FormulaMode::Ask && self.enabled {
            self.input.update(cx, |i, cx| i.focus(window, cx));
        } else {
            window.focus(&self.focus, cx);
        }
    }

    fn query(&self, cx: &App) -> String {
        self.input.read(cx).value().trim().to_owned()
    }

    /// Enter: insert the ready formula if the request is unchanged, else ask.
    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.query(cx);
        if matches!(self.state, State::Ready { .. }) && self.asked.as_deref() == Some(&query) {
            self.insert(window, cx);
        } else {
            self.ask(cx);
        }
    }

    fn ask(&mut self, cx: &mut Context<Self>) {
        let query = self.query(cx);
        if query.is_empty() || !self.enabled {
            return;
        }
        self.stop();
        let ai = Settings::load().ai;
        let req = prompt::formula_request(&query, &self.sample);
        self.asked = Some(query);
        self.state = State::Asking { chars: 0 };
        self.cancel = Some(super::run(
            cx,
            move |cancel, on_text| {
                let client = super::client(&ai)?;
                let text = client.complete(&req, cancel, on_text)?;
                prompt::parse_formula_reply(&text)
            },
            Self::on_text,
            |this: &mut Self, r, cx| this.on_reply(r, cx),
        ));
        cx.notify();
    }

    fn explain(&mut self, cx: &mut Context<Self>) {
        if !self.formula.starts_with('=') {
            self.state = State::Failed {
                message: format!("{} has no formula to explain.", self.sample.active),
                formula: None,
            };
            return;
        }
        let ai = Settings::load().ai;
        let req = prompt::explain_request(&self.formula, &self.sample, Lang::from_env());
        self.state = State::Asking { chars: 0 };
        self.cancel = Some(super::run(
            cx,
            move |cancel, on_text| {
                let client = super::client(&ai)?;
                let text = client.complete(&req, cancel, on_text)?;
                prompt::parse_explain_reply(&text)
            },
            Self::on_text,
            |this: &mut Self, r, cx| {
                this.cancel = None;
                this.state = match r {
                    Ok(text) => State::Explained(text),
                    Err(e) => State::Failed {
                        message: e.to_string(),
                        formula: None,
                    },
                };
                cx.notify();
            },
        ));
    }

    fn on_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if let State::Asking { chars } = &mut self.state {
            *chars += text.chars().count();
            cx.notify();
        }
    }

    /// Check the suggestion: parse here, evaluate in a scratch copy on a
    /// worker thread.
    fn on_reply(&mut self, r: Result<FormulaReply, AiError>, cx: &mut Context<Self>) {
        self.cancel = None;
        let reply = match r {
            Ok(reply) => reply,
            Err(e) => {
                self.state = State::Failed {
                    message: e.to_string(),
                    formula: None,
                };
                cx.notify();
                return;
            }
        };
        let AiTarget { sheet, row, col } = self.target;
        let checked = self.sheets.upgrade().map(|s| {
            s.read(cx).read_book(cx, |book, _| {
                book.parse_formula(sheet, row, col, &reply.formula)
                    .map(|()| book.snapshot())
            })
        });
        let snapshot = match checked {
            None => {
                cx.emit(PanelEvent::Close);
                return;
            }
            Some(Err(e)) => {
                self.state = State::Failed {
                    message: format!("The suggested formula doesn't parse: {e}. Try rephrasing."),
                    formula: Some(reply.formula),
                };
                cx.notify();
                return;
            }
            Some(Ok(bytes)) => bytes,
        };
        let formula = reply.formula.clone();
        self.state = State::Asking { chars: 0 };
        super::run(
            cx,
            move |_, _| {
                evaluate_in_copy(&snapshot, sheet, row, col, &formula).map_err(AiError::BadReply)
            },
            |_, _, _| {},
            move |this: &mut Self, r, cx| {
                this.state = match r {
                    Ok(preview) => State::Ready { reply, preview },
                    Err(e) => State::Failed {
                        message: format!("{e}. Try rephrasing."),
                        formula: Some(reply.formula),
                    },
                };
                cx.notify();
            },
        );
        cx.notify();
    }

    fn insert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let State::Ready { reply, .. } = &self.state else {
            return;
        };
        let formula = reply.formula.clone();
        let target = self.target;
        let Some(sheets) = self.sheets.upgrade() else {
            cx.emit(PanelEvent::Close);
            return;
        };
        match sheets.update(cx, |v, cx| {
            v.insert_formula_doc(target, &formula, window, cx)
        }) {
            Ok(()) => cx.emit(PanelEvent::Close),
            Err(e) => {
                self.state = State::Failed {
                    message: format!("Couldn't insert: {e}"),
                    formula: Some(formula),
                };
                cx.notify();
            }
        }
    }

    fn stop(&mut self) {
        if let Some(c) = self.cancel.take() {
            c.cancel();
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.stop();
        cx.emit(PanelEvent::Close);
    }

    fn status(&self) -> Status {
        match &self.state {
            State::Idle => Status::None,
            State::Asking { chars: 0 } => Status::Busy(format!("Asking {}…", self.provider)),
            State::Asking { chars } => Status::Busy(format!("Receiving… ({chars} characters)")),
            State::Ready { preview, .. } => match &preview.warning {
                Some(w) => Status::Warn(w.clone()),
                None => Status::Ok(format!("Checked: shows {}", preview.value)),
            },
            State::Explained(_) => Status::None,
            State::Failed { message, .. } => Status::Error(message.clone()),
        }
    }
}

impl Render for FormulaPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = kantor_ui::tokens(cx);
        let title = match self.mode {
            FormulaMode::Ask => "Ask AI for a formula",
            FormulaMode::Explain => "Explain formula",
        };
        let subtitle = format!("{} · {}", self.sample.sheet, self.sample.active);
        let busy = matches!(self.state, State::Asking { .. });
        let ready = matches!(self.state, State::Ready { .. });

        let mut body = v_flex().p(SPACE_7).gap(SPACE_5);
        if !self.enabled {
            body = body.child(note(
                "AI features are off. Turn them on and choose a provider in AI Settings.",
                cx,
            ));
        } else {
            body = match self.mode {
                FormulaMode::Ask => body.child(Input::new(&self.input)).child(note(
                    format!(
                        "Sends the column headers, up to 20 sample rows and the address {} to {}. \
                         Write in English or Bahasa Indonesia.",
                        self.sample.active, self.provider
                    ),
                    cx,
                )),
                FormulaMode::Explain => body
                    .child(result_box(cx).child(self.formula.clone()))
                    .child(note(
                        format!(
                            "Sends this formula and the column headers to {}.",
                            self.provider
                        ),
                        cx,
                    )),
            };
            match &self.state {
                State::Ready { reply, preview } => {
                    body = body.child(
                        result_box(cx)
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(reply.formula.clone()),
                            )
                            .when(!reply.explanation.is_empty(), |d| {
                                d.child(
                                    div()
                                        .pt(SPACE_3)
                                        .text_size(FONT_SMALL)
                                        .text_color(t.fg_muted)
                                        .child(reply.explanation.clone()),
                                )
                            })
                            .child(
                                div()
                                    .pt(SPACE_3)
                                    .text_size(FONT_SMALL)
                                    .text_color(t.fg_dim)
                                    .child(format!("Result now: {}", preview.value)),
                            ),
                    );
                }
                State::Explained(text) => {
                    body = body.child(result_box(cx).child(text.clone()));
                }
                State::Failed {
                    formula: Some(f), ..
                } => {
                    body = body.child(result_box(cx).text_color(t.fg_muted).child(f.clone()));
                }
                _ => {}
            }
        }

        let mut buttons =
            footer(cx).child(div().flex_1().children(status_line(&self.status(), cx)));
        if !self.enabled {
            buttons = buttons.child(
                Button::new("ai-open-settings")
                    .primary()
                    .small()
                    .label("AI Settings…")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(PanelEvent::OpenSettings))),
            );
        } else {
            buttons = buttons.child(
                Button::new("ai-close")
                    .ghost()
                    .small()
                    .label(if busy { "Cancel" } else { "Close" })
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
            );
            if self.mode == FormulaMode::Ask {
                buttons = buttons
                    .child(
                        Button::new("ai-ask")
                            .outline()
                            .small()
                            .label(if ready { "Ask again" } else { "Ask" })
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.ask(cx))),
                    )
                    .child(
                        Button::new("ai-insert")
                            .primary()
                            .small()
                            .label(format!("Insert in {}", self.sample.active))
                            .disabled(!ready)
                            .on_click(cx.listener(|this, _, window, cx| this.insert(window, cx))),
                    );
            }
        }

        div()
            .id("ai-formula")
            .key_context(AI_PANEL)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ClosePanel, _, cx| this.close(cx)))
            .on_action(cx.listener(|this, _: &input::Escape, _, cx| this.close(cx)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(card(title, Some(subtitle), cx).child(body).child(buttons))
    }
}
