//! Chart types beyond one 2-D plot: combo charts (Excel's Change Chart Type
//! ▸ Combo: a type and an axis per series) and the 3-D forms of column,
//! bar, line, area and pie charts.
//!
//! [`set_type`] goes through a single 2-D plot: a 3-D chart is made 2-D
//! ([`to_2d`]), a combo's series are gathered into one plot
//! ([`set_combo`] with every series a column on the primary axis), the
//! existing [`set_kind`] converts that plot, then [`set_combo`] regroups the
//! series or [`to_3d`] makes the plot 3-D. Series keep their data, names,
//! labels and formatting; their colours are put back after a regrouping.

use anyhow::{Result, bail};
use roxmltree::{Document, Node};

use super::*;

const BAR3D_ORDER: &[&str] = &[
    "barDir",
    "grouping",
    "varyColors",
    "ser",
    "dLbls",
    "gapWidth",
    "gapDepth",
    "shape",
    "axId",
    "extLst",
];
const LINE3D_ORDER: &[&str] = &[
    "grouping",
    "varyColors",
    "ser",
    "dLbls",
    "dropLines",
    "gapDepth",
    "axId",
    "extLst",
];
const PIE3D_ORDER: &[&str] = &["varyColors", "ser", "dLbls", "extLst"];

/// Children of every plot type, 2-D and 3-D (see `rebuild`).
fn known() -> Vec<&'static str> {
    let mut k = GROUP_ALL.to_vec();
    k.extend(["gapDepth", "shape"]);
    k
}

fn parse(xml: &str) -> Result<Document<'_>> {
    Ok(Document::parse(xml)?)
}

/// Change the chart to `kind` (its 3-D form when `three_d`; for
/// [`ChartType::Combo`], each series' type and axis from `types`).
pub(crate) fn set_type(
    xml: &str,
    kind: ChartType,
    three_d: bool,
    types: &[SeriesType],
) -> Result<String> {
    if kind == ChartType::Other || kind.is_ex() {
        bail!("Kantor can't make this chart type");
    }
    let mut xml = to_2d(xml)?;
    let doc = parse(&xml)?;
    let (_, plot) = plot_area(&doc)?;
    let gs = groups(plot);
    let n = all_series(plot).len();
    let several = gs.len() > 1;
    let combo_ready = gs.iter().all(|g| combo_kind(*g).is_some());
    drop(doc);
    if kind == ChartType::Combo {
        if !combo_ready {
            if several {
                bail!("Kantor can't change this chart's type");
            }
            xml = set_kind(&xml, ChartType::Column)?;
        }
        let mut t: Vec<SeriesType> = (0..n)
            .map(|i| {
                let mut s = types.get(i).copied().unwrap_or_default();
                if !ChartType::COMBO.contains(&s.kind) {
                    s.kind = ChartType::Column;
                }
                s
            })
            .collect();
        // Excel keeps at least one series on the primary axis.
        if t.iter().all(|s| s.secondary) {
            for s in &mut t {
                s.secondary = false;
            }
        }
        return Ok(drop_bad_positions(&set_combo(&xml, &t)?));
    }
    if several {
        if !combo_ready {
            bail!("Kantor can't change this chart's type");
        }
        xml = set_combo(&xml, &vec![SeriesType::default(); n])?;
    }
    xml = set_kind(&xml, kind)?;
    if three_d && kind.has_3d() {
        xml = to_3d(&xml)?;
    }
    Ok(drop_bad_positions(&xml))
}

/// The solid colours of every series (to put back after a regrouping).
fn solid_colors(xml: &str, plot: Node) -> Vec<Option<String>> {
    all_series(plot)
        .into_iter()
        .map(|(s, tag)| {
            color_node(s, tag)
                .filter(|f| is(*f, NS_A, "solidFill"))
                .map(|f| raw(xml, f).to_owned())
        })
        .collect()
}

fn put_colors(mut xml: String, colors: Vec<Option<String>>) -> Result<String> {
    for (i, c) in colors.into_iter().enumerate() {
        if let Some(c) = c {
            xml = set_fill(&xml, i, Some(&NewFill::Raw(c)))?;
        }
    }
    Ok(xml)
}

/// A plot type element for series of type `kind`.
fn group_tag(kind: ChartType) -> &'static str {
    match kind {
        ChartType::Line | ChartType::StackedLine => "lineChart",
        ChartType::Area | ChartType::StackedArea => "areaChart",
        _ => "barChart",
    }
}

/// Regroup the series of a chart whose plots are 2-D columns, lines and
/// areas: series `i` gets `types[i]`. Series of one type on one axis
/// share a plot; plots Kantor finds again keep their settings (gap width,
/// labels, drop lines...). The secondary axes are made when first needed
/// and removed when no series is on them.
pub(crate) fn set_combo(xml: &str, types: &[SeriesType]) -> Result<String> {
    let doc = parse(xml)?;
    let (_, plot) = plot_area(&doc)?;
    let gs = groups(plot);
    let Some(&first) = gs.first() else {
        bail!("This chart has no series");
    };
    if !gs.iter().all(|g| combo_kind(*g).is_some()) {
        bail!("Kantor can't change this chart's type");
    }
    let p = prefix_of(xml, first);
    let sers = all_series(plot);
    let colors = solid_colors(xml, plot);
    let primary = ax_ids(first);
    if primary.len() < 2 {
        bail!("This chart has no axes");
    }
    let all_axes = axes(plot);
    let ids_in_use: Vec<u64> = all_axes
        .iter()
        .filter_map(|a| val(*a, "axId")?.parse().ok())
        .collect();
    // The secondary axes: an existing plot's other pair, else new ones.
    let old_secondary: Option<Vec<String>> = gs
        .iter()
        .map(|g| ax_ids(*g))
        .find(|ids| *ids != primary && ids.len() == 2);
    let wants_secondary = types.iter().any(|t| t.secondary);
    let mut new_axes = String::new();
    let secondary: Vec<String> = match &old_secondary {
        Some(ids) => ids.clone(),
        None => {
            let base = ids_in_use.iter().copied().max().unwrap_or(500_000_002) + 1;
            let (cat, valu) = (base, base + 1);
            if wants_secondary {
                let tail = "<c:majorTickMark val=\"out\"/><c:minorTickMark val=\"none\"/><c:tickLblPos val=\"nextTo\"/>";
                new_axes = prefixed(
                    &format!(
                        "<c:valAx><c:axId val=\"{valu}\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"r\"/><c:numFmt formatCode=\"General\" sourceLinked=\"1\"/>{tail}<c:crossAx val=\"{cat}\"/><c:crosses val=\"max\"/><c:crossBetween val=\"between\"/></c:valAx>\
<c:catAx><c:axId val=\"{cat}\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"1\"/><c:axPos val=\"b\"/><c:numFmt formatCode=\"General\" sourceLinked=\"1\"/>{tail}<c:crossAx val=\"{valu}\"/><c:crosses val=\"autoZero\"/><c:auto val=\"1\"/><c:lblAlgn val=\"ctr\"/><c:lblOffset val=\"100\"/><c:noMultiLvlLbl val=\"0\"/></c:catAx>"
                    ),
                    &p,
                    "a:",
                );
            }
            vec![cat.to_string(), valu.to_string()]
        }
    };
    // The new plots: (type, secondary) in drawing order.
    let mut keys: Vec<(ChartType, bool)> = Vec::new();
    for (i, _) in sers.iter().enumerate() {
        let t = types.get(i).copied().unwrap_or_default();
        if !keys.contains(&(t.kind, t.secondary)) {
            keys.push((t.kind, t.secondary));
        }
    }
    let rank = |k: ChartType| match group_tag(k) {
        "areaChart" => 0,
        "barChart" => 1,
        _ => 2,
    };
    keys.sort_by_key(|(k, s)| (*s, rank(*k), ChartType::COMBO.iter().position(|c| c == k)));
    let mut text = String::new();
    for (kind, sec) in &keys {
        let tag = group_tag(*kind);
        let stacked = kind.stacked();
        let ids = if *sec { &secondary } else { &primary };
        let members: String = sers
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                let t = types.get(*i).copied().unwrap_or_default();
                t.kind == *kind && t.secondary == *sec
            })
            .map(|(_, (s, from))| convert_ser(xml, *s, from, tag))
            .collect();
        let axis = ids
            .iter()
            .map(|i| format!("<{p}axId val=\"{i}\"/>"))
            .collect::<String>();
        let old = gs
            .iter()
            .copied()
            .find(|g| combo_kind(*g) == Some(*kind) && ax_ids(*g) == *ids);
        let group = match old {
            Some(g) => {
                let mut over = |slot: &str| -> Option<Option<String>> {
                    match slot {
                        "ser" => Some(Some(members.clone())),
                        "axId" => Some(Some(axis.clone())),
                        _ => None,
                    }
                };
                rebuild(xml, g, tag, group_order(tag), GROUP_ALL, &mut over)
            }
            None => {
                let grouping = match (tag, stacked) {
                    (_, true) => "stacked",
                    ("barChart", false) => "clustered",
                    _ => "standard",
                };
                match tag {
                    "barChart" => format!(
                        "<{p}barChart><{p}barDir val=\"col\"/><{p}grouping val=\"{grouping}\"/><{p}varyColors val=\"0\"/>{members}<{p}gapWidth val=\"150\"/>{}{axis}</{p}barChart>",
                        if stacked {
                            format!("<{p}overlap val=\"100\"/>")
                        } else {
                            String::new()
                        }
                    ),
                    "lineChart" => format!(
                        "<{p}lineChart><{p}grouping val=\"{grouping}\"/><{p}varyColors val=\"0\"/>{members}<{p}marker val=\"1\"/>{axis}</{p}lineChart>"
                    ),
                    _ => format!(
                        "<{p}areaChart><{p}grouping val=\"{grouping}\"/><{p}varyColors val=\"0\"/>{members}{axis}</{p}areaChart>"
                    ),
                }
            }
        };
        text.push_str(&group);
    }
    let mut edits: Vec<Edit> = vec![(first.range(), text)];
    for g in gs.iter().skip(1) {
        edits.push((g.range(), String::new()));
    }
    if wants_secondary {
        if !new_axes.is_empty() {
            let at = all_axes.last().map_or(first.range().end, |a| a.range().end);
            edits.push((at..at, new_axes));
        }
    } else {
        // Axes no plot uses any more.
        for a in &all_axes {
            if val(*a, "axId").is_some_and(|id| !primary.iter().any(|p| p == id)) {
                edits.push((a.range(), String::new()));
            }
        }
    }
    let out = splice(xml, edits);
    put_colors(out, colors)
}

/// The 3-D form of a chart with one 2-D column, bar, line, area or pie
/// plot. Lines and (unstacked) areas get a series (depth) axis, as in
/// Excel; the chart gets Excel's default rotation.
pub(crate) fn to_3d(xml: &str) -> Result<String> {
    let doc = parse(xml)?;
    let (chart, plot) = plot_area(&doc)?;
    let gs = groups(plot);
    if gs.len() != 1 {
        bail!("Kantor can't make this chart 3-D");
    }
    let g = gs[0];
    let p = prefix_of(xml, g);
    let tag = g.tag_name().name();
    let (new_tag, order, depth_axis, pie) = match tag {
        "barChart" => ("bar3DChart", BAR3D_ORDER, false, false),
        "lineChart" => ("line3DChart", LINE3D_ORDER, true, false),
        "areaChart" => ("area3DChart", LINE3D_ORDER, grouping(g).is_empty(), false),
        "pieChart" => ("pie3DChart", PIE3D_ORDER, false, true),
        _ => bail!("Kantor can't make this chart 3-D"),
    };
    let ids = ax_ids(g);
    let ser_id = axes(plot)
        .iter()
        .filter_map(|a| val(*a, "axId")?.parse::<u64>().ok())
        .max()
        .unwrap_or(500_000_002)
        + 1;
    let val_id = axes(plot)
        .into_iter()
        .find(|a| {
            a.tag_name().name() == "valAx"
                && val(*a, "axId").is_some_and(|i| ids.iter().any(|x| x == i))
        })
        .and_then(|a| val(a, "axId").map(str::to_owned));
    let add_ser_ax = depth_axis && val_id.is_some();
    let mut over = |slot: &str| -> Option<Option<String>> {
        match slot {
            "shape" if tag == "barChart" => Some(Some(format!("<{p}shape val=\"box\"/>"))),
            "axId" if add_ser_ax => Some(Some(
                ids.iter()
                    .map(|i| format!("<{p}axId val=\"{i}\"/>"))
                    .chain(std::iter::once(format!("<{p}axId val=\"{ser_id}\"/>")))
                    .collect(),
            )),
            _ => None,
        }
    };
    let k = known();
    let group = rebuild(xml, g, new_tag, order, &k, &mut over);
    let mut edits: Vec<Edit> = vec![(g.range(), group)];
    if add_ser_ax {
        let val_id = val_id.expect("checked");
        let text = prefixed(
            &format!(
                "<c:serAx><c:axId val=\"{ser_id}\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"b\"/><c:majorTickMark val=\"out\"/><c:minorTickMark val=\"none\"/><c:tickLblPos val=\"nextTo\"/><c:crossAx val=\"{val_id}\"/><c:crosses val=\"autoZero\"/></c:serAx>"
            ),
            &p,
            "a:",
        );
        let at = axes(plot).last().map_or(g.range().end, |a| a.range().end);
        edits.push((at..at, text));
    }
    if kid(chart, "view3D").is_none() {
        let view = if pie {
            "<c:view3D><c:rotX val=\"30\"/><c:rotY val=\"0\"/><c:rAngAx val=\"0\"/></c:view3D>"
        } else {
            "<c:view3D><c:rotX val=\"15\"/><c:rotY val=\"20\"/><c:rAngAx val=\"1\"/></c:view3D>"
        };
        edits.push(insert_ordered(
            xml,
            chart,
            "view3D",
            CHART_ORDER,
            &prefixed(view, &p, "a:"),
        ));
    }
    Ok(splice(xml, edits))
}

/// The 2-D form of a 3-D column, bar, line, area or pie chart (else `xml`
/// as it is): the series axis, rotation, floor and walls go.
pub(crate) fn to_2d(xml: &str) -> Result<String> {
    let doc = parse(xml)?;
    let (chart, plot) = plot_area(&doc)?;
    let gs = groups(plot);
    let Some(g) = gs.first().copied().filter(|_| gs.len() == 1) else {
        return Ok(xml.to_owned());
    };
    let tag = g.tag_name().name();
    let new_tag = match tag {
        "bar3DChart" => "barChart",
        "line3DChart" => "lineChart",
        "area3DChart" => "areaChart",
        "pie3DChart" => "pieChart",
        _ => return Ok(xml.to_owned()),
    };
    let p = prefix_of(xml, g);
    let plot_axes = axes(plot);
    let ser_axes: Vec<Node> = plot_axes
        .iter()
        .copied()
        .filter(|a| a.tag_name().name() == "serAx")
        .collect();
    let keep_ids: Vec<String> = ax_ids(g)
        .into_iter()
        .filter(|i| !ser_axes.iter().any(|a| val(*a, "axId") == Some(i.as_str())))
        .collect();
    let stacked = grouping(g) == "stacked";
    let mut over = |slot: &str| -> Option<Option<String>> {
        match slot {
            "grouping" if new_tag == "barChart" && grouping(g).is_empty() => {
                Some(Some(format!("<{p}grouping val=\"clustered\"/>")))
            }
            "overlap" if new_tag == "barChart" && stacked => {
                Some(Some(format!("<{p}overlap val=\"100\"/>")))
            }
            "marker" if new_tag == "lineChart" => Some(Some(format!("<{p}marker val=\"1\"/>"))),
            "firstSliceAng" if new_tag == "pieChart" => {
                Some(Some(format!("<{p}firstSliceAng val=\"0\"/>")))
            }
            "axId" => Some(Some(
                keep_ids
                    .iter()
                    .map(|i| format!("<{p}axId val=\"{i}\"/>"))
                    .collect(),
            )),
            _ => None,
        }
    };
    let k = known();
    let group = rebuild(xml, g, new_tag, group_order(new_tag), &k, &mut over);
    let mut edits: Vec<Edit> = vec![(g.range(), group)];
    for a in ser_axes {
        edits.push((a.range(), String::new()));
    }
    for name in ["view3D", "floor", "sideWall", "backWall"] {
        if let Some(n) = kid(chart, name) {
            edits.push((n.range(), String::new()));
        }
    }
    Ok(splice(xml, edits))
}
