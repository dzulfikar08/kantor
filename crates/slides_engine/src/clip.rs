//! Copying and pasting shapes, within a presentation and between
//! presentations.
//!
//! A copied shape is the XML it would be saved as (edits included), the
//! namespace declarations of its slide, and the relationships it refers
//! to (pictures, media, hyperlinks), with the bytes of referenced parts so
//! another presentation can take them. Pasting re-reads that XML with a
//! new shape id and new relationship ids in the target slide.
//!
//! Charts and SmartArt live in parts of their own (the chart and its
//! workbook, SmartArt's data, layout, style, colours and drawing): a copy
//! carries those parts, and pasting adds them under new names (as
//! PowerPoint does), so the copy and the original are edited apart.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context as _, bail};

use crate::edit::ShapeRef;
use crate::history::{Focus, Kind};
use crate::model::*;
use crate::presentation::Presentation;
use crate::shape::{ParseCtx, parse_shape, write_shape};
use crate::theme::ColorCtx;
use crate::xml::*;

/// Relationship kinds a copied shape may carry along.
const COPYABLE: [&str; 5] = ["image", "hyperlink", "media", "audio", "video"];

/// Relationship kinds whose parts are copied with the shape (new names).
const OWNED: [&str; 6] = [
    "chart",
    "diagramData",
    "diagramLayout",
    "diagramQuickStyle",
    "diagramColors",
    "diagramDrawing",
];

/// A part added by a paste (see the module docs).
#[derive(Clone, Debug)]
pub(crate) struct AddedPart {
    pub bytes: Arc<Vec<u8>>,
    /// Its content type (`None`: a relationships part).
    pub ct: Option<String>,
    /// The part a slide refers to (this one is saved while it's used).
    pub root: String,
}

/// A copied part, its content type and its own relationships (owned
/// parts copied along, external and other targets kept).
#[derive(Clone, Debug)]
struct PartTree {
    part: String,
    bytes: Arc<Vec<u8>>,
    ct: Option<String>,
    rels: Vec<(Rel, Option<PartTree>)>,
}

/// How far a pasted copy is moved when it would land on its original.
pub const PASTE_OFFSET: Emu = 228_600;

static NEXT_DECK: AtomicU64 = AtomicU64::new(1);

pub(crate) fn new_deck_id() -> u64 {
    NEXT_DECK.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Debug)]
enum Target {
    External(String),
    /// A part of the source presentation and its bytes.
    Part(String, Option<Arc<Vec<u8>>>),
    /// A chart's or SmartArt's part, copied under a new name.
    Tree(PartTree),
}

#[derive(Clone, Debug)]
struct ClipRel {
    id: String,
    ty: String,
    target: Target,
}

#[derive(Clone, Debug)]
struct ClipShape {
    xml: String,
    xfrm: Option<Xfrm>,
    rels: Vec<ClipRel>,
}

/// Copied shapes (see the module docs).
#[derive(Clone, Debug)]
pub struct ShapeClip {
    deck: u64,
    ns: Vec<(String, String)>,
    shapes: Vec<ClipShape>,
    /// The shapes' text, for other applications.
    pub text: String,
}

impl ShapeClip {
    pub fn len(&self) -> usize {
        self.shapes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }
}

/// Namespace declarations on the root start tag in `prefix`.
pub(crate) fn root_ns(prefix: &str) -> Vec<(String, String)> {
    let mut rest = prefix;
    while let Some(i) = rest.find('<') {
        let tail = &rest[i..];
        if tail.starts_with("<?") || tail.starts_with("<!") {
            rest = &tail[2..];
            continue;
        }
        let Some(end) = tag_end(tail) else { break };
        return split_attrs(tag_attrs(&tail[..end]))
            .into_iter()
            .filter(|(k, _)| k.starts_with("xmlns"))
            .collect();
    }
    Vec::new()
}

/// `xml` wrapped in an element declaring `ns`, and the wrapper's length.
pub(crate) fn wrap(ns: &[(String, String)], xml: &str) -> (String, usize) {
    let mut open = String::from("<w");
    write_attrs(&mut open, ns);
    open.push('>');
    let n = open.len();
    (format!("{open}{xml}</w>"), n)
}

impl Presentation {
    /// Copy shapes of slide `slide` (in z-order). Tables, charts, SmartArt
    /// and other objects Kantor keeps but doesn't model can't be copied yet.
    pub fn copy_shapes(&self, slide: usize, shapes: &[usize]) -> anyhow::Result<ShapeClip> {
        let s = self.slides.get(slide).context("no such slide")?;
        let ns = root_ns(&s.src.prefix);
        let mut out = Vec::new();
        let mut text = Vec::new();
        let mut idx = shapes.to_vec();
        idx.sort_unstable();
        idx.dedup();
        for i in idx {
            let shape = s.shapes.get(i).context("no such shape")?;
            if let ShapeKind::Other { what, .. } = &shape.kind
                && !shape.is_group()
                && shape.table().is_none()
                && !shape.is_chart()
                && !shape.is_smartart()
            {
                bail!("Copying a {what} isn't supported yet");
            }
            let mut copy = shape.clone();
            // A placeholder that inherits its position takes it along.
            copy.xfrm = self.effective_xfrm(slide, shape);
            let mut xml = String::new();
            write_shape(&mut xml, &copy);
            let (wrapped, _) = wrap(&ns, &xml);
            let doc = roxmltree::Document::parse(&wrapped).context("copied shape XML")?;
            let mut rels: Vec<ClipRel> = Vec::new();
            let mut ids: Vec<String> = doc
                .descendants()
                .flat_map(|n| n.attributes())
                .filter(|a| a.namespace() == Some(NS_R))
                .map(|a| a.value().to_owned())
                .collect();
            // SmartArt's drawing is named in its data part (`dsp:dataModelExt`).
            if let Some(data) = shape_data_part(shape)
                && let Some(id) = self.part_bytes(&data).and_then(|b| drawing_rel_id(&b))
            {
                ids.push(id);
            }
            for id in ids {
                if rels.iter().any(|r| r.id == id) {
                    continue;
                }
                let Some(rel) = s.rels.iter().find(|r| r.id == id) else {
                    continue;
                };
                let owned = OWNED.contains(&rel.kind());
                if !COPYABLE.contains(&rel.kind()) && !owned {
                    bail!("Copying a shape with a {} isn't supported yet", rel.kind());
                }
                let target = match rel.part(&s.part) {
                    None => Target::External(rel.target.clone()),
                    Some(p) if owned => {
                        let mut seen = Vec::new();
                        Target::Tree(self.part_tree(&p, &mut seen)?)
                    }
                    Some(p) => {
                        let bytes = self.part_bytes(&p);
                        Target::Part(p, bytes)
                    }
                };
                rels.push(ClipRel {
                    id: rel.id.clone(),
                    ty: rel.ty.clone(),
                    target,
                });
            }
            if let Some(t) = &shape.text {
                text.push(t.plain_text());
            }
            out.push(ClipShape {
                xml,
                xfrm: copy.xfrm,
                rels,
            });
        }
        if out.is_empty() {
            bail!("Nothing to copy");
        }
        Ok(ShapeClip {
            deck: self.deck,
            ns,
            shapes: out,
            text: text.join("\n"),
        })
    }

    /// Paste copied shapes on top of slide `slide`; returns their indices.
    /// A copy that would land exactly on a shape of the slide is moved
    /// down and right (so Duplicate and pasting on the same slide show it).
    pub fn paste_shapes(&mut self, slide: usize, clip: &ShapeClip) -> anyhow::Result<Vec<usize>> {
        let s = self.slides.get(slide).context("no such slide")?.clone();
        if let Some(why) = &s.locked {
            bail!("This slide can't be edited: {why}");
        }
        // Namespaces the copies use must be declared on the target slide.
        let have = root_ns(&s.src.prefix);
        let mut missing = Vec::new();
        for (k, v) in &clip.ns {
            match have.iter().find(|(hk, _)| hk == k) {
                Some((_, hv)) if hv != v => {
                    bail!(
                        "These shapes can't be pasted here: their XML prefixes clash with this slide's"
                    )
                }
                Some(_) => {}
                None => missing.push((k.clone(), v.clone())),
            }
        }
        let mut rels = (*s.rels).clone();
        let mut media: Vec<(String, Arc<Vec<u8>>)> = Vec::new();
        // Parts pasted with charts and SmartArt (added to the presentation
        // before the shapes are read, so they can be).
        let mut trees: Vec<(String, AddedPart)> = Vec::new();
        let mut taken: Vec<Xfrm> = s
            .shapes
            .iter()
            .filter_map(|x| self.effective_xfrm(slide, x))
            .collect();
        let mut next_id = s.next_shape_id();
        let mut shapes = Vec::new();
        for c in &clip.shapes {
            // New relationship ids (and media parts from another deck).
            let mut map: Vec<(String, String)> = Vec::new();
            for r in &c.rels {
                let id = next_rel_id(&rels);
                let (target, external) = match &r.target {
                    Target::External(t) => (t.clone(), true),
                    Target::Tree(tree) => {
                        let part = self.add_tree(tree, None, &mut trees);
                        (relative(&s.part, &part), false)
                    }
                    Target::Part(p, bytes) => {
                        let same = clip.deck == self.deck && self.part_bytes(p).is_some();
                        let part = if same {
                            p.clone()
                        } else {
                            let bytes = bytes
                                .clone()
                                .context("a copied picture's data is missing")?;
                            let ext = p.rsplit('.').next().unwrap_or("bin").to_ascii_lowercase();
                            let part = (1..)
                                .map(|n| format!("ppt/media/image{n}.{ext}"))
                                .find(|c| {
                                    !self.pkg.names.contains(c)
                                        && !self.media.contains_key(c)
                                        && !self.parts.contains_key(c)
                                        && !trees.iter().any(|(t, _)| t == c)
                                        && !media.iter().any(|(m, _)| m == c)
                                })
                                .expect("a free media name");
                            media.push((part.clone(), bytes));
                            part
                        };
                        (relative(&s.part, &part), false)
                    }
                };
                rels.push(Rel {
                    id: id.clone(),
                    ty: r.ty.clone(),
                    target,
                    external,
                });
                map.push((r.id.clone(), id));
            }
            // The new SmartArt data part names the new drawing relationship.
            for r in &c.rels {
                if r.ty.ends_with("/diagramData")
                    && let Some((_, new_id)) = map.iter().find(|(old, _)| *old == r.id)
                    && let Some(rel) = rels.iter().find(|x| x.id == *new_id)
                    && let Some(data) = rel.part(&s.part)
                {
                    let drawing_old = c.rels.iter().find(|x| x.ty.ends_with("/diagramDrawing"));
                    if let Some(d) = drawing_old
                        && let Some((_, d_new)) = map.iter().find(|(o, _)| *o == d.id)
                        && let Some((_, added)) = trees.iter_mut().find(|(p, _)| *p == data)
                        && let Ok(xml) = std::str::from_utf8(&added.bytes)
                    {
                        let fixed = xml.replace(
                            &format!("relId=\"{}\"", d.id),
                            &format!("relId=\"{d_new}\""),
                        );
                        added.bytes = Arc::new(fixed.into_bytes());
                    }
                }
            }
            for (p, a) in &trees {
                self.parts.insert(p.clone(), a.clone());
            }
            // Rewrite the shape's id and its relationship references.
            let (wrapped, start) = wrap(&clip.ns, &c.xml);
            let doc = roxmltree::Document::parse(&wrapped).context("copied shape XML")?;
            let el = doc
                .root_element()
                .first_element_child()
                .context("copied shape")?;
            let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
            // New ids for the shape and, in a group, every shape in it.
            let mut id = next_id;
            for cnv in el.descendants().filter(|n| is(*n, NS_P, "cNvPr")) {
                if let Some(a) = cnv.attributes().find(|a| a.name() == "id") {
                    edits.push((a.range_value(), id.to_string()));
                    id += 1;
                }
            }
            for a in el.descendants().flat_map(|n| n.attributes()) {
                if a.namespace() == Some(NS_R)
                    && let Some((_, new)) = map.iter().find(|(old, _)| old == a.value())
                {
                    edits.push((a.range_value(), new.clone()));
                }
            }
            edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
            let mut xml = wrapped.clone();
            for (r, t) in edits {
                xml.replace_range(r, &t);
            }
            let rewritten = xml;
            let doc = roxmltree::Document::parse(&rewritten).context("pasted shape XML")?;
            let el = doc
                .root_element()
                .first_element_child()
                .context("pasted shape")?;
            let master = self.master_of(slide);
            let colors = ColorCtx {
                theme: &master.theme,
                map: &master.clr_map,
                ph: None,
            };
            let cx = ParseCtx {
                src: &rewritten,
                part: &s.part,
                rels: &rels,
                colors,
            };
            let mut shape = parse_shape(el, &cx);
            debug_assert!(el.range().start >= start);
            // A pasted SmartArt reads its (copied) drawing.
            if shape.is_smartart() || shape.is_group() {
                let mut tmp = (*s).clone();
                tmp.rels = Arc::new(rels.clone());
                self.attach_nested(&tmp, &mut shape, slide);
            }
            next_id = shape.max_id() + 1;
            if let Some(mut x) = c.xfrm {
                while taken.iter().any(|t| t.x == x.x && t.y == x.y) {
                    x.x += PASTE_OFFSET;
                    x.y += PASTE_OFFSET;
                }
                taken.push(x);
                shape.xfrm = Some(x);
            }
            shapes.push(shape);
        }
        for (part, bytes) in media {
            self.media.insert(part, bytes);
        }
        self.record(Kind::Other, None, Focus::slide(slide), false, |p| {
            let s = p.slide_mut(slide)?;
            if !missing.is_empty() {
                s.declare_ns(&missing);
            }
            s.rels = Arc::new(rels);
            let first = s.shapes.len();
            s.shapes.extend(shapes);
            let added: Vec<usize> = (first..s.shapes.len()).collect();
            let focus = match added.as_slice() {
                [one] => Focus::shape(slide, *one),
                _ => Focus::slide(slide),
            };
            Ok((added, focus))
        })
    }

    /// Part `part` with the parts it owns (charts' workbooks and styles,
    /// SmartArt drawings' pictures), for copying.
    fn part_tree(&self, part: &str, seen: &mut Vec<String>) -> anyhow::Result<PartTree> {
        if seen.len() > 64 || seen.iter().any(|p| p == part) {
            bail!("This object's parts refer to each other in a loop; it can't be copied");
        }
        seen.push(part.to_owned());
        let bytes = self
            .part_bytes(part)
            .with_context(|| format!("{part} is missing"))?;
        let rels_bytes = self.part_bytes(&rels_name(part));
        let mut rels = Vec::new();
        if let Some(rb) = rels_bytes {
            for r in parse_rels(&String::from_utf8_lossy(&rb)) {
                let tree = match r.part(part) {
                    Some(t) => Some(self.part_tree(&t, seen)?),
                    None => None,
                };
                rels.push((r, tree));
            }
        }
        Ok(PartTree {
            part: part.to_owned(),
            bytes,
            ct: self.content_type(part),
            rels,
        })
    }

    /// The content type of a part (as an override or by its extension).
    pub(crate) fn content_type(&self, part: &str) -> Option<String> {
        if let Some(a) = self.parts.get(part) {
            return a.ct.clone();
        }
        let ct = ContentTypes::parse(&self.pkg.ct_raw);
        if let Some((_, t)) = ct
            .overrides
            .iter()
            .find(|(p, _)| p.eq_ignore_ascii_case(part))
        {
            return Some(t.clone());
        }
        let ext = part.rsplit('.').next()?.to_ascii_lowercase();
        ct.defaults
            .iter()
            .find(|(e, _)| *e == ext)
            .map(|(_, t)| t.clone())
    }

    /// A free part name like `orig` (`ppt/charts/chart7.xml`).
    fn free_part(&self, orig: &str, taken: &[(String, AddedPart)]) -> String {
        let (dir, file) = orig.rsplit_once('/').unwrap_or(("", orig));
        let (stem, ext) = file.rsplit_once('.').unwrap_or((file, ""));
        let stem = stem.trim_end_matches(|c: char| c.is_ascii_digit());
        let dot = if ext.is_empty() { "" } else { "." };
        let slash = if dir.is_empty() { "" } else { "/" };
        (1..)
            .map(|n| format!("{dir}{slash}{stem}{n}{dot}{ext}"))
            .find(|c| {
                !self.pkg.names.contains(c)
                    && !self.media.contains_key(c)
                    && !self.parts.contains_key(c)
                    && !taken.iter().any(|(t, _)| t == c)
            })
            .expect("a free part name")
    }

    /// Add a copied part tree under new names; returns the new root name.
    fn add_tree(
        &self,
        tree: &PartTree,
        root: Option<&str>,
        out: &mut Vec<(String, AddedPart)>,
    ) -> String {
        let part = self.free_part(&tree.part, out);
        let root = root.map_or_else(|| part.clone(), str::to_owned);
        // Reserve the name before the children take theirs.
        out.push((
            part.clone(),
            AddedPart {
                bytes: tree.bytes.clone(),
                ct: tree.ct.clone(),
                root: root.clone(),
            },
        ));
        if !tree.rels.is_empty() {
            let rels: Vec<Rel> = tree
                .rels
                .iter()
                .map(|(r, sub)| match sub {
                    Some(t) => {
                        let child = self.add_tree(t, Some(&root), out);
                        Rel {
                            target: relative(&part, &child),
                            ..r.clone()
                        }
                    }
                    None => r.clone(),
                })
                .collect();
            out.push((
                rels_name(&part),
                AddedPart {
                    bytes: Arc::new(write_rels(&rels).into_bytes()),
                    ct: None,
                    root,
                },
            ));
        }
        part
    }

    /// Ctrl+D: copy a shape onto its own slide, offset.
    pub fn duplicate_shape(&mut self, r: impl Into<ShapeRef>) -> anyhow::Result<usize> {
        let r: ShapeRef = r.into();
        let clip = self.copy_shapes(r.0, &[r.1])?;
        let added = self.paste_shapes(r.0, &clip)?;
        added.first().copied().context("nothing pasted")
    }
}

/// The SmartArt data part a frame shows.
fn shape_data_part(shape: &Shape) -> Option<String> {
    match &shape.kind {
        ShapeKind::Other { frame, .. } => match frame.get()? {
            crate::frame::FrameContent::Diagram(d) => d.data.clone(),
            _ => None,
        },
        _ => None,
    }
}

/// The slide relationship a SmartArt data part names for its drawing.
fn drawing_rel_id(data: &[u8]) -> Option<String> {
    let xml = std::str::from_utf8(data).ok()?;
    let doc = roxmltree::Document::parse(xml).ok()?;
    doc.descendants()
        .find(|n| is(*n, crate::frame::NS_DSP, "dataModelExt"))
        .and_then(|n| n.attribute("relId"))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_namespaces() {
        let ns = root_ns(
            "<?xml version=\"1.0\"?>\n<p:sld xmlns:a=\"A\" xmlns:p=\"P\" xmlns:a14=\"X\"><p:cSld>",
        );
        assert_eq!(ns.len(), 3);
        assert_eq!(ns[2], ("xmlns:a14".into(), "X".into()));
    }
}
