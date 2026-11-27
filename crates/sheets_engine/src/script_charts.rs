//! What macros need for charts (`ChartObjects`, `Chart`, `Series` and
//! `Charts` of the Sheets object model, `crates/sheets_script`) beyond the
//! Chart panel's edits ([`Workbook::set_chart`]): a chart placed at a given
//! spot, empty or made from data; a chart's series changed one by one
//! (names, categories and values as references or literal arrays, as
//! `Series.Values = Array(...)` gives them); chart sheets added; a chart's
//! part as it is now.
//!
//! Every change is one history state made the way the Chart panel's are
//! (`own_edit`): a chart made in Kantor keeps its part in
//! [`crate::edits::Edits::charts`] (from a spec until its series are edited
//! one by one, then as XML, like a moved chart), a file's chart or a chart
//! sheet's in [`crate::edits::Edits::file_charts`]. So undo, redo and save
//! treat them as edits made by hand.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use roxmltree::{Document, Node};

use crate::chart_edit::{
    CHART_AREA_FILL, ChartSpec, ChartType, LegendPos, NEW_CHART_BASE, NewChart, abs_ref,
    chart_space,
};
use crate::chart_style::{self, NS_C, NewSeries};
use crate::drawing::{Anchor, ChartSheetNow, DrawingKey, RefValue, Tab, chart_sheet_key_id};
use crate::{CellRange, MAX_COLS, MAX_ROWS, Workbook};

/// Where a series' name, categories or values come from.
#[derive(Clone, Debug, PartialEq)]
pub enum SeriesData {
    /// Cells, as a reference with its sheet (`Sheet1!$B$2:$B$5`).
    Ref(String),
    /// A name typed in (`c:tx/c:v`).
    Text(String),
    /// Literal numbers (`c:numLit`).
    Numbers(Vec<f64>),
    /// Literal texts (`c:strLit`).
    Texts(Vec<String>),
}

/// One series of a chart, in the order the part has them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptSeries {
    pub name: Option<SeriesData>,
    /// The name as the chart shows it (a reference's cached text).
    pub shown_name: String,
    /// Categories (X values of a scatter chart).
    pub cats: Option<SeriesData>,
    /// `None`: a new series with no values yet.
    pub vals: Option<SeriesData>,
}

fn is_c(n: Node, name: &str) -> bool {
    n.is_element() && n.tag_name().namespace() == Some(NS_C) && n.tag_name().name() == name
}

fn kid<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|k| is_c(*k, name))
}

/// The `c:pt` values of a cache or literal, in index order.
fn points(n: Node) -> Vec<String> {
    let mut v: Vec<(usize, String)> = n
        .children()
        .filter(|p| is_c(*p, "pt"))
        .filter_map(|p| {
            let i = p.attribute("idx")?.trim().parse().ok()?;
            let t = kid(p, "v").and_then(|v| v.text()).unwrap_or_default();
            Some((i, t.to_owned()))
        })
        .collect();
    v.sort_by_key(|p| p.0);
    v.into_iter().map(|p| p.1).collect()
}

/// What a `c:tx`, `c:cat`, `c:val`... holds.
fn source(holder: Node) -> Option<SeriesData> {
    for k in holder.children().filter(|k| k.is_element()) {
        match k.tag_name().name() {
            "strRef" | "numRef" | "multiLvlStrRef" => {
                let f = kid(k, "f")?.text()?.trim().to_owned();
                return Some(SeriesData::Ref(f));
            }
            "numLit" => {
                return Some(SeriesData::Numbers(
                    points(k)
                        .iter()
                        .filter_map(|v| v.trim().parse().ok())
                        .collect(),
                ));
            }
            "strLit" => return Some(SeriesData::Texts(points(k))),
            "v" => return Some(SeriesData::Text(k.text().unwrap_or_default().to_owned())),
            _ => {}
        }
    }
    None
}

/// The text a series' `c:tx` shows.
fn shown(tx: Node) -> String {
    if let Some(v) = kid(tx, "v") {
        return v.text().unwrap_or_default().to_owned();
    }
    tx.descendants()
        .filter(|d| is_c(*d, "v"))
        .filter_map(|d| d.text())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The series of chart part `xml` in document order, and whether its
/// (first) plot is a scatter chart.
fn read_series(xml: &str) -> Result<(Vec<ScriptSeries>, bool)> {
    let doc = Document::parse(xml)?;
    let plot = kid(doc.root_element(), "chart")
        .and_then(|c| kid(c, "plotArea"))
        .ok_or_else(|| anyhow!("not a chart part"))?;
    let groups: Vec<Node> = plot
        .children()
        .filter(|g| {
            g.is_element()
                && g.tag_name().namespace() == Some(NS_C)
                && g.tag_name().name().ends_with("Chart")
        })
        .collect();
    let scatter = groups
        .first()
        .is_some_and(|g| g.tag_name().name() == "scatterChart");
    let mut out = Vec::new();
    for g in &groups {
        for s in g.children().filter(|s| is_c(*s, "ser")) {
            let tx = kid(s, "tx");
            out.push(ScriptSeries {
                name: tx.and_then(source),
                shown_name: tx.map(shown).unwrap_or_default(),
                cats: kid(s, "cat").or_else(|| kid(s, "xVal")).and_then(source),
                vals: kid(s, "val").or_else(|| kid(s, "yVal")).and_then(source),
            });
        }
    }
    Ok((out, scatter))
}

fn num_lit(v: &[f64]) -> String {
    let mut x = format!(
        "<c:numLit><c:formatCode>General</c:formatCode><c:ptCount val=\"{}\"/>",
        v.len()
    );
    for (i, n) in v.iter().enumerate() {
        x.push_str(&format!("<c:pt idx=\"{i}\"><c:v>{n}</c:v></c:pt>"));
    }
    x.push_str("</c:numLit>");
    x
}

fn str_lit(v: &[String]) -> String {
    let mut x = format!("<c:strLit><c:ptCount val=\"{}\"/>", v.len());
    for (i, s) in v.iter().enumerate() {
        x.push_str(&format!(
            "<c:pt idx=\"{i}\"><c:v>{}</c:v></c:pt>",
            chart_style::esc(s)
        ));
    }
    x.push_str("</c:strLit>");
    x
}

/// An empty chart of `spec`'s type (no series), with a Sheets chart's
/// chart area (as `chart_edit::with_chart_area`).
fn empty_part(spec: &ChartSpec) -> String {
    let xml = chart_space(spec, &[]);
    match xml.rfind("</c:chart>") {
        Some(i) => {
            let at = i + "</c:chart>".len();
            format!(
                "{}<c:spPr>{CHART_AREA_FILL}</c:spPr>{}",
                &xml[..at],
                &xml[at..]
            )
        }
        None => xml,
    }
}

impl Workbook {
    /// The part of chart `key` as it is now (its references in the current
    /// rows and columns): a chart made in Kantor, one of the file, or a
    /// chart sheet's. `None`: no such chart (deleted, not a chart).
    pub fn script_chart_part(&self, key: DrawingKey) -> Option<String> {
        if key.index >= NEW_CHART_BASE {
            self.sheet_index_of(key.sheet_id)?;
            let c = self.new_chart(key)?;
            if c.picture.is_some() || c.shape.is_some() {
                return None;
            }
            return Some(self.new_chart_part(&c));
        }
        self.chart_info(key)?;
        Some(self.file_chart_now(key)?.0)
    }

    /// A chart reference to `range` of sheet `sheet` (an index), as chart
    /// parts write them (`Sheet1!$B$2:$B$5`, `'My data'!$A$1`).
    pub fn script_chart_ref(&self, sheet: u32, range: CellRange) -> String {
        let name = self
            .sheet_names()
            .get(sheet as usize)
            .cloned()
            .unwrap_or_default();
        abs_ref(&name, &range)
    }

    /// The part of `data` on `sheet` a chart would show: whole rows or
    /// columns cut to the filled cells, empty rows and columns at the edges
    /// left out. `None`: it has no number to chart.
    pub fn script_chart_data(&self, sheet: u32, data: CellRange) -> Option<CellRange> {
        let mut d = data;
        if d.bottom >= MAX_ROWS - 1 || d.right >= MAX_COLS - 1 {
            let filled = self.filled_cells(sheet);
            if filled.is_empty() {
                return None;
            }
            let (r, c) = filled
                .iter()
                .fold((0, 0), |(r, c), &(a, b)| (r.max(a), c.max(b)));
            d.bottom = d.bottom.min(r);
            d.right = d.right.min(c);
            if d.top > d.bottom || d.left > d.right {
                return None;
            }
        }
        let row = |d: &CellRange, r: u32| (d.left..=d.right).any(|c| self.is_filled(sheet, r, c));
        let col = |d: &CellRange, c: u32| (d.top..=d.bottom).any(|r| self.is_filled(sheet, r, c));
        while d.bottom > d.top && !row(&d, d.bottom) {
            d.bottom -= 1;
        }
        while d.right > d.left && !col(&d, d.right) {
            d.right -= 1;
        }
        while d.top < d.bottom && !row(&d, d.top) {
            d.top += 1;
        }
        while d.left < d.right && !col(&d, d.left) {
            d.left += 1;
        }
        if (d.bottom - d.top + 1) as u64 * (d.right - d.left + 1) as u64 > 50_000 {
            return Some(d);
        }
        let number = (d.top..=d.bottom)
            .any(|r| (d.left..=d.right).any(|c| self.raw_number(sheet, r, c).is_some()));
        number.then_some(d)
    }

    /// Whether a chart of `data` on `sheet` takes its series from rows, as
    /// Insert Chart decides (more columns than rows, labels left out).
    pub fn script_plot_by_rows(&self, sheet: u32, d: CellRange) -> bool {
        let is_label = |r: u32, c: u32| self.raw_number(sheet, r, c).is_none();
        let row_labels = d.bottom > d.top
            && (d.left..=d.right).all(|c| is_label(d.top, c))
            && (d.left..=d.right).any(|c| !self.display_value(sheet, d.top, c).is_empty());
        let col_labels = d.right > d.left
            && (d.top + u32::from(row_labels)..=d.bottom).all(|r| is_label(r, d.left));
        let rows = d.bottom - d.top + 1 - u32::from(row_labels);
        let cols = d.right - d.left + 1 - u32::from(col_labels);
        cols > rows
    }

    /// A new chart's spec: of `kind`, from `data` (data sheet index, range,
    /// series from rows) when it has numbers. `home`: the worksheet the
    /// chart is on (`None`: a chart sheet). Also whether it has data.
    fn script_spec(
        &self,
        home: Option<u32>,
        kind: ChartType,
        three_d: bool,
        data: Option<(u32, CellRange, Option<bool>)>,
    ) -> Result<(ChartSpec, bool)> {
        if !ChartType::ALL.contains(&kind) {
            bail!("Kantor can't make a {} chart", kind.label());
        }
        let mut spec = ChartSpec {
            kind,
            legend: LegendPos::Right,
            three_d: three_d && kind.has_3d(),
            ..ChartSpec::placeholder()
        };
        let Some((ds, d, by_rows)) = data else {
            return Ok((spec, false));
        };
        let Some(d) = self.script_chart_data(ds, d) else {
            return Ok((spec, false));
        };
        if (d.bottom - d.top + 1) as u64 * (d.right - d.left + 1) as u64 > 50_000 {
            bail!("That's too many cells for a chart");
        }
        spec.by_rows = by_rows.unwrap_or_else(|| self.script_plot_by_rows(ds, d));
        spec.data = d;
        spec.data_sheet = match home {
            Some(h) if h == ds => None,
            _ => Some(self.sheet_id(ds)),
        };
        Ok((spec, true))
    }

    /// A chart of `kind` (its 3-D form with `three_d`) on worksheet `sheet`
    /// (an index) at `anchor`: made from `data` (sheet index, range, series
    /// from rows or `None` to decide) when that has numbers, else empty
    /// (`ChartObjects.Add`). One undo step. Returns its key.
    pub fn script_add_chart(
        &mut self,
        sheet: u32,
        anchor: Anchor,
        kind: ChartType,
        three_d: bool,
        data: Option<(u32, CellRange, Option<bool>)>,
    ) -> Result<DrawingKey> {
        if sheet as usize >= self.sheet_names().len() {
            bail!("there's no such sheet");
        }
        let (spec, has_data) = self.script_spec(Some(sheet), kind, three_d, data)?;
        let xml: Option<Arc<str>> = (!has_data).then(|| empty_part(&spec).into());
        let sheet_id = self.sheet_id(sheet);
        let mut id = 0;
        self.own_edit(|e, layout| {
            id = e.charts.iter().map(|c| c.id).max().map_or(1, |m| m + 1);
            e.charts.push(NewChart {
                id,
                sheet_id,
                spec,
                anchor,
                layout: layout.clone(),
                xml,
                picture: None,
                shape: None,
                links: None,
            });
        });
        Ok(DrawingKey {
            sheet_id,
            index: NEW_CHART_BASE + id as usize,
        })
    }

    /// A new chart sheet (`Charts.Add`) called `name` (`None`: the next
    /// free `ChartN`), its tab before tab `before` (`None`: after the last
    /// worksheet), with a chart of `kind` made from `data` as
    /// [`Workbook::script_add_chart`] does. One undo step. Returns its
    /// index `k` (see [`Workbook::chart_sheet_name`]).
    pub fn script_add_chart_sheet(
        &mut self,
        name: Option<&str>,
        before: Option<Tab>,
        kind: ChartType,
        three_d: bool,
        data: Option<(u32, CellRange, Option<bool>)>,
    ) -> Result<usize> {
        let name = match name {
            Some(n) => n.trim().to_owned(),
            None => self.next_chart_sheet_name(),
        };
        self.sheet_name_taken(&name, None)?;
        let (spec, has_data) = self.script_spec(None, kind, three_d, data)?;
        let xml = if has_data {
            self.chart_xml(0, &spec)
        } else {
            empty_part(&spec)
        };
        let after = match before {
            Some(t) => self.worksheets_before(t),
            None => (0..self.sheet_names().len() as u32)
                .rev()
                .map(|i| self.sheet_id(i))
                .collect(),
        };
        let k = self.drawings.chart_sheet_count();
        let parts = crate::move_chart::chart_sheet_parts(&name, k, xml.clone(), None, None);
        let k = self.drawings.add_chart_sheet(parts);
        let mut list = self.chart_sheets_now();
        list.push(ChartSheetNow {
            k,
            name,
            state: None,
            after,
            tab: None,
        });
        let key = DrawingKey {
            sheet_id: chart_sheet_key_id(k),
            index: 0,
        };
        self.own_edit(|e, layout| {
            e.chart_sheets = Some(Arc::new(list));
            e.file_charts.insert(key, (xml.into(), layout.clone()));
        });
        Ok(k)
    }

    /// The chart sheets (`k`s) in tab order, hidden ones too.
    pub fn script_chart_sheets(&self) -> Vec<usize> {
        self.all_tabs()
            .into_iter()
            .filter_map(|t| match t {
                Tab::Chart(k) => Some(k),
                Tab::Sheet(_) => None,
            })
            .collect()
    }

    /// The series of chart `key`, in the order its part has them.
    pub fn script_series(&self, key: DrawingKey) -> Result<Vec<ScriptSeries>> {
        let xml = self
            .script_chart_part(key)
            .ok_or_else(|| anyhow!("the chart is gone"))?;
        if crate::chartex_edit::is_chartex(&xml) {
            bail!("Kantor can't read the series of this type of chart");
        }
        Ok(read_series(&xml)?.0)
    }

    /// Replace the series of chart `key` by `list` (the first ones keep
    /// their formatting). One undo step.
    pub fn script_set_series(&mut self, key: DrawingKey, list: &[ScriptSeries]) -> Result<()> {
        let xml = self
            .script_chart_part(key)
            .ok_or_else(|| anyhow!("the chart is gone"))?;
        if crate::chartex_edit::is_chartex(&xml) {
            bail!("Kantor can't change the series of this type of chart");
        }
        let (_, scatter) = read_series(&xml)?;
        let parts = list
            .iter()
            .map(|s| self.series_part(s, scatter))
            .collect::<Result<Vec<_>>>()?;
        let new = chart_style::set_series(&xml, &parts)?;
        if new == xml {
            return Ok(());
        }
        self.script_put_part(key, new)
    }

    /// Chart `key`'s part replaced by `xml` (references in the current rows
    /// and columns). One undo step.
    fn script_put_part(&mut self, key: DrawingKey, xml: String) -> Result<()> {
        if key.index >= NEW_CHART_BASE {
            let c = self
                .new_chart(key)
                .ok_or_else(|| anyhow!("the chart is gone"))?;
            let (spec, anchor) = self.chart_now(&c);
            let id = c.id;
            self.own_edit(|e, layout| {
                if let Some(c) = e
                    .charts
                    .iter_mut()
                    .find(|c| c.id == id && c.sheet_id == key.sheet_id)
                {
                    c.spec = spec;
                    c.anchor = anchor;
                    c.layout = layout.clone();
                    c.xml = Some(xml.into());
                }
            });
            return Ok(());
        }
        if self.chart_info(key).is_none() {
            bail!("Kantor can't change this chart");
        }
        self.own_edit(|e, layout| {
            e.file_charts.insert(key, (xml.into(), layout.clone()));
        });
        Ok(())
    }

    fn cached_ref(&self, f: &str, numbers: bool) -> Result<String> {
        let data = self
            .chart_reference_now(f)
            .ok_or_else(|| anyhow!("{f} isn't a range of this workbook"))?;
        let f_xml = chart_style::esc(f);
        if numbers {
            let fmt = data.format.clone().unwrap_or_else(|| "General".into());
            let mut x = format!(
                "<c:numRef><c:f>{f_xml}</c:f><c:numCache><c:formatCode>{}</c:formatCode><c:ptCount val=\"{}\"/>",
                chart_style::esc(&fmt),
                data.values.len()
            );
            for (i, v) in data.values.iter().enumerate() {
                if let RefValue::Number(n) = v {
                    x.push_str(&format!("<c:pt idx=\"{i}\"><c:v>{n}</c:v></c:pt>"));
                }
            }
            x.push_str("</c:numCache></c:numRef>");
            Ok(x)
        } else {
            let mut x = format!(
                "<c:strRef><c:f>{f_xml}</c:f><c:strCache><c:ptCount val=\"{}\"/>",
                data.shown.len()
            );
            for (i, s) in data.shown.iter().enumerate() {
                x.push_str(&format!(
                    "<c:pt idx=\"{i}\"><c:v>{}</c:v></c:pt>",
                    chart_style::esc(s)
                ));
            }
            x.push_str("</c:strCache></c:strRef>");
            Ok(x)
        }
    }

    /// `s` as `c:tx` / `c:cat` / `c:val` content.
    fn series_part(&self, s: &ScriptSeries, scatter: bool) -> Result<NewSeries> {
        let tx = match &s.name {
            None => None,
            Some(SeriesData::Ref(f)) => Some(self.cached_ref(f, false)?),
            Some(SeriesData::Text(t)) => Some(format!("<c:v>{}</c:v>", chart_style::esc(t))),
            Some(SeriesData::Texts(t)) => {
                Some(format!("<c:v>{}</c:v>", chart_style::esc(&t.join(" "))))
            }
            Some(SeriesData::Numbers(n)) => Some(format!(
                "<c:v>{}</c:v>",
                n.iter().map(f64::to_string).collect::<Vec<_>>().join(" ")
            )),
        };
        let cat = match &s.cats {
            None => None,
            Some(SeriesData::Ref(f)) => {
                let numeric = scatter
                    || self.chart_reference_now(f).is_some_and(|d| {
                        d.values.iter().any(|v| matches!(v, RefValue::Number(_)))
                            && d.values.iter().all(|v| !matches!(v, RefValue::Text(_)))
                    });
                Some(self.cached_ref(f, numeric)?)
            }
            Some(SeriesData::Numbers(n)) => Some(num_lit(n)),
            Some(SeriesData::Texts(t)) => Some(str_lit(t)),
            Some(SeriesData::Text(t)) => Some(str_lit(std::slice::from_ref(t))),
        };
        let val = match &s.vals {
            None => num_lit(&[]),
            Some(SeriesData::Ref(f)) => self.cached_ref(f, true)?,
            Some(SeriesData::Numbers(n)) => num_lit(n),
            Some(SeriesData::Text(_) | SeriesData::Texts(_)) => {
                bail!("A series' values must be numbers")
            }
        };
        Ok(NewSeries { tx, cat, val })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn series_read_back() {
        let xml = "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"><c:chart><c:plotArea><c:barChart><c:ser><c:idx val=\"0\"/><c:order val=\"0\"/><c:tx><c:v>Plan</c:v></c:tx><c:cat><c:strLit><c:ptCount val=\"2\"/><c:pt idx=\"1\"><c:v>b</c:v></c:pt><c:pt idx=\"0\"><c:v>a</c:v></c:pt></c:strLit></c:cat><c:val><c:numRef><c:f>Sheet1!$B$2:$B$3</c:f></c:numRef></c:val></c:ser></c:barChart></c:plotArea></c:chart></c:chartSpace>";
        let (s, scatter) = read_series(xml).unwrap();
        assert!(!scatter);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].name, Some(SeriesData::Text("Plan".into())));
        assert_eq!(s[0].shown_name, "Plan");
        assert_eq!(
            s[0].cats,
            Some(SeriesData::Texts(vec!["a".into(), "b".into()]))
        );
        assert_eq!(s[0].vals, Some(SeriesData::Ref("Sheet1!$B$2:$B$3".into())));
        assert!(num_lit(&[1.0, 2.5]).contains("<c:pt idx=\"1\"><c:v>2.5</c:v></c:pt>"));
    }
}
