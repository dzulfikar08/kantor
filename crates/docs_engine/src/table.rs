//! Tables: structure and properties.
//!
//! Cell text lives in the document's flat paragraph list: every paragraph of
//! a cell carries that cell's [`CellRef`], and a table's paragraphs are
//! consecutive, row by row, cell by cell. [`Table`] holds the rest (grid,
//! rows, cell properties), keyed by the table id in the `CellRef`. Property
//! XML that Kantor doesn't edit (`w:tblPr`, `w:trPr`, `w:tcPr`) is kept
//! verbatim and parsed only for what layout needs.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::model::Rgb;

/// The cell a paragraph belongs to: table id, row index, cell index in the
/// row (not the grid column: see [`Table::grid_col`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CellRef {
    pub table: u32,
    pub row: u32,
    pub cell: u32,
}

/// Vertical merge (`w:vMerge`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VMerge {
    #[default]
    None,
    /// First cell of a vertically merged group.
    Restart,
    /// Covered by the cell above.
    Continue,
}

/// One border line. Every style is drawn as a single line for now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Border {
    /// `false` for `none` / `nil`.
    pub on: bool,
    /// Width in eighths of a point (`w:sz`).
    pub size: u32,
    /// `None` = automatic (black).
    pub color: Option<Rgb>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Borders {
    pub top: Option<Border>,
    pub left: Option<Border>,
    pub bottom: Option<Border>,
    pub right: Option<Border>,
    pub inside_h: Option<Border>,
    pub inside_v: Option<Border>,
}

impl Borders {
    /// Sides set in `o` replace the ones here.
    pub fn overlay(&mut self, o: &Borders) {
        macro_rules! take {
            ($($f:ident),*) => {$( if o.$f.is_some() { self.$f = o.$f; } )*};
        }
        take!(top, left, bottom, right, inside_h, inside_v);
    }

    /// A single thin line on every side (new tables).
    pub fn grid() -> Self {
        let b = Some(Border {
            on: true,
            size: 4,
            color: None,
        });
        Self {
            top: b,
            left: b,
            bottom: b,
            right: b,
            inside_h: b,
            inside_v: b,
        }
    }
}

/// Cell padding in twips (`w:tblCellMar`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CellMargins {
    pub top: u32,
    pub left: u32,
    pub bottom: u32,
    pub right: u32,
}

impl Default for CellMargins {
    /// Word's defaults: 0.19 cm left and right.
    fn default() -> Self {
        Self {
            top: 0,
            left: 108,
            bottom: 0,
            right: 108,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Cell {
    /// The original `<w:tcPr>…</w:tcPr>`.
    pub tc_pr: Option<Arc<str>>,
    /// Grid columns covered (`w:gridSpan`).
    pub span: u32,
    pub vmerge: VMerge,
    /// Preferred width in twips (`w:tcW` of type `dxa`).
    pub width: Option<u32>,
    pub borders: Borders,
}

impl Cell {
    pub fn new() -> Self {
        Self {
            span: 1,
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Row {
    /// The original `<w:trPr>…</w:trPr>`.
    pub tr_pr: Option<Arc<str>>,
    /// The original `<w:tblPrEx>…</w:tblPrEx>`.
    pub tbl_pr_ex: Option<Arc<str>>,
    /// Height in twips and whether it's exact (else "at least").
    pub height: Option<(u32, bool)>,
    /// Repeated on every page (`w:tblHeader`).
    pub header: bool,
    /// Kept on one page (`w:cantSplit`); other rows break across pages.
    pub cant_split: bool,
    pub cells: Vec<Cell>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Table {
    /// The original `<w:tblPr>…</w:tblPr>`.
    pub tbl_pr: Option<Arc<str>>,
    /// Grid column widths in twips (`w:tblGrid`).
    pub grid: Vec<u32>,
    pub rows: Vec<Row>,
    /// Table style id (`w:tblStyle`).
    pub style: Option<String>,
    /// Direct borders (`w:tblBorders`); the style's apply underneath.
    pub borders: Borders,
    /// Left indent in twips (`w:tblInd`).
    pub indent: i32,
    pub cell_margins: Option<CellMargins>,
}

pub type Tables = BTreeMap<u32, Table>;

impl Table {
    /// A new `rows` × `cols` table with single borders, `width` twips wide.
    pub fn new(rows: usize, cols: usize, width: u32) -> Self {
        let cols = cols.max(1);
        let w = width / cols as u32;
        let tbl_pr = format!(
            "<w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblBorders>{}</w:tblBorders>\
             <w:tblLook w:val=\"04A0\" w:firstRow=\"1\" w:lastRow=\"0\" w:firstColumn=\"1\" \
             w:lastColumn=\"0\" w:noHBand=\"0\" w:noVBand=\"1\"/></w:tblPr>",
            ["top", "left", "bottom", "right", "insideH", "insideV"]
                .iter()
                .map(|s| {
                    format!("<w:{s} w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>")
                })
                .collect::<String>()
        );
        Self {
            tbl_pr: Some(tbl_pr.into()),
            grid: vec![w; cols],
            rows: (0..rows.max(1))
                .map(|_| Self::new_row(&vec![w; cols]))
                .collect(),
            style: None,
            borders: Borders::grid(),
            indent: 0,
            cell_margins: None,
        }
    }

    /// A row of plain cells with the given widths (twips).
    pub fn new_row(widths: &[u32]) -> Row {
        Row {
            cells: widths.iter().map(|&w| Self::new_cell(w, 1)).collect(),
            ..Row::default()
        }
    }

    pub fn new_cell(width: u32, span: u32) -> Cell {
        let span_xml = if span > 1 {
            format!("<w:gridSpan w:val=\"{span}\"/>")
        } else {
            String::new()
        };
        Cell {
            tc_pr: Some(
                format!("<w:tcPr><w:tcW w:w=\"{width}\" w:type=\"dxa\"/>{span_xml}</w:tcPr>")
                    .into(),
            ),
            span,
            vmerge: VMerge::None,
            width: Some(width),
            borders: Borders::default(),
        }
    }

    /// First grid column of cell `cell` in row `row`.
    pub fn grid_col(&self, row: usize, cell: usize) -> usize {
        self.rows
            .get(row)
            .map(|r| {
                r.cells
                    .iter()
                    .take(cell)
                    .map(|c| c.span.max(1) as usize)
                    .sum()
            })
            .unwrap_or(0)
    }

    /// Number of grid columns (the grid, or the widest row).
    pub fn columns(&self) -> usize {
        let widest = self
            .rows
            .iter()
            .map(|r| {
                r.cells
                    .iter()
                    .map(|c| c.span.max(1) as usize)
                    .sum::<usize>()
            })
            .max()
            .unwrap_or(0);
        self.grid.len().max(widest)
    }

    /// Grid column widths in twips, filling gaps from cell widths or an even
    /// split of `available`.
    pub fn column_widths(&self, available: u32) -> Vec<u32> {
        let n = self.columns().max(1);
        let mut w: Vec<Option<u32>> = (0..n)
            .map(|i| self.grid.get(i).copied().filter(|&v| v > 0))
            .collect();
        if w.iter().any(Option::is_none) {
            for (ri, row) in self.rows.iter().enumerate() {
                for (ci, cell) in row.cells.iter().enumerate() {
                    let g = self.grid_col(ri, ci);
                    if cell.span <= 1
                        && let (Some(slot), Some(cw)) = (w.get_mut(g), cell.width)
                        && slot.is_none()
                    {
                        *slot = Some(cw);
                    }
                }
            }
        }
        let known: u32 = w.iter().flatten().sum();
        let missing = w.iter().filter(|v| v.is_none()).count() as u32;
        let fill = available
            .saturating_sub(known)
            .checked_div(missing)
            .map_or(0, |f| f.max(400));
        w.into_iter().map(|v| v.unwrap_or(fill)).collect()
    }

    /// Whether any cell spans columns or rows (column edits are refused then).
    pub fn has_merges(&self) -> bool {
        self.rows
            .iter()
            .flat_map(|r| &r.cells)
            .any(|c| c.span > 1 || c.vmerge != VMerge::None)
    }
}

/// Remove every `<w:name …/>` or `<w:name …>…</w:name>` from a property
/// element's XML (e.g. `vMerge` from a copied `w:tcPr`).
pub fn strip_element(xml: &str, name: &str) -> String {
    let mut out = xml.to_owned();
    let open = format!("<w:{name}");
    while let Some(start) = out.find(&open) {
        let after = start + open.len();
        // Not a longer name (`<w:vMergeX`).
        if !out[after..].starts_with(['/', '>', ' ']) {
            break;
        }
        let tag_end = match out[start..].find('>') {
            Some(i) => start + i + 1,
            None => break,
        };
        let end = if out[..tag_end].ends_with("/>") {
            tag_end
        } else {
            let close = format!("</w:{name}>");
            match out[tag_end..].find(&close) {
                Some(i) => tag_end + i + close.len(),
                None => break,
            }
        };
        out.replace_range(start..end, "");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_elements() {
        let x = "<w:tcPr><w:tcW w:w=\"10\"/><w:vMerge w:val=\"restart\"/><w:vAlign w:val=\"top\"/></w:tcPr>";
        assert_eq!(
            strip_element(x, "vMerge"),
            "<w:tcPr><w:tcW w:w=\"10\"/><w:vAlign w:val=\"top\"/></w:tcPr>"
        );
        assert_eq!(
            strip_element("<w:tcPr><w:vMerge/></w:tcPr>", "vMerge"),
            "<w:tcPr></w:tcPr>"
        );
    }

    #[test]
    fn new_table_geometry() {
        let t = Table::new(2, 3, 9000);
        assert_eq!(t.columns(), 3);
        assert_eq!(t.grid_col(1, 2), 2);
        assert_eq!(t.column_widths(9000), vec![3000; 3]);
        assert!(!t.has_merges());
    }
}
