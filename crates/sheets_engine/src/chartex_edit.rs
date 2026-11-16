//! Charts of the Office 2016 types (`cx:chartSpace`: waterfall, funnel,
//! treemap, sunburst, histogram, Pareto, box & whisker, map) that came with
//! the file, in the Chart panel: their data range is shown and can be
//! changed; their type, title and other options stay as the file has them.
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

use crate::chart_edit::{ChartInfo, ChartSpec, LegendPos};
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
            .and_then(|t| kid(t, "tx"))
            .and_then(|t| kid(t, "txData"))
            .and_then(|t| kid(t, "v"))
            .and_then(|v| v.text())
            .unwrap_or_default()
            .to_owned();
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
        spec.title = title;
        spec.legend = legend;
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
        if same != *spec {
            bail!(
                "Kantor can change only the data range of a {} chart",
                info.kind_label
            );
        }
        if old.data == spec.data && old.data_sheet == spec.data_sheet && old.by_rows == spec.by_rows
        {
            return Ok(xml);
        }
        if spec.by_rows {
            bail!(
                "The series of a {} chart are its data's columns",
                info.kind_label
            );
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
        self.chartex_with_range(&xml, &name, spec.data)
    }

    /// `xml` with its references laid out over `range` of sheet `sheet`
    /// (quoted as a reference writes it) and its points read from there.
    fn chartex_with_range(&self, xml: &str, sheet: &str, range: CellRange) -> Result<String> {
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
        let first = range.top + u32::from(header);
        if first > range.bottom {
            bail!("The data range needs a row of values under the series' names");
        }
        let cols = range.right - range.left + 1;
        let needed = levels as u32 + list.len() as u32;
        if cols < needed {
            bail!(
                "The data range needs {needed} columns: {levels} of categories and {} of values",
                list.len()
            );
        }
        let cell = |c: u32, r: u32| format!("${}${}", crate::column_name(c), r + 1);
        let column = |c: u32| format!("{sheet}!{}:{}", cell(c, first), cell(c, range.bottom));
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
                let left = range.left;
                let right = left + levels as u32 - 1;
                set_f(
                    cd,
                    &format!(
                        "{sheet}!{}:{}",
                        cell(left, first),
                        cell(right, range.bottom)
                    ),
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
            let col = range.left + levels as u32 + j as u32;
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
                let at = format!("{sheet}!{}", cell(col, range.top));
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
