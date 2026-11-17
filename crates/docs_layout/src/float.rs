//! Floating objects (Word's `wp:anchor` pictures, shapes and text boxes):
//! where they go on the page and the room they take from the text.
//!
//! A floating object belongs to its anchor paragraph ([`FloatSpec`] in
//! [`crate::Paragraph::floats`]) and lands on the page where that
//! paragraph's first line does. Its position is measured from a reference
//! (the page, the margins, the paragraph, the line, the character) as Word
//! does; text wraps around it per [`FloatWrap`]:
//!
//! - `Square` (Word's Square, Tight and Through): lines next to the
//!   object's box (plus its distance from text) are shortened to the free
//!   stretches beside it. With a wrap polygon (Tight, Through) the
//!   polygon's extent at each line's height is used instead of the box.
//! - `TopAndBottom`: no text beside it; lines continue below.
//! - `None`: in front of or behind the text, which doesn't move.

/// What a horizontal position is measured from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FloatRefH {
    Page,
    Margin,
    Column,
    Character,
    LeftMargin,
    RightMargin,
    InsideMargin,
    OutsideMargin,
}

/// What a vertical position is measured from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FloatRefV {
    Page,
    Margin,
    Paragraph,
    Line,
    TopMargin,
    BottomMargin,
    InsideMargin,
    OutsideMargin,
}

/// Where in its reference an object goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FloatPos {
    /// Left or top edge on the reference's.
    Start,
    Center,
    /// Right or bottom edge on the reference's.
    End,
    /// The inside edge (left on odd pages).
    Inside,
    Outside,
    /// px from the reference's left or top.
    Offset(f32),
    /// A fraction of the reference's width or height from its left or top.
    Pct(f32),
}

/// How text treats an object.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FloatWrap {
    /// In front of or behind the text.
    #[default]
    None,
    Square,
    TopAndBottom,
}

/// Which side of a `Square` object text may use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FloatSide {
    #[default]
    Both,
    Left,
    Right,
    Largest,
}

/// A floating object anchored in a paragraph.
#[derive(Clone, Debug, PartialEq)]
pub struct FloatSpec {
    /// Byte offset of its object character in the paragraph.
    pub index: usize,
    pub h: (FloatRefH, FloatPos),
    pub v: (FloatRefV, FloatPos),
    /// Size, px.
    pub width: f32,
    pub height: f32,
    /// Distance kept from text: top, bottom, left, right (px).
    pub dist: [f32; 4],
    pub wrap: FloatWrap,
    pub side: FloatSide,
    /// Drawn behind the text.
    pub behind: bool,
    /// Z-order: higher is drawn on top.
    pub z: u32,
    /// The wrap polygon (Tight, Through) as fractions of the box.
    pub polygon: Option<Vec<[f32; 2]>>,
    /// Through wrapping: text also goes into the polygon's hollows (the
    /// gaps between its parts across a line), not only around it.
    pub through: bool,
    /// How far the object reaches past its box on the left, top, right
    /// and bottom (px; negative inside it): a turned object's turned
    /// bounds, which text wraps around as in Word.
    pub extent: [f32; 4],
}

/// A floating object placed on a page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedFloat {
    pub para: usize,
    pub index: usize,
    /// `[x, y, w, h]`, page coordinates.
    pub rect: [f32; 4],
    pub z: u32,
    pub behind: bool,
    pub wrap: FloatWrap,
}

/// A wrap polygon on the page (px) and the distance from text kept left
/// and right of it.
pub type WrapPolygon = (std::sync::Arc<[[f32; 2]]>, [f32; 2]);

/// Room a wrapping object takes from the text on its page.
#[derive(Clone, Debug, PartialEq)]
pub struct WrapArea {
    /// The object's box grown by its distance from text: x0, y0, x1, y1.
    pub area: [f32; 4],
    pub wrap: FloatWrap,
    pub side: FloatSide,
    /// The object (paragraph, byte index), to tell repeats apart.
    pub key: (usize, usize),
    /// Its wrap polygon on the page (px) and the distance from text kept
    /// left and right of it, when text follows it.
    pub polygon: Option<WrapPolygon>,
    /// Through: text goes into the polygon's hollows too.
    pub through: bool,
}

/// Stretches narrower than this (px) get no text.
pub(crate) const MIN_SEGMENT: f32 = 24.0;

/// Where the anchor's paragraph and line are, for positions relative to
/// them, and the anchor character's x.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AnchorAt {
    /// Top of the paragraph (after its space before), page coordinates.
    pub para_top: f32,
    /// Top of the line holding the anchor.
    pub line_top: f32,
    /// x of the anchor character.
    pub char_x: f32,
    /// The text column holding the anchor (left edge and width, page
    /// coordinates), when the section has columns; else the margins.
    pub column: Option<(f32, f32)>,
}

/// An object's box on a page with geometry `setup`, page number `number`
/// (odd pages have the inside margin on the left).
pub fn float_rect(
    spec: &FloatSpec,
    setup: &crate::PageSetup,
    number: u32,
    at: AnchorAt,
) -> [f32; 4] {
    let odd = number % 2 == 1;
    let (w, h) = (spec.width, spec.height);
    let left_margin = (0.0, setup.margin_left);
    let right_margin = (setup.width - setup.margin_right, setup.margin_right);
    let (rx, rw) = match spec.h.0 {
        FloatRefH::Page => (0.0, setup.width),
        FloatRefH::Margin => (setup.margin_left, setup.content_width()),
        FloatRefH::Column => at
            .column
            .unwrap_or((setup.margin_left, setup.content_width())),
        FloatRefH::Character => (at.char_x, 0.0),
        FloatRefH::LeftMargin => left_margin,
        FloatRefH::RightMargin => right_margin,
        FloatRefH::InsideMargin if odd => left_margin,
        FloatRefH::InsideMargin => right_margin,
        FloatRefH::OutsideMargin if odd => right_margin,
        FloatRefH::OutsideMargin => left_margin,
    };
    let top_margin = (0.0, setup.margin_top);
    let bottom_margin = (setup.height - setup.margin_bottom, setup.margin_bottom);
    let (ry, rh) = match spec.v.0 {
        FloatRefV::Page => (0.0, setup.height),
        FloatRefV::Margin => (setup.margin_top, setup.content_height()),
        FloatRefV::Paragraph => (at.para_top, 0.0),
        FloatRefV::Line => (at.line_top, 0.0),
        FloatRefV::TopMargin | FloatRefV::InsideMargin => top_margin,
        FloatRefV::BottomMargin | FloatRefV::OutsideMargin => bottom_margin,
    };
    let place = |r: f32, len: f32, size: f32, pos: FloatPos, inside_start: bool| match pos {
        FloatPos::Start => r,
        FloatPos::Center => r + (len - size) / 2.0,
        FloatPos::End => r + len - size,
        FloatPos::Inside if inside_start => r,
        FloatPos::Inside => r + len - size,
        FloatPos::Outside if inside_start => r + len - size,
        FloatPos::Outside => r,
        FloatPos::Offset(o) => r + o,
        FloatPos::Pct(p) => r + len * p,
    };
    [
        place(rx, rw, w, spec.h.1, odd),
        place(ry, rh, h, spec.v.1, true),
        w,
        h,
    ]
}

impl WrapArea {
    pub(crate) fn of(spec: &FloatSpec, rect: [f32; 4], key: (usize, usize)) -> Option<Self> {
        if spec.wrap == FloatWrap::None {
            return None;
        }
        let [x, y, w, h] = rect;
        let [t, b, l, r] = spec.dist;
        // (Negative when the turned bounds are narrower than the box.)
        let [el, et, er, eb] = spec.extent.map(|v| v.max(-w.min(h) / 2.0));
        let polygon = spec
            .polygon
            .as_ref()
            .filter(|p| p.len() >= 3 && spec.wrap == FloatWrap::Square)
            .map(|p| {
                let pts: std::sync::Arc<[[f32; 2]]> =
                    p.iter().map(|[px, py]| [x + px * w, y + py * h]).collect();
                (pts, [l, r])
            });
        Some(Self {
            area: [x - l - el, y - t - et, x + w + r + er, y + h + b + eb],
            wrap: spec.wrap,
            side: spec.side,
            key,
            polygon,
            through: spec.through,
        })
    }

    /// What the object takes between `top` and `bottom`, as stretches:
    /// one (its box, or its wrap polygon's extent there) except with
    /// Through, where each part of the polygon across the band is one
    /// and the hollows between them stay free (each grown by the distance
    /// from text). `None` when the band is free.
    fn spans(&self, top: f32, bottom: f32) -> Option<Vec<(f32, f32)>> {
        let whole = self.span(top, bottom)?;
        let Some((pts, [l, r])) = self.polygon.as_ref().filter(|_| self.through) else {
            return Some(vec![whole]);
        };
        let mut parts: Vec<(f32, f32)> = Vec::new();
        let n = pts.len();
        // The polygon's inside on a few lines across the band (even-odd).
        let steps = 4;
        for k in 0..=steps {
            let y = top + (bottom - top) * k as f32 / steps as f32;
            let mut xs: Vec<f32> = Vec::new();
            for i in 0..n {
                let (a, b) = (pts[i], pts[(i + 1) % n]);
                if (a[1] <= y && b[1] > y) || (b[1] <= y && a[1] > y) {
                    xs.push(a[0] + (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]));
                }
            }
            xs.sort_by(f32::total_cmp);
            parts.extend(xs.as_chunks::<2>().0.iter().map(|c| (c[0], c[1])));
        }
        // Edges crossing the band take their stretch too.
        for i in 0..n {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            let (y0, y1) = (a[1].min(b[1]), a[1].max(b[1]));
            if y1 < top || y0 > bottom {
                continue;
            }
            let at = |y: f32| {
                if (b[1] - a[1]).abs() < 1e-6 {
                    None
                } else {
                    Some(a[0] + (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]))
                }
            };
            let (xa, xb) = match (at(top.max(y0)), at(bottom.min(y1))) {
                (Some(p), Some(q)) => (p.min(q), p.max(q)),
                _ => (a[0].min(b[0]), a[0].max(b[0])),
            };
            parts.push((xa, xb));
        }
        if parts.is_empty() {
            return Some(vec![whole]);
        }
        parts.sort_by(|p, q| p.0.total_cmp(&q.0));
        let mut merged: Vec<(f32, f32)> = Vec::new();
        for (a, b) in parts {
            let (a, b) = (a - l, b + r);
            match merged.last_mut() {
                // Gaps too narrow for text count as taken.
                Some(m) if a <= m.1 + MIN_SEGMENT => m.1 = m.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        Some(merged)
    }

    /// Left and right of what the object takes between `top` and `bottom`
    /// (its box, or its wrap polygon there grown by the distance from
    /// text); `None` when the polygon leaves that band free.
    fn span(&self, top: f32, bottom: f32) -> Option<(f32, f32)> {
        let Some((pts, [l, r])) = &self.polygon else {
            return Some((self.area[0], self.area[2]));
        };
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        let n = pts.len();
        for i in 0..n {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            let (y0, y1) = (a[1].min(b[1]), a[1].max(b[1]));
            if y1 < top || y0 > bottom {
                continue;
            }
            // The edge clipped to the band.
            for y in [top.max(y0), bottom.min(y1)] {
                let x = if (b[1] - a[1]).abs() < 1e-6 {
                    if y == top.max(y0) { a[0] } else { b[0] }
                } else {
                    a[0] + (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1])
                };
                lo = lo.min(x);
                hi = hi.max(x);
            }
        }
        (lo <= hi).then_some((lo - l, hi + r))
    }

    fn touches(&self, top: f32, bottom: f32) -> bool {
        self.area[1] < bottom && self.area[3] > top
    }
}

/// The stretches of `[x0, x1]` free for text between `top` and `bottom`;
/// `Err(y)`: none, and nothing frees up before `y`.
pub(crate) fn free_segments(
    excl: &[WrapArea],
    (x0, x1): (f32, f32),
    top: f32,
    bottom: f32,
) -> Result<Vec<(f32, f32)>, f32> {
    let mut segs = vec![(x0, x1)];
    let mut blocked_to = f32::INFINITY;
    let mut any = false;
    for e in excl.iter().filter(|e| e.touches(top, bottom)) {
        let [_, _, _, ey1] = e.area;
        let Some(taken) = e.spans(top, bottom) else {
            continue;
        };
        let (ex0, ex1) = (taken[0].0, taken[taken.len() - 1].1);
        any = true;
        if e.wrap == FloatWrap::TopAndBottom {
            // Only when it's beside the column at all.
            if ex1 > x0 && ex0 < x1 {
                return Err(ey1);
            }
            continue;
        }
        let cuts: Vec<(f32, f32)> = match e.side {
            FloatSide::Left => vec![(ex0, f32::INFINITY)],
            FloatSide::Right => vec![(f32::NEG_INFINITY, ex1)],
            FloatSide::Both | FloatSide::Largest => taken,
        };
        for (cut0, cut1) in cuts {
            let mut next = Vec::new();
            for (a, b) in segs {
                if cut1 <= a || cut0 >= b {
                    next.push((a, b));
                    continue;
                }
                if cut0 > a {
                    next.push((a, cut0));
                }
                if cut1 < b {
                    next.push((cut1, b));
                }
            }
            segs = next;
        }
        if e.side == FloatSide::Largest
            && let Some(best) = segs
                .iter()
                .copied()
                .max_by(|p, q| (p.1 - p.0).total_cmp(&(q.1 - q.0)))
        {
            segs = vec![best];
        }
        // A polygon may free up before its box's bottom: try again a
        // little lower.
        blocked_to = blocked_to.min(if e.polygon.is_some() {
            (top + 2.0).min(ey1)
        } else {
            ey1
        });
    }
    segs.retain(|(a, b)| b - a >= MIN_SEGMENT);
    if segs.is_empty() {
        // Nothing wide enough: skip past the lowest-reaching object that's
        // in the way (at least one is when `any`).
        return Err(if any { blocked_to } else { top + 1.0 });
    }
    Ok(segs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PageSetup;

    fn spec(h: (FloatRefH, FloatPos), v: (FloatRefV, FloatPos)) -> FloatSpec {
        FloatSpec {
            index: 0,
            h,
            v,
            width: 100.0,
            height: 50.0,
            dist: [0.0, 0.0, 12.0, 12.0],
            wrap: FloatWrap::Square,
            side: FloatSide::Both,
            behind: false,
            z: 1,
            polygon: None,
            through: false,
            extent: [0.0; 4],
        }
    }

    #[test]
    fn positions_per_reference() {
        let s = PageSetup::a4();
        let at = AnchorAt {
            para_top: 300.0,
            line_top: 320.0,
            char_x: 210.0,
            column: None,
        };
        let rect = |h, v| float_rect(&spec(h, v), &s, 1, at);
        // Offsets from each horizontal reference.
        let off = FloatPos::Offset(10.0);
        let top = (FloatRefV::Page, FloatPos::Offset(0.0));
        assert_eq!(rect((FloatRefH::Page, off), top)[0], 10.0);
        assert_eq!(rect((FloatRefH::Margin, off), top)[0], s.margin_left + 10.0);
        assert_eq!(rect((FloatRefH::Column, off), top)[0], s.margin_left + 10.0);
        let in_col = AnchorAt {
            column: Some((400.0, 200.0)),
            ..at
        };
        let r = float_rect(&spec((FloatRefH::Column, off), top), &s, 1, in_col);
        assert_eq!(r[0], 410.0, "relative to its column");
        assert_eq!(rect((FloatRefH::Character, off), top)[0], 220.0);
        assert_eq!(rect((FloatRefH::LeftMargin, off), top)[0], 10.0);
        let rm = s.width - s.margin_right;
        assert_eq!(rect((FloatRefH::RightMargin, off), top)[0], rm + 10.0);
        // Alignment in the margin box; inside/outside follow the page.
        let right = rect((FloatRefH::Margin, FloatPos::End), top)[0];
        assert!((right - (s.width - s.margin_right - 100.0)).abs() < 1e-3);
        let center = rect((FloatRefH::Page, FloatPos::Center), top)[0];
        assert!((center - (s.width - 100.0) / 2.0).abs() < 1e-3);
        assert_eq!(
            rect((FloatRefH::Margin, FloatPos::Inside), top)[0],
            s.margin_left
        );
        let even = float_rect(&spec((FloatRefH::Margin, FloatPos::Inside), top), &s, 2, at);
        assert!((even[0] - right).abs() < 1e-3);
        // Percentages of the reference.
        assert!(
            (rect((FloatRefH::Page, FloatPos::Pct(0.25)), top)[0] - s.width / 4.0).abs() < 1e-3
        );
        // Vertical references.
        let left = (FloatRefH::Page, FloatPos::Offset(0.0));
        assert_eq!(rect(left, (FloatRefV::Paragraph, off))[1], 310.0);
        assert_eq!(rect(left, (FloatRefV::Line, off))[1], 330.0);
        assert_eq!(rect(left, (FloatRefV::Margin, off))[1], s.margin_top + 10.0);
        assert_eq!(
            rect(left, (FloatRefV::Page, FloatPos::End))[1],
            s.height - 50.0
        );
        let bm = rect(left, (FloatRefV::BottomMargin, FloatPos::Start))[1];
        assert_eq!(bm, s.height - s.margin_bottom);
        assert_eq!(
            rect(left, (FloatRefV::TopMargin, FloatPos::Center))[1],
            (s.margin_top - 50.0) / 2.0
        );
    }

    #[test]
    fn segments_beside_objects() {
        let sp = spec(
            (FloatRefH::Page, FloatPos::Offset(200.0)),
            (FloatRefV::Page, FloatPos::Offset(100.0)),
        );
        let e = WrapArea::of(&sp, [200.0, 100.0, 100.0, 50.0], (0, 0)).unwrap();
        assert_eq!(e.area, [188.0, 100.0, 312.0, 150.0]);
        let col = (100.0, 500.0);
        // Above and below: the whole column.
        assert_eq!(
            free_segments(std::slice::from_ref(&e), col, 60.0, 90.0),
            Ok(vec![col])
        );
        assert_eq!(
            free_segments(std::slice::from_ref(&e), col, 150.0, 170.0),
            Ok(vec![col])
        );
        // Beside: both sides.
        assert_eq!(
            free_segments(std::slice::from_ref(&e), col, 110.0, 130.0),
            Ok(vec![(100.0, 188.0), (312.0, 500.0)])
        );
        let largest = WrapArea {
            side: FloatSide::Largest,
            ..e.clone()
        };
        assert_eq!(
            free_segments(&[largest], col, 110.0, 130.0),
            Ok(vec![(312.0, 500.0)])
        );
        let left = WrapArea {
            side: FloatSide::Left,
            ..e.clone()
        };
        assert_eq!(
            free_segments(&[left], col, 110.0, 130.0),
            Ok(vec![(100.0, 188.0)])
        );
        // Top and bottom: nothing beside it until its bottom.
        let tb = WrapArea {
            wrap: FloatWrap::TopAndBottom,
            ..e.clone()
        };
        assert_eq!(free_segments(&[tb], col, 110.0, 130.0), Err(150.0));
        // A full-width square object leaves no room either.
        let wide = WrapArea {
            area: [90.0, 100.0, 510.0, 150.0],
            ..e.clone()
        };
        assert_eq!(free_segments(&[wide], col, 110.0, 130.0), Err(150.0));
    }

    #[test]
    fn turned_bounds_widen_the_wrap_area() {
        let mut sp = spec(
            (FloatRefH::Page, FloatPos::Offset(100.0)),
            (FloatRefV::Page, FloatPos::Offset(100.0)),
        );
        let plain = WrapArea::of(&sp, [100.0, 100.0, 100.0, 50.0], (0, 0)).unwrap();
        assert_eq!(plain.area, [88.0, 100.0, 212.0, 150.0]);
        sp.extent = [5.0, 20.0, 5.0, 20.0];
        let turned = WrapArea::of(&sp, [100.0, 100.0, 100.0, 50.0], (0, 0)).unwrap();
        assert_eq!(turned.area, [83.0, 80.0, 217.0, 170.0]);
    }

    #[test]
    fn tight_wrap_follows_the_polygon() {
        // A triangle pointing down: wide at the top, a point at the bottom.
        let mut sp = spec(
            (FloatRefH::Page, FloatPos::Offset(200.0)),
            (FloatRefV::Page, FloatPos::Offset(100.0)),
        );
        sp.width = 100.0;
        sp.height = 100.0;
        sp.dist = [0.0, 0.0, 10.0, 10.0];
        sp.polygon = Some(vec![[0.0, 0.0], [1.0, 0.0], [0.5, 1.0]]);
        let e = WrapArea::of(&sp, [200.0, 100.0, 100.0, 100.0], (0, 0)).unwrap();
        let col = (100.0, 500.0);
        // Near the top: the whole width (plus the distance).
        assert_eq!(
            free_segments(std::slice::from_ref(&e), col, 100.0, 110.0),
            Ok(vec![(100.0, 190.0), (310.0, 500.0)])
        );
        // Near the point: text comes close to the middle.
        let Ok(segs) = free_segments(std::slice::from_ref(&e), col, 180.0, 190.0) else {
            panic!()
        };
        assert_eq!(segs.len(), 2);
        assert!(segs[0].1 > 220.0 && segs[0].1 < 250.0, "{segs:?}");
        assert!(segs[1].0 > 250.0 && segs[1].0 < 280.0, "{segs:?}");
        // Without a polygon, the box.
        let square = WrapArea {
            polygon: None,
            ..e.clone()
        };
        assert_eq!(
            free_segments(&[square], col, 180.0, 190.0),
            Ok(vec![(100.0, 190.0), (310.0, 500.0)])
        );
    }
}

#[cfg(test)]
mod through_tests {
    use super::*;

    #[test]
    fn through_wrap_fills_the_polygons_hollows() {
        // A "U": two uprights joined at the bottom, open at the top.
        let polygon = vec![
            [0.0, 0.0],
            [0.3, 0.0],
            [0.3, 0.7],
            [0.7, 0.7],
            [0.7, 0.0],
            [1.0, 0.0],
            [1.0, 1.0],
            [0.0, 1.0],
        ];
        let sp = FloatSpec {
            index: 0,
            h: (FloatRefH::Page, FloatPos::Offset(200.0)),
            v: (FloatRefV::Page, FloatPos::Offset(100.0)),
            width: 200.0,
            height: 100.0,
            dist: [0.0, 0.0, 5.0, 5.0],
            wrap: FloatWrap::Square,
            side: FloatSide::Both,
            behind: false,
            z: 1,
            polygon: Some(polygon),
            through: true,
            extent: [0.0; 4],
        };
        let e = WrapArea::of(&sp, [200.0, 100.0, 200.0, 100.0], (0, 0)).unwrap();
        let col = (100.0, 500.0);
        // Between the uprights: text goes into the hollow.
        let segs = free_segments(std::slice::from_ref(&e), col, 110.0, 125.0).unwrap();
        assert_eq!(segs.len(), 3, "{segs:?}");
        assert!((segs[1].0 - 265.0).abs() < 0.5 && (segs[1].1 - 335.0).abs() < 0.5);
        // Across the bottom bar: around it only.
        let segs = free_segments(std::slice::from_ref(&e), col, 180.0, 195.0).unwrap();
        assert_eq!(segs, vec![(100.0, 195.0), (405.0, 500.0)]);
        // Tight: around the outside only.
        let tight = WrapArea {
            through: false,
            ..e
        };
        let segs = free_segments(&[tight], col, 110.0, 125.0).unwrap();
        assert_eq!(segs, vec![(100.0, 195.0), (405.0, 500.0)]);
    }
}

#[cfg(test)]
mod flow_tests {
    use super::*;
    use crate::{Engine, PageSetup, PagedDocument, Paragraph};
    use std::sync::Arc;

    /// A paragraph of many short words (wraps at any font's width).
    fn words(n: usize) -> Paragraph {
        Paragraph::new(vec!["ab"; n].join(" "))
    }

    fn lay(paras: &[Paragraph]) -> PagedDocument {
        let mut e = Engine::new();
        let setup = PageSetup::a4();
        let layouts = paras
            .iter()
            .map(|p| Arc::new(e.layout_paragraph(p, setup.content_width())))
            .collect();
        PagedDocument::paginate(paras, layouts, setup)
    }

    fn float(wrap: FloatWrap, h: (FloatRefH, FloatPos), v: (FloatRefV, FloatPos)) -> FloatSpec {
        FloatSpec {
            index: 0,
            h,
            v,
            width: 200.0,
            height: 120.0,
            dist: [0.0, 0.0, 12.0, 12.0],
            wrap,
            side: FloatSide::Both,
            behind: false,
            z: 1,
            polygon: None,
            through: false,
            extent: [0.0; 4],
        }
    }

    fn overlaps(l: &crate::PlacedLine, r: [f32; 4]) -> bool {
        let v = l.top < r[1] + r[3] && l.top + l.height > r[1];
        let h = l.x < r[0] + r[2] && l.x + l.width > r[0];
        v && h
    }

    #[test]
    fn square_wrap_puts_lines_beside_the_object() {
        let s = PageSetup::a4();
        let mut p = words(400);
        p.text.insert(0, crate::OBJECT);
        p.floats.push(float(
            FloatWrap::Square,
            (FloatRefH::Margin, FloatPos::Start),
            (FloatRefV::Paragraph, FloatPos::Offset(0.0)),
        ));
        let doc = lay(&[p]);
        let page = &doc.pages[0];
        assert_eq!(page.floats.len(), 1);
        let r = page.floats[0].rect;
        assert_eq!(r[0], s.margin_left);
        assert_eq!(r[1], s.margin_top);
        let beside: Vec<_> = page.lines.iter().filter(|l| l.top < r[1] + r[3]).collect();
        assert!(!beside.is_empty());
        for l in &beside {
            // Right of the object and its distance from text.
            assert!(l.x >= r[0] + r[2] + 12.0 - 0.01, "{l:?}");
            assert!(!overlaps(l, r));
        }
        // Below it, lines take the whole column again.
        let below = page.lines.iter().find(|l| l.top >= r[1] + r[3]).unwrap();
        assert_eq!(below.x, s.margin_left);
        assert_eq!(below.width, s.content_width());
        // Every placed line's glyphs stay in its stretch.
        for run in doc.glyph_runs(0) {
            for g in &run.glyphs {
                if g.y - 4.0 < r[1] + r[3] && g.y > r[1] {
                    assert!(g.x >= r[0] + r[2], "glyph at {} in the object", g.x);
                }
            }
        }
    }

    #[test]
    fn both_sides_of_a_centred_object() {
        let s = PageSetup::a4();
        let mut p = words(400);
        p.text.insert(0, crate::OBJECT);
        p.floats.push(float(
            FloatWrap::Square,
            (FloatRefH::Margin, FloatPos::Center),
            (FloatRefV::Paragraph, FloatPos::Offset(0.0)),
        ));
        let doc = lay(&[p]);
        let r = doc.pages[0].floats[0].rect;
        let first_row: Vec<_> = doc.pages[0]
            .lines
            .iter()
            .filter(|l| (l.top - s.margin_top).abs() < 0.01)
            .collect();
        assert_eq!(first_row.len(), 2, "{first_row:?}");
        assert_eq!(first_row[0].x, s.margin_left);
        assert!(first_row[0].x + first_row[0].width <= r[0] - 12.0 + 0.01);
        assert!(first_row[1].x >= r[0] + r[2] + 12.0 - 0.01);
        // Lines are in text order: the right stretch continues the left.
        assert_eq!(first_row[1].line, first_row[0].line + 1);
        // Clicks and carets still work on both stretches.
        let (para, index) = doc
            .hit_test(0, first_row[1].x + 5.0, first_row[1].top + 2.0)
            .unwrap();
        assert_eq!(para, 0);
        let c = doc.caret(0, index).unwrap();
        let layout = &doc.layouts[0].layout;
        let rng = layout.get(first_row[1].line).unwrap().text_range();
        assert!(
            c.x >= r[0] + r[2],
            "{c:?} index {index} right line {rng:?} {first_row:?}"
        );
    }

    #[test]
    fn top_and_bottom_and_no_wrap() {
        let s = PageSetup::a4();
        let mk = |wrap| {
            let mut p = words(200);
            p.text.insert(0, crate::OBJECT);
            p.floats.push(float(
                wrap,
                (FloatRefH::Margin, FloatPos::End),
                (FloatRefV::Paragraph, FloatPos::Offset(30.0)),
            ));
            lay(&[Paragraph::new("ab"), p])
        };
        let doc = mk(FloatWrap::TopAndBottom);
        let r = doc.pages[0].floats[0].rect;
        assert!((r[0] + r[2] - (s.width - s.margin_right)).abs() < 0.01);
        for l in &doc.pages[0].lines {
            assert!(
                l.top + l.height <= r[1] + 0.01 || l.top >= r[1] + r[3] - 0.01,
                "{l:?} beside {r:?}"
            );
            assert_eq!(l.width, s.content_width());
        }
        // In front of the text: lines don't move.
        let front = mk(FloatWrap::None);
        let plain = lay(&[Paragraph::new("ab"), {
            let mut p = words(200);
            p.text.insert(0, crate::OBJECT);
            p
        }]);
        let tops = |d: &PagedDocument| d.pages[0].lines.iter().map(|l| l.top).collect::<Vec<_>>();
        assert_eq!(tops(&front), tops(&plain));
        assert_eq!(front.pages[0].floats.len(), 1);
    }

    #[test]
    fn page_fixed_objects_push_text_above_their_anchor() {
        let s = PageSetup::a4();
        // The object is anchored in the third paragraph but sits at the
        // top of the page: the first two wrap around it too.
        let mut anchor = words(10);
        anchor.text.insert(0, crate::OBJECT);
        anchor.floats.push(float(
            FloatWrap::TopAndBottom,
            (FloatRefH::Margin, FloatPos::Start),
            (FloatRefV::Margin, FloatPos::Start),
        ));
        let doc = lay(&[words(5), words(5), anchor]);
        let r = doc.pages[0].floats[0].rect;
        assert_eq!(r[1], s.margin_top);
        let first = doc.pages[0].lines.iter().find(|l| l.para == 0).unwrap();
        assert!(first.top >= r[1] + r[3] - 0.01, "{first:?}");
    }

    #[test]
    fn z_order_and_behind_are_reported() {
        let mut p = words(5);
        p.text.insert(0, crate::OBJECT);
        p.text.insert(0, crate::OBJECT);
        let mut back = float(
            FloatWrap::None,
            (FloatRefH::Page, FloatPos::Offset(0.0)),
            (FloatRefV::Page, FloatPos::Offset(0.0)),
        );
        back.behind = true;
        back.z = 7;
        let mut front = back.clone();
        front.index = crate::OBJECT.len_utf8();
        front.behind = false;
        front.z = 3;
        p.floats = vec![back, front];
        let doc = lay(&[p]);
        let f = &doc.pages[0].floats;
        assert_eq!(f.len(), 2);
        assert!(f[0].behind && f[0].z == 7);
        assert!(!f[1].behind && f[1].z == 3);
    }
}

#[cfg(test)]
mod note_tests {
    use crate::{
        Block, Engine, GapBlock, NOTE_SEPARATOR, NoteSplit, PageSetup, PagedDocument, Paragraph,
    };
    use std::sync::Arc;

    fn lay(paras: &[Paragraph], blocks: &[Block]) -> PagedDocument {
        let mut e = Engine::new();
        let setup = PageSetup::a4();
        let layouts = paras
            .iter()
            .map(|p| Arc::new(e.layout_paragraph(p, setup.content_width())))
            .collect();
        PagedDocument::paginate_blocks(paras, layouts, blocks, setup)
    }

    #[test]
    fn long_footnotes_continue_on_the_next_page() {
        let s = PageSetup::a4();
        // A short paragraph with a note of 100 lines of 20 px (2000 px:
        // longer than a page), then text.
        let mut p = Paragraph::new("ab");
        let breaks: Vec<f32> = (1..=100).map(|i| i as f32 * 20.0).collect();
        p.notes.push((0, 2000.0));
        p.note_splits.push(NoteSplit {
            index: 0,
            breaks,
            notice: 0.0,
        });
        let paras = vec![Paragraph::new("ab"), p, Paragraph::new("cd")];
        let blocks: Vec<Block> = (0..3).map(Block::Para).collect();
        let doc = lay(&paras, &blocks);
        let first = &doc.pages[0];
        // The reference stays on the first page with the note's start.
        assert!(first.lines.iter().any(|l| l.para == 1));
        assert_eq!(first.note_columns.len(), 1);
        let nc = &first.note_columns[0];
        assert_eq!(nc.parts.len(), 1);
        let a = nc.parts[0];
        assert!(a.more && a.from == 0.0 && a.to > 0.0);
        assert_eq!(a.to % 20.0, 0.0, "breaks at a line");
        // The rest of the first page's text area is the note's.
        let bottom = s.height - s.margin_bottom;
        assert!(nc.height + NOTE_SEPARATOR >= a.to);
        assert_eq!(nc.bottom, bottom);
        assert!(
            first
                .lines
                .iter()
                .all(|l| l.top + l.height <= bottom - nc.height + 0.01)
        );
        // It goes on (continued) on the next pages until it's all shown.
        let mut shown = a.to;
        for page in &doc.pages[1..] {
            for part in page.note_columns.iter().flat_map(|c| {
                assert!(c.continued);
                c.parts.iter()
            }) {
                assert_eq!(part.from, shown);
                shown = part.to;
            }
        }
        assert_eq!(shown, 2000.0);
        // Text after the reference goes on the next page, above the note.
        let next = doc
            .pages
            .iter()
            .position(|p| p.lines.iter().any(|l| l.para == 2))
            .unwrap();
        assert!(next >= 1);
    }

    #[test]
    fn gaps_break_at_their_lines() {
        let paras: Vec<Paragraph> = (0..3).map(|_| Paragraph::new("ab")).collect();
        let mut blocks: Vec<Block> = (0..3).map(Block::Para).collect();
        blocks.push(Block::Gap(GapBlock {
            id: 7,
            breaks: (1..=80).map(|i| i as f32 * 20.0).collect(),
            before: 12.0,
        }));
        let doc = lay(&paras, &blocks);
        let parts: Vec<_> = doc.pages.iter().flat_map(|p| p.gaps.iter()).collect();
        assert!(parts.len() >= 2, "{parts:?}");
        assert_eq!(parts[0].from, 0.0);
        for w in parts.windows(2) {
            assert_eq!(w[0].to, w[1].from);
            assert_eq!(w[0].to % 20.0, 0.0);
        }
        assert_eq!(parts.last().unwrap().to, 1600.0);
        // It starts right after the text.
        let last_line = doc.pages[0].lines.last().unwrap();
        assert!(
            (parts[0].y - (last_line.top + last_line.height + 8.0 * crate::PT + 12.0)).abs() < 0.5
        );
    }
}
