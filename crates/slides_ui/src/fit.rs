//! Where the slide sits on the canvas: scaled to fit the pane with an even
//! margin, times the user's zoom (1.0 = fit), centred on each axis where it
//! fits and scrolled where it overflows. Pure geometry, unit-tested.

use gpui_kit::*;
use kantor_ui::metrics::SLIDE_MARGIN;
use slides_engine::SlideSize;

use crate::draw::View;

/// Smallest margin around the slide, in screen px (small panes).
const MIN_MARGIN: f32 = 12.0;

/// Margin around the slide for a pane: 4 % of its short side, between
/// [`MIN_MARGIN`] and [`SLIDE_MARGIN`], so small panes keep most of the
/// space for the slide.
pub(crate) fn canvas_margin(pane: Size<Pixels>) -> f32 {
    let short = f32::from(pane.width).min(f32::from(pane.height));
    (short * 0.04).clamp(MIN_MARGIN, SLIDE_MARGIN)
}

/// One axis: the slide's offset from the pane's start and the scroll
/// actually used (clamped). Centred if `len + 2 * margin` fits in `avail`.
fn axis(avail: f32, len: f32, margin: f32, scroll: f32) -> (f32, f32) {
    let overflow = len + 2.0 * margin - avail;
    if overflow <= 0.0 {
        ((avail - len) / 2.0, 0.0)
    } else {
        let s = scroll.clamp(0.0, overflow);
        (margin - s, s)
    }
}

/// The slide's placement in `bounds`: the screen mapping, the slide's
/// screen rectangle and the scroll (screen px) after clamping.
pub(crate) fn place_slide(
    bounds: Bounds<Pixels>,
    slide: SlideSize,
    margin: f32,
    zoom: f32,
    scroll: Point<f32>,
) -> (View, Bounds<Pixels>, Point<f32>) {
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let fit = ((w - 2.0 * margin) / slide.cx as f32)
        .min((h - 2.0 * margin) / slide.cy as f32)
        .max(1e-7);
    let scale = fit * zoom;
    let (sw, sh) = (slide.cx as f32 * scale, slide.cy as f32 * scale);
    let (ox, sx) = axis(w, sw, margin, scroll.x);
    let (oy, sy) = axis(h, sh, margin, scroll.y);
    let origin = point(bounds.origin.x + px(ox), bounds.origin.y + px(oy));
    (
        View { origin, scale },
        Bounds::new(origin, size(px(sw), px(sh))),
        point(sx, sy),
    )
}

/// Scroll that puts slide point `emu` (x, y in EMU) under screen point
/// `at` with `scale` (zooming about the pointer or the pane's centre);
/// [`place_slide`] clamps it.
pub(crate) fn scroll_keeping(
    bounds: Bounds<Pixels>,
    margin: f32,
    scale: f32,
    emu: (f32, f32),
    at: Point<Pixels>,
) -> Point<f32> {
    point(
        margin + emu.0 * scale - f32::from(at.x - bounds.origin.x),
        margin + emu.1 * scale - f32::from(at.y - bounds.origin.y),
    )
}

/// Cursor over a resize handle: which edges it moves (-1, 0, 1 per axis).
pub(crate) fn handle_cursor(h: (i8, i8)) -> CursorStyle {
    match h {
        (0, _) => CursorStyle::ResizeUpDown,
        (_, 0) => CursorStyle::ResizeLeftRight,
        (x, y) if x == y => CursorStyle::ResizeUpLeftDownRight,
        _ => CursorStyle::ResizeUpRightDownLeft,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(10.), px(20.)), size(px(w), px(h)))
    }

    fn close(a: Pixels, b: f32) -> bool {
        (f32::from(a) - b).abs() < 0.5
    }

    /// A tall pane: the slide fills the width less the margin and is
    /// centred vertically (equal gaps above and below).
    #[std::prelude::v1::test]
    fn fit_fills_width_and_centres_vertically() {
        let b = pane(1000., 800.);
        let (_, sb, s) = place_slide(b, SlideSize::WIDE, 32., 1.0, point(0., 0.));
        assert!(close(sb.size.width, 936.));
        let top = f32::from(sb.top() - b.top());
        let bottom = f32::from(b.bottom() - sb.bottom());
        assert!((top - bottom).abs() < 0.5, "{top} vs {bottom}");
        assert!(close(sb.left() - b.left(), 32.));
        assert_eq!(s, point(0., 0.));
    }

    /// A wide pane: height-bound, centred horizontally.
    #[std::prelude::v1::test]
    fn fit_fills_height_and_centres_horizontally() {
        let b = pane(2000., 600.);
        let (_, sb, _) = place_slide(b, SlideSize::WIDE, 20., 1.0, point(0., 0.));
        assert!(close(sb.size.height, 560.));
        let left = f32::from(sb.left() - b.left());
        let right = f32::from(b.right() - sb.right());
        assert!((left - right).abs() < 0.5);
        assert!(close(sb.top() - b.top(), 20.));
    }

    /// Re-fitting on resize: the slide follows the pane.
    #[std::prelude::v1::test]
    fn fit_follows_the_pane_size() {
        let (_, a, _) = place_slide(pane(800., 800.), SlideSize::WIDE, 16., 1.0, point(0., 0.));
        let (_, b, _) = place_slide(pane(1600., 800.), SlideSize::WIDE, 16., 1.0, point(0., 0.));
        assert!(b.size.width > a.size.width);
    }

    #[std::prelude::v1::test]
    fn view_maps_slide_corners_to_the_slide_rect() {
        let (v, sb, _) = place_slide(pane(1000., 800.), SlideSize::WIDE, 32., 1.0, point(0., 0.));
        let (x, y) = v.to_emu(sb.origin);
        assert!(x.abs() < 20_000 && y.abs() < 20_000);
        let r = v.rect(&slides_engine::Xfrm::new(
            0,
            0,
            SlideSize::WIDE.cx,
            SlideSize::WIDE.cy,
        ));
        assert!(close(r.size.width, f32::from(sb.size.width)));
    }

    /// Zoomed in past the pane: the slide starts at the margin and scrolls,
    /// with the scroll clamped to the overflow.
    #[std::prelude::v1::test]
    fn zoom_overflows_and_scroll_is_clamped() {
        let b = pane(1000., 800.);
        let (_, sb, s) = place_slide(b, SlideSize::WIDE, 32., 2.0, point(-50., 1e6));
        assert!(close(sb.size.width, 1872.));
        assert_eq!(s.x, 0.0);
        assert!(close(sb.left() - b.left(), 32.));
        // Bottom edge sits a margin above the pane's bottom at full scroll.
        assert!(close(b.bottom() - sb.bottom(), 32.));
        let (_, sb2, s2) = place_slide(b, SlideSize::WIDE, 32., 2.0, point(100., 0.));
        assert_eq!(s2.x, 100.0);
        assert!(close(sb2.left() - b.left(), -68.));
    }

    /// Zoomed out: smaller than fit, still centred.
    #[std::prelude::v1::test]
    fn zoom_out_stays_centred() {
        let b = pane(1000., 800.);
        let (_, sb, s) = place_slide(b, SlideSize::WIDE, 32., 0.5, point(30., 30.));
        assert!(close(sb.size.width, 468.));
        assert!(close(sb.center().x, 510.) && close(sb.center().y, 420.));
        assert_eq!(s, point(0., 0.));
    }

    #[std::prelude::v1::test]
    fn zoom_keeps_the_point_under_the_pointer() {
        let b = pane(1000., 800.);
        let at = point(px(300.), px(250.));
        let (v1, _, _) = place_slide(b, SlideSize::WIDE, 32., 1.0, point(0., 0.));
        let (ex, ey) = v1.to_emu(at);
        let (v0, _, _) = place_slide(b, SlideSize::WIDE, 32., 3.0, point(0., 0.));
        let s = scroll_keeping(b, 32., v0.scale, (ex as f32, ey as f32), at);
        let (v, _, _) = place_slide(b, SlideSize::WIDE, 32., 3.0, s);
        let p = v.point(ex as f32, ey as f32);
        assert!(close(p.x, 300.) && close(p.y, 250.), "{p:?}");
    }

    #[std::prelude::v1::test]
    fn margin_shrinks_in_small_panes() {
        assert_eq!(canvas_margin(size(px(2000.), px(1200.))), SLIDE_MARGIN);
        assert_eq!(canvas_margin(size(px(200.), px(150.))), MIN_MARGIN);
        let m = canvas_margin(size(px(600.), px(500.)));
        assert!(m > MIN_MARGIN && m < SLIDE_MARGIN);
    }

    #[std::prelude::v1::test]
    fn handle_cursors() {
        assert_eq!(handle_cursor((-1, -1)), CursorStyle::ResizeUpLeftDownRight);
        assert_eq!(handle_cursor((1, 1)), CursorStyle::ResizeUpLeftDownRight);
        assert_eq!(handle_cursor((1, -1)), CursorStyle::ResizeUpRightDownLeft);
        assert_eq!(handle_cursor((-1, 1)), CursorStyle::ResizeUpRightDownLeft);
        assert_eq!(handle_cursor((0, -1)), CursorStyle::ResizeUpDown);
        assert_eq!(handle_cursor((1, 0)), CursorStyle::ResizeLeftRight);
    }
}
