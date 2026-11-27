//! What macros need of pivot tables beyond the PivotTable Fields list's
//! operations (`sheets_script`'s `PivotTable` object): renaming one, and
//! where its header and data cells start in its body (`DataBodyRange`,
//! `RowRange`, `ColumnRange`).

use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};

use crate::Workbook;
use crate::pivot::Rebuild;
use crate::slicer::CacheSource;

impl Workbook {
    /// Where the parts of the body of the pivot table at `(row, col)` of
    /// `sheet` start, as offsets from its top-left cell: Excel's
    /// `(firstHeaderRow, firstDataRow, firstDataCol)`.
    pub fn pivot_body_offsets(&self, sheet: u32, row: u32, col: u32) -> Option<(u32, u32, u32)> {
        let placed = self.placed_pivots(sheet);
        let p = placed.iter().find(|p| p.contains(row, col))?;
        Some((
            p.def.first_header_row,
            p.def.first_data_row,
            p.def.first_data_col,
        ))
    }

    /// Whether a slicer or a timeline is connected to the pivot table
    /// `name` on the sheet with `sheet_id` (they name it in their caches).
    fn pivot_has_slicers(&self, sheet_id: u32, name: &str) -> bool {
        let names = |list: &[(u32, String)]| {
            list.iter()
                .any(|(tab, n)| *tab == sheet_id && n.eq_ignore_ascii_case(name))
        };
        let file = &self.pivots.slicers;
        let file_slicer = file.caches.iter().any(|c| match &c.source {
            CacheSource::Pivot { pivots, .. } => names(pivots),
            _ => false,
        });
        let file_timeline = file.timeline_caches.iter().any(|c| names(&c.pivots));
        let new_slicer = self.new_slicers().iter().any(|n| match &n.cache.source {
            CacheSource::Pivot { pivots, .. } => names(pivots),
            _ => false,
        });
        let new_timeline = self.new_timelines().iter().any(|n| names(&n.cache.pivots));
        file_slicer || file_timeline || new_slicer || new_timeline
    }

    /// Rename the pivot table at `(row, col)` of `sheet` (one undo step; it
    /// is laid out again from its source, as a refresh does). The name must
    /// not be another pivot table's on the same sheet. A pivot table with
    /// slicers or timelines keeps its name (they refer to it by name).
    pub fn rename_pivot(&mut self, sheet: u32, row: u32, col: u32, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            bail!("a PivotTable needs a name");
        }
        let placed = self.placed_pivots(sheet);
        let p = placed
            .iter()
            .find(|p| p.contains(row, col))
            .ok_or_else(|| anyhow!("there is no PivotTable at {}", crate::cell_name(row, col)))?;
        if p.def.name == name {
            return Ok(());
        }
        if placed
            .iter()
            .any(|o| o.index != p.index && o.def.name.eq_ignore_ascii_case(name))
        {
            bail!("A PivotTable report with the name {name:?} already exists on this sheet");
        }
        if self.pivot_has_slicers(p.def.sheet_id, &p.def.name) {
            bail!(
                "the PivotTable {} has slicers or timelines connected: Kantor can't rename it",
                p.def.name
            );
        }
        let index = p.index;
        let mut def = (*p.def).clone();
        def.name = name.to_owned();
        let cache = def.cache;
        self.rebuild_pivots(
            &[cache],
            Rebuild {
                defs: HashMap::from([(index, def)]),
                ..Default::default()
            },
        )?;
        Ok(())
    }
}
