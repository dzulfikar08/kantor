//! What the formula assistant may see of a sheet: the header row and a few
//! sample rows near the selection, capped in rows, columns and cell length.
//! Never the whole sheet.

/// Most data rows sent.
pub const MAX_SAMPLE_ROWS: usize = 20;
/// Most columns sent.
pub const MAX_COLUMNS: u32 = 26;
/// Longer cell values are cut (with "…").
pub const MAX_CELL_CHARS: usize = 40;
/// Rows searched from the top for the header row.
const HEADER_SCAN_ROWS: u32 = 10;

/// A sample of one sheet. Rows and columns are 0-based here; prompts show
/// them as the user sees them (row 1, column A).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SheetSample {
    pub sheet: String,
    /// The active cell, e.g. `F2`.
    pub active: String,
    /// The used range, e.g. `A1:E250`; empty for an empty sheet.
    pub used_range: String,
    /// The header row (0-based), if the first filled row looks like one.
    pub header_row: Option<u32>,
    /// First and last data row (0-based, inclusive), if there is data.
    pub data_rows: Option<(u32, u32)>,
    /// `(column letter, header text)` for each sent column.
    pub columns: Vec<(String, String)>,
    /// `(row, cell values in column order)`, at most [`MAX_SAMPLE_ROWS`].
    pub rows: Vec<(u32, Vec<String>)>,
}

/// Sample a sheet whose data spans `used = (rows, cols)` from A1.
/// `cell(row, col)` returns the displayed value; `col_name(col)` the letter.
pub fn sample_sheet(
    sheet: &str,
    used: (u32, u32),
    active: (u32, u32),
    col_name: impl Fn(u32) -> String,
    cell: impl Fn(u32, u32) -> String,
) -> SheetSample {
    let (used_rows, used_cols) = used;
    let active_name = format!("{}{}", col_name(active.1), active.0 + 1);
    let mut out = SheetSample {
        sheet: sheet.to_owned(),
        active: active_name,
        ..Default::default()
    };
    if used_rows == 0 || used_cols == 0 {
        return out;
    }
    out.used_range = format!("A1:{}{}", col_name(used_cols - 1), used_rows);

    // Columns: the first MAX_COLUMNS, shifted right if the active cell is
    // past them so its neighbourhood is visible.
    let ncols = used_cols.min(MAX_COLUMNS);
    let first_col = if active.1 >= ncols && active.1 < used_cols {
        (active.1 + 1).saturating_sub(ncols)
    } else {
        0
    };
    let cols: Vec<u32> = (first_col..first_col + ncols).collect();

    let first_filled = (0..used_rows.min(HEADER_SCAN_ROWS))
        .find(|&r| cols.iter().any(|&c| !cell(r, c).trim().is_empty()));
    let Some(first_filled) = first_filled else {
        return out;
    };
    let looks_like_header = {
        let values: Vec<String> = cols.iter().map(|&c| cell(first_filled, c)).collect();
        let filled: Vec<&String> = values.iter().filter(|v| !v.trim().is_empty()).collect();
        !filled.is_empty() && filled.iter().all(|v| !is_number(v))
    } && first_filled + 1 < used_rows;
    let data_start = if looks_like_header {
        out.header_row = Some(first_filled);
        first_filled + 1
    } else {
        first_filled
    };
    let data_end = used_rows - 1;
    out.data_rows = Some((data_start, data_end));

    out.columns = cols
        .iter()
        .map(|&c| {
            let header = out
                .header_row
                .map(|r| clip(&cell(r, c)))
                .unwrap_or_default();
            (col_name(c), header)
        })
        .collect();

    // Rows: a window of MAX_SAMPLE_ROWS around the active row when it's in
    // the data, else the first rows.
    let len = (data_end - data_start + 1) as usize;
    let take = len.min(MAX_SAMPLE_ROWS) as u32;
    let start = if (data_start..=data_end).contains(&active.0) {
        let half = take / 2;
        active
            .0
            .saturating_sub(half)
            .clamp(data_start, data_end + 1 - take)
    } else {
        data_start
    };
    out.rows = (start..start + take)
        .map(|r| (r, cols.iter().map(|&c| clip(&cell(r, c))).collect()))
        .collect();
    out
}

fn is_number(s: &str) -> bool {
    let t: String = s
        .trim()
        .chars()
        .filter(|c| !matches!(c, ',' | '%' | ' ' | '$'))
        .collect();
    let t = t.trim_start_matches("Rp");
    !t.is_empty() && t.parse::<f64>().is_ok()
}

/// One line, at most [`MAX_CELL_CHARS`] characters.
fn clip(s: &str) -> String {
    let one_line: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let one_line = one_line.trim();
    match one_line.char_indices().nth(MAX_CELL_CHARS) {
        Some((i, _)) => format!("{}…", &one_line[..i]),
        None => one_line.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(c: u32) -> String {
        let mut n = c + 1;
        let mut s = String::new();
        while n > 0 {
            let r = (n - 1) % 26;
            s.insert(0, (b'A' + r as u8) as char);
            n = (n - 1) / 26;
        }
        s
    }

    /// Headers in row 0 ("H0", "H1", …), values "r{row}c{col}" below.
    fn table(row: u32, c: u32) -> String {
        if row == 0 {
            format!("H{c}")
        } else {
            format!("r{row}c{c}")
        }
    }

    #[test]
    fn caps_rows_and_columns() {
        let s = sample_sheet("Data", (500, 40), (0, 0), col, table);
        assert_eq!(s.header_row, Some(0));
        assert_eq!(s.data_rows, Some((1, 499)));
        assert_eq!(s.used_range, "A1:AN500");
        assert_eq!(s.rows.len(), MAX_SAMPLE_ROWS);
        assert_eq!(s.columns.len(), MAX_COLUMNS as usize);
        assert_eq!(s.columns[1], ("B".to_owned(), "H1".to_owned()));
        assert_eq!(s.rows[0].0, 1, "first data row when selection is above");
        assert!(s.rows.iter().all(|(_, v)| v.len() == MAX_COLUMNS as usize));
    }

    #[test]
    fn window_follows_the_active_row_and_column() {
        let s = sample_sheet("Data", (500, 40), (300, 35), col, table);
        assert_eq!(s.active, "AJ301");
        let rows: Vec<u32> = s.rows.iter().map(|r| r.0).collect();
        assert!(rows.contains(&300));
        assert_eq!(rows.len(), MAX_SAMPLE_ROWS);
        assert_eq!(s.columns.last().unwrap().0, "AJ", "active column visible");

        // Near the bottom the window stays inside the data.
        let s = sample_sheet("Data", (500, 3), (499, 0), col, table);
        assert_eq!(s.rows.last().unwrap().0, 499);
        assert_eq!(s.rows.len(), MAX_SAMPLE_ROWS);
    }

    #[test]
    fn small_and_empty_sheets() {
        let s = sample_sheet("S", (0, 0), (0, 0), col, table);
        assert!(s.rows.is_empty() && s.columns.is_empty());
        assert_eq!(s.active, "A1");

        let s = sample_sheet("S", (4, 2), (10, 5), col, table);
        assert_eq!(s.rows.len(), 3);
        assert_eq!(s.data_rows, Some((1, 3)));
    }

    #[test]
    fn numeric_first_row_is_not_a_header() {
        let cell = |r: u32, c: u32| format!("{}", r * 10 + c);
        let s = sample_sheet("S", (5, 2), (0, 0), col, cell);
        assert_eq!(s.header_row, None);
        assert_eq!(s.data_rows, Some((0, 4)));
        assert_eq!(s.columns[0], ("A".to_owned(), String::new()));
    }

    #[test]
    fn long_cells_are_clipped() {
        let long = "x".repeat(100) + "\nsecond line";
        let cell = |r: u32, _c: u32| if r == 0 { "Note".into() } else { long.clone() };
        let s = sample_sheet("S", (2, 1), (0, 0), col, cell);
        let v = &s.rows[0].1[0];
        assert_eq!(v.chars().count(), MAX_CELL_CHARS + 1);
        assert!(v.ends_with('…') && !v.contains('\n'));
    }
}
