//! Citations and bibliographies as formatted text, through hayagriva.
//!
//! All of a document's citations are rendered together, in document order:
//! numeric styles number items by first citation, author-date styles tell
//! same-author-same-year works apart, and the bibliography lists every
//! cited item once.
//!
//! Items are CSL-JSON as Zotero and Mendeley embed them. The same work is
//! recognised across citations by its library URI, else by its title,
//! year and first author (Mendeley Desktop numbers items per citation:
//! `ITEM-1` in two citations may be two works).
//!
//! What hayagriva doesn't do for a CSL processor, done here: a citation
//! item's prefix and suffix go inside the citation's brackets (first
//! item's prefix, last item's suffix); "suppress author" renders the year
//! in the style's brackets.

use std::collections::HashMap;
use std::str::FromStr as _;
use std::sync::OnceLock;

use hayagriva::citationberg::json as csl_json;
use hayagriva::citationberg::taxonomy::Locator;
use hayagriva::citationberg::{
    FontStyle, FontVariant, FontWeight, Locale, LocaleCode, TextDecoration, VerticalAlign,
};
use hayagriva::{
    BibliographyDriver, BibliographyRequest, CitationItem, CitationRequest, CitePurpose, ElemChild,
    ElemChildren, Formatting, LocatorPayload, SpecificLocator,
};
use serde_json::Value;

use crate::styles::CslStyle;

/// Character formatting of a piece of rendered text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Fmt {
    pub italic: bool,
    pub bold: bool,
    pub small_caps: bool,
    pub underline: bool,
    pub superscript: bool,
    pub subscript: bool,
}

impl Fmt {
    fn of(f: &Formatting) -> Self {
        Self {
            italic: f.font_style == FontStyle::Italic,
            bold: f.font_weight == FontWeight::Bold,
            small_caps: f.font_variant == FontVariant::SmallCaps,
            underline: f.text_decoration == TextDecoration::Underline,
            superscript: f.vertical_align == VerticalAlign::Sup,
            subscript: f.vertical_align == VerticalAlign::Sub,
        }
    }
}

/// A run of rendered text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Piece {
    pub text: String,
    pub fmt: Fmt,
}

/// Pieces as plain text.
pub fn plain(pieces: &[Piece]) -> String {
    pieces.iter().map(|p| p.text.as_str()).collect()
}

/// One item of a citation to render.
#[derive(Clone, Copy, Debug)]
pub struct CiteIn<'a> {
    /// CSL-JSON.
    pub data: &'a Value,
    /// The item's library URI (tells works apart across citations).
    pub uri: Option<&'a str>,
    pub locator: Option<&'a str>,
    /// The locator's kind (`page` when not given).
    pub label: Option<&'a str>,
    pub prefix: Option<&'a str>,
    pub suffix: Option<&'a str>,
    pub suppress_author: bool,
    pub author_only: bool,
}

impl<'a> CiteIn<'a> {
    pub fn new(data: &'a Value) -> Self {
        Self {
            data,
            uri: None,
            locator: None,
            label: None,
            prefix: None,
            suffix: None,
            suppress_author: false,
            author_only: false,
        }
    }
}

/// What [`render`] made.
#[derive(Clone, Debug, Default)]
pub struct Rendered {
    /// One per citation asked for, in order.
    pub citations: Vec<Vec<Piece>>,
    /// The bibliography's entries (empty when the style has none); a
    /// numbered entry's number is followed by a tab.
    pub bibliography: Vec<Vec<Piece>>,
    /// The entries take a hanging indent.
    pub hanging_indent: bool,
    /// The entries start with a number or label set off by a tab
    /// (`second-field-align`).
    pub numbered: bool,
}

/// The CSL locales (parsed once).
fn locales() -> &'static [Locale] {
    static LOCALES: OnceLock<Vec<Locale>> = OnceLock::new();
    LOCALES.get_or_init(hayagriva::archive::locales)
}

/// The key a work is known by across citations.
fn work_key(c: &CiteIn) -> String {
    if let Some(u) = c.uri.filter(|u| !u.is_empty()) {
        return format!("uri:{u}");
    }
    let s = |k: &str| {
        c.data
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_lowercase()
    };
    let author = c
        .data
        .get("author")
        .or_else(|| c.data.get("editor"))
        .and_then(|a| a.get(0))
        .and_then(|n| n.get("family").or_else(|| n.get("literal")))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    let year = c
        .data
        .pointer("/issued/date-parts/0/0")
        .map(|v| v.to_string().trim_matches('"').to_owned())
        .unwrap_or_default();
    let title = s("title");
    if title.is_empty() && author.is_empty() {
        // Nothing to go on: the item's own id.
        return format!(
            "id:{}",
            c.data.get("id").map(|v| v.to_string()).unwrap_or_default()
        );
    }
    format!("w:{author}|{year}|{title}|{}", s("type"))
}

/// A CSL-JSON value as hayagriva's item, field by field: a field it can't
/// read (a boolean, a date only as text) is dropped or simplified instead
/// of failing the item.
pub fn to_item(data: &Value, id: &str) -> csl_json::Item {
    let mut out = std::collections::BTreeMap::new();
    if let Some(o) = data.as_object() {
        for (k, v) in o {
            if k == "id" {
                continue;
            }
            let v = match v {
                Value::Number(n) if n.as_i64().is_none() => Value::String(n.to_string()),
                Value::Bool(_) | Value::Null => continue,
                v => v.clone(),
            };
            match serde_json::from_value::<csl_json::Value>(v.clone()) {
                Ok(x) => {
                    out.insert(k.clone(), x);
                }
                Err(_) => {
                    // A date given as text: its year.
                    let year = v
                        .get("raw")
                        .or_else(|| v.get("literal"))
                        .and_then(Value::as_str)
                        .and_then(|r| {
                            r.split(|c: char| !c.is_ascii_digit())
                                .find(|p| p.len() == 4)
                                .and_then(|y| y.parse::<i64>().ok())
                        });
                    if let Some(y) = year
                        && let Ok(x) = serde_json::from_value::<csl_json::Value>(
                            serde_json::json!({ "date-parts": [[y]] }),
                        )
                    {
                        out.insert(k.clone(), x);
                    } else {
                        log::debug!("citation item {id}: field {k} dropped");
                    }
                }
            }
        }
    }
    if !out.contains_key("type") {
        out.insert("type".into(), csl_json::Value::String("document".into()));
    }
    out.insert("id".into(), csl_json::Value::String(id.to_owned()));
    csl_json::Item(out)
}

/// Render `citations` (each a list of items, in document order) and the
/// bibliography of everything cited, in `style` and `locale` (`id-ID`,
/// `en-US`; `None`: the style's default).
pub fn render(style: &CslStyle, locale: Option<&str>, citations: &[Vec<CiteIn>]) -> Rendered {
    // One item per work.
    let mut keys: HashMap<String, usize> = HashMap::new();
    let mut items: Vec<csl_json::Item> = Vec::new();
    let mut refs: Vec<Vec<usize>> = Vec::with_capacity(citations.len());
    for cite in citations {
        let mut r = Vec::with_capacity(cite.len());
        for c in cite {
            let key = work_key(c);
            let n = keys.len();
            let k = *keys.entry(key).or_insert(n);
            if k == items.len() {
                items.push(to_item(c.data, &format!("kantor-{k}")));
            }
            r.push(k);
        }
        refs.push(r);
    }

    let locale_code = locale
        .map(str::to_owned)
        .or_else(|| style.default_locale.clone())
        .map(LocaleCode);
    let locale_files = locales();
    let s = &style.style;
    let mut driver: BibliographyDriver<'_, csl_json::Item> = BibliographyDriver::new();
    for (cite, r) in citations.iter().zip(&refs) {
        let request_items: Vec<_> = cite
            .iter()
            .zip(r)
            .map(|(c, &k)| {
                let locator = c.locator.filter(|l| !l.trim().is_empty()).map(|l| {
                    let kind = c
                        .label
                        .and_then(|l| Locator::from_str(l).ok())
                        .unwrap_or(Locator::Page);
                    SpecificLocator(kind, LocatorPayload::Str(l))
                });
                let purpose = if c.author_only {
                    Some(CitePurpose::Author)
                } else if c.suppress_author {
                    Some(CitePurpose::Year)
                } else {
                    None
                };
                CitationItem::new(&items[k], locator, None, false, purpose)
            })
            .collect();
        driver.citation(CitationRequest::new(
            request_items,
            s,
            locale_code.clone(),
            locale_files,
            None,
        ));
    }
    let done = driver.finish(BibliographyRequest::new(
        s,
        locale_code.clone(),
        locale_files,
    ));

    let layout_prefix = s.citation.layout.prefix.clone().unwrap_or_default();
    let layout_suffix = s.citation.layout.suffix.clone().unwrap_or_default();
    let mut out = Rendered::default();
    for (cite, r) in citations.iter().zip(&done.citations) {
        let mut pieces = pieces_of(&r.citation);
        // Suppressed author: the year in the style's brackets.
        let purposed = cite.iter().any(|c| c.suppress_author || c.author_only);
        if purposed && cite.iter().all(|c| c.suppress_author) && !layout_prefix.is_empty() {
            pieces.insert(
                0,
                Piece {
                    text: layout_prefix.clone(),
                    fmt: Fmt::default(),
                },
            );
            pieces.push(Piece {
                text: layout_suffix.clone(),
                fmt: Fmt::default(),
            });
        }
        add_affixes(&mut pieces, cite, &layout_prefix, &layout_suffix);
        out.citations.push(merge(pieces));
    }
    if let Some(bib) = done.bibliography {
        out.hanging_indent = bib.hanging_indent;
        out.numbered = bib.second_field_align.is_some();
        for item in &bib.items {
            let mut pieces = Vec::new();
            if let Some(first) = &item.first_field {
                walk(first, &mut pieces);
                pieces.push(Piece {
                    text: "\t".into(),
                    fmt: Fmt::default(),
                });
            }
            pieces.extend(pieces_of(&item.content));
            out.bibliography.push(merge(trim(pieces)));
        }
    }
    out
}

/// The first item's prefix after the citation's opening bracket, the last
/// item's suffix before its closing one.
fn add_affixes(pieces: &mut Vec<Piece>, cite: &[CiteIn], open: &str, close: &str) {
    let first = cite
        .first()
        .and_then(|c| c.prefix)
        .filter(|p| !p.is_empty());
    let last = cite.last().and_then(|c| c.suffix).filter(|s| !s.is_empty());
    if let Some(prefix) = first {
        let text = with_space_after(prefix);
        let at = match pieces.first_mut() {
            Some(p) if !open.is_empty() && p.text.starts_with(open) => {
                let rest = p.text.split_off(open.len());
                let fmt = p.fmt;
                pieces.insert(1, Piece { text: rest, fmt });
                1
            }
            _ => 0,
        };
        pieces.insert(
            at,
            Piece {
                text,
                fmt: Fmt::default(),
            },
        );
    }
    if let Some(suffix) = last {
        let text = with_space_before(suffix);
        let at = match pieces.last_mut() {
            Some(p) if !close.is_empty() && p.text.ends_with(close) => {
                let keep = p.text.len() - close.len();
                let tail = p.text.split_off(keep);
                let fmt = p.fmt;
                pieces.push(Piece { text: tail, fmt });
                pieces.len() - 1
            }
            _ => pieces.len(),
        };
        pieces.insert(
            at,
            Piece {
                text,
                fmt: Fmt::default(),
            },
        );
    }
}

fn with_space_after(s: &str) -> String {
    if s.ends_with(char::is_whitespace) {
        s.to_owned()
    } else {
        format!("{s} ")
    }
}

fn with_space_before(s: &str) -> String {
    if s.starts_with(char::is_whitespace) || s.starts_with([',', ';', '.', ':']) {
        s.to_owned()
    } else {
        format!(", {s}")
    }
}

fn pieces_of(children: &ElemChildren) -> Vec<Piece> {
    let mut out = Vec::new();
    for c in &children.0 {
        walk(c, &mut out);
    }
    out
}

fn walk(c: &ElemChild, out: &mut Vec<Piece>) {
    match c {
        ElemChild::Text(t) => out.push(Piece {
            text: t.text.clone(),
            fmt: Fmt::of(&t.formatting),
        }),
        ElemChild::Elem(e) => {
            for c in &e.children.0 {
                walk(c, out);
            }
        }
        ElemChild::Link { text, .. } => out.push(Piece {
            text: text.text.clone(),
            fmt: Fmt::of(&text.formatting),
        }),
        ElemChild::Markup(s) => out.push(Piece {
            text: s.clone(),
            fmt: Fmt::default(),
        }),
        ElemChild::Transparent { .. } => {}
    }
}

/// Neighbouring pieces with the same formatting joined; empty ones gone.
fn merge(pieces: Vec<Piece>) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::with_capacity(pieces.len());
    for p in pieces {
        if p.text.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.fmt == p.fmt => last.text.push_str(&p.text),
            _ => out.push(p),
        }
    }
    out
}

/// Leading and trailing white space of an entry gone.
fn trim(mut pieces: Vec<Piece>) -> Vec<Piece> {
    if let Some(p) = pieces.first_mut() {
        p.text = p.text.trim_start().to_owned();
    }
    if let Some(p) = pieces.last_mut() {
        p.text = p.text.trim_end().to_owned();
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::styles::load_style_in;
    use serde_json::json;

    fn nugroho() -> Value {
        json!({"id": 101, "type": "article-journal",
            "title": "Pengaruh media sosial terhadap prestasi belajar mahasiswa",
            "container-title": "Jurnal Pendidikan Indonesia", "volume": "10", "issue": "2",
            "page": "115-130", "DOI": "10.1234/jpi.v10i2.5678",
            "author": [{"family": "Nugroho", "given": "Adi"}],
            "issued": {"date-parts": [[2021, 6]]}})
    }
    fn santoso() -> Value {
        json!({"id": "ITEM-1", "type": "book", "title": "Metodologi penelitian kuantitatif",
            "publisher": "Penerbit Andi", "publisher-place": "Yogyakarta",
            "author": [{"family": "Santoso", "given": "Budi"}, {"family": "Wijaya", "given": "Citra"}],
            "issued": {"date-parts": [["2019"]]}})
    }
    fn rahmawati() -> Value {
        json!({"id": "ITEM-1", "type": "article-journal",
            "title": "Sistem informasi akademik berbasis web",
            "container-title": "Jurnal Teknologi Informasi", "volume": "6", "issue": "1",
            "page": "45-58",
            "author": [{"family": "Rahmawati", "given": "Dewi"}, {"family": "Pratama", "given": "Eko"},
                {"family": "Hidayat", "given": "Fajar"}],
            "issued": {"date-parts": [[2022]]}, "isTemporary": false})
    }

    fn texts(r: &Rendered) -> Vec<String> {
        r.citations.iter().map(|c| plain(c)).collect()
    }

    #[test]
    fn apa_in_indonesian() {
        let style = load_style_in("http://www.zotero.org/styles/apa", None).unwrap();
        let (n, s, r) = (nugroho(), santoso(), rahmawati());
        let mut page = CiteIn::new(&n);
        page.locator = Some("12");
        page.label = Some("page");
        let cites = vec![
            vec![CiteIn::new(&n)],
            vec![page, CiteIn::new(&s)],
            vec![CiteIn::new(&r)],
        ];
        let out = render(&style, Some("id-ID"), &cites);
        let t = texts(&out);
        assert_eq!(t[0], "(Nugroho, 2021)");
        assert!(t[1].starts_with("(Nugroho, 2021, "), "{}", t[1]);
        assert!(t[1].contains("12"), "{}", t[1]);
        assert!(t[1].contains("Santoso & Wijaya, 2019"), "{}", t[1]);
        assert_eq!(t[2], "(Rahmawati dkk., 2022)");
        // Mendeley's per-citation ids don't merge different works.
        assert_eq!(out.bibliography.len(), 3);
        assert!(out.hanging_indent);
        let first = &out.bibliography[0];
        assert!(
            plain(first).starts_with("Nugroho, A. (2021)."),
            "{}",
            plain(first)
        );
        // The journal is italic.
        assert!(
            first
                .iter()
                .any(|p| p.fmt.italic && p.text.contains("Jurnal Pendidikan Indonesia")),
            "{first:?}"
        );
        assert!(
            plain(&out.bibliography[1])
                .starts_with("Rahmawati, D., Pratama, E., & Hidayat, F. (2022).")
        );
    }

    #[test]
    fn apa_in_english_says_et_al() {
        let style = load_style_in("apa", None).unwrap();
        let r = rahmawati();
        let out = render(&style, Some("en-US"), &[vec![CiteIn::new(&r)]]);
        assert_eq!(texts(&out)[0], "(Rahmawati et al., 2022)");
    }

    #[test]
    fn ieee_numbers_by_first_citation() {
        let style = load_style_in("http://www.zotero.org/styles/ieee", None).unwrap();
        let (n, s) = (nugroho(), santoso());
        let cites = vec![
            vec![CiteIn::new(&s)],
            vec![CiteIn::new(&n)],
            vec![CiteIn::new(&s)],
        ];
        let out = render(&style, Some("en-US"), &cites);
        assert_eq!(texts(&out), ["[1]", "[2]", "[1]"]);
        assert!(out.numbered);
        assert_eq!(out.bibliography.len(), 2);
        assert!(
            plain(&out.bibliography[0]).starts_with("[1]\t"),
            "{:?}",
            out.bibliography[0]
        );
        assert!(plain(&out.bibliography[0]).contains("Santoso"));
    }

    #[test]
    fn affixes_and_suppressed_authors() {
        let style = load_style_in("apa", None).unwrap();
        let n = nugroho();
        let mut c = CiteIn::new(&n);
        c.prefix = Some("lihat");
        c.suffix = Some("untuk ulasan");
        let mut year = CiteIn::new(&n);
        year.suppress_author = true;
        let out = render(&style, Some("id-ID"), &[vec![c], vec![year]]);
        assert_eq!(texts(&out)[0], "(lihat Nugroho, 2021, untuk ulasan)");
        assert_eq!(texts(&out)[1], "(2021)");
    }

    #[test]
    fn odd_fields_are_dropped_not_fatal() {
        let v = json!({"id": 1, "type": "book", "title": "T", "flag": true,
            "issued": {"raw": "June 2019"}, "score": 1.5});
        let item = to_item(&v, "k");
        assert!(item.0.contains_key("issued"));
        assert!(!item.0.contains_key("flag"));
        assert!(item.0.contains_key("score"));
    }

    #[test]
    fn other_bundled_styles_render() {
        let n = nugroho();
        for (id, _) in crate::BUNDLED {
            let style = load_style_in(id, None).unwrap();
            let out = render(&style, Some("en-US"), &[vec![CiteIn::new(&n)]]);
            assert!(!plain(&out.citations[0]).is_empty(), "{id}");
            assert!(
                plain(&out.bibliography[0]).contains("Pengaruh media sosial"),
                "{id}"
            );
        }
    }
}
