//! Floating objects on the page: where each one landed (body ones from the
//! layout, header and footer ones placed here), their text boxes laid out,
//! and what they draw as renderer-neutral primitives (paths, a cropped
//! picture, a placeholder box) for the Docs view and the PDF.
//!
//! Shape outlines come from Slides' DrawingML preset geometry
//! ([`slides_engine::geom`], all 187 presets and custom geometry), colours
//! from the document's theme with Slides' colour rules. Outer shadows are
//! drawn blurred ([`crate::blur`]), 3-D extrusions and bevels
//! approximated (the outline repeated along the depth, light and dark
//! edges), and glows (a few fading edges) and
//! soft edges (the shape a little transparent) approximated. Gradients
//! are drawn as gradients (linear; path gradients as radial ones); the
//! screen and the PDF draw them, [`DrawPath::fill`] holds the first colour
//! for anything that can't. Not drawn: picture fills of shapes.

use std::sync::Arc;

use docs_engine::object::EMU_PER_PX;
use docs_engine::{Document, Float, FloatKind};
use docs_layout::{AnchorAt, PageSetup, Rgba};
use roxmltree::Node;
use slides_engine::fill::Dash;
use slides_engine::geom::{Geometry, PathCmd, outline};
use slides_engine::theme::{ClrMap, ColorCtx, Theme};

use crate::pages::StoryLayout;

/// A floating object placed on a page.
#[derive(Clone)]
pub struct FloatItem {
    /// Its anchor: paragraph and byte index in the body, or in header or
    /// footer story `story` (index into [`Document::stories`]).
    pub para: usize,
    pub index: usize,
    pub story: Option<usize>,
    /// `[x, y, w, h]`, page coordinates.
    pub rect: [f32; 4],
    pub z: u32,
    pub behind: bool,
    /// Its text box laid out, with where the layout's (0, 0) goes.
    pub text: Option<(Arc<StoryLayout>, f32, f32)>,
    /// A chart's or SmartArt's drawing (drawn instead of the object's
    /// placeholder; SmartArt's text is in `boxes`).
    pub chart: Option<Arc<crate::chart::ChartDraw>>,
    /// The text of a group's shapes ([`Float::boxes`]), in their order, or
    /// of a SmartArt's shapes (`which`: the shape in its drawing).
    pub boxes: Vec<BoxText>,
}

/// The text of a shape in a group, laid out.
#[derive(Clone)]
pub struct BoxText {
    /// Which of the object's [`Float::boxes`].
    pub which: usize,
    pub layout: Arc<StoryLayout>,
    /// Where the layout's (0, 0) goes, page px.
    pub x: f32,
    pub y: f32,
    /// The shape's box on the page (the text turns about its centre).
    pub rect: [f32; 4],
    /// Degrees clockwise.
    pub turn: f32,
}

/// Where a group's shape's text goes on the page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxArea {
    /// The shape's box.
    pub rect: [f32; 4],
    /// Its text area (geometry's text rectangle less the insets).
    pub area: [f32; 4],
    /// Degrees clockwise (0 when the text stays upright).
    pub turn: f32,
}

impl FloatItem {
    /// The object in `doc`.
    pub fn float<'a>(&self, doc: &'a Document) -> Option<&'a Float> {
        self.doc(doc).float_at(self.para, self.index)
    }

    /// The document its anchor is in (the body or a header or footer).
    pub fn doc<'a>(&self, doc: &'a Document) -> &'a Document {
        match self.story {
            Some(s) => doc.stories().get(s).map_or(doc, |st| &st.doc),
            None => doc,
        }
    }
}

/// A path segment, page coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Move([f32; 2]),
    Line([f32; 2]),
    Quad([f32; 2], [f32; 2]),
    Cubic([f32; 2], [f32; 2], [f32; 2]),
    Close,
}

/// A filled and/or stroked outline.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawPath {
    pub segs: Vec<Seg>,
    pub fill: Option<Rgba>,
    /// Colour, width (px) and dash pattern (px).
    pub stroke: Option<(Rgba, f32, Option<Vec<f32>>)>,
    /// A gradient fill (page coordinates), drawn instead of `fill` (which
    /// holds its first colour) where the renderer can.
    pub gradient: Option<slides_scene::Paint>,
}

/// What a floating object draws.
#[derive(Clone, Debug, PartialEq)]
pub enum FloatPrim {
    Path(DrawPath),
    /// A picture: drawn into `image` (the whole picture), clipped to
    /// `clip` (the object's box, for cropped pictures).
    Picture {
        rel_id: String,
        image: [f32; 4],
        clip: [f32; 4],
        /// Turned clockwise by this many degrees about the box's centre,
        /// and flipped (horizontally, vertically) first.
        rot: f32,
        flip: (bool, bool),
    },
    /// Content Kantor doesn't draw (charts, groups, SmartArt): a box.
    Placeholder([f32; 4], String),
    /// A blurred shadow, drawn as a picture.
    Blurred(crate::blur::Blurred),
}

/// Colours of a document's shapes.
pub struct Painter {
    theme: Theme,
    map: ClrMap,
}

impl Painter {
    pub fn new(doc: &Document) -> Self {
        Self {
            theme: Theme::parse(&doc.theme_xml().unwrap_or_default()),
            map: ClrMap::default(),
        }
    }

    pub(crate) fn ctx(&self) -> ColorCtx<'_> {
        ColorCtx {
            theme: &self.theme,
            map: &self.map,
            ph: None,
        }
    }

    /// What a floating object at `rect` draws, back to front.
    pub fn prims(&self, f: &Float, rect: [f32; 4]) -> Vec<FloatPrim> {
        let mut out = Vec::new();
        let (sp, style) = f.shape_xml();
        let (sp_src, style_src) = (sp.map(wrap), style.map(wrap));
        let sp = sp_src
            .as_deref()
            .and_then(|x| roxmltree::Document::parse(x).ok());
        let style = style_src
            .as_deref()
            .and_then(|x| roxmltree::Document::parse(x).ok());
        // The fragments' own elements, inside their wrappers.
        let sp_root = sp
            .as_ref()
            .and_then(|d| d.root_element().first_element_child());
        let style_root = style
            .as_ref()
            .and_then(|d| d.root_element().first_element_child());
        match &f.kind {
            FloatKind::Picture { rel_id, crop, .. } => {
                let crop = crop.map(|c| c as f32 / 100_000.0);
                self.picture(sp_root, style_root, rel_id, crop, rect, &mut out);
            }
            FloatKind::Shape => {
                let geom_src = f.geometry_xml().map(wrap);
                let geom_doc = geom_src
                    .as_deref()
                    .and_then(|x| roxmltree::Document::parse(x).ok());
                let geom = geom_doc
                    .as_ref()
                    .and_then(|d| d.root_element().first_element_child());
                // At the size it's drawn (a "Resize shape to fit text"
                // shape is as tall as its text).
                let size = if rect[2] > 0.0 || rect[3] > 0.0 {
                    (
                        (rect[2].max(0.0) * EMU_PER_PX) as f64,
                        (rect[3].max(0.0) * EMU_PER_PX) as f64,
                    )
                } else {
                    (f.at.cx.max(0) as f64, f.at.cy.max(0) as f64)
                };
                let k = 1.0 / EMU_PER_PX;
                self.shape(sp_root, style_root, geom, size, rect, (k, k), &mut out);
            }
            FloatKind::Group { canvas } => {
                let src = f.graphic_xml().map(wrap);
                let doc = src
                    .as_deref()
                    .and_then(|x| roxmltree::Document::parse(x).ok());
                let top = doc
                    .as_ref()
                    .and_then(|d| d.root_element().first_element_child())
                    .and_then(|data| data.first_element_child());
                match top {
                    Some(g) => self.group(g, *canvas, f, rect, &mut out),
                    None => out.push(FloatPrim::Placeholder(rect, f.label().to_owned())),
                }
            }
            FloatKind::Chart { .. } | FloatKind::SmartArt { .. } | FloatKind::Other(_) => {
                out.push(FloatPrim::Placeholder(rect, f.label().to_owned()))
            }
        }
        out
    }

    /// A picture in `rect`, cropped (`crop` l, t, r, b as fractions), with
    /// its shadow and outline.
    fn picture(
        &self,
        sp: Option<Node>,
        style: Option<Node>,
        rel_id: &str,
        crop: [f32; 4],
        rect: [f32; 4],
        out: &mut Vec<FloatPrim>,
    ) {
        // The crop insets are of the whole picture: draw it larger and
        // clip it to the box.
        let [l, t, r, b] = crop;
        let (sw, sh) = ((1.0 - l - r).max(0.01), (1.0 - t - b).max(0.01));
        let [x, y, w, h] = rect;
        let (fw, fh) = (w / sw, h / sh);
        let xform = Xform::of(sp, rect, (1.0, 1.0));
        let outline = xform.rect_segs();
        out.extend(self.shadow(sp, std::slice::from_ref(&outline)));
        out.push(FloatPrim::Picture {
            rel_id: rel_id.to_owned(),
            image: [x - l * fw, y - t * fh, fw, fh],
            clip: rect,
            rot: xform.rot,
            flip: (xform.flip_h, xform.flip_v),
        });
        if let Some((color, width, dash)) = self.line(sp, style) {
            out.push(FloatPrim::Path(DrawPath {
                segs: outline,
                fill: None,
                stroke: Some((color, width, dash)),
                gradient: None,
            }));
        }
    }

    /// A shape whose outline is `geom` at `size` (EMU) drawn into `rect`
    /// (`scale`: px per EMU), with its fill, line, arrowheads and shadow.
    #[allow(clippy::too_many_arguments)]
    fn shape(
        &self,
        sp: Option<Node>,
        style: Option<Node>,
        geom: Option<Node>,
        (cx, cy): (f64, f64),
        rect: [f32; 4],
        scale: (f32, f32),
        out: &mut Vec<FloatPrim>,
    ) {
        let xform = Xform::of(sp, rect, scale);
        let geometry = geom
            .and_then(Geometry::from_xml)
            .unwrap_or_else(|| Geometry::preset("rect"));
        let o = outline(&geometry, cx, cy);
        let fill = self.fill(sp, style);
        let gradient = self.gradient(sp, style, rect);
        let line = self.line(sp, style);
        let style_ln = sp.and_then(|s| child(s, "ln"));
        let ls = style_ln.map(|ln| {
            self.ctx()
                .line_style(ln, &slides_engine::fill::LineStyle::default())
        });
        if fill.is_some() || line.is_some() {
            let outlines: Vec<Vec<Seg>> = o
                .paths
                .iter()
                .filter(|p| p.fill.is_filled() || p.stroke)
                .map(|p| p.cmds.iter().map(|c| xform.seg(c)).collect())
                .collect();
            out.extend(self.shadow(sp, &outlines));
            out.extend(self.glow(sp, &outlines));
            out.extend(self.extrusion(sp, &outlines, fill));
        }
        // Soft edges fade the shape out towards its edge: approximated as
        // the whole shape a little transparent, without its outline.
        let soft = self.soft_edge(sp).is_some();
        let faded = |c: Rgba| {
            let [r, g, b, a] = c.0;
            Rgba([r, g, b, (a as f32 * 0.7).round() as u8])
        };
        let fade_stops = |p: slides_scene::Paint| match p {
            slides_scene::Paint::Linear { from, to, stops } => slides_scene::Paint::Linear {
                from,
                to,
                stops: stops
                    .into_iter()
                    .map(|s| slides_scene::Stop {
                        color: faded(s.color),
                        ..s
                    })
                    .collect(),
            },
            slides_scene::Paint::Radial {
                center,
                rx,
                ry,
                stops,
            } => slides_scene::Paint::Radial {
                center,
                rx,
                ry,
                stops: stops
                    .into_iter()
                    .map(|s| slides_scene::Stop {
                        color: faded(s.color),
                        ..s
                    })
                    .collect(),
            },
            slides_scene::Paint::Solid(c) => slides_scene::Paint::Solid(faded(c)),
        };
        for sub in &o.paths {
            let segs: Vec<Seg> = sub.cmds.iter().map(|c| xform.seg(c)).collect();
            let filled = sub.fill.is_filled();
            let mut fill = fill.filter(|_| filled);
            let mut gradient = gradient.clone().filter(|_| filled);
            if soft {
                fill = fill.map(faded);
                gradient = gradient.map(fade_stops);
            }
            out.push(FloatPrim::Path(DrawPath {
                segs,
                fill,
                stroke: line.clone().filter(|_| sub.stroke && !soft),
                gradient,
            }));
        }
        if fill.is_some() {
            let outlines: Vec<Vec<Seg>> = o
                .paths
                .iter()
                .filter(|p| p.fill.is_filled())
                .map(|p| p.cmds.iter().map(|c| xform.seg(c)).collect())
                .collect();
            out.extend(self.bevel(sp, &outlines));
        }
        // Arrowheads on open paths.
        if let (Some(ls), Some((color, width, _))) = (ls, line)
            && let Some(sub) = o.paths.iter().find(|s| s.stroke)
        {
            let pts: Vec<[f32; 2]> = sub
                .flatten(1.0)
                .into_iter()
                .flat_map(|(c, _)| c)
                .map(|p| xform.pt(p))
                .collect();
            if pts.len() >= 2 {
                if ls.head.is_some() {
                    out.push(arrow(pts[1], pts[0], color, width));
                }
                if ls.tail.is_some() {
                    let n = pts.len();
                    out.push(arrow(pts[n - 2], pts[n - 1], color, width));
                }
            }
        }
    }

    /// A group's (`wpg:wgp`) or canvas's (`wpc:wpc`) shapes and pictures,
    /// its child coordinates mapped onto `rect`.
    fn group(&self, g: Node, canvas: bool, f: &Float, rect: [f32; 4], out: &mut Vec<FloatPrim>) {
        let map = if canvas {
            // Children in EMU from the canvas's corner; a resized canvas
            // scales them.
            let kx = rect[2] / f.read.cx.max(1) as f32;
            let ky = rect[3] / f.read.cy.max(1) as f32;
            if let Some(bg) = child(g, "bg")
                && let Some(c) = self.fill(Some(bg), None)
            {
                out.push(FloatPrim::Path(DrawPath {
                    segs: rect_segs(rect),
                    fill: Some(c),
                    stroke: None,
                    gradient: None,
                }));
            }
            ChildMap {
                off: (0.0, 0.0),
                k: (kx, ky),
                at: (rect[0], rect[1]),
            }
        } else {
            ChildMap::of(child(g, "grpSpPr"), rect)
        };
        self.children(g, &map, 0, out);
    }

    fn children(&self, g: Node, map: &ChildMap, depth: usize, out: &mut Vec<FloatPrim>) {
        if depth > 16 {
            return;
        }
        for c in g.children().filter(|c| c.is_element()) {
            match c.tag_name().name() {
                // Word's shapes; SmartArt drawings' (`dsp:sp`).
                "wsp" | "sp" | "cxnSp" => {
                    let sp = child(c, "spPr");
                    let Some((r, size)) = map.child_rect(sp) else {
                        continue;
                    };
                    let geom =
                        sp.and_then(|s| child(s, "prstGeom").or_else(|| child(s, "custGeom")));
                    self.shape(sp, child(c, "style"), geom, size, r, map.k, out);
                }
                "pic" => {
                    let sp = child(c, "spPr");
                    let Some((r, _)) = map.child_rect(sp) else {
                        continue;
                    };
                    let fill = child(c, "blipFill");
                    let Some(rel) = fill
                        .and_then(|b| child(b, "blip"))
                        .and_then(|b| b.attributes().find(|a| a.name() == "embed"))
                        .map(|a| a.value().to_owned())
                    else {
                        continue;
                    };
                    let crop = fill
                        .and_then(|b| child(b, "srcRect"))
                        .map_or([0.0; 4], |s| {
                            ["l", "t", "r", "b"].map(|k| {
                                s.attribute(k)
                                    .and_then(|v| v.parse::<f32>().ok())
                                    .unwrap_or(0.0)
                                    / 100_000.0
                            })
                        });
                    self.picture(sp, None, &rel, crop, r, out);
                }
                "grpSp" => {
                    let gp = child(c, "grpSpPr");
                    let Some((r, _)) = map.child_rect(gp) else {
                        continue;
                    };
                    self.children(c, &ChildMap::of(gp, r), depth + 1, out);
                }
                _ => {}
            }
        }
    }

    /// The shapes of a SmartArt drawing part (`dsp:drawing`, positions in
    /// EMU from the frame's corner) drawn into `rect`, `k` px per EMU.
    pub(crate) fn smartart_prims(
        &self,
        xml: &str,
        rect: [f32; 4],
        k: (f32, f32),
    ) -> Vec<FloatPrim> {
        let mut out = Vec::new();
        let Ok(doc) = roxmltree::Document::parse(xml) else {
            return out;
        };
        let Some(tree) = child(doc.root_element(), "spTree") else {
            return out;
        };
        let map = ChildMap {
            off: (0.0, 0.0),
            k,
            at: (rect[0], rect[1]),
        };
        self.children(tree, &map, 0, &mut out);
        out
    }

    /// The theme's body font.
    pub(crate) fn minor_font(&self) -> Option<&str> {
        self.theme.minor_font.as_deref()
    }

    /// A glow (`a:effectLst/a:glow`) round `outlines`: its colour in a few
    /// widening, fainter edges out to its radius.
    fn glow(&self, sp: Option<Node>, outlines: &[Vec<Seg>]) -> Vec<FloatPrim> {
        let Some(gl) = sp
            .and_then(|s| child(s, "effectLst"))
            .and_then(|e| child(e, "glow"))
        else {
            return Vec::new();
        };
        let rad = gl
            .attribute("rad")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.0)
            / EMU_PER_PX;
        if rad < 0.5 {
            return Vec::new();
        }
        let cx = self.ctx();
        let Some((color, alpha)) = gl
            .children()
            .filter(|c| c.is_element())
            .find_map(|c| cx.color_alpha(c))
        else {
            return Vec::new();
        };
        const LAYERS: usize = 4;
        let mut out = Vec::new();
        for segs in outlines {
            for k in (1..=LAYERS).rev() {
                let a = (alpha as f32 / LAYERS as f32).round() as u8;
                out.push(FloatPrim::Path(DrawPath {
                    segs: segs.clone(),
                    fill: None,
                    // Stroked on the outline: half its width lies outside.
                    stroke: Some((
                        rgba(color, a.max(1)),
                        2.0 * rad * k as f32 / LAYERS as f32,
                        None,
                    )),
                    gradient: None,
                }));
            }
        }
        out
    }

    /// A 3-D extrusion (`a:sp3d/@extrusionH`) seen from the scene's
    /// camera (`a:scene3d/a:camera/@prst`), approximated: the outline
    /// repeated back along the depth in the extrusion colour (or the fill
    /// darkened), behind the shape. A camera straight in front shows
    /// none, as in Word.
    fn extrusion(
        &self,
        sp: Option<Node>,
        outlines: &[Vec<Seg>],
        fill: Option<Rgba>,
    ) -> Vec<FloatPrim> {
        let Some(sp) = sp else { return Vec::new() };
        let Some(d3) = child(sp, "sp3d") else {
            return Vec::new();
        };
        let depth = d3
            .attribute("extrusionH")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.0)
            / EMU_PER_PX;
        let camera = child(sp, "scene3d")
            .and_then(|s| child(s, "camera"))
            .and_then(|c| c.attribute("prst"))
            .unwrap_or("orthographicFront");
        if depth < 0.5 || camera == "orthographicFront" {
            return Vec::new();
        }
        let depth = depth.min(80.0) * 0.6;
        let sx = if camera.contains("Left") { -1.0 } else { 1.0 };
        let sy = if camera.contains("Top") || camera.contains("Above") {
            -1.0
        } else {
            1.0
        };
        let (ux, uy) = (
            sx * std::f32::consts::FRAC_1_SQRT_2,
            sy * std::f32::consts::FRAC_1_SQRT_2,
        );
        let cx = self.ctx();
        let color = child(d3, "extrusionClr")
            .and_then(|c| {
                c.children()
                    .filter(|n| n.is_element())
                    .find_map(|n| cx.color_alpha(n))
            })
            .map(|(c, a)| rgba(c, a))
            .or_else(|| {
                fill.map(|c| {
                    let [r, g, b, a] = c.0;
                    let k = |v: u8| (v as f32 * 0.6).round() as u8;
                    Rgba([k(r), k(g), k(b), a])
                })
            });
        let Some(color) = color else {
            return Vec::new();
        };
        let steps = (depth / 1.0).ceil().clamp(1.0, 60.0) as usize;
        let mut out = Vec::new();
        for k in (1..=steps).rev() {
            let t = depth * k as f32 / steps as f32;
            for segs in outlines {
                out.push(FloatPrim::Path(DrawPath {
                    segs: segs
                        .iter()
                        .map(|s| offset_seg(*s, ux * t, uy * t))
                        .collect(),
                    fill: Some(color),
                    stroke: None,
                    gradient: None,
                }));
            }
        }
        out
    }

    /// A top bevel (`a:sp3d/a:bevelT`), approximated: a light edge up and
    /// left and a dark one down and right inside the outline, as light
    /// from the top left shows it.
    fn bevel(&self, sp: Option<Node>, outlines: &[Vec<Seg>]) -> Vec<FloatPrim> {
        let Some(b) = sp
            .and_then(|s| child(s, "sp3d"))
            .and_then(|d| child(d, "bevelT"))
        else {
            return Vec::new();
        };
        if b.attribute("prst") == Some("none") {
            return Vec::new();
        }
        let w = b
            .attribute("w")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(76_200.0)
            / EMU_PER_PX;
        if w < 0.5 {
            return Vec::new();
        }
        let w = w.min(24.0);
        let mut out = Vec::new();
        for segs in outlines {
            for (shift, color) in [
                (w / 3.0, Rgba([0, 0, 0, 60])),
                (-w / 3.0, Rgba([255, 255, 255, 110])),
            ] {
                out.push(FloatPrim::Path(DrawPath {
                    segs: segs.iter().map(|s| offset_seg(*s, shift, shift)).collect(),
                    fill: None,
                    stroke: Some((color, w * 0.5, None)),
                    gradient: None,
                }));
            }
        }
        out
    }

    /// Soft edges' radius (`a:effectLst/a:softEdge/@rad`) in px.
    fn soft_edge(&self, sp: Option<Node>) -> Option<f32> {
        sp.and_then(|s| child(s, "effectLst"))
            .and_then(|e| child(e, "softEdge"))
            .and_then(|e| e.attribute("rad"))
            .and_then(|v| v.parse::<f32>().ok())
            .map(|r| r / EMU_PER_PX)
            .filter(|r| *r >= 0.5)
    }

    /// An outer shadow (`a:effectLst/a:outerShdw`) under `outlines`: the
    /// outlines moved by the shadow's distance and direction, filled in
    /// its colour, the blur as a few fainter widening edges.
    fn shadow(&self, sp: Option<Node>, outlines: &[Vec<Seg>]) -> Vec<FloatPrim> {
        let Some(sh) = sp
            .and_then(|s| child(s, "effectLst"))
            .and_then(|e| child(e, "outerShdw"))
        else {
            return Vec::new();
        };
        let num = |k: &str| {
            sh.attribute(k)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.0)
        };
        let dist = num("dist") / EMU_PER_PX;
        let dir = (num("dir") / 60_000.0).to_radians();
        let blur = num("blurRad") / EMU_PER_PX;
        let cx = self.ctx();
        let (color, alpha) = sh
            .children()
            .filter(|c| c.is_element())
            .find_map(|c| cx.color_alpha(c))
            .unwrap_or((docs_engine::Rgb([0, 0, 0]), 255));
        let (dx, dy) = (dist * dir.cos(), dist * dir.sin());
        let moved: Vec<Vec<Seg>> = outlines
            .iter()
            .map(|segs| segs.iter().map(|s| offset_seg(*s, dx, dy)).collect())
            .collect();
        let mut out = Vec::new();
        // Blurred: filled and blurred on the CPU, drawn as a picture.
        if blur > 0.5
            && let Some(b) = crate::blur::blurred(&moved, rgba(color, alpha).0, blur)
        {
            out.push(FloatPrim::Blurred(b));
            return out;
        }
        // Without a picture (nothing to fill): the shadow's alpha spread
        // over the core and a few edges that widen out to the blur radius.
        let layers = if blur > 0.5 { 3 } else { 0 };
        let core = if layers > 0 { alpha / 2 } else { alpha };
        for segs in &moved {
            for k in (1..=layers).rev() {
                let a = (alpha as f32 / 2.0 / layers as f32).round() as u8;
                out.push(FloatPrim::Path(DrawPath {
                    segs: segs.clone(),
                    fill: None,
                    stroke: Some((rgba(color, a.max(1)), blur * k as f32 / layers as f32, None)),
                    gradient: None,
                }));
            }
            out.push(FloatPrim::Path(DrawPath {
                segs: segs.clone(),
                fill: Some(rgba(color, core)),
                stroke: None,
                gradient: None,
            }));
        }
        out
    }

    /// Fill colour: `spPr`'s own, else the style's `fillRef`.
    fn fill(&self, sp: Option<Node>, style: Option<Node>) -> Option<Rgba> {
        let cx = self.ctx();
        let own = sp.and_then(|s| {
            s.children().find(|c| {
                c.is_element()
                    && matches!(
                        c.tag_name().name(),
                        "noFill" | "solidFill" | "gradFill" | "pattFill" | "blipFill" | "grpFill"
                    )
            })
        });
        match own {
            Some(n) if n.tag_name().name() == "noFill" => None,
            Some(n) => cx.fill(n).map(|c| rgba(c, alpha_of(&cx, n))),
            None => style
                .and_then(|s| child(s, "fillRef"))
                .filter(|r| r.attribute("idx").is_some_and(|i| i != "0"))
                .and_then(|r| cx.first_color(r))
                .map(|c| rgba(c, 255)),
        }
    }

    /// A gradient fill (`spPr`'s own `a:gradFill`, else a gradient fill
    /// style the style's `fillRef` names) over `rect` (page px): linear,
    /// or radial for path gradients (circle, rectangle and shape ones as
    /// ellipses). `None` for other fills.
    fn gradient(
        &self,
        sp: Option<Node>,
        style: Option<Node>,
        rect: [f32; 4],
    ) -> Option<slides_scene::Paint> {
        let cx = self.ctx();
        let own = sp.and_then(|s| {
            s.children().find(|c| {
                c.is_element()
                    && matches!(
                        c.tag_name().name(),
                        "noFill" | "solidFill" | "gradFill" | "pattFill" | "blipFill" | "grpFill"
                    )
            })
        });
        let fill = match own {
            Some(n) if n.tag_name().name() == "gradFill" => cx.full_fill(n, &|_| None)?,
            Some(_) => return None,
            None => {
                let r = style.and_then(|s| child(s, "fillRef"))?;
                let idx: i64 = r.attribute("idx")?.parse().ok()?;
                let ph = cx.first_color(r);
                cx.style_fill(idx, ph)?
            }
        };
        if !matches!(fill, slides_engine::Fill::Gradient(_)) {
            return None;
        }
        let [x, y, w, h] = rect;
        let paint = slides_scene::paint::fill_paint(
            &fill,
            slides_scene::PathFill::Norm,
            w,
            h,
            &slides_scene::Affine::IDENTITY,
        )?;
        let at = |q: slides_scene::P| slides_scene::p(q.x + x, q.y + y);
        Some(match paint {
            slides_scene::Paint::Linear { from, to, stops } => slides_scene::Paint::Linear {
                from: at(from),
                to: at(to),
                stops,
            },
            slides_scene::Paint::Radial {
                center,
                rx,
                ry,
                stops,
            } => slides_scene::Paint::Radial {
                center: at(center),
                rx,
                ry,
                stops,
            },
            solid => solid,
        })
    }

    /// Outline: `spPr/a:ln`, else the style's `lnRef`.
    fn line(&self, sp: Option<Node>, style: Option<Node>) -> Option<(Rgba, f32, Option<Vec<f32>>)> {
        let cx = self.ctx();
        let ln = sp.and_then(|s| child(s, "ln"));
        let lnref = style
            .and_then(|s| child(s, "lnRef"))
            .filter(|r| r.attribute("idx").is_some_and(|i| i != "0"));
        let width_emu = ln
            .and_then(|l| l.attribute("w"))
            .and_then(|w| w.parse::<f32>().ok())
            .unwrap_or(9525.0);
        let fill = ln.and_then(|l| {
            l.children().find(|c| {
                c.is_element() && matches!(c.tag_name().name(), "noFill" | "solidFill" | "gradFill")
            })
        });
        let color = match fill {
            Some(n) if n.tag_name().name() == "noFill" => return None,
            Some(n) => cx.fill(n).map(|c| rgba(c, alpha_of(&cx, n))),
            None => lnref.and_then(|r| cx.first_color(r)).map(|c| rgba(c, 255)),
        }?;
        let width = (width_emu / EMU_PER_PX).max(0.75);
        let dash = ln
            .map(|l| {
                cx.line_style(l, &slides_engine::fill::LineStyle::default())
                    .dash
            })
            .filter(|d| *d != Dash::Solid)
            .and_then(|d| d.pattern())
            .map(|p| p.into_iter().map(|v| v * width).collect());
        Some((color, width, dash))
    }
}

fn alpha_of(cx: &ColorCtx, fill: Node) -> u8 {
    fill.children()
        .filter(|c| c.is_element())
        .find_map(|c| cx.color_alpha(c))
        .map_or(255, |(_, a)| a)
}

fn rgba(c: docs_engine::Rgb, a: u8) -> Rgba {
    let [r, g, b] = c.0;
    Rgba([r, g, b, a])
}

/// An XML fragment whose prefixes may be undeclared (they're declared on
/// the document's root) wrapped so it parses on its own.
fn wrap(x: &str) -> String {
    let decl = "xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
                xmlns:wps=\"http://schemas.microsoft.com/office/word/2010/wordprocessingShape\" \
                xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\" \
                xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
                xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\" \
                xmlns:wpg=\"http://schemas.microsoft.com/office/word/2010/wordprocessingGroup\" \
                xmlns:wpc=\"http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas\" \
                xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" \
                xmlns:a14=\"http://schemas.microsoft.com/office/drawing/2010/main\" \
                xmlns:wp14=\"http://schemas.microsoft.com/office/word/2010/wordprocessingDrawing\" \
                xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\"";
    // Other prefixes the fragment uses (a text box's `w:`, VML…) get a
    // stand-in namespace so it still parses.
    let mut extra = String::new();
    let b = x.as_bytes();
    let mut seen: Vec<&str> = Vec::new();
    for (i, _) in x.match_indices(':') {
        let start = x[..i]
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .map_or(0, |k| k + 1);
        let before = start.checked_sub(1).map(|k| b[k]);
        if !matches!(before, Some(b'<' | b'/' | b' ' | b'\t' | b'\n' | b'\r')) || start == i {
            continue;
        }
        let pfx = &x[start..i];
        if pfx == "xmlns"
            || pfx == "xml"
            || seen.contains(&pfx)
            || decl.contains(&format!("xmlns:{pfx}="))
            || x.contains(&format!("xmlns:{pfx}="))
        {
            continue;
        }
        seen.push(pfx);
        extra.push_str(&format!(" xmlns:{pfx}=\"urn:kantor:{pfx}\""));
    }
    format!("<root {decl}{extra}>{x}</root>")
}

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

/// Shape-local EMU → page px: the box, rotation and flips (`a:xfrm`).
struct Xform {
    rect: [f32; 4],
    rot: f32,
    flip_h: bool,
    flip_v: bool,
    sx: f32,
    sy: f32,
}

impl Xform {
    /// `scale`: px per unit of the outline (EMU).
    fn of(sp: Option<Node>, rect: [f32; 4], scale: (f32, f32)) -> Self {
        let xfrm = sp.and_then(|s| child(s, "xfrm"));
        let num = |n: Option<Node>, k: &str| {
            n.and_then(|n| n.attribute(k))
                .and_then(|v| v.parse::<f32>().ok())
        };
        let on = |k: &str| {
            xfrm.and_then(|x| x.attribute(k))
                .is_some_and(|v| v == "1" || v == "true")
        };
        // The outline is in EMU at the object's size.
        Self {
            rect,
            rot: num(xfrm, "rot").unwrap_or(0.0) / 60_000.0,
            flip_h: on("flipH"),
            flip_v: on("flipV"),
            sx: scale.0,
            sy: scale.1,
        }
    }

    /// The box's outline, turned.
    fn rect_segs(&self) -> Vec<Seg> {
        let [_, _, w, h] = self.rect;
        let k = |x: f32, y: f32| {
            let (sx, sy) = (self.sx, self.sy);
            self.pt(((x / sx) as f64, (y / sy) as f64))
        };
        vec![
            Seg::Move(k(0.0, 0.0)),
            Seg::Line(k(w, 0.0)),
            Seg::Line(k(w, h)),
            Seg::Line(k(0.0, h)),
            Seg::Close,
        ]
    }

    fn pt(&self, (x, y): (f64, f64)) -> [f32; 2] {
        let [rx, ry, w, h] = self.rect;
        let (mut x, mut y) = (x as f32 * self.sx, y as f32 * self.sy);
        if self.flip_h {
            x = w - x;
        }
        if self.flip_v {
            y = h - y;
        }
        let (cx, cy) = (w / 2.0, h / 2.0);
        if self.rot != 0.0 {
            let (s, c) = self.rot.to_radians().sin_cos();
            let (dx, dy) = (x - cx, y - cy);
            x = cx + dx * c - dy * s;
            y = cy + dx * s + dy * c;
        }
        [rx + x, ry + y]
    }

    fn seg(&self, c: &PathCmd) -> Seg {
        match *c {
            PathCmd::MoveTo(p) => Seg::Move(self.pt(p)),
            PathCmd::LineTo(p) => Seg::Line(self.pt(p)),
            PathCmd::QuadTo(c, p) => Seg::Quad(self.pt(c), self.pt(p)),
            PathCmd::CubicTo(a, b, p) => Seg::Cubic(self.pt(a), self.pt(b), self.pt(p)),
            PathCmd::Close => Seg::Close,
        }
    }
}

/// A group's child coordinates (EMU) → page px.
struct ChildMap {
    /// Child-space origin (`a:chOff`).
    off: (f32, f32),
    /// px per child EMU.
    k: (f32, f32),
    /// Where the origin lands, page px.
    at: (f32, f32),
}

impl ChildMap {
    /// From a group's `grpSpPr` (`a:xfrm` with `chOff`/`chExt`), its box
    /// being `rect`.
    fn of(gp: Option<Node>, rect: [f32; 4]) -> Self {
        let xfrm = gp.and_then(|g| child(g, "xfrm"));
        let pair = |name: &str, a: &str, b: &str| {
            let n = xfrm.and_then(|x| child(x, name))?;
            let v = |k: &str| n.attribute(k).and_then(|v| v.parse::<f32>().ok());
            Some((v(a)?, v(b)?))
        };
        let off = pair("chOff", "x", "y")
            .or_else(|| pair("off", "x", "y"))
            .unwrap_or((0.0, 0.0));
        let ext = pair("chExt", "cx", "cy")
            .or_else(|| pair("ext", "cx", "cy"))
            .filter(|e| e.0 > 0.0 && e.1 > 0.0);
        let k = match ext {
            Some((cx, cy)) => (rect[2] / cx, rect[3] / cy),
            None => (1.0 / EMU_PER_PX, 1.0 / EMU_PER_PX),
        };
        Self {
            off,
            k,
            at: (rect[0], rect[1]),
        }
    }

    /// A child's box on the page and its size in EMU, from its `a:xfrm`.
    fn child_rect(&self, sp: Option<Node>) -> Option<([f32; 4], (f64, f64))> {
        let xfrm = sp.and_then(|s| child(s, "xfrm"))?;
        let v = |name: &str, k: &str| {
            child(xfrm, name)
                .and_then(|n| n.attribute(k))
                .and_then(|v| v.parse::<f32>().ok())
        };
        let (x, y) = (v("off", "x").unwrap_or(0.0), v("off", "y").unwrap_or(0.0));
        let (cx, cy) = (v("ext", "cx")?, v("ext", "cy")?);
        let r = [
            self.at.0 + (x - self.off.0) * self.k.0,
            self.at.1 + (y - self.off.1) * self.k.1,
            cx * self.k.0,
            cy * self.k.1,
        ];
        Some((r, (cx as f64, cy as f64)))
    }
}

fn offset_seg(s: Seg, dx: f32, dy: f32) -> Seg {
    let m = |p: [f32; 2]| [p[0] + dx, p[1] + dy];
    match s {
        Seg::Move(a) => Seg::Move(m(a)),
        Seg::Line(a) => Seg::Line(m(a)),
        Seg::Quad(c, a) => Seg::Quad(m(c), m(a)),
        Seg::Cubic(c1, c2, a) => Seg::Cubic(m(c1), m(c2), m(a)),
        Seg::Close => Seg::Close,
    }
}

fn rect_segs([x, y, w, h]: [f32; 4]) -> Vec<Seg> {
    vec![
        Seg::Move([x, y]),
        Seg::Line([x + w, y]),
        Seg::Line([x + w, y + h]),
        Seg::Line([x, y + h]),
        Seg::Close,
    ]
}

// ── gradients for renderers without shadings ───────────────────────────────

/// A piece of a gradient fill for a renderer that fills only with a
/// colour or a two-colour linear gradient (the Docs view: GPUI).
#[derive(Clone, Debug, PartialEq)]
pub enum GradientPiece {
    Solid(Rgba),
    /// Colour `from.1` where the projection of a point on unit vector
    /// `dir` is `from.0`, `to.1` at `to.0`; `bbox` is the piece's box.
    Band {
        bbox: [f32; 4],
        dir: [f32; 2],
        from: (f32, Rgba),
        to: (f32, Rgba),
    },
}

/// Steps of a radial gradient drawn as nested ellipses.
const RADIAL_STEPS: usize = 24;

/// Gradient `g` over the path `segs` as pieces, each with its outline: a
/// linear gradient as one band per pair of stops (exact), a radial one as
/// nested ellipses of solid colour clipped to the path (approximated).
pub fn gradient_pieces(segs: &[Seg], g: &slides_scene::Paint) -> Vec<(Vec<Seg>, GradientPiece)> {
    use slides_scene::{P, Paint, p};
    let path = slides_scene::Path {
        segs: segs
            .iter()
            .map(|s| match *s {
                Seg::Move(a) => slides_scene::Seg::Move(p(a[0], a[1])),
                Seg::Line(a) => slides_scene::Seg::Line(p(a[0], a[1])),
                Seg::Quad(a, b) => slides_scene::Seg::Quad(p(a[0], a[1]), p(b[0], b[1])),
                Seg::Cubic(a, b, c) => {
                    slides_scene::Seg::Cubic(p(a[0], a[1]), p(b[0], b[1]), p(c[0], c[1]))
                }
                Seg::Close => slides_scene::Seg::Close,
            })
            .collect(),
    };
    let polys: Vec<Vec<P>> = path.flatten(0.25).into_iter().map(|(q, _)| q).collect();
    let to_segs = |polys: &[Vec<P>]| -> Vec<Seg> {
        let mut out = Vec::new();
        for poly in polys.iter().filter(|q| q.len() >= 3) {
            out.push(Seg::Move([poly[0].x, poly[0].y]));
            out.extend(poly[1..].iter().map(|q| Seg::Line([q.x, q.y])));
            out.push(Seg::Close);
        }
        out
    };
    let clip = |poly: &[P], f: &dyn Fn(P) -> f32| -> Vec<P> {
        let mut out = Vec::with_capacity(poly.len() + 4);
        for i in 0..poly.len() {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            let (fa, fb) = (f(a), f(b));
            if fa >= 0.0 {
                out.push(a);
            }
            if (fa >= 0.0) != (fb >= 0.0) {
                out.push(a.plus(b.minus(a).scale(fa / (fa - fb))));
            }
        }
        out
    };
    let mut out = Vec::new();
    match g {
        Paint::Solid(c) => out.push((segs.to_vec(), GradientPiece::Solid(*c))),
        Paint::Linear { from, to, stops } if stops.len() >= 2 => {
            let len = to.minus(*from).len().max(1e-4);
            let u = to.minus(*from).scale(1.0 / len);
            let s_of = |o: f32| from.dot(u) + o * len;
            for k in 0..stops.len() - 1 {
                let (lo, hi) = (s_of(stops[k].offset), s_of(stops[k + 1].offset));
                let (first, last) = (k == 0, k + 2 == stops.len());
                let band: Vec<Vec<P>> = polys
                    .iter()
                    .map(|poly| {
                        let mut q = poly.clone();
                        if !first {
                            q = clip(&q, &|v| v.dot(u) - lo);
                        }
                        if !last {
                            q = clip(&q, &|v| hi - v.dot(u));
                        }
                        q
                    })
                    .collect();
                let all: Vec<P> = band.concat();
                let piece = to_segs(&band);
                if piece.is_empty() {
                    continue;
                }
                let x0 = all.iter().map(|q| q.x).fold(f32::MAX, f32::min);
                let y0 = all.iter().map(|q| q.y).fold(f32::MAX, f32::min);
                let x1 = all.iter().map(|q| q.x).fold(f32::MIN, f32::max);
                let y1 = all.iter().map(|q| q.y).fold(f32::MIN, f32::max);
                out.push((
                    piece,
                    GradientPiece::Band {
                        bbox: [x0, y0, x1 - x0, y1 - y0],
                        dir: [u.x, u.y],
                        from: (lo, stops[k].color),
                        to: (hi.max(lo + 1e-3), stops[k + 1].color),
                    },
                ));
            }
        }
        Paint::Linear { stops, .. } | Paint::Radial { stops, .. } if stops.len() < 2 => {
            if let Some(s) = stops.first() {
                out.push((segs.to_vec(), GradientPiece::Solid(s.color)));
            }
        }
        Paint::Radial {
            center,
            rx,
            ry,
            stops,
        } => {
            let color_at = |o: f32| -> Rgba {
                let (mut lo, mut hi) = (stops[0], stops[stops.len() - 1]);
                for w in stops.windows(2) {
                    if o >= w[0].offset && o <= w[1].offset {
                        (lo, hi) = (w[0], w[1]);
                        break;
                    }
                }
                let k = if o <= lo.offset {
                    0.0
                } else if o >= hi.offset || hi.offset <= lo.offset {
                    1.0
                } else {
                    (o - lo.offset) / (hi.offset - lo.offset)
                };
                let (a, b) = (lo.color.0, hi.color.0);
                let m = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * k).round() as u8;
                Rgba([m(0), m(1), m(2), m(3)])
            };
            out.push((segs.to_vec(), GradientPiece::Solid(color_at(1.0))));
            for i in 1..RADIAL_STEPS {
                let r = 1.0 - i as f32 / RADIAL_STEPS as f32;
                let ell: Vec<P> = (0..48)
                    .map(|j| {
                        let a = j as f32 / 48.0 * std::f32::consts::TAU;
                        p(center.x + rx * r * a.cos(), center.y + ry * r * a.sin())
                    })
                    .collect();
                let clipped: Vec<Vec<P>> = polys
                    .iter()
                    .map(|poly| {
                        let mut q = poly.clone();
                        for e in 0..ell.len() {
                            let (a, b) = (ell[e], ell[(e + 1) % ell.len()]);
                            let n = p(-(b.y - a.y), b.x - a.x);
                            let side = if center.minus(a).dot(n) >= 0.0 {
                                1.0
                            } else {
                                -1.0
                            };
                            q = clip(&q, &|v| v.minus(a).dot(n) * side);
                            if q.is_empty() {
                                break;
                            }
                        }
                        q
                    })
                    .collect();
                let piece = to_segs(&clipped);
                if !piece.is_empty() {
                    let c = color_at(r - 0.5 / RADIAL_STEPS as f32);
                    out.push((piece, GradientPiece::Solid(c)));
                }
            }
        }
        _ => {}
    }
    out
}

/// A filled triangle at `tip`, pointing from `from`.
fn arrow(from: [f32; 2], tip: [f32; 2], color: Rgba, width: f32) -> FloatPrim {
    let (dx, dy) = (tip[0] - from[0], tip[1] - from[1]);
    let len = (dx * dx + dy * dy).sqrt().max(1e-3);
    let (ux, uy) = (dx / len, dy / len);
    let (l, half) = ((width * 3.0).max(6.0), (width * 1.5).max(3.0));
    let base = [tip[0] - ux * l, tip[1] - uy * l];
    FloatPrim::Path(DrawPath {
        segs: vec![
            Seg::Move(tip),
            Seg::Line([base[0] - uy * half, base[1] + ux * half]),
            Seg::Line([base[0] + uy * half, base[1] - ux * half]),
            Seg::Close,
        ],
        fill: Some(color),
        stroke: None,
        gradient: None,
    })
}

/// The box a floating object takes on a page with `setup` and number
/// `number`, anchored at `at` (headers and footers place theirs here).
pub fn place(f: &Float, index: usize, setup: &PageSetup, number: u32, at: AnchorAt) -> [f32; 4] {
    docs_layout::float_rect(&crate::build::float_spec(index, f), setup, number, at)
}

/// Where a shape's text goes: its geometry's text rectangle (an
/// ellipse's is inside the curve) less the text box's inner margins,
/// `[x, y, w, h]` on the page.
pub fn text_area(f: &Float, rect: [f32; 4]) -> [f32; 4] {
    let src = f.geometry_xml().map(wrap);
    let doc = src
        .as_deref()
        .and_then(|x| roxmltree::Document::parse(x).ok());
    let geometry = doc
        .as_ref()
        .and_then(|d| d.root_element().first_element_child())
        .and_then(Geometry::from_xml)
        .unwrap_or_else(|| Geometry::preset("rect"));
    // The box as drawn (a "Resize shape to fit text" shape grows with its
    // text: see `pages::float_item`).
    let (cx, cy) = if rect[2] > 0.0 || rect[3] > 0.0 {
        (
            (rect[2].max(0.0) * EMU_PER_PX) as f64,
            (rect[3].max(0.0) * EMU_PER_PX) as f64,
        )
    } else {
        (f.at.cx.max(0) as f64, f.at.cy.max(0) as f64)
    };
    let tr = outline(&geometry, cx, cy).text_rect;
    let px = |v: f64| v as f32 / EMU_PER_PX;
    let [l, t, r, b] = insets(f);
    let x = rect[0] + px(tr.l) + l;
    let y = rect[1] + px(tr.t) + t;
    let w = (px(tr.r - tr.l) - l - r).max(8.0);
    let h = (px(tr.b - tr.t) - t - b).max(0.0);
    [x, y, w, h]
}

/// How far a shape is turned (degrees clockwise, `a:xfrm/@rot`): its text
/// box's text turns with it, unless the body says `upright`.
pub fn text_turn(f: &Float) -> f32 {
    if f.text.is_none() || f.xml.contains("upright=\"1\"") {
        return 0.0;
    }
    let (sp, _) = f.shape_xml();
    let Some(sp) = sp else { return 0.0 };
    let src = wrap(sp);
    let Ok(doc) = roxmltree::Document::parse(&src) else {
        return 0.0;
    };
    let sp = doc.root_element().first_element_child();
    Xform::of(sp, [0.0; 4], (1.0, 1.0)).rot
}

/// Where the text of a group's or canvas's shapes goes when the object
/// is at `rect`: one entry per [`Float::boxes`] (`None` for a shape
/// without a size of its own).
pub fn group_text_areas(f: &Float, rect: [f32; 4]) -> Vec<Option<BoxArea>> {
    let FloatKind::Group { canvas } = f.kind else {
        return Vec::new();
    };
    let src = f.graphic_xml().map(wrap);
    let Some(doc) = src
        .as_deref()
        .and_then(|x| roxmltree::Document::parse(x).ok())
    else {
        return Vec::new();
    };
    let Some(g) = doc
        .root_element()
        .first_element_child()
        .and_then(|data| data.first_element_child())
    else {
        return Vec::new();
    };
    let map = if canvas {
        ChildMap {
            off: (0.0, 0.0),
            k: (
                rect[2] / f.read.cx.max(1) as f32,
                rect[3] / f.read.cy.max(1) as f32,
            ),
            at: (rect[0], rect[1]),
        }
    } else {
        ChildMap::of(child(g, "grpSpPr"), rect)
    };
    let mut out = Vec::new();
    box_areas(f, g, &map, 0, &mut out);
    out
}

fn box_areas(f: &Float, g: Node, map: &ChildMap, depth: usize, out: &mut Vec<Option<BoxArea>>) {
    if depth > 16 {
        return;
    }
    for c in g.children().filter(|c| c.is_element()) {
        match c.tag_name().name() {
            "wsp" => {
                let has_text = child(c, "txbx").is_some_and(|t| child(t, "txbxContent").is_some());
                if !has_text {
                    continue;
                }
                let sp = child(c, "spPr");
                let Some((r, (cx, cy))) = map.child_rect(sp) else {
                    out.push(None);
                    continue;
                };
                let geometry = sp
                    .and_then(|s| child(s, "prstGeom").or_else(|| child(s, "custGeom")))
                    .and_then(Geometry::from_xml)
                    .unwrap_or_else(|| Geometry::preset("rect"));
                let tr = outline(&geometry, cx, cy).text_rect;
                let k = map.k;
                let [l, t, rr, b] = f.boxes.get(out.len()).map_or([0.0; 4], |b| {
                    b.text.insets.map(|v| v.max(0) as f32 / EMU_PER_PX)
                });
                let x = r[0] + tr.l as f32 * k.0 + l;
                let y = r[1] + tr.t as f32 * k.1 + t;
                let w = ((tr.r - tr.l) as f32 * k.0 - l - rr).max(8.0);
                let h = ((tr.b - tr.t) as f32 * k.1 - t - b).max(0.0);
                let upright = child(c, "bodyPr")
                    .and_then(|b| b.attribute("upright"))
                    .is_some_and(|v| v == "1" || v == "true");
                let turn = if upright {
                    0.0
                } else {
                    Xform::of(sp, r, (1.0, 1.0)).rot
                };
                out.push(Some(BoxArea {
                    rect: r,
                    area: [x, y, w, h],
                    turn,
                }));
            }
            "grpSp" => {
                let gp = child(c, "grpSpPr");
                let Some((r, _)) = map.child_rect(gp) else {
                    continue;
                };
                box_areas(f, c, &ChildMap::of(gp, r), depth + 1, out);
            }
            _ => {}
        }
    }
}

/// Inner margins of a text box, px: left, top, right, bottom.
pub fn insets(f: &Float) -> [f32; 4] {
    f.text
        .as_ref()
        .map_or([0.0; 4], |t| t.insets.map(|v| v.max(0) as f32 / EMU_PER_PX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crops_rotations_and_arrows() {
        let xml = "<w:drawing xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:wps=\"http://schemas.microsoft.com/office/word/2010/wordprocessingShape\"><wp:anchor distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\" simplePos=\"0\" relativeHeight=\"1\" behindDoc=\"0\" locked=\"0\" layoutInCell=\"1\" allowOverlap=\"1\"><wp:simplePos x=\"0\" y=\"0\"/><wp:positionH relativeFrom=\"page\"><wp:posOffset>0</wp:posOffset></wp:positionH><wp:positionV relativeFrom=\"page\"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx=\"952500\" cy=\"0\"/><wp:wrapNone/><wp:docPr id=\"1\" name=\"Line\"/><a:graphic><a:graphicData uri=\"http://schemas.microsoft.com/office/word/2010/wordprocessingShape\"><wps:wsp><wps:spPr><a:xfrm flipH=\"1\"><a:off x=\"0\" y=\"0\"/><a:ext cx=\"952500\" cy=\"0\"/></a:xfrm><a:prstGeom prst=\"line\"><a:avLst/></a:prstGeom><a:ln w=\"19050\"><a:solidFill><a:srgbClr val=\"C00000\"/></a:solidFill><a:tailEnd type=\"triangle\"/></a:ln></wps:spPr><wps:bodyPr/></wps:wsp></a:graphicData></a:graphic></wp:anchor></w:drawing>";
        let doc = Document::new();
        let p = Painter::new(&doc);
        let read = Float::from_xml(xml).unwrap();
        let prims = p.prims(&read, [10.0, 20.0, 100.0, 0.0]);
        // The line, flipped: it runs right to left, the arrow at the left.
        let FloatPrim::Path(line) = &prims[0] else {
            panic!("{prims:?}")
        };
        assert_eq!(line.segs[0], Seg::Move([110.0, 20.0]));
        assert_eq!(line.segs[1], Seg::Line([10.0, 20.0]));
        let (color, width, _) = line.stroke.clone().unwrap();
        assert_eq!(color, Rgba([0xC0, 0, 0, 255]));
        assert_eq!(width, 2.0);
        let FloatPrim::Path(head) = prims.last().unwrap() else {
            panic!()
        };
        assert_eq!(head.segs[0], Seg::Move([10.0, 20.0]));
        assert!(head.fill.is_some());
    }

    fn shape_xml(sp_pr: &str) -> String {
        format!(
            "<w:drawing xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:wps=\"http://schemas.microsoft.com/office/word/2010/wordprocessingShape\"><wp:anchor distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\" simplePos=\"0\" relativeHeight=\"1\" behindDoc=\"0\" locked=\"0\" layoutInCell=\"1\" allowOverlap=\"1\"><wp:simplePos x=\"0\" y=\"0\"/><wp:positionH relativeFrom=\"page\"><wp:posOffset>0</wp:posOffset></wp:positionH><wp:positionV relativeFrom=\"page\"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx=\"952500\" cy=\"952500\"/><wp:wrapNone/><wp:docPr id=\"1\" name=\"Box\"/><a:graphic><a:graphicData uri=\"http://schemas.microsoft.com/office/word/2010/wordprocessingShape\"><wps:wsp><wps:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"952500\" cy=\"952500\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom>{sp_pr}</wps:spPr><wps:bodyPr/></wps:wsp></a:graphicData></a:graphic></wp:anchor></w:drawing>"
        )
    }

    const GRAD: &str = "<a:gradFill><a:gsLst><a:gs pos=\"0\"><a:srgbClr val=\"FF0000\"/></a:gs><a:gs pos=\"50000\"><a:srgbClr val=\"00FF00\"/></a:gs><a:gs pos=\"100000\"><a:srgbClr val=\"0000FF\"/></a:gs></a:gsLst><a:lin ang=\"0\" scaled=\"0\"/></a:gradFill>";

    #[test]
    fn gradients_are_drawn_as_gradients() {
        let doc = Document::new();
        let p = Painter::new(&doc);
        let f = Float::from_xml(&shape_xml(GRAD)).unwrap();
        let prims = p.prims(&f, [10.0, 20.0, 100.0, 100.0]);
        let FloatPrim::Path(box_) = &prims[0] else {
            panic!("{prims:?}")
        };
        // The first colour stays as the plain fill.
        assert_eq!(box_.fill, Some(Rgba([255, 0, 0, 255])));
        let Some(slides_scene::Paint::Linear { from, to, stops }) = &box_.gradient else {
            panic!("{:?}", box_.gradient)
        };
        assert_eq!((from.x, to.x), (10.0, 110.0));
        assert_eq!(stops.len(), 3);
        // On screen: two bands, left and right halves.
        let pieces = gradient_pieces(&box_.segs, box_.gradient.as_ref().unwrap());
        assert_eq!(pieces.len(), 2);
        let GradientPiece::Band { bbox, from, to, .. } = &pieces[0].1 else {
            panic!()
        };
        assert!((bbox[0] - 10.0).abs() < 0.01 && (bbox[2] - 50.0).abs() < 0.01);
        assert_eq!(
            (from.1, to.1),
            (Rgba([255, 0, 0, 255]), Rgba([0, 255, 0, 255]))
        );
        // A path gradient: nested ellipses, the outer colour first.
        let radial = GRAD.replace(
            "<a:lin ang=\"0\" scaled=\"0\"/>",
            "<a:path path=\"circle\"><a:fillToRect l=\"50000\" t=\"50000\" r=\"50000\" b=\"50000\"/></a:path>",
        );
        let f = Float::from_xml(&shape_xml(&radial)).unwrap();
        let prims = p.prims(&f, [0.0, 0.0, 100.0, 100.0]);
        let FloatPrim::Path(box_) = &prims[0] else {
            panic!()
        };
        let pieces = gradient_pieces(&box_.segs, box_.gradient.as_ref().unwrap());
        assert!(pieces.len() > 10);
        assert_eq!(pieces[0].1, GradientPiece::Solid(Rgba([0, 0, 255, 255])));
    }

    #[test]
    fn three_d_extrusions_and_bevels_are_approximated() {
        let doc = Document::new();
        let painter = Painter::new(&doc);
        let fill = "<a:solidFill><a:srgbClr val=\"4472C4\"/></a:solidFill>";
        let count = |extra: &str| {
            let f = Float::from_xml(&shape_xml(&format!("{fill}{extra}"))).unwrap();
            painter.prims(&f, [0.0, 0.0, 100.0, 100.0]).len()
        };
        let plain = count("");
        // Seen from in front: no depth shows.
        assert_eq!(count("<a:sp3d extrusionH=\"190500\"/>"), plain);
        let deep = count(
            "<a:scene3d><a:camera prst=\"isometricOffAxis1Right\"/><a:lightRig rig=\"threePt\" dir=\"t\"/></a:scene3d><a:sp3d extrusionH=\"190500\"/>",
        );
        assert!(deep > plain + 5, "{deep} {plain}");
        let bevel = count("<a:sp3d><a:bevelT w=\"101600\" h=\"101600\"/></a:sp3d>");
        assert_eq!(bevel, plain + 2);
    }

    #[test]
    fn glow_and_soft_edges_are_approximated() {
        let doc = Document::new();
        let p = Painter::new(&doc);
        let glow = "<a:solidFill><a:srgbClr val=\"4472C4\"/></a:solidFill><a:ln><a:solidFill><a:srgbClr val=\"000000\"/></a:solidFill></a:ln><a:effectLst><a:glow rad=\"101600\"><a:srgbClr val=\"FFC000\"><a:alpha val=\"60000\"/></a:srgbClr></a:glow></a:effectLst>";
        let f = Float::from_xml(&shape_xml(glow)).unwrap();
        let prims = p.prims(&f, [0.0, 0.0, 100.0, 100.0]);
        let glows = prims
            .iter()
            .filter(|q| matches!(q, FloatPrim::Path(d) if d.fill.is_none() && d.stroke.as_ref().is_some_and(|s| s.0.0[..3] == [0xFF, 0xC0, 0])))
            .count();
        assert_eq!(glows, 4);
        let soft = "<a:solidFill><a:srgbClr val=\"4472C4\"/></a:solidFill><a:ln><a:solidFill><a:srgbClr val=\"000000\"/></a:solidFill></a:ln><a:effectLst><a:softEdge rad=\"63500\"/></a:effectLst>";
        let f = Float::from_xml(&shape_xml(soft)).unwrap();
        let prims = p.prims(&f, [0.0, 0.0, 100.0, 100.0]);
        let FloatPrim::Path(d) = &prims[0] else {
            panic!()
        };
        assert!(d.stroke.is_none());
        assert!(d.fill.unwrap().0[3] < 255);
    }
}
