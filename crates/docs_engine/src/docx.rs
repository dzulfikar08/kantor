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
//!    embedded blip), and the formatting the editor understands
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
//!    floating pictures (`wp:anchor`), shapes and text boxes
//!    (`mc:AlternateContent`, `w:pict`, `w:object`), content controls
//!    (`w:sdt`, inline or block), simple and complex fields whose result
//!    doesn't depend on the document (PAGE, DATE, HYPERLINK…; the whole
//!    `begin…end` run sequence when it sits in one paragraph), equations,
//!    smart tags and custom XML, footnote/endnote references, symbols,
//!    nested tables, and any other unknown body, paragraph or run element.
//!    Bookmarks and permission ranges are zero-width markers that text
//!    deletion never removes. Word's `_GoBack` bookmark, proofing marks and
//!    `w:lastRenderedPageBreak` are dropped (no content).
//! 4. **Unsupported** (the conservative whole-document flag, see
//!    [`Read::unsupported`]): tracked changes (`w:ins`, `w:del`, moves,
//!    property changes, "track changes" switched on), comments, fields that
//!    depend on document content (TOC, REF, PAGEREF, SEQ…) or span
//!    paragraphs, content controls around table rows or cells, and unusual
//!    namespace prefixes. Their XML is still kept as opaque objects where
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
use crate::model::*;
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
    /// Relationship ids of the main document part.
    rel_ids: HashSet<String>,
    /// Every zip entry name.
    parts: HashSet<String>,
}

impl Package {
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
        Some(b) => Some(String::from_utf8(b).with_context(|| format!("{name} isn't UTF-8"))?),
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

fn borders(el: &El) -> Borders {
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

fn cell_margins(el: &El) -> CellMargins {
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

/// Field instructions whose result depends on the document's own content.
fn content_dependent_field(instr: &str) -> bool {
    let word = instr
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_start_matches('=')
        .to_ascii_uppercase();
    instr.trim_start().starts_with('=')
        || matches!(
            word.as_str(),
            "TOC"
                | "REF"
                | "PAGEREF"
                | "NOTEREF"
                | "SEQ"
                | "STYLEREF"
                | "INDEX"
                | "TA"
                | "TC"
                | "XE"
                | "TOA"
                | "IF"
                | "SECTIONPAGES"
                | "SECTION"
                | "RD"
                | "LISTNUM"
        )
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

/// A complex field (`w:fldChar` begin … end) being collected.
struct Field {
    start: usize,
    depth: u32,
    in_result: bool,
    instr: String,
    result: String,
    /// Stack depth of the paragraph content level where it began.
    level: usize,
    link: Option<Arc<str>>,
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
    field: Option<Field>,
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

    fn push_text(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if let Some(f) = &mut self.field {
            if f.in_result && f.depth == 1 {
                f.result.push_str(s);
            }
            return;
        }
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
        if self.field.is_some() {
            return;
        }
        if !o.is_marker() {
            self.link_runs += 1;
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
            "footnoteReference" | "endnoteReference" => {
                self.preserved.insert("footnotes and endnotes".into());
                if local == "footnoteReference" {
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
        if content_dependent_field(instr) {
            self.unsupported.insert(
                "fields that depend on document content (contents, cross-references…)".into(),
            );
        } else {
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
            (P | Hyperlink | Rev, "r") => {
                self.run = RunProps::default();
                self.run_start = before;
                self.run_text_start = self.para.text.len();
                R
            }
            (P, "hyperlink") => {
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
            (P | Hyperlink, "ins" | "del" | "moveTo" | "moveFrom") if !empty => {
                match Revision::parse(&self.src[before..after]) {
                    Some(r) => {
                        self.rev = Some(Arc::new(r));
                        Rev
                    }
                    None => cap(self, Cap::ParaObj),
                }
            }
            (P | Hyperlink | Rev, "proofErr") => Skip,
            (P | Hyperlink | Rev, "bookmarkStart" | "bookmarkEnd" | "permStart" | "permEnd")
            | (P | Hyperlink | Rev, "commentRangeStart" | "commentRangeEnd")
            | (P | Hyperlink | Rev, "moveFromRangeStart" | "moveFromRangeEnd")
            | (P | Hyperlink | Rev, "moveToRangeStart" | "moveToRangeEnd") => {
                if empty {
                    let xml = &self.src[before..after];
                    self.marker(local, e, xml, true);
                    Skip
                } else {
                    cap(self, Cap::Marker)
                }
            }
            (P | Hyperlink, _) => cap(self, Cap::ParaObj),
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

    fn fld_char(&mut self, kind: Option<&str>, stack_len: usize) {
        match kind {
            Some("begin") => match &mut self.field {
                Some(f) => f.depth += 1,
                None => {
                    // Text this run already added goes into the field's XML.
                    let len = self.para.text.len();
                    if self.run_text_start < len {
                        self.para.delete(self.run_text_start..len);
                    }
                    self.field = Some(Field {
                        start: self.run_start,
                        depth: 1,
                        in_result: false,
                        instr: String::new(),
                        result: String::new(),
                        // The run's parent level (the run frame isn't counted).
                        level: stack_len.saturating_sub(1),
                        link: self.link.clone(),
                    });
                }
            },
            Some("separate") => {
                if let Some(f) = &mut self.field
                    && f.depth == 1
                {
                    f.in_result = true;
                }
            }
            Some("end") => {
                if let Some(f) = &mut self.field {
                    f.depth = f.depth.saturating_sub(1);
                }
            }
            _ => {}
        }
    }

    /// A captured element closed: `xml` is its full source.
    fn captured(&mut self, cap: Cap, xml: &str) {
        match cap {
            Cap::RunObj => {
                let local = xml::parse(xml).map(|e| e.name).unwrap_or_default();
                if is_tracked(&local) {
                    self.unsupported.insert(category(&local));
                }
                let obj = if local == "drawing" {
                    self.image(xml)
                } else {
                    None
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
                if matches!(self.frame_top, Some(Frame::P | Frame::Hyperlink)) {
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
                    if let Some(el) = xml::parse(xml) {
                        for c in &el.children {
                            match c.name.as_str() {
                                "tblStyle" => t.style = c.attr("val").map(str::to_owned),
                                "tblBorders" => t.borders = borders(c),
                                "tblInd" => t.indent = c.num("w").unwrap_or(0),
                                "tblCellMar" => t.cell_margins = Some(cell_margins(c)),
                                "tblPrChange" => {
                                    self.unsupported.insert("tracked changes".into());
                                }
                                _ => {}
                            }
                        }
                    }
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
                    if el.child("tblGridChange").is_some() {
                        self.unsupported.insert("tracked changes".into());
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
                            if el.children.iter().any(|c| is_tracked(&c.name)) {
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
                                "cellIns" | "cellDel" | "cellMerge" | "tcPrChange" => {
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

    fn close(&mut self, frame: Frame, (_, after): (usize, usize), stack_len: usize) {
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
                if let Some(f) = &self.field
                    && f.depth == 0
                {
                    let f = self.field.take().expect("field");
                    if f.level == stack_len {
                        let xml = &self.src[f.start..after];
                        self.field_checks(&f.instr);
                        let o = Self::opaque(Level::Para, xml, field_label(&f.instr, &f.result));
                        self.link = f.link;
                        self.push_object(o, RunProps::default());
                    } else {
                        self.unsupported
                            .insert("fields across hyperlinks or other elements".into());
                    }
                } else if self.field.is_none() {
                    self.link_runs += 1;
                }
            }
            Frame::Rev => self.rev = None,
            Frame::Hyperlink => {
                if self.link_runs == 0 && self.field.is_none() {
                    let xml = self.src[self.link_start..after].to_owned();
                    self.link = None;
                    let o =
                        Self::opaque(Level::Para, &xml, short(&xml::inner_text(&xml, &["t"]), 40));
                    self.push_object(o, RunProps::default());
                }
                self.link = None;
            }
            Frame::P => {
                if self.field.take().is_some() {
                    self.unsupported
                        .insert("fields spanning paragraphs (tables of contents…)".into());
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
        field: None,
        caps: Vec::new(),
        pending: Vec::new(),
        go_back: HashSet::new(),
        table: None,
        next_table: 0,
        cell_paras: 0,
        image_rels: BTreeSet::new(),
        frame_top: None,
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
        if let Some(f) = &mut st.field
            && f.depth == 1
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
    let apply = |target: u8, xml: &str, sheet: &mut StyleSheet, style: &mut Style| match target {
        0 => {
            run_prop(xml, &mut sheet.default_run);
        }
        1 => {
            para_prop(xml, &mut sheet.default_para);
        }
        2 => {
            run_prop(xml, &mut style.run);
        }
        3 => {
            para_prop(xml, &mut style.para);
        }
        _ => {
            // A table style's w:tblPr.
            if let Some(el) = xml::parse(xml) {
                let t = style.table.get_or_insert_with(TableStyle::default);
                for c in &el.children {
                    match c.name.as_str() {
                        "tblBorders" => t.borders = borders(c),
                        "tblCellMar" => t.cell_margins = Some(cell_margins(c)),
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

fn track_changes_on(settings: &str) -> bool {
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
    let bytes = Arc::new(bytes);
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
    let mut preserved = body.preserved;
    let mut settings = None;
    let mut even_odd = false;
    if let Some(name) = part("/settings")
        && let Some(src) = zip_text(&mut zip, &name)?
    {
        if track_changes_on(&src) {
            preserved
                .insert("track changes switched on (Kantor's own edits aren't tracked)".into());
        }
        even_odd = settings_even_odd(&src);
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
    })
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
fn rpr_xml(p: &RunProps) -> String {
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

fn ppr_xml(p: &Paragraph) -> String {
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
fn image_xml(img: &Image) -> String {
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
        Some(Object::Opaque(o)) => match o.level {
            Level::Run => {
                out.push_str("<w:r>");
                out.push_str(&rpr_xml(&run.props));
                out.push_str(&o.xml);
                out.push_str("</w:r>");
            }
            Level::Para | Level::Marker | Level::Block => out.push_str(&o.xml),
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

/// A paragraph; block objects in it are written between `<w:p>`s.
fn write_paragraph(out: &mut String, p: &Paragraph) {
    let runs: Vec<_> = p.run_ranges().collect();
    if !runs
        .iter()
        .any(|(_, r)| r.object.as_ref().is_some_and(Object::is_block))
    {
        write_p(out, p, &runs);
        return;
    }
    let mut seg = Vec::new();
    let flush = |out: &mut String, seg: &mut Vec<(std::ops::Range<usize>, &Run)>| {
        if seg.is_empty() {
            return;
        }
        if seg
            .iter()
            .all(|(_, r)| r.object.as_ref().is_some_and(Object::is_marker))
        {
            // Markers alone stay at this level, without an extra paragraph.
            for (_, r) in seg.iter() {
                write_run(out, "", r);
            }
        } else {
            write_p(out, p, seg);
        }
        seg.clear();
    };
    for (range, run) in runs {
        if run.object.as_ref().is_some_and(Object::is_block) {
            flush(out, &mut seg);
            write_run(out, "", run);
        } else {
            seg.push((range, run));
        }
    }
    flush(out, &mut seg);
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

fn body_xml(doc: &Document) -> String {
    let mut out = String::with_capacity(4096);
    let paras = doc.paragraphs();
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

// docx-rs builders for the styles of a new package.

fn docx_rpr(p: &RunProps) -> docx_rs::RunProperty {
    let mut r = docx_rs::RunProperty::new();
    if !p.fonts.is_empty() {
        let f = &p.fonts;
        let mut rf = docx_rs::RunFonts::new();
        macro_rules! set {
            ($($field:ident => $method:ident),*) => {$(
                if let Some(v) = &f.$field { rf = rf.$method(v.clone()); }
            )*};
        }
        set!(ascii => ascii, h_ansi => hi_ansi, east_asia => east_asia, cs => cs,
             ascii_theme => ascii_theme, h_ansi_theme => hi_ansi_theme,
             east_asia_theme => east_asia_theme, cs_theme => cs_theme, hint => hint);
        r = r.fonts(rf);
    }
    match p.bold {
        Some(true) => r = r.bold(),
        Some(false) => r = r.disable_bold(),
        None => {}
    }
    match p.italic {
        Some(true) => r = r.italic(),
        Some(false) => r = r.disable_italic(),
        None => {}
    }
    match p.underline {
        Some(true) => r = r.underline("single"),
        Some(false) => r = r.underline("none"),
        None => {}
    }
    if let Some(sz) = p.size {
        r = r.size(sz as usize);
    }
    if let Some(c) = p.color {
        r = r.color(c.hex());
    }
    r
}

fn line_spacing(p: &ParaProps) -> Option<docx_rs::LineSpacing> {
    if p.before.is_none() && p.after.is_none() && p.line.is_none() {
        return None;
    }
    let mut s = docx_rs::LineSpacing::new();
    if let Some(b) = p.before {
        s = s.before(b);
    }
    if let Some(a) = p.after {
        s = s.after(a);
    }
    if let Some(l) = p.line {
        s = s.line(l.line).line_rule(match l.rule {
            LineRule::Auto => docx_rs::LineSpacingType::Auto,
            LineRule::AtLeast => docx_rs::LineSpacingType::AtLeast,
            LineRule::Exact => docx_rs::LineSpacingType::Exact,
        });
    }
    Some(s)
}

fn alignment(a: Align) -> docx_rs::AlignmentType {
    match a {
        Align::Left => docx_rs::AlignmentType::Left,
        Align::Center => docx_rs::AlignmentType::Center,
        Align::Right => docx_rs::AlignmentType::Right,
        Align::Justify => docx_rs::AlignmentType::Both,
    }
}

fn style(s: &Style) -> docx_rs::Style {
    let kind = match s.kind {
        StyleKind::Paragraph => docx_rs::StyleType::Paragraph,
        StyleKind::Character => docx_rs::StyleType::Character,
        StyleKind::Table => docx_rs::StyleType::Table,
        StyleKind::Numbering => docx_rs::StyleType::Numbering,
    };
    let mut out = docx_rs::Style::new(&s.id, kind)
        .name(&s.name)
        .q_format(true);
    if let Some(b) = &s.based_on {
        out = out.based_on(b);
    }
    if let Some(n) = &s.next {
        out = out.next(n);
    }
    out.run_property = docx_rpr(&s.run);
    if let Some(a) = s.para.align {
        out = out.align(alignment(a));
    }
    if let Some(l) = line_spacing(&s.para) {
        out = out.line_spacing(l);
    }
    if s.id.starts_with("Heading")
        && let Ok(level) = s.id["Heading".len()..].parse::<usize>()
    {
        out = out.outline_lvl(level.saturating_sub(1));
    }
    out
}

/// A new empty package (styles, settings, page setup) built by docx-rs.
fn new_package(doc: &Document) -> anyhow::Result<Vec<u8>> {
    let s = &doc.styles;
    let mut docx = docx_rs::Docx::new()
        .page_size(doc.page.width, doc.page.height)
        .page_margin(
            docx_rs::PageMargin::new()
                .top(doc.page.margin_top as i32)
                .bottom(doc.page.margin_bottom as i32)
                .left(doc.page.margin_left as i32)
                .right(doc.page.margin_right as i32),
        );
    if let Some(sz) = s.default_run.size {
        docx = docx.default_size(sz as usize);
    }
    if let Some(f) = &s.default_run.fonts.ascii {
        docx = docx.default_fonts(
            docx_rs::RunFonts::new()
                .ascii(f.clone())
                .hi_ansi(f.clone())
                .east_asia(f.clone())
                .cs(f.clone()),
        );
    }
    if let Some(l) = line_spacing(&s.default_para) {
        docx = docx.default_line_spacing(l);
    }
    for st in &s.styles {
        docx = docx.add_style(style(st));
    }
    docx = docx.add_paragraph(docx_rs::Paragraph::new());
    let mut buf = Cursor::new(Vec::new());
    docx.build().pack(&mut buf)?;
    Ok(buf.into_inner())
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
    // Comments changed in Kantor.
    if doc.comments_modified {
        let with_styles = doc.styles.get("CommentText").is_some();
        match &pkg.comments {
            Some(part) => {
                replace.insert(
                    part.name.clone(),
                    crate::review::write_comments(&part.root_tag, doc.comments(), with_styles)
                        .into_bytes(),
                );
            }
            None if !doc.comments().is_empty() => {
                let name = match pkg.document_part.rsplit_once('/') {
                    Some((dir, _)) => format!("{dir}/comments.xml"),
                    None => "comments.xml".into(),
                };
                let root = format!(
                    "<w:comments xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"{REL_NS}\">"
                );
                let bytes =
                    crate::review::write_comments(&root, doc.comments(), with_styles).into_bytes();
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
            None => {}
        }
    }
    // Different odd and even pages.
    let even_odd_was = pkg
        .settings
        .as_ref()
        .is_some_and(|(_, s)| settings_even_odd(s));
    if doc.even_odd_headers() != even_odd_was
        && let Some((name, src)) = &pkg.settings
    {
        replace.insert(
            name.clone(),
            set_settings_even_odd(src, doc.even_odd_headers()).into_bytes(),
        );
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
