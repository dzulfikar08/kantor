//! Kantor's own layout of the common SmartArt families, so a resized
//! SmartArt is laid out again (not scaled) and items can be added and
//! removed: Basic Block List (`layout/default`, PowerPoint's `snake`
//! algorithm), Vertical Bullet List (`layout/vList2`, `lin` with `tx`
//! children), Basic Process (`layout/process1`, `lin` with arrows) and
//! Basic Cycle (`layout/cycle2`, `cycle`); and the hierarchies
//! Organization Chart, Hierarchy, Horizontal Hierarchy and Labeled
//! Hierarchy (child module `tree`). The family comes from the data model's document item
//! (`dgm:prSet/@loTypeId`); other layouts keep PowerPoint's drawing and are
//! scaled.
//!
//! Everything here works on the parts' XML: the data model (items in
//! order, their sibling transitions and presentation items) and the
//! drawing (`dsp:sp` shapes, each naming the presentation item it draws in
//! `modelId`). Shapes are placed by what they show: an item's shape, its
//! sub-items' text, or the arrow after it. New shapes are copies of an
//! existing one (so they keep the style); new items get the data model
//! items PowerPoint writes for one (the item, its parent and sibling
//! transitions and the connection). PowerPoint makes the presentation
//! items of new items itself when it lays the diagram out on opening;
//! Kantor's new shapes name the data item directly.

use std::collections::HashMap;
use std::ops::Range;

use anyhow::{Context as _, bail};
use roxmltree::{Document, Node};

use crate::frame::{NS_DGM, NS_DSP};
use crate::model::{Emu, Xfrm};
use crate::xml::*;

/// A SmartArt layout family Kantor can lay out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// Basic Block List: boxes in rows, the last row centred.
    Blocks,
    /// Vertical Bullet List: a header bar per item, its sub-items in a
    /// text box under it.
    Bullets,
    /// Basic Process: boxes left to right, arrows between them.
    Process,
    /// Basic Cycle: circles round a circle, arrows between them.
    Cycle,
    /// Organization Chart: a tree top down, elbow connectors.
    OrgChart,
    /// Hierarchy: a tree top down.
    Hierarchy,
    /// Horizontal Hierarchy: a tree left to right.
    HorizontalHierarchy,
    /// Labeled Hierarchy: a tree top down over a band per level, the
    /// level's label at its left (the document's later top-level items).
    LabeledHierarchy,
}

impl Family {
    fn of(lo_type: &str) -> Option<Self> {
        let id = lo_type.rsplit('/').next()?;
        Some(match id {
            "default" => Family::Blocks,
            "vList2" => Family::Bullets,
            "process1" => Family::Process,
            "cycle2" => Family::Cycle,
            "orgChart1" => Family::OrgChart,
            "hierarchy1" => Family::Hierarchy,
            "hierarchy2" => Family::HorizontalHierarchy,
            "hierarchy6" => Family::LabeledHierarchy,
            _ => return None,
        })
    }

    fn arrows(self) -> bool {
        matches!(self, Family::Process | Family::Cycle)
    }

    /// A hierarchy (laid out by the `tree` module).
    pub fn is_tree(self) -> bool {
        matches!(
            self,
            Family::OrgChart
                | Family::Hierarchy
                | Family::HorizontalHierarchy
                | Family::LabeledHierarchy
        )
    }
}

/// Where a new item goes, from the selected one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddPlace {
    After,
    Before,
    Above,
    Below,
}

/// A top-level item of the data model.
#[derive(Clone, Debug)]
struct Item {
    id: String,
    /// Its sibling transition (the arrow after it).
    sib: Option<String>,
    /// Its sub-items, depth first.
    subs: Vec<String>,
}

/// What the layout needs from a data model.
struct View {
    family: Family,
    doc: String,
    items: Vec<Item>,
    /// Presentation item → (data item it shows, `presName`).
    pres: HashMap<String, (String, String)>,
}

/// Each item's children: `(srcOrd, child, its sibling transition)`.
type Kids<'a> = HashMap<&'a str, Vec<(i64, &'a str, Option<&'a str>)>>;

fn view(data: &str) -> anyhow::Result<View> {
    let doc = Document::parse(data).context("SmartArt data")?;
    let pts: Vec<Node> = doc.descendants().filter(|n| is(*n, NS_DGM, "pt")).collect();
    let root = pts
        .iter()
        .find(|p| p.attribute("type") == Some("doc"))
        .context("SmartArt data without a document item")?;
    let lo = child(*root, NS_DGM, "prSet")
        .and_then(|p| p.attribute("loTypeId"))
        .unwrap_or_default();
    let family = Family::of(lo).context(LAYOUT_REFUSAL)?;
    let mut kids: Kids = HashMap::new();
    for c in doc.descendants().filter(|n| is(*n, NS_DGM, "cxn")) {
        if c.attribute("type").is_some_and(|t| t != "parOf") {
            continue;
        }
        let (Some(src), Some(dst)) = (c.attribute("srcId"), c.attribute("destId")) else {
            continue;
        };
        kids.entry(src).or_default().push((
            attr_i64(c, "srcOrd").unwrap_or(0),
            dst,
            c.attribute("sibTransId"),
        ));
    }
    for v in kids.values_mut() {
        v.sort_by_key(|k| k.0);
    }
    let doc_id = root.attribute("modelId").unwrap_or_default().to_owned();
    fn subtree<'a>(kids: &Kids<'a>, id: &'a str, out: &mut Vec<String>) {
        for (_, k, _) in kids.get(id).into_iter().flatten() {
            if out.len() < 10_000 && !out.iter().any(|o| o == k) {
                out.push((*k).to_owned());
                subtree(kids, k, out);
            }
        }
    }
    let items = kids
        .get(doc_id.as_str())
        .into_iter()
        .flatten()
        .map(|(_, id, sib)| {
            let mut subs = Vec::new();
            subtree(&kids, id, &mut subs);
            Item {
                id: (*id).to_owned(),
                sib: sib.map(str::to_owned),
                subs,
            }
        })
        .collect();
    let pres = pts
        .iter()
        .filter(|p| p.attribute("type") == Some("pres"))
        .filter_map(|p| {
            let pr = child(*p, NS_DGM, "prSet")?;
            Some((
                p.attribute("modelId")?.to_owned(),
                (
                    pr.attribute("presAssocID")?.to_owned(),
                    pr.attribute("presName").unwrap_or_default().to_owned(),
                ),
            ))
        })
        .collect();
    Ok(View {
        family,
        doc: doc_id,
        items,
        pres,
    })
}

pub const LAYOUT_REFUSAL: &str = "Kantor can't lay out this SmartArt layout yet: items can only be added or removed in lists, processes, cycles and hierarchies";

/// The family of a SmartArt (drawing and data model XML), if Kantor lays
/// it out: a known layout whose items each have one shape (and, in a
/// bullet list, one sub-item box).
pub fn family(drawing: &str, data: &str) -> Option<Family> {
    if let Some(f) = tree::family(data) {
        return Some(f);
    }
    let v = view(data).ok()?;
    let (list, _) = shapes(drawing, &v).ok()?;
    for i in 0..v.items.len() {
        let count = |want: Role| list.iter().filter(|s| s.role == want).count();
        if count(Role::Node(i)) > 1 || count(Role::Sub(i)) > 1 || count(Role::Arrow(i)) > 1 {
            return None;
        }
        if v.family != Family::Bullets && count(Role::Sub(i)) > 0 {
            return None;
        }
    }
    Some(v.family)
}

/// What a drawing shape shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Node(usize),
    Sub(usize),
    Arrow(usize),
    Other,
}

impl View {
    fn role(&self, model_id: &str) -> Role {
        let (target, name) = match self.pres.get(model_id) {
            Some((a, n)) => (a.as_str(), n.as_str()),
            None => (model_id, ""),
        };
        for (i, it) in self.items.iter().enumerate() {
            if it.id == target {
                return if name.to_ascii_lowercase().contains("child") {
                    Role::Sub(i)
                } else {
                    Role::Node(i)
                };
            }
            if it.sib.as_deref() == Some(target) {
                return Role::Arrow(i);
            }
            if it.subs.iter().any(|s| s == target) {
                return Role::Sub(i);
            }
        }
        Role::Other
    }

    /// The top-level item a shape belongs to.
    fn item_of(&self, model_id: &str) -> Option<usize> {
        match self.role(model_id) {
            Role::Node(i) | Role::Sub(i) | Role::Arrow(i) => Some(i),
            Role::Other => None,
        }
    }
}

/// A drawing shape: where it is in the XML and what it shows.
struct DShape {
    range: Range<usize>,
    model: String,
    xfrm: Option<(Range<usize>, Xfrm)>,
    tx: Option<(Range<usize>, Xfrm)>,
    role: Role,
}

fn xfrm_of(n: Node) -> Option<Xfrm> {
    let off = child(n, NS_A, "off")?;
    let ext = child(n, NS_A, "ext")?;
    let mut x = Xfrm::new(
        attr_i64(off, "x")?,
        attr_i64(off, "y")?,
        attr_i64(ext, "cx")?,
        attr_i64(ext, "cy")?,
    );
    x.rot = attr_i64(n, "rot").unwrap_or(0) as i32;
    Some(x)
}

fn shapes(drawing: &str, v: &View) -> anyhow::Result<(Vec<DShape>, usize)> {
    let doc = Document::parse(drawing).context("SmartArt drawing")?;
    let tree = doc
        .root_element()
        .children()
        .find(|n| is(*n, NS_DSP, "spTree"))
        .context("SmartArt drawing without shapes")?;
    let end = tree.range().end - format!("</{}>", tag_name(start_tag(drawing, tree))).len();
    let out = tree
        .children()
        .filter(|n| is(*n, NS_DSP, "sp"))
        .map(|n| {
            let model = n.attribute("modelId").unwrap_or_default().to_owned();
            let xfrm = child(n, NS_DSP, "spPr")
                .and_then(|s| child(s, NS_A, "xfrm"))
                .and_then(|x| Some((x.range(), xfrm_of(x)?)));
            let tx = child(n, NS_DSP, "txXfrm").and_then(|x| Some((x.range(), xfrm_of(x)?)));
            DShape {
                range: n.range(),
                role: v.role(&model),
                model,
                xfrm,
                tx,
            }
        })
        .collect();
    Ok((out, end))
}

/// Boxes of item `i`: its shape, its sub-items' text box, the arrow after it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Boxes {
    node: Xfrm,
    sub: Option<Xfrm>,
    arrow: Option<Xfrm>,
}

fn r(v: f64) -> Emu {
    v.round() as Emu
}

/// Lay out `n` items in a `w` × `h` frame. `subs[i]`: item i has a
/// sub-item text box (Vertical Bullet List), `sub_ratio` its height as a
/// share of the header's.
fn boxes(family: Family, subs: &[bool], sub_ratio: f64, w: f64, h: f64) -> Vec<Boxes> {
    let n = subs.len();
    if n == 0 || w <= 0.0 || h <= 0.0 {
        return Vec::new();
    }
    let nf = n as f64;
    match family {
        Family::Blocks => {
            // Boxes 10:6, gaps a tenth of a box; the column count that
            // gives the biggest boxes.
            let mut best = (0.0, 1usize);
            for c in 1..=n {
                let rows = n.div_ceil(c) as f64;
                let cf = c as f64;
                let bw = (w / (cf + 0.1 * (cf - 1.0))).min(h / (0.6 * rows + 0.1 * (rows - 1.0)));
                if bw > best.0 + 1e-6 {
                    best = (bw, c);
                }
            }
            let (bw, c) = best;
            let bh = bw * 0.6;
            let gap = bw * 0.1;
            let rows = n.div_ceil(c);
            let total_h = rows as f64 * bh + (rows as f64 - 1.0) * gap;
            let y0 = (h - total_h) / 2.0;
            (0..n)
                .map(|i| {
                    let (row, col) = (i / c, i % c);
                    let in_row = if row + 1 == rows { n - row * c } else { c } as f64;
                    let row_w = in_row * bw + (in_row - 1.0) * gap;
                    let x = (w - row_w) / 2.0 + col as f64 * (bw + gap);
                    let y = y0 + row as f64 * (bh + gap);
                    Boxes {
                        node: Xfrm::new(r(x), r(y), r(bw), r(bh)),
                        ..Boxes::default()
                    }
                })
                .collect()
        }
        Family::Bullets => {
            let with_subs = subs.iter().filter(|s| **s).count() as f64;
            let gap_k = 0.15;
            let hh = (h / (nf + with_subs * sub_ratio + (nf - 1.0) * gap_k)).min(h * 0.3);
            let mut y = 0.0;
            subs.iter()
                .map(|&has| {
                    let node = Xfrm::new(0, r(y), r(w), r(hh));
                    y += hh;
                    let sub = has.then(|| {
                        let s = Xfrm::new(0, r(y), r(w), r(hh * sub_ratio));
                        y += hh * sub_ratio;
                        s
                    });
                    y += hh * gap_k;
                    Boxes {
                        node,
                        sub,
                        arrow: None,
                    }
                })
                .collect()
        }
        Family::Process => {
            // Box 10:6; between boxes a gap of 0.1, an arrow 0.25 wide and
            // 0.3 tall, a gap of 0.1 (in box widths).
            let bw = (w / (nf + 0.45 * (nf - 1.0))).min(h / 0.6);
            let bh = bw * 0.6;
            let total = nf * bw + (nf - 1.0) * 0.45 * bw;
            let x0 = (w - total) / 2.0;
            let y = (h - bh) / 2.0;
            (0..n)
                .map(|i| {
                    let x = x0 + i as f64 * 1.45 * bw;
                    Boxes {
                        node: Xfrm::new(r(x), r(y), r(bw), r(bh)),
                        sub: None,
                        arrow: (i + 1 < n).then(|| {
                            Xfrm::new(
                                r(x + 1.1 * bw),
                                r(y + bh / 2.0 - 0.15 * bw),
                                r(0.25 * bw),
                                r(0.3 * bw),
                            )
                        }),
                    }
                })
                .collect()
        }
        // Laid out by the `tree` module.
        Family::OrgChart
        | Family::Hierarchy
        | Family::HorizontalHierarchy
        | Family::LabeledHierarchy => Vec::new(),
        Family::Cycle => {
            let m = w.min(h);
            let k = if n == 1 {
                1.0
            } else {
                (1.5 * (std::f64::consts::PI / nf).sin()).min(1.2)
            };
            let rad = (m / 2.0) / (1.0 + k / 2.0);
            let d = k * rad;
            let (cx, cy) = (w / 2.0, h / 2.0);
            let at = |t: f64| {
                let a = (-90.0 + 360.0 * t / nf).to_radians();
                (cx + rad * a.cos(), cy + rad * a.sin())
            };
            (0..n)
                .map(|i| {
                    let (x, y) = if n == 1 { (cx, cy) } else { at(i as f64) };
                    let s = d * 0.3;
                    Boxes {
                        node: Xfrm::new(r(x - d / 2.0), r(y - d / 2.0), r(d), r(d)),
                        sub: None,
                        arrow: (n > 1).then(|| {
                            let (ax, ay) = at(i as f64 + 0.5);
                            Xfrm::new(r(ax - s / 2.0), r(ay - s / 2.0), r(s), r(s))
                                .with_degrees(360.0 * (i as f64 + 0.5) / nf)
                        }),
                    }
                })
                .collect()
        }
    }
}

/// `a:off`/`a:ext` element body (and `rot`) of `x`, written as the
/// element `src` was (its tag name and prefix).
fn write_box(src: &str, old: &Range<usize>, x: &Xfrm) -> String {
    let tag = tag_name(&src[old.start..]);
    let a = tag
        .split_once(':')
        .filter(|(p, _)| *p != "dsp")
        .map(|(p, _)| format!("{p}:"))
        .unwrap_or_else(|| {
            // `dsp:txXfrm` holds `a:off`/`a:ext`: find their prefix.
            let inner = &src[old.clone()];
            inner
                .find("off ")
                .and_then(|i| inner[..i].rfind('<').map(|s| inner[s + 1..i].to_owned()))
                .unwrap_or_else(|| "a:".into())
        });
    let rot = if x.rot != 0 {
        format!(" rot=\"{}\"", x.rot)
    } else {
        String::new()
    };
    format!(
        "<{tag}{rot}><{a}off x=\"{}\" y=\"{}\"/><{a}ext cx=\"{}\" cy=\"{}\"/></{tag}>",
        x.x, x.y, x.cx, x.cy
    )
}

/// `b` (a box inside `from`) moved and scaled with it to `to`.
fn follow(from: &Xfrm, to: &Xfrm, b: &Xfrm) -> Xfrm {
    let k = |a: Emu, b: Emu| if b == 0 { 1.0 } else { a as f64 / b as f64 };
    let (sx, sy) = (k(to.cx, from.cx), k(to.cy, from.cy));
    Xfrm {
        x: to.x + ((b.x - from.x) as f64 * sx).round() as Emu,
        y: to.y + ((b.y - from.y) as f64 * sy).round() as Emu,
        cx: ((b.cx as f64 * sx).round() as Emu).max(1),
        cy: ((b.cy as f64 * sy).round() as Emu).max(1),
        rot: to.rot,
        ..*b
    }
}

/// A copy of drawing shape `xml` drawing `model` instead, its text empty.
fn copy_shape(xml: &str, model: &str) -> String {
    let mut s = xml.to_owned();
    if let Some(i) = s.find("modelId=\"")
        && let Some(j) = s[i + 9..].find('"')
    {
        s.replace_range(i + 9..i + 9 + j, model);
    }
    // The paragraphs: one empty one, with the first run's formatting.
    let full = wrap_ns(&s);
    if let Ok(doc) = Document::parse(&full) {
        let pre = wrap_pre();
        if let Some(body) = doc.descendants().find(|n| is(*n, NS_DSP, "txBody")) {
            let ps: Vec<Node> = children(body, NS_A, "p").collect();
            if let (Some(first), Some(last)) = (ps.first(), ps.last()) {
                let a = tag_name(start_tag(&full, *first))
                    .split_once(':')
                    .map(|(p, _)| format!("{p}:"))
                    .unwrap_or_default();
                let rpr = first
                    .descendants()
                    .find(|d| is(*d, NS_A, "rPr") || is(*d, NS_A, "endParaRPr"))
                    .map(|d| {
                        tag_attrs(start_tag(&full, d))
                            .trim_end_matches('/')
                            .trim()
                            .to_owned()
                    })
                    .unwrap_or_default();
                let ppr = child(*first, NS_A, "pPr")
                    .map(|p| full[p.range()].to_owned())
                    .unwrap_or_default();
                let sp = if rpr.is_empty() {
                    String::new()
                } else {
                    format!(" {rpr}")
                };
                let new = format!("<{a}p>{ppr}<{a}endParaRPr{sp}/></{a}p>");
                let range = first.range().start - pre..last.range().end - pre;
                let mut out = s.clone();
                out.replace_range(range, &new);
                return out;
            }
        }
    }
    s
}

/// The namespaces a drawing shape copied out of its part uses.
const DRAWING_NS: &str = "xmlns:dsp=\"http://schemas.microsoft.com/office/drawing/2008/diagram\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:dgm=\"http://schemas.openxmlformats.org/drawingml/2006/diagram\"";

fn wrap_ns(s: &str) -> String {
    format!("<w {DRAWING_NS}>{s}</w>")
}

fn wrap_pre() -> usize {
    format!("<w {DRAWING_NS}>").len()
}

/// Lay the drawing out again for a `w` × `h` frame from the data model;
/// returns the new drawing XML.
pub fn relayout(drawing: &str, data: &str, w: Emu, h: Emu) -> anyhow::Result<String> {
    if tree::family(data).is_some() {
        return tree::relayout(drawing, data, w, h);
    }
    let v = view(data)?;
    let (list, tree_end) = shapes(drawing, &v)?;
    let n = v.items.len();
    let has_sub = |i: usize| list.iter().any(|s| s.role == Role::Sub(i));
    let subs: Vec<bool> = (0..n).map(has_sub).collect();
    // The sub-item boxes' height as a share of the headers', as drawn.
    let avg = |want: fn(Role) -> bool| {
        let hs: Vec<f64> = list
            .iter()
            .filter(|s| want(s.role))
            .filter_map(|s| s.xfrm.as_ref().map(|x| x.1.cy as f64))
            .collect();
        (!hs.is_empty()).then(|| hs.iter().sum::<f64>() / hs.len() as f64)
    };
    let ratio = match (
        avg(|r| matches!(r, Role::Sub(_))),
        avg(|r| matches!(r, Role::Node(_))),
    ) {
        (Some(s), Some(n)) if n > 0.0 => (s / n).clamp(0.3, 4.0),
        _ => 1.4,
    };
    let lay = boxes(v.family, &subs, ratio, w as f64, h as f64);
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut arrow_drawn = vec![false; n];
    // Shapes the layout no longer has (an arrow after the last process
    // step) go.
    for s in &list {
        let target = match s.role {
            Role::Node(i) => lay.get(i).map(|b| b.node),
            Role::Sub(i) => lay.get(i).and_then(|b| b.sub),
            Role::Arrow(i) => {
                let b = lay.get(i).and_then(|b| b.arrow);
                if b.is_some() {
                    if arrow_drawn[i] {
                        None
                    } else {
                        arrow_drawn[i] = true;
                        b
                    }
                } else {
                    None
                }
            }
            Role::Other => continue,
        };
        // Items' shapes keep a turn given them; arrows turn with the layout.
        let target = target.map(|b| match (s.role, &s.xfrm) {
            (Role::Node(_) | Role::Sub(_), Some((_, old))) => Xfrm { rot: old.rot, ..b },
            _ => b,
        });
        let Some(new) = target else {
            if matches!(s.role, Role::Arrow(_) | Role::Sub(_)) {
                edits.push((s.range.clone(), String::new()));
            }
            continue;
        };
        if let Some((range, old)) = &s.xfrm {
            if let Some((trange, told)) = &s.tx {
                edits.push((
                    trange.clone(),
                    write_box(drawing, trange, &follow(old, &new, told)),
                ));
            }
            edits.push((range.clone(), write_box(drawing, range, &new)));
        }
    }
    // Arrows the layout needs that aren't drawn: copies of one that is.
    if v.family.arrows()
        && let Some(t) = list
            .iter()
            .find(|s| matches!(s.role, Role::Arrow(_)) && s.xfrm.is_some())
    {
        let mut add = String::new();
        for (i, b) in lay.iter().enumerate() {
            let (Some(abox), false, Some(sib)) =
                (b.arrow, arrow_drawn[i], v.items[i].sib.as_deref())
            else {
                continue;
            };
            let (_, old) = t.xfrm.clone().expect("checked");
            let tx = t.tx.as_ref().map(|(_, told)| follow(&old, &abox, told));
            let xml = rebox(&copy_shape(&drawing[t.range.clone()], sib), &abox, tx);
            add.push_str(&xml);
        }
        if !add.is_empty() {
            edits.push((tree_end..tree_end, add));
        }
    }
    Ok(apply(drawing, edits))
}

/// `xml` (one drawing shape) with its `a:xfrm` set to `b` and its
/// `dsp:txXfrm` (if any) to `tx`.
fn rebox(xml: &str, b: &Xfrm, tx: Option<Xfrm>) -> String {
    let full = wrap_ns(xml);
    let pre = wrap_pre();
    let Ok(doc) = Document::parse(&full) else {
        return xml.to_owned();
    };
    let mut edits = Vec::new();
    if let Some(sp) = doc.descendants().find(|n| is(*n, NS_DSP, "sp")) {
        if let Some(x) = child(sp, NS_DSP, "spPr").and_then(|s| child(s, NS_A, "xfrm")) {
            let r = x.range();
            edits.push((r.start - pre..r.end - pre, write_box(&full, &r, b)));
        }
        if let (Some(x), Some(t)) = (child(sp, NS_DSP, "txXfrm"), tx) {
            let r = x.range();
            edits.push((r.start - pre..r.end - pre, write_box(&full, &r, &t)));
        }
    }
    apply(xml, edits)
}

fn apply(src: &str, mut edits: Vec<(Range<usize>, String)>) -> String {
    edits.sort_by_key(|(r, _)| std::cmp::Reverse((r.start, r.end)));
    let mut out = src.to_owned();
    let mut last = usize::MAX;
    for (r, t) in edits {
        if r.end > last {
            continue;
        }
        last = r.start;
        out.replace_range(r, &t);
    }
    out
}

/// A model id not used in `data` (a GUID derived from it).
fn new_id(data: &str, salt: u32) -> String {
    use std::hash::{Hash, Hasher};
    let mut k = 0u32;
    loop {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        data.len().hash(&mut h);
        data.hash(&mut h);
        (salt, k).hash(&mut h);
        let a = h.finish();
        (a, 1u8).hash(&mut h);
        let b = h.finish();
        let id = format!(
            "{{{:08X}-{:04X}-4{:03X}-8{:03X}-{:012X}}}",
            a >> 32,
            (a >> 16) & 0xFFFF,
            a & 0xFFF,
            (b >> 48) & 0xFFF,
            b & 0xFFFF_FFFF_FFFF
        );
        if !data.contains(&id) {
            return id;
        }
        k += 1;
    }
}

/// The new item's model id and the parts with it added next to the item
/// whose drawing shape is `selected` (a `modelId`).
pub fn add_item(
    drawing: &str,
    data: &str,
    selected: &str,
    place: AddPlace,
    w: Emu,
    h: Emu,
) -> anyhow::Result<(String, String, String)> {
    if tree::family(data).is_some() {
        return tree::add_item(drawing, data, selected, place, w, h);
    }
    let v = view(data)?;
    if matches!(place, AddPlace::Above | AddPlace::Below) {
        bail!("This SmartArt layout has one level: add shapes before or after");
    }
    let i = v
        .item_of(selected)
        .context("Select an item's shape to add a shape next to it")?;
    let at = if place == AddPlace::After { i + 1 } else { i };
    let doc = Document::parse(data).context("SmartArt data")?;
    let dgm = doc
        .root_element()
        .tag_name()
        .namespace()
        .and_then(|ns| doc.root_element().lookup_prefix(ns))
        .map(|p| format!("{p}:"))
        .unwrap_or_default();
    let a = doc
        .root_element()
        .lookup_prefix(NS_A)
        .map(|p| format!("{p}:"))
        .unwrap_or_else(|| "a:".into());
    let node = new_id(data, 1);
    let par = new_id(data, 2);
    let sib = new_id(data, 3);
    let cxn = new_id(data, 4);
    let body = |pre: &str| {
        format!(
            "<{dgm}t><{a}bodyPr/><{a}lstStyle/><{a}p><{a}endParaRPr lang=\"en-US\"/></{a}p></{dgm}t>{pre}"
        )
    };
    let pts = format!(
        "<{dgm}pt modelId=\"{node}\"><{dgm}prSet phldrT=\"[Text]\"/><{dgm}spPr/>{}</{dgm}pt>\
         <{dgm}pt modelId=\"{par}\" type=\"parTrans\" cxnId=\"{cxn}\"><{dgm}prSet/><{dgm}spPr/>{}</{dgm}pt>\
         <{dgm}pt modelId=\"{sib}\" type=\"sibTrans\" cxnId=\"{cxn}\"><{dgm}prSet/><{dgm}spPr/>{}</{dgm}pt>",
        body(""),
        body(""),
        body("")
    );
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let pt_lst = doc
        .descendants()
        .find(|n| is(*n, NS_DGM, "ptLst"))
        .context("SmartArt data without items")?;
    let close = |n: Node| n.range().end - format!("</{}>", tag_name(start_tag(data, n))).len();
    edits.push((close(pt_lst)..close(pt_lst), pts));
    let cxn_lst = doc
        .descendants()
        .find(|n| is(*n, NS_DGM, "cxnLst"))
        .context("SmartArt data without connections")?;
    // Later siblings move one place on.
    for c in cxn_lst.children().filter(|n| is(*n, NS_DGM, "cxn")) {
        if c.attribute("type").is_none_or(|t| t == "parOf")
            && c.attribute("srcId") == Some(v.doc.as_str())
            && let Some(ord) = attr_i64(c, "srcOrd")
            && ord as usize >= at
        {
            edits.push(set_attr(data, c, "srcOrd", &(ord + 1).to_string()));
        }
    }
    edits.push((
        close(cxn_lst)..close(cxn_lst),
        format!(
            "<{dgm}cxn modelId=\"{cxn}\" srcId=\"{}\" destId=\"{node}\" srcOrd=\"{at}\" destOrd=\"0\" parTransId=\"{par}\" sibTransId=\"{sib}\"/>",
            v.doc
        ),
    ));
    let data2 = apply(data, edits);
    // The drawing: a copy of the selected item's shape.
    let (list, _) = shapes(drawing, &v)?;
    let template = list
        .iter()
        .find(|s| s.role == Role::Node(i))
        .or_else(|| list.iter().find(|s| matches!(s.role, Role::Node(_))))
        .context("This SmartArt has no shape to copy")?;
    let copy = copy_shape(&drawing[template.range.clone()], &node);
    let insert = template.range.end;
    let drawing2 = apply(drawing, vec![(insert..insert, copy)]);
    let drawing3 = relayout(&drawing2, &data2, w, h)?;
    Ok((drawing3, data2, node))
}

/// The parts with the item whose drawing shape is `selected` removed (with
/// its sub-items, transitions, presentation items and shapes).
pub fn remove_item(
    drawing: &str,
    data: &str,
    selected: &str,
    w: Emu,
    h: Emu,
) -> anyhow::Result<(String, String)> {
    if tree::family(data).is_some() {
        return tree::remove_item(drawing, data, selected, w, h);
    }
    let v = view(data)?;
    let i = v
        .item_of(selected)
        .context("This SmartArt shape can't be deleted: it doesn't show an item")?;
    if v.items.len() <= 1 {
        bail!("A SmartArt keeps at least one item");
    }
    let item = &v.items[i];
    let doc = Document::parse(data).context("SmartArt data")?;
    let mut gone: Vec<String> = std::iter::once(item.id.clone())
        .chain(item.subs.iter().cloned())
        .collect();
    let cxns: Vec<Node> = doc
        .descendants()
        .filter(|n| is(*n, NS_DGM, "cxn"))
        .collect();
    // Connections into removed items, and their transitions.
    for c in &cxns {
        if c.attribute("type").is_none_or(|t| t == "parOf")
            && c.attribute("destId")
                .is_some_and(|d| gone.iter().any(|g| g == d))
        {
            for k in ["modelId", "parTransId", "sibTransId"] {
                if let Some(x) = c.attribute(k) {
                    gone.push(x.to_owned());
                }
            }
        }
    }
    // Presentation items of removed items.
    let pres: Vec<String> = v
        .pres
        .iter()
        .filter(|(_, (a, _))| gone.iter().any(|g| g == a))
        .map(|(p, _)| p.clone())
        .collect();
    gone.extend(pres);
    let is_gone = |s: Option<&str>| s.is_some_and(|s| gone.iter().any(|g| g == s));
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    for p in doc.descendants().filter(|n| is(*n, NS_DGM, "pt")) {
        if is_gone(p.attribute("modelId")) {
            edits.push((p.range(), String::new()));
        }
    }
    let removed_ord = cxns
        .iter()
        .find(|c| c.attribute("destId") == Some(item.id.as_str()))
        .and_then(|c| attr_i64(*c, "srcOrd"));
    for c in &cxns {
        if is_gone(c.attribute("modelId"))
            || is_gone(c.attribute("srcId"))
            || is_gone(c.attribute("destId"))
        {
            edits.push((c.range(), String::new()));
        } else if c.attribute("type").is_none_or(|t| t == "parOf")
            && c.attribute("srcId") == Some(v.doc.as_str())
            && let (Some(ord), Some(gone_ord)) = (attr_i64(*c, "srcOrd"), removed_ord)
            && ord > gone_ord
        {
            edits.push(set_attr(data, *c, "srcOrd", &(ord - 1).to_string()));
        }
    }
    let data2 = apply(data, edits);
    let (list, _) = shapes(drawing, &v)?;
    let mut dedits = Vec::new();
    for s in &list {
        let target = v
            .pres
            .get(&s.model)
            .map(|p| p.0.as_str())
            .unwrap_or(&s.model);
        if gone.iter().any(|g| g == target || *g == s.model) {
            dedits.push((s.range.clone(), String::new()));
        }
    }
    let drawing2 = apply(drawing, dedits);
    let drawing3 = relayout(&drawing2, &data2, w, h)?;
    Ok((drawing3, data2))
}

/// An edit setting attribute `k` of element `n`'s start tag.
fn set_attr(src: &str, n: Node, k: &str, val: &str) -> (Range<usize>, String) {
    let tag = start_tag(src, n);
    let mut attrs = split_attrs(tag_attrs(tag));
    match attrs.iter_mut().find(|(a, _)| a == k) {
        Some(a) => a.1 = val.to_owned(),
        None => attrs.push((k.to_owned(), val.to_owned())),
    }
    let mut out = format!("<{}", tag_name(tag));
    write_attrs(&mut out, &attrs);
    out.push_str(if tag.ends_with("/>") { "/>" } else { ">" });
    let st = n.range().start;
    (st..st + tag.len(), out)
}

#[path = "smartart_tree.rs"]
mod tree;
pub use tree::{LABEL_LEVELS, TOP_ALONE, TOP_KEEP};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_lists_centre_their_last_row() {
        let b = boxes(Family::Blocks, &[false; 5], 1.0, 1000.0, 500.0);
        assert_eq!(b.len(), 5);
        // Three columns: two rows, the second (two boxes) centred.
        assert_eq!(b[0].node.y, b[2].node.y);
        assert!(b[3].node.y > b[0].node.y);
        let row2 = (b[3].node.x + b[4].node.x + b[4].node.cx) as f64 / 2.0;
        assert!((row2 - 500.0).abs() < 2.0, "{row2}");
        // 10:6 boxes.
        assert!((b[0].node.cy as f64 / b[0].node.cx as f64 - 0.6).abs() < 0.01);
    }

    #[test]
    fn processes_and_cycles_have_arrows_between_items() {
        let p = boxes(Family::Process, &[false; 3], 1.0, 1450.0, 1000.0);
        assert!(p[0].arrow.is_some() && p[1].arrow.is_some() && p[2].arrow.is_none());
        let a = p[0].arrow.unwrap();
        assert!(a.x > p[0].node.x + p[0].node.cx && a.x + a.cx < p[1].node.x);
        let c = boxes(Family::Cycle, &[false; 4], 1.0, 1000.0, 1000.0);
        // Round a circle: the first at the top, the third at the bottom.
        assert!(c[0].node.y < c[1].node.y && c[2].node.y > c[1].node.y);
        assert!(c.iter().all(|b| b.arrow.is_some()));
        assert_eq!(c[0].arrow.unwrap().rot, 45 * 60_000);
    }

    #[test]
    fn bullet_lists_stack_headers_and_sub_items() {
        let b = boxes(Family::Bullets, &[true, false, true], 1.5, 1000.0, 1000.0);
        let s0 = b[0].sub.unwrap();
        assert_eq!(s0.y, b[0].node.y + b[0].node.cy);
        assert!(b[1].sub.is_none() && b[1].node.y > s0.y + s0.cy);
        assert!(b[2].sub.unwrap().y + b[2].sub.unwrap().cy <= 1001);
    }
}
