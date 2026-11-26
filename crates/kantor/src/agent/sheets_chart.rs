//! Charts of a workbook's cells (`add_chart`, a sheet's `charts` in
//! `kantor sheets build`). The keys are the slides charts' (see
//! [`super::slides_chart`] and docs/slides-json.md), with ranges where
//! slides take values: `data` (a block: series names along the top,
//! categories down the left), or `categories` and each series' `values`
//! and `name` as references. The part is written by the slides charts'
//! writer ([`sheets_engine::data_chart_part`]) with the series pointing at
//! the cells, so Excel and Kantor redraw the chart when the cells change.

use anyhow::{Context as _, Result, bail};
use serde_json::{Map, Value, json};
use sheets_engine::drawing::{Anchor, EditAs, Marker};
use sheets_engine::{CellRange, SeriesRefs, Workbook, column_name, quote_sheet_name};

use super::sheets_author::{get, opt_str, parse_ref, sheet_arg, used_part};
use super::slides_author::{Ctx, length};
use super::slides_chart::CHART_KEYS;

/// EMU per Excel pixel.
const EMU_PER_PX: i64 = 9525;

/// `'Data'!$B$2:$B$13`.
pub fn abs_ref(book: &Workbook, sheet: u32, r: CellRange) -> String {
    let q = quote_sheet_name(&book.sheet_names()[sheet as usize]);
    let a = format!("${}${}", column_name(r.left), r.top + 1);
    if r.is_single() {
        format!("{q}!{a}")
    } else {
        format!("{q}!{a}:${}${}", column_name(r.right), r.bottom + 1)
    }
}

/// One series before the chart is planned.
struct Series {
    name: String,
    name_ref: Option<String>,
    values: Vec<Option<f64>>,
    values_ref: String,
    /// Its other keys (colour, type, axis, labels...).
    extra: Map<String, Value>,
}

/// Cells of a one-row or one-column range, in order.
fn cells_of(r: CellRange) -> Vec<(u32, u32)> {
    if r.left == r.right {
        (r.top..=r.bottom).map(|row| (row, r.left)).collect()
    } else if r.top == r.bottom {
        (r.left..=r.right).map(|c| (r.top, c)).collect()
    } else {
        Vec::new()
    }
}

fn line_ref(book: &Workbook, text: &str, sheet: u32, what: &str) -> Result<(u32, CellRange)> {
    let (s, r) = parse_ref(book, text, sheet).with_context(|| what.to_owned())?;
    let r = used_part(book, s, r);
    if r.left != r.right && r.top != r.bottom {
        bail!("{what} \"{text}\" must be one row or one column of cells");
    }
    Ok((s, r))
}

fn numbers(book: &Workbook, s: u32, r: CellRange) -> Vec<Option<f64>> {
    cells_of(r)
        .into_iter()
        .map(|(row, c)| book.number_at(s, row, c).map(|n| n.0))
        .collect()
}

fn labels(book: &Workbook, s: u32, r: CellRange, scatter: bool) -> Vec<Value> {
    cells_of(r)
        .into_iter()
        .map(|(row, c)| match (scatter, book.number_at(s, row, c)) {
            (true, Some((n, _))) => super::sheets_author::number(n),
            _ => json!(book.display_value(s, row, c)),
        })
        .collect()
}

/// Categories, their cells, the series and the block (sheet, range).
type Block = (Vec<Value>, Option<String>, Vec<Series>, (u32, CellRange));

/// The chart's categories and series from `data` (a block).
fn from_block(book: &Workbook, o: &Map<String, Value>, sheet: u32, scatter: bool) -> Result<Block> {
    let text = opt_str(o, "data")?.context("no data")?;
    let (s, d) = parse_ref(book, text, sheet).context("`data`")?;
    let d = used_part(book, s, d);
    let rows = match opt_str(o, "series_in")?.unwrap_or("columns") {
        "columns" => false,
        "rows" => true,
        other => bail!("`series_in` is columns or rows, not \"{other}\""),
    };
    // Turn rows into columns so one code path reads both.
    let at = |i: u32, j: u32| -> (u32, u32) {
        if rows {
            (d.top + j, d.left + i)
        } else {
            (d.top + i, d.left + j)
        }
    };
    let (n_i, n_j) = if rows {
        (d.cols_count(), d.rows_count())
    } else {
        (d.rows_count(), d.cols_count())
    };
    if n_j < 1 || n_i < 1 {
        bail!("`data` {} is empty", d.name());
    }
    let is_text = |(r, c): (u32, u32)| book.number_at(s, r, c).is_none();
    // Names along the top when the top row (past the first column) is text.
    let header = n_i > 1 && (1.min(n_j - 1)..n_j).all(|j| is_text(at(0, j)));
    let first = u32::from(header);
    // Categories down the left: text or dates, or a scatter chart's x.
    let cat_col = n_j > 1
        && (scatter
            || (first..n_i).all(|i| {
                let (r, c) = at(i, 0);
                is_text((r, c)) || book.number_at(s, r, c).is_some_and(|n| n.1)
            }));
    let line = |j: u32| -> CellRange {
        let (a, b) = (at(first, j), at(n_i - 1, j));
        CellRange::from_corners(a, b)
    };
    if first >= n_i {
        bail!("`data` {} has names but no values", d.name());
    }
    let (categories, cat_ref) = if cat_col {
        let r = line(0);
        (labels(book, s, r, scatter), Some(abs_ref(book, s, r)))
    } else {
        (
            (first..n_i)
                .map(|i| json!((i - first + 1).to_string()))
                .collect(),
            None,
        )
    };
    let mut series = Vec::new();
    for j in u32::from(cat_col)..n_j {
        let r = line(j);
        let (name, name_ref) = if header {
            let (hr, hc) = at(0, j);
            (
                book.display_value(s, hr, hc),
                Some(abs_ref(
                    book,
                    s,
                    CellRange::from_corners((hr, hc), (hr, hc)),
                )),
            )
        } else {
            (format!("Series {}", series.len() + 1), None)
        };
        series.push(Series {
            name,
            name_ref,
            values: numbers(book, s, r),
            values_ref: abs_ref(book, s, r),
            extra: Map::new(),
        });
    }
    if series.is_empty() {
        bail!("`data` {} has no column of values", d.name());
    }
    Ok((categories, cat_ref, series, (s, d)))
}

/// A series given as `{name, values, ...}` with references.
fn given_series(
    book: &Workbook,
    v: &Value,
    i: usize,
    sheet: u32,
    n: Option<usize>,
) -> Result<Series> {
    let what = format!("series {}", i + 1);
    let o = v
        .as_object()
        .with_context(|| format!("{what} must be an object"))?;
    let mut extra = o.clone();
    extra.remove("name");
    extra.remove("values");
    let (values, values_ref) = match get(o, "values") {
        Some(Value::String(t)) => {
            let (s, r) = line_ref(book, t, sheet, &format!("{what} `values`"))?;
            (numbers(book, s, r), abs_ref(book, s, r))
        }
        Some(Value::Array(a)) => {
            if a.iter().any(|x| !x.is_null() && !x.is_number()) {
                bail!("{what} `values` are numbers or null");
            }
            (a.iter().map(Value::as_f64).collect(), String::new())
        }
        _ => bail!("{what} needs `values`: cells such as \"Data!B2:B13\" (or numbers)"),
    };
    if let Some(n) = n
        && values.len() != n
    {
        bail!("{what} has {} values for {n} categories", values.len());
    }
    let (name, name_ref) = match opt_str(o, "name")? {
        Some(t) if t.starts_with('=') => {
            let (s, r) = parse_ref(book, t, sheet).with_context(|| format!("{what} `name`"))?;
            (
                book.display_value(s, r.top, r.left),
                Some(abs_ref(
                    book,
                    s,
                    CellRange::from_corners((r.top, r.left), (r.top, r.left)),
                )),
            )
        }
        Some(t) => (t.to_owned(), None),
        None => (format!("Series {}", i + 1), None),
    };
    Ok(Series {
        name,
        name_ref,
        values,
        values_ref,
        extra,
    })
}

/// Where the chart goes: `at` (top-left cell), and `to` (bottom-right cell,
/// inclusive) or `w`/`h` (lengths), else 8 columns by 15 rows.
fn anchor(
    book: &Workbook,
    o: &Map<String, Value>,
    sheet: u32,
    beside: Option<CellRange>,
) -> Result<Anchor> {
    let from = match opt_str(o, "at")? {
        Some(t) => parse_ref(book, t, sheet).context("`at`")?.1,
        None => {
            let b = beside.unwrap_or(CellRange::from_corners((0, 0), (0, 0)));
            CellRange::from_corners((b.top, b.right + 2), (b.top, b.right + 2))
        }
    };
    let m = |row: u32, col: u32| Marker {
        col,
        col_off: 0,
        row,
        row_off: 0,
    };
    if let Some(t) = opt_str(o, "to")? {
        if get(o, "w").is_some() || get(o, "h").is_some() {
            bail!("give `to` or `w` and `h`, not both");
        }
        let to = parse_ref(book, t, sheet).context("`to`")?.1;
        if to.bottom < from.top || to.right < from.left {
            bail!("`to` must be below and right of `at`");
        }
        return Ok(Anchor::TwoCell {
            from: m(from.top, from.left),
            to: m(to.bottom + 1, to.right + 1),
            edit_as: EditAs::OneCell,
        });
    }
    match (get(o, "w"), get(o, "h")) {
        (None, None) => Ok(Anchor::TwoCell {
            from: m(from.top, from.left),
            to: m(from.top + 15, from.left + 8),
            edit_as: EditAs::OneCell,
        }),
        (Some(w), Some(h)) => {
            let unit = opt_str(o, "units")?;
            let len = |v: &Value, key: &str| -> Result<i64> {
                let v = match (v, unit) {
                    (Value::Number(n), Some(u)) => json!(format!("{n}{u}")),
                    _ => v.clone(),
                };
                let e = length(&v, &Ctx::default(), key)?;
                if e < 20 * EMU_PER_PX {
                    bail!("`{key}` is too small for a chart");
                }
                Ok(e)
            };
            Ok(Anchor::OneCell {
                from: m(from.top, from.left),
                cx: len(w, "w")?,
                cy: len(h, "h")?,
            })
        }
        _ => bail!("give both `w` and `h`"),
    }
}

/// `add_chart`.
pub(super) fn run(book: &mut Workbook, o: &Map<String, Value>) -> Result<(String, Value)> {
    let sheet = sheet_arg(book, o)?;
    let kind = opt_str(o, "chart")?.context("a chart needs `chart`")?;
    let scatter = kind == "scatter";
    let categories: Vec<Value>;
    let mut cat_ref = None;
    let mut series = Vec::new();
    let mut block = None;
    if get(o, "data").is_some() {
        let (c, r, s, b) = from_block(book, o, sheet, scatter)?;
        (categories, cat_ref, series, block) = (c, r, s, Some(b));
        // `series` given beside `data` style the block's series.
        if let Some(Value::Array(list)) = get(o, "series") {
            if list.len() > series.len() {
                bail!(
                    "`series` has {} entries for the {} series of `data`",
                    list.len(),
                    series.len()
                );
            }
            for (i, v) in list.iter().enumerate() {
                let m = v
                    .as_object()
                    .with_context(|| format!("series {} must be an object", i + 1))?;
                if m.contains_key("values") {
                    bail!(
                        "series {}: with `data`, series take their values from it",
                        i + 1
                    );
                }
                for (k, x) in m {
                    if k == "name" {
                        if let Some(t) = x.as_str() {
                            series[i].name = t.to_owned();
                            series[i].name_ref = None;
                        }
                    } else {
                        series[i].extra.insert(k.clone(), x.clone());
                    }
                }
            }
        }
        if get(o, "categories").is_some() {
            bail!("give `data`, or `categories` and `series`, not both");
        }
    } else {
        match get(o, "categories") {
            Some(Value::String(t)) => {
                let (s, r) = line_ref(book, t, sheet, "`categories`")?;
                categories = labels(book, s, r, scatter);
                cat_ref = Some(abs_ref(book, s, r));
            }
            Some(Value::Array(a)) => categories = a.clone(),
            Some(_) => bail!("`categories` is a range such as \"Data!A2:A13\" (or a list)"),
            None => bail!("a chart needs `data` (a block of cells), or `categories` and `series`"),
        }
        let list = get(o, "series").and_then(Value::as_array).context(
            "a chart needs `series`: [{\"name\": \"=Data!B1\", \"values\": \"Data!B2:B13\"}]",
        )?;
        for (i, v) in list.iter().enumerate() {
            series.push(given_series(book, v, i, sheet, Some(categories.len()))?);
        }
    }
    if categories.is_empty() || series.is_empty() {
        bail!("the chart has no data");
    }
    // The slides chart's JSON, values written out.
    let mut c = Map::new();
    for (k, v) in o {
        if CHART_KEYS.contains(&k.as_str())
            && !["series", "categories", "x", "y", "w", "h", "name"].contains(&k.as_str())
        {
            c.insert(k.clone(), v.clone());
        }
    }
    c.insert("w".into(), json!(1_000_000));
    c.insert("h".into(), json!(1_000_000));
    c.insert("categories".into(), json!(categories));
    let list: Vec<Value> = series
        .iter()
        .map(|s| {
            let mut m = s.extra.clone();
            m.insert("name".into(), json!(s.name));
            m.insert("values".into(), json!(s.values));
            Value::Object(m)
        })
        .collect();
    c.insert("series".into(), json!(list));
    let mut plan = super::slides_chart::plan(&Value::Object(c), &Ctx::default())?;
    for (i, ds) in plan.data.series.iter_mut().enumerate() {
        ds.refs = Some(match series.get(i) {
            Some(s) => SeriesRefs {
                name: s.name_ref.clone(),
                categories: cat_ref.clone(),
                values: s.values_ref.clone(),
            },
            // A target line: values in the chart.
            None => SeriesRefs {
                name: None,
                categories: cat_ref.clone(),
                values: String::new(),
            },
        });
    }
    plan.data.workbook_rel = None;
    // On a sheet a chart has Excel's white chart area: see-through, its dark
    // text is lost on Kantor's dark grid.
    plan.data
        .area
        .get_or_insert_with(|| sheets_engine::CHART_AREA_FILL.to_owned());
    let xml = sheets_engine::data_chart_part(&plan.data)?;
    let at = anchor(book, o, sheet, block.map(|b| b.1))?;
    let mut spec = plan.data.spec.clone();
    if let Some((s, d)) = block {
        spec.data = d;
        spec.data_sheet = (s != sheet).then(|| book.sheet_id(s));
    }
    book.insert_chart_part(sheet, at, xml, spec)?;
    let where_ = match at {
        Anchor::TwoCell { from, to, .. } => CellRange::from_corners(
            (from.row, from.col),
            (
                to.row.saturating_sub(1).max(from.row),
                to.col.saturating_sub(1).max(from.col),
            ),
        )
        .name(),
        Anchor::OneCell { from, .. } => sheets_engine::cell_name(from.row, from.col),
        Anchor::Absolute { .. } => String::new(),
    };
    let names: Vec<&str> = series.iter().map(|s| s.name.as_str()).collect();
    let cats = plan.data.categories.len();
    Ok((
        format!("added a {kind} chart at {where_}"),
        json!({"chart": kind, "at": where_, "series": names, "categories": cats}),
    ))
}
