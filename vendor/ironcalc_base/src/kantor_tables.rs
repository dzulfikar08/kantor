//! KANTOR PATCH (tables): structured references and table geometry.
//!
//! Upstream turned a structured reference into a fixed range when the
//! formula was parsed: the formula was then saved as that range
//! (`Sales[Units]` became `$C$2:$C$9`), `[#This Row]` became an absolute
//! row (one formula per row), and the reference did not follow the table
//! when it grew. Here the parser keeps a [`TableRef`] node; it is resolved
//! against the table's current range each time it is evaluated, and written
//! back as structured text (`[#This Row]` in files, `@` on screen, as Excel
//! does). See KANTOR_PATCHES.md §13.

use std::collections::{HashMap, HashSet};

use crate::{
    expressions::{
        parser::{parse_range, Node, TableRef},
        token::{Error, TableSpecifier},
        types::CellReferenceIndex,
    },
    model::Model,
    types::{Cell, Row, Table},
    user_model::{history::Diff, UserModel},
};

/// Whether a reference is one cell wherever it is evaluated (`T[@Col]`).
pub(crate) fn is_single_cell(t: &TableRef) -> bool {
    matches!(t.specifier, Some(TableSpecifier::ThisRow))
        && matches!(&t.columns, Some((a, b)) if a == b)
}

/// Characters that make Excel put a column name in its own brackets.
fn needs_brackets(name: &str) -> bool {
    name.is_empty()
        || name.chars().any(|c| !(c.is_alphanumeric() || c == '_'))
}

/// A column name with `[`, `]`, `#` and `'` escaped by `'`.
fn escape(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if matches!(c, '[' | ']' | '#' | '\'') {
            out.push('\'');
        }
        out.push(c);
    }
    out
}

fn bracketed(name: &str) -> String {
    format!("[{}]", escape(name))
}

fn specifier_text(s: &TableSpecifier) -> &'static str {
    match s {
        TableSpecifier::All => "[#All]",
        TableSpecifier::Data => "[#Data]",
        TableSpecifier::Headers => "[#Headers]",
        TableSpecifier::Totals => "[#Totals]",
        TableSpecifier::ThisRow => "[#This Row]",
        TableSpecifier::HeadersData => "[#Headers],[#Data]",
        TableSpecifier::DataTotals => "[#Data],[#Totals]",
    }
}

/// The text of a structured reference: as Excel stores it in files
/// (`export`: `Sales[[#This Row],[Units]]`) or shows it (`Sales[@Units]`).
pub(crate) fn table_ref_text(t: &TableRef, export: bool) -> String {
    let table = &t.table;
    let columns = t.columns.as_ref().map(|(a, b)| {
        if a == b {
            (bracketed(a), needs_brackets(a), escape(a))
        } else {
            (format!("{}:{}", bracketed(a), bracketed(b)), true, String::new())
        }
    });
    match (&t.specifier, columns) {
        (None, None) => format!("{table}[]"),
        (None, Some((text, brackets, plain))) => {
            if brackets {
                format!("{table}[{text}]")
            } else {
                format!("{table}[{plain}]")
            }
        }
        (Some(TableSpecifier::ThisRow), None) if !export => format!("{table}[@]"),
        (Some(s), None) => {
            let s = specifier_text(s);
            if s.matches('[').count() == 1 {
                format!("{table}{s}")
            } else {
                format!("{table}[{s}]")
            }
        }
        (Some(TableSpecifier::ThisRow), Some((text, brackets, plain))) if !export => {
            if brackets {
                format!("{table}[@{text}]")
            } else {
                format!("{table}[@{plain}]")
            }
        }
        (Some(s), Some((text, _, _))) => format!("{table}[{},{text}]", specifier_text(s)),
    }
}

/// The table called `name` (any case).
pub(crate) fn find_table<'t>(
    tables: &'t HashMap<String, Table>,
    name: &str,
) -> Option<&'t Table> {
    tables.get(name).or_else(|| {
        let lower = name.to_lowercase();
        tables.values().find(|t| t.name.to_lowercase() == lower)
    })
}

/// The table called `name` (any case).
pub fn find_table_pub<'t>(tables: &'t HashMap<String, Table>, name: &str) -> Option<&'t Table> {
    find_table(tables, name)
}

/// A table's area: `(top, left, bottom, right)`, 1-based.
pub fn table_area(table: &Table) -> Option<(i32, i32, i32, i32)> {
    let (c1, r1, c2, r2) = parse_range(&table.reference).ok()?;
    Some((r1, c1, r2, c2))
}

impl Model<'_> {
    /// The range a structured reference stands for when evaluated in `cell`:
    /// a reference, a range, or an error node.
    pub(crate) fn resolve_table_ref(&self, t: &TableRef, cell: CellReferenceIndex) -> Node {
        let Some(table) = find_table(&self.workbook.tables, &t.table) else {
            return Node::ErrorKind(Error::REF);
        };
        let Some(sheet_index) = self.get_sheet_index_by_name(&table.sheet_name) else {
            return Node::ErrorKind(Error::REF);
        };
        let Some((top, left, bottom, right)) = table_area(table) else {
            return Node::ErrorKind(Error::REF);
        };
        let headers = table.header_row_count as i32;
        let totals = table.totals_row_count as i32;
        let data_top = top + headers;
        let data_bottom = bottom - totals;
        let (row1, row2) = match t.specifier {
            None | Some(TableSpecifier::Data) => (data_top, data_bottom),
            Some(TableSpecifier::All) => (top, bottom),
            Some(TableSpecifier::Headers) if headers > 0 => (top, top),
            Some(TableSpecifier::Totals) if totals > 0 => (bottom, bottom),
            Some(TableSpecifier::Headers) | Some(TableSpecifier::Totals) => {
                return Node::ErrorKind(Error::REF)
            }
            Some(TableSpecifier::HeadersData) => (top, data_bottom),
            Some(TableSpecifier::DataTotals) => (data_top, bottom),
            Some(TableSpecifier::ThisRow) => {
                // Excel: #VALUE! outside the table's rows.
                if cell.sheet != sheet_index || cell.row < data_top || cell.row > data_bottom {
                    return Node::ErrorKind(Error::VALUE);
                }
                (cell.row, cell.row)
            }
        };
        if row2 < row1 {
            return Node::ErrorKind(Error::REF);
        }
        let (col1, col2) = match &t.columns {
            None => (left, right),
            Some((a, b)) => {
                let index = |name: &str| {
                    let lower = name.to_lowercase();
                    table
                        .columns
                        .iter()
                        .position(|c| c.name.to_lowercase() == lower)
                        .map(|i| left + i as i32)
                };
                match (index(a), index(b)) {
                    (Some(x), Some(y)) => (x.min(y), x.max(y)),
                    _ => return Node::ErrorKind(Error::REF),
                }
            }
        };
        if row1 == row2 && col1 == col2 {
            return Node::ReferenceKind {
                sheet_name: None,
                sheet_index,
                absolute_row: true,
                absolute_column: true,
                row: row1,
                column: col1,
            };
        }
        Node::RangeKind {
            sheet_name: None,
            sheet_index,
            absolute_row1: true,
            absolute_column1: true,
            row1,
            column1: col1,
            absolute_row2: true,
            absolute_column2: true,
            row2,
            column2: col2,
        }
    }

    /// `args` with structured references resolved, for functions that look
    /// at their argument nodes (ROW, SUBTOTAL...). Only top-level arguments:
    /// deeper ones are resolved when evaluated. Bare table names stay (SHEET
    /// looks at them).
    pub(crate) fn resolve_table_args(
        &self,
        args: &[Node],
        cell: CellReferenceIndex,
    ) -> Option<Vec<Node>> {
        if !args.iter().any(|a| matches!(a, Node::TableRefKind(_))) {
            return None;
        }
        Some(
            args.iter()
                .map(|a| match a {
                    Node::TableRefKind(t) => self.resolve_table_ref(t, cell),
                    other => other.clone(),
                })
                .collect(),
        )
    }
}

/// The order that undoes `order` (see `Diff::KantorPermuteRows`).
pub(crate) fn inverse_order(top: i32, order: &[i32]) -> Vec<i32> {
    let mut inverse = vec![0; order.len()];
    for (i, &src) in order.iter().enumerate() {
        inverse[(src - top) as usize] = top + i as i32;
    }
    inverse
}

/// `area` after rows (or columns) `at..at+delta` were inserted (`delta > 0`)
/// or `at..at-delta` deleted: `None` if nothing is left of it. An insert
/// inside the area (not above its first line) grows it, as Excel's tables do.
fn displace_span(first: i32, last: i32, at: i32, delta: i32) -> Option<(i32, i32)> {
    if delta > 0 {
        if at <= first {
            Some((first + delta, last + delta))
        } else if at <= last {
            Some((first, last + delta))
        } else {
            Some((first, last))
        }
    } else {
        let count = -delta;
        let end = at + count - 1;
        let map_first = if first < at {
            first
        } else if first > end {
            first - count
        } else {
            at
        };
        let map_last = if last < at {
            last
        } else if last > end {
            last - count
        } else {
            at - 1
        };
        (map_last >= map_first).then_some((map_first, map_last))
    }
}

fn area_text(top: i32, left: i32, bottom: i32, right: i32) -> String {
    use crate::expressions::utils::number_to_column;
    format!(
        "{}{top}:{}{bottom}",
        number_to_column(left).unwrap_or_default(),
        number_to_column(right).unwrap_or_default()
    )
}

/// A table's area as a reference (`A1:D10`).
pub fn table_reference(top: i32, left: i32, bottom: i32, right: i32) -> String {
    area_text(top, left, bottom, right)
}

impl Model<'_> {
    /// Put `table` under `name` (or remove it), and let the parser know.
    pub(crate) fn kantor_put_table(&mut self, name: &str, table: Option<Table>) {
        self.workbook.tables.remove(name);
        if let Some(t) = table {
            self.workbook.tables.insert(t.name.clone(), t);
        }
        self.parser.kantor_set_tables(self.workbook.tables.clone());
    }

    /// Tables follow inserted and deleted rows and columns: an insert inside
    /// one grows it (new columns get free names `Column1`...), a delete
    /// shrinks it; a table whose header row or every cell goes is removed.
    pub(crate) fn displace_tables(&mut self, displace: &crate::expressions::parser::stringify::DisplaceData) {
        use crate::expressions::parser::stringify::DisplaceData;
        let (sheet, at, delta, rows) = match *displace {
            DisplaceData::Row { sheet, row, delta } => (sheet, row, delta, true),
            DisplaceData::Column {
                sheet,
                column,
                delta,
            } => (sheet, column, delta, false),
            _ => return,
        };
        let Ok(sheet_name) = self.workbook.worksheet(sheet).map(|w| w.name.clone()) else {
            return;
        };
        let names: Vec<String> = self
            .workbook
            .tables
            .values()
            .filter(|t| t.sheet_name == sheet_name)
            .map(|t| t.name.clone())
            .collect();
        let mut changed = false;
        for name in names {
            let Some(table) = self.workbook.tables.get(&name).cloned() else {
                continue;
            };
            let Some((top, left, bottom, right)) = table_area(&table) else {
                continue;
            };
            let mut t = table.clone();
            let new = if rows {
                let header_gone =
                    delta < 0 && t.header_row_count > 0 && top >= at && top < at - delta;
                match displace_span(top, bottom, at, delta) {
                    Some((nt, nb)) if !header_gone => {
                        // Keep at least one data row.
                        let nb =
                            nb.max(nt + t.header_row_count as i32 + t.totals_row_count as i32);
                        t.reference = area_text(nt, left, nb, right);
                        Some(t)
                    }
                    _ => None,
                }
            } else {
                match displace_span(left, right, at, delta) {
                    Some((nl, nr)) => {
                        if delta > 0 && at > left && at <= right {
                            let pos = (at - left) as usize;
                            let mut next_id = t.columns.iter().map(|c| c.id).max().unwrap_or(0) + 1;
                            for k in 0..delta {
                                let mut n = 1;
                                let lower: Vec<String> =
                                    t.columns.iter().map(|c| c.name.to_lowercase()).collect();
                                while lower.contains(&format!("column{n}")) {
                                    n += 1;
                                }
                                t.columns.insert(
                                    pos + k as usize,
                                    crate::types::TableColumn {
                                        id: next_id,
                                        name: format!("Column{n}"),
                                        ..Default::default()
                                    },
                                );
                                next_id += 1;
                            }
                        } else if delta < 0 {
                            let end = at - delta - 1;
                            let mut i = 0;
                            t.columns.retain(|_| {
                                let col = left + i;
                                i += 1;
                                col < at || col > end
                            });
                        }
                        t.reference = area_text(top, nl, bottom, nr);
                        Some(t)
                    }
                    None => None,
                }
            };
            if new.as_ref() != Some(&table) {
                changed = true;
                self.workbook.tables.remove(&name);
                if let Some(t) = new {
                    self.workbook.tables.insert(name, t);
                }
            }
        }
        if changed {
            self.parser.kantor_set_tables(self.workbook.tables.clone());
        }
        // The sheet's AutoFilter range, like a table without header rules.
        let Ok(ws) = self.workbook.worksheet_mut(sheet) else {
            return;
        };
        if let Some(xml) = ws.kantor_auto_filter.clone() {
            if let Some((c1, r1, c2, r2)) = auto_filter_ref(&xml).and_then(|r| parse_range(r).ok()) {
                let moved = if rows {
                    displace_span(r1, r2, at, delta).map(|(a, b)| (a, c1, b, c2))
                } else {
                    displace_span(c1, c2, at, delta).map(|(a, b)| (r1, a, r2, b))
                };
                ws.kantor_auto_filter = moved
                    .map(|(t, l, b, r)| with_auto_filter_ref(&xml, &area_text(t, l, b, r)));
            }
        }
    }

    /// Replace the entries of several rows (`None` removes one), looking the
    /// sheet's rows up once.
    pub(crate) fn kantor_set_row_entries(
        &mut self,
        sheet: u32,
        entries: &[(i32, Option<Row>)],
    ) -> Result<(), String> {
        let rows = &mut self.workbook.worksheet_mut(sheet)?.rows;
        let mut index: HashMap<i32, usize> =
            rows.iter().enumerate().map(|(i, r)| (r.r, i)).collect();
        let mut removed: HashSet<usize> = HashSet::new();
        for (r, entry) in entries {
            match (index.get(r).copied(), entry) {
                (Some(i), Some(e)) => {
                    rows[i] = e.clone();
                    removed.remove(&i);
                }
                (Some(i), None) => {
                    removed.insert(i);
                }
                (None, Some(e)) => {
                    index.insert(*r, rows.len());
                    rows.push(e.clone());
                }
                (None, None) => {}
            }
        }
        if !removed.is_empty() {
            let mut i = 0;
            rows.retain(|_| {
                let keep = !removed.contains(&i);
                i += 1;
                keep
            });
        }
        Ok(())
    }

    /// Move the cells (columns `left..=right`) of rows `top..top+order.len()`:
    /// row `top + i` gets what row `order[i]` had. Formulas move as they are
    /// (their references are relative to the cell, so they follow the row, as
    /// in Excel's sort); hyperlinks move with their cells.
    pub(crate) fn kantor_permute_rows(
        &mut self,
        sheet: u32,
        top: i32,
        left: i32,
        right: i32,
        order: &[i32],
    ) -> Result<(), String> {
        let n = order.len() as i32;
        let ws = self.workbook.worksheet_mut(sheet)?;
        let data = &mut ws.sheet_data;
        // KANTOR PATCH (fast sort): rows with nothing outside the range move
        // whole (§18).
        let mut taken: Vec<Vec<(i32, Cell)>> = Vec::with_capacity(order.len());
        let whole_rows = data.kantor_permute_whole_rows(top, left, right, order);
        // Otherwise take every row's cells out.
        for r in (top..top + n).filter(|_| !whole_rows) {
            let cols = data.columns_in_row(r);
            let whole = cols.first().is_none_or(|&c| c >= left)
                && cols.last().is_none_or(|&c| c <= right);
            if whole {
                let cells: Vec<(i32, Cell)> = data
                    .cells_in_row(r)
                    .into_iter()
                    .map(|(c, cell)| (c, cell.into_owned()))
                    .collect();
                data.remove_row(r);
                taken.push(cells);
            } else {
                let mut cells = Vec::new();
                for c in cols.into_iter().filter(|c| (left..=right).contains(c)) {
                    if let Some(cell) = data.remove_cell(r, c) {
                        cells.push((c, cell));
                    }
                }
                taken.push(cells);
            }
        }
        for (i, &src) in order.iter().enumerate().filter(|_| !whole_rows) {
            let dest = top + i as i32;
            let cells = std::mem::take(&mut taken[(src - top) as usize]);
            if cells.is_empty() {
                continue;
            }
            if data.columns_in_row(dest).is_empty() {
                data.set_row(dest, cells);
            } else {
                for (c, cell) in cells {
                    data.set_cell(dest, c, cell);
                }
            }
        }
        // Hyperlinks.
        let moved: Vec<((i32, i32), crate::types::Link)> = ws
            .links
            .keys()
            .filter(|(r, c)| (top..top + n).contains(r) && (left..=right).contains(c))
            .copied()
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|k| ws.links.remove(&k).map(|l| (k, l)))
            .collect();
        if !moved.is_empty() {
            let mut dest_of = HashMap::new();
            for (i, &src) in order.iter().enumerate() {
                dest_of.insert(src, top + i as i32);
            }
            for ((r, c), link) in moved {
                let d = dest_of.get(&r).copied().unwrap_or(r);
                ws.links.insert((d, c), link);
            }
        }
        Ok(())
    }

    /// Whether a hidden row `row` (at `column`) of `sheet` is filtered out:
    /// it lies under the sheet's AutoFilter and the filter is on (tables say
    /// so with `has_filters`, as upstream).
    pub(crate) fn kantor_row_filtered(&self, sheet: u32, row: i32, column: i32) -> bool {
        let Ok(ws) = self.workbook.worksheet(sheet) else {
            return false;
        };
        let Some(xml) = ws.kantor_auto_filter.as_deref() else {
            return false;
        };
        if !xml.contains("<filterColumn") {
            return false;
        }
        match auto_filter_ref(xml).and_then(|r| parse_range(r).ok()) {
            Some((c1, r1, c2, r2)) => (r1..=r2).contains(&row) && (c1..=c2).contains(&column),
            None => false,
        }
    }
}

/// The `ref` of an `<autoFilter>` element.
pub fn auto_filter_ref(xml: &str) -> Option<&str> {
    let head = &xml[..xml.find('>')?];
    let at = head.find(" ref=\"")? + 6;
    let len = head[at..].find('"')?;
    Some(&head[at..at + len])
}

/// The `<autoFilter>` element with another `ref`.
pub fn with_auto_filter_ref(xml: &str, reference: &str) -> String {
    match auto_filter_ref(xml) {
        Some(old) => {
            let at = old.as_ptr() as usize - xml.as_ptr() as usize;
            format!("{}{reference}{}", &xml[..at], &xml[at + old.len()..])
        }
        None => xml.to_string(),
    }
}

impl UserModel<'_> {
    /// Set (or remove) the sheet's `<autoFilter>` element as one undo step.
    pub fn kantor_set_sheet_filter(
        &mut self,
        sheet: u32,
        filter: Option<String>,
    ) -> Result<(), String> {
        let old = self.model.workbook.worksheet(sheet)?.kantor_auto_filter.clone();
        if old == filter {
            return Ok(());
        }
        self.model.workbook.worksheet_mut(sheet)?.kantor_auto_filter = filter.clone();
        self.push_diff_list(vec![Diff::KantorSetSheetFilter {
            sheet,
            old_value: old,
            new_value: filter,
        }]);
        self.evaluate_if_not_paused();
        Ok(())
    }

    /// Create, change (same `name`) or remove (`None`) a table, as one undo
    /// step.
    pub fn kantor_set_table(&mut self, name: &str, table: Option<Table>) -> Result<(), String> {
        let old = self.model.workbook.tables.get(name).cloned();
        if let Some(t) = &table {
            self.model.workbook.worksheet(
                self.model
                    .get_sheet_index_by_name(&t.sheet_name)
                    .ok_or_else(|| format!("No sheet {}", t.sheet_name))?,
            )?;
            table_area(t).ok_or_else(|| format!("Invalid table range {}", t.reference))?;
        }
        if old == table {
            return Ok(());
        }
        self.model.kantor_put_table(name, table.clone());
        self.push_diff_list(vec![Diff::KantorSetTable {
            name: name.to_string(),
            old_value: old.map(Box::new),
            new_value: table.map(Box::new),
        }]);
        self.evaluate_if_not_paused();
        Ok(())
    }

    /// KANTOR PATCH (frozen panes): set the frozen rows and columns of
    /// `sheet` without an undo step, as Excel's Freeze Panes is not undone.
    pub fn kantor_set_frozen(&mut self, sheet: u32, rows: i32, cols: i32) -> Result<(), String> {
        let old_rows = self.model.get_frozen_rows_count(sheet)?;
        self.model.set_frozen_rows(sheet, rows)?;
        if let Err(e) = self.model.set_frozen_columns(sheet, cols) {
            let _ = self.model.set_frozen_rows(sheet, old_rows);
            return Err(e);
        }
        Ok(())
    }

    /// KANTOR PATCH (hide columns): hide (`true`) or show columns
    /// (1-based) as one undo step. Widths are kept, so a shown column comes
    /// back as wide as it was. Returns whether anything changed.
    pub fn kantor_set_columns_hidden(
        &mut self,
        sheet: u32,
        changes: &[(i32, bool)],
    ) -> Result<bool, String> {
        let mut diffs = Vec::new();
        for &(column, hide) in changes {
            let old = self.model.is_column_hidden(sheet, column)?;
            if old == hide {
                continue;
            }
            if let Err(e) = self.model.set_column_hidden(sheet, column, hide) {
                for d in diffs.iter().rev() {
                    if let Diff::SetColumnHidden {
                        column, old_value, ..
                    } = d
                    {
                        let _ = self.model.set_column_hidden(sheet, *column, *old_value);
                    }
                }
                return Err(e);
            }
            diffs.push(Diff::SetColumnHidden {
                sheet,
                column,
                new_value: hide,
                old_value: old,
            });
        }
        if diffs.is_empty() {
            return Ok(false);
        }
        self.push_diff_list(diffs);
        Ok(true)
    }

    /// Hide or show rows (1-based) as one undo step, e.g. for a filter.
    /// Returns whether any row changed.
    pub fn kantor_set_rows_hidden(
        &mut self,
        sheet: u32,
        changes: &[(i32, bool)],
    ) -> Result<bool, String> {
        let ws = self.model.workbook.worksheet(sheet)?;
        let default_height = ws.default_row_height.unwrap_or(15.0);
        let index: HashMap<i32, &Row> = ws.rows.iter().map(|r| (r.r, r)).collect();
        let mut old_value = Vec::new();
        let mut new_value = Vec::new();
        for &(r, hide) in changes {
            if !(1..=crate::constants::LAST_ROW).contains(&r) {
                return Err(format!("Row {r} is not valid."));
            }
            let old = index.get(&r).map(|e| (*e).clone());
            let new = match (&old, hide) {
                (Some(e), _) if e.hidden == hide => continue,
                (None, false) => continue,
                (Some(e), true) => Some(Row {
                    hidden: true,
                    ..e.clone()
                }),
                (None, true) => Some(Row {
                    r,
                    height: default_height,
                    custom_format: false,
                    custom_height: false,
                    s: 0,
                    hidden: true,
                    outline_level: 0,
                    collapsed: false,
                }),
                (Some(e), false) => {
                    let shown = Row {
                        hidden: false,
                        ..e.clone()
                    };
                    let bare = !shown.custom_height
                        && !shown.custom_format
                        && shown.s == 0
                        && shown.outline_level == 0
                        && !shown.collapsed;
                    (!bare).then_some(shown)
                }
            };
            old_value.push((r, old));
            new_value.push((r, new));
        }
        if new_value.is_empty() {
            return Ok(false);
        }
        self.model.kantor_set_row_entries(sheet, &new_value)?;
        self.push_diff_list(vec![Diff::KantorRowEntries {
            sheet,
            old_value,
            new_value,
        }]);
        self.evaluate_if_not_paused();
        Ok(true)
    }

    /// Sort rows: row `top + i` gets the cells (columns `left..=right`) of
    /// row `order[i]`, as one undo step. `order` must be a permutation of
    /// `top..top + order.len()`. Refused over merged cells and array
    /// formulas (which can't be cut in pieces).
    pub fn kantor_permute_rows(
        &mut self,
        sheet: u32,
        top: i32,
        left: i32,
        right: i32,
        order: &[i32],
    ) -> Result<(), String> {
        let n = order.len() as i32;
        if n == 0 || order.iter().enumerate().all(|(i, &r)| r == top + i as i32) {
            return Ok(());
        }
        let mut seen = vec![false; order.len()];
        for &r in order {
            let i = r - top;
            if i < 0 || i >= n || seen[i as usize] {
                return Err("Not a permutation of the rows".to_string());
            }
            seen[i as usize] = true;
        }
        let bottom = top + n - 1;
        let ws = self.model.workbook.worksheet(sheet)?;
        if ws.merged_cells.iter().any(|m| {
            m.row <= bottom
                && m.row + m.height - 1 >= top
                && m.column <= right
                && m.column + m.width - 1 >= left
        }) {
            return Err("Can't sort merged cells".to_string());
        }
        if ws
            .sheet_data
            .kantor_has_array_cells(top, bottom, left, right)
        {
            return Err("Can't sort array formulas or spilled values".to_string());
        }
        self.model.kantor_permute_rows(sheet, top, left, right, order)?;
        self.push_diff_list(vec![Diff::KantorPermuteRows {
            sheet,
            top,
            left,
            right,
            order: order.to_vec(),
        }]);
        self.evaluate_if_not_paused();
        Ok(())
    }

    /// Snapshots of the tables a row/column insert or delete changed, put
    /// before it in its diff list: undo restores the old table after the
    /// cells are back (a delete can't be undone by re-inserting).
    pub(crate) fn sheet_filter_snapshot_diff(&self, sheet: u32, old: Option<String>) -> Option<Diff> {
        let now = self.model.workbook.worksheet(sheet).ok()?.kantor_auto_filter.clone();
        (now != old).then(|| Diff::KantorSetSheetFilter {
            sheet,
            old_value: old.clone(),
            new_value: old,
        })
    }

    pub(crate) fn tables_snapshot_diffs(&self, old: &HashMap<String, Table>) -> Vec<Diff> {
        old.iter()
            .filter(|(name, t)| self.model.workbook.tables.get(*name) != Some(*t))
            .map(|(name, t)| Diff::KantorSetTable {
                name: name.clone(),
                old_value: Some(Box::new(t.clone())),
                new_value: Some(Box::new(t.clone())),
            })
            .collect()
    }
}

/// Call `f` on every structured reference in `node`.
pub(crate) fn for_each_table_ref(node: &mut Node, f: &mut dyn FnMut(&mut TableRef)) {
    match node {
        Node::TableRefKind(t) => f(t),
        Node::OpRangeKind { left, right }
        | Node::OpConcatenateKind { left, right }
        | Node::OpSumKind { left, right, .. }
        | Node::OpProductKind { left, right, .. }
        | Node::OpPowerKind { left, right }
        | Node::CompareKind { left, right, .. } => {
            for_each_table_ref(left, f);
            for_each_table_ref(right, f);
        }
        Node::FunctionKind { args, .. } | Node::NamedFunctionKind { args, .. } => {
            for a in args {
                for_each_table_ref(a, f);
            }
        }
        Node::LambdaDefKind { body, .. } => for_each_table_ref(body, f),
        Node::LambdaCallKind { lambda, args } => {
            for_each_table_ref(lambda, f);
            for a in args {
                for_each_table_ref(a, f);
            }
        }
        Node::ImplicitIntersection { child, .. } | Node::SpillRangeOperator { child } => {
            for_each_table_ref(child, f)
        }
        Node::UnaryKind { right, .. } => for_each_table_ref(right, f),
        _ => {}
    }
}

impl Model<'_> {
    /// Rename column `old` of table `table` to `new` and every structured
    /// reference to it, as Excel does when a header cell is edited.
    pub(crate) fn kantor_rename_table_column(
        &mut self,
        table: &str,
        old: &str,
        new: &str,
    ) -> Result<(), String> {
        use crate::expressions::{lexer::LexerMode, parser::stringify::to_rc_format, types::CellReferenceRC};
        let mut t = find_table(&self.workbook.tables, table)
            .cloned()
            .ok_or_else(|| format!("No table {table}"))?;
        let lower = old.to_lowercase();
        let column = t
            .columns
            .iter_mut()
            .find(|c| c.name.to_lowercase() == lower)
            .ok_or_else(|| format!("No column {old} in {table}"))?;
        column.name = new.to_string();
        let table_name = t.name.clone();
        // Parsed with the old names, written with the new.
        self.parser.set_lexer_mode(LexerMode::R1C1);
        let mut changed = false;
        for worksheet in self.workbook.worksheets.iter_mut() {
            if !worksheet.shared_formulas.iter().any(|f| f.contains('[')) {
                continue;
            }
            let context = CellReferenceRC {
                sheet: worksheet.get_name(),
                row: 1,
                column: 1,
            };
            let mut formulas = Vec::with_capacity(worksheet.shared_formulas.len());
            for formula in &worksheet.shared_formulas {
                if !formula.contains('[') {
                    formulas.push(formula.clone());
                    continue;
                }
                let mut node = self.parser.parse(formula, &context);
                let mut hit = false;
                for_each_table_ref(&mut node, &mut |r| {
                    if r.table != table_name {
                        return;
                    }
                    if let Some((a, b)) = &mut r.columns {
                        for c in [a, b] {
                            if c.to_lowercase() == lower {
                                *c = new.to_string();
                                hit = true;
                            }
                        }
                    }
                });
                if hit {
                    changed = true;
                    formulas.push(to_rc_format(&node));
                } else {
                    formulas.push(formula.clone());
                }
            }
            worksheet.shared_formulas = formulas;
        }
        self.parser.set_lexer_mode(LexerMode::A1);
        self.kantor_put_table(&table_name, Some(t));
        if changed {
            self.reset_parsed_structures();
        }
        Ok(())
    }
}

impl UserModel<'_> {
    /// Rename a table's column (and the references to it) as one undo step.
    pub fn kantor_rename_table_column(
        &mut self,
        table: &str,
        old: &str,
        new: &str,
    ) -> Result<(), String> {
        if new.trim().is_empty() {
            return Err("A column needs a name".to_string());
        }
        self.model.kantor_rename_table_column(table, old, new)?;
        self.push_diff_list(vec![Diff::KantorRenameTableColumn {
            table: table.to_string(),
            old_name: old.to_string(),
            new_name: new.to_string(),
        }]);
        self.evaluate_if_not_paused();
        Ok(())
    }
}
