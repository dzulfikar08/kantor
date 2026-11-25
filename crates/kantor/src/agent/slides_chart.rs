//! Chart elements of the slides JSON (`{"type": "chart", ...}`, see
//! docs/slides-json.md): real PowerPoint charts made from data. The chart
//! part is written by Sheets' chart writer ([`sheets_engine::data_chart_part`])
//! and the data goes into an embedded workbook
//! ([`slides_engine::chart_edit::chart_workbook`]), so PowerPoint's Edit
//! Data and Kantor's chart data panel both open it.

use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Map, Value};
use sheets_engine::{
    AxisOptions, CellRange, ChartSpec, ChartType, DataCategory, DataChart, DataLabels, DataSeries,
    LabelFont, LabelPos, LegendPos, SeriesType,
};
use slides_engine::Xfrm;
use slides_engine::author::{
    ChartPartSpec, Color, ColorBase, Element, ElementKind, FillSpec, LineSpec,
};
use slides_engine::chart_edit::{CellValue, chart_workbook};

use super::slides_author::{
    Ctx, color, font_size, get, keys_with, line, need_len, object, opt_bool, opt_len, opt_num,
    opt_str,
};

/// Categories (points per series) one chart may have.
const MAX_CATEGORIES: usize = 10_000;
/// Series one chart may have.
const MAX_SERIES: usize = 255;
/// The embedded workbook's sheet.
const SHEET: &str = "Sheet1";

/// The chart types of `chart`.
pub const CHART_TYPES: [&str; 8] = [
    "column", "bar", "line", "area", "pie", "doughnut", "scatter", "combo",
];

/// Keys of a chart element.
pub const CHART_KEYS: &[&str] = &[
    "type",
    "name",
    "x",
    "y",
    "w",
    "h",
    "chart",
    "stacked",
    "categories",
    "series",
    "title",
    "legend",
    "data_labels",
    "number_format",
    "axis",
    "category_axis",
    "gap_width",
    "overlap",
    "hole",
    "font",
    "colors",
    "target_line",
];

const SERIES_KEYS: &[&str] = &[
    "name",
    "values",
    "color",
    "colors",
    "type",
    "axis",
    "line",
    "data_labels",
];
const AXIS_KEYS: &[&str] = &[
    "title",
    "min",
    "max",
    "major_unit",
    "gridlines",
    "number_format",
    "reverse",
    "visible",
];
const LABEL_KEYS: &[&str] = &[
    "show", "position", "format", "size", "bold", "italic", "color",
];
const FONT_KEYS: &[&str] = &["font", "size", "color", "bold", "italic"];
const TARGET_KEYS: &[&str] = &["value", "label", "color", "width", "dash"];

/// The chart's base type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Base {
    Column,
    Bar,
    Line,
    Area,
    Pie,
    Doughnut,
    Scatter,
    Combo,
}

impl Base {
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "column" => Base::Column,
            "bar" => Base::Bar,
            "line" => Base::Line,
            "area" => Base::Area,
            "pie" => Base::Pie,
            "doughnut" => Base::Doughnut,
            "scatter" => Base::Scatter,
            "combo" => Base::Combo,
            other => bail!(
                "`chart` must be {}, not \"{other}\"",
                CHART_TYPES.join(", ")
            ),
        })
    }

    fn is_round(self) -> bool {
        matches!(self, Base::Pie | Base::Doughnut)
    }

    /// Whether its series may mix types and axes.
    fn combines(self) -> bool {
        matches!(self, Base::Column | Base::Line | Base::Area | Base::Combo)
    }
}

/// `stacked`: false, true / "stacked", or "percent".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stacking {
    None,
    Stacked,
    Percent,
}

fn stacking(o: &Map<String, Value>) -> Result<Stacking> {
    Ok(match get(o, "stacked") {
        None | Some(Value::Bool(false)) => Stacking::None,
        Some(Value::Bool(true)) => Stacking::Stacked,
        Some(Value::String(s)) => match s.as_str() {
            "stacked" => Stacking::Stacked,
            "percent" | "100%" => Stacking::Percent,
            "none" => Stacking::None,
            _ => bail!("`stacked` must be false, true, \"stacked\" or \"percent\""),
        },
        Some(_) => bail!("`stacked` must be false, true, \"stacked\" or \"percent\""),
    })
}

/// A series' plot type in a combo (`type`: column, line, area).
fn series_kind(ty: &str, stacked: bool) -> Result<ChartType> {
    Ok(match (ty, stacked) {
        ("column", false) => ChartType::Column,
        ("column", true) => ChartType::StackedColumn,
        ("line", false) => ChartType::Line,
        ("line", true) => ChartType::StackedLine,
        ("area", false) => ChartType::Area,
        ("area", true) => ChartType::StackedArea,
        _ => bail!("a series `type` must be column, line or area, not \"{ty}\""),
    })
}

fn rgb_of(c: &Color, key: &str) -> Result<[u8; 3]> {
    match &c.base {
        ColorBase::Rgb(rgb) => Ok(rgb.0),
        ColorBase::Scheme(_) => bail!("`{key}` must be #RRGGBB here (not a theme colour)"),
    }
}

fn fill_xml(v: &Value, key: &str) -> Result<String> {
    Ok(FillSpec::Solid(color(v, key)?).xml())
}

fn label_pos(s: &str) -> Result<LabelPos> {
    Ok(match s {
        "center" => LabelPos::Center,
        "inside_end" => LabelPos::InsideEnd,
        "inside_base" => LabelPos::InsideBase,
        "outside_end" => LabelPos::OutsideEnd,
        "left" => LabelPos::Left,
        "right" => LabelPos::Right,
        "above" => LabelPos::Above,
        "below" => LabelPos::Below,
        "best_fit" => LabelPos::BestFit,
        other => bail!(
            "data label `position` must be center, inside_end, inside_base, outside_end, left, right, above, below or best_fit, not \"{other}\""
        ),
    })
}

/// `data_labels`: true / false, or {show, position, format, size, bold,
/// italic, color}. `round`: a pie or doughnut (percentages by default).
fn data_labels(v: &Value, round: bool) -> Result<Option<DataLabels>> {
    let default_show = |l: &mut DataLabels| {
        if round {
            l.percent = true;
        } else {
            l.value = true;
        }
    };
    match v {
        Value::Bool(false) | Value::Null => Ok(None),
        Value::Bool(true) => {
            let mut l = DataLabels::default();
            default_show(&mut l);
            Ok(Some(l))
        }
        Value::Object(_) => {
            let o = object(v, "`data_labels`", LABEL_KEYS)?;
            let mut l = DataLabels::default();
            match get(o, "show") {
                None => default_show(&mut l),
                Some(s) => {
                    let items: Vec<&Value> = match s {
                        Value::Array(a) => a.iter().collect(),
                        one => vec![one],
                    };
                    for it in items {
                        match it.as_str() {
                            Some("value") => l.value = true,
                            Some("percent") if round => l.percent = true,
                            Some("percent") => {
                                bail!("data label `percent` is for pie and doughnut charts")
                            }
                            Some("category") => l.category = true,
                            Some("series") => l.series = true,
                            _ => {
                                bail!("data label `show` lists value, percent, category or series")
                            }
                        }
                    }
                }
            }
            if let Some(p) = opt_str(o, "position")? {
                l.pos = Some(label_pos(p)?);
            }
            l.num_fmt = opt_str(o, "format")?.map(str::to_owned);
            l.font = LabelFont {
                size: get(o, "size")
                    .map(font_size)
                    .transpose()?
                    .map(|s| s as f32 / 100.0),
                bold: opt_bool(o, "bold")?,
                italic: opt_bool(o, "italic")?,
                color: get(o, "color")
                    .map(|c| color(c, "color").and_then(|c| rgb_of(&c, "data_labels.color")))
                    .transpose()?,
            };
            Ok(Some(l))
        }
        _ => bail!("`data_labels` must be true, false or an object"),
    }
}

/// An axis: {title, min, max, major_unit, gridlines, number_format,
/// reverse, visible}.
struct Axis {
    title: String,
    options: AxisOptions,
    gridlines: Option<bool>,
    /// `visible: false`: no line, ticks or labels.
    hidden: bool,
}

fn axis(v: Option<&Value>, key: &str) -> Result<Axis> {
    let mut out = Axis {
        title: String::new(),
        options: AxisOptions::default(),
        gridlines: None,
        hidden: false,
    };
    let Some(v) = v else { return Ok(out) };
    let o = object(v, &format!("`{key}`"), AXIS_KEYS)?;
    out.title = opt_str(o, "title")?.unwrap_or_default().to_owned();
    out.options.min = opt_num(o, "min")?;
    out.options.max = opt_num(o, "max")?;
    out.options.major = opt_num(o, "major_unit")?;
    if out.options.major.is_some_and(|m| m <= 0.0) {
        bail!("`{key}.major_unit` must be more than 0");
    }
    if let (Some(a), Some(b)) = (out.options.min, out.options.max)
        && a >= b
    {
        bail!("`{key}.min` must be less than `max`");
    }
    out.options.num_fmt = opt_str(o, "number_format")?.map(str::to_owned);
    out.options.reverse = opt_bool(o, "reverse")?.unwrap_or(false);
    out.gridlines = opt_bool(o, "gridlines")?;
    out.hidden = opt_bool(o, "visible")? == Some(false);
    Ok(out)
}

fn category(v: &Value) -> Result<DataCategory> {
    Ok(match v {
        Value::String(s) => DataCategory::Text(s.clone()),
        Value::Number(n) => DataCategory::Number(
            n.as_f64()
                .filter(|x| x.is_finite())
                .context("a category number is out of range")?,
        ),
        Value::Null => DataCategory::Text(String::new()),
        _ => bail!("`categories` must hold strings or numbers"),
    })
}

fn values(v: &Value, n: usize, what: &str) -> Result<Vec<Option<f64>>> {
    let Some(a) = v.as_array() else {
        bail!("{what} `values` must be an array of numbers");
    };
    if a.len() != n {
        bail!("{what} has {} values for {n} categories", a.len());
    }
    a.iter()
        .map(|x| match x {
            Value::Null => Ok(None),
            Value::Number(n) => n
                .as_f64()
                .filter(|x| x.is_finite())
                .map(Some)
                .ok_or_else(|| anyhow!("{what} has a number out of range")),
            _ => bail!("{what} `values` must be numbers (or null for a gap)"),
        })
        .collect()
}

/// Per-point colours: an array of colours (null: automatic).
fn point_colors(v: &Value, n: usize, key: &str) -> Result<Vec<(usize, String)>> {
    let Some(a) = v.as_array() else {
        bail!("`{key}` must be an array of colours (null for automatic)");
    };
    if a.len() > n {
        bail!("`{key}` has {} colours for {n} categories", a.len());
    }
    let mut out = Vec::new();
    for (i, c) in a.iter().enumerate() {
        if !c.is_null() {
            out.push((i, fill_xml(c, key)?));
        }
    }
    Ok(out)
}

/// The chart's text formatting (`a:defRPr`): `font` over the deck's text
/// typeface and colour (not its size: charts keep their own).
fn text_props(v: Option<&Value>, cx: &Ctx) -> Result<Option<String>> {
    let mut face = cx.style.font.clone();
    let mut fill = cx.style.color.clone().map(|c| FillSpec::Solid(c).xml());
    let (mut size, mut bold, mut italic) = (None, None, None);
    if let Some(v) = v {
        let o = object(v, "`font`", FONT_KEYS)?;
        if let Some(f) = opt_str(o, "font")? {
            face = Some(f.to_owned());
        }
        if let Some(c) = get(o, "color") {
            fill = Some(fill_xml(c, "font.color")?);
        }
        size = get(o, "size").map(font_size).transpose()?;
        bold = opt_bool(o, "bold")?;
        italic = opt_bool(o, "italic")?;
    }
    if face.is_none() && fill.is_none() && size.is_none() && bold.is_none() && italic.is_none() {
        return Ok(None);
    }
    let mut x = String::from("<a:defRPr");
    if let Some(s) = size {
        x.push_str(&format!(" sz=\"{s}\""));
    }
    if let Some(b) = bold {
        x.push_str(&format!(" b=\"{}\"", u8::from(b)));
    }
    if let Some(i) = italic {
        x.push_str(&format!(" i=\"{}\"", u8::from(i)));
    }
    x.push('>');
    if let Some(f) = fill {
        x.push_str(&f);
    }
    if let Some(f) = face {
        let f = slides_engine_esc(&f);
        x.push_str(&format!(
            "<a:latin typeface=\"{f}\"/><a:ea typeface=\"{f}\"/><a:cs typeface=\"{f}\"/>"
        ));
    }
    x.push_str("</a:defRPr>");
    Ok(Some(x))
}

fn slides_engine_esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// What a chart element describes, before it is written.
pub struct ChartPlan {
    pub data: DataChart,
    pub xfrm: Xfrm,
    pub name: Option<String>,
}

/// Read a chart element (see the module docs).
pub fn plan(v: &Value, cx: &Ctx) -> Result<ChartPlan> {
    let o = object(v, "a chart", &keys_with(&[CHART_KEYS]))?;
    let ty = opt_str(o, "chart")?.context(
        "a chart needs `chart`: column, bar, line, area, pie, doughnut, scatter or combo",
    )?;
    let base = Base::parse(ty)?;
    let stacked = stacking(o)?;
    if stacked != Stacking::None && matches!(base, Base::Pie | Base::Doughnut | Base::Scatter) {
        bail!("a {ty} chart can't be stacked");
    }
    let x = opt_len(o, "x", cx)?.unwrap_or(0);
    let y = opt_len(o, "y", cx)?.unwrap_or(0);
    let w = need_len(o, "w", cx, "a chart")?;
    let h = need_len(o, "h", cx, "a chart")?;
    if w <= 0 || h <= 0 {
        bail!("a chart's `w` and `h` must be more than 0");
    }

    // Categories and series.
    let cats = get(o, "categories")
        .context(
            "a chart needs `categories` (the labels along the axis, or a scatter chart's x values)",
        )?
        .as_array()
        .context("`categories` must be an array")?;
    if cats.is_empty() || cats.len() > MAX_CATEGORIES {
        bail!("a chart needs 1 to {MAX_CATEGORIES} categories");
    }
    let categories: Vec<DataCategory> = cats.iter().map(category).collect::<Result<_>>()?;
    if base == Base::Scatter
        && !categories
            .iter()
            .all(|c| matches!(c, DataCategory::Number(_)))
    {
        bail!("a scatter chart's `categories` are its x values: numbers");
    }
    let n = categories.len();
    let list = get(o, "series")
        .context("a chart needs `series`: [{\"name\", \"values\"}, ...]")?
        .as_array()
        .context("`series` must be an array")?;
    if list.is_empty() || list.len() > MAX_SERIES {
        bail!("a chart needs 1 to {MAX_SERIES} series");
    }
    if base.is_round() && list.len() > 1 {
        bail!("a pie or doughnut chart shows one series (give one)");
    }
    let round = base.is_round();
    let chart_labels = get(o, "data_labels")
        .map(|v| data_labels(v, round))
        .transpose()?
        .flatten();
    let mut series = Vec::new();
    let mut types: Vec<SeriesType> = Vec::new();
    let mut mixed = base == Base::Combo;
    for (i, s) in list.iter().enumerate() {
        let what = format!("series {}", i + 1);
        let so = object(s, &what, SERIES_KEYS)?;
        let name = opt_str(so, "name")?
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Series {}", i + 1));
        let vals = values(
            get(so, "values").with_context(|| format!("{what} needs `values`"))?,
            n,
            &what,
        )?;
        let ty = opt_str(so, "type")?;
        let secondary = match opt_str(so, "axis")? {
            None | Some("primary") => false,
            Some("secondary") => true,
            Some(other) => bail!("{what} `axis` must be primary or secondary, not \"{other}\""),
        };
        if (ty.is_some() || secondary) && !base.combines() {
            bail!(
                "{what}: only column, line, area and combo charts mix series types or use a secondary axis"
            );
        }
        let default_ty = match base {
            Base::Line => "line",
            Base::Area => "area",
            Base::Combo if i > 0 => "line",
            _ => "column",
        };
        let ty = ty.unwrap_or(default_ty);
        if ty != default_ty || secondary {
            mixed = true;
        }
        types.push(SeriesType {
            kind: series_kind(ty, stacked != Stacking::None)?,
            secondary,
        });
        let fill = get(so, "color").map(|c| fill_xml(c, "color")).transpose()?;
        let line_xml = get(so, "line")
            .map(|l| line(l, cx, "line").map(|l| l.xml()))
            .transpose()?;
        let mut point_fills = Vec::new();
        if let Some(c) = get(so, "colors") {
            if !matches!(
                base,
                Base::Column | Base::Bar | Base::Pie | Base::Doughnut | Base::Combo
            ) {
                bail!(
                    "{what} `colors` (one per bar or slice) is for column, bar, pie and doughnut charts"
                );
            }
            point_fills = point_colors(c, n, "colors")?;
        }
        let labels = match get(so, "data_labels") {
            Some(v) => data_labels(v, round)?,
            None => chart_labels.clone(),
        };
        series.push(DataSeries {
            name,
            values: vals,
            fill,
            line: line_xml,
            point_fills,
            labels,
            refs: None,
        });
    }
    if let Some(c) = get(o, "colors") {
        if series.len() != 1
            || !matches!(base, Base::Column | Base::Bar | Base::Pie | Base::Doughnut)
        {
            bail!(
                "`colors` (one per bar or slice) is for a column, bar, pie or doughnut chart of one series; give a series its own `colors` otherwise"
            );
        }
        series[0].point_fills = point_colors(c, n, "colors")?;
    }

    // A target line: a dashed line series at one value.
    if let Some(t) = get(o, "target_line") {
        if !base.combines() || stacked == Stacking::Percent {
            bail!("`target_line` is for column, line, area and combo charts (not 100% stacked)");
        }
        let (value, label, col, width, dash) = match t {
            Value::Number(_) => (t.as_f64(), None, None, None, None),
            Value::Object(_) => {
                let to = object(t, "`target_line`", TARGET_KEYS)?;
                (
                    opt_num(to, "value")?,
                    opt_str(to, "label")?.map(str::to_owned),
                    get(to, "color")
                        .map(|c| color(c, "target_line.color"))
                        .transpose()?,
                    opt_len(to, "width", cx)?,
                    opt_str(to, "dash")?.map(str::to_owned),
                )
            }
            _ => bail!("`target_line` must be a number or {{value, label, color, width, dash}}"),
        };
        let value = value
            .filter(|v| v.is_finite())
            .context("`target_line` needs a `value`")?;
        if let Some(d) = &dash
            && !slides_engine::author::DASHES.contains(&d.as_str())
        {
            bail!(
                "`target_line.dash` must be one of {}",
                slides_engine::author::DASHES.join(", ")
            );
        }
        let ln = LineSpec {
            fill: Some(FillSpec::Solid(col.unwrap_or_else(|| Color::scheme("tx1")))),
            width: Some(width.unwrap_or(28_575)),
            dash: Some(dash.unwrap_or_else(|| "dash".into())),
            ..LineSpec::default()
        };
        series.push(DataSeries {
            name: label.unwrap_or_else(|| "Target".into()),
            values: vec![Some(value); n],
            line: Some(ln.xml()),
            ..DataSeries::default()
        });
        types.push(SeriesType {
            kind: ChartType::Line,
            secondary: false,
        });
        mixed = true;
    }

    let st = stacked != Stacking::None;
    let kind = if mixed {
        if stacked == Stacking::Percent {
            bail!("a combo chart can't be 100% stacked");
        }
        ChartType::Combo
    } else {
        match (base, st) {
            (Base::Column, false) => ChartType::Column,
            (Base::Column, true) => ChartType::StackedColumn,
            (Base::Bar, false) => ChartType::Bar,
            (Base::Bar, true) => ChartType::StackedBar,
            (Base::Line, false) => ChartType::Line,
            (Base::Line, true) => ChartType::StackedLine,
            (Base::Area, false) => ChartType::Area,
            (Base::Area, true) => ChartType::StackedArea,
            (Base::Pie | Base::Doughnut, _) => ChartType::Pie,
            (Base::Scatter, _) => ChartType::Scatter,
            (Base::Combo, _) => unreachable!("a combo is mixed"),
        }
    };

    // Axes: `axis` is the value axis (horizontal in a bar chart).
    let mut value_axis = axis(get(o, "axis"), "axis")?;
    let cat_axis = axis(get(o, "category_axis"), "category_axis")?;
    if round && (get(o, "axis").is_some() || get(o, "category_axis").is_some()) {
        bail!("a pie or doughnut chart has no axes");
    }
    if cat_axis.gridlines.is_some() {
        bail!("`category_axis.gridlines` isn't supported; gridlines belong to `axis`");
    }
    if base != Base::Scatter
        && (cat_axis.options.min.is_some()
            || cat_axis.options.max.is_some()
            || cat_axis.options.major.is_some())
    {
        bail!("`category_axis` min, max and major_unit are for scatter charts (x values)");
    }
    if stacked == Stacking::Percent && value_axis.options.num_fmt.is_none() {
        value_axis.options.num_fmt = Some("0%".into());
    }
    let horizontal_values = base == Base::Bar;
    let (x_axis, y_axis) = if horizontal_values {
        (value_axis.options.clone(), cat_axis.options.clone())
    } else {
        (cat_axis.options.clone(), value_axis.options.clone())
    };
    let (x_title, y_title) = if horizontal_values {
        (value_axis.title.clone(), cat_axis.title.clone())
    } else {
        (cat_axis.title.clone(), value_axis.title.clone())
    };

    let legend = match opt_str(o, "legend")? {
        None if series.len() > 1 || round => LegendPos::Bottom,
        None => LegendPos::None,
        Some("none") => LegendPos::None,
        Some("bottom") => LegendPos::Bottom,
        Some("top") => LegendPos::Top,
        Some("right") => LegendPos::Right,
        Some("left") => LegendPos::Left,
        Some(other) => bail!("`legend` must be none, bottom, top, right or left, not \"{other}\""),
    };
    let gap_width = opt_num(o, "gap_width")?;
    let overlap = opt_num(o, "overlap")?;
    if (gap_width.is_some() || overlap.is_some())
        && !matches!(base, Base::Column | Base::Bar | Base::Combo)
    {
        bail!("`gap_width` and `overlap` are for column, bar and combo charts");
    }
    if gap_width.is_some_and(|g| !(0.0..=500.0).contains(&g)) {
        bail!("`gap_width` must be 0 to 500 (% of a bar's width)");
    }
    if overlap.is_some_and(|g| !(-100.0..=100.0).contains(&g)) {
        bail!("`overlap` must be -100 to 100");
    }
    let hole = opt_num(o, "hole")?;
    if hole.is_some() && base != Base::Doughnut {
        bail!("`hole` is for doughnut charts");
    }
    if hole.is_some_and(|h| !(10.0..=90.0).contains(&h)) {
        bail!("`hole` must be 10 to 90 (% of the doughnut)");
    }
    let title = opt_str(o, "title")?.unwrap_or_default().to_owned();

    let spec = ChartSpec {
        kind,
        title,
        legend,
        by_rows: false,
        data: CellRange::from_corners((0, 0), (n as u32, series.len() as u32)),
        data_sheet: None,
        x_title,
        y_title,
        colors: Vec::new(),
        labels: Vec::new(),
        x_axis,
        y_axis,
        three_d: false,
        series_types: if kind == ChartType::Combo {
            types
        } else {
            Vec::new()
        },
    };
    let data = DataChart {
        spec,
        sheet: SHEET.into(),
        categories,
        series,
        percent: stacked == Stacking::Percent,
        doughnut: (base == Base::Doughnut).then(|| hole.unwrap_or(50.0).round() as u32),
        number_format: opt_str(o, "number_format")?.map(str::to_owned),
        gap_width: gap_width.map(|g| g.round() as u32),
        overlap: overlap.map(|g| g.round() as i32),
        gridlines: value_axis.gridlines,
        hidden_axes: [cat_axis.hidden, value_axis.hidden],
        text: text_props(get(o, "font"), cx)?,
        title_size: get(o, "font")
            .and_then(|f| f.get("size"))
            .map(font_size)
            .transpose()?
            .map(|s| s * 6 / 5),
        area: None,
        workbook_rel: Some("rId1".into()),
    };
    Ok(ChartPlan {
        data,
        xfrm: Xfrm::new(x, y, w, h),
        name: opt_str(o, "name")?.map(str::to_owned),
    })
}

/// The workbook embedded with chart `d`.
pub(super) fn workbook(d: &DataChart) -> Result<Vec<u8>> {
    let cats: Vec<CellValue> = d
        .categories
        .iter()
        .map(|c| match c {
            DataCategory::Text(t) if t.is_empty() => CellValue::Empty,
            DataCategory::Text(t) => CellValue::Text(t.clone()),
            DataCategory::Number(x) => CellValue::Number(*x),
        })
        .collect();
    let series: Vec<(String, Vec<Option<f64>>)> = d
        .series
        .iter()
        .map(|s| (s.name.clone(), s.values.clone()))
        .collect();
    chart_workbook(&d.sheet, &cats, &series, d.number_format.as_deref())
}

/// A chart element (`{"type": "chart", ...}`).
pub fn chart_element(v: &Value, cx: &Ctx) -> Result<Element> {
    let p = plan(v, cx)?;
    let xml = sheets_engine::data_chart_part(&p.data)?;
    let workbook = workbook(&p.data)?;
    Ok(Element {
        kind: ElementKind::Chart(ChartPartSpec {
            xml: Arc::new(xml.into_bytes()),
            workbook: Arc::new(workbook),
        }),
        name: p.name,
        xfrm: p.xfrm,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use slides_engine::Presentation;

    use super::*;

    fn cx() -> Ctx {
        Ctx {
            units: super::super::slides_author::Unit::In,
            ..Ctx::default()
        }
    }

    fn column() -> Value {
        json!({
            "type": "chart", "chart": "column", "x": 1, "y": 1, "w": 6, "h": 4,
            "title": "Revenue", "categories": ["Q1", "Q2", "Q3", "Q4"],
            "series": [
                {"name": "2025", "values": [10, 12.5, null, 14]},
                {"name": "2026", "values": [11, 13, 15, 16], "color": "accent2"}
            ],
            "number_format": "#,##0.0",
            "axis": {"min": 0, "max": 20, "major_unit": 5, "title": "IDR bn", "gridlines": false},
            "data_labels": {"show": "value", "position": "outside_end"},
            "gap_width": 80, "legend": "bottom", "font": {"size": 12, "color": "tx1"}
        })
    }

    #[std::prelude::v1::test]
    fn reads_a_column_chart() {
        let p = plan(&column(), &cx()).unwrap();
        assert_eq!(p.data.spec.kind, ChartType::Column);
        assert_eq!(p.data.series.len(), 2);
        assert_eq!(p.data.series[0].values[2], None);
        assert_eq!(p.data.spec.y_axis.max, Some(20.0));
        assert_eq!(p.data.spec.y_title, "IDR bn");
        assert_eq!(p.data.gridlines, Some(false));
        assert_eq!(p.data.gap_width, Some(80));
        assert!(
            p.data.series[1]
                .fill
                .as_deref()
                .unwrap()
                .contains("accent2")
        );
        assert_eq!(
            p.data.series[0].labels.as_ref().unwrap().pos,
            Some(LabelPos::OutsideEnd)
        );
        assert_eq!(p.xfrm.cx, 6 * 914_400);
        assert!(p.data.text.as_deref().unwrap().contains("sz=\"1200\""));
    }

    #[std::prelude::v1::test]
    fn reversed_bars_and_hidden_axes() {
        let bar = json!({"type": "chart", "chart": "bar", "w": 4, "h": 3,
            "categories": ["a", "b", "c"], "series": [{"values": [1, 2, 3]}],
            "category_axis": {"reverse": true}, "axis": {"visible": false}});
        let p = plan(&bar, &cx()).unwrap();
        assert!(p.data.spec.y_axis.reverse);
        assert_eq!(p.data.hidden_axes, [false, true]);
        let xml = sheets_engine::data_chart_part(&p.data).unwrap();
        let val = &xml[xml.find("<c:valAx>").unwrap()..];
        let val = &val[..val.find("</c:valAx>").unwrap()];
        assert!(val.contains("<c:crosses val=\"max\"/>"), "{val}");
        assert!(val.contains("<c:delete val=\"1\"/>"), "{val}");
        let col = json!({"type": "chart", "chart": "column", "w": 4, "h": 3,
            "categories": ["a", "b"], "series": [{"values": [1, 2]}],
            "category_axis": {"visible": false}});
        assert_eq!(plan(&col, &cx()).unwrap().data.hidden_axes, [true, false]);
        let bad = json!({"type": "chart", "chart": "column", "w": 4, "h": 3,
            "categories": ["a"], "series": [{"values": [1]}], "axis": {"visible": "no"}});
        assert!(plan(&bad, &cx()).is_err());
    }

    #[std::prelude::v1::test]
    fn reads_types_and_refuses_bad_input() {
        let bar = json!({"type": "chart", "chart": "bar", "w": 4, "h": 3, "stacked": "percent",
            "categories": ["a", "b"], "series": [{"values": [1, 2]}, {"values": [3, 4]}]});
        let p = plan(&bar, &cx()).unwrap();
        assert_eq!(p.data.spec.kind, ChartType::StackedBar);
        assert!(p.data.percent);
        assert_eq!(p.data.spec.x_axis.num_fmt.as_deref(), Some("0%"));
        assert_eq!(p.data.series[1].name, "Series 2");

        let combo = json!({"type": "chart", "chart": "column", "w": 4, "h": 3,
            "categories": ["a", "b"], "series": [{"values": [1, 2]},
            {"name": "Margin", "values": [0.1, 0.2], "type": "line", "axis": "secondary"}],
            "target_line": {"value": 1.5, "label": "Goal"}});
        let p = plan(&combo, &cx()).unwrap();
        assert_eq!(p.data.spec.kind, ChartType::Combo);
        assert_eq!(p.data.series.len(), 3);
        assert_eq!(p.data.spec.series_types[1].kind, ChartType::Line);
        assert!(p.data.spec.series_types[1].secondary);
        assert_eq!(p.data.series[2].name, "Goal");

        let pie = json!({"type": "chart", "chart": "doughnut", "w": 4, "h": 3, "hole": 60,
            "categories": ["a", "b"], "series": [{"values": [1, 2]}],
            "colors": ["#C00000", null], "data_labels": true});
        let p = plan(&pie, &cx()).unwrap();
        assert_eq!(p.data.doughnut, Some(60));
        assert_eq!(p.data.series[0].point_fills.len(), 1);
        assert!(p.data.series[0].labels.as_ref().unwrap().percent);

        for bad in [
            json!({"type": "chart", "chart": "radar", "w": 1, "h": 1, "categories": ["a"], "series": [{"values": [1]}]}),
            json!({"type": "chart", "chart": "column", "w": 1, "h": 1, "categories": ["a"], "series": [{"values": [1, 2]}]}),
            json!({"type": "chart", "chart": "pie", "w": 1, "h": 1, "categories": ["a"], "series": [{"values": [1]}, {"values": [2]}]}),
            json!({"type": "chart", "chart": "scatter", "w": 1, "h": 1, "categories": ["a"], "series": [{"values": [1]}]}),
            json!({"type": "chart", "chart": "column", "w": 1, "h": 1, "categories": ["a"], "series": [{"values": [1]}], "bogus": 1}),
            json!({"type": "chart", "chart": "bar", "w": 1, "h": 1, "categories": ["a"], "series": [{"values": [1], "type": "line"}]}),
        ] {
            assert!(plan(&bad, &cx()).is_err(), "{bad}");
        }
        let err = plan(
            &json!({"type": "chart", "chart": "column", "w": 1, "h": 1, "categories": ["a"], "series": [{"values": [1]}], "bogus": 1}),
            &cx(),
        )
        .err()
        .unwrap()
        .to_string();
        assert!(err.contains("allowed:"), "{err}");
    }

    /// tests/corpus/pptx/kantor_charts.pptx is docs/examples/slides/charts.json
    /// built (`kantor slides build docs/examples/slides/charts.json -o
    /// tests/corpus/pptx/kantor_charts.pptx`): the fidelity job validates it.
    #[std::prelude::v1::test]
    fn the_chart_corpus_deck_is_current() {
        use std::path::Path;
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let dir = root.join("docs/examples/slides");
        let deck: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("charts.json")).unwrap())
                .unwrap();
        let p = super::super::slides_author::build(&deck, Some(&dir)).unwrap();
        let bytes = p.to_bytes().unwrap();
        assert!(slides_engine::package_problems(&bytes).is_empty());
        let file = std::fs::read(root.join("tests/corpus/pptx/kantor_charts.pptx")).unwrap();
        // The parts, not the zip bytes: deflate output may differ between
        // platforms' compression back ends.
        fn parts(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
            use std::io::Read as _;
            let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
            let mut out = Vec::new();
            for i in 0..z.len() {
                let mut f = z.by_index(i).unwrap();
                let name = f.name().unwrap().into_owned();
                let mut b = Vec::new();
                f.read_to_end(&mut b).unwrap();
                if name.ends_with(".xlsx") {
                    for (n, x) in parts(&b) {
                        out.push((format!("{name}/{n}"), x));
                    }
                } else {
                    out.push((name, b));
                }
            }
            out
        }
        assert!(
            parts(&bytes) == parts(&file),
            "tests/corpus/pptx/kantor_charts.pptx is out of date: run `kantor slides build docs/examples/slides/charts.json -o tests/corpus/pptx/kantor_charts.pptx -f`"
        );
        // Every chart reads back with its type and series.
        let q = Presentation::from_bytes(bytes).unwrap();
        let listed = super::super::slides::list(&q, 0);
        let charts: Vec<&Value> = listed["slides"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|s| s["shapes"].as_array().unwrap())
            .filter(|s| s["kind"] == "chart")
            .collect();
        let kinds: Vec<&str> = charts
            .iter()
            .map(|c| c["chart"].as_str().unwrap())
            .collect();
        assert_eq!(
            kinds,
            [
                "combo",
                "bar",
                "line",
                "stacked area",
                "pie",
                "doughnut",
                "100% stacked column",
                "scatter",
                "combo"
            ]
        );
        assert_eq!(charts[0]["series"], json!(["2025", "2026", "Target 45"]));
        assert_eq!(charts[0]["categories"], 4);
    }

    #[std::prelude::v1::test]
    fn charts_read_back_from_the_saved_file() {
        let mut p = Presentation::new();
        let el = chart_element(&column(), &cx()).unwrap();
        let ix = p.add_element(0, &el).unwrap();
        let bytes = p.to_bytes().unwrap();
        let again = p.to_bytes().unwrap();
        assert_eq!(bytes, again, "the same deck saves to the same bytes");
        let back = Presentation::from_bytes(bytes).unwrap();
        let shape = back
            .slide(0)
            .shapes
            .iter()
            .position(|s| s.is_chart())
            .expect("a chart");
        assert_eq!(shape, ix);
        let data = back.chart_data(0, shape).unwrap();
        assert_eq!(data.categories, vec!["Q1", "Q2", "Q3", "Q4"]);
        assert_eq!(data.series[0].name, "2025");
        assert_eq!(
            data.series[1].values,
            vec![Some(11.0), Some(13.0), Some(15.0), Some(16.0)]
        );
        assert_eq!(data.series[0].values[2], None);
        assert_eq!(back.chart_kind(0, shape).as_deref(), Some("column"));
    }
}
