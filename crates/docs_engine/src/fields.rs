//! Word fields: where they are, what they show, and updating them (F9).
//!
//! A field reads as its start mark ([`FieldMark::Begin`], holding the
//! instruction and the code's XML), its result as ordinary runs, and its
//! end mark. The result is what Word last showed and what Kantor lays out;
//! [`Document::update_fields`] works out new results the way Word does:
//!
//! - `PAGEREF` from the page the bookmark is laid out on (the caller lays
//!   the document out: see [`PageLookup`]), `REF` from the bookmark's text,
//!   `NOTEREF` from the note number inside it, `SEQ` by counting;
//! - `DATE` / `TIME` from the clock, `CREATEDATE` / `SAVEDATE` /
//!   `PRINTDATE` and `AUTHOR` / `TITLE` / `DOCPROPERTY`… from the file's
//!   properties, all with `\@` date pictures and `\*` formats;
//! - `TOC`: rebuilt from the headings (`\o`, `\u`, `\t`, `\c`), each entry
//!   a paragraph in the `TOC n` style with a `PAGEREF` to a `_Toc`
//!   bookmark on its heading (`\h` makes entries links, `\n` drops page
//!   numbers, `\p` sets the separator);
//! - `TOC \b` lists only the headings inside a bookmark, `TOC \f` the
//!   `TC` entries (with its identifier, `\l` levels);
//! - formulas (`= expression`), `IF`, `COMPARE` (worked out in document
//!   order, fields nested in their codes first: see
//!   [`crate::field_calc`]), `STYLEREF`, `LISTNUM`, `SET` values read by
//!   `REF`, and `INDEX` from the `XE` entries and their pages;
//! - `MERGEFIELD` shows «Name» (there is no data source);
//! - anything else (`HYPERLINK`…) keeps its result.
//!
//! Page number fields (`PAGE`, `NUMPAGES`, `SECTIONPAGES`) aren't spans:
//! they stay one object each, filled in per page (see `crate::section`).

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use chrono::{Datelike, FixedOffset, NaiveDateTime, Timelike};

use crate::document::{Document, Kind, LINE_BREAK, PAGE_BREAK};
use crate::model::{OBJECT, Object, Paragraph, Position, Run, RunProps, Selection, element_name};
use crate::object::{FIELD_END, FIELD_SEPARATE, FieldMark, Level, Opaque};
use crate::section::NumFormat;
use crate::xml;

const OBJ: usize = OBJECT.len_utf8();

/// A field in the body text: from its start mark to its end mark.
#[derive(Clone, Debug)]
pub struct FieldSpan {
    /// Where the start mark is.
    pub begin: Position,
    /// Where the end mark is.
    pub end: Position,
    pub instr: String,
    /// How many fields' results it sits in.
    pub depth: usize,
    mark: Arc<FieldMark>,
}

impl FieldSpan {
    /// The field type, upper case (`PAGEREF`, `TOC`; `=` for a formula).
    pub fn kind(&self) -> String {
        Instr::parse(&self.instr).kind
    }

    /// The code as the field codes view shows it, nested fields' codes in
    /// braces ([`FieldMark::shown_code`]).
    pub fn shown_code(&self) -> String {
        self.mark
            .shown_code()
            .unwrap_or_else(|| self.instr.trim().to_owned())
    }

    /// The result's range (between the marks).
    pub fn result(&self) -> (Position, Position) {
        (
            Position::new(self.begin.para, self.begin.offset + OBJ),
            self.end,
        )
    }

    /// Whether `pos` is inside the field (marks included).
    pub fn contains(&self, pos: Position) -> bool {
        self.begin <= pos && pos <= Position::new(self.end.para, self.end.offset + OBJ)
    }

    fn id(&self) -> usize {
        Arc::as_ptr(&self.mark) as usize
    }

    /// Tells fields apart across edits (the start mark is shared).
    pub fn mark_id(&self) -> usize {
        self.id()
    }
}

/// The fields in `paras`, ordered by where they start.
pub fn spans(paras: &[Paragraph]) -> Vec<FieldSpan> {
    let mut open: Vec<(Position, Arc<FieldMark>)> = Vec::new();
    let mut out = Vec::new();
    for (i, p) in paras.iter().enumerate() {
        for (r, run) in p.run_ranges() {
            let Some(Object::Field(f)) = &run.object else {
                continue;
            };
            // A field mark struck through (a tracked update replaced it)
            // no longer counts: the inserted one does.
            if run
                .rev
                .as_ref()
                .is_some_and(|v| v.kind == crate::RevKind::Delete)
            {
                continue;
            }
            let pos = Position::new(i, r.start);
            match &**f {
                FieldMark::Begin { .. } => open.push((pos, f.clone())),
                FieldMark::End { simple, .. } => {
                    if open.last().is_some_and(
                        |(_, b)| matches!(&**b, FieldMark::Begin { simple: s, .. } if s == simple),
                    ) && let Some((begin, mark)) = open.pop()
                    {
                        out.push(FieldSpan {
                            begin,
                            end: pos,
                            instr: mark.instr().unwrap_or_default().to_owned(),
                            depth: open.len(),
                            mark,
                        });
                    }
                }
            }
        }
    }
    out.sort_by_key(|s| s.begin);
    out
}

/// `paras` made safe to write, when they need it: field marks without a
/// partner dropped (their result stays as text), and simple fields whose
/// start and end no longer sit together in one paragraph, hyperlink and
/// tracked change written as complex fields. `None`: nothing to change.
pub(crate) fn writable(paras: &[Paragraph]) -> Option<Vec<Paragraph>> {
    let spans = spans(paras);
    let paired: HashSet<Position> = spans.iter().flat_map(|s| [s.begin, s.end]).collect();
    let mut drop: Vec<Position> = Vec::new();
    for (i, p) in paras.iter().enumerate() {
        for (r, run) in p.run_ranges() {
            let struck = run
                .rev
                .as_ref()
                .is_some_and(|v| v.kind == crate::RevKind::Delete);
            if matches!(run.object, Some(Object::Field(_)))
                && !struck
                && !paired.contains(&Position::new(i, r.start))
            {
                drop.push(Position::new(i, r.start));
            }
        }
    }
    let loose: Vec<&FieldSpan> = spans
        .iter()
        .filter(|s| {
            matches!(&*s.mark, FieldMark::Begin { simple: true, .. }) && !simple_holds(paras, s)
        })
        .collect();
    // Struck simple field marks (a tracked update replaced the field):
    // written as a complex field's, which can be inside a `w:del`.
    let struck_simple: Vec<(Position, Option<String>)> = paras
        .iter()
        .enumerate()
        .flat_map(|(i, p)| {
            p.run_ranges().filter_map(move |(r, run)| {
                let Some(Object::Field(f)) = &run.object else {
                    return None;
                };
                let struck = run
                    .rev
                    .as_ref()
                    .is_some_and(|v| v.kind == crate::RevKind::Delete);
                match &**f {
                    FieldMark::Begin {
                        simple: true,
                        instr,
                        ..
                    } if struck => Some((Position::new(i, r.start), Some(instr.clone()))),
                    FieldMark::End { simple: true, .. } if struck => {
                        Some((Position::new(i, r.start), None))
                    }
                    _ => None,
                }
            })
        })
        .collect();
    if drop.is_empty() && loose.is_empty() && struck_simple.is_empty() {
        return None;
    }
    let mut out = paras.to_vec();
    for (pos, instr) in struck_simple {
        let mark = match instr {
            Some(instr) => complex_begin(&instr, true),
            None => FieldMark::End {
                xml: FIELD_END.into(),
                simple: false,
            },
        };
        replace_mark(&mut out, pos, Object::Field(Arc::new(mark)));
    }
    for s in loose {
        let instr = s.instr.clone();
        replace_mark(
            &mut out,
            s.begin,
            Object::Field(Arc::new(complex_begin(&instr, true))),
        );
        replace_mark(
            &mut out,
            s.end,
            Object::Field(Arc::new(FieldMark::End {
                xml: FIELD_END.into(),
                simple: false,
            })),
        );
    }
    for pos in drop.into_iter().rev() {
        out[pos.para].delete(pos.offset..pos.offset + OBJ);
    }
    Some(out)
}

/// Whether a simple field can be written as `<w:fldSimple>…</w:fldSimple>`:
/// one paragraph, one hyperlink and tracked change throughout.
fn simple_holds(paras: &[Paragraph], s: &FieldSpan) -> bool {
    if s.begin.para != s.end.para {
        return false;
    }
    let p = &paras[s.begin.para];
    let mut runs = p
        .run_ranges()
        .filter(|(r, _)| r.start >= s.begin.offset && r.start <= s.end.offset);
    let Some((_, first)) = runs.next() else {
        return false;
    };
    // `w:fldSimple` can't sit inside a `w:ins` / `w:del`.
    first.rev.is_none() && runs.all(|(_, r)| r.link == first.link && r.rev == first.rev)
}

/// A complex field's start for `instr` (with a separator when it will
/// have a result).
fn complex_begin(instr: &str, result: bool) -> FieldMark {
    match FieldMark::begin(instr) {
        FieldMark::Begin {
            instr,
            code,
            simple,
            ..
        } => FieldMark::Begin {
            instr,
            code,
            sep: result.then(|| FIELD_SEPARATE.into()),
            simple,
        },
        end => end,
    }
}

/// Swap the object at `pos` for `o` (same run formatting, link, change).
fn replace_mark(paras: &mut [Paragraph], pos: Position, o: Object) {
    let p = &mut paras[pos.para];
    let i = p.split_run_at(pos.offset);
    if let Some(run) = p.runs.get_mut(i) {
        run.object = Some(o);
    }
}

// ── instructions ────────────────────────────────────────────────────────────

/// A parsed field instruction: `PAGEREF _Toc1 \h \* MERGEFORMAT`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Instr {
    /// The field type, upper case; `=` for a formula.
    pub kind: String,
    /// Arguments that aren't switches (bookmark names, quoted text).
    pub args: Vec<String>,
    /// Switches (`h`, `*`, `@`, `o`…) and their arguments.
    pub switches: Vec<(String, Option<String>)>,
}

impl Instr {
    pub fn parse(src: &str) -> Self {
        let mut tokens: Vec<(String, bool)> = Vec::new();
        let mut chars = src.trim().chars().peekable();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                chars.next();
            } else if c == '"' {
                chars.next();
                let mut t = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' if chars.peek() == Some(&'"') => {
                            t.push('"');
                            chars.next();
                        }
                        '"' => break,
                        c => t.push(c),
                    }
                }
                tokens.push((t, true));
            } else {
                let mut t = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || (c == '"' && !t.is_empty()) {
                        break;
                    }
                    t.push(c);
                    chars.next();
                }
                tokens.push((t, false));
            }
        }
        let mut out = Instr::default();
        let mut i = 0;
        if let Some((first, false)) = tokens.first() {
            if let Some(rest) = first.strip_prefix('=') {
                out.kind = "=".into();
                if !rest.is_empty() {
                    out.args.push(rest.to_owned());
                }
            } else {
                out.kind = first.to_ascii_uppercase();
            }
            i = 1;
        }
        while i < tokens.len() {
            let (t, quoted) = &tokens[i];
            i += 1;
            if !quoted && t.starts_with('\\') && t.len() > 1 {
                let name = t[1..].to_owned();
                let takes = matches!(
                    name.as_str(),
                    "*" | "@"
                        | "#"
                        | "r"
                        | "s"
                        | "d"
                        | "b"
                        | "c"
                        | "f"
                        | "l"
                        | "o"
                        | "t"
                        | "p"
                        | "n"
                );
                let arg = match tokens.get(i) {
                    Some((a, true)) => Some(a.clone()),
                    Some((a, false)) if takes && !a.starts_with('\\') && name != "n" => {
                        Some(a.clone())
                    }
                    _ => None,
                };
                if arg.is_some() {
                    i += 1;
                }
                out.switches.push((name, arg));
            } else {
                out.args.push(t.clone());
            }
        }
        out
    }

    pub fn has(&self, switch: &str) -> bool {
        self.switches.iter().any(|(s, _)| s == switch)
    }

    /// A switch's argument (the first one with an argument).
    pub fn arg(&self, switch: &str) -> Option<&str> {
        self.switches
            .iter()
            .find(|(s, a)| s == switch && a.is_some())
            .and_then(|(_, a)| a.as_deref())
    }

    /// The `\*` formats (`MERGEFORMAT`, `roman`, `Upper`…).
    fn formats(&self) -> impl Iterator<Item = &str> {
        self.switches
            .iter()
            .filter(|(s, _)| s == "*")
            .filter_map(|(_, a)| a.as_deref())
    }

    /// `n` with the `\*` number format (Arabic when none).
    pub fn number(&self, n: u32) -> String {
        for f in self.formats() {
            let s = match f {
                "roman" => NumFormat::LowerRoman.format(n),
                "ROMAN" => NumFormat::UpperRoman.format(n),
                "alphabetic" => NumFormat::LowerLetter.format(n),
                "ALPHABETIC" => NumFormat::UpperLetter.format(n),
                f if f.eq_ignore_ascii_case("arabic") => n.to_string(),
                f if f.eq_ignore_ascii_case("ordinal") => ordinal(n),
                f if f.eq_ignore_ascii_case("hex") => format!("{n:X}"),
                _ => continue,
            };
            return self.cased(&s);
        }
        self.cased(&n.to_string())
    }

    /// Whether a `\*` number format is given.
    fn has_number_format(&self) -> bool {
        self.formats().any(|f| {
            matches!(f, "roman" | "ROMAN" | "alphabetic" | "ALPHABETIC")
                || ["arabic", "ordinal", "hex"]
                    .iter()
                    .any(|x| f.eq_ignore_ascii_case(x))
        })
    }

    /// `s` with the `\*` case formats (`Upper`, `Lower`, `FirstCap`, `Caps`).
    pub fn cased(&self, s: &str) -> String {
        let mut out = s.to_owned();
        for f in self.formats() {
            if f.eq_ignore_ascii_case("upper") {
                out = out.to_uppercase();
            } else if f.eq_ignore_ascii_case("lower") {
                out = out.to_lowercase();
            } else if f.eq_ignore_ascii_case("firstcap") {
                let mut c = out.chars();
                out = match c.next() {
                    Some(f) => f.to_uppercase().chain(c).collect(),
                    None => out,
                };
            } else if f.eq_ignore_ascii_case("caps") {
                let mut prev = ' ';
                out = out
                    .chars()
                    .flat_map(|c| {
                        let up = prev.is_whitespace();
                        prev = c;
                        if up {
                            c.to_uppercase().collect::<Vec<_>>()
                        } else {
                            vec![c]
                        }
                    })
                    .collect();
            }
        }
        out
    }

    /// `\* MERGEFORMAT`: the new result keeps the old one's formatting.
    fn merge_format(&self) -> bool {
        self.formats()
            .any(|f| f.eq_ignore_ascii_case("mergeformat"))
    }
}

fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

// ── dates ──────────────────────────────────────────────────────────────────

const MONTHS_EN: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const MONTHS_ID: [&str; 12] = [
    "Januari",
    "Februari",
    "Maret",
    "April",
    "Mei",
    "Juni",
    "Juli",
    "Agustus",
    "September",
    "Oktober",
    "November",
    "Desember",
];
const DAYS_EN: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const DAYS_ID: [&str; 7] = [
    "Senin", "Selasa", "Rabu", "Kamis", "Jumat", "Sabtu", "Minggu",
];

/// `t` written with a Word date-time picture (`\@ "d MMMM yyyy"`):
/// `d dd ddd dddd`, `M MM MMM MMMM`, `yy yyyy`, `h hh` (12-hour), `H HH`,
/// `m mm`, `s ss`, `am/pm` / `AM/PM`, text in single quotes kept as is.
/// `indonesian` names months and days in Indonesian.
pub fn format_date(t: NaiveDateTime, picture: &str, indonesian: bool) -> String {
    let (months, days) = if indonesian {
        (MONTHS_ID, DAYS_ID)
    } else {
        (MONTHS_EN, DAYS_EN)
    };
    let chars: Vec<char> = picture.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let twelve = picture.to_ascii_lowercase().contains("am/pm");
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            i += 1;
            while i < chars.len() && chars[i] != '\'' {
                out.push(chars[i]);
                i += 1;
            }
            i += 1;
            continue;
        }
        let rest: String = chars[i..].iter().take(5).collect();
        if rest.eq_ignore_ascii_case("am/pm") {
            let pm = t.hour() >= 12;
            let s = if pm { "PM" } else { "AM" };
            out.push_str(&if rest.starts_with('a') {
                s.to_lowercase()
            } else {
                s.to_owned()
            });
            i += 5;
            continue;
        }
        let mut n = 1;
        while i + n < chars.len() && chars[i + n] == c {
            n += 1;
        }
        let two = |v: u32| format!("{v:02}");
        let hour12 = match t.hour() % 12 {
            0 => 12,
            h => h,
        };
        let s = match (c, n) {
            ('d', 1) => t.day().to_string(),
            ('d', 2) => two(t.day()),
            ('d', 3) => days[t.weekday().num_days_from_monday() as usize][..3].to_owned(),
            ('d', _) => days[t.weekday().num_days_from_monday() as usize].to_owned(),
            ('M', 1) => t.month().to_string(),
            ('M', 2) => two(t.month()),
            ('M', 3) => months[t.month0() as usize][..3].to_owned(),
            ('M', _) => months[t.month0() as usize].to_owned(),
            ('y' | 'Y', 1 | 2) => two((t.year().rem_euclid(100)) as u32),
            ('y' | 'Y', _) => t.year().to_string(),
            ('h', 1) if twelve => hour12.to_string(),
            ('h', _) if twelve => two(hour12),
            ('h' | 'H', 1) => t.hour().to_string(),
            ('h' | 'H', _) => two(t.hour()),
            ('m', 1) => t.minute().to_string(),
            ('m', _) => two(t.minute()),
            ('s' | 'S', 1) => t.second().to_string(),
            ('s' | 'S', _) => two(t.second()),
            _ => {
                out.push(c);
                i += 1;
                continue;
            }
        };
        out.push_str(&s);
        i += n;
    }
    out
}

/// Parse an ISO 8601 time from the document properties into local time.
fn parse_time(s: &str, offset: FixedOffset) -> Option<NaiveDateTime> {
    let s = s.trim();
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&offset).naive_local())
        .or_else(|| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok())
}

// ── context ────────────────────────────────────────────────────────────────

/// Where a laid-out document's text landed, for page numbers.
pub trait PageLookup {
    /// The page (0-based) a position is laid out on.
    fn page_of(&self, pos: Position) -> Option<usize>;
    /// Page `page`'s number in its section's numbering (restarts counted).
    fn page_number(&self, page: usize) -> u32;
    /// That number as the section writes it (`iv`, `C`…).
    fn page_label(&self, page: usize) -> String;
}

/// What updating fields needs from outside the document.
#[derive(Clone, Copy, Debug)]
pub struct FieldContext {
    /// Local time now.
    pub now: NaiveDateTime,
    /// The local time zone (document times are stored in UTC).
    pub offset: FixedOffset,
}

/// The file's document properties (`docProps/core.xml`, `app.xml`,
/// `custom.xml`).
#[derive(Clone, Debug, Default)]
pub(crate) struct Props {
    core: String,
    app: String,
    custom: Vec<(String, String)>,
    path: Option<std::path::PathBuf>,
}

impl Props {
    pub(crate) fn of(doc: &Document) -> Self {
        if let Some(p) = &doc.field_props {
            return (**p).clone();
        }
        let path = doc.path().map(|p| p.to_path_buf());
        let Some(pkg) = &doc.package else {
            return Self {
                path,
                ..Self::default()
            };
        };
        let custom = pkg
            .part_text("docProps/custom.xml")
            .map(|x| custom_props(&x))
            .unwrap_or_default();
        Self {
            core: pkg.part_text("docProps/core.xml").unwrap_or_default(),
            app: pkg.part_text("docProps/app.xml").unwrap_or_default(),
            custom,
            path,
        }
    }

    fn core(&self, el: &str) -> Option<String> {
        let v = xml::inner_text(&self.core, &[el]);
        (!v.trim().is_empty()).then(|| v.trim().to_owned())
    }

    /// A property by its `DOCPROPERTY` name.
    fn named(&self, name: &str) -> Option<String> {
        let core = match name.to_ascii_lowercase().as_str() {
            "title" => Some("title"),
            "subject" => Some("subject"),
            "author" => Some("creator"),
            "keywords" => Some("keywords"),
            "comments" => Some("description"),
            "category" => Some("category"),
            "lastsavedby" => Some("lastModifiedBy"),
            "revisionnumber" => Some("revision"),
            "contentstatus" => Some("contentStatus"),
            _ => None,
        };
        if let Some(el) = core {
            return self.core(el);
        }
        let app = ["Company", "Manager", "Template", "HyperlinkBase"]
            .into_iter()
            .find(|a| a.eq_ignore_ascii_case(name));
        if let Some(el) = app {
            let v = xml::inner_text(&self.app, &[el]);
            return (!v.trim().is_empty()).then(|| v.trim().to_owned());
        }
        self.custom
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    }
}

/// `custom.xml`'s properties: name and text value.
fn custom_props(src: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut r = quick_xml::Reader::from_str(src);
    let mut name: Option<String> = None;
    let mut depth = 0;
    loop {
        match r.read_event() {
            Ok(quick_xml::events::Event::Start(e)) => {
                if e.local_name().as_ref() == b"property" {
                    name = e
                        .attributes()
                        .flatten()
                        .find(|a| a.key.local_name().as_ref() == b"name")
                        .map(|a| String::from_utf8_lossy(&a.value).into_owned());
                    out.push((name.clone().unwrap_or_default(), String::new()));
                    depth = 0;
                } else if name.is_some() {
                    depth += 1;
                }
            }
            Ok(quick_xml::events::Event::End(e)) => {
                if e.local_name().as_ref() == b"property" {
                    name = None;
                } else {
                    depth -= 1;
                }
            }
            Ok(quick_xml::events::Event::Text(t)) if name.is_some() && depth > 0 => {
                if let (Some(last), Ok(s)) = (out.last_mut(), t.xml10_content()) {
                    last.1.push_str(&s);
                }
            }
            Ok(quick_xml::events::Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

// ── bookmarks and headings ─────────────────────────────────────────────────

/// A bookmark: where it starts and ends.
#[derive(Clone, Debug)]
struct Bookmark {
    start: Position,
    end: Option<Position>,
}

/// `(id, name)` of a bookmark start, or `(id, "")` for its end.
fn bookmark_of(o: &Object) -> Option<(bool, String, String)> {
    let Object::Opaque(op) = o else { return None };
    if op.level != Level::Marker {
        return None;
    }
    let start = match element_name(&op.xml) {
        "bookmarkStart" => true,
        "bookmarkEnd" => false,
        _ => return None,
    };
    let el = xml::parse(&op.xml)?;
    Some((
        start,
        el.attr("id").unwrap_or_default().to_owned(),
        el.attr("name").unwrap_or_default().to_owned(),
    ))
}

fn bookmarks(paras: &[Paragraph]) -> HashMap<String, Bookmark> {
    let mut by_id: HashMap<String, String> = HashMap::new();
    let mut out: HashMap<String, Bookmark> = HashMap::new();
    for (i, p) in paras.iter().enumerate() {
        for (at, o) in p.objects() {
            let Some((start, id, name)) = bookmark_of(o) else {
                continue;
            };
            let pos = Position::new(i, at);
            if start {
                by_id.insert(id, name.clone());
                out.entry(name).or_insert(Bookmark {
                    start: pos,
                    end: None,
                });
            } else if let Some(name) = by_id.get(&id)
                && let Some(b) = out.get_mut(name)
                && b.end.is_none()
            {
                b.end = Some(pos);
            }
        }
    }
    out
}

/// Visible text from `a` to `b` (objects left out, breaks and tabs as
/// spaces between paragraphs).
fn text_between(paras: &[Paragraph], a: Position, b: Position) -> String {
    let mut out = String::new();
    let last = b.para.min(paras.len().saturating_sub(1));
    for (i, p) in paras.iter().enumerate().take(last + 1).skip(a.para) {
        let from = if i == a.para { a.offset } else { 0 };
        let to = if i == b.para { b.offset } else { p.text.len() };
        if i > a.para {
            out.push(' ');
        }
        out.extend(
            p.text
                .get(from..to.max(from))
                .unwrap_or_default()
                .chars()
                .filter(|&c| c != OBJECT),
        );
    }
    out
}

impl Document {
    /// A paragraph's outline level from its style (1–9: built-in headings
    /// and styles with `w:outlineLvl`), if it has one.
    pub(crate) fn heading_level(&self, p: &Paragraph) -> Option<u8> {
        self.style_level(p)
    }

    fn style_level(&self, p: &Paragraph) -> Option<u8> {
        for st in self
            .styles
            .chain(p.props.style.as_deref())
            .into_iter()
            .rev()
        {
            if let Some(l) = outline_of(&st.para.extras) {
                return (l < 9).then_some(l + 1);
            }
            let name = st.name.to_ascii_lowercase();
            if let Some(n) = name
                .strip_prefix("heading ")
                .and_then(|n| n.parse::<u8>().ok())
            {
                return (1..=9).contains(&n).then_some(n);
            }
            if let Some(n) = st
                .id
                .strip_prefix("Heading")
                .and_then(|n| n.parse::<u8>().ok())
            {
                return (1..=9).contains(&n).then_some(n);
            }
        }
        None
    }
}

/// `w:outlineLvl` (0-based) among property extras.
fn outline_of(extras: &crate::model::Extras) -> Option<u8> {
    let x = extras.iter().find(|x| element_name(x) == "outlineLvl")?;
    xml::parse(x)?.num("val")
}

// ── results ────────────────────────────────────────────────────────────────

/// What a field shows after updating.
enum Outcome {
    Text(String),
    Keep,
}

/// Word's text when a bookmark a field refers to is missing.
pub const NO_BOOKMARK: &str = "Error! Reference source not found.";
/// Word's text for a table of contents with no headings to list.
pub const NO_TOC_ENTRIES: &str = "No table of contents entries found.";

struct Eval<'a> {
    doc: &'a Document,
    paras: &'a [Paragraph],
    cx: &'a FieldContext,
    props: Props,
    bookmarks: HashMap<String, Bookmark>,
    seq: HashMap<usize, Option<u32>>,
    listnum: HashMap<usize, String>,
    /// `SET name value`: values REF and formulas read by name (lower case).
    sets: HashMap<String, String>,
    pages: Option<&'a dyn PageLookup>,
}

impl<'a> Eval<'a> {
    fn new(
        doc: &'a Document,
        paras: &'a [Paragraph],
        cx: &'a FieldContext,
        spans: &[FieldSpan],
    ) -> Self {
        Self {
            doc,
            paras,
            cx,
            props: Props::of(doc),
            bookmarks: bookmarks(paras),
            seq: seq_values(doc, paras, spans),
            listnum: listnum_values(doc, paras, spans),
            sets: spans
                .iter()
                .map(|s| Instr::parse(&s.instr))
                .filter(|i| i.kind == "SET" && !i.args.is_empty())
                .map(|i| {
                    (
                        i.args[0].to_lowercase(),
                        i.args.get(1).cloned().unwrap_or_default(),
                    )
                })
                .collect(),
            pages: None,
        }
    }

    fn indonesian(&self, span: &FieldSpan) -> bool {
        let p = &self.paras[span.begin.para];
        let props = p.props_at(span.begin.offset + OBJ);
        let lang = |e: &crate::model::Extras| {
            e.iter()
                .find(|x| element_name(x) == "lang")
                .and_then(xml::parse)
                .and_then(|el| el.attr("val").map(str::to_owned))
        };
        let l = lang(&props.extras)
            .or_else(|| lang(&self.doc.styles.default_run.extras))
            .unwrap_or_default();
        l.to_ascii_lowercase().starts_with("id")
    }

    fn date(&self, span: &FieldSpan, i: &Instr, t: NaiveDateTime, default: &str) -> String {
        let pic = i.arg("@").unwrap_or(default);
        i.cased(&format_date(t, pic, self.indonesian(span)))
    }

    fn eval(&self, span: &FieldSpan) -> Outcome {
        let i = Instr::parse(&span.instr);
        let text = |s: String| Outcome::Text(i.cased(&s));
        match i.kind.as_str() {
            "DATE" => Outcome::Text(self.date(span, &i, self.cx.now, "M/d/yyyy")),
            "TIME" => Outcome::Text(self.date(span, &i, self.cx.now, "h:mm am/pm")),
            "CREATEDATE" | "SAVEDATE" | "PRINTDATE" => {
                let el = match i.kind.as_str() {
                    "CREATEDATE" => "created",
                    "SAVEDATE" => "modified",
                    _ => "lastPrinted",
                };
                match self
                    .props
                    .core(el)
                    .and_then(|v| parse_time(&v, self.cx.offset))
                {
                    Some(t) => Outcome::Text(self.date(span, &i, t, "M/d/yyyy h:mm:ss am/pm")),
                    None => Outcome::Keep,
                }
            }
            "AUTHOR" | "TITLE" | "SUBJECT" | "KEYWORDS" | "COMMENTS" | "LASTSAVEDBY" => {
                if !i.args.is_empty() {
                    // `AUTHOR "name"` sets the property in Word.
                    return Outcome::Keep;
                }
                let name = match i.kind.as_str() {
                    "LASTSAVEDBY" => "LastSavedBy",
                    k => k,
                };
                text(self.props.named(name).unwrap_or_default())
            }
            "DOCPROPERTY" => match i.args.first().and_then(|n| self.props.named(n)) {
                Some(v) => text(v),
                None => Outcome::Keep,
            },
            "FILENAME" => match self.props.path.as_deref() {
                Some(path) => {
                    let s = if i.has("p") {
                        path.display().to_string()
                    } else {
                        path.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    };
                    text(s)
                }
                None => Outcome::Keep,
            },
            "NUMWORDS" => Outcome::Text(i.number(word_count(self.paras) as u32)),
            "NUMCHARS" => Outcome::Text(
                i.number(
                    self.paras
                        .iter()
                        .flat_map(|p| p.text.chars())
                        .filter(|&c| c != OBJECT && !c.is_whitespace())
                        .count() as u32,
                ),
            ),
            "MERGEFIELD" => match i.args.first() {
                Some(name) => Outcome::Text(format!("«{name}»")),
                None => Outcome::Keep,
            },
            "SEQ" => match self.seq.get(&span.id()) {
                Some(Some(n)) if !i.has("h") => Outcome::Text(i.number(*n)),
                Some(_) => Outcome::Text(String::new()),
                None => Outcome::Keep,
            },
            "REF" => self.reference(&i, i.args.first().map(String::as_str)),
            "NOTEREF" => self.note_ref(&i),
            "PAGEREF" => self.page_ref(span, &i),
            "=" | "IF" | "COMPARE" => {
                let code = self.code(span);
                let i = Instr::parse(&code);
                let out = match i.kind.as_str() {
                    "=" => self.formula(span, &code, &i),
                    "IF" => self.if_field(&i),
                    _ => self.compare_field(&i),
                };
                Outcome::Text(i.cased(&out))
            }
            "STYLEREF" => self.style_ref(span, &i),
            "LISTNUM" => match self.listnum.get(&span.id()) {
                Some(n) => Outcome::Text(n.clone()),
                None => Outcome::Keep,
            },
            "INDEX" => self.index(&i),
            k if !k.is_empty()
                && i.switches.iter().all(|(s, _)| s != "@")
                && self.bookmarks.contains_key(&i.kind) =>
            {
                // `{ Bookmark }` alone is a REF.
                let name = span
                    .instr
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                self.reference(&i, Some(&name))
            }
            _ => Outcome::Keep,
        }
    }

    /// The field's instruction with the fields nested in its code worked
    /// out (bookmark references and formulas; others keep their result).
    fn code(&self, span: &FieldSpan) -> String {
        let mut nested = |code: &str, old: &str| -> String {
            let i = Instr::parse(code);
            let name = match i.kind.as_str() {
                "REF" => i.args.first().cloned(),
                "=" => {
                    return self.formula(span, code, &i);
                }
                k if !k.is_empty()
                    && self
                        .bookmark(code.split_whitespace().next().unwrap_or(""))
                        .is_some() =>
                {
                    code.split_whitespace().next().map(str::to_owned)
                }
                _ => None,
            };
            name.as_deref()
                .and_then(|n| self.bookmark_text(n))
                .unwrap_or_else(|| old.to_owned())
        };
        span.mark
            .code_with_results(&mut nested)
            .unwrap_or_else(|| span.instr.clone())
    }

    /// A bookmark's text (for formulas, `IF` and `COMPARE`).
    fn bookmark_text(&self, name: &str) -> Option<String> {
        let Some(b) = self.bookmark(name) else {
            return self.sets.get(&name.to_lowercase()).cloned();
        };
        let end = b.end.unwrap_or(b.start);
        Some(text_between(self.paras, b.start, end).trim().to_owned())
    }

    /// `= expression`, with its `\#` picture.
    fn formula(&self, span: &FieldSpan, code: &str, i: &Instr) -> String {
        let body = code.trim_start().strip_prefix('=').unwrap_or(code);
        // The expression ends where the switches start.
        let expr = match body.find('\\') {
            Some(k) => &body[..k],
            None => body,
        };
        let lookup = |n: &str| self.bookmark_text(n);
        let env = crate::field_calc::FormulaEnv {
            paras: self.paras,
            bookmark: &lookup,
            cell: self.paras.get(span.begin.para).and_then(|p| p.cell),
            decimal: self.doc.decimal,
        };
        match crate::field_calc::formula(expr, &env) {
            Ok(v) => match i.arg("#") {
                Some(pic) => crate::field_calc::number_picture(v, pic, self.doc.decimal),
                None => crate::field_calc::plain_number(v, self.doc.decimal),
            },
            Err(e) => e,
        }
    }

    /// An operand of `IF` or `COMPARE`: a bookmark's text, or as written.
    fn operand(&self, s: &str) -> String {
        self.bookmark_text(s).unwrap_or_else(|| s.to_owned())
    }

    /// `IF a op b "true" "false"`.
    fn if_field(&self, i: &Instr) -> String {
        let [a, op, b, rest @ ..] = &i.args[..] else {
            return "!Syntax Error".into();
        };
        match crate::field_calc::compare(&self.operand(a), op, &self.operand(b), self.doc.decimal) {
            Some(true) => rest.first().cloned().unwrap_or_default(),
            Some(false) => rest.get(1).cloned().unwrap_or_default(),
            None => "!Syntax Error".into(),
        }
    }

    /// `COMPARE a op b`: 1 or 0.
    fn compare_field(&self, i: &Instr) -> String {
        let [a, op, b, ..] = &i.args[..] else {
            return "!Syntax Error".into();
        };
        match crate::field_calc::compare(&self.operand(a), op, &self.operand(b), self.doc.decimal) {
            Some(r) => (r as u8).to_string(),
            None => "!Syntax Error".into(),
        }
    }

    /// `STYLEREF "Style"` (or a heading level): the nearest paragraph in
    /// that style above the field, else below it. In headers and footers
    /// it depends on the page: the saved result stays.
    fn style_ref(&self, span: &FieldSpan, i: &Instr) -> Outcome {
        if self.doc.field_props.is_some() {
            return Outcome::Keep;
        }
        let Some(want) = i.args.first() else {
            return Outcome::Keep;
        };
        let level: Option<u8> = want.trim().parse().ok();
        let matches = |p: &Paragraph| -> bool {
            if let Some(l) = level {
                return self.doc.style_level(p) == Some(l)
                    && p.props.style.as_deref().is_some_and(|s| {
                        self.doc
                            .styles
                            .get(s)
                            .is_some_and(|st| st.name.to_ascii_lowercase().starts_with("heading"))
                    });
            }
            p.props.style.as_deref().is_some_and(|id| {
                id.eq_ignore_ascii_case(want)
                    || self
                        .doc
                        .styles
                        .get(id)
                        .is_some_and(|st| st.name.eq_ignore_ascii_case(want))
            }) || (want.eq_ignore_ascii_case("normal") && p.props.style.is_none())
        };
        let here = span.begin.para.min(self.paras.len().saturating_sub(1));
        let found = (0..=here)
            .rev()
            .chain(here + 1..self.paras.len())
            .find(|&k| {
                matches(&self.paras[k]) && !crate::field_calc::para_text(&self.paras[k]).is_empty()
            });
        let Some(k) = found else {
            return Outcome::Text("Error! No text of specified style in document.".into());
        };
        if i.has("n") || i.has("r") || i.has("w") {
            let markers = self.doc.list_markers_of(self.paras);
            let m = markers.get(k).cloned().flatten().unwrap_or_default();
            return Outcome::Text(m.trim().trim_end_matches(['.', ')']).to_owned());
        }
        Outcome::Text(i.cased(&crate::field_calc::para_text(&self.paras[k])))
    }

    /// `INDEX`: the `XE` entries in alphabetical order with the pages
    /// they're on (needs the layout), one per line (`\e` sets the
    /// separator, `Main:Sub` entries go under their main entry).
    fn index(&self, i: &Instr) -> Outcome {
        let Some(pages) = self.pages else {
            return Outcome::Keep;
        };
        let sep = i.arg("e").unwrap_or(", ");
        let mut entries: std::collections::BTreeMap<(String, String), (String, String, Vec<u32>)> =
            std::collections::BTreeMap::new();
        for s in spans(self.paras) {
            let xi = Instr::parse(&s.instr);
            if xi.kind != "XE" {
                continue;
            }
            let Some(text) = xi.args.first() else {
                continue;
            };
            let (main, sub) = match text.split_once(':') {
                Some((m, s)) => (m.trim().to_owned(), s.trim().to_owned()),
                None => (text.trim().to_owned(), String::new()),
            };
            let page = pages.page_of(s.begin).map(|p| pages.page_number(p));
            let e = entries
                .entry((main.to_lowercase(), sub.to_lowercase()))
                .or_insert_with(|| (main, sub, Vec::new()));
            if let Some(p) = page
                && !e.2.contains(&p)
            {
                e.2.push(p);
            }
        }
        if entries.is_empty() {
            return Outcome::Text("No index entries found.".into());
        }
        let mut lines = Vec::new();
        let mut last_main = String::new();
        for ((main_key, _), (main, sub, pages)) in &entries {
            let mut pages = pages.clone();
            pages.sort_unstable();
            let nums = pages
                .iter()
                .map(|p| i.number(*p))
                .collect::<Vec<_>>()
                .join(", ");
            if sub.is_empty() {
                lines.push(format!("{main}{sep}{nums}"));
            } else {
                if *main_key != last_main
                    && !entries.contains_key(&(main_key.clone(), String::new()))
                {
                    lines.push(main.clone());
                }
                lines.push(format!("    {sub}{sep}{nums}"));
            }
            last_main = main_key.clone();
        }
        Outcome::Text(lines.join(&LINE_BREAK.to_string()))
    }

    fn bookmark(&self, name: &str) -> Option<&Bookmark> {
        self.bookmarks.get(name).or_else(|| {
            self.bookmarks
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v)
        })
    }

    /// A bookmark that isn't there: Word's error, except in a header,
    /// footer or note (its bookmark may be in the body: keep the result).
    fn missing(&self) -> Outcome {
        if self.doc.field_props.is_some() {
            Outcome::Keep
        } else {
            Outcome::Text(NO_BOOKMARK.into())
        }
    }

    fn reference(&self, i: &Instr, name: Option<&str>) -> Outcome {
        let Some(b) = name.and_then(|n| self.bookmark(n)) else {
            if let Some(v) = name.and_then(|n| self.sets.get(&n.to_lowercase())) {
                return Outcome::Text(i.cased(v));
            }
            return self.missing();
        };
        if i.has("n") || i.has("r") || i.has("w") {
            let markers = self.doc.list_markers_of(self.paras);
            let m = markers
                .get(b.start.para)
                .cloned()
                .flatten()
                .unwrap_or_default();
            return Outcome::Text(m.trim().trim_end_matches(['.', ')']).to_owned());
        }
        let end = b.end.unwrap_or(b.start);
        Outcome::Text(i.cased(text_between(self.paras, b.start, end).trim()))
    }

    fn note_ref(&self, i: &Instr) -> Outcome {
        let Some(b) = i.args.first().and_then(|n| self.bookmark(n)) else {
            return self.missing();
        };
        let end = b.end.unwrap_or(b.start);
        let refs = self.doc.note_refs();
        match refs.iter().find(|r| r.pos >= b.start && r.pos <= end) {
            Some(r) => Outcome::Text(r.label.clone()),
            None => Outcome::Keep,
        }
    }

    fn page_ref(&self, span: &FieldSpan, i: &Instr) -> Outcome {
        let Some(pages) = self.pages else {
            return Outcome::Keep;
        };
        let Some(b) = i.args.first().and_then(|n| self.bookmark(n)) else {
            return self.missing();
        };
        let Some(page) = pages.page_of(b.start) else {
            return Outcome::Keep;
        };
        let shown = if i.has_number_format() {
            i.number(pages.page_number(page))
        } else {
            i.cased(&pages.page_label(page))
        };
        if i.has("p") {
            let here = pages.page_of(span.begin);
            return Outcome::Text(if here == Some(page) {
                if b.start < span.begin {
                    "above"
                } else {
                    "below"
                }
                .into()
            } else {
                format!("on page {shown}")
            });
        }
        Outcome::Text(shown)
    }
}

fn word_count(paras: &[Paragraph]) -> usize {
    paras
        .iter()
        .map(|p| {
            p.text
                .split_whitespace()
                .filter(|w| w.chars().any(|c| c != OBJECT))
                .count()
        })
        .sum()
}

/// Every `SEQ` field's number (`None`: hidden by `\h` or with nothing to
/// show), counting per identifier in document order.
fn seq_values(
    doc: &Document,
    paras: &[Paragraph],
    spans: &[FieldSpan],
) -> HashMap<usize, Option<u32>> {
    let mut out = HashMap::new();
    let mut counters: HashMap<String, u32> = HashMap::new();
    let seqs: Vec<(&FieldSpan, Instr)> = spans
        .iter()
        .map(|s| (s, Instr::parse(&s.instr)))
        .filter(|(_, i)| i.kind == "SEQ")
        .collect();
    if seqs.is_empty() {
        return out;
    }
    // Heading levels, for `\s` restarts.
    let levels: Vec<Option<u8>> = paras.iter().map(|p| doc.style_level(p)).collect();
    let mut seen_heading: HashMap<String, usize> = HashMap::new();
    for (s, i) in seqs {
        let Some(id) = i.args.first().map(|a| a.to_ascii_lowercase()) else {
            out.insert(s.id(), None);
            continue;
        };
        if let Some(level) = i.arg("s").and_then(|l| l.trim().parse::<u8>().ok()) {
            // Restart after a heading of this level or above since the
            // previous number in this sequence.
            let from = seen_heading.get(&id).copied().unwrap_or(0);
            if (from..s.begin.para).any(|k| levels[k].is_some_and(|l| l <= level)) {
                counters.insert(id.clone(), 0);
            }
            seen_heading.insert(id.clone(), s.begin.para);
        }
        let c = counters.entry(id).or_insert(0);
        if let Some(r) = i.arg("r").and_then(|r| r.trim().parse::<u32>().ok()) {
            *c = r;
        } else if !i.has("c") {
            *c += 1;
        }
        out.insert(s.id(), Some(*c));
    }
    out
}

/// Every `LISTNUM` field's number: Word's built-in lists, counted per
/// list name in document order (`\l` sets the level, else one below the
/// paragraph's list level; `\s` the number; a level counted resets the
/// levels below it).
fn listnum_values(
    doc: &Document,
    paras: &[Paragraph],
    spans: &[FieldSpan],
) -> HashMap<usize, String> {
    let mut out = HashMap::new();
    let mut counters: HashMap<String, [u32; 10]> = HashMap::new();
    for s in spans {
        let i = Instr::parse(&s.instr);
        if i.kind != "LISTNUM" {
            continue;
        }
        let list = i
            .args
            .first()
            .cloned()
            .unwrap_or_else(|| "NumberDefault".into());
        let level = match i.arg("l").and_then(|l| l.trim().parse::<usize>().ok()) {
            Some(l) => l.clamp(1, 9),
            None => paras
                .get(s.begin.para)
                .and_then(|p| doc.styles.effective_num(p))
                .map_or(1, |n| (n.level as usize + 2).min(9)),
        };
        let c = counters.entry(list.to_ascii_lowercase()).or_insert([0; 10]);
        match i.arg("s").and_then(|n| n.trim().parse::<u32>().ok()) {
            Some(n) => c[level] = n,
            None => c[level] += 1,
        }
        for k in level + 1..10 {
            c[k] = 0;
        }
        out.insert(
            s.id(),
            crate::field_calc::listnum_label(&list, level, c[level]),
        );
    }
    out
}

// ── changing results ───────────────────────────────────────────────────────

/// Replace a field's result with `text`. Returns the field's start mark's
/// id afterwards (it changes when an empty simple field gets a result).
fn set_result(paras: &mut Vec<Paragraph>, span: &FieldSpan, text: &str) -> usize {
    let i = Instr::parse(&span.instr);
    let (start, end) = span.result();
    let begin_run = paras[span.begin.para]
        .run_ranges()
        .find(|(r, _)| r.start == span.begin.offset)
        .map(|(_, run)| run.clone());
    let old_first = (start.para..=end.para).find_map(|k| {
        let p = &paras[k];
        let from = if k == start.para { start.offset } else { 0 };
        let to = if k == end.para {
            end.offset
        } else {
            p.text.len()
        };
        p.run_ranges()
            .find(|(r, run)| {
                r.start >= from && r.end <= to && !r.is_empty() && run.object.is_none()
            })
            .map(|(_, run)| run.clone())
    });
    let props = match (&old_first, i.merge_format()) {
        (Some(r), true) => r.props.clone(),
        _ => begin_run
            .as_ref()
            .map(|r| r.props.clone())
            .or_else(|| old_first.as_ref().map(|r| r.props.clone()))
            .unwrap_or_default(),
    };
    let link = old_first
        .as_ref()
        .map(|r| r.link.clone())
        .or_else(|| begin_run.as_ref().map(|r| r.link.clone()))
        .flatten();

    // Cut the old result (keeping bookmarks), joining paragraphs if it
    // spanned several.
    let mut first = paras[start.para].clone();
    if start.para == end.para {
        first.delete(start.offset..end.offset);
    } else {
        let len = first.text.len();
        first.delete(start.offset..len);
        let mut markers = Vec::new();
        for p in &paras[start.para + 1..end.para] {
            markers.extend(p.markers_in(0..p.text.len()));
        }
        let at = first.text.len();
        first.insert_runs(at, markers);
        let mut last = paras[end.para].clone();
        last.delete(0..end.offset);
        first.append(last);
    }
    // Bookmarks kept from the old result sit at its start; the new text
    // goes after them.
    let mut at = start.offset;
    while first.is_marker_at(at) && !matches!(first.object_at(at), Some(Object::Field(_))) {
        at += OBJ;
    }
    if !text.is_empty() {
        let k = first.split_run_at(at);
        first.runs.insert(
            k,
            Run {
                link,
                ..Run::text(text.len(), props)
            },
        );
        first.text.insert_str(at, text);
        first.normalize();
    }
    let mut id = Arc::as_ptr(&span.mark) as usize;
    // A self-closing simple field gets its end tag; a complex field with
    // no separator gets one.
    let fix = match &*span.mark {
        FieldMark::Begin {
            instr,
            code,
            simple: true,
            ..
        } if code.trim_end().ends_with("/>") && !text.is_empty() => {
            let c = code.trim_end();
            Some(FieldMark::Begin {
                instr: instr.clone(),
                code: format!("{}>", &c[..c.len() - 2]).into(),
                sep: None,
                simple: true,
            })
        }
        FieldMark::Begin {
            instr,
            code,
            sep: None,
            simple: false,
        } if !text.is_empty() => Some(FieldMark::Begin {
            instr: instr.clone(),
            code: code.clone(),
            sep: Some(FIELD_SEPARATE.into()),
            simple: false,
        }),
        _ => None,
    };
    let simple_end = matches!(fix, Some(FieldMark::Begin { simple: true, .. }));
    if let Some(mark) = fix {
        let mark = Arc::new(mark);
        id = Arc::as_ptr(&mark) as usize;
        let k = first.split_run_at(span.begin.offset);
        first.runs[k].object = Some(Object::Field(mark));
    }
    if simple_end {
        let end_at = at + text.len();
        let k = first.split_run_at(end_at);
        if let Some(run) = first.runs.get_mut(k) {
            run.object = Some(Object::Field(Arc::new(FieldMark::End {
                xml: "</w:fldSimple>".into(),
                simple: true,
            })));
        }
    }
    paras.splice(start.para..=end.para, [first]);
    id
}

// ── tables of contents ─────────────────────────────────────────────────────

/// One table of contents line.
struct Entry {
    level: u8,
    text: String,
    /// The heading paragraph.
    para: usize,
}

impl Document {
    /// The lines a `TOC` instruction lists, from `paras` (leaving out the
    /// table itself, `skip`).
    fn toc_entries(&self, paras: &[Paragraph], i: &Instr, skip: Range<usize>) -> Vec<Entry> {
        let range = |s: &str| -> Option<(u8, u8)> {
            let (a, b) = s.split_once('-').unwrap_or((s, s));
            Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
        };
        let by_style = match i.arg("o") {
            Some(r) => range(r),
            None if !i.has("o") && !i.has("t") && !i.has("u") && !i.has("c") && !i.has("f") => {
                Some((1, 9))
            }
            None if i.has("o") => Some((1, 9)),
            None => None,
        };
        let custom: Vec<(String, u8)> = i
            .arg("t")
            .map(|t| {
                let parts: Vec<&str> = t.split([',', ';']).map(str::trim).collect();
                parts
                    .chunks(2)
                    .filter_map(|c| {
                        Some((
                            c.first()?.to_string(),
                            c.get(1).and_then(|l| l.parse().ok()).unwrap_or(1),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let caption = i.arg("c").map(|c| c.to_ascii_lowercase());
        let spans = if caption.is_some() {
            spans(paras)
        } else {
            Vec::new()
        };
        let markers = self.list_markers_of(paras);
        // `\b`: only what's inside the bookmark.
        let within = i.arg("b").map(|name| {
            let marks = bookmarks(paras);
            let b = marks.get(name).or_else(|| {
                marks
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(name))
                    .map(|(_, v)| v)
            });
            b.map_or(0..0, |b| {
                let end = b.end.unwrap_or(b.start);
                // An end before anything of its paragraph (a bookmark
                // closed between paragraphs) leaves that paragraph out.
                let before = paras
                    .get(end.para)
                    .and_then(|p| p.text.get(..end.offset))
                    .is_some_and(|t| t.chars().all(|c| c == OBJECT));
                let last = if end.para > b.start.para && before {
                    end.para - 1
                } else {
                    end.para
                };
                b.start.para..last + 1
            })
        });
        // `\f`: `TC` entries (with the same `\f` identifier; `\l` levels).
        let tc_levels = i.arg("l").and_then(range);
        let mut tcs: HashMap<usize, Vec<(String, u8)>> = HashMap::new();
        if i.has("f") {
            let want = i.arg("f").map(str::to_ascii_lowercase);
            for s in crate::fields::spans(paras) {
                let ti = Instr::parse(&s.instr);
                if ti.kind != "TC" {
                    continue;
                }
                let id = ti.arg("f").map_or("c".to_owned(), str::to_ascii_lowercase);
                if want.as_ref().is_some_and(|w| *w != id) || (want.is_none() && id != "c") {
                    continue;
                }
                let level = ti
                    .arg("l")
                    .and_then(|l| l.trim().parse::<u8>().ok())
                    .unwrap_or(1);
                if tc_levels.is_some_and(|(a, b)| !(a..=b).contains(&level)) {
                    continue;
                }
                if let Some(text) = ti.args.first() {
                    tcs.entry(s.begin.para)
                        .or_default()
                        .push((text.clone(), level));
                }
            }
        }
        let mut out = Vec::new();
        for (k, p) in paras.iter().enumerate() {
            if skip.contains(&k) || within.as_ref().is_some_and(|w| !w.contains(&k)) {
                continue;
            }
            for (text, level) in tcs.remove(&k).unwrap_or_default() {
                out.push(Entry {
                    level: level.clamp(1, 9),
                    text,
                    para: k,
                });
            }
            let level = if let Some(c) = &caption {
                spans
                    .iter()
                    .any(|s| {
                        s.begin.para == k && {
                            let si = Instr::parse(&s.instr);
                            si.kind == "SEQ"
                                && si
                                    .args
                                    .first()
                                    .is_some_and(|a| a.to_ascii_lowercase() == *c)
                        }
                    })
                    .then_some(1)
            } else {
                let style = p.props.style.as_deref().map(|id| {
                    (
                        id.to_owned(),
                        self.styles
                            .get(id)
                            .map(|s| s.name.clone())
                            .unwrap_or_default(),
                    )
                });
                let from_custom = style.as_ref().and_then(|(id, name)| {
                    custom
                        .iter()
                        .find(|(n, _)| n.eq_ignore_ascii_case(id) || n.eq_ignore_ascii_case(name))
                        .map(|(_, l)| *l)
                });
                let from_outline = i
                    .has("u")
                    .then(|| outline_of(&p.props.extras))
                    .flatten()
                    .filter(|&l| l < 9)
                    .map(|l| l + 1);
                let from_style =
                    by_style.and_then(|(a, b)| self.style_level(p).filter(|l| (a..=b).contains(l)));
                let from_outline = from_outline.filter(|l| match by_style {
                    Some((a, b)) => (a..=b).contains(l),
                    None => true,
                });
                from_custom.or(from_outline).or(from_style)
            };
            let Some(level) = level else { continue };
            let text: String = p
                .text
                .chars()
                .filter(|&c| c != OBJECT)
                .map(|c| {
                    if c == '\t' || c == LINE_BREAK || c == PAGE_BREAK {
                        ' '
                    } else {
                        c
                    }
                })
                .collect();
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if text.is_empty() {
                continue;
            }
            let text = match markers.get(k).cloned().flatten() {
                Some(m) if caption.is_none() && !m.trim().is_empty() => {
                    format!("{}\t{text}", m.trim())
                }
                _ => text,
            };
            out.push(Entry {
                level: level.clamp(1, 9),
                text,
                para: k,
            });
        }
        out
    }

    /// The style id of `TOC n` (or Table of Figures), if the document has it.
    fn toc_style(&self, level: u8, figures: bool) -> Option<String> {
        let (id, name) = if figures {
            ("TableofFigures".to_owned(), "table of figures".to_owned())
        } else {
            (format!("TOC{level}"), format!("toc {level}"))
        };
        self.styles
            .styles
            .iter()
            .find(|s| s.id == id || s.name.eq_ignore_ascii_case(&name))
            .map(|s| s.id.clone())
    }
}

/// Rebuild the table of contents `span` in `paras`. Returns the ids of
/// the page reference fields it made (for their numbers).
fn rebuild_toc(doc: &Document, paras: &mut Vec<Paragraph>, span: &FieldSpan) -> Vec<usize> {
    let i = Instr::parse(&span.instr);
    let (pa, pb) = (span.begin.para, span.end.para);
    let entries = doc.toc_entries(paras, &i, pa..pb + 1);
    let figures = i.has("c");

    // Paragraph formatting per level: from the old lines, else the style,
    // else Word's own (indent, and a right tab with dots at the margin).
    let mut old_props: HashMap<u8, crate::model::ParaProps> = HashMap::new();
    for p in &paras[pa..=pb] {
        if let Some(st) = p.props.style.as_deref() {
            let lvl = (1..=9u8).find(|&l| doc.toc_style(l, figures).as_deref() == Some(st));
            if let Some(l) = lvl {
                let mut props = p.props.clone();
                props.extras.remove("sectPr");
                old_props.entry(l).or_insert(props);
            }
        }
    }
    let right = doc.content_width() as i32;
    let para_props = |level: u8| -> crate::model::ParaProps {
        if let Some(p) = old_props.get(&level) {
            return p.clone();
        }
        let mut props = crate::model::ParaProps::default();
        match doc.toc_style(level, figures) {
            Some(id) => props.style = Some(id),
            None => {
                props.after = Some(100);
                if level > 1 {
                    props.indent.left = Some(220 * (level as i32 - 1));
                }
            }
        }
        let probe = Paragraph {
            props: props.clone(),
            ..Paragraph::default()
        };
        let has_right = doc
            .tab_stops(&probe)
            .iter()
            .any(|t| t.kind == crate::tabs::TabKind::Right);
        if !has_right {
            props.extras.push(&format!(
                "<w:tabs><w:tab w:val=\"right\" w:leader=\"dot\" w:pos=\"{right}\"/></w:tabs>"
            ));
        }
        props
    };

    // `_Toc` bookmarks on the headings (an existing one is reused).
    let mut next_id = paras
        .iter()
        .flat_map(|p| p.objects())
        .filter_map(|(_, o)| bookmark_of(o))
        .filter_map(|(_, id, _)| id.parse::<u64>().ok())
        .max()
        .map_or(0, |m| m + 1);
    let names: HashSet<String> = bookmarks(paras).into_keys().collect();
    let mut next_name = 1u64;
    let mut targets: Vec<String> = Vec::with_capacity(entries.len());
    for e in &entries {
        let p = &mut paras[e.para];
        let existing = p.objects().find_map(|(_, o)| {
            bookmark_of(o)
                .filter(|(s, _, n)| *s && n.starts_with("_Toc"))
                .map(|(_, _, n)| n)
        });
        let name = match existing {
            Some(n) => n,
            None => {
                let name = loop {
                    let n = format!("_Toc{:09}", 100_000_000 + next_name);
                    next_name += 1;
                    if !names.contains(&n) {
                        break n;
                    }
                };
                let marker = |xml: String| {
                    Object::Opaque(Arc::new(Opaque {
                        level: Level::Marker,
                        xml,
                        label: String::new(),
                    }))
                };
                let mut start = Run::text(OBJ, RunProps::default());
                start.object = Some(marker(format!(
                    "<w:bookmarkStart w:id=\"{next_id}\" w:name=\"{name}\"/>"
                )));
                let mut end = Run::text(OBJ, RunProps::default());
                end.object = Some(marker(format!("<w:bookmarkEnd w:id=\"{next_id}\"/>")));
                next_id += 1;
                let len = p.text.len();
                p.insert_runs(len, vec![end]);
                p.insert_runs(0, vec![start]);
                name
            }
        };
        targets.push(name);
    }

    // The new lines.
    let no_pages = |level: u8| match i.switches.iter().find(|(s, _)| s == "n") {
        Some((_, None)) => true,
        Some((_, Some(r))) => {
            let (a, b) = r.split_once('-').unwrap_or((r, r));
            let (a, b) = (a.trim().parse().unwrap_or(1), b.trim().parse().unwrap_or(9));
            (a..=b).contains(&level)
        }
        None => false,
    };
    let sep = i.arg("p").map(str::to_owned).unwrap_or_else(|| "\t".into());
    let mut made = Vec::new();
    let text_props = RunProps {
        extras: {
            let mut e = crate::model::Extras::default();
            e.push("<w:noProof/>");
            e
        },
        ..RunProps::default()
    };
    let mut lines: Vec<Paragraph> = entries
        .iter()
        .zip(&targets)
        .map(|(e, name)| {
            let mut p = Paragraph {
                props: para_props(e.level),
                ..Paragraph::default()
            };
            let link: Option<Arc<str>> = i.has("h").then(|| {
                format!(
                    "<w:hyperlink w:anchor=\"{}\" w:history=\"1\">",
                    xml::escape(name)
                )
                .into()
            });
            p.push_tracked(&e.text, text_props.clone(), link.clone(), None);
            if !no_pages(e.level) {
                p.push_tracked(&sep, text_props.clone(), link.clone(), None);
                let begin = Arc::new(complex_begin(&format!(" PAGEREF {name} \\h "), true));
                made.push(Arc::as_ptr(&begin) as usize);
                // The marks' XML has no run properties.
                p.push_object(Object::Field(begin), RunProps::default(), link.clone());
                p.push_tracked("1", text_props.clone(), link.clone(), None);
                p.push_object(
                    Object::Field(Arc::new(FieldMark::end())),
                    RunProps::default(),
                    link,
                );
            }
            p
        })
        .collect();
    if lines.is_empty() {
        let mut p = Paragraph {
            props: para_props(1),
            ..Paragraph::default()
        };
        p.push(NO_TOC_ENTRIES, RunProps::default());
        lines.push(p);
    }

    // The field's start (and what precedes it) goes into the first line;
    // its end (and what follows) into the last, or stays in its own
    // paragraph when nothing of the old table shares it.
    let a = &paras[pa];
    let head_end = span.begin.offset + OBJ;
    let visible = |p: &Paragraph, r: Range<usize>| {
        p.run_ranges().any(|(rr, run)| {
            rr.start >= r.start
                && rr.end <= r.end
                && !rr.is_empty()
                && !run.object.as_ref().is_some_and(Object::is_zero_width)
        })
    };
    let mut out: Vec<Paragraph> = Vec::new();
    let first = if visible(a, 0..span.begin.offset) {
        // Text before the field keeps its paragraph (and the old result's
        // bookmarks, which `delete` keeps).
        let mut head = a.clone();
        let len = head.text.len();
        head.delete(head_end..len);
        head
    } else {
        let mut line = lines.remove(0);
        let head: Vec<Run> = a
            .run_ranges()
            .filter(|(r, _)| r.end <= head_end && !r.is_empty())
            .map(|(_, run)| run.clone())
            .collect();
        line.insert_runs(0, head);
        line
    };
    out.push(first);
    out.extend(lines);
    let b = &paras[pb];
    let tail: Vec<Run> = b
        .run_ranges()
        .filter(|(r, _)| r.start >= span.end.offset && !r.is_empty())
        .map(|(_, run)| run.clone())
        .collect();
    let tail_text: String = b.text[span.end.offset..].to_owned();
    let own_para = pa != pb && !visible(b, 0..span.end.offset);
    if own_para {
        let mut keep = b.clone();
        keep.delete(0..span.end.offset);
        out.push(keep);
    } else {
        let last = out.last_mut().expect("lines");
        let mut tail_p = Paragraph {
            text: tail_text,
            runs: tail,
            props: last.props.clone(),
            cell: last.cell,
        };
        tail_p.normalize();
        last.append(tail_p);
        if pb != pa {
            // The last old line's paragraph mark ends the table.
            let mut props = b.props.clone();
            if !b.props.extras.iter().any(|x| element_name(x) == "sectPr") {
                props = last.props.clone();
            }
            last.props = props;
        }
    }
    // A section break inside the old table stays with the last paragraph.
    if let Some(sect) = paras[pa..=pb].iter().rev().find_map(|p| {
        p.props
            .extras
            .iter()
            .find(|x| element_name(x) == "sectPr")
            .map(str::to_owned)
    }) && let Some(last) = out.last_mut()
        && !last
            .props
            .extras
            .iter()
            .any(|x| element_name(x) == "sectPr")
    {
        last.props.extras.push(&sect);
    }
    for p in &mut out {
        p.cell = paras[pa].cell;
    }
    paras.splice(pa..=pb, out);
    made
}

// ── updating ───────────────────────────────────────────────────────────────

/// Field types whose result Kantor leaves as it is (their nested fields
/// are updated instead).
fn keeps_result(kind: &str) -> bool {
    !matches!(
        kind,
        "DATE"
            | "TIME"
            | "CREATEDATE"
            | "SAVEDATE"
            | "PRINTDATE"
            | "AUTHOR"
            | "TITLE"
            | "SUBJECT"
            | "KEYWORDS"
            | "COMMENTS"
            | "LASTSAVEDBY"
            | "DOCPROPERTY"
            | "FILENAME"
            | "NUMWORDS"
            | "NUMCHARS"
            | "MERGEFIELD"
            | "SEQ"
            | "REF"
            | "NOTEREF"
            | "PAGEREF"
            | "TOC"
            | "="
            | "IF"
            | "COMPARE"
            | "STYLEREF"
            | "LISTNUM"
            | "INDEX"
    )
}

/// Field types worked out one at a time in document order, each seeing
/// the results before it (a column total of formulas).
fn in_order(kind: &str) -> bool {
    matches!(kind, "=" | "IF" | "COMPARE")
}

/// Which fields show their codes instead of their results (Word's
/// Alt+F9 for all of them, Shift+F9 for one): a view setting, not saved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodeView {
    /// Every field (Alt+F9).
    pub all: bool,
    /// These fields (by [`FieldSpan::mark_id`]) toggled one by one: with
    /// `all`, these show their results instead.
    pub ids: HashSet<usize>,
}

impl CodeView {
    /// Whether the field shows its code.
    pub fn shows(&self, id: usize) -> bool {
        self.all != self.ids.contains(&id)
    }
}

/// Where a hyperlink goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    /// A bookmark in this document.
    Bookmark(String),
    /// An address (web page, e-mail, file).
    Url(String),
}

impl Document {
    /// Which fields show their codes (layout reads it).
    pub fn field_codes(&self) -> Option<&Arc<CodeView>> {
        self.field_codes.as_ref()
    }

    /// Show field codes as `view` says (`None`: results everywhere).
    pub fn set_field_codes(&mut self, view: Option<CodeView>) {
        self.field_codes = view.filter(|v| v.all || !v.ids.is_empty()).map(Arc::new);
    }

    /// The fields in the body, ordered by where they start.
    pub fn fields(&self) -> Vec<FieldSpan> {
        spans(&self.paragraphs)
    }

    /// The outermost field around `pos`, if any.
    pub fn field_at(&self, pos: Position) -> Option<FieldSpan> {
        self.fields().into_iter().find(|s| s.contains(pos))
    }

    /// Update the fields in the selection, or the field around the caret
    /// (F9), as one undo step. `lay_out` lays out a version of the
    /// document so page references can be numbered. Returns the selection
    /// afterwards.
    pub fn update_fields(
        &mut self,
        sel: Selection,
        cx: &FieldContext,
        lay_out: &mut dyn FnMut(&Document) -> Box<dyn PageLookup>,
    ) -> Selection {
        let all = self.fields();
        let (a, b) = sel.ordered();
        let chosen: Vec<&FieldSpan> = if sel.is_empty() {
            all.iter().find(|s| s.contains(a)).into_iter().collect()
        } else {
            all.iter()
                .filter(|s| s.begin < b && Position::new(s.end.para, s.end.offset + OBJ) > a)
                .collect()
        };
        // Fields inside one whose result is replaced go with it.
        let replaced: Vec<&FieldSpan> = chosen
            .iter()
            .copied()
            .filter(|s| !keeps_result(&s.kind()))
            .collect();
        let targets: Vec<usize> = chosen
            .iter()
            .filter(|s| {
                !replaced
                    .iter()
                    .any(|r| r.id() != s.id() && r.begin < s.begin && s.end < r.end)
            })
            .map(|s| s.id())
            .collect();
        self.update_targets(sel, targets, cx, lay_out)
    }

    /// Rebuild the table of contents around the caret, or every one in the
    /// document when the caret isn't in one (Update Table), as one undo
    /// step. Returns the selection afterwards.
    pub fn update_tables_of_contents(
        &mut self,
        sel: Selection,
        cx: &FieldContext,
        lay_out: &mut dyn FnMut(&Document) -> Box<dyn PageLookup>,
    ) -> Selection {
        let tocs: Vec<FieldSpan> = self
            .fields()
            .into_iter()
            .filter(|s| s.kind() == "TOC")
            .collect();
        let here: Vec<usize> = tocs
            .iter()
            .filter(|s| s.contains(sel.head))
            .map(FieldSpan::id)
            .collect();
        let targets = if here.is_empty() {
            tocs.iter().map(FieldSpan::id).collect()
        } else {
            here
        };
        self.update_targets(sel, targets, cx, lay_out)
    }

    /// Whether the document has a table of contents.
    pub fn has_table_of_contents(&self) -> bool {
        self.fields().iter().any(|s| s.kind() == "TOC")
    }

    pub(crate) fn update_targets(
        &mut self,
        sel: Selection,
        targets: Vec<usize>,
        cx: &FieldContext,
        lay_out: &mut dyn FnMut(&Document) -> Box<dyn PageLookup>,
    ) -> Selection {
        if targets.is_empty() {
            return sel;
        }
        let mut targets: HashSet<usize> = targets.into_iter().collect();
        let mut paras = self.paragraphs.clone();

        // Tables of contents first: they change the paragraphs.
        let tocs: Vec<usize> = spans(&paras)
            .iter()
            .filter(|s| targets.contains(&s.id()) && s.kind() == "TOC")
            .map(FieldSpan::id)
            .collect();
        for id in tocs {
            if let Some(span) = spans(&paras).into_iter().find(|s| s.id() == id) {
                targets.extend(rebuild_toc(self, &mut paras, &span));
            }
        }

        // Fields that don't depend on the layout, last first so earlier
        // positions hold; cross-references after the rest, so they quote
        // the updated text (a caption's new number).
        for refs in [false, true] {
            let now = spans(&paras);
            let snapshot = self.snapshot(paras.clone());
            let mut changes: Vec<(FieldSpan, String)> = Vec::new();
            {
                let ev = Eval::new(&snapshot, &paras, cx, &now);
                for s in now.iter().filter(|s| targets.contains(&s.id())) {
                    let k = s.kind();
                    if k == "TOC"
                        || k == "PAGEREF"
                        || k == "INDEX"
                        || in_order(&k)
                        || matches!(k.as_str(), "REF" | "NOTEREF") != refs
                    {
                        continue;
                    }
                    if let Outcome::Text(t) = ev.eval(s) {
                        changes.push((s.clone(), t));
                    }
                }
            }
            for (s, t) in changes.into_iter().rev() {
                let id = set_result(&mut paras, &s, &t);
                targets.insert(id);
            }
        }

        // Formulas, IF and COMPARE in document order.
        let mut k = 0;
        loop {
            let now = spans(&paras);
            let Some(s) = now
                .iter()
                .filter(|s| targets.contains(&s.id()) && in_order(&s.kind()))
                .nth(k)
                .cloned()
            else {
                break;
            };
            k += 1;
            let snapshot = self.snapshot(paras.clone());
            let out = match Eval::new(&snapshot, &paras, cx, &now).eval(&s) {
                Outcome::Text(t) => t,
                Outcome::Keep => continue,
            };
            let id = set_result(&mut paras, &s, &out);
            targets.insert(id);
        }

        // Page references and indexes: from where the updated document is
        // laid out.
        let now = spans(&paras);
        let pagerefs: Vec<&FieldSpan> = now
            .iter()
            .filter(|s| {
                targets.contains(&s.id()) && matches!(s.kind().as_str(), "PAGEREF" | "INDEX")
            })
            .collect();
        if !pagerefs.is_empty() {
            let snapshot = self.snapshot(paras.clone());
            let pages = lay_out(&snapshot);
            let mut ev = Eval::new(&snapshot, &paras, cx, &now);
            ev.pages = Some(&*pages);
            let changes: Vec<(FieldSpan, String)> = pagerefs
                .iter()
                .filter_map(|s| match ev.eval(s) {
                    Outcome::Text(t) => Some(((*s).clone(), t)),
                    Outcome::Keep => None,
                })
                .collect();
            drop(ev);
            for (s, t) in changes.into_iter().rev() {
                set_result(&mut paras, &s, &t);
            }
        }

        if paras == self.paragraphs {
            return sel;
        }
        let old = &self.paragraphs;
        let start = old.iter().zip(&paras).take_while(|(a, b)| a == b).count();
        let tail = old[start..]
            .iter()
            .rev()
            .zip(paras[start..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let count = old.len() - start - tail;
        let new: Vec<Paragraph> = paras[start..paras.len() - tail].to_vec();
        // With Track Changes on, the new results are tracked insertions and
        // the old ones deletions (Word's behaviour).
        let new = self.track_rewrite(&self.paragraphs[start..start + count], new);
        let after = Selection::caret(sel.head);
        self.replace(start, count, new, (sel, after), Kind::Other, true);
        self.clamp_selection(after)
    }

    /// Where the hyperlink at `pos` goes: a hyperlink's run, or a
    /// `HYPERLINK` field's result.
    pub fn link_at(&self, pos: Position) -> Option<LinkTarget> {
        let p = self.paragraphs.get(pos.para)?;
        let run = p
            .run_ranges()
            .find(|(r, _)| r.start <= pos.offset && pos.offset < r.end)
            .or_else(|| {
                p.run_ranges()
                    .find(|(r, _)| r.end == pos.offset && !r.is_empty())
            })
            .map(|(_, run)| run);
        if let Some(tag) = run.and_then(|r| r.link.as_deref())
            && let Some(el) = xml::parse(&format!("{tag}</w:hyperlink>"))
        {
            if let Some(id) = el.attr("id")
                && let Some(url) = self.package.as_ref().and_then(|p| p.rel_target(id))
            {
                return Some(LinkTarget::Url(match el.attr("anchor") {
                    Some(a) => format!("{url}#{a}"),
                    None => url,
                }));
            }
            if let Some(a) = el.attr("anchor") {
                return Some(LinkTarget::Bookmark(a.to_owned()));
            }
        }
        let field = self
            .fields()
            .into_iter()
            .rfind(|s| s.contains(pos) && s.kind() == "HYPERLINK")?;
        let i = Instr::parse(&field.instr);
        match (i.args.first(), i.arg("l")) {
            (Some(url), Some(a)) => Some(LinkTarget::Url(format!("{url}#{a}"))),
            (Some(url), None) => Some(LinkTarget::Url(url.clone())),
            (None, Some(a)) => Some(LinkTarget::Bookmark(a.to_owned())),
            (None, None) => None,
        }
    }

    /// Where a bookmark starts.
    pub fn bookmark_position(&self, name: &str) -> Option<Position> {
        let b = bookmarks(&self.paragraphs);
        b.get(name)
            .or_else(|| {
                b.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(name))
                    .map(|(_, v)| v)
            })
            .map(|b| b.start)
    }
}

/// What a field with no result shows: «Name» for a mail merge field
/// (Word's display without a data source).
pub fn empty_result_label(instr: &str) -> Option<String> {
    let i = Instr::parse(instr);
    (i.kind == "MERGEFIELD")
        .then(|| i.args.first().map(|n| format!("«{n}»")))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn instructions_parse_switches_and_quotes() {
        let i = Instr::parse(r#" TOC \o "1-3" \h \z \u "#);
        assert_eq!(i.kind, "TOC");
        assert_eq!(i.arg("o"), Some("1-3"));
        assert!(i.has("h") && i.has("z") && i.has("u"));
        let i = Instr::parse(r#" DATE \@ "d MMMM yyyy" \* MERGEFORMAT "#);
        assert_eq!(i.arg("@"), Some("d MMMM yyyy"));
        assert!(i.merge_format());
        let i = Instr::parse(" SEQ Figure \\* ARABIC \\r 3 ");
        assert_eq!(i.args, ["Figure"]);
        assert_eq!(i.arg("r"), Some("3"));
        let i = Instr::parse(" REF _Ref12 \\h ");
        assert_eq!(i.args, ["_Ref12"]);
        assert_eq!(i.arg("h"), None);
        let i = Instr::parse(r#"HYPERLINK "https://kantor.paparanai.id" \l "top""#);
        assert_eq!(i.args, ["https://kantor.paparanai.id"]);
        assert_eq!(i.arg("l"), Some("top"));
        assert_eq!(Instr::parse("=2*3").kind, "=");
    }

    #[test]
    fn number_and_case_formats() {
        let n = |s: &str, v| Instr::parse(s).number(v);
        assert_eq!(n("PAGE", 4), "4");
        assert_eq!(n("PAGE \\* roman", 4), "iv");
        assert_eq!(n("PAGE \\* ROMAN", 14), "XIV");
        assert_eq!(n("SEQ x \\* alphabetic", 3), "c");
        assert_eq!(
            n("SEQ x \\* ALPHABETIC", 28),
            "BB",
            "Word repeats the letter"
        );
        assert_eq!(n("SEQ x \\* Ordinal", 22), "22nd");
        assert_eq!(n("SEQ x \\* Arabic \\* MERGEFORMAT", 7), "7");
        let c = |s: &str, v| Instr::parse(s).cased(v);
        assert_eq!(c("TITLE \\* Upper", "Laporan tahunan"), "LAPORAN TAHUNAN");
        assert_eq!(c("TITLE \\* Lower", "Laporan"), "laporan");
        assert_eq!(c("TITLE \\* Caps", "laporan tahunan"), "Laporan Tahunan");
        assert_eq!(
            c("TITLE \\* FirstCap", "laporan tahunan"),
            "Laporan tahunan"
        );
    }

    #[test]
    fn date_pictures() {
        let t = NaiveDate::from_ymd_opt(2026, 3, 5)
            .unwrap()
            .and_hms_opt(14, 7, 9)
            .unwrap();
        let f = |p| format_date(t, p, false);
        assert_eq!(f("M/d/yyyy"), "3/5/2026");
        assert_eq!(f("dd/MM/yy"), "05/03/26");
        assert_eq!(f("d MMMM yyyy"), "5 March 2026");
        assert_eq!(f("dddd, MMM d"), "Thursday, Mar 5");
        assert_eq!(f("HH:mm"), "14:07");
        assert_eq!(f("h:mm am/pm"), "2:07 pm");
        assert_eq!(f("h:mm AM/PM"), "2:07 PM");
        assert_eq!(f("hh:mm:ss"), "14:07:09");
        assert_eq!(f("'Week of' d MMM"), "Week of 5 Mar");
        assert_eq!(
            format_date(t, "dddd, d MMMM yyyy", true),
            "Kamis, 5 Maret 2026"
        );
    }

    #[test]
    fn spans_pair_nested_marks() {
        let mut p = Paragraph::new("");
        let mark = |m: FieldMark| Object::Field(Arc::new(m));
        p.push_object(
            mark(complex_begin(" TOC \\o ", true)),
            RunProps::default(),
            None,
        );
        p.push("Intro\t", RunProps::default());
        p.push_object(
            mark(complex_begin(" PAGEREF _Toc1 ", true)),
            RunProps::default(),
            None,
        );
        p.push("1", RunProps::default());
        p.push_object(mark(FieldMark::end()), RunProps::default(), None);
        let mut q = Paragraph::new("");
        q.push_object(mark(FieldMark::end()), RunProps::default(), None);
        let paras = vec![p, q];
        let s = spans(&paras);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].kind(), "TOC");
        assert_eq!(s[0].end, Position::new(1, 0));
        assert_eq!(s[1].kind(), "PAGEREF");
        assert_eq!(s[1].depth, 1);
        assert!(writable(&paras).is_none(), "balanced");
        // A lone end is dropped when writing.
        let lone = vec![paras[1].clone()];
        let w = writable(&lone).unwrap();
        assert_eq!(w[0].text, "");
    }
}
