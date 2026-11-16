//! SmartArt: PowerPoint keeps a diagram as a data model (`dgm:dataModel`,
//! the items and their text) plus a pre-drawn shape tree (the `dsp`
//! drawing part) that its layout engine made from it.
//!
//! Kantor reads the drawing into the model when a file opens, so its
//! shapes can be selected like a group's: their text (and fill and
//! outline) can be edited. Saving writes the edited shapes into the
//! drawing part and the same text (and formatting) into the data model's
//! items, so PowerPoint, which lays SmartArt out again from the data
//! model, shows the edit too. Each drawing shape's `modelId` names a
//! presentation item whose `presAssocID` is the data item its text comes
//! from; a shape showing several items (a list's bullets) maps one
//! paragraph run per item, and keeps its paragraph count.
//!
//! Adding or removing items and changing the layout need PowerPoint's
//! layout engine and aren't possible; *Convert to Shapes* turns the
//! drawing into a group of ordinary shapes.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::ops::Range;
use std::sync::Arc;

use anyhow::{Context as _, bail};
use roxmltree::Node;

use crate::frame::{DiagramDrawing, DiagramRef, FrameContent, NS_DGM, NS_DSP};
use crate::group::ShapeRef;
use crate::history::{Focus, Kind};
use crate::model::*;
use crate::presentation::{Presentation, Slide};
use crate::shape::{
    ParseCtx, is_group_child, parse_group, parse_text_body, sppr_at, sppr_paint_edits, write_para,
    write_shape,
};
use crate::theme::ColorCtx;
use crate::xml::*;

/// Which data model items a drawing shape's text comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Link {
    /// None Kantor can tell (decorations, connectors): the text can't be
    /// edited.
    None,
    /// All of it is this item's text.
    One(String),
    /// Paragraphs from several items in order: `(modelId, paragraphs)`.
    Many(Vec<(String, usize)>),
}

/// Relationship types of a SmartArt's parts.
const DIAGRAM_RELS: [&str; 5] = [
    "diagramData",
    "diagramLayout",
    "diagramQuickStyle",
    "diagramColors",
    "diagramDrawing",
];

pub(crate) fn is_diagram_rel(kind: &str) -> bool {
    DIAGRAM_RELS.contains(&kind)
}

/// A data model item, as far as text goes.
struct Point<'a> {
    node: Node<'a, 'a>,
    assoc: Option<&'a str>,
    paras: Vec<String>,
}

/// The data model's items by `modelId`, and each item's children in order
/// (`parOf` connections).
struct Model<'a> {
    points: HashMap<&'a str, Point<'a>>,
    kids: HashMap<&'a str, Vec<(i64, &'a str)>>,
}

impl<'a> Model<'a> {
    fn read(doc: &'a roxmltree::Document<'a>, cx: &ParseCtx) -> Self {
        let mut points = HashMap::new();
        let mut kids: HashMap<&str, Vec<(i64, &str)>> = HashMap::new();
        for n in doc.descendants() {
            if is(n, NS_DGM, "pt")
                && let Some(id) = n.attribute("modelId")
            {
                let assoc = child(n, NS_DGM, "prSet").and_then(|p| p.attribute("presAssocID"));
                let paras = child(n, NS_DGM, "t")
                    .map(|t| {
                        parse_text_body(t, cx)
                            .0
                            .paras
                            .into_iter()
                            .map(|p| p.text)
                            .collect()
                    })
                    .unwrap_or_default();
                points.insert(
                    id,
                    Point {
                        node: n,
                        assoc,
                        paras,
                    },
                );
            }
            if is(n, NS_DGM, "cxn")
                && n.attribute("type").is_none_or(|t| t == "parOf")
                && let (Some(src), Some(dst)) = (n.attribute("srcId"), n.attribute("destId"))
            {
                let ord = attr_i64(n, "srcOrd").unwrap_or(0);
                kids.entry(src).or_default().push((ord, dst));
            }
        }
        for v in kids.values_mut() {
            v.sort_by_key(|k| k.0);
        }
        Self { points, kids }
    }

    /// `id` and its descendants, depth first.
    fn subtree(&self, id: &'a str, out: &mut Vec<&'a str>) {
        if out.len() > 10_000 || out.contains(&id) {
            return;
        }
        out.push(id);
        for (_, k) in self.kids.get(id).into_iter().flatten() {
            self.subtree(k, out);
        }
    }

    /// Where the text `shown` of the shape drawing item `model_id` comes from.
    fn link(&self, model_id: Option<&str>, shown: &[String]) -> Link {
        let Some(assoc) = model_id
            .and_then(|m| self.points.get(m))
            .and_then(|p| p.assoc)
        else {
            return Link::None;
        };
        let Some(a) = self.points.get(assoc) else {
            return Link::None;
        };
        if shown.is_empty() {
            return Link::None;
        }
        if a.paras == shown {
            return Link::One(assoc.to_owned());
        }
        let mut order = Vec::new();
        self.subtree(assoc, &mut order);
        let mut at = 0;
        let mut out = Vec::new();
        for id in order {
            let Some(p) = self.points.get(id) else {
                continue;
            };
            if p.paras.is_empty() || p.paras.iter().all(String::is_empty) {
                continue;
            }
            let n = p.paras.len();
            if shown.get(at..at + n) != Some(&p.paras[..]) {
                continue;
            }
            out.push((id.to_owned(), n));
            at += n;
            if at == shown.len() {
                return Link::Many(out);
            }
        }
        Link::None
    }
}

/// The drawing part of SmartArt `d` on `slide`, its XML and relationships.
pub(crate) fn drawing_source(
    pres: &Presentation,
    slide: &Slide,
    d: &DiagramRef,
) -> Option<(String, String, Vec<Rel>)> {
    let data = d.data.as_deref()?;
    let bytes = pres.part_bytes(data)?;
    let xml = std::str::from_utf8(&bytes).ok()?;
    let doc = roxmltree::Document::parse(xml).ok()?;
    let rel = doc
        .descendants()
        .find(|n| is(*n, NS_DSP, "dataModelExt"))
        .and_then(|n| n.attribute("relId"))?;
    let drawing = slide
        .rels
        .iter()
        .find(|r| r.id == rel && r.ty.ends_with("/diagramDrawing"))
        .and_then(|r| r.part(&slide.part))?;
    let dbytes = pres.part_bytes(&drawing)?;
    let dxml = String::from_utf8(dbytes.to_vec()).ok()?;
    let rels = pres
        .part_bytes(&rels_name(&drawing))
        .and_then(|b| String::from_utf8(b.to_vec()).ok())
        .map(|x| parse_rels(&x))
        .unwrap_or_default();
    Some((drawing, dxml, rels))
}

impl Presentation {
    /// Read every SmartArt drawing on the slides into the model (SmartArt
    /// inside groups too).
    pub(crate) fn with_smartart(mut self) -> Self {
        for i in 0..self.slides.len() {
            let slide = self.slides[i].clone();
            if slide.locked.is_some() {
                continue;
            }
            let loaded: Vec<Option<Shape>> = slide
                .shapes
                .iter()
                .map(|sh| {
                    let mut sh = sh.clone();
                    self.attach_nested(&slide, &mut sh, i).then_some(sh)
                })
                .collect();
            if loaded.iter().all(Option::is_none) {
                continue;
            }
            let s = Arc::make_mut(&mut self.slides[i]);
            let mut src_shapes = s.src.shapes.clone();
            for (k, sh) in loaded.into_iter().enumerate() {
                let Some(sh) = sh else { continue };
                if let Some(o) = src_shapes.get_mut(k) {
                    *o = sh.clone();
                }
                s.shapes[k] = sh;
            }
            s.src = Arc::new(crate::presentation::SlideSrc {
                prefix: s.src.prefix.clone(),
                suffix: s.src.suffix.clone(),
                shapes: src_shapes,
                rels: s.src.rels.clone(),
            });
        }
        self
    }

    /// Read the drawings of SmartArt frame `shape`, or of the SmartArt in
    /// group `shape` (at any depth). Loading isn't an edit: the groups'
    /// originals take the loaded frames too. Whether anything was read.
    pub(crate) fn attach_nested(&self, slide: &Slide, shape: &mut Shape, index: usize) -> bool {
        if shape.is_smartart() {
            self.attach_drawing(slide, shape, index);
            return shape.smartart().is_some();
        }
        if !shape.is_group() {
            return false;
        }
        let Some(g) = crate::group::inner_shapes_mut(shape) else {
            return false;
        };
        let mut any = false;
        for c in &mut g.children {
            any |= self.attach_nested(slide, c, index);
        }
        if any
            && let ShapeKind::Other { frame, .. } = &shape.kind
            && let Some(src) = &shape.src
        {
            let mut src2 = (**src).clone();
            src2.frame = frame.clone();
            shape.src = Some(Arc::new(src2));
        }
        any
    }

    /// Read the drawing of SmartArt frame `shape` (on `slide`, with its
    /// relationships) into it.
    pub(crate) fn attach_drawing(&self, slide: &Slide, shape: &mut Shape, index: usize) {
        let colors = self.color_ctx(index);
        let ShapeKind::Other { frame, .. } = &mut shape.kind else {
            return;
        };
        let Some(FrameContent::Diagram(d)) = frame.get() else {
            return;
        };
        let Some(size) = shape.xfrm else { return };
        let Some(dd) = self.load_drawing(slide, d, &colors, size) else {
            return;
        };
        let new = crate::frame::Frame::new(FrameContent::Diagram(DiagramRef {
            data: d.data.clone(),
            drawing: Some(Arc::new(dd)),
        }));
        *frame = new.clone();
        if let Some(src) = &shape.src {
            let mut src2 = (**src).clone();
            src2.frame = new;
            shape.src = Some(Arc::new(src2));
        }
    }

    fn load_drawing(
        &self,
        slide: &Slide,
        d: &DiagramRef,
        colors: &ColorCtx,
        frame: Xfrm,
    ) -> Option<DiagramDrawing> {
        let (part, xml, rels) = drawing_source(self, slide, d)?;
        let data_part = d.data.as_deref()?;
        let data_bytes = self.part_bytes(data_part)?;
        let data_xml = std::str::from_utf8(&data_bytes).ok()?;
        read_drawing(
            &part,
            &xml,
            &rels,
            data_part,
            data_xml,
            colors,
            (frame.cx, frame.cy),
        )
    }

    /// The SmartArt drawing `r` is in, and the shape's index in it.
    fn smartart_of(&self, r: ShapeRef) -> Option<(&DiagramDrawing, usize)> {
        if r.is_top() {
            return None;
        }
        let parent = r.parent();
        let dd = self.shape(parent)?.smartart()?;
        Some((dd, r.2.last()?))
    }

    /// For text edits of SmartArt shape `r`: an error when its text can't
    /// be edited, the paragraph count it must keep when it shows several
    /// items, `None` otherwise (and for any other shape).
    pub(crate) fn smartart_fixed_paras(&self, r: ShapeRef) -> anyhow::Result<Option<usize>> {
        let Some((dd, i)) = self.smartart_of(r) else {
            return Ok(None);
        };
        match dd.fixed_paras(i) {
            Ok(n) => Ok(n),
            Err(e) => bail!("{e}"),
        }
    }

    /// Whether the text of SmartArt shape `r` can be edited.
    pub fn smartart_text_editable(&self, r: impl Into<ShapeRef>) -> bool {
        self.smartart_fixed_paras(r.into()).is_ok()
    }

    /// Drawing and data parts of SmartArt edited in Kantor, as they are saved.
    pub(crate) fn smartart_parts(&self) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        for (i, s) in self.slides.iter().enumerate() {
            // SmartArt on the slide and inside groups.
            let mut stack: Vec<&Shape> = s.shapes.iter().collect();
            while let Some(sh) = stack.pop() {
                if let Some(g) = sh.group() {
                    stack.extend(g.children.iter());
                    continue;
                }
                let ShapeKind::Other { frame, .. } = &sh.kind else {
                    continue;
                };
                let Some(FrameContent::Diagram(d)) = frame.get() else {
                    continue;
                };
                let Some(dd) = &d.drawing else { continue };
                let changed = dd.changed();
                if changed.is_empty() {
                    continue;
                }
                out.push((
                    dd.part.clone(),
                    dd.drawing_with_edits(&changed).into_bytes(),
                ));
                if let Some(data) = &d.data
                    && let Some(bytes) = self.part_bytes(data)
                    && let Ok(text) = std::str::from_utf8(&bytes)
                    && let Some(new) = data_with_edits(&self.color_ctx(i), data, text, dd, &changed)
                {
                    out.push((data.clone(), new.into_bytes()));
                }
            }
        }
        out
    }

    /// PowerPoint's *Convert to Shapes*: SmartArt `r` becomes a group of
    /// ordinary shapes from its drawing (edits included). Returns the
    /// group's index.
    pub fn convert_to_shapes(&mut self, r: impl Into<ShapeRef>) -> anyhow::Result<usize> {
        let r: ShapeRef = r.into();
        if !r.is_top() {
            bail!("Select the SmartArt to convert it");
        }
        let s = self.slides.get(r.0).context("no such slide")?.clone();
        if let Some(why) = &s.locked {
            bail!("This slide can't be edited: {why}");
        }
        let sh = s.shapes.get(r.1).context("no such shape")?;
        let dd = sh
            .smartart()
            .context("Only SmartArt with a saved drawing can be converted to shapes")?;
        let fx = sh.xfrm.context("This SmartArt has no position")?;
        // The drawing's relationships (pictures) become the slide's.
        let ddoc = roxmltree::Document::parse(&dd.xml).context("SmartArt drawing")?;
        let drawing_ns: Vec<(String, String)> = ddoc
            .root_element()
            .namespaces()
            .filter(|n| n.name().is_some_and(|p| p != "xml"))
            .map(|n| {
                (
                    format!("xmlns:{}", n.name().unwrap_or_default()),
                    n.uri().to_owned(),
                )
            })
            .collect();
        let drawing_rels = self
            .part_bytes(&rels_name(&dd.part))
            .and_then(|b| String::from_utf8(b.to_vec()).ok())
            .map(|x| parse_rels(&x))
            .unwrap_or_default();
        let mut rels = (*s.rels).clone();
        let gid = s.next_shape_id();
        let mut xml = format!(
            "<p:grpSp><p:nvGrpSpPr><p:cNvPr id=\"{gid}\" name=\"Group {}\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr>",
            gid - 1
        );
        crate::shape::write_xfrm_as(&mut xml, "a:xfrm", "", &fx, Some(&dd.group.child_box));
        xml.push_str("</p:grpSpPr>");
        let mut id = gid;
        for (k, c) in dd.group.children.iter().enumerate() {
            let mut raw = String::new();
            write_shape(&mut raw, c);
            id += 1;
            let tb = dd.group.text_boxes.get(k).copied().flatten();
            xml.push_str(&to_slide_shape(
                &raw,
                &drawing_ns,
                id,
                c,
                tb,
                &drawing_rels,
                &dd.part,
                &s.part,
                &mut rels,
            )?);
        }
        xml.push_str("</p:grpSp>");
        let group = self.parse_new_with(r.0, &xml, &rels)?;
        // The SmartArt's parts are dropped from the slide's relationships
        // (and from the file on save, unless used elsewhere).
        let frame_raw = sh.src.as_ref().map(|x| x.raw.clone()).unwrap_or_default();
        let mut drop_ids: Vec<String> = Vec::new();
        if let Ok(fdoc) = roxmltree::Document::parse(&wrap(&clip_ns(&s), &frame_raw).0) {
            for a in fdoc.descendants().flat_map(|n| n.attributes()) {
                if a.namespace() == Some(NS_R) {
                    drop_ids.push(a.value().to_owned());
                }
            }
        }
        if let Some(r) = rels
            .iter()
            .find(|x| x.kind() == "diagramDrawing" && x.part(&s.part).as_deref() == Some(&dd.part))
        {
            drop_ids.push(r.id.clone());
        }
        let users = |id: &str| {
            s.shapes
                .iter()
                .enumerate()
                .filter(|(k, _)| *k != r.1)
                .any(|(_, o)| {
                    o.src
                        .as_ref()
                        .is_some_and(|x| x.raw.contains(&format!("\"{id}\"")))
                })
        };
        rels.retain(|x| !(drop_ids.contains(&x.id) && is_diagram_rel(x.kind()) && !users(&x.id)));
        self.record(Kind::Other, None, Focus::at(r), false, |p| {
            let s = p.slide_mut(r.0)?;
            s.rels = Arc::new(rels);
            s.shapes[r.1] = group;
            Ok((r.1, Focus::at(r)))
        })
    }
}

/// The data model `xml` with the edited shapes' text and formatting.
fn data_with_edits(
    colors: &ColorCtx,
    part: &str,
    xml: &str,
    dd: &DiagramDrawing,
    changed: &[usize],
) -> Option<String> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let cx = ParseCtx {
        src: xml,
        part,
        rels: &[],
        colors: *colors,
    };
    let model = Model::read(&doc, &cx);
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    for &k in changed {
        let c = &dd.group.children[k];
        // Text.
        if let Some(t) = &c.text {
            let parts: Vec<(&str, &[TextPara])> = match dd.links.get(k) {
                Some(Link::One(id)) => vec![(id.as_str(), &t.paras[..])],
                Some(Link::Many(v)) if v.iter().map(|x| x.1).sum::<usize>() == t.paras.len() => {
                    let mut at = 0;
                    v.iter()
                        .map(|(id, n)| {
                            let s = &t.paras[at..at + n];
                            at += n;
                            (id.as_str(), s)
                        })
                        .collect()
                }
                _ => Vec::new(),
            };
            let unchanged_text = c
                .src
                .as_ref()
                .is_some_and(|s| s.text.as_ref() == Some(&t.paras));
            if !unchanged_text {
                for (id, paras) in parts {
                    if let Some(p) = model.points.get(id) {
                        edits.extend(point_text_edits(xml, p.node, paras, &cx));
                    }
                }
            }
        }
        // A move or resize: PowerPoint's custom position and size on the
        // presentation item, so its layout puts the shape there too.
        if let (Some(new), Some(old)) = (c.xfrm, c.src.as_ref().and_then(|o| o.xfrm))
            && new != old
            && let Some(id) = dd.group.model_ids.get(k).cloned().flatten()
            && let Some(p) = model.points.get(id.as_str())
        {
            edits.extend(point_layout_edits(xml, p.node, &old, &new));
        }
        // Fill and outline go on the presentation item the shape draws.
        if !c.paint.is_empty()
            && let Some(id) = dd.group.model_ids.get(k).cloned().flatten()
            && let Some(p) = model.points.get(id.as_str())
        {
            edits.extend(point_paint_edits(xml, p.node, &c.paint));
        }
    }
    if edits.is_empty() {
        return None;
    }
    edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
    let mut out = xml.to_owned();
    let mut last = usize::MAX;
    for (r, t) in edits {
        // Edits never overlap; a defensive skip if they ever did.
        if r.end > last {
            continue;
        }
        last = r.start;
        out.replace_range(r, &t);
    }
    Some(out)
}

/// A SmartArt drawing (`dsp:drawing`, part `part` with relationships
/// `rels`) read with its data model (`data_part`, `data_xml`) for a frame
/// of `frame` EMU: Kantor Docs reads Word's SmartArt with it, as Slides
/// does PowerPoint's.
pub fn read_drawing(
    part: &str,
    xml: &str,
    rels: &[Rel],
    data_part: &str,
    data_xml: &str,
    colors: &ColorCtx,
    frame: (Emu, Emu),
) -> Option<DiagramDrawing> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let tree = doc
        .root_element()
        .children()
        .find(|n| is(*n, NS_DSP, "spTree"))?;
    let cx = ParseCtx {
        src: xml,
        part,
        rels,
        colors: *colors,
    };
    let mut group = parse_group(tree, &cx, None, None, None);
    group.child_box = Xfrm::new(0, 0, frame.0, frame.1);
    let ranges = tree
        .children()
        .filter(is_group_child)
        .map(|n| n.range())
        .collect();
    // Links to the data model.
    let data_doc = roxmltree::Document::parse(data_xml).ok()?;
    let dcx = ParseCtx {
        src: data_xml,
        part: data_part,
        rels: &[],
        colors: *colors,
    };
    let model = Model::read(&data_doc, &dcx);
    let links = group
        .children
        .iter()
        .zip(&group.model_ids)
        .map(|(c, m)| {
            let shown: Vec<String> = c
                .text
                .as_ref()
                .map(|t| t.paras.iter().map(|p| p.text.clone()).collect())
                .unwrap_or_default();
            model.link(m.as_deref(), &shown)
        })
        .collect();
    drop(data_doc);
    Some(DiagramDrawing {
        part: part.to_owned(),
        group,
        xml: Arc::from(xml),
        ranges,
        links,
    })
}

/// The relationship id (`dsp:dataModelExt/@relId`) of the drawing a
/// SmartArt data model names: a relationship of the part holding the
/// SmartArt frame.
pub fn drawing_rel_id(data_xml: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(data_xml).ok()?;
    doc.descendants()
        .find(|n| is(*n, NS_DSP, "dataModelExt"))
        .and_then(|n| n.attribute("relId"))
        .map(str::to_owned)
}

/// Why a SmartArt shape showing several items can't take text with
/// another number of lines.
pub const SMARTART_LINES: &str = "This SmartArt shape shows several items: keep one line per item";

impl DiagramDrawing {
    /// Whether shape `k`'s text can be edited: an error when Kantor can't
    /// tell which data model item it belongs to, else the paragraph count
    /// it must keep (a shape showing several items) or `None`.
    pub fn fixed_paras(&self, k: usize) -> Result<Option<usize>, &'static str> {
        match self.links.get(k) {
            Some(Link::One(_)) => Ok(None),
            Some(Link::Many(v)) => Ok(Some(v.iter().map(|x| x.1).sum())),
            _ => Err(
                "This SmartArt text can't be edited: Kantor can't tell which item it belongs to",
            ),
        }
    }

    /// Give shape `k` the text `text` (`\n` between paragraphs): each
    /// paragraph keeps the formatting of the one it replaces (the last
    /// one's for added paragraphs) and of its first run. Whether it
    /// changed.
    pub fn set_plain_text(&mut self, k: usize, text: &str) -> Result<bool, &'static str> {
        let fixed = self.fixed_paras(k)?;
        let lines: Vec<&str> = text.split('\n').collect();
        if fixed.is_some_and(|n| n != lines.len()) {
            return Err(SMARTART_LINES);
        }
        let Some(shape) = self.group.children.get_mut(k) else {
            return Ok(false);
        };
        let Some(body) = shape.text.as_mut() else {
            return Ok(false);
        };
        let old: Vec<&str> = body.paras.iter().map(|p| p.text.as_str()).collect();
        if old == lines {
            return Ok(false);
        }
        let paras: Vec<TextPara> = lines
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let base = body.paras.get(i).or(body.paras.last());
                let run = base
                    .and_then(|p| p.runs.first())
                    .map(|r| r.props.clone())
                    .unwrap_or_default();
                let mut p = TextPara::styled(*line, run);
                if let Some(b) = base {
                    p.props = b.props.clone();
                    p.end = b.end.clone();
                }
                p
            })
            .collect();
        body.paras = paras;
        Ok(true)
    }

    /// The shapes changed since it was read.
    pub fn changed(&self) -> Vec<usize> {
        (0..self.group.children.len())
            .filter(|&k| !self.group.children[k].unchanged())
            .collect()
    }

    /// The drawing part's XML with shapes `changed` written in.
    pub fn drawing_with_edits(&self, changed: &[usize]) -> String {
        let mut xml = self.xml.to_string();
        for &k in changed.iter().rev() {
            let Some(r) = self.ranges.get(k) else {
                continue;
            };
            let mut buf = String::new();
            let c = &self.group.children[k];
            write_shape(&mut buf, c);
            // A moved or resized shape's text box went with it.
            if c.src.as_ref().is_some_and(|o| o.xfrm != c.xfrm)
                && let Some(Some(tb)) = self.group.text_boxes.get(k)
            {
                set_tx_xfrm(&mut buf, tb);
            }
            xml.replace_range(r.clone(), &buf);
        }
        xml
    }

    /// The drawing and data model parts as saved after edits: the new
    /// drawing XML, and the data model `data_xml` (part `data_part`) with
    /// the edited text, when it changed.
    pub fn saved_parts(
        &self,
        colors: &ColorCtx,
        data_part: &str,
        data_xml: &str,
    ) -> (String, Option<String>) {
        let changed = self.changed();
        (
            self.drawing_with_edits(&changed),
            data_with_edits(colors, data_part, data_xml, self, &changed),
        )
    }
}

/// The slide's namespace declarations.
fn clip_ns(s: &Slide) -> Vec<(String, String)> {
    crate::clip::root_ns(&s.src.prefix)
}

/// `xml` wrapped in an element declaring `ns`.
fn wrap(ns: &[(String, String)], xml: &str) -> (String, usize) {
    crate::clip::wrap(ns, xml)
}

/// A drawing shape (`dsp:sp`, as it would be saved) as a slide shape
/// (`p:sp`): new id and name, `p:nvPr`, no `dsp:txXfrm` (its offset from
/// the shape's box goes into the text insets), picture references moved
/// to the slide's relationships.
#[allow(clippy::too_many_arguments)]
fn to_slide_shape(
    raw: &str,
    ns: &[(String, String)],
    id: u32,
    shape: &Shape,
    text_box: Option<Xfrm>,
    drawing_rels: &[Rel],
    drawing_part: &str,
    slide_part: &str,
    rels: &mut Vec<Rel>,
) -> anyhow::Result<String> {
    let (wrapped, start) = wrap(ns, raw);
    let doc = roxmltree::Document::parse(&wrapped).context("SmartArt shape XML")?;
    let el = doc
        .root_element()
        .first_element_child()
        .context("SmartArt shape")?;
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    for n in el.descendants().filter(|n| n.is_element()) {
        let r = n.range();
        let tag = start_tag(&wrapped, n);
        let name = tag_name(tag);
        let local = n.tag_name().name();
        let in_dsp = n.tag_name().namespace() == Some(NS_DSP);
        if in_dsp && local == "txXfrm" {
            edits.push((r, String::new()));
            continue;
        }
        if in_dsp {
            // Rename start and end tags to `p:`; attributes as they were.
            let mut attrs = split_attrs(tag_attrs(tag));
            attrs.retain(|(k, _)| k != "modelId");
            if local == "cNvPr" {
                attrs.retain(|(k, _)| k != "id" && k != "name");
                attrs.insert(0, ("name".into(), format!("Shape {}", id - 1)));
                attrs.insert(0, ("id".into(), id.to_string()));
            }
            let mut st = format!("<p:{local}");
            write_attrs(&mut st, &attrs);
            let close = tag.ends_with("/>");
            st.push_str(if close { "/>" } else { ">" });
            if close {
                if local.starts_with("cNv") && local != "cNvPr" {
                    st.push_str("<p:nvPr/>");
                }
                edits.push((r.start..r.start + tag.len(), st));
            } else {
                edits.push((r.start..r.start + tag.len(), st));
                let end_tag = format!("</{name}>");
                let mut end = format!("</p:{local}>");
                if local.starts_with("cNv") && local != "cNvPr" {
                    end.push_str("<p:nvPr/>");
                }
                edits.push((r.end - end_tag.len()..r.end, end));
            }
            continue;
        }
        if n.tag_name().namespace() == Some(NS_A) && local == "bodyPr" {
            if let (Some(tb), Some(x)) = (text_box, shape.xfrm)
                && tb.rot == x.rot
            {
                let mut attrs = split_attrs(tag_attrs(tag));
                let ins = |k: &str, d: i64| {
                    attrs_value(&attrs, k)
                        .and_then(|v| v.parse::<i64>().ok())
                        .unwrap_or(d)
                };
                let l = ins("lIns", 91_440) + (tb.x - x.x).max(0);
                let t = ins("tIns", 45_720) + (tb.y - x.y).max(0);
                let rr = ins("rIns", 91_440) + ((x.x + x.cx) - (tb.x + tb.cx)).max(0);
                let b = ins("bIns", 45_720) + ((x.y + x.cy) - (tb.y + tb.cy)).max(0);
                attrs.retain(|(k, _)| !matches!(k.as_str(), "lIns" | "tIns" | "rIns" | "bIns"));
                for (k, v) in [("lIns", l), ("tIns", t), ("rIns", rr), ("bIns", b)] {
                    attrs.push((k.into(), v.to_string()));
                }
                let mut st = format!("<{name}");
                write_attrs(&mut st, &attrs);
                st.push_str(if tag.ends_with("/>") { "/>" } else { ">" });
                edits.push((r.start..r.start + tag.len(), st));
            }
            continue;
        }
        // Relationship references (pictures) move to the slide.
        for a in n.attributes() {
            if a.namespace() != Some(NS_R) {
                continue;
            }
            let rel = drawing_rels
                .iter()
                .find(|x| x.id == a.value())
                .context("A SmartArt picture is missing")?;
            if rel.kind() != "image" {
                bail!(
                    "This SmartArt has content Kantor can't convert ({})",
                    rel.kind()
                );
            }
            let target = match rel.part(drawing_part) {
                Some(p) => relative(slide_part, &p),
                None => rel.target.clone(),
            };
            let new_id = next_rel_id(rels);
            rels.push(Rel {
                id: new_id.clone(),
                ty: rel.ty.clone(),
                target,
                external: rel.external,
            });
            edits.push((a.range_value(), new_id));
        }
    }
    // Nested edits (a renamed element's start tag and an attribute in it)
    // can't both apply: the renamed tag already carries its attributes, so
    // attribute edits inside a replaced start tag are dropped.
    edits.sort_by_key(|(r, _)| (std::cmp::Reverse(r.start), r.end));
    let mut out = wrapped.clone();
    let mut floor = usize::MAX;
    for (r, t) in edits {
        if r.end > floor {
            continue;
        }
        floor = r.start;
        out.replace_range(r, &t);
    }
    let end = out.len() - "</w>".len();
    Ok(out[start..end].to_owned())
}

fn attrs_value<'a>(attrs: &'a [(String, String)], k: &str) -> Option<&'a str> {
    attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
}

/// The qualified-name prefix of an element as written (`dgm` in `<dgm:pt`).
fn prefix_of(xml: &str, n: Node) -> String {
    let name = tag_name(start_tag(xml, n));
    name.split_once(':')
        .map(|(p, _)| format!("{p}:"))
        .unwrap_or_default()
}

/// Edits that give data model item `pt` the text `paras` (from a drawing
/// shape): each paragraph keeps the item's own paragraph formatting; runs
/// keep their character formatting but not the drawing's fitted sizes.
fn point_text_edits(
    xml: &str,
    pt: Node,
    paras: &[TextPara],
    cx: &ParseCtx,
) -> Vec<(Range<usize>, String)> {
    let mut edits = Vec::new();
    let t = child(pt, NS_DGM, "t");
    let orig: Vec<TextPara> = t
        .map(|t| parse_text_body(t, cx).0.paras)
        .unwrap_or_default();
    let base_run = orig
        .first()
        .and_then(|p| p.runs.first())
        .map(|r| r.props.clone())
        .unwrap_or_default();
    let mut body = String::new();
    for (k, p) in paras.iter().enumerate() {
        let base = orig.get(k).or(orig.last());
        let mut np = p.clone();
        if let Some(b) = base {
            np.props = b.props.clone();
            np.end = b.end.clone();
        } else {
            np.props = ParaProps::default();
            np.end = None;
        }
        for r in &mut np.runs {
            if base_run.size.is_none() {
                r.props.size = None;
            }
            if !base_run.extra.attrs.iter().any(|(k, _)| k == "kern") {
                r.props.extra.attrs.retain(|(k, _)| k != "kern");
            }
        }
        np.normalize();
        write_para(&mut body, &np);
    }
    let dgm = prefix_of(xml, pt);
    match t {
        Some(t) => {
            let ps: Vec<Node> = children(t, NS_A, "p").collect();
            let r = match (ps.first(), ps.last()) {
                (Some(a), Some(b)) => a.range().start..b.range().end,
                _ => {
                    let tag = start_tag(xml, t);
                    if tag.ends_with("/>") {
                        let mut full = format!("<{dgm}t><a:bodyPr/><a:lstStyle/>");
                        full.push_str(&body);
                        let _ = write!(full, "</{dgm}t>");
                        edits.push((t.range(), full));
                        body.clear();
                        t.range().end..t.range().end
                    } else {
                        let at = t.range().end - format!("</{dgm}t>").len();
                        at..at
                    }
                }
            };
            if !body.is_empty() {
                edits.push((r, body));
            }
        }
        None => {
            let after = child(pt, NS_DGM, "spPr")
                .or_else(|| child(pt, NS_DGM, "prSet"))
                .map(|n| n.range().end);
            let tag = start_tag(xml, pt);
            let at = after.unwrap_or(pt.range().start + tag.len());
            let mut full = format!("<{dgm}t><a:bodyPr/><a:lstStyle/>");
            full.push_str(&body);
            let _ = write!(full, "</{dgm}t>");
            if tag.ends_with("/>") {
                // `<dgm:pt .../>`: give it content.
                let name = tag_name(tag);
                let mut st = format!("<{name}");
                write_attrs(&mut st, &split_attrs(tag_attrs(tag)));
                let _ = write!(st, ">{full}</{name}>");
                edits.push((pt.range(), st));
            } else {
                edits.push((at..at, full));
            }
        }
    }
    // The item isn't an empty placeholder any more.
    if let Some(ps) = child(pt, NS_DGM, "prSet")
        && ps.attribute("phldr").is_some()
        && paras.iter().any(|p| !p.text.is_empty())
    {
        let tag = start_tag(xml, ps);
        let mut attrs = split_attrs(tag_attrs(tag));
        attrs.retain(|(k, _)| k != "phldr");
        let mut st = format!("<{}", tag_name(tag));
        write_attrs(&mut st, &attrs);
        st.push_str(if tag.ends_with("/>") { "/>" } else { ">" });
        let r = ps.range();
        edits.push((r.start..r.start + tag.len(), st));
    }
    edits
}

/// Give the `dsp:txXfrm` of a written drawing shape the box `tb`.
fn set_tx_xfrm(shape: &mut String, tb: &Xfrm) {
    let Some(open) = shape.find(":txXfrm").and_then(|i| shape[..i].rfind('<')) else {
        return;
    };
    let Some(open_end) = tag_end(&shape[open..]).map(|e| open + e) else {
        return;
    };
    if shape[..open_end].ends_with("/>") {
        return;
    }
    let Some(close) = shape[open_end..].find("txXfrm>").and_then(|i| {
        let at = open_end + i;
        shape[..at].rfind("</")
    }) else {
        return;
    };
    // The drawingml prefix the box is written with (`a:`).
    let inner = &shape[open_end..close];
    let a = inner
        .find("off ")
        .map(|i| inner[..i].trim_start_matches('<').to_owned())
        .unwrap_or_else(|| "a:".into());
    let body = format!(
        r#"<{a}off x="{}" y="{}"/><{a}ext cx="{}" cy="{}"/>"#,
        tb.x, tb.y, tb.cx, tb.cy
    );
    shape.replace_range(open_end..close, &body);
}

/// Edits recording on presentation item `pt` (`dgm:prSet`) that its shape
/// moved from `old` to `new` (drawing space), as PowerPoint does when a
/// SmartArt shape is dragged or resized: `custLinFactNeighborX/Y` (the
/// offset of its centre in 100 000ths of its width and height) and
/// `custScaleX/Y` (its size, 100 000 = the layout's).
fn point_layout_edits(xml: &str, pt: Node, old: &Xfrm, new: &Xfrm) -> Vec<(Range<usize>, String)> {
    let pr = child(pt, NS_DGM, "prSet");
    let mut attrs = pr
        .map(|n| split_attrs(tag_attrs(start_tag(xml, n))))
        .unwrap_or_default();
    let get = |attrs: &[(String, String)], k: &str| {
        attrs
            .iter()
            .find(|(n, _)| n == k)
            .and_then(|(_, v)| v.parse::<i64>().ok())
    };
    let ratio = |a: Emu, b: Emu| if b == 0 { 1.0 } else { a as f64 / b as f64 };
    let (ocx, ocy) = old.center();
    let (ncx, ncy) = new.center();
    let set = |attrs: &mut Vec<(String, String)>, k: &str, v: i64| match attrs
        .iter_mut()
        .find(|(n, _)| n == k)
    {
        Some(a) => a.1 = v.to_string(),
        None => attrs.push((k.to_owned(), v.to_string())),
    };
    if new.cx != old.cx || new.cy != old.cy {
        let sx = get(&attrs, "custScaleX").unwrap_or(100_000) as f64 * ratio(new.cx, old.cx);
        let sy = get(&attrs, "custScaleY").unwrap_or(100_000) as f64 * ratio(new.cy, old.cy);
        set(&mut attrs, "custScaleX", sx.round() as i64);
        set(&mut attrs, "custScaleY", sy.round() as i64);
    }
    if (ncx - ocx).abs() > 0.5 || (ncy - ocy).abs() > 0.5 {
        let fx = get(&attrs, "custLinFactNeighborX").unwrap_or(0) as f64
            + (ncx - ocx) / old.cx.max(1) as f64 * 100_000.0;
        let fy = get(&attrs, "custLinFactNeighborY").unwrap_or(0) as f64
            + (ncy - ocy) / old.cy.max(1) as f64 * 100_000.0;
        set(&mut attrs, "custLinFactNeighborX", fx.round() as i64);
        set(&mut attrs, "custLinFactNeighborY", fy.round() as i64);
    }
    let dgm = prefix_of(xml, pt);
    let mut tag = format!("<{dgm}prSet");
    write_attrs(&mut tag, &attrs);
    match pr {
        Some(n) => {
            let old_tag = start_tag(xml, n);
            tag.push_str(if old_tag.ends_with("/>") { "/>" } else { ">" });
            let st = n.range().start;
            vec![(st..st + old_tag.len(), tag)]
        }
        None => {
            let pt_tag = start_tag(xml, pt);
            if pt_tag.ends_with("/>") {
                return Vec::new();
            }
            tag.push_str("/>");
            let at = pt.range().start + pt_tag.len();
            vec![(at..at, tag)]
        }
    }
}

/// Edits that put a fill / outline change on data model item `pt`'s
/// `dgm:spPr`.
fn point_paint_edits(xml: &str, pt: Node, paint: &PaintEdit) -> Vec<(Range<usize>, String)> {
    match child(pt, NS_DGM, "spPr") {
        Some(sp) => {
            let at = sppr_at(xml, sp, 0);
            sppr_paint_edits(&at, paint, None)
                .into_iter()
                .map(|(r, t, _)| (r, t))
                .collect()
        }
        None => {
            let dgm = prefix_of(xml, pt);
            let mut buf = format!("<{dgm}spPr>");
            crate::shape::write_new_paint(&mut buf, paint);
            let _ = write!(buf, "</{dgm}spPr>");
            let tag = start_tag(xml, pt);
            if tag.ends_with("/>") {
                return Vec::new();
            }
            let at = child(pt, NS_DGM, "prSet")
                .map(|n| n.range().end)
                .unwrap_or(pt.range().start + tag.len());
            vec![(at..at, buf)]
        }
    }
}
