//! The ODF document written as a WordprocessingML package.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{Cursor, Write as _};

use super::style::{
    Level, PExtra, ParaProps, Scope, Styles, TextProps, border_xml, display_name, levels, ppr_xml,
};
use super::*;

const NS: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
    xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
    xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" \
    xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
    xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\" \
    xmlns:wps=\"http://schemas.microsoft.com/office/word/2010/wordprocessingShape\"";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const CT: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml";

/// Most rows or columns a `number-*-repeated` run adds (a style given to
/// the rest of a table).
const MAX_REPEAT: usize = 1000;

struct Rel {
    id: String,
    kind: &'static str,
    target: String,
    external: bool,
}

enum Block {
    /// `w:pPr` (with wrapper, may be empty) and the runs.
    P(String, String),
    /// A whole `w:tbl`.
    T(String),
}

impl Block {
    fn xml(&self, out: &mut String) {
        match self {
            Block::P(ppr, runs) => {
                let _ = write!(out, "<w:p>{ppr}{runs}</w:p>");
            }
            Block::T(t) => out.push_str(t),
        }
    }
}

/// Blocks as XML; a story or cell ends with a paragraph.
fn blocks_xml(blocks: &[Block]) -> String {
    let mut s = String::new();
    for b in blocks {
        b.xml(&mut s);
    }
    if !matches!(blocks.last(), Some(Block::P(..))) {
        s.push_str("<w:p/>");
    }
    s
}

/// A tracked change from `text:tracked-changes`.
struct Change {
    kind: &'static str,
    author: String,
    date: Option<String>,
    deleted: String,
}

/// A list being read: its list style and numbering instance.
#[derive(Clone)]
struct ListFrame {
    style: Option<String>,
    inst: usize,
}

/// A numbering instance (one ODF list and the lists continuing it).
struct Inst {
    /// The `w:num` in use, once a paragraph was numbered.
    num: Option<(u32, usize)>,
    /// A `text:start-value` restart waiting for the next item: level, value.
    restart: Option<(u8, u32)>,
}

#[derive(Clone)]
struct Sect {
    master: String,
    /// `w:cols` of a `text:section` (replacing the page's).
    cols: Option<String>,
    start: Option<u32>,
    kind: &'static str,
}

#[derive(Default)]
struct LeftOut {
    shapes: usize,
    pictures: usize,
    objects: usize,
    format_changes: usize,
    forms: usize,
}

/// A list paragraph's `w:numPr` (num id, level), or the left indent and
/// hanging of an unnumbered paragraph of the item.
type ListNum = (Option<(u32, u8)>, Option<(i32, i32)>);

/// Where a list paragraph is.
#[derive(Clone, Copy)]
struct ListPos {
    inst: usize,
    level: u8,
    /// The item's first paragraph (it gets the label).
    first: bool,
    header: bool,
}

struct Conv<'a> {
    st: &'a Styles<'a>,
    pkg: &'a Pkg,
    scope: Scope,
    rels: Vec<Rel>,
    part_rels: Vec<(String, Vec<Rel>)>,
    /// Parts besides document.xml: name, content type, XML.
    parts: Vec<(String, String, String)>,
    media: Vec<(&'static str, Vec<u8>)>,
    media_by_href: HashMap<String, Option<usize>>,
    /// Media index → relationship id, in the part being written.
    media_rel: HashMap<usize, String>,
    doc_pr: u32,
    footnotes: Vec<String>,
    endnotes: Vec<String>,
    comments: Vec<String>,
    comment_ids: HashMap<String, u32>,
    bookmarks: HashMap<String, u32>,
    changes: HashMap<&'a str, Change>,
    /// The open tracked insertion: change id, `w:ins` attributes.
    ins: Option<(String, String)>,
    rev: u32,
    abstracts: Vec<Vec<Level>>,
    abstract_by: HashMap<String, usize>,
    /// `w:num`s: abstract index, start overrides.
    nums: Vec<(usize, Vec<(u8, u32)>)>,
    insts: Vec<Inst>,
    lists: Vec<ListFrame>,
    last_by_style: HashMap<String, usize>,
    by_xml_id: HashMap<String, usize>,
    outline: Option<(u32, Vec<Level>)>,
    prev_space: bool,
    in_link: bool,
    pending: String,
    break_next: bool,
    sect: Sect,
    hf_cache: HashMap<String, (String, bool, [bool; 2])>,
    empty_hf: [Option<String>; 2],
    any_hf: [bool; 2],
    hf_count: u32,
    even_odd: bool,
    left_out: LeftOut,
}

pub(super) fn package(
    pkg: &Pkg,
    content: &roxmltree::Document,
    meta: Option<&roxmltree::Document>,
    st: &Styles,
) -> anyhow::Result<Import> {
    let root = content.root_element();
    let text = child(root, NS_OFFICE, "body")
        .and_then(|b| child(b, NS_OFFICE, "text"))
        .ok_or_else(|| anyhow!("not a text document (no office:text)"))?;
    let first_master = st
        .masters
        .first()
        .and_then(|m| attr(*m, NS_STYLE, "name"))
        .unwrap_or("Standard")
        .to_owned();
    let shown = |m: Node, tag: &str| {
        child(m, NS_STYLE, tag).is_some_and(|h| attr(h, NS_STYLE, "display") != Some("false"))
    };
    let even_odd = st
        .masters
        .iter()
        .any(|m| shown(*m, "header-left") || shown(*m, "footer-left"));
    let any_hf = [
        st.masters
            .iter()
            .any(|m| shown(*m, "header") || shown(*m, "header-first")),
        st.masters
            .iter()
            .any(|m| shown(*m, "footer") || shown(*m, "footer-first")),
    ];
    let mut c = Conv {
        st,
        pkg,
        scope: Scope::Content,
        rels: Vec::new(),
        part_rels: Vec::new(),
        parts: Vec::new(),
        media: Vec::new(),
        media_by_href: HashMap::new(),
        media_rel: HashMap::new(),
        doc_pr: 0,
        footnotes: Vec::new(),
        endnotes: Vec::new(),
        comments: Vec::new(),
        comment_ids: HashMap::new(),
        bookmarks: HashMap::new(),
        changes: HashMap::new(),
        ins: None,
        rev: 0,
        abstracts: Vec::new(),
        abstract_by: HashMap::new(),
        nums: Vec::new(),
        insts: Vec::new(),
        lists: Vec::new(),
        last_by_style: HashMap::new(),
        by_xml_id: HashMap::new(),
        outline: None,
        prev_space: true,
        in_link: false,
        pending: String::new(),
        break_next: false,
        sect: Sect {
            master: first_master,
            cols: None,
            start: None,
            kind: "nextPage",
        },
        hf_cache: HashMap::new(),
        empty_hf: [None, None],
        any_hf,
        hf_count: 0,
        even_odd,
        left_out: LeftOut::default(),
    };
    c.read_changes(text);
    c.read_outline();
    c.rel("styles", "styles.xml", false);
    c.rel("settings", "settings.xml", false);

    let mut body = Vec::new();
    c.flow(text, &mut body, true);
    if !c.pending.is_empty() {
        let runs = std::mem::take(&mut c.pending);
        body.push(Block::P(String::new(), runs));
    }
    let last = c.sect.clone();
    let sect = c.sect_pr(&last);
    let mut body_xml = String::new();
    for b in &body {
        b.xml(&mut body_xml);
    }
    if !matches!(body.last(), Some(Block::P(..))) {
        body_xml.push_str("<w:p/>");
    }
    let document = format!("<w:document {NS}><w:body>{body_xml}{sect}</w:body></w:document>");

    // Notes, comments, numbering, styles, settings.
    for (tag, file, notes) in [
        ("footnote", "footnotes", std::mem::take(&mut c.footnotes)),
        ("endnote", "endnotes", std::mem::take(&mut c.endnotes)),
    ] {
        if notes.is_empty() {
            continue;
        }
        let mut x = format!(
            "<w:{file} {NS}><w:{tag} w:type=\"separator\" w:id=\"-1\"><w:p><w:r><w:separator/></w:r></w:p></w:{tag}>\
             <w:{tag} w:type=\"continuationSeparator\" w:id=\"0\"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:{tag}>"
        );
        for (i, body) in notes.iter().enumerate() {
            let _ = write!(x, "<w:{tag} w:id=\"{}\">{body}</w:{tag}>", i + 1);
        }
        let _ = write!(x, "</w:{file}>");
        c.rel(file, &format!("{file}.xml"), false);
        c.parts
            .push((format!("word/{file}.xml"), format!("{CT}.{file}+xml"), x));
    }
    if !c.comments.is_empty() {
        let x = format!("<w:comments {NS}>{}</w:comments>", c.comments.concat());
        c.rel("comments", "comments.xml", false);
        c.parts
            .push(("word/comments.xml".into(), format!("{CT}.comments+xml"), x));
    }
    if let Some(n) = c.numbering_xml() {
        c.rel("numbering", "numbering.xml", false);
        c.parts.push((
            "word/numbering.xml".into(),
            format!("{CT}.numbering+xml"),
            n,
        ));
    }
    let styles = c.styles_xml();
    c.parts
        .push(("word/styles.xml".into(), format!("{CT}.styles+xml"), styles));
    let settings = format!(
        "<w:settings {NS}><w:defaultTabStop w:val=\"{}\"/>{}<w:compat><w:compatSetting w:name=\"compatibilityMode\" \
         w:uri=\"http://schemas.microsoft.com/office/word\" w:val=\"15\"/></w:compat></w:settings>",
        st.default_tab(),
        if c.even_odd {
            "<w:evenAndOddHeaders/>"
        } else {
            ""
        }
    );
    c.parts.push((
        "word/settings.xml".into(),
        format!("{CT}.settings+xml"),
        settings,
    ));

    let core = meta.map(core_xml);
    let docx = c.zip(&document, core.as_deref())?;
    Ok(Import {
        docx,
        notes: c.notes(),
    })
}

/// `docProps/core.xml` from `meta.xml`.
fn core_xml(meta: &roxmltree::Document) -> String {
    let find = |ns: &str, name: &str| {
        meta.descendants()
            .find(|n| is(*n, ns, name))
            .and_then(|n| n.text())
            .map(str::trim)
            .filter(|t| !t.is_empty())
    };
    let date = |s: &str| {
        let s = s.split('.').next().unwrap_or(s);
        let s = s.trim_end_matches('Z');
        format!("{s}Z")
    };
    let mut x = String::from(
        "<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" \
         xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">",
    );
    let mut el = |tag: &str, v: Option<&str>| {
        if let Some(v) = v {
            let _ = write!(x, "<{tag}>{}</{tag}>", esc(v));
        }
    };
    el("dc:title", find(NS_DC, "title"));
    el("dc:subject", find(NS_DC, "subject"));
    el(
        "dc:creator",
        find(NS_META, "initial-creator").or(find(NS_DC, "creator")),
    );
    let keywords: Vec<&str> = meta
        .descendants()
        .filter(|n| is(*n, NS_META, "keyword"))
        .filter_map(|n| n.text())
        .collect();
    el(
        "cp:keywords",
        (!keywords.is_empty())
            .then(|| keywords.join(", "))
            .as_deref(),
    );
    el("dc:description", find(NS_DC, "description"));
    el("cp:lastModifiedBy", find(NS_DC, "creator"));
    if let Some(d) = find(NS_META, "creation-date") {
        let _ = write!(
            x,
            "<dcterms:created xsi:type=\"dcterms:W3CDTF\">{}</dcterms:created>",
            esc(&date(d))
        );
    }
    if let Some(d) = find(NS_DC, "date") {
        let _ = write!(
            x,
            "<dcterms:modified xsi:type=\"dcterms:W3CDTF\">{}</dcterms:modified>",
            esc(&date(d))
        );
    }
    x.push_str("</cp:coreProperties>");
    x
}

/// An internal link's bookmark: LibreOffice's `#name|outline` targets
/// name a heading, not a bookmark; keep the name part.
fn anchor_name(s: &str) -> &str {
    s.split('|').next().unwrap_or(s)
}

fn iso_date(s: &str) -> String {
    let s = s.split('.').next().unwrap_or(s).trim_end_matches('Z');
    format!("{s}Z")
}

impl<'a> Conv<'a> {
    fn notes(&self) -> Vec<String> {
        let mut n = Vec::new();
        let l = &self.left_out;
        if l.shapes > 0 {
            n.push("drawing shapes were left out".to_owned());
        }
        if l.pictures > 0 {
            n.push("pictures in formats Kantor can't show (SVG, TIFF) were left out".to_owned());
        }
        if l.objects > 0 {
            n.push("embedded objects without a preview picture were left out".to_owned());
        }
        if l.format_changes > 0 {
            n.push("tracked formatting changes were left out (shown as made)".to_owned());
        }
        if l.forms > 0 {
            n.push("form controls were left out".to_owned());
        }
        n
    }

    fn rel(&mut self, kind: &'static str, target: &str, external: bool) -> String {
        let id = format!("rId{}", self.rels.len() + 1);
        self.rels.push(Rel {
            id: id.clone(),
            kind,
            target: target.to_owned(),
            external,
        });
        id
    }

    /// Run `f` writing part `file` (`header1.xml`): its relationships go to
    /// that part's own `.rels`, and the text state is its own.
    fn in_part<T>(&mut self, file: &str, scope: Scope, f: impl FnOnce(&mut Self) -> T) -> T {
        let so_far = self
            .part_rels
            .iter()
            .position(|(f, _)| f == file)
            .map(|i| self.part_rels.remove(i).1)
            .unwrap_or_default();
        let rels = std::mem::replace(&mut self.rels, so_far);
        let media = std::mem::take(&mut self.media_rel);
        let prev_space = self.prev_space;
        let in_link = std::mem::replace(&mut self.in_link, false);
        let pending = std::mem::take(&mut self.pending);
        let break_next = std::mem::replace(&mut self.break_next, false);
        let lists = std::mem::take(&mut self.lists);
        let old_scope = std::mem::replace(&mut self.scope, scope);
        let out = f(self);
        let own = std::mem::replace(&mut self.rels, rels);
        self.media_rel = media;
        self.prev_space = prev_space;
        self.in_link = in_link;
        self.pending = pending;
        self.break_next = break_next;
        self.lists = lists;
        self.scope = old_scope;
        if !own.is_empty() {
            self.part_rels.push((file.to_owned(), own));
        }
        out
    }

    // ── tracked changes, outline numbering ─────────────────────────────────

    fn read_changes(&mut self, text: Node<'a>) {
        let Some(tc) = child(text, NS_TEXT, "tracked-changes") else {
            return;
        };
        for region in tc.children().filter(|r| is(*r, NS_TEXT, "changed-region")) {
            let Some(id) = attr(region, NS_TEXT, "id").or(attr(region, NS_XML, "id")) else {
                continue;
            };
            let Some(ch) = region.children().find(|c| c.is_element()) else {
                continue;
            };
            let kind = match ch.tag_name().name() {
                "insertion" => "ins",
                "deletion" => "del",
                _ => "format",
            };
            let info = child(ch, NS_OFFICE, "change-info");
            let author = info
                .and_then(|i| child(i, NS_DC, "creator"))
                .map(text_of)
                .unwrap_or_else(|| "Unknown".into());
            let date = info
                .and_then(|i| child(i, NS_DC, "date"))
                .map(|d| iso_date(&text_of(d)));
            let deleted = if kind == "del" {
                ch.children()
                    .filter(|p| p.is_element() && !is(*p, NS_OFFICE, "change-info"))
                    .map(text_of)
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                String::new()
            };
            self.changes.insert(
                id,
                Change {
                    kind,
                    author,
                    date,
                    deleted,
                },
            );
        }
    }

    fn rev_attrs(&mut self, c: &Change) -> String {
        self.rev += 1;
        format!(
            "w:id=\"{}\" w:author=\"{}\"{}",
            self.rev,
            esc(&c.author),
            c.date
                .as_deref()
                .map(|d| format!(" w:date=\"{}\"", esc(d)))
                .unwrap_or_default()
        )
    }

    fn read_outline(&mut self) {
        let Some(o) = self.st.outline else { return };
        let lv = levels(self.st, Scope::Styles, o);
        if lv.iter().any(Level::shows) {
            self.abstracts.push(lv.clone());
            let a = self.abstracts.len() - 1;
            self.nums.push((a, Vec::new()));
            self.outline = Some((self.nums.len() as u32, lv));
        }
    }

    // ── flow ───────────────────────────────────────────────────────────────

    /// The blocks of a container (`office:text`, a cell, a note body, a
    /// header…). `body`: the main text, where sections can start.
    fn flow(&mut self, n: Node<'a>, out: &mut Vec<Block>, body: bool) {
        for c in n.children().filter(|c| c.is_element()) {
            self.block(c, out, body);
        }
    }

    fn block(&mut self, c: Node<'a>, out: &mut Vec<Block>, body: bool) {
        let ns = c.tag_name().namespace().unwrap_or("");
        let name = c.tag_name().name();
        match (ns, name) {
            (NS_TEXT, "p" | "h") => self.para(c, None, out, body),
            (NS_TEXT, "list") => self.list(c, out, body),
            (NS_TABLE, "table") => {
                if body {
                    let tstyle = attr(c, NS_TABLE, "style-name")
                        .and_then(|s| self.st.get(self.scope, "table", s));
                    if let Some(m) = tstyle
                        .and_then(|s| attr(s, NS_STYLE, "master-page-name"))
                        .filter(|m| !m.is_empty())
                    {
                        self.new_section(out, Some(m), None, None, "nextPage");
                    }
                }
                let t = self.table(c);
                out.push(Block::T(t));
            }
            (NS_TEXT, "section") => {
                if attr(c, NS_TEXT, "display") == Some("none") {
                    return;
                }
                let cols = attr(c, NS_TEXT, "style-name")
                    .and_then(|s| self.st.get(self.scope, "section", s))
                    .and_then(|s| child(s, NS_STYLE, "section-properties"))
                    .and_then(|p| child(p, NS_STYLE, "columns"))
                    .and_then(cols_xml);
                if body && let Some(cols) = cols {
                    self.new_section(out, None, Some(Some(cols)), None, "continuous");
                    self.flow(c, out, body);
                    self.new_section(out, None, Some(None), None, "continuous");
                } else {
                    self.flow(c, out, body);
                }
            }
            (NS_DRAW, "frame" | "a") => {
                let mut runs = String::new();
                self.drawing(c, &TextProps::default(), 12.0, &mut runs);
                self.pending.push_str(&runs);
            }
            // The forms' definitions; their controls are `draw:control`s.
            (NS_OFFICE, "forms") => {}
            (NS_DRAW, "control") => self.left_out.forms += 1,
            (NS_DRAW, _) => self.left_out.shapes += 1,
            (NS_TEXT, "change-start" | "change-end" | "change") => {
                let mut runs = String::new();
                self.change(c, &TextProps::default(), 12.0, &mut runs);
                if !runs.is_empty() {
                    out.push(Block::P(String::new(), runs));
                }
            }
            (NS_TEXT, "soft-page-break")
            | (NS_TEXT, "tracked-changes")
            | (NS_TEXT, "sequence-decls")
            | (NS_TEXT, "variable-decls")
            | (NS_TEXT, "user-field-decls")
            | (NS_TEXT, "dde-connection-decls")
            | (NS_TEXT, "alphabetical-index-auto-mark-file")
            | (NS_TABLE, "calculation-settings")
            | (NS_TABLE, "content-validations")
            | (NS_TABLE, "label-ranges")
            | (NS_OFFICE, "change-info")
            | (NS_DC, _) => {}
            (NS_OFFICE, "annotation" | "annotation-end") => {}
            // Indexes, numbered paragraphs, unknown wrappers: their blocks.
            _ => self.flow(c, out, body),
        }
    }

    /// Start a new section before the next block: the current one ends
    /// with the last paragraph so far.
    fn new_section(
        &mut self,
        out: &mut Vec<Block>,
        master: Option<&str>,
        cols: Option<Option<String>>,
        start: Option<u32>,
        kind: &'static str,
    ) {
        if !out.is_empty() {
            let cur = self.sect.clone();
            let sect = self.sect_pr(&cur);
            match out.last_mut() {
                Some(Block::P(ppr, _)) => {
                    if let Some(inner) = ppr.strip_suffix("</w:pPr>") {
                        *ppr = format!("{inner}{sect}</w:pPr>");
                    } else {
                        *ppr = format!("<w:pPr>{sect}</w:pPr>");
                    }
                }
                _ => out.push(Block::P(format!("<w:pPr>{sect}</w:pPr>"), String::new())),
            }
            self.sect.kind = kind;
            self.sect.start = start;
        } else {
            if start.is_some() {
                self.sect.start = start;
            }
        }
        if let Some(m) = master {
            self.sect.master = m.to_owned();
            // A new page style starts with its own page columns.
            if cols.is_none() {
                self.sect.cols = None;
            }
        }
        if let Some(c) = cols {
            self.sect.cols = c;
        }
    }

    // ── lists ──────────────────────────────────────────────────────────────

    fn list(&mut self, n: Node<'a>, out: &mut Vec<Block>, body: bool) {
        let own = attr(n, NS_TEXT, "style-name").map(str::to_owned);
        let frame = if let Some(outer) = self.lists.last() {
            ListFrame {
                style: outer.style.clone().or(own),
                inst: outer.inst,
            }
        } else {
            let cont = attr(n, NS_TEXT, "continue-list")
                .and_then(|id| self.by_xml_id.get(id).copied())
                .or_else(|| {
                    (attr(n, NS_TEXT, "continue-numbering") == Some("true"))
                        .then(|| {
                            own.as_ref()
                                .and_then(|s| self.last_by_style.get(s).copied())
                        })
                        .flatten()
                });
            let inst = cont.unwrap_or_else(|| {
                self.insts.push(Inst {
                    num: None,
                    restart: None,
                });
                self.insts.len() - 1
            });
            ListFrame { style: own, inst }
        };
        if let Some(id) = attr(n, NS_XML, "id") {
            self.by_xml_id.insert(id.to_owned(), frame.inst);
        }
        let outermost = self.lists.is_empty();
        let level = self.lists.len().min(8) as u8;
        self.lists.push(frame.clone());
        for item in n.children().filter(|c| c.is_element()) {
            let header = is(item, NS_TEXT, "list-header");
            if !header && !is(item, NS_TEXT, "list-item") {
                continue;
            }
            if let Some(v) = attr(item, NS_TEXT, "start-value").and_then(|v| v.parse().ok()) {
                self.insts[frame.inst].restart = Some((level, v));
            }
            let mut first = true;
            for c in item.children().filter(|c| c.is_element()) {
                if is(c, NS_TEXT, "p") || is(c, NS_TEXT, "h") {
                    let pos = ListPos {
                        inst: frame.inst,
                        level,
                        first,
                        header,
                    };
                    self.para(c, Some(pos), out, body);
                    first = false;
                } else if is(c, NS_TEXT, "list") {
                    self.list(c, out, body);
                } else {
                    self.block(c, out, body);
                }
            }
        }
        self.lists.pop();
        if outermost && let Some(s) = &frame.style {
            self.last_by_style.insert(s.clone(), frame.inst);
        }
    }

    /// The numbering of a list paragraph: `w:numPr` or the indent of the
    /// item's text.
    fn list_num(&mut self, pos: ListPos, para_style: Option<&str>) -> ListNum {
        let style = self.lists.last().and_then(|f| f.style.clone()).or_else(|| {
            self.st
                .prop(
                    self.scope,
                    "paragraph",
                    para_style,
                    "paragraph-properties",
                    NS_STYLE,
                    "list-style-name",
                )
                .or_else(|| {
                    para_style
                        .and_then(|s| self.st.get(self.scope, "paragraph", s))
                        .and_then(|s| attr(s, NS_STYLE, "list-style-name"))
                })
                .map(str::to_owned)
        });
        let Some(style) = style.filter(|s| !s.is_empty()) else {
            return (None, None);
        };
        let key = format!("{:?}:{style}", self.scope);
        let a = match self.abstract_by.get(&key) {
            Some(a) => *a,
            None => {
                let Some(node) = self.st.list(self.scope, &style) else {
                    return (None, None);
                };
                self.abstracts.push(levels(self.st, self.scope, node));
                let a = self.abstracts.len() - 1;
                self.abstract_by.insert(key, a);
                a
            }
        };
        let lv = self.abstracts[a][pos.level as usize].clone();
        if !pos.first || pos.header || !lv.shows() {
            return (None, Some((lv.left, 0)));
        }
        let inst = &mut self.insts[pos.inst];
        let restart = inst.restart.take();
        let num = match (inst.num, restart) {
            (Some((n, ia)), None) if ia == a => n,
            (cur, r) => {
                let overrides = match r {
                    Some((l, v)) if cur.is_some_and(|(_, ia)| ia == a) => vec![(l, v)],
                    Some((l, v)) => {
                        let mut o: Vec<(u8, u32)> = self.abstracts[a]
                            .iter()
                            .enumerate()
                            .map(|(i, l)| (i as u8, l.start))
                            .collect();
                        o[l as usize].1 = v;
                        o
                    }
                    None => self.abstracts[a]
                        .iter()
                        .enumerate()
                        .map(|(i, l)| (i as u8, l.start))
                        .collect(),
                };
                self.nums.push((a, overrides));
                let n = self.nums.len() as u32;
                self.insts[pos.inst].num = Some((n, a));
                n
            }
        };
        (Some((num, pos.level)), None)
    }

    // ── paragraphs ─────────────────────────────────────────────────────────

    fn para(&mut self, n: Node<'a>, list: Option<ListPos>, out: &mut Vec<Block>, body: bool) {
        let st = self.st;
        let sname = attr(n, NS_TEXT, "style-name");
        let (named, auto) = match sname {
            Some(s) => st.split(self.scope, "paragraph", s),
            None => (None, None),
        };
        let props = ParaProps::of_style(auto);
        if body {
            let master = auto
                .and_then(|a| attr(a, NS_STYLE, "master-page-name"))
                .or_else(|| {
                    named
                        .and_then(|s| st.named("paragraph", s))
                        .and_then(|s| attr(s, NS_STYLE, "master-page-name"))
                })
                .filter(|m| !m.is_empty());
            if let Some(m) = master {
                let start = props.page_number.flatten();
                self.new_section(out, Some(m), None, start, "nextPage");
            }
        }
        let style_id = named.and_then(|s| st.word_id("paragraph", s));
        let base_pt = st.size(self.scope, "paragraph", sname);
        let tp = TextProps::of_style(st, auto);

        let mut e = PExtra {
            style: style_id,
            ..Default::default()
        };
        if is(n, NS_TEXT, "h") {
            let level: u8 = attr(n, NS_TEXT, "outline-level")
                .and_then(|l| l.parse().ok())
                .unwrap_or(1)
                .clamp(1, 9);
            if style_id != Some(format!("Heading{level}").as_str()) {
                e.outline = Some(level - 1);
            }
            if list.is_none()
                && attr(n, NS_TEXT, "is-list-header") != Some("true")
                && let Some((num, lv)) = &self.outline
                && lv[level as usize - 1].shows()
            {
                e.num = Some((*num, level - 1));
            }
        }
        if let Some(pos) = list {
            let (num, ind) = self.list_num(pos, sname);
            e.num = num.or(e.num);
            e.ind = ind;
        }
        if st.settings.tabs_relative && props.tabs.as_ref().is_some_and(|t| !t.is_empty()) {
            e.tab_shift = props
                .left
                .or_else(|| {
                    st.prop(
                        self.scope,
                        "paragraph",
                        named,
                        "paragraph-properties",
                        NS_FO,
                        "margin-left",
                    )
                    .and_then(twips)
                })
                .unwrap_or(0);
        }
        e.page_break = std::mem::take(&mut self.break_next);
        if props.break_after == Some("page") {
            self.break_next = true;
        }
        e.rpr = tp.xml(None, base_pt);

        let mut runs = std::mem::take(&mut self.pending);
        if props.break_before == Some("column") {
            runs.push_str("<w:r><w:br w:type=\"column\"/></w:r>");
        }
        self.prev_space = true;
        self.inlines(n, &tp, None, base_pt, &mut runs);
        out.push(Block::P(ppr_xml(&props, &e), runs));
    }

    fn run(&self, tp: &TextProps, rstyle: Option<&str>, base_pt: f64, content: &str) -> String {
        let rpr = tp.xml(rstyle, base_pt);
        let r = if rpr.is_empty() {
            format!("<w:r>{content}</w:r>")
        } else {
            format!("<w:r><w:rPr>{rpr}</w:rPr>{content}</w:r>")
        };
        match &self.ins {
            Some((_, a)) => format!("<w:ins {a}>{r}</w:ins>"),
            None => r,
        }
    }

    fn text_run(
        &mut self,
        s: &str,
        tp: &TextProps,
        rstyle: Option<&str>,
        base_pt: f64,
        out: &mut String,
    ) {
        let mut t = String::new();
        for ch in s.chars() {
            if matches!(ch, ' ' | '\t' | '\n' | '\r') {
                if !self.prev_space {
                    t.push(' ');
                    self.prev_space = true;
                }
            } else {
                t.push(ch);
                self.prev_space = false;
            }
        }
        if !t.is_empty() {
            let r = self.run(
                tp,
                rstyle,
                base_pt,
                &format!("<w:t xml:space=\"preserve\">{}</w:t>", esc(&t)),
            );
            out.push_str(&r);
        }
    }

    fn inlines(
        &mut self,
        n: Node<'a>,
        tp: &TextProps,
        rstyle: Option<&str>,
        base_pt: f64,
        out: &mut String,
    ) {
        for c in n.children() {
            if c.is_text() {
                self.text_run(c.text().unwrap_or(""), tp, rstyle, base_pt, out);
                continue;
            }
            if !c.is_element() {
                continue;
            }
            self.inline(c, tp, rstyle, base_pt, out);
        }
    }

    fn inline(
        &mut self,
        c: Node<'a>,
        tp: &TextProps,
        rstyle: Option<&str>,
        base_pt: f64,
        out: &mut String,
    ) {
        let st = self.st;
        let ns = c.tag_name().namespace().unwrap_or("");
        let name = c.tag_name().name();
        match (ns, name) {
            (NS_TEXT, "span") => {
                let (named, auto) = match attr(c, NS_TEXT, "style-name") {
                    Some(s) => st.split(self.scope, "text", s),
                    None => (None, None),
                };
                let rs = named.and_then(|s| st.word_id("text", s)).or(rstyle);
                let tp2 = tp.over(&TextProps::of_style(st, auto));
                self.inlines(c, &tp2, rs, base_pt, out);
            }
            (NS_TEXT, "a") => self.link(c, tp, rstyle, base_pt, out),
            (NS_TEXT, "s") => {
                let n: usize = attr(c, NS_TEXT, "c")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1)
                    .clamp(1, 1000);
                let r = self.run(
                    tp,
                    rstyle,
                    base_pt,
                    &format!("<w:t xml:space=\"preserve\">{}</w:t>", " ".repeat(n)),
                );
                out.push_str(&r);
                self.prev_space = false;
            }
            (NS_TEXT, "tab") => {
                let r = self.run(tp, rstyle, base_pt, "<w:tab/>");
                out.push_str(&r);
                self.prev_space = false;
            }
            (NS_TEXT, "line-break") => {
                let r = self.run(tp, rstyle, base_pt, "<w:br/>");
                out.push_str(&r);
                self.prev_space = true;
            }
            (NS_TEXT, "note") => self.note(c, tp, rstyle, base_pt, out),
            (NS_TEXT, "bookmark") => {
                if let Some(nm) = attr(c, NS_TEXT, "name") {
                    let id = self.bookmark_id(nm);
                    let _ = write!(
                        out,
                        "<w:bookmarkStart w:id=\"{id}\" w:name=\"{}\"/><w:bookmarkEnd w:id=\"{id}\"/>",
                        esc(nm)
                    );
                }
            }
            (NS_TEXT, "bookmark-start") => {
                if let Some(nm) = attr(c, NS_TEXT, "name") {
                    let id = self.bookmark_id(nm);
                    let _ = write!(
                        out,
                        "<w:bookmarkStart w:id=\"{id}\" w:name=\"{}\"/>",
                        esc(nm)
                    );
                }
            }
            (NS_TEXT, "bookmark-end") => {
                if let Some(id) = attr(c, NS_TEXT, "name").and_then(|nm| self.bookmarks.get(nm)) {
                    let _ = write!(out, "<w:bookmarkEnd w:id=\"{id}\"/>");
                }
            }
            (NS_OFFICE, "annotation") => self.comment(c, tp, base_pt, out),
            (NS_OFFICE, "annotation-end") => {
                if let Some(id) = attr(c, NS_OFFICE, "name").and_then(|nm| self.comment_ids.get(nm))
                {
                    let _ = write!(
                        out,
                        "<w:commentRangeEnd w:id=\"{id}\"/><w:r><w:commentReference w:id=\"{id}\"/></w:r>"
                    );
                }
            }
            (NS_DRAW, "frame" | "a") => self.drawing(c, tp, base_pt, out),
            (NS_DRAW, "control") => self.left_out.forms += 1,
            (NS_DRAW, _) => self.left_out.shapes += 1,
            (NS_TEXT, "page-number") => self.field(" PAGE ", c, tp, rstyle, base_pt, out),
            (NS_TEXT, "page-count") => self.field(" NUMPAGES ", c, tp, rstyle, base_pt, out),
            (NS_TEXT, "title") => self.field(" TITLE ", c, tp, rstyle, base_pt, out),
            (NS_TEXT, "subject") => self.field(" SUBJECT ", c, tp, rstyle, base_pt, out),
            (NS_TEXT, "initial-creator" | "author-name") => {
                self.field(" AUTHOR ", c, tp, rstyle, base_pt, out)
            }
            (NS_TEXT, "file-name") => self.field(" FILENAME ", c, tp, rstyle, base_pt, out),
            (NS_TEXT, "change-start" | "change-end" | "change") => self.change(c, tp, base_pt, out),
            (NS_TEXT, "ruby") => {
                if let Some(b) = child(c, NS_TEXT, "ruby-base") {
                    self.inlines(b, tp, rstyle, base_pt, out);
                }
            }
            (NS_TEXT, "soft-page-break")
            | (NS_TEXT, "reference-mark")
            | (NS_TEXT, "reference-mark-start")
            | (NS_TEXT, "reference-mark-end")
            | (NS_TEXT, "toc-mark")
            | (NS_TEXT, "toc-mark-start")
            | (NS_TEXT, "toc-mark-end")
            | (NS_TEXT, "alphabetical-index-mark")
            | (NS_TEXT, "alphabetical-index-mark-start")
            | (NS_TEXT, "alphabetical-index-mark-end")
            | (NS_TEXT, "user-index-mark")
            | (NS_TEXT, "user-index-mark-start")
            | (NS_TEXT, "user-index-mark-end")
            | (NS_TEXT, "variable-decls")
            | (NS_TEXT, "sequence-decls") => {}
            (NS_TEXT, "hidden-text") => {
                if attr(c, NS_TEXT, "is-hidden") != Some("true") {
                    self.inlines(c, tp, rstyle, base_pt, out);
                }
            }
            _ => self.inlines(c, tp, rstyle, base_pt, out),
        }
    }

    fn bookmark_id(&mut self, name: &str) -> u32 {
        let next = self.bookmarks.len() as u32 + 1;
        *self.bookmarks.entry(name.to_owned()).or_insert(next)
    }

    fn field(
        &mut self,
        instr: &str,
        c: Node,
        tp: &TextProps,
        rstyle: Option<&str>,
        base_pt: f64,
        out: &mut String,
    ) {
        let result = text_of(c);
        let r = self.run(
            tp,
            rstyle,
            base_pt,
            &format!("<w:t xml:space=\"preserve\">{}</w:t>", esc(&result)),
        );
        let _ = write!(
            out,
            "<w:fldSimple w:instr=\"{}\">{r}</w:fldSimple>",
            esc(instr)
        );
        self.prev_space = false;
    }

    fn link(
        &mut self,
        c: Node<'a>,
        tp: &TextProps,
        rstyle: Option<&str>,
        base_pt: f64,
        out: &mut String,
    ) {
        let href = attr(c, NS_XLINK, "href").unwrap_or("").trim();
        let st = self.st;
        let rs = attr(c, NS_TEXT, "style-name")
            .and_then(|s| st.split(self.scope, "text", s).0)
            .and_then(|s| st.word_id("text", s))
            .or(rstyle);
        if self.in_link || href.is_empty() {
            self.inlines(c, tp, rs, base_pt, out);
            return;
        }
        let open = if let Some(a) = href.strip_prefix('#') {
            format!("<w:hyperlink w:anchor=\"{}\">", esc(anchor_name(a)))
        } else {
            let id = self.rel("hyperlink", href, true);
            format!("<w:hyperlink r:id=\"{id}\">")
        };
        let mut inner = String::new();
        self.in_link = true;
        self.inlines(c, tp, rs, base_pt, &mut inner);
        self.in_link = false;
        let _ = write!(out, "{open}{inner}</w:hyperlink>");
    }

    fn change(&mut self, c: Node<'a>, tp: &TextProps, base_pt: f64, out: &mut String) {
        let Some(id) = attr(c, NS_TEXT, "change-id") else {
            return;
        };
        let Some(ch) = self.changes.get(id) else {
            return;
        };
        match (c.tag_name().name(), ch.kind) {
            ("change-start", "ins") => {
                let attrs = {
                    let ch = Change {
                        kind: ch.kind,
                        author: ch.author.clone(),
                        date: ch.date.clone(),
                        deleted: String::new(),
                    };
                    self.rev_attrs(&ch)
                };
                self.ins = Some((id.to_owned(), attrs));
            }
            ("change-end", "ins") => {
                if self.ins.as_ref().is_some_and(|(i, _)| i == id) {
                    self.ins = None;
                }
            }
            ("change", "del") => {
                let text = ch.deleted.clone();
                let tmp = Change {
                    kind: ch.kind,
                    author: ch.author.clone(),
                    date: ch.date.clone(),
                    deleted: String::new(),
                };
                if text.is_empty() {
                    return;
                }
                let attrs = self.rev_attrs(&tmp);
                let rpr = tp.xml(None, base_pt);
                let rpr = if rpr.is_empty() {
                    String::new()
                } else {
                    format!("<w:rPr>{rpr}</w:rPr>")
                };
                let _ = write!(
                    out,
                    "<w:del {attrs}><w:r>{rpr}<w:delText xml:space=\"preserve\">{}</w:delText></w:r></w:del>",
                    esc(&text)
                );
            }
            ("change-start", "format") => self.left_out.format_changes += 1,
            _ => {}
        }
    }

    fn note(
        &mut self,
        c: Node<'a>,
        tp: &TextProps,
        rstyle: Option<&str>,
        base_pt: f64,
        out: &mut String,
    ) {
        let end = attr(c, NS_TEXT, "note-class") == Some("endnote");
        let Some(body) = child(c, NS_TEXT, "note-body") else {
            return;
        };
        let (file, tag) = if end {
            ("endnotes.xml", "endnote")
        } else {
            ("footnotes.xml", "footnote")
        };
        let scope = self.scope;
        let ins = self.ins.take();
        let mut blocks = self.in_part(file, scope, |c| {
            let mut b = Vec::new();
            c.flow(body, &mut b, false);
            b
        });
        self.ins = ins;
        // A custom mark (`text:label`) is typed after the reference, as
        // Word's `w:customMarkFollows`.
        let label = child(c, NS_TEXT, "note-citation").and_then(|n| attr(n, NS_TEXT, "label"));
        let sup = "<w:rPr><w:vertAlign w:val=\"superscript\"/></w:rPr>";
        let space = "<w:r><w:t xml:space=\"preserve\"> </w:t></w:r>";
        let mark = match label {
            Some(l) if !l.is_empty() => format!(
                "<w:r>{sup}<w:t xml:space=\"preserve\">{}</w:t></w:r>{space}",
                esc(l)
            ),
            Some(_) => String::new(),
            None => format!("<w:r>{sup}<w:{tag}Ref/></w:r>{space}"),
        };
        match blocks.first_mut() {
            Some(Block::P(_, runs)) => runs.insert_str(0, &mark),
            _ => blocks.insert(0, Block::P(String::new(), mark)),
        }
        let xml = blocks_xml(&blocks);
        let id = if end {
            self.endnotes.push(xml);
            self.endnotes.len()
        } else {
            self.footnotes.push(xml);
            self.footnotes.len()
        };
        let mut tp = tp.clone();
        tp.valign = Some("superscript");
        let custom = if label.is_some() {
            " w:customMarkFollows=\"1\""
        } else {
            ""
        };
        let r = self.run(
            &tp,
            rstyle,
            base_pt,
            &format!("<w:{tag}Reference{custom} w:id=\"{id}\"/>"),
        );
        out.push_str(&r);
        if let Some(l) = label.filter(|l| !l.is_empty()) {
            let r = self.run(
                &tp,
                rstyle,
                base_pt,
                &format!("<w:t xml:space=\"preserve\">{}</w:t>", esc(l)),
            );
            out.push_str(&r);
        }
        self.prev_space = false;
    }

    fn comment(&mut self, c: Node<'a>, _tp: &TextProps, _base_pt: f64, out: &mut String) {
        let id = self.comments.len() as u32;
        let author = child(c, NS_DC, "creator")
            .map(text_of)
            .filter(|a| !a.trim().is_empty())
            .unwrap_or_else(|| "Unknown".into());
        let date = child(c, NS_DC, "date").map(|d| iso_date(&text_of(d)));
        let initials: String = author
            .split_whitespace()
            .filter_map(|w| w.chars().next())
            .collect();
        let scope = self.scope;
        let ins = self.ins.take();
        let blocks = self.in_part("comments.xml", scope, |cv| {
            let mut b = Vec::new();
            cv.flow(c, &mut b, false);
            b
        });
        self.ins = ins;
        self.comments.push(format!(
            "<w:comment w:id=\"{id}\" w:author=\"{}\"{} w:initials=\"{}\">{}</w:comment>",
            esc(&author),
            date.map(|d| format!(" w:date=\"{}\"", esc(&d)))
                .unwrap_or_default(),
            esc(&initials),
            blocks_xml(&blocks)
        ));
        match attr(c, NS_OFFICE, "name") {
            Some(nm) => {
                self.comment_ids.insert(nm.to_owned(), id);
                let _ = write!(out, "<w:commentRangeStart w:id=\"{id}\"/>");
            }
            None => {
                let _ = write!(
                    out,
                    "<w:commentRangeStart w:id=\"{id}\"/><w:commentRangeEnd w:id=\"{id}\"/><w:r><w:commentReference w:id=\"{id}\"/></w:r>"
                );
            }
        }
    }

    // ── pictures and frames ────────────────────────────────────────────────

    /// The media index of a picture, reading it on first use.
    fn media_of(&mut self, img: Node) -> Option<usize> {
        let href = attr(img, NS_XLINK, "href").unwrap_or("");
        if !href.is_empty()
            && let Some(m) = self.media_by_href.get(href)
        {
            return *m;
        }
        let bytes = if href.is_empty() {
            child(img, NS_OFFICE, "binary-data")
                .and_then(|b| b.text())
                .and_then(base64)
        } else if href.contains("://") {
            None
        } else {
            self.pkg.file(href)
        };
        let m = bytes.and_then(|b| {
            let (ext, ok) = image_kind(&b)?;
            ok.then(|| {
                self.media.push((ext, b));
                self.media.len() - 1
            })
        });
        if !href.is_empty() {
            self.media_by_href.insert(href.to_owned(), m);
        }
        m
    }

    fn media_rel(&mut self, m: usize) -> String {
        if let Some(id) = self.media_rel.get(&m) {
            return id.clone();
        }
        let ext = self.media[m].0;
        let id = self.rel("image", &format!("media/image{}.{ext}", m + 1), false);
        self.media_rel.insert(m, id.clone());
        id
    }

    fn graphic_prop(&self, style: Option<&str>, ns: &str, at: &str) -> Option<&'a str> {
        self.st
            .prop(self.scope, "graphic", style, "graphic-properties", ns, at)
    }

    /// A `draw:frame` (or a `draw:a` around one) as a run.
    fn drawing(&mut self, f: Node<'a>, tp: &TextProps, base_pt: f64, out: &mut String) {
        if is(f, NS_DRAW, "a") {
            for c in f.children().filter(|c| c.is_element()) {
                self.drawing(c, tp, base_pt, out);
            }
            return;
        }
        if !is(f, NS_DRAW, "frame") {
            self.left_out.shapes += 1;
            return;
        }
        let style = attr(f, NS_DRAW, "style-name");
        let mut cx = attr(f, NS_SVG, "width")
            .or(attr(f, NS_FO, "min-width"))
            .and_then(emu)
            .unwrap_or(0);
        let mut cy = attr(f, NS_SVG, "height")
            .or(attr(f, NS_FO, "min-height"))
            .and_then(emu)
            .unwrap_or(0);
        self.doc_pr += 1;
        let n = self.doc_pr;
        let images: Vec<Node> = f.children().filter(|c| is(*c, NS_DRAW, "image")).collect();
        let tb = child(f, NS_DRAW, "text-box");
        let has_object = f
            .children()
            .any(|c| is(c, NS_DRAW, "object") || is(c, NS_DRAW, "object-ole"));
        let picture = images.iter().find_map(|i| self.media_of(*i));
        let (graphic, uri) = if let Some(m) = picture {
            if cx <= 0 || cy <= 0 {
                let (w, h) = pixel_size(&self.media[m].1).unwrap_or((96, 96));
                cx = i64::from(w.max(1)) * 9525;
                cy = i64::from(h.max(1)) * 9525;
            }
            let id = self.media_rel(m);
            let ext = self.media[m].0;
            (
                format!(
                    "<pic:pic><pic:nvPicPr><pic:cNvPr id=\"{n}\" name=\"image{n}.{ext}\"/><pic:cNvPicPr/></pic:nvPicPr>\
                     <pic:blipFill><a:blip r:embed=\"{id}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
                     <pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
                     <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>"
                ),
                "http://schemas.openxmlformats.org/drawingml/2006/picture",
            )
        } else if let Some(tb) = tb {
            let grow = attr(tb, NS_FO, "min-height").is_some();
            if cy <= 0 {
                cy = attr(tb, NS_FO, "min-height")
                    .and_then(emu)
                    .unwrap_or(457_200);
            }
            if cx <= 0 {
                cx = attr(tb, NS_FO, "min-width")
                    .and_then(emu)
                    .unwrap_or(1_828_800);
            }
            let ins = self.ins.take();
            let lists = std::mem::take(&mut self.lists);
            let saved_space = self.prev_space;
            let saved_pending = std::mem::take(&mut self.pending);
            let saved_break = std::mem::replace(&mut self.break_next, false);
            let link = std::mem::replace(&mut self.in_link, false);
            let mut blocks = Vec::new();
            self.flow(tb, &mut blocks, false);
            if !self.pending.is_empty() {
                let p = std::mem::take(&mut self.pending);
                blocks.push(Block::P(String::new(), p));
            }
            self.in_link = link;
            self.pending = saved_pending;
            self.break_next = saved_break;
            self.prev_space = saved_space;
            self.lists = lists;
            self.ins = ins;
            let fill = match self.graphic_prop(style, NS_FO, "background-color") {
                Some(c) if color(c).is_some() => format!(
                    "<a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill>",
                    color(c).unwrap_or_default()
                ),
                _ => match (
                    self.graphic_prop(style, NS_DRAW, "fill"),
                    self.graphic_prop(style, NS_DRAW, "fill-color")
                        .and_then(color),
                ) {
                    (Some("solid"), Some(c)) => {
                        format!("<a:solidFill><a:srgbClr val=\"{c}\"/></a:solidFill>")
                    }
                    _ => "<a:noFill/>".to_owned(),
                },
            };
            let ln = match self
                .graphic_prop(style, NS_FO, "border")
                .or(self.graphic_prop(style, NS_FO, "border-top"))
            {
                Some(b) if b != "none" => {
                    let mut w = 9525i64;
                    let mut col = "000000".to_owned();
                    for p in b.split_whitespace() {
                        if let Some(c) = color(p) {
                            col = c;
                        } else if let Some(e) = emu(p) {
                            w = e.max(3175);
                        }
                    }
                    format!(
                        "<a:ln w=\"{w}\"><a:solidFill><a:srgbClr val=\"{col}\"/></a:solidFill></a:ln>"
                    )
                }
                _ => "<a:ln><a:noFill/></a:ln>".to_owned(),
            };
            let pad = |side: &str| {
                self.graphic_prop(style, NS_FO, &format!("padding-{side}"))
                    .or(self.graphic_prop(style, NS_FO, "padding"))
                    .and_then(emu)
                    .unwrap_or(0)
                    .max(0)
            };
            let (il, it, ir, ib) = (pad("left"), pad("top"), pad("right"), pad("bottom"));
            (
                format!(
                    "<wps:wsp><wps:cNvSpPr txBox=\"1\"/><wps:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
                     <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom>{fill}{ln}</wps:spPr>\
                     <wps:txbx><w:txbxContent>{}</w:txbxContent></wps:txbx>\
                     <wps:bodyPr rot=\"0\" vert=\"horz\" wrap=\"square\" lIns=\"{il}\" tIns=\"{it}\" rIns=\"{ir}\" bIns=\"{ib}\" anchor=\"t\">{}</wps:bodyPr></wps:wsp>",
                    blocks_xml(&blocks),
                    if grow {
                        "<a:spAutoFit/>"
                    } else {
                        "<a:noAutofit/>"
                    }
                ),
                "http://schemas.microsoft.com/office/word/2010/wordprocessingShape",
            )
        } else {
            if images.is_empty() || has_object {
                self.left_out.objects += 1;
            } else {
                self.left_out.pictures += 1;
            }
            return;
        };
        let (cx, cy) = (cx.max(1), cy.max(1));
        let graphic = format!(
            "<a:graphic><a:graphicData uri=\"{uri}\">{graphic}</a:graphicData></a:graphic>"
        );
        let kind = if uri.ends_with("picture") {
            "Picture"
        } else {
            "Text Box"
        };
        let anchor = attr(f, NS_TEXT, "anchor-type").unwrap_or("paragraph");
        let xml = if anchor == "as-char" {
            format!(
                "<w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">\
                 <wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:docPr id=\"{n}\" name=\"{kind} {n}\"/>{graphic}</wp:inline></w:drawing>"
            )
        } else {
            self.anchor_xml(f, style, anchor, [cx, cy], n, kind, &graphic)
        };
        let r = self.run(tp, None, base_pt, &xml);
        out.push_str(&r);
        self.prev_space = false;
    }

    #[allow(clippy::too_many_arguments)]
    fn anchor_xml(
        &self,
        f: Node,
        style: Option<&str>,
        anchor: &str,
        [cx, cy]: [i64; 2],
        n: u32,
        kind: &str,
        graphic: &str,
    ) -> String {
        let g = |ns, at| self.graphic_prop(style, ns, at);
        let page = anchor == "page";
        let hrel = match g(NS_STYLE, "horizontal-rel") {
            Some("page") => "page",
            Some("page-content") => "margin",
            Some("page-start-margin") | Some("paragraph-start-margin") => "leftMargin",
            Some("page-end-margin") | Some("paragraph-end-margin") => "rightMargin",
            Some("char") => "character",
            Some(_) => "column",
            None if page => "page",
            None => "column",
        };
        let vrel = match g(NS_STYLE, "vertical-rel") {
            Some("page") => "page",
            Some("page-content") => "margin",
            Some("char" | "line" | "baseline" | "text") => "line",
            Some(_) => "paragraph",
            None if page => "page",
            None => "paragraph",
        };
        let x = attr(f, NS_SVG, "x").and_then(emu).unwrap_or(0);
        let y = attr(f, NS_SVG, "y").and_then(emu).unwrap_or(0);
        let hpos = match g(NS_STYLE, "horizontal-pos") {
            Some("left") => "<wp:align>left</wp:align>".to_owned(),
            Some("center") => "<wp:align>center</wp:align>".to_owned(),
            Some("right") => "<wp:align>right</wp:align>".to_owned(),
            Some("inside") => "<wp:align>inside</wp:align>".to_owned(),
            Some("outside") => "<wp:align>outside</wp:align>".to_owned(),
            _ => format!("<wp:posOffset>{x}</wp:posOffset>"),
        };
        let vpos = match g(NS_STYLE, "vertical-pos") {
            Some("top") => "<wp:align>top</wp:align>".to_owned(),
            Some("middle") => "<wp:align>center</wp:align>".to_owned(),
            Some("bottom") => "<wp:align>bottom</wp:align>".to_owned(),
            Some("below") => "<wp:posOffset>0</wp:posOffset>".to_owned(),
            _ => format!("<wp:posOffset>{y}</wp:posOffset>"),
        };
        let wrap_kind = g(NS_STYLE, "wrap").unwrap_or("none");
        // Only an object the text runs through can be behind it.
        let behind = wrap_kind == "run-through" && g(NS_STYLE, "run-through") == Some("background");
        let wrap = match wrap_kind {
            "run-through" => "<wp:wrapNone/>",
            "left" => "<wp:wrapSquare wrapText=\"left\"/>",
            "right" => "<wp:wrapSquare wrapText=\"right\"/>",
            "biggest" => "<wp:wrapSquare wrapText=\"largest\"/>",
            "parallel" | "dynamic" => "<wp:wrapSquare wrapText=\"bothSides\"/>",
            _ => "<wp:wrapTopAndBottom/>",
        };
        let dist = |side: &str| {
            self.graphic_prop(style, NS_FO, &format!("margin-{side}"))
                .and_then(emu)
                .unwrap_or(0)
                .max(0)
        };
        let z = attr(f, NS_DRAW, "z-index")
            .and_then(|z| z.parse::<u32>().ok())
            .unwrap_or(0)
            + 251_658_240;
        format!(
            "<w:drawing><wp:anchor distT=\"{}\" distB=\"{}\" distL=\"{}\" distR=\"{}\" simplePos=\"0\" relativeHeight=\"{z}\" \
             behindDoc=\"{}\" locked=\"0\" layoutInCell=\"1\" allowOverlap=\"1\"><wp:simplePos x=\"0\" y=\"0\"/>\
             <wp:positionH relativeFrom=\"{hrel}\">{hpos}</wp:positionH><wp:positionV relativeFrom=\"{vrel}\">{vpos}</wp:positionV>\
             <wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/>{wrap}\
             <wp:docPr id=\"{n}\" name=\"{kind} {n}\"/><wp:cNvGraphicFramePr/>{graphic}</wp:anchor></w:drawing>",
            dist("top"),
            dist("bottom"),
            dist("left"),
            dist("right"),
            u8::from(behind)
        )
    }

    // ── tables ─────────────────────────────────────────────────────────────

    /// The text width of the current page (for tables without widths).
    fn text_width(&self) -> i32 {
        let layout = self
            .st
            .master(&self.sect.master)
            .and_then(|m| attr(m, NS_STYLE, "page-layout-name"))
            .and_then(|l| self.st.layouts.get(l))
            .and_then(|l| child(*l, NS_STYLE, "page-layout-properties"));
        let get = |at: &str, d: i32| {
            layout
                .and_then(|p| attr(p, NS_FO, at))
                .and_then(twips)
                .unwrap_or(d)
        };
        (get("page-width", 11906) - get("margin-left", 1134) - get("margin-right", 1134)).max(1440)
    }

    fn table(&mut self, t: Node<'a>) -> String {
        let st = self.st;
        let scope = self.scope;
        let tstyle = attr(t, NS_TABLE, "style-name").and_then(|s| st.get(scope, "table", s));
        let tprops = tstyle.and_then(|s| child(s, NS_STYLE, "table-properties"));
        let tp = |at: &str| tprops.and_then(|p| attr(p, NS_STYLE, at).or(attr(p, NS_FO, at)));
        let align = tprops.and_then(|p| attr(p, NS_TABLE, "align"));
        let width = tp("width").and_then(twips);

        // Columns.
        let mut cols: Vec<(Option<i32>, Option<f64>)> = Vec::new();
        fn collect_cols<'b>(
            n: Node<'b>,
            st: &Styles<'b>,
            scope: Scope,
            cols: &mut Vec<(Option<i32>, Option<f64>)>,
        ) {
            for c in n.children().filter(|c| c.is_element()) {
                if is(c, NS_TABLE, "table-column") {
                    let s = attr(c, NS_TABLE, "style-name")
                        .and_then(|s| st.get(scope, "table-column", s))
                        .and_then(|s| child(s, NS_STYLE, "table-column-properties"));
                    let w = s
                        .and_then(|p| attr(p, NS_STYLE, "column-width"))
                        .and_then(twips);
                    let rel = s
                        .and_then(|p| attr(p, NS_STYLE, "rel-column-width"))
                        .and_then(|r| r.trim_end_matches('*').parse::<f64>().ok());
                    let rep: usize = attr(c, NS_TABLE, "number-columns-repeated")
                        .and_then(|r| r.parse().ok())
                        .unwrap_or(1)
                        .clamp(1, MAX_REPEAT);
                    for _ in 0..rep {
                        if cols.len() >= MAX_REPEAT {
                            return;
                        }
                        cols.push((w, rel));
                    }
                } else if matches!(
                    c.tag_name().name(),
                    "table-columns" | "table-header-columns" | "table-column-group"
                ) && c.tag_name().namespace() == Some(NS_TABLE)
                {
                    collect_cols(c, st, scope, cols);
                }
            }
        }
        collect_cols(t, st, scope, &mut cols);

        // Rows (with whether each is a header row).
        let mut rows: Vec<(Node, bool)> = Vec::new();
        fn collect_rows<'b>(n: Node<'b>, header: bool, rows: &mut Vec<(Node<'b>, bool)>) {
            for c in n.children().filter(|c| c.is_element()) {
                if is(c, NS_TABLE, "table-row") {
                    let rep: usize = attr(c, NS_TABLE, "number-rows-repeated")
                        .and_then(|r| r.parse().ok())
                        .unwrap_or(1)
                        .clamp(1, MAX_REPEAT);
                    for _ in 0..rep {
                        if rows.len() >= MAX_REPEAT * 10 {
                            return;
                        }
                        rows.push((c, header));
                    }
                } else if is(c, NS_TABLE, "table-header-rows") {
                    collect_rows(c, true, rows);
                } else if matches!(c.tag_name().name(), "table-rows" | "table-row-group")
                    && c.tag_name().namespace() == Some(NS_TABLE)
                {
                    collect_rows(c, header, rows);
                }
            }
        }
        collect_rows(t, false, &mut rows);

        // Grid width per column.
        let ncols = cols
            .len()
            .max(
                rows.iter()
                    .map(|(r, _)| {
                        r.children()
                            .filter(|c| {
                                is(*c, NS_TABLE, "table-cell")
                                    || is(*c, NS_TABLE, "covered-table-cell")
                            })
                            .map(|c| {
                                attr(c, NS_TABLE, "number-columns-repeated")
                                    .and_then(|r| r.parse::<usize>().ok())
                                    .unwrap_or(1)
                                    .clamp(1, MAX_REPEAT)
                            })
                            .sum::<usize>()
                    })
                    .max()
                    .unwrap_or(1),
            )
            .clamp(1, MAX_REPEAT);
        cols.resize(ncols, (None, None));
        let total = width.unwrap_or_else(|| self.text_width());
        let grid: Vec<i32> = if cols.iter().all(|c| c.0.is_some()) {
            cols.iter().map(|c| c.0.unwrap_or(0).max(1)).collect()
        } else if cols.iter().all(|c| c.1.is_some()) {
            let sum: f64 = cols
                .iter()
                .map(|c| c.1.unwrap_or(0.0))
                .sum::<f64>()
                .max(1.0);
            cols.iter()
                .map(|c| {
                    ((c.1.unwrap_or(0.0) / sum) * f64::from(total))
                        .round()
                        .max(1.0) as i32
                })
                .collect()
        } else {
            vec![(total / ncols as i32).max(1); ncols]
        };
        let sum: i32 = grid.iter().sum();

        let mut x = String::from("<w:tbl><w:tblPr>");
        let _ = write!(x, "<w:tblW w:w=\"{sum}\" w:type=\"dxa\"/>");
        match align {
            Some("center") => x.push_str("<w:jc w:val=\"center\"/>"),
            Some("right") => x.push_str("<w:jc w:val=\"right\"/>"),
            _ => {
                if let Some(ml) = tp("margin-left").and_then(twips).filter(|m| *m != 0) {
                    let _ = write!(x, "<w:tblInd w:w=\"{ml}\" w:type=\"dxa\"/>");
                }
            }
        }
        if let Some(c) = tp("background-color").and_then(color) {
            let _ = write!(
                x,
                "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{c}\"/>"
            );
        }
        x.push_str(
            "<w:tblLayout w:type=\"fixed\"/><w:tblLook w:val=\"0000\"/></w:tblPr><w:tblGrid>",
        );
        for w in &grid {
            let _ = write!(x, "<w:gridCol w:w=\"{w}\"/>");
        }
        x.push_str("</w:tblGrid>");

        // Vertical spans going down: per grid column, the cell's span, rows
        // left and its look.
        let mut down: Vec<Option<(usize, usize, String)>> = vec![None; ncols];
        for (row, header) in rows {
            let rstyle = attr(row, NS_TABLE, "style-name")
                .and_then(|s| st.get(scope, "table-row", s))
                .and_then(|s| child(s, NS_STYLE, "table-row-properties"));
            let mut trpr = String::new();
            if rstyle.and_then(|p| attr(p, NS_FO, "keep-together")) == Some("always") {
                trpr.push_str("<w:cantSplit/>");
            }
            if let Some(h) = rstyle
                .and_then(|p| attr(p, NS_STYLE, "row-height"))
                .and_then(twips)
            {
                let _ = write!(trpr, "<w:trHeight w:val=\"{h}\" w:hRule=\"exact\"/>");
            } else if let Some(h) = rstyle
                .and_then(|p| attr(p, NS_STYLE, "min-row-height"))
                .and_then(twips)
            {
                let _ = write!(trpr, "<w:trHeight w:val=\"{h}\" w:hRule=\"atLeast\"/>");
            }
            if header {
                trpr.push_str("<w:tblHeader/>");
            }
            x.push_str("<w:tr>");
            if !trpr.is_empty() {
                let _ = write!(x, "<w:trPr>{trpr}</w:trPr>");
            }
            let mut col = 0usize;
            let mut skip_until = 0usize;
            let mut cells = 0;
            for cell in row.children().filter(|c| c.is_element()) {
                let covered = is(cell, NS_TABLE, "covered-table-cell");
                if !covered && !is(cell, NS_TABLE, "table-cell") {
                    continue;
                }
                let rep: usize = attr(cell, NS_TABLE, "number-columns-repeated")
                    .and_then(|r| r.parse().ok())
                    .unwrap_or(1)
                    .clamp(1, MAX_REPEAT);
                for _ in 0..rep {
                    if col >= ncols {
                        break;
                    }
                    if col < skip_until {
                        col += 1;
                        continue;
                    }
                    if covered {
                        if let Some((span, left, look)) = down[col].clone() {
                            let w: i32 = grid[col..(col + span).min(ncols)].iter().sum();
                            let _ = write!(
                                x,
                                "<w:tc><w:tcPr><w:tcW w:w=\"{w}\" w:type=\"dxa\"/>{}<w:vMerge/>{look}</w:tcPr><w:p/></w:tc>",
                                if span > 1 {
                                    format!("<w:gridSpan w:val=\"{span}\"/>")
                                } else {
                                    String::new()
                                }
                            );
                            down[col] = (left > 1).then(|| (span, left - 1, look));
                            skip_until = col + span;
                            cells += 1;
                        } else {
                            // A cell covered by a span from the left: done.
                        }
                        col += 1;
                        continue;
                    }
                    let span: usize = attr(cell, NS_TABLE, "number-columns-spanned")
                        .and_then(|r| r.parse().ok())
                        .unwrap_or(1)
                        .clamp(1, ncols - col);
                    let vspan: usize = attr(cell, NS_TABLE, "number-rows-spanned")
                        .and_then(|r| r.parse().ok())
                        .unwrap_or(1)
                        .max(1);
                    let look = self.cell_look(cell);
                    let w: i32 = grid[col..col + span].iter().sum();
                    let mut blocks = Vec::new();
                    self.flow(cell, &mut blocks, false);
                    let _ = write!(
                        x,
                        "<w:tc><w:tcPr><w:tcW w:w=\"{w}\" w:type=\"dxa\"/>{}{}{look}</w:tcPr>{}</w:tc>",
                        if span > 1 {
                            format!("<w:gridSpan w:val=\"{span}\"/>")
                        } else {
                            String::new()
                        },
                        if vspan > 1 {
                            "<w:vMerge w:val=\"restart\"/>"
                        } else {
                            ""
                        },
                        blocks_xml(&blocks)
                    );
                    cells += 1;
                    down[col] = (vspan > 1).then(|| (span, vspan - 1, look));
                    skip_until = col + span;
                    col += 1;
                }
            }
            if cells == 0 {
                x.push_str(
                    "<w:tc><w:tcPr><w:tcW w:w=\"0\" w:type=\"auto\"/></w:tcPr><w:p/></w:tc>",
                );
            }
            x.push_str("</w:tr>");
        }
        if !x.contains("<w:tr>") {
            x.push_str("<w:tr><w:tc><w:p/></w:tc></w:tr>");
        }
        x.push_str("</w:tbl>");
        x
    }

    /// A cell's borders, shading, padding and vertical alignment (the
    /// `w:tcPr` children after `w:vMerge`).
    fn cell_look(&self, cell: Node) -> String {
        let p = attr(cell, NS_TABLE, "style-name")
            .and_then(|s| self.st.get(self.scope, "table-cell", s))
            .and_then(|s| child(s, NS_STYLE, "table-cell-properties"));
        let Some(p) = p else { return String::new() };
        let a = |at: &str| attr(p, NS_FO, at);
        let mut x = String::new();
        let mut b = String::new();
        for (side, w) in [
            ("top", "top"),
            ("left", "left"),
            ("bottom", "bottom"),
            ("right", "right"),
        ] {
            if let Some(v) = a(&format!("border-{side}")).or(a("border"))
                && let Some(xml) = border_xml(w, v, None)
            {
                b.push_str(&xml);
            }
        }
        if !b.is_empty() {
            let _ = write!(x, "<w:tcBorders>{b}</w:tcBorders>");
        }
        if let Some(c) = a("background-color").and_then(color) {
            let _ = write!(
                x,
                "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{c}\"/>"
            );
        }
        let pad: Vec<(&str, Option<i32>)> = ["top", "left", "bottom", "right"]
            .iter()
            .map(|s| {
                (
                    *s,
                    a(&format!("padding-{s}")).or(a("padding")).and_then(twips),
                )
            })
            .collect();
        if pad.iter().any(|p| p.1.is_some()) {
            x.push_str("<w:tcMar>");
            for (s, v) in pad {
                if let Some(v) = v {
                    let _ = write!(x, "<w:{s} w:w=\"{}\" w:type=\"dxa\"/>", v.max(0));
                }
            }
            x.push_str("</w:tcMar>");
        }
        match attr(p, NS_STYLE, "vertical-align") {
            Some("middle") => x.push_str("<w:vAlign w:val=\"center\"/>"),
            Some("bottom") => x.push_str("<w:vAlign w:val=\"bottom\"/>"),
            _ => {}
        }
        x
    }

    // ── sections, headers and footers ──────────────────────────────────────

    fn sect_pr(&mut self, s: &Sect) -> String {
        let st = self.st;
        let master = st.master(&s.master).or_else(|| st.masters.first().copied());
        let layout = master
            .and_then(|m| attr(m, NS_STYLE, "page-layout-name"))
            .and_then(|l| st.layouts.get(l).copied());
        let pp = layout.and_then(|l| child(l, NS_STYLE, "page-layout-properties"));
        let get = |at: &str| pp.and_then(|p| attr(p, NS_FO, at)).and_then(twips);
        let w = get("page-width").unwrap_or(11906).clamp(1440, 31680);
        let h = get("page-height").unwrap_or(16838).clamp(1440, 31680);
        let landscape =
            pp.and_then(|p| attr(p, NS_STYLE, "print-orientation")) == Some("landscape") || w > h;
        let mt = get("margin-top").unwrap_or(1134).max(0);
        let mb = get("margin-bottom").unwrap_or(1134).max(0);
        let ml = get("margin-left").unwrap_or(1134).max(0);
        let mr = get("margin-right").unwrap_or(1134).max(0);
        // ODF margins run to the header; Word's to the body text.
        let hf = |tag: &str| {
            layout
                .and_then(|l| child(l, NS_STYLE, tag))
                .and_then(|s| child(s, NS_STYLE, "header-footer-properties"))
        };
        let hf_len = |p: Option<Node>, space: &str| {
            let p = match p {
                Some(p) => p,
                None => return 0,
            };
            let height = attr(p, NS_FO, "min-height")
                .or(attr(p, NS_SVG, "height"))
                .and_then(twips)
                .unwrap_or(0);
            let gap = attr(p, NS_FO, space).and_then(twips).unwrap_or(0);
            (height + gap).max(0)
        };
        let (refs, title, [has_header, has_footer]) = match master {
            Some(m) => self.headers_for(m),
            None => (String::new(), false, [false, false]),
        };
        let (top, header) = if has_header {
            (mt + hf_len(hf("header-style"), "margin-bottom"), mt)
        } else {
            (mt, (mt / 2).clamp(1, 720))
        };
        let (bottom, footer) = if has_footer {
            (mb + hf_len(hf("footer-style"), "margin-top"), mb)
        } else {
            (mb, (mb / 2).clamp(1, 720))
        };
        let mut x = format!("<w:sectPr>{refs}<w:type w:val=\"{}\"/>", s.kind);
        let (pw, ph) = if landscape {
            (w.max(h), w.min(h))
        } else {
            (w, h)
        };
        let _ = write!(
            x,
            "<w:pgSz w:w=\"{pw}\" w:h=\"{ph}\"{}/>",
            if landscape {
                " w:orient=\"landscape\""
            } else {
                ""
            }
        );
        let _ = write!(
            x,
            "<w:pgMar w:top=\"{top}\" w:right=\"{mr}\" w:bottom=\"{bottom}\" w:left=\"{ml}\" w:header=\"{header}\" w:footer=\"{footer}\" w:gutter=\"0\"/>"
        );
        let fmt = pp
            .and_then(|p| attr(p, NS_STYLE, "num-format"))
            .map(|f| match f {
                "i" => "lowerRoman",
                "I" => "upperRoman",
                "a" => "lowerLetter",
                "A" => "upperLetter",
                _ => "decimal",
            });
        if s.start.is_some() || fmt.is_some_and(|f| f != "decimal") {
            x.push_str("<w:pgNumType");
            if let Some(f) = fmt.filter(|f| *f != "decimal") {
                let _ = write!(x, " w:fmt=\"{f}\"");
            }
            if let Some(n) = s.start {
                let _ = write!(x, " w:start=\"{n}\"");
            }
            x.push_str("/>");
        }
        let cols = s.cols.clone().or_else(|| {
            pp.and_then(|p| child(p, NS_STYLE, "columns"))
                .and_then(cols_xml)
        });
        if let Some(c) = cols {
            x.push_str(&c);
        }
        if title {
            x.push_str("<w:titlePg/>");
        }
        x.push_str("</w:sectPr>");
        x
    }

    /// A master page's header and footer references, written once per
    /// master; and whether the first page has its own.
    fn headers_for(&mut self, m: Node<'a>) -> (String, bool, [bool; 2]) {
        let name = attr(m, NS_STYLE, "name").unwrap_or("").to_owned();
        if let Some(c) = self.hf_cache.get(&name) {
            return c.clone();
        }
        let shown = |m: Node<'a>, tag: &str| {
            child(m, NS_STYLE, tag).filter(|h| attr(*h, NS_STYLE, "display") != Some("false"))
        };
        // A first-page master that hands over to another one.
        let next = attr(m, NS_STYLE, "next-style-name")
            .filter(|n| !n.is_empty() && *n != name)
            .and_then(|n| self.st.master(n));
        let mut refs = String::new();
        let mut title = false;
        let mut own = [false, false];
        for (i, (tag, footer)) in [("header", false), ("footer", true)]
            .into_iter()
            .enumerate()
        {
            let main = next.unwrap_or(m);
            let default = shown(main, tag);
            let left = shown(main, &format!("{tag}-left"));
            let first = match next {
                Some(_) => Some(shown(m, tag)),
                None => shown(m, &format!("{tag}-first")).map(Some),
            };
            if first.is_some() {
                title = true;
            }
            own[i] = default.is_some() || left.is_some() || first.flatten().is_some();
            if default.is_none() && left.is_none() && first.is_none() && !self.any_hf[i] {
                continue;
            }
            let default_id = self.hf_part(footer, default);
            let mut add = |kind: &str, id: &str| {
                let _ = write!(
                    refs,
                    "<w:{} w:type=\"{kind}\" r:id=\"{id}\"/>",
                    if footer {
                        "footerReference"
                    } else {
                        "headerReference"
                    }
                );
            };
            add("default", &default_id);
            if self.even_odd {
                let id = match left {
                    Some(l) => self.hf_part(footer, Some(l)),
                    None => default_id.clone(),
                };
                let mut add = |kind: &str, id: &str| {
                    let _ = write!(
                        refs,
                        "<w:{} w:type=\"{kind}\" r:id=\"{id}\"/>",
                        if footer {
                            "footerReference"
                        } else {
                            "headerReference"
                        }
                    );
                };
                add("even", &id);
            }
            if let Some(f) = first {
                let id = match f {
                    Some(f) if Some(f) != default => self.hf_part(footer, Some(f)),
                    Some(_) => default_id.clone(),
                    None => self.hf_part(footer, None),
                };
                let _ = write!(
                    refs,
                    "<w:{} w:type=\"first\" r:id=\"{id}\"/>",
                    if footer {
                        "footerReference"
                    } else {
                        "headerReference"
                    }
                );
            }
        }
        self.hf_cache.insert(name, (refs.clone(), title, own));
        (refs, title, own)
    }

    /// Write a header or footer part (an empty one for `None`, shared) and
    /// return its relationship id in document.xml.
    fn hf_part(&mut self, footer: bool, el: Option<Node<'a>>) -> String {
        let i = usize::from(footer);
        if el.is_none()
            && let Some(id) = &self.empty_hf[i]
        {
            return id.clone();
        }
        self.hf_count += 1;
        let (tag, file, kind) = if footer {
            ("ftr", format!("footer{}.xml", self.hf_count), "footer")
        } else {
            ("hdr", format!("header{}.xml", self.hf_count), "header")
        };
        let ins = self.ins.take();
        let inner = self.in_part(&file, Scope::Styles, |c| {
            let mut b = Vec::new();
            if let Some(el) = el {
                c.flow(el, &mut b, false);
            }
            if !c.pending.is_empty() {
                let p = std::mem::take(&mut c.pending);
                b.push(Block::P(String::new(), p));
            }
            blocks_xml(&b)
        });
        self.ins = ins;
        self.parts.push((
            format!("word/{file}"),
            format!("{CT}.{kind}+xml"),
            format!("<w:{tag} {NS}>{inner}</w:{tag}>"),
        ));
        let id = self.rel(kind, &file, false);
        if el.is_none() {
            self.empty_hf[i] = Some(id.clone());
        }
        id
    }

    // ── styles, numbering, package ─────────────────────────────────────────

    fn styles_xml(&self) -> String {
        let st = self.st;
        let mut s = format!("<w:styles {NS}><w:docDefaults><w:rPrDefault><w:rPr>");
        let def = st.default_style("paragraph");
        let mut dtp = TextProps::of_style(st, def);
        if dtp.size.is_none() {
            dtp.size = Some(super::style::Size::Pt(12.0));
        }
        s.push_str(&dtp.xml(None, 12.0));
        s.push_str("</w:rPr></w:rPrDefault><w:pPrDefault><w:pPr>");
        let dpp = ParaProps::of_style(def);
        let dppr = ppr_xml(&dpp, &PExtra::default());
        s.push_str(
            dppr.strip_prefix("<w:pPr>")
                .and_then(|x| x.strip_suffix("</w:pPr>"))
                .unwrap_or(""),
        );
        s.push_str("</w:pPr></w:pPrDefault></w:docDefaults>");
        let mut have_normal = false;
        let mut have_link = false;
        for &(family, name) in &st.order {
            let Some(node) = st.named(family, name) else {
                continue;
            };
            let Some(id) = st.word_id(family, name) else {
                continue;
            };
            let (_, wname) = super::style::word_style(family, name, &display_name(node));
            have_normal |= id == "Normal";
            have_link |= id == "Hyperlink";
            let kind = if family == "paragraph" {
                "paragraph"
            } else {
                "character"
            };
            let _ = write!(
                s,
                "<w:style w:type=\"{kind}\"{} w:styleId=\"{}\"><w:name w:val=\"{}\"/>",
                if id == "Normal" {
                    " w:default=\"1\""
                } else {
                    ""
                },
                esc(id),
                esc(&wname)
            );
            let parent = attr(node, NS_STYLE, "parent-style-name");
            if let Some(p) = parent.and_then(|p| st.word_id(family, p)) {
                let _ = write!(s, "<w:basedOn w:val=\"{}\"/>", esc(p));
            }
            if family == "paragraph" {
                if let Some(n) =
                    attr(node, NS_STYLE, "next-style-name").and_then(|n| st.word_id("paragraph", n))
                {
                    let _ = write!(s, "<w:next w:val=\"{}\"/>", esc(n));
                }
                s.push_str("<w:qFormat/>");
                let pp = ParaProps::of_style(Some(node));
                let mut e = PExtra::default();
                if let Some(l) = attr(node, NS_STYLE, "default-outline-level")
                    .and_then(|l| l.parse::<u8>().ok())
                    .filter(|l| (1..=9).contains(l))
                {
                    e.outline = Some(l - 1);
                } else if let Some(l) = id
                    .strip_prefix("Heading")
                    .and_then(|l| l.parse::<u8>().ok())
                {
                    e.outline = Some(l.clamp(1, 9) - 1);
                }
                if st.settings.tabs_relative && pp.tabs.as_ref().is_some_and(|t| !t.is_empty()) {
                    e.tab_shift = st
                        .prop(
                            Scope::Styles,
                            "paragraph",
                            Some(name),
                            "paragraph-properties",
                            NS_FO,
                            "margin-left",
                        )
                        .and_then(twips)
                        .unwrap_or(0);
                }
                s.push_str(&ppr_xml(&pp, &e));
            }
            let base = st.size(Scope::Styles, family, parent);
            let rpr = TextProps::of_style(st, Some(node)).xml(None, base);
            if !rpr.is_empty() {
                let _ = write!(s, "<w:rPr>{rpr}</w:rPr>");
            }
            s.push_str("</w:style>");
        }
        if !have_normal {
            s.push_str(
                "<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:qFormat/></w:style>",
            );
        }
        s.push_str(
            "<w:style w:type=\"character\" w:default=\"1\" w:styleId=\"DefaultParagraphFont\">\
             <w:name w:val=\"Default Paragraph Font\"/><w:uiPriority w:val=\"1\"/><w:semiHidden/></w:style>\
             <w:style w:type=\"table\" w:default=\"1\" w:styleId=\"TableNormal\"><w:name w:val=\"Normal Table\"/>\
             <w:semiHidden/><w:tblPr><w:tblInd w:w=\"0\" w:type=\"dxa\"/><w:tblCellMar><w:top w:w=\"0\" w:type=\"dxa\"/>\
             <w:left w:w=\"108\" w:type=\"dxa\"/><w:bottom w:w=\"0\" w:type=\"dxa\"/><w:right w:w=\"108\" w:type=\"dxa\"/>\
             </w:tblCellMar></w:tblPr></w:style>",
        );
        if !have_link {
            s.push_str(
                "<w:style w:type=\"character\" w:styleId=\"Hyperlink\"><w:name w:val=\"Hyperlink\"/>\
                 <w:rPr><w:color w:val=\"000080\"/><w:u w:val=\"single\"/></w:rPr></w:style>",
            );
        }
        s.push_str("</w:styles>");
        s
    }

    fn numbering_xml(&self) -> Option<String> {
        if self.nums.is_empty() {
            return None;
        }
        let mut x = format!("<w:numbering {NS}>");
        for (i, lv) in self.abstracts.iter().enumerate() {
            let _ = write!(
                x,
                "<w:abstractNum w:abstractNumId=\"{i}\"><w:multiLevelType w:val=\"multilevel\"/>"
            );
            for (l, level) in lv.iter().enumerate() {
                x.push_str(&level.xml(l));
            }
            x.push_str("</w:abstractNum>");
        }
        for (i, (a, overrides)) in self.nums.iter().enumerate() {
            let _ = write!(
                x,
                "<w:num w:numId=\"{}\"><w:abstractNumId w:val=\"{a}\"/>",
                i + 1
            );
            for (l, v) in overrides {
                let _ = write!(
                    x,
                    "<w:lvlOverride w:ilvl=\"{l}\"><w:startOverride w:val=\"{v}\"/></w:lvlOverride>"
                );
            }
            x.push_str("</w:num>");
        }
        x.push_str("</w:numbering>");
        Some(x)
    }

    fn zip(&self, document: &str, core: Option<&str>) -> anyhow::Result<Vec<u8>> {
        let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let decl = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n";
        let mut ct = String::from(
            "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
             <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
             <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
             <Default Extension=\"png\" ContentType=\"image/png\"/>\
             <Default Extension=\"jpeg\" ContentType=\"image/jpeg\"/>\
             <Default Extension=\"gif\" ContentType=\"image/gif\"/>\
             <Default Extension=\"bmp\" ContentType=\"image/bmp\"/>\
             <Default Extension=\"emf\" ContentType=\"image/x-emf\"/>\
             <Default Extension=\"wmf\" ContentType=\"image/x-wmf\"/>",
        );
        let _ = write!(
            ct,
            "<Override PartName=\"/word/document.xml\" ContentType=\"{CT}.document.main+xml\"/>"
        );
        for (name, kind, _) in &self.parts {
            let _ = write!(
                ct,
                "<Override PartName=\"/{name}\" ContentType=\"{kind}\"/>"
            );
        }
        if core.is_some() {
            ct.push_str(
                "<Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>",
            );
        }
        ct.push_str("</Types>");
        let mut root_rels = format!(
            "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
             <Relationship Id=\"rId1\" Type=\"{REL}/officeDocument\" Target=\"word/document.xml\"/>"
        );
        if core.is_some() {
            root_rels.push_str(
                "<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/>",
            );
        }
        root_rels.push_str("</Relationships>");
        let mut files: Vec<(String, Vec<u8>)> = vec![
            (
                "[Content_Types].xml".into(),
                format!("{decl}{ct}").into_bytes(),
            ),
            (
                "_rels/.rels".into(),
                format!("{decl}{root_rels}").into_bytes(),
            ),
            (
                "word/document.xml".into(),
                format!("{decl}{document}").into_bytes(),
            ),
            (
                "word/_rels/document.xml.rels".into(),
                format!("{decl}{}", rels_xml(&self.rels)).into_bytes(),
            ),
        ];
        if let Some(c) = core {
            files.push((
                "docProps/core.xml".into(),
                format!("{decl}{c}").into_bytes(),
            ));
        }
        for (file, own) in &self.part_rels {
            files.push((
                format!("word/_rels/{file}.rels"),
                format!("{decl}{}", rels_xml(own)).into_bytes(),
            ));
        }
        for (name, _, xml) in &self.parts {
            files.push((name.clone(), format!("{decl}{xml}").into_bytes()));
        }
        for (m, (ext, bytes)) in self.media.iter().enumerate() {
            files.push((format!("word/media/image{}.{ext}", m + 1), bytes.clone()));
        }
        for (name, bytes) in files {
            out.start_file(name, opts)?;
            out.write_all(&bytes)?;
        }
        Ok(out.finish()?.into_inner())
    }
}

/// `style:columns` as `w:cols` (`None` for a single column).
fn cols_xml(c: Node) -> Option<String> {
    let n: u32 = attr(c, NS_FO, "column-count")
        .and_then(|n| n.parse().ok())
        .unwrap_or(1);
    if n < 2 {
        return None;
    }
    let n = n.min(45);
    let gap = attr(c, NS_FO, "column-gap").and_then(twips).or_else(|| {
        // Gaps from the columns' own indents.
        let cs: Vec<Node> = c
            .children()
            .filter(|x| is(*x, NS_STYLE, "column"))
            .collect();
        let end = cs
            .first()
            .and_then(|x| attr(*x, NS_FO, "end-indent"))
            .and_then(twips)?;
        let start = cs
            .get(1)
            .and_then(|x| attr(*x, NS_FO, "start-indent"))
            .and_then(twips)?;
        Some(end + start)
    });
    let sep = child(c, NS_STYLE, "column-sep").is_some_and(|s| {
        attr(s, NS_STYLE, "style") != Some("none")
            && attr(s, NS_STYLE, "width")
                .and_then(points)
                .is_none_or(|w| w > 0.0)
    });
    Some(format!(
        "<w:cols w:num=\"{n}\"{} w:space=\"{}\"/>",
        if sep { " w:sep=\"1\"" } else { "" },
        gap.unwrap_or(720).max(0)
    ))
}

fn rels_xml(rels: &[Rel]) -> String {
    let mut x = String::from(
        "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    );
    for r in rels {
        let _ = write!(
            x,
            "<Relationship Id=\"{}\" Type=\"{REL}/{}\" Target=\"{}\"{}/>",
            r.id,
            r.kind,
            esc(&r.target),
            if r.external {
                " TargetMode=\"External\""
            } else {
                ""
            }
        );
    }
    x.push_str("</Relationships>");
    x
}
