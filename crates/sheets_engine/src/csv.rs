//! Delimited text (CSV / TSV): decoding, delimiter detection, parsing and writing.
//!
//! Pure text handling; turning records into cells happens in [`crate::import`].
//! Parsing follows RFC 4180 loosely, the way spreadsheet apps read CSV:
//! fields may be quoted with `"` (a doubled `""` is a literal quote, and quoted
//! fields may contain delimiters and line breaks); records end with LF, CRLF or
//! a lone CR; a quote in the middle of an unquoted field is kept as text.

use encoding_rs::{Encoding, WINDOWS_1252};

/// Delimiters [`detect_delimiter`] chooses between.
const CANDIDATES: [u8; 3] = *b",;\t";
/// How many records detection looks at.
const SAMPLE_RECORDS: usize = 50;

/// Decode file bytes to text.
///
/// A byte-order mark picks the encoding (UTF-8, UTF-16LE/BE) and is dropped.
/// Without one, valid UTF-8 is read as UTF-8 and anything else as
/// Windows-1252, which is what Excel writes for "CSV" on Western-locale Windows.
pub fn decode(bytes: &[u8]) -> String {
    if let Some((encoding, bom_len)) = Encoding::for_bom(bytes) {
        let (text, _) = encoding.decode_without_bom_handling(&bytes[bom_len..]);
        return text.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => WINDOWS_1252
            .decode_without_bom_handling(bytes)
            .0
            .into_owned(),
    }
}

/// Guess the delimiter: the candidate (`,` `;` tab) that splits the most of
/// the first records into the same number (> 1) of fields. Quoted text is
/// ignored, so `"a,b";c` counts as two `;` fields. Falls back to `,`.
pub fn detect_delimiter(text: &str) -> u8 {
    let mut best = (b',', 0usize, 0usize); // (delimiter, consistent records, fields)
    for delim in CANDIDATES {
        let counts: Vec<usize> = Records::new(text, delim)
            .take(SAMPLE_RECORDS)
            .filter(|r| !(r.len() == 1 && r[0].is_empty()))
            .map(|r| r.len())
            .collect();
        // The most common field count, and how many records have it.
        let mut mode = (0usize, 0usize);
        for &n in &counts {
            let hits = counts.iter().filter(|&&m| m == n).count();
            if hits > mode.1 || (hits == mode.1 && n > mode.0) {
                mode = (n, hits);
            }
        }
        let (fields, hits) = mode;
        if fields > 1 && (hits > best.1 || (hits == best.1 && fields > best.2)) {
            best = (delim, hits, fields);
        }
    }
    best.0
}

/// Parse `text` into records of fields.
pub fn parse(text: &str, delim: u8) -> Vec<Vec<String>> {
    Records::new(text, delim).collect()
}

/// Iterator over the records of delimited text.
struct Records<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    delim: char,
}

impl<'a> Records<'a> {
    fn new(text: &'a str, delim: u8) -> Self {
        Self {
            chars: text.chars().peekable(),
            delim: delim as char,
        }
    }
}

impl Iterator for Records<'_> {
    type Item = Vec<String>;

    fn next(&mut self) -> Option<Vec<String>> {
        // A trailing line break does not start another (empty) record.
        self.chars.peek()?;
        let mut record = Vec::new();
        let mut field = String::new();
        // Inside a quoted section of the current field.
        let mut quoted = false;
        // The current field began with a quote (so a closing quote is meaningful).
        let mut field_quoted = false;
        while let Some(c) = self.chars.next() {
            if quoted {
                if c == '"' {
                    if self.chars.peek() == Some(&'"') {
                        self.chars.next();
                        field.push('"');
                    } else {
                        quoted = false;
                    }
                } else {
                    field.push(c);
                }
                continue;
            }
            match c {
                '"' if field.is_empty() && !field_quoted => {
                    quoted = true;
                    field_quoted = true;
                }
                c if c == self.delim => {
                    record.push(std::mem::take(&mut field));
                    field_quoted = false;
                }
                '\r' => {
                    if self.chars.peek() == Some(&'\n') {
                        self.chars.next();
                    }
                    record.push(field);
                    return Some(record);
                }
                '\n' => {
                    record.push(field);
                    return Some(record);
                }
                c => field.push(c),
            }
        }
        record.push(field);
        Some(record)
    }
}

/// Append one record to `out`, quoting fields that need it, ending with CRLF
/// (RFC 4180; every spreadsheet app reads it).
pub fn write_record<S: AsRef<str>>(out: &mut String, fields: &[S], delim: u8) {
    let delim = delim as char;
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            out.push(delim);
        }
        let field = field.as_ref();
        if field.contains([delim, '"', '\r', '\n']) {
            out.push('"');
            out.push_str(&field.replace('"', "\"\""));
            out.push('"');
        } else {
            out.push_str(field);
        }
    }
    out.push_str("\r\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(fields: &[&str]) -> Vec<String> {
        fields.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_quotes_and_line_endings() {
        let text = "a,\"b,c\",\"say \"\"hi\"\"\"\r\n\"multi\nline\",x,\r\nlast,row,here";
        assert_eq!(
            parse(text, b','),
            vec![
                rec(&["a", "b,c", "say \"hi\""]),
                rec(&["multi\nline", "x", ""]),
                rec(&["last", "row", "here"]),
            ]
        );
    }

    #[test]
    fn trailing_newline_and_blank_lines() {
        assert_eq!(
            parse("a\n\nb\n", b','),
            vec![rec(&["a"]), rec(&[""]), rec(&["b"])]
        );
        assert_eq!(parse("a\rb\r", b','), vec![rec(&["a"]), rec(&["b"])]);
        assert!(parse("", b',').is_empty());
    }

    #[test]
    fn stray_quote_in_unquoted_field_is_text() {
        assert_eq!(parse("5\" pipe,x", b','), vec![rec(&["5\" pipe", "x"])]);
    }

    #[test]
    fn detects_delimiters() {
        assert_eq!(detect_delimiter("a,b,c\n1,2,3\n"), b',');
        assert_eq!(detect_delimiter("a;b;c\n1,5;2,5;3\n"), b';');
        assert_eq!(detect_delimiter("a\tb\n1\t2\n"), b'\t');
        // Commas only inside quotes don't count.
        assert_eq!(detect_delimiter("\"x, y\";z\n\"1, 2\";3\n"), b';');
        assert_eq!(detect_delimiter("single column\nonly\n"), b',');
    }

    #[test]
    fn decodes_bom_utf16_and_latin1() {
        assert_eq!(decode(b"\xEF\xBB\xBFcaf\xC3\xA9"), "café");
        assert_eq!(decode(b"\xFF\xFEa\x00,\x00b\x00"), "a,b");
        assert_eq!(decode(b"caf\xE9"), "café");
        assert_eq!(decode("plain ü".as_bytes()), "plain ü");
    }

    #[test]
    fn writes_and_reparses() {
        let rows = vec![
            rec(&["plain", "with,comma", "with \"quote\""]),
            rec(&["new\nline", "", "tab\there"]),
        ];
        for delim in *b",;\t" {
            let mut out = String::new();
            for r in &rows {
                write_record(&mut out, r, delim);
            }
            assert_eq!(parse(&out, delim), rows, "delimiter {}", delim as char);
        }
    }
}
