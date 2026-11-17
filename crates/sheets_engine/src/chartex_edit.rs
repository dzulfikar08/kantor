//! Charts of the Office 2016 types (`cx:chartSpace`: waterfall, funnel,
//! treemap, sunburst, histogram, Pareto, box & whisker, map) that came with
//! the file, in the Chart panel: their type (among these, but the map),
//! title, legend, data range and whether series are the data's rows or
//! columns are shown and can be changed; other options stay as the file
//! has them.
//!
//! A chartex part keeps one `cx:data` block per series (`cx:series/
//! cx:dataId`), each with a category dimension (`cx:strDim type="cat"`,
//! one `cx:lvl` per hierarchy level, innermost first) and a value dimension
//! (`cx:numDim type="val"`). Their references (`cx:f`) name cells, or a
//! hidden defined name standing for them (`_xlchart.v1.0`, as Excel writes
//! them). A new range is laid out as Excel lays one out for these charts:
//! the series' names in its first row (when the chart had names from
//! cells), then the category columns (as many as the chart has levels),
//! then one column of values per data block. The references are written as
//! cells and the cached points refilled from them.

use std::ops::Range;

use anyhow::{Result, anyhow, bail};
use roxmltree::{Document, Node};

use crate::chart_edit::{ChartInfo, ChartSpec, ChartType, LegendPos};
use crate::drawing::{DrawingKey, RefValue};
use crate::{CellRange, Workbook};

const NS_CX: &str = "http://schemas.microsoft.com/office/drawing/2014/chartex";

/// Whether `xml` is a chartex part.
pub(crate) fn is_chartex(xml: &str) -> bool {
    let head = &xml[..xml.len().min(600)];
    head.contains("chartSpace") && head.contains(NS_CX)
}

fn is(n: Node, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some(NS_CX)
}

fn kid<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| is(*c, name))
}

/// The `prefix:` an element is written with.
fn prefix_of(src: &str, n: Node) -> String {
    let name: String = src[n.range().start + 1..]
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '>' && *c != '/')
        .collect();
    match name.split_once(':') {
        Some((p, _)) => format!("{p}:"),
        None => String::new(),
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// The data blocks in order, each with the series using it.
fn blocks<'a, 'i>(doc: &'a Document<'i>) -> Vec<(Node<'a, 'i>, Vec<Node<'a, 'i>>)> {
    let series: Vec<Node> = doc.descendants().filter(|n| is(*n, "series")).collect();
    doc.descendants()
        .filter(|n| is(*n, "data"))
        .map(|d| {
            let id = d.attribute("id");
            let owners = series
                .iter()
                .filter(|s| kid(**s, "dataId").and_then(|i| i.attribute("val")) == id)
                .copied()
                .collect();
            (d, owners)
        })
        .filter(|(_, owners): &(Node, Vec<Node>)| !owners.is_empty())
        .collect()
}

fn dim<'a, 'i>(data: Node<'a, 'i>, kind: &str) -> Option<Node<'a, 'i>> {
    data.children()
        .find(|c| (is(*c, "strDim") || is(*c, "numDim")) && c.attribute("type") == Some(kind))
}

fn tx_data<'a, 'i>(series: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    kid(series, "tx").and_then(|t| kid(t, "txData"))
}

/// The type's name, from its series' layout.
fn kind_label(doc: &Document) -> &'static str {
    let layouts: Vec<&str> = doc
        .descendants()
        .filter(|n| is(*n, "series"))
        .filter_map(|s| s.attribute("layoutId"))
        .collect();
    match layouts.first().copied().unwrap_or_default() {
        "waterfall" => "Waterfall",
        "funnel" => "Funnel",
        "treemap" => "Treemap",
        "sunburst" => "Sunburst",
        "boxWhisker" => "Box & Whisker",
        "regionMap" => "Map",
        "clusteredColumn" if layouts.contains(&"paretoLine") => "Pareto",
        "clusteredColumn" => "Histogram",
        _ => "Office 2016",
    }
}

/// The type as the Chart panel names it (`Other`: a map).
fn ex_kind(doc: &Document) -> ChartType {
    match kind_label(doc) {
        "Waterfall" => ChartType::Waterfall,
        "Funnel" => ChartType::Funnel,
        "Treemap" => ChartType::Treemap,
        "Sunburst" => ChartType::Sunburst,
        "Box & Whisker" => ChartType::BoxWhisker,
        "Pareto" => ChartType::Pareto,
        "Histogram" => ChartType::Histogram,
        _ => ChartType::Other,
    }
}

fn layout_id(k: ChartType) -> Option<&'static str> {
    Some(match k {
        ChartType::Waterfall => "waterfall",
        ChartType::Funnel => "funnel",
        ChartType::Treemap => "treemap",
        ChartType::Sunburst => "sunburst",
        ChartType::Histogram | ChartType::Pareto => "clusteredColumn",
        ChartType::BoxWhisker => "boxWhisker",
        _ => return None,
    })
}

/// A chartex title's text: its cached cell text or rich text.
fn ex_title(t: Node) -> String {
    let Some(tx) = kid(t, "tx") else {
        return String::new();
    };
    if let Some(v) = kid(tx, "txData").and_then(|d| kid(d, "v")) {
        return v.text().unwrap_or_default().to_owned();
    }
    const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
    kid(tx, "rich")
        .map(|r| {
            r.children()
                .filter(|p| p.tag_name().name() == "p" && p.tag_name().namespace() == Some(NS_A))
                .map(|p| {
                    p.descendants()
                        .filter(|d| {
                            d.tag_name().name() == "t" && d.tag_name().namespace() == Some(NS_A)
                        })
                        .filter_map(|d| d.text())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn splice(xml: &str, mut edits: Vec<(Range<usize>, String)>) -> String {
    edits.sort_by_key(|(r, _)| std::cmp::Reverse((r.start, r.end)));
    let mut out = xml.to_owned();
    for (r, t) in edits {
        out.replace_range(r, &t);
    }
    out
}

fn chart_node<'a, 'i>(doc: &'a Document<'i>) -> Result<Node<'a, 'i>> {
    doc.root_element()
        .children()
        .find(|c| is(*c, "chart"))
        .ok_or_else(|| anyhow!("no cx:chart"))
}

/// Set (or remove, when empty) a chartex chart's title. A title from a
/// cell becomes typed text, as in Excel; its formatting stays.
pub(crate) fn set_ex_title(xml: &str, text: &str) -> Result<String> {
    let doc = Document::parse(xml)?;
    let chart = chart_node(&doc)?;
    let p = prefix_of(xml, chart);
    let text = text.trim();
    let tx = format!(
        "<{p}tx><{p}txData><{p}v>{}</{p}v></{p}txData></{p}tx>",
        esc(text)
    );
    let edit = match kid(chart, "title") {
        Some(t) if text.is_empty() => (t.range(), String::new()),
        Some(t) => match kid(t, "tx") {
            Some(old) => (old.range(), tx),
            None => {
                let open = &xml[t.range()];
                if open.ends_with("/>") {
                    let head = open.trim_end_matches("/>").trim_end();
                    (t.range(), format!("{head}>{tx}</{p}title>"))
                } else {
                    let at = t.range().start + open.find('>').map_or(0, |i| i + 1);
                    (at..at, tx)
                }
            }
        },
        None if text.is_empty() => return Ok(xml.to_owned()),
        None => {
            let at = chart
                .children()
                .find(|c| c.is_element())
                .map_or(chart.range().end, |c| c.range().start);
            (
                at..at,
                format!("<{p}title pos=\"t\" align=\"ctr\" overlay=\"0\">{tx}</{p}title>"),
            )
        }
    };
    Ok(splice(xml, vec![edit]))
}

/// Set a chartex chart's legend position (`None`: no legend).
pub(crate) fn set_ex_legend(xml: &str, pos: LegendPos) -> Result<String> {
    let doc = Document::parse(xml)?;
    let chart = chart_node(&doc)?;
    let p = prefix_of(xml, chart);
    let code = match pos {
        LegendPos::None => None,
        LegendPos::Right => Some("r"),
        LegendPos::Top => Some("t"),
        LegendPos::Bottom => Some("b"),
        LegendPos::Left => Some("l"),
    };
    let old = kid(chart, "legend");
    let edit = match (old, code) {
        (None, None) => return Ok(xml.to_owned()),
        (Some(l), None) => (l.range(), String::new()),
        (Some(l), Some(c)) => match l.attributes().find(|a| a.name() == "pos") {
            Some(a) => (a.range_value(), c.to_owned()),
            None => {
                let at = l.range().start + 1 + format!("{p}legend").len();
                (at..at, format!(" pos=\"{c}\""))
            }
        },
        (None, Some(c)) => {
            let text = format!("<{p}legend pos=\"{c}\" align=\"ctr\" overlay=\"0\"/>");
            match kid(chart, "plotArea") {
                Some(pa) => (pa.range().end..pa.range().end, text),
                None => {
                    let at = kid(chart, "extLst").map_or_else(
                        || {
                            xml[..chart.range().end]
                                .rfind("</")
                                .unwrap_or(chart.range().end)
                        },
                        |e| e.range().start,
                    );
                    (at..at, text)
                }
            }
        }
    };
    Ok(splice(xml, vec![edit]))
}

/// Change a chartex chart to another of Excel 2016's types: each series'
/// layout and its type's default layout options (binning, quartiles...),
/// the Pareto line, and the axes the type has (axes that fit are kept).
pub(crate) fn set_ex_kind(xml: &str, to: ChartType) -> Result<String> {
    let Some(layout) = layout_id(to) else {
        bail!("Kantor can't change the chart to that type");
    };
    let doc = Document::parse(xml)?;
    let chart = chart_node(&doc)?;
    let plot = kid(chart, "plotArea").ok_or_else(|| anyhow!("no cx:plotArea"))?;
    let region = kid(plot, "plotAreaRegion").ok_or_else(|| anyhow!("no cx:plotAreaRegion"))?;
    if ex_kind(&doc) == ChartType::Other {
        bail!("Kantor can't change the type of this chart");
    }
    let p = prefix_of(xml, plot);
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let all: Vec<Node> = region.children().filter(|c| is(*c, "series")).collect();
    let mut owner: Option<usize> = None;
    for (i, s) in all.iter().enumerate() {
        if s.attribute("layoutId") == Some("paretoLine") {
            edits.push((s.range(), String::new()));
            continue;
        }
        owner.get_or_insert(i);
        if let Some(a) = s.attributes().find(|a| a.name() == "layoutId") {
            edits.push((a.range_value(), layout.to_owned()));
        }
        for c in s
            .children()
            .filter(|c| is(*c, "layoutPr") || is(*c, "axisId"))
        {
            edits.push((c.range(), String::new()));
        }
        let extra = match to {
            ChartType::Waterfall => {
                format!("<{p}layoutPr><{p}visibility connectorLines=\"1\"/></{p}layoutPr>")
            }
            ChartType::Treemap => {
                format!("<{p}layoutPr><{p}parentLabelLayout val=\"overlapping\"/></{p}layoutPr>")
            }
            ChartType::Histogram => {
                format!("<{p}layoutPr><{p}binning intervalClosed=\"r\"/></{p}layoutPr>")
            }
            ChartType::Pareto => {
                format!("<{p}layoutPr><{p}aggregation/></{p}layoutPr><{p}axisId>1</{p}axisId>")
            }
            ChartType::BoxWhisker => format!(
                "<{p}layoutPr><{p}visibility meanLine=\"0\" meanMarker=\"1\" nonoutliers=\"0\" outliers=\"1\"/><{p}statistics quartileMethod=\"exclusive\"/></{p}layoutPr>"
            ),
            _ => String::new(),
        };
        if !extra.is_empty() {
            // After cx:dataId (CT_Series: ... dataLabels, dataId, layoutPr,
            // axisId, extLst).
            let at = match kid(*s, "dataId") {
                Some(d) => d.range().end,
                None => kid(*s, "extLst").map_or_else(
                    || xml[..s.range().end].rfind("</").unwrap_or(s.range().end),
                    |e| e.range().start,
                ),
            };
            edits.push((at..at, extra));
        }
    }
    let Some(owner) = owner else {
        bail!("The chart has no series");
    };
    if to == ChartType::Pareto {
        let at = all[owner].range().end;
        edits.push((
            at..at,
            format!(
                "<{p}series layoutId=\"paretoLine\" ownerIdx=\"{owner}\"><{p}axisId>2</{p}axisId></{p}series>"
            ),
        ));
    }
    // The axes: (id, value axis, default markup).
    let cat = |gap: &str| {
        format!("<{p}axis id=\"0\"><{p}catScaling gapWidth=\"{gap}\"/><{p}tickLabels/></{p}axis>")
    };
    let valu =
        format!("<{p}axis id=\"1\"><{p}valScaling/><{p}majorGridlines/><{p}tickLabels/></{p}axis>");
    let want: Vec<(u32, bool, String)> = match to {
        ChartType::Waterfall | ChartType::BoxWhisker => {
            vec![(0, false, cat("0.5")), (1, true, valu)]
        }
        ChartType::Histogram => vec![(0, false, cat("0")), (1, true, valu)],
        ChartType::Pareto => vec![
            (0, false, cat("0")),
            (1, true, valu),
            (
                2,
                true,
                format!(
                    "<{p}axis id=\"2\"><{p}valScaling max=\"1\" min=\"0\"/><{p}units unit=\"percentage\"/><{p}tickLabels/></{p}axis>"
                ),
            ),
        ],
        ChartType::Funnel => vec![(0, false, cat("0.06"))],
        _ => Vec::new(),
    };
    let old_axes: Vec<Node> = plot.children().filter(|c| is(*c, "axis")).collect();
    let mut axes = String::new();
    for (id, value, text) in &want {
        let keep = old_axes.iter().find(|a| {
            a.attribute("id") == Some(id.to_string().as_str())
                && kid(**a, if *value { "valScaling" } else { "catScaling" }).is_some()
                && (*id != 2 || kid(**a, "units").is_some())
        });
        axes.push_str(match keep {
            Some(a) => &xml[a.range()],
            None => text,
        });
    }
    for a in &old_axes {
        edits.push((a.range(), String::new()));
    }
    let at = region.range().end;
    edits.push((at..at, axes));
    Ok(splice(xml, edits))
}

impl Workbook {
    /// A chartex reference as cells: a defined name (`_xlchart.v1.0`)
    /// stands for its formula.
    fn chartex_cells(&self, text: &str) -> String {
        let t = text.trim();
        let model = self.model.get_model();
        model
            .workbook
            .defined_names
            .iter()
            .filter(|d| d.name.eq_ignore_ascii_case(t))
            .min_by_key(|d| d.sheet_id.is_some())
            .map(|d| d.formula.trim_start_matches('=').to_owned())
            .unwrap_or_else(|| t.to_owned())
    }

    /// What the Chart panel shows for chartex part `xml` of chart `key`.
    pub(crate) fn chartex_info(&self, key: DrawingKey, xml: &str) -> Option<ChartInfo> {
        let doc = Document::parse(xml).ok()?;
        let list = blocks(&doc);
        let mut sheet: Option<String> = None;
        let mut area: Option<CellRange> = None;
        let mut unknown = list.is_empty();
        let refs = list.iter().flat_map(|(d, owners)| {
            [dim(*d, "cat"), dim(*d, "val")]
                .into_iter()
                .flatten()
                .chain(owners.iter().filter_map(|s| tx_data(*s)))
                .filter_map(|n| kid(n, "f"))
                .collect::<Vec<_>>()
        });
        for f in refs {
            let cells = self.chartex_cells(f.text().unwrap_or_default());
            let tokens = crate::shift::references(&cells);
            if tokens.len() != 1 {
                unknown = true;
            }
            for t in tokens {
                let Some(s) = t.sheet.clone() else {
                    unknown = true;
                    continue;
                };
                match &sheet {
                    Some(s0) if !s0.eq_ignore_ascii_case(&s) => unknown = true,
                    Some(_) => {}
                    None => sheet = Some(s),
                }
                let a = t.range.area();
                let r = CellRange::from_corners((a.top, a.left), (a.bottom, a.right));
                area = Some(match area {
                    None => r,
                    Some(b) => CellRange::from_corners(
                        (b.top.min(r.top), b.left.min(r.left)),
                        (b.bottom.max(r.bottom), b.right.max(r.right)),
                    ),
                });
            }
        }
        let data_sheet = sheet.as_deref().and_then(|s| {
            let i = self
                .sheet_names()
                .iter()
                .position(|n| n.eq_ignore_ascii_case(s))?;
            Some(self.sheet_id(i as u32))
        });
        if data_sheet.is_none() {
            unknown = true;
        }
        let chart = doc.descendants().find(|n| is(*n, "chart"));
        let title = chart
            .and_then(|c| kid(c, "title"))
            .map(ex_title)
            .unwrap_or_default();
        // Series in rows: the first values run along a row.
        let by_rows = list
            .first()
            .and_then(|(d, _)| dim(*d, "val"))
            .and_then(|v| kid(v, "f"))
            .map(|f| crate::shift::references(&self.chartex_cells(f.text().unwrap_or_default())))
            .and_then(|t| t.into_iter().next())
            .is_some_and(|t| {
                let a = t.range.area();
                a.top == a.bottom && a.left != a.right
            });
        let legend = match chart.and_then(|c| kid(c, "legend")) {
            None => LegendPos::None,
            Some(l) => match l.attribute("pos") {
                Some("t") => LegendPos::Top,
                Some("b") => LegendPos::Bottom,
                Some("l") => LegendPos::Left,
                _ => LegendPos::Right,
            },
        };
        let series = list
            .iter()
            .flat_map(|(_, owners)| owners.iter())
            .map(|s| {
                tx_data(*s)
                    .and_then(|t| kid(t, "v"))
                    .and_then(|v| v.text())
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect();
        let mut spec = ChartSpec::placeholder();
        spec.kind = ex_kind(&doc);
        spec.title = title;
        spec.legend = legend;
        spec.by_rows = by_rows;
        spec.data = area.unwrap_or_else(|| CellRange::from_corners((0, 0), (0, 0)));
        spec.data_sheet = data_sheet.filter(|id| *id != key.sheet_id);
        Some(ChartInfo {
            from_file: true,
            kind_label: kind_label(&doc).to_owned(),
            fixed: false,
            data_only: true,
            data_unknown: unknown,
            has_axes: false,
            series,
            label_positions: Vec::new(),
            percent_labels: false,
            x_value: false,
            y_value: false,
            spec,
        })
    }

    /// Chartex part `xml` (shown in the panel as `info`) with the data
    /// range of `spec`; anything else changed is refused.
    pub(crate) fn chartex_restyled(
        &self,
        key: DrawingKey,
        xml: String,
        info: &ChartInfo,
        spec: &ChartSpec,
    ) -> Result<String> {
        let old = &info.spec;
        let mut same = old.clone();
        same.data = spec.data;
        same.data_sheet = spec.data_sheet;
        same.by_rows = spec.by_rows;
        same.title = spec.title.clone();
        same.legend = spec.legend;
        same.kind = spec.kind;
        if same != *spec {
            bail!("Kantor can't change that on a {} chart", info.kind_label);
        }
        let mut xml = xml;
        if spec.kind != old.kind {
            if old.kind == ChartType::Other || !spec.kind.is_ex() {
                bail!(
                    "Kantor can't change the type of a {} chart",
                    info.kind_label
                );
            }
            xml = set_ex_kind(&xml, spec.kind)?;
        }
        if spec.title != old.title {
            xml = set_ex_title(&xml, &spec.title)?;
        }
        if spec.legend != old.legend {
            xml = set_ex_legend(&xml, spec.legend)?;
        }
        if old.data == spec.data && old.data_sheet == spec.data_sheet && old.by_rows == spec.by_rows
        {
            return Ok(xml);
        }
        let sheet = match spec.data_sheet {
            Some(id) => self
                .sheet_index_of(id)
                .ok_or_else(|| anyhow!("the data's sheet is gone"))?,
            None => match crate::drawing::chart_sheet_of_key(key.sheet_id) {
                Some(_) => 0,
                None => self
                    .sheet_index_of(key.sheet_id)
                    .ok_or_else(|| anyhow!("no sheet"))?,
            },
        };
        let name = crate::quote_sheet_name(&self.sheet_names()[sheet as usize]);
        self.chartex_with_range(&xml, &name, spec.data, spec.by_rows)
    }

    /// `xml` with its references laid out over `range` of sheet `sheet`
    /// (quoted as a reference writes it) and its points read from there.
    fn chartex_with_range(
        &self,
        xml: &str,
        sheet: &str,
        range: CellRange,
        by_rows: bool,
    ) -> Result<String> {
        let doc = Document::parse(xml).map_err(|e| anyhow!("chart XML: {e}"))?;
        let list = blocks(&doc);
        if list.is_empty() {
            bail!("The chart has no data Kantor can change");
        }
        let levels = dim(list[0].0, "cat")
            .map(|d| d.children().filter(|c| is(*c, "lvl")).count().max(1))
            .unwrap_or(0);
        let header = list
            .iter()
            .flat_map(|(_, o)| o.iter())
            .any(|s| tx_data(*s).and_then(|t| kid(t, "f")).is_some());
        // Lines (columns, or rows when series are in rows) across, points
        // along them.
        let (across0, across1, along0, along1) = if by_rows {
            (range.top, range.bottom, range.left, range.right)
        } else {
            (range.left, range.right, range.top, range.bottom)
        };
        let first = along0 + u32::from(header);
        if first > along1 {
            bail!(if by_rows {
                "The data range needs a column of values beside the series' names"
            } else {
                "The data range needs a row of values under the series' names"
            });
        }
        let lines = across1 - across0 + 1;
        let needed = levels as u32 + list.len() as u32;
        if lines < needed {
            let what = if by_rows { "rows" } else { "columns" };
            bail!(
                "The data range needs {needed} {what}: {levels} of categories and {} of values",
                list.len()
            );
        }
        // (line, point) → cell text.
        let cell = |line: u32, at: u32| {
            let (c, r) = if by_rows { (at, line) } else { (line, at) };
            format!("${}${}", crate::column_name(c), r + 1)
        };
        let column = |line: u32| format!("{sheet}!{}:{}", cell(line, first), cell(line, along1));
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        let set_f = |n: Node, text: &str, edits: &mut Vec<(Range<usize>, String)>| {
            if let Some(t) = kid(n, "f")
                .and_then(|f| f.first_child())
                .filter(|t| t.is_text())
            {
                edits.push((t.range(), esc(text)));
            }
        };
        let lvl = |n: Node, values: &[Option<String>]| -> String {
            let p = prefix_of(xml, n);
            // Its attributes but ptCount.
            let attrs: String = n
                .attributes()
                .filter(|a| a.name() != "ptCount" && a.namespace().is_none())
                .map(|a| {
                    format!(
                        " {}=\"{}\"",
                        a.name(),
                        esc(a.value()).replace('"', "&quot;")
                    )
                })
                .collect();
            let mut out = format!("<{p}lvl ptCount=\"{}\"{attrs}>", values.len());
            for (i, v) in values.iter().enumerate() {
                if let Some(v) = v {
                    out.push_str(&format!("<{p}pt idx=\"{i}\">{}</{p}pt>", esc(v)));
                }
            }
            out.push_str(&format!("</{p}lvl>"));
            out
        };
        for (j, (d, owners)) in list.iter().enumerate() {
            if let Some(cd) = dim(*d, "cat") {
                let numeric = is(cd, "numDim");
                let left = across0;
                let right = left + levels as u32 - 1;
                set_f(
                    cd,
                    &format!("{sheet}!{}:{}", cell(left, first), cell(right, along1)),
                    &mut edits,
                );
                for (l, n) in cd.children().filter(|c| is(*c, "lvl")).enumerate() {
                    // Level 0 is the innermost: the rightmost column.
                    let col = right.saturating_sub(l as u32).max(left);
                    let r = self.chart_reference_now(&column(col)).unwrap_or_default();
                    let values: Vec<Option<String>> = r
                        .values
                        .iter()
                        .zip(&r.shown)
                        .map(|(v, shown)| match v {
                            RefValue::Number(x) if numeric => Some(fmt_num(*x)),
                            RefValue::Empty if numeric => None,
                            _ => Some(shown.clone()),
                        })
                        .collect();
                    edits.push((n.range(), lvl(n, &values)));
                }
            }
            let col = across0 + levels as u32 + j as u32;
            if let Some(vd) = dim(*d, "val") {
                set_f(vd, &column(col), &mut edits);
                let r = self.chart_reference_now(&column(col)).unwrap_or_default();
                let values: Vec<Option<String>> = r
                    .values
                    .iter()
                    .map(|v| match v {
                        RefValue::Number(x) => Some(fmt_num(*x)),
                        _ => None,
                    })
                    .collect();
                if let Some(n) = vd.children().find(|c| is(*c, "lvl")) {
                    edits.push((n.range(), lvl(n, &values)));
                }
            }
            if header {
                let at = format!("{sheet}!{}", cell(col, along0));
                let name = self
                    .chart_reference_now(&at)
                    .and_then(|r| r.shown.first().cloned())
                    .unwrap_or_default();
                for s in owners {
                    let Some(td) = tx_data(*s) else { continue };
                    set_f(td, &at, &mut edits);
                    if let Some(v) = kid(td, "v") {
                        match v.first_child().filter(|t| t.is_text()) {
                            Some(t) => edits.push((t.range(), esc(&name))),
                            None => {
                                let p = prefix_of(xml, v);
                                edits.push((v.range(), format!("<{p}v>{}</{p}v>", esc(&name))));
                            }
                        }
                    }
                }
            }
        }
        edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
        let mut out = xml.to_owned();
        for (r, t) in edits {
            out.replace_range(r, &t);
        }
        Ok(out)
    }
}
