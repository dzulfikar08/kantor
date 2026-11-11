//! Pivot tables (PivotTables): where they are, how their style draws their
//! cells, and refreshing them from their source.
//!
//! A pivot table's cells are ordinary cells of the sheet (IronCalc keeps
//! them); its definition (`xl/pivotTables/pivotTableN.xml`) and its cache
//! (`xl/pivotCache/pivotCacheDefinitionN.xml` with the records) are carried
//! through saves by `preserve`, which also moves them with inserted and
//! deleted rows and columns. This module reads them when a file opens:
//!
//! * **Geometry and look.** The location and page-field area of each pivot
//!   table, and the role of each of its rows and columns (header, row
//!   subheading, subtotal, grand total) from the definition's `rowItems` and
//!   `colItems`. The built-in `PivotStyleLight1-28`, `Medium1-28` and
//!   `Dark1-28` are described by family, coloured by the workbook's theme as
//!   [`crate::table`] does for table styles (an approximation: Excel's exact
//!   per-style details are not all reproduced).
//! * **Refresh.** The cache is rebuilt from its worksheet source (a range on
//!   a sheet, a table or a defined name): fields from the header row, shared
//!   items and records. Each pivot table on the cache is then laid out again
//!   the way Excel lays it out: row and column fields (compact, outline or
//!   tabular form per field), items in the definition's order with new ones
//!   after them, hidden items and report filters, collapsed items, subtotals
//!   at the top or bottom, grand totals, several data fields (the `Σ Values`
//!   field) with sum, count, average, max, min, product, countNums, stdDev,
//!   stdDevp, var and varp. The cells are rewritten as one undo step, and the
//!   definition, cache and records are rewritten for the next save.
//!   Not refreshed (an error says why, nothing changes): non-worksheet
//!   sources, calculated fields and items, grouped fields (dates by month...),
//!   value filters, "show values as", OLAP.
//!
//! Refreshes are kept per history state (like drawing edits), so undo and
//! redo bring back the matching definitions; positions are kept with the
//! [`Layout`] they were made in and moved to the current one when used.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use roxmltree::{Document, Node};

use crate::format::LineStyle;
use crate::shift::{Area, Layout};
use crate::table::{Edges, Elem, TableCellLook, line, theme, thin};
use crate::{CellRange, Workbook, cell_name};

const NS_MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

// ------------------------------------------------------------------ values

/// A value in a pivot cache.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Val {
    Str(String),
    Num(f64),
    Bool(bool),
    /// A date as a serial number.
    Date(f64),
    Err(String),
    Missing,
}

impl Val {
    fn parse(n: Node) -> Option<Val> {
        let v = n.attribute("v");
        Some(match n.tag_name().name() {
            "s" => Val::Str(v.unwrap_or_default().to_owned()),
            "n" => Val::Num(v?.parse().ok()?),
            "b" => Val::Bool(matches!(v?, "1" | "true")),
            "d" => Val::Date(serial_of_iso(v?)?),
            "e" => Val::Err(v.unwrap_or_default().to_owned()),
            "m" => Val::Missing,
            _ => return None,
        })
    }

    /// The key two values are the same item by (strings ignore case, as in
    /// Excel).
    fn key(&self) -> String {
        match self {
            Val::Str(s) => format!("s{}", s.to_lowercase()),
            Val::Num(n) => format!("n{n}"),
            Val::Bool(b) => format!("b{b}"),
            Val::Date(d) => format!("d{d}"),
            Val::Err(e) => format!("e{e}"),
            Val::Missing => "m".into(),
        }
    }

    fn number(&self) -> Option<f64> {
        match self {
            Val::Num(n) | Val::Date(n) => Some(*n),
            _ => None,
        }
    }

    /// What a label cell shows (`(blank)` for a missing value).
    fn label(&self) -> String {
        match self {
            Val::Str(s) => s.clone(),
            Val::Num(n) => format_number(*n),
            Val::Bool(true) => "TRUE".into(),
            Val::Bool(false) => "FALSE".into(),
            Val::Date(d) => iso_of_serial(*d).replace("T00:00:00", ""),
            Val::Err(e) => e.clone(),
            Val::Missing => "(blank)".into(),
        }
    }

    /// The cell input that shows the value.
    fn input(&self) -> String {
        match self {
            Val::Num(n) => format_number(*n),
            Val::Date(_) | Val::Bool(_) | Val::Err(_) => self.label(),
            Val::Str(_) | Val::Missing => text_input(&self.label()),
        }
    }

    /// The order items sort in (Excel: numbers, then text, then booleans,
    /// then errors, blanks last).
    fn sort_key(&self) -> (u8, f64, String) {
        match self {
            Val::Num(n) | Val::Date(n) => (0, *n, String::new()),
            Val::Str(s) => (1, 0.0, s.to_lowercase()),
            Val::Bool(b) => (2, f64::from(u8::from(*b)), String::new()),
            Val::Err(e) => (3, 0.0, e.clone()),
            Val::Missing => (4, 0.0, String::new()),
        }
    }

    fn xml(&self, out: &mut String) {
        match self {
            Val::Str(s) => out.push_str(&format!("<s v=\"{}\"/>", esc(s))),
            Val::Num(n) => out.push_str(&format!("<n v=\"{}\"/>", format_number(*n))),
            Val::Bool(b) => out.push_str(&format!("<b v=\"{}\"/>", u8::from(*b))),
            Val::Date(d) => out.push_str(&format!("<d v=\"{}\"/>", iso_of_serial(*d))),
            Val::Err(e) => out.push_str(&format!("<e v=\"{}\"/>", esc(e))),
            Val::Missing => out.push_str("<m/>"),
        }
    }
}

fn format_number(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Text as cell input that stays text (an apostrophe in front of what would
/// read as a number, date, formula or boolean).
fn text_input(s: &str) -> String {
    let first = s.chars().next();
    let risky = matches!(first, Some(c) if c.is_ascii_digit() || "=+-'$.€(%@".contains(c))
        || s.eq_ignore_ascii_case("true")
        || s.eq_ignore_ascii_case("false");
    if risky { format!("'{s}") } else { s.to_owned() }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Days from 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Serial day 0 is 1899-12-30 (Excel's 1900 date system, after its
/// non-existent 1900-02-29).
const SERIAL_EPOCH: i64 = -25569;

fn serial_of_iso(s: &str) -> Option<f64> {
    let (date, time) = s.split_once('T').unwrap_or((s, ""));
    let mut p = date.split('-');
    let y: i64 = p.next()?.parse().ok()?;
    let m: i64 = p.next()?.parse().ok()?;
    let d: i64 = p.next()?.parse().ok()?;
    let mut serial = (days_from_civil(y, m, d) - SERIAL_EPOCH) as f64;
    if !time.is_empty() {
        let t: Vec<f64> = time.split(':').filter_map(|x| x.parse().ok()).collect();
        if let [h, mi, sec] = t[..] {
            serial += (h * 3600.0 + mi * 60.0 + sec) / 86400.0;
        }
    }
    Some(serial)
}

fn iso_of_serial(serial: f64) -> String {
    let days = serial.floor();
    let (y, m, d) = civil_from_days(days as i64 + SERIAL_EPOCH);
    let secs = ((serial - days) * 86400.0).round() as i64;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

// ------------------------------------------------------------------ the model

/// Where a cache's data comes from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Source {
    /// A range on a sheet (by name), in the coordinates of `at`.
    Range { sheet: String, area: Area },
    /// A table or a defined name.
    Name(String),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CacheField {
    pub name: String,
    /// Shared items (what pivot fields' `x` index), if the cache lists them.
    pub items: Vec<Val>,
    pub num_fmt_id: u32,
}

/// A pivot cache as the workbook has it now.
#[derive(Debug, Clone)]
pub(crate) struct Cache {
    pub part: String,
    pub records_part: Option<String>,
    /// The definition as read (its other elements and attributes are kept).
    pub xml: Arc<String>,
    pub source: Option<Source>,
    pub fields: Vec<CacheField>,
    /// Records, when the cache was refreshed (values by field).
    pub records: Option<Arc<Vec<Vec<Val>>>>,
    /// The layout `source` is in.
    pub at: Layout,
    /// Why it can't be refreshed.
    pub unsupported: Option<String>,
}

/// An entry of a pivot field's item list.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FieldItem {
    /// Index into the cache field's shared items; `None` for subtotal items.
    pub x: Option<usize>,
    pub hidden: bool,
    /// `default`, `sum`... for subtotal items.
    pub t: Option<String>,
    /// Details shown (`sd`): `false` for a collapsed item.
    pub expanded: bool,
    /// A caption replacing the value.
    pub n: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Field {
    pub axis: Option<String>,
    pub data: bool,
    pub items: Vec<FieldItem>,
    pub compact: bool,
    pub outline: bool,
    pub subtotal_top: bool,
    pub default_subtotal: bool,
    pub show_all: bool,
    /// `ascending` / `descending` / `manual`.
    pub sort: Option<String>,
    /// A caption replacing the field name.
    pub name: Option<String>,
    /// The `<pivotField>` element as read (its other attributes are kept).
    pub xml: Option<String>,
}

/// A row or column item (`<i>`): which items of the axis fields it shows.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AxisItem {
    /// `data` for a plain row, `default` for a subtotal, `grand`, `blank`...
    pub t: String,
    /// How many leading items repeat the previous `<i>`'s.
    pub r: usize,
    /// The data field (for the `Σ Values` field).
    pub i: usize,
    /// Indexes into the fields' item lists, from level `r` on.
    pub x: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Func {
    Sum,
    Count,
    Average,
    Max,
    Min,
    Product,
    CountNums,
    StdDev,
    StdDevp,
    Var,
    Varp,
}

impl Func {
    fn parse(s: &str) -> Option<Func> {
        Some(match s {
            "sum" => Func::Sum,
            "count" => Func::Count,
            "average" => Func::Average,
            "max" => Func::Max,
            "min" => Func::Min,
            "product" => Func::Product,
            "countNums" => Func::CountNums,
            "stdDev" => Func::StdDev,
            "stdDevp" => Func::StdDevp,
            "var" => Func::Var,
            "varp" => Func::Varp,
            _ => return None,
        })
    }

    /// What Excel calls it in a data field's default name.
    fn caption(self) -> &'static str {
        match self {
            Func::Sum => "Sum",
            Func::Count => "Count",
            Func::Average => "Average",
            Func::Max => "Max",
            Func::Min => "Min",
            Func::Product => "Product",
            Func::CountNums => "Count",
            Func::StdDev => "StdDev",
            Func::StdDevp => "StdDevp",
            Func::Var => "Var",
            Func::Varp => "Varp",
        }
    }

    /// The function over `vals`; `None` shows an empty cell. A function
    /// that has no answer (an average of no numbers) gives `Err`.
    fn apply(self, vals: &[&Val]) -> Option<std::result::Result<f64, &'static str>> {
        if vals.is_empty() {
            return None;
        }
        let nums: Vec<f64> = vals.iter().filter_map(|v| v.number()).collect();
        let n = nums.len() as f64;
        let mean = || nums.iter().sum::<f64>() / n;
        let var = |sample: bool| {
            let m = mean();
            let ss: f64 = nums.iter().map(|x| (x - m) * (x - m)).sum();
            ss / if sample { n - 1.0 } else { n }
        };
        Some(match self {
            Func::Sum => Ok(nums.iter().sum()),
            Func::Count => Ok(vals.iter().filter(|v| **v != &Val::Missing).count() as f64),
            Func::CountNums => Ok(n),
            Func::Average if nums.is_empty() => Err("#DIV/0!"),
            Func::Average => Ok(mean()),
            Func::Max => Ok(nan0(nums.iter().copied().fold(f64::NAN, f64::max))),
            Func::Min => Ok(nan0(nums.iter().copied().fold(f64::NAN, f64::min))),
            Func::Product if nums.is_empty() => Ok(0.0),
            Func::Product => Ok(nums.iter().product()),
            Func::StdDev | Func::Var if nums.len() < 2 => Err("#DIV/0!"),
            Func::StdDevp | Func::Varp if nums.is_empty() => Err("#DIV/0!"),
            Func::StdDev => Ok(var(true).sqrt()),
            Func::StdDevp => Ok(var(false).sqrt()),
            Func::Var => Ok(var(true)),
            Func::Varp => Ok(var(false)),
        })
    }
}

fn nan0(x: f64) -> f64 {
    if x.is_nan() { 0.0 } else { x }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DataField {
    pub name: Option<String>,
    pub fld: usize,
    pub func: Func,
    pub num_fmt_id: Option<u32>,
    /// The `<dataField>` element as read.
    pub xml: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PageField {
    pub fld: usize,
    /// The selected item (an index into the field's items), if one.
    pub item: Option<usize>,
    pub xml: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct StyleInfo {
    pub name: Option<String>,
    pub row_headers: bool,
    pub col_headers: bool,
    pub row_stripes: bool,
    pub col_stripes: bool,
    pub last_column: bool,
}

/// A pivot table definition.
#[derive(Debug, Clone)]
pub(crate) struct Def {
    pub part: String,
    pub sheet_id: u32,
    pub name: String,
    /// Index of its cache in [`Pivots::caches`].
    pub cache: usize,
    /// In the coordinates of `at`.
    pub location: Area,
    pub first_header_row: u32,
    pub first_data_row: u32,
    pub first_data_col: u32,
    pub row_page_count: u32,
    pub col_page_count: u32,
    pub fields: Vec<Field>,
    /// Field indexes; `-2` is the `Σ Values` field.
    pub row_fields: Vec<i32>,
    pub col_fields: Vec<i32>,
    pub row_items: Vec<AxisItem>,
    pub col_items: Vec<AxisItem>,
    pub page_fields: Vec<PageField>,
    pub data_fields: Vec<DataField>,
    pub row_grand: bool,
    pub col_grand: bool,
    pub style: StyleInfo,
    pub grand_caption: Option<String>,
    pub row_header_caption: Option<String>,
    pub col_header_caption: Option<String>,
    pub data_caption: String,
    /// The definition as read.
    pub xml: Arc<String>,
    /// Item indexes changed since the file was read: drop `<formats>` and
    /// the like, which point at items by index.
    pub reindexed: bool,
    pub at: Layout,
    pub unsupported: Option<String>,
}

fn attr_bool(n: Node, name: &str, default: bool) -> bool {
    match n.attribute(name) {
        Some("1" | "true") => true,
        Some("0" | "false") => false,
        _ => default,
    }
}

fn attr_u32(n: Node, name: &str) -> Option<u32> {
    n.attribute(name)?.parse().ok()
}

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

fn children<'a, 'i>(n: Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = Node<'a, 'i>> + 'a {
    n.children()
        .filter(move |c| c.is_element() && c.tag_name().name() == name)
}

fn area_of(r: &str) -> Option<Area> {
    let r = r.split_whitespace().next()?;
    Some(crate::shift::parse_range(r)?.area())
}

fn parse_cache(part: &str, records_part: Option<String>, xml: &str) -> Result<Cache, String> {
    let doc = Document::parse(xml).map_err(|e| format!("{part}: {e}"))?;
    let root = doc.root_element();
    let mut unsupported = None;
    let source = child(root, "cacheSource");
    let source = match source {
        Some(s) if s.attribute("type") == Some("worksheet") => child(s, "worksheetSource")
            .and_then(|w| {
                match (
                    w.attribute("name"),
                    w.attribute("ref"),
                    w.attribute("sheet"),
                ) {
                    (Some(n), _, _) => Some(Source::Name(n.to_owned())),
                    (None, Some(r), Some(sheet)) => Some(Source::Range {
                        sheet: sheet.to_owned(),
                        area: area_of(r)?,
                    }),
                    _ => None,
                }
            }),
        _ => None,
    };
    if source.is_none() {
        unsupported = Some("its data doesn't come from a worksheet in this workbook".to_owned());
    } else if let Some(w) = root
        .descendants()
        .find(|n| n.tag_name().name() == "worksheetSource")
        && w.attribute((NS_R, "id")).is_some()
    {
        unsupported = Some("its data comes from another workbook".to_owned());
    }
    let mut fields = Vec::new();
    if let Some(cf) = child(root, "cacheFields") {
        for f in children(cf, "cacheField") {
            if f.attribute("formula").is_some() || f.attribute("databaseField") == Some("0") {
                unsupported.get_or_insert_with(|| "it has a calculated field".to_owned());
            }
            if child(f, "fieldGroup").is_some() {
                unsupported.get_or_insert_with(|| {
                    "it has grouped items (dates by month, number ranges...)".to_owned()
                });
            }
            let items = child(f, "sharedItems")
                .map(|s| {
                    s.children()
                        .filter(|c| c.is_element())
                        .filter_map(Val::parse)
                        .collect()
                })
                .unwrap_or_default();
            fields.push(CacheField {
                name: f.attribute("name").unwrap_or_default().to_owned(),
                items,
                num_fmt_id: attr_u32(f, "numFmtId").unwrap_or(0),
            });
        }
    }
    if child(root, "calculatedItems").is_some() {
        unsupported.get_or_insert_with(|| "it has calculated items".to_owned());
    }
    Ok(Cache {
        part: part.to_owned(),
        records_part,
        xml: Arc::new(xml.to_owned()),
        source,
        fields,
        records: None,
        at: Layout::default(),
        unsupported,
    })
}

fn parse_items(list: Option<Node>) -> Vec<AxisItem> {
    let Some(list) = list else {
        return Vec::new();
    };
    children(list, "i")
        .map(|i| AxisItem {
            t: i.attribute("t").unwrap_or("data").to_owned(),
            r: attr_u32(i, "r").unwrap_or(0) as usize,
            i: attr_u32(i, "i").unwrap_or(0) as usize,
            x: children(i, "x")
                .map(|x| attr_u32(x, "v").unwrap_or(0) as usize)
                .collect(),
        })
        .collect()
}

fn parse_def(part: &str, sheet_id: u32, cache: usize, xml: &str) -> Result<Def, String> {
    let doc = Document::parse(xml).map_err(|e| format!("{part}: {e}"))?;
    let root = doc.root_element();
    let mut unsupported = None;
    let loc = child(root, "location").ok_or_else(|| format!("{part}: no location"))?;
    let location = loc
        .attribute("ref")
        .and_then(area_of)
        .ok_or_else(|| format!("{part}: bad location"))?;
    let mut fields = Vec::new();
    if let Some(pf) = child(root, "pivotFields") {
        for f in children(pf, "pivotField") {
            for sub in [
                "sumSubtotal",
                "countASubtotal",
                "avgSubtotal",
                "maxSubtotal",
                "minSubtotal",
                "productSubtotal",
                "countSubtotal",
                "stdDevSubtotal",
                "stdDevPSubtotal",
                "varSubtotal",
                "varPSubtotal",
            ] {
                if attr_bool(f, sub, false) {
                    unsupported.get_or_insert_with(|| "a field has custom subtotals".to_owned());
                }
            }
            let items: Vec<FieldItem> = child(f, "items")
                .map(|its| {
                    children(its, "item")
                        .map(|it| FieldItem {
                            x: it
                                .attribute("t")
                                .is_none()
                                .then(|| attr_u32(it, "x").unwrap_or(0) as usize),
                            hidden: attr_bool(it, "h", false),
                            t: it.attribute("t").map(str::to_owned),
                            expanded: attr_bool(it, "sd", true),
                            n: it.attribute("n").map(str::to_owned),
                        })
                        .collect()
                })
                .unwrap_or_default();
            if child(f, "items")
                .is_some_and(|its| children(its, "item").any(|i| i.attribute("f").is_some()))
            {
                unsupported.get_or_insert_with(|| "it has calculated items".to_owned());
            }
            fields.push(Field {
                axis: f.attribute("axis").map(str::to_owned),
                data: attr_bool(f, "dataField", false),
                items,
                compact: attr_bool(f, "compact", true),
                outline: attr_bool(f, "outline", true),
                subtotal_top: attr_bool(f, "subtotalTop", true),
                default_subtotal: attr_bool(f, "defaultSubtotal", true),
                show_all: attr_bool(f, "showAll", true),
                sort: f.attribute("sortType").map(str::to_owned),
                name: f.attribute("name").map(str::to_owned),
                xml: Some(xml[f.range()].to_owned()),
            });
        }
    }
    let field_list = |name: &str| -> Vec<i32> {
        child(root, name)
            .map(|l| {
                children(l, "field")
                    .filter_map(|f| f.attribute("x")?.parse().ok())
                    .collect()
            })
            .unwrap_or_default()
    };
    let page_fields = child(root, "pageFields")
        .map(|l| {
            children(l, "pageField")
                .filter_map(|p| {
                    Some(PageField {
                        fld: attr_u32(p, "fld")? as usize,
                        item: attr_u32(p, "item").map(|i| i as usize),
                        xml: Some(xml[p.range()].to_owned()),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let mut data_fields = Vec::new();
    if let Some(l) = child(root, "dataFields") {
        for d in children(l, "dataField") {
            if d.attribute("showDataAs").is_some_and(|s| s != "normal") {
                unsupported.get_or_insert_with(|| "a value is shown as a calculation".to_owned());
            }
            let func = d.attribute("subtotal").unwrap_or("sum");
            let Some(func) = Func::parse(func) else {
                unsupported.get_or_insert_with(|| format!("it summarises by {func}"));
                continue;
            };
            let Some(fld) = attr_u32(d, "fld") else {
                continue;
            };
            data_fields.push(DataField {
                name: d.attribute("name").map(str::to_owned),
                fld: fld as usize,
                func,
                num_fmt_id: attr_u32(d, "numFmtId"),
                xml: Some(xml[d.range()].to_owned()),
            });
        }
    }
    if child(root, "filters").is_some() {
        unsupported.get_or_insert_with(|| "it has label or value filters".to_owned());
    }
    if child(root, "pivotHierarchies").is_some() {
        unsupported.get_or_insert_with(|| "it is an OLAP PivotTable".to_owned());
    }
    let style = child(root, "pivotTableStyleInfo")
        .map(|s| StyleInfo {
            name: s.attribute("name").map(str::to_owned),
            row_headers: attr_bool(s, "showRowHeaders", true),
            col_headers: attr_bool(s, "showColHeaders", true),
            row_stripes: attr_bool(s, "showRowStripes", false),
            col_stripes: attr_bool(s, "showColStripes", false),
            last_column: attr_bool(s, "showLastColumn", true),
        })
        .unwrap_or_default();
    Ok(Def {
        part: part.to_owned(),
        sheet_id,
        name: root.attribute("name").unwrap_or_default().to_owned(),
        cache,
        location,
        first_header_row: attr_u32(loc, "firstHeaderRow").unwrap_or(1),
        first_data_row: attr_u32(loc, "firstDataRow").unwrap_or(1),
        first_data_col: attr_u32(loc, "firstDataCol").unwrap_or(1),
        row_page_count: attr_u32(loc, "rowPageCount").unwrap_or(0),
        col_page_count: attr_u32(loc, "colPageCount").unwrap_or(0),
        fields,
        row_fields: field_list("rowFields"),
        col_fields: field_list("colFields"),
        row_items: parse_items(child(root, "rowItems")),
        col_items: parse_items(child(root, "colItems")),
        page_fields,
        data_fields,
        row_grand: attr_bool(root, "rowGrandTotals", true),
        col_grand: attr_bool(root, "colGrandTotals", true),
        style,
        grand_caption: root.attribute("grandTotalCaption").map(str::to_owned),
        row_header_caption: root.attribute("rowHeaderCaption").map(str::to_owned),
        col_header_caption: root.attribute("colHeaderCaption").map(str::to_owned),
        data_caption: root.attribute("dataCaption").unwrap_or("Values").to_owned(),
        xml: Arc::new(xml.to_owned()),
        reindexed: false,
        at: Layout::default(),
        unsupported,
    })
}

/// The pivot tables and caches of the opened file.
#[derive(Debug, Clone, Default)]
pub(crate) struct Pivots {
    pub defs: Vec<Arc<Def>>,
    pub caches: Vec<Arc<Cache>>,
    /// The caches' records parts as opened, kept from the first refresh on
    /// (a save after it writes every pivot part for the current state, so
    /// undoing a refresh after saving it brings these back).
    pub records: HashMap<String, Arc<Vec<u8>>>,
    pub refreshed: bool,
}

impl Pivots {
    pub(crate) fn read(bytes: &[u8]) -> Self {
        let sources = match crate::preserve::pivot_sources(bytes) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("pivot tables not read: {e}");
                return Self::default();
            }
        };
        let mut out = Pivots::default();
        let mut cache_index: HashMap<String, usize> = HashMap::new();
        for s in sources {
            let cache = match cache_index.get(&s.cache_part) {
                Some(&i) => i,
                None => match parse_cache(&s.cache_part, s.records_part.clone(), &s.cache_xml) {
                    Ok(c) => {
                        out.caches.push(Arc::new(c));
                        cache_index.insert(s.cache_part.clone(), out.caches.len() - 1);
                        out.caches.len() - 1
                    }
                    Err(e) => {
                        log::warn!("pivot cache not read: {e}");
                        continue;
                    }
                },
            };
            match parse_def(&s.part, s.sheet_id, cache, &s.xml) {
                Ok(d) => out.defs.push(Arc::new(d)),
                Err(e) => log::warn!("pivot table not read: {e}"),
            }
        }
        out
    }
}

/// Refreshed definitions and caches, by index, for one history state.
#[derive(Debug, Clone, Default)]
pub(crate) struct State {
    pub defs: HashMap<usize, Arc<LaidXml>>,
    pub caches: HashMap<usize, Arc<Cache>>,
}

// ------------------------------------------------------------------ geometry

/// What a row or column of a pivot table's body shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Line {
    Body,
    /// An outer item's own row (compact and outline form), by level.
    Subheading(u8),
    /// A subtotal, by level.
    Subtotal(u8),
    Grand,
    Blank,
}

fn line_of(it: &AxisItem, fields: usize) -> Line {
    let depth = it.r + it.x.len();
    let level = depth.saturating_sub(1).min(2) as u8;
    match it.t.as_str() {
        "grand" => Line::Grand,
        "blank" => Line::Blank,
        "data" if depth < fields => Line::Subheading(level),
        "data" => Line::Body,
        _ => Line::Subtotal(level),
    }
}

/// A pivot table as the sheet shows it now.
#[derive(Debug, Clone)]
pub(crate) struct Placed {
    pub index: usize,
    pub def: Arc<Def>,
    pub area: Area,
    /// The report filters above it.
    pub page: Option<Area>,
    rows: Vec<Line>,
    cols: Vec<Line>,
    style: Option<Arc<PivotStyleDef>>,
}

/// The page-field area of a pivot at `area`: `rowPageCount` rows of
/// filters (label and value, a column between groups) and a blank row.
fn page_area(def: &Def, area: Area) -> Option<Area> {
    let n = def.page_fields.len() as u32;
    if n == 0 {
        return None;
    }
    let rows = if def.row_page_count > 0 {
        def.row_page_count
    } else {
        n
    };
    let cols = def.col_page_count.max(1);
    if area.top < rows + 1 {
        return None;
    }
    Some(Area {
        top: area.top - rows - 1,
        bottom: area.top - 2,
        left: area.left,
        right: area.left + cols * 3 - 2,
    })
}

impl Placed {
    fn new(index: usize, def: Arc<Def>, area: Area, style: Option<Arc<PivotStyleDef>>) -> Self {
        let rows = def
            .row_items
            .iter()
            .map(|i| line_of(i, def.row_fields.len()))
            .collect();
        let cols = def
            .col_items
            .iter()
            .map(|i| line_of(i, def.col_fields.len()))
            .collect();
        let page = page_area(&def, area);
        Placed {
            index,
            def,
            area,
            page,
            rows,
            cols,
            style,
        }
    }

    pub fn contains(&self, row: u32, col: u32) -> bool {
        let inside = |a: &Area| row >= a.top && row <= a.bottom && col >= a.left && col <= a.right;
        inside(&self.area) || self.page.as_ref().is_some_and(inside)
    }

    /// The whole of it: the body and the report filters.
    pub fn bounds(&self) -> Area {
        match self.page {
            Some(p) => Area {
                top: p.top,
                left: self.area.left,
                bottom: self.area.bottom,
                right: self.area.right.max(p.right),
            },
            None => self.area,
        }
    }

    fn look(&self, row: u32, col: u32) -> Option<TableCellLook> {
        let s = self.style.as_ref()?;
        let d = &self.def;
        let a = self.area;
        let mut look = TableCellLook::default();
        if let Some(p) = self.page
            && row >= p.top
            && row <= p.bottom
        {
            let label = (col - p.left).is_multiple_of(3);
            let e = if label { &s.page_label } else { &s.page_value };
            look.apply(e, Edges::all());
            return Some(look);
        }
        let edges = Edges {
            top: row == a.top,
            bottom: row == a.bottom,
            left: col == a.left,
            right: col == a.right,
        };
        look.apply(&s.whole, edges);
        let data_top = a.top + d.first_data_row;
        let data_left = a.left + d.first_data_col;
        let row_edges = Edges {
            top: true,
            bottom: true,
            left: col == a.left,
            right: col == a.right,
        };
        let col_edges = Edges {
            top: row == data_top,
            bottom: row == a.bottom,
            left: true,
            right: true,
        };
        let row_line = row
            .checked_sub(data_top)
            .and_then(|i| self.rows.get(i as usize))
            .copied();
        let col_line = col
            .checked_sub(data_left)
            .and_then(|i| self.cols.get(i as usize))
            .copied();
        if d.style.row_stripes && row_line == Some(Line::Body) && (row - data_top).is_multiple_of(2)
        {
            look.apply(&s.row_stripe, row_edges);
        }
        if d.style.col_stripes
            && col_line == Some(Line::Body)
            && (col - data_left).is_multiple_of(2)
        {
            look.apply(&s.col_stripe, col_edges);
        }
        if d.style.row_headers && col < data_left && row >= data_top {
            look.apply(&s.first_col, col_edges);
        }
        if row < data_top {
            if d.style.col_headers {
                look.apply(&s.header, row_edges);
            }
            if row == a.top && col == a.left {
                look.apply(&s.first_header_cell, Edges::all());
            }
        }
        if let Some(Line::Subtotal(l)) = col_line {
            look.apply(&s.subtotal_col[l as usize], col_edges);
        }
        match row_line {
            Some(Line::Subheading(l)) => look.apply(&s.subheading[l as usize], row_edges),
            Some(Line::Subtotal(l)) => look.apply(&s.subtotal_row[l as usize], row_edges),
            _ => {}
        }
        if d.style.last_column && col_line == Some(Line::Grand) {
            look.apply(&s.last_col, col_edges);
        }
        if row_line == Some(Line::Grand) {
            look.apply(&s.total, row_edges);
        }
        Some(look)
    }
}

impl Edges {
    fn all() -> Self {
        Edges {
            top: true,
            bottom: true,
            left: true,
            right: true,
        }
    }
}

// ------------------------------------------------------------------ styles

/// A pivot table style: its elements, from the lowest priority up.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct PivotStyleDef {
    whole: Elem,
    row_stripe: Elem,
    col_stripe: Elem,
    first_col: Elem,
    header: Elem,
    first_header_cell: Elem,
    subtotal_col: [Elem; 3],
    subheading: [Elem; 3],
    subtotal_row: [Elem; 3],
    last_col: Elem,
    total: Elem,
    page_label: Elem,
    page_value: Elem,
}

/// The built-in style `name` (`PivotStyleLight16`...), coloured by `t`.
pub(crate) fn builtin(name: &str, t: &ironcalc_base::types::Theme) -> Option<PivotStyleDef> {
    let (family, n) = if let Some(n) = name.strip_prefix("PivotStyleLight") {
        ("light", n)
    } else if let Some(n) = name.strip_prefix("PivotStyleMedium") {
        ("medium", n)
    } else {
        ("dark", name.strip_prefix("PivotStyleDark")?)
    };
    let n: u32 = n.parse().ok()?;
    if !(1..=28).contains(&n) {
        return None;
    }
    let white = theme(t, 0, 0.0);
    let black = theme(t, 1, 0.0);
    let (block, k) = ((n - 1) / 7, (n - 1) % 7);
    // Colour index within a block of 7: 0 is dark 1 (greys), 1-6 the accents.
    let accent = |tint: f64| {
        if k == 0 {
            theme(
                t,
                1,
                if tint == 0.0 {
                    0.0
                } else {
                    tint.max(0.0) * 0.9 + 0.05
                },
            )
        } else {
            theme(t, 3 + k as i32, tint)
        }
    };
    let base = if k == 0 { black } else { accent(0.0) };
    let mut s = PivotStyleDef::default();
    s.header.bold = Some(true);
    s.total.bold = Some(true);
    s.page_label.bold = Some(true);
    for e in s.subheading.iter_mut().chain(s.subtotal_row.iter_mut()) {
        e.bold = Some(true);
    }
    match (family, block) {
        ("light", 0) => {
            s.whole.top = Some(thin(base));
            s.whole.bottom = Some(thin(base));
            s.header.bottom = Some(thin(base));
            s.total.top = Some(thin(base));
            s.subtotal_row[0].top = Some(thin(base));
        }
        ("light", 1) => {
            let b = Some(thin(base));
            s.whole.top = b;
            s.whole.bottom = b;
            s.whole.left = b;
            s.whole.right = b;
            s.header.fill = Some(base);
            s.header.font = Some(white);
            s.total.top = Some(line(LineStyle::Double, base));
            s.subtotal_row[0].top = b;
        }
        ("light", 2) => {
            // Light 16 is Excel's default: tinted header and grand total
            // row, lines under the header and above the total.
            s.header.fill = Some(accent(0.8));
            s.header.bottom = Some(thin(base));
            s.whole.top = Some(thin(base));
            s.whole.bottom = Some(thin(base));
            s.total.fill = Some(accent(0.8));
            s.total.top = Some(thin(base));
            s.page_label.fill = Some(accent(0.8));
            s.page_value.fill = Some(accent(0.8));
        }
        ("light", _) => {
            let b = Some(thin(accent(0.4)));
            s.whole.top = b;
            s.whole.bottom = b;
            s.whole.left = b;
            s.whole.right = b;
            s.whole.horizontal = b;
            s.whole.vertical = b;
            s.header.bottom = Some(line(LineStyle::Medium, base));
            s.total.top = Some(line(LineStyle::Double, base));
        }
        ("medium", 0) => {
            s.header.fill = Some(base);
            s.header.font = Some(white);
            s.total.fill = Some(accent(0.6));
            s.total.top = Some(line(LineStyle::Double, base));
            s.subheading[0].fill = Some(accent(0.8));
            s.whole.horizontal = Some(thin(accent(0.4)));
        }
        ("medium", 1) => {
            s.whole.fill = Some(accent(0.8));
            s.whole.horizontal = Some(thin(white));
            s.header.fill = Some(base);
            s.header.font = Some(white);
            s.total.fill = Some(base);
            s.total.font = Some(white);
            s.subtotal_row[0].fill = Some(accent(0.6));
            s.subheading[0].fill = Some(accent(0.6));
            s.page_label.fill = Some(accent(0.6));
            s.page_value.fill = Some(accent(0.8));
        }
        ("medium", 2) => {
            s.header.fill = Some(base);
            s.header.font = Some(white);
            s.whole.horizontal = Some(thin(accent(0.4)));
            s.whole.bottom = Some(thin(base));
            s.total.top = Some(line(LineStyle::Double, base));
            s.subheading[0].top = Some(thin(base));
        }
        ("medium", _) => {
            let l = Some(thin(accent(0.4)));
            s.whole.fill = Some(accent(0.8));
            s.whole.top = l;
            s.whole.bottom = l;
            s.whole.horizontal = l;
            s.header.bottom = Some(line(LineStyle::Medium, base));
            s.total.top = Some(line(LineStyle::Double, base));
            s.subheading[0].fill = Some(accent(0.6));
        }
        _ => {
            let body = if k == 0 {
                theme(t, 1, 0.35)
            } else {
                accent(-0.25)
            };
            let band = if k == 0 {
                theme(t, 1, 0.15)
            } else {
                accent(-0.5)
            };
            s.whole.fill = Some(body);
            s.whole.font = Some(white);
            s.header.fill = Some(black);
            s.header.font = Some(white);
            s.total.fill = Some(band);
            s.total.top = Some(line(LineStyle::Double, white));
            s.subheading[0].fill = Some(band);
            s.page_label.fill = Some(band);
            s.page_label.font = Some(white);
        }
    }
    s.first_header_cell = s.header.clone();
    s.last_col.bold = Some(true);
    Some(s)
}

// ------------------------------------------------------------------ refresh

/// A field level of an axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Field(usize),
    Values,
}

fn levels(fields: &[i32], fields_len: usize) -> Vec<Level> {
    fields
        .iter()
        .filter_map(|&f| match f {
            -2 => Some(Level::Values),
            f if f >= 0 && (f as usize) < fields_len => Some(Level::Field(f as usize)),
            _ => None,
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Data,
    Sub,
    Grand,
}

/// A row or column of the body.
#[derive(Debug, Clone)]
struct Entry {
    kind: Kind,
    /// Item positions per level (the data field index on the Values level).
    path: Vec<usize>,
    /// Records it sums up.
    recs: Vec<usize>,
    /// Whether its cells show values (an outline header row with subtotals
    /// at the bottom shows none).
    values: bool,
    /// The data field, if a Values level is on the path (or a grand total
    /// per data field).
    data: Option<usize>,
}

/// Rows or columns of an axis, in order.
fn entries(
    def: &Def,
    lv: &[Level],
    pos: &[Vec<Option<usize>>],
    recs: &[usize],
    grand: bool,
) -> Vec<Entry> {
    let mut out = Vec::new();
    if lv.is_empty() {
        out.push(Entry {
            kind: Kind::Data,
            path: Vec::new(),
            recs: recs.to_vec(),
            values: true,
            data: None,
        });
        return out;
    }
    walk(def, lv, pos, recs, 0, &mut Vec::new(), None, &mut out);
    let real = lv.iter().any(|l| matches!(l, Level::Field(_)));
    if grand && real {
        if lv.contains(&Level::Values) {
            for d in 0..def.data_fields.len() {
                out.push(Entry {
                    kind: Kind::Grand,
                    path: Vec::new(),
                    recs: recs.to_vec(),
                    values: true,
                    data: Some(d),
                });
            }
        } else {
            out.push(Entry {
                kind: Kind::Grand,
                path: Vec::new(),
                recs: recs.to_vec(),
                values: true,
                data: None,
            });
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn walk(
    def: &Def,
    lv: &[Level],
    pos: &[Vec<Option<usize>>],
    recs: &[usize],
    level: usize,
    path: &mut Vec<usize>,
    data: Option<usize>,
    out: &mut Vec<Entry>,
) {
    let leaf = level + 1 == lv.len();
    let groups: Vec<(usize, Vec<usize>, Option<usize>, &Field)> = match lv[level] {
        Level::Values => {
            static PLAIN: std::sync::OnceLock<Field> = std::sync::OnceLock::new();
            let plain = PLAIN.get_or_init(|| Field {
                compact: true,
                outline: true,
                subtotal_top: true,
                default_subtotal: false,
                ..Default::default()
            });
            (0..def.data_fields.len())
                .map(|d| (d, recs.to_vec(), Some(d), plain))
                .collect()
        }
        Level::Field(f) => {
            let field = &def.fields[f];
            let mut by_item: Vec<Vec<usize>> = vec![Vec::new(); field.items.len()];
            for &r in recs {
                if let Some(p) = pos[f][r] {
                    by_item[p].push(r);
                }
            }
            field
                .items
                .iter()
                .enumerate()
                .filter(|(_, it)| it.t.is_none() && !it.hidden)
                .map(|(k, _)| (k, std::mem::take(&mut by_item[k]), data, field))
                .filter(|(_, sub, _, _)| field.show_all || !sub.is_empty())
                .collect()
        }
    };
    for (k, sub, d, field) in groups {
        path.push(k);
        let expanded = match lv[level] {
            Level::Field(_) => field.items[k].expanded,
            Level::Values => true,
        };
        if leaf || !expanded {
            out.push(Entry {
                kind: Kind::Data,
                path: path.clone(),
                recs: sub,
                values: true,
                data: d,
            });
        } else {
            let tabular = !field.outline;
            let top = field.subtotal_top && field.default_subtotal;
            if !tabular {
                out.push(Entry {
                    kind: Kind::Data,
                    path: path.clone(),
                    recs: sub.clone(),
                    values: top,
                    data: d,
                });
            }
            walk(def, lv, pos, &sub, level + 1, path, d, out);
            if field.default_subtotal && (tabular || !field.subtotal_top) {
                out.push(Entry {
                    kind: Kind::Sub,
                    path: path.clone(),
                    recs: sub,
                    values: true,
                    data: d,
                });
            }
        }
        path.pop();
    }
}

/// How many leading levels an entry repeats from the previous data entry.
fn repeats(prev: Option<&Entry>, e: &Entry) -> usize {
    if e.kind != Kind::Data || e.path.is_empty() {
        return 0;
    }
    let Some(p) = prev else { return 0 };
    let common = p
        .path
        .iter()
        .zip(&e.path)
        .take_while(|(a, b)| a == b)
        .count();
    common.min(e.path.len() - 1)
}

fn items_xml(entries: &[Entry], lv: &[Level]) -> (String, Vec<AxisItem>) {
    let mut xml = String::new();
    let mut items = Vec::new();
    let mut prev: Option<&Entry> = None;
    for e in entries {
        let (t, r, x) = match e.kind {
            Kind::Data => {
                let r = repeats(prev, e);
                prev = Some(e);
                ("data", r, e.path[r..].to_vec())
            }
            Kind::Sub => ("default", 0, e.path.clone()),
            Kind::Grand => ("grand", 0, vec![0]),
        };
        let x = if lv.is_empty() { Vec::new() } else { x };
        let i = e.data.unwrap_or(0);
        let mut a = String::new();
        if t != "data" {
            a.push_str(&format!(" t=\"{t}\""));
        }
        if r > 0 {
            a.push_str(&format!(" r=\"{r}\""));
        }
        if i > 0 {
            a.push_str(&format!(" i=\"{i}\""));
        }
        if x.is_empty() {
            xml.push_str(&format!("<i{a}/>"));
        } else {
            xml.push_str(&format!("<i{a}>"));
            for v in &x {
                if *v == 0 {
                    xml.push_str("<x/>");
                } else {
                    xml.push_str(&format!("<x v=\"{v}\"/>"));
                }
            }
            xml.push_str("</i>");
        }
        items.push(AxisItem {
            t: t.to_owned(),
            r,
            i,
            x,
        });
    }
    (xml, items)
}

/// A pivot table laid out: its new definition and its cells.
pub(crate) struct Laid {
    pub def: Def,
    /// The XML lists the layout rewrites (`rowItems`, `colItems`...).
    pub row_items_xml: String,
    pub col_items_xml: String,
    /// `(row, col, input)` of every cell of the body and the report filters.
    pub cells: Vec<(u32, u32, String)>,
    /// Value cells with a data field's number format.
    pub num_fmts: Vec<(Area, u32)>,
}

impl Def {
    fn field_caption(&self, cache: &Cache, f: usize) -> String {
        self.fields
            .get(f)
            .and_then(|x| x.name.clone())
            .or_else(|| cache.fields.get(f).map(|c| c.name.clone()))
            .unwrap_or_default()
    }

    fn data_caption_of(&self, cache: &Cache, d: usize) -> String {
        let df = &self.data_fields[d];
        df.name.clone().unwrap_or_else(|| {
            format!(
                "{} of {}",
                df.func.caption(),
                self.field_caption(cache, df.fld)
            )
        })
    }

    fn item_label(&self, cache: &Cache, f: usize, k: usize) -> Val {
        let it = &self.fields[f].items[k];
        if let Some(n) = &it.n {
            return Val::Str(n.clone());
        }
        it.x.and_then(|x| cache.fields[f].items.get(x).cloned())
            .unwrap_or(Val::Missing)
    }
}

/// Lay out `def` (already matched to `cache`) with its top-left corner at
/// `(top, left)`.
pub(crate) fn lay_out(def: &Def, cache: &Cache, records: &[Vec<Val>], top: u32, left: u32) -> Laid {
    let nf = cache.fields.len();
    // Each record's item position in each field.
    let mut key_index: Vec<HashMap<String, usize>> = Vec::with_capacity(nf);
    for f in 0..nf {
        let mut by_x: HashMap<usize, usize> = HashMap::new();
        if let Some(field) = def.fields.get(f) {
            for (k, it) in field.items.iter().enumerate() {
                if let Some(x) = it.x {
                    by_x.insert(x, k);
                }
            }
        }
        key_index.push(
            cache.fields[f]
                .items
                .iter()
                .enumerate()
                .filter_map(|(x, v)| Some((v.key(), *by_x.get(&x)?)))
                .collect(),
        );
    }
    let pos: Vec<Vec<Option<usize>>> = (0..nf)
        .map(|f| {
            records
                .iter()
                .map(|r| r.get(f).and_then(|v| key_index[f].get(&v.key()).copied()))
                .collect()
        })
        .collect();
    let row_lv = levels(&def.row_fields, nf);
    let col_lv = levels(&def.col_fields, nf);
    // Records the filters let through.
    let axis_fields: Vec<usize> = row_lv
        .iter()
        .chain(&col_lv)
        .filter_map(|l| match l {
            Level::Field(f) => Some(*f),
            Level::Values => None,
        })
        .chain(def.page_fields.iter().map(|p| p.fld).filter(|&f| f < nf))
        .collect();
    let recs: Vec<usize> = (0..records.len())
        .filter(|&r| {
            axis_fields.iter().all(|&f| match pos[f][r] {
                Some(k) => !def.fields[f].items[k].hidden,
                None => false,
            }) && def.page_fields.iter().all(|p| match p.item {
                Some(sel) if p.fld < nf => pos[p.fld][r] == Some(sel),
                _ => true,
            })
        })
        .collect();
    let rows = entries(def, &row_lv, &pos, &recs, def.row_grand);
    let cols = entries(def, &col_lv, &pos, &recs, def.col_grand);

    // Columns of the row labels: a compact field shares its parent's.
    let mut label_col = Vec::with_capacity(row_lv.len());
    for (i, l) in row_lv.iter().enumerate() {
        let compact = |l: &Level| match l {
            Level::Field(f) => def.fields[*f].compact && def.fields[*f].outline,
            Level::Values => true,
        };
        let outline = |l: &Level| match l {
            Level::Field(f) => def.fields[*f].outline,
            Level::Values => true,
        };
        let c = if i == 0 {
            0
        } else if compact(l) && outline(&row_lv[i - 1]) {
            label_col[i - 1]
        } else {
            label_col[i - 1] + 1
        };
        label_col.push(c);
    }
    let label_cols = label_col.last().map_or(1, |c| c + 1);
    let real_cols = col_lv.iter().any(|l| matches!(l, Level::Field(_)));
    let header_rows = if real_cols {
        1 + col_lv.len() as u32
    } else {
        1
    };
    let first_header_row = if col_lv.is_empty() || real_cols { 1 } else { 0 };

    let mut cells: Vec<(u32, u32, String)> = Vec::new();
    let mut put = |r: u32, c: u32, s: String| {
        if !s.is_empty() {
            cells.push((r, c, s));
        }
    };
    let label = |lv: &Level, k: usize| -> Val {
        match lv {
            Level::Field(f) => def.item_label(cache, *f, k),
            Level::Values => Val::Str(def.data_caption_of(cache, k)),
        }
    };
    let grand_label = |e: &Entry| match (e.data, &def.grand_caption) {
        (Some(d), _) if def.data_fields.len() > 1 => {
            format!("Total {}", def.data_caption_of(cache, d))
        }
        (_, Some(c)) => c.clone(),
        _ => "Grand Total".to_owned(),
    };
    let data_left = left + label_cols;
    let data_top = top + header_rows;

    // Header rows.
    let hr = top + header_rows - 1;
    if !row_lv.is_empty() {
        if label_cols == 1 && compact_all(def, &row_lv) {
            put(
                hr,
                left,
                text_input(def.row_header_caption.as_deref().unwrap_or("Row Labels")),
            );
        } else {
            let mut done = Vec::new();
            for (i, l) in row_lv.iter().enumerate() {
                if done.contains(&label_col[i]) {
                    continue;
                }
                done.push(label_col[i]);
                let name = match l {
                    Level::Field(f) => def.field_caption(cache, *f),
                    Level::Values => def.data_caption.clone(),
                };
                put(hr, left + label_col[i], text_input(&name));
            }
        }
    }
    if real_cols {
        if def.data_fields.len() == 1 {
            put(top, left, text_input(&def.data_caption_of(cache, 0)));
        }
        put(
            top,
            data_left,
            text_input(def.col_header_caption.as_deref().unwrap_or("Column Labels")),
        );
        let mut prev: Option<&Entry> = None;
        for (j, e) in cols.iter().enumerate() {
            let c = data_left + j as u32;
            match e.kind {
                Kind::Grand => put(top + 1, c, text_input(&grand_label(e))),
                Kind::Sub => {
                    let l = e.path.len() - 1;
                    let v = label(&col_lv[l], e.path[l]).label();
                    put(top + 1 + l as u32, c, text_input(&format!("{v} Total")));
                }
                Kind::Data => {
                    let r = repeats(prev, e);
                    prev = Some(e);
                    for (l, k) in e.path.iter().enumerate().skip(r) {
                        put(top + 1 + l as u32, c, label(&col_lv[l], *k).input());
                    }
                }
            }
        }
    } else {
        for (j, e) in cols.iter().enumerate() {
            let d = e.data.or((def.data_fields.len() == 1).then_some(0));
            if let Some(d) = d {
                put(
                    hr,
                    data_left + j as u32,
                    text_input(&def.data_caption_of(cache, d)),
                );
            }
        }
    }

    // Row labels.
    let mut prev: Option<&Entry> = None;
    for (i, e) in rows.iter().enumerate() {
        let r = data_top + i as u32;
        match e.kind {
            Kind::Grand => put(r, left, text_input(&grand_label(e))),
            Kind::Sub => {
                let l = e.path.len() - 1;
                let v = label(&row_lv[l], e.path[l]).label();
                put(r, left + label_col[l], text_input(&format!("{v} Total")));
            }
            Kind::Data => {
                let rep = repeats(prev, e);
                prev = Some(e);
                let last = e.path.len().saturating_sub(1);
                for (l, k) in e.path.iter().enumerate().skip(rep) {
                    // An outer level shows here only when its field is in
                    // tabular form (else it has its own row above).
                    let own_row = match row_lv[l] {
                        Level::Field(f) => def.fields[f].outline,
                        Level::Values => true,
                    };
                    if l < last && own_row {
                        continue;
                    }
                    put(r, left + label_col[l], label(&row_lv[l], *k).input());
                }
            }
        }
    }

    // Values.
    let matches = |e: &Entry, lv: &[Level], r: usize| {
        e.path.iter().enumerate().all(|(l, k)| match lv[l] {
            Level::Field(f) => pos[f][r] == Some(*k),
            Level::Values => true,
        })
    };
    let mut num_fmts: Vec<(Area, u32)> = Vec::new();
    if !def.data_fields.is_empty() {
        for (i, re) in rows.iter().enumerate() {
            if !re.values {
                continue;
            }
            for (j, ce) in cols.iter().enumerate() {
                if !ce.values {
                    continue;
                }
                let d = re.data.or(ce.data).unwrap_or(0);
                let df = &def.data_fields[d];
                let vals: Vec<&Val> = re
                    .recs
                    .iter()
                    .filter(|&&r| ce.kind == Kind::Grand || matches(ce, &col_lv, r))
                    .filter_map(|&r| records[r].get(df.fld))
                    .collect();
                let v = match df.func.apply(&vals) {
                    None => continue,
                    Some(Ok(x)) => format_number(x),
                    Some(Err(e)) => e.to_owned(),
                };
                put(data_top + i as u32, data_left + j as u32, v);
            }
        }
        // Number formats, by column (or row) of each data field.
        let bottom = data_top + rows.len() as u32 - 1;
        let right = data_left + cols.len() as u32 - 1;
        let fmt = |d: usize| def.data_fields[d].num_fmt_id.filter(|&n| n != 0);
        if def.data_fields.len() == 1 {
            if let Some(n) = fmt(0) {
                num_fmts.push((
                    Area {
                        top: data_top,
                        left: data_left,
                        bottom,
                        right,
                    },
                    n,
                ));
            }
        } else if col_lv.contains(&Level::Values) {
            for (j, e) in cols.iter().enumerate() {
                if let Some(n) = e.data.and_then(fmt) {
                    let c = data_left + j as u32;
                    num_fmts.push((
                        Area {
                            top: data_top,
                            left: c,
                            bottom,
                            right: c,
                        },
                        n,
                    ));
                }
            }
        } else {
            for (i, e) in rows.iter().enumerate() {
                if let Some(n) = e.data.and_then(fmt) {
                    let r = data_top + i as u32;
                    num_fmts.push((
                        Area {
                            top: r,
                            left: data_left,
                            bottom: r,
                            right,
                        },
                        n,
                    ));
                }
            }
        }
    }

    // Report filters, above.
    let pages = def.page_fields.len() as u32;
    if pages > 0 && top > pages {
        for (k, p) in def.page_fields.iter().enumerate() {
            let r = top - pages - 1 + k as u32;
            if p.fld >= nf {
                continue;
            }
            put(r, left, text_input(&def.field_caption(cache, p.fld)));
            let value = match p.item {
                Some(sel) if sel < def.fields[p.fld].items.len() => {
                    def.item_label(cache, p.fld, sel).input()
                }
                _ if def.fields[p.fld].items.iter().any(|i| i.hidden) => {
                    "(Multiple Items)".to_owned()
                }
                _ => "(All)".to_owned(),
            };
            put(r, left + 1, value);
        }
    }

    let (row_items_xml, row_items) = items_xml(&rows, &row_lv);
    let (col_items_xml, col_items) = items_xml(&cols, &col_lv);
    let mut def = def.clone();
    def.location = Area {
        top,
        left,
        bottom: data_top + rows.len() as u32 - 1,
        right: data_left + cols.len() as u32 - 1,
    };
    def.first_header_row = first_header_row;
    def.first_data_row = header_rows;
    def.first_data_col = label_cols;
    def.row_items = row_items;
    def.col_items = col_items;
    def.row_page_count = if pages > 0 { pages } else { 0 };
    def.col_page_count = u32::from(pages > 0);
    Laid {
        def,
        row_items_xml,
        col_items_xml,
        cells,
        num_fmts,
    }
}

fn compact_all(def: &Def, lv: &[Level]) -> bool {
    lv.iter().all(|l| match l {
        Level::Field(f) => def.fields[*f].compact && def.fields[*f].outline,
        Level::Values => true,
    })
}

// ------------------------------------------------------------------ the cache

/// Records read from a source range: the header row's names and the rows
/// under it.
pub(crate) struct SourceData {
    pub names: Vec<String>,
    pub rows: Vec<Vec<Val>>,
}

/// The cache rebuilt from `data`: fields in the old order of their names,
/// shared items keeping the old order with new ones after them. `axis`
/// lists the fields pivot tables put on an axis (they need item lists).
pub(crate) fn rebuild_cache(old: &Cache, data: SourceData, axis: &[String]) -> Cache {
    let mut fields = Vec::with_capacity(data.names.len());
    for (f, name) in data.names.iter().enumerate() {
        let prev = old
            .fields
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name));
        let mut items: Vec<Val> = Vec::new();
        let mut seen: HashMap<String, ()> = HashMap::new();
        let column = data.rows.iter().map(|r| &r[f]);
        let needs_items = prev.is_some_and(|p| !p.items.is_empty())
            || axis.iter().any(|a| a.eq_ignore_ascii_case(name))
            || data
                .rows
                .iter()
                .any(|r| matches!(r[f], Val::Str(_) | Val::Bool(_) | Val::Err(_)));
        if needs_items {
            let present: HashMap<String, ()> = column.clone().map(|v| (v.key(), ())).collect();
            if let Some(p) = prev {
                for v in &p.items {
                    if present.contains_key(&v.key()) && seen.insert(v.key(), ()).is_none() {
                        items.push(v.clone());
                    }
                }
            }
            for v in column {
                if seen.insert(v.key(), ()).is_none() {
                    items.push(v.clone());
                }
            }
        }
        fields.push(CacheField {
            name: name.clone(),
            items,
            num_fmt_id: prev.map_or(0, |p| p.num_fmt_id),
        });
    }
    Cache {
        fields,
        records: Some(Arc::new(data.rows)),
        unsupported: None,
        ..old.clone()
    }
}

/// `def` matched to the rebuilt cache `new` (it was made for `old`): fields
/// by name, items by value; fields gone leave the axes, new items go after
/// the known ones (in order for sorted fields).
pub(crate) fn rematch(def: &Def, old: &Cache, new: &Cache) -> Def {
    let mut out = def.clone();
    let map: Vec<Option<usize>> = old
        .fields
        .iter()
        .map(|c| {
            new.fields
                .iter()
                .position(|n| n.name.eq_ignore_ascii_case(&c.name))
        })
        .collect();
    let mut reindexed =
        map.iter().enumerate().any(|(i, m)| *m != Some(i)) || old.fields.len() != new.fields.len();
    let mut fields: Vec<Field> = (0..new.fields.len())
        .map(|_| Field {
            compact: true,
            outline: true,
            subtotal_top: true,
            default_subtotal: true,
            show_all: false,
            ..Default::default()
        })
        .collect();
    for (i, m) in map.iter().enumerate() {
        let (Some(j), Some(f)) = (*m, def.fields.get(i)) else {
            continue;
        };
        let mut f = f.clone();
        let on_axis = f.axis.is_some();
        let new_items = &new.fields[j].items;
        let index: HashMap<String, usize> = new_items
            .iter()
            .enumerate()
            .map(|(x, v)| (v.key(), x))
            .collect();
        let mut kept = Vec::new();
        let mut subtotals = Vec::new();
        let mut used = vec![false; new_items.len()];
        for it in &f.items {
            match it.x {
                None => subtotals.push(it.clone()),
                Some(x) => {
                    let nx = old.fields[i]
                        .items
                        .get(x)
                        .and_then(|v| index.get(&v.key()).copied());
                    match nx {
                        Some(nx) if !used[nx] => {
                            if nx != x {
                                reindexed = true;
                            }
                            used[nx] = true;
                            kept.push(FieldItem {
                                x: Some(nx),
                                ..it.clone()
                            });
                        }
                        _ => reindexed = true,
                    }
                }
            }
        }
        if on_axis || !f.items.is_empty() {
            let mut fresh: Vec<usize> = (0..new_items.len()).filter(|x| !used[*x]).collect();
            fresh.sort_by(|a, b| {
                let (ka, kb) = (new_items[*a].sort_key(), new_items[*b].sort_key());
                ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)
            });
            if !fresh.is_empty() {
                reindexed = true;
            }
            kept.extend(fresh.into_iter().map(|x| FieldItem {
                x: Some(x),
                hidden: false,
                t: None,
                expanded: true,
                n: None,
            }));
            if let Some(dir @ ("ascending" | "descending")) = f.sort.as_deref() {
                {
                    kept.sort_by(|a, b| {
                        let k = |i: &FieldItem| new_items[i.x.unwrap_or(0)].sort_key();
                        let o = k(a).partial_cmp(&k(b)).unwrap_or(std::cmp::Ordering::Equal);
                        if dir == "descending" { o.reverse() } else { o }
                    });
                }
            }
            if on_axis && f.default_subtotal && subtotals.is_empty() {
                subtotals.push(FieldItem {
                    x: None,
                    hidden: false,
                    t: Some("default".into()),
                    expanded: true,
                    n: None,
                });
            }
            kept.extend(subtotals);
        }
        f.items = kept;
        fields[j] = f;
    }
    let remap = |x: i32| -> Option<i32> {
        if x == -2 {
            return Some(-2);
        }
        map.get(x as usize).copied().flatten().map(|j| j as i32)
    };
    out.row_fields = def.row_fields.iter().filter_map(|&x| remap(x)).collect();
    out.col_fields = def.col_fields.iter().filter_map(|&x| remap(x)).collect();
    out.page_fields = def
        .page_fields
        .iter()
        .filter_map(|p| {
            let j = map.get(p.fld).copied().flatten()?;
            let item = p.item.and_then(|k| {
                let x = def.fields.get(p.fld)?.items.get(k)?.x?;
                let key = old.fields[p.fld].items.get(x)?.key();
                fields[j]
                    .items
                    .iter()
                    .position(|it| it.x.is_some_and(|nx| new.fields[j].items[nx].key() == key))
            });
            Some(PageField {
                fld: j,
                item,
                xml: None,
            })
        })
        .collect();
    out.data_fields = def
        .data_fields
        .iter()
        .filter_map(|d| {
            let j = map.get(d.fld).copied().flatten()?;
            Some(DataField {
                fld: j,
                xml: None,
                ..d.clone()
            })
        })
        .collect();
    for f in fields.iter_mut() {
        f.data = false;
    }
    for d in &out.data_fields {
        fields[d.fld].data = true;
    }
    for (j, f) in fields.iter_mut().enumerate() {
        let x = j as i32;
        f.axis = if out.row_fields.contains(&x) {
            Some("axisRow".into())
        } else if out.col_fields.contains(&x) {
            Some("axisCol".into())
        } else if out.page_fields.iter().any(|p| p.fld == j) {
            Some("axisPage".into())
        } else {
            None
        };
    }
    out.fields = fields;
    // The Σ Values field: there with two data fields or more.
    let has_values = out.row_fields.contains(&-2) || out.col_fields.contains(&-2);
    if out.data_fields.len() >= 2 && !has_values {
        out.col_fields.push(-2);
    } else if out.data_fields.len() < 2 {
        out.row_fields.retain(|&x| x != -2);
        out.col_fields.retain(|&x| x != -2);
    }
    if reindexed {
        out.reindexed = true;
    }
    out
}

// ------------------------------------------------------------------ XML

/// The start tag of the root element of `xml`, with attribute `name` set to
/// `value` (added if missing).
fn root_start_with(xml: &str, sets: &[(&str, String)]) -> Option<String> {
    let doc = Document::parse(xml).ok()?;
    let root = doc.root_element();
    let start = root.range().start;
    let end = start + xml[start..].find('>')?;
    let mut tag = xml[start..end].trim_end_matches('/').to_owned();
    let mut edits = Vec::new();
    let mut added = String::new();
    for (name, value) in sets {
        let a = root
            .attributes()
            .find(|a| a.namespace().is_none() && a.name() == *name);
        match a {
            Some(a) => edits.push((a.range_value(), esc(value))),
            None => added.push_str(&format!(" {name}=\"{}\"", esc(value))),
        }
    }
    edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
    for (r, v) in edits {
        tag.replace_range(r.start - start..r.end - start, &v);
    }
    tag.push_str(&added);
    tag.push('>');
    Some(tag)
}

fn root_name(xml: &str) -> Option<String> {
    let doc = Document::parse(xml).ok()?;
    let start = doc.root_element().range().start;
    let tag = &xml[start + 1..];
    let end = tag.find(|c: char| c.is_whitespace() || c == '>' || c == '/')?;
    Some(tag[..end].to_owned())
}

/// `xml` with attribute `attr` of its first `element` set to `value`.
fn with_attr(xml: &str, element: &str, attr: &str, value: &str) -> Option<String> {
    let doc = Document::parse(xml).ok()?;
    let a = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == element)?
        .attributes()
        .find(|a| a.namespace().is_none() && a.name() == attr)?;
    let mut out = xml.to_owned();
    out.replace_range(a.range_value(), &esc(value));
    Some(out)
}

const XML_HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";

/// A `<pivotField>` element for `f`, keeping what the original said
/// besides its axis, data flag and items.
fn field_xml(f: &Field) -> String {
    let mut attrs = String::new();
    let mut rest = String::new();
    let original = f.xml.as_deref().and_then(|x| {
        let wrapped = format!("<w xmlns=\"{NS_MAIN}\" xmlns:r=\"{NS_R}\">{x}</w>");
        let doc = Document::parse(&wrapped).ok()?;
        let el = doc.root_element().first_element_child()?;
        let mut a = String::new();
        for at in el.attributes() {
            if at.namespace().is_some() {
                continue;
            }
            if matches!(at.name(), "axis" | "dataField") {
                continue;
            }
            a.push_str(&format!(" {}=\"{}\"", at.name(), esc(at.value())));
        }
        let mut r = String::new();
        for c in el.children().filter(|c| c.is_element()) {
            if c.tag_name().name() == "items" {
                continue;
            }
            r.push_str(&wrapped[c.range()]);
        }
        Some((a, r))
    });
    match original {
        Some((a, r)) => {
            attrs = a;
            rest = r;
        }
        None => attrs.push_str(" showAll=\"0\""),
    }
    let mut out = String::from("<pivotField");
    if let Some(axis) = &f.axis {
        out.push_str(&format!(" axis=\"{axis}\""));
    }
    if f.data {
        out.push_str(" dataField=\"1\"");
    }
    out.push_str(&attrs);
    if f.items.is_empty() && rest.is_empty() {
        out.push_str("/>");
        return out;
    }
    out.push('>');
    if !f.items.is_empty() {
        out.push_str(&format!("<items count=\"{}\">", f.items.len()));
        for it in &f.items {
            out.push_str("<item");
            if let Some(n) = &it.n {
                out.push_str(&format!(" n=\"{}\"", esc(n)));
            }
            if let Some(t) = &it.t {
                out.push_str(&format!(" t=\"{}\"", esc(t)));
            }
            if it.hidden {
                out.push_str(" h=\"1\"");
            }
            if !it.expanded {
                out.push_str(" sd=\"0\"");
            }
            if let Some(x) = it.x {
                out.push_str(&format!(" x=\"{x}\""));
            }
            out.push_str("/>");
        }
        out.push_str("</items>");
    }
    out.push_str(&rest);
    out.push_str("</pivotField>");
    out
}

/// The pivot table definition part for `laid`, placed at `location` (the
/// current coordinates).
pub(crate) fn def_xml(laid: &LaidXml, location: Area) -> Option<String> {
    let def = &laid.def;
    let xml = def.xml.as_str();
    let doc = Document::parse(xml).ok()?;
    let root = doc.root_element();
    let start = root_start_with(xml, &[("updatedVersion", "6".into())])?;
    let name = root_name(xml)?;
    let mut out = String::from(XML_HEAD);
    out.push_str(&start);
    let reference = format!(
        "{}:{}",
        cell_name(location.top, location.left),
        cell_name(location.bottom, location.right)
    );
    out.push_str(&format!(
        "<location ref=\"{reference}\" firstHeaderRow=\"{}\" firstDataRow=\"{}\" firstDataCol=\"{}\"",
        def.first_header_row, def.first_data_row, def.first_data_col
    ));
    if !def.page_fields.is_empty() {
        out.push_str(&format!(
            " rowPageCount=\"{}\" colPageCount=\"{}\"",
            def.row_page_count.max(1),
            def.col_page_count.max(1)
        ));
    }
    out.push_str("/>");
    out.push_str(&format!("<pivotFields count=\"{}\">", def.fields.len()));
    for f in &def.fields {
        out.push_str(&field_xml(f));
    }
    out.push_str("</pivotFields>");
    let fields = |name: &str, list: &[i32], out: &mut String| {
        if !list.is_empty() {
            out.push_str(&format!("<{name} count=\"{}\">", list.len()));
            for x in list {
                out.push_str(&format!("<field x=\"{x}\"/>"));
            }
            out.push_str(&format!("</{name}>"));
        }
    };
    fields("rowFields", &def.row_fields, &mut out);
    out.push_str(&format!(
        "<rowItems count=\"{}\">{}</rowItems>",
        def.row_items.len(),
        laid.row_items_xml
    ));
    fields("colFields", &def.col_fields, &mut out);
    out.push_str(&format!(
        "<colItems count=\"{}\">{}</colItems>",
        def.col_items.len(),
        laid.col_items_xml
    ));
    if !def.page_fields.is_empty() {
        out.push_str(&format!("<pageFields count=\"{}\">", def.page_fields.len()));
        for p in &def.page_fields {
            out.push_str(&format!("<pageField fld=\"{}\"", p.fld));
            if let Some(i) = p.item {
                out.push_str(&format!(" item=\"{i}\""));
            }
            out.push_str(" hier=\"-1\"/>");
        }
        out.push_str("</pageFields>");
    }
    if !def.data_fields.is_empty() {
        out.push_str(&format!("<dataFields count=\"{}\">", def.data_fields.len()));
        for d in &def.data_fields {
            out.push_str(&format!("<dataField fld=\"{}\"", d.fld));
            if let Some(n) = &d.name {
                out.push_str(&format!(" name=\"{}\"", esc(n)));
            }
            if d.func != Func::Sum {
                let f = match d.func {
                    Func::Sum => "sum",
                    Func::Count => "count",
                    Func::Average => "average",
                    Func::Max => "max",
                    Func::Min => "min",
                    Func::Product => "product",
                    Func::CountNums => "countNums",
                    Func::StdDev => "stdDev",
                    Func::StdDevp => "stdDevp",
                    Func::Var => "var",
                    Func::Varp => "varp",
                };
                out.push_str(&format!(" subtotal=\"{f}\""));
            }
            out.push_str(" baseField=\"0\" baseItem=\"0\"");
            if let Some(n) = d.num_fmt_id {
                out.push_str(&format!(" numFmtId=\"{n}\""));
            }
            out.push_str("/>");
        }
        out.push_str("</dataFields>");
    }
    const REWRITTEN: [&str; 8] = [
        "location",
        "pivotFields",
        "rowFields",
        "rowItems",
        "colFields",
        "colItems",
        "pageFields",
        "dataFields",
    ];
    const BY_INDEX: [&str; 3] = ["formats", "conditionalFormats", "chartFormats"];
    for c in root.children().filter(|c| c.is_element()) {
        let n = c.tag_name().name();
        if REWRITTEN.contains(&n) || (def.reindexed && BY_INDEX.contains(&n)) {
            continue;
        }
        out.push_str(&xml[c.range()]);
    }
    out.push_str(&format!("</{name}>"));
    Some(out)
}

/// What [`def_xml`] needs.
#[derive(Debug, Clone)]
pub(crate) struct LaidXml {
    pub def: Def,
    pub row_items_xml: String,
    pub col_items_xml: String,
}

/// The cache definition and records parts for `cache`, its source range at
/// `source` (the current coordinates) when it has one.
pub(crate) fn cache_xml(cache: &Cache, source: Option<Area>) -> Option<(String, String)> {
    let xml = cache.xml.as_str();
    let records = cache.records.as_ref()?;
    let doc = Document::parse(xml).ok()?;
    let root = doc.root_element();
    let start = root_start_with(
        xml,
        &[
            ("refreshedBy", "Kantor".into()),
            ("refreshedVersion", "6".into()),
            ("recordCount", records.len().to_string()),
        ],
    )?;
    let name = root_name(xml)?;
    let mut out = String::from(XML_HEAD);
    out.push_str(&start);
    for c in root.children().filter(|c| c.is_element()) {
        match c.tag_name().name() {
            "cacheSource" => match (source, child(c, "worksheetSource")) {
                (Some(a), Some(ws)) if ws.attribute("ref").is_some() => {
                    let r = ws.attributes().find(|x| x.name() == "ref")?;
                    let reference = format!(
                        "{}:{}",
                        cell_name(a.top, a.left),
                        cell_name(a.bottom, a.right)
                    );
                    let rv = r.range_value();
                    let mut text = xml[c.range()].to_owned();
                    let base = c.range().start;
                    text.replace_range(rv.start - base..rv.end - base, &reference);
                    out.push_str(&text);
                }
                _ => out.push_str(&xml[c.range()]),
            },
            "cacheFields" => {
                out.push_str(&format!("<cacheFields count=\"{}\">", cache.fields.len()));
                for (f, cf) in cache.fields.iter().enumerate() {
                    out.push_str(&format!(
                        "<cacheField name=\"{}\" numFmtId=\"{}\">",
                        esc(&cf.name),
                        cf.num_fmt_id
                    ));
                    shared_items_xml(cf, records.iter().map(|r| &r[f]), &mut out);
                    out.push_str("</cacheField>");
                }
                out.push_str("</cacheFields>");
            }
            _ => out.push_str(&xml[c.range()]),
        }
    }
    out.push_str(&format!("</{name}>"));

    let mut rec = String::from(XML_HEAD);
    rec.push_str(&format!(
        "<pivotCacheRecords xmlns=\"{NS_MAIN}\" xmlns:r=\"{NS_R}\" count=\"{}\">",
        records.len()
    ));
    let index: Vec<HashMap<String, usize>> = cache
        .fields
        .iter()
        .map(|cf| {
            cf.items
                .iter()
                .enumerate()
                .map(|(i, v)| (v.key(), i))
                .collect()
        })
        .collect();
    for r in records.iter() {
        rec.push_str("<r>");
        for (f, v) in r.iter().enumerate() {
            match index
                .get(f)
                .and_then(|m| (!m.is_empty()).then(|| m.get(&v.key())))
            {
                Some(Some(i)) => rec.push_str(&format!("<x v=\"{i}\"/>")),
                _ => v.xml(&mut rec),
            }
        }
        rec.push_str("</r>");
    }
    rec.push_str("</pivotCacheRecords>");
    Some((out, rec))
}

/// `<sharedItems>` of a field: its items, or the kinds of values it has.
fn shared_items_xml<'a>(cf: &CacheField, values: impl Iterator<Item = &'a Val>, out: &mut String) {
    let (mut s, mut n, mut b, mut d, mut e, mut m) = (false, false, false, false, false, false);
    let mut int = true;
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for v in values {
        match v {
            Val::Str(_) => s = true,
            Val::Num(x) => {
                n = true;
                int &= x.fract() == 0.0;
                lo = lo.min(*x);
                hi = hi.max(*x);
            }
            Val::Date(x) => {
                d = true;
                lo = lo.min(*x);
                hi = hi.max(*x);
            }
            Val::Bool(_) => b = true,
            Val::Err(_) => e = true,
            Val::Missing => m = true,
        }
    }
    let mut a = String::new();
    let kinds = [s, n, b, d, e].iter().filter(|x| **x).count();
    if !s && !b && !e {
        a.push_str(" containsSemiMixedTypes=\"0\"");
    }
    if d && !n {
        if !s && !b && !e {
            a.push_str(" containsNonDate=\"0\"");
        }
        a.push_str(" containsDate=\"1\"");
    }
    if !s {
        a.push_str(" containsString=\"0\"");
    }
    if m {
        a.push_str(" containsBlank=\"1\"");
    }
    if kinds > 1 {
        a.push_str(" containsMixedTypes=\"1\"");
    }
    if n {
        a.push_str(" containsNumber=\"1\"");
        if int {
            a.push_str(" containsInteger=\"1\"");
        }
    }
    if (n || d) && lo.is_finite() {
        if d && !n {
            a.push_str(&format!(
                " minDate=\"{}\" maxDate=\"{}\"",
                iso_of_serial(lo),
                iso_of_serial(hi)
            ));
        } else if !d {
            a.push_str(&format!(
                " minValue=\"{}\" maxValue=\"{}\"",
                format_number(lo),
                format_number(hi)
            ));
        }
    }
    if cf.items.is_empty() {
        out.push_str(&format!("<sharedItems{a}/>"));
        return;
    }
    out.push_str(&format!("<sharedItems{a} count=\"{}\">", cf.items.len()));
    for v in &cf.items {
        v.xml(out);
    }
    out.push_str("</sharedItems>");
}

/// A row or column of a pivot's body for GETPIVOTDATA: its `(field, item)`
/// pairs and its data field, if it names one.
type AxisKey = (Vec<(String, String)>, Option<usize>);

// ------------------------------------------------------------------ the workbook

/// A pivot table, as the grid needs it. Rows and columns are 0-based.
#[derive(Debug, Clone, PartialEq)]
pub struct PivotInfo {
    pub name: String,
    /// The body (Excel's location).
    pub area: CellRange,
    /// The body and the report filters above it.
    pub bounds: CellRange,
    /// Why Refresh can't recompute it, if it can't.
    pub refresh_problem: Option<String>,
}

fn range_of(a: Area) -> CellRange {
    CellRange {
        top: a.top,
        left: a.left,
        bottom: a.bottom,
        right: a.right,
    }
}

impl Workbook {
    fn pivot_state(&self) -> Arc<State> {
        self.history.pivot_state()
    }

    /// The definition of pivot `i` now.
    fn pivot_def(&self, state: &State, i: usize) -> Arc<Def> {
        match state.defs.get(&i) {
            Some(l) => Arc::new(l.def.clone()),
            None => self.pivots.defs[i].clone(),
        }
    }

    fn pivot_cache(&self, state: &State, i: usize) -> Arc<Cache> {
        state
            .caches
            .get(&i)
            .cloned()
            .unwrap_or_else(|| self.pivots.caches[i].clone())
    }

    /// Where `area` (in the coordinates of `at`) is now on sheet `sheet_id`.
    fn now(&self, at: &Layout, sheet_id: u32, area: Area) -> Option<Area> {
        at.moves_to(&self.history.layout()).area(sheet_id, area)
    }

    /// The pivot tables of `sheet` as they are now.
    pub(crate) fn placed_pivots(&self, sheet: u32) -> Arc<Vec<Placed>> {
        if self.pivots.defs.is_empty() {
            return Arc::default();
        }
        if let Some(p) = self.cached(|c| c.pivots.get(&sheet).cloned()) {
            return p;
        }
        let state = self.pivot_state();
        let sheet_id = self.sheet_id(sheet);
        let theme = &self.model.get_model().workbook.theme;
        let mut out = Vec::new();
        for i in 0..self.pivots.defs.len() {
            let def = self.pivot_def(&state, i);
            if def.sheet_id != sheet_id {
                continue;
            }
            let Some(area) = self.now(&def.at, sheet_id, def.location) else {
                continue;
            };
            let style = def
                .style
                .name
                .as_deref()
                .and_then(|n| builtin(n, theme))
                .map(Arc::new);
            out.push(Placed::new(i, def, area, style));
        }
        let rc = Arc::new(out);
        self.cached(|c| c.pivots.insert(sheet, rc.clone()));
        rc
    }

    fn pivot_info(&self, p: &Placed) -> PivotInfo {
        let state = self.pivot_state();
        let cache = self.pivot_cache(&state, p.def.cache);
        PivotInfo {
            name: p.def.name.clone(),
            area: range_of(p.area),
            bounds: range_of(p.bounds()),
            refresh_problem: cache
                .unsupported
                .clone()
                .or_else(|| p.def.unsupported.clone()),
        }
    }

    /// The pivot tables of `sheet`.
    pub fn pivot_tables(&self, sheet: u32) -> Vec<PivotInfo> {
        self.placed_pivots(sheet)
            .iter()
            .map(|p| self.pivot_info(p))
            .collect()
    }

    /// The pivot table `(row, col)` of `sheet` is in (its body or its report
    /// filters).
    pub fn pivot_at(&self, sheet: u32, row: u32, col: u32) -> Option<PivotInfo> {
        self.placed_pivots(sheet)
            .iter()
            .find(|p| p.contains(row, col))
            .map(|p| self.pivot_info(p))
    }

    /// The look the pivot table style gives cell `(row, col)`.
    pub(crate) fn pivot_cell_look(&self, sheet: u32, row: u32, col: u32) -> Option<TableCellLook> {
        self.placed_pivots(sheet)
            .iter()
            .find(|p| p.contains(row, col))?
            .look(row, col)
    }

    /// Each pivot table's value cells, for GETPIVOTDATA: the data field and
    /// the field/item pairs of every cell of the body.
    fn pivot_grids(&self) -> Vec<ironcalc_base::kantor_pivot::PivotGrid> {
        use ironcalc_base::kantor_pivot::{PivotGrid, PivotValueCell};
        let state = self.pivot_state();
        let sheets = self.model.get_model().workbook.worksheets.len() as u32;
        let mut out = Vec::new();
        for sheet in 0..sheets {
            for p in self.placed_pivots(sheet).iter() {
                let d = &p.def;
                let cache = self.pivot_cache(&state, d.cache);
                let nf = cache.fields.len();
                if d.fields.len() != nf || d.data_fields.is_empty() {
                    continue;
                }
                // (pairs, data field) of each row or column of the body.
                let axis = |fields: &[i32], items: &[AxisItem]| {
                    let lv = levels(fields, nf);
                    let mut prev: Vec<usize> = Vec::new();
                    let mut out: Vec<AxisKey> = Vec::new();
                    for it in items {
                        let path: Vec<usize> = if it.t == "grand" {
                            Vec::new()
                        } else {
                            prev.iter()
                                .take(it.r)
                                .copied()
                                .chain(it.x.iter().copied())
                                .collect()
                        };
                        if it.t != "grand" {
                            prev = path.clone();
                        }
                        let mut pairs = Vec::new();
                        let mut data = (it.i > 0).then_some(it.i);
                        for (l, k) in path.iter().enumerate() {
                            match lv.get(l) {
                                Some(Level::Field(f)) if *k < d.fields[*f].items.len() => pairs
                                    .push((
                                        cache.fields[*f].name.to_lowercase(),
                                        d.item_label(&cache, *f, *k).label().to_lowercase(),
                                    )),
                                Some(Level::Values) => data = Some(*k),
                                _ => {}
                            }
                        }
                        out.push((pairs, data));
                    }
                    out
                };
                let rows = axis(&d.row_fields, &d.row_items);
                let cols = axis(&d.col_fields, &d.col_items);
                let names: Vec<Vec<String>> = (0..d.data_fields.len())
                    .map(|i| {
                        let mut n = vec![d.data_caption_of(&cache, i).to_lowercase()];
                        if let Some(c) = cache.fields.get(d.data_fields[i].fld) {
                            n.push(c.name.to_lowercase());
                        }
                        n
                    })
                    .collect();
                let top = p.area.top + d.first_data_row;
                let left = p.area.left + d.first_data_col;
                let mut cells = Vec::new();
                for (i, (rp, rd)) in rows.iter().enumerate() {
                    for (j, (cp, cd)) in cols.iter().enumerate() {
                        let k = rd.or(*cd).unwrap_or(0).min(names.len() - 1);
                        let mut items: Vec<(String, String)> =
                            rp.iter().chain(cp.iter()).cloned().collect();
                        items.sort();
                        cells.push(PivotValueCell {
                            data: names[k].clone(),
                            items,
                            row: (top + i as u32) as i32 + 1,
                            column: (left + j as u32) as i32 + 1,
                        });
                    }
                }
                let b = p.bounds();
                out.push(PivotGrid {
                    sheet,
                    top: b.top as i32 + 1,
                    left: b.left as i32 + 1,
                    bottom: b.bottom as i32 + 1,
                    right: b.right as i32 + 1,
                    cells,
                });
            }
        }
        out
    }

    /// Give IronCalc the pivot tables as they are now (for GETPIVOTDATA),
    /// recalculating if they changed and `recalc` (automatic calculation).
    pub(crate) fn sync_pivot_grids(&mut self, recalc: bool) {
        if self.pivots.defs.is_empty() {
            return;
        }
        let grids = self.pivot_grids();
        if grids == self.model.get_model().kantor_pivots {
            return;
        }
        self.model.kantor_set_pivots(grids);
        if recalc && self.calc.mode == crate::CalcMode::Automatic {
            self.model.evaluate();
        }
    }

    /// Refuse inserting (`count` at `at`) or deleting (`at..at + count`) rows
    /// or columns through a pivot table, as Excel does: its cells would no
    /// longer match its definition.
    pub(crate) fn check_pivots(
        &self,
        sheet: u32,
        rows: bool,
        insert: bool,
        at: u32,
        count: u32,
    ) -> Result<()> {
        for p in self.placed_pivots(sheet).iter() {
            let b = p.bounds();
            let (lo, hi) = if rows {
                (b.top, b.bottom)
            } else {
                (b.left, b.right)
            };
            let cuts = if insert {
                lo < at && at <= hi
            } else {
                at <= hi && at + count > lo
            };
            if cuts {
                bail!(
                    "this would change part of the PivotTable {} ({}); move or delete it first",
                    p.def.name,
                    range_of(b).name()
                );
            }
        }
        Ok(())
    }

    /// Refresh the pivot table at `(row, col)` of `sheet`, and the others
    /// sharing its cache, from their source (one undo step). Returns how
    /// many pivot tables were refreshed.
    pub fn refresh_pivot(&mut self, sheet: u32, row: u32, col: u32) -> Result<usize> {
        let p = self
            .placed_pivots(sheet)
            .iter()
            .find(|p| p.contains(row, col))
            .cloned()
            .ok_or_else(|| anyhow!("there is no PivotTable at {}", cell_name(row, col)))?;
        self.refresh_caches(&[p.def.cache])
    }

    /// Refresh every pivot table of the workbook (one undo step).
    pub fn refresh_all_pivots(&mut self) -> Result<usize> {
        let all: Vec<usize> = (0..self.pivots.caches.len()).collect();
        self.refresh_caches(&all)
    }

    /// The records of a cache's source now: `(sheet, area, data)`, the area
    /// including the header row.
    fn read_source(&self, cache: &Cache) -> Result<(u32, Area, SourceData)> {
        let model = self.model.get_model();
        let sheet_index = |name: &str| -> Result<u32> {
            model
                .workbook
                .worksheets
                .iter()
                .position(|w| w.name.eq_ignore_ascii_case(name))
                .map(|i| i as u32)
                .ok_or_else(|| anyhow!("its source sheet {name:?} is gone"))
        };
        let (sheet, mut area) = match cache.source.as_ref() {
            Some(Source::Range { sheet, area }) => {
                let s = sheet_index(sheet)?;
                let a = self
                    .now(&cache.at, self.sheet_id(s), *area)
                    .ok_or_else(|| anyhow!("its source cells were deleted"))?;
                (s, a)
            }
            Some(Source::Name(name)) => {
                if let Some(t) =
                    ironcalc_base::kantor_tables::find_table_pub(&model.workbook.tables, name)
                {
                    let (top, left, bottom, right) = ironcalc_base::kantor_tables::table_area(t)
                        .ok_or_else(|| anyhow!("the table {name} has no range"))?;
                    let bottom = (bottom - t.totals_row_count as i32).max(top);
                    (
                        sheet_index(&t.sheet_name)?,
                        Area {
                            top: top as u32 - 1,
                            left: left as u32 - 1,
                            bottom: bottom as u32 - 1,
                            right: right as u32 - 1,
                        },
                    )
                } else {
                    let dn = model
                        .workbook
                        .defined_names
                        .iter()
                        .filter(|d| d.name.eq_ignore_ascii_case(name))
                        .min_by_key(|d| d.sheet_id.is_some())
                        .ok_or_else(|| anyhow!("its source {name:?} is not in the workbook"))?;
                    let f = dn.formula.trim_start_matches('=');
                    let (s, r) = f
                        .rsplit_once('!')
                        .ok_or_else(|| anyhow!("its source {name:?} is not a range"))?;
                    let s = s.trim_matches('\'').replace("''", "'");
                    let a = crate::shift::parse_range(r)
                        .ok_or_else(|| anyhow!("its source {name:?} is not a range"))?
                        .area();
                    (sheet_index(&s)?, a)
                }
            }
            None => bail!(
                "{}",
                cache
                    .unsupported
                    .clone()
                    .unwrap_or_else(|| "its source is not a worksheet range".into())
            ),
        };
        // Whole columns: as far as the sheet has cells.
        let (used_rows, used_cols) = self.used_size(sheet);
        area.bottom = area.bottom.min(used_rows.saturating_sub(1).max(area.top));
        area.right = area.right.min(used_cols.saturating_sub(1).max(area.left));
        if area.bottom <= area.top {
            bail!(
                "its source {} has no rows under its header",
                range_of(area).name()
            );
        }
        let mut names: Vec<String> = Vec::new();
        for c in area.left..=area.right {
            let n = model
                .get_formatted_cell_value(sheet, area.top as i32 + 1, c as i32 + 1)
                .unwrap_or_default();
            let n = n.trim().to_owned();
            if n.is_empty() {
                bail!(
                    "the PivotTable field name is not valid: {} in the source's header row is empty",
                    cell_name(area.top, c)
                );
            }
            let mut unique = n.clone();
            let mut k = 2;
            while names.iter().any(|x| x.eq_ignore_ascii_case(&unique)) {
                unique = format!("{n}{k}");
                k += 1;
            }
            names.push(unique);
        }
        let mut rows = Vec::with_capacity((area.bottom - area.top) as usize);
        for r in area.top + 1..=area.bottom {
            let mut rec = Vec::with_capacity(names.len());
            for c in area.left..=area.right {
                let v = model
                    .get_cell_value_by_index(sheet, r as i32 + 1, c as i32 + 1)
                    .unwrap_or(ironcalc_base::cell::CellValue::None);
                rec.push(match v {
                    ironcalc_base::cell::CellValue::None => Val::Missing,
                    ironcalc_base::cell::CellValue::String(s) if s.is_empty() => Val::Missing,
                    ironcalc_base::cell::CellValue::String(s) => Val::Str(s),
                    ironcalc_base::cell::CellValue::Boolean(b) => Val::Bool(b),
                    ironcalc_base::cell::CellValue::Number(n) => {
                        if crate::grid::is_date_code(&self.cell_style(sheet, r, c).num_fmt) {
                            Val::Date(n)
                        } else {
                            Val::Num(n)
                        }
                    }
                });
            }
            rows.push(rec);
        }
        Ok((sheet, area, SourceData { names, rows }))
    }

    fn refresh_caches(&mut self, caches: &[usize]) -> Result<usize> {
        let state = self.pivot_state();
        let layout = self.history.layout();
        // Work everything out first: nothing changes if any of it fails.
        struct Plan {
            index: usize,
            sheet: u32,
            old: Area,
            laid: Laid,
        }
        let mut plans: Vec<Plan> = Vec::new();
        let mut new_caches: Vec<(usize, Arc<Cache>)> = Vec::new();
        for &ci in caches {
            let users: Vec<usize> = (0..self.pivots.defs.len())
                .filter(|&i| self.pivot_def(&state, i).cache == ci)
                .collect();
            if users.is_empty() {
                continue;
            }
            let old = self.pivot_cache(&state, ci);
            let first = self.pivot_def(&state, users[0]);
            if let Some(why) = &old.unsupported {
                bail!("can't refresh the PivotTable {}: {why}", first.name);
            }
            for &i in &users {
                let d = self.pivot_def(&state, i);
                if let Some(why) = &d.unsupported {
                    bail!("can't refresh the PivotTable {}: {why}", d.name);
                }
            }
            let (src_sheet, src_area, data) = self
                .read_source(&old)
                .map_err(|e| anyhow!("can't refresh the PivotTable {}: {e}", first.name))?;
            let axis: Vec<String> = users
                .iter()
                .flat_map(|&i| {
                    let d = self.pivot_def(&state, i);
                    d.fields
                        .iter()
                        .enumerate()
                        .filter(|(_, f)| f.axis.is_some())
                        .filter_map(|(k, _)| old.fields.get(k).map(|c| c.name.clone()))
                        .collect::<Vec<_>>()
                })
                .collect();
            let mut cache = rebuild_cache(&old, data, &axis);
            cache.at = layout.clone();
            if let Some(Source::Range { sheet, .. }) = &cache.source {
                cache.source = Some(Source::Range {
                    sheet: sheet.clone(),
                    area: src_area,
                });
            }
            let _ = src_sheet;
            let records = cache.records.clone().unwrap_or_default();
            for &i in &users {
                let d = self.pivot_def(&state, i);
                let sheet = self
                    .model
                    .get_model()
                    .workbook
                    .worksheets
                    .iter()
                    .position(|w| w.sheet_id == d.sheet_id)
                    .ok_or_else(|| anyhow!("the PivotTable {}'s sheet is gone", d.name))?
                    as u32;
                let placed = self.placed_pivots(sheet);
                let p = placed
                    .iter()
                    .find(|p| p.index == i)
                    .ok_or_else(|| anyhow!("the PivotTable {}'s cells were deleted", d.name))?;
                let matched = rematch(&d, &old, &cache);
                let mut laid = lay_out(&matched, &cache, &records, p.area.top, p.area.left);
                laid.def.at = layout.clone();
                plans.push(Plan {
                    index: i,
                    sheet,
                    old: p.bounds(),
                    laid,
                });
            }
            new_caches.push((ci, Arc::new(cache)));
        }
        if plans.is_empty() {
            return Ok(0);
        }
        if !self.pivots.refreshed {
            let bytes = self
                .origin
                .xlsx
                .as_ref()
                .ok_or_else(|| anyhow!("can't refresh: the opened file is not at hand"))?
                .bytes()
                .map_err(|e| anyhow!("can't refresh: {e}"))?;
            let mut kept = HashMap::new();
            for c in &self.pivots.caches {
                if let Some(rp) = &c.records_part {
                    let data = crate::preserve::read_part(&bytes, rp)
                        .map_err(|e| anyhow!("can't refresh: {e}"))?;
                    kept.insert(rp.clone(), Arc::new(data));
                }
            }
            drop(bytes);
            self.pivots.records = kept;
            self.pivots.refreshed = true;
        }
        // Room for the new cells: nothing else in the way.
        for plan in &plans {
            let new = plan.laid.def.location;
            let bounds = page_area(&plan.laid.def, new).map_or(new, |p| Area {
                top: p.top,
                left: new.left,
                bottom: new.bottom,
                right: new.right.max(p.right),
            });
            if bounds.bottom >= crate::MAX_ROWS || bounds.right >= crate::MAX_COLS {
                bail!(
                    "the PivotTable {} would go past the sheet's edge",
                    plan.laid.def.name
                );
            }
            for other in self.placed_pivots(plan.sheet).iter() {
                if other.index == plan.index {
                    continue;
                }
                let o = other.bounds();
                if o.top <= bounds.bottom
                    && bounds.top <= o.bottom
                    && o.left <= bounds.right
                    && bounds.left <= o.right
                {
                    bail!(
                        "the PivotTable {} would overlap the PivotTable {}",
                        plan.laid.def.name,
                        other.def.name
                    );
                }
            }
            let old = plan.old;
            for &(r, c) in &self.filled_cells(plan.sheet) {
                let inside = |a: &Area| r >= a.top && r <= a.bottom && c >= a.left && c <= a.right;
                if inside(&bounds) && !inside(&old) {
                    bail!(
                        "the PivotTable {} needs {} and there's already data in {}: move it first",
                        plan.laid.def.name,
                        range_of(bounds).name(),
                        cell_name(r, c)
                    );
                }
            }
        }
        // Write.
        self.model.pause_evaluation();
        let mut steps = 0u32;
        let mut result = Ok(());
        let num_fmts = self.model.get_model().workbook.styles.num_fmts.clone();
        'plans: for plan in &plans {
            let old = range_of(plan.old);
            if let Err(e) = self.model.range_clear_all(&old.area(plan.sheet)) {
                result = Err(anyhow!("{e}"));
                break;
            }
            steps += 1;
            for (r, c, v) in &plan.laid.cells {
                if let Err(e) =
                    self.model
                        .set_user_input(plan.sheet, *r as i32 + 1, *c as i32 + 1, v)
                {
                    result = Err(anyhow!("{e}"));
                    break 'plans;
                }
                steps += 1;
            }
            for (a, id) in &plan.laid.num_fmts {
                let code = ironcalc_base::number_format::get_num_fmt(*id as i32, &num_fmts);
                if code.is_empty() || code == "general" {
                    continue;
                }
                if self
                    .model
                    .update_range_style(&range_of(*a).area(plan.sheet), "num_fmt", &code)
                    .is_ok()
                {
                    steps += 1;
                }
            }
        }
        if self.calc.mode == crate::CalcMode::Automatic {
            self.model.resume_evaluation();
            self.model.evaluate();
        }
        if steps == 0 {
            return result.map(|()| 0);
        }
        self.history.edited_steps(steps);
        if result.is_ok() {
            let mut next = (*state).clone();
            for (ci, c) in new_caches {
                next.caches.insert(ci, c);
            }
            for plan in &plans {
                next.defs.insert(
                    plan.index,
                    Arc::new(LaidXml {
                        def: plan.laid.def.clone(),
                        row_items_xml: plan.laid.row_items_xml.clone(),
                        col_items_xml: plan.laid.col_items_xml.clone(),
                    }),
                );
            }
            self.history.set_pivot_state(Arc::new(next));
        }
        self.sync_pivot_grids(true);
        result.map(|()| plans.len())
    }

    /// The pivot parts refreshes rewrote, placed where they are now, for
    /// the next save (see `preserve::Context::pivot_parts`).
    pub(crate) fn pivot_parts(&self) -> HashMap<String, Vec<u8>> {
        let mut out = HashMap::new();
        if !self.pivots.refreshed {
            return out;
        }
        let state = self.pivot_state();
        for (i, d) in self.pivots.defs.iter().enumerate() {
            let xml = match state.defs.get(&i) {
                Some(laid) => {
                    let d = &laid.def;
                    self.now(&d.at, d.sheet_id, d.location)
                        .and_then(|loc| def_xml(laid, loc))
                }
                None => self
                    .now(&d.at, d.sheet_id, d.location)
                    .and_then(|loc| with_attr(&d.xml, "location", "ref", &range_of(loc).name())),
            };
            if let Some(xml) = xml {
                out.insert(d.part.clone(), xml.into_bytes());
            }
        }
        for (i, orig) in self.pivots.caches.iter().enumerate() {
            let cache = state.caches.get(&i).unwrap_or(orig);
            let source = match &cache.source {
                Some(Source::Range { sheet, area }) => {
                    let model = self.model.get_model();
                    model
                        .workbook
                        .worksheets
                        .iter()
                        .find(|w| w.name.eq_ignore_ascii_case(sheet))
                        .and_then(|w| self.now(&cache.at, w.sheet_id, *area))
                }
                _ => None,
            };
            if state.caches.contains_key(&i) {
                if let Some((def, rec)) = cache_xml(cache, source) {
                    out.insert(cache.part.clone(), def.into_bytes());
                    if let Some(rp) = &cache.records_part {
                        out.insert(rp.clone(), rec.into_bytes());
                    }
                }
                continue;
            }
            let def = match source {
                Some(a) => with_attr(&cache.xml, "worksheetSource", "ref", &range_of(a).name()),
                None => Some(cache.xml.to_string()),
            };
            if let Some(def) = def {
                out.insert(cache.part.clone(), def.into_bytes());
            }
            if let Some(rp) = &cache.records_part
                && let Some(rec) = self.pivots.records.get(rp)
            {
                out.insert(rp.clone(), rec.to_vec());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> (Cache, Vec<Vec<Val>>) {
        let xml = r#"<pivotCacheDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" recordCount="3"><cacheSource type="worksheet"><worksheetSource ref="A1:C4" sheet="D"/></cacheSource><cacheFields count="3"><cacheField name="Region" numFmtId="0"><sharedItems count="2"><s v="East"/><s v="West"/></sharedItems></cacheField><cacheField name="Product" numFmtId="0"><sharedItems count="2"><s v="Apples"/><s v="Pears"/></sharedItems></cacheField><cacheField name="Units" numFmtId="0"><sharedItems containsSemiMixedTypes="0" containsString="0" containsNumber="1"/></cacheField></cacheFields></pivotCacheDefinition>"#;
        let c = parse_cache("c.xml", None, xml).unwrap();
        let s = |x: &str| Val::Str(x.into());
        let recs = vec![
            vec![s("East"), s("Apples"), Val::Num(1.0)],
            vec![s("East"), s("Pears"), Val::Num(2.0)],
            vec![s("West"), s("Apples"), Val::Num(4.0)],
        ];
        (c, recs)
    }

    /// A definition with rows Region, Product and Sum of Units; `field`
    /// holds extra attributes for both row fields, `item` for East's item.
    fn def(field: &str, item: &str) -> Def {
        let xml = format!(
            r#"<pivotTableDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" name="P" cacheId="1" dataCaption="Values"><location ref="A1:B7" firstHeaderRow="1" firstDataRow="1" firstDataCol="1"/><pivotFields count="3"><pivotField axis="axisRow" showAll="0"{field}><items count="3"><item x="0"{item}/><item x="1"/><item t="default"/></items></pivotField><pivotField axis="axisRow" showAll="0"{field}><items count="3"><item x="0"/><item x="1"/><item t="default"/></items></pivotField><pivotField dataField="1" showAll="0"/></pivotFields><rowFields count="2"><field x="0"/><field x="1"/></rowFields><colItems count="1"><i/></colItems><dataFields count="1"><dataField name="Sum of Units" fld="2"/></dataFields></pivotTableDefinition>"#
        );
        parse_def("p.xml", 1, 0, &xml).unwrap()
    }

    fn table(l: &Laid) -> Vec<Vec<String>> {
        let a = l.def.location;
        let mut t = vec![vec![String::new(); (a.right + 1) as usize]; (a.bottom + 1) as usize];
        for (r, c, v) in &l.cells {
            t[*r as usize][*c as usize] = v.clone();
        }
        t
    }

    #[test]
    fn compact_form_puts_outer_items_on_their_own_rows() {
        let (c, recs) = cache();
        let l = lay_out(&def("", ""), &c, &recs, 0, 0);
        assert_eq!(
            table(&l),
            [
                ["Row Labels", "Sum of Units"],
                ["East", "3"],
                ["Apples", "1"],
                ["Pears", "2"],
                ["West", "4"],
                ["Apples", "4"],
                ["Grand Total", "7"],
            ]
        );
        assert_eq!(
            l.row_items_xml,
            r#"<i><x/></i><i r="1"><x/></i><i r="1"><x v="1"/></i><i><x v="1"/></i><i r="1"><x/></i><i t="grand"><x/></i>"#
        );
        assert_eq!(
            (
                l.def.first_header_row,
                l.def.first_data_row,
                l.def.first_data_col
            ),
            (1, 1, 1)
        );
        let lines: Vec<Line> = l.def.row_items.iter().map(|i| line_of(i, 2)).collect();
        assert_eq!(
            lines,
            [
                Line::Subheading(0),
                Line::Body,
                Line::Body,
                Line::Subheading(0),
                Line::Body,
                Line::Grand
            ]
        );
    }

    #[test]
    fn outline_form_gives_each_field_a_column() {
        let (c, recs) = cache();
        let l = lay_out(&def(r#" compact="0""#, ""), &c, &recs, 0, 0);
        assert_eq!(
            table(&l),
            [
                ["Region", "Product", "Sum of Units"],
                ["East", "", "3"],
                ["", "Apples", "1"],
                ["", "Pears", "2"],
                ["West", "", "4"],
                ["", "Apples", "4"],
                ["Grand Total", "", "7"],
            ]
        );
    }

    #[test]
    fn subtotals_at_the_bottom_and_collapsed_items() {
        let (c, recs) = cache();
        let l = lay_out(&def(r#" subtotalTop="0""#, ""), &c, &recs, 0, 0);
        assert_eq!(
            table(&l),
            [
                ["Row Labels", "Sum of Units"],
                ["East", ""],
                ["Apples", "1"],
                ["Pears", "2"],
                ["East Total", "3"],
                ["West", ""],
                ["Apples", "4"],
                ["West Total", "4"],
                ["Grand Total", "7"],
            ]
        );
        // East collapsed: one row with its total.
        let l = lay_out(&def("", r#" sd="0""#), &c, &recs, 0, 0);
        assert_eq!(
            table(&l),
            [
                ["Row Labels", "Sum of Units"],
                ["East", "3"],
                ["West", "4"],
                ["Apples", "4"],
                ["Grand Total", "7"],
            ]
        );
    }

    #[test]
    fn rematch_follows_fields_by_name_and_appends_new_items() {
        let (old, _) = cache();
        let d = def("", r#" h="1""#); // East hidden
        let s = |x: &str| Val::Str(x.into());
        // Columns swapped, a new region, Pears gone.
        let data = SourceData {
            names: vec!["Units".into(), "Product".into(), "Region".into()],
            rows: vec![
                vec![Val::Num(5.0), s("Apples"), s("North")],
                vec![Val::Num(6.0), s("Apples"), s("East")],
            ],
        };
        let new = rebuild_cache(&old, data, &["Region".into(), "Product".into()]);
        assert_eq!(
            new.fields[2].items,
            [s("East"), s("North")],
            "old order first"
        );
        let m = rematch(&d, &old, &new);
        assert_eq!(m.row_fields, [2, 1]);
        assert_eq!(m.data_fields[0].fld, 0);
        assert!(m.fields[0].data && m.fields[2].axis.as_deref() == Some("axisRow"));
        let region: Vec<(Option<usize>, bool)> =
            m.fields[2].items.iter().map(|i| (i.x, i.hidden)).collect();
        // East (still hidden), North (new), then the subtotal item.
        assert_eq!(region, [(Some(0), true), (Some(1), false), (None, false)]);
        assert!(m.reindexed);
        let recs = new.records.clone().unwrap();
        let l = lay_out(&m, &new, &recs, 0, 0);
        assert_eq!(
            table(&l),
            [
                ["Row Labels", "Sum of Units"],
                ["North", "5"],
                ["Apples", "5"],
                ["Grand Total", "5"],
            ]
        );
    }

    #[test]
    fn summary_functions() {
        let v: Vec<Val> = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]
            .into_iter()
            .map(Val::Num)
            .chain([Val::Str("x".into()), Val::Missing])
            .collect();
        let r: Vec<&Val> = v.iter().collect();
        let f = |func: Func| func.apply(&r).unwrap().unwrap();
        assert_eq!(f(Func::Sum), 40.0);
        assert_eq!(f(Func::Count), 9.0, "text counts, blanks don't");
        assert_eq!(f(Func::CountNums), 8.0);
        assert_eq!(f(Func::Average), 5.0);
        assert_eq!(f(Func::Max), 9.0);
        assert_eq!(f(Func::Min), 2.0);
        assert_eq!(
            f(Func::Product),
            2.0 * 4.0 * 4.0 * 4.0 * 5.0 * 5.0 * 7.0 * 9.0
        );
        assert_eq!(f(Func::Varp), 4.0);
        assert_eq!(f(Func::StdDevp), 2.0);
        assert!((f(Func::Var) - 32.0 / 7.0).abs() < 1e-12);
        assert!((f(Func::StdDev) - (32.0f64 / 7.0).sqrt()).abs() < 1e-12);
        let one = [Val::Num(3.0)];
        let one: Vec<&Val> = one.iter().collect();
        assert_eq!(Func::StdDev.apply(&one), Some(Err("#DIV/0!")));
        assert_eq!(Func::Sum.apply(&[]), None);
    }

    #[test]
    fn dates_convert_both_ways() {
        assert_eq!(serial_of_iso("2026-01-05T00:00:00"), Some(46027.0));
        assert_eq!(iso_of_serial(46027.0), "2026-01-05T00:00:00");
        assert_eq!(iso_of_serial(46027.5), "2026-01-05T12:00:00");
        assert_eq!(serial_of_iso("1900-03-01"), Some(61.0));
    }

    #[test]
    fn text_that_looks_like_a_number_stays_text() {
        assert_eq!(text_input("East"), "East");
        assert_eq!(text_input("2026"), "'2026");
        assert_eq!(text_input("=x"), "'=x");
        assert_eq!(text_input("true"), "'true");
    }
}
