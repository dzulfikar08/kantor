//! Cell addresses: `A1` / `$A$1:B2` / `A:C` / `1:3` text, sheet prefixes,
//! and formulas between A1 and R1C1 notation (`Range.FormulaR1C1`).

use sheets_engine::{CellRange, MAX_COLS, MAX_ROWS, column_name};

/// Column number (0-based) of letters `A`..`XFD`.
pub fn parse_col(letters: &str) -> Option<u32> {
    if letters.is_empty() || letters.len() > 3 || !letters.bytes().all(|b| b.is_ascii_alphabetic())
    {
        return None;
    }
    let n = letters.bytes().fold(0u32, |n, b| {
        n * 26 + u32::from(b.to_ascii_uppercase() - b'A' + 1)
    });
    (1..=MAX_COLS).contains(&n).then(|| n - 1)
}

fn parse_row(digits: &str) -> Option<u32> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u32 = digits.parse().ok()?;
    (1..=MAX_ROWS).contains(&n).then(|| n - 1)
}

/// `B12`, `$B$12`: `(row, col)` 0-based.
pub fn parse_cell(s: &str) -> Option<(u32, u32)> {
    let s = s.trim().replace('$', "");
    let split = s.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = s.split_at(split);
    Some((parse_row(digits)?, parse_col(letters)?))
}

/// One area: `A1`, `A1:B2`, `A:C`, `3:5` (`$` anywhere).
pub fn parse_area(s: &str) -> Option<CellRange> {
    let s = s.trim();
    match s.split_once(':') {
        None => parse_cell(s).map(|c| CellRange::from_corners(c, c)),
        Some((a, b)) => {
            let (a, b) = (a.replace('$', ""), b.replace('$', ""));
            if let (Some(c1), Some(c2)) = (parse_col(&a), parse_col(&b)) {
                return Some(CellRange::columns(c1.min(c2), c1.max(c2)));
            }
            if let (Some(r1), Some(r2)) = (parse_row(&a), parse_row(&b)) {
                return Some(CellRange::rows(r1.min(r2), r1.max(r2)));
            }
            Some(CellRange::from_corners(parse_cell(&a)?, parse_cell(&b)?))
        }
    }
}

/// `'My Sheet'!A1` → (`My Sheet`, `A1`); `[Book1.xlsm]Data!A1` → (`Data`,
/// `A1`); `A1` → (`None`, `A1`).
pub fn split_sheet(s: &str) -> (Option<String>, &str) {
    let s = s.trim();
    let Some(bang) = s.rfind('!') else {
        return (None, s);
    };
    let (sheet, cells) = (&s[..bang], &s[bang + 1..]);
    let sheet = sheet.trim();
    let sheet = match sheet.strip_prefix('\'').and_then(|t| t.strip_suffix('\'')) {
        Some(q) => q.replace("''", "'"),
        None => sheet.to_owned(),
    };
    // A workbook prefix: `[Book1.xlsm]Sheet1`.
    let sheet = match sheet.rfind(']') {
        Some(i) => sheet[i + 1..].to_owned(),
        None => sheet,
    };
    (Some(sheet), cells)
}

/// A sheet name as a formula writes it (quoted when needed).
pub fn sheet_ref(name: &str) -> String {
    sheets_engine::quote_sheet_name(name)
}

/// `$A$1`, `A$1`…
pub fn cell_text(row: u32, col: u32, abs_row: bool, abs_col: bool) -> String {
    format!(
        "{}{}{}{}",
        if abs_col { "$" } else { "" },
        column_name(col),
        if abs_row { "$" } else { "" },
        row + 1
    )
}

/// An area in A1 notation (`$A$1:$B$2`, `$A:$C`, `$3:$5`).
pub fn area_text(r: CellRange, abs_row: bool, abs_col: bool) -> String {
    let d = |b: bool| if b { "$" } else { "" };
    if r.is_whole_columns() {
        return format!(
            "{}{}:{}{}",
            d(abs_col),
            column_name(r.left),
            d(abs_col),
            column_name(r.right)
        );
    }
    if r.is_whole_rows() {
        return format!("{}{}:{}{}", d(abs_row), r.top + 1, d(abs_row), r.bottom + 1);
    }
    let a = cell_text(r.top, r.left, abs_row, abs_col);
    if r.is_single() {
        return a;
    }
    format!("{a}:{}", cell_text(r.bottom, r.right, abs_row, abs_col))
}

/// An area in R1C1 notation, relative parts against (`row`, `col`).
pub fn area_text_r1c1(r: CellRange, abs_row: bool, abs_col: bool, row: u32, col: u32) -> String {
    let rp = |n: u32| {
        if abs_row {
            format!("R{}", n + 1)
        } else if n == row {
            "R".to_owned()
        } else {
            format!("R[{}]", i64::from(n) - i64::from(row))
        }
    };
    let cp = |n: u32| {
        if abs_col {
            format!("C{}", n + 1)
        } else if n == col {
            "C".to_owned()
        } else {
            format!("C[{}]", i64::from(n) - i64::from(col))
        }
    };
    if r.is_whole_columns() {
        return if r.left == r.right {
            cp(r.left)
        } else {
            format!("{}:{}", cp(r.left), cp(r.right))
        };
    }
    if r.is_whole_rows() {
        return if r.top == r.bottom {
            rp(r.top)
        } else {
            format!("{}:{}", rp(r.top), rp(r.bottom))
        };
    }
    let a = format!("{}{}", rp(r.top), cp(r.left));
    if r.is_single() {
        return a;
    }
    format!("{a}:{}{}", rp(r.bottom), cp(r.right))
}

fn ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.' || c == '$'
}

/// Copies string literals and quoted sheet names through unchanged, and
/// hands every other run of text to `f`.
fn outside_quotes(formula: &str, mut f: impl FnMut(&str, &mut String)) -> String {
    let mut out = String::with_capacity(formula.len() + 8);
    let mut plain = String::new();
    let mut chars = formula.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' || c == '\'' {
            f(&plain, &mut out);
            plain.clear();
            out.push(c);
            while let Some(d) = chars.next() {
                out.push(d);
                if d == c {
                    if chars.peek() == Some(&c) {
                        out.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
            }
        } else {
            plain.push(c);
        }
    }
    f(&plain, &mut out);
    out
}

/// A formula in A1 notation written in R1C1 relative to (`row`, `col`).
pub fn a1_to_r1c1(formula: &str, row: u32, col: u32) -> String {
    outside_quotes(formula, |text, out| {
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let start_ok = i == 0 || !ident_char(chars[i - 1]);
            if start_ok && (chars[i] == '$' || chars[i].is_ascii_alphanumeric()) {
                let mut j = i;
                while j < chars.len() && ident_char(chars[j]) && chars[j] != '.' {
                    j += 1;
                }
                let word: String = chars[i..j].iter().collect();
                let next = chars.get(j).copied();
                // `A1:B2`, `A:C`, `1:3`.
                if next == Some(':') {
                    let mut k = j + 1;
                    while k < chars.len() && ident_char(chars[k]) && chars[k] != '.' {
                        k += 1;
                    }
                    let other: String = chars[j + 1..k].iter().collect();
                    let after = chars.get(k).copied();
                    if after != Some('(')
                        && let Some(text) = range_r1c1(&word, &other, row, col)
                    {
                        out.push_str(&text);
                        i = k;
                        continue;
                    }
                }
                if next != Some('(')
                    && next != Some('!')
                    && let Some((r, c)) = cell_parts(&word)
                {
                    let range = CellRange::from_corners((r.0, c.0), (r.0, c.0));
                    out.push_str(&area_text_r1c1(range, r.1, c.1, row, col));
                } else {
                    out.push_str(&word);
                }
                i = j;
                continue;
            }
            out.push(chars[i]);
            i += 1;
        }
    })
}

/// `$B12` → ((row 11, relative), (col 1, absolute)).
fn cell_parts(word: &str) -> Option<((u32, bool), (u32, bool))> {
    let abs_col = word.starts_with('$');
    let rest = word.trim_start_matches('$');
    let split = rest.find(|c: char| c == '$' || c.is_ascii_digit())?;
    let (letters, digits) = rest.split_at(split);
    let abs_row = digits.starts_with('$');
    let digits = digits.trim_start_matches('$');
    Some((
        (parse_row(digits)?, abs_row),
        (parse_col(letters)?, abs_col),
    ))
}

fn range_r1c1(a: &str, b: &str, row: u32, col: u32) -> Option<String> {
    let abs = |s: &str| s.starts_with('$');
    if let (Some(c1), Some(c2)) = (
        parse_col(a.trim_start_matches('$')),
        parse_col(b.trim_start_matches('$')),
    ) {
        let left = area_text_r1c1(CellRange::columns(c1, c1), false, abs(a), row, col);
        let right = area_text_r1c1(CellRange::columns(c2, c2), false, abs(b), row, col);
        return Some(format!("{left}:{right}"));
    }
    if let (Some(r1), Some(r2)) = (
        parse_row(a.trim_start_matches('$')),
        parse_row(b.trim_start_matches('$')),
    ) {
        let top = area_text_r1c1(CellRange::rows(r1, r1), abs(a), false, row, col);
        let bottom = area_text_r1c1(CellRange::rows(r2, r2), abs(b), false, row, col);
        return Some(format!("{top}:{bottom}"));
    }
    let (r1, c1) = cell_parts(a)?;
    let (r2, c2) = cell_parts(b)?;
    let p = |r: (u32, bool), c: (u32, bool)| {
        area_text_r1c1(
            CellRange::from_corners((r.0, c.0), (r.0, c.0)),
            r.1,
            c.1,
            row,
            col,
        )
    };
    Some(format!("{}:{}", p(r1, c1), p(r2, c2)))
}

/// One R or C part at `chars[i]` (`R`, `R3`, `R[-2]`): the number (0-based,
/// absolute or relative to `base`), whether absolute, and where it ends.
fn rc_part(chars: &[char], i: usize, letter: char, base: u32) -> Option<(u32, bool, usize)> {
    if chars.get(i).map(char::to_ascii_uppercase) != Some(letter) {
        return None;
    }
    let mut j = i + 1;
    if chars.get(j) == Some(&'[') {
        let mut k = j + 1;
        while k < chars.len() && chars[k] != ']' {
            k += 1;
        }
        let n: i64 = chars[j + 1..k]
            .iter()
            .collect::<String>()
            .trim()
            .parse()
            .ok()?;
        let v = i64::from(base) + n;
        if v < 0 {
            return None;
        }
        return Some((v as u32, false, k + 1));
    }
    let digits_start = j;
    while j < chars.len() && chars[j].is_ascii_digit() {
        j += 1;
    }
    if j == digits_start {
        return Some((base, false, j));
    }
    let n: u32 = chars[digits_start..j]
        .iter()
        .collect::<String>()
        .parse()
        .ok()?;
    if n == 0 {
        return None;
    }
    Some((n - 1, true, j))
}

/// A formula in R1C1 notation written in A1 for cell (`row`, `col`).
pub fn r1c1_to_a1(formula: &str, row: u32, col: u32) -> String {
    outside_quotes(formula, |text, out| {
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let start_ok = i == 0 || !ident_char(chars[i - 1]);
            if start_ok {
                let r = rc_part(&chars, i, 'R', row);
                let after_r = r.map_or(i, |p| p.2);
                let c = rc_part(&chars, after_r, 'C', col);
                let end = c.map_or(after_r, |p| p.2);
                let next = chars.get(end).copied();
                let clean_end =
                    end > i && !next.is_some_and(|n| ident_char(n) || n == '(' || n == '[');
                if clean_end && (r.is_some() || c.is_some()) {
                    let prev_colon = i > 0 && chars[i - 1] == ':';
                    let next_colon = next == Some(':');
                    let d = |b: bool| if b { "$" } else { "" };
                    match (r, c) {
                        (Some((rn, ra, _)), Some((cn, ca, _))) => {
                            out.push_str(&format!(
                                "{}{}{}{}",
                                d(ca),
                                column_name(cn),
                                d(ra),
                                rn + 1
                            ));
                        }
                        (Some((rn, ra, _)), None) => {
                            let t = format!("{}{}", d(ra), rn + 1);
                            if prev_colon || next_colon {
                                out.push_str(&t);
                            } else {
                                out.push_str(&format!("{t}:{t}"));
                            }
                        }
                        (None, Some((cn, ca, _))) => {
                            let t = format!("{}{}", d(ca), column_name(cn));
                            if prev_colon || next_colon {
                                out.push_str(&t);
                            } else {
                                out.push_str(&format!("{t}:{t}"));
                            }
                        }
                        (None, None) => {}
                    }
                    i = end;
                    continue;
                }
                // Skip the rest of a word (a function or a name).
                if chars[i].is_alphanumeric() || chars[i] == '_' {
                    let mut j = i;
                    while j < chars.len() && ident_char(chars[j]) {
                        j += 1;
                    }
                    out.extend(&chars[i..j]);
                    i = j;
                    continue;
                }
            }
            out.push(chars[i]);
            i += 1;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn areas() {
        assert_eq!(parse_cell("$B$12"), Some((11, 1)));
        assert_eq!(
            parse_area("A1:C3"),
            Some(CellRange::from_corners((0, 0), (2, 2)))
        );
        assert_eq!(parse_area("B:D"), Some(CellRange::columns(1, 3)));
        assert_eq!(parse_area("$2:$4"), Some(CellRange::rows(1, 3)));
        assert_eq!(parse_area("XFE1"), None);
        assert_eq!(
            split_sheet("'My ''Data'''!A1").0.as_deref(),
            Some("My 'Data'")
        );
        assert_eq!(
            split_sheet("[Book1.xlsm]Data!A1:B2").0.as_deref(),
            Some("Data")
        );
        assert_eq!(
            area_text(CellRange::from_corners((0, 0), (1, 1)), true, true),
            "$A$1:$B$2"
        );
        assert_eq!(area_text(CellRange::columns(0, 0), true, true), "$A:$A");
    }

    #[test]
    fn r1c1_both_ways() {
        // In C5 (row 4, col 2).
        assert_eq!(a1_to_r1c1("=A1+$B$2", 4, 2), "=R[-4]C[-2]+R2C2");
        assert_eq!(a1_to_r1c1("=SUM(C1:C4)", 4, 2), "=SUM(R[-4]C:R[-1]C)");
        assert_eq!(
            a1_to_r1c1("=LOG10(A1)&\"A1\"", 4, 2),
            "=LOG10(R[-4]C[-2])&\"A1\""
        );
        assert_eq!(r1c1_to_a1("=R[-4]C[-2]+R2C2", 4, 2), "=A1+$B$2");
        assert_eq!(r1c1_to_a1("=SUM(R1C:R[-1]C)", 4, 2), "=SUM(C$1:C4)");
        assert_eq!(r1c1_to_a1("=ROUND(RC[-1],2)", 4, 2), "=ROUND(B5,2)");
        assert_eq!(r1c1_to_a1("=SUM(C1)", 0, 0), "=SUM($A:$A)");
        assert_eq!(r1c1_to_a1("=\"RC\"&R1C1", 0, 0), "=\"RC\"&$A$1");
    }
}
