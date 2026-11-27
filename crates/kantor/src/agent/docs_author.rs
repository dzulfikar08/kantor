//! Word documents authored from JSON (`kantor docs build`, the MCP
//! `docs_build` tool, and the blocks `kantor docs edit insert` adds; see
//! docs/docs-json.md and docs/docs-json.schema.json).
//!
//! The JSON is written as WordprocessingML with real styles, list and
//! heading numbering, fields (TOC, SEQ, REF, PAGEREF, PAGE…), footnotes,
//! sections, headers and footers, pictures and charts. The docs engine
//! then opens the package, updates every field from Kantor's own layout
//! (the table of contents gets its page numbers) and saves it; the zip is
//! written with fixed times, so the same JSON gives the same bytes.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Map, Value, json};

use super::docs_inline::{Piece, pieces, plain};
use super::docs_look::{
    HfSpec, LOOK_KEYS, Look, PageSpec, ParaFmt, RunFmt, border, get, hf_spec, look_json, object,
    opt_bool, opt_str, page_spec, split_unit, twips,
};
use super::docs_pkg::{
    CT_APP, CT_CHART, CT_CORE, CT_MAIN, CT_THEME, CT_XLSX, NS_A, NS_C, NS_MC, NS_PIC, NS_R, NS_W,
    NS_W14, NS_WP, NS_WPS, Pkg, XML_DECL, esc, max_attr, rel_type, relative, wml_type,
};

/// Blocks one document (or one insert) may have.
pub const MAX_BLOCKS: usize = 20_000;
/// Largest picture file read (bytes).
const MAX_PICTURE: u64 = 100 * 1024 * 1024;
/// EMU per twip.
const EMU_TW: i64 = 635;

/// Keys of a document.
pub fn doc_keys() -> Vec<&'static str> {
    let mut v = vec![
        "preset",
        "lang",
        "accent",
        "properties",
        "cover",
        "toc",
        "blocks",
        "markdown",
        "citations",
    ];
    v.extend_from_slice(LOOK_KEYS);
    v
}

/// Block types.
pub const BLOCK_TYPES: &[&str] = &[
    "heading",
    "paragraph",
    "list",
    "table",
    "picture",
    "chart",
    "callout",
    "quote",
    "page_break",
    "section",
    "toc",
    "list_of_figures",
    "list_of_tables",
    "bibliography",
];

const PARA_KEYS: &[&str] = &[
    "type",
    "text",
    "style",
    "align",
    "space_before",
    "space_after",
    "line_spacing",
    "indent",
    "indent_right",
    "first_line",
    "keep_with_next",
    "keep_together",
    "page_break_before",
    "bold",
    "italic",
    "underline",
    "color",
    "size",
    "font",
    "highlight",
    "caps",
    "small_caps",
];
const HEADING_KEYS: &[&str] = &[
    "type",
    "text",
    "level",
    "id",
    "numbered",
    "page_break_before",
];
const LIST_KEYS: &[&str] = &["type", "kind", "items", "start", "continue"];
const ITEM_KEYS: &[&str] = &["text", "items", "bullets", "numbers"];
const TABLE_KEYS: &[&str] = &[
    "type",
    "columns",
    "header",
    "rows",
    "align",
    "caption",
    "id",
    "width",
    "look",
    "total",
    "header_rows",
    "repeat_header",
];
const CELL_KEYS: &[&str] = &[
    "text", "col_span", "row_span", "align", "bold", "italic", "color", "fill", "bullets", "valign",
];
const PICTURE_KEYS: &[&str] = &[
    "type", "path", "width", "height", "alt", "caption", "id", "align",
];
/// The chart keys docs add to the slides chart's.
const CHART_DOC_KEYS: &[&str] = &["type", "width", "height", "alt", "caption", "id", "align"];
const CALLOUT_KEYS: &[&str] = &["type", "title", "text", "fill", "bar"];
const QUOTE_KEYS: &[&str] = &["type", "text", "source"];
const SECTION_KEYS: &[&str] = &[
    "type",
    "orientation",
    "size",
    "margins",
    "header",
    "footer",
    "page_numbers",
    "columns",
    "start",
    "first_page_header",
];
const TOC_KEYS: &[&str] = &["type", "title", "levels"];
const COVER_KEYS: &[&str] = &[
    "label",
    "title",
    "subtitle",
    "meta",
    "note",
    "logo",
    "logo_width",
];
const PROPS_KEYS: &[&str] = &[
    "title",
    "subject",
    "author",
    "company",
    "keywords",
    "description",
    "date",
    "version",
    "status",
];

/// Document properties (`docProps`), also what header tokens show.
#[derive(Clone, Debug, Default)]
pub struct Props {
    pub title: String,
    pub subject: String,
    pub author: String,
    pub company: String,
    pub keywords: String,
    pub description: String,
    /// `YYYY-MM-DD`.
    pub date: String,
    pub version: String,
    pub status: String,
}

impl Props {
    pub fn read(v: Option<&Value>) -> Result<Self> {
        let Some(v) = v.filter(|v| !v.is_null()) else {
            return Ok(Self::default());
        };
        let o = object(v, "`properties`", PROPS_KEYS)?;
        let s = |k: &str| -> Result<String> { Ok(opt_str(o, k)?.unwrap_or("").trim().to_owned()) };
        let p = Self {
            title: s("title")?,
            subject: s("subject")?,
            author: s("author")?,
            company: s("company")?,
            keywords: s("keywords")?,
            description: s("description")?,
            date: s("date")?,
            version: s("version")?,
            status: s("status")?,
        };
        if !p.date.is_empty() && chrono::NaiveDate::parse_from_str(&p.date, "%Y-%m-%d").is_err() {
            bail!("`properties.date` must be YYYY-MM-DD, not \"{}\"", p.date);
        }
        Ok(p)
    }

    /// The time fields are updated at: the document's date, else a fixed
    /// day (so builds stay byte for byte the same).
    pub fn now(&self) -> chrono::NaiveDateTime {
        chrono::NaiveDate::parse_from_str(&self.date, "%Y-%m-%d")
            .ok()
            .or_else(|| chrono::NaiveDate::from_ymd_opt(2026, 1, 1))
            .and_then(|d| d.and_hms_opt(9, 0, 0))
            .unwrap_or_default()
    }
}

// ── blocks ────────────────────────────────────────────────────────────────

/// A block as an object with its `type` (strings are paragraphs;
/// `{"h1": …}`, `{"p": …}`, `{"bullets": […]}`, `{"numbers": […]}` are
/// shorthands).
pub fn normalize(v: &Value) -> Result<Map<String, Value>> {
    let mut o = match v {
        Value::String(_) | Value::Array(_) => {
            let mut m = Map::new();
            m.insert("type".into(), json!("paragraph"));
            m.insert("text".into(), v.clone());
            return Ok(m);
        }
        Value::Object(o) => o.clone(),
        _ => bail!("a block must be text or an object with `type`"),
    };
    if o.contains_key("type") {
        return Ok(o);
    }
    for (k, lvl) in [("h1", 1), ("h2", 2), ("h3", 3), ("h4", 4)] {
        if let Some(t) = o.remove(k) {
            o.insert("type".into(), json!("heading"));
            o.insert("level".into(), json!(lvl));
            o.insert("text".into(), t);
            return Ok(o);
        }
    }
    if let Some(t) = o.remove("p") {
        o.insert("type".into(), json!("paragraph"));
        o.insert("text".into(), t);
        return Ok(o);
    }
    for (k, kind) in [("bullets", "bullet"), ("numbers", "number")] {
        if let Some(items) = o.remove(k) {
            o.insert("type".into(), json!("list"));
            o.insert("kind".into(), json!(kind));
            o.insert("items".into(), items);
            return Ok(o);
        }
    }
    if let Some(t) = o.remove("bibliography") {
        o.insert("type".into(), json!("bibliography"));
        o.insert("title".into(), t);
        return Ok(o);
    }
    if o.contains_key("text") {
        o.insert("type".into(), json!("paragraph"));
        return Ok(o);
    }
    bail!(
        "a block needs `type` ({}) or a shorthand key (h1-h4, p, bullets, numbers)",
        BLOCK_TYPES.join(", ")
    )
}

fn type_of(o: &Map<String, Value>) -> Result<&str> {
    let t = opt_str(o, "type")?.context("a block needs `type`")?;
    if !BLOCK_TYPES.contains(&t) {
        bail!("`type` \"{t}\" is not one of {}", BLOCK_TYPES.join(", "));
    }
    Ok(t)
}

/// A cross-reference target.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub bookmark: String,
    pub heading: bool,
    /// Heading number (`2.3`; empty when unnumbered) or caption number.
    pub number: String,
    /// Heading text or caption text.
    pub text: String,
    /// `Figure 2` (captions).
    pub label: String,
}

#[derive(Clone, Debug)]
struct Track {
    author: String,
    date: String,
}

#[derive(Clone, Debug, PartialEq)]
struct Sect {
    page: PageSpec,
    header: Option<HfSpec>,
    footer: Option<HfSpec>,
    title_pg: bool,
    num_fmt: Option<String>,
    num_start: Option<u32>,
    cols: u32,
    start: &'static str,
}

#[derive(Clone, Debug)]
enum Item {
    P {
        ppr: String,
        content: String,
        sect: Option<String>,
    },
    Raw(String),
}

/// Numbering ids a writer uses.
#[derive(Clone, Copy, Debug)]
pub struct Nums {
    pub bullets: u32,
    pub bullets_abs: u32,
    pub numbers_abs: u32,
    pub next: u32,
}

/// Writes blocks as WordprocessingML into a package.
pub struct Writer<'a> {
    pub look: &'a Look,
    base: Option<PathBuf>,
    pub pkg: Pkg,
    /// The main document part.
    part: String,
    /// The part relationships go from (the document, or the notes).
    rel_part: String,
    notes_part: String,
    md: bool,
    pub props: Props,
    next_id: u64,
    next_ref: u64,
    next_docpr: u64,
    next_note: i64,
    pub notes: Vec<String>,
    pub nums: Nums,
    /// Numbered lists' `w:num`s: id and start.
    pub lists: Vec<(u32, u32)>,
    last_list: Option<u32>,
    pub targets: HashMap<String, Target>,
    seq: HashMap<String, u32>,
    track: Option<Track>,
    pub warnings: Vec<String>,
    items: Vec<Item>,
    page_break: bool,
    after_table: bool,
    sect: Sect,
    sect_from: usize,
    last_refs: Option<String>,
    first_sect: bool,
    hf_cache: HashMap<String, String>,
    pub used_styles: BTreeSet<String>,
    /// Sections allowed (a whole document, not inserted blocks).
    sections: bool,
    /// Bookmark names already in the document (edits).
    existing: BTreeSet<String>,
    charts: u32,
    /// The document's own ids of built-in styles (edits).
    style_map: HashMap<String, String>,
    /// What `{pages}` counts with.
    pages_field: PagesField,
    /// Sources citations point at, by key (`citations.sources`; in edits,
    /// the document's own source list).
    pub cites: BTreeMap<String, Value>,
    /// Citations written: their Zotero ids are made from it (the same JSON
    /// gives the same file).
    cite_n: u64,
}

/// The field behind the `{pages}` header/footer token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PagesField {
    /// `NUMPAGES`: every page (no front matter).
    All,
    /// `SECTIONPAGES`: the body is one section after the cover and
    /// contents, so its pages are the section's.
    Section,
    /// `PAGEREF` to a bookmark at the end of the document: the last
    /// page's number, which counts from 1 at the body's start (front
    /// matter and a body of several sections).
    LastPage,
}

/// The bookmark at the end of the document that `{pages}` refers to.
const END_BOOKMARK: &str = "_KantorEnd";
/// The `{pages}` result until `build` fills in the number.
const PAGES_MARK: &str = "KANTOR-BODY-PAGES";

impl<'a> Writer<'a> {
    fn new(
        look: &'a Look,
        base: Option<&Path>,
        pkg: Pkg,
        part: &str,
        md: bool,
        props: Props,
        nums: Nums,
    ) -> Self {
        let sect = Sect {
            page: look.page,
            header: look.header.clone(),
            footer: look.footer.clone(),
            title_pg: !look.first_page_header,
            num_fmt: None,
            num_start: None,
            cols: 1,
            start: "nextPage",
        };
        let notes_part = match part.rsplit_once('/') {
            Some((dir, _)) => format!("{dir}/footnotes.xml"),
            None => "footnotes.xml".into(),
        };
        Self {
            look,
            base: base.map(Path::to_path_buf),
            pkg,
            part: part.to_owned(),
            rel_part: part.to_owned(),
            notes_part,
            md,
            props,
            next_id: 1,
            next_ref: 100_000_001,
            next_docpr: 1,
            next_note: 1,
            notes: Vec::new(),
            nums,
            lists: Vec::new(),
            last_list: None,
            targets: HashMap::new(),
            seq: HashMap::new(),
            track: None,
            warnings: Vec::new(),
            items: Vec::new(),
            page_break: false,
            after_table: false,
            sect,
            sect_from: 0,
            last_refs: None,
            first_sect: true,
            hf_cache: HashMap::new(),
            used_styles: BTreeSet::new(),
            sections: true,
            existing: BTreeSet::new(),
            charts: 0,
            style_map: HashMap::new(),
            pages_field: PagesField::All,
            cites: BTreeMap::new(),
            cite_n: 0,
        }
    }

    /// A writer adding blocks to an existing package (`kantor docs edit`):
    /// ids continue after the document's own; `track` makes the new text a
    /// tracked insertion by that author.
    pub fn for_edit(
        look: &'a Look,
        pkg: Pkg,
        base: Option<&Path>,
        md: bool,
        track: Option<(&str, &str)>,
    ) -> Result<Self> {
        let part = pkg.document_part()?;
        let doc = pkg.text(&part).unwrap_or_default();
        let numbering = pkg
            .related(&part, "numbering")
            .and_then(|n| pkg.text(&n))
            .unwrap_or_default();
        let max_abs = max_attr(&numbering, "w:abstractNumId").max(0) as u32;
        let max_num = max_attr(&numbering, "w:numId").max(0) as u32;
        let nums = Nums {
            bullets: max_num + 1,
            bullets_abs: max_abs + 1,
            numbers_abs: max_abs + 2,
            next: max_num + 2,
        };
        let mut w = Self::new(look, base, pkg.clone(), &part, md, Props::default(), nums);
        if let Some(n) = pkg.related(&part, "footnotes") {
            let t = pkg.text(&n).unwrap_or_default();
            w.next_note = max_attr(&t, "w:id").max(0) + 1;
            w.notes_part = n;
        }
        w.next_id = max_attr(&doc, "w:id").max(0) as u64 + 1;
        w.next_docpr = max_attr(&doc, "id").max(0) as u64 + 1;
        let mut rest = doc.as_str();
        while let Some(i) = rest.find("w:name=\"") {
            let s = &rest[i + 8..];
            let end = s.find('"').unwrap_or(0);
            let name = &s[..end];
            if let Some(n) = name
                .strip_prefix("_Ref")
                .and_then(|n| n.parse::<u64>().ok())
            {
                w.next_ref = w.next_ref.max(n + 1);
            }
            w.existing.insert(name.to_owned());
            rest = &s[end..];
        }
        w.track = track.map(|(a, d)| Track {
            author: a.to_owned(),
            date: d.to_owned(),
        });
        w.sections = false;
        // Built-in styles under the document's own ids (a translated
        // Word's "Judul1" for Heading 1).
        if let Some(sp) = pkg.related(&part, "styles") {
            let st = pkg.text(&sp).unwrap_or_default();
            for b in super::docs_look::BUILTIN {
                let pat = format!("<w:name w:val=\"{}\"/>", b.2);
                let found = st.to_ascii_lowercase().find(&pat.to_ascii_lowercase());
                if let Some(i) = found
                    && let Some(s) = st[..i].rfind("<w:style ")
                    && let Some(id) = super::docs_pkg::attr_of(&st[s..i], "w:styleId")
                    && id != b.1
                {
                    w.style_map.insert(b.1.to_owned(), id);
                }
            }
        }
        w.charts = pkg
            .parts
            .keys()
            .filter(|k| k.contains("/charts/chart"))
            .count() as u32;
        Ok(w)
    }

    fn id(&mut self) -> u64 {
        let i = self.next_id;
        self.next_id += 1;
        i
    }

    fn text_width(&self) -> i64 {
        self.sect.page.text_width()
    }

    fn style(&mut self, id: &str) -> String {
        let id = self
            .style_map
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_owned());
        self.used_styles.insert(id.clone());
        id
    }

    // ── collecting targets ────────────────────────────────────────────────

    /// Number the headings, figures and tables and note every block with
    /// an `id`, so references may point forward.
    pub fn collect(&mut self, blocks: &[Map<String, Value>]) -> Result<()> {
        let mut counters = [0u32; 9];
        let mut figures = 0;
        let mut tables = 0;
        fn add(w: &mut Writer, id: Option<&str>, t: Target) -> Result<()> {
            let Some(id) = id else { return Ok(()) };
            if id.trim().is_empty() {
                bail!("`id` is empty");
            }
            if w.targets.insert(id.to_owned(), t).is_some() {
                bail!("two blocks have the id \"{id}\"");
            }
            Ok(())
        }
        for o in blocks {
            match type_of(o)? {
                "heading" => {
                    let level = heading_level(o)?;
                    let numbered = self.look.numbered
                        && level <= 3
                        && opt_bool(o, "numbered")?.unwrap_or(true);
                    let number = if numbered {
                        counters[level - 1] += 1;
                        for c in &mut counters[level..] {
                            *c = 0;
                        }
                        counters[..level]
                            .iter()
                            .map(u32::to_string)
                            .collect::<Vec<_>>()
                            .join(".")
                    } else {
                        String::new()
                    };
                    let text = plain(&pieces(
                        get(o, "text").context("a heading needs `text`")?,
                        self.look,
                        &RunFmt::default(),
                        self.md,
                    )?);
                    let bookmark = self.new_ref();
                    add(
                        self,
                        opt_str(o, "id")?,
                        Target {
                            bookmark,
                            heading: true,
                            number,
                            text,
                            label: String::new(),
                        },
                    )?;
                }
                t @ ("picture" | "chart" | "table") => {
                    let Some(cap) = get(o, "caption") else {
                        if opt_str(o, "id")?.is_some() {
                            bail!(
                                "a {t} with an `id` needs a `caption` (references show its number)"
                            );
                        }
                        continue;
                    };
                    let (label, n) = if t == "table" {
                        tables += 1;
                        (self.look.labels.table.clone(), tables)
                    } else {
                        figures += 1;
                        (self.look.labels.figure.clone(), figures)
                    };
                    let text = plain(&pieces(cap, self.look, &RunFmt::default(), self.md)?);
                    let bookmark = self.new_ref();
                    add(
                        self,
                        opt_str(o, "id")?,
                        Target {
                            bookmark,
                            heading: false,
                            number: n.to_string(),
                            text,
                            label: format!("{label} {n}"),
                        },
                    )?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn new_ref(&mut self) -> String {
        let n = self.next_ref;
        self.next_ref += 1;
        format!("_Ref{n}")
    }

    // ── runs ──────────────────────────────────────────────────────────────

    /// A run, inside a tracked insertion when tracking.
    fn r(&mut self, rpr: &str, inner: &str) -> String {
        format!("<w:r>{rpr}{inner}</w:r>")
    }

    /// A paragraph's content with its runs in tracked insertions when
    /// tracking: consecutive runs share one `w:ins` (a field's runs stay
    /// together), hyperlinks hold their own.
    fn tracked(&mut self, content: String) -> String {
        let Some(t) = self.track.clone() else {
            return content;
        };
        let mut out = String::with_capacity(content.len() + 64);
        let mut group = String::new();
        let flush = |w: &mut Self, group: &mut String, out: &mut String| {
            if !group.is_empty() {
                let id = w.id();
                out.push_str(&format!(
                    "<w:ins w:id=\"{id}\" w:author=\"{}\" w:date=\"{}\">{group}</w:ins>",
                    esc(&t.author),
                    esc(&t.date)
                ));
                group.clear();
            }
        };
        for el in super::docs_edit::children(&content) {
            if el.starts_with("<w:r>") || el.starts_with("<w:r ") {
                group.push_str(&el);
                continue;
            }
            flush(self, &mut group, &mut out);
            if el.starts_with("<w:hyperlink")
                && let Some(open_end) = el.find('>')
                && let Some(close) = el.rfind("</w:hyperlink>")
                && open_end < close
            {
                let inner = self.tracked(el[open_end + 1..close].to_owned());
                out.push_str(&el[..=open_end]);
                out.push_str(&inner);
                out.push_str("</w:hyperlink>");
            } else {
                out.push_str(&el);
            }
        }
        flush(self, &mut group, &mut out);
        out
    }

    /// The paragraph mark's `w:rPr` (a tracked insertion when tracking).
    fn mark(&mut self) -> String {
        match self.track.clone() {
            Some(t) => {
                let id = self.id();
                format!(
                    "<w:rPr><w:ins w:id=\"{id}\" w:author=\"{}\" w:date=\"{}\"/></w:rPr>",
                    esc(&t.author),
                    esc(&t.date)
                )
            }
            None => String::new(),
        }
    }

    /// Text runs: tabs and line breaks become `w:tab` and `w:br`.
    fn text_runs(&mut self, text: &str, rpr: &str) -> String {
        let mut inner = String::new();
        let mut buf = String::new();
        let flush = |buf: &mut String, inner: &mut String| {
            if !buf.is_empty() {
                inner.push_str(&format!("<w:t xml:space=\"preserve\">{}</w:t>", esc(buf)));
                buf.clear();
            }
        };
        for c in text.chars() {
            match c {
                '\n' | '\u{2028}' => {
                    flush(&mut buf, &mut inner);
                    inner.push_str("<w:br/>");
                }
                '\t' => {
                    flush(&mut buf, &mut inner);
                    inner.push_str("<w:tab/>");
                }
                '\r' => {}
                c => buf.push(c),
            }
        }
        flush(&mut buf, &mut inner);
        if inner.is_empty() {
            return String::new();
        }
        self.r(rpr, &inner)
    }

    /// A field's runs: begin, code, separator, result, end.
    pub fn field(&mut self, instr: &str, result: &str, rpr: &str) -> String {
        let mut x = self.r(rpr, "<w:fldChar w:fldCharType=\"begin\"/>");
        x.push_str(&self.r(
            rpr,
            &format!(
                "<w:instrText xml:space=\"preserve\"> {} </w:instrText>",
                esc(instr)
            ),
        ));
        x.push_str(&self.r(rpr, "<w:fldChar w:fldCharType=\"separate\"/>"));
        if !result.is_empty() {
            x.push_str(&self.text_runs(result, rpr));
        }
        x.push_str(&self.r(rpr, "<w:fldChar w:fldCharType=\"end\"/>"));
        x
    }

    fn bookmark(&mut self, name: &str, inner: &str) -> String {
        let id = self.id();
        format!(
            "<w:bookmarkStart w:id=\"{id}\" w:name=\"{}\"/>{inner}<w:bookmarkEnd w:id=\"{id}\"/>",
            esc(name)
        )
    }

    /// The runs of `pieces`.
    fn runs(&mut self, pieces: &[Piece]) -> Result<String> {
        let mut x = String::new();
        for p in pieces {
            match p {
                Piece::Text { text, fmt, link } => {
                    let mut fmt = fmt.clone();
                    if let Some(s) = &fmt.style {
                        self.used_styles.insert(s.clone());
                    }
                    if link.is_some() && fmt.style.is_none() {
                        fmt.style = Some(self.style("Hyperlink"));
                    }
                    let runs = self.text_runs(text, &fmt.xml());
                    match link {
                        None => x.push_str(&runs),
                        Some(l) => {
                            if let Some(name) = l.strip_prefix('#') {
                                let anchor = self
                                    .targets
                                    .get(name)
                                    .map_or_else(|| name.to_owned(), |t| t.bookmark.clone());
                                x.push_str(&format!(
                                    "<w:hyperlink w:anchor=\"{}\" w:history=\"1\">{runs}</w:hyperlink>",
                                    esc(&anchor)
                                ));
                            } else {
                                let url = if l.contains("://") || l.starts_with("mailto:") {
                                    l.clone()
                                } else if l.contains('@') {
                                    format!("mailto:{l}")
                                } else {
                                    format!("https://{l}")
                                };
                                let rp = self.rel_part.clone();
                                let id = self.pkg.add_rel(
                                    &rp,
                                    &rel_type("hyperlink"),
                                    &url,
                                    true,
                                    "rId",
                                );
                                x.push_str(&format!(
                                    "<w:hyperlink r:id=\"{id}\" w:history=\"1\">{runs}</w:hyperlink>"
                                ));
                            }
                        }
                    }
                }
                Piece::Note(note) => {
                    if self.rel_part != self.part {
                        bail!("a footnote can't hold another footnote");
                    }
                    let id = self.next_note;
                    self.next_note += 1;
                    let fs = self.style("FootnoteReference");
                    let rpr = format!("<w:rPr><w:rStyle w:val=\"{fs}\"/></w:rPr>");
                    x.push_str(&self.r(&rpr, &format!("<w:footnoteReference w:id=\"{id}\"/>")));
                    // The note's own text: relationships from the notes part.
                    let saved = std::mem::replace(&mut self.rel_part, self.notes_part.clone());
                    let track = self.track.take();
                    let body = self.runs(note);
                    self.rel_part = saved;
                    self.track = track;
                    let body = body?;
                    let ft = self.style("FootnoteText");
                    self.notes.push(format!(
                        "<w:footnote w:id=\"{id}\"><w:p><w:pPr><w:pStyle w:val=\"{ft}\"/></w:pPr>\
                         <w:r>{rpr}<w:footnoteRef/></w:r><w:r><w:t xml:space=\"preserve\"> </w:t></w:r>{body}</w:p></w:footnote>"
                    ));
                }
                Piece::Ref { id, show, fmt } => {
                    let rpr = fmt.xml();
                    let (instr, result) = match self.targets.get(id).cloned() {
                        Some(t) => {
                            let show = show.as_deref().unwrap_or(if t.heading {
                                if t.number.is_empty() {
                                    "text"
                                } else {
                                    "number"
                                }
                            } else {
                                "label"
                            });
                            match show {
                                "page" => (format!("PAGEREF {} \\h", t.bookmark), "1".to_owned()),
                                "number" if t.heading && !t.number.is_empty() => {
                                    (format!("REF {} \\r \\h", t.bookmark), t.number.clone())
                                }
                                "number" => (format!("REF {} \\h", t.bookmark), t.label.clone()),
                                "full" if !t.heading => {
                                    (format!("REF {} \\h", t.bookmark), t.label.clone())
                                }
                                "label" if t.heading => {
                                    (format!("REF {} \\h", t.bookmark), t.text.clone())
                                }
                                "label" => (format!("REF {} \\h", t.bookmark), t.label.clone()),
                                _ if t.heading => {
                                    (format!("REF {} \\h", t.bookmark), t.text.clone())
                                }
                                _ => (format!("REF {}_text \\h", t.bookmark), t.text.clone()),
                            }
                        }
                        None if self.existing.contains(id) => {
                            let i = if show.as_deref() == Some("page") {
                                "PAGEREF"
                            } else {
                                "REF"
                            };
                            (format!("{i} {id} \\h"), "?".to_owned())
                        }
                        None => bail!(
                            "a reference points at \"{id}\", but no heading, figure or table has that `id`"
                        ),
                    };
                    x.push_str(&self.field(&instr, &result, &rpr));
                    if show.as_deref() == Some("full")
                        && let Some(t) = self.targets.get(id).cloned()
                        && !t.heading
                    {
                        let more = self.text_runs(": ", &rpr);
                        x.push_str(&more);
                        let f = self.field(&format!("REF {}_text \\h", t.bookmark), &t.text, &rpr);
                        x.push_str(&f);
                    }
                }
                Piece::Field { instr, result, fmt } => {
                    let rpr = fmt.xml();
                    x.push_str(&self.field(instr, result, &rpr));
                }
                Piece::Cite {
                    keys,
                    prefix,
                    suffix,
                    no_author,
                    fmt,
                } => {
                    let rpr = fmt.xml();
                    let instr = self.cite_instr(keys, prefix, suffix, *no_author)?;
                    // The text comes from the style when the document is
                    // finished (Refresh Citations).
                    let shown = format!(
                        "({})",
                        keys.iter()
                            .map(|k| k.0.as_str())
                            .collect::<Vec<_>>()
                            .join("; ")
                    );
                    x.push_str(&self.field(&instr, &shown, &rpr));
                }
            }
        }
        Ok(x)
    }

    /// Make `items` (CSL-JSON with an `id`) citable by their ids.
    pub fn add_cite_sources(&mut self, items: &[Value]) {
        for it in items {
            if let Some(id) = it.get("id") {
                let key = match id {
                    Value::String(s) => s.clone(),
                    v => v.to_string(),
                };
                self.cites.insert(key, it.clone());
            }
        }
    }

    /// A Zotero citation field's instruction for sources `keys`.
    fn cite_instr(
        &mut self,
        keys: &[(String, Option<String>)],
        prefix: &Option<String>,
        suffix: &Option<String>,
        no_author: bool,
    ) -> Result<String> {
        let mut items = Vec::new();
        let n = keys.len();
        for (i, (key, loc)) in keys.iter().enumerate() {
            let Some(data) = self.cites.get(key) else {
                let known: Vec<&str> = self.cites.keys().take(8).map(String::as_str).collect();
                if known.is_empty() {
                    bail!(
                        "a citation names \"{key}\", but there are no sources: add `citations`: {{\"sources\": \"refs.bib\"}}"
                    );
                }
                bail!(
                    "a citation names \"{key}\", which isn't among the sources (they have {}{})",
                    known.join(", "),
                    if self.cites.len() > known.len() {
                        ", …"
                    } else {
                        ""
                    }
                );
            };
            let mut it = docs_engine::CiteItem::new(data.clone());
            it.id = key.clone();
            if let Some(l) = loc {
                it.locator = Some(l.clone());
                it.label = Some("page".into());
            }
            if i == 0 {
                it.prefix = prefix.clone();
            }
            if i + 1 == n {
                it.suffix = suffix.clone();
            }
            it.suppress_author = no_author;
            items.push(it);
        }
        self.cite_n += 1;
        let id = docs_engine::cite_edit::random_id(
            0x5EED_C17E ^ self.cite_n.wrapping_mul(0x9E37_79B9_7F4A_7C15),
            8,
        );
        let json = json!({
            "citationID": id,
            "properties": {"formattedCitation": "", "plainCitation": "", "noteIndex": 0},
            "citationItems": docs_engine::citations::csl_citation_items(&items),
            "schema": docs_engine::citations::CSL_CITATION_SCHEMA,
        });
        Ok(format!("ADDIN ZOTERO_ITEM CSL_CITATION {json}"))
    }

    /// A bibliography: a heading (`title`, unnumbered) and Zotero's
    /// bibliography field, filled in when the document is finished.
    fn bibliography(&mut self, o: &Map<String, Value>) -> Result<()> {
        object(
            &Value::Object(o.clone()),
            "a bibliography",
            &["type", "title", "id"],
        )?;
        let title = match get(o, "title") {
            None | Some(Value::Bool(true)) | Some(Value::Null) => {
                Some(self.look.labels.bibliography.clone())
            }
            Some(Value::Bool(false)) => None,
            Some(Value::String(t)) => Some(t.clone()),
            Some(_) => bail!("a bibliography's `title` is text, or false for none"),
        };
        if let Some(t) = title {
            let mut h = Map::new();
            h.insert("type".into(), json!("heading"));
            h.insert("level".into(), json!(1));
            h.insert("text".into(), json!(t));
            h.insert("numbered".into(), json!(false));
            if let Some(id) = get(o, "id") {
                h.insert("id".into(), id.clone());
            }
            self.heading(&h)?;
        }
        let instr = r#"ADDIN ZOTERO_BIBL {"uncited":[],"omitted":[],"custom":[]} CSL_BIBLIOGRAPHY"#;
        let c = self.field(instr, "", "");
        // A hanging indent, as Zotero's bibliography paragraphs have.
        self.para(
            None,
            ParaFmt {
                indent: Some(720),
                first_line: Some(-720),
                ..ParaFmt::default()
            },
            c,
        );
        Ok(())
    }

    /// The runs of JSON paragraph text.
    fn content(&mut self, v: &Value, base: &RunFmt) -> Result<String> {
        let p = pieces(v, self.look, base, self.md)?;
        self.runs(&p)
    }

    // ── paragraphs ────────────────────────────────────────────────────────

    /// A paragraph in style `style` with direct paragraph formatting `fmt`.
    fn para(&mut self, style: Option<&str>, mut fmt: ParaFmt, content: String) {
        if std::mem::take(&mut self.page_break) {
            fmt.page_break_before = Some(true);
        }
        if std::mem::take(&mut self.after_table) && fmt.before.is_none() {
            fmt.before = Some(200);
        }
        let style = style.map(|s| self.style(s));
        let ppr = format!("{}{}", fmt.children(style.as_deref()), self.mark());
        let content = self.tracked(content);
        self.items.push(Item::P {
            ppr,
            content,
            sect: None,
        });
    }

    /// Raw block XML (a table).
    fn raw(&mut self, xml: String) {
        if std::mem::take(&mut self.page_break) {
            let br = self.r("", "<w:br w:type=\"page\"/>");
            let br = self.tracked(br);
            let mark = self.mark();
            self.items.push(Item::P {
                ppr: mark,
                content: br,
                sect: None,
            });
        }
        // Tables side by side merge in Word: keep a paragraph between.
        if matches!(self.items.last(), Some(Item::Raw(_))) {
            self.para(
                None,
                ParaFmt {
                    after: Some(0),
                    ..ParaFmt::default()
                },
                String::new(),
            );
        }
        self.after_table = false;
        self.items.push(Item::Raw(xml));
        self.after_table = true;
    }

    fn block_fmt(&self, o: &Map<String, Value>) -> Result<(RunFmt, ParaFmt)> {
        self.look.formats(o)
    }

    /// Write one block.
    pub fn block(&mut self, o: &Map<String, Value>) -> Result<()> {
        let ty = type_of(o)?;
        match ty {
            "heading" => self.heading(o),
            "paragraph" => {
                object(&Value::Object(o.clone()), "a paragraph", PARA_KEYS)?;
                let style = opt_str(o, "style")?
                    .map(|s| self.look.style_id(s))
                    .transpose()?;
                let (r, p) = self.block_fmt(o)?;
                let text = get(o, "text").context("a paragraph needs `text`")?;
                let c = self.content(text, &r)?;
                self.para(style.as_deref(), p, c);
                Ok(())
            }
            "list" => {
                object(&Value::Object(o.clone()), "a list", LIST_KEYS)?;
                let kind = opt_str(o, "kind")?.unwrap_or("bullet");
                let items = get(o, "items")
                    .and_then(Value::as_array)
                    .context("a list needs `items`: [\"…\", {\"text\": …, \"items\": […]}]")?;
                let start = match get(o, "start") {
                    None => 1,
                    Some(v) => v
                        .as_u64()
                        .filter(|n| (0..=32767).contains(n))
                        .context("`start` must be a whole number")?
                        as u32,
                };
                let cont = opt_bool(o, "continue")?.unwrap_or(false);
                self.list(kind, items, 0, start, cont, None)
            }
            "table" => self.table(o),
            "picture" => self.picture(o),
            "chart" => self.chart(o),
            "callout" => self.callout(o),
            "quote" => {
                object(&Value::Object(o.clone()), "a quote", QUOTE_KEYS)?;
                let text = get(o, "text").context("a quote needs `text`")?;
                let c = self.content(text, &RunFmt::default())?;
                let has_source = opt_str(o, "source")?.is_some();
                self.para(
                    Some("Quote"),
                    ParaFmt {
                        keep_next: has_source.then_some(true),
                        after: has_source.then_some(2),
                        ..ParaFmt::default()
                    },
                    c,
                );
                if let Some(s) = opt_str(o, "source")? {
                    let c = self.content(&json!(format!("\u{2014} {s}")), &RunFmt::default())?;
                    self.para(
                        Some("Quote"),
                        ParaFmt {
                            align: Some("right"),
                            ..ParaFmt::default()
                        },
                        c,
                    );
                }
                Ok(())
            }
            "page_break" => {
                object(&Value::Object(o.clone()), "a page break", &["type"])?;
                self.page_break = true;
                Ok(())
            }
            "section" => self.section(o),
            "toc" | "list_of_figures" | "list_of_tables" => self.toc(o, ty),
            "bibliography" => self.bibliography(o),
            _ => unreachable!(),
        }
    }

    fn heading(&mut self, o: &Map<String, Value>) -> Result<()> {
        object(&Value::Object(o.clone()), "a heading", HEADING_KEYS)?;
        let level = heading_level(o)?;
        let text = get(o, "text").context("a heading needs `text`")?;
        let mut fmt = ParaFmt::default();
        if opt_bool(o, "numbered")? == Some(false) && self.look.numbered && level <= 3 {
            fmt.num = Some((0, 0));
        }
        fmt.page_break_before = opt_bool(o, "page_break_before")?;
        let mut c = self.content(text, &RunFmt::default())?;
        if c.is_empty() {
            self.warnings.push("a heading has no text".into());
        }
        if let Some(t) = opt_str(o, "id")?
            .and_then(|id| self.targets.get(id))
            .cloned()
        {
            c = self.bookmark(&t.bookmark, &c);
        }
        self.para(Some(&format!("Heading{level}")), fmt, c);
        Ok(())
    }

    fn list(
        &mut self,
        kind: &str,
        items: &[Value],
        level: u8,
        start: u32,
        cont: bool,
        style_base: Option<&str>,
    ) -> Result<()> {
        if level > 8 {
            bail!("lists nest at most 9 levels deep");
        }
        if items.is_empty() {
            bail!("a list needs at least one item");
        }
        let num = match kind {
            "bullet" => self.nums.bullets,
            "number" => match (cont, self.last_list) {
                (true, Some(id)) if level == 0 => id,
                _ => {
                    let id = self.nums.next;
                    self.nums.next += 1;
                    self.lists.push((id, start));
                    if level == 0 {
                        self.last_list = Some(id);
                    }
                    id
                }
            },
            other => bail!("`kind` must be bullet or number, not \"{other}\""),
        };
        let style = match (style_base, kind) {
            (Some(s), _) => s.to_owned(),
            (None, "bullet") => {
                ["ListBullet", "ListBullet2", "ListBullet3"][level.min(2) as usize].to_owned()
            }
            (None, _) => {
                ["ListNumber", "ListNumber2", "ListNumber3"][level.min(2) as usize].to_owned()
            }
        };
        for it in items {
            let (text, sub): (Value, Vec<(&str, &Vec<Value>)>) = match it {
                Value::Object(m) => {
                    let o = object(it, "a list item", ITEM_KEYS)?;
                    let mut sub = Vec::new();
                    for (k, kk) in [
                        ("items", kind),
                        ("bullets", "bullet"),
                        ("numbers", "number"),
                    ] {
                        if let Some(v) = get(o, k) {
                            sub.push((
                                kk,
                                v.as_array()
                                    .with_context(|| format!("`{k}` must be an array"))?,
                            ));
                        }
                    }
                    (m.get("text").cloned().unwrap_or(Value::Null), sub)
                }
                other => (other.clone(), Vec::new()),
            };
            let c = self.content(&text, &RunFmt::default())?;
            self.para(
                Some(&style),
                ParaFmt {
                    num: Some((num, level)),
                    ..ParaFmt::default()
                },
                c,
            );
            for (k, v) in sub {
                let base = style_base.map(str::to_owned);
                self.list(k, v, level + 1, 1, false, base.as_deref())?;
            }
        }
        Ok(())
    }

    // ── captions ──────────────────────────────────────────────────────────

    /// A caption paragraph: label and SEQ number (bookmarked for
    /// references), then its text.
    fn caption(&mut self, o: &Map<String, Value>, table: bool) -> Result<()> {
        let Some(cap) = get(o, "caption") else {
            return Ok(());
        };
        let label = if table {
            self.look.labels.table.clone()
        } else {
            self.look.labels.figure.clone()
        };
        let seq: String = label.chars().filter(|c| c.is_alphanumeric()).collect();
        let n = {
            let c = self.seq.entry(seq.clone()).or_insert(0);
            *c += 1;
            *c
        };
        let strong = self.style("Strong");
        let rpr = format!("<w:rPr><w:rStyle w:val=\"{strong}\"/></w:rPr>");
        let mut head = self.text_runs(&format!("{label}\u{a0}"), &rpr);
        head.push_str(&self.field(&format!("SEQ {seq} \\* ARABIC"), &n.to_string(), &rpr));
        let target = opt_str(o, "id")?
            .and_then(|id| self.targets.get(id))
            .cloned();
        if let Some(t) = &target {
            head = self.bookmark(&t.bookmark, &head);
        }
        let mut x = head;
        x.push_str(&self.text_runs(".", &rpr));
        x.push_str(&self.text_runs(" ", ""));
        let mut body = self.content(cap, &RunFmt::default())?;
        if let Some(t) = &target {
            body = self.bookmark(&format!("{}_text", t.bookmark), &body);
        }
        x.push_str(&body);
        self.para(
            Some("Caption"),
            ParaFmt {
                keep_next: table.then_some(true),
                ..ParaFmt::default()
            },
            x,
        );
        Ok(())
    }

    // ── tables ────────────────────────────────────────────────────────────

    fn table(&mut self, o: &Map<String, Value>) -> Result<()> {
        object(&Value::Object(o.clone()), "a table", TABLE_KEYS)?;
        let text_w = self.text_width();
        let width = match get(o, "width") {
            None => text_w,
            Some(v) => pct_or_len(v, text_w, "width")?,
        };
        let mut rows: Vec<Vec<Value>> = Vec::new();
        let mut header_rows = 0;
        if let Some(h) = get(o, "header") {
            let h = h.as_array().context("`header` must be an array of cells")?;
            if h.first().is_some_and(Value::is_array) {
                for r in h {
                    rows.push(
                        r.as_array()
                            .context("`header` rows must be arrays")?
                            .clone(),
                    );
                }
            } else {
                rows.push(h.clone());
            }
            header_rows = rows.len();
        }
        if let Some(v) = get(o, "header_rows") {
            header_rows = v.as_u64().context("`header_rows` must be a whole number")? as usize;
        }
        for r in get(o, "rows")
            .and_then(Value::as_array)
            .context("a table needs `rows`: [[cell, …], …]")?
        {
            rows.push(
                r.as_array()
                    .context("each row must be an array of cells")?
                    .clone(),
            );
        }
        if rows.is_empty() || rows.len() > 5000 {
            bail!("a table needs 1 to 5000 rows");
        }
        if header_rows > rows.len() {
            bail!("`header_rows` is more than the table's rows");
        }
        let ncols = match get(o, "columns") {
            Some(Value::Number(n)) => n.as_u64().unwrap_or(0) as usize,
            Some(Value::Array(a)) => a.len(),
            _ => rows.iter().map(Vec::len).max().unwrap_or(0),
        };
        if !(1..=63).contains(&ncols) {
            bail!("a table needs 1 to 63 columns");
        }
        for (i, r) in rows.iter().enumerate() {
            if r.len() != ncols {
                bail!(
                    "row {} has {} cells, the table {ncols} columns (put null under a span)",
                    i + 1,
                    r.len()
                );
            }
        }
        // Column widths: weights, lengths or percentages.
        let weights: Vec<f64> = match get(o, "columns") {
            Some(Value::Array(a)) => a
                .iter()
                .map(|v| match v {
                    Value::Number(n) => n
                        .as_f64()
                        .filter(|w| *w > 0.0)
                        .context("column widths must be more than 0"),
                    v => Ok(pct_or_len(v, width, "columns")? as f64),
                })
                .collect::<Result<_>>()?,
            _ => vec![1.0; ncols],
        };
        let sum: f64 = weights.iter().sum();
        let mut grid: Vec<i64> = weights
            .iter()
            .map(|w| (w / sum * width as f64).round() as i64)
            .collect();
        let diff = width - grid.iter().sum::<i64>();
        if let Some(last) = grid.last_mut() {
            *last += diff;
        }
        let aligns: Vec<Option<&'static str>> = match get(o, "align") {
            None => vec![None; ncols],
            Some(Value::String(s)) => vec![Some(super::docs_look::align_of(s)?); ncols],
            Some(Value::Array(a)) => {
                if a.len() != ncols {
                    bail!("`align` needs one entry per column");
                }
                a.iter()
                    .map(|v| v.as_str().map(super::docs_look::align_of).transpose())
                    .collect::<Result<_>>()?
            }
            Some(_) => bail!("`align` must be a string or one per column"),
        };
        // Numbers line up on the right unless told otherwise.
        let numeric: Vec<bool> = (0..ncols)
            .map(|c| {
                let body: Vec<&Value> = rows[header_rows..]
                    .iter()
                    .map(|r| &r[c])
                    .filter(|v| !v.is_null())
                    .collect();
                !body.is_empty() && body.iter().all(|v| looks_numeric(v))
            })
            .collect();
        let total = opt_bool(o, "total")?.unwrap_or(false);
        let repeat = opt_bool(o, "repeat_header")?.unwrap_or(true);
        let tl = self.look.table.clone();
        let look = opt_str(o, "look")?.unwrap_or(tl.look.as_str()).to_owned();
        if !["rules", "grid", "header", "plain"].contains(&look.as_str()) {
            bail!("`look` must be rules, grid, header or plain");
        }
        let header_fill = match look.as_str() {
            "header" => Some(
                tl.header_fill
                    .clone()
                    .unwrap_or_else(|| self.look.palette["tint"].clone()),
            ),
            _ => tl.header_fill.clone(),
        };

        // Spans: who covers each position.
        let nrows = rows.len();
        let mut owner = vec![vec![None::<(usize, usize)>; ncols]; nrows];
        let mut spans = vec![vec![(1usize, 1usize); ncols]; nrows];
        for r in 0..nrows {
            for c in 0..ncols {
                if owner[r][c].is_some() {
                    continue;
                }
                let (cs, rs) = match &rows[r][c] {
                    Value::Object(m) => (
                        m.get("col_span")
                            .and_then(Value::as_u64)
                            .unwrap_or(1)
                            .max(1) as usize,
                        m.get("row_span")
                            .and_then(Value::as_u64)
                            .unwrap_or(1)
                            .max(1) as usize,
                    ),
                    _ => (1, 1),
                };
                if c + cs > ncols || r + rs > nrows {
                    bail!(
                        "the cell in row {} column {} spans past the table",
                        r + 1,
                        c + 1
                    );
                }
                spans[r][c] = (cs, rs);
                for rr in r..r + rs {
                    for cc in c..c + cs {
                        if (rr, cc) != (r, c) {
                            if owner[rr][cc].is_some() || !rows[rr][cc].is_null() {
                                bail!(
                                    "row {} column {} is under a span: give null there",
                                    rr + 1,
                                    cc + 1
                                );
                            }
                            owner[rr][cc] = Some((r, c));
                        }
                    }
                }
            }
        }

        if get(o, "caption").is_some() {
            self.caption(o, true)?;
        }
        let tstyle = self.style("DocumentTable");
        let edge = &tl.edge;
        let rule = &tl.rule;
        let tbl_borders = match look.as_str() {
            "grid" => ["top", "left", "bottom", "right", "insideH", "insideV"]
                .iter()
                .map(|s| border(s, 4, rule))
                .collect::<String>(),
            "plain" => String::new(),
            "header" => format!(
                "{}{}",
                border("bottom", 8, edge),
                border("insideH", 4, rule)
            ),
            _ => format!(
                "{}{}{}",
                border("top", 8, edge),
                border("bottom", 8, edge),
                border("insideH", 4, rule)
            ),
        };
        let mut x = format!(
            "<w:tbl><w:tblPr><w:tblStyle w:val=\"{tstyle}\"/><w:tblW w:w=\"{width}\" w:type=\"dxa\"/>\
             <w:tblBorders>{tbl_borders}</w:tblBorders><w:tblLayout w:type=\"fixed\"/>\
             <w:tblCellMar><w:top w:w=\"60\" w:type=\"dxa\"/><w:left w:w=\"100\" w:type=\"dxa\"/>\
             <w:bottom w:w=\"60\" w:type=\"dxa\"/><w:right w:w=\"100\" w:type=\"dxa\"/></w:tblCellMar>\
             <w:tblLook w:val=\"04A0\" w:firstRow=\"{}\" w:lastRow=\"{}\" w:firstColumn=\"0\" w:lastColumn=\"0\" w:noHBand=\"1\" w:noVBand=\"1\"/></w:tblPr><w:tblGrid>",
            u8::from(header_rows > 0),
            u8::from(total)
        );
        for g in &grid {
            x.push_str(&format!("<w:gridCol w:w=\"{g}\"/>"));
        }
        x.push_str("</w:tblGrid>");
        let saved_after = std::mem::take(&mut self.after_table);
        for r in 0..nrows {
            let is_head = r < header_rows;
            let is_total = total && r + 1 == nrows && r >= header_rows;
            let mut trpr = String::from("<w:cantSplit/>");
            if is_head && repeat {
                trpr.push_str("<w:tblHeader/>");
            }
            if let Some(t) = self.track.clone() {
                let id = self.id();
                trpr.push_str(&format!(
                    "<w:ins w:id=\"{id}\" w:author=\"{}\" w:date=\"{}\"/>",
                    esc(&t.author),
                    esc(&t.date)
                ));
            }
            x.push_str(&format!("<w:tr><w:trPr>{trpr}</w:trPr>"));
            let mut c = 0;
            while c < ncols {
                let (orow, ocol) = owner[r][c].unwrap_or((r, c));
                let (cs, rs) = spans[orow][ocol];
                if orow == r && ocol < c {
                    // Under a horizontal span of this row's cell: written there.
                    c += 1;
                    continue;
                }
                let w: i64 = grid[ocol..ocol + cs].iter().sum();
                let mut tcpr = format!("<w:tcW w:w=\"{w}\" w:type=\"dxa\"/>");
                if cs > 1 {
                    tcpr.push_str(&format!("<w:gridSpan w:val=\"{cs}\"/>"));
                }
                if rs > 1 {
                    tcpr.push_str(if orow == r {
                        "<w:vMerge w:val=\"restart\"/>"
                    } else {
                        "<w:vMerge/>"
                    });
                }
                let cell = if orow == r {
                    rows[r][c].clone()
                } else {
                    Value::Null
                };
                let cobj = match &cell {
                    Value::Object(_) => Some(object(&cell, "a cell", CELL_KEYS)?.clone()),
                    _ => None,
                };
                let mut borders = String::new();
                if is_head && r + 1 == header_rows && look != "grid" {
                    borders.push_str(&border("bottom", if look == "plain" { 4 } else { 8 }, edge));
                }
                if is_total {
                    borders.push_str(&border("top", 8, edge));
                }
                if !borders.is_empty() {
                    tcpr.push_str(&format!("<w:tcBorders>{borders}</w:tcBorders>"));
                }
                let fill = match cobj
                    .as_ref()
                    .and_then(|m| m.get("fill"))
                    .and_then(Value::as_str)
                {
                    Some(f) => Some(self.look.color(f)?),
                    None if is_head => header_fill.clone(),
                    None if !is_head && !is_total && (r - header_rows) % 2 == 1 => {
                        tl.banded.clone()
                    }
                    None => None,
                };
                if let Some(f) = fill {
                    tcpr.push_str(&format!(
                        "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{f}\"/>"
                    ));
                }
                let valign = cobj
                    .as_ref()
                    .and_then(|m| m.get("valign"))
                    .and_then(Value::as_str)
                    .map(|v| match v {
                        "top" | "center" | "bottom" => Ok(v),
                        "middle" => Ok("center"),
                        o => Err(anyhow!(
                            "`valign` must be top, middle or bottom, not \"{o}\""
                        )),
                    })
                    .transpose()?
                    .or(if is_head { Some("bottom") } else { None });
                if let Some(v) = valign {
                    tcpr.push_str(&format!("<w:vAlign w:val=\"{v}\"/>"));
                }
                x.push_str(&format!("<w:tc><w:tcPr>{tcpr}</w:tcPr>"));
                let style = if is_head || is_total {
                    "TableHeader"
                } else {
                    "TableText"
                };
                let align = cobj
                    .as_ref()
                    .and_then(|m| m.get("align"))
                    .and_then(Value::as_str)
                    .map(super::docs_look::align_of)
                    .transpose()?
                    .or(aligns[c])
                    .or(if numeric[c] && cs == 1 {
                        Some("right")
                    } else {
                        None
                    });
                let mut fmt_run = RunFmt::default();
                if let Some(m) = &cobj {
                    fmt_run.bold = m.get("bold").and_then(Value::as_bool);
                    fmt_run.italic = m.get("italic").and_then(Value::as_bool);
                    if let Some(col) = m.get("color").and_then(Value::as_str) {
                        fmt_run.color = Some(self.look.color(col)?);
                    }
                }
                let start = self.items.len();
                let text = match &cobj {
                    Some(m) => m.get("text").cloned().unwrap_or(Value::Null),
                    None => cell.clone(),
                };
                let c_runs = self.content(&text, &fmt_run)?;
                let has_bullets = cobj.as_ref().and_then(|m| m.get("bullets")).is_some();
                if !c_runs.is_empty() || !has_bullets {
                    // Header rows keep with the first body row (Word keeps
                    // a row whose paragraphs keep with next on the next
                    // row's page): no header alone at a page's bottom.
                    self.para(
                        Some(style),
                        ParaFmt {
                            align,
                            keep_next: (is_head && nrows > header_rows).then_some(true),
                            ..ParaFmt::default()
                        },
                        c_runs,
                    );
                }
                if let Some(b) = cobj.as_ref().and_then(|m| m.get("bullets")) {
                    let b = b.as_array().context("`bullets` must be an array")?;
                    self.list("bullet", b, 0, 1, false, None)?;
                }
                // The cell's paragraphs, taken back out of the body.
                let cell_items: Vec<Item> = self.items.drain(start..).collect();
                for it in cell_items {
                    x.push_str(&item_xml(&it));
                }
                x.push_str("</w:tc>");
                c += cs;
            }
            x.push_str("</w:tr>");
        }
        x.push_str("</w:tbl>");
        self.after_table = saved_after;
        self.raw(x);
        if header_rows == 0 {
            self.warnings
                .push("a table has no header row (give `header`)".into());
        }
        Ok(())
    }

    // ── pictures and charts ───────────────────────────────────────────────

    fn drawing(&mut self, cx: i64, cy: i64, name: &str, alt: &str, graphic: &str) -> String {
        let id = self.next_docpr;
        self.next_docpr += 1;
        format!(
            "<w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\"><wp:extent cx=\"{cx}\" cy=\"{cy}\"/>\
             <wp:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/><wp:docPr id=\"{id}\" name=\"{} {id}\" descr=\"{}\"/>\
             <wp:cNvGraphicFramePr/><a:graphic xmlns:a=\"{NS_A}\">{graphic}</a:graphic></wp:inline></w:drawing>",
            esc(name),
            esc(alt)
        )
    }

    fn figure(&mut self, o: &Map<String, Value>, run: String) -> Result<()> {
        let align = opt_str(o, "align")?
            .map(super::docs_look::align_of)
            .transpose()?;
        let has_caption = get(o, "caption").is_some();
        let r = self.r("", &run);
        self.para(
            Some("Figure"),
            ParaFmt {
                align,
                keep_next: (!has_caption).then_some(false),
                ..ParaFmt::default()
            },
            r,
        );
        self.caption(o, false)
    }

    fn picture(&mut self, o: &Map<String, Value>) -> Result<()> {
        object(&Value::Object(o.clone()), "a picture", PICTURE_KEYS)?;
        let d = self.picture_run(o)?;
        self.figure(o, d)
    }

    /// A picture's `w:drawing` (its file added to the package).
    fn picture_run(&mut self, o: &Map<String, Value>) -> Result<String> {
        let path = opt_str(o, "path")?.context("a picture needs `path` (a PNG or JPEG file)")?;
        let file = self.resolve_path(path)?;
        let meta =
            std::fs::metadata(&file).with_context(|| format!("can't read {}", file.display()))?;
        if meta.len() > MAX_PICTURE {
            bail!("{} is larger than 100 MB", file.display());
        }
        let bytes =
            std::fs::read(&file).with_context(|| format!("can't read {}", file.display()))?;
        let format = docs_engine::ImageFormat::sniff(&bytes);
        if format == docs_engine::ImageFormat::Other {
            bail!("{} is not a PNG or JPEG picture", file.display());
        }
        let (pw, ph) = docs_engine::object::image_dimensions(&bytes)
            .with_context(|| format!("can't read the size of {}", file.display()))?;
        let max_w = self.text_width();
        let natural_w = (pw as i64 * 1440 / 96).min(max_w);
        let (w, h) = match (get(o, "width"), get(o, "height")) {
            (Some(w), Some(h)) => (pct_or_len(w, max_w, "width")?, twips(h, "height")?),
            (Some(w), None) => {
                let w = pct_or_len(w, max_w, "width")?;
                (w, w * ph as i64 / pw.max(1) as i64)
            }
            (None, Some(h)) => {
                let h = twips(h, "height")?;
                (h * pw as i64 / ph.max(1) as i64, h)
            }
            (None, None) => (natural_w, natural_w * ph as i64 / pw.max(1) as i64),
        };
        if w <= 0 || h <= 0 {
            bail!("a picture's size must be more than 0");
        }
        if w > max_w + 20 {
            self.warnings.push(format!(
                "{path} is wider than the text ({} cm)",
                w as f64 / 567.0
            ));
        }
        let alt = opt_str(o, "alt")?.unwrap_or("");
        if alt.trim().is_empty() {
            self.warnings
                .push(format!("{path} has no alt text (`alt`)"));
        }
        let ext = format.extension();
        let part = self.pkg.free_name("word/media/image", ext);
        self.pkg.set(&part, bytes);
        self.pkg.set_default(ext, format.content_type());
        let rp = self.rel_part.clone();
        let rel = self
            .pkg
            .add_rel(&rp, &rel_type("image"), &relative(&rp, &part), false, "rId");
        let (cx, cy) = (w * EMU_TW, h * EMU_TW);
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let graphic = format!(
            "<a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:pic xmlns:pic=\"{NS_PIC}\">\
             <pic:nvPicPr><pic:cNvPr id=\"0\" name=\"{}\"/><pic:cNvPicPr/></pic:nvPicPr>\
             <pic:blipFill><a:blip r:embed=\"{rel}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
             <pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
             <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData>",
            esc(&name)
        );
        Ok(self.drawing(cx, cy, "Picture", alt, &graphic))
    }

    fn resolve_path(&self, path: &str) -> Result<PathBuf> {
        let text = path.trim();
        let mut p = match text.strip_prefix("~/") {
            Some(rest) => kantor_core::dirs::user_home_dir()
                .context("HOME is not set")?
                .join(rest),
            None => PathBuf::from(text),
        };
        if p.is_relative() {
            match &self.base {
                Some(b) => p = b.join(p),
                None => bail!("give an absolute picture path (got \"{text}\")"),
            }
        }
        Ok(p)
    }

    /// A chart block: the slides chart JSON, written by the same chart
    /// writer, with its data in an embedded workbook.
    fn chart(&mut self, o: &Map<String, Value>) -> Result<()> {
        let mut spec = o.clone();
        for k in CHART_DOC_KEYS {
            spec.remove(*k);
        }
        for k in spec.keys() {
            if !super::slides_chart::CHART_KEYS.contains(&k.as_str())
                || ["x", "y", "w", "h", "name", "type"].contains(&k.as_str())
            {
                bail!(
                    "a chart has an unknown key \"{k}\" (allowed: {}, {})",
                    CHART_DOC_KEYS.join(", "),
                    super::slides_chart::CHART_KEYS
                        .iter()
                        .filter(|k| !["x", "y", "w", "h", "name", "type"].contains(k))
                        .copied()
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
        let max_w = self.text_width();
        let w = get(o, "width")
            .map(|v| pct_or_len(v, max_w, "width"))
            .transpose()?
            .unwrap_or(max_w);
        let h = get(o, "height")
            .map(|v| twips(v, "height"))
            .transpose()?
            .unwrap_or((w as f64 * 0.55).round() as i64);
        if w <= 0 || h <= 0 {
            bail!("a chart's size must be more than 0");
        }
        let mut v = Value::Object(spec);
        self.resolve_colors(&mut v)?;
        if let Some(m) = v.as_object_mut() {
            m.insert("type".into(), json!("chart"));
            m.insert("x".into(), json!(0));
            m.insert("y".into(), json!(0));
            m.insert("w".into(), json!(w * EMU_TW));
            m.insert("h".into(), json!(h * EMU_TW));
            let font = m.entry("font").or_insert_with(|| json!({}));
            if let Some(f) = font.as_object_mut() {
                f.entry("font").or_insert(json!(self.look.font("body")));
                f.entry("size").or_insert(json!(9));
                f.entry("color")
                    .or_insert(json!(format!("#{}", self.look.palette["ink"])));
            }
        }
        let cx = super::slides_author::Ctx::default();
        let plan = super::slides_chart::plan(&v, &cx)?;
        let xml = sheets_engine::data_chart_part(&plan.data)?;
        let workbook = super::slides_chart::workbook(&plan.data)?;
        self.charts += 1;
        let n = self.charts;
        let part = self.pkg.free_name("word/charts/chart", "xml");
        let book = self
            .pkg
            .free_name("word/embeddings/Microsoft_Excel_Worksheet", "xlsx");
        self.pkg.set(&part, xml.into_bytes());
        self.pkg.set(&book, workbook);
        self.pkg.set_override(&part, CT_CHART);
        self.pkg.set_default("xlsx", CT_XLSX);
        let wb_rel = self.pkg.add_rel(
            &part,
            &rel_type("package"),
            &relative(&part, &book),
            false,
            "rId",
        );
        if wb_rel != "rId1" {
            bail!("the chart part already had relationships");
        }
        let rp = self.rel_part.clone();
        let rel = self
            .pkg
            .add_rel(&rp, &rel_type("chart"), &relative(&rp, &part), false, "rId");
        let alt = opt_str(o, "alt")?
            .map(str::to_owned)
            .or_else(|| {
                get(o, "caption")
                    .and_then(|c| pieces(c, self.look, &RunFmt::default(), self.md).ok())
                    .map(|p| plain(&p))
            })
            .unwrap_or_default();
        let graphic = format!(
            "<a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/chart\">\
             <c:chart xmlns:c=\"{NS_C}\" xmlns:r=\"{NS_R}\" r:id=\"{rel}\"/></a:graphicData>"
        );
        let d = self.drawing(w * EMU_TW, h * EMU_TW, &format!("Chart{n}"), &alt, &graphic);
        self.figure(o, d)
    }

    /// Palette names in a chart's colours as `#RRGGBB`.
    fn resolve_colors(&self, v: &mut Value) -> Result<()> {
        match v {
            Value::Object(m) => {
                for (k, x) in m.iter_mut() {
                    if matches!(k.as_str(), "color" | "colors" | "fill") {
                        match x {
                            Value::String(s) => {
                                if let Ok(c) = self.look.color(s) {
                                    *s = format!("#{c}");
                                }
                            }
                            Value::Array(a) => {
                                for c in a.iter_mut() {
                                    if let Value::String(s) = c
                                        && let Ok(h) = self.look.color(s)
                                    {
                                        *s = format!("#{h}");
                                    }
                                }
                            }
                            other => self.resolve_colors(other)?,
                        }
                    } else {
                        self.resolve_colors(x)?;
                    }
                }
            }
            Value::Array(a) => {
                for x in a {
                    self.resolve_colors(x)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    // ── callouts ──────────────────────────────────────────────────────────

    fn callout(&mut self, o: &Map<String, Value>) -> Result<()> {
        object(&Value::Object(o.clone()), "a callout", CALLOUT_KEYS)?;
        let fill = match opt_str(o, "fill")? {
            Some("none") => None,
            Some(c) => Some(self.look.color(c)?),
            None => self.look.callout.fill.clone(),
        };
        let bar = match opt_str(o, "bar")? {
            Some("none") => None,
            Some(c) => Some(self.look.color(c)?),
            None => self.look.callout.bar.clone(),
        };
        let width = self.text_width();
        let start = self.items.len();
        let saved_after = std::mem::take(&mut self.after_table);
        let saved_break = std::mem::take(&mut self.page_break);
        if let Some(t) = get(o, "title") {
            let c = self.content(t, &RunFmt::default())?;
            self.para(Some("CalloutTitle"), ParaFmt::default(), c);
        }
        let text = get(o, "text").context("a callout needs `text`")?;
        // A string is one paragraph; an array holds paragraphs (text, runs,
        // or {"bullets": […]}).
        let paras: Vec<Value> = match text {
            Value::Array(a) => a.clone(),
            other => vec![other.clone()],
        };
        for p in &paras {
            match p {
                Value::Object(m) if m.contains_key("bullets") || m.contains_key("numbers") => {
                    let b = normalize(p)?;
                    let kind = opt_str(&b, "kind")?.unwrap_or("bullet").to_owned();
                    let items = get(&b, "items")
                        .and_then(Value::as_array)
                        .context("`bullets` must be an array")?
                        .clone();
                    self.list(&kind, &items, 0, 1, false, None)?;
                }
                other => {
                    let c = self.content(other, &RunFmt::default())?;
                    self.para(Some("Callout"), ParaFmt::default(), c);
                }
            }
        }
        let inner: String = self.items.drain(start..).map(|i| item_xml(&i)).collect();
        self.after_table = saved_after;
        self.page_break = saved_break;
        let borders = bar.map(|b| border("left", 24, &b)).unwrap_or_default();
        let shd = fill
            .map(|f| format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{f}\"/>"))
            .unwrap_or_default();
        let trpr = match self.track.clone() {
            Some(t) => {
                let id = self.id();
                format!(
                    "<w:trPr><w:cantSplit/><w:ins w:id=\"{id}\" w:author=\"{}\" w:date=\"{}\"/></w:trPr>",
                    esc(&t.author),
                    esc(&t.date)
                )
            }
            None => "<w:trPr><w:cantSplit/></w:trPr>".into(),
        };
        let x = format!(
            "<w:tbl><w:tblPr><w:tblW w:w=\"{width}\" w:type=\"dxa\"/><w:tblBorders>{borders}</w:tblBorders>\
             <w:tblLayout w:type=\"fixed\"/><w:tblCellMar><w:top w:w=\"120\" w:type=\"dxa\"/><w:left w:w=\"220\" w:type=\"dxa\"/>\
             <w:bottom w:w=\"100\" w:type=\"dxa\"/><w:right w:w=\"200\" w:type=\"dxa\"/></w:tblCellMar>\
             <w:tblLook w:val=\"0000\" w:firstRow=\"0\" w:lastRow=\"0\" w:firstColumn=\"0\" w:lastColumn=\"0\" w:noHBand=\"1\" w:noVBand=\"1\"/></w:tblPr>\
             <w:tblGrid><w:gridCol w:w=\"{width}\"/></w:tblGrid><w:tr>{trpr}<w:tc><w:tcPr><w:tcW w:w=\"{width}\" w:type=\"dxa\"/>{shd}</w:tcPr>{inner}</w:tc></w:tr></w:tbl>"
        );
        self.used_styles.insert("TableNormal".into());
        self.raw(x);
        Ok(())
    }

    // ── tables of contents ────────────────────────────────────────────────

    fn toc(&mut self, o: &Map<String, Value>, ty: &str) -> Result<()> {
        object(&Value::Object(o.clone()), "a table of contents", TOC_KEYS)?;
        let default_title = match ty {
            "toc" => self.look.labels.contents.clone(),
            "list_of_figures" => self.look.labels.figures.clone(),
            _ => self.look.labels.tables.clone(),
        };
        let title = match get(o, "title") {
            Some(Value::Bool(false)) => None,
            Some(Value::String(s)) => Some(s.clone()),
            None => Some(default_title),
            Some(_) => bail!("`title` must be text or false"),
        };
        let levels = match get(o, "levels") {
            None => 2,
            Some(v) => v
                .as_u64()
                .filter(|l| (1..=4).contains(l))
                .context("`levels` must be 1 to 4")?,
        };
        if let Some(t) = title {
            let c = self.text_runs(&t, "");
            self.para(Some("TOCHeading"), ParaFmt::default(), c);
        }
        let (instr, style) = match ty {
            "toc" => (format!("TOC \\o \"1-{levels}\" \\h \\z \\u"), "TOC1"),
            "list_of_figures" => {
                let seq: String = self
                    .look
                    .labels
                    .figure
                    .chars()
                    .filter(|c| c.is_alphanumeric())
                    .collect();
                (format!("TOC \\h \\z \\c \"{seq}\""), "TableofFigures")
            }
            _ => {
                let seq: String = self
                    .look
                    .labels
                    .table
                    .chars()
                    .filter(|c| c.is_alphanumeric())
                    .collect();
                (format!("TOC \\h \\z \\c \"{seq}\""), "TableofFigures")
            }
        };
        for l in 2..=levels.min(3) {
            self.used_styles.insert(format!("TOC{l}"));
        }
        let f = self.field(&instr, "Update the field to show the entries.", "");
        self.para(Some(style), ParaFmt::default(), f);
        Ok(())
    }

    // ── cover ─────────────────────────────────────────────────────────────

    /// An accent shape behind the cover's text, fixed to the page.
    fn cover_shape(&mut self, x: i64, y: i64, w: i64, h: i64, color: &str) -> String {
        let id = self.next_docpr;
        self.next_docpr += 1;
        let (x, y, w, h) = (x * EMU_TW, y * EMU_TW, w * EMU_TW, h * EMU_TW);
        let inner = format!(
            "<mc:AlternateContent><mc:Choice Requires=\"wps\"><w:drawing><wp:anchor distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\" \
             simplePos=\"0\" relativeHeight=\"251658240\" behindDoc=\"1\" locked=\"1\" layoutInCell=\"1\" allowOverlap=\"1\">\
             <wp:simplePos x=\"0\" y=\"0\"/><wp:positionH relativeFrom=\"page\"><wp:posOffset>{x}</wp:posOffset></wp:positionH>\
             <wp:positionV relativeFrom=\"page\"><wp:posOffset>{y}</wp:posOffset></wp:positionV><wp:extent cx=\"{w}\" cy=\"{h}\"/>\
             <wp:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/><wp:wrapNone/><wp:docPr id=\"{id}\" name=\"Cover accent {id}\"/>\
             <wp:cNvGraphicFramePr/><a:graphic xmlns:a=\"{NS_A}\"><a:graphicData uri=\"{NS_WPS}\"><wps:wsp><wps:cNvSpPr/>\
             <wps:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{w}\" cy=\"{h}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom>\
             <a:solidFill><a:srgbClr val=\"{color}\"/></a:solidFill><a:ln><a:noFill/></a:ln></wps:spPr><wps:bodyPr/></wps:wsp>\
             </a:graphicData></a:graphic></wp:anchor></w:drawing></mc:Choice></mc:AlternateContent>"
        );
        self.r("", &inner)
    }

    fn cover(&mut self, v: &Value) -> Result<()> {
        let o = object(v, "`cover`", COVER_KEYS)?;
        let page = self.sect.page;
        let kind = match self.look.cover.kind.as_str() {
            "none" => "plain",
            k => k,
        }
        .to_owned();
        let color = self.look.cover.color.clone();
        // Where the text starts, from the page's top edge (twips).
        let (shape, start) = match kind.as_str() {
            "band" => {
                let h = (page.height as f64 * 0.30) as i64;
                (self.cover_shape(0, 0, page.width, h, &color), h + 1000)
            }
            "bar" => (
                self.cover_shape(0, 0, 340, page.height, &color),
                (page.height as f64 * 0.28) as i64,
            ),
            _ => (String::new(), (page.height as f64 * 0.25) as i64),
        };
        // An empty line of exactly that height holds the accent shape and
        // moves the text down (space before is dropped at a page top).
        let room = (start - page.top).clamp(20, 31_000);
        let mark = self.mark();
        let shape = self.tracked(shape);
        self.items.push(Item::P {
            ppr: format!(
                "<w:spacing w:before=\"0\" w:after=\"0\" w:line=\"{room}\" w:lineRule=\"exact\"/>{mark}"
            ),
            content: shape,
            sect: None,
        });
        if let Some(logo) = opt_str(o, "logo")? {
            let mut lo = Map::new();
            lo.insert("path".into(), json!(logo));
            lo.insert(
                "width".into(),
                get(o, "logo_width").cloned().unwrap_or(json!("3.5cm")),
            );
            lo.insert("alt".into(), json!("Logo"));
            let d = self.picture_run(&lo)?;
            let run = self.r("", &d);
            self.para(
                Some("Figure"),
                ParaFmt {
                    align: Some("left"),
                    after: Some(480),
                    keep_next: Some(false),
                    ..ParaFmt::default()
                },
                run,
            );
        }
        let title = match get(o, "title") {
            Some(t) => Some(t.clone()),
            None if !self.props.title.is_empty() => Some(json!(self.props.title)),
            None => None,
        };
        for (key, style) in [
            ("label", "CoverLabel"),
            ("title", "Title"),
            ("subtitle", "Subtitle"),
        ] {
            let v = if key == "title" {
                title.clone()
            } else {
                get(o, key).cloned()
            };
            if let Some(v) = v {
                let c = self.content(&v, &RunFmt::default())?;
                self.para(Some(style), ParaFmt::default(), c);
            }
        }
        let meta: Vec<(String, Value)> = match get(o, "meta") {
            None => Vec::new(),
            Some(Value::Array(a)) => a
                .iter()
                .map(|p| match p {
                    Value::Array(kv) if kv.len() == 2 => {
                        Ok((kv[0].as_str().unwrap_or_default().to_owned(), kv[1].clone()))
                    }
                    Value::String(s) => Ok((String::new(), json!(s))),
                    _ => bail!("`meta` items must be [label, value] or text"),
                })
                .collect::<Result<_>>()?,
            Some(Value::Object(m)) => m.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            Some(_) => bail!("`meta` must be [[label, value], …]"),
        };
        let strong = self.style("Strong");
        for (i, (k, v)) in meta.iter().enumerate() {
            let mut c = String::new();
            if !k.is_empty() {
                c.push_str(
                    &self.text_runs(k, &format!("<w:rPr><w:rStyle w:val=\"{strong}\"/></w:rPr>")),
                );
                c.push_str(&self.r("", "<w:tab/>"));
            }
            c.push_str(&self.content(v, &RunFmt::default())?);
            let fmt = ParaFmt {
                before: (i == 0).then_some(1800),
                tabs: vec!["<w:tab w:val=\"left\" w:pos=\"2268\"/>".into()],
                ..ParaFmt::default()
            };
            self.para(Some("CoverMeta"), fmt, c);
        }
        if let Some(n) = get(o, "note") {
            let c = self.content(n, &RunFmt::default())?;
            self.para(
                Some("CoverMeta"),
                ParaFmt {
                    before: Some(480),
                    ..ParaFmt::default()
                },
                c,
            );
        }
        Ok(())
    }

    // ── sections, headers and footers ─────────────────────────────────────

    /// Header or footer runs for `text` with `{page}`, `{pages}`,
    /// `{title}`… tokens.
    fn hf_runs(&mut self, text: &str) -> String {
        let mut x = String::new();
        let mut rest = text;
        while let Some(i) = rest.find('{') {
            let Some(end) = rest[i..].find('}').map(|e| i + e) else {
                break;
            };
            x.push_str(&self.text_runs(&rest[..i], ""));
            let token = &rest[i + 1..end];
            let p = self.props.clone();
            let first_h1 = self
                .targets
                .values()
                .filter(|t| t.heading && t.number == "1")
                .map(|t| t.text.clone())
                .next()
                .unwrap_or_default();
            match token {
                "page" => x.push_str(&self.field("PAGE", "1", "")),
                "pages" => {
                    let f = match self.pages_field {
                        PagesField::All => self.field("NUMPAGES", "1", ""),
                        PagesField::Section => self.field("SECTIONPAGES", "1", ""),
                        PagesField::LastPage => {
                            self.field(&format!("PAGEREF {END_BOOKMARK}"), PAGES_MARK, "")
                        }
                    };
                    x.push_str(&f);
                }
                "section_pages" => x.push_str(&self.field("SECTIONPAGES", "1", "")),
                "title" => x.push_str(&self.field("TITLE", &p.title, "")),
                "subject" => x.push_str(&self.field("SUBJECT", &p.subject, "")),
                "author" => x.push_str(&self.field("AUTHOR", &p.author, "")),
                "company" => x.push_str(&self.field("DOCPROPERTY Company", &p.company, "")),
                "heading" => x.push_str(&self.field("STYLEREF \"Heading 1\"", &first_h1, "")),
                "date" => x.push_str(&self.text_runs(&p.date, "")),
                "version" => x.push_str(&self.text_runs(&p.version, "")),
                "status" => x.push_str(&self.text_runs(&p.status, "")),
                other => x.push_str(&self.text_runs(&format!("{{{other}}}"), "")),
            }
            rest = &rest[end + 1..];
        }
        x.push_str(&self.text_runs(rest, ""));
        x
    }

    /// A header or footer part showing `spec` (empty when `None`) for text
    /// `width` twips wide; returns its relationship id.
    pub(crate) fn hf_rel(&mut self, kind: &str, spec: Option<&HfSpec>, width: i64) -> String {
        let key = format!("{kind}|{spec:?}|{width}");
        if let Some(id) = self.hf_cache.get(&key) {
            return id.clone();
        }
        let track = self.track.take();
        let style = self.style(if kind == "header" { "Header" } else { "Footer" });
        let mut p = String::new();
        if let Some(s) = spec.filter(|s| !s.is_empty()) {
            let (l, c, r) = (s.left.clone(), s.center.clone(), s.right.clone());
            if let Some(l) = &l {
                p.push_str(&self.hf_runs(l));
            }
            if c.is_some() || r.is_some() {
                p.push_str(&self.r("", "<w:tab/>"));
            }
            if let Some(c) = &c {
                p.push_str(&self.hf_runs(c));
            }
            if let Some(r) = &r {
                p.push_str(&self.r("", "<w:tab/>"));
                p.push_str(&self.hf_runs(r));
            }
        }
        self.track = track;
        let tabs = if width == self.look.page.text_width() {
            String::new()
        } else {
            let lw = self.look.page.text_width();
            format!(
                "<w:tabs><w:tab w:val=\"clear\" w:pos=\"{}\"/><w:tab w:val=\"clear\" w:pos=\"{lw}\"/>\
                 <w:tab w:val=\"center\" w:pos=\"{}\"/><w:tab w:val=\"right\" w:pos=\"{width}\"/></w:tabs>",
                lw / 2,
                width / 2
            )
        };
        let root = if kind == "header" { "hdr" } else { "ftr" };
        let xml = format!(
            "{XML_DECL}<w:{root} xmlns:w=\"{NS_W}\" xmlns:r=\"{NS_R}\"><w:p><w:pPr><w:pStyle w:val=\"{style}\"/>{tabs}</w:pPr>{p}</w:p></w:{root}>"
        );
        let part = self.pkg.free_name(&format!("word/{kind}"), "xml");
        self.pkg.set_text(&part, xml);
        self.pkg.set_override(&part, &wml_type(kind));
        let doc = self.part.clone();
        let id = self
            .pkg
            .add_rel(&doc, &rel_type(kind), &relative(&doc, &part), false, "rId");
        self.hf_cache.insert(key, id.clone());
        id
    }

    /// The `w:sectPr` of section `s`.
    fn sect_pr(&mut self, s: &Sect) -> String {
        let width = s.page.text_width();
        let key = format!("{:?}|{:?}|{width}|{}", s.header, s.footer, s.title_pg);
        let mut x = String::from("<w:sectPr>");
        if self.first_sect || self.last_refs.as_deref() != Some(&key) {
            let h = self.hf_rel("header", s.header.as_ref(), width);
            let f = self.hf_rel("footer", s.footer.as_ref(), width);
            x.push_str(&format!(
                "<w:headerReference w:type=\"default\" r:id=\"{h}\"/>"
            ));
            if s.title_pg {
                let hf = self.hf_rel("header", None, width);
                x.push_str(&format!(
                    "<w:headerReference w:type=\"first\" r:id=\"{hf}\"/>"
                ));
            }
            x.push_str(&format!(
                "<w:footerReference w:type=\"default\" r:id=\"{f}\"/>"
            ));
            if s.title_pg {
                let ff = self.hf_rel("footer", None, width);
                x.push_str(&format!(
                    "<w:footerReference w:type=\"first\" r:id=\"{ff}\"/>"
                ));
            }
            self.last_refs = Some(key);
        }
        self.first_sect = false;
        if s.start != "nextPage" {
            x.push_str(&format!("<w:type w:val=\"{}\"/>", s.start));
        }
        let p = &s.page;
        x.push_str(&format!(
            "<w:pgSz w:w=\"{}\" w:h=\"{}\"{}/><w:pgMar w:top=\"{}\" w:right=\"{}\" w:bottom=\"{}\" w:left=\"{}\" \
             w:header=\"{}\" w:footer=\"{}\" w:gutter=\"0\"/>",
            p.width,
            p.height,
            if p.landscape { " w:orient=\"landscape\"" } else { "" },
            p.top,
            p.right,
            p.bottom,
            p.left,
            p.header,
            p.footer
        ));
        if s.num_fmt.is_some() || s.num_start.is_some() {
            x.push_str("<w:pgNumType");
            if let Some(f) = &s.num_fmt {
                x.push_str(&format!(" w:fmt=\"{f}\""));
            }
            if let Some(n) = s.num_start {
                x.push_str(&format!(" w:start=\"{n}\""));
            }
            x.push_str("/>");
        }
        if s.cols > 1 {
            x.push_str(&format!("<w:cols w:num=\"{}\" w:space=\"567\"/>", s.cols));
        } else {
            x.push_str("<w:cols w:space=\"720\"/>");
        }
        if s.title_pg {
            x.push_str("<w:titlePg/>");
        }
        x.push_str("</w:sectPr>");
        x
    }

    /// End the current section and start `next`.
    fn break_section(&mut self, next: Sect) {
        if self.items.len() == self.sect_from {
            // Nothing in it yet: the new settings take its place.
            self.sect = next;
            return;
        }
        let cur = self.sect.clone();
        let pr = self.sect_pr(&cur);
        if !matches!(self.items.last(), Some(Item::P { sect: None, .. })) {
            self.para(
                None,
                ParaFmt {
                    after: Some(0),
                    ..ParaFmt::default()
                },
                String::new(),
            );
        }
        if let Some(Item::P { sect, .. }) = self.items.last_mut() {
            *sect = Some(pr);
        }
        self.page_break = false;
        self.after_table = false;
        self.sect = next;
        self.sect_from = self.items.len();
    }

    fn section(&mut self, o: &Map<String, Value>) -> Result<()> {
        object(&Value::Object(o.clone()), "a section", SECTION_KEYS)?;
        if !self.sections {
            bail!("inserted blocks can't start sections (change sections with page_setup)");
        }
        let mut next = self.sect.clone();
        next.num_start = None;
        next.num_fmt = None;
        let mut po = Map::new();
        for k in ["size", "orientation", "margins"] {
            if let Some(v) = get(o, k) {
                po.insert(k.into(), v.clone());
            }
        }
        next.page = page_spec(&Value::Object(po), self.sect.page)?;
        if o.contains_key("header") {
            next.header = hf_spec(o.get("header"), "header")?;
        }
        if o.contains_key("footer") {
            next.footer = hf_spec(o.get("footer"), "footer")?;
        }
        if let Some(b) = opt_bool(o, "first_page_header")? {
            next.title_pg = !b;
        }
        if let Some(n) = get(o, "page_numbers") {
            let no = object(n, "`page_numbers`", &["start", "format"])?;
            if let Some(s) = get(no, "start") {
                next.num_start = Some(
                    s.as_u64()
                        .context("`page_numbers.start` must be a whole number")?
                        as u32,
                );
            }
            if let Some(f) = opt_str(no, "format")? {
                const F: &[&str] = &[
                    "decimal",
                    "lowerRoman",
                    "upperRoman",
                    "lowerLetter",
                    "upperLetter",
                ];
                if !F.contains(&f) {
                    bail!("`page_numbers.format` must be one of {}", F.join(", "));
                }
                next.num_fmt = Some(f.to_owned());
            }
        }
        if let Some(c) = get(o, "columns") {
            next.cols = c
                .as_u64()
                .filter(|c| (1..=6).contains(c))
                .context("`columns` must be 1 to 6")? as u32;
        }
        next.start = match opt_str(o, "start")?.unwrap_or("next_page") {
            "next_page" => "nextPage",
            "continuous" => "continuous",
            "odd_page" => "oddPage",
            "even_page" => "evenPage",
            other => bail!(
                "`start` must be next_page, continuous, odd_page or even_page, not \"{other}\""
            ),
        };
        self.break_section(next);
        Ok(())
    }

    /// The body XML of the blocks written so far (for inserting).
    pub fn body_xml(&mut self) -> String {
        if self.page_break {
            self.page_break = false;
            let br = self.r("", "<w:br w:type=\"page\"/>");
            let br = self.tracked(br);
            let mark = self.mark();
            self.items.push(Item::P {
                ppr: mark,
                content: br,
                sect: None,
            });
        }
        self.items.iter().map(item_xml).collect()
    }
}

fn item_xml(it: &Item) -> String {
    match it {
        Item::Raw(x) => x.clone(),
        Item::P { ppr, content, sect } => {
            let sect = sect.as_deref().unwrap_or("");
            if ppr.is_empty() && sect.is_empty() {
                format!("<w:p>{content}</w:p>")
            } else {
                format!("<w:p><w:pPr>{ppr}{sect}</w:pPr>{content}</w:p>")
            }
        }
    }
}

fn heading_level(o: &Map<String, Value>) -> Result<usize> {
    match get(o, "level") {
        None => Ok(1),
        Some(v) => v
            .as_u64()
            .filter(|l| (1..=4).contains(l))
            .map(|l| l as usize)
            .context("a heading's `level` must be 1 to 4"),
    }
}

/// A width: `"60%"` of `full`, or a length.
fn pct_or_len(v: &Value, full: i64, key: &str) -> Result<i64> {
    if let Value::String(s) = v
        && let Some((n, "%")) = split_unit(s)
    {
        if !(1.0..=100.0).contains(&n) {
            bail!("`{key}` must be 1% to 100%");
        }
        return Ok((full as f64 * n / 100.0).round() as i64);
    }
    twips(v, key)
}

/// A table cell that holds a number (`1200`, `"1,200"`, `"45%"`,
/// `"$3.2M"`, `"-4.5"`, `"Rp 12.000"`).
fn looks_numeric(v: &Value) -> bool {
    let s = match v {
        Value::Number(_) => return true,
        Value::String(s) => s.trim(),
        Value::Object(m) => match m.get("text") {
            Some(t) => return looks_numeric(t),
            None => return false,
        },
        _ => return false,
    };
    let core = s
        .trim_start_matches(['$', '€', '£', '¥', '+', '-', '−', '(', ' '])
        .trim_start_matches("Rp")
        .trim_start_matches("IDR")
        .trim()
        .trim_end_matches(['%', ')', 'k', 'K', 'M', 'B', 'x', '×', ' ']);
    !core.is_empty()
        && core.chars().next().is_some_and(|c| c.is_ascii_digit())
        && core
            .chars()
            .all(|c| c.is_ascii_digit() || ",. ".contains(c))
}

/// What a build wrote.
#[derive(Clone, Debug)]
pub struct Built {
    pub bytes: Vec<u8>,
    pub pages: usize,
    pub warnings: Vec<String>,
}

fn core_xml(p: &Props) -> String {
    let date = if p.date.is_empty() {
        String::new()
    } else {
        format!(
            "<dcterms:created xsi:type=\"dcterms:W3CDTF\">{d}T00:00:00Z</dcterms:created>\
             <dcterms:modified xsi:type=\"dcterms:W3CDTF\">{d}T00:00:00Z</dcterms:modified>",
            d = esc(&p.date)
        )
    };
    let el = |name: &str, v: &str| {
        if v.is_empty() {
            String::new()
        } else {
            format!("<{name}>{}</{name}>", esc(v))
        }
    };
    format!(
        "{XML_DECL}<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" \
         xmlns:dcmitype=\"http://purl.org/dc/dcmitype/\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\
         {}{}{}{}{}{}{date}</cp:coreProperties>",
        el("dc:title", &p.title),
        el("dc:subject", &p.subject),
        el("dc:creator", &p.author),
        el("cp:keywords", &p.keywords),
        el("dc:description", &p.description),
        el("cp:contentStatus", &p.status),
    )
}

fn app_xml(p: &Props) -> String {
    let company = if p.company.is_empty() {
        String::new()
    } else {
        format!("<Company>{}</Company>", esc(&p.company))
    };
    format!(
        "{XML_DECL}<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\" \
         xmlns:vt=\"http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes\"><Application>Kantor</Application>\
         {company}</Properties>"
    )
}

/// The `<w:document …>` start tag of new documents.
pub fn document_root() -> String {
    format!(
        "<w:document xmlns:mc=\"{NS_MC}\" xmlns:o=\"urn:schemas-microsoft-com:office:office\" xmlns:r=\"{NS_R}\" \
         xmlns:v=\"urn:schemas-microsoft-com:vml\" xmlns:wp=\"{NS_WP}\" xmlns:w=\"{NS_W}\" xmlns:w14=\"{NS_W14}\" \
         xmlns:wps=\"{NS_WPS}\" xmlns:a=\"{NS_A}\" xmlns:pic=\"{NS_PIC}\" xmlns:c=\"{NS_C}\" mc:Ignorable=\"w14\">"
    )
}

/// The footnotes part with Word's separators and `notes`.
pub fn footnotes_xml(notes: &[String]) -> String {
    let sep = |ty: &str, id: i32, el: &str| {
        format!(
            "<w:footnote w:type=\"{ty}\" w:id=\"{id}\"><w:p><w:pPr><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr>\
             <w:r><w:{el}/></w:r></w:p></w:footnote>"
        )
    };
    format!(
        "{XML_DECL}<w:footnotes xmlns:w=\"{NS_W}\" xmlns:r=\"{NS_R}\">{}{}{}</w:footnotes>",
        sep("separator", -1, "separator"),
        sep("continuationSeparator", 0, "continuationSeparator"),
        notes.concat()
    )
}

/// Build a document from its JSON (`base`: where relative picture paths
/// start). The same JSON gives the same bytes.
pub fn build(json: &Value, base: Option<&Path>) -> Result<Built> {
    let keys = doc_keys();
    let o = object(json, "the document", &keys)?;
    let lang = opt_str(o, "lang")?.unwrap_or("en-US").trim().to_owned();
    if lang.is_empty() || !lang.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        bail!("`lang` must be a language tag such as en-US or id-ID");
    }
    let look = Look::read(&look_json(o)?, &lang)?;
    let props = Props::read(o.get("properties"))?;
    let md = opt_bool(o, "markdown")?.unwrap_or(true);
    let cite = get(o, "citations")
        .map(|c| cite_setup(c, base, &lang, json))
        .transpose()
        .context("citations")?;
    let blocks: Vec<Map<String, Value>> = match get(o, "blocks") {
        None => Vec::new(),
        Some(Value::Array(a)) => {
            if a.len() > MAX_BLOCKS {
                bail!("more than {MAX_BLOCKS} blocks");
            }
            a.iter()
                .enumerate()
                .map(|(i, b)| normalize(b).with_context(|| format!("block {}", i + 1)))
                .collect::<Result<_>>()?
        }
        Some(_) => bail!("`blocks` must be an array"),
    };
    let nums = Nums {
        bullets: super::docs_look::NUM_BULLETS,
        bullets_abs: 2,
        numbers_abs: 3,
        next: super::docs_look::NUM_LISTS,
    };
    let mut w = Writer::new(
        &look,
        base,
        Pkg::new(),
        "word/document.xml",
        md,
        props.clone(),
        nums,
    );
    if let Some(c) = &cite {
        w.add_cite_sources(&c.sources);
    }
    w.collect(&blocks)?;

    // Front matter: the cover and the table of contents, unnumbered, no
    // header or footer; the body starts on a new page at 1.
    let cover = get(o, "cover");
    let toc = get(o, "toc").filter(|t| !matches!(t, Value::Bool(false)));
    let body_sect = w.sect.clone();
    let front = cover.is_some() || toc.is_some();
    // `{pages}`: the body's pages, numbered from 1 after the front matter.
    if front {
        let sections = blocks
            .iter()
            .any(|b| b.get("type").and_then(Value::as_str) == Some("section"));
        w.pages_field = if sections {
            PagesField::LastPage
        } else {
            PagesField::Section
        };
    }
    if front {
        w.sect = Sect {
            header: None,
            footer: None,
            title_pg: false,
            ..body_sect.clone()
        };
        if let Some(c) = cover {
            w.cover(c).context("cover")?;
        }
        if let Some(t) = toc {
            let mut to = match t {
                Value::Bool(true) => Map::new(),
                Value::Object(m) => m.clone(),
                _ => bail!("`toc` must be true or {{title, levels}}"),
            };
            to.insert("type".into(), json!("toc"));
            if cover.is_some() {
                w.page_break = true;
            }
            w.toc(&to, "toc").context("toc")?;
        }
        w.break_section(Sect {
            num_start: Some(1),
            ..body_sect
        });
    }
    for (i, b) in blocks.iter().enumerate() {
        let what = match get(b, "text").and_then(Value::as_str) {
            Some(t) => format!(
                "block {} (\"{}\")",
                i + 1,
                t.chars().take(40).collect::<String>()
            ),
            None => format!("block {} ({})", i + 1, opt_str(b, "type")?.unwrap_or("")),
        };
        w.block(b).with_context(|| what)?;
    }
    if w.items.len() == w.sect_from || !matches!(w.items.last(), Some(Item::P { .. })) {
        w.para(
            None,
            ParaFmt {
                after: Some(0),
                ..ParaFmt::default()
            },
            String::new(),
        );
    }
    if w.pages_field == PagesField::LastPage {
        let mark = w.bookmark(END_BOOKMARK, "");
        if let Some(Item::P { content, .. }) = w.items.last_mut() {
            content.push_str(&mark);
        }
    }
    let last = w.sect.clone();
    let final_sect = w.sect_pr(&last);
    let body = w.body_xml();

    // The package.
    let mut pkg = std::mem::take(&mut w.pkg);
    let doc = "word/document.xml";
    pkg.set_text(
        doc,
        format!(
            "{XML_DECL}{}<w:body>{body}{final_sect}</w:body></w:document>",
            document_root()
        ),
    );
    pkg.set_override(doc, CT_MAIN);
    pkg.add_rel("", &rel_type("officeDocument"), doc, false, "rId");
    pkg.set_text("docProps/core.xml", core_xml(&props));
    pkg.set_override("docProps/core.xml", CT_CORE);
    pkg.add_rel(
        "",
        "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties",
        "docProps/core.xml",
        false,
        "rId",
    );
    pkg.set_text("docProps/app.xml", app_xml(&props));
    pkg.set_override("docProps/app.xml", CT_APP);
    pkg.add_rel(
        "",
        &rel_type("extended-properties"),
        "docProps/app.xml",
        false,
        "rId",
    );
    let lists: Vec<u32> = w.lists.iter().map(|l| l.1).collect();
    let parts: [(&str, String, String); 5] = [
        (
            "styles",
            super::docs_look::styles_xml(&look),
            wml_type("styles"),
        ),
        (
            "settings",
            super::docs_look::settings_xml(&look, !w.notes.is_empty(), false),
            wml_type("settings"),
        ),
        (
            "numbering",
            super::docs_look::numbering_xml(&look, &lists),
            wml_type("numbering"),
        ),
        (
            "fontTable",
            super::docs_look::font_table_xml(&look),
            wml_type("fontTable"),
        ),
        (
            "theme",
            super::docs_look::theme_xml(&look),
            CT_THEME.to_owned(),
        ),
    ];
    for (kind, xml, ct) in parts {
        let name = if kind == "theme" {
            "word/theme/theme1.xml".to_owned()
        } else {
            format!("word/{kind}.xml")
        };
        pkg.set_text(&name, xml);
        pkg.set_override(&name, &ct);
        pkg.add_rel(doc, &rel_type(kind), &relative(doc, &name), false, "rId");
    }
    if !w.notes.is_empty() {
        let name = "word/footnotes.xml";
        pkg.set_text(name, footnotes_xml(&w.notes));
        pkg.set_override(name, &wml_type("footnotes"));
        pkg.add_rel(doc, &rel_type("footnotes"), "footnotes.xml", false, "rId");
    }
    let warnings = std::mem::take(&mut w.warnings);
    let now = props.now();
    let (mut bytes, pages) = finish_with(pkg.to_bytes()?, now, cite.as_ref())?;
    if w.pages_field == PagesField::LastPage {
        bytes = fill_last_page(bytes)?;
    }
    Ok(Built {
        bytes,
        pages,
        warnings,
    })
}

/// What a document's `citations` says: its sources, style and language.
pub struct CiteSetup {
    /// The style id written into the document (Zotero's preference).
    pub style: String,
    /// A `.csl` file's style (else the id is looked up).
    pub csl: Option<kantor_cite::CslStyle>,
    pub locale: String,
    /// CSL-JSON items, each with an `id` (the key citations use).
    pub sources: Vec<Value>,
    /// For Zotero's session id: the same JSON gives the same file.
    pub seed: u64,
}

/// Read `citations`: `{"sources": "refs.bib" | ["a.bib", …] | [CSL-JSON
/// items], "style": "apa" | id | "file.csl", "locale": "id-ID"}`.
fn cite_setup(v: &Value, base: Option<&Path>, lang: &str, all: &Value) -> Result<CiteSetup> {
    let o = object(v, "`citations`", &["sources", "style", "locale"])?;
    let path = |p: &str| -> PathBuf {
        let p = PathBuf::from(p);
        match base {
            Some(b) if p.is_relative() => b.join(p),
            _ => p,
        }
    };
    let mut sources = Vec::new();
    let add_value = |v: &Value, sources: &mut Vec<Value>| -> Result<()> {
        match v {
            Value::String(p) => {
                let items = kantor_cite::import::read_file(&path(p))?;
                sources.extend(items);
            }
            Value::Object(m) if m.contains_key("items") => {
                sources.extend(kantor_cite::import::from_csl_json(&v.to_string())?);
            }
            Value::Object(m) => {
                if !m.contains_key("id") {
                    bail!("a source needs an `id` (the key citations use)");
                }
                sources.push(v.clone());
            }
            _ => bail!("`sources` lists files (.bib, .ris, .json) or CSL-JSON items"),
        }
        Ok(())
    };
    match get(o, "sources") {
        None => {}
        Some(Value::Array(a)) => {
            for v in a {
                add_value(v, &mut sources)?;
            }
        }
        Some(v) => add_value(v, &mut sources)?,
    }
    let style_arg = opt_str(o, "style")?.unwrap_or("apa").trim().to_owned();
    let (style, csl) = if style_arg.to_ascii_lowercase().ends_with(".csl") {
        let file = path(&style_arg);
        let s = kantor_cite::load_style(&file.to_string_lossy())?;
        (s.style.info.id.clone(), Some(s))
    } else {
        kantor_cite::load_style(&style_arg)?;
        (kantor_cite::styles::canonical_id(&style_arg), None)
    };
    let locale = opt_str(o, "locale")?.map(str::to_owned).unwrap_or_else(|| {
        if lang.to_ascii_lowercase().starts_with("id") {
            "id-ID".to_owned()
        } else {
            lang.to_owned()
        }
    });
    // FNV-1a over the document's JSON.
    let mut seed: u64 = 0xcbf2_9ce4_8422_2325;
    for b in all.to_string().bytes() {
        seed ^= u64::from(b);
        seed = seed.wrapping_mul(0x0100_0000_01b3);
    }
    Ok(CiteSetup {
        style,
        csl,
        locale,
        sources,
        seed,
    })
}

/// Open `bytes` in the docs engine, update every field from Kantor's
/// layout (tables of contents, page references, cross-references,
/// captions), save; returns the file and its page count.
pub fn finish(bytes: Vec<u8>, now: chrono::NaiveDateTime) -> Result<(Vec<u8>, usize)> {
    finish_with(bytes, now, None)
}

/// [`finish`], with citations rendered first (`cite`: the style, written
/// as Zotero's document preference, and the sources, kept as the
/// document's source list).
pub fn finish_with(
    bytes: Vec<u8>,
    now: chrono::NaiveDateTime,
    cite: Option<&CiteSetup>,
) -> Result<(Vec<u8>, usize)> {
    let read = docs_engine::docx::read(bytes).context("reading the document back")?;
    let mut doc = docs_engine::Document::from_read(read, None);
    let mut engine = docs_layout::Engine::new();
    if let Some(c) = cite {
        doc.set_citation_prefs_seeded(&c.style, Some(&c.locale), Some(c.seed));
        if !c.sources.is_empty() {
            doc.add_sources(c.sources.clone());
        }
        doc.refresh_citations_in(
            docs_engine::Selection::default(),
            Some(&c.locale),
            c.csl.as_ref(),
        )
        .context("formatting the citations")?;
    }
    update_all_fields(&mut doc, &mut engine, now);
    let out = doc.to_docx_bytes()?;
    let pages = docs_pdf::lay_out(&doc, &mut engine).paged.pages.len();
    Ok((Pkg::read(&out)?.to_bytes()?, pages))
}

/// `{pages}` as `PAGEREF _KantorEnd` in headers and footers: the last
/// page's number from Kantor's layout as the field's result (Word works it
/// out again when it lays the pages out).
fn fill_last_page(bytes: Vec<u8>) -> Result<Vec<u8>> {
    let read = docs_engine::docx::read(bytes.clone()).context("reading the document back")?;
    let doc = docs_engine::Document::from_read(read, None);
    let mut engine = docs_layout::Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let last = laid.paged.pages.last().map_or(1, |p| p.number);
    let mut pkg = Pkg::read(&bytes)?;
    let names: Vec<String> = pkg
        .parts
        .keys()
        .filter(|n| n.starts_with("word/header") || n.starts_with("word/footer"))
        .cloned()
        .collect();
    for n in names {
        if let Some(x) = pkg.text(&n).filter(|x| x.contains(PAGES_MARK)) {
            pkg.set_text(&n, x.replace(PAGES_MARK, &last.to_string()));
        }
    }
    pkg.to_bytes()
}

/// Update every field in `doc` (twice: page numbers settle after the
/// tables of contents grow).
pub fn update_all_fields(
    doc: &mut docs_engine::Document,
    engine: &mut docs_layout::Engine,
    now: chrono::NaiveDateTime,
) {
    let cx = docs_engine::FieldContext {
        now,
        offset: chrono::Offset::fix(&chrono::Utc),
    };
    for _ in 0..2 {
        let all = docs_engine::Selection {
            anchor: docs_engine::Position::new(0, 0),
            head: doc.end(),
        };
        doc.update_fields(all, &cx, &mut |d| docs_pdf::field_pages(d, engine));
    }
}

/// The styles a look defines that `styles_xml` (an existing document's)
/// lacks, among `wanted` and what they're based on.
pub fn missing_styles(look: &Look, styles_xml: &str, wanted: &BTreeSet<String>) -> String {
    let has = |id: &str| styles_xml.contains(&format!("w:styleId=\"{id}\""));
    let mut need: Vec<String> = Vec::new();
    let mut todo: Vec<String> = wanted.iter().cloned().collect();
    while let Some(id) = todo.pop() {
        if has(&id) || need.contains(&id) {
            continue;
        }
        if id == "DocumentTable" {
            need.push(id);
            todo.push("TableNormal".into());
            continue;
        }
        if let Some(s) = look.style(&id) {
            need.push(id.clone());
            if let Some(b) = &s.based_on {
                todo.push(b.clone());
            }
        }
    }
    if need.is_empty() {
        return String::new();
    }
    // The look's own styles.xml, cut into styles.
    let all = super::docs_look::styles_xml(look);
    let mut out = String::new();
    for id in need {
        let pat = format!("w:styleId=\"{id}\"");
        if let Some(i) = all.find(&pat) {
            let start = all[..i].rfind("<w:style ").unwrap_or(i);
            if let Some(e) = all[i..].find("</w:style>") {
                let mut el = all[start..i + e + "</w:style>".len()].to_owned();
                // A default in the look isn't one here.
                el = el.replacen(" w:default=\"1\"", "", 1);
                out.push_str(&el);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

    fn part(bytes: &[u8], name: &str) -> String {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut s = String::new();
        z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
        s
    }

    fn png(dir: &Path) -> PathBuf {
        // A 2 × 1 PNG.
        let img = crate::cli::render_png::Image {
            width: 2,
            height: 1,
            rgb: vec![200, 30, 30, 200, 30, 30],
        };
        let p = dir.join("dot.png");
        std::fs::write(&p, img.png()).unwrap();
        p
    }

    #[std::prelude::v1::test]
    fn builds_every_block_and_is_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        png(dir.path());
        let doc = json!({
            "preset": "report",
            "properties": {"title": "Plan", "company": "Example Co", "date": "2026-11-25"},
            "cover": {"label": "Report", "subtitle": "A test", "meta": [["Prepared for", "Someone"]]},
            "toc": {"levels": 2},
            "blocks": [
                {"h1": "Introduction", "id": "intro"},
                "Plain text with **bold**, *italic*, a [link](https://example.com) and a note[^Source: test.].",
                {"bullets": ["One", {"text": "Two", "items": ["Two a"]}]},
                {"numbers": ["First", "Second"]},
                {"h2": "Data", "id": "data"},
                {"type": "table", "id": "t1", "caption": "Costs", "columns": [3, 1],
                 "header": ["Item", "Cost"], "rows": [["A", "1,200"], [{"text": "Total", "col_span": 2}, null]], "total": true},
                {"type": "picture", "path": "dot.png", "width": "4cm", "alt": "A dot", "caption": "A dot", "id": "f1"},
                {"type": "chart", "chart": "column", "categories": ["Q1", "Q2"],
                 "series": [{"name": "Sales", "values": [1, 2], "color": "accent"}], "caption": "Sales", "id": "f2", "height": "5cm"},
                "See {ref:intro}, {ref:data:text}, {ref:t1}, {ref:f2} on page {ref:f2:page}.",
                {"type": "callout", "title": "Note", "text": ["One", {"bullets": ["x"]}]},
                {"type": "quote", "text": "Short.", "source": "Someone"},
                {"type": "page_break"},
                {"type": "section", "orientation": "landscape"},
                {"h1": "Appendix", "numbered": false},
                {"type": "list_of_figures"}
            ]
        });
        let a = build(&doc, Some(dir.path())).unwrap();
        let b = build(&doc, Some(dir.path())).unwrap();
        assert_eq!(a.bytes, b.bytes, "the same JSON gives the same bytes");
        assert!(a.pages >= 4, "{} pages", a.pages);
        let d = part(&a.bytes, "word/document.xml");
        for want in [
            "w:val=\"Heading1\"",
            "SEQ Figure",
            "REF _Ref",
            "PAGEREF _Ref",
            "TOC \\o",
            "w:orient=\"landscape\"",
            "c:chart",
            "a:blip",
            "w:footnoteReference",
            "<w:tblHeader/>",
            "w:gridSpan",
        ] {
            assert!(d.contains(want), "{want} missing");
        }
        // The table of contents was filled in with page numbers.
        let doc_e = docs_engine::Document::from_read(
            docs_engine::docx::read(a.bytes.clone()).unwrap(),
            None,
        );
        let toc_lines: Vec<String> = doc_e
            .paragraphs()
            .iter()
            .filter(|p| p.props.style.as_deref() == Some("TOC1"))
            .map(|p| p.text.replace('\u{FFFC}', ""))
            .collect();
        assert!(
            toc_lines.iter().any(|t| t.contains("Introduction")),
            "{toc_lines:?}"
        );
        assert!(part(&a.bytes, "word/footnotes.xml").contains("Source: test."));
        assert!(part(&a.bytes, "word/styles.xml").contains("heading 1"));
        assert!(part(&a.bytes, "[Content_Types].xml").contains("drawingml.chart"));
    }

    #[std::prelude::v1::test]
    fn bad_json_says_what_is_wrong() {
        let err = |v: Value| build(&v, None).unwrap_err().to_string();
        let e = |v: Value| format!("{:#}", build(&v, None).unwrap_err());
        assert!(err(json!({"blocks": [], "colour": 1})).contains("unknown key"));
        assert!(
            e(json!({"blocks": [{"type": "table", "rows": [["a", "b"], ["c"]]}]}))
                .contains("row 2 has 1")
        );
        assert!(e(json!({"blocks": ["See {ref:nope}"]})).contains("nope"));
        assert!(
            e(json!({"blocks": [{"type": "heading", "text": "x", "level": 7}]})).contains("1 to 4")
        );
        assert!(e(json!({"blocks": [{"type": "picture", "path": "x.png"}]})).contains("absolute"));
        assert!(e(json!({"blocks": [{"wat": 1}]})).contains("shorthand"));
        assert!(
            e(json!({"blocks": [{"h1": "a", "id": "x"}, {"h2": "b", "id": "x"}]}))
                .contains("two blocks")
        );
    }

    #[std::prelude::v1::test]
    fn numbers_look_numeric() {
        for s in ["1,200", "45%", "$3.2M", "-4.5", "Rp 12.000", "(12)"] {
            assert!(looks_numeric(&json!(s)), "{s}");
        }
        for s in ["Q1", "n/a", "", "Total"] {
            assert!(!looks_numeric(&json!(s)), "{s}");
        }
        assert!(looks_numeric(&json!(3)));
    }

    /// Per page of `bytes` as Kantor lays it out: the footer's "a / b"
    /// numbers, if it shows any.
    fn footer_numbers(bytes: &[u8]) -> Vec<Option<(u32, u32)>> {
        let read = docs_engine::docx::read(bytes.to_vec()).unwrap();
        let doc = docs_engine::Document::from_read(read, None);
        let laid = docs_pdf::lay_out(&doc, &mut docs_layout::Engine::new());
        laid.furniture
            .iter()
            .map(|placed| {
                placed
                    .iter()
                    .filter(|p| p.kind == docs_engine::HfKind::Footer)
                    .find_map(|p| {
                        let t = p.layout.shown(0);
                        let (a, b) = t.trim().split_once(" / ")?;
                        Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
                    })
            })
            .collect()
    }

    #[std::prelude::v1::test]
    fn pages_counts_the_body_after_the_front_matter() {
        for sections in [false, true] {
            let mut blocks = Vec::new();
            for k in 0..5 {
                if sections && k == 2 {
                    blocks.push(json!({"type": "section", "orientation": "landscape"}));
                }
                if sections && k == 4 {
                    blocks.push(json!({"type": "section", "orientation": "portrait"}));
                }
                blocks.push(json!({"h1": format!("Part {k}")}));
                blocks.push(json!("Some text."));
            }
            let b = build(
                &json!({
                    "preset": "proposal",
                    "cover": {"title": "Proposal"},
                    "toc": true,
                    "footer": "{page} / {pages}",
                    "blocks": blocks
                }),
                None,
            )
            .unwrap();
            let pkg = Pkg::read(&b.bytes).unwrap();
            let footers: String = pkg
                .parts
                .keys()
                .filter(|n| n.starts_with("word/footer"))
                .filter_map(|n| pkg.text(n))
                .collect();
            if sections {
                assert!(footers.contains("PAGEREF _KantorEnd"), "{footers}");
                assert!(!footers.contains(PAGES_MARK));
                let d = part(&b.bytes, "word/document.xml");
                assert!(d.contains("w:name=\"_KantorEnd\""));
            } else {
                assert!(footers.contains("SECTIONPAGES"), "{footers}");
            }
            assert!(!footers.contains("NUMPAGES"));
            // Kantor's pages: the cover and contents show no footer, the
            // body's pages count 1…n of n.
            let numbers = footer_numbers(&b.bytes);
            let body: Vec<(u32, u32)> = numbers.iter().flatten().copied().collect();
            let n = body.len() as u32;
            assert!(n >= 5, "{numbers:?}");
            assert!(numbers.len() as u32 > n, "front matter: {numbers:?}");
            for (k, (a, b)) in body.iter().enumerate() {
                assert_eq!((*a, *b), (k as u32 + 1, n), "{numbers:?}");
            }
        }
    }

    #[std::prelude::v1::test]
    fn code_runs_take_the_code_style() {
        let b = build(
            &json!({
                "styles": {"Code": {"size": 9.5}},
                "blocks": [
                    "Run `cargo fmt` now.",
                    {"text": [{"text": "Set "}, {"text": "x = 1", "code": true}]},
                    {"type": "table", "header": ["Key"], "rows": [["`[update]`"]]}
                ]
            }),
            None,
        )
        .unwrap();
        let read = docs_engine::docx::read(b.bytes).unwrap();
        let doc = docs_engine::Document::from_read(read, None);
        let mut seen = BTreeSet::new();
        for i in 0..doc.len() {
            let p = doc.paragraph(i);
            for (range, run) in p.run_ranges() {
                let t = &p.text[range];
                if ["cargo fmt", "x = 1", "[update]"].contains(&t) {
                    seen.insert(t.to_owned());
                    assert_eq!(run.props.style.as_deref(), Some("Code"), "{t}");
                    assert!(
                        run.props.fonts.is_empty() && run.props.size.is_none(),
                        "{t}: no direct formatting"
                    );
                    let r = doc.resolve_run(p, &run.props);
                    assert_eq!(r.size_pt, 9.5, "{t}");
                    assert_eq!(r.font, "Courier New", "{t}");
                }
            }
        }
        assert_eq!(seen.len(), 3, "{seen:?}");
        // The default mono font has a metric-compatible stand-in.
        assert_eq!(
            kantor_core::fonts::metric_substitute("Courier New"),
            Some("Liberation Mono")
        );
    }

    #[std::prelude::v1::test]
    fn citations_from_a_bib_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("pustaka.bib"),
            r"@article{nugroho2021,
  author = {Nugroho, Adi},
  title = {Pengaruh media sosial terhadap prestasi belajar},
  journal = {Jurnal Pendidikan Indonesia},
  year = {2021}, volume = {10}, pages = {115--130}
}
@book{santoso2019,
  author = {Santoso, Budi and Wijaya, Citra},
  title = {Metodologi penelitian kuantitatif},
  publisher = {Penerbit Andi}, year = {2019}
}",
        )
        .unwrap();
        let doc = json!({
            "lang": "id-ID",
            "citations": {"sources": "pustaka.bib"},
            "blocks": [
                {"h1": "Pendahuluan"},
                "Media sosial berpengaruh {cite:nugroho2021@118;santoso2019}.",
                ["Menurut Santoso ", {"cite": "santoso2019", "author": false}, ", metode ini umum."],
                {"bibliography": true}
            ]
        });
        let a = build(&doc, Some(dir.path())).unwrap();
        let b = build(&doc, Some(dir.path())).unwrap();
        assert_eq!(a.bytes, b.bytes, "the same JSON gives the same bytes");
        let read = docs_engine::docx::read(a.bytes.clone()).unwrap();
        let d = docs_engine::Document::from_read(read, None);
        let cites = d.citations();
        assert_eq!(cites.len(), 2);
        assert!(
            cites
                .iter()
                .all(|c| c.source == docs_engine::CiteSource::Zotero)
        );
        assert!(
            cites[0].text.contains("Nugroho, 2021") && cites[0].text.contains("118"),
            "{}",
            cites[0].text
        );
        assert!(
            cites[1].text.contains("2019") && !cites[1].text.contains("Santoso"),
            "{}",
            cites[1].text
        );
        assert_eq!(d.bibliographies().len(), 1);
        let prefs = d.citation_prefs(None);
        assert_eq!(prefs.from, docs_engine::PrefsFrom::Zotero);
        assert_eq!(prefs.style, "http://www.zotero.org/styles/apa");
        assert_eq!(prefs.locale.as_deref(), Some("id-ID"));
        assert_eq!(d.sources().len(), 2);
        let text: Vec<String> = (0..d.len())
            .map(|i| d.paragraph(i).text.replace(docs_engine::OBJECT, ""))
            .collect();
        assert!(text.iter().any(|t| t == "Daftar Pustaka"), "{text:?}");
        assert!(
            text.iter().any(|t| t.starts_with("Nugroho, A. (2021).")),
            "{text:?}"
        );

        // An unknown key says which keys there are.
        let bad = json!({
            "citations": {"sources": "pustaka.bib"},
            "blocks": ["{cite:nobody2020}"]
        });
        let e = format!("{:#}", build(&bad, Some(dir.path())).unwrap_err());
        assert!(e.contains("nobody2020") && e.contains("nugroho2021"), "{e}");
    }
}
