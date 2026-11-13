//! The rows of a sheet, by position: chunks of consecutive rows, allocated as
//! they are needed.

use std::borrow::Cow;

use bitcode::{Decode, Encode};

use super::row::Row;
use crate::constants::LAST_ROW;
use crate::types::Cell;

const CHUNK_BITS: u32 = 10;
const CHUNK_ROWS: usize = 1 << CHUNK_BITS;
const CHUNK_MASK: usize = CHUNK_ROWS - 1;

/// The cells of a worksheet, by row and column. Both are 1-indexed.
///
/// A row can exist without cells: `set_row` with no cells creates one. The xlsx
/// importer does that for rows that carry a height or a style and nothing
/// else, and the exporter writes a `<row>` element for every row that exists
/// here, so such rows survive a round trip. Removing the last cell of a row
/// with `remove_cell` removes the row.
///
/// Rows outside the sheet, below 1 or beyond the last row, cannot hold cells:
/// writing to them does nothing and reading them finds nothing.
#[derive(Encode, Decode, Debug, Clone, Default)]
pub struct SheetData {
    /// `chunks[i]` holds rows `i * CHUNK_ROWS ..`, if any of them exists.
    chunks: Vec<Option<Box<Chunk>>>,
    /// How many rows exist.
    row_count: usize,
}

#[derive(Encode, Decode, Debug, Clone)]
struct Chunk {
    /// Always `CHUNK_ROWS` long.
    rows: Vec<Option<Row>>,
    /// How many of them exist. A chunk with none is freed.
    used: usize,
}

impl Chunk {
    fn new() -> Self {
        let mut rows = Vec::new();
        rows.resize_with(CHUNK_ROWS, || None);
        Chunk { rows, used: 0 }
    }
}

/// The chunk and the place in it of a row, if the row is within the sheet.
#[inline]
fn locate(row: i32) -> Option<(usize, usize)> {
    if !(1..=LAST_ROW).contains(&row) {
        return None;
    }
    let row = row as usize;
    Some((row >> CHUNK_BITS, row & CHUNK_MASK))
}

impl SheetData {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub(super) fn row(&self, row: i32) -> Option<&Row> {
        let (chunk, index) = locate(row)?;
        self.chunks.get(chunk)?.as_ref()?.rows[index].as_ref()
    }

    #[inline]
    fn row_mut(&mut self, row: i32) -> Option<&mut Row> {
        let (chunk, index) = locate(row)?;
        self.chunks.get_mut(chunk)?.as_mut()?.rows[index].as_mut()
    }

    /// The row, created if it does not exist. `None` outside the sheet.
    fn row_or_insert(&mut self, row: i32) -> Option<&mut Row> {
        let (chunk, index) = locate(row)?;
        if chunk >= self.chunks.len() {
            self.chunks.resize_with(chunk + 1, || None);
        }
        let chunk = self.chunks[chunk].get_or_insert_with(|| Box::new(Chunk::new()));
        if chunk.rows[index].is_none() {
            chunk.rows[index] = Some(Row::default());
            chunk.used += 1;
            self.row_count += 1;
        }
        chunk.rows[index].as_mut()
    }

    /// Every row that exists with its number, in ascending order.
    fn rows_in_order(&self) -> impl Iterator<Item = (i32, &Row)> {
        self.chunks
            .iter()
            .enumerate()
            .filter_map(|(chunk_index, chunk)| Some((chunk_index, chunk.as_ref()?)))
            .flat_map(|(chunk_index, chunk)| {
                chunk
                    .rows
                    .iter()
                    .enumerate()
                    .filter_map(move |(index, row)| {
                        let number = (chunk_index << CHUNK_BITS) | index;
                        Some((number as i32, row.as_ref()?))
                    })
            })
    }

    /// The cell at a position, if there is one.
    #[inline]
    pub fn cell(&self, row: i32, column: i32) -> Option<Cow<'_, Cell>> {
        self.row(row)?.get(column)
    }

    /// The cell at a position, to be changed in place.
    pub fn cell_mut(&mut self, row: i32, column: i32) -> Option<&mut Cell> {
        self.row_mut(row)?.get_mut(column)
    }

    /// KANTOR PATCH: sets the style of the cell at a position in place, if
    /// there is a cell (see `Row::set_style`). Returns whether there was.
    pub fn set_cell_style(&mut self, row: i32, column: i32, style: i32) -> bool {
        self.row_mut(row)
            .is_some_and(|row_data| row_data.set_style(column, style))
    }

    /// Puts a cell at a position and returns the one that was there.
    pub fn set_cell(&mut self, row: i32, column: i32, cell: Cell) -> Option<Cell> {
        self.row_or_insert(row)?.insert(column, cell)
    }

    /// Removes the cell at a position and returns it.
    pub fn remove_cell(&mut self, row: i32, column: i32) -> Option<Cell> {
        let row_data = self.row_mut(row)?;
        let cell = row_data.remove(column)?;
        if row_data.is_empty() {
            self.remove_row(row);
        }
        Some(cell)
    }

    /// True if the sheet has no rows.
    pub fn is_empty(&self) -> bool {
        self.row_count == 0
    }

    /// Every cell of the sheet with its row and column, in natural order:
    /// by row, and within a row by column.
    pub fn cells(&self) -> impl Iterator<Item = (i32, i32, Cow<'_, Cell>)> {
        self.rows_in_order().flat_map(|(row, row_data)| {
            row_data
                .iter()
                .map(move |(column, cell)| (row, column, cell))
        })
    }

    /// KANTOR PATCH: the positions of the formula cells (`CellFormula` and
    /// `ArrayFormula`) in natural order, like `cells` but without making a
    /// cell of every packed one (which copies text values).
    pub fn for_each_formula(&self, mut found: impl FnMut(i32, i32)) {
        for (row, row_data) in self.rows_in_order() {
            row_data.formula_columns(|column| found(row, column));
        }
    }

    /// KANTOR PATCH (partial recalculation): every formula cell in natural
    /// order with its formula index and whether it has no value yet; the
    /// index is `None` for an array formula.
    pub fn kantor_for_each_formula(&self, mut found: impl FnMut(i32, i32, Option<(i32, bool)>)) {
        for (row, row_data) in self.rows_in_order() {
            row_data.kantor_formulas(|column, formula| found(row, column, formula));
        }
    }

    /// KANTOR PATCH: the array formulas and spill cells in natural order. Rows
    /// of numbers and of packed cells hold none and are skipped.
    pub fn array_cells(&self) -> impl Iterator<Item = (i32, i32, &Cell)> {
        self.rows_in_order().flat_map(|(row, row_data)| {
            row_data
                .array_cells()
                .map(move |(column, cell)| (row, column, cell))
        })
    }

    /// KANTOR PATCH (fast sort): whether an array formula or a spill cell is
    /// in rows `top..=bottom`, columns `left..=right`. Rows of numbers and of
    /// packed cells hold none and are not looked into.
    pub fn kantor_has_array_cells(&self, top: i32, bottom: i32, left: i32, right: i32) -> bool {
        (top..=bottom).any(|r| {
            self.row(r).is_some_and(|row| {
                row.array_cells()
                    .any(|(column, _)| (left..=right).contains(&column))
            })
        })
    }

    /// KANTOR PATCH: `(min_row, max_row, min_column, max_column)` of the
    /// cells, one step per row rather than per cell. `None` without cells.
    pub fn kantor_extent(&self) -> Option<(i32, i32, i32, i32)> {
        let mut out: Option<(i32, i32, i32, i32)> = None;
        for (row, data) in self.rows_in_order() {
            let Some((first, last)) = data.column_span() else {
                continue;
            };
            out = Some(match out {
                None => (row, row, first, last),
                Some((r0, _, c0, c1)) => (r0, row, c0.min(first), c1.max(last)),
            });
        }
        out
    }

    /// The rows that exist, in ascending order.
    pub fn rows(&self) -> Vec<i32> {
        self.rows_in_order().map(|(row, _)| row).collect()
    }

    /// The columns of a row that have cells, in ascending order.
    pub fn columns_in_row(&self, row: i32) -> Vec<i32> {
        match self.row(row) {
            Some(row_data) => row_data.iter().map(|(column, _)| column).collect(),
            None => Vec::new(),
        }
    }

    /// The cells of a row with their column, in ascending order of column.
    pub fn cells_in_row(&self, row: i32) -> Vec<(i32, Cow<'_, Cell>)> {
        match self.row(row) {
            Some(row_data) => row_data.iter().collect(),
            None => Vec::new(),
        }
    }

    /// The cells of a row from `first_column` to `last_column`, both included,
    /// with their column, in ascending order of column. Reading a stretch of a
    /// row this way finds the row once, not once per cell.
    pub fn cells_in_row_between(
        &self,
        row: i32,
        first_column: i32,
        last_column: i32,
    ) -> impl Iterator<Item = (i32, Cow<'_, Cell>)> {
        self.row(row)
            .map(|row_data| row_data.iter_from(first_column))
            .into_iter()
            .flatten()
            .take_while(move |(column, _)| *column <= last_column)
    }

    /// The numbers of a row from `first_column` to `last_column`, both
    /// included, if the row holds them as numbers and nothing else, with none
    /// missing. `None` is no news about the row: its cells say what it holds.
    #[inline]
    pub fn numbers_in_row_between(
        &self,
        row: i32,
        first_column: i32,
        last_column: i32,
    ) -> Option<&[f64]> {
        match self.row(row)? {
            Row::Numbers {
                first_column: row_first_column,
                values,
                ..
            } => {
                let first = Row::offset(*row_first_column, values, first_column)?;
                let last = Row::offset(*row_first_column, values, last_column)?;
                values.get(first..=last)
            }
            Row::Cells(_) | Row::Packed(_) => None,
        }
    }

    /// Removes every cell of a row, and the row.
    pub fn remove_row(&mut self, row: i32) {
        let Some((chunk_index, index)) = locate(row) else {
            return;
        };
        let Some(Some(chunk)) = self.chunks.get_mut(chunk_index) else {
            return;
        };
        if chunk.rows[index].take().is_some() {
            chunk.used -= 1;
            self.row_count -= 1;
            if chunk.used == 0 {
                self.chunks[chunk_index] = None;
            }
        }
    }

    /// Takes a row out as it is held (numbers stay numbers, packed cells
    /// stay packed), leaving no row there.
    fn take_row(&mut self, row: i32) -> Option<Row> {
        let (chunk_index, index) = locate(row)?;
        let chunk = self.chunks.get_mut(chunk_index)?.as_mut()?;
        let taken = chunk.rows[index].take()?;
        chunk.used -= 1;
        self.row_count -= 1;
        if chunk.used == 0 {
            self.chunks[chunk_index] = None;
        }
        Some(taken)
    }

    /// Puts a row taken by `take_row` where there is no row.
    fn put_row(&mut self, row: i32, data: Row) {
        if let Some(slot) = self.row_or_insert(row) {
            *slot = data;
        }
    }

    /// KANTOR PATCH (fast sort): row `top + i` gets the row that was at
    /// `order[i]`, for `order` a permutation of `top..top + order.len()`,
    /// moving each row whole, as it is held. Only when every one of those
    /// rows has its cells within `left..=right`; otherwise nothing changes
    /// and it returns false (the caller moves the cells one by one).
    pub fn kantor_permute_whole_rows(
        &mut self,
        top: i32,
        left: i32,
        right: i32,
        order: &[i32],
    ) -> bool {
        let n = order.len() as i32;
        let whole = (top..top + n).all(|r| {
            self.row(r)
                .and_then(Row::column_span)
                .is_none_or(|(first, last)| first >= left && last <= right)
        });
        if !whole {
            return false;
        }
        let mut taken: Vec<Option<Row>> = (top..top + n).map(|r| self.take_row(r)).collect();
        for (i, &src) in order.iter().enumerate() {
            let Some(slot) = taken.get_mut((src - top) as usize) else {
                continue;
            };
            if let Some(data) = slot.take() {
                self.put_row(top + i as i32, data);
            }
        }
        true
    }

    /// Replaces the cells of a row. The row exists afterwards, even with no
    /// cells. If a column is given twice the last one stays.
    pub fn set_row(&mut self, row: i32, cells: impl IntoIterator<Item = (i32, Cell)>) {
        let Some(row_data) = self.row_or_insert(row) else {
            return;
        };
        // Collected at its exact size: a row grown cell by cell from empty
        // starts with room for four, and a sheet with a million short rows
        // paid for that four times over.
        let mut cells: Vec<(i32, Cell)> = cells.into_iter().collect();
        // A file gives the cells of a row from left to right, and then there
        // is nothing to sort and no column given twice.
        if !cells.is_sorted_by(|earlier, later| earlier.0 < later.0) {
            cells.sort_by_key(|(column, _)| *column);
            // Of two cells in the same column the later one stays: it is moved
            // into the slot of the earlier one, which `dedup_by` then keeps.
            cells.dedup_by(|later, earlier| {
                if later.0 == earlier.0 {
                    std::mem::swap(&mut later.1, &mut earlier.1);
                    true
                } else {
                    false
                }
            });
        }
        cells.shrink_to_fit();
        *row_data = Row::from_cells(cells);
    }
}

/// Two sheets are equal when they have the same rows and the same cells.
/// Which chunks happen to be allocated is not part of that.
impl PartialEq for SheetData {
    fn eq(&self, other: &Self) -> bool {
        self.row_count == other.row_count && self.rows_in_order().eq(other.rows_in_order())
    }
}
