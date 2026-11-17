//! The Fill and Outline popovers: the slide's theme colours (as theme
//! references, so they follow the theme), no fill / no outline, pattern
//! fills (PowerPoint's 48 patterns in two theme colours) and outline
//! widths.

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
    fn paint_target(&self) -> Option<slides_engine::ShapeRef> {
        let r = self.sel_ref().filter(|_| self.also.is_empty())?;
        let s = self.pres.shape(r)?;
        s.paintable().then_some(r)
    }

    pub fn set_fill_doc(&mut self, fill: Paint, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.paint_target() else {
            return;
        };
        let res = self.pres.set_fill(i, fill);
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
        let res = self.pres.set_line(i, color, width);
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
        let look = self.pres.shape(i)?.look.clone();
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
                let shape = self.pres.shape(i)?;
                panel = panel.child(self.render_pattern_fill(
                    shape,
                    self.pres.theme_colors(self.current),
                    &view,
                    cx,
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

impl SlidesView {
    /// Pattern fill: PowerPoint's 48 patterns and, for a patterned shape,
    /// its foreground and background colours (theme colours).
    fn render_pattern_fill(
        &self,
        shape: &slides_engine::Shape,
        theme: Vec<(&'static str, slides_engine::Rgb)>,
        view: &WeakEntity<SlidesView>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = tokens(cx);
        // The shape's pattern: as set in Kantor, else as read.
        let current: Option<(String, Paint, Paint)> = match &shape.paint.fill {
            Some(Paint::Pattern { prst, fg, bg }) => {
                Some((prst.clone(), (**fg).clone(), (**bg).clone()))
            }
            Some(_) => None,
            None => match &shape.look.fill_as_read {
                Some(slides_engine::Fill::Pattern(p)) => {
                    Some((p.prst.clone(), Paint::Rgb(p.fg.0), Paint::Rgb(p.bg.0)))
                }
                _ => None,
            },
        };
        let resolve = |p: &Paint| match p {
            Paint::Rgb(c) => Some(*c),
            Paint::Scheme(s) => theme.iter().find(|(k, _)| k == s).map(|(_, c)| *c),
            _ => None,
        };
        let (fg, bg) = current.as_ref().map_or_else(
            || (Paint::Scheme("accent1".into()), Paint::Scheme("bg1".into())),
            |(_, f, b)| (f.clone(), b.clone()),
        );
        let fg_rgb = resolve(&fg).unwrap_or(slides_engine::Rgb([0x44, 0x72, 0xC4]));
        let bg_rgb = resolve(&bg).unwrap_or(slides_engine::Rgb([255, 255, 255]));
        let mut grid = h_flex().flex_wrap().gap(px(3.)).px(SPACE_4);
        for (k, pr) in kantor_ooxml::pattern::PRESETS[..kantor_ooxml::pattern::GALLERY]
            .iter()
            .enumerate()
        {
            let v = view.clone();
            let on = current.as_ref().is_some_and(|c| c.0 == pr.prst);
            let name = i18n::t_dyn(kantor_ui::components::PATTERN_NAMES[k]);
            let paint = Paint::Pattern {
                prst: pr.prst.into(),
                fg: Box::new(fg.clone()),
                bg: Box::new(bg.clone()),
            };
            grid = grid.child(
                div()
                    .id(("slides-pattern", k))
                    .size(px(22.))
                    .p(px(2.))
                    .rounded(RADIUS_SM)
                    .border_1()
                    .border_color(if on { t.accent } else { t.border })
                    .cursor_pointer()
                    // The slide's own colours, not UI colours.
                    .child(
                        kantor_ui::components::pattern_swatch(
                            pr.bits,
                            hsla(fg_rgb),
                            hsla(bg_rgb),
                            2.0,
                        )
                        .size_full(),
                    )
                    .tooltip(move |window, cx| Tooltip::new(name).build(window, cx))
                    .on_click(move |_, window, cx| {
                        let paint = paint.clone();
                        let _ = v.update(cx, |this, cx| this.set_fill_doc(paint, window, cx));
                    }),
            );
        }
        let mut out = v_flex()
            .gap(SPACE_2)
            .child(
                section_label(i18n::t("Pattern fill"), cx)
                    .px(SPACE_4)
                    .pt(SPACE_2),
            )
            .child(grid);
        if let Some((prst, fg, bg)) = current {
            // One row of theme colours each for the foreground and background.
            for (fore, label) in [
                (true, i18n::t("Foreground")),
                (false, i18n::t("Background")),
            ] {
                let now = if fore { &fg } else { &bg };
                let mut sw = h_flex().gap(px(3.)).px(SPACE_4).flex_wrap();
                for (slot, rgb) in &theme {
                    let v = view.clone();
                    let on = resolve(now) == Some(*rgb);
                    let pick = Paint::Scheme((*slot).into());
                    let paint = Paint::Pattern {
                        prst: prst.clone(),
                        fg: Box::new(if fore { pick.clone() } else { fg.clone() }),
                        bg: Box::new(if fore { bg.clone() } else { pick }),
                    };
                    let id = format!("slides-pattern-{}-{slot}", if fore { "fg" } else { "bg" });
                    sw = sw.child(
                        div()
                            .id(SharedString::from(id))
                            .size(px(16.))
                            .rounded(RADIUS_SM)
                            .border_1()
                            .border_color(if on { t.accent } else { t.border_strong })
                            .cursor_pointer()
                            .child(div().size_full().rounded(RADIUS_SM).bg(hsla(*rgb)))
                            .on_click(move |_, window, cx| {
                                let paint = paint.clone();
                                let _ =
                                    v.update(cx, |this, cx| this.set_fill_doc(paint, window, cx));
                            }),
                    );
                }
                out = out
                    .child(div().px(SPACE_4).text_color(t.fg_muted).child(label))
                    .child(sw);
            }
        }
        out
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
