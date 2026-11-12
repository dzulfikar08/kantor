//! Making and changing Excel tables in Kantor: Format as Table, the Table
//! Design options (style, header and total rows, banding, first/last column,
//! filter buttons), total row functions, rename and Convert to Range.
//!
//! The table lives in IronCalc's model (`UserModel::kantor_set_table`, one
//! IronCalc step); cells written with it (header names, total formulas) are
//! folded into the same undo step ([`Workbook::batch`]). Tables made or
//! changed here are listed in the history state's [`crate::edits::Edits`] so
//! a save writes their parts from the model ([`table_xml`]); tables nobody
//! touched are carried from the file as before.

use std::collections::HashSet;

use anyhow::{Result, anyhow, bail};
use ironcalc_base::kantor_tables::{table_area, table_reference, with_auto_filter_ref};
use ironcalc_base::types::{Table, TableColumn, TableStyleInfo};

use crate::table::{TableCellLook, TableInfo};
use crate::{CellRange, MAX_ROWS, Workbook};

/// A total row function, as Excel offers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TotalFunc {
    None,
    Sum,
    Average,
    Count,
    CountNumbers,
    Max,
    Min,
    StdDev,
    Var,
}

impl TotalFunc {
    pub const ALL: [TotalFunc; 9] = [
        TotalFunc::None,
        TotalFunc::Sum,
        TotalFunc::Average,
        TotalFunc::Count,
        TotalFunc::CountNumbers,
        TotalFunc::Max,
        TotalFunc::Min,
        TotalFunc::StdDev,
        TotalFunc::Var,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TotalFunc::None => "None",
            TotalFunc::Sum => "Sum",
            TotalFunc::Average => "Average",
            TotalFunc::Count => "Count",
            TotalFunc::CountNumbers => "Count Numbers",
            TotalFunc::Max => "Max",
            TotalFunc::Min => "Min",
            TotalFunc::StdDev => "StdDev",
            TotalFunc::Var => "Var",
        }
    }

    /// `totalsRowFunction` in the table part.
    pub fn xml(self) -> Option<&'static str> {
        Some(match self {
            TotalFunc::None => return None,
            TotalFunc::Sum => "sum",
            TotalFunc::Average => "average",
            TotalFunc::Count => "count",
            TotalFunc::CountNumbers => "countNums",
            TotalFunc::Max => "max",
            TotalFunc::Min => "min",
            TotalFunc::StdDev => "stdDev",
            TotalFunc::Var => "var",
        })
    }

    pub fn from_xml(s: &str) -> TotalFunc {
        TotalFunc::ALL
            .into_iter()
            .find(|f| f.xml() == Some(s))
            .unwrap_or(TotalFunc::None)
    }

    /// SUBTOTAL's function number (the 100s skip hidden rows).
    pub fn subtotal(self) -> Option<u32> {
        Some(match self {
            TotalFunc::None => return None,
            TotalFunc::Average => 101,
            TotalFunc::CountNumbers => 102,
            TotalFunc::Count => 103,
            TotalFunc::Max => 104,
            TotalFunc::Min => 105,
            TotalFunc::StdDev => 107,
            TotalFunc::Sum => 109,
            TotalFunc::Var => 110,
        })
    }
}

/// The built-in table styles, as the gallery lists them.
pub fn table_style_names() -> Vec<String> {
    let mut v = Vec::new();
    for (family, n) in [("Light", 21), ("Medium", 28), ("Dark", 11)] {
        for i in 1..=n {
            v.push(format!("TableStyle{family}{i}"));
        }
    }
    v
}

/// What Table Design shows and changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableOptions {
    pub style: Option<String>,
    pub header_row: bool,
    pub total_row: bool,
    pub banded_rows: bool,
    pub banded_columns: bool,
    pub first_column: bool,
    pub last_column: bool,
    pub filter_button: bool,
}

/// A table name as Excel allows it: letters, digits, `_` and `.`, starting
/// with a letter, `_` or `\`, not a cell reference, at most 255 characters.
pub fn valid_table_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_alphabetic() || first == '_' || first == '\\')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '.')
        && name.len() <= 255
        && crate::formula::parse_range(name).is_none()
        && !matches!(name.to_ascii_uppercase().as_str(), "R" | "C")
}

fn filter_xml(area: &CellRange, totals: u32) -> String {
    let bottom = area.bottom.saturating_sub(totals).max(area.top);
    format!(
        "<autoFilter ref=\"{}:{}\"/>",
        crate::cell_name(area.top, area.left),
        crate::cell_name(bottom, area.right)
    )
}

impl Workbook {
    fn model_table(&self, name: &str) -> Option<Table> {
        ironcalc_base::kantor_tables::find_table_pub(&self.model.get_model().workbook.tables, name)
            .cloned()
    }

    fn table_info(&self, name: &str) -> Option<(u32, TableInfo)> {
        let t = self.model_table(name)?;
        let sheet = self.sheet_names().iter().position(|n| *n == t.sheet_name)? as u32;
        let info = self
            .tables(sheet)
            .into_iter()
            .find(|i| i.name.eq_ignore_ascii_case(name))?;
        Some((sheet, info))
    }

    /// The range Format as Table proposes: the selection if it is more than
    /// one cell, else the data around the active cell.
    pub fn table_suggestion(
        &self,
        sheet: u32,
        selection: CellRange,
        active: (u32, u32),
    ) -> CellRange {
        if !selection.is_single() && !selection.is_whole_rows() && !selection.is_whole_columns() {
            return selection;
        }
        self.current_region(sheet, active.0, active.1)
    }

    /// A free table name: `Table1`, `Table2`...
    pub fn next_table_name(&self) -> String {
        (1..)
            .map(|n| format!("Table{n}"))
            .find(|n| !self.name_taken(n))
            .expect("a free name")
    }

    fn name_taken(&self, name: &str) -> bool {
        let m = self.model.get_model();
        m.workbook
            .tables
            .keys()
            .any(|k| k.eq_ignore_ascii_case(name))
            || m.workbook
                .defined_names
                .iter()
                .any(|d| d.name.eq_ignore_ascii_case(name))
    }

    /// Why `range` of `sheet` can't become a table, if it can't.
    pub fn table_problem(&self, sheet: u32, range: CellRange) -> Option<String> {
        if range.is_whole_columns() || range.is_whole_rows() {
            return Some("Select the cells of the table, not whole rows or columns".into());
        }
        if range.right - range.left >= 16_000 {
            return Some("Too many columns".into());
        }
        for t in self.tables(sheet) {
            if t.area.intersects(&range) {
                return Some(format!("It overlaps table {}", t.name));
            }
        }
        for p in self.placed_pivots(sheet).iter() {
            let a =
                CellRange::from_corners((p.area.top, p.area.left), (p.area.bottom, p.area.right));
            if a.intersects(&range) {
                return Some("It overlaps a PivotTable".into());
            }
        }
        if let Ok(ws) = self.model.get_model().workbook.worksheet(sheet) {
            for m in &ws.merged_cells {
                let r = m.row.max(1) as u32 - 1;
                let c = m.column.max(1) as u32 - 1;
                let mr = CellRange::from_corners(
                    (r, c),
                    (
                        r + m.height.max(1) as u32 - 1,
                        c + m.width.max(1) as u32 - 1,
                    ),
                );
                if mr.intersects(&range) {
                    return Some("Tables can't contain merged cells".into());
                }
            }
            if ws.kantor_auto_filter.is_some()
                && let Some(f) = self.sheet_filter(sheet)
                && f.area.intersects(&range)
            {
                return Some("It overlaps the sheet's filter: remove the filter first".into());
            }
        }
        None
    }

    /// Format as Table: make `range` of `sheet` a table with `style`. With
    /// `has_headers` its first row names the columns (blank or repeated
    /// names are made unique, as Excel does); without, a header row with
    /// `Column1`, `Column2`... is put above it (in the empty row above, else
    /// a new row). One undo step. Returns the table's name.
    pub fn create_table(
        &mut self,
        sheet: u32,
        range: CellRange,
        has_headers: bool,
        style: &str,
    ) -> Result<String> {
        if let Some(p) = self.table_problem(sheet, range) {
            bail!(p);
        }
        let sheet_name = self
            .sheet_names()
            .get(sheet as usize)
            .cloned()
            .ok_or_else(|| anyhow!("no sheet {sheet}"))?;
        let name = self.next_table_name();
        let width = range.right - range.left + 1;
        self.batch(|wb| {
            let mut area = range;
            let mut writes: Vec<(u32, u32, String)> = Vec::new();
            let mut names: Vec<String> = Vec::new();
            if has_headers {
                for c in range.left..=range.right {
                    names.push(wb.display_value(sheet, range.top, c).trim().to_owned());
                }
            } else {
                let above_free = range.top > 0
                    && (range.left..=range.right).all(|c| !wb.is_filled(sheet, range.top - 1, c))
                    && wb
                        .table_problem(
                            sheet,
                            CellRange::from_corners(
                                (range.top - 1, range.left),
                                (range.top - 1, range.right),
                            ),
                        )
                        .is_none();
                if above_free {
                    area.top -= 1;
                } else {
                    if range.bottom + 1 >= MAX_ROWS {
                        bail!("No room for a header row");
                    }
                    wb.insert_rows(sheet, range.top, 1)?;
                    area.bottom += 1;
                }
                names = vec![String::new(); width as usize];
            }
            // Blank names become ColumnN, repeated ones get a number.
            let mut seen: HashSet<String> = HashSet::new();
            for (i, n) in names.iter_mut().enumerate() {
                let original = n.clone();
                if n.is_empty() {
                    *n = format!("Column{}", i + 1);
                }
                let base = n.clone();
                let mut k = 2;
                while !seen.insert(n.to_lowercase()) {
                    *n = format!("{base}{k}");
                    k += 1;
                }
                if *n != original || !has_headers {
                    writes.push((area.top, area.left + i as u32, n.clone()));
                } else if wb.raw_input_is_number(sheet, area.top, area.left + i as u32) {
                    // Excel keeps headers as text.
                    writes.push((area.top, area.left + i as u32, n.clone()));
                }
            }
            if area.bottom == area.top {
                // Only a header: Excel adds one empty data row.
                if area.bottom + 1 >= MAX_ROWS {
                    bail!("No room for a data row");
                }
                area.bottom += 1;
            }
            let table = Table {
                name: name.clone(),
                display_name: name.clone(),
                sheet_name: sheet_name.clone(),
                reference: table_reference(
                    area.top as i32 + 1,
                    area.left as i32 + 1,
                    area.bottom as i32 + 1,
                    area.right as i32 + 1,
                ),
                totals_row_count: 0,
                header_row_count: 1,
                header_row_dxf_id: None,
                data_dxf_id: None,
                totals_row_dxf_id: None,
                columns: names
                    .iter()
                    .enumerate()
                    .map(|(i, n)| TableColumn {
                        id: i as u32 + 1,
                        name: n.clone(),
                        ..TableColumn::default()
                    })
                    .collect(),
                style_info: TableStyleInfo {
                    name: Some(style.to_owned()),
                    show_first_column: false,
                    show_last_column: false,
                    show_row_stripes: true,
                    show_column_stripes: false,
                },
                has_filters: true,
                kantor_auto_filter: Some(filter_xml(&area, 0)),
            };
            // Header text first (an edit of a table header renames the
            // column), then the table.
            if !writes.is_empty() {
                wb.set_inputs(sheet, &writes)?;
            }
            wb.model
                .kantor_set_table(&name, Some(table))
                .map_err(|e| anyhow!("{e}"))?;
            wb.history.edited();
            wb.touch_table(&name, None);
            Ok(())
        })?;
        Ok(name)
    }

    fn raw_input_is_number(&self, sheet: u32, row: u32, col: u32) -> bool {
        let s = self.input(sheet, row, col);
        !s.is_empty() && s.trim().parse::<f64>().is_ok()
    }

    /// Mark a table as made or changed in Kantor (a save writes its part
    /// from the model). `renamed_from`: its previous name.
    fn touch_table(&mut self, name: &str, renamed_from: Option<&str>) {
        let name = name.to_lowercase();
        let from = renamed_from.map(str::to_lowercase);
        self.own_edit(|e, _| {
            let mut aliases = from
                .as_ref()
                .and_then(|f| e.tables.remove(f))
                .unwrap_or_default();
            if let Some(f) = from {
                aliases.push(f);
            }
            let entry = e.tables.entry(name.clone()).or_default();
            for a in aliases {
                if !entry.contains(&a) && a != name {
                    entry.push(a);
                }
            }
            e.removed_tables.remove(&name);
        });
        // The own edit and the IronCalc steps before it are one undo step
        // (callers run in a batch).
    }

    /// The Table Design options of table `name`.
    pub fn table_options(&self, name: &str) -> Option<TableOptions> {
        let t = self.model_table(name)?;
        Some(TableOptions {
            style: t.style_info.name.clone(),
            header_row: t.header_row_count > 0,
            total_row: t.totals_row_count > 0,
            banded_rows: t.style_info.show_row_stripes,
            banded_columns: t.style_info.show_column_stripes,
            first_column: t.style_info.show_first_column,
            last_column: t.style_info.show_last_column,
            filter_button: t.kantor_auto_filter.is_some(),
        })
    }

    /// The total row function of each column of table `name`.
    pub fn table_totals(&self, name: &str) -> Vec<TotalFunc> {
        self.model_table(name)
            .map(|t| {
                t.columns
                    .iter()
                    .map(|c| {
                        c.totals_row_function
                            .as_deref()
                            .map_or(TotalFunc::None, TotalFunc::from_xml)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Change the Table Design options of table `name` (one undo step).
    /// Turning the total row on uses the empty row below (else inserts
    /// one) and totals the last column; the header row needs the row above
    /// to be empty when it is turned back on.
    pub fn set_table_options(&mut self, name: &str, opts: TableOptions) -> Result<()> {
        let mut t = self
            .model_table(name)
            .ok_or_else(|| anyhow!("no table {name}"))?;
        let (sheet, info) = self
            .table_info(name)
            .ok_or_else(|| anyhow!("no table {name}"))?;
        if self.table_options(name).as_ref() == Some(&opts) {
            return Ok(());
        }
        let name = t.name.clone();
        self.batch(|wb| {
            let mut area = info.area;
            let mut writes: Vec<(u32, u32, String)> = Vec::new();
            let mut clear: Option<CellRange> = None;
            // Total row.
            if opts.total_row && t.totals_row_count == 0 {
                let below = area.bottom + 1;
                if below >= MAX_ROWS {
                    bail!("No room for a total row");
                }
                let free = (area.left..=area.right).all(|c| !wb.is_filled(sheet, below, c))
                    && wb
                        .table_problem(
                            sheet,
                            CellRange::from_corners((below, area.left), (below, area.right)),
                        )
                        .is_none();
                if !free {
                    wb.insert_rows(sheet, below, 1)?;
                }
                area.bottom = below;
                t.totals_row_count = 1;
                let last = t.columns.len() - 1;
                for (i, col) in t.columns.iter_mut().enumerate() {
                    if col.totals_row_function.is_none() && col.totals_row_label.is_none() {
                        if i == 0 && last > 0 {
                            col.totals_row_label = Some("Total".into());
                        } else if i == last {
                            let numeric = wb.column_is_numeric(sheet, &info, i as u32);
                            let f = if numeric {
                                TotalFunc::Sum
                            } else {
                                TotalFunc::Count
                            };
                            col.totals_row_function = f.xml().map(str::to_owned);
                        }
                    }
                }
                for (i, col) in t.columns.iter().enumerate() {
                    let text = total_cell(&name, col);
                    if !text.is_empty() {
                        writes.push((below, area.left + i as u32, text));
                    }
                }
            } else if !opts.total_row && t.totals_row_count > 0 {
                clear = Some(CellRange::from_corners(
                    (area.bottom, area.left),
                    (area.bottom, area.right),
                ));
                area.bottom -= 1;
                t.totals_row_count = 0;
            }
            // Header row.
            if !opts.header_row && t.header_row_count > 0 {
                let header = CellRange::from_corners((area.top, area.left), (area.top, area.right));
                clear = Some(match clear {
                    Some(c) => {
                        wb.clear_range(sheet, c)?;
                        header
                    }
                    None => header,
                });
                area.top += 1;
                t.header_row_count = 0;
                if area.top > area.bottom {
                    bail!("A table needs a data row");
                }
            } else if opts.header_row && t.header_row_count == 0 {
                if area.top == 0
                    || (area.left..=area.right).any(|c| wb.is_filled(sheet, area.top - 1, c))
                {
                    bail!("The row above the table must be empty to show the header row");
                }
                area.top -= 1;
                t.header_row_count = 1;
                for (i, col) in t.columns.iter().enumerate() {
                    writes.push((area.top, area.left + i as u32, col.name.clone()));
                }
            }
            t.reference = table_reference(
                area.top as i32 + 1,
                area.left as i32 + 1,
                area.bottom as i32 + 1,
                area.right as i32 + 1,
            );
            t.style_info = TableStyleInfo {
                name: opts.style.clone(),
                show_first_column: opts.first_column,
                show_last_column: opts.last_column,
                show_row_stripes: opts.banded_rows,
                show_column_stripes: opts.banded_columns,
            };
            t.kantor_auto_filter = if opts.filter_button && t.header_row_count > 0 {
                Some(match t.kantor_auto_filter.take() {
                    Some(x) => with_auto_filter_ref(&x, &filter_ref(&area, t.totals_row_count)),
                    None => filter_xml(&area, t.totals_row_count),
                })
            } else {
                None
            };
            t.has_filters = t.kantor_auto_filter.is_some();
            if let Some(c) = clear {
                wb.clear_range(sheet, c)?;
            }
            wb.model
                .kantor_set_table(&name, Some(t))
                .map_err(|e| anyhow!("{e}"))?;
            wb.history.edited();
            if !writes.is_empty() {
                wb.set_inputs(sheet, &writes)?;
            }
            wb.touch_table(&name, None);
            Ok(())
        })
    }

    fn column_is_numeric(&self, sheet: u32, info: &TableInfo, col: u32) -> bool {
        let (top, bottom) = info.data_rows();
        let c = info.area.left + col;
        let mut numbers = 0;
        let mut others = 0;
        for r in top..=bottom.min(top + 200) {
            let s = self.input(sheet, r, c);
            if s.is_empty() {
                continue;
            }
            if self.raw_number(sheet, r, c).is_some() {
                numbers += 1;
            } else {
                others += 1;
            }
        }
        numbers > 0 && numbers >= others
    }

    /// Set the total row function of column `col` (0-based in the table) of
    /// table `name`; the table must show its total row. One undo step.
    pub fn set_table_total(&mut self, name: &str, col: usize, f: TotalFunc) -> Result<()> {
        let mut t = self
            .model_table(name)
            .ok_or_else(|| anyhow!("no table {name}"))?;
        let (sheet, info) = self
            .table_info(name)
            .ok_or_else(|| anyhow!("no table {name}"))?;
        let row = info
            .totals_row()
            .ok_or_else(|| anyhow!("Turn on the total row first"))?;
        if self.table_totals(name).get(col) == Some(&f) {
            return Ok(());
        }
        let column = t
            .columns
            .get_mut(col)
            .ok_or_else(|| anyhow!("no column {col}"))?;
        column.totals_row_function = f.xml().map(str::to_owned);
        if f != TotalFunc::None {
            column.totals_row_label = None;
        }
        let text = total_cell(&t.name, column);
        let name = t.name.clone();
        self.batch(|wb| {
            wb.model
                .kantor_set_table(&name, Some(t))
                .map_err(|e| anyhow!("{e}"))?;
            wb.history.edited();
            wb.set_inputs(sheet, &[(row, info.area.left + col as u32, text)])?;
            wb.touch_table(&name, None);
            Ok(())
        })
    }

    /// Rename table `name` (structured references in formulas follow). One
    /// undo step.
    pub fn rename_table(&mut self, name: &str, new: &str) -> Result<()> {
        let new = new.trim();
        let t = self
            .model_table(name)
            .ok_or_else(|| anyhow!("no table {name}"))?;
        if t.name == new {
            return Ok(());
        }
        if !valid_table_name(new) {
            bail!("A table name starts with a letter and has no spaces or symbols");
        }
        if !t.name.eq_ignore_ascii_case(new) && self.name_taken(new) {
            bail!("The name {new} is already taken");
        }
        let old = t.name.clone();
        // Formulas that name the table.
        let needle = format!("{}[", old.to_lowercase());
        let mut fixes: Vec<(u32, Vec<CellInput>)> = Vec::new();
        for (i, _) in self.sheet_names().iter().enumerate() {
            let sheet = i as u32;
            let mut v = Vec::new();
            for (r, c) in self.filled_cells(sheet) {
                let s = self.input(sheet, r, c);
                if s.starts_with('=') && s.to_lowercase().contains(&needle) {
                    v.push((r, c, replace_table_name(&s, &old, new)));
                }
            }
            if !v.is_empty() {
                fixes.push((sheet, v));
            }
        }
        let mut renamed = t;
        renamed.name = new.to_owned();
        renamed.display_name = new.to_owned();
        let new = new.to_owned();
        self.batch(|wb| {
            wb.model
                .kantor_set_table(&new, Some(renamed))
                .map_err(|e| anyhow!("{e}"))?;
            wb.history.edited();
            for (sheet, cells) in &fixes {
                wb.set_inputs(*sheet, cells)?;
            }
            wb.model
                .kantor_set_table(&old, None)
                .map_err(|e| anyhow!("{e}"))?;
            wb.history.edited();
            wb.touch_table(&new, Some(&old));
            Ok(())
        })
    }

    /// Convert to Range: the table goes, its cells and their values stay
    /// (formulas with structured references become plain references). One
    /// undo step.
    pub fn convert_table_to_range(&mut self, name: &str) -> Result<()> {
        let t = self
            .model_table(name)
            .ok_or_else(|| anyhow!("no table {name}"))?;
        let (sheet, info) = self
            .table_info(name)
            .ok_or_else(|| anyhow!("no table {name}"))?;
        // Structured references anywhere become A1 references first.
        let mut fixes: Vec<(u32, Vec<CellInput>)> = Vec::new();
        let needle = format!("{}[", t.name.to_lowercase());
        for (i, _) in self.sheet_names().iter().enumerate() {
            let s = i as u32;
            let mut v = Vec::new();
            for (r, c) in self.filled_cells(s) {
                let input = self.input(s, r, c);
                let in_table = s == sheet && info.area.contains(r, c);
                if input.starts_with('=')
                    && (input.to_lowercase().contains(&needle) || (in_table && input.contains('[')))
                    && let Some(plain) = self.plain_formula(s, r, c, &input, &t)
                {
                    v.push((r, c, plain));
                }
            }
            if !v.is_empty() {
                fixes.push((s, v));
            }
        }
        let name = t.name.clone();
        self.batch(|wb| {
            for (s, cells) in &fixes {
                wb.set_inputs(*s, cells)?;
            }
            wb.model
                .kantor_set_table(&name, None)
                .map_err(|e| anyhow!("{e}"))?;
            wb.history.edited();
            let lower = name.to_lowercase();
            wb.own_edit(|e, _| {
                let aliases = e.tables.remove(&lower).unwrap_or_default();
                e.removed_tables.insert(lower.clone());
                e.removed_tables.extend(aliases);
            });
            Ok(())
        })
    }

    /// `input` with the structured references to table `t` written as A1
    /// references (for Convert to Range).
    fn plain_formula(
        &self,
        _sheet: u32,
        row: u32,
        _col: u32,
        input: &str,
        t: &Table,
    ) -> Option<String> {
        let (top, left, bottom, right) = table_area(t)?;
        let (top, left, bottom, right) = (
            top as u32 - 1,
            left as u32 - 1,
            bottom as u32 - 1,
            right as u32 - 1,
        );
        let data_top = top + t.header_row_count;
        let data_bottom = bottom.saturating_sub(t.totals_row_count);
        let col_of = |n: &str| {
            t.columns
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case(n.trim()))
                .map(|i| left + i as u32)
        };
        let abs = |r: u32, c: u32| format!("${}${}", crate::column_name(c), r + 1);
        let rng = |r1: u32, c1: u32, r2: u32, c2: u32| {
            if r1 == r2 && c1 == c2 {
                abs(r1, c1)
            } else {
                format!("{}:{}", abs(r1, c1), abs(r2, c2))
            }
        };
        let mut out = String::new();
        let b = input;
        let mut i = 0;
        let lower = b.to_lowercase();
        let tname = t.name.to_lowercase();
        while i < b.len() {
            let rest = &lower[i..];
            let (skip, starts) = if rest.starts_with(&format!("{tname}[")) {
                (t.name.len(), true)
            } else if rest.starts_with('[') {
                (0, true)
            } else {
                (0, false)
            };
            let prev_ok = i == 0
                || !b[..i]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.');
            if starts && (skip > 0 && prev_ok || skip == 0) {
                let open = i + skip;
                // The matching bracket.
                let mut depth = 0;
                let mut end = None;
                for (k, ch) in b[open..].char_indices() {
                    match ch {
                        '[' => depth += 1,
                        ']' => {
                            depth -= 1;
                            if depth == 0 {
                                end = Some(open + k);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let end = end?;
                let inner = &b[open + 1..end];
                let spec = inner.replace(['[', ']'], "");
                let parts: Vec<&str> = spec.split(',').map(str::trim).collect();
                let mut rows = (data_top, data_bottom);
                let mut cols = (left, right);
                for p in parts {
                    let pl = p.to_lowercase();
                    if pl.is_empty() {
                        continue;
                    }
                    match pl.as_str() {
                        "#all" => rows = (top, bottom),
                        "#data" => rows = (data_top, data_bottom),
                        "#headers" => rows = (top, top),
                        "#totals" => rows = (bottom, bottom),
                        "#this row" | "@" => rows = (row, row),
                        _ => {
                            let (this_row, name) = match p.strip_prefix('@') {
                                Some(n) => (true, n),
                                None => (false, p),
                            };
                            if this_row {
                                rows = (row, row);
                            }
                            if name.is_empty() {
                                continue;
                            }
                            if let Some((a, z)) = name.split_once(':') {
                                cols = (col_of(a)?, col_of(z)?);
                            } else {
                                let c = col_of(name)?;
                                cols = (c, c);
                            }
                        }
                    }
                }
                out.push_str(&rng(rows.0, cols.0, rows.1, cols.1));
                i = end + 1;
                continue;
            }
            let ch = b[i..].chars().next()?;
            out.push(ch);
            i += ch.len_utf8();
        }
        Some(out)
    }

    /// How built-in style `name` draws a small 4 × 5 table (header row,
    /// four data rows), for a gallery.
    pub fn table_style_preview(&self, name: &str) -> Option<Vec<Vec<TableCellLook>>> {
        let style = self.table_style(name)?;
        let t = TableInfo {
            name: String::new(),
            area: CellRange::from_corners((0, 0), (4, 3)),
            header_rows: 1,
            totals_rows: 0,
            columns: vec![String::new(); 4],
            style: Some(name.to_owned()),
            first_column: false,
            last_column: false,
            row_stripes: true,
            column_stripes: false,
            filter_buttons: false,
        };
        Some(
            (0..5)
                .map(|r| (0..4).map(|c| style.look_pub(&t, r, c)).collect())
                .collect(),
        )
    }

    /// Patches a save writes for tables made or changed in Kantor.
    pub(crate) fn table_patches(&self, out: &mut crate::preserve::Patches) {
        let edits = self.edits();
        for (lower, aliases) in &edits.tables {
            let Some(t) = self.model_table(lower) else {
                continue;
            };
            let Some(sheet) = self.sheet_names().iter().position(|n| *n == t.sheet_name) else {
                continue;
            };
            let mut names = vec![t.name.clone()];
            names.extend(aliases.iter().cloned());
            out.tables.push(crate::preserve::TableSave {
                sheet_id: self.sheet_id(sheet as u32),
                names,
                table: t,
            });
        }
        out.removed_tables
            .extend(edits.removed_tables.iter().cloned());
        for aliases in edits.tables.values() {
            out.removed_tables.extend(aliases.iter().cloned());
        }
        out.touched_tables.extend(edits.tables.keys().cloned());
        // Tables changed earlier in the session (then undone, or saved):
        // written from the model, or not carried if the model has none now.
        for name in &self.touched.tables {
            if edits.tables.contains_key(name) || out.removed_tables.contains(name) {
                continue;
            }
            match self.model_table(name) {
                Some(t) => {
                    if let Some(sheet) = self.sheet_names().iter().position(|n| *n == t.sheet_name)
                    {
                        out.touched_tables.insert(name.clone());
                        out.tables.push(crate::preserve::TableSave {
                            sheet_id: self.sheet_id(sheet as u32),
                            names: vec![t.name.clone()],
                            table: t,
                        });
                    }
                }
                None => {
                    out.removed_tables.insert(name.clone());
                }
            }
        }
    }
}

fn filter_ref(area: &CellRange, totals: u32) -> String {
    let bottom = area.bottom.saturating_sub(totals).max(area.top);
    format!(
        "{}:{}",
        crate::cell_name(area.top, area.left),
        crate::cell_name(bottom, area.right)
    )
}

/// What the total row cell of `col` holds: its label, or its SUBTOTAL.
fn total_cell(table: &str, col: &TableColumn) -> String {
    let f = col
        .totals_row_function
        .as_deref()
        .map_or(TotalFunc::None, TotalFunc::from_xml);
    match f.subtotal() {
        Some(code) => format!("=SUBTOTAL({code},{table}[{}])", escape_column(&col.name)),
        None => col.totals_row_label.clone().unwrap_or_default(),
    }
}

/// A column name inside a structured reference (`'` escapes `[ ] # '`).
fn escape_column(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if matches!(c, '[' | ']' | '#' | '\'') {
            out.push('\'');
        }
        out.push(c);
    }
    if name.contains([' ', ',', ':', '@']) || out.len() != name.len() {
        format!("[{out}]")
    } else {
        out
    }
}

/// `formula` with table name `old` (before a `[`) replaced by `new`.
fn replace_table_name(formula: &str, old: &str, new: &str) -> String {
    let lower = formula.to_lowercase();
    let needle = format!("{}[", old.to_lowercase());
    let mut out = String::new();
    let mut i = 0;
    while let Some(k) = lower[i..].find(&needle) {
        let at = i + k;
        let prev_ok = at == 0
            || !formula[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.');
        out.push_str(&formula[i..at]);
        if prev_ok {
            out.push_str(new);
        } else {
            out.push_str(&formula[at..at + old.len()]);
        }
        i = at + old.len();
    }
    out.push_str(&formula[i..]);
    out
}

/// `(row, column, input)`.
type CellInput = (u32, u32, String);

const NS_MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The table part of `t` with id `id`. `existing`: the part the package has
/// for it (its unmodelled markup, e.g. calculated column formulas and
/// column dxfs, is kept where the columns are the same).
pub(crate) fn table_xml(t: &Table, id: u32, existing: Option<&str>) -> String {
    let doc = existing.and_then(|x| roxmltree::Document::parse(x).ok());
    let old_cols: Vec<roxmltree::Node> = doc
        .as_ref()
        .map(|d| {
            d.descendants()
                .filter(|n| n.is_element() && n.tag_name().name() == "tableColumn")
                .collect()
        })
        .unwrap_or_default();
    let mut x = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n");
    x.push_str(&format!(
        "<table xmlns=\"{NS_MAIN}\" id=\"{id}\" name=\"{}\" displayName=\"{}\" ref=\"{}\"",
        esc(&t.name),
        esc(&t.name),
        esc(&t.reference)
    ));
    if t.header_row_count == 0 {
        x.push_str(" headerRowCount=\"0\"");
    }
    if t.totals_row_count > 0 {
        x.push_str(&format!(" totalsRowCount=\"{}\"", t.totals_row_count));
    } else {
        x.push_str(" totalsRowShown=\"0\"");
    }
    for (attr, v) in [
        ("headerRowDxfId", t.header_row_dxf_id),
        ("dataDxfId", t.data_dxf_id),
        ("totalsRowDxfId", t.totals_row_dxf_id),
    ] {
        if let Some(v) = v {
            x.push_str(&format!(" {attr}=\"{v}\""));
        }
    }
    x.push('>');
    if let Some(f) = &t.kantor_auto_filter
        && t.header_row_count > 0
    {
        let area = table_area(t);
        let r = area.map(|(top, left, bottom, right)| {
            let bottom = (bottom as u32)
                .saturating_sub(t.totals_row_count)
                .max(top as u32);
            format!(
                "{}:{}",
                crate::cell_name(top as u32 - 1, left as u32 - 1),
                crate::cell_name(bottom - 1, right as u32 - 1)
            )
        });
        match r {
            Some(r) => x.push_str(&with_auto_filter_ref(f, &r)),
            None => x.push_str(f),
        }
    }
    x.push_str(&format!("<tableColumns count=\"{}\">", t.columns.len()));
    for c in &t.columns {
        x.push_str(&format!(
            "<tableColumn id=\"{}\" name=\"{}\"",
            c.id,
            esc(&c.name)
        ));
        if let Some(l) = &c.totals_row_label {
            x.push_str(&format!(" totalsRowLabel=\"{}\"", esc(l)));
        }
        if let Some(f) = &c.totals_row_function {
            x.push_str(&format!(" totalsRowFunction=\"{}\"", esc(f)));
        }
        for (attr, v) in [
            ("headerRowDxfId", c.header_row_dxf_id),
            ("dataDxfId", c.data_dxf_id),
            ("totalsRowDxfId", c.totals_row_dxf_id),
        ] {
            if let Some(v) = v {
                x.push_str(&format!(" {attr}=\"{v}\""));
            }
        }
        // Children of the same column in the existing part (calculated
        // column formulas...), kept unless they belong to a totals function
        // that changed.
        let kept: String = old_cols
            .iter()
            .find(|o| o.attribute("id").and_then(|v| v.parse::<u32>().ok()) == Some(c.id))
            .map(|o| {
                o.children()
                    .filter(|ch| ch.is_element())
                    .filter(|ch| ch.tag_name().name() != "totalsRowFormula")
                    .map(|ch| &existing.unwrap_or_default()[ch.range()])
                    .collect()
            })
            .unwrap_or_default();
        if kept.is_empty() {
            x.push_str("/>");
        } else {
            x.push('>');
            x.push_str(&kept);
            x.push_str("</tableColumn>");
        }
    }
    x.push_str("</tableColumns>");
    if let Some(name) = &t.style_info.name {
        let b = |v: bool| if v { "1" } else { "0" };
        x.push_str(&format!(
            "<tableStyleInfo name=\"{}\" showFirstColumn=\"{}\" showLastColumn=\"{}\" showRowStripes=\"{}\" showColumnStripes=\"{}\"/>",
            esc(name),
            b(t.style_info.show_first_column),
            b(t.style_info.show_last_column),
            b(t.style_info.show_row_stripes),
            b(t.style_info.show_column_stripes)
        ));
    }
    x.push_str("</table>");
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_escapes() {
        assert!(valid_table_name("Sales_2026"));
        assert!(!valid_table_name("A1"));
        assert!(!valid_table_name("2x"));
        assert!(!valid_table_name("My Table"));
        assert_eq!(escape_column("Units"), "Units");
        assert_eq!(escape_column("Unit Price"), "[Unit Price]");
        assert_eq!(escape_column("#Qty"), "['#Qty]");
        assert_eq!(
            replace_table_name("=SUM(Table1[Units])+MyTable1[x]", "Table1", "Sales"),
            "=SUM(Sales[Units])+MyTable1[x]"
        );
        assert_eq!(TotalFunc::from_xml("countNums"), TotalFunc::CountNumbers);
        assert_eq!(TotalFunc::Sum.subtotal(), Some(109));
    }
}
