//! Citations and bibliographies written by reference managers (see
//! `docs/decisions/0007-citations.md`).
//!
//! Docs keeps them the way it keeps any field or content control (their
//! code XML byte for byte); this module finds them and reads their data:
//!
//! - **Zotero**: fields `ADDIN ZOTERO_ITEM CSL_CITATION {json}` and
//!   `ADDIN ZOTERO_BIBL {json} CSL_BIBLIOGRAPHY`, document preferences in
//!   `docProps/custom.xml` (`ZOTERO_PREF_1…n`);
//! - **Mendeley Desktop**: `ADDIN CSL_CITATION {json}`,
//!   `ADDIN Mendeley Bibliography CSL_BIBLIOGRAPHY`, `Mendeley Citation
//!   Style_1` in custom.xml;
//! - **Mendeley Cite**: inline content controls tagged
//!   `MENDELEY_CITATION_v3_<base64 json>`, a block control tagged
//!   `MENDELEY_BIBLIOGRAPHY` (unwrapped when read: its paragraphs are
//!   ordinary ones between two `Level::Wrap` markers);
//! - **Word**: `w:citation` content controls around `CITATION Tag` fields,
//!   a `BIBLIOGRAPHY` field, sources as `b:Sources` in a custom XML part;
//! - **EndNote**: `ADDIN EN.CITE <EndNote>…</EndNote>`, `ADDIN EN.REFLIST`.
//!
//! The CSL ones (Zotero, Mendeley) are what [`Document::refresh_citations`]
//! re-renders; Word's and EndNote's are listed, not re-rendered.

use base64::Engine as _;
use serde_json::{Map, Value, json};

use crate::document::Document;
use crate::fields::{FieldSpan, spans};
use crate::model::{OBJECT, Object, Paragraph, Position};
use crate::object::{Level, Opaque};
use crate::xml;

/// The tag prefix of a Mendeley Cite citation content control.
pub(crate) const MENDELEY_CITE_TAG: &str = "MENDELEY_CITATION_v3_";
/// The tag of a Mendeley Cite bibliography content control.
pub(crate) const MENDELEY_BIB_TAG: &str = "MENDELEY_BIBLIOGRAPHY";
/// The CSL citation schema Zotero and Mendeley name in their JSON.
pub const CSL_CITATION_SCHEMA: &str =
    "https://github.com/citation-style-language/schema/raw/master/csl-citation.json";

/// Which tool wrote a citation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CiteSource {
    Zotero,
    /// Mendeley Desktop's Word plug-in (fields).
    Mendeley,
    /// Mendeley Cite, the Office add-in (content controls).
    MendeleyCite,
    Word,
    EndNote,
}

impl CiteSource {
    pub fn name(self) -> &'static str {
        match self {
            Self::Zotero => "Zotero",
            Self::Mendeley => "Mendeley Desktop",
            Self::MendeleyCite => "Mendeley Cite",
            Self::Word => "Word",
            Self::EndNote => "EndNote",
        }
    }

    /// Whether Kantor re-renders it (its data is CSL-JSON).
    pub fn is_csl(self) -> bool {
        matches!(self, Self::Zotero | Self::Mendeley | Self::MendeleyCite)
    }
}

/// Where a citation or bibliography is in the body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiteAt {
    /// A field: its start and end marks ([`FieldSpan::mark_id`] tells it
    /// apart across edits).
    Field {
        begin: Position,
        end: Position,
        mark_id: usize,
    },
    /// An inline content control: one object.
    Control { at: Position },
    /// An unwrapped block content control: its paragraphs.
    Paragraphs { first: usize, last: usize },
}

impl CiteAt {
    /// Where it starts.
    pub fn start(&self) -> Position {
        match self {
            Self::Field { begin, .. } => *begin,
            Self::Control { at } => *at,
            Self::Paragraphs { first, .. } => Position::new(*first, 0),
        }
    }
}

/// One cited item.
#[derive(Clone, Debug, PartialEq)]
pub struct CiteItem {
    /// The item's id in the citation (Zotero's item id, Mendeley's
    /// `ITEM-1`, Word's source tag, EndNote's record number).
    pub id: String,
    /// The item's data as CSL-JSON (`itemData`; converted for Word and
    /// EndNote).
    pub data: Value,
    pub locator: Option<String>,
    /// The locator's kind (`page`, `chapter`…).
    pub label: Option<String>,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
    pub suppress_author: bool,
    pub author_only: bool,
    /// Links into the manager's library.
    pub uris: Vec<String>,
}

impl CiteItem {
    /// An item cited plainly.
    pub fn new(data: Value) -> Self {
        Self {
            id: data
                .get("id")
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    v => v.to_string(),
                })
                .unwrap_or_default(),
            data,
            locator: None,
            label: None,
            prefix: None,
            suffix: None,
            suppress_author: false,
            author_only: false,
            uris: Vec::new(),
        }
    }

    /// "Nugroho 2021: Pengaruh media sosial…", for lists.
    pub fn summary(&self) -> String {
        summary(&self.data)
    }
}

/// A short description of a CSL-JSON item: first author, year, title.
pub fn summary(data: &Value) -> String {
    let author = data
        .get("author")
        .or_else(|| data.get("editor"))
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .map(|n| {
            n.get("family")
                .or_else(|| n.get("literal"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        })
        .unwrap_or_default();
    let year = year_of(data).map(|y| y.to_string()).unwrap_or_default();
    let title = data
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let head = [author, year]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    match (head.is_empty(), title.is_empty()) {
        (false, false) => format!("{head}: {title}"),
        (false, true) => head,
        _ => title.to_owned(),
    }
}

/// The year an item was issued.
pub fn year_of(data: &Value) -> Option<i64> {
    let issued = data.get("issued")?;
    let parts = issued
        .get("date-parts")
        .and_then(|d| d.get(0))
        .and_then(|d| d.get(0))
        .and_then(|first| match first {
            Value::Number(n) => n.as_i64(),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        });
    parts.or_else(|| {
        let raw = issued
            .get("raw")
            .or_else(|| issued.get("literal"))?
            .as_str()?;
        raw.split(|c: char| !c.is_ascii_digit())
            .find(|p| p.len() == 4)?
            .parse()
            .ok()
    })
}

/// A citation in the body.
#[derive(Clone, Debug)]
pub struct Citation {
    pub source: CiteSource,
    pub at: CiteAt,
    /// The text it shows (its last result).
    pub text: String,
    pub items: Vec<CiteItem>,
    /// The CSL citation JSON (Zotero and Mendeley), as read.
    pub json: Option<Value>,
    /// Edited by hand in the manager (Zotero's `dontUpdate`, Mendeley
    /// Cite's manual override): Refresh keeps its text.
    pub edited: bool,
}

/// A bibliography in the body.
#[derive(Clone, Debug)]
pub struct Bibliography {
    pub source: CiteSource,
    pub at: CiteAt,
    /// Zotero's bibliography JSON (uncited, omitted, custom entries).
    pub json: Option<Value>,
}

/// What a field instruction is, citation-wise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FieldKind {
    Cite(CiteSource),
    Bib(CiteSource),
}

fn field_kind(instr: &str) -> Option<FieldKind> {
    let words: Vec<String> = instr
        .split_whitespace()
        .take(3)
        .map(str::to_ascii_uppercase)
        .collect();
    let w = |i: usize| words.get(i).map(String::as_str).unwrap_or_default();
    use CiteSource::*;
    use FieldKind::*;
    Some(match (w(0), w(1)) {
        ("ADDIN", "ZOTERO_ITEM") => Cite(Zotero),
        ("ADDIN", "ZOTERO_BIBL") => Bib(Zotero),
        ("ADDIN", "CSL_CITATION") => Cite(Mendeley),
        ("ADDIN", "MENDELEY") if w(2) == "BIBLIOGRAPHY" => Bib(Mendeley),
        ("ADDIN", "EN.CITE") => Cite(EndNote),
        ("ADDIN", "EN.REFLIST") => Bib(EndNote),
        ("CITATION", _) => Cite(Word),
        ("BIBLIOGRAPHY", _) => Bib(Word),
        _ => return None,
    })
}

/// The first JSON object in `s` (after the field's keywords).
pub(crate) fn json_in(s: &str) -> Option<Value> {
    let at = s.find('{')?;
    serde_json::Deserializer::from_str(&s[at..])
        .into_iter::<Value>()
        .next()?
        .ok()
}

/// A content control's `w:tag` value, from its XML.
fn sdt_tag(xml: &str) -> Option<String> {
    let pr = &xml[..xml.find("</w:sdtPr>")?];
    let rest = &pr[pr.find("<w:tag ")?..];
    let el = xml::parse(&rest[..=rest.find('>')?])?;
    el.attr("val").map(str::to_owned)
}

/// Whether a content control's properties hold `w:citation` (Word).
fn sdt_is_word_citation(xml: &str) -> bool {
    let end = xml.find("</w:sdtPr>").unwrap_or(0);
    let pr = &xml[..end];
    pr.contains("<w:citation/>") || pr.contains("<w:citation ")
}

/// The text an inline citation content control shows (Mendeley Cite,
/// Word), if `o` is one: laid out as text in place of a placeholder.
pub fn control_text(o: &Opaque) -> Option<String> {
    if o.level != Level::Para || crate::model::element_name(&o.xml) != "sdt" {
        return None;
    }
    let cite = sdt_is_word_citation(&o.xml)
        || sdt_tag(&o.xml).is_some_and(|t| t.starts_with(MENDELEY_CITE_TAG));
    cite.then(|| {
        let content = o.xml.find("<w:sdtContent").unwrap_or(0);
        xml::inner_text(&o.xml[content..], &["t"])
    })
}

/// A Mendeley Cite control's citation JSON.
fn mendeley_cite_json(tag: &str) -> Option<Value> {
    let b64 = tag.strip_prefix(MENDELEY_CITE_TAG)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(b64.trim()))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn str_of(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The items of a CSL citation JSON.
fn csl_items(json: &Value) -> Vec<CiteItem> {
    let Some(items) = json.get("citationItems").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .map(|it| {
            let mut data = it.get("itemData").cloned().unwrap_or(Value::Null);
            let id = str_of(it.get("id"))
                .or_else(|| str_of(data.get("id")))
                .unwrap_or_default();
            if let Some(o) = data.as_object_mut()
                && !o.contains_key("id")
            {
                o.insert("id".into(), Value::String(id.clone()));
            }
            CiteItem {
                id,
                data,
                locator: str_of(it.get("locator")),
                label: str_of(it.get("label")),
                prefix: str_of(it.get("prefix")),
                suffix: str_of(it.get("suffix")),
                suppress_author: it
                    .get("suppress-author")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                author_only: it
                    .get("author-only")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                uris: it
                    .get("uris")
                    .or_else(|| it.get("uri"))
                    .map(|u| match u {
                        Value::Array(a) => a
                            .iter()
                            .filter_map(|x| x.as_str().map(str::to_owned))
                            .collect(),
                        Value::String(s) => vec![s.clone()],
                        _ => Vec::new(),
                    })
                    .unwrap_or_default(),
            }
        })
        .collect()
}

/// A CSL citation JSON's items as citation items again (`itemData`
/// included): what Kantor writes into a citation it makes.
pub fn csl_citation_items(items: &[CiteItem]) -> Vec<Value> {
    items
        .iter()
        .map(|it| {
            let mut o = Map::new();
            let id = it
                .id
                .parse::<i64>()
                .map(Value::from)
                .unwrap_or_else(|_| Value::String(it.id.clone()));
            o.insert("id".into(), id);
            if !it.uris.is_empty() {
                o.insert("uris".into(), json!(it.uris));
            }
            o.insert("itemData".into(), it.data.clone());
            for (k, v) in [
                ("locator", &it.locator),
                ("label", &it.label),
                ("prefix", &it.prefix),
                ("suffix", &it.suffix),
            ] {
                if let Some(v) = v {
                    o.insert(k.into(), Value::String(v.clone()));
                }
            }
            if it.suppress_author {
                o.insert("suppress-author".into(), Value::Bool(true));
            }
            if it.author_only {
                o.insert("author-only".into(), Value::Bool(true));
            }
            Value::Object(o)
        })
        .collect()
}

/// The text between two positions (objects left out, paragraphs joined
/// by a space).
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

fn field_at(s: &FieldSpan) -> CiteAt {
    CiteAt::Field {
        begin: s.begin,
        end: s.end,
        mark_id: s.mark_id(),
    }
}

/// A Word `CITATION` instruction's sources: `CITATION Tag \p 23 \l 1057
/// \m Tag2 \p 4` (`\m` adds a source, `\p` gives the last one pages, `\n`
/// leaves out its author).
fn word_cite_items(instr: &str, sources: &[WordSource]) -> Vec<CiteItem> {
    let item = |tag: &str| {
        let data = sources
            .iter()
            .find(|s| s.tag.eq_ignore_ascii_case(tag))
            .map(|s| s.data.clone())
            .unwrap_or_else(|| json!({ "id": tag, "title": tag }));
        let mut it = CiteItem::new(data);
        it.id = tag.to_owned();
        it
    };
    let words: Vec<&str> = instr.split_whitespace().collect();
    let mut out: Vec<CiteItem> = Vec::new();
    let mut i = 1;
    while i < words.len() {
        let w = words[i];
        let arg = words.get(i + 1).copied();
        match w.to_ascii_lowercase().as_str() {
            "\\m" => {
                if let Some(t) = arg {
                    out.push(item(t));
                }
                i += 2;
            }
            "\\p" => {
                if let (Some(last), Some(p)) = (out.last_mut(), arg) {
                    last.locator = Some(p.trim_matches('"').to_owned());
                    last.label = Some("page".into());
                }
                i += 2;
            }
            "\\f" | "\\s" | "\\v" | "\\l" => i += 2,
            "\\n" => {
                if let Some(last) = out.last_mut() {
                    last.suppress_author = true;
                }
                i += 1;
            }
            s if s.starts_with('\\') => i += 1,
            _ => {
                if out.is_empty() {
                    out.push(item(w));
                }
                i += 1;
            }
        }
    }
    out
}

/// The `w:instrText` of the field in a Word citation control.
fn control_instr(xml: &str) -> String {
    xml::inner_text(xml, &["instrText"])
}

/// A source from Word's `b:Sources`.
#[derive(Clone, Debug)]
pub struct WordSource {
    pub tag: String,
    /// Converted to CSL-JSON.
    pub data: Value,
}

/// Word's sources (`b:Sources` in a custom XML part), as CSL-JSON.
pub fn word_sources(xml: &str) -> Vec<WordSource> {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    let child_text = |n: roxmltree::Node, name: &str| -> Option<String> {
        n.children()
            .find(|c| c.tag_name().name() == name)
            .and_then(|c| c.text())
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
    };
    let people = |src: roxmltree::Node, role: &str| -> Vec<Value> {
        let Some(author) = src.children().find(|c| c.tag_name().name() == "Author") else {
            return Vec::new();
        };
        let Some(r) = author.children().find(|c| c.tag_name().name() == role) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for n in r.descendants() {
            match n.tag_name().name() {
                "Person" => {
                    let last = child_text(n, "Last").unwrap_or_default();
                    let first = [child_text(n, "First"), child_text(n, "Middle")]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" ");
                    out.push(json!({ "family": last, "given": first }));
                }
                "Corporate" => {
                    if let Some(t) = n.text() {
                        out.push(json!({ "literal": t.trim() }));
                    }
                }
                _ => {}
            }
        }
        out
    };
    let mut out = Vec::new();
    for src in doc
        .descendants()
        .filter(|n| n.tag_name().name() == "Source")
    {
        let Some(tag) = child_text(src, "Tag") else {
            continue;
        };
        let kind = child_text(src, "SourceType").unwrap_or_default();
        let ty = match kind.as_str() {
            "JournalArticle" => "article-journal",
            "ArticleInAPeriodical" => "article-magazine",
            "Book" => "book",
            "BookSection" => "chapter",
            "ConferenceProceedings" => "paper-conference",
            "Report" => "report",
            "InternetSite" | "DocumentFromInternetSite" => "webpage",
            "Patent" => "patent",
            "Case" => "legal_case",
            "Film" => "motion_picture",
            "Interview" => "interview",
            "SoundRecording" => "song",
            "Performance" => "performance",
            "Art" => "graphic",
            "ElectronicSource" => "dataset",
            _ => "document",
        };
        let mut o = Map::new();
        o.insert("id".into(), Value::String(tag.clone()));
        o.insert("type".into(), Value::String(ty.into()));
        for (role, key) in [
            ("Author", "author"),
            ("Editor", "editor"),
            ("Translator", "translator"),
        ] {
            let p = people(src, role);
            if !p.is_empty() {
                o.insert(key.into(), Value::Array(p));
            }
        }
        for (el, key) in [
            ("Title", "title"),
            ("JournalName", "container-title"),
            ("BookTitle", "container-title"),
            ("PeriodicalTitle", "container-title"),
            ("ConferenceName", "container-title"),
            ("InternetSiteTitle", "container-title"),
            ("Publisher", "publisher"),
            ("City", "publisher-place"),
            ("Pages", "page"),
            ("Volume", "volume"),
            ("Issue", "issue"),
            ("Edition", "edition"),
            ("URL", "URL"),
            ("DOI", "DOI"),
            ("StandardNumber", "ISBN"),
            ("Institution", "publisher"),
        ] {
            if !o.contains_key(key)
                && let Some(v) = child_text(src, el)
            {
                o.insert(key.into(), Value::String(v));
            }
        }
        if let Some(y) = child_text(src, "Year") {
            let mut parts = vec![json!(
                y.parse::<i64>()
                    .map(Value::from)
                    .unwrap_or(Value::String(y))
            )];
            if let Some(m) = child_text(src, "Month").and_then(|m| month_number(&m)) {
                parts.push(json!(m));
                if let Some(d) = child_text(src, "Day").and_then(|d| d.parse::<i64>().ok()) {
                    parts.push(json!(d));
                }
            }
            o.insert("issued".into(), json!({ "date-parts": [parts] }));
        }
        out.push(WordSource {
            tag,
            data: Value::Object(o),
        });
    }
    out
}

fn month_number(m: &str) -> Option<i64> {
    if let Ok(n) = m.trim().parse::<i64>() {
        return (1..=12).contains(&n).then_some(n);
    }
    let m = m.trim().to_ascii_lowercase();
    const NAMES: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    NAMES
        .iter()
        .position(|n| m.starts_with(n))
        .map(|i| i as i64 + 1)
}

/// An EndNote `<Cite>` record as CSL-JSON (authors, title, journal,
/// year, pages, volume, number).
fn endnote_items(instr: &str) -> Vec<CiteItem> {
    let Some(start) = instr.find("<EndNote>") else {
        return Vec::new();
    };
    let end = instr
        .rfind("</EndNote>")
        .map_or(instr.len(), |e| e + "</EndNote>".len());
    let Ok(doc) = roxmltree::Document::parse(&instr[start..end]) else {
        return Vec::new();
    };
    let text = |n: roxmltree::Node, name: &str| -> Option<String> {
        n.descendants()
            .find(|c| c.tag_name().name() == name)
            .map(|c| {
                c.descendants()
                    .filter(|t| t.is_text())
                    .map(|t| t.text().unwrap_or_default())
                    .collect::<String>()
            })
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
    };
    doc.root_element()
        .children()
        .filter(|c| c.tag_name().name() == "Cite")
        .map(|cite| {
            let rec = text(cite, "RecNum").unwrap_or_default();
            let mut o = Map::new();
            o.insert("id".into(), Value::String(rec.clone()));
            let ty = cite
                .descendants()
                .find(|c| c.tag_name().name() == "ref-type")
                .and_then(|c| c.attribute("name"))
                .unwrap_or_default();
            let ty = match ty {
                "Journal Article" => "article-journal",
                "Book" => "book",
                "Book Section" => "chapter",
                "Conference Proceedings" | "Conference Paper" => "paper-conference",
                "Thesis" => "thesis",
                "Report" => "report",
                "Web Page" => "webpage",
                _ => "document",
            };
            o.insert("type".into(), Value::String(ty.into()));
            let authors: Vec<Value> = cite
                .descendants()
                .filter(|c| c.tag_name().name() == "author")
                .filter_map(|a| {
                    let t: String = a
                        .descendants()
                        .filter(|x| x.is_text())
                        .map(|x| x.text().unwrap_or_default())
                        .collect();
                    let t = t.trim();
                    (!t.is_empty()).then(|| match t.split_once(',') {
                        Some((f, g)) => json!({ "family": f.trim(), "given": g.trim() }),
                        None => json!({ "literal": t }),
                    })
                })
                .collect();
            if authors.is_empty() {
                if let Some(a) = text(cite, "Author") {
                    o.insert("author".into(), json!([{ "family": a }]));
                }
            } else {
                o.insert("author".into(), Value::Array(authors));
            }
            for (el, key) in [
                ("title", "title"),
                ("secondary-title", "container-title"),
                ("pages", "page"),
                ("volume", "volume"),
                ("number", "issue"),
                ("publisher", "publisher"),
                ("pub-location", "publisher-place"),
            ] {
                if let Some(v) = text(cite, el) {
                    o.insert(key.into(), Value::String(v));
                }
            }
            if let Some(y) = text(cite, "Year").or_else(|| text(cite, "year")) {
                let v = y
                    .parse::<i64>()
                    .map(Value::from)
                    .unwrap_or(Value::String(y));
                o.insert("issued".into(), json!({ "date-parts": [[v]] }));
            }
            let mut it = CiteItem::new(Value::Object(o));
            it.id = rec;
            it
        })
        .collect()
}

impl Document {
    /// Word's sources (`b:Sources` in a custom XML part of the file).
    pub fn word_sources(&self) -> Vec<WordSource> {
        let Some(pkg) = &self.package else {
            return Vec::new();
        };
        let mut names: Vec<&String> = pkg
            .part_names()
            .filter(|n| {
                n.starts_with("customXml/")
                    && n.ends_with(".xml")
                    && !n.contains("itemProps")
                    && !n.contains("_rels")
            })
            .collect();
        names.sort();
        for n in names {
            if let Some(x) = pkg.part_text(n)
                && x.contains("officeDocument/2006/bibliography")
                && x.contains("Sources")
            {
                return word_sources(&x);
            }
        }
        Vec::new()
    }

    /// The citations in the body, in document order.
    pub fn citations(&self) -> Vec<Citation> {
        citations_in(&self.paragraphs, &self.word_sources())
    }

    /// The bibliographies in the body, in document order.
    pub fn bibliographies(&self) -> Vec<Bibliography> {
        bibliographies_in(&self.paragraphs)
    }

    /// Whether the body has any citation or bibliography.
    pub fn has_citations(&self) -> bool {
        !self.citations().is_empty() || !self.bibliographies().is_empty()
    }
}

pub(crate) fn citations_in(paras: &[Paragraph], sources: &[WordSource]) -> Vec<Citation> {
    let mut out = Vec::new();
    for s in spans(paras) {
        let Some(FieldKind::Cite(source)) = field_kind(&s.instr) else {
            continue;
        };
        let (a, b) = s.result();
        let text = text_between(paras, a, b);
        let (items, json, edited) = match source {
            CiteSource::Zotero | CiteSource::Mendeley => {
                let json = json_in(&s.instr);
                let items = json.as_ref().map(csl_items).unwrap_or_default();
                let edited = json
                    .as_ref()
                    .and_then(|j| j.pointer("/properties/dontUpdate"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                (items, json, edited)
            }
            CiteSource::Word => (word_cite_items(&s.instr, sources), None, false),
            CiteSource::EndNote => (endnote_items(&s.instr), None, false),
            CiteSource::MendeleyCite => (Vec::new(), None, false),
        };
        out.push(Citation {
            source,
            at: field_at(&s),
            text,
            items,
            json,
            edited,
        });
    }
    // Content controls: Mendeley Cite, Word.
    for (i, p) in paras.iter().enumerate() {
        for (offset, o) in p.objects() {
            let Object::Opaque(op) = o else { continue };
            let Some(text) = control_text(op) else {
                continue;
            };
            let at = CiteAt::Control {
                at: Position::new(i, offset),
            };
            if sdt_is_word_citation(&op.xml) {
                let instr = control_instr(&op.xml);
                out.push(Citation {
                    source: CiteSource::Word,
                    at,
                    text,
                    items: word_cite_items(&instr, sources),
                    json: None,
                    edited: false,
                });
            } else if let Some(json) = sdt_tag(&op.xml).and_then(|t| mendeley_cite_json(&t)) {
                let edited = json
                    .pointer("/manualOverride/isManuallyOverridden")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                out.push(Citation {
                    source: CiteSource::MendeleyCite,
                    at,
                    text,
                    items: csl_items(&json),
                    json: Some(json),
                    edited,
                });
            }
        }
    }
    out.sort_by_key(|c| c.at.start());
    out
}

pub(crate) fn bibliographies_in(paras: &[Paragraph]) -> Vec<Bibliography> {
    let mut out = Vec::new();
    for s in spans(paras) {
        if let Some(FieldKind::Bib(source)) = field_kind(&s.instr) {
            let json = (source == CiteSource::Zotero)
                .then(|| json_in(&s.instr))
                .flatten();
            out.push(Bibliography {
                source,
                at: field_at(&s),
                json,
            });
        }
    }
    // Mendeley Cite: an unwrapped content control.
    for (i, p) in paras.iter().enumerate() {
        for (offset, o) in p.objects() {
            let Object::Opaque(op) = o else { continue };
            if op.level != Level::Wrap || op.xml.starts_with("</") {
                continue;
            }
            if !sdt_tag(&op.xml).is_some_and(|t| t.starts_with(MENDELEY_BIB_TAG)) {
                continue;
            }
            if let Some(last) = wrap_end(paras, Position::new(i, offset)) {
                out.push(Bibliography {
                    source: CiteSource::MendeleyCite,
                    at: CiteAt::Paragraphs { first: i, last },
                    json: None,
                });
            }
        }
    }
    out.sort_by_key(|b| b.at.start());
    out
}

/// The paragraph holding the end marker of the wrap that starts at
/// `start`.
fn wrap_end(paras: &[Paragraph], start: Position) -> Option<usize> {
    let mut depth = 0usize;
    for (i, p) in paras.iter().enumerate().skip(start.para) {
        for (offset, o) in p.objects() {
            if i == start.para && offset < start.offset {
                continue;
            }
            let Object::Opaque(op) = o else { continue };
            if op.level != Level::Wrap {
                continue;
            }
            if op.xml.starts_with("</") {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i);
                }
            } else {
                depth += 1;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_kinds() {
        use CiteSource::*;
        use FieldKind::*;
        let k = |s: &str| field_kind(s);
        assert_eq!(
            k(" ADDIN ZOTERO_ITEM CSL_CITATION {\"a\":1} "),
            Some(Cite(Zotero))
        );
        assert_eq!(
            k(" ADDIN ZOTERO_BIBL {} CSL_BIBLIOGRAPHY "),
            Some(Bib(Zotero))
        );
        assert_eq!(k("ADDIN CSL_CITATION {}"), Some(Cite(Mendeley)));
        assert_eq!(
            k(" ADDIN Mendeley Bibliography CSL_BIBLIOGRAPHY "),
            Some(Bib(Mendeley))
        );
        assert_eq!(k(" CITATION Nug21 \\l 1057 "), Some(Cite(Word)));
        assert_eq!(k(" BIBLIOGRAPHY "), Some(Bib(Word)));
        assert_eq!(k(" ADDIN EN.CITE <EndNote/>"), Some(Cite(EndNote)));
        assert_eq!(k(" ADDIN EN.REFLIST "), Some(Bib(EndNote)));
        assert_eq!(k(" ADDIN EN.CITE.DATA "), None);
        assert_eq!(k(" PAGE "), None);
    }

    #[test]
    fn json_after_keywords() {
        let v =
            json_in(r#" ADDIN ZOTERO_BIBL {"uncited":[],"custom":[]} CSL_BIBLIOGRAPHY "#).unwrap();
        assert_eq!(v["uncited"], json!([]));
    }

    #[test]
    fn summaries() {
        let v = json!({"title": "T", "author": [{"family": "Nugroho", "given": "Adi"}],
            "issued": {"date-parts": [["2021"]]}});
        assert_eq!(summary(&v), "Nugroho 2021: T");
        assert_eq!(
            year_of(&json!({"issued": {"raw": "June 2019"}})),
            Some(2019)
        );
    }

    #[test]
    fn word_citation_switches() {
        let sources = vec![
            WordSource {
                tag: "Nug21".into(),
                data: json!({"id": "Nug21", "title": "A"}),
            },
            WordSource {
                tag: "San19".into(),
                data: json!({"id": "San19", "title": "B"}),
            },
        ];
        let items = word_cite_items(" CITATION Nug21 \\p 12 \\l 1057 \\m San19 \\p 4", &sources);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "Nug21");
        assert_eq!(items[0].locator.as_deref(), Some("12"));
        assert_eq!(items[1].id, "San19");
        assert_eq!(items[1].locator.as_deref(), Some("4"));
    }
}
