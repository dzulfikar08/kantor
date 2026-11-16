//! Shapes and text boxes on worksheets, as Excel's Insert ▸ Shapes and
//! Insert ▸ Text Box make them, and editing their text.
//!
//! A shape is its element's XML ([`DrawingKind::Shape`]). New ones are
//! objects made in Kantor like inserted charts (`chart_edit::NewChart` with
//! a `shape`): moved, resized and deleted the same way, written into the
//! sheet's drawing on save. Editing the text of a file's shape keeps its
//! new element per history state ([`crate::edits::Edits::shape_texts`]);
//! a save writes it in place of the old one, leaving the rest of the
//! drawing as it was.
//!
//! New text keeps the formatting of the paragraphs it replaces: each line
//! takes the paragraph properties and first run's formatting of the
//! paragraph at its place (the last one past the end).

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use roxmltree::{Document, Node};

use crate::Workbook;
use crate::chart_edit::{NEW_CHART_BASE, NewChart, NewShape};
use crate::drawing::{Anchor, DrawingKey, DrawingKind, EditAs, Marker, NS_XDR};

const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";

/// What Insert ▸ Text Box and Insert ▸ Shapes make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapePreset {
    TextBox,
    Rectangle,
    RoundedRectangle,
    Oval,
    Line,
    Arrow,
    RightArrow,
    Callout,
}

impl ShapePreset {
    /// The Shapes gallery (Text Box has its own command).
    pub const SHAPES: [ShapePreset; 7] = [
        ShapePreset::Rectangle,
        ShapePreset::RoundedRectangle,
        ShapePreset::Oval,
        ShapePreset::Line,
        ShapePreset::Arrow,
        ShapePreset::RightArrow,
        ShapePreset::Callout,
    ];

    /// Excel's name for it (in menus).
    pub fn label(self) -> &'static str {
        match self {
            ShapePreset::TextBox => "Text Box",
            ShapePreset::Rectangle => "Rectangle",
            ShapePreset::RoundedRectangle => "Rectangle: Rounded Corners",
            ShapePreset::Oval => "Oval",
            ShapePreset::Line => "Line",
            ShapePreset::Arrow => "Line Arrow",
            ShapePreset::RightArrow => "Arrow: Right",
            ShapePreset::Callout => "Speech Bubble: Rectangle",
        }
    }

    /// The object's name in the file (`Rectangle 3`).
    fn object_name(self) -> &'static str {
        match self {
            ShapePreset::TextBox => "TextBox",
            ShapePreset::Rectangle => "Rectangle",
            ShapePreset::RoundedRectangle => "Rectangle: Rounded Corners",
            ShapePreset::Oval => "Oval",
            ShapePreset::Line => "Straight Connector",
            ShapePreset::Arrow => "Straight Arrow Connector",
            ShapePreset::RightArrow => "Arrow: Right",
            ShapePreset::Callout => "Speech Bubble: Rectangle",
        }
    }

    fn prst(self) -> &'static str {
        match self {
            ShapePreset::TextBox | ShapePreset::Rectangle => "rect",
            ShapePreset::RoundedRectangle => "roundRect",
            ShapePreset::Oval => "ellipse",
            ShapePreset::Line | ShapePreset::Arrow => "line",
            ShapePreset::RightArrow => "rightArrow",
            ShapePreset::Callout => "wedgeRectCallout",
        }
    }

    /// Lines and arrows (connectors, no text).
    pub fn is_line(self) -> bool {
        matches!(self, ShapePreset::Line | ShapePreset::Arrow)
    }

    /// Its size in cells (columns, rows) when inserted.
    fn cells(self) -> (u32, u32) {
        match self {
            ShapePreset::TextBox => (3, 4),
            ShapePreset::Line | ShapePreset::Arrow => (3, 0),
            ShapePreset::RightArrow => (2, 3),
            _ => (2, 5),
        }
    }

    /// The element Excel writes for a new one (with `cNvPr id="0"`).
    pub fn xml(self, name: &str) -> String {
        let name = esc(name);
        let prst = self.prst();
        let ns = format!("xmlns:xdr=\"{NS_XDR}\" xmlns:a=\"{NS_A}\"");
        let xfrm = "<a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/></a:xfrm>";
        let style = |ln: &str, fill: &str, font: &str| {
            format!(
                "<xdr:style>{ln}{fill}<a:effectRef idx=\"0\"><a:schemeClr val=\"accent1\"/></a:effectRef><a:fontRef idx=\"minor\"><a:schemeClr val=\"{font}\"/></a:fontRef></xdr:style>"
            )
        };
        let body = |anchor: &str| {
            format!(
                "<xdr:txBody><a:bodyPr vertOverflow=\"clip\" horzOverflow=\"clip\" wrap=\"square\" rtlCol=\"0\" anchor=\"{anchor}\"/><a:lstStyle/><a:p><a:pPr algn=\"l\"/><a:endParaRPr lang=\"en-US\" sz=\"1100\"/></a:p></xdr:txBody>"
            )
        };
        match self {
            ShapePreset::TextBox => format!(
                "<xdr:sp {ns} macro=\"\" textlink=\"\"><xdr:nvSpPr><xdr:cNvPr id=\"0\" name=\"{name}\"/><xdr:cNvSpPr txBox=\"1\"/></xdr:nvSpPr><xdr:spPr>{xfrm}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom><a:solidFill><a:schemeClr val=\"lt1\"/></a:solidFill><a:ln w=\"9525\" cmpd=\"sng\"><a:solidFill><a:schemeClr val=\"lt1\"><a:shade val=\"50000\"/></a:schemeClr></a:solidFill></a:ln></xdr:spPr>{}{}</xdr:sp>",
                "<xdr:style><a:lnRef idx=\"0\"><a:scrgbClr r=\"0\" g=\"0\" b=\"0\"/></a:lnRef><a:fillRef idx=\"0\"><a:scrgbClr r=\"0\" g=\"0\" b=\"0\"/></a:fillRef><a:effectRef idx=\"0\"><a:scrgbClr r=\"0\" g=\"0\" b=\"0\"/></a:effectRef><a:fontRef idx=\"minor\"><a:schemeClr val=\"dk1\"/></a:fontRef></xdr:style>",
                body("t"),
            ),
            ShapePreset::Line | ShapePreset::Arrow => {
                let ln = if self == ShapePreset::Arrow {
                    "<a:ln><a:tailEnd type=\"triangle\"/></a:ln>"
                } else {
                    ""
                };
                format!(
                    "<xdr:cxnSp {ns} macro=\"\"><xdr:nvCxnSpPr><xdr:cNvPr id=\"0\" name=\"{name}\"/><xdr:cNvCxnSpPr/></xdr:nvCxnSpPr><xdr:spPr>{xfrm}<a:prstGeom prst=\"{prst}\"><a:avLst/></a:prstGeom>{ln}</xdr:spPr>{}</xdr:cxnSp>",
                    style(
                        "<a:lnRef idx=\"1\"><a:schemeClr val=\"accent1\"/></a:lnRef>",
                        "<a:fillRef idx=\"0\"><a:schemeClr val=\"accent1\"/></a:fillRef>",
                        "tx1"
                    ),
                )
            }
            _ => format!(
                "<xdr:sp {ns} macro=\"\" textlink=\"\"><xdr:nvSpPr><xdr:cNvPr id=\"0\" name=\"{name}\"/><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr>{xfrm}<a:prstGeom prst=\"{prst}\"><a:avLst/></a:prstGeom></xdr:spPr>{}{}</xdr:sp>",
                style(
                    "<a:lnRef idx=\"2\"><a:schemeClr val=\"accent1\"><a:shade val=\"50000\"/></a:schemeClr></a:lnRef>",
                    "<a:fillRef idx=\"1\"><a:schemeClr val=\"accent1\"/></a:fillRef>",
                    "lt1"
                ),
                body("t"),
            ),
        }
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The `txBody` of a shape element.
fn text_body<'a, 'i>(sp: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    sp.children()
        .find(|c| c.is_element() && c.tag_name().name() == "txBody")
}

fn is_a(n: Node, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some(NS_A)
}

/// Whether a shape element can hold text (a shape or text box, not a
/// connector or group).
pub fn takes_text(xml: &str) -> bool {
    Document::parse(xml).is_ok_and(|d| {
        let r = d.root_element();
        r.tag_name().name() == "sp" && r.tag_name().namespace() == Some(NS_XDR)
    })
}

/// A shape element's text: its paragraphs, one per line (line breaks
/// inside a paragraph too).
pub fn plain_text(xml: &str) -> Option<String> {
    let doc = Document::parse(xml).ok()?;
    let Some(tb) = text_body(doc.root_element()) else {
        return Some(String::new());
    };
    let paras: Vec<String> = tb
        .children()
        .filter(|p| is_a(*p, "p"))
        .map(|p| {
            let mut s = String::new();
            for d in p.descendants() {
                if is_a(d, "t") {
                    s.push_str(d.text().unwrap_or_default());
                } else if is_a(d, "br") {
                    s.push('\n');
                }
            }
            s
        })
        .collect();
    Some(paras.join("\n"))
}

/// `xml` (a shape element declaring its namespaces) with its text
/// replaced by `text` (lines become paragraphs), keeping the formatting
/// of the paragraphs it replaces (see the module).
pub fn with_text(xml: &str, text: &str) -> Result<String> {
    let doc = Document::parse(xml).map_err(|e| anyhow!("shape XML: {e}"))?;
    let sp = doc.root_element();
    let a = match sp.lookup_prefix(NS_A) {
        Some("") => String::new(),
        Some(p) => format!("{p}:"),
        None => bail!("this shape has no DrawingML namespace"),
    };
    let text = text.replace("\r\n", "\n");
    let lines: Vec<&str> = text.split('\n').collect();
    // Each existing paragraph's pPr and run properties (as an rPr).
    let templates: Vec<(String, String)> = text_body(sp)
        .map(|tb| {
            tb.children()
                .filter(|p| is_a(*p, "p"))
                .map(|p| {
                    let ppr = p
                        .children()
                        .find(|c| is_a(*c, "pPr"))
                        .map(|c| xml[c.range()].to_owned())
                        .unwrap_or_default();
                    let rpr = p
                        .descendants()
                        .find(|c| is_a(*c, "rPr"))
                        .map(|c| xml[c.range()].to_owned())
                        .or_else(|| {
                            p.children().find(|c| is_a(*c, "endParaRPr")).map(|c| {
                                rename(
                                    &xml[c.range()],
                                    &format!("{a}endParaRPr"),
                                    &format!("{a}rPr"),
                                )
                            })
                        })
                        .unwrap_or_default();
                    (ppr, rpr)
                })
                .collect()
        })
        .unwrap_or_default();
    let mut paras = String::new();
    for (i, line) in lines.iter().enumerate() {
        let (ppr, rpr) = templates
            .get(i)
            .or(templates.last())
            .cloned()
            .unwrap_or_default();
        paras.push_str(&format!("<{a}p>{ppr}"));
        if line.is_empty() {
            let end = rename(&rpr, &format!("{a}rPr"), &format!("{a}endParaRPr"));
            paras.push_str(&end);
        } else {
            paras.push_str(&format!(
                "<{a}r>{rpr}<{a}t>{}</{a}t></{a}r>",
                esc_text(line)
            ));
        }
        paras.push_str(&format!("</{a}p>"));
    }
    let mut out = xml.to_owned();
    match text_body(sp) {
        Some(tb) => {
            let ps: Vec<Node> = tb.children().filter(|p| is_a(*p, "p")).collect();
            let range = match (ps.first(), ps.last()) {
                (Some(f), Some(l)) => f.range().start..l.range().end,
                _ => {
                    // No paragraph: after the body's other children.
                    let at = tb.range().end
                        - xml[tb.range()]
                            .rfind("</")
                            .map_or(0, |i| tb.range().len() - i);
                    at..at
                }
            };
            out.replace_range(range, &paras);
        }
        None => {
            let x = match sp.tag_name().namespace() {
                Some(ns) => match sp.lookup_prefix(ns) {
                    Some("") | None => String::new(),
                    Some(p) => format!("{p}:"),
                },
                None => String::new(),
            };
            let close = xml
                .rfind("</")
                .ok_or_else(|| anyhow!("shape element not closed"))?;
            out.insert_str(
                close,
                &format!(
                    "<{x}txBody><{a}bodyPr rtlCol=\"0\" anchor=\"t\"/><{a}lstStyle/>{paras}</{x}txBody>"
                ),
            );
        }
    }
    Ok(out)
}

fn esc_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// An element's XML with its tag `from` renamed `to` (start and end tags).
fn rename(el: &str, from: &str, to: &str) -> String {
    if el.is_empty() {
        return String::new();
    }
    let mut s = el.replacen(&format!("<{from}"), &format!("<{to}"), 1);
    if let Some(i) = s.rfind(&format!("</{from}>")) {
        s.replace_range(i..i + from.len() + 3, &format!("</{to}>"));
    }
    s
}

impl Workbook {
    /// Insert ▸ Text Box / Insert ▸ Shapes: a new `preset` on `sheet` with
    /// its top left corner at cell `(row, col)`. One undo step. Returns
    /// its key.
    pub fn insert_shape(
        &mut self,
        sheet: u32,
        preset: ShapePreset,
        row: u32,
        col: u32,
    ) -> Result<DrawingKey> {
        let (w, h) = preset.cells();
        let col = col.min(crate::MAX_COLS - 1 - w);
        let row = row.min(crate::MAX_ROWS - 1 - h);
        // A line sits 10 px down its row.
        let off = if h == 0 { 95_250 } else { 0 };
        let anchor = Anchor::TwoCell {
            from: Marker {
                col,
                col_off: 0,
                row,
                row_off: off,
            },
            to: Marker {
                col: col + w,
                col_off: 0,
                row: row + h,
                row_off: off,
            },
            edit_as: EditAs::TwoCell,
        };
        let sheet_id = self.sheet_id(sheet);
        let mut id = 0;
        self.own_edit(|e, layout| {
            id = e.charts.iter().map(|c| c.id).max().map_or(1, |m| m + 1);
            let name = format!("{} {id}", preset.object_name());
            e.charts.push(NewChart {
                id,
                sheet_id,
                spec: crate::ChartSpec::placeholder(),
                anchor,
                layout: layout.clone(),
                xml: None,
                picture: None,
                shape: Some(Arc::new(NewShape {
                    xml: preset.xml(&name).into(),
                    name,
                    slicer: None,
                    timeline: None,
                })),
            });
        });
        Ok(DrawingKey {
            sheet_id,
            index: NEW_CHART_BASE + id as usize,
        })
    }

    /// The element of shape `key` as it is now (`None`: not a shape on a
    /// sheet).
    pub(crate) fn shape_now(&self, key: DrawingKey) -> Option<(Arc<str>, bool)> {
        let sheet = self.sheet_index_of(key.sheet_id)?;
        let d = self.drawings(sheet).into_iter().find(|d| d.key == key)?;
        match &*d.kind {
            DrawingKind::Shape { xml } => Some((xml.clone(), d.editable)),
            _ => None,
        }
    }

    /// The text of shape or text box `key` (`None`: not one whose text can
    /// be edited).
    pub fn shape_text(&self, key: DrawingKey) -> Option<String> {
        let (xml, editable) = self.shape_now(key)?;
        if !editable || !takes_text(&xml) {
            return None;
        }
        plain_text(&xml)
    }

    /// Set the text of shape or text box `key` (lines become paragraphs).
    /// One undo step; nothing when it is the same.
    pub fn set_shape_text(&mut self, key: DrawingKey, text: &str) -> Result<()> {
        let (xml, editable) = self
            .shape_now(key)
            .ok_or_else(|| anyhow!("this object has no text"))?;
        if !editable || !takes_text(&xml) {
            bail!("this object's text can't be edited");
        }
        if plain_text(&xml).as_deref() == Some(text) {
            return Ok(());
        }
        let new = with_text(&xml, text)?;
        self.set_shape_xml(key, &new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_text_keeps_the_paragraph_formatting() {
        let xml = ShapePreset::Rectangle.xml("Rectangle 1");
        assert_eq!(plain_text(&xml).as_deref(), Some(""));
        let one = with_text(&xml, "Hello & <you>\n\nBye").unwrap();
        assert_eq!(plain_text(&one).as_deref(), Some("Hello & <you>\n\nBye"));
        assert!(one.contains("<a:p><a:pPr algn=\"l\"/><a:r><a:rPr lang=\"en-US\" sz=\"1100\"/><a:t>Hello &amp; &lt;you&gt;</a:t></a:r></a:p>"));
        assert!(
            one.contains(
                "<a:p><a:pPr algn=\"l\"/><a:endParaRPr lang=\"en-US\" sz=\"1100\"/></a:p>"
            )
        );
        Document::parse(&one).unwrap();
        // A bold run keeps its bold.
        let bold = one.replacen(
            "<a:rPr lang=\"en-US\" sz=\"1100\"/>",
            "<a:rPr lang=\"en-US\" sz=\"1400\" b=\"1\"/>",
            1,
        );
        let two = with_text(&bold, "New").unwrap();
        assert!(two.contains("<a:rPr lang=\"en-US\" sz=\"1400\" b=\"1\"/><a:t>New</a:t>"));
        // A shape without a text body gets one.
        let line = ShapePreset::Line.xml("Line 1");
        assert!(!takes_text(&line));
        let plain = "<xdr:sp xmlns:xdr=\"http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><xdr:nvSpPr><xdr:cNvPr id=\"2\" name=\"x\"/><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr/></xdr:sp>";
        let t = with_text(plain, "Hi").unwrap();
        assert_eq!(plain_text(&t).as_deref(), Some("Hi"));
        assert!(t.contains("<xdr:txBody>"));
    }
}
