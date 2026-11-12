//! Small XML helpers: a tree for property snippets, escaping, and the
//! child order WordprocessingML's schema requires.

use quick_xml::events::Event;

/// An element with local names (prefixes dropped).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct El {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<El>,
}

impl El {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn num<T: std::str::FromStr>(&self, name: &str) -> Option<T> {
        self.attr(name).and_then(|v| v.parse().ok())
    }

    pub fn child(&self, name: &str) -> Option<&El> {
        self.children.iter().find(|c| c.name == name)
    }

    /// Whether every attribute's local name is in `allowed`.
    pub fn only_attrs(&self, allowed: &[&str]) -> bool {
        self.attrs
            .iter()
            .all(|(k, _)| allowed.contains(&k.as_str()))
    }

    /// `w:val` as an on/off switch (absent = on).
    pub fn on(&self) -> bool {
        !matches!(self.attr("val"), Some("0" | "false" | "off" | "none"))
    }
}

fn local(name: &[u8]) -> String {
    let s = String::from_utf8_lossy(name);
    match s.rsplit_once(':') {
        Some((_, l)) => l.to_owned(),
        None => s.into_owned(),
    }
}

/// Parse the first element of an XML fragment (prefixes need no
/// declarations: names are matched by local name).
pub fn parse(xml: &str) -> Option<El> {
    let mut r = quick_xml::Reader::from_str(xml);
    let mut stack: Vec<El> = Vec::new();
    loop {
        let ev = r.read_event().ok()?;
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let empty = matches!(ev, Event::Empty(_));
                let el = El {
                    name: local(e.name().as_ref()),
                    attrs: e
                        .attributes()
                        .flatten()
                        .filter(|a| !a.key.as_ref().starts_with(b"xmlns"))
                        .map(|a| {
                            let raw = String::from_utf8_lossy(&a.value).into_owned();
                            let v = quick_xml::escape::unescape(&raw)
                                .map(|v| v.into_owned())
                                .unwrap_or(raw);
                            (local(a.key.as_ref()), v)
                        })
                        .collect(),
                    children: Vec::new(),
                };
                if empty {
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(el),
                        None => return Some(el),
                    }
                } else {
                    stack.push(el);
                }
            }
            Event::End(_) => {
                let el = stack.pop()?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(el),
                    None => return Some(el),
                }
            }
            Event::Eof => return None,
            _ => {}
        }
    }
}

/// All text inside `w:t` / `w:instrText`-like elements of a fragment
/// (for placeholder labels).
pub fn inner_text(xml: &str, elements: &[&str]) -> String {
    let mut r = quick_xml::Reader::from_str(xml);
    let mut out = String::new();
    let mut depth_in = 0usize;
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => {
                if elements.contains(&local(e.name().as_ref()).as_str()) {
                    depth_in += 1;
                }
            }
            Ok(Event::End(e)) => {
                if elements.contains(&local(e.name().as_ref()).as_str()) {
                    depth_in = depth_in.saturating_sub(1);
                }
            }
            Ok(Event::Text(t)) if depth_in > 0 => {
                if let Ok(s) = t.xml10_content() {
                    out.push_str(&s);
                }
            }
            Ok(Event::GeneralRef(g)) if depth_in > 0 => {
                if let Ok(Some(c)) = g.resolve_char_ref() {
                    out.push(c);
                } else {
                    out.push_str(match &*g as &[u8] {
                        b"amp" => "&",
                        b"lt" => "<",
                        b"gt" => ">",
                        b"quot" => "\"",
                        b"apos" => "'",
                        _ => "",
                    });
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

/// Escape text content.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// `w:rPr` children in schema order.
pub const RPR_ORDER: &[&str] = &[
    // A paragraph mark's own revision marks come first (CT_ParaRPr).
    "ins",
    "del",
    "moveFrom",
    "moveTo",
    "rStyle",
    "rFonts",
    "b",
    "bCs",
    "i",
    "iCs",
    "caps",
    "smallCaps",
    "strike",
    "dstrike",
    "outline",
    "shadow",
    "emboss",
    "imprint",
    "noProof",
    "snapToGrid",
    "vanish",
    "webHidden",
    "color",
    "spacing",
    "w",
    "kern",
    "position",
    "sz",
    "szCs",
    "highlight",
    "u",
    "effect",
    "bdr",
    "shd",
    "fitText",
    "vertAlign",
    "rtl",
    "cs",
    "em",
    "lang",
    "eastAsianLayout",
    "specVanish",
    "oMath",
    "rPrChange",
];

/// `w:pPr` children in schema order.
pub const PPR_ORDER: &[&str] = &[
    "pStyle",
    "keepNext",
    "keepLines",
    "pageBreakBefore",
    "framePr",
    "widowControl",
    "numPr",
    "suppressLineNumbers",
    "pBdr",
    "shd",
    "tabs",
    "suppressAutoHyphens",
    "kinsoku",
    "wordWrap",
    "overflowPunct",
    "topLinePunct",
    "autoSpaceDE",
    "autoSpaceDN",
    "bidi",
    "adjustRightInd",
    "snapToGrid",
    "spacing",
    "ind",
    "contextualSpacing",
    "mirrorIndents",
    "suppressOverlap",
    "jc",
    "textDirection",
    "textAlignment",
    "textboxTightWrap",
    "outlineLvl",
    "divId",
    "cnfStyle",
    "rPr",
    "sectPr",
    "pPrChange",
];

/// Join property elements (XML fragments) in schema order; unknown names
/// go last in their original order.
pub fn ordered(order: &[&str], mut elements: Vec<String>) -> String {
    let rank = |x: &String| {
        let name = crate::model::element_name(x);
        order.iter().position(|n| *n == name).unwrap_or(order.len())
    };
    elements.sort_by_key(rank);
    elements.concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fragments_without_namespaces() {
        let el = parse(r#"<w:numPr><w:ilvl w:val="1"/><w:numId w:val="3"/></w:numPr>"#).unwrap();
        assert_eq!(el.name, "numPr");
        assert_eq!(el.child("numId").unwrap().num::<u32>("val"), Some(3));
        assert_eq!(
            inner_text("<w:sdt><w:r><w:t>a &amp; b</w:t></w:r></w:sdt>", &["t"]),
            "a & b"
        );
        assert_eq!(
            ordered(
                RPR_ORDER,
                vec![
                    "<w:sz w:val=\"2\"/>".into(),
                    "<w:b/>".into(),
                    "<w:x/>".into()
                ]
            ),
            "<w:b/><w:sz w:val=\"2\"/><w:x/>"
        );
    }
}
