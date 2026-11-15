//! Groups, as in PowerPoint: shapes inside a group (and the shapes of a
//! SmartArt drawing) are addressed by a path ([`Inner`]) under a slide
//! shape ([`ShapeRef`]), so moving, text and formatting edits reach them;
//! Group, Ungroup and Regroup.
//!
//! A group's children are stored in its child space (`a:chOff`/`a:chExt`)
//! and drawn through [`compose`]: moving, resizing or turning the group
//! changes only its own box, so the children follow. Moving a child keeps
//! the others where they are and fits the group's box (and child space)
//! to the children again, as PowerPoint does.

use std::sync::Arc;

use anyhow::{Context as _, bail};

use crate::clip::{root_ns, wrap};
use crate::frame::{FrameContent, Group, compose, uncompose};
use crate::history::{Focus, Kind};
use crate::model::*;
use crate::presentation::{Presentation, Slide};
use crate::shape::{ParseCtx, parse_shape};
use crate::theme::ColorCtx;

/// A path of child indices from the outermost group down; empty = the
/// slide shape itself. Groups nest at most [`Inner::MAX`] deep here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Inner {
    len: u8,
    at: [u16; Inner::MAX],
    /// A table frame's cell (row, column), for typing into it.
    cell: Option<(u16, u16)>,
}

impl Inner {
    pub const MAX: usize = 8;
    pub const TOP: Self = Self {
        len: 0,
        at: [0; Self::MAX],
        cell: None,
    };

    /// The slide shape itself (a table cell of it counts as the shape).
    pub fn is_top(&self) -> bool {
        self.len == 0
    }

    /// Cell `(row, col)` of a table frame (on the slide).
    pub fn cell(row: usize, col: usize) -> Self {
        Self {
            cell: Some((
                row.min(u16::MAX as usize) as u16,
                col.min(u16::MAX as usize) as u16,
            )),
            ..Self::TOP
        }
    }

    /// The table cell this addresses, if any.
    pub fn cell_at(&self) -> Option<(usize, usize)> {
        self.cell.map(|(r, c)| (r as usize, c as usize))
    }

    pub fn depth(&self) -> usize {
        self.len as usize
    }

    pub fn path(&self) -> impl Iterator<Item = usize> + '_ {
        self.at[..self.len as usize].iter().map(|&i| i as usize)
    }

    /// One level deeper, at child `i` (`None` when too deep).
    pub fn child(self, i: usize) -> Option<Self> {
        if self.len as usize == Self::MAX || i > u16::MAX as usize {
            return None;
        }
        let mut out = self;
        out.cell = None;
        out.at[out.len as usize] = i as u16;
        out.len += 1;
        Some(out)
    }

    /// One level up (the top stays the top).
    pub fn parent(self) -> Self {
        self.prefix(self.depth().saturating_sub(1))
    }

    /// The last step (the index in the parent group).
    pub fn last(&self) -> Option<usize> {
        self.len
            .checked_sub(1)
            .map(|i| self.at[i as usize] as usize)
    }

    /// The first `depth` steps.
    pub fn prefix(self, depth: usize) -> Self {
        // Unused steps stay zero, so equal paths compare equal.
        let mut out = Self::TOP;
        out.len = (depth as u8).min(self.len);
        out.at[..out.len as usize].copy_from_slice(&self.at[..out.len as usize]);
        out
    }

    /// A number that sorts paths in reading order (the first four steps).
    pub fn order(&self) -> u64 {
        self.path()
            .take(4)
            .enumerate()
            .map(|(k, i)| ((i as u64 + 1).min(0xFFFF)) << (48 - 16 * k))
            .sum()
    }

    pub fn of(path: &[usize]) -> Option<Self> {
        path.iter().try_fold(Self::TOP, |p, &i| p.child(i))
    }
}

/// A shape on a slide: `(slide index, index in the slide's shapes, path
/// inside that shape)`. The path is empty except for shapes inside groups
/// and SmartArt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShapeRef(pub usize, pub usize, pub Inner);

impl From<(usize, usize)> for ShapeRef {
    fn from((slide, shape): (usize, usize)) -> Self {
        Self(slide, shape, Inner::TOP)
    }
}

impl ShapeRef {
    pub fn is_top(&self) -> bool {
        self.2.is_top()
    }

    /// The slide shape this one is in (itself at the top).
    pub fn top(self) -> Self {
        Self(self.0, self.1, Inner::TOP)
    }

    pub fn parent(self) -> Self {
        Self(self.0, self.1, self.2.parent())
    }

    pub fn child(self, i: usize) -> Option<Self> {
        Some(Self(self.0, self.1, self.2.child(i)?))
    }

    /// Cell `(row, col)` of table frame `self` (a slide shape).
    pub fn cell(self, row: usize, col: usize) -> Self {
        Self(self.0, self.1, Inner::cell(row, col))
    }

    /// The table cell this addresses, if any.
    pub fn cell_at(&self) -> Option<(usize, usize)> {
        self.2.cell_at()
    }
}

/// Where a shape is drawn on its slide.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    /// The shape's box on the slide.
    pub xfrm: Xfrm,
    /// SmartArt: the text's own box on the slide.
    pub text_box: Option<Xfrm>,
    /// The group it is in: the group's box on the slide and its child space.
    pub parent: Option<(Xfrm, Xfrm)>,
    /// Inside a SmartArt drawing.
    pub smartart: bool,
}

/// The child space of a group or SmartArt frame.
fn space_of(s: &Shape) -> Option<Xfrm> {
    s.inner_shapes().map(|g| g.child_box)
}

/// A group's or SmartArt drawing's shapes, for editing.
pub(crate) fn inner_shapes_mut(s: &mut Shape) -> Option<&mut Group> {
    let ShapeKind::Other { frame, .. } = &mut s.kind else {
        return None;
    };
    let arc = frame.0.as_mut()?;
    match Arc::make_mut(arc) {
        FrameContent::Group(g) => Some(g),
        FrameContent::Diagram(d) => d.drawing.as_mut().map(|dd| &mut Arc::make_mut(dd).group),
        _ => None,
    }
}

impl Group {
    /// Replace child `i` with `with` (the SmartArt extras follow).
    pub(crate) fn splice(&mut self, i: usize, with: Vec<Shape>) {
        let n = with.len();
        self.text_boxes.resize(self.children.len(), None);
        self.model_ids.resize(self.children.len(), None);
        self.children.splice(i..=i, with);
        self.text_boxes.splice(i..=i, std::iter::repeat_n(None, n));
        self.model_ids.splice(i..=i, std::iter::repeat_n(None, n));
    }

    /// The children's bounds in the child space.
    fn bounds(&self) -> Option<Xfrm> {
        let boxes: Vec<Xfrm> = self
            .children
            .iter()
            .filter_map(|c| c.xfrm.map(|x| x.bounding_box()))
            .collect();
        union(&boxes)
    }
}

/// The box around `boxes`.
pub(crate) fn union(boxes: &[Xfrm]) -> Option<Xfrm> {
    let x0 = boxes.iter().map(|b| b.x).min()?;
    let y0 = boxes.iter().map(|b| b.y).min()?;
    let x1 = boxes.iter().map(|b| b.x + b.cx).max()?;
    let y1 = boxes.iter().map(|b| b.y + b.cy).max()?;
    Some(Xfrm::new(x0, y0, x1 - x0, y1 - y0))
}

/// Fit a group (box `xfrm`, in its parent's space) to its children: the
/// child space becomes their bounds and the box follows, so nothing moves
/// on the slide (turned and flipped groups included).
pub(crate) fn refit(g: &mut Group, xfrm: &mut Xfrm) {
    let Some(u) = g.bounds() else { return };
    let c = g.child_box;
    if u == c {
        return;
    }
    let k = |a: Emu, b: Emu| if b == 0 { 1.0 } else { a as f64 / b as f64 };
    let (sx, sy) = (k(xfrm.cx, c.cx), k(xfrm.cy, c.cy));
    let mut l = (
        xfrm.x as f64 + (u.x - c.x) as f64 * sx,
        xfrm.y as f64 + (u.y - c.y) as f64 * sy,
        u.cx as f64 * sx,
        u.cy as f64 * sy,
    );
    // The box turns (and flips) about its centre: keep the children where
    // they are when that centre moves.
    let (ox, oy) = xfrm.center();
    let (nx, ny) = (l.0 + l.2 / 2.0, l.1 + l.3 / 2.0);
    let (mut vx, mut vy) = (nx - ox, ny - oy);
    if xfrm.flip_h {
        vx = -vx;
    }
    if xfrm.flip_v {
        vy = -vy;
    }
    let (s, co) = xfrm.angle().sin_cos();
    let (rx, ry) = (vx * co - vy * s, vx * s + vy * co);
    l.0 += (ox - nx) + rx;
    l.1 += (oy - ny) + ry;
    *xfrm = Xfrm {
        x: l.0.round() as Emu,
        y: l.1.round() as Emu,
        cx: (l.2.round() as Emu).max(1),
        cy: (l.3.round() as Emu).max(1),
        ..*xfrm
    };
    g.child_box = u;
}

/// Box `b` (inside or beside box `from`) where it goes when `from` moves
/// and resizes to `to`: a SmartArt shape's text box.
pub(crate) fn follow(from: &Xfrm, to: &Xfrm, b: &Xfrm) -> Xfrm {
    let k = |a: Emu, b: Emu| if b == 0 { 1.0 } else { a as f64 / b as f64 };
    let (sx, sy) = (k(to.cx, from.cx), k(to.cy, from.cy));
    Xfrm {
        x: to.x + ((b.x - from.x) as f64 * sx).round() as Emu,
        y: to.y + ((b.y - from.y) as f64 * sy).round() as Emu,
        cx: ((b.cx as f64 * sx).round() as Emu).max(1),
        cy: ((b.cy as f64 * sy).round() as Emu).max(1),
        ..*b
    }
}

/// Fit the group `s` (if it is one) to its children.
fn refit_shape(s: &mut Shape) {
    if !s.is_group() {
        return;
    }
    let Some(mut x) = s.xfrm else { return };
    if let Some(g) = inner_shapes_mut(s) {
        refit(g, &mut x);
    }
    s.xfrm = Some(x);
}

/// Fit every group on the way to `r`, innermost first.
fn refit_ancestors(s: &mut Slide, r: ShapeRef) {
    for depth in (0..r.2.depth()).rev() {
        if let Some(g) = s.shape_at_mut(r.1, r.2.prefix(depth)) {
            refit_shape(g);
        }
    }
}

impl Slide {
    /// The shape `r` points at on this slide (`r.0` is ignored).
    pub fn shape_at(&self, top: usize, inner: Inner) -> Option<&Shape> {
        let mut s = self.shapes.get(top)?;
        for i in inner.path() {
            s = s.inner_shapes()?.children.get(i)?;
        }
        Some(s)
    }

    pub(crate) fn shape_at_mut(&mut self, top: usize, inner: Inner) -> Option<&mut Shape> {
        let mut s = self.shapes.get_mut(top)?;
        for i in inner.path() {
            s = inner_shapes_mut(s)?.children.get_mut(i)?;
        }
        Some(s)
    }
}

/// What the regroup command remembers of the last ungrouped group.
#[derive(Clone, Debug)]
pub(crate) struct Regroup {
    pub slide: String,
    pub group: Shape,
    pub ids: Vec<u32>,
}

impl Presentation {
    /// The shape `r` points at.
    pub fn shape(&self, r: impl Into<ShapeRef>) -> Option<&Shape> {
        let r = r.into();
        self.slides.get(r.0)?.shape_at(r.1, r.2)
    }

    /// Where shape `r` is drawn on its slide.
    pub fn placed(&self, r: impl Into<ShapeRef>) -> Option<Placed> {
        let r = r.into();
        let slide = self.slides.get(r.0)?;
        let mut s = slide.shapes.get(r.1)?;
        let mut out = Placed {
            xfrm: self.effective_xfrm(r.0, s)?,
            text_box: None,
            parent: None,
            smartart: false,
        };
        for i in r.2.path() {
            let space = space_of(s)?;
            out.smartart |= s.smartart().is_some();
            let g = s.inner_shapes()?;
            let child = g.children.get(i)?;
            let at = compose(&out.xfrm, &space, &child.xfrm?);
            out.text_box = g
                .text_boxes
                .get(i)
                .copied()
                .flatten()
                .map(|t| compose(&out.xfrm, &space, &t));
            out.parent = Some((out.xfrm, space));
            out.xfrm = at;
            s = child;
        }
        Some(out)
    }

    /// The topmost shape inside `r` (a group or SmartArt frame) under the
    /// slide point `(x, y)`.
    pub fn inner_at(&self, r: impl Into<ShapeRef>, x: Emu, y: Emu) -> Option<ShapeRef> {
        let r = r.into();
        let g = self.shape(r)?.inner_shapes()?;
        (0..g.children.len()).rev().find_map(|i| {
            let c = r.child(i)?;
            let shown = !matches!(&g.children[i].kind, ShapeKind::Other { frame, .. } if frame.get().is_none());
            (shown && self.placed(c)?.xfrm.contains(x, y)).then_some(c)
        })
    }

    /// Whether `r` is a shape inside a SmartArt drawing.
    pub fn in_smartart(&self, r: impl Into<ShapeRef>) -> bool {
        let r = r.into();
        !r.is_top() && self.placed(r).is_some_and(|p| p.smartart)
    }

    /// Move / resize a shape inside a group: `xfrm` is where it goes on
    /// the slide. The group fits itself to its children again.
    pub(crate) fn set_inner_xfrm(
        &mut self,
        r: ShapeRef,
        xfrm: Xfrm,
        nudge: bool,
    ) -> anyhow::Result<()> {
        let placed = self.placed(r).context("no such shape")?;
        let (gx, space) = placed.parent.context("no such shape")?;
        let xfrm = Xfrm {
            cx: xfrm.cx.max(1),
            cy: xfrm.cy.max(1),
            ..xfrm
        };
        let mut local = uncompose(&gx, &space, &xfrm);
        let kind = if nudge { Kind::Nudge } else { Kind::Other };
        let focus = Focus::at(r);
        if placed.smartart {
            // SmartArt shapes keep their turn; the frame doesn't refit (its
            // child space is the frame), and the text box follows the shape.
            let old = self
                .shape(r)
                .and_then(|s| s.xfrm)
                .context("no such shape")?;
            local.rot = old.rot;
            local.flip_h = old.flip_h;
            local.flip_v = old.flip_v;
            let i = r.2.last().context("no such shape")?;
            let parent = r.parent();
            return self.record(kind, Some(r), focus.clone(), false, |p| {
                let s = p.slide_mut(r.0)?;
                let g = s
                    .shape_at_mut(parent.1, parent.2)
                    .and_then(inner_shapes_mut)
                    .context("no such shape")?;
                let c = g.children.get_mut(i).context("no such shape")?;
                c.xfrm = Some(local);
                if let Some(Some(tb)) = g.text_boxes.get_mut(i) {
                    *tb = follow(&old, &local, tb);
                }
                Ok(((), focus))
            });
        }
        self.record(kind, Some(r), focus.clone(), false, |p| {
            let s = p.slide_mut(r.0)?;
            let shape = s.shape_at_mut(r.1, r.2).context("no such shape")?;
            shape.xfrm = Some(local);
            refit_ancestors(s, r);
            Ok(((), focus))
        })
    }

    /// Delete a shape inside a group. A group left with one shape is
    /// dissolved into it (as PowerPoint); an emptied group goes too.
    pub(crate) fn delete_inner(&mut self, r: ShapeRef) -> anyhow::Result<()> {
        if self.in_smartart(r) {
            bail!("SmartArt shapes can't be deleted yet (that needs PowerPoint's layouts)");
        }
        let focus = Focus::at(r);
        self.record(Kind::Other, None, focus, false, |p| {
            let s = p.slide_mut(r.0)?;
            let parent = r.parent();
            let i = r.2.last().context("no such shape")?;
            let g = s
                .shape_at_mut(parent.1, parent.2)
                .and_then(inner_shapes_mut)
                .context("no such shape")?;
            if i >= g.children.len() {
                bail!("no such shape");
            }
            g.splice(i, Vec::new());
            let left = g.children.len();
            if left >= 2 {
                refit_ancestors(s, r);
                return Ok(((), Focus::at(parent)));
            }
            // Replace the group with what's left of it.
            let gs = s
                .shape_at_mut(parent.1, parent.2)
                .context("no such shape")?;
            let gx = gs.xfrm;
            let g = gs.group().cloned().context("no such shape")?;
            let rest: Vec<Shape> = g
                .children
                .into_iter()
                .map(|mut c| {
                    if let (Some(gx), Some(cx)) = (gx, c.xfrm) {
                        c.xfrm = Some(compose(&gx, &g.child_box, &cx));
                    }
                    c
                })
                .collect();
            replace_at(s, parent, rest)?;
            refit_ancestors(s, parent);
            let focus = match (left, parent.is_top()) {
                (1, _) => Focus::at(parent),
                (_, true) => Focus::slide(r.0),
                _ => Focus::at(parent.parent()),
            };
            Ok(((), focus))
        })
    }

    /// Group shapes of slide `slide` (indices into its shapes, at least
    /// two). The group takes the place of the topmost of them; returns its
    /// index. Placeholders and tables can't be grouped (as in PowerPoint).
    pub fn group_shapes(&mut self, slide: usize, shapes: &[usize]) -> anyhow::Result<usize> {
        let mut idx = shapes.to_vec();
        idx.sort_unstable();
        idx.dedup();
        if idx.len() < 2 {
            bail!("Select two or more shapes to group them");
        }
        let s = self.slides.get(slide).context("no such slide")?;
        if let Some(why) = &s.locked {
            bail!("This slide can't be edited: {why}");
        }
        for &i in &idx {
            let sh = s.shapes.get(i).context("no such shape")?;
            if sh.ph.is_some() {
                bail!("Placeholders can't be grouped");
            }
            if sh.table().is_some() {
                bail!("Tables can't be grouped");
            }
            if sh.is_graphic_frame() && !sh.is_chart() && !sh.is_smartart() {
                bail!("Embedded objects can't be grouped");
            }
            if sh.xfrm.is_none() {
                bail!("This shape can't be grouped");
            }
        }
        let id = s.next_shape_id();
        let xml = format!(
            "<p:grpSp><p:nvGrpSpPr><p:cNvPr id=\"{id}\" name=\"Group {}\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>\
             <p:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/><a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"0\" cy=\"0\"/></a:xfrm></p:grpSpPr></p:grpSp>",
            id - 1
        );
        let template = self.parse_new(slide, &xml)?;
        let at = *idx.last().expect("two shapes") + 1 - idx.len();
        self.record(Kind::Other, None, Focus::slide(slide), false, |p| {
            let s = p.slide_mut(slide)?;
            let mut kids = Vec::new();
            for &i in idx.iter().rev() {
                kids.push(s.shapes.remove(i));
            }
            kids.reverse();
            let group = make_group(template, kids);
            s.shapes.insert(at, group);
            Ok((at, Focus::shape(slide, at)))
        })
    }

    /// Ungroup group `r` (on the slide or inside another group): its
    /// shapes take its place, where they were drawn. Returns them. A
    /// SmartArt frame is converted to shapes instead (as PowerPoint).
    pub fn ungroup(&mut self, r: impl Into<ShapeRef>) -> anyhow::Result<Vec<ShapeRef>> {
        let r = r.into();
        let shape = self.shape(r).context("no such shape")?;
        if shape.smartart().is_some() && r.is_top() {
            return self.convert_to_shapes(r).map(|i| vec![(r.0, i).into()]);
        }
        if self.in_smartart(r) {
            bail!("Shapes inside SmartArt can't be ungrouped");
        }
        let g = shape
            .group()
            .cloned()
            .context("Select a group to ungroup it")?;
        let gx = shape.xfrm.context("This group has no position")?;
        let memo = Regroup {
            slide: self.slides[r.0].part.clone(),
            group: shape.clone(),
            ids: g.children.iter().map(|c| c.id).collect(),
        };
        let n = g.children.len();
        let kids: Vec<Shape> = g
            .children
            .into_iter()
            .map(|mut c| {
                if let Some(cx) = c.xfrm {
                    c.xfrm = Some(compose(&gx, &g.child_box, &cx));
                }
                c
            })
            .collect();
        let out = self.record(Kind::Other, None, Focus::at(r), false, |p| {
            let s = p.slide_mut(r.0)?;
            replace_at(s, r, kids)?;
            if !r.is_top() {
                refit_ancestors(s, r);
            }
            let refs: Vec<ShapeRef> = (0..n)
                .filter_map(|k| match r.2.last() {
                    None => Some(ShapeRef(r.0, r.1 + k, Inner::TOP)),
                    Some(i) => r.parent().child(i + k),
                })
                .collect();
            let focus = refs.first().copied().map_or(Focus::slide(r.0), Focus::at);
            Ok((refs, focus))
        })?;
        self.regroup = Some(memo);
        Ok(out)
    }

    /// Whether Regroup can put the last ungrouped group back on `slide`.
    pub fn can_regroup(&self, slide: usize) -> bool {
        self.regroup_indices(slide).is_some()
    }

    fn regroup_indices(&self, slide: usize) -> Option<Vec<usize>> {
        let m = self.regroup.as_ref()?;
        let s = self.slides.get(slide)?;
        if s.part != m.slide {
            return None;
        }
        let idx: Option<Vec<usize>> = m
            .ids
            .iter()
            .map(|id| s.shapes.iter().position(|x| x.id == *id))
            .collect();
        idx.filter(|v| v.len() >= 2)
    }

    /// Put the last ungrouped group back together (same name, id and
    /// group formatting) from the shapes it had; returns its index.
    pub fn regroup(&mut self, slide: usize) -> anyhow::Result<usize> {
        let Some(mut idx) = self.regroup_indices(slide) else {
            bail!("There's no group to regroup on this slide");
        };
        let template = self.regroup.as_ref().expect("regroup").group.clone();
        idx.sort_unstable();
        let at = *idx.last().expect("two shapes") + 1 - idx.len();
        let r = self.record(Kind::Other, None, Focus::slide(slide), false, |p| {
            let s = p.slide_mut(slide)?;
            let mut kids = Vec::new();
            for &i in idx.iter().rev() {
                kids.push(s.shapes.remove(i));
            }
            kids.reverse();
            s.shapes.insert(at, make_group(template, kids));
            Ok((at, Focus::shape(slide, at)))
        })?;
        self.regroup = None;
        Ok(r)
    }

    /// Read a new shape from `xml` (in the slide's namespaces).
    pub(crate) fn parse_new(&self, slide: usize, xml: &str) -> anyhow::Result<Shape> {
        let s = self.slides.get(slide).context("no such slide")?;
        self.parse_new_with(slide, xml, &s.rels)
    }

    /// [`Self::parse_new`] with the slide's relationships as `rels`.
    pub(crate) fn parse_new_with(
        &self,
        slide: usize,
        xml: &str,
        rels: &[crate::xml::Rel],
    ) -> anyhow::Result<Shape> {
        let s = self.slides.get(slide).context("no such slide")?;
        let ns = root_ns(&s.src.prefix);
        let (wrapped, _) = wrap(&ns, xml);
        let doc = roxmltree::Document::parse(&wrapped).context("new shape XML")?;
        let el = doc
            .root_element()
            .first_element_child()
            .context("new shape")?;
        let master = self.master_of(slide);
        let cx = ParseCtx {
            src: &wrapped,
            part: &s.part,
            rels,
            colors: ColorCtx {
                theme: &master.theme,
                map: &master.clr_map,
                ph: None,
            },
        };
        Ok(parse_shape(el, &cx))
    }
}

/// A group read from `template` (its own XML is kept) holding `kids`, which
/// are in slide space: the child space is their bounds, as is the box.
fn make_group(mut template: Shape, kids: Vec<Shape>) -> Shape {
    let n = kids.len();
    let mut g = Group {
        children: kids,
        text_boxes: vec![None; n],
        model_ids: vec![None; n],
        ..Group::default()
    };
    let b = g.bounds().unwrap_or_default();
    g.child_box = b;
    let what = match &template.kind {
        ShapeKind::Other { what, .. } => what.clone(),
        _ => "group".into(),
    };
    template.kind = ShapeKind::Other {
        what,
        frame: crate::frame::Frame::new(FrameContent::Group(g)),
    };
    template.xfrm = Some(Xfrm::new(b.x, b.y, b.cx.max(1), b.cy.max(1)));
    template
}

/// Put `with` where the shape `r` is (on the slide or in its group).
fn replace_at(s: &mut Slide, r: ShapeRef, with: Vec<Shape>) -> anyhow::Result<()> {
    match r.2.last() {
        None => {
            if r.1 >= s.shapes.len() {
                bail!("no such shape");
            }
            s.shapes.splice(r.1..=r.1, with);
        }
        Some(i) => {
            let p = r.parent();
            let g = s
                .shape_at_mut(p.1, p.2)
                .and_then(inner_shapes_mut)
                .context("no such shape")?;
            if i >= g.children.len() {
                bail!("no such shape");
            }
            g.splice(i, with);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Xfrm, b: Xfrm) {
        let d = |x: Emu, y: Emu| (x - y).abs() <= 2;
        assert!(
            d(a.x, b.x) && d(a.y, b.y) && d(a.cx, b.cx) && d(a.cy, b.cy) && a.rot == b.rot,
            "{a:?} != {b:?}"
        );
    }

    #[test]
    fn paths_nest_and_unwind() {
        let p = Inner::TOP.child(3).unwrap().child(1).unwrap();
        assert_eq!(p.path().collect::<Vec<_>>(), [3, 1]);
        assert_eq!(p.last(), Some(1));
        assert_eq!(p.parent().last(), Some(3));
        assert!(p.parent().parent().is_top());
        assert_eq!(p.prefix(1), Inner::of(&[3]).unwrap());
        assert!(Inner::of(&[0; 9]).is_none());
    }

    #[test]
    fn refit_keeps_children_in_place_in_turned_flipped_groups() {
        for (rot, fh) in [(0.0, false), (30.0, false), (90.0, true), (200.0, true)] {
            let mut gx = Xfrm::new(1000, 2000, 4000, 2000).with_degrees(rot);
            gx.flip_h = fh;
            let mut g = Group {
                child_box: Xfrm::new(0, 0, 200, 100),
                children: Vec::new(),
                ..Group::default()
            };
            let kids = [Xfrm::new(10, 10, 50, 20), Xfrm::new(150, 60, 80, 60)];
            for k in kids {
                g.children.push(Shape {
                    id: 2,
                    name: String::new(),
                    kind: ShapeKind::Shape {
                        geom: Geom::Rect,
                        text_box: false,
                        geometry: None,
                    },
                    xfrm: Some(k),
                    ph: None,
                    look: Look::default(),
                    text: None,
                    paint: PaintEdit::default(),
                    src: None,
                });
            }
            let before: Vec<Xfrm> = kids.iter().map(|k| compose(&gx, &g.child_box, k)).collect();
            let mut x = gx;
            refit(&mut g, &mut x);
            assert_eq!(g.child_box, Xfrm::new(10, 10, 220, 110));
            for (k, b) in kids.iter().zip(before) {
                close(compose(&x, &g.child_box, k), b);
            }
        }
    }

    #[test]
    fn uncompose_inverts_compose() {
        let space = Xfrm::new(-500, 300, 1000, 2000);
        for (rot, fh, fv) in [
            (0.0, false, false),
            (45.0, true, false),
            (120.0, false, true),
        ] {
            let mut gx = Xfrm::new(100, 200, 3000, 1500).with_degrees(rot);
            gx.flip_h = fh;
            gx.flip_v = fv;
            for child in [
                Xfrm::new(0, 500, 300, 200),
                Xfrm::new(-400, 300, 100, 900).with_degrees(100.0),
            ] {
                let placed = compose(&gx, &space, &child);
                let back = uncompose(&gx, &space, &placed);
                close(back, child);
                assert_eq!((back.flip_h, back.flip_v), (child.flip_h, child.flip_v));
            }
        }
    }
}
