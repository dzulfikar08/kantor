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
//!
//! **Captions and language.** Most labels a refresh writes have no place in
//! the file: Excel computes them in its interface language. Which ones the
//! file can hold (ECMA-376 Part 1 §18.10.1.73 `pivotTableDefinition`,
//! §18.10.1.69 `pivotField` and the field items' `n`; Excel's reading per
//! the MS-OI29500 notes on those two sections):
//!
//! | Label | Held by |
//! |---|---|
//! | `Row Labels` (compact header) | `rowHeaderCaption` |
//! | `Column Labels` (compact header) | `colHeaderCaption` |
//! | `Values` (the `Σ Values` field's header) | `dataCaption` |
//! | `Grand Total` | `grandTotalCaption` |
//! | `Total Sum of X` (grand total per data field, several data fields) | none |
//! | `East Total` (a field's subtotals) | the field's `subtotalCaption`: one text for all of them; MS-OI29500 says Office puts the field name for `?` in it, here it's the item's name, as Excel's UI does when a subtotal label is typed over |
//! | `(blank)` (an item for empty source cells) | none, unless the item has a caption (`n`) |
//! | `(All)`, `(Multiple Items)` (a report filter's value) | none |
//! | empty value cells | `missingCaption` when `showMissing` (default on; no caption = empty) |
//! | error value cells | `errorCaption` when `showError` (default off = the error) |
//!
//! A pivot table made here writes its captions in the interface language
//! where they differ from Excel's English: the root captions (see
//! `pivot_edit::Captions`) and an `n` on its `(blank)` items. It writes no
//! `subtotalCaption` (what Excel puts for `?` isn't settled) and nothing for
//! the labels with no attribute, which follow the interface language only
//! while the session that made it lasts. Pivot tables from a file show the
//! captions it holds and Excel's English for the rest, and get no new
//! attributes.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use roxmltree::{Document, Node};

use crate::content_text::{tr, trf};
use crate::format::LineStyle;
use crate::pivot_calc::{
    BASE_NEXT, BASE_PREVIOUS, FilterFamily, Formula, NS_X14, PivotFilter, ShowAs,
    URI_X14_DATA_FIELD, URI_X14_PIVOT_FIELD, filter_family,
};
use crate::pivot_group::{FieldGroup, GroupKind};
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
    pub(crate) fn key(&self) -> String {
        match self {
            Val::Str(s) => format!("s{}", s.to_lowercase()),
            Val::Num(n) => format!("n{n}"),
            Val::Bool(b) => format!("b{b}"),
            Val::Date(d) => format!("d{d}"),
            Val::Err(e) => format!("e{e}"),
            Val::Missing => "m".into(),
        }
    }

    pub(crate) fn number(&self) -> Option<f64> {
        match self {
            Val::Num(n) | Val::Date(n) => Some(*n),
            _ => None,
        }
    }

    /// What a label cell shows (`(blank)` for a missing value).
    pub(crate) fn label(&self) -> String {
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
    pub(crate) fn sort_key(&self) -> (u8, f64, String) {
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

pub(crate) fn format_number(n: f64) -> String {
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
pub(crate) fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub(crate) fn civil_from_days(z: i64) -> (i64, i64, i64) {
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

pub(crate) fn serial_of_iso(s: &str) -> Option<f64> {
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

pub(crate) fn iso_of_serial(serial: f64) -> String {
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
    /// Shared items (what records' `x` index), if the cache lists them.
    pub items: Vec<Val>,
    pub num_fmt_id: u32,
    /// A column of the source (else a calculated or grouping field, which
    /// records don't hold; those come after the source's).
    pub database: bool,
    /// A calculated field's formula (without `=`).
    pub formula: Option<String>,
    pub group: Option<FieldGroup>,
    /// Calculated items (Insert Calculated Item), in solve order. Each one's
    /// name is also a shared item (records never point at it), after the
    /// source's.
    pub calc_items: Vec<CalcItem>,
}

/// A calculated item: a formula over other items of its field.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CalcItem {
    pub name: String,
    /// Without `=`, as the file has it (`East+West`, `Region[East]*2`).
    pub formula: String,
}

impl CacheField {
    pub(crate) fn source(name: String) -> Self {
        CacheField {
            name,
            items: Vec::new(),
            num_fmt_id: 0,
            database: true,
            formula: None,
            group: None,
            calc_items: Vec::new(),
        }
    }

    /// Whether shared item `x` is one of its calculated items.
    pub(crate) fn is_calc(&self, x: usize) -> bool {
        !self.calc_items.is_empty()
            && self.items.get(x).is_some_and(|v| {
                self.calc_items
                    .iter()
                    .any(|c| Val::Str(c.name.clone()).key() == v.key())
            })
    }

    /// What pivot fields' items point at: a grouped field's group items,
    /// else the shared items.
    pub(crate) fn item_list(&self) -> &[Val] {
        match &self.group {
            Some(g) if g.groups_items() => &g.items,
            _ => &self.items,
        }
    }

    /// Grouped by ranges (it lists no shared items).
    pub(crate) fn range_grouped(&self) -> bool {
        matches!(
            &self.group,
            Some(FieldGroup {
                kind: Some(GroupKind::Range(_)),
                ..
            })
        )
    }
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
    /// Made in Kantor (for a new pivot table).
    pub added: bool,
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
    /// A calculated item (`f`).
    pub calc: bool,
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
    /// `subtotalCaption`: the label of each of its subtotals, `?` standing
    /// for the item.
    pub subtotal_caption: Option<String>,
    /// Custom subtotals (Field Settings: Custom), in Excel's order; empty
    /// for none or the automatic one (`default_subtotal`).
    pub subtotals: Vec<Func>,
    /// `insertBlankRow`: a blank line after each item.
    pub insert_blank: bool,
    /// Excel 2010 `fillDownLabels`: repeat item labels.
    pub fill_down: bool,
    /// `autoSortScope`: items sorted by this data field's values (in the
    /// `sort` direction).
    pub sort_by: Option<usize>,
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

    pub(crate) const ALL: [Func; 11] = [
        Func::Sum,
        Func::Count,
        Func::Average,
        Func::Max,
        Func::Min,
        Func::Product,
        Func::CountNums,
        Func::StdDev,
        Func::StdDevp,
        Func::Var,
        Func::Varp,
    ];

    /// The `subtotal` attribute value.
    pub(crate) fn name(self) -> &'static str {
        match self {
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
        }
    }

    /// A field's custom subtotal: its item type (`ST_ItemType`) and the
    /// `pivotField` attribute that turns it on.
    pub(crate) fn subtotal_xml(self) -> (&'static str, &'static str) {
        match self {
            Func::Sum => ("sum", "sumSubtotal"),
            Func::Count => ("countA", "countASubtotal"),
            Func::Average => ("avg", "avgSubtotal"),
            Func::Max => ("max", "maxSubtotal"),
            Func::Min => ("min", "minSubtotal"),
            Func::Product => ("product", "productSubtotal"),
            Func::CountNums => ("count", "countSubtotal"),
            Func::StdDev => ("stdDev", "stdDevSubtotal"),
            Func::StdDevp => ("stdDevP", "stdDevPSubtotal"),
            Func::Var => ("var", "varSubtotal"),
            Func::Varp => ("varP", "varPSubtotal"),
        }
    }

    /// What a custom subtotal's label says after the item (`East Sum`).
    pub(crate) fn subtotal_label(self) -> &'static str {
        match self {
            Func::CountNums => "Count Numbers",
            f => f.caption(),
        }
    }

    /// What Excel calls it in a data field's default name.
    pub(crate) fn caption(self) -> &'static str {
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

    /// A value field's default name, `Sum of Units`, in the interface
    /// language (new content; see [`crate::content_text`]).
    pub(crate) fn default_name(self, field: &str) -> String {
        trf("{} of {}", &[&tr(self.caption()), &field])
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
    /// Show Values As.
    pub show_as: ShowAs,
    pub base_field: usize,
    /// An index into the base field's items, or [`BASE_PREVIOUS`] /
    /// [`BASE_NEXT`].
    pub base_item: u32,
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
    /// Label, value and date filters.
    pub filters: Vec<PivotFilter>,
    /// `multipleFieldFilters`: a field may have a label (or date) filter
    /// and a value (or top 10) filter at once (PivotTable Options' "Allow
    /// multiple filters per field").
    pub multiple_filters: bool,
    pub row_grand: bool,
    pub col_grand: bool,
    pub style: StyleInfo,
    pub grand_caption: Option<String>,
    pub row_header_caption: Option<String>,
    pub col_header_caption: Option<String>,
    pub data_caption: String,
    /// `missingCaption` when `showMissing`: shown in empty value cells.
    pub missing_caption: Option<String>,
    /// `errorCaption` when `showError`: shown in place of error values.
    pub error_caption: Option<String>,
    /// The definition as read.
    pub xml: Arc<String>,
    /// Item indexes changed since the file was read: drop `<formats>` and
    /// the like, which point at items by index.
    pub reindexed: bool,
    pub at: Layout,
    pub unsupported: Option<String>,
    /// Made in Kantor: it exists only in the history states that list it
    /// (see [`State::defs`]).
    pub added: bool,
    /// Its parts are in the package the next save starts from (the file
    /// opened or last saved): saved by carrying them, else written anew.
    pub in_origin: bool,
    /// Root attributes a layout change sets (`compact`, `outline`...).
    pub root_sets: Vec<(&'static str, String)>,
    /// `indent`: how far each level of a compact row field is indented
    /// (Excel's "When in compact form indent row labels"; Excel writes 0
    /// for its default of one step and still indents).
    pub indent: u32,
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
        for (k, f) in children(cf, "cacheField").enumerate() {
            let database = attr_bool(f, "databaseField", true);
            let formula = f.attribute("formula").map(|x| x.trim().to_owned());
            if !database && formula.is_none() && child(f, "fieldGroup").is_none() {
                unsupported.get_or_insert_with(|| "it has a field Kantor can't read".to_owned());
            }
            if database && fields.iter().any(|x: &CacheField| !x.database) {
                unsupported.get_or_insert_with(|| {
                    "its calculated or grouping fields come before its source fields".to_owned()
                });
            }
            let group = match child(f, "fieldGroup").map(|g| FieldGroup::parse(g, k)) {
                None => None,
                Some(Ok(g)) => Some(g),
                Some(Err(why)) => {
                    unsupported.get_or_insert_with(|| format!("it has {why}"));
                    None
                }
            };
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
                database,
                formula,
                group,
                calc_items: Vec::new(),
            });
        }
    }
    if let Some(list) = child(root, "calculatedItems") {
        for ci in children(list, "calculatedItem") {
            // The item: a reference to one shared item of one field.
            let reference = ci
                .descendants()
                .find(|n| n.is_element() && n.tag_name().name() == "reference");
            let found = reference.and_then(|r| {
                let f = attr_u32(r, "field")? as usize;
                let x = children(r, "x")
                    .next()
                    .map(|x| attr_u32(x, "v").unwrap_or(0))? as usize;
                let name = fields.get(f)?.items.get(x)?.label();
                Some((f, name))
            });
            match (found, ci.attribute("formula")) {
                (Some((f, name)), Some(formula))
                    if fields[f].database && fields[f].group.is_none() =>
                {
                    fields[f].calc_items.push(CalcItem {
                        name,
                        formula: formula.trim().trim_start_matches('=').to_owned(),
                    });
                }
                _ => {
                    unsupported.get_or_insert_with(|| {
                        "it has a calculated item Kantor can't read".to_owned()
                    });
                }
            }
        }
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
        added: false,
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
            let subtotals: Vec<Func> = Func::ALL
                .into_iter()
                .filter(|func| attr_bool(f, func.subtotal_xml().1, false))
                .collect();
            let fill_down = f
                .descendants()
                .find(|n| {
                    n.is_element()
                        && n.tag_name().name() == "pivotField"
                        && n.tag_name().namespace() == Some(NS_X14)
                })
                .is_some_and(|n| attr_bool(n, "fillDownLabels", false));
            // Sorted by a data field's values: a reference to the Σ Values
            // field (4294967294) naming the data field.
            let sort_by = child(f, "autoSortScope").and_then(|a| {
                a.descendants()
                    .filter(|n| n.is_element() && n.tag_name().name() == "reference")
                    .find(|r| r.attribute("field") == Some("4294967294"))
                    .and_then(|r| children(r, "x").next())
                    .map(|x| attr_u32(x, "v").unwrap_or(0) as usize)
            });
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
                            calc: attr_bool(it, "f", false),
                        })
                        .collect()
                })
                .unwrap_or_default();
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
                subtotal_caption: f.attribute("subtotalCaption").map(str::to_owned),
                subtotals,
                insert_blank: attr_bool(f, "insertBlankRow", false),
                fill_down,
                sort_by,
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
            let Some(show_as) = ShowAs::parse(d) else {
                unsupported.get_or_insert_with(|| "a value is shown as a calculation".to_owned());
                continue;
            };
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
                show_as,
                base_field: attr_u32(d, "baseField").unwrap_or(0) as usize,
                base_item: attr_u32(d, "baseItem").unwrap_or(0),
                xml: Some(xml[d.range()].to_owned()),
            });
        }
    }
    let mut filters = Vec::new();
    if let Some(l) = child(root, "filters") {
        for f in children(l, "filter") {
            match PivotFilter::parse(f) {
                Ok(f) => filters.push(f),
                Err(why) => {
                    unsupported.get_or_insert_with(|| format!("it has {why}"));
                }
            }
        }
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
        filters,
        multiple_filters: attr_bool(root, "multipleFieldFilters", true),
        row_grand: attr_bool(root, "rowGrandTotals", true),
        col_grand: attr_bool(root, "colGrandTotals", true),
        style,
        grand_caption: root.attribute("grandTotalCaption").map(str::to_owned),
        row_header_caption: root.attribute("rowHeaderCaption").map(str::to_owned),
        col_header_caption: root.attribute("colHeaderCaption").map(str::to_owned),
        data_caption: root.attribute("dataCaption").unwrap_or("Values").to_owned(),
        missing_caption: attr_bool(root, "showMissing", true)
            .then(|| root.attribute("missingCaption").map(str::to_owned))
            .flatten(),
        error_caption: attr_bool(root, "showError", false)
            .then(|| root.attribute("errorCaption").map(str::to_owned))
            .flatten(),
        xml: Arc::new(xml.to_owned()),
        reindexed: false,
        at: Layout::default(),
        unsupported,
        added: false,
        in_origin: true,
        root_sets: Vec::new(),
        indent: attr_u32(root, "indent").unwrap_or(1),
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
    /// The file's slicers and timelines (see [`crate::slicer`]).
    pub slicers: crate::slicer::Slicers,
}

impl Pivots {
    pub(crate) fn read(bytes: &[u8]) -> Self {
        let slicers = crate::slicer::Slicers::read(bytes);
        let sources = match crate::preserve::pivot_sources(bytes) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("pivot tables not read: {e}");
                return Self {
                    slicers,
                    ..Self::default()
                };
            }
        };
        let mut out = Pivots {
            slicers,
            ..Pivots::default()
        };
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
    /// Pivot tables deleted.
    pub gone: std::collections::HashSet<usize>,
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
    /// Per body row: the label column (offset from the left) holding a
    /// compact row label and its indent.
    indents: Vec<Option<(u32, u32)>>,
}

/// Each row item's compact label: its column offset and indent (Excel
/// indents an inner item of a compact field by its level below the
/// outermost field sharing the column).
fn compact_indents(def: &Def) -> Vec<Option<(u32, u32)>> {
    let nf = def.fields.len();
    let lv = levels(&def.row_fields, nf);
    let compact = |l: &Level| match l {
        Level::Field(f) => def.fields[*f].compact && def.fields[*f].outline,
        Level::Values => true,
    };
    let outline = |l: &Level| match l {
        Level::Field(f) => def.fields[*f].outline,
        Level::Values => true,
    };
    let mut col: Vec<u32> = Vec::with_capacity(lv.len());
    for (i, l) in lv.iter().enumerate() {
        let c = if i == 0 {
            0
        } else if compact(l) && outline(&lv[i - 1]) {
            col[i - 1]
        } else {
            col[i - 1] + 1
        };
        col.push(c);
    }
    def.row_items
        .iter()
        .map(|it| {
            if matches!(it.t.as_str(), "grand" | "blank") {
                return None;
            }
            let depth = (it.r + it.x.len()).checked_sub(1)?;
            let c = *col.get(depth)?;
            let level = col[..depth].iter().filter(|x| **x == c).count() as u32;
            (level > 0).then_some((c, level * def.indent.max(1)))
        })
        .collect()
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
        let indents = compact_indents(&def);
        Placed {
            index,
            def,
            area,
            page,
            rows,
            cols,
            style,
            indents,
        }
    }

    /// The indent of cell `(row, col)`'s compact row label, if it has one.
    fn indent(&self, row: u32, col: u32) -> u32 {
        let top = self.area.top + self.def.first_data_row;
        row.checked_sub(top)
            .and_then(|k| self.indents.get(k as usize).copied().flatten())
            .filter(|(c, _)| self.area.left + c == col)
            .map_or(0, |(_, n)| n)
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
        let indent = self.indent(row, col);
        let Some(s) = self.style.as_ref() else {
            return (indent > 0).then(|| TableCellLook {
                indent,
                ..Default::default()
            });
        };
        let d = &self.def;
        if d.row_fields.is_empty() && d.col_fields.is_empty() && d.data_fields.is_empty() {
            // Excel's placeholder for a pivot with no fields: plain.
            return None;
        }
        let mut look = look_of(
            s,
            &Frame {
                style: &d.style,
                area: self.area,
                page: self.page,
                first_data_row: d.first_data_row,
                first_data_col: d.first_data_col,
                rows: &self.rows,
                cols: &self.cols,
            },
            row,
            col,
        );
        look.indent = indent;
        Some(look)
    }
}

/// What a pivot table's style draws on: its area, report filters and the
/// roles of its rows and columns.
struct Frame<'a> {
    style: &'a StyleInfo,
    area: Area,
    page: Option<Area>,
    first_data_row: u32,
    first_data_col: u32,
    rows: &'a [Line],
    cols: &'a [Line],
}

/// The look style `s` gives cell `(row, col)` of `f`.
fn look_of(s: &PivotStyleDef, f: &Frame, row: u32, col: u32) -> TableCellLook {
    {
        let a = f.area;
        let mut look = TableCellLook::default();
        if let Some(p) = f.page
            && row >= p.top
            && row <= p.bottom
        {
            let label = (col - p.left).is_multiple_of(3);
            let e = if label { &s.page_label } else { &s.page_value };
            look.apply(e, Edges::all());
            return look;
        }
        let edges = Edges {
            top: row == a.top,
            bottom: row == a.bottom,
            left: col == a.left,
            right: col == a.right,
        };
        look.apply(&s.whole, edges);
        let data_top = a.top + f.first_data_row;
        let data_left = a.left + f.first_data_col;
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
            .and_then(|i| f.rows.get(i as usize))
            .copied();
        let col_line = col
            .checked_sub(data_left)
            .and_then(|i| f.cols.get(i as usize))
            .copied();
        if f.style.row_stripes && row_line == Some(Line::Body) {
            let e = if (row - data_top).is_multiple_of(2) {
                &s.row_stripe
            } else {
                &s.row_stripe2
            };
            look.apply(e, row_edges);
        }
        if f.style.col_stripes && col_line == Some(Line::Body) {
            let e = if (col - data_left).is_multiple_of(2) {
                &s.col_stripe
            } else {
                &s.col_stripe2
            };
            look.apply(e, col_edges);
        }
        if f.style.row_headers && col < data_left && row >= data_top {
            look.apply(&s.first_col, col_edges);
        }
        if row < data_top {
            if f.style.col_headers {
                look.apply(&s.header, row_edges);
            }
            if row == a.top && col == a.left {
                look.apply(&s.first_header_cell, Edges::all());
            }
            // The labels of an outer column field.
            if row > a.top && col >= data_left && row + 1 < data_top {
                let l = ((row - a.top - 1) as usize).min(2);
                look.apply(&s.col_subheading[l], row_edges);
            }
        }
        if let Some(Line::Subtotal(l)) = col_line {
            look.apply(&s.subtotal_col[l as usize], col_edges);
        }
        match row_line {
            Some(Line::Subheading(l)) => look.apply(&s.subheading[l as usize], row_edges),
            Some(Line::Subtotal(l)) => look.apply(&s.subtotal_row[l as usize], row_edges),
            Some(Line::Blank) => look.apply(&s.blank_row, row_edges),
            _ => {}
        }
        if f.style.last_column && col_line == Some(Line::Grand) {
            look.apply(&s.last_col, col_edges);
        }
        if row_line == Some(Line::Grand) {
            look.apply(&s.total, row_edges);
        }
        look
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
    row_stripe2: Elem,
    col_stripe2: Elem,
    first_col: Elem,
    header: Elem,
    first_header_cell: Elem,
    subtotal_col: [Elem; 3],
    subheading: [Elem; 3],
    col_subheading: [Elem; 3],
    subtotal_row: [Elem; 3],
    blank_row: Elem,
    last_col: Elem,
    total: Elem,
    page_label: Elem,
    page_value: Elem,
}

/// The pivot elements as OOXML types, with their place in a
/// [`PivotStyleDef`].
fn pivot_slots(s: &mut PivotStyleDef) -> Vec<(&'static str, &mut Elem)> {
    let [st1, st2, st3] = &mut s.subtotal_col;
    let [sr1, sr2, sr3] = &mut s.subtotal_row;
    let [rh1, rh2, rh3] = &mut s.subheading;
    let [ch1, ch2, ch3] = &mut s.col_subheading;
    vec![
        ("wholeTable", &mut s.whole),
        ("pageFieldLabels", &mut s.page_label),
        ("pageFieldValues", &mut s.page_value),
        ("firstColumnStripe", &mut s.col_stripe),
        ("secondColumnStripe", &mut s.col_stripe2),
        ("firstRowStripe", &mut s.row_stripe),
        ("secondRowStripe", &mut s.row_stripe2),
        ("firstColumn", &mut s.first_col),
        ("headerRow", &mut s.header),
        ("firstHeaderCell", &mut s.first_header_cell),
        ("firstSubtotalColumn", st1),
        ("secondSubtotalColumn", st2),
        ("thirdSubtotalColumn", st3),
        ("firstSubtotalRow", sr1),
        ("secondSubtotalRow", sr2),
        ("thirdSubtotalRow", sr3),
        ("blankRow", &mut s.blank_row),
        ("firstColumnSubheading", ch1),
        ("secondColumnSubheading", ch2),
        ("thirdColumnSubheading", ch3),
        ("firstRowSubheading", rh1),
        ("secondRowSubheading", rh2),
        ("thirdRowSubheading", rh3),
        ("lastColumn", &mut s.last_col),
        ("totalRow", &mut s.total),
    ]
}

impl PivotStyleDef {
    /// A custom PivotTable style (`<tableStyle pivot="1">`) as drawn.
    pub(crate) fn from_table_def(d: &crate::table::TableStyleDef) -> Self {
        let mut s = PivotStyleDef::default();
        for (kind, slot) in pivot_slots(&mut s) {
            if let Some((e, _)) = d.get(kind) {
                *slot = e.clone();
            }
        }
        s
    }

    /// This style's elements as a table style definition (for New
    /// PivotTable Style from a built-in one).
    pub(crate) fn to_table_def(&self) -> crate::table::TableStyleDef {
        let mut s = self.clone();
        let mut d = crate::table::TableStyleDef::empty();
        for (kind, slot) in pivot_slots(&mut s) {
            if !slot.is_empty() {
                d.set(kind, slot.clone(), 1);
            }
        }
        d
    }
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
    // Banded rows and columns: a tint of the colour (a deeper one on a
    // filled body, a darker shade for the dark styles).
    let band = match family {
        "dark" if k == 0 => theme(t, 1, 0.25),
        "dark" => accent(-0.35),
        _ if s.whole.fill.is_some() => accent(0.6),
        _ => accent(0.8),
    };
    s.row_stripe.fill = Some(band);
    s.col_stripe.fill = Some(band);
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
    /// A blank line after an item (`insertBlankRow`).
    Blank,
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
    /// A custom subtotal's function (instead of the data field's).
    func: Option<Func>,
}

impl Entry {
    fn new(kind: Kind, path: Vec<usize>, recs: Vec<usize>, data: Option<usize>) -> Self {
        Entry {
            kind,
            path,
            recs,
            values: kind != Kind::Blank,
            data,
            func: None,
        }
    }
}

/// What a layout works from: the definition matched to its cache, the
/// records and each record's item in each field.
struct Ctx<'a> {
    def: &'a Def,
    cache: &'a Cache,
    records: &'a [Vec<Val>],
    /// Each record's item position (in the definition's item list) in each
    /// field.
    pos: Vec<Vec<Option<usize>>>,
    /// Calculated fields' formulas, by cache field.
    formulas: Vec<Option<std::result::Result<Formula, String>>>,
    /// Calculated items' formulas, by field and item position (their
    /// `fields` are item positions of the same field).
    calc: Vec<HashMap<usize, std::result::Result<Formula, String>>>,
}

/// The calculated items of each field of `def` (matched to `cache`),
/// compiled against the field's item names; `order` gives a field's items
/// as they show when that differs from their list (sorted by values), for
/// items named by position.
fn calc_formulas(
    def: &Def,
    cache: &Cache,
    order: &dyn Fn(usize) -> Option<Vec<usize>>,
) -> Vec<HashMap<usize, std::result::Result<Formula, String>>> {
    def.fields
        .iter()
        .enumerate()
        .map(|(f, fd)| {
            let mut out = HashMap::new();
            let Some(cf) = cache.fields.get(f) else {
                return out;
            };
            if cf.calc_items.is_empty() {
                return out;
            }
            let names: Vec<String> = fd
                .items
                .iter()
                .map(|it| {
                    it.x.and_then(|x| cf.items.get(x))
                        .map_or_else(|| "\u{0}".to_owned(), Val::label)
                })
                .collect();
            let shown = order(f);
            for (k, it) in fd.items.iter().enumerate() {
                let Some(v) = it.x.and_then(|x| cf.items.get(x)) else {
                    continue;
                };
                if let Some(ci) = cf
                    .calc_items
                    .iter()
                    .find(|c| Val::Str(c.name.clone()).key() == v.key())
                {
                    out.insert(
                        k,
                        Formula::compile_items(
                            &ci.formula,
                            &cf.name,
                            &names,
                            Some(k),
                            shown.as_deref(),
                        ),
                    );
                }
            }
            out
        })
        .collect()
}

/// A total over a field with calculated items: `a` and a calculated item's
/// value `b` together, as `func` totals them (Excel counts calculated items
/// in totals; averages and deviations leave them out here).
fn combine(func: Func, a: Agg, b: Agg) -> Agg {
    match (a, b) {
        (None, x) | (x, None) => x,
        (Some(Err(e)), _) | (_, Some(Err(e))) => Some(Err(e)),
        (Some(Ok(x)), Some(Ok(y))) => Some(Ok(match func {
            Func::Sum | Func::Count | Func::CountNums => x + y,
            Func::Max => x.max(y),
            Func::Min => x.min(y),
            Func::Product => x * y,
            _ => x,
        })),
    }
}

type Agg = Option<std::result::Result<f64, &'static str>>;

/// An item of an axis level: its position, records, data field and field.
type Group<'a> = (usize, Vec<usize>, Option<usize>, &'a Field);

impl Ctx<'_> {
    /// Field `f`'s items in the order they show when sorted by a value
    /// field (as places in its item list; calculated items, which have no
    /// records, last), over all records. `None`: they show as listed.
    fn shown_order(&self, f: usize) -> Option<Vec<usize>> {
        let field = self.def.fields.get(f)?;
        let d = field.sort_by.filter(|d| *d < self.def.data_fields.len())?;
        let desc = field.sort.as_deref() == Some("descending");
        let mut by_item: Vec<Vec<usize>> = vec![Vec::new(); field.items.len()];
        for (r, p) in self.pos.get(f)?.iter().enumerate() {
            if let Some(p) = *p {
                by_item[p].push(r);
            }
        }
        let mut keyed: Vec<(f64, usize)> = (0..field.items.len())
            .filter(|&k| field.items[k].t.is_none())
            .map(|k| {
                let key = match self.summarise(d, None, &by_item[k]) {
                    Some(Ok(x)) => x,
                    _ => f64::NAN,
                };
                (key, k)
            })
            .collect();
        keyed.sort_by(|a, b| match (a.0.is_nan(), b.0.is_nan()) {
            (true, true) => std::cmp::Ordering::Equal,
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            _ => {
                let o = a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal);
                if desc { o.reverse() } else { o }
            }
        });
        Some(keyed.into_iter().map(|(_, k)| k).collect())
    }

    fn is_calc(&self, f: usize, k: usize) -> bool {
        self.calc.get(f).is_some_and(|m| m.contains_key(&k))
    }

    fn has_calc(&self) -> bool {
        self.calc.iter().any(|m| !m.is_empty())
    }

    /// Data field `d` over records `recs` (by `func` for a custom
    /// subtotal). A calculated field is its formula over the sums of the
    /// fields it names, as in Excel.
    fn summarise(&self, d: usize, func: Option<Func>, recs: &[usize]) -> Agg {
        let df = &self.def.data_fields[d];
        if let Some(f) = self.formulas.get(df.fld).and_then(|f| f.as_ref()) {
            if recs.is_empty() {
                return None;
            }
            let Ok(f) = f else {
                return Some(Err("#NAME?"));
            };
            return Some(f.eval(&|k| {
                recs.iter()
                    .filter_map(|&r| self.records[r].get(k)?.number())
                    .sum()
            }));
        }
        let vals: Vec<&Val> = recs
            .iter()
            .filter_map(|&r| self.records[r].get(df.fld))
            .collect();
        func.unwrap_or(df.func).apply(&vals)
    }

    /// The cache value behind item `k` of field `f` (not its caption).
    fn item_value(&self, f: usize, k: usize) -> Val {
        self.def.fields[f].items[k]
            .x
            .and_then(|x| self.cache.fields.get(f)?.item_list().get(x).cloned())
            .unwrap_or(Val::Missing)
    }

    /// `recs` without what the label, date, value and top 10 filters of
    /// the axis fields leave out. Value filters on an inner field apply
    /// within each item of the fields outside it, as in Excel.
    fn filtered(&self, row_lv: &[Level], col_lv: &[Level], mut recs: Vec<usize>) -> Vec<usize> {
        let def = self.def;
        let on_axis =
            |f: usize| row_lv.contains(&Level::Field(f)) || col_lv.contains(&Level::Field(f));
        let today = crate::pivot_calc::today();
        for flt in &def.filters {
            let f = flt.fld;
            if f >= def.fields.len() {
                continue;
            }
            if !on_axis(f) {
                // A timeline's date filter on a field on no axis filters
                // the records by their dates.
                if flt.name.is_some() && filter_family(&flt.kind) == Some(FilterFamily::Date) {
                    recs.retain(|&r| match self.records[r].get(f) {
                        Some(Val::Date(d) | Val::Num(d)) => {
                            crate::pivot_calc::date_matches(&flt.kind, *d, &flt.v1, &flt.v2, today)
                        }
                        _ => false,
                    });
                }
                continue;
            }
            let pass: Vec<bool> = match filter_family(&flt.kind) {
                Some(FilterFamily::Label) => (0..def.fields[f].items.len())
                    .map(|k| {
                        let label = def.item_label(self.cache, f, k).label();
                        crate::pivot_calc::label_matches(&flt.kind, &label, &flt.v1, &flt.v2)
                    })
                    .collect(),
                Some(FilterFamily::Date) => (0..def.fields[f].items.len())
                    .map(|k| match self.item_value(f, k) {
                        Val::Date(d) | Val::Num(d) => {
                            crate::pivot_calc::date_matches(&flt.kind, d, &flt.v1, &flt.v2, today)
                        }
                        _ => false,
                    })
                    .collect(),
                _ => continue,
            };
            recs.retain(|&r| self.pos[f][r].is_some_and(|k| pass[k]));
        }
        for lv in [row_lv, col_lv] {
            for (l, level) in lv.iter().enumerate() {
                let Level::Field(f) = *level else { continue };
                let Some(flt) = def.filters.iter().find(|x| {
                    x.fld == f
                        && matches!(
                            filter_family(&x.kind),
                            Some(FilterFamily::Value | FilterFamily::Top)
                        )
                }) else {
                    continue;
                };
                let Some(d) = flt.measure.filter(|d| *d < def.data_fields.len()) else {
                    continue;
                };
                let outer: Vec<usize> = lv[..l]
                    .iter()
                    .filter_map(|x| match x {
                        Level::Field(o) => Some(*o),
                        Level::Values => None,
                    })
                    .collect();
                // Records by the items outside, then by this field's item.
                let mut groups: HashMap<Vec<Option<usize>>, HashMap<usize, Vec<usize>>> =
                    HashMap::new();
                for &r in &recs {
                    let key: Vec<Option<usize>> = outer.iter().map(|&o| self.pos[o][r]).collect();
                    if let Some(k) = self.pos[f][r] {
                        groups.entry(key).or_default().entry(k).or_default().push(r);
                    }
                }
                let mut keep: std::collections::HashSet<(Vec<Option<usize>>, usize)> =
                    std::collections::HashSet::new();
                for (key, items) in &groups {
                    let values: Vec<(usize, f64)> = items
                        .iter()
                        .filter_map(|(k, rs)| match self.summarise(d, None, rs) {
                            Some(Ok(x)) => Some((*k, x)),
                            _ => None,
                        })
                        .collect();
                    let kept: Vec<usize> = if filter_family(&flt.kind) == Some(FilterFamily::Top) {
                        let n: f64 = flt.v1.trim().parse().unwrap_or(10.0);
                        crate::pivot_calc::top_items(&flt.kind, n, flt.top, &values)
                    } else {
                        values
                            .iter()
                            .filter(|(_, x)| {
                                crate::pivot_calc::value_matches(&flt.kind, *x, &flt.v1, &flt.v2)
                            })
                            .map(|(k, _)| *k)
                            .collect()
                    };
                    for k in kept {
                        keep.insert((key.clone(), k));
                    }
                }
                recs.retain(|&r| {
                    let key: Vec<Option<usize>> = outer.iter().map(|&o| self.pos[o][r]).collect();
                    self.pos[f][r].is_some_and(|k| keep.contains(&(key, k)))
                });
            }
        }
        recs
    }

    /// Rows or columns of an axis, in order.
    fn entries(&self, lv: &[Level], recs: &[usize], grand: bool, rows: bool) -> Vec<Entry> {
        let def = self.def;
        let mut out = Vec::new();
        if lv.is_empty() {
            out.push(Entry::new(Kind::Data, Vec::new(), recs.to_vec(), None));
            return out;
        }
        self.walk(lv, rows, recs, 0, &mut Vec::new(), None, &mut out);
        let real = lv.iter().any(|l| matches!(l, Level::Field(_)));
        if grand && real {
            if lv.contains(&Level::Values) {
                for d in 0..def.data_fields.len() {
                    out.push(Entry::new(Kind::Grand, Vec::new(), recs.to_vec(), Some(d)));
                }
            } else {
                out.push(Entry::new(Kind::Grand, Vec::new(), recs.to_vec(), None));
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn walk(
        &self,
        lv: &[Level],
        rows: bool,
        recs: &[usize],
        level: usize,
        path: &mut Vec<usize>,
        data: Option<usize>,
        out: &mut Vec<Entry>,
    ) {
        let def = self.def;
        let leaf = level + 1 == lv.len();
        let mut groups: Vec<Group> = match lv[level] {
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
                    if let Some(p) = self.pos[f][r] {
                        by_item[p].push(r);
                    }
                }
                field
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, it)| it.t.is_none() && !it.hidden)
                    .map(|(k, _)| (k, std::mem::take(&mut by_item[k]), data, field))
                    .filter(|(k, sub, _, _)| {
                        field.show_all || !sub.is_empty() || self.is_calc(f, *k)
                    })
                    .collect()
            }
        };
        // Sorted by a value field's values.
        if let Level::Field(f) = lv[level]
            && let Some(d) = def.fields[f].sort_by.filter(|d| *d < def.data_fields.len())
        {
            let desc = def.fields[f].sort.as_deref() == Some("descending");
            let key = |sub: &[usize]| match self.summarise(d, None, sub) {
                Some(Ok(x)) => x,
                _ => f64::NAN,
            };
            let mut keyed: Vec<(f64, Group)> = groups.into_iter().map(|g| (key(&g.1), g)).collect();
            keyed.sort_by(|a, b| match (a.0.is_nan(), b.0.is_nan()) {
                (true, true) => std::cmp::Ordering::Equal,
                (true, false) => std::cmp::Ordering::Greater,
                (false, true) => std::cmp::Ordering::Less,
                _ => {
                    let o = a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal);
                    if desc { o.reverse() } else { o }
                }
            });
            groups = keyed.into_iter().map(|(_, g)| g).collect();
        }
        for (k, sub, d, field) in groups {
            path.push(k);
            let expanded = match lv[level] {
                Level::Field(_) => field.items[k].expanded,
                Level::Values => true,
            };
            if leaf || !expanded {
                out.push(Entry::new(Kind::Data, path.clone(), sub, d));
            } else {
                let tabular = !field.outline;
                // With the Σ Values field further down, a total across value
                // fields means nothing: no values on the item's own row and no
                // subtotal (as Excel shows it).
                let mixed = d.is_none() && lv[level + 1..].contains(&Level::Values);
                let funcs: Vec<Option<Func>> = if !field.subtotals.is_empty() {
                    field.subtotals.iter().map(|f| Some(*f)).collect()
                } else if field.default_subtotal {
                    vec![None]
                } else {
                    Vec::new()
                };
                let has_sub = !funcs.is_empty() && !mixed;
                // One subtotal can go on the item's own row; several go
                // below its items.
                let top = has_sub && field.subtotal_top && funcs.len() == 1 && !tabular;
                if !tabular {
                    let mut e = Entry::new(Kind::Data, path.clone(), sub.clone(), d);
                    e.values = top;
                    e.func = if top { funcs[0] } else { None };
                    out.push(e);
                }
                self.walk(lv, rows, &sub, level + 1, path, d, out);
                if has_sub && !top {
                    for func in funcs {
                        let mut e = Entry::new(Kind::Sub, path.clone(), sub.clone(), d);
                        e.func = func;
                        out.push(e);
                    }
                }
                if rows && field.insert_blank {
                    out.push(Entry::new(Kind::Blank, path.clone(), Vec::new(), d));
                }
            }
            path.pop();
        }
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
            Kind::Sub => (
                e.func.map_or("default", |f| f.subtotal_xml().0),
                0,
                e.path.clone(),
            ),
            Kind::Grand => ("grand", 0, vec![0]),
            Kind::Blank => ("blank", 0, e.path.clone()),
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
    pub(crate) fn field_caption(&self, cache: &Cache, f: usize) -> String {
        self.fields
            .get(f)
            .and_then(|x| x.name.clone())
            .or_else(|| cache.fields.get(f).map(|c| c.name.clone()))
            .unwrap_or_default()
    }

    pub(crate) fn data_caption_of(&self, cache: &Cache, d: usize) -> String {
        let df = &self.data_fields[d];
        df.name.clone().unwrap_or_else(|| {
            format!(
                "{} of {}",
                df.func.caption(),
                self.field_caption(cache, df.fld)
            )
        })
    }

    pub(crate) fn item_label(&self, cache: &Cache, f: usize, k: usize) -> Val {
        let it = &self.fields[f].items[k];
        if let Some(n) = &it.n {
            return Val::Str(n.clone());
        }
        it.x.and_then(|x| cache.fields[f].item_list().get(x).cloned())
            .unwrap_or(Val::Missing)
    }
}

/// Each record's item position (in `def`'s item list) in each field.
fn positions(def: &Def, cache: &Cache, records: &[Vec<Val>]) -> Vec<Vec<Option<usize>>> {
    cache
        .fields
        .iter()
        .enumerate()
        .map(|(f, cache_field)| {
            let mut by_x: HashMap<usize, usize> = HashMap::new();
            if let Some(field) = def.fields.get(f) {
                for (k, it) in field.items.iter().enumerate() {
                    if let Some(x) = it.x {
                        by_x.insert(x, k);
                    }
                }
            }
            if let Some(groups) = crate::pivot_group::group_positions(&cache.fields, f, records) {
                return groups
                    .into_iter()
                    .map(|g| g.and_then(|g| by_x.get(&g).copied()))
                    .collect();
            }
            let key_index: HashMap<String, usize> = cache_field
                .items
                .iter()
                .enumerate()
                .filter_map(|(x, v)| Some((v.key(), *by_x.get(&x)?)))
                .collect();
            records
                .iter()
                .map(|r| r.get(f).and_then(|v| key_index.get(&v.key()).copied()))
                .collect()
        })
        .collect()
}

/// Lay out `def` (already matched to `cache`) with its top-left corner at
/// `(top, left)`.
pub(crate) fn lay_out(def: &Def, cache: &Cache, records: &[Vec<Val>], top: u32, left: u32) -> Laid {
    let nf = cache.fields.len();
    let names: Vec<String> = cache.fields.iter().map(|c| c.name.clone()).collect();
    let ctx = Ctx {
        def,
        cache,
        records,
        pos: positions(def, cache, records),
        formulas: cache
            .fields
            .iter()
            .map(|c| c.formula.as_ref().map(|f| Formula::compile(f, &names)))
            .collect(),
        calc: Vec::new(),
    };
    let calc = calc_formulas(def, cache, &|f| ctx.shown_order(f));
    let mut ctx = ctx;
    ctx.calc = calc;
    let pos = &ctx.pos;
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
        // A field on no axis with hidden items (a slicer's) filters too.
        .chain((0..def.fields.len().min(nf)).filter(|&f| {
            def.fields[f].axis.is_none() && def.fields[f].items.iter().any(|i| i.hidden)
        }))
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
    let recs = ctx.filtered(&row_lv, &col_lv, recs);
    // `colGrandTotals` totals each column (a row at the bottom),
    // `rowGrandTotals` each row (a column at the right).
    let rows = ctx.entries(&row_lv, &recs, def.col_grand, true);
    let cols = ctx.entries(&col_lv, &recs, def.row_grand, false);

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
    // Labels the file doesn't hold (Excel writes them in its interface
    // language at each refresh): English for pivot tables from a file, the
    // interface language for ones made here.
    let local = |en: &'static str| if def.added { tr(en) } else { en };
    let fill = |template: &'static str, v: &dyn std::fmt::Display| {
        if def.added {
            trf(template, &[v])
        } else {
            template.replacen("{}", &v.to_string(), 1)
        }
    };
    let shown = |v: Val| match v {
        Val::Missing => text_input(local("(blank)")),
        v => v.input(),
    };
    // A subtotal's label: the field's `subtotalCaption`, else `East Total`
    // (`East Sum` for a custom subtotal).
    let sub_label = |lv: &Level, k: usize, func: Option<Func>| {
        let v = label(lv, k).label();
        if let Some(func) = func {
            return format!("{v} {}", local(func.subtotal_label()));
        }
        match lv {
            Level::Field(f) => match &def.fields[*f].subtotal_caption {
                Some(c) => c.replace('?', &v),
                None => fill("{} Total", &v),
            },
            Level::Values => fill("{} Total", &v),
        }
    };
    let grand_label = |e: &Entry| match (e.data, &def.grand_caption) {
        (Some(d), _) if def.data_fields.len() > 1 => {
            fill("Total {}", &def.data_caption_of(cache, d))
        }
        (_, Some(c)) => c.clone(),
        _ => local("Grand Total").to_owned(),
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
                    let s = sub_label(&col_lv[l], e.path[l], e.func);
                    put(top + 1 + l as u32, c, text_input(&s));
                }
                Kind::Blank => {}
                Kind::Data => {
                    let r = repeats(prev, e);
                    prev = Some(e);
                    for (l, k) in e.path.iter().enumerate().skip(r) {
                        put(top + 1 + l as u32, c, shown(label(&col_lv[l], *k)));
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
                let s = sub_label(&row_lv[l], e.path[l], e.func);
                put(r, left + label_col[l], text_input(&s));
            }
            Kind::Blank => {}
            Kind::Data => {
                let rep = repeats(prev, e);
                prev = Some(e);
                let last = e.path.len().saturating_sub(1);
                for (l, k) in e.path.iter().enumerate() {
                    // An outer level shows here only when its field is in
                    // tabular form (else it has its own row above), and
                    // once, unless its labels repeat (not in compact form).
                    let (own_row, fill_down) = match row_lv[l] {
                        Level::Field(f) => {
                            let fd = &def.fields[f];
                            (fd.outline, fd.fill_down && !(fd.compact && fd.outline))
                        }
                        Level::Values => (true, false),
                    };
                    if (l < rep || (l < last && own_row)) && !fill_down {
                        continue;
                    }
                    put(r, left + label_col[l], shown(label(&row_lv[l], *k)));
                }
            }
        }
    }

    // Values.
    let mut num_fmts: Vec<(Area, u32)> = Vec::new();
    if !def.data_fields.is_empty() {
        let shows = ShowCtx {
            ctx: &ctx,
            recs: &recs,
            row_lv: &row_lv,
            col_lv: &col_lv,
            rows: &rows,
            cols: &cols,
        };
        for (i, re) in rows.iter().enumerate() {
            if !re.values {
                continue;
            }
            for (j, ce) in cols.iter().enumerate() {
                if !ce.values {
                    continue;
                }
                let d = re.data.or(ce.data).unwrap_or(0);
                let v = match (
                    shows.value(d, re, ce),
                    &def.missing_caption,
                    &def.error_caption,
                ) {
                    (None, Some(c), _) => text_input(c),
                    (None, None, _) => continue,
                    (Some(Ok(x)), ..) => format_number(x),
                    (Some(Err(_)), _, Some(c)) => text_input(c),
                    (Some(Err(e)), _, None) => e.to_owned(),
                };
                put(data_top + i as u32, data_left + j as u32, v);
            }
        }
        // Number formats, by column (or row) of each data field.
        let bottom = data_top + rows.len() as u32 - 1;
        let right = data_left + cols.len() as u32 - 1;
        // "Show values as" a percentage: 0.00% unless the field has a
        // format of its own.
        let fmt = |d: usize| {
            let df = &def.data_fields[d];
            df.num_fmt_id
                .filter(|&n| n != 0)
                .or(df.show_as.is_percent().then_some(10))
        };
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
                    shown(def.item_label(cache, p.fld, sel))
                }
                _ if def.fields[p.fld].items.iter().any(|i| i.hidden) => {
                    local("(Multiple Items)").to_owned()
                }
                _ => local("(All)").to_owned(),
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

/// What "show values as" works from: a layout's records and axes.
struct ShowCtx<'a> {
    ctx: &'a Ctx<'a>,
    /// The records the filters let through.
    recs: &'a [usize],
    row_lv: &'a [Level],
    col_lv: &'a [Level],
    rows: &'a [Entry],
    cols: &'a [Entry],
}

/// Which items each field must have: `(field, allowed items)`.
type Spec = Vec<(usize, Vec<usize>)>;

fn spec_of(lv: &[Level], e: &Entry) -> Spec {
    if e.kind == Kind::Grand {
        return Vec::new();
    }
    e.path
        .iter()
        .enumerate()
        .filter_map(|(l, k)| match lv.get(l)? {
            Level::Field(f) => Some((*f, vec![*k])),
            Level::Values => None,
        })
        .collect()
}

impl ShowCtx<'_> {
    fn agg(&self, d: usize, func: Option<Func>, rows: &Spec, cols: &Spec) -> Agg {
        if self.ctx.has_calc() {
            let spec: Spec = rows.iter().chain(cols).cloned().collect();
            return self.cell(d, func, &spec, 0);
        }
        let pos = &self.ctx.pos;
        let recs: Vec<usize> = self
            .recs
            .iter()
            .copied()
            .filter(|&r| {
                rows.iter()
                    .chain(cols)
                    .all(|(f, ks)| pos[*f][r].is_some_and(|k| ks.contains(&k)))
            })
            .collect();
        self.ctx.summarise(d, func, &recs)
    }

    /// Data field `d` where the fields have the items `spec` says, with
    /// calculated items as Excel works them out: one in `spec` is its
    /// formula over the values of the items it names (with everything else
    /// in `spec` the same); a field on an axis but not in `spec` (a total
    /// across it) adds its shown calculated items to its items' total.
    fn cell(&self, d: usize, func: Option<Func>, spec: &Spec, depth: usize) -> Agg {
        let ctx = self.ctx;
        let def = ctx.def;
        if depth > 32 {
            return Some(Err("#REF!"));
        }
        for (i, (f, ks)) in spec.iter().enumerate() {
            let [k] = ks[..] else { continue };
            let Some(formula) = ctx.calc.get(*f).and_then(|m| m.get(&k)) else {
                continue;
            };
            let Ok(formula) = formula else {
                return Some(Err("#NAME?"));
            };
            let err: std::cell::Cell<Option<&'static str>> = std::cell::Cell::new(None);
            let any = std::cell::Cell::new(false);
            let v = formula.eval(&|j| {
                let mut s = spec.clone();
                s[i].1 = vec![j];
                match self.cell(d, func, &s, depth + 1) {
                    Some(Ok(x)) => {
                        any.set(true);
                        x
                    }
                    Some(Err(e)) => {
                        err.set(Some(e));
                        0.0
                    }
                    None => 0.0,
                }
            });
            if let Some(e) = err.get() {
                return Some(Err(e));
            }
            if !any.get() && !formula.fields.is_empty() {
                return None;
            }
            return Some(v);
        }
        for l in self.row_lv.iter().chain(self.col_lv) {
            let Level::Field(f) = *l else { continue };
            let Some(calc) = ctx.calc.get(f).filter(|m| !m.is_empty()) else {
                continue;
            };
            if spec.iter().any(|(g, _)| *g == f) {
                continue;
            }
            let items = &def.fields[f].items;
            let mut shown: Vec<usize> =
                calc.keys().copied().filter(|k| !items[*k].hidden).collect();
            shown.sort_unstable();
            let plain: Vec<usize> = (0..items.len())
                .filter(|k| !calc.contains_key(k) && items[*k].t.is_none())
                .collect();
            let mut s = spec.clone();
            s.push((f, plain));
            let mut acc = self.cell(d, func, &s, depth + 1);
            let total_by = func.unwrap_or(def.data_fields[d].func);
            for k in shown {
                let mut s = spec.clone();
                s.push((f, vec![k]));
                acc = combine(total_by, acc, self.cell(d, func, &s, depth + 1));
            }
            return acc;
        }
        let pos = &ctx.pos;
        let recs: Vec<usize> = self
            .recs
            .iter()
            .copied()
            .filter(|&r| {
                spec.iter()
                    .all(|(f, ks)| pos[*f][r].is_some_and(|k| ks.contains(&k)))
            })
            .collect();
        ctx.summarise(d, func, &recs)
    }

    /// The items of field `f` in the order an axis shows them.
    fn order(&self, rows_axis: bool, level: usize) -> Vec<usize> {
        let entries = if rows_axis { self.rows } else { self.cols };
        let mut out = Vec::new();
        for e in entries {
            if e.kind == Kind::Data
                && let Some(k) = e.path.get(level)
                && !out.contains(k)
            {
                out.push(*k);
            }
        }
        out
    }

    /// The value cell of data field `d` at row `re`, column `ce`.
    fn value(&self, d: usize, re: &Entry, ce: &Entry) -> Agg {
        let def = self.ctx.def;
        let df = &def.data_fields[d];
        let func = re.func.or(ce.func);
        let rs = spec_of(self.row_lv, re);
        let cs = spec_of(self.col_lv, ce);
        // The cell's records: the row's, in the column.
        let pos = &self.ctx.pos;
        let cell: Vec<usize> = re
            .recs
            .iter()
            .copied()
            .filter(|&r| {
                cs.iter()
                    .all(|(f, ks)| pos[*f][r].is_some_and(|k| ks.contains(&k)))
            })
            .collect();
        let raw = if self.ctx.has_calc() {
            self.agg(d, func, &rs, &cs)
        } else {
            self.ctx.summarise(d, func, &cell)
        };
        if df.show_as == ShowAs::Normal {
            return raw;
        }
        let Some(Ok(v)) = raw else { return raw };
        let ratio = |a: f64, b: Agg| -> Agg {
            match b {
                Some(Ok(b)) if b != 0.0 => Some(Ok(a / b)),
                Some(Ok(_)) => Some(Err("#DIV/0!")),
                Some(Err(e)) => Some(Err(e)),
                None => Some(Err("#NULL!")),
            }
        };
        let total = || self.agg(d, func, &Vec::new(), &Vec::new());
        let row_total = || self.agg(d, func, &rs, &Vec::new());
        let col_total = || self.agg(d, func, &Vec::new(), &cs);
        let parent = |s: &Spec| {
            let mut p = s.clone();
            p.pop();
            p
        };
        match df.show_as {
            ShowAs::Normal => raw,
            ShowAs::PercentOfTotal => ratio(v, total()),
            ShowAs::PercentOfColumn => ratio(v, col_total()),
            ShowAs::PercentOfRow => ratio(v, row_total()),
            ShowAs::PercentOfParentRow if re.kind == Kind::Grand => Some(Ok(1.0)),
            ShowAs::PercentOfParentRow => ratio(v, self.agg(d, func, &parent(&rs), &cs)),
            ShowAs::PercentOfParentColumn if ce.kind == Kind::Grand => Some(Ok(1.0)),
            ShowAs::PercentOfParentColumn => ratio(v, self.agg(d, func, &rs, &parent(&cs))),
            ShowAs::Index => {
                let (Some(Ok(t)), Some(Ok(r)), Some(Ok(c))) = (total(), row_total(), col_total())
                else {
                    return Some(Err("#DIV/0!"));
                };
                if r == 0.0 || c == 0.0 {
                    Some(Err("#DIV/0!"))
                } else {
                    Some(Ok(v * t / (r * c)))
                }
            }
            kind => {
                // The base field: where it is, and the cell's item in it.
                let bf = df.base_field;
                let at = |lv: &[Level]| lv.iter().position(|l| *l == Level::Field(bf));
                let (rows_axis, level) = match (at(self.row_lv), at(self.col_lv)) {
                    (Some(l), _) => (true, l),
                    (None, Some(l)) => (false, l),
                    _ => return Some(Err("#N/A")),
                };
                let (e, own, other) = if rows_axis {
                    (re, &rs, &cs)
                } else {
                    (ce, &cs, &rs)
                };
                let with = |items: Vec<usize>| -> Spec {
                    own.iter()
                        .map(|(f, ks)| (*f, if *f == bf { items.clone() } else { ks.clone() }))
                        .collect()
                };
                let agg_own = |s: &Spec| {
                    if rows_axis {
                        self.agg(d, func, s, other)
                    } else {
                        self.agg(d, func, other, s)
                    }
                };
                if e.kind == Kind::Grand || e.path.len() <= level {
                    return None;
                }
                let k = e.path[level];
                let order = self.order(rows_axis, level);
                let idx = order.iter().position(|x| *x == k).unwrap_or(0);
                match kind {
                    ShowAs::Percent | ShowAs::Difference | ShowAs::PercentDifference => {
                        let base = match df.base_item {
                            BASE_PREVIOUS => idx.checked_sub(1).map(|i| order[i]),
                            BASE_NEXT => order.get(idx + 1).copied(),
                            b => Some(b as usize),
                        };
                        let base = base.unwrap_or(k);
                        if base == k {
                            return match kind {
                                ShowAs::Percent => Some(Ok(1.0)),
                                _ => None,
                            };
                        }
                        let b = agg_own(&with(vec![base]));
                        match kind {
                            ShowAs::Percent => ratio(v, b),
                            ShowAs::Difference => match b {
                                Some(Ok(b)) => Some(Ok(v - b)),
                                None => Some(Ok(v)),
                                e => e,
                            },
                            _ => match b {
                                Some(Ok(b)) if b != 0.0 => Some(Ok((v - b) / b)),
                                Some(Ok(_)) => Some(Err("#DIV/0!")),
                                None => Some(Err("#NULL!")),
                                e => e,
                            },
                        }
                    }
                    ShowAs::RunningTotal | ShowAs::PercentRunningTotal => {
                        let run = agg_own(&with(order[..=idx].to_vec()));
                        if kind == ShowAs::RunningTotal {
                            return run;
                        }
                        match run {
                            Some(Ok(x)) => ratio(x, agg_own(&with(order.clone()))),
                            other => other,
                        }
                    }
                    ShowAs::RankAscending | ShowAs::RankDescending => {
                        let values: Vec<f64> = order
                            .iter()
                            .filter_map(|x| match agg_own(&with(vec![*x])) {
                                Some(Ok(y)) => Some(y),
                                _ => None,
                            })
                            .collect();
                        let better = values
                            .iter()
                            .filter(|y| {
                                if kind == ShowAs::RankAscending {
                                    **y < v
                                } else {
                                    **y > v
                                }
                            })
                            .count();
                        Some(Ok((better + 1) as f64))
                    }
                    ShowAs::PercentOfParent => {
                        if e.path.len() == level + 1 {
                            return Some(Ok(1.0));
                        }
                        let cut: Spec = own
                            .iter()
                            .take_while(|(f, _)| *f != bf)
                            .cloned()
                            .chain(own.iter().find(|(f, _)| *f == bf).cloned())
                            .collect();
                        ratio(v, agg_own(&cut))
                    }
                    _ => raw,
                }
            }
        }
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
            .find(|c| c.database && c.name.eq_ignore_ascii_case(name));
        // A field grouped by ranges lists no items (records hold values);
        // the base of a manual grouping must.
        let ranged = prev.is_some_and(|p| p.range_grouped());
        let discrete_base = old.fields.iter().any(|c| {
            matches!(&c.group, Some(g) if matches!(g.kind, Some(GroupKind::Discrete(_)))
                && g.base.and_then(|b| old.fields.get(b)).is_some_and(|b| b.name.eq_ignore_ascii_case(name)))
        });
        let mut items: Vec<Val> = Vec::new();
        let mut seen: HashMap<String, ()> = HashMap::new();
        let column = data.rows.iter().map(|r| &r[f]);
        let calc_items = prev.map(|p| p.calc_items.clone()).unwrap_or_default();
        let needs_items = discrete_base
            || !calc_items.is_empty()
            || (!ranged
                && (prev.is_some_and(|p| !p.items.is_empty())
                    || axis.iter().any(|a| a.eq_ignore_ascii_case(name))
                    || data
                        .rows
                        .iter()
                        .any(|r| matches!(r[f], Val::Str(_) | Val::Bool(_) | Val::Err(_)))));
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
            // Calculated items after the source's (records never hold them).
            for c in &calc_items {
                let v = Val::Str(c.name.clone());
                if seen.insert(v.key(), ()).is_none() {
                    items.push(v);
                }
            }
        }
        fields.push(CacheField {
            items,
            num_fmt_id: prev.map_or(0, |p| p.num_fmt_id),
            calc_items,
            ..CacheField::source(name.clone())
        });
    }
    crate::pivot_group::regroup(&old.fields, &mut fields, &data.rows);
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
        let new_items = new.fields[j].item_list();
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
                        .item_list()
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
            // Group items keep their order (`<1/1/2026`, `Jan`, `Feb`...).
            let grouped = new.fields[j]
                .group
                .as_ref()
                .is_some_and(|g| g.groups_items());
            let order = |a: usize, b: usize| {
                if grouped {
                    a.cmp(&b)
                } else {
                    let (ka, kb) = (new_items[a].sort_key(), new_items[b].sort_key());
                    ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)
                }
            };
            let calc = |x: usize| new.fields[j].is_calc(x);
            let mut fresh: Vec<usize> = (0..new_items.len()).filter(|x| !used[*x]).collect();
            // Calculated items after the others, in the order made.
            fresh.sort_by(|a, b| calc(*a).cmp(&calc(*b)).then(order(*a, *b)));
            if !fresh.is_empty() {
                reindexed = true;
            }
            // In a pivot table made here, `(blank)` in the interface
            // language is kept as the item's caption: the file has no other
            // place for it, and Excel shows the caption.
            let blank = def.added.then(|| tr("(blank)")).filter(|s| *s != "(blank)");
            kept.extend(fresh.into_iter().map(|x| {
                FieldItem {
                    x: Some(x),
                    hidden: false,
                    t: None,
                    expanded: true,
                    n: blank
                        .filter(|_| new_items[x] == Val::Missing)
                        .map(str::to_owned),
                    calc: false,
                }
            }));
            for it in kept.iter_mut() {
                it.calc = it.x.is_some_and(calc);
            }
            if let Some(dir @ ("ascending" | "descending")) = f.sort.as_deref() {
                {
                    kept.sort_by(|a, b| {
                        let o = order(a.x.unwrap_or(0), b.x.unwrap_or(0));
                        let o = if dir == "descending" { o.reverse() } else { o };
                        a.calc.cmp(&b.calc).then(o)
                    });
                }
            }
            let sub_item = |t: &str| FieldItem {
                x: None,
                hidden: false,
                t: Some(t.into()),
                expanded: true,
                n: None,
                calc: false,
            };
            if !f.subtotals.is_empty() {
                // Custom subtotals: one item each, in Excel's order.
                subtotals = f
                    .subtotals
                    .iter()
                    .map(|func| sub_item(func.subtotal_xml().0))
                    .collect();
            } else if !f.default_subtotal {
                subtotals.retain(|it| it.t.as_deref() != Some("default"));
                subtotals.retain(|it| {
                    !Func::ALL
                        .iter()
                        .any(|func| it.t.as_deref() == Some(func.subtotal_xml().0))
                });
            } else {
                subtotals.retain(|it| it.t.as_deref() == Some("default"));
                if on_axis && subtotals.is_empty() {
                    subtotals.push(sub_item("default"));
                }
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
                let key = old.fields[p.fld].item_list().get(x)?.key();
                fields[j].items.iter().position(|it| {
                    it.x.is_some_and(|nx| new.fields[j].item_list()[nx].key() == key)
                })
            });
            Some(PageField {
                fld: j,
                item,
                xml: None,
            })
        })
        .collect();
    let mut data_map: Vec<Option<usize>> = Vec::with_capacity(def.data_fields.len());
    out.data_fields = Vec::new();
    for d in &def.data_fields {
        let Some(j) = map.get(d.fld).copied().flatten() else {
            data_map.push(None);
            continue;
        };
        data_map.push(Some(out.data_fields.len()));
        out.data_fields.push(DataField {
            fld: j,
            base_field: map.get(d.base_field).copied().flatten().unwrap_or(0),
            xml: None,
            ..d.clone()
        });
    }
    for f in fields.iter_mut() {
        f.sort_by = f.sort_by.and_then(|d| data_map.get(d).copied().flatten());
    }
    out.filters = def
        .filters
        .iter()
        .filter_map(|flt| {
            let fld = map.get(flt.fld).copied().flatten()?;
            let measure = match flt.measure {
                Some(m) => Some(data_map.get(m).copied().flatten()?),
                None => None,
            };
            Some(PivotFilter {
                fld,
                measure,
                ..flt.clone()
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
/// besides what the model has (axis, data flag, layout flags and items).
fn field_xml(f: &Field) -> String {
    /// Attributes written from the model, with their defaults.
    const MODELLED: [&str; 21] = [
        "axis",
        "dataField",
        "compact",
        "outline",
        "subtotalTop",
        "defaultSubtotal",
        "showAll",
        "multipleItemSelectionAllowed",
        "sortType",
        "insertBlankRow",
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
    ];
    let mut attrs = String::new();
    let mut rest = String::new();
    let mut had: Vec<&str> = Vec::new();
    let original = f.xml.as_deref().and_then(|x| {
        let wrapped = format!("<w xmlns=\"{NS_MAIN}\" xmlns:r=\"{NS_R}\">{x}</w>");
        let doc = Document::parse(&wrapped).ok()?;
        let el = doc.root_element().first_element_child()?;
        let mut a = String::new();
        let mut seen = Vec::new();
        for at in el.attributes() {
            if at.namespace().is_some() {
                continue;
            }
            if let Some(m) = MODELLED.iter().find(|m| **m == at.name()) {
                seen.push(*m);
                continue;
            }
            a.push_str(&format!(" {}=\"{}\"", at.name(), esc(at.value())));
        }
        // Other extensions than the x14 one (written from the model).
        let mut r = String::new();
        for c in el.children().filter(|c| c.is_element()) {
            if c.tag_name().name() == "extLst" {
                for e in c.children().filter(|e| e.is_element()) {
                    let x14 = e.attribute("uri") == Some(URI_X14_PIVOT_FIELD);
                    if !x14 {
                        r.push_str(&wrapped[e.range()]);
                    }
                }
            }
        }
        Some((a, r, seen))
    });
    if let Some((a, r, seen)) = original {
        attrs = a;
        rest = r;
        had = seen;
    }
    let mut out = String::from("<pivotField");
    if let Some(axis) = &f.axis {
        out.push_str(&format!(" axis=\"{axis}\""));
    }
    if f.data {
        out.push_str(" dataField=\"1\"");
    }
    let multi = had.contains(&"multipleItemSelectionAllowed")
        || (f.axis.as_deref() == Some("axisPage") && f.items.iter().any(|i| i.hidden));
    let custom = !f.subtotals.is_empty();
    for (name, value, default) in [
        ("compact", f.compact, true),
        ("outline", f.outline, true),
        ("subtotalTop", f.subtotal_top, true),
        ("showAll", f.show_all, true),
        ("insertBlankRow", f.insert_blank, false),
        ("defaultSubtotal", f.default_subtotal && !custom, true),
        ("multipleItemSelectionAllowed", multi, false),
    ] {
        if value != default || had.contains(&name) {
            out.push_str(&format!(" {name}=\"{}\"", u8::from(value)));
        }
    }
    if let Some(dir @ ("ascending" | "descending")) = f.sort.as_deref() {
        out.push_str(&format!(" sortType=\"{dir}\""));
    }
    for func in &f.subtotals {
        out.push_str(&format!(" {}=\"1\"", func.subtotal_xml().1));
    }
    out.push_str(&attrs);
    let mut ext = rest;
    if f.fill_down {
        ext.push_str(&format!(
            "<ext uri=\"{URI_X14_PIVOT_FIELD}\" xmlns:x14=\"{NS_X14}\"><x14:pivotField fillDownLabels=\"1\"/></ext>"
        ));
    }
    let sort_scope = f.sort_by.map(|d| {
        format!(
            "<autoSortScope><pivotArea dataOnly=\"0\" outline=\"0\" fieldPosition=\"0\"><references count=\"1\"><reference field=\"4294967294\" count=\"1\" selected=\"0\"><x v=\"{d}\"/></reference></references></pivotArea></autoSortScope>"
        )
    });
    if f.items.is_empty() && ext.is_empty() && sort_scope.is_none() {
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
            if it.calc {
                out.push_str(" f=\"1\"");
            }
            if let Some(x) = it.x {
                out.push_str(&format!(" x=\"{x}\""));
            }
            out.push_str("/>");
        }
        out.push_str("</items>");
    }
    if let Some(s) = sort_scope {
        out.push_str(&s);
    }
    if !ext.is_empty() {
        out.push_str(&format!("<extLst>{ext}</extLst>"));
    }
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
    let mut sets: Vec<(&str, String)> =
        vec![("name", def.name.clone()), ("updatedVersion", "6".into())];
    sets.extend(def.root_sets.iter().map(|(k, v)| (*k, v.clone())));
    let start = root_start_with(xml, &sets)?;
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
                out.push_str(&format!(" subtotal=\"{}\"", d.func.name()));
            }
            let (show, x14) = d.show_as.xml();
            if let Some(show) = show {
                out.push_str(&format!(" showDataAs=\"{show}\""));
            }
            out.push_str(&format!(
                " baseField=\"{}\" baseItem=\"{}\"",
                d.base_field, d.base_item
            ));
            if let Some(n) = d.num_fmt_id {
                out.push_str(&format!(" numFmtId=\"{n}\""));
            }
            match x14 {
                Some(kind) => out.push_str(&format!(
                    "><extLst><ext uri=\"{URI_X14_DATA_FIELD}\" xmlns:x14=\"{NS_X14}\"><x14:dataField pivotShowAs=\"{kind}\"/></ext></extLst></dataField>"
                )),
                None => out.push_str("/>"),
            }
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
    // The style and the filters are written from the model, where the
    // schema puts them (after `pivotHierarchies`, before the hierarchy
    // usages and `extLst`).
    const AFTER: [&str; 3] = ["rowHierarchiesUsage", "colHierarchiesUsage", "extLst"];
    let mut tail_done = false;
    let had_style = child(root, "pivotTableStyleInfo").is_some();
    let tail = |out: &mut String| {
        if had_style || def.style.name.is_some() {
            out.push_str(&style_info_xml(&def.style));
        }
        if !def.filters.is_empty() {
            out.push_str(&format!("<filters count=\"{}\">", def.filters.len()));
            for (k, f) in def.filters.iter().enumerate() {
                out.push_str(&f.xml(k + 1));
            }
            out.push_str("</filters>");
        }
    };
    for c in root.children().filter(|c| c.is_element()) {
        let n = c.tag_name().name();
        if REWRITTEN.contains(&n)
            || (def.reindexed && BY_INDEX.contains(&n))
            || n == "pivotTableStyleInfo"
            || n == "filters"
        {
            continue;
        }
        if !tail_done && AFTER.contains(&n) {
            tail(&mut out);
            tail_done = true;
        }
        out.push_str(&xml[c.range()]);
    }
    if !tail_done {
        tail(&mut out);
    }
    out.push_str(&format!("</{name}>"));
    Some(out)
}

/// `<pivotTableStyleInfo>` for `s`.
fn style_info_xml(s: &StyleInfo) -> String {
    let mut out = String::from("<pivotTableStyleInfo");
    if let Some(n) = &s.name {
        out.push_str(&format!(" name=\"{}\"", esc(n)));
    }
    out.push_str(&format!(
        " showRowHeaders=\"{}\" showColHeaders=\"{}\" showRowStripes=\"{}\" showColStripes=\"{}\" showLastColumn=\"{}\"/>",
        u8::from(s.row_headers),
        u8::from(s.col_headers),
        u8::from(s.row_stripes),
        u8::from(s.col_stripes),
        u8::from(s.last_column)
    ));
    out
}

/// A definition as read with its row and column items as the file has
/// them (for a copy of a pivot table not laid out in Kantor).
pub(crate) fn laid_of(def: &Def) -> LaidXml {
    let inner = |name: &str| -> String {
        let Ok(doc) = Document::parse(&def.xml) else {
            return String::new();
        };
        child(doc.root_element(), name)
            .map(|n| {
                n.children()
                    .filter(|c| c.is_element())
                    .map(|c| def.xml[c.range()].to_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    LaidXml {
        def: def.clone(),
        row_items_xml: inner("rowItems"),
        col_items_xml: inner("colItems"),
    }
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
    // `calculatedItems` goes before these (CT_PivotCacheDefinition).
    const CALC_AFTER: [&str; 5] = [
        "calculatedMembers",
        "dimensions",
        "measureGroups",
        "maps",
        "extLst",
    ];
    let mut calc_done = false;
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
                        "<cacheField name=\"{}\" numFmtId=\"{}\"",
                        esc(&cf.name),
                        cf.num_fmt_id
                    ));
                    if let Some(formula) = &cf.formula {
                        out.push_str(&format!(" formula=\"{}\"", esc(formula)));
                    }
                    if !cf.database {
                        out.push_str(" databaseField=\"0\"");
                    }
                    if cf.database {
                        out.push('>');
                        shared_items_xml(cf, records.iter().filter_map(|r| r.get(f)), &mut out);
                    } else if cf.group.is_none() {
                        out.push_str("/>");
                        continue;
                    } else {
                        out.push('>');
                    }
                    if let Some(g) = &cf.group {
                        g.xml(&mut out);
                    }
                    out.push_str("</cacheField>");
                }
                out.push_str("</cacheFields>");
            }
            "calculatedItems" => {}
            n => {
                if !calc_done && CALC_AFTER.contains(&n) {
                    calc_items_xml(cache, &mut out);
                    calc_done = true;
                }
                out.push_str(&xml[c.range()]);
            }
        }
    }
    if !calc_done {
        calc_items_xml(cache, &mut out);
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

/// `<calculatedItems>` of a cache with calculated items: each one a formula
/// and the shared item it fills (`cacheIndex`: `x` is a shared item index).
fn calc_items_xml(cache: &Cache, out: &mut String) {
    let mut list = Vec::new();
    for (f, cf) in cache.fields.iter().enumerate() {
        for c in &cf.calc_items {
            let key = Val::Str(c.name.clone()).key();
            if let Some(x) = cf.items.iter().position(|v| v.key() == key) {
                list.push(format!(
                    "<calculatedItem formula=\"{}\"><pivotArea cacheIndex=\"1\" outline=\"0\" fieldPosition=\"0\"><references count=\"1\"><reference field=\"{f}\" count=\"1\" selected=\"0\"><x v=\"{x}\"/></reference></references></pivotArea></calculatedItem>",
                    esc(&c.formula)
                ));
            }
        }
    }
    if !list.is_empty() {
        out.push_str(&format!(
            "<calculatedItems count=\"{}\">{}</calculatedItems>",
            list.len(),
            list.concat()
        ));
    }
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
    // Calculated items' names are text items.
    s |= !cf.calc_items.is_empty();
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
    pub(crate) fn pivot_state(&self) -> Arc<State> {
        self.history.pivot_state()
    }

    /// The definition of pivot `i` now.
    pub(crate) fn pivot_def(&self, state: &State, i: usize) -> Arc<Def> {
        match state.defs.get(&i) {
            Some(l) => Arc::new(l.def.clone()),
            None => self.pivots.defs[i].clone(),
        }
    }

    /// Whether pivot `i` is in the workbook in `state` (not deleted; made
    /// in Kantor and not undone).
    pub(crate) fn pivot_present(&self, state: &State, i: usize) -> bool {
        !state.gone.contains(&i) && (!self.pivots.defs[i].added || state.defs.contains_key(&i))
    }

    pub(crate) fn pivot_cache(&self, state: &State, i: usize) -> Arc<Cache> {
        state
            .caches
            .get(&i)
            .cloned()
            .unwrap_or_else(|| self.pivots.caches[i].clone())
    }

    /// Where `area` (in the coordinates of `at`) is now on sheet `sheet_id`.
    pub(crate) fn now(&self, at: &Layout, sheet_id: u32, area: Area) -> Option<Area> {
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
        let mut out = Vec::new();
        for i in 0..self.pivots.defs.len() {
            if !self.pivot_present(&state, i) {
                continue;
            }
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
                .and_then(|n| self.pivot_style_def(n))
                .map(Arc::new);
            out.push(Placed::new(i, def, area, style));
        }
        let rc = Arc::new(out);
        self.cached(|c| c.pivots.insert(sheet, rc.clone()));
        rc
    }

    pub(crate) fn pivot_info(&self, p: &Placed) -> PivotInfo {
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

    /// How built-in style `name` draws a small pivot table with `options`
    /// (a header row, two outer items with two inner ones each and a grand
    /// total row; a label column, two value columns and a grand total
    /// column), for a gallery.
    pub fn pivot_style_preview(
        &self,
        name: &str,
        options: &crate::PivotStyleOptions,
    ) -> Option<Vec<Vec<TableCellLook>>> {
        let s = self.pivot_style_def(name)?;
        Some(preview_of(&s, name, options))
    }

    /// How a PivotTable style being made (New / Modify PivotTable Style)
    /// draws the preview pivot table, with every option on.
    pub fn pivot_style_spec_preview(
        &self,
        spec: &crate::TableStyleSpec,
    ) -> Vec<Vec<TableCellLook>> {
        let s = PivotStyleDef::from_table_def(&spec.def());
        let options = crate::PivotStyleOptions {
            name: None,
            row_headers: true,
            column_headers: true,
            banded_rows: true,
            banded_columns: false,
        };
        preview_of(&s, &spec.name, &options)
    }

    /// PivotTable style `name`: the workbook's own, else a built-in one.
    pub(crate) fn pivot_style_def(&self, name: &str) -> Option<PivotStyleDef> {
        match self.custom_pivot_style(name) {
            Some(d) => Some(PivotStyleDef::from_table_def(&d)),
            None => builtin(name, &self.model.get_model().workbook.theme),
        }
    }

    /// Pivot tables styled `old` get style `new` (no style for `None`), in
    /// the current history state (part of the caller's undo step).
    pub(crate) fn restyle_pivots(&mut self, old: &str, new: Option<&str>) -> Result<()> {
        let state = self.pivot_state();
        let users: Vec<usize> = (0..self.pivots.defs.len())
            .filter(|&i| self.pivot_present(&state, i))
            .filter(|&i| {
                self.pivot_def(&state, i)
                    .style
                    .name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(old))
            })
            .collect();
        if users.is_empty() {
            return Ok(());
        }
        self.keep_pivot_records()
            .map_err(|e| anyhow!("can't restyle the PivotTables: {e}"))?;
        let mut next = (*state).clone();
        for i in users {
            let mut laid = match state.defs.get(&i) {
                Some(l) => (**l).clone(),
                None => laid_of(&self.pivots.defs[i]),
            };
            laid.def.style.name = new.map(str::to_owned);
            next.defs.insert(i, Arc::new(laid));
        }
        self.history.set_pivot_state(Arc::new(next));
        self.cache.borrow_mut().pivots.clear();
        Ok(())
    }
}

/// The preview pivot table of the style gallery drawn in `s`: a header
/// row, two outer items with two inner ones each and a grand total row; a
/// label column, two value columns and a grand total column.
fn preview_of(
    s: &PivotStyleDef,
    name: &str,
    options: &crate::PivotStyleOptions,
) -> Vec<Vec<TableCellLook>> {
    {
        let style = StyleInfo {
            name: Some(name.to_owned()),
            row_headers: options.row_headers,
            col_headers: options.column_headers,
            row_stripes: options.banded_rows,
            col_stripes: options.banded_columns,
            last_column: true,
        };
        let rows = [
            Line::Subheading(0),
            Line::Body,
            Line::Body,
            Line::Subheading(0),
            Line::Body,
            Line::Body,
            Line::Grand,
        ];
        let cols = [Line::Body, Line::Body, Line::Grand];
        let frame = Frame {
            style: &style,
            area: Area {
                top: 0,
                left: 0,
                bottom: 7,
                right: 3,
            },
            page: None,
            first_data_row: 1,
            first_data_col: 1,
            rows: &rows,
            cols: &cols,
        };
        (0..8)
            .map(|r| (0..4).map(|c| look_of(s, &frame, r, c)).collect())
            .collect()
    }
}

impl Workbook {
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
    pub(crate) fn read_source(&self, cache: &Cache) -> Result<(u32, Area, SourceData)> {
        let model = self.model.get_model();
        // A sheet renamed since the file was opened is found by its name then.
        let sheet_index = |name: &str| -> Result<u32> {
            self.sheet_index_by_name(name)
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
        self.rebuild_pivots(caches, Rebuild::default())
            .map(|n| n.len())
    }

    /// Keep the records parts of the opened file's caches: a save after a
    /// pivot edit writes every pivot part for the current state, and those
    /// not refreshed need their records as the file had them.
    pub(crate) fn keep_pivot_records(&mut self) -> Result<()> {
        if self.pivots.refreshed {
            return Ok(());
        }
        let needed: Vec<String> = self
            .pivots
            .caches
            .iter()
            .filter(|c| !c.added)
            .filter_map(|c| c.records_part.clone())
            .collect();
        let mut kept = HashMap::new();
        if !needed.is_empty() {
            let bytes = self
                .origin
                .xlsx
                .as_ref()
                .ok_or_else(|| anyhow!("the opened file is not at hand"))?
                .bytes()
                .map_err(|e| anyhow!("{e}"))?;
            for rp in needed {
                let data = crate::preserve::read_part(&bytes, &rp).map_err(|e| anyhow!("{e}"))?;
                kept.insert(rp, Arc::new(data));
            }
        }
        self.pivots.records = kept;
        self.pivots.refreshed = true;
        Ok(())
    }

    /// Rebuild `caches` from their sources and lay out again the pivot
    /// tables on them, with `change` applied, as one undo step. Returns the
    /// pivot indexes laid out.
    pub(crate) fn rebuild_pivots(
        &mut self,
        caches: &[usize],
        change: Rebuild,
    ) -> Result<Vec<usize>> {
        let state = self.pivot_state();
        let layout = self.history.layout();
        let def_of = |i: usize| -> Arc<Def> {
            match change.defs.get(&i) {
                Some(d) => Arc::new(d.clone()),
                None => self.pivot_def(&state, i),
            }
        };
        let sheet_of = |sheet_id: u32| -> Option<u32> {
            let model = self.model.get_model();
            match model
                .workbook
                .worksheets
                .iter()
                .position(|w| w.sheet_id == sheet_id)
            {
                Some(s) => Some(s as u32),
                None if change.new_sheet == Some(sheet_id) => {
                    Some(model.workbook.worksheets.len() as u32)
                }
                None => None,
            }
        };
        // Work everything out first: nothing changes if any of it fails.
        struct Plan {
            index: usize,
            sheet: u32,
            old: Option<Area>,
            laid: Laid,
        }
        let mut plans: Vec<Plan> = Vec::new();
        let mut new_caches: Vec<(usize, Arc<Cache>)> = Vec::new();
        for &ci in caches {
            let users: Vec<usize> = (0..self.pivots.defs.len())
                .filter(|&i| {
                    (self.pivot_present(&state, i) || change.defs.contains_key(&i))
                        && def_of(i).cache == ci
                })
                .collect();
            if users.is_empty() {
                continue;
            }
            let old = self.pivot_cache(&state, ci);
            let first = def_of(users[0]);
            if let Some(why) = &old.unsupported {
                bail!("can't refresh the PivotTable {}: {why}", first.name);
            }
            for &i in &users {
                let d = def_of(i);
                if let Some(why) = &d.unsupported {
                    bail!("can't refresh the PivotTable {}: {why}", d.name);
                }
            }
            let (_, src_area, data) = self
                .read_source(&old)
                .map_err(|e| anyhow!("can't refresh the PivotTable {}: {e}", first.name))?;
            let axis: Vec<String> = users
                .iter()
                .flat_map(|&i| {
                    let d = def_of(i);
                    d.fields
                        .iter()
                        .enumerate()
                        .filter(|(_, f)| f.axis.is_some())
                        .filter_map(|(k, _)| old.fields.get(k).map(|c| c.name.clone()))
                        .collect::<Vec<_>>()
                })
                .chain(change.axis.iter().cloned())
                .collect();
            // Fields added, grouped or removed first (by name: the
            // definitions are matched to the rebuilt cache by name).
            let mut base = (*old).clone();
            if let Some(cc) = &change.cache_change {
                cc(&mut base)?;
            }
            let mut cache = rebuild_cache(&base, data, &axis);
            cache.at = layout.clone();
            if let Some(Source::Range { sheet, .. }) = &cache.source {
                cache.source = Some(Source::Range {
                    sheet: sheet.clone(),
                    area: src_area,
                });
            }
            let records = cache.records.clone().unwrap_or_default();
            for &i in &users {
                let d = def_of(i);
                let sheet = sheet_of(d.sheet_id)
                    .ok_or_else(|| anyhow!("the PivotTable {}'s sheet is gone", d.name))?;
                let placed = self.placed_pivots(sheet);
                let found = placed.iter().find(|p| p.index == i);
                let (old_bounds, top, left) = match found {
                    Some(p) => (Some(p.bounds()), p.area.top, p.area.left),
                    None if change.defs.contains_key(&i) => (None, d.location.top, d.location.left),
                    None => bail!("the PivotTable {}'s cells were deleted", d.name),
                };
                let mut matched = rematch(&d, &old, &cache);
                if let Some(adjust) = &change.adjust {
                    adjust(&mut matched, &cache);
                    // Items for fields the change put on an axis.
                    matched = rematch(&matched, &cache, &cache);
                }
                // Room above for the report filters.
                let pages = matched.page_fields.len() as u32;
                let top = if pages > 0 { top.max(pages + 1) } else { top };
                let mut laid = lay_out_any(&matched, &cache, &records, top, left);
                laid.def.at = layout.clone();
                plans.push(Plan {
                    index: i,
                    sheet,
                    old: old_bounds,
                    laid,
                });
            }
            new_caches.push((ci, Arc::new(cache)));
        }
        if plans.is_empty() {
            return Ok(Vec::new());
        }
        self.keep_pivot_records()
            .map_err(|e| anyhow!("can't change the PivotTables: {e}"))?;
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
            if change.new_sheet == Some(plan.laid.def.sheet_id) {
                continue;
            }
            let overlaps = |o: &Area| {
                o.top <= bounds.bottom
                    && bounds.top <= o.bottom
                    && o.left <= bounds.right
                    && bounds.left <= o.right
            };
            for other in self.placed_pivots(plan.sheet).iter() {
                if other.index == plan.index || plans.iter().any(|p| p.index == other.index) {
                    continue;
                }
                if overlaps(&other.bounds()) {
                    bail!(
                        "the PivotTable {} would overlap the PivotTable {}",
                        plan.laid.def.name,
                        other.def.name
                    );
                }
            }
            for t in self.tables(plan.sheet) {
                let a = Area {
                    top: t.area.top,
                    left: t.area.left,
                    bottom: t.area.bottom,
                    right: t.area.right,
                };
                if overlaps(&a) {
                    bail!(
                        "the PivotTable {} would overlap the table {}",
                        plan.laid.def.name,
                        t.name
                    );
                }
            }
            let old = plan.old;
            for &(r, c) in &self.filled_cells(plan.sheet) {
                let inside = |a: &Area| r >= a.top && r <= a.bottom && c >= a.left && c <= a.right;
                if inside(&bounds) && !old.as_ref().is_some_and(inside) {
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
        if let Some(id) = change.new_sheet {
            match self.model.new_sheet() {
                Ok(()) => {
                    steps += 1;
                    let model = self.model.get_model();
                    let made = model.workbook.worksheets.last().map(|w| w.sheet_id);
                    if made != Some(id) {
                        // Not the id planned: follow the sheet.
                        if let Some(made) = made {
                            for plan in &mut plans {
                                if plan.laid.def.sheet_id == id {
                                    plan.laid.def.sheet_id = made;
                                }
                            }
                        }
                    }
                }
                Err(e) => result = Err(anyhow!("{e}")),
            }
        }
        let num_fmts = self.model.get_model().workbook.styles.num_fmts.clone();
        'plans: for plan in &plans {
            if result.is_err() {
                break;
            }
            if let Some(old) = plan.old {
                if let Err(e) = self.model.range_clear_all(&range_of(old).area(plan.sheet)) {
                    result = Err(anyhow!("{e}"));
                    break;
                }
                steps += 1;
            }
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
        if steps == 0 && change.defs.is_empty() {
            return result.map(|()| Vec::new());
        }
        if steps == 0 {
            // Nothing to write (an empty new pivot): a step of our own.
            self.history.edited_steps(0);
        } else {
            self.history.edited_steps(steps);
        }
        if result.is_ok() {
            let mut next = (*state).clone();
            for (ci, c) in new_caches {
                next.caches.insert(ci, c);
            }
            for plan in &plans {
                next.gone.remove(&plan.index);
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
        result.map(|()| plans.iter().map(|p| p.index).collect())
    }

    /// The pivot parts to save for the current state (see
    /// `preserve::Context::pivots`).
    pub(crate) fn pivot_save(&self) -> crate::preserve::PivotSave {
        let mut out = crate::preserve::PivotSave::default();
        if !self.pivots.refreshed {
            return out;
        }
        let state = self.pivot_state();
        // The definition and records of cache `i`, placed where they are now.
        let cache_parts = |i: usize| -> Option<(String, Option<String>)> {
            let orig = &self.pivots.caches[i];
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
                let (def, rec) = cache_xml(cache, source)?;
                return Some((def, Some(rec)));
            }
            let def = match source {
                Some(a) => with_attr(&cache.xml, "worksheetSource", "ref", &range_of(a).name())?,
                None => cache.xml.to_string(),
            };
            let rec = orig
                .records_part
                .as_ref()
                .and_then(|rp| self.pivots.records.get(rp))
                .map(|r| String::from_utf8_lossy(r).into_owned());
            Some((def, rec))
        };
        for (i, base) in self.pivots.defs.iter().enumerate() {
            if !self.pivot_present(&state, i) {
                if base.in_origin {
                    out.dropped.insert(base.part.to_lowercase());
                }
                continue;
            }
            let xml = match state.defs.get(&i) {
                Some(laid) => {
                    let d = &laid.def;
                    self.now(&d.at, d.sheet_id, d.location)
                        .and_then(|loc| def_xml(laid, loc))
                }
                None => self
                    .now(&base.at, base.sheet_id, base.location)
                    .and_then(|loc| with_attr(&base.xml, "location", "ref", &range_of(loc).name())),
            };
            let Some(xml) = xml else { continue };
            if base.in_origin {
                out.replaced.insert(base.part.clone(), xml.into_bytes());
                continue;
            }
            let def = self.pivot_def(&state, i);
            let Some((cache_xml, records_xml)) = cache_parts(def.cache) else {
                continue;
            };
            let cache = &self.pivots.caches[def.cache];
            out.added.push(crate::preserve::NewPivot {
                sheet_id: def.sheet_id,
                table_part: base.part.clone(),
                table_xml: xml,
                cache_part: cache.part.clone(),
                cache_xml,
                records_part: cache.records_part.clone(),
                records_xml,
            });
        }
        for (i, orig) in self.pivots.caches.iter().enumerate() {
            if orig.added {
                continue;
            }
            let Some((def, rec)) = cache_parts(i) else {
                continue;
            };
            out.replaced.insert(orig.part.clone(), def.into_bytes());
            if let (Some(rp), Some(rec)) = (&orig.records_part, rec) {
                out.replaced.insert(rp.clone(), rec.into_bytes());
            }
        }
        self.slicer_saves(&mut out.replaced);
        out
    }

    /// After a save: new pivot parts got their names, and each pivot table
    /// is in the saved package or not.
    pub(crate) fn pivots_saved(&mut self, saved: &crate::preserve::PivotsSaved) {
        for d in self.pivots.defs.iter_mut() {
            let d = Arc::make_mut(d);
            if let Some(a) = saved.added.iter().find(|a| a.proposed_table == d.part) {
                d.part = a.table.clone();
            }
            d.in_origin = saved.tables.contains(&d.part.to_lowercase());
        }
        for c in self.pivots.caches.iter_mut() {
            if !c.part.starts_with(NEW_PREFIX) {
                continue;
            }
            if let Some(a) = saved.added.iter().find(|a| a.proposed_cache == c.part) {
                let c = Arc::make_mut(c);
                c.part = a.cache.clone();
                c.records_part = a.records.clone();
                // In the package now: the next save rewrites it in place.
                c.added = false;
            }
        }
    }
}

/// Part names of pivot parts made in Kantor until a save gives them their
/// names in the package.
pub(crate) const NEW_PREFIX: &str = "kantor-new/";

/// What [`Workbook::rebuild_pivots`] changes besides refreshing.
#[derive(Default)]
pub(crate) struct Rebuild {
    /// Definitions to use instead of the current ones (new ones too, at
    /// their `location`'s top-left), for the cache they had.
    pub defs: HashMap<usize, Def>,
    /// Field names to give shared items (put on an axis).
    pub axis: Vec<String>,
    /// Applied to each definition once matched to the rebuilt cache.
    #[allow(clippy::type_complexity)]
    pub adjust: Option<Box<dyn Fn(&mut Def, &Cache)>>,
    /// A sheet to add first, with this `sheetId` (a new pivot goes there).
    pub new_sheet: Option<u32>,
    /// Applied to the cache before it is rebuilt (calculated fields,
    /// grouping).
    #[allow(clippy::type_complexity)]
    pub cache_change: Option<Box<dyn Fn(&mut Cache) -> Result<()>>>,
}

/// [`lay_out`], with Excel's placeholder for a pivot with no fields yet:
/// an empty 3 × 18 area.
pub(crate) fn lay_out_any(
    def: &Def,
    cache: &Cache,
    records: &[Vec<Val>],
    top: u32,
    left: u32,
) -> Laid {
    let mut laid = lay_out(def, cache, records, top, left);
    if def.row_fields.is_empty() && def.col_fields.is_empty() && def.data_fields.is_empty() {
        laid.cells.retain(|(r, _, _)| *r < top);
        laid.num_fmts.clear();
        laid.def.location = Area {
            top,
            left,
            bottom: top + 17,
            right: left + 2,
        };
        laid.def.first_header_row = 1;
        laid.def.first_data_row = 1;
        laid.def.first_data_col = 0;
        let item = AxisItem {
            t: "data".into(),
            r: 0,
            i: 0,
            x: Vec::new(),
        };
        laid.def.row_items = vec![item.clone()];
        laid.def.col_items = vec![item];
        laid.row_items_xml = "<i/>".into();
        laid.col_items_xml = "<i/>".into();
    }
    laid
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
    fn captions_from_the_file_label_subtotals_and_empty_cells() {
        let (c, recs) = cache();
        let l = lay_out(
            &def(r#" subtotalTop="0" subtotalCaption="Total ?""#, ""),
            &c,
            &recs,
            0,
            0,
        );
        let t = table(&l);
        assert_eq!(t[4], ["Total East", "3"]);
        assert_eq!(t[7], ["Total West", "4"]);
        // Product across: West has no Pears.
        let xml = |root: &str| {
            format!(
                r#"<pivotTableDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" name="P" cacheId="1" dataCaption="Values"{root}><location ref="A1:D5" firstHeaderRow="1" firstDataRow="2" firstDataCol="1"/><pivotFields count="3"><pivotField axis="axisRow" showAll="0"><items count="3"><item x="0"/><item x="1"/><item t="default"/></items></pivotField><pivotField axis="axisCol" showAll="0"><items count="3"><item x="0"/><item x="1"/><item t="default"/></items></pivotField><pivotField dataField="1" showAll="0"/></pivotFields><rowFields count="1"><field x="0"/></rowFields><colFields count="1"><field x="1"/></colFields><dataFields count="1"><dataField name="Sum of Units" fld="2"/></dataFields></pivotTableDefinition>"#
            )
        };
        let west_pears = |root: &str| {
            let d = parse_def("p.xml", 1, 0, &xml(root)).unwrap();
            let t = table(&lay_out(&d, &c, &recs, 0, 0));
            assert_eq!(t[3][0], "West");
            t[3][2].clone()
        };
        assert_eq!(west_pears(""), "");
        assert_eq!(west_pears(r#" missingCaption="-""#), "'-");
        assert_eq!(west_pears(r#" missingCaption="-" showMissing="0""#), "");
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
