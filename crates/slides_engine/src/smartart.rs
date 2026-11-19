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
//! In the layouts Kantor lays out itself ([`crate::smartart_layout`]:
//! lists, bullet lists, processes, cycles) a resized SmartArt is laid out
//! again and items can be added and removed; the new drawing and data
//! model are kept on the frame ([`DiagramDrawing`]'s `data_xml`,
//! `rebuilt`) and saved. *Change Layout* draws any SmartArt again in one of
//! those layouts (with a layout definition for the layout part, kept as
//! `layout_xml`); other layouts are scaled. *Convert to Shapes* turns the
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
        // Shapes Kantor added name their data item itself (showing its
        // text, or its text and its sub-items').
        let direct = model_id
            .and_then(|m| self.points.get_key_value(m))
            .filter(|(_, p)| {
                p.assoc.is_none()
                    && p.node
                        .attribute("type")
                        .is_none_or(|t| t == "node" || t == "asst")
            })
            .map(|(k, _)| *k);
        if let Some(m) = direct
            && (shown.is_empty() || self.points[m].paras == shown)
        {
            return Link::One(m.to_owned());
        }
        let Some(assoc) = direct.or_else(|| {
            model_id
                .and_then(|m| self.points.get(m))
                .and_then(|p| p.assoc)
        }) else {
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
        match direct {
            Some(m) => Link::One(m.to_owned()),
            None => Link::None,
        }
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
                edited: s.src.edited,
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
            layout: d.layout.clone(),
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
}

impl Presentation {
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
                if !dd.needs_save() {
                    continue;
                }
                let Some((drawing, data)) = self.smartart_xml(i, d, dd) else {
                    continue;
                };
                out.push((dd.part.clone(), drawing.into_bytes()));
                if let (Some(part), Some(data)) = (&d.data, data) {
                    out.push((part.clone(), data.into_bytes()));
                }
                if let (Some(part), Some(lo)) = (&d.layout, &dd.layout_xml) {
                    out.push((part.clone(), lo.as_bytes().to_vec()));
                }
            }
        }
        out
    }

    /// The drawing XML of SmartArt `d` on slide `slide` with its edited
    /// shapes spliced in, and its data model with their text and
    /// formatting (`None` when the data model is as in the file).
    pub(crate) fn smartart_xml(
        &self,
        slide: usize,
        d: &DiagramRef,
        dd: &DiagramDrawing,
    ) -> Option<(String, Option<String>)> {
        let changed = dd.changed();
        let data = d.data.as_ref().and_then(|part| {
            let file = match &dd.data_xml {
                Some(_) => String::new(),
                None => String::from_utf8(self.part_bytes(part)?.to_vec()).ok()?,
            };
            dd.data_saved(&self.color_ctx(slide), part, &file, &changed)
        });
        Some((dd.drawing_with_edits(&changed), data))
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
        data_xml: None,
        rebuilt: false,
        layout_xml: None,
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

    /// Whether saving writes this SmartArt's parts: shapes were edited,
    /// or Kantor laid it out again or changed its items.
    pub fn needs_save(&self) -> bool {
        self.rebuilt || self.data_xml.is_some() || !self.changed().is_empty()
    }

    /// The data model as saved with shapes `changed` edited: Kantor's own
    /// (items added or removed) or else `file_xml` (part `data_part`, as
    /// read) with their text and formatting; `None` when it is as in the
    /// file.
    pub fn data_saved(
        &self,
        colors: &ColorCtx,
        data_part: &str,
        file_xml: &str,
        changed: &[usize],
    ) -> Option<String> {
        let base = self.data_xml.as_deref().unwrap_or(file_xml);
        match data_with_edits(colors, data_part, base, self, changed) {
            Some(new) => Some(new),
            None => self.data_xml.is_some().then(|| base.to_owned()),
        }
    }

    /// The drawing and data model parts as saved after edits: the new
    /// drawing XML, and the data model (`data_xml`, part `data_part`, as
    /// read, unless Kantor changed its items) with the edited text, when
    /// it changed.
    pub fn saved_parts(
        &self,
        colors: &ColorCtx,
        data_part: &str,
        data_xml: &str,
    ) -> (String, Option<String>) {
        let changed = self.changed();
        (
            self.drawing_with_edits(&changed),
            self.data_saved(colors, data_part, data_xml, &changed),
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
    // Its turn (`rot` on the start tag).
    let tag = &shape[open..open_end];
    let mut attrs = split_attrs(tag_attrs(tag));
    attrs.retain(|(k, _)| k != "rot");
    if tb.rot != 0 {
        attrs.insert(0, ("rot".into(), tb.rot.to_string()));
    }
    let mut new = format!("<{}", tag_name(tag));
    write_attrs(&mut new, &attrs);
    new.push('>');
    shape.replace_range(open..open_end, &new);
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
    // A turn: PowerPoint's custom angle on the presentation item.
    if new.rot != old.rot {
        let ang = get(&attrs, "custAng").unwrap_or(0) + (new.rot - old.rot) as i64;
        set(
            &mut attrs,
            "custAng",
            ang.rem_euclid(360 * crate::model::ROT_PER_DEG as i64),
        );
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

// ── laying SmartArt out again, adding and removing items ───────────────────

use crate::smartart_layout::{AddPlace, Family, LAYOUT_REFUSAL, OrgLayout};

impl Presentation {
    /// The SmartArt frame `r` is, or is a shape of.
    fn smartart_frame_of(&self, r: ShapeRef) -> Option<ShapeRef> {
        if self.shape(r)?.smartart().is_some() {
            return Some(r);
        }
        if r.is_top() {
            return None;
        }
        let p = r.parent();
        self.shape(p)?.smartart().is_some().then_some(p)
    }

    /// The layout family of SmartArt `r` (the frame or one of its
    /// shapes), when Kantor lays it out itself.
    pub fn smartart_family(&self, r: impl Into<ShapeRef>) -> Option<Family> {
        let f = self.smartart_frame_of(r.into())?;
        let (d, dd) = diagram_of(self.shape(f)?)?;
        let (drawing, data) = self.smartart_xml(f.0, d, dd)?;
        let data = match data {
            Some(x) => x,
            None => String::from_utf8(self.part_bytes(d.data.as_deref()?)?.to_vec()).ok()?,
        };
        crate::smartart_layout::family(&drawing, &data)
    }

    /// SmartArt frame `f`'s content after `op` changed its parts (current
    /// drawing and data model XML in, new ones out), laid out in a
    /// `size` frame.
    fn smartart_redone(
        &self,
        f: ShapeRef,
        size: (Emu, Emu),
        op: impl FnOnce(&str, &str) -> anyhow::Result<(String, String)>,
    ) -> anyhow::Result<crate::frame::Frame> {
        let sh = self.shape(f).context("no such shape")?;
        let (d, dd) = diagram_of(sh).context("Select a SmartArt")?;
        let data_part = d.data.clone().context("SmartArt without data")?;
        let (drawing, data) = self.smartart_xml(f.0, d, dd).context("SmartArt drawing")?;
        let data = match data {
            Some(x) => x,
            None => String::from_utf8(
                self.part_bytes(&data_part)
                    .context("SmartArt data")?
                    .to_vec(),
            )?,
        };
        let (drawing, data) = op(&drawing, &data)?;
        let rels = self
            .part_bytes(&rels_name(&dd.part))
            .and_then(|b| String::from_utf8(b.to_vec()).ok())
            .map(|x| parse_rels(&x))
            .unwrap_or_default();
        let colors = self.color_ctx(f.0);
        let mut nd = read_drawing(&dd.part, &drawing, &rels, &data_part, &data, &colors, size)
            .context("SmartArt drawing")?;
        // Its text shrinks to fit the shapes as laid out now.
        let text = |sh: &Shape, x: &Xfrm, own: bool| self.shape_text(f.0, sh, x, own);
        if let Some(fitted) = crate::smartart_fit::fit_text(&nd, &text) {
            nd = read_drawing(&dd.part, &fitted, &rels, &data_part, &data, &colors, size)
                .context("SmartArt drawing")?;
        }
        nd.data_xml = Some(data.into());
        nd.rebuilt = true;
        nd.layout_xml = dd.layout_xml.clone();
        Ok(crate::frame::Frame::new(FrameContent::Diagram(
            DiagramRef {
                data: Some(data_part),
                layout: d.layout.clone(),
                drawing: Some(Arc::new(nd)),
            },
        )))
    }

    /// PowerPoint's *Layouts* gallery: SmartArt `r` (the frame or one of
    /// its shapes) drawn in layout `to`, its items, text and their
    /// formatting kept ([`crate::smartart_layout::change_layout`]). The
    /// layout part gets the new layout's definition on save.
    pub fn smartart_change_layout(
        &mut self,
        r: impl Into<ShapeRef>,
        to: Family,
    ) -> anyhow::Result<()> {
        let f = self
            .smartart_frame_of(r.into())
            .context("Select a SmartArt")?;
        let (d, _) = self
            .shape(f)
            .and_then(diagram_of)
            .context("Select a SmartArt")?;
        if d.layout.is_none() {
            bail!("This SmartArt has no layout part to change");
        }
        let size = self.smartart_size(f)?;
        let new = self.smartart_redone(f, size, |drawing, data| {
            let (d2, data2, _) =
                crate::smartart_layout::change_layout(drawing, data, to, size.0, size.1)?;
            Ok((d2, data2))
        })?;
        let Some(FrameContent::Diagram(nd)) = new.get() else {
            bail!("Select a SmartArt");
        };
        let mut dd = nd.drawing.as_deref().context("SmartArt drawing")?.clone();
        dd.layout_xml = Some(crate::smartart_layout::layout_def(to).into());
        let new = crate::frame::Frame::new(FrameContent::Diagram(DiagramRef {
            data: nd.data.clone(),
            layout: nd.layout.clone(),
            drawing: Some(Arc::new(dd)),
        }));
        self.set_frame(f, new, Focus::at(f))
    }

    /// SmartArt frame `f` laid out again for size `size`, if Kantor lays
    /// its layout out (`None`: scale it as before).
    pub(crate) fn smartart_relaid(
        &self,
        f: ShapeRef,
        size: (Emu, Emu),
    ) -> Option<crate::frame::Frame> {
        self.smartart_family(f)?;
        self.smartart_redone(f, size, |drawing, data| {
            Ok((
                crate::smartart_layout::relayout(drawing, data, size.0, size.1)?,
                data.to_owned(),
            ))
        })
        .ok()
    }

    fn set_frame(
        &mut self,
        f: ShapeRef,
        new: crate::frame::Frame,
        focus: Focus,
    ) -> anyhow::Result<()> {
        self.record(Kind::Other, None, focus.clone(), false, |p| {
            let s = p.slide_mut(f.0)?;
            let sh = s.shape_at_mut(f.1, f.2).context("no such shape")?;
            let ShapeKind::Other { frame, .. } = &mut sh.kind else {
                bail!("Select a SmartArt");
            };
            *frame = new;
            Ok(((), focus))
        })
    }

    fn smartart_size(&self, f: ShapeRef) -> anyhow::Result<(Emu, Emu)> {
        let x = self
            .shape(f)
            .and_then(|s| s.xfrm)
            .context("This SmartArt has no position")?;
        Ok((x.cx.max(1), x.cy.max(1)))
    }

    /// PowerPoint's *Add Shape After / Before / Above / Below* on SmartArt
    /// shape `r`: a new empty item next to the one `r` shows, laid out
    /// with the others. Returns the new item's shape.
    pub fn smartart_add_shape(
        &mut self,
        r: impl Into<ShapeRef>,
        place: AddPlace,
    ) -> anyhow::Result<ShapeRef> {
        let r: ShapeRef = r.into();
        let f = self
            .smartart_frame_of(r)
            .context("Select a SmartArt shape")?;
        if self.smartart_family(f).is_none() {
            bail!(LAYOUT_REFUSAL);
        }
        let dd = self
            .shape(f)
            .and_then(|s| s.smartart())
            .context("Select a SmartArt")?;
        // The selected shape's item (the frame alone: after the last one).
        let selected = if f == r {
            dd.group.model_ids.iter().rev().flatten().next().cloned()
        } else {
            dd.group
                .model_ids
                .get(r.2.last().unwrap_or(0))
                .cloned()
                .flatten()
        }
        .context("Select an item's shape to add a shape next to it")?;
        let size = self.smartart_size(f)?;
        let mut node = String::new();
        let new = self.smartart_redone(f, size, |drawing, data| {
            let (d2, data2, id) =
                crate::smartart_layout::add_item(drawing, data, &selected, place, size.0, size.1)?;
            node = id;
            Ok((d2, data2))
        })?;
        let at = match new.get() {
            Some(FrameContent::Diagram(DiagramRef {
                drawing: Some(dd), ..
            })) => dd
                .group
                .model_ids
                .iter()
                .position(|m| m.as_deref() == Some(&node)),
            _ => None,
        };
        let target = at.and_then(|i| f.child(i)).unwrap_or(f);
        self.set_frame(f, new, Focus::at(target))?;
        Ok(target)
    }

    /// The data item SmartArt shape `r` shows (not the frame).
    fn smartart_item_of(&self, r: ShapeRef) -> Option<(ShapeRef, String)> {
        let f = self.smartart_frame_of(r)?;
        if f == r {
            return None;
        }
        let dd = self.shape(f)?.smartart()?;
        let id = dd.group.model_ids.get(r.2.last().unwrap_or(0))?.clone()?;
        Some((f, id))
    }

    /// Whether SmartArt shape `r` shows an Organization Chart's item (its
    /// *Layout* and *Add Assistant* apply).
    pub fn smartart_is_org_item(&self, r: impl Into<ShapeRef>) -> bool {
        let r: ShapeRef = r.into();
        self.smartart_family(r) == Some(Family::OrgChart) && self.smartart_item_of(r).is_some()
    }

    /// How the Organization Chart item SmartArt shape `r` shows hangs its
    /// children: the layout set on it, else PowerPoint's initial one (the
    /// one the Layout menu marks). `None` when it isn't such an item.
    pub fn smartart_org_layout(&self, r: impl Into<ShapeRef>) -> Option<OrgLayout> {
        let (f, id) = self.smartart_item_of(r.into())?;
        let (d, dd) = diagram_of(self.shape(f)?)?;
        let (_, data) = self.smartart_xml(f.0, d, dd)?;
        let data = match data {
            Some(x) => x,
            None => String::from_utf8(self.part_bytes(d.data.as_deref()?)?.to_vec()).ok()?,
        };
        crate::smartart_layout::org_layout_shown(&data, &id)
    }

    /// PowerPoint's *Layout* on an Organization Chart shape `r`: how the
    /// children of the item it shows hang under it; laid out again.
    pub fn smartart_set_org_layout(
        &mut self,
        r: impl Into<ShapeRef>,
        layout: OrgLayout,
    ) -> anyhow::Result<()> {
        let r: ShapeRef = r.into();
        let (f, id) = self
            .smartart_item_of(r)
            .context("Select an Organization Chart shape")?;
        let size = self.smartart_size(f)?;
        let new = self.smartart_redone(f, size, |drawing, data| {
            crate::smartart_layout::set_org_layout(drawing, data, &id, layout, size.0, size.1)
        })?;
        // The same shape stays selected (its item keeps its place).
        let at = match new.get() {
            Some(FrameContent::Diagram(DiagramRef {
                drawing: Some(dd), ..
            })) => dd
                .group
                .model_ids
                .iter()
                .position(|m| m.as_deref() == Some(&id)),
            _ => None,
        };
        let target = at.and_then(|i| f.child(i)).unwrap_or(f);
        self.set_frame(f, new, Focus::at(target))
    }

    /// Delete SmartArt shape `r`: the item it shows goes (with its
    /// sub-items), and the others are laid out again.
    pub fn smartart_delete_shape(&mut self, r: impl Into<ShapeRef>) -> anyhow::Result<()> {
        let r: ShapeRef = r.into();
        let f = self
            .smartart_frame_of(r)
            .context("Select a SmartArt shape")?;
        if f == r {
            bail!("Select a SmartArt shape to delete it");
        }
        if self.smartart_family(f).is_none() {
            bail!(LAYOUT_REFUSAL);
        }
        let dd = self
            .shape(f)
            .and_then(|s| s.smartart())
            .context("Select a SmartArt")?;
        let selected = dd
            .group
            .model_ids
            .get(r.2.last().unwrap_or(0))
            .cloned()
            .flatten()
            .context("This SmartArt shape can't be deleted: it doesn't show an item")?;
        let size = self.smartart_size(f)?;
        let new = self.smartart_redone(f, size, |drawing, data| {
            crate::smartart_layout::remove_item(drawing, data, &selected, size.0, size.1)
        })?;
        self.set_frame(f, new, Focus::at(f))
    }
}

/// A SmartArt frame's data reference and drawing.
fn diagram_of(sh: &Shape) -> Option<(&DiagramRef, &DiagramDrawing)> {
    let ShapeKind::Other { frame, .. } = &sh.kind else {
        return None;
    };
    match frame.get()? {
        FrameContent::Diagram(d) => Some((d, d.drawing.as_deref()?)),
        _ => None,
    }
}
