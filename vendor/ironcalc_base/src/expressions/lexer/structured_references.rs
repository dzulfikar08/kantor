// Grammar:
// structured references -> table_name "[" arguments "]"
//  arguments -> table_reference | "["specifier"]"  "," table_reference
//  specifier > "#All"      |
//              "#This Row" |
//              "#Data"     |
//              "#Headers"  |
//              "#Totals"
// table_reference -> column_reference | range_reference
// column reference -> column_name | "["column_name"]"
// range_reference -> column_reference":"column_reference

use crate::expressions::token::TokenType;
use crate::expressions::token::{TableReference, TableSpecifier};

use super::Result;
use super::{Lexer, LexerError};

impl<'a> Lexer<'a> {
    /// KANTOR PATCH (tables): the whole bracketed part after a table name,
    /// as Excel writes it in files and as users type it:
    ///
    ///  * `T[]` (the table), `T[Col]`, `T[[Col Name]]`, `T[[A]:[B]]`
    ///  * `T[#All]`, `T[#Data]`, `T[#Headers]`, `T[#Totals]`, `T[#This Row]`
    ///  * `T[[#Totals],[Col]]`, `T[[#Headers],[#Data],[A]:[B]]`
    ///  * `T[@Col]`, `T[@[Col Name]]`, `T[@[A]:[B]]`, `T[@]`
    ///
    /// Specifiers are case-insensitive; `'` escapes `[`, `]`, `#`, `@` and
    /// `'` in a column name. Upstream read only one specifier, no `@`, and
    /// gave up on other spellings.
    pub(crate) fn consume_structured_reference(&mut self, table_name: &str) -> Result<TokenType> {
        let start = self.position;
        self.expect_char('[')?;
        // Find the matching `]`.
        let mut depth = 1;
        let mut end = self.position;
        while end < self.len {
            match self.chars[end] {
                '\'' => end += 1,
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            end += 1;
        }
        if depth != 0 || end >= self.len {
            return Err(LexerError {
                position: start,
                message: "Invalid structured reference".to_string(),
            });
        }
        let inner: Vec<char> = self.chars[self.position..end].to_vec();
        self.position = end + 1;
        let error = |message: &str| LexerError {
            position: start,
            message: message.to_string(),
        };
        let inner_text: String = inner.iter().collect();
        let trimmed = inner_text.trim();
        if trimmed.is_empty() {
            return Ok(TokenType::Ident(table_name.to_string()));
        }
        let mut specifiers: Vec<TableSpecifier> = Vec::new();
        let mut columns: Vec<String> = Vec::new();
        let mut range = false;
        let chars: Vec<char> = trimmed.chars().collect();
        if chars[0] == '@' {
            specifiers.push(TableSpecifier::ThisRow);
            let rest: String = chars[1..].iter().collect();
            let rest = rest.trim();
            if !rest.is_empty() {
                if rest.starts_with('[') {
                    let (cols, is_range) = parse_column_items(rest).ok_or_else(|| error("Invalid column"))?;
                    columns = cols;
                    range = is_range;
                } else {
                    columns.push(unescape(rest));
                }
            }
        } else if chars[0] == '#' {
            specifiers.push(parse_specifier(trimmed).ok_or_else(|| error("Invalid specifier"))?);
        } else if chars[0] == '[' {
            // A list of `[item]`, separated by commas; the last may be a range.
            let items = split_items(&chars).ok_or_else(|| error("Invalid structured reference"))?;
            for item in items {
                let item = item.trim();
                if item.starts_with("[#") || item.starts_with("#") {
                    let inner = item.trim_start_matches('[').trim_end_matches(']');
                    if !columns.is_empty() {
                        return Err(error("Specifier after a column"));
                    }
                    specifiers.push(parse_specifier(inner).ok_or_else(|| error("Invalid specifier"))?);
                } else {
                    if !columns.is_empty() {
                        return Err(error("Too many columns"));
                    }
                    let (cols, is_range) = parse_column_items(item).ok_or_else(|| error("Invalid column"))?;
                    columns = cols;
                    range = is_range;
                }
            }
        } else {
            columns.push(unescape(trimmed));
        }
        let specifier = match specifiers.as_slice() {
            [] => None,
            [one] => Some(one.clone()),
            [TableSpecifier::Headers, TableSpecifier::Data]
            | [TableSpecifier::Data, TableSpecifier::Headers] => Some(TableSpecifier::HeadersData),
            [TableSpecifier::Data, TableSpecifier::Totals]
            | [TableSpecifier::Totals, TableSpecifier::Data] => Some(TableSpecifier::DataTotals),
            _ => return Err(error("Invalid combination of specifiers")),
        };
        let table_reference = match (columns.len(), range) {
            (0, _) => None,
            (1, _) => Some(TableReference::ColumnReference(columns.remove(0))),
            (2, true) => {
                let right = columns.remove(1);
                Some(TableReference::RangeReference((columns.remove(0), right)))
            }
            _ => return Err(error("Invalid columns")),
        };
        Ok(TokenType::StructuredReference {
            table_name: table_name.to_string(),
            specifier,
            table_reference,
        })
    }
}

fn parse_specifier(text: &str) -> Option<TableSpecifier> {
    let t = text.trim().to_ascii_lowercase();
    Some(match t.as_str() {
        "#all" => TableSpecifier::All,
        "#data" => TableSpecifier::Data,
        "#headers" => TableSpecifier::Headers,
        "#totals" => TableSpecifier::Totals,
        "#this row" => TableSpecifier::ThisRow,
        _ => return None,
    })
}

/// `'x` stands for `x`.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\'' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Split `[a],[b],[c]:[d]` at the top-level commas.
fn split_items(chars: &[char]) -> Option<Vec<String>> {
    let mut items = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' => {
                current.push(c);
                i += 1;
                if i < chars.len() {
                    current.push(chars[i]);
                }
            }
            '[' => {
                depth += 1;
                current.push(c);
            }
            ']' => {
                depth -= 1;
                if depth < 0 {
                    return None;
                }
                current.push(c);
            }
            ',' if depth == 0 => items.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
        i += 1;
    }
    if depth != 0 {
        return None;
    }
    items.push(current);
    Some(items)
}

/// `[Col]` or `[A]:[B]` (also `A` without brackets): the column names and
/// whether it is a range.
fn parse_column_items(text: &str) -> Option<(Vec<String>, bool)> {
    let text = text.trim();
    let chars: Vec<char> = text.chars().collect();
    let mut names = Vec::new();
    let mut i = 0;
    loop {
        while i < chars.len() && chars[i] == ' ' {
            i += 1;
        }
        if i >= chars.len() {
            return None;
        }
        if chars[i] == '[' {
            let mut j = i + 1;
            let mut name = String::new();
            while j < chars.len() && chars[j] != ']' {
                if chars[j] == '\'' && j + 1 < chars.len() {
                    j += 1;
                }
                name.push(chars[j]);
                j += 1;
            }
            if j >= chars.len() {
                return None;
            }
            names.push(name);
            i = j + 1;
        } else {
            let mut name = String::new();
            let mut j = i;
            while j < chars.len() && chars[j] != ':' {
                if chars[j] == '\'' && j + 1 < chars.len() {
                    j += 1;
                }
                name.push(chars[j]);
                j += 1;
            }
            names.push(name.trim_end().to_string());
            i = j;
        }
        while i < chars.len() && chars[i] == ' ' {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        if chars[i] == ':' && names.len() == 1 {
            i += 1;
            continue;
        }
        return None;
    }
    let range = names.len() == 2;
    Some((names, range))
}
