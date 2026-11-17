//! Format Shape for worksheet shapes and text boxes, as Excel's pane sets
//! them: the fill (no fill, a solid colour with transparency, a gradient),
//! the outline (no line, colour, width, dash, arrowheads) and where the
//! text sits (top, middle, bottom).
//!
//! Each edit rewrites only the elements it changes inside the shape's
//! `spPr` (or the `bodyPr` attribute), in the schema's order; the rest of
//! the element stays byte for byte. Edits go through
//! [`Workbook::set_shape_xml`], one undo step each.

use anyhow::{Result, anyhow, bail};
use roxmltree::{Document, Node};

use crate::Workbook;
use crate::chart_edit::{NEW_CHART_BASE, NewShape};
use crate::drawing::DrawingKey;
use std::sync::Arc;

pub use crate::shape_edit::{plain_text, takes_text};

/// Whether a shape element is a connector (`xdr:cxnSp`: lines and arrows).
pub fn is_connector(xml: &str) -> bool {
    Document::parse(xml).is_ok_and(|d| d.root_element().tag_name().name() == "cxnSp")
}

const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";

/// Fill elements of `spPr` and `ln` (one at most).
const FILLS: [&str; 6] = [
    "noFill",
    "solidFill",
    "gradFill",
    "blipFill",
    "pattFill",
    "grpFill",
];

/// A shape's fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeFill {
    None,
    /// A colour and its transparency in percent (0 = opaque).
    Solid {
        rgb: [u8; 3],
        transparency: u8,
    },
    /// Excel's "light gradient" preset of a colour: lighter to the colour,
    /// top to bottom (`vertical`) or left to right.
    Gradient {
        rgb: [u8; 3],
        vertical: bool,
    },
}

/// Excel's dash types (Format Shape ▸ Line ▸ Dash type).
pub const DASHES: [&str; 8] = [
    "solid",
    "sysDot",
    "sysDash",
    "dash",
    "dashDot",
    "lgDash",
    "lgDashDot",
    "lgDashDotDot",
];

/// Arrowhead types (`a:headEnd` / `a:tailEnd`); `"none"` takes it off.
pub const ARROWS: [&str; 6] = ["none", "triangle", "arrow", "stealth", "diamond", "oval"];

/// An outline edit; `None` fields stay as they are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutlineEdit {
    /// `Some(None)`: no line; `Some(Some(rgb))`: a solid colour.
    pub color: Option<Option<[u8; 3]>>,
    /// EMU.
    pub width: Option<i64>,
    /// One of [`DASHES`].
    pub dash: Option<&'static str>,
    /// One of [`ARROWS`], at the start and end of a line.
    pub head: Option<&'static str>,
    pub tail: Option<&'static str>,
}

/// Vertical alignment of a shape's text (`bodyPr/@anchor`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAnchor {
    Top,
    Middle,
    Bottom,
}

impl TextAnchor {
    fn attr(self) -> &'static str {
        match self {
            TextAnchor::Top => "t",
            TextAnchor::Middle => "ctr",
            TextAnchor::Bottom => "b",
        }
    }
}

fn is_a(n: Node, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some(NS_A)
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("{r:02X}{g:02X}{b:02X}")
}

/// The prefix (with its colon) DrawingML elements take in `el`'s scope.
fn a_prefix(el: Node) -> Result<String> {
    match el.lookup_prefix(NS_A) {
        Some("") => Ok(String::new()),
        Some(p) => Ok(format!("{p}:")),
        None => bail!("this shape has no DrawingML namespace"),
    }
}

/// The `spPr` child of the shape element.
fn sp_pr<'a, 'i>(root: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    root.children()
        .find(|c| c.is_element() && c.tag_name().name() == "spPr")
}

/// Where a new child goes in `parent`: before the first child named in
/// `before`, else before the end tag. (`parent` must have an end tag;
/// see [`open_up`].)
fn insert_at(xml: &str, parent: Node, before: &[&str]) -> usize {
    parent
        .children()
        .filter(|c| c.is_element())
        .find(|c| before.contains(&c.tag_name().name()))
        .map(|c| c.range().start)
        .unwrap_or_else(|| parent.range().end - end_tag_len(xml, parent))
}

fn end_tag_len(xml: &str, n: Node) -> usize {
    let s = &xml[n.range()];
    if s.ends_with("/>") {
        0
    } else {
        s.len() - s.rfind("</").unwrap_or(s.len())
    }
}

/// `xml` with a self-closing element `n` (`<a:spPr/>`) given an end tag,
/// so children can go in. `None`: it already has one.
fn open_up(xml: &str, n: Node) -> Option<String> {
    let s = &xml[n.range()];
    if !s.ends_with("/>") {
        return None;
    }
    let name = match n.lookup_prefix(n.tag_name().namespace().unwrap_or_default()) {
        Some(p) if !p.is_empty() => format!("{p}:{}", n.tag_name().name()),
        _ => n.tag_name().name().to_owned(),
    };
    let mut out = xml.to_owned();
    let end = n.range().end;
    out.replace_range(end - 2..end, &format!("></{name}>"));
    Some(out)
}

/// Run `f` on the shape's `spPr` (made, or opened up, first if needed).
fn with_sp_pr(xml: &str, f: impl Fn(&str, Node, &str) -> Result<String>) -> Result<String> {
    let doc = Document::parse(xml).map_err(|e| anyhow!("shape XML: {e}"))?;
    let root = doc.root_element();
    let a = a_prefix(root)?;
    let Some(sp) = sp_pr(root) else {
        bail!("this object has no shape properties");
    };
    if let Some(opened) = open_up(xml, sp) {
        return with_sp_pr(&opened, f);
    }
    f(xml, sp, &a)
}

/// The fill's XML.
fn fill_xml(fill: ShapeFill, a: &str) -> String {
    match fill {
        ShapeFill::None => format!("<{a}noFill/>"),
        ShapeFill::Solid { rgb, transparency } => {
            let t = transparency.min(100);
            if t == 0 {
                format!(
                    "<{a}solidFill><{a}srgbClr val=\"{}\"/></{a}solidFill>",
                    hex(rgb)
                )
            } else {
                format!(
                    "<{a}solidFill><{a}srgbClr val=\"{}\"><{a}alpha val=\"{}\"/></{a}srgbClr></{a}solidFill>",
                    hex(rgb),
                    (100 - t as u32) * 1000
                )
            }
        }
        ShapeFill::Gradient { rgb, vertical } => {
            let light = rgb.map(|c| (c as u32 + (255 - c as u32) * 3 / 5) as u8);
            let ang = if vertical { 5_400_000 } else { 0 };
            format!(
                "<{a}gradFill rotWithShape=\"1\"><{a}gsLst><{a}gs pos=\"0\"><{a}srgbClr val=\"{}\"/></{a}gs><{a}gs pos=\"100000\"><{a}srgbClr val=\"{}\"/></{a}gs></{a}gsLst><{a}lin ang=\"{ang}\" scaled=\"0\"/></{a}gradFill>",
                hex(light),
                hex(rgb)
            )
        }
    }
}

/// `xml` (a shape element) with its fill set to `fill`.
pub fn set_fill(xml: &str, fill: ShapeFill) -> Result<String> {
    with_sp_pr(xml, |xml, sp, a| {
        let new = fill_xml(fill, a);
        let mut out = xml.to_owned();
        match sp.children().find(|c| FILLS.iter().any(|f| is_a(*c, f))) {
            Some(old) => out.replace_range(old.range(), &new),
            None => {
                let at = insert_at(
                    xml,
                    sp,
                    &["ln", "effectLst", "effectDag", "scene3d", "sp3d", "extLst"],
                );
                out.insert_str(at, &new);
            }
        }
        Ok(out)
    })
}

/// `xml` (a shape element) with its outline changed by `edit`.
pub fn set_outline(xml: &str, edit: &OutlineEdit) -> Result<String> {
    with_sp_pr(xml, |xml, sp, a| {
        let old = sp.children().find(|c| is_a(*c, "ln"));
        // The `ln` start tag's attributes, with the new width.
        let mut attrs: Vec<(String, String)> = old
            .map(|ln| {
                ln.attributes()
                    .map(|at| (at.name().to_owned(), at.value().to_owned()))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(w) = edit.width {
            attrs.retain(|(k, _)| k != "w");
            attrs.insert(0, ("w".into(), w.max(0).to_string()));
        }
        let raw = |name: &[&str]| -> String {
            old.map(|ln| {
                ln.children()
                    .filter(|c| name.iter().any(|n| is_a(*c, n)))
                    .map(|c| xml[c.range()].to_owned())
                    .collect()
            })
            .unwrap_or_default()
        };
        let fill = match edit.color {
            Some(None) => format!("<{a}noFill/>"),
            Some(Some(rgb)) => {
                format!(
                    "<{a}solidFill><{a}srgbClr val=\"{}\"/></{a}solidFill>",
                    hex(rgb)
                )
            }
            None => raw(&FILLS),
        };
        let dash = match edit.dash {
            Some(d) => format!("<{a}prstDash val=\"{d}\"/>"),
            None => raw(&["prstDash", "custDash"]),
        };
        let end = |tag: &str, v: Option<&str>| match v {
            Some("none") => String::new(),
            Some(t) => format!("<{a}{tag} type=\"{t}\"/>"),
            None => raw(&[tag]),
        };
        let join = raw(&["round", "bevel", "miter"]);
        let head = end("headEnd", edit.head);
        let tail = end("tailEnd", edit.tail);
        let ext = raw(&["extLst"]);
        let mut new = format!("<{a}ln");
        for (k, v) in &attrs {
            new.push_str(&format!(" {k}=\"{}\"", esc_attr(v)));
        }
        let body = format!("{fill}{dash}{join}{head}{tail}{ext}");
        if body.is_empty() {
            new.push_str("/>");
        } else {
            new.push_str(&format!(">{body}</{a}ln>"));
        }
        let mut out = xml.to_owned();
        match old {
            Some(ln) => out.replace_range(ln.range(), &new),
            None => {
                let at = insert_at(
                    xml,
                    sp,
                    &["effectLst", "effectDag", "scene3d", "sp3d", "extLst"],
                );
                out.insert_str(at, &new);
            }
        }
        Ok(out)
    })
}

fn esc_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
}

/// `xml` (a shape element with a text body) with its text anchored at
/// `anchor`.
pub fn set_text_anchor(xml: &str, anchor: TextAnchor) -> Result<String> {
    let doc = Document::parse(xml).map_err(|e| anyhow!("shape XML: {e}"))?;
    let root = doc.root_element();
    let body = root
        .children()
        .find(|c| c.is_element() && c.tag_name().name() == "txBody")
        .and_then(|tb| tb.children().find(|c| is_a(*c, "bodyPr")))
        .ok_or_else(|| anyhow!("this shape has no text"))?;
    let start = body.range().start;
    let tag_end = start
        + xml[start..]
            .find('>')
            .ok_or_else(|| anyhow!("shape XML: open tag"))?;
    let tag = &xml[start..tag_end];
    let mut out = xml.to_owned();
    let val = anchor.attr();
    if let Some(at) = body.attributes().find(|at| at.name() == "anchor") {
        out.replace_range(at.range_value(), val);
    } else {
        let at = if tag.ends_with('/') {
            tag_end - 1
        } else {
            tag_end
        };
        out.insert_str(at, &format!(" anchor=\"{val}\""));
    }
    Ok(out)
}

impl Workbook {
    /// The element of shape `key` as it is now, when Kantor can change it
    /// (a shape, text box or line from the file or made in Kantor).
    pub fn shape_xml(&self, key: DrawingKey) -> Option<Arc<str>> {
        let (xml, editable) = self.shape_now(key)?;
        editable.then_some(xml)
    }

    /// Replace the element of shape `key` with `xml` (one undo step;
    /// nothing when it is the same). A save writes it in place of the old
    /// one.
    pub fn set_shape_xml(&mut self, key: DrawingKey, xml: &str) -> Result<()> {
        let (old, editable) = self
            .shape_now(key)
            .ok_or_else(|| anyhow!("this object isn't a shape"))?;
        if !editable {
            bail!("Kantor can't change this shape");
        }
        if &*old == xml {
            return Ok(());
        }
        Document::parse(xml).map_err(|e| anyhow!("shape XML: {e}"))?;
        let new: Arc<str> = xml.into();
        if key.index >= NEW_CHART_BASE {
            let id = (key.index - NEW_CHART_BASE) as u32;
            self.own_edit(|e, _| {
                if let Some(c) = e
                    .charts
                    .iter_mut()
                    .find(|c| c.id == id && c.sheet_id == key.sheet_id)
                    && let Some(s) = &c.shape
                {
                    c.shape = Some(Arc::new(NewShape {
                        xml: new,
                        name: s.name.clone(),
                        slicer: s.slicer.clone(),
                        timeline: s.timeline.clone(),
                        media: s.media.clone(),
                    }));
                }
            });
        } else {
            self.own_edit(|e, _| {
                e.shape_texts.insert(key, new);
            });
        }
        Ok(())
    }

    /// Apply `f` to shape `key`'s element (one undo step).
    pub fn edit_shape(
        &mut self,
        key: DrawingKey,
        f: impl FnOnce(&str) -> Result<String>,
    ) -> Result<()> {
        let xml = self
            .shape_xml(key)
            .ok_or_else(|| anyhow!("Kantor can't change this object"))?;
        let new = f(&xml)?;
        self.set_shape_xml(key, &new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ShapePreset;

    fn parses(x: &str) {
        Document::parse(x).unwrap();
    }

    #[test]
    fn fills_replace_the_old_one_in_place() {
        let rect = ShapePreset::Rectangle.xml("Rectangle 1");
        let red = set_fill(
            &rect,
            ShapeFill::Solid {
                rgb: [255, 0, 0],
                transparency: 0,
            },
        )
        .unwrap();
        parses(&red);
        assert!(red.contains(
            "</a:prstGeom><a:solidFill><a:srgbClr val=\"FF0000\"/></a:solidFill></xdr:spPr>"
        ));
        let half = set_fill(
            &red,
            ShapeFill::Solid {
                rgb: [0, 0, 255],
                transparency: 40,
            },
        )
        .unwrap();
        assert!(half.contains("<a:srgbClr val=\"0000FF\"><a:alpha val=\"60000\"/></a:srgbClr>"));
        assert_eq!(half.matches("<a:solidFill>").count(), 1);
        let none = set_fill(&half, ShapeFill::None).unwrap();
        assert!(none.contains("</a:prstGeom><a:noFill/></xdr:spPr>"));
        let grad = set_fill(
            &none,
            ShapeFill::Gradient {
                rgb: [0, 0, 0],
                vertical: true,
            },
        )
        .unwrap();
        parses(&grad);
        assert!(grad.contains("<a:gs pos=\"0\"><a:srgbClr val=\"999999\"/>"));
        assert!(grad.contains("<a:lin ang=\"5400000\""));
        // The text box's fill sits before its outline.
        let tb = ShapePreset::TextBox.xml("TextBox 1");
        let t = set_fill(&tb, ShapeFill::None).unwrap();
        assert!(t.contains("</a:prstGeom><a:noFill/><a:ln w=\"9525\""));
        // Everything outside spPr is untouched.
        let tail = |s: &str| s[s.find("</xdr:spPr>").unwrap()..].to_owned();
        assert_eq!(tail(&t), tail(&tb));
        // A self-closing spPr opens up.
        let bare = "<xdr:sp xmlns:xdr=\"http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><xdr:nvSpPr><xdr:cNvPr id=\"2\" name=\"x\"/><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr/></xdr:sp>";
        let b = set_fill(bare, ShapeFill::None).unwrap();
        assert!(b.contains("<xdr:spPr><a:noFill/></xdr:spPr>"));
    }

    #[test]
    fn outlines_keep_what_they_dont_change() {
        let tb = ShapePreset::TextBox.xml("TextBox 1");
        let blue = set_outline(
            &tb,
            &OutlineEdit {
                color: Some(Some([0, 0, 255])),
                width: Some(28_575),
                dash: Some("dash"),
                ..Default::default()
            },
        )
        .unwrap();
        parses(&blue);
        assert!(blue.contains("<a:ln w=\"28575\" cmpd=\"sng\"><a:solidFill><a:srgbClr val=\"0000FF\"/></a:solidFill><a:prstDash val=\"dash\"/></a:ln>"));
        let none = set_outline(
            &blue,
            &OutlineEdit {
                color: Some(None),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(none.contains(
            "<a:ln w=\"28575\" cmpd=\"sng\"><a:noFill/><a:prstDash val=\"dash\"/></a:ln>"
        ));
        // Arrowheads on a line: added, changed, taken off.
        let arrow = ShapePreset::Arrow.xml("Arrow 1");
        let both = set_outline(
            &arrow,
            &OutlineEdit {
                head: Some("oval"),
                tail: Some("stealth"),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            both.contains("<a:ln><a:headEnd type=\"oval\"/><a:tailEnd type=\"stealth\"/></a:ln>")
        );
        let plain = set_outline(
            &both,
            &OutlineEdit {
                head: Some("none"),
                tail: Some("none"),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(plain.contains("<a:ln/>"));
        // A shape without an outline gets one before its effects.
        let rect = ShapePreset::Rectangle.xml("R");
        let r = set_outline(
            &rect,
            &OutlineEdit {
                width: Some(12_700),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(r.contains("</a:prstGeom><a:ln w=\"12700\"/></xdr:spPr>"));
    }

    #[test]
    fn text_anchor_is_set_on_the_body() {
        let rect = ShapePreset::Rectangle.xml("R");
        let mid = set_text_anchor(&rect, TextAnchor::Middle).unwrap();
        assert!(mid.contains("anchor=\"ctr\"/>"));
        assert!(!mid.contains("anchor=\"t\""));
        let bare = mid.replace(" anchor=\"ctr\"", "");
        let b = set_text_anchor(&bare, TextAnchor::Bottom).unwrap();
        assert!(b.contains("rtlCol=\"0\" anchor=\"b\"/>"));
        let line = ShapePreset::Line.xml("L");
        assert!(set_text_anchor(&line, TextAnchor::Top).is_err());
    }
}
