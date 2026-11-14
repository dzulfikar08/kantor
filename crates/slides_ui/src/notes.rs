//! The speaker-notes pane under the canvas (a resizable text area showing
//! the current slide's notes) and the notes strip in presenter mode.
//!
//! The text area holds plain text; each change is handed to
//! [`Presentation::set_notes_text`](slides_engine::Presentation::set_notes_text),
//! which replaces only the part that differs (formatting elsewhere is
//! kept) and creates the notes part on the first edit.

use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, tf};
use kantor_ui::{metrics::*, tokens};

use crate::SlidesView;

/// Default and smallest height of the notes pane.
pub(crate) const NOTES_HEIGHT: Pixels = px(96.);
const NOTES_MIN: Pixels = px(48.);

/// The notes pane's state, owned by the view.
pub(crate) struct NotesPane {
    pub input: Entity<TextareaState>,
    pub open: bool,
    pub height: Pixels,
    /// Pointer y and pane height when a resize drag started.
    pub drag: Option<(Pixels, Pixels)>,
    pub focused: bool,
    /// Slide whose notes the text area shows.
    pub shown: Option<usize>,
    _sub: Subscription,
}

impl NotesPane {
    pub fn new(window: &mut Window, cx: &mut Context<SlidesView>) -> Self {
        let input =
            cx.new(|cx| TextareaState::new(window, cx).placeholder(i18n::t("Click to add notes")));
        let sub = cx.subscribe_in(
            &input,
            window,
            |v: &mut SlidesView, input, ev, window, cx| {
                match ev {
                    InputEvent::Change => {
                        let text = input.read(cx).value().to_string();
                        v.notes_changed(text, window, cx);
                    }
                    InputEvent::Focus => {
                        v.notes.focused = true;
                        // Keys like Tab and Ctrl+B now belong to the notes.
                        v.editing = None;
                        v.pres.seal_history();
                        cx.notify();
                    }
                    InputEvent::Blur => v.notes.focused = false,
                    _ => {}
                }
            },
        );
        Self {
            input,
            open: true,
            height: NOTES_HEIGHT,
            drag: None,
            focused: false,
            shown: None,
            _sub: sub,
        }
    }
}

impl SlidesView {
    fn notes_changed(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.pres.is_empty() || self.notes.shown != Some(self.current) {
            return;
        }
        let res = self.pres.set_notes_text(self.current, &text);
        if self.report(res, cx).is_some() {
            self.after_edit(window, cx);
        }
    }

    /// Show / hide the notes pane.
    pub fn toggle_notes_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.notes.open = !self.notes.open;
        if !self.notes.open && self.notes.focused {
            self.focus(window, cx);
        }
        cx.notify();
    }

    pub fn notes_open(&self) -> bool {
        self.notes.open
    }

    /// Put the current slide's notes in the text area when they differ
    /// (another slide, undo, a reopened file); typing keeps them equal.
    pub(crate) fn sync_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pres.is_empty() {
            return;
        }
        let text = self.pres.notes_text(self.current);
        let same_slide = self.notes.shown == Some(self.current);
        if same_slide && self.notes.input.read(cx).value().as_ref() == text {
            return;
        }
        self.notes.shown = Some(self.current);
        self.notes
            .input
            .update(cx, |i, cx| i.set_value(text, window, cx));
    }

    pub(crate) fn on_notes_drag(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some((y0, h0)) = self.notes.drag else {
            return;
        };
        if ev.pressed_button != Some(MouseButton::Left) {
            self.notes.drag = None;
            return;
        }
        let max = (self.viewport.get().size.height + self.notes.height - NOTES_MIN).max(NOTES_MIN);
        self.notes.height = (h0 + (y0 - ev.position.y)).clamp(NOTES_MIN, max);
        cx.notify();
    }

    /// The pane: a resize bar, then the notes text.
    pub(crate) fn render_notes(&self, cx: &mut Context<Self>) -> Option<Div> {
        if !self.notes.open || self.pres.is_empty() {
            return None;
        }
        let t = tokens(cx);
        let locked = self.pres.notes_locked(self.current);
        let dragging = self.notes.drag.is_some();
        let bar = div()
            .id("slides-notes-resize")
            .flex_none()
            .h(SPACE_2)
            .w_full()
            .cursor(CursorStyle::ResizeUpDown)
            .border_t_1()
            .border_color(if dragging { t.accent } else { t.border })
            .hover(|d| d.border_color(t.accent))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|v, ev: &MouseDownEvent, _, cx| {
                    v.notes.drag = Some((ev.position.y, v.notes.height));
                    cx.notify();
                }),
            );
        Some(
            v_flex()
                .flex_none()
                .h(self.notes.height)
                .w_full()
                .bg(t.chrome)
                .child(bar)
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .px(SPACE_6)
                        .pb(SPACE_3)
                        .text_size(FONT_BODY)
                        .child(
                            Textarea::new(&self.notes.input)
                                .h_full()
                                .appearance(false)
                                .bordered(false)
                                .readonly(locked.is_some()),
                        ),
                )
                .when_some(locked, |d, why| {
                    d.child(
                        div()
                            .flex_none()
                            .px(SPACE_6)
                            .pb(SPACE_3)
                            .text_size(FONT_SMALL)
                            .text_color(t.fg_muted)
                            .child(tf("The notes can't be edited: {}", &[&why])),
                    )
                }),
        )
    }

    /// Notes under the slide while presenting (S shows / hides them).
    pub(crate) fn render_presenter_notes(&self, slide: usize, cx: &App) -> Option<Stateful<Div>> {
        if !self.present_notes {
            return None;
        }
        let text = self.pres.notes_text(slide);
        if text.trim().is_empty() {
            return None;
        }
        let t = tokens(cx);
        Some(
            div()
                .id("slides-presenter-notes")
                .flex_none()
                .max_h(relative(0.3))
                .overflow_y_scroll()
                .w_full()
                .px(SPACE_10)
                .py(SPACE_6)
                .bg(t.chrome)
                .border_t_1()
                .border_color(t.border)
                .text_size(FONT_TITLE)
                .text_color(t.fg)
                .children(
                    text.lines()
                        .map(|l| div().child(l.to_owned()))
                        .collect::<Vec<_>>(),
                ),
        )
    }
}
