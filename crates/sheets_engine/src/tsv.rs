//! Tab-separated text, as spreadsheets put on the clipboard.
//!
//! Excel, LibreOffice and Google Sheets copy a range as rows separated by
//! newlines (`\r\n` on Windows) and cells separated by tabs. A cell that
//! contains a tab, a newline or a double quote is wrapped in double quotes,
//! with inner quotes doubled (`"say ""hi"""`).

/// Parse clipboard text into rows of cells.
///
/// Accepts `\n`, `\r\n` and lone `\r` line ends; a final line end does not add
/// an empty row. Rows may have different lengths. A field is quoted only if it
/// *starts* with `"`; quotes elsewhere are kept as typed. A quoted field that
/// is not closed properly keeps the rest of its text literally.
pub fn parse(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    if text.is_empty() {
        return rows;
    }
    let mut row = Vec::new();
    let mut field = String::new();
    let mut chars = text.chars().peekable();
    // At the start of a field (where an opening quote is recognised).
    let mut field_start = true;
    while let Some(ch) = chars.next() {
        match ch {
            '"' if field_start => {
                field_start = false;
                // Quoted field: read up to the closing quote.
                loop {
                    match chars.next() {
                        Some('"') if chars.peek() == Some(&'"') => {
                            chars.next();
                            field.push('"');
                        }
                        Some('"') => break,
                        Some(c) => field.push(c),
                        None => break,
                    }
                }
                // Anything between the closing quote and the next separator is
                // kept as typed (lenient, like Excel).
            }
            '\t' => {
                row.push(std::mem::take(&mut field));
                field_start = true;
            }
            '\r' | '\n' => {
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                field_start = true;
            }
            c => {
                field.push(c);
                field_start = false;
            }
        }
    }
    // The last line, unless the text ended with a line end.
    if !field_start || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

/// Format rows of cells as clipboard text: tabs between cells, `\n` between
/// rows, no final line end (like Google Sheets). Cells containing a tab, line
/// end or double quote are quoted.
pub fn format(rows: &[Vec<String>]) -> String {
    let mut out = String::new();
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        for (j, cell) in row.iter().enumerate() {
            if j > 0 {
                out.push('\t');
            }
            if cell.contains(['\t', '\n', '\r', '"']) {
                push_quoted(&mut out, cell);
            } else {
                out.push_str(cell);
            }
        }
    }
    out
}

/// Rows padded to a rectangle with every cell quoted. This is the form
/// IronCalc's `paste_csv_string` reads safely: its reader rejects rows of
/// uneven length and skips blank lines, and quoting every cell avoids both.
pub(crate) fn format_rect_quoted(rows: &[Vec<String>], width: usize) -> String {
    let mut out = String::new();
    for row in rows {
        for j in 0..width {
            if j > 0 {
                out.push('\t');
            }
            push_quoted(&mut out, row.get(j).map(String::as_str).unwrap_or(""));
        }
        out.push('\n');
    }
    out
}

fn push_quoted(out: &mut String, cell: &str) {
    out.push('"');
    out.push_str(&cell.replace('"', "\"\""));
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(r: &[&[&str]]) -> Vec<Vec<String>> {
        r.iter()
            .map(|row| row.iter().map(|s| s.to_string()).collect())
            .collect()
    }

    #[test]
    fn plain() {
        assert_eq!(parse(""), Vec::<Vec<String>>::new());
        assert_eq!(parse("a"), rows(&[&["a"]]));
        assert_eq!(parse("a\tb\nc\td"), rows(&[&["a", "b"], &["c", "d"]]));
    }

    #[test]
    fn line_ends() {
        let want = rows(&[&["a", "b"], &["c", "d"]]);
        assert_eq!(parse("a\tb\r\nc\td\r\n"), want);
        assert_eq!(parse("a\tb\nc\td\n"), want);
        assert_eq!(parse("a\tb\rc\td"), want);
    }

    #[test]
    fn empty_cells_and_rows() {
        assert_eq!(parse("\t\n"), rows(&[&["", ""]]));
        assert_eq!(parse("a\n\nb"), rows(&[&["a"], &[""], &["b"]]));
        assert_eq!(parse("a\t\tc"), rows(&[&["a", "", "c"]]));
        assert_eq!(parse("a\tb\nc"), rows(&[&["a", "b"], &["c"]]));
        assert_eq!(parse("\n"), rows(&[&[""]]));
    }

    #[test]
    fn quoted() {
        assert_eq!(
            parse("\"line 1\nline 2\"\tx\n\"a\tb\"\t\"say \"\"hi\"\"\""),
            rows(&[&["line 1\nline 2", "x"], &["a\tb", "say \"hi\""]])
        );
        // Quotes not at the start of a field are literal.
        assert_eq!(parse("5\" pipe\tb"), rows(&[&["5\" pipe", "b"]]));
        // Unterminated quote: the rest of the text is the field.
        assert_eq!(parse("\"abc\td"), rows(&[&["abc\td"]]));
        // Junk after the closing quote is kept.
        assert_eq!(parse("\"ab\"c\td"), rows(&[&["abc", "d"]]));
        assert_eq!(parse("\"\"\tx"), rows(&[&["", "x"]]));
    }

    #[test]
    fn format_round_trips() {
        let data = rows(&[
            &["Item", "=SUM(B2:B3)", ""],
            &["two\nlines", "tab\there", "say \"hi\""],
        ]);
        let text = format(&data);
        assert_eq!(
            text,
            "Item\t=SUM(B2:B3)\t\n\"two\nlines\"\t\"tab\there\"\t\"say \"\"hi\"\"\""
        );
        assert_eq!(parse(&text), data);
    }

    #[test]
    fn rect_quoted() {
        let data = rows(&[&["a", "b"], &[], &["c"]]);
        assert_eq!(
            format_rect_quoted(&data, 2),
            "\"a\"\t\"b\"\n\"\"\t\"\"\n\"c\"\t\"\"\n"
        );
    }
}
