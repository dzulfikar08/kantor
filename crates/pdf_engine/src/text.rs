//! A page's text with the place of every character: what search,
//! selection and copy work on.
//!
//! The page is interpreted with a device that only records glyphs: each
//! glyph's Unicode (`ToUnicode`, glyph names, standard encodings) and its
//! box. Characters come in content-stream order, the order nearly every
//! producer writes text in; a space goes in where two glyphs on one line are
//! apart by more than a fraction of the font size, a line break where the
//! baseline moves. Invisible text (OCR layers of scanned pages) counts.

use std::ops::Range;

use hayro::hayro_interpret::font::{Glyph, GlyphRun};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps,
    InterpreterCache, InterpreterSettings, SoftMask, TransformExt as _, interpret_page,
};
use hayro::kurbo::{Affine, BezPath, Point, Rect, Vec2};

use crate::PdfDoc;

/// One character's box on the page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharBox {
    /// Byte range in [`PageText::text`].
    pub start: u32,
    pub end: u32,
    /// `[x0, y0, x1, y1]`, page space.
    pub rect: [f32; 4],
}

/// The text of a page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageText {
    /// Lines separated by `\n`; words by spaces (inserted where glyphs are
    /// apart, they have no box).
    pub text: String,
    /// The boxes of the characters that were drawn, in text order.
    pub boxes: Vec<CharBox>,
}

/// A search hit: page and byte range in its [`PageText::text`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub page: usize,
    pub range: Range<usize>,
}

impl PageText {
    /// The text as searched: line breaks count as spaces, so a phrase that
    /// wraps is found (same byte offsets as [`Self::text`]).
    pub fn search_text(&self) -> String {
        self.text.replace('\n', " ")
    }

    /// Rectangles covering the characters in `range`, one per run of
    /// characters on a line.
    pub fn rects(&self, range: Range<usize>) -> Vec<[f32; 4]> {
        let mut out: Vec<[f32; 4]> = Vec::new();
        for b in &self.boxes {
            let (s, e) = (b.start as usize, b.end as usize);
            if e <= range.start || s >= range.end {
                continue;
            }
            let r = b.rect;
            if let Some(last) = out.last_mut() {
                let h = (last[3] - last[1]).max(r[3] - r[1]);
                let overlap = last[3].min(r[3]) - last[1].max(r[1]);
                let gap = r[0] - last[2];
                if overlap > h * 0.5 && gap < h * 1.5 && gap > -h {
                    last[0] = last[0].min(r[0]);
                    last[1] = last[1].min(r[1]);
                    last[2] = last[2].max(r[2]);
                    last[3] = last[3].max(r[3]);
                    continue;
                }
            }
            out.push(r);
        }
        out
    }

    /// The text offset nearest to the point (page space): before or after
    /// the character under it, whichever half it is in; with no character
    /// there, the closest one on the nearest line.
    pub fn offset_at(&self, x: f32, y: f32) -> Option<usize> {
        let mut best: Option<(f32, &CharBox)> = None;
        for b in &self.boxes {
            let r = b.rect;
            let dx = if x < r[0] {
                r[0] - x
            } else if x > r[2] {
                x - r[2]
            } else {
                0.0
            };
            let dy = if y < r[1] {
                r[1] - y
            } else if y > r[3] {
                y - r[3]
            } else {
                0.0
            };
            // Being on the line matters more than being near along it.
            let d = dx + dy * 4.0;
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, b));
            }
        }
        let (_, b) = best?;
        let mid = (b.rect[0] + b.rect[2]) / 2.0;
        Some(if x < mid {
            b.start as usize
        } else {
            b.end as usize
        })
    }

    /// Whether the point is on a character (the I-beam cursor shows).
    pub fn is_on_text(&self, x: f32, y: f32) -> bool {
        self.boxes
            .iter()
            .any(|b| x >= b.rect[0] && x <= b.rect[2] && y >= b.rect[1] && y <= b.rect[3])
    }

    /// The word around byte `at` (double-click).
    pub fn word_at(&self, at: usize) -> Range<usize> {
        let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '\'' || c == '-';
        let at = at.min(self.text.len());
        let start = self.text[..at]
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_word(*c))
            .last()
            .map_or(at, |(i, _)| i);
        let end = self.text[at..]
            .char_indices()
            .find(|(_, c)| !is_word(*c))
            .map_or(self.text.len(), |(i, _)| at + i);
        start..end
    }

    /// The line around byte `at` (triple-click).
    pub fn line_at(&self, at: usize) -> Range<usize> {
        let at = at.min(self.text.len());
        let start = self.text[..at].rfind('\n').map_or(0, |i| i + 1);
        let end = self.text[at..]
            .find('\n')
            .map_or(self.text.len(), |i| at + i);
        start..end
    }
}

/// Every match of `needle` on one page (see [`PageText::search_text`]).
pub fn find_in(
    page: usize,
    text: &PageText,
    needle: &str,
    match_case: bool,
    whole_word: bool,
) -> Vec<Match> {
    let hay = text.search_text();
    let needle = needle.replace(['\n', '\r', '\t'], " ");
    find_all(&hay, &needle, match_case, whole_word)
        .into_iter()
        .map(|range| Match { page, range })
        .collect()
}

/// Case-folded, whole-word aware substring search (byte ranges on char
/// boundaries). Kept here so the engine doesn't need the UI's crates.
fn find_all(text: &str, needle: &str, match_case: bool, whole_word: bool) -> Vec<Range<usize>> {
    let fold = |c: char, out: &mut Vec<char>| {
        if match_case {
            out.push(c)
        } else {
            out.extend(c.to_lowercase())
        }
    };
    let mut folded = Vec::new();
    for c in needle.chars() {
        fold(c, &mut folded);
    }
    let mut out = Vec::new();
    if folded.iter().all(|c| c.is_whitespace()) {
        return out;
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut from = 0;
    let mut buf = Vec::with_capacity(2);
    for (start, _) in text.char_indices() {
        if start < from {
            continue;
        }
        let mut i = 0;
        let mut end = None;
        'chars: for (off, c) in text[start..].char_indices() {
            buf.clear();
            fold(c, &mut buf);
            for f in &buf {
                if folded.get(i) != Some(f) {
                    break 'chars;
                }
                i += 1;
            }
            if i == folded.len() {
                end = Some(start + off + c.len_utf8());
                break;
            }
        }
        let Some(end) = end else {
            continue;
        };
        if whole_word {
            let before = text[..start].chars().next_back();
            let after = text[end..].chars().next();
            let first = text[start..].chars().next();
            let last = text[..end].chars().next_back();
            let left_ok = !(before.is_some_and(is_word) && first.is_some_and(is_word));
            let right_ok = !(after.is_some_and(is_word) && last.is_some_and(is_word));
            if !(left_ok && right_ok) {
                continue;
            }
        }
        out.push(start..end);
        from = end;
    }
    out
}

impl PdfDoc {
    /// The text of page `page` (empty for a page without text).
    pub fn text(&self, page: usize) -> PageText {
        let Some(p) = self.pdf().pages().get(page) else {
            return PageText::default();
        };
        let (w, h) = p.render_dimensions();
        let cache = InterpreterCache::new();
        let settings = InterpreterSettings::default();
        let mut ctx = Context::new(
            p.initial_transform(true).to_kurbo(),
            Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
            &cache,
            p.xref(),
            settings,
        );
        let mut dev = TextDevice::default();
        interpret_page(p, &mut ctx, &mut dev);
        dev.out
    }
}

/// The last glyph placed, to decide on spaces and line breaks.
struct Pen {
    /// Where the next glyph would start (end of the advance).
    end: Point,
    /// The baseline direction (unit) and the font size (page units).
    dir: Vec2,
    size: f64,
}

#[derive(Default)]
struct TextDevice {
    out: PageText,
    pen: Option<Pen>,
}

impl TextDevice {
    fn glyph(&mut self, t: Affine, adv: f64, text: &str) {
        let origin = t * Point::ORIGIN;
        let x_axis = t * Point::new(1000.0, 0.0) - origin;
        let y_axis = t * Point::new(0.0, 1000.0) - origin;
        let size = y_axis.length().max(x_axis.length() * 0.5);
        if !size.is_finite() || size < 0.05 {
            return;
        }
        let dir = if x_axis.length() > 1e-6 {
            x_axis / x_axis.length()
        } else {
            Vec2::new(1.0, 0.0)
        };
        if let Some(pen) = &self.pen {
            let d = origin - pen.end;
            // Distance along the line and across it, in the font size.
            let along = d.dot(pen.dir) / pen.size;
            let across = d.cross(pen.dir).abs() / pen.size;
            let ends_blank = self.out.text.ends_with(char::is_whitespace);
            let starts_blank = text.starts_with(char::is_whitespace);
            if across > 0.6 || along < -2.0 {
                if !self.out.text.is_empty() {
                    while self.out.text.ends_with(' ') {
                        self.out.text.pop();
                    }
                    if !self.out.text.ends_with('\n') {
                        self.out.text.push('\n');
                    }
                }
            } else if along > 0.2 && !ends_blank && !starts_blank {
                self.out.text.push(' ');
            }
        }
        let corners = [
            t * Point::new(0.0, -220.0),
            t * Point::new(adv, -220.0),
            t * Point::new(0.0, 880.0),
            t * Point::new(adv, 880.0),
        ];
        let x0 = corners.iter().map(|p| p.x).fold(f64::MAX, f64::min);
        let y0 = corners.iter().map(|p| p.y).fold(f64::MAX, f64::min);
        let x1 = corners.iter().map(|p| p.x).fold(f64::MIN, f64::max);
        let y1 = corners.iter().map(|p| p.y).fold(f64::MIN, f64::max);
        let start = self.out.text.len() as u32;
        self.out.text.push_str(text);
        let end = self.out.text.len() as u32;
        if end > start {
            self.out.boxes.push(CharBox {
                start,
                end,
                rect: [x0 as f32, y0 as f32, x1 as f32, y1 as f32],
            });
        }
        self.pen = Some(Pen {
            end: t * Point::new(adv, 0.0),
            dir,
            size,
        });
    }
}

/// What a glyph stands for, with ligatures spelled out (so “ﬁnd” is found
/// as “find”) and control characters dropped.
fn glyph_text(g: &Glyph<'_>) -> String {
    let s = match g.as_unicode() {
        Some(BfString::Char(c)) => c.to_string(),
        Some(BfString::String(s)) => s,
        None => return "\u{FFFD}".into(),
    };
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'ﬀ' => out.push_str("ff"),
            'ﬁ' => out.push_str("fi"),
            'ﬂ' => out.push_str("fl"),
            'ﬃ' => out.push_str("ffi"),
            'ﬄ' => out.push_str("ffl"),
            'ﬅ' | 'ﬆ' => out.push_str("st"),
            '\u{00AD}' => {}
            c if c.is_control() => {}
            '\u{00A0}' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

fn advance(g: &Glyph<'_>) -> f64 {
    let adv = match g {
        Glyph::Outline(o) => o.advance_width(),
        Glyph::Type3(t) => t.advance_width(),
    };
    match adv {
        Some(a) if a.is_finite() && a > 0.0 => f64::from(a),
        _ => 500.0,
    }
}

impl<'a> Device<'a> for TextDevice {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
        for g in run.glyphs() {
            let text = glyph_text(g);
            if text.is_empty() {
                continue;
            }
            self.glyph(props.transform * g.transform(), advance(g), &text);
        }
    }
    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}
