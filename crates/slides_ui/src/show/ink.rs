//! Ink drawn during a slide show (pen and highlighter strokes, the laser
//! pointer), kept per slide in slide coordinates: `0..1` across the
//! slide's width and height, so the same stroke lands in the same place on
//! the projector and in the presenter view's smaller copy. Strokes stay
//! while the show goes back and forth and are dropped when it ends.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// A point on the slide: `x` and `y` from 0 (left / top) to 1.
pub type SlidePoint = (f32, f32);

/// What the pointer does in the show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    /// Clicks go to the next slide.
    #[default]
    Arrow,
    Laser,
    Pen,
    Highlighter,
    Eraser,
}

impl Tool {
    /// Clicks and drags draw (or point) instead of advancing.
    pub fn draws(self) -> bool {
        self != Tool::Arrow
    }
}

/// Pen colours (RGB), the first is the default.
pub const PEN_COLORS: [u32; 6] = [0xE53935, 0x1E88E5, 0x43A047, 0xFDD835, 0x111111, 0xFFFFFF];
/// Highlighter colours (RGB), drawn translucent.
pub const HIGHLIGHTER_COLORS: [u32; 4] = [0xFFEB3B, 0x76FF03, 0x40C4FF, 0xFF4081];

/// Pen width as a part of the slide's width.
const PEN_WIDTH: f32 = 0.0035;
/// Highlighter width as a part of the slide's width.
const HIGHLIGHTER_WIDTH: f32 = 0.018;
/// How far around the eraser's tip strokes are hit (part of the width).
const ERASER_REACH: f32 = 0.012;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InkKind {
    Pen,
    Highlighter,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub kind: InkKind,
    pub color: u32,
    /// Width as a part of the slide's width.
    pub width: f32,
    pub points: Vec<SlidePoint>,
}

/// Strokes by slide index, and the stroke being drawn.
#[derive(Debug, Default)]
pub struct Ink {
    slides: HashMap<usize, Vec<Stroke>>,
    drawing: Option<(usize, Stroke)>,
}

impl Ink {
    /// The finished strokes on `slide`, then the one being drawn there.
    pub fn strokes(&self, slide: usize) -> impl Iterator<Item = &Stroke> {
        self.slides.get(&slide).into_iter().flatten().chain(
            self.drawing
                .iter()
                .filter(move |d| d.0 == slide)
                .map(|d| &d.1),
        )
    }

    #[allow(dead_code)] // tests
    pub fn has_ink(&self, slide: usize) -> bool {
        self.strokes(slide).next().is_some()
    }

    /// Start a stroke on `slide` at `p`.
    pub fn begin(&mut self, slide: usize, kind: InkKind, color: u32, p: SlidePoint) {
        self.finish();
        let width = match kind {
            InkKind::Pen => PEN_WIDTH,
            InkKind::Highlighter => HIGHLIGHTER_WIDTH,
        };
        self.drawing = Some((
            slide,
            Stroke {
                kind,
                color,
                width,
                points: vec![p],
            },
        ));
    }

    /// Extend the stroke being drawn (tiny moves are skipped).
    pub fn extend(&mut self, p: SlidePoint) {
        if let Some((_, s)) = &mut self.drawing
            && s.points.last().is_none_or(|&q| dist(q, p) > 0.0008)
        {
            s.points.push(p);
        }
    }

    #[allow(dead_code)] // tests
    pub fn is_drawing(&self) -> bool {
        self.drawing.is_some()
    }

    /// Keep the stroke being drawn.
    pub fn finish(&mut self) {
        if let Some((slide, s)) = self.drawing.take() {
            self.slides.entry(slide).or_default().push(s);
        }
    }

    /// Remove every stroke on `slide` the eraser at `p` touches. Returns
    /// whether something was removed.
    pub fn erase_at(&mut self, slide: usize, p: SlidePoint, aspect: f32) -> bool {
        let Some(strokes) = self.slides.get_mut(&slide) else {
            return false;
        };
        let before = strokes.len();
        strokes.retain(|s| !touches(s, p, ERASER_REACH, aspect));
        strokes.len() != before
    }

    /// Remove all ink on `slide`.
    pub fn clear(&mut self, slide: usize) {
        self.slides.remove(&slide);
        if self.drawing.as_ref().is_some_and(|d| d.0 == slide) {
            self.drawing = None;
        }
    }
}

/// Distance in slide-width units between two points of a slide whose
/// height is `aspect` times its width.
fn dist_aspect(a: SlidePoint, b: SlidePoint, aspect: f32) -> f32 {
    let dx = a.0 - b.0;
    let dy = (a.1 - b.1) * aspect;
    (dx * dx + dy * dy).sqrt()
}

fn dist(a: SlidePoint, b: SlidePoint) -> f32 {
    dist_aspect(a, b, 1.0)
}

/// Distance from `p` to the segment `a`–`b` (slide-width units).
fn seg_dist(p: SlidePoint, a: SlidePoint, b: SlidePoint, aspect: f32) -> f32 {
    let (ax, ay) = (a.0, a.1 * aspect);
    let (bx, by) = (b.0, b.1 * aspect);
    let (px, py) = (p.0, p.1 * aspect);
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (cx, cy) = (ax + t * dx, ay + t * dy);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// Whether a circle of radius `reach` at `p` touches the stroke.
pub fn touches(s: &Stroke, p: SlidePoint, reach: f32, aspect: f32) -> bool {
    let r = reach + s.width / 2.0;
    match s.points.as_slice() {
        [] => false,
        [one] => dist_aspect(*one, p, aspect) <= r,
        pts => pts.windows(2).any(|w| seg_dist(p, w[0], w[1], aspect) <= r),
    }
}

/// How long a laser trail point stays.
pub const LASER_FADE: Duration = Duration::from_millis(350);

/// The laser pointer: where it is, and its recent path (fading).
#[derive(Debug, Default)]
pub struct Laser {
    pub at: Option<(usize, SlidePoint)>,
    trail: VecDeque<(SlidePoint, Instant)>,
}

impl Laser {
    pub fn move_to(&mut self, slide: usize, p: SlidePoint, now: Instant) {
        if self.at.is_some_and(|(s, _)| s != slide) {
            self.trail.clear();
        }
        if let Some((_, q)) = self.at {
            self.trail.push_back((q, now));
        }
        self.at = Some((slide, p));
        self.prune(now);
    }

    pub fn hide(&mut self) {
        self.at = None;
        self.trail.clear();
    }

    /// Drop trail points older than [`LASER_FADE`].
    pub fn prune(&mut self, now: Instant) {
        while self
            .trail
            .front()
            .is_some_and(|(_, t)| now.saturating_duration_since(*t) > LASER_FADE)
        {
            self.trail.pop_front();
        }
    }

    /// Trail points with their remaining strength (1 = new, 0 = gone).
    pub fn trail(&self, now: Instant) -> impl Iterator<Item = (SlidePoint, f32)> + '_ {
        self.trail.iter().map(move |(p, t)| {
            let age = now.saturating_duration_since(*t).as_secs_f32();
            (*p, (1.0 - age / LASER_FADE.as_secs_f32()).clamp(0.0, 1.0))
        })
    }

    #[allow(dead_code)] // tests
    pub fn has_trail(&self) -> bool {
        !self.trail.is_empty()
    }
}

/// A rectangle on screen where a slide is drawn (any unit, usually px).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    /// A screen point as a slide point (`None` outside the slide).
    pub fn to_slide(&self, x: f32, y: f32) -> Option<SlidePoint> {
        if self.w <= 0.0 || self.h <= 0.0 {
            return None;
        }
        let p = ((x - self.x) / self.w, (y - self.y) / self.h);
        ((0.0..=1.0).contains(&p.0) && (0.0..=1.0).contains(&p.1)).then_some(p)
    }

    /// A screen point as a slide point, held to the slide's edge (a stroke
    /// dragged past the edge stays on it).
    pub fn to_slide_clamped(&self, x: f32, y: f32) -> SlidePoint {
        if self.w <= 0.0 || self.h <= 0.0 {
            return (0.0, 0.0);
        }
        (
            ((x - self.x) / self.w).clamp(0.0, 1.0),
            ((y - self.y) / self.h).clamp(0.0, 1.0),
        )
    }

    pub fn to_screen(&self, p: SlidePoint) -> (f32, f32) {
        (self.x + p.0 * self.w, self.y + p.1 * self.h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn strokes_are_kept_per_slide() {
        let mut ink = Ink::default();
        ink.begin(2, InkKind::Pen, PEN_COLORS[0], (0.1, 0.1));
        ink.extend((0.2, 0.2));
        ink.extend((0.2, 0.2)); // no movement: skipped
        assert!(ink.is_drawing());
        assert_eq!(ink.strokes(2).count(), 1);
        assert_eq!(ink.strokes(1).count(), 0);
        ink.finish();
        assert!(!ink.is_drawing());
        let s = ink.strokes(2).next().unwrap();
        assert_eq!(s.points, vec![(0.1, 0.1), (0.2, 0.2)]);
        ink.begin(2, InkKind::Highlighter, HIGHLIGHTER_COLORS[0], (0.5, 0.5));
        ink.finish();
        assert_eq!(ink.strokes(2).count(), 2);
        ink.clear(2);
        assert!(!ink.has_ink(2));
    }

    #[std::prelude::v1::test]
    fn eraser_removes_whole_strokes_it_touches() {
        let mut ink = Ink::default();
        ink.begin(0, InkKind::Pen, 0, (0.1, 0.5));
        ink.extend((0.9, 0.5));
        ink.begin(0, InkKind::Pen, 0, (0.1, 0.9));
        ink.extend((0.9, 0.9));
        ink.finish();
        // Near the middle of the first line's segment (not near a point).
        assert!(!ink.erase_at(0, (0.5, 0.7), 1.0));
        assert!(ink.erase_at(0, (0.5, 0.505), 1.0));
        assert_eq!(ink.strokes(0).count(), 1);
        assert_eq!(ink.strokes(0).next().unwrap().points[0], (0.1, 0.9));
        // Other slides aren't touched.
        assert!(!ink.erase_at(3, (0.5, 0.9), 1.0));
    }

    #[std::prelude::v1::test]
    fn hit_test_uses_the_slide_aspect() {
        let s = Stroke {
            kind: InkKind::Pen,
            color: 0,
            width: 0.0,
            points: vec![(0.5, 0.5)],
        };
        // 0.02 of the height on a 16:9 slide is 0.01125 of the width.
        assert!(touches(&s, (0.5, 0.52), 0.012, 9.0 / 16.0));
        assert!(!touches(&s, (0.5, 0.52), 0.012, 1.0));
    }

    #[std::prelude::v1::test]
    fn coordinates_map_between_windows() {
        let projector = Rect {
            x: 0.,
            y: 60.,
            w: 1920.,
            h: 960.,
        };
        let presenter = Rect {
            x: 24.,
            y: 80.,
            w: 800.,
            h: 400.,
        };
        let p = projector.to_slide(960., 300.).unwrap();
        assert_eq!(p, (0.5, 0.25));
        assert_eq!(presenter.to_screen(p), (424., 180.));
        assert_eq!(presenter.to_slide(424., 180.), Some(p));
        assert_eq!(projector.to_slide(960., 20.), None);
        assert_eq!(projector.to_slide_clamped(-5., 2000.), (0.0, 1.0));
        let empty = Rect {
            x: 0.,
            y: 0.,
            w: 0.,
            h: 0.,
        };
        assert_eq!(empty.to_slide(0., 0.), None);
    }

    #[std::prelude::v1::test]
    fn laser_trail_fades() {
        let t0 = Instant::now();
        let mut l = Laser::default();
        l.move_to(0, (0.1, 0.1), t0);
        assert!(!l.has_trail());
        l.move_to(0, (0.2, 0.2), t0 + Duration::from_millis(10));
        assert!(l.has_trail());
        let (p, a) = l.trail(t0 + Duration::from_millis(10)).next().unwrap();
        assert_eq!(p, (0.1, 0.1));
        assert!(a > 0.9);
        l.prune(t0 + Duration::from_secs(1));
        assert!(!l.has_trail());
        assert_eq!(l.at, Some((0, (0.2, 0.2))));
        // Another slide starts a new trail.
        l.move_to(0, (0.3, 0.3), t0 + Duration::from_secs(1));
        l.move_to(1, (0.3, 0.3), t0 + Duration::from_secs(1));
        assert_eq!(l.trail(t0 + Duration::from_secs(1)).count(), 1);
        l.hide();
        assert!(l.at.is_none());
    }
}
