//! Mail merge without a window: the recipients read from a workbook or a
//! CSV file through Kantor's Sheets engine, and templates filled in once
//! per recipient.
//!
//! - [`Recipients::open`]: a sheet's rows under a header row (`.xlsx`,
//!   `.xlsm`, `.xls`, `.xlsb`, `.ods`, `.csv`, `.tsv`).
//! - [`docs`]: Word mail merge main documents (`MERGEFIELD`s; see
//!   [`docs_engine::merge`]) to one `.docx` / `.pdf` per record or one
//!   merged `.docx`.
//! - [`placeholders`]: `{{Field}}` in a presentation's text or a
//!   workbook's cells, one file per record.
//! - [`file_name`]: output names from a pattern (`{Name} - Offer.pdf`).
//! - [`parse_where`]: `Field=Value` filters from the command line.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use docs_engine::{Filter, FilterOp, MergeData, MergeValue, Query};

pub mod docs;
pub mod placeholders;

/// Data sources Kantor reads.
pub const DATA_EXTENSIONS: &[&str] = &[
    "xlsx", "xlsm", "xls", "xlt", "xlsb", "ods", "csv", "tsv", "txt",
];

/// A data source opened: its sheets and the rows of one of them.
#[derive(Clone, Debug, Default)]
pub struct Recipients {
    pub path: PathBuf,
    /// Sheet names (one for a CSV file).
    pub sheets: Vec<String>,
    /// The sheet read (index into `sheets`).
    pub sheet: usize,
    /// The header row (1-based).
    pub header_row: u32,
    pub data: MergeData,
}

impl Recipients {
    /// Whether the data source is a text file (one table, no sheets).
    pub fn is_text_file(&self) -> bool {
        matches!(ext(&self.path).as_str(), "csv" | "tsv" | "txt")
    }

    /// Read `path`: `sheet` by name or 1-based number (default: the
    /// first), the column names in row `header_row` (1-based, default 1),
    /// the records below it. Empty rows are left out.
    pub fn open(path: &Path, sheet: Option<&str>, header_row: Option<u32>) -> Result<Self> {
        if !path.is_file() {
            bail!("no such file: {}", path.display());
        }
        let e = ext(path);
        if !DATA_EXTENSIONS.contains(&e.as_str()) {
            bail!(
                "{} is not a data source Kantor reads (.xlsx, .xls, .xlsb, .ods, .csv, .tsv)",
                path.display()
            );
        }
        let book = sheets_engine::Workbook::open(path)
            .with_context(|| format!("can't read {}", path.display()))?;
        let sheets = book.sheet_names();
        let index = match sheet {
            None => 0,
            Some(want) => sheets
                .iter()
                .position(|n| n == want)
                .or_else(|| sheets.iter().position(|n| n.eq_ignore_ascii_case(want)))
                .or_else(|| {
                    want.parse::<usize>()
                        .ok()
                        .filter(|n| (1..=sheets.len()).contains(n))
                        .map(|n| n - 1)
                })
                .ok_or_else(|| {
                    anyhow!(
                        "no sheet {want:?} in {}; its sheets are {}",
                        path.display(),
                        sheets
                            .iter()
                            .map(|s| format!("{s:?}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?,
        };
        let header_row = header_row.unwrap_or(1).max(1);
        let data = read_sheet(&book, index as u32, header_row - 1);
        Ok(Self {
            path: path.to_path_buf(),
            sheets,
            sheet: index,
            header_row,
            data,
        })
    }

    /// The sheet's name (what Word's query names).
    pub fn sheet_name(&self) -> &str {
        self.sheets.get(self.sheet).map_or("", String::as_str)
    }
}

/// Excel's serial day 0 (1899-12-30, so 1900-03-01 is 61).
fn serial_date(n: f64) -> Option<chrono::NaiveDateTime> {
    let base = chrono::NaiveDate::from_ymd_opt(1899, 12, 30)?.and_hms_opt(0, 0, 0)?;
    let secs = (n * 86_400.0).round() as i64;
    base.checked_add_signed(chrono::Duration::seconds(secs))
}

/// The records of a sheet: names from row `header` (0-based), values from
/// the rows below.
fn read_sheet(book: &sheets_engine::Workbook, sheet: u32, header: u32) -> MergeData {
    let (rows, cols) = book.used_size(sheet);
    let mut fields: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for c in 0..cols {
        let mut name = book.display_value(sheet, header, c).trim().to_owned();
        if name.is_empty() {
            // Word names unnamed columns F1, F2… (by position).
            name = format!("F{}", c + 1);
        }
        let mut unique = name.clone();
        let mut n = 2;
        while !seen.insert(unique.to_lowercase()) {
            unique = format!("{name}{n}");
            n += 1;
        }
        fields.push(unique);
    }
    // Trailing unnamed columns with nothing in them go.
    while fields.len() > 1 {
        let c = fields.len() as u32 - 1;
        let empty_header = book.display_value(sheet, header, c).trim().is_empty();
        let empty = (header + 1..rows).all(|r| book.display_value(sheet, r, c).trim().is_empty());
        if empty_header && empty {
            fields.pop();
        } else {
            break;
        }
    }
    let mut out = Vec::new();
    for r in header + 1..rows {
        let row: Vec<MergeValue> = (0..fields.len() as u32)
            .map(|c| {
                let text = book.display_value(sheet, r, c);
                match book.number_at(sheet, r, c) {
                    Some((n, true)) => MergeValue {
                        text,
                        number: Some(n),
                        date: serial_date(n),
                    },
                    Some((n, false)) => MergeValue {
                        text,
                        number: Some(n),
                        date: None,
                    },
                    None => MergeValue::text(text),
                }
            })
            .collect();
        if row.iter().all(|v| v.text.trim().is_empty()) {
            continue;
        }
        out.push(row);
    }
    MergeData { fields, rows: out }
}

fn ext(p: &Path) -> String {
    p.extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

// ── filters from the command line ─────────────────────────────────────────

/// A `--where` condition: `Field=Value`, `Field!=Value` (or `<>`),
/// `Field<Value`, `>`, `<=`, `>=`, `Field~Value` (contains), `Field=`
/// (blank), `Field!=` (not blank). Several are joined with And.
pub fn parse_where(s: &str) -> Result<Filter> {
    const OPS: &[(&str, FilterOp)] = &[
        ("!=", FilterOp::NotEqual),
        ("<>", FilterOp::NotEqual),
        ("<=", FilterOp::LessEqual),
        (">=", FilterOp::GreaterEqual),
        ("!~", FilterOp::NotContains),
        ("=", FilterOp::Equal),
        ("<", FilterOp::Less),
        (">", FilterOp::Greater),
        ("~", FilterOp::Contains),
    ];
    let at = OPS
        .iter()
        .filter_map(|(o, op)| s.find(o).map(|i| (i, *o, *op)))
        .min_by_key(|(i, o, _)| (*i, std::cmp::Reverse(o.len())))
        .ok_or_else(|| {
            anyhow!("{s:?} is not a condition (Field=Value, Field!=Value, Field~Text…)")
        })?;
    let (i, o, mut op) = at;
    let field = s[..i].trim().to_owned();
    let value = s[i + o.len()..].trim().to_owned();
    if field.is_empty() {
        bail!("{s:?} names no field");
    }
    if value.is_empty() {
        op = match op {
            FilterOp::Equal => FilterOp::IsBlank,
            FilterOp::NotEqual => FilterOp::IsNotBlank,
            other => other,
        };
    }
    Ok(Filter {
        field,
        op,
        value,
        or: false,
    })
}

/// The records `query` keeps, after checking its fields exist.
pub fn select(data: &MergeData, query: &Query) -> Result<Vec<usize>> {
    for f in query
        .filters
        .iter()
        .map(|f| &f.field)
        .chain(query.sort.iter().map(|(f, _)| f))
    {
        if data.column(f).is_none() {
            bail!(
                "no field {f:?}; the fields are {}",
                data.fields
                    .iter()
                    .map(|s| format!("{s:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }
    Ok(data.select(query))
}

// ── output names ──────────────────────────────────────────────────────────

/// An output file name from `pattern` for record `row`: `{Field}` is the
/// field's value (`{{Field}}` works too), `{#}` the record's number in
/// the merge. Characters file systems refuse become `_`; an empty result
/// becomes `Record N`. `ext` is added when the pattern has no extension
/// of its own (or another one).
pub fn file_name(pattern: &str, data: &MergeData, row: usize, n: usize, ext: &str) -> String {
    let mut out = String::new();
    let mut rest = pattern;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let (inner, skip) = match after.strip_prefix('{') {
            Some(a) => match a.find("}}") {
                Some(j) => (&a[..j], 1 + j + 3),
                None => {
                    out.push('{');
                    rest = after;
                    continue;
                }
            },
            None => match after.find('}') {
                Some(j) => (&after[..j], j + 2),
                None => {
                    out.push('{');
                    rest = after;
                    continue;
                }
            },
        };
        if inner.trim() == "#" {
            out.push_str(&n.to_string());
        } else {
            match data.value(row, inner.trim()) {
                Some(v) => out.push_str(&v.text),
                None => {
                    out.push('{');
                    out.push_str(inner);
                    out.push('}');
                }
            }
        }
        rest = &rest[i + skip..];
    }
    out.push_str(rest);
    let mut name: String = out
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    name = name.trim().trim_end_matches('.').trim().to_owned();
    let lower = name.to_lowercase();
    let want = format!(".{}", ext.to_lowercase());
    if !ext.is_empty() && !lower.ends_with(&want) {
        // `{Name}.docx` asked for PDF: swap the extension.
        for e in [".docx", ".pdf", ".pptx", ".xlsx"] {
            if lower.ends_with(e) {
                name.truncate(name.len() - e.len());
                break;
            }
        }
        if name.trim().is_empty() {
            name = format!("Record {n}");
        }
        name.push_str(&want);
    } else if name.trim().is_empty() || name.to_lowercase() == want {
        name = format!("Record {n}{want}");
    }
    name
}

/// `dir/name`, numbered ` (2)`, ` (3)`… when `taken` (names already
/// written in this merge) has it.
pub fn unique_path(dir: &Path, name: &str, taken: &mut HashSet<String>) -> PathBuf {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_owned(), format!(".{e}")),
        None => (name.to_owned(), String::new()),
    };
    let mut n = 1;
    loop {
        let candidate = if n == 1 {
            name.to_owned()
        } else {
            format!("{stem} ({n}){ext}")
        };
        if taken.insert(candidate.to_lowercase()) {
            return dir.join(candidate);
        }
        n += 1;
    }
}

/// What a merge wrote.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub files: Vec<PathBuf>,
    /// Records merged (copies made).
    pub records: usize,
    /// Notes for the user (fields the data lacks, records skipped…).
    pub notes: Vec<String>,
}

/// Output files the caller may overwrite.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Overwrite {
    #[default]
    Refuse,
    Allow,
}

/// Fails when `path` exists and may not be replaced.
pub(crate) fn check_new(path: &Path, overwrite: Overwrite) -> Result<()> {
    if path.exists() && overwrite == Overwrite::Refuse {
        bail!("{} exists (use -f to overwrite)", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> MergeData {
        MergeData {
            fields: vec!["Name".into(), "Dept".into()],
            rows: vec![
                vec![MergeValue::text("Siti Aminah"), MergeValue::text("R&D/QA")],
                vec![MergeValue::text(""), MergeValue::text("Ops")],
            ],
        }
    }

    #[test]
    fn names_from_patterns() {
        let d = data();
        assert_eq!(
            file_name("{Name} - Offer.pdf", &d, 0, 1, "pdf"),
            "Siti Aminah - Offer.pdf"
        );
        assert_eq!(
            file_name("{{Name}} ({Dept})", &d, 0, 1, "docx"),
            "Siti Aminah (R&D_QA).docx"
        );
        assert_eq!(file_name("{Name}.docx", &d, 0, 1, "pdf"), "Siti Aminah.pdf");
        assert_eq!(file_name("{Name}", &d, 1, 2, "pdf"), "Record 2.pdf");
        assert_eq!(
            file_name("{#}-{Missing}", &d, 1, 7, "pdf"),
            "7-{Missing}.pdf"
        );
        let mut taken = HashSet::new();
        let dir = Path::new("out");
        assert_eq!(unique_path(dir, "a.pdf", &mut taken), dir.join("a.pdf"));
        assert_eq!(unique_path(dir, "A.pdf", &mut taken), dir.join("A (2).pdf"));
    }

    #[test]
    fn where_conditions() {
        let f = parse_where("Dept = Ops").unwrap();
        assert_eq!(
            (f.field.as_str(), f.op, f.value.as_str()),
            ("Dept", FilterOp::Equal, "Ops")
        );
        assert_eq!(parse_where("Name!=").unwrap().op, FilterOp::IsNotBlank);
        assert_eq!(
            parse_where("Salary>=100").unwrap().op,
            FilterOp::GreaterEqual
        );
        assert_eq!(parse_where("Name~ami").unwrap().op, FilterOp::Contains);
        assert!(parse_where("nothing").is_err());
    }

    #[test]
    fn workbook_rows_and_csv() {
        let dir = tempfile::tempdir().unwrap();
        let csv = dir.path().join("people.csv");
        std::fs::write(
            &csv,
            "Name,City,,Joined\nBudi,Jakarta,,2026-01-05\n,,,\n\"Zoë, 张\",Medan,,\n",
        )
        .unwrap();
        let r = Recipients::open(&csv, None, None).unwrap();
        assert!(r.is_text_file());
        assert_eq!(r.data.fields, vec!["Name", "City", "F3", "Joined"]);
        assert_eq!(r.data.rows.len(), 2, "the empty row goes");
        assert_eq!(r.data.rows[1][0].text, "Zoë, 张");

        let xlsx = dir.path().join("staff.xlsx");
        let mut book = sheets_engine::Workbook::new_empty().unwrap();
        book.set_input(0, 1, 0, "Title row").unwrap();
        book.set_input(0, 2, 0, "Name").unwrap();
        book.set_input(0, 2, 1, "Salary").unwrap();
        book.set_input(0, 3, 0, "Ani").unwrap();
        book.set_input(0, 3, 1, "12500000").unwrap();
        book.save_as(&xlsx).unwrap();
        let r = Recipients::open(&xlsx, Some("1"), Some(3)).unwrap();
        assert_eq!(r.data.fields, vec!["Name", "Salary"]);
        assert_eq!(r.data.rows[0][1].number, Some(12_500_000.0));
        assert!(Recipients::open(&xlsx, Some("Nope"), None).is_err());
    }
}
