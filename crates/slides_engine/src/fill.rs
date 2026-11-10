//! Fills and line formatting as read, for display (never written back: the
//! source XML is kept). Gradients (`a:gradFill`), pictures (`a:blipFill`),
//! transparency, and line dashes, caps, joins and arrowheads (`a:ln`).
//!
//! Units are the file's: gradient stop positions and crop insets in
//! 1/1000 % (100 000 = 100 %), angles in 60 000ths of a degree.

use std::sync::Arc;

use docs_engine::Rgb;
use roxmltree::Node;

use crate::model::Emu;
use crate::theme::ColorCtx;
use crate::xml::*;

/// A gradient stop: position (0..=100 000), colour and opacity (255 = opaque).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GradStop {
    pub pos: i32,
    pub color: Rgb,
    pub alpha: u8,
}

/// How a gradient runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GradPath {
    /// `a:lin`: `ang` clockwise from left → right (60 000ths of a degree);
    /// `scaled` = the angle is scaled with the box's aspect ratio.
    Linear { angle: i32, scaled: bool },
    /// `a:path path="circle"`: circles from the focus outwards.
    Circle,
    /// `a:path path="rect"`.
    Rect,
    /// `a:path path="shape"`: follows the outline.
    Shape,
}

/// `a:gradFill`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Gradient {
    /// Sorted by position; at least one.
    pub stops: Vec<GradStop>,
    pub path: GradPath,
    /// Path gradients: the focus rectangle as insets from the left, top,
    /// right and bottom edges (1/1000 %); position 0 is there.
    pub focus: [i32; 4],
    /// The gradient turns with the shape (`rotWithShape`, default on).
    pub rot_with_shape: bool,
}

impl Gradient {
    /// The colour and opacity at `pos` (0..=100 000), interpolated.
    pub fn at(&self, pos: i32) -> (Rgb, u8) {
        let s = &self.stops;
        let first = s[0];
        if pos <= first.pos {
            return (first.color, first.alpha);
        }
        for w in s.windows(2) {
            let (a, b) = (w[0], w[1]);
            if pos <= b.pos {
                let t = if b.pos == a.pos {
                    1.0
                } else {
                    (pos - a.pos) as f32 / (b.pos - a.pos) as f32
                };
                let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
                let c = Rgb(std::array::from_fn(|i| mix(a.color.0[i], b.color.0[i])));
                return (c, mix(a.alpha, b.alpha));
            }
        }
        let last = s[s.len() - 1];
        (last.color, last.alpha)
    }
}

/// A picture fill (`a:blipFill`) or a picture's own picture.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlipFill {
    /// The media part, when the relationship resolves.
    pub part: Option<String>,
    /// `a:srcRect`: what's cut off each edge, left, top, right, bottom
    /// (1/1000 %; negative = padding).
    pub crop: [i32; 4],
    /// `a:tile` (drawn stretched for now).
    pub tile: bool,
    /// `a:alphaModFix`, 255 = opaque.
    pub alpha: u8,
}

/// A fill as read.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Fill {
    None,
    /// Colour and opacity (255 = opaque).
    Solid(Rgb, u8),
    Gradient(Arc<Gradient>),
    Picture(Arc<BlipFill>),
}

impl Fill {
    /// One colour standing for the fill (chrome, contrast checks).
    pub fn main_color(&self) -> Option<Rgb> {
        match self {
            Fill::Solid(c, _) => Some(*c),
            Fill::Gradient(g) => Some(g.stops[0].color),
            Fill::None | Fill::Picture(_) => None,
        }
    }
}

/// `a:prstDash` / `a:custDash`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Dash {
    #[default]
    Solid,
    Dot,
    Dash,
    LgDash,
    DashDot,
    LgDashDot,
    LgDashDotDot,
    SysDash,
    SysDot,
    SysDashDot,
    SysDashDotDot,
    /// Dash and space lengths in 1/1000 % of the line width.
    Custom(Arc<[(u32, u32)]>),
}

impl Dash {
    pub fn from_prst(v: &str) -> Self {
        match v {
            "dot" => Self::Dot,
            "dash" => Self::Dash,
            "lgDash" => Self::LgDash,
            "dashDot" => Self::DashDot,
            "lgDashDot" => Self::LgDashDot,
            "lgDashDotDot" => Self::LgDashDotDot,
            "sysDash" => Self::SysDash,
            "sysDot" => Self::SysDot,
            "sysDashDot" => Self::SysDashDot,
            "sysDashDotDot" => Self::SysDashDotDot,
            _ => Self::Solid,
        }
    }

    /// Dash, gap, dash, gap... in multiples of the line width (ECMA-376
    /// §20.1.10.48); `None` for a solid line.
    pub fn pattern(&self) -> Option<Vec<f32>> {
        let p: &[f32] = match self {
            Self::Solid => return None,
            Self::Dot => &[1.0, 3.0],
            Self::Dash => &[4.0, 3.0],
            Self::LgDash => &[8.0, 3.0],
            Self::DashDot => &[4.0, 3.0, 1.0, 3.0],
            Self::LgDashDot => &[8.0, 3.0, 1.0, 3.0],
            Self::LgDashDotDot => &[8.0, 3.0, 1.0, 3.0, 1.0, 3.0],
            Self::SysDash => &[3.0, 1.0],
            Self::SysDot => &[1.0, 1.0],
            Self::SysDashDot => &[3.0, 1.0, 1.0, 1.0],
            Self::SysDashDotDot => &[3.0, 1.0, 1.0, 1.0, 1.0, 1.0],
            Self::Custom(v) => {
                let out: Vec<f32> = v
                    .iter()
                    .flat_map(|&(d, s)| [d as f32 / 100_000.0, s as f32 / 100_000.0])
                    .collect();
                return (out.iter().sum::<f32>() > 0.0).then_some(out);
            }
        };
        Some(p.to_vec())
    }
}

/// `a:ln/@cap`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LineCap {
    #[default]
    Flat,
    Round,
    Square,
}

/// `a:round`, `a:bevel`, `a:miter`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LineJoin {
    #[default]
    Round,
    Bevel,
    Miter,
}

/// `a:headEnd` / `a:tailEnd` type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArrowKind {
    Triangle,
    Stealth,
    /// Open arrow (two strokes).
    Arrow,
    Oval,
    Diamond,
}

/// `sm` / `med` / `lg`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ArrowSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl ArrowSize {
    fn parse(v: Option<&str>) -> Self {
        match v {
            Some("sm") => Self::Small,
            Some("lg") => Self::Large,
            _ => Self::Medium,
        }
    }

    /// Multiple of the line width.
    pub fn factor(self) -> f32 {
        match self {
            Self::Small => 2.0,
            Self::Medium => 3.0,
            Self::Large => 5.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Arrow {
    pub kind: ArrowKind,
    pub width: ArrowSize,
    pub length: ArrowSize,
}

/// Line formatting other than colour and width (those are in
/// [`crate::Look::line`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LineStyle {
    pub dash: Dash,
    pub cap: LineCap,
    pub join: LineJoin,
    /// At the start of the path (`a:headEnd`).
    pub head: Option<Arrow>,
    /// At the end (`a:tailEnd`).
    pub tail: Option<Arrow>,
    /// Opacity of the line colour, 255 = opaque.
    pub alpha: u8,
}

impl Default for LineStyle {
    fn default() -> Self {
        Self {
            dash: Dash::Solid,
            cap: LineCap::Flat,
            join: LineJoin::Round,
            head: None,
            tail: None,
            alpha: 255,
        }
    }
}

/// How a picture (`p:pic`) is shown: cropping and opacity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PictureLook {
    /// `a:srcRect` insets (1/1000 %), left, top, right, bottom.
    pub crop: [i32; 4],
    pub alpha: u8,
}

impl Default for PictureLook {
    fn default() -> Self {
        Self {
            crop: [0; 4],
            alpha: 255,
        }
    }
}

fn arrow(n: Node) -> Option<Arrow> {
    let kind = match n.attribute("type")? {
        "triangle" => ArrowKind::Triangle,
        "stealth" => ArrowKind::Stealth,
        "arrow" => ArrowKind::Arrow,
        "oval" => ArrowKind::Oval,
        "diamond" => ArrowKind::Diamond,
        _ => return None,
    };
    Some(Arrow {
        kind,
        width: ArrowSize::parse(n.attribute("w")),
        length: ArrowSize::parse(n.attribute("len")),
    })
}

/// `a:srcRect` insets.
pub(crate) fn src_rect(n: Option<Node>) -> [i32; 4] {
    let Some(n) = n else { return [0; 4] };
    ["l", "t", "r", "b"].map(|k| attr_i64(n, k).unwrap_or(0).clamp(-10_000_000, 99_000) as i32)
}

/// Opacity of `a:alphaModFix` inside a blip.
fn blip_alpha(blip: Option<Node>) -> u8 {
    blip.and_then(|b| child(b, NS_A, "alphaModFix"))
        .and_then(|a| attr_i64(a, "amt"))
        .map_or(255, |v| {
            (v.clamp(0, 100_000) as f32 / 100_000.0 * 255.0).round() as u8
        })
}

/// Picture look of a `p:pic`'s `p:blipFill`.
pub(crate) fn picture_look(blip_fill: Option<Node>) -> PictureLook {
    PictureLook {
        crop: src_rect(blip_fill.and_then(|b| child(b, NS_A, "srcRect"))),
        alpha: blip_alpha(blip_fill.and_then(|b| child(b, NS_A, "blip"))),
    }
}

impl ColorCtx<'_> {
    /// A full fill element (`a:noFill`, `a:solidFill`, `a:gradFill`,
    /// `a:blipFill`, `a:pattFill`). `resolve` maps a relationship id to a
    /// media part. `None` for elements that aren't fills (`a:grpFill`).
    pub fn full_fill(&self, n: Node, resolve: &dyn Fn(&str) -> Option<String>) -> Option<Fill> {
        Some(match n.tag_name().name() {
            "noFill" => Fill::None,
            "solidFill" => {
                let (c, a) = n
                    .children()
                    .filter(|c| c.is_element())
                    .find_map(|c| self.color_alpha(c))?;
                Fill::Solid(c, a)
            }
            "gradFill" => match self.gradient(n) {
                Some(g) => Fill::Gradient(Arc::new(g)),
                None => Fill::None,
            },
            "blipFill" => {
                let blip = child(n, NS_A, "blip");
                let part = blip.and_then(|b| r_attr(b, "embed")).and_then(resolve);
                Fill::Picture(Arc::new(BlipFill {
                    part,
                    crop: src_rect(child(n, NS_A, "srcRect")),
                    tile: child(n, NS_A, "tile").is_some(),
                    alpha: blip_alpha(blip),
                }))
            }
            // Patterns: the foreground colour (as before).
            "pattFill" => {
                let (c, a) = path(n, &[(NS_A, "fgClr")])
                    .and_then(|f| {
                        f.children()
                            .filter(|c| c.is_element())
                            .find_map(|c| self.color_alpha(c))
                    })
                    .unwrap_or((Rgb([0, 0, 0]), 255));
                Fill::Solid(c, a)
            }
            _ => return None,
        })
    }

    /// `a:gradFill` with resolved stop colours; `None` without stops.
    pub fn gradient(&self, n: Node) -> Option<Gradient> {
        let mut stops: Vec<GradStop> = path(n, &[(NS_A, "gsLst")])
            .into_iter()
            .flat_map(|l| children(l, NS_A, "gs"))
            .filter_map(|gs| {
                let pos = attr_i64(gs, "pos").unwrap_or(0).clamp(0, 100_000) as i32;
                let (color, alpha) = gs
                    .children()
                    .filter(|c| c.is_element())
                    .find_map(|c| self.color_alpha(c))?;
                Some(GradStop { pos, color, alpha })
            })
            .collect();
        if stops.is_empty() {
            return None;
        }
        stops.sort_by_key(|s| s.pos);
        let lin = child(n, NS_A, "lin");
        let pth = child(n, NS_A, "path");
        let path = match (lin, pth) {
            (_, Some(p)) => match p.attribute("path") {
                Some("circle") => GradPath::Circle,
                Some("rect") => GradPath::Rect,
                _ => GradPath::Shape,
            },
            (Some(l), None) => GradPath::Linear {
                angle: attr_i64(l, "ang").unwrap_or(0).rem_euclid(21_600_000) as i32,
                scaled: attr_bool(l, "scaled").unwrap_or(false),
            },
            // No direction: PowerPoint shows a left → right gradient.
            (None, None) => GradPath::Linear {
                angle: 0,
                scaled: false,
            },
        };
        let focus = pth
            .and_then(|p| child(p, NS_A, "fillToRect"))
            .map(|r| ["l", "t", "r", "b"].map(|k| attr_i64(r, k).unwrap_or(0) as i32))
            .unwrap_or([0; 4]);
        Some(Gradient {
            stops,
            path,
            focus,
            rot_with_shape: attr_bool(n, "rotWithShape").unwrap_or(true),
        })
    }

    /// Line formatting of an `a:ln`, on top of `base` (a theme line style).
    pub fn line_style(&self, ln: Node, base: &LineStyle) -> LineStyle {
        let mut s = base.clone();
        s.cap = match ln.attribute("cap") {
            Some("rnd") => LineCap::Round,
            Some("sq") => LineCap::Square,
            Some("flat") => LineCap::Flat,
            _ => s.cap,
        };
        for c in ln.children().filter(|c| c.is_element()) {
            match c.tag_name().name() {
                "prstDash" => s.dash = Dash::from_prst(c.attribute("val").unwrap_or("solid")),
                "custDash" => {
                    let v: Vec<(u32, u32)> = children(c, NS_A, "ds")
                        .map(|d| {
                            let g = |k| attr_i64(d, k).unwrap_or(0).clamp(0, 100_000_000) as u32;
                            (g("d"), g("sp"))
                        })
                        .collect();
                    s.dash = if v.is_empty() {
                        Dash::Solid
                    } else {
                        Dash::Custom(v.into())
                    };
                }
                "round" => s.join = LineJoin::Round,
                "bevel" => s.join = LineJoin::Bevel,
                "miter" => s.join = LineJoin::Miter,
                "headEnd" => s.head = arrow(c),
                "tailEnd" => s.tail = arrow(c),
                "solidFill" => {
                    if let Some((_, a)) = c
                        .children()
                        .filter(|k| k.is_element())
                        .find_map(|k| self.color_alpha(k))
                    {
                        s.alpha = a;
                    }
                }
                _ => {}
            }
        }
        s
    }
}

/// Width of an `a:ln` (EMU), if given.
pub(crate) fn line_width(ln: Node) -> Option<Emu> {
    attr_i64(ln, "w").map(|w| w.clamp(0, 20_116_800))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ClrMap, Theme};

    fn with<R>(xml: &str, f: impl FnOnce(&ColorCtx, Node) -> R) -> R {
        let src = format!(
            r#"<w xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">{xml}</w>"#
        );
        let doc = roxmltree::Document::parse(&src).unwrap();
        let theme = Theme::parse(
            r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:themeElements><a:clrScheme name="x"><a:dk1><a:srgbClr val="000000"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:accent1><a:srgbClr val="4472C4"/></a:accent1></a:clrScheme></a:themeElements></a:theme>"#,
        );
        let map = ClrMap::default();
        let cx = ColorCtx {
            theme: &theme,
            map: &map,
            ph: None,
        };
        f(&cx, doc.root_element().first_element_child().unwrap())
    }

    #[test]
    fn gradient_stops_resolve_scheme_colours_and_alpha() {
        let g = with(
            r#"<a:gradFill rotWithShape="0"><a:gsLst>
              <a:gs pos="100000"><a:srgbClr val="FF0000"><a:alpha val="50000"/></a:srgbClr></a:gs>
              <a:gs pos="0"><a:schemeClr val="accent1"><a:lumMod val="50000"/></a:schemeClr></a:gs>
            </a:gsLst><a:lin ang="5400000" scaled="1"/></a:gradFill>"#,
            |cx, n| cx.gradient(n).unwrap(),
        );
        assert_eq!(g.stops.len(), 2);
        assert_eq!(g.stops[0].pos, 0, "sorted by position");
        // accent1 at half luminance.
        let c = g.stops[0].color.0;
        assert!(c[2] > c[0] && c[2] < 0xC4, "{c:?}");
        assert_eq!(g.stops[1].color, Rgb([255, 0, 0]));
        assert_eq!(g.stops[1].alpha, 128);
        assert_eq!(
            g.path,
            GradPath::Linear {
                angle: 5_400_000,
                scaled: true
            }
        );
        assert!(!g.rot_with_shape);
        let (mid, a) = g.at(50_000);
        assert!(mid.0[0] > 100 && a > 128 && a < 255, "{mid:?} {a}");
    }

    #[test]
    fn path_gradients_keep_their_focus() {
        let g = with(
            r#"<a:gradFill><a:gsLst><a:gs pos="0"><a:srgbClr val="FFFFFF"/></a:gs><a:gs pos="100000"><a:srgbClr val="000000"/></a:gs></a:gsLst><a:path path="circle"><a:fillToRect l="50000" t="50000" r="50000" b="50000"/></a:path></a:gradFill>"#,
            |cx, n| cx.gradient(n).unwrap(),
        );
        assert_eq!(g.path, GradPath::Circle);
        assert_eq!(g.focus, [50_000; 4]);
        assert!(g.rot_with_shape);
    }

    #[test]
    fn line_dash_caps_joins_and_arrowheads() {
        let s = with(
            r#"<a:ln w="25400" cap="rnd"><a:solidFill><a:srgbClr val="00FF00"><a:alpha val="25000"/></a:srgbClr></a:solidFill><a:prstDash val="lgDashDot"/><a:miter lim="800000"/><a:headEnd type="oval" w="sm" len="lg"/><a:tailEnd type="stealth"/></a:ln>"#,
            |cx, n| cx.line_style(n, &LineStyle::default()),
        );
        assert_eq!(s.dash, Dash::LgDashDot);
        assert_eq!(s.cap, LineCap::Round);
        assert_eq!(s.join, LineJoin::Miter);
        assert_eq!(
            s.head,
            Some(Arrow {
                kind: ArrowKind::Oval,
                width: ArrowSize::Small,
                length: ArrowSize::Large
            })
        );
        assert_eq!(s.tail.unwrap().kind, ArrowKind::Stealth);
        assert_eq!(s.alpha, 64);
        assert_eq!(Dash::LgDashDot.pattern().unwrap(), vec![8.0, 3.0, 1.0, 3.0]);
        assert_eq!(Dash::SysDot.pattern().unwrap(), vec![1.0, 1.0]);
        assert!(Dash::Solid.pattern().is_none());
        let c = with(
            r#"<a:ln><a:custDash><a:ds d="300000" sp="100000"/></a:custDash><a:tailEnd type="none"/></a:ln>"#,
            |cx, n| cx.line_style(n, &s),
        );
        assert_eq!(c.dash.pattern().unwrap(), vec![3.0, 1.0]);
        assert_eq!(c.tail, None, "type none removes the arrow");
        assert_eq!(c.head, s.head, "inherited");
    }

    #[test]
    fn picture_fills_resolve_their_part() {
        let f = with(
            r#"<a:blipFill><a:blip r:embed="rId4"><a:alphaModFix amt="40000"/></a:blip><a:srcRect l="10000" r="-5000"/><a:stretch/></a:blipFill>"#,
            |cx, n| {
                cx.full_fill(n, &|id| {
                    (id == "rId4").then(|| "ppt/media/image1.png".into())
                })
            },
        );
        let Some(Fill::Picture(b)) = f else {
            panic!("{f:?}")
        };
        assert_eq!(b.part.as_deref(), Some("ppt/media/image1.png"));
        assert_eq!(b.crop, [10_000, 0, -5_000, 0]);
        assert_eq!(b.alpha, 102);
    }
}
