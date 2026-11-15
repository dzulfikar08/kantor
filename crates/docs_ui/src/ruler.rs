//! The horizontal ruler above the page, in centimetres: the page width with
//! the margins shaded, numbered from the left margin like Word, and the
//! caret paragraph's indents as markers (first line on top; left/hanging
//! and right at the bottom). Dragging a margin edge or a marker shows a
//! guide line on the page and applies on release, as one undo step.
//!
//! Tab stops, as Word: the caret paragraph's stops sit along the ruler's
//! bottom edge (default stops as small ticks after them). A click on the
//! ruler sets a stop of the kind chosen in the selector at the ruler's
//! left (click it to cycle Left, Center, Right, Decimal, Bar); dragging a
//! stop moves it, dragging it off the ruler removes it, and double-clicking
//! one opens the Tabs popover.
//!
//! It shares the page canvas's [`Geometry`](crate::Geometry), so it scrolls
//! and zooms with the page. Visibility is a user setting
//! (`docs_ruler` in `kantor_core::settings`), on by default.

use docs_engine::tabs::TabEdit;
use docs_engine::{Indent, TabKind, TabStop};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_core::settings::Settings;
use kantor_ui::i18n::{tf, tk};
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
    /// A tab stop, by its position (twips from the left margin).
    Tab(i32),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Drag {
    pub handle: Handle,
    /// The snapped position, in page px (also the guide line's x).
    pub x: f32,
    /// Window y where the drag started.
    pub y0: f32,
    /// A tab stop dragged off the ruler: released, it's removed.
    pub off: bool,
}

/// How far (px) a tab stop must be dragged up or down to come off.
const OFF_PX: f32 = 28.0;

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
            Handle::Tab(pos) => {
                i?;
                self.margin_left + pos as f32 / 15.0
            }
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
            Handle::Tab(_) => {
                self.margin_left + snap(x - self.margin_left).clamp(0.0, self.text_w())
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
            Handle::MarginLeft | Handle::MarginRight | Handle::Tab(_) => return None,
        })
    }
}

/// Mouse-down on a ruler handle: start dragging it.
fn start_drag(
    hd: Handle,
    cx: &mut Context<DocsView>,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
    cx.listener(
        move |this: &mut DocsView, ev: &MouseDownEvent, window, cx| {
            cx.stop_propagation();
            if let Handle::Tab(pos) = hd
                && ev.click_count >= 2
            {
                this.ruler_drag = None;
                let stop = this.ruler_tabs().into_iter().find(|s| s.pos == pos);
                let at = point(ev.position.x, ev.position.y + SPACE_5);
                this.open_tab_menu(Some(at), stop, window, cx);
                return;
            }
            if let Some(x) = this.ruler_marks().x(hd) {
                this.ruler_drag = Some(Drag {
                    handle: hd,
                    x,
                    y0: f32::from(ev.position.y),
                    off: false,
                });
                this.ruler_move(ev.position, cx);
                cx.notify();
            }
        },
    )
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

/// The tab stop kind after `k` in the ruler's selector.
pub(crate) fn next_kind(k: TabKind) -> TabKind {
    let all = TabKind::ALL;
    let i = all.iter().position(|x| *x == k).unwrap_or(0);
    all[(i + 1) % all.len()]
}

/// Twips from the left margin for a ruler x (page px).
pub(crate) fn tab_pos(marks: &Marks, x: f32) -> i32 {
    ((x - marks.margin_left) * 15.0).round() as i32
}

/// The edit a finished tab-stop drag makes (`None`: nothing changes).
pub(crate) fn tab_drag_edit(marks: &Marks, stops: &[TabStop], d: &Drag) -> Option<TabEdit> {
    let Handle::Tab(pos) = d.handle else {
        return None;
    };
    let old = stops.iter().find(|s| s.pos == pos)?;
    if d.off {
        return Some(TabEdit {
            clear: vec![pos],
            ..TabEdit::default()
        });
    }
    let new = tab_pos(marks, d.x);
    (new != pos).then(|| TabEdit {
        clear: vec![pos],
        set: vec![TabStop { pos: new, ..*old }],
        ..TabEdit::default()
    })
}

/// A small tab-stop symbol as Word's ruler draws them (the stop is at its
/// upright): └ left, ┴ center, ┘ right, ┴ with a dot decimal, │ bar.
fn tab_glyph(kind: TabKind, color: Hsla) -> Div {
    let size = px(9.);
    let stroke = px(2.);
    let mid = (size - stroke) / 2.;
    let mut g = div().relative().size(size).flex_none();
    // The upright through the stop.
    g = g.child(
        div()
            .absolute()
            .left(mid)
            .top_0()
            .w(stroke)
            .h_full()
            .bg(color),
    );
    let foot = |left: Pixels, w: Pixels| {
        div()
            .absolute()
            .left(left)
            .bottom_0()
            .w(w)
            .h(stroke)
            .bg(color)
    };
    match kind {
        TabKind::Left => g.child(foot(mid, size - mid)),
        TabKind::Right => g.child(foot(px(0.), mid + stroke)),
        TabKind::Center => g.child(foot(px(0.), size)),
        TabKind::Decimal => g.child(foot(px(0.), size)).child(
            div()
                .absolute()
                .right(px(-1.))
                .top(px(1.))
                .size(stroke)
                .bg(color),
        ),
        TabKind::Bar => g,
    }
}

impl DocsView {
    /// The caret paragraph's tab stops (none shown in a table).
    pub(crate) fn ruler_tabs(&self) -> Vec<TabStop> {
        if self.in_table() {
            return Vec::new();
        }
        self.doc.tab_stops(self.doc.paragraph(self.sel.head.para))
    }

    /// A click on the ruler (not on a handle): set a stop there.
    fn ruler_click(&mut self, x: Pixels, window: &mut Window, cx: &mut Context<Self>) {
        let marks = self.ruler_marks();
        if marks.indents.is_none() {
            return;
        }
        let px = self.page_x_at(x);
        if px < marks.margin_left || px > marks.page_w - marks.margin_right {
            return;
        }
        let x = marks.constrain(Handle::Tab(0), px);
        let stop = TabStop::new(tab_pos(&marks, x), self.ruler_tab, Default::default());
        self.edit_tabs_doc(
            TabEdit {
                set: vec![stop],
                ..TabEdit::default()
            },
            window,
            cx,
        );
    }

    pub(crate) fn ruler_marks(&self) -> Marks {
        let s = docs_pdf::build::geometry_setup(&self.doc.section_page(self.sel.head.para));
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
        f32::from(x - g.origin.x) / g.zoom + g.scroll.x - g.page_x(self.caret_page())
    }

    fn ruler_move(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let marks = self.ruler_marks();
        let px = self.page_x_at(pos.x);
        if let Some(d) = &mut self.ruler_drag {
            let x = marks.constrain(d.handle, px);
            let off =
                matches!(d.handle, Handle::Tab(_)) && (f32::from(pos.y) - d.y0).abs() > OFF_PX;
            if x != d.x || off != d.off {
                d.x = x;
                d.off = off;
                cx.notify();
            }
        }
    }

    fn ruler_drop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.ruler_drag.take() else {
            return;
        };
        let marks = self.ruler_marks();
        if let Handle::Tab(_) = d.handle {
            match tab_drag_edit(&marks, &self.ruler_tabs(), &d) {
                Some(edit) => self.edit_tabs_doc(edit, window, cx),
                None => cx.notify(),
            }
            return;
        }
        if marks.x(d.handle).is_some_and(|x| (x - d.x).abs() < 0.01) {
            cx.notify();
            return;
        }
        let tw = |px: f32| (px * 15.0).round().max(0.0) as u32;
        match d.handle {
            Handle::MarginLeft => {
                let mut p = self.page_geometry();
                p.margin_left = tw(d.x);
                self.set_page_doc(p, window, cx);
            }
            Handle::MarginRight => {
                let mut p = self.page_geometry();
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
        let page_x = g.page_x(self.caret_page());
        let sx = |x: f32| px((page_x + x - g.scroll.x) * z);
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

        // Tab stops along the bottom edge; default stops as ticks after
        // the last one, as Word.
        if marks.indents.is_some() {
            let tabs = self.ruler_tabs();
            let text_end = marks.page_w - marks.margin_right;
            let last = tabs.iter().map(|s| s.pos).max().unwrap_or(0).max(0) as f32 / 15.0;
            let step = (self.doc.default_tab() as f32 / 15.0).max(4.0);
            let mut d = ((last / step).floor() + 1.0) * step;
            while marks.margin_left + d < text_end {
                ruler = ruler.child(
                    div()
                        .absolute()
                        .bottom(px(1.))
                        .h(px(3.))
                        .w(px(1.))
                        .left(sx(marks.margin_left + d))
                        .bg(t.fg_dim),
                );
                d += step;
            }
            for s in tabs {
                let hd = Handle::Tab(s.pos);
                let Some(x) = at(hd) else { continue };
                let off = drag.is_some_and(|d| d.handle == hd && d.off);
                let color = if dragging(hd) { t.accent } else { t.fg };
                ruler = ruler.child(
                    div()
                        .id(SharedString::from(format!("docs-ruler-tab-{}", s.pos)))
                        .absolute()
                        .bottom(px(2.))
                        .left(sx(x) - SPACE_3)
                        .w(SPACE_6)
                        .h(SPACE_5)
                        .flex()
                        .items_end()
                        .justify_center()
                        .when(off, |d| d.opacity(0.3))
                        .cursor(CursorStyle::ResizeLeftRight)
                        .child(tab_glyph(s.kind, color))
                        .on_mouse_down(MouseButton::Left, start_drag(hd, cx)),
                );
            }
            // The tab kind selector, at the ruler's left.
            let kind = self.ruler_tab;
            ruler = ruler.child(
                div()
                    .id("docs-ruler-tab-kind")
                    .absolute()
                    .left(SPACE_2)
                    .top(SPACE_1)
                    .bottom(SPACE_1)
                    .w(h - SPACE_2)
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(RADIUS_SM)
                    .bg(t.elevated)
                    .border_1()
                    .border_color(t.border)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .child(tab_glyph(kind, t.fg))
                    .tooltip(move |window, cx| {
                        let msg = tk("{} tab: click to change, then click the ruler to set one");
                        Tooltip::new(tf(msg, &[&crate::tab_menu::kind_label(kind)]))
                            .build(window, cx)
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.ruler_tab = next_kind(this.ruler_tab);
                            cx.notify();
                        }),
                    ),
            );
            ruler = ruler.on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    this.ruler_click(ev.position.x, window, cx);
                }),
            );
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
                                let _ = v.update(cx, |this, cx| this.ruler_move(ev.position, cx));
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

    #[std::prelude::v1::test]
    fn tab_stops_drag_move_and_come_off() {
        use docs_engine::TabLeader;
        let m = a4();
        let stop = TabStop::new(1417, TabKind::Right, TabLeader::Dot);
        // Snapped to 0.25 cm from the margin, within the text area.
        let x = m.constrain(Handle::Tab(0), m.margin_left + 3.1 * CM);
        assert!((x - m.margin_left - 3.0 * CM).abs() < 0.01);
        assert_eq!(m.constrain(Handle::Tab(0), 0.0), m.margin_left);
        let drag = |x: f32, off: bool| Drag {
            handle: Handle::Tab(1417),
            x,
            y0: 0.0,
            off,
        };
        // Moved: cleared at the old place, set at the new one (same kind
        // and leader).
        let e = tab_drag_edit(&m, &[stop], &drag(x, false)).unwrap();
        assert_eq!(e.clear, [1417]);
        assert_eq!(e.set[0].kind, TabKind::Right);
        assert_eq!(e.set[0].leader, TabLeader::Dot);
        assert_eq!(e.set[0].pos, 1701);
        // Released where it was: nothing.
        let here = m.x(Handle::Tab(1417)).unwrap();
        assert!(tab_drag_edit(&m, &[stop], &drag(here, false)).is_none());
        // Off the ruler: removed.
        let e = tab_drag_edit(&m, &[stop], &drag(x, true)).unwrap();
        assert_eq!(e.clear, [1417]);
        assert!(e.set.is_empty());
        // The selector cycles through every kind.
        let mut k = TabKind::Left;
        for _ in 0..5 {
            k = next_kind(k);
        }
        assert_eq!(k, TabKind::Left);
        assert_eq!(next_kind(TabKind::Decimal), TabKind::Bar);
    }
}
