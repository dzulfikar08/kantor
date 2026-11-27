//! Shapes and pictures for macros (`Worksheet.Shapes`, `Shape`): what the
//! object model needs beyond Insert ▸ Shapes and Format Shape.
//!
//! - New objects at a given place: [`Workbook::script_add_shape`] (any
//!   preset geometry, `Shapes.AddShape` / `AddTextbox`),
//!   [`Workbook::script_add_line`] and [`Workbook::script_add_picture`].
//!   They are objects made in Kantor like Insert ▸ Shapes' (one undo step
//!   each, written into the sheet's drawing on save).
//! - Properties kept on the object's `cNvPr` (name, alternative text,
//!   hidden) and its `macro` attribute (Assign Macro):
//!   [`Workbook::script_set_cnvpr`], [`Workbook::script_set_macro`]. For
//!   shapes they change the element through [`Workbook::set_shape_xml`],
//!   so a save writes them; pictures made in Kantor keep their name and
//!   alternative text on the object.
//! - Reading a shape element ([`shape_xml_info`]) and its colours
//!   ([`Workbook::shape_look`]), theme colours resolved.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use roxmltree::{Document, Node};

use crate::Workbook;
use crate::chart_edit::{NEW_CHART_BASE, NewChart, NewPicture, NewShape};
use crate::drawing::{Anchor, DrawingKey};
use crate::shape_edit::ShapePreset;

const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";

/// What a shape element says about itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShapeXml {
    /// The element's local name: `sp`, `cxnSp` or `grpSp`.
    pub element: String,
    /// Its `cNvPr` attributes.
    pub name: Option<String>,
    pub descr: Option<String>,
    pub hidden: bool,
    /// Its `macro` attribute (`None` when absent).
    pub macro_attr: Option<String>,
    /// A text box (`cNvSpPr txBox="1"`).
    pub text_box: bool,
    /// Its preset geometry (`a:prstGeom prst`).
    pub prst: Option<String>,
    /// A freeform (`a:custGeom`).
    pub cust_geom: bool,
    /// A form control's shape (`a14:compatExt`).
    pub form_control: bool,
}

/// A shape's colours and outline as Excel reports them (`Fill.ForeColor`,
/// `Line.Weight`...).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeLook {
    /// The fill colour (`0xRRGGBB`); `None`: no fill.
    pub fill: Option<u32>,
    /// The fill's transparency, 0 (opaque) to 1.
    pub transparency: f64,
    /// The outline colour; `None`: no line.
    pub line: Option<u32>,
    /// The outline's width in points.
    pub line_width: f64,
}

fn esc_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

/// The object's own `cNvPr` (in its `nv…Pr`).
fn own_cnvpr<'a, 'i>(root: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    root.children()
        .find(|c| c.is_element() && c.tag_name().name().starts_with("nv"))
        .and_then(|nv| child(nv, "cNvPr"))
}

/// An unprefixed attribute of `n`.
fn plain_attr<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<roxmltree::Attribute<'a, 'i>> {
    n.attributes()
        .find(|a| a.name() == name && a.namespace().is_none())
}

/// `xml` with unprefixed attribute `name` of element `n` set to `value`
/// (`None`: removed).
fn with_attr(xml: &str, n: Node, name: &str, value: Option<&str>) -> String {
    let mut out = xml.to_owned();
    match (plain_attr(n, name), value) {
        (Some(a), Some(v)) => out.replace_range(a.range_value(), &esc_attr(v)),
        (Some(a), None) => {
            let r = a.range();
            let start = xml[..r.start].trim_end().len();
            out.replace_range(start..r.end, "");
        }
        (None, Some(v)) => {
            // Right after the element's name in its start tag.
            let s = n.range().start + 1;
            let end = xml[s..]
                .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
                .map_or(xml.len(), |i| s + i);
            out.insert_str(end, &format!(" {name}=\"{}\"", esc_attr(v)));
        }
        (None, None) => {}
    }
    out
}

/// `xml` (an object element) with attribute `attr` of its own `cNvPr` set
/// to `value` (`None`: removed).
pub fn with_cnvpr_attr(xml: &str, attr: &str, value: Option<&str>) -> Result<String> {
    let doc = Document::parse(xml).map_err(|e| anyhow!("shape XML: {e}"))?;
    let nv = own_cnvpr(doc.root_element()).ok_or_else(|| anyhow!("this object has no name"))?;
    Ok(with_attr(xml, nv, attr, value))
}

/// What shape element `xml` says about itself (`None`: it doesn't parse).
pub fn shape_xml_info(xml: &str) -> Option<ShapeXml> {
    let doc = Document::parse(xml).ok()?;
    let root = doc.root_element();
    let nv = own_cnvpr(root);
    let text = |n: Option<Node>, k: &str| n.and_then(|n| n.attribute(k)).map(str::to_owned);
    let sp_pr = child(root, "spPr");
    Some(ShapeXml {
        element: root.tag_name().name().to_owned(),
        name: text(nv, "name"),
        descr: text(nv, "descr"),
        hidden: nv
            .and_then(|n| n.attribute("hidden"))
            .is_some_and(|v| v == "1" || v == "true"),
        macro_attr: plain_attr(root, "macro").map(|a| a.value().to_owned()),
        text_box: child(root, "nvSpPr")
            .and_then(|n| child(n, "cNvSpPr"))
            .and_then(|n| n.attribute("txBox"))
            .is_some_and(|v| v == "1" || v == "true"),
        prst: sp_pr
            .and_then(|s| child(s, "prstGeom"))
            .and_then(|g| g.attribute("prst"))
            .map(str::to_owned),
        cust_geom: sp_pr.and_then(|s| child(s, "custGeom")).is_some(),
        form_control: xml.contains("compatExt"),
    })
}

/// The theme's colour scheme as `(name, 0xRRGGBB)`; Office's defaults
/// for what it doesn't say.
fn theme_scheme(theme: Option<&str>) -> Vec<(&'static str, u32)> {
    let mut out = vec![
        ("lt1", 0xFFFFFF),
        ("dk1", 0x000000),
        ("lt2", 0xE7E6E6),
        ("dk2", 0x44546A),
        ("accent1", 0x4472C4),
        ("accent2", 0xED7D31),
        ("accent3", 0xA5A5A5),
        ("accent4", 0xFFC000),
        ("accent5", 0x5B9BD5),
        ("accent6", 0x70AD47),
        ("hlink", 0x0563C1),
        ("folHlink", 0x954F72),
    ];
    let Some(doc) = theme.and_then(|x| Document::parse(x).ok()) else {
        return out;
    };
    let Some(scheme) = doc
        .descendants()
        .find(|n| n.tag_name().name() == "clrScheme")
    else {
        return out;
    };
    for (name, rgb) in &mut out {
        let value = child(scheme, name)
            .and_then(|el| el.children().find(|n| n.is_element()))
            .and_then(|c| {
                if c.tag_name().name() == "srgbClr" {
                    c.attribute("val")
                } else {
                    c.attribute("lastClr")
                }
            })
            .and_then(|v| u32::from_str_radix(v, 16).ok());
        if let Some(v) = value {
            *rgb = v;
        }
    }
    out
}

fn split(c: u32) -> [f64; 3] {
    [
        ((c >> 16) & 0xFF) as f64,
        ((c >> 8) & 0xFF) as f64,
        (c & 0xFF) as f64,
    ]
}

fn join([r, g, b]: [f64; 3]) -> u32 {
    let k = |v: f64| v.round().clamp(0.0, 255.0) as u32;
    (k(r) << 16) | (k(g) << 8) | k(b)
}

/// The colour a DrawingML colour element (`a:srgbClr`, `a:schemeClr`...)
/// stands for, with its `shade` / `tint` / `lumMod` / `lumOff` applied
/// (approximately: in RGB).
fn color_value(c: Node, scheme: &[(&'static str, u32)]) -> Option<u32> {
    let val = c.attribute("val").unwrap_or_default();
    let base = match c.tag_name().name() {
        "srgbClr" => u32::from_str_radix(val, 16).ok()?,
        "sysClr" => c
            .attribute("lastClr")
            .and_then(|v| u32::from_str_radix(v, 16).ok())
            .unwrap_or(if val == "window" { 0xFFFFFF } else { 0 }),
        "schemeClr" => {
            let name = match val {
                "bg1" => "lt1",
                "tx1" => "dk1",
                "bg2" => "lt2",
                "tx2" => "dk2",
                other => other,
            };
            scheme.iter().find(|(n, _)| *n == name)?.1
        }
        "scrgbClr" => {
            let p = |k: &str| {
                c.attribute(k)
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(0.0)
                    / 100_000.0
                    * 255.0
            };
            join([p("r"), p("g"), p("b")])
        }
        "prstClr" => match val {
            "black" => 0x000000,
            "white" => 0xFFFFFF,
            "red" => 0xFF0000,
            "green" => 0x008000,
            "lime" => 0x00FF00,
            "blue" => 0x0000FF,
            "yellow" => 0xFFFF00,
            "gray" | "grey" => 0x808080,
            _ => 0x000000,
        },
        _ => return None,
    };
    let mut rgb = split(base);
    for m in c.children().filter(|m| m.is_element()) {
        let Some(v) = m.attribute("val").and_then(|v| v.parse::<f64>().ok()) else {
            continue;
        };
        let f = v / 100_000.0;
        for ch in &mut rgb {
            *ch = match m.tag_name().name() {
                "shade" => *ch * f,
                "tint" => *ch + (255.0 - *ch) * (1.0 - f),
                "lumMod" => *ch * f,
                "lumOff" => *ch + 255.0 * f,
                _ => *ch,
            };
        }
    }
    Some(join(rgb))
}

/// The first colour element inside `n`.
fn color_in(n: Node, scheme: &[(&'static str, u32)]) -> Option<u32> {
    n.children()
        .filter(|c| c.is_element())
        .find_map(|c| color_value(c, scheme))
}

/// The colour of a fill element (`a:solidFill`, `a:gradFill`...): `None`
/// for `a:noFill`.
fn fill_color(f: Node, scheme: &[(&'static str, u32)]) -> Option<u32> {
    match f.tag_name().name() {
        "noFill" => None,
        "solidFill" => color_in(f, scheme),
        "gradFill" => child(f, "gsLst")
            .and_then(|l| child(l, "gs"))
            .and_then(|g| color_in(g, scheme)),
        "pattFill" => child(f, "fgClr").and_then(|g| color_in(g, scheme)),
        "blipFill" => Some(0xFFFFFF),
        _ => None,
    }
}

fn is_fill(n: Node) -> bool {
    n.is_element()
        && n.tag_name().namespace() == Some(NS_A)
        && matches!(
            n.tag_name().name(),
            "noFill" | "solidFill" | "gradFill" | "blipFill" | "pattFill" | "grpFill"
        )
}

/// A `lnRef` / `fillRef` of the shape's style: its index and colour.
fn style_ref(root: Node, name: &str, scheme: &[(&'static str, u32)]) -> (u32, Option<u32>) {
    match child(root, "style").and_then(|s| child(s, name)) {
        Some(r) => (
            r.attribute("idx").and_then(|v| v.parse().ok()).unwrap_or(0),
            color_in(r, scheme),
        ),
        None => (0, None),
    }
}

impl Workbook {
    /// The fill and outline shape element `xml` shows (its own, else its
    /// style's), theme colours resolved.
    pub fn shape_look(&self, xml: &str) -> ShapeLook {
        let scheme = theme_scheme(self.theme_xml().as_deref());
        let mut look = ShapeLook {
            fill: None,
            transparency: 0.0,
            line: None,
            line_width: 0.75,
        };
        let Ok(doc) = Document::parse(xml) else {
            return look;
        };
        let root = doc.root_element();
        let sp = child(root, "spPr");
        let connector = root.tag_name().name() == "cxnSp";
        match sp.and_then(|s| s.children().find(|c| is_fill(*c))) {
            Some(f) => {
                look.fill = fill_color(f, &scheme);
                if f.tag_name().name() == "solidFill"
                    && let Some(alpha) = f
                        .children()
                        .find(|c| c.is_element())
                        .and_then(|c| child(c, "alpha"))
                        .and_then(|a| a.attribute("val"))
                        .and_then(|v| v.parse::<f64>().ok())
                {
                    look.transparency = (1.0 - alpha / 100_000.0).clamp(0.0, 1.0);
                }
            }
            None if !connector => {
                let (idx, color) = style_ref(root, "fillRef", &scheme);
                look.fill = if idx == 0 { None } else { color };
            }
            None => {}
        }
        let ln = sp.and_then(|s| child(s, "ln"));
        let (ref_idx, ref_color) = style_ref(root, "lnRef", &scheme);
        look.line = match ln.and_then(|l| l.children().find(|c| is_fill(*c))) {
            Some(f) => fill_color(f, &scheme),
            None if ref_idx == 0 => None,
            None => ref_color,
        };
        look.line_width = match ln
            .and_then(|l| l.attribute("w"))
            .and_then(|w| w.parse::<f64>().ok())
        {
            Some(w) => w / 12_700.0,
            None => match ref_idx {
                1 => 0.5,
                2 => 1.0,
                3 => 1.5,
                _ => 0.75,
            },
        };
        look
    }

    /// The macro object `key` runs when clicked, as written (`[0]!Macro1`):
    /// a shape's `macro` attribute as it is now (Assign Macro from a macro
    /// changes it), else the one the file gave it.
    pub fn drawing_macro_now(&self, key: DrawingKey) -> Option<String> {
        if let Some(xml) = self.shape_xml(key)
            && let Some(info) = shape_xml_info(&xml)
            && !info.form_control
        {
            return info.macro_attr.filter(|m| !m.trim().is_empty());
        }
        self.drawing_macro(key)
    }

    /// A new object made in Kantor on `sheet` (an index) at `anchor`, one
    /// undo step: a shape (`xml` made from its name, which is
    /// `"{base} {n}"`) or a picture.
    fn push_object(
        &mut self,
        sheet: u32,
        anchor: Anchor,
        base: &str,
        shape: Option<&dyn Fn(&str) -> String>,
        picture: Option<(Arc<Vec<u8>>, String)>,
    ) -> Result<DrawingKey> {
        if sheet as usize >= self.sheet_names().len() {
            bail!("no such sheet");
        }
        let sheet_id = self.sheet_id(sheet);
        let mut id = 0;
        self.own_edit(|e, layout| {
            id = e.charts.iter().map(|c| c.id).max().map_or(1, |m| m + 1);
            let name = format!("{base} {id}");
            e.charts.push(NewChart {
                id,
                sheet_id,
                spec: crate::ChartSpec::placeholder(),
                anchor,
                layout: layout.clone(),
                xml: None,
                picture: picture.map(|(data, ext)| {
                    Arc::new(NewPicture {
                        data,
                        crop: [0.0; 4],
                        ext,
                        name: name.clone(),
                        descr: String::new(),
                        shape: None,
                        outline: None,
                    })
                }),
                shape: shape.map(|make| {
                    Arc::new(NewShape {
                        xml: make(name.as_str()).into(),
                        name: name.clone(),
                        slicer: None,
                        timeline: None,
                        media: Vec::new(),
                    })
                }),
                links: None,
            });
        });
        Ok(DrawingKey {
            sheet_id,
            index: NEW_CHART_BASE + id as usize,
        })
    }

    /// `Shapes.AddShape` / `AddTextbox`: a shape of preset geometry `prst`
    /// (`rect`, `ellipse`, `triangle`...; a text box when `text_box`) named
    /// `"{base} {n}"`, at `anchor` on `sheet` (an index). One undo step.
    pub fn script_add_shape(
        &mut self,
        sheet: u32,
        prst: &str,
        base: &str,
        text_box: bool,
        anchor: Anchor,
    ) -> Result<DrawingKey> {
        if prst.is_empty() || !prst.chars().all(|c| c.is_ascii_alphanumeric()) {
            bail!("unknown shape geometry {prst:?}");
        }
        let prst = prst.to_owned();
        let make = move |name: &str| {
            let xml = if text_box {
                ShapePreset::TextBox.xml(name)
            } else {
                ShapePreset::Rectangle.xml(name)
            };
            xml.replacen("prst=\"rect\"", &format!("prst=\"{prst}\""), 1)
        };
        self.push_object(sheet, anchor, base, Some(&make), None)
    }

    /// `Shapes.AddLine`: a straight line across `anchor`'s box, flipped
    /// across (`flip_h`) or down (`flip_v`) so it runs between the given
    /// ends. One undo step.
    pub fn script_add_line(
        &mut self,
        sheet: u32,
        anchor: Anchor,
        flip_h: bool,
        flip_v: bool,
    ) -> Result<DrawingKey> {
        let make = move |name: &str| {
            let xml = ShapePreset::Line.xml(name);
            let mut flips = String::new();
            if flip_h {
                flips.push_str(" flipH=\"1\"");
            }
            if flip_v {
                flips.push_str(" flipV=\"1\"");
            }
            xml.replacen("<a:xfrm>", &format!("<a:xfrm{flips}>"), 1)
        };
        self.push_object(sheet, anchor, "Straight Connector", Some(&make), None)
    }

    /// `Shapes.AddPicture` / `Pictures.Insert`: picture `data` (an image
    /// file's bytes; `ext` the media part's extension, `png`) at `anchor`
    /// on `sheet` (an index), named `Picture n`. One undo step.
    pub fn script_add_picture(
        &mut self,
        sheet: u32,
        data: Vec<u8>,
        ext: &str,
        anchor: Anchor,
    ) -> Result<DrawingKey> {
        let ext = ext.trim_start_matches('.').to_lowercase();
        if !matches!(
            ext.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "tif" | "tiff" | "emf" | "wmf" | "svg"
        ) {
            bail!("Kantor can't insert a .{ext} picture");
        }
        self.push_object(sheet, anchor, "Picture", None, Some((Arc::new(data), ext)))
    }

    /// Set attribute `attr` (`name`, `descr`, `hidden`, `title`) of object
    /// `key`'s `cNvPr` to `value` (`None`: removed). One undo step. Works on
    /// shapes (their element), and on the name and alternative text of
    /// pictures made in Kantor.
    pub fn script_set_cnvpr(
        &mut self,
        key: DrawingKey,
        attr: &str,
        value: Option<&str>,
    ) -> Result<()> {
        if key.index >= NEW_CHART_BASE {
            let c = self
                .new_chart(key)
                .ok_or_else(|| anyhow!("this object can't be changed"))?;
            if let Some(s) = &c.shape {
                if s.slicer.is_some() || s.timeline.is_some() {
                    bail!("Kantor can't change this slicer that way");
                }
                let xml: Arc<str> = with_cnvpr_attr(&s.xml, attr, value)?.into();
                let name = if attr == "name" {
                    value.unwrap_or_default().to_owned()
                } else {
                    s.name.clone()
                };
                let id = c.id;
                self.own_edit(|e, _| {
                    if let Some(c) = e
                        .charts
                        .iter_mut()
                        .find(|c| c.id == id && c.sheet_id == key.sheet_id)
                        && let Some(s) = &c.shape
                    {
                        c.shape = Some(Arc::new(NewShape {
                            xml,
                            name,
                            slicer: None,
                            timeline: None,
                            media: s.media.clone(),
                        }));
                    }
                });
                return Ok(());
            }
            if let Some(p) = &c.picture {
                if !matches!(attr, "name" | "descr") {
                    bail!("Kantor can't change that on a picture");
                }
                let v = value.unwrap_or_default().to_owned();
                let (name, descr) = if attr == "name" {
                    (v, p.descr.clone())
                } else {
                    (p.name.clone(), v)
                };
                let id = c.id;
                self.own_edit(|e, _| {
                    if let Some(c) = e
                        .charts
                        .iter_mut()
                        .find(|c| c.id == id && c.sheet_id == key.sheet_id)
                        && let Some(p) = &c.picture
                    {
                        c.picture = Some(Arc::new(NewPicture {
                            data: p.data.clone(),
                            crop: p.crop,
                            ext: p.ext.clone(),
                            name,
                            descr,
                            shape: p.shape.clone(),
                            outline: p.outline.clone(),
                        }));
                    }
                });
                return Ok(());
            }
            bail!("Kantor can't change that on a chart");
        }
        let xml = self
            .shape_xml(key)
            .ok_or_else(|| anyhow!("Kantor can change that only on shapes"))?;
        let new = with_cnvpr_attr(&xml, attr, value)?;
        self.set_shape_xml(key, &new)
    }

    /// Assign macro `name` (as written, `[0]!Macro1`; empty: none) to
    /// shape `key` (its `macro` attribute). One undo step.
    pub fn script_set_macro(&mut self, key: DrawingKey, name: &str) -> Result<()> {
        let xml = self
            .shape_xml(key)
            .ok_or_else(|| anyhow!("Kantor can assign macros only to shapes"))?;
        let doc = Document::parse(&xml).map_err(|e| anyhow!("shape XML: {e}"))?;
        let root = doc.root_element();
        if root.tag_name().name() == "grpSp" {
            bail!("Kantor can't assign a macro to a group");
        }
        let new = with_attr(&xml, root, "macro", Some(name));
        self.set_shape_xml(key, &new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cnvpr_attributes_are_set_and_removed() {
        let xml = ShapePreset::Rectangle.xml("Rectangle 1");
        let named = with_cnvpr_attr(&xml, "name", Some("Go & back")).unwrap();
        assert!(named.contains("name=\"Go &amp; back\""));
        let hidden = with_cnvpr_attr(&named, "hidden", Some("1")).unwrap();
        let info = shape_xml_info(&hidden).unwrap();
        assert_eq!(info.name.as_deref(), Some("Go & back"));
        assert!(info.hidden);
        let shown = with_cnvpr_attr(&hidden, "hidden", None).unwrap();
        assert_eq!(shown, named);
        assert_eq!(info.prst.as_deref(), Some("rect"));
        assert_eq!(info.macro_attr.as_deref(), Some(""));
        let doc = Document::parse(&xml).unwrap();
        let m = with_attr(&xml, doc.root_element(), "macro", Some("[0]!Go"));
        assert_eq!(
            shape_xml_info(&m).unwrap().macro_attr.as_deref(),
            Some("[0]!Go")
        );
        assert!(
            shape_xml_info(&ShapePreset::TextBox.xml("T"))
                .unwrap()
                .text_box
        );
    }
}
