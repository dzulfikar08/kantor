//! Paged rich-text layout for Kantor Docs (no GPUI).
//!
//! Text shaping, bidi, UAX #14 line breaking and justification come from
//! [parley]; this crate adds what a word processor needs on top: fixed-size
//! pages with margins, paragraph indents and list markers, inline object
//! boxes (pictures, placeholders), tables whose rows flow onto pages,
//! pagination with widow/orphan control and page breaks, a renderer-neutral
//! list of positioned glyph runs and rules per page (painted by GPUI with
//! `Window::paint_glyph`, written to PDF by krilla), and caret hit-testing
//! in page coordinates.
//!
//! Units are CSS pixels at 96 dpi, so 1 pt = 4/3 px.
//!
//! Text conventions shared with `docs_engine` (byte offsets must match the
//! model): `\t` is laid out as a space (no tab stops yet), U+000C (page
//! break) as a hard line break followed by a new page, U+FFFC (an object)
//! as a zero-width space with the object's box at the same offset.

use std::ops::Range;
use std::sync::Arc;

use parley::{
    Affinity, Alignment, AlignmentOptions, BreakReason, Cursor, FontContext, FontData, FontFamily,
    FontStyle, FontWeight, IndentOptions, InlineBox, InlineBoxKind, Layout, LayoutContext,
    LineHeight, PositionedLayoutItem, StyleProperty, VerticalAlign,
};

pub use parley;

/// Points to pixels at 96 dpi.
pub const PT: f32 = 96.0 / 72.0;
/// Millimetres to pixels at 96 dpi.
pub const MM: f32 = 96.0 / 25.4;

/// The model's object character.
pub const OBJECT: char = '\u{FFFC}';
/// The model's page-break character.
pub const PAGE_BREAK: char = '\u{000C}';

/// Straight sRGB colour, the brush type stored in parley layouts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgba(pub [u8; 4]);

impl Rgba {
    pub const BLACK: Self = Self([0, 0, 0, 255]);
    /// Placeholder text.
    pub const GREY: Self = Self([0x70, 0x70, 0x70, 255]);
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

impl Default for PageSetup {
    fn default() -> Self {
        Self::a4()
    }
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

    /// US Letter portrait (8.5 × 11 in) with 1 in margins.
    pub fn letter() -> Self {
        Self {
            width: 215.9 * MM,
            height: 279.4 * MM,
            ..Self::a4()
        }
    }

    /// The same page turned sideways (margins turn with it).
    pub fn landscape(self) -> Self {
        if self.width >= self.height {
            return self;
        }
        Self {
            width: self.height,
            height: self.width,
            margin_top: self.margin_left,
            margin_left: self.margin_top,
            margin_bottom: self.margin_right,
            margin_right: self.margin_bottom,
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

/// A list marker ("1.", "•") drawn in the paragraph's hanging indent.
#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    pub text: String,
    /// Formatting (the range is ignored).
    pub style: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ObjectKind {
    /// A picture: the renderer draws it into the box.
    Image,
    /// A box with a label, for content shown only as a placeholder.
    Placeholder,
    /// Zero size (bookmarks).
    Hidden,
}

/// An object in the text: a box in place of the object's character.
#[derive(Clone, Debug, PartialEq)]
pub struct InlineObject {
    /// Byte offset of the object character in the paragraph text.
    pub index: usize,
    pub kind: ObjectKind,
    /// Size in px; a placeholder with width 0 is sized to its label.
    pub width: f32,
    pub height: f32,
    pub label: String,
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
    /// Left and right indents and the first-line indent (negative =
    /// hanging), in pt.
    pub indent_left_pt: f32,
    pub indent_right_pt: f32,
    pub first_line_pt: f32,
    pub marker: Option<Marker>,
    pub objects: Vec<InlineObject>,
    /// Start on a new page.
    pub page_break_before: bool,
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
            indent_left_pt: 0.0,
            indent_right_pt: 0.0,
            first_line_pt: 0.0,
            marker: None,
            objects: Vec::new(),
            page_break_before: false,
            locale: None,
        }
    }

    /// The text as shaped: same byte offsets, layout stand-ins for tabs,
    /// page breaks and objects.
    fn shaped_text(&self) -> String {
        self.text
            .chars()
            .map(|c| match c {
                '\t' => " ".to_owned(),
                PAGE_BREAK => "\n".to_owned(),
                OBJECT => "\u{200B}".to_owned(),
                c => c.to_string(),
            })
            .collect()
    }
}

/// A shaped paragraph: the text layout plus its marker and placeholder
/// labels.
pub struct ParaLayout {
    pub layout: Layout<Rgba>,
    /// x of the layout's origin from the column's left edge (indent).
    pub dx: f32,
    /// The marker's layout, its x from the column's left edge, and text.
    pub marker: Option<(Layout<Rgba>, f32, String)>,
    /// Placeholder labels: object byte index, layout, text.
    pub labels: Vec<(usize, Layout<Rgba>, String)>,
    /// Objects by inline-box id: byte index and kind.
    pub objects: Vec<(usize, ObjectKind)>,
}

impl ParaLayout {
    pub fn len(&self) -> usize {
        self.layout.len()
    }

    pub fn is_empty(&self) -> bool {
        self.layout.len() == 0
    }
}

/// Padding inside a placeholder box, px.
const LABEL_PAD: f32 = 4.0;
/// Gap between a list marker and the text, px.
const MARKER_GAP: f32 = 6.0;

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

fn push_span(b: &mut parley::RangedBuilder<'_, Rgba>, span: &Span, r: Range<usize>) {
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

impl Engine {
    pub fn new() -> Self {
        Self::with_fonts(FontContext::new())
    }

    /// An engine on fonts already found (a clone of another context skips
    /// scanning the system's fonts again).
    pub fn with_fonts(fonts: FontContext) -> Self {
        Self {
            fonts,
            layout_cx: LayoutContext::new(),
        }
    }

    /// A one-line label or marker layout.
    fn small_layout(
        &mut self,
        text: &str,
        family: &str,
        size_pt: f32,
        style: Option<&Span>,
        color: Rgba,
    ) -> Layout<Rgba> {
        let mut b = self
            .layout_cx
            .ranged_builder(&mut self.fonts, text, 1.0, true);
        b.push_default(StyleProperty::FontFamily(FontFamily::Source(family.into())));
        b.push_default(StyleProperty::FontSize(size_pt * PT));
        b.push_default(StyleProperty::Brush(color));
        if let Some(s) = style {
            push_span(&mut b, s, 0..text.len());
        }
        let mut layout = b.build(text);
        layout.break_all_lines(None);
        layout.align(Alignment::Start, AlignmentOptions::default());
        layout
    }

    /// Shapes one paragraph and breaks it into lines in a column `width` px
    /// wide (indents are taken off that width).
    pub fn layout_paragraph(&mut self, para: &Paragraph, width: f32) -> ParaLayout {
        let left = para.indent_left_pt * PT;
        let right = para.indent_right_pt * PT;
        let first = para.first_line_pt * PT;

        // Marker first: its width decides where the first line starts.
        let marker = para
            .marker
            .as_ref()
            .filter(|m| !m.text.is_empty())
            .map(|m| {
                let family = m
                    .style
                    .family
                    .clone()
                    .unwrap_or_else(|| para.family.clone());
                let size = m.style.size_pt.unwrap_or(para.size_pt);
                let color = m.style.color.unwrap_or(Rgba::BLACK);
                let l = self.small_layout(&m.text, &family, size, Some(&m.style), color);
                (l, m.text.clone())
            });

        // Placeholder labels, which size their boxes.
        let mut labels = Vec::new();
        let mut boxes = Vec::new();
        for (id, o) in para.objects.iter().enumerate() {
            let (mut w, mut h) = (o.width, o.height);
            if o.kind == ObjectKind::Placeholder {
                let l = self.small_layout(
                    &o.label,
                    &para.family,
                    para.size_pt * 0.85,
                    None,
                    Rgba::GREY,
                );
                let lw = l.width();
                let lh = l.height();
                if w <= 0.0 {
                    w = lw + 2.0 * LABEL_PAD;
                }
                if h <= 0.0 {
                    h = lh + LABEL_PAD;
                }
                labels.push((o.index, l, o.label.clone()));
            }
            if o.kind == ObjectKind::Hidden {
                w = 0.0;
                h = 0.0;
            }
            // The box goes at the object character itself (before its
            // zero-width stand-in): parley then puts the caret at the
            // character's start before the box and after it at its end.
            let at = o.index;
            if at < para.text.len() {
                boxes.push(InlineBox {
                    id: id as u64,
                    kind: InlineBoxKind::InFlow,
                    index: at,
                    width: w.min(width.max(1.0)),
                    height: h,
                    baseline: None,
                    vertical_align: VerticalAlign::BASELINE,
                });
            }
        }

        let text = para.shaped_text();
        let mut b = self
            .layout_cx
            .ranged_builder(&mut self.fonts, &text, 1.0, true);
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
            push_span(&mut b, span, span.range.clone());
        }
        for ib in boxes {
            b.push_inline_box(ib);
        }
        let mut layout = b.build(&text);

        // Indents: with a marker, every line starts at the left indent and
        // the first line makes room for the marker (Word's tab after the
        // number); otherwise first-line / hanging indents as usual.
        let (dx, marker) = match marker {
            Some((ml, mtext)) => {
                let mx = left + first;
                let text_x =
                    (mx + ml.width() + MARKER_GAP).max(if first < 0.0 { left } else { mx });
                layout.set_text_indent((text_x - left).max(0.0), IndentOptions::default());
                (left, Some((ml, mx, mtext)))
            }
            None => {
                let dx = left + first.min(0.0);
                layout.set_text_indent(
                    first.abs(),
                    IndentOptions {
                        each_line: false,
                        hanging: first < 0.0,
                    },
                );
                (dx, None)
            }
        };
        let avail = (width - dx - right).max(8.0);
        layout.break_all_lines(Some(avail));
        let alignment = match para.align {
            Align::Start => Alignment::Start,
            Align::Center => Alignment::Center,
            Align::End => Alignment::End,
            Align::Justify => Alignment::Justify,
        };
        layout.align(alignment, AlignmentOptions::default());
        ParaLayout {
            layout,
            dx,
            marker,
            labels,
            objects: para.objects.iter().map(|o| (o.index, o.kind)).collect(),
        }
    }

    /// Lays out every paragraph (all in the body) and paginates the result.
    pub fn layout_document(&mut self, paras: &[Paragraph], setup: PageSetup) -> PagedDocument {
        let width = setup.content_width();
        let layouts = paras
            .iter()
            .map(|p| Arc::new(self.layout_paragraph(p, width)))
            .collect();
        PagedDocument::paginate(paras, layouts, setup)
    }
}

// ── blocks and tables ──────────────────────────────────────────────────────

/// A border line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rule {
    pub width: f32,
    pub color: Rgba,
}

/// A filled rectangle on a page (table borders): `[x, y, w, h]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RuleRect {
    pub rect: [f32; 4],
    pub color: Rgba,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Merge {
    #[default]
    None,
    /// First cell of a vertical merge: its content sits in the first row.
    Restart,
    /// Covered by the cell above.
    Continue,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableCell {
    /// First grid column and columns covered.
    pub col: usize,
    pub span: usize,
    /// Paragraph indices of the cell's content.
    pub paras: Range<usize>,
    pub merge: Merge,
    /// Top, right, bottom, left.
    pub borders: [Option<Rule>; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableRow {
    pub cells: Vec<TableCell>,
    /// Minimum (or exact) height in px.
    pub min_height: f32,
    pub exact: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableBlock {
    /// Left edge from the content area's left, px.
    pub x: f32,
    /// Grid column widths, px.
    pub columns: Vec<f32>,
    pub rows: Vec<TableRow>,
    /// Cell padding: top, right, bottom, left (px).
    pub padding: [f32; 4],
}

impl TableBlock {
    /// x of a grid column from the table's left edge.
    pub fn col_x(&self, col: usize) -> f32 {
        self.columns.iter().take(col).sum()
    }

    pub fn span_width(&self, col: usize, span: usize) -> f32 {
        self.columns.iter().skip(col).take(span.max(1)).sum()
    }

    /// Width for the text of a cell (its columns minus padding).
    pub fn text_width(&self, cell: &TableCell) -> f32 {
        (self.span_width(cell.col, cell.span) - self.padding[1] - self.padding[3]).max(8.0)
    }
}

/// What the body is made of, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Para(usize),
    Table(TableBlock),
}

// ── pages ──────────────────────────────────────────────────────────────────

/// A line of a paragraph placed on a page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedLine {
    pub para: usize,
    /// Line index within the paragraph's layout.
    pub line: usize,
    /// Top of the line box in page coordinates (px from the page's top edge).
    pub top: f32,
    pub height: f32,
    /// The column holding the line (the content area, or a cell's text
    /// area): left edge and width, page coordinates.
    pub x: f32,
    pub width: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub lines: Vec<PlacedLine>,
    /// Table borders.
    pub rules: Vec<RuleRect>,
    /// This page's size and margins (sections can differ).
    pub setup: PageSetup,
    /// The section the page belongs to.
    pub section: usize,
    /// Its number within the section's page numbering run: 1-based page
    /// count from the last restart (see [`SectionFlow::restart`]).
    pub number: u32,
}

/// How a section starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SectionBreak {
    #[default]
    NextPage,
    Continuous,
    EvenPage,
    OddPage,
}

/// A section for [`PagedDocument::paginate_sections`]: where it starts and
/// its pages' geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SectionFlow {
    /// Index into `blocks` of the section's first block.
    pub first_block: usize,
    pub setup: PageSetup,
    /// The section's first page, when it differs (a taller first-page
    /// header pushes the text down).
    pub first_setup: Option<PageSetup>,
    /// Even pages, when they differ.
    pub even_setup: Option<PageSetup>,
    pub start: SectionBreak,
    /// Restart page numbering at this number.
    pub restart: Option<u32>,
}

/// What a glyph run's text comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunSource {
    /// The paragraph's text (`text_range` indexes it).
    Text,
    /// The paragraph's list marker.
    Marker,
    /// The label of the placeholder for the object at this byte index.
    Label(usize),
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
    pub para: usize,
    /// Byte range of the source text covered by this run (see `source`).
    pub text_range: Range<usize>,
    pub source: RunSource,
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

/// An object's box on a page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedObject {
    pub para: usize,
    /// Byte offset of the object character.
    pub index: usize,
    pub kind: ObjectKind,
    /// `[x, y, w, h]` in page coordinates.
    pub rect: [f32; 4],
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
    pub layouts: Vec<Arc<ParaLayout>>,
    pub pages: Vec<Page>,
    /// For each paragraph and each of its lines: the page, the y that maps
    /// the layout's coordinates to the page, and the x of the layout origin.
    line_pages: Vec<Vec<(usize, f32, f32)>>,
}

/// Lines a paragraph keeps together at a page break (Word's widow/orphan control).
const MIN_LINES: usize = 2;

/// Lays blocks out onto pages.
struct Flow<'a> {
    /// The current page's geometry.
    setup: PageSetup,
    paras: &'a [Paragraph],
    layouts: &'a [Arc<ParaLayout>],
    pages: Vec<Page>,
    line_pages: Vec<Vec<(usize, f32, f32)>>,
    y: f32,
    sections: &'a [SectionFlow],
    section: usize,
    /// Pages of the current section so far.
    section_pages: usize,
    /// Number of the next page.
    number: u32,
}

impl Flow<'_> {
    fn bottom(&self) -> f32 {
        self.setup.height - self.setup.margin_bottom
    }

    fn at_top(&self) -> bool {
        self.y <= self.setup.margin_top
    }

    /// The geometry of the section's next page.
    fn next_setup(&self) -> PageSetup {
        let s = &self.sections[self.section];
        if self.section_pages == 0
            && let Some(f) = s.first_setup
        {
            return f;
        }
        if self.number.is_multiple_of(2)
            && let Some(e) = s.even_setup
        {
            return e;
        }
        s.setup
    }

    fn new_page(&mut self) {
        self.setup = self.next_setup();
        self.pages.push(Page {
            setup: self.setup,
            section: self.section,
            number: self.number,
            ..Page::default()
        });
        self.number += 1;
        self.section_pages += 1;
        self.y = self.setup.margin_top;
    }

    /// Start section `i` (its first block is about to be placed).
    fn start_section(&mut self, i: usize) {
        let s = self.sections[i];
        let continuous = s.start == SectionBreak::Continuous
            && !self.pages.is_empty()
            && s.setup.width == self.setup.width
            && s.setup.height == self.setup.height;
        self.section = i;
        if continuous {
            // Same page: the new section's margins apply from here on.
            self.section_pages = 1;
            if let Some(n) = s.restart {
                self.number = n;
            }
            return;
        }
        self.section_pages = 0;
        if let Some(n) = s.restart {
            self.number = n;
        }
        // Even/odd page starts leave a blank page when needed.
        let parity = match s.start {
            SectionBreak::EvenPage => Some(0),
            SectionBreak::OddPage => Some(1),
            _ => None,
        };
        if let Some(want) = parity
            && !self.pages.is_empty()
            && self.number % 2 != want
        {
            self.new_page();
            self.section_pages = 0;
        }
        self.new_page();
    }

    /// Whether line `k` of paragraph `pi` ends with a page break.
    fn breaks_after(&self, pi: usize, k: usize) -> bool {
        let Some(line) = self.layouts[pi].layout.get(k) else {
            return false;
        };
        let end = line.text_range().end;
        end > 0 && self.paras[pi].text.as_bytes().get(end - 1) == Some(&(PAGE_BREAK as u8))
    }

    fn push_line(&mut self, pi: usize, k: usize, top: f32, (x, width): (f32, f32)) {
        let layout = &self.layouts[pi];
        let m = *layout.layout.get(k).expect("line exists").metrics();
        let page = self.pages.len() - 1;
        self.pages[page].lines.push(PlacedLine {
            para: pi,
            line: k,
            top,
            height: m.line_height,
            x,
            width,
        });
        self.line_pages[pi].push((page, top - m.block_min_coord, x + layout.dx));
    }

    /// A body paragraph in the full-width column.
    fn place_para(&mut self, pi: usize) {
        let para = &self.paras[pi];
        let layout = self.layouts[pi].clone();
        let column = (self.setup.margin_left, self.setup.content_width());
        if para.page_break_before && !self.at_top() {
            self.new_page();
        }
        let heights: Vec<f32> = layout
            .layout
            .lines()
            .map(|l| l.metrics().line_height)
            .collect();
        let n = heights.len();
        let forced: Vec<bool> = (0..n).map(|k| self.breaks_after(pi, k)).collect();
        let keep_together = !forced.iter().any(|&f| f);
        // Space before is dropped at the top of a page, as in Word.
        if !self.at_top() {
            self.y += para.space_before_pt * PT;
        }
        let bottom = self.bottom();
        let mut i = 0;
        while i < n {
            // How many lines fit on this page from line i on?
            let mut fit = 0;
            let mut h = 0.0;
            while i + fit < n && self.y + h + heights[i + fit] <= bottom {
                h += heights[i + fit];
                fit += 1;
                if forced[i + fit - 1] {
                    break;
                }
            }
            let rest = n - i - fit;
            let mut take = fit;
            if rest > 0 && n >= MIN_LINES * 2 && keep_together {
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
            if take == 0 && self.at_top() {
                // A line taller than the page, or nothing fits: force one line.
                take = 1;
            }
            for (k, h) in heights.iter().enumerate().skip(i).take(take) {
                let top = self.y;
                self.push_line(pi, k, top, column);
                self.y += h;
            }
            i += take;
            let broke = take > 0 && forced[i - 1];
            if i < n || broke {
                self.new_page();
            }
        }
        self.y += para.space_after_pt * PT;
    }

    /// Height of a cell's content (paragraph spacing included).
    fn cell_height(&self, cell: &TableCell) -> f32 {
        let mut h = 0.0;
        for (k, pi) in cell.paras.clone().enumerate() {
            let p = &self.paras[pi];
            if k > 0 {
                h += p.space_before_pt * PT;
            }
            h += self.layouts[pi]
                .layout
                .lines()
                .map(|l| l.metrics().line_height)
                .sum::<f32>();
            h += p.space_after_pt * PT;
        }
        h
    }

    fn place_table(&mut self, t: &TableBlock) {
        let [pad_t, pad_r, pad_b, pad_l] = t.padding;
        let x0 = self.setup.margin_left + t.x;
        for row in &t.rows {
            let content = row
                .cells
                .iter()
                .filter(|c| c.merge != Merge::Continue)
                .map(|c| self.cell_height(c) + pad_t + pad_b)
                .fold(0.0f32, f32::max);
            let h = if row.exact {
                row.min_height
            } else {
                content.max(row.min_height)
            }
            .max(1.0);
            // Rows don't split across pages.
            if self.y + h > self.bottom() && !self.at_top() {
                self.new_page();
            }
            let top = self.y;
            let page = self.pages.len() - 1;
            for cell in &row.cells {
                let cx = x0 + t.col_x(cell.col);
                let cw = t.span_width(cell.col, cell.span);
                let column = (cx + pad_l, (cw - pad_l - pad_r).max(8.0));
                let mut y = top + pad_t;
                for (k, pi) in cell.paras.clone().enumerate() {
                    let p = &self.paras[pi];
                    if k > 0 {
                        y += p.space_before_pt * PT;
                    }
                    for line in 0..self.layouts[pi].layout.len() {
                        let lh = self.layouts[pi]
                            .layout
                            .get(line)
                            .map_or(0.0, |l| l.metrics().line_height);
                        self.push_line(pi, line, y, column);
                        y += lh;
                    }
                    y += p.space_after_pt * PT;
                }
                let rules = &mut self.pages[page].rules;
                let [bt, br, bb, bl] = cell.borders;
                if let Some(r) = bt {
                    rules.push(RuleRect {
                        rect: [
                            cx - r.width / 2.0,
                            top - r.width / 2.0,
                            cw + r.width,
                            r.width,
                        ],
                        color: r.color,
                    });
                }
                if let Some(r) = bb {
                    rules.push(RuleRect {
                        rect: [
                            cx - r.width / 2.0,
                            top + h - r.width / 2.0,
                            cw + r.width,
                            r.width,
                        ],
                        color: r.color,
                    });
                }
                if let Some(r) = bl {
                    rules.push(RuleRect {
                        rect: [cx - r.width / 2.0, top, r.width, h],
                        color: r.color,
                    });
                }
                if let Some(r) = br {
                    rules.push(RuleRect {
                        rect: [cx + cw - r.width / 2.0, top, r.width, h],
                        color: r.color,
                    });
                }
            }
            self.y = top + h;
        }
    }
}

impl PagedDocument {
    /// Flows already broken body paragraphs onto pages.
    pub fn paginate(paras: &[Paragraph], layouts: Vec<Arc<ParaLayout>>, setup: PageSetup) -> Self {
        let blocks: Vec<Block> = (0..paras.len()).map(Block::Para).collect();
        Self::paginate_blocks(paras, layouts, &blocks, setup)
    }

    /// Flows paragraphs and tables onto pages. Every paragraph must appear
    /// once in `blocks` (directly or in a table cell), in order.
    pub fn paginate_blocks(
        paras: &[Paragraph],
        layouts: Vec<Arc<ParaLayout>>,
        blocks: &[Block],
        setup: PageSetup,
    ) -> Self {
        let one = [SectionFlow {
            first_block: 0,
            setup,
            first_setup: None,
            even_setup: None,
            start: SectionBreak::NextPage,
            restart: None,
        }];
        Self::paginate_sections(paras, layouts, blocks, &one)
    }

    /// [`Self::paginate_blocks`] with sections, each with its own page
    /// geometry and break (`sections` in block order, the first starting
    /// at block 0; at least one).
    pub fn paginate_sections(
        paras: &[Paragraph],
        layouts: Vec<Arc<ParaLayout>>,
        blocks: &[Block],
        sections: &[SectionFlow],
    ) -> Self {
        let fallback = [SectionFlow {
            first_block: 0,
            setup: PageSetup::a4(),
            first_setup: None,
            even_setup: None,
            start: SectionBreak::NextPage,
            restart: None,
        }];
        let sections = if sections.is_empty() {
            &fallback[..]
        } else {
            sections
        };
        let mut flow = Flow {
            setup: sections[0].setup,
            paras,
            layouts: &layouts,
            pages: Vec::new(),
            line_pages: vec![Vec::new(); layouts.len()],
            y: 0.0,
            sections,
            section: 0,
            section_pages: 0,
            number: sections[0].restart.unwrap_or(1),
        };
        flow.new_page();
        let mut next = 1;
        for (bi, block) in blocks.iter().enumerate() {
            while next < sections.len() && sections[next].first_block <= bi {
                flow.start_section(next);
                next += 1;
            }
            match block {
                Block::Para(pi) => flow.place_para(*pi),
                Block::Table(t) => {
                    flow.place_table(t);
                }
            }
        }
        // Sections with no blocks still get their page.
        while next < sections.len() {
            flow.start_section(next);
            next += 1;
        }
        let Flow {
            pages, line_pages, ..
        } = flow;
        Self {
            setup: pages.first().map_or(sections[0].setup, |p| p.setup),
            layouts,
            pages,
            line_pages,
        }
    }

    /// Size and margins of page `page`.
    pub fn page_setup(&self, page: usize) -> PageSetup {
        self.pages.get(page).map_or(self.setup, |p| p.setup)
    }

    pub fn line_count(&self) -> usize {
        self.layouts.iter().map(|l| l.len()).sum()
    }

    fn origin(&self, para: usize, line: usize) -> Option<(usize, f32, f32)> {
        self.line_pages.get(para)?.get(line).copied()
    }

    /// The text a glyph run's clusters index: the paragraph's own text for
    /// [`RunSource::Text`] (`para_text`), else the marker or label.
    pub fn run_text<'a>(&'a self, run: &GlyphRun, para_text: &'a str) -> &'a str {
        let l = &self.layouts[run.para];
        match run.source {
            RunSource::Text => para_text,
            RunSource::Marker => l.marker.as_ref().map_or("", |m| m.2.as_str()),
            RunSource::Label(i) => l
                .labels
                .iter()
                .find(|x| x.0 == i)
                .map_or("", |x| x.2.as_str()),
        }
    }

    /// Positioned glyph runs on one page, in page coordinates.
    pub fn glyph_runs(&self, page: usize) -> Vec<GlyphRun> {
        let mut out = Vec::new();
        for pl in &self.pages[page].lines {
            let pl_layout = &self.layouts[pl.para];
            let Some((_, dy, dx)) = self.origin(pl.para, pl.line) else {
                continue;
            };
            let line = pl_layout.layout.get(pl.line).expect("placed line exists");
            emit_runs(&mut out, &line, pl.para, RunSource::Text, dx, dy);
            if pl.line == 0
                && let Some((ml, mx, _)) = &pl_layout.marker
                && let Some(mline) = ml.get(0)
            {
                // Marker baseline on the first line's baseline.
                let base = line.metrics().baseline + dy;
                let my = base - mline.metrics().baseline;
                let mdx = dx - pl_layout.dx + mx;
                emit_runs(&mut out, &mline, pl.para, RunSource::Marker, mdx, my);
            }
            for item in line.items() {
                if let PositionedLayoutItem::InlineBox(b) = item
                    && let Some(&(index, ObjectKind::Placeholder)) =
                        pl_layout.objects.get(b.id as usize)
                    && let Some((_, ll, _)) = pl_layout.labels.iter().find(|x| x.0 == index)
                    && let Some(lline) = ll.get(0)
                {
                    let lx = dx + b.x + LABEL_PAD;
                    let ly = dy + b.y + (b.height - lline.metrics().line_height) / 2.0;
                    emit_runs(&mut out, &lline, pl.para, RunSource::Label(index), lx, ly);
                }
            }
        }
        out
    }

    /// Object boxes on a page.
    pub fn objects(&self, page: usize) -> Vec<PlacedObject> {
        let mut out = Vec::new();
        for pl in &self.pages[page].lines {
            let l = &self.layouts[pl.para];
            if l.objects.is_empty() {
                continue;
            }
            let Some((_, dy, dx)) = self.origin(pl.para, pl.line) else {
                continue;
            };
            let Some(line) = l.layout.get(pl.line) else {
                continue;
            };
            for item in line.items() {
                if let PositionedLayoutItem::InlineBox(b) = item
                    && let Some(&(index, kind)) = l.objects.get(b.id as usize)
                    && kind != ObjectKind::Hidden
                {
                    out.push(PlacedObject {
                        para: pl.para,
                        index,
                        kind,
                        rect: [dx + b.x, dy + b.y, b.width, b.height],
                    });
                }
            }
        }
        out
    }

    /// The line on `page` nearest to a point: lines whose column contains
    /// `x` first, then by vertical distance.
    fn pick_line(&self, page: usize, x: f32, y: f32) -> Option<usize> {
        let lines = &self.pages.get(page)?.lines;
        let dist = |a: f32, lo: f32, hi: f32| {
            if a < lo {
                lo - a
            } else if a > hi {
                a - hi
            } else {
                0.0
            }
        };
        lines
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let ka = (
                    dist(x, a.x, a.x + a.width) > 0.5,
                    dist(y, a.top, a.top + a.height),
                    dist(x, a.x, a.x + a.width),
                );
                let kb = (
                    dist(x, b.x, b.x + b.width) > 0.5,
                    dist(y, b.top, b.top + b.height),
                    dist(x, b.x, b.x + b.width),
                );
                ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)
    }

    /// Maps a point on a page to `(paragraph, byte index)`.
    pub fn hit_test(&self, page: usize, x: f32, y: f32) -> Option<(usize, usize)> {
        let i = self.pick_line(page, x, y)?;
        self.hit_placed(page, i, x)
    }

    /// Hit-test at `x` on line `i` of `page` (a click in the margin hits the
    /// nearest end of that line).
    fn hit_placed(&self, page: usize, i: usize, x: f32) -> Option<(usize, usize)> {
        let pl = *self.pages.get(page)?.lines.get(i)?;
        let (_, dy, dx) = self.origin(pl.para, pl.line)?;
        let layout = &self.layouts[pl.para].layout;
        let m = *layout.get(pl.line)?.metrics();
        let ly = ((m.block_min_coord + m.block_max_coord) / 2.0).max(pl.top - dy);
        let cursor = Cursor::from_point(layout, x - dx, ly);
        Some((pl.para, cursor.index()))
    }

    /// The layout line (index into `layouts[para].lines()`) showing `index`.
    fn layout_line_of(&self, para: usize, index: usize) -> Option<usize> {
        let layout = &self.layouts.get(para)?.layout;
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
        let layout = &self.layouts.get(para)?.layout;
        let cursor = Cursor::from_byte_index(layout, index, Affinity::Downstream);
        let r = cursor.geometry(layout, 1.0);
        let line = self.layout_line_of(para, index)?;
        let (page, dy, dx) = self.origin(para, line)?;
        Some(CaretRect {
            page,
            x: dx + r.x0 as f32,
            top: r.y0 as f32 + dy,
            height: (r.y1 - r.y0) as f32,
        })
    }

    /// `(page, index into that page's lines)` of the line showing `index` of `para`.
    pub fn placed_line(&self, para: usize, index: usize) -> Option<(usize, usize)> {
        let k = self.layout_line_of(para, index)?;
        let (page, _, _) = self.origin(para, k)?;
        let i = self.pages[page]
            .lines
            .iter()
            .position(|l| l.para == para && l.line == k)?;
        Some((page, i))
    }

    /// The position on the line above (`down == false`) or below the caret
    /// at `para`/`index`, nearest to `x` (page coordinates): the next line
    /// in the same column (table cells stack vertically). Crosses pages;
    /// `None` at the first / last line of the document.
    pub fn vertical(
        &self,
        para: usize,
        index: usize,
        x: f32,
        down: bool,
    ) -> Option<(usize, usize)> {
        let (mut page, i) = self.placed_line(para, index)?;
        let cur = self.pages[page].lines[i];
        let in_col = |l: &PlacedLine| x >= l.x - 0.5 && x <= l.x + l.width + 0.5;
        // Same page first.
        let pick = |lines: &[PlacedLine], from: Option<f32>| -> Option<usize> {
            let mut best: Option<(usize, f32)> = None;
            for (k, l) in lines.iter().enumerate() {
                let ok = match from {
                    Some(y) if down => l.top >= y - 0.5,
                    Some(y) => l.top + l.height <= y + 0.5,
                    None => true,
                };
                if !ok || (l.para == cur.para && l.line == cur.line && from.is_some()) {
                    continue;
                }
                // Prefer the column under x; else any line.
                let score = if down { l.top } else { -l.top } + if in_col(l) { 0.0 } else { 1e6 };
                if best.is_none_or(|(_, s)| score < s) {
                    best = Some((k, score));
                }
            }
            best.map(|(k, _)| k)
        };
        let from = if down { cur.top + cur.height } else { cur.top };
        if let Some(k) = pick(&self.pages[page].lines, Some(from)) {
            let l = self.pages[page].lines[k];
            // Only accept an out-of-column line if no in-column one exists on
            // this page; otherwise move pages.
            if in_col(&l)
                || !self.pages[page]
                    .lines
                    .iter()
                    .any(|m| in_col(m) && (if down { m.top > from } else { m.top < from }))
            {
                return self.hit_placed(page, k, x);
            }
        }
        loop {
            if down {
                page += 1;
                if page >= self.pages.len() {
                    return None;
                }
            } else {
                page = page.checked_sub(1)?;
            }
            if let Some(k) = pick(&self.pages[page].lines, None) {
                return self.hit_placed(page, k, x);
            }
        }
    }

    /// Byte range of the visual line showing `index` of `para`, and whether
    /// that line ends softly (wrapped, so its range includes the trailing
    /// space the line broke at).
    pub fn line_range(&self, para: usize, index: usize) -> Option<(Range<usize>, bool)> {
        let k = self.layout_line_of(para, index)?;
        let layout = &self.layouts[para].layout;
        let line = layout.get(k)?;
        let soft = k + 1 < layout.len() && line.break_reason() != BreakReason::Explicit;
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
        for para in start.0..=end.0.min(self.layouts.len().saturating_sub(1)) {
            let layout = &self.layouts[para].layout;
            let len = layout.lines().last().map_or(0, |l| l.text_range().end);
            let a = if para == start.0 { start.1 } else { 0 };
            let b = if para == end.0 { end.1 } else { len };
            let sel = parley::Selection::new(
                Cursor::from_byte_index(layout, a, Affinity::Downstream),
                Cursor::from_byte_index(layout, b, Affinity::Downstream),
            );
            for (bb, line) in sel.geometry(layout) {
                if let Some((page, dy, dx)) = self.origin(para, line) {
                    out.push((
                        page,
                        [
                            dx + bb.x0 as f32,
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
        self.pages.iter().map(|p| p.setup.height).sum::<f32>() + (n - 1.0).max(0.0) * gap
    }
}

/// Glyph runs of one layout line, offset to page coordinates.
fn emit_runs(
    out: &mut Vec<GlyphRun>,
    line: &parley::Line<'_, Rgba>,
    para: usize,
    source: RunSource,
    x0: f32,
    dy: f32,
) {
    // Per-glyph cluster ranges of the current run, in visual order; a run's
    // glyph items consume them in sequence.
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
            para,
            text_range: run.text_range(),
            source,
        });
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
mod section_tests {
    use super::*;

    #[test]
    fn sections_start_new_pages_with_their_own_size() {
        let mut engine = Engine::new();
        let paras: Vec<Paragraph> = (0..3).map(|i| Paragraph::new(format!("p{i}"))).collect();
        let portrait = PageSetup::a4();
        let landscape = portrait.landscape();
        let layouts = paras
            .iter()
            .map(|p| Arc::new(engine.layout_paragraph(p, 300.0)))
            .collect();
        let blocks: Vec<Block> = (0..3).map(Block::Para).collect();
        let flow = |first_block, setup, start| SectionFlow {
            first_block,
            setup,
            first_setup: None,
            even_setup: None,
            start,
            restart: None,
        };
        let sections = [
            flow(0, portrait, SectionBreak::NextPage),
            flow(1, landscape, SectionBreak::NextPage),
            SectionFlow {
                restart: Some(7),
                ..flow(2, portrait, SectionBreak::OddPage)
            },
        ];
        let doc = PagedDocument::paginate_sections(&paras, layouts, &blocks, &sections);
        // p0 | p1 (landscape) | p2 on an odd page: 7 is odd, no blank page.
        assert_eq!(doc.pages.len(), 3);
        assert_eq!(doc.page_setup(1), landscape);
        assert_eq!(doc.pages[1].section, 1);
        assert_eq!(doc.pages[2].number, 7);
        assert_eq!(doc.caret(2, 0).unwrap().page, 2);
        let h = doc.total_height(10.0);
        assert!((h - (2.0 * portrait.height + landscape.height + 20.0)).abs() < 0.01);
    }

    #[test]
    fn continuous_sections_share_the_page() {
        let mut engine = Engine::new();
        let paras: Vec<Paragraph> = (0..2).map(|i| Paragraph::new(format!("p{i}"))).collect();
        let layouts = paras
            .iter()
            .map(|p| Arc::new(engine.layout_paragraph(p, 300.0)))
            .collect();
        let blocks: Vec<Block> = (0..2).map(Block::Para).collect();
        let s = PageSetup::a4();
        let sections = [
            SectionFlow {
                first_block: 0,
                setup: s,
                first_setup: None,
                even_setup: None,
                start: SectionBreak::NextPage,
                restart: None,
            },
            SectionFlow {
                first_block: 1,
                setup: s,
                first_setup: None,
                even_setup: None,
                start: SectionBreak::Continuous,
                restart: None,
            },
        ];
        let doc = PagedDocument::paginate_sections(&paras, layouts, &blocks, &sections);
        assert_eq!(doc.pages.len(), 1);
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

    /// Pages for `n` one-word paragraphs (never wrap, whatever the font).
    fn short_lines(setup: PageSetup, n: usize) -> PagedDocument {
        let mut engine = Engine::new();
        let paras: Vec<Paragraph> = (0..n).map(|_| Paragraph::new("ok")).collect();
        let doc = engine.layout_document(&paras, setup);
        assert_eq!(doc.setup, setup);
        for page in &doc.pages {
            for l in &page.lines {
                assert!(l.top >= setup.margin_top - 0.01);
                assert!(l.top + l.height <= setup.height - setup.margin_bottom + 0.01);
            }
        }
        doc
    }

    #[test]
    fn repaginates_for_landscape_and_letter() {
        let a4 = short_lines(PageSetup::a4(), 150);
        let land = short_lines(PageSetup::a4().landscape(), 150);
        let letter = short_lines(PageSetup::letter(), 150);
        let per_page = |d: &PagedDocument| d.pages[0].lines.len();
        assert!(a4.pages.len() >= 2);
        // Landscape pages are shorter: fewer lines each, more pages.
        assert!(per_page(&land) < per_page(&a4));
        assert!(land.pages.len() > a4.pages.len());
        // Letter is 18 mm shorter than A4.
        assert!(per_page(&letter) < per_page(&a4));
        assert!(letter.pages.len() >= a4.pages.len());
        let l = PageSetup::a4().landscape();
        assert!(l.width > l.height);
        assert_eq!(l.landscape(), l);
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
            for (page, _, _) in &doc.line_pages[pi] {
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
        let layout = &doc.layouts[last.para].layout;
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

    #[test]
    fn indents_and_list_markers() {
        let mut engine = Engine::new();
        let mut p = Paragraph::new(
            "Item text that is long enough to wrap onto a second line: it goes on and on about maintenance schedules, spare parts, vibration readings and the weekly report for the plant.",
        );
        p.indent_left_pt = 36.0;
        p.first_line_pt = -18.0;
        p.marker = Some(Marker {
            text: "1.".into(),
            style: Span::new(0..0),
        });
        let doc = engine.layout_document(&[p], PageSetup::a4());
        let x0 = PageSetup::a4().margin_left;
        let first = doc.caret(0, 0).unwrap();
        assert!(
            (first.x - (x0 + 48.0)).abs() < 0.5,
            "text at the left indent: {first:?}"
        );
        let runs = doc.glyph_runs(0);
        let marker = runs
            .iter()
            .find(|r| r.source == RunSource::Marker)
            .expect("marker run");
        assert!(
            (marker.origin_x - (x0 + 24.0)).abs() < 0.5,
            "marker at the hanging edge"
        );
        assert_eq!(doc.run_text(marker, ""), "1.");
        let text = runs.iter().find(|r| r.source == RunSource::Text).unwrap();
        assert!((marker.baseline - text.baseline).abs() < 0.01);
        // Wrapped lines start at the left indent too.
        assert!(doc.layouts[0].len() >= 2);
        let second = doc.layouts[0].layout.get(1).unwrap().text_range().start;
        assert!((doc.caret(0, second).unwrap().x - first.x).abs() < 0.5);
    }

    #[test]
    fn hanging_indent_without_marker() {
        let mut engine = Engine::new();
        let mut p = Paragraph::new(
            "Hanging paragraph with enough words to wrap at least once: it goes on and on about maintenance schedules, spare parts, vibration readings and the weekly report for the plant.",
        );
        p.indent_left_pt = 36.0;
        p.first_line_pt = -36.0;
        let doc = engine.layout_document(&[p], PageSetup::a4());
        let x0 = PageSetup::a4().margin_left;
        assert!((doc.caret(0, 0).unwrap().x - x0).abs() < 0.5);
        let second = doc.layouts[0].layout.get(1).unwrap().text_range().start;
        assert!((doc.caret(0, second).unwrap().x - (x0 + 48.0)).abs() < 0.5);
    }

    #[test]
    fn objects_get_boxes_and_carets_around_them() {
        let mut engine = Engine::new();
        let text = format!("ab{OBJECT}cd{OBJECT}");
        let mut p = Paragraph::new(text.clone());
        p.objects = vec![
            InlineObject {
                index: 2,
                kind: ObjectKind::Image,
                width: 100.0,
                height: 40.0,
                label: String::new(),
            },
            InlineObject {
                index: 2 + OBJECT.len_utf8() + 2,
                kind: ObjectKind::Placeholder,
                width: 0.0,
                height: 0.0,
                label: "Floating image".into(),
            },
        ];
        let doc = engine.layout_document(&[p], PageSetup::a4());
        let objs = doc.objects(0);
        assert_eq!(objs.len(), 2);
        let img = objs[0];
        assert_eq!((img.index, img.rect[2], img.rect[3]), (2, 100.0, 40.0));
        let before = doc.caret(0, 2).unwrap();
        let after = doc.caret(0, 2 + OBJECT.len_utf8()).unwrap();
        assert!(
            before.x <= img.rect[0] + 0.5,
            "caret before the box: {before:?} {img:?}"
        );
        assert!(
            after.x >= img.rect[0] + img.rect[2] - 0.5,
            "caret after the box: {after:?}"
        );
        // The line grew to hold the picture.
        assert!(doc.pages[0].lines[0].height >= 40.0);
        let label = doc
            .glyph_runs(0)
            .into_iter()
            .find(|r| matches!(r.source, RunSource::Label(_)))
            .expect("placeholder label");
        assert_eq!(doc.run_text(&label, &text), "Floating image");
        assert!(objs[1].rect[2] > 40.0, "placeholder sized to its label");
        let end = doc.caret(0, text.len()).unwrap();
        assert!(
            end.x >= objs[1].rect[0] + objs[1].rect[2] - 0.5,
            "end caret after the last box"
        );
    }

    #[test]
    fn page_breaks_start_new_pages() {
        let mut engine = Engine::new();
        let mut b = Paragraph::new("x");
        b.page_break_before = true;
        let paras = vec![Paragraph::new(format!("a{PAGE_BREAK}b")), b];
        let doc = engine.layout_document(&paras, PageSetup::a4());
        assert_eq!(doc.pages.len(), 3);
        assert_eq!(doc.caret(0, 0).unwrap().page, 0);
        assert_eq!(doc.caret(0, 2).unwrap().page, 1);
        assert_eq!(doc.caret(1, 0).unwrap().page, 2);
    }

    fn table(paras: &[Paragraph], setup: PageSetup, rows: usize) -> (Vec<Block>, Vec<f32>) {
        let cols = vec![200.0, 200.0];
        let rule = Some(Rule {
            width: 1.0,
            color: Rgba::BLACK,
        });
        let t = TableBlock {
            x: 0.0,
            columns: cols.clone(),
            padding: [0.0, 7.2, 0.0, 7.2],
            rows: (0..rows)
                .map(|r| TableRow {
                    cells: (0..2)
                        .map(|c| TableCell {
                            col: c,
                            span: 1,
                            paras: 1 + r * 2 + c..2 + r * 2 + c,
                            merge: Merge::None,
                            borders: [rule; 4],
                        })
                        .collect(),
                    min_height: 0.0,
                    exact: false,
                })
                .collect(),
        };
        let _ = (paras, setup);
        let widths = vec![t.text_width(&t.rows[0].cells[0]); 1];
        (
            vec![Block::Para(0), Block::Table(t), Block::Para(1 + rows * 2)],
            widths,
        )
    }

    #[test]
    fn tables_place_cells_side_by_side_and_paginate_rows() {
        let mut engine = Engine::new();
        let setup = PageSetup::a4();
        let rows = 40;
        let mut paras = vec![Paragraph::new("before")];
        for r in 0..rows {
            // Short enough never to wrap in a 200 px cell, whatever font CI has.
            paras.push(Paragraph::new(format!("r{r} left")));
            paras.push(Paragraph::new(format!("r{r} right")));
        }
        paras.push(Paragraph::new("after"));
        let (blocks, widths) = table(&paras, setup, rows);
        let layouts: Vec<_> = paras
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let w = if i == 0 || i == paras.len() - 1 {
                    setup.content_width()
                } else {
                    widths[0]
                };
                Arc::new(engine.layout_paragraph(p, w))
            })
            .collect();
        let doc = PagedDocument::paginate_blocks(&paras, layouts, &blocks, setup);
        assert!(doc.pages.len() >= 2, "rows flow onto a second page");
        let l = doc.caret(1, 0).unwrap();
        let r = doc.caret(2, 0).unwrap();
        assert!((l.top - r.top).abs() < 0.01, "same row, same top");
        assert!(
            (r.x - l.x - 200.0).abs() < 0.5,
            "second column 200 px right"
        );
        assert!(!doc.pages[0].rules.is_empty(), "borders drawn");
        // Hit-testing picks the cell under the pointer.
        assert_eq!(
            doc.hit_test(0, r.x + 2.0, r.top + 2.0).map(|h| h.0),
            Some(2)
        );
        assert_eq!(
            doc.hit_test(0, l.x + 2.0, l.top + 2.0).map(|h| h.0),
            Some(1)
        );
        // Down from a left cell goes to the left cell of the next row.
        assert_eq!(doc.vertical(1, 0, l.x + 2.0, true).map(|h| h.0), Some(3));
        assert_eq!(doc.vertical(3, 0, l.x + 2.0, false).map(|h| h.0), Some(1));
        // No row is split: both cells of every row are on the same page.
        for row in 0..rows {
            let a = doc.caret(1 + row * 2, 0).unwrap().page;
            let b = doc.caret(2 + row * 2, 0).unwrap().page;
            assert_eq!(a, b);
        }
        let after = doc.caret(paras.len() - 1, 0).unwrap();
        assert!(after.top > doc.caret(paras.len() - 2, 0).unwrap().top);
    }
}
