//! Page Break Preview: where a sheet's pages fall, as Excel shows them in
//! that view. The pages come from the same plan as printing ([`Plan`]), so
//! the view and the PDF always agree.

use sheets_engine::{CellRange, Workbook};

use crate::{Options, Plan, Shaper};

/// One printed range and its pages.
#[derive(Debug, Clone, PartialEq)]
pub struct PageArea {
    pub range: CellRange,
    /// Column and row bands (inclusive), as the pages split the range.
    pub cols: Vec<(u32, u32)>,
    pub rows: Vec<(u32, u32)>,
    /// Each page: `(column band, row band, number)`, numbered from 1 in
    /// printing order across the sheet.
    pub pages: Vec<(usize, usize, usize)>,
}

/// A page break inside a printed range: a new page starts at `at` (a row
/// or column), set by the user (`manual`, solid in Excel) or by the page
/// size (dashed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageBreak {
    pub at: u32,
    pub manual: bool,
}

/// Where a sheet's pages are.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BreakMap {
    pub areas: Vec<PageArea>,
    /// The sheet has a print area (else the cells in use print).
    pub print_area: bool,
    /// Manual breaks count (Fit to pages leaves them out, as Excel does).
    pub manual_breaks: bool,
    pub manual_rows: Vec<u32>,
    pub manual_cols: Vec<u32>,
}

impl BreakMap {
    /// The pages of `sheet` as it prints now.
    pub fn new(book: &Workbook, sheet: u32, shaper: &mut Shaper) -> BreakMap {
        let opts = Options {
            sheets: Some(vec![sheet]),
            ..Options::default()
        };
        let plan = Plan::new(book, &opts, shaper);
        let Some(sp) = plan.sheets.first() else {
            return BreakMap::default();
        };
        let mut areas: Vec<PageArea> = sp
            .areas
            .iter()
            .map(|(range, grid)| PageArea {
                range: *range,
                cols: grid.cols.clone(),
                rows: grid.rows.clone(),
                pages: Vec::new(),
            })
            .collect();
        let mut n = 0;
        for p in plan.pages.iter().filter(|p| p.notes.is_none()) {
            n += 1;
            if let Some(a) = areas.get_mut(p.area) {
                a.pages.push((p.cols, p.rows, n));
            }
        }
        BreakMap {
            areas,
            print_area: !sp.print.print_area.is_empty(),
            manual_breaks: sp.print.setup.fit.is_none(),
            manual_rows: sp.print.row_breaks.clone(),
            manual_cols: sp.print.col_breaks.clone(),
        }
    }

    /// The breaks inside area `area` between its row bands (`rows`) or its
    /// column bands.
    pub fn breaks(&self, area: usize, rows: bool) -> Vec<PageBreak> {
        let Some(a) = self.areas.get(area) else {
            return Vec::new();
        };
        let (bands, manual) = if rows {
            (&a.rows, &self.manual_rows)
        } else {
            (&a.cols, &self.manual_cols)
        };
        bands
            .iter()
            .skip(1)
            .map(|&(start, _)| PageBreak {
                at: start,
                manual: self.manual_breaks && manual.binary_search(&start).is_ok(),
            })
            .collect()
    }

    /// The page number shown over `(row, col)`, if it prints.
    pub fn page_at(&self, row: u32, col: u32) -> Option<usize> {
        self.areas.iter().find_map(|a| {
            let c = a.cols.iter().position(|b| (b.0..=b.1).contains(&col))?;
            let r = a.rows.iter().position(|b| (b.0..=b.1).contains(&row))?;
            a.pages.iter().find(|p| p.0 == c && p.1 == r).map(|p| p.2)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_and_breaks_of_a_sheet() {
        let mut wb = Workbook::new_empty().unwrap();
        // 120 rows print on more than one Letter page at 100 %.
        for r in 0..120 {
            wb.set_input(0, r, 0, &format!("{r}")).unwrap();
        }
        wb.set_input(0, 0, 3, "x").unwrap();
        let mut shaper = Shaper::new();
        let map = BreakMap::new(&wb, 0, &mut shaper);
        assert_eq!(map.areas.len(), 1);
        assert!(!map.print_area);
        let a = &map.areas[0];
        assert_eq!(a.range, CellRange::from_corners((0, 0), (119, 3)));
        assert!(a.rows.len() >= 2, "{:?}", a.rows);
        let auto = map.breaks(0, true);
        assert!(auto.iter().all(|b| !b.manual));
        assert_eq!(map.page_at(0, 0), Some(1));
        assert_eq!(map.page_at(119, 0), Some(a.rows.len()));
        assert_eq!(map.page_at(200, 0), None);

        // A manual break at row 10 (0-based) starts page 2 there.
        wb.insert_page_break(0, Some(10), None).unwrap();
        let map = BreakMap::new(&wb, 0, &mut shaper);
        let b = map.breaks(0, true);
        assert_eq!(
            b[0],
            PageBreak {
                at: 10,
                manual: true
            }
        );
        assert!(b[1..].iter().all(|b| !b.manual));
        assert_eq!(map.page_at(9, 0), Some(1));
        assert_eq!(map.page_at(10, 0), Some(2));

        // A print area: only it prints.
        let mut p = wb.print_settings(0);
        p.print_area = vec![CellRange::from_corners((2, 0), (5, 1))];
        wb.set_print_settings(0, p).unwrap();
        let map = BreakMap::new(&wb, 0, &mut shaper);
        assert!(map.print_area);
        assert_eq!(map.areas[0].range, CellRange::from_corners((2, 0), (5, 1)));
        assert_eq!(map.page_at(0, 0), None);
        assert_eq!(map.page_at(3, 1), Some(1));
    }
}
