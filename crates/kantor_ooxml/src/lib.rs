//! Open XML package helpers shared by Kantor's readers (Sheets, Docs, Slides).
//!
//! # Strict Open XML
//!
//! Office can save "Strict Open XML" files (ISO/IEC 29500 Strict): the same
//! package and part names as the usual Transitional files, but other
//! namespace and relationship-type URIs (`http://purl.oclc.org/ooxml/...`)
//! and a few values written differently:
//!
//! - SpreadsheetML date cells (`t="d"`) hold an ISO 8601 date instead of a
//!   serial number;
//! - DrawingML and PresentationML percentages are `"35%"` instead of
//!   thousandths of a percent (`35000`), chart percentages `"50%"` instead of
//!   whole percents (`50`), and lengths may carry a unit (`"1in"`);
//! - WordprocessingML table widths in percent are `"50%"` instead of
//!   fiftieths of a percent, and lengths may carry a unit;
//! - the root element says `conformance="strict"`.
//!
//! Kantor's readers read Transitional, so [`transitional`] rewrites a Strict
//! package as Transitional before they run (as LibreOffice and the Open XML
//! SDK map Strict to Transitional on reading). The rewritten package is what
//! the document keeps, so a save writes Transitional, Office's default.
//! Values the same in both (booleans `true`/`false`, `start`/`end` sides)
//! are left alone: Transitional allows them too and the readers take them.

mod layout;
pub mod officeart;
pub mod pattern;

pub use layout::{needs_spreadsheet_layout, spreadsheet_layout};

use std::borrow::Cow;
use std::io::{Cursor, Read, Seek, Write};

use anyhow::{Context, Result};
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::reader::NsReader;
use zip::write::SimpleFileOptions;

/// The start of every Strict namespace and relationship type.
const STRICT: &str = "http://purl.oclc.org/ooxml/";
const STRICT_OFFICE_DOCUMENT: &str =
    "http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument";
const TRANSITIONAL: &str = "http://schemas.openxmlformats.org/";
const PACKAGE_RELS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

/// Whether a package is Strict Open XML: its main part is related with the
/// Strict `officeDocument` relationship type.
pub fn is_strict(pkg: &[u8]) -> bool {
    is_strict_package(Cursor::new(pkg))
}

/// [`is_strict`] for a package read from a file or another reader; only
/// `_rels/.rels` is read.
pub fn is_strict_package(reader: impl Read + Seek) -> bool {
    let Ok(mut zip) = zip::ZipArchive::new(reader) else {
        return false;
    };
    let Ok(mut rels) = zip.by_name("_rels/.rels") else {
        return false;
    };
    let mut text = String::new();
    rels.by_ref()
        .take(16 * 1024 * 1024)
        .read_to_string(&mut text)
        .is_ok()
        && text.contains(STRICT_OFFICE_DOCUMENT)
}

/// The package as Transitional Open XML: a Strict package rewritten (see
/// the module docs), any other one returned as it is.
pub fn transitional(pkg: Vec<u8>) -> Result<Vec<u8>> {
    if is_strict(&pkg) {
        to_transitional(&pkg).context("converting Strict Open XML")
    } else {
        Ok(pkg)
    }
}

/// Rewrite a Strict package as Transitional. Parts that don't change (media,
/// parts without Strict URIs) are copied as they are.
pub fn to_transitional(pkg: &[u8]) -> Result<Vec<u8>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(pkg)).context("not a zip package")?;
    let date1904 = uses_1904_dates(&mut zip);
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::with_capacity(pkg.len() + 1024)));
    let deflate = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for i in 0..zip.len() {
        let converted = {
            let mut file = zip.by_index(i)?;
            let name = file.name()?.into_owned();
            if file.is_dir() || !is_xml_part(&name) {
                None
            } else {
                let mut bytes = Vec::with_capacity(file.size() as usize);
                file.read_to_end(&mut bytes)
                    .with_context(|| format!("reading {name}"))?;
                convert_part(&bytes, date1904).map(|b| (name, b))
            }
        };
        match converted {
            Some((name, bytes)) => {
                out.start_file(name, deflate)?;
                out.write_all(&bytes)?;
            }
            None => out.raw_copy_file(zip.by_index(i)?)?,
        }
    }
    Ok(out.finish()?.into_inner())
}

fn is_xml_part(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".xml") || lower.ends_with(".rels")
}

/// Whether the workbook counts dates from 1904 (`workbookPr date1904`).
fn uses_1904_dates<R: Read + Seek>(zip: &mut zip::ZipArchive<R>) -> bool {
    let names: Vec<String> = zip
        .file_names()
        .filter_map(|n| n.ok().map(|n| n.into_owned()))
        .filter(|n| n.to_ascii_lowercase().ends_with("workbook.xml"))
        .collect();
    names.iter().any(|n| {
        let mut text = String::new();
        zip.by_name(n).is_ok_and(|mut f| {
            f.by_ref()
                .take(64 * 1024 * 1024)
                .read_to_string(&mut text)
                .is_ok()
        }) && (text.contains("date1904=\"1\"") || text.contains("date1904=\"true\""))
    })
}

/// One XML part as Transitional, or `None` when nothing changes.
fn convert_part(bytes: &[u8], date1904: bool) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mapped = map_uris(text);
    let converted = if has_strict_values(&mapped) {
        convert_values(&mapped, date1904)
    } else {
        None
    };
    match (converted, mapped) {
        (Some(c), _) => Some(c.into_bytes()),
        (None, Cow::Owned(m)) => Some(m.into_bytes()),
        (None, Cow::Borrowed(_)) => None,
    }
}

// ---------------------------------------------------------------- URIs

/// Replace every Strict URI in the text by its Transitional equivalent.
/// They only appear as namespace declarations, relationship types and
/// `a:graphicData`'s `uri`, never in document text a reader shows.
fn map_uris(text: &str) -> Cow<'_, str> {
    if !text.contains(STRICT) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 512);
    let mut rest = text;
    while let Some(i) = rest.find(STRICT) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let end = tail
            .find(|c: char| matches!(c, '"' | '\'' | '<' | '>') || c.is_whitespace())
            .unwrap_or(tail.len());
        let uri = &tail[..end];
        match transitional_uri(uri) {
            Some(t) => out.push_str(&t),
            None => out.push_str(uri),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// The Transitional URI for a Strict one: `http://purl.oclc.org/ooxml/<area>/<name>`
/// is `http://schemas.openxmlformats.org/<area>/2006/<name>`, relationship
/// types keep their last segment, with the few names the two spell
/// differently (as in LibreOffice's `oox` namespace tables).
pub fn transitional_uri(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix(STRICT)?;
    if let Some(ty) = rest.strip_prefix("officeDocument/relationships") {
        let ty = match ty {
            "/metadata/thumbnail" => return Some(format!("{PACKAGE_RELS}/metadata/thumbnail")),
            "/metadata/core-properties" => {
                return Some(format!("{PACKAGE_RELS}/metadata/core-properties"));
            }
            "/extendedProperties" => "/extended-properties",
            "/customProperties" => "/custom-properties",
            "" => "",
            t if t.starts_with('/') => t,
            _ => return None,
        };
        return Some(format!(
            "{TRANSITIONAL}officeDocument/2006/relationships{ty}"
        ));
    }
    let (area, name) = rest.split_once('/')?;
    if area.is_empty() || name.is_empty() || name.contains('/') {
        return None;
    }
    let name = match name {
        "extendedProperties" => "extended-properties",
        "customProperties" => "custom-properties",
        n => n,
    };
    Some(format!("{TRANSITIONAL}{area}/2006/{name}"))
}

// ---------------------------------------------------------------- values

/// A cheap test for values worth a parse: a percentage, a length with a
/// unit, a date cell or the conformance attribute.
fn has_strict_values(text: &str) -> bool {
    text.contains("%\"")
        || text.contains("t=\"d\"")
        || text.contains("conformance=")
        || ["mm\"", "cm\"", "in\"", "pt\"", "pc\"", "pi\""]
            .iter()
            .any(|u| text.contains(u))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// DrawingML (except charts) and PresentationML: percentages in
    /// thousandths, lengths in EMU.
    Drawing,
    /// DrawingML charts: percentages in whole percents.
    Chart,
    /// WordprocessingML: table percentages in fiftieths, lengths in twips
    /// (font sizes in half-points).
    Word,
    /// SpreadsheetML: date cells.
    Sheet,
    Other,
}

fn kind_of(ns: &[u8]) -> Kind {
    const DRAWINGML: &[u8] = b"http://schemas.openxmlformats.org/drawingml/2006/";
    match ns {
        b"http://schemas.openxmlformats.org/drawingml/2006/chart" => Kind::Chart,
        b"http://schemas.openxmlformats.org/presentationml/2006/main" => Kind::Drawing,
        b"http://schemas.openxmlformats.org/wordprocessingml/2006/main" => Kind::Word,
        b"http://schemas.openxmlformats.org/spreadsheetml/2006/main" => Kind::Sheet,
        n if n.starts_with(DRAWINGML) => Kind::Drawing,
        _ => Kind::Other,
    }
}

/// Attributes that hold names or text, never a measure (a shape called
/// "50%" stays so).
const TEXT_ATTRS: &[&str] = &[
    "name",
    "descr",
    "title",
    "tooltip",
    "typeface",
    "panose",
    "uri",
    "macro",
    "fmla",
    "id",
    "action",
    "tgtFrame",
    "invalidUrl",
    "altLang",
    "lang",
    "styleName",
    "type",
];

/// Word elements whose `val` is a name (a style called "12pt" stays so).
const WORD_TEXT_ELEMENTS: &[&str] = &[
    "name",
    "alias",
    "aliases",
    "tag",
    "docVar",
    "basedOn",
    "next",
    "link",
    "lang",
    "rFonts",
    "font",
    "altName",
    "attachedTemplate",
    "placeholder",
    "docPart",
    "docPartGallery",
    "docPartCategory",
    "lid",
];

/// Rewrite Strict values in one part (namespaces already Transitional).
/// `None` when nothing changes or the XML can't be read (the reader then
/// sees it as it was).
fn convert_values(text: &str, date1904: bool) -> Option<String> {
    let mut reader = NsReader::from_str(text);
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut depth = 0usize;
    // The open `c` is a date cell: its tag's edit and the tag to use if its
    // value turns out not to be a date.
    let mut date_cell: Option<(usize, String)> = None;
    let mut in_value = false;
    loop {
        let before = reader.buffer_position() as usize;
        let (ns, event) = reader.read_resolved_event().ok()?;
        let kind = match ns {
            ResolveResult::Bound(Namespace(n)) => kind_of(n),
            _ => Kind::Other,
        };
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let empty = matches!(event, Event::Empty(_));
                let local = e.local_name();
                let local = std::str::from_utf8(local.as_ref()).unwrap_or_default();
                let root = depth == 0;
                if let Some(tag) = rewrite_tag(e, kind, local, root, empty) {
                    if tag.date {
                        date_cell = Some((edits.len(), tag.as_text));
                    }
                    edits.push((before, after, tag.text));
                }
                if kind == Kind::Sheet && local == "v" && date_cell.is_some() && !empty {
                    in_value = true;
                }
                if !empty {
                    depth += 1;
                }
            }
            Event::End(ref e) => {
                depth = depth.saturating_sub(1);
                if kind == Kind::Sheet {
                    match e.local_name().as_ref() {
                        b"v" => in_value = false,
                        b"c" => date_cell = None,
                        _ => {}
                    }
                }
            }
            Event::Text(ref t) if in_value => {
                let value = t.decode().ok()?;
                match iso_to_serial(value.trim(), date1904) {
                    Some(serial) => edits.push((before, after, serial)),
                    None => {
                        // Not a date after all: keep it as text.
                        if let Some((i, as_text)) = date_cell.take() {
                            edits[i].2 = as_text;
                        }
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if edits.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (start, end, replacement) in edits {
        out.push_str(&text[at..start]);
        out.push_str(&replacement);
        at = end;
    }
    out.push_str(&text[at..]);
    Some(out)
}

struct NewTag {
    text: String,
    /// A date cell (`t="d"` dropped); `as_text` keeps its value as text.
    date: bool,
    as_text: String,
}

/// A start tag with its Strict values rewritten, or `None` if none are.
fn rewrite_tag(
    e: &BytesStart<'_>,
    kind: Kind,
    local: &str,
    root: bool,
    empty: bool,
) -> Option<NewTag> {
    let mut changed = false;
    let mut date = false;
    let mut attrs: Vec<(String, Cow<'_, str>)> = Vec::new();
    for attr in e.attributes().with_checks(false) {
        let attr = attr.ok()?;
        let key = std::str::from_utf8(attr.key.as_ref()).ok()?.to_owned();
        let raw = match attr.value {
            Cow::Borrowed(b) => Cow::Borrowed(std::str::from_utf8(b).ok()?),
            Cow::Owned(b) => Cow::Owned(String::from_utf8(b).ok()?),
        };
        let key_local = key.rsplit(':').next().unwrap_or(&key);
        if root && key_local == "conformance" && !key.starts_with("xmlns") {
            changed = true;
            continue;
        }
        if kind == Kind::Sheet && local == "c" && key == "t" && raw == "d" {
            changed = true;
            date = true;
            continue;
        }
        if let Some(v) = convert_value(kind, local, key_local, &raw) {
            changed = true;
            attrs.push((key, Cow::Owned(v)));
        } else {
            attrs.push((key, raw));
        }
    }
    if !changed {
        return None;
    }
    let write = |extra: Option<(&str, &str)>| {
        let mut s = String::with_capacity(64);
        s.push('<');
        s.push_str(std::str::from_utf8(e.name().as_ref()).unwrap_or_default());
        for (k, v) in attrs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_ref()))
            .chain(extra)
        {
            let q = if v.contains('"') { '\'' } else { '"' };
            s.push(' ');
            s.push_str(k);
            s.push('=');
            s.push(q);
            s.push_str(v);
            s.push(q);
        }
        s.push_str(if empty { "/>" } else { ">" });
        s
    };
    Some(NewTag {
        text: write(None),
        date,
        as_text: if date {
            write(Some(("t", "str")))
        } else {
            String::new()
        },
    })
}

/// One attribute value in Transitional form, if Strict writes it otherwise.
fn convert_value(kind: Kind, element: &str, attr: &str, value: &str) -> Option<String> {
    if kind == Kind::Other || kind == Kind::Sheet || TEXT_ATTRS.contains(&attr) {
        return None;
    }
    if attr.starts_with("xmlns") {
        return None;
    }
    if let Some(p) = value.strip_suffix('%').and_then(number) {
        return Some(int(match kind {
            Kind::Chart => p,
            Kind::Word => p * 50.0,
            _ => p * 1000.0,
        }));
    }
    let (n, unit) = universal_measure(value)?;
    match kind {
        Kind::Drawing => Some(int(n * emu_per(unit))),
        Kind::Word if !WORD_TEXT_ELEMENTS.contains(&element) && !element.ends_with("Style") => {
            let points = n * emu_per(unit) / 12700.0;
            Some(int(match element {
                "sz" | "szCs" | "kern" | "position" => points * 2.0,
                _ => points * 20.0,
            }))
        }
        _ => None,
    }
}

/// `-?[0-9]+(\.[0-9]+)?` as a number.
fn number(s: &str) -> Option<f64> {
    let digits = s.strip_prefix('-').unwrap_or(s);
    let (int, frac) = digits.split_once('.').unwrap_or((digits, "0"));
    let ok = |d: &str| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit());
    if !(ok(int) && ok(frac)) {
        return None;
    }
    s.parse().ok()
}

/// A length with a unit (ST_UniversalMeasure): `"2.5in"` is `(2.5, "in")`.
fn universal_measure(s: &str) -> Option<(f64, &str)> {
    let unit = s.get(s.len().checked_sub(2)?..)?;
    if !matches!(unit, "mm" | "cm" | "in" | "pt" | "pc" | "pi") {
        return None;
    }
    Some((number(&s[..s.len() - 2])?, unit))
}

fn emu_per(unit: &str) -> f64 {
    match unit {
        "in" => 914_400.0,
        "cm" => 360_000.0,
        "mm" => 36_000.0,
        "pt" => 12_700.0,
        _ => 152_400.0, // pc, pi: 12 pt
    }
}

fn int(x: f64) -> String {
    format!("{}", x.round() as i64)
}

// ---------------------------------------------------------------- dates

/// An ISO 8601 date or date-time (`2024-02-29`, `2024-02-29T12:30:00`,
/// with an optional fraction and zone, which is ignored as Excel does) as
/// a spreadsheet serial number in the workbook's date system.
pub fn iso_to_serial(s: &str, date1904: bool) -> Option<String> {
    let (date, time) = match s.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };
    let mut parts = date.splitn(3, '-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || !(0..=9999).contains(&y) {
        return None;
    }
    let day = days_from_civil(y, m, d);
    let serial = if date1904 {
        day - days_from_civil(1904, 1, 1)
    } else {
        // Excel counts 1900-02-29, which didn't exist: one day more from March 1900.
        let n = day - days_from_civil(1899, 12, 31);
        if (y, m, d) >= (1900, 3, 1) { n + 1 } else { n }
    };
    let mut seconds = 0.0;
    if let Some(t) = time {
        let t = t.trim_end_matches('Z');
        let t = match t.find(['+', '-']) {
            Some(i) => &t[..i],
            None => t,
        };
        let mut hms = t.splitn(3, ':');
        let h: f64 = hms.next()?.parse().ok()?;
        let mi: f64 = hms.next()?.parse().ok()?;
        let sec: f64 = hms.next().map_or(Some(0.0), |x| x.parse().ok())?;
        if !(0.0..=24.0).contains(&h) || !(0.0..60.0).contains(&mi) || !(0.0..61.0).contains(&sec) {
            return None;
        }
        seconds = h * 3600.0 + mi * 60.0 + sec;
    }
    if seconds == 0.0 {
        Some(serial.to_string())
    } else {
        Some(format!("{}", serial as f64 + seconds / 86_400.0))
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris() {
        let t = |s: &str| transitional_uri(s).unwrap();
        assert_eq!(
            t("http://purl.oclc.org/ooxml/spreadsheetml/main"),
            "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
        );
        assert_eq!(
            t("http://purl.oclc.org/ooxml/drawingml/wordprocessingDrawing"),
            "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
        );
        assert_eq!(
            t("http://purl.oclc.org/ooxml/officeDocument/extendedProperties"),
            "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"
        );
        assert_eq!(
            t("http://purl.oclc.org/ooxml/officeDocument/relationships"),
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
        );
        assert_eq!(
            t("http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument"),
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument"
        );
        assert_eq!(
            t("http://purl.oclc.org/ooxml/officeDocument/relationships/extendedProperties"),
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties"
        );
        assert_eq!(
            t("http://purl.oclc.org/ooxml/officeDocument/relationships/metadata/thumbnail"),
            "http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail"
        );
        assert_eq!(transitional_uri("http://purl.oclc.org/ooxml/odd"), None);
    }

    #[test]
    fn dates() {
        let s = |d: &str| iso_to_serial(d, false).unwrap();
        assert_eq!(s("1900-01-01"), "1");
        assert_eq!(s("1900-02-28"), "59");
        assert_eq!(s("1900-03-01"), "61");
        assert_eq!(s("1990-01-01"), "32874");
        assert_eq!(s("2024-02-29T12:00:00"), "45351.5");
        assert_eq!(s("2024-02-29T06:00:00.000Z"), "45351.25");
        assert_eq!(s("1899-12-31T18:00:00"), "0.75");
        assert_eq!(iso_to_serial("1904-01-02", true).unwrap(), "1");
        assert_eq!(iso_to_serial("hello", false), None);
        assert_eq!(iso_to_serial("2024-13-01", false), None);
    }

    fn part(xml: &str) -> String {
        String::from_utf8(convert_part(xml.as_bytes(), false).expect("changed")).unwrap()
    }

    #[test]
    fn date_cells() {
        let xml = r#"<worksheet xmlns="http://purl.oclc.org/ooxml/spreadsheetml/main" conformance="strict"><sheetData><row r="1"><c r="A1" s="1" t="d"><v>1990-01-01</v></c><c r="B1" t="d"><v>soon</v></c><c r="C1" t="d"><f>A1+1</f><v>1990-01-02T12:00:00</v></c><c r="D1"><v>5</v></c></row></sheetData></worksheet>"#;
        assert_eq!(
            part(xml),
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" s="1"><v>32874</v></c><c r="B1" t="str"><v>soon</v></c><c r="C1"><f>A1+1</f><v>32875.5</v></c><c r="D1"><v>5</v></c></row></sheetData></worksheet>"#
        );
    }

    #[test]
    fn drawing_values() {
        let xml = r#"<p:sld xmlns:p="http://purl.oclc.org/ooxml/presentationml/main" xmlns:a="http://purl.oclc.org/ooxml/drawingml/main" xmlns:c="http://purl.oclc.org/ooxml/drawingml/chart"><p:cNvPr id="2" name="50%"/><a:alpha val="35%"/><a:fillToRect l="50%" t="12.5%"/><a:off x="1in" y="0"/><c:holeSize val="50%"/><p:restoredLeft sz="15.987%"/><x v="50%"/></p:sld>"#;
        assert_eq!(
            part(xml),
            r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"><p:cNvPr id="2" name="50%"/><a:alpha val="35000"/><a:fillToRect l="50000" t="12500"/><a:off x="914400" y="0"/><c:holeSize val="50"/><p:restoredLeft sz="15987"/><x v="50%"/></p:sld>"#
        );
    }

    #[test]
    fn word_values() {
        let xml = r#"<w:document xmlns:w="http://purl.oclc.org/ooxml/wordprocessingml/main" w:conformance="strict"><w:body><w:tbl><w:tblPr><w:tblW w:w="50%" w:type="pct"/></w:tblPr></w:tbl><w:p><w:pPr><w:pStyle w:val="12pt"/><w:ind w:start="1in" w:hanging="0.5cm"/></w:pPr><w:r><w:rPr><w:sz w:val="12pt"/><w:b w:val="true"/></w:rPr><w:t>50%</w:t></w:r></w:p></w:body></w:document>"#;
        assert_eq!(
            part(xml),
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:tbl><w:tblPr><w:tblW w:w="2500" w:type="pct"/></w:tblPr></w:tbl><w:p><w:pPr><w:pStyle w:val="12pt"/><w:ind w:start="1440" w:hanging="283"/></w:pPr><w:r><w:rPr><w:sz w:val="24"/><w:b w:val="true"/></w:rPr><w:t>50%</w:t></w:r></w:p></w:body></w:document>"#
        );
    }

    #[test]
    fn unchanged_parts_are_none() {
        let xml = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><c r="A1"><v>1</v></c></worksheet>"#;
        assert!(convert_part(xml.as_bytes(), false).is_none());
    }

    fn zip_of(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, text) in parts {
            w.start_file(*name, SimpleFileOptions::default()).unwrap();
            w.write_all(text.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn read(pkg: &[u8], name: &str) -> String {
        let mut z = zip::ZipArchive::new(Cursor::new(pkg)).unwrap();
        let mut s = String::new();
        z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
        s
    }

    #[test]
    fn packages() {
        let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
        let pkg = zip_of(&[
            ("_rels/.rels", rels),
            (
                "xl/workbook.xml",
                r#"<workbook xmlns="http://purl.oclc.org/ooxml/spreadsheetml/main"><workbookPr date1904="1"/></workbook>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet xmlns="http://purl.oclc.org/ooxml/spreadsheetml/main"><sheetData><row r="1"><c r="A1" t="d"><v>1904-01-03</v></c></row></sheetData></worksheet>"#,
            ),
            ("xl/media/image1.png", "\u{89}PNG not really"),
        ]);
        assert!(is_strict(&pkg));
        let out = transitional(pkg).unwrap();
        assert!(!is_strict(&out));
        assert!(read(&out, "_rels/.rels").contains(
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument"
        ));
        assert!(read(&out, "xl/worksheets/sheet1.xml").contains(r#"<c r="A1"><v>2</v></c>"#));
        assert_eq!(read(&out, "xl/media/image1.png"), "\u{89}PNG not really");

        // A Transitional package comes back as it was.
        let plain = zip_of(&[("_rels/.rels", "<Relationships/>")]);
        assert_eq!(transitional(plain.clone()).unwrap(), plain);
    }
}
