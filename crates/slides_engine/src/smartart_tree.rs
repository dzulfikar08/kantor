//! Kantor's own layout of the hierarchy SmartArt layouts: Organization
//! Chart (`layout/orgChart1`), Hierarchy (`layout/hierarchy1`),
//! Horizontal Hierarchy (`layout/hierarchy2`) and Labeled Hierarchy
//! (`layout/hierarchy6`). A child of [`super`] (the list layouts), sharing
//! its helpers.
//!
//! In a Labeled Hierarchy the document's first top-level item is the
//! tree's top; its later top-level items are the level labels, each drawn
//! as a band across the diagram behind a level of the tree with its text
//! at the left (PowerPoint's text pane: "the first line of Level 1 text
//! appears in the shape at the top of the hierarchy, the others in the
//! wide rectangles"). Labels take items before and after them.
//!
//! The data model's items form a tree (`parOf` connections from the
//! document item down); each item is drawn by one or more shapes (Hierarchy
//! draws a background and a text box) and each item with a parent by a
//! connector, the shape naming the item's parent transition. Laying out
//! again places every item's shapes by the tree (a "tidy" tree: leaves side
//! by side, parents centred over their children, boxes 10:6) and redraws
//! the connectors as elbow lines (a `custGeom` path). Items are added
//! after, before, above or below the selected one, as PowerPoint's Add
//! Shape does, and a deleted item's children move up to its parent.

use super::*;

/// One item of the tree.
#[derive(Clone, Debug)]
struct TNode {
    id: String,
    parent: Option<usize>,
    kids: Vec<usize>,
    /// Parent and sibling transitions and the connection from the parent.
    par: Option<String>,
    sib: Option<String>,
    cxn: Option<String>,
    /// Its place among its siblings (`srcOrd`).
    ord: i64,
    depth: usize,
}

struct Tree {
    family: Family,
    doc: String,
    nodes: Vec<TNode>,
    roots: Vec<usize>,
    /// Labeled Hierarchy: the level labels, top down.
    labels: Vec<usize>,
    /// Presentation item → the data item it shows.
    pres: HashMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TRole {
    Node(usize),
    Conn(usize),
    Other,
}

impl Tree {
    fn target<'a>(&'a self, model: &'a str) -> &'a str {
        self.pres.get(model).map_or(model, String::as_str)
    }

    fn role(&self, model: &str) -> TRole {
        let t = self.target(model);
        if let Some(k) = self.nodes.iter().position(|n| n.id == t) {
            return TRole::Node(k);
        }
        if let Some(k) = self.nodes.iter().position(|n| n.par.as_deref() == Some(t)) {
            return TRole::Conn(k);
        }
        TRole::Other
    }

    fn node_of(&self, model: &str) -> Option<usize> {
        match self.role(model) {
            TRole::Node(k) => Some(k),
            _ => None,
        }
    }

    fn parent_id(&self, k: usize) -> &str {
        match self.nodes[k].parent {
            Some(p) => &self.nodes[p].id,
            None => &self.doc,
        }
    }
}

fn tree(data: &str) -> anyhow::Result<Tree> {
    let doc = Document::parse(data).context("SmartArt data")?;
    let pts: Vec<Node> = doc.descendants().filter(|n| is(*n, NS_DGM, "pt")).collect();
    let root = pts
        .iter()
        .find(|p| p.attribute("type") == Some("doc"))
        .context("SmartArt data without a document item")?;
    let lo = child(*root, NS_DGM, "prSet")
        .and_then(|p| p.attribute("loTypeId"))
        .unwrap_or_default();
    let family = Family::of(lo)
        .filter(|f| f.is_tree())
        .context(LAYOUT_REFUSAL)?;
    let items: Vec<&str> = pts
        .iter()
        .filter(|p| matches!(p.attribute("type"), None | Some("node" | "asst")))
        .filter_map(|p| p.attribute("modelId"))
        .collect();
    // (srcOrd, dest, parTrans, sibTrans, cxn) per source.
    type Edge<'a> = (
        i64,
        &'a str,
        Option<&'a str>,
        Option<&'a str>,
        Option<&'a str>,
    );
    let mut kids: HashMap<&str, Vec<Edge>> = HashMap::new();
    for c in doc.descendants().filter(|n| is(*n, NS_DGM, "cxn")) {
        if c.attribute("type").is_some_and(|t| t != "parOf") {
            continue;
        }
        let (Some(src), Some(dst)) = (c.attribute("srcId"), c.attribute("destId")) else {
            continue;
        };
        if !items.contains(&dst) {
            continue;
        }
        kids.entry(src).or_default().push((
            attr_i64(c, "srcOrd").unwrap_or(0),
            dst,
            c.attribute("parTransId"),
            c.attribute("sibTransId"),
            c.attribute("modelId"),
        ));
    }
    for v in kids.values_mut() {
        v.sort_by_key(|k| k.0);
    }
    let doc_id = root.attribute("modelId").unwrap_or_default().to_owned();
    let mut t = Tree {
        family,
        doc: doc_id.clone(),
        nodes: Vec::new(),
        roots: Vec::new(),
        labels: Vec::new(),
        pres: HashMap::new(),
    };
    fn walk(
        t: &mut Tree,
        kids: &HashMap<&str, Vec<Edge>>,
        src: &str,
        parent: Option<usize>,
        depth: usize,
    ) {
        for (ord, dst, par, sib, cxn) in kids.get(src).into_iter().flatten() {
            if t.nodes.len() >= 5_000 || t.nodes.iter().any(|n| n.id == *dst) {
                continue;
            }
            let k = t.nodes.len();
            t.nodes.push(TNode {
                id: (*dst).to_owned(),
                parent,
                kids: Vec::new(),
                par: par.map(str::to_owned),
                sib: sib.map(str::to_owned),
                cxn: cxn.map(str::to_owned),
                ord: *ord,
                depth,
            });
            match parent {
                Some(p) => t.nodes[p].kids.push(k),
                None if t.family == Family::LabeledHierarchy && !t.roots.is_empty() => {
                    // A level label: its own items (if any) aren't drawn.
                    t.labels.push(k);
                    continue;
                }
                None => t.roots.push(k),
            }
            walk(t, kids, dst, Some(k), depth + 1);
        }
    }
    walk(&mut t, &kids, &doc_id, None, 0);
    if t.nodes.is_empty() {
        bail!("This SmartArt has no items");
    }
    t.pres = pts
        .iter()
        .filter(|p| p.attribute("type") == Some("pres"))
        .filter_map(|p| {
            let pr = child(*p, NS_DGM, "prSet")?;
            Some((
                p.attribute("modelId")?.to_owned(),
                pr.attribute("presAssocID")?.to_owned(),
            ))
        })
        .collect();
    Ok(t)
}

/// A drawing shape of a tree SmartArt.
struct TShape {
    range: Range<usize>,
    model: String,
    xfrm: Option<(Range<usize>, Xfrm)>,
    tx: Option<(Range<usize>, Xfrm)>,
    role: TRole,
}

fn tshapes(drawing: &str, t: &Tree) -> anyhow::Result<(Vec<TShape>, usize)> {
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
            let sppr = child(n, NS_DSP, "spPr");
            TShape {
                range: n.range(),
                role: t.role(&model),
                model,
                xfrm: sppr
                    .and_then(|s| child(s, NS_A, "xfrm"))
                    .and_then(|x| Some((x.range(), xfrm_of(x)?))),
                tx: child(n, NS_DSP, "txXfrm").and_then(|x| Some((x.range(), xfrm_of(x)?))),
            }
        })
        .collect();
    Ok((out, end))
}

/// The family of a tree SmartArt Kantor lays out.
pub(super) fn family(data: &str) -> Option<Family> {
    tree(data).ok().map(|t| t.family)
}

/// Gaps between boxes: across a level and between levels (box widths /
/// heights).
const GAP_ACROSS: f64 = 0.25;
const GAP_LEVELS: f64 = 0.5;
/// Labeled Hierarchy: the labels' column (share of the width), and how
/// much taller than its boxes a level's band is (box heights).
const LABEL_COLUMN: f64 = 0.2;
const BAND_PAD: f64 = 0.4;

/// Each item's box in a `w` × `h` frame.
fn place(t: &Tree, w: f64, h: f64) -> Vec<Xfrm> {
    let n = t.nodes.len();
    let mut leaves = vec![0usize; n];
    fn count(t: &Tree, k: usize, leaves: &mut [usize]) -> usize {
        let c = if t.nodes[k].kids.is_empty() {
            1
        } else {
            t.nodes[k].kids.iter().map(|c| count(t, *c, leaves)).sum()
        };
        leaves[k] = c;
        c
    }
    let slots: usize = t.roots.iter().map(|r| count(t, *r, &mut leaves)).sum();
    let levels = t.nodes.iter().map(|n| n.depth + 1).max().unwrap_or(1);
    let labeled = t.family == Family::LabeledHierarchy;
    // A Labeled Hierarchy: a row per level or label, the labels' column
    // at the left.
    let levels = if labeled {
        levels.max(t.labels.len())
    } else {
        levels
    };
    let label_w = if labeled { w * LABEL_COLUMN } else { 0.0 };
    let (s, l) = (slots.max(1) as f64, levels as f64);
    let horizontal = t.family == Family::HorizontalHierarchy;
    // Box width (10:6 boxes) that fits both ways.
    let (across, along) = if horizontal { (h, w) } else { (w - label_w, h) };
    let bw = if labeled {
        // Rows of bands (a box and some room round it) one under another.
        let by_slots = across / (s + GAP_ACROSS * (s - 1.0));
        let by_levels = along / (l * (1.0 + GAP_LEVELS) - (GAP_LEVELS - BAND_PAD)) / 0.6;
        by_slots.min(by_levels)
    } else if horizontal {
        // Slots run down (box heights), levels across (box widths).
        let by_slots = across / (s + GAP_ACROSS * (s - 1.0)) / 0.6;
        let by_levels = along / (l + GAP_LEVELS * (l - 1.0));
        by_slots.min(by_levels)
    } else {
        let by_slots = across / (s + GAP_ACROSS * (s - 1.0));
        let by_levels = along / (l + GAP_LEVELS * (l - 1.0)) / 0.6;
        by_slots.min(by_levels)
    };
    let bh = bw * 0.6;
    // Pitches: between slot centres, between level starts.
    let (slot, size_across, level, size_along) = if horizontal {
        (bh * (1.0 + GAP_ACROSS), bh, bw * (1.0 + GAP_LEVELS), bw)
    } else {
        (bw * (1.0 + GAP_ACROSS), bw, bh * (1.0 + GAP_LEVELS), bh)
    };
    let total_across = s * slot - (slot - size_across);
    // A band reaches past its boxes by half its padding each way.
    let band_pad = if labeled { bh * BAND_PAD / 2.0 } else { 0.0 };
    let total_along = l * level - (level - size_along) + 2.0 * band_pad;
    let a0 = label_w + (across - total_across) / 2.0;
    let l0 = (along - total_along) / 2.0 + band_pad;
    // Centre of each item across: leaves in slot order, parents over the
    // middle of their children.
    let mut centre = vec![0.0f64; n];
    fn assign(t: &Tree, k: usize, next: &mut f64, slot: f64, centre: &mut [f64]) {
        if t.nodes[k].kids.is_empty() {
            centre[k] = *next;
            *next += slot;
            return;
        }
        for c in &t.nodes[k].kids {
            assign(t, *c, next, slot, centre);
        }
        let first = centre[t.nodes[k].kids[0]];
        let last = centre[*t.nodes[k].kids.last().expect("kids")];
        centre[k] = (first + last) / 2.0;
    }
    let mut next = a0 + size_across / 2.0;
    for r in &t.roots {
        assign(t, *r, &mut next, slot, &mut centre);
    }
    let band = |row: usize| {
        let y = l0 + row as f64 * level - band_pad;
        Xfrm::new(0, r(y), r(w), r(bh + 2.0 * band_pad))
    };
    (0..n)
        .map(|k| {
            if let Some(row) = t.labels.iter().position(|x| *x == k) {
                return band(row);
            }
            let c = centre[k];
            let at = l0 + t.nodes[k].depth as f64 * level;
            if horizontal {
                Xfrm::new(r(at), r(c - bh / 2.0), r(bw), r(bh))
            } else {
                Xfrm::new(r(c - bw / 2.0), r(at), r(bw), r(bh))
            }
        })
        .collect()
}

/// The elbow line from a parent's box to a child's (frame EMUs).
fn elbow(horizontal: bool, p: &Xfrm, c: &Xfrm) -> Vec<(Emu, Emu)> {
    if horizontal {
        let (x0, y0) = (p.x + p.cx, p.y + p.cy / 2);
        let (x1, y1) = (c.x, c.y + c.cy / 2);
        let mx = x0 + (x1 - x0) / 2;
        vec![(x0, y0), (mx, y0), (mx, y1), (x1, y1)]
    } else {
        let (x0, y0) = (p.x + p.cx / 2, p.y + p.cy);
        let (x1, y1) = (c.x + c.cx / 2, c.y);
        let my = y0 + (y1 - y0) / 2;
        vec![(x0, y0), (x0, my), (x1, my), (x1, y1)]
    }
}

/// A polyline as a shape box and its `custGeom` (prefix `a` for
/// DrawingML).
fn polyline_geom(pts: &[(Emu, Emu)], a: &str) -> (Xfrm, String) {
    let x0 = pts.iter().map(|p| p.0).min().unwrap_or(0);
    let y0 = pts.iter().map(|p| p.1).min().unwrap_or(0);
    let x1 = pts.iter().map(|p| p.0).max().unwrap_or(0);
    let y1 = pts.iter().map(|p| p.1).max().unwrap_or(0);
    let (w, h) = ((x1 - x0).max(1), (y1 - y0).max(1));
    let mut path = String::new();
    for (i, (x, y)) in pts.iter().enumerate() {
        let op = if i == 0 { "moveTo" } else { "lnTo" };
        path.push_str(&format!(
            "<{a}{op}><{a}pt x=\"{}\" y=\"{}\"/></{a}{op}>",
            x - x0,
            y - y0
        ));
    }
    let geom = format!(
        "<{a}custGeom><{a}avLst/><{a}gdLst/><{a}ahLst/><{a}cxnLst/><{a}rect l=\"0\" t=\"0\" r=\"0\" b=\"0\"/>\
         <{a}pathLst><{a}path w=\"{w}\" h=\"{h}\" fill=\"none\">{path}</{a}path></{a}pathLst></{a}custGeom>"
    );
    (Xfrm::new(x0, y0, w, h), geom)
}

/// The DrawingML prefix (with its colon) of element source `src`.
fn prefix_of(src: &str) -> String {
    let tag = tag_name(src.trim_start());
    tag.split_once(':')
        .map(|(p, _)| format!("{p}:"))
        .unwrap_or_default()
}

/// A connector shape for `model` when the drawing has none to copy.
fn new_connector(model: &str) -> String {
    format!(
        "<dsp:sp modelId=\"{model}\"><dsp:nvSpPr><dsp:cNvPr id=\"0\" name=\"\"/><dsp:cNvSpPr/></dsp:nvSpPr>\
         <dsp:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"1\" cy=\"1\"/></a:xfrm>\
         <a:custGeom><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:rect l=\"0\" t=\"0\" r=\"0\" b=\"0\"/><a:pathLst/></a:custGeom>\
         <a:noFill/><a:ln w=\"12700\"><a:solidFill><a:schemeClr val=\"accent1\"><a:shade val=\"60000\"/></a:schemeClr></a:solidFill></a:ln></dsp:spPr>\
         <dsp:style><a:lnRef idx=\"2\"><a:scrgbClr r=\"0\" g=\"0\" b=\"0\"/></a:lnRef><a:fillRef idx=\"0\"><a:scrgbClr r=\"0\" g=\"0\" b=\"0\"/></a:fillRef>\
         <a:effectRef idx=\"0\"><a:scrgbClr r=\"0\" g=\"0\" b=\"0\"/></a:effectRef><a:fontRef idx=\"minor\"/></dsp:style></dsp:sp>"
    )
}

/// One connector shape (`xml`, standalone) drawn as `pts`.
fn draw_connector(xml: &str, pts: &[(Emu, Emu)]) -> String {
    let full = wrap_ns(xml);
    let pre = wrap_pre();
    let Ok(doc) = Document::parse(&full) else {
        return xml.to_owned();
    };
    let Some(sp) = doc.descendants().find(|n| is(*n, NS_DSP, "sp")) else {
        return xml.to_owned();
    };
    let Some(sppr) = child(sp, NS_DSP, "spPr") else {
        return xml.to_owned();
    };
    let mut edits = Vec::new();
    let xf = child(sppr, NS_A, "xfrm");
    let a = xf.map_or_else(|| "a:".to_owned(), |x| prefix_of(start_tag(&full, x)));
    let (bx, geom) = polyline_geom(pts, &a);
    match sppr
        .children()
        .find(|c| is(*c, NS_A, "prstGeom") || is(*c, NS_A, "custGeom"))
    {
        Some(g) => {
            let r = g.range();
            edits.push((r.start - pre..r.end - pre, geom));
        }
        None => {
            let at = xf.map_or_else(
                || start_tag(&full, sppr).len() + sppr.range().start,
                |x| x.range().end,
            ) - pre;
            edits.push((at..at, geom));
        }
    }
    if let Some(x) = xf {
        let r = x.range();
        edits.push((r.start - pre..r.end - pre, write_box(&full, &r, &bx)));
    }
    // The connector has no text: its text box follows it.
    if let Some(t) = child(sp, NS_DSP, "txXfrm") {
        let r = t.range();
        edits.push((r.start - pre..r.end - pre, write_box(&full, &r, &bx)));
    }
    apply(xml, edits)
}

fn union(xs: &[Xfrm]) -> Option<Xfrm> {
    let x0 = xs.iter().map(|x| x.x).min()?;
    let y0 = xs.iter().map(|x| x.y).min()?;
    let x1 = xs.iter().map(|x| x.x + x.cx).max()?;
    let y1 = xs.iter().map(|x| x.y + x.cy).max()?;
    Some(Xfrm::new(x0, y0, (x1 - x0).max(1), (y1 - y0).max(1)))
}

/// Lay a tree SmartArt out again for a `w` × `h` frame.
pub(super) fn relayout(drawing: &str, data: &str, w: Emu, h: Emu) -> anyhow::Result<String> {
    let t = tree(data)?;
    let (list, tree_end) = tshapes(drawing, &t)?;
    let boxes = place(&t, w as f64, h as f64);
    let horizontal = t.family == Family::HorizontalHierarchy;
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    // Items: their shapes moved and scaled with the box round them all.
    let mut drawn = vec![false; t.nodes.len()];
    for (k, b) in boxes.iter().enumerate() {
        let mine: Vec<&TShape> = list.iter().filter(|s| s.role == TRole::Node(k)).collect();
        let olds: Vec<Xfrm> = mine
            .iter()
            .filter_map(|s| s.xfrm.as_ref().map(|x| x.1))
            .collect();
        let Some(u) = union(&olds) else { continue };
        drawn[k] = true;
        for s in mine {
            let Some((range, old)) = &s.xfrm else {
                continue;
            };
            let new = Xfrm {
                rot: old.rot,
                ..follow(&u, b, old)
            };
            if let Some((trange, told)) = &s.tx {
                edits.push((
                    trange.clone(),
                    write_box(drawing, trange, &follow(old, &new, told)),
                ));
            }
            edits.push((range.clone(), write_box(drawing, range, &new)));
        }
    }
    // Items without shapes (made elsewhere): copies of another item's.
    let mut add = String::new();
    if let Some(tk) = drawn.iter().position(|d| *d) {
        for k in (0..t.nodes.len()).filter(|k| !drawn[*k]) {
            let mine: Vec<&TShape> = list.iter().filter(|s| s.role == TRole::Node(tk)).collect();
            let olds: Vec<Xfrm> = mine
                .iter()
                .filter_map(|s| s.xfrm.as_ref().map(|x| x.1))
                .collect();
            let Some(u) = union(&olds) else { continue };
            for s in mine {
                let Some((_, old)) = &s.xfrm else { continue };
                let new = follow(&u, &boxes[k], old);
                let tx = s.tx.as_ref().map(|(_, told)| follow(old, &new, told));
                add.push_str(&rebox(
                    &copy_shape(&drawing[s.range.clone()], &t.nodes[k].id),
                    &new,
                    tx,
                ));
            }
        }
    }
    // Connectors: one per item with a parent.
    let mut has_conn = vec![false; t.nodes.len()];
    for s in &list {
        let TRole::Conn(k) = s.role else { continue };
        let Some(p) = t.nodes[k].parent else {
            edits.push((s.range.clone(), String::new()));
            continue;
        };
        if has_conn[k] {
            edits.push((s.range.clone(), String::new()));
            continue;
        }
        has_conn[k] = true;
        let pts = elbow(horizontal, &boxes[p], &boxes[k]);
        edits.push((
            s.range.clone(),
            draw_connector(&drawing[s.range.clone()], &pts),
        ));
    }
    let template = list
        .iter()
        .find(|s| matches!(s.role, TRole::Conn(_)))
        .map(|s| &drawing[s.range.clone()]);
    for (k, n) in t.nodes.iter().enumerate() {
        let (Some(p), false, Some(par)) = (n.parent, has_conn[k], n.par.as_deref()) else {
            continue;
        };
        let base = match template {
            Some(x) => copy_shape(x, par),
            None => new_connector(par),
        };
        add.push_str(&draw_connector(
            &base,
            &elbow(horizontal, &boxes[p], &boxes[k]),
        ));
    }
    // New shapes go first (behind the items): connectors under the boxes
    // (but over a Labeled Hierarchy's bands).
    if !add.is_empty() {
        let after_bands = list
            .iter()
            .filter(|s| matches!(s.role, TRole::Node(k) if t.labels.contains(&k)))
            .map(|s| s.range.end)
            .max();
        let first = after_bands.unwrap_or_else(|| list.first().map_or(tree_end, |s| s.range.start));
        edits.push((first..first, add));
    }
    Ok(apply(drawing, edits))
}

/// An edit setting several attributes of element `n`'s start tag.
fn set_attrs(src: &str, n: Node, kv: &[(&str, String)]) -> (Range<usize>, String) {
    let tag = start_tag(src, n);
    let mut attrs = split_attrs(tag_attrs(tag));
    for (k, v) in kv {
        match attrs.iter_mut().find(|(a, _)| a == k) {
            Some(a) => a.1 = v.clone(),
            None => attrs.push(((*k).to_owned(), v.clone())),
        }
    }
    let mut out = format!("<{}", tag_name(tag));
    write_attrs(&mut out, &attrs);
    out.push_str(if tag.ends_with("/>") { "/>" } else { ">" });
    let st = n.range().start;
    (st..st + tag.len(), out)
}

/// Labeled Hierarchy messages (translated by the apps).
pub const LABEL_LEVELS: &str = "A Labeled Hierarchy's labels take no shapes above or below them: add labels before or after them";
pub const TOP_ALONE: &str =
    "The top of a Labeled Hierarchy has no shapes beside it: add shapes below or above it";
pub const TOP_KEEP: &str =
    "The top of this Labeled Hierarchy can't be deleted while it has several shapes under it";

fn is_par_of(c: Node) -> bool {
    c.attribute("type").is_none_or(|t| t == "parOf")
}

/// Add an item next to the one drawn by `selected` (a `modelId`); returns
/// the drawing, the data model and the new item's id.
pub(super) fn add_item(
    drawing: &str,
    data: &str,
    selected: &str,
    place: AddPlace,
    w: Emu,
    h: Emu,
) -> anyhow::Result<(String, String, String)> {
    let t = tree(data)?;
    let k = t
        .node_of(selected)
        .context("Select an item's shape to add a shape next to it")?;
    if t.labels.contains(&k) && matches!(place, AddPlace::Above | AddPlace::Below) {
        bail!(LABEL_LEVELS);
    }
    if t.family == Family::LabeledHierarchy
        && t.roots.contains(&k)
        && matches!(place, AddPlace::After | AddPlace::Before)
    {
        bail!(TOP_ALONE);
    }
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
    let body = format!(
        "<{dgm}t><{a}bodyPr/><{a}lstStyle/><{a}p><{a}endParaRPr lang=\"en-US\"/></{a}p></{dgm}t>"
    );
    let pts = format!(
        "<{dgm}pt modelId=\"{node}\"><{dgm}prSet phldrT=\"[Text]\"/><{dgm}spPr/>{body}</{dgm}pt>\
         <{dgm}pt modelId=\"{par}\" type=\"parTrans\" cxnId=\"{cxn}\"><{dgm}prSet/><{dgm}spPr/>{body}</{dgm}pt>\
         <{dgm}pt modelId=\"{sib}\" type=\"sibTrans\" cxnId=\"{cxn}\"><{dgm}prSet/><{dgm}spPr/>{body}</{dgm}pt>"
    );
    let close = |n: Node| n.range().end - format!("</{}>", tag_name(start_tag(data, n))).len();
    let pt_lst = doc
        .descendants()
        .find(|n| is(*n, NS_DGM, "ptLst"))
        .context("SmartArt data without items")?;
    let cxn_lst = doc
        .descendants()
        .find(|n| is(*n, NS_DGM, "cxnLst"))
        .context("SmartArt data without connections")?;
    let mut edits: Vec<(Range<usize>, String)> = vec![(close(pt_lst)..close(pt_lst), pts)];
    let me = &t.nodes[k];
    let (src, ord) = match place {
        AddPlace::After | AddPlace::Before => {
            let ord = me.ord + i64::from(place == AddPlace::After);
            let parent = t.parent_id(k).to_owned();
            // Later siblings move one place on.
            for c in cxn_lst.children().filter(|n| is(*n, NS_DGM, "cxn")) {
                if is_par_of(c)
                    && c.attribute("srcId") == Some(parent.as_str())
                    && let Some(o) = attr_i64(c, "srcOrd")
                    && o >= ord
                {
                    edits.push(set_attrs(data, c, &[("srcOrd", (o + 1).to_string())]));
                }
            }
            (parent, ord)
        }
        AddPlace::Below => {
            let ord = me
                .kids
                .iter()
                .map(|c| t.nodes[*c].ord + 1)
                .max()
                .unwrap_or(0);
            (me.id.clone(), ord)
        }
        AddPlace::Above => {
            // The new item takes the selected one's place; it goes under
            // the new one.
            let c = cxn_lst
                .children()
                .find(|c| {
                    is(*c, NS_DGM, "cxn")
                        && is_par_of(*c)
                        && c.attribute("destId") == Some(me.id.as_str())
                })
                .context("SmartArt item without its connection")?;
            edits.push(set_attrs(
                data,
                c,
                &[("srcId", node.clone()), ("srcOrd", "0".to_owned())],
            ));
            (t.parent_id(k).to_owned(), me.ord)
        }
    };
    edits.push((
        close(cxn_lst)..close(cxn_lst),
        format!(
            "<{dgm}cxn modelId=\"{cxn}\" srcId=\"{src}\" destId=\"{node}\" srcOrd=\"{ord}\" destOrd=\"0\" parTransId=\"{par}\" sibTransId=\"{sib}\"/>"
        ),
    ));
    let data2 = apply(data, edits);
    // The drawing: copies of the selected item's shapes.
    let (list, _) = tshapes(drawing, &t)?;
    let mine: Vec<&TShape> = list.iter().filter(|s| s.role == TRole::Node(k)).collect();
    let last = mine.last().context("This SmartArt has no shape to copy")?;
    let copies: String = mine
        .iter()
        .map(|s| copy_shape(&drawing[s.range.clone()], &node))
        .collect();
    let at = last.range.end;
    let drawing2 = apply(drawing, vec![(at..at, copies)]);
    let drawing3 = relayout(&drawing2, &data2, w, h)?;
    Ok((drawing3, data2, node))
}

/// Remove the item drawn by `selected`: its children move up to its
/// parent, in its place.
pub(super) fn remove_item(
    drawing: &str,
    data: &str,
    selected: &str,
    w: Emu,
    h: Emu,
) -> anyhow::Result<(String, String)> {
    let t = tree(data)?;
    let k = t
        .node_of(selected)
        .context("This SmartArt shape can't be deleted: it doesn't show an item")?;
    if t.nodes.len() <= 1 {
        bail!("A SmartArt keeps at least one item");
    }
    let me = &t.nodes[k];
    // A Labeled Hierarchy's top with several items under it: they'd move
    // up beside it and become labels.
    if t.family == Family::LabeledHierarchy && t.roots.contains(&k) && me.kids.len() != 1 {
        bail!(TOP_KEEP);
    }
    let parent = t.parent_id(k).to_owned();
    let doc = Document::parse(data).context("SmartArt data")?;
    let mut gone: Vec<String> = vec![me.id.clone()];
    gone.extend(me.par.iter().cloned());
    gone.extend(me.sib.iter().cloned());
    gone.extend(me.cxn.iter().cloned());
    let pres: Vec<String> = t
        .pres
        .iter()
        .filter(|(_, a)| gone.contains(a))
        .map(|(p, _)| p.clone())
        .collect();
    gone.extend(pres);
    let is_gone = |s: Option<&str>| s.is_some_and(|s| gone.iter().any(|g| g == s));
    let shift = me.kids.len() as i64 - 1;
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    for p in doc.descendants().filter(|n| is(*n, NS_DGM, "pt")) {
        if is_gone(p.attribute("modelId")) {
            edits.push((p.range(), String::new()));
        }
    }
    for c in doc.descendants().filter(|n| is(*n, NS_DGM, "cxn")) {
        let src = c.attribute("srcId");
        if is_par_of(c) && src == Some(me.id.as_str()) && !is_gone(c.attribute("destId")) {
            // A child moves up, in order, into the removed item's place.
            let o = attr_i64(c, "srcOrd").unwrap_or(0);
            let pos = me
                .kids
                .iter()
                .position(|x| Some(t.nodes[*x].id.as_str()) == c.attribute("destId"))
                .map_or(o, |p| p as i64);
            edits.push(set_attrs(
                data,
                c,
                &[
                    ("srcId", parent.clone()),
                    ("srcOrd", (me.ord + pos).to_string()),
                ],
            ));
        } else if is_gone(c.attribute("modelId")) || is_gone(src) || is_gone(c.attribute("destId"))
        {
            edits.push((c.range(), String::new()));
        } else if is_par_of(c)
            && src == Some(parent.as_str())
            && let Some(o) = attr_i64(c, "srcOrd")
            && o > me.ord
            && shift != 0
        {
            edits.push(set_attrs(data, c, &[("srcOrd", (o + shift).to_string())]));
        }
    }
    let data2 = apply(data, edits);
    let (list, _) = tshapes(drawing, &t)?;
    let dedits = list
        .iter()
        .filter(|s| s.role == TRole::Node(k) || s.role == TRole::Conn(k) || gone.contains(&s.model))
        .map(|s| (s.range.clone(), String::new()))
        .collect();
    let drawing2 = apply(drawing, dedits);
    Ok((relayout(&drawing2, &data2, w, h)?, data2))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DGM: &str = "http://schemas.openxmlformats.org/drawingml/2006/diagram";

    /// A data model: `edges` (parent index or None for the document, child
    /// index); item i is "Ni".
    fn data(lo: &str, edges: &[(Option<usize>, usize)]) -> String {
        let mut pts = format!(
            r#"<dgm:pt modelId="D" type="doc"><dgm:prSet loTypeId="urn:microsoft.com/office/officeart/2005/8/layout/{lo}"/></dgm:pt>"#
        );
        let mut cxns = String::new();
        let mut ords: HashMap<Option<usize>, usize> = HashMap::new();
        for (p, c) in edges {
            pts += &format!(
                r#"<dgm:pt modelId="N{c}"><dgm:prSet/><dgm:t><a:bodyPr/><a:p><a:r><a:t>Item {c}</a:t></a:r></a:p></dgm:t></dgm:pt><dgm:pt modelId="P{c}" type="parTrans" cxnId="X{c}"/><dgm:pt modelId="S{c}" type="sibTrans" cxnId="X{c}"/>"#
            );
            let o = ords.entry(*p).or_default();
            let src = p.map_or("D".to_owned(), |p| format!("N{p}"));
            cxns += &format!(
                r#"<dgm:cxn modelId="X{c}" srcId="{src}" destId="N{c}" srcOrd="{o}" destOrd="0" parTransId="P{c}" sibTransId="S{c}"/>"#
            );
            *o += 1;
        }
        format!(
            r#"<dgm:dataModel xmlns:dgm="{DGM}" xmlns:a="{NS_A}"><dgm:ptLst>{pts}</dgm:ptLst><dgm:cxnLst>{cxns}</dgm:cxnLst></dgm:dataModel>"#
        )
    }

    fn box_sp(model: &str) -> String {
        format!(
            r#"<dsp:sp modelId="{model}"><dsp:nvSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvSpPr/></dsp:nvSpPr><dsp:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="100" cy="60"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></dsp:spPr><dsp:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>x</a:t></a:r></a:p></dsp:txBody><dsp:txXfrm><a:off x="0" y="0"/><a:ext cx="100" cy="60"/></dsp:txXfrm></dsp:sp>"#
        )
    }

    fn drawing(models: &[String]) -> String {
        let sps: String = models.iter().map(|m| box_sp(m)).collect();
        format!(
            r#"<dsp:drawing xmlns:dsp="{NS_DSP}" xmlns:a="{NS_A}"><dsp:spTree><dsp:nvGrpSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvGrpSpPr/></dsp:nvGrpSpPr><dsp:grpSpPr/>{sps}</dsp:spTree></dsp:drawing>"#
        )
    }

    /// Item boxes (by model id) and connector paths of a drawing.
    fn read(d: &str) -> (HashMap<String, Xfrm>, Vec<(String, Xfrm)>) {
        let doc = Document::parse(d).unwrap();
        let mut boxes = HashMap::new();
        let mut conns = Vec::new();
        for sp in doc.descendants().filter(|n| is(*n, NS_DSP, "sp")) {
            let m = sp.attribute("modelId").unwrap().to_owned();
            let x = xfrm_of(sp.descendants().find(|n| is(*n, NS_A, "xfrm")).unwrap()).unwrap();
            if sp.descendants().any(|n| is(n, NS_A, "custGeom")) {
                conns.push((m, x));
            } else {
                boxes.insert(m, x);
            }
        }
        (boxes, conns)
    }

    // Boss over A and B; A over A1 and A2.
    const ORG: &[(Option<usize>, usize)] = &[
        (None, 0),
        (Some(0), 1),
        (Some(0), 2),
        (Some(1), 3),
        (Some(1), 4),
    ];

    #[test]
    fn org_charts_centre_parents_over_children_with_elbow_connectors() {
        let d = data("orgChart1", ORG);
        let models: Vec<String> = (0..5).map(|i| format!("N{i}")).collect();
        let out = relayout(&drawing(&models), &d, 10_000, 6_000).unwrap();
        let (b, c) = read(&out);
        let mid = |x: &Xfrm| x.x + x.cx / 2;
        // Levels top to bottom; the boss over A and B, A over its two.
        assert!(b["N0"].y < b["N1"].y && b["N1"].y < b["N3"].y);
        assert_eq!(b["N1"].y, b["N2"].y);
        assert!((mid(&b["N1"]) - (mid(&b["N3"]) + mid(&b["N4"])) / 2).abs() <= 1);
        assert!((mid(&b["N0"]) - (mid(&b["N1"]) + mid(&b["N2"])) / 2).abs() <= 1);
        // Inside the frame, 10:6.
        for x in b.values() {
            assert!(
                x.x >= 0 && x.y >= 0 && x.x + x.cx <= 10_000 && x.y + x.cy <= 6_000,
                "{x:?}"
            );
            assert!((x.cy as f64 / x.cx as f64 - 0.6).abs() < 0.01);
        }
        // A connector per item with a parent, from the parent's bottom to
        // the child's top.
        assert_eq!(c.len(), 4);
        let a1 = c.iter().find(|(m, _)| m == "P3").unwrap().1;
        assert_eq!(a1.y, b["N1"].y + b["N1"].cy);
        assert_eq!(a1.y + a1.cy, b["N3"].y);
    }

    #[test]
    fn horizontal_hierarchies_run_left_to_right() {
        let d = data("hierarchy2", ORG);
        let models: Vec<String> = (0..5).map(|i| format!("N{i}")).collect();
        let out = relayout(&drawing(&models), &d, 10_000, 6_000).unwrap();
        let (b, c) = read(&out);
        assert!(b["N0"].x < b["N1"].x && b["N1"].x < b["N3"].x);
        assert_eq!(b["N3"].x, b["N4"].x);
        assert!(b["N3"].y < b["N4"].y);
        let a1 = c.iter().find(|(m, _)| m == "P3").unwrap().1;
        assert_eq!(a1.x, b["N1"].x + b["N1"].cx);
    }

    #[test]
    fn add_shape_after_before_above_and_below() {
        let d = data("orgChart1", ORG);
        let models: Vec<String> = (0..5).map(|i| format!("N{i}")).collect();
        let dr = relayout(&drawing(&models), &d, 10_000, 6_000).unwrap();
        // Below B: B gets a child.
        let (dr2, d2, id) = add_item(&dr, &d, "N2", AddPlace::Below, 10_000, 6_000).unwrap();
        let t = tree(&d2).unwrap();
        let k = t.nodes.iter().position(|n| n.id == id).unwrap();
        assert_eq!(
            t.nodes[k].parent.map(|p| t.nodes[p].id.as_str()),
            Some("N2")
        );
        let (b, c) = read(&dr2);
        assert!(b[&id].y > b["N2"].y);
        assert_eq!(c.len(), 5);
        // Before A: a sibling first under the boss.
        let (_, d3, id) = add_item(&dr, &d, "N1", AddPlace::Before, 10_000, 6_000).unwrap();
        let t = tree(&d3).unwrap();
        let kids: Vec<&str> = t.nodes[0]
            .kids
            .iter()
            .map(|k| t.nodes[*k].id.as_str())
            .collect();
        assert_eq!(kids, [id.as_str(), "N1", "N2"]);
        // After A: between A and B.
        let (_, d4, id) = add_item(&dr, &d, "N1", AddPlace::After, 10_000, 6_000).unwrap();
        let t = tree(&d4).unwrap();
        let kids: Vec<&str> = t.nodes[0]
            .kids
            .iter()
            .map(|k| t.nodes[*k].id.as_str())
            .collect();
        assert_eq!(kids, ["N1", id.as_str(), "N2"]);
        // Above the boss: a new top item, the boss under it, with a
        // connector now.
        let (dr5, d5, id) = add_item(&dr, &d, "N0", AddPlace::Above, 10_000, 6_000).unwrap();
        let t = tree(&d5).unwrap();
        assert_eq!(t.roots.len(), 1);
        assert_eq!(t.nodes[t.roots[0]].id, id);
        assert_eq!(t.nodes[t.nodes[t.roots[0]].kids[0]].id, "N0");
        let (b, c) = read(&dr5);
        assert!(b[&id].y < b["N0"].y);
        assert_eq!(c.len(), 5, "{c:?}");
    }

    #[test]
    fn deleting_an_item_moves_its_children_up() {
        let d = data("hierarchy1", ORG);
        let models: Vec<String> = (0..5).map(|i| format!("N{i}")).collect();
        let dr = relayout(&drawing(&models), &d, 10_000, 6_000).unwrap();
        let (dr2, d2) = remove_item(&dr, &d, "N1", 10_000, 6_000).unwrap();
        let t = tree(&d2).unwrap();
        let kids: Vec<&str> = t.nodes[0]
            .kids
            .iter()
            .map(|k| t.nodes[*k].id.as_str())
            .collect();
        assert_eq!(kids, ["N3", "N4", "N2"]);
        assert!(!d2.contains("\"N1\"") && !d2.contains("P1") && !d2.contains("X1\""));
        let (b, c) = read(&dr2);
        assert!(!b.contains_key("N1"));
        assert_eq!(c.len(), 3);
        assert_eq!(b["N3"].y, b["N2"].y);
        // The last item stays.
        let one = data("hierarchy1", &[(None, 0)]);
        assert!(remove_item(&drawing(&["N0".into()]), &one, "N0", 100, 100).is_err());
    }
}
