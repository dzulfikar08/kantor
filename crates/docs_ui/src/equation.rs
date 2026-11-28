//! Insert ▸ Equation (Docs: Alt+=; Slides too): a popover with a LaTeX
//! line (Word's linear format works too: `a/b`, `x^2`), a live preview, a
//! palette of structures and symbols, and an inline / display switch.
//! Double-clicking an equation opens it here as LaTeX. The host saves the
//! result as Office Math, so Word and PowerPoint open it as an editable
//! equation; inserting or changing one is one undo step.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use docs_engine::Position;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{t, tf};
use kantor_ui::tokens::Elevation;
use kantor_ui::{metrics::*, tokens};

use crate::{DocsView, FontBridge};

/// Structures: button label and the LaTeX it inserts.
const STRUCTURES: [(&str, &str); 14] = [
    ("a/b", r"\frac{a}{b}"),
    ("xⁿ", "x^{n}"),
    ("xᵢ", "x_{i}"),
    ("√x", r"\sqrt{x}"),
    ("ⁿ√x", r"\sqrt[n]{x}"),
    ("∑", r"\sum_{i=1}^{n} "),
    ("∫", r"\int_{a}^{b} "),
    ("∏", r"\prod_{i=1}^{n} "),
    ("lim", r"\lim_{x \to 0} "),
    ("(x)", r"\left( x \right)"),
    ("[▦]", r"\begin{bmatrix} a & b \\ c & d \end{bmatrix}"),
    ("{…", r"\begin{cases} a & x > 0 \\ b & x \leq 0 \end{cases}"),
    ("x̂", r"\hat{x}"),
    ("x̄", r"\bar{x}"),
];

/// Symbols: button label and the LaTeX it inserts.
const SYMBOLS: [(&str, &str); 28] = [
    ("α", r"\alpha "),
    ("β", r"\beta "),
    ("γ", r"\gamma "),
    ("δ", r"\delta "),
    ("ε", r"\varepsilon "),
    ("θ", r"\theta "),
    ("λ", r"\lambda "),
    ("μ", r"\mu "),
    ("π", r"\pi "),
    ("σ", r"\sigma "),
    ("φ", r"\varphi "),
    ("ω", r"\omega "),
    ("Δ", r"\Delta "),
    ("Σ", r"\Sigma "),
    ("Ω", r"\Omega "),
    ("±", r"\pm "),
    ("×", r"\times "),
    ("÷", r"\div "),
    ("·", r"\cdot "),
    ("≤", r"\leq "),
    ("≥", r"\geq "),
    ("≠", r"\neq "),
    ("≈", r"\approx "),
    ("∞", r"\infty "),
    ("→", r"\to "),
    ("∈", r"\in "),
    ("∂", r"\partial "),
    ("∇", r"\nabla "),
];

/// Font size of the preview, px.
const PREVIEW_SIZE: f32 = 22.0;

/// What the equation popover asks its host to do.
pub enum EquationEvent {
    /// Save this equation (new, or in place of the one opened).
    Done(kantor_math::Equation),
    /// Closed without saving.
    Cancel,
}

/// The equation popover (see the module docs). The host places it with
/// [`EquationDialog::new`]'s `at` and handles its [`EquationEvent`]s.
pub struct EquationDialog {
    input: Entity<InputState>,
    display: bool,
    editing: bool,
    at: Point<Pixels>,
    bridge: Rc<RefCell<FontBridge>>,
    /// The typed equation laid out, or what's wrong with it.
    preview: Result<Option<Arc<kantor_math::Layout>>, String>,
    /// Why the last Insert didn't go through.
    problem: Option<String>,
    _sub: Subscription,
}

impl EventEmitter<EquationEvent> for EquationDialog {}

fn parse(src: &str, display: bool) -> Result<Option<kantor_math::Equation>, String> {
    if src.trim().is_empty() {
        return Ok(None);
    }
    kantor_math::parse_input(src, display)
        .map(Some)
        .map_err(|e| e.message)
}

impl EquationDialog {
    /// A popover showing `existing` (LaTeX) or empty, its top centre at `at`.
    pub fn new(
        existing: Option<&kantor_math::Equation>,
        display: bool,
        at: Point<Pixels>,
        bridge: Rc<RefCell<FontBridge>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let latex = existing.map(kantor_math::latex::write).unwrap_or_default();
        let display = existing.map_or(display, |e| e.display);
        let input = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_value(latex, window, cx);
            s
        });
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, _, cx| match ev {
            InputEvent::PressEnter { .. } => this.done(cx),
            InputEvent::Change => {
                this.refresh(cx);
                cx.notify();
            }
            _ => {}
        });
        input.update(cx, |s, cx| s.focus(window, cx));
        let mut d = Self {
            input,
            display,
            editing: existing.is_some(),
            at,
            bridge,
            preview: Ok(None),
            problem: None,
            _sub: sub,
        };
        d.refresh(cx);
        d
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let src = self.input.read(cx).value().to_string();
        self.problem = None;
        self.preview = parse(&src, self.display).map(|eq| {
            eq.map(|eq| {
                Arc::new(kantor_math::layout(
                    &eq,
                    &kantor_math::Options {
                        size: PREVIEW_SIZE,
                        ..kantor_math::Options::default()
                    },
                ))
            })
        });
    }

    fn done(&mut self, cx: &mut Context<Self>) {
        let src = self.input.read(cx).value().to_string();
        match parse(&src, self.display) {
            Ok(Some(eq)) => cx.emit(EquationEvent::Done(eq)),
            Ok(None) => self.problem = Some(t("Type an equation first").into()),
            Err(e) => self.problem = Some(tf("Equation: {}", &[&e])),
        }
        cx.notify();
    }
}

impl Render for EquationDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tok = tokens(cx);
        let view = cx.entity().downgrade();
        let close = {
            let v = view.clone();
            move |_: &MouseDownEvent, _: &mut Window, cx: &mut App| {
                let _ = v.update(cx, |_, cx| cx.emit(EquationEvent::Cancel));
            }
        };
        let chip = |id: SharedString, label: &'static str, latex: &'static str| {
            let v = view.clone();
            div()
                .id(id)
                .min_w(CONTROL_SM)
                .h(CONTROL_SM)
                .px(SPACE_2)
                .flex()
                .items_center()
                .justify_center()
                .rounded(RADIUS_SM)
                .border_1()
                .border_color(tok.border)
                .cursor_pointer()
                .hover(|s| s.bg(tok.hover))
                .child(label)
                .on_click(move |_, window, cx| {
                    let _ = v.update(cx, |this, cx| {
                        let input = this.input.clone();
                        input.update(cx, |s, cx| {
                            s.insert(latex, window, cx);
                            s.focus(window, cx);
                        });
                        this.refresh(cx);
                        cx.notify();
                    });
                })
        };
        let mut structures = h_flex().flex_wrap().gap(SPACE_1);
        for (k, (label, latex)) in STRUCTURES.iter().enumerate() {
            structures = structures.child(chip(format!("eq-s{k}").into(), label, latex));
        }
        let mut symbols = h_flex().flex_wrap().gap(SPACE_1);
        for (k, (label, latex)) in SYMBOLS.iter().enumerate() {
            symbols = symbols.child(chip(format!("eq-y{k}").into(), label, latex));
        }
        let toggle = |id: &'static str, label: &'static str, display: bool| {
            let v = view.clone();
            let on = self.display == display;
            div()
                .id(id)
                .h(CONTROL_SM)
                .px(SPACE_3)
                .flex()
                .items_center()
                .rounded(RADIUS_MD)
                .cursor_pointer()
                .when(on, |d| d.bg(tok.selected))
                .when(!on, |d| d.hover(|s| s.bg(tok.hover)))
                .child(label)
                .on_click(move |_, _, cx| {
                    let _ = v.update(cx, |this, cx| {
                        this.display = display;
                        this.refresh(cx);
                        cx.notify();
                    });
                })
        };
        let preview: AnyElement = match &self.preview {
            Ok(Some(l)) => {
                let l = l.clone();
                let bridge = self.bridge.clone();
                let ink = tok.paper_ink;
                let w = l.width.max(1.0);
                let h = l.height().max(1.0);
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        paint_layout(&l, &bridge, bounds.origin, ink, window);
                    },
                )
                .w(px(w))
                .h(px(h))
                .into_any_element()
            }
            Ok(None) => div()
                .text_color(tok.fg_dim)
                .child(t("Type LaTeX such as \\frac{a}{b}, or a/b"))
                .into_any_element(),
            Err(e) => div()
                .text_color(tok.danger)
                .child(e.clone())
                .into_any_element(),
        };
        let ok = {
            let v = view.clone();
            div()
                .id("eq-ok")
                .h(CONTROL_SM)
                .px(SPACE_4)
                .flex()
                .items_center()
                .rounded(RADIUS_MD)
                .cursor_pointer()
                .bg(tok.accent)
                .text_color(tok.accent_fg)
                .child(if self.editing {
                    t("Update")
                } else {
                    t("Insert")
                })
                .on_click(move |_, _, cx| {
                    let _ = v.update(cx, |this, cx| this.done(cx));
                })
        };
        let body = v_flex()
            .id("equation-dialog")
            .key_context(crate::POPOVER_CONTEXT)
            .on_action(|_: &crate::PopoverKey, _, _| {})
            .occlude()
            .w(REM * 34.)
            .gap(SPACE_3)
            .p(SPACE_4)
            .bg(tok.elevated)
            .border_1()
            .border_color(tok.border)
            .rounded(RADIUS_LG)
            .shadow(tok.shadow(Elevation::Popover))
            .text_size(FONT_SMALL)
            .text_color(tok.fg)
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t("Equation")),
                    )
                    .child(toggle("eq-inline", t("Inline"), false))
                    .child(toggle("eq-display", t("Display"), true)),
            )
            .child(Input::new(&self.input).small())
            .child(
                div()
                    .min_h(REM * 4.)
                    .p(SPACE_3)
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .rounded(RADIUS_MD)
                    .bg(tok.paper)
                    .border_1()
                    .border_color(tok.border)
                    .child(preview),
            )
            .children(
                self.problem
                    .clone()
                    .map(|p| div().text_color(tok.danger).child(p)),
            )
            .child(div().text_color(tok.fg_muted).child(t("Structures")))
            .child(structures)
            .child(div().text_color(tok.fg_muted).child(t("Symbols")))
            .child(symbols)
            .child(h_flex().child(div().flex_1()).child(ok));
        kantor_ui::overlay::popover(
            anchored()
                .position(self.at)
                .anchor(Anchor::TopCenter)
                .snap_to_window_with_margin(SPACE_4)
                .child(body.on_mouse_down_out(close)),
        )
    }
}

/// Docs' open equation popover and the equation it changes.
pub(crate) struct DocsEquation {
    dialog: Entity<EquationDialog>,
    /// The equation being changed (`None`: a new one at the selection).
    target: Option<Position>,
    _sub: Subscription,
}

impl DocsView {
    /// Insert ▸ Equation, or edit the equation selected or at the caret.
    pub fn open_equation_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.doc.equation_near(self.sel);
        let existing = target.and_then(|p| self.doc.equation_at(p)).map(|(_, e)| e);
        // A new equation alone in its paragraph is display math.
        let display = self.doc.paragraph(self.sel.head.para).text.is_empty();
        let at = {
            let b = self.viewport.get();
            point(b.origin.x + b.size.width / 2., b.origin.y + SPACE_7)
        };
        let bridge = self.bridge.clone();
        let dialog =
            cx.new(|cx| EquationDialog::new(existing.as_ref(), display, at, bridge, window, cx));
        let sub = cx.subscribe_in(
            &dialog,
            window,
            |this, _, ev: &EquationEvent, window, cx| match ev {
                EquationEvent::Done(eq) => this.apply_equation(eq.clone(), window, cx),
                EquationEvent::Cancel => {
                    this.equation = None;
                    this.focus(window, cx);
                    cx.notify();
                }
            },
        );
        self.equation = Some(DocsEquation {
            dialog,
            target,
            _sub: sub,
        });
        cx.notify();
    }

    fn apply_equation(
        &mut self,
        eq: kantor_math::Equation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.equation.take() else {
            return;
        };
        let sel = match open.target.and_then(|p| self.doc.set_equation(p, &eq)) {
            Some(sel) => sel,
            None => self.doc.insert_equation(self.sel, &eq),
        };
        self.edited(sel, window, cx);
        self.status = t("Equation saved as an Office equation (editable in Word)").into();
        self.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn render_equation(&self) -> Option<Entity<EquationDialog>> {
        self.equation.as_ref().map(|e| e.dialog.clone())
    }
}

/// Paints a laid-out equation with its top left at `origin`.
fn paint_layout(
    l: &kantor_math::Layout,
    bridge: &Rc<std::cell::RefCell<crate::FontBridge>>,
    origin: Point<Pixels>,
    ink: Hsla,
    window: &mut Window,
) {
    let ts = window.text_system().clone();
    let font = docs_layout::math_font();
    let Some(id) = bridge.borrow_mut().font_id(&font, &[], "x", &ts) else {
        return;
    };
    let base_y = origin.y + px(l.ascent);
    for g in l.glyphs.iter().filter(|g| g.font == 0) {
        let at = point(origin.x + px(g.x), base_y + px(g.y));
        let _ = window.paint_glyph(at, id, GlyphId(g.id as u32), px(g.size), ink);
    }
    for r in &l.rules {
        let b = Bounds::new(
            point(origin.x + px(r[0]), base_y + px(r[1])),
            size(px(r[2].max(1.0)), px(r[3].max(1.0))),
        );
        window.paint_quad(fill(b, ink));
    }
}
