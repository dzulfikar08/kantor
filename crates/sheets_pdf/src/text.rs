//! Shaping cell text with parley into glyph runs `docs_pdf` can write
//! (embedded subset fonts, ToUnicode, so the PDF's text can be searched and
//! copied). One-line texts are cached: a sheet repeats the same dates,
//! names and categories over and over.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use docs_layout::parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontStyle, FontWeight, Layout,
    LayoutContext, LineHeight, PositionedLayoutItem, StyleProperty,
};
use docs_layout::{PositionedGlyph, Rgba};
use docs_pdf::TextRun;

/// Points to px (the unit `docs_pdf` runs are in).
pub(crate) const PX: f32 = 96.0 / 72.0;

/// The font of a run of cell text.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FontKey {
    /// CSS-style family list.
    pub family: Arc<str>,
    /// Size in hundredths of a point.
    pub size_centi: u32,
    pub bold: bool,
    pub italic: bool,
}

impl FontKey {
    pub fn new(family: Arc<str>, size_pt: f32, bold: bool, italic: bool) -> Self {
        Self {
            family,
            size_centi: (size_pt.clamp(1.0, 409.0) * 100.0).round() as u32,
            bold,
            italic,
        }
    }

    pub fn size_pt(&self) -> f32 {
        self.size_centi as f32 / 100.0
    }
}

/// One shaped line: runs from the pen at x 0 on the baseline (px), and its
/// metrics in points.
#[derive(Debug, Clone, Default)]
pub struct Line {
    pub runs: Vec<TextRun>,
    pub width: f32,
    pub ascent: f32,
    pub descent: f32,
    /// Line height (the font's ascent + descent + leading), points.
    pub height: f32,
}

/// A wrapped paragraph: lines with their baselines from the top, points.
#[derive(Debug, Clone, Default)]
pub struct Block {
    pub lines: Vec<(Line, f32)>,
    pub height: f32,
}

/// Font database, parley's scratch space and the one-line cache. Build one
/// per export: creating the font context scans the system's fonts.
pub struct Shaper {
    fonts: FontContext,
    layout: LayoutContext<Rgba>,
    cache: HashMap<(FontKey, Box<str>), Arc<Line>>,
    /// Single printable ASCII characters, for [`Shaper::line`]'s fast path.
    chars: HashMap<(FontKey, u8), Arc<Line>>,
}

/// One-line texts kept at most; the cache starts over beyond.
const CACHE_MAX: usize = 50_000;

impl Default for Shaper {
    fn default() -> Self {
        Self::new()
    }
}

impl Shaper {
    pub fn new() -> Self {
        Self {
            fonts: FontContext::new(),
            layout: LayoutContext::new(),
            cache: HashMap::new(),
            chars: HashMap::new(),
        }
    }

    fn build(
        &mut self,
        text: &str,
        font: &FontKey,
        width: Option<f32>,
        justify: bool,
    ) -> Layout<Rgba> {
        let mut b = self.layout.ranged_builder(&mut self.fonts, text, 1.0, true);
        b.push_default(StyleProperty::FontFamily(FontFamily::Source(
            font.family.as_ref().into(),
        )));
        b.push_default(StyleProperty::FontSize(font.size_pt() * PX));
        b.push_default(StyleProperty::LineHeight(LineHeight::MetricsRelative(1.0)));
        b.push_default(StyleProperty::Brush(Rgba::BLACK));
        if font.bold {
            b.push_default(StyleProperty::FontWeight(FontWeight::BOLD));
        }
        if font.italic {
            b.push_default(StyleProperty::FontStyle(FontStyle::Italic));
        }
        let mut layout = b.build(text);
        layout.break_all_lines(width.map(|w| w * PX));
        let align = if justify {
            Alignment::Justify
        } else {
            Alignment::Start
        };
        layout.align(align, AlignmentOptions::default());
        layout
    }

    /// `text` on one line (line breaks are shown as spaces), cached.
    ///
    /// Printable ASCII is put together from its characters, each shaped
    /// once: no kerning or ligatures, which is also how Excel draws cell
    /// text, and far quicker than shaping every number of a big sheet.
    pub fn line(&mut self, text: &str, font: &FontKey) -> Arc<Line> {
        let key = (font.clone(), Box::<str>::from(text));
        if let Some(l) = self.cache.get(&key) {
            return l.clone();
        }
        if text.len() > 1
            && text.bytes().all(|b| (0x20..0x7f).contains(&b))
            && let Some(line) = self.ascii_line(text, font)
        {
            let line = Arc::new(line);
            if self.cache.len() >= CACHE_MAX {
                self.cache.clear();
            }
            self.cache.insert(key, line.clone());
            return line;
        }
        let flat: String = text
            .chars()
            .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
            .collect();
        let layout = self.build(&flat, font, None, false);
        let text: Arc<str> = Arc::from(flat.as_str());
        let line = layout
            .lines()
            .next()
            .map(|l| line_of(&l, &text))
            .unwrap_or_else(|| self.empty_metrics(font));
        let line = Arc::new(line);
        if self.cache.len() >= CACHE_MAX {
            self.cache.clear();
        }
        self.cache.insert(key, line.clone());
        line
    }

    /// [`Self::line`] for printable ASCII, from cached characters. `None`
    /// when a character doesn't shape to one glyph.
    fn ascii_line(&mut self, text: &str, font: &FontKey) -> Option<Line> {
        let shared: Arc<str> = Arc::from(text);
        let mut out = Line::default();
        let mut pen = 0.0f32; // px
        for (i, b) in text.bytes().enumerate() {
            let key = (font.clone(), b);
            let ch = match self.chars.get(&key) {
                Some(c) => c.clone(),
                None => {
                    let s = (b as char).to_string();
                    let layout = self.build(&s, font, None, false);
                    let one: Arc<str> = Arc::from(s.as_str());
                    let l = Arc::new(layout.lines().next().map(|l| line_of(&l, &one))?);
                    self.chars.insert(key, l.clone());
                    l
                }
            };
            let [run] = &ch.runs[..] else { return None };
            let [g] = &run.glyphs[..] else { return None };
            out.ascent = out.ascent.max(ch.ascent);
            out.descent = out.descent.max(ch.descent);
            out.height = out.height.max(ch.height);
            let glyph = PositionedGlyph {
                id: g.id,
                x: pen + g.x - run.origin_x,
                y: g.y,
                advance: g.advance,
                cluster: i..i + 1,
            };
            let same = out.runs.last().is_some_and(|r: &TextRun| {
                r.font.data.id() == run.font.data.id()
                    && r.font.index == run.font.index
                    && r.font_size == run.font_size
                    && r.embolden == run.embolden
                    && r.skew_deg == run.skew_deg
            });
            if same {
                let r = out.runs.last_mut().expect("a run");
                r.glyphs.push(glyph);
                r.advance += run.advance;
            } else {
                out.runs.push(TextRun {
                    origin_x: pen,
                    advance: run.advance,
                    glyphs: vec![glyph],
                    text: shared.clone(),
                    ..run.clone()
                });
            }
            pen += run.advance;
        }
        // Width without trailing spaces, as parley measures it.
        let trailing: f32 = text
            .bytes()
            .rev()
            .take_while(|&b| b == b' ')
            .filter_map(|b| self.chars.get(&(font.clone(), b)))
            .map(|c| c.runs.first().map_or(0.0, |r| r.advance))
            .sum();
        out.width = (pen - trailing).max(0.0) / PX;
        Some(out)
    }

    /// Metrics of an empty line in `font`.
    fn empty_metrics(&mut self, font: &FontKey) -> Line {
        let layout = self.build(" ", font, None, false);
        let mut l = layout
            .lines()
            .next()
            .map(|l| line_of(&l, &Arc::from(" ")))
            .unwrap_or_default();
        l.runs.clear();
        l.width = 0.0;
        l
    }

    /// `text` wrapped to `width` points (explicit line breaks kept).
    pub fn wrapped(&mut self, text: &str, font: &FontKey, width: f32, justify: bool) -> Block {
        let layout = self.build(text, font, Some(width.max(1.0)), justify);
        let shared: Arc<str> = Arc::from(text);
        let mut lines = Vec::new();
        let mut y = 0.0;
        for l in layout.lines() {
            let m = l.metrics();
            let line = line_of(&l, &shared);
            let baseline = m.baseline / PX;
            lines.push((line, baseline));
            y = (m.block_max_coord / PX).max(y);
        }
        Block { lines, height: y }
    }
}

/// Glyph runs of one parley line, from x 0 on its baseline.
fn line_of(line: &docs_layout::parley::Line<'_, Rgba>, text: &Arc<str>) -> Line {
    let m = line.metrics();
    let mut runs = Vec::new();
    let mut run_key = usize::MAX;
    let mut clusters: Vec<Range<usize>> = Vec::new();
    let mut next = 0;
    let base = m.baseline;
    for item in line.items() {
        let PositionedLayoutItem::GlyphRun(gr) = item else {
            continue;
        };
        let run = gr.run();
        if run.index() != run_key {
            run_key = run.index();
            next = 0;
            clusters.clear();
            for c in run.visual_clusters() {
                let n = c.glyphs().count();
                if n == 0
                    && let Some(last) = clusters.last_mut()
                {
                    let r = c.text_range();
                    *last = last.start.min(r.start)..last.end.max(r.end);
                }
                clusters.extend(std::iter::repeat_n(c.text_range(), n));
            }
        }
        let synthesis = run.synthesis();
        let dy = gr.baseline() - base;
        runs.push(TextRun {
            font: run.font().clone(),
            font_size: run.font_size(),
            color: Rgba::BLACK,
            embolden: synthesis.embolden(),
            skew_deg: synthesis.skew(),
            origin_x: gr.offset(),
            baseline: dy,
            advance: gr.advance(),
            underline: None,
            glyphs: gr
                .positioned_glyphs()
                .map(|g| {
                    let cluster = clusters.get(next).cloned().unwrap_or_default();
                    next += 1;
                    PositionedGlyph {
                        id: g.id,
                        x: g.x,
                        y: g.y - base,
                        advance: g.advance,
                        cluster,
                    }
                })
                .collect(),
            text: text.clone(),
        });
    }
    Line {
        runs,
        width: line_width(line) / PX,
        ascent: (m.baseline - m.content_block_min_coord) / PX,
        descent: (m.content_block_max_coord - m.baseline) / PX,
        height: m.line_height / PX,
    }
}

/// Width of a line's glyphs (without trailing spaces).
fn line_width(line: &docs_layout::parley::Line<'_, Rgba>) -> f32 {
    let m = line.metrics();
    (m.advance - m.hanging_advance).max(0.0)
}
