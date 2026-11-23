//! Reading OpenDocument text (`.odt`, `.ott` templates, flat `.fodt`),
//! import only.
//!
//! The package (`content.xml`, `styles.xml`, `meta.xml`, `settings.xml`
//! and the pictures) is read with `roxmltree` and written out as a
//! WordprocessingML package ([`conv`]) that the `.docx` reader opens like
//! any other file, as [`crate::rtf`] and the `.doc` import do. So an
//! imported document is modelled exactly like a `.docx` one, and Save
//! writes a `.docx` (the source has no path, so Save goes to Save As).
//!
//! What is read:
//! * paragraph and character styles (named ones become Word styles with
//!   their parents; automatic ones become direct formatting over their
//!   parent style), the default style, fonts, `Standard` as Normal and
//!   LibreOffice's heading, body text, header, footer, footnote, list,
//!   caption and contents styles under Word's names;
//! * character formatting: bold, italic, underline types, strike, double
//!   strike, caps, small caps, hidden, size (also in percent), font
//!   (western, Asian, complex), colour, highlight or shading, super and
//!   subscript, letter spacing, scale, outline, shadow, relief, language;
//! * paragraphs: alignment, indents, spacing, line spacing, keep with next,
//!   keep together, page and column breaks, widows, tab stops (made
//!   absolute as Word counts them), borders, shading, right-to-left;
//! * headings (`text:h`) with their outline level, and chapter numbering
//!   (`text:outline-style`);
//! * lists: bullets, numbers (with prefix, suffix and the levels shown),
//!   nesting, list headers and later paragraphs of an item, restarts,
//!   `text:continue-numbering` and `text:continue-list`, the label
//!   positions of both ODF layouts;
//! * tables: column widths, horizontal and vertical spans (covered cells),
//!   header rows, row heights, cell borders, padding, background and
//!   vertical alignment, table alignment and indent; nested tables;
//! * page layout from master pages: paper size, orientation, margins,
//!   columns, page number format; headers and footers (first page, left
//!   pages, and a first page master followed by another), a master page
//!   change or `text:section` columns start a new section;
//! * pictures (PNG, JPEG, GIF, BMP, WMF, EMF; an SVG's PNG fallback) inline
//!   or floating with their position and wrap; text frames as text boxes;
//!   charts and embedded objects as their preview picture;
//! * footnotes and endnotes (custom marks too), comments (with their
//!   ranges), bookmarks, hyperlinks (web and in-document), page number,
//!   page count and other fields (as their text), tracked insertions and
//!   deletions;
//! * document properties from `meta.xml`.
//!
//! Left out (and listed in [`Import::notes`]): drawing shapes, pictures in
//! formats Kantor can't show (SVG without a fallback, TIFF), tracked
//! formatting changes, forms. Malformed files give an error, never a
//! panic.

mod conv;
mod style;

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Cursor, Read as _};

use anyhow::{Context as _, anyhow, bail};

/// A read OpenDocument text file.
pub struct Import {
    /// The document as a `.docx` package.
    pub docx: Vec<u8>,
    /// What could not be brought over, as short sentences.
    pub notes: Vec<String>,
}

/// File extensions of OpenDocument text: documents, templates, flat XML.
pub const EXTENSIONS: [&str; 3] = ["odt", "ott", "fodt"];

const MIME_TEXT: &str = "application/vnd.oasis.opendocument.text";

/// Whether `bytes` look like an OpenDocument text file: a package whose
/// `mimetype` is ODF text (or a text template), or flat ODF XML.
pub fn is_odt(bytes: &[u8]) -> bool {
    if bytes.starts_with(b"PK") {
        // The mimetype file comes first, stored, so its name and value sit
        // in the first local header.
        let head = &bytes[..bytes.len().min(160)];
        return contains(head, b"mimetype") && contains(head, MIME_TEXT.as_bytes());
    }
    let head = &bytes[..bytes.len().min(8192)];
    contains(head, b"<office:document") && contains(head, MIME_TEXT.as_bytes())
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// The files of the package Kantor reads.
pub(crate) struct Pkg {
    content: String,
    /// `styles.xml` (empty for flat XML: its styles are in `content`).
    styles: Option<String>,
    meta: Option<String>,
    settings: Option<String>,
    zip: Option<RefCell<zip::ZipArchive<Cursor<Vec<u8>>>>>,
}

/// Largest part read (pictures included): a bigger one is a broken or
/// hostile file.
const MAX_PART: u64 = 512 << 20;

impl Pkg {
    fn read(bytes: &[u8]) -> anyhow::Result<Self> {
        if !bytes.starts_with(b"PK") {
            let text = std::str::from_utf8(bytes)
                .map_err(|_| anyhow!("not an OpenDocument file"))?
                .to_owned();
            return Ok(Self {
                content: text,
                styles: None,
                meta: None,
                settings: None,
                zip: None,
            });
        }
        let mut zip =
            zip::ZipArchive::new(Cursor::new(bytes.to_vec())).context("not a zip package")?;
        let mut text = |name: &str| -> anyhow::Result<Option<String>> {
            let Ok(f) = zip.by_name(name) else {
                return Ok(None);
            };
            let mut s = String::new();
            f.take(MAX_PART)
                .read_to_string(&mut s)
                .with_context(|| format!("reading {name}"))?;
            Ok(Some(s))
        };
        let content = text("content.xml")?.context("no content.xml")?;
        let styles = text("styles.xml")?;
        let meta = text("meta.xml")?;
        let settings = text("settings.xml")?;
        Ok(Self {
            content,
            styles,
            meta,
            settings,
            zip: Some(RefCell::new(zip)),
        })
    }

    /// A file of the package (`Pictures/x.png`).
    fn file(&self, name: &str) -> Option<Vec<u8>> {
        let name = name.strip_prefix("./").unwrap_or(name);
        let mut zip = self.zip.as_ref()?.borrow_mut();
        let f = zip.by_name(name).ok()?;
        let mut out = Vec::new();
        f.take(MAX_PART).read_to_end(&mut out).ok()?;
        Some(out)
    }
}

/// Convert an OpenDocument text file to a `.docx` package.
pub fn to_docx(bytes: &[u8]) -> anyhow::Result<Import> {
    if !is_odt(bytes) {
        bail!("not an OpenDocument text file");
    }
    let pkg = Pkg::read(bytes)?;
    let opts = || roxmltree::ParsingOptions {
        allow_dtd: false,
        ..Default::default()
    };
    let content = roxmltree::Document::parse_with_options(&pkg.content, opts())
        .context("content.xml is not well-formed")?;
    let styles = match &pkg.styles {
        Some(s) => Some(
            roxmltree::Document::parse_with_options(s, opts())
                .context("styles.xml is not well-formed")?,
        ),
        None => None,
    };
    let meta = pkg
        .meta
        .as_deref()
        .and_then(|m| roxmltree::Document::parse_with_options(m, opts()).ok());
    let settings = pkg
        .settings
        .as_deref()
        .and_then(|m| roxmltree::Document::parse_with_options(m, opts()).ok());
    let st = style::Styles::read(&content, styles.as_ref(), settings.as_ref());
    conv::package(&pkg, &content, meta.as_ref(), &st)
}

// ── shared helpers ─────────────────────────────────────────────────────────

pub(crate) const NS_OFFICE: &str = "urn:oasis:names:tc:opendocument:xmlns:office:1.0";
pub(crate) const NS_STYLE: &str = "urn:oasis:names:tc:opendocument:xmlns:style:1.0";
pub(crate) const NS_TEXT: &str = "urn:oasis:names:tc:opendocument:xmlns:text:1.0";
pub(crate) const NS_TABLE: &str = "urn:oasis:names:tc:opendocument:xmlns:table:1.0";
pub(crate) const NS_DRAW: &str = "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0";
pub(crate) const NS_FO: &str = "urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0";
pub(crate) const NS_SVG: &str = "urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0";
pub(crate) const NS_XLINK: &str = "http://www.w3.org/1999/xlink";
pub(crate) const NS_DC: &str = "http://purl.org/dc/elements/1.1/";
pub(crate) const NS_META: &str = "urn:oasis:names:tc:opendocument:xmlns:meta:1.0";
pub(crate) const NS_XML: &str = "http://www.w3.org/XML/1998/namespace";
pub(crate) const NS_CONFIG: &str = "urn:oasis:names:tc:opendocument:xmlns:config:1.0";

pub(crate) type Node<'a> = roxmltree::Node<'a, 'a>;

/// Whether `n` is the element `ns:name`.
pub(crate) fn is(n: Node, ns: &str, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some(ns)
}

pub(crate) fn child<'a>(n: Node<'a>, ns: &str, name: &str) -> Option<Node<'a>> {
    n.children().find(|c| is(*c, ns, name))
}

pub(crate) fn attr<'a>(n: Node<'a>, ns: &str, name: &str) -> Option<&'a str> {
    n.attribute((ns, name))
}

/// A length (`1.85in`, `2.5cm`, `10pt`…) in points.
pub(crate) fn points(s: &str) -> Option<f64> {
    let s = s.trim();
    let split = s.find(|c: char| c.is_ascii_alphabetic())?;
    let (n, unit) = s.split_at(split);
    let n: f64 = n.trim().parse().ok()?;
    let k = match unit {
        "pt" => 1.0,
        "in" | "inch" => 72.0,
        "cm" => 72.0 / 2.54,
        "mm" => 72.0 / 25.4,
        "pc" => 12.0,
        "px" => 0.75,
        _ => return None,
    };
    let v = n * k;
    (v.is_finite() && v.abs() < 1e7).then_some(v)
}

/// A length in twentieths of a point.
pub(crate) fn twips(s: &str) -> Option<i32> {
    points(s).map(|p| (p * 20.0).round() as i32)
}

/// A length in EMUs.
pub(crate) fn emu(s: &str) -> Option<i64> {
    points(s).map(|p| (p * 12700.0).round() as i64)
}

/// A percentage (`130%`) as a fraction.
pub(crate) fn percent(s: &str) -> Option<f64> {
    let v: f64 = s.trim().strip_suffix('%')?.trim().parse().ok()?;
    v.is_finite().then_some(v / 100.0)
}

/// `#rrggbb` as `RRGGBB`.
pub(crate) fn color(s: &str) -> Option<String> {
    let hex = s.trim().strip_prefix('#')?;
    (hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| hex.to_ascii_uppercase())
}

/// Text for XML: escaped, without characters XML can't hold.
pub(crate) fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\t' | '\n' | '\r' => o.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => o.push(c),
        }
    }
    o
}

/// All text under `n` (fields' shown results, deleted text).
pub(crate) fn text_of(n: Node) -> String {
    let mut s = String::new();
    for d in n.descendants() {
        if d.is_text() {
            s.push_str(d.text().unwrap_or(""));
        } else if is(d, NS_TEXT, "s") {
            let c: usize = attr(d, NS_TEXT, "c")
                .and_then(|c| c.parse().ok())
                .unwrap_or(1);
            s.extend(std::iter::repeat_n(' ', c.min(1000)));
        } else if is(d, NS_TEXT, "tab") {
            s.push('\t');
        }
    }
    s
}

/// Image type of `bytes` by its signature: extension and whether Kantor
/// draws it.
pub(crate) fn image_kind(bytes: &[u8]) -> Option<(&'static str, bool)> {
    Some(if bytes.starts_with(b"\x89PNG") {
        ("png", true)
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        ("jpeg", true)
    } else if bytes.starts_with(b"GIF8") {
        ("gif", true)
    } else if bytes.starts_with(b"BM") {
        ("bmp", true)
    } else if bytes.len() > 44 && bytes.starts_with(&[1, 0, 0, 0]) && &bytes[40..44] == b" EMF" {
        ("emf", true)
    } else if bytes.starts_with(&[0xD7, 0xCD, 0xC6, 0x9A])
        || bytes.starts_with(&[1, 0, 9, 0])
        || bytes.starts_with(&[2, 0, 9, 0])
    {
        ("wmf", true)
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        ("tiff", false)
    } else if contains(&bytes[..bytes.len().min(2048)], b"<svg") {
        ("svg", false)
    } else {
        return None;
    })
}

/// A picture's size in pixels (PNG, GIF, JPEG, BMP).
pub(crate) fn pixel_size(b: &[u8]) -> Option<(u32, u32)> {
    let be32 =
        |i: usize| -> Option<u32> { Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?)) };
    if b.starts_with(b"\x89PNG") {
        return Some((be32(16)?, be32(20)?));
    }
    if b.starts_with(b"GIF8") {
        let w = u16::from_le_bytes(b.get(6..8)?.try_into().ok()?);
        let h = u16::from_le_bytes(b.get(8..10)?.try_into().ok()?);
        return Some((w.into(), h.into()));
    }
    if b.starts_with(b"BM") {
        let w = i32::from_le_bytes(b.get(18..22)?.try_into().ok()?);
        let h = i32::from_le_bytes(b.get(22..26)?.try_into().ok()?);
        return Some((w.unsigned_abs(), h.unsigned_abs()));
    }
    if b.starts_with(&[0xFF, 0xD8]) {
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] != 0xFF {
                i += 1;
                continue;
            }
            let m = b[i + 1];
            let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
            if matches!(m, 0xC0..=0xCF) && !matches!(m, 0xC4 | 0xC8 | 0xCC) {
                let h = u16::from_be_bytes([b[i + 5], b[i + 6]]);
                let w = u16::from_be_bytes([b[i + 7], b[i + 8]]);
                return Some((w.into(), h.into()));
            }
            i += 2 + len.max(2);
        }
    }
    None
}

/// Decoded `office:binary-data` (base64, whitespace ignored).
pub(crate) fn base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            c if c.is_ascii_whitespace() => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Settings from `settings.xml` that change layout.
#[derive(Default)]
pub(crate) struct Settings {
    /// Tab stops count from the paragraph's indent (LibreOffice's default)
    /// rather than the page margin (Word's).
    pub tabs_relative: bool,
}

pub(crate) fn read_settings(doc: Option<&roxmltree::Document>) -> Settings {
    let mut s = Settings {
        tabs_relative: true,
    };
    let Some(doc) = doc else { return s };
    let items: HashMap<&str, &str> = doc
        .descendants()
        .filter(|n| n.tag_name().name() == "config-item")
        .filter_map(|n| Some((n.attribute((NS_CONFIG, "name"))?, n.text().unwrap_or(""))))
        .collect();
    if items.get("TabsRelativeToIndent") == Some(&"false") {
        s.tabs_relative = false;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn lengths_colours_base64() {
        assert_eq!(twips("1in"), Some(1440));
        assert_eq!(twips("2.54cm"), Some(1440));
        assert_eq!(twips("12pt"), Some(240));
        assert_eq!(twips("-0.25in"), Some(-360));
        assert_eq!(twips("abc"), None);
        assert_eq!(twips("1e400in"), None);
        assert_eq!(color("#c00000"), Some("C00000".into()));
        assert_eq!(color("transparent"), None);
        assert_eq!(percent("130%"), Some(1.3));
        assert_eq!(base64("aGVs\nbG8="), Some(b"hello".to_vec()));
        assert!(!is_odt(b"PK\x03\x04garbage"));
        assert!(!is_odt(b"{\\rtf1}"));
    }

    #[std::prelude::v1::test]
    fn picture_sizes() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend(7u32.to_be_bytes());
        png.extend(5u32.to_be_bytes());
        assert_eq!(pixel_size(&png), Some((7, 5)));
        assert_eq!(image_kind(&png), Some(("png", true)));
        assert_eq!(pixel_size(&[0xFF, 0xD8, 0xFF]), None);
    }
}
