//! Laying out a note: one box per buffer line, shaped with GPUI's text
//! system.
//!
//! A line is drawn either *formatted* (its markup bytes left out of the
//! shaped text, list markers and quote bars drawn as decorations before
//! it) or *revealed* (every byte shaped, markup muted), see
//! [`notes_engine::parse`]. Either way the shaped text is a projection of
//! the line's bytes, and a [`DisplayMap`] turns offsets in it back into
//! buffer offsets, so carets, clicks and selections always land on bytes
//! of the file.
//!
//! Lines are cached by a hash of everything that shapes them; a keystroke
//! reshapes the edited line (and the lines whose reveal state changed).
//! A table that isn't revealed is laid out as one grid on its first line;
//! its other lines take no height.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::*;
use notes_engine::Buffer;
use notes_engine::parse::{Align, LineInfo, LineKind, ListMarker, Parsed, SpanKind};

/// Colours and fonts the layout uses.
#[derive(Clone, Debug)]
pub struct Look {
    pub font: SharedString,
    pub mono: SharedString,
    /// Body text size (zoom applied).
    pub size: Pixels,
    pub fg: Hsla,
    /// Markup on a revealed line, list numbers, quote text.
    pub muted: Hsla,
    pub dim: Hsla,
    pub accent: Hsla,
    pub code_bg: Hsla,
    pub border: Hsla,
}

impl Look {
    fn hash_into(&self, h: &mut impl Hasher) {
        self.font.hash(h);
        self.mono.hash(h);
        f32::from(self.size).to_bits().hash(h);
        for c in [
            self.fg,
            self.muted,
            self.dim,
            self.accent,
            self.code_bg,
            self.border,
        ] {
            for v in [c.h, c.s, c.l, c.a] {
                v.to_bits().hash(h);
            }
        }
    }

    pub fn line_height(&self) -> Pixels {
        self.size * LINE_HEIGHT
    }

    pub fn list_slot(&self) -> Pixels {
        self.size * 1.6
    }

    pub fn quote_slot(&self) -> Pixels {
        self.size * 1.1
    }
}

/// Line height as a multiple of the font size.
pub const LINE_HEIGHT: f32 = 1.6;
/// Heading font sizes as multiples of the body size (H1–H6).
pub const HEADING_SCALE: [f32; 6] = [1.8, 1.5, 1.3, 1.15, 1.05, 1.0];

/// What the view lays out in: the formatted note, all source, or reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Markup hidden except where the caret is.
    Live,
    /// Every line as typed, in one size.
    Source,
    /// Markup hidden everywhere, not editable.
    Reading,
}

/// Display offsets ↔ line offsets for one shaped text. Offsets are
/// relative to the start of the line the text is on, so a line's layout
/// stays valid when text before it changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayMap {
    /// `(display start, source range)`, in order.
    segs: Vec<(usize, Range<usize>)>,
    len: usize,
    /// Where an empty text sits in the buffer.
    at: usize,
}

impl DisplayMap {
    fn new(segs: Vec<Range<usize>>, at: usize) -> Self {
        let mut out = Vec::with_capacity(segs.len());
        let mut d = 0;
        for r in segs {
            let n = r.len();
            out.push((d, r));
            d += n;
        }
        Self {
            segs: out,
            len: d,
            at,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The display offset of buffer offset `src` (markup before a segment
    /// maps to the segment's start).
    pub fn to_display(&self, src: usize) -> usize {
        for (d, r) in &self.segs {
            if src < r.start {
                return *d;
            }
            if src <= r.end {
                return d + (src - r.start);
            }
        }
        self.len
    }

    /// The buffer offset of display offset `disp`.
    pub fn to_source(&self, disp: usize) -> usize {
        for (d, r) in &self.segs {
            if disp <= d + r.len() {
                return r.start + disp.saturating_sub(*d);
            }
        }
        self.segs.last().map_or(self.at, |(_, r)| r.end)
    }

    /// Buffer range the display covers.
    pub fn source_range(&self) -> Range<usize> {
        match (self.segs.first(), self.segs.last()) {
            (Some((_, a)), Some((_, b))) => a.start..b.end,
            _ => self.at..self.at,
        }
    }
}

/// Shaped, wrapped text with its map back to the buffer.
pub struct TextBox {
    /// Left edge of the text, relative to the line's left.
    pub x: Pixels,
    pub line_height: Pixels,
    pub wrapped: Option<WrappedLine>,
    pub map: DisplayMap,
    /// Display offsets where each visual row starts.
    pub row_starts: Vec<usize>,
    pub display: SharedString,
}

impl TextBox {
    pub fn rows(&self) -> usize {
        self.row_starts.len().max(1)
    }

    pub fn height(&self) -> Pixels {
        self.line_height * self.rows() as f32
    }

    fn row_of(&self, disp: usize) -> usize {
        match self.row_starts.binary_search(&disp) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
    }

    fn x_of(&self, disp: usize) -> Pixels {
        match &self.wrapped {
            Some(w) => w.unwrapped_layout.x_for_index(disp),
            None => px(0.),
        }
    }

    /// Caret position for display offset `disp`, relative to the box's
    /// top-left (row top).
    pub fn caret(&self, disp: usize) -> Point<Pixels> {
        let disp = disp.min(self.map.len());
        let row = self.row_of(disp);
        let start = self.row_starts.get(row).copied().unwrap_or(0);
        point(
            self.x + self.x_of(disp) - self.x_of(start),
            self.line_height * row as f32,
        )
    }

    /// The display offset nearest to a point relative to the box's top-left.
    pub fn hit(&self, p: Point<Pixels>) -> usize {
        let Some(w) = &self.wrapped else {
            return 0;
        };
        let rows = self.rows();
        let row = ((p.y / self.line_height).floor().max(0.) as usize).min(rows - 1);
        let start = self.row_starts.get(row).copied().unwrap_or(0);
        let end = self
            .row_starts
            .get(row + 1)
            .copied()
            .unwrap_or(self.map.len());
        let x = (p.x - self.x).max(px(0.)) + self.x_of(start);
        let mut i = w.unwrapped_layout.closest_index_for_x(x).clamp(start, end);
        if row + 1 < rows && i >= end {
            // The end of a wrapped row is the next row's start: stay on
            // this row, before its last character.
            i = prev_boundary(&self.display, end).max(start);
        }
        i
    }

    /// Rectangles covering display range `r`, relative to the box (a
    /// sliver for an empty text).
    pub fn rects(&self, r: Range<usize>) -> Vec<Bounds<Pixels>> {
        if self.map.is_empty() {
            return vec![Bounds::new(
                point(self.x, px(0.)),
                size(px(6.), self.line_height),
            )];
        }
        let mut out = Vec::new();
        let rows = self.rows();
        for row in 0..rows {
            let start = self.row_starts.get(row).copied().unwrap_or(0);
            let end = self
                .row_starts
                .get(row + 1)
                .copied()
                .unwrap_or(self.map.len());
            let (a, b) = (r.start.max(start), r.end.min(end));
            if a >= b {
                continue;
            }
            let x0 = self.x + self.x_of(a) - self.x_of(start);
            let x1 = self.x + self.x_of(b) - self.x_of(start);
            let top = self.line_height * row as f32;
            out.push(Bounds::from_corners(
                point(x0, top),
                point(x1.max(x0 + px(4.)), top + self.line_height),
            ));
        }
        out
    }
}

fn prev_boundary(s: &str, i: usize) -> usize {
    let mut j = i.min(s.len()).saturating_sub(1);
    while j > 0 && !s.is_char_boundary(j) {
        j -= 1;
    }
    j
}

/// What a line's left side draws when it is formatted.
#[derive(Clone, Debug, Default)]
pub struct Deco {
    /// Quote bars (one per level).
    pub quote: u8,
    /// List marker and the x its slot starts at.
    pub marker: Option<(MarkerDeco, Pixels)>,
}

#[derive(Clone, Debug)]
pub enum MarkerDeco {
    Bullet { depth: u8 },
    Number(SharedString),
    Task { checked: bool },
}

pub struct TableCell {
    pub text: TextBox,
    /// The cell's line, counted from the table's first line.
    pub dl: usize,
    /// Relative to the table's left / the row's top.
    pub x: Pixels,
    pub width: Pixels,
    pub align: Align,
}

pub struct TableRowBox {
    pub top: Pixels,
    pub height: Pixels,
    pub header: bool,
    pub cells: Vec<TableCell>,
}

pub struct TableBox {
    pub x: Pixels,
    pub width: Pixels,
    pub cols: Vec<Pixels>,
    pub rows: Vec<TableRowBox>,
}

pub enum Body {
    /// Takes no space (a table's other lines, a setext underline).
    Hidden,
    Text {
        text: TextBox,
        deco: Deco,
        /// Code-block background behind the whole line.
        code: bool,
        /// A picture shown under the text (an image line being edited).
        image: Option<ImageBox>,
    },
    /// A formatted fenced-code line: the opening fence shows the language.
    Fence {
        label: Option<TextBox>,
    },
    Rule,
    Table(TableBox),
    Image(ImageBox),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageBox {
    pub path: PathBuf,
    pub size: Size<Pixels>,
}

/// One buffer line's box (`Laid::lines[i]` is line `i`).
pub struct LineBox {
    pub pad_top: Pixels,
    pub height: Pixels,
    pub body: Body,
}

/// The laid-out note.
#[derive(Default)]
pub struct Laid {
    pub lines: Vec<Rc<LineBox>>,
    /// Top of each line box; one more entry: the total height.
    pub tops: Vec<Pixels>,
    pub width: Pixels,
}

impl Laid {
    pub fn height(&self) -> Pixels {
        self.tops.last().copied().unwrap_or_default()
    }

    /// The line box at `y` (document space).
    pub fn line_at_y(&self, y: Pixels) -> usize {
        if self.lines.is_empty() {
            return 0;
        }
        let i = match self.tops.binary_search(&y) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        // Skip boxes with no height (hidden lines).
        let mut i = i.min(self.lines.len() - 1);
        while i > 0 && self.lines[i].height <= px(0.) {
            i -= 1;
        }
        i
    }

    /// Lines with any part in `y0..y1`.
    pub fn visible(&self, y0: Pixels, y1: Pixels) -> Range<usize> {
        if self.lines.is_empty() {
            return 0..0;
        }
        let a = self.line_at_y(y0);
        let mut b = a;
        while b < self.lines.len() && self.tops[b] < y1 {
            b += 1;
        }
        a..b
    }
}

/// Image sizes, read from file headers (`None`: not a readable picture).
pub type ImageSizes = HashMap<PathBuf, Option<(u32, u32)>>;

/// Everything [`layout`] reads.
pub struct Input<'a> {
    pub buf: &'a Buffer,
    pub parsed: &'a Parsed,
    pub look: &'a Look,
    pub mode: Mode,
    /// Revealed lines (`Live` mode).
    pub revealed: &'a dyn Fn(usize) -> bool,
    pub width: Pixels,
    /// The note's folder, for relative picture paths.
    pub dir: Option<&'a Path>,
    pub image_sizes: &'a mut ImageSizes,
}

/// Lay out every line, reusing `cache` (keyed by what shapes each line).
pub fn layout(
    ts: &WindowTextSystem,
    input: Input<'_>,
    cache: &mut HashMap<u64, Rc<LineBox>>,
) -> Laid {
    let Input {
        buf,
        parsed,
        look,
        mode,
        revealed,
        width,
        dir,
        image_sizes,
    } = input;
    let mut next_cache = HashMap::with_capacity(cache.len());
    let n = buf.line_count();
    let mut lines = Vec::with_capacity(n);
    let mut tops = Vec::with_capacity(n + 1);
    let mut y = px(0.);
    let mut l = 0;
    while l < n {
        let info = &parsed.lines[l];
        let is_revealed = match mode {
            Mode::Source => true,
            Mode::Reading => false,
            Mode::Live => revealed(l),
        };
        // A formatted table: one box on its first line, the rest hidden.
        if !is_revealed
            && let LineKind::TableRow { table, row: 0 } = info.kind
            && let Some(t) = parsed.tables.get(table)
            && t.lines.start == l
        {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            "table".hash(&mut h);
            look.hash_into(&mut h);
            f32::from(width).to_bits().hash(&mut h);
            for tl in t.lines.clone() {
                hash_line(&mut h, buf, parsed, tl);
            }
            let key = h.finish();
            let b = match cache.get(&key) {
                Some(b) => b.clone(),
                None => Rc::new(table_box(ts, buf, parsed, look, table, width)),
            };
            next_cache.insert(key, b.clone());
            tops.push(y);
            y += b.height;
            lines.push(b);
            for _ in t.lines.start + 1..t.lines.end {
                tops.push(y);
                lines.push(Rc::new(LineBox {
                    pad_top: px(0.),
                    height: px(0.),
                    body: Body::Hidden,
                }));
            }
            l = t.lines.end;
            continue;
        }
        // Pictures on a line of their own.
        let image = if mode == Mode::Source {
            None
        } else {
            picture_of(buf, parsed, info, l, dir, image_sizes, width, look)
        };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        look.hash_into(&mut h);
        (mode, is_revealed).hash(&mut h);
        f32::from(width).to_bits().hash(&mut h);
        hash_line(&mut h, buf, parsed, l);
        if let Some(img) = &image {
            img.path.hash(&mut h);
            f32::from(img.size.width).to_bits().hash(&mut h);
            f32::from(img.size.height).to_bits().hash(&mut h);
        }
        let key = h.finish();
        let b = match cache.get(&key) {
            Some(b) => b.clone(),
            None => Rc::new(line_box(
                ts,
                buf,
                parsed,
                look,
                mode,
                l,
                is_revealed,
                width,
                image,
            )),
        };
        next_cache.insert(key, b.clone());
        tops.push(y);
        y += b.height;
        lines.push(b);
        l += 1;
    }
    tops.push(y);
    *cache = next_cache;
    Laid { lines, tops, width }
}

/// Hash what shapes line `l`, with offsets relative to the line start.
fn hash_line(h: &mut impl Hasher, buf: &Buffer, parsed: &Parsed, l: usize) {
    let start = buf.line_start(l);
    let rel = |o: usize| o.wrapping_sub(start);
    buf.line(l).hash(h);
    let info = &parsed.lines[l];
    match &info.kind {
        LineKind::Blank => 0u8.hash(h),
        LineKind::Raw => 1u8.hash(h),
        LineKind::Paragraph => 2u8.hash(h),
        LineKind::Heading(n) => (3u8, *n).hash(h),
        LineKind::SetextUnderline(n) => (4u8, *n).hash(h),
        LineKind::CodeFence { open, .. } => (5u8, *open).hash(h),
        LineKind::Code { .. } => 6u8.hash(h),
        LineKind::TableRow { row, .. } => (7u8, *row == 0).hash(h),
        LineKind::TableDelimiter { .. } => 8u8.hash(h),
        LineKind::Rule => 9u8.hash(h),
        LineKind::Html => 10u8.hash(h),
        LineKind::FrontMatter { delimiter } => (11u8, *delimiter).hash(h),
    }
    if let LineKind::CodeFence { block, .. } = info.kind
        && let Some(b) = parsed.code_blocks.get(block)
    {
        b.lang.hash(h);
    }
    if let LineKind::TableRow { table, row } = info.kind
        && let Some(r) = parsed.tables.get(table).and_then(|t| t.rows.get(row))
    {
        for c in &r.cells {
            (rel(c.start), rel(c.end)).hash(h);
        }
    }
    (rel(info.content_start), info.quote, info.list_depth).hash(h);
    match &info.marker {
        ListMarker::None => 0u8.hash(h),
        ListMarker::Bullet => 1u8.hash(h),
        ListMarker::Ordered(r) => (2u8, rel(r.start), rel(r.end)).hash(h),
        ListMarker::Task { checked, .. } => (3u8, *checked).hash(h),
    }
    for v in &info.visible {
        (rel(v.start), rel(v.end)).hash(h);
    }
    for s in &info.spans {
        let k: u8 = match s.kind {
            SpanKind::Strong => 0,
            SpanKind::Emphasis => 1,
            SpanKind::Strike => 2,
            SpanKind::Code => 3,
            SpanKind::Link(_) => 4,
            SpanKind::Image(_) => 5,
            SpanKind::Html => 6,
            SpanKind::Footnote => 7,
        };
        (k, rel(s.range.start), rel(s.range.end)).hash(h);
    }
}

/// How a line's text looks before inline styles.
#[derive(Clone, Copy)]
struct Base {
    size: Pixels,
    line_height: Pixels,
    bold: bool,
    mono: bool,
    color: Hsla,
    strike: bool,
}

fn base_for(look: &Look, mode: Mode, info: &LineInfo) -> Base {
    let body = Base {
        size: look.size,
        line_height: look.line_height(),
        bold: false,
        mono: false,
        color: look.fg,
        strike: false,
    };
    let code = Base {
        size: look.size * 0.9,
        line_height: look.line_height(),
        mono: true,
        ..body
    };
    if mode == Mode::Source {
        return body;
    }
    match &info.kind {
        LineKind::Heading(n) | LineKind::SetextUnderline(n) => {
            let s = look.size * HEADING_SCALE[(*n as usize).clamp(1, 6) - 1];
            Base {
                size: s,
                line_height: s * 1.35,
                bold: true,
                color: if *n >= 6 { look.muted } else { look.fg },
                ..body
            }
        }
        LineKind::Code { .. } | LineKind::CodeFence { .. } => code,
        LineKind::FrontMatter { .. } | LineKind::Html => Base {
            color: look.muted,
            ..code
        },
        _ => match info.marker {
            ListMarker::Task { checked: true, .. } => Base {
                color: look.muted,
                strike: true,
                ..body
            },
            _ => body,
        },
    }
}

/// Space above a heading (more for bigger ones).
fn pad_top(look: &Look, mode: Mode, info: &LineInfo) -> Pixels {
    match (&info.kind, mode) {
        (_, Mode::Source) => px(0.),
        (LineKind::Heading(n), _) => look.size * (0.9 - 0.12 * f32::from((*n).min(6))),
        _ => px(0.),
    }
}

/// Where a formatted line's text starts, and its decorations.
fn deco_for(look: &Look, info: &LineInfo, buf: &Buffer) -> (Pixels, Deco) {
    let quote_x = look.quote_slot() * f32::from(info.quote);
    let depth = info.list_depth;
    let mut x = quote_x + look.list_slot() * f32::from(depth);
    let marker = match &info.marker {
        ListMarker::None => None,
        ListMarker::Bullet => Some(MarkerDeco::Bullet { depth }),
        ListMarker::Ordered(r) => Some(MarkerDeco::Number(buf.slice(r.clone()).to_owned().into())),
        ListMarker::Task { checked, .. } => Some(MarkerDeco::Task { checked: *checked }),
    };
    // A numbered marker wider than its slot ("100.") pushes the text.
    if let Some(MarkerDeco::Number(n)) = &marker {
        let need = look.size * (0.62 * n.len() as f32 + 0.5);
        if need > look.list_slot() {
            x += need - look.list_slot();
        }
    }
    let slot = quote_x + look.list_slot() * f32::from(depth.saturating_sub(1));
    (
        x,
        Deco {
            quote: info.quote,
            marker: marker.map(|m| (m, slot)),
        },
    )
}

/// Shape `segs` (buffer ranges, in order) with the line's styles; the map
/// is relative to `origin` (the line's start). `revealed`: markup bytes
/// among them are muted.
#[allow(clippy::too_many_arguments)]
fn shape(
    ts: &WindowTextSystem,
    buf: &Buffer,
    look: &Look,
    base: Base,
    info: &LineInfo,
    segs: Vec<Range<usize>>,
    origin: usize,
    at: usize,
    revealed: bool,
    x: Pixels,
    width: Pixels,
) -> TextBox {
    let segs: Vec<Range<usize>> = merge(segs);
    let mut display = String::new();
    let mut runs: Vec<TextRun> = Vec::new();
    for seg in &segs {
        // Cut points inside this segment.
        let mut cuts: Vec<usize> = vec![seg.start, seg.end];
        for s in &info.spans {
            cuts.extend([s.range.start, s.range.end]);
        }
        if revealed {
            for v in &info.visible {
                cuts.extend([v.start, v.end]);
            }
        }
        cuts.retain(|c| *c >= seg.start && *c <= seg.end);
        cuts.sort_unstable();
        cuts.dedup();
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if a >= b {
                continue;
            }
            display.push_str(buf.slice(a..b));
            runs.push(run_for(look, base, info, a, b, revealed));
        }
    }
    let map = DisplayMap::new(
        segs.iter()
            .map(|r| r.start - origin..r.end - origin)
            .collect(),
        at - origin,
    );
    let line_height = base.line_height;
    let wrap = (width - x).max(look.size * 4.);
    let display: SharedString = display.into();
    let wrapped = if display.is_empty() {
        None
    } else {
        ts.shape_text(display.clone(), base.size, &runs, Some(wrap), None)
            .ok()
            .and_then(|v| v.into_iter().next())
    };
    let row_starts = match &wrapped {
        Some(w) => std::iter::once(0)
            .chain(
                w.wrap_boundaries
                    .iter()
                    .map(|b| w.unwrapped_layout.runs[b.run_ix].glyphs[b.glyph_ix].index),
            )
            .collect(),
        None => vec![0],
    };
    TextBox {
        x,
        line_height,
        wrapped,
        map,
        row_starts,
        display,
    }
}

/// Join touching ranges.
fn merge(segs: Vec<Range<usize>>) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::with_capacity(segs.len());
    for s in segs {
        if s.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.end == s.start => last.end = s.end,
            _ => out.push(s),
        }
    }
    out
}

fn run_for(
    look: &Look,
    base: Base,
    info: &LineInfo,
    a: usize,
    b: usize,
    revealed: bool,
) -> TextRun {
    let mut bold = base.bold;
    let mut italic = false;
    let mut mono = base.mono;
    let mut strike = base.strike;
    let mut underline = false;
    let mut color = base.color;
    let mut background = None;
    for s in &info.spans {
        if s.range.start <= a && b <= s.range.end {
            match s.kind {
                SpanKind::Strong => bold = true,
                SpanKind::Emphasis => italic = true,
                SpanKind::Strike => strike = true,
                SpanKind::Code => {
                    mono = true;
                    background = Some(look.code_bg);
                }
                SpanKind::Link(_) | SpanKind::Footnote => {
                    color = look.accent;
                    underline = true;
                }
                SpanKind::Image(_) => {
                    color = look.accent;
                    italic = true;
                }
                SpanKind::Html => {
                    color = look.muted;
                    mono = true;
                }
            }
        }
    }
    let markup = revealed && !info.visible.iter().any(|v| v.start <= a && b <= v.end);
    if markup {
        color = look.dim;
        underline = false;
    }
    let mut font = font(if mono {
        look.mono.clone()
    } else {
        look.font.clone()
    });
    if bold {
        font.weight = FontWeight::BOLD;
    }
    if italic {
        font.style = FontStyle::Italic;
    }
    TextRun {
        len: b - a,
        font,
        color,
        background_color: background,
        underline: underline.then_some(UnderlineStyle {
            thickness: px(1.),
            color: Some(color),
            wavy: false,
        }),
        strikethrough: strike.then_some(StrikethroughStyle {
            thickness: px(1.),
            color: Some(color),
        }),
    }
}

#[allow(clippy::too_many_arguments)]
fn line_box(
    ts: &WindowTextSystem,
    buf: &Buffer,
    parsed: &Parsed,
    look: &Look,
    mode: Mode,
    l: usize,
    revealed: bool,
    width: Pixels,
    image: Option<ImageBox>,
) -> LineBox {
    let info = &parsed.lines[l];
    let lr = buf.line_range(l);
    let base = base_for(look, mode, info);
    let pad = pad_top(look, mode, info);
    let boxed = |body: Body, h: Pixels| LineBox {
        pad_top: pad,
        height: h + pad,
        body,
    };
    if !revealed {
        match &info.kind {
            LineKind::SetextUnderline(_) | LineKind::TableDelimiter { .. } => {
                return LineBox {
                    pad_top: px(0.),
                    height: px(0.),
                    body: Body::Hidden,
                };
            }
            LineKind::Rule => return boxed(Body::Rule, look.line_height()),
            LineKind::CodeFence { block, open } => {
                let lang = parsed
                    .code_blocks
                    .get(*block)
                    .map(|b| b.lang.as_str())
                    .unwrap_or("");
                let label = (*open && !lang.is_empty()).then(|| {
                    let small = Base {
                        size: look.size * 0.75,
                        color: look.dim,
                        ..base
                    };
                    // The language word, from the fence line itself.
                    let s = buf.line(l);
                    let at = s.find(lang).map_or(lr.end, |i| lr.start + i);
                    shape(
                        ts,
                        buf,
                        look,
                        small,
                        &plain_info(),
                        vec![at..(at + lang.len()).min(lr.end)],
                        lr.start,
                        at,
                        false,
                        look.size * 0.6,
                        width,
                    )
                });
                return boxed(Body::Fence { label }, base.line_height);
            }
            _ => {}
        }
        if let Some(img) = image.clone() {
            let h = img.size.height + look.size * 0.5;
            return boxed(Body::Image(img), h);
        }
    }
    let code = matches!(
        info.kind,
        LineKind::Code { .. } | LineKind::CodeFence { .. } | LineKind::FrontMatter { .. }
    ) && mode != Mode::Source;
    let (text, deco) = if revealed {
        // Every byte, the content kept where the formatted line has it:
        // the markup sits to its left.
        let (fx, _) = deco_for(look, info, buf);
        // A line the parser didn't recognise has no markup to mute.
        let dim = !matches!(info.kind, LineKind::Raw);
        let prefix = lr.start..info.content_start.clamp(lr.start, lr.end);
        let prefix_w = if prefix.is_empty() || mode == Mode::Source {
            px(0.)
        } else {
            let t = shape(
                ts,
                buf,
                look,
                base,
                info,
                vec![prefix],
                lr.start,
                lr.start,
                true,
                px(0.),
                px(100_000.),
            );
            t.wrapped
                .as_ref()
                .map_or(px(0.), |w| w.unwrapped_layout.width)
        };
        let x = if mode == Mode::Source || code {
            px(0.)
        } else {
            (fx - prefix_w).max(px(0.))
        };
        let x = if code { look.size * 0.6 } else { x };
        (
            shape(
                ts,
                buf,
                look,
                base,
                info,
                vec![lr.clone()],
                lr.start,
                lr.start,
                dim,
                x,
                width,
            ),
            Deco::default(),
        )
    } else {
        let (x, deco) = if code {
            (look.size * 0.6, Deco::default())
        } else {
            deco_for(look, info, buf)
        };
        let segs = match info.kind {
            LineKind::Raw => vec![info.content_start..lr.end],
            _ => info.visible.clone(),
        };
        (
            shape(
                ts,
                buf,
                look,
                base,
                info,
                segs,
                lr.start,
                info.content_start.clamp(lr.start, lr.end),
                false,
                x,
                width,
            ),
            deco,
        )
    };
    let image_h = image
        .as_ref()
        .map_or(px(0.), |i| i.size.height + look.size * 0.5);
    let h = text.height() + image_h;
    boxed(
        Body::Text {
            text,
            deco,
            code,
            image,
        },
        h,
    )
}

fn plain_info() -> LineInfo {
    LineInfo {
        kind: LineKind::Paragraph,
        content_start: 0,
        quote: 0,
        list_depth: 0,
        marker: ListMarker::None,
        group: 0..1,
        visible: Vec::new(),
        spans: Vec::new(),
    }
}

/// The picture a line shows on its own: the line's content is one image
/// (a local file Kantor can read).
#[allow(clippy::too_many_arguments)]
fn picture_of(
    buf: &Buffer,
    parsed: &Parsed,
    info: &LineInfo,
    l: usize,
    dir: Option<&Path>,
    sizes: &mut ImageSizes,
    width: Pixels,
    look: &Look,
) -> Option<ImageBox> {
    if !matches!(info.kind, LineKind::Paragraph) {
        return None;
    }
    let lr = buf.line_range(l);
    let content = info.content_start..lr.end;
    let img = info.spans.iter().find_map(|s| match s.kind {
        SpanKind::Image(i) if s.range.start <= content.start + leading_ws(buf, &content) => {
            Some((i, s.range.clone()))
        }
        _ => None,
    })?;
    if !buf.slice(img.1.end..lr.end).trim().is_empty() {
        return None;
    }
    let dest = &parsed.images.get(img.0)?.dest;
    let path = resolve(dir, dest)?;
    let (w, h) = (*sizes.entry(path.clone()).or_insert_with(|| {
        imagesize::size(&path)
            .ok()
            .map(|s| (s.width as u32, s.height as u32))
    }))?;
    if w == 0 || h == 0 {
        return None;
    }
    let max_w = f32::from(width);
    let max_h = f32::from(look.size) * 30.;
    let (mut dw, mut dh) = (w as f32, h as f32);
    let k = (max_w / dw).min(max_h / dh).min(1.0);
    dw *= k;
    dh *= k;
    Some(ImageBox {
        path,
        size: size(px(dw), px(dh)),
    })
}

fn leading_ws(buf: &Buffer, r: &Range<usize>) -> usize {
    let s = buf.slice(r.clone());
    s.len() - s.trim_start().len()
}

/// A picture link's file: relative to the note's folder; `file://` URLs
/// and absolute paths as they are. Web pictures aren't fetched.
pub fn resolve(dir: Option<&Path>, dest: &str) -> Option<PathBuf> {
    let dest = dest.trim();
    if dest.is_empty() || (dest.contains("://") && !dest.starts_with("file://")) {
        return None;
    }
    let dest = dest.strip_prefix("file://").unwrap_or(dest);
    let dest = percent_decode(dest);
    let p = Path::new(&dest);
    if p.is_absolute() {
        return Some(p.to_path_buf());
    }
    Some(dir?.join(p))
}

/// `%20` and friends in a link (Obsidian writes spaces that way).
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 3 <= b.len()
            && let Some(v) = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

fn table_box(
    ts: &WindowTextSystem,
    buf: &Buffer,
    parsed: &Parsed,
    look: &Look,
    table: usize,
    width: Pixels,
) -> LineBox {
    let t = &parsed.tables[table];
    let first = t.lines.start;
    let ncols = t
        .rows
        .iter()
        .map(|r| r.cells.len())
        .max()
        .unwrap_or(1)
        .max(1);
    let pad_x = look.size * 0.6;
    let pad_y = look.size * 0.3;
    let info_of = |l: usize| &parsed.lines[l];
    let base = |header: bool| Base {
        size: look.size,
        line_height: look.line_height(),
        bold: header,
        mono: false,
        color: look.fg,
        strike: false,
    };
    let cell_segs = |l: usize, c: &Range<usize>| -> Vec<Range<usize>> {
        info_of(l)
            .visible
            .iter()
            .filter_map(|v| {
                let (a, b) = (v.start.max(c.start), v.end.min(c.end));
                (a < b).then_some(a..b)
            })
            .collect()
    };
    // Natural widths.
    let mut natural = vec![look.size * 3.; ncols];
    for (ri, row) in t.rows.iter().enumerate() {
        for (ci, c) in row.cells.iter().enumerate() {
            let b = shape(
                ts,
                buf,
                look,
                base(ri == 0),
                info_of(row.line),
                cell_segs(row.line, c),
                buf.line_start(row.line),
                c.start,
                false,
                px(0.),
                px(100_000.),
            );
            let w = b
                .wrapped
                .as_ref()
                .map_or(px(0.), |w| w.unwrapped_layout.width)
                + pad_x * 2.;
            natural[ci] = natural[ci].max(w);
        }
    }
    let total: Pixels = natural.iter().copied().fold(px(0.), |a, b| a + b);
    let cols: Vec<Pixels> = if total <= width {
        natural
    } else {
        let k = width / total;
        natural
            .iter()
            .map(|w| (*w * k).max(look.size * 3.))
            .collect()
    };
    let mut xs = Vec::with_capacity(ncols);
    let mut x = px(0.);
    for w in &cols {
        xs.push(x);
        x += *w;
    }
    let table_w = x;
    let mut rows = Vec::new();
    let mut y = px(0.);
    for (ri, row) in t.rows.iter().enumerate() {
        let mut cells = Vec::new();
        let mut h = look.line_height();
        for (ci, c) in row.cells.iter().enumerate() {
            let text = shape(
                ts,
                buf,
                look,
                base(ri == 0),
                info_of(row.line),
                cell_segs(row.line, c),
                buf.line_start(row.line),
                c.start,
                false,
                px(0.),
                cols[ci] - pad_x * 2.,
            );
            h = h.max(text.height());
            cells.push(TableCell {
                text,
                dl: row.line - first,
                x: xs[ci] + pad_x,
                width: cols[ci] - pad_x * 2.,
                align: t.aligns.get(ci).copied().unwrap_or(Align::None),
            });
        }
        let height = h + pad_y * 2.;
        rows.push(TableRowBox {
            top: y,
            height,
            header: ri == 0,
            cells,
        });
        y += height;
    }
    LineBox {
        pad_top: px(0.),
        height: y + look.size * 0.4,
        body: Body::Table(TableBox {
            x: px(0.),
            width: table_w,
            cols,
            rows,
        }),
    }
}

// ── positions ────────────────────────────────────────────────────────────

/// A caret: top-left and height, in document space (relative to the text
/// column's left and the layout's top).
#[derive(Clone, Copy, Debug)]
pub struct Caret {
    pub at: Point<Pixels>,
    pub height: Pixels,
}

impl Laid {
    /// Where the caret for buffer offset `offset` (on line `line`) goes.
    pub fn caret(&self, buf: &Buffer, line: usize, offset: usize) -> Option<Caret> {
        let b = self.lines.get(line)?;
        let top = self.tops[line] + b.pad_top;
        let start = buf.line_start(line);
        match &b.body {
            Body::Text { text, .. } => {
                let p = text.caret(text.map.to_display(offset.saturating_sub(start)));
                Some(Caret {
                    at: point(p.x, top + p.y),
                    height: text.line_height,
                })
            }
            Body::Table(t) => {
                for r in &t.rows {
                    for c in &r.cells {
                        let base = buf.line_start(line + c.dl);
                        let sr = c.text.map.source_range();
                        if offset >= base + sr.start && offset <= base + sr.end {
                            let p = c.text.caret(c.text.map.to_display(offset - base));
                            return Some(Caret {
                                at: point(t.x + c.x + p.x, top + r.top + p.y),
                                height: c.text.line_height,
                            });
                        }
                    }
                }
                Some(Caret {
                    at: point(px(0.), top),
                    height: b.height,
                })
            }
            _ => Some(Caret {
                at: point(px(0.), top),
                height: (b.height - b.pad_top).max(px(1.)),
            }),
        }
    }

    /// The buffer offset nearest to a document-space point.
    pub fn hit(&self, p: Point<Pixels>, buf: &Buffer) -> usize {
        if self.lines.is_empty() {
            return 0;
        }
        if p.y >= self.height() {
            return buf.len();
        }
        let i = self.line_at_y(p.y);
        let b = &self.lines[i];
        let top = self.tops[i] + b.pad_top;
        let local = point(p.x, p.y - top);
        let start = buf.line_start(i);
        match &b.body {
            Body::Text { text, .. } => start + text.map.to_source(text.hit(local)),
            Body::Table(t) => {
                for r in &t.rows {
                    if local.y >= r.top && local.y < r.top + r.height {
                        let cell = r
                            .cells
                            .iter()
                            .rev()
                            .find(|c| local.x >= t.x + c.x - px(8.))
                            .or(r.cells.first());
                        if let Some(c) = cell {
                            let lp = point(local.x - t.x - c.x, local.y - r.top);
                            return buf.line_start(i + c.dl) + c.text.map.to_source(c.text.hit(lp));
                        }
                    }
                }
                start
            }
            _ => start,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn display_map_round_trips() {
        // "a **b** c": shown "a b c".
        let m = DisplayMap::new(vec![0..2, 4..5, 7..9], 0);
        assert_eq!(m.len(), 5);
        assert_eq!(m.to_display(0), 0);
        assert_eq!(m.to_display(3), 2, "inside the opening marks");
        assert_eq!(m.to_display(4), 2);
        assert_eq!(m.to_display(5), 3);
        assert_eq!(m.to_display(9), 5);
        assert_eq!(m.to_source(2), 2);
        assert_eq!(m.to_source(3), 5);
        assert_eq!(m.to_source(5), 9);
        assert_eq!(m.source_range(), 0..9);
        let empty = DisplayMap::new(vec![], 7);
        assert_eq!(empty.to_source(0), 7);
    }

    #[std::prelude::v1::test]
    fn merges_touching_ranges() {
        assert_eq!(merge(vec![0..2, 2..4, 5..6, 6..6]), vec![0..4, 5..6]);
    }

    fn text_system() -> WindowTextSystem {
        let platform = std::sync::Arc::new(gpui_wgpu::CosmicTextSystem::new("Liberation Sans"));
        WindowTextSystem::new(std::sync::Arc::new(TextSystem::new(platform)))
    }

    fn look() -> Look {
        Look {
            font: "Liberation Sans".into(),
            mono: "Liberation Mono".into(),
            size: px(15.),
            fg: black(),
            muted: black(),
            dim: black(),
            accent: black(),
            code_bg: black(),
            border: black(),
        }
    }

    fn lay(
        ts: &WindowTextSystem,
        buf: &Buffer,
        revealed: &dyn Fn(usize) -> bool,
        cache: &mut HashMap<u64, Rc<LineBox>>,
    ) -> Laid {
        let parsed = notes_engine::parse(buf);
        let mut sizes = HashMap::new();
        layout(
            ts,
            Input {
                buf,
                parsed: &parsed,
                look: &look(),
                mode: Mode::Live,
                revealed,
                width: px(600.),
                dir: None,
                image_sizes: &mut sizes,
            },
            cache,
        )
    }

    const NOTE: &str =
        "# Title\n\nSome **bold** text\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n- [x] done\n";

    #[std::prelude::v1::test]
    fn one_box_per_line_and_tables_as_grids() {
        let ts = text_system();
        let buf = Buffer::new(NOTE);
        let laid = lay(&ts, &buf, &|_| false, &mut HashMap::new());
        assert_eq!(laid.lines.len(), buf.line_count());
        assert!(matches!(laid.lines[4].body, Body::Table(_)));
        assert_eq!(laid.lines[5].height, px(0.));
        assert_eq!(laid.lines[6].height, px(0.));
        match &laid.lines[8].body {
            Body::Text { deco, text, .. } => {
                assert!(matches!(
                    deco.marker,
                    Some((MarkerDeco::Task { checked: true }, _))
                ));
                assert_eq!(text.display.as_ref(), "done");
            }
            _ => panic!("task line"),
        }
        match &laid.lines[2].body {
            Body::Text { text, .. } => assert_eq!(text.display.as_ref(), "Some bold text"),
            _ => panic!("paragraph"),
        }
    }

    #[std::prelude::v1::test]
    fn revealed_lines_show_every_byte() {
        let ts = text_system();
        let buf = Buffer::new(NOTE);
        let laid = lay(&ts, &buf, &|l| l == 2, &mut HashMap::new());
        match &laid.lines[2].body {
            Body::Text { text, .. } => assert_eq!(text.display.as_ref(), "Some **bold** text"),
            _ => panic!("paragraph"),
        }
    }

    #[std::prelude::v1::test]
    fn carets_and_clicks_land_on_the_same_bytes() {
        let ts = text_system();
        let buf = Buffer::new(NOTE);
        let laid = lay(&ts, &buf, &|_| false, &mut HashMap::new());
        let start = buf.line_start(2);
        // Inside "Some ", "bold" and " text" (not at the hidden `**`).
        for o in [0, 2, 8, 10, 15, 18] {
            let c = laid.caret(&buf, 2, start + o).expect("caret");
            let back = laid.hit(point(c.at.x + px(0.3), c.at.y + c.height / 2.), &buf);
            assert_eq!(back, start + o, "offset {o}");
        }
        // Below everything: the end.
        assert_eq!(
            laid.hit(point(px(0.), laid.height() + px(5.)), &buf),
            buf.len()
        );
    }

    #[std::prelude::v1::test]
    fn an_edit_reshapes_only_its_line() {
        let ts = text_system();
        let mut cache = HashMap::new();
        let buf = Buffer::new(NOTE);
        let a = lay(&ts, &buf, &|_| false, &mut cache);
        // Typing on the first line moves every later line's bytes.
        let buf = Buffer::new(&NOTE.replacen("Title", "Title!", 1));
        let b = lay(&ts, &buf, &|_| false, &mut cache);
        assert!(!Rc::ptr_eq(&a.lines[0], &b.lines[0]));
        for i in 1..a.lines.len() {
            if matches!(a.lines[i].body, Body::Hidden) {
                continue;
            }
            assert!(Rc::ptr_eq(&a.lines[i], &b.lines[i]), "line {i} reshaped");
        }
    }

    #[std::prelude::v1::test]
    fn empty_note() {
        let ts = text_system();
        let buf = Buffer::new("");
        let laid = lay(&ts, &buf, &|_| true, &mut HashMap::new());
        assert_eq!(laid.lines.len(), 1);
        assert!(laid.height() > px(0.));
        assert_eq!(laid.hit(point(px(10.), px(1.)), &buf), 0);
    }

    #[std::prelude::v1::test]
    fn picture_paths() {
        let dir = Path::new("/notes");
        assert_eq!(
            resolve(Some(dir), "attachments/Pasted%20image.png"),
            Some(PathBuf::from("/notes/attachments/Pasted image.png"))
        );
        assert_eq!(resolve(Some(dir), "https://x.y/a.png"), None);
        assert_eq!(resolve(None, "a.png"), None);
    }
}
