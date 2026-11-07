//! `.docx` reading and writing.
//!
//! ## Round-trip safety (plan §5)
//!
//! Saving a document that was opened from a `.docx` **copies every part of
//! the original package byte for byte** (styles, theme, settings, numbering,
//! fonts, headers, media, custom XML…) and regenerates only the main
//! document part's body. The original `<w:document>` start tag (namespace
//! declarations) and the final `<w:sectPr>` (page size, margins, header
//! references) are written back verbatim too.
//!
//! The body is where loss could happen, so reading checks it against an
//! allowlist of exactly what the model holds: paragraphs, runs, text, tabs,
//! line breaks, `w:pStyle`/`w:jc`/`w:spacing`, and `w:b`/`w:i`/`w:u`/`w:sz`/
//! `w:color`/`w:rFonts`. A few elements that carry no content are dropped on
//! purpose (`w:proofErr`, `w:lang`, `w:noProof`, the complex-script twins
//! `w:bCs`/`w:iCs`/`w:szCs`, `w:lastRenderedPageBreak`, the paragraph-mark
//! `w:rPr`, Word's `_GoBack` bookmark). **Anything else** (tables, drawings,
//! fields, comments, tracked changes, hyperlinks, lists, section or page
//! breaks, headers/footers, unmodelled formatting…) is reported in
//! [`Read::unsupported`]; the document then counts as "imported with
//! unsupported content", and [`crate::Document::save`] refuses to overwrite
//! the original: the UI routes Save to Save As, and `save_as` refuses the
//! original's path.
//!
//! docx-rs writes: paragraphs and runs are built with its builder API, and
//! new documents are packaged by it. Its reader isn't used: most of what it
//! reads sits in private fields (reachable only through its JSON
//! serialiser), and it silently drops XML it doesn't know, so it can't tell
//! what a save would lose. The reader here is a small quick-xml pass over
//! the same subset the allowlist accepts, so the model and the safety check
//! can't disagree.

use std::collections::{BTreeSet, HashSet};
use std::io::{Cursor, Read as _, Write as _};
use std::sync::Arc;

use anyhow::{Context as _, bail};
use quick_xml::NsReader;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};

use crate::document::{Document, LINE_BREAK, PageGeometry};
use crate::model::*;
use crate::style::{Style, StyleKind, StyleSheet};

const W: &[u8] = b"http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const W_DECL: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";
const W14_DECL: &str = "xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\"";
const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n";

/// The original package a document was read from: the template for saving.
#[derive(Clone, Debug)]
pub struct Package {
    bytes: Arc<Vec<u8>>,
    /// Zip entry name of the main document part (usually `word/document.xml`).
    document_part: String,
    /// The original `<w:document …>` start tag.
    root_tag: String,
    /// The original body-level `<w:sectPr>…</w:sectPr>`, if any.
    sect_pr: Option<String>,
}

/// What [`read`] found.
pub struct Read {
    pub paragraphs: Vec<Paragraph>,
    pub styles: StyleSheet,
    pub page: PageGeometry,
    pub package: Package,
    /// Unmodelled content, as short descriptions ("tables", "<w:ind>").
    pub unsupported: Vec<String>,
}

fn zip_text(
    zip: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> anyhow::Result<Option<String>> {
    let mut file = match zip.by_name(name) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut s = String::new();
    file.read_to_string(&mut s)
        .with_context(|| format!("reading {name}"))?;
    Ok(Some(s))
}

/// Relationship targets of a `.rels` part whose type ends with `kind`.
fn rel_target(rels: &str, kind: &str) -> Option<String> {
    let mut r = quick_xml::Reader::from_str(rels);
    loop {
        match r.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) if e.local_name().as_ref() == b"Relationship" => {
                let ty = attr(&e, b"Type").unwrap_or_default();
                if ty.ends_with(kind) && attr(&e, b"TargetMode").as_deref() != Some("External") {
                    return attr(&e, b"Target");
                }
            }
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
    }
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

fn on_off(e: &BytesStart) -> bool {
    !matches!(
        attr(e, b"val").as_deref(),
        Some("0" | "false" | "off" | "none")
    )
}

/// How a property element was handled.
enum Prop {
    Modelled,
    /// Dropped on purpose (no content, see module docs).
    Ignored,
    Unsupported(String),
}

fn unsupported_attrs(e: &BytesStart, allowed: &[&[u8]]) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        let local = a.key.local_name();
        let name = local.as_ref();
        (!allowed.contains(&name)).then(|| {
            format!(
                "<w:{} w:{}>",
                String::from_utf8_lossy(e.local_name().as_ref()),
                String::from_utf8_lossy(name)
            )
        })
    })
}

/// A `w:rPr` child.
fn run_prop(e: &BytesStart, p: &mut RunProps) -> Prop {
    match e.local_name().as_ref() {
        b"b" => p.bold = Some(on_off(e)),
        b"i" => p.italic = Some(on_off(e)),
        b"u" => {
            let val = attr(e, b"val");
            p.underline = Some(!matches!(val.as_deref(), Some("none")));
            if !matches!(val.as_deref(), None | Some("single" | "none")) {
                return Prop::Unsupported("underline styles other than single".into());
            }
            if let Some(a) = unsupported_attrs(e, &[b"val"]) {
                return Prop::Unsupported(a);
            }
        }
        b"sz" => match attr(e, b"val").and_then(|v| v.parse().ok()) {
            Some(v) => p.size = Some(v),
            None => return Prop::Unsupported("<w:sz> without a size".into()),
        },
        b"color" => {
            let val = attr(e, b"val").unwrap_or_default();
            p.color = Some(Rgb::parse(&val).unwrap_or(Rgb([0, 0, 0])));
            if let Some(a) = unsupported_attrs(e, &[b"val"]) {
                return Prop::Unsupported(if a.contains("theme") {
                    "theme colours".into()
                } else {
                    a
                });
            }
        }
        b"rFonts" => {
            let f = &mut p.fonts;
            for a in e.attributes().flatten() {
                let v = Some(attr_value(&a));
                match a.key.local_name().as_ref() {
                    b"ascii" => f.ascii = v,
                    b"hAnsi" => f.h_ansi = v,
                    b"eastAsia" => f.east_asia = v,
                    b"cs" => f.cs = v,
                    b"asciiTheme" => f.ascii_theme = v,
                    b"hAnsiTheme" => f.h_ansi_theme = v,
                    b"eastAsiaTheme" => f.east_asia_theme = v,
                    b"cstheme" => f.cs_theme = v,
                    b"hint" => f.hint = v,
                    other => {
                        return Prop::Unsupported(format!(
                            "<w:rFonts w:{}>",
                            String::from_utf8_lossy(other)
                        ));
                    }
                }
            }
        }
        b"bCs" | b"iCs" | b"szCs" | b"lang" | b"noProof" => return Prop::Ignored,
        other => return Prop::Unsupported(category(other)),
    }
    Prop::Modelled
}

/// A `w:pPr` child (not `w:rPr`, handled by the caller).
fn para_prop(e: &BytesStart, p: &mut ParaProps) -> Prop {
    match e.local_name().as_ref() {
        b"pStyle" => p.style = attr(e, b"val"),
        b"jc" => match attr(e, b"val").as_deref().and_then(Align::from_jc) {
            Some(a) => p.align = Some(a),
            None => return Prop::Unsupported("distributed or other alignments".into()),
        },
        b"spacing" => {
            for a in e.attributes().flatten() {
                let v = attr_value(&a);
                match a.key.local_name().as_ref() {
                    b"before" => p.before = v.parse().ok(),
                    b"after" => p.after = v.parse().ok(),
                    b"line" => {
                        let rule = p.line.map(|l| l.rule).unwrap_or_default();
                        p.line = v.parse().ok().map(|line| LineSpacing { line, rule });
                    }
                    b"lineRule" => {
                        let rule = match v.as_str() {
                            "exact" => LineRule::Exact,
                            "atLeast" => LineRule::AtLeast,
                            _ => LineRule::Auto,
                        };
                        let line = p.line.map_or(240, |l| l.line);
                        p.line = Some(LineSpacing { line, rule });
                    }
                    b"beforeAutospacing" | b"afterAutospacing" if v == "0" || v == "false" => {}
                    other => {
                        return Prop::Unsupported(format!(
                            "<w:spacing w:{}>",
                            String::from_utf8_lossy(other)
                        ));
                    }
                }
            }
        }
        other => return Prop::Unsupported(category(other)),
    }
    Prop::Modelled
}

/// A readable name for unsupported content.
fn category(local: &[u8]) -> String {
    match local {
        b"tbl" => "tables",
        b"drawing" | b"pict" | b"object" => "images or drawings",
        b"fldChar" | b"instrText" | b"fldSimple" => "fields (page numbers, contents…)",
        b"commentRangeStart" | b"commentRangeEnd" | b"commentReference" => "comments",
        b"ins" | b"del" | b"moveFrom" | b"moveTo" | b"rPrChange" | b"pPrChange"
        | b"sectPrChange" | b"delText" => "tracked changes",
        b"headerReference" | b"footerReference" => "headers and footers",
        b"numPr" => "lists",
        b"hyperlink" => "hyperlinks",
        b"footnoteReference" | b"endnoteReference" => "footnotes or endnotes",
        b"sdt" => "content controls",
        b"sectPr" => "section breaks",
        b"AlternateContent" => "embedded objects",
        b"bookmarkStart" | b"bookmarkEnd" => "bookmarks",
        b"rStyle" => "character styles",
        b"smartTag" | b"customXml" => "smart tags",
        other => return format!("<w:{}>", String::from_utf8_lossy(other)),
    }
    .to_owned()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Frame {
    Document,
    Body,
    P,
    PPr,
    /// The paragraph mark's `w:rPr` (formatting of an empty paragraph).
    MarkRPr,
    R,
    RPr,
    T,
    SectPr,
    /// Inside an element whose content is ignored (unsupported or dropped).
    Skip,
}

struct BodyParser<'a> {
    src: &'a str,
    paragraphs: Vec<Paragraph>,
    unsupported: BTreeSet<String>,
    page: PageGeometry,
    root_tag: Option<String>,
    sect_pr: Option<String>,
    sect_start: usize,
    para: Paragraph,
    mark: RunProps,
    run: RunProps,
    go_back: HashSet<String>,
}

impl BodyParser<'_> {
    fn push_text(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.para.push(s, self.run.clone());
    }

    /// An element opened (or empty) under `top`. Returns the frame to push.
    fn open(&mut self, top: Frame, local: &[u8], e: &BytesStart, pos: (usize, usize)) -> Frame {
        use Frame::*;
        match (top, local) {
            (Document, b"body") => Body,
            (Body, b"p") => {
                self.para = Paragraph::default();
                self.mark = RunProps::default();
                P
            }
            (Body, b"sectPr") => {
                self.sect_start = pos.0;
                SectPr
            }
            (SectPr, b"pgSz") => {
                let n = |k| attr(e, k).and_then(|v| v.parse().ok());
                self.page.width = n(b"w").unwrap_or(self.page.width);
                self.page.height = n(b"h").unwrap_or(self.page.height);
                Skip
            }
            (SectPr, b"pgMar") => {
                let n = |k| {
                    attr(e, k)
                        .and_then(|v| v.parse::<i64>().ok())
                        .map(|v| v.unsigned_abs() as u32)
                };
                let pg = &mut self.page;
                pg.margin_top = n(b"top").unwrap_or(pg.margin_top);
                pg.margin_bottom = n(b"bottom").unwrap_or(pg.margin_bottom);
                pg.margin_left = n(b"left").unwrap_or(pg.margin_left);
                pg.margin_right = n(b"right").unwrap_or(pg.margin_right);
                Skip
            }
            (SectPr, b"headerReference" | b"footerReference") => {
                self.unsupported.insert(category(local));
                Skip
            }
            (SectPr, _) => Skip,
            (Body | P, b"bookmarkStart") => {
                if attr(e, b"name").as_deref() == Some("_GoBack") {
                    self.go_back.insert(attr(e, b"id").unwrap_or_default());
                } else {
                    self.unsupported.insert(category(local));
                }
                Skip
            }
            (Body | P, b"bookmarkEnd") => {
                if !self.go_back.contains(&attr(e, b"id").unwrap_or_default()) {
                    self.unsupported.insert(category(local));
                }
                Skip
            }
            (P, b"pPr") => PPr,
            (P, b"r") => {
                self.run = RunProps::default();
                R
            }
            (P, b"proofErr") => Skip,
            (PPr, b"rPr") => MarkRPr,
            (PPr, _) => {
                if let Prop::Unsupported(what) = para_prop(e, &mut self.para.props) {
                    self.unsupported.insert(what);
                }
                Skip
            }
            (MarkRPr, _) => {
                let mut mark = std::mem::take(&mut self.mark);
                // Tracked paragraph marks are content; formatting isn't.
                if let Prop::Unsupported(what) = run_prop(e, &mut mark)
                    && what == "tracked changes"
                {
                    self.unsupported.insert(what);
                }
                self.mark = mark;
                Skip
            }
            (R, b"rPr") => RPr,
            (R, b"t") => T,
            (R, b"tab") => {
                self.push_text("\t");
                Skip
            }
            (R, b"br" | b"cr") => {
                match attr(e, b"type").as_deref() {
                    None | Some("textWrapping") => {
                        let mut b = [0; 4];
                        self.push_text(LINE_BREAK.encode_utf8(&mut b));
                    }
                    Some(_) => {
                        self.unsupported.insert("page or column breaks".into());
                    }
                }
                Skip
            }
            (R, b"lastRenderedPageBreak") => Skip,
            (RPr, _) => {
                let mut run = std::mem::take(&mut self.run);
                if let Prop::Unsupported(what) = run_prop(e, &mut run) {
                    self.unsupported.insert(what);
                }
                self.run = run;
                Skip
            }
            _ => {
                self.unsupported.insert(category(local));
                Skip
            }
        }
    }

    fn close(&mut self, frame: Frame, pos: (usize, usize)) {
        match frame {
            Frame::P => {
                let mut p = std::mem::take(&mut self.para);
                if p.text.is_empty() {
                    p.runs[0].props = std::mem::take(&mut self.mark);
                }
                p.normalize();
                self.paragraphs.push(p);
            }
            Frame::SectPr => {
                self.sect_pr = Some(self.src[self.sect_start..pos.1].to_owned());
            }
            _ => {}
        }
    }
}

fn parse_body(src: &str) -> anyhow::Result<BodyParser<'_>> {
    let mut reader = NsReader::from_str(src);
    let mut st = BodyParser {
        src,
        paragraphs: Vec::new(),
        unsupported: BTreeSet::new(),
        page: PageGeometry::default(),
        root_tag: None,
        sect_pr: None,
        sect_start: 0,
        para: Paragraph::default(),
        mark: RunProps::default(),
        run: RunProps::default(),
        go_back: HashSet::new(),
    };
    // `None` = above the root element.
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
                let frame = match stack.last().copied() {
                    None => {
                        if !(is_w && local.as_ref() == b"document") {
                            bail!("not a WordprocessingML document");
                        }
                        let tag = &src[before..after];
                        if !tag.contains(W_DECL) {
                            // Body XML is written with the usual `w:` prefix.
                            st.unsupported
                                .insert("unusual XML namespace prefixes".into());
                        }
                        st.root_tag = Some(tag.to_owned());
                        Frame::Document
                    }
                    Some(Frame::Skip) => Frame::Skip,
                    Some(top) if !is_w => {
                        if top != Frame::SectPr {
                            st.unsupported.insert(category(local.as_ref()));
                        }
                        Frame::Skip
                    }
                    Some(top) => st.open(top, local.as_ref(), e, (before, after)),
                };
                if empty {
                    st.close(frame, (before, after));
                } else {
                    stack.push(frame);
                }
            }
            Event::End(_) => {
                if let Some(frame) = stack.pop() {
                    st.close(frame, (before, after));
                }
            }
            Event::Text(t) if stack.last() == Some(&Frame::T) => {
                let s = t.xml10_content().context("text")?;
                st.push_text(&s);
            }
            Event::GeneralRef(r) if stack.last() == Some(&Frame::T) => {
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
                st.push_text(&s);
            }
            Event::CData(c) if stack.last() == Some(&Frame::T) => {
                st.push_text(&String::from_utf8_lossy(&c));
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if st.root_tag.is_none() {
        bail!("empty document part");
    }
    Ok(st)
}

/// Read `styles.xml` (defaults, paragraph and character styles).
fn parse_styles(src: &str, sheet: &mut StyleSheet) {
    #[derive(PartialEq, Clone, Copy)]
    enum In {
        Other,
        RPrDefault,
        PPrDefault,
        Style,
        StyleRPr,
        StylePPr,
    }
    let mut r = quick_xml::Reader::from_str(src);
    let mut stack: Vec<In> = Vec::new();
    let mut style = Style::default();
    loop {
        let event = match r.read_event() {
            Ok(e) => e,
            Err(err) => {
                log::warn!("styles.xml: {err}");
                break;
            }
        };
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let empty = matches!(event, Event::Empty(_));
                let top = stack.last().copied().unwrap_or(In::Other);
                let local = e.local_name();
                let next = match (top, local.as_ref()) {
                    (_, b"rPrDefault") => In::RPrDefault,
                    (_, b"pPrDefault") => In::PPrDefault,
                    (In::RPrDefault, b"rPr") => In::RPrDefault,
                    (In::PPrDefault, b"pPr") => In::PPrDefault,
                    (In::RPrDefault, _) => {
                        run_prop(e, &mut sheet.default_run);
                        In::Other
                    }
                    (In::PPrDefault, _) => {
                        para_prop(e, &mut sheet.default_para);
                        In::Other
                    }
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
                    (In::StyleRPr, _) => {
                        run_prop(e, &mut style.run);
                        In::Other
                    }
                    (In::StylePPr, _) => {
                        para_prop(e, &mut style.para);
                        In::Other
                    }
                    _ => In::Other,
                };
                if empty {
                    if next == In::Style {
                        sheet.styles.push(std::mem::take(&mut style));
                    }
                } else {
                    stack.push(next);
                }
            }
            Event::End(_) => {
                if stack.pop() == Some(In::Style) {
                    sheet.styles.push(std::mem::take(&mut style));
                }
            }
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
                return on_off(&e);
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
    let root_rels = zip_text(&mut zip, "_rels/.rels")?.unwrap_or_default();
    let document_part = rel_target(&root_rels, "/officeDocument")
        .map(|t| resolve("", &t))
        .unwrap_or_else(|| "word/document.xml".to_owned());
    let doc_xml =
        zip_text(&mut zip, &document_part)?.with_context(|| format!("missing {document_part}"))?;
    let doc_rels = zip_text(&mut zip, &rels_name(&document_part))?.unwrap_or_default();
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

    let body = parse_body(&doc_xml)?;
    let mut unsupported = body.unsupported;
    if let Some(name) = part("/settings")
        && let Some(src) = zip_text(&mut zip, &name)?
        && track_changes_on(&src)
    {
        unsupported.insert("track changes switched on".into());
    }
    let package = Package {
        bytes: bytes.clone(),
        document_part,
        root_tag: body.root_tag.unwrap_or_default(),
        sect_pr: body.sect_pr,
    };
    Ok(Read {
        paragraphs: body.paragraphs,
        styles,
        page: body.page,
        package,
        unsupported: unsupported.into_iter().collect(),
    })
}

// ── writing ────────────────────────────────────────────────────────────────

fn rpr(p: &RunProps) -> docx_rs::RunProperty {
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

fn paragraph(p: &Paragraph) -> docx_rs::Paragraph {
    let mut out = docx_rs::Paragraph::new();
    if let Some(s) = &p.props.style {
        out = out.style(s);
    }
    if let Some(a) = p.props.align {
        out = out.align(alignment(a));
    }
    if let Some(s) = line_spacing(&p.props) {
        out = out.line_spacing(s);
    }
    if p.text.is_empty() {
        // An empty paragraph's formatting lives on its mark.
        if p.runs[0].props != RunProps::default() {
            out = out.run_property(rpr(&p.runs[0].props));
        }
        return out;
    }
    for (range, run) in p.run_ranges() {
        let mut r = docx_rs::Run::new();
        r.run_property = rpr(&run.props);
        let mut buf = String::new();
        for ch in p.text[range].chars() {
            match ch {
                '\t' | LINE_BREAK => {
                    if !buf.is_empty() {
                        r = r.add_text(std::mem::take(&mut buf));
                    }
                    r = if ch == '\t' {
                        r.add_tab()
                    } else {
                        r.add_break(docx_rs::BreakType::TextWrapping)
                    };
                }
                c => buf.push(c),
            }
        }
        if !buf.is_empty() {
            r = r.add_text(buf);
        }
        out = out.add_run(r);
    }
    out
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
    out.run_property = rpr(&s.run);
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

fn body_xml(doc: &Document) -> anyhow::Result<String> {
    use docx_rs::BuildXML as _;
    let mut out = String::new();
    for p in doc.paragraphs() {
        out.push_str(std::str::from_utf8(&paragraph(p).build())?);
    }
    Ok(out)
}

/// A new package, built by docx-rs from the model.
fn write_new(doc: &Document) -> anyhow::Result<Vec<u8>> {
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
    for p in doc.paragraphs() {
        docx = docx.add_paragraph(paragraph(p));
    }
    let mut buf = Cursor::new(Vec::new());
    docx.build().pack(&mut buf)?;
    Ok(buf.into_inner())
}

/// The original package with a regenerated body.
fn write_into(doc: &Document, pkg: &Package) -> anyhow::Result<Vec<u8>> {
    let mut root = pkg.root_tag.clone();
    if !root.contains("xmlns:w14=") {
        // docx-rs tags paragraphs with w14:paraId.
        let at = root.len() - if root.ends_with("/>") { 2 } else { 1 };
        root.insert_str(at, &format!(" {W14_DECL}"));
    }
    if root.ends_with("/>") {
        root.replace_range(root.len() - 2.., ">");
    }
    let mut xml = String::with_capacity(4096);
    xml.push_str(XML_DECL);
    xml.push_str(&root);
    xml.push_str("<w:body>");
    xml.push_str(&body_xml(doc)?);
    match &pkg.sect_pr {
        Some(s) => xml.push_str(s),
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

    let mut src = zip::ZipArchive::new(Cursor::new(pkg.bytes.as_slice()))?;
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let mut wrote_doc = false;
    for i in 0..src.len() {
        let file = src.by_index_raw(i)?;
        if file.name() == pkg.document_part {
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            out.start_file(pkg.document_part.as_str(), opts)?;
            out.write_all(xml.as_bytes())?;
            wrote_doc = true;
        } else {
            out.raw_copy_file(file)?;
        }
    }
    if !wrote_doc {
        bail!("{} vanished from the package", pkg.document_part);
    }
    Ok(out.finish()?.into_inner())
}

/// Serialise `doc`: into its original package when it has one, else as a
/// new package.
pub fn write(doc: &Document, package: Option<&Package>) -> anyhow::Result<Vec<u8>> {
    match package {
        Some(pkg) => write_into(doc, pkg),
        None => write_new(doc),
    }
}
