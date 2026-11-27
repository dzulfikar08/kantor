//! Cell hyperlinks for macros (`sheets_script`'s `Hyperlinks`): the links
//! IronCalc keeps on cells (saved as the sheet's `<hyperlinks>`), read and
//! changed as undoable edits.
//!
//! Excel's object model splits a link into an `Address` (a URL, `mailto:`
//! or file) and a `SubAddress` (a place: `Sheet2!A1` or a name). IronCalc
//! keeps an external link as `address#place` and an internal one as the
//! place alone.

use anyhow::{Result, anyhow, bail};
use ironcalc_base::types::Link;

use crate::{CellRange, Workbook};

/// `(address, sub-address, screen tip)` of a link.
fn parts(link: &Link) -> (String, String, Option<String>) {
    match link {
        Link::External { target, tooltip } => match target.split_once('#') {
            Some((a, s)) => (a.to_owned(), s.to_owned(), tooltip.clone()),
            None => (target.clone(), String::new(), tooltip.clone()),
        },
        Link::Internal { location, tooltip } => (String::new(), location.clone(), tooltip.clone()),
    }
}

fn link_of(address: &str, sub_address: &str, tip: Option<&str>) -> Option<Link> {
    let tooltip = tip.filter(|t| !t.is_empty()).map(str::to_owned);
    let (address, sub_address) = (address.trim(), sub_address.trim());
    Some(match (address.is_empty(), sub_address.is_empty()) {
        (true, true) => return None,
        (true, false) => Link::Internal {
            location: sub_address.to_owned(),
            tooltip,
        },
        (false, true) => Link::External {
            target: address.to_owned(),
            tooltip,
        },
        (false, false) => Link::External {
            target: format!("{address}#{sub_address}"),
            tooltip,
        },
    })
}

impl Workbook {
    /// The hyperlinks on the cells of `sheet`, by row then column: `(row,
    /// col, address, sub-address, screen tip)`. Links made by HYPERLINK
    /// formulas aren't included.
    pub fn cell_links(&self, sheet: u32) -> Vec<(u32, u32, String, String, Option<String>)> {
        let Ok(links) = self.model.get_links(sheet) else {
            return Vec::new();
        };
        let mut v: Vec<_> = links
            .iter()
            .filter(|((r, c), _)| *r >= 1 && *c >= 1)
            .map(|(&(r, c), l)| {
                let (a, s, t) = parts(l);
                (r as u32 - 1, c as u32 - 1, a, s, t)
            })
            .collect();
        v.sort_by_key(|l| (l.0, l.1));
        v
    }

    /// The hyperlink on cell `(row, col)`: `(address, sub-address, screen
    /// tip)`.
    pub fn cell_link(
        &self,
        sheet: u32,
        row: u32,
        col: u32,
    ) -> Option<(String, String, Option<String>)> {
        self.model
            .get_cell_link(sheet, row as i32 + 1, col as i32 + 1)
            .ok()
            .flatten()
            .map(|l| parts(&l))
    }

    /// Put a hyperlink on cell `(row, col)` (replacing the one it has): to
    /// `address` (a URL, `mailto:` or file), `sub_address` (a place in the
    /// workbook or in the file) or both, with an optional screen tip. With
    /// `text`, the cell shows it (its content). A cell that had no link gets
    /// the hyperlink look (underlined, theme hyperlink colour). One undo
    /// step.
    #[allow(clippy::too_many_arguments)]
    pub fn set_cell_link(
        &mut self,
        sheet: u32,
        row: u32,
        col: u32,
        address: &str,
        sub_address: &str,
        tip: Option<&str>,
        text: Option<&str>,
    ) -> Result<()> {
        let Some(link) = link_of(address, sub_address, tip) else {
            bail!("A hyperlink needs an address or a place in the document");
        };
        self.batch(|wb| {
            if let Some(text) = text
                && text != wb.input(sheet, row, col)
            {
                if text.is_empty() {
                    wb.clear_range(sheet, CellRange::from_corners((row, col), (row, col)))?;
                } else {
                    wb.set_input(sheet, row, col, text)?;
                }
            }
            // After the text: typing a URL links the cell to it by itself.
            let (r, c) = (row as i32 + 1, col as i32 + 1);
            let old = wb
                .model
                .get_cell_link(sheet, r, c)
                .map_err(|e| anyhow!("{e}"))?;
            if old.as_ref() != Some(&link) {
                wb.model
                    .set_cell_link(sheet, r, c, link, None)
                    .map_err(|e| anyhow!("{e}"))?;
                wb.history.edited();
            }
            Ok(())
        })
    }

    /// Remove the hyperlink of cell `(row, col)` (its text and look stay).
    /// One undo step. Returns whether there was one.
    pub fn remove_cell_link(&mut self, sheet: u32, row: u32, col: u32) -> Result<bool> {
        let (r, c) = (row as i32 + 1, col as i32 + 1);
        let old = self
            .model
            .get_cell_link(sheet, r, c)
            .map_err(|e| anyhow!("{e}"))?;
        if old.is_none() {
            return Ok(false);
        }
        self.model
            .delete_cell_link(sheet, r, c)
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use crate::Workbook;

    #[test]
    fn links_are_set_read_removed_and_undone() {
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_cell_link(
            0,
            0,
            0,
            "https://example.com",
            "",
            Some("Tip"),
            Some("Site"),
        )
        .unwrap();
        wb.set_cell_link(0, 2, 1, "", "Sheet1!C3", None, None)
            .unwrap();
        wb.set_cell_link(0, 3, 0, "book.xlsx", "Data!A1", None, None)
            .unwrap();
        assert_eq!(wb.display_value(0, 0, 0), "Site");
        let links = wb.cell_links(0);
        assert_eq!(links.len(), 3);
        assert_eq!(
            links[0],
            (
                0,
                0,
                "https://example.com".into(),
                String::new(),
                Some("Tip".into())
            )
        );
        assert_eq!(links[1], (2, 1, String::new(), "Sheet1!C3".into(), None));
        assert_eq!(links[2].2, "book.xlsx");
        assert_eq!(links[2].3, "Data!A1");
        // The same link again changes nothing (no undo step).
        wb.set_cell_link(0, 2, 1, "", "Sheet1!C3", None, None)
            .unwrap();
        assert!(wb.remove_cell_link(0, 2, 1).unwrap());
        assert!(!wb.remove_cell_link(0, 2, 1).unwrap());
        assert_eq!(wb.cell_links(0).len(), 2);
        assert!(wb.undo().unwrap());
        assert_eq!(wb.cell_link(0, 2, 1).unwrap().1, "Sheet1!C3");
        assert!(wb.undo().unwrap());
        assert!(wb.cell_link(0, 3, 0).is_none());
        assert!(wb.undo().unwrap());
        assert!(wb.cell_link(0, 2, 1).is_none());
        // Text and link were one step.
        assert!(wb.undo().unwrap());
        assert!(wb.cell_link(0, 0, 0).is_none());
        assert_eq!(wb.display_value(0, 0, 0), "");
    }
}
