//! Paged rich-text layout for Kantor Docs (no GPUI).
//!
//! Text shaping, bidi, UAX #14 line breaking and justification come from
//! [parley]; this crate adds what a word processor needs on top: fixed-size
//! pages with margins, pagination with widow/orphan control, a
//! renderer-neutral list of positioned glyph runs per page (painted by GPUI
//! with `Window::paint_glyph`, written to PDF by krilla), and caret
//! hit-testing in page coordinates.
//!
//! Units are CSS pixels at 96 dpi, so 1 pt = 4/3 px.
//!
//! Status: M0 spike (`docs/decisions/0001-text-engine.md`). The API is a
//! sketch of the real `docs_layout`, not a stable surface.

use std::ops::Range;
use std::sync::Arc;

use parley::{
    Affinity, Alignment, AlignmentOptions, BreakReason, Cursor, FontContext, FontData, FontFamily,
    FontStyle, FontWeight, Layout, LayoutContext, LineHeight, PositionedLayoutItem, StyleProperty,
};

pub use parley;

/// Points to pixels at 96 dpi.
pub const PT: f32 = 96.0 / 72.0;
/// Millimetres to pixels at 96 dpi.
pub const MM: f32 = 96.0 / 25.4;

/// Straight sRGB colour, the brush type stored in parley layouts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgba(pub [u8; 4]);

impl Rgba {
    pub const BLACK: Self = Self([0, 0, 0, 255]);
}

/// Page size and margins, in px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageSetup {
    pub width: f32,
    pub height: f32,
    pub margin_top: f32,
    pub margin_bottom: f32,
    pub margin_left: f32,
    pub margin_right: f32,
}

impl PageSetup {
    /// A4 portrait with Word's default 2.54 cm margins.
    pub fn a4() -> Self {
        let margin = 25.4 * MM;
        Self {
            width: 210.0 * MM,
            height: 297.0 * MM,
            margin_top: margin,
            margin_bottom: margin,
            margin_left: margin,
            margin_right: margin,
        }
    }

    pub fn content_width(&self) -> f32 {
        self.width - self.margin_left - self.margin_right
    }

    pub fn content_height(&self) -> f32 {
        self.height - self.margin_top - self.margin_bottom
    }
}

/// Character formatting for a byte range of a paragraph.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub range: Range<usize>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// Font size in pt, if different from the paragraph's.
    pub size_pt: Option<f32>,
    pub color: Option<Rgba>,
    /// CSS-style family list, if different from the paragraph's.
    pub family: Option<String>,
}

impl Span {
    pub fn new(range: Range<usize>) -> Self {
        Self {
            range,
            bold: false,
            italic: false,
            underline: false,
            size_pt: None,
            color: None,
            family: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
    Justify,
}

/// One paragraph: plain text plus character spans and paragraph formatting.
#[derive(Clone, Debug, PartialEq)]
pub struct Paragraph {
    pub text: String,
    pub spans: Vec<Span>,
    pub align: Align,
    /// CSS-style family list, e.g. `"Carlito, Noto Sans CJK SC, Noto Sans Arabic"`.
    pub family: String,
    pub size_pt: f32,
    /// Line height as a multiple of the font size.
    pub line_spacing: f32,
    /// Overrides `line_spacing` when set: e.g. `MetricsRelative(1.08)` for
    /// Word's "multiple" (a multiple of the font's own line height), or
    /// `Absolute(px)` for "exactly".
    pub line_height: Option<LineHeight>,
    pub space_before_pt: f32,
    pub space_after_pt: f32,
    /// BCP 47 language, used for shaping and line breaking.
    pub locale: Option<String>,
}

impl Paragraph {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            spans: Vec::new(),
            align: Align::Start,
            family: "Carlito, Liberation Sans, Noto Sans, sans-serif".into(),
            size_pt: 11.0,
            line_spacing: 1.15,
            line_height: None,
            space_before_pt: 0.0,
            space_after_pt: 8.0,
            locale: None,
        }
    }
}

/// Font database plus shaping scratch space. Create once and reuse: building
/// the font context scans the system fonts.
pub struct Engine {
    pub fonts: FontContext,
    layout_cx: LayoutContext<Rgba>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self {
            fonts: FontContext::new(),
            layout_cx: LayoutContext::new(),
        }
    }

    /// Shapes one paragraph and breaks it into lines `width` px wide.
    pub fn layout_paragraph(&mut self, para: &Paragraph, width: f32) -> Layout<Rgba> {
        let text = para.text.as_str();
        let mut b = self
            .layout_cx
            .ranged_builder(&mut self.fonts, text, 1.0, true);
        b.push_default(StyleProperty::FontFamily(FontFamily::Source(
            para.family.as_str().into(),
        )));
        b.push_default(StyleProperty::FontSize(para.size_pt * PT));
        b.push_default(StyleProperty::LineHeight(
            para.line_height
                .unwrap_or(LineHeight::FontSizeRelative(para.line_spacing)),
        ));
        b.push_default(StyleProperty::Brush(Rgba::BLACK));
        if let Some(locale) = para.locale.as_deref().and_then(|l| l.parse().ok()) {
            b.push_default(StyleProperty::Locale(Some(locale)));
        }
        for span in &para.spans {
            let r = span.range.clone();
            if span.bold {
                b.push(StyleProperty::FontWeight(FontWeight::BOLD), r.clone());
            }
            if span.italic {
                b.push(StyleProperty::FontStyle(FontStyle::Italic), r.clone());
            }
            if span.underline {
                b.push(StyleProperty::Underline(true), r.clone());
            }
            if let Some(size) = span.size_pt {
                b.push(StyleProperty::FontSize(size * PT), r.clone());
            }
            if let Some(color) = span.color {
                b.push(StyleProperty::Brush(color), r.clone());
            }
            if let Some(family) = span.family.as_deref() {
                b.push(
                    StyleProperty::FontFamily(FontFamily::Source(family.into())),
                    r,
                );
            }
        }
        let mut layout = b.build(text);
        layout.break_all_lines(Some(width));
        let alignment = match para.align {
            Align::Start => Alignment::Start,
            Align::Center => Alignment::Center,
            Align::End => Alignment::End,
            Align::Justify => Alignment::Justify,
        };
        layout.align(alignment, AlignmentOptions::default());
        layout
    }

    /// Lays out every paragraph and paginates the result.
    pub fn layout_document(&mut self, paras: &[Paragraph], setup: PageSetup) -> PagedDocument {
        let width = setup.content_width();
        let layouts = paras
            .iter()
            .map(|p| Arc::new(self.layout_paragraph(p, width)))
            .collect();
        PagedDocument::paginate(paras, layouts, setup)
    }
}

/// A line of a paragraph placed on a page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedLine {
    pub para: usize,
    /// Line index within the paragraph's layout.
    pub line: usize,
    /// Top of the line box in page coordinates (px from the page's top edge).
    pub top: f32,
    pub height: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub lines: Vec<PlacedLine>,
}

/// A run of glyphs sharing one font, size and colour, in page coordinates.
/// This is the hand-off to renderers: GPUI paints each glyph with
/// `Window::paint_glyph`, krilla writes the run with `Surface::draw_glyphs`.
#[derive(Clone, Debug)]
pub struct GlyphRun {
    pub font: FontData,
    pub font_size: f32,
    pub color: Rgba,
    /// Variation coordinates of the selected instance (variable fonts).
    pub normalized_coords: Vec<i16>,
    /// Synthetic bold / oblique the renderer must apply itself.
    pub embolden: bool,
    pub skew_deg: Option<f32>,
    pub baseline: f32,
    /// Pen x at the start of the run (page coordinates); glyph `x` is this
    /// plus the preceding advances plus the glyph's own offset.
    pub origin_x: f32,
    pub glyphs: Vec<PositionedGlyph>,
    /// Width of the run (for decorations).
    pub advance: f32,
    /// Underline: `(offset below the baseline, thickness)` in px.
    pub underline: Option<(f32, f32)>,
    /// Byte range of the source paragraph text covered by this run.
    pub para: usize,
    pub text_range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PositionedGlyph {
    pub id: u32,
    /// Pen position in page coordinates (px), baseline-relative y already applied.
    pub x: f32,
    pub y: f32,
    pub advance: f32,
    /// Source text of the glyph's cluster (for PDF ToUnicode and copy).
    pub cluster: Range<usize>,
}

/// A caret position resolved to a page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaretRect {
    pub page: usize,
    pub x: f32,
    pub top: f32,
    pub height: f32,
}

/// The laid-out document: paragraph layouts plus their placement on pages.
///
/// Layouts are shared (`Arc`) so an editor can keep a per-paragraph cache and
/// re-paginate after an edit without reshaping unchanged paragraphs.
pub struct PagedDocument {
    pub setup: PageSetup,
    pub layouts: Vec<Arc<Layout<Rgba>>>,
    pub pages: Vec<Page>,
    /// For each paragraph, the y (in its layout's coordinates) that maps to
    /// the top of each of its lines on the page, indexed like `layout.lines()`.
    line_pages: Vec<Vec<(usize, f32)>>,
}

/// Lines a paragraph keeps together at a page break (Word's widow/orphan control).
const MIN_LINES: usize = 2;

impl PagedDocument {
    /// Flows already broken paragraph layouts onto pages.
    pub fn paginate(
        paras: &[Paragraph],
        layouts: Vec<Arc<Layout<Rgba>>>,
        setup: PageSetup,
    ) -> Self {
        let bottom = setup.height - setup.margin_bottom;
        let mut pages = vec![Page::default()];
        let mut line_pages = Vec::with_capacity(layouts.len());
        let mut y = setup.margin_top;

        for (pi, (para, layout)) in paras.iter().zip(&layouts).enumerate() {
            let heights: Vec<f32> = layout.lines().map(|l| l.metrics().line_height).collect();
            let n = heights.len();
            // Space before is dropped at the top of a page, as in Word.
            if y > setup.margin_top {
                y += para.space_before_pt * PT;
            }
            let mut placed = Vec::with_capacity(n);
            let mut i = 0;
            while i < n {
                // How many lines fit on this page from line i on?
                let mut fit = 0;
                let mut h = 0.0;
                while i + fit < n && y + h + heights[i + fit] <= bottom {
                    h += heights[i + fit];
                    fit += 1;
                }
                let rest = n - i - fit;
                let mut take = fit;
                if rest > 0 && n >= MIN_LINES * 2 {
                    // Orphan: don't leave fewer than MIN_LINES at the page bottom.
                    if i == 0 && take < MIN_LINES {
                        take = 0;
                    }
                    // Widow: don't carry fewer than MIN_LINES to the next page.
                    if take > 0 && rest < MIN_LINES {
                        take = take.saturating_sub(MIN_LINES - rest);
                        if i == 0 && take < MIN_LINES {
                            take = 0;
                        }
                    }
                }
                let at_top = y <= setup.margin_top;
                if take == 0 && at_top {
                    // A line taller than the page, or nothing fits: force one line.
                    take = 1;
                }
                let page = pages.len() - 1;
                for (k, line) in layout.lines().enumerate().skip(i).take(take) {
                    let m = line.metrics();
                    pages[page].lines.push(PlacedLine {
                        para: pi,
                        line: k,
                        top: y,
                        height: m.line_height,
                    });
                    placed.push((page, y - m.block_min_coord));
                    y += m.line_height;
                }
                i += take;
                if i < n {
                    pages.push(Page::default());
                    y = setup.margin_top;
                }
            }
            line_pages.push(placed);
            y += para.space_after_pt * PT;
        }

        Self {
            setup,
            layouts,
            pages,
            line_pages,
        }
    }

    pub fn line_count(&self) -> usize {
        self.layouts.iter().map(|l| l.len()).sum()
    }

    /// Positioned glyph runs on one page, in page coordinates.
    pub fn glyph_runs(&self, page: usize) -> Vec<GlyphRun> {
        let mut out = Vec::new();
        let x0 = self.setup.margin_left;
        for pl in &self.pages[page].lines {
            let layout = &self.layouts[pl.para];
            let dy = self.line_pages[pl.para][pl.line].1;
            let line = layout.get(pl.line).expect("placed line exists");
            // Per-glyph cluster ranges of the current run, in visual order;
            // a run's glyph items consume them in sequence.
            let mut run_key = usize::MAX;
            let mut clusters: Vec<Range<usize>> = Vec::new();
            let mut next = 0;
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
                        if n == 0 {
                            // Ligature continuation: extend the previous glyph's text.
                            if let Some(last) = clusters.last_mut() {
                                let r = c.text_range();
                                *last = last.start.min(r.start)..last.end.max(r.end);
                            }
                        }
                        clusters.extend(std::iter::repeat_n(c.text_range(), n));
                    }
                }
                let synthesis = run.synthesis();
                let baseline = gr.baseline() + dy;
                let size = run.font_size();
                // parley 0.12 has no public run metrics; use typical values.
                let underline = gr.style().underline.as_ref().map(|d| {
                    (
                        d.offset.unwrap_or(size * 0.1),
                        d.size.unwrap_or((size / 14.0).max(1.0)),
                    )
                });
                out.push(GlyphRun {
                    font: run.font().clone(),
                    font_size: run.font_size(),
                    color: gr.style().brush,
                    normalized_coords: run
                        .normalized_coords()
                        .iter()
                        .map(|c| c.to_bits())
                        .collect(),
                    embolden: synthesis.embolden(),
                    skew_deg: synthesis.skew(),
                    baseline,
                    origin_x: x0 + gr.offset(),
                    advance: gr.advance(),
                    underline,
                    glyphs: gr
                        .positioned_glyphs()
                        .map(|g| {
                            let cluster = clusters.get(next).cloned().unwrap_or_default();
                            next += 1;
                            PositionedGlyph {
                                id: g.id,
                                x: x0 + g.x,
                                y: dy + g.y,
                                advance: g.advance,
                                cluster,
                            }
                        })
                        .collect(),
                    para: pl.para,
                    text_range: run.text_range(),
                });
            }
        }
        out
    }

    /// Maps a point on a page to `(paragraph, byte index)`.
    pub fn hit_test(&self, page: usize, x: f32, y: f32) -> Option<(usize, usize)> {
        let lines = &self.pages.get(page)?.lines;
        let i = lines
            .iter()
            .position(|l| y < l.top + l.height)
            .or(lines.len().checked_sub(1))?;
        self.hit_placed(page, i, x)
    }

    /// Hit-test at `x` on line `i` of `page` (a click in the margin hits the
    /// nearest end of that line).
    fn hit_placed(&self, page: usize, i: usize, x: f32) -> Option<(usize, usize)> {
        let pl = *self.pages.get(page)?.lines.get(i)?;
        let dy = self.line_pages[pl.para][pl.line].1;
        let layout = &self.layouts[pl.para];
        let m = *layout.get(pl.line)?.metrics();
        let ly = ((m.block_min_coord + m.block_max_coord) / 2.0).max(pl.top - dy);
        let cursor = Cursor::from_point(layout, x - self.setup.margin_left, ly);
        Some((pl.para, cursor.index()))
    }

    /// The layout line (index into `layouts[para].lines()`) showing `index`.
    fn layout_line_of(&self, para: usize, index: usize) -> Option<usize> {
        let layout = self.layouts.get(para)?;
        let cursor = Cursor::from_byte_index(layout, index, Affinity::Downstream);
        let r = cursor.geometry(layout, 1.0);
        let mid = ((r.y0 + r.y1) / 2.0) as f32;
        Some(
            layout
                .lines()
                .position(|l| mid < l.metrics().block_max_coord)
                .unwrap_or(layout.len().saturating_sub(1)),
        )
    }

    /// Caret rectangle for a byte index of a paragraph.
    pub fn caret(&self, para: usize, index: usize) -> Option<CaretRect> {
        let layout = self.layouts.get(para)?;
        let cursor = Cursor::from_byte_index(layout, index, Affinity::Downstream);
        let r = cursor.geometry(layout, 1.0);
        let line = self.layout_line_of(para, index)?;
        let (page, dy) = *self.line_pages[para].get(line)?;
        Some(CaretRect {
            page,
            x: self.setup.margin_left + r.x0 as f32,
            top: r.y0 as f32 + dy,
            height: (r.y1 - r.y0) as f32,
        })
    }

    /// `(page, index into that page's lines)` of the line showing `index` of `para`.
    pub fn placed_line(&self, para: usize, index: usize) -> Option<(usize, usize)> {
        let k = self.layout_line_of(para, index)?;
        let (page, _) = *self.line_pages.get(para)?.get(k)?;
        let i = self.pages[page]
            .lines
            .iter()
            .position(|l| l.para == para && l.line == k)?;
        Some((page, i))
    }

    /// The position on the line above (`down == false`) or below the caret
    /// at `para`/`index`, nearest to `x` (page coordinates). Crosses pages;
    /// `None` at the first / last line of the document.
    pub fn vertical(
        &self,
        para: usize,
        index: usize,
        x: f32,
        down: bool,
    ) -> Option<(usize, usize)> {
        let (mut page, mut i) = self.placed_line(para, index)?;
        if down {
            if i + 1 < self.pages[page].lines.len() {
                i += 1;
            } else {
                loop {
                    page += 1;
                    if !self.pages.get(page)?.lines.is_empty() {
                        i = 0;
                        break;
                    }
                }
            }
        } else if i > 0 {
            i -= 1;
        } else {
            loop {
                page = page.checked_sub(1)?;
                if let Some(last) = self.pages[page].lines.len().checked_sub(1) {
                    i = last;
                    break;
                }
            }
        }
        self.hit_placed(page, i, x)
    }

    /// Byte range of the visual line showing `index` of `para`, and whether
    /// that line ends softly (wrapped, so its range includes the trailing
    /// space the line broke at).
    pub fn line_range(&self, para: usize, index: usize) -> Option<(Range<usize>, bool)> {
        let k = self.layout_line_of(para, index)?;
        let line = self.layouts[para].get(k)?;
        let soft = k + 1 < self.layouts[para].len() && line.break_reason() != BreakReason::Explicit;
        Some((line.text_range(), soft))
    }

    /// Selection highlight rectangles `(page, [x, y, w, h])` from
    /// `start` to `end` (`(paragraph, byte index)`, in order). A selection
    /// that continues past a paragraph's end gets a small box there, like
    /// Word's selected pilcrow.
    pub fn selection_rects(
        &self,
        start: (usize, usize),
        end: (usize, usize),
    ) -> Vec<(usize, [f32; 4])> {
        let mut out = Vec::new();
        let x0 = self.setup.margin_left;
        for para in start.0..=end.0.min(self.layouts.len().saturating_sub(1)) {
            let layout = &self.layouts[para];
            let len = layout.lines().last().map_or(0, |l| l.text_range().end);
            let a = if para == start.0 { start.1 } else { 0 };
            let b = if para == end.0 { end.1 } else { len };
            let sel = parley::Selection::new(
                Cursor::from_byte_index(layout, a, Affinity::Downstream),
                Cursor::from_byte_index(layout, b, Affinity::Downstream),
            );
            for (bb, line) in sel.geometry(layout) {
                if let Some(&(page, dy)) = self.line_pages[para].get(line) {
                    out.push((
                        page,
                        [
                            x0 + bb.x0 as f32,
                            bb.y0 as f32 + dy,
                            (bb.x1 - bb.x0) as f32,
                            (bb.y1 - bb.y0) as f32,
                        ],
                    ));
                }
            }
            if para != end.0
                && let Some(c) = self.caret(para, len)
            {
                out.push((c.page, [c.x, c.top, c.height * 0.3, c.height]));
            }
        }
        out
    }

    /// Height of every page stacked with `gap` px between them.
    pub fn total_height(&self, gap: f32) -> f32 {
        let n = self.pages.len() as f32;
        n * self.setup.height + (n - 1.0).max(0.0) * gap
    }
}

/// Resident set size and its peak, in KiB, from `/proc/self/status` (Linux).
pub fn rss_kib() -> Option<(u64, u64)> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let field = |name: &str| {
        status
            .lines()
            .find(|l| l.starts_with(name))?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()
    };
    Some((field("VmRSS:")?, field("VmHWM:")?))
}

/// Sample text for tests and benchmarks: Indonesian prose with mixed
/// formatting, plus CJK and Arabic paragraphs.
pub mod sample {
    use super::{Align, Paragraph, Rgba, Span};

    pub const INDONESIAN: &str = "Laporan ini merangkum hasil pemeliharaan mesin selama kuartal ketiga. \
        Secara keseluruhan, ketersediaan peralatan meningkat menjadi 94,7 persen, sementara biaya \
        perbaikan darurat turun sebesar dua belas persen dibandingkan kuartal sebelumnya. \
        Pengecekan berkala yang dijadwalkan ulang berdasarkan data getaran terbukti mengurangi \
        kerusakan bantalan, dan tim teknisi menyelesaikan pelatihan keselamatan kerja sesuai \
        rencana. Rekomendasi untuk kuartal berikutnya tercantum pada bagian akhir dokumen.";

    pub const CJK: &str = "本报告总结了第三季度的设备维护结果。设备可用率提高到百分之九十四点七，\
        紧急维修费用比上一季度下降了百分之十二。根据振动数据重新安排的定期检查减少了轴承故障。\
        本報告書は第3四半期の設備保全の結果をまとめたものです。";

    pub const ARABIC: &str = "يلخص هذا التقرير نتائج صيانة المعدات خلال الربع الثالث. ارتفع توافر \
        المعدات إلى 94.7 بالمائة، بينما انخفضت تكاليف الإصلاح الطارئ بنسبة اثني عشر بالمائة.";

    /// Font stack with Indonesian/Latin first and CJK/Arabic fallbacks.
    pub const FAMILY: &str =
        "Carlito, Liberation Sans, Noto Sans, Noto Sans CJK SC, Noto Sans Arabic, sans-serif";

    /// A heading plus three body paragraphs (one justified with mixed runs,
    /// one CJK, one Arabic). Roughly a quarter of an A4 page.
    pub fn section(n: usize) -> Vec<Paragraph> {
        let mut heading = Paragraph::new(format!("{n}. Ringkasan pemeliharaan"));
        heading.family = FAMILY.into();
        heading.size_pt = 16.0;
        heading.space_before_pt = 12.0;
        heading.spans.push(Span {
            bold: true,
            color: Some(Rgba([0x1f, 0x4e, 0x79, 255])),
            ..Span::new(0..heading.text.len())
        });

        let mut body = Paragraph::new(INDONESIAN);
        body.family = FAMILY.into();
        body.align = Align::Justify;
        body.locale = Some("id".into());
        let text = INDONESIAN;
        let find = |s: &str| {
            let i = text.find(s).expect("sample substring");
            i..i + s.len()
        };
        body.spans.push(Span {
            bold: true,
            ..Span::new(find("94,7 persen"))
        });
        body.spans.push(Span {
            italic: true,
            color: Some(Rgba([0xc0, 0x39, 0x2b, 255])),
            ..Span::new(find("dua belas persen"))
        });
        body.spans.push(Span {
            family: Some("Liberation Serif, serif".into()),
            size_pt: Some(13.0),
            underline: true,
            ..Span::new(find("data getaran"))
        });

        let mut cjk = Paragraph::new(CJK);
        cjk.family = FAMILY.into();
        cjk.locale = Some("zh".into());

        let mut arabic = Paragraph::new(ARABIC);
        arabic.family = FAMILY.into();
        arabic.locale = Some("ar".into());

        vec![heading, body, cjk, arabic]
    }

    pub fn document(sections: usize) -> Vec<Paragraph> {
        (1..=sections).flat_map(section).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a4_at_96_dpi() {
        let a4 = PageSetup::a4();
        assert!((a4.width - 793.7).abs() < 0.1);
        assert!((a4.height - 1122.5).abs() < 0.1);
        assert!((a4.content_width() - 601.7).abs() < 0.1);
    }

    #[test]
    fn paginates_and_keeps_lines_inside_margins() {
        let mut engine = Engine::new();
        let setup = PageSetup::a4();
        let doc = engine.layout_document(&sample::document(24), setup);
        assert!(doc.pages.len() >= 3, "pages: {}", doc.pages.len());
        let placed: usize = doc.pages.iter().map(|p| p.lines.len()).sum();
        assert_eq!(placed, doc.line_count(), "every line placed exactly once");
        for page in &doc.pages {
            for l in &page.lines {
                assert!(l.top >= setup.margin_top - 0.01);
                assert!(l.top + l.height <= setup.height - setup.margin_bottom + 0.01);
            }
        }
    }

    #[test]
    fn no_orphans_or_widows() {
        let mut engine = Engine::new();
        let doc = engine.layout_document(&sample::document(24), PageSetup::a4());
        for (pi, layout) in doc.layouts.iter().enumerate() {
            let n = layout.len();
            if n < MIN_LINES * 2 {
                continue;
            }
            let mut per_page = std::collections::BTreeMap::<usize, usize>::new();
            for (page, _) in &doc.line_pages[pi] {
                *per_page.entry(*page).or_default() += 1;
            }
            for count in per_page.values() {
                assert!(*count >= MIN_LINES, "paragraph {pi} split {per_page:?}");
            }
        }
    }

    #[test]
    fn glyph_runs_carry_font_data() {
        let mut engine = Engine::new();
        let paragraphs = sample::document(2);
        let doc = engine.layout_document(&paragraphs, PageSetup::a4());
        let runs = doc.glyph_runs(0);
        assert!(!runs.is_empty());
        for run in &runs {
            assert!(!run.font.data.as_ref().is_empty());
            // Only Latin text is guaranteed a font everywhere: CI runners have
            // no CJK/Arabic fonts, so those runs fall back to .notdef there.
            let text = &paragraphs[run.para].text[run.text_range.clone()];
            if text
                .chars()
                .all(|c| (c as u32) < 0x0250 || c.is_whitespace())
            {
                assert!(
                    run.glyphs.iter().all(|g| g.id != 0),
                    "no .notdef in {run:?}"
                );
            }
        }
    }

    #[test]
    fn empty_paragraph_has_a_line_and_caret() {
        let mut engine = Engine::new();
        let paras = vec![
            Paragraph::new("one"),
            Paragraph::new(""),
            Paragraph::new("three"),
        ];
        let doc = engine.layout_document(&paras, PageSetup::a4());
        assert_eq!(
            doc.layouts[1].len(),
            1,
            "an empty paragraph still takes a line"
        );
        let c = doc.caret(1, 0).unwrap();
        assert!(c.height > 5.0);
        assert_eq!(doc.hit_test(0, c.x + 30.0, c.top + 1.0), Some((1, 0)));
        // Down from "one" lands on the empty paragraph, then on "three".
        assert_eq!(doc.vertical(0, 1, c.x, true), Some((1, 0)));
        assert_eq!(doc.vertical(1, 0, c.x, true), Some((2, 0)));
        assert_eq!(doc.vertical(2, 0, c.x, true), None);
        assert_eq!(doc.vertical(0, 0, c.x, false), None);
    }

    #[test]
    fn vertical_moves_cross_pages_and_selection_rects() {
        let mut engine = Engine::new();
        let doc = engine.layout_document(&sample::document(24), PageSetup::a4());
        let last = doc.pages[0].lines.last().copied().unwrap();
        let first_next = doc.pages[1].lines[0];
        let layout = &doc.layouts[last.para];
        let idx = layout.get(last.line).unwrap().text_range().start;
        let x = PageSetup::a4().margin_left + 1.0;
        let (p, _) = doc.vertical(last.para, idx, x, true).unwrap();
        assert_eq!(p, first_next.para);
        let (range, soft) = doc.line_range(1, 0).unwrap();
        assert_eq!(range.start, 0);
        assert!(soft, "the justified body wraps");
        let rects = doc.selection_rects((1, 0), (2, 3));
        assert!(rects.len() >= 3);
        assert!(rects.iter().all(|(_, r)| r[2] >= 0.0 && r[3] > 0.0));
    }

    #[test]
    fn caret_round_trips_through_hit_test() {
        let mut engine = Engine::new();
        let doc = engine.layout_document(&sample::document(8), PageSetup::a4());
        let body = 1; // first Indonesian paragraph
        let index = sample::INDONESIAN.find("ketersediaan").unwrap();
        let caret = doc.caret(body, index).unwrap();
        let hit = doc
            .hit_test(caret.page, caret.x + 0.5, caret.top + caret.height / 2.0)
            .unwrap();
        assert_eq!(hit, (body, index));
    }
}
