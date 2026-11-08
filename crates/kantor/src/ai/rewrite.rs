//! Docs: "Rewrite / Translate selection". Sends only the selected
//! paragraphs' text; shows the result for review; Replace is one undo step
//! that keeps paragraph styles (run formatting becomes each paragraph's first
//! run's, see `docs_engine::rewrite`).

use std::ops::Range;

use docs_ui::DocsView;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ai::prompt::{self, RewriteMode};
use kantor_ai::{AiError, Cancel};
use kantor_core::settings::Settings;
use kantor_ui::metrics::*;

use super::widgets::{PREVIEW_MAX_HEIGHT, Status, card, footer, note, result_box, status_line};
use super::{AI_PANEL, ClosePanel, PanelEvent};

enum State {
    Idle,
    Working { chars: usize },
    Ready(Vec<String>),
    Failed(String),
}

pub struct RewritePanel {
    docs: WeakEntity<DocsView>,
    /// The paragraphs sent and their text, or why the selection can't be used.
    source: Result<(Range<usize>, Vec<String>), String>,
    mode: Option<RewriteMode>,
    state: State,
    cancel: Option<Cancel>,
    enabled: bool,
    provider: &'static str,
    focus: FocusHandle,
}

impl EventEmitter<PanelEvent> for RewritePanel {}

impl RewritePanel {
    pub fn new(docs: Entity<DocsView>, cx: &mut Context<Self>) -> Self {
        let ai = Settings::load().ai;
        let source = docs.read(cx).rewrite_source();
        Self {
            docs: docs.downgrade(),
            source,
            mode: None,
            state: State::Idle,
            cancel: None,
            enabled: super::client_allowed(&ai),
            provider: super::provider_label(&ai),
            focus: cx.focus_handle(),
        }
    }

    pub fn focus_first(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }

    fn run_mode(&mut self, mode: RewriteMode, cx: &mut Context<Self>) {
        let Ok((_, texts)) = &self.source else {
            return;
        };
        if let Some(c) = self.cancel.take() {
            c.cancel();
        }
        self.mode = Some(mode);
        let req = match prompt::rewrite_request(mode, texts) {
            Ok(r) => r,
            Err(e) => {
                self.state = State::Failed(e.to_string());
                cx.notify();
                return;
            }
        };
        let ai = Settings::load().ai;
        self.state = State::Working { chars: 0 };
        self.cancel = Some(super::run(
            cx,
            move |cancel, on_text| {
                let client = super::client(&ai)?;
                let text = client.complete(&req, cancel, on_text)?;
                prompt::parse_rewrite_reply(&text)
            },
            |this: &mut Self, t, cx| {
                if let State::Working { chars } = &mut this.state {
                    *chars += t.chars().count();
                    cx.notify();
                }
            },
            |this: &mut Self, r: Result<Vec<String>, AiError>, cx| {
                this.cancel = None;
                this.state = match r {
                    Ok(p) => State::Ready(p),
                    Err(e) => State::Failed(e.to_string()),
                };
                cx.notify();
            },
        ));
        cx.notify();
    }

    fn replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (State::Ready(new), Ok((range, old))) = (&self.state, &self.source) else {
            return;
        };
        let (range, old, new) = (range.clone(), old.clone(), new.clone());
        let Some(docs) = self.docs.upgrade() else {
            cx.emit(PanelEvent::Close);
            return;
        };
        match docs.update(cx, |v, cx| {
            v.replace_paragraphs_doc(range, &old, &new, window, cx)
        }) {
            Ok(()) => cx.emit(PanelEvent::Close),
            Err(e) => {
                self.state = State::Failed(e);
                cx.notify();
            }
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.cancel.take() {
            c.cancel();
        }
        cx.emit(PanelEvent::Close);
    }

    fn status(&self) -> Status {
        match &self.state {
            State::Idle | State::Ready(_) => Status::None,
            State::Working { chars: 0 } => Status::Busy(format!("Asking {}…", self.provider)),
            State::Working { chars } => Status::Busy(format!("Receiving… ({chars} characters)")),
            State::Failed(e) => Status::Error(e.clone()),
        }
    }
}

impl Render for RewritePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = kantor_ui::tokens(cx);
        let busy = matches!(self.state, State::Working { .. });
        let ready = matches!(self.state, State::Ready(_));
        let subtitle = match &self.source {
            Ok((r, _)) if r.len() == 1 => Some("1 paragraph".to_owned()),
            Ok((r, _)) => Some(format!("{} paragraphs", r.len())),
            Err(_) => None,
        };

        let mut body = v_flex().p(SPACE_7).gap(SPACE_5);
        if !self.enabled {
            body = body.child(note(
                "AI features are off. Turn them on and choose a provider in AI Settings.",
                cx,
            ));
        } else {
            match &self.source {
                Err(e) => {
                    body = body.child(
                        div()
                            .text_size(FONT_SMALL)
                            .text_color(t.danger)
                            .child(format!("Can't rewrite this: {e}.")),
                    );
                }
                Ok(_) => {
                    let mut modes = h_flex().flex_wrap().gap(SPACE_3);
                    for (i, mode) in RewriteMode::ALL.into_iter().enumerate() {
                        let selected = self.mode == Some(mode);
                        let mut b = Button::new(("ai-mode", i))
                            .small()
                            .label(mode.label())
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| this.run_mode(mode, cx)));
                        b = if selected { b.primary() } else { b.outline() };
                        modes = modes.child(b);
                    }
                    body = body.child(modes).child(note(
                        format!(
                            "Sends only the selected paragraphs' text to {}. Replace keeps \
                             paragraph styles; formatting inside a paragraph becomes that of its \
                             first word.",
                            self.provider
                        ),
                        cx,
                    ));
                }
            }
            if let State::Ready(paras) = &self.state {
                body = body.child(
                    result_box(cx)
                        .id("ai-rewrite-preview")
                        .max_h(PREVIEW_MAX_HEIGHT)
                        .overflow_y_scroll()
                        .child(
                            v_flex()
                                .gap(SPACE_4)
                                .children(paras.iter().map(|p| div().child(p.clone()))),
                        ),
                );
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
            buttons = buttons
                .child(
                    Button::new("ai-close")
                        .ghost()
                        .small()
                        .label(if busy { "Cancel" } else { "Close" })
                        .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                )
                .child(
                    Button::new("ai-replace")
                        .primary()
                        .small()
                        .label("Replace")
                        .disabled(!ready)
                        .on_click(cx.listener(|this, _, window, cx| this.replace(window, cx))),
                );
        }

        div()
            .id("ai-rewrite")
            .key_context(AI_PANEL)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ClosePanel, _, cx| this.close(cx)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                card("Rewrite selection", subtitle, cx)
                    .child(body)
                    .child(buttons),
            )
    }
}
