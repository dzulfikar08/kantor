//! Charts made from data rather than from a sheet's cells: the
//! `c:chartSpace` part of a chart whose values sit in a small workbook of
//! their own (PowerPoint's embedded chart data: names in row 1, categories
//! in column A, one series per column from B), for `kantor slides build`
//! and the slides tools.
//!
//! The part is Insert Chart's ([`crate::chart_edit::chart_space`], with the
//! Chart panel's edits for titles, axes, combos and labels), then the
//! extras a slide chart may ask for: 100 % stacking, doughnuts, fills from
//! the theme, per-point colours, bar gap and overlap, gridlines, the
//! chart's text formatting and the embedded workbook's relationship.

use anyhow::{Result, bail};
use roxmltree::Document;

use super::*;
use crate::chart_edit::{ChartSpec, ChartType, chart_space};
use crate::chart_options::DataLabels;

/// A category: text, or a number (scatter charts' x values, years...).
#[derive(Clone, Debug, PartialEq)]
pub enum DataCategory {
    Text(String),
    Number(f64),
}

/// One series of a [`DataChart`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DataSeries {
    pub name: String,
    /// One per category (`None`: a gap).
    pub values: Vec<Option<f64>>,
    /// DrawingML fill markup (`<a:solidFill>...`, written with `a:`):
    /// bars, areas and slices are filled with it, lines and markers drawn.
    pub fill: Option<String>,
    /// The series' line (`<a:ln>...</a:ln>`): width and dash of a line
    /// series, or a bar's outline.
    pub line: Option<String>,
    /// Fills of single points (bars, slices): (category index, fill).
    pub point_fills: Vec<(usize, String)>,
    /// Data labels (`None`: none).
    pub labels: Option<DataLabels>,
    /// Where the series is on a worksheet (`None`: the data sheet's
    /// layout, see the module docs). `name`, `categories` and `values` are
    /// still its cached text and numbers.
    pub refs: Option<SeriesRefs>,
}

/// References (`Data!$B$2:$B$13`, without `=`) of a series whose data is in
/// a workbook's own cells (`kantor sheets` charts).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SeriesRefs {
    /// The name's cell (`None`: the name is written as text).
    pub name: Option<String>,
    /// The categories' cells (`None`: no categories).
    pub categories: Option<String>,
    /// The values' cells; empty: the values are written into the chart
    /// (`c:numLit`, a target line's).
    pub values: String,
}

/// A chart from data (see the module docs).
#[derive(Clone, Debug, PartialEq)]
pub struct DataChart {
    /// Type, title, legend, axis titles and options, combo series types
    /// (`data`, `data_sheet`, `colors` and `labels` are not used).
    pub spec: ChartSpec,
    /// The data's sheet name (`Sheet1`).
    pub sheet: String,
    pub categories: Vec<DataCategory>,
    pub series: Vec<DataSeries>,
    /// Stacked to 100 % (with a stacked `spec.kind`).
    pub percent: bool,
    /// A doughnut (with `spec.kind` Pie); the hole's size in percent.
    pub doughnut: Option<u32>,
    /// The values' number format (`#,##0.0`; `None`: General).
    pub number_format: Option<String>,
    /// Bar charts: the gap between categories (% of a bar) and the overlap
    /// of a category's bars (-100 to 100).
    pub gap_width: Option<u32>,
    pub overlap: Option<i32>,
    /// Gridlines of the value axis (`None`: Insert Chart's, shown).
    pub gridlines: Option<bool>,
    /// Hide the primary category (x) axis and the primary value (y) axis
    /// (`c:delete`): no line, ticks or labels. A category axis in reverse
    /// order (`spec`'s axis options) also moves the value axis to the
    /// category axis' far end (`c:crosses val="max"`), so a bar chart
    /// listing its categories from the top keeps its values at the bottom.
    pub hidden_axes: [bool; 2],
    /// The chart's text formatting: an `a:defRPr` element.
    pub text: Option<String>,
    /// The title's size (hundredths of a point; `None`: from `text`, as
    /// the application sizes titles).
    pub title_size: Option<u32>,
    /// The chart area's fill and outline (`c:spPr` content); `None`: none.
    pub area: Option<String>,
    /// The relationship id of the embedded workbook (`c:externalData`).
    pub workbook_rel: Option<String>,
}

fn col_ref(sheet: &str, col: u32, first: u32, last: u32) -> String {
    let q = crate::print::quote_sheet(sheet);
    let c = crate::column_name(col);
    if first == last {
        format!("{q}!${c}${first}")
    } else {
        format!("{q}!${c}${first}:${c}${last}")
    }
}

/// `v` as chart XML writes a number.
fn num(v: f64) -> String {
    crate::chart_options::num_text(v)
}

fn str_ref(f: &str, items: &[String]) -> String {
    let mut x = format!(
        "<c:strRef><c:f>{}</c:f><c:strCache><c:ptCount val=\"{}\"/>",
        esc(f),
        items.len()
    );
    for (i, s) in items.iter().enumerate() {
        x.push_str(&format!("<c:pt idx=\"{i}\"><c:v>{}</c:v></c:pt>", esc(s)));
    }
    x + "</c:strCache></c:strRef>"
}

fn num_ref(f: &str, format: &str, values: &[Option<f64>]) -> String {
    let mut x = format!(
        "<c:numRef><c:f>{}</c:f><c:numCache><c:formatCode>{}</c:formatCode><c:ptCount val=\"{}\"/>",
        esc(f),
        esc(format),
        values.len()
    );
    for (i, v) in values.iter().enumerate() {
        if let Some(v) = v.filter(|v| v.is_finite()) {
            x.push_str(&format!("<c:pt idx=\"{i}\"><c:v>{}</c:v></c:pt>", num(v)));
        }
    }
    x + "</c:numCache></c:numRef>"
}

/// Categories written into the chart itself (`c:strLit`).
fn str_lit(cats: &[DataCategory]) -> String {
    let mut x = format!("<c:strLit><c:ptCount val=\"{}\"/>", cats.len());
    for (i, c) in cats.iter().enumerate() {
        let t = match c {
            DataCategory::Text(t) => t.clone(),
            DataCategory::Number(v) => num(*v),
        };
        x.push_str(&format!("<c:pt idx=\"{i}\"><c:v>{}</c:v></c:pt>", esc(&t)));
    }
    x + "</c:strLit>"
}

/// Values written into the chart itself (`c:numLit`): a series with refs
/// but no `values` reference (a target line).
fn num_lit(format: &str, values: &[Option<f64>]) -> String {
    let mut x = format!(
        "<c:numLit><c:formatCode>{}</c:formatCode><c:ptCount val=\"{}\"/>",
        esc(format),
        values.len()
    );
    for (i, v) in values.iter().enumerate() {
        if let Some(v) = v.filter(|v| v.is_finite()) {
            x.push_str(&format!("<c:pt idx=\"{i}\"><c:v>{}</c:v></c:pt>", num(v)));
        }
    }
    x + "</c:numLit>"
}

/// The series as `c:tx` / `c:cat` / `c:val` content referring to the data
/// sheet (names in row 1, categories in column A).
fn new_series(d: &DataChart) -> Vec<NewSeries> {
    let n = d.categories.len() as u32;
    let last = n + 1;
    let numeric = !d.categories.is_empty()
        && d.categories
            .iter()
            .all(|c| matches!(c, DataCategory::Number(_)));
    let cat_f = col_ref(&d.sheet, 0, 2, last);
    let cat = if numeric {
        let v: Vec<Option<f64>> = d
            .categories
            .iter()
            .map(|c| match c {
                DataCategory::Number(x) => Some(*x),
                DataCategory::Text(_) => None,
            })
            .collect();
        num_ref(&cat_f, "General", &v)
    } else {
        let v: Vec<String> = d
            .categories
            .iter()
            .map(|c| match c {
                DataCategory::Text(t) => t.clone(),
                DataCategory::Number(x) => num(*x),
            })
            .collect();
        str_ref(&cat_f, &v)
    };
    let format = d.number_format.as_deref().unwrap_or("General");
    d.series
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let col = i as u32 + 1;
            if let Some(r) = &s.refs {
                let tx = match &r.name {
                    Some(f) => str_ref(f, std::slice::from_ref(&s.name)),
                    None => format!("<c:v>{}</c:v>", esc(&s.name)),
                };
                let cat = r.categories.as_ref().filter(|_| n > 0).map(|f| {
                    // The cached categories, under this series' reference.
                    let at = cat.find("<c:f>").map_or(0, |i| i + 5);
                    let end = cat[at..].find("</c:f>").map_or(at, |i| at + i);
                    format!("{}{}{}", &cat[..at], esc(f), &cat[end..])
                });
                // Categories given as a list: written into the chart.
                let cat = cat.or_else(|| (n > 0).then(|| str_lit(&d.categories)));
                let val = if r.values.is_empty() {
                    num_lit(format, &s.values)
                } else {
                    num_ref(&r.values, format, &s.values)
                };
                return NewSeries {
                    tx: Some(tx),
                    cat,
                    val,
                };
            }
            NewSeries {
                tx: Some(str_ref(
                    &col_ref(&d.sheet, col, 1, 1),
                    std::slice::from_ref(&s.name),
                )),
                cat: (n > 0).then(|| cat.clone()),
                val: num_ref(&col_ref(&d.sheet, col, 2, last), format, &s.values),
            }
        })
        .collect()
}

fn reparse(xml: &str) -> Result<Document<'_>> {
    Ok(Document::parse(xml)?)
}

/// Every plot group's `c:grouping` "stacked" made "percentStacked".
fn percent_stacked(xml: &str) -> Result<String> {
    let doc = reparse(xml)?;
    let (_, plot) = plot_area(&doc)?;
    let mut edits = Vec::new();
    for g in groups(plot) {
        if let Some(n) = kid(g, "grouping").filter(|n| n.attribute("val") == Some("stacked")) {
            edits.push((
                n.range(),
                format!("<{}grouping val=\"percentStacked\"/>", prefix_of(xml, n)),
            ));
        }
    }
    Ok(splice(xml, edits))
}

/// The pie made a doughnut with a hole of `hole` %.
fn doughnut(xml: &str, hole: u32) -> Result<String> {
    let doc = reparse(xml)?;
    let (_, plot) = plot_area(&doc)?;
    let Some(pie) = groups(plot)
        .into_iter()
        .find(|g| g.tag_name().name() == "pieChart")
    else {
        bail!("not a pie chart");
    };
    let p = prefix_of(xml, pie);
    let hole = hole.clamp(10, 90);
    // CT_DoughnutChart: varyColors, ser*, dLbls, firstSliceAng, holeSize.
    let text = rebuild(
        xml,
        pie,
        "doughnutChart",
        &[
            "varyColors",
            "ser",
            "dLbls",
            "firstSliceAng",
            "holeSize",
            "extLst",
        ],
        &[],
        &mut |slot| match slot {
            "firstSliceAng" => Some(Some(format!("<{p}firstSliceAng val=\"0\"/>"))),
            "holeSize" => Some(Some(format!("<{p}holeSize val=\"{hole}\"/>"))),
            _ => None,
        },
    );
    Ok(splice(xml, vec![(pie.range(), text)]))
}

/// Gap width and overlap of every bar group.
fn bar_spacing(xml: &str, gap: Option<u32>, overlap: Option<i32>) -> Result<String> {
    let doc = reparse(xml)?;
    let (_, plot) = plot_area(&doc)?;
    let mut edits = Vec::new();
    for g in groups(plot)
        .into_iter()
        .filter(|g| g.tag_name().name() == "barChart")
    {
        let order = group_order("barChart");
        if let Some(v) = gap {
            edits.push(set_val(xml, g, "gapWidth", order, &v.min(500).to_string()));
        }
        if let Some(v) = overlap {
            edits.push(set_val(
                xml,
                g,
                "overlap",
                order,
                &v.clamp(-100, 100).to_string(),
            ));
        }
    }
    Ok(splice(xml, edits))
}

/// Hidden primary axes (`[category, value]`), and the value axis crossing
/// a reversed category axis at its far end (see [`DataChart::hidden_axes`]).
fn axes_look(xml: &str, hidden: [bool; 2]) -> Result<String> {
    let doc = reparse(xml)?;
    let (_, plot) = plot_area(&doc)?;
    let (h, v) = primary_axes(plot);
    // A bar chart's category axis is the vertical one.
    let (cat, value) = match (h, v) {
        (Some(h), Some(v)) if is_value_axis(h) && !is_value_axis(v) => (Some(v), Some(h)),
        other => other,
    };
    let mut edits = Vec::new();
    let set = |ax: Node, name: &str, v: &str, edits: &mut Vec<Edit>| {
        let p = prefix_of(xml, ax);
        let order: &[&str] = match ax.tag_name().name() {
            "valAx" => VAL_AX,
            "catAx" => CAT_AX,
            "dateAx" => DATE_AX,
            _ => AX_ORDER,
        };
        let text = format!("<{p}{name} val=\"{v}\"/>");
        edits.push(match kid(ax, name) {
            Some(n) => (n.range(), text),
            None => insert_ordered(xml, ax, name, order, &text),
        });
    };
    for (ax, hide) in [(cat, hidden[0]), (value, hidden[1])] {
        if let Some(ax) = ax
            && hide
        {
            set(ax, "delete", "1", &mut edits);
        }
    }
    if let (Some(cat), Some(value)) = (cat, value)
        && !is_value_axis(cat)
        && kid(cat, "scaling").and_then(|s| val(s, "orientation")) == Some("maxMin")
        && kid(value, "crossesAt").is_none()
        && val(value, "crosses").is_none_or(|c| c == "autoZero")
    {
        set(value, "crosses", "max", &mut edits);
    }
    Ok(splice(xml, edits))
}

/// The values' number format `code` written on every value axis that has
/// no format of its own (`sourceLinked="1"`), as `sourceLinked="0"`: a
/// linked axis shows the source cells' format, which on a worksheet is
/// often General. Scatter charts' x axes keep theirs.
fn value_axis_formats(xml: &str, code: &str) -> Result<String> {
    let doc = reparse(xml)?;
    let (_, plot) = plot_area(&doc)?;
    let all = axes(plot);
    let mut edits = Vec::new();
    let mut done = Vec::new();
    for g in groups(plot) {
        let ids: Vec<&str> = kids(g, "axId").filter_map(|a| a.attribute("val")).collect();
        let mine: Vec<Node> = all
            .iter()
            .copied()
            .filter(|ax| val(*ax, "axId").is_some_and(|i| ids.contains(&i)))
            .collect();
        let has_cat = mine.iter().any(|ax| !is_value_axis(*ax));
        // With a category axis, the value axis; a scatter's two value
        // axes: the vertical one (the y values).
        let Some(ax) = mine.into_iter().find(|ax| {
            is_value_axis(*ax) && (has_cat || matches!(val(*ax, "axPos"), Some("l" | "r")))
        }) else {
            continue;
        };
        if done.contains(&ax.range().start) {
            continue;
        }
        done.push(ax.range().start);
        let linked = kid(ax, "numFmt")
            .is_none_or(|f| matches!(f.attribute("sourceLinked"), Some("1" | "true")));
        if !linked {
            continue;
        }
        let p = prefix_of(xml, ax);
        let text = format!(
            "<{p}numFmt formatCode=\"{}\" sourceLinked=\"0\"/>",
            esc(code)
        );
        edits.push(match kid(ax, "numFmt") {
            Some(n) => (n.range(), text),
            None => insert_ordered(xml, ax, "numFmt", VAL_AX, &text),
        });
    }
    Ok(splice(xml, edits))
}

/// Value axes' major gridlines shown or not.
fn gridlines(xml: &str, on: bool) -> Result<String> {
    let doc = reparse(xml)?;
    let (_, plot) = plot_area(&doc)?;
    let mut edits = Vec::new();
    for ax in axes(plot).into_iter().filter(|a| is_value_axis(*a)) {
        // Secondary axes (crossing at the maximum) keep none.
        let secondary =
            val(ax, "crosses") == Some("max") && matches!(val(ax, "axPos"), Some("r" | "t"));
        match kid(ax, "majorGridlines") {
            Some(g) if !on => edits.push((g.range(), String::new())),
            None if on && !secondary => {
                let p = prefix_of(xml, ax);
                edits.push(insert_ordered(
                    xml,
                    ax,
                    "majorGridlines",
                    VAL_AX,
                    &format!("<{p}majorGridlines/>"),
                ));
            }
            _ => {}
        }
    }
    Ok(splice(xml, edits))
}

/// Series `index`'s line (`a:ln` of its `c:spPr`) replaced by `ln`.
fn series_line(xml: &str, index: usize, ln: &str) -> Result<String> {
    let doc = reparse(xml)?;
    let (_, plot) = plot_area(&doc)?;
    let sers = all_series(plot);
    let Some(&(ser, _)) = sers.get(index) else {
        bail!("no series {}", index + 1);
    };
    let edit = match kid(ser, "spPr") {
        Some(sp) => match akid(sp, "ln") {
            Some(old) => (old.range(), ln.to_owned()),
            None => insert_ordered(xml, sp, "ln", SPPR_ORDER, ln),
        },
        None => insert_ordered(xml, ser, "spPr", SER_ORDER, &new_sppr(xml, ser, ln)),
    };
    Ok(splice(xml, vec![edit]))
}

/// Fills of single points of series `index` (`c:dPt`).
fn point_fills(xml: &str, index: usize, fills: &[(usize, String)]) -> Result<String> {
    if fills.is_empty() {
        return Ok(xml.to_owned());
    }
    let doc = reparse(xml)?;
    let (_, plot) = plot_area(&doc)?;
    let sers = all_series(plot);
    let Some(&(ser, tag)) = sers.get(index) else {
        bail!("no series {}", index + 1);
    };
    if family(tag) != Family::Fill {
        bail!("only bars and slices take colours of their own");
    }
    let p = prefix_of(xml, ser);
    let bar = tag == "barChart";
    let mut fills = fills.to_vec();
    fills.sort_by_key(|(i, _)| *i);
    fills.dedup_by_key(|(i, _)| *i);
    let mut text = String::new();
    for (i, f) in &fills {
        text.push_str(&format!("<{p}dPt><{p}idx val=\"{i}\"/>"));
        if bar {
            text.push_str(&format!("<{p}invertIfNegative val=\"0\"/>"));
        }
        text.push_str(&format!(
            "<{p}bubble3D val=\"0\"/><{p}spPr>{f}</{p}spPr></{p}dPt>"
        ));
    }
    let old: Vec<_> = kids(ser, "dPt").collect();
    let mut edits: Vec<Edit> = old.iter().map(|o| (o.range(), String::new())).collect();
    edits.push(insert_ordered(xml, ser, "dPt", SER_ORDER, &text));
    Ok(splice(xml, edits))
}

/// The chart space's own fill, text formatting and workbook link, after
/// `c:chart`.
fn space_tail(xml: &str, d: &DataChart) -> Result<String> {
    let doc = reparse(xml)?;
    let (chart, _) = plot_area(&doc)?;
    let mut tail = String::new();
    tail.push_str(&format!(
        "<c:spPr>{}</c:spPr>",
        d.area
            .as_deref()
            .unwrap_or("<a:noFill/><a:ln><a:noFill/></a:ln>")
    ));
    if let Some(r) = &d.text {
        tail.push_str(&format!(
            "<c:txPr><a:bodyPr/><a:lstStyle/><a:p><a:pPr>{r}</a:pPr><a:endParaRPr lang=\"en-US\"/></a:p></c:txPr>"
        ));
    }
    if let Some(id) = &d.workbook_rel {
        tail.push_str(&format!(
            "<c:externalData r:id=\"{}\"><c:autoUpdate val=\"0\"/></c:externalData>",
            esc(id)
        ));
    }
    let at = chart.range().end;
    let mut edits = vec![(at..at, tail)];
    // The title's own size (its `a:defRPr` as `chart_space` wrote it).
    if let Some(sz) = d.title_size
        && let Some(t) = kid(chart, "title")
    {
        let r = t.range();
        let empty = "<a:pPr><a:defRPr/></a:pPr>";
        if let Some(i) = xml[r.clone()].find(empty) {
            let s = r.start + i;
            edits.push((
                s..s + empty.len(),
                format!("<a:pPr><a:defRPr sz=\"{sz}\"/></a:pPr>"),
            ));
        }
    }
    Ok(splice(xml, edits))
}

/// The `c:chartSpace` part of `d`.
pub fn data_chart_part(d: &DataChart) -> Result<String> {
    if d.series.is_empty() {
        bail!("a chart needs at least one series");
    }
    let n = d.categories.len();
    if let Some((i, s)) = d
        .series
        .iter()
        .enumerate()
        .find(|(_, s)| s.values.len() != n)
    {
        bail!(
            "series {} ({}) has {} values for {n} categories",
            i + 1,
            s.name,
            s.values.len()
        );
    }
    let kind = d.spec.kind;
    if kind == ChartType::Other || kind.is_ex() {
        bail!("Kantor can't make this chart type from data");
    }
    if kind == ChartType::Pie && d.series.len() > 1 {
        bail!("a pie or doughnut chart shows one series");
    }
    let mut spec = d.spec.clone();
    spec.colors.clear();
    spec.labels.clear();
    spec.three_d = false;
    if kind != ChartType::Combo {
        spec.series_types.clear();
    }
    let mut xml = chart_space(&spec, &new_series(d));
    if d.percent {
        xml = percent_stacked(&xml)?;
    }
    if let Some(h) = d.doughnut {
        xml = doughnut(&xml, h)?;
    }
    if d.gap_width.is_some() || d.overlap.is_some() {
        xml = bar_spacing(&xml, d.gap_width, d.overlap)?;
    }
    if let Some(on) = d.gridlines {
        xml = gridlines(&xml, on)?;
    }
    xml = axes_look(&xml, d.hidden_axes)?;
    // The values' format on the axes and labels themselves, not only in the
    // cached values: sheet charts' caches follow their cells' formats.
    let values_format = d
        .number_format
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty() && !c.eq_ignore_ascii_case("general"));
    if let Some(code) = values_format
        && kind != ChartType::Pie
        && !d.percent
    {
        xml = value_axis_formats(&xml, code)?;
    }
    for (i, s) in d.series.iter().enumerate() {
        if let Some(f) = &s.fill {
            xml = set_fill(&xml, i, Some(&NewFill::Raw(f.clone())))?;
        }
        if let Some(ln) = &s.line {
            xml = series_line(&xml, i, ln)?;
        }
        xml = point_fills(&xml, i, &s.point_fills)?;
        if let Some(l) = s.labels.as_ref().filter(|l| l.shown()) {
            // Percentages (pies) keep their own format: Excel would apply
            // the values' code to them too.
            let mut l = l.clone();
            if l.num_fmt.is_none() && l.value && !l.percent {
                l.num_fmt = values_format.map(str::to_owned);
            }
            xml = set_labels(&xml, i, &l)?;
        }
    }
    xml = drop_bad_positions(&xml);
    space_tail(&xml, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart_edit::{LegendPos, SeriesType};
    use crate::chart_options::{AxisOptions, LabelPos};
    use crate::{CellRange, SeriesColor};

    fn spec(kind: ChartType) -> ChartSpec {
        ChartSpec {
            kind,
            title: "Sales".into(),
            legend: LegendPos::Bottom,
            by_rows: false,
            data: CellRange::from_corners((0, 0), (0, 0)),
            data_sheet: None,
            x_title: String::new(),
            y_title: String::new(),
            colors: vec![SeriesColor::Automatic],
            labels: Vec::new(),
            x_axis: AxisOptions::default(),
            y_axis: AxisOptions::default(),
            three_d: false,
            series_types: Vec::new(),
        }
    }

    fn chart(kind: ChartType) -> DataChart {
        DataChart {
            spec: spec(kind),
            sheet: "Sheet1".into(),
            categories: ["Q1", "Q2", "Q3"]
                .iter()
                .map(|c| DataCategory::Text((*c).into()))
                .collect(),
            series: vec![
                DataSeries {
                    name: "2025".into(),
                    values: vec![Some(1.5), Some(2.0), None],
                    ..DataSeries::default()
                },
                DataSeries {
                    name: "2026".into(),
                    values: vec![Some(3.0), Some(4.0), Some(5.0)],
                    ..DataSeries::default()
                },
            ],
            percent: false,
            doughnut: None,
            number_format: Some("#,##0.0".into()),
            gap_width: None,
            overlap: None,
            gridlines: None,
            hidden_axes: [false; 2],
            text: None,
            title_size: None,
            area: None,
            workbook_rel: Some("rId1".into()),
        }
    }

    #[test]
    fn column_chart_refers_to_the_data_sheet() {
        let xml = data_chart_part(&chart(ChartType::Column)).unwrap();
        Document::parse(&xml).unwrap();
        assert!(xml.contains("<c:f>Sheet1!$B$1</c:f>"), "{xml}");
        assert!(xml.contains("<c:f>Sheet1!$A$2:$A$4</c:f>"));
        assert!(xml.contains("<c:f>Sheet1!$C$2:$C$4</c:f>"));
        assert!(xml.contains("<c:formatCode>#,##0.0</c:formatCode>"));
        assert!(xml.ends_with(
            "<c:externalData r:id=\"rId1\"><c:autoUpdate val=\"0\"/></c:externalData></c:chartSpace>"
        ));
        let style = read(&xml).unwrap();
        assert_eq!(style.names, vec!["2025".to_owned(), "2026".to_owned()]);
        // The same data, the same part.
        assert_eq!(xml, data_chart_part(&chart(ChartType::Column)).unwrap());
    }

    #[test]
    fn series_can_refer_to_a_workbook() {
        let mut d = chart(ChartType::Column);
        d.workbook_rel = None;
        d.series[0].refs = Some(SeriesRefs {
            name: Some("Data!$B$1".into()),
            categories: Some("Data!$A$2:$A$4".into()),
            values: "Data!$B$2:$B$4".into(),
        });
        d.series[1].refs = Some(SeriesRefs {
            name: None,
            categories: Some("Data!$A$2:$A$4".into()),
            values: String::new(),
        });
        let xml = data_chart_part(&d).unwrap();
        Document::parse(&xml).unwrap();
        assert!(xml.contains("<c:f>Data!$B$1</c:f>"), "{xml}");
        assert!(xml.contains("<c:f>Data!$B$2:$B$4</c:f>"));
        assert_eq!(xml.matches("<c:f>Data!$A$2:$A$4</c:f>").count(), 2);
        assert!(xml.contains("<c:tx><c:v>2026</c:v></c:tx>"));
        assert!(xml.contains("<c:numLit>"));
        assert!(!xml.contains("Sheet1") && !xml.contains("externalData"));
    }

    #[test]
    fn extras_are_written() {
        let mut d = chart(ChartType::StackedBar);
        d.percent = true;
        d.gap_width = Some(50);
        d.overlap = Some(100);
        d.gridlines = Some(false);
        d.series[0].fill = Some("<a:solidFill><a:schemeClr val=\"accent2\"/></a:solidFill>".into());
        d.series[1].point_fills = vec![(
            2,
            "<a:solidFill><a:srgbClr val=\"C00000\"/></a:solidFill>".into(),
        )];
        d.series[1].labels = Some(DataLabels {
            value: true,
            pos: Some(LabelPos::Center),
            ..DataLabels::default()
        });
        d.text = Some("<a:defRPr sz=\"1200\"/>".into());
        d.title_size = Some(1440);
        let xml = data_chart_part(&d).unwrap();
        Document::parse(&xml).unwrap();
        assert!(xml.contains("<c:grouping val=\"percentStacked\"/>"));
        assert!(xml.contains("<c:gapWidth val=\"50\"/>"));
        assert!(!xml.contains("majorGridlines"));
        assert!(xml.contains("<a:schemeClr val=\"accent2\"/>"));
        assert!(xml.contains("<a:defRPr sz=\"1440\"/>"), "{xml}");
        assert!(xml.contains("<c:dPt><c:idx val=\"2\"/>"));
        assert!(xml.contains("<c:dLblPos val=\"ctr\"/>"));
        assert!(
            xml.contains("<c:txPr><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz=\"1200\"/>")
        );
    }

    #[test]
    fn doughnut_and_combo() {
        let mut d = chart(ChartType::Pie);
        d.series.truncate(1);
        d.doughnut = Some(60);
        d.series[0].labels = Some(DataLabels {
            percent: true,
            pos: Some(LabelPos::OutsideEnd),
            ..DataLabels::default()
        });
        let xml = data_chart_part(&d).unwrap();
        assert!(xml.contains("<c:doughnutChart>"), "{xml}");
        assert!(xml.contains("<c:holeSize val=\"60\"/>"));
        assert!(
            !xml.contains("dLblPos"),
            "doughnuts have no label positions"
        );
        assert!(!xml.contains("pieChart"));

        let mut d = chart(ChartType::Combo);
        d.spec.series_types = vec![
            SeriesType::default(),
            SeriesType {
                kind: ChartType::Line,
                secondary: true,
            },
        ];
        let xml = data_chart_part(&d).unwrap();
        assert!(
            xml.contains("<c:barChart>") && xml.contains("<c:lineChart>"),
            "{xml}"
        );
        assert_eq!(xml.matches("<c:valAx>").count(), 2);
    }

    /// The section of `xml` from `<c:{tag}>` to its end.
    fn section<'a>(xml: &'a str, tag: &str, nth: usize) -> &'a str {
        let a = xml
            .match_indices(&format!("<c:{tag}>"))
            .nth(nth)
            .unwrap_or_else(|| panic!("no {tag} {nth} in {xml}"))
            .0;
        let b = xml[a..].find(&format!("</c:{tag}>")).unwrap();
        &xml[a..a + b]
    }

    #[test]
    fn the_values_format_is_on_the_axis_and_labels() {
        // A sheet chart: the cells may be General, the chart's format wins.
        let mut d = chart(ChartType::Column);
        d.number_format = Some("\"Rp\"#,##0.0,,".into());
        d.series[0].labels = Some(DataLabels {
            value: true,
            ..DataLabels::default()
        });
        d.series[1].labels = Some(DataLabels {
            value: true,
            num_fmt: Some("#,##0".into()),
            ..DataLabels::default()
        });
        let xml = data_chart_part(&d).unwrap();
        Document::parse(&xml).unwrap();
        let code = "<c:numFmt formatCode=\"&quot;Rp&quot;#,##0.0,,\" sourceLinked=\"0\"/>";
        assert!(section(&xml, "valAx", 0).contains(code), "{xml}");
        assert!(!section(&xml, "catAx", 0).contains("Rp"), "{xml}");
        assert!(section(&xml, "dLbls", 0).contains(code), "{xml}");
        assert!(
            section(&xml, "dLbls", 1)
                .contains("<c:numFmt formatCode=\"#,##0\" sourceLinked=\"0\"/>"),
            "{xml}"
        );
        // The axis' own format stays.
        d.spec.y_axis.num_fmt = Some("0%".into());
        let xml = data_chart_part(&d).unwrap();
        assert!(
            section(&xml, "valAx", 0).contains("<c:numFmt formatCode=\"0%\" sourceLinked=\"0\"/>"),
            "{xml}"
        );
        assert_eq!(section(&xml, "valAx", 0).matches("numFmt").count(), 1);

        // A bar chart's value axis is the horizontal one; a combo's
        // secondary axis takes the format too.
        let mut d = chart(ChartType::Bar);
        d.number_format = Some("#,##0".into());
        let xml = data_chart_part(&d).unwrap();
        let fmt = "<c:numFmt formatCode=\"#,##0\" sourceLinked=\"0\"/>";
        assert!(section(&xml, "valAx", 0).contains(fmt), "{xml}");
        assert!(!section(&xml, "catAx", 0).contains(fmt), "{xml}");
        let mut d = chart(ChartType::Combo);
        d.number_format = Some("#,##0".into());
        d.spec.series_types = vec![
            SeriesType::default(),
            SeriesType {
                kind: ChartType::Line,
                secondary: true,
            },
        ];
        let xml = data_chart_part(&d).unwrap();
        assert_eq!(xml.matches(fmt).count(), 2, "{xml}");

        // A scatter's x values keep theirs; General writes nothing.
        let mut d = chart(ChartType::Scatter);
        d.categories = (1..=3).map(|x| DataCategory::Number(x as f64)).collect();
        d.number_format = Some("#,##0".into());
        let xml = data_chart_part(&d).unwrap();
        assert_eq!(xml.matches(fmt).count(), 1, "{xml}");
        d.number_format = Some("General".into());
        let xml = data_chart_part(&d).unwrap();
        assert!(!xml.contains("sourceLinked=\"0\""), "{xml}");

        // Pie percentages keep their own format.
        let mut d = chart(ChartType::Pie);
        d.series.truncate(1);
        d.series[0].labels = Some(DataLabels {
            percent: true,
            ..DataLabels::default()
        });
        let xml = data_chart_part(&d).unwrap();
        assert!(!xml.contains("sourceLinked=\"0\""), "{xml}");
    }

    #[test]
    fn reversed_categories_keep_the_values_at_the_bottom() {
        let part = |xml: &str, tag: &str| -> String {
            let a = xml.find(&format!("<c:{tag}>")).unwrap();
            let b = xml[a..].find(&format!("</c:{tag}>")).unwrap();
            xml[a..a + b].to_owned()
        };
        let mut d = chart(ChartType::Bar);
        d.spec.y_axis.reverse = true;
        let xml = data_chart_part(&d).unwrap();
        Document::parse(&xml).unwrap();
        let cat = part(&xml, "catAx");
        let val = part(&xml, "valAx");
        assert!(cat.contains("<c:orientation val=\"maxMin\"/>"), "{cat}");
        assert!(val.contains("<c:crosses val=\"max\"/>"), "{val}");
        assert!(val.contains("<c:majorGridlines/>"), "{val}");
        // Not reversed: the value axis crosses at zero as usual.
        let d = chart(ChartType::Bar);
        let xml = data_chart_part(&d).unwrap();
        assert!(part(&xml, "valAx").contains("<c:crosses val=\"autoZero\"/>"));
        // Hidden axes.
        let mut d = chart(ChartType::Column);
        d.hidden_axes = [false, true];
        let xml = data_chart_part(&d).unwrap();
        assert!(part(&xml, "catAx").contains("<c:delete val=\"0\"/>"));
        assert!(part(&xml, "valAx").contains("<c:delete val=\"1\"/>"));
        let mut d = chart(ChartType::Bar);
        d.hidden_axes = [true, false];
        let xml = data_chart_part(&d).unwrap();
        assert!(part(&xml, "catAx").contains("<c:delete val=\"1\"/>"));
        assert!(part(&xml, "valAx").contains("<c:delete val=\"0\"/>"));
    }

    #[test]
    fn mismatched_series_are_refused() {
        let mut d = chart(ChartType::Line);
        d.series[0].values.pop();
        assert!(data_chart_part(&d).is_err());
        let d = chart(ChartType::Pie);
        assert!(data_chart_part(&d).is_err());
    }
}
