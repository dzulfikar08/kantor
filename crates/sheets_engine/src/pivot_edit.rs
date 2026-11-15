//! Making, rearranging and deleting pivot tables (Excel's Insert PivotTable
//! and its PivotTable Fields list).
//!
//! A new pivot table is a definition and a cache like the ones a file
//! brings ([`crate::pivot`]), marked as added: it exists only in the history
//! states that list it, so making it is one undo step. Each change of its
//! fields builds a definition and refreshes it from its source with the
//! same engine as Refresh (`Workbook::rebuild_pivots`), one undo step each.
//! A save writes the parts of new pivot tables (`preserve::add_pivots`);
//! from then on they are carried like any other.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::content_text::tr;
use crate::pivot::{
    Cache, CacheField, DataField, Def, Field, Func, NEW_PREFIX, PageField, Rebuild, Source,
    StyleInfo, Val,
};
use crate::pivot_calc::{BASE_NEXT, BASE_PREVIOUS, PivotFilter, ShowAs, filter_family};
use crate::pivot_group::{By, DateLevel, FieldGroup, GroupKind};
use crate::shift::Area;
use crate::{CellRange, Workbook, cell_name};

/// How a value field summarises its values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PivotFunc {
    Sum,
    Count,
    Average,
    Max,
    Min,
    Product,
    CountNumbers,
    StdDev,
    StdDevp,
    Var,
    Varp,
}

impl PivotFunc {
    pub const ALL: [PivotFunc; 11] = [
        PivotFunc::Sum,
        PivotFunc::Count,
        PivotFunc::Average,
        PivotFunc::Max,
        PivotFunc::Min,
        PivotFunc::Product,
        PivotFunc::CountNumbers,
        PivotFunc::StdDev,
        PivotFunc::StdDevp,
        PivotFunc::Var,
        PivotFunc::Varp,
    ];

    /// What Excel's Value Field Settings calls it.
    pub fn label(self) -> &'static str {
        match self {
            PivotFunc::Sum => "Sum",
            PivotFunc::Count => "Count",
            PivotFunc::Average => "Average",
            PivotFunc::Max => "Max",
            PivotFunc::Min => "Min",
            PivotFunc::Product => "Product",
            PivotFunc::CountNumbers => "Count Numbers",
            PivotFunc::StdDev => "StdDev",
            PivotFunc::StdDevp => "StdDevp",
            PivotFunc::Var => "Var",
            PivotFunc::Varp => "Varp",
        }
    }

    pub(crate) fn func(self) -> Func {
        match self {
            PivotFunc::Sum => Func::Sum,
            PivotFunc::Count => Func::Count,
            PivotFunc::Average => Func::Average,
            PivotFunc::Max => Func::Max,
            PivotFunc::Min => Func::Min,
            PivotFunc::Product => Func::Product,
            PivotFunc::CountNumbers => Func::CountNums,
            PivotFunc::StdDev => Func::StdDev,
            PivotFunc::StdDevp => Func::StdDevp,
            PivotFunc::Var => Func::Var,
            PivotFunc::Varp => Func::Varp,
        }
    }

    pub(crate) fn of(f: Func) -> Self {
        match f {
            Func::Sum => PivotFunc::Sum,
            Func::Count => PivotFunc::Count,
            Func::Average => PivotFunc::Average,
            Func::Max => PivotFunc::Max,
            Func::Min => PivotFunc::Min,
            Func::Product => PivotFunc::Product,
            Func::CountNums => PivotFunc::CountNumbers,
            Func::StdDev => PivotFunc::StdDev,
            Func::StdDevp => PivotFunc::StdDevp,
            Func::Var => PivotFunc::Var,
            Func::Varp => PivotFunc::Varp,
        }
    }
}

/// A row or column area entry: a source field, or the `Σ Values` field
/// (there when two value fields or more are).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AxisEntry {
    Field(usize),
    Values,
}

/// A value field: a source field summarised.
#[derive(Debug, Clone, PartialEq)]
pub struct PivotValueField {
    pub field: usize,
    pub func: PivotFunc,
    /// A custom name; `None` gives Excel's `Sum of Units`.
    pub name: Option<String>,
    /// Number format code of the value cells; `None` for General.
    pub num_fmt: Option<String>,
    /// Show Values As.
    pub show_as: ShowAs,
    /// Its base field (for `% Of`, `Difference From`, running totals and
    /// ranks).
    pub base_field: Option<usize>,
    pub base_item: PivotBaseItem,
}

/// The base item of `% Of` / `Difference From` / `% Difference From`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PivotBaseItem {
    #[default]
    Previous,
    Next,
    /// An item, by its label.
    Item(String),
}

/// A row or column field's subtotals (Field Settings).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FieldSubtotals {
    #[default]
    Automatic,
    None,
    /// The functions picked, in Excel's order.
    Custom(Vec<PivotFunc>),
}

/// How a field's items are sorted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum PivotSort {
    /// In data source order (or as dragged).
    #[default]
    Manual,
    Ascending,
    Descending,
}

/// A label, value (Top 10, greater than, between...) or date filter.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PivotFieldFilter {
    /// Excel's filter type (`captionBeginsWith`, `valueGreaterThan`,
    /// `count`, `thisMonth`...): see [`crate::PIVOT_FILTER_TYPES`].
    pub kind: String,
    /// The value field (by position in [`PivotLayout::values`]) a value
    /// or Top 10 filter tests.
    pub value_field: Option<usize>,
    pub value1: String,
    pub value2: String,
    /// Top 10: Top (else Bottom).
    pub top: bool,
}

/// Field Settings of a row or column field.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PivotFieldSettings {
    pub subtotals: FieldSubtotals,
    pub form: ReportLayout,
    pub repeat_labels: bool,
    pub blank_line: bool,
    pub sort: PivotSort,
    /// Sort by a value field's values (by position in
    /// [`PivotLayout::values`]) rather than by label.
    pub sort_by_value: Option<usize>,
    pub filter: Option<PivotFieldFilter>,
}

/// How a field is grouped by ranges (Excel's Grouping dialog).
#[derive(Debug, Clone, PartialEq)]
pub enum PivotGrouping {
    Numbers {
        start: f64,
        end: f64,
        by: f64,
        auto_start: bool,
        auto_end: bool,
    },
    Dates {
        start: f64,
        end: f64,
        auto_start: bool,
        auto_end: bool,
        /// From the finest; at least one.
        levels: Vec<DateLevel>,
        /// Number of days, when Days is the only level.
        days: u32,
    },
}

/// The PivotTable Styles options (Design tab).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PivotStyleOptions {
    /// `PivotStyleLight16`...; `None` for no style.
    pub name: Option<String>,
    pub row_headers: bool,
    pub column_headers: bool,
    pub banded_rows: bool,
    pub banded_columns: bool,
}

/// An item (distinct value) of a source field.
#[derive(Debug, Clone, PartialEq)]
pub struct PivotItem {
    pub label: String,
    /// Left out by the field's filter.
    pub hidden: bool,
}

/// A field of the pivot table's cache, as the field list shows it: a
/// column of the source, a calculated field or a grouping field.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PivotSourceField {
    pub name: String,
    /// Holds numbers (or dates) only: checking it adds it to Values.
    pub numeric: bool,
    /// The source's number format for its first value, if not General.
    pub num_fmt: Option<String>,
    /// Its distinct values, sorted (a grouped field's groups, in order).
    pub items: Vec<PivotItem>,
    /// Field Settings (as a row or column field).
    pub settings: PivotFieldSettings,
    /// A calculated field's formula (with `=`).
    pub formula: Option<String>,
    /// Its range grouping (on the field grouped; levels above the finest
    /// are fields of their own, see `group_base`).
    pub grouping: Option<PivotGrouping>,
    /// A grouping field (`Years`, `Region2`): the field it groups.
    pub group_base: Option<usize>,
    /// Holds dates.
    pub dates: bool,
    /// Its smallest and largest values (numbers and dates).
    pub min: Option<f64>,
    pub max: Option<f64>,
}

/// Excel's Report Layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum ReportLayout {
    #[default]
    Compact,
    Outline,
    Tabular,
}

/// Where subtotals go (tabular form puts them at the bottom).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum SubtotalPlace {
    Off,
    #[default]
    Top,
    Bottom,
}

/// A pivot table's fields and options, as the PivotTable Fields list
/// arranges them. Field indexes are into `fields`.
#[derive(Debug, Clone, PartialEq)]
pub struct PivotLayout {
    pub name: String,
    /// The source as a reference or a table name, e.g. `Data!A1:E13`.
    pub source: String,
    pub fields: Vec<PivotSourceField>,
    pub filters: Vec<usize>,
    pub columns: Vec<AxisEntry>,
    pub rows: Vec<AxisEntry>,
    pub values: Vec<PivotValueField>,
    /// The item each report filter shows (`None` for all), by position in
    /// `filters`.
    pub filter_item: Vec<Option<String>>,
    pub layout: ReportLayout,
    pub subtotals: SubtotalPlace,
    /// A grand total row at the bottom (totals of each column).
    pub grand_total_row: bool,
    /// A grand total column at the right (totals of each row).
    pub grand_total_column: bool,
    /// Why it can't be changed or refreshed, if it can't.
    pub refresh_problem: Option<String>,
    pub style: PivotStyleOptions,
}

impl PivotLayout {
    /// Which area holds field `f`, if any (Values for a value field).
    pub fn area_of(&self, f: usize) -> Option<&'static str> {
        if self.filters.contains(&f) {
            Some("Filters")
        } else if self.columns.contains(&AxisEntry::Field(f)) {
            Some("Columns")
        } else if self.rows.contains(&AxisEntry::Field(f)) {
            Some("Rows")
        } else if self.values.iter().any(|v| v.field == f) {
            Some("Values")
        } else {
            None
        }
    }

    /// Check or uncheck field `f` in the list, as Excel does: checking a
    /// numeric field adds it to Values as Sum (with the source's number
    /// format), any other to Rows; unchecking takes it out of every area.
    pub fn toggle_field(&mut self, f: usize) {
        if self.area_of(f).is_some() {
            self.remove_field(f);
        } else if self.fields[f].numeric {
            self.add_value(f);
        } else {
            self.rows.push(AxisEntry::Field(f));
        }
        self.fix_values_entry();
    }

    /// Take field `f` out of every area.
    pub fn remove_field(&mut self, f: usize) {
        if let Some(k) = self.filters.iter().position(|x| *x == f) {
            self.filters.remove(k);
            self.filter_item.remove(k);
        }
        self.rows.retain(|e| *e != AxisEntry::Field(f));
        self.columns.retain(|e| *e != AxisEntry::Field(f));
        self.values.retain(|v| v.field != f);
        self.fix_values_entry();
    }

    /// Take the k-th value field out.
    pub fn remove_value(&mut self, k: usize) {
        if k < self.values.len() {
            self.values.remove(k);
        }
        self.fix_values_entry();
    }

    /// Add field `f` to Values: Sum for numbers, Count otherwise.
    pub fn add_value(&mut self, f: usize) {
        let numeric = self.fields[f].numeric;
        self.values.push(PivotValueField {
            show_as: ShowAs::Normal,
            base_field: None,
            base_item: PivotBaseItem::Previous,
            field: f,
            func: if numeric {
                PivotFunc::Sum
            } else {
                PivotFunc::Count
            },
            name: None,
            num_fmt: if numeric {
                self.fields[f].num_fmt.clone()
            } else {
                None
            },
        });
        self.fix_values_entry();
    }

    /// Move an entry to `area` (`"Filters"`, `"Columns"`, `"Rows"`,
    /// `"Values"`) at position `at` (clamped). A field moved out of Values
    /// keeps its other places; a field moved into an axis leaves the others
    /// (a field is on one axis at a time, as in Excel).
    pub fn move_entry(&mut self, entry: AreaEntry, area: &str, at: usize) {
        match (entry, area) {
            (AreaEntry::Value(k), "Values") => {
                if k < self.values.len() {
                    let v = self.values.remove(k);
                    let at = at.min(self.values.len());
                    self.values.insert(at, v);
                }
            }
            (AreaEntry::Value(k), _) => {
                if k < self.values.len() {
                    let f = self.values.remove(k).field;
                    self.place_field(f, area, at);
                }
            }
            (AreaEntry::Field(f), "Values") => {
                self.take_off_axes(AxisEntry::Field(f));
                let at = at.min(self.values.len());
                self.add_value(f);
                let v = self.values.pop().expect("just added");
                self.values.insert(at, v);
            }
            (AreaEntry::Field(f), _) => self.place_field(f, area, at),
            (AreaEntry::Values, "Rows" | "Columns") => {
                self.take_off_axes(AxisEntry::Values);
                let list = if area == "Rows" {
                    &mut self.rows
                } else {
                    &mut self.columns
                };
                let at = at.min(list.len());
                list.insert(at, AxisEntry::Values);
            }
            (AreaEntry::Values, _) => {}
        }
        self.fix_values_entry();
    }

    fn take_off_axes(&mut self, e: AxisEntry) {
        if let AxisEntry::Field(f) = e
            && let Some(k) = self.filters.iter().position(|x| *x == f)
        {
            self.filters.remove(k);
            self.filter_item.remove(k);
        }
        self.rows.retain(|x| *x != e);
        self.columns.retain(|x| *x != e);
    }

    fn place_field(&mut self, f: usize, area: &str, at: usize) {
        // Same area: just reorder.
        let was_filter = self.filters.iter().position(|x| *x == f);
        let item = was_filter.and_then(|k| self.filter_item[k].clone());
        self.take_off_axes(AxisEntry::Field(f));
        match area {
            "Filters" => {
                let at = at.min(self.filters.len());
                self.filters.insert(at, f);
                self.filter_item.insert(at, item);
            }
            "Rows" | "Columns" => {
                let list = if area == "Rows" {
                    &mut self.rows
                } else {
                    &mut self.columns
                };
                let at = at.min(list.len());
                list.insert(at, AxisEntry::Field(f));
            }
            _ => {}
        }
    }

    /// The `Σ Values` entry is there with two value fields or more (in
    /// Columns unless placed).
    fn fix_values_entry(&mut self) {
        let has =
            self.rows.contains(&AxisEntry::Values) || self.columns.contains(&AxisEntry::Values);
        if self.values.len() >= 2 && !has {
            self.columns.push(AxisEntry::Values);
        } else if self.values.len() < 2 {
            self.rows.retain(|e| *e != AxisEntry::Values);
            self.columns.retain(|e| *e != AxisEntry::Values);
        }
    }

    /// The caption a value field shows (its name, else `Sum of Units`).
    pub fn value_caption(&self, k: usize) -> String {
        let v = &self.values[k];
        v.name.clone().unwrap_or_else(|| {
            v.func
                .func()
                .default_name(self.fields.get(v.field).map_or("", |f| f.name.as_str()))
        })
    }
}

/// Something in one of the four areas, for [`PivotLayout::move_entry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AreaEntry {
    /// A source field (in Filters, Columns or Rows; or to add to Values).
    Field(usize),
    /// The k-th value field.
    Value(usize),
    /// The `Σ Values` field.
    Values,
}

/// Where Insert PivotTable puts the new pivot table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PivotDestination {
    /// A new sheet (named like Excel's: `Sheet2`...), at `A3`.
    NewSheet,
    /// Its top-left cell on an existing sheet.
    Cell { sheet: u32, row: u32, col: u32 },
}

/// A source checked for Insert PivotTable.
#[derive(Debug, Clone, PartialEq)]
pub struct PivotSourceCheck {
    pub sheet: u32,
    /// Header row and data rows.
    pub range: CellRange,
    /// The column headers.
    pub fields: Vec<String>,
    /// A table's name, when the source is a table.
    pub table: Option<String>,
}

const STYLE: &str = "PivotStyleLight16";
const NS_MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn quote_sheet(name: &str) -> String {
    let plain = name.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !name.chars().next().is_some_and(|c| c.is_ascii_digit());
    if plain {
        name.to_owned()
    } else {
        format!("'{}'", name.replace('\'', "''"))
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

impl Workbook {
    /// What Insert PivotTable offers as its source, as reference text: the
    /// table the active cell is in, else the selection if it is more than
    /// one cell, else the current region around the active cell. Empty if
    /// there's no data there.
    pub fn pivot_source_suggestion(
        &self,
        sheet: u32,
        selection: CellRange,
        (row, col): (u32, u32),
    ) -> String {
        if let Some(t) = self.table_at(sheet, row, col) {
            return t.name;
        }
        let range = if selection.is_single() {
            if !self.is_filled(sheet, row, col) {
                return String::new();
            }
            self.current_region(sheet, row, col)
        } else {
            let (rows, cols) = self.used_size(sheet);
            match selection.clip(rows.max(1), cols.max(1)) {
                Some(r) => r,
                None => return String::new(),
            }
        };
        let name = self
            .sheet_names()
            .get(sheet as usize)
            .cloned()
            .unwrap_or_default();
        format!("{}!{}", quote_sheet(&name), absolute(range))
    }

    /// Resolve and check a source typed for Insert PivotTable: a table
    /// name, a defined name, or a range (`A1:E13` on `sheet`, or with a
    /// sheet). The header row must have a name in every column, each one
    /// different, and there must be rows under it.
    pub fn check_pivot_source(&self, text: &str, sheet: u32) -> Result<PivotSourceCheck> {
        let (source, _, asked, table) = self.resolve_pivot_source(text, sheet)?;
        let cache = new_cache(source, Vec::new(), self.history.layout());
        let (s, area, data) = self.read_source(&cache)?;
        // Columns past the used ones have empty headers too.
        let right = asked
            .right
            .min(asked.left + 16_383)
            .min(crate::MAX_COLS - 1);
        if asked.top == area.top && right > area.right {
            bail!(
                "the PivotTable field name is not valid: {} in the source's header row is empty",
                cell_name(area.top, area.right + 1)
            );
        }
        let mut seen: HashSet<String> = HashSet::new();
        for c in area.left..=area.right {
            let n = self.display_value(s, area.top, c).trim().to_lowercase();
            if !seen.insert(n) {
                bail!(
                    "the PivotTable field name {:?} is used twice in the header row ({}): give each column its own name",
                    self.display_value(s, area.top, c).trim(),
                    cell_name(area.top, c)
                );
            }
        }
        Ok(PivotSourceCheck {
            sheet: s,
            range: range_of(area),
            fields: data.names,
            table,
        })
    }

    /// `(source, sheet, area, table name)` of a source text.
    fn resolve_pivot_source(
        &self,
        text: &str,
        sheet: u32,
    ) -> Result<(Source, u32, Area, Option<String>)> {
        let text = text.trim().trim_start_matches('=').trim();
        if text.is_empty() {
            bail!("choose the data to analyse: a table or a range with a header row");
        }
        let model = self.model.get_model();
        if let Some(t) = ironcalc_base::kantor_tables::find_table_pub(&model.workbook.tables, text)
        {
            let s = model
                .workbook
                .worksheets
                .iter()
                .position(|w| w.name == t.sheet_name)
                .ok_or_else(|| anyhow!("the table {text} has no sheet"))?;
            let (top, left, bottom, right) = ironcalc_base::kantor_tables::table_area(t)
                .ok_or_else(|| anyhow!("the table {text} has no range"))?;
            return Ok((
                Source::Name(t.name.clone()),
                s as u32,
                Area {
                    top: top as u32 - 1,
                    left: left as u32 - 1,
                    bottom: bottom as u32 - 1,
                    right: right as u32 - 1,
                },
                Some(t.name.clone()),
            ));
        }
        let (sheet_name, range_text) = match text.rsplit_once('!') {
            Some((s, r)) => {
                let s = s.trim();
                let s = s
                    .strip_prefix('\'')
                    .and_then(|x| x.strip_suffix('\''))
                    .map(|x| x.replace("''", "'"))
                    .unwrap_or_else(|| s.to_owned());
                (Some(s), r)
            }
            None => (None, text),
        };
        let range = crate::formula::parse_range(&range_text.replace('$', ""));
        let Some(range) = range else {
            // A defined name.
            if sheet_name.is_none()
                && model
                    .workbook
                    .defined_names
                    .iter()
                    .any(|d| d.name.eq_ignore_ascii_case(text))
            {
                let cache = new_cache(
                    Source::Name(text.to_owned()),
                    Vec::new(),
                    self.history.layout(),
                );
                let (s, area, _) = self.read_source(&cache)?;
                return Ok((Source::Name(text.to_owned()), s, area, None));
            }
            bail!("{text:?} is not a range, a table or a name in this workbook");
        };
        let s = match &sheet_name {
            Some(n) => model
                .workbook
                .worksheets
                .iter()
                .position(|w| w.name.eq_ignore_ascii_case(n))
                .ok_or_else(|| anyhow!("there is no sheet {n:?}"))? as u32,
            None => sheet,
        };
        let name = model.workbook.worksheets[s as usize].name.clone();
        if range.is_single() {
            bail!("the source must have a header row and at least one row of data");
        }
        Ok((
            Source::Range {
                sheet: name,
                area: Area {
                    top: range.top,
                    left: range.left,
                    bottom: range.bottom,
                    right: range.right,
                },
            },
            s,
            Area {
                top: range.top,
                left: range.left,
                bottom: range.bottom,
                right: range.right,
            },
            None,
        ))
    }

    /// Make a pivot table of `source` (see [`Workbook::check_pivot_source`])
    /// at `dest`, with no fields yet, as Excel's Insert PivotTable does (one
    /// undo step). Returns its sheet and top-left cell.
    pub fn insert_pivot(
        &mut self,
        source: &str,
        source_sheet: u32,
        dest: PivotDestination,
    ) -> Result<(u32, u32, u32)> {
        let check = self.check_pivot_source(source, source_sheet)?;
        let (src, _, _, _) = self.resolve_pivot_source(source, source_sheet)?;
        let layout = self.history.layout();
        let fields: Vec<CacheField> = check
            .fields
            .iter()
            .map(|n| CacheField::source(n.clone()))
            .collect();
        let (sheet_id, new_sheet, top, left) = match dest {
            PivotDestination::NewSheet => {
                let id = self
                    .model
                    .get_model()
                    .workbook
                    .worksheets
                    .iter()
                    .map(|w| w.sheet_id)
                    .max()
                    .unwrap_or(0)
                    + 1;
                (id, Some(id), 2, 0)
            }
            PivotDestination::Cell { sheet, row, col } => {
                let range = CellRange {
                    top: row,
                    left: col,
                    bottom: row,
                    right: col,
                };
                if self.pivot_at(sheet, row, col).is_some() {
                    bail!("{} is inside another PivotTable", range.name());
                }
                (self.sheet_id(sheet), None, row, col)
            }
        };
        let k = self.pivots.defs.len() + 1;
        // A pivot table on the same source shares its cache, as Excel does.
        let shared = self.shared_cache_for(&src);
        let mut cache = new_cache(src, fields, layout.clone());
        cache.part = format!(
            "{NEW_PREFIX}pivotCacheDefinition{}.xml",
            self.pivots.caches.len() + 1
        );
        cache.records_part = Some(format!(
            "{NEW_PREFIX}pivotCacheRecords{}.xml",
            self.pivots.caches.len() + 1
        ));
        let name = self.next_pivot_name();
        let captions = Captions::new();
        let def = Def {
            part: format!("{NEW_PREFIX}pivotTable{k}.xml"),
            sheet_id,
            name: name.clone(),
            cache: self.pivots.caches.len(),
            location: Area {
                top,
                left,
                bottom: top,
                right: left,
            },
            first_header_row: 1,
            first_data_row: 1,
            first_data_col: 0,
            row_page_count: 0,
            col_page_count: 0,
            fields: (0..check.fields.len()).map(|_| new_field()).collect(),
            row_fields: Vec::new(),
            col_fields: Vec::new(),
            row_items: Vec::new(),
            col_items: Vec::new(),
            page_fields: Vec::new(),
            data_fields: Vec::new(),
            filters: Vec::new(),
            row_grand: true,
            col_grand: true,
            style: StyleInfo {
                name: Some(STYLE.into()),
                row_headers: true,
                col_headers: true,
                row_stripes: false,
                col_stripes: false,
                last_column: true,
            },
            grand_caption: captions.grand_total.clone(),
            row_header_caption: captions.row_labels.clone(),
            col_header_caption: captions.column_labels.clone(),
            data_caption: captions.values.clone(),
            missing_caption: None,
            error_caption: None,
            xml: Arc::new(def_skeleton(&name, &captions)),
            reindexed: false,
            at: layout,
            unsupported: None,
            added: true,
            in_origin: false,
            root_sets: Vec::new(),
        };
        let mut def = def;
        let ci = match shared {
            Some(ci) => {
                def.cache = ci;
                ci
            }
            None => {
                self.pivots.caches.push(Arc::new(cache));
                self.pivots.caches.len() - 1
            }
        };
        let di = self.pivots.defs.len();
        self.pivots.defs.push(Arc::new(def.clone()));
        let change = Rebuild {
            defs: HashMap::from([(di, def)]),
            new_sheet,
            ..Default::default()
        };
        match self.rebuild_pivots(&[ci], change) {
            Ok(_) => {}
            Err(e) => {
                self.pivots.defs.pop();
                if shared.is_none() {
                    self.pivots.caches.pop();
                }
                return Err(e);
            }
        }
        self.cache.borrow_mut().pivots.clear();
        let sheet =
            self.model
                .get_model()
                .workbook
                .worksheets
                .iter()
                .position(|w| w.sheet_id == self.pivot_def(&self.pivot_state(), di).sheet_id)
                .ok_or_else(|| anyhow!("the PivotTable's sheet is missing"))? as u32;
        Ok((sheet, top, left))
    }

    /// The cache of a pivot table in the workbook on source `src` (the
    /// same range on the same sheet, or the same table or name), if one can
    /// be shared.
    fn shared_cache_for(&self, src: &Source) -> Option<usize> {
        let state = self.pivot_state();
        (0..self.pivots.defs.len())
            .filter(|&i| self.pivot_present(&state, i))
            .map(|i| self.pivot_def(&state, i).cache)
            .find(|&ci| {
                let c = self.pivot_cache(&state, ci);
                if c.unsupported.is_some() {
                    return false;
                }
                match (&c.source, src) {
                    (Some(Source::Name(a)), Source::Name(b)) => a.eq_ignore_ascii_case(b),
                    (
                        Some(Source::Range { sheet, area }),
                        Source::Range {
                            sheet: s2,
                            area: a2,
                        },
                    ) => {
                        sheet.eq_ignore_ascii_case(s2)
                            && self
                                .sheet_index_by_name(sheet)
                                .and_then(|k| self.now(&c.at, self.sheet_id(k), *area))
                                == Some(*a2)
                    }
                    _ => false,
                }
            })
    }

    /// `PivotTable1`, `PivotTable2`... the first not taken.
    fn next_pivot_name(&self) -> String {
        let taken: HashSet<String> = self
            .pivots
            .defs
            .iter()
            .map(|d| d.name.to_lowercase())
            .collect();
        (1..)
            .map(|n| format!("PivotTable{n}"))
            .find(|n| !taken.contains(&n.to_lowercase()))
            .expect("a free name")
    }

    /// The index of the pivot table at `(row, col)` of `sheet`.
    fn pivot_index_at(&self, sheet: u32, row: u32, col: u32) -> Result<usize> {
        self.placed_pivots(sheet)
            .iter()
            .find(|p| p.contains(row, col))
            .map(|p| p.index)
            .ok_or_else(|| anyhow!("there is no PivotTable at {}", cell_name(row, col)))
    }

    /// The fields and options of the pivot table at `(row, col)`, for the
    /// PivotTable Fields list.
    pub fn pivot_layout(&self, sheet: u32, row: u32, col: u32) -> Option<PivotLayout> {
        let i = self.pivot_index_at(sheet, row, col).ok()?;
        let state = self.pivot_state();
        let def = self.pivot_def(&state, i);
        let cache = self.pivot_cache(&state, def.cache);
        let source_data = self.read_source(&cache).ok();
        let num_fmts = &self.model.get_model().workbook.styles.num_fmts;
        let fmt_code = |id: Option<u32>| {
            id.filter(|&n| n != 0)
                .map(|n| ironcalc_base::number_format::get_num_fmt(n as i32, num_fmts))
                .filter(|c| !c.is_empty() && c != "general")
        };
        let mut fields = Vec::with_capacity(cache.fields.len());
        // Records, for grouped fields' items in use and calculated fields.
        let records: Option<Vec<Vec<Val>>> = match (&source_data, &cache.records) {
            (Some((_, _, d)), _) => Some(d.rows.clone()),
            (None, Some(r)) => Some((**r).clone()),
            _ => None,
        };
        for (f, cf) in cache.fields.iter().enumerate() {
            // Distinct values: from the source now, else the cache.
            let column: Vec<Val> = match (&source_data, &cache.records) {
                _ if !cf.database => Vec::new(),
                (Some((_, _, d)), _) => match d
                    .names
                    .iter()
                    .position(|n| n.eq_ignore_ascii_case(&cf.name))
                {
                    Some(k) => d.rows.iter().map(|r| r[k].clone()).collect(),
                    None => Vec::new(),
                },
                (None, Some(recs)) => recs.iter().filter_map(|r| r.get(f).cloned()).collect(),
                (None, None) => cf.items.clone(),
            };
            let numeric = cf.formula.is_some()
                || (cf.group.is_none()
                    && column.iter().any(|v| v.number().is_some())
                    && column
                        .iter()
                        .all(|v| v.number().is_some() || *v == Val::Missing));
            let dates = column.iter().any(|v| matches!(v, Val::Date(_)));
            let nums: Vec<f64> = column.iter().filter_map(Val::number).collect();
            let (min, max) = if nums.is_empty() {
                (None, None)
            } else {
                (
                    Some(nums.iter().copied().fold(f64::INFINITY, f64::min)),
                    Some(nums.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
                )
            };
            let grouped = cf.group.as_ref().is_some_and(|g| g.groups_items());
            let values: Vec<Val> = if grouped {
                // Groups in their order, those with data.
                let used: Option<HashSet<usize>> = records.as_ref().and_then(|r| {
                    crate::pivot_group::group_positions(&cache.fields, f, r)
                        .map(|p| p.into_iter().flatten().collect())
                });
                cf.item_list()
                    .iter()
                    .enumerate()
                    .filter(|(k, _)| used.as_ref().is_none_or(|u| u.contains(k)))
                    .map(|(_, v)| v.clone())
                    .collect()
            } else {
                let mut seen = HashSet::new();
                let mut values: Vec<Val> = column
                    .iter()
                    .filter(|v| seen.insert(v.key()))
                    .cloned()
                    .collect();
                values.sort_by(|a, b| {
                    a.sort_key()
                        .partial_cmp(&b.sort_key())
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                values
            };
            let fd = def.fields.get(f);
            let hidden: HashSet<String> = fd
                .map(|fd| {
                    fd.items
                        .iter()
                        .filter(|it| it.hidden)
                        .filter_map(|it| it.x)
                        .filter_map(|x| cf.item_list().get(x))
                        .map(|v| v.key())
                        .collect()
                })
                .unwrap_or_default();
            let num_fmt = source_data.as_ref().and_then(|(s, area, d)| {
                let k = d
                    .names
                    .iter()
                    .position(|n| n.eq_ignore_ascii_case(&cf.name))?;
                let code = self
                    .cell_style(*s, area.top + 1, area.left + k as u32)
                    .num_fmt
                    .clone();
                (!code.is_empty() && !code.eq_ignore_ascii_case("general")).then_some(code)
            });
            let settings = fd
                .map(|fd| field_settings(fd, f, &def.filters))
                .unwrap_or_default();
            let (grouping, group_base) = grouping_of(&cache.fields, f);
            fields.push(PivotSourceField {
                name: cf.name.clone(),
                numeric,
                num_fmt,
                items: values
                    .iter()
                    .map(|v| PivotItem {
                        label: v.label(),
                        hidden: hidden.contains(&v.key()),
                    })
                    .collect(),
                settings,
                formula: cf.formula.as_ref().map(|f| format!("={f}")),
                grouping,
                group_base,
                dates,
                min,
                max,
            });
        }
        let entry = |x: &i32| match *x {
            -2 => Some(AxisEntry::Values),
            x if x >= 0 && (x as usize) < cache.fields.len() => Some(AxisEntry::Field(x as usize)),
            _ => None,
        };
        let filters: Vec<usize> = def
            .page_fields
            .iter()
            .map(|p| p.fld)
            .filter(|f| *f < cache.fields.len())
            .collect();
        let filter_item = def
            .page_fields
            .iter()
            .filter(|p| p.fld < cache.fields.len())
            .map(|p| {
                p.item
                    .filter(|k| *k < def.fields[p.fld].items.len())
                    .map(|k| def.item_label(&cache, p.fld, k).label())
            })
            .collect();
        let values = def
            .data_fields
            .iter()
            .map(|d| {
                // A name other than the default (in the interface
                // language) is kept as the file has it.
                let default = d
                    .func
                    .default_name(cache.fields.get(d.fld).map_or("", |c| c.name.as_str()));
                let base_item = match d.base_item {
                    _ if !d.show_as.needs_item() => PivotBaseItem::Previous,
                    BASE_PREVIOUS => PivotBaseItem::Previous,
                    BASE_NEXT => PivotBaseItem::Next,
                    k => def
                        .fields
                        .get(d.base_field)
                        .filter(|f| (k as usize) < f.items.len())
                        .map_or(PivotBaseItem::Previous, |_| {
                            PivotBaseItem::Item(
                                def.item_label(&cache, d.base_field, k as usize).label(),
                            )
                        }),
                };
                PivotValueField {
                    field: d.fld,
                    func: PivotFunc::of(d.func),
                    name: d.name.clone().filter(|n| *n != default),
                    num_fmt: fmt_code(d.num_fmt_id),
                    show_as: d.show_as,
                    base_field: (d.show_as.needs_field() && d.base_field < cache.fields.len())
                        .then_some(d.base_field),
                    base_item,
                }
            })
            .collect();
        // Layout and subtotals as the first field on an axis has them.
        let lead = def
            .row_fields
            .iter()
            .chain(&def.col_fields)
            .find(|x| **x >= 0)
            .map(|x| *x as usize)
            .or((!def.fields.is_empty()).then_some(0))
            .and_then(|f| def.fields.get(f));
        let (layout, subtotals) = match lead {
            Some(f) => (
                if !f.outline {
                    ReportLayout::Tabular
                } else if !f.compact {
                    ReportLayout::Outline
                } else {
                    ReportLayout::Compact
                },
                if !f.default_subtotal {
                    SubtotalPlace::Off
                } else if f.subtotal_top && f.outline {
                    SubtotalPlace::Top
                } else {
                    SubtotalPlace::Bottom
                },
            ),
            None => (ReportLayout::Compact, SubtotalPlace::Top),
        };
        let source = match &cache.source {
            Some(Source::Range { sheet, area }) => {
                let now = self
                    .model
                    .get_model()
                    .workbook
                    .worksheets
                    .iter()
                    .find(|w| w.name.eq_ignore_ascii_case(sheet))
                    .and_then(|w| self.now(&cache.at, w.sheet_id, *area))
                    .unwrap_or(*area);
                format!("{}!{}", quote_sheet(sheet), absolute(range_of(now)))
            }
            Some(Source::Name(n)) => n.clone(),
            None => String::new(),
        };
        Some(PivotLayout {
            name: def.name.clone(),
            source,
            fields,
            filters,
            columns: def.col_fields.iter().filter_map(entry).collect(),
            rows: def.row_fields.iter().filter_map(entry).collect(),
            values,
            filter_item,
            layout,
            subtotals,
            grand_total_row: def.col_grand,
            grand_total_column: def.row_grand,
            refresh_problem: cache
                .unsupported
                .clone()
                .or_else(|| def.unsupported.clone()),
            style: PivotStyleOptions {
                name: def.style.name.clone(),
                row_headers: def.style.row_headers,
                column_headers: def.style.col_headers,
                banded_rows: def.style.row_stripes,
                banded_columns: def.style.col_stripes,
            },
        })
    }

    /// Arrange the pivot table at `(row, col)` as `layout` says and lay it
    /// out again from its source (one undo step).
    pub fn set_pivot_layout(
        &mut self,
        sheet: u32,
        row: u32,
        col: u32,
        layout: &PivotLayout,
    ) -> Result<()> {
        let i = self.pivot_index_at(sheet, row, col)?;
        let state = self.pivot_state();
        let base = self.pivot_def(&state, i);
        let cache = self.pivot_cache(&state, base.cache);
        if let Some(why) = cache.unsupported.as_ref().or(base.unsupported.as_ref()) {
            bail!("can't change the PivotTable {}: {why}", base.name);
        }
        let nf = cache.fields.len();
        let ok = |f: usize| f < nf && f < layout.fields.len();
        // A calculated field can only be summarised (as in Excel).
        for e in layout.rows.iter().chain(&layout.columns) {
            if let AxisEntry::Field(f) = e
                && cache.fields.get(*f).is_some_and(|c| c.formula.is_some())
            {
                bail!(
                    "The field {} is a calculated field: it can only go in Values",
                    cache.fields[*f].name
                );
            }
        }
        if let Some(f) = layout
            .filters
            .iter()
            .find(|f| cache.fields.get(**f).is_some_and(|c| c.formula.is_some()))
        {
            bail!(
                "The field {} is a calculated field: it can only go in Values",
                cache.fields[*f].name
            );
        }
        let mut def = (*base).clone();
        let axis = |list: &[AxisEntry]| -> Vec<i32> {
            list.iter()
                .filter_map(|e| match e {
                    AxisEntry::Values => Some(-2),
                    AxisEntry::Field(f) if ok(*f) => Some(*f as i32),
                    AxisEntry::Field(_) => None,
                })
                .collect()
        };
        def.row_fields = axis(&layout.rows);
        def.col_fields = axis(&layout.columns);
        def.page_fields = layout
            .filters
            .iter()
            .filter(|f| ok(**f))
            .map(|&fld| PageField {
                fld,
                item: None,
                xml: None,
            })
            .collect();
        let num_fmts = self.model.get_model().workbook.styles.num_fmts.clone();
        let fmt_id = |code: &str| -> Option<u32> {
            if let Some(id) = ironcalc_base::number_format::get_default_num_fmt_id(code) {
                return Some(id as u32);
            }
            num_fmts
                .iter()
                .find(|n| n.format_code == code)
                .map(|n| n.num_fmt_id as u32)
        };
        let mut names: HashSet<String> = HashSet::new();
        let mut data_fields = Vec::new();
        for (k, v) in layout.values.iter().enumerate() {
            if !ok(v.field) {
                continue;
            }
            let wanted = layout.value_caption(k);
            let mut name = wanted.clone();
            let mut n = 2;
            while !names.insert(name.to_lowercase())
                || cache
                    .fields
                    .iter()
                    .any(|c| c.name.eq_ignore_ascii_case(&name))
            {
                name = format!("{wanted}{n}");
                n += 1;
                if n > 1000 {
                    break;
                }
            }
            let num_fmt_id = match &v.num_fmt {
                Some(code) => Some(fmt_id(code).ok_or_else(|| {
                    anyhow!("the number format {code:?} is not in this workbook")
                })?),
                None => None,
            };
            if v.show_as.needs_field() && v.base_field.is_none_or(|b| !ok(b)) {
                bail!(
                    "choose a base field for {:?} ({})",
                    layout.value_caption(k),
                    v.show_as.label()
                );
            }
            data_fields.push(DataField {
                name: Some(name),
                fld: v.field,
                func: v.func.func(),
                num_fmt_id,
                show_as: v.show_as,
                // The item is matched once the cache is rebuilt.
                base_field: v.base_field.filter(|b| ok(*b)).unwrap_or(0),
                base_item: match v.base_item {
                    _ if !v.show_as.needs_item() => 0,
                    PivotBaseItem::Next => BASE_NEXT,
                    _ => BASE_PREVIOUS,
                },
                xml: None,
            });
        }
        def.data_fields = data_fields;
        // Each field's axis and data flag (what `rematch` gives items by).
        for (j, f) in def.fields.iter_mut().enumerate() {
            let x = j as i32;
            f.axis = if def.row_fields.contains(&x) {
                Some("axisRow".into())
            } else if def.col_fields.contains(&x) {
                Some("axisCol".into())
            } else if def.page_fields.iter().any(|p| p.fld == j) {
                Some("axisPage".into())
            } else {
                None
            };
            f.data = def.data_fields.iter().any(|d| d.fld == j);
        }
        def.row_grand = layout.grand_total_column;
        def.col_grand = layout.grand_total_row;
        let axis_names: Vec<String> = layout
            .rows
            .iter()
            .chain(&layout.columns)
            .filter_map(|e| match e {
                AxisEntry::Field(f) if ok(*f) => Some(*f),
                _ => None,
            })
            .chain(layout.filters.iter().copied().filter(|f| ok(*f)))
            .map(|f| cache.fields[f].name.clone())
            .collect();
        // A changed report layout or subtotal place applies to every
        // field (as Excel's Design tab does); else each field's settings.
        let current = self.pivot_layout(sheet, row, col);
        let all_layout = current.as_ref().is_none_or(|c| c.layout != layout.layout);
        let all_subtotals = current
            .as_ref()
            .is_none_or(|c| c.subtotals != layout.subtotals);
        // Filters, by field name (one per field, as Excel keeps them).
        let mut filters: Vec<(String, PivotFieldFilter)> = Vec::new();
        for (f, sf) in layout.fields.iter().enumerate() {
            let Some(flt) = &sf.settings.filter else {
                continue;
            };
            if !ok(f) || filter_family(&flt.kind).is_none() {
                bail!("{:?} is not a filter Kantor knows", flt.kind);
            }
            let needs_value = matches!(
                filter_family(&flt.kind),
                Some(crate::pivot_calc::FilterFamily::Value | crate::pivot_calc::FilterFamily::Top)
            );
            if needs_value && flt.value_field.is_none_or(|v| v >= layout.values.len()) {
                bail!("choose the value field the filter on {} tests", sf.name);
            }
            filters.push((cache.fields[f].name.clone(), flt.clone()));
        }
        let wanted = layout.clone();
        let d_values = layout.values.len();
        let adjust = move |d: &mut Def, c: &Cache| {
            let report = wanted.layout;
            let by_name = |name: &str| {
                wanted
                    .fields
                    .iter()
                    .position(|w| w.name.eq_ignore_ascii_case(name))
            };
            for (j, f) in d.fields.iter_mut().enumerate() {
                let lf = c
                    .fields
                    .get(j)
                    .and_then(|cf| by_name(&cf.name))
                    .map(|k| &wanted.fields[k]);
                let form = match (all_layout, lf) {
                    (false, Some(lf)) => lf.settings.form,
                    _ => report,
                };
                f.compact = form == ReportLayout::Compact;
                f.outline = form != ReportLayout::Tabular;
                if all_subtotals || lf.is_none() {
                    f.default_subtotal = wanted.subtotals != SubtotalPlace::Off;
                    f.subtotal_top = wanted.subtotals == SubtotalPlace::Top;
                    f.subtotals.clear();
                } else if let Some(lf) = lf {
                    match &lf.settings.subtotals {
                        FieldSubtotals::Automatic => {
                            f.default_subtotal = true;
                            f.subtotals.clear();
                        }
                        FieldSubtotals::None => {
                            f.default_subtotal = false;
                            f.subtotals.clear();
                        }
                        FieldSubtotals::Custom(list) => {
                            f.default_subtotal = false;
                            f.subtotals = PivotFunc::ALL
                                .into_iter()
                                .filter(|x| list.contains(x))
                                .map(PivotFunc::func)
                                .collect();
                        }
                    }
                }
                let Some(lf) = lf else {
                    continue;
                };
                f.fill_down = lf.settings.repeat_labels;
                f.insert_blank = lf.settings.blank_line;
                f.sort = match lf.settings.sort {
                    PivotSort::Manual => None,
                    PivotSort::Ascending => Some("ascending".into()),
                    PivotSort::Descending => Some("descending".into()),
                };
                f.sort_by = lf
                    .settings
                    .sort_by_value
                    .filter(|v| *v < d_values)
                    .filter(|_| lf.settings.sort != PivotSort::Manual);
                // Items in label order for a field sorted by label.
                if f.sort_by.is_none() && lf.settings.sort != PivotSort::Manual {
                    let grouped = c.fields[j].group.as_ref().is_some_and(|g| g.groups_items());
                    let list = c.fields[j].item_list();
                    let desc = lf.settings.sort == PivotSort::Descending;
                    let (mut items, subs): (Vec<_>, Vec<_>) =
                        f.items.drain(..).partition(|it| it.x.is_some());
                    items.sort_by(|a, b| {
                        let o = if grouped {
                            a.x.cmp(&b.x)
                        } else {
                            let k = |i: &crate::pivot::FieldItem| {
                                list.get(i.x.unwrap_or(0)).map(|v| v.sort_key())
                            };
                            k(a).partial_cmp(&k(b)).unwrap_or(std::cmp::Ordering::Equal)
                        };
                        if desc { o.reverse() } else { o }
                    });
                    f.items = items.into_iter().chain(subs).collect();
                }
                let hidden: HashSet<&str> = lf
                    .items
                    .iter()
                    .filter(|it| it.hidden)
                    .map(|it| it.label.as_str())
                    .collect();
                for it in f.items.iter_mut() {
                    if let Some(x) = it.x {
                        it.hidden = c.fields[j]
                            .items
                            .get(x)
                            .is_some_and(|v| hidden.contains(v.label().as_str()));
                    }
                }
            }
            // Report filter items, by label.
            for p in d.page_fields.iter_mut() {
                let Some(cf) = c.fields.get(p.fld) else {
                    continue;
                };
                let k = wanted.filters.iter().position(|&f| {
                    wanted
                        .fields
                        .get(f)
                        .is_some_and(|w| w.name.eq_ignore_ascii_case(&cf.name))
                });
                let label = k.and_then(|k| wanted.filter_item.get(k).cloned().flatten());
                p.item = label.and_then(|l| {
                    d.fields[p.fld].items.iter().position(|it| {
                        it.x.and_then(|x| cf.items.get(x))
                            .is_some_and(|v| v.label() == l)
                    })
                });
            }
            // Value fields' base items, by label.
            let labels: Vec<Vec<(bool, String)>> = (0..d.fields.len())
                .map(|f| {
                    (0..d.fields[f].items.len())
                        .map(|i| {
                            (
                                d.fields[f].items[i].x.is_some(),
                                d.item_label(c, f, i).label(),
                            )
                        })
                        .collect()
                })
                .collect();
            for (k, df) in d.data_fields.iter_mut().enumerate() {
                let Some(v) = wanted.values.get(k) else {
                    continue;
                };
                let Some(bf) = v
                    .base_field
                    .and_then(|b| wanted.fields.get(b))
                    .and_then(|w| {
                        c.fields
                            .iter()
                            .position(|cf| cf.name.eq_ignore_ascii_case(&w.name))
                    })
                else {
                    continue;
                };
                df.base_field = bf;
                if let PivotBaseItem::Item(label) = &v.base_item {
                    df.base_item = labels[bf]
                        .iter()
                        .position(|(real, l)| *real && l == label)
                        .map_or(BASE_PREVIOUS, |i| i as u32);
                }
            }
            d.filters = filters
                .iter()
                .filter_map(|(name, f)| {
                    let fld = c
                        .fields
                        .iter()
                        .position(|cf| cf.name.eq_ignore_ascii_case(name))?;
                    Some(PivotFilter {
                        fld,
                        kind: f.kind.clone(),
                        measure: f.value_field.filter(|v| *v < d.data_fields.len()),
                        v1: f.value1.clone(),
                        v2: f.value2.clone(),
                        top: f.top,
                    })
                })
                .filter(|f| {
                    !matches!(
                        filter_family(&f.kind),
                        Some(
                            crate::pivot_calc::FilterFamily::Value
                                | crate::pivot_calc::FilterFamily::Top
                        )
                    ) || f.measure.is_some()
                })
                .collect();
            d.style = StyleInfo {
                name: wanted.style.name.clone(),
                row_headers: wanted.style.row_headers,
                col_headers: wanted.style.column_headers,
                row_stripes: wanted.style.banded_rows,
                col_stripes: wanted.style.banded_columns,
                last_column: d.style.last_column,
            };
            d.root_sets = vec![
                ("rowGrandTotals", u8::from(d.row_grand).to_string()),
                ("colGrandTotals", u8::from(d.col_grand).to_string()),
                (
                    "dataOnRows",
                    u8::from(d.row_fields.contains(&-2)).to_string(),
                ),
                (
                    "compact",
                    u8::from(report == ReportLayout::Compact).to_string(),
                ),
                (
                    "compactData",
                    u8::from(report == ReportLayout::Compact).to_string(),
                ),
                (
                    "outline",
                    u8::from(report != ReportLayout::Tabular).to_string(),
                ),
                (
                    "outlineData",
                    u8::from(report != ReportLayout::Tabular).to_string(),
                ),
            ];
        };
        let change = Rebuild {
            defs: HashMap::from([(i, def)]),
            axis: axis_names,
            adjust: Some(Box::new(adjust)),
            ..Default::default()
        };
        self.rebuild_pivots(&[base.cache], change)?;
        Ok(())
    }

    /// Delete the pivot table at `(row, col)`: its cells (and its parts at
    /// the next save; its cache too if no other pivot table uses it). One
    /// undo step.
    pub fn delete_pivot(&mut self, sheet: u32, row: u32, col: u32) -> Result<String> {
        let i = self.pivot_index_at(sheet, row, col)?;
        let p = self
            .placed_pivots(sheet)
            .iter()
            .find(|p| p.index == i)
            .cloned()
            .expect("found above");
        self.keep_pivot_records()
            .map_err(|e| anyhow!("can't delete the PivotTable: {e}"))?;
        let b = p.bounds();
        let area = range_of(b).area(sheet);
        self.model
            .range_clear_all(&area)
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        let mut next = (*self.pivot_state()).clone();
        next.gone.insert(i);
        self.history.set_pivot_state(Arc::new(next));
        self.cache.borrow_mut().pivots.clear();
        self.sync_pivot_grids(true);
        Ok(p.def.name.clone())
    }
}

fn range_of(a: Area) -> CellRange {
    CellRange {
        top: a.top,
        left: a.left,
        bottom: a.bottom,
        right: a.right,
    }
}

/// `$A$1:$E$13`.
fn absolute(r: CellRange) -> String {
    let cell = |row: u32, col: u32| format!("${}${}", crate::column_name(col), row + 1);
    format!("{}:{}", cell(r.top, r.left), cell(r.bottom, r.right))
}

fn new_field() -> Field {
    Field {
        axis: None,
        data: false,
        items: Vec::new(),
        compact: true,
        outline: true,
        subtotal_top: true,
        default_subtotal: true,
        show_all: false,
        sort: None,
        name: None,
        subtotal_caption: None,
        subtotals: Vec::new(),
        insert_blank: false,
        fill_down: false,
        sort_by: None,
        xml: None,
    }
}

fn new_cache(source: Source, fields: Vec<CacheField>, at: crate::shift::Layout) -> Cache {
    let source_xml = match &source {
        Source::Range { sheet, area } => format!(
            "<worksheetSource ref=\"{}\" sheet=\"{}\"/>",
            range_of(*area).name(),
            esc(sheet)
        ),
        Source::Name(n) => format!("<worksheetSource name=\"{}\"/>", esc(n)),
    };
    let xml = format!(
        "<pivotCacheDefinition xmlns=\"{NS_MAIN}\" xmlns:r=\"{NS_R}\" r:id=\"rId1\" refreshOnLoad=\"1\" refreshedBy=\"Kantor\" refreshedDate=\"0\" createdVersion=\"6\" refreshedVersion=\"6\" minRefreshableVersion=\"3\" recordCount=\"0\"><cacheSource type=\"worksheet\">{source_xml}</cacheSource><cacheFields count=\"0\"/></pivotCacheDefinition>"
    );
    Cache {
        part: String::new(),
        records_part: None,
        xml: Arc::new(xml),
        source: Some(source),
        fields,
        records: None,
        at,
        unsupported: None,
        added: true,
    }
}

/// A new pivot table's captions in the interface language. `None` (and
/// `Values`) where that is English: Excel's defaults, not written.
struct Captions {
    row_labels: Option<String>,
    column_labels: Option<String>,
    grand_total: Option<String>,
    values: String,
}

impl Captions {
    fn new() -> Self {
        let own = |en: &'static str| Some(tr(en)).filter(|s| *s != en).map(str::to_owned);
        Self {
            row_labels: own("Row Labels"),
            column_labels: own("Column Labels"),
            grand_total: own("Grand Total"),
            values: tr("Values").to_owned(),
        }
    }

    /// The root attributes that hold them.
    fn attributes(&self) -> String {
        let mut out = format!(" dataCaption=\"{}\"", esc(&self.values));
        for (attr, v) in [
            ("grandTotalCaption", &self.grand_total),
            ("rowHeaderCaption", &self.row_labels),
            ("colHeaderCaption", &self.column_labels),
        ] {
            if let Some(v) = v {
                out.push_str(&format!(" {attr}=\"{}\"", esc(v)));
            }
        }
        out
    }
}

/// A new pivot table definition as Excel 2016 writes one; `location`,
/// fields, items and the rest are written from the model.
fn def_skeleton(name: &str, captions: &Captions) -> String {
    format!(
        "<pivotTableDefinition xmlns=\"{NS_MAIN}\" name=\"{}\" cacheId=\"0\" applyNumberFormats=\"0\" applyBorderFormats=\"0\" applyFontFormats=\"0\" applyPatternFormats=\"0\" applyAlignmentFormats=\"0\" applyWidthHeightFormats=\"1\"{} updatedVersion=\"6\" minRefreshableVersion=\"3\" useAutoFormatting=\"1\" itemPrintTitles=\"1\" createdVersion=\"6\" indent=\"0\" outline=\"1\" outlineData=\"1\" multipleFieldFilters=\"0\"><location ref=\"A1\" firstHeaderRow=\"1\" firstDataRow=\"1\" firstDataCol=\"0\"/><pivotTableStyleInfo name=\"{STYLE}\" showRowHeaders=\"1\" showColHeaders=\"1\" showRowStripes=\"0\" showColStripes=\"0\" showLastColumn=\"1\"/></pivotTableDefinition>",
        esc(name),
        captions.attributes()
    )
}

/// Field Settings of field `f` as the definition has them.
fn field_settings(fd: &Field, f: usize, filters: &[PivotFilter]) -> PivotFieldSettings {
    PivotFieldSettings {
        subtotals: if !fd.subtotals.is_empty() {
            FieldSubtotals::Custom(fd.subtotals.iter().map(|x| PivotFunc::of(*x)).collect())
        } else if fd.default_subtotal {
            FieldSubtotals::Automatic
        } else {
            FieldSubtotals::None
        },
        form: if !fd.outline {
            ReportLayout::Tabular
        } else if !fd.compact {
            ReportLayout::Outline
        } else {
            ReportLayout::Compact
        },
        repeat_labels: fd.fill_down,
        blank_line: fd.insert_blank,
        sort: match fd.sort.as_deref() {
            Some("ascending") => PivotSort::Ascending,
            Some("descending") => PivotSort::Descending,
            _ => PivotSort::Manual,
        },
        sort_by_value: fd.sort_by,
        filter: filters
            .iter()
            .find(|x| x.fld == f)
            .map(|x| PivotFieldFilter {
                kind: x.kind.clone(),
                value_field: x.measure,
                value1: x.v1.clone(),
                value2: x.v2.clone(),
                top: x.top,
            }),
    }
}

/// The range grouping field `f` shows (the finest level, with the levels
/// above it), and the field a grouping field groups.
fn grouping_of(fields: &[CacheField], f: usize) -> (Option<PivotGrouping>, Option<usize>) {
    let Some(g) = fields.get(f).and_then(|c| c.group.as_ref()) else {
        return (None, None);
    };
    let base = g.base.filter(|b| *b != f);
    let Some(GroupKind::Range(r)) = &g.kind else {
        return (None, base);
    };
    if base.is_some() {
        return (None, base);
    }
    let grouping = match r.by {
        By::Numbers => PivotGrouping::Numbers {
            start: r.start,
            end: r.end,
            by: r.interval,
            auto_start: r.auto_start,
            auto_end: r.auto_end,
        },
        By::Date(level) => {
            let mut levels = vec![level];
            let mut par = g.par;
            let mut guard = 0;
            while let Some(p) = par {
                guard += 1;
                let Some(FieldGroup {
                    kind: Some(GroupKind::Range(pr)),
                    par: next,
                    ..
                }) = fields.get(p).and_then(|c| c.group.as_ref())
                else {
                    break;
                };
                if let By::Date(l) = pr.by {
                    levels.push(l);
                }
                par = *next;
                if guard > 10 {
                    break;
                }
            }
            levels.sort();
            PivotGrouping::Dates {
                start: r.start,
                end: r.end,
                auto_start: r.auto_start,
                auto_end: r.auto_end,
                levels,
                days: if level == DateLevel::Days {
                    r.interval.round().max(1.0) as u32
                } else {
                    1
                },
            }
        }
    };
    (Some(grouping), None)
}
