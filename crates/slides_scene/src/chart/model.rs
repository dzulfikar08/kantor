//! ChartML (`c:chartSpace`) → [`Chart`], from the values cached in the
//! chart part (`c:numCache`, `c:strCache`); the embedded workbook is never
//! opened. Colours are resolved against the slide's theme while parsing.

use roxmltree::Node;
use slides_engine::theme::ColorCtx;
use slides_engine::{EMU_PER_PX, Fill, LineStyle};

pub const NS_C: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
pub const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";

fn is(n: Node, ns: &str, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some(ns)
}

fn kid<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| is(*c, NS_C, name))
}

fn kids<'a, 'i>(n: Node<'a, 'i>, name: &'static str) -> impl Iterator<Item = Node<'a, 'i>> {
    n.children().filter(move |c| is(*c, NS_C, name))
}

fn a_kid<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| is(*c, NS_A, name))
}

/// `<c:x val="..."/>` of child `name`.
fn val<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
    kid(n, name).and_then(|c| c.attribute("val"))
}

fn val_f(n: Node, name: &str) -> Option<f64> {
    val(n, name)?
        .trim()
        .parse()
        .ok()
        .filter(|v: &f64| v.is_finite())
}

/// Boolean `c:` flags: a missing `val` means true.
fn flag(n: Node, name: &str) -> Option<bool> {
    kid(n, name).map(|c| !matches!(c.attribute("val"), Some("0" | "false")))
}

/// A line: its fill, width (px) and formatting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    /// `None` = automatic; `Some(Fill::None)` = no line.
    pub fill: Option<Fill>,
    pub width: Option<f32>,
    pub style: LineStyle,
}

/// `c:spPr`: fill and outline (`None` = automatic).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Look {
    pub fill: Option<Fill>,
    pub line: Option<Line>,
}

impl Look {
    fn parse(n: Option<Node>, cx: &ColorCtx) -> Self {
        let Some(sp) = n.and_then(|n| kid(n, "spPr")) else {
            return Self::default();
        };
        let fill = sp
            .children()
            .filter(|c| c.is_element() && c.tag_name().namespace() == Some(NS_A))
            .find_map(|c| cx.full_fill(c, &|_| None));
        let line = a_kid(sp, "ln").map(|ln| Line {
            fill: ln
                .children()
                .filter(|c| c.is_element())
                .find_map(|c| cx.full_fill(c, &|_| None)),
            width: ln
                .attribute("w")
                .and_then(|w| w.parse::<f32>().ok())
                .map(|w| w / EMU_PER_PX),
            style: cx.line_style(ln, &LineStyle::default()),
        });
        Self { fill, line }
    }

    /// `self` with what it leaves automatic taken from `o`.
    pub fn or(&self, o: &Look) -> Look {
        Look {
            fill: self.fill.clone().or_else(|| o.fill.clone()),
            line: match (&self.line, &o.line) {
                (Some(a), Some(b)) => Some(Line {
                    fill: a.fill.clone().or_else(|| b.fill.clone()),
                    width: a.width.or(b.width),
                    style: a.style.clone(),
                }),
                (a, b) => a.clone().or_else(|| b.clone()),
            },
        }
    }
}

/// Character formatting (`a:defRPr` / `a:rPr`); `None` = inherited.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Font {
    pub size_pt: Option<f32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub color: Option<(slides_engine::Rgb, u8)>,
    /// Typeface as written (`+mn-lt` resolved later).
    pub face: Option<String>,
}

impl Font {
    fn from_rpr(r: Node, cx: &ColorCtx) -> Self {
        let b = |k| r.attribute(k).map(|v| v == "1" || v == "true");
        Font {
            size_pt: r
                .attribute("sz")
                .and_then(|s| s.parse::<f32>().ok())
                .map(|s| s / 100.0),
            bold: b("b"),
            italic: b("i"),
            color: a_kid(r, "solidFill").and_then(|f| {
                f.children()
                    .filter(|c| c.is_element())
                    .find_map(|c| cx.color_alpha(c))
            }),
            face: a_kid(r, "latin")
                .and_then(|l| l.attribute("typeface"))
                .filter(|f| !f.is_empty())
                .map(str::to_owned),
        }
    }

    /// `self` over `base`.
    pub fn over(&self, base: &Font) -> Font {
        Font {
            size_pt: self.size_pt.or(base.size_pt),
            bold: self.bold.or(base.bold),
            italic: self.italic.or(base.italic),
            color: self.color.or(base.color),
            face: self.face.clone().or_else(|| base.face.clone()),
        }
    }

    /// The first paragraph's default run properties of a `c:txPr` or
    /// `c:rich` (`a:p/a:pPr/a:defRPr`).
    fn of_body(body: Node, cx: &ColorCtx) -> Self {
        body.children()
            .filter(|c| is(*c, NS_A, "p"))
            .find_map(|p| a_kid(p, "pPr").and_then(|ppr| a_kid(ppr, "defRPr")))
            .map(|r| Font::from_rpr(r, cx))
            .unwrap_or_default()
    }

    fn of_txpr(n: Option<Node>, cx: &ColorCtx) -> Self {
        n.and_then(|n| kid(n, "txPr"))
            .map(|t| Font::of_body(t, cx))
            .unwrap_or_default()
    }
}

/// A title: its text (`None` = automatic) and formatting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Title {
    /// Paragraphs (`c:rich`) or the cached cell text (`c:strRef`).
    pub text: Option<Vec<String>>,
    pub font: Font,
    pub look: Look,
    pub overlay: bool,
}

impl Title {
    fn parse(t: Node, cx: &ColorCtx) -> Self {
        let tx = kid(t, "tx");
        let rich = tx.and_then(|tx| kid(tx, "rich"));
        let mut font = Font::of_txpr(Some(t), cx);
        let text = if let Some(rich) = rich {
            font = Font::of_body(rich, cx).over(&font);
            // The first run's own formatting wins over the defaults.
            if let Some(r) = rich
                .descendants()
                .find(|d| is(*d, NS_A, "r"))
                .and_then(|r| a_kid(r, "rPr"))
            {
                font = Font::from_rpr(r, cx).over(&font);
            }
            Some(
                rich.children()
                    .filter(|c| is(*c, NS_A, "p"))
                    .map(|p| {
                        p.descendants()
                            .filter(|d| is(*d, NS_A, "t"))
                            .filter_map(|d| d.text())
                            .collect::<String>()
                    })
                    .collect(),
            )
        } else {
            tx.and_then(|tx| kid(tx, "strRef"))
                .map(|r| vec![str_cache(r).into_iter().flatten().collect::<String>()])
        };
        Title {
            text,
            font,
            look: Look::parse(Some(t), cx),
            overlay: flag(t, "overlay").unwrap_or(false),
        }
    }
}

/// `c:manualLayout` as fractions of the chart.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ManualLayout {
    /// The rectangle is the plot's inside (no tick labels).
    pub inner: bool,
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub w: Option<f32>,
    pub h: Option<f32>,
}

fn manual_layout(n: Option<Node>) -> Option<ManualLayout> {
    let m = kid(kid(n?, "layout")?, "manualLayout")?;
    let edge = |mode: &str| val(m, mode) == Some("edge");
    let f = |k: &str| val_f(m, k).map(|v| v as f32);
    let l = ManualLayout {
        inner: val(m, "layoutTarget") == Some("inner"),
        // Offsets from the automatic position ("factor") aren't modelled.
        x: f("x").filter(|_| edge("xMode")),
        y: f("y").filter(|_| edge("yMode")),
        w: f("w"),
        h: f("h"),
    };
    (l.x.is_some() || l.y.is_some() || l.w.is_some() || l.h.is_some()).then_some(l)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Grouping {
    #[default]
    Clustered,
    Standard,
    Stacked,
    PercentStacked,
}

impl Grouping {
    fn parse(v: Option<&str>, default: Grouping) -> Self {
        match v {
            Some("stacked") => Self::Stacked,
            Some("percentStacked") => Self::PercentStacked,
            Some("clustered") => Self::Clustered,
            Some("standard") => Self::Standard,
            _ => default,
        }
    }

    pub fn stacked(self) -> bool {
        matches!(self, Self::Stacked | Self::PercentStacked)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Bar {
        horizontal: bool,
        grouping: Grouping,
        /// Gap between categories, % of a bar's width.
        gap: f32,
        /// Overlap of a category's bars, -100..100 %.
        overlap: f32,
    },
    Line {
        grouping: Grouping,
    },
    Area {
        grouping: Grouping,
    },
    Pie {
        /// Angle of the first slice, degrees clockwise from 12 o'clock.
        first_angle: f32,
        /// Doughnut hole, % of the radius.
        hole: Option<f32>,
    },
    Scatter {
        /// `lineMarker`, `line`, `marker`, `smooth`, `smoothMarker`, `none`.
        style: String,
    },
}

/// Marker symbols.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Symbol {
    None,
    Auto,
    Circle,
    Square,
    Diamond,
    Triangle,
    X,
    Star,
    Plus,
    Dash,
    Dot,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    pub symbol: Symbol,
    /// 2..72 pt.
    pub size: f32,
    pub look: Look,
}

impl Marker {
    fn parse(n: Option<Node>, cx: &ColorCtx) -> Option<Self> {
        let m = n.and_then(|n| kid(n, "marker"))?;
        Some(Marker {
            symbol: match val(m, "symbol") {
                Some("none") => Symbol::None,
                Some("circle") => Symbol::Circle,
                Some("square") => Symbol::Square,
                Some("diamond") => Symbol::Diamond,
                Some("triangle") => Symbol::Triangle,
                Some("x") => Symbol::X,
                Some("star") => Symbol::Star,
                Some("plus") => Symbol::Plus,
                Some("dash") => Symbol::Dash,
                Some("dot") => Symbol::Dot,
                _ => Symbol::Auto,
            },
            size: val_f(m, "size").unwrap_or(5.0).clamp(2.0, 72.0) as f32,
            look: Look::parse(Some(m), cx),
        })
    }
}

/// Data labels (`c:dLbls`) of a group, series or point.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Labels {
    pub delete: bool,
    pub show_val: bool,
    pub show_percent: bool,
    pub show_cat: bool,
    pub show_ser: bool,
    /// `outEnd`, `inEnd`, `ctr`, `inBase`, `t`, `b`, `l`, `r`, `bestFit`.
    pub pos: Option<String>,
    pub num_fmt: Option<String>,
    pub separator: Option<String>,
    pub font: Font,
    /// A label moved by hand (`c:dLbl/c:layout`): offset from its usual
    /// place, as fractions of the chart's width and height.
    pub offset: Option<(f32, f32)>,
    /// Per-point overrides (`c:dLbl`), by point index.
    pub points: Vec<(usize, Labels)>,
}

impl Labels {
    fn parse(n: Node, cx: &ColorCtx) -> Self {
        let mut l = Labels {
            delete: flag(n, "delete").unwrap_or(false),
            show_val: flag(n, "showVal").unwrap_or(false),
            show_percent: flag(n, "showPercent").unwrap_or(false),
            show_cat: flag(n, "showCatName").unwrap_or(false),
            show_ser: flag(n, "showSerName").unwrap_or(false),
            pos: val(n, "dLblPos").map(str::to_owned),
            num_fmt: kid(n, "numFmt")
                .filter(|f| f.attribute("sourceLinked") != Some("1"))
                .and_then(|f| f.attribute("formatCode"))
                .map(str::to_owned),
            separator: kid(n, "separator")
                .and_then(|s| s.text())
                .map(str::to_owned),
            font: Font::of_txpr(Some(n), cx),
            offset: kid(n, "layout")
                .and_then(|l| kid(l, "manualLayout"))
                .filter(|m| val(*m, "xMode") != Some("edge") && val(*m, "yMode") != Some("edge"))
                .map(|m| {
                    let f = |k| val_f(m, k).unwrap_or(0.0).clamp(-1.0, 1.0) as f32;
                    (f("x"), f("y"))
                }),
            points: Vec::new(),
        };
        l.points = kids(n, "dLbl")
            .filter_map(|d| {
                let idx = val(d, "idx")?.parse().ok()?;
                Some((idx, Labels::parse(d, cx)))
            })
            .collect();
        l
    }

    pub fn shown(&self) -> bool {
        !self.delete && (self.show_val || self.show_percent || self.show_cat || self.show_ser)
    }

    /// The labels of point `i` (its own override, else these).
    pub fn point(&self, i: usize) -> Labels {
        match self.points.iter().find(|(k, _)| *k == i) {
            Some((_, p)) => {
                // A point override that only deletes or moves keeps the rest.
                let mut l = p.clone();
                l.font = p.font.over(&self.font);
                if l.num_fmt.is_none() {
                    l.num_fmt = self.num_fmt.clone();
                }
                if l.pos.is_none() {
                    l.pos = self.pos.clone();
                }
                l.points.clear();
                l
            }
            None => Labels {
                points: Vec::new(),
                ..self.clone()
            },
        }
    }
}

/// A data point override (`c:dPt`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Point {
    pub idx: usize,
    pub look: Look,
    pub explosion: Option<f32>,
    pub marker: Option<Marker>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Series {
    pub idx: usize,
    pub order: usize,
    pub name: Option<String>,
    /// Category labels (`c:cat`, or `c:xVal` of scatter series as text).
    pub cats: Vec<String>,
    /// Number format of the categories when they're numbers (dates).
    pub cat_fmt: Option<String>,
    /// Category values when they're numbers.
    pub cat_nums: Vec<Option<f64>>,
    pub vals: Vec<Option<f64>>,
    /// Scatter x values.
    pub xs: Option<Vec<Option<f64>>>,
    pub fmt: Option<String>,
    pub look: Look,
    pub marker: Option<Marker>,
    pub points: Vec<Point>,
    pub labels: Option<Labels>,
    pub smooth: bool,
    pub explosion: f32,
    pub invert_if_negative: bool,
}

impl Series {
    pub fn point(&self, i: usize) -> Option<&Point> {
        self.points.iter().find(|p| p.idx == i)
    }
}

/// One plot type of the plot area (`c:barChart`, `c:lineChart`...).
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub kind: Kind,
    pub vary_colors: bool,
    pub series: Vec<Series>,
    pub ax_ids: Vec<i64>,
    pub labels: Option<Labels>,
    /// Lines show markers (`c:marker val=1` on line charts).
    pub markers: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxisKind {
    Cat,
    Val,
    Date,
    Ser,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxPos {
    B,
    T,
    L,
    R,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Axis {
    pub id: i64,
    pub kind: AxisKind,
    pub pos: AxPos,
    pub delete: bool,
    pub cross_ax: i64,
    pub reversed: bool,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub major_unit: Option<f64>,
    pub log_base: Option<f64>,
    /// Format code and whether it's linked to the source.
    pub num_fmt: Option<(String, bool)>,
    pub major_grid: Option<Look>,
    pub minor_grid: Option<Look>,
    /// `nextTo`, `low`, `high`, `none`.
    pub tick_lbl_pos: String,
    pub major_tick: String,
    pub font: Font,
    /// Text rotation of the tick labels (degrees), if set.
    pub rot: Option<f32>,
    pub look: Look,
    pub title: Option<Title>,
    /// `autoZero`, `min`, `max`, or a value (`crossesAt`).
    pub crosses: Option<String>,
    pub crosses_at: Option<f64>,
    /// `between` (categories in bands) or `midCat` (on the ticks).
    pub cross_between: Option<String>,
}

impl Axis {
    fn parse(n: Node, cx: &ColorCtx) -> Option<Self> {
        let kind = match n.tag_name().name() {
            "catAx" => AxisKind::Cat,
            "valAx" => AxisKind::Val,
            "dateAx" => AxisKind::Date,
            "serAx" => AxisKind::Ser,
            _ => return None,
        };
        let scaling = kid(n, "scaling");
        let grid = |k| {
            kid(n, k).map(|g| {
                let l = Look::parse(Some(g), cx);
                if l.line.is_none() { Look::default() } else { l }
            })
        };
        Some(Axis {
            id: val(n, "axId")?.parse().ok()?,
            kind,
            pos: match val(n, "axPos") {
                Some("t") => AxPos::T,
                Some("l") => AxPos::L,
                Some("r") => AxPos::R,
                _ => AxPos::B,
            },
            delete: flag(n, "delete").unwrap_or(false),
            cross_ax: val(n, "crossAx").and_then(|v| v.parse().ok()).unwrap_or(0),
            reversed: scaling.and_then(|s| val(s, "orientation")) == Some("maxMin"),
            min: scaling.and_then(|s| val_f(s, "min")),
            max: scaling.and_then(|s| val_f(s, "max")),
            major_unit: val_f(n, "majorUnit").filter(|u| *u > 0.0),
            log_base: scaling
                .and_then(|s| val_f(s, "logBase"))
                .filter(|b| *b >= 2.0),
            num_fmt: kid(n, "numFmt").and_then(|f| {
                Some((
                    f.attribute("formatCode")?.to_owned(),
                    matches!(f.attribute("sourceLinked"), Some("1" | "true")),
                ))
            }),
            major_grid: grid("majorGridlines"),
            minor_grid: grid("minorGridlines"),
            tick_lbl_pos: val(n, "tickLblPos").unwrap_or("nextTo").to_owned(),
            major_tick: val(n, "majorTickMark").unwrap_or("out").to_owned(),
            font: Font::of_txpr(Some(n), cx),
            rot: kid(n, "txPr")
                .and_then(|t| a_kid(t, "bodyPr"))
                .and_then(|b| b.attribute("rot"))
                .and_then(|r| r.parse::<f32>().ok())
                .filter(|r| r.abs() <= 5_400_000.0)
                .map(|r| r / 60_000.0),
            look: Look::parse(Some(n), cx),
            title: kid(n, "title").map(|t| Title::parse(t, cx)),
            crosses: val(n, "crosses").map(str::to_owned),
            crosses_at: val_f(n, "crossesAt"),
            cross_between: val(n, "crossBetween").map(str::to_owned),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegendPos {
    R,
    L,
    T,
    B,
    Tr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Legend {
    pub pos: LegendPos,
    pub overlay: bool,
    pub font: Font,
    pub look: Look,
    /// Entries removed (`c:legendEntry/c:delete`), by index.
    pub deleted: Vec<usize>,
    pub layout: Option<ManualLayout>,
}

/// A parsed chart.
#[derive(Clone, Debug, PartialEq)]
pub struct Chart {
    pub title: Option<Title>,
    pub auto_title_deleted: bool,
    pub legend: Option<Legend>,
    pub groups: Vec<Group>,
    pub axes: Vec<Axis>,
    pub plot_look: Look,
    pub plot_layout: Option<ManualLayout>,
    /// The chart area (`c:chartSpace/c:spPr`).
    pub space_look: Look,
    /// Default text of the whole chart (`c:chartSpace/c:txPr`).
    pub font: Font,
    pub rounded: bool,
    /// `c:style` (1..48), for defaults.
    pub style: u32,
    /// Plot types seen that aren't drawn (radar, bubble, 3-D, stock...).
    pub unsupported: Vec<String>,
}

/// Cached strings of a `c:strRef`/`c:strLit`/`c:multiLvlStrRef` (by
/// `c:pt/@idx`; gaps are `None`).
fn str_cache(r: Node) -> Vec<Option<String>> {
    let cache = r
        .descendants()
        .find(|d| is(*d, NS_C, "strCache") || is(*d, NS_C, "lvl"))
        .or_else(|| (r.tag_name().name() == "strLit").then_some(r));
    pts(cache, |s| Some(s.to_owned()))
}

fn pts<T>(cache: Option<Node>, f: impl Fn(&str) -> Option<T>) -> Vec<Option<T>> {
    let Some(cache) = cache else {
        return Vec::new();
    };
    let count = val(cache, "ptCount")
        .and_then(|c| c.parse::<usize>().ok())
        .unwrap_or(0)
        .min(100_000);
    let mut out: Vec<Option<T>> = (0..count).map(|_| None).collect();
    for p in kids(cache, "pt") {
        let Some(i) = p.attribute("idx").and_then(|i| i.parse::<usize>().ok()) else {
            continue;
        };
        if i >= 100_000 {
            continue;
        }
        let v = kid(p, "v").and_then(|v| v.text()).and_then(&f);
        if out.len() <= i {
            out.resize_with(i + 1, || None);
        }
        out[i] = v;
    }
    out
}

/// Numbers of a `c:numRef`/`c:numLit` and their cache's format code.
fn num_cache(r: Node) -> (Vec<Option<f64>>, Option<String>) {
    let cache = r
        .descendants()
        .find(|d| is(*d, NS_C, "numCache"))
        .or_else(|| (r.tag_name().name() == "numLit").then_some(r));
    let fmt = cache
        .and_then(|c| kid(c, "formatCode"))
        .and_then(|f| f.text())
        .map(str::to_owned);
    (
        pts(cache, |s| {
            s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
        }),
        fmt,
    )
}

/// Category labels from `c:cat` / `c:xVal`: text, or numbers and their format.
fn categories(n: Option<Node>) -> (Vec<String>, Vec<Option<f64>>, Option<String>) {
    let Some(n) = n else {
        return (Vec::new(), Vec::new(), None);
    };
    let Some(src) = n.children().find(|c| c.is_element()) else {
        return (Vec::new(), Vec::new(), None);
    };
    match src.tag_name().name() {
        "numRef" | "numLit" => {
            let (v, fmt) = num_cache(src);
            (Vec::new(), v, fmt)
        }
        "multiLvlStrRef" => {
            // The innermost level (the first `c:lvl`) labels the points.
            let lvl = src.descendants().find(|d| is(*d, NS_C, "lvl"));
            (
                pts(lvl, |s| Some(s.to_owned()))
                    .into_iter()
                    .map(Option::unwrap_or_default)
                    .collect(),
                Vec::new(),
                None,
            )
        }
        _ => (
            str_cache(src)
                .into_iter()
                .map(Option::unwrap_or_default)
                .collect(),
            Vec::new(),
            None,
        ),
    }
}

fn series(s: Node, cx: &ColorCtx) -> Series {
    let name = kid(s, "tx").and_then(|tx| {
        if let Some(r) = kid(tx, "strRef") {
            Some(
                str_cache(r)
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        } else {
            kid(tx, "v").and_then(|v| v.text()).map(str::to_owned)
        }
    });
    let scatter_x = kid(s, "xVal");
    let (cats, cat_nums, cat_fmt) = categories(kid(s, "cat").or(scatter_x));
    let vals_node = kid(s, "val").or_else(|| kid(s, "yVal"));
    let (vals, fmt) = vals_node
        .and_then(|v| v.children().find(|c| c.is_element()))
        .map(num_cache)
        .unwrap_or_default();
    let xs = scatter_x.map(|_| {
        if cat_nums.is_empty() {
            // Text x values: points are numbered 1, 2, 3...
            (1..=vals.len()).map(|i| Some(i as f64)).collect()
        } else {
            cat_nums.clone()
        }
    });
    Series {
        idx: val(s, "idx").and_then(|v| v.parse().ok()).unwrap_or(0),
        order: val(s, "order").and_then(|v| v.parse().ok()).unwrap_or(0),
        name,
        cats,
        cat_fmt,
        cat_nums,
        vals,
        xs,
        fmt,
        look: Look::parse(Some(s), cx),
        marker: Marker::parse(Some(s), cx),
        points: kids(s, "dPt")
            .map(|d| Point {
                idx: val(d, "idx").and_then(|v| v.parse().ok()).unwrap_or(0),
                look: Look::parse(Some(d), cx),
                explosion: val_f(d, "explosion").map(|e| e as f32),
                marker: Marker::parse(Some(d), cx),
            })
            .collect(),
        labels: kid(s, "dLbls").map(|d| Labels::parse(d, cx)),
        smooth: flag(s, "smooth").unwrap_or(false),
        explosion: val_f(s, "explosion").unwrap_or(0.0) as f32,
        invert_if_negative: flag(s, "invertIfNegative").unwrap_or(false),
    }
}

fn group(g: Node, cx: &ColorCtx) -> Option<Group> {
    let grouping = val(g, "grouping");
    let kind = match g.tag_name().name() {
        "barChart" => Kind::Bar {
            horizontal: val(g, "barDir") == Some("bar"),
            grouping: Grouping::parse(grouping, Grouping::Clustered),
            gap: val_f(g, "gapWidth").unwrap_or(150.0).clamp(0.0, 500.0) as f32,
            overlap: val_f(g, "overlap")
                .unwrap_or(match Grouping::parse(grouping, Grouping::Clustered) {
                    Grouping::Stacked | Grouping::PercentStacked => 100.0,
                    _ => 0.0,
                })
                .clamp(-100.0, 100.0) as f32,
        },
        "lineChart" => Kind::Line {
            grouping: Grouping::parse(grouping, Grouping::Standard),
        },
        "areaChart" => Kind::Area {
            grouping: Grouping::parse(grouping, Grouping::Standard),
        },
        "pieChart" => Kind::Pie {
            first_angle: val_f(g, "firstSliceAng").unwrap_or(0.0) as f32,
            hole: None,
        },
        "doughnutChart" => Kind::Pie {
            first_angle: val_f(g, "firstSliceAng").unwrap_or(0.0) as f32,
            hole: Some(val_f(g, "holeSize").unwrap_or(50.0).clamp(1.0, 90.0) as f32),
        },
        "scatterChart" => Kind::Scatter {
            style: val(g, "scatterStyle").unwrap_or("marker").to_owned(),
        },
        _ => return None,
    };
    let mut series: Vec<Series> = kids(g, "ser").map(|s| series(s, cx)).collect();
    series.sort_by_key(|s| s.order);
    Some(Group {
        markers: flag(g, "marker").unwrap_or(true),
        kind,
        vary_colors: flag(g, "varyColors").unwrap_or(false),
        series,
        ax_ids: kids(g, "axId")
            .filter_map(|a| a.attribute("val")?.parse().ok())
            .collect(),
        labels: kid(g, "dLbls").map(|d| Labels::parse(d, cx)),
    })
}

impl Chart {
    /// Parse a chart part. `None` when it isn't a chart.
    pub fn parse(xml: &str, cx: &ColorCtx) -> Option<Chart> {
        let doc = roxmltree::Document::parse(xml).ok()?;
        let space = doc.root_element();
        if !is(space, NS_C, "chartSpace") {
            return None;
        }
        let chart = kid(space, "chart")?;
        let plot = kid(chart, "plotArea")?;
        let mut groups = Vec::new();
        let mut unsupported = Vec::new();
        let mut axes = Vec::new();
        for n in plot.children().filter(|c| c.is_element()) {
            let name = n.tag_name().name();
            if name.ends_with("Chart") {
                match group(n, cx) {
                    Some(g) => groups.push(g),
                    None => unsupported.push(name.to_owned()),
                }
            } else if name.ends_with("Ax") {
                axes.extend(Axis::parse(n, cx));
            }
        }
        // Content inside `mc:AlternateContent` (`c14:style`) is ignored;
        // `c:style` is the fallback every writer includes.
        let style = val(space, "style")
            .and_then(|s| s.parse().ok())
            .unwrap_or(2);
        let legend = kid(chart, "legend").map(|l| Legend {
            pos: match val(l, "legendPos") {
                Some("l") => LegendPos::L,
                Some("t") => LegendPos::T,
                Some("b") => LegendPos::B,
                Some("tr") => LegendPos::Tr,
                _ => LegendPos::R,
            },
            overlay: flag(l, "overlay").unwrap_or(false),
            font: Font::of_txpr(Some(l), cx),
            look: Look::parse(Some(l), cx),
            deleted: kids(l, "legendEntry")
                .filter(|e| flag(*e, "delete").unwrap_or(false))
                .filter_map(|e| val(e, "idx")?.parse().ok())
                .collect(),
            layout: manual_layout(Some(l)),
        });
        Some(Chart {
            title: kid(chart, "title").map(|t| Title::parse(t, cx)),
            auto_title_deleted: flag(chart, "autoTitleDeleted").unwrap_or(false),
            legend,
            groups,
            axes,
            plot_look: Look::parse(Some(plot), cx),
            plot_layout: manual_layout(Some(plot)),
            space_look: Look::parse(Some(space), cx),
            font: Font::of_txpr(Some(space), cx),
            rounded: flag(space, "roundedCorners").unwrap_or(false),
            style,
            unsupported,
        })
    }

    /// Whether Kantor can draw this chart.
    pub fn drawable(&self) -> bool {
        !self.groups.is_empty() && self.unsupported.is_empty()
    }

    pub fn axis(&self, id: i64) -> Option<&Axis> {
        self.axes.iter().find(|a| a.id == id)
    }

    /// Every series of every group, in plot order.
    pub fn all_series(&self) -> impl Iterator<Item = (&Group, &Series)> {
        self.groups
            .iter()
            .flat_map(|g| g.series.iter().map(move |s| (g, s)))
    }
}
