#![deny(missing_docs)]

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use crate::{
    constants::{LAST_COLUMN, LAST_ROW},
    expressions::{
        parser::{static_analysis::StaticResult, Node},
        types::Area,
        utils::{is_valid_column_number, is_valid_row},
    },
    model::CellStructure,
    types::{Cell, Link, MergedCell, Style},
    user_model::sequence_detector::detect_progression,
    UserModel,
};

use crate::user_model::history::Diff;

const PARTIAL_MERGE_ERROR: &str =
    "Cannot auto-fill: a merged cell partially overlaps the fill area";
const CUT_MERGE_ERROR: &str = "Cannot auto-fill: the fill size must fit whole merged cells";

/// KANTOR PATCH (fast fill): one target cell of a fill, before it is written.
struct KantorFilled {
    /// The translated text (or the next value of a series): what upstream
    /// typed into the cell, and what the diff replays.
    value: String,
    /// The value is the source's content moved over, not a series.
    copy_formula: bool,
    old_value: Option<Cell>,
    old_style: Option<Style>,
}

/// KANTOR PATCH (fast fill): what a source cell gives every copy of it.
struct KantorFillSource {
    style: Style,
    style_index: i32,
    link: Option<Link>,
}

/// KANTOR PATCH (fast fill): whether the formula `node`, typed into the cell
/// at `row`, `column` as text translated from its source, parses back to
/// `node` itself. Relative references are offsets from the cell, so that
/// holds when every reference stays on the sheet and nothing in the formula
/// is looked up in the workbook when parsed or printed. Anything not known
/// to be safe says no, and the fill takes the slow way.
fn kantor_copies_unchanged(node: &Node, row: i32, column: i32) -> bool {
    let on_sheet = |value: i32, absolute: bool, at: i32, last: i32| {
        absolute || (1..=last).contains(&(at + value))
    };
    let row_ok = |value: i32, absolute: bool| on_sheet(value, absolute, row, LAST_ROW);
    let column_ok = |value: i32, absolute: bool| on_sheet(value, absolute, column, LAST_COLUMN);
    let all = |nodes: &[Node]| {
        nodes
            .iter()
            .all(|n| kantor_copies_unchanged(n, row, column))
    };
    match node {
        Node::BooleanKind(_)
        | Node::NumberKind(_)
        | Node::StringKind(_)
        | Node::ErrorKind(_)
        | Node::EmptyArgKind
        | Node::ArrayKind(_) => true,
        Node::ReferenceKind {
            absolute_row,
            absolute_column,
            row: r,
            column: c,
            ..
        } => row_ok(*r, *absolute_row) && column_ok(*c, *absolute_column),
        Node::RangeKind {
            absolute_row1,
            absolute_column1,
            row1,
            column1,
            absolute_row2,
            absolute_column2,
            row2,
            column2,
            ..
        } => {
            row_ok(*row1, *absolute_row1)
                && column_ok(*column1, *absolute_column1)
                && row_ok(*row2, *absolute_row2)
                && column_ok(*column2, *absolute_column2)
        }
        Node::OpRangeKind { left, right }
        | Node::OpConcatenateKind { left, right }
        | Node::OpSumKind { left, right, .. }
        | Node::OpProductKind { left, right, .. }
        | Node::OpPowerKind { left, right }
        | Node::CompareKind { left, right, .. } => {
            kantor_copies_unchanged(left, row, column)
                && kantor_copies_unchanged(right, row, column)
        }
        Node::UnaryKind { right, .. } => kantor_copies_unchanged(right, row, column),
        Node::FunctionKind { args, .. } => all(args),
        Node::WrongReferenceKind { .. }
        | Node::WrongRangeKind { .. }
        | Node::LambdaDefKind { .. }
        | Node::LambdaCallKind { .. }
        | Node::NamedFunctionKind { .. }
        | Node::DefinedNameKind(_)
        | Node::TableNameKind(_)
        | Node::TableRefKind(_)
        | Node::NamedVariableKind { .. }
        | Node::ImplicitIntersection { .. }
        | Node::SpillRangeOperator { .. }
        | Node::ParseErrorKind { .. } => false,
    }
}

// How the merged cells of the sheet interact with a fill:
// the merges of the source are tiled into the fill target, and the merges
// already in the target are replaced by that pattern.
struct MergedFillPlan {
    // source cells covered by a merge (every cell of a source merge except
    // its anchor): they hold no content, and neither do their tiled copies
    covered_source: HashSet<(i32, i32)>,
    // the tiled copies of the source merges, to create in the fill target
    new_merges: Vec<MergedCell>,
    // the merged cells of the sheet before the fill
    old_merged_cells: Vec<MergedCell>,
    // the merged cells after removing the ones replaced by the fill, before
    // adding `new_merges`
    merged_cells_after_removal: Vec<MergedCell>,
}

fn merge_is_contained(m: &MergedCell, row1: i32, column1: i32, row2: i32, column2: i32) -> bool {
    m.row >= row1
        && m.column >= column1
        && m.row + m.height - 1 <= row2
        && m.column + m.width - 1 <= column2
}

impl<'a> UserModel<'a> {
    // Validates the fill of `target` from `source` (both closed rectangles
    // `(first_row, first_column, last_row, last_column)` on `sheet`) against
    // the merged cells of the sheet and returns the resulting plan:
    //
    // * A merge intersecting the source or the target but not fully contained
    //   in it fails with [PARTIAL_MERGE_ERROR].
    // * The source merges are tiled into the target with period the source
    //   height (`by_rows`) or width. A tiled merge that would stick out of
    //   the target fails with [CUT_MERGE_ERROR]: the fill can stop mid-tile,
    //   but only where it cuts no merge.
    // * Merges fully contained in the target are replaced by the tiled
    //   pattern (they are dropped from `merged_cells_after_removal`).
    fn plan_fill_merges(
        &self,
        sheet: u32,
        source: (i32, i32, i32, i32),
        target: (i32, i32, i32, i32),
        by_rows: bool,
    ) -> Result<MergedFillPlan, String> {
        let (s_row1, s_column1, s_row2, s_column2) = source;
        let (t_row1, t_column1, t_row2, t_column2) = target;
        let merged_cells = &self.model.workbook.worksheet(sheet)?.merged_cells;

        let mut source_merges = Vec::new();
        let mut merged_cells_after_removal = Vec::new();
        for m in merged_cells {
            if m.intersects(
                s_row1,
                s_column1,
                s_column2 - s_column1 + 1,
                s_row2 - s_row1 + 1,
            ) {
                if !merge_is_contained(m, s_row1, s_column1, s_row2, s_column2) {
                    return Err(PARTIAL_MERGE_ERROR.to_string());
                }
                source_merges.push(*m);
                merged_cells_after_removal.push(*m);
            } else if m.intersects(
                t_row1,
                t_column1,
                t_column2 - t_column1 + 1,
                t_row2 - t_row1 + 1,
            ) {
                if !merge_is_contained(m, t_row1, t_column1, t_row2, t_column2) {
                    return Err(PARTIAL_MERGE_ERROR.to_string());
                }
                // fully contained in the fill target: replaced by the fill
            } else {
                merged_cells_after_removal.push(*m);
            }
        }

        let mut covered_source = HashSet::new();
        for m in &source_merges {
            for row in m.row..m.row + m.height {
                for column in m.column..m.column + m.width {
                    if (row, column) != (m.row, m.column) {
                        covered_source.insert((row, column));
                    }
                }
            }
        }

        // Tile the source merges into the target. Tiles start at the target
        // edge next to the source and repeat away from it.
        let mut new_merges = Vec::new();
        if !source_merges.is_empty() {
            let mut tile_starts = Vec::new();
            if by_rows {
                let period = s_row2 - s_row1 + 1;
                if t_row1 > s_row2 {
                    let mut start = t_row1;
                    while start <= t_row2 {
                        tile_starts.push(start);
                        start += period;
                    }
                } else {
                    let mut start = s_row1 - period;
                    while start + period > t_row1 {
                        tile_starts.push(start);
                        start -= period;
                    }
                }
                for tile_start in tile_starts {
                    for m in &source_merges {
                        let first_row = tile_start + (m.row - s_row1);
                        let last_row = first_row + m.height - 1;
                        if last_row < t_row1 || first_row > t_row2 {
                            continue;
                        }
                        if first_row < t_row1 || last_row > t_row2 {
                            return Err(CUT_MERGE_ERROR.to_string());
                        }
                        new_merges.push(MergedCell {
                            row: first_row,
                            column: m.column,
                            width: m.width,
                            height: m.height,
                        });
                    }
                }
            } else {
                let period = s_column2 - s_column1 + 1;
                if t_column1 > s_column2 {
                    let mut start = t_column1;
                    while start <= t_column2 {
                        tile_starts.push(start);
                        start += period;
                    }
                } else {
                    let mut start = s_column1 - period;
                    while start + period > t_column1 {
                        tile_starts.push(start);
                        start -= period;
                    }
                }
                for tile_start in tile_starts {
                    for m in &source_merges {
                        let first_column = tile_start + (m.column - s_column1);
                        let last_column = first_column + m.width - 1;
                        if last_column < t_column1 || first_column > t_column2 {
                            continue;
                        }
                        if first_column < t_column1 || last_column > t_column2 {
                            return Err(CUT_MERGE_ERROR.to_string());
                        }
                        new_merges.push(MergedCell {
                            row: m.row,
                            column: first_column,
                            width: m.width,
                            height: m.height,
                        });
                    }
                }
            }
        }

        Ok(MergedFillPlan {
            covered_source,
            new_merges,
            old_merged_cells: merged_cells.clone(),
            merged_cells_after_removal,
        })
    }

    // Removes the merges the fill target replaces, with its diff. Pushed
    // before the cell diffs so a forward replay writes on unmerged cells.
    fn apply_fill_merge_removal(
        &mut self,
        sheet: u32,
        plan: &MergedFillPlan,
        diff_list: &mut Vec<Diff>,
    ) -> Result<(), String> {
        if plan.merged_cells_after_removal != plan.old_merged_cells {
            self.model.workbook.worksheet_mut(sheet)?.merged_cells =
                plan.merged_cells_after_removal.clone();
            diff_list.push(Diff::SetMergedCells {
                sheet,
                old_value: plan.old_merged_cells.clone(),
                new_value: plan.merged_cells_after_removal.clone(),
            });
        }
        Ok(())
    }

    // Creates the tiled merges in the fill target, with its diff. Pushed
    // after the cell diffs so an undo unmerges first and can then restore the
    // old content of the covered cells.
    fn apply_fill_merge_creation(
        &mut self,
        sheet: u32,
        plan: &MergedFillPlan,
        diff_list: &mut Vec<Diff>,
    ) -> Result<(), String> {
        if !plan.new_merges.is_empty() {
            let worksheet = self.model.workbook.worksheet_mut(sheet)?;
            worksheet
                .merged_cells
                .extend(plan.new_merges.iter().copied());
            diff_list.push(Diff::SetMergedCells {
                sheet,
                old_value: plan.merged_cells_after_removal.clone(),
                new_value: worksheet.merged_cells.clone(),
            });
        }
        Ok(())
    }

    // The fill target cell is covered by a tiled merge: it gets no value,
    // only the style and (lack of) link of its source cell, and its old
    // content is cleared. `source` and `target` are `(row, column)` pairs.
    fn fill_covered_cell(
        &mut self,
        sheet: u32,
        source: (i32, i32),
        target: (i32, i32),
        old_value: Option<crate::types::Cell>,
        diff_list: &mut Vec<Diff>,
    ) -> Result<(), String> {
        let (source_row, source_column) = source;
        let (row, column) = target;
        let old_style = self.model.get_cell_style_or_none(sheet, row, column)?;
        // Going through prepare_cell_for_user_input keeps dynamic array
        // formulas consistent (an anchor spilling into the cell is reset)
        self.model.prepare_cell_for_user_input(sheet, row, column)?;
        let worksheet = self.model.workbook.worksheet_mut(sheet)?;
        if worksheet.cell(row, column).is_some() {
            worksheet.cell_clear_contents(row, column)?;
        }
        if old_value.is_some() {
            diff_list.push(Diff::SetCellValue {
                sheet,
                row,
                column,
                new_value: "".to_string(),
                old_value: Box::new(old_value),
            });
        }

        let new_style = self
            .model
            .get_style_for_cell(sheet, source_row, source_column)?;
        self.model.set_cell_style(sheet, row, column, &new_style)?;
        diff_list.push(Diff::SetCellStyle {
            sheet,
            row,
            column,
            old_value: Box::new(old_style),
            new_value: Box::new(new_style),
        });

        self.fill_cell_link(sheet, source_row, source_column, row, column, diff_list)
    }
    /// Scans the fill target rectangle (`row_start..=row_end`, `col_start..=col_end`) for
    /// CSE array formulas and prepares them for overwriting:
    ///
    /// * A CSE formula **completely** inside the rectangle is cleared; its original cell
    ///   values are returned keyed by `(row, col)` so the caller can emit correct undo diffs.
    /// * A CSE formula only **partially** overlapping the rectangle causes an immediate error.
    fn collect_and_clear_cse_in_fill_target(
        &mut self,
        sheet: u32,
        row_start: i32,
        row_end: i32,
        col_start: i32,
        col_end: i32,
    ) -> Result<HashMap<(i32, i32), Option<crate::types::Cell>>, String> {
        // First pass: validate all affected CSE anchors without mutating the worksheet.
        // An error here leaves the sheet untouched.
        let mut anchors: Vec<(i32, i32, i32, i32)> = Vec::new();
        let mut handled: Vec<(i32, i32)> = Vec::new();
        for row in row_start..=row_end {
            for col in col_start..=col_end {
                let (ar, ac, w, h) = match self.model.get_cell_structure(sheet, row, col)? {
                    CellStructure::ArrayFormula { range: (w, h) } if w > 1 || h > 1 => {
                        (row, col, w, h)
                    }
                    CellStructure::SpillArray {
                        anchor: (ar, ac),
                        range: (w, h),
                    } => (ar, ac, w, h),
                    _ => continue,
                };
                if handled.contains(&(ar, ac)) {
                    continue;
                }
                handled.push((ar, ac));
                let completely_covered = ar >= row_start
                    && ar + h - 1 <= row_end
                    && ac >= col_start
                    && ac + w - 1 <= col_end;
                if !completely_covered {
                    return Err(
                        "Cannot autofill: selection partially overlaps an array formula"
                            .to_string(),
                    );
                }
                anchors.push((ar, ac, w, h));
            }
        }

        // Second pass: all anchors are completely covered — safe to save and clear.
        let mut saved: HashMap<(i32, i32), Option<crate::types::Cell>> = HashMap::new();
        for (ar, ac, w, h) in anchors {
            for r in ar..ar + h {
                for c in ac..ac + w {
                    let cell = self
                        .model
                        .workbook
                        .worksheet(sheet)?
                        .cell(r, c)
                        .map(Cow::into_owned);
                    saved.insert((r, c), cell);
                }
            }
            let ws = self.model.workbook.worksheet_mut(sheet)?;
            for r in ar..ar + h {
                for c in ac..ac + w {
                    let _ = ws.cell_clear_contents(r, c);
                }
            }
        }
        Ok(saved)
    }

    /// Fills the cells from `source_area` until `to_row`.
    /// This simulates the user clicking on the cell outline handle and dragging it downwards (or upwards)
    pub fn auto_fill_rows(&mut self, source_area: &Area, to_row: i32) -> Result<(), String> {
        let mut diff_list = Vec::new();
        self.kantor_fill_copies = 0;
        let sheet = source_area.sheet;
        let row1 = source_area.row;
        let column1 = source_area.column;
        let width = source_area.width;
        let height = source_area.height;

        // Check first all parameters are valid
        if self.model.workbook.worksheet(sheet).is_err() {
            return Err(format!("Invalid worksheet index: '{sheet}'"));
        }

        if !is_valid_column_number(column1) {
            return Err(format!("Invalid column: '{column1}'"));
        }
        if !is_valid_row(row1) {
            return Err(format!("Invalid row: '{row1}'"));
        }
        if width <= 0 || height <= 0 {
            return Err(format!("Invalid width='{}' or height='{}'", width, height));
        }

        let last_column = column1 + width - 1;
        let last_row = row1 + height - 1;

        if !is_valid_column_number(last_column) {
            return Err(format!("Invalid column: '{last_column}'"));
        }
        if !is_valid_row(last_row) {
            return Err(format!("Invalid row: '{last_row}'"));
        }

        if !is_valid_row(to_row) {
            return Err(format!("Invalid row: '{to_row}'"));
        }

        // anchor_row is the first row that repeats in each case.
        let anchor_row;
        let sign;
        // this is the range of rows we are going to fill
        let row_range: Vec<i32>;

        if to_row > last_row {
            // we go downwards, we start from `row1 + height1` to `to_row`,
            anchor_row = row1;
            sign = 1;
            row_range = (last_row + 1..=to_row).collect();
        } else if to_row < row1 {
            // we go upwards, starting from `row1 - 1` all the way to `to_row`
            anchor_row = last_row;
            sign = -1;
            row_range = (to_row..row1).rev().collect();
        } else {
            return Err("Invalid parameters for autofill".to_string());
        }

        // Fill target: rows in row_range, all source columns.
        let fill_row_start = if sign < 0 { to_row } else { last_row + 1 };
        let fill_row_end = if sign < 0 { row1 - 1 } else { to_row };

        // The merged cells of the source are tiled into the fill target and
        // replace the merges already there; a merge partially overlapping the
        // source or the target, or cut by the fill boundary, is an error.
        let plan = self.plan_fill_merges(
            sheet,
            (row1, column1, last_row, last_column),
            (fill_row_start, column1, fill_row_end, last_column),
            true,
        )?;
        self.apply_fill_merge_removal(sheet, &plan, &mut diff_list)?;

        let saved_cse = self.collect_and_clear_cse_in_fill_target(
            sheet,
            fill_row_start,
            fill_row_end,
            column1,
            last_column,
        )?;
        // KANTOR PATCH (fast fill): what each source cell gives its copies.
        let mut sources = HashMap::new();

        for column in column1..=last_column {
            let mut index = 0;
            let locale = &self.model.locale;
            // covered cells hold no content: the progression is detected over
            // the rest and lands on the non-covered cells of the target
            let values = if sign < 0 {
                (row1..=last_row)
                    .rev()
                    .filter(|row| !plan.covered_source.contains(&(*row, column)))
                    .map(|row| self.get_cell_content(sheet, row, column))
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                (row1..=last_row)
                    .filter(|row| !plan.covered_source.contains(&(*row, column)))
                    .map(|row| self.get_cell_content(sheet, row, column))
                    .collect::<Result<Vec<_>, _>>()?
            };
            let case_seed = self.get_cell_content(sheet, row1, column)?;
            let possible_progression = if values.is_empty() {
                None
            } else {
                detect_progression(&values, locale, &case_seed)
            };
            let mut progression_idx = 0;
            for row_ref in row_range.iter() {
                let row = *row_ref;

                let old_value = saved_cse.get(&(row, column)).cloned().unwrap_or_else(|| {
                    self.model
                        .workbook
                        .worksheet(sheet)
                        .ok()
                        .and_then(|ws| ws.cell(row, column).map(Cow::into_owned))
                });

                let source_row = anchor_row + index;

                if plan.covered_source.contains(&(source_row, column)) {
                    // the target cell is covered by a tiled merge
                    self.fill_covered_cell(
                        sheet,
                        (source_row, column),
                        (row, column),
                        old_value,
                        &mut diff_list,
                    )?;
                    index = (index + sign) % source_area.height;
                    continue;
                }

                let old_style = self.model.get_cell_style_or_none(sheet, row, column)?;
                let target_value;

                // compute the new value and set it
                if let Some(ref detected_progression) = possible_progression {
                    target_value = detected_progression.next(progression_idx);
                    progression_idx += 1;
                } else {
                    target_value = self
                        .model
                        .extend_to(sheet, source_row, column, row, column)?;
                }

                // KANTOR PATCH (fast fill): see `kantor_fill_cell`.
                self.kantor_fill_cell(
                    sheet,
                    (source_row, column),
                    (row, column),
                    KantorFilled {
                        value: target_value,
                        copy_formula: possible_progression.is_none(),
                        old_value,
                        old_style,
                    },
                    &mut sources,
                    &mut diff_list,
                )?;

                index = (index + sign) % source_area.height;
            }
        }
        self.apply_fill_merge_creation(sheet, &plan, &mut diff_list)?;
        self.push_diff_list(diff_list);
        // KANTOR PATCH (partial recalculation): what the fill can change.
        self.kantor_evaluate_changes();
        Ok(())
    }

    /// Fills the cells from `source_area` until `to_column`.
    /// This simulates the user clicking on the cell outline handle and dragging it to the right (or to the left)
    pub fn auto_fill_columns(&mut self, source_area: &Area, to_column: i32) -> Result<(), String> {
        let mut diff_list = Vec::new();
        self.kantor_fill_copies = 0;
        let sheet = source_area.sheet;
        let row1 = source_area.row;
        let column1 = source_area.column;
        let width = source_area.width;
        let height = source_area.height;

        // Check first all parameters are valid
        if self.model.workbook.worksheet(sheet).is_err() {
            return Err(format!("Invalid worksheet index: '{sheet}'"));
        }

        if !is_valid_column_number(column1) {
            return Err(format!("Invalid column: '{column1}'"));
        }
        if !is_valid_row(row1) {
            return Err(format!("Invalid row: '{row1}'"));
        }
        if width <= 0 || height <= 0 {
            return Err(format!("Invalid width='{}' or height='{}'", width, height));
        }

        let last_column = column1 + width - 1;
        let last_row = row1 + height - 1;

        if !is_valid_column_number(last_column) {
            return Err(format!("Invalid column: '{last_column}'"));
        }
        if !is_valid_row(last_row) {
            return Err(format!("Invalid row: '{last_row}'"));
        }

        if !is_valid_column_number(to_column) {
            return Err(format!("Invalid column: '{to_column}'"));
        }

        // anchor_column is the first column that repeats in each case.
        let anchor_column;
        let sign;
        // this is the range of columns we are going to fill
        let column_range: Vec<i32>;

        if to_column > last_column {
            // we go right, we start from `last_column + 1` to `to_column`,
            anchor_column = column1;
            sign = 1;
            column_range = (last_column + 1..to_column + 1).collect();
        } else if to_column < column1 {
            // we go left, starting from `column1 - 1` all the way to `to_column`
            anchor_column = last_column;
            sign = -1;
            column_range = (to_column..column1).rev().collect();
        } else {
            return Err("Invalid parameters for autofill".to_string());
        }

        // Fill target: all source rows, columns in column_range.
        let fill_col_start = if sign < 0 { to_column } else { last_column + 1 };
        let fill_col_end = if sign < 0 { column1 - 1 } else { to_column };

        // The merged cells of the source are tiled into the fill target and
        // replace the merges already there; a merge partially overlapping the
        // source or the target, or cut by the fill boundary, is an error.
        let plan = self.plan_fill_merges(
            sheet,
            (row1, column1, last_row, last_column),
            (row1, fill_col_start, last_row, fill_col_end),
            false,
        )?;
        self.apply_fill_merge_removal(sheet, &plan, &mut diff_list)?;

        let saved_cse = self.collect_and_clear_cse_in_fill_target(
            sheet,
            row1,
            last_row,
            fill_col_start,
            fill_col_end,
        )?;
        // KANTOR PATCH (fast fill): what each source cell gives its copies.
        let mut sources = HashMap::new();

        for row in row1..=last_row {
            let mut index = 0;
            let locale = &self.model.locale;
            // covered cells hold no content: the progression is detected over
            // the rest and lands on the non-covered cells of the target
            let values = if sign < 0 {
                (column1..=last_column)
                    .rev()
                    .filter(|column| !plan.covered_source.contains(&(row, *column)))
                    .map(|column| self.get_cell_content(sheet, row, column))
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                (column1..=last_column)
                    .filter(|column| !plan.covered_source.contains(&(row, *column)))
                    .map(|column| self.get_cell_content(sheet, row, column))
                    .collect::<Result<Vec<_>, _>>()?
            };
            let case_seed = self.get_cell_content(sheet, row, column1)?;
            let possible_progression = if values.is_empty() {
                None
            } else {
                detect_progression(&values, locale, &case_seed)
            };
            let mut progression_idx = 0;
            for column_ref in column_range.iter() {
                let column = *column_ref;

                // Save value and style first
                let old_value = saved_cse.get(&(row, column)).cloned().unwrap_or_else(|| {
                    self.model
                        .workbook
                        .worksheet(sheet)
                        .ok()
                        .and_then(|ws| ws.cell(row, column).map(Cow::into_owned))
                });

                let source_column = anchor_column + index;

                if plan.covered_source.contains(&(row, source_column)) {
                    // the target cell is covered by a tiled merge
                    self.fill_covered_cell(
                        sheet,
                        (row, source_column),
                        (row, column),
                        old_value,
                        &mut diff_list,
                    )?;
                    index = (index + sign) % source_area.width;
                    continue;
                }

                let old_style = self.model.get_cell_style_or_none(sheet, row, column)?;
                let target_value;

                // compute the new value and set it
                if let Some(ref detected_progression) = possible_progression {
                    target_value = detected_progression.next(progression_idx);
                    progression_idx += 1;
                } else {
                    target_value = self
                        .model
                        .extend_to(sheet, row, source_column, row, column)?;
                }

                // KANTOR PATCH (fast fill): see `kantor_fill_cell`.
                self.kantor_fill_cell(
                    sheet,
                    (row, source_column),
                    (row, column),
                    KantorFilled {
                        value: target_value,
                        copy_formula: possible_progression.is_none(),
                        old_value,
                        old_style,
                    },
                    &mut sources,
                    &mut diff_list,
                )?;

                index = (index + sign) % source_area.width;
            }
        }
        self.apply_fill_merge_creation(sheet, &plan, &mut diff_list)?;
        self.push_diff_list(diff_list);
        // KANTOR PATCH (partial recalculation): what the fill can change.
        self.kantor_evaluate_changes();
        Ok(())
    }

    /// Makes the link of the fill target cell match the one of its source cell,
    /// adding the corresponding diff. This runs after the value is set so that
    /// it also overrides any link auto-created by an URL value.
    fn fill_cell_link(
        &mut self,
        sheet: u32,
        source_row: i32,
        source_column: i32,
        row: i32,
        column: i32,
        diff_list: &mut Vec<Diff>,
    ) -> Result<(), String> {
        let new_link = self.model.get_cell_link(sheet, source_row, source_column)?;
        self.kantor_set_fill_link(sheet, row, column, &new_link, diff_list)
    }

    /// KANTOR PATCH (fast fill): the second half of `fill_cell_link`, with
    /// the source's link already known.
    fn kantor_set_fill_link(
        &mut self,
        sheet: u32,
        row: i32,
        column: i32,
        new_link: &Option<Link>,
        diff_list: &mut Vec<Diff>,
    ) -> Result<(), String> {
        let ws = self.model.workbook.worksheet(sheet)?;
        if ws.links.get(&(row, column)) == new_link.as_ref() {
            return Ok(());
        }
        let old_link = self.model.get_cell_link(sheet, row, column)?;
        match new_link {
            Some(link) => self.model.set_cell_link(sheet, row, column, link.clone())?,
            None => self.model.delete_cell_link(sheet, row, column)?,
        }
        diff_list.push(Diff::SetCellLink {
            sheet,
            row,
            column,
            old_value: Box::new(old_link),
            new_value: Box::new(new_link.clone()),
        });
        Ok(())
    }

    /// KANTOR PATCH (fast fill): writes one cell of a fill (not a covered
    /// one) and records its diffs, as upstream did inline in both fill
    /// loops, but faster when filling many rows of formulas:
    ///
    /// * A formula is copied by giving the target its source's shared
    ///   formula: references relative to the cell are stored as offsets, so
    ///   a copy parses to the same formula wherever it lands, unless a
    ///   reference falls off the sheet or the formula holds something parsed
    ///   against the workbook (names, tables, lambdas; see
    ///   `kantor_copies_unchanged`). Those, and every non-formula, still go
    ///   through `set_user_input` with the translated text.
    /// * Each source cell's style (and its index in the style table) and
    ///   link are looked up once per fill, not once per copy.
    ///
    /// The diffs are the same as before: undo and redo replay the
    /// translated text, so they don't depend on the shortcut.
    fn kantor_fill_cell(
        &mut self,
        sheet: u32,
        source: (i32, i32),
        target: (i32, i32),
        filled: KantorFilled,
        sources: &mut HashMap<(i32, i32), KantorFillSource>,
        diff_list: &mut Vec<Diff>,
    ) -> Result<(), String> {
        let (row, column) = target;
        let copied = filled.copy_formula && self.kantor_copy_formula(sheet, source, target)?;
        if copied {
            self.kantor_fill_copies += 1;
        } else {
            self.model
                .set_user_input(sheet, row, column, filled.value.clone())?;
        }

        let source_data = match sources.entry(source) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let style = self.model.get_style_for_cell(sheet, source.0, source.1)?;
                let style_index = self.model.workbook.styles.get_style_index_or_create(&style);
                let link = self.model.get_cell_link(sheet, source.0, source.1)?;
                entry.insert(KantorFillSource {
                    style,
                    style_index,
                    link,
                })
            }
        };
        self.model.workbook.worksheet_mut(sheet)?.set_cell_style(
            row,
            column,
            source_data.style_index,
        )?;

        diff_list.push(Diff::SetCellStyle {
            sheet,
            row,
            column,
            old_value: Box::new(filled.old_style),
            new_value: Box::new(source_data.style.clone()),
        });
        diff_list.push(Diff::SetCellValue {
            sheet,
            row,
            column,
            new_value: filled.value,
            old_value: Box::new(filled.old_value),
        });
        let link = source_data.link.clone();
        self.kantor_set_fill_link(sheet, row, column, &link, diff_list)
    }

    /// KANTOR PATCH (fast fill): puts the formula of `source` in `target` as
    /// `set_user_input` would have with the translated formula, if that is
    /// sure to give the same shared formula. Returns whether it did.
    fn kantor_copy_formula(
        &mut self,
        sheet: u32,
        source: (i32, i32),
        target: (i32, i32),
    ) -> Result<bool, String> {
        let model = &self.model;
        let formula = match model.workbook.worksheet(sheet)?.cell(source.0, source.1) {
            Some(cell) => match cell.as_ref() {
                Cell::CellFormula { f, .. } => *f,
                _ => return Ok(false),
            },
            None => return Ok(false),
        };
        let fits = model
            .parsed_formulas
            .get(sheet as usize)
            .and_then(|formulas| formulas.get(formula as usize))
            .is_some_and(|(node, result)| {
                matches!(result, StaticResult::Scalar)
                    && kantor_copies_unchanged(node, target.0, target.1)
            });
        if !fits {
            return Ok(false);
        }
        let (row, column) = target;
        self.model.prepare_cell_for_user_input(sheet, row, column)?;
        // As `set_user_input`: the cell keeps its style without a quote
        // prefix (the fill sets the source's style right after anyway).
        let mut style = self.model.get_cell_style_index(sheet, row, column)?;
        if self.model.workbook.styles.style_is_quote_prefix(style) {
            style = self
                .model
                .workbook
                .styles
                .get_style_without_quote_prefix(style)?;
        }
        self.model
            .workbook
            .worksheet_mut(sheet)?
            .set_cell_with_formula(row, column, formula, style)?;
        Ok(true)
    }
}
