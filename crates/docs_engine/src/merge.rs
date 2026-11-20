//! Mail merge, as Word's Mailings tab does it: a main document with
//! `MERGEFIELD` fields filled in once per record of a data source.
//!
//! - [`MergeData`]: the recipients (column names and rows of values; the
//!   caller reads them from a workbook or a CSV file).
//! - [`Query`]: which rows and in what order (Word keeps them as the SQL
//!   of `w:mailMerge/w:query`: `SELECT * FROM \`Sheet1$\` WHERE … ORDER BY …`).
//! - [`Document::merge_record`]: one merged copy. `MERGEFIELD` (with its
//!   `\b` / `\f` text and `\*`, `\#`, `\@` formats), `MERGEREC`,
//!   `MERGESEQ`, `IF` fields comparing merge fields, formulas and `COMPARE`
//!   fed by merge fields, `SET`, `ASK` (its `\d` default), `FILLIN` (its
//!   `\d` default) and `REF`s to their values, and `NEXT`, `NEXTIF`,
//!   `SKIPIF` become plain text, as in the document Word's Finish & Merge
//!   makes; other fields (`PAGE`, `DATE`…) stay fields. Headers, footers
//!   and text boxes are merged too. A paragraph that held only merge
//!   fields that came out empty is left out (Word's blank line
//!   suppression).
//! - [`Document::merge_into_one`]: every record in one document, each
//!   starting a new section (Word's "Edit Individual Documents").
//! - [`MailMergeLink`]: the data source link in `settings.xml` that makes
//!   a document a mail merge main document in Word (kept as read, or
//!   written when the data source is chosen in Kantor).
//! - [`Document::merge_preview`]: what the fields show for one record
//!   (Preview Results), without changing the document.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{NaiveDate, NaiveDateTime};

use crate::document::Document;
use crate::fields::{FieldSpan, Instr, format_date, spans};
use crate::model::{OBJECT, Object, Paragraph, Position, element_name};
use crate::object::FieldMark;
use crate::xml;

const OBJ: usize = OBJECT.len_utf8();

/// Word's result for a `MERGEFIELD` whose name the data source lacks.
pub const NO_MERGE_FIELD: &str = "Error! MergeField was not found in header record of data source.";

// ── data ───────────────────────────────────────────────────────────────────

/// One value of a record.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MergeValue {
    /// As the data source shows it (a workbook cell's formatted text).
    pub text: String,
    /// The number behind it, when it is one (`\#` pictures use it).
    pub number: Option<f64>,
    /// The date and time behind it, when it is one (`\@` pictures).
    pub date: Option<NaiveDateTime>,
}

impl MergeValue {
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            text: s.into(),
            ..Self::default()
        }
    }
}

/// A data source: column names (the header row) and records.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MergeData {
    pub fields: Vec<String>,
    pub rows: Vec<Vec<MergeValue>>,
}

/// Whether a `MERGEFIELD` name refers to a column: Word writes a column
/// "First Name" as `MERGEFIELD First_Name` (or quoted as is); case is
/// ignored.
pub fn same_field(code_name: &str, column: &str) -> bool {
    let norm = |s: &str| {
        s.trim()
            .chars()
            .map(|c| if c.is_whitespace() { '_' } else { c })
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    norm(code_name) == norm(column)
}

/// The name a column gets in a `MERGEFIELD` code: spaces become
/// underscores, as Word's Insert Merge Field writes them.
pub fn field_code_name(column: &str) -> String {
    column
        .trim()
        .chars()
        .map(|c| if c.is_whitespace() { '_' } else { c })
        .collect()
}

/// The code Insert Merge Field writes for a column.
pub fn merge_field_code(column: &str) -> String {
    let name = field_code_name(column);
    if name.contains('"') || name.is_empty() {
        format!("MERGEFIELD \"{}\"", name.replace('"', "\\\""))
    } else {
        format!("MERGEFIELD {name}")
    }
}

impl MergeData {
    /// The column a `MERGEFIELD` name refers to.
    pub fn column(&self, name: &str) -> Option<usize> {
        self.fields.iter().position(|f| same_field(name, f))
    }

    /// Row `row`'s value of the column `name` refers to (`None`: no such
    /// column; an empty value for a short row).
    pub fn value(&self, row: usize, name: &str) -> Option<MergeValue> {
        let c = self.column(name)?;
        Some(
            self.rows
                .get(row)
                .and_then(|r| r.get(c))
                .cloned()
                .unwrap_or_default(),
        )
    }

    /// The rows `query` keeps, in its order (indices into [`Self::rows`]).
    pub fn select(&self, query: &Query) -> Vec<usize> {
        let mut rows: Vec<usize> = (0..self.rows.len())
            .filter(|&r| query.keeps(self, r))
            .collect();
        if !query.sort.is_empty() {
            rows.sort_by(|&a, &b| {
                for (name, asc) in &query.sort {
                    let (Some(x), Some(y)) = (self.value(a, name), self.value(b, name)) else {
                        continue;
                    };
                    let o = compare_values(&x, &y);
                    if o != std::cmp::Ordering::Equal {
                        return if *asc { o } else { o.reverse() };
                    }
                }
                std::cmp::Ordering::Equal
            });
        }
        rows
    }
}

fn compare_values(a: &MergeValue, b: &MergeValue) -> std::cmp::Ordering {
    match (a.number, b.number) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => a.text.to_lowercase().cmp(&b.text.to_lowercase()),
    }
}

// ── filtering and sorting (Word's query) ─────────────────────────────────────

/// A comparison in Filter and Sort (Word's Query Options).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterOp {
    Equal,
    NotEqual,
    Less,
    Greater,
    LessEqual,
    GreaterEqual,
    IsBlank,
    IsNotBlank,
    Contains,
    NotContains,
}

impl FilterOp {
    pub const ALL: [Self; 10] = [
        Self::Equal,
        Self::NotEqual,
        Self::Less,
        Self::Greater,
        Self::LessEqual,
        Self::GreaterEqual,
        Self::IsBlank,
        Self::IsNotBlank,
        Self::Contains,
        Self::NotContains,
    ];

    /// Word's name for it in Filter Records.
    pub fn label(self) -> &'static str {
        match self {
            Self::Equal => "Equal to",
            Self::NotEqual => "Not equal to",
            Self::Less => "Less than",
            Self::Greater => "Greater than",
            Self::LessEqual => "Less than or equal",
            Self::GreaterEqual => "Greater than or equal",
            Self::IsBlank => "Is blank",
            Self::IsNotBlank => "Is not blank",
            Self::Contains => "Contains",
            Self::NotContains => "Does not contain",
        }
    }

    /// Whether it compares with a value (not the blank tests).
    pub fn takes_value(self) -> bool {
        !matches!(self, Self::IsBlank | Self::IsNotBlank)
    }

    fn sql(self) -> &'static str {
        match self {
            Self::Equal => "=",
            Self::NotEqual => "<>",
            Self::Less => "<",
            Self::Greater => ">",
            Self::LessEqual => "<=",
            Self::GreaterEqual => ">=",
            Self::IsBlank => "IS NULL",
            Self::IsNotBlank => "IS NOT NULL",
            Self::Contains => "LIKE",
            Self::NotContains => "NOT LIKE",
        }
    }
}

/// One line of Filter Records: `field op value`, joined to the line
/// before with And (`or` false) or Or.
#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    pub field: String,
    pub op: FilterOp,
    pub value: String,
    /// Joined to the previous line with Or (else And).
    pub or: bool,
}

impl Filter {
    fn test(&self, data: &MergeData, row: usize) -> bool {
        let Some(v) = data.value(row, &self.field) else {
            return false;
        };
        let text = v.text.trim();
        let want = self.value.trim();
        let ord = || match (v.number, crate::field_calc::number_in(want, '.')) {
            (Some(x), Some(y)) if !want.is_empty() => x.partial_cmp(&y),
            _ => Some(text.to_lowercase().cmp(&want.to_lowercase())),
        };
        use std::cmp::Ordering::*;
        match self.op {
            FilterOp::Equal => ord() == Some(Equal),
            FilterOp::NotEqual => ord() != Some(Equal),
            FilterOp::Less => ord() == Some(Less),
            FilterOp::Greater => ord() == Some(Greater),
            FilterOp::LessEqual => matches!(ord(), Some(Less | Equal)),
            FilterOp::GreaterEqual => matches!(ord(), Some(Greater | Equal)),
            FilterOp::IsBlank => text.is_empty(),
            FilterOp::IsNotBlank => !text.is_empty(),
            FilterOp::Contains => text.to_lowercase().contains(&want.to_lowercase()),
            FilterOp::NotContains => !text.to_lowercase().contains(&want.to_lowercase()),
        }
    }
}

/// Which records merge and in what order: Word's query (`w:query`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    /// The sheet (`Sheet1` of `` `Sheet1$` ``), or the text file's path.
    pub table: String,
    pub filters: Vec<Filter>,
    /// Sort keys: column and ascending.
    pub sort: Vec<(String, bool)>,
}

impl Query {
    /// Whether row `row` passes the filters (And binds tighter than Or,
    /// as in SQL).
    pub fn keeps(&self, data: &MergeData, row: usize) -> bool {
        if self.filters.is_empty() {
            return true;
        }
        let mut any = false;
        let mut all = true;
        for (i, f) in self.filters.iter().enumerate() {
            if i > 0 && f.or {
                any |= all;
                all = true;
            }
            all &= f.test(data, row);
        }
        any || all
    }

    /// Word's SQL for a sheet of a workbook (`sheet` set) or a text file.
    pub fn sql(&self, text_file: bool) -> String {
        let quote = |s: &str| s.replace('`', "");
        let mut q = if text_file {
            format!("SELECT * FROM {}", self.table)
        } else {
            format!("SELECT * FROM `{}$`", quote(&self.table))
        };
        if !self.filters.is_empty() {
            q.push_str(" WHERE (");
            for (i, f) in self.filters.iter().enumerate() {
                if i > 0 {
                    q.push_str(if f.or { ") OR (" } else { " AND " });
                }
                let value = f.value.replace('\'', "''");
                let value = match f.op {
                    FilterOp::Contains | FilterOp::NotContains => format!("'%{value}%'"),
                    _ => format!("'{value}'"),
                };
                if f.op.takes_value() {
                    q.push_str(&format!("(`{}` {} {value})", quote(&f.field), f.op.sql()));
                } else {
                    q.push_str(&format!("(`{}` {})", quote(&f.field), f.op.sql()));
                }
            }
            q.push(')');
        }
        if !self.sort.is_empty() {
            q.push_str(" ORDER BY ");
            let keys: Vec<String> = self
                .sort
                .iter()
                .map(|(f, asc)| format!("`{}` {}", quote(f), if *asc { "ASC" } else { "DESC" }))
                .collect();
            q.push_str(&keys.join(", "));
        }
        q.push(' ');
        q
    }

    /// Read Word's SQL (the forms [`Self::sql`] writes, which are Word's).
    pub fn parse(sql: &str) -> Self {
        let mut out = Self::default();
        let s = sql.trim();
        let upper = s.to_ascii_uppercase();
        let Some(from) = upper.find(" FROM ") else {
            return out;
        };
        let rest = &s[from + 6..];
        let rest_upper = &upper[from + 6..];
        let end = [" WHERE ", " ORDER BY "]
            .iter()
            .filter_map(|k| rest_upper.find(k))
            .min()
            .unwrap_or(rest.len());
        let table = rest[..end].trim();
        out.table = table
            .trim_matches('`')
            .trim_matches(|c| c == '[' || c == ']')
            .trim_end_matches('$')
            .to_owned();
        let tail = &rest[end..];
        let tail_upper = &rest_upper[end..];
        let order = tail_upper.find(" ORDER BY ");
        if let Some(w) = tail_upper.find(" WHERE ") {
            let stop = order.filter(|&o| o > w).unwrap_or(tail.len());
            out.filters = parse_where(&tail[w + 7..stop]);
        }
        if let Some(o) = order {
            for key in split_top(&tail[o + 10..], ',') {
                let key = key.trim();
                let (name, asc) = match key.rsplit_once(char::is_whitespace) {
                    Some((n, d)) if d.eq_ignore_ascii_case("desc") => (n, false),
                    Some((n, d)) if d.eq_ignore_ascii_case("asc") => (n, true),
                    _ => (key, true),
                };
                let name = unquote_name(name);
                if !name.is_empty() {
                    out.sort.push((name, asc));
                }
            }
        }
        out
    }
}

fn unquote_name(s: &str) -> String {
    s.trim()
        .trim_matches('`')
        .trim_matches(|c| c == '[' || c == ']')
        .to_owned()
}

/// `s` split at `sep` outside quotes and backquotes.
fn split_top(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut q, mut bq, mut start) = (false, false, 0);
    for (i, c) in s.char_indices() {
        match c {
            '\'' if !bq => q = !q,
            '`' if !q => bq = !bq,
            c if c == sep && !q && !bq => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// The conditions of a WHERE clause: `` (`A` = 'x') AND (`B` LIKE '%y%') ``,
/// groups joined by OR.
fn parse_where(s: &str) -> Vec<Filter> {
    // Tokens: backquoted names, quoted strings, words, operators.
    #[derive(Debug, PartialEq)]
    enum Tok {
        Name(String),
        Str(String),
        Word(String),
    }
    let mut toks = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() || c == '(' || c == ')' {
            i += 1;
        } else if c == '`' || c == '[' {
            let close = if c == '`' { '`' } else { ']' };
            let mut t = String::new();
            i += 1;
            while i < chars.len() && chars[i] != close {
                t.push(chars[i]);
                i += 1;
            }
            i += 1;
            toks.push(Tok::Name(t));
        } else if c == '\'' {
            let mut t = String::new();
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        t.push('\'');
                        i += 2;
                        continue;
                    }
                    break;
                }
                t.push(chars[i]);
                i += 1;
            }
            i += 1;
            toks.push(Tok::Str(t));
        } else if "<>=".contains(c) {
            let mut t = String::new();
            while i < chars.len() && "<>=".contains(chars[i]) {
                t.push(chars[i]);
                i += 1;
            }
            toks.push(Tok::Word(t));
        } else {
            let mut t = String::new();
            while i < chars.len() && !chars[i].is_whitespace() && !"()<>='`[".contains(chars[i]) {
                t.push(chars[i]);
                i += 1;
            }
            toks.push(Tok::Word(t.to_ascii_uppercase()));
        }
    }
    let mut out = Vec::new();
    let mut or = false;
    let mut k = 0;
    while k < toks.len() {
        match &toks[k] {
            Tok::Word(w) if w == "AND" => {
                or = false;
                k += 1;
            }
            Tok::Word(w) if w == "OR" => {
                or = true;
                k += 1;
            }
            Tok::Name(name) | Tok::Word(name) => {
                let name = name.clone();
                k += 1;
                let word = |k: usize| match toks.get(k) {
                    Some(Tok::Word(w)) => w.as_str(),
                    _ => "",
                };
                let (op, used) = match (word(k), word(k + 1), word(k + 2)) {
                    ("IS", "NOT", "NULL") => (FilterOp::IsNotBlank, 3),
                    ("IS", "NULL", _) => (FilterOp::IsBlank, 2),
                    ("NOT", "LIKE", _) => (FilterOp::NotContains, 2),
                    ("LIKE", _, _) => (FilterOp::Contains, 1),
                    ("=", _, _) => (FilterOp::Equal, 1),
                    ("<>", _, _) => (FilterOp::NotEqual, 1),
                    ("<", _, _) => (FilterOp::Less, 1),
                    (">", _, _) => (FilterOp::Greater, 1),
                    ("<=", _, _) => (FilterOp::LessEqual, 1),
                    (">=", _, _) => (FilterOp::GreaterEqual, 1),
                    _ => {
                        continue;
                    }
                };
                k += used;
                let mut value = String::new();
                if op.takes_value() {
                    match toks.get(k) {
                        Some(Tok::Str(v)) | Some(Tok::Word(v)) | Some(Tok::Name(v)) => {
                            value = v.clone();
                            k += 1;
                        }
                        None => {}
                    }
                    if matches!(op, FilterOp::Contains | FilterOp::NotContains) {
                        value = value.trim_matches('%').to_owned();
                    }
                }
                out.push(Filter {
                    field: name,
                    op,
                    value,
                    or: or && !out.is_empty(),
                });
                or = false;
            }
            Tok::Str(_) => k += 1,
        }
    }
    out
}

// ── the link in settings.xml ───────────────────────────────────────────────

/// Word's relationship type for a mail merge data source.
pub const REL_MAIL_MERGE_SOURCE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/mailMergeSource";
/// …and for the recipient list's ticks (`w:odso/w:recipientData`).
pub const REL_RECIPIENT_DATA: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/recipientData";

/// A main document's data source and settings (`w:mailMerge`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MailMergeLink {
    /// `w:mainDocumentType`: `formLetters`, `mailingLabels`, `envelopes`,
    /// `catalog`, `email`, `fax`.
    pub doc_type: String,
    /// `w:dataType`: `native` (workbooks through OLE DB), `textFile`…
    pub data_type: String,
    /// `w:connectString`.
    pub connect: String,
    /// `w:query`: Word's SQL ([`Query::parse`]).
    pub query: String,
    /// The data source as the relationship names it: a path relative to
    /// the document (written by Kantor) or a `file:///` URL (Word).
    pub source: Option<String>,
    /// Its relationship id as read.
    pub(crate) source_rel: Option<String>,
    /// `w:viewMergedData`: Preview Results is on.
    pub preview: bool,
    /// `w:activeRecord`: the record shown (1-based).
    pub active_record: Option<u32>,
    /// `w:odso` (Office data source object: column mapping, unticked
    /// recipients) as read, kept while the data source stays the same.
    pub(crate) odso: Option<String>,
    /// Other children as read (`w:addressFieldName`, `w:mailSubject`…),
    /// written back in place.
    pub(crate) others: Vec<String>,
}

impl MailMergeLink {
    /// A form letter merge from a sheet of a workbook, or (`sheet` None)
    /// from a CSV/text file, at `source` (a path relative to the document,
    /// `/` separated).
    pub fn new(source: &str, sheet: Option<&str>, query: &Query) -> Self {
        let mut q = query.clone();
        let (data_type, connect) = match sheet {
            Some(s) => {
                q.table = s.to_owned();
                let win = source.replace('/', "\\");
                (
                    "native".to_owned(),
                    format!(
                        "Provider=Microsoft.ACE.OLEDB.12.0;User ID=Admin;Data Source={win};Mode=Read;Extended Properties=\"HDR=YES;IMEX=1;\";"
                    ),
                )
            }
            None => {
                q.table = source.replace('/', "\\");
                ("textFile".to_owned(), String::new())
            }
        };
        Self {
            doc_type: "formLetters".into(),
            data_type,
            connect,
            query: q.sql(sheet.is_none()),
            source: Some(source.to_owned()),
            ..Self::default()
        }
    }

    /// Whether the data source is a text file (CSV) rather than a sheet.
    pub fn is_text_file(&self) -> bool {
        self.data_type == "textFile"
    }

    /// The filter, sort and sheet of the query.
    pub fn parsed_query(&self) -> Query {
        Query::parse(&self.query)
    }

    /// The same link with another query (filter and sort), keeping the
    /// rest as it was.
    pub fn with_query(&self, query: &Query) -> Self {
        let mut q = query.clone();
        q.table = self.parsed_query().table;
        Self {
            query: q.sql(self.is_text_file()),
            ..self.clone()
        }
    }

    /// The data source file: the relationship's target, a `file:` URL
    /// made a path, a relative one resolved against `doc_dir`. Word looks
    /// for a moved data source in the document's own folder: so does this
    /// when the named file doesn't exist.
    pub fn source_path(&self, doc_dir: Option<&std::path::Path>) -> Option<std::path::PathBuf> {
        let raw = self.source.as_deref()?.trim();
        let s = raw
            .strip_prefix("file:///")
            .or_else(|| raw.strip_prefix("file://"))
            .unwrap_or(raw);
        let s = percent_decode(s);
        // A Windows path keeps its drive; on other systems only its file
        // name can be found.
        let windows = s.len() > 2 && s.as_bytes()[1] == b':' || s.contains('\\');
        let unix_abs = raw.starts_with("file:///") && !windows;
        let s = if unix_abs { format!("/{s}") } else { s };
        let p = std::path::PathBuf::from(if cfg!(windows) {
            s.clone()
        } else {
            s.replace('\\', "/")
        });
        let name = p.file_name().map(std::path::PathBuf::from);
        let candidates = [
            if p.is_absolute() {
                Some(p.clone())
            } else {
                doc_dir.map(|d| d.join(&p))
            },
            doc_dir.zip(name).map(|(d, n)| d.join(n)),
        ];
        // A Windows path elsewhere: only beside the document.
        let candidates = if windows && !cfg!(windows) {
            [None, candidates[1].clone()]
        } else {
            candidates
        };
        let first = candidates.iter().flatten().next().cloned();
        candidates
            .into_iter()
            .flatten()
            .find(|c| c.exists())
            .or(first)
    }

    /// Read `w:mailMerge` from settings.xml; `rels` is settings.xml's
    /// relationships (id → target).
    pub(crate) fn read(settings: &str, rels: &HashMap<String, String>) -> Option<Self> {
        let (start, end) = element_range(settings, "mailMerge")?;
        let el = xml::parse(&settings[start..end])?;
        let val = |name: &str| {
            el.child(name)
                .and_then(|c| c.attr("val"))
                .map(str::to_owned)
                .unwrap_or_default()
        };
        let source_rel = el
            .child("dataSource")
            .and_then(|c| c.attr("id"))
            .map(str::to_owned);
        let inner = &settings[start..end];
        let odso = element_range(inner, "odso").map(|(a, b)| inner[a..b].to_owned());
        let known = [
            "mainDocumentType",
            "linkToQuery",
            "dataType",
            "connectString",
            "query",
            "dataSource",
            "viewMergedData",
            "activeRecord",
            "odso",
        ];
        let others = child_elements(inner)
            .into_iter()
            .filter(|c| !known.contains(&element_name(c)))
            .collect();
        Some(Self {
            doc_type: val("mainDocumentType"),
            data_type: val("dataType"),
            connect: val("connectString"),
            query: val("query"),
            source: source_rel
                .as_ref()
                .and_then(|id| rels.get(id))
                .map(|t| percent_decode(t)),
            source_rel,
            preview: el.child("viewMergedData").is_some_and(xml::El::on),
            active_record: el.child("activeRecord").and_then(|c| c.num("val")),
            odso,
            others,
        })
    }

    /// The `w:mailMerge` element, its data source relationship id given.
    pub(crate) fn to_xml(&self, rel_id: Option<&str>) -> String {
        let esc = xml::escape;
        let mut x = String::from("<w:mailMerge>");
        let ty = if self.doc_type.is_empty() {
            "formLetters"
        } else {
            &self.doc_type
        };
        x.push_str(&format!("<w:mainDocumentType w:val=\"{}\"/>", esc(ty)));
        if rel_id.is_some() || !self.query.is_empty() {
            x.push_str("<w:linkToQuery/>");
        }
        if !self.data_type.is_empty() {
            x.push_str(&format!("<w:dataType w:val=\"{}\"/>", esc(&self.data_type)));
        }
        if !self.connect.is_empty() || self.data_type == "native" {
            x.push_str(&format!(
                "<w:connectString w:val=\"{}\"/>",
                esc(&self.connect)
            ));
        }
        if !self.query.is_empty() {
            x.push_str(&format!("<w:query w:val=\"{}\"/>", esc(&self.query)));
        }
        if let Some(id) = rel_id {
            x.push_str(&format!("<w:dataSource r:id=\"{}\"/>", esc(id)));
        }
        // CT_MailMerge: headerSource … mailAsAttachment, then
        // viewMergedData, activeRecord, checkErrors, odso.
        let order = [
            "headerSource",
            "doNotSuppressBlankLines",
            "destination",
            "addressFieldName",
            "mailSubject",
            "mailAsAttachment",
        ];
        let mut others: Vec<&String> = self.others.iter().collect();
        others.sort_by_key(|o| {
            order
                .iter()
                .position(|n| *n == element_name(o))
                .unwrap_or(order.len())
        });
        let (before, after): (Vec<&String>, Vec<&String>) = others
            .into_iter()
            .partition(|o| order.contains(&element_name(o)));
        for o in before {
            x.push_str(o);
        }
        if self.preview {
            x.push_str("<w:viewMergedData/>");
        }
        if let Some(r) = self.active_record {
            x.push_str(&format!("<w:activeRecord w:val=\"{r}\"/>"));
        }
        for o in after {
            x.push_str(o);
        }
        if let Some(o) = &self.odso {
            x.push_str(o);
        }
        x.push_str("</w:mailMerge>");
        x
    }

    /// Whether blank lines from empty fields stay (`w:doNotSuppressBlankLines`).
    pub fn keeps_blank_lines(&self) -> bool {
        self.others.iter().any(|o| {
            element_name(o) == "doNotSuppressBlankLines" && xml::parse(o).is_some_and(|e| e.on())
        })
    }
}

/// A path as a relationship target: `%`, spaces and `#` escaped, as Word
/// writes them (`My%20Data.xlsx`).
pub(crate) fn uri_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '%' => out.push_str("%25"),
            ' ' => out.push_str("%20"),
            '#' => out.push_str("%23"),
            c => out.push(c),
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Byte range of the first `name` element (any prefix) in `src`, start tag
/// to end tag.
pub(crate) fn element_range(src: &str, name: &str) -> Option<(usize, usize)> {
    let mut from = 0;
    let start = loop {
        let i = from + src[from..].find(&format!(":{name}"))?;
        let lt = src[..i].rfind('<')?;
        let after = src.as_bytes().get(i + 1 + name.len()).copied();
        if !src[lt + 1..i].contains(|c: char| c.is_whitespace() || c == '>' || c == '/')
            && matches!(after, Some(b' ' | b'>' | b'/' | b'\t' | b'\n' | b'\r'))
        {
            break lt;
        }
        from = i + 1;
    };
    let prefix = &src[start + 1..src[start..].find(':')? + start];
    let gt = start + src[start..].find('>')?;
    if src[..gt].ends_with('/') {
        return Some((start, gt + 1));
    }
    let close = format!("</{prefix}:{name}>");
    let end = gt + src[gt..].find(&close)? + close.len();
    Some((start, end))
}

/// The child elements of the element `src` starts with, as written.
fn child_elements(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut r = quick_xml::Reader::from_str(src);
    let mut depth = 0usize;
    let mut start = 0usize;
    loop {
        let before = r.buffer_position() as usize;
        match r.read_event() {
            Ok(quick_xml::events::Event::Start(_)) => {
                if depth == 1 {
                    start = before;
                }
                depth += 1;
            }
            Ok(quick_xml::events::Event::Empty(_)) => {
                if depth == 1 {
                    out.push(src[before..r.buffer_position() as usize].to_owned());
                }
            }
            Ok(quick_xml::events::Event::End(_)) => {
                depth = depth.saturating_sub(1);
                if depth == 1 {
                    out.push(src[start..r.buffer_position() as usize].to_owned());
                }
            }
            Ok(quick_xml::events::Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

/// settings.xml with `w:mailMerge` replaced by `el` (or removed when
/// `None`), everything else byte for byte.
pub(crate) fn set_settings_mail_merge(settings: &str, el: Option<&str>) -> String {
    let mut s = settings.to_owned();
    if let Some((a, b)) = element_range(&s, "mailMerge") {
        s.replace_range(a..b, el.unwrap_or(""));
        return s;
    }
    let Some(el) = el else {
        return s;
    };
    // CT_Settings: mailMerge comes before revisionView, trackRevisions…
    const AFTER: &[&str] = &[
        "revisionView",
        "trackRevisions",
        "doNotTrackMoves",
        "doNotTrackFormatting",
        "documentProtection",
        "autoFormatOverride",
        "styleLockTheme",
        "styleLockQFSet",
        "defaultTabStop",
        "autoHyphenation",
        "consecutiveHyphenLimit",
        "hyphenationZone",
        "doNotHyphenateCaps",
        "showEnvelope",
        "summaryLength",
        "clickAndTypeStyle",
        "defaultTableStyle",
        "evenAndOddHeaders",
        "bookFoldRevPrinting",
        "bookFoldPrinting",
        "bookFoldPrintingSheets",
        "drawingGridHorizontalSpacing",
        "drawingGridVerticalSpacing",
        "displayHorizontalDrawingGridEvery",
        "displayVerticalDrawingGridEvery",
        "doNotUseMarginsForDrawingGridOrigin",
        "drawingGridHorizontalOrigin",
        "drawingGridVerticalOrigin",
        "doNotShadeFormData",
        "noPunctuationKerning",
        "characterSpacingControl",
        "printTwoOnOne",
        "strictFirstAndLastChars",
        "noLineBreaksAfter",
        "noLineBreaksBefore",
        "savePreviewPicture",
        "doNotValidateAgainstSchema",
        "saveInvalidXml",
        "ignoreMixedContent",
        "alwaysShowPlaceholderText",
        "doNotDemarcateInvalidXml",
        "saveXmlDataOnly",
        "useXSLTWhenSaving",
        "saveThroughXslt",
        "showXMLTags",
        "alwaysMergeEmptyNamespace",
        "updateFields",
        "hdrShapeDefaults",
        "footnotePr",
        "endnotePr",
        "compat",
        "docVars",
        "rsids",
        "mathPr",
        "attachedSchema",
        "themeFontLang",
        "clrSchemeMapping",
        "doNotIncludeSubdocsInStats",
        "doNotAutoCompressPictures",
        "forceUpgrade",
        "captions",
        "readModeInkLockDown",
        "smartTagType",
        "schemaLibrary",
        "shapeDefaults",
        "doNotEmbedSmartTags",
        "decimalSymbol",
        "listSeparator",
    ];
    let at = AFTER
        .iter()
        .filter_map(|n| s.find(&format!("<w:{n}")))
        .filter(|&i| {
            // Whole names only (`<w:compat>` vs `<w:compatSetting`).
            let rest = &s[i + 3..];
            AFTER.iter().any(|n| {
                rest.starts_with(n)
                    && matches!(rest.as_bytes().get(n.len()), Some(b' ' | b'>' | b'/'))
            })
        })
        .min()
        .or_else(|| s.rfind("</"));
    match at {
        Some(at) => s.insert_str(at, el),
        None => s.push_str(el),
    }
    s
}

/// settings.xml.rels with the mail merge relationships changed: `drop`
/// ids removed, `add` appended.
pub(crate) fn set_settings_rels(rels: Option<&str>, drop: &HashSet<String>, add: &str) -> String {
    let mut s = rels.map(str::to_owned).unwrap_or_else(|| {
        format!(
            "{}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"></Relationships>",
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n"
        )
    });
    for id in drop {
        let needle = format!("Id=\"{id}\"");
        while let Some(i) = s.find(&needle) {
            let Some(a) = s[..i].rfind("<Relationship") else {
                break;
            };
            let Some(e) = s[i..].find("/>") else {
                break;
            };
            s.replace_range(a..i + e + 2, "");
        }
    }
    if !add.is_empty() {
        match s.rfind("</Relationships>") {
            Some(i) => s.insert_str(i, add),
            None => s.push_str(add),
        }
    }
    s
}

// ── evaluating the fields of one record ──────────────────────────────────────

/// Fields a merge works out (and leaves as text). `ASK`, `FILLIN` and
/// `SET` are worked out once per record as Word's merge does (`ASK` and
/// `FILLIN` take their `\d` default: a merge doesn't prompt), formulas and
/// `COMPARE` when merge fields feed them.
fn merge_kind(span: &FieldSpan) -> bool {
    let k = span.kind();
    match k.as_str() {
        "MERGEFIELD" | "MERGEREC" | "MERGESEQ" | "NEXT" | "NEXTIF" | "SKIPIF" | "SET" | "ASK"
        | "FILLIN" => true,
        "IF" | "=" | "COMPARE" => {
            let code = span.shown_code().to_ascii_uppercase();
            code.contains("MERGEFIELD") || code.contains("MERGEREC") || code.contains("MERGESEQ")
        }
        _ => false,
    }
}

/// Where a merge is in the records.
#[derive(Clone, Copy, Debug)]
struct Cursor<'a> {
    data: &'a MergeData,
    records: &'a [usize],
    /// Index into `records`.
    at: usize,
    /// Merged documents so far, this one included (`MERGESEQ`).
    seq: usize,
    skipped: bool,
    decimal: char,
}

impl Cursor<'_> {
    fn row(&self) -> Option<usize> {
        self.records.get(self.at).copied()
    }

    /// A `MERGEFIELD` name's value in the current record (`None`: no such
    /// column). Past the last record values are empty.
    fn value(&self, name: &str) -> Option<MergeValue> {
        self.data.column(name)?;
        Some(
            self.row()
                .and_then(|r| self.data.value(r, name))
                .unwrap_or_default(),
        )
    }

    /// A merge field's text (`MERGEFIELD`, `MERGEREC`, `MERGESEQ`).
    fn text(&self, i: &Instr, indonesian: bool) -> Option<String> {
        match i.kind.as_str() {
            "MERGEFIELD" => {
                let name = i.args.first()?;
                Some(match self.value(name) {
                    Some(v) => merge_text(i, &v, self.decimal, indonesian),
                    None => NO_MERGE_FIELD.to_owned(),
                })
            }
            "MERGEREC" => Some(i.number(self.at as u32 + 1)),
            "MERGESEQ" => Some(i.number(self.seq as u32)),
            _ => None,
        }
    }

    /// A field's code with the merge fields nested in it filled in
    /// (quoted where they stand alone as operands).
    fn code(&self, mark: &FieldMark, indonesian: bool) -> String {
        let mut values: Vec<String> = Vec::new();
        let raw = mark
            .code_with_results(&mut |code: &str, old: &str| {
                let i = Instr::parse(code);
                match self.text(&i, indonesian) {
                    Some(v) => {
                        values.push(v);
                        format!("\u{E000}{}\u{E001}", values.len() - 1)
                    }
                    None => old.to_owned(),
                }
            })
            .unwrap_or_default();
        let mut out = String::with_capacity(raw.len());
        let mut quoted = false;
        let mut chars = raw.chars().peekable();
        let mut prev = ' ';
        while let Some(c) = chars.next() {
            match c {
                '"' if prev != '\\' => {
                    quoted = !quoted;
                    out.push(c);
                }
                '\u{E000}' => {
                    let mut n = String::new();
                    for d in chars.by_ref() {
                        if d == '\u{E001}' {
                            break;
                        }
                        n.push(d);
                    }
                    let v = n
                        .parse::<usize>()
                        .ok()
                        .and_then(|k| values.get(k))
                        .cloned()
                        .unwrap_or_default();
                    let v = v.replace('"', "\\\"");
                    if quoted {
                        out.push_str(&v);
                    } else {
                        out.push('"');
                        out.push_str(&v);
                        out.push('"');
                    }
                }
                c => out.push(c),
            }
            prev = c;
        }
        out
    }

    /// A field's code as Word's field codes view shows it (nested fields
    /// in braces) with the nested fields worked out, innermost first:
    /// merge fields, formulas and `REF`s to `SET`/`ASK` values. With
    /// `quote`, a value standing alone as an operand is quoted (`IF`,
    /// `COMPARE`, `SET`); formulas take them bare.
    fn resolve(
        &self,
        code: &str,
        quote: bool,
        sets: &HashMap<String, String>,
        env: &FormulaCx,
        indonesian: bool,
    ) -> String {
        let mut s = code.to_owned();
        // The innermost `{ … }` each time (no brace inside it).
        while let Some(open) = s[..s.find('}').unwrap_or(0)].rfind('{') {
            let close = open + s[open..].find('}').unwrap_or(0);
            let inner = s[open + 1..close].trim().to_owned();
            let i = Instr::parse(&inner);
            let v = match i.kind.as_str() {
                "MERGEFIELD" | "MERGEREC" | "MERGESEQ" => {
                    self.text(&i, indonesian).unwrap_or_default()
                }
                "=" => env.formula(&inner, &i, sets),
                "REF" => i
                    .args
                    .first()
                    .and_then(|n| sets.get(&n.to_lowercase()))
                    .map(|v| i.cased(v))
                    .unwrap_or_default(),
                _ => String::new(),
            };
            let in_quotes = s[..open]
                .char_indices()
                .filter(|&(k, c)| c == '"' && (k == 0 || !s[..k].ends_with('\\')))
                .count()
                % 2
                == 1;
            // Only the outer field's operands: inside a nested formula
            // values stay bare.
            let depth = s[..open].matches('{').count() - s[..open].matches('}').count();
            let v = if quote && !in_quotes && depth == 0 {
                format!("\"{}\"", v.replace('"', "\\\""))
            } else {
                v
            };
            s.replace_range(open..=close, &v);
        }
        s
    }

    /// `a op b` of `NEXTIF`, `SKIPIF` and `IF`.
    fn condition(&self, args: &[String]) -> Option<bool> {
        let [a, op, b, ..] = args else {
            return None;
        };
        crate::field_calc::compare(a, op, b, self.decimal)
    }
}

/// What formulas in a merged story need: its paragraphs (for table
/// cells) and the cell the field is in.
struct FormulaCx<'a> {
    paras: &'a [Paragraph],
    cell: Option<crate::model::CellRef>,
    decimal: char,
}

impl FormulaCx<'_> {
    /// `= expr \# picture`, bookmarks being `SET`/`ASK` values.
    fn formula(&self, code: &str, i: &Instr, sets: &HashMap<String, String>) -> String {
        let body = code.trim_start().strip_prefix('=').unwrap_or(code);
        let expr = body.find('\\').map_or(body, |k| &body[..k]);
        let lookup = |n: &str| sets.get(&n.to_lowercase()).cloned();
        let env = crate::field_calc::FormulaEnv {
            paras: self.paras,
            bookmark: &lookup,
            cell: self.cell,
            decimal: self.decimal,
        };
        match crate::field_calc::formula(expr, &env) {
            Ok(v) => match i.arg("#") {
                Some(pic) => crate::field_calc::number_picture(v, pic, self.decimal),
                None => crate::field_calc::plain_number(v, self.decimal),
            },
            Err(e) => e,
        }
    }
}

/// A `MERGEFIELD`'s text for a value: `\#` / `\@` picture, `\*` case,
/// `\b` before and `\f` after (only when there is text).
pub fn merge_text(i: &Instr, v: &MergeValue, decimal: char, indonesian: bool) -> String {
    let mut s = if let Some(pic) = i.arg("#") {
        match v
            .number
            .or_else(|| crate::field_calc::number_in(&v.text, decimal))
        {
            Some(n) if !v.text.trim().is_empty() => {
                crate::field_calc::number_picture(n, pic, decimal)
            }
            _ => v.text.clone(),
        }
    } else if let Some(pic) = i.arg("@") {
        match v.date.or_else(|| parse_date(&v.text, decimal == ',')) {
            Some(t) => format_date(t, pic, indonesian),
            None => v.text.clone(),
        }
    } else {
        v.text.clone()
    };
    s = i.cased(&s);
    if !s.is_empty() {
        if let Some(b) = i.arg("b") {
            s.insert_str(0, b);
        }
        if let Some(f) = i.arg("f") {
            s.push_str(f);
        }
    }
    s
}

/// A date written as text: ISO, `d/m/yyyy` (`day_first`) or `m/d/yyyy`,
/// `d-m-yyyy`, `d MMMM yyyy`, `MMMM d, yyyy`, with an optional time.
pub fn parse_date(s: &str, day_first: bool) -> Option<NaiveDateTime> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    for f in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(t) = NaiveDateTime::parse_from_str(s, f) {
            return Some(t);
        }
    }
    let slash = if day_first {
        ["%d/%m/%Y", "%m/%d/%Y"]
    } else {
        ["%m/%d/%Y", "%d/%m/%Y"]
    };
    let dates = ["%Y-%m-%d"].into_iter().chain(slash).chain([
        "%d-%m-%Y",
        "%d.%m.%Y",
        "%d %B %Y",
        "%d %b %Y",
        "%B %d, %Y",
        "%b %d, %Y",
    ]);
    for f in dates {
        if let Ok(d) = NaiveDate::parse_from_str(s, f) {
            return d.and_hms_opt(0, 0, 0);
        }
    }
    None
}

/// Whether the run at `pos` is in Indonesian (month names of `\@`).
fn indonesian_at(doc: &Document, paras: &[Paragraph], pos: Position) -> bool {
    let lang = |e: &crate::model::Extras| {
        e.iter()
            .find(|x| element_name(x) == "lang")
            .and_then(xml::parse)
            .and_then(|el| el.attr("val").map(str::to_owned))
    };
    let props = paras
        .get(pos.para)
        .map(|p| p.props_at((pos.offset + OBJ).min(p.text.len())))
        .unwrap_or_default();
    lang(&props.extras)
        .or_else(|| lang(&doc.styles.default_run.extras))
        .unwrap_or_default()
        .to_ascii_lowercase()
        .starts_with("id")
}

/// Replace a field (marks and result) with plain text, keeping the
/// result's formatting.
fn unlink(paras: &mut Vec<Paragraph>, span: &FieldSpan, text: &str) {
    let id = crate::fields::set_result(paras, span, text);
    let Some(s) = spans(paras).into_iter().find(|s| s.mark_id() == id) else {
        return;
    };
    paras[s.end.para].delete(s.end.offset..s.end.offset + OBJ);
    paras[s.begin.para].delete(s.begin.offset..s.begin.offset + OBJ);
}

/// What merging a story found.
#[derive(Default)]
struct Merged {
    /// Paragraphs with `MERGEFIELD`s: whether any of them had text.
    filled: HashMap<usize, bool>,
}

/// Merge the fields of `paras` (one story) for the cursor's record.
/// `NEXT` and friends move the cursor.
fn merge_story(doc: &Document, paras: &mut Vec<Paragraph>, cur: &mut Cursor) -> Merged {
    let mut out = Merged::default();
    // `SET` and `ASK` values by bookmark name (lower case), for `REF`.
    let mut sets: HashMap<String, String> = HashMap::new();
    let refers = |s: &FieldSpan, sets: &HashMap<String, String>| {
        let i = Instr::parse(&s.instr);
        i.kind == "REF"
            && i.args
                .first()
                .is_some_and(|n| sets.contains_key(&n.to_lowercase()))
    };
    loop {
        if cur.skipped {
            break;
        }
        let Some(span) = spans(paras)
            .into_iter()
            .find(|s| merge_kind(s) || refers(s, &sets))
        else {
            break;
        };
        let i = Instr::parse(&span.instr);
        let ind = indonesian_at(doc, paras, span.begin);
        let env = FormulaCx {
            paras,
            cell: paras.get(span.begin.para).and_then(|p| p.cell),
            decimal: cur.decimal,
        };
        let text = match i.kind.as_str() {
            "=" => {
                let code = cur.resolve(&span.shown_code(), false, &sets, &env, ind);
                env.formula(&code, &Instr::parse(&code), &sets)
            }
            "COMPARE" => {
                let code = cur.resolve(&span.shown_code(), true, &sets, &env, ind);
                let ci = Instr::parse(&code);
                match cur.condition(&ci.args) {
                    Some(r) => (r as u8).to_string(),
                    None => "!Syntax Error".into(),
                }
            }
            "SET" | "ASK" => {
                let code = cur.resolve(&span.shown_code(), true, &sets, &env, ind);
                let ci = Instr::parse(&code);
                if let Some(name) = ci.args.first() {
                    let v = if i.kind == "SET" {
                        ci.args.get(1).cloned().unwrap_or_default()
                    } else {
                        ci.arg("d").unwrap_or_default().to_owned()
                    };
                    sets.insert(name.to_lowercase(), v);
                }
                String::new()
            }
            "FILLIN" => {
                let code = cur.resolve(&span.shown_code(), true, &sets, &env, ind);
                let ci = Instr::parse(&code);
                ci.cased(ci.arg("d").unwrap_or_default())
            }
            "REF" => i
                .args
                .first()
                .and_then(|n| sets.get(&n.to_lowercase()))
                .map(|v| i.cased(v))
                .unwrap_or_default(),
            "MERGEFIELD" | "MERGEREC" | "MERGESEQ" => {
                let t = cur.text(&i, ind).unwrap_or_default();
                if i.kind == "MERGEFIELD" {
                    let e = out.filled.entry(span.begin.para).or_insert(false);
                    *e |= !t.trim().is_empty();
                }
                t
            }
            "NEXT" => {
                cur.at += 1;
                String::new()
            }
            "NEXTIF" | "SKIPIF" | "IF" => {
                let code = cur.code(&span.mark, ind);
                let ci = Instr::parse(&code);
                let hit = cur.condition(&ci.args);
                match i.kind.as_str() {
                    "NEXTIF" => {
                        if hit == Some(true) {
                            cur.at += 1;
                        }
                        String::new()
                    }
                    "SKIPIF" => {
                        if hit == Some(true) {
                            cur.skipped = true;
                        }
                        String::new()
                    }
                    _ => match hit {
                        Some(true) => ci.cased(ci.args.get(3).map_or("", String::as_str)),
                        Some(false) => ci.cased(ci.args.get(4).map_or("", String::as_str)),
                        None => "!Syntax Error".into(),
                    },
                }
            }
            _ => String::new(),
        };
        unlink(paras, &span, &text);
    }
    out
}

/// Leave out paragraphs whose merge fields all came out empty and that
/// hold nothing else (not in tables, not ending a section).
fn suppress_blank_lines(paras: &mut Vec<Paragraph>, merged: &Merged) {
    let mut drop: Vec<usize> = merged
        .filled
        .iter()
        .filter(|(_, filled)| !**filled)
        .map(|(&k, _)| k)
        .filter(|&k| {
            paras.get(k).is_some_and(|p| {
                p.cell.is_none()
                    && !p.props.extras.iter().any(|x| element_name(x) == "sectPr")
                    && p.objects().all(|(_, o)| o.is_marker())
                    && p.text.chars().all(|c| c == OBJECT || c.is_whitespace())
            })
        })
        .collect();
    drop.sort_unstable();
    for k in drop.into_iter().rev() {
        if paras.len() > 1 {
            paras.remove(k);
        }
    }
}

/// Merge the text boxes of floating objects in `paras`.
fn merge_boxes(doc: &Document, paras: &mut [Paragraph], cur: &Cursor, keep_blank: bool) {
    for p in paras.iter_mut() {
        for run in &mut p.runs {
            let Some(Object::Float(f)) = &run.object else {
                continue;
            };
            let has = |d: &Document| spans(d.paragraphs()).iter().any(merge_kind);
            let main = f.text.as_ref().is_some_and(|t| has(&t.doc));
            let group = f.boxes.iter().any(|b| has(&b.text.doc));
            if !main && !group {
                continue;
            }
            let mut nf = (**f).clone();
            let merge = |d: &Document| {
                let mut c = *cur;
                let mut ps = d.paragraphs.clone();
                let m = merge_story(doc, &mut ps, &mut c);
                if !keep_blank {
                    suppress_blank_lines(&mut ps, &m);
                }
                let mut nd = d.clone();
                nd.paragraphs = ps;
                nd
            };
            if main && let Some(t) = &nf.text {
                let mut tb = (**t).clone();
                tb.doc = merge(&t.doc);
                nf.text = Some(Arc::new(tb));
                nf.text_edited = true;
            }
            for b in &mut nf.boxes {
                if has(&b.text.doc) {
                    let mut tb = (*b.text).clone();
                    tb.doc = merge(&b.text.doc);
                    b.text = Arc::new(tb);
                    b.edited = true;
                }
            }
            run.object = Some(Object::Float(Arc::new(nf)));
        }
    }
}

/// One merged copy ([`Document::merge_record`]).
pub struct MergeOutcome {
    /// The merged document; `None` when a `SKIPIF` skipped the record.
    pub doc: Option<Document>,
    /// Where the next copy starts in the record list.
    pub next: usize,
}

impl Document {
    /// The data source link (`w:mailMerge` in settings.xml), if this is a
    /// mail merge main document.
    pub fn mail_merge(&self) -> Option<&MailMergeLink> {
        self.mail_merge.as_ref()
    }

    /// Set or remove the data source link (saved in settings.xml; not an
    /// undo step, as in Word). Choosing another data source drops what
    /// Word kept about the old one (`w:odso`).
    pub fn set_mail_merge(&mut self, link: Option<MailMergeLink>) {
        let link = link.map(|mut l| {
            if let Some(old) = &self.mail_merge
                && old.source == l.source
                && old.data_type == l.data_type
            {
                l.source_rel = l.source_rel.or_else(|| old.source_rel.clone());
                l.odso = l.odso.or_else(|| old.odso.clone());
                if l.others.is_empty() {
                    l.others = old.others.clone();
                }
            }
            l
        });
        if link != self.mail_merge {
            self.mail_merge = link;
            self.mark_modified();
        }
    }

    /// The column names the document's `MERGEFIELD`s use (body, headers,
    /// footers, text boxes), in order, once each.
    pub fn merge_field_names(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut add = |paras: &[Paragraph]| {
            for s in spans(paras) {
                let i = Instr::parse(&s.instr);
                let mut names = Vec::new();
                if i.kind == "MERGEFIELD"
                    && let Some(n) = i.args.first()
                {
                    names.push(n.clone());
                }
                // Nested in an IF's code.
                let code = s.shown_code();
                let mut rest = code.as_str();
                while let Some(k) = rest.to_ascii_uppercase().find("MERGEFIELD") {
                    let after = &rest[k + 10..];
                    let ni = Instr::parse(&format!(
                        "MERGEFIELD {}",
                        after.split('}').next().unwrap_or("")
                    ));
                    if let Some(n) = ni.args.first() {
                        names.push(n.clone());
                    }
                    rest = after;
                }
                for n in names {
                    if !out.iter().any(|o| same_field(o, &n)) {
                        out.push(n);
                    }
                }
            }
        };
        add(&self.paragraphs);
        for st in &self.stories {
            add(st.doc.paragraphs());
        }
        for p in &self.paragraphs {
            for (_, o) in p.objects() {
                if let Object::Float(f) = o {
                    if let Some(t) = &f.text {
                        add(t.doc.paragraphs());
                    }
                    for b in &f.boxes {
                        add(b.text.doc.paragraphs());
                    }
                }
            }
        }
        out
    }

    /// Whether the document has merge fields of any kind.
    pub fn has_merge_fields(&self) -> bool {
        let any = |d: &Document| spans(&d.paragraphs).iter().any(merge_kind);
        any(self)
            || self.stories.iter().any(|s| any(&s.doc))
            || !self.merge_field_names().is_empty()
    }

    /// The copy for the records from `records[at]` on (indices into
    /// `data.rows`), the `seq`th document of the merge (1-based). Fields
    /// become text; the copy has no data source link and no undo history.
    pub fn merge_record(
        &self,
        data: &MergeData,
        records: &[usize],
        at: usize,
        seq: usize,
    ) -> MergeOutcome {
        let keep_blank = self
            .mail_merge
            .as_ref()
            .is_some_and(MailMergeLink::keeps_blank_lines);
        let start = Cursor {
            data,
            records,
            at,
            seq,
            skipped: false,
            decimal: self.decimal,
        };
        let mut cur = start;
        let mut paras = self.paragraphs.clone();
        let merged = merge_story(self, &mut paras, &mut cur);
        let next = cur.at + 1;
        if cur.skipped {
            return MergeOutcome { doc: None, next };
        }
        if !keep_blank {
            suppress_blank_lines(&mut paras, &merged);
        }
        merge_boxes(self, &mut paras, &start, keep_blank);
        let mut doc = self.snapshot(paras);
        // A new, untitled document (saving it never touches the template).
        doc.set_path_none();
        doc.mail_merge = None;
        doc.field_codes = None;
        doc.merge_view = None;
        for st in &mut doc.stories {
            if st.locked.is_some() || !spans(&st.doc.paragraphs).iter().any(merge_kind) {
                continue;
            }
            let mut c = start;
            let mut ps = st.doc.paragraphs.clone();
            let m = merge_story(self, &mut ps, &mut c);
            if !keep_blank {
                suppress_blank_lines(&mut ps, &m);
            }
            merge_boxes(self, &mut ps, &start, keep_blank);
            st.doc.paragraphs = ps;
            st.modified = true;
        }
        doc.mark_modified();
        MergeOutcome {
            doc: Some(doc),
            next,
        }
    }

    /// Every merged copy of `records`, in order (skipped records left
    /// out): the first record of each and the document.
    pub fn merge_all(&self, data: &MergeData, records: &[usize]) -> Vec<(usize, Document)> {
        let mut out = Vec::new();
        let mut at = 0;
        while at < records.len() {
            let r = self.merge_record(data, records, at, out.len() + 1);
            if let Some(d) = r.doc {
                out.push((records[at], d));
            }
            at = r.next.max(at + 1);
        }
        out
    }

    /// Every record merged into one document, each copy starting a new
    /// section on a new page (Word's Finish & Merge ▸ Edit Individual
    /// Documents). `None` when no record is left.
    pub fn merge_into_one(&self, data: &MergeData, records: &[usize]) -> Option<Document> {
        let copies = self.merge_all(data, records);
        let (_, first) = copies.first()?;
        let mut out = first.snapshot(Vec::new());
        out.tables.clear();
        out.stories.clear();
        // Headers and footers with merge fields get a part per copy.
        let merged_story = |rel: &str| {
            self.stories
                .iter()
                .any(|s| s.rel_id == rel && spans(&s.doc.paragraphs).iter().any(merge_kind))
        };
        let mut next_table = 1u32;
        let last = copies.len() - 1;
        let mut body_sect = self.body_sect.clone();
        for (k, (_, copy)) in copies.iter().enumerate() {
            // Stories: the first copy's as they are; later copies get new
            // parts for those that differ per record.
            let mut renames: Vec<(String, String)> = Vec::new();
            for st in &copy.stories {
                if k == 0 {
                    out.stories.push(st.clone());
                } else if merged_story(&st.rel_id) {
                    let mut ns = out.new_story(st.kind);
                    ns.doc = st.doc.clone();
                    ns.root_tag = st.root_tag.clone();
                    ns.rels_from = Some(st.part.clone());
                    renames.push((st.rel_id.clone(), ns.rel_id.clone()));
                    out.stories.push(ns);
                }
            }
            let rename = |x: &str| {
                let mut x = x.to_owned();
                for (a, b) in &renames {
                    x = x.replace(&format!("\"{a}\""), &format!("\"{b}\""));
                }
                x
            };
            // Tables get ids of their own.
            let mut ids: HashMap<u32, u32> = HashMap::new();
            for (id, t) in &copy.tables {
                ids.insert(*id, next_table);
                out.tables.insert(next_table, t.clone());
                next_table += 1;
            }
            let mut paras = copy.paragraphs.clone();
            for p in &mut paras {
                if let Some(c) = &mut p.cell
                    && let Some(n) = ids.get(&c.table)
                {
                    c.table = *n;
                }
                let sp = p
                    .props
                    .extras
                    .iter()
                    .find(|x| element_name(x) == "sectPr")
                    .map(str::to_owned);
                if !renames.is_empty()
                    && let Some(sp) = sp
                {
                    let new = rename(&sp);
                    p.props.extras.remove("sectPr");
                    p.props.extras.push(&new);
                }
                if k > 0 {
                    // Bookmarks once only (ids and names must be unique).
                    drop_bookmarks(p);
                }
            }
            let sect = copy.body_sect.as_deref().map(rename);
            if k < last {
                // The copy ends with its last section's break, on a
                // paragraph of its own when the copy ends in a table.
                let xml = crate::page::update_sect_pr(
                    sect.as_deref().unwrap_or("<w:sectPr/>"),
                    &copy.page,
                );
                let xml = with_section_type(&xml, "nextPage");
                let end_in_table = paras.last().is_some_and(|p| p.cell.is_some());
                if end_in_table || paras.is_empty() {
                    paras.push(Paragraph::default());
                }
                let p = paras.last_mut().expect("one paragraph");
                p.props.extras.remove("sectPr");
                p.props.extras.push(&xml);
            } else {
                body_sect = sect;
            }
            out.paragraphs.extend(paras);
        }
        out.body_sect = body_sect;
        if out.paragraphs.is_empty() {
            out.paragraphs.push(Paragraph::default());
        }
        out.mark_modified();
        Some(out)
    }

    /// What each merge field shows for the record `records[at]` (Preview
    /// Results), by [`FieldSpan::mark_id`], for the body. Fields after a
    /// `NEXT` show the next record, as Word's preview does.
    pub fn merge_preview(
        &self,
        data: &MergeData,
        records: &[usize],
        at: usize,
    ) -> HashMap<usize, String> {
        let mut cur = Cursor {
            data,
            records,
            at,
            seq: at + 1,
            skipped: false,
            decimal: self.decimal,
        };
        let mut out = HashMap::new();
        for span in spans(&self.paragraphs) {
            if !merge_kind(&span) || span.depth > 0 {
                continue;
            }
            let i = Instr::parse(&span.instr);
            let ind = indonesian_at(self, &self.paragraphs, span.begin);
            let text = match i.kind.as_str() {
                "MERGEFIELD" | "MERGEREC" | "MERGESEQ" => cur.text(&i, ind).unwrap_or_default(),
                "NEXT" => {
                    cur.at += 1;
                    String::new()
                }
                "NEXTIF" | "SKIPIF" | "IF" => {
                    let code = cur.code(&span.mark, ind);
                    let ci = Instr::parse(&code);
                    let hit = cur.condition(&ci.args);
                    match i.kind.as_str() {
                        "NEXTIF" => {
                            if hit == Some(true) {
                                cur.at += 1;
                            }
                            String::new()
                        }
                        "SKIPIF" => String::new(),
                        _ => match hit {
                            Some(true) => ci.cased(ci.args.get(3).map_or("", String::as_str)),
                            Some(false) => ci.cased(ci.args.get(4).map_or("", String::as_str)),
                            None => "!Syntax Error".into(),
                        },
                    }
                }
                _ => continue,
            };
            out.insert(span.mark_id(), text);
        }
        out
    }

    /// Show merge fields with these texts (Preview Results), by
    /// [`FieldSpan::mark_id`]; `None`: their own results. A view setting:
    /// not saved, not undone.
    pub fn set_merge_view(&mut self, view: Option<HashMap<usize, String>>) {
        self.merge_view = view.map(Arc::new);
    }

    /// The texts merge fields show instead of their results.
    pub fn merge_view(&self) -> Option<&Arc<HashMap<usize, String>>> {
        self.merge_view.as_ref()
    }

    /// The merge fields in the body: their ranges (marks included), for
    /// Highlight Merge Fields.
    pub fn merge_field_ranges(&self) -> Vec<(Position, Position)> {
        spans(&self.paragraphs)
            .into_iter()
            .filter(|s| merge_kind(s) && s.depth == 0)
            .map(|s| (s.begin, Position::new(s.end.para, s.end.offset + OBJ)))
            .collect()
    }
}

/// A bookmark start or end left out of a paragraph.
fn drop_bookmarks(p: &mut Paragraph) {
    let at: Vec<usize> = p
        .objects()
        .filter(|(_, o)| {
            matches!(o, Object::Opaque(op) if matches!(element_name(&op.xml), "bookmarkStart" | "bookmarkEnd"))
        })
        .map(|(k, _)| k)
        .collect();
    for k in at.into_iter().rev() {
        let a = p.split_run_at(k);
        let b = p.split_run_at(k + OBJ);
        p.runs.drain(a..b);
        p.text.replace_range(k..k + OBJ, "");
    }
    if p.runs.is_empty() {
        p.runs.push(crate::model::Run::text(0, Default::default()));
    }
}

/// A `w:sectPr` with its `w:type` set (`nextPage`, `continuous`…).
fn with_section_type(sect: &str, ty: &str) -> String {
    let el = format!("<w:type w:val=\"{ty}\"/>");
    crate::page::set_sect_child(sect, "type", |_| true, Some(el))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> MergeData {
        let v = MergeValue::text;
        MergeData {
            fields: vec!["First Name".into(), "City".into(), "Amount".into()],
            rows: vec![
                vec![
                    v("Budi"),
                    v("Jakarta"),
                    MergeValue {
                        text: "1,500".into(),
                        number: Some(1500.0),
                        date: None,
                    },
                ],
                vec![
                    v("Ani"),
                    v("Bandung"),
                    MergeValue {
                        text: "250".into(),
                        number: Some(250.0),
                        date: None,
                    },
                ],
                vec![v("Çelik"), v("Jakarta"), v("")],
            ],
        }
    }

    #[test]
    fn names_match_like_word() {
        assert!(same_field("First_Name", "First Name"));
        assert!(same_field("first_name", "First Name"));
        assert!(!same_field("FirstName", "First Name"));
        assert_eq!(merge_field_code("First Name"), "MERGEFIELD First_Name");
    }

    #[test]
    fn switches_format_values() {
        let d = data();
        let v = d.value(0, "Amount").unwrap();
        let i = Instr::parse(r##"MERGEFIELD Amount \# "#,##0.00" \b "Rp " \f " ,-""##);
        assert_eq!(merge_text(&i, &v, '.', false), "Rp 1,500.00 ,-");
        let empty = d.value(2, "Amount").unwrap();
        assert_eq!(merge_text(&i, &empty, '.', false), "");
        let up = Instr::parse(r"MERGEFIELD City \* Upper");
        assert_eq!(
            merge_text(&up, &d.value(1, "City").unwrap(), '.', false),
            "BANDUNG"
        );
        let date = Instr::parse(r#"MERGEFIELD D \@ "d MMMM yyyy""#);
        let v = MergeValue::text("2026-11-20");
        assert_eq!(merge_text(&date, &v, '.', false), "20 November 2026");
        assert_eq!(merge_text(&date, &v, '.', true), "20 November 2026");
        let v = MergeValue::text("17/08/2026");
        assert_eq!(merge_text(&date, &v, ',', true), "17 Agustus 2026");
    }

    #[test]
    fn queries_round_trip_and_filter() {
        let q = Query {
            table: "Data".into(),
            filters: vec![
                Filter {
                    field: "City".into(),
                    op: FilterOp::Equal,
                    value: "Jakarta".into(),
                    or: false,
                },
                Filter {
                    field: "Amount".into(),
                    op: FilterOp::IsNotBlank,
                    value: String::new(),
                    or: false,
                },
                Filter {
                    field: "First Name".into(),
                    op: FilterOp::Contains,
                    value: "an".into(),
                    or: true,
                },
            ],
            sort: vec![("First Name".into(), false)],
        };
        let sql = q.sql(false);
        assert_eq!(
            sql,
            "SELECT * FROM `Data$` WHERE ((`City` = 'Jakarta') AND (`Amount` IS NOT NULL)) OR ((`First Name` LIKE '%an%')) ORDER BY `First Name` DESC "
        );
        assert_eq!(Query::parse(&sql), q);
        // Jakarta with an amount (Budi), or a name with "an" (Ani).
        assert_eq!(data().select(&q), vec![0, 1]);
        let csv = Query {
            table: "people.csv".into(),
            ..Query::default()
        };
        assert_eq!(csv.sql(true), "SELECT * FROM people.csv ");
        assert_eq!(Query::parse("SELECT * FROM `Sheet1$` ").table, "Sheet1");
    }

    #[test]
    fn settings_element_is_replaced_in_place() {
        let s = r#"<w:settings xmlns:w="w"><w:zoom w:percent="100"/><w:mailMerge><w:mainDocumentType w:val="formLetters"/><w:dataType w:val="textFile"/><w:query w:val="SELECT * FROM a.csv"/><w:dataSource r:id="rId1"/><w:addressFieldName w:val="Email"/><w:odso><w:fHdr/></w:odso></w:mailMerge><w:defaultTabStop w:val="720"/><w:compat/></w:settings>"#;
        let rels: HashMap<String, String> =
            [("rId1".to_owned(), "file:///C:\\data\\a.csv".to_owned())].into();
        let link = MailMergeLink::read(s, &rels).unwrap();
        assert_eq!(link.data_type, "textFile");
        assert_eq!(link.source.as_deref(), Some("file:///C:\\data\\a.csv"));
        assert_eq!(link.odso.as_deref(), Some("<w:odso><w:fHdr/></w:odso>"));
        let back = link.to_xml(Some("rId1"));
        assert_eq!(
            set_settings_mail_merge(s, Some(&back)),
            s.replace(
                "<w:mainDocumentType w:val=\"formLetters\"/>",
                "<w:mainDocumentType w:val=\"formLetters\"/><w:linkToQuery/>",
            )
        );
        let none = set_settings_mail_merge(s, None);
        assert!(!none.contains("mailMerge") && none.contains("<w:defaultTabStop"));
        let plain = r#"<w:settings xmlns:w="w"><w:zoom w:percent="100"/><w:trackRevisions/><w:defaultTabStop w:val="720"/><w:compat><w:compatSetting/></w:compat></w:settings>"#;
        let added = set_settings_mail_merge(plain, Some("<w:mailMerge/>"));
        assert!(
            added.contains(r#"<w:zoom w:percent="100"/><w:mailMerge/><w:trackRevisions/>"#),
            "{added}"
        );
        // A Word link on another computer: found beside the document.
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        std::fs::write(dir.join("a.csv"), "x\n").unwrap();
        assert_eq!(link.source_path(Some(dir)), Some(dir.join("a.csv")));
    }
}
