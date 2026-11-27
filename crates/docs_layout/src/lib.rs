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

mod float;
pub mod instance;
pub mod perf;

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use float::free_segments;
pub use float::{
    AnchorAt, FloatPos, FloatRefH, FloatRefV, FloatSide, FloatSpec, FloatWrap, PlacedFloat,
    WrapArea, float_rect,
};

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
/// The model's column-break character.
pub const COLUMN_BREAK: char = '\u{000E}';

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
    /// A drawing in line with the text (SmartArt): an empty box here, the
    /// renderer draws the object into it as it draws floating objects.
    Drawing,
    /// A box with a label, for content shown only as a placeholder.
    Placeholder,
    /// Zero size (bookmarks).
    Hidden,
    /// Text drawn small and raised, with no box: a footnote or endnote
    /// number (the label).
    Mark,
    /// The label drawn as text in the paragraph's font, with no box: what
    /// a field with no result shows (`«Name»` for a mail merge field).
    Text,
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
    /// Keep with the next paragraph (Word's `w:keepNext`): when this
    /// paragraph and the first lines of the next don't fit on the page
    /// together, this one starts the next page.
    pub keep_next: bool,
    /// BCP 47 language, used for shaping and line breaking.
    pub locale: Option<String>,
    /// Byte ranges drawn struck through (tracked deletions): see
    /// [`PagedDocument::strike_rules`].
    pub strikes: Vec<Range<usize>>,
    /// Custom tab stops, measured from the column's left edge (px).
    pub tabs: Vec<TabStop>,
    /// Default tab stops every this many px after the custom ones (Word:
    /// `w:defaultTabStop`, 1.27 cm).
    pub default_tab: f32,
    /// What decimal tab stops line up on (Word's `w:decimalSymbol`).
    pub decimal: char,
    /// Footnote references: byte index of the reference and the height
    /// (px) its note takes at the bottom of the page. The note goes on the
    /// page that shows the line holding the reference.
    pub notes: Vec<(usize, f32)>,
    /// Never break lines at the column width (PowerPoint's `wrap="none"`):
    /// each line keeps its natural width, and the alignment places the
    /// lines in the column, overflowing its edges when they're wider.
    pub no_wrap: bool,
    /// Floating objects anchored here (see [`float`]).
    pub floats: Vec<FloatSpec>,
    /// Footnotes that may continue on the next page: where each may break.
    pub note_splits: Vec<NoteSplit>,
    /// The footnotes laid out at other widths, for a section's columns of
    /// unequal widths (Word lays a note out at its column's width).
    pub note_widths: Vec<NoteWidth>,
    /// Byte ranges laid out with no width and drawn in no colour (a
    /// field's result while its code shows, Word's Alt+F9). Offsets stay
    /// those of `text`.
    pub hidden: Vec<Range<usize>>,
}

/// A paragraph's footnotes laid out at another width: their heights and
/// splits, as [`Paragraph::notes`] and [`Paragraph::note_splits`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NoteWidth {
    pub width: f32,
    pub notes: Vec<(usize, f32)>,
    pub splits: Vec<NoteSplit>,
}

/// Where a footnote may break when it's too long for its page.
#[derive(Clone, Debug, PartialEq)]
pub struct NoteSplit {
    /// Byte index of its reference.
    pub index: usize,
    /// Heights from the note's top it may break at (its lines' bottoms),
    /// ascending; the last is its whole height.
    pub breaks: Vec<f32>,
    /// Height of the continuation notice shown under the part that
    /// continues (0: none).
    pub notice: f32,
}

/// The part of a footnote shown on a page: from `from` to `to` (px from
/// the note's top).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NotePart {
    pub para: usize,
    pub index: usize,
    pub from: f32,
    pub to: f32,
    /// It goes on on the next page.
    pub more: bool,
}

/// Vertical room in the text flow that can break across pages (endnotes):
/// `breaks` as in [`NoteSplit`], `before` the space above it.
#[derive(Clone, Debug, PartialEq)]
pub struct GapBlock {
    pub id: usize,
    pub breaks: Vec<f32>,
    pub before: f32,
}

/// The part of a [`GapBlock`] on a page: its top `y` (page coordinates)
/// and the stretch `from`..`to` it shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GapPart {
    pub id: usize,
    /// Left edge of its column (page coordinates).
    pub x: f32,
    pub y: f32,
    pub from: f32,
    pub to: f32,
}

/// How text lines up at a tab stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabAlign {
    Left,
    Center,
    Right,
    /// The first decimal symbol after the tab ([`Paragraph::decimal`])
    /// sits on the stop (none: as Right).
    Decimal,
    /// Not a stop for tabs: a vertical line through the paragraph's lines.
    Bar,
}

/// What fills the space a tab takes (Word's leaders).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Leader {
    #[default]
    None,
    Dot,
    Hyphen,
    Underscore,
    Heavy,
    MiddleDot,
}

/// A tab stop: position from the column's left edge (px), alignment and
/// leader.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabStop {
    pub pos: f32,
    pub align: TabAlign,
    pub leader: Leader,
}

impl TabStop {
    pub fn new(pos: f32, align: TabAlign) -> Self {
        Self {
            pos,
            align,
            leader: Leader::None,
        }
    }

    pub fn with_leader(self, leader: Leader) -> Self {
        Self { leader, ..self }
    }
}

/// How a tab character is drawn: its leader, in the tab's colour and size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabFill {
    pub leader: Leader,
    pub color: Rgba,
    /// Font size of the tab character, px.
    pub size: f32,
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
            keep_next: false,
            locale: None,
            strikes: Vec::new(),
            tabs: Vec::new(),
            default_tab: 12.7 * MM,
            decimal: '.',
            notes: Vec::new(),
            no_wrap: false,
            floats: Vec::new(),
            note_splits: Vec::new(),
            note_widths: Vec::new(),
            hidden: Vec::new(),
        }
    }

    /// Whether byte `at` is in a hidden range.
    pub fn is_hidden_at(&self, at: usize) -> bool {
        self.is_hidden(at)
    }

    fn is_hidden(&self, at: usize) -> bool {
        self.hidden.iter().any(|r| r.contains(&at))
    }

    /// The text as shaped: same byte offsets, layout stand-ins for tabs,
    /// page breaks and objects.
    fn shaped_text(&self) -> String {
        self.text
            .chars()
            .map(|c| match c {
                // Tabs are inline boxes sized to reach their stop; the
                // space standing in for the tab is shrunk to nothing.
                '\t' => " ".to_owned(),
                PAGE_BREAK | COLUMN_BREAK => "\n".to_owned(),
                OBJECT => "\u{200B}".to_owned(),
                c => c.to_string(),
            })
            .collect()
    }
}

/// A shaped paragraph: the text layout plus its marker and placeholder
/// labels.
#[derive(Clone)]
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
    /// Objects by inline-box id: their full height, px. An object taller
    /// than an exact line's room above the baseline gets a box only that
    /// tall (the line keeps its height, as in Word) and shows clipped.
    pub heights: Vec<f32>,
    /// How far its lines' content is moved down in their boxes: an exact
    /// line puts its baseline four fifths of the way down, as Word and
    /// LibreOffice do (parley centres the text instead).
    pub shift: f32,
    /// Per tab character (text order): how its space is filled.
    pub tab_fills: Vec<TabFill>,
    /// Bar tabs: x of each line from the column's left edge, px.
    pub bars: Vec<f32>,
    /// How lines are aligned, and the width they were broken at (from
    /// `dx`): lines broken again around floating objects use them.
    pub alignment: Alignment,
    pub avail: f32,
    /// The column width it was laid out for (a column of another width
    /// breaks its lines again).
    pub width: f32,
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
/// Size of a note mark relative to its paragraph's text.
const MARK_SCALE: f32 = 0.65;
/// Where an exact line's baseline sits, as a share of its height from
/// its top, whatever the font (Word's "exactly" spacing).
///
/// Word's rule, checked 2026-11-19 against public sources (no Word here):
/// - LibreOffice: `nAsc = (4 * nLineHeight) / 5` for
///   `SvxLineSpaceRule::Fix` in `SwTextFormatter::CalcRealHeight`
///   (sw/source/core/text/itrform2.cxx), with no Word-compat branch.
/// - GraphCompose PR #789 (github.com/DemchaAV/GraphCompose/pull/789):
///   measured through Word COM, 0.80 ± 0.1 pt for Spectral, Lato and Arial
///   at 10-46 pt in lines 12-100 pt tall.
/// - office2pdf PR #2272 (github.com/developer0hye/office2pdf/pull/2272):
///   Word-exported PDFs, e.g. Times New Roman 13 pt in 16 / 24 pt lines
///   has its baseline 12.80 / 19.30 pt down (0.8 × = 12.80 / 19.20).
/// - Heliograph PR #63 (github.com/jonnymuir/Heliograph/pull/63): its
///   Word probe puts the baseline at 80% of an exact line.
///
/// ONLYOFFICE (sdkjs word/Editor/Paragraph_Recalculate.js,
/// `CParaLineMetrics.Recalculate_LineGap`, `linerule_Exact`) puts all the
/// extra height above the text instead (baseline = line − descent); the
/// Word measurements above don't bear that out.
const EXACT_ASCENT: f32 = 0.8;
/// How far a note mark's baseline sits above the text's, in ems.
const MARK_RAISE: f32 = 0.08;
/// Gap between a list marker and the text, px.
const MARKER_GAP: f32 = 6.0;

/// Font database plus shaping scratch space. Create once and reuse: building
/// the font context scans the system fonts.
pub struct Engine {
    pub fonts: FontContext,
    layout_cx: LayoutContext<Rgba>,
    /// Family lists naming a style ("Plus Jakarta Sans ExtraBold"), see
    /// [`Engine::style_family`].
    styled: HashMap<String, Option<StyledFamily>>,
}

/// A family name that names a style of another family, the way Windows
/// lists each weight of a variable font (or of a big static family) as a
/// family of its own: "Plus Jakarta Sans ExtraBold" is Plus Jakarta Sans
/// at weight 800.
#[derive(Clone, Debug, PartialEq)]
pub struct StyledFamily {
    /// The family list with the style taken off the first name.
    pub family: String,
    pub weight: f32,
    pub italic: bool,
}

/// `"ExtraBold Italic"` → (800, true); `None` for words that aren't a style.
fn style_words(words: &str) -> Option<(f32, bool)> {
    let mut w = words.to_ascii_lowercase().replace(['-', '_'], " ");
    let mut italic = false;
    for suffix in [" italic", " oblique"] {
        if let Some(rest) = w.strip_suffix(suffix) {
            w = rest.to_owned();
            italic = true;
        }
    }
    if matches!(w.as_str(), "italic" | "oblique") {
        return Some((400.0, true));
    }
    let weight = match w.replace(' ', "").as_str() {
        "thin" | "hairline" => 100.0,
        "extralight" | "ultralight" => 200.0,
        "light" => 300.0,
        "regular" | "normal" | "book" => 400.0,
        "medium" => 500.0,
        "semibold" | "demibold" => 600.0,
        "bold" => 700.0,
        "extrabold" | "ultrabold" => 800.0,
        "black" | "heavy" => 900.0,
        _ => return None,
    };
    Some((weight, italic))
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

/// Push a span's style. `styled` is what its family names (see
/// [`Engine::style_family`]), `base` the paragraph's own.
fn push_span(
    b: &mut parley::RangedBuilder<'_, Rgba>,
    span: &Span,
    r: Range<usize>,
    styled: Option<&StyledFamily>,
    base: Option<&StyledFamily>,
) {
    let style = styled.or(if span.family.is_none() { base } else { None });
    let weight = style.map_or(400.0, |s| s.weight);
    if span.bold {
        // Bold on a style that is already bold or heavier keeps it.
        b.push(
            StyleProperty::FontWeight(FontWeight::new(weight.max(700.0))),
            r.clone(),
        );
    } else if let Some(s) = styled {
        b.push(
            StyleProperty::FontWeight(FontWeight::new(s.weight)),
            r.clone(),
        );
    }
    if span.italic || styled.is_some_and(|s| s.italic) {
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
        let family = styled.map_or(family, |s| s.family.as_str());
        b.push(
            StyleProperty::FontFamily(FontFamily::Source(family.to_owned().into())),
            r,
        );
    }
}

/// The default family (and the style it names, see
/// [`Engine::style_family`]).
fn push_family_default(
    b: &mut parley::RangedBuilder<'_, Rgba>,
    family: &str,
    styled: Option<&StyledFamily>,
) {
    let family = styled.map_or(family, |s| s.family.as_str());
    b.push_default(StyleProperty::FontFamily(FontFamily::Source(
        family.to_owned().into(),
    )));
    if let Some(s) = styled {
        b.push_default(StyleProperty::FontWeight(FontWeight::new(s.weight)));
        if s.italic {
            b.push_default(StyleProperty::FontStyle(FontStyle::Italic));
        }
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
            styled: HashMap::new(),
        }
    }

    /// The family and style a family list's first name stands for when no
    /// family has that name but it is an installed family plus a style
    /// ("Plus Jakarta Sans ExtraBold", "Inter SemiBold Italic"): what
    /// PowerPoint and Word write when a style of a variable font is picked
    /// as a font on Windows. `None` for every other list.
    pub fn style_family(&mut self, list: &str) -> Option<StyledFamily> {
        if let Some(hit) = self.styled.get(list) {
            return hit.clone();
        }
        let found = self.find_styled(list);
        self.styled.insert(list.to_owned(), found.clone());
        found
    }

    fn find_styled(&mut self, list: &str) -> Option<StyledFamily> {
        let (first, rest) = match list.split_once(',') {
            Some((a, b)) => (a, Some(b)),
            None => (list, None),
        };
        let name = first.trim().trim_matches(['"', '\'']);
        let collection = &mut self.fonts.collection;
        if name.is_empty() || collection.family_id(name).is_some() {
            return None;
        }
        // Take one, two or three words off the end as the style.
        let words: Vec<&str> = name.split(' ').filter(|w| !w.is_empty()).collect();
        for n in 1..=3.min(words.len().saturating_sub(1)) {
            let (base, style) = words.split_at(words.len() - n);
            let Some((weight, italic)) = style_words(&style.join(" ")) else {
                continue;
            };
            let base = base.join(" ");
            if collection.family_id(&base).is_some() {
                let family = match rest {
                    Some(r) => format!("\"{base}\",{r}"),
                    None => format!("\"{base}\""),
                };
                return Some(StyledFamily {
                    family,
                    weight,
                    italic,
                });
            }
        }
        None
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
        let base = self.style_family(family);
        let styled = style
            .and_then(|s| s.family.as_deref())
            .and_then(|f| self.style_family(f));
        let mut b = self
            .layout_cx
            .ranged_builder(&mut self.fonts, text, 1.0, true);
        push_family_default(&mut b, family, base.as_ref());
        b.push_default(StyleProperty::FontSize(size_pt * PT));
        b.push_default(StyleProperty::Brush(color));
        if let Some(s) = style {
            push_span(&mut b, s, 0..text.len(), styled.as_ref(), base.as_ref());
        }
        let mut layout = b.build(text);
        layout.break_all_lines(None);
        layout.align(Alignment::Start, AlignmentOptions::default());
        layout
    }

    /// How far an exact line `lh` px high reaches above the baseline of
    /// `para`'s text.
    fn exact_room(&mut self, para: &Paragraph, lh: f32) -> f32 {
        let mut b = self
            .layout_cx
            .ranged_builder(&mut self.fonts, "x", 1.0, true);
        b.push_default(StyleProperty::FontFamily(FontFamily::Source(
            para.family.as_str().into(),
        )));
        b.push_default(StyleProperty::FontSize(para.size_pt * PT));
        b.push_default(StyleProperty::LineHeight(LineHeight::Absolute(lh)));
        let mut layout: Layout<Rgba> = b.build("x");
        layout.break_all_lines(None);
        layout.get(0).map_or(lh, |l| {
            let m = l.metrics();
            (m.baseline - m.block_min_coord).max(0.0)
        })
    }

    /// Shapes one paragraph and breaks it into lines in a column `width` px
    /// wide (indents are taken off that width). Tabs go to the next tab
    /// stop: laid out once with empty tabs to find where each one starts,
    /// then again with each tab as wide as it needs to be.
    pub fn layout_paragraph(&mut self, para: &Paragraph, width: f32) -> ParaLayout {
        let tabs: Vec<usize> = para
            .text
            .char_indices()
            .filter(|(_, c)| *c == '\t')
            .map(|(i, _)| i)
            .collect();
        if tabs.is_empty() {
            return self.layout_with_tabs(para, width, &[], &[]);
        }
        let first = self.layout_with_tabs(para, width, &vec![0.01; tabs.len()], &[]);
        let (plan, line_extra) = tab_widths(para, &first, &tabs, width);
        // Hidden tabs take no room.
        let widths: Vec<f32> = plan
            .iter()
            .zip(&tabs)
            .map(|(p, &at)| if para.is_hidden(at) { 0.0 } else { p.0 })
            .collect();
        let mut laid = self.layout_with_tabs(para, width, &widths, &line_extra);
        laid.tab_fills = tabs
            .iter()
            .zip(&plan)
            .map(|(&at, &(_, leader))| {
                let span = para
                    .spans
                    .iter()
                    .rev()
                    .find(|s| s.range.start <= at && at < s.range.end);
                TabFill {
                    leader,
                    color: span.and_then(|s| s.color).unwrap_or(Rgba::BLACK),
                    size: span.and_then(|s| s.size_pt).unwrap_or(para.size_pt) * PT,
                }
            })
            .collect();
        laid
    }

    /// [`Self::layout_paragraph`] with the tabs (in text order) `tab_widths`
    /// px wide; line `k` may run `line_extra[k]` px into the right margin
    /// (a custom tab stop past it).
    fn layout_with_tabs(
        &mut self,
        para: &Paragraph,
        width: f32,
        tab_widths: &[f32],
        line_extra: &[f32],
    ) -> ParaLayout {
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
        let mut heights = Vec::with_capacity(para.objects.len());
        // An exact line height ("exactly", `w:lineRule="exact"`) is kept
        // whatever the line holds: an object taller than the room above
        // the baseline is cut at the line's top, as Word does.
        let (room, shift) = match para.line_height {
            Some(LineHeight::Absolute(lh)) => {
                let room = self.exact_room(para, lh);
                (Some(room), lh * EXACT_ASCENT - room)
            }
            _ => (None, 0.0),
        };
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
            if o.kind == ObjectKind::Mark {
                let l = self.small_layout(
                    &o.label,
                    &para.family,
                    para.size_pt * MARK_SCALE,
                    None,
                    Rgba::BLACK,
                );
                // Raised: the box's bottom sits on the baseline.
                w = l.width();
                h = l.height() + para.size_pt * PT * MARK_RAISE;
                labels.push((o.index, l, o.label.clone()));
            }
            if o.kind == ObjectKind::Text {
                // In the formatting of the text it sits in (a page number
                // in a footer's font).
                let span = para
                    .spans
                    .iter()
                    .rev()
                    .find(|s| s.range.start <= o.index && o.index < s.range.end)
                    .cloned();
                let family = span
                    .as_ref()
                    .and_then(|s| s.family.clone())
                    .unwrap_or_else(|| para.family.clone());
                let size = span
                    .as_ref()
                    .and_then(|s| s.size_pt)
                    .unwrap_or(para.size_pt);
                let color = span.as_ref().and_then(|s| s.color).unwrap_or(Rgba::BLACK);
                let l = self.small_layout(&o.label, &family, size, span.as_ref(), color);
                // The box's bottom is the label's baseline.
                w = l.width();
                h = l.get(0).map_or(l.height(), |line| line.metrics().baseline);
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
            heights.push(h);
            if at < para.text.len() {
                boxes.push(InlineBox {
                    id: id as u64,
                    kind: InlineBoxKind::InFlow,
                    index: at,
                    width: w.min(width.max(1.0)),
                    height: room.map_or(h, |r| h.min(r)),
                    baseline: None,
                    vertical_align: VerticalAlign::BASELINE,
                });
            }
        }

        // Tabs: boxes after the objects' ids (they aren't objects).
        let tab_at = para.text.char_indices().filter(|(_, c)| *c == '\t');
        for (k, (at, _)) in tab_at.enumerate() {
            boxes.push(InlineBox {
                id: (TAB_BOX + k) as u64,
                kind: InlineBoxKind::InFlow,
                index: at,
                width: tab_widths.get(k).copied().unwrap_or(0.0).max(0.0),
                height: 0.0,
                baseline: None,
                vertical_align: VerticalAlign::BASELINE,
            });
        }

        let text = para.shaped_text();
        let base = self.style_family(&para.family);
        let styled: Vec<Option<StyledFamily>> = para
            .spans
            .iter()
            .map(|s| s.family.as_deref().and_then(|f| self.style_family(f)))
            .collect();
        let mut b = self
            .layout_cx
            .ranged_builder(&mut self.fonts, &text, 1.0, true);
        push_family_default(&mut b, &para.family, base.as_ref());
        b.push_default(StyleProperty::FontSize(para.size_pt * PT));
        b.push_default(StyleProperty::LineHeight(
            para.line_height
                .unwrap_or(LineHeight::FontSizeRelative(para.line_spacing)),
        ));
        b.push_default(StyleProperty::Brush(Rgba::BLACK));
        if let Some(locale) = para.locale.as_deref().and_then(|l| l.parse().ok()) {
            b.push_default(StyleProperty::Locale(Some(locale)));
        }
        for (span, styled) in para.spans.iter().zip(&styled) {
            push_span(
                &mut b,
                span,
                span.range.clone(),
                styled.as_ref(),
                base.as_ref(),
            );
        }
        for (at, _) in para.text.char_indices().filter(|(_, c)| *c == '\t') {
            b.push(StyleProperty::FontSize(0.01), at..at + 1);
        }
        for r in &para.hidden {
            let r = r.start.min(text.len())..r.end.min(text.len());
            b.push(StyleProperty::FontSize(0.01), r.clone());
            b.push(StyleProperty::Brush(Rgba([0, 0, 0, 0])), r.clone());
            b.push(StyleProperty::Underline(false), r);
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
        let max_extra = line_extra.iter().copied().fold(0.0f32, f32::max);
        if para.no_wrap || max_extra <= 0.0 {
            layout.break_all_lines((!para.no_wrap).then_some(avail));
        } else {
            // Modern Word lets a line run into the right margin up to a
            // tab stop set there.
            let mut br = layout.break_lines();
            br.state_mut().set_layout_max_advance(avail + max_extra);
            let mut k = 0usize;
            for _ in 0..100_000 {
                if br.is_done() {
                    break;
                }
                let extra = line_extra.get(k).copied().unwrap_or(0.0);
                br.state_mut().set_line_max_advance(avail + extra);
                match br.break_next() {
                    Some(parley::layout::YieldData::LineBreak(_)) => k += 1,
                    Some(_) => {}
                    None => break,
                }
            }
            br.finish();
        }
        let alignment = match para.align {
            Align::Start => Alignment::Start,
            Align::Center => Alignment::Center,
            Align::End => Alignment::End,
            Align::Justify => Alignment::Justify,
        };
        layout.align(alignment, AlignmentOptions::default());
        // Unbroken lines are aligned against the widest one; that block
        // then goes where the alignment puts it in the column.
        let dx = if para.no_wrap {
            let share = match para.align {
                Align::Center => 0.5,
                Align::End => 1.0,
                Align::Start | Align::Justify => 0.0,
            };
            dx + (avail - layout.width()) * share
        } else {
            dx
        };
        ParaLayout {
            layout,
            dx,
            marker,
            labels,
            objects: para.objects.iter().map(|o| (o.index, o.kind)).collect(),
            heights,
            shift,
            tab_fills: Vec::new(),
            bars: para
                .tabs
                .iter()
                .filter(|t| t.align == TabAlign::Bar)
                .map(|t| t.pos)
                .collect(),
            alignment,
            avail,
            width,
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
    /// Shading drawn under the cell's text (Track Changes' inserted and
    /// deleted cells and rows).
    pub fill: Option<Rgba>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableRow {
    pub cells: Vec<TableCell>,
    /// Minimum (or exact) height in px.
    pub min_height: f32,
    pub exact: bool,
    /// Repeated at the top of each page the table continues on (only
    /// rows at the start of the table: Word's "Repeat as header row").
    pub header: bool,
    /// Kept on one page (Word's `w:cantSplit`): a row that doesn't fit
    /// moves to the next page instead of breaking at a line.
    pub cant_split: bool,
    /// Kept on the next row's page (Word: every paragraph in the row keeps
    /// with next), so a header row doesn't end a page alone.
    pub keep_next: bool,
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
    Gap(GapBlock),
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
    /// The y that maps the layout's coordinates to the page, and the x of
    /// the layout origin.
    pub dy: f32,
    pub dx: f32,
    /// A repeated table header row: drawn, but the caret and clicks use
    /// the row where it really is.
    pub repeat: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub lines: Vec<PlacedLine>,
    /// Footnote references whose notes sit at the bottom of this page:
    /// `(paragraph, byte index)` in document order.
    pub notes: Vec<(usize, usize)>,
    /// Height reserved for them at the bottom of the text area
    /// ([`NOTE_SEPARATOR`] included), px.
    pub notes_height: f32,
    /// Table borders.
    pub rules: Vec<RuleRect>,
    /// This page's size and margins (sections can differ).
    pub setup: PageSetup,
    /// The section the page belongs to.
    pub section: usize,
    /// Its number within the section's page numbering run: 1-based page
    /// count from the last restart (see [`SectionFlow::restart`]).
    pub number: u32,
    /// Floating objects anchored on this page, in document order.
    pub floats: Vec<PlacedFloat>,
    /// The footnotes shown at its bottom (whole, or the part that fits),
    /// in order: a note continued from the page before comes first.
    pub note_parts: Vec<NotePart>,
    /// It starts with a footnote continued from the page before (under
    /// Word's full-width continuation separator).
    pub continued: bool,
    /// Gap blocks (endnotes) placed on it.
    pub gaps: Vec<GapPart>,
    /// Room taken by its wrapping objects whose position doesn't depend on
    /// the text (relative to the page or margins), for a second pass.
    pub fixed: Vec<WrapArea>,
    /// Its stretches of text in columns (sections with more than one):
    /// each one's columns as `[x, y, w, h]`, left to right.
    pub columns: Vec<Vec<[f32; 4]>>,
    /// Its footnotes by column, once the page is laid out (a page without
    /// columns has one, the text area's width).
    pub note_columns: Vec<NoteColumn>,
}

impl Page {
    /// The text column (left edge and width) holding `(x, y)` on this
    /// page, when that part of it has columns.
    pub fn column_at(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let region = self.columns.iter().find(|r| {
            r.first()
                .is_some_and(|c| y >= c[1] - 0.5 && y <= c[1] + c[3] + 0.5)
        })?;
        // The column it's in, else the nearest one.
        region
            .iter()
            .min_by(|a, b| {
                let d = |c: &[f32; 4]| {
                    if x < c[0] {
                        c[0] - x
                    } else {
                        (x - c[0] - c[2]).max(0.0)
                    }
                };
                d(a).total_cmp(&d(b))
            })
            .map(|c| (c[0], c[2]))
    }
}

/// The footnotes at the bottom of one column of a page (Word puts a
/// column's notes under it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NoteColumn {
    /// The column's left edge and width.
    pub x: f32,
    pub width: f32,
    /// Where the notes end (the bottom of the text area, above any notes
    /// already there), and their height with the separator.
    pub bottom: f32,
    pub height: f32,
    pub parts: Vec<NotePart>,
    /// It starts with a note continued from the column or page before
    /// (under a full-width continuation separator).
    pub continued: bool,
}

/// How a section starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SectionBreak {
    #[default]
    NextPage,
    Continuous,
    /// At the top of the next column (the next page after the last).
    NextColumn,
    EvenPage,
    OddPage,
}

/// A section for [`PagedDocument::paginate_sections`]: where it starts and
/// its pages' geometry.
#[derive(Clone, Debug, PartialEq)]
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
    /// Its text columns: each one's left edge (from the left margin) and
    /// width, px. Empty or one: the whole text width.
    pub columns: Vec<(f32, f32)>,
    /// A line between the columns.
    pub sep: bool,
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
    /// The number of the note mark at this byte index (document text, in
    /// the PDF too).
    Mark(usize),
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
    /// The same instance as user-space axis values (`wght` 700, …): what
    /// [`instance::instance`] takes to make a static font for renderers.
    pub variations: Vec<([u8; 4], f32)>,
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

/// The axis values fontique chose for a run (bold → `wght` 700, …).
pub fn variations(synthesis: &parley::fontique::Synthesis) -> Vec<([u8; 4], f32)> {
    synthesis
        .variation_settings()
        .iter()
        .map(|(t, v)| (t.to_be_bytes(), *v))
        .collect()
}

/// An object's box on a page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedObject {
    pub para: usize,
    /// Byte offset of the object character.
    pub index: usize,
    pub kind: ObjectKind,
    /// `[x, y, w, h]` in page coordinates: what shows.
    pub rect: [f32; 4],
    /// Where the whole object lands: taller than `rect` when an exact
    /// line height cuts its top off (see [`ParaLayout::heights`]).
    pub whole: [f32; 4],
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
    /// Per page: where it starts in a paragraph and the flow's state
    /// there, when a later pagination could take the rest from this one
    /// (see [`Reuse`]).
    starts: Vec<Option<PageStart>>,
    /// The pages from `.0` on are those of an earlier pagination from its
    /// page `.1` on ([`Reuse`]).
    reused: Option<(usize, usize)>,
}

/// A page that starts by going on with a paragraph, and the flow's state
/// there: what decides how the rest of the document is paginated.
#[derive(Clone, Debug, PartialEq)]
struct PageStart {
    block: usize,
    para: usize,
    line: usize,
    section: usize,
    number: u32,
    setup: PageSetup,
    floats_placed: Option<usize>,
}

/// An earlier pagination of the same document before an edit, to take
/// pages from: once the new pagination starts a page where the old one
/// started one (the same line of the same paragraph, in the same state),
/// the rest is the old pages with their paragraphs renumbered. Blocks from
/// `from_block` on (new numbering) must be the old ones from
/// `from_block - block_shift` on, their paragraphs shifted by
/// `para_shift`, and laid out the same.
pub struct Reuse<'a> {
    pub old: &'a PagedDocument,
    pub from_block: usize,
    pub block_shift: isize,
    pub para_shift: isize,
    /// Page numbers may differ from the old ones there (nothing depends on
    /// whether a page is odd or even); the old pages are renumbered.
    pub renumber: bool,
}

/// `index` hit on line `k` of `layout`, kept on that line. A hit past the
/// end of a line that isn't the paragraph's last gives the line's end,
/// which is also where the next line starts, and carets (downstream) are
/// drawn there: moving Up from the right of a shorter wrapped line, or
/// clicking right of it, left the caret on the line below. Like Word's End
/// key, such a hit goes before the line's trailing spaces or break (or
/// before its last character when the line wrapped inside a word).
fn on_line_end(layout: &Layout<Rgba>, k: usize, index: usize) -> usize {
    let Some(line) = layout.get(k) else {
        return index;
    };
    let range = line.text_range();
    if k + 1 >= layout.len() || index < range.end {
        return index;
    }
    let mut clusters: Vec<(Range<usize>, bool)> = line
        .runs()
        .flat_map(|r| r.clusters().collect::<Vec<_>>())
        .map(|c| {
            let blank = c.is_space_or_nbsp() || c.is_soft_line_break() || c.is_hard_line_break();
            (c.text_range(), blank)
        })
        .filter(|(r, _)| !r.is_empty())
        .collect();
    clusters.sort_by_key(|(r, _)| r.start);
    let end = clusters
        .iter()
        .rev()
        .find(|(_, blank)| !blank)
        .map_or(range.start, |(r, _)| r.end);
    if end < range.end {
        return end.max(range.start);
    }
    clusters
        .last()
        .map_or(range.start, |(r, _)| r.start)
        .max(range.start)
}

/// Inline box ids of tabs start here (object boxes use their index).
const TAB_BOX: usize = 1 << 20;

/// How wide each tab must be to reach its stop, and its stop's leader,
/// from a layout with empty tabs (`tabs` are their byte offsets), as Word:
/// the next custom stop right of the tab (bar tabs aren't stops), the
/// left indent of a hanging first line when that comes first, else the
/// next default stop. A custom stop past the right indent is honoured, the
/// line running into the margin (Word 2013 and later); default stops stop
/// at the right indent. Also returns, per line of `laid`, how far it may
/// run past the right indent.
fn tab_widths(
    para: &Paragraph,
    laid: &ParaLayout,
    tabs: &[usize],
    width: f32,
) -> (Vec<(f32, Leader)>, Vec<f32>) {
    let mut out = vec![(0.0f32, Leader::None); tabs.len()];
    let mut line_extra = Vec::new();
    let left = para.indent_left_pt * PT;
    let right_edge = (width - para.indent_right_pt * PT).max(left + 8.0);
    let default = if para.default_tab > 1.0 {
        para.default_tab
    } else {
        12.7 * MM
    };
    let mut stops: Vec<TabStop> = para
        .tabs
        .iter()
        .filter(|t| t.align != TabAlign::Bar)
        .copied()
        .collect();
    stops.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    let min_w = para.size_pt * PT * 0.25;
    for line in laid.layout.lines() {
        line_extra.push(0.0f32);
        // Tab boxes on this line, in order, with their x (column coords).
        let mut on_line: Vec<(usize, f32)> = Vec::new();
        let mut line_end_x = 0.0f32;
        for item in line.items() {
            match item {
                PositionedLayoutItem::InlineBox(b) if b.id as usize >= TAB_BOX => {
                    on_line.push((b.id as usize - TAB_BOX, laid.dx + b.x));
                }
                PositionedLayoutItem::InlineBox(b) => {
                    line_end_x = line_end_x.max(laid.dx + b.x + b.width);
                }
                PositionedLayoutItem::GlyphRun(g) => {
                    line_end_x = line_end_x.max(laid.dx + g.offset() + g.advance());
                }
            }
        }
        let line_range = line.text_range();
        // Each tab moves what follows it; positions after an earlier tab
        // shift by that tab's width.
        let mut shift = 0.0f32;
        for (j, &(k, x0)) in on_line.iter().enumerate() {
            let x = x0 + shift;
            // The text after this tab up to the next tab or the line's end.
            let seg_end = on_line.get(j + 1).map_or(line_end_x, |n| n.1);
            let seg = (seg_end - x0).max(0.0);
            let custom = stops.iter().find(|t| t.pos > x + 0.5).copied();
            // A hanging indent counts as a stop (Word's list tab).
            let implicit = (left > x + 0.5).then_some(TabStop::new(left, TabAlign::Left));
            let (stop, is_custom) = match (custom, implicit) {
                (Some(c), Some(i)) if i.pos < c.pos => (i, false),
                (Some(c), _) => (c, true),
                (None, Some(i)) => (i, false),
                (None, None) => (
                    TabStop::new(((x / default).floor() + 1.0) * default, TabAlign::Left),
                    false,
                ),
            };
            // A custom stop in the margin keeps its place (the text after
            // a left one may run on to it); never past the page's width
            // again from the indent.
            let edge = if is_custom && stop.pos > right_edge {
                stop.pos.min(right_edge + width)
            } else {
                right_edge
            };
            let pos = stop.pos.min(edge);
            let w = match stop.align {
                TabAlign::Left | TabAlign::Bar => pos - x,
                TabAlign::Center => pos - x - seg / 2.0,
                TabAlign::Right => pos - x - seg,
                TabAlign::Decimal => {
                    // Up to the first '.' of the segment, if on this line.
                    let start = tabs.get(k).map_or(0, |t| t + 1);
                    let end = tabs.get(k + 1).copied().unwrap_or(para.text.len());
                    let dot = para.text.get(start..end).and_then(|t| t.find(para.decimal));
                    match dot.map(|d| start + d) {
                        Some(d) if d < line_range.end => {
                            let c = Cursor::from_byte_index(&laid.layout, d, Affinity::Downstream);
                            let dx = laid.dx + c.geometry(&laid.layout, 1.0).x0 as f32;
                            pos - x - (dx - x0).clamp(0.0, seg)
                        }
                        _ => pos - x - seg,
                    }
                }
            };
            // Past the stop already: at least a space's worth; never past
            // the right indent (a little slack keeps the line from wrapping).
            let aligned_end = stop.align != TabAlign::Left;
            let room = edge - 0.05 - x - if aligned_end { seg } else { 0.0 };
            let w = w.min(room).max(min_w).min(edge.max(1.0));
            if let Some(slot) = out.get_mut(k) {
                *slot = (w, stop.leader);
            }
            if edge > right_edge
                && let Some(e) = line_extra.last_mut()
            {
                // Room for the stop and, after a left stop, the text that
                // follows it.
                let after = if aligned_end { 0.0 } else { seg };
                *e = e.max(edge - right_edge + after);
            }
            shift += w;
        }
    }
    (out, line_extra)
}

/// Space above a page's footnotes, holding the short separator line, px.
pub const NOTE_SEPARATOR: f32 = 18.0;

/// Room left for text on a page that starts with a continued footnote, px.
const CONTINUED_TEXT: f32 = 60.0;

/// Lines a paragraph keeps together at a page break (Word's widow/orphan control).
const MIN_LINES: usize = 2;

/// Lays blocks out onto pages.
struct Flow<'a> {
    /// The current page's geometry.
    setup: PageSetup,
    paras: &'a [Paragraph],
    layouts: Vec<Arc<ParaLayout>>,
    /// Room the current page's wrapping objects take from the text.
    excl: Vec<WrapArea>,
    /// Per page, from a first pass: wrapping objects fixed to the page,
    /// known before the text above their anchor is placed.
    pre: &'a [Vec<WrapArea>],
    /// Some paragraph has floating objects.
    has_floats: bool,
    /// The paragraph whose floating objects are already placed.
    floats_placed: Option<usize>,
    /// Footnotes going on on the next page: paragraph, reference, the
    /// height shown so far.
    carry: Vec<(usize, usize, f32)>,
    pages: Vec<Page>,
    line_pages: Vec<Vec<(usize, f32, f32)>>,
    y: f32,
    sections: &'a [SectionFlow],
    section: usize,
    /// Pages of the current section so far.
    section_pages: usize,
    /// Number of the next page.
    number: u32,
    /// The current stretch of columns: the column being filled, the top
    /// of the columns on this page, and the lowest any column reached.
    col: usize,
    col_top: f32,
    col_max: f32,
    /// The columns of that stretch (empty: one, the text width).
    cols: Vec<(f32, f32)>,
    sep: bool,
    /// Height of the footnotes at the page bottom from before the stretch
    /// of columns began (its columns' notes sit above them).
    notes_base: f32,
    /// Balancing columns: on page `.0`, the text goes no lower than `.1`.
    limit: Option<(usize, f32)>,
    /// The layouts as given (balancing restores them).
    orig: Vec<Arc<ParaLayout>>,
    /// Where balancing may start again from (block boundaries).
    snaps: Vec<Snap>,
    /// The block being placed.
    block: usize,
    /// The paragraph and line the page about to start goes on with.
    cont: Option<(usize, usize)>,
    /// Per page: see [`PagedDocument::starts`].
    starts: Vec<Option<PageStart>>,
    /// An earlier pagination to take pages from, its page starts by
    /// (block, paragraph, line).
    reuse: Option<(&'a Reuse<'a>, HashMap<(usize, usize, usize), usize>)>,
    /// The page start where the old pages take over: old page, new
    /// paragraph and line.
    done: Option<(usize, usize, usize)>,
}

/// The flow's state at the start of a block, to lay the rest of a
/// section out again from there (balancing columns).
#[derive(Clone)]
struct Snap {
    block: usize,
    /// Paragraphs from this one on aren't placed yet.
    first_para: usize,
    pages: usize,
    last: Page,
    setup: PageSetup,
    excl: Vec<WrapArea>,
    floats_placed: Option<usize>,
    carry: Vec<(usize, usize, f32)>,
    y: f32,
    section_pages: usize,
    number: u32,
    col: usize,
    col_top: f32,
    col_max: f32,
    notes_base: f32,
}

/// A re-broken line placed: page, line, top, its stretch (left, right)
/// and the width of the column holding it.
type WrappedLine = (usize, usize, f32, (f32, f32), f32);

/// A cell's lines in its own coordinates: paragraph, line, top, height.
struct CellLines {
    lines: Vec<(usize, usize, f32, f32)>,
    /// Height of the whole content (paragraph spacing included).
    height: f32,
}

impl<'a> Flow<'a> {
    /// Bottom of the text area: the margin, less the page's footnotes.
    fn bottom(&self) -> f32 {
        let notes = self.pages.last().map_or(0.0, |p| p.notes_height);
        let b = self.setup.height - self.setup.margin_bottom - notes;
        match self.limit {
            Some((page, l)) if page + 1 == self.pages.len() => b.min(l),
            _ => b,
        }
    }

    fn at_top(&self) -> bool {
        // The top of a page (its other columns start there too).
        self.y <= self.setup.margin_top
    }

    /// At the top of a page, or of a column after the first one (also in
    /// columns that start part way down a page): Word drops the space
    /// before a paragraph there.
    fn at_column_top(&self) -> bool {
        self.at_top() || (self.col > 0 && self.y <= self.col_top + 0.01)
    }

    /// Paragraph `pi`'s footnotes (heights, splits) as laid out for the
    /// current column's width.
    fn notes_here(&self, pi: usize) -> (&'a [(usize, f32)], &'a [NoteSplit]) {
        self.notes_at(pi, self.column().1)
    }

    /// Paragraph `pi`'s footnotes laid out for a column `width` wide (the
    /// section's first column's when there's no such layout).
    fn notes_at(&self, pi: usize, width: f32) -> (&'a [(usize, f32)], &'a [NoteSplit]) {
        let p: &'a Paragraph = &self.paras[pi];
        match p.note_widths.iter().find(|n| (n.width - width).abs() < 0.5) {
            Some(n) if self.cols.len() > 1 => (&n.notes, &n.splits),
            _ => (&p.notes, &p.note_splits),
        }
    }

    /// Columns in the current stretch.
    fn ncols(&self) -> usize {
        self.cols.len().max(1)
    }

    /// The current column: left edge and width.
    fn column(&self) -> (f32, f32) {
        match self.cols.get(self.col) {
            Some(&(x, w)) if self.cols.len() > 1 => (self.setup.margin_left + x, w),
            _ => (self.setup.margin_left, self.setup.content_width()),
        }
    }

    /// The current column when the section has several (for objects
    /// placed relative to their column).
    fn anchor_column(&self) -> Option<(f32, f32)> {
        (self.cols.len() > 1).then(|| self.column())
    }

    /// Text that doesn't fit goes on in the next column, or on the next
    /// page after the last one.
    fn next_column(&mut self) {
        if self.col + 1 >= self.ncols() {
            self.new_page();
            return;
        }
        self.end_column();
        self.col += 1;
        self.y = self.col_top;
        let page = self.pages.len() - 1;
        self.pages[page].notes_height = self.notes_base;
        self.continue_notes();
    }

    /// The current column is done: note how low it went and move its
    /// footnotes into the page's columns of notes.
    fn end_column(&mut self) {
        self.col_max = self.col_max.max(self.y);
        let Some(page) = self.pages.len().checked_sub(1) else {
            return;
        };
        let (x, width) = self.column();
        let s = self.setup;
        let base = self.notes_base;
        let p = &mut self.pages[page];
        if p.note_parts.is_empty() {
            p.continued = false;
            return;
        }
        p.note_columns.push(NoteColumn {
            x,
            width,
            bottom: s.height - s.margin_bottom - base,
            height: (p.notes_height - base).max(0.0),
            parts: std::mem::take(&mut p.note_parts),
            continued: std::mem::take(&mut p.continued),
        });
    }

    /// The stretch of columns on this page ends (at the page's end, or
    /// where a continuous section break starts others): its column boxes
    /// and the lines between them.
    fn close_region(&mut self) {
        self.end_column();
        let Some(page) = self.pages.len().checked_sub(1) else {
            return;
        };
        if self.cols.len() > 1 {
            let (top, bottom) = (self.col_top, self.col_max.max(self.col_top));
            let left = self.setup.margin_left;
            let boxes: Vec<[f32; 4]> = self
                .cols
                .iter()
                .map(|&(x, w)| [left + x, top, w, bottom - top])
                .collect();
            if self.sep && bottom > top + 0.5 {
                // Lines between the columns that have text.
                for i in 1..=self.col.min(self.cols.len() - 1) {
                    let (ax, aw) = self.cols[i - 1];
                    let (bx, _) = self.cols[i];
                    let mid = left + (ax + aw + bx) / 2.0;
                    self.pages[page].rules.push(RuleRect {
                        rect: [mid - 0.375, top, 0.75, bottom - top],
                        color: Rgba::BLACK,
                    });
                }
            }
            self.pages[page].columns.push(boxes);
        }
        // What follows on the page sits above every column's notes.
        let page_bottom = self.setup.height - self.setup.margin_bottom;
        let p = &mut self.pages[page];
        let stack = p
            .note_columns
            .iter()
            .map(|n| page_bottom - n.bottom + n.height)
            .fold(p.notes_height, f32::max);
        p.notes_height = stack;
        self.notes_base = stack;
    }

    /// The current section's columns become the stretch's, from `top`.
    fn open_region(&mut self, top: f32) {
        let s = &self.sections[self.section];
        self.cols = if s.columns.len() > 1 {
            s.columns.clone()
        } else {
            Vec::new()
        };
        self.sep = s.sep;
        self.col = 0;
        self.col_top = top;
        self.col_max = top;
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
        self.close_region();
        self.setup = self.next_setup();
        self.notes_base = 0.0;
        self.pages.push(Page {
            setup: self.setup,
            section: self.section,
            number: self.number,
            ..Page::default()
        });
        self.number += 1;
        self.section_pages += 1;
        self.y = self.setup.margin_top;
        self.open_region(self.y);
        self.excl = self
            .pre
            .get(self.pages.len() - 1)
            .cloned()
            .unwrap_or_default();
        // A page going on with a paragraph, nothing carried over: a later
        // pagination may take the pages from here on from this one.
        let start = self
            .cont
            .take()
            .filter(|_| {
                self.carry.is_empty()
                    && self.excl.is_empty()
                    && self.cols.len() <= 1
                    && self.limit.is_none()
                    && self.pre.is_empty()
            })
            .map(|(para, line)| PageStart {
                block: self.block,
                para,
                line,
                section: self.section,
                number: self.number - 1,
                setup: self.setup,
                floats_placed: self.floats_placed,
            });
        self.starts.push(start.clone());
        self.continue_notes();
        if let Some(start) = start {
            self.try_reuse(&start);
        }
    }

    /// The rest of the document is the old pagination's ([`Reuse`]) from
    /// its page `j` on: the page just started is that one, going on with
    /// line `line` of paragraph `para`.
    fn take_rest(self, j: usize, para: usize, line: usize) -> PagedDocument {
        let Some((reuse, _)) = self.reuse else {
            unreachable!("only reusing pages ends a flow early");
        };
        let old = reuse.old;
        let d = reuse.para_shift;
        let shift = |p: usize| (p as isize + d).max(0) as usize;
        let mut pages = self.pages;
        let mut starts = self.starts;
        let mut line_pages = self.line_pages;
        let mut layouts = self.layouts;
        let at = pages.len() - 1;
        let delta = i64::from(pages[at].number) - i64::from(old.pages[j].number);
        pages.truncate(at);
        starts.truncate(at);
        // Page numbers move by `delta` until a section restarts them.
        let mut moving = delta != 0;
        for (k, op) in old.pages.iter().enumerate().skip(j) {
            if moving
                && k > j
                && op.section != old.pages[k - 1].section
                && self
                    .sections
                    .get(op.section)
                    .is_some_and(|s| s.restart.is_some())
            {
                moving = false;
            }
            let number = |n: u32| {
                if moving {
                    (i64::from(n) + delta).max(0) as u32
                } else {
                    n
                }
            };
            let mut p = op.clone();
            p.number = number(p.number);
            for l in &mut p.lines {
                l.para = shift(l.para);
            }
            for n in &mut p.notes {
                n.0 = shift(n.0);
            }
            for f in &mut p.floats {
                f.para = shift(f.para);
            }
            for n in p
                .note_parts
                .iter_mut()
                .chain(p.note_columns.iter_mut().flat_map(|c| c.parts.iter_mut()))
            {
                n.para = shift(n.para);
            }
            for w in &mut p.fixed {
                w.key.0 = shift(w.key.0);
            }
            pages.push(p);
            starts.push(old.starts.get(k).cloned().flatten().map(|s| PageStart {
                block: (s.block as isize + reuse.block_shift).max(0) as usize,
                para: shift(s.para),
                number: number(s.number),
                floats_placed: s.floats_placed.map(shift),
                ..s
            }));
        }
        // Old page `j` is this page.
        let page = |p: usize| p + at - j;
        let from_old = |q: usize| usize::try_from(q as isize - d).ok();
        if let Some(oq) = from_old(para) {
            line_pages[para].truncate(line);
            let rest = old.line_pages.get(oq).into_iter().flatten().skip(line);
            line_pages[para].extend(rest.map(|&(p, dy, dx)| (page(p), dy, dx)));
        }
        for q in para + 1..line_pages.len() {
            let Some(oq) = from_old(q) else { continue };
            if let Some(lines) = old.line_pages.get(oq) {
                line_pages[q] = lines.iter().map(|&(p, dy, dx)| (page(p), dy, dx)).collect();
            }
            if let Some(l) = old.layouts.get(oq) {
                layouts[q] = l.clone();
            }
        }
        PagedDocument {
            setup: pages.first().map_or(old.setup, |p| p.setup),
            layouts,
            pages,
            line_pages,
            starts,
            reused: Some((at, j)),
        }
    }

    /// The old pagination ([`Reuse`]) started a page in the same place and
    /// state as `start`: take the rest from it ([`Self::done`]).
    fn try_reuse(&mut self, start: &PageStart) {
        let Some((reuse, by_place)) = &self.reuse else {
            return;
        };
        if self.block < reuse.from_block {
            return;
        }
        let shift = |i: usize, by: isize| usize::try_from(i as isize - by).ok();
        let (Some(block), Some(para)) = (
            shift(self.block, reuse.block_shift),
            shift(start.para, reuse.para_shift),
        ) else {
            return;
        };
        let Some(&j) = by_place.get(&(block, para, start.line)) else {
            return;
        };
        let Some(Some(old)) = reuse.old.starts.get(j) else {
            return;
        };
        let floats = start.floats_placed.map(|p| shift(p, reuse.para_shift));
        if old.section != start.section
            || old.setup != start.setup
            || floats != old.floats_placed.map(Some)
            || (old.number != start.number && !reuse.renumber)
        {
            return;
        }
        self.done = Some((j, start.para, start.line));
    }

    /// The split of a footnote, if it may break across pages.
    fn note_split(&self, pi: usize, index: usize) -> Option<&'a NoteSplit> {
        self.notes_here(pi).1.iter().find(|s| s.index == index)
    }

    /// Put the footnotes carried over at the bottom of the new page, under
    /// a continuation separator: as much as fits while leaving a few lines
    /// for text, the rest carried on again.
    fn continue_notes(&mut self) {
        if self.carry.is_empty() {
            return;
        }
        let carry = std::mem::take(&mut self.carry);
        let page = self.pages.len() - 1;
        let room = self.setup.height - self.setup.margin_bottom - self.notes_base - self.col_top;
        let max = (room - CONTINUED_TEXT).max(NOTE_SEPARATOR * 2.0);
        let mut used = NOTE_SEPARATOR;
        let mut parts = Vec::new();
        for (pi, index, from) in carry {
            let Some(split) = self.note_split(pi, index).cloned() else {
                continue;
            };
            let total = split.breaks.last().copied().unwrap_or(from);
            if !self.carry.is_empty() {
                self.carry.push((pi, index, from));
                continue;
            }
            if used + (total - from) <= max {
                parts.push(NotePart {
                    para: pi,
                    index,
                    from,
                    to: total,
                    more: false,
                });
                used += total - from;
                continue;
            }
            let room = max - used - split.notice;
            // The lowest break that fits; at least one line (progress).
            let to = split
                .breaks
                .iter()
                .copied()
                .rev()
                .find(|b| *b > from + 0.01 && b - from <= room)
                .or_else(|| split.breaks.iter().copied().find(|b| *b > from + 0.01))
                .unwrap_or(total);
            parts.push(NotePart {
                para: pi,
                index,
                from,
                to,
                more: to < total,
            });
            used += to - from + if to < total { split.notice } else { 0.0 };
            if to < total {
                self.carry.push((pi, index, to));
            }
        }
        let p = &mut self.pages[page];
        p.continued = true;
        p.notes_height = self.notes_base + used;
        p.note_parts = parts;
    }

    /// Whether line `k` of paragraph `pi` (its bottom at `bottom`, with
    /// `prior` px of footnotes from lines before it on the page not placed
    /// yet) fits with the first part of a footnote that will continue.
    fn fits_with_split(&self, pi: usize, k: usize, bottom: f32, prior: f32) -> bool {
        let Some(line) = self.layouts[pi].layout.get(k) else {
            return false;
        };
        let r = line.text_range();
        let last = k + 1 == self.layouts[pi].layout.len();
        let first = self
            .notes_here(pi)
            .0
            .iter()
            .filter(|(i, _)| *i >= r.start && (*i < r.end || last))
            .find_map(|(i, _)| self.note_split(pi, *i))
            .and_then(|s| s.breaks.first().map(|b| b + s.notice));
        let Some(first) = first else {
            return false;
        };
        let page_bottom = self.setup.height - self.setup.margin_bottom;
        let notes = self.pages.last().map_or(0.0, |p| p.notes_height);
        bottom + self.notes_cost(prior + first) + notes <= page_bottom
    }

    /// Room for endnotes and the like, breaking across pages where it may.
    fn place_gap(&mut self, g: &GapBlock) {
        if !self.at_top() {
            self.y += g.before;
        }
        let total = g.breaks.last().copied().unwrap_or(0.0);
        let mut from = 0.0f32;
        let mut guard = 0;
        loop {
            guard += 1;
            let avail = self.bottom() - self.y;
            let page = self.pages.len() - 1;
            if total - from <= avail + 0.01 || guard > 1000 {
                let x = self.column().0;
                self.pages[page].gaps.push(GapPart {
                    id: g.id,
                    x,
                    y: self.y,
                    from,
                    to: total,
                });
                self.y += total - from;
                return;
            }
            let fit = g
                .breaks
                .iter()
                .copied()
                .rev()
                .find(|b| *b > from + 0.01 && b - from <= avail);
            let to = match fit {
                Some(b) => Some(b),
                // Not even a line fits on an empty page: one anyway.
                None if self.at_top() => g.breaks.iter().copied().find(|b| *b > from + 0.01),
                None => None,
            };
            if let Some(to) = to {
                let x = self.column().0;
                self.pages[page].gaps.push(GapPart {
                    id: g.id,
                    x,
                    y: self.y,
                    from,
                    to,
                });
                from = to;
                if from >= total - 0.01 {
                    self.y += 0.0;
                    return;
                }
            }
            self.next_column();
        }
    }

    /// Place paragraph `pi`'s floating objects on the current page, its
    /// first line's top at `top` in a column starting at `x`.
    fn place_floats(&mut self, pi: usize, top: f32, x: f32) {
        self.floats_placed = Some(pi);
        let para = &self.paras[pi];
        if para.floats.is_empty() {
            return;
        }
        let layout = self.layouts[pi].clone();
        let page = self.pages.len() - 1;
        let number = self.pages[page].number;
        for spec in &para.floats {
            let cursor = Cursor::from_byte_index(&layout.layout, spec.index, Affinity::Downstream);
            let g = cursor.geometry(&layout.layout, 1.0);
            let at = AnchorAt {
                para_top: top,
                line_top: top + g.y0 as f32,
                char_x: x + layout.dx + g.x0 as f32,
                column: self.anchor_column(),
            };
            let rect = float_rect(spec, &self.setup, number, at);
            let key = (pi, spec.index);
            let fixed = matches!(
                spec.v.0,
                FloatRefV::Page
                    | FloatRefV::Margin
                    | FloatRefV::TopMargin
                    | FloatRefV::BottomMargin
                    | FloatRefV::InsideMargin
                    | FloatRefV::OutsideMargin
            );
            if let Some(e) = WrapArea::of(spec, rect, key) {
                if fixed {
                    self.pages[page].fixed.push(e.clone());
                }
                if !self.excl.iter().any(|x| x.key == key) {
                    self.excl.push(e);
                }
            }
            self.pages[page].floats.push(PlacedFloat {
                para: pi,
                index: spec.index,
                rect,
                z: spec.z,
                behind: spec.behind,
                wrap: spec.wrap,
            });
        }
    }

    /// Whether one of paragraph `pi`'s objects placed on `page` (from its
    /// `from`th object on) is kept on its page ([`FloatSpec::keep_on_page`]),
    /// moves with the text and reaches below the bottom of the text area.
    fn float_overflows(&self, pi: usize, page: usize, from: usize) -> bool {
        let bottom = self.bottom();
        self.pages[page].floats[from..].iter().any(|pf| {
            pf.para == pi
                && pf.wrap != FloatWrap::None
                && pf.rect[1] + pf.rect[3] > bottom + 0.5
                && self.paras[pi].floats.iter().any(|f| {
                    f.index == pf.index
                        && f.keep_on_page
                        && matches!(f.v.0, FloatRefV::Paragraph | FloatRefV::Line)
                })
        })
    }

    /// Whether paragraph `pi`, about to be placed, has lines to break
    /// around floating objects, or for columns of another width than the
    /// one it was laid out for.
    fn needs_wrap(&self, pi: usize) -> bool {
        let para = &self.paras[pi];
        if para.no_wrap {
            return false;
        }
        let laid_for = self.layouts[pi].width;
        if self.cols.len() > 1 && self.cols.iter().any(|c| (c.1 - laid_for).abs() > 0.5) {
            return true;
        }
        if !self.has_floats {
            return false;
        }
        if para.floats.iter().any(|f| f.wrap != FloatWrap::None) {
            return true;
        }
        let h = self.layouts[pi].layout.height() + para.space_before_pt * PT;
        let (top, bottom) = (self.y, self.y + h);
        self.excl
            .iter()
            .any(|e| e.area[1] < bottom && e.area[3] > top)
            || (bottom > self.bottom()
                && self
                    .pre
                    .get(self.pages.len())
                    .is_some_and(|v| !v.is_empty()))
    }

    /// A body paragraph whose lines are broken again around floating
    /// objects: each line gets the free stretches beside the objects at
    /// its height (one line per stretch), or moves below them; in a column
    /// of another width each line gets that column's width (a page or
    /// column break inside it goes on in the next one at its width).
    /// Widow and orphan control doesn't apply here. Footnotes: which
    /// lines hold them is only known once the lines are broken, so a
    /// paragraph with notes is placed again knowing them (until the lines
    /// stay the same, at most three times).
    fn place_para_wrapped(&mut self, pi: usize) {
        let base = self.layouts[pi].clone();
        if self.paras[pi].notes.is_empty() {
            self.wrapped_pass(pi, &base, None);
            return;
        }
        let snap = self.snap_at(pi);
        let mut known: Option<Vec<Vec<(usize, f32)>>> = None;
        for attempt in 0..3 {
            let got = self.wrapped_pass(pi, &base, known.as_deref());
            let none = known.is_none() && got.iter().all(|n| n.is_empty());
            if none || known.as_ref() == Some(&got) || attempt == 2 {
                return;
            }
            self.restore(&snap);
            self.layouts[pi] = base.clone();
            known = Some(got);
        }
    }

    /// The flow's state before paragraph `pi` is placed (to place it
    /// again).
    fn snap_at(&self, pi: usize) -> Snap {
        let pages = self.pages.len();
        Snap {
            block: 0,
            first_para: pi,
            pages,
            last: self.pages[pages - 1].clone(),
            setup: self.setup,
            excl: self.excl.clone(),
            floats_placed: self.floats_placed,
            carry: self.carry.clone(),
            y: self.y,
            section_pages: self.section_pages,
            number: self.number,
            col: self.col,
            col_top: self.col_top,
            col_max: self.col_max,
            notes_base: self.notes_base,
        }
    }

    /// One placement of a re-broken paragraph (see
    /// [`Self::place_para_wrapped`]); `known`: per line, the footnotes it
    /// holds from an earlier pass, placed with it. Returns the footnotes
    /// each line holds now.
    fn wrapped_pass(
        &mut self,
        pi: usize,
        base: &Arc<ParaLayout>,
        known: Option<&[Vec<(usize, f32)>]>,
    ) -> Vec<Vec<(usize, f32)>> {
        let para = &self.paras[pi];
        if para.page_break_before && !self.at_top() {
            self.new_page();
        }
        if !self.at_column_top() {
            self.y += para.space_before_pt * PT;
        }
        // The page and column breaks among the text's hard breaks, in
        // order (parley ends a line at each hard break).
        let hard: Vec<Option<char>> = para
            .text
            .chars()
            .filter(|c| {
                matches!(
                    *c,
                    '\n' | '\r'
                        | '\u{000B}'
                        | '\u{0085}'
                        | '\u{2028}'
                        | '\u{2029}'
                        | PAGE_BREAK
                        | COLUMN_BREAK
                )
            })
            .map(|c| matches!(c, PAGE_BREAK | COLUMN_BREAK).then_some(c))
            .collect();
        let mut hard_seen = 0usize;
        let first_h = base
            .layout
            .get(0)
            .map_or(para.size_pt * PT * 1.2, |l| l.metrics().line_height);
        if self.y + first_h > self.bottom() && !self.at_top() {
            self.next_column();
        }
        let page = self.pages.len() - 1;
        let before = (
            self.pages[page].floats.len(),
            self.pages[page].fixed.len(),
            self.excl.len(),
        );
        self.place_floats(pi, self.y, self.column().0);
        // A wrapping object that moves with its paragraph and runs past the
        // bottom of the page goes to the next page (or column) with the
        // paragraph, as Word moves it (FloatSpec::keep_on_page says which).
        if !self.at_column_top() && self.float_overflows(pi, page, before.0) {
            self.pages[page].floats.truncate(before.0);
            self.pages[page].fixed.truncate(before.1);
            self.excl.truncate(before.2);
            self.next_column();
            self.place_floats(pi, self.y, self.column().0);
        }
        // The text width from the indent in a column `w` wide.
        let avail_in = |w: f32| (base.avail + w - base.width).max(8.0);
        let widest = self
            .cols
            .iter()
            .map(|c| avail_in(c.1))
            .fold(avail_in(self.column().1), f32::max);
        let mut laid = (**base).clone();
        let mut placed: Vec<WrappedLine> = Vec::new();
        let note_of = |k: usize| -> (f32, Vec<(usize, f32)>) {
            let here = known.and_then(|n| n.get(k)).cloned().unwrap_or_default();
            (here.iter().map(|n| n.1).sum(), here)
        };
        {
            let mut br = laid.layout.break_lines();
            br.state_mut().set_layout_max_advance(widest.max(1.0));
            let mut k = 0usize;
            let mut guard = 0usize;
            'lines: while !br.is_done() {
                guard += 1;
                if guard > 100_000 {
                    break;
                }
                let est = base
                    .layout
                    .get(k.min(base.len().saturating_sub(1)))
                    .map_or(first_h, |l| l.metrics().line_height);
                let (segs, avail) = loop {
                    let (cx, cw) = self.column();
                    let avail = avail_in(cw);
                    let x0 = cx + base.dx;
                    let area = (x0, x0 + avail);
                    if self.y + est > self.bottom() && !self.at_top() {
                        self.next_column();
                        continue;
                    }
                    match free_segments(&self.excl, area, self.y, self.y + est) {
                        Ok(s) => break (s, avail),
                        Err(_) if guard > 50_000 => break (vec![area], avail),
                        Err(y) => {
                            guard += 1;
                            self.y = y.max(self.y + 1.0);
                        }
                    }
                };
                let mut row_h = 0.0f32;
                let mut forced = None;
                for (a, b) in segs {
                    // Each line starts at its layout's x 0 (parley's carets
                    // ignore a line's own x): the stretch's left edge goes
                    // in the placed line's origin instead.
                    br.state_mut()
                        .set_line_max_advance((b - a).min(avail).max(1.0));
                    if let Some(parley::layout::YieldData::LineBreak(d)) = br.break_next() {
                        let h = d.metrics.line_height;
                        let (note_h, here) = note_of(k);
                        let room = self.bottom() - self.notes_cost(note_h);
                        if row_h == 0.0 && self.y + h > room + 0.01 && !self.at_top() {
                            br.revert();
                            self.next_column();
                            continue 'lines;
                        }
                        let page = self.pages.len() - 1;
                        placed.push((page, k, self.y, (a, b), self.column().1));
                        if !here.is_empty() {
                            self.push_notes(page, pi, here, self.y + h);
                        }
                        row_h = row_h.max(h);
                        k += 1;
                        if d.reason == parley::layout::BreakReason::Explicit {
                            forced = hard.get(hard_seen).copied().flatten();
                            hard_seen += 1;
                            break;
                        }
                    }
                    if br.is_done() {
                        break;
                    }
                }
                self.y += row_h.max(0.5);
                if let Some(c) = forced {
                    self.take_break(c);
                }
            }
            br.finish();
        }
        laid.layout
            .align(laid.alignment, AlignmentOptions::default());
        let laid = Arc::new(laid);
        self.layouts[pi] = laid.clone();
        let mut got = Vec::with_capacity(placed.len());
        for &(page, k, top, (a, b), width) in &placed {
            self.push_line_bare(page, pi, k, top, (a, b - a), a);
            got.push(self.notes_on_line(pi, k, width));
        }
        self.y += self.paras[pi].space_after_pt * PT;
        got
    }

    /// Start section `i` (its first block is about to be placed).
    fn start_section(&mut self, i: usize) {
        let s = &self.sections[i];
        let same_size = !self.pages.is_empty()
            && s.setup.width == self.setup.width
            && s.setup.height == self.setup.height;
        let continuous = s.start == SectionBreak::Continuous && same_size;
        // "New column": on in the next column of the same stretch.
        let next_col =
            s.start == SectionBreak::NextColumn && same_size && self.col + 1 < self.ncols();
        let (start, restart) = (s.start, s.restart);
        self.section = i;
        self.snaps.clear();
        if continuous || next_col {
            // Same page: the new section's margins apply from here on.
            self.section_pages = 1;
            if let Some(n) = restart {
                self.number = n;
            }
            if next_col {
                self.next_column();
                return;
            }
            // Its columns start below the longest column of the ones before.
            self.close_region();
            let top = self.col_max.max(self.y);
            self.y = top;
            self.open_region(top);
            return;
        }
        self.section_pages = 0;
        if let Some(n) = restart {
            self.number = n;
        }
        // Even/odd page starts leave a blank page when needed.
        let parity = match start {
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

    /// The break (page or column) line `k` of paragraph `pi` ends with.
    fn break_after(&self, pi: usize, k: usize) -> Option<char> {
        let line = self.layouts[pi].layout.get(k)?;
        // The empty line after a break that ends the paragraph doesn't
        // break again (its range is empty, right after the break).
        let r = line.text_range();
        if r.end <= r.start {
            return None;
        }
        match self.paras[pi].text.as_bytes().get(r.end - 1).copied() {
            Some(b) if b == PAGE_BREAK as u8 => Some(PAGE_BREAK),
            Some(b) if b == COLUMN_BREAK as u8 => Some(COLUMN_BREAK),
            _ => None,
        }
    }

    /// Go on after a page or column break.
    fn take_break(&mut self, c: char) {
        if c == COLUMN_BREAK {
            self.next_column();
        } else {
            self.new_page();
        }
    }

    /// Height of the footnotes referenced from line `k` of paragraph `pi`.
    fn line_notes(&self, pi: usize, k: usize) -> f32 {
        self.notes_on_line(pi, k, self.column().1)
            .iter()
            .map(|n| n.1)
            .sum()
    }

    /// The footnotes referenced from line `k` of paragraph `pi` (byte
    /// index, height laid out for a column `width` wide).
    fn notes_on_line(&self, pi: usize, k: usize, width: f32) -> Vec<(usize, f32)> {
        let notes = self.notes_at(pi, width).0;
        if notes.is_empty() {
            return Vec::new();
        }
        let Some(line) = self.layouts[pi].layout.get(k) else {
            return Vec::new();
        };
        let r = line.text_range();
        let last = k + 1 == self.layouts[pi].layout.len();
        notes
            .iter()
            .filter(|(i, _)| *i >= r.start && (*i < r.end || last))
            .copied()
            .collect()
    }

    /// Extra room the footnotes of a line need on the current page (the
    /// separator comes with the page's first note).
    fn notes_cost(&self, note: f32) -> f32 {
        if note <= 0.0 {
            return 0.0;
        }
        let has = self.pages.last().is_some_and(|p| p.notes_height > 0.0);
        note + if has { 0.0 } else { NOTE_SEPARATOR }
    }

    fn push_line(&mut self, pi: usize, k: usize, top: f32, (x, width): (f32, f32)) {
        if k == 0 && self.floats_placed != Some(pi) {
            self.place_floats(pi, top, x);
        }
        let page = self.pages.len() - 1;
        let dx = x + self.layouts[pi].dx;
        self.push_line_on(page, pi, k, top, (x, width), dx);
    }

    /// Place line `k` of paragraph `pi` on `page` at `top`, in the column
    /// `x`/`width`, its layout's origin at `dx`, with its footnotes.
    fn push_line_on(
        &mut self,
        page: usize,
        pi: usize,
        k: usize,
        top: f32,
        (x, width): (f32, f32),
        dx: f32,
    ) {
        let bottom = self.push_line_bare(page, pi, k, top, (x, width), dx);
        let here = self.notes_on_line(pi, k, self.column().1);
        if !here.is_empty() {
            self.push_notes(page, pi, here, bottom);
        }
    }

    /// Place a line without its footnotes; returns its bottom.
    fn push_line_bare(
        &mut self,
        page: usize,
        pi: usize,
        k: usize,
        top: f32,
        (x, width): (f32, f32),
        dx: f32,
    ) -> f32 {
        let layout = &self.layouts[pi];
        let m = *layout.layout.get(k).expect("line exists").metrics();
        let dy = top - m.block_min_coord + layout.shift;
        self.pages[page].lines.push(PlacedLine {
            para: pi,
            line: k,
            top,
            height: m.line_height,
            x,
            width,
            dy,
            dx,
            repeat: false,
        });
        self.line_pages[pi].push((page, dy, dx));
        top + m.line_height
    }

    /// The footnotes `here` of a line of paragraph `pi` ending at
    /// `line_bottom` go to the bottom of this page's column; one too long
    /// for what's left starts here and goes on in the next column or page.
    fn push_notes(&mut self, page: usize, pi: usize, here: Vec<(usize, f32)>, line_bottom: f32) {
        for (i, h) in here {
            self.pages[page].notes.push((pi, i));
            if !self.carry.is_empty() {
                // Notes after one that continues follow it.
                self.carry.push((pi, i, 0.0));
                continue;
            }
            let p = &self.pages[page];
            let sep = if p.notes_height > 0.0 {
                0.0
            } else {
                NOTE_SEPARATOR
            };
            let page_bottom = p.setup.height - p.setup.margin_bottom;
            let avail = page_bottom - line_bottom - p.notes_height - sep;
            let whole = NotePart {
                para: pi,
                index: i,
                from: 0.0,
                to: h,
                more: false,
            };
            let split = self.note_split(pi, i);
            let part = match split {
                Some(s) if h > avail + 0.01 => s
                    .breaks
                    .iter()
                    .copied()
                    .rev()
                    .find(|b| *b > 0.01 && b + s.notice <= avail)
                    .filter(|b| *b < h - 0.01),
                _ => None,
            };
            let p = &mut self.pages[page];
            match part {
                Some(to) => {
                    p.note_parts.push(NotePart {
                        to,
                        more: true,
                        ..whole
                    });
                    // The page's text area ends at this line.
                    p.notes_height = (page_bottom - line_bottom).max(p.notes_height + sep + to);
                    self.carry.push((pi, i, to));
                }
                None => {
                    p.note_parts.push(whole);
                    p.notes_height += h + sep;
                }
            }
        }
    }

    /// A line of a repeated header row: drawn only.
    fn push_repeat(&mut self, pi: usize, k: usize, top: f32, (x, width): (f32, f32)) {
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
            dy: top - m.block_min_coord + layout.shift,
            dx: x + layout.dx,
            repeat: true,
        });
    }

    /// A body paragraph in the full-width column.
    fn place_para(&mut self, pi: usize) {
        if self.needs_wrap(pi) {
            self.place_para_wrapped(pi);
            return;
        }
        let para = &self.paras[pi];
        let layout = self.layouts[pi].clone();
        self.cont = Some((pi, 0));
        if para.page_break_before && !self.at_top() {
            self.new_page();
        } else if para.keep_next && !self.at_top() && self.ncols() == 1 && self.limit.is_none() {
            let need = self.kept_height(pi);
            let room = self.setup.height - self.setup.margin_top - self.setup.margin_bottom;
            if self.y + need > self.bottom() && need < room * 0.8 {
                self.new_page();
            }
        }
        self.cont = None;
        if self.done.is_some() {
            return;
        }
        let heights: Vec<f32> = layout
            .layout
            .lines()
            .map(|l| l.metrics().line_height)
            .collect();
        let n = heights.len();
        let notes: Vec<f32> = (0..n).map(|k| self.line_notes(pi, k)).collect();
        let forced: Vec<Option<char>> = (0..n).map(|k| self.break_after(pi, k)).collect();
        let keep_together = !forced.iter().any(|f| f.is_some());
        // Space before is dropped at the top of a page, as in Word.
        if !self.at_column_top() {
            self.y += para.space_before_pt * PT;
        }
        let mut i = 0;
        while i < n {
            let bottom = self.bottom();
            // How many lines fit on this page from line i on (with the
            // footnotes they bring)?
            let mut fit = 0;
            let mut h = 0.0;
            let mut nh = 0.0;
            while i + fit < n {
                let k = i + fit;
                if self.y + h + heights[k] > bottom - self.notes_cost(nh + notes[k]) {
                    // A long footnote starts here and continues (Word
                    // keeps the reference with the note's first lines).
                    if notes[k] > 0.0 && self.fits_with_split(pi, k, self.y + h + heights[k], nh) {
                        fit += 1;
                    }
                    break;
                }
                h += heights[k];
                nh += notes[k];
                fit += 1;
                if forced[k].is_some() {
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
            let column = self.column();
            for (k, h) in heights.iter().enumerate().skip(i).take(take) {
                let top = self.y;
                self.push_line(pi, k, top, column);
                self.y += h;
            }
            i += take;
            self.cont = Some((pi, i));
            match forced.get(i.wrapping_sub(1)).copied().flatten() {
                Some(c) if take > 0 => self.take_break(c),
                _ if i < n => self.next_column(),
                _ => {}
            }
            self.cont = None;
            if self.done.is_some() {
                return;
            }
        }
        self.y += para.space_after_pt * PT;
    }

    /// The height paragraph `pi` needs at the bottom of a page to stay with
    /// what follows it: all of it and of the paragraphs it keeps with,
    /// then the first two lines of the next.
    fn kept_height(&self, pi: usize) -> f32 {
        let lines = |k: usize| -> Vec<f32> {
            self.layouts
                .get(k)
                .map(|l| l.layout.lines().map(|x| x.metrics().line_height).collect())
                .unwrap_or_default()
        };
        let mut h = 0.0;
        for k in (pi..).take(6) {
            let p = &self.paras[k];
            if k > pi {
                h += p.space_before_pt * PT;
            }
            let ls = lines(k);
            if k > pi && !(p.keep_next && k + 1 < self.paras.len()) {
                // The next paragraph: its first lines.
                return h + ls.iter().take(MIN_LINES).sum::<f32>();
            }
            h += ls.iter().sum::<f32>() + p.space_after_pt * PT;
            if k + 1 >= self.paras.len() || self.paras[k + 1].page_break_before {
                return 0.0;
            }
        }
        h
    }

    /// A cell's lines in cell coordinates.
    fn cell_lines(&self, cell: &TableCell) -> CellLines {
        let mut lines = Vec::new();
        let mut y = 0.0;
        for (k, pi) in cell.paras.clone().enumerate() {
            let p = &self.paras[pi];
            if k > 0 {
                y += p.space_before_pt * PT;
            }
            for (line, l) in self.layouts[pi].layout.lines().enumerate() {
                let lh = l.metrics().line_height;
                lines.push((pi, line, y, lh));
                y += lh;
            }
            y += p.space_after_pt * PT;
        }
        CellLines { lines, height: y }
    }

    /// Draw a cell's borders on the current page.
    fn cell_borders(&mut self, cell: &TableCell, cx: f32, cw: f32, top: f32, h: f32) {
        let page = self.pages.len() - 1;
        let rules = &mut self.pages[page].rules;
        if let Some(color) = cell.fill {
            rules.push(RuleRect {
                rect: [cx, top, cw, h],
                color,
            });
        }
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

    /// A row's full height and its cells' lines.
    fn row_lines(&self, t: &TableBlock, row: &TableRow) -> (f32, Vec<Option<CellLines>>) {
        let [pad_t, _, pad_b, _] = t.padding;
        let cells: Vec<Option<CellLines>> = row
            .cells
            .iter()
            .map(|c| (c.merge != Merge::Continue).then(|| self.cell_lines(c)))
            .collect();
        let content = cells
            .iter()
            .flatten()
            .map(|c| c.height + pad_t + pad_b)
            .fold(0.0f32, f32::max);
        let h = if row.exact {
            row.min_height
        } else {
            content.max(row.min_height)
        }
        .max(1.0);
        (h, cells)
    }

    /// Place a whole row at `self.y` (`repeat`: a repeated header row).
    fn place_row_whole(
        &mut self,
        t: &TableBlock,
        row: &TableRow,
        h: f32,
        cells: &[Option<CellLines>],
        repeat: bool,
    ) {
        let [pad_t, pad_r, _, pad_l] = t.padding;
        let x0 = self.column().0 + t.x;
        let top = self.y;
        for (cell, lines) in row.cells.iter().zip(cells) {
            let cx = x0 + t.col_x(cell.col);
            let cw = t.span_width(cell.col, cell.span);
            let column = (cx + pad_l, (cw - pad_l - pad_r).max(8.0));
            for &(pi, line, y, _) in lines.iter().flat_map(|c| c.lines.iter()) {
                if repeat {
                    self.push_repeat(pi, line, top + pad_t + y, column);
                } else {
                    self.push_line(pi, line, top + pad_t + y, column);
                }
            }
            self.cell_borders(cell, cx, cw, top, h);
        }
        self.y = top + h;
    }

    /// Repeat the table's header rows at the top of a new page.
    fn repeat_headers(&mut self, t: &TableBlock, headers: usize) {
        for row in &t.rows[..headers] {
            let (h, cells) = self.row_lines(t, row);
            self.place_row_whole(t, row, h, &cells, true);
        }
    }

    /// Footnotes of every line in a row.
    fn row_notes(&self, cells: &[Option<CellLines>]) -> f32 {
        cells
            .iter()
            .flatten()
            .flat_map(|c| c.lines.iter())
            .map(|&(pi, k, _, _)| self.line_notes(pi, k))
            .sum()
    }

    fn place_table(&mut self, t: &TableBlock) {
        // Header rows repeat only when rows other than headers follow.
        let headers = t.rows.iter().take_while(|r| r.header).count();
        let headers = if headers < t.rows.len() { headers } else { 0 };
        let new_page = |f: &mut Self, ri: usize| {
            f.next_column();
            if headers > 0 && ri >= headers {
                f.repeat_headers(t, headers);
            }
        };
        for (ri, row) in t.rows.iter().enumerate() {
            // Rows kept with the next (Word: every paragraph in them keeps
            // with next) start a page with that row when they'd end one
            // alone: a header row isn't left at the bottom of a page.
            if !self.at_top()
                && self.ncols() == 1
                && self.limit.is_none()
                && (ri == 0 || !t.rows[ri - 1].keep_next)
            {
                let need = self.kept_rows_height(t, ri);
                let room = self.setup.height - self.setup.margin_top - self.setup.margin_bottom;
                if need > 0.0 && self.y + need > self.bottom() && need < room * 0.8 {
                    new_page(self, ri);
                }
            }
            let (h, cells) = self.row_lines(t, row);
            let notes = self.notes_cost(self.row_notes(&cells));
            if self.y + h <= self.bottom() - notes {
                self.place_row_whole(t, row, h, &cells, false);
                continue;
            }
            // Word breaks a row at a line unless it's marked "can't split"
            // (or has an exact height); header rows stay whole too.
            let splits = !row.exact && !row.cant_split && ri >= headers;
            if !self.at_top() && (!splits || !self.first_line_fits(t, &cells)) {
                new_page(self, ri);
                let notes = self.notes_cost(self.row_notes(&cells));
                if self.y + h <= self.bottom() - notes {
                    self.place_row_whole(t, row, h, &cells, false);
                    continue;
                }
            }
            // Taller than a page: a row that can't split breaks anyway
            // (as in Word), unless its height is exact.
            if row.exact {
                self.place_row_whole(t, row, h, &cells, false);
                continue;
            }
            self.place_row_split(t, row, ri, &cells, &new_page);
        }
    }

    /// The height rows `ri…` need at the bottom of a page to stay
    /// together: the rows that keep with the next, then the first line of
    /// the row after them (all of it when it can't split); 0 when row `ri`
    /// doesn't keep with the next or nothing follows it.
    fn kept_rows_height(&self, t: &TableBlock, ri: usize) -> f32 {
        let mut h = 0.0;
        let mut k = ri;
        while k < t.rows.len() && t.rows[k].keep_next {
            h += self.row_lines(t, &t.rows[k]).0;
            k += 1;
        }
        if k == ri || k >= t.rows.len() {
            return 0.0;
        }
        let next = &t.rows[k];
        let (nh, cells) = self.row_lines(t, next);
        if next.cant_split || next.exact {
            return h + nh;
        }
        let [pad_t, _, pad_b, _] = t.padding;
        let first = cells
            .iter()
            .flatten()
            .filter_map(|c| c.lines.first().map(|&(_, _, y, lh)| y + lh))
            .fold(0.0f32, f32::max);
        h + (first + pad_t + pad_b).min(nh)
    }

    /// Whether some cell's first line fits on this page.
    fn first_line_fits(&self, t: &TableBlock, cells: &[Option<CellLines>]) -> bool {
        let [pad_t, _, pad_b, _] = t.padding;
        cells.iter().flatten().any(|c| {
            c.lines.first().is_some_and(|&(pi, k, y, lh)| {
                self.y + pad_t + y + lh + pad_b
                    <= self.bottom() - self.notes_cost(self.line_notes(pi, k))
            })
        })
    }

    /// Place a row line by line, breaking it across pages where its cells'
    /// lines stop fitting.
    fn place_row_split(
        &mut self,
        t: &TableBlock,
        row: &TableRow,
        ri: usize,
        cells: &[Option<CellLines>],
        new_page: &dyn Fn(&mut Self, usize),
    ) {
        let [pad_t, pad_r, pad_b, pad_l] = t.padding;
        let x0 = self.column().0 + t.x;
        let mut next = vec![0usize; cells.len()];
        let mut used = 0.0f32;
        loop {
            let top = self.y;
            // Each cell continues from its first line not placed yet.
            let starts: Vec<f32> = cells
                .iter()
                .zip(&next)
                .map(|(c, &n)| {
                    c.as_ref()
                        .map_or(0.0, |c| c.lines.get(n).map_or(c.height, |&(_, _, y, _)| y))
                })
                .collect();
            let mut extent = 0.0f32;
            let mut placed = false;
            for (ci, cell) in row.cells.iter().enumerate() {
                let Some(lines) = &cells[ci] else { continue };
                let cx = x0 + t.col_x(cell.col);
                let cw = t.span_width(cell.col, cell.span);
                let column = (cx + pad_l, (cw - pad_l - pad_r).max(8.0));
                while let Some(&(pi, k, y, lh)) = lines.lines.get(next[ci]) {
                    let ly = top + pad_t + y - starts[ci];
                    let cost = self.notes_cost(self.line_notes(pi, k));
                    let fits = ly + lh + pad_b <= self.bottom() - cost;
                    // Nothing fits on an empty page: one line anyway.
                    if !fits && (placed || !self.at_top()) {
                        break;
                    }
                    self.push_line(pi, k, ly, column);
                    extent = extent.max(ly + lh - top - pad_t);
                    next[ci] += 1;
                    placed = true;
                }
            }
            let done = cells
                .iter()
                .zip(&next)
                .all(|(c, &n)| c.as_ref().is_none_or(|c| n >= c.lines.len()));
            let h = if done {
                // The rest of each cell (its last spacing) and the rest of
                // the row's minimum height.
                let rest = cells
                    .iter()
                    .zip(&starts)
                    .filter_map(|(c, s)| c.as_ref().map(|c| c.height - s))
                    .fold(extent, f32::max);
                (rest + pad_t + pad_b).max(row.min_height - used)
            } else {
                extent + pad_t + pad_b
            }
            .max(1.0);
            for cell in &row.cells {
                let cx = x0 + t.col_x(cell.col);
                let cw = t.span_width(cell.col, cell.span);
                self.cell_borders(cell, cx, cw, top, h);
            }
            self.y = top + h;
            used += h;
            if done {
                break;
            }
            new_page(self, ri);
        }
    }
}

impl<'a> Flow<'a> {
    /// Place blocks `range`, remembering where balancing could start
    /// again (`snap`).
    fn run(&mut self, blocks: &[Block], range: Range<usize>, snap: bool) {
        for bi in range {
            if self.done.is_some() {
                return;
            }
            self.block = bi;
            if snap && self.limit.is_none() {
                self.snapshot(blocks, bi);
            }
            match &blocks[bi] {
                Block::Para(pi) => self.place_para(*pi),
                Block::Table(t) => self.place_table(t),
                Block::Gap(g) => self.place_gap(g),
            }
        }
    }

    /// Remember the state before block `bi` (the first block starting on
    /// each page, and the section's first).
    fn snapshot(&mut self, blocks: &[Block], bi: usize) {
        let pages = self.pages.len();
        if self.snaps.last().is_some_and(|s| s.pages == pages) {
            return;
        }
        let first_para = blocks[bi..]
            .iter()
            .find_map(|b| match b {
                Block::Para(i) => Some(*i),
                Block::Table(t) => t
                    .rows
                    .first()
                    .and_then(|r| r.cells.first())
                    .map(|c| c.paras.start),
                Block::Gap(_) => None,
            })
            .unwrap_or(self.paras.len());
        self.snaps.push(Snap {
            block: bi,
            first_para,
            pages,
            last: self.pages[pages - 1].clone(),
            setup: self.setup,
            excl: self.excl.clone(),
            floats_placed: self.floats_placed,
            carry: self.carry.clone(),
            y: self.y,
            section_pages: self.section_pages,
            number: self.number,
            col: self.col,
            col_top: self.col_top,
            col_max: self.col_max,
            notes_base: self.notes_base,
        });
        // The section's first and the last two pages' are enough.
        if self.snaps.len() > 3 {
            self.snaps.remove(1);
        }
    }

    fn restore(&mut self, s: &Snap) {
        self.pages.truncate(s.pages);
        self.starts.truncate(s.pages);
        if let Some(last) = self.pages.last_mut() {
            *last = s.last.clone();
        }
        for pi in s.first_para..self.line_pages.len() {
            self.line_pages[pi].clear();
            if let Some(o) = self.orig.get(pi)
                && !Arc::ptr_eq(&self.layouts[pi], o)
            {
                self.layouts[pi] = o.clone();
            }
        }
        self.setup = s.setup;
        self.excl = s.excl.clone();
        self.floats_placed = s.floats_placed;
        self.carry = s.carry.clone();
        self.y = s.y;
        self.section_pages = s.section_pages;
        self.number = s.number;
        self.col = s.col;
        self.col_top = s.col_top;
        self.col_max = s.col_max;
        self.notes_base = s.notes_base;
    }

    /// Balance the columns of the section just placed (blocks up to
    /// `end`) on its last page, as Word does before a continuous section
    /// break: the lowest bottom at which the rest still fits on the page.
    fn balance(&mut self, blocks: &[Block], end: usize) {
        let target = self.pages.len() - 1;
        let snaps = std::mem::take(&mut self.snaps);
        let Some(snap) = snaps
            .iter()
            .rev()
            .find(|s| s.pages - 1 < target)
            .or(snaps.first())
            .cloned()
        else {
            return;
        };
        let top = self.col_top;
        let page_bottom = self.setup.height - self.setup.margin_bottom;
        let fits = |f: &mut Self, limit: f32| {
            f.restore(&snap);
            f.limit = Some((target, limit));
            f.run(blocks, snap.block..end, false);
            f.limit = None;
            f.pages.len() - 1 == target && f.carry.is_empty()
        };
        let (mut lo, mut hi) = (top, page_bottom);
        for _ in 0..16 {
            if hi - lo < 0.5 {
                break;
            }
            let mid = (lo + hi) / 2.0;
            if fits(self, mid) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        if !fits(self, hi) {
            fits(self, page_bottom);
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
            columns: Vec::new(),
            sep: false,
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
        Self::paginate_reusing(paras, layouts, blocks, sections, None)
    }

    /// [`Self::paginate_sections`] taking what it can from an earlier
    /// pagination ([`Reuse`]).
    pub fn paginate_reusing(
        paras: &[Paragraph],
        layouts: Vec<Arc<ParaLayout>>,
        blocks: &[Block],
        sections: &[SectionFlow],
        reuse: Option<&Reuse<'_>>,
    ) -> Self {
        let fallback = [SectionFlow {
            first_block: 0,
            setup: PageSetup::a4(),
            first_setup: None,
            even_setup: None,
            start: SectionBreak::NextPage,
            restart: None,
            columns: Vec::new(),
            sep: false,
        }];
        let sections = if sections.is_empty() {
            &fallback[..]
        } else {
            sections
        };
        let has_floats = paras.iter().any(|p| !p.floats.is_empty());
        let first = Self::flow(
            paras,
            layouts.clone(),
            blocks,
            sections,
            &[],
            has_floats,
            reuse,
        );
        // Wrapping objects fixed to their page also push away the text
        // above their anchor: lay out again knowing where they are.
        let fixed: Vec<Vec<WrapArea>> = first.pages.iter().map(|p| p.fixed.clone()).collect();
        if fixed.iter().any(|f| !f.is_empty()) {
            return Self::flow(paras, layouts, blocks, sections, &fixed, has_floats, None);
        }
        first
    }

    /// The pages from `.0` on are the earlier pagination's from its page
    /// `.1` on (see [`Reuse`]).
    pub fn reused(&self) -> Option<(usize, usize)> {
        self.reused
    }

    fn flow<'a>(
        paras: &'a [Paragraph],
        layouts: Vec<Arc<ParaLayout>>,
        blocks: &[Block],
        sections: &'a [SectionFlow],
        pre: &'a [Vec<WrapArea>],
        has_floats: bool,
        reuse: Option<&'a Reuse<'a>>,
    ) -> Self {
        let n = layouts.len();
        let columns = sections.iter().any(|s| s.columns.len() > 1);
        let mut flow = Flow {
            setup: sections[0].setup,
            paras,
            orig: if columns { layouts.clone() } else { Vec::new() },
            layouts,
            excl: Vec::new(),
            pre,
            has_floats,
            floats_placed: None,
            carry: Vec::new(),
            pages: Vec::new(),
            line_pages: vec![Vec::new(); n],
            y: 0.0,
            sections,
            section: 0,
            section_pages: 0,
            number: sections[0].restart.unwrap_or(1),
            col: 0,
            col_top: 0.0,
            col_max: 0.0,
            cols: Vec::new(),
            sep: false,
            notes_base: 0.0,
            limit: None,
            snaps: Vec::new(),
            block: 0,
            cont: None,
            starts: Vec::new(),
            reuse: reuse.map(|r| {
                let by_place = r
                    .old
                    .starts
                    .iter()
                    .enumerate()
                    .filter_map(|(j, s)| s.as_ref().map(|s| ((s.block, s.para, s.line), j)))
                    .collect();
                (r, by_place)
            }),
            done: None,
        };
        flow.new_page();
        let first = |si: usize| {
            if si == 0 {
                0
            } else {
                sections[si].first_block.min(blocks.len())
            }
        };
        for si in 0..sections.len() {
            if flow.done.is_some() {
                break;
            }
            if si > 0 {
                flow.start_section(si);
            }
            let start = first(si);
            let end = sections
                .get(si + 1)
                .map_or(blocks.len(), |_| first(si + 1))
                .max(start);
            // Word balances a section's columns on its last page when a
            // continuous section break follows it.
            let balance = flow.cols.len() > 1
                && sections.get(si + 1).is_some_and(|next| {
                    next.start == SectionBreak::Continuous
                        && next.setup.width == flow.setup.width
                        && next.setup.height == flow.setup.height
                });
            flow.run(blocks, start..end, balance);
            if balance && flow.done.is_none() {
                flow.balance(blocks, end);
            }
        }
        if let Some((j, para, line)) = flow.done {
            return flow.take_rest(j, para, line);
        }
        // A footnote still going on gets the pages it needs.
        let mut guard = 0;
        while !flow.carry.is_empty() && guard < 1000 {
            flow.new_page();
            guard += 1;
        }
        flow.close_region();
        let Flow {
            pages,
            line_pages,
            layouts,
            starts,
            ..
        } = flow;
        Self {
            setup: pages.first().map_or(sections[0].setup, |p| p.setup),
            layouts,
            pages,
            line_pages,
            starts,
            reused: None,
        }
    }

    /// The stretch `from`..`to` (px down page `page`) of a laid-out
    /// story, moved up to start at 0: the part of a long note shown on a
    /// page.
    pub fn slice(&self, page: usize, from: f32, to: f32) -> Self {
        let mut out = Self {
            setup: self.setup,
            layouts: self.layouts.clone(),
            pages: Vec::new(),
            line_pages: self
                .line_pages
                .iter()
                .map(|v| v.iter().map(|&(p, dy, dx)| (p, dy - from, dx)).collect())
                .collect(),
            starts: Vec::new(),
            reused: None,
        };
        let mut pg = self.pages.get(page).cloned().unwrap_or_default();
        pg.lines
            .retain(|l| l.top >= from - 0.5 && l.top + l.height <= to + 0.5);
        for l in &mut pg.lines {
            l.top -= from;
            l.dy -= from;
        }
        pg.rules
            .retain(|r| r.rect[1] >= from - 0.5 && r.rect[1] <= to + 0.5);
        for r in &mut pg.rules {
            r.rect[1] -= from;
        }
        out.pages.push(pg);
        out
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
            RunSource::Label(i) | RunSource::Mark(i) => l
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
            let (dy, dx) = (pl.dy, pl.dx);
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
                    && let Some(&(index, kind)) = pl_layout.objects.get(b.id as usize)
                    && matches!(
                        kind,
                        ObjectKind::Placeholder | ObjectKind::Mark | ObjectKind::Text
                    )
                    && let Some((_, ll, _)) = pl_layout.labels.iter().find(|x| x.0 == index)
                    && let Some(lline) = ll.get(0)
                {
                    let (lx, ly, source) = if kind != ObjectKind::Placeholder {
                        (dx + b.x, dy + b.y, RunSource::Mark(index))
                    } else {
                        (
                            dx + b.x + LABEL_PAD,
                            dy + b.y + (b.height - lline.metrics().line_height) / 2.0,
                            RunSource::Label(index),
                        )
                    };
                    emit_runs(&mut out, &lline, pl.para, source, lx, ly);
                }
            }
        }
        out
    }

    /// Strikethrough lines on a page for the paragraphs' `strikes`
    /// (`paras` are the paragraphs the document was laid out from), in the
    /// text's colour, ready to fill.
    pub fn strike_rules(&self, page: usize, paras: &[Paragraph]) -> Vec<RuleRect> {
        let mut out = Vec::new();
        if !paras.iter().any(|p| !p.strikes.is_empty()) {
            return out;
        }
        for run in self.glyph_runs(page) {
            if run.source != RunSource::Text {
                continue;
            }
            let Some(p) = paras.get(run.para) else {
                continue;
            };
            for s in &p.strikes {
                let mut x0 = f32::MAX;
                let mut x1 = f32::MIN;
                for g in &run.glyphs {
                    if g.cluster.start >= s.start && g.cluster.end <= s.end {
                        x0 = x0.min(g.x);
                        x1 = x1.max(g.x + g.advance);
                    }
                }
                if x1 > x0 {
                    let thick = (run.font_size / 16.0).max(1.0);
                    let y = run.baseline - run.font_size * 0.28 - thick / 2.0;
                    out.push(RuleRect {
                        rect: [x0, y, x1 - x0, thick],
                        color: run.color,
                    });
                }
            }
        }
        out
    }

    /// Tab leaders and bar-tab lines on a page, ready to fill. Leaders
    /// are drawn as shapes (dots, dashes, lines) sized from the tab's font
    /// size, the dots on a grid from the column's edge so they line up from
    /// line to line as Word's do.
    pub fn tab_rules(&self, page: usize) -> Vec<RuleRect> {
        let mut out = Vec::new();
        let Some(p) = self.pages.get(page) else {
            return out;
        };
        for pl in &p.lines {
            let l = &self.layouts[pl.para];
            for &bx in &l.bars {
                out.push(RuleRect {
                    rect: [pl.x + bx - 0.5, pl.top, 1.0, pl.height],
                    color: Rgba::BLACK,
                });
            }
            if l.tab_fills.iter().all(|f| f.leader == Leader::None) {
                continue;
            }
            let Some(line) = l.layout.get(pl.line) else {
                continue;
            };
            let baseline = pl.dy + line.metrics().baseline;
            for item in line.items() {
                if let PositionedLayoutItem::InlineBox(b) = item
                    && b.id as usize >= TAB_BOX
                    && let Some(fill) = l.tab_fills.get(b.id as usize - TAB_BOX)
                {
                    let x0 = pl.dx + b.x;
                    leader_rects(&mut out, *fill, pl.x, x0, x0 + b.width, baseline);
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
            let (dy, dx) = (pl.dy, pl.dx);
            let Some(line) = l.layout.get(pl.line) else {
                continue;
            };
            for item in line.items() {
                if let PositionedLayoutItem::InlineBox(b) = item
                    && let Some(&(index, kind)) = l.objects.get(b.id as usize)
                    && !matches!(
                        kind,
                        ObjectKind::Hidden | ObjectKind::Mark | ObjectKind::Text
                    )
                {
                    let whole = [dx + b.x, dy + b.y, b.width, b.height];
                    let full = l.heights.get(b.id as usize).copied().unwrap_or(b.height);
                    let bottom = whole[1] + whole[3];
                    let whole = [whole[0], bottom - full, whole[2], full];
                    // An exact line cuts off what's above its top.
                    let shown = whole[1].max(pl.top).min(bottom);
                    let rect = [whole[0], shown, whole[2], bottom - shown];
                    out.push(PlacedObject {
                        para: pl.para,
                        index,
                        kind,
                        rect,
                        whole,
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
            .filter(|(_, l)| !l.repeat)
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
        let (dy, dx) = (pl.dy, pl.dx);
        let layout = &self.layouts[pl.para].layout;
        let m = *layout.get(pl.line)?.metrics();
        let ly = ((m.block_min_coord + m.block_max_coord) / 2.0).max(pl.top - dy);
        let cursor = Cursor::from_point(layout, x - dx, ly);
        Some((pl.para, on_line_end(layout, pl.line, cursor.index())))
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

    /// Where the label of object `index` of `para` (a field code shown in
    /// the text) is drawn: page, its left and top (page px), its layout.
    fn label_box(&self, para: usize, index: usize) -> Option<(usize, f32, f32, &Layout<Rgba>)> {
        let l = self.layouts.get(para)?;
        let (_, label, _) = l.labels.iter().find(|x| x.0 == index)?;
        for (k, line) in l.layout.lines().enumerate() {
            let Some((page, dy, dx)) = self.origin(para, k) else {
                continue;
            };
            for item in line.items() {
                if let PositionedLayoutItem::InlineBox(b) = item
                    && l.objects.get(b.id as usize).is_some_and(|o| o.0 == index)
                {
                    return Some((page, dx + b.x, dy + b.y, label));
                }
            }
        }
        None
    }

    /// The caret at byte `at` of the label of object `index` of `para`
    /// (a field's code shown in the text).
    pub fn label_caret(&self, para: usize, index: usize, at: usize) -> Option<CaretRect> {
        let (page, x, y, label) = self.label_box(para, index)?;
        let cursor = Cursor::from_byte_index(label, at, Affinity::Downstream);
        let r = cursor.geometry(label, 1.0);
        Some(CaretRect {
            page,
            x: x + r.x0 as f32,
            top: y + r.y0 as f32,
            height: (r.y1 - r.y0) as f32,
        })
    }

    /// The byte of the label of object `index` of `para` nearest to `x`
    /// on `page`, when `(x, y)` is on the label.
    pub fn label_hit(
        &self,
        para: usize,
        index: usize,
        page: usize,
        x: f32,
        y: f32,
    ) -> Option<usize> {
        let (p, lx, ly, label) = self.label_box(para, index)?;
        let (w, h) = (label.width(), label.height());
        if p != page || x < lx - 1.0 || x > lx + w + 1.0 || y < ly - 1.0 || y > ly + h + 1.0 {
            return None;
        }
        Some(Cursor::from_point(label, x - lx, h / 2.0).index())
    }

    /// `(page, index into that page's lines)` of the line showing `index` of `para`.
    pub fn placed_line(&self, para: usize, index: usize) -> Option<(usize, usize)> {
        let k = self.layout_line_of(para, index)?;
        let (page, _, _) = self.origin(para, k)?;
        let i = self.pages[page]
            .lines
            .iter()
            .position(|l| l.para == para && l.line == k && !l.repeat)?;
        Some((page, i))
    }

    /// The stretch of columns and the column on `page` holding line `l`.
    pub fn column_of(&self, page: usize, l: &PlacedLine) -> Option<(usize, usize)> {
        let p = self.pages.get(page)?;
        p.columns.iter().enumerate().find_map(|(r, cols)| {
            cols.iter()
                .position(|c| {
                    l.x + 0.5 >= c[0]
                        && l.x < c[0] + c[2]
                        && l.top + 0.5 >= c[1]
                        && l.top < c[1] + c[3] + 0.5
                })
                .map(|j| (r, j))
        })
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
        let from = if down { cur.top + cur.height } else { cur.top };
        // In columns: past a column's last line Down goes to the top of the
        // next column, Up from its first line to the bottom of the one
        // before (as in Word); off the page, to the first or last column.
        let mut x = x;
        if let Some((r, j)) = self.column_of(page, &cur) {
            let region = &self.pages[page].columns[r];
            let inside = |l: &PlacedLine, c: &[f32; 4]| {
                !l.repeat
                    && l.x + 0.5 >= c[0]
                    && l.x < c[0] + c[2]
                    && l.top + 0.5 >= c[1]
                    && l.top < c[1] + c[3] + 0.5
            };
            let col = region[j];
            let lines = &self.pages[page].lines;
            let more = lines.iter().any(|l| {
                inside(l, &col)
                    && in_col(l)
                    && if down {
                        l.top >= from - 0.5
                    } else {
                        l.top + l.height <= from + 0.5
                    }
                    && !(l.para == cur.para && l.line == cur.line)
            });
            if !more {
                let next = if down {
                    region.get(j + 1)
                } else {
                    j.checked_sub(1).and_then(|k| region.get(k))
                };
                if let Some(nc) = next {
                    let best = lines
                        .iter()
                        .enumerate()
                        .filter(|(_, l)| inside(l, nc))
                        .min_by(|(_, a), (_, b)| {
                            let (ka, kb) = if down {
                                (a.top, b.top)
                            } else {
                                (-a.top, -b.top)
                            };
                            ka.total_cmp(&kb)
                        })
                        .map(|(k, _)| k);
                    if let Some(k) = best {
                        return self.hit_placed(page, k, x - col[0] + nc[0]);
                    }
                }
                // Leaving the columns: as if from the first (down) or last
                // (up) column, so the next page's first column is next.
                let edge = if down {
                    region[0]
                } else {
                    region[region.len() - 1]
                };
                x = x - col[0] + edge[0];
            }
        }
        let in_col = |l: &PlacedLine| x >= l.x - 0.5 && x <= l.x + l.width + 0.5;
        let pick = |lines: &[PlacedLine], from: Option<f32>| -> Option<usize> {
            let mut best: Option<(usize, f32)> = None;
            for (k, l) in lines.iter().enumerate() {
                let ok = match from {
                    Some(y) if down => l.top >= y - 0.5,
                    Some(y) => l.top + l.height <= y + 0.5,
                    None => true,
                };
                if !ok || l.repeat || (l.para == cur.para && l.line == cur.line && from.is_some()) {
                    continue;
                }
                let score = if down { l.top } else { -l.top } + if in_col(l) { 0.0 } else { 1e6 };
                if best.is_none_or(|(_, s)| score < s) {
                    best = Some((k, score));
                }
            }
            best.map(|(k, _)| k)
        };
        if let Some(k) = pick(&self.pages[page].lines, Some(from)) {
            let l = self.pages[page].lines[k];
            // Only accept an out-of-column line if no in-column one exists on
            // this page; otherwise move pages.
            if in_col(&l)
                || !self.pages[page].lines.iter().any(|m| {
                    !m.repeat && in_col(m) && (if down { m.top > from } else { m.top < from })
                })
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
    /// The first and last paragraph with a line on pages `pages`.
    pub fn paras_on(&self, pages: Range<usize>) -> Option<(usize, usize)> {
        let n = self.pages.len();
        let mut span: Option<(usize, usize)> = None;
        for page in &self.pages[pages.start.min(n)..pages.end.min(n)] {
            for l in &page.lines {
                span = Some(match span {
                    Some((lo, hi)) => (lo.min(l.para), hi.max(l.para)),
                    None => (l.para, l.para),
                });
            }
        }
        span
    }

    /// [`Self::selection_rects`] on pages `pages` only: paragraphs with no
    /// line there are skipped, so a long selection (or thousands of
    /// highlighted ranges) costs what the pages show.
    pub fn selection_rects_on(
        &self,
        start: (usize, usize),
        end: (usize, usize),
        pages: Range<usize>,
    ) -> Vec<(usize, [f32; 4])> {
        let Some((lo, hi)) = self.paras_on(pages.clone()) else {
            return Vec::new();
        };
        if end.0 < lo || start.0 > hi {
            return Vec::new();
        }
        let start = if start.0 < lo { (lo, 0) } else { start };
        // Past the last paragraph shown: to the start of the next one, so
        // the last one's paragraph mark is selected as before.
        let end = if end.0 > hi { (hi + 1, 0) } else { end };
        let mut out = self.selection_rects(start, end);
        out.retain(|(page, _)| pages.contains(page));
        out
    }

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
/// Shapes for a leader from `x0` to `x1` on `baseline` (`grid` is the
/// column's left edge, where the dot grid starts).
fn leader_rects(
    out: &mut Vec<RuleRect>,
    fill: TabFill,
    grid: f32,
    x0: f32,
    x1: f32,
    baseline: f32,
) {
    let s = fill.size.max(1.0);
    let color = fill.color;
    let mut push = |x: f32, y: f32, w: f32, h: f32| {
        out.push(RuleRect {
            rect: [x, y, w, h],
            color,
        });
    };
    // Repeated marks `mark` wide every `step`, kept off the text at both ends.
    let mut repeat = |step: f32, mark: f32, y: f32, h: f32| {
        let start = x0 + s * 0.08;
        let end = x1 - s * 0.12;
        let mut x = grid + ((start - grid) / step).ceil() * step;
        while x + mark <= end {
            push(x, y, mark, h);
            x += step;
        }
    };
    match fill.leader {
        Leader::None => {}
        Leader::Dot => {
            let d = (s * 0.09).max(1.0);
            repeat(s * 0.25, d, baseline - d, d);
        }
        Leader::MiddleDot => {
            let d = (s * 0.09).max(1.0);
            repeat(s * 0.25, d, baseline - s * 0.3 - d / 2.0, d);
        }
        Leader::Hyphen => {
            let h = (s * 0.06).max(1.0);
            repeat(s * 0.33, s * 0.2, baseline - s * 0.27 - h / 2.0, h);
        }
        Leader::Underscore | Leader::Heavy => {
            let h = if fill.leader == Leader::Heavy {
                (s * 0.12).max(1.5)
            } else {
                (s * 0.06).max(1.0)
            };
            let (a, b) = (x0 + s * 0.04, x1 - s * 0.04);
            if b > a {
                push(a, baseline + s * 0.1, b - a, h);
            }
        }
    }
}

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
            variations: variations(&synthesis),
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
    fn strikes_become_rules_over_their_glyphs() {
        let mut engine = Engine::new();
        let mut p = Paragraph::new("keep gone keep");
        p.strikes.push(5..9);
        let doc = engine.layout_document(std::slice::from_ref(&p), PageSetup::a4());
        let rules = doc.strike_rules(0, std::slice::from_ref(&p));
        assert_eq!(rules.len(), 1);
        let caret = |i| doc.caret(0, i).unwrap().x;
        let [x, _, w, h] = rules[0].rect;
        assert!((x - caret(5)).abs() < 1.0, "{x} vs {}", caret(5));
        assert!((x + w - caret(9)).abs() < 1.0);
        assert!(h >= 1.0);
        assert!(
            doc.strike_rules(0, &[Paragraph::new("keep gone keep")])
                .is_empty()
        );
    }

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
            columns: Vec::new(),
            sep: false,
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
                columns: Vec::new(),
                sep: false,
            },
            SectionFlow {
                first_block: 1,
                setup: s,
                first_setup: None,
                even_setup: None,
                start: SectionBreak::Continuous,
                restart: None,
                columns: Vec::new(),
                sep: false,
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
    fn unwrapped_lines_align_in_the_column() {
        let mut engine = Engine::new();
        let setup = PageSetup {
            width: 200.0,
            height: 1000.0,
            margin_top: 0.0,
            margin_bottom: 0.0,
            margin_left: 0.0,
            margin_right: 0.0,
        };
        let para = |text: &str, align| {
            let mut p = Paragraph::new(text);
            p.align = align;
            p.no_wrap = true;
            p
        };
        let long = "A line far wider than the two hundred px column it is laid out in";
        let doc = engine.layout_document(
            &[
                para("Short", Align::Center),
                para(long, Align::Center),
                para(long, Align::End),
                para("Left", Align::Start),
            ],
            setup,
        );
        let extent = |para: usize| {
            let runs: Vec<_> = doc
                .glyph_runs(0)
                .into_iter()
                .filter(|r| r.para == para)
                .collect();
            let x0 = runs.iter().map(|r| r.origin_x).fold(f32::MAX, f32::min);
            let x1 = runs
                .iter()
                .map(|r| r.origin_x + r.advance)
                .fold(f32::MIN, f32::max);
            (x0, x1)
        };
        for (i, l) in doc.layouts.iter().enumerate() {
            assert_eq!(l.len(), 1, "paragraph {i} is one line");
        }
        let (a, b) = extent(0);
        assert!(((a + b) / 2.0 - 100.0).abs() < 0.5, "centred: {a} {b}");
        let (a, b) = extent(1);
        assert!(a < 0.0 && b > 200.0, "overflows both sides: {a} {b}");
        assert!(((a + b) / 2.0 - 100.0).abs() < 0.5, "centred: {a} {b}");
        let (a, b) = extent(2);
        assert!(a < 0.0 && (b - 200.0).abs() < 0.5, "right: {a} {b}");
        assert!(extent(3).0.abs() < 0.5);
        // Carets follow the shifted lines.
        let c = doc.caret(2, long.len()).unwrap();
        assert!((c.x - 200.0).abs() < 0.5, "{c:?}");
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
    fn exact_lines_keep_their_height_and_cut_tall_pictures() {
        let mut engine = Engine::new();
        let line = 54.0 * PT;
        let para = |pic: f32| {
            let mut p = Paragraph::new(format!("ab{OBJECT}cd"));
            p.line_height = Some(LineHeight::Absolute(line));
            p.space_before_pt = 0.0;
            p.space_after_pt = 0.0;
            p.objects = vec![InlineObject {
                index: 2,
                kind: ObjectKind::Image,
                width: 50.0,
                height: pic,
                label: String::new(),
            }];
            p
        };
        // As tall as the line, taller, and short enough to fit.
        let paras = [para(line), para(96.0 * PT), para(10.0)];
        let doc = engine.layout_document(&paras, PageSetup::a4());
        let lines = &doc.pages[0].lines;
        for (k, l) in lines.iter().enumerate() {
            assert!((l.height - line).abs() < 0.01, "line {k}: {}", l.height);
        }
        let top = PageSetup::a4().margin_top;
        for (k, l) in lines.iter().enumerate() {
            assert!((l.top - (top + k as f32 * line)).abs() < 0.01, "{l:?}");
        }
        let objs = doc.objects(0);
        // Cut at the line's top, the bottom on the baseline.
        for (o, full) in objs.iter().zip([line, 96.0 * PT]) {
            let line_top = lines[o.para].top;
            assert!((o.rect[1] - line_top).abs() < 0.01, "{o:?}");
            assert!(o.rect[3] < full);
            assert_eq!(o.whole[3], full);
            let bottom = |r: [f32; 4]| r[1] + r[3];
            assert!((bottom(o.rect) - bottom(o.whole)).abs() < 0.01);
            // On the baseline, four fifths of the way down.
            assert!((bottom(o.rect) - (line_top + 0.8 * line)).abs() < 0.01);
        }
        // The text's baseline too.
        let run = doc.glyph_runs(0).into_iter().next().unwrap();
        assert!(
            (run.baseline - (top + 0.8 * line)).abs() < 0.01,
            "{}",
            run.baseline
        );
        // A short picture isn't cut.
        assert_eq!(objs[2].rect, objs[2].whole);
        assert_eq!(objs[2].rect[3], 10.0);
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

    #[test]
    fn a_page_break_ending_a_paragraph_breaks_once() {
        // Word: the paragraph mark after the break opens the next page,
        // and the next paragraph follows on that page.
        let mut engine = Engine::new();
        let paras = vec![
            Paragraph::new(format!("a{PAGE_BREAK}")),
            Paragraph::new("b"),
        ];
        let doc = engine.layout_document(&paras, PageSetup::a4());
        assert_eq!(doc.pages.len(), 2);
        assert_eq!(doc.caret(0, 2).unwrap().page, 1);
        assert_eq!(doc.caret(1, 0).unwrap().page, 1);
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
                            fill: None,
                        })
                        .collect(),
                    min_height: 0.0,
                    exact: false,
                    header: false,
                    cant_split: false,
                    keep_next: false,
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

#[cfg(test)]
mod tab_tests {
    use super::*;

    fn x_of(doc: &PagedDocument, i: usize) -> f32 {
        doc.caret(0, i).unwrap().x
    }

    /// A one-column table of `rows` rows, each cell one paragraph; returns
    /// the paginated document. `tall` rows get `lines` lines of text.
    fn split_table(
        rows: usize,
        tall: &[usize],
        lines: usize,
        header: bool,
        cant_split: bool,
    ) -> (PagedDocument, Vec<Paragraph>) {
        let mut engine = Engine::new();
        let setup = PageSetup::a4();
        let mut paras = Vec::new();
        for r in 0..rows {
            let text = if tall.contains(&r) {
                (0..lines)
                    .map(|k| format!("r{r} line {k}"))
                    .collect::<Vec<_>>()
                    .join("\u{2028}")
            } else {
                format!("row {r}")
            };
            paras.push(Paragraph::new(text));
        }
        let rule = Some(Rule {
            width: 1.0,
            color: Rgba::BLACK,
        });
        let t = TableBlock {
            x: 0.0,
            columns: vec![300.0],
            padding: [0.0, 7.2, 0.0, 7.2],
            rows: (0..rows)
                .map(|r| TableRow {
                    cells: vec![TableCell {
                        col: 0,
                        span: 1,
                        paras: r..r + 1,
                        merge: Merge::None,
                        borders: [rule; 4],
                        fill: None,
                    }],
                    min_height: 0.0,
                    exact: false,
                    header: header && r == 0,
                    cant_split: cant_split && tall.contains(&r),
                    keep_next: false,
                })
                .collect(),
        };
        let w = t.text_width(&t.rows[0].cells[0]);
        let layouts: Vec<_> = paras
            .iter()
            .map(|p| Arc::new(engine.layout_paragraph(p, w)))
            .collect();
        let doc = PagedDocument::paginate_blocks(&paras, layouts, &[Block::Table(t)], setup);
        (doc, paras)
    }

    #[test]
    fn tall_rows_break_across_pages_at_lines() {
        // Row 5 has 80 lines: more than one page holds.
        let (doc, _) = split_table(8, &[5], 80, false, false);
        assert!(doc.pages.len() >= 2);
        let first = doc.caret(5, 0).unwrap();
        assert_eq!(first.page, 0, "the tall row starts right after row 4");
        let row4 = doc.caret(4, 0).unwrap();
        assert!(first.top > row4.top && first.top - row4.top < 40.0);
        // Its lines continue on the next page, every line placed once.
        let pages: Vec<usize> = doc
            .pages
            .iter()
            .map(|p| p.lines.iter().filter(|l| l.para == 5).count())
            .collect();
        assert!(pages[0] > 0 && pages[1] > 0, "{pages:?}");
        assert_eq!(pages.iter().sum::<usize>(), 80);
        let bottom = doc.pages[0].setup.height - doc.pages[0].setup.margin_bottom;
        for l in &doc.pages[0].lines {
            assert!(l.top + l.height <= bottom + 0.01);
        }
        // Borders close the row on both pages.
        assert!(!doc.pages[1].rules.is_empty());
        assert_eq!(doc.caret(6, 0).unwrap().page, doc.pages.len() - 1);
    }

    #[test]
    fn cant_split_rows_move_whole() {
        // Row 30 has 20 lines and doesn't fit what's left of page 1.
        let (doc, paras) = split_table(40, &[30], 20, false, true);
        let first = doc.caret(30, 0).unwrap();
        let last = doc.caret(30, paras[30].text.len()).unwrap();
        assert_eq!(first.page, last.page, "kept on one page");
        assert_eq!(first.page, 1);
        let (split, _) = split_table(40, &[30], 20, false, false);
        let pages: std::collections::BTreeSet<usize> = split
            .pages
            .iter()
            .enumerate()
            .filter(|(_, p)| p.lines.iter().any(|l| l.para == 30))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(pages.len(), 2, "without cantSplit it breaks");
    }

    #[test]
    fn header_rows_kept_with_the_next_row() {
        // `n` filler paragraphs, then a table of a header row and three
        // rows: for some `n` the header row ends page 1 alone, unless it
        // keeps with the next row.
        let alone = |n: usize, keep: bool| -> bool {
            let mut engine = Engine::new();
            let setup = PageSetup::a4();
            let mut paras: Vec<Paragraph> = (0..n)
                .map(|i| Paragraph::new(format!("para {i}")))
                .collect();
            let first = paras.len();
            paras.extend((0..4).map(|r| Paragraph::new(format!("row {r}"))));
            let t = TableBlock {
                x: 0.0,
                columns: vec![300.0],
                padding: [0.0, 7.2, 0.0, 7.2],
                rows: (0..4)
                    .map(|r| TableRow {
                        cells: vec![TableCell {
                            col: 0,
                            span: 1,
                            paras: first + r..first + r + 1,
                            merge: Merge::None,
                            borders: [None; 4],
                            fill: None,
                        }],
                        min_height: 0.0,
                        exact: false,
                        header: r == 0,
                        cant_split: true,
                        keep_next: keep && r == 0,
                    })
                    .collect(),
            };
            let w = 300.0 - 14.4;
            let layouts: Vec<_> =
                paras
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        Arc::new(engine.layout_paragraph(
                            p,
                            if i >= first { w } else { setup.content_width() },
                        ))
                    })
                    .collect();
            let mut blocks: Vec<Block> = (0..first).map(Block::Para).collect();
            blocks.push(Block::Table(t));
            let doc = PagedDocument::paginate_blocks(&paras, layouts, &blocks, setup);
            doc.caret(first, 0).unwrap().page < doc.caret(first + 1, 0).unwrap().page
        };
        let hits: Vec<usize> = (20..90).filter(|&n| alone(n, false)).collect();
        assert!(!hits.is_empty(), "some n leaves the header row alone");
        for n in hits {
            assert!(!alone(n, true), "n = {n}: kept with the next row");
        }
    }

    #[test]
    fn header_rows_repeat_on_every_page() {
        let (doc, paras) = split_table(120, &[], 1, true, false);
        assert!(doc.pages.len() >= 3);
        for (i, page) in doc.pages.iter().enumerate() {
            let first = page.lines.first().unwrap();
            assert_eq!(first.para, 0, "page {i} starts with the header row");
            assert_eq!(first.repeat, i > 0, "repeats are drawn copies");
            // The repeat shows the header's text.
            let runs = doc.glyph_runs(i);
            assert!(runs.iter().any(|r| r.para == 0));
        }
        // The caret stays on the real header row; clicks skip the copies.
        assert_eq!(doc.caret(0, 0).unwrap().page, 0);
        let l = doc.pages[1].lines[0];
        assert_ne!(
            doc.hit_test(1, l.x + 2.0, l.top + 2.0).map(|h| h.0),
            Some(0)
        );
        let _ = paras;
        // A table of header rows only doesn't repeat.
        let (only, _) = split_table(1, &[], 1, true, false);
        assert!(only.pages[0].lines.iter().all(|l| !l.repeat));
    }

    #[test]
    fn footnotes_take_room_at_the_page_bottom() {
        let mut engine = Engine::new();
        let setup = PageSetup::a4();
        let mut paras: Vec<Paragraph> = (0..60)
            .map(|i| Paragraph::new(format!("para {i}")))
            .collect();
        paras[3].notes = vec![(4, 120.0)];
        let layouts: Vec<_> = paras
            .iter()
            .map(|p| Arc::new(engine.layout_paragraph(p, setup.content_width())))
            .collect();
        let with = PagedDocument::paginate(&paras, layouts.clone(), setup);
        let mut plain = paras.clone();
        plain[3].notes.clear();
        let without = PagedDocument::paginate(&plain, layouts, setup);
        assert_eq!(with.pages[0].notes, vec![(3, 4)]);
        assert!((with.pages[0].notes_height - (120.0 + NOTE_SEPARATOR)).abs() < 0.01);
        let bottom = setup.height - setup.margin_bottom - with.pages[0].notes_height;
        for l in &with.pages[0].lines {
            assert!(l.top + l.height <= bottom + 0.01);
        }
        assert!(with.pages[0].lines.len() < without.pages[0].lines.len());
        assert!(with.pages[1].notes.is_empty());
    }

    #[test]
    fn tabs_reach_default_and_custom_stops() {
        let mut engine = Engine::new();
        let setup = PageSetup::a4();
        // Default stops every 1.27 cm from the column edge.
        let p = Paragraph::new("a\tb");
        let doc = engine.layout_document(std::slice::from_ref(&p), setup);
        let col = setup.margin_left;
        assert!(
            (x_of(&doc, 2) - col - 12.7 * MM).abs() < 0.6,
            "{}",
            x_of(&doc, 2) - col
        );
        // A left stop at 5 cm, then a right stop at 15 cm.
        let mut p = Paragraph::new("Total\tQty\t1.000.000");
        p.tabs = vec![
            TabStop::new(50.0 * MM, TabAlign::Left),
            TabStop::new(150.0 * MM, TabAlign::Right),
        ];
        let doc = engine.layout_document(std::slice::from_ref(&p), setup);
        let qty = "Total\t".len();
        assert!((x_of(&doc, qty) - col - 50.0 * MM).abs() < 0.6);
        let end = p.text.len();
        assert!(
            (x_of(&doc, end) - col - 150.0 * MM).abs() < 1.0,
            "right tab ends at the stop"
        );
        assert_eq!(doc.pages[0].lines.len(), 1);
    }

    /// One paragraph on an A4 page; returns the layout and the column's left.
    fn one(p: &Paragraph) -> (PagedDocument, f32) {
        let mut engine = Engine::new();
        let setup = PageSetup::a4();
        let doc = engine.layout_document(std::slice::from_ref(p), setup);
        (doc, setup.margin_left)
    }

    #[test]
    fn center_and_decimal_stops() {
        // Centre stop at 8 cm: the word after the tab is centred on it.
        let mut p = Paragraph::new("a\tmid");
        p.tabs = vec![TabStop::new(80.0 * MM, TabAlign::Center)];
        let (doc, col) = one(&p);
        let (a, b) = (x_of(&doc, 2), x_of(&doc, 5));
        assert!(b > a);
        assert!(((a + b) / 2.0 - col - 80.0 * MM).abs() < 0.6, "{a} {b}");
        // Decimal stop at 10 cm: the point sits on it, whatever the digits'
        // widths; without a point the number ends there.
        for text in ["x\t12.5", "x\t1234.567", "x\t7"] {
            let mut p = Paragraph::new(text);
            p.tabs = vec![TabStop::new(100.0 * MM, TabAlign::Decimal)];
            let (doc, col) = one(&p);
            let at = text.find('.').unwrap_or(text.len());
            assert!(
                (x_of(&doc, at) - col - 100.0 * MM).abs() < 0.6,
                "{text}: {}",
                x_of(&doc, at) - col
            );
        }
    }

    #[test]
    fn decimal_stops_follow_the_documents_decimal_symbol() {
        // Indonesian Word: "1.250,75" lines up on the comma.
        let mut p = Paragraph::new("x\t1.250,75");
        p.decimal = ',';
        p.tabs = vec![TabStop::new(100.0 * MM, TabAlign::Decimal)];
        let (doc, col) = one(&p);
        let at = p.text.find(',').unwrap();
        assert!((x_of(&doc, at) - col - 100.0 * MM).abs() < 0.6);
    }

    #[test]
    fn bar_tabs_are_lines_not_stops() {
        let mut p = Paragraph::new("a\tb");
        p.tabs = vec![TabStop::new(5.0 * MM, TabAlign::Bar)];
        let (doc, col) = one(&p);
        // The tab skips the bar and goes to the default stop.
        assert!((x_of(&doc, 2) - col - 12.7 * MM).abs() < 0.6);
        let rules = doc.tab_rules(0);
        assert_eq!(rules.len(), 1);
        let r = rules[0].rect;
        assert!((r[0] + r[2] / 2.0 - col - 5.0 * MM).abs() < 0.01);
        assert!(r[3] > 5.0, "spans the line");
    }

    #[test]
    fn dot_leaders_fill_the_tab_and_line_up() {
        // A table-of-contents line: right stop with dots at the right
        // margin; a second line with a different title.
        let setup = PageSetup::a4();
        let right = setup.content_width();
        let paras: Vec<Paragraph> = ["Introduction\t1", "Scope\t12"]
            .iter()
            .map(|t| {
                let mut p = Paragraph::new(*t);
                p.tabs = vec![TabStop::new(right, TabAlign::Right).with_leader(Leader::Dot)];
                p
            })
            .collect();
        let mut engine = Engine::new();
        let doc = engine.layout_document(&paras, setup);
        let col = setup.margin_left;
        // Page numbers end at the right margin, on one line each.
        for (i, p) in paras.iter().enumerate() {
            let end = doc.caret(i, p.text.len()).unwrap().x;
            assert!((end - col - right).abs() < 0.6, "{end}");
        }
        assert_eq!(doc.pages[0].lines.len(), 2);
        let rules = doc.tab_rules(0);
        assert!(rules.len() > 40, "dots: {}", rules.len());
        // Every dot is between its title and its number, square, on the
        // baseline area of its line.
        for (i, p) in paras.iter().enumerate() {
            let tab = p.text.find('\t').unwrap();
            let (x0, x1) = (
                doc.caret(i, tab).unwrap().x,
                doc.caret(i, p.text.len() - if i == 0 { 1 } else { 2 })
                    .unwrap()
                    .x,
            );
            let c = doc.caret(i, 0).unwrap();
            let dots: Vec<_> = rules
                .iter()
                .filter(|r| r.rect[1] > c.top && r.rect[1] < c.top + c.height)
                .collect();
            assert!(!dots.is_empty());
            for d in &dots {
                assert!(
                    d.rect[0] >= x0 && d.rect[0] + d.rect[2] <= x1,
                    "{:?}",
                    d.rect
                );
                assert_eq!(d.rect[2], d.rect[3]);
            }
        }
        // Same grid on both lines: the last dots line up.
        let ends: Vec<f32> = [0usize, 1]
            .iter()
            .map(|&i| {
                let c = doc.caret(i, 0).unwrap();
                rules
                    .iter()
                    .filter(|r| r.rect[1] > c.top && r.rect[1] < c.top + c.height)
                    .map(|r| r.rect[0])
                    .fold(f32::MIN, f32::max)
            })
            .collect();
        let step = 11.0 * PT * 0.25;
        let k = (ends[0] - ends[1]) / step;
        assert!((k - k.round()).abs() < 0.01, "{ends:?}");
    }

    #[test]
    fn other_leaders_and_no_leader() {
        for (leader, many) in [
            (Leader::Hyphen, true),
            (Leader::MiddleDot, true),
            (Leader::Underscore, false),
            (Leader::Heavy, false),
            (Leader::None, false),
        ] {
            let mut p = Paragraph::new("a\tb");
            p.tabs = vec![TabStop::new(100.0 * MM, TabAlign::Left).with_leader(leader)];
            let (doc, _) = one(&p);
            let n = doc.tab_rules(0).len();
            match (leader, many) {
                (Leader::None, _) => assert_eq!(n, 0),
                (_, true) => assert!(n > 10, "{leader:?}: {n}"),
                (_, false) => assert_eq!(n, 1, "{leader:?}"),
            }
        }
    }

    #[test]
    fn stops_past_the_right_indent_and_hanging_indents() {
        // A right stop in the margin is honoured, as in Word 2013+: the
        // text after it ends there, on the same line.
        let setup = PageSetup::a4();
        let mut p = Paragraph::new("a\t9");
        p.tabs = vec![TabStop::new(setup.content_width() + 40.0, TabAlign::Right)];
        let (doc, col) = one(&p);
        let end = x_of(&doc, p.text.len());
        assert!(end <= col + setup.content_width() + 40.01, "{end}");
        assert!(end > col + setup.content_width() + 39.0, "{end}");
        assert_eq!(doc.pages[0].lines.len(), 1);
        // A left stop there too: the text starts at it.
        let mut p = Paragraph::new("a\tb");
        p.tabs = vec![TabStop::new(setup.content_width() + 20.0, TabAlign::Left)];
        let (doc, col) = one(&p);
        let b = x_of(&doc, 2);
        assert!((b - col - setup.content_width() - 20.0).abs() < 0.6, "{b}");
        assert_eq!(doc.pages[0].lines.len(), 1);
        // Default stops still end at the margin: no stop past it.
        let mut p = Paragraph::new("a\t9");
        p.tabs = vec![TabStop::new(30.0 * MM, TabAlign::Left)];
        p.default_tab = setup.content_width() * 0.75;
        let (doc, col) = one(&p);
        let x = x_of(&doc, 3);
        assert!(x <= col + setup.content_width() + 0.01, "{x}");
        // Hanging indent: the first tab goes to the left indent, before a
        // later custom stop and before the default ones.
        let mut p = Paragraph::new("1.\tItem");
        p.indent_left_pt = 18.0;
        p.first_line_pt = -18.0;
        p.tabs = vec![TabStop::new(60.0 * MM, TabAlign::Left)];
        let (doc, col) = one(&p);
        assert!((x_of(&doc, 3) - col - 18.0 * PT).abs() < 0.6);
    }
}

#[cfg(test)]
mod column_tests {
    use super::*;

    const GAP: f32 = 40.0;

    /// A4 split into two columns `GAP` px apart (or the widths given).
    fn two_columns(widths: Option<(f32, f32)>) -> Vec<(f32, f32)> {
        let w = PageSetup::a4().content_width();
        let (a, b) = widths.unwrap_or(((w - GAP) / 2.0, (w - GAP) / 2.0));
        vec![(0.0, a), (a + GAP, b)]
    }

    fn section(first_block: usize, start: SectionBreak, columns: Vec<(f32, f32)>) -> SectionFlow {
        SectionFlow {
            first_block,
            setup: PageSetup::a4(),
            first_setup: None,
            even_setup: None,
            start,
            restart: None,
            columns,
            sep: true,
        }
    }

    /// Lays `paras` out, each at the width of the column its section
    /// starts with.
    fn lay(paras: &[Paragraph], sections: &[SectionFlow]) -> PagedDocument {
        let mut engine = Engine::new();
        let full = PageSetup::a4().content_width();
        let width_of = |i: usize| {
            let s = sections.iter().rev().find(|s| s.first_block <= i).unwrap();
            s.columns.first().map_or(full, |c| c.1)
        };
        let layouts = paras
            .iter()
            .enumerate()
            .map(|(i, p)| Arc::new(engine.layout_paragraph(p, width_of(i))))
            .collect();
        let blocks: Vec<Block> = (0..paras.len()).map(Block::Para).collect();
        PagedDocument::paginate_sections(paras, layouts, &blocks, sections)
    }

    fn short(n: usize) -> Vec<Paragraph> {
        (0..n).map(|i| Paragraph::new(format!("p{i}"))).collect()
    }

    fn left(page: &Page, para: usize) -> f32 {
        page.lines.iter().find(|l| l.para == para).unwrap().x
    }

    #[test]
    fn text_fills_one_column_then_the_next() {
        let s = PageSetup::a4();
        let cols = two_columns(None);
        let doc = lay(
            &short(200),
            &[section(0, SectionBreak::NextPage, cols.clone())],
        );
        let page = &doc.pages[0];
        let in_first = page.lines.iter().filter(|l| l.x == s.margin_left).count();
        let in_second = page
            .lines
            .iter()
            .filter(|l| l.x == s.margin_left + cols[1].0)
            .count();
        assert!(in_first > 10 && in_second > 10, "{in_first} {in_second}");
        assert_eq!(in_first, in_second, "a full page has two full columns");
        assert_eq!(in_first + in_second, page.lines.len());
        // The second column starts at the top, right after the first.
        let first_of_second = page.lines.iter().find(|l| l.x > s.margin_left).unwrap();
        assert_eq!(first_of_second.top, s.margin_top);
        assert_eq!(first_of_second.para, in_first);
        // Twice as much text per page as without columns.
        let one = lay(
            &short(200),
            &[section(0, SectionBreak::NextPage, Vec::new())],
        );
        assert!(doc.pages.len() * 2 <= one.pages.len() + 1);
        // The column boxes and the line between them.
        assert_eq!(page.columns.len(), 1);
        assert_eq!(page.columns[0].len(), 2);
        let mid = s.margin_left + cols[0].1 + GAP / 2.0;
        assert!(
            page.rules
                .iter()
                .any(|r| (r.rect[0] + r.rect[2] / 2.0 - mid).abs() < 0.5 && r.rect[3] > 100.0)
        );
    }

    #[test]
    fn column_break_moves_to_the_next_column() {
        let s = PageSetup::a4();
        let cols = two_columns(None);
        let mut paras = short(3);
        paras[1] = Paragraph::new(format!("a{COLUMN_BREAK}b"));
        let doc = lay(&paras, &[section(0, SectionBreak::NextPage, cols.clone())]);
        assert_eq!(doc.pages.len(), 1);
        let page = &doc.pages[0];
        let b = page
            .lines
            .iter()
            .find(|l| l.para == 1 && l.line == 1)
            .unwrap();
        assert_eq!(b.x, s.margin_left + cols[1].0);
        assert_eq!(b.top, s.margin_top);
        assert_eq!(left(page, 2), s.margin_left + cols[1].0);
        // From the last column it goes to the next page.
        let mut paras = short(2);
        paras[0] = Paragraph::new(format!("a{COLUMN_BREAK}b{COLUMN_BREAK}c"));
        let doc = lay(&paras, &[section(0, SectionBreak::NextPage, cols)]);
        assert_eq!(doc.pages.len(), 2);
        assert_eq!(doc.caret(0, 4).unwrap().page, 1);
        // Without columns a column break acts as a page break.
        let mut paras = short(2);
        paras[0] = Paragraph::new(format!("a{COLUMN_BREAK}"));
        let doc = lay(&paras, &[section(0, SectionBreak::NextPage, Vec::new())]);
        assert_eq!(doc.pages.len(), 2);
    }

    #[test]
    fn continuous_break_balances_the_columns_before_it() {
        let s = PageSetup::a4();
        let cols = two_columns(None);
        let mut paras = short(11);
        paras.push(Paragraph::new("after"));
        let doc = lay(
            &paras,
            &[
                section(0, SectionBreak::NextPage, cols.clone()),
                section(11, SectionBreak::Continuous, Vec::new()),
            ],
        );
        assert_eq!(doc.pages.len(), 1);
        let page = &doc.pages[0];
        let firsts = page
            .lines
            .iter()
            .filter(|l| l.para < 11 && l.x == s.margin_left)
            .count();
        let seconds = page
            .lines
            .iter()
            .filter(|l| l.para < 11 && l.x > s.margin_left)
            .count();
        assert_eq!(
            (firsts, seconds),
            (6, 5),
            "balanced, the first column longer"
        );
        // The next section starts under the longer column, full width.
        let bottom = page
            .lines
            .iter()
            .filter(|l| l.para < 11)
            .map(|l| l.top + l.height)
            .fold(0.0f32, f32::max);
        let after = page.lines.iter().find(|l| l.para == 11).unwrap();
        assert!(after.top >= bottom - 0.01 && after.top < bottom + 30.0);
        assert_eq!(after.x, s.margin_left);
        assert_eq!(after.width, s.content_width());
        // The line between the columns is as long as the longer column.
        let rule = page.rules.iter().find(|r| r.rect[2] < 1.0).unwrap();
        assert!((rule.rect[1] + rule.rect[3] - bottom).abs() < 30.0);
    }

    #[test]
    fn continuous_columns_start_mid_page() {
        let cols = two_columns(None);
        let doc = lay(
            &short(5),
            &[
                section(0, SectionBreak::NextPage, Vec::new()),
                section(2, SectionBreak::Continuous, cols.clone()),
            ],
        );
        assert_eq!(doc.pages.len(), 1);
        let page = &doc.pages[0];
        let p1 = page.lines.iter().find(|l| l.para == 1).unwrap();
        let p2 = page.lines.iter().find(|l| l.para == 2).unwrap();
        assert!(p2.top >= p1.top + p1.height - 0.01);
        assert_eq!(p2.width, cols[0].1);
        let top = page.columns[0][0][1];
        assert!(top >= p1.top + p1.height && top <= p2.top + 0.01);
    }

    #[test]
    fn unequal_columns_break_lines_at_their_own_width() {
        let s = PageSetup::a4();
        let cols = two_columns(Some((150.0, s.content_width() - 150.0 - GAP)));
        let words = "word ".repeat(1500);
        let doc = lay(
            &[Paragraph::new(words)],
            &[section(0, SectionBreak::NextPage, cols.clone())],
        );
        let page = &doc.pages[0];
        let narrow: Vec<&PlacedLine> = page
            .lines
            .iter()
            .filter(|l| l.x < s.margin_left + 100.0)
            .collect();
        let wide: Vec<&PlacedLine> = page
            .lines
            .iter()
            .filter(|l| l.x > s.margin_left + 100.0)
            .collect();
        assert!(!narrow.is_empty() && !wide.is_empty());
        assert!(narrow.iter().all(|l| l.width <= 150.5));
        assert!(wide.iter().all(|l| l.width > 300.0));
        // Each wide line holds more text than a narrow one.
        let range = |l: &PlacedLine| {
            let r = doc.layouts[0].layout.get(l.line).unwrap().text_range();
            r.end - r.start
        };
        assert!(range(wide[0]) > range(narrow[0]) * 2);
    }

    #[test]
    fn space_before_is_dropped_at_later_column_tops() {
        let s = PageSetup::a4();
        // Columns starting mid-page: the paragraph that opens the second
        // column loses its space before, as at a page top (Word).
        let cols = two_columns(None);
        let mut paras = short(40);
        for p in &mut paras {
            p.space_before_pt = 30.0;
        }
        let doc = lay(
            &paras,
            &[
                section(0, SectionBreak::NextPage, Vec::new()),
                section(2, SectionBreak::Continuous, cols.clone()),
            ],
        );
        let page = &doc.pages[0];
        let top = page.columns[0][0][1];
        let first = page.lines.iter().find(|l| l.para == 2).unwrap();
        // The first column keeps it (the text before is on the page).
        assert!(first.top >= top + 30.0 * PT - 0.5, "{} {top}", first.top);
        let second = page.lines.iter().find(|l| l.x > first.x + 100.0).unwrap();
        assert!(top > s.margin_top + 50.0);
        assert!((second.top - top).abs() < 0.5, "{} {top}", second.top);
    }

    #[test]
    fn a_break_mid_paragraph_rebreaks_in_a_column_of_another_width() {
        let s = PageSetup::a4();
        let cols = two_columns(Some((150.0, s.content_width() - 150.0 - GAP)));
        let words = "word ".repeat(60);
        let doc = lay(
            &[Paragraph::new(format!("{words}{COLUMN_BREAK}{words}"))],
            &[section(0, SectionBreak::NextPage, cols.clone())],
        );
        assert_eq!(doc.pages.len(), 1);
        let page = &doc.pages[0];
        let wide: Vec<&PlacedLine> = page
            .lines
            .iter()
            .filter(|l| l.x > s.margin_left + 100.0)
            .collect();
        assert!(!wide.is_empty());
        assert!(wide.iter().all(|l| l.width > 300.0));
        // The text after the break starts the wide column, its lines
        // broken at its width.
        let first = doc.layouts[0]
            .layout
            .get(wide[0].line)
            .unwrap()
            .text_range();
        assert_eq!(first.start, words.len() + COLUMN_BREAK.len_utf8());
        assert!((wide[0].top - s.margin_top).abs() < 0.5);
        assert!(wide.len() * 2 < page.lines.len() - wide.len());
    }

    #[test]
    fn footnotes_in_unequal_columns_use_their_column_width() {
        let s = PageSetup::a4();
        let cols = two_columns(Some((150.0, s.content_width() - 150.0 - GAP)));
        let mut paras = short(80);
        // A reference in each column: tall at the narrow width, short at
        // the wide one.
        for i in [2usize, 40] {
            let p = &mut paras[i];
            p.notes.push((0, 60.0));
            p.note_widths.push(NoteWidth {
                width: cols[1].1,
                notes: vec![(0, 20.0)],
                splits: Vec::new(),
            });
        }
        let doc = lay(&paras, &[section(0, SectionBreak::NextPage, cols.clone())]);
        let page = &doc.pages[0];
        assert_eq!(page.note_columns.len(), 2);
        let (a, b) = (&page.note_columns[0], &page.note_columns[1]);
        assert!(a.width < 151.0 && b.width > 300.0);
        assert!(
            (a.height - (60.0 + NOTE_SEPARATOR)).abs() < 0.5,
            "{}",
            a.height
        );
        assert!(
            (b.height - (20.0 + NOTE_SEPARATOR)).abs() < 0.5,
            "{}",
            b.height
        );
    }

    #[test]
    fn column_at_finds_the_column_of_a_point() {
        let s = PageSetup::a4();
        let cols = two_columns(None);
        let doc = lay(
            &short(200),
            &[section(0, SectionBreak::NextPage, cols.clone())],
        );
        let page = &doc.pages[0];
        let y = s.margin_top + 10.0;
        assert_eq!(
            page.column_at(s.margin_left + 5.0, y),
            Some((s.margin_left, cols[0].1))
        );
        let second = s.margin_left + cols[1].0;
        assert_eq!(page.column_at(second + 5.0, y), Some((second, cols[1].1)));
        let one = lay(&short(3), &[section(0, SectionBreak::NextPage, Vec::new())]);
        assert_eq!(one.pages[0].column_at(s.margin_left + 5.0, y), None);
    }

    #[test]
    fn down_and_up_move_between_columns() {
        let s = PageSetup::a4();
        let cols = two_columns(None);
        let doc = lay(
            &short(200),
            &[section(0, SectionBreak::NextPage, cols.clone())],
        );
        let page = &doc.pages[0];
        let last_first = page
            .lines
            .iter()
            .rfind(|l| l.x == s.margin_left)
            .unwrap()
            .para;
        let x = s.margin_left + 2.0;
        let down = doc.vertical(last_first, 0, x, true).unwrap();
        assert_eq!(down.0, last_first + 1, "top of the next column");
        assert_eq!(doc.caret(down.0, down.1).unwrap().page, 0);
        let up = doc
            .vertical(last_first + 1, 0, x + cols[1].0, false)
            .unwrap();
        assert_eq!(up.0, last_first);
        // Off the last column, on to the next page's first column.
        let last = page.lines.last().unwrap().para;
        let next = doc.vertical(last, 0, x + cols[1].0, true).unwrap();
        assert_eq!(next.0, last + 1);
        let c = doc.caret(next.0, next.1).unwrap();
        assert_eq!((c.page, c.x < s.margin_left + cols[0].1), (1, true));
        // Clicking in the second column hits its text.
        let (para, _) = doc.hit_test(0, x + cols[1].0, s.margin_top + 2.0).unwrap();
        assert_eq!(para, last_first + 1);
    }
}
