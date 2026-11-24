//! Changing a chart part in place: title, legend, axis titles, series
//! colours, chart type and series.
//!
//! Each change is a pass over the XML that rewrites only the elements it
//! owns (the `c:title`, the `c:legend`, one series' fill...) and copies
//! everything else byte for byte: extensions (`c:extLst`), elements Kantor
//! doesn't know, manual layouts, text formatting. Inserted elements go in
//! schema order, with the part's own namespace prefixes.
//!
//! Used for charts that came with a file (`crate::file_chart`) and for the
//! axis titles and colours of charts made in Kantor (`crate::chart_edit`).

use std::ops::Range;

use anyhow::{Result, anyhow, bail};
use roxmltree::{Document, Node};

use crate::chart_edit::{ChartType, LegendPos, SeriesType};
use crate::chart_options::{
    AxisOptions, AxisType, DataLabels, DisplayUnits, LabelFont, LabelPos, TimeUnit, num_text,
};

pub(crate) const NS_C: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";

/// A series colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SeriesColor {
    /// The chart style's colour for the series' position.
    #[default]
    Automatic,
    /// A solid RGB colour.
    Rgb([u8; 3]),
    /// A colour from the file Kantor doesn't name (a theme colour, a
    /// gradient...): kept unless changed.
    Custom,
}

// Children in schema order (CT_Chart, CT_Title, CT_Legend, the axes merged,
// the series of every type merged, CT_ShapeProperties, CT_LineProperties,
// CT_Marker).
const CHART_ORDER: &[&str] = &[
    "title",
    "autoTitleDeleted",
    "pivotFmts",
    "view3D",
    "floor",
    "sideWall",
    "backWall",
    "plotArea",
    "legend",
    "plotVisOnly",
    "dispBlanksAs",
    "showDLblsOverMax",
    "extLst",
];
const TITLE_ORDER: &[&str] = &["tx", "layout", "overlay", "spPr", "txPr", "extLst"];
const LEGEND_ORDER: &[&str] = &[
    "legendPos",
    "legendEntry",
    "layout",
    "overlay",
    "spPr",
    "txPr",
    "extLst",
];
const AX_ORDER: &[&str] = &[
    "axId",
    "scaling",
    "delete",
    "axPos",
    "majorGridlines",
    "minorGridlines",
    "title",
    "numFmt",
    "majorTickMark",
    "minorTickMark",
    "tickLblPos",
    "spPr",
    "txPr",
    "crossAx",
    "crosses",
    "crossesAt",
    "crossBetween",
    "auto",
    "lblAlgn",
    "lblOffset",
    "baseTimeUnit",
    "majorUnit",
    "majorTimeUnit",
    "minorUnit",
    "minorTimeUnit",
    "tickLblSkip",
    "tickMarkSkip",
    "noMultiLvlLbl",
    "dispUnits",
    "extLst",
];
const CAT_AX: &[&str] = &[
    "axId",
    "scaling",
    "delete",
    "axPos",
    "majorGridlines",
    "minorGridlines",
    "title",
    "numFmt",
    "majorTickMark",
    "minorTickMark",
    "tickLblPos",
    "spPr",
    "txPr",
    "crossAx",
    "crosses",
    "crossesAt",
    "auto",
    "lblAlgn",
    "lblOffset",
    "tickLblSkip",
    "tickMarkSkip",
    "noMultiLvlLbl",
    "extLst",
];
const DATE_AX: &[&str] = &[
    "axId",
    "scaling",
    "delete",
    "axPos",
    "majorGridlines",
    "minorGridlines",
    "title",
    "numFmt",
    "majorTickMark",
    "minorTickMark",
    "tickLblPos",
    "spPr",
    "txPr",
    "crossAx",
    "crosses",
    "crossesAt",
    "auto",
    "lblOffset",
    "baseTimeUnit",
    "majorUnit",
    "majorTimeUnit",
    "minorUnit",
    "minorTimeUnit",
    "extLst",
];
const VAL_AX: &[&str] = &[
    "axId",
    "scaling",
    "delete",
    "axPos",
    "majorGridlines",
    "minorGridlines",
    "title",
    "numFmt",
    "majorTickMark",
    "minorTickMark",
    "tickLblPos",
    "spPr",
    "txPr",
    "crossAx",
    "crosses",
    "crossesAt",
    "crossBetween",
    "majorUnit",
    "minorUnit",
    "dispUnits",
    "extLst",
];
const SER_ORDER: &[&str] = &[
    "idx",
    "order",
    "tx",
    "spPr",
    "invertIfNegative",
    "pictureOptions",
    "marker",
    "explosion",
    "dPt",
    "dLbls",
    "trendline",
    "errBars",
    "cat",
    "val",
    "xVal",
    "yVal",
    "shape",
    "smooth",
    "bubbleSize",
    "bubble3D",
    "extLst",
];
const SPPR_ORDER: &[&str] = &[
    "xfrm",
    "custGeom",
    "prstGeom",
    "noFill",
    "solidFill",
    "gradFill",
    "blipFill",
    "pattFill",
    "grpFill",
    "ln",
    "effectLst",
    "effectDag",
    "scene3d",
    "sp3d",
    "extLst",
];
const LN_ORDER: &[&str] = &[
    "noFill",
    "solidFill",
    "gradFill",
    "pattFill",
    "prstDash",
    "custDash",
    "round",
    "bevel",
    "miter",
    "headEnd",
    "tailEnd",
    "extLst",
];
const MARKER_ORDER: &[&str] = &["symbol", "size", "spPr", "extLst"];
const FILLS: &[&str] = &[
    "noFill",
    "solidFill",
    "gradFill",
    "blipFill",
    "pattFill",
    "grpFill",
];

// ------------------------------------------------------------------ helpers

type Edit = (Range<usize>, String);

fn is(n: Node, ns: &str, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some(ns)
}

fn kid<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| is(*c, NS_C, name))
}

fn kids<'a, 'i>(n: Node<'a, 'i>, name: &'static str) -> impl Iterator<Item = Node<'a, 'i>> {
    n.children().filter(move |c| is(*c, NS_C, name))
}

fn akid<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| is(*c, NS_A, name))
}

fn val<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
    kid(n, name)?.attribute("val")
}

fn elements<'a, 'i>(n: Node<'a, 'i>) -> impl Iterator<Item = Node<'a, 'i>> {
    n.children().filter(|c| c.is_element())
}

pub(crate) fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Apply `edits` (insertions at one place keep their order).
fn splice(src: &str, edits: Vec<Edit>) -> String {
    let mut edits: Vec<(usize, Edit)> = edits.into_iter().enumerate().collect();
    edits.sort_by_key(|(i, (r, _))| std::cmp::Reverse((r.start, r.end, *i)));
    let mut out = src.to_owned();
    for (_, (r, t)) in edits {
        out.replace_range(r, &t);
    }
    out
}

fn raw<'s>(src: &'s str, n: Node) -> &'s str {
    &src[n.range()]
}

/// The qualified name an element is written with (`c:ser`).
fn qname<'s>(src: &'s str, n: Node) -> &'s str {
    let s = &src[n.range().start + 1..];
    let end = s
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(s.len());
    &s[..end]
}

/// The `prefix:` of an element (empty for the default namespace).
fn prefix_of(src: &str, n: Node) -> String {
    match qname(src, n).split_once(':') {
        Some((p, _)) => format!("{p}:"),
        None => String::new(),
    }
}

/// Just past the `>` of `n`'s start tag.
fn open_end(src: &str, n: Node) -> usize {
    let b = src.as_bytes();
    let mut quote: Option<u8> = None;
    for i in n.range() {
        let c = b[i];
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == b'"' || c == b'\'' => quote = Some(c),
            None if c == b'>' => return i + 1,
            None => {}
        }
    }
    n.range().end
}

fn self_closing(src: &str, n: Node) -> bool {
    src[..open_end(src, n)].ends_with("/>")
}

/// The DrawingML prefix in scope at `n`, and a declaration to add to a new
/// element when the part doesn't bind it there.
fn a_ns(n: Node) -> (String, String) {
    match n.lookup_prefix(NS_A) {
        Some("") => (String::new(), String::new()),
        Some(p) => (format!("{p}:"), String::new()),
        None => ("a:".into(), format!(" xmlns:a=\"{NS_A}\"")),
    }
}

/// `text` (written with `c:` and `a:`) with the part's prefixes.
fn prefixed(text: &str, p: &str, a: &str) -> String {
    let mut t = text.to_owned();
    if p != "c:" {
        t = t
            .replace("<c:", &format!("<{p}"))
            .replace("</c:", &format!("</{p}"));
    }
    if a != "a:" {
        t = t
            .replace("<a:", &format!("<{a}"))
            .replace("</a:", &format!("</{a}"));
    }
    t
}

/// Put `decl` (` xmlns:a="..."`) into the first start tag of `text`.
fn declare(text: &str, decl: &str) -> String {
    if decl.is_empty() {
        return text.to_owned();
    }
    let end = text
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(text.len());
    format!("{}{decl}{}", &text[..end], &text[end..])
}

/// Insert `text` into `parent`: before `before`, else at its end.
fn insert_at(src: &str, parent: Node, before: Option<Node>, text: &str) -> Edit {
    if let Some(b) = before {
        let at = b.range().start;
        return (at..at, text.to_owned());
    }
    if self_closing(src, parent) {
        let open = src[parent.range().start..open_end(src, parent)]
            .trim_end_matches("/>")
            .trim_end();
        let q = qname(src, parent);
        return (parent.range(), format!("{open}>{text}</{q}>"));
    }
    let r = parent.range();
    let at = src[r.clone()].rfind("</").map_or(r.end, |i| r.start + i);
    (at..at, text.to_owned())
}

/// Insert a `name` element into `parent` where `order` puts it.
fn insert_ordered(src: &str, parent: Node, name: &str, order: &[&str], text: &str) -> Edit {
    let pos = |n: &str| order.iter().position(|o| *o == n);
    let before = pos(name)
        .and_then(|i| elements(parent).find(|c| pos(c.tag_name().name()).is_some_and(|j| j > i)));
    insert_at(src, parent, before, text)
}

/// Set `<name val="v"/>` under `parent` (replaced, or inserted in order).
fn set_val(src: &str, parent: Node, name: &str, order: &[&str], v: &str) -> Edit {
    match kid(parent, name) {
        Some(n) => (
            n.range(),
            format!("<{}{name} val=\"{}\"/>", prefix_of(src, n), esc(v)),
        ),
        None => {
            let p = prefix_of(src, parent);
            insert_ordered(
                src,
                parent,
                name,
                order,
                &format!("<{p}{name} val=\"{}\"/>", esc(v)),
            )
        }
    }
}

/// `n`'s markup with its descendants matching `drop` left out.
fn raw_without(src: &str, n: Node, drop: &dyn Fn(Node) -> bool) -> String {
    let cut: Vec<Range<usize>> = n
        .descendants()
        .skip(1)
        .filter(|d| d.is_element() && drop(*d))
        .map(|d| d.range())
        .collect();
    let r = n.range();
    let mut out = String::new();
    let mut at = r.start;
    for c in cut {
        if c.start < at {
            continue; // inside one already cut
        }
        out.push_str(&src[at..c.start]);
        at = c.end;
    }
    out.push_str(&src[at..r.end]);
    out
}

/// `n`'s markup with its element name changed to `name` (same prefix).
fn renamed(src: &str, n: Node, name: &str) -> String {
    let text = raw(src, n);
    let q = qname(src, n);
    let p = prefix_of(src, n);
    let new_q = format!("{p}{name}");
    let mut out = format!("<{new_q}{}", &text[1 + q.len()..]);
    let close = format!("</{q}>");
    if out.ends_with(&close) {
        out.truncate(out.len() - close.len());
        out.push_str(&format!("</{new_q}>"));
    }
    out
}

/// Rebuild element `n` as `name` with its children in `order`: `over`
/// gives a child's new markup (`Some(None)`: leave it out), others are
/// copied. Children `order` doesn't list are left out when `known` lists
/// them (they belong to another type), else copied before `extLst`.
fn rebuild(
    src: &str,
    n: Node,
    name: &str,
    order: &[&str],
    known: &[&str],
    over: &mut dyn FnMut(&str) -> Option<Option<String>>,
) -> String {
    let q = qname(src, n);
    let p = prefix_of(src, n);
    let open = &src[n.range().start..open_end(src, n)];
    let attrs = open[1 + q.len()..]
        .trim_end_matches('>')
        .trim_end_matches('/');
    let mut out = format!("<{p}{name}{attrs}>");
    for slot in order {
        if *slot == "extLst" {
            for c in elements(n) {
                let cn = c.tag_name().name();
                if !order.contains(&cn) && !known.contains(&cn) {
                    out.push_str(raw(src, c));
                }
            }
        }
        match over(slot) {
            Some(Some(t)) => out.push_str(&t),
            Some(None) => {}
            None => {
                for c in elements(n).filter(|c| c.tag_name().name() == *slot) {
                    out.push_str(raw(src, c));
                }
            }
        }
    }
    out.push_str(&format!("</{p}{name}>"));
    out
}

fn hex(c: [u8; 3]) -> String {
    format!("{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

/// `xml` with the text of every `c:f` replaced by `f(text)`.
pub(crate) fn map_formulas(xml: &str, f: impl Fn(&str) -> String) -> String {
    let Ok(doc) = Document::parse(xml) else {
        return xml.to_owned();
    };
    let edits: Vec<Edit> = doc
        .descendants()
        .filter(|n| is(*n, NS_C, "f"))
        .filter_map(|n| {
            let t = n.first_child().filter(|t| t.is_text())?;
            let old = t.text()?;
            let new = f(old);
            (new != old).then(|| (t.range(), esc(&new)))
        })
        .collect();
    if edits.is_empty() {
        return xml.to_owned();
    }
    splice(xml, edits)
}

/// The first `c:f` text in a fragment written with `c:`.
pub(crate) fn first_formula(fragment: &str) -> Option<String> {
    let a = fragment.find("<c:f>")? + 5;
    let b = fragment[a..].find("</c:f>")? + a;
    Some(
        fragment[a..b]
            .replace("&apos;", "'")
            .replace("&quot;", "\"")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&"),
    )
}

// ------------------------------------------------------------------ reading

/// The plot types (`c:barChart`...) of a plot area.
fn groups<'a, 'i>(plot: Node<'a, 'i>) -> Vec<Node<'a, 'i>> {
    elements(plot)
        .filter(|c| {
            c.tag_name().namespace() == Some(NS_C) && c.tag_name().name().ends_with("Chart")
        })
        .collect()
}

fn axes<'a, 'i>(plot: Node<'a, 'i>) -> Vec<Node<'a, 'i>> {
    elements(plot)
        .filter(|c| c.tag_name().namespace() == Some(NS_C) && c.tag_name().name().ends_with("Ax"))
        .collect()
}

fn plot_area<'a, 'i>(doc: &'a Document<'i>) -> Result<(Node<'a, 'i>, Node<'a, 'i>)> {
    let root = doc.root_element();
    if !is(root, NS_C, "chartSpace") {
        bail!("not a chart part");
    }
    let chart = kid(root, "chart").ok_or_else(|| anyhow!("no c:chart"))?;
    let plot = kid(chart, "plotArea").ok_or_else(|| anyhow!("no c:plotArea"))?;
    Ok((chart, plot))
}

/// The types Kantor converts between.
const CONVERTIBLE: &[&str] = &[
    "barChart",
    "lineChart",
    "areaChart",
    "pieChart",
    "doughnutChart",
    "scatterChart",
];

/// What a chart part shows, as the Chart panel edits it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Style {
    /// `Other` when it isn't one of Insert Chart's types.
    pub kind: ChartType,
    /// What to call it (for `Other`: "Combo", "3-D Column"...).
    pub kind_label: String,
    /// Kantor can't change its type or series (radar, stock, 100% stacked
    /// 3-D...).
    pub fixed: bool,
    /// The 3-D form of `kind`.
    pub three_d: bool,
    /// A combo's series' types and axes (empty for other types).
    pub series_types: Vec<SeriesType>,
    pub title: String,
    pub legend: LegendPos,
    pub x_title: String,
    pub y_title: String,
    pub has_axes: bool,
    pub colors: Vec<SeriesColor>,
    /// Series names (cached).
    pub names: Vec<String>,
    /// Every series' references: name, categories, values.
    pub refs: Vec<[Option<String>; 3]>,
    /// Every series' data labels.
    pub labels: Vec<DataLabels>,
    /// Label positions the (first) plot type has.
    pub label_positions: Vec<LabelPos>,
    /// Percentage labels apply (pie, doughnut).
    pub percent_labels: bool,
    /// The horizontal and vertical primary axes' options, and whether each
    /// is a value axis.
    pub axes: [(AxisOptions, bool); 2],
}

fn grouping(g: Node) -> &'static str {
    match val(g, "grouping") {
        Some("stacked") => "stacked",
        Some("percentStacked") => "percentStacked",
        _ => "",
    }
}

/// A plot type's series type in a combo chart (`None`: it can't be part
/// of one: bars, pies, scatter, 100% stacked, 3-D...).
fn combo_kind(g: Node) -> Option<ChartType> {
    let st = grouping(g);
    Some(match (g.tag_name().name(), st) {
        ("barChart", _) if val(g, "barDir") == Some("bar") => return None,
        ("barChart", "") => ChartType::Column,
        ("barChart", "stacked") => ChartType::StackedColumn,
        ("lineChart", "") => ChartType::Line,
        ("lineChart", "stacked") => ChartType::StackedLine,
        ("areaChart", "") => ChartType::Area,
        ("areaChart", "stacked") => ChartType::StackedArea,
        _ => return None,
    })
}

/// The chart's type, its name, whether Kantor can't change its type or
/// series, and whether it's 3-D.
fn kind_of(groups: &[Node]) -> (ChartType, String, bool, bool) {
    if groups.len() != 1 {
        if groups.len() > 1 && groups.iter().all(|g| combo_kind(*g).is_some()) {
            return (ChartType::Combo, "Combo".into(), false, false);
        }
        return (ChartType::Other, "Combo".into(), true, false);
    }
    let g = groups[0];
    let tag = g.tag_name().name();
    let st = grouping(g);
    let bar = val(g, "barDir") == Some("bar");
    // 3-D forms Kantor changes (100% stacked ones stay as they are).
    let three = match (tag, st) {
        ("bar3DChart", "" | "stacked") => Some(match (bar, st) {
            (false, "") => ChartType::Column,
            (false, _) => ChartType::StackedColumn,
            (true, "") => ChartType::Bar,
            (true, _) => ChartType::StackedBar,
        }),
        ("line3DChart", "") => Some(ChartType::Line),
        ("area3DChart", "") => Some(ChartType::Area),
        ("area3DChart", "stacked") => Some(ChartType::StackedArea),
        ("pie3DChart", _) => Some(ChartType::Pie),
        _ => None,
    };
    if let Some(k) = three {
        let label = if tag == "bar3DChart" && val(g, "grouping") == Some("standard") {
            if bar { "3-D Bar" } else { "3-D Column" }
        } else {
            k.label_3d()
        };
        return (k, label.to_owned(), false, true);
    }
    let (kind, label) = match tag {
        "barChart" => match (bar, st) {
            (false, "") => (ChartType::Column, ""),
            (false, "stacked") => (ChartType::StackedColumn, ""),
            (false, _) => (ChartType::Other, "100% Stacked Column"),
            (true, "") => (ChartType::Bar, ""),
            (true, "stacked") => (ChartType::StackedBar, ""),
            (true, _) => (ChartType::Other, "100% Stacked Bar"),
        },
        "lineChart" => match st {
            "" => (ChartType::Line, ""),
            "stacked" => (ChartType::StackedLine, ""),
            _ => (ChartType::Other, "100% Stacked Line"),
        },
        "areaChart" => match st {
            "" => (ChartType::Area, ""),
            "stacked" => (ChartType::StackedArea, ""),
            _ => (ChartType::Other, "100% Stacked Area"),
        },
        "pieChart" => (ChartType::Pie, ""),
        "doughnutChart" => (ChartType::Other, "Doughnut"),
        "scatterChart" => (ChartType::Scatter, ""),
        "bar3DChart" => (ChartType::Other, "3-D Column"),
        "line3DChart" => (ChartType::Other, "3-D Line"),
        "area3DChart" => (ChartType::Other, "3-D Area"),
        "ofPieChart" => (ChartType::Other, "Pie of Pie"),
        "radarChart" => (ChartType::Other, "Radar"),
        "bubbleChart" => (ChartType::Other, "Bubble"),
        "stockChart" => (ChartType::Other, "Stock"),
        "surfaceChart" | "surface3DChart" => (ChartType::Other, "Surface"),
        _ => (ChartType::Other, "Other"),
    };
    let label = if kind == ChartType::Other {
        label.to_owned()
    } else {
        kind.label().to_owned()
    };
    (kind, label, !CONVERTIBLE.contains(&tag), false)
}

/// Text of a `c:title` (rich text runs, or a cell's cached text).
fn title_text(t: Option<Node>) -> String {
    let Some(tx) = t.and_then(|t| kid(t, "tx")) else {
        return String::new();
    };
    if let Some(rich) = kid(tx, "rich") {
        return rich
            .children()
            .filter(|p| is(*p, NS_A, "p"))
            .map(|p| {
                p.descendants()
                    .filter(|d| is(*d, NS_A, "t"))
                    .filter_map(|d| d.text())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    tx.descendants()
        .filter(|d| is(*d, NS_C, "v"))
        .filter_map(|d| d.text())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The plot's first group's axes: (horizontal, vertical).
fn primary_axes<'a, 'i>(plot: Node<'a, 'i>) -> (Option<Node<'a, 'i>>, Option<Node<'a, 'i>>) {
    let Some(g) = groups(plot).into_iter().next() else {
        return (None, None);
    };
    let ids: Vec<&str> = kids(g, "axId").filter_map(|a| a.attribute("val")).collect();
    let mut h = None;
    let mut v = None;
    for ax in axes(plot) {
        if !val(ax, "axId").is_some_and(|i| ids.contains(&i)) {
            continue;
        }
        match val(ax, "axPos") {
            Some("b" | "t") => h = h.or(Some(ax)),
            _ => v = v.or(Some(ax)),
        }
    }
    (h, v)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    Fill,
    Line,
    Scatter,
}

fn family(group_tag: &str) -> Family {
    match group_tag {
        "lineChart" | "line3DChart" | "radarChart" | "stockChart" => Family::Line,
        "scatterChart" => Family::Scatter,
        _ => Family::Fill,
    }
}

/// Every series, in document order, with its group's element name.
/// Every series, in `c:order` (Excel's series order; document order
/// for ties), with its group's element name.
fn all_series<'a, 'i>(plot: Node<'a, 'i>) -> Vec<(Node<'a, 'i>, &'a str)> {
    let mut v: Vec<(Node<'a, 'i>, &'a str)> = groups(plot)
        .into_iter()
        .flat_map(|g| {
            let tag = g.tag_name().name();
            kids(g, "ser").map(move |s| (s, tag))
        })
        .collect();
    v.sort_by_key(|(s, _)| val(*s, "order").and_then(|o| o.parse::<u32>().ok()));
    v
}

/// A plot type's axis ids.
fn ax_ids(g: Node) -> Vec<String> {
    kids(g, "axId")
        .filter_map(|a| a.attribute("val").map(str::to_owned))
        .collect()
}

fn marker_shown(ser: Node) -> bool {
    kid(ser, "marker").is_some_and(|m| val(m, "symbol") != Some("none"))
}

/// The fill element giving a series its colour.
fn color_node<'a, 'i>(ser: Node<'a, 'i>, tag: &str) -> Option<Node<'a, 'i>> {
    let sp = kid(ser, "spPr");
    let ln = sp.and_then(|s| akid(s, "ln"));
    let fill_of = |n: Option<Node<'a, 'i>>| {
        n.and_then(|n| elements(n).find(|c| FILLS.contains(&c.tag_name().name())))
    };
    let marker = || fill_of(kid(ser, "marker").and_then(|m| kid(m, "spPr")));
    match family(tag) {
        Family::Fill => fill_of(sp),
        Family::Line => fill_of(ln).or_else(|| marker_shown(ser).then(marker).flatten()),
        Family::Scatter => {
            if ln.is_some_and(|l| akid(l, "noFill").is_some()) {
                marker()
            } else {
                fill_of(ln).or_else(marker)
            }
        }
    }
}

fn color_of(fill: Option<Node>) -> SeriesColor {
    let Some(f) = fill else {
        return SeriesColor::Automatic;
    };
    if is(f, NS_A, "solidFill")
        && let Some(c) = akid(f, "srgbClr")
        && elements(c).next().is_none()
        && let Some(v) = c.attribute("val")
        && v.len() == 6
        && let Ok(n) = u32::from_str_radix(v, 16)
    {
        return SeriesColor::Rgb([(n >> 16) as u8, (n >> 8) as u8, n as u8]);
    }
    SeriesColor::Custom
}

fn formula(n: Option<Node>) -> Option<String> {
    let n = n?;
    let src = elements(n).next()?;
    let f = kid(src, "f")?.text()?.trim();
    (!f.is_empty()).then(|| f.to_owned())
}

fn series_name(ser: Node) -> String {
    let Some(tx) = kid(ser, "tx") else {
        return String::new();
    };
    if let Some(v) = kid(tx, "v") {
        return v.text().unwrap_or_default().to_owned();
    }
    tx.descendants()
        .filter(|d| is(*d, NS_C, "v"))
        .filter_map(|d| d.text())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Read what the Chart panel shows from a chart part.
pub(crate) fn read(xml: &str) -> Result<Style> {
    let doc = Document::parse(xml)?;
    let (chart, plot) = plot_area(&doc)?;
    let gs = groups(plot);
    let (kind, kind_label, fixed, three_d) = kind_of(&gs);
    let title = kid(chart, "title");
    let legend = match kid(chart, "legend") {
        None => LegendPos::None,
        Some(l) => match val(l, "legendPos") {
            Some("l") => LegendPos::Left,
            Some("t") => LegendPos::Top,
            Some("b") => LegendPos::Bottom,
            _ => LegendPos::Right,
        },
    };
    let (h, v) = primary_axes(plot);
    let sers = all_series(plot);
    Ok(Style {
        kind,
        kind_label,
        fixed,
        three_d,
        series_types: if kind != ChartType::Combo {
            Vec::new()
        } else {
            let primary = gs.first().map(|g| ax_ids(*g)).unwrap_or_default();
            sers.iter()
                .map(|(s, _)| {
                    let g = group_of(*s);
                    SeriesType {
                        kind: g.and_then(combo_kind).unwrap_or(ChartType::Column),
                        secondary: g.is_some_and(|g| ax_ids(g) != primary),
                    }
                })
                .collect()
        },
        title: title_text(title),
        legend,
        x_title: title_text(h.and_then(|a| kid(a, "title"))),
        y_title: title_text(v.and_then(|a| kid(a, "title"))),
        has_axes: h.is_some() || v.is_some(),
        colors: sers
            .iter()
            .map(|(s, tag)| color_of(color_node(*s, tag)))
            .collect(),
        names: sers.iter().map(|(s, _)| series_name(*s)).collect(),
        refs: sers
            .iter()
            .map(|(s, _)| {
                [
                    formula(kid(*s, "tx")),
                    formula(kid(*s, "cat").or_else(|| kid(*s, "xVal"))),
                    formula(kid(*s, "val").or_else(|| kid(*s, "yVal"))),
                ]
            })
            .collect(),
        labels: sers
            .iter()
            .map(|(s, _)| {
                group_of(*s)
                    .map(|g| series_labels(g, *s))
                    .unwrap_or_default()
            })
            .collect(),
        label_positions: gs
            .first()
            .map(|g| positions(*g).to_vec())
            .unwrap_or_default(),
        percent_labels: gs.iter().any(|g| is_pie(g.tag_name().name())),
        axes: primary_axis_options(plot),
    })
}

// ------------------------------------------------------------------ titles

/// A `c:tx` with `text` as rich text, keeping the old one's body, list
/// style, paragraph and run properties.
fn rich_tx(src: &str, at: Node, old: Option<Node>, text: &str, vertical: bool) -> String {
    let p = prefix_of(src, at);
    let (a, decl) = a_ns(at);
    let rich = old
        .and_then(|t| kid(t, "tx"))
        .and_then(|tx| kid(tx, "rich"));
    let body = rich
        .and_then(|r| akid(r, "bodyPr"))
        .map(|n| raw(src, n).to_owned())
        .unwrap_or_else(|| {
            if vertical {
                format!("<{a}bodyPr rot=\"-5400000\" vert=\"horz\"/>")
            } else {
                format!("<{a}bodyPr/>")
            }
        });
    let lst = rich
        .and_then(|r| akid(r, "lstStyle"))
        .map(|n| raw(src, n).to_owned())
        .unwrap_or_else(|| format!("<{a}lstStyle/>"));
    let p0 = rich.and_then(|r| akid(r, "p"));
    let ppr = p0
        .and_then(|x| akid(x, "pPr"))
        .map(|n| raw(src, n).to_owned())
        .unwrap_or_else(|| format!("<{a}pPr><{a}defRPr/></{a}pPr>"));
    let rpr = p0
        .and_then(|x| x.descendants().find(|d| is(*d, NS_A, "rPr")))
        .map(|n| raw(src, n).to_owned())
        .unwrap_or_default();
    let mut paras = String::new();
    for line in text.split('\n') {
        paras.push_str(&format!(
            "<{a}p>{ppr}<{a}r>{rpr}<{a}t>{}</{a}t></{a}r></{a}p>",
            esc(line)
        ));
    }
    declare(
        &format!("<{p}tx><{p}rich>{body}{lst}{paras}</{p}rich></{p}tx>"),
        &decl,
    )
}

/// Set (or with empty `text`, remove) the `c:title` of `parent` (the chart
/// or an axis).
fn title_edits(src: &str, parent: Node, order: &[&str], text: &str, vertical: bool) -> Vec<Edit> {
    let title = kid(parent, "title");
    let text = text.trim();
    if text.is_empty() {
        return title
            .map(|t| (t.range(), String::new()))
            .into_iter()
            .collect();
    }
    if title_text(title) == text
        && title
            .and_then(|t| kid(t, "tx"))
            .and_then(|tx| kid(tx, "rich"))
            .is_some()
    {
        return Vec::new();
    }
    match title {
        Some(t) => {
            let tx = rich_tx(src, t, Some(t), text, vertical);
            match kid(t, "tx") {
                Some(old) => vec![(old.range(), tx)],
                None => vec![insert_ordered(src, t, "tx", TITLE_ORDER, &tx)],
            }
        }
        None => {
            let p = prefix_of(src, parent);
            let tx = rich_tx(src, parent, None, text, vertical);
            vec![insert_ordered(
                src,
                parent,
                "title",
                order,
                &format!("<{p}title>{tx}<{p}overlay val=\"0\"/></{p}title>"),
            )]
        }
    }
}

/// The chart title (empty: none).
pub(crate) fn set_title(xml: &str, text: &str) -> Result<String> {
    let doc = Document::parse(xml)?;
    let (chart, _) = plot_area(&doc)?;
    let mut edits = title_edits(xml, chart, CHART_ORDER, text, false);
    let deleted = if text.trim().is_empty() { "1" } else { "0" };
    if text.trim().is_empty() || kid(chart, "autoTitleDeleted").is_some() {
        edits.push(set_val(
            xml,
            chart,
            "autoTitleDeleted",
            CHART_ORDER,
            deleted,
        ));
    }
    Ok(splice(xml, edits))
}

/// An axis title: the horizontal or the vertical primary axis.
pub(crate) fn set_axis_title(xml: &str, horizontal: bool, text: &str) -> Result<String> {
    let doc = Document::parse(xml)?;
    let (_, plot) = plot_area(&doc)?;
    let (h, v) = primary_axes(plot);
    let Some(ax) = (if horizontal { h } else { v }) else {
        if text.trim().is_empty() {
            return Ok(xml.to_owned());
        }
        bail!("This chart has no axes");
    };
    Ok(splice(
        xml,
        title_edits(xml, ax, AX_ORDER, text, !horizontal),
    ))
}

/// Where the legend goes (`None` removes it). A manual layout of the legend
/// goes with a new position.
pub(crate) fn set_legend(xml: &str, pos: LegendPos) -> Result<String> {
    let doc = Document::parse(xml)?;
    let (chart, _) = plot_area(&doc)?;
    let legend = kid(chart, "legend");
    let code = match pos {
        LegendPos::None => {
            return Ok(match legend {
                Some(l) => splice(xml, vec![(l.range(), String::new())]),
                None => xml.to_owned(),
            });
        }
        LegendPos::Right => "r",
        LegendPos::Top => "t",
        LegendPos::Bottom => "b",
        LegendPos::Left => "l",
    };
    match legend {
        Some(l) => {
            let mut edits = vec![set_val(xml, l, "legendPos", LEGEND_ORDER, code)];
            if let Some(lay) = kid(l, "layout") {
                edits.push((lay.range(), String::new()));
            }
            Ok(splice(xml, edits))
        }
        None => {
            let p = prefix_of(xml, chart);
            Ok(splice(
                xml,
                vec![insert_ordered(
                    xml,
                    chart,
                    "legend",
                    CHART_ORDER,
                    &format!(
                        "<{p}legend><{p}legendPos val=\"{code}\"/><{p}overlay val=\"0\"/></{p}legend>"
                    ),
                )],
            ))
        }
    }
}

// ------------------------------------------------------------------ colours

/// A fill to set: an RGB colour, or markup taken from the same part.
#[derive(Clone)]
enum NewFill {
    Rgb([u8; 3]),
    Raw(String),
}

impl NewFill {
    fn text(&self, at: Node) -> String {
        match self {
            NewFill::Raw(t) => t.clone(),
            NewFill::Rgb(c) => {
                let (a, decl) = a_ns(at);
                declare(
                    &format!(
                        "<{a}solidFill><{a}srgbClr val=\"{}\"/></{a}solidFill>",
                        hex(*c)
                    ),
                    &decl,
                )
            }
        }
    }
}

/// Set (or remove) the fill of `holder` (an `a:spPr` or `a:ln`).
fn fill_edits(src: &str, holder: Node, order: &[&str], fill: Option<&NewFill>) -> Vec<Edit> {
    let old = elements(holder).find(|c| FILLS.contains(&c.tag_name().name()));
    match (old, fill) {
        (Some(o), Some(f)) => vec![(o.range(), f.text(holder))],
        (Some(o), None) => vec![(o.range(), String::new())],
        (None, Some(f)) => vec![insert_ordered(
            src,
            holder,
            "solidFill",
            order,
            &f.text(holder),
        )],
        (None, None) => Vec::new(),
    }
}

/// `<c:spPr>` holding `inner` (with `a:` declared if needed).
fn new_sppr(src: &str, at: Node, inner: &str) -> String {
    let p = prefix_of(src, at);
    format!("<{p}spPr>{inner}</{p}spPr>")
}

/// Edits colouring the series' line (`ln` of its `spPr`).
fn line_edits(src: &str, ser: Node, fill: Option<&NewFill>) -> Vec<Edit> {
    let sp = kid(ser, "spPr");
    let ln = sp.and_then(|s| akid(s, "ln"));
    match (sp, ln) {
        (_, Some(ln)) => fill_edits(src, ln, LN_ORDER, fill),
        (_, None) if fill.is_none() => Vec::new(),
        (Some(sp), None) => {
            let (a, decl) = a_ns(sp);
            let f = fill.expect("checked").text(sp);
            vec![insert_ordered(
                src,
                sp,
                "ln",
                SPPR_ORDER,
                &declare(&format!("<{a}ln>{f}</{a}ln>"), &decl),
            )]
        }
        (None, None) => {
            let (a, decl) = a_ns(ser);
            let f = fill.expect("checked").text(ser);
            let ln = declare(&format!("<{a}ln>{f}</{a}ln>"), &decl);
            vec![insert_ordered(
                src,
                ser,
                "spPr",
                SER_ORDER,
                &new_sppr(src, ser, &ln),
            )]
        }
    }
}

/// Edits colouring the series' markers (fill and outline).
fn marker_edits(src: &str, ser: Node, fill: Option<&NewFill>) -> Vec<Edit> {
    let p = prefix_of(src, ser);
    let Some(m) = kid(ser, "marker") else {
        let Some(f) = fill else { return Vec::new() };
        let (a, decl) = a_ns(ser);
        let f = f.text(ser);
        let sp = declare(&format!("<{p}spPr>{f}<{a}ln>{f}</{a}ln></{p}spPr>"), &decl);
        return vec![insert_ordered(
            src,
            ser,
            "marker",
            SER_ORDER,
            &format!("<{p}marker>{sp}</{p}marker>"),
        )];
    };
    match kid(m, "spPr") {
        Some(sp) => {
            let mut e = fill_edits(src, sp, SPPR_ORDER, fill);
            match akid(sp, "ln") {
                Some(ln) => e.extend(fill_edits(src, ln, LN_ORDER, fill)),
                None => {
                    if let Some(f) = fill {
                        let (a, decl) = a_ns(sp);
                        e.push(insert_ordered(
                            src,
                            sp,
                            "ln",
                            SPPR_ORDER,
                            &declare(&format!("<{a}ln>{}</{a}ln>", f.text(sp)), &decl),
                        ));
                    }
                }
            }
            e
        }
        None => {
            let Some(f) = fill else { return Vec::new() };
            let (a, decl) = a_ns(m);
            let f = f.text(m);
            let sp = declare(&format!("<{p}spPr>{f}<{a}ln>{f}</{a}ln></{p}spPr>"), &decl);
            vec![insert_ordered(src, m, "spPr", MARKER_ORDER, &sp)]
        }
    }
}

fn set_fill(xml: &str, index: usize, fill: Option<&NewFill>) -> Result<String> {
    // Two passes at most: the series' own fill or line, then its markers.
    let doc = Document::parse(xml)?;
    let (_, plot) = plot_area(&doc)?;
    let sers = all_series(plot);
    let Some(&(ser, tag)) = sers.get(index) else {
        bail!("no series {}", index + 1);
    };
    let fam = family(tag);
    let first = match fam {
        Family::Fill => match kid(ser, "spPr") {
            Some(sp) => fill_edits(xml, sp, SPPR_ORDER, fill),
            None => match fill {
                Some(f) => vec![insert_ordered(
                    xml,
                    ser,
                    "spPr",
                    SER_ORDER,
                    &new_sppr(xml, ser, &f.text(ser)),
                )],
                None => Vec::new(),
            },
        },
        Family::Line => line_edits(xml, ser, fill),
        Family::Scatter => {
            let no_line = kid(ser, "spPr")
                .and_then(|s| akid(s, "ln"))
                .is_some_and(|l| akid(l, "noFill").is_some());
            if no_line {
                Vec::new()
            } else {
                line_edits(xml, ser, fill)
            }
        }
    };
    let xml = splice(xml, first);
    let markers = match fam {
        Family::Fill => false,
        Family::Line => true,
        Family::Scatter => true,
    };
    if !markers {
        return Ok(xml);
    }
    let doc = Document::parse(&xml)?;
    let (_, plot) = plot_area(&doc)?;
    let sers = all_series(plot);
    let (ser, _) = sers[index];
    if fam == Family::Line && !marker_shown(ser) {
        return Ok(xml);
    }
    let e = marker_edits(&xml, ser, fill);
    Ok(splice(&xml, e))
}

/// Series `index`'s colour (`Custom` leaves it as it is).
pub(crate) fn set_color(xml: &str, index: usize, color: SeriesColor) -> Result<String> {
    match color {
        SeriesColor::Custom => Ok(xml.to_owned()),
        SeriesColor::Automatic => set_fill(xml, index, None),
        SeriesColor::Rgb(c) => set_fill(xml, index, Some(&NewFill::Rgb(c))),
    }
}

// ------------------------------------------------------------------ type

fn tag_of(k: ChartType) -> &'static str {
    match k {
        ChartType::Combo => "barChart",
        ChartType::Column | ChartType::StackedColumn | ChartType::Bar | ChartType::StackedBar => {
            "barChart"
        }
        ChartType::Line | ChartType::StackedLine => "lineChart",
        ChartType::Area | ChartType::StackedArea => "areaChart",
        ChartType::Pie => "pieChart",
        ChartType::Scatter => "scatterChart",
        _ => "barChart",
    }
}

/// Children of every plot type Kantor converts between.
const GROUP_ALL: &[&str] = &[
    "barDir",
    "grouping",
    "scatterStyle",
    "varyColors",
    "ser",
    "dLbls",
    "gapWidth",
    "overlap",
    "serLines",
    "dropLines",
    "hiLowLines",
    "upDownBars",
    "marker",
    "smooth",
    "firstSliceAng",
    "holeSize",
    "axId",
    "extLst",
];

fn group_order(tag: &str) -> &'static [&'static str] {
    match tag {
        "barChart" => &[
            "barDir",
            "grouping",
            "varyColors",
            "ser",
            "dLbls",
            "gapWidth",
            "overlap",
            "serLines",
            "axId",
            "extLst",
        ],
        "lineChart" => &[
            "grouping",
            "varyColors",
            "ser",
            "dLbls",
            "dropLines",
            "hiLowLines",
            "upDownBars",
            "marker",
            "smooth",
            "axId",
            "extLst",
        ],
        "areaChart" => &[
            "grouping",
            "varyColors",
            "ser",
            "dLbls",
            "dropLines",
            "axId",
            "extLst",
        ],
        "pieChart" => &["varyColors", "ser", "dLbls", "firstSliceAng", "extLst"],
        "doughnutChart" => &[
            "varyColors",
            "ser",
            "dLbls",
            "firstSliceAng",
            "holeSize",
            "extLst",
        ],
        _ => &[
            "scatterStyle",
            "varyColors",
            "ser",
            "dLbls",
            "axId",
            "extLst",
        ],
    }
}

fn ser_order(tag: &str) -> &'static [&'static str] {
    match tag {
        "barChart" => &[
            "idx",
            "order",
            "tx",
            "spPr",
            "invertIfNegative",
            "pictureOptions",
            "dPt",
            "dLbls",
            "trendline",
            "errBars",
            "cat",
            "val",
            "shape",
            "extLst",
        ],
        "lineChart" => &[
            "idx",
            "order",
            "tx",
            "spPr",
            "marker",
            "dPt",
            "dLbls",
            "trendline",
            "errBars",
            "cat",
            "val",
            "smooth",
            "extLst",
        ],
        "areaChart" => &[
            "idx",
            "order",
            "tx",
            "spPr",
            "pictureOptions",
            "dPt",
            "dLbls",
            "trendline",
            "errBars",
            "cat",
            "val",
            "extLst",
        ],
        "pieChart" | "doughnutChart" => &[
            "idx",
            "order",
            "tx",
            "spPr",
            "explosion",
            "dPt",
            "dLbls",
            "cat",
            "val",
            "extLst",
        ],
        _ => &[
            "idx",
            "order",
            "tx",
            "spPr",
            "marker",
            "dPt",
            "dLbls",
            "trendline",
            "errBars",
            "xVal",
            "yVal",
            "smooth",
            "extLst",
        ],
    }
}

/// `dLbls` without positions (they differ between chart types).
fn labels_for(src: &str, n: Node, same: bool) -> String {
    if same {
        raw(src, n).to_owned()
    } else {
        raw_without(src, n, &|d| d.tag_name().name() == "dLblPos")
    }
}

/// A series converted from group `from` to group `to`.
fn convert_ser(src: &str, s: Node, from: &str, to: &str) -> String {
    let p = prefix_of(src, s);
    let same = from == to;
    let line_to_fill = family(to) == Family::Fill && family(from) != Family::Fill;
    let mut over = |slot: &str| -> Option<Option<String>> {
        let one = |name: &str| kid(s, name);
        match slot {
            "cat" | "xVal" => {
                let n = one("cat").or_else(|| one("xVal"))?;
                Some(Some(renamed(src, n, slot)))
            }
            "val" | "yVal" => {
                let n = one("val").or_else(|| one("yVal"))?;
                Some(Some(renamed(src, n, slot)))
            }
            "dLbls" => Some(one("dLbls").map(|n| labels_for(src, n, same))),
            "spPr" => {
                let sp = one("spPr")?;
                let ln = akid(sp, "ln");
                let drop_ln =
                    !same && ln.is_some_and(|l| line_to_fill || akid(l, "noFill").is_some());
                Some(Some(if drop_ln {
                    let r = ln.expect("checked").range();
                    format!(
                        "{}{}",
                        &src[sp.range().start..r.start],
                        &src[r.end..sp.range().end]
                    )
                } else {
                    raw(src, sp).to_owned()
                }))
            }
            "errBars" if matches!(to, "barChart" | "lineChart") => {
                Some(one("errBars").map(|n| raw(src, n).to_owned()))
            }
            "invertIfNegative" if !same => Some(Some(
                one(slot)
                    .map(|n| raw(src, n).to_owned())
                    .unwrap_or_else(|| format!("<{p}invertIfNegative val=\"0\"/>")),
            )),
            "marker" if !same && to == "lineChart" => Some(Some(
                one(slot)
                    .map(|n| raw(src, n).to_owned())
                    .unwrap_or_else(|| format!("<{p}marker><{p}symbol val=\"none\"/></{p}marker>")),
            )),
            "smooth" if !same => Some(Some(
                one(slot)
                    .map(|n| raw(src, n).to_owned())
                    .unwrap_or_else(|| format!("<{p}smooth val=\"0\"/>")),
            )),
            _ => None,
        }
    };
    rebuild(src, s, "ser", ser_order(to), SER_ORDER, &mut over)
}

/// An axis converted to `tag` at `pos`.
fn convert_axis(src: &str, ax: Node, tag: &str, pos: &str, cross_between: &str) -> String {
    let p = prefix_of(src, ax);
    let order = if tag == "valAx" { VAL_AX } else { CAT_AX };
    let mut over = |slot: &str| -> Option<Option<String>> {
        match slot {
            "axPos" => Some(Some(format!("<{p}axPos val=\"{pos}\"/>"))),
            "crossBetween" => Some(Some(format!("<{p}crossBetween val=\"{cross_between}\"/>"))),
            "auto" | "lblAlgn" | "lblOffset" | "noMultiLvlLbl" if kid(ax, slot).is_none() => {
                Some(Some(match slot {
                    "auto" => format!("<{p}auto val=\"1\"/>"),
                    "lblAlgn" => format!("<{p}lblAlgn val=\"ctr\"/>"),
                    "lblOffset" => format!("<{p}lblOffset val=\"100\"/>"),
                    _ => format!("<{p}noMultiLvlLbl val=\"0\"/>"),
                }))
            }
            _ => None,
        }
    };
    rebuild(src, ax, tag, order, AX_ORDER, &mut over)
}

/// Default axes for a plot that had none (from a pie): ids and markup.
fn default_axes(kind: ChartType, p: &str) -> ([u32; 2], String) {
    let ids = [500_000_001, 500_000_002];
    let horizontal = matches!(kind, ChartType::Bar | ChartType::StackedBar);
    let (cat_pos, val_pos) = if horizontal { ("l", "b") } else { ("b", "l") };
    let tail = "<c:majorTickMark val=\"out\"/><c:minorTickMark val=\"none\"/><c:tickLblPos val=\"nextTo\"/>";
    let text = if kind == ChartType::Scatter {
        format!(
            "<c:valAx><c:axId val=\"{a}\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"b\"/><c:numFmt formatCode=\"General\" sourceLinked=\"1\"/>{tail}<c:crossAx val=\"{b}\"/><c:crosses val=\"autoZero\"/><c:crossBetween val=\"midCat\"/></c:valAx>\
<c:valAx><c:axId val=\"{b}\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"l\"/><c:majorGridlines/><c:numFmt formatCode=\"General\" sourceLinked=\"1\"/>{tail}<c:crossAx val=\"{a}\"/><c:crosses val=\"autoZero\"/><c:crossBetween val=\"midCat\"/></c:valAx>",
            a = ids[0],
            b = ids[1]
        )
    } else {
        let between = if matches!(kind, ChartType::Area | ChartType::StackedArea) {
            "midCat"
        } else {
            "between"
        };
        format!(
            "<c:catAx><c:axId val=\"{a}\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"{cat_pos}\"/><c:numFmt formatCode=\"General\" sourceLinked=\"1\"/>{tail}<c:crossAx val=\"{b}\"/><c:crosses val=\"autoZero\"/><c:auto val=\"1\"/><c:lblAlgn val=\"ctr\"/><c:lblOffset val=\"100\"/><c:noMultiLvlLbl val=\"0\"/></c:catAx>\
<c:valAx><c:axId val=\"{b}\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"{val_pos}\"/><c:majorGridlines/><c:numFmt formatCode=\"General\" sourceLinked=\"1\"/>{tail}<c:crossAx val=\"{a}\"/><c:crosses val=\"autoZero\"/><c:crossBetween val=\"{between}\"/></c:valAx>",
            a = ids[0],
            b = ids[1]
        )
    };
    (ids, prefixed(&text, p, "a:"))
}

/// Change the chart's type. Its series keep their data, names, labels and
/// formatting; the axes are converted (category ↔ value for scatter,
/// positions for bars), made (from a pie) or removed (to a pie).
pub(crate) fn set_kind(xml: &str, to: ChartType) -> Result<String> {
    if to == ChartType::Other {
        return Ok(xml.to_owned());
    }
    let doc = Document::parse(xml)?;
    let (_, plot) = plot_area(&doc)?;
    let gs = groups(plot);
    let (now, _, fixed, three_d) = kind_of(&gs);
    if fixed || three_d || gs.len() != 1 {
        bail!("Kantor can't change this chart's type");
    }
    if now == to {
        return Ok(xml.to_owned());
    }
    let g = gs[0];
    let from = g.tag_name().name();
    let to_tag = tag_of(to);
    let same = from == to_tag;
    let p = prefix_of(xml, g);
    let stacked = matches!(
        to,
        ChartType::StackedColumn
            | ChartType::StackedBar
            | ChartType::StackedLine
            | ChartType::StackedArea
    );
    let horizontal = matches!(to, ChartType::Bar | ChartType::StackedBar);
    // Colours, to put back where the new type keeps them.
    let colors: Vec<Option<String>> = kids(g, "ser")
        .map(|s| {
            color_node(s, from)
                .filter(|f| is(*f, NS_A, "solidFill"))
                .map(|f| raw(xml, f).to_owned())
        })
        .collect();
    let mut edits: Vec<Edit> = Vec::new();
    let old_ids: Vec<String> = kids(g, "axId")
        .filter_map(|a| a.attribute("val").map(str::to_owned))
        .collect();
    let plot_axes = axes(plot);
    let mut ax_ids: Vec<String> = Vec::new();
    if to == ChartType::Pie {
        for ax in &plot_axes {
            edits.push((ax.range(), String::new()));
        }
    } else if old_ids.len() < 2 || plot_axes.is_empty() {
        let (ids, text) = default_axes(to, &p);
        ax_ids = ids.iter().map(u32::to_string).collect();
        for ax in &plot_axes {
            edits.push((ax.range(), String::new()));
        }
        let at = g.range().end;
        edits.push((at..at, text));
    } else {
        ax_ids = old_ids.clone();
        let by_id = |id: &str| {
            plot_axes
                .iter()
                .copied()
                .find(|a| val(*a, "axId") == Some(id))
        };
        let (a0, a1) = (by_id(&old_ids[0]), by_id(&old_ids[1]));
        // Which of the two is the category (or x) axis.
        let x_first = match (a0, a1) {
            (Some(a), Some(b)) => {
                fn t<'a>(n: Node<'a, '_>) -> &'a str {
                    n.tag_name().name()
                }
                if t(a) != "valAx" && t(b) == "valAx" {
                    true
                } else if t(b) != "valAx" && t(a) == "valAx" {
                    false
                } else {
                    // Two value axes (scatter): x is the horizontal one.
                    !matches!(val(b, "axPos"), Some("b" | "t"))
                }
            }
            _ => true,
        };
        let (x, y) = if x_first { (a0, a1) } else { (a1, a0) };
        let flip = |pos: Option<&str>, want_horizontal: bool| -> &'static str {
            match (pos, want_horizontal) {
                (Some("t"), true) | (Some("r"), true) => "t",
                (_, true) => "b",
                (Some("r"), false) | (Some("t"), false) => "r",
                (_, false) => "l",
            }
        };
        let between = match to {
            ChartType::Area | ChartType::StackedArea | ChartType::Scatter => "midCat",
            _ => "between",
        };
        if let Some(x) = x {
            let tag = if to == ChartType::Scatter {
                "valAx"
            } else if x.tag_name().name() == "valAx" {
                "catAx"
            } else {
                x.tag_name().name()
            };
            let pos = flip(val(x, "axPos"), !horizontal);
            edits.push((x.range(), convert_axis(xml, x, tag, pos, between)));
        }
        if let Some(y) = y {
            let pos = flip(val(y, "axPos"), horizontal);
            edits.push((y.range(), convert_axis(xml, y, "valAx", pos, between)));
        }
    }
    let sers: String = kids(g, "ser")
        .map(|s| convert_ser(xml, s, from, to_tag))
        .collect();
    let vary = if to == ChartType::Pie {
        "1"
    } else if from == "pieChart" || from == "doughnutChart" {
        "0"
    } else {
        val(g, "varyColors").unwrap_or("0")
    };
    let keep = |name: &str| kid(g, name).map(|n| raw(xml, n).to_owned());
    let mut over = |slot: &str| -> Option<Option<String>> {
        Some(match slot {
            "barDir" => Some(format!(
                "<{p}barDir val=\"{}\"/>",
                if horizontal { "bar" } else { "col" }
            )),
            "grouping" => Some(format!(
                "<{p}grouping val=\"{}\"/>",
                match (to_tag, stacked) {
                    (_, true) => "stacked",
                    ("barChart", false) => "clustered",
                    _ => "standard",
                }
            )),
            "varyColors" => Some(format!("<{p}varyColors val=\"{vary}\"/>")),
            "ser" => Some(sers.clone()),
            "dLbls" => kid(g, "dLbls").map(|n| labels_for(xml, n, same)),
            "gapWidth" => Some(
                same.then(|| keep("gapWidth"))
                    .flatten()
                    .unwrap_or_else(|| format!("<{p}gapWidth val=\"150\"/>")),
            ),
            "overlap" => {
                if stacked {
                    Some(format!("<{p}overlap val=\"100\"/>"))
                } else if same && grouping(g).is_empty() {
                    keep("overlap")
                } else {
                    None
                }
            }
            "marker" if to_tag == "lineChart" => Some(
                same.then(|| keep("marker"))
                    .flatten()
                    .unwrap_or_else(|| format!("<{p}marker val=\"1\"/>")),
            ),
            "firstSliceAng" => Some(
                matches!(from, "pieChart" | "doughnutChart")
                    .then(|| keep("firstSliceAng"))
                    .flatten()
                    .unwrap_or_else(|| format!("<{p}firstSliceAng val=\"0\"/>")),
            ),
            "scatterStyle" => Some(
                same.then(|| keep("scatterStyle"))
                    .flatten()
                    .unwrap_or_else(|| format!("<{p}scatterStyle val=\"lineMarker\"/>")),
            ),
            "axId" => Some(
                ax_ids
                    .iter()
                    .map(|i| format!("<{p}axId val=\"{i}\"/>"))
                    .collect(),
            ),
            "serLines" | "dropLines" | "hiLowLines" | "upDownBars" | "smooth" | "extLst" => {
                if same { keep(slot) } else { None }
            }
            _ => None,
        })
    };
    let group = rebuild(xml, g, to_tag, group_order(to_tag), GROUP_ALL, &mut over);
    edits.push((g.range(), group));
    let mut out = splice(xml, edits);
    if to != ChartType::Pie {
        for (i, c) in colors.into_iter().enumerate() {
            if let Some(c) = c {
                out = set_fill(&out, i, Some(&NewFill::Raw(c)))?;
            }
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------ series

/// A series' references and cached values, as `c:tx` / `c:cat` / `c:val`
/// content (`c:strRef`...), written with `c:`.
pub(crate) struct NewSeries {
    pub tx: Option<String>,
    pub cat: Option<String>,
    pub val: String,
}

/// Replace the series of a chart with one plot type: the first ones keep
/// their formatting (only their name, categories and values change), the
/// rest are removed or added.
pub(crate) fn set_series(xml: &str, series: &[NewSeries]) -> Result<String> {
    let doc = Document::parse(xml)?;
    let (_, plot) = plot_area(&doc)?;
    let gs = groups(plot);
    let (_, _, fixed, three_d) = kind_of(&gs);
    if fixed || three_d || gs.len() != 1 {
        bail!("Kantor can't change this chart's data range");
    }
    let g = gs[0];
    let tag = g.tag_name().name();
    let p = prefix_of(xml, g);
    let old: Vec<Node> = kids(g, "ser").collect();
    let scatter = tag == "scatterChart";
    let (cat_name, val_name) = if scatter {
        ("xVal", "yVal")
    } else {
        ("cat", "val")
    };
    let mut next_idx = old
        .iter()
        .filter_map(|s| val(*s, "idx")?.parse::<u32>().ok())
        .max()
        .map_or(0, |m| m + 1);
    let mut next_order = old
        .iter()
        .filter_map(|s| val(*s, "order")?.parse::<u32>().ok())
        .max()
        .map_or(0, |m| m + 1);
    let mut out = String::new();
    for (i, s) in series.iter().enumerate() {
        let tx =
            s.tx.as_ref()
                .map(|t| prefixed(&format!("<c:tx>{t}</c:tx>"), &p, "a:"));
        let cat = s
            .cat
            .as_ref()
            .map(|c| prefixed(&format!("<c:{cat_name}>{c}</c:{cat_name}>"), &p, "a:"));
        let v = prefixed(&format!("<c:{val_name}>{}</c:{val_name}>", s.val), &p, "a:");
        match old.get(i) {
            Some(o) => {
                let mut over = |slot: &str| -> Option<Option<String>> {
                    match slot {
                        "tx" => Some(tx.clone()),
                        "cat" | "xVal" => Some(cat.clone()),
                        "val" | "yVal" => Some(Some(v.clone())),
                        _ => None,
                    }
                };
                out.push_str(&rebuild(
                    xml,
                    *o,
                    "ser",
                    ser_order(tag),
                    SER_ORDER,
                    &mut over,
                ));
            }
            None => {
                let mut x =
                    format!("<{p}ser><{p}idx val=\"{next_idx}\"/><{p}order val=\"{next_order}\"/>");
                next_idx += 1;
                next_order += 1;
                if let Some(t) = &tx {
                    x.push_str(t);
                }
                match tag {
                    "barChart" => x.push_str(&format!("<{p}invertIfNegative val=\"0\"/>")),
                    "lineChart" => {
                        x.push_str(&format!("<{p}marker><{p}symbol val=\"none\"/></{p}marker>"))
                    }
                    _ => {}
                }
                if let Some(c) = &cat {
                    x.push_str(c);
                }
                x.push_str(&v);
                if matches!(tag, "lineChart" | "scatterChart") {
                    x.push_str(&format!("<{p}smooth val=\"0\"/>"));
                }
                x.push_str(&format!("</{p}ser>"));
                out.push_str(&x);
            }
        }
    }
    let edit = match (old.first(), old.last()) {
        (Some(a), Some(b)) => (a.range().start..b.range().end, out),
        _ => insert_ordered(xml, g, "ser", group_order(tag), &out),
    };
    // Legend entries of series that are gone point nowhere: Excel keys
    // them by index, so they stay valid; nothing else refers to series.
    Ok(splice(xml, vec![edit]))
}

// ------------------------------------------------------------------ data labels

const DLBLS_ORDER: &[&str] = &[
    "dLbl",
    "delete",
    "numFmt",
    "spPr",
    "txPr",
    "dLblPos",
    "showLegendKey",
    "showVal",
    "showCatName",
    "showSerName",
    "showPercent",
    "showBubbleSize",
    "separator",
    "showLeaderLines",
    "leaderLines",
    "extLst",
];

fn on(n: Node, name: &str) -> bool {
    matches!(val(n, name), Some("1" | "true"))
}

fn is_pie(tag: &str) -> bool {
    matches!(
        tag,
        "pieChart" | "pie3DChart" | "doughnutChart" | "ofPieChart"
    )
}

/// Label positions allowed for group `g`.
fn positions(g: Node) -> &'static [LabelPos] {
    LabelPos::for_group(g.tag_name().name(), !grouping(g).is_empty())
}

/// `c:dLbls` as the panel shows it (`None`: deleted or nothing shown).
fn labels_of(d: Node) -> Option<DataLabels> {
    if on(d, "delete") {
        return None;
    }
    let l = DataLabels {
        value: on(d, "showVal"),
        category: on(d, "showCatName"),
        series: on(d, "showSerName"),
        percent: on(d, "showPercent"),
        pos: val(d, "dLblPos").and_then(LabelPos::from_code),
        num_fmt: kid(d, "numFmt")
            .filter(|f| !matches!(f.attribute("sourceLinked"), Some("1" | "true")))
            .and_then(|f| f.attribute("formatCode"))
            .map(str::to_owned),
        font: label_font(d),
    };
    l.shown().then_some(l)
}

/// The default run properties of a `c:txPr` (`a:p/a:pPr/a:defRPr`).
fn def_rpr<'a, 'i>(holder: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    let tx = kid(holder, "txPr")?;
    let p = akid(tx, "p")?;
    akid(akid(p, "pPr")?, "defRPr")
}

fn label_font(d: Node) -> LabelFont {
    let Some(r) = def_rpr(d) else {
        return LabelFont::default();
    };
    let flag = |name: &str| r.attribute(name).map(|v| matches!(v, "1" | "true"));
    LabelFont {
        size: r
            .attribute("sz")
            .and_then(|v| v.parse::<f32>().ok())
            .map(|v| v / 100.0),
        bold: flag("b"),
        italic: flag("i"),
        color: akid(r, "solidFill")
            .and_then(|f| akid(f, "srgbClr"))
            .and_then(|c| c.attribute("val"))
            .filter(|v| v.len() == 6)
            .and_then(|v| u32::from_str_radix(v, 16).ok())
            .map(|n| [(n >> 16) as u8, (n >> 8) as u8, n as u8]),
    }
}

/// A `c:txPr` giving labels `font` (its body properties kept from `old`).
fn label_txpr(src: &str, at: Node, old: Option<Node>, font: &LabelFont) -> String {
    let p = prefix_of(src, at);
    let (a, decl) = a_ns(at);
    let body = old
        .and_then(|t| akid(t, "bodyPr"))
        .map(|b| raw(src, b).to_owned())
        .unwrap_or_else(|| format!("<{a}bodyPr/>"));
    let mut attrs = String::new();
    if let Some(sz) = font.size {
        attrs.push_str(&format!(" sz=\"{}\"", (sz * 100.0).round() as i64));
    }
    if let Some(b) = font.bold {
        attrs.push_str(&format!(" b=\"{}\"", u8::from(b)));
    }
    if let Some(i) = font.italic {
        attrs.push_str(&format!(" i=\"{}\"", u8::from(i)));
    }
    let fill = font
        .color
        .map(|c| {
            format!(
                "<{a}solidFill><{a}srgbClr val=\"{}\"/></{a}solidFill>",
                hex(c)
            )
        })
        .unwrap_or_default();
    let rpr = if fill.is_empty() {
        format!("<{a}defRPr{attrs}/>")
    } else {
        format!("<{a}defRPr{attrs}>{fill}</{a}defRPr>")
    };
    declare(
        &format!(
            "<{p}txPr>{body}<{a}lstStyle/><{a}p><{a}pPr>{rpr}</{a}pPr><{a}endParaRPr lang=\"en-US\"/></{a}p></{p}txPr>"
        ),
        &decl,
    )
}

/// The labels series `ser` of group `g` shows.
fn series_labels(g: Node, ser: Node) -> DataLabels {
    match kid(ser, "dLbls") {
        Some(d) => labels_of(d),
        None => kid(g, "dLbls").and_then(labels_of),
    }
    .unwrap_or_default()
}

/// The group of series `ser`.
fn group_of<'a, 'i>(ser: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    ser.parent_element()
}

/// Series `index`'s data labels: Excel's Label Contains, Label Position
/// and Number. Other formatting (text, fill, per-point labels, leader
/// lines) stays. Positions the chart type doesn't have are left out.
pub(crate) fn set_labels(xml: &str, index: usize, labels: &DataLabels) -> Result<String> {
    let doc = Document::parse(xml)?;
    let (_, plot) = plot_area(&doc)?;
    let sers = all_series(plot);
    let Some((ser, tag)) = sers.get(index).copied() else {
        bail!("no series {}", index + 1);
    };
    let g = group_of(ser).ok_or_else(|| anyhow!("no group"))?;
    let old = kid(ser, "dLbls");
    let p = prefix_of(xml, ser);
    let group_shows = kid(g, "dLbls").and_then(labels_of).is_some();
    if !labels.shown() {
        let edit = match (old, group_shows) {
            (Some(o), false) => (o.range(), String::new()),
            (Some(o), true) => (
                o.range(),
                format!("<{p}dLbls><{p}delete val=\"1\"/></{p}dLbls>"),
            ),
            (None, true) => insert_ordered(
                xml,
                ser,
                "dLbls",
                SER_ORDER,
                &format!("<{p}dLbls><{p}delete val=\"1\"/></{p}dLbls>"),
            ),
            (None, false) => return Ok(xml.to_owned()),
        };
        return Ok(splice(xml, vec![edit]));
    }
    let pos = labels.pos.filter(|l| positions(g).contains(l));
    let percent = labels.percent && is_pie(tag);
    // Unset flags and parts come from the series' own labels, else the
    // group's (Excel copies them down when one series changes).
    let base = old
        .filter(|o| !on(*o, "delete"))
        .or_else(|| kid(g, "dLbls").filter(|o| !on(*o, "delete")));
    let flag = |name: &str, v: bool| format!("<{p}{name} val=\"{}\"/>", u8::from(v));
    let over = |slot: &str| -> Option<Option<String>> {
        Some(match slot {
            "delete" => None,
            "numFmt" => match &labels.num_fmt {
                Some(code) => {
                    let keep = base.and_then(|b| kid(b, "numFmt")).filter(|f| {
                        f.attribute("formatCode") == Some(code.as_str())
                            && !matches!(f.attribute("sourceLinked"), Some("1" | "true"))
                    });
                    Some(match keep {
                        Some(f) => raw(xml, f).to_owned(),
                        None => format!(
                            "<{p}numFmt formatCode=\"{}\" sourceLinked=\"0\"/>",
                            esc(code)
                        ),
                    })
                }
                None => base
                    .and_then(|b| kid(b, "numFmt"))
                    .filter(|f| matches!(f.attribute("sourceLinked"), Some("1" | "true")))
                    .map(|f| raw(xml, f).to_owned()),
            },
            "dLblPos" => pos.map(|l| format!("<{p}dLblPos val=\"{}\"/>", l.code())),
            "showLegendKey" => Some(
                base.and_then(|b| kid(b, "showLegendKey"))
                    .map(|n| raw(xml, n).to_owned())
                    .unwrap_or_else(|| flag("showLegendKey", false)),
            ),
            "showVal" => Some(flag("showVal", labels.value)),
            "showCatName" => Some(flag("showCatName", labels.category)),
            "showSerName" => Some(flag("showSerName", labels.series)),
            "showPercent" => Some(flag("showPercent", percent)),
            "showBubbleSize" => Some(
                base.and_then(|b| kid(b, "showBubbleSize"))
                    .map(|n| raw(xml, n).to_owned())
                    .unwrap_or_else(|| flag("showBubbleSize", false)),
            ),
            "txPr" if base.map(label_font).unwrap_or_default() != labels.font => {
                let old = old
                    .filter(|o| !on(*o, "delete"))
                    .or(base)
                    .and_then(|b| kid(b, "txPr"));
                if labels.font.is_default() {
                    None
                } else {
                    Some(label_txpr(xml, ser, old, &labels.font))
                }
            }
            "dLbl" | "spPr" | "txPr" | "separator" | "showLeaderLines" | "leaderLines"
            | "extLst" => {
                // Copied from the series' own labels, else from the group's.
                let src = old.filter(|o| !on(*o, "delete")).or(base)?;
                let t: String = kids_named(src, slot)
                    .map(|n| raw(xml, n).to_owned())
                    .collect();
                if t.is_empty() { None } else { Some(t) }
            }
            _ => return None,
        })
    };
    let mut text = format!("<{p}dLbls>");
    for slot in DLBLS_ORDER {
        if let Some(Some(t)) = over(slot) {
            text.push_str(&t);
        }
    }
    text.push_str(&format!("</{p}dLbls>"));
    let edit = match old {
        Some(o) => (o.range(), text),
        None => insert_ordered(xml, ser, "dLbls", SER_ORDER, &text),
    };
    Ok(splice(xml, vec![edit]))
}

fn kids_named<'a, 'i>(n: Node<'a, 'i>, name: &str) -> impl Iterator<Item = Node<'a, 'i>> {
    let name = name.to_owned();
    n.children().filter(move |c| is(*c, NS_C, &name))
}

/// `xml` without label positions its plot types don't have (after a type
/// change: a stacked column has no Outside End).
pub(crate) fn drop_bad_positions(xml: &str) -> String {
    let Ok(doc) = Document::parse(xml) else {
        return xml.to_owned();
    };
    let Ok((_, plot)) = plot_area(&doc) else {
        return xml.to_owned();
    };
    let mut edits = Vec::new();
    for g in groups(plot) {
        let ok = positions(g);
        for d in g.descendants().filter(|d| is(*d, NS_C, "dLblPos")) {
            let bad = d
                .attribute("val")
                .and_then(LabelPos::from_code)
                .is_none_or(|l| !ok.contains(&l));
            if bad {
                edits.push((d.range(), String::new()));
            }
        }
    }
    if edits.is_empty() {
        xml.to_owned()
    } else {
        splice(xml, edits)
    }
}

// ------------------------------------------------------------------ axis options

const SCALING_ORDER: &[&str] = &["logBase", "orientation", "max", "min", "extLst"];
const DISP_UNITS_ORDER: &[&str] = &["custUnit", "builtInUnit", "dispUnitsLbl", "extLst"];

fn is_value_axis(ax: Node) -> bool {
    ax.tag_name().name() == "valAx"
}

/// What the panel shows for an axis.
fn axis_options(ax: Node) -> AxisOptions {
    let scaling = kid(ax, "scaling");
    let f = |n: Option<Node>, name: &str| -> Option<f64> {
        n.and_then(|n| val(n, name))
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
    };
    let value = is_value_axis(ax);
    let date = ax.tag_name().name() == "dateAx";
    let units = value || date;
    let time = |name: &str| val(ax, name).and_then(TimeUnit::from_code).filter(|_| date);
    AxisOptions {
        min: f(scaling, "min").filter(|_| value),
        max: f(scaling, "max").filter(|_| value),
        major: f(Some(ax), "majorUnit").filter(|u| units && *u > 0.0),
        minor: f(Some(ax), "minorUnit").filter(|u| units && *u > 0.0),
        axis_type: match (ax.tag_name().name(), val(ax, "auto")) {
            (_, Some("1" | "true")) | ("valAx" | "serAx", _) => AxisType::Auto,
            ("dateAx", _) => AxisType::Date,
            ("catAx", Some(_)) => AxisType::Text,
            _ => AxisType::Auto,
        },
        base_unit: time("baseTimeUnit"),
        major_time: time("majorTimeUnit"),
        minor_time: time("minorTimeUnit"),
        log_base: f(scaling, "logBase").filter(|b| value && *b >= 2.0),
        reverse: scaling.and_then(|s| val(s, "orientation")) == Some("maxMin"),
        num_fmt: kid(ax, "numFmt")
            .filter(|f| !matches!(f.attribute("sourceLinked"), Some("1" | "true")))
            .and_then(|f| f.attribute("formatCode"))
            .map(str::to_owned),
        units: match kid(ax, "dispUnits").filter(|_| value) {
            None => DisplayUnits::None,
            Some(d) => val(d, "builtInUnit").map_or(DisplayUnits::Custom, DisplayUnits::from_code),
        },
    }
}

/// The primary axes' options: (horizontal, vertical), and whether each is
/// a value axis.
fn primary_axis_options(plot: Node) -> [(AxisOptions, bool); 2] {
    let (h, v) = primary_axes(plot);
    [h, v].map(|a| {
        a.map(|a| (axis_options(a), is_value_axis(a)))
            .unwrap_or_default()
    })
}

/// Make the primary category axis a text axis (`c:catAx`, not automatic),
/// a date axis (`c:dateAx`) or automatic (`c:auto` on, its element kept).
fn set_axis_type(xml: &str, horizontal: bool, to: AxisType) -> Result<String> {
    let doc = Document::parse(xml)?;
    let (_, plot) = plot_area(&doc)?;
    let (h, v) = primary_axes(plot);
    let Some(ax) = (if horizontal { h } else { v }) else {
        return Ok(xml.to_owned());
    };
    let tag = ax.tag_name().name();
    if !matches!(tag, "catAx" | "dateAx") || axis_options(ax).axis_type == to {
        return Ok(xml.to_owned());
    }
    let p = prefix_of(xml, ax);
    let auto = |on: bool| Some(Some(format!("<{p}auto val=\"{}\"/>", u8::from(on))));
    let text = match (to, tag) {
        (AxisType::Auto, _) => {
            let order = if tag == "dateAx" { DATE_AX } else { CAT_AX };
            rebuild(xml, ax, tag, order, AX_ORDER, &mut |slot| {
                if slot == "auto" { auto(true) } else { None }
            })
        }
        (AxisType::Text, _) => {
            rebuild(xml, ax, "catAx", CAT_AX, AX_ORDER, &mut |slot| match slot {
                "auto" => auto(false),
                "lblAlgn" if kid(ax, slot).is_none() => {
                    Some(Some(format!("<{p}lblAlgn val=\"ctr\"/>")))
                }
                "lblOffset" if kid(ax, slot).is_none() => {
                    Some(Some(format!("<{p}lblOffset val=\"100\"/>")))
                }
                "noMultiLvlLbl" if kid(ax, slot).is_none() => {
                    Some(Some(format!("<{p}noMultiLvlLbl val=\"0\"/>")))
                }
                _ => None,
            })
        }
        (AxisType::Date, _) => rebuild(
            xml,
            ax,
            "dateAx",
            DATE_AX,
            AX_ORDER,
            &mut |slot| match slot {
                "auto" => auto(false),
                "lblOffset" if kid(ax, slot).is_none() => {
                    Some(Some(format!("<{p}lblOffset val=\"100\"/>")))
                }
                _ => None,
            },
        ),
    };
    Ok(splice(xml, vec![(ax.range(), text)]))
}

/// Set the options of the horizontal or vertical primary axis. Only the
/// parts that change are rewritten.
pub(crate) fn set_axis_options(xml: &str, horizontal: bool, new: &AxisOptions) -> Result<String> {
    let xml = &set_axis_type(xml, horizontal, new.axis_type)?;
    let doc = Document::parse(xml)?;
    let (_, plot) = plot_area(&doc)?;
    let (h, v) = primary_axes(plot);
    let Some(ax) = (if horizontal { h } else { v }) else {
        bail!("This chart has no axes");
    };
    let old = axis_options(ax);
    if old == *new {
        return Ok(xml.to_owned());
    }
    let value = is_value_axis(ax);
    let date = ax.tag_name().name() == "dateAx";
    let order: &[&str] = match ax.tag_name().name() {
        "valAx" => VAL_AX,
        "catAx" => CAT_AX,
        "dateAx" => DATE_AX,
        _ => AX_ORDER,
    };
    let p = prefix_of(xml, ax);
    let mut edits = Vec::new();
    // c:scaling: orientation for every axis; bounds and log for values.
    let scaling_changed = old.reverse != new.reverse
        || (value && (old.min != new.min || old.max != new.max || old.log_base != new.log_base));
    if scaling_changed {
        let sc = kid(ax, "scaling");
        let mut text = format!("<{p}scaling>");
        for slot in SCALING_ORDER {
            let own = |name: &str| {
                sc.map(|s| {
                    kids_named(s, name)
                        .map(|n| raw(xml, n).to_owned())
                        .collect::<String>()
                })
                .unwrap_or_default()
            };
            let num = |name: &str, v: Option<f64>| {
                v.map(|v| format!("<{p}{name} val=\"{}\"/>", num_text(v)))
                    .unwrap_or_default()
            };
            text.push_str(&match *slot {
                "orientation" => format!(
                    "<{p}orientation val=\"{}\"/>",
                    if new.reverse { "maxMin" } else { "minMax" }
                ),
                "logBase" if value => num("logBase", new.log_base),
                "max" if value => num("max", new.max),
                "min" if value => num("min", new.min),
                other => own(other),
            });
        }
        text.push_str(&format!("</{p}scaling>"));
        edits.push(match sc {
            Some(s) => (s.range(), text),
            None => insert_ordered(xml, ax, "scaling", order, &text),
        });
    }
    if old.num_fmt != new.num_fmt {
        let text = match &new.num_fmt {
            Some(code) => format!(
                "<{p}numFmt formatCode=\"{}\" sourceLinked=\"0\"/>",
                esc(code)
            ),
            None => format!("<{p}numFmt formatCode=\"General\" sourceLinked=\"1\"/>"),
        };
        edits.push(match kid(ax, "numFmt") {
            Some(n) => (n.range(), text),
            None => insert_ordered(xml, ax, "numFmt", order, &text),
        });
    }
    // Units in schema order (insertions at one place keep it).
    let num = |v: Option<f64>| v.map(num_text);
    let time = |v: Option<TimeUnit>| v.map(|u| u.code().to_owned());
    let mut slots = vec![
        ("majorUnit", num(old.major), num(new.major)),
        ("minorUnit", num(old.minor), num(new.minor)),
    ];
    if date {
        slots = vec![
            ("baseTimeUnit", time(old.base_unit), time(new.base_unit)),
            ("majorUnit", num(old.major), num(new.major)),
            ("majorTimeUnit", time(old.major_time), time(new.major_time)),
            ("minorUnit", num(old.minor), num(new.minor)),
            ("minorTimeUnit", time(old.minor_time), time(new.minor_time)),
        ];
    }
    for (name, was, now) in slots {
        if (value || date) && was != now {
            match now {
                Some(v) => edits.push(set_val(xml, ax, name, order, &v)),
                None => {
                    if let Some(n) = kid(ax, name) {
                        edits.push((n.range(), String::new()));
                    }
                }
            }
        }
    }
    if value && old.units != new.units {
        let du = kid(ax, "dispUnits");
        match new.units.code() {
            None if new.units == DisplayUnits::Custom => {}
            None => {
                if let Some(n) = du {
                    edits.push((n.range(), String::new()));
                }
            }
            Some(code) => {
                let unit = format!("<{p}builtInUnit val=\"{code}\"/>");
                let text = match du {
                    // Keep the units label (and its formatting).
                    Some(d) => {
                        let mut t = format!("<{p}dispUnits>");
                        for slot in DISP_UNITS_ORDER {
                            match *slot {
                                "custUnit" => {}
                                "builtInUnit" => t.push_str(&unit),
                                other => {
                                    for n in kids_named(d, other) {
                                        t.push_str(raw(xml, n));
                                    }
                                }
                            }
                        }
                        t.push_str(&format!("</{p}dispUnits>"));
                        t
                    }
                    None => format!("<{p}dispUnits>{unit}<{p}dispUnitsLbl/></{p}dispUnits>"),
                };
                edits.push(match du {
                    Some(d) => (d.range(), text),
                    None => insert_ordered(xml, ax, "dispUnits", order, &text),
                });
            }
        }
    }
    Ok(splice(xml, edits))
}

mod types;
pub(crate) use types::set_type;
pub(crate) mod from_data;

#[cfg(test)]
mod tests;
