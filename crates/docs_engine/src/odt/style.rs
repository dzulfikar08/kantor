//! ODF styles: the style tables of `styles.xml` and `content.xml`, their
//! properties as WordprocessingML (`w:rPr`, `w:pPr`), Word style ids for
//! named styles, and list styles as numbering levels.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use super::*;

/// Where an element's automatic styles live: body text uses those of
/// `content.xml`, headers and footers those of `styles.xml`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Scope {
    Content,
    Styles,
}

type Key<'a> = (&'a str, &'a str);

pub(crate) struct Styles<'a> {
    named: HashMap<Key<'a>, Node<'a>>,
    auto_content: HashMap<Key<'a>, Node<'a>>,
    auto_styles: HashMap<Key<'a>, Node<'a>>,
    defaults: HashMap<&'a str, Node<'a>>,
    lists_named: HashMap<&'a str, Node<'a>>,
    lists_content: HashMap<&'a str, Node<'a>>,
    lists_styles: HashMap<&'a str, Node<'a>>,
    fonts: HashMap<&'a str, String>,
    pub masters: Vec<Node<'a>>,
    pub layouts: HashMap<&'a str, Node<'a>>,
    pub outline: Option<Node<'a>>,
    pub settings: Settings,
    /// Word style ids of named paragraph and text styles.
    ids: HashMap<String, String>,
    /// Named paragraph and text styles in file order (for styles.xml).
    pub order: Vec<Key<'a>>,
}

/// The display name of a named style.
pub(crate) fn display_name<'a>(n: Node<'a>) -> String {
    if let Some(d) = attr(n, NS_STYLE, "display-name") {
        return d.to_owned();
    }
    decode_name(attr(n, NS_STYLE, "name").unwrap_or(""))
}

/// `Heading_20_1` → `Heading 1` (ODF encodes characters as `_xx_` hex).
fn decode_name(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('_') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(j) = after.find('_')
            && j > 0
            && j <= 6
            && let Ok(v) = u32::from_str_radix(&after[..j], 16)
            && let Some(c) = char::from_u32(v)
        {
            out.push(c);
            rest = &after[j + 1..];
        } else {
            out.push('_');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Word's style id for a name: words capitalised, joined, letters and
/// digits only ("heading 1" → "Heading1").
pub(crate) fn style_id(name: &str) -> String {
    let mut id = String::new();
    for w in name.split_whitespace() {
        let mut cs = w.chars();
        if let Some(f) = cs.next() {
            id.extend(f.to_uppercase());
            id.extend(cs);
        }
    }
    id.retain(|c| c.is_alphanumeric());
    id
}

/// Word's id and name for a LibreOffice style, so Word's own styles
/// (Normal, Heading 1, Hyperlink…) are recognised.
pub(crate) fn word_style(family: &str, name: &str, display: &str) -> (String, String) {
    let lower = display.to_lowercase();
    let pair = |id: &str, n: &str| (id.to_owned(), n.to_owned());
    if family == "paragraph" {
        if name == "Standard" || lower == "default paragraph style" {
            return pair("Normal", "Normal");
        }
        if let Some(n) = lower.strip_prefix("heading ")
            && let Ok(n) = n.trim().parse::<u8>()
            && (1..=9).contains(&n)
        {
            return (format!("Heading{n}"), format!("heading {n}"));
        }
        if let Some(n) = lower.strip_prefix("contents ")
            && let Ok(n) = n.trim().parse::<u8>()
            && (1..=9).contains(&n)
        {
            return (format!("TOC{n}"), format!("toc {n}"));
        }
        match lower.as_str() {
            "title" => return pair("Title", "Title"),
            "subtitle" => return pair("Subtitle", "Subtitle"),
            "text body" | "body text" => return pair("BodyText", "Body Text"),
            "header" => return pair("Header", "header"),
            "footer" => return pair("Footer", "footer"),
            "footnote" | "footnote text" => return pair("FootnoteText", "footnote text"),
            "endnote" | "endnote text" => return pair("EndnoteText", "endnote text"),
            "caption" => return pair("Caption", "caption"),
            "list paragraph" => return pair("ListParagraph", "List Paragraph"),
            "quotations" | "quote" => return pair("Quote", "Quote"),
            "contents heading" | "toc heading" => return pair("TOCHeading", "TOC Heading"),
            _ => {}
        }
    } else {
        match lower.as_str() {
            "internet link" | "hyperlink" => return pair("Hyperlink", "Hyperlink"),
            "visited internet link" | "followedhyperlink" => {
                return pair("FollowedHyperlink", "FollowedHyperlink");
            }
            "strong emphasis" | "strong" => return pair("Strong", "Strong"),
            "emphasis" => return pair("Emphasis", "Emphasis"),
            "footnote anchor" | "footnote reference" => {
                return pair("FootnoteReference", "footnote reference");
            }
            "endnote anchor" | "endnote reference" => {
                return pair("EndnoteReference", "endnote reference");
            }
            _ => {}
        }
    }
    let mut id = style_id(display);
    if id.is_empty() || id.starts_with(|c: char| c.is_ascii_digit()) {
        id = format!("Style{id}");
    }
    (id, display.to_owned())
}

impl<'a> Styles<'a> {
    pub fn read(
        content: &'a roxmltree::Document<'a>,
        styles: Option<&'a roxmltree::Document<'a>>,
        settings: Option<&roxmltree::Document>,
    ) -> Self {
        let mut s = Styles {
            named: HashMap::new(),
            auto_content: HashMap::new(),
            auto_styles: HashMap::new(),
            defaults: HashMap::new(),
            lists_named: HashMap::new(),
            lists_content: HashMap::new(),
            lists_styles: HashMap::new(),
            fonts: HashMap::new(),
            masters: Vec::new(),
            layouts: HashMap::new(),
            outline: None,
            settings: read_settings(settings),
            ids: HashMap::new(),
            order: Vec::new(),
        };
        let flat = styles.is_none();
        let mut roots = vec![(content.root_element(), Scope::Content)];
        if let Some(d) = styles {
            roots.push((d.root_element(), Scope::Styles));
        }
        for (root, scope) in roots {
            for sec in root.children().filter(|n| n.is_element()) {
                if is(sec, NS_OFFICE, "font-face-decls") {
                    for f in sec.children().filter(|f| is(*f, NS_STYLE, "font-face")) {
                        if let Some(name) = attr(f, NS_STYLE, "name") {
                            let fam = attr(f, NS_SVG, "font-family")
                                .map(unquote)
                                .filter(|f| !f.is_empty())
                                .unwrap_or_else(|| name.to_owned());
                            s.fonts.insert(name, fam);
                        }
                    }
                } else if is(sec, NS_OFFICE, "styles") {
                    for st in sec.children().filter(|n| n.is_element()) {
                        if is(st, NS_STYLE, "style") {
                            if let (Some(f), Some(n)) =
                                (attr(st, NS_STYLE, "family"), attr(st, NS_STYLE, "name"))
                            {
                                s.named.insert((f, n), st);
                                if matches!(f, "paragraph" | "text") {
                                    s.order.push((f, n));
                                }
                            }
                        } else if is(st, NS_STYLE, "default-style") {
                            if let Some(f) = attr(st, NS_STYLE, "family") {
                                s.defaults.insert(f, st);
                            }
                        } else if is(st, NS_TEXT, "list-style") {
                            if let Some(n) = attr(st, NS_STYLE, "name") {
                                s.lists_named.insert(n, st);
                            }
                        } else if is(st, NS_TEXT, "outline-style") {
                            s.outline = Some(st);
                        }
                    }
                } else if is(sec, NS_OFFICE, "automatic-styles") {
                    for st in sec.children().filter(|n| n.is_element()) {
                        let name = attr(st, NS_STYLE, "name");
                        if is(st, NS_STYLE, "style") {
                            if let (Some(f), Some(n)) = (attr(st, NS_STYLE, "family"), name) {
                                if scope == Scope::Content {
                                    s.auto_content.insert((f, n), st);
                                }
                                if scope == Scope::Styles || flat {
                                    s.auto_styles.insert((f, n), st);
                                }
                            }
                        } else if is(st, NS_TEXT, "list-style") {
                            if let Some(n) = name {
                                if scope == Scope::Content {
                                    s.lists_content.insert(n, st);
                                }
                                if scope == Scope::Styles || flat {
                                    s.lists_styles.insert(n, st);
                                }
                            }
                        } else if is(st, NS_STYLE, "page-layout")
                            && let Some(n) = name
                        {
                            s.layouts.insert(n, st);
                        }
                    }
                } else if is(sec, NS_OFFICE, "master-styles") {
                    s.masters
                        .extend(sec.children().filter(|m| is(*m, NS_STYLE, "master-page")));
                }
            }
        }
        // Word ids, unique across paragraph and character styles.
        let mut seen = HashSet::new();
        for &(f, n) in &s.order {
            let Some(node) = s.named.get(&(f, n)) else {
                continue;
            };
            let (base, _) = word_style(f, n, &display_name(*node));
            let mut id = base.clone();
            let mut k = 1;
            while !seen.insert(id.to_lowercase()) {
                k += 1;
                id = format!("{base}{k}");
            }
            s.ids.insert(format!("{f}\0{n}"), id);
        }
        s
    }

    /// The style `name` of `family` as seen from `scope`: an automatic one
    /// of that part, else a named one.
    pub fn get(&self, scope: Scope, family: &str, name: &str) -> Option<Node<'a>> {
        let auto = match scope {
            Scope::Content => &self.auto_content,
            Scope::Styles => &self.auto_styles,
        };
        auto.get(&(family, name))
            .or_else(|| self.named.get(&(family, name)))
            .copied()
    }

    /// Whether `name` is a named (not automatic) style in `scope`.
    pub fn is_named(&self, scope: Scope, family: &str, name: &str) -> bool {
        let auto = match scope {
            Scope::Content => &self.auto_content,
            Scope::Styles => &self.auto_styles,
        };
        !auto.contains_key(&(family, name)) && self.named.contains_key(&(family, name))
    }

    /// The Word style id of a named style.
    pub fn word_id(&self, family: &str, name: &str) -> Option<&str> {
        self.ids
            .get(&format!("{family}\0{name}"))
            .map(String::as_str)
    }

    /// For a style used by an element: the named style it stands for (the
    /// style itself, or an automatic style's parent) and the automatic
    /// style, if any.
    pub fn split(
        &self,
        scope: Scope,
        family: &str,
        name: &str,
    ) -> (Option<&'a str>, Option<Node<'a>>) {
        if self.is_named(scope, family, name) {
            let named: &HashMap<(&str, &str), Node<'a>> = &self.named;
            let key = named
                .get(&(family, name))
                .and_then(|n| attr(*n, NS_STYLE, "name"));
            return (key, None);
        }
        match self.get(scope, family, name) {
            Some(n) => (attr(n, NS_STYLE, "parent-style-name"), Some(n)),
            None => (None, None),
        }
    }

    /// An attribute of the `el` properties, following parent styles, then
    /// the family's default style.
    pub fn prop(
        &self,
        scope: Scope,
        family: &str,
        name: Option<&str>,
        el: &str,
        ns: &str,
        at: &str,
    ) -> Option<&'a str> {
        let mut cur = name.and_then(|n| self.get(scope, family, n));
        for _ in 0..32 {
            let Some(n) = cur else { break };
            if let Some(v) = child(n, NS_STYLE, el).and_then(|p| attr(p, ns, at)) {
                return Some(v);
            }
            cur = attr(n, NS_STYLE, "parent-style-name").and_then(|p| self.get(scope, family, p));
        }
        self.defaults
            .get(family)
            .and_then(|d| child(*d, NS_STYLE, el))
            .and_then(|p| attr(p, ns, at))
    }

    /// The font size in points of a paragraph or text style, resolving
    /// percentages against the parents (12 pt with nothing set).
    pub fn size(&self, scope: Scope, family: &str, name: Option<&str>) -> f64 {
        let mut pcts = 1.0;
        let mut cur = name.and_then(|n| self.get(scope, family, n));
        for _ in 0..32 {
            let Some(n) = cur else { break };
            if let Some(v) =
                child(n, NS_STYLE, "text-properties").and_then(|p| attr(p, NS_FO, "font-size"))
            {
                if let Some(p) = points(v) {
                    return p * pcts;
                }
                if let Some(f) = percent(v) {
                    pcts *= f;
                }
            }
            let parent = attr(n, NS_STYLE, "parent-style-name");
            cur = parent.and_then(|p| self.get(scope, family, p));
            // Text styles without a size take the paragraph default.
            if cur.is_none() && family == "text" {
                break;
            }
        }
        let def = self
            .defaults
            .get("paragraph")
            .and_then(|d| child(*d, NS_STYLE, "text-properties"))
            .and_then(|p| attr(p, NS_FO, "font-size"))
            .and_then(points)
            .unwrap_or(12.0);
        def * pcts
    }

    /// A list style by name, as seen from `scope`.
    pub fn list(&self, scope: Scope, name: &str) -> Option<Node<'a>> {
        let auto = match scope {
            Scope::Content => &self.lists_content,
            Scope::Styles => &self.lists_styles,
        };
        auto.get(name)
            .or_else(|| self.lists_named.get(name))
            .copied()
    }

    pub fn master(&self, name: &str) -> Option<Node<'a>> {
        self.masters
            .iter()
            .find(|m| attr(**m, NS_STYLE, "name") == Some(name))
            .copied()
    }

    pub fn font(&self, name: &str) -> String {
        self.fonts
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_owned())
    }

    pub fn default_tab(&self) -> i32 {
        self.defaults
            .get("paragraph")
            .and_then(|d| child(*d, NS_STYLE, "paragraph-properties"))
            .and_then(|p| attr(p, NS_STYLE, "tab-stop-distance"))
            .and_then(twips)
            .filter(|t| *t > 0)
            .unwrap_or(720)
    }

    pub fn default_style(&self, family: &str) -> Option<Node<'a>> {
        self.defaults.get(family).copied()
    }

    pub fn named(&self, family: &str, name: &str) -> Option<Node<'a>> {
        self.named.get(&(family, name)).copied()
    }
}

fn unquote(s: &str) -> String {
    let first = s.split(',').next().unwrap_or(s).trim();
    first.trim_matches(|c| c == '\'' || c == '"').to_owned()
}

// ── character properties ───────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Size {
    Pt(f64),
    Pct(f64),
}

#[derive(Clone, Default, Debug)]
pub(crate) struct TextProps {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<&'static str>,
    pub strike: Option<bool>,
    pub dstrike: Option<bool>,
    pub caps: Option<bool>,
    pub small_caps: Option<bool>,
    pub hidden: Option<bool>,
    pub outline: Option<bool>,
    pub shadow: Option<bool>,
    pub emboss: Option<bool>,
    pub imprint: Option<bool>,
    pub size: Option<Size>,
    pub font: Option<String>,
    pub font_asian: Option<String>,
    pub font_cs: Option<String>,
    /// `auto` or `RRGGBB`.
    pub color: Option<String>,
    /// Background: `Some(None)` is transparent.
    pub bg: Option<Option<String>>,
    pub valign: Option<&'static str>,
    pub spacing: Option<i32>,
    pub scale: Option<u32>,
    pub lang: Option<String>,
    pub lang_asian: Option<String>,
    pub lang_cs: Option<String>,
}

macro_rules! over {
    ($a:ident, $b:ident, $($f:ident),*) => {
        TextProps { $($f: $b.$f.clone().or_else(|| $a.$f.clone()),)* }
    };
}

impl TextProps {
    /// `top` over `self`.
    pub fn over(&self, top: &TextProps) -> TextProps {
        let a = self;
        let b = top;
        over!(
            a, b, bold, italic, underline, strike, dstrike, caps, small_caps, hidden, outline,
            shadow, emboss, imprint, size, font, font_asian, font_cs, color, bg, valign, spacing,
            scale, lang, lang_asian, lang_cs
        )
    }

    /// The text properties of a style element (`style:style`).
    pub fn of_style(st: &Styles, style: Option<Node>) -> TextProps {
        match style.and_then(|s| child(s, NS_STYLE, "text-properties")) {
            Some(p) => Self::read(st, p),
            None => TextProps::default(),
        }
    }

    /// Read `style:text-properties`.
    pub fn read(st: &Styles, p: Node) -> TextProps {
        let a = |ns, n| attr(p, ns, n);
        let mut t = TextProps::default();
        if let Some(w) = a(NS_FO, "font-weight") {
            t.bold = Some(match w {
                "bold" | "bolder" => true,
                "normal" | "lighter" => false,
                n => n.parse::<u32>().is_ok_and(|n| n >= 600),
            });
        }
        if let Some(s) = a(NS_FO, "font-style") {
            t.italic = Some(s != "normal");
        }
        if let Some(u) = a(NS_STYLE, "text-underline-style") {
            let double = a(NS_STYLE, "text-underline-type") == Some("double");
            let thick = a(NS_STYLE, "text-underline-width") == Some("bold");
            let words = a(NS_STYLE, "text-underline-mode") == Some("skip-white-space");
            t.underline = Some(match u {
                "none" => "none",
                "dotted" if thick => "dottedHeavy",
                "dotted" => "dotted",
                "dash" if thick => "dashedHeavy",
                "dash" => "dash",
                "long-dash" => "dashLong",
                "dot-dash" => "dotDash",
                "dot-dot-dash" => "dotDotDash",
                "wave" if double => "wavyDouble",
                "wave" => "wave",
                _ if double => "double",
                _ if thick => "thick",
                _ if words => "words",
                _ => "single",
            });
        } else if a(NS_STYLE, "text-underline-type") == Some("none") {
            t.underline = Some("none");
        }
        let through = a(NS_STYLE, "text-line-through-style")
            .map(|s| s != "none")
            .or_else(|| a(NS_STYLE, "text-line-through-type").map(|s| s != "none"));
        if let Some(on) = through {
            if on && a(NS_STYLE, "text-line-through-type") == Some("double") {
                t.dstrike = Some(true);
                t.strike = Some(false);
            } else {
                t.strike = Some(on);
                t.dstrike = Some(false);
            }
        }
        if let Some(v) = a(NS_FO, "font-variant") {
            t.small_caps = Some(v == "small-caps");
        }
        if let Some(v) = a(NS_FO, "text-transform") {
            t.caps = Some(v == "uppercase");
        }
        if let Some(v) = a(NS_TEXT, "display") {
            t.hidden = Some(v == "none");
        }
        if let Some(v) = a(NS_STYLE, "text-outline") {
            t.outline = Some(v == "true");
        }
        if let Some(v) = a(NS_FO, "text-shadow") {
            t.shadow = Some(v != "none");
        }
        if let Some(v) = a(NS_STYLE, "font-relief") {
            t.emboss = Some(v == "embossed");
            t.imprint = Some(v == "engraved");
        }
        if let Some(v) = a(NS_FO, "font-size") {
            t.size = points(v)
                .map(Size::Pt)
                .or_else(|| percent(v).map(Size::Pct));
        }
        let font = |name: Option<&str>, family: Option<&str>| {
            name.map(|n| st.font(n))
                .or_else(|| family.map(unquote))
                .filter(|f| !f.is_empty())
        };
        t.font = font(a(NS_STYLE, "font-name"), a(NS_FO, "font-family"));
        t.font_asian = font(
            a(NS_STYLE, "font-name-asian"),
            a(NS_STYLE, "font-family-asian"),
        );
        t.font_cs = font(
            a(NS_STYLE, "font-name-complex"),
            a(NS_STYLE, "font-family-complex"),
        );
        if a(NS_STYLE, "use-window-font-color") == Some("true") {
            t.color = Some("auto".into());
        } else if let Some(c) = a(NS_FO, "color").and_then(color) {
            t.color = Some(c);
        }
        if let Some(b) = a(NS_FO, "background-color") {
            t.bg = Some(color(b));
        }
        if let Some(pos) = a(NS_STYLE, "text-position") {
            let first = pos.split_whitespace().next().unwrap_or("");
            t.valign = Some(match first {
                "super" => "superscript",
                "sub" => "subscript",
                p => match percent(p) {
                    Some(v) if v > 0.0 => "superscript",
                    Some(v) if v < 0.0 => "subscript",
                    _ => "baseline",
                },
            });
        }
        if let Some(s) = a(NS_FO, "letter-spacing") {
            t.spacing = Some(if s == "normal" {
                0
            } else {
                twips(s).unwrap_or(0)
            });
        }
        if let Some(s) = a(NS_STYLE, "text-scale").and_then(percent) {
            t.scale = Some((s * 100.0).round().clamp(1.0, 600.0) as u32);
        }
        let lang = |l: Option<&str>, c: Option<&str>| {
            let l = l.filter(|l| !matches!(*l, "zxx" | "none" | ""))?;
            Some(match c.filter(|c| !matches!(*c, "none" | "")) {
                Some(c) => format!("{l}-{c}"),
                None => l.to_owned(),
            })
        };
        t.lang = lang(a(NS_FO, "language"), a(NS_FO, "country"));
        t.lang_asian = lang(a(NS_STYLE, "language-asian"), a(NS_STYLE, "country-asian"));
        t.lang_cs = lang(
            a(NS_STYLE, "language-complex"),
            a(NS_STYLE, "country-complex"),
        );
        t
    }

    /// The `w:rPr` children (no wrapper), in schema order. Percent sizes
    /// are taken of `base_pt`.
    pub fn xml(&self, rstyle: Option<&str>, base_pt: f64) -> String {
        let mut x = String::new();
        if let Some(s) = rstyle {
            let _ = write!(x, "<w:rStyle w:val=\"{}\"/>", esc(s));
        }
        if self.font.is_some() || self.font_asian.is_some() || self.font_cs.is_some() {
            x.push_str("<w:rFonts");
            if let Some(f) = &self.font {
                let _ = write!(x, " w:ascii=\"{0}\" w:hAnsi=\"{0}\"", esc(f));
            }
            if let Some(f) = &self.font_asian {
                let _ = write!(x, " w:eastAsia=\"{}\"", esc(f));
            }
            if let Some(f) = &self.font_cs {
                let _ = write!(x, " w:cs=\"{}\"", esc(f));
            }
            x.push_str("/>");
        }
        let toggle = |x: &mut String, tag: &str, v: Option<bool>| match v {
            Some(true) => {
                let _ = write!(x, "<w:{tag}/>");
            }
            Some(false) => {
                let _ = write!(x, "<w:{tag} w:val=\"0\"/>");
            }
            None => {}
        };
        toggle(&mut x, "b", self.bold);
        toggle(&mut x, "bCs", self.bold);
        toggle(&mut x, "i", self.italic);
        toggle(&mut x, "iCs", self.italic);
        toggle(&mut x, "caps", self.caps);
        toggle(&mut x, "smallCaps", self.small_caps);
        toggle(&mut x, "strike", self.strike);
        toggle(&mut x, "dstrike", self.dstrike);
        toggle(&mut x, "outline", self.outline);
        toggle(&mut x, "shadow", self.shadow);
        toggle(&mut x, "emboss", self.emboss);
        toggle(&mut x, "imprint", self.imprint);
        toggle(&mut x, "vanish", self.hidden);
        if let Some(c) = &self.color {
            let _ = write!(x, "<w:color w:val=\"{c}\"/>");
        }
        if let Some(s) = self.spacing {
            let _ = write!(x, "<w:spacing w:val=\"{s}\"/>");
        }
        if let Some(s) = self.scale {
            let _ = write!(x, "<w:w w:val=\"{s}\"/>");
        }
        if let Some(sz) = self.size {
            let pt = match sz {
                Size::Pt(p) => p,
                Size::Pct(f) => base_pt * f,
            };
            let hp = (pt * 2.0).round().clamp(1.0, 3276.0) as u32;
            let _ = write!(x, "<w:sz w:val=\"{hp}\"/><w:szCs w:val=\"{hp}\"/>");
        }
        match &self.bg {
            Some(Some(c)) => match highlight(c) {
                Some(h) => {
                    let _ = write!(x, "<w:highlight w:val=\"{h}\"/>");
                }
                None => {
                    let _ = write!(
                        x,
                        "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{c}\"/>"
                    );
                }
            },
            Some(None) => x.push_str("<w:highlight w:val=\"none\"/>"),
            None => {}
        }
        if let Some(u) = self.underline {
            let _ = write!(x, "<w:u w:val=\"{u}\"/>");
        }
        if let Some(v) = self.valign {
            let _ = write!(x, "<w:vertAlign w:val=\"{v}\"/>");
        }
        if self.lang.is_some() || self.lang_asian.is_some() || self.lang_cs.is_some() {
            x.push_str("<w:lang");
            if let Some(l) = &self.lang {
                let _ = write!(x, " w:val=\"{}\"", esc(l));
            }
            if let Some(l) = &self.lang_asian {
                let _ = write!(x, " w:eastAsia=\"{}\"", esc(l));
            }
            if let Some(l) = &self.lang_cs {
                let _ = write!(x, " w:bidi=\"{}\"", esc(l));
            }
            x.push_str("/>");
        }
        x
    }
}

/// Word's highlight name for a background colour, if it is one of them.
fn highlight(c: &str) -> Option<&'static str> {
    Some(match c {
        "FFFF00" => "yellow",
        "00FF00" => "green",
        "00FFFF" => "cyan",
        "FF00FF" => "magenta",
        "0000FF" => "blue",
        "FF0000" => "red",
        "000080" => "darkBlue",
        "008080" => "darkCyan",
        "008000" => "darkGreen",
        "800080" => "darkMagenta",
        "800000" => "darkRed",
        "808000" => "darkYellow",
        "808080" => "darkGray",
        "C0C0C0" => "lightGray",
        "000000" => "black",
        _ => return None,
    })
}

// ── paragraph properties ───────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub(crate) struct Tab {
    pub pos: i32,
    pub val: &'static str,
    pub leader: Option<&'static str>,
}

#[derive(Clone, Default, Debug)]
pub(crate) struct ParaProps {
    pub align: Option<&'static str>,
    pub left: Option<i32>,
    pub right: Option<i32>,
    /// First-line indent (negative: hanging).
    pub first: Option<i32>,
    pub before: Option<i32>,
    pub after: Option<i32>,
    pub line: Option<(i32, &'static str)>,
    pub keep_next: Option<bool>,
    pub keep_lines: Option<bool>,
    pub break_before: Option<&'static str>,
    pub break_after: Option<&'static str>,
    pub widows: Option<bool>,
    pub tabs: Option<Vec<Tab>>,
    pub shd: Option<Option<String>>,
    /// `w:pBdr` children, top/left/bottom/right.
    pub borders: [Option<String>; 4],
    pub bidi: Option<bool>,
    pub contextual: Option<bool>,
    /// `style:page-number`: `Some(None)` is "auto".
    pub page_number: Option<Option<u32>>,
}

impl ParaProps {
    pub fn of_style(style: Option<Node>) -> ParaProps {
        match style.and_then(|s| child(s, NS_STYLE, "paragraph-properties")) {
            Some(p) => Self::read(p),
            None => ParaProps::default(),
        }
    }

    pub fn read(p: Node) -> ParaProps {
        let a = |ns, n| attr(p, ns, n);
        let align = a(NS_FO, "text-align").map(|v| {
            let rtl = a(NS_STYLE, "writing-mode").is_some_and(|w| w.starts_with("rl"));
            match v {
                "start" if rtl => "right",
                "end" if rtl => "left",
                "start" | "left" => "left",
                "end" | "right" => "right",
                "center" => "center",
                "justify" => "both",
                _ => "left",
            }
        });
        let mut o = ParaProps {
            align,
            ..Default::default()
        };
        o.left = a(NS_FO, "margin-left").and_then(twips);
        o.right = a(NS_FO, "margin-right").and_then(twips);
        o.first = a(NS_FO, "text-indent").and_then(twips);
        o.before = a(NS_FO, "margin-top").and_then(twips);
        o.after = a(NS_FO, "margin-bottom").and_then(twips);
        if let Some(lh) = a(NS_FO, "line-height") {
            o.line = if lh == "normal" {
                Some((240, "auto"))
            } else if let Some(f) = percent(lh) {
                Some(((f * 240.0).round().clamp(1.0, 31680.0) as i32, "auto"))
            } else {
                twips(lh).map(|t| (t.max(1), "exact"))
            };
        } else if let Some(t) = a(NS_STYLE, "line-height-at-least").and_then(twips) {
            o.line = Some((t.max(1), "atLeast"));
        }
        if let Some(v) = a(NS_FO, "keep-with-next") {
            o.keep_next = Some(v == "always");
        }
        if let Some(v) = a(NS_FO, "keep-together") {
            o.keep_lines = Some(v == "always");
        }
        o.break_before = a(NS_FO, "break-before").map(brk);
        o.break_after = a(NS_FO, "break-after").map(brk);
        if let Some(w) = a(NS_FO, "widows").or(a(NS_FO, "orphans")) {
            o.widows = Some(w != "0");
        }
        if let Some(ts) = child(p, NS_STYLE, "tab-stops") {
            let mut tabs = Vec::new();
            for t in ts.children().filter(|t| is(*t, NS_STYLE, "tab-stop")) {
                let Some(pos) = attr(t, NS_STYLE, "position").and_then(twips) else {
                    continue;
                };
                let val = match attr(t, NS_STYLE, "type") {
                    Some("center") => "center",
                    Some("right") => "right",
                    Some("char") => "decimal",
                    _ => "left",
                };
                let leader = match attr(t, NS_STYLE, "leader-text") {
                    Some(".") | Some("…") => Some("dot"),
                    Some("-") => Some("hyphen"),
                    Some("_") => Some("underscore"),
                    Some("·") => Some("middleDot"),
                    Some(" ") | None => match attr(t, NS_STYLE, "leader-style") {
                        Some("dotted") => Some("dot"),
                        Some("dash") | Some("long-dash") => Some("hyphen"),
                        Some("solid") => Some("underscore"),
                        _ => None,
                    },
                    _ => None,
                };
                tabs.push(Tab { pos, val, leader });
            }
            o.tabs = Some(tabs);
        }
        if let Some(b) = a(NS_FO, "background-color") {
            o.shd = Some(color(b));
        }
        let pad = |side: &str| {
            attr(p, NS_FO, &format!("padding-{side}"))
                .or(a(NS_FO, "padding"))
                .and_then(points)
                .unwrap_or(0.0)
        };
        for (i, side) in ["top", "left", "bottom", "right"].iter().enumerate() {
            let b = attr(p, NS_FO, &format!("border-{side}")).or(a(NS_FO, "border"));
            o.borders[i] = b.and_then(|b| border_xml(side, b, Some(pad(side))));
        }
        if let Some(w) = a(NS_STYLE, "writing-mode") {
            o.bidi = Some(w.starts_with("rl"));
        }
        if let Some(c) = a(NS_STYLE, "contextual-spacing") {
            o.contextual = Some(c == "true");
        }
        if let Some(n) = a(NS_STYLE, "page-number") {
            o.page_number = Some(n.parse().ok().filter(|n| *n > 0));
        }
        o
    }
}

fn brk(v: &str) -> &'static str {
    match v {
        "page" | "even-page" | "odd-page" => "page",
        "column" => "column",
        _ => "auto",
    }
}

/// A border (`0.5pt solid #000000`) as `<w:{side} …/>`; `None` for no line.
pub(crate) fn border_xml(side: &str, s: &str, space_pt: Option<f64>) -> Option<String> {
    let mut width = None;
    let mut kind = "solid";
    let mut rgb = None;
    for part in s.split_whitespace() {
        if part == "none" || part == "hidden" {
            return Some(format!("<w:{side} w:val=\"nil\"/>"));
        } else if let Some(c) = color(part) {
            rgb = Some(c);
        } else if let Some(w) = points(part) {
            width = Some(w);
        } else {
            kind = part;
        }
    }
    let w = width.unwrap_or(0.75);
    if w <= 0.0 {
        return Some(format!("<w:{side} w:val=\"nil\"/>"));
    }
    let val = match kind {
        "double" | "double-thin" => "double",
        "dotted" => "dotted",
        "dashed" | "long-dash" => "dashed",
        "fine-dashed" => "dashSmallGap",
        "dash-dot" => "dotDash",
        "dash-dot-dot" => "dotDotDash",
        "groove" => "threeDEngrave",
        "ridge" => "threeDEmboss",
        "inset" => "inset",
        "outset" => "outset",
        _ => "single",
    };
    let sz = ((w * 8.0).round() as i64).clamp(2, 96);
    let space = space_pt
        .map(|p| format!(" w:space=\"{}\"", p.round().clamp(0.0, 31.0) as i64))
        .unwrap_or_default();
    Some(format!(
        "<w:{side} w:val=\"{val}\" w:sz=\"{sz}\"{space} w:color=\"{}\"/>",
        rgb.as_deref().unwrap_or("auto")
    ))
}

/// What a paragraph's `w:pPr` holds besides its properties.
#[derive(Default)]
pub(crate) struct PExtra<'s> {
    pub style: Option<&'s str>,
    /// `w:numPr`: num id, level.
    pub num: Option<(u32, u8)>,
    /// Left indent and hanging that replace the properties' (a list item's
    /// later paragraph).
    pub ind: Option<(i32, i32)>,
    pub outline: Option<u8>,
    pub page_break: bool,
    /// Added to tab positions (LibreOffice counts them from the indent).
    pub tab_shift: i32,
    /// The paragraph mark's `w:rPr` children.
    pub rpr: String,
    pub sect: String,
}

/// `w:pPr` (with its wrapper, empty when there is nothing to say).
pub(crate) fn ppr_xml(p: &ParaProps, e: &PExtra) -> String {
    let mut x = String::new();
    if let Some(s) = e.style {
        let _ = write!(x, "<w:pStyle w:val=\"{}\"/>", esc(s));
    }
    let toggle = |x: &mut String, tag: &str, v: Option<bool>| match v {
        Some(true) => {
            let _ = write!(x, "<w:{tag}/>");
        }
        Some(false) => {
            let _ = write!(x, "<w:{tag} w:val=\"0\"/>");
        }
        None => {}
    };
    toggle(&mut x, "keepNext", p.keep_next);
    toggle(&mut x, "keepLines", p.keep_lines);
    if e.page_break || p.break_before == Some("page") {
        x.push_str("<w:pageBreakBefore/>");
    }
    toggle(&mut x, "widowControl", p.widows);
    if let Some((id, l)) = e.num {
        let _ = write!(
            x,
            "<w:numPr><w:ilvl w:val=\"{l}\"/><w:numId w:val=\"{id}\"/></w:numPr>"
        );
    }
    if p.borders.iter().any(Option::is_some) {
        x.push_str("<w:pBdr>");
        for b in p.borders.iter().flatten() {
            x.push_str(b);
        }
        x.push_str("</w:pBdr>");
    }
    match &p.shd {
        Some(Some(c)) => {
            let _ = write!(
                x,
                "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{c}\"/>"
            );
        }
        Some(None) => x.push_str("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"auto\"/>"),
        None => {}
    }
    if let Some(tabs) = &p.tabs
        && !tabs.is_empty()
    {
        x.push_str("<w:tabs>");
        for t in tabs {
            let _ = write!(
                x,
                "<w:tab w:val=\"{}\"{} w:pos=\"{}\"/>",
                t.val,
                t.leader
                    .map(|l| format!(" w:leader=\"{l}\""))
                    .unwrap_or_default(),
                (t.pos + e.tab_shift).clamp(-31680, 31680)
            );
        }
        x.push_str("</w:tabs>");
    }
    toggle(&mut x, "bidi", p.bidi);
    if p.before.is_some() || p.after.is_some() || p.line.is_some() {
        x.push_str("<w:spacing");
        if let Some(b) = p.before {
            let _ = write!(x, " w:before=\"{}\"", b.max(0));
        }
        if let Some(a) = p.after {
            let _ = write!(x, " w:after=\"{}\"", a.max(0));
        }
        if let Some((l, r)) = p.line {
            let _ = write!(x, " w:line=\"{l}\" w:lineRule=\"{r}\"");
        }
        x.push_str("/>");
    }
    if let Some((left, hanging)) = e.ind {
        let _ = write!(x, "<w:ind w:left=\"{left}\"");
        if let Some(r) = p.right {
            let _ = write!(x, " w:right=\"{r}\"");
        }
        let _ = write!(x, " w:hanging=\"{hanging}\"/>");
    } else if p.left.is_some() || p.right.is_some() || p.first.is_some() {
        x.push_str("<w:ind");
        if let Some(l) = p.left {
            let _ = write!(x, " w:left=\"{l}\"");
        }
        if let Some(r) = p.right {
            let _ = write!(x, " w:right=\"{r}\"");
        }
        match p.first {
            Some(f) if f < 0 => {
                let _ = write!(x, " w:hanging=\"{}\"", -f);
            }
            Some(f) => {
                let _ = write!(x, " w:firstLine=\"{f}\"");
            }
            None => {}
        }
        x.push_str("/>");
    }
    toggle(&mut x, "contextualSpacing", p.contextual);
    if let Some(a) = p.align {
        let _ = write!(x, "<w:jc w:val=\"{a}\"/>");
    }
    if let Some(o) = e.outline {
        let _ = write!(x, "<w:outlineLvl w:val=\"{o}\"/>");
    }
    if !e.rpr.is_empty() {
        let _ = write!(x, "<w:rPr>{}</w:rPr>", e.rpr);
    }
    x.push_str(&e.sect);
    if x.is_empty() {
        x
    } else {
        format!("<w:pPr>{x}</w:pPr>")
    }
}

// ── lists ──────────────────────────────────────────────────────────────────

/// A list level as Word's `w:lvl`.
#[derive(Clone, Debug)]
pub(crate) struct Level {
    pub fmt: &'static str,
    pub text: String,
    pub start: u32,
    /// `w:rPr` children of the number or bullet.
    pub rpr: String,
    pub left: i32,
    pub hanging: i32,
    pub tab: Option<i32>,
    pub suff: &'static str,
    pub jc: &'static str,
}

impl Level {
    /// Whether the level shows a label at all.
    pub fn shows(&self) -> bool {
        self.fmt != "none" || !self.text.is_empty()
    }

    pub fn xml(&self, ilvl: usize) -> String {
        let mut x = format!(
            "<w:lvl w:ilvl=\"{ilvl}\"><w:start w:val=\"{}\"/><w:numFmt w:val=\"{}\"/>",
            self.start, self.fmt
        );
        if self.suff != "tab" {
            let _ = write!(x, "<w:suff w:val=\"{}\"/>", self.suff);
        }
        let _ = write!(
            x,
            "<w:lvlText w:val=\"{}\"/><w:lvlJc w:val=\"{}\"/><w:pPr>",
            esc(&self.text),
            self.jc
        );
        if let Some(t) = self.tab.filter(|_| self.suff == "tab") {
            let _ = write!(x, "<w:tabs><w:tab w:val=\"num\" w:pos=\"{t}\"/></w:tabs>");
        }
        let _ = write!(
            x,
            "<w:ind w:left=\"{}\" w:hanging=\"{}\"/></w:pPr>",
            self.left, self.hanging
        );
        if !self.rpr.is_empty() {
            let _ = write!(x, "<w:rPr>{}</w:rPr>", self.rpr);
        }
        x.push_str("</w:lvl>");
        x
    }
}

fn num_format(f: &str) -> &'static str {
    match f {
        "1" => "decimal",
        "a" => "lowerLetter",
        "A" => "upperLetter",
        "i" => "lowerRoman",
        "I" => "upperRoman",
        "" => "none",
        "１, ２, ３, ..." | "１" => "decimalFullWidth",
        "①, ②, ③, ..." | "①" => "decimalEnclosedCircle",
        "一, 二, 三, ..." => "chineseCounting",
        _ => "decimal",
    }
}

/// Bullets drawn with OpenSymbol's private characters, as Unicode.
fn bullet_char(c: &str) -> String {
    match c {
        "\u{E00A}" | "\u{E00C}" => "■".into(),
        "\u{E011}" => "◆".into(),
        "\u{E012}" => "➢".into(),
        "\u{E013}" => "✔".into(),
        "" => "•".into(),
        c => c
            .chars()
            .next()
            .map(String::from)
            .unwrap_or_else(|| "•".into()),
    }
}

/// The 9 levels of a list style (or the outline style), as Word levels.
pub(crate) fn levels(st: &Styles, scope: Scope, list: Node) -> Vec<Level> {
    let mut out: Vec<Option<Level>> = vec![None; 9];
    for l in list.children().filter(|n| n.is_element()) {
        let name = l.tag_name().name();
        if !matches!(
            name,
            "list-level-style-number"
                | "list-level-style-bullet"
                | "list-level-style-image"
                | "outline-level-style"
        ) {
            continue;
        }
        let Some(lvl) = attr(l, NS_TEXT, "level")
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| (1..=9).contains(v))
        else {
            continue;
        };
        let i = lvl - 1;
        // The number's or bullet's character style.
        let mut tp = attr(l, NS_TEXT, "style-name")
            .and_then(|s| st.get(scope, "text", s))
            .map(|s| TextProps::of_style(st, Some(s)))
            .unwrap_or_default();
        if let Some(own) = child(l, NS_STYLE, "text-properties") {
            tp = tp.over(&TextProps::read(st, own));
        }
        let (fmt, text, start) =
            if name == "list-level-style-number" || name == "outline-level-style" {
                let fmt = num_format(attr(l, NS_STYLE, "num-format").unwrap_or(""));
                let shown: usize = attr(l, NS_TEXT, "display-levels")
                    .and_then(|d| d.parse().ok())
                    .unwrap_or(1)
                    .clamp(1, lvl);
                let mut t = String::new();
                t.push_str(attr(l, NS_STYLE, "num-prefix").unwrap_or(""));
                if fmt != "none" {
                    for k in (lvl + 1 - shown)..=lvl {
                        if k > lvl + 1 - shown {
                            t.push('.');
                        }
                        let _ = write!(t, "%{k}");
                    }
                }
                t.push_str(attr(l, NS_STYLE, "num-suffix").unwrap_or(""));
                let start = attr(l, NS_TEXT, "start-value")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1);
                (fmt, t, start)
            } else {
                let c = if name == "list-level-style-bullet" {
                    bullet_char(attr(l, NS_TEXT, "bullet-char").unwrap_or("•"))
                } else {
                    "•".into()
                };
                // OpenSymbol isn't on most systems; the bullet's Unicode
                // character draws in any font.
                if tp
                    .font
                    .as_deref()
                    .is_some_and(|f| f.eq_ignore_ascii_case("OpenSymbol") || f == "StarSymbol")
                {
                    tp.font = None;
                }
                ("bullet", c, 1)
            };
        // Percent sizes on bullets are relative to the text: leave them.
        if matches!(tp.size, Some(Size::Pct(_))) {
            tp.size = None;
        }
        let mut lv = Level {
            fmt,
            text,
            start,
            rpr: tp.xml(None, 12.0),
            left: 0,
            hanging: 0,
            tab: None,
            suff: "tab",
            jc: "left",
        };
        if let Some(p) = child(l, NS_STYLE, "list-level-properties") {
            lv.jc = match attr(p, NS_FO, "text-align") {
                Some("center") => "center",
                Some("end") | Some("right") => "right",
                _ => "left",
            };
            if attr(p, NS_TEXT, "list-level-position-and-space-mode") == Some("label-alignment") {
                if let Some(la) = child(p, NS_STYLE, "list-level-label-alignment") {
                    let left = attr(la, NS_FO, "margin-left").and_then(twips).unwrap_or(0);
                    let ind = attr(la, NS_FO, "text-indent").and_then(twips).unwrap_or(0);
                    lv.left = left;
                    lv.hanging = -ind;
                    lv.suff = match attr(la, NS_TEXT, "label-followed-by") {
                        Some("space") => "space",
                        Some("nothing") => "nothing",
                        _ => "tab",
                    };
                    lv.tab = attr(la, NS_TEXT, "list-tab-stop-position").and_then(twips);
                }
            } else {
                let before = attr(p, NS_TEXT, "space-before")
                    .and_then(twips)
                    .unwrap_or(0);
                let width = attr(p, NS_TEXT, "min-label-width")
                    .and_then(twips)
                    .unwrap_or(0);
                lv.left = before + width;
                lv.hanging = width;
                lv.tab = Some(before + width);
            }
        }
        out[i] = Some(lv);
    }
    out.into_iter()
        .enumerate()
        .map(|(i, l)| {
            l.unwrap_or(Level {
                fmt: "none",
                text: String::new(),
                start: 1,
                rpr: String::new(),
                left: 720 * (i as i32 + 1),
                hanging: 360,
                tab: None,
                suff: "tab",
                jc: "left",
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn names_and_borders() {
        assert_eq!(decode_name("Heading_20_1"), "Heading 1");
        assert_eq!(decode_name("a_b"), "a_b");
        assert_eq!(decode_name("x_"), "x_");
        assert_eq!(
            word_style("paragraph", "Standard", "Default Paragraph Style").0,
            "Normal"
        );
        assert_eq!(
            word_style("paragraph", "Heading_20_2", "Heading 2").0,
            "Heading2"
        );
        assert_eq!(
            word_style("text", "Internet_20_link", "Internet link").0,
            "Hyperlink"
        );
        assert_eq!(word_style("paragraph", "x", "2col").0, "Style2col");
        assert_eq!(
            border_xml("top", "0.5pt solid #000000", None).as_deref(),
            Some("<w:top w:val=\"single\" w:sz=\"4\" w:color=\"000000\"/>")
        );
        assert_eq!(
            border_xml("top", "none", None).as_deref(),
            Some("<w:top w:val=\"nil\"/>")
        );
    }
}
