//! The data a model is estimated on, read from the workbook: the first row
//! of the range names the columns, each row below is a case. Empty cells
//! and text are missing values. Shared with `kantor pls` and the MCP tools
//! (`crates/kantor/src/agent/pls.rs`), which open the file first.

use anyhow::{Result, anyhow, bail};
use kantor_pls::Data;
use sheets_engine::{CellRange, Workbook};

/// The data read and what was noticed on the way.
pub struct Loaded {
    pub data: Data,
    pub sheet: String,
    /// The cells read, `A1:K251`.
    pub range: String,
    /// Columns with text in them (read as missing values): name → count.
    pub text_cells: Vec<(String, usize)>,
}

/// `A`, `B`, … `AA` for column `c` (0-based).
pub fn col_name(mut c: u32) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (c % 26) as u8);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

/// Read `range` of `sheet` (cut to the sheet's used area).
pub fn read(book: &Workbook, sheet: u32, range: CellRange) -> Result<Loaded> {
    let (rows, cols) = book.used_size(sheet);
    let (top, left) = (range.top, range.left);
    let bottom = range.bottom.min(rows.saturating_sub(1));
    let right = range.right.min(cols.saturating_sub(1));
    if rows == 0 || bottom <= top || right < left {
        bail!("the data needs a header row and at least one row below it");
    }
    let mut names: Vec<String> = Vec::new();
    for c in left..=right {
        let mut name = book.display_value(sheet, top, c).trim().to_owned();
        if name.is_empty() {
            name = col_name(c);
        }
        if names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
            bail!("two columns are called {name}: column names must differ");
        }
        names.push(name);
    }
    let mut text = vec![0usize; names.len()];
    let mut out = Vec::new();
    for r in top + 1..=bottom {
        let mut row = Vec::with_capacity(names.len());
        let mut empty = true;
        for (j, c) in (left..=right).enumerate() {
            match book.number_at(sheet, r, c) {
                Some((n, _)) => {
                    empty = false;
                    row.push(n);
                }
                None => {
                    if !book.display_value(sheet, r, c).trim().is_empty() {
                        text[j] += 1;
                        empty = false;
                    }
                    row.push(f64::NAN);
                }
            }
        }
        if !empty {
            out.push(row);
        }
    }
    let text_cells = names
        .iter()
        .zip(&text)
        .filter(|(_, n)| **n > 0)
        .map(|(c, n)| (c.clone(), *n))
        .collect();
    let sheet_name = book
        .sheet_names()
        .get(sheet as usize)
        .cloned()
        .unwrap_or_default();
    Ok(Loaded {
        data: Data::new(names, out).map_err(|e| anyhow!("{e}"))?,
        sheet: sheet_name,
        range: format!(
            "{}{}:{}{}",
            col_name(left),
            top + 1,
            col_name(right),
            bottom + 1
        ),
        text_cells,
    })
}

/// The sheet called `name` (any case) and `range` (`A1:H251`).
pub fn locate(book: &Workbook, name: &str, range: &str) -> Option<(u32, CellRange)> {
    let sheet = book
        .sheet_names()
        .iter()
        .position(|s| s.eq_ignore_ascii_case(name.trim()))? as u32;
    let r = sheets_engine::formula::parse_range(&range.replace('$', ""))?;
    Some((sheet, r))
}

/// The header row's names of `range` (the indicator list).
pub fn column_names(book: &Workbook, sheet: u32, range: CellRange) -> Vec<String> {
    let (_, cols) = book.used_size(sheet);
    let right = range.right.min(cols.saturating_sub(1));
    (range.left..=right)
        .map(|c| {
            let name = book.display_value(sheet, range.top, c).trim().to_owned();
            if name.is_empty() { col_name(c) } else { name }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn reads_a_range_with_a_header() {
        let mut b = Workbook::new_empty().unwrap();
        let cells: Vec<(u32, u32, String)> = [
            (0, 0, "x1"),
            (0, 1, "x2"),
            (0, 2, ""),
            (1, 0, "1"),
            (1, 1, "2"),
            (1, 2, "3"),
            (2, 0, "4"),
            (2, 1, "n/a"),
            (2, 2, "6"),
            (3, 0, "7"),
            (3, 1, "8"),
            (3, 2, "9"),
        ]
        .iter()
        .map(|(r, c, v)| (*r, *c, (*v).to_owned()))
        .collect();
        b.set_inputs(0, &cells).unwrap();
        let (sheet, r) = locate(&b, "sheet1", "$A$1:$C$40").unwrap();
        let l = read(&b, sheet, r).unwrap();
        assert_eq!(l.data.names(), ["x1", "x2", "C"]);
        assert_eq!(l.data.rows(), 3);
        assert!(l.data.get(1, 1).is_nan());
        assert_eq!(l.text_cells, vec![("x2".to_owned(), 1)]);
        assert_eq!(l.range, "A1:C4");
        assert_eq!(column_names(&b, sheet, r), ["x1", "x2", "C"]);
        assert!(locate(&b, "Nope", "A1:B2").is_none());
        assert_eq!(col_name(27), "AB");
    }
}
