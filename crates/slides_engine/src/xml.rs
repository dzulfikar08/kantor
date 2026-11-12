//! Small XML and OPC helpers: escaping, element lookup, relationships,
//! content types and part-name arithmetic.

use roxmltree::Node;

pub const NS_P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
pub const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
pub const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
pub const NS_PKG_REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
pub const NS_CT: &str = "http://schemas.openxmlformats.org/package/2006/content-types";

const REL_BASE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/";

pub const CT_SLIDE: &str = "application/vnd.openxmlformats-officedocument.presentationml.slide+xml";

pub const CT_NOTES: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.notesSlide+xml";
pub const CT_NOTES_MASTER: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.notesMaster+xml";
pub const CT_THEME: &str = "application/vnd.openxmlformats-officedocument.theme+xml";

pub const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";

/// Relationship type by its short name (`slideLayout`, `image`, ...).
pub fn rel_type(short: &str) -> String {
    format!("{REL_BASE}{short}")
}

/// The short name of a relationship type (`.../slideLayout` → `slideLayout`).
pub fn rel_kind(ty: &str) -> &str {
    ty.rsplit('/').next().unwrap_or(ty)
}

/// Escape text content.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// Escape an attribute value (double-quoted).
pub fn esc_attr(s: &str) -> String {
    esc(s).replace('"', "&quot;")
}

pub fn is(n: Node, ns: &str, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some(ns)
}

pub fn child<'a, 'i>(n: Node<'a, 'i>, ns: &str, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| is(*c, ns, name))
}

pub fn children<'a, 'i>(
    n: Node<'a, 'i>,
    ns: &'static str,
    name: &'static str,
) -> impl Iterator<Item = Node<'a, 'i>> {
    n.children().filter(move |c| is(*c, ns, name))
}

/// Follow a path of children, e.g. `&[(NS_P, "nvSpPr"), (NS_P, "cNvPr")]`.
pub fn path<'a, 'i>(n: Node<'a, 'i>, steps: &[(&str, &str)]) -> Option<Node<'a, 'i>> {
    let mut cur = n;
    for (ns, name) in steps {
        cur = child(cur, ns, name)?;
    }
    Some(cur)
}

pub fn attr_i64(n: Node, name: &str) -> Option<i64> {
    n.attribute(name)?.trim().parse().ok()
}

pub fn attr_bool(n: Node, name: &str) -> Option<bool> {
    match n.attribute(name)? {
        "1" | "true" | "on" => Some(true),
        "0" | "false" | "off" => Some(false),
        _ => None,
    }
}

/// An attribute in the relationships namespace (`r:id`, `r:embed`).
pub fn r_attr<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
    n.attribute((NS_R, name))
}

/// Raw source text of a node.
pub fn raw<'i>(src: &'i str, n: Node) -> &'i str {
    &src[n.range()]
}

/// The start tag of an element as written in the source (`<p:sp ...>`).
pub fn start_tag<'i>(src: &'i str, n: Node) -> &'i str {
    let s = &src[n.range()];
    match tag_end(s) {
        Some(e) => &s[..e],
        None => s,
    }
}

/// Byte length of the first tag in `s` (through its `>`), honouring quotes.
pub fn tag_end(s: &str) -> Option<usize> {
    let mut quote = None;
    for (i, b) in s.bytes().enumerate() {
        match (quote, b) {
            (None, b'"' | b'\'') => quote = Some(b),
            (Some(q), b2) if q == b2 => quote = None,
            (None, b'>') => return Some(i + 1),
            _ => {}
        }
    }
    None
}

/// Raw attributes of a start tag (between the name and `>` / `/>`).
pub fn tag_attrs(tag: &str) -> &str {
    let inner = tag.trim_start_matches('<');
    let inner = inner.trim_end_matches('>').trim_end_matches('/');
    match inner.find(char::is_whitespace) {
        Some(i) => inner[i..].trim(),
        None => "",
    }
}

/// Raw attributes of `n`'s start tag except those in `skip` (qualified names
/// as written) and namespace declarations. Values keep their source escaping.
pub fn attrs_except(n: Node, src: &str, skip: &[&str]) -> Vec<(String, String)> {
    split_attrs(tag_attrs(start_tag(src, n)))
        .into_iter()
        .filter(|(k, _)| !skip.contains(&k.as_str()) && !k.starts_with("xmlns"))
        .collect()
}

/// Parse `a="1" b='2'` into pairs (values stay escaped as in the source).
pub fn split_attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let ks = i;
        while i < b.len() && b[i] != b'=' && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        let key = s[ks..i].to_owned();
        while i < b.len() && b[i] != b'"' && b[i] != b'\'' {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let q = b[i];
        i += 1;
        let vs = i;
        while i < b.len() && b[i] != q {
            i += 1;
        }
        let val = s[vs..i.min(b.len())].to_owned();
        i += 1;
        if !key.is_empty() {
            out.push((key, val));
        }
    }
    out
}

/// Append raw (already escaped) attributes.
pub fn write_attrs(out: &mut String, attrs: &[(String, String)]) {
    for (k, v) in attrs {
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        out.push_str(&v.replace('"', "&quot;"));
        out.push('"');
    }
}

// ── OPC ────────────────────────────────────────────────────────────────────

/// Resolve a relationship target relative to the part at `base`.
pub fn resolve(base: &str, target: &str) -> String {
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

/// A relationship target for `part`, relative to the part at `base`.
pub fn relative(base: &str, part: &str) -> String {
    let from: Vec<&str> = base.split('/').collect();
    let to: Vec<&str> = part.split('/').collect();
    let from_dir = &from[..from.len().saturating_sub(1)];
    let common = from_dir.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut out: Vec<&str> = vec![".."; from_dir.len() - common];
    out.extend(&to[common..]);
    out.join("/")
}

pub fn rels_name(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rel {
    pub id: String,
    pub ty: String,
    /// As written (relative to the source part, or absolute / external).
    pub target: String,
    pub external: bool,
}

impl Rel {
    /// The package part this points to, relative to `base` (`None` if external).
    pub fn part(&self, base: &str) -> Option<String> {
        (!self.external).then(|| resolve(base, &self.target))
    }

    pub fn kind(&self) -> &str {
        rel_kind(&self.ty)
    }
}

pub fn parse_rels(xml: &str) -> Vec<Rel> {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    doc.root_element()
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "Relationship")
        .filter_map(|n| {
            Some(Rel {
                id: n.attribute("Id")?.to_owned(),
                ty: n.attribute("Type")?.to_owned(),
                target: n.attribute("Target")?.to_owned(),
                external: n.attribute("TargetMode") == Some("External"),
            })
        })
        .collect()
}

pub fn write_rels(rels: &[Rel]) -> String {
    let mut s = String::from(XML_DECL);
    s.push_str(&format!("<Relationships xmlns=\"{NS_PKG_REL}\">"));
    for r in rels {
        s.push_str(&format!(
            "<Relationship Id=\"{}\" Type=\"{}\" Target=\"{}\"{}/>",
            esc_attr(&r.id),
            esc_attr(&r.ty),
            esc_attr(&r.target),
            if r.external {
                " TargetMode=\"External\""
            } else {
                ""
            }
        ));
    }
    s.push_str("</Relationships>");
    s
}

/// A fresh `rIdN` not used in `rels`.
pub fn next_rel_id(rels: &[Rel]) -> String {
    let max = rels
        .iter()
        .filter_map(|r| r.id.strip_prefix("rId")?.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    format!("rId{}", max + 1)
}

/// `[Content_Types].xml`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContentTypes {
    /// Extension (lowercase) → content type.
    pub defaults: Vec<(String, String)>,
    /// Part name (without the leading `/`) → content type.
    pub overrides: Vec<(String, String)>,
}

impl ContentTypes {
    pub fn parse(xml: &str) -> Self {
        let mut ct = Self::default();
        let Ok(doc) = roxmltree::Document::parse(xml) else {
            return ct;
        };
        for n in doc.root_element().children().filter(|n| n.is_element()) {
            match n.tag_name().name() {
                "Default" => {
                    if let (Some(e), Some(t)) =
                        (n.attribute("Extension"), n.attribute("ContentType"))
                    {
                        ct.defaults.push((e.to_ascii_lowercase(), t.to_owned()));
                    }
                }
                "Override" => {
                    if let (Some(p), Some(t)) =
                        (n.attribute("PartName"), n.attribute("ContentType"))
                    {
                        ct.overrides
                            .push((p.trim_start_matches('/').to_owned(), t.to_owned()));
                    }
                }
                _ => {}
            }
        }
        ct
    }

    pub fn write(&self) -> String {
        let mut s = String::from(XML_DECL);
        s.push_str(&format!("<Types xmlns=\"{NS_CT}\">"));
        for (e, t) in &self.defaults {
            s.push_str(&format!(
                "<Default Extension=\"{}\" ContentType=\"{}\"/>",
                esc_attr(e),
                esc_attr(t)
            ));
        }
        for (p, t) in &self.overrides {
            s.push_str(&format!(
                "<Override PartName=\"/{}\" ContentType=\"{}\"/>",
                esc_attr(p),
                esc_attr(t)
            ));
        }
        s.push_str("</Types>");
        s
    }

    /// Whether `part` has a content type.
    pub fn covers(&self, part: &str) -> bool {
        if self
            .overrides
            .iter()
            .any(|(p, _)| p.eq_ignore_ascii_case(part))
        {
            return true;
        }
        let ext = part.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
        ext.is_some_and(|ext| self.defaults.iter().any(|(e, _)| *e == ext))
    }

    /// Give `part` the content type `ty` (in place when it has one, so an
    /// edited slide doesn't reorder an untouched `[Content_Types].xml`).
    pub fn set_override(&mut self, part: &str, ty: &str) {
        match self.overrides.iter_mut().find(|(p, _)| p == part) {
            Some(o) => o.1 = ty.to_owned(),
            None => self.overrides.push((part.to_owned(), ty.to_owned())),
        }
    }

    pub fn remove_override(&mut self, part: &str) {
        self.overrides.retain(|(p, _)| p != part);
    }

    pub fn ensure_default(&mut self, ext: &str, ty: &str) {
        let ext = ext.to_ascii_lowercase();
        if !self.defaults.iter().any(|(e, _)| *e == ext) {
            self.defaults.push((ext, ty.to_owned()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_paths() {
        assert_eq!(
            resolve("ppt/slides/slide1.xml", "../slideLayouts/slideLayout2.xml"),
            "ppt/slideLayouts/slideLayout2.xml"
        );
        assert_eq!(
            resolve("ppt/presentation.xml", "slides/slide1.xml"),
            "ppt/slides/slide1.xml"
        );
        assert_eq!(resolve("ppt/x.xml", "/ppt/media/a.png"), "ppt/media/a.png");
        assert_eq!(
            relative("ppt/slides/slide1.xml", "ppt/media/image1.png"),
            "../media/image1.png"
        );
        assert_eq!(
            relative("ppt/presentation.xml", "ppt/slides/slide3.xml"),
            "slides/slide3.xml"
        );
        assert_eq!(
            rels_name("ppt/slides/slide1.xml"),
            "ppt/slides/_rels/slide1.xml.rels"
        );
    }

    #[test]
    fn attrs_split_and_tag_end() {
        let a = split_attrs(r#"lang="en-US" dirty='0'  b="a&amp;b""#);
        assert_eq!(a.len(), 3);
        assert_eq!(a[2], ("b".into(), "a&amp;b".into()));
        assert_eq!(tag_end(r#"<a:t x=">">hi</a:t>"#), Some(11));
        assert_eq!(
            tag_attrs("<a:rPr lang=\"en\" b=\"1\"/>"),
            "lang=\"en\" b=\"1\""
        );
    }

    #[test]
    fn rels_round_trip() {
        let rels = vec![Rel {
            id: "rId1".into(),
            ty: rel_type("slideLayout"),
            target: "../slideLayouts/slideLayout1.xml".into(),
            external: false,
        }];
        assert_eq!(parse_rels(&write_rels(&rels)), rels);
        assert_eq!(next_rel_id(&rels), "rId2");
    }
}
