//! The horizontal ruler above the page, in centimetres: the page width with
//! the margins shaded, numbered from the left margin like Word, and the
//! caret paragraph's indents as markers (first line on top; left/hanging
//! and right at the bottom). Dragging a margin edge or a marker shows a
//! guide line on the page and applies on release, as one undo step.
//!
//! It shares the page canvas's [`Geometry`](crate::Geometry), so it scrolls
//! and zooms with the page. Visibility is a user setting
//! (`docs_ruler` in `kantor_core::settings`), on by default.

use docs_engine::Indent;
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_core::settings::Settings;
use kantor_ui::{metrics::*, tokens};

use crate::DocsView;

/// Px per cm at 96 dpi.
pub(crate) const CM: f32 = 96.0 / 2.54;
/// Drags snap to this (0.25 cm).
const SNAP_PX: f32 = CM / 4.0;
/// Text area kept at least this wide/high when dragging margins.
const MIN_TEXT_PX: f32 = CM;

/// Whether rulers are shown (all Docs views).
struct RulerVisible(bool);
impl Global for RulerVisible {}

/// Read the setting (call once at start-up).
pub(crate) fn init(cx: &mut App) {
    cx.set_global(RulerVisible(Settings::load().docs_ruler));
}

pub fn ruler_visible(cx: &App) -> bool {
    cx.try_global::<RulerVisible>().is_none_or(|r| r.0)
}

/// Show or hide the ruler in every Docs view, and remember it.
pub fn set_ruler_visible(on: bool, cx: &mut App) {
    cx.set_global(RulerVisible(on));
    cx.refresh_windows();
    cx.background_spawn(async move {
        let mut s = Settings::load();
        if s.docs_ruler != on {
            s.docs_ruler = on;
            if let Err(err) = s.save() {
                log::error!("saving settings: {err:#}");
            }
        }
    })
    .detach();
}

pub fn toggle_ruler(cx: &mut App) {
    set_ruler_visible(!ruler_visible(cx), cx);
}

/// What a ruler drag moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Handle {
    MarginLeft,
    MarginRight,
    FirstLine,
    /// The left indent; the first line stays where it is (hanging indent).
    Left,
    Right,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Drag {
    pub handle: Handle,
    /// The snapped position, in page px (also the guide line's x).
    pub x: f32,
}

/// The caret paragraph's indents and the page margins, in page px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Marks {
    pub page_w: f32,
    pub margin_left: f32,
    pub margin_right: f32,
    /// `None` in a table (cell indents aren't on the page ruler).
    pub indents: Option<Indents>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Indents {
    pub left: f32,
    pub first: f32,
    pub right: f32,
}

impl Marks {
    /// Where a handle sits, in page px.
    pub fn x(&self, h: Handle) -> Option<f32> {
        let i = self.indents;
        Some(match h {
            Handle::MarginLeft => self.margin_left,
            Handle::MarginRight => self.page_w - self.margin_right,
            Handle::FirstLine => self.margin_left + i?.left + i?.first,
            Handle::Left => self.margin_left + i?.left,
            Handle::Right => self.page_w - self.margin_right - i?.right,
        })
    }

    /// Snap and clamp a dragged position.
    pub fn constrain(&self, h: Handle, x: f32) -> f32 {
        let snap = |v: f32| (v / SNAP_PX).round() * SNAP_PX;
        match h {
            Handle::MarginLeft => snap(x).clamp(0.0, self.page_w - self.margin_right - MIN_TEXT_PX),
            Handle::MarginRight => {
                let m =
                    snap(self.page_w - x).clamp(0.0, self.page_w - self.margin_left - MIN_TEXT_PX);
                self.page_w - m
            }
            // Indents snap relative to the margin they're measured from.
            Handle::FirstLine | Handle::Left => {
                self.margin_left
                    + snap(x - self.margin_left).clamp(-self.margin_left, self.text_w())
            }
            Handle::Right => {
                let edge = self.page_w - self.margin_right;
                edge - snap(edge - x).clamp(-self.margin_right, self.text_w())
            }
        }
    }

    fn text_w(&self) -> f32 {
        self.page_w - self.margin_left - self.margin_right
    }

    /// The new indent for an indent drag ending at `x` (twips).
    pub fn indent_for(&self, h: Handle, x: f32) -> Option<Indent> {
        let i = self.indents?;
        let tw = |px: f32| (px * 15.0).round() as i32;
        let rel = x - self.margin_left;
        Some(match h {
            Handle::FirstLine => Indent {
                first: Some(tw(rel - i.left)),
                ..Indent::default()
            },
            Handle::Left => Indent {
                left: Some(tw(rel)),
                // Keep the first line where it is.
                first: Some(tw(i.left + i.first - rel)),
                right: None,
            },
            Handle::Right => Indent {
                right: Some(tw(self.page_w - self.margin_right - x)),
                ..Indent::default()
            },
            Handle::MarginLeft | Handle::MarginRight => return None,
        })
    }
}

/// Mouse-down on a ruler handle: start dragging it.
fn start_drag(
    hd: Handle,
    cx: &mut Context<DocsView>,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
    cx.listener(move |this: &mut DocsView, ev: &MouseDownEvent, _, cx| {
        cx.stop_propagation();
        if let Some(x) = this.ruler_marks().x(hd) {
            this.ruler_drag = Some(Drag { handle: hd, x });
            this.ruler_move(ev.position.x, cx);
            cx.notify();
        }
    })
}

/// Ticks every half centimetre from the left margin: `(x, whole cm number)`.
pub(crate) fn ticks(page_w: f32, margin_left: f32) -> Vec<(f32, Option<i32>)> {
    let mut out = Vec::new();
    let first = -(margin_left / (CM / 2.0)).floor() as i32;
    let last = ((page_w - margin_left) / (CM / 2.0)).floor() as i32;
    for k in first..=last {
        let x = margin_left + k as f32 * CM / 2.0;
        out.push((x, (k % 2 == 0).then_some(k / 2)));
    }
    out
}

impl DocsView {
    pub(crate) fn ruler_marks(&self) -> Marks {
        let s = self.laid.paged.setup;
        let indents = (!self.in_table()).then(|| {
            let p = self.doc.paragraph(self.sel.head.para);
            let r = self.doc.resolve_para(p);
            let px = |pt: f32| pt * 96.0 / 72.0;
            Indents {
                left: px(r.indent_left_pt),
                first: px(r.first_line_pt),
                right: px(r.indent_right_pt),
            }
        });
        Marks {
            page_w: s.width,
            margin_left: s.margin_left,
            margin_right: s.margin_right,
            indents,
        }
    }

    /// Page x (unzoomed) under a window x.
    fn page_x_at(&self, x: Pixels) -> f32 {
        let g = self.geometry();
        f32::from(x - g.origin.x) / g.zoom + g.scroll.x - g.page_x()
    }

    fn ruler_move(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let marks = self.ruler_marks();
        let px = self.page_x_at(x);
        if let Some(d) = &mut self.ruler_drag {
            let x = marks.constrain(d.handle, px);
            if x != d.x {
                d.x = x;
                cx.notify();
            }
        }
    }

    fn ruler_drop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.ruler_drag.take() else {
            return;
        };
        let marks = self.ruler_marks();
        if marks.x(d.handle).is_some_and(|x| (x - d.x).abs() < 0.01) {
            cx.notify();
            return;
        }
        let tw = |px: f32| (px * 15.0).round().max(0.0) as u32;
        match d.handle {
            Handle::MarginLeft => {
                let mut p = self.doc.page;
                p.margin_left = tw(d.x);
                self.set_page_doc(p, window, cx);
            }
            Handle::MarginRight => {
                let mut p = self.doc.page;
                p.margin_right = tw(marks.page_w - d.x);
                self.set_page_doc(p, window, cx);
            }
            h => {
                if let Some(ind) = marks.indent_for(h, d.x) {
                    self.doc.set_indent(self.sel, ind);
                    let sel = self.sel;
                    self.edited(sel, window, cx);
                }
            }
        }
    }

    /// The ruler strip (`None` when hidden).
    pub(crate) fn render_ruler(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        if !ruler_visible(cx) {
            return None;
        }
        let t = tokens(cx);
        let g = self.geometry();
        let z = g.zoom;
        let marks = self.ruler_marks();
        // Ruler-local x of a page x.
        let sx = |x: f32| px((g.page_x() + x - g.scroll.x) * z);
        // Drawn in the chrome palette, not paper white: the ruler is UI,
        // and a white strip glares on dark themes.
        let ink = t.fg;
        let h = CONTROL_SM;

        let mut ruler = div()
            .id("docs-ruler")
            .flex_none()
            .relative()
            .w_full()
            .h(h)
            .overflow_hidden()
            .bg(t.chrome)
            .border_b_1()
            .border_color(t.border)
            .text_size(FONT_CAPTION);
        // The page band, margins shaded.
        let band = |x: f32, w: f32| {
            div()
                .absolute()
                .top(SPACE_2)
                .bottom(SPACE_2)
                .left(sx(x))
                .w(px(w * z))
        };
        ruler = ruler
            .child(
                band(0.0, marks.page_w)
                    .rounded(RADIUS_SM)
                    .bg(t.elevated)
                    .border_1()
                    .border_color(t.border),
            )
            .child(band(0.0, marks.margin_left).bg(t.bg.opacity(0.6)))
            .child(
                band(marks.page_w - marks.margin_right, marks.margin_right).bg(t.bg.opacity(0.6)),
            );
        // Ticks and numbers; numbers thin out when zoomed far out.
        let every = [1, 2, 5, 10]
            .into_iter()
            .find(|n| *n as f32 * CM * z >= 24.0)
            .unwrap_or(10);
        for (x, n) in ticks(marks.page_w, marks.margin_left) {
            match n {
                Some(n) if n != 0 && n % every == 0 => {
                    ruler = ruler.child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(sx(x) - SPACE_5)
                            .w(SPACE_5 * 2.)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(t.fg_muted)
                            .child(n.abs().to_string()),
                    );
                }
                Some(_) => {
                    ruler = ruler.child(
                        div()
                            .absolute()
                            .top(SPACE_5)
                            .bottom(SPACE_5)
                            .left(sx(x))
                            .w(px(1.))
                            .bg(t.fg_dim),
                    );
                }
                None if CM * z / 2.0 >= 6.0 => {
                    ruler = ruler.child(
                        div()
                            .absolute()
                            .top(SPACE_6 - px(1.))
                            .bottom(SPACE_6 - px(1.))
                            .left(sx(x))
                            .w(px(1.))
                            .bg(ink.opacity(0.22)),
                    );
                }
                None => {}
            }
        }

        let drag = self.ruler_drag;
        let dragging = |hd: Handle| drag.is_some_and(|d| d.handle == hd);
        // Where a handle is drawn: the drag position while it's dragged.
        let at = |hd: Handle| {
            drag.filter(|d| d.handle == hd)
                .map(|d| d.x)
                .or_else(|| marks.x(hd))
        };

        // Margin edges: the middle of the ruler, between the markers.
        for (hd, id) in [
            (Handle::MarginLeft, "docs-ruler-ml"),
            (Handle::MarginRight, "docs-ruler-mr"),
        ] {
            let Some(x) = at(hd) else { continue };
            ruler = ruler.child(
                div()
                    .id(id)
                    .absolute()
                    .top(SPACE_4)
                    .bottom(SPACE_4)
                    .left(sx(x) - SPACE_2)
                    .w(SPACE_4)
                    .cursor(CursorStyle::ResizeLeftRight)
                    .child(
                        div()
                            .mx_auto()
                            .w(px(2.))
                            .h_full()
                            .when(dragging(hd), |d| d.bg(t.accent)),
                    )
                    .on_mouse_down(MouseButton::Left, start_drag(hd, cx)),
            );
        }
        // Indent markers: first line on top, left and right at the bottom.
        for (hd, id, top) in [
            (Handle::FirstLine, "docs-ruler-first", true),
            (Handle::Left, "docs-ruler-left", false),
            (Handle::Right, "docs-ruler-right", false),
        ] {
            let Some(x) = at(hd) else { continue };
            let color = if dragging(hd) { t.accent } else { t.fg_muted };
            let marker = div()
                .id(id)
                .absolute()
                .left(sx(x) - SPACE_3)
                .w(SPACE_6)
                .h(SPACE_4)
                .rounded(RADIUS_SM)
                .bg(color)
                .border_1()
                .border_color(t.chrome)
                .cursor(CursorStyle::ResizeLeftRight)
                .on_mouse_down(MouseButton::Left, start_drag(hd, cx));
            ruler = ruler.child(if top {
                marker.top_0()
            } else {
                marker.bottom_0()
            });
        }

        // While dragging, follow the pointer anywhere in the window.
        if drag.is_some() {
            let view = cx.entity().downgrade();
            ruler = ruler.child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let v = view.clone();
                        window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ = v.update(cx, |this, cx| this.ruler_move(ev.position.x, cx));
                            }
                        });
                        let v = view.clone();
                        window.on_mouse_event(move |_: &MouseUpEvent, phase, window, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ = v.update(cx, |this, cx| this.ruler_drop(window, cx));
                            }
                        });
                    },
                )
                .absolute()
                .size_0(),
            );
        }
        Some(ruler)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a4() -> Marks {
        let m = 2.54 * CM;
        Marks {
            page_w: 21.0 * CM,
            margin_left: m,
            margin_right: m,
            indents: Some(Indents {
                left: 0.0,
                first: 0.0,
                right: 0.0,
            }),
        }
    }

    #[std::prelude::v1::test]
    fn ticks_count_from_the_margin() {
        let t = ticks(21.0 * CM, 2.54 * CM);
        let zero = t.iter().find(|(_, n)| *n == Some(0)).unwrap();
        assert!((zero.0 - 2.54 * CM).abs() < 0.01);
        assert_eq!(t.iter().filter_map(|(_, n)| *n).max(), Some(18));
        assert_eq!(t.iter().filter_map(|(_, n)| *n).min(), Some(-2));
    }

    #[std::prelude::v1::test]
    fn drags_snap_and_clamp() {
        let m = a4();
        // Snaps to 0.25 cm.
        let x = m.constrain(Handle::MarginLeft, 3.1 * CM);
        assert!((x - 3.0 * CM).abs() < 0.01);
        // Never past the other margin.
        let x = m.constrain(Handle::MarginLeft, 30.0 * CM);
        assert!(x <= m.page_w - m.margin_right - MIN_TEXT_PX + 0.01);
        assert_eq!(m.constrain(Handle::MarginRight, 50.0 * CM), m.page_w);
        // Indents snap from the margin, not the page edge.
        let x = m.constrain(Handle::Left, m.margin_left + 1.05 * CM);
        assert!((x - m.margin_left - CM).abs() < 0.01);
    }

    #[std::prelude::v1::test]
    fn indent_drags_become_twips() {
        let m = a4();
        let left = m.indent_for(Handle::Left, m.margin_left + CM).unwrap();
        assert_eq!(left.left, Some(567));
        // The first line stays put: a 1 cm hanging indent.
        assert_eq!(left.first, Some(-567));
        assert_eq!(left.right, None);
        let first = m
            .indent_for(Handle::FirstLine, m.margin_left + 2.0 * CM)
            .unwrap();
        assert_eq!(first.first, Some(1134));
        assert_eq!(first.left, None);
        let right = m
            .indent_for(Handle::Right, m.page_w - m.margin_right - CM)
            .unwrap();
        assert_eq!(right.right, Some(567));
        assert!(m.indent_for(Handle::MarginLeft, 0.0).is_none());
    }
}
