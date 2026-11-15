//! Spreadsheet packages laid out differently from Excel's own.
//!
//! The package format (OPC) finds parts through relationships, and part
//! names are case-insensitive, but Excel always writes the workbook as
//! `xl/workbook.xml`, its styles as `xl/styles.xml` and its strings as
//! `xl/sharedStrings.xml`, and IronCalc reads them from there. Other
//! writers don't always:
//!
//! - the workbook part named otherwise or elsewhere (`xl/workbook2.xml`,
//!   `/workbook.xml` with its sheets next to it);
//! - part names with backslashes (`xl\worksheets\sheet1.xml`; some .NET
//!   writers) and in another case than the relationships give
//!   (`xl/sharedstrings.xml`, `[content_types].xml`).
//!
//! [`spreadsheet_layout`] moves those parts to Excel's names and points the
//! relationships and content types at them; Excel and LibreOffice open such
//! files the same way.

use std::collections::HashMap;
use std::io::{Cursor, Read, Seek, Write};

use anyhow::{Context, Result};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use zip::write::SimpleFileOptions;

const WORKBOOK: &str = "xl/workbook.xml";
const WORKBOOK_RELS: &str = "xl/_rels/workbook.xml.rels";
const CONTENT_TYPES: &str = "[Content_Types].xml";
const ROOT_RELS: &str = "_rels/.rels";
const OFFICE_DOCUMENT: &str = "/officeDocument";
/// Parts IronCalc reads by name rather than through the workbook's
/// relationships: relationship type ending, Excel's part name.
const FIXED: [(&str, &str); 2] = [
    ("/styles", "xl/styles.xml"),
    ("/sharedStrings", "xl/sharedStrings.xml"),
];

/// Whether the package's part names need [`spreadsheet_layout`]: reads only
/// the zip's directory.
pub fn needs_spreadsheet_layout(reader: impl Read + Seek) -> bool {
    let Ok(zip) = zip::ZipArchive::new(reader) else {
        return false;
    };
    let names: Vec<String> = zip
        .file_names()
        .filter_map(|n| n.ok().map(|n| n.into_owned()))
        .collect();
    names.iter().any(|n| n.contains('\\'))
        || ![WORKBOOK, CONTENT_TYPES, ROOT_RELS]
            .iter()
            .all(|want| names.iter().any(|n| n == want))
}

/// The package with its workbook, styles and shared strings at Excel's part
/// names (see the module docs), or `None` when it already is laid out so or
/// its main part can't be found.
pub fn spreadsheet_layout(pkg: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(pkg)).context("not a zip package")?;
    // Every part under a forward-slash name.
    let mut parts: Vec<(String, Vec<u8>)> = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        if file.is_dir() {
            continue;
        }
        let name = file.name()?.replace('\\', "/");
        let name = name.trim_start_matches('/').to_string();
        let mut bytes = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut bytes)
            .with_context(|| format!("reading {name}"))?;
        parts.push((name, bytes));
    }
    let original: Vec<String> = parts.iter().map(|p| p.0.clone()).collect();

    // Names in the case the package refers to them.
    case_as_referenced(&mut parts);

    let find = |parts: &[(String, Vec<u8>)], name: &str| parts.iter().position(|p| p.0 == name);
    let Some(root) = find(&parts, ROOT_RELS) else {
        return Ok(None);
    };
    let Some(main) = relationships(&parts[root].1)
        .into_iter()
        .find(|r| r.kind.ends_with(OFFICE_DOCUMENT) && !r.external)
        .map(|r| resolve_part("", &r.target))
    else {
        return Ok(None);
    };
    if find(&parts, &main).is_none() {
        return Ok(None);
    }

    // Moves: old name -> new name.
    let mut moves: HashMap<String, String> = HashMap::new();
    let main_rels = rels_part(&main);
    if main != WORKBOOK {
        moves.insert(main.clone(), WORKBOOK.into());
        moves.insert(main_rels.clone(), WORKBOOK_RELS.into());
    }
    let workbook_rels = find(&parts, &main_rels)
        .map(|i| relationships(&parts[i].1))
        .unwrap_or_default();
    for (kind, fixed) in FIXED {
        if let Some(r) = workbook_rels
            .iter()
            .find(|r| r.kind.ends_with(kind) && !r.external)
        {
            let at = resolve_part(&main, &r.target);
            if at != fixed && find(&parts, &at).is_some() && find(&parts, fixed).is_none() {
                moves.insert(at.clone(), fixed.into());
                if find(&parts, &rels_part(&at)).is_some() {
                    moves.insert(rels_part(&at), rels_part(fixed));
                }
            }
        }
    }

    let renamed = parts.iter().map(|p| &p.0).ne(original.iter());
    if moves.is_empty() && !renamed {
        return Ok(None);
    }

    let moved = |name: &str| moves.get(name).cloned().unwrap_or_else(|| name.to_string());
    for part in &mut parts {
        let source_of_rels = rels_source(&part.0);
        if part.0 == ROOT_RELS {
            part.1 = retarget(&part.1, |t| {
                let at = resolve_part("", t);
                moves.get(&at).cloned()
            });
        } else if part.0 == main_rels && main != WORKBOOK {
            // The workbook moves: its targets as absolute names, so they
            // still point where they did.
            part.1 = retarget(&part.1, |t| {
                Some(format!("/{}", moved(&resolve_part(&main, t))))
            });
        } else if let Some(source) = source_of_rels {
            // Another part's relationships: follow parts that moved.
            part.1 = retarget(&part.1, |t| {
                let at = resolve_part(&source, t);
                moves.get(&at).map(|to| format!("/{to}"))
            });
        } else if part.0 == CONTENT_TYPES {
            part.1 = rename_overrides(&part.1, |name| {
                let name = name.trim_start_matches('/');
                moves.get(name).map(|to| format!("/{to}"))
            });
        }
    }
    for part in &mut parts {
        part.0 = moved(&part.0);
    }

    let mut out = zip::ZipWriter::new(Cursor::new(Vec::with_capacity(pkg.len() + 1024)));
    let deflate = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in parts {
        out.start_file(name, deflate)?;
        out.write_all(&bytes)?;
    }
    Ok(Some(out.finish()?.into_inner()))
}

/// Rename parts whose names differ only in case from a name the package
/// refers to (content types, relationship targets, the fixed names).
fn case_as_referenced(parts: &mut [(String, Vec<u8>)]) {
    let mut wanted: HashMap<String, String> = HashMap::new();
    let mut want = |name: String| {
        wanted.entry(name.to_ascii_lowercase()).or_insert(name);
    };
    for fixed in [CONTENT_TYPES, ROOT_RELS, WORKBOOK, WORKBOOK_RELS] {
        want(fixed.into());
    }
    for (_, fixed) in FIXED {
        want(fixed.into());
    }
    for (name, bytes) in parts.iter() {
        if name.eq_ignore_ascii_case(CONTENT_TYPES) {
            for n in override_names(bytes) {
                let n = n.trim_start_matches('/').to_string();
                want(rels_part(&n));
                want(n);
            }
        } else if let Some(source) = rels_source(name) {
            for r in relationships(bytes).into_iter().filter(|r| !r.external) {
                let at = resolve_part(&source, &r.target);
                want(rels_part(&at));
                want(at);
            }
        }
    }
    let present: std::collections::HashSet<String> = parts.iter().map(|p| p.0.clone()).collect();
    for part in parts.iter_mut() {
        if let Some(name) = wanted.get(&part.0.to_ascii_lowercase())
            && *name != part.0
            && !present.contains(name)
        {
            part.0 = name.clone();
        }
    }
}

struct Relationship {
    kind: String,
    target: String,
    external: bool,
}

/// A relationships part's relationships.
fn relationships(xml: &[u8]) -> Vec<Relationship> {
    let mut out = Vec::new();
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e) | Event::Empty(e)) if e.local_name().as_ref() == b"Relationship" => {
                let attr = |key: &[u8]| {
                    e.attributes()
                        .with_checks(false)
                        .flatten()
                        .find(|a| a.key.local_name().as_ref() == key)
                        .map(|a| String::from_utf8_lossy(&a.value).into_owned())
                };
                out.push(Relationship {
                    kind: attr(b"Type").unwrap_or_default(),
                    target: attr(b"Target").unwrap_or_default().replace('\\', "/"),
                    external: attr(b"TargetMode").is_some_and(|m| m == "External"),
                });
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

/// The `PartName`s of a content types part's overrides.
fn override_names(xml: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e) | Event::Empty(e)) if e.local_name().as_ref() == b"Override" => {
                if let Some(a) = e
                    .attributes()
                    .with_checks(false)
                    .flatten()
                    .find(|a| a.key.as_ref() == b"PartName")
                {
                    out.push(String::from_utf8_lossy(&a.value).replace('\\', "/"));
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

/// `xml` with one attribute of the elements named `element` replaced where
/// `map` gives a new value (values are part names: nothing to escape).
fn rewrite_attr(
    xml: &[u8],
    element: &[u8],
    key: &[u8],
    map: impl Fn(&str) -> Option<String>,
) -> Vec<u8> {
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    loop {
        let start = reader.buffer_position() as usize;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e) | Event::Empty(e)) if e.local_name().as_ref() == element => {
                let end = reader.buffer_position() as usize;
                let tag = &xml[start..end];
                let found = e
                    .attributes()
                    .with_checks(false)
                    .flatten()
                    .find(|a| a.key.as_ref() == key)
                    .map(|a| String::from_utf8_lossy(&a.value).into_owned());
                if let Some(old) = found
                    && let Some(new) = map(&old.replace('\\', "/"))
                {
                    // The value's place in the tag: after `key=` and a quote.
                    if let Some(at) = find_attr_value(tag, key, old.as_bytes()) {
                        edits.push((start + at, start + at + old.len(), new));
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    if edits.is_empty() {
        return xml.to_vec();
    }
    let mut out = Vec::with_capacity(xml.len() + 64);
    let mut at = 0;
    for (s, e, new) in edits {
        out.extend_from_slice(&xml[at..s]);
        out.extend_from_slice(new.as_bytes());
        at = e;
    }
    out.extend_from_slice(&xml[at..]);
    out
}

/// Where `key="value"` (or with single quotes) has its value in `tag`.
fn find_attr_value(tag: &[u8], key: &[u8], value: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i + key.len() < tag.len() {
        let preceded = i > 0 && tag[i - 1].is_ascii_whitespace();
        if preceded && tag[i..].starts_with(key) {
            let mut j = i + key.len();
            while j < tag.len() && tag[j].is_ascii_whitespace() {
                j += 1;
            }
            if tag.get(j) == Some(&b'=') {
                j += 1;
                while j < tag.len() && tag[j].is_ascii_whitespace() {
                    j += 1;
                }
                if matches!(tag.get(j), Some(b'"' | b'\'')) && tag[j + 1..].starts_with(value) {
                    return Some(j + 1);
                }
            }
        }
        i += 1;
    }
    None
}

fn retarget(xml: &[u8], map: impl Fn(&str) -> Option<String>) -> Vec<u8> {
    rewrite_attr(xml, b"Relationship", b"Target", |t| {
        // External targets (URLs) are never part names.
        if t.contains("://") { None } else { map(t) }
    })
}

fn rename_overrides(xml: &[u8], map: impl Fn(&str) -> Option<String>) -> Vec<u8> {
    rewrite_attr(xml, b"Override", b"PartName", map)
}

/// `resolve_part("xl/worksheets/sheet1.xml", "../comments1.xml")` is
/// `xl/comments1.xml`; an absolute target starts at the package's root.
fn resolve_part(source: &str, target: &str) -> String {
    let target = target.replace('\\', "/");
    let joined = match target.strip_prefix('/') {
        Some(absolute) => absolute.to_string(),
        None => {
            let dir = source.rfind('/').map_or("", |i| &source[..=i]);
            format!("{dir}{target}")
        }
    };
    let mut segments: Vec<&str> = Vec::new();
    for segment in joined.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            s => segments.push(s),
        }
    }
    segments.join("/")
}

/// `xl/workbook.xml` -> `xl/_rels/workbook.xml.rels`.
fn rels_part(source: &str) -> String {
    match source.rfind('/') {
        Some(i) => format!("{}_rels/{}.rels", &source[..=i], &source[i + 1..]),
        None => format!("_rels/{source}.rels"),
    }
}

/// The part a relationships part belongs to (`""` for the package's own
/// `_rels/.rels`), or `None` for any other part.
fn rels_source(name: &str) -> Option<String> {
    let file = name.strip_suffix(".rels")?;
    let (dir, file) = match file.rfind('/') {
        Some(i) => (&file[..i], &file[i + 1..]),
        None => ("", file),
    };
    let dir = dir.strip_suffix("_rels")?;
    Some(format!("{dir}{file}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, text) in parts {
            out.start_file(*name, SimpleFileOptions::default()).unwrap();
            out.write_all(text.as_bytes()).unwrap();
        }
        out.finish().unwrap().into_inner()
    }

    fn read(pkg: &[u8]) -> HashMap<String, String> {
        let mut zip = zip::ZipArchive::new(Cursor::new(pkg)).unwrap();
        (0..zip.len())
            .map(|i| {
                let mut f = zip.by_index(i).unwrap();
                let mut s = String::new();
                f.read_to_string(&mut s).unwrap();
                (f.name().unwrap().into_owned(), s)
            })
            .collect()
    }

    const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

    fn rels(items: &[(&str, &str)]) -> String {
        let mut s = String::from(
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
        );
        for (i, (kind, target)) in items.iter().enumerate() {
            s += &format!(r#"<Relationship Id="rId{i}" Type="{REL}/{kind}" Target="{target}"/>"#);
        }
        s + "</Relationships>"
    }

    #[test]
    fn excel_layout_is_left_alone() {
        let pkg = package(&[
            (CONTENT_TYPES, "<Types/>"),
            (ROOT_RELS, &rels(&[("officeDocument", "xl/workbook.xml")])),
            (WORKBOOK, "<workbook/>"),
        ]);
        assert!(!needs_spreadsheet_layout(Cursor::new(&pkg)));
        assert!(spreadsheet_layout(&pkg).unwrap().is_none());
    }

    #[test]
    fn workbook_at_the_root_moves_to_xl() {
        let pkg = package(&[
            (
                CONTENT_TYPES,
                r#"<Types><Override PartName="/workbook.xml" ContentType="m"/><Override PartName="/styles.xml" ContentType="s"/></Types>"#,
            ),
            (ROOT_RELS, &rels(&[("officeDocument", "workbook.xml")])),
            ("workbook.xml", "<workbook/>"),
            (
                "_rels/workbook.xml.rels",
                &rels(&[("worksheet", "sheet1.xml"), ("styles", "styles.xml")]),
            ),
            ("sheet1.xml", "<worksheet/>"),
            ("styles.xml", "<styleSheet/>"),
        ]);
        assert!(needs_spreadsheet_layout(Cursor::new(&pkg)));
        let out = read(&spreadsheet_layout(&pkg).unwrap().unwrap());
        assert_eq!(out[WORKBOOK], "<workbook/>");
        assert_eq!(out["xl/styles.xml"], "<styleSheet/>");
        assert!(out.contains_key("sheet1.xml"));
        assert!(!out.contains_key("workbook.xml"));
        assert!(out[ROOT_RELS].contains(r#"Target="xl/workbook.xml""#));
        let wb = &out[WORKBOOK_RELS];
        assert!(wb.contains(r#"Target="/sheet1.xml""#), "{wb}");
        assert!(wb.contains(r#"Target="/xl/styles.xml""#), "{wb}");
        let ct = &out[CONTENT_TYPES];
        assert!(ct.contains(r#"PartName="/xl/workbook.xml""#), "{ct}");
        assert!(ct.contains(r#"PartName="/xl/styles.xml""#), "{ct}");
    }

    #[test]
    fn renamed_workbook_part_moves() {
        let pkg = package(&[
            (CONTENT_TYPES, "<Types/>"),
            (ROOT_RELS, &rels(&[("officeDocument", "xl/workbook2.xml")])),
            ("xl/workbook2.xml", "<workbook/>"),
            (
                "xl/_rels/workbook2.xml.rels",
                &rels(&[("worksheet", "sheet1.xml")]),
            ),
            ("xl/sheet1.xml", "<worksheet/>"),
        ]);
        let out = read(&spreadsheet_layout(&pkg).unwrap().unwrap());
        assert!(out.contains_key(WORKBOOK));
        assert!(out[WORKBOOK_RELS].contains(r#"Target="/xl/sheet1.xml""#));
    }

    #[test]
    fn backslashes_and_case_follow_the_references() {
        let pkg = package(&[
            ("[content_types].xml", "<Types/>"),
            (
                "_rels\\.rels",
                &rels(&[("officeDocument", "/xl/workbook.xml")]),
            ),
            ("xl\\workbook.xml", "<workbook/>"),
            (
                "xl\\_rels\\workbook.xml.rels",
                &rels(&[
                    ("worksheet", "/xl/worksheets/sheet1.xml"),
                    ("sharedStrings", "/xl/sharedStrings.xml"),
                ]),
            ),
            ("xl\\worksheets\\sheet1.xml", "<worksheet/>"),
            ("xl\\sharedstrings.xml", "<sst/>"),
        ]);
        assert!(needs_spreadsheet_layout(Cursor::new(&pkg)));
        let out = read(&spreadsheet_layout(&pkg).unwrap().unwrap());
        let mut names: Vec<_> = out.keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                CONTENT_TYPES,
                ROOT_RELS,
                WORKBOOK_RELS,
                "xl/sharedStrings.xml",
                WORKBOOK,
                "xl/worksheets/sheet1.xml"
            ]
        );
    }

    #[test]
    fn rels_names() {
        assert_eq!(rels_source("_rels/.rels").as_deref(), Some(""));
        assert_eq!(
            rels_source("xl/_rels/workbook.xml.rels").as_deref(),
            Some("xl/workbook.xml")
        );
        assert_eq!(rels_source("xl/workbook.xml"), None);
        assert_eq!(rels_part("workbook.xml"), "_rels/workbook.xml.rels");
        assert_eq!(resolve_part("", "/xl/workbook.xml"), "xl/workbook.xml");
        assert_eq!(resolve_part("xl/a/b.xml", "../c.xml"), "xl/c.xml");
    }
}
