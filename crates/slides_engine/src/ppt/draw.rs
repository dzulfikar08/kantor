//! Shapes ([MS-ODRAW] OfficeArt drawings) as PresentationML shape XML.

use std::collections::HashMap;

use super::rec::{Cur, Rec, rt};
use super::text::{self, Level, Styles, Text, tx};
use super::{Ctx, Doc, MU_EMU, Media, esc};

/// Shape properties (OfficeArtFOPT): value and complex data per property id.
#[derive(Clone, Debug, Default)]
pub(super) struct Props<'a> {
    map: HashMap<u16, (u32, &'a [u8])>,
}

impl<'a> Props<'a> {
    pub fn read(&mut self, r: &Rec<'a>) {
        let n = r.inst as usize;
        let mut c = r.cur();
        let mut complex = Cur::new(r.data.get(n.saturating_mul(6)..).unwrap_or(&[]));
        for _ in 0..n {
            let (Some(id), Some(v)) = (c.u16(), c.u32()) else {
                break;
            };
            let data = if id & 0x8000 != 0 {
                complex.bytes(v as usize).unwrap_or_else(|| complex.rest())
            } else {
                &[]
            };
            self.map.insert(id & 0x3FFF, (v, data));
        }
    }

    /// Properties of `self` over `base`.
    pub fn over(&self, base: &Props<'a>) -> Props<'a> {
        let mut map = base.map.clone();
        for (k, v) in &self.map {
            map.insert(*k, *v);
        }
        Props { map }
    }

    pub fn get(&self, id: u16) -> Option<u32> {
        self.map.get(&id).map(|v| v.0)
    }

    pub fn data(&self, id: u16) -> Option<&'a [u8]> {
        self.map.get(&id).map(|v| v.1).filter(|d| !d.is_empty())
    }

    /// A boolean of a boolean-set property: value bit `bit`, used if bit
    /// `bit + 16` is set.
    pub fn flag(&self, id: u16, bit: u32) -> Option<bool> {
        let v = self.get(id)?;
        (v & (1 << (bit + 16)) != 0).then_some(v & (1 << bit) != 0)
    }
}

/// A shape or group read from an OfficeArtSpContainer/SpgrContainer.
#[derive(Clone, Debug, Default)]
pub(super) struct Shape<'a> {
    pub kind: u16,
    pub spid: u32,
    pub flags: u32,
    pub props: Props<'a>,
    /// Slide anchor, master units: left, top, right, bottom.
    pub client: Option<[i32; 4]>,
    /// Anchor in the parent group's coordinates.
    pub child: Option<[i32; 4]>,
    /// A group's own coordinate space.
    pub group: Option<[i32; 4]>,
    /// Placeholder type (PlaceholderEnum) and position.
    pub ph: Option<(u8, i32)>,
    pub text: Option<Text>,
    pub outline_ref: Option<i32>,
    pub kids: Vec<Shape<'a>>,
    pub is_group: bool,
}

pub(super) const F_FLIP_H: u32 = 0x40;
pub(super) const F_FLIP_V: u32 = 0x80;
pub(super) const F_HAVE_MASTER: u32 = 0x20;
pub(super) const F_BACKGROUND: u32 = 0x400;
pub(super) const F_DELETED: u32 = 0x8;
pub(super) const F_OLE: u32 = 0x10;

fn rect4(c: &mut Cur) -> Option<[i32; 4]> {
    Some([c.i32()?, c.i32()?, c.i32()?, c.i32()?])
}

pub(super) fn read_sp<'a>(r: &Rec<'a>) -> Shape<'a> {
    let mut s = Shape::default();
    for c in r.children() {
        match c.ty {
            rt::FSP => {
                s.kind = c.inst;
                let mut cur = c.cur();
                s.spid = cur.u32().unwrap_or(0);
                s.flags = cur.u32().unwrap_or(0);
            }
            rt::FSPGR => s.group = rect4(&mut c.cur()),
            rt::FOPT | rt::SECONDARY_FOPT | rt::TERTIARY_FOPT => s.props.read(&c),
            rt::CHILD_ANCHOR => s.child = rect4(&mut c.cur()),
            rt::CLIENT_ANCHOR => {
                let mut cur = c.cur();
                // SmallRectStruct / RectStruct: top, left, right, bottom.
                let r = if c.data.len() >= 16 {
                    rect4(&mut cur)
                } else {
                    (|| {
                        Some([
                            cur.i16()? as i32,
                            cur.i16()? as i32,
                            cur.i16()? as i32,
                            cur.i16()? as i32,
                        ])
                    })()
                };
                s.client = r.map(|[t, l, rr, b]| [l, t, rr, b]);
            }
            rt::CLIENT_DATA => {
                if let Some(p) = c.find(rt::PLACEHOLDER) {
                    let mut cur = p.cur();
                    if let (Some(pos), Some(ty)) = (cur.i32(), cur.u8()) {
                        s.ph = Some((ty, pos));
                    }
                }
            }
            rt::CLIENT_TEXTBOX => {
                s.text = Text::read(c.children(), tx::OTHER);
                if let Some(o) = c.child(rt::OUTLINE_TEXT_REF) {
                    s.outline_ref = o.cur().i32();
                }
                if s.text.is_none()
                    && let Some(h) = c.child(rt::TEXT_HEADER)
                {
                    // Header only: keep the type for an empty text body.
                    s.text = Some(Text {
                        ty: h.cur().u32().unwrap_or(tx::OTHER),
                        ..Text::default()
                    });
                }
            }
            _ => {}
        }
    }
    s
}

pub(super) fn read_group<'a>(r: &Rec<'a>, depth: u32) -> Shape<'a> {
    let mut g = Shape {
        is_group: true,
        ..Shape::default()
    };
    let mut first = true;
    for c in r.children() {
        match c.ty {
            rt::SP_CONTAINER if first => {
                first = false;
                let own = read_sp(&c);
                g.kind = own.kind;
                g.spid = own.spid;
                g.flags = own.flags;
                g.props = own.props;
                g.client = own.client;
                g.child = own.child;
                g.group = own.group;
            }
            rt::SP_CONTAINER => g.kids.push(read_sp(&c)),
            rt::SPGR_CONTAINER if depth < 24 => g.kids.push(read_group(&c, depth + 1)),
            _ => {}
        }
    }
    g
}

/// The shapes of a drawing (DrawingContainer → OfficeArtDgContainer) and
/// its background shape.
pub(super) fn read_drawing<'a>(container: &Rec<'a>) -> (Vec<Shape<'a>>, Option<Shape<'a>>) {
    let Some(dg) = container
        .child(rt::DRAWING)
        .and_then(|d| d.child(rt::DG_CONTAINER))
    else {
        return (Vec::new(), None);
    };
    let mut shapes = Vec::new();
    let mut bg = None;
    for c in dg.children() {
        match c.ty {
            rt::SPGR_CONTAINER => {
                let g = read_group(&c, 0);
                shapes.extend(g.kids);
            }
            rt::SP_CONTAINER => {
                let s = read_sp(&c);
                if s.flags & F_BACKGROUND != 0 {
                    bg = Some(s);
                }
            }
            _ => {}
        }
    }
    shapes.retain(|s| s.flags & F_DELETED == 0);
    (shapes, bg)
}

/// Collect every shape's properties by shape id (for `hspMaster`).
pub(super) fn index_props<'a>(shapes: &[Shape<'a>], out: &mut HashMap<u32, Props<'a>>) {
    for s in shapes {
        out.insert(s.spid, s.props.clone());
        index_props(&s.kids, out);
    }
}

/// The DrawingML preset for an MSOSPT shape type.
pub(super) fn preset(kind: u16) -> Option<&'static str> {
    Some(match kind {
        1 => "rect",
        2 => "roundRect",
        3 => "ellipse",
        4 => "diamond",
        5 => "triangle",
        6 => "rtTriangle",
        7 => "parallelogram",
        8 => "trapezoid",
        9 => "hexagon",
        10 => "octagon",
        11 => "plus",
        12 => "star5",
        13 | 14 => "rightArrow",
        15 => "homePlate",
        16 => "cube",
        17 => "wedgeRoundRectCallout",
        18 => "star16",
        19 => "arc",
        20 => "line",
        21 => "plaque",
        22 => "can",
        23 => "donut",
        32 => "straightConnector1",
        33 => "bentConnector2",
        34 => "bentConnector3",
        35 => "bentConnector4",
        36 => "bentConnector5",
        37 => "curvedConnector2",
        38 => "curvedConnector3",
        39 => "curvedConnector4",
        40 => "curvedConnector5",
        41 => "callout1",
        42 => "callout2",
        43 => "callout3",
        44 => "accentCallout1",
        45 => "accentCallout2",
        46 => "accentCallout3",
        47 => "borderCallout1",
        48 => "borderCallout2",
        49 => "borderCallout3",
        50 => "accentBorderCallout1",
        51 => "accentBorderCallout2",
        52 => "accentBorderCallout3",
        53 => "ribbon",
        54 => "ribbon2",
        55 => "chevron",
        56 => "pentagon",
        57 => "noSmoking",
        58 => "star8",
        59 => "star16",
        60 => "star32",
        61 => "wedgeRectCallout",
        62 => "wedgeRoundRectCallout",
        63 => "wedgeEllipseCallout",
        64 => "wave",
        65 => "foldedCorner",
        66 => "leftArrow",
        67 => "downArrow",
        68 => "upArrow",
        69 => "leftRightArrow",
        70 => "upDownArrow",
        71 => "irregularSeal1",
        72 => "irregularSeal2",
        73 => "lightningBolt",
        74 => "heart",
        76 => "quadArrow",
        77 => "leftArrowCallout",
        78 => "rightArrowCallout",
        79 => "upArrowCallout",
        80 => "downArrowCallout",
        81 => "leftRightArrowCallout",
        82 => "upDownArrowCallout",
        83 => "quadArrowCallout",
        84 => "bevel",
        85 => "leftBracket",
        86 => "rightBracket",
        87 => "leftBrace",
        88 => "rightBrace",
        89 => "leftUpArrow",
        90 => "bentUpArrow",
        91 => "bentArrow",
        92 => "star24",
        93 => "stripedRightArrow",
        94 => "notchedRightArrow",
        95 => "blockArc",
        96 => "smileyFace",
        97 => "verticalScroll",
        98 => "horizontalScroll",
        99 => "circularArrow",
        101 => "uturnArrow",
        102 => "curvedRightArrow",
        103 => "curvedLeftArrow",
        104 => "curvedUpArrow",
        105 => "curvedDownArrow",
        106 => "cloudCallout",
        107 => "ellipseRibbon",
        108 => "ellipseRibbon2",
        109 => "flowChartProcess",
        110 => "flowChartDecision",
        111 => "flowChartInputOutput",
        112 => "flowChartPredefinedProcess",
        113 => "flowChartInternalStorage",
        114 => "flowChartDocument",
        115 => "flowChartMultidocument",
        116 => "flowChartTerminator",
        117 => "flowChartPreparation",
        118 => "flowChartManualInput",
        119 => "flowChartManualOperation",
        120 => "flowChartConnector",
        121 => "flowChartPunchedCard",
        122 => "flowChartPunchedTape",
        123 => "flowChartSummingJunction",
        124 => "flowChartOr",
        125 => "flowChartCollate",
        126 => "flowChartSort",
        127 => "flowChartExtract",
        128 => "flowChartMerge",
        129 => "flowChartOfflineStorage",
        130 => "flowChartOnlineStorage",
        131 => "flowChartMagneticTape",
        132 => "flowChartMagneticDisk",
        133 => "flowChartMagneticDrum",
        134 => "flowChartDisplay",
        135 => "flowChartDelay",
        176 => "flowChartAlternateProcess",
        177 => "flowChartOffpageConnector",
        182 => "leftRightUpArrow",
        183 => "sun",
        184 => "moon",
        185 => "bracketPair",
        186 => "bracePair",
        187 => "star4",
        188 => "doubleWave",
        189..=200 => "actionButtonBlank",
        _ => return None,
    })
}

fn is_line(kind: u16) -> bool {
    kind == 20 || (32..=40).contains(&kind)
}

/// Placeholder (PlaceholderEnum) as `p:ph` attributes, and the text type
/// Kantor gives it.
pub(super) fn ph_attrs(ph: u8, idx: u32) -> Option<(String, u32)> {
    Some(match ph {
        1 | 13 => ("type=\"title\"".into(), tx::TITLE),
        3 | 15 => ("type=\"ctrTitle\"".into(), tx::TITLE),
        17 => ("type=\"title\" orient=\"vert\"".into(), tx::TITLE),
        2 | 14 => (format!("type=\"body\" idx=\"{idx}\""), tx::BODY),
        4 | 16 => (format!("type=\"subTitle\" idx=\"{idx}\""), tx::BODY),
        18 => (
            format!("type=\"body\" orient=\"vert\" idx=\"{idx}\""),
            tx::BODY,
        ),
        6 | 12 => (format!("type=\"body\" idx=\"{idx}\""), tx::BODY),
        5 | 11 => ("type=\"sldImg\"".into(), tx::OTHER),
        7 => (format!("type=\"dt\" sz=\"half\" idx=\"{idx}\""), tx::OTHER),
        8 => (
            format!("type=\"sldNum\" sz=\"quarter\" idx=\"{idx}\""),
            tx::OTHER,
        ),
        9 => (
            format!("type=\"ftr\" sz=\"quarter\" idx=\"{idx}\""),
            tx::OTHER,
        ),
        10 => (
            format!("type=\"hdr\" sz=\"quarter\" idx=\"{idx}\""),
            tx::OTHER,
        ),
        19..=24 => (format!("idx=\"{idx}\""), tx::BODY),
        _ => return None,
    })
}

fn emu(v: f64) -> i64 {
    (v * MU_EMU).round() as i64
}

/// Writes the shapes of one part, collecting its relationships.
pub(super) struct Part<'d, 'a> {
    pub doc: &'d Doc<'a>,
    pub cx: Ctx<'d>,
    pub styles: &'d Styles,
    /// `(type, target)`; the id is `rId{index + 1}`.
    pub rels: Vec<(&'static str, String)>,
    pub media: &'d mut Media,
    pub next_id: u32,
    /// Texts of the slide in SlideListWithText (for OutlineTextRefAtom).
    pub outline: &'d [Text],
    /// Placeholder text bodies use the master's list styles (else each
    /// gets its own).
    pub ph_inherit: bool,
}

impl<'d, 'a> Part<'d, 'a> {
    pub fn rel(&mut self, ty: &'static str, target: String) -> String {
        if let Some(i) = self.rels.iter().position(|(t, g)| *t == ty && *g == target) {
            return format!("rId{}", i + 1);
        }
        self.rels.push((ty, target));
        format!("rId{}", self.rels.len())
    }

    fn id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    fn full_props(&self, s: &Shape<'a>) -> Props<'a> {
        if s.flags & F_HAVE_MASTER != 0
            && let Some(m) = s.props.get(0x301)
            && let Some(base) = self.doc.master_props.get(&m)
        {
            return s.props.over(base);
        }
        s.props.clone()
    }

    /// An OfficeArtCOLORREF as `RRGGBB`.
    fn colorref(&self, v: u32, p: &Props, depth: u32) -> Option<String> {
        let [r, g, b, f] = v.to_le_bytes();
        if f & 0x08 != 0 {
            return self.cx.rgb(super::Color::Scheme(r));
        }
        if f & 0x10 != 0 {
            if depth > 2 {
                return None;
            }
            let other = match r {
                0xF0 | 0xF7 => 0x181,
                0xF1 | 0xF2 => 0x1C0,
                0xF3 => 0x201,
                0xF5 => 0x183,
                0xF6 => 0x1C3,
                _ => return Some("808080".into()),
            };
            return self.colorref(p.get(other)?, p, depth + 1);
        }
        Some(format!("{r:02X}{g:02X}{b:02X}"))
    }

    fn color_prop(&self, p: &Props, id: u16, default: u32) -> Option<String> {
        self.colorref(p.get(id).unwrap_or(default), p, 0)
    }

    fn defaults(&self) -> &Props<'a> {
        &self.doc.dgg_defaults
    }

    /// `<a:...Fill>` for a shape (empty if nothing to say).
    pub fn fill(&mut self, p: &Props<'a>) -> String {
        let p = p.over(self.defaults());
        if p.flag(0x1BF, 4) == Some(false) {
            return "<a:noFill/>".into();
        }
        let alpha = p
            .get(0x182)
            .filter(|&a| a < 0x10000)
            .map(|a| format!("<a:alpha val=\"{}\"/>", a as u64 * 100_000 / 65_536))
            .unwrap_or_default();
        let fore = self
            .color_prop(&p, 0x181, 0xFFFFFF)
            .unwrap_or_else(|| "FFFFFF".into());
        match p.get(0x180).unwrap_or(0) {
            2 | 3 => {
                if let Some(rid) = p.get(0x186).and_then(|b| self.blip(b)) {
                    let tile = if p.get(0x180) == Some(2) {
                        "<a:tile tx=\"0\" ty=\"0\" sx=\"100000\" sy=\"100000\" flip=\"none\" algn=\"tl\"/>"
                    } else {
                        "<a:stretch><a:fillRect/></a:stretch>"
                    };
                    return format!(
                        "<a:blipFill dpi=\"0\" rotWithShape=\"1\"><a:blip r:embed=\"{rid}\"/>{tile}</a:blipFill>"
                    );
                }
                format!("<a:solidFill><a:srgbClr val=\"{fore}\">{alpha}</a:srgbClr></a:solidFill>")
            }
            4..=8 => {
                let back = self
                    .color_prop(&p, 0x183, 0xFFFFFF)
                    .unwrap_or_else(|| "FFFFFF".into());
                let angle = p.get(0x18B).map(|a| (a as i32) >> 16).unwrap_or(0);
                let ang = (angle + 90).rem_euclid(360) as i64 * 60_000;
                let focus = p.get(0x18C).map(|f| f as i32).unwrap_or(0);
                let gs = |pos: u32, c: &str| {
                    format!("<a:gs pos=\"{pos}\"><a:srgbClr val=\"{c}\"/></a:gs>")
                };
                let stops = match focus {
                    f if f.abs() >= 75 => format!("{}{}", gs(0, &back), gs(100_000, &fore)),
                    f if f >= 25 => format!(
                        "{}{}{}",
                        gs(0, &back),
                        gs(50_000, &fore),
                        gs(100_000, &back)
                    ),
                    f if f <= -25 => format!(
                        "{}{}{}",
                        gs(0, &fore),
                        gs(50_000, &back),
                        gs(100_000, &fore)
                    ),
                    _ => format!("{}{}", gs(0, &fore), gs(100_000, &back)),
                };
                format!(
                    "<a:gradFill rotWithShape=\"1\"><a:gsLst>{stops}</a:gsLst><a:lin ang=\"{ang}\" scaled=\"0\"/></a:gradFill>"
                )
            }
            9 => "<a:noFill/>".into(),
            _ => {
                format!("<a:solidFill><a:srgbClr val=\"{fore}\">{alpha}</a:srgbClr></a:solidFill>")
            }
        }
    }

    /// `<a:ln>` for a shape.
    fn line(&self, p: &Props<'a>) -> String {
        let p = p.over(self.defaults());
        if p.flag(0x1FF, 3) == Some(false) {
            return "<a:ln><a:noFill/></a:ln>".into();
        }
        let w = p.get(0x1CB).unwrap_or(9525).min(20_116_800);
        let color = self
            .color_prop(&p, 0x1C0, 0)
            .unwrap_or_else(|| "000000".into());
        let dash = match p.get(0x1CE).unwrap_or(0) {
            1 => "sysDash",
            2 => "sysDot",
            3 => "sysDashDot",
            4 => "sysDashDotDot",
            5 => "dot",
            6 => "dash",
            7 => "lgDash",
            8 => "dashDot",
            9 => "lgDashDot",
            10 => "lgDashDotDot",
            _ => "solid",
        };
        let end = |name: &str, ty: u16, w: u16, l: u16| {
            let t = match p.get(ty).unwrap_or(0) {
                1 => "triangle",
                2 => "stealth",
                3 => "diamond",
                4 => "oval",
                5 => "arrow",
                _ => return String::new(),
            };
            let size = |v: Option<u32>| match v.unwrap_or(1) {
                0 => "sm",
                2 => "lg",
                _ => "med",
            };
            format!(
                "<a:{name} type=\"{t}\" w=\"{}\" len=\"{}\"/>",
                size(p.get(w)),
                size(p.get(l))
            )
        };
        format!(
            "<a:ln w=\"{w}\"><a:solidFill><a:srgbClr val=\"{color}\"/></a:solidFill><a:prstDash val=\"{dash}\"/>{}{}</a:ln>",
            end("headEnd", 0x1D0, 0x1D2, 0x1D3),
            end("tailEnd", 0x1D1, 0x1D4, 0x1D5)
        )
    }

    /// The relationship id of picture `pib` (1-based BStore index).
    pub fn blip(&mut self, pib: u32) -> Option<String> {
        let target = self.media.part_for(self.doc, pib)?;
        Some(self.rel("image", format!("../media/{target}")))
    }

    /// `<p:bg>` for a background shape.
    pub fn background(&mut self, bg: &Shape<'a>) -> String {
        let props = self.full_props(bg);
        let fill = self.fill(&props);
        if fill.is_empty() || fill == "<a:noFill/>" {
            return String::new();
        }
        format!("<p:bg><p:bgPr>{fill}<a:effectLst/></p:bgPr></p:bg>")
    }

    fn xfrm(&self, s: &Shape, p: &Props, top: bool) -> Option<(String, [i64; 4])> {
        let r = if top {
            s.client.or(s.child)
        } else {
            s.child.or(s.client)
        }?;
        let [l, t, rr, b] = r.map(|v| v as f64);
        let (mut x, mut y, mut w, mut h) = (l, t, (rr - l).max(0.0), (b - t).max(0.0));
        let rot = p.get(4).map(|v| v as i32 as f64 / 65536.0).unwrap_or(0.0);
        let norm = rot.rem_euclid(360.0);
        // Anchors of shapes turned near 90° hold the turned bounds.
        if (45.0..135.0).contains(&norm) || (225.0..315.0).contains(&norm) {
            let (cx, cy) = (x + w / 2.0, y + h / 2.0);
            std::mem::swap(&mut w, &mut h);
            x = cx - w / 2.0;
            y = cy - h / 2.0;
        }
        let mut attrs = String::new();
        let rot60k = (norm * 60_000.0).round() as i64 % 21_600_000;
        if rot60k != 0 {
            attrs.push_str(&format!(" rot=\"{rot60k}\""));
        }
        if s.flags & F_FLIP_H != 0 {
            attrs.push_str(" flipH=\"1\"");
        }
        if s.flags & F_FLIP_V != 0 {
            attrs.push_str(" flipV=\"1\"");
        }
        let v = [emu(x), emu(y), emu(w), emu(h)];
        Some((attrs, v))
    }

    fn xfrm_xml(&self, s: &Shape, p: &Props, top: bool) -> ([i64; 4], String) {
        match self.xfrm(s, p, top) {
            Some((attrs, [x, y, w, h])) => (
                [x, y, w, h],
                format!(
                    "<a:xfrm{attrs}><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{w}\" cy=\"{h}\"/></a:xfrm>"
                ),
            ),
            None => ([0; 4], String::new()),
        }
    }

    fn geometry(&self, s: &Shape, p: &Props) -> String {
        if (s.kind == 0 || p.data(0x145).is_some())
            && let Some(g) = custom_geometry(p)
        {
            return g;
        }
        let name = preset(s.kind).unwrap_or("rect");
        let adj = match (s.kind, p.get(0x147)) {
            (2 | 5, Some(v)) => format!(
                "<a:gd name=\"adj\" fmla=\"val {}\"/>",
                (v as i32 as i64 * 100_000 / 21_600).clamp(0, 100_000)
            ),
            _ => String::new(),
        };
        format!("<a:prstGeom prst=\"{name}\"><a:avLst>{adj}</a:avLst></a:prstGeom>")
    }

    fn body_pr(&self, p: &Props, ph_master: Option<&Props>) -> String {
        let get = |id| p.get(id).or_else(|| ph_master.and_then(|m| m.get(id)));
        let mut a = String::new();
        if get(0x85) == Some(2) {
            a.push_str(" wrap=\"none\"");
        }
        for (id, name, def) in [
            (0x81, "lIns", 91440),
            (0x82, "tIns", 45720),
            (0x83, "rIns", 91440),
            (0x84, "bIns", 45720),
        ] {
            let v = get(id)
                .map(|v| v as i32 as i64)
                .unwrap_or(def)
                .clamp(0, 5_000_000);
            a.push_str(&format!(" {name}=\"{v}\""));
        }
        match get(0x88).unwrap_or(0) {
            1 => a.push_str(" vert=\"vert\""),
            2 => a.push_str(" vert=\"vert270\""),
            _ => {}
        }
        let anchor = get(0x87).unwrap_or(0);
        a.push_str(match anchor % 10 {
            1 | 4 | 7 => " anchor=\"ctr\"",
            2 | 5 | 8 => " anchor=\"b\"",
            _ => " anchor=\"t\"",
        });
        if (3..=5).contains(&anchor) {
            a.push_str(" anchorCtr=\"1\"");
        }
        format!("<a:bodyPr rtlCol=\"0\"{a}><a:noAutofit/></a:bodyPr>")
    }

    /// The text body (`<p:txBody>` contents) of `s`; `inherited` is the
    /// text type whose master style the shape gets without a list style.
    fn text_body(&self, s: &Shape, p: &Props, inherited: Option<u32>) -> Option<String> {
        let t = match s.outline_ref {
            Some(i) if i >= 0 => self.outline.get(i as usize).or(s.text.as_ref()),
            _ => s.text.as_ref(),
        }?;
        let ph_master = None;
        let body_pr = self.body_pr(p, ph_master);
        let ty = t.ty;
        let doc_styles = &self.doc.styles;
        let styles = self.styles;
        let style = |lvl: usize| -> Level { styles.resolve(doc_styles, ty, lvl) };
        let lst = if inherited == Some(ty) || ty == tx::NOTES {
            String::new()
        } else {
            let levels: Vec<Level> = (0..5).map(style).collect();
            text::list_style(&self.cx, &levels)
        };
        let paras = if t.chars.is_empty() {
            "<a:p><a:endParaRPr lang=\"en-US\"/></a:p>".to_owned()
        } else {
            text::paragraphs(&self.cx, t, &style)
        };
        Some(format!("{body_pr}<a:lstStyle>{lst}</a:lstStyle>{paras}"))
    }

    /// The XML of a shape (`p:sp`, `p:pic`, `p:grpSp` or `p:graphicFrame`).
    pub fn shape(&mut self, s: &Shape<'a>, top: bool) -> String {
        if s.is_group {
            if s.props.get(0x39F).is_some_and(|v| v & 1 != 0)
                && let Some(t) = super::table::table(self, s, top)
            {
                return t;
            }
            return self.group(s, top);
        }
        let p = self.full_props(s);
        let id = self.id();
        let name = p
            .data(0x380)
            .map(utf16z)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("Shape {}", id - 1));
        let (_, xfrm) = self.xfrm_xml(s, &p, top);
        let pib = p
            .get(0x104)
            .filter(|_| s.kind == 75 || s.flags & F_OLE != 0);
        if let Some(pib) = pib
            && let Some(rid) = self.blip(pib)
        {
            let crop = {
                let f = |id| {
                    p.get(id)
                        .map(|v| v as i32 as i64 * 100_000 / 65_536)
                        .unwrap_or(0)
                };
                let (t, b, l, r) = (f(0x100), f(0x101), f(0x102), f(0x103));
                if t | b | l | r != 0 {
                    format!("<a:srcRect l=\"{l}\" t=\"{t}\" r=\"{r}\" b=\"{b}\"/>")
                } else {
                    String::new()
                }
            };
            let shown = self.media.renderable(pib);
            let line = if shown {
                self.line_only_if_set(&p)
            } else {
                // No way to draw EMF/WMF yet: a thin grey box shows where it is.
                "<a:ln w=\"6350\"><a:solidFill><a:srgbClr val=\"A6A6A6\"/></a:solidFill></a:ln>"
                    .to_owned()
            };
            let ph =
                s.ph.and_then(|(ty, pos)| ph_attrs(ty, ph_idx(pos)))
                    .map(|(a, _)| format!("<p:ph {a}/>"))
                    .unwrap_or_default();
            return format!(
                "<p:pic><p:nvPicPr><p:cNvPr id=\"{id}\" name=\"{}\"/><p:cNvPicPr><a:picLocks noChangeAspect=\"1\"/></p:cNvPicPr><p:nvPr>{ph}</p:nvPr></p:nvPicPr>\
                 <p:blipFill><a:blip r:embed=\"{rid}\"/>{crop}<a:stretch><a:fillRect/></a:stretch></p:blipFill>\
                 <p:spPr>{xfrm}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom>{line}</p:spPr></p:pic>",
                esc(&name)
            );
        }
        let (ph_xml, inherited) = match s.ph.and_then(|(ty, pos)| ph_attrs(ty, ph_idx(pos))) {
            Some((a, t)) => (format!("<p:ph {a}/>"), Some(t)),
            None => (String::new(), Some(tx::OTHER)),
        };
        let inherited = if self.ph_inherit || s.ph.is_none() {
            inherited
        } else {
            None
        };
        let geom = self.geometry(s, &p);
        let (fill, line) = if is_line(s.kind) {
            ("<a:noFill/>".to_owned(), self.line(&p))
        } else {
            (self.fill(&p), self.line(&p))
        };
        let txbox = if s.kind == 202 && s.ph.is_none() {
            " txBox=\"1\""
        } else {
            ""
        };
        let body = self
            .text_body(s, &p, inherited)
            .map(|b| format!("<p:txBody>{b}</p:txBody>"))
            .unwrap_or_default();
        format!(
            "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"{}\"/><p:cNvSpPr{txbox}/><p:nvPr>{ph_xml}</p:nvPr></p:nvSpPr>\
             <p:spPr>{xfrm}{geom}{fill}{line}</p:spPr>{body}</p:sp>",
            esc(&name)
        )
    }

    fn line_only_if_set(&self, p: &Props<'a>) -> String {
        if p.flag(0x1FF, 3) == Some(true) {
            self.line(p)
        } else {
            "<a:ln><a:noFill/></a:ln>".into()
        }
    }

    fn group(&mut self, g: &Shape<'a>, top: bool) -> String {
        let id = self.id();
        let p = g.props.clone();
        let (attrs, [x, y, w, h]) = self.xfrm(g, &p, top).unwrap_or_default();
        let [cl, ct, cr, cb] = g.group.unwrap_or([0, 0, 0, 0]).map(|v| v as f64);
        let (cx, cy, cw, ch) = if cr > cl && cb > ct {
            (emu(cl), emu(ct), emu(cr - cl), emu(cb - ct))
        } else {
            (x, y, w, h)
        };
        let kids: String = g.kids.iter().map(|k| self.shape(k, false)).collect();
        format!(
            "<p:grpSp><p:nvGrpSpPr><p:cNvPr id=\"{id}\" name=\"Group {}\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>\
             <p:grpSpPr><a:xfrm{attrs}><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{w}\" cy=\"{h}\"/>\
             <a:chOff x=\"{cx}\" y=\"{cy}\"/><a:chExt cx=\"{cw}\" cy=\"{ch}\"/></a:xfrm></p:grpSpPr>{kids}</p:grpSp>",
            id - 1
        )
    }

    /// Text body for a table cell (no list style unless the type differs).
    pub fn cell_text(&self, s: &Shape<'a>) -> Option<String> {
        let t = s.text.as_ref()?;
        let ty = t.ty;
        let doc_styles = &self.doc.styles;
        let styles = self.styles;
        let style = |lvl: usize| -> Level { styles.resolve(doc_styles, ty, lvl) };
        if t.chars.is_empty() {
            return None;
        }
        Some(text::paragraphs(&self.cx, t, &style))
    }

    pub fn props_of(&self, s: &Shape<'a>) -> Props<'a> {
        self.full_props(s)
    }

    pub fn solid_fill(&mut self, p: &Props<'a>) -> String {
        self.fill(p)
    }

    pub fn line_color_width(&self, p: &Props<'a>) -> Option<(String, u32)> {
        let p = p.over(self.defaults());
        if p.flag(0x1FF, 3) == Some(false) {
            return None;
        }
        Some((
            self.color_prop(&p, 0x1C0, 0)
                .unwrap_or_else(|| "000000".into()),
            p.get(0x1CB).unwrap_or(9525).min(20_116_800),
        ))
    }

    pub fn next_shape_id(&mut self) -> u32 {
        self.id()
    }
}

/// `idx` for a placeholder from its PlaceholderAtom position.
pub(super) fn ph_idx(pos: i32) -> u32 {
    if pos <= 0 { 1 } else { pos as u32 + 1 }.min(1000)
}

fn utf16z(d: &[u8]) -> String {
    let v: Vec<u16> = d
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|&c| c != 0)
        .collect();
    String::from_utf16_lossy(&v)
}

/// IMsoArray elements: `(count, element size, data)`.
fn mso_array(d: &[u8]) -> Option<(usize, usize, &[u8])> {
    let mut c = Cur::new(d);
    let n = c.u16()? as usize;
    c.skip(2)?;
    let size = match c.u16()? {
        0xFFF0 => 4,
        s => s as usize,
    };
    if size == 0 {
        return None;
    }
    Some((n, size, c.rest()))
}

/// `a:custGeom` from pVertices / pSegmentInfo.
fn custom_geometry(p: &Props) -> Option<String> {
    let (n, size, data) = mso_array(p.data(0x145)?)?;
    let mut pts = Vec::new();
    for i in 0..n {
        let e = data.get(i * size..(i + 1) * size)?;
        let mut c = Cur::new(e);
        let (x, y) = match size {
            4 => (c.i16()? as i64, c.i16()? as i64),
            8 => (c.i32()? as i64, c.i32()? as i64),
            _ => return None,
        };
        pts.push((x, y));
    }
    if pts.is_empty() {
        return None;
    }
    let left = p.get(0x140).map(|v| v as i32 as i64).unwrap_or(0);
    let top = p.get(0x141).map(|v| v as i32 as i64).unwrap_or(0);
    let right = p
        .get(0x142)
        .map(|v| v as i32 as i64)
        .unwrap_or_else(|| pts.iter().map(|p| p.0).max().unwrap_or(21600));
    let bottom = p
        .get(0x143)
        .map(|v| v as i32 as i64)
        .unwrap_or_else(|| pts.iter().map(|p| p.1).max().unwrap_or(21600));
    let (w, h) = ((right - left).max(1), (bottom - top).max(1));
    let pt = |(x, y): (i64, i64)| format!("<a:pt x=\"{}\" y=\"{}\"/>", x - left, y - top);
    let mut path = String::new();
    let mut i = 0usize;
    let segs: Vec<u16> = p
        .data(0x146)
        .and_then(mso_array)
        .map(|(n, size, d)| {
            (0..n)
                .filter_map(|k| {
                    let e = d.get(k * size..k * size + 2)?;
                    Some(u16::from_le_bytes([e[0], e[1]]))
                })
                .collect()
        })
        .unwrap_or_default();
    if segs.is_empty() {
        path.push_str(&format!("<a:moveTo>{}</a:moveTo>", pt(pts[0])));
        for &q in &pts[1..] {
            path.push_str(&format!("<a:lnTo>{}</a:lnTo>", pt(q)));
        }
    } else {
        let mut started = false;
        for seg in segs {
            let kind = seg >> 13;
            let count = (seg & 0x1FFF) as usize;
            match kind {
                0 => {
                    for _ in 0..count.max(1) {
                        let Some(&q) = pts.get(i) else { break };
                        i += 1;
                        if started {
                            path.push_str(&format!("<a:lnTo>{}</a:lnTo>", pt(q)));
                        } else {
                            path.push_str(&format!("<a:moveTo>{}</a:moveTo>", pt(q)));
                            started = true;
                        }
                    }
                }
                1 => {
                    for _ in 0..count.max(1) {
                        let (Some(&a), Some(&b), Some(&c)) =
                            (pts.get(i), pts.get(i + 1), pts.get(i + 2))
                        else {
                            break;
                        };
                        i += 3;
                        path.push_str(&format!(
                            "<a:cubicBezTo>{}{}{}</a:cubicBezTo>",
                            pt(a),
                            pt(b),
                            pt(c)
                        ));
                    }
                }
                2 => {
                    let Some(&q) = pts.get(i) else { break };
                    i += 1;
                    path.push_str(&format!("<a:moveTo>{}</a:moveTo>", pt(q)));
                    started = true;
                }
                3 => path.push_str("<a:close/>"),
                4 => break,
                5 | 6 => i += (seg & 0xFF) as usize,
                _ => {}
            }
        }
    }
    let filled = p.flag(0x1BF, 4) != Some(false);
    let fill_attr = if filled { "" } else { " fill=\"none\"" };
    Some(format!(
        "<a:custGeom><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:rect l=\"l\" t=\"t\" r=\"r\" b=\"b\"/>\
         <a:pathLst><a:path w=\"{w}\" h=\"{h}\"{fill_attr}>{path}</a:path></a:pathLst></a:custGeom>"
    ))
}
