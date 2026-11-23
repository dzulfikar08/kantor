//! Text of OpenDocument shapes (`text:p`, `text:h`, `text:list` with
//! spans, links, fields and breaks) as DrawingML paragraphs, every
//! property resolved through the paragraph's, the outline level's and the
//! shape's styles and written out, so the text looks the same without the
//! master's help.

use roxmltree::Node;

use super::shape::Part;
use super::style::{
    NS_FO, NS_LOEXT, NS_PRES, NS_STYLE, NS_TEXT, NS_XLINK, Ref, Styles, color, emu, esc, percent,
    points,
};

/// How a text body is read.
pub(super) struct TextOpts<'c, 'a> {
    /// The shape's styles: its `draw:text-style-name` (paragraph) and its
    /// graphic or presentation style.
    pub base: &'c [Ref<'a>],
    /// An outline placeholder: levels take `<master>-outlineN`.
    pub outline: bool,
    /// Speaker notes: sizes and fonts come from the notes master.
    pub notes: bool,
}

/// Where a paragraph sits in a list.
#[derive(Clone, Copy)]
struct ListAt<'a> {
    /// The list style named by the list (or an enclosing one).
    style: Option<&'a str>,
    /// A `text:list-header` (no bullet).
    header: bool,
    start: Option<i64>,
}

/// Resolved character properties.
#[derive(Default)]
pub(super) struct RunProps {
    pub size: Option<f64>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<&'static str>,
    pub strike: Option<&'static str>,
    pub baseline: Option<i64>,
    pub cap: Option<&'static str>,
    /// Hundredths of a point.
    pub spacing: Option<i64>,
    pub color: Option<String>,
    pub alpha: Option<i64>,
    pub highlight: Option<String>,
    pub latin: Option<String>,
    pub ea: Option<String>,
    pub cs: Option<String>,
    pub lang: Option<String>,
}

impl RunProps {
    /// `<a:{tag} …>…</a:{tag}>` (`rPr`, `endParaRPr` or `defRPr`).
    pub fn xml(&self, tag: &str, link: Option<&str>) -> String {
        let mut a = String::new();
        if let Some(l) = &self.lang {
            a.push_str(&format!(" lang=\"{}\"", esc(l)));
        }
        if let Some(sz) = self.size {
            a.push_str(&format!(
                " sz=\"{}\"",
                ((sz * 100.0).round() as i64).clamp(100, 400_000)
            ));
        }
        if let Some(b) = self.bold {
            a.push_str(if b { " b=\"1\"" } else { " b=\"0\"" });
        }
        if let Some(i) = self.italic {
            a.push_str(if i { " i=\"1\"" } else { " i=\"0\"" });
        }
        if let Some(u) = self.underline {
            a.push_str(&format!(" u=\"{u}\""));
        }
        if let Some(s) = self.strike {
            a.push_str(&format!(" strike=\"{s}\""));
        }
        if let Some(k) = self.spacing {
            a.push_str(&format!(" spc=\"{}\"", k.clamp(-400_000, 400_000)));
        }
        if let Some(c) = self.cap {
            a.push_str(&format!(" cap=\"{c}\""));
        }
        if let Some(b) = self.baseline {
            a.push_str(&format!(" baseline=\"{b}\""));
        }
        let mut kids = String::new();
        if let Some(c) = &self.color {
            match self.alpha {
                Some(al) if al < 100_000 => kids.push_str(&format!(
                    "<a:solidFill><a:srgbClr val=\"{c}\"><a:alpha val=\"{al}\"/></a:srgbClr></a:solidFill>"
                )),
                _ => kids.push_str(&format!(
                    "<a:solidFill><a:srgbClr val=\"{c}\"/></a:solidFill>"
                )),
            }
        }
        if let Some(h) = &self.highlight {
            kids.push_str(&format!(
                "<a:highlight><a:srgbClr val=\"{h}\"/></a:highlight>"
            ));
        }
        for (t, f) in [("latin", &self.latin), ("ea", &self.ea), ("cs", &self.cs)] {
            if let Some(f) = f {
                kids.push_str(&format!("<a:{t} typeface=\"{}\"/>", esc(f)));
            }
        }
        if let Some(id) = link {
            kids.push_str(&format!("<a:hlinkClick r:id=\"{id}\"/>"));
        }
        if kids.is_empty() {
            format!("<a:{tag}{a}/>")
        } else {
            format!("<a:{tag}{a}>{kids}</a:{tag}>")
        }
    }
}

/// Paragraph properties.
#[derive(Default)]
pub(super) struct ParaProps {
    pub align: Option<&'static str>,
    pub mar_l: Option<i64>,
    pub indent: Option<i64>,
    pub line: Option<String>,
    pub before: Option<i64>,
    pub after: Option<i64>,
    pub bullet: String,
    pub rtl: bool,
}

impl ParaProps {
    /// `<a:{tag} …>…</a:{tag}>` (`pPr` or `lvlNpPr`), `inner` after the
    /// bullet (a `defRPr`).
    pub fn xml(&self, tag: &str, lvl: usize, inner: &str) -> String {
        let mut a = String::new();
        if let Some(m) = self.mar_l {
            a.push_str(&format!(" marL=\"{}\"", m.clamp(0, 51_206_400)));
        }
        if lvl > 0 {
            a.push_str(&format!(" lvl=\"{}\"", lvl.min(8)));
        }
        if let Some(i) = self.indent {
            a.push_str(&format!(" indent=\"{}\"", i.clamp(-51_206_400, 51_206_400)));
        }
        if let Some(al) = self.align {
            a.push_str(&format!(" algn=\"{al}\""));
        }
        if self.rtl {
            a.push_str(" rtl=\"1\"");
        }
        let mut kids = String::new();
        if let Some(l) = &self.line {
            kids.push_str(&format!("<a:lnSpc>{l}</a:lnSpc>"));
        }
        if let Some(b) = self.before {
            kids.push_str(&format!(
                "<a:spcBef><a:spcPts val=\"{}\"/></a:spcBef>",
                b.clamp(0, 158_400)
            ));
        }
        if let Some(b) = self.after {
            kids.push_str(&format!(
                "<a:spcAft><a:spcPts val=\"{}\"/></a:spcAft>",
                b.clamp(0, 158_400)
            ));
        }
        kids.push_str(&self.bullet);
        kids.push_str(inner);
        if kids.is_empty() {
            format!("<a:{tag}{a}/>")
        } else {
            format!("<a:{tag}{a}>{kids}</a:{tag}>")
        }
    }
}

/// Character properties of `chain`.
pub(super) fn run_props(st: &Styles, chain: &[Ref<'_>]) -> RunProps {
    let t = |ns: &str, attr: &str| st.prop(chain, "text-properties", ns, attr);
    let mut r = RunProps::default();
    // Sizes: a percentage scales the size found further up.
    let mut factor = 1.0;
    let mut size = None;
    for n in st.nodes(chain) {
        let Some(v) = n
            .children()
            .filter(|c| c.tag_name().name() == "text-properties")
            .find_map(|p| p.attribute((NS_FO, "font-size")))
        else {
            continue;
        };
        if let Some(p) = percent(v) {
            factor *= p;
        } else if let Some(pt) = points(v) {
            size = Some(pt * factor);
            break;
        }
    }
    r.size = size.or((factor != 1.0).then_some(18.0 * factor));
    r.bold =
        t(NS_FO, "font-weight").map(|w| w == "bold" || w.parse::<u32>().is_ok_and(|n| n >= 600));
    r.italic = t(NS_FO, "font-style").map(|s| s == "italic" || s == "oblique");
    r.underline = t(NS_STYLE, "text-underline-style").map(|u| {
        let double = t(NS_STYLE, "text-underline-type") == Some("double");
        match u {
            "none" => "none",
            _ if double => "dbl",
            "dotted" => "dotted",
            "dash" => "dash",
            "long-dash" => "dashLong",
            "dot-dash" => "dotDash",
            "dot-dot-dash" => "dotDotDash",
            "wave" => "wavy",
            _ => "sng",
        }
    });
    r.strike = t(NS_STYLE, "text-line-through-style").map(|s| match s {
        "none" => "noStrike",
        _ if t(NS_STYLE, "text-line-through-type") == Some("double") => "dblStrike",
        _ => "sngStrike",
    });
    r.baseline = t(NS_STYLE, "text-position").and_then(|p| {
        let first = p.split_whitespace().next()?;
        Some(match first {
            "super" => 30_000,
            "sub" => -25_000,
            v => (percent(v)? * 100_000.0).round() as i64,
        })
    });
    r.cap = match (t(NS_FO, "text-transform"), t(NS_FO, "font-variant")) {
        (Some("uppercase"), _) => Some("all"),
        (_, Some("small-caps")) => Some("small"),
        (Some("none"), _) | (_, Some("normal")) => Some("none"),
        _ => None,
    };
    r.spacing = t(NS_FO, "letter-spacing")
        .filter(|v| *v != "normal")
        .and_then(points)
        .map(|p| (p * 100.0).round() as i64);
    // Colour: the first style that says, "automatic" included.
    r.color = st
        .find(chain, "text-properties", |p| {
            if p.attribute((NS_STYLE, "use-window-font-color")) == Some("true") {
                return Some(None);
            }
            p.attribute((NS_FO, "color")).map(color)
        })
        .flatten();
    r.alpha = t(NS_LOEXT, "opacity")
        .and_then(percent)
        .map(|o| (o * 100_000.0).round() as i64)
        .filter(|a| (1..100_000).contains(a));
    r.highlight = t(NS_FO, "background-color")
        .filter(|c| *c != "transparent")
        .and_then(color);
    let font = |name: &str, family: &str| {
        st.find(chain, "text-properties", |p| {
            p.attribute((NS_STYLE, name))
                .map(|f| st.font(f))
                .or_else(|| {
                    p.attribute((NS_FO, family))
                        .or(p.attribute((NS_STYLE, family)))
                        .map(|f| st.font(f))
                })
        })
        .filter(|f| !f.is_empty())
    };
    r.latin = font("font-name", "font-family");
    r.ea = font("font-name-asian", "font-family-asian");
    r.cs = font("font-name-complex", "font-family-complex");
    r.lang = match (t(NS_FO, "language"), t(NS_FO, "country")) {
        (Some(l), Some(c)) if l != "zxx" && c != "none" => Some(format!("{l}-{c}")),
        (Some(l), _) if l != "zxx" && l != "none" => Some(l.to_owned()),
        _ => None,
    };
    r
}

/// The list level element (`text:list-level-style-*`) of level `lvl`
/// (1-based), or the deepest one above it.
pub(super) fn list_level<'a, 'i>(list: Node<'a, 'i>, lvl: usize) -> Option<Node<'a, 'i>> {
    let mut best: Option<(usize, Node)> = None;
    for c in list.children().filter(|c| c.is_element()) {
        let l: usize = c
            .attribute((NS_TEXT, "level"))
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        if l == lvl {
            return Some(c);
        }
        if l < lvl && best.is_none_or(|(b, _)| l > b) {
            best = Some((l, c));
        }
    }
    best.map(|(_, c)| c)
}

/// Paragraph properties of `chain` at level `lvl` (0-based), with the
/// bullet of list level element `level` (`None`: no bullet), the item
/// starting at `start`.
pub(super) fn para_props(
    st: &Styles,
    chain: &[Ref<'_>],
    level: Option<Node>,
    bulleted: bool,
    start: Option<i64>,
) -> ParaProps {
    let p = |ns: &str, attr: &str| st.prop(chain, "paragraph-properties", ns, attr);
    let mut out = ParaProps {
        align: p(NS_FO, "text-align").map(|a| match a {
            "center" => "ctr",
            "end" | "right" => "r",
            "justify" => "just",
            _ => "l",
        }),
        ..ParaProps::default()
    };
    out.rtl = p(NS_STYLE, "writing-mode").is_some_and(|w| w.starts_with("rl"));
    let ml = p(NS_FO, "margin-left").and_then(emu);
    let ti = p(NS_FO, "text-indent").and_then(emu);
    out.before = p(NS_FO, "margin-top")
        .and_then(points)
        .map(|v| (v * 100.0).round() as i64);
    out.after = p(NS_FO, "margin-bottom")
        .and_then(points)
        .map(|v| (v * 100.0).round() as i64);
    out.line = p(NS_FO, "line-height")
        .and_then(|l| {
            if let Some(pc) = percent(l) {
                Some(format!(
                    "<a:spcPct val=\"{}\"/>",
                    ((pc * 100_000.0).round() as i64).clamp(0, 13_200_000)
                ))
            } else {
                points(l).map(|v| {
                    format!(
                        "<a:spcPts val=\"{}\"/>",
                        ((v * 100.0).round() as i64).clamp(0, 158_400)
                    )
                })
            }
        })
        .or_else(|| {
            p(NS_STYLE, "line-height-at-least")
                .and_then(points)
                .map(|v| {
                    format!(
                        "<a:spcPts val=\"{}\"/>",
                        ((v * 100.0).round() as i64).clamp(0, 158_400)
                    )
                })
        });
    // Indents: the paragraph's own, plus the list level's.
    let (mut mar_l, mut indent) = (ml, ti);
    if let Some(lv) = level {
        let props = lv
            .children()
            .find(|c| c.tag_name().name() == "list-level-properties");
        let label = props.and_then(|p| {
            p.children()
                .find(|c| c.tag_name().name() == "list-level-label-alignment")
        });
        if let Some(la) = label.filter(|_| {
            props.and_then(|p| p.attribute((NS_TEXT, "list-level-position-and-space-mode")))
                == Some("label-alignment")
        }) {
            let lm = la.attribute((NS_FO, "margin-left")).and_then(emu);
            let li = la.attribute((NS_FO, "text-indent")).and_then(emu);
            if ml.unwrap_or(0) == 0 {
                mar_l = lm.or(ml);
                indent = if bulleted { li.or(ti) } else { ti };
            }
        } else {
            let sb = props
                .and_then(|p| p.attribute((NS_TEXT, "space-before")))
                .and_then(emu)
                .unwrap_or(0);
            let mlw = props
                .and_then(|p| p.attribute((NS_TEXT, "min-label-width")))
                .and_then(emu)
                .unwrap_or(0);
            mar_l = Some(ml.unwrap_or(0) + sb + mlw);
            indent = Some(ti.unwrap_or(0) - if bulleted { mlw } else { 0 });
        }
    }
    out.mar_l = mar_l;
    out.indent = indent;
    out.bullet = match level.filter(|_| bulleted) {
        Some(lv) => bullet(st, lv, start),
        None => "<a:buNone/>".into(),
    };
    out
}

/// The bullet elements of a list level.
fn bullet(st: &Styles, lv: Node, start: Option<i64>) -> String {
    let tp = lv
        .children()
        .find(|c| c.tag_name().name() == "text-properties");
    let tpa = |ns: &str, a: &str| tp.and_then(|t| t.attribute((ns, a)));
    let mut s = String::new();
    if tpa(NS_STYLE, "use-window-font-color") != Some("true")
        && let Some(c) = tpa(NS_FO, "color").and_then(color)
    {
        s.push_str(&format!("<a:buClr><a:srgbClr val=\"{c}\"/></a:buClr>"));
    }
    if let Some(p) = tpa(NS_FO, "font-size")
        .or(lv.attribute((NS_TEXT, "bullet-relative-size")))
        .and_then(percent)
    {
        s.push_str(&format!(
            "<a:buSzPct val=\"{}\"/>",
            ((p * 100_000.0).round() as i64).clamp(25_000, 400_000)
        ));
    }
    let font = tpa(NS_STYLE, "font-name")
        .or(tpa(NS_FO, "font-family"))
        .map(|f| st.font(f))
        .filter(|f| !f.is_empty());
    match lv.tag_name().name() {
        "list-level-style-number" => {
            let fmt = lv.attribute((NS_STYLE, "num-format")).unwrap_or("1");
            let kind = match fmt {
                "" => return "<a:buNone/>".into(),
                "a" => "alphaLc",
                "A" => "alphaUc",
                "i" => "romanLc",
                "I" => "romanUc",
                _ => "arabic",
            };
            let pre = lv.attribute((NS_STYLE, "num-prefix")).unwrap_or("");
            let suf = lv.attribute((NS_STYLE, "num-suffix")).unwrap_or("");
            let form = match (pre.trim(), suf.trim()) {
                ("(", ")") => "ParenBoth",
                (_, ")") => "ParenR",
                (_, "") if kind == "arabic" => "Plain",
                _ => "Period",
            };
            if let Some(f) = font {
                s.push_str(&format!("<a:buFont typeface=\"{}\"/>", esc(&f)));
            }
            let start = start
                .or(lv
                    .attribute((NS_TEXT, "start-value"))
                    .and_then(|v| v.parse().ok()))
                .unwrap_or(1)
                .clamp(1, 32_767);
            let at = if start == 1 {
                String::new()
            } else {
                format!(" startAt=\"{start}\"")
            };
            s.push_str(&format!("<a:buAutoNum type=\"{kind}{form}\"{at}/>"));
        }
        "list-level-style-image" => s.push_str("<a:buChar char=\"\u{2022}\"/>"),
        _ => {
            let ch = lv
                .attribute((NS_TEXT, "bullet-char"))
                .and_then(|c| c.chars().next());
            let Some(ch) = ch.filter(|c| !c.is_whitespace()) else {
                return "<a:buNone/>".into();
            };
            // OpenSymbol's private-use bullets: their Unicode look-alikes.
            let (ch, font) = match ch {
                '\u{E00C}' => ('\u{25C6}', None),
                '\u{E00A}' => ('\u{25A0}', None),
                '\u{E006}' => ('\u{2794}', None),
                '\u{E011}' | '\u{E012}' | '\u{E013}' => ('\u{27A2}', None),
                c => (c, font.filter(|f| f != "OpenSymbol" && f != "StarSymbol")),
            };
            if let Some(f) = font {
                s.push_str(&format!("<a:buFont typeface=\"{}\"/>", esc(&f)));
            }
            s.push_str(&format!("<a:buChar char=\"{}\"/>", esc(&ch.to_string())));
        }
    }
    s
}

/// ODF text content: runs of white space are one space.
fn collapse(s: &str, prev_space: &mut bool) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, ' ' | '\n' | '\r' | '\t') {
            if !*prev_space {
                o.push(' ');
            }
            *prev_space = true;
        } else {
            o.push(c);
            *prev_space = false;
        }
    }
    o
}

impl<'a, 'i> Part<'_, 'a, 'i, '_> {
    /// The paragraphs of a text container (`draw:text-box`, a shape, a
    /// table cell), as `a:p` elements; empty when it has none with text.
    pub fn paragraphs(&mut self, n: Node<'a, 'i>, o: &TextOpts<'_, 'a>) -> Vec<String> {
        let mut out = Vec::new();
        let mut any = false;
        self.walk(n, 0, None, o, &mut out, &mut any);
        if any { out } else { Vec::new() }
    }

    fn walk(
        &mut self,
        n: Node<'a, 'i>,
        depth: usize,
        list: Option<ListAt<'a>>,
        o: &TextOpts<'_, 'a>,
        out: &mut Vec<String>,
        any: &mut bool,
    ) {
        for c in n.children().filter(|c| c.is_element()) {
            let tag = c.tag_name();
            if tag.namespace() != Some(NS_TEXT) {
                continue;
            }
            match tag.name() {
                "p" | "h" => {
                    let (xml, text) = self.paragraph(c, depth.saturating_sub(1), list, o);
                    *any |= text;
                    out.push(xml);
                }
                "list" => {
                    let style = c
                        .attribute((NS_TEXT, "style-name"))
                        .or(list.and_then(|l| l.style));
                    for item in c.children().filter(|i| i.is_element()) {
                        let header = item.has_tag_name((NS_TEXT, "list-header"));
                        if !header && !item.has_tag_name((NS_TEXT, "list-item")) {
                            continue;
                        }
                        let at = ListAt {
                            style,
                            header,
                            start: item
                                .attribute((NS_TEXT, "start-value"))
                                .and_then(|v| v.parse().ok()),
                        };
                        self.walk(item, (depth + 1).min(9), Some(at), o, out, any);
                    }
                }
                "section" | "soft-page-break" => {
                    self.walk(c, depth, list, o, out, any);
                }
                _ => {}
            }
        }
    }

    /// One paragraph at level `lvl` (0-based); whether it has text.
    fn paragraph(
        &mut self,
        p: Node<'a, 'i>,
        lvl: usize,
        list: Option<ListAt<'a>>,
        o: &TextOpts<'_, 'a>,
    ) -> (String, bool) {
        let st = self.st;
        let level_style = o
            .outline
            .then(|| format!("{}-outline{}", self.master, lvl + 1))
            .filter(|n| st.has("presentation", n));
        let mut chain: Vec<Ref> = Vec::new();
        if let Some(s) = p.attribute((NS_TEXT, "style-name")) {
            chain.push(("paragraph", s));
        }
        if let Some(l) = &level_style {
            chain.push(("presentation", l.as_str()));
        }
        chain.extend_from_slice(o.base);
        let level = list.and_then(|l| {
            let ls = l
                .style
                .and_then(|n| st.lists.get(n).copied())
                .or_else(|| st.list_of(&chain))?;
            list_level(ls, lvl + 1)
        });
        let bulleted = list.is_some_and(|l| !l.header)
            && st.prop(&chain, "paragraph-properties", NS_TEXT, "enable-numbering")
                != Some("false");
        let pp = para_props(st, &chain, level, bulleted, list.and_then(|l| l.start));
        let mut body = String::new();
        let mut spans = Vec::new();
        let mut text = false;
        let mut prev_space = true;
        self.runs(
            p,
            &mut spans,
            None,
            &chain,
            o,
            &mut body,
            &mut text,
            &mut prev_space,
        );
        let mut end = run_props(st, &chain);
        if o.notes {
            end.size = None;
            end.latin = None;
            end.ea = None;
            end.cs = None;
        }
        end.lang.get_or_insert_with(|| "en-US".into());
        let ppr = pp.xml("pPr", lvl, "");
        (
            format!("<a:p>{ppr}{body}{}</a:p>", end.xml("endParaRPr", None)),
            text,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn runs(
        &mut self,
        n: Node<'a, 'i>,
        spans: &mut Vec<&'a str>,
        link: Option<&str>,
        pchain: &[Ref<'_>],
        o: &TextOpts<'_, 'a>,
        out: &mut String,
        text: &mut bool,
        prev_space: &mut bool,
    ) {
        for c in n.children() {
            if c.is_text() {
                let s = collapse(c.text().unwrap_or_default(), prev_space);
                if !s.is_empty() {
                    *text |= !s.trim().is_empty();
                    self.run(&s, spans, link, pchain, o, out);
                }
                continue;
            }
            if !c.is_element() {
                continue;
            }
            let tag = c.tag_name();
            match (tag.namespace(), tag.name()) {
                (Some(NS_TEXT), "span") => {
                    let s = c.attribute((NS_TEXT, "style-name"));
                    if let Some(s) = s {
                        spans.push(s);
                    }
                    self.runs(c, spans, link, pchain, o, out, text, prev_space);
                    if s.is_some() {
                        spans.pop();
                    }
                }
                (Some(NS_TEXT), "a") => {
                    let href = c.attribute((NS_XLINK, "href"));
                    let id = href
                        .filter(|h| !o.notes && !h.starts_with('#') && !h.is_empty())
                        .map(|h| self.rel("hyperlink", h.to_owned(), true));
                    let s = c.attribute((NS_TEXT, "style-name"));
                    if let Some(s) = s {
                        spans.push(s);
                    }
                    self.runs(
                        c,
                        spans,
                        id.as_deref().or(link),
                        pchain,
                        o,
                        out,
                        text,
                        prev_space,
                    );
                    if s.is_some() {
                        spans.pop();
                    }
                }
                (Some(NS_TEXT), "s") => {
                    let k: usize = c
                        .attribute((NS_TEXT, "c"))
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(1);
                    *prev_space = true;
                    self.run(&" ".repeat(k.clamp(1, 200)), spans, link, pchain, o, out);
                }
                (Some(NS_TEXT), "tab") => {
                    *prev_space = true;
                    self.run("\t", spans, link, pchain, o, out);
                }
                (Some(NS_TEXT), "line-break") => {
                    *prev_space = true;
                    let rp = self.rpr(spans, pchain, o);
                    out.push_str(&format!("<a:br>{}</a:br>", rp.xml("rPr", None)));
                }
                (Some(NS_TEXT), "page-number") => {
                    *text = true;
                    *prev_space = false;
                    let rp = self.rpr(spans, pchain, o);
                    self.fld += 1;
                    out.push_str(&format!(
                        "<a:fld id=\"{{B6F15528-21DE-4FAA-801E-{:012X}}}\" type=\"slidenum\">{}<a:t>{}</a:t></a:fld>",
                        self.fld,
                        rp.xml("rPr", None),
                        if self.slide_no == 0 {
                            "‹#›".to_owned()
                        } else {
                            self.slide_no.to_string()
                        }
                    ));
                }
                (
                    Some(NS_TEXT),
                    "soft-page-break"
                    | "bookmark"
                    | "bookmark-start"
                    | "bookmark-end"
                    | "note"
                    | "reference-mark"
                    | "reference-mark-start"
                    | "reference-mark-end"
                    | "change"
                    | "change-start"
                    | "change-end",
                ) => {}
                (Some(NS_PRES), kind) => {
                    // The slide's footer and date (from their declarations).
                    let decl = match kind {
                        "footer" => self.footer.clone().map(Ok),
                        "date-time" => self.date.clone(),
                        _ => None,
                    };
                    let rp = self.rpr(spans, pchain, o);
                    match decl {
                        Some(Ok(t)) if !t.is_empty() => {
                            *text = true;
                            *prev_space = false;
                            out.push_str(&format!(
                                "<a:r>{}<a:t>{}</a:t></a:r>",
                                rp.xml("rPr", link),
                                esc(&t)
                            ));
                        }
                        Some(Err(())) => {
                            *text = true;
                            self.fld += 1;
                            out.push_str(&format!(
                                "<a:fld id=\"{{B6F15528-21DE-4FAA-801E-{:012X}}}\" type=\"datetime1\">{}<a:t></a:t></a:fld>",
                                self.fld,
                                rp.xml("rPr", None)
                            ));
                        }
                        _ => {}
                    }
                }
                (Some(NS_TEXT), _) => {
                    // Other fields (dates, titles…): their shown text.
                    self.runs(c, spans, link, pchain, o, out, text, prev_space);
                }
                _ => {}
            }
        }
    }

    fn rpr(&self, spans: &[&str], pchain: &[Ref<'_>], o: &TextOpts) -> RunProps {
        let mut chain: Vec<Ref> = spans.iter().rev().map(|s| ("text", *s)).collect();
        chain.extend_from_slice(pchain);
        let mut r = run_props(self.st, &chain);
        if o.notes {
            r.size = None;
            r.latin = None;
            r.ea = None;
            r.cs = None;
        }
        r.lang.get_or_insert_with(|| "en-US".into());
        r
    }

    fn run(
        &mut self,
        s: &str,
        spans: &[&'a str],
        link: Option<&str>,
        pchain: &[Ref<'_>],
        o: &TextOpts,
        out: &mut String,
    ) {
        let r = self.rpr(spans, pchain, o);
        out.push_str(&format!(
            "<a:r>{}<a:t>{}</a:t></a:r>",
            r.xml("rPr", link),
            esc(s)
        ));
    }
}

/// The text body properties (`a:bodyPr`) of a shape's style chain.
pub(super) fn body_pr(st: &Styles, chain: &[Ref<'_>], vertical_default: bool) -> String {
    let g = |ns: &str, a: &str| st.g(chain, ns, a);
    let mut a = String::new();
    let wrap = g(NS_FO, "wrap-option") == Some("no-wrap");
    a.push_str(if wrap {
        " wrap=\"none\""
    } else {
        " wrap=\"square\""
    });
    for (attr, side) in [
        ("lIns", "padding-left"),
        ("tIns", "padding-top"),
        ("rIns", "padding-right"),
        ("bIns", "padding-bottom"),
    ] {
        if let Some(v) = g(NS_FO, side).or(g(NS_FO, "padding")).and_then(emu) {
            a.push_str(&format!(" {attr}=\"{}\"", v.clamp(0, 10_000_000)));
        }
    }
    let anchor = match g(super::style::NS_DRAW, "textarea-vertical-align") {
        Some("middle") => Some("ctr"),
        Some("bottom") => Some("b"),
        Some("top" | "justify") => Some("t"),
        _ => None,
    };
    if let Some(an) = anchor {
        a.push_str(&format!(" anchor=\"{an}\""));
    }
    let mode = st
        .prop(chain, "paragraph-properties", NS_STYLE, "writing-mode")
        .or(g(NS_STYLE, "writing-mode"));
    if mode.is_some_and(|m| m.starts_with("tb")) || (vertical_default && mode.is_none()) {
        a.push_str(" vert=\"eaVert\"");
    }
    a.push_str(" rtlCol=\"0\"");
    let fit = if g(super::style::NS_DRAW, "auto-grow-height") == Some("true") {
        "<a:spAutoFit/>"
    } else if g(NS_STYLE, "shrink-to-fit") == Some("true")
        || matches!(
            g(super::style::NS_DRAW, "fit-to-size"),
            Some("shrink-to-fit" | "true")
        )
    {
        "<a:normAutofit/>"
    } else {
        "<a:noAutofit/>"
    };
    format!("<a:bodyPr{a}>{fit}</a:bodyPr>")
}
