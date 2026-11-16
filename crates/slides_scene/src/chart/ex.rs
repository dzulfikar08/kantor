//! The Office 2016 chart types (`cx:chartSpace` parts, "chartex"):
//! waterfall, funnel, treemap, sunburst, histogram, Pareto and box &
//! whisker. Region maps aren't drawn (they need map data).
//!
//! Parsing keeps the data as the part caches it (`cx:chartData`), so a
//! spreadsheet can put live cell values in its place, as it does for
//! ChartML. [`expand`] then works out what the chart type shows from the
//! data (bins, running totals, quartiles) right before layout: waterfall,
//! histogram and Pareto become ordinary bar and line groups; funnel,
//! treemap, sunburst and box & whisker keep their own [`Kind::Ex`] group
//! and are drawn by the layout's own code for them.

use std::borrow::Cow;

use roxmltree::Node;
use slides_engine::Fill;
use slides_engine::display_text::tr;
use slides_engine::theme::ColorCtx;

use super::model::*;

pub const NS_CX: &str = "http://schemas.microsoft.com/office/drawing/2014/chartex";

fn is(n: Node, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some(NS_CX)
}

fn kid<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| is(*c, name))
}

fn kids<'a, 'i>(n: Node<'a, 'i>, name: &'static str) -> impl Iterator<Item = Node<'a, 'i>> {
    n.children().filter(move |c| is(*c, name))
}

fn attr_f(n: Node, k: &str) -> Option<f64> {
    n.attribute(k)?
        .trim()
        .parse()
        .ok()
        .filter(|v: &f64| v.is_finite())
}

/// A number written as the element's text (the schema) or a `val`
/// attribute (some writers).
fn text_or_val<'a>(n: Node<'a, '_>) -> Option<&'a str> {
    n.text()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .or_else(|| n.attribute("val"))
}

fn attr_flag(n: Option<Node>, k: &str) -> Option<bool> {
    n?.attribute(k).map(|v| matches!(v, "1" | "true"))
}

/// What a chartex series' layout shows.
#[derive(Clone, Debug, PartialEq)]
pub enum ExKind {
    /// Floating bars from a running total; `subtotals` are points drawn
    /// from zero (their value is the total so far).
    Waterfall {
        subtotals: Vec<usize>,
        connectors: bool,
    },
    Funnel,
    Treemap {
        /// `parentLabelLayout`: `overlapping` (default), `banner`, `none`.
        parent_labels: String,
    },
    Sunburst,
    /// Bins of the values (or sums per category), as columns; with a
    /// Pareto line of their cumulative share when `pareto`.
    Histogram {
        bins: Binning,
        pareto: bool,
    },
    BoxWhisker {
        mean_line: bool,
        mean_marker: bool,
        /// Inner points (`nonoutliers`) and outliers shown as dots.
        inner: bool,
        outliers: bool,
        /// `quartileMethod="exclusive"` (default) or inclusive.
        exclusive: bool,
    },
}

/// How a histogram makes its bins.
#[derive(Clone, Debug, PartialEq)]
pub struct Binning {
    /// `cx:aggregation`: one bin per category.
    pub by_category: bool,
    pub count: Option<usize>,
    pub size: Option<f64>,
    /// Values at or below / above go to one end bin.
    pub underflow: Option<f64>,
    pub overflow: Option<f64>,
    /// `intervalClosed="r"` (default): bins are (a, b]; else [a, b).
    pub closed_right: bool,
}

/// A chartex plot: its type, the box & whisker statistics once worked out
/// ([`expand`]) and the category gap.
#[derive(Clone, Debug, PartialEq)]
pub struct ExPlot {
    pub kind: ExKind,
    /// Gap between categories, % of a bar's width.
    pub gap: f32,
    /// Box & whisker: per series, per category of [`ExPlot::cats`].
    pub boxes: Vec<Vec<Option<BoxStats>>>,
    pub cats: Vec<String>,
}

/// One box of a box & whisker chart.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxStats {
    pub low: f64,
    pub q1: f64,
    pub median: f64,
    pub q3: f64,
    pub high: f64,
    pub mean: f64,
    /// Points outside the whiskers, and the others.
    pub outliers: Vec<f64>,
    pub inner: Vec<f64>,
}

/// A data block (`cx:data`): category levels (innermost first), values,
/// their format and the cells they come from.
#[derive(Default)]
struct Data {
    levels: Vec<Vec<String>>,
    cat_nums: Vec<Option<f64>>,
    vals: Vec<Option<f64>>,
    fmt: Option<String>,
    cat_ref: Option<String>,
    val_ref: Option<String>,
}

fn lvl_pts(lvl: Node) -> Vec<Option<String>> {
    let count = lvl
        .attribute("ptCount")
        .and_then(|c| c.parse::<usize>().ok())
        .unwrap_or(0)
        .min(100_000);
    let mut out: Vec<Option<String>> = vec![None; count];
    for p in kids(lvl, "pt") {
        let Some(i) = p.attribute("idx").and_then(|i| i.parse::<usize>().ok()) else {
            continue;
        };
        if i >= 100_000 {
            continue;
        }
        if out.len() <= i {
            out.resize(i + 1, None);
        }
        out[i] = Some(p.text().unwrap_or_default().to_owned());
    }
    out
}

fn formula(dim: Node) -> Option<String> {
    kid(dim, "f")
        .and_then(|f| f.text())
        .map(|f| f.trim().to_owned())
        .filter(|f| !f.is_empty())
}

fn data(d: Node) -> Data {
    let mut out = Data::default();
    for dim in d.children().filter(|c| c.is_element()) {
        let ty = dim.attribute("type").unwrap_or("");
        match dim.tag_name().name() {
            "strDim" if ty == "cat" || ty == "colorStr" || ty.is_empty() => {
                out.levels = kids(dim, "lvl")
                    .map(|l| {
                        lvl_pts(l)
                            .into_iter()
                            .map(Option::unwrap_or_default)
                            .collect()
                    })
                    .collect();
                out.cat_ref = formula(dim);
            }
            "numDim" if ty == "cat" || ty == "x" => {
                if let Some(l) = kid(dim, "lvl") {
                    out.cat_nums = lvl_pts(l)
                        .into_iter()
                        .map(|v| v.and_then(|v| v.trim().parse().ok()))
                        .collect();
                }
                out.cat_ref = formula(dim);
            }
            "numDim" => {
                // `val` and `size` (treemap, sunburst); the first one wins.
                if out.val_ref.is_some() || !out.vals.is_empty() {
                    continue;
                }
                if let Some(l) = kid(dim, "lvl") {
                    out.vals = lvl_pts(l)
                        .into_iter()
                        .map(|v| {
                            v.and_then(|v| v.trim().parse().ok())
                                .filter(|v: &f64| v.is_finite())
                        })
                        .collect();
                    out.fmt = l
                        .attribute("formatCode")
                        .filter(|f| !f.is_empty())
                        .map(str::to_owned);
                }
                out.val_ref = formula(dim);
            }
            _ => {}
        }
    }
    out
}

/// Text of a `cx:tx` (`cx:txData/cx:v` or `cx:rich` paragraphs).
fn tx_text(tx: Node) -> Option<Vec<String>> {
    if let Some(rich) = kid(tx, "rich") {
        return Some(
            rich.children()
                .filter(|c| c.is_element() && c.tag_name().name() == "p")
                .map(|p| {
                    p.descendants()
                        .filter(|d| d.is_element() && d.tag_name().name() == "t")
                        .filter_map(|d| d.text())
                        .collect::<String>()
                })
                .collect(),
        );
    }
    kid(tx, "txData")
        .and_then(|d| kid(d, "v"))
        .and_then(|v| v.text())
        .map(|t| vec![t.to_owned()])
}

fn font_of(n: Node, cx: &ColorCtx) -> Font {
    let mut f = kid(n, "txPr")
        .map(|t| Font::of_body(t, cx))
        .unwrap_or_default();
    if let Some(rich) = kid(n, "tx").and_then(|t| kid(t, "rich")) {
        f = Font::of_body(rich, cx).over(&f);
        if let Some(r) = rich
            .descendants()
            .find(|d| d.is_element() && d.tag_name().name() == "rPr")
        {
            f = Font::from_rpr(r, cx).over(&f);
        }
    }
    f
}

fn look(n: Node, cx: &ColorCtx) -> Look {
    kid(n, "spPr")
        .map(|sp| Look::of_sppr(sp, cx))
        .unwrap_or_default()
}

fn title(t: Node, cx: &ColorCtx) -> Title {
    Title {
        text: kid(t, "tx").and_then(tx_text),
        font: font_of(t, cx),
        look: look(t, cx),
        overlay: attr_flag(Some(t), "overlay").unwrap_or(false),
    }
}

fn labels(n: Node, cx: &ColorCtx) -> Labels {
    let vis = kid(n, "visibility");
    let on = |k: &str, d: bool| attr_flag(vis, k).unwrap_or(d);
    Labels {
        delete: false,
        show_val: on("value", vis.is_none()),
        show_cat: on("categoryName", false),
        show_ser: on("seriesName", false),
        show_percent: false,
        pos: n.attribute("pos").map(str::to_owned),
        num_fmt: kid(n, "numFmt")
            .filter(|f| !matches!(f.attribute("sourceLinked"), Some("1" | "true")))
            .and_then(|f| f.attribute("formatCode"))
            .map(str::to_owned),
        separator: kid(n, "separator")
            .and_then(|s| s.text())
            .map(str::to_owned),
        font: kid(n, "txPr")
            .map(|t| Font::of_body(t, cx))
            .unwrap_or_default(),
        offset: None,
        points: kids(n, "dataLabel")
            .filter_map(|d| {
                let idx = d.attribute("idx")?.parse().ok()?;
                Some((idx, labels(d, cx)))
            })
            .chain(kids(n, "dataLabelHidden").filter_map(|d| {
                let idx = d.attribute("idx")?.parse().ok()?;
                Some((
                    idx,
                    Labels {
                        delete: true,
                        ..Labels::default()
                    },
                ))
            }))
            .collect(),
    }
}

fn axis(n: Node, cx: &ColorCtx) -> Option<(Axis, Option<f32>)> {
    let id: i64 = n.attribute("id")?.parse().ok()?;
    let mut a = Axis::plain(id, AxisKind::Cat, AxPos::B, 0);
    let mut gap = None;
    if let Some(s) = kid(n, "catScaling") {
        // A fraction of a bar's width (or "auto").
        gap = attr_f(s, "gapWidth").map(|g| (g * 100.0) as f32);
    } else if let Some(s) = kid(n, "valScaling") {
        a.kind = AxisKind::Val;
        a.pos = AxPos::L;
        a.min = attr_f(s, "min");
        a.max = attr_f(s, "max");
        a.major_unit = attr_f(s, "majorUnit").filter(|u| *u > 0.0);
    }
    a.delete = attr_flag(Some(n), "hidden").unwrap_or(false);
    a.major_grid = kid(n, "majorGridlines").map(|g| {
        let l = look(g, cx);
        if l.line.is_none() { Look::default() } else { l }
    });
    a.minor_grid = kid(n, "minorGridlines").map(|g| look(g, cx));
    if kid(n, "tickLabels").is_none() {
        a.tick_lbl_pos = "none".into();
    }
    a.major_tick = kid(n, "majorTickMarks")
        .and_then(|t| t.attribute("type"))
        .unwrap_or("none")
        .to_owned();
    a.num_fmt = kid(n, "numFmt").and_then(|f| {
        Some((
            f.attribute("formatCode")?.to_owned(),
            matches!(f.attribute("sourceLinked"), Some("1" | "true")),
        ))
    });
    a.font = font_of(n, cx);
    a.look = look(n, cx);
    a.title = kid(n, "title").map(|t| title(t, cx));
    Some((a, gap))
}

/// A `cx:chartSpace` part. `None` when it isn't one.
pub(crate) fn parse(space: Node, cx: &ColorCtx) -> Option<Chart> {
    let chart = kid(space, "chart")?;
    let plot = kid(chart, "plotArea")?;
    let region = kid(plot, "plotAreaRegion")?;
    let blocks: Vec<(String, Node)> = kid(space, "chartData")
        .into_iter()
        .flat_map(|d| kids(d, "data"))
        .filter_map(|d| Some((d.attribute("id")?.to_owned(), d)))
        .collect();
    let mut unsupported = Vec::new();
    let mut main: Option<(ExKind, Vec<Series>)> = None;
    let mut pareto = false;
    for (k, s) in kids(region, "series").enumerate() {
        let layout = s.attribute("layoutId").unwrap_or("");
        let lp = kid(s, "layoutPr");
        let vis = lp.and_then(|l| kid(l, "visibility"));
        let kind = match layout {
            "waterfall" => ExKind::Waterfall {
                subtotals: lp
                    .and_then(|l| kid(l, "subtotals"))
                    .into_iter()
                    .flat_map(|t| kids(t, "idx"))
                    .filter_map(|i| i.attribute("val")?.parse().ok())
                    .collect(),
                connectors: attr_flag(vis, "connectorLines").unwrap_or(true),
            },
            "funnel" => ExKind::Funnel,
            "treemap" => ExKind::Treemap {
                parent_labels: lp
                    .and_then(|l| kid(l, "parentLabelLayout"))
                    .and_then(|p| p.attribute("val"))
                    .unwrap_or("overlapping")
                    .to_owned(),
            },
            "sunburst" => ExKind::Sunburst,
            "clusteredColumn" => {
                let b = lp.and_then(|l| kid(l, "binning"));
                let bound = |k: &str| b.and_then(|b| attr_f(b, k));
                ExKind::Histogram {
                    bins: Binning {
                        by_category: lp.and_then(|l| kid(l, "aggregation")).is_some()
                            || b.is_none(),
                        count: b
                            .and_then(|b| kid(b, "binCount"))
                            .and_then(|c| text_or_val(c)?.parse().ok())
                            .filter(|c: &usize| *c > 0),
                        size: b
                            .and_then(|b| kid(b, "binSize"))
                            .and_then(|c| text_or_val(c)?.parse().ok())
                            .filter(|s| *s > 0.0),
                        underflow: bound("underflow"),
                        overflow: bound("overflow"),
                        closed_right: b.and_then(|b| b.attribute("intervalClosed")) != Some("l"),
                    },
                    pareto: false,
                }
            }
            "paretoLine" => {
                pareto = true;
                continue;
            }
            "boxWhisker" => ExKind::BoxWhisker {
                mean_line: attr_flag(vis, "meanLine").unwrap_or(false),
                mean_marker: attr_flag(vis, "meanMarker").unwrap_or(true),
                inner: attr_flag(vis, "nonoutliers").unwrap_or(false),
                outliers: attr_flag(vis, "outliers").unwrap_or(true),
                exclusive: lp
                    .and_then(|l| kid(l, "statistics"))
                    .and_then(|s| s.attribute("quartileMethod"))
                    != Some("inclusive"),
            },
            other => {
                unsupported.push(if other.is_empty() {
                    "chartex".to_owned()
                } else {
                    other.to_owned()
                });
                continue;
            }
        };
        let id = kid(s, "dataId").and_then(|d| d.attribute("val"));
        let d = id
            .and_then(|id| blocks.iter().find(|(b, _)| b == id))
            .map(|(_, n)| data(*n))
            .unwrap_or_default();
        let mut levels = d.levels;
        let cats = if levels.is_empty() {
            Vec::new()
        } else {
            levels.remove(0)
        };
        let name = kid(s, "tx").and_then(tx_text).map(|t| t.join(" "));
        let series = Series {
            idx: k,
            order: k,
            name,
            cats,
            cat_nums: d.cat_nums,
            vals: d.vals,
            fmt: d.fmt,
            look: look(s, cx),
            points: kids(s, "dataPt")
                .map(|p| Point {
                    idx: p.attribute("idx").and_then(|v| v.parse().ok()).unwrap_or(0),
                    look: look(p, cx),
                    explosion: None,
                    marker: None,
                })
                .collect(),
            labels: kid(s, "dataLabels").map(|l| labels(l, cx)),
            refs: SeriesRefs {
                name: kid(s, "tx")
                    .and_then(|t| kid(t, "txData"))
                    .and_then(|d| kid(d, "f"))
                    .and_then(|f| f.text())
                    .map(|f| f.trim().to_owned()),
                cat: d.cat_ref,
                val: d.val_ref,
                size: None,
            },
            levels,
            ..Series::default()
        };
        match &mut main {
            None => main = Some((kind, vec![series])),
            Some((k0, list)) if std::mem::discriminant(k0) == std::mem::discriminant(&kind) => {
                list.push(series)
            }
            Some(_) => {}
        }
    }
    let mut axes = Vec::new();
    let mut gap = None;
    for n in kids(plot, "axis") {
        if let Some((a, g)) = axis(n, cx) {
            gap = gap.or(g);
            axes.push(a);
        }
    }
    let mut groups = Vec::new();
    if let Some((mut kind, series)) = main {
        if let ExKind::Histogram { pareto: p, .. } = &mut kind {
            *p = pareto;
        }
        let default_gap = match kind {
            ExKind::Histogram { .. } => 0.0,
            ExKind::Funnel => 10.0,
            _ => 50.0,
        };
        groups.push(Group {
            kind: Kind::Ex(Box::new(ExPlot {
                kind,
                gap: gap.unwrap_or(default_gap).clamp(0.0, 500.0),
                boxes: Vec::new(),
                cats: Vec::new(),
            })),
            vary_colors: false,
            series,
            ax_ids: Vec::new(),
            labels: None,
            markers: false,
        });
    }
    let legend = kid(chart, "legend").map(|l| Legend {
        pos: match l.attribute("pos") {
            Some("l") => LegendPos::L,
            Some("t") => LegendPos::T,
            Some("b") => LegendPos::B,
            _ => LegendPos::R,
        },
        overlay: attr_flag(Some(l), "overlay").unwrap_or(false),
        font: font_of(l, cx),
        look: look(l, cx),
        deleted: Vec::new(),
        layout: None,
    });
    let ch_look = look(space, cx);
    Some(Chart {
        title: kid(chart, "title").map(|t| title(t, cx)),
        auto_title_deleted: kid(chart, "title").is_none(),
        legend,
        groups,
        axes,
        plot_look: look(plot, cx),
        plot_layout: None,
        space_look: ch_look,
        font: font_of(space, cx),
        rounded: false,
        style: 2,
        unsupported,
        legend_keys: None,
    })
}

// ── working out what the chart shows ───────────────────────────────────────

/// The chart with its chartex group worked out: waterfall, histogram and
/// Pareto as bar (and line) groups on their axes, box & whisker with its
/// statistics. Other charts as they are.
pub fn expand<'c>(chart: &'c Chart, d: &super::Defaults) -> Cow<'c, Chart> {
    let Some(Kind::Ex(plot)) = chart.groups.first().map(|g| &g.kind) else {
        return Cow::Borrowed(chart);
    };
    let mut out = chart.clone();
    let g = &mut out.groups[0];
    match &plot.kind {
        ExKind::Waterfall {
            subtotals,
            connectors,
        } => {
            let ax = axis_ids(&mut out.axes);
            let g = &mut out.groups[0];
            let gap = plot.gap;
            let Some(s) = g.series.first_mut() else {
                return Cow::Owned(out);
            };
            let mut run = 0.0;
            let mut ranges = Vec::new();
            for (i, v) in s.vals.iter().enumerate() {
                let Some(v) = *v else {
                    ranges.push(None);
                    continue;
                };
                if subtotals.contains(&i) {
                    ranges.push(Some((0.0, v)));
                    run = v;
                } else {
                    ranges.push(Some((run, run + v)));
                    run += v;
                }
            }
            let colors = [d.accents[0], d.accents[1], d.accents[2]];
            for (i, v) in s.vals.iter().enumerate() {
                let c = if subtotals.contains(&i) {
                    colors[2]
                } else if v.unwrap_or(0.0) < 0.0 {
                    colors[1]
                } else {
                    colors[0]
                };
                match s.points.iter_mut().find(|p| p.idx == i) {
                    Some(p) if p.look.fill.is_some() => {}
                    Some(p) => p.look.fill = Some(Fill::Solid(c, 255)),
                    None => s.points.push(Point {
                        idx: i,
                        look: Look {
                            fill: Some(Fill::Solid(c, 255)),
                            line: None,
                        },
                        explosion: None,
                        marker: None,
                    }),
                }
            }
            s.float = Some(super::model::Float {
                ranges,
                connectors: connectors.then(|| Look {
                    fill: None,
                    line: Some(Line {
                        fill: Some(Fill::Solid(slides_engine::Rgb([0x7F, 0x7F, 0x7F]), 255)),
                        width: Some(0.75),
                        style: Default::default(),
                    }),
                }),
            });
            g.kind = Kind::Bar {
                horizontal: false,
                grouping: Grouping::Clustered,
                gap,
                overlap: 0.0,
            };
            g.ax_ids = ax.to_vec();
            out.legend_keys = Some(vec![
                (
                    word("Waterfall: Increase", "Increase"),
                    Fill::Solid(colors[0], 255),
                ),
                (
                    word("Waterfall: Decrease", "Decrease"),
                    Fill::Solid(colors[1], 255),
                ),
                (tr("Total").to_owned(), Fill::Solid(colors[2], 255)),
            ]);
        }
        ExKind::Histogram { bins, pareto } => {
            let ax = axis_ids(&mut out.axes);
            let g = &mut out.groups[0];
            let gap = plot.gap;
            let Some(s) = g.series.first_mut() else {
                return Cow::Owned(out);
            };
            let (labels, counts) = histogram(s, bins);
            let mut order: Vec<usize> = (0..counts.len()).collect();
            if *pareto {
                // Most frequent first.
                order.sort_by(|a, b| counts[*b].total_cmp(&counts[*a]));
            }
            s.cats = order.iter().map(|&i| labels[i].clone()).collect();
            s.cat_nums.clear();
            s.vals = order.iter().map(|&i| Some(counts[i])).collect();
            s.fmt = None;
            s.refs = SeriesRefs::default();
            g.kind = Kind::Bar {
                horizontal: false,
                grouping: Grouping::Clustered,
                gap,
                overlap: 0.0,
            };
            g.ax_ids = ax.to_vec();
            if *pareto {
                let total: f64 = s.vals.iter().flatten().sum();
                let mut acc = 0.0;
                let cum: Vec<Option<f64>> = s
                    .vals
                    .iter()
                    .map(|v| {
                        acc += v.unwrap_or(0.0);
                        Some(if total > 0.0 { acc / total } else { 0.0 })
                    })
                    .collect();
                let line = Series {
                    idx: 1,
                    order: 1,
                    name: Some(tr("Cumulative %").to_owned()),
                    cats: s.cats.clone(),
                    vals: cum,
                    fmt: Some("0%".into()),
                    ..Series::default()
                };
                let (cat2, val2) = (90_001, 90_002);
                let mut c = Axis::plain(cat2, AxisKind::Cat, AxPos::B, val2);
                c.delete = true;
                let mut v = Axis::plain(val2, AxisKind::Val, AxPos::R, cat2);
                v.crosses = Some("max".into());
                v.min = Some(0.0);
                v.max = Some(1.0);
                v.num_fmt = Some(("0%".into(), false));
                v.major_tick = "none".into();
                out.axes.push(c);
                out.axes.push(v);
                out.groups.push(Group {
                    kind: Kind::Line {
                        grouping: Grouping::Standard,
                    },
                    vary_colors: false,
                    series: vec![line],
                    ax_ids: vec![cat2, val2],
                    labels: None,
                    markers: false,
                });
            }
        }
        ExKind::BoxWhisker { exclusive, .. } => {
            let ax = axis_ids(&mut out.axes);
            let g = &mut out.groups[0];
            let mut cats: Vec<String> = Vec::new();
            for s in &g.series {
                for i in 0..s.vals.len() {
                    let c = s.cats.get(i).cloned().unwrap_or_default();
                    if !cats.contains(&c) {
                        cats.push(c);
                    }
                }
            }
            let boxes = g
                .series
                .iter()
                .map(|s| {
                    cats.iter()
                        .map(|c| {
                            let vals: Vec<f64> = s
                                .vals
                                .iter()
                                .enumerate()
                                .filter(|(i, _)| s.cats.get(*i).map_or("", |x| x) == c)
                                .filter_map(|(_, v)| *v)
                                .collect();
                            box_stats(vals, *exclusive)
                        })
                        .collect()
                })
                .collect();
            if let Kind::Ex(p) = &mut g.kind {
                p.boxes = boxes;
                p.cats = cats;
            }
            g.ax_ids = ax.to_vec();
        }
        _ => {
            g.ax_ids.clear();
        }
    }
    Cow::Owned(out)
}

/// `key` in the interface language, `en` when it has no translation (the
/// key names its context: the same English word is translated
/// differently elsewhere).
fn word(key: &'static str, en: &str) -> String {
    let t = tr(key);
    if t == key {
        en.to_owned()
    } else {
        t.to_owned()
    }
}

/// The (category, value) axes of a chartex plot, made up when the part
/// has none, and pointed at each other.
fn axis_ids(axes: &mut Vec<Axis>) -> [i64; 2] {
    let cat = axes.iter().position(|a| a.kind == AxisKind::Cat);
    let val = axes.iter().position(|a| a.kind == AxisKind::Val);
    let cat = cat.unwrap_or_else(|| {
        let mut a = Axis::plain(80_001, AxisKind::Cat, AxPos::B, 0);
        a.delete = true;
        axes.push(a);
        axes.len() - 1
    });
    let val = val.unwrap_or_else(|| {
        let mut a = Axis::plain(80_002, AxisKind::Val, AxPos::L, 0);
        a.delete = true;
        axes.push(a);
        axes.len() - 1
    });
    let (c, v) = (axes[cat].id, axes[val].id);
    axes[cat].cross_ax = v;
    axes[val].cross_ax = c;
    // Office draws the category axis line, not the value axis'.
    if axes[cat].look.line.is_none() {
        axes[cat].look.line = Some(Line::default());
    }
    [c, v]
}

/// Bin labels and counts (sums per category when `by_category` and the
/// series has values).
pub fn histogram(s: &Series, b: &Binning) -> (Vec<String>, Vec<f64>) {
    let fmt = |v: f64| super::format::format_number(v, s.fmt.as_deref());
    if b.by_category {
        let mut labels: Vec<String> = Vec::new();
        let mut sums: Vec<f64> = Vec::new();
        let n = s.vals.len().max(s.cats.len());
        for i in 0..n {
            let c = s.cats.get(i).cloned().unwrap_or_default();
            let add = if s.vals.is_empty() {
                1.0
            } else {
                match s.vals.get(i).copied().flatten() {
                    Some(v) => v,
                    None => continue,
                }
            };
            match labels.iter().position(|l| *l == c) {
                Some(k) => sums[k] += add,
                None => {
                    labels.push(c);
                    sums.push(add);
                }
            }
        }
        return (labels, sums);
    }
    let vals: Vec<f64> = s.vals.iter().flatten().copied().collect();
    if vals.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let lo = vals.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = vals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let start = b.underflow.unwrap_or(lo);
    let end = b.overflow.unwrap_or(hi);
    let span = (end - start).max(0.0);
    let width = match (b.size, b.count) {
        (Some(w), _) => w,
        (None, Some(n)) => span / n as f64,
        _ => {
            // Scott's normal reference rule, as Excel's automatic bins.
            let n = vals.len() as f64;
            let mean = vals.iter().sum::<f64>() / n;
            let sd =
                (vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0).max(1.0)).sqrt();
            3.5 * sd / n.cbrt()
        }
    };
    let width = if width.is_finite() && width > 0.0 {
        width.max(span / 1000.0)
    } else {
        1.0
    };
    let nbins = ((span / width).ceil() as usize).clamp(1, 1000);
    let mut labels = Vec::new();
    let mut counts = Vec::new();
    let (open, close) = if b.closed_right {
        ("(", "]")
    } else {
        ("[", ")")
    };
    if let Some(u) = b.underflow {
        labels.push(format!("≤{}", fmt(u)));
        counts.push(0.0);
    }
    for k in 0..nbins {
        let a = start + width * k as f64;
        let z = a + width;
        // The first bin includes its low end (Excel's "[a, b]").
        let first = k == 0 && b.underflow.is_none() && b.closed_right;
        labels.push(if first {
            format!("[{}, {}]", fmt(a), fmt(z))
        } else {
            format!("{open}{}, {}{close}", fmt(a), fmt(z))
        });
        counts.push(0.0);
    }
    if let Some(o) = b.overflow {
        labels.push(format!(">{}", fmt(o)));
        counts.push(0.0);
    }
    let first_bin = usize::from(b.underflow.is_some());
    for v in vals {
        let k = if b.underflow.is_some_and(|u| v <= u) {
            0
        } else if b.overflow.is_some_and(|o| v > o) {
            counts.len() - 1
        } else {
            let f = (v - start) / width;
            let k = if b.closed_right {
                (f.ceil() as isize - 1).max(0)
            } else {
                f.floor() as isize
            };
            first_bin + (k.max(0) as usize).min(nbins - 1)
        };
        counts[k] += 1.0;
    }
    (labels, counts)
}

/// A quartile of sorted values, Excel's QUARTILE.EXC or QUARTILE.INC.
fn quartile(v: &[f64], q: f64, exclusive: bool) -> f64 {
    let n = v.len();
    if n == 1 {
        return v[0];
    }
    let pos = if exclusive {
        q * (n as f64 + 1.0) - 1.0
    } else {
        q * (n as f64 - 1.0)
    };
    let pos = pos.clamp(0.0, (n - 1) as f64);
    let i = pos.floor() as usize;
    let f = pos - i as f64;
    match v.get(i + 1) {
        Some(next) => v[i] + (next - v[i]) * f,
        None => v[i],
    }
}

/// The box of `vals`: quartiles, whiskers at the furthest points within
/// 1.5 × the interquartile range, and the points beyond.
pub fn box_stats(mut vals: Vec<f64>, exclusive: bool) -> Option<BoxStats> {
    if vals.is_empty() {
        return None;
    }
    vals.sort_by(f64::total_cmp);
    let q1 = quartile(&vals, 0.25, exclusive);
    let median = quartile(&vals, 0.5, exclusive);
    let q3 = quartile(&vals, 0.75, exclusive);
    let iqr = q3 - q1;
    let (lo_fence, hi_fence) = (q1 - 1.5 * iqr, q3 + 1.5 * iqr);
    let inside: Vec<f64> = vals
        .iter()
        .copied()
        .filter(|v| *v >= lo_fence && *v <= hi_fence)
        .collect();
    let low = inside.first().copied().unwrap_or(q1).min(q1);
    let high = inside.last().copied().unwrap_or(q3).max(q3);
    Some(BoxStats {
        low,
        q1,
        median,
        q3,
        high,
        mean: vals.iter().sum::<f64>() / vals.len() as f64,
        outliers: vals
            .iter()
            .copied()
            .filter(|v| *v < lo_fence || *v > hi_fence)
            .collect(),
        inner: inside,
    })
}

/// A node of a treemap or sunburst: its label, value (the sum of its
/// leaves) and children.
#[derive(Clone, Debug, PartialEq)]
pub struct Node2 {
    pub label: String,
    pub value: f64,
    /// The data point (leaves).
    pub point: Option<usize>,
    pub children: Vec<Node2>,
}

/// The hierarchy of a series' points: each point's path runs from its
/// outermost category level to the innermost non-empty one.
pub fn hierarchy(s: &Series) -> Vec<Node2> {
    let mut roots: Vec<Node2> = Vec::new();
    for (i, v) in s.vals.iter().enumerate() {
        let v = v.unwrap_or(0.0);
        if v <= 0.0 {
            continue;
        }
        let mut path: Vec<String> = s
            .levels
            .iter()
            .rev()
            .map(|l| l.get(i).cloned().unwrap_or_default())
            .collect();
        path.push(s.cats.get(i).cloned().unwrap_or_default());
        while path.len() > 1 && path.last().is_some_and(|p| p.is_empty()) {
            path.pop();
        }
        let mut level = &mut roots;
        let last = path.len() - 1;
        for (k, name) in path.into_iter().enumerate() {
            let at = match level
                .iter()
                .position(|n| n.label == name && n.point.is_none())
            {
                Some(p) if k < last => p,
                _ => {
                    level.push(Node2 {
                        label: name,
                        value: 0.0,
                        point: (k == last).then_some(i),
                        children: Vec::new(),
                    });
                    level.len() - 1
                }
            };
            level[at].value += v;
            level = &mut level[at].children;
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quartiles_follow_excel() {
        // QUARTILE.EXC({1..8}, 1) = 2.25, QUARTILE.INC = 2.75.
        let v: Vec<f64> = (1..=8).map(f64::from).collect();
        let e = box_stats(v.clone(), true).unwrap();
        assert_eq!((e.q1, e.median, e.q3), (2.25, 4.5, 6.75));
        let i = box_stats(v, false).unwrap();
        assert_eq!((i.q1, i.q3), (2.75, 6.25));
        // Inclusive quartiles 2 and 4: the whisker stops at 4, 50 is out.
        let o = box_stats(vec![1.0, 2.0, 3.0, 4.0, 50.0], false).unwrap();
        assert_eq!(o.outliers, vec![50.0]);
        assert_eq!(o.high, 4.0);
    }

    #[test]
    fn histogram_bins_count_into_right_closed_intervals() {
        let s = Series {
            vals: [1.0, 2.0, 2.5, 4.0, 6.0].map(Some).to_vec(),
            ..Series::default()
        };
        let b = Binning {
            by_category: false,
            count: None,
            size: Some(2.0),
            underflow: None,
            overflow: None,
            closed_right: true,
        };
        let (labels, counts) = histogram(&s, &b);
        assert_eq!(labels, ["[1, 3]", "(3, 5]", "(5, 7]"]);
        assert_eq!(counts, [3.0, 1.0, 1.0]);
    }

    #[test]
    fn hierarchy_groups_points_under_their_parents() {
        let s = Series {
            cats: vec!["a".into(), "b".into(), "".into()],
            levels: vec![vec!["X".into(), "X".into(), "Y".into()]],
            vals: [1.0, 2.0, 4.0].map(Some).to_vec(),
            ..Series::default()
        };
        let h = hierarchy(&s);
        assert_eq!(h.len(), 2);
        assert_eq!((h[0].label.as_str(), h[0].value), ("X", 3.0));
        assert_eq!(h[0].children.len(), 2);
        // A point whose inner level is blank is a leaf of the outer one.
        assert_eq!((h[1].point, h[1].children.len()), (Some(2), 0));
    }
}
