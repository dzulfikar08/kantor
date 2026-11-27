//! Changing citations: the document's citation style (kept the way Zotero
//! and Mendeley keep it), Refresh Citations, and the package parts this
//! touches (`docProps/custom.xml`, the document's own source list).
//!
//! Refresh renders every CSL citation (Zotero, Mendeley Desktop, Mendeley
//! Cite) in document order through [`kantor_cite::render`] and writes:
//!
//! - a citation field's new result, and in its code the new
//!   `formattedCitation` / `plainCitation` (Zotero) or `mendeley`
//!   formatted citation, so the manager's plug-in doesn't see a citation
//!   edited by hand;
//! - a Mendeley Cite control's new text and `manualOverride.citeprocText`;
//! - every bibliography's entries, one paragraph each, in the style of the
//!   bibliography's paragraphs.
//!
//! Citations marked as edited by hand (Zotero's `dontUpdate`, Mendeley
//! Cite's manual override) keep their text; Word's and EndNote's are left
//! alone.

use std::sync::Arc;

use base64::Engine as _;
use kantor_cite::{CiteIn, Fmt, Piece};
use serde_json::{Value, json};

use crate::citations::{Bibliography, Citation, CiteAt, CiteSource, MENDELEY_CITE_TAG};
use crate::document::{Document, Kind};
use crate::fields::{FieldSpan, complex_begin, replace_mark, spans};
use crate::model::{Object, Paragraph, Position, Run, RunProps, Selection};
use crate::object::{FieldMark, Level, Opaque};
use crate::xml;

const CUSTOM_PART: &str = "docProps/custom.xml";
const CUSTOM_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/custom-properties";
const VT_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes";
/// The `fmtid` Word gives user-defined properties.
const FMTID: &str = "{D5CDD505-2E9C-101B-9397-08002B2CF9AE}";
const ZOTERO_PREF: &str = "ZOTERO_PREF_";
const MENDELEY_STYLE: &str = "Mendeley Citation Style_1";

/// Package parts citations changed, written on save.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CiteParts {
    /// `docProps/custom.xml` as it is now.
    pub custom: Option<String>,
    /// The document's own source list (CSL-JSON items).
    pub sources: Option<Vec<Value>>,
}

/// Where the document's citation style is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefsFrom {
    /// Zotero's document preferences (`ZOTERO_PREF_n`).
    Zotero,
    /// Mendeley Desktop's `Mendeley Citation Style_1`.
    Mendeley,
    /// Nothing in the file: Kantor's default.
    Default,
}

/// The document's citation style and locale.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CitePrefs {
    /// CSL style id (`http://www.zotero.org/styles/apa`).
    pub style: String,
    /// CSL locale (`id-ID`); `None`: the style's.
    pub locale: Option<String>,
    pub from: PrefsFrom,
}

/// What Refresh Citations did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefreshReport {
    /// Citations whose text changed.
    pub changed: usize,
    /// CSL citations rendered.
    pub citations: usize,
    /// Bibliographies rebuilt.
    pub bibliographies: usize,
    /// Kept as edited by hand.
    pub kept: usize,
    /// Word and EndNote citations, left alone.
    pub other: usize,
}

// ── custom.xml ─────────────────────────────────────────────────────────────

/// `custom.xml` with the properties `drop` matches removed and `add` added
/// (text values), or a new part.
fn edit_custom(src: Option<&str>, drop: &dyn Fn(&str) -> bool, add: &[(String, String)]) -> String {
    let src = src.unwrap_or_default();
    let mut out = String::new();
    let mut max_pid = 1u32;
    let body_end;
    if let Some(close) = src.rfind("</Properties>") {
        let mut at = 0;
        let mut kept = String::new();
        while let Some(i) = src[at..close].find("<property ") {
            let start = at + i;
            kept.push_str(&src[at..start]);
            let end = src[start..close]
                .find("</property>")
                .map_or(close, |e| start + e + "</property>".len());
            let el = &src[start..end];
            let head = &el[..el.find('>').map_or(el.len(), |e| e + 1)];
            let tag = if head.ends_with("/>") {
                xml::parse(head)
            } else {
                xml::parse(&format!("{head}</property>"))
            };
            let name = tag
                .as_ref()
                .and_then(|t| t.attr("name"))
                .unwrap_or_default()
                .to_owned();
            if let Some(pid) = tag
                .as_ref()
                .and_then(|t| t.attr("pid"))
                .and_then(|p| p.parse::<u32>().ok())
            {
                max_pid = max_pid.max(pid);
            }
            if !drop(&name) {
                kept.push_str(el);
            }
            at = end;
        }
        kept.push_str(&src[at..close]);
        out.push_str(&kept);
        body_end = &src[close..];
    } else {
        out.push_str(&format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<Properties xmlns=\"{CUSTOM_NS}\" xmlns:vt=\"{VT_NS}\">"
        ));
        body_end = "</Properties>";
    }
    for (k, v) in add {
        max_pid += 1;
        out.push_str(&format!(
            "<property fmtid=\"{FMTID}\" pid=\"{max_pid}\" name=\"{}\"><vt:lpwstr>{}</vt:lpwstr></property>",
            xml::escape(k),
            xml::escape(v)
        ));
    }
    out.push_str(body_end);
    out
}

/// Zotero's document preferences from the custom properties.
fn zotero_pref_xml(props: &[(String, String)]) -> Option<String> {
    let mut chunks: Vec<(u32, &str)> = props
        .iter()
        .filter_map(|(k, v)| Some((k.strip_prefix(ZOTERO_PREF)?.parse().ok()?, v.as_str())))
        .collect();
    if chunks.is_empty() {
        return None;
    }
    chunks.sort_by_key(|c| c.0);
    Some(chunks.into_iter().map(|c| c.1).collect())
}

/// The style id and locale in Zotero's `<data>` preferences.
fn zotero_style(pref: &str) -> Option<(String, Option<String>)> {
    let doc = roxmltree::Document::parse(pref).ok()?;
    let style = doc.descendants().find(|n| n.tag_name().name() == "style")?;
    Some((
        style.attribute("id")?.to_owned(),
        style
            .attribute("locale")
            .filter(|l| !l.is_empty())
            .map(str::to_owned),
    ))
}

/// Zotero's preferences with a new style and locale (a fresh set when
/// there were none).
fn zotero_pref_with(
    pref: Option<&str>,
    style: &str,
    locale: Option<&str>,
    seed: Option<u64>,
) -> String {
    let attrs = |l: Option<&str>| {
        format!(
            "id=\"{}\"{}",
            xml::escape(style),
            l.map(|l| format!(" locale=\"{}\"", xml::escape(l)))
                .unwrap_or_default()
        )
    };
    if let Some(p) = pref
        && let Some(i) = p.find("<style ")
        && let Some(e) = p[i..].find("/>").map(|e| i + e)
    {
        // Keep the element's other attributes (hasBibliography…).
        let head = &p[i + "<style ".len()..e];
        let mut rest = String::new();
        if let Ok(d) = roxmltree::Document::parse(&format!("<style {head}/>")) {
            for a in d.root_element().attributes() {
                if a.name() != "id" && a.name() != "locale" {
                    rest.push_str(&format!(" {}=\"{}\"", a.name(), xml::escape(a.value())));
                }
            }
        }
        return format!("{}<style {}{rest}/>{}", &p[..i], attrs(locale), &p[e + 2..]);
    }
    format!(
        "<data data-version=\"3\" zotero-version=\"7.0\"><session id=\"{}\"/><style {} hasBibliography=\"1\" bibliographyStyleHasBeenSet=\"1\"/><prefs><pref name=\"fieldType\" value=\"Field\"/></prefs></data>",
        seed.map_or_else(session_id, |s| random_id(s, 8)),
        attrs(locale)
    )
}

/// Eight letters and digits, as Zotero's session ids.
fn session_id() -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    random_id(n as u64 ^ u64::from(std::process::id()), 8)
}

/// `len` letters and digits made from `seed` (the same seed gives the same
/// id).
pub fn random_id(mut seed: u64, len: usize) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    if seed == 0 {
        seed = 0x9E37_79B9_7F4A_7C15;
    }
    (0..len)
        .map(|_| {
            // xorshift
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            ALPHABET[(seed % ALPHABET.len() as u64) as usize] as char
        })
        .collect()
}

/// Chunks of at most 255 characters (Word's limit for a text property).
fn chunks(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    chars.chunks(255).map(|c| c.iter().collect()).collect()
}

// ── rendering into the document ────────────────────────────────────────────

/// A piece's run formatting on top of `base`.
fn piece_props(base: &RunProps, f: Fmt) -> RunProps {
    let mut p = base.clone();
    if f.italic {
        p.italic = Some(!base.italic.unwrap_or(false));
    }
    if f.bold {
        p.bold = Some(true);
    }
    if f.underline {
        p.underline = Some(true);
    }
    if f.superscript || f.subscript {
        p.extras.remove("vertAlign");
        p.extras.push(if f.superscript {
            "<w:vertAlign w:val=\"superscript\"/>"
        } else {
            "<w:vertAlign w:val=\"subscript\"/>"
        });
    }
    if f.small_caps {
        p.extras.remove("smallCaps");
        p.extras.push("<w:smallCaps/>");
    }
    p
}

/// Zotero's `formattedCitation`: RTF-style markup for italics and bold,
/// non-ASCII characters as `\uc0\uN{}`.
fn rtf(pieces: &[Piece]) -> String {
    let mut out = String::new();
    for p in pieces {
        let mut text = String::new();
        for c in p.text.chars() {
            match c {
                '\\' | '{' | '}' => {
                    text.push('\\');
                    text.push(c);
                }
                c if (c as u32) < 128 => text.push(c),
                c => {
                    let mut buf = [0u16; 2];
                    for u in c.encode_utf16(&mut buf) {
                        text.push_str(&format!("\\uc0\\u{}{{}}", *u as i16));
                    }
                }
            }
        }
        let wrap = |t: String, cmd: &str| format!("{{\\{cmd}{{}}{t}}}");
        let mut t = text;
        if p.fmt.italic {
            t = wrap(t, "i");
        }
        if p.fmt.bold {
            t = wrap(t, "b");
        }
        if p.fmt.superscript {
            t = format!("\\super {t}\\nosupersub{{}}");
        }
        if p.fmt.subscript {
            t = format!("\\sub {t}\\nosupersub{{}}");
        }
        if p.fmt.small_caps {
            t = wrap(t, "scaps");
        }
        out.push_str(&t);
    }
    out
}

/// Mendeley's formatted citations: HTML-style markup.
fn html(pieces: &[Piece]) -> String {
    let mut out = String::new();
    for p in pieces {
        let mut t = p
            .text
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        if p.fmt.italic {
            t = format!("<i>{t}</i>");
        }
        if p.fmt.bold {
            t = format!("<b>{t}</b>");
        }
        if p.fmt.superscript {
            t = format!("<sup>{t}</sup>");
        }
        if p.fmt.subscript {
            t = format!("<sub>{t}</sub>");
        }
        out.push_str(&t);
    }
    out
}

/// `instr` with its JSON swapped for `json` (keywords around it kept).
fn instr_with_json(instr: &str, json: &Value) -> String {
    let Some(at) = instr.find('{') else {
        return instr.to_owned();
    };
    let mut stream = serde_json::Deserializer::from_str(&instr[at..]).into_iter::<Value>();
    let end = match stream.next() {
        Some(Ok(_)) => at + stream.byte_offset(),
        _ => return instr.to_owned(),
    };
    format!("{}{}{}", &instr[..at], json, &instr[end..])
}

/// A field start with a new instruction: the code's instruction runs keep
/// their formatting (the whole instruction in the first, the others
/// emptied); a code with nested fields is written anew.
fn mark_with_instr(mark: &FieldMark, instr: &str) -> FieldMark {
    let FieldMark::Begin {
        code, sep, simple, ..
    } = mark
    else {
        return complex_begin(instr, true);
    };
    if *simple || code.matches("fldCharType=\"begin\"").count() != 1 {
        return complex_begin(instr, true);
    }
    let mut out = String::with_capacity(code.len() + instr.len());
    let mut at = 0;
    let mut first = true;
    while let Some(i) = code[at..].find("<w:instrText") {
        let start = at + i;
        let Some(gt) = code[start..].find('>').map(|g| start + g + 1) else {
            break;
        };
        let self_closing = code[..gt].ends_with("/>");
        let close = if self_closing {
            gt
        } else {
            match code[gt..].find("</w:instrText>") {
                Some(c) => gt + c,
                None => break,
            }
        };
        out.push_str(&code[at..start]);
        let text = if first {
            xml::escape(instr)
        } else {
            String::new()
        };
        out.push_str("<w:instrText xml:space=\"preserve\">");
        out.push_str(&text);
        out.push_str("</w:instrText>");
        first = false;
        at = if self_closing {
            gt
        } else {
            close + "</w:instrText>".len()
        };
    }
    if first {
        return complex_begin(instr, true);
    }
    out.push_str(&code[at..]);
    FieldMark::Begin {
        instr: instr.to_owned(),
        code: out.into(),
        sep: sep
            .clone()
            .or_else(|| Some(crate::object::FIELD_SEPARATE.into())),
        simple: false,
    }
}

/// The formatting of a field's old result (its first text run), else of
/// its start.
fn result_props(paras: &[Paragraph], a: Position, b: Position) -> RunProps {
    let last = b.para.min(paras.len() - 1);
    for (k, p) in paras.iter().enumerate().take(last + 1).skip(a.para) {
        let from = if k == a.para { a.offset } else { 0 };
        let to = if k == b.para { b.offset } else { p.text.len() };
        if let Some((_, run)) = p.run_ranges().find(|(r, run)| {
            r.start >= from && r.end <= to && !r.is_empty() && run.object.is_none()
        }) {
            return run.props.clone();
        }
    }
    paras[a.para].props_at(a.offset)
}

/// A paragraph of rendered pieces.
fn paragraph_of(pieces: &[Piece], base: &RunProps, like: &Paragraph) -> Paragraph {
    let mut p = Paragraph::new("");
    p.props = like.props.clone();
    p.props.extras.remove("sectPr");
    p.cell = like.cell;
    p.runs[0].props = base.clone();
    for piece in pieces {
        let at = p.text.len();
        p.insert(at, &piece.text, piece_props(base, piece.fmt));
    }
    p
}

/// Replace a field's result (`paras[a..b]`) by `entries`, one paragraph
/// each (a citation: one entry). Bookmarks in the old result stay at its
/// start.
fn set_field_entries(paras: &mut Vec<Paragraph>, span: &FieldSpan, entries: &[Vec<Piece>]) {
    let (a, b) = span.result();
    let base = result_props(paras, a, b);
    let mut left = paras[a.para].clone();
    let mut markers = Vec::new();
    let right = if a.para == b.para {
        markers.extend(left.markers_in(a.offset..b.offset));
        let mut right = left.clone();
        let right_part = right.split_off(b.offset);
        let _ = left.split_off(a.offset);
        right_part
    } else {
        markers.extend(left.markers_in(a.offset..left.text.len()));
        for p in &paras[a.para + 1..b.para] {
            markers.extend(p.markers_in(0..p.text.len()));
        }
        let mut last = paras[b.para].clone();
        markers.extend(last.markers_in(0..b.offset));
        let right = last.split_off(b.offset);
        let _ = left.split_off(a.offset);
        right
    };
    // A paragraph mark's own section break goes with the right part.
    let at = left.text.len();
    left.insert_runs(at, markers);
    let mut out: Vec<Paragraph> = Vec::new();
    let empty: Vec<Piece> = Vec::new();
    let entries: Vec<&Vec<Piece>> = if entries.is_empty() {
        vec![&empty]
    } else {
        entries.iter().collect()
    };
    let n = entries.len();
    for (i, e) in entries.iter().enumerate() {
        let mut p = if i == 0 {
            let mut p = left.clone();
            for piece in e.iter() {
                let at = p.text.len();
                p.insert(at, &piece.text, piece_props(&base, piece.fmt));
            }
            p
        } else if i + 1 == n {
            paragraph_of(e, &base, &right)
        } else {
            paragraph_of(e, &base, &left)
        };
        if i + 1 == n {
            p.append(right.clone());
            if i > 0 {
                p.props = right.props.clone();
            }
        }
        out.push(p);
    }
    paras.splice(a.para..=b.para, out);
}

/// A content control's XML with new text (and tag), the text's runs in the
/// formatting of the control's first run.
fn control_with(xml: &str, tag: Option<&str>, pieces: &[Piece]) -> Option<String> {
    let open = xml.find("<w:sdtContent")?;
    let content_start = open + xml[open..].find('>')? + 1;
    let content_end = xml.rfind("</w:sdtContent>")?;
    let mut head = xml[..content_start].to_owned();
    if let Some(tag) = tag
        && let Some(t) = head.find("<w:tag ")
        && let Some(v) = head[t..].find("w:val=\"").map(|v| t + v + "w:val=\"".len())
        && let Some(q) = head[v..].find('"').map(|q| v + q)
    {
        head.replace_range(v..q, &xml::escape(tag));
    }
    // The first run's properties.
    let content = &xml[content_start..content_end];
    let rpr_inner = content
        .find("<w:rPr>")
        .and_then(|i| {
            let s = i + "<w:rPr>".len();
            content[s..].find("</w:rPr>").map(|e| &content[s..s + e])
        })
        .filter(|_| content.find("<w:rPr>") < content.find("</w:r>"))
        .unwrap_or_default();
    let base = children(rpr_inner);
    let mut runs = String::new();
    for p in pieces {
        let mut els: Vec<String> = base
            .iter()
            .filter(|e| {
                let n = crate::model::element_name(e);
                !(p.fmt.italic && n == "i"
                    || p.fmt.bold && n == "b"
                    || (p.fmt.superscript || p.fmt.subscript) && n == "vertAlign")
            })
            .cloned()
            .collect();
        if p.fmt.italic {
            els.push("<w:i/>".into());
        }
        if p.fmt.bold {
            els.push("<w:b/>".into());
        }
        if p.fmt.superscript {
            els.push("<w:vertAlign w:val=\"superscript\"/>".into());
        }
        if p.fmt.subscript {
            els.push("<w:vertAlign w:val=\"subscript\"/>".into());
        }
        let rpr = xml::ordered(xml::RPR_ORDER, els);
        runs.push_str("<w:r>");
        if !rpr.is_empty() {
            runs.push_str(&format!("<w:rPr>{rpr}</w:rPr>"));
        }
        runs.push_str(&format!(
            "<w:t xml:space=\"preserve\">{}</w:t></w:r>",
            xml::escape(&p.text)
        ));
    }
    Some(format!("{head}{runs}{}", &xml[content_end..]))
}

/// The child elements of an XML fragment, as written.
fn children(inner: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut r = quick_xml::Reader::from_str(inner);
    let mut depth = 0usize;
    let mut start = 0usize;
    loop {
        let before = r.buffer_position() as usize;
        let Ok(ev) = r.read_event() else { break };
        let after = r.buffer_position() as usize;
        match ev {
            quick_xml::events::Event::Start(_) => {
                if depth == 0 {
                    start = before;
                }
                depth += 1;
            }
            quick_xml::events::Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    out.push(inner[start..after].to_owned());
                }
            }
            quick_xml::events::Event::Empty(_) if depth == 0 => {
                out.push(inner[before..after].to_owned());
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    out
}

/// The citation's items to render.
fn inputs(c: &Citation) -> Vec<CiteIn<'_>> {
    c.items
        .iter()
        .map(|it| CiteIn {
            data: &it.data,
            uri: it.uris.first().map(String::as_str),
            locator: it.locator.as_deref(),
            label: it.label.as_deref(),
            prefix: it.prefix.as_deref(),
            suffix: it.suffix.as_deref(),
            suppress_author: it.suppress_author,
            author_only: it.author_only,
        })
        .collect()
}

/// A change to make, at a position (applied last first).
enum Change {
    Field {
        at: Position,
        mark_id: usize,
        entries: Vec<Vec<Piece>>,
        instr: Option<String>,
    },
    Control {
        at: Position,
        xml: String,
    },
    Paragraphs {
        first: usize,
        last: usize,
        entries: Vec<Vec<Piece>>,
    },
}

impl Change {
    fn at(&self) -> Position {
        match self {
            Self::Field { at, .. } | Self::Control { at, .. } => *at,
            Self::Paragraphs { first, .. } => Position::new(*first, 0),
        }
    }
}

/// A citation JSON with its new formatted text recorded the manager's way.
fn json_with_text(c: &Citation, pieces: &[Piece]) -> Option<Value> {
    let mut j = c.json.clone()?;
    let plain = kantor_cite::plain(pieces);
    match c.source {
        CiteSource::Zotero => {
            let props = j
                .as_object_mut()?
                .entry("properties")
                .or_insert_with(|| json!({}));
            if let Some(o) = props.as_object_mut() {
                o.insert("formattedCitation".into(), Value::String(rtf(pieces)));
                o.insert("plainCitation".into(), Value::String(plain));
            }
        }
        CiteSource::Mendeley => {
            let m = j
                .as_object_mut()?
                .entry("mendeley")
                .or_insert_with(|| json!({}));
            if let Some(o) = m.as_object_mut() {
                let h = html(pieces);
                o.insert("formattedCitation".into(), Value::String(h.clone()));
                o.insert("plainTextFormattedCitation".into(), Value::String(plain));
                o.insert("previouslyFormattedCitation".into(), Value::String(h));
            }
        }
        CiteSource::MendeleyCite => {
            let m = j.as_object_mut()?.entry("manualOverride").or_insert_with(
                || json!({"isManuallyOverridden": false, "manualOverrideText": ""}),
            );
            if let Some(o) = m.as_object_mut() {
                o.insert("citeprocText".into(), Value::String(html(pieces)));
            }
        }
        _ => return None,
    }
    Some(j)
}

impl Document {
    /// `docProps/custom.xml` as the document has it now.
    pub(crate) fn custom_xml(&self) -> Option<String> {
        if let Some(c) = self.cite_parts.as_ref().and_then(|p| p.custom.clone()) {
            return Some(c);
        }
        self.package.as_ref()?.part_text(CUSTOM_PART)
    }

    fn custom_props_now(&self) -> Vec<(String, String)> {
        self.custom_xml()
            .map(|x| crate::fields::custom_props(&x))
            .unwrap_or_default()
    }

    /// The document's citation style and locale: Zotero's preferences,
    /// else Mendeley Desktop's style, else APA 7th (`locale_default`, e.g.
    /// the interface language's `id-ID`, when the file says nothing).
    pub fn citation_prefs(&self, locale_default: Option<&str>) -> CitePrefs {
        let props = self.custom_props_now();
        if let Some((style, locale)) = zotero_pref_xml(&props).and_then(|p| zotero_style(&p)) {
            return CitePrefs {
                style,
                locale,
                from: PrefsFrom::Zotero,
            };
        }
        if let Some((_, v)) = props
            .iter()
            .find(|(k, v)| k == MENDELEY_STYLE && !v.trim().is_empty())
        {
            return CitePrefs {
                style: v.trim().to_owned(),
                locale: locale_default.map(str::to_owned),
                from: PrefsFrom::Mendeley,
            };
        }
        CitePrefs {
            style: kantor_cite::DEFAULT_STYLE.to_owned(),
            locale: locale_default.map(str::to_owned),
            from: PrefsFrom::Default,
        }
    }

    /// Set the citation style and locale, kept as Zotero keeps them (and
    /// as Mendeley Desktop does, in a document that has its citations).
    /// Not an undo step: like a document property. Refresh to apply it.
    pub fn set_citation_prefs(&mut self, style: &str, locale: Option<&str>) {
        self.set_citation_prefs_seeded(style, locale, None);
    }

    /// [`Self::set_citation_prefs`] with Zotero's session id (for new
    /// preferences) made from `seed`, so the same input gives the same
    /// file (`kantor docs build`); `None`: from the clock.
    pub fn set_citation_prefs_seeded(
        &mut self,
        style: &str,
        locale: Option<&str>,
        seed: Option<u64>,
    ) {
        let props = self.custom_props_now();
        let cites = self.citations();
        let mendeley_only = cites.iter().any(|c| c.source == CiteSource::Mendeley)
            && !cites.iter().any(|c| c.source == CiteSource::Zotero)
            && zotero_pref_xml(&props).is_none();
        let src = self.custom_xml();
        let xml = if mendeley_only {
            edit_custom(
                src.as_deref(),
                &|n| n == MENDELEY_STYLE,
                &[(MENDELEY_STYLE.to_owned(), style.to_owned())],
            )
        } else {
            let pref = zotero_pref_with(zotero_pref_xml(&props).as_deref(), style, locale, seed);
            let add: Vec<(String, String)> = chunks(&pref)
                .into_iter()
                .enumerate()
                .map(|(i, c)| (format!("{ZOTERO_PREF}{}", i + 1), c))
                .collect();
            edit_custom(src.as_deref(), &|n| n.starts_with(ZOTERO_PREF), &add)
        };
        if self.custom_xml().as_deref() == Some(xml.as_str()) {
            return;
        }
        let mut parts = self.cite_parts.as_deref().cloned().unwrap_or_default();
        parts.custom = Some(xml);
        self.cite_parts = Some(Arc::new(parts));
        self.mark_modified();
    }

    /// Re-render every Zotero and Mendeley citation and bibliography in the
    /// document's style (see the module docs), as one undo step.
    pub fn refresh_citations(
        &mut self,
        sel: Selection,
        locale_default: Option<&str>,
    ) -> anyhow::Result<(Selection, RefreshReport)> {
        self.refresh_citations_in(sel, locale_default, None)
    }

    /// [`Self::refresh_citations`] in `style` (a `.csl` file loaded by the
    /// caller) instead of the one the document names.
    pub fn refresh_citations_in(
        &mut self,
        sel: Selection,
        locale_default: Option<&str>,
        style: Option<&kantor_cite::CslStyle>,
    ) -> anyhow::Result<(Selection, RefreshReport)> {
        let (paras, report) = self.refreshed(self.paragraphs.clone(), locale_default, style)?;
        if paras == self.paragraphs {
            return Ok((sel, report));
        }
        let after = Selection::caret(sel.head);
        Ok((self.apply_paragraphs(paras, sel, after), report))
    }

    /// `paras` with their CSL citations and bibliographies rendered again.
    fn refreshed(
        &self,
        paras: Vec<Paragraph>,
        locale_default: Option<&str>,
        style: Option<&kantor_cite::CslStyle>,
    ) -> anyhow::Result<(Vec<Paragraph>, RefreshReport)> {
        let snap = self.snapshot(paras);
        let prefs = self.citation_prefs(locale_default);
        let loaded;
        let style = match style {
            Some(s) => s,
            None => {
                loaded = kantor_cite::load_style(&prefs.style)?;
                &loaded
            }
        };
        let cites = snap.citations();
        let bibs = snap.bibliographies();
        let mut report = RefreshReport {
            other: cites.iter().filter(|c| !c.source.is_csl()).count(),
            ..RefreshReport::default()
        };
        let csl: Vec<&Citation> = cites.iter().filter(|c| c.source.is_csl()).collect();
        let csl_bibs: Vec<&Bibliography> = bibs.iter().filter(|b| b.source.is_csl()).collect();
        if csl.is_empty() && csl_bibs.is_empty() {
            return Ok((snap.paragraphs, report));
        }
        let ins: Vec<Vec<CiteIn>> = csl.iter().map(|c| inputs(c)).collect();
        let out = kantor_cite::render(style, prefs.locale.as_deref(), &ins);
        report.citations = csl.len();
        let mut changes: Vec<Change> = Vec::new();
        for (c, pieces) in csl.iter().zip(&out.citations) {
            if c.edited {
                report.kept += 1;
                continue;
            }
            let text = kantor_cite::plain(pieces);
            let json = json_with_text(c, pieces);
            if text == c.text && json == c.json {
                continue;
            }
            if text != c.text {
                report.changed += 1;
            }
            match &c.at {
                CiteAt::Field { begin, mark_id, .. } => {
                    let instr = json.as_ref().map(|j| {
                        let old = spans(&snap.paragraphs)
                            .into_iter()
                            .find(|s| s.mark_id() == *mark_id)
                            .map(|s| s.instr)
                            .unwrap_or_default();
                        instr_with_json(&old, j)
                    });
                    changes.push(Change::Field {
                        at: *begin,
                        mark_id: *mark_id,
                        entries: vec![pieces.clone()],
                        instr,
                    });
                }
                CiteAt::Control { at } => {
                    let Some(Object::Opaque(op)) = snap.paragraphs[at.para].object_at(at.offset)
                    else {
                        continue;
                    };
                    let tag = json.as_ref().map(|j| {
                        format!(
                            "{MENDELEY_CITE_TAG}{}",
                            base64::engine::general_purpose::STANDARD.encode(j.to_string())
                        )
                    });
                    if let Some(x) = control_with(&op.xml, tag.as_deref(), pieces) {
                        changes.push(Change::Control { at: *at, xml: x });
                    }
                }
                CiteAt::Paragraphs { .. } => {}
            }
        }
        for b in &csl_bibs {
            report.bibliographies += 1;
            let entries = out.bibliography.clone();
            match &b.at {
                CiteAt::Field { begin, mark_id, .. } => changes.push(Change::Field {
                    at: *begin,
                    mark_id: *mark_id,
                    entries,
                    instr: None,
                }),
                CiteAt::Paragraphs { first, last } => changes.push(Change::Paragraphs {
                    first: *first,
                    last: *last,
                    entries,
                }),
                CiteAt::Control { .. } => {}
            }
        }
        changes.sort_by_key(|c| std::cmp::Reverse(c.at()));
        let mut paras = snap.paragraphs.clone();
        for ch in changes {
            match ch {
                Change::Field {
                    mark_id,
                    entries,
                    instr,
                    ..
                } => {
                    let Some(span) = spans(&paras).into_iter().find(|s| s.mark_id() == mark_id)
                    else {
                        continue;
                    };
                    if let Some(instr) = instr {
                        let mark = mark_with_instr(&span.mark, &instr);
                        replace_mark(&mut paras, span.begin, Object::Field(Arc::new(mark)));
                    }
                    // The mark was swapped: find the field again.
                    let Some(span) = spans(&paras).into_iter().find(|s| s.begin == span.begin)
                    else {
                        continue;
                    };
                    set_field_entries(&mut paras, &span, &entries);
                }
                Change::Control { at, xml } => {
                    let Some(Object::Opaque(op)) = paras[at.para].object_at(at.offset) else {
                        continue;
                    };
                    let o = Opaque {
                        level: op.level,
                        xml,
                        label: op.label.clone(),
                    };
                    replace_mark(&mut paras, at, Object::Opaque(Arc::new(o)));
                }
                Change::Paragraphs {
                    first,
                    last,
                    entries,
                } => set_paragraph_entries(&mut paras, first, last, &entries),
            }
        }
        Ok((paras, report))
    }

    /// The kind of citation fields Kantor writes here: Mendeley Desktop's
    /// in a document whose citations are all Mendeley Desktop's, else
    /// Zotero's.
    pub fn new_citation_source(&self) -> CiteSource {
        let cites = self.citations();
        let bibs = self.bibliographies();
        let mendeley = cites.iter().any(|c| c.source == CiteSource::Mendeley)
            || bibs.iter().any(|b| b.source == CiteSource::Mendeley);
        let zotero = cites.iter().any(|c| c.source == CiteSource::Zotero)
            || bibs.iter().any(|b| b.source == CiteSource::Zotero);
        if mendeley && !zotero {
            CiteSource::Mendeley
        } else {
            CiteSource::Zotero
        }
    }

    /// Insert a citation of `items` at the selection (its text replaced),
    /// as a Zotero field (Mendeley Desktop's in a Mendeley document; see
    /// [`Self::new_citation_source`]), and render every citation and
    /// bibliography again (numbers can shift): one undo step. Returns the
    /// caret after the citation.
    pub fn insert_citation(
        &mut self,
        sel: Selection,
        items: &[crate::citations::CiteItem],
        locale_default: Option<&str>,
    ) -> anyhow::Result<Selection> {
        anyhow::ensure!(!items.is_empty(), "no items to cite");
        let source = self.new_citation_source();
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(7)
            ^ u64::from(std::process::id())
            ^ ((self.citations().len() as u64) << 32);
        let mut cite_items = crate::citations::csl_citation_items(items);
        let json = match source {
            CiteSource::Mendeley => {
                for (i, it) in cite_items.iter_mut().enumerate() {
                    let id = format!("ITEM-{}", i + 1);
                    if let Some(o) = it.as_object_mut() {
                        o.insert("id".into(), Value::String(id.clone()));
                        if let Some(d) = o.get_mut("itemData").and_then(Value::as_object_mut) {
                            d.insert("id".into(), Value::String(id));
                        }
                    }
                }
                json!({
                    "citationItems": cite_items,
                    "mendeley": {"formattedCitation": "", "plainTextFormattedCitation": "", "previouslyFormattedCitation": ""},
                    "properties": {"noteIndex": 0},
                    "schema": crate::citations::CSL_CITATION_SCHEMA,
                })
            }
            _ => json!({
                "citationID": random_id(seed, 8),
                "properties": {"formattedCitation": "", "plainCitation": "", "noteIndex": 0},
                "citationItems": cite_items,
                "schema": crate::citations::CSL_CITATION_SCHEMA,
            }),
        };
        let instr = match source {
            CiteSource::Mendeley => format!(" ADDIN CSL_CITATION {json} "),
            _ => format!(" ADDIN ZOTERO_ITEM CSL_CITATION {json} "),
        };
        self.insert_cite_field(sel, &instr, false, locale_default)
    }

    /// Insert a bibliography at the caret (Zotero's field, or Mendeley
    /// Desktop's in a Mendeley document), its entries rendered: one undo
    /// step. A bibliography in an empty paragraph takes the
    /// `Bibliography` style when the document has it.
    pub fn insert_bibliography(
        &mut self,
        sel: Selection,
        locale_default: Option<&str>,
    ) -> anyhow::Result<Selection> {
        let instr = match self.new_citation_source() {
            CiteSource::Mendeley => " ADDIN Mendeley Bibliography CSL_BIBLIOGRAPHY ".to_owned(),
            _ => {
                " ADDIN ZOTERO_BIBL {\"uncited\":[],\"omitted\":[],\"custom\":[]} CSL_BIBLIOGRAPHY "
                    .to_owned()
            }
        };
        self.insert_cite_field(sel, &instr, true, locale_default)
    }

    fn insert_cite_field(
        &mut self,
        sel: Selection,
        instr: &str,
        block: bool,
        locale_default: Option<&str>,
    ) -> anyhow::Result<Selection> {
        // Fail before changing anything when the style can't be found.
        let prefs = self.citation_prefs(locale_default);
        kantor_cite::load_style(&prefs.style)?;
        let sel = self.clamp_selection(sel);
        let (mut paras, at) = if sel.is_empty() {
            (self.paragraphs.clone(), sel.head)
        } else {
            let mut d = self.snapshot(self.paragraphs.clone());
            let at = d.delete_selection(sel).head;
            (std::mem::take(&mut d.paragraphs), at)
        };
        let p = &mut paras[at.para];
        let props = p.props_at(at.offset);
        if block && p.text.is_empty() && self.styles.get("Bibliography").is_some() {
            p.props.style = Some("Bibliography".into());
        }
        let begin = Arc::new(complex_begin(instr, true));
        let mut b = Run::text(crate::model::OBJECT.len_utf8(), props.clone());
        b.object = Some(Object::Field(begin));
        let mut e = Run::text(crate::model::OBJECT.len_utf8(), props);
        e.object = Some(Object::Field(Arc::new(FieldMark::end())));
        p.insert_runs(at.offset, vec![b, e]);
        let (paras, _) = self.refreshed(paras, locale_default, None)?;
        // After the new field's end.
        let end = spans(&paras)
            .into_iter()
            .find(|s| s.begin == at)
            .map_or(at, |s| {
                Position::new(s.end.para, s.end.offset + crate::model::OBJECT.len_utf8())
            });
        let after = Selection::caret(end);
        let after = self.apply_paragraphs(paras, sel, after);
        // Zotero's plug-in needs the document's preferences to go on.
        if prefs.from == PrefsFrom::Default && self.new_citation_source() == CiteSource::Zotero {
            self.set_citation_prefs(&prefs.style, prefs.locale.as_deref());
        }
        Ok(after)
    }

    /// Make `paras` the body as one undo step (the changed range only;
    /// tracked when Track Changes is on).
    pub(crate) fn apply_paragraphs(
        &mut self,
        paras: Vec<Paragraph>,
        sel: Selection,
        after: Selection,
    ) -> Selection {
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
        let new = self.track_rewrite(&self.paragraphs[start..start + count], new);
        self.replace(start, count, new, (sel, after), Kind::Other, true);
        self.clamp_selection(after)
    }
}

/// Replace the paragraphs of an unwrapped bibliography control by
/// `entries`, keeping its start and end markers (and other markers) at its
/// edges.
fn set_paragraph_entries(
    paras: &mut Vec<Paragraph>,
    first: usize,
    last: usize,
    entries: &[Vec<Piece>],
) {
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    for p in &paras[first..=last] {
        for r in p.markers_in(0..p.text.len()) {
            let is_end = matches!(&r.object, Some(Object::Opaque(o)) if o.level == Level::Wrap && o.xml.starts_with("</"));
            if is_end {
                ends.push(r);
            } else {
                starts.push(r);
            }
        }
    }
    let base = (first..=last)
        .find_map(|k| {
            paras[k]
                .runs
                .iter()
                .find(|r| r.object.is_none() && r.len > 0)
                .map(|r| r.props.clone())
        })
        .unwrap_or_default();
    let n = entries.len().max(1);
    let empty: Vec<Piece> = Vec::new();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let like = &paras[(first + i).min(last)];
        let e = entries.get(i).unwrap_or(&empty);
        let mut p = paragraph_of(e, &base, like);
        if i == 0 {
            p.insert_runs(0, std::mem::take(&mut starts));
        }
        if i + 1 == n {
            let at = p.text.len();
            p.insert_runs(at, std::mem::take(&mut ends));
        }
        out.push(p);
    }
    // The last paragraph's section break stays on the last one.
    if let Some(sect) = paras[last]
        .props
        .extras
        .iter()
        .find(|x| crate::model::element_name(x) == "sectPr")
        .map(str::to_owned)
        && let Some(p) = out.last_mut()
    {
        p.props.extras.push(&sect);
    }
    paras.splice(first..=last, out);
}

// ── the document's own source list ─────────────────────────────────────────

/// The namespace of Kantor's source list part.
pub const SOURCES_NS: &str = "urn:kantor:citations:1";
const CT_CUSTOM: &str = "application/vnd.openxmlformats-officedocument.custom-properties+xml";
const REL_CUSTOM: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/custom-properties";
const REL_CUSTOM_XML: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXml";
const REL_CUSTOM_XML_PROPS: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXmlProps";
const CT_CUSTOM_XML_PROPS: &str =
    "application/vnd.openxmlformats-officedocument.customXmlProperties+xml";
/// The source list part's datastore id (the same in every file).
const SOURCES_ITEM_ID: &str = "{6B1F6A52-3C1D-4E0B-9A7E-4B414E544F52}";

/// The source list part's XML.
fn sources_xml(items: &[Value]) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<kantor:sources xmlns:kantor=\"{SOURCES_NS}\" format=\"csl-json\">{}</kantor:sources>",
        xml::escape(&Value::Array(items.to_vec()).to_string())
    )
}

/// The items in a source list part.
fn parse_sources(xml: &str) -> Option<Vec<Value>> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    if root.tag_name().namespace() != Some(SOURCES_NS) {
        return None;
    }
    let text: String = root.children().filter_map(|c| c.text()).collect();
    match serde_json::from_str(&text).ok()? {
        Value::Array(a) => Some(a),
        _ => None,
    }
}

/// The package's source list part: its name and items.
fn sources_part(pkg: &crate::docx::Package) -> Option<(String, Vec<Value>)> {
    let mut names: Vec<&String> = pkg
        .part_names()
        .filter(|n| n.starts_with("customXml/item") && n.ends_with(".xml") && !n.contains("Props"))
        .collect();
    names.sort();
    names.into_iter().find_map(|n| {
        let x = pkg.part_text(n)?;
        if !x.contains(SOURCES_NS) {
            return None;
        }
        parse_sources(&x).map(|v| (n.clone(), v))
    })
}

/// Where [`write_parts`] puts what it writes.
pub(crate) struct PartSink<'a> {
    pub replace: &'a mut std::collections::HashMap<String, Vec<u8>>,
    pub add: &'a mut Vec<(String, Vec<u8>)>,
    /// Relationships of the main document part.
    pub doc_rels: &'a mut String,
    /// Relationships of the package (`_rels/.rels`).
    pub root_rels: &'a mut String,
    pub overrides: &'a mut String,
}

/// Write the citation parts the document changed.
pub(crate) fn write_parts(
    doc: &Document,
    pkg: &crate::docx::Package,
    out: PartSink,
) -> anyhow::Result<()> {
    let Some(parts) = &doc.cite_parts else {
        return Ok(());
    };
    if let Some(custom) = &parts.custom {
        if pkg.has_part(CUSTOM_PART) {
            out.replace
                .insert(CUSTOM_PART.into(), custom.clone().into_bytes());
        } else {
            out.add
                .push((CUSTOM_PART.into(), custom.clone().into_bytes()));
            out.overrides.push_str(&format!(
                "<Override PartName=\"/{CUSTOM_PART}\" ContentType=\"{CT_CUSTOM}\"/>"
            ));
            out.root_rels.push_str(&format!(
                "<Relationship Id=\"rIdKantorCustom\" Type=\"{REL_CUSTOM}\" Target=\"{CUSTOM_PART}\"/>"
            ));
        }
    }
    if let Some(items) = &parts.sources {
        let xml = sources_xml(items).into_bytes();
        match sources_part(pkg) {
            Some((name, _)) => {
                out.replace.insert(name, xml);
            }
            None => {
                let n = (1..)
                    .find(|n| !pkg.has_part(&format!("customXml/item{n}.xml")))
                    .unwrap_or(1);
                let name = format!("customXml/item{n}.xml");
                let props = format!("customXml/itemProps{n}.xml");
                out.add.push((name.clone(), xml));
                out.add.push((
                    props.clone(),
                    format!(
                        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\r\n<ds:datastoreItem ds:itemID=\"{SOURCES_ITEM_ID}\" xmlns:ds=\"http://schemas.openxmlformats.org/officeDocument/2006/customXml\"><ds:schemaRefs><ds:schemaRef ds:uri=\"{SOURCES_NS}\"/></ds:schemaRefs></ds:datastoreItem>"
                    )
                    .into_bytes(),
                ));
                out.add.push((
                    format!("customXml/_rels/item{n}.xml.rels"),
                    format!(
                        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"{REL_CUSTOM_XML_PROPS}\" Target=\"itemProps{n}.xml\"/></Relationships>"
                    )
                    .into_bytes(),
                ));
                out.overrides.push_str(&format!(
                    "<Override PartName=\"/{props}\" ContentType=\"{CT_CUSTOM_XML_PROPS}\"/>"
                ));
                // Packages without a default for .xml still read it.
                if !pkg
                    .part_text("[Content_Types].xml")
                    .is_some_and(|ct| ct.contains("Extension=\"xml\""))
                {
                    out.overrides.push_str(&format!(
                        "<Override PartName=\"/{name}\" ContentType=\"application/xml\"/>"
                    ));
                }
                let id = crate::docx::fresh_rel_id(pkg, "rIdKantorSources");
                out.doc_rels.push_str(&format!(
                    "<Relationship Id=\"{id}\" Type=\"{REL_CUSTOM_XML}\" Target=\"{}\"/>",
                    xml::escape(&crate::docx::relative_to(&pkg.document_part, &name))
                ));
            }
        }
    }
    Ok(())
}

/// The key two source items are the same work by: their id, else title
/// and year.
fn source_key(v: &Value) -> String {
    let title = v
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let year = crate::citations::year_of(v).unwrap_or(0);
    format!("{title}|{year}")
}

impl Document {
    /// The document's own source list (imported `.bib`, RIS or CSL-JSON
    /// items), CSL-JSON.
    pub fn sources(&self) -> Vec<Value> {
        if let Some(s) = self.cite_parts.as_ref().and_then(|p| p.sources.clone()) {
            return s;
        }
        self.package
            .as_ref()
            .and_then(sources_part)
            .map(|(_, v)| v)
            .unwrap_or_default()
    }

    /// Add `items` to the source list (an item already there, by id or by
    /// title and year, is replaced). Not an undo step, like a document
    /// property. Returns how many were new.
    pub fn add_sources(&mut self, items: Vec<Value>) -> usize {
        let mut list = self.sources();
        let mut added = 0;
        for it in items {
            let id = it.get("id").map(Value::to_string);
            let key = source_key(&it);
            let same = list.iter().position(|x| {
                (id.is_some() && x.get("id").map(Value::to_string) == id)
                    || (key != "|0" && source_key(x) == key)
            });
            match same {
                Some(i) => list[i] = it,
                None => {
                    list.push(it);
                    added += 1;
                }
            }
        }
        self.set_sources(list);
        added
    }

    /// Replace the source list.
    pub fn set_sources(&mut self, items: Vec<Value>) {
        if self.sources() == items {
            return;
        }
        let mut parts = self.cite_parts.as_deref().cloned().unwrap_or_default();
        parts.sources = Some(items);
        self.cite_parts = Some(Arc::new(parts));
        self.mark_modified();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_xml_edits() {
        let src = "<?xml version=\"1.0\"?><Properties xmlns=\"x\" xmlns:vt=\"y\"><property fmtid=\"{F}\" pid=\"2\" name=\"ZOTERO_PREF_1\"><vt:lpwstr>a</vt:lpwstr></property><property fmtid=\"{F}\" pid=\"3\" name=\"Other\"><vt:bool>true</vt:bool></property></Properties>";
        let out = edit_custom(
            Some(src),
            &|n| n.starts_with(ZOTERO_PREF),
            &[("ZOTERO_PREF_1".into(), "b<c".into())],
        );
        assert!(!out.contains("<vt:lpwstr>a</vt:lpwstr>"), "{out}");
        assert!(out.contains("<vt:bool>true</vt:bool>"));
        assert!(
            out.contains("pid=\"4\" name=\"ZOTERO_PREF_1\"><vt:lpwstr>b&lt;c</vt:lpwstr>"),
            "{out}"
        );
        let props = crate::fields::custom_props(&out);
        assert_eq!(zotero_pref_xml(&props).as_deref(), Some("b<c"));
        let fresh = edit_custom(None, &|_| false, &[("K".into(), "V".into())]);
        assert!(fresh.contains("<Properties xmlns="));
        assert_eq!(
            crate::fields::custom_props(&fresh),
            [("K".into(), "V".into())]
        );
    }

    #[test]
    fn zotero_prefs_round_trip() {
        let pref = r#"<data data-version="3"><session id="X"/><style id="http://www.zotero.org/styles/apa" locale="id-ID" hasBibliography="1"/><prefs/></data>"#;
        assert_eq!(
            zotero_style(pref),
            Some((
                "http://www.zotero.org/styles/apa".into(),
                Some("id-ID".into())
            ))
        );
        let new = zotero_pref_with(Some(pref), "http://www.zotero.org/styles/ieee", None, None);
        assert_eq!(
            zotero_style(&new),
            Some(("http://www.zotero.org/styles/ieee".into(), None))
        );
        assert!(new.contains("hasBibliography=\"1\""));
        assert!(new.contains("<session id=\"X\"/>"));
        let fresh = zotero_pref_with(None, "apa", Some("en-US"), Some(1));
        assert_eq!(
            zotero_style(&fresh),
            Some(("apa".into(), Some("en-US".into())))
        );
        let long = "x".repeat(600);
        assert_eq!(chunks(&long).len(), 3);
    }

    #[test]
    fn markup_for_managers() {
        let pieces = vec![
            Piece {
                text: "Jurnal ".into(),
                fmt: Fmt {
                    italic: true,
                    ..Fmt::default()
                },
            },
            Piece {
                text: "– {x}".into(),
                fmt: Fmt::default(),
            },
        ];
        assert_eq!(rtf(&pieces), "{\\i{}Jurnal }\\uc0\\u8211{} \\{x\\}");
        assert_eq!(html(&pieces), "<i>Jurnal </i>– {x}");
    }

    #[test]
    fn instructions_and_codes() {
        let instr = r#" ADDIN ZOTERO_ITEM CSL_CITATION {"a":1} "#;
        assert_eq!(
            instr_with_json(instr, &json!({"b": 2})),
            r#" ADDIN ZOTERO_ITEM CSL_CITATION {"b":2} "#
        );
        let mark = FieldMark::Begin {
            instr: instr.into(),
            code: r#"<w:r><w:rPr><w:noProof/></w:rPr><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> ADDIN ZOTERO_ITEM CSL_CITATION {"a"</w:instrText></w:r><w:r><w:instrText>:1} </w:instrText></w:r>"#.into(),
            sep: Some(crate::object::FIELD_SEPARATE.into()),
            simple: false,
        };
        let FieldMark::Begin { code, instr, .. } = mark_with_instr(&mark, " NEW & ") else {
            panic!()
        };
        assert_eq!(instr, " NEW & ");
        assert!(code.contains("<w:noProof/>"));
        assert!(code.contains(r#"<w:instrText xml:space="preserve"> NEW &amp; </w:instrText></w:r><w:r><w:instrText xml:space="preserve"></w:instrText>"#), "{code}");
    }

    #[test]
    fn controls_get_new_text() {
        let xml = r#"<w:sdt><w:sdtPr><w:tag w:val="MENDELEY_CITATION_v3_old"/></w:sdtPr><w:sdtContent><w:r><w:rPr><w:color w:val="000000"/></w:rPr><w:t>(Old)</w:t></w:r></w:sdtContent></w:sdt>"#;
        let pieces = vec![
            Piece {
                text: "(".into(),
                fmt: Fmt::default(),
            },
            Piece {
                text: "Ibid".into(),
                fmt: Fmt {
                    italic: true,
                    ..Fmt::default()
                },
            },
        ];
        let out = control_with(xml, Some("MENDELEY_CITATION_v3_new"), &pieces).unwrap();
        assert!(out.contains(r#"<w:tag w:val="MENDELEY_CITATION_v3_new"/>"#));
        assert!(out.contains(r#"<w:r><w:rPr><w:i/><w:color w:val="000000"/></w:rPr><w:t xml:space="preserve">Ibid</w:t></w:r>"#), "{out}");
        assert!(out.ends_with("</w:sdtContent></w:sdt>"));
        assert_eq!(
            children("<w:a/><w:b x=\"1\"><w:c/></w:b>"),
            ["<w:a/>", "<w:b x=\"1\"><w:c/></w:b>"]
        );
    }
}
