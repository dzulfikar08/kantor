//! The Fill and Outline popovers: the slide's theme colours (as theme
//! references, so they follow the theme), no fill / no outline, and
//! outline widths.

use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{Icon, Sizable as _, h_flex, tooltip::Tooltip, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::components::{icon_button, section_label};
use kantor_ui::i18n::{self, tf};
use kantor_ui::tokens::Elevation;
use kantor_ui::{metrics::*, tokens};
use slides_engine::{EMU_PER_PT, Emu, Paint};

use crate::SlidesView;
use crate::draw::hsla;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaintKind {
    Fill,
    Line,
}

pub(crate) struct PaintMenu {
    pub at: Point<Pixels>,
    pub kind: PaintKind,
}

type Apply = Box<dyn Fn(&mut SlidesView, &mut Window, &mut Context<SlidesView>)>;

/// Outline widths offered, points.
const WIDTHS: [f32; 7] = [0.75, 1.0, 1.5, 2.25, 3.0, 4.5, 6.0];

/// A readable name for a theme colour slot.
fn slot_name(slot: &str) -> String {
    match slot {
        "bg1" => i18n::t("Background 1").into(),
        "tx1" => i18n::t("Text 1").into(),
        "bg2" => i18n::t("Background 2").into(),
        "tx2" => i18n::t("Text 2").into(),
        s => match s.strip_prefix("accent") {
            Some(n) => tf("Accent {}", &[&n]),
            None => s.to_owned(),
        },
    }
}

impl SlidesView {
    pub(crate) fn toggle_paint_menu(
        &mut self,
        kind: PaintKind,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let was = self.paint_menu.take().map(|m| m.kind);
        let just_closed = self
            .paint_menu_closed
            .is_some_and(|t| t.elapsed() < Duration::from_millis(250));
        self.paint_menu_closed = None;
        if was == Some(kind) || (was.is_none() && just_closed) {
            cx.notify();
            return;
        }
        if self.paint_target().is_none() {
            let msg = i18n::t("Select a shape or picture to change its fill or outline");
            self.set_status(msg, cx);
            return;
        }
        self.paint_menu = Some(PaintMenu { at, kind });
        cx.notify();
    }

    /// Open the Fill (`outline` false) or Outline popover at the canvas's
    /// top right (palette).
    pub fn open_paint_menu_doc(&mut self, outline: bool, cx: &mut Context<Self>) {
        let b = self.viewport.get();
        let at = point(b.right() - REM * 15., b.top() + SPACE_4);
        let kind = if outline {
            PaintKind::Line
        } else {
            PaintKind::Fill
        };
        self.paint_menu = None;
        self.paint_menu_closed = None;
        self.toggle_paint_menu(kind, at, cx);
    }

    /// The selected shape, if its fill and outline can be changed.
    fn paint_target(&self) -> Option<usize> {
        let i = self.selected?;
        let s = self.pres.slide(self.current).shapes.get(i)?;
        s.paintable().then_some(i)
    }

    pub fn set_fill_doc(&mut self, fill: Paint, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.paint_target() else {
            return;
        };
        let res = self.pres.set_fill((self.current, i), fill);
        if self.report(res, cx).is_some() {
            self.after_edit(window, cx);
        }
    }

    pub fn set_line_doc(
        &mut self,
        color: Option<Paint>,
        width: Option<Emu>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(i) = self.paint_target() else {
            return;
        };
        let res = self.pres.set_line((self.current, i), color, width);
        if self.report(res, cx).is_some() {
            self.after_edit(window, cx);
        }
    }

    pub(crate) fn render_paint_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let menu = self.paint_menu.as_ref()?;
        let i = self.paint_target()?;
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let kind = menu.kind;
        let look = self.pres.slide(self.current).shapes[i].look.clone();
        let current = match kind {
            PaintKind::Fill => look.fill,
            PaintKind::Line => look.line.map(|l| l.0),
        };

        let mut swatches = h_flex().flex_wrap().gap(SPACE_2).px(SPACE_4);
        for (slot, rgb) in self.pres.theme_colors(self.current) {
            let v = view.clone();
            let name = slot_name(slot);
            let on = current == Some(rgb);
            swatches = swatches.child(
                div()
                    .id(SharedString::from(format!("slides-swatch-{slot}")))
                    .size(CONTROL_SM)
                    .rounded(RADIUS_SM)
                    .border_2()
                    .border_color(if on { t.accent } else { t.border_strong })
                    .cursor_pointer()
                    // The slide's own colour, not a UI colour.
                    .child(div().size_full().rounded(RADIUS_SM).bg(hsla(rgb)))
                    .tooltip(move |window, cx| Tooltip::new(name.clone()).build(window, cx))
                    .on_click(move |_, window, cx| {
                        let paint = Paint::Scheme(slot.into());
                        let _ = v.update(cx, |this, cx| match kind {
                            PaintKind::Fill => this.set_fill_doc(paint, window, cx),
                            PaintKind::Line => this.set_line_doc(Some(paint), None, window, cx),
                        });
                    }),
            );
        }

        let row = |id: &'static str, label: String, on: bool, run: Apply| {
            let v = view.clone();
            h_flex()
                .id(id)
                .h(CONTROL_MD)
                .px(SPACE_4)
                .gap(SPACE_4)
                .rounded(RADIUS_MD)
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .child(
                    div()
                        .flex_none()
                        .size(ICON_MD)
                        .when(on, |d| d.child(Icon::new(Lucide::Check).with_size(ICON_MD))),
                )
                .child(div().text_color(t.fg).child(label))
                .on_click(move |_, window, cx| {
                    let _ = v.update(cx, |this, cx| run(this, window, cx));
                })
        };

        let mut panel = v_flex()
            .id("slides-paint-menu")
            .occlude()
            .w(REM * 14.)
            .gap(SPACE_3)
            .p(SPACE_4)
            .bg(t.elevated)
            .border_1()
            .border_color(t.border)
            .rounded(RADIUS_LG)
            .shadow(t.shadow(Elevation::Popover))
            .text_size(FONT_SMALL)
            .text_color(t.fg)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.paint_menu = None;
                this.paint_menu_closed = Some(Instant::now());
                cx.notify();
            }))
            .child(
                section_label(i18n::t("Theme colours"), cx)
                    .px(SPACE_4)
                    .pt(SPACE_2),
            )
            .child(swatches);
        match kind {
            PaintKind::Fill => {
                panel = panel.child(row(
                    "slides-no-fill",
                    i18n::t("No fill").into(),
                    look.fill.is_none(),
                    Box::new(|v, w, cx| v.set_fill_doc(Paint::None, w, cx)),
                ));
            }
            PaintKind::Line => {
                panel = panel.child(row(
                    "slides-no-line",
                    i18n::t("No outline").into(),
                    look.line.is_none(),
                    Box::new(|v, w, cx| v.set_line_doc(Some(Paint::None), None, w, cx)),
                ));
                panel = panel.child(section_label(i18n::t("Width"), cx).px(SPACE_4).pt(SPACE_2));
                let width = look.line.map(|l| l.1);
                let mut widths = v_flex();
                for (k, pt) in WIDTHS.into_iter().enumerate() {
                    let emu = (pt * EMU_PER_PT).round() as Emu;
                    let ids = [
                        "slides-w0",
                        "slides-w1",
                        "slides-w2",
                        "slides-w3",
                        "slides-w4",
                        "slides-w5",
                        "slides-w6",
                    ];
                    widths = widths.child(row(
                        ids[k],
                        format!("{pt} pt"),
                        width == Some(emu),
                        Box::new(move |v, w, cx| v.set_line_doc(None, Some(emu), w, cx)),
                    ));
                }
                panel = panel.child(widths);
            }
        }
        Some(deferred(
            anchored()
                .position(menu.at)
                .snap_to_window_with_margin(SPACE_4)
                .child(panel),
        ))
    }
}

/// A toolbar button that opens the Fill or Outline popover under itself.
pub(crate) fn paint_button(
    view: &SlidesView,
    kind: PaintKind,
    cx: &mut Context<SlidesView>,
) -> impl IntoElement {
    let v = cx.entity().downgrade();
    let (id, icon, tip) = match kind {
        PaintKind::Fill => ("slides-fill", Lucide::PaintBucket, i18n::t("Shape fill")),
        PaintKind::Line => (
            "slides-line",
            Lucide::PenLine,
            i18n::t("Shape outline: colour and width"),
        ),
    };
    let open = view.paint_menu.as_ref().is_some_and(|m| m.kind == kind);
    icon_button(id, icon, open, cx)
        .tooltip(tip)
        .on_click(move |ev, _, cx| {
            let p = ev.position();
            let at = point(p.x - SPACE_7, p.y + SPACE_7);
            let _ = v.update(cx, |this, cx| this.toggle_paint_menu(kind, at, cx));
        })
}
