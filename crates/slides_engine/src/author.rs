//! Authoring slides from a description (`kantor slides build`, the MCP
//! slides tools): shapes, lines, pictures and tables written as DrawingML,
//! slide backgrounds, z-order, and new presentations with a chosen size,
//! theme colours and fonts.
//!
//! An [`Element`] is turned into the XML PowerPoint writes for it
//! ([`element_xml`]) and read back like any shape of a file, so what Kantor
//! shows, saves and converts is exactly that XML.

use std::fmt::Write as _;
use std::sync::Arc;

use anyhow::{Context as _, bail};
use docs_engine::Rgb;

use crate::frame::NS_C;
use crate::history::{Focus, Kind};
use crate::model::*;
use crate::presentation::Presentation;
use crate::xml::{Rel, esc, esc_attr, next_rel_id, rel_type, relative};

pub use crate::template::{DeckTheme, PackageOptions, THEME_COLOR_NAMES};

/// A colour: `#RRGGBB` or a theme colour (`accent1`, `tx1`, `bg1`, ...),
/// with an optional opacity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Color {
    pub base: ColorBase,
    /// Opacity in 1/1000 % (100 000 = opaque); `None` = opaque.
    pub alpha: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColorBase {
    Rgb(Rgb),
    /// `a:schemeClr/@val`.
    Scheme(String),
}

/// Theme colour names a [`ColorBase::Scheme`] may use.
pub const SCHEME_COLORS: [&str; 16] = [
    "bg1", "tx1", "bg2", "tx2", "accent1", "accent2", "accent3", "accent4", "accent5", "accent6",
    "hlink", "folHlink", "dk1", "lt1", "dk2", "lt2",
];

impl Color {
    pub fn rgb(c: Rgb) -> Self {
        Self {
            base: ColorBase::Rgb(c),
            alpha: None,
        }
    }

    pub fn scheme(name: &str) -> Self {
        Self {
            base: ColorBase::Scheme(name.to_owned()),
            alpha: None,
        }
    }

    fn write(&self, out: &mut String) {
        let (tag, val) = match &self.base {
            ColorBase::Rgb(c) => ("a:srgbClr", c.hex()),
            ColorBase::Scheme(s) => ("a:schemeClr", s.clone()),
        };
        match self.alpha.filter(|&a| a < 100_000) {
            Some(a) => {
                let _ = write!(out, "<{tag} val=\"{val}\"><a:alpha val=\"{a}\"/></{tag}>");
            }
            None => {
                let _ = write!(out, "<{tag} val=\"{val}\"/>");
            }
        }
    }
}

/// A shape or background fill.
#[derive(Clone, Debug, PartialEq)]
pub enum FillSpec {
    None,
    Solid(Color),
    /// A gradient through `stops` (position 0–100 000, colour).
    Gradient {
        stops: Vec<(u32, Color)>,
        kind: GradientKind,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GradientKind {
    /// Along a line at this angle (degrees clockwise from left → right).
    Linear(f64),
    /// From the centre outwards.
    Radial,
}

impl FillSpec {
    /// The fill's DrawingML (`a:solidFill`..., written with `a:`).
    pub fn xml(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Self::None => out.push_str("<a:noFill/>"),
            Self::Solid(c) => {
                out.push_str("<a:solidFill>");
                c.write(out);
                out.push_str("</a:solidFill>");
            }
            Self::Gradient { stops, kind } => {
                out.push_str("<a:gradFill rotWithShape=\"1\"><a:gsLst>");
                for (pos, c) in stops {
                    let _ = write!(out, "<a:gs pos=\"{}\">", pos.min(&100_000));
                    c.write(out);
                    out.push_str("</a:gs>");
                }
                out.push_str("</a:gsLst>");
                match kind {
                    GradientKind::Linear(deg) => {
                        let ang = ((deg * 60_000.0).round() as i64).rem_euclid(21_600_000);
                        let _ = write!(out, "<a:lin ang=\"{ang}\" scaled=\"0\"/>");
                    }
                    GradientKind::Radial => out.push_str(
                        "<a:path path=\"circle\"><a:fillToRect l=\"50000\" t=\"50000\" r=\"50000\" b=\"50000\"/></a:path>",
                    ),
                }
                out.push_str("</a:gradFill>");
            }
        }
    }
}

/// An arrowhead (`a:headEnd` / `a:tailEnd`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArrowSpec {
    /// `triangle`, `stealth`, `diamond`, `oval`, `arrow` (or `none`).
    pub kind: String,
    /// `sm`, `med`, `lg`.
    pub width: String,
    pub length: String,
}

/// Arrowhead types (`ST_LineEndType`).
pub const ARROW_KINDS: [&str; 6] = ["none", "triangle", "stealth", "diamond", "oval", "arrow"];
/// Preset dashes (`ST_PresetLineDashVal`).
pub const DASHES: [&str; 11] = [
    "solid",
    "dot",
    "dash",
    "lgDash",
    "dashDot",
    "lgDashDot",
    "lgDashDotDot",
    "sysDash",
    "sysDot",
    "sysDashDot",
    "sysDashDotDot",
];

/// An outline or line (`a:ln`); `None` fields are left to the shape style.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineSpec {
    /// [`FillSpec::None`] = no line.
    pub fill: Option<FillSpec>,
    pub width: Option<Emu>,
    /// A preset of [`DASHES`].
    pub dash: Option<String>,
    /// Arrowheads at the start (`head`) and the end (`tail`) of a line.
    pub head: Option<ArrowSpec>,
    pub tail: Option<ArrowSpec>,
    /// Round line ends.
    pub round_cap: bool,
}

impl LineSpec {
    pub fn none() -> Self {
        Self {
            fill: Some(FillSpec::None),
            ..Self::default()
        }
    }

    /// The line's DrawingML (`a:ln`).
    pub fn xml(&self) -> String {
        let mut out = String::new();
        self.write_as(&mut out, "a:ln");
        out
    }

    fn write_as(&self, out: &mut String, tag: &str) {
        let _ = write!(out, "<{tag}");
        if let Some(w) = self.width {
            let _ = write!(out, " w=\"{}\"", w.clamp(0, 20_116_800));
        }
        if self.round_cap {
            out.push_str(" cap=\"rnd\"");
        }
        out.push('>');
        if let Some(f) = &self.fill {
            // Lines take no gradient here: its first colour.
            match f {
                FillSpec::Gradient { stops, .. } => match stops.first() {
                    Some((_, c)) => FillSpec::Solid(c.clone()).write(out),
                    None => out.push_str("<a:noFill/>"),
                },
                f => f.write(out),
            }
        }
        if let Some(d) = &self.dash {
            let _ = write!(out, "<a:prstDash val=\"{}\"/>", esc_attr(d));
        }
        if self.round_cap {
            out.push_str("<a:round/>");
        }
        for (t, a) in [("a:headEnd", &self.head), ("a:tailEnd", &self.tail)] {
            if let Some(a) = a {
                let _ = write!(
                    out,
                    "<{t} type=\"{}\" w=\"{}\" len=\"{}\"/>",
                    esc_attr(&a.kind),
                    esc_attr(&a.width),
                    esc_attr(&a.length)
                );
            }
        }
        let _ = write!(out, "</{tag}>");
    }
}

/// Character formatting of a run; `None` = inherited (from the
/// paragraph, the element, then the theme).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunStyle {
    pub font: Option<String>,
    /// Hundredths of a point.
    pub size: Option<u32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    pub color: Option<Color>,
}

impl RunStyle {
    /// `o`'s settings over this one's.
    pub fn over(&self, o: &RunStyle) -> RunStyle {
        RunStyle {
            font: o.font.clone().or_else(|| self.font.clone()),
            size: o.size.or(self.size),
            bold: o.bold.or(self.bold),
            italic: o.italic.or(self.italic),
            underline: o.underline.or(self.underline),
            strike: o.strike.or(self.strike),
            color: o.color.clone().or_else(|| self.color.clone()),
        }
    }

    fn write(&self, out: &mut String, tag: &str) {
        let _ = write!(out, "<{tag} lang=\"en-US\"");
        if let Some(sz) = self.size {
            let _ = write!(out, " sz=\"{}\"", sz.clamp(100, 400_000));
        }
        if let Some(b) = self.bold {
            let _ = write!(out, " b=\"{}\"", u8::from(b));
        }
        if let Some(i) = self.italic {
            let _ = write!(out, " i=\"{}\"", u8::from(i));
        }
        if let Some(u) = self.underline {
            out.push_str(if u { " u=\"sng\"" } else { " u=\"none\"" });
        }
        if let Some(s) = self.strike {
            out.push_str(if s {
                " strike=\"sngStrike\""
            } else {
                " strike=\"noStrike\""
            });
        }
        out.push_str(" dirty=\"0\"");
        if self.color.is_none() && self.font.is_none() {
            out.push_str("/>");
            return;
        }
        out.push('>');
        if let Some(c) = &self.color {
            out.push_str("<a:solidFill>");
            c.write(out);
            out.push_str("</a:solidFill>");
        }
        if let Some(f) = &self.font {
            let f = esc_attr(f);
            let _ = write!(
                out,
                "<a:latin typeface=\"{f}\"/><a:ea typeface=\"{f}\"/><a:cs typeface=\"{f}\"/>"
            );
        }
        let _ = write!(out, "</{tag}>");
    }
}

/// A run of text with its formatting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunSpec {
    /// `\n` inside is a line break (`a:br`) within the paragraph.
    pub text: String,
    pub style: RunStyle,
}

/// A paragraph's bullet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BulletSpec {
    None,
    Char(String),
    /// `arabicPeriod`, `alphaLcParenR`, ... (`ST_TextAutonumberScheme`).
    Number(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParaSpec {
    pub runs: Vec<RunSpec>,
    pub align: Option<Align>,
    /// Outline level 0–8.
    pub level: u8,
    pub line_spacing: Option<Spacing>,
    pub space_before: Option<Spacing>,
    pub space_after: Option<Spacing>,
    pub bullet: Option<BulletSpec>,
    /// Left margin and first-line indent (EMU), for bullets.
    pub margin_left: Option<Emu>,
    pub indent: Option<Emu>,
    /// Formatting of the paragraph mark (an empty paragraph's height).
    pub end_style: RunStyle,
}

/// How text fits its shape (`a:bodyPr` autofit).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Autofit {
    /// Text may overflow (`a:noAutofit`).
    #[default]
    None,
    /// Shrink text on overflow (`a:normAutofit`).
    Shrink,
    /// The shape grows to fit its text (`a:spAutoFit`).
    Resize,
}

/// A text body: paragraphs and how they sit in the shape.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextSpec {
    pub paras: Vec<ParaSpec>,
    pub anchor: Option<Anchor>,
    /// Left, top, right, bottom (EMU); `None` = PowerPoint's default.
    pub insets: Option<[Emu; 4]>,
    pub autofit: Autofit,
    /// `false`: one line per paragraph (`wrap="none"`).
    pub wrap: bool,
    /// Text turned a quarter clockwise (`vert`), or `vert270`.
    pub vertical: Option<String>,
}

impl TextSpec {
    /// Plain text, one paragraph per line.
    pub fn plain(text: &str) -> Self {
        Self {
            paras: text
                .split('\n')
                .map(|l| ParaSpec {
                    runs: vec![RunSpec {
                        text: l.to_owned(),
                        style: RunStyle::default(),
                    }],
                    ..ParaSpec::default()
                })
                .collect(),
            wrap: true,
            ..Self::default()
        }
    }

    fn write_body_pr(&self, out: &mut String) {
        out.push_str("<a:bodyPr");
        if let Some(v) = &self.vertical {
            let _ = write!(out, " vert=\"{}\"", esc_attr(v));
        }
        out.push_str(if self.wrap {
            " wrap=\"square\""
        } else {
            " wrap=\"none\""
        });
        if let Some([l, t, r, b]) = self.insets {
            let _ = write!(
                out,
                " lIns=\"{}\" tIns=\"{}\" rIns=\"{}\" bIns=\"{}\"",
                l.max(0),
                t.max(0),
                r.max(0),
                b.max(0)
            );
        }
        out.push_str(" rtlCol=\"0\"");
        if let Some(a) = self.anchor {
            out.push_str(match a {
                Anchor::Top => " anchor=\"t\"",
                Anchor::Middle => " anchor=\"ctr\"",
                Anchor::Bottom => " anchor=\"b\"",
            });
        }
        out.push('>');
        out.push_str(match self.autofit {
            Autofit::None => "<a:noAutofit/>",
            Autofit::Shrink => "<a:normAutofit/>",
            Autofit::Resize => "<a:spAutoFit/>",
        });
        out.push_str("</a:bodyPr>");
    }

    /// `a:bodyPr`, `a:lstStyle` and the paragraphs (the inside of
    /// `p:txBody` / `a:txBody`).
    fn write_inner(&self, out: &mut String, base: &RunStyle) {
        self.write_body_pr(out);
        out.push_str("<a:lstStyle/>");
        if self.paras.is_empty() {
            out.push_str("<a:p>");
            base.write(out, "a:endParaRPr");
            out.push_str("</a:p>");
        }
        for p in &self.paras {
            write_para(out, p, base);
        }
    }
}

fn write_spacing(out: &mut String, tag: &str, s: Spacing) {
    match s {
        Spacing::Pct(v) => {
            let _ = write!(out, "<{tag}><a:spcPct val=\"{}\"/></{tag}>", v.max(0));
        }
        Spacing::Pts(v) => {
            let _ = write!(
                out,
                "<{tag}><a:spcPts val=\"{}\"/></{tag}>",
                v.clamp(0, 158_400)
            );
        }
    }
}

fn write_para(out: &mut String, p: &ParaSpec, base: &RunStyle) {
    out.push_str("<a:p><a:pPr");
    if let Some(m) = p.margin_left {
        let _ = write!(out, " marL=\"{}\"", m.clamp(0, 51_206_400));
    }
    if p.level > 0 {
        let _ = write!(out, " lvl=\"{}\"", p.level.min(8));
    }
    if let Some(i) = p.indent {
        let _ = write!(out, " indent=\"{}\"", i.clamp(-51_206_400, 51_206_400));
    }
    if let Some(a) = p.align {
        let _ = write!(out, " algn=\"{}\"", a.attr());
    }
    let mut inner = String::new();
    if let Some(s) = p.line_spacing {
        write_spacing(&mut inner, "a:lnSpc", s);
    }
    if let Some(s) = p.space_before {
        write_spacing(&mut inner, "a:spcBef", s);
    }
    if let Some(s) = p.space_after {
        write_spacing(&mut inner, "a:spcAft", s);
    }
    match &p.bullet {
        None => {}
        Some(BulletSpec::None) => inner.push_str("<a:buNone/>"),
        Some(BulletSpec::Char(c)) => {
            let _ = write!(
                inner,
                "<a:buFont typeface=\"Arial\"/><a:buChar char=\"{}\"/>",
                esc_attr(c)
            );
        }
        Some(BulletSpec::Number(scheme)) => {
            let _ = write!(inner, "<a:buAutoNum type=\"{}\"/>", esc_attr(scheme));
        }
    }
    if inner.is_empty() {
        out.push_str("/>");
    } else {
        out.push('>');
        out.push_str(&inner);
        out.push_str("</a:pPr>");
    }
    for r in &p.runs {
        let style = base.over(&r.style);
        for (i, piece) in r.text.split('\n').enumerate() {
            if i > 0 {
                out.push_str("<a:br>");
                style.write(out, "a:rPr");
                out.push_str("</a:br>");
            }
            if piece.is_empty() {
                continue;
            }
            out.push_str("<a:r>");
            style.write(out, "a:rPr");
            let _ = write!(out, "<a:t>{}</a:t></a:r>", esc(piece));
        }
    }
    base.over(&p.end_style).write(out, "a:endParaRPr");
    out.push_str("</a:p>");
}

/// A preset geometry with its adjust values (`a:prstGeom`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeomSpec {
    pub preset: String,
    /// `(name, value)` of `a:avLst/a:gd` (`fmla="val {value}"`).
    pub adjust: Vec<(String, i64)>,
}

impl GeomSpec {
    pub fn preset(name: &str) -> Self {
        Self {
            preset: name.to_owned(),
            adjust: Vec::new(),
        }
    }

    /// Check the preset name and adjust names against the ECMA presets.
    pub fn check(&self) -> anyhow::Result<()> {
        let Some(def) = crate::geom::preset(&self.preset) else {
            bail!(
                "\"{}\" is not a preset shape (see the DrawingML preset names, e.g. rect, roundRect, ellipse, chevron, homePlate, rightArrow)",
                self.preset
            );
        };
        for (name, _) in &self.adjust {
            if !def.av.iter().any(|g| g.name == *name) {
                let known: Vec<&str> = def.av.iter().map(|g| g.name.as_str()).collect();
                if known.is_empty() {
                    bail!("{} takes no adjust values", self.preset);
                }
                bail!(
                    "{} has no adjust value \"{name}\" (it has {})",
                    self.preset,
                    known.join(", ")
                );
            }
        }
        Ok(())
    }

    fn write(&self, out: &mut String) {
        let _ = write!(out, "<a:prstGeom prst=\"{}\">", esc_attr(&self.preset));
        if self.adjust.is_empty() {
            out.push_str("<a:avLst/>");
        } else {
            out.push_str("<a:avLst>");
            for (n, v) in &self.adjust {
                let _ = write!(out, "<a:gd name=\"{}\" fmla=\"val {v}\"/>", esc_attr(n));
            }
            out.push_str("</a:avLst>");
        }
        out.push_str("</a:prstGeom>");
    }
}

/// The `adj` value that rounds the corners of a `w` × `h` rounded
/// rectangle by `radius` (PowerPoint: a fraction of the shorter side, at
/// most half).
pub fn round_rect_adj(radius: Emu, w: Emu, h: Emu) -> i64 {
    let short = w.min(h).max(1);
    ((radius.max(0) as f64 / short as f64) * 100_000.0)
        .round()
        .clamp(0.0, 50_000.0) as i64
}

/// A shape: geometry, fill, outline, text (`p:sp`).
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeSpec {
    pub geom: GeomSpec,
    /// A text box (`txBox="1"`, no shape style).
    pub text_box: bool,
    pub fill: Option<FillSpec>,
    pub line: Option<LineSpec>,
    pub text: Option<TextSpec>,
    /// Default formatting of the text's runs.
    pub style: RunStyle,
}

/// A straight line or connector between two points (`p:cxnSp`).
#[derive(Clone, Debug, PartialEq)]
pub struct LineShapeSpec {
    pub line: LineSpec,
}

/// Cropping as fractions of the picture removed from each side, in
/// 1/1000 % (`a:srcRect`): left, top, right, bottom.
pub type Crop = [i64; 4];

/// A picture (`p:pic`).
#[derive(Clone, Debug, PartialEq)]
pub struct PictureSpec {
    /// PNG or JPEG bytes.
    pub bytes: Arc<Vec<u8>>,
    pub crop: Crop,
    /// The shape it's cut to (`None`: its box).
    pub geom: Option<GeomSpec>,
    pub line: Option<LineSpec>,
    /// Alternative text (`descr`).
    pub alt: Option<String>,
}

/// One table cell.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CellSpec {
    pub text: TextSpec,
    pub style: RunStyle,
    pub fill: Option<FillSpec>,
    /// Left, right, top, bottom borders.
    pub borders: [Option<LineSpec>; 4],
    /// Left, right, top, bottom margins (EMU).
    pub margins: Option<[Emu; 4]>,
    /// Columns and rows this cell spans (1 = itself).
    pub col_span: u32,
    pub row_span: u32,
    /// Covered by a cell spanning from the left / from above.
    pub h_merge: bool,
    pub v_merge: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowSpec {
    pub height: Emu,
    pub cells: Vec<CellSpec>,
}

/// A table (`p:graphicFrame` holding `a:tbl`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableSpec {
    /// Column widths (EMU).
    pub columns: Vec<Emu>,
    pub rows: Vec<RowSpec>,
    /// `a:tblPr` flags (what a table style would emphasise).
    pub first_row: bool,
    pub band_rows: bool,
    /// A table style id (`{GUID}`) to apply, if any.
    pub style_id: Option<String>,
}

/// A chart (`p:graphicFrame` holding `c:chart`): its part and the
/// workbook embedded with it (made by `sheets_engine::data_chart_part` and
/// [`crate::chart_edit::chart_workbook`]). The part names the workbook
/// as relationship `rId1` (`c:externalData`).
#[derive(Clone, Debug, PartialEq)]
pub struct ChartPartSpec {
    pub xml: Arc<Vec<u8>>,
    pub workbook: Arc<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ElementKind {
    Shape(ShapeSpec),
    Line(LineShapeSpec),
    Picture(PictureSpec),
    Table(TableSpec),
    Chart(ChartPartSpec),
}

/// Something to put on a slide.
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub kind: ElementKind,
    /// Name (`cNvPr/@name`); default from the kind and id.
    pub name: Option<String>,
    /// Position, size, rotation and flips.
    pub xfrm: Xfrm,
}

impl Element {
    fn default_name(&self) -> &'static str {
        match &self.kind {
            ElementKind::Shape(s) if s.text_box => "TextBox",
            ElementKind::Shape(s) => match s.geom.preset.as_str() {
                "rect" => "Rectangle",
                "roundRect" => "Rounded Rectangle",
                "ellipse" => "Oval",
                _ => "Shape",
            },
            ElementKind::Line(_) => "Straight Connector",
            ElementKind::Picture(_) => "Picture",
            ElementKind::Table(_) => "Table",
            ElementKind::Chart(_) => "Chart",
        }
    }

    /// Check what can't be written as is.
    pub fn check(&self) -> anyhow::Result<()> {
        match &self.kind {
            ElementKind::Shape(s) => s.geom.check(),
            ElementKind::Line(_) => Ok(()),
            ElementKind::Picture(p) => {
                if crate::edit::image_ext(&p.bytes).is_none() {
                    bail!("only PNG and JPEG pictures can be inserted");
                }
                if p.crop.iter().any(|c| !(0..100_000).contains(c))
                    || p.crop[0] + p.crop[2] >= 100_000
                    || p.crop[1] + p.crop[3] >= 100_000
                {
                    bail!("cropping must leave part of the picture");
                }
                p.geom.as_ref().map_or(Ok(()), GeomSpec::check)
            }
            ElementKind::Table(t) => {
                if t.columns.is_empty() || t.rows.is_empty() {
                    bail!("a table needs at least one row and one column");
                }
                for (i, r) in t.rows.iter().enumerate() {
                    if r.cells.len() != t.columns.len() {
                        bail!(
                            "table row {} has {} cells; the table has {} columns",
                            i + 1,
                            r.cells.len(),
                            t.columns.len()
                        );
                    }
                }
                Ok(())
            }
            ElementKind::Chart(c) => {
                let xml = std::str::from_utf8(&c.xml).context("chart part")?;
                let doc = roxmltree::Document::parse(xml).context("chart part")?;
                if doc.root_element().tag_name().name() != "chartSpace" {
                    bail!("not a chart part");
                }
                Ok(())
            }
        }
    }
}

fn write_xfrm(out: &mut String, tag: &str, x: &Xfrm) {
    let _ = write!(out, "<{tag}");
    if x.rot != 0 {
        let _ = write!(out, " rot=\"{}\"", x.rot);
    }
    if x.flip_h {
        out.push_str(" flipH=\"1\"");
    }
    if x.flip_v {
        out.push_str(" flipV=\"1\"");
    }
    let _ = write!(
        out,
        "><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></{tag}>",
        x.x,
        x.y,
        x.cx.max(0),
        x.cy.max(0)
    );
}

/// PowerPoint's style for a new shape: accent 1 fill, a darker outline,
/// light text (each overridden by the shape's own settings).
const SHAPE_STYLE: &str = "<p:style><a:lnRef idx=\"2\"><a:schemeClr val=\"accent1\"><a:shade val=\"50000\"/></a:schemeClr></a:lnRef><a:fillRef idx=\"1\"><a:schemeClr val=\"accent1\"/></a:fillRef><a:effectRef idx=\"0\"><a:schemeClr val=\"accent1\"/></a:effectRef><a:fontRef idx=\"minor\"><a:schemeClr val=\"lt1\"/></a:fontRef></p:style>";
/// PowerPoint's style for a new line: accent 1.
const LINE_STYLE: &str = "<p:style><a:lnRef idx=\"1\"><a:schemeClr val=\"accent1\"/></a:lnRef><a:fillRef idx=\"0\"><a:schemeClr val=\"accent1\"/></a:fillRef><a:effectRef idx=\"0\"><a:schemeClr val=\"accent1\"/></a:effectRef><a:fontRef idx=\"minor\"><a:schemeClr val=\"tx1\"/></a:fontRef></p:style>";

fn cnv_pr(out: &mut String, id: u32, name: &str, alt: Option<&str>) {
    let _ = write!(out, "<p:cNvPr id=\"{id}\" name=\"{}\"", esc_attr(name));
    if let Some(a) = alt.filter(|a| !a.is_empty()) {
        let _ = write!(out, " descr=\"{}\"", esc_attr(a));
    }
    out.push_str("/>");
}

/// The XML of `el` as shape `id` on a slide (`p:` / `a:` / `r:`
/// prefixes); `rel` is the relationship of a picture's media.
pub fn element_xml(el: &Element, id: u32, rel: Option<&str>) -> String {
    let name = el
        .name
        .clone()
        .unwrap_or_else(|| format!("{} {}", el.default_name(), id.saturating_sub(1)));
    let mut out = String::new();
    match &el.kind {
        ElementKind::Shape(s) => {
            out.push_str("<p:sp><p:nvSpPr>");
            cnv_pr(&mut out, id, &name, None);
            out.push_str(if s.text_box {
                "<p:cNvSpPr txBox=\"1\"/>"
            } else {
                "<p:cNvSpPr/>"
            });
            out.push_str("<p:nvPr/></p:nvSpPr><p:spPr>");
            write_xfrm(&mut out, "a:xfrm", &el.xfrm);
            s.geom.write(&mut out);
            match (&s.fill, s.text_box) {
                (Some(f), _) => f.write(&mut out),
                (None, true) => out.push_str("<a:noFill/>"),
                (None, false) => {}
            }
            match (&s.line, s.text_box) {
                (Some(l), _) => l.write_as(&mut out, "a:ln"),
                (None, true) => {}
                (None, false) => {}
            }
            out.push_str("</p:spPr>");
            if !s.text_box {
                out.push_str(SHAPE_STYLE);
            }
            out.push_str("<p:txBody>");
            let text = s.text.clone().unwrap_or_else(|| TextSpec {
                wrap: true,
                anchor: (!s.text_box).then_some(Anchor::Middle),
                paras: vec![ParaSpec {
                    align: (!s.text_box).then_some(Align::Center),
                    ..ParaSpec::default()
                }],
                ..TextSpec::default()
            });
            text.write_inner(&mut out, &s.style);
            out.push_str("</p:txBody></p:sp>");
        }
        ElementKind::Line(l) => {
            out.push_str("<p:cxnSp><p:nvCxnSpPr>");
            cnv_pr(&mut out, id, &name, None);
            out.push_str("<p:cNvCxnSpPr/><p:nvPr/></p:nvCxnSpPr><p:spPr>");
            write_xfrm(&mut out, "a:xfrm", &el.xfrm);
            GeomSpec::preset("line").write(&mut out);
            l.line.write_as(&mut out, "a:ln");
            out.push_str("</p:spPr>");
            out.push_str(LINE_STYLE);
            out.push_str("</p:cxnSp>");
        }
        ElementKind::Picture(p) => {
            out.push_str("<p:pic><p:nvPicPr>");
            cnv_pr(&mut out, id, &name, p.alt.as_deref());
            out.push_str(
                "<p:cNvPicPr><a:picLocks noChangeAspect=\"1\"/></p:cNvPicPr><p:nvPr/></p:nvPicPr><p:blipFill>",
            );
            let _ = write!(
                out,
                "<a:blip r:embed=\"{}\"/>",
                esc_attr(rel.unwrap_or("rId1"))
            );
            let [l, t, r, b] = p.crop;
            if p.crop == [0; 4] {
                out.push_str("<a:srcRect/>");
            } else {
                out.push_str("<a:srcRect");
                for (k, v) in [("l", l), ("t", t), ("r", r), ("b", b)] {
                    if v != 0 {
                        let _ = write!(out, " {k}=\"{v}\"");
                    }
                }
                out.push_str("/>");
            }
            out.push_str("<a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr>");
            write_xfrm(&mut out, "a:xfrm", &el.xfrm);
            p.geom
                .clone()
                .unwrap_or_else(|| GeomSpec::preset("rect"))
                .write(&mut out);
            if let Some(l) = &p.line {
                l.write_as(&mut out, "a:ln");
            }
            out.push_str("</p:spPr></p:pic>");
        }
        ElementKind::Table(t) => write_table(&mut out, t, id, &name, &el.xfrm),
        ElementKind::Chart(_) => {
            out.push_str("<p:graphicFrame><p:nvGraphicFramePr>");
            cnv_pr(&mut out, id, &name, None);
            out.push_str(
                "<p:cNvGraphicFramePr><a:graphicFrameLocks noGrp=\"1\"/></p:cNvGraphicFramePr><p:nvPr/></p:nvGraphicFramePr>",
            );
            let frame = Xfrm {
                rot: 0,
                flip_h: false,
                flip_v: false,
                ..el.xfrm
            };
            write_xfrm(&mut out, "p:xfrm", &frame);
            let _ = write!(
                out,
                "<a:graphic><a:graphicData uri=\"{NS_C}\"><c:chart xmlns:c=\"{NS_C}\" r:id=\"{}\"/></a:graphicData></a:graphic></p:graphicFrame>",
                esc_attr(rel.unwrap_or("rId1"))
            );
        }
    }
    out
}

fn write_table(out: &mut String, t: &TableSpec, id: u32, name: &str, x: &Xfrm) {
    out.push_str("<p:graphicFrame><p:nvGraphicFramePr>");
    cnv_pr(out, id, name, None);
    out.push_str(
        "<p:cNvGraphicFramePr><a:graphicFrameLocks noGrp=\"1\"/></p:cNvGraphicFramePr><p:nvPr/></p:nvGraphicFramePr>",
    );
    let w: Emu = t.columns.iter().sum();
    let h: Emu = t.rows.iter().map(|r| r.height).sum();
    let frame = Xfrm {
        cx: w,
        cy: h,
        rot: 0,
        flip_h: false,
        flip_v: false,
        ..*x
    };
    write_xfrm(out, "p:xfrm", &frame);
    out.push_str(
        "<a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/table\"><a:tbl><a:tblPr",
    );
    if t.first_row {
        out.push_str(" firstRow=\"1\"");
    }
    if t.band_rows {
        out.push_str(" bandRow=\"1\"");
    }
    match &t.style_id {
        Some(s) => {
            let _ = write!(
                out,
                "><a:tableStyleId>{}</a:tableStyleId></a:tblPr>",
                esc(s)
            );
        }
        None => out.push_str("/>"),
    }
    out.push_str("<a:tblGrid>");
    for c in &t.columns {
        let _ = write!(out, "<a:gridCol w=\"{}\"/>", c.max(&0));
    }
    out.push_str("</a:tblGrid>");
    for r in &t.rows {
        let _ = write!(out, "<a:tr h=\"{}\">", r.height.max(0));
        for c in &r.cells {
            out.push_str("<a:tc");
            if c.col_span > 1 {
                let _ = write!(out, " gridSpan=\"{}\"", c.col_span);
            }
            if c.row_span > 1 {
                let _ = write!(out, " rowSpan=\"{}\"", c.row_span);
            }
            if c.h_merge {
                out.push_str(" hMerge=\"1\"");
            }
            if c.v_merge {
                out.push_str(" vMerge=\"1\"");
            }
            out.push_str("><a:txBody>");
            // Cells: `a:bodyPr` stays empty (anchor and margins are on `a:tcPr`).
            out.push_str("<a:bodyPr/><a:lstStyle/>");
            if c.text.paras.is_empty() {
                out.push_str("<a:p>");
                c.style.write(out, "a:endParaRPr");
                out.push_str("</a:p>");
            }
            for p in &c.text.paras {
                write_para(out, p, &c.style);
            }
            out.push_str("</a:txBody><a:tcPr");
            if let Some([l, r, t, b]) = c.margins {
                let _ = write!(
                    out,
                    " marL=\"{}\" marR=\"{}\" marT=\"{}\" marB=\"{}\"",
                    l.max(0),
                    r.max(0),
                    t.max(0),
                    b.max(0)
                );
            }
            if let Some(a) = c.text.anchor {
                out.push_str(match a {
                    Anchor::Top => " anchor=\"t\"",
                    Anchor::Middle => " anchor=\"ctr\"",
                    Anchor::Bottom => " anchor=\"b\"",
                });
            }
            out.push('>');
            for (tag, b) in ["a:lnL", "a:lnR", "a:lnT", "a:lnB"].iter().zip(&c.borders) {
                if let Some(b) = b {
                    b.write_as(out, tag);
                }
            }
            if let Some(f) = &c.fill {
                f.write(out);
            }
            out.push_str("</a:tcPr></a:tc>");
        }
        out.push_str("</a:tr>");
    }
    out.push_str("</a:tbl></a:graphicData></a:graphic></p:graphicFrame>");
}

/// A slide background (`p:bg`).
#[derive(Clone, Debug, PartialEq)]
pub enum BackgroundSpec {
    /// Remove the slide's own background (follow the layout and master).
    Inherit,
    Fill(FillSpec),
    /// A picture stretched over the slide (PNG or JPEG bytes).
    Picture(Arc<Vec<u8>>),
}

/// Where the next new media part goes.
fn free_media_part(p: &Presentation, ext: &str) -> String {
    (1..)
        .map(|n| format!("ppt/media/image{n}.{ext}"))
        .find(|part| !p.pkg.names.contains(part) && !p.media.contains_key(part))
        .expect("a free part name")
}

impl Presentation {
    /// A new presentation as `o` says (size, theme, layouts, with or
    /// without its title slide).
    pub fn new_with(o: &PackageOptions) -> anyhow::Result<Self> {
        Self::from_bytes(crate::template::package(o))
    }

    /// The media part holding `bytes`: one already added with the same
    /// bytes, else a new one.
    fn media_part(&mut self, bytes: &Arc<Vec<u8>>) -> anyhow::Result<String> {
        let ext =
            crate::edit::image_ext(bytes).context("only PNG and JPEG pictures can be used")?;
        let mut same: Vec<&String> = self
            .media
            .iter()
            .filter(|(_, b)| b.as_slice() == bytes.as_slice())
            .map(|(p, _)| p)
            .collect();
        same.sort();
        if let Some(p) = same.first() {
            return Ok((*p).clone());
        }
        let part = free_media_part(self, ext);
        self.media.insert(part.clone(), bytes.clone());
        Ok(part)
    }

    /// Add `el` on top of slide `slide`; returns its index there. One undo
    /// step.
    pub fn add_element(&mut self, slide: usize, el: &Element) -> anyhow::Result<usize> {
        el.check()?;
        self.slides.get(slide).context("no such slide")?;
        let media = match &el.kind {
            ElementKind::Picture(p) => Some(self.media_part(&p.bytes)?),
            _ => None,
        };
        let chart = match &el.kind {
            ElementKind::Chart(c) => Some(self.add_chart_parts(c)),
            _ => None,
        };
        self.record(Kind::Other, None, Focus::slide(slide), false, |p| {
            let s = p.slide_mut(slide)?;
            let id = s.next_shape_id();
            let mut rels = (*s.rels).clone();
            let chart_rel = chart.as_ref().map(|part| {
                let id = next_rel_id(&rels);
                rels.push(Rel {
                    id: id.clone(),
                    ty: rel_type("chart"),
                    target: relative(&s.part, part),
                    external: false,
                });
                id
            });
            let rel = chart_rel.or_else(|| {
                media.as_ref().map(|part| {
                    let rel = rels
                        .iter()
                        .find(|r| r.kind() == "image" && r.part(&s.part).as_deref() == Some(part))
                        .map(|r| r.id.clone());
                    rel.unwrap_or_else(|| {
                        let id = next_rel_id(&rels);
                        rels.push(Rel {
                            id: id.clone(),
                            ty: rel_type("image"),
                            target: relative(&s.part, part),
                            external: false,
                        });
                        id
                    })
                })
            });
            let xml = element_xml(el, id, rel.as_deref());
            let shape = p.parse_new_with(slide, &xml, &rels)?;
            let s = p.slide_mut(slide)?;
            s.rels = Arc::new(rels);
            s.shapes.push(shape);
            let i = s.shapes.len() - 1;
            Ok((i, Focus::shape(slide, i)))
        })
    }

    /// Move shape `from` of slide `slide` to position `to` of its shape
    /// list (0 = at the back); returns where it went. One undo step.
    pub fn reorder_shape(&mut self, slide: usize, from: usize, to: usize) -> anyhow::Result<usize> {
        self.record(Kind::Other, None, Focus::shape(slide, from), false, |p| {
            let s = p.slide_mut(slide)?;
            if from >= s.shapes.len() {
                bail!("no such shape");
            }
            let to = to.min(s.shapes.len() - 1);
            let shape = s.shapes.remove(from);
            s.shapes.insert(to, shape);
            Ok((to, Focus::shape(slide, to)))
        })
    }

    /// Set the background of slide `slide`. One undo step.
    pub fn set_background(&mut self, slide: usize, bg: &BackgroundSpec) -> anyhow::Result<()> {
        self.slides.get(slide).context("no such slide")?;
        let media = match bg {
            BackgroundSpec::Picture(bytes) => Some(self.media_part(bytes)?),
            _ => None,
        };
        let master = self.master_of(slide);
        let (theme, map) = (master.theme.clone(), master.clr_map.clone());
        self.record(Kind::Other, None, Focus::slide(slide), false, |p| {
            let s = p.slide_mut(slide)?;
            let mut rels = (*s.rels).clone();
            let mut xml = String::new();
            match bg {
                BackgroundSpec::Inherit => {}
                BackgroundSpec::Fill(f) => {
                    xml.push_str("<p:bg><p:bgPr>");
                    f.write(&mut xml);
                    xml.push_str("<a:effectLst/></p:bgPr></p:bg>");
                }
                BackgroundSpec::Picture(_) => {
                    let part = media.as_deref().expect("media part");
                    let id = next_rel_id(&rels);
                    rels.push(Rel {
                        id: id.clone(),
                        ty: rel_type("image"),
                        target: relative(&s.part, part),
                        external: false,
                    });
                    let _ = write!(
                        xml,
                        "<p:bg><p:bgPr><a:blipFill dpi=\"0\" rotWithShape=\"1\"><a:blip r:embed=\"{id}\"/><a:srcRect/><a:stretch><a:fillRect/></a:stretch></a:blipFill><a:effectLst/></p:bgPr></p:bg>"
                    );
                }
            }
            let mut prefix = s.src.prefix.clone();
            let start = prefix.find("<p:cSld").context("this slide's XML can't take a background")?;
            let open = crate::xml::tag_end(&prefix[start..]).context("slide XML")? + start;
            if prefix[..open].ends_with("/>") {
                bail!("this slide's XML can't take a background");
            }
            // Drop the old background (`p:bg` comes first in `p:cSld`).
            if prefix[open..].starts_with("<p:bg>") || prefix[open..].starts_with("<p:bg ") {
                let end = prefix[open..]
                    .find("</p:bg>")
                    .context("slide background XML")?
                    + open
                    + "</p:bg>".len();
                prefix.replace_range(open..end, "");
            }
            prefix.insert_str(open, &xml);
            // What's shown: read the new background back.
            let (bg_rgb, fill) = if xml.is_empty() {
                (None, None)
            } else {
                let doc_xml = format!(
                    "<p:cSld xmlns:p=\"{}\" xmlns:a=\"{}\" xmlns:r=\"{}\">{xml}</p:cSld>",
                    crate::xml::NS_P,
                    crate::xml::NS_A,
                    crate::xml::NS_R
                );
                let doc = roxmltree::Document::parse(&doc_xml).context("background XML")?;
                let colors = crate::theme::ColorCtx {
                    theme: &theme,
                    map: &map,
                    ph: None,
                };
                let csld = doc.root_element();
                (
                    crate::presentation::background(csld, &colors),
                    crate::presentation::background_fill(csld, &colors, &s.part, &rels),
                )
            };
            s.bg = bg_rgb;
            s.background = fill;
            s.rels = Arc::new(rels);
            s.src = Arc::new(crate::presentation::SlideSrc {
                prefix,
                suffix: s.src.suffix.clone(),
                shapes: s.src.shapes.clone(),
                rels: s.src.rels.clone(),
                edited: true,
            });
            Ok(((), Focus::slide(slide)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(h: &str) -> Color {
        Color::rgb(Rgb::parse(h).unwrap())
    }

    fn shape(geom: GeomSpec) -> Element {
        Element {
            kind: ElementKind::Shape(ShapeSpec {
                geom,
                text_box: false,
                fill: Some(FillSpec::Solid(rgb("1F3864"))),
                line: Some(LineSpec::none()),
                text: Some(TextSpec {
                    paras: vec![ParaSpec {
                        runs: vec![
                            RunSpec {
                                text: "Fast & ".into(),
                                style: RunStyle {
                                    bold: Some(true),
                                    ..RunStyle::default()
                                },
                            },
                            RunSpec {
                                text: "simple".into(),
                                style: RunStyle {
                                    italic: Some(true),
                                    color: Some(Color::scheme("accent2")),
                                    ..RunStyle::default()
                                },
                            },
                        ],
                        align: Some(Align::Center),
                        line_spacing: Some(Spacing::Pct(120_000)),
                        ..ParaSpec::default()
                    }],
                    anchor: Some(Anchor::Middle),
                    insets: Some([91_440, 45_720, 91_440, 45_720]),
                    autofit: Autofit::Shrink,
                    wrap: true,
                    vertical: None,
                }),
                style: RunStyle {
                    font: Some("Inter".into()),
                    size: Some(1800),
                    color: Some(rgb("FFFFFF")),
                    ..RunStyle::default()
                },
            }),
            name: None,
            xfrm: Xfrm::new(914_400, 914_400, 2_743_200, 914_400),
        }
    }

    #[test]
    fn writes_a_shape_with_runs() {
        let mut g = GeomSpec::preset("roundRect");
        g.adjust
            .push(("adj".into(), round_rect_adj(182_880, 2_743_200, 914_400)));
        let xml = element_xml(&shape(g), 4, None);
        assert!(
            xml.starts_with("<p:sp><p:nvSpPr><p:cNvPr id=\"4\" name=\"Rounded Rectangle 3\"/>")
        );
        assert!(xml.contains("<a:prstGeom prst=\"roundRect\"><a:avLst><a:gd name=\"adj\" fmla=\"val 20000\"/></a:avLst></a:prstGeom>"), "{xml}");
        assert!(xml.contains("<a:solidFill><a:srgbClr val=\"1F3864\"/></a:solidFill><a:ln><a:noFill/></a:ln></p:spPr>"));
        assert!(xml.contains("anchor=\"ctr\"><a:normAutofit/></a:bodyPr>"));
        assert!(
            xml.contains(
                "<a:pPr algn=\"ctr\"><a:lnSpc><a:spcPct val=\"120000\"/></a:lnSpc></a:pPr>"
            )
        );
        assert!(xml.contains("<a:t>Fast &amp; </a:t>"));
        assert!(xml.contains("b=\"1\""));
        assert!(xml.contains("<a:schemeClr val=\"accent2\"/>"));
        assert!(xml.contains("<a:latin typeface=\"Inter\"/>"));
        // Well-formed once the prefixes are declared.
        let doc = format!(
            "<r xmlns:p=\"{}\" xmlns:a=\"{}\" xmlns:r=\"{}\">{xml}</r>",
            crate::xml::NS_P,
            crate::xml::NS_A,
            crate::xml::NS_R
        );
        roxmltree::Document::parse(&doc).unwrap();
    }

    #[test]
    fn checks_presets_and_adjust_names() {
        assert!(GeomSpec::preset("chevron").check().is_ok());
        assert!(GeomSpec::preset("homePlate").check().is_ok());
        assert!(GeomSpec::preset("pentagonish").check().is_err());
        let mut g = GeomSpec::preset("rect");
        g.adjust.push(("adj".into(), 1));
        assert!(g.check().is_err());
        let mut g = GeomSpec::preset("roundRect");
        g.adjust.push(("adj".into(), 1));
        assert!(g.check().is_ok());
        assert_eq!(round_rect_adj(10_000_000, 100, 50), 50_000);
    }

    #[test]
    fn writes_lines_pictures_and_tables() {
        let line = Element {
            kind: ElementKind::Line(LineShapeSpec {
                line: LineSpec {
                    fill: Some(FillSpec::Solid(rgb("FF0000"))),
                    width: Some(25_400),
                    dash: Some("dash".into()),
                    tail: Some(ArrowSpec {
                        kind: "triangle".into(),
                        width: "med".into(),
                        length: "lg".into(),
                    }),
                    ..LineSpec::default()
                },
            }),
            name: Some("Arrow".into()),
            xfrm: Xfrm {
                flip_v: true,
                ..Xfrm::new(0, 0, 100, 100)
            },
        };
        let xml = element_xml(&line, 2, None);
        assert!(xml.contains("<a:xfrm flipV=\"1\">"));
        assert!(xml.contains("<a:ln w=\"25400\"><a:solidFill><a:srgbClr val=\"FF0000\"/></a:solidFill><a:prstDash val=\"dash\"/><a:tailEnd type=\"triangle\" w=\"med\" len=\"lg\"/></a:ln>"), "{xml}");

        let pic = Element {
            kind: ElementKind::Picture(PictureSpec {
                bytes: Arc::new(b"\x89PNG".to_vec()),
                crop: [10_000, 0, 10_000, 0],
                geom: Some(GeomSpec::preset("ellipse")),
                line: None,
                alt: Some("Logo".into()),
            }),
            name: None,
            xfrm: Xfrm::new(0, 0, 100, 100),
        };
        let xml = element_xml(&pic, 3, Some("rId7"));
        assert!(xml.contains("descr=\"Logo\""));
        assert!(xml.contains("<a:blip r:embed=\"rId7\"/><a:srcRect l=\"10000\" r=\"10000\"/>"));
        assert!(xml.contains("<a:prstGeom prst=\"ellipse\">"));

        let cell = |t: &str| CellSpec {
            text: TextSpec::plain(t),
            fill: Some(FillSpec::Solid(rgb("EEEEEE"))),
            borders: [None, None, None, Some(LineSpec::none())],
            ..CellSpec::default()
        };
        let table = Element {
            kind: ElementKind::Table(TableSpec {
                columns: vec![1000, 2000],
                rows: vec![
                    RowSpec {
                        height: 300,
                        cells: vec![cell("A"), cell("B")],
                    },
                    RowSpec {
                        height: 300,
                        cells: vec![cell("1"), cell("2")],
                    },
                ],
                first_row: true,
                band_rows: false,
                style_id: None,
            }),
            name: None,
            xfrm: Xfrm::new(10, 20, 0, 0),
        };
        table.check().unwrap();
        let xml = element_xml(&table, 5, None);
        assert!(xml.contains(
            "<p:xfrm><a:off x=\"10\" y=\"20\"/><a:ext cx=\"3000\" cy=\"600\"/></p:xfrm>"
        ));
        assert!(xml.contains("<a:tblPr firstRow=\"1\"/>"));
        assert!(xml.contains("<a:tcPr><a:lnB><a:noFill/></a:lnB><a:solidFill><a:srgbClr val=\"EEEEEE\"/></a:solidFill></a:tcPr>"), "{xml}");
    }

    fn png() -> Vec<u8> {
        // A 1×1 PNG.
        vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0, 0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99,
            0x3D, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ]
    }

    #[test]
    fn added_elements_save_and_read_back() {
        let mut p = Presentation::new_with(&PackageOptions {
            more_layouts: true,
            title_slide: false,
            size: SlideSize {
                cx: 9_144_000,
                cy: 6_858_000,
            },
            ..PackageOptions::default()
        })
        .unwrap();
        assert_eq!(p.len(), 0);
        assert_eq!(p.layouts.len(), 4);
        let blank = p.layouts.iter().position(|l| l.name == "Blank").unwrap();
        let s = p.new_slide_with_layout(None, Some(blank)).unwrap();
        let mut g = GeomSpec::preset("roundRect");
        g.adjust.push(("adj".into(), 10_000));
        p.add_element(s, &shape(g)).unwrap();
        let pic = Element {
            kind: ElementKind::Picture(PictureSpec {
                bytes: Arc::new(png()),
                crop: [0; 4],
                geom: Some(GeomSpec::preset("roundRect")),
                line: None,
                alt: None,
            }),
            name: None,
            xfrm: Xfrm::new(0, 0, 500_000, 500_000),
        };
        p.add_element(s, &pic).unwrap();
        p.add_element(s, &pic).unwrap();
        p.set_background(s, &BackgroundSpec::Fill(FillSpec::Solid(rgb("102030"))))
            .unwrap();
        assert_eq!(p.background(s), Rgb::parse("102030").unwrap());
        p.reorder_shape(s, 2, 0).unwrap();
        assert!(p.slide(s).shapes[0].is_picture());

        let bytes = p.to_bytes().unwrap();
        assert!(crate::package_problems(&bytes).is_empty());
        let q = Presentation::from_bytes(bytes.clone()).unwrap();
        assert_eq!(q.size.cx, 9_144_000);
        let shapes = &q.slide(0).shapes;
        assert_eq!(shapes.len(), 3);
        assert_eq!(
            shapes[1].text.as_ref().unwrap().plain_text(),
            "Fast & simple"
        );
        assert_eq!(
            shapes[1].xfrm,
            Some(Xfrm::new(914_400, 914_400, 2_743_200, 914_400))
        );
        assert_eq!(q.background(0), Rgb::parse("102030").unwrap());
        // One media part for the same picture twice.
        let zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let media = zip
            .file_names()
            .filter(|n| n.as_ref().is_ok_and(|n| n.starts_with("ppt/media/")))
            .count();
        assert_eq!(media, 1);
        // The same edits save to the same bytes.
        assert_eq!(p.to_bytes().unwrap(), bytes);

        // A background on a slide read from a file is saved.
        let mut q = q;
        q.set_background(0, &BackgroundSpec::Inherit).unwrap();
        let r = Presentation::from_bytes(q.to_bytes().unwrap()).unwrap();
        assert!(r.slide(0).background.is_none());
        q.set_background(0, &BackgroundSpec::Picture(Arc::new(png())))
            .unwrap();
        let r = Presentation::from_bytes(q.to_bytes().unwrap()).unwrap();
        assert!(
            matches!(r.slide(0).background, Some(crate::Fill::Picture(_))),
            "{:?}",
            r.slide(0).background
        );
    }
}
