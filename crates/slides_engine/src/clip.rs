//! Copying and pasting shapes, within a presentation and between
//! presentations.
//!
//! A copied shape is the XML it would be saved as (edits included), the
//! namespace declarations of its slide, and the relationships it refers
//! to (pictures, media, hyperlinks), with the bytes of referenced parts so
//! another presentation can take them. Pasting re-reads that XML with a
//! new shape id and new relationship ids in the target slide.

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
fn root_ns(prefix: &str) -> Vec<(String, String)> {
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
fn wrap(ns: &[(String, String)], xml: &str) -> (String, usize) {
    let mut open = String::from("<w");
    write_attrs(&mut open, ns);
    open.push('>');
    let n = open.len();
    (format!("{open}{xml}</w>"), n)
}

impl Presentation {
    /// Copy shapes of slide `slide` (in z-order). Groups, tables, charts
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
            if let ShapeKind::Other { what, .. } = &shape.kind {
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
            for a in doc.descendants().flat_map(|n| n.attributes()) {
                if a.namespace() != Some(NS_R) || rels.iter().any(|r| r.id == a.value()) {
                    continue;
                }
                let Some(rel) = s.rels.iter().find(|r| r.id == a.value()) else {
                    continue;
                };
                if !COPYABLE.contains(&rel.kind()) {
                    bail!("Copying a shape with a {} isn't supported yet", rel.kind());
                }
                let target = match rel.part(&s.part) {
                    None => Target::External(rel.target.clone()),
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
        let master = self.master_of(slide);
        let colors = ColorCtx {
            theme: &master.theme,
            map: &master.clr_map,
            ph: None,
        };
        let mut rels = (*s.rels).clone();
        let mut media: Vec<(String, Arc<Vec<u8>>)> = Vec::new();
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
            // Rewrite the shape's id and its relationship references.
            let (wrapped, start) = wrap(&clip.ns, &c.xml);
            let doc = roxmltree::Document::parse(&wrapped).context("copied shape XML")?;
            let el = doc
                .root_element()
                .first_element_child()
                .context("copied shape")?;
            let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
            let id = next_id;
            if let Some(cnv) = el
                .children()
                .find(|n| n.is_element() && n.tag_name().name().starts_with("nv"))
                .and_then(|nv| child(nv, NS_P, "cNvPr"))
                && let Some(a) = cnv.attributes().find(|a| a.name() == "id")
            {
                edits.push((a.range_value(), id.to_string()));
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
            let cx = ParseCtx {
                src: &rewritten,
                part: &s.part,
                rels: &rels,
                colors,
            };
            let mut shape = parse_shape(el, &cx);
            debug_assert!(el.range().start >= start);
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

    /// Ctrl+D: copy a shape onto its own slide, offset.
    pub fn duplicate_shape(&mut self, r: ShapeRef) -> anyhow::Result<usize> {
        let clip = self.copy_shapes(r.0, &[r.1])?;
        let added = self.paste_shapes(r.0, &clip)?;
        added.first().copied().context("nothing pasted")
    }
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
