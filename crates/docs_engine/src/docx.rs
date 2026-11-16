//! `.docx` reading and writing.
//!
//! ## Round-trip safety (plan §5)
//!
//! Saving a document that was opened from a `.docx` **copies every part of
//! the original package byte for byte** (styles, theme, settings, fonts,
//! headers, footnotes, media, custom XML…) and regenerates only the main
//! document part's body. Headers and footers are read into their own small
//! documents ([`crate::section`]) and written back only when edited or
//! added in Kantor; `settings.xml` changes only for "different odd and even
//! pages". The original `<w:document>` start tag (namespace
//! declarations) and the final `<w:sectPr>` (page size, margins, header
//! references) are written back verbatim. Parts change only when the
//! document needs them to: `numbering.xml` gets Kantor's new list
//! definitions appended (originals untouched), inserted pictures add a
//! media part, a relationship and a content type.
//!
//! The body is where loss could happen. Every element is handled by one of
//! four rules:
//!
//! 1. **Modelled**: paragraphs, runs, text, tabs, line and page breaks, soft
//!    and non-breaking hyphens, hyperlinks (text editable, the link start
//!    tag kept), lists (`w:numPr`), tables (`w:tbl`/`w:tr`/`w:tc`, grid,
//!    spans, vertical merges, borders), inline pictures (`wp:inline` with an
//!    embedded blip), footnote and endnote references (run objects whose
//!    notes are read from `footnotes.xml` / `endnotes.xml`, see
//!    [`crate::notes`]), floating pictures, shapes and text boxes
//!    (`wp:anchor`, in a `w:drawing` or an `mc:AlternateContent`'s choice:
//!    their placement and text box are read, the XML written back as read
//!    unless moved or edited, see [`crate::float`]), and the formatting the
//!    editor understands
//!    (`w:pStyle`, `w:jc`, `w:spacing`, `w:ind`, `w:rStyle`, `w:b`/`w:i`/
//!    `w:u`/`w:sz`/`w:color`/`w:rFonts`…).
//! 2. **Kept verbatim with the model**: property elements Kantor doesn't
//!    edit (`w:highlight`, `w:shd`, `w:tabs`, `w:keepNext`, a paragraph's
//!    `w:sectPr`, theme colour attributes…) ride along in [`Extras`] on
//!    their run or paragraph and are written back as they were; table, row
//!    and cell property elements are kept whole.
//! 3. **Opaque objects**: elements that can be carried as one unit without
//!    understanding them become an [`Opaque`] object, one character in the
//!    text with a placeholder in the layout, written back byte for byte:
//!    VML shapes and embedded objects (`w:pict`, `w:object`, an
//!    `mc:AlternateContent` without a drawing anchor), content controls
//!    (`w:sdt`, inline or block), page number fields (PAGE, NUMPAGES,
//!    SECTIONPAGES: the whole `begin…end` run sequence, numbered per page)
//!    and fields whose runs are laid out unusually, equations,
//!    smart tags and custom XML, symbols,
//!    nested tables, and any other unknown body, paragraph or run element.
//!    Bookmarks and permission ranges are zero-width markers that text
//!    deletion never removes. Word's `_GoBack` bookmark, proofing marks and
//!    `w:lastRenderedPageBreak` are dropped (no content).
//!
//!    Other fields are modelled as their result between two zero-width
//!    marks ([`FieldMark`]): the start holds the code's XML (`fldChar
//!    begin`, the instruction runs and the separator run, or the
//!    `<w:fldSimple>` start tag), the end its closing run or tag. A field
//!    may span paragraphs (a table of contents); its result is ordinary
//!    text, nested fields included. A table of contents' block content
//!    control is unwrapped the same way: its start and end tags are
//!    zero-width markers (`Level::Wrap`) on its first and last paragraph.
//!    See [`crate::fields`].
//! 4. **Unsupported** (the conservative whole-document flag, see
//!    [`Read::unsupported`]): tracked changes (`w:ins`, `w:del`, moves,
//!    property changes, "track changes" switched on), comments, fields whose
//!    code (not result) runs across paragraphs or that never end, content
//!    controls around table rows or cells, and unusual namespace prefixes. Their XML is still kept as opaque objects where
//!    possible, but the document counts as "imported with unsupported
//!    content": [`crate::Document::save`] refuses to overwrite the original,
//!    the UI routes Save to Save As, and `save_as` refuses the original's
//!    path.
//!
//! The reader is a quick-xml pass; the writer is hand-written XML (children
//! of property elements in schema order). docx-rs packages new documents
//! (styles, settings, page setup), which are then written like any other.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{Cursor, Read as _, Write as _};
use std::sync::Arc;

use anyhow::{Context as _, bail};
use quick_xml::NsReader;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};

use crate::document::{Document, LINE_BREAK, PAGE_BREAK, PageGeometry};
use crate::float::{FloatKind, TextBox};
use crate::model::*;
use crate::notes::{Note, NoteKind, NoteProps, NotesPart};
use crate::numbering::{Numbering, parse_ind};
use crate::review::Revision;
use crate::section::{HfKind, Story, set_settings_even_odd, settings_even_odd};
use crate::style::{Style, StyleKind, StyleSheet, TableStyle};
use crate::table::{Border, Borders, Cell, CellMargins, Row, Table, Tables, VMerge};
use crate::xml::{self, El, PPR_ORDER, RPR_ORDER, escape, ordered};

const W: &[u8] = b"http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const W_DECL: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";
const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n";
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const REL_IMAGE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
const REL_NUMBERING: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering";
const REL_HEADER: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/header";
const REL_FOOTER: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer";
const CT_HEADER: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml";
const CT_FOOTER: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml";
const REL_COMMENTS: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments";
const CT_COMMENTS: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml";
const REL_FOOTNOTES: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes";
const CT_FOOTNOTES: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml";
const REL_ENDNOTES: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/endnotes";
const CT_ENDNOTES: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml";
const CT_NUMBERING: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml";

/// The original package a document was read from: the template for saving.
#[derive(Clone, Debug)]
pub struct Package {
    bytes: Arc<Vec<u8>>,
    /// Zip entry name of the main document part (usually `word/document.xml`).
    pub(crate) document_part: String,
    /// The original `<w:document …>` start tag.
    pub(crate) root_tag: String,
    /// The original body-level `<w:sectPr>…</w:sectPr>`, if any.
    pub(crate) sect_pr: Option<String>,
    /// The numbering part's name and text, if the package has one.
    numbering: Option<(String, String)>,
    /// The settings part's name and text, if the package has one.
    settings: Option<(String, String)>,
    /// The comments part, if the package has one.
    comments: Option<crate::review::CommentsPart>,
    /// The threaded comment parts (commentsExtended, commentsIds,
    /// commentsExtensible, people), as read.
    comment_threads: crate::comment_thread::ThreadParts,
    /// The footnotes and endnotes parts, if the package has them.
    pub(crate) notes: [Option<NotesPart>; 2],
    /// The notes as read (kind, id, element): a part is written again only
    /// when the document's notes differ.
    notes_read: Vec<(NoteKind, String, Option<String>)>,
    /// Relationship ids of the main document part.
    rel_ids: HashSet<String>,
    /// Every zip entry name.
    parts: HashSet<String>,
}

impl Package {
    /// A part's text, if the package has it (document properties, for
    /// fields).
    pub(crate) fn part_text(&self, name: &str) -> Option<String> {
        let mut zip = zip::ZipArchive::new(Cursor::new(self.bytes.as_slice())).ok()?;
        zip_text(&mut zip, name).ok().flatten()
    }

    /// The theme part's text (colours of shapes), if the package has one.
    pub(crate) fn theme_text(&self) -> Option<String> {
        let mut names: Vec<&String> = self
            .parts
            .iter()
            .filter(|p| p.starts_with("word/theme/") && p.ends_with(".xml"))
            .collect();
        names.sort();
        self.part_text(names.first()?)
    }

    /// Where the main document's relationship `id` points (a hyperlink's
    /// address).
    pub(crate) fn rel_target(&self, id: &str) -> Option<String> {
        let rels = self.part_text(&rels_name(&self.document_part))?;
        parse_rels(&rels)
            .into_iter()
            .find(|r| r.id == id)
            .map(|r| r.target)
    }

    pub fn has_rel(&self, id: &str) -> bool {
        self.rel_ids.contains(id)
    }

    pub fn has_part(&self, name: &str) -> bool {
        self.parts.contains(name)
    }
}

/// A picture part referenced from the body.
#[derive(Clone, Debug)]
pub struct Media {
    pub part: String,
    /// The bytes, for formats Kantor draws (PNG, JPEG).
    pub bytes: Option<Arc<[u8]>>,
}

/// What [`read`] found.
pub struct Read {
    pub paragraphs: Vec<Paragraph>,
    pub tables: Tables,
    pub styles: StyleSheet,
    pub numbering: Numbering,
    pub page: PageGeometry,
    pub package: Package,
    /// Pictures by relationship id.
    pub media: HashMap<String, Media>,
    /// Unmodelled content that makes Save go to Save As, as short
    /// descriptions ("tracked changes", "comments").
    pub unsupported: Vec<String>,
    /// Content kept byte for byte but shown only as a placeholder (or not at
    /// all), e.g. "floating images", "headers and footers".
    pub preserved: Vec<String>,
    /// Header and footer parts.
    pub stories: Vec<Story>,
    /// Different odd and even page headers.
    pub even_odd: bool,
    pub comments: Vec<crate::review::Comment>,
    /// `w:defaultTabStop` (twips).
    pub default_tab: u32,
    /// `w:decimalSymbol` (what decimal tab stops line up on).
    pub decimal: char,
    /// Track Changes is on (`w:trackRevisions`).
    pub track: bool,
    /// Footnotes and endnotes (separators included), in file order.
    pub notes: Vec<Note>,
    /// Note numbering from settings.xml (footnotes, endnotes).
    pub note_settings: [NoteProps; 2],
}

fn zip_bytes(
    zip: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> anyhow::Result<Option<Vec<u8>>> {
    let mut file = match zip.by_name(name) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut v = Vec::new();
    file.read_to_end(&mut v)
        .with_context(|| format!("reading {name}"))?;
    Ok(Some(v))
}

fn zip_text(
    zip: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> anyhow::Result<Option<String>> {
    Ok(match zip_bytes(zip, name)? {
        Some(mut b) => {
            // A byte order mark (the Open XML SDK writes one) would put the
            // XML reader's positions 3 bytes off the text sliced with them.
            if b.starts_with(b"\xEF\xBB\xBF") {
                b.drain(..3);
            }
            Some(String::from_utf8(b).with_context(|| format!("{name} isn't UTF-8"))?)
        }
        None => None,
    })
}

#[derive(Clone, Debug)]
struct Rel {
    id: String,
    ty: String,
    target: String,
    external: bool,
}

fn parse_rels(rels: &str) -> Vec<Rel> {
    let mut out = Vec::new();
    let mut r = quick_xml::Reader::from_str(rels);
    loop {
        match r.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) if e.local_name().as_ref() == b"Relationship" => {
                out.push(Rel {
                    id: attr(&e, b"Id").unwrap_or_default(),
                    ty: attr(&e, b"Type").unwrap_or_default(),
                    target: attr(&e, b"Target").unwrap_or_default(),
                    external: attr(&e, b"TargetMode").as_deref() == Some("External"),
                });
            }
            Ok(Event::Eof) | Err(_) => return out,
            _ => {}
        }
    }
}

/// Target of the first internal relationship whose type ends with `kind`.
fn rel_target(rels: &[Rel], kind: &str) -> Option<String> {
    rels.iter()
        .find(|r| r.ty.ends_with(kind) && !r.external)
        .map(|r| r.target.clone())
}

/// Resolve a relationship target relative to the part at `base`.
fn resolve(base: &str, target: &str) -> String {
    if let Some(abs) = target.strip_prefix('/') {
        return abs.to_owned();
    }
    let mut parts: Vec<&str> = base.split('/').collect();
    parts.pop();
    for seg in target.split('/') {
        match seg {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn rels_name(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}

/// `target` relative to the directory of `base` (for new relationships).
fn relative_to(base: &str, target: &str) -> String {
    match base.rsplit_once('/') {
        Some((dir, _)) => target
            .strip_prefix(&format!("{dir}/"))
            .map(str::to_owned)
            .unwrap_or_else(|| format!("/{target}")),
        None => target.to_owned(),
    }
}

fn attr_value(a: &quick_xml::events::attributes::Attribute) -> String {
    let raw = String::from_utf8_lossy(&a.value);
    quick_xml::escape::unescape(&raw)
        .map(|v| v.into_owned())
        .unwrap_or_else(|_| raw.into_owned())
}

/// Attribute value by local name (namespace prefixes ignored).
fn attr(e: &BytesStart, name: &[u8]) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        (a.key.local_name().as_ref() == name)
            .then(|| Some(attr_value(&a)))
            .flatten()
    })
}

// ── properties ────────────────────────────────────────────────────────────

/// How a property element was handled.
enum Prop {
    /// Fully held by the model.
    Modelled,
    /// Keep the XML verbatim (the model may hold part of it too).
    Keep,
    /// Keep it, and the document is unsupported.
    Unsupported(String),
}

fn border(el: &El) -> Border {
    Border {
        on: !matches!(el.attr("val"), Some("nil" | "none") | None),
        size: el.num("sz").unwrap_or(4),
        color: el.attr("color").and_then(Rgb::parse),
    }
}

pub(crate) fn borders(el: &El) -> Borders {
    let mut b = Borders::default();
    for c in &el.children {
        let v = Some(border(c));
        match c.name.as_str() {
            "top" => b.top = v,
            "left" | "start" => b.left = v,
            "bottom" => b.bottom = v,
            "right" | "end" => b.right = v,
            "insideH" => b.inside_h = v,
            "insideV" => b.inside_v = v,
            _ => {}
        }
    }
    b
}

pub(crate) fn cell_margins(el: &El) -> CellMargins {
    let mut m = CellMargins::default();
    for c in &el.children {
        let w = c.num::<u32>("w");
        match (c.name.as_str(), w) {
            ("top", Some(w)) => m.top = w,
            ("left" | "start", Some(w)) => m.left = w,
            ("bottom", Some(w)) => m.bottom = w,
            ("right" | "end", Some(w)) => m.right = w,
            _ => {}
        }
    }
    m
}

/// A `w:rPr` child.
fn run_prop(xml: &str, p: &mut RunProps) -> Prop {
    let Some(el) = xml::parse(xml) else {
        return Prop::Keep;
    };
    let exact = |allowed: &[&str]| {
        if el.only_attrs(allowed) && el.children.is_empty() {
            Prop::Modelled
        } else {
            Prop::Keep
        }
    };
    match el.name.as_str() {
        "b" => {
            p.bold = Some(el.on());
            exact(&["val"])
        }
        "i" => {
            p.italic = Some(el.on());
            exact(&["val"])
        }
        "u" => {
            let val = el.attr("val");
            p.underline = Some(!matches!(val, Some("none")));
            if matches!(val, None | Some("single" | "none")) {
                exact(&["val"])
            } else {
                Prop::Keep
            }
        }
        "sz" => match el.num("val") {
            Some(v) => {
                p.size = Some(v);
                exact(&["val"])
            }
            None => Prop::Keep,
        },
        "color" => {
            p.color = Some(
                el.attr("val")
                    .and_then(Rgb::parse)
                    .unwrap_or(Rgb([0, 0, 0])),
            );
            exact(&["val"])
        }
        "rStyle" => {
            p.style = el.attr("val").map(str::to_owned);
            exact(&["val"])
        }
        "rFonts" => {
            let f = &mut p.fonts;
            let mut known = true;
            for (k, v) in &el.attrs {
                let v = Some(v.clone());
                match k.as_str() {
                    "ascii" => f.ascii = v,
                    "hAnsi" => f.h_ansi = v,
                    "eastAsia" => f.east_asia = v,
                    "cs" => f.cs = v,
                    "asciiTheme" => f.ascii_theme = v,
                    "hAnsiTheme" => f.h_ansi_theme = v,
                    "eastAsiaTheme" => f.east_asia_theme = v,
                    "cstheme" => f.cs_theme = v,
                    "hint" => f.hint = v,
                    _ => known = false,
                }
            }
            if known { Prop::Modelled } else { Prop::Keep }
        }
        // A paragraph mark's revision marks and formatting changes:
        // resolved by Accept / Reject All (crate::review).
        "rPrChange" | "ins" | "del" | "moveFrom" | "moveTo" => Prop::Keep,
        _ => Prop::Keep,
    }
}

/// Parse a whole `<w:rPr>…</w:rPr>` into `p` (Reject All's old run
/// formatting).
pub(crate) fn parse_rpr_into(rpr: &str, p: &mut RunProps) {
    for child in child_elements(rpr) {
        if let Prop::Keep | Prop::Unsupported(_) = run_prop(&child, p) {
            p.extras.push(&child);
        }
    }
}

/// Parse a whole `<w:pPr>…</w:pPr>` into `p` (Reject All's old paragraph
/// formatting).
pub(crate) fn parse_ppr_into(ppr: &str, p: &mut ParaProps) {
    for child in child_elements(ppr) {
        if let Prop::Keep | Prop::Unsupported(_) = para_prop(&child, p) {
            p.extras.push(&child);
        }
    }
}

/// The direct child elements of the root of `xml`, as source text.
fn child_elements(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut depth = 0usize;
    let mut start = 0usize;
    loop {
        let before = reader.buffer_position() as usize;
        let Ok(ev) = reader.read_event() else { break };
        let after = reader.buffer_position() as usize;
        match ev {
            Event::Start(_) => {
                if depth == 1 {
                    start = before;
                }
                depth += 1;
            }
            Event::Empty(_) => {
                if depth == 1 {
                    out.push(xml[before..after].to_owned());
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 1 {
                    out.push(xml[start..after].to_owned());
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    out
}

/// A `w:pPr` child (not the mark's `w:rPr`, handled by the caller).
fn para_prop(xml: &str, p: &mut ParaProps) -> Prop {
    let Some(el) = xml::parse(xml) else {
        return Prop::Keep;
    };
    match el.name.as_str() {
        "pStyle" => {
            p.style = el.attr("val").map(str::to_owned);
            Prop::Modelled
        }
        "jc" => match el.attr("val").and_then(Align::from_jc) {
            Some(a) => {
                p.align = Some(a);
                Prop::Modelled
            }
            None => Prop::Keep,
        },
        "spacing" => {
            let mut known = true;
            for (k, v) in &el.attrs {
                match k.as_str() {
                    "before" => p.before = v.parse().ok(),
                    "after" => p.after = v.parse().ok(),
                    "line" => {
                        let rule = p.line.map(|l| l.rule).unwrap_or_default();
                        p.line = v.parse().ok().map(|line| LineSpacing { line, rule });
                    }
                    "lineRule" => {
                        let rule = match v.as_str() {
                            "exact" => LineRule::Exact,
                            "atLeast" => LineRule::AtLeast,
                            _ => LineRule::Auto,
                        };
                        let line = p.line.map_or(240, |l| l.line);
                        p.line = Some(LineSpacing { line, rule });
                    }
                    "beforeAutospacing" | "afterAutospacing" if v == "0" || v == "false" => {}
                    _ => known = false,
                }
            }
            if known { Prop::Modelled } else { Prop::Keep }
        }
        "ind" => {
            p.indent.overlay(&parse_ind(&el));
            if el.only_attrs(&["left", "start", "right", "end", "firstLine", "hanging"]) {
                Prop::Modelled
            } else {
                Prop::Keep
            }
        }
        "numPr" => {
            if el.children.iter().any(|c| c.name == "ins") {
                return Prop::Unsupported("tracked changes".into());
            }
            let id = el.child("numId").and_then(|n| n.num("val"));
            let level = el.child("ilvl").and_then(|n| n.num("val")).unwrap_or(0);
            match id {
                Some(id) => {
                    p.num = Some(NumRef { id, level });
                    if el
                        .children
                        .iter()
                        .all(|c| c.name == "numId" || c.name == "ilvl")
                    {
                        Prop::Modelled
                    } else {
                        Prop::Keep
                    }
                }
                None => Prop::Keep,
            }
        }
        "pageBreakBefore" => {
            p.page_break_before = Some(el.on());
            Prop::Modelled
        }
        "contextualSpacing" => {
            p.contextual = Some(el.on());
            Prop::Modelled
        }
        // Resolved by Accept / Reject All (crate::review).
        "pPrChange" => Prop::Keep,
        _ => Prop::Keep,
    }
}

// ── body ───────────────────────────────────────────────────────────────────

/// A readable name for unsupported content.
fn category(local: &str) -> String {
    match local {
        "ins"
        | "del"
        | "moveFrom"
        | "moveTo"
        | "rPrChange"
        | "pPrChange"
        | "sectPrChange"
        | "delText"
        | "delInstrText"
        | "moveFromRangeStart"
        | "moveFromRangeEnd"
        | "moveToRangeStart"
        | "moveToRangeEnd"
        | "tblPrChange"
        | "tcPrChange"
        | "trPrChange"
        | "tblGridChange"
        | "cellIns"
        | "cellDel"
        | "customXmlInsRangeStart"
        | "customXmlDelRangeStart" => "tracked changes",
        "commentRangeStart" | "commentRangeEnd" | "commentReference" => "comments",
        other => return format!("<w:{other}>"),
    }
    .to_owned()
}

fn is_tracked(local: &str) -> bool {
    category(local) == "tracked changes"
}

fn field_label(instr: &str, result: &str) -> String {
    let r = result.trim();
    if !r.is_empty() {
        return r.chars().take(60).collect();
    }
    let word = instr.split_whitespace().next().unwrap_or("field");
    format!("Field: {word}")
}

fn short(text: &str, n: usize) -> String {
    let t: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > n {
        format!("{}…", t.chars().take(n).collect::<String>())
    } else {
        t
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Frame {
    Document,
    Body,
    P,
    PPr,
    MarkRPr,
    R,
    RPr,
    T,
    Instr,
    Hyperlink,
    /// Inside a simple field (`w:fldSimple`): its result runs.
    Simple,
    /// A table of contents' block content control (`w:sdt`) whose
    /// paragraphs are read as ordinary ones.
    Wrap,
    /// Inside a tracked insertion or deletion (`w:ins`, `w:del`, moves).
    Rev,
    Tbl,
    Tr,
    Tc,
    /// Raw capture in progress (see `BodyParser::caps`); inner content ignored.
    Capture,
    /// Inside an element whose content is ignored.
    Skip,
}

#[derive(Clone, Debug)]
enum Cap {
    RunObj,
    ParaObj,
    Marker,
    BlockObj,
    PPrChild,
    RPrChild,
    MarkChild,
    TblPr,
    TblGrid,
    TrPr,
    TblPrEx,
    TcPr,
    BodySect,
}

/// How a complex field (`w:fldChar` begin … end) is being read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FieldMode {
    /// Between `begin` and `separate`: the code, kept as XML.
    Code,
    /// After `separate`: the result is read as ordinary runs between the
    /// field's start and end marks ([`FieldMark`]).
    Result,
    /// The whole field kept as one opaque object (page numbers, which are
    /// filled in per page, and fields whose runs are laid out unusually).
    Opaque,
}

/// A complex field being read.
struct Field {
    /// Byte offset of the `begin` run.
    start: usize,
    /// Nesting of fields inside the code (or inside an opaque field).
    depth: u32,
    mode: FieldMode,
    /// For opaque fields: the separator was seen (text is result).
    in_result: bool,
    instr: String,
    result: String,
    /// Stack depth of the paragraph content level where it began.
    level: usize,
    /// Where it began: stack depth, hyperlink start, tracked change; the
    /// separator must be in the same place for the code to be well formed.
    container: (usize, Option<usize>, Option<*const Revision>),
    /// Formatting of the `begin` run.
    props: RunProps,
    /// The `end` was seen (in the run being read).
    ended: bool,
}

/// A `w:fldChar` met in the run being read, acted on when the run closes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RunFld {
    Begin,
    /// `separate` of a field read in result mode; the paragraph length then.
    Separate(usize),
    /// `end`; the paragraph length then.
    End(usize),
}

/// Whether a field instruction is a page number (`PAGE`, `NUMPAGES`,
/// `SECTIONPAGES`): those stay one object, numbered per page.
fn page_number_field(instr: &str) -> bool {
    let word = instr.split_whitespace().next().unwrap_or("");
    ["PAGE", "NUMPAGES", "SECTIONPAGES"]
        .iter()
        .any(|w| word.eq_ignore_ascii_case(w))
}

struct BodyParser<'a> {
    src: &'a str,
    rels: &'a HashMap<String, Rel>,
    document_part: &'a str,
    paragraphs: Vec<Paragraph>,
    tables: Tables,
    unsupported: BTreeSet<String>,
    preserved: BTreeSet<String>,
    page: PageGeometry,
    root_tag: Option<String>,
    sect_pr: Option<String>,
    para: Paragraph,
    mark: RunProps,
    mark_raw: Option<String>,
    mark_start: usize,
    run: RunProps,
    run_start: usize,
    run_text_start: usize,
    link: Option<Arc<str>>,
    link_start: usize,
    link_runs: usize,
    /// The tracked change being read.
    rev: Option<Arc<Revision>>,
    /// Complex fields being read, innermost last.
    fields: Vec<Field>,
    /// A `w:fldChar` in the run being read.
    run_fld: Option<RunFld>,
    /// Text or objects went into the paragraph during the run being read.
    run_pushed: bool,
    /// The table of contents content controls being read: where their
    /// content ends (set when `w:sdtContent` closes).
    wraps: Vec<usize>,
    caps: Vec<(usize, Cap)>,
    /// Markers met between paragraphs: they go into the next one.
    pending: Vec<Object>,
    go_back: HashSet<String>,
    /// The table being read: id and structure.
    table: Option<(u32, Table)>,
    next_table: u32,
    cell_paras: usize,
    image_rels: BTreeSet<String>,
    /// The frame an element opens or closes under.
    frame_top: Option<Frame>,
    /// Namespace declarations on the current `<w:r>` start tag.
    run_ns: String,
}

impl BodyParser<'_> {
    fn cell(&self) -> Option<CellRef> {
        let (id, t) = self.table.as_ref()?;
        let row = t.rows.len().checked_sub(1)?;
        let cell = t.rows[row].cells.len().checked_sub(1)?;
        Some(CellRef {
            table: *id,
            row: row as u32,
            cell: cell as u32,
        })
    }

    /// Whether a field's code (or a whole opaque field) is being read:
    /// its content is kept as XML, not added to the paragraph.
    fn capturing(&self) -> bool {
        self.fields
            .last()
            .is_some_and(|f| f.mode != FieldMode::Result && f.depth > 0)
    }

    fn push_text(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if self.capturing() {
            if let Some(f) = self.fields.last_mut()
                && f.mode == FieldMode::Opaque
                && f.in_result
                && f.depth == 1
            {
                f.result.push_str(s);
            }
            return;
        }
        self.run_pushed = true;
        self.link_runs += 1;
        self.para
            .push_tracked(s, self.run.clone(), self.link.clone(), self.rev.clone());
    }

    fn push_char(&mut self, c: char) {
        let mut b = [0; 4];
        self.push_text(c.encode_utf8(&mut b));
    }

    /// Add an object to the current paragraph (unless a field's raw capture
    /// already includes it).
    fn push_object(&mut self, o: Object, props: RunProps) {
        if self.capturing() {
            return;
        }
        if !o.is_marker() {
            self.link_runs += 1;
            self.run_pushed = true;
        }
        self.para
            .push_object_tracked(o, props, self.link.clone(), self.rev.clone());
    }

    fn opaque(level: Level, xml: &str, label: impl Into<String>) -> Object {
        Object::Opaque(Arc::new(Opaque {
            level,
            xml: xml.to_owned(),
            label: label.into(),
        }))
    }

    /// A block-level object becomes a paragraph of its own.
    fn push_block(&mut self, o: Object) {
        let mut p = Paragraph {
            cell: self.cell(),
            ..Paragraph::default()
        };
        for m in self.pending.drain(..) {
            p.push_object(m, RunProps::default(), None);
        }
        p.push_object(o, RunProps::default(), None);
        self.paragraphs.push(p);
        self.cell_paras += 1;
    }

    fn marker(&mut self, local: &str, e: &BytesStart, xml: &str, in_para: bool) {
        if local == "bookmarkStart" && attr(e, b"name").as_deref() == Some("_GoBack") {
            self.go_back.insert(attr(e, b"id").unwrap_or_default());
            return;
        }
        if local == "bookmarkEnd" && self.go_back.contains(&attr(e, b"id").unwrap_or_default()) {
            return;
        }
        let o = Self::opaque(Level::Marker, xml, "");
        if in_para {
            self.push_object(o, RunProps::default());
        } else {
            self.pending.push(o);
        }
    }

    fn image(&mut self, xml: &str) -> Option<Object> {
        let el = xml::parse(xml)?;
        let inline = el.child("inline")?;
        let extent = inline.child("extent")?;
        let doc_pr = inline.child("docPr");
        let blip = find(inline, "blip")?;
        let rel_id = blip.attr("embed")?.to_owned();
        let part = self
            .rels
            .get(&rel_id)
            .filter(|r| !r.external)
            .map(|r| resolve(self.document_part, &r.target))?;
        self.image_rels.insert(rel_id.clone());
        Some(Object::Image(Arc::new(Image {
            rel_id,
            cx: extent.num("cx").unwrap_or(0),
            cy: extent.num("cy").unwrap_or(0),
            descr: doc_pr
                .and_then(|d| d.attr("descr"))
                .unwrap_or_default()
                .to_owned(),
            xml: Some(xml.into()),
            doc_pr: doc_pr.and_then(|d| d.num("id")).unwrap_or(0),
            part,
        })))
    }

    /// A floating picture, shape or text box (`wp:anchor` in a
    /// `w:drawing`, or in an `mc:AlternateContent`'s choice).
    fn float(&mut self, xml: &str) -> Option<Object> {
        let (rels, part) = (self.rels, self.document_part);
        let mut f = crate::float::read(xml, &|id| {
            rels.get(id)
                .filter(|r| !r.external)
                .map(|r| resolve(part, &r.target))
        })?;
        match &f.kind {
            FloatKind::Picture { rel_id, part, .. } if !part.is_empty() => {
                self.image_rels.insert(rel_id.clone());
            }
            FloatKind::Group { .. } => {
                // Pictures in the group (its shapes' text: below).
                for id in f.picture_rels() {
                    if rels.get(&id).is_some_and(|r| !r.external) {
                        self.image_rels.insert(id);
                    }
                }
            }
            FloatKind::Chart { part, .. } if !part.is_empty() => {}
            FloatKind::SmartArt { .. } => {}
            FloatKind::Chart { .. } => {
                self.preserved.insert("charts".into());
            }
            FloatKind::Other(what) => {
                self.preserved.insert(match what.as_str() {
                    "SmartArt" => "SmartArt".into(),
                    "Chart" => "charts".into(),
                    _ => "drawings".into(),
                });
            }
            _ => {}
        }
        if let FloatKind::SmartArt { drawings, .. } = &mut f.kind {
            for (id, r) in rels.iter() {
                if r.ty.ends_with("/diagramDrawing") && !r.external {
                    drawings.push((id.clone(), resolve(part, &r.target)));
                }
            }
            drawings.sort();
        }
        if matches!(f.kind, FloatKind::Group { .. }) {
            let boxes: Vec<_> = crate::float::group_text_boxes(xml)
                .into_iter()
                .map(|(inner, insets, v_align)| crate::float::GroupBox {
                    text: Arc::new(TextBox {
                        doc: self.text_box_doc(inner),
                        insets,
                        v_align,
                    }),
                    edited: false,
                })
                .collect();
            f.boxes = boxes;
        }
        // A shape's own text box.
        if f.kind == FloatKind::Shape
            && let Some((inner, insets, v_align)) = crate::float::text_box_xml(xml)
        {
            let doc = self.text_box_doc(inner);
            f.text = Some(Arc::new(TextBox {
                doc,
                insets,
                v_align,
            }));
        }
        Some(Object::Float(Arc::new(f)))
    }

    /// A text box's block content read into its own small document (styles
    /// are the main document's: see [`crate::float::share_styles`]).
    fn text_box_doc(&mut self, inner: &str) -> Document {
        let mut doc = Document::new();
        let Some(root) = self.root_tag.as_deref() else {
            return doc;
        };
        let Some(name_end) = root.find(|c: char| c.is_whitespace() || c == '>') else {
            return doc;
        };
        let src = format!("<w:hdr{}{inner}</w:hdr>", &root[name_end..]);
        if let Ok(body) = parse_body(&src, self.rels, self.document_part) {
            self.image_rels.extend(body.image_rels);
            if !body.paragraphs.is_empty() {
                doc.paragraphs = body.paragraphs;
            }
            doc.tables = body.tables;
        }
        doc
    }

    /// Label for an opaque run element.
    fn run_label(&mut self, local: &str, xml: &str) -> String {
        match local {
            "drawing" => {
                let el = xml::parse(xml);
                let anchor = el.as_ref().and_then(|e| e.child("anchor")).is_some();
                let chart = el.as_ref().and_then(|e| find(e, "chart")).is_some();
                if chart {
                    self.preserved.insert("charts".into());
                    "Chart".into()
                } else if anchor {
                    self.preserved.insert("floating images and shapes".into());
                    "Floating image".into()
                } else {
                    self.preserved.insert("drawings".into());
                    "Drawing".into()
                }
            }
            "pict" | "object" | "AlternateContent" => {
                self.preserved.insert("shapes and embedded objects".into());
                let text = short(&xml::inner_text(xml, &["t"]), 30);
                if text.is_empty() {
                    "Shape".into()
                } else {
                    format!("Text box: {text}")
                }
            }
            "footnoteReference" | "endnoteReference" | "footnoteRef" | "endnoteRef" => {
                if local.starts_with("footnote") {
                    "Footnote".into()
                } else {
                    "Endnote".into()
                }
            }
            "sym" => "Symbol".into(),
            // Comments show in the margin.
            "commentReference" => String::new(),
            "ptab" => "Tab".into(),
            other => {
                self.preserved.insert(format!("<w:{other}>"));
                other.to_owned()
            }
        }
    }

    fn para_label(&mut self, local: &str, xml: &str) -> String {
        let text = short(&xml::inner_text(xml, &["t"]), 40);
        match local {
            "sdt" => {
                self.preserved.insert("content controls".into());
                if text.is_empty() {
                    "Content control".into()
                } else {
                    text
                }
            }
            "oMath" | "oMathPara" => {
                self.preserved.insert("equations".into());
                "Equation".into()
            }
            "smartTag" | "customXml" => {
                if text.is_empty() {
                    "Tagged text".into()
                } else {
                    text
                }
            }
            "fldSimple" => {
                let instr = xml::parse(xml)
                    .and_then(|e| e.attr("instr").map(str::to_owned))
                    .unwrap_or_default();
                self.field_checks(&instr);
                field_label(&instr, &xml::inner_text(xml, &["t"]))
            }
            other => {
                if is_tracked(other) {
                    self.unsupported.insert(category(other));
                } else {
                    self.preserved.insert(format!("<w:{other}>"));
                }
                if text.is_empty() {
                    other.to_owned()
                } else {
                    text
                }
            }
        }
    }

    fn field_checks(&mut self, instr: &str) {
        if !page_number_field(instr) {
            self.preserved.insert("fields".into());
        }
    }

    fn block_label(&mut self, local: &str, xml: &str) -> String {
        match local {
            "sdt" => {
                self.preserved.insert("content controls".into());
                let gallery = xml::parse(xml).and_then(|e| {
                    find(&e, "docPartGallery").and_then(|g| g.attr("val").map(str::to_owned))
                });
                match gallery {
                    Some(g) => format!("{g} (kept as is)"),
                    None => {
                        let t = short(&xml::inner_text(xml, &["t"]), 50);
                        if t.is_empty() {
                            "Content control".into()
                        } else {
                            format!("Content control: {t}")
                        }
                    }
                }
            }
            "tbl" => {
                self.preserved.insert("nested tables".into());
                let t = short(&xml::inner_text(xml, &["t"]), 50);
                format!("Nested table: {t}")
            }
            "altChunk" => {
                self.preserved.insert("embedded documents".into());
                "Embedded document".into()
            }
            other => {
                if is_tracked(other) {
                    self.unsupported.insert(category(other));
                } else {
                    self.preserved.insert(format!("<w:{other}>"));
                }
                let t = short(&xml::inner_text(xml, &["t"]), 50);
                if t.is_empty() { other.to_owned() } else { t }
            }
        }
    }

    /// An element opened (or empty) under `top`, in the `w` namespace (or
    /// not, `is_w == false`). Returns the frame to push.
    #[allow(clippy::too_many_arguments)]
    fn open(
        &mut self,
        top: Frame,
        local: &str,
        is_w: bool,
        e: &BytesStart,
        (before, after): (usize, usize),
        stack_len: usize,
        empty: bool,
    ) -> Frame {
        use Frame::*;
        let cap = |s: &mut Self, c: Cap| {
            s.caps.push((before, c));
            Capture
        };
        if !is_w {
            return match top {
                Body | Tc => cap(self, Cap::BlockObj),
                P | Hyperlink => cap(self, Cap::ParaObj),
                R => cap(self, Cap::RunObj),
                PPr => cap(self, Cap::PPrChild),
                RPr => cap(self, Cap::RPrChild),
                MarkRPr => cap(self, Cap::MarkChild),
                Tr | Tbl => {
                    self.unsupported.insert(format!("<{local}> in a table"));
                    Skip
                }
                _ => Skip,
            };
        }
        match (top, local) {
            (Document, "body") => Body,
            (Document, _) => Skip,
            (Body | Tc, "p") => {
                self.para = Paragraph {
                    cell: self.cell(),
                    ..Paragraph::default()
                };
                for m in std::mem::take(&mut self.pending) {
                    self.para.push_object(m, RunProps::default(), None);
                }
                self.mark = RunProps::default();
                self.mark_raw = None;
                P
            }
            (Body, "tbl") => {
                self.next_table += 1;
                self.table = Some((self.next_table, Table::default()));
                Tbl
            }
            (Body, "sectPr") => cap(self, Cap::BodySect),
            (Body, "sdt") if !empty && self.cell().is_none() => match self.toc_sdt(before) {
                Some((content_start, _)) => {
                    let xml = &self.src[before..content_start];
                    self.pending.push(Self::opaque(Level::Wrap, xml, ""));
                    Wrap
                }
                None => cap(self, Cap::BlockObj),
            },
            (Wrap, "sdtContent") => Body,
            (Wrap, _) => Skip,
            (Body | Tc | Tbl | Tr, "bookmarkStart" | "bookmarkEnd" | "permStart" | "permEnd")
            | (Body | Tc | Tbl | Tr, "moveFromRangeStart" | "moveFromRangeEnd")
            | (Body | Tc | Tbl | Tr, "moveToRangeStart" | "moveToRangeEnd")
            | (Body | Tc | Tbl | Tr, "commentRangeStart" | "commentRangeEnd") => {
                if empty {
                    let xml = &self.src[before..after];
                    self.marker(local, e, xml, false);
                    Skip
                } else {
                    cap(self, Cap::Marker)
                }
            }
            (Body | Tc, "proofErr") => Skip,
            (Tc, "tcPr") => cap(self, Cap::TcPr),
            (Body | Tc, _) => cap(self, Cap::BlockObj),
            (Tbl, "tblPr") => cap(self, Cap::TblPr),
            (Tbl, "tblGrid") => cap(self, Cap::TblGrid),
            (Tbl, "tr") => {
                if let Some((_, t)) = &mut self.table {
                    t.rows.push(Row::default());
                }
                Tr
            }
            (Tr, "trPr") => cap(self, Cap::TrPr),
            (Tr, "tblPrEx") => cap(self, Cap::TblPrEx),
            (Tr, "tc") => {
                if let Some((_, t)) = &mut self.table
                    && let Some(row) = t.rows.last_mut()
                {
                    row.cells.push(Cell::new());
                }
                self.cell_paras = 0;
                Tc
            }
            (Tbl | Tr, other) => {
                let what = if other == "sdt" || other == "customXml" {
                    "content controls around table rows or cells".to_owned()
                } else if is_tracked(other) {
                    category(other)
                } else {
                    format!("<w:{other}> in a table")
                };
                self.unsupported.insert(what);
                Skip
            }
            (P, "pPr") => PPr,
            (PPr, "rPr") => {
                self.mark_start = before;
                MarkRPr
            }
            (PPr, _) => cap(self, Cap::PPrChild),
            (MarkRPr, _) => cap(self, Cap::MarkChild),
            (P | Hyperlink | Rev | Simple, "r") => {
                // Namespaces declared on the run itself: its objects need
                // them (Kantor writes a plain `<w:r>`).
                self.run_ns = e
                    .attributes()
                    .flatten()
                    .filter(|a| a.key.as_ref().starts_with(b"xmlns"))
                    .map(|a| {
                        format!(
                            " {}=\"{}\"",
                            String::from_utf8_lossy(a.key.as_ref()),
                            String::from_utf8_lossy(&a.value)
                        )
                    })
                    .collect();
                self.run = RunProps::default();
                self.run_start = before;
                self.run_text_start = self.para.text.len();
                self.run_pushed = false;
                self.run_fld = None;
                R
            }
            (P | Hyperlink | Rev | Simple, "fldSimple") if !self.capturing() => {
                let instr = attr(e, b"instr").unwrap_or_default();
                if page_number_field(&instr) {
                    return cap(self, Cap::ParaObj);
                }
                let code = &self.src[before..after];
                let begin = FieldMark::Begin {
                    instr,
                    code: code.into(),
                    sep: None,
                    simple: true,
                };
                let at = self.para.text.len();
                self.insert_mark(at, begin, RunProps::default());
                if empty {
                    let end = FieldMark::End {
                        xml: "".into(),
                        simple: true,
                    };
                    self.insert_mark(at + OBJECT.len_utf8(), end, RunProps::default());
                    return Skip;
                }
                Simple
            }
            (P | Simple, "hyperlink") => {
                self.link = Some(self.src[before..after].into());
                self.link_start = before;
                self.link_runs = 0;
                if empty {
                    // An empty hyperlink: keep it as is.
                    self.link = None;
                    let xml = &self.src[before..after];
                    let o = Self::opaque(Level::Para, xml, "");
                    self.push_object(o, RunProps::default());
                    return Skip;
                }
                Hyperlink
            }
            (P | Hyperlink | Simple, "ins" | "del" | "moveTo" | "moveFrom") if !empty => {
                match Revision::parse(&self.src[before..after]) {
                    Some(r) => {
                        self.rev = Some(Arc::new(r));
                        Rev
                    }
                    None => cap(self, Cap::ParaObj),
                }
            }
            (P | Hyperlink | Rev | Simple, "proofErr") => Skip,
            (
                P | Hyperlink | Rev | Simple,
                "bookmarkStart" | "bookmarkEnd" | "permStart" | "permEnd",
            )
            | (P | Hyperlink | Rev | Simple, "commentRangeStart" | "commentRangeEnd")
            | (P | Hyperlink | Rev | Simple, "moveFromRangeStart" | "moveFromRangeEnd")
            | (P | Hyperlink | Rev | Simple, "moveToRangeStart" | "moveToRangeEnd") => {
                if empty {
                    let xml = &self.src[before..after];
                    self.marker(local, e, xml, true);
                    Skip
                } else {
                    cap(self, Cap::Marker)
                }
            }
            (P | Hyperlink | Simple, _) => cap(self, Cap::ParaObj),
            (Rev, _) => {
                // Anything but runs inside a tracked change: kept, but the
                // change can't be resolved around it.
                self.unsupported.insert("tracked changes".into());
                cap(self, Cap::ParaObj)
            }
            (R, "rPr") => RPr,
            (RPr, _) => cap(self, Cap::RPrChild),
            (R, "t" | "delText") => T,
            (R, "tab") => {
                self.push_text("\t");
                Skip
            }
            (R, "br" | "cr") => {
                match attr(e, b"type").as_deref() {
                    None | Some("textWrapping") => self.push_char(LINE_BREAK),
                    Some("page") => self.push_char(PAGE_BREAK),
                    Some(_) => {
                        // Column breaks: kept as an opaque run element.
                        if empty {
                            let xml = self.src[before..after].to_owned();
                            let o = Self::opaque(Level::Run, &xml, "Column break");
                            let props = self.run.clone();
                            self.push_object(o, props);
                        }
                    }
                }
                Skip
            }
            (R, "softHyphen") => {
                self.push_char('\u{AD}');
                Skip
            }
            (R, "noBreakHyphen") => {
                self.push_char('\u{2011}');
                Skip
            }
            (R, "lastRenderedPageBreak") => Skip,
            (R, "fldChar") => {
                self.fld_char(attr(e, b"fldCharType").as_deref(), stack_len);
                Skip
            }
            (R, "instrText" | "delInstrText") => Instr,
            (R, _) => cap(self, Cap::RunObj),
            _ => Skip,
        }
    }

    /// A table of contents' content control starting at byte `start`
    /// (Word's `docPartGallery` "Table of Contents") holding only
    /// paragraphs: where its content starts and ends.
    fn toc_sdt(&self, start: usize) -> Option<(usize, usize)> {
        let src = &self.src[start..];
        let mut r = quick_xml::Reader::from_str(src);
        let mut depth = 0usize;
        let mut content: Option<usize> = None;
        let mut content_end = None;
        let end;
        loop {
            let before = r.buffer_position() as usize;
            let ev = r.read_event().ok()?;
            let after = r.buffer_position() as usize;
            match ev {
                Event::Start(ref e) | Event::Empty(ref e) => {
                    let empty = matches!(ev, Event::Empty(_));
                    let name = e.local_name();
                    let name = name.as_ref();
                    if depth == 1 && name == b"sdtContent" && !empty {
                        content = Some(start + after);
                    } else if depth == 2
                        && content.is_some()
                        && content_end.is_none()
                        && !matches!(name, b"p" | b"bookmarkStart" | b"bookmarkEnd")
                    {
                        return None;
                    }
                    if !empty {
                        depth += 1;
                    }
                }
                Event::End(ref e) => {
                    depth = depth.checked_sub(1)?;
                    if depth == 1 && e.local_name().as_ref() == b"sdtContent" {
                        content_end = Some(start + before);
                    }
                    if depth == 0 {
                        end = after;
                        break;
                    }
                }
                Event::Eof => return None,
                _ => {}
            }
        }
        let (content, content_end) = (content?, content_end?);
        // The control without its content: is it a table of contents?
        let shell = format!(
            "{}{}",
            &self.src[start..content],
            &self.src[content_end..start + end]
        );
        let el = xml::parse(&shell)?;
        let gallery = find(&el, "docPartGallery")?.attr("val")?;
        gallery
            .to_ascii_lowercase()
            .starts_with("table of contents")
            .then_some((content, content_end))
    }

    /// Where the parser is: stack depth, hyperlink, tracked change.
    fn container(&self, stack_len: usize) -> (usize, Option<usize>, Option<*const Revision>) {
        (
            stack_len,
            self.link.as_ref().map(|_| self.link_start),
            self.rev.as_ref().map(Arc::as_ptr),
        )
    }

    /// Put a field mark into the paragraph at byte `at` (in the current
    /// hyperlink and tracked change).
    fn insert_mark(&mut self, at: usize, mark: FieldMark, props: RunProps) {
        self.link_runs += 1;
        let run = Run {
            object: Some(Object::Field(Arc::new(mark))),
            link: self.link.clone(),
            rev: self.rev.clone(),
            ..Run::text(OBJECT.len_utf8(), props)
        };
        self.para.insert_runs(at, vec![run]);
    }

    fn fld_char(&mut self, kind: Option<&str>, stack_len: usize) {
        match kind {
            Some("begin") => {
                if self.capturing() {
                    if let Some(f) = self.fields.last_mut() {
                        f.depth += 1;
                    }
                    return;
                }
                // Text this run already added: the field's XML holds it
                // (an unusual layout: keep the field whole).
                let len = self.para.text.len();
                let mode = if self.run_text_start < len {
                    self.para.delete(self.run_text_start..len);
                    FieldMode::Opaque
                } else {
                    FieldMode::Code
                };
                self.fields.push(Field {
                    start: self.run_start,
                    depth: 1,
                    mode,
                    in_result: false,
                    instr: String::new(),
                    result: String::new(),
                    // The run's parent level (the run frame isn't counted).
                    level: stack_len.saturating_sub(1),
                    container: self.container(stack_len.saturating_sub(1)),
                    props: self.run.clone(),
                    ended: false,
                });
                self.run_fld = Some(RunFld::Begin);
            }
            Some("separate") => {
                let here = self.container(stack_len.saturating_sub(1));
                let at = self.para.text.len();
                let pushed = self.run_pushed;
                let Some(f) = self.fields.last_mut() else {
                    return;
                };
                if f.depth != 1 {
                    return;
                }
                match f.mode {
                    FieldMode::Code => {
                        if page_number_field(&f.instr) || f.container != here || pushed {
                            f.mode = FieldMode::Opaque;
                            f.in_result = true;
                        } else {
                            f.mode = FieldMode::Result;
                            self.run_fld = Some(RunFld::Separate(at));
                        }
                    }
                    FieldMode::Opaque => f.in_result = true,
                    FieldMode::Result => {}
                }
            }
            Some("end") => {
                let at = self.para.text.len();
                let Some(f) = self.fields.last_mut() else {
                    return;
                };
                match f.mode {
                    FieldMode::Result => {
                        f.ended = true;
                        f.depth = 0;
                        self.run_fld = Some(RunFld::End(at));
                    }
                    FieldMode::Code | FieldMode::Opaque => {
                        f.depth = f.depth.saturating_sub(1);
                        if f.depth == 0 {
                            f.ended = true;
                            if f.mode == FieldMode::Code && page_number_field(&f.instr) {
                                f.mode = FieldMode::Opaque;
                            }
                            self.run_fld = Some(RunFld::End(at));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// A run holding a `w:fldChar` closed (`run` is its source range).
    fn fld_run_closed(
        &mut self,
        fld: RunFld,
        (run_start, after): (usize, usize),
        stack_len: usize,
    ) {
        let clean = !self.run_pushed;
        match fld {
            RunFld::Begin => {}
            RunFld::Separate(at) => {
                let Some(f) = self.fields.last() else { return };
                let (code, sep) = if clean {
                    (
                        self.src[f.start..run_start].to_owned(),
                        self.src[run_start..after].to_owned(),
                    )
                } else {
                    // Result text in the separator's run: write the
                    // separator on its own.
                    (
                        self.src[f.start..run_start].to_owned(),
                        crate::object::FIELD_SEPARATE.to_owned(),
                    )
                };
                let mark = FieldMark::Begin {
                    instr: f.instr.clone(),
                    code: code.into(),
                    sep: Some(sep.into()),
                    simple: false,
                };
                let props = f.props.clone();
                self.insert_mark(at, mark, props);
            }
            RunFld::End(at) => {
                let Some(f) = self.fields.pop() else { return };
                match f.mode {
                    FieldMode::Result => {
                        let xml = if clean {
                            self.src[run_start..after].to_owned()
                        } else {
                            crate::object::FIELD_END.to_owned()
                        };
                        let mark = FieldMark::End {
                            xml: xml.into(),
                            simple: false,
                        };
                        self.insert_mark(at, mark, self.run.clone());
                    }
                    FieldMode::Code if f.container == self.container(stack_len) => {
                        // No result: the code, then the end.
                        let begin = FieldMark::Begin {
                            instr: f.instr.clone(),
                            code: self.src[f.start..run_start].into(),
                            sep: None,
                            simple: false,
                        };
                        let end = FieldMark::End {
                            xml: if clean {
                                self.src[run_start..after].into()
                            } else {
                                crate::object::FIELD_END.into()
                            },
                            simple: false,
                        };
                        self.insert_mark(at, end, self.run.clone());
                        self.insert_mark(at, begin, f.props.clone());
                    }
                    FieldMode::Code | FieldMode::Opaque => {
                        if f.level == stack_len {
                            let xml = &self.src[f.start..after];
                            if !page_number_field(&f.instr) {
                                self.preserved.insert("fields".into());
                            }
                            let o =
                                Self::opaque(Level::Para, xml, field_label(&f.instr, &f.result));
                            self.push_object(o, RunProps::default());
                        } else {
                            self.unsupported
                                .insert("fields across hyperlinks or other elements".into());
                        }
                    }
                }
            }
        }
    }

    /// A captured element closed: `xml` is its full source.
    fn captured(&mut self, cap: Cap, xml: &str) {
        match cap {
            Cap::RunObj => {
                let with_ns = with_namespaces(xml, &self.run_ns);
                let xml = with_ns.as_str();
                let local = xml::parse(xml).map(|e| e.name).unwrap_or_default();
                if is_tracked(&local) {
                    self.unsupported.insert(category(&local));
                }
                let obj = match local.as_str() {
                    "drawing" => self.image(xml).or_else(|| self.float(xml)),
                    "AlternateContent" => self.float(xml),
                    _ => None,
                };
                let obj = obj.unwrap_or_else(|| {
                    let label = self.run_label(&local, xml);
                    Self::opaque(Level::Run, xml, label)
                });
                let props = self.run.clone();
                self.push_object(obj, props);
            }
            Cap::ParaObj => {
                let local = xml::parse(xml).map(|e| e.name).unwrap_or_default();
                let label = self.para_label(&local, xml);
                let o = Self::opaque(Level::Para, xml, label);
                self.push_object(o, RunProps::default());
            }
            Cap::Marker => {
                let local = xml::parse(xml).map(|e| e.name).unwrap_or_default();
                if local.starts_with("comment") || is_tracked(&local) {
                    self.unsupported.insert(category(&local));
                }
                let o = Self::opaque(Level::Marker, xml, "");
                if matches!(
                    self.frame_top,
                    Some(Frame::P | Frame::Hyperlink | Frame::Simple)
                ) {
                    self.push_object(o, RunProps::default());
                } else {
                    self.pending.push(o);
                }
            }
            Cap::BlockObj => {
                let local = xml::parse(xml).map(|e| e.name).unwrap_or_default();
                let label = self.block_label(&local, xml);
                let o = Self::opaque(Level::Block, xml, label);
                self.push_block(o);
            }
            Cap::PPrChild => {
                let mut props = std::mem::take(&mut self.para.props);
                match para_prop(xml, &mut props) {
                    Prop::Modelled => {}
                    Prop::Keep => props.extras.push(xml),
                    Prop::Unsupported(w) => {
                        self.unsupported.insert(w);
                        props.extras.push(xml);
                    }
                }
                self.para.props = props;
            }
            Cap::RPrChild | Cap::MarkChild => {
                let mut props = if matches!(cap, Cap::RPrChild) {
                    std::mem::take(&mut self.run)
                } else {
                    std::mem::take(&mut self.mark)
                };
                match run_prop(xml, &mut props) {
                    Prop::Modelled => {}
                    Prop::Keep => props.extras.push(xml),
                    Prop::Unsupported(w) => {
                        self.unsupported.insert(w);
                        props.extras.push(xml);
                    }
                }
                if matches!(cap, Cap::RPrChild) {
                    self.run = props;
                } else {
                    self.mark = props;
                }
            }
            Cap::TblPr => {
                if let Some((_, t)) = &mut self.table {
                    t.tbl_pr = Some(xml.into());
                    // Tracked property changes: crate::track_table.
                    t.refresh();
                }
            }
            Cap::TblGrid => {
                if let Some((_, t)) = &mut self.table
                    && let Some(el) = xml::parse(xml)
                {
                    t.grid = el
                        .children
                        .iter()
                        .filter(|c| c.name == "gridCol")
                        .map(|c| c.num("w").unwrap_or(0))
                        .collect();
                    if let Some(r) = crate::float::element_range(xml, "tblGridChange") {
                        t.grid_change = Some(xml[r].into());
                    }
                }
            }
            Cap::TrPr | Cap::TblPrEx => {
                let el = xml::parse(xml);
                if let Some((_, t)) = &mut self.table
                    && let Some(row) = t.rows.last_mut()
                {
                    if matches!(cap, Cap::TblPrEx) {
                        row.tbl_pr_ex = Some(xml.into());
                    } else {
                        row.tr_pr = Some(xml.into());
                        if let Some(el) = &el {
                            if let Some(h) = el.child("trHeight") {
                                let exact = h.attr("hRule") == Some("exact");
                                row.height = h.num("val").map(|v| (v, exact));
                            }
                            row.header = el.child("tblHeader").is_some_and(El::on);
                            row.cant_split = el.child("cantSplit").is_some_and(El::on);
                            // Inserted and deleted rows and row property
                            // changes are modelled (crate::track_table).
                            if el.children.iter().any(|c| {
                                is_tracked(&c.name) && !crate::track_table::is_row_mark(&c.name)
                            }) {
                                self.unsupported.insert("tracked changes".into());
                            }
                        }
                    }
                }
            }
            Cap::TcPr => {
                let el = xml::parse(xml);
                if let Some((_, t)) = &mut self.table
                    && let Some(cell) = t.rows.last_mut().and_then(|r| r.cells.last_mut())
                {
                    cell.tc_pr = Some(xml.into());
                    if let Some(el) = &el {
                        for c in &el.children {
                            match c.name.as_str() {
                                "gridSpan" => cell.span = c.num("val").unwrap_or(1).max(1),
                                "vMerge" => {
                                    cell.vmerge = if c.attr("val") == Some("restart") {
                                        VMerge::Restart
                                    } else {
                                        VMerge::Continue
                                    }
                                }
                                "tcW" if matches!(c.attr("type"), None | Some("dxa")) => {
                                    cell.width = c.num("w");
                                }
                                "tcBorders" => cell.borders = borders(c),
                                "hMerge" => {
                                    self.preserved.insert("horizontally merged cells".into());
                                }
                                // Inserted and deleted cells and property
                                // changes: crate::track_table.
                                "cellMerge" => {
                                    self.unsupported.insert("tracked changes".into());
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            Cap::BodySect => {
                self.sect_pr = Some(xml.to_owned());
                if let Some(el) = xml::parse(xml) {
                    for c in &el.children {
                        match c.name.as_str() {
                            "pgSz" => {
                                self.page.width = c.num("w").unwrap_or(self.page.width);
                                self.page.height = c.num("h").unwrap_or(self.page.height);
                            }
                            "pgMar" => {
                                let n = |k: &str| c.num::<i64>(k).map(|v| v.unsigned_abs() as u32);
                                let pg = &mut self.page;
                                pg.margin_top = n("top").unwrap_or(pg.margin_top);
                                pg.margin_bottom = n("bottom").unwrap_or(pg.margin_bottom);
                                pg.margin_left = n("left").unwrap_or(pg.margin_left);
                                pg.margin_right = n("right").unwrap_or(pg.margin_right);
                            }
                            "sectPrChange" => {
                                self.unsupported.insert("tracked changes".into());
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    fn close(&mut self, frame: Frame, (before, after): (usize, usize), stack_len: usize) {
        match frame {
            Frame::Capture => {
                if let Some((start, cap)) = self.caps.pop() {
                    let xml = &self.src[start..after];
                    self.captured(cap, xml);
                }
            }
            Frame::MarkRPr => {
                self.mark_raw = Some(self.src[self.mark_start..after].to_owned());
            }
            Frame::R => {
                if let Some(fld) = self.run_fld.take() {
                    self.fld_run_closed(fld, (self.run_start, after), stack_len);
                } else if !self.capturing() {
                    self.link_runs += 1;
                }
            }
            Frame::Simple => {
                let end = FieldMark::End {
                    xml: self.src[before..after].into(),
                    simple: true,
                };
                let at = self.para.text.len();
                self.insert_mark(at, end, RunProps::default());
            }
            Frame::Body if self.frame_top == Some(Frame::Wrap) => {
                self.wraps.push(before);
            }
            Frame::Wrap => {
                let start = self.wraps.pop().unwrap_or(before);
                let o = Self::opaque(Level::Wrap, &self.src[start..after], "");
                let pending = std::mem::take(&mut self.pending);
                match self.paragraphs.last_mut() {
                    Some(last) => {
                        for m in pending.into_iter().chain([o]) {
                            last.push_object(m, RunProps::default(), None);
                        }
                    }
                    None => {
                        self.pending = pending;
                        self.pending.push(o);
                    }
                }
            }
            Frame::Rev => self.rev = None,
            Frame::Hyperlink => {
                if self.link_runs == 0 && !self.capturing() {
                    let xml = self.src[self.link_start..after].to_owned();
                    self.link = None;
                    let o =
                        Self::opaque(Level::Para, &xml, short(&xml::inner_text(&xml, &["t"]), 40));
                    self.push_object(o, RunProps::default());
                }
                self.link = None;
            }
            Frame::P => {
                // A field whose code (or whole opaque field) runs on into
                // the next paragraph: Kantor can't keep it.
                while let Some(f) = self.fields.pop_if(|f| f.mode != FieldMode::Result) {
                    self.unsupported.insert("fields spanning paragraphs".into());
                    // Keep what Word shows of it here as text.
                    if f.in_result && !f.result.is_empty() {
                        self.para.push_tracked(
                            &f.result,
                            self.run.clone(),
                            self.link.clone(),
                            self.rev.clone(),
                        );
                    }
                }
                let mut p = std::mem::take(&mut self.para);
                if p.text.is_empty() {
                    p.runs[0].props = std::mem::take(&mut self.mark);
                } else if let Some(raw) = self.mark_raw.take() {
                    p.props.extras.push(&raw);
                }
                p.normalize();
                self.paragraphs.push(p);
                self.cell_paras += 1;
            }
            Frame::Tc => {
                if self.cell_paras == 0 {
                    let mut p = Paragraph {
                        cell: self.cell(),
                        ..Paragraph::default()
                    };
                    for m in self.pending.drain(..) {
                        p.push_object(m, RunProps::default(), None);
                    }
                    self.paragraphs.push(p);
                }
            }
            Frame::Tbl => {
                if let Some((id, t)) = self.table.take() {
                    self.tables.insert(id, t);
                }
            }
            _ => {}
        }
    }
}

/// `xml` with namespace declarations `ns` (` xmlns:a="…"`…) added to its
/// first start tag, skipping prefixes it declares itself.
fn with_namespaces(xml: &str, ns: &str) -> String {
    if ns.is_empty() {
        return xml.to_owned();
    }
    let Some(name_end) = xml.find(|c: char| c.is_whitespace() || c == '>' || c == '/') else {
        return xml.to_owned();
    };
    let tag_end = xml.find('>').unwrap_or(xml.len());
    let tag = &xml[..tag_end];
    let mut add = String::new();
    for decl in ns.split(" xmlns").filter(|d| !d.trim().is_empty()) {
        let key = decl.split('=').next().unwrap_or("");
        if !tag.contains(&format!(" xmlns{key}=")) {
            add.push_str(" xmlns");
            add.push_str(decl.trim_end());
        }
    }
    format!("{}{}{}", &xml[..name_end], add, &xml[name_end..])
}

/// First descendant with local name `name`.
fn find<'e>(el: &'e El, name: &str) -> Option<&'e El> {
    for c in &el.children {
        if c.name == name {
            return Some(c);
        }
        if let Some(f) = find(c, name) {
            return Some(f);
        }
    }
    None
}

/// Parsed body plus what the package needs.
struct Body {
    paragraphs: Vec<Paragraph>,
    tables: Tables,
    unsupported: BTreeSet<String>,
    preserved: BTreeSet<String>,
    page: PageGeometry,
    root_tag: Option<String>,
    sect_pr: Option<String>,
    image_rels: BTreeSet<String>,
}

fn parse_body(src: &str, rels: &HashMap<String, Rel>, document_part: &str) -> anyhow::Result<Body> {
    let mut reader = NsReader::from_str(src);
    let mut st = BodyParser {
        src,
        rels,
        document_part,
        paragraphs: Vec::new(),
        tables: Tables::new(),
        unsupported: BTreeSet::new(),
        preserved: BTreeSet::new(),
        page: PageGeometry::default(),
        root_tag: None,
        sect_pr: None,
        para: Paragraph::default(),
        mark: RunProps::default(),
        mark_raw: None,
        mark_start: 0,
        run: RunProps::default(),
        run_start: 0,
        run_text_start: 0,
        link: None,
        link_start: 0,
        link_runs: 0,
        rev: None,
        fields: Vec::new(),
        run_fld: None,
        run_pushed: false,
        wraps: Vec::new(),
        caps: Vec::new(),
        pending: Vec::new(),
        go_back: HashSet::new(),
        table: None,
        next_table: 0,
        cell_paras: 0,
        image_rels: BTreeSet::new(),
        frame_top: None,
        run_ns: String::new(),
    };
    let mut stack: Vec<Frame> = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader.read_event().context("document.xml")?;
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let empty = matches!(event, Event::Empty(_));
                let (ns, local) = reader.resolver().resolve_element(e.name());
                let is_w = matches!(ns, ResolveResult::Bound(Namespace(n)) if n == W);
                let local = String::from_utf8_lossy(local.as_ref()).into_owned();
                let frame = match stack.last().copied() {
                    None => {
                        let story = is_w && (local == "hdr" || local == "ftr");
                        if !(is_w && local == "document") && !story {
                            bail!("not a WordprocessingML document");
                        }
                        let tag = &src[before..after];
                        if !tag.contains(W_DECL) {
                            // Body XML is written with the usual `w:` prefix.
                            st.unsupported
                                .insert("unusual XML namespace prefixes".into());
                        }
                        st.root_tag = Some(tag.to_owned());
                        // A header's or footer's content sits right in its root.
                        if story { Frame::Body } else { Frame::Document }
                    }
                    Some(Frame::Skip) => Frame::Skip,
                    Some(Frame::Capture) => Frame::Skip,
                    Some(top) => {
                        st.frame_top = Some(top);
                        st.open(top, &local, is_w, e, (before, after), stack.len(), empty)
                    }
                };
                if empty {
                    st.frame_top = stack.last().copied();
                    st.close(frame, (before, after), stack.len());
                } else {
                    stack.push(frame);
                }
            }
            Event::End(_) => {
                if let Some(frame) = stack.pop() {
                    st.frame_top = stack.last().copied();
                    st.close(frame, (before, after), stack.len());
                }
            }
            Event::Text(t) if matches!(stack.last(), Some(Frame::T | Frame::Instr)) => {
                let s = t.xml10_content().context("text")?;
                text_event(&mut st, stack.last() == Some(&Frame::Instr), &s);
            }
            Event::GeneralRef(r) if matches!(stack.last(), Some(Frame::T | Frame::Instr)) => {
                let s = match r.resolve_char_ref()? {
                    Some(c) => c.to_string(),
                    None => match &*r as &[u8] {
                        b"amp" => "&".into(),
                        b"lt" => "<".into(),
                        b"gt" => ">".into(),
                        b"quot" => "\"".into(),
                        b"apos" => "'".into(),
                        _ => String::new(),
                    },
                };
                text_event(&mut st, stack.last() == Some(&Frame::Instr), &s);
            }
            Event::CData(c) if matches!(stack.last(), Some(Frame::T | Frame::Instr)) => {
                let s = String::from_utf8_lossy(&c).into_owned();
                text_event(&mut st, stack.last() == Some(&Frame::Instr), &s);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if st.root_tag.is_none() {
        bail!("empty document part");
    }
    if !st.fields.is_empty() {
        st.unsupported.insert("fields without an end".into());
    }
    // Markers after the last paragraph go to its end.
    if !st.pending.is_empty() {
        if st.paragraphs.is_empty() {
            st.paragraphs.push(Paragraph::default());
        }
        let last = st.paragraphs.last_mut().expect("non-empty");
        for m in std::mem::take(&mut st.pending) {
            last.push_object(m, RunProps::default(), None);
        }
    }
    Ok(Body {
        paragraphs: st.paragraphs,
        tables: st.tables,
        unsupported: st.unsupported,
        preserved: st.preserved,
        page: st.page,
        root_tag: st.root_tag,
        sect_pr: st.sect_pr,
        image_rels: st.image_rels,
    })
}

fn text_event(st: &mut BodyParser, instr: bool, s: &str) {
    if instr {
        if let Some(f) = st.fields.last_mut()
            && f.depth == 1
            && f.mode != FieldMode::Result
        {
            f.instr.push_str(s);
        }
    } else {
        st.push_text(s);
    }
}

// ── styles, theme, settings ───────────────────────────────────────────────

/// Read `styles.xml` (defaults, paragraph, character and table styles).
fn parse_styles(src: &str, sheet: &mut StyleSheet) {
    #[derive(PartialEq, Clone, Copy)]
    enum In {
        Other,
        RPrDefault,
        PPrDefault,
        DefRPr,
        DefPPr,
        Style,
        StyleRPr,
        StylePPr,
        /// A property child being captured; `.0` says where it goes.
        Child(u8, usize),
    }
    let mut r = quick_xml::Reader::from_str(src);
    let mut stack: Vec<In> = Vec::new();
    let mut style = Style::default();
    // Proofing properties (`w:lang`, `w:noProof`) aren't modelled, but the
    // spell checker reads them from the extras (see `StyleSheet::proofing`).
    let run = |xml: &str, p: &mut RunProps| {
        if matches!(run_prop(xml, p), Prop::Keep) && matches!(element_name(xml), "lang" | "noProof")
        {
            p.extras.push(xml);
        }
    };
    // Tab stops aren't modelled either; layout reads them from the extras
    // (see `Document::tab_stops`).
    let para = |xml: &str, p: &mut ParaProps| {
        if matches!(para_prop(xml, p), Prop::Keep) && element_name(xml) == "tabs" {
            p.extras.push(xml);
        }
    };
    let apply = |target: u8, xml: &str, sheet: &mut StyleSheet, style: &mut Style| match target {
        0 => run(xml, &mut sheet.default_run),
        1 => para(xml, &mut sheet.default_para),
        2 => run(xml, &mut style.run),
        3 => para(xml, &mut style.para),
        _ => {
            // A table style's w:tblPr.
            if let Some(el) = xml::parse(xml) {
                let t = style.table.get_or_insert_with(TableStyle::default);
                for c in &el.children {
                    match c.name.as_str() {
                        "tblBorders" => t.borders = borders(c),
                        "tblCellMar" => t.cell_margins = Some(cell_margins(c)),
                        "jc" => {
                            t.align = c.attr("val").and_then(crate::table::TableAlign::from_jc);
                        }
                        _ => {}
                    }
                }
            }
        }
    };
    loop {
        let before = r.buffer_position() as usize;
        let event = match r.read_event() {
            Ok(e) => e,
            Err(err) => {
                log::warn!("styles.xml: {err}");
                break;
            }
        };
        let after = r.buffer_position() as usize;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let empty = matches!(event, Event::Empty(_));
                let top = stack.last().copied().unwrap_or(In::Other);
                let local = e.local_name();
                let child_of = |t: u8| In::Child(t, before);
                let next = match (top, local.as_ref()) {
                    (In::Child(..), _) => In::Other,
                    (_, b"rPrDefault") => In::RPrDefault,
                    (_, b"pPrDefault") => In::PPrDefault,
                    (In::RPrDefault, b"rPr") => In::DefRPr,
                    (In::PPrDefault, b"pPr") => In::DefPPr,
                    (In::DefRPr, _) => child_of(0),
                    (In::DefPPr, _) => child_of(1),
                    (In::Other, b"style") => {
                        style = Style {
                            id: attr(e, b"styleId").unwrap_or_default(),
                            kind: match attr(e, b"type").as_deref() {
                                Some("character") => StyleKind::Character,
                                Some("table") => StyleKind::Table,
                                Some("numbering") => StyleKind::Numbering,
                                _ => StyleKind::Paragraph,
                            },
                            is_default: matches!(
                                attr(e, b"default").as_deref(),
                                Some("1" | "true")
                            ),
                            ..Style::default()
                        };
                        In::Style
                    }
                    (In::Style, b"name") => {
                        style.name = attr(e, b"val").unwrap_or_default();
                        In::Other
                    }
                    (In::Style, b"basedOn") => {
                        style.based_on = attr(e, b"val");
                        In::Other
                    }
                    (In::Style, b"next") => {
                        style.next = attr(e, b"val");
                        In::Other
                    }
                    (In::Style, b"rPr") => In::StyleRPr,
                    (In::Style, b"pPr") => In::StylePPr,
                    (In::Style, b"tblPr") => child_of(4),
                    (In::StyleRPr, _) => child_of(2),
                    (In::StylePPr, _) => child_of(3),
                    _ => In::Other,
                };
                if empty {
                    match next {
                        In::Style => sheet.styles.push(std::mem::take(&mut style)),
                        In::Child(t, start) => apply(t, &src[start..after], sheet, &mut style),
                        _ => {}
                    }
                } else {
                    stack.push(next);
                }
            }
            Event::End(_) => match stack.pop() {
                Some(In::Style) => sheet.styles.push(std::mem::take(&mut style)),
                Some(In::Child(t, start)) => apply(t, &src[start..after], sheet, &mut style),
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    // A style's paragraph `style` field is meaningless; keep ids out of it.
    for s in &mut sheet.styles {
        s.para.style = None;
    }
}

/// Theme fonts: `(major, minor)` Latin typefaces.
fn parse_theme_fonts(src: &str) -> (Option<String>, Option<String>) {
    let mut r = quick_xml::Reader::from_str(src);
    let (mut major, mut minor) = (None, None);
    let mut slot = 0; // 1 = majorFont, 2 = minorFont
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                b"majorFont" => slot = 1,
                b"minorFont" => slot = 2,
                _ => {}
            },
            Ok(Event::Empty(e)) if e.local_name().as_ref() == b"latin" => {
                let face = attr(&e, b"typeface").filter(|f| !f.is_empty());
                match slot {
                    1 if major.is_none() => major = face,
                    2 if minor.is_none() => minor = face,
                    _ => {}
                }
            }
            Ok(Event::End(e)) if matches!(e.local_name().as_ref(), b"majorFont" | b"minorFont") => {
                slot = 0;
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    (major, minor)
}

pub(crate) fn track_changes_on(settings: &str) -> bool {
    let mut r = quick_xml::Reader::from_str(settings);
    loop {
        match r.read_event() {
            Ok(Event::Start(e) | Event::Empty(e))
                if e.local_name().as_ref() == b"trackRevisions" =>
            {
                return !matches!(attr(&e, b"val").as_deref(), Some("0" | "false" | "off"));
            }
            Ok(Event::Eof) | Err(_) => return false,
            _ => {}
        }
    }
}

/// Read a `.docx` package.
pub fn read(bytes: Vec<u8>) -> anyhow::Result<Read> {
    // Strict Open XML is read, and kept for saving, as Transitional.
    let bytes = Arc::new(kantor_ooxml::transitional(bytes)?);
    let mut zip =
        zip::ZipArchive::new(Cursor::new(bytes.as_slice())).context("not a .docx (zip) file")?;
    let root_rels = parse_rels(&zip_text(&mut zip, "_rels/.rels")?.unwrap_or_default());
    let document_part = rel_target(&root_rels, "/officeDocument")
        .map(|t| resolve("", &t))
        .unwrap_or_else(|| "word/document.xml".to_owned());
    let doc_xml =
        zip_text(&mut zip, &document_part)?.with_context(|| format!("missing {document_part}"))?;
    let doc_rels = parse_rels(&zip_text(&mut zip, &rels_name(&document_part))?.unwrap_or_default());
    let part = |kind: &str| rel_target(&doc_rels, kind).map(|t| resolve(&document_part, &t));

    let mut styles = StyleSheet::default();
    if let Some(name) = part("/styles")
        && let Some(src) = zip_text(&mut zip, &name)?
    {
        parse_styles(&src, &mut styles);
    }
    if let Some(name) = part("/theme")
        && let Some(src) = zip_text(&mut zip, &name)?
    {
        let (major, minor) = parse_theme_fonts(&src);
        styles.major_font = major.unwrap_or(styles.major_font);
        styles.minor_font = minor.unwrap_or(styles.minor_font);
    }
    let mut numbering_part = None;
    let mut numbering = Numbering::default();
    if let Some(name) = part("/numbering")
        && let Some(src) = zip_text(&mut zip, &name)?
    {
        numbering = Numbering::parse(&src);
        numbering_part = Some((name, src));
    }

    let rels: HashMap<String, Rel> = doc_rels.iter().map(|r| (r.id.clone(), r.clone())).collect();
    let body = parse_body(&doc_xml, &rels, &document_part)?;
    let unsupported = body.unsupported;
    let preserved = body.preserved;
    let mut settings = None;
    let mut even_odd = false;
    let mut default_tab = 720;
    let mut decimal = '.';
    let mut track = false;
    let mut note_settings = NoteKind::ALL.map(NoteProps::default_for);
    if let Some(name) = part("/settings")
        && let Some(src) = zip_text(&mut zip, &name)?
    {
        track = track_changes_on(&src);
        note_settings = crate::notes::settings_note_props(&src);
        even_odd = settings_even_odd(&src);
        default_tab = crate::tabs::settings_default_tab(&src).unwrap_or(720);
        decimal = crate::tabs::settings_decimal(&src).unwrap_or('.');
        settings = Some((name, src));
    }
    let mut comments = Vec::new();
    let mut comments_part = None;
    if let Some(name) = part("/comments")
        && let Some(src) = zip_text(&mut zip, &name)?
    {
        let (cs, root) = crate::review::parse_comments(&src);
        comments = cs;
        comments_part = Some(crate::review::CommentsPart {
            name,
            root_tag: root,
        });
    }
    let mut thread_srcs = Vec::new();
    for kind in crate::comment_thread::Extra::ALL {
        if let Some(name) = part(kind.rel_suffix())
            && let Some(src) = zip_text(&mut zip, &name)?
        {
            thread_srcs.push((kind, name, src));
        }
    }
    let comment_threads = crate::comment_thread::read(&mut comments, thread_srcs);
    let mut notes = Vec::new();
    let mut notes_parts: [Option<NotesPart>; 2] = [None, None];
    for (k, kind) in NoteKind::ALL.into_iter().enumerate() {
        let Some(name) = part(&format!("/{}", kind.root())) else {
            continue;
        };
        let Some(src) = zip_text(&mut zip, &name)? else {
            continue;
        };
        let (read, root_tag) = read_notes(&mut zip, kind, &name, &src, &styles, &numbering)?;
        notes.extend(read);
        notes_parts[k] = Some(NotesPart { name, root_tag });
    }
    let mut stories = Vec::new();
    for r in &doc_rels {
        let kind = if r.ty.ends_with("/header") {
            HfKind::Header
        } else if r.ty.ends_with("/footer") {
            HfKind::Footer
        } else {
            continue;
        };
        if r.external {
            continue;
        }
        let name = resolve(&document_part, &r.target);
        let Some(src) = zip_text(&mut zip, &name)? else {
            continue;
        };
        stories.push(read_story(
            &mut zip, kind, &r.id, &name, &src, &styles, &numbering,
        )?);
    }
    let mut media = HashMap::new();
    for id in &body.image_rels {
        let Some(rel) = rels.get(id) else { continue };
        let part = resolve(&document_part, &rel.target);
        let data = zip_bytes(&mut zip, &part)?;
        let drawable = data.as_deref().map(ImageFormat::sniff);
        media.insert(
            id.clone(),
            Media {
                part,
                bytes: data
                    .filter(|_| matches!(drawable, Some(ImageFormat::Png | ImageFormat::Jpeg)))
                    .map(Into::into),
            },
        );
    }
    let package = Package {
        bytes: bytes.clone(),
        document_part,
        root_tag: body.root_tag.unwrap_or_default(),
        sect_pr: body.sect_pr,
        numbering: numbering_part,
        settings,
        comments: comments_part,
        comment_threads,
        notes: notes_parts,
        notes_read: notes
            .iter()
            .map(|n: &Note| (n.kind, n.id.clone(), n.xml.clone()))
            .collect(),
        rel_ids: rels.keys().cloned().collect(),
        parts: (0..zip.len())
            .filter_map(|i| {
                zip.name_for_index(i)
                    .and_then(|n| n.ok())
                    .map(|n| n.into_owned())
            })
            .collect(),
    };
    Ok(Read {
        paragraphs: body.paragraphs,
        tables: body.tables,
        styles,
        numbering,
        page: body.page,
        package,
        media,
        unsupported: unsupported.into_iter().collect(),
        preserved: preserved.into_iter().collect(),
        stories,
        even_odd,
        comments,
        default_tab,
        decimal,
        track,
        notes,
        note_settings,
    })
}

/// The notes of a footnotes or endnotes part, each read into its own small
/// document. Returns them and the part's root start tag.
fn read_notes(
    zip: &mut zip::ZipArchive<Cursor<&[u8]>>,
    kind: NoteKind,
    name: &str,
    src: &str,
    styles: &StyleSheet,
    numbering: &Numbering,
) -> anyhow::Result<(Vec<Note>, String)> {
    let root = format!("<w:{}", kind.root());
    let root_tag = src
        .find(&root)
        .and_then(|i| src[i..].find('>').map(|e| src[i..=i + e].to_owned()))
        .unwrap_or_else(|| {
            format!(
                "<w:{} {} {}>",
                kind.root(),
                crate::notes::W_DECL,
                crate::notes::R_DECL
            )
        });
    // Each note's content goes through the body reader as a story root.
    let open = root_tag
        .replacen(&root, "<w:hdr", 1)
        .trim_end_matches("/>")
        .trim_end_matches('>')
        .to_owned()
        + ">";
    let mut out = Vec::new();
    for (id, ty, whole, inner) in crate::notes::split_notes(src, kind) {
        let separator = ty.as_deref().is_some_and(|t| t != "normal");
        let story = read_story(
            zip,
            HfKind::Header,
            "",
            name,
            &format!("{open}{inner}</w:hdr>"),
            styles,
            numbering,
        )?;
        out.push(Note {
            kind,
            id,
            doc: story.doc,
            xml: Some(whole),
            separator,
        });
    }
    Ok((out, root_tag))
}

/// A header or footer part read into its own small document.
fn read_story(
    zip: &mut zip::ZipArchive<Cursor<&[u8]>>,
    kind: HfKind,
    rel_id: &str,
    name: &str,
    src: &str,
    styles: &StyleSheet,
    numbering: &Numbering,
) -> anyhow::Result<Story> {
    let rels: HashMap<String, Rel> =
        parse_rels(&zip_text(zip, &rels_name(name))?.unwrap_or_default())
            .into_iter()
            .map(|r| (r.id.clone(), r))
            .collect();
    let body = match parse_body(src, &rels, name) {
        Ok(b) => b,
        Err(e) => {
            // Unreadable: keep the part as it is, show nothing.
            let mut doc = Document::new();
            doc.styles = styles.clone();
            return Ok(Story {
                kind,
                rel_id: rel_id.to_owned(),
                part: name.to_owned(),
                root_tag: String::new(),
                doc,
                modified: false,
                new: false,
                locked: Some(format!("Kantor can't read this part: {e:#}")),
            });
        }
    };
    let mut media = HashMap::new();
    for id in &body.image_rels {
        let Some(rel) = rels.get(id) else { continue };
        let part = resolve(name, &rel.target);
        let data = zip_bytes(zip, &part)?;
        let drawable = data.as_deref().map(ImageFormat::sniff);
        media.insert(
            id.clone(),
            Media {
                part,
                bytes: data
                    .filter(|_| matches!(drawable, Some(ImageFormat::Png | ImageFormat::Jpeg)))
                    .map(Into::into),
            },
        );
    }
    let locked = (!body.unsupported.is_empty()).then(|| {
        format!(
            "It has content Kantor can't edit ({}); it is shown and kept as it is",
            body.unsupported
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )
    });
    let mut doc = Document::new();
    doc.paragraphs = if body.paragraphs.is_empty() {
        vec![Paragraph::default()]
    } else {
        body.paragraphs
    };
    doc.tables = body.tables;
    doc.styles = styles.clone();
    doc.numbering = numbering.clone();
    doc.media = media;
    let mut paras = std::mem::take(&mut doc.paragraphs);
    crate::float::share_parts(&mut paras, &doc);
    doc.paragraphs = paras;
    Ok(Story {
        kind,
        rel_id: rel_id.to_owned(),
        part: name.to_owned(),
        root_tag: body.root_tag.unwrap_or_default(),
        doc,
        modified: false,
        new: false,
        locked,
    })
}

// ── writing ────────────────────────────────────────────────────────────────

fn on_off(name: &str, v: bool) -> String {
    if v {
        format!("<w:{name}/>")
    } else {
        format!("<w:{name} w:val=\"0\"/>")
    }
}

/// `<w:rPr>…</w:rPr>` for run properties (empty string if none).
pub(crate) fn rpr_xml(p: &RunProps) -> String {
    let mut els: Vec<String> = Vec::new();
    let has = |name: &str| p.extras.iter().any(|x| element_name(x) == name);
    if let Some(s) = &p.style
        && !has("rStyle")
    {
        els.push(format!("<w:rStyle w:val=\"{}\"/>", escape(s)));
    }
    if !p.fonts.is_empty() && !has("rFonts") {
        let f = &p.fonts;
        let mut a = String::new();
        for (k, v) in [
            ("ascii", &f.ascii),
            ("hAnsi", &f.h_ansi),
            ("eastAsia", &f.east_asia),
            ("cs", &f.cs),
            ("asciiTheme", &f.ascii_theme),
            ("hAnsiTheme", &f.h_ansi_theme),
            ("eastAsiaTheme", &f.east_asia_theme),
            ("cstheme", &f.cs_theme),
            ("hint", &f.hint),
        ] {
            if let Some(v) = v {
                a.push_str(&format!(" w:{k}=\"{}\"", escape(v)));
            }
        }
        els.push(format!("<w:rFonts{a}/>"));
    }
    if let Some(b) = p.bold
        && !has("b")
    {
        els.push(on_off("b", b));
    }
    if let Some(i) = p.italic
        && !has("i")
    {
        els.push(on_off("i", i));
    }
    if let Some(u) = p.underline
        && !has("u")
    {
        els.push(format!(
            "<w:u w:val=\"{}\"/>",
            if u { "single" } else { "none" }
        ));
    }
    if let Some(c) = p.color
        && !has("color")
    {
        els.push(format!("<w:color w:val=\"{}\"/>", c.hex()));
    }
    if let Some(sz) = p.size
        && !has("sz")
    {
        els.push(format!("<w:sz w:val=\"{sz}\"/>"));
    }
    els.extend(p.extras.iter().map(str::to_owned));
    if els.is_empty() {
        return String::new();
    }
    format!("<w:rPr>{}</w:rPr>", ordered(RPR_ORDER, els))
}

pub(crate) fn ppr_xml(p: &Paragraph) -> String {
    let props = &p.props;
    let has = |name: &str| props.extras.iter().any(|x| element_name(x) == name);
    let empty = p.text.is_empty();
    let mut els: Vec<String> = Vec::new();
    if let Some(s) = &props.style {
        els.push(format!("<w:pStyle w:val=\"{}\"/>", escape(s)));
    }
    if let Some(n) = props.num
        && !has("numPr")
    {
        els.push(format!(
            "<w:numPr><w:ilvl w:val=\"{}\"/><w:numId w:val=\"{}\"/></w:numPr>",
            n.level, n.id
        ));
    }
    if (props.before.is_some() || props.after.is_some() || props.line.is_some()) && !has("spacing")
    {
        let mut a = String::new();
        if let Some(b) = props.before {
            a.push_str(&format!(" w:before=\"{b}\""));
        }
        if let Some(v) = props.after {
            a.push_str(&format!(" w:after=\"{v}\""));
        }
        if let Some(l) = props.line {
            let rule = match l.rule {
                LineRule::Auto => "auto",
                LineRule::AtLeast => "atLeast",
                LineRule::Exact => "exact",
            };
            a.push_str(&format!(" w:line=\"{}\" w:lineRule=\"{rule}\"", l.line));
        }
        els.push(format!("<w:spacing{a}/>"));
    }
    if !props.indent.is_empty() && !has("ind") {
        let i = props.indent;
        let mut a = String::new();
        if let Some(l) = i.left {
            a.push_str(&format!(" w:left=\"{l}\""));
        }
        if let Some(r) = i.right {
            a.push_str(&format!(" w:right=\"{r}\""));
        }
        match i.first {
            Some(f) if f < 0 => a.push_str(&format!(" w:hanging=\"{}\"", -f)),
            Some(f) => a.push_str(&format!(" w:firstLine=\"{f}\"")),
            None => {}
        }
        els.push(format!("<w:ind{a}/>"));
    }
    if let Some(a) = props.align
        && !has("jc")
    {
        let v = match a {
            Align::Left => "left",
            Align::Center => "center",
            Align::Right => "right",
            Align::Justify => "both",
        };
        els.push(format!("<w:jc w:val=\"{v}\"/>"));
    }
    if let Some(v) = props.page_break_before {
        els.push(on_off("pageBreakBefore", v));
    }
    if let Some(v) = props.contextual {
        els.push(on_off("contextualSpacing", v));
    }
    if empty {
        let mark = rpr_xml(&p.runs[0].props);
        if !mark.is_empty() {
            els.push(mark);
        }
    }
    for x in props.extras.iter() {
        if empty && element_name(x) == "rPr" {
            continue;
        }
        els.push(x.to_owned());
    }
    if els.is_empty() {
        return String::new();
    }
    format!("<w:pPr>{}</w:pPr>", ordered(PPR_ORDER, els))
}

fn text_content(out: &mut String, text: &str, deleted: bool) {
    let mut buf = String::new();
    let (open, close) = if deleted {
        ("<w:delText xml:space=\"preserve\">", "</w:delText>")
    } else {
        ("<w:t xml:space=\"preserve\">", "</w:t>")
    };
    let flush = |out: &mut String, buf: &mut String| {
        if !buf.is_empty() {
            out.push_str(open);
            out.push_str(&escape(buf));
            out.push_str(close);
            buf.clear();
        }
    };
    for ch in text.chars() {
        let el = match ch {
            '\t' => "<w:tab/>",
            LINE_BREAK => "<w:br/>",
            PAGE_BREAK => "<w:br w:type=\"page\"/>",
            '\u{AD}' => "<w:softHyphen/>",
            '\u{2011}' => "<w:noBreakHyphen/>",
            OBJECT => continue,
            c if (c as u32) < 0x20 => continue,
            c => {
                buf.push(c);
                continue;
            }
        };
        flush(out, &mut buf);
        out.push_str(el);
    }
    flush(out, &mut buf);
}

/// Generated `<w:drawing>` for a picture inserted in Kantor.
pub(crate) fn image_xml(img: &Image) -> String {
    let name = img.part.rsplit('/').next().unwrap_or("image");
    format!(
        "<w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\" \
         xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\">\
         <wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/>\
         <wp:docPr id=\"{id}\" name=\"Picture {id}\" descr=\"{descr}\"/><wp:cNvGraphicFramePr>\
         <a:graphicFrameLocks xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
         noChangeAspect=\"1\"/></wp:cNvGraphicFramePr><a:graphic \
         xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><a:graphicData \
         uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:pic \
         xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:nvPicPr>\
         <pic:cNvPr id=\"0\" name=\"{name}\"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill>\
         <a:blip r:embed=\"{rel}\" xmlns:r=\"{REL_NS}\"/><a:stretch><a:fillRect/></a:stretch>\
         </pic:blipFill><pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/>\
         </a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>\
         </a:graphicData></a:graphic></wp:inline></w:drawing>",
        cx = img.cx,
        cy = img.cy,
        id = img.doc_pr,
        descr = escape(&img.descr),
        name = escape(name),
        rel = escape(&img.rel_id),
    )
}

fn write_run(out: &mut String, text: &str, run: &Run) {
    match &run.object {
        Some(Object::Image(img)) => {
            out.push_str("<w:r>");
            out.push_str(&rpr_xml(&run.props));
            match &img.xml {
                Some(x) => out.push_str(x),
                None => out.push_str(&image_xml(img)),
            }
            out.push_str("</w:r>");
        }
        Some(Object::Float(f)) => {
            out.push_str("<w:r>");
            out.push_str(&rpr_xml(&run.props));
            out.push_str(&f.write_xml());
            out.push_str("</w:r>");
        }
        Some(Object::Opaque(o)) => match o.level {
            Level::Run => {
                out.push_str("<w:r>");
                out.push_str(&rpr_xml(&run.props));
                out.push_str(&o.xml);
                out.push_str("</w:r>");
            }
            Level::Para | Level::Marker | Level::Block | Level::Wrap => out.push_str(&o.xml),
        },
        Some(Object::Field(f)) => match &**f {
            FieldMark::Begin { code, sep, .. } => {
                let struck = run
                    .rev
                    .as_ref()
                    .is_some_and(|r| r.kind == crate::review::RevKind::Delete);
                if struck {
                    // A deleted field's instruction is `w:delInstrText`.
                    out.push_str(
                        &code
                            .replace("<w:instrText", "<w:delInstrText")
                            .replace("</w:instrText>", "</w:delInstrText>"),
                    );
                } else {
                    out.push_str(code);
                }
                if let Some(sep) = sep {
                    out.push_str(sep);
                }
            }
            FieldMark::End { xml, .. } => out.push_str(xml),
        },
        None => {
            if text.is_empty() {
                return;
            }
            out.push_str("<w:r>");
            out.push_str(&rpr_xml(&run.props));
            let deleted = run
                .rev
                .as_ref()
                .is_some_and(|r| r.kind == crate::review::RevKind::Delete);
            text_content(out, text, deleted);
            out.push_str("</w:r>");
        }
    }
}

/// Runs with their tracked changes: consecutive runs of one change go
/// inside its original `w:ins`/`w:del` tags.
fn write_tracked(out: &mut String, p: &Paragraph, runs: &[(std::ops::Range<usize>, &Run)]) {
    let mut i = 0;
    while i < runs.len() {
        match runs[i].1.rev.clone() {
            Some(rev) => {
                out.push_str(&rev.tag);
                while i < runs.len() && runs[i].1.rev.as_deref() == Some(&*rev) {
                    let (r, run) = &runs[i];
                    write_run(out, &p.text[r.clone()], run);
                    i += 1;
                }
                out.push_str(&rev.close_tag());
            }
            None => {
                let (r, run) = &runs[i];
                write_run(out, &p.text[r.clone()], run);
                i += 1;
            }
        }
    }
}

/// One `<w:p>` from a paragraph's runs (or a subset of them).
fn write_p(out: &mut String, p: &Paragraph, runs: &[(std::ops::Range<usize>, &Run)]) {
    out.push_str("<w:p>");
    out.push_str(&ppr_xml(p));
    let mut i = 0;
    while i < runs.len() {
        let start = i;
        match runs[i].1.link.clone() {
            Some(tag) => {
                while i < runs.len() && runs[i].1.link.as_deref() == Some(&*tag) {
                    i += 1;
                }
                out.push_str(&tag);
                write_tracked(out, p, &runs[start..i]);
                out.push_str("</w:hyperlink>");
            }
            None => {
                while i < runs.len() && runs[i].1.link.is_none() {
                    i += 1;
                }
                write_tracked(out, p, &runs[start..i]);
            }
        }
    }
    out.push_str("</w:p>");
}

/// A paragraph; block objects and the edges of a table of contents'
/// content control in it are written between `<w:p>`s.
fn write_paragraph(out: &mut String, p: &Paragraph) {
    type Seg<'a> = Vec<(std::ops::Range<usize>, &'a Run)>;
    let runs: Vec<_> = p.run_ranges().collect();
    let between = |r: &Run| {
        r.object
            .as_ref()
            .is_some_and(|o| o.is_block() || o.is_wrap())
    };
    if !runs.iter().any(|(_, r)| between(r)) {
        write_p(out, p, &runs);
        return;
    }
    // Only wraps, starts before ends: the paragraph is one `<w:p>` inside
    // them, wherever the caret put text around them.
    let wraps: Vec<&Run> = runs
        .iter()
        .filter(|(_, r)| r.object.as_ref().is_some_and(Object::is_wrap))
        .map(|(_, r)| *r)
        .collect();
    let is_end = |r: &Run| matches!(&r.object, Some(Object::Opaque(o)) if o.xml.starts_with("</"));
    let blocks = runs
        .iter()
        .any(|(_, r)| r.object.as_ref().is_some_and(Object::is_block));
    let first_end = wraps.iter().position(|r| is_end(r)).unwrap_or(wraps.len());
    if !blocks && wraps[first_end..].iter().all(|r| is_end(r)) {
        for r in &wraps[..first_end] {
            write_run(out, "", r);
        }
        let inner: Vec<_> = runs
            .iter()
            .filter(|(_, r)| !r.object.as_ref().is_some_and(Object::is_wrap))
            .cloned()
            .collect();
        write_p(out, p, &inner);
        for r in &wraps[first_end..] {
            write_run(out, "", r);
        }
        return;
    }
    // Segments of runs between block objects and wraps.
    let mut segs: Vec<Seg> = vec![Vec::new()];
    let mut cuts: Vec<&Run> = Vec::new();
    for (range, run) in runs {
        if between(run) {
            cuts.push(run);
            segs.push(Vec::new());
        } else if let Some(seg) = segs.last_mut() {
            seg.push((range, run));
        }
    }
    let markers_only = |seg: &Seg| {
        seg.iter()
            .all(|(_, r)| r.object.as_ref().is_some_and(Object::is_marker))
    };
    // Wraps alone don't make the paragraph go away: one segment is its
    // `<w:p>` even when empty (the first with text, else the one after the
    // leading wraps).
    let has_block = cuts
        .iter()
        .any(|r| r.object.as_ref().is_some_and(Object::is_block));
    let main = (!has_block).then(|| {
        segs.iter()
            .position(|s| !markers_only(s))
            .unwrap_or_else(|| {
                let lead = segs.iter().take_while(|s| s.is_empty()).count();
                lead.min(segs.len() - 1)
            })
    });
    for (k, seg) in segs.iter().enumerate() {
        if k > 0 {
            write_run(out, "", cuts[k - 1]);
        }
        if Some(k) == main || !markers_only(seg) {
            write_p(out, p, seg);
        } else {
            // Markers alone stay at this level, without an extra paragraph.
            for (_, r) in seg {
                write_run(out, "", r);
            }
        }
    }
}

fn write_table(out: &mut String, t: &Table, id: u32, paras: &[Paragraph], available: u32) {
    out.push_str("<w:tbl>");
    match &t.tbl_pr {
        Some(x) => out.push_str(x),
        None => out.push_str("<w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/></w:tblPr>"),
    }
    out.push_str("<w:tblGrid>");
    for w in t.column_widths(available) {
        out.push_str(&format!("<w:gridCol w:w=\"{w}\"/>"));
    }
    if let Some(x) = &t.grid_change {
        out.push_str(x);
    }
    out.push_str("</w:tblGrid>");
    let mut used = vec![false; paras.len()];
    for (ri, row) in t.rows.iter().enumerate() {
        out.push_str("<w:tr>");
        if let Some(x) = &row.tbl_pr_ex {
            out.push_str(x);
        }
        if let Some(x) = &row.tr_pr {
            out.push_str(x);
        }
        for (ci, cell) in row.cells.iter().enumerate() {
            out.push_str("<w:tc>");
            match &cell.tc_pr {
                Some(x) => out.push_str(x),
                None => out.push_str("<w:tcPr/>"),
            }
            let want = CellRef {
                table: id,
                row: ri as u32,
                cell: ci as u32,
            };
            let mut any = false;
            for (k, p) in paras.iter().enumerate() {
                if p.cell == Some(want) {
                    used[k] = true;
                    any = true;
                    write_paragraph(out, p);
                }
            }
            if !any {
                out.push_str("<w:p/>");
            }
            out.push_str("</w:tc>");
        }
        out.push_str("</w:tr>");
    }
    out.push_str("</w:tbl>");
    // Never drop text whose cell vanished from the structure.
    for (k, p) in paras.iter().enumerate() {
        if !used[k] {
            let mut p = p.clone();
            p.cell = None;
            write_paragraph(out, &p);
        }
    }
}

/// Tracked-change ids made unique: a change whose runs are written in
/// several pieces (around a hyperlink's edge, a text box's own changes)
/// would repeat its `w:id`; later repeats get fresh ones.
fn unique_rev_ids(xml: String) -> String {
    const TAGS: [&str; 10] = [
        "<w:ins ",
        "<w:del ",
        "<w:rPrChange ",
        "<w:pPrChange ",
        "<w:cellIns ",
        "<w:cellDel ",
        "<w:tcPrChange ",
        "<w:trPrChange ",
        "<w:tblPrChange ",
        "<w:tblGridChange ",
    ];
    if !TAGS.iter().any(|t| xml.contains(t)) {
        return xml;
    }
    let id_at = |s: &str| -> Option<(usize, usize, u64)> {
        let end = s.find('>')?;
        let i = s[..end].find(" w:id=\"")? + 7;
        let e = s[i..end].find('"')? + i;
        Some((i, e, s[i..e].parse().ok()?))
    };
    let mut max = 0u64;
    for t in TAGS {
        for (at, _) in xml.match_indices(t) {
            if let Some((_, _, v)) = id_at(&xml[at..]) {
                max = max.max(v);
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml.as_str();
    loop {
        let next = TAGS.iter().filter_map(|t| rest.find(t)).min();
        let Some(at) = next else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        match id_at(rest) {
            Some((i, e, v)) if !seen.insert(v) => {
                max += 1;
                seen.insert(max);
                out.push_str(&rest[..i]);
                out.push_str(&max.to_string());
                rest = &rest[e..];
            }
            _ => {
                out.push('<');
                rest = &rest[1..];
            }
        }
    }
    out
}

pub(crate) fn body_xml(doc: &Document) -> String {
    unique_rev_ids(body_xml_raw(doc))
}

fn body_xml_raw(doc: &Document) -> String {
    let mut out = String::with_capacity(4096);
    let fixed = crate::fields::writable(doc.paragraphs());
    let paras = fixed.as_deref().unwrap_or(doc.paragraphs());
    let mut i = 0;
    while i < paras.len() {
        match paras[i]
            .cell
            .and_then(|c| doc.table(c.table).map(|t| (c.table, t)))
        {
            Some((id, t)) => {
                let mut j = i;
                while j < paras.len() && paras[j].cell.map(|c| c.table) == Some(id) {
                    j += 1;
                }
                write_table(&mut out, t, id, &paras[i..j], doc.content_width());
                i = j;
            }
            None => {
                if paras[i].cell.is_some() {
                    let mut p = paras[i].clone();
                    p.cell = None;
                    write_paragraph(&mut out, &p);
                } else {
                    write_paragraph(&mut out, &paras[i]);
                }
                i += 1;
            }
        }
    }
    out
}

/// A new empty package (styles, settings, page setup) built by docx-rs.
fn new_package(doc: &Document) -> anyhow::Result<Vec<u8>> {
    let s = &doc.styles;
    let docx = docx_rs::Docx::new()
        .page_size(doc.page.width, doc.page.height)
        .page_margin(
            docx_rs::PageMargin::new()
                .top(doc.page.margin_top as i32)
                .bottom(doc.page.margin_bottom as i32)
                .left(doc.page.margin_left as i32)
                .right(doc.page.margin_right as i32),
        )
        .add_paragraph(docx_rs::Paragraph::new());
    let mut buf = Cursor::new(Vec::new());
    docx.build().pack(&mut buf)?;
    // docx-rs writes style children and settings out of schema order (Word
    // offers to repair such files) and a second `Normal` style: those two
    // parts are Kantor's own.
    let mut src = zip::ZipArchive::new(Cursor::new(buf.into_inner()))?;
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for i in 0..src.len() {
        let mut f = src.by_index(i)?;
        let name = f.name().unwrap_or_default().into_owned();
        let text = match name.as_str() {
            "word/styles.xml" => styles_xml(s),
            "word/settings.xml" => {
                let mut t = String::new();
                f.read_to_string(&mut t)?;
                settings_in_order(&t)
            }
            _ => {
                drop(f);
                out.raw_copy_file(src.by_index_raw(i)?)?;
                continue;
            }
        };
        out.start_file(name, opts)?;
        out.write_all(text.as_bytes())?;
    }
    Ok(out.finish()?.into_inner())
}

/// `styles.xml` of a new document: defaults and styles, every element in
/// schema order (CT_Style: name, basedOn, next, qFormat, pPr, rPr, tblPr).
fn styles_xml(s: &StyleSheet) -> String {
    let mut x = String::with_capacity(4096);
    x.push_str(XML_DECL);
    x.push_str(&format!("<w:styles {W_DECL}><w:docDefaults><w:rPrDefault>"));
    let mut run = s.default_run.clone();
    if let Some(sz) = run.size
        && !run.extras.iter().any(|e| element_name(e) == "szCs")
    {
        run.extras.push(&format!("<w:szCs w:val=\"{sz}\"/>"));
    }
    let rpr = rpr_xml(&run);
    x.push_str(if rpr.is_empty() { "<w:rPr/>" } else { &rpr });
    x.push_str("</w:rPrDefault><w:pPrDefault>");
    let ppr = style_ppr_xml(&s.default_para, None);
    x.push_str(if ppr.is_empty() { "<w:pPr/>" } else { &ppr });
    x.push_str("</w:pPrDefault></w:docDefaults>");
    for st in &s.styles {
        let kind = match st.kind {
            StyleKind::Paragraph => "paragraph",
            StyleKind::Character => "character",
            StyleKind::Table => "table",
            StyleKind::Numbering => "numbering",
        };
        let default = if st.is_default {
            " w:default=\"1\""
        } else {
            ""
        };
        x.push_str(&format!(
            "<w:style w:type=\"{kind}\"{default} w:styleId=\"{}\"><w:name w:val=\"{}\"/>",
            escape(&st.id),
            escape(&st.name)
        ));
        if let Some(b) = &st.based_on {
            x.push_str(&format!("<w:basedOn w:val=\"{}\"/>", escape(b)));
        }
        if let Some(n) = &st.next {
            x.push_str(&format!("<w:next w:val=\"{}\"/>", escape(n)));
        }
        x.push_str("<w:qFormat/>");
        let outline = st
            .id
            .strip_prefix("Heading")
            .and_then(|l| l.parse::<u32>().ok())
            .filter(|l| (1..=9).contains(l));
        if matches!(st.kind, StyleKind::Paragraph | StyleKind::Table) {
            x.push_str(&style_ppr_xml(&st.para, outline));
        }
        x.push_str(&rpr_xml(&st.run));
        x.push_str("</w:style>");
    }
    x.push_str("</w:styles>");
    x
}

/// A style's `<w:pPr>` (no paragraph style of its own).
fn style_ppr_xml(props: &ParaProps, outline: Option<u32>) -> String {
    let mut props = props.clone();
    props.style = None;
    if let Some(level) = outline
        && !props.extras.iter().any(|e| element_name(e) == "outlineLvl")
    {
        if !props.extras.iter().any(|e| element_name(e) == "keepNext") {
            props.extras.push("<w:keepNext/>");
        }
        props
            .extras
            .push(&format!("<w:outlineLvl w:val=\"{}\"/>", level - 1));
    }
    let p = Paragraph {
        props,
        ..Paragraph::new(" ")
    };
    ppr_xml(&p)
}

/// docx-rs's settings with `w:zoom` moved before `w:defaultTabStop`
/// (CT_Settings: writeProtection, view, zoom, ...).
fn settings_in_order(t: &str) -> String {
    let (Some(tab), Some(zs)) = (t.find("<w:defaultTabStop"), t.find("<w:zoom")) else {
        return t.to_owned();
    };
    if zs < tab {
        return t.to_owned();
    }
    let Some(len) = t[zs..].find("/>").map(|e| e + 2) else {
        return t.to_owned();
    };
    let zoom = t[zs..zs + len].to_owned();
    let mut out = t.to_owned();
    out.replace_range(zs..zs + len, "");
    out.insert_str(tab, &zoom);
    out
}

/// Insert `items` before the closing tag `close` (or append).
fn insert_before_close(src: &str, close: &str, items: &str) -> String {
    let mut out = src.to_owned();
    match out.rfind(close) {
        Some(i) => out.insert_str(i, items),
        None => out.push_str(items),
    }
    out
}

/// The original package with a regenerated body (and new parts if needed).
fn write_into(doc: &Document, pkg: &Package) -> anyhow::Result<Vec<u8>> {
    let mut root = pkg.root_tag.clone();
    if root.ends_with("/>") {
        root.replace_range(root.len() - 2.., ">");
    }
    let mut xml = String::with_capacity(4096);
    xml.push_str(XML_DECL);
    xml.push_str(&root);
    xml.push_str("<w:body>");
    xml.push_str(&body_xml(doc));
    match doc.body_sect.as_ref().or(pkg.sect_pr.as_ref()) {
        // Only pgSz/pgMar attributes that differ change; the rest is verbatim.
        Some(s) => xml.push_str(&crate::page::update_sect_pr(s, &doc.page)),
        None => {
            let pg = &doc.page;
            xml.push_str(&format!(
                "<w:sectPr><w:pgSz w:w=\"{}\" w:h=\"{}\"/><w:pgMar w:top=\"{}\" w:right=\"{}\" \
                 w:bottom=\"{}\" w:left=\"{}\" w:header=\"708\" w:footer=\"708\" w:gutter=\"0\"/></w:sectPr>",
                pg.width, pg.height, pg.margin_top, pg.margin_right, pg.margin_bottom, pg.margin_left
            ));
        }
    }
    xml.push_str("</w:body></w:document>");

    // Parts to replace or add, by zip entry name.
    let mut replace: HashMap<String, Vec<u8>> = HashMap::new();
    let mut add: Vec<(String, Vec<u8>)> = Vec::new();
    let mut new_rels = String::new();
    let mut overrides = String::new();
    let mut defaults: BTreeSet<(&str, &str)> = BTreeSet::new();
    replace.insert(pkg.document_part.clone(), xml.into_bytes());
    let mut src = zip::ZipArchive::new(Cursor::new(pkg.bytes.as_slice()))?;

    if doc.numbering.is_modified() {
        match &pkg.numbering {
            Some((name, src)) => {
                replace.insert(name.clone(), doc.numbering.write(Some(src)).into_bytes());
            }
            None => {
                let name = match pkg.document_part.rsplit_once('/') {
                    Some((dir, _)) => format!("{dir}/numbering.xml"),
                    None => "numbering.xml".into(),
                };
                let part = doc.numbering.write(None).into_bytes();
                // docx-rs packages carry an unreferenced numbering.xml.
                if src.by_name(&name).is_ok() {
                    replace.insert(name.clone(), part);
                } else {
                    add.push((name.clone(), part));
                }
                let id = fresh_rel_id(pkg, "rIdKantorNumbering");
                new_rels.push_str(&format!(
                    "<Relationship Id=\"{id}\" Type=\"{REL_NUMBERING}\" Target=\"{}\"/>",
                    escape(&relative_to(&pkg.document_part, &name))
                ));
                overrides.push_str(&format!(
                    "<Override PartName=\"/{}\" ContentType=\"{CT_NUMBERING}\"/>",
                    escape(&name)
                ));
            }
        }
    }

    // Headers and footers changed or added in Kantor.
    for st in doc.stories() {
        if !st.modified {
            continue;
        }
        let mut root = st.root_tag.clone();
        if root.ends_with("/>") {
            root.replace_range(root.len() - 2.., ">");
        }
        let close = match st.kind {
            HfKind::Header => "</w:hdr>",
            HfKind::Footer => "</w:ftr>",
        };
        let part = format!("{XML_DECL}{root}{}{close}", body_xml(&st.doc)).into_bytes();
        if st.new || !pkg.has_part(&st.part) {
            add.push((st.part.clone(), part));
            let (ty, ct) = match st.kind {
                HfKind::Header => (REL_HEADER, CT_HEADER),
                HfKind::Footer => (REL_FOOTER, CT_FOOTER),
            };
            if !pkg.has_rel(&st.rel_id) {
                new_rels.push_str(&format!(
                    "<Relationship Id=\"{}\" Type=\"{ty}\" Target=\"{}\"/>",
                    escape(&st.rel_id),
                    escape(&relative_to(&pkg.document_part, &st.part))
                ));
            }
            overrides.push_str(&format!(
                "<Override PartName=\"/{}\" ContentType=\"{ct}\"/>",
                escape(&st.part)
            ));
        } else {
            replace.insert(st.part.clone(), part);
        }
    }
    // Pictures inserted into headers and footers in Kantor.
    for st in doc.stories().iter().filter(|s| s.modified) {
        part_media(
            &mut src,
            pkg,
            &st.part,
            &[&st.doc],
            (&mut replace, &mut add, &mut defaults),
        )?;
    }
    // Comments changed in Kantor (with the threaded comment parts Word
    // 2013+ and 365 keep beside them).
    if doc.comments_modified && (pkg.comments.is_some() || !doc.comments().is_empty()) {
        use crate::comment_thread::{self as ct, Extra};
        let with_styles = doc.styles.get("CommentText").is_some();
        let threads = ct::write(doc.comments(), &pkg.comment_threads);
        let dir = match pkg.document_part.rsplit_once('/') {
            Some((dir, _)) => format!("{dir}/"),
            None => String::new(),
        };
        let root = match &pkg.comments {
            Some(part) => part.root_tag.clone(),
            None => format!(
                "<w:comments xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"{REL_NS}\">"
            ),
        };
        let root = ct::ensure_ns(&root, "w14", ct::W14_NS, true);
        let bytes =
            crate::review::write_comments(&root, &threads.comments, with_styles).into_bytes();
        match &pkg.comments {
            Some(part) => {
                replace.insert(part.name.clone(), bytes);
            }
            None => {
                let name = format!("{dir}comments.xml");
                if pkg.has_part(&name) {
                    replace.insert(name.clone(), bytes);
                } else {
                    add.push((name.clone(), bytes));
                }
                let id = fresh_rel_id(pkg, "rIdKantorComments");
                new_rels.push_str(&format!(
                    "<Relationship Id=\"{id}\" Type=\"{REL_COMMENTS}\" Target=\"{}\"/>",
                    escape(&relative_to(&pkg.document_part, &name))
                ));
                overrides.push_str(&format!(
                    "<Override PartName=\"/{}\" ContentType=\"{CT_COMMENTS}\"/>",
                    escape(&name)
                ));
            }
        }
        for (kind, existing, bytes) in threads.parts {
            match existing {
                Some(name) => {
                    replace.insert(name, bytes);
                }
                None => {
                    let name = format!("{dir}{}", kind.file());
                    if pkg.has_part(&name) {
                        replace.insert(name.clone(), bytes);
                    } else {
                        add.push((name.clone(), bytes));
                    }
                    let base = match kind {
                        Extra::Extended => "rIdKantorCommentsEx",
                        Extra::Ids => "rIdKantorCommentsIds",
                        Extra::Extensible => "rIdKantorCommentsCex",
                        Extra::People => "rIdKantorPeople",
                    };
                    let id = fresh_rel_id(pkg, base);
                    new_rels.push_str(&format!(
                        "<Relationship Id=\"{id}\" Type=\"{}\" Target=\"{}\"/>",
                        kind.rel_type(),
                        escape(&relative_to(&pkg.document_part, &name))
                    ));
                    overrides.push_str(&format!(
                        "<Override PartName=\"/{}\" ContentType=\"{}\"/>",
                        escape(&name),
                        kind.content_type()
                    ));
                }
            }
        }
    }
    // Footnotes and endnotes changed or added in Kantor.
    for (k, kind) in NoteKind::ALL.into_iter().enumerate() {
        let now: Vec<(NoteKind, String, Option<String>)> = doc
            .notes()
            .iter()
            .filter(|n| n.kind == kind)
            .map(|n| (n.kind, n.id.clone(), n.xml.clone()))
            .collect();
        let was: Vec<_> = pkg
            .notes_read
            .iter()
            .filter(|n| n.0 == kind)
            .cloned()
            .collect();
        if now == was {
            continue;
        }
        let part = match &pkg.notes[k] {
            Some(p) => p.clone(),
            None => NotesPart {
                name: match pkg.document_part.rsplit_once('/') {
                    Some((dir, _)) => format!("{dir}/{}.xml", kind.root()),
                    None => format!("{}.xml", kind.root()),
                },
                root_tag: format!(
                    "<w:{} {} {}>",
                    kind.root(),
                    crate::notes::W_DECL,
                    crate::notes::R_DECL
                ),
            },
        };
        let bytes =
            crate::notes::write_notes(&part.root_tag, kind, doc.notes(), &body_xml).into_bytes();
        let docs: Vec<&Document> = doc
            .notes()
            .iter()
            .filter(|n| n.kind == kind)
            .map(|n| &n.doc)
            .collect();
        part_media(
            &mut src,
            pkg,
            &part.name,
            &docs,
            (&mut replace, &mut add, &mut defaults),
        )?;
        if pkg.notes[k].is_some() {
            replace.insert(part.name.clone(), bytes);
            continue;
        }
        if pkg.has_part(&part.name) {
            replace.insert(part.name.clone(), bytes);
        } else {
            add.push((part.name.clone(), bytes));
        }
        let (rel, ct) = match kind {
            NoteKind::Footnote => (REL_FOOTNOTES, CT_FOOTNOTES),
            NoteKind::Endnote => (REL_ENDNOTES, CT_ENDNOTES),
        };
        let id = fresh_rel_id(pkg, &format!("rIdKantor{}", kind.label()));
        new_rels.push_str(&format!(
            "<Relationship Id=\"{id}\" Type=\"{rel}\" Target=\"{}\"/>",
            escape(&relative_to(&pkg.document_part, &part.name))
        ));
        overrides.push_str(&format!(
            "<Override PartName=\"/{}\" ContentType=\"{ct}\"/>",
            escape(&part.name)
        ));
    }
    // Different odd and even pages.
    let even_odd_was = pkg
        .settings
        .as_ref()
        .is_some_and(|(_, s)| settings_even_odd(s));
    let track_was = pkg
        .settings
        .as_ref()
        .is_some_and(|(_, s)| track_changes_on(s));
    let tab_was = pkg
        .settings
        .as_ref()
        .and_then(|(_, s)| crate::tabs::settings_default_tab(s))
        .unwrap_or(720);
    if (doc.even_odd_headers() != even_odd_was
        || doc.track_changes() != track_was
        || doc.default_tab() != tab_was)
        && let Some((name, src)) = &pkg.settings
    {
        let mut out = src.clone();
        if doc.default_tab() != tab_was {
            out = crate::tabs::set_settings_default_tab(&out, doc.default_tab());
        }
        if doc.even_odd_headers() != even_odd_was {
            out = set_settings_even_odd(&out, doc.even_odd_headers());
        }
        if doc.track_changes() != track_was {
            out = crate::section::set_settings_track(&out, doc.track_changes());
        }
        replace.insert(name.clone(), out.into_bytes());
    }

    // Parts edited with floating objects (SmartArt text).
    for (name, xml) in doc.edited_parts().into_iter().rev() {
        if pkg.has_part(&name) {
            replace.insert(name, xml.as_bytes().to_vec());
        }
    }

    // Pictures inserted in Kantor that the body still uses.
    let used: HashSet<&str> = doc
        .paragraphs()
        .iter()
        .flat_map(|p| p.objects())
        .filter_map(|(_, o)| match o {
            Object::Image(i) => Some(i.rel_id.as_str()),
            _ => None,
        })
        .collect();
    for m in doc.new_media() {
        if pkg.has_rel(&m.rel_id) || !used.contains(m.rel_id.as_str()) {
            continue;
        }
        add.push((m.part.clone(), m.bytes.to_vec()));
        new_rels.push_str(&format!(
            "<Relationship Id=\"{}\" Type=\"{REL_IMAGE}\" Target=\"{}\"/>",
            escape(&m.rel_id),
            escape(&relative_to(&pkg.document_part, &m.part))
        ));
        let f = ImageFormat::from_name(&m.part);
        defaults.insert((f.extension(), f.content_type()));
    }

    if !new_rels.is_empty() {
        let name = rels_name(&pkg.document_part);
        let rels = zip_text(&mut src, &name)?.unwrap_or_else(|| {
            format!(
                "{XML_DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"></Relationships>"
            )
        });
        let out = insert_before_close(&rels, "</Relationships>", &new_rels);
        if src.by_name(&name).is_ok() {
            replace.insert(name, out.into_bytes());
        } else {
            add.push((name, out.into_bytes()));
        }
    }
    if !overrides.is_empty() || !defaults.is_empty() {
        let name = "[Content_Types].xml";
        let ct = zip_text(&mut src, name)?.unwrap_or_default();
        let mut items = String::new();
        for o in overrides.split_inclusive("/>") {
            let part = o.split('"').nth(1).unwrap_or_default();
            if !ct.contains(&format!("PartName=\"{part}\"")) {
                items.push_str(o);
            }
        }
        for (ext, ty) in defaults {
            let present = ct.contains(&format!("Extension=\"{ext}\""))
                || ct.contains(&format!("Extension=\"{}\"", ext.to_ascii_uppercase()));
            if !present {
                items.push_str(&format!(
                    "<Default Extension=\"{ext}\" ContentType=\"{ty}\"/>"
                ));
            }
        }
        if !items.is_empty() {
            replace.insert(
                name.into(),
                insert_before_close(&ct, "</Types>", &items).into_bytes(),
            );
        }
    }

    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut wrote_doc = false;
    for i in 0..src.len() {
        let file = src.by_index_raw(i)?;
        let name = file.name()?.to_string();
        match replace.remove(&name) {
            Some(bytes) => {
                drop(file);
                out.start_file(name.as_str(), opts)?;
                out.write_all(&bytes)?;
                wrote_doc |= name == pkg.document_part;
            }
            None => out.raw_copy_file(file)?,
        }
    }
    if !wrote_doc {
        bail!("{} vanished from the package", pkg.document_part);
    }
    for (name, bytes) in add {
        let stored = name.ends_with(".png") || name.ends_with(".jpeg") || name.ends_with(".jpg");
        let o = if stored {
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored)
        } else {
            opts
        };
        out.start_file(name.as_str(), o)?;
        out.write_all(&bytes)?;
    }
    Ok(out.finish()?.into_inner())
}

/// Pictures inserted in Kantor into `docs` (a header, footer or the notes
/// of a notes part) that they still use: their files, relationships in the
/// part's own `.rels` (created if missing) and content types.
#[allow(clippy::type_complexity)]
fn part_media<'a>(
    src: &mut zip::ZipArchive<Cursor<&[u8]>>,
    pkg: &Package,
    part: &str,
    docs: &[&'a Document],
    (replace, add, defaults): (
        &mut HashMap<String, Vec<u8>>,
        &mut Vec<(String, Vec<u8>)>,
        &mut BTreeSet<(&'a str, &'a str)>,
    ),
) -> anyhow::Result<()> {
    let mut rels_add = String::new();
    let rels_name = rels_name(part);
    let existing = zip_text(src, &rels_name)?;
    let has_id = |id: &str| {
        existing
            .as_deref()
            .is_some_and(|r| r.contains(&format!("Id=\"{id}\"")))
    };
    let mut seen = HashSet::new();
    for d in docs {
        let used: HashSet<&str> = d
            .paragraphs()
            .iter()
            .flat_map(|p| p.objects())
            .filter_map(|(_, o)| match o {
                Object::Image(i) => Some(i.rel_id.as_str()),
                _ => None,
            })
            .collect();
        for m in d.new_media() {
            if !used.contains(m.rel_id.as_str()) || !seen.insert(m.rel_id.clone()) {
                continue;
            }
            if !pkg.has_part(&m.part) && !add.iter().any(|(n, _)| *n == m.part) {
                add.push((m.part.clone(), m.bytes.to_vec()));
            }
            if !has_id(&m.rel_id) {
                rels_add.push_str(&format!(
                    "<Relationship Id=\"{}\" Type=\"{REL_IMAGE}\" Target=\"{}\"/>",
                    escape(&m.rel_id),
                    escape(&relative_to(part, &m.part))
                ));
            }
            let f = ImageFormat::from_name(&m.part);
            defaults.insert((f.extension(), f.content_type()));
        }
    }
    if rels_add.is_empty() {
        return Ok(());
    }
    let base = existing.unwrap_or_else(|| {
        format!(
            "{XML_DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"></Relationships>"
        )
    });
    let out = insert_before_close(&base, "</Relationships>", &rels_add).into_bytes();
    if src.by_name(&rels_name).is_ok() {
        replace.insert(rels_name, out);
    } else {
        add.push((rels_name, out));
    }
    Ok(())
}

/// A relationship id not used by the document part.
pub fn fresh_rel_id(pkg: &Package, base: &str) -> String {
    if !pkg.has_rel(base) {
        return base.to_owned();
    }
    (2..)
        .map(|n| format!("{base}{n}"))
        .find(|id| !pkg.has_rel(id))
        .expect("infinite ids")
}

/// Serialise `doc`: into its original package when it has one, else into a
/// new package.
pub fn write(doc: &Document, package: Option<&Package>) -> anyhow::Result<Vec<u8>> {
    match package {
        Some(pkg) => write_into(doc, pkg),
        None => {
            let base = read(new_package(doc)?)?.package;
            write_into(doc, &base)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Proofing;

    #[test]
    fn styles_keep_proofing_properties() {
        let src = r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val="22"/><w:lang w:val="en-US" w:eastAsia="ja-JP" w:bidi="ar-SA"/></w:rPr></w:rPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Isi"><w:name w:val="Isi"/><w:basedOn w:val="Normal"/><w:rPr><w:lang w:val="id-ID"/></w:rPr></w:style><w:style w:type="character" w:styleId="Code"><w:name w:val="Code"/><w:rPr><w:noProof/></w:rPr></w:style></w:styles>"#;
        let mut sheet = StyleSheet::default();
        parse_styles(src, &mut sheet);
        assert_eq!(sheet.default_run.size, Some(22));
        let plain = Paragraph::new("x");
        let isi = Paragraph::new("x").with_style("Isi");
        let none = RunProps::default();
        let code = RunProps {
            style: Some("Code".into()),
            ..RunProps::default()
        };
        assert_eq!(
            sheet.proofing(&plain, &none, None),
            Proofing::Check(Some("en-US".into()))
        );
        assert_eq!(
            sheet.proofing(&isi, &none, None),
            Proofing::Check(Some("id-ID".into()))
        );
        assert_eq!(sheet.proofing(&isi, &code, None), Proofing::Skip);
        // Direct formatting wins over styles.
        let mut direct = code.clone();
        direct.extras.push(r#"<w:noProof w:val="0"/>"#);
        direct.extras.push(r#"<w:lang w:val="en-GB"/>"#);
        assert_eq!(
            sheet.proofing(&isi, &direct, None),
            Proofing::Check(Some("en-GB".into()))
        );
    }
}
