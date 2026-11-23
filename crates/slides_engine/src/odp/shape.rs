//! OpenDocument drawing objects (`draw:*` on a page or master page) as
//! PresentationML shapes: their place, geometry, fill, outline, shadow
//! and text; pictures, tables and groups.

use std::collections::HashMap;
use std::io::{Cursor, Read as _};

use roxmltree::Node;

use super::geom;
use super::style::{
    NS_DRAW, NS_DRAWOOO, NS_FO, NS_LOEXT, NS_PRES, NS_STYLE, NS_SVG, NS_TABLE, NS_XLINK, Ref,
    Styles, angle_degrees, color, emu, esc, percent,
};
use super::text::{TextOpts, body_pr};

/// The pictures of the package, read when first used.
pub(super) struct Media<'z> {
    zip: zip::ZipArchive<Cursor<&'z [u8]>>,
    by_href: HashMap<String, Option<String>>,
    /// SVG pictures' PNG copies (what Kantor and older Office show), by
    /// the SVG's file name.
    png_of: HashMap<String, String>,
    /// `(file name, bytes)` for `ppt/media/`.
    pub parts: Vec<(String, Vec<u8>)>,
}

impl<'z> Media<'z> {
    pub fn new(zip: zip::ZipArchive<Cursor<&'z [u8]>>) -> Self {
        Self {
            zip,
            by_href: HashMap::new(),
            png_of: HashMap::new(),
            parts: Vec::new(),
        }
    }

    /// A part of the package (at most 256 MB).
    pub fn bytes(&mut self, name: &str) -> Option<Vec<u8>> {
        let name = name.trim_start_matches("./");
        let mut entry = self.zip.by_name(name).ok()?;
        if entry.size() > 256 << 20 {
            return None;
        }
        let mut v = Vec::new();
        entry.read_to_end(&mut v).ok()?;
        Some(v)
    }

    /// The media file name of the picture at `href` in the package, if
    /// it is one Kantor shows.
    pub fn picture(&mut self, href: &str) -> Option<String> {
        if let Some(n) = self.by_href.get(href) {
            return n.clone();
        }
        let name = (|| {
            if href.contains("://") {
                return None;
            }
            let data = self.bytes(href)?;
            let ext = picture_ext(&data)?;
            let png = (ext == "svg")
                .then(|| kantor_metafile::svg_to_png(&data, 1920))
                .flatten();
            let name = format!("image{}.{ext}", self.parts.len() + 1);
            self.parts.push((name.clone(), data));
            if let Some(png) = png {
                let p = format!("image{}.png", self.parts.len() + 1);
                self.parts.push((p.clone(), png));
                self.png_of.insert(name.clone(), p);
            }
            Some(name)
        })();
        self.by_href.insert(href.to_owned(), name.clone());
        name
    }
}

/// The kind of a picture by its first bytes.
pub(super) fn picture_ext(d: &[u8]) -> Option<&'static str> {
    if d.starts_with(b"\x89PNG") {
        Some("png")
    } else if d.starts_with(&[0xFF, 0xD8]) {
        Some("jpeg")
    } else if d.starts_with(b"GIF8") {
        Some("gif")
    } else if let Some(kind) = kantor_metafile::kind(d) {
        Some(match kind {
            kantor_metafile::Kind::Emf => "emf",
            kantor_metafile::Kind::Wmf => "wmf",
            kantor_metafile::Kind::Pict => "pict",
            kantor_metafile::Kind::Bmp => "bmp",
        })
    } else {
        let head = String::from_utf8_lossy(&d[..d.len().min(512)]);
        (head.contains("<svg") || (head.trim_start().starts_with("<?xml") && head.contains("svg")))
            .then_some("svg")
    }
}

/// What was left out.
#[derive(Default)]
pub(super) struct Skipped {
    pub charts: usize,
    pub objects: usize,
    pub media: usize,
    pub shapes: usize,
    pub pictures: usize,
}

/// One part (slide, master, layout) being written.
pub(super) struct Part<'p, 'a, 'i, 'z> {
    pub st: &'p Styles<'a, 'i>,
    pub media: &'p mut Media<'z>,
    pub skipped: &'p mut Skipped,
    /// `(type, target, external)`; the id is `rId{index + 1}`.
    pub rels: Vec<(&'static str, String, bool)>,
    pub next_id: u32,
    /// The master page's name (for `<master>-outlineN` styles).
    pub master: String,
    pub fld: u32,
    /// The slide's number (0 on masters and layouts).
    pub slide_no: usize,
    /// The slide's footer text.
    pub footer: Option<String>,
    /// The slide's date: fixed text, or `Err` for today's date (a field).
    pub date: Option<Result<String, ()>>,
}

/// A written shape and its box (EMU).
pub(super) struct Out {
    pub xml: String,
    pub rect: [i64; 4],
}

/// Where an object is: its box (EMU) and rotation (degrees clockwise).
#[derive(Clone, Copy, Debug)]
pub(super) struct Place {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    pub rot: f64,
}

impl Place {
    pub fn of(n: Node) -> Self {
        let p = |a: &str| n.attribute((NS_SVG, a)).and_then(emu).unwrap_or(0);
        if n.attribute((NS_SVG, "x1")).is_some() {
            let (x1, y1, x2, y2) = (p("x1"), p("y1"), p("x2"), p("y2"));
            return Self {
                x: x1.min(x2),
                y: y1.min(y2),
                w: (x2 - x1).abs(),
                h: (y2 - y1).abs(),
                rot: 0.0,
            };
        }
        let (mut x, mut y, w, h) = (p("x"), p("y"), p("width").max(0), p("height").max(0));
        let mut rot = 0.0;
        if let Some(t) = n.attribute((NS_DRAW, "transform")) {
            // `rotate (a) translate (x y)`: the box turned about its top
            // left corner by `a` radians anticlockwise, then moved.
            let (mut a, mut tx, mut ty) = (0.0f64, None, None);
            let mut rest = t;
            while let Some(open) = rest.find('(') {
                let name = rest[..open].trim();
                let Some(close) = rest[open..].find(')') else {
                    break;
                };
                let args: Vec<&str> = rest[open + 1..open + close]
                    .split(|c: char| c.is_whitespace() || c == ',')
                    .filter(|s| !s.is_empty())
                    .collect();
                match name {
                    "rotate" => a = args.first().and_then(|v| v.parse().ok()).unwrap_or(0.0),
                    "translate" => {
                        tx = args.first().and_then(|v| emu(v));
                        ty = args.get(1).and_then(|v| emu(v)).or(Some(0));
                    }
                    _ => {}
                }
                rest = &rest[open + close + 1..];
            }
            if !a.is_finite() {
                a = 0.0;
            }
            let (tx, ty) = (tx.unwrap_or(x) as f64, ty.unwrap_or(y) as f64);
            let (s, c) = a.sin_cos();
            let (hw, hh) = (w as f64 / 2.0, h as f64 / 2.0);
            // Centre after turning (y down: anticlockwise is
            // (x cos + y sin, -x sin + y cos)).
            let cx = tx + hw * c + hh * s;
            let cy = ty - hw * s + hh * c;
            x = (cx - hw).round() as i64;
            y = (cy - hh).round() as i64;
            rot = (-a.to_degrees()).rem_euclid(360.0);
            if rot < 0.01 || 360.0 - rot < 0.01 {
                rot = 0.0;
            }
        }
        Self { x, y, w, h, rot }
    }

    fn xfrm(&self, tag: &str, flip: (bool, bool)) -> String {
        let rot = (self.rot * 60_000.0).round() as i64;
        let mut a = String::new();
        if rot != 0 {
            a.push_str(&format!(" rot=\"{rot}\""));
        }
        if flip.0 {
            a.push_str(" flipH=\"1\"");
        }
        if flip.1 {
            a.push_str(" flipV=\"1\"");
        }
        format!(
            "<{tag}{a}><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></{tag}>",
            self.x.clamp(-27_273_042_316_900, 27_273_042_316_900),
            self.y.clamp(-27_273_042_316_900, 27_273_042_316_900),
            self.w.clamp(0, 27_273_042_316_900),
            self.h.clamp(0, 27_273_042_316_900)
        )
    }

    fn rect(&self) -> [i64; 4] {
        [self.x, self.y, self.w, self.h]
    }
}

/// The DrawingML preset of a LibreOffice custom shape type.
fn lo_preset(ty: &str) -> Option<&'static str> {
    Some(match ty {
        "rectangle" | "flowchart-process" | "mso-spt1" => "rect",
        "round-rectangle" | "flowchart-alternate-process" => "roundRect",
        "ellipse" | "circle" | "flowchart-connector" => "ellipse",
        "isosceles-triangle" => "triangle",
        "right-triangle" => "rtTriangle",
        "diamond" | "flowchart-decision" => "diamond",
        "parallelogram" | "flowchart-data" => "parallelogram",
        "trapezoid" => "trapezoid",
        "pentagon" => "pentagon",
        "hexagon" | "flowchart-preparation" => "hexagon",
        "octagon" => "octagon",
        "cross" => "plus",
        "star4" => "star4",
        "star5" => "star5",
        "star6" => "star6",
        "star8" => "star8",
        "star12" => "star12",
        "star24" => "star24",
        "can" | "flowchart-magnetic-disk" => "can",
        "cube" => "cube",
        "heart" => "heart",
        "smiley" => "smileyFace",
        "sun" => "sun",
        "moon" => "moon",
        "lightning" => "lightningBolt",
        "ring" => "donut",
        "frame" => "frame",
        "right-arrow" => "rightArrow",
        "left-arrow" => "leftArrow",
        "up-arrow" => "upArrow",
        "down-arrow" => "downArrow",
        "left-right-arrow" => "leftRightArrow",
        "up-down-arrow" => "upDownArrow",
        "chevron" => "chevron",
        "pentagon-right" => "homePlate",
        "rectangular-callout" => "wedgeRectCallout",
        "round-rectangular-callout" => "wedgeRoundRectCallout",
        "round-callout" => "wedgeEllipseCallout",
        "cloud-callout" | "cloud" => "cloudCallout",
        "flowchart-terminator" => "flowChartTerminator",
        "flowchart-document" => "flowChartDocument",
        "flowchart-predefined-process" => "flowChartPredefinedProcess",
        "flowchart-manual-input" => "flowChartManualInput",
        "block-arc" => "blockArc",
        "quad-bevel" => "bevel",
        _ => return None,
    })
}

/// `a:prstGeom` of a preset with adjust values `adj` in its `avLst` order.
fn prst_geom(prst: &str, adj: &[i64]) -> String {
    let names: Vec<String> = crate::geom::preset(prst)
        .map(|d| d.av.iter().map(|g| g.name.clone()).collect())
        .unwrap_or_default();
    let mut av = String::new();
    for (name, v) in names.iter().zip(adj) {
        av.push_str(&format!("<a:gd name=\"{}\" fmla=\"val {v}\"/>", esc(name)));
    }
    format!(
        "<a:prstGeom prst=\"{}\"><a:avLst>{av}</a:avLst></a:prstGeom>",
        esc(prst)
    )
}

/// The DrawingML pattern nearest to an ODF hatch.
fn hatch_preset(style: &str, degrees: f64, distance_emu: i64) -> &'static str {
    let a = degrees.rem_euclid(180.0);
    // 0: horizontal, 1: rising (/), 2: vertical, 3: falling (\).
    let dir = ((a + 22.5) / 45.0).floor() as i64 % 4;
    let distance = distance_emu as f64 / 9525.0;
    let close = distance <= 2.5;
    let mid = distance <= 5.0;
    match style {
        "double" | "triple" => match dir {
            0 | 2 if close || mid => "smGrid",
            0 | 2 => "lgGrid",
            _ => "diagCross",
        },
        _ => match (dir, close, mid) {
            (0, true, _) => "narHorz",
            (0, _, true) => "ltHorz",
            (0, ..) => "horz",
            (2, true, _) => "narVert",
            (2, _, true) => "ltVert",
            (2, ..) => "vert",
            (1, true, _) => "dkUpDiag",
            (1, _, true) => "ltUpDiag",
            (1, ..) => "upDiag",
            (_, true, _) => "dkDnDiag",
            (_, _, true) => "ltDnDiag",
            _ => "dnDiag",
        },
    }
}

fn srgb(c: &str, alpha: Option<i64>) -> String {
    match alpha {
        Some(a) if a < 100_000 => {
            format!(
                "<a:srgbClr val=\"{c}\"><a:alpha val=\"{}\"/></a:srgbClr>",
                a.max(0)
            )
        }
        _ => format!("<a:srgbClr val=\"{c}\"/>"),
    }
}

/// A colour scaled by an ODF gradient intensity (`80%`).
fn intensity(c: &str, i: Option<&str>) -> String {
    let k = i.and_then(percent).unwrap_or(1.0).clamp(0.0, 1.0);
    if k >= 1.0 {
        return c.to_owned();
    }
    let v = u32::from_str_radix(c, 16).unwrap_or(0);
    let ch = |s: u32| (((v >> s) & 0xFF) as f64 * k).round() as u32;
    format!("{:02X}{:02X}{:02X}", ch(16), ch(8), ch(0))
}

impl<'a, 'i> Part<'_, 'a, 'i, '_> {
    pub fn rel(&mut self, ty: &'static str, target: String, external: bool) -> String {
        if let Some(i) = self
            .rels
            .iter()
            .position(|(t, g, e)| *t == ty && *g == target && *e == external)
        {
            return format!("rId{}", i + 1);
        }
        self.rels.push((ty, target, external));
        format!("rId{}", self.rels.len())
    }

    /// `a:blip` of media file `name` with `inner` effects: an SVG as its
    /// PNG copy with the SVG beside it (`asvg:svgBlip`), as Office writes.
    fn blip(&mut self, name: &str, inner: &str) -> String {
        if let Some(png) = self.media.png_of.get(name).cloned() {
            let p = self.rel("image", format!("../media/{png}"), false);
            let s = self.rel("image", format!("../media/{name}"), false);
            return format!(
                "<a:blip r:embed=\"{p}\">{inner}<a:extLst><a:ext uri=\"{{96DAC541-7B7A-43D3-8B79-37D633B846F1}}\"><asvg:svgBlip xmlns:asvg=\"http://schemas.microsoft.com/office/drawing/2016/SVG/main\" r:embed=\"{s}\"/></a:ext></a:extLst></a:blip>"
            );
        }
        let id = self.rel("image", format!("../media/{name}"), false);
        format!("<a:blip r:embed=\"{id}\">{inner}</a:blip>")
    }

    fn id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    /// The fill of a style chain's graphic (or drawing page) properties:
    /// `None` when the chain doesn't say.
    pub fn fill(&mut self, chain: &[Ref<'_>], props: &str) -> Option<String> {
        let st = self.st;
        let g = |ns: &str, a: &str| st.prop(chain, props, ns, a);
        let alpha = g(NS_DRAW, "opacity")
            .and_then(percent)
            .map(|o| (o * 100_000.0).round() as i64);
        let kind = g(NS_DRAW, "fill")?;
        Some(match kind {
            "none" => "<a:noFill/>".into(),
            "solid" => {
                let c = g(NS_DRAW, "fill-color")
                    .and_then(color)
                    .unwrap_or_else(|| "FFFFFF".into());
                format!("<a:solidFill>{}</a:solidFill>", srgb(&c, alpha))
            }
            "gradient" => {
                let Some(gr) = g(NS_DRAW, "fill-gradient-name").and_then(|n| st.gradients.get(n))
                else {
                    return Some(
                        g(NS_DRAW, "fill-color")
                            .and_then(color)
                            .map(|c| format!("<a:solidFill>{}</a:solidFill>", srgb(&c, alpha)))
                            .unwrap_or_else(|| "<a:noFill/>".into()),
                    );
                };
                gradient(*gr, alpha)
            }
            "hatch" => {
                let bg = (g(NS_DRAW, "fill-hatch-solid") == Some("true"))
                    .then(|| g(NS_DRAW, "fill-color").and_then(color))
                    .flatten();
                match g(NS_DRAW, "fill-hatch-name").and_then(|n| st.hatches.get(n)) {
                    Some(h) => {
                        let prst = hatch_preset(
                            h.attribute((NS_DRAW, "style")).unwrap_or("single"),
                            h.attribute((NS_DRAW, "rotation"))
                                .and_then(angle_degrees)
                                .unwrap_or(0.0),
                            h.attribute((NS_DRAW, "distance"))
                                .and_then(emu)
                                .unwrap_or(38_100),
                        );
                        let fg = h
                            .attribute((NS_DRAW, "color"))
                            .and_then(color)
                            .unwrap_or_else(|| "000000".into());
                        format!(
                            "<a:pattFill prst=\"{prst}\"><a:fgClr>{}</a:fgClr><a:bgClr>{}</a:bgClr></a:pattFill>",
                            srgb(&fg, alpha),
                            match bg {
                                Some(c) => srgb(&c, alpha),
                                None =>
                                    "<a:srgbClr val=\"FFFFFF\"><a:alpha val=\"0\"/></a:srgbClr>"
                                        .into(),
                            }
                        )
                    }
                    None => match bg {
                        Some(c) => format!("<a:solidFill>{}</a:solidFill>", srgb(&c, alpha)),
                        None => "<a:noFill/>".into(),
                    },
                }
            }
            "bitmap" => {
                let img = g(NS_DRAW, "fill-image-name").and_then(|n| st.images.get(n).copied());
                let href = img.and_then(|i| i.attribute((NS_XLINK, "href")));
                let name = href.and_then(|h| self.media.picture(h));
                match name {
                    Some(name) => {
                        let repeat = g(NS_STYLE, "repeat").unwrap_or("repeat");
                        let mode = if repeat == "repeat" {
                            "<a:tile tx=\"0\" ty=\"0\" sx=\"100000\" sy=\"100000\" flip=\"none\" algn=\"tl\"/>"
                        } else {
                            "<a:stretch><a:fillRect/></a:stretch>"
                        };
                        let a = alpha
                            .filter(|a| *a < 100_000)
                            .map(|a| format!("<a:alphaModFix amt=\"{}\"/>", a.max(0)))
                            .unwrap_or_default();
                        format!(
                            "<a:blipFill rotWithShape=\"1\">{}{mode}</a:blipFill>",
                            self.blip(&name, &a)
                        )
                    }
                    None => {
                        if href.is_some() {
                            self.skipped.pictures += 1;
                        }
                        match g(NS_DRAW, "fill-color").and_then(color) {
                            Some(c) => format!("<a:solidFill>{}</a:solidFill>", srgb(&c, alpha)),
                            None => "<a:noFill/>".into(),
                        }
                    }
                }
            }
            _ => "<a:noFill/>".into(),
        })
    }

    /// The outline (`a:ln`) of a style chain, `None` for no line.
    fn line(&self, chain: &[Ref<'_>], default_on: bool) -> String {
        let st = self.st;
        let g = |ns: &str, a: &str| st.g(chain, ns, a);
        let stroke = g(NS_DRAW, "stroke");
        if stroke == Some("none") || (stroke.is_none() && !default_on) {
            return "<a:ln><a:noFill/></a:ln>".into();
        }
        let width = g(NS_SVG, "stroke-width")
            .and_then(emu)
            .map(|w| if w <= 0 { 9525 } else { w })
            .unwrap_or(9525)
            .clamp(0, 20_116_800);
        let c = g(NS_SVG, "stroke-color")
            .and_then(color)
            .unwrap_or_else(|| "000000".into());
        let alpha = g(NS_SVG, "stroke-opacity")
            .and_then(percent)
            .map(|o| (o * 100_000.0).round() as i64);
        let mut a = format!(" w=\"{width}\"");
        match g(NS_SVG, "stroke-linecap") {
            Some("round") => a.push_str(" cap=\"rnd\""),
            Some("square") => a.push_str(" cap=\"sq\""),
            _ => {}
        }
        let mut s = format!("<a:solidFill>{}</a:solidFill>", srgb(&c, alpha));
        if stroke == Some("dash") {
            let dash = g(NS_DRAW, "stroke-dash").and_then(|n| st.dashes.get(n));
            s.push_str(&format!(
                "<a:prstDash val=\"{}\"/>",
                dash_preset(dash.copied())
            ));
        }
        match g(NS_DRAW, "stroke-linejoin") {
            Some("round") => s.push_str("<a:round/>"),
            Some("bevel") => s.push_str("<a:bevel/>"),
            Some("miter") => s.push_str("<a:miter lim=\"800000\"/>"),
            _ => {}
        }
        for (attr, tag) in [("marker-start", "headEnd"), ("marker-end", "tailEnd")] {
            if let Some(m) = g(NS_DRAW, attr).and_then(|m| self.marker(m)) {
                let w = g(NS_DRAW, &format!("{attr}-width")).and_then(emu);
                let size = match w {
                    Some(w) if w > width * 5 => " w=\"lg\" len=\"lg\"",
                    Some(w) if w < width * 3 => " w=\"sm\" len=\"sm\"",
                    _ => "",
                };
                s.push_str(&format!("<a:{tag} type=\"{m}\"{size}/>"));
            }
        }
        format!("<a:ln{a}>{s}</a:ln>")
    }

    /// An arrowhead of a marker style.
    fn marker(&self, name: &str) -> Option<&'static str> {
        let m = self.st.markers.get(name);
        let label = m
            .and_then(|m| m.attribute((NS_DRAW, "display-name")))
            .unwrap_or(name)
            .to_ascii_lowercase()
            .replace("_20_", " ");
        Some(if label.contains("circle") || label.contains("oval") {
            "oval"
        } else if label.contains("square") || label.contains("diamond") {
            "diamond"
        } else if label.contains("line") && label.contains("arrow") || label.contains("concave") {
            "stealth"
        } else if label.contains("line") && !label.contains("arrow") {
            "arrow"
        } else {
            "triangle"
        })
    }

    /// The shadow of a style chain (`a:effectLst`), or empty.
    fn shadow(&self, chain: &[Ref<'_>]) -> String {
        let st = self.st;
        let g = |ns: &str, a: &str| st.g(chain, ns, a);
        if g(NS_DRAW, "shadow") != Some("visible") {
            return String::new();
        }
        let dx = g(NS_DRAW, "shadow-offset-x").and_then(emu).unwrap_or(0) as f64;
        let dy = g(NS_DRAW, "shadow-offset-y").and_then(emu).unwrap_or(0) as f64;
        let c = g(NS_DRAW, "shadow-color")
            .and_then(color)
            .unwrap_or_else(|| "808080".into());
        let alpha = g(NS_DRAW, "shadow-opacity")
            .and_then(percent)
            .map(|o| (o * 100_000.0).round() as i64);
        let blur = g(NS_LOEXT, "shadow-blur").and_then(emu).unwrap_or(0);
        let dist = dx.hypot(dy).round() as i64;
        let dir = (dy.atan2(dx).to_degrees().rem_euclid(360.0) * 60_000.0).round() as i64;
        format!(
            "<a:effectLst><a:outerShdw blurRad=\"{}\" dist=\"{}\" dir=\"{}\" algn=\"ctr\" rotWithShape=\"0\">{}</a:outerShdw></a:effectLst>",
            blur.clamp(0, 2_147_483_647),
            dist.clamp(0, 2_147_483_647),
            dir.clamp(0, 21_599_999),
            srgb(&c, alpha)
        )
    }

    fn nv(&mut self, n: Node, default: &str) -> String {
        let id = self.id();
        let name = n
            .attribute((NS_DRAW, "name"))
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{default} {}", id - 1));
        let descr = n
            .children()
            .find(|c| c.has_tag_name((NS_SVG, "desc")))
            .and_then(|c| c.text())
            .or_else(|| {
                n.children()
                    .find(|c| c.has_tag_name((NS_SVG, "title")))
                    .and_then(|c| c.text())
            })
            .map(|d| format!(" descr=\"{}\"", esc(d)))
            .unwrap_or_default();
        let hidden = [
            n.attribute((NS_DRAWOOO, "display")),
            n.attribute((NS_DRAW, "display")),
        ]
        .contains(&Some("none"));
        format!(
            "<p:cNvPr id=\"{id}\" name=\"{}\"{descr}{}/>",
            esc(&name),
            if hidden { " hidden=\"1\"" } else { "" }
        )
    }

    /// The style chain of a drawing object: its text style (paragraph
    /// family) first, then its graphic or presentation style.
    pub fn chain_of(n: Node<'a, 'i>) -> Vec<Ref<'a>> {
        let mut chain = Vec::new();
        if let Some(t) = n.attribute((NS_DRAW, "text-style-name")) {
            chain.push(("paragraph", t));
        }
        if let Some(s) = n.attribute((NS_PRES, "style-name")) {
            chain.push(("presentation", s));
        }
        if let Some(s) = n.attribute((NS_DRAW, "style-name")) {
            chain.push(("graphic", s));
        }
        chain
    }

    /// Objects of a page, in order.
    pub fn shapes(&mut self, parent: Node<'a, 'i>, ph: &mut dyn FnMut(Node) -> Phx) -> Vec<Out> {
        let mut out = Vec::new();
        for c in parent.children().filter(|c| c.is_element()) {
            if let Some(o) = self.object(c, ph) {
                out.push(o);
            }
        }
        out
    }

    /// One drawing object; `ph` says how a presentation object becomes a
    /// placeholder.
    pub fn object(&mut self, n: Node<'a, 'i>, ph: &mut dyn FnMut(Node) -> Phx) -> Option<Out> {
        let tag = n.tag_name();
        if tag.namespace() != Some(NS_DRAW) {
            return None;
        }
        match tag.name() {
            "a" => {
                let c = n
                    .children()
                    .find(|c| c.is_element() && c.tag_name().namespace() == Some(NS_DRAW))?;
                self.object(c, ph)
            }
            "g" => self.group(n, ph),
            "frame" => self.frame(n, ph),
            "custom-shape" | "rect" | "ellipse" | "circle" | "path" | "polygon" | "polyline"
            | "regular-polygon" | "caption" => self.shape(n),
            "line" | "connector" | "measure" => self.connector(n),
            "page-thumbnail" | "layer-set" => None,
            _ => {
                self.skipped.shapes += 1;
                None
            }
        }
    }

    fn group(&mut self, n: Node<'a, 'i>, ph: &mut dyn FnMut(Node) -> Phx) -> Option<Out> {
        let nv = self.nv(n, "Group");
        let kids = self.shapes(n, ph);
        if kids.is_empty() {
            return None;
        }
        let x0 = kids.iter().map(|k| k.rect[0]).min().unwrap_or(0);
        let y0 = kids.iter().map(|k| k.rect[1]).min().unwrap_or(0);
        let x1 = kids
            .iter()
            .map(|k| k.rect[0] + k.rect[2])
            .max()
            .unwrap_or(0);
        let y1 = kids
            .iter()
            .map(|k| k.rect[1] + k.rect[3])
            .max()
            .unwrap_or(0);
        let (w, h) = ((x1 - x0).max(0), (y1 - y0).max(0));
        let body: String = kids.into_iter().map(|k| k.xml).collect();
        Some(Out {
            xml: format!(
                "<p:grpSp><p:nvGrpSpPr>{nv}<p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x=\"{x0}\" y=\"{y0}\"/><a:ext cx=\"{w}\" cy=\"{h}\"/><a:chOff x=\"{x0}\" y=\"{y0}\"/><a:chExt cx=\"{w}\" cy=\"{h}\"/></a:xfrm></p:grpSpPr>{body}</p:grpSp>"
            ),
            rect: [x0, y0, w, h],
        })
    }

    /// `p:sp` with geometry `geom`, the object's fill, outline, shadow and
    /// text.
    #[allow(clippy::too_many_arguments)]
    fn sp(
        &mut self,
        n: Node<'a, 'i>,
        place: Place,
        flip: (bool, bool),
        geom: &str,
        chain: &[Ref<'a>],
        text_in: Option<Node<'a, 'i>>,
        tx_box: bool,
        ph: Option<&Phx>,
        default_line: bool,
        open: bool,
    ) -> Out {
        let nv = self.nv(n, if tx_box { "TextBox" } else { "Shape" });
        let fill = self
            .fill(chain, "graphic-properties")
            .unwrap_or_else(|| "<a:noFill/>".into());
        let fill = if open { "<a:noFill/>".into() } else { fill };
        let ln = self.line(chain, default_line);
        let fx = self.shadow(chain);
        let outline = ph.is_some_and(|p| p.outline);
        let paras = match text_in {
            Some(t) => self.paragraphs(
                t,
                &TextOpts {
                    base: chain,
                    outline,
                    notes: false,
                },
            ),
            None => Vec::new(),
        };
        let (ph_xml, locks) = match ph {
            Some(p) => (
                format!("<p:nvPr>{}</p:nvPr>", p.ph),
                "<p:cNvSpPr><a:spLocks noGrp=\"1\"/></p:cNvSpPr>".to_owned(),
            ),
            None => (
                "<p:nvPr/>".into(),
                if tx_box {
                    "<p:cNvSpPr txBox=\"1\"/>".into()
                } else {
                    "<p:cNvSpPr/>".into()
                },
            ),
        };
        let tx = if paras.is_empty() && ph.is_none() && !tx_box {
            // A shape keeps a text body so text can be typed into it.
            format!(
                "<p:txBody>{}<a:lstStyle/><a:p><a:endParaRPr lang=\"en-US\"/></a:p></p:txBody>",
                body_pr(self.st, chain, false)
            )
        } else if paras.is_empty() {
            format!(
                "<p:txBody>{}<a:lstStyle/><a:p><a:endParaRPr lang=\"en-US\"/></a:p></p:txBody>",
                body_pr(self.st, chain, false)
            )
        } else {
            format!(
                "<p:txBody>{}<a:lstStyle/>{}</p:txBody>",
                body_pr(self.st, chain, false),
                paras.concat()
            )
        };
        Out {
            xml: format!(
                "<p:sp><p:nvSpPr>{nv}{locks}{ph_xml}</p:nvSpPr><p:spPr>{}{geom}{fill}{ln}{fx}</p:spPr>{tx}</p:sp>",
                place.xfrm("a:xfrm", flip)
            ),
            rect: place.rect(),
        }
    }

    /// A shape element.
    fn shape(&mut self, n: Node<'a, 'i>) -> Option<Out> {
        let tag = n.tag_name().name();
        let place = Place::of(n);
        let chain = Self::chain_of(n);
        let st = self.st;
        let mut flip = (false, false);
        let geom = match tag {
            "path" | "polygon" | "polyline" => {
                let vb = geom::view_box(n);
                let g = match tag {
                    "path" => n
                        .attribute((NS_SVG, "d"))
                        .and_then(|d| geom::svg_path_geom(d, vb.unwrap_or([0.0, 0.0, 1.0, 1.0]))),
                    _ => n
                        .attribute((NS_DRAW, "points"))
                        .and_then(|p| geom::points_geom(p, vb?, tag == "polygon")),
                };
                let Some(g) = g else {
                    self.skipped.shapes += 1;
                    return None;
                };
                g
            }
            "regular-polygon" => {
                let corners = n
                    .attribute((NS_DRAW, "corners"))
                    .and_then(|c| c.parse::<usize>().ok())
                    .unwrap_or(3);
                let sharp = n
                    .attribute((NS_DRAW, "sharpness"))
                    .and_then(|v| v.trim_end_matches('%').parse::<f64>().ok());
                let concave = n.attribute((NS_DRAW, "concave")) == Some("true");
                geom::regular_polygon_geom(corners, concave.then(|| sharp.unwrap_or(0.0)))?
            }
            "caption" => prst_geom("wedgeRectCallout", &[]),
            "rect" => {
                let r = n
                    .attribute((NS_DRAW, "corner-radius"))
                    .and_then(emu)
                    .unwrap_or(0);
                let m = place.w.min(place.h);
                if r > 0 && m > 0 {
                    prst_geom("roundRect", &[(r * 100_000 / m).clamp(0, 50_000)])
                } else {
                    prst_geom("rect", &[])
                }
            }
            "ellipse" | "circle" => {
                let kind = n.attribute((NS_DRAW, "kind")).unwrap_or("full");
                let ang = |a: &str| n.attribute((NS_DRAW, a)).and_then(angle_degrees_plain);
                match (kind, ang("start-angle"), ang("end-angle")) {
                    ("full", ..) | (_, None, _) | (_, _, None) => prst_geom("ellipse", &[]),
                    (k, Some(s), Some(e)) => {
                        let d = |v: f64| ((360.0 - v).rem_euclid(360.0) * 60_000.0).round() as i64;
                        let prst = match k {
                            "section" => "pie",
                            "cut" => "chord",
                            _ => "arc",
                        };
                        prst_geom(prst, &[d(e), d(s)])
                    }
                }
            }
            _ => {
                let eg = n
                    .children()
                    .find(|c| c.has_tag_name((NS_DRAW, "enhanced-geometry")));
                if let Some(g) = eg {
                    let f = |a: &str| g.attribute((NS_DRAW, a)) == Some("true");
                    flip = (f("mirror-horizontal"), f("mirror-vertical"));
                }
                let ty = eg
                    .and_then(|g| g.attribute((NS_DRAW, "type")))
                    .unwrap_or("non-primitive");
                let own = || eg.and_then(geom::enhanced_geom);
                let ooxml = ty
                    .strip_prefix("ooxml-")
                    .filter(|p| crate::geom::preset(p).is_some());
                match ooxml {
                    Some(p) => {
                        let adj: Vec<i64> = eg
                            .and_then(|g| g.attribute((NS_DRAW, "modifiers")))
                            .unwrap_or_default()
                            .split_whitespace()
                            .filter_map(|v| v.parse::<f64>().ok())
                            .filter(|v| v.is_finite())
                            .map(|v| v.round() as i64)
                            .collect();
                        prst_geom(p, &adj)
                    }
                    None => match lo_preset(ty) {
                        Some(p)
                            if eg
                                .and_then(|g| g.attribute((NS_DRAW, "modifiers")))
                                .is_none_or(|m| m.trim().is_empty())
                                || own().is_none() =>
                        {
                            prst_geom(p, &[])
                        }
                        _ => match own() {
                            Some(g) => g,
                            None => match lo_preset(ty) {
                                Some(p) => prst_geom(p, &[]),
                                None => {
                                    self.skipped.shapes += 1;
                                    return None;
                                }
                            },
                        },
                    },
                }
            }
        };
        let _ = st;
        let open = tag == "polyline" || (tag == "path" && !geom.contains("<a:close/>"));
        Some(self.sp(
            n,
            place,
            flip,
            &geom,
            &chain,
            Some(n),
            false,
            None,
            true,
            open,
        ))
    }

    /// A line, connector or dimension line.
    fn connector(&mut self, n: Node<'a, 'i>) -> Option<Out> {
        let p = |a: &str| n.attribute((NS_SVG, a)).and_then(emu);
        let (Some(x1), Some(y1), Some(x2), Some(y2)) = (p("x1"), p("y1"), p("x2"), p("y2")) else {
            self.skipped.shapes += 1;
            return None;
        };
        let chain = Self::chain_of(n);
        let place = Place {
            x: x1.min(x2),
            y: y1.min(y2),
            w: (x2 - x1).abs(),
            h: (y2 - y1).abs(),
            rot: Place::of(n).rot,
        };
        // A bent or curved connector: its path, in page 1/100 mm.
        let bent = n.attribute((NS_DRAW, "type")).is_some_and(|t| t != "line");
        if bent
            && let Some(d) = n.attribute((NS_SVG, "d"))
            && let Some(g) = geom::svg_path_geom(
                d,
                [
                    place.x as f64 / 360.0,
                    place.y as f64 / 360.0,
                    (place.w as f64 / 360.0).max(1.0),
                    (place.h as f64 / 360.0).max(1.0),
                ],
            )
        {
            let g = if g.contains("fill=\"none\"") {
                g
            } else {
                g.replace("<a:path ", "<a:path fill=\"none\" ")
            };
            let nv = self.nv(n, "Connector");
            let ln = self.line(&chain, true);
            return Some(Out {
                xml: format!(
                    "<p:sp><p:nvSpPr>{nv}<p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr>{}{g}<a:noFill/>{ln}</p:spPr></p:sp>",
                    place.xfrm("a:xfrm", (false, false))
                ),
                rect: place.rect(),
            });
        }
        let nv = self.nv(n, "Connector");
        let ln = self.line(&chain, true);
        let fx = self.shadow(&chain);
        Some(Out {
            xml: format!(
                "<p:cxnSp><p:nvCxnSpPr>{nv}<p:cNvCxnSpPr/><p:nvPr/></p:nvCxnSpPr><p:spPr>{}{}{ln}{fx}</p:spPr></p:cxnSp>",
                place.xfrm("a:xfrm", (x2 < x1, y2 < y1)),
                prst_geom("line", &[])
            ),
            rect: place.rect(),
        })
    }

    /// A `draw:frame`: text box, picture, table, embedded object.
    fn frame(&mut self, n: Node<'a, 'i>, ph: &mut dyn FnMut(Node) -> Phx) -> Option<Out> {
        let place = Place::of(n);
        let chain = Self::chain_of(n);
        let first = n.children().find(|c| {
            c.is_element() && matches!(c.tag_name().namespace(), Some(NS_DRAW | NS_TABLE))
        })?;
        let phx = ph(n);
        if phx.skip {
            return None;
        }
        let ph_some = (!phx.ph.is_empty()).then_some(&phx);
        let tag = first.tag_name();
        match (tag.namespace(), tag.name()) {
            (Some(NS_DRAW), "text-box") => {
                let out = self.sp(
                    n,
                    place,
                    (false, false),
                    &prst_geom("rect", &[]),
                    &chain,
                    Some(first),
                    ph_some.is_none(),
                    ph_some,
                    false,
                    false,
                );
                if ph_some.is_none() && !out.xml.contains("<a:r>") && !out.xml.contains("<a:fld") {
                    // An empty text frame with nothing to draw.
                    let visible = self
                        .st
                        .g(&chain, NS_DRAW, "fill")
                        .is_some_and(|f| f != "none")
                        || self
                            .st
                            .g(&chain, NS_DRAW, "stroke")
                            .is_some_and(|f| f != "none");
                    if !visible {
                        return None;
                    }
                }
                Some(out)
            }
            (Some(NS_DRAW), "image") => self.picture(n, place, &chain, phx.ph.as_str()),
            (Some(NS_TABLE), "table") => self.table(n, first, place),
            (Some(NS_DRAW), "object" | "object-ole") => {
                let href = first.attribute((NS_XLINK, "href")).unwrap_or_default();
                let is_chart = self
                    .media
                    .bytes(&format!(
                        "{}/content.xml",
                        href.trim_start_matches("./").trim_end_matches('/')
                    ))
                    .is_some_and(|b| {
                        let head = &b[..b.len().min(4096)];
                        String::from_utf8_lossy(head).contains("chart:chart")
                            || b.windows(11).any(|w| w == b"chart:chart")
                    });
                // The object's replacement picture, when Kantor can show it.
                let pic = n
                    .children()
                    .filter(|c| c.has_tag_name((NS_DRAW, "image")))
                    .find(|img| {
                        img.attribute((NS_XLINK, "href"))
                            .is_some_and(|h| self.media.picture(h).is_some())
                    });
                if let Some(img) = pic {
                    let _ = img;
                    return self.picture(n, place, &chain, "");
                }
                if is_chart {
                    self.skipped.charts += 1;
                } else {
                    self.skipped.objects += 1;
                }
                None
            }
            (Some(NS_DRAW), "plugin") => {
                // A video or sound: its preview picture, if any.
                self.skipped.media += 1;
                let preview = first
                    .children()
                    .chain(n.children())
                    .find(|c| c.has_tag_name((NS_DRAW, "image")))?;
                let before = self.skipped.pictures;
                let out = self.picture(preview.parent()?, place, &chain, "");
                self.skipped.pictures = before;
                out
            }
            _ => {
                self.skipped.shapes += 1;
                None
            }
        }
    }

    /// A picture frame: the first `draw:image` Kantor shows (LibreOffice
    /// may give an SVG first and a PNG after it).
    fn picture(
        &mut self,
        n: Node<'a, 'i>,
        place: Place,
        chain: &[Ref<'a>],
        ph: &str,
    ) -> Option<Out> {
        let imgs: Vec<Node> = n
            .children()
            .filter(|c| c.has_tag_name((NS_DRAW, "image")))
            .collect();
        let mut name = None;
        let mut svg = None;
        for img in &imgs {
            let Some(href) = img.attribute((NS_XLINK, "href")) else {
                continue;
            };
            match self.media.picture(href) {
                Some(m) if m.ends_with(".svg") => svg = svg.or(Some(m)),
                Some(m) => {
                    name = Some(m);
                    break;
                }
                None => {}
            }
        }
        let Some(name) = name.or(svg) else {
            self.skipped.pictures += 1;
            return None;
        };
        let nv = self.nv(n, "Picture");
        let st = self.st;
        let mirror = st.g(chain, NS_STYLE, "mirror").unwrap_or("none");
        let flip = (mirror.contains("horizontal"), mirror.contains("vertical"));
        let ln = self.line(chain, false);
        let fx = self.shadow(chain);
        let alpha = st
            .g(chain, NS_DRAW, "image-opacity")
            .and_then(percent)
            .map(|o| (o * 100_000.0).round() as i64)
            .filter(|a| *a < 100_000)
            .map(|a| format!("<a:alphaModFix amt=\"{}\"/>", a.max(0)))
            .unwrap_or_default();
        let blip = self.blip(&name, &alpha);
        let nvpr = if ph.is_empty() {
            "<p:nvPr/>".to_owned()
        } else {
            format!("<p:nvPr>{ph}</p:nvPr>")
        };
        Some(Out {
            xml: format!(
                "<p:pic><p:nvPicPr>{nv}<p:cNvPicPr><a:picLocks noChangeAspect=\"1\"/></p:cNvPicPr>{nvpr}</p:nvPicPr>\
                 <p:blipFill>{blip}<a:stretch><a:fillRect/></a:stretch></p:blipFill>\
                 <p:spPr>{}{}{ln}{fx}</p:spPr></p:pic>",
                place.xfrm("a:xfrm", flip),
                prst_geom("rect", &[])
            ),
            rect: place.rect(),
        })
    }

    /// A table (`table:table` in a frame) as `a:tbl`.
    fn table(&mut self, frame: Node<'a, 'i>, t: Node<'a, 'i>, place: Place) -> Option<Out> {
        let st = self.st;
        let template = t
            .attribute((NS_TABLE, "template-name"))
            .and_then(|n| st.templates.get(n).copied());
        let flag = |a: &str| t.attribute((NS_TABLE, a)) == Some("true");
        // Columns (repeats expanded) and their default cell styles.
        let mut cols: Vec<(Option<i64>, Option<&'a str>)> = Vec::new();
        let mut col_nodes: Vec<Node> = Vec::new();
        for c in t
            .descendants()
            .filter(|c| c.has_tag_name((NS_TABLE, "table-column")))
        {
            col_nodes.push(c);
        }
        for c in col_nodes {
            let k: usize = c
                .attribute((NS_TABLE, "number-columns-repeated"))
                .and_then(|v| v.parse().ok())
                .unwrap_or(1)
                .clamp(1, 1000);
            let w = c
                .attribute((NS_TABLE, "style-name"))
                .and_then(|s| {
                    st.prop(
                        &[("table-column", s)],
                        "table-column-properties",
                        NS_STYLE,
                        "column-width",
                    )
                })
                .and_then(emu);
            let def = c.attribute((NS_TABLE, "default-cell-style-name"));
            for _ in 0..k {
                if cols.len() < 1000 {
                    cols.push((w, def));
                }
            }
        }
        // Rows, with repeats expanded.
        let mut rows: Vec<Node> = Vec::new();
        for r in t
            .descendants()
            .filter(|c| c.has_tag_name((NS_TABLE, "table-row")))
        {
            let k: usize = r
                .attribute((NS_TABLE, "number-rows-repeated"))
                .and_then(|v| v.parse().ok())
                .unwrap_or(1)
                .clamp(1, 1000);
            for _ in 0..k {
                if rows.len() < 1000 {
                    rows.push(r);
                }
            }
        }
        if rows.is_empty() {
            self.skipped.shapes += 1;
            return None;
        }
        // Cells per row (repeats expanded).
        let grid: Vec<Vec<Node>> = rows
            .iter()
            .map(|r| {
                let mut v = Vec::new();
                for c in r.children().filter(|c| {
                    c.has_tag_name((NS_TABLE, "table-cell"))
                        || c.has_tag_name((NS_TABLE, "covered-table-cell"))
                }) {
                    let k: usize = c
                        .attribute((NS_TABLE, "number-columns-repeated"))
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(1)
                        .clamp(1, 1000);
                    for _ in 0..k {
                        if v.len() < 1000 {
                            v.push(c);
                        }
                    }
                }
                v
            })
            .collect();
        let ncols = grid.iter().map(Vec::len).max().unwrap_or(0).max(cols.len());
        if ncols == 0 {
            self.skipped.shapes += 1;
            return None;
        }
        cols.resize(ncols, (None, None));
        let known: i64 = cols.iter().filter_map(|c| c.0).sum();
        let unknown = cols.iter().filter(|c| c.0.is_none()).count() as i64;
        let rest = if unknown > 0 {
            ((place.w - known).max(0) / unknown).max(360_000)
        } else {
            0
        };
        let mut widths: Vec<i64> = cols.iter().map(|c| c.0.unwrap_or(rest).max(1)).collect();
        let total: i64 = widths.iter().sum();
        if place.w > 0 && total > 0 && (total - place.w).abs() > total / 50 {
            for w in &mut widths {
                *w = (*w as i128 * place.w as i128 / total as i128).max(1) as i64;
            }
        }
        let nrows = rows.len();
        let heights: Vec<i64> = rows
            .iter()
            .map(|r| {
                r.attribute((NS_TABLE, "style-name"))
                    .and_then(|s| {
                        let c = [("table-row", s)];
                        st.prop(&c, "table-row-properties", NS_STYLE, "row-height")
                            .or(st.prop(&c, "table-row-properties", NS_STYLE, "min-row-height"))
                    })
                    .and_then(emu)
                    .unwrap_or((place.h / nrows as i64).max(1))
                    .max(1)
            })
            .collect();
        // Spans: what each covered cell is covered by.
        let mut cover = vec![vec![(false, false); ncols]; nrows];
        for (r, row) in grid.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                if !cell.has_tag_name((NS_TABLE, "table-cell")) {
                    continue;
                }
                let span = |a: &str| {
                    cell.attribute((NS_TABLE, a))
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(1)
                        .max(1)
                };
                let (cs, rs) = (span("number-columns-spanned"), span("number-rows-spanned"));
                for (dr, row) in cover.iter_mut().enumerate().skip(r).take(rs) {
                    for (dc, slot) in row.iter_mut().enumerate().skip(c).take(cs) {
                        if dr == r && dc == c {
                            continue;
                        }
                        *slot = (dc > c, dr > r);
                    }
                }
            }
        }
        let last_r = nrows - 1;
        let last_c = ncols - 1;
        let mut body = String::new();
        for (r, row) in grid.iter().enumerate() {
            body.push_str(&format!("<a:tr h=\"{}\">", heights[r]));
            let row_def = rows[r].attribute((NS_TABLE, "default-cell-style-name"));
            for c in 0..ncols {
                let cell = row.get(c).copied();
                let covered = cell.is_none_or(|x| x.has_tag_name((NS_TABLE, "covered-table-cell")));
                // The template's style for this cell.
                let tpl = template.and_then(|tp| {
                    let pick = |name: &str| {
                        tp.children()
                            .find(|k| k.has_tag_name((NS_TABLE, name)))
                            .and_then(|k| k.attribute((NS_TABLE, "style-name")))
                    };
                    let mut s = None;
                    if flag("use-first-row-styles") && r == 0 {
                        s = pick("first-row");
                    } else if flag("use-last-row-styles") && r == last_r {
                        s = pick("last-row");
                    } else if flag("use-first-column-styles") && c == 0 {
                        s = pick("first-column");
                    } else if flag("use-last-column-styles") && c == last_c {
                        s = pick("last-column");
                    } else if flag("use-banding-rows-styles") && r % 2 == 1 {
                        s = pick("odd-rows");
                    } else if flag("use-banding-columns-styles") && c % 2 == 1 {
                        s = pick("odd-columns");
                    }
                    s.or_else(|| pick("body"))
                });
                let own = cell
                    .and_then(|x| x.attribute((NS_TABLE, "style-name")))
                    .or(row_def)
                    .or(cols[c].1);
                let mut chain: Vec<Ref> = Vec::new();
                if let Some(s) = own {
                    chain.push((st.cell_family(s), s));
                }
                if let Some(s) = tpl {
                    chain.push((st.cell_family(s), s));
                }
                let base = Self::chain_of(frame);
                let mut text_chain = chain.clone();
                text_chain.extend_from_slice(&base);
                let mut attrs = String::new();
                if let Some(x) = cell.filter(|_| !covered) {
                    let span = |a: &str| {
                        x.attribute((NS_TABLE, a))
                            .and_then(|v| v.parse::<usize>().ok())
                            .unwrap_or(1)
                    };
                    let (cs, rs) = (
                        span("number-columns-spanned").min(ncols - c),
                        span("number-rows-spanned").min(nrows - r),
                    );
                    if cs > 1 {
                        attrs.push_str(&format!(" gridSpan=\"{cs}\""));
                    }
                    if rs > 1 {
                        attrs.push_str(&format!(" rowSpan=\"{rs}\""));
                    }
                } else {
                    let (h, v) = cover[r][c];
                    if h {
                        attrs.push_str(" hMerge=\"1\"");
                    }
                    if v {
                        attrs.push_str(" vMerge=\"1\"");
                    }
                    if !h && !v {
                        // A covered cell nothing spans: an empty cell.
                    }
                }
                let paras = match cell.filter(|_| !covered) {
                    Some(x) => self.paragraphs(
                        x,
                        &TextOpts {
                            base: &text_chain,
                            outline: false,
                            notes: false,
                        },
                    ),
                    None => Vec::new(),
                };
                let paras = if paras.is_empty() {
                    let r = super::text::run_props(st, &text_chain);
                    format!("<a:p>{}</a:p>", r.xml("endParaRPr", None))
                } else {
                    paras.concat()
                };
                let tc = self.cell_props(&chain);
                body.push_str(&format!(
                    "<a:tc{attrs}><a:txBody><a:bodyPr/><a:lstStyle/>{paras}</a:txBody>{tc}</a:tc>"
                ));
            }
            body.push_str("</a:tr>");
        }
        let grid_xml: String = widths
            .iter()
            .map(|w| format!("<a:gridCol w=\"{w}\"/>"))
            .collect();
        let nv = self.nv(frame, "Table");
        let tbl_pr = format!(
            "<a:tblPr{}{}/>",
            if flag("use-first-row-styles") {
                " firstRow=\"1\""
            } else {
                ""
            },
            if flag("use-banding-rows-styles") {
                " bandRow=\"1\""
            } else {
                ""
            }
        );
        let h_total: i64 = heights.iter().sum();
        let place = Place {
            w: widths.iter().sum(),
            h: place.h.max(h_total),
            ..place
        };
        Some(Out {
            xml: format!(
                "<p:graphicFrame><p:nvGraphicFramePr>{nv}<p:cNvGraphicFramePr><a:graphicFrameLocks noGrp=\"1\"/></p:cNvGraphicFramePr><p:nvPr/></p:nvGraphicFramePr>{}\
                 <a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/table\"><a:tbl>{tbl_pr}<a:tblGrid>{grid_xml}</a:tblGrid>{body}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>",
                Place { rot: 0.0, ..place }.xfrm("p:xfrm", (false, false))
            ),
            rect: place.rect(),
        })
    }

    /// `a:tcPr` of a cell's styles: margins, anchor, borders, fill.
    fn cell_props(&mut self, chain: &[Ref<'_>]) -> String {
        let st = self.st;
        let g = |ns: &str, a: &str| st.g(chain, ns, a);
        let mut a = String::new();
        for (attr, side) in [
            ("marL", "padding-left"),
            ("marR", "padding-right"),
            ("marT", "padding-top"),
            ("marB", "padding-bottom"),
        ] {
            if let Some(v) = g(NS_FO, side).or(g(NS_FO, "padding")).and_then(emu) {
                a.push_str(&format!(" {attr}=\"{}\"", v.clamp(0, 10_000_000)));
            }
        }
        match g(NS_DRAW, "textarea-vertical-align") {
            Some("middle") => a.push_str(" anchor=\"ctr\""),
            Some("bottom") => a.push_str(" anchor=\"b\""),
            _ => {}
        }
        let mut kids = String::new();
        for (tag, side) in [
            ("lnL", "border-left"),
            ("lnR", "border-right"),
            ("lnT", "border-top"),
            ("lnB", "border-bottom"),
        ] {
            let b = st
                .prop(chain, "paragraph-properties", NS_FO, side)
                .or(st.prop(chain, "paragraph-properties", NS_FO, "border"))
                .or(st.prop(chain, "table-cell-properties", NS_FO, side))
                .or(st.prop(chain, "table-cell-properties", NS_FO, "border"));
            if let Some(b) = b {
                kids.push_str(&border(tag, b));
            }
        }
        if let Some(f) = self.fill(chain, "graphic-properties") {
            kids.push_str(&f);
        } else if let Some(c) = st
            .prop(chain, "table-cell-properties", NS_FO, "background-color")
            .and_then(color)
        {
            kids.push_str(&format!(
                "<a:solidFill><a:srgbClr val=\"{c}\"/></a:solidFill>"
            ));
        }
        if kids.is_empty() {
            format!("<a:tcPr{a}/>")
        } else {
            format!("<a:tcPr{a}>{kids}</a:tcPr>")
        }
    }
}

/// `fo:border` (`0.5pt solid #000000`, `none`) as a cell border line.
fn border(tag: &str, b: &str) -> String {
    let mut width = None;
    let mut col = None;
    let mut style = "solid";
    for t in b.split_whitespace() {
        if let Some(c) = color(t) {
            col = Some(c);
        } else if let Some(w) = emu(t).filter(|_| t.chars().any(|c| c.is_ascii_alphabetic())) {
            width = Some(w);
        } else {
            style = match t {
                "none" | "hidden" => "none",
                "dashed" => "dash",
                "dotted" => "dot",
                "double" => "double",
                _ => style,
            };
        }
    }
    if style == "none" || b.trim() == "none" {
        return format!("<a:{tag} w=\"0\"><a:noFill/></a:{tag}>");
    }
    let dash = match style {
        "dash" => "<a:prstDash val=\"dash\"/>",
        "dot" => "<a:prstDash val=\"sysDot\"/>",
        _ => "",
    };
    let cmpd = if style == "double" {
        " cmpd=\"dbl\""
    } else {
        ""
    };
    format!(
        "<a:{tag} w=\"{}\"{cmpd}><a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill>{dash}</a:{tag}>",
        width.unwrap_or(12_700).clamp(0, 20_116_800),
        col.unwrap_or_else(|| "000000".into())
    )
}

/// A DrawingML gradient of an ODF `draw:gradient`.
fn gradient(gr: Node, alpha: Option<i64>) -> String {
    let at = |a: &str| gr.attribute((NS_DRAW, a));
    let c1 = intensity(
        &at("start-color")
            .and_then(color)
            .unwrap_or_else(|| "000000".into()),
        at("start-intensity"),
    );
    let c2 = intensity(
        &at("end-color")
            .and_then(color)
            .unwrap_or_else(|| "FFFFFF".into()),
        at("end-intensity"),
    );
    let border = at("border")
        .and_then(percent)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let angle = at("angle").and_then(angle_degrees).unwrap_or(0.0);
    let style = at("style").unwrap_or("linear");
    // Multi-colour stops (LibreOffice 7.6+), offsets 0..1 from start.
    let mut stops: Vec<(f64, String)> = gr
        .children()
        .filter(|c| c.tag_name().name() == "gradient-stop")
        .filter_map(|c| {
            let off: f64 = c.attribute((NS_SVG, "offset"))?.trim().parse().ok()?;
            let col = c
                .attribute((NS_LOEXT, "color-value"))
                .or(c.attribute((NS_SVG, "stop-color")))
                .and_then(color)?;
            off.is_finite().then(|| (off.clamp(0.0, 1.0), col))
        })
        .collect();
    if stops.len() < 2 {
        stops = vec![(0.0, c1), (1.0, c2)];
    }
    let gs = |stops: &[(f64, String)]| -> String {
        stops
            .iter()
            .map(|(o, c)| {
                format!(
                    "<a:gs pos=\"{}\">{}</a:gs>",
                    ((o * 100_000.0).round() as i64).clamp(0, 100_000),
                    srgb(c, alpha)
                )
            })
            .collect()
    };
    // Border: the start colour held over that part.
    let scale = |stops: &[(f64, String)], b: f64| -> Vec<(f64, String)> {
        stops
            .iter()
            .map(|(o, c)| (b + o * (1.0 - b), c.clone()))
            .collect()
    };
    match style {
        "axial" => {
            // Start colour at both edges, the end colour in the middle.
            let half: Vec<(f64, String)> = scale(&stops, border)
                .into_iter()
                .map(|(o, c)| (o / 2.0, c))
                .collect();
            let mut all = half.clone();
            for (o, c) in half.iter().rev() {
                all.push((1.0 - o, c.clone()));
            }
            let ang = ((90.0 - angle).rem_euclid(360.0) * 60_000.0).round() as i64;
            format!(
                "<a:gradFill rotWithShape=\"1\"><a:gsLst>{}</a:gsLst><a:lin ang=\"{ang}\" scaled=\"0\"/></a:gradFill>",
                gs(&all)
            )
        }
        "radial" | "ellipsoid" | "square" | "rectangular" => {
            // DrawingML paths run from the centre out: reversed.
            let s: Vec<(f64, String)> = scale(&stops, border)
                .into_iter()
                .rev()
                .map(|(o, c)| (1.0 - o, c))
                .collect();
            let cx = at("cx").and_then(percent).unwrap_or(0.5).clamp(0.0, 1.0);
            let cy = at("cy").and_then(percent).unwrap_or(0.5).clamp(0.0, 1.0);
            let path = if matches!(style, "square" | "rectangular") {
                "rect"
            } else {
                "circle"
            };
            let l = (cx * 100_000.0).round() as i64;
            let t = (cy * 100_000.0).round() as i64;
            format!(
                "<a:gradFill rotWithShape=\"1\"><a:gsLst>{}</a:gsLst><a:path path=\"{path}\"><a:fillToRect l=\"{l}\" t=\"{t}\" r=\"{}\" b=\"{}\"/></a:path></a:gradFill>",
                gs(&s),
                100_000 - l,
                100_000 - t
            )
        }
        _ => {
            // ODF 0°: start colour at the top; DrawingML `lin ang` turns
            // clockwise from left to right.
            let ang = ((90.0 - angle).rem_euclid(360.0) * 60_000.0).round() as i64;
            format!(
                "<a:gradFill rotWithShape=\"1\"><a:gsLst>{}</a:gsLst><a:lin ang=\"{ang}\" scaled=\"0\"/></a:gradFill>",
                gs(&scale(&stops, border))
            )
        }
    }
}

/// The DrawingML dash nearest to a `draw:stroke-dash`.
fn dash_preset(d: Option<Node>) -> &'static str {
    let Some(d) = d else { return "dash" };
    let at = |a: &str| d.attribute((NS_DRAW, a));
    let n = |a: &str| at(a).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    let len = |a: &str| -> f64 {
        at(a).map_or(0.0, |v| {
            percent(v).unwrap_or_else(|| emu(v).map_or(0.0, |e| e as f64 / 12_700.0))
        })
    };
    let (d1, d2) = (n("dots1"), n("dots2"));
    let (l1, l2) = (len("dots1-length"), len("dots2-length"));
    let rel = at("dots1-length").is_some_and(|v| v.ends_with('%'));
    let long = |l: f64| if rel { l > 3.0 } else { l > 0.3 };
    let dot = |l: f64| if rel { l <= 1.01 } else { l <= 0.08 };
    match (d1 > 0, d2 > 0) {
        (true, true) => {
            let (dashes, dots) = if dot(l1) { (d2, d1) } else { (d1, d2) };
            if dots >= 2 && dashes >= 1 {
                "lgDashDotDot"
            } else if long(l1.max(l2)) {
                "lgDashDot"
            } else {
                "dashDot"
            }
        }
        _ => {
            let l = l1.max(l2);
            if dot(l) || l == 0.0 {
                "sysDot"
            } else if long(l) {
                "lgDash"
            } else {
                "dash"
            }
        }
    }
}

/// An ODF angle in plain degrees (`draw:start-angle`), units allowed.
fn angle_degrees_plain(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.ends_with("deg") || s.ends_with("rad") || s.ends_with("grad") {
        angle_degrees(s)
    } else {
        s.parse().ok().filter(|v: &f64| v.is_finite())
    }
}

/// How a presentation object becomes a placeholder.
#[derive(Default)]
pub(super) struct Phx {
    /// `<p:ph …/>`, or empty for a plain shape.
    pub ph: String,
    /// Text levels take the master's outline styles.
    pub outline: bool,
    /// Left out (an empty placeholder Kantor doesn't show).
    pub skip: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borders_and_dashes() {
        let b = border("lnL", "0.48pt solid #ffffff");
        assert!(b.contains("w=\"6096\"") && b.contains("FFFFFF"), "{b}");
        assert!(border("lnT", "none").contains("noFill"));
        assert_eq!(dash_preset(None), "dash");
    }

    #[test]
    fn transform_turns_about_the_corner() {
        let xml = format!(
            "<r xmlns:draw=\"{NS_DRAW}\" xmlns:svg=\"{NS_SVG}\" svg:width=\"4cm\" svg:height=\"4cm\" draw:transform=\"rotate (-0.523598775598299) translate (11.268cm 1.268cm)\"/>"
        );
        let d = roxmltree::Document::parse(&xml).unwrap();
        let p = Place::of(d.root_element());
        assert!((p.rot - 30.0).abs() < 0.01, "{p:?}");
        // python-pptx put it at 10 cm, 2 cm.
        assert!(
            (p.x - 3_600_000).abs() < 2_000 && (p.y - 720_000).abs() < 2_000,
            "{p:?}"
        );
    }
}
