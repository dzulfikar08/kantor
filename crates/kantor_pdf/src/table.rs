//! Form values for many files, from a small table: a CSV file, a JSON file
//! or the first sheet of a spreadsheet (`.xlsx`, `.xls`, `.ods`…). One row
//! per file, matched by its file name, or the same values for every file.
//!
//! The first row holds the column names: the file name column (`file`,
//! `filename`, `file name` or `name`, any case; otherwise the first
//! column) and one column per form field, named as the field.

use std::collections::BTreeMap;
use std::path::Path;

use crate::{Error, Result};

/// Field values by field name.
pub type Values = BTreeMap<String, String>;

/// Form values from a table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    /// (file name as written, values) per row.
    pub rows: Vec<(String, Values)>,
    /// Values for every file (a JSON object of plain values, or a table
    /// without a file name column).
    pub all: Option<Values>,
}

/// Column names taken as the file name column.
const FILE_COLUMNS: &[&str] = &[
    "file",
    "filename",
    "file name",
    "file_name",
    "name",
    "nama file",
];

impl Table {
    /// Read a table file; its kind comes from the extension.
    pub fn read(path: &Path) -> Result<Table> {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "csv" | "txt" | "tsv" => {
                let text = read_text(path)?;
                Table::from_csv(&text, None)
            }
            "json" => Table::from_json(&read_text(path)?),
            "xlsx" | "xlsm" | "xlsb" | "xls" | "ods" => Table::from_sheet(path, None),
            _ => Err(Error::Invalid(format!(
                "{}: use a .csv, .json or .xlsx file for the values",
                path.display()
            ))),
        }
    }

    /// Rows of cells (the first row the column names) as a table. `key`:
    /// the file name column's name; by default the first column called
    /// like one, else the first column.
    pub fn from_rows(rows: Vec<Vec<String>>, key: Option<&str>) -> Result<Table> {
        let mut it = rows.into_iter();
        let Some(header) = it.next() else {
            return Err(Error::Invalid("the table is empty".into()));
        };
        let header: Vec<String> = header.iter().map(|h| h.trim().to_owned()).collect();
        let key_col = match key {
            Some(k) => Some(
                header
                    .iter()
                    .position(|h| h.eq_ignore_ascii_case(k.trim()))
                    .ok_or_else(|| Error::Invalid(format!("the table has no column \"{k}\"")))?,
            ),
            None => header
                .iter()
                .position(|h| FILE_COLUMNS.contains(&h.to_lowercase().as_str())),
        };
        let mut table = Table::default();
        for row in it {
            if row.iter().all(|c| c.trim().is_empty()) {
                continue;
            }
            let mut values = Values::new();
            for (i, cell) in row.iter().enumerate() {
                let Some(name) = header.get(i).filter(|h| !h.is_empty()) else {
                    continue;
                };
                if Some(i) == key_col || cell.is_empty() {
                    continue;
                }
                values.insert(name.clone(), cell.clone());
            }
            match key_col {
                Some(k) => {
                    let file = row.get(k).map(|s| s.trim().to_owned()).unwrap_or_default();
                    if !file.is_empty() {
                        table.rows.push((file, values));
                    }
                }
                None => {
                    // No file name column: one row of values for every file.
                    if table.all.is_some() {
                        return Err(Error::Invalid(
                            "the table has several rows but no file name column (call it \"file\")"
                                .into(),
                        ));
                    }
                    table.all = Some(values);
                }
            }
        }
        Ok(table)
    }

    /// A CSV text (comma, semicolon or tab separated; quotes as in RFC 4180).
    pub fn from_csv(text: &str, key: Option<&str>) -> Result<Table> {
        Table::from_rows(parse_csv(text), key)
    }

    /// A JSON file: `{"file.pdf": {"Field": "value"}, …}`, `[{"file":
    /// "file.pdf", "Field": "value"}, …]`, or `{"Field": "value"}` for
    /// every file.
    pub fn from_json(text: &str) -> Result<Table> {
        let v: serde_json::Value = serde_json::from_str(text)
            .map_err(|e| Error::Invalid(format!("the values file isn't valid JSON ({e})")))?;
        let scalar = |v: &serde_json::Value| -> Option<String> {
            match v {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Number(n) => Some(n.to_string()),
                serde_json::Value::Bool(b) => Some(if *b { "Yes" } else { "Off" }.into()),
                serde_json::Value::Null => None,
                _ => None,
            }
        };
        let values = |o: &serde_json::Map<String, serde_json::Value>| -> Values {
            o.iter()
                .filter_map(|(k, v)| Some((k.clone(), scalar(v)?)))
                .collect()
        };
        match v {
            serde_json::Value::Object(o) if o.values().all(|v| v.is_object()) && !o.is_empty() => {
                Ok(Table {
                    rows: o
                        .iter()
                        .filter_map(|(k, v)| Some((k.clone(), values(v.as_object()?))))
                        .collect(),
                    all: None,
                })
            }
            serde_json::Value::Object(o) => Ok(Table {
                rows: Vec::new(),
                all: Some(values(&o)),
            }),
            serde_json::Value::Array(a) => {
                let mut rows = Vec::new();
                for item in &a {
                    let Some(o) = item.as_object() else {
                        return Err(Error::Invalid(
                            "each item of the JSON list must be an object".into(),
                        ));
                    };
                    let key = o
                        .keys()
                        .find(|k| FILE_COLUMNS.contains(&k.to_lowercase().as_str()))
                        .ok_or_else(|| {
                            Error::Invalid("each JSON item needs a \"file\" name".into())
                        })?;
                    let file = o.get(key).and_then(scalar).unwrap_or_default();
                    let mut v = values(o);
                    v.remove(key);
                    rows.push((file, v));
                }
                Ok(Table { rows, all: None })
            }
            _ => Err(Error::Invalid(
                "the JSON values must be an object or a list of objects".into(),
            )),
        }
    }

    /// The first sheet of a spreadsheet file.
    pub fn from_sheet(path: &Path, key: Option<&str>) -> Result<Table> {
        use calamine::{Data, Reader, open_workbook_auto};
        let mut book = open_workbook_auto(path)
            .map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?;
        let range = book
            .worksheet_range_at(0)
            .ok_or_else(|| Error::Invalid(format!("{}: no sheets", path.display())))?
            .map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?;
        let cell = |d: &Data| -> String {
            match d {
                Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", *f as i64),
                Data::DateTime(dt) => {
                    let (y, m, d, h, mi, s, _) = dt.to_ymd_hms_milli();
                    if (h, mi, s) == (0, 0, 0) {
                        format!("{y:04}-{m:02}-{d:02}")
                    } else {
                        format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}")
                    }
                }
                Data::Bool(b) => if *b { "Yes" } else { "Off" }.into(),
                Data::Error(_) => String::new(),
                other => other.to_string(),
            }
        };
        let rows = range.rows().map(|r| r.iter().map(cell).collect()).collect();
        Table::from_rows(rows, key)
    }

    /// The values for the file `name` (a file name, with or without its
    /// folder): its row, matched without case and with or without `.pdf`;
    /// else the values for every file.
    pub fn values_for(&self, name: &str) -> Option<&Values> {
        let base = Path::new(name)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| name.to_owned());
        let stem = |s: &str| -> String {
            let s = s.trim().to_lowercase();
            s.strip_suffix(".pdf").map(str::to_owned).unwrap_or(s)
        };
        let want = stem(&base);
        self.rows
            .iter()
            .find(|(f, _)| {
                let f = Path::new(f.trim())
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                stem(&f) == want
            })
            .map(|(_, v)| v)
            .or(self.all.as_ref())
    }
}

fn read_text(path: &Path) -> Result<String> {
    let bytes = crate::read(path)?;
    // Excel saves CSV with a byte order mark.
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

/// Split CSV text into rows of cells. The separator is the first of tab,
/// semicolon (Excel in Indonesian and European locales) or comma found
/// outside quotes on the first line.
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let first = text.lines().next().unwrap_or("");
    let mut sep = ',';
    let mut quoted = false;
    for c in first.chars() {
        match c {
            '"' => quoted = !quoted,
            '\t' | ';' | ',' if !quoted => {
                sep = c;
                break;
            }
            _ => {}
        }
    }
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    cell.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                cell.push(c);
            }
            continue;
        }
        match c {
            '"' if cell.is_empty() => quoted = true,
            c if c == sep => row.push(std::mem::take(&mut cell)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            c => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_rows_quotes_and_separators() {
        let rows =
            parse_csv("file;Name;Note\r\na.pdf;\"Budi; S.\";\"say \"\"hi\"\"\"\nb.pdf;Sari;\n");
        assert_eq!(rows[0], ["file", "Name", "Note"]);
        assert_eq!(rows[1], ["a.pdf", "Budi; S.", "say \"hi\""]);
        assert_eq!(rows[2], ["b.pdf", "Sari", ""]);
        let rows = parse_csv("a,b\n\"x\ny\",2");
        assert_eq!(rows[1], ["x\ny", "2"]);
    }

    #[test]
    fn rows_match_by_file_name() {
        let t = Table::from_csv(
            "Name,File,Agree\nBudi,Contract A.pdf,yes\nSari,contract b,\n",
            None,
        )
        .unwrap();
        assert_eq!(t.rows.len(), 2);
        let a = t.values_for("/x/contract a.PDF").unwrap();
        assert_eq!(a.get("Name").map(String::as_str), Some("Budi"));
        assert!(!a.contains_key("File"));
        // Empty cells leave the field alone.
        assert!(
            !t.values_for("contract b.pdf")
                .unwrap()
                .contains_key("Agree")
        );
        assert!(t.values_for("other.pdf").is_none());
    }

    #[test]
    fn one_row_without_file_column_is_for_every_file() {
        let t = Table::from_csv("Company,City\nPT Maju,Surabaya\n", None).unwrap();
        assert_eq!(
            t.values_for("any.pdf")
                .unwrap()
                .get("City")
                .map(String::as_str),
            Some("Surabaya")
        );
        assert!(Table::from_csv("Company\nA\nB\n", None).is_err());
        // A named key column.
        let t = Table::from_csv("Company,Doc\nA,x.pdf\n", Some("doc")).unwrap();
        assert!(t.values_for("x.pdf").is_some());
    }

    #[test]
    fn json_shapes() {
        let t = Table::from_json(r#"{"a.pdf": {"Name": "Budi", "Age": 30, "Ok": true}}"#).unwrap();
        let v = t.values_for("a.pdf").unwrap();
        assert_eq!(v.get("Age").map(String::as_str), Some("30"));
        assert_eq!(v.get("Ok").map(String::as_str), Some("Yes"));
        let t = Table::from_json(r#"[{"file": "b.pdf", "Name": "Sari"}]"#).unwrap();
        assert_eq!(
            t.values_for("b").unwrap().get("Name").map(String::as_str),
            Some("Sari")
        );
        let t = Table::from_json(r#"{"Name": "All"}"#).unwrap();
        assert!(t.values_for("z.pdf").is_some());
        assert!(Table::from_json("[1]").is_err());
    }
}
