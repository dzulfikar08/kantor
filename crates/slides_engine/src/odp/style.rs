//! Styles of an OpenDocument presentation: named and automatic styles of
//! `styles.xml` and `content.xml` by family and name, the drawing
//! resources they name (gradients, hatches, bitmaps, dashes, markers,
//! list styles), font declarations, and lookups of a property through a
//! chain of styles and their parents.

use std::collections::HashMap;

use roxmltree::{Document, Node};

pub(super) const NS_DRAW: &str = "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0";
pub(super) const NS_SVG: &str = "urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0";
pub(super) const NS_STYLE: &str = "urn:oasis:names:tc:opendocument:xmlns:style:1.0";
pub(super) const NS_FO: &str = "urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0";
pub(super) const NS_TEXT: &str = "urn:oasis:names:tc:opendocument:xmlns:text:1.0";
pub(super) const NS_TABLE: &str = "urn:oasis:names:tc:opendocument:xmlns:table:1.0";
pub(super) const NS_XLINK: &str = "http://www.w3.org/1999/xlink";
pub(super) const NS_PRES: &str = "urn:oasis:names:tc:opendocument:xmlns:presentation:1.0";
pub(super) const NS_OFFICE: &str = "urn:oasis:names:tc:opendocument:xmlns:office:1.0";
pub(super) const NS_LOEXT: &str =
    "urn:org:documentfoundation:names:experimental:office:xmlns:loext:1.0";
pub(super) const NS_DRAWOOO: &str = "http://openoffice.org/2010/draw";
pub(super) const NS_ANIM: &str = "urn:oasis:names:tc:opendocument:xmlns:animation:1.0";
pub(super) const NS_SMIL: &str = "urn:oasis:names:tc:opendocument:xmlns:smil-compatible:1.0";

/// One style to look in: its family and name.
pub(super) type Ref<'a> = (&'static str, &'a str);

/// Every style of a presentation.
pub(super) struct Styles<'a, 'i> {
    styles: HashMap<(&'a str, &'a str), Node<'a, 'i>>,
    defaults: HashMap<&'a str, Node<'a, 'i>>,
    pub lists: HashMap<&'a str, Node<'a, 'i>>,
    pub gradients: HashMap<&'a str, Node<'a, 'i>>,
    pub hatches: HashMap<&'a str, Node<'a, 'i>>,
    pub images: HashMap<&'a str, Node<'a, 'i>>,
    pub dashes: HashMap<&'a str, Node<'a, 'i>>,
    pub markers: HashMap<&'a str, Node<'a, 'i>>,
    pub page_layouts: HashMap<&'a str, Node<'a, 'i>>,
    pub templates: HashMap<&'a str, Node<'a, 'i>>,
    /// `style:font-face` names to their family.
    fonts: HashMap<&'a str, &'a str>,
}

impl<'a, 'i> Styles<'a, 'i> {
    /// Read `docs` in order (later ones win: content after styles).
    pub fn read(docs: &[&'a Document<'i>]) -> Self {
        let mut s = Styles {
            styles: HashMap::new(),
            defaults: HashMap::new(),
            lists: HashMap::new(),
            gradients: HashMap::new(),
            hatches: HashMap::new(),
            images: HashMap::new(),
            dashes: HashMap::new(),
            markers: HashMap::new(),
            page_layouts: HashMap::new(),
            templates: HashMap::new(),
            fonts: HashMap::new(),
        };
        for doc in docs {
            for n in doc.descendants().filter(|n| n.is_element()) {
                let tag = n.tag_name();
                let draw_name = || n.attribute((NS_DRAW, "name"));
                let style_name = || n.attribute((NS_STYLE, "name"));
                match (tag.namespace().unwrap_or_default(), tag.name()) {
                    (NS_STYLE, "style") => {
                        if let (Some(name), Some(family)) =
                            (style_name(), n.attribute((NS_STYLE, "family")))
                        {
                            s.styles.insert((family, name), n);
                        }
                    }
                    (NS_STYLE, "default-style") => {
                        if let Some(family) = n.attribute((NS_STYLE, "family")) {
                            s.defaults.insert(family, n);
                        }
                    }
                    (NS_STYLE, "page-layout") => {
                        if let Some(name) = style_name() {
                            s.page_layouts.insert(name, n);
                        }
                    }
                    (NS_STYLE, "font-face") => {
                        if let (Some(name), Some(family)) =
                            (style_name(), n.attribute((NS_SVG, "font-family")))
                        {
                            s.fonts.insert(name, family);
                        }
                    }
                    (NS_TEXT, "list-style") => {
                        if let Some(name) = style_name() {
                            s.lists.insert(name, n);
                        }
                    }
                    (NS_TABLE, "table-template") => {
                        if let Some(name) = n
                            .attribute((NS_TABLE, "name"))
                            .or_else(|| n.attribute((NS_TEXT, "style-name")))
                        {
                            s.templates.insert(name, n);
                        }
                    }
                    (NS_DRAW, kind) => {
                        let map = match kind {
                            "gradient" => &mut s.gradients,
                            "hatch" => &mut s.hatches,
                            "fill-image" => &mut s.images,
                            "stroke-dash" => &mut s.dashes,
                            "marker" => &mut s.markers,
                            _ => continue,
                        };
                        if let Some(name) = draw_name() {
                            map.insert(name, n);
                        }
                    }
                    _ => {}
                }
            }
        }
        s
    }

    /// Whether style `name` of `family` exists.
    pub fn has(&self, family: &str, name: &str) -> bool {
        self.styles.contains_key(&(family, name))
    }

    /// The style elements of `chain`, each followed by its parents, then
    /// the default styles of the families involved and of graphics.
    pub fn nodes(&self, chain: &[Ref<'_>]) -> Vec<Node<'a, 'i>> {
        let mut out = Vec::new();
        let mut families: Vec<&str> = Vec::new();
        for &(family, name) in chain {
            if !families.contains(&family) {
                families.push(family);
            }
            let mut at = Some(name);
            for _ in 0..16 {
                let Some(n) = at.and_then(|n| self.styles.get(&(family, n))) else {
                    break;
                };
                out.push(*n);
                at = n.attribute((NS_STYLE, "parent-style-name"));
            }
        }
        if !families.contains(&"graphic") {
            families.push("graphic");
        }
        for f in families {
            if let Some(d) = self.defaults.get(f) {
                out.push(*d);
            }
        }
        out
    }

    /// The first `f(props)` that is `Some`, over the `props` elements
    /// (`graphic-properties`, `text-properties`...) of `chain`.
    pub fn find<T>(
        &self,
        chain: &[Ref<'_>],
        props: &str,
        mut f: impl FnMut(Node<'a, 'i>) -> Option<T>,
    ) -> Option<T> {
        for n in self.nodes(chain) {
            for p in n.children().filter(|c| {
                c.is_element()
                    && c.tag_name().name() == props
                    && matches!(c.tag_name().namespace(), Some(NS_STYLE | NS_LOEXT))
            }) {
                if let Some(v) = f(p) {
                    return Some(v);
                }
            }
        }
        None
    }

    /// Attribute `ns:attr` of the `props` elements of `chain`.
    pub fn prop(&self, chain: &[Ref<'_>], props: &str, ns: &str, attr: &str) -> Option<&'a str> {
        self.find(chain, props, |p| p.attribute((ns, attr)))
    }

    /// A graphic property.
    pub fn g(&self, chain: &[Ref<'_>], ns: &str, attr: &str) -> Option<&'a str> {
        self.prop(chain, "graphic-properties", ns, attr)
    }

    /// The family name of a font declaration (or the name itself).
    pub fn font(&self, name: &str) -> String {
        let f = self.fonts.get(name).copied().unwrap_or(name);
        f.trim().trim_matches('\'').trim_matches('"').to_owned()
    }

    /// A list style named by the style elements of `chain` (the
    /// `style:list-style-name` of a style, or a `text:list-style` inside
    /// its graphic properties, as LibreOffice keeps outline bullets).
    pub fn list_of(&self, chain: &[Ref<'_>]) -> Option<Node<'a, 'i>> {
        for n in self.nodes(chain) {
            if let Some(l) = n
                .attribute((NS_STYLE, "list-style-name"))
                .and_then(|l| self.lists.get(l))
            {
                return Some(*l);
            }
            for p in n
                .children()
                .filter(|c| c.tag_name().name() == "graphic-properties")
            {
                if let Some(l) = p
                    .children()
                    .find(|c| c.has_tag_name((NS_TEXT, "list-style")))
                {
                    return Some(l);
                }
            }
        }
        None
    }

    /// A style of family `table-cell` or `graphic` named `name`.
    pub fn cell_family(&self, name: &str) -> &'static str {
        if self.has("table-cell", name) {
            "table-cell"
        } else {
            "graphic"
        }
    }
}

/// A length (`1.85in`, `2.5cm`, `10pt`...) in EMU.
pub(super) fn emu(s: &str) -> Option<i64> {
    let s = s.trim();
    let split = s
        .find(|c: char| c.is_ascii_alphabetic() || c == '%')
        .unwrap_or(s.len());
    let (n, unit) = s.split_at(split);
    let n: f64 = n.trim().parse().ok()?;
    let k = match unit {
        "cm" => 360_000.0,
        "mm" => 36_000.0,
        "in" | "inch" => 914_400.0,
        "pt" => 12_700.0,
        "pc" => 152_400.0,
        "px" => 9_525.0,
        // Unitless: 1/100 mm (as in view boxes).
        "" => 360.0,
        _ => return None,
    };
    let v = n * k;
    (v.is_finite() && v.abs() < 1e13).then(|| v.round() as i64)
}

/// A length in points.
pub(super) fn points(s: &str) -> Option<f64> {
    emu(s).map(|e| e as f64 / 12_700.0)
}

/// A percentage (`45%`) as a fraction.
pub(super) fn percent(s: &str) -> Option<f64> {
    let v: f64 = s.trim().strip_suffix('%')?.trim().parse().ok()?;
    v.is_finite().then_some(v / 100.0)
}

/// `#rrggbb` as `RRGGBB`.
pub(super) fn color(s: &str) -> Option<String> {
    let hex = s.trim().strip_prefix('#')?;
    (hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| hex.to_ascii_uppercase())
}

/// An angle (`300`: tenths of a degree, `30deg`, `0.5rad`) in degrees.
pub(super) fn angle_degrees(s: &str) -> Option<f64> {
    let s = s.trim();
    let v = if let Some(d) = s.strip_suffix("deg") {
        d.trim().parse().ok()?
    } else if let Some(r) = s.strip_suffix("grad") {
        r.trim().parse::<f64>().ok()? * 0.9
    } else if let Some(r) = s.strip_suffix("rad") {
        r.trim().parse::<f64>().ok()?.to_degrees()
    } else {
        s.parse::<f64>().ok()? / 10.0
    };
    v.is_finite().then_some(v)
}

/// Text escaped for XML.
pub(super) fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => {}
            '\u{FFFE}' | '\u{FFFF}' => {}
            c => o.push(c),
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units() {
        assert_eq!(emu("1in"), Some(914_400));
        assert_eq!(emu("2.54cm"), Some(914_400));
        assert_eq!(emu("10pt"), Some(127_000));
        assert_eq!(emu("1000"), Some(360_000));
        assert_eq!(emu("x"), None);
        assert_eq!(percent("45%"), Some(0.45));
        assert_eq!(color("#4a7ebb").as_deref(), Some("4A7EBB"));
        assert_eq!(color("red"), None);
        assert_eq!(angle_degrees("300"), Some(30.0));
        assert_eq!(angle_degrees("30deg"), Some(30.0));
    }
}
