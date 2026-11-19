//! A page's text with the place of every character: what search,
//! selection and copy work on.
//!
//! The page is interpreted with a device that only records glyphs: each
//! glyph's Unicode (`ToUnicode`, glyph names, standard encodings) and its
//! box. Characters come in content-stream order, the order nearly every
//! producer writes text in; a space goes in where two glyphs on one line are
//! apart by more than a fraction of the font size, a line break where the
//! baseline moves. Invisible text (OCR layers of scanned pages) counts.
//!
//! Two clean-ups follow what other readers do: a glyph drawn again on top
//! of itself (fake bold, shadows) counts once, and Arabic and Hebrew lines
//! that the file stores in visual order (left to right, as most producers
//! do) are put back in reading order, with presentation forms turned into
//! plain letters, so search and copy see the words as typed.

use std::ops::Range;

use hayro::hayro_interpret::font::{Glyph, GlyphRun};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps,
    InterpreterCache, InterpreterSettings, SoftMask, TransformExt as _, interpret_page,
};
use hayro::kurbo::{Affine, BezPath, Point, Rect, Vec2};
use unicode_normalization::UnicodeNormalization as _;

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
                // Apart either way: right-to-left text runs leftwards.
                let gap = (r[0] - last[2]).max(last[0] - r[2]);
                if overlap > h * 0.5 && gap < h * 1.5 {
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

const TATWEEL: char = '\u{0640}';

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
            // Arabic kashida only stretches a word: “المتحـدة” is found
            // as “المتحدة”.
            if c == TATWEEL && off > 0 && folded.get(i) != Some(&TATWEEL) {
                continue;
            }
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
        reading_order(dev.out)
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

/// How many of the last glyphs a new one is compared with to find one
/// drawn again on top of itself (fake bold strikes each glyph or each line
/// twice, a little apart).
const OVERDRAW_LOOKBACK: usize = 64;

impl TextDevice {
    /// Whether the same text was just drawn at (nearly) the same place.
    fn drawn_again(&self, text: &str, rect: [f32; 4], tol: f32) -> bool {
        let n = self.out.boxes.len();
        self.out.boxes[n.saturating_sub(OVERDRAW_LOOKBACK)..]
            .iter()
            .rev()
            .any(|b| {
                b.rect.iter().zip(rect).all(|(a, b)| (a - b).abs() <= tol)
                    && self.out.text.get(b.start as usize..b.end as usize) == Some(text)
            })
    }

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
        let rect = [x0 as f32, y0 as f32, x1 as f32, y1 as f32];
        if self.drawn_again(text, rect, (size * 0.15) as f32) {
            return;
        }
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
                    // Space glyphs dropped at the line end lose their boxes.
                    let len = self.out.text.len() as u32;
                    while self.out.boxes.last().is_some_and(|b| b.end > len) {
                        self.out.boxes.pop();
                    }
                    if !self.out.text.ends_with('\n') {
                        self.out.text.push('\n');
                    }
                }
            } else if along > 0.2 && !ends_blank && !starts_blank {
                self.out.text.push(' ');
            }
        }
        let start = self.out.text.len() as u32;
        self.out.text.push_str(text);
        let end = self.out.text.len() as u32;
        if end > start {
            self.out.boxes.push(CharBox { start, end, rect });
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
            // Arabic presentation forms (each letter's joined shape, and
            // ligatures such as lam-alef) are the letters themselves.
            c if is_presentation_form(c) => out.extend(std::iter::once(c).nfkc()),
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

fn is_presentation_form(c: char) -> bool {
    matches!(c, '\u{FB50}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFF}')
}

/// Right-to-left letters: Hebrew, Arabic, Syriac, Thaana, N'Ko and their
/// presentation forms.
fn is_rtl(c: char) -> bool {
    matches!(c,
        '\u{0590}'..='\u{05FF}' | '\u{0600}'..='\u{065F}' | '\u{066A}'..='\u{06EF}'
        | '\u{06FA}'..='\u{08FF}' | '\u{FB1D}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFF}')
        && c != TATWEEL
}

/// The direction class of a piece of a line, for reordering.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dir {
    Rtl,
    /// Left-to-right letters or digits (digits keep their order in
    /// right-to-left text too).
    Ltr,
    Neutral,
}

fn dir_of(s: &str) -> Dir {
    if s.chars().any(|c| is_rtl(c) || c == TATWEEL) {
        Dir::Rtl
    } else if s.chars().any(char::is_alphanumeric) {
        Dir::Ltr
    } else {
        Dir::Neutral
    }
}

/// A piece of a line: one glyph's text with its box, or a space put in
/// between glyphs.
struct Piece<'a> {
    text: &'a str,
    rect: Option<[f32; 4]>,
}

/// Put lines with right-to-left text that the page draws in visual order
/// back in reading order (see the module comment). Lines without such text
/// stay as they are.
fn reading_order(page: PageText) -> PageText {
    if !page.text.chars().any(is_rtl) {
        return page;
    }
    let mut out = PageText {
        text: String::with_capacity(page.text.len()),
        boxes: Vec::with_capacity(page.boxes.len()),
    };
    let mut boxes = page.boxes.iter().peekable();
    let mut line_start = 0;
    for line in page.text.split_inclusive('\n') {
        let line_end = line_start + line.len();
        // The line as pieces: glyph texts and the spaces between them.
        let mut pieces = Vec::new();
        let mut at = line_start;
        while at < line_end {
            while boxes.peek().is_some_and(|b| (b.start as usize) < at) {
                boxes.next();
            }
            match boxes.peek() {
                Some(b) if (b.start as usize) == at && (b.end as usize) <= line_end => {
                    pieces.push(Piece {
                        text: &page.text[at..b.end as usize],
                        rect: Some(b.rect),
                    });
                    at = b.end as usize;
                    boxes.next();
                }
                next => {
                    let to = next.map_or(line_end, |b| (b.start as usize).min(line_end));
                    let to = if to <= at { line_end } else { to };
                    pieces.push(Piece {
                        text: &page.text[at..to],
                        rect: None,
                    });
                    at = to;
                }
            }
        }
        let newline = pieces
            .last()
            .is_some_and(|p| p.rect.is_none() && p.text.ends_with('\n'));
        let tail = if newline { pieces.pop() } else { None };
        if is_visual_rtl(&pieces) {
            reorder(&mut pieces);
        }
        for p in pieces.iter().chain(tail.iter()) {
            let start = out.text.len() as u32;
            out.text.push_str(p.text);
            if let Some(rect) = p.rect {
                out.boxes.push(CharBox {
                    start,
                    end: out.text.len() as u32,
                    rect,
                });
            }
        }
        line_start = line_end;
    }
    out
}

/// Whether the line's right-to-left glyphs come left to right (visual
/// order) rather than right to left (already in reading order).
fn is_visual_rtl(pieces: &[Piece<'_>]) -> bool {
    let xs: Vec<f32> = pieces
        .iter()
        .filter(|p| dir_of(p.text) == Dir::Rtl)
        .filter_map(|p| p.rect.map(|r| (r[0] + r[2]) / 2.0))
        .collect();
    let (mut right, mut left) = (0, 0);
    for w in xs.windows(2) {
        if w[1] > w[0] {
            right += 1;
        } else if w[1] < w[0] {
            left += 1;
        }
    }
    right > left
}

/// Visual (left-to-right) order to reading order: in a line that is mostly
/// right-to-left, the whole line turns round and its left-to-right runs
/// (words, numbers) turn back; otherwise only the right-to-left runs turn.
fn reorder(pieces: &mut [Piece<'_>]) {
    // The line's direction goes by its letters (digits have none).
    let rtl = pieces
        .iter()
        .flat_map(|p| p.text.chars())
        .filter(|&c| is_rtl(c))
        .count();
    let ltr = pieces
        .iter()
        .flat_map(|p| p.text.chars())
        .filter(|&c| c.is_alphabetic() && !is_rtl(c) && c != TATWEEL)
        .count();
    let (turned, keep) = if rtl >= ltr {
        pieces.reverse();
        (Dir::Ltr, Dir::Rtl)
    } else {
        (Dir::Rtl, Dir::Ltr)
    };
    // Each run from a `turned` piece to the last one before a `keep`
    // piece (neutrals at its ends stay where they are) turns round.
    let mut i = 0;
    while i < pieces.len() {
        if dir_of(pieces[i].text) != turned {
            i += 1;
            continue;
        }
        let mut last = i;
        let mut j = i + 1;
        while j < pieces.len() {
            match dir_of(pieces[j].text) {
                d if d == keep => break,
                d if d == turned => last = j,
                _ => {}
            }
            j += 1;
        }
        pieces[i..=last].reverse();
        i = last + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 12 pt glyph `text` at (`x`, `y`) (page space), 6 pt wide.
    fn draw(dev: &mut TextDevice, x: f64, y: f64, text: &str) {
        dev.glyph(Affine::new([0.012, 0.0, 0.0, -0.012, x, y]), 500.0, text);
    }

    /// `glyphs` drawn left to right from `x`, one per 6 pt; `' '` leaves a
    /// gap instead of drawing.
    fn line(dev: &mut TextDevice, x: f64, y: f64, glyphs: &str) {
        for (i, c) in glyphs.chars().enumerate() {
            if c != ' ' {
                draw(dev, x + 6.0 * i as f64, y, &c.to_string());
            }
        }
    }

    fn page(f: impl FnOnce(&mut TextDevice)) -> PageText {
        let mut dev = TextDevice::default();
        f(&mut dev);
        reading_order(dev.out)
    }

    /// Every box covers its own text, in order, without overlaps.
    fn boxes_consistent(p: &PageText) {
        let mut at = 0;
        for b in &p.boxes {
            assert!(b.start >= at && b.end > b.start, "{:?}", p.boxes);
            assert!(p.text.is_char_boundary(b.start as usize));
            at = b.end;
        }
        assert!(at as usize <= p.text.len());
    }

    #[std::prelude::v1::test]
    fn arabic_in_visual_order_reads_right_to_left() {
        // “سلام” drawn as most producers do: its last letter leftmost.
        let p = page(|d| line(d, 72.0, 100.0, "مالس"));
        assert_eq!(p.text, "سلام");
        boxes_consistent(&p);
        // The first letter's box is the rightmost.
        assert!(p.boxes[0].rect[0] > p.boxes[3].rect[0]);
        assert_eq!(find_in(0, &p, "سلام", false, false).len(), 1);
    }

    #[std::prelude::v1::test]
    fn numbers_and_latin_keep_their_order_in_arabic_lines() {
        // “خطة العام 2030” in an Arabic line, shown left to right.
        let p = page(|d| line(d, 72.0, 100.0, "2030 ماعلا ةطخ"));
        assert_eq!(p.text, "خطة العام 2030");
        boxes_consistent(&p);
        let p = page(|d| line(d, 72.0, 100.0, "A/RES/70/1 ماعلا ةطخ"));
        assert_eq!(p.text, "خطة العام A/RES/70/1");
        boxes_consistent(&p);
    }

    #[std::prelude::v1::test]
    fn arabic_words_in_a_latin_line_turn_alone() {
        let p = page(|d| line(d, 72.0, 100.0, "The word مالس means peace"));
        assert_eq!(p.text, "The word سلام means peace");
        boxes_consistent(&p);
    }

    #[std::prelude::v1::test]
    fn arabic_already_in_reading_order_stays() {
        // Drawn right to left: the file has the letters in reading order.
        let p = page(|d| {
            for (i, c) in "سلام".chars().enumerate() {
                draw(d, 200.0 - 6.0 * i as f64, 100.0, &c.to_string());
            }
        });
        assert_eq!(p.text, "سلام");
    }

    #[std::prelude::v1::test]
    fn only_lines_with_right_to_left_text_change() {
        let p = page(|d| {
            line(d, 72.0, 100.0, "Plain line");
            line(d, 72.0, 130.0, "مالس");
            line(d, 72.0, 160.0, "Last line");
        });
        assert_eq!(p.text, "Plain line\nسلام\nLast line");
        boxes_consistent(&p);
    }

    #[std::prelude::v1::test]
    fn presentation_forms_are_plain_letters() {
        // Final heh-goal… : “ﻻ” (lam-alef) and “ﺔ” (final teh marbuta).
        let p = page(|d| {
            draw(d, 72.0, 100.0, &glyph_char('\u{FE94}'));
            draw(d, 78.0, 100.0, &glyph_char('\u{FEFB}'));
        });
        assert_eq!(p.text, "لاة");
        assert_eq!(find_in(0, &p, "لاة", false, false).len(), 1);
    }

    fn glyph_char(c: char) -> String {
        let mut out = String::new();
        if is_presentation_form(c) {
            out.extend(std::iter::once(c).nfkc());
        } else {
            out.push(c);
        }
        out
    }

    #[std::prelude::v1::test]
    fn kashida_does_not_stop_a_search() {
        let p = PageText {
            text: "الأمم المتحـدة".into(),
            boxes: Vec::new(),
        };
        let hits = find_in(0, &p, "المتحدة", false, false);
        assert_eq!(hits.len(), 1);
        assert_eq!(&p.text[hits[0].range.clone()], "المتحـدة");
    }

    #[std::prelude::v1::test]
    fn glyphs_drawn_twice_count_once() {
        // Fake bold: each glyph struck twice, a little apart.
        let p = page(|d| {
            for (i, c) in "第二条".chars().enumerate() {
                let x = 72.0 + 6.0 * i as f64;
                draw(d, x, 100.0, &c.to_string());
                draw(d, x + 0.4, 100.0, &c.to_string());
            }
        });
        assert_eq!(p.text, "第二条");
        // A whole line drawn again, as some producers do for bold.
        let p = page(|d| {
            line(d, 72.0, 100.0, "Bold");
            line(d, 72.3, 100.0, "Bold");
            line(d, 72.0, 130.0, "Next");
        });
        assert_eq!(p.text, "Bold\nNext");
        // The same letter twice in a row, side by side, is two letters.
        let p = page(|d| line(d, 72.0, 100.0, "aa"));
        assert_eq!(p.text, "aa");
    }

    #[std::prelude::v1::test]
    fn space_glyphs_at_a_line_end_leave_no_box_behind() {
        // Word draws the space after the last word of a line.
        let mut dev = TextDevice::default();
        line(&mut dev, 72.0, 100.0, "ab");
        draw(&mut dev, 84.0, 100.0, " ");
        line(&mut dev, 72.0, 130.0, "cd");
        let p = dev.out;
        assert_eq!(p.text, "ab\ncd");
        boxes_consistent(&p);
        for b in &p.boxes {
            assert!(!p.text[b.start as usize..b.end as usize].contains('\n'));
        }
        // The same with right-to-left lines, which are rebuilt.
        let p = page(|d| {
            line(d, 72.0, 100.0, "مالس");
            draw(d, 96.0, 100.0, " ");
            line(d, 72.0, 130.0, "مالس");
        });
        assert_eq!(p.text, "سلام\nسلام");
        assert_eq!(p.boxes.len(), 8);
        boxes_consistent(&p);
    }

    #[std::prelude::v1::test]
    fn selection_rects_join_right_to_left_runs() {
        let p = page(|d| line(d, 72.0, 100.0, "مالس"));
        assert_eq!(p.rects(0..p.text.len()).len(), 1);
    }
}
