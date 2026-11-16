//! Slicers and timelines drawn as DrawingML shapes (a group of rectangles
//! with text), laid out by `sheets_engine::slicer`, so the shape renderer
//! draws them on screen as it draws any shape.

use sheets_engine::slicer::{
    SlicerColors, SlicerView, TimelineView, slicer_layout, timeline_layout,
};

const NS: &str = "xmlns:xdr=\"http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"";

fn emu(px: f32) -> i64 {
    (px.max(0.0) * 9525.0).round() as i64
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// `text` cut to what fits `width` px at `size` hundredths of a point (an
/// estimate: about half the font size per character), with `…`.
fn fit(text: &str, width: f32, size: u32) -> String {
    let char_px = size as f32 / 100.0 * 96.0 / 72.0 * 0.55;
    let max = ((width - 8.0) / char_px).floor().max(1.0) as usize;
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// A text box style for [`Shapes::rect`].
#[derive(Clone, Copy)]
struct Text<'a> {
    text: &'a str,
    color: u32,
    size: u32,
    bold: bool,
    align: &'a str,
}

struct Shapes {
    out: String,
    id: u32,
}

impl Shapes {
    fn new() -> Self {
        Shapes {
            out: String::new(),
            id: 2,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn rect(
        &mut self,
        [x, y, w, h]: [f32; 4],
        prst: &str,
        fill: Option<u32>,
        line: Option<u32>,
        text: Option<Text>,
    ) {
        let id = self.id;
        self.id += 1;
        let fill = match fill {
            Some(c) => format!("<a:solidFill><a:srgbClr val=\"{c:06X}\"/></a:solidFill>"),
            None => "<a:noFill/>".to_owned(),
        };
        let line = match line {
            Some(c) => format!(
                "<a:ln w=\"9525\"><a:solidFill><a:srgbClr val=\"{c:06X}\"/></a:solidFill></a:ln>"
            ),
            None => "<a:ln><a:noFill/></a:ln>".to_owned(),
        };
        let adj = if prst == "roundRect" {
            "<a:gd name=\"adj\" fmla=\"val 10000\"/>"
        } else {
            ""
        };
        let body = match text {
            Some(t) => format!(
                "<xdr:txBody><a:bodyPr wrap=\"none\" lIns=\"45720\" tIns=\"0\" rIns=\"45720\" bIns=\"0\" anchor=\"ctr\"/><a:lstStyle/><a:p><a:pPr algn=\"{}\"/><a:r><a:rPr lang=\"en-US\" sz=\"{}\" b=\"{}\"><a:solidFill><a:srgbClr val=\"{:06X}\"/></a:solidFill></a:rPr><a:t>{}</a:t></a:r></a:p></xdr:txBody>",
                t.align,
                t.size,
                u8::from(t.bold),
                t.color,
                esc(&fit(t.text, w, t.size))
            ),
            None => String::new(),
        };
        self.out.push_str(&format!(
            "<xdr:sp><xdr:nvSpPr><xdr:cNvPr id=\"{id}\" name=\"s{id}\"/><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr><a:xfrm><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm><a:prstGeom prst=\"{prst}\"><a:avLst>{adj}</a:avLst></a:prstGeom>{fill}{line}</xdr:spPr>{body}</xdr:sp>",
            emu(x),
            emu(y),
            emu(w),
            emu(h)
        ));
    }

    fn group(self, w: f32, h: f32) -> String {
        let (cx, cy) = (emu(w), emu(h));
        format!(
            "<xdr:grpSp {NS}><xdr:nvGrpSpPr><xdr:cNvPr id=\"1\" name=\"Slicer\"/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr><xdr:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/><a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm></xdr:grpSpPr>{}</xdr:grpSp>",
            self.out
        )
    }
}

fn frame(s: &mut Shapes, c: &SlicerColors, w: f32, h: f32) {
    s.rect(
        [0.0, 0.0, w, h],
        "rect",
        Some(c.background),
        Some(c.border),
        None,
    );
}

/// Slicer `v` drawn into `w` × `h` px: its frame, header (caption,
/// Multi-Select pressed when `multi`, Clear Filter dimmed when nothing is
/// filtered) and its buttons (selected, not selected, greyed without data).
pub fn slicer_xml(v: &SlicerView, w: f32, h: f32, multi: bool) -> String {
    let c = &v.colors;
    let l = slicer_layout(v, w, h);
    let mut s = Shapes::new();
    frame(&mut s, c, w, h);
    if let Some(r) = l.header {
        let icons = l.multi.map_or(0.0, |m| r[0] + r[2] - m[0] + 4.0);
        s.rect(
            [r[0], r[1], (r[2] - icons).max(1.0), r[3]],
            "rect",
            None,
            None,
            Some(Text {
                text: &v.caption,
                color: c.header,
                size: 1100,
                bold: false,
                align: "l",
            }),
        );
        // A line under the header.
        s.rect(
            [r[0], r[1] + r[3] + 1.0, r[2], 0.0],
            "line",
            None,
            Some(c.border),
            None,
        );
    }
    if let Some([x, y, iw, ih]) = l.multi {
        s.rect(
            [x, y, iw, ih],
            "roundRect",
            multi.then_some(c.selected),
            Some(c.button_border),
            None,
        );
        // A list with ticks.
        for k in 0..3 {
            let ly = y + 5.0 + k as f32 * 4.5;
            s.rect([x + 4.0, ly, 3.0, 2.0], "rect", Some(c.header), None, None);
            s.rect(
                [x + 9.0, ly, iw - 13.0, 2.0],
                "rect",
                Some(c.header),
                None,
                None,
            );
        }
    }
    if let Some([x, y, iw, ih]) = l.clear {
        let ink = if v.filtered { c.header } else { c.no_data_text };
        s.rect(
            [x + 3.0, y + 4.0, iw - 9.0, ih - 9.0],
            "funnel",
            None,
            Some(ink),
            None,
        );
        if v.filtered {
            s.rect(
                [x + iw - 9.0, y + ih - 9.0, 8.0, 8.0],
                "mathMultiply",
                Some(0xC00000),
                None,
                None,
            );
        }
    }
    for (it, r) in v.items.iter().zip(&l.buttons) {
        let (fill, text) = match (it.selected, it.no_data) {
            (true, false) => (c.selected, c.selected_text),
            (false, false) => (c.unselected, c.unselected_text),
            (true, true) => (c.no_data, c.no_data_text),
            (false, true) => (c.background, c.no_data_text),
        };
        s.rect(
            *r,
            "roundRect",
            Some(fill),
            Some(c.button_border),
            Some(Text {
                text: &it.label,
                color: text,
                size: 1000,
                bold: false,
                align: "ctr",
            }),
        );
    }
    s.group(w, h)
}

/// Timeline `v` drawn into `w` × `h` px: caption and Clear Filter, the
/// selected dates, the level, the periods (selected ones filled) and the
/// scroll bar when not all of them fit.
pub fn timeline_xml(v: &TimelineView, w: f32, h: f32) -> String {
    let c = &v.colors;
    let l = timeline_layout(v, w, h);
    let mut s = Shapes::new();
    frame(&mut s, c, w, h);
    if let Some(r) = l.header {
        s.rect(
            r,
            "rect",
            None,
            None,
            Some(text(&v.caption, c.header, 1100, "l")),
        );
    }
    if let Some([x, y, iw, ih]) = l.clear {
        let ink = if v.filtered { c.header } else { c.no_data_text };
        s.rect(
            [x + 3.0, y + 4.0, iw - 9.0, ih - 9.0],
            "funnel",
            None,
            Some(ink),
            None,
        );
        if v.filtered {
            s.rect(
                [x + iw - 9.0, y + ih - 9.0, 8.0, 8.0],
                "mathMultiply",
                Some(0xC00000),
                None,
                None,
            );
        }
    }
    if let Some((bar, thumb)) = l.scroll {
        s.rect(bar, "roundRect", Some(0xF2F2F2), None, None);
        s.rect(thumb, "roundRect", Some(0xBFBFBF), None, None);
    }
    if let Some(label) = &v.label {
        s.rect(
            l.label,
            "rect",
            None,
            None,
            Some(text(label, c.header, 900, "l")),
        );
    }
    if let Some(level) = &v.level {
        s.rect(
            l.level,
            "rect",
            None,
            None,
            Some(text(level, c.border_accent(), 800, "r")),
        );
    }
    for (p, r) in v.periods.iter().zip(&l.periods) {
        if let Some(g) = &p.group {
            s.rect(
                [r[0], l.groups[1], 80.0_f32.min(l.groups[2]), l.groups[3]],
                "rect",
                None,
                None,
                Some(text(g, c.header, 800, "l")),
            );
        }
        let label_h = 14.0_f32.min(r[3] * 0.6);
        s.rect(
            [r[0], r[1], r[2], label_h],
            "rect",
            None,
            None,
            Some(text(&p.label, c.unselected_text, 800, "ctr")),
        );
        s.rect(
            [r[0], r[1] + label_h, r[2], (r[3] - label_h).max(4.0)],
            "rect",
            Some(if p.selected { c.selected } else { 0xE7E6E6 }),
            None,
            None,
        );
    }
    s.group(w, h)
}

fn text<'a>(text: &'a str, color: u32, size: u32, align: &'a str) -> Text<'a> {
    Text {
        text,
        color,
        size,
        bold: false,
        align,
    }
}

trait Accent {
    fn border_accent(&self) -> u32;
}

impl Accent for SlicerColors {
    /// The timeline's level text: the style's strong colour.
    fn border_accent(&self) -> u32 {
        if self.selected == 0xFFFFFF {
            self.header
        } else {
            self.selected
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_labels_are_cut() {
        assert_eq!(fit("Apples", 200.0, 1000), "Apples");
        let cut = fit("A very long product name indeed", 60.0, 1000);
        assert!(cut.ends_with('…') && cut.chars().count() < 12, "{cut}");
    }
}
