//! Shapes, text boxes and connectors of worksheet drawings (`xdr:sp`,
//! `xdr:cxnSp`, `xdr:grpSp`), read with the same code as slide shapes
//! (their elements are the same DrawingML in another namespace), for Kantor
//! Sheets to draw them. Their text is resolved by
//! [`crate::render::free_shape_text`].

use roxmltree::Document;

use crate::model::Shape;
use crate::shape::{ParseCtx, parse_shape_in};
use crate::theme::ColorCtx;

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
}
