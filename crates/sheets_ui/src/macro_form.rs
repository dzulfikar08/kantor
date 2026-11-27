//! UserForms a running macro shows (`UserForm1.Show`).
//!
//! The interpreter owns the form (control values, lists, focus) on the
//! macro's worker thread; it sends a [`FormView`] and waits for the user's
//! next step, as for a message box (see `macro_run`). Here the view is drawn
//! as a modal card over the grid, from the form's design: positions and
//! sizes in points from the file (× 4/3 for pixels), the form's and
//! controls' colours, captions and font sizes, in Kantor's look otherwise.
//! What the user does becomes a [`FormInput`]: edits (typed text, a ticked
//! CheckBox, a picked list row) and an action (a click, a focus change,
//! Enter / Esc for the Default / Cancel buttons, the close box). Text typed
//! into a TextBox waits for the next action unless the form's code handles
//! that control's `Change` (then each change goes at once).
//!
//! The same drawing, without any of the handlers, is the Macros dialog's
//! read-only preview of a form module ([`form_preview`]).

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_script::forms::{
    Action, ControlKind, Edit, FormInput, FormView, PictureFormat, ole_rgb,
};
use kantor_ui::i18n::t;
use kantor_ui::tokens::Elevation;
use kantor_ui::{metrics::*, tokens};

use crate::SheetsView;

/// Pixels per point.
const S: f32 = 4.0 / 3.0;
/// A MultiPage's or TabStrip's tab row, in points.
const TAB_ROW: f32 = 18.0;

/// The form being shown: its inputs (kept between steps, so typing isn't
/// interrupted), edits not sent yet, the open drop-down.
pub(crate) struct FormUi {
    pub(crate) name: String,
    /// The last view the macro sent.
    pub(crate) view: FormView,
    inputs: HashMap<usize, (Entity<InputState>, Subscription)>,
    /// Edits waiting for the next action.
    edits: Vec<Edit>,
    open_combo: Option<usize>,
    images: HashMap<usize, Option<Arc<Image>>>,
    pub(crate) focus: FocusHandle,
    /// Set while values are put into inputs (their Change isn't the user's).
    quiet: bool,
}

fn color(ole: u32) -> Hsla {
    rgb(ole_rgb(ole)).into()
}

fn picture_source(p: &kantor_script::forms::Picture) -> Option<Arc<Image>> {
    let format = match p.format {
        PictureFormat::Png => ImageFormat::Png,
        PictureFormat::Jpeg => ImageFormat::Jpeg,
        PictureFormat::Gif => ImageFormat::Gif,
        PictureFormat::Bmp => ImageFormat::Bmp,
        PictureFormat::Icon => ImageFormat::Ico,
        _ => return None,
    };
    Some(Arc::new(Image::from_bytes(format, p.bytes.clone())))
}

/// Whether a control takes typed text in an input.
fn has_input(c: &kantor_script::forms::Control) -> bool {
    c.kind == ControlKind::TextBox || (c.kind == ControlKind::ComboBox && !c.drop_list)
}

impl SheetsView {
    /// A view from the macro: create or update the form's inputs and focus.
    pub(crate) fn form_arrived(
        &mut self,
        view: FormView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(run) = self.macros.run.as_mut() else {
            return;
        };
        let reuse = run
            .form
            .as_ref()
            .is_some_and(|f| f.name.eq_ignore_ascii_case(&view.design.name));
        if !reuse {
            run.form = Some(FormUi {
                name: view.design.name.clone(),
                view: view.clone(),
                inputs: HashMap::new(),
                edits: Vec::new(),
                open_combo: None,
                images: HashMap::new(),
                focus: cx.focus_handle(),
                quiet: false,
            });
        }
        let mut new_inputs = Vec::new();
        {
            let Some(form) = run.form.as_mut() else {
                return;
            };
            form.view = view.clone();
            form.quiet = true;
            for (i, c) in view.design.controls.iter().enumerate() {
                if !has_input(c) {
                    continue;
                }
                let text = view.state[i].text.clone();
                let typed_meanwhile = form
                    .edits
                    .iter()
                    .any(|e| matches!(e, Edit::Text { control, .. } if *control == i));
                match form.inputs.get(&i) {
                    Some((state, _)) => {
                        if !typed_meanwhile && state.read(cx).value().as_ref() != text {
                            state.update(cx, |s, cx| s.set_value(text, window, cx));
                        }
                    }
                    None => new_inputs.push((i, text, c.password_char.is_some())),
                }
            }
        }
        for (i, text, masked) in new_inputs {
            let state = cx.new(|cx| InputState::new(window, cx).masked(masked));
            state.update(cx, |s, cx| s.set_value(text, window, cx));
            let sub = cx.subscribe_in(
                &state,
                window,
                move |this: &mut Self, _, e: &InputEvent, w, cx| {
                    this.on_form_text(i, e, w, cx);
                },
            );
            if let Some(form) = self.macros.run.as_mut().and_then(|r| r.form.as_mut()) {
                form.inputs.insert(i, (state, sub));
            }
        }
        let Some(form) = self.macros.run.as_mut().and_then(|r| r.form.as_mut()) else {
            return;
        };
        form.quiet = false;
        // The focus the macro says (SetFocus, a cancelled Exit).
        let target = view
            .focus
            .and_then(|i| form.inputs.get(&i))
            .map(|(s, _)| s.clone());
        let card = form.focus.clone();
        match target {
            Some(state) => {
                if !state.read(cx).focus_handle(cx).is_focused(window) {
                    state.update(cx, |s, cx| s.focus(window, cx));
                }
            }
            None => window.focus(&card, cx),
        }
        cx.notify();
    }

    /// The form closed (hidden or unloaded): forget its inputs.
    pub(crate) fn form_gone(&mut self, name: &str) {
        if let Some(run) = self.macros.run.as_mut()
            && run
                .form
                .as_ref()
                .is_some_and(|f| f.name.eq_ignore_ascii_case(name))
        {
            run.form = None;
        }
    }

    fn form_mut(&mut self) -> Option<&mut FormUi> {
        self.macros.run.as_mut()?.form.as_mut()
    }

    /// Whether the macro waits for this form's answer (not a MsgBox shown
    /// over it).
    fn form_waiting(&self) -> bool {
        self.macros.run.as_ref().is_some_and(|r| r.waits_for_form())
    }

    /// Sends the pending edits and `action`.
    fn form_send(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        if !self.form_waiting() {
            return;
        }
        let Some(form) = self.form_mut() else {
            return;
        };
        form.open_combo = None;
        let edits = std::mem::take(&mut form.edits);
        self.answer_form(FormInput { edits, action }, window, cx);
    }

    /// An edit: sent now (live controls, lists, buttons), as VBA fires the
    /// events right away.
    fn form_edit(&mut self, edit: Edit, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.form_mut() else {
            return;
        };
        form.edits.push(edit);
        self.form_send(Action::None, window, cx);
    }

    fn on_form_text(
        &mut self,
        i: usize,
        e: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = self.form_mut() else {
            return;
        };
        if form.quiet {
            return;
        }
        let Some((state, _)) = form.inputs.get(&i) else {
            return;
        };
        let state = state.clone();
        match e {
            InputEvent::Change => {
                let text = state.read(cx).value().to_string();
                let old = form
                    .view
                    .state
                    .get(i)
                    .map(|s| s.text.clone())
                    .unwrap_or_default();
                let key_press = form.view.key_press.contains(&i);
                let live = form.view.live.contains(&i);
                // One character typed into a control with a KeyPress
                // handler: the macro decides whether it goes in.
                if key_press
                    && text.starts_with(&old)
                    && text.chars().count() == old.chars().count() + 1
                    && let Some(ch) = text.chars().last()
                {
                    form.quiet = true;
                    state.update(cx, |s, cx| s.set_value(old, window, cx));
                    if let Some(form) = self.form_mut() {
                        form.quiet = false;
                    }
                    self.form_send(Action::KeyPress { control: i, ch }, window, cx);
                    return;
                }
                form.edits
                    .retain(|e| !matches!(e, Edit::Text { control, .. } if *control == i));
                form.edits.push(Edit::Text { control: i, text });
                if live {
                    self.form_send(Action::None, window, cx);
                }
            }
            InputEvent::Focus => {
                if form.view.focus != Some(i) {
                    self.form_send(Action::Focus(i), window, cx);
                }
            }
            InputEvent::PressEnter { .. } => {
                let multi = form
                    .view
                    .design
                    .controls
                    .get(i)
                    .is_some_and(|c| c.multi_line);
                if !multi {
                    self.form_send(Action::DefaultKey, window, cx);
                }
            }
            _ => {}
        }
    }

    /// The form card (and its scrim).
    pub(crate) fn render_form(&self, form: &FormUi, cx: &mut Context<Self>) -> AnyElement {
        let t_ = tokens(cx);
        let v = &form.view;
        let d = &v.design;
        let w = d.width * S;
        let h = d.height * S;
        let title = h_flex()
            .h(px(28.))
            .px(SPACE_4)
            .gap(SPACE_3)
            .border_b_1()
            .border_color(t_.border)
            .bg(t_.surface)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t_.fg)
                    .child(SharedString::from(d.caption.clone())),
            )
            .child(
                div()
                    .id("macro-form-close")
                    .flex_none()
                    .size(px(22.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(RADIUS_SM)
                    .cursor_pointer()
                    .hover(|s| s.bg(t_.hover))
                    .text_color(t_.fg_muted)
                    .child(Icon::new(Lucide::X).with_size(px(14.)))
                    .on_click(cx.listener(|this, _: &ClickEvent, w, cx| {
                        this.form_send(Action::Close, w, cx);
                    })),
            );
        let client = div()
            .id("macro-form-client")
            .relative()
            .w(px(w))
            .h(px(h))
            .overflow_hidden()
            .bg(color(d.back_color))
            .text_color(color(d.fore_color))
            .text_size(px(d.font.size * S))
            // Clicks on the form itself (controls stop theirs).
            .on_click(cx.listener(|this, _: &ClickEvent, w, cx| {
                this.form_send(Action::FormClick, w, cx);
            }))
            .children(
                d.children(None)
                    .into_iter()
                    .map(|i| self.render_form_control(form, i, cx)),
            );
        let card = v_flex()
            .id("macro-form")
            .track_focus(&form.focus)
            .key_context("KantorMacroForm")
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                match e.keystroke.key.as_str() {
                    "escape" => {
                        cx.stop_propagation();
                        let open = this.form_mut().and_then(|f| f.open_combo.take());
                        if open.is_some() {
                            cx.notify();
                        } else {
                            this.form_send(Action::CancelKey, window, cx);
                        }
                    }
                    "enter" => {
                        cx.stop_propagation();
                        this.form_send(Action::DefaultKey, window, cx);
                    }
                    _ => {}
                }
            }))
            .overflow_hidden()
            .rounded(RADIUS_LG)
            .border_1()
            .border_color(t_.border)
            .shadow(t_.shadow(Elevation::Modal))
            .text_size(FONT_SMALL)
            .child(title)
            .child(client);
        let card = match v.position {
            Some((x, y)) if d.start_up_position == 0 => div()
                .absolute()
                .left(px(x * S))
                .top(px(y * S))
                .child(card)
                .into_any_element(),
            _ => card.into_any_element(),
        };
        div()
            .id("macro-form-scrim")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .when(v.modal, |d| d.bg(t_.scrim))
            .child(card)
            .into_any_element()
    }

    fn render_form_control(&self, form: &FormUi, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let v = &form.view;
        let c = &v.design.controls[i];
        if !v.shown(i) {
            return div().into_any_element();
        }
        let t_ = tokens(cx);
        let s = &v.state[i];
        let enabled = c.enabled;
        let (x, y, w, h) = (c.left * S, c.top * S, c.width * S, c.height * S);
        let base = div()
            .id(("macro-form-control", i))
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(w))
            .h(px(h))
            .text_size(px(c.font.size * S))
            .when(c.font.bold, |d| d.font_weight(FontWeight::BOLD))
            .when(c.font.italic, |d| d.italic())
            .when(c.font.underline, |d| d.underline())
            .when(c.font.strikeout, |d| d.line_through())
            .when(!enabled, |d| d.opacity(0.5));
        let aligned = |d: Stateful<Div>| match c.text_align {
            2 => d.justify_end().text_right(),
            3 => d.justify_center().text_center(),
            _ => d,
        };
        let caption = SharedString::from(c.caption.clone());
        match c.kind {
            ControlKind::Label => aligned(
                base.flex()
                    .when(c.back_opaque, |d| d.bg(color(c.back_color)))
                    .when(c.border_style == 1, |d| {
                        d.border_1().border_color(color(c.border_color))
                    })
                    .text_color(color(c.fore_color))
                    .overflow_hidden()
                    .when(!c.word_wrap, |d| d.whitespace_nowrap())
                    .child(caption),
            )
            .when(enabled, |d| {
                d.on_click(cx.listener(move |this, _: &ClickEvent, w, cx| {
                    cx.stop_propagation();
                    this.form_send(Action::Click(i), w, cx);
                }))
            })
            .into_any_element(),
            ControlKind::CommandButton | ControlKind::ToggleButton => {
                let pressed = c.kind == ControlKind::ToggleButton && s.checked == Some(true);
                let focused = v.focus == Some(i);
                base.flex()
                    .items_center()
                    .justify_center()
                    .px(px(4.))
                    .rounded(RADIUS_MD)
                    .border_1()
                    .border_color(if c.default || focused {
                        t_.accent
                    } else {
                        t_.border_strong
                    })
                    .bg(if pressed {
                        t_.selected
                    } else {
                        color(c.back_color)
                    })
                    .text_color(color(c.fore_color))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .when(enabled, |d| {
                        d.cursor_pointer()
                            .hover(|s| s.bg(t_.hover))
                            .on_click(cx.listener(move |this, _: &ClickEvent, w, cx| {
                                cx.stop_propagation();
                                let action = this.form_mut().map(|f| {
                                    let c = &f.view.design.controls[i];
                                    let s = &f.view.state[i];
                                    if c.kind == ControlKind::ToggleButton {
                                        Some(Edit::Checked {
                                            control: i,
                                            checked: Some(s.checked != Some(true)),
                                        })
                                    } else {
                                        None
                                    }
                                });
                                match action {
                                    Some(Some(edit)) => this.form_edit(edit, w, cx),
                                    Some(None) => this.form_send(Action::Click(i), w, cx),
                                    None => {}
                                }
                            }))
                    })
                    .child(caption)
                    .into_any_element()
            }
            ControlKind::CheckBox | ControlKind::OptionButton => {
                let round = c.kind == ControlKind::OptionButton;
                let mark = match s.checked {
                    Some(true) => {
                        if round {
                            "●"
                        } else {
                            "✓"
                        }
                    }
                    Some(false) => "",
                    None => "■",
                };
                let box_ = div()
                    .flex_none()
                    .size(px(13.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(t_.surface)
                    .border_1()
                    .border_color(t_.border_strong)
                    .when(round, |d| d.rounded_full())
                    .when(!round, |d| d.rounded(px(2.)))
                    .text_size(px(10.))
                    .text_color(t_.accent)
                    .child(mark);
                base.flex()
                    .items_center()
                    .gap(px(5.))
                    .when(c.back_opaque, |d| d.bg(color(c.back_color)))
                    .text_color(color(c.fore_color))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .when(c.caption_left, |d| d.flex_row_reverse().justify_end())
                    .child(box_)
                    .child(caption)
                    .when(enabled && !c.locked, |d| {
                        d.cursor_pointer().on_click(cx.listener(
                            move |this, _: &ClickEvent, w, cx| {
                                cx.stop_propagation();
                                let edit = this.form_mut().map(|f| {
                                    let s = &f.view.state[i];
                                    let round =
                                        f.view.design.controls[i].kind == ControlKind::OptionButton;
                                    Edit::Checked {
                                        control: i,
                                        checked: Some(round || s.checked != Some(true)),
                                    }
                                });
                                if let Some(edit) = edit {
                                    this.form_edit(edit, w, cx);
                                }
                            },
                        ))
                    })
                    .into_any_element()
            }
            ControlKind::TextBox => match form.inputs.get(&i) {
                Some((state, _)) => base
                    .child(
                        Input::new(state)
                            .small()
                            .h(px(h))
                            .disabled(!enabled)
                            .readonly(c.locked),
                    )
                    .into_any_element(),
                None => base.into_any_element(),
            },
            ControlKind::ComboBox => self.render_form_combo(form, i, base, cx),
            ControlKind::ListBox => self.render_form_list(form, i, base, cx),
            ControlKind::Frame => base
                .border_1()
                .border_color(t_.border_strong)
                .rounded(RADIUS_SM)
                .when(c.back_opaque, |d| d.bg(color(c.back_color)))
                .child(
                    div()
                        .absolute()
                        .left(px(6.))
                        .top(px(-8.))
                        .px(px(2.))
                        .bg(color(c.back_color))
                        .text_color(color(c.fore_color))
                        .whitespace_nowrap()
                        .child(caption),
                )
                .children(
                    v.design
                        .children(Some(i))
                        .into_iter()
                        .map(|k| self.render_form_control(form, k, cx)),
                )
                .into_any_element(),
            ControlKind::MultiPage | ControlKind::TabStrip => {
                let tabs: Vec<String> = if c.kind == ControlKind::MultiPage {
                    v.design
                        .children(Some(i))
                        .iter()
                        .map(|&p| v.design.controls[p].caption.clone())
                        .collect()
                } else {
                    c.tabs.clone()
                };
                let row = h_flex()
                    .h(px(TAB_ROW * S))
                    .children(tabs.into_iter().enumerate().map(|(n, label)| {
                        let on = s.number == n as i32;
                        div()
                            .id(("macro-form-tab", i * 1000 + n))
                            .h_full()
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .border_1()
                            .border_color(t_.border)
                            .rounded_t(RADIUS_SM)
                            .when(on, |d| d.bg(t_.elevated).font_weight(FontWeight::MEDIUM))
                            .when(!on, |d| d.bg(t_.surface))
                            .cursor_pointer()
                            .child(SharedString::from(label))
                            .when(enabled, |d| {
                                d.on_click(cx.listener(move |this, _: &ClickEvent, w, cx| {
                                    cx.stop_propagation();
                                    this.form_edit(
                                        Edit::Number {
                                            control: i,
                                            value: n as i32,
                                        },
                                        w,
                                        cx,
                                    );
                                }))
                            })
                    }));
                let pages = if c.kind == ControlKind::MultiPage {
                    let selected = v
                        .design
                        .children(Some(i))
                        .get(s.number.max(0) as usize)
                        .copied();
                    selected
                        .map(|p| {
                            v.design
                                .children(Some(p))
                                .into_iter()
                                .map(|k| self.render_form_control(form, k, cx))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                base.flex()
                    .flex_col()
                    .child(row)
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .border_1()
                            .border_color(t_.border)
                            .bg(color(c.back_color))
                            .children(pages),
                    )
                    .into_any_element()
            }
            ControlKind::Image => {
                let source = form
                    .images
                    .get(&i)
                    .cloned()
                    .unwrap_or_else(|| c.picture.as_ref().and_then(picture_source));
                let fit = match c.picture_size_mode {
                    1 => ObjectFit::Fill,
                    3 => ObjectFit::Contain,
                    _ => ObjectFit::None,
                };
                base.overflow_hidden()
                    .when(c.back_opaque, |d| d.bg(color(c.back_color)))
                    .when(c.border_style == 1, |d| {
                        d.border_1().border_color(color(c.border_color))
                    })
                    .children(source.map(|src| img(src).size_full().object_fit(fit)))
                    .when(enabled, |d| {
                        d.on_click(cx.listener(move |this, _: &ClickEvent, w, cx| {
                            cx.stop_propagation();
                            this.form_send(Action::Click(i), w, cx);
                        }))
                    })
                    .into_any_element()
            }
            ControlKind::SpinButton | ControlKind::ScrollBar => {
                let horizontal = match c.orientation {
                    0 => false,
                    1 => true,
                    _ => w > h,
                };
                let step = |sign: i32, large: bool| {
                    let c = c.clone();
                    let pos = s.number;
                    move |this: &mut Self,
                          _: &ClickEvent,
                          w: &mut Window,
                          cx: &mut Context<Self>| {
                        cx.stop_propagation();
                        let by = if large {
                            c.large_change
                        } else {
                            c.small_change
                        };
                        let (lo, hi) = (c.min.min(c.max), c.min.max(c.max));
                        let dir = if c.max >= c.min { sign } else { -sign };
                        let value = (pos + dir * by).clamp(lo, hi);
                        this.form_edit(Edit::Number { control: i, value }, w, cx);
                    }
                };
                let arrow = |id: usize, glyph: &'static str, sign: i32| {
                    div()
                        .id(("macro-form-arrow", i * 4 + id))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(horizontal, |d| d.w(px(h.min(16.))).h_full())
                        .when(!horizontal, |d| d.h(px(w.min(16.))).w_full())
                        .bg(t_.surface)
                        .border_1()
                        .border_color(t_.border)
                        .text_size(px(8.))
                        .text_color(t_.fg_muted)
                        .child(glyph)
                        .when(enabled, |d| {
                            d.cursor_pointer().on_click(cx.listener(step(sign, false)))
                        })
                };
                let (back, fwd) = if horizontal {
                    ("◀", "▶")
                } else {
                    ("▲", "▼")
                };
                let back_arrow = arrow(0, back, -1);
                let fwd_arrow = arrow(1, fwd, 1);
                let mut el = base
                    .flex()
                    .when(!horizontal, |d| d.flex_col())
                    .bg(t_.surface)
                    .child(back_arrow);
                if c.kind == ControlKind::ScrollBar {
                    let span = (c.max - c.min).abs().max(1) as f32;
                    let at = ((s.number - c.min.min(c.max)) as f32 / span).clamp(0.0, 1.0);
                    let track = div().relative().flex_1().child(
                        div()
                            .absolute()
                            .bg(t_.border_strong)
                            .rounded(px(2.))
                            .when(horizontal, |d| {
                                d.top(px(2.))
                                    .bottom(px(2.))
                                    .w(px(12.))
                                    .left(relative(at * 0.9))
                            })
                            .when(!horizontal, |d| {
                                d.left(px(2.))
                                    .right(px(2.))
                                    .h(px(12.))
                                    .top(relative(at * 0.9))
                            }),
                    );
                    let track = h_flex().flex_1().size_full().child(track);
                    let back_page = div()
                        .id(("macro-form-page", i * 2))
                        .absolute()
                        .left_0()
                        .top_0()
                        .when(horizontal, |d| d.h_full().w(relative(at)))
                        .when(!horizontal, |d| d.w_full().h(relative(at)))
                        .when(enabled, |d| d.on_click(cx.listener(step(-1, true))));
                    let fwd_page = div()
                        .id(("macro-form-page", i * 2 + 1))
                        .absolute()
                        .right_0()
                        .bottom_0()
                        .when(horizontal, |d| d.h_full().w(relative(1.0 - at)))
                        .when(!horizontal, |d| d.w_full().h(relative(1.0 - at)))
                        .when(enabled, |d| d.on_click(cx.listener(step(1, true))));
                    el = el.child(
                        div()
                            .relative()
                            .flex_1()
                            .child(track)
                            .child(back_page)
                            .child(fwd_page),
                    );
                }
                el.child(fwd_arrow).into_any_element()
            }
            ControlKind::Page | ControlKind::Other(_) => {
                base.border_1().border_color(t_.border).into_any_element()
            }
        }
    }

    fn render_form_combo(
        &self,
        form: &FormUi,
        i: usize,
        base: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t_ = tokens(cx);
        let v = &form.view;
        let c = &v.design.controls[i];
        let s = &v.state[i];
        let enabled = c.enabled && !c.locked;
        let field: AnyElement = match form.inputs.get(&i) {
            Some((state, _)) => div()
                .flex_1()
                .min_w_0()
                .child(
                    Input::new(state)
                        .small()
                        .h(px(c.height * S))
                        .disabled(!enabled),
                )
                .into_any_element(),
            None => div()
                .flex_1()
                .min_w_0()
                .px(px(4.))
                .overflow_hidden()
                .whitespace_nowrap()
                .bg(color(c.back_color))
                .text_color(color(c.fore_color))
                .child(SharedString::from(s.text.clone()))
                .into_any_element(),
        };
        let open = form.open_combo == Some(i);
        let toggle = div()
            .id(("macro-form-drop", i))
            .flex_none()
            .w(px(16.))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(t_.surface)
            .border_l_1()
            .border_color(t_.border)
            .child(Icon::new(Lucide::ChevronDown).with_size(px(12.)))
            .when(enabled, |d| {
                d.cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        if let Some(f) = this.form_mut() {
                            f.open_combo = if f.open_combo == Some(i) {
                                None
                            } else {
                                Some(i)
                            };
                            cx.notify();
                        }
                    }))
            });
        let rows = c.list_rows.clamp(1, 20) as usize;
        let list = open.then(|| {
            deferred(
                v_flex()
                    .id(("macro-form-drop-list", i))
                    // A pick mustn't also reach the control under it.
                    .occlude()
                    .absolute()
                    .top(px(c.height * S))
                    .left_0()
                    .w(px(c.width * S))
                    .max_h(px(rows as f32 * 18.0))
                    .overflow_y_scroll()
                    .bg(t_.elevated)
                    .border_1()
                    .border_color(t_.border_strong)
                    .shadow(t_.shadow(Elevation::Popover))
                    .children(s.list.iter().enumerate().map(|(n, _)| {
                        let label = SharedString::from(v.row_text(i, n));
                        div()
                            .id(("macro-form-drop-item", i * 10_000 + n))
                            .px(px(4.))
                            .h(px(18.))
                            .flex()
                            .items_center()
                            .whitespace_nowrap()
                            .when(s.list_index == n as i32, |d| d.bg(t_.selected))
                            .hover(|d| d.bg(t_.hover))
                            .cursor_pointer()
                            .child(label)
                            .on_click(cx.listener(move |this, _: &ClickEvent, w, cx| {
                                cx.stop_propagation();
                                this.form_edit(
                                    Edit::ListIndex {
                                        control: i,
                                        index: n as i32,
                                    },
                                    w,
                                    cx,
                                );
                            }))
                    })),
            )
            .with_priority(1)
        });
        base.flex()
            .border_1()
            .border_color(t_.border_strong)
            .bg(color(c.back_color))
            .child(field)
            .child(toggle)
            .children(list)
            .into_any_element()
    }

    fn render_form_list(
        &self,
        form: &FormUi,
        i: usize,
        base: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t_ = tokens(cx);
        let v = &form.view;
        let c = &v.design.controls[i];
        let s = &v.state[i];
        let enabled = c.enabled && !c.locked;
        let cols = c.column_count.max(1) as usize;
        let col_w = c.width * S / cols as f32;
        let multi = c.multi_select != 0;
        let rows = v_flex()
            .id(("macro-form-list-rows", i))
            .size_full()
            .overflow_y_scroll()
            .children(s.list.iter().enumerate().map(|(n, row)| {
                let on = if multi {
                    s.selected.get(n).copied().unwrap_or(false)
                } else {
                    s.list_index == n as i32
                };
                h_flex()
                    .id(("macro-form-list-row", i * 100_000 + n))
                    .flex_none()
                    .h(px(c.font.size * S * 1.5))
                    .when(on, |d| d.bg(t_.selected))
                    .when(multi && s.list_index == n as i32, |d| {
                        d.border_1().border_color(t_.accent)
                    })
                    .children((0..cols).map(|k| {
                        div()
                            .w(px(col_w))
                            .px(px(3.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(SharedString::from(row.get(k).cloned().unwrap_or_default()))
                    }))
                    .when(enabled, |d| {
                        d.cursor_pointer().on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, e: &MouseDownEvent, w, cx| {
                                cx.stop_propagation();
                                if e.click_count == 2 {
                                    this.form_send(Action::DblClick(i), w, cx);
                                    return;
                                }
                                let edit = this.form_mut().map(|f| {
                                    let c = &f.view.design.controls[i];
                                    let s = &f.view.state[i];
                                    if c.multi_select != 0 {
                                        Edit::Selected {
                                            control: i,
                                            row: n,
                                            selected: !s.selected.get(n).copied().unwrap_or(false),
                                        }
                                    } else {
                                        Edit::ListIndex {
                                            control: i,
                                            index: n as i32,
                                        }
                                    }
                                });
                                if let Some(edit) = edit {
                                    this.form_edit(edit, w, cx);
                                }
                            }),
                        )
                    })
            }));
        base.border_1()
            .border_color(t_.border_strong)
            .bg(color(c.back_color))
            .text_color(color(c.fore_color))
            .child(rows)
            .into_any_element()
    }
}

/// A read-only drawing of a form's design (the Macros dialog's preview of a
/// form module): the controls as boxes with their captions, scaled to fit
/// `max_width` pixels.
pub(crate) fn form_preview(
    design: &kantor_script::forms::FormDesign,
    max_width: f32,
    cx: &App,
) -> AnyElement {
    let t_ = tokens(cx);
    let scale = (max_width / (design.width * S).max(1.0)).min(1.0) * S;
    fn node(
        design: &kantor_script::forms::FormDesign,
        i: usize,
        scale: f32,
        t_: &kantor_ui::tokens::Tokens,
    ) -> AnyElement {
        let c = &design.controls[i];
        let label = match c.kind {
            ControlKind::TextBox | ControlKind::ComboBox | ControlKind::ListBox => {
                if c.value.is_empty() {
                    c.name.clone()
                } else {
                    c.value.clone()
                }
            }
            ControlKind::Image | ControlKind::SpinButton | ControlKind::ScrollBar => String::new(),
            _ => c.caption.clone(),
        };
        let boxed = matches!(
            c.kind,
            ControlKind::TextBox
                | ControlKind::ComboBox
                | ControlKind::ListBox
                | ControlKind::CommandButton
                | ControlKind::ToggleButton
                | ControlKind::Frame
                | ControlKind::MultiPage
                | ControlKind::TabStrip
                | ControlKind::Image
                | ControlKind::SpinButton
                | ControlKind::ScrollBar
                | ControlKind::Other(_)
        );
        let children: Vec<usize> = match c.kind {
            // A MultiPage shows its first page.
            ControlKind::MultiPage => design
                .children(Some(i))
                .first()
                .map(|&p| design.children(Some(p)))
                .unwrap_or_default(),
            _ => design.children(Some(i)),
        };
        div()
            .absolute()
            .left(px(c.left * scale))
            .top(px(c.top * scale))
            .w(px(c.width * scale))
            .h(px(c.height * scale))
            .overflow_hidden()
            .text_size(px((c.font.size * scale).max(6.0)))
            .when(c.font.bold, |d| d.font_weight(FontWeight::BOLD))
            .when(!c.visible, |d| d.opacity(0.4))
            .when(boxed, |d| d.border_1().border_color(t_.border_strong))
            .when(c.kind == ControlKind::CommandButton, |d| {
                d.rounded(px(3.)).flex().items_center().justify_center()
            })
            .when(c.back_opaque && c.kind != ControlKind::Frame, |d| {
                d.bg(color(c.back_color))
            })
            .text_color(color(c.fore_color))
            .whitespace_nowrap()
            .child(SharedString::from(label))
            .children(children.into_iter().map(|k| node(design, k, scale, t_)))
            .into_any_element()
    }
    v_flex()
        .flex_none()
        .border_1()
        .border_color(t_.border)
        .rounded(RADIUS_SM)
        .overflow_hidden()
        .child(
            div()
                .px(SPACE_3)
                .py(px(2.))
                .bg(t_.surface)
                .border_b_1()
                .border_color(t_.border)
                .text_size(FONT_SMALL)
                .text_color(t_.fg)
                .child(SharedString::from(design.caption.clone())),
        )
        .child(
            div()
                .relative()
                .w(px(design.width * scale))
                .h(px(design.height * scale))
                .bg(color(design.back_color))
                .children(
                    design
                        .children(None)
                        .into_iter()
                        .map(|i| node(design, i, scale, &t_)),
                ),
        )
        .child(
            div()
                .px(SPACE_3)
                .py(px(2.))
                .text_size(FONT_SMALL)
                .text_color(t_.fg_muted)
                .child(SharedString::from(crate::macro_form::preview_note(design))),
        )
        .into_any_element()
}

/// "8 controls, 360 × 270 pt".
pub(crate) fn preview_note(design: &kantor_script::forms::FormDesign) -> String {
    kantor_ui::i18n::tf(
        "{} controls, {} × {} pt",
        &[
            &design.controls.len(),
            &design.width.round(),
            &design.height.round(),
        ],
    )
}

/// A form that can't be drawn: why.
pub(crate) fn preview_unavailable(why: &str, cx: &App) -> AnyElement {
    let t_ = tokens(cx);
    div()
        .text_size(FONT_SMALL)
        .text_color(t_.fg_muted)
        .child(SharedString::from(format!(
            "{} {why}",
            t("The form's layout can't be shown:")
        )))
        .into_any_element()
}
