//! The presentation model: slides of shapes; text bodies of paragraphs of
//! styled runs.
//!
//! Units are the file's: positions and sizes in EMU (914 400 per inch,
//! 9 525 per CSS pixel at 96 dpi), font sizes in hundredths of a point.
//! Everything the model doesn't map is kept as raw XML next to the node it
//! belongs to (`Extra`, and each shape's source element) and written back
//! verbatim (plan §5).

use std::ops::Range;
use std::sync::Arc;

use docs_engine::Rgb;

use crate::theme::ListStyle;

pub type Emu = i64;
/// EMU per CSS pixel (96 dpi).
pub const EMU_PER_PX: f32 = 9525.0;
/// EMU per point.
pub const EMU_PER_PT: f32 = 12700.0;

/// Slide size (`p:sldSz`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlideSize {
    pub cx: Emu,
    pub cy: Emu,
}

impl SlideSize {
    /// 16:9, 13.333 × 7.5 in.
    pub const WIDE: Self = Self {
        cx: 12_192_000,
        cy: 6_858_000,
    };
}

/// Offset and extent of a shape (`a:xfrm`). Rotation and flips stay in the
/// source XML (not modelled; kept when the shape moves).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Xfrm {
    pub x: Emu,
    pub y: Emu,
    pub cx: Emu,
    pub cy: Emu,
}

impl Xfrm {
    pub const fn new(x: Emu, y: Emu, cx: Emu, cy: Emu) -> Self {
        Self { x, y, cx, cy }
    }

    pub fn contains(&self, x: Emu, y: Emu) -> bool {
        x >= self.x && x <= self.x + self.cx && y >= self.y && y <= self.y + self.cy
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

impl Align {
    pub fn from_attr(v: &str) -> Option<Self> {
        Some(match v {
            "l" => Self::Left,
            "ctr" => Self::Center,
            "r" => Self::Right,
            "just" | "dist" | "justLow" | "thaiDist" => Self::Justify,
            _ => return None,
        })
    }

    pub fn attr(self) -> &'static str {
        match self {
            Self::Left => "l",
            Self::Center => "ctr",
            Self::Right => "r",
            Self::Justify => "just",
        }
    }
}

/// Unmodelled XML of an element: its other attributes (raw, escaped as in
/// the source) and raw child elements around the modelled ones.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Extra {
    pub attrs: Vec<(String, String)>,
    /// Children that come before the modelled ones (`a:ln` in `a:rPr`).
    pub pre: String,
    /// Children after them.
    pub post: String,
}

impl Extra {
    pub fn is_empty(&self) -> bool {
        self.attrs.is_empty() && self.pre.is_empty() && self.post.is_empty()
    }
}

/// Character formatting (`a:rPr`). `None` = inherit.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct RunProps {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    /// Hundredths of a point.
    pub size: Option<u32>,
    /// A plain `a:srgbClr` fill. Other fills stay in `extra`.
    pub color: Option<Rgb>,
    pub extra: Extra,
    /// Display only (never written): colour of a fill kept in `extra`
    /// (theme colours), and the `a:latin` typeface.
    pub shown_color: Option<Rgb>,
    pub shown_font: Option<String>,
    /// Display only: an underline style kept in `extra` (`dbl`, `wavy`...).
    pub shown_underline: bool,
}

/// Paragraph formatting (`a:pPr`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ParaProps {
    pub align: Option<Align>,
    /// Outline level, 0-based (`lvl`).
    pub level: u8,
    /// Other attributes and all children (spacing, bullets, tabs...).
    pub extra: Extra,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Run {
    /// Byte length in the paragraph text.
    pub len: usize,
    pub props: RunProps,
}

/// A paragraph: text plus runs covering it end to end. Line breaks
/// (`a:br`) are [`docs_engine::LINE_BREAK`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TextPara {
    pub text: String,
    pub runs: Vec<Run>,
    pub props: ParaProps,
    /// Raw `a:endParaRPr`, written back as is.
    pub end: Option<Arc<str>>,
}

impl Default for TextPara {
    fn default() -> Self {
        Self::new("")
    }
}

impl TextPara {
    pub fn new(text: impl Into<String>) -> Self {
        Self::styled(text, RunProps::default())
    }

    pub fn styled(text: impl Into<String>, props: RunProps) -> Self {
        let text = text.into();
        Self {
            runs: vec![Run {
                len: text.len(),
                props,
            }],
            text,
            props: ParaProps::default(),
            end: None,
        }
    }

    pub fn run_ranges(&self) -> impl Iterator<Item = (Range<usize>, &Run)> {
        let mut start = 0;
        self.runs.iter().map(move |r| {
            let range = start..start + r.len;
            start += r.len;
            (range, r)
        })
    }

    /// Formatting text typed at `offset` takes (the character before it).
    pub fn props_at(&self, offset: usize) -> RunProps {
        let mut first = None;
        for (range, run) in self.run_ranges() {
            if offset > range.start && offset <= range.end {
                return run.props.clone();
            }
            first.get_or_insert(run);
        }
        first.map(|r| r.props.clone()).unwrap_or_default()
    }

    fn split_run_at(&mut self, offset: usize) -> usize {
        let mut start = 0;
        for i in 0..self.runs.len() {
            let len = self.runs[i].len;
            if offset == start {
                return i;
            }
            if offset < start + len {
                let right = Run {
                    len: start + len - offset,
                    props: self.runs[i].props.clone(),
                };
                self.runs[i].len = offset - start;
                self.runs.insert(i + 1, right);
                return i + 1;
            }
            start += len;
        }
        self.runs.len()
    }

    pub fn insert(&mut self, offset: usize, text: &str, props: RunProps) {
        if text.is_empty() {
            return;
        }
        if self.text.is_empty() {
            self.text.push_str(text);
            self.runs = vec![Run {
                len: text.len(),
                props,
            }];
            return;
        }
        self.text.insert_str(offset, text);
        let i = self.split_run_at(offset);
        self.runs.insert(
            i,
            Run {
                len: text.len(),
                props,
            },
        );
        self.normalize();
    }

    pub fn delete(&mut self, range: Range<usize>) {
        if range.is_empty() {
            return;
        }
        let keep = self.props_at(range.start.max(1).min(self.text.len()));
        let a = self.split_run_at(range.start);
        let b = self.split_run_at(range.end);
        self.runs.drain(a..b);
        self.text.replace_range(range, "");
        if self.text.is_empty() {
            self.runs = vec![Run {
                len: 0,
                props: keep,
            }];
        }
        self.normalize();
    }

    /// Split at `offset`; the right part keeps the paragraph formatting.
    pub fn split_off(&mut self, offset: usize) -> TextPara {
        let carry = self.props_at(offset);
        let i = self.split_run_at(offset);
        let mut runs = self.runs.split_off(i);
        let text = self.text.split_off(offset);
        if text.is_empty() {
            runs = vec![Run {
                len: 0,
                props: carry.clone(),
            }];
        }
        if self.text.is_empty() {
            self.runs = vec![Run {
                len: 0,
                props: carry,
            }];
        }
        let mut right = TextPara {
            text,
            runs,
            props: self.props.clone(),
            end: self.end.clone(),
        };
        right.normalize();
        self.normalize();
        right
    }

    pub fn append(&mut self, other: TextPara) {
        if other.text.is_empty() {
            return;
        }
        if self.text.is_empty() {
            self.text = other.text;
            self.runs = other.runs;
            return;
        }
        self.text.push_str(&other.text);
        self.runs.extend(other.runs);
        self.normalize();
    }

    pub fn format(&mut self, range: Range<usize>, f: &impl Fn(&mut RunProps)) {
        if self.text.is_empty() {
            f(&mut self.runs[0].props);
            return;
        }
        let a = self.split_run_at(range.start);
        let b = self.split_run_at(range.end);
        for run in &mut self.runs[a..b] {
            f(&mut run.props);
        }
        self.normalize();
    }

    /// Drop empty runs and merge equal neighbours.
    pub fn normalize(&mut self) {
        if self.text.is_empty() {
            self.runs.truncate(1);
            if self.runs.is_empty() {
                self.runs.push(Run {
                    len: 0,
                    props: RunProps::default(),
                });
            }
            self.runs[0].len = 0;
            return;
        }
        self.runs.retain(|r| r.len > 0);
        let mut out: Vec<Run> = Vec::with_capacity(self.runs.len());
        for run in self.runs.drain(..) {
            match out.last_mut() {
                Some(last) if last.props == run.props => last.len += run.len,
                _ => out.push(run),
            }
        }
        self.runs = out;
    }
}

/// Vertical text anchoring (`bodyPr/@anchor`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Anchor {
    #[default]
    Top,
    Middle,
    Bottom,
}

/// Text-body layout settings (`a:bodyPr`), display only: the element
/// itself is written back raw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct BodyProps {
    /// Insets: left, top, right, bottom (EMU); `None` = inherit.
    pub insets: [Option<Emu>; 4],
    pub anchor: Option<Anchor>,
    /// `wrap="none"`.
    pub no_wrap: Option<bool>,
    /// `normAutofit/@fontScale` in 1/1000 %.
    pub font_scale: Option<u32>,
}

impl BodyProps {
    pub fn overlay(&mut self, o: &BodyProps) {
        for i in 0..4 {
            if o.insets[i].is_some() {
                self.insets[i] = o.insets[i];
            }
        }
        if o.anchor.is_some() {
            self.anchor = o.anchor;
        }
        if o.no_wrap.is_some() {
            self.no_wrap = o.no_wrap;
        }
        if o.font_scale.is_some() {
            self.font_scale = o.font_scale;
        }
    }
}

/// A text body (`p:txBody`).
#[derive(Clone, Debug, PartialEq)]
pub struct TextBody {
    pub paras: Vec<TextPara>,
    pub body: BodyProps,
    /// The body's own list style (`a:lstStyle`), for display.
    pub list: ListStyle,
    /// Content Kantor can't edit (fields, equations...): shown, kept, but
    /// text editing is refused.
    pub locked: Option<String>,
}

impl TextBody {
    pub fn new(paras: Vec<TextPara>) -> Self {
        Self {
            paras,
            body: BodyProps::default(),
            list: ListStyle::default(),
            locked: None,
        }
    }

    pub fn plain_text(&self) -> String {
        self.paras
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.paras.iter().all(|p| p.text.is_empty())
    }
}

/// Preset geometry, for display.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Geom {
    Rect,
    RoundRect,
    Ellipse,
    /// Lines and connectors.
    Line,
    /// Any other preset or a custom geometry: drawn as its bounding box.
    Other(String),
}

impl Geom {
    pub fn from_prst(p: &str) -> Self {
        match p {
            "rect" => Self::Rect,
            "roundRect" | "snipRoundRect" | "round2SameRect" => Self::RoundRect,
            "ellipse" => Self::Ellipse,
            "line" | "straightConnector1" => Self::Line,
            o => Self::Other(o.to_owned()),
        }
    }
}

/// `p:ph`: placeholder type (`title`, `body`, `ctrTitle`, ...) and index.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Placeholder {
    /// `None` = the default (`obj` / body).
    pub kind: Option<String>,
    pub idx: Option<u32>,
}

impl Placeholder {
    /// The effective type (`obj` when omitted).
    pub fn ty(&self) -> &str {
        self.kind.as_deref().unwrap_or("obj")
    }

    pub fn is_title(&self) -> bool {
        matches!(self.ty(), "title" | "ctrTitle")
    }

    /// Footer-type placeholders (not copied onto new slides).
    pub fn is_footer(&self) -> bool {
        matches!(self.ty(), "dt" | "ftr" | "sldNum" | "hdr")
    }
}

/// How a shape looks, resolved for display when read (fills from the
/// theme and `p:style`). Not written back: unmodelled formatting stays in
/// the source XML; new shapes are generated from it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Look {
    pub fill: Option<Rgb>,
    /// Outline colour and width (EMU).
    pub line: Option<(Rgb, Emu)>,
    /// Default text colour from the shape style (`fontRef`).
    pub text_color: Option<Rgb>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ShapeKind {
    /// `p:sp`: geometry with optional text.
    Shape { geom: Geom, text_box: bool },
    /// `p:pic`: `rel` in the slide's relationships, `part` the media part.
    Picture { rel: String, part: Option<String> },
    /// Groups, tables, charts, connectors, SmartArt...: kept verbatim and
    /// drawn as a labelled box.
    Other { what: String },
}

/// Where a shape's `a:xfrm` sits in its source element (byte offsets
/// into the raw XML).
#[derive(Debug, PartialEq)]
pub(crate) enum XfrmAt {
    /// An existing `a:xfrm`, with its raw attributes (rotation, flips).
    Replace(Range<usize>, String),
    /// No `a:xfrm`: insert one here (right after `<p:spPr>`).
    Insert(usize),
    /// `<p:spPr/>`: replace it, keeping its attributes.
    Expand(Range<usize>, String),
}

/// Where a shape came from in the file, for verbatim / spliced writing.
#[derive(Debug, PartialEq)]
pub(crate) struct ShapeSrc {
    /// The whole element as read.
    pub raw: String,
    /// The model as read; unchanged parts are written from `raw`.
    pub xfrm: Option<Xfrm>,
    pub text: Option<Vec<TextPara>>,
    /// Where the shape's `a:xfrm` is inside `raw`.
    pub xfrm_at: Option<XfrmAt>,
    /// Byte range of the paragraphs inside `p:txBody`.
    pub paras_at: Option<Range<usize>>,
    /// Highest `cNvPr/@id` inside the element (groups nest shapes).
    pub max_id: u32,
}

/// A shape on a slide (or a layout / master, read-only).
#[derive(Clone, Debug)]
pub struct Shape {
    pub id: u32,
    pub name: String,
    pub kind: ShapeKind,
    /// Own position; `None` = inherited from the layout placeholder.
    pub xfrm: Option<Xfrm>,
    pub ph: Option<Placeholder>,
    pub look: Look,
    pub text: Option<TextBody>,
    pub(crate) src: Option<Arc<ShapeSrc>>,
}

impl PartialEq for Shape {
    fn eq(&self, o: &Self) -> bool {
        self.id == o.id
            && self.name == o.name
            && self.kind == o.kind
            && self.xfrm == o.xfrm
            && self.ph == o.ph
            && self.look == o.look
            && self.text == o.text
            && match (&self.src, &o.src) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

impl Shape {
    /// Whether text can be typed into this shape.
    pub fn text_editable(&self) -> bool {
        matches!(self.kind, ShapeKind::Shape { .. })
            && self.text.as_ref().is_some_and(|t| t.locked.is_none())
    }

    /// Whether the shape can be moved and resized.
    pub fn movable(&self) -> bool {
        !matches!(self.kind, ShapeKind::Other { .. }) || self.src.is_none()
    }

    pub fn is_picture(&self) -> bool {
        matches!(self.kind, ShapeKind::Picture { .. })
    }

    pub(crate) fn max_id(&self) -> u32 {
        self.src.as_ref().map_or(self.id, |s| s.max_id.max(self.id))
    }

    /// Whether the shape is exactly as read (so its raw XML is written).
    pub(crate) fn unchanged(&self) -> bool {
        self.src.as_ref().is_some_and(|s| {
            s.xfrm == self.xfrm && s.text.as_ref() == self.text.as_ref().map(|t| &t.paras)
        })
    }
}

/// A short readable description of an unmodelled element.
pub(crate) fn describe_element(local: &str) -> String {
    match local {
        "grpSp" => "group".into(),
        "graphicFrame" => "table, chart or diagram".into(),
        "cxnSp" => "connector".into(),
        "contentPart" => "ink".into(),
        "AlternateContent" => "embedded object".into(),
        o => format!("<{o}>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bold() -> RunProps {
        RunProps {
            bold: Some(true),
            ..RunProps::default()
        }
    }

    #[test]
    fn paragraph_edits_keep_runs_consistent() {
        let mut p = TextPara::new("hello world");
        p.insert(5, " big", bold());
        assert_eq!(p.text, "hello big world");
        assert_eq!(p.runs.iter().map(|r| r.len).collect::<Vec<_>>(), [5, 4, 6]);
        let right = p.split_off(9);
        assert_eq!(right.text, " world");
        p.append(right);
        assert_eq!(p.runs.len(), 3);
        p.format(0..15, &|r| r.bold = Some(true));
        assert_eq!(p.runs.len(), 1);
        p.delete(0..15);
        assert_eq!(p.text, "");
        assert_eq!(p.runs[0].props, bold());
    }
}
