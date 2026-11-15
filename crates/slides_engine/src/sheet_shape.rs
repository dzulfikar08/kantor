//! Shapes, text boxes and connectors of worksheet drawings (`xdr:sp`,
//! `xdr:cxnSp`, `xdr:grpSp`), read with the same code as slide shapes
//! (their elements are the same DrawingML in another namespace), for Kantor
//! Sheets to draw them. Their text is resolved by
//! [`crate::render::free_shape_text`].

use roxmltree::Document;

use crate::Rgb;
use crate::model::{RunProps, Shape, TextPara};
use crate::shape::{ParseCtx, parse_shape_in, parse_text_body, strip_elements, write_paras};
use crate::theme::ColorCtx;

const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";

/// SpreadsheetML drawing namespace.
pub const NS_XDR: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";

/// Office's default theme (colours, Calibri, the format scheme's fill and
/// line styles), for drawings of a workbook that has no theme.
pub fn office_theme() -> String {
    crate::template::theme()
}

/// The shape whose element is the first child of `xml`'s root (a wrapper
/// declaring the namespaces it uses), with colours from `colors`. Picture
/// fills aren't resolved (no relationships). `None`: not XML.
pub fn parse(xml: &str, colors: ColorCtx) -> Option<Shape> {
    let doc = Document::parse(xml).ok()?;
    let el = doc.root_element().first_element_child()?;
    let cx = ParseCtx {
        src: xml,
        part: "",
        rels: &[],
        colors,
    };
    Some(parse_shape_in(el, &cx, None))
}

/// The paragraphs of a shape element's text (`xml`: the element itself,
/// declaring the namespaces it uses), with their runs' formatting, for
/// editing that keeps it. `None`: no text body, text Kantor can't edit
/// (fields, equations), or DrawingML under a prefix other than `a`
/// ([`with_paras`] writes `a:`).
pub fn text_paras(xml: &str) -> Option<Vec<TextPara>> {
    let doc = Document::parse(xml).ok()?;
    let el = doc.root_element();
    if el.lookup_prefix(NS_A) != Some("a") {
        return None;
    }
    let tb = el
        .children()
        .find(|c| c.is_element() && c.tag_name().name() == "txBody")?;
    let theme = crate::theme::Theme::default();
    let map = crate::theme::ClrMap::default();
    let cx = ParseCtx {
        src: xml,
        part: "",
        rels: &[],
        colors: ColorCtx {
            theme: &theme,
            map: &map,
            ph: None,
        },
    };
    let (body, _) = parse_text_body(tb, &cx);
    if body.locked.is_some() {
        return None;
    }
    Some(body.paras)
}

/// `xml` (as for [`text_paras`]) with its paragraphs replaced by `paras`;
/// everything else stays byte for byte. `None` when [`text_paras`] is.
pub fn with_paras(xml: &str, paras: &[TextPara]) -> Option<String> {
    text_paras(xml)?;
    let doc = Document::parse(xml).ok()?;
    let tb = doc
        .root_element()
        .children()
        .find(|c| c.is_element() && c.tag_name().name() == "txBody")?;
    let theme = crate::theme::Theme::default();
    let map = crate::theme::ClrMap::default();
    let cx = ParseCtx {
        src: xml,
        part: "",
        rels: &[],
        colors: ColorCtx {
            theme: &theme,
            map: &map,
            ph: None,
        },
    };
    let (_, range) = parse_text_body(tb, &cx);
    let mut out = xml.to_owned();
    out.replace_range(range, &write_paras(paras));
    Some(out)
}

/// Set a run's typeface (`a:latin`; `None` drops it so the theme's body
/// font applies).
pub fn set_typeface(p: &mut RunProps, face: Option<&str>) {
    p.extra.post = strip_elements(&p.extra.post, &["latin"]);
    p.shown_font = face.map(str::to_owned);
    let Some(face) = face else { return };
    let el = format!("<a:latin typeface=\"{}\"/>", crate::xml::esc_attr(face));
    // Schema order: after the fills, effects, highlight and underline
    // elements, before `ea`, `cs`, `sym`, hyperlinks and `rtl`.
    let post = &p.extra.post;
    let at = [
        "<a:ea",
        "<a:cs",
        "<a:sym",
        "<a:hlinkClick",
        "<a:hlinkMouseOver",
        "<a:rtl",
        "<a:extLst",
    ]
    .iter()
    .filter_map(|t| post.find(t))
    .min()
    .unwrap_or(post.len());
    p.extra.post.insert_str(at, &el);
}

/// Take a run's own colour off (it shows the shape's text colour).
pub fn clear_color(p: &mut RunProps) {
    p.extra.pre = strip_elements(
        &p.extra.pre,
        &[
            "noFill",
            "solidFill",
            "gradFill",
            "blipFill",
            "pattFill",
            "grpFill",
        ],
    );
    p.color = None;
    p.shown_color = None;
}

/// Set a run's colour to `c` (any other fill it had goes).
pub fn set_color(p: &mut RunProps, c: Rgb) {
    p.extra.pre = strip_elements(
        &p.extra.pre,
        &[
            "noFill",
            "solidFill",
            "gradFill",
            "blipFill",
            "pattFill",
            "grpFill",
        ],
    );
    p.color = Some(c);
    p.shown_color = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ShapeKind;
    use crate::theme::{ClrMap, Theme};

    #[test]
    fn reads_a_worksheet_text_box() {
        let theme = Theme::default();
        let map = ClrMap::default();
        let cx = ColorCtx {
            theme: &theme,
            map: &map,
            ph: None,
        };
        let xml = r#"<r xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="2" name="TextBox 1"/><xdr:cNvSpPr txBox="1"/></xdr:nvSpPr><xdr:spPr><a:xfrm rot="5400000"><a:off x="0" y="0"/><a:ext cx="100" cy="50"/></a:xfrm><a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill><a:ln w="25400"><a:solidFill><a:srgbClr val="0000FF"/></a:solidFill></a:ln></xdr:spPr><xdr:txBody><a:bodyPr anchor="ctr"/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" sz="1400"/><a:t>Hello</a:t></a:r></a:p></xdr:txBody></xdr:sp></r>"#;
        let s = parse(xml, cx).unwrap();
        assert_eq!(s.name, "TextBox 1");
        assert!(matches!(s.kind, ShapeKind::Shape { text_box: true, .. }));
        assert_eq!(s.look.fill, Some(crate::Rgb([255, 0, 0])));
        assert_eq!(s.look.line, Some((crate::Rgb([0, 0, 255]), 25400)));
        assert_eq!(s.xfrm.unwrap().rot, 5_400_000);
        let t = s.text.as_ref().unwrap();
        assert_eq!(t.plain_text(), "Hello");
    }

    #[test]
    fn mixed_runs_survive_a_text_edit() {
        let xml = r#"<xdr:sp xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" macro=""><xdr:nvSpPr><xdr:cNvPr id="2" name="T"/><xdr:cNvSpPr txBox="1"/></xdr:nvSpPr><xdr:spPr/><xdr:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" sz="1100" b="1"/><a:t>Bold</a:t></a:r><a:r><a:rPr lang="en-US" sz="1100"/><a:t> plain</a:t></a:r></a:p></xdr:txBody></xdr:sp>"#;
        let mut paras = text_paras(xml).unwrap();
        assert_eq!(paras[0].text, "Bold plain");
        crate::text_ops::replace_changed(&mut paras, "Bold plain text");
        let out = with_paras(xml, &paras).unwrap();
        // The bold run is still bold; the new words take the plain run's.
        assert!(out.contains("<a:t>Bold</a:t>"), "{out}");
        assert!(out.contains("<a:t> plain text</a:t>"), "{out}");
        assert!(out.contains("b=\"1\""));
        assert!(out.starts_with(&xml[..xml.find("<a:p>").unwrap()]));
        // Colour and typeface helpers.
        let mut p = paras[0].runs[0].props.clone();
        set_color(&mut p, Rgb([255, 0, 0]));
        set_typeface(&mut p, Some("Arial"));
        let mut s = String::new();
        crate::shape::write_rpr(&mut s, "a:rPr", &p);
        assert!(s.contains("<a:solidFill><a:srgbClr val=\"FF0000\"/></a:solidFill><a:latin typeface=\"Arial\"/>"), "{s}");
        // Another prefix isn't edited (written as `a:`).
        let other = xml.replace("a:", "d:").replace("xmlns:a=", "xmlns:d=");
        assert!(roxmltree::Document::parse(&other).is_ok());
        assert!(text_paras(&other).is_none());
    }
}
