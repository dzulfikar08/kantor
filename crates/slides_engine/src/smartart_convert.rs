//! Changing a SmartArt's layout (PowerPoint's and Word's *Layouts*
//! gallery): the same data model drawn in another layout Kantor lays out
//! ([`super::LAYOUTS`]). A child of [`super`] (the list layouts).
//!
//! As in Office, the data model keeps its items, their text and their
//! connections; the document item takes the new layout's id and category
//! (`loTypeId`, `loCatId`), and the old layout's presentation items go
//! (Office makes new ones when it lays the diagram out). The drawing is
//! made again: one shape per item drawn, each a copy of the shape that
//! showed the item before (so it keeps its fill, outline and text
//! formatting) with the new layout's geometry, and the shapes the layout
//! adds (arrows, a Step Up Process's steps, a Continuous Block Process's
//! arrow, sub-item boxes) in the layout's own style. The layout part gets
//! a layout definition for the new layout; the style and colours stay, as
//! Office keeps them.

use super::*;

/// A placeholder box: shapes are made with it and laid out after.
const PLACE: &str = "<a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"1000\" cy=\"1000\"/></a:xfrm>";

/// Lay SmartArt (drawing and data model XML) out in layout `to`, in a
/// `w` × `h` frame: the new drawing, data model and layout definition.
pub fn change_layout(
    drawing: &str,
    data: &str,
    to: Family,
    w: Emu,
    h: Emu,
) -> anyhow::Result<(String, String, String)> {
    let model = DataModel::read(data)?;
    let old = OldDrawing::read(drawing)?;
    let mut new_pres = String::new();
    let mut shapes = Shapes::default();
    let tmpl = |id: &str| old.template(id, &model);
    let lvl0 = |id: &str| model.paras(id, &tmpl(id), 0);
    let prst = geometry(to);
    if to.is_tree() {
        let centre = model.top.first().cloned();
        for (k, (id, parent)) in model.walk().into_iter().enumerate() {
            let id = id.as_str();
            let label = to == Family::LabeledHierarchy && parent.is_none() && k > 0;
            if to == Family::RadialCycle && centre.as_deref() != Some(id) && parent != centre {
                continue;
            }
            let t = tmpl(id);
            let paras = lvl0(id);
            if label {
                shapes.back.push_str(&scratch(
                    id,
                    "roundRect",
                    "<a:solidFill><a:schemeClr val=\"accent1\"><a:tint val=\"40000\"/><a:alpha val=\"90000\"/></a:schemeClr></a:solidFill><a:ln><a:noFill/></a:ln>",
                    "l",
                    &paras,
                    "<a:off x=\"0\" y=\"0\"/><a:ext cx=\"200\" cy=\"1000\"/>",
                ));
                continue;
            }
            shapes.items.push_str(&node(&t, id, prst, &paras, None));
        }
    } else {
        for (i, id) in model.top.iter().enumerate() {
            let id = id.as_str();
            let t = tmpl(id);
            let subs = model.subs(id);
            let mut paras = lvl0(id);
            if !to.sub_boxes() {
                for s in &subs {
                    // Bullets in shapes narrower at their edges, centred.
                    let lvl = if matches!(to, Family::Pyramid | Family::Venn | Family::Cycle) {
                        2
                    } else {
                        1
                    };
                    paras.push_str(&model.paras(s, &t, lvl));
                }
            }
            match to {
                Family::StepUp => {
                    shapes
                        .items
                        .push_str(&scratch(id, "rect", NO_FILL, "t", &paras, ""));
                    if let Some(par) = model.par.get(id) {
                        shapes
                            .back
                            .push_str(&scratch(par, "corner", ACCENT, "", "", ""));
                    }
                }
                Family::Venn => {
                    let fill = t.as_ref().and_then(|t| translucent(t));
                    shapes
                        .items
                        .push_str(&node(&t, id, prst, &paras, fill.as_deref()));
                }
                _ => shapes.items.push_str(&node(&t, id, prst, &paras, None)),
            }
            if to.sub_boxes() && !subs.is_empty() {
                let pid = new_id(data, 100 + i as u32);
                let sub_paras: String = subs.iter().map(|s| model.paras(s, &t, 1)).collect();
                shapes.items.push_str(&scratch(
                    &pid,
                    "rect",
                    "<a:solidFill><a:schemeClr val=\"accent1\"><a:alpha val=\"90000\"/><a:tint val=\"40000\"/></a:schemeClr></a:solidFill><a:ln><a:noFill/></a:ln>",
                    "t",
                    &sub_paras,
                    "",
                ));
                new_pres.push_str(&format!(
                    "<{d}pt modelId=\"{pid}\" type=\"pres\"><{d}prSet presAssocID=\"{id}\" presName=\"childText\" presStyleLbl=\"revTx\" presStyleIdx=\"{i}\" presStyleCnt=\"{n}\"/><{d}spPr/></{d}pt>",
                    d = model.dgm,
                    n = model.top.len()
                ));
            }
        }
        if to.arrows()
            && model.top.len() > 1
            && let Some(sib) = model.sib.get(&model.top[0])
        {
            shapes.arrows.push_str(&scratch(
                sib,
                "rightArrow",
                "<a:solidFill><a:schemeClr val=\"accent1\"><a:tint val=\"60000\"/></a:schemeClr></a:solidFill><a:ln><a:noFill/></a:ln>",
                "",
                "",
                "",
            ));
        }
        if to == Family::ContinuousBlock {
            let arrow = scratch(
                &model.doc_id,
                "rightArrow",
                "<a:solidFill><a:schemeClr val=\"accent1\"><a:tint val=\"40000\"/></a:schemeClr></a:solidFill><a:ln><a:noFill/></a:ln>",
                "",
                "",
                "",
            );
            shapes.back.insert_str(0, &arrow);
        }
    }
    let data2 = model.retyped(data, to, &new_pres)?;
    let drawing2 = format!(
        "{}{}<dsp:spTree><dsp:nvGrpSpPr><dsp:cNvPr id=\"0\" name=\"\"/><dsp:cNvGrpSpPr/></dsp:nvGrpSpPr><dsp:grpSpPr/>{}{}{}</dsp:spTree></dsp:drawing>",
        XML_DECL, old.root, shapes.back, shapes.items, shapes.arrows
    );
    let drawing3 = relayout(&drawing2, &data2, w, h)?;
    Ok((drawing3, data2, layout_def(to)))
}

const NO_FILL: &str = "<a:noFill/><a:ln><a:noFill/></a:ln>";
const ACCENT: &str =
    "<a:solidFill><a:schemeClr val=\"accent1\"/></a:solidFill><a:ln><a:noFill/></a:ln>";

#[derive(Default)]
struct Shapes {
    /// Drawn first: a Continuous Block Process's arrow, steps, bands.
    back: String,
    items: String,
    arrows: String,
}

/// Each layout's shape for its items.
fn geometry(f: Family) -> &'static str {
    match f {
        Family::Blocks | Family::HorizontalBullets | Family::OrgChart => "rect",
        Family::Cycle | Family::RadialCycle | Family::Venn => "ellipse",
        Family::Pyramid => "trapezoid",
        Family::StepUp => "rect",
        Family::Bullets
        | Family::Process
        | Family::ContinuousBlock
        | Family::Hierarchy
        | Family::HorizontalHierarchy
        | Family::LabeledHierarchy
        | Family::Matrix => "roundRect",
    }
}

/// What changing the layout needs from the data model.
struct DataModel<'a> {
    doc: Document<'a>,
    src: &'a str,
    doc_id: String,
    /// The data model's prefixes (with their colons).
    dgm: String,
    a: String,
    /// Top-level items in order, and each item's children.
    top: Vec<String>,
    kids: HashMap<String, Vec<String>>,
    sib: HashMap<String, String>,
    par: HashMap<String, String>,
    /// Presentation item → the data item it shows.
    pres: HashMap<String, String>,
}

impl<'a> DataModel<'a> {
    fn read(src: &'a str) -> anyhow::Result<Self> {
        let doc = Document::parse(src).context("SmartArt data")?;
        let root = doc.root_element();
        let prefix = |ns: &str| {
            root.lookup_prefix(ns)
                .filter(|p| !p.is_empty())
                .map(|p| format!("{p}:"))
                .unwrap_or_default()
        };
        let dgm = prefix(NS_DGM);
        let a = prefix(NS_A);
        let mut doc_id = String::new();
        let mut items: Vec<String> = Vec::new();
        let mut pres = HashMap::new();
        for p in doc.descendants().filter(|n| is(*n, NS_DGM, "pt")) {
            let Some(id) = p.attribute("modelId") else {
                continue;
            };
            match p.attribute("type") {
                Some("doc") => doc_id = id.to_owned(),
                None | Some("node" | "asst") => items.push(id.to_owned()),
                Some("pres") => {
                    if let Some(a) =
                        child(p, NS_DGM, "prSet").and_then(|s| s.attribute("presAssocID"))
                    {
                        pres.insert(id.to_owned(), a.to_owned());
                    }
                }
                _ => {}
            }
        }
        if doc_id.is_empty() {
            bail!("SmartArt data without a document item");
        }
        let mut edges: HashMap<String, Vec<(i64, String)>> = HashMap::new();
        let mut sib = HashMap::new();
        let mut par = HashMap::new();
        for c in doc.descendants().filter(|n| is(*n, NS_DGM, "cxn")) {
            if c.attribute("type").is_some_and(|t| t != "parOf") {
                continue;
            }
            let (Some(s), Some(d)) = (c.attribute("srcId"), c.attribute("destId")) else {
                continue;
            };
            if !items.iter().any(|i| i == d) {
                continue;
            }
            edges
                .entry(s.to_owned())
                .or_default()
                .push((attr_i64(c, "srcOrd").unwrap_or(0), d.to_owned()));
            if let Some(x) = c.attribute("sibTransId") {
                sib.insert(d.to_owned(), x.to_owned());
            }
            if let Some(x) = c.attribute("parTransId") {
                par.insert(d.to_owned(), x.to_owned());
            }
        }
        let kids: HashMap<String, Vec<String>> = edges
            .into_iter()
            .map(|(k, mut v)| {
                v.sort_by_key(|e| e.0);
                (k, v.into_iter().map(|e| e.1).collect())
            })
            .collect();
        let top = kids.get(&doc_id).cloned().unwrap_or_default();
        if top.is_empty() {
            bail!("This SmartArt has no items");
        }
        Ok(Self {
            doc,
            src,
            doc_id,
            dgm,
            a,
            top,
            kids,
            sib,
            par,
            pres,
        })
    }

    /// Every item depth first, with its parent.
    fn walk(&self) -> Vec<(String, Option<String>)> {
        fn go(
            m: &DataModel,
            id: &str,
            parent: Option<&str>,
            out: &mut Vec<(String, Option<String>)>,
        ) {
            if out.len() > 5_000 || out.iter().any(|o| o.0 == id) {
                return;
            }
            out.push((id.to_owned(), parent.map(str::to_owned)));
            for k in m.kids.get(id).into_iter().flatten() {
                go(m, k, Some(id), out);
            }
        }
        let mut out = Vec::new();
        for t in &self.top {
            go(self, t, None, &mut out);
        }
        out
    }

    /// Item `id`'s descendants, depth first.
    fn subs(&self, id: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut stack: Vec<&String> = self.kids.get(id).into_iter().flatten().rev().collect();
        while let Some(k) = stack.pop() {
            if out.len() > 5_000 || out.contains(k) {
                continue;
            }
            out.push(k.clone());
            stack.extend(self.kids.get(k).into_iter().flatten().rev());
        }
        out
    }

    fn point(&self, id: &str) -> Option<Node<'_, 'a>> {
        self.doc
            .descendants()
            .find(|n| is(*n, NS_DGM, "pt") && n.attribute("modelId") == Some(id))
    }

    /// Item `id`'s paragraphs for a drawing shape at level `lvl` (0: the
    /// item's own shape; 1: bullets; 2: centred bullets), formatted as `tmpl` (the shape that
    /// showed it, if any) where the data model's runs have no formatting
    /// of their own. The template's own paragraphs when they show the
    /// same text (they keep all of their formatting).
    fn paras(&self, id: &str, tmpl: &Option<String>, lvl: u8) -> String {
        let t = tmpl.as_deref().map(Template::of).unwrap_or_default();
        let pt = self.point(id);
        let body = pt.and_then(|p| child(p, NS_DGM, "t"));
        let ps: Vec<Node> = body
            .map(|b| children(b, NS_A, "p").collect())
            .unwrap_or_default();
        let text = |p: &Node| -> String {
            p.descendants()
                .filter(|n| is(*n, NS_A, "t"))
                .filter_map(|n| n.text())
                .collect()
        };
        let mine: Vec<String> = ps.iter().map(text).collect();
        if lvl == 0 && !t.paras.is_empty() && t.texts == mine {
            return centred(&t.paras);
        }
        let ppr = if lvl == 0 {
            centred_ppr(&t.ppr)
        } else {
            let algn = if lvl == 2 { "ctr" } else { "l" };
            format!(
                "<a:pPr marL=\"114300\" lvl=\"1\" indent=\"-114300\" algn=\"{algn}\"><a:buChar char=\"•\"/></a:pPr>"
            )
        };
        let own_rpr = |r: Node| -> Option<String> {
            let rpr = child(r, NS_A, "rPr")?;
            let formatted = rpr
                .attributes()
                .any(|a| !matches!(a.name(), "lang" | "dirty" | "altLang" | "err"))
                || rpr.has_children();
            (formatted && self.a == "a:").then(|| self.src[rpr.range()].to_owned())
        };
        let mut out = String::new();
        if ps.is_empty() {
            out.push_str(&format!("<a:p>{ppr}{}</a:p>", t.end_rpr()));
        }
        for p in &ps {
            out.push_str("<a:p>");
            out.push_str(&ppr);
            for r in p
                .children()
                .filter(|n| is(*n, NS_A, "r") || is(*n, NS_A, "fld"))
            {
                let s: String = r
                    .descendants()
                    .filter(|n| is(*n, NS_A, "t"))
                    .filter_map(|n| n.text())
                    .collect();
                let rpr = own_rpr(r).unwrap_or_else(|| t.rpr.clone());
                out.push_str(&format!("<a:r>{rpr}<a:t>{}</a:t></a:r>", esc(&s)));
            }
            out.push_str(&t.end_rpr());
            out.push_str("</a:p>");
        }
        out
    }

    /// The data model for layout `to`: the document item's layout id and
    /// category set, the old layout's presentation items and their
    /// connections gone, `extra` presentation items added.
    fn retyped(&self, src: &str, to: Family, extra: &str) -> anyhow::Result<String> {
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        let d = &self.dgm;
        let lo = [
            ("loTypeId", to.layout_id().to_owned()),
            ("loCatId", to.category().to_owned()),
        ];
        let root = self
            .doc
            .descendants()
            .find(|n| is(*n, NS_DGM, "pt") && n.attribute("type") == Some("doc"))
            .context("SmartArt data without a document item")?;
        match child(root, NS_DGM, "prSet") {
            Some(ps) => edits.push(set_attrs(src, ps, &lo)),
            None => {
                let tag = start_tag(src, root);
                let st = root.range().start;
                let ps = format!(
                    "<{d}prSet loTypeId=\"{}\" loCatId=\"{}\"/>",
                    lo[0].1, lo[1].1
                );
                if tag.ends_with("/>") {
                    let open = format!("{}>", tag.trim_end_matches("/>"));
                    edits.push((root.range(), format!("{open}{ps}</{}>", tag_name(tag))));
                } else {
                    let at = st + tag.len();
                    edits.push((at..at, ps));
                }
            }
        }
        for p in self.doc.descendants().filter(|n| is(*n, NS_DGM, "pt")) {
            if p.attribute("type") == Some("pres") {
                edits.push((p.range(), String::new()));
            }
        }
        for c in self.doc.descendants().filter(|n| is(*n, NS_DGM, "cxn")) {
            if matches!(c.attribute("type"), Some("presOf" | "presParOf")) {
                edits.push((c.range(), String::new()));
            }
        }
        if !extra.is_empty() {
            let lst = self
                .doc
                .descendants()
                .find(|n| is(*n, NS_DGM, "ptLst"))
                .context("SmartArt data without items")?;
            let at = lst.range().end - format!("</{}>", tag_name(start_tag(src, lst))).len();
            edits.push((at..at, extra.to_owned()));
        }
        Ok(apply(src, edits))
    }
}

/// Paragraph properties `ppr` (XML, may be empty) centred unless they
/// say how to align: SmartArt text is centred in its shapes unless
/// formatted otherwise.
fn centred_ppr(ppr: &str) -> String {
    if ppr.is_empty() {
        return "<a:pPr algn=\"ctr\"/>".to_owned();
    }
    let end = tag_end(ppr).unwrap_or(ppr.len());
    let tag = &ppr[..end];
    if tag.contains(" algn=") || tag.contains(" lvl=") {
        return ppr.to_owned();
    }
    let at = if tag.ends_with("/>") {
        end - 2
    } else {
        end - 1
    };
    format!("{} algn=\"ctr\"{}", &ppr[..at], &ppr[at..])
}

/// Paragraphs `paras` (XML, `a:` prefix) with [`centred_ppr`] applied.
fn centred(paras: &str) -> String {
    let mut out = String::new();
    let mut rest = paras;
    while let Some(i) = rest.find("<a:p>") {
        out.push_str(&rest[..i + 5]);
        rest = &rest[i + 5..];
        if rest.starts_with("<a:pPr") {
            let end = rest
                .find("</a:pPr>")
                .map(|j| j + 8)
                .filter(|j| !tag_end(rest).is_some_and(|t| rest[..t].ends_with("/>") && t < *j))
                .or_else(|| tag_end(rest))
                .unwrap_or(rest.len());
            out.push_str(&centred_ppr(&rest[..end]));
            rest = &rest[end..];
        } else {
            out.push_str(&centred_ppr(""));
        }
    }
    out.push_str(rest);
    out
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

/// The old drawing: its root's start tag (namespaces) and its shapes.
struct OldDrawing {
    root: String,
    /// Each shape's XML and the data item it shows.
    /// Each shape's XML, the item it shows and how good a template it is
    /// (filled: 2, unfilled with text: 1).
    shapes: Vec<(String, String, u8)>,
}

impl OldDrawing {
    fn read(src: &str) -> anyhow::Result<Self> {
        let doc = Document::parse(src).context("SmartArt drawing")?;
        let root = doc.root_element();
        let ok = root.lookup_namespace_uri(Some("a")) == Some(NS_A)
            && root.lookup_namespace_uri(Some("dsp")) == Some(NS_DSP);
        let root_tag = if ok {
            start_tag(src, root).trim_end_matches("/>").to_owned()
        } else {
            format!("<dsp:drawing {DRAWING_NS}>")
        };
        let root_tag = if root_tag.ends_with('>') {
            root_tag
        } else {
            format!("{root_tag}>")
        };
        let mut shapes = Vec::new();
        if ok && let Some(tree) = root.children().find(|n| is(*n, NS_DSP, "spTree")) {
            for n in tree.children().filter(|n| is(*n, NS_DSP, "sp")) {
                let model = n.attribute("modelId").unwrap_or_default().to_owned();
                let has_text = n
                    .descendants()
                    .any(|t| is(t, NS_A, "t") && t.text().is_some_and(|s| !s.trim().is_empty()));
                // Shapes drawing lines (connectors) aren't templates.
                let line = n.descendants().any(|g| is(g, NS_A, "custGeom"));
                let filled = child(n, NS_DSP, "spPr").is_some_and(|sp| {
                    ["solidFill", "gradFill", "pattFill", "blipFill"]
                        .iter()
                        .any(|f| child(sp, NS_A, f).is_some())
                });
                if !line {
                    shapes.push((
                        src[n.range()].to_owned(),
                        model,
                        // An item drawn as a background and a text box
                        // (Hierarchy): the background, drawn first.
                        2 * u8::from(filled) + u8::from(has_text && !filled),
                    ));
                }
            }
        }
        Ok(Self {
            root: root_tag,
            shapes,
        })
    }

    /// The shape that showed item `id` (a filled one, with text, if
    /// there's one), else the best of the others.
    fn template(&self, id: &str, m: &DataModel) -> Option<String> {
        let target = |model: &str| m.pres.get(model).map_or(model, String::as_str).to_owned();
        // The first of the best (max_by_key keeps the last).
        fn best<'s>(
            it: impl Iterator<Item = &'s (String, String, u8)>,
        ) -> Option<&'s (String, String, u8)> {
            it.fold(None, |b: Option<&(String, String, u8)>, s| match b {
                Some(b) if b.2 >= s.2 => Some(b),
                _ => Some(s),
            })
        }
        best(self.shapes.iter().filter(|s| target(&s.1) == id))
            .or_else(|| best(self.shapes.iter()))
            .map(|s| s.0.clone())
    }
}

/// What a new shape takes from a template's text.
#[derive(Default)]
struct Template {
    /// Its paragraphs (XML) and their text.
    paras: String,
    texts: Vec<String>,
    /// Its first paragraph's properties and first run's (XML).
    ppr: String,
    rpr: String,
}

impl Template {
    fn of(xml: &str) -> Self {
        let full = wrap_ns(xml);
        let Ok(doc) = Document::parse(&full) else {
            return Self::default();
        };
        let Some(body) = doc.descendants().find(|n| is(*n, NS_DSP, "txBody")) else {
            return Self::default();
        };
        let ps: Vec<Node> = children(body, NS_A, "p").collect();
        let paras = match (ps.first(), ps.last()) {
            (Some(f), Some(l)) => full[f.range().start..l.range().end].to_owned(),
            _ => String::new(),
        };
        let texts = ps
            .iter()
            .map(|p| {
                p.descendants()
                    .filter(|n| is(*n, NS_A, "t"))
                    .filter_map(|n| n.text())
                    .collect()
            })
            .collect();
        let ppr = ps
            .first()
            .and_then(|p| child(*p, NS_A, "pPr"))
            .map(|p| full[p.range()].to_owned())
            .unwrap_or_default();
        let rpr = body
            .descendants()
            .find(|d| is(*d, NS_A, "rPr"))
            .map(|d| full[d.range()].to_owned())
            .unwrap_or_else(|| "<a:rPr lang=\"en-US\"/>".to_owned());
        Self {
            paras,
            texts,
            ppr,
            rpr,
        }
    }

    /// An `a:endParaRPr` with the first run's attributes.
    fn end_rpr(&self) -> String {
        let attrs = tag_attrs(&self.rpr).trim_end_matches('/').trim().to_owned();
        if attrs.is_empty() {
            "<a:endParaRPr lang=\"en-US\"/>".to_owned()
        } else {
            format!("<a:endParaRPr {attrs}/>")
        }
    }
}

/// An item's shape for `model`: a copy of `tmpl` (its fill, outline, style
/// and text body) with preset `prst`, the paragraphs `paras`, a
/// placeholder box and, if given, the fill `fill`; a shape in the layout's
/// style when there's no template.
fn node(tmpl: &Option<String>, model: &str, prst: &str, paras: &str, fill: Option<&str>) -> String {
    let Some(t) = tmpl else {
        return scratch(model, prst, fill.unwrap_or(ACCENT), "ctr", paras, "")
            .replace("val=\"tx1\"", "val=\"lt1\"");
    };
    let full = wrap_ns(t);
    let pre = wrap_pre();
    let Ok(doc) = Document::parse(&full) else {
        return scratch(model, prst, fill.unwrap_or(ACCENT), "ctr", paras, "");
    };
    let Some(sp) = doc.descendants().find(|n| is(*n, NS_DSP, "sp")) else {
        return scratch(model, prst, fill.unwrap_or(ACCENT), "ctr", paras, "");
    };
    let at = |r: Range<usize>| r.start - pre..r.end - pre;
    let mut edits: Vec<(Range<usize>, String)> =
        vec![set_attrs(&full, sp, &[("modelId", model.to_owned())])];
    edits[0].0 = at(edits[0].0.clone());
    let geom = format!("<a:prstGeom prst=\"{prst}\"><a:avLst/></a:prstGeom>");
    match child(sp, NS_DSP, "spPr") {
        Some(sppr) => {
            let xf = child(sppr, NS_A, "xfrm");
            let g = sppr
                .children()
                .find(|c| is(*c, NS_A, "prstGeom") || is(*c, NS_A, "custGeom"));
            let after_tag = sppr.range().start + start_tag(&full, sppr).len();
            match xf {
                Some(x) => edits.push((at(x.range()), PLACE.to_owned())),
                None => edits.push((at(after_tag..after_tag), PLACE.to_owned())),
            }
            match g {
                Some(g) => edits.push((at(g.range()), geom.clone())),
                None => {
                    let p = xf.map_or(after_tag, |x| x.range().end);
                    edits.push((at(p..p), geom.clone()));
                }
            }
            if let Some(fill) = fill {
                let fills = [
                    "solidFill",
                    "gradFill",
                    "noFill",
                    "pattFill",
                    "blipFill",
                    "grpFill",
                ];
                match sppr
                    .children()
                    .find(|c| fills.iter().any(|f| is(*c, NS_A, f)))
                {
                    Some(f) => edits.push((at(f.range()), fill.to_owned())),
                    None => {
                        let p = g.or(xf).map_or(after_tag, |x| x.range().end);
                        edits.push((at(p..p), fill.to_owned()));
                    }
                }
            }
        }
        None => {
            let nv = child(sp, NS_DSP, "nvSpPr")
                .map_or(sp.range().start + start_tag(&full, sp).len(), |n| {
                    n.range().end
                });
            edits.push((
                at(nv..nv),
                format!("<dsp:spPr>{PLACE}{geom}{}</dsp:spPr>", fill.unwrap_or("")),
            ));
        }
    }
    let body = child(sp, NS_DSP, "txBody");
    match body {
        Some(b) => {
            let ps: Vec<Node> = children(b, NS_A, "p").collect();
            match (ps.first(), ps.last()) {
                (Some(f), Some(l)) => {
                    edits.push((at(f.range().start..l.range().end), paras.to_owned()))
                }
                _ => {
                    let end = b.range().end - format!("</{}>", tag_name(start_tag(&full, b))).len();
                    edits.push((at(end..end), paras.to_owned()));
                }
            }
        }
        None => {
            let p = child(sp, NS_DSP, "style")
                .or_else(|| child(sp, NS_DSP, "spPr"))
                .map_or(sp.range().end - "</dsp:sp>".len(), |n| n.range().end);
            edits.push((
                at(p..p),
                format!("<dsp:txBody><a:bodyPr anchor=\"ctr\"/><a:lstStyle/>{paras}</dsp:txBody>"),
            ));
        }
    }
    let tx = "<dsp:txXfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"1000\" cy=\"1000\"/></dsp:txXfrm>";
    match child(sp, NS_DSP, "txXfrm") {
        Some(x) => edits.push((at(x.range()), tx.to_owned())),
        None => {
            let end = sp.range().end - format!("</{}>", tag_name(start_tag(&full, sp))).len();
            edits.push((at(end..end), tx.to_owned()));
        }
    }
    apply(t, edits)
}

/// The fill of `tmpl` (a drawing shape) half see-through (Basic Venn's
/// circles): its solid fill's colour with 50 % alpha.
fn translucent(tmpl: &str) -> Option<String> {
    let full = wrap_ns(tmpl);
    let doc = Document::parse(&full).ok()?;
    let sppr = doc.descendants().find(|n| is(*n, NS_DSP, "spPr"))?;
    let fill = child(sppr, NS_A, "solidFill")?;
    let clr = fill.first_element_child()?;
    let tag = start_tag(&full, clr);
    let name = tag_name(tag);
    let inner: String = clr
        .children()
        .filter(|c| c.is_element() && !is(*c, NS_A, "alpha"))
        .map(|c| full[c.range()].to_owned())
        .collect();
    let open = tag.trim_end_matches("/>").trim_end_matches('>');
    Some(format!(
        "<a:solidFill>{open}><a:alpha val=\"50000\"/>{inner}</{name}></a:solidFill>"
    ))
}

/// A shape in the layout's own style: preset `prst`, fill and outline
/// `paint`, text `paras` (none: no text body) anchored `anchor`, its text
/// box `tx` within its 1000 × 1000 placeholder box (empty: all of it).
fn scratch(model: &str, prst: &str, paint: &str, anchor: &str, paras: &str, tx: &str) -> String {
    let body = if paras.is_empty() {
        String::new()
    } else {
        let tx = if tx.is_empty() {
            "<a:off x=\"0\" y=\"0\"/><a:ext cx=\"1000\" cy=\"1000\"/>"
        } else {
            tx
        };
        let anchor = if anchor == "l" { "ctr" } else { anchor };
        format!(
            "<dsp:txBody><a:bodyPr spcFirstLastPara=\"0\" vert=\"horz\" wrap=\"square\" lIns=\"76200\" tIns=\"76200\" rIns=\"76200\" bIns=\"76200\" numCol=\"1\" spcCol=\"1270\" anchor=\"{anchor}\" anchorCtr=\"0\"><a:noAutofit/></a:bodyPr><a:lstStyle/>{}</dsp:txBody><dsp:txXfrm>{tx}</dsp:txXfrm>",
            dark(paras)
        )
    };
    format!(
        "<dsp:sp modelId=\"{model}\"><dsp:nvSpPr><dsp:cNvPr id=\"0\" name=\"\"/><dsp:cNvSpPr/></dsp:nvSpPr>\
         <dsp:spPr>{PLACE}<a:prstGeom prst=\"{prst}\"><a:avLst/></a:prstGeom>{paint}</dsp:spPr>\
         <dsp:style><a:lnRef idx=\"2\"><a:scrgbClr r=\"0\" g=\"0\" b=\"0\"/></a:lnRef><a:fillRef idx=\"1\"><a:scrgbClr r=\"0\" g=\"0\" b=\"0\"/></a:fillRef>\
         <a:effectRef idx=\"0\"><a:scrgbClr r=\"0\" g=\"0\" b=\"0\"/></a:effectRef><a:fontRef idx=\"minor\"><a:schemeClr val=\"tx1\"/></a:fontRef></dsp:style>{body}</dsp:sp>"
    )
}

/// Paragraphs with their runs' own colours taken out (text in the dark
/// text colour, on a light or no fill).
fn dark(paras: &str) -> String {
    let mut s = paras.to_owned();
    while let Some(i) = s.find("<a:solidFill>") {
        let Some(j) = s[i..].find("</a:solidFill>") else {
            break;
        };
        s.replace_range(i..i + j + "</a:solidFill>".len(), "");
    }
    s
}

/// A layout definition (`dgm:layoutDef`) for layout `f`: its id and
/// category, and a layout tree for its algorithm (PowerPoint and Word lay
/// the built-in layouts out by their id).
pub fn layout_def(f: Family) -> String {
    const R: &str =
        "xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"";
    let (alg, params) = match f {
        Family::Blocks | Family::Matrix => (
            "snake",
            "<dgm:param type=\"grDir\" val=\"tL\"/><dgm:param type=\"flowDir\" val=\"row\"/><dgm:param type=\"contDir\" val=\"sameDir\"/><dgm:param type=\"off\" val=\"ctr\"/>",
        ),
        Family::Bullets => ("lin", "<dgm:param type=\"linDir\" val=\"fromT\"/>"),
        Family::HorizontalBullets | Family::Process | Family::ContinuousBlock | Family::StepUp => {
            ("lin", "<dgm:param type=\"linDir\" val=\"fromL\"/>")
        }
        Family::Cycle | Family::Venn => (
            "cycle",
            "<dgm:param type=\"stAng\" val=\"0\"/><dgm:param type=\"spanAng\" val=\"360\"/>",
        ),
        Family::RadialCycle => ("cycle", "<dgm:param type=\"ctrShpMap\" val=\"fNode\"/>"),
        Family::OrgChart | Family::Hierarchy | Family::LabeledHierarchy => {
            ("hierRoot", "<dgm:param type=\"hierAlign\" val=\"tCtrCH\"/>")
        }
        Family::HorizontalHierarchy => {
            ("hierRoot", "<dgm:param type=\"hierAlign\" val=\"lCtrCV\"/>")
        }
        Family::Pyramid => ("pyra", "<dgm:param type=\"linDir\" val=\"fromT\"/>"),
    };
    let sib = if f.arrows() {
        format!(
            "<dgm:forEach name=\"sibTransForEach\" axis=\"followSib\" ptType=\"sibTrans\" cnt=\"1\"><dgm:layoutNode name=\"sibTrans\"><dgm:alg type=\"conn\"/><dgm:shape {R} type=\"rightArrow\" r:blip=\"\"><dgm:adjLst/></dgm:shape><dgm:presOf axis=\"self\"/><dgm:constrLst/><dgm:ruleLst/></dgm:layoutNode></dgm:forEach>"
        )
    } else {
        String::new()
    };
    format!(
        "{XML_DECL}<dgm:layoutDef xmlns:dgm=\"{NS_DGM}\" xmlns:a=\"{NS_A}\" uniqueId=\"{id}\"><dgm:title val=\"{name}\"/><dgm:desc val=\"\"/>\
         <dgm:catLst><dgm:cat type=\"{cat}\" pri=\"1000\"/></dgm:catLst>\
         <dgm:layoutNode name=\"diagram\"><dgm:varLst><dgm:dir/><dgm:resizeHandles val=\"exact\"/></dgm:varLst>\
         <dgm:alg type=\"{alg}\">{params}</dgm:alg><dgm:shape {R} r:blip=\"\"><dgm:adjLst/></dgm:shape><dgm:presOf/><dgm:constrLst/><dgm:ruleLst/>\
         <dgm:forEach name=\"nodesForEach\" axis=\"ch\" ptType=\"node\"><dgm:layoutNode name=\"node\"><dgm:varLst><dgm:bulletEnabled val=\"1\"/></dgm:varLst>\
         <dgm:alg type=\"tx\"/><dgm:shape {R} type=\"{prst}\" r:blip=\"\"><dgm:adjLst/></dgm:shape><dgm:presOf axis=\"desOrSelf\" ptType=\"node\"/><dgm:constrLst/><dgm:ruleLst/></dgm:layoutNode>{sib}</dgm:forEach>\
         </dgm:layoutNode></dgm:layoutDef>",
        id = f.layout_id(),
        name = f.name(),
        cat = f.category(),
        prst = geometry(f),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const DGM: &str = "http://schemas.openxmlformats.org/drawingml/2006/diagram";

    /// A Basic Block List data model: items `names` (with sub-items), a
    /// presentation item per item, and its drawing (a red box per item).
    fn blocks(names: &[(&str, &[&str])]) -> (String, String) {
        let mut pts = r#"<dgm:pt modelId="D" type="doc"><dgm:prSet loTypeId="urn:microsoft.com/office/officeart/2005/8/layout/default" loCatId="list" qsTypeId="urn:microsoft.com/office/officeart/2005/8/quickstyle/simple1"/></dgm:pt>"#.to_owned();
        let mut cxns = String::new();
        let mut sps = String::new();
        for (i, (n, subs)) in names.iter().enumerate() {
            pts += &format!(
                r#"<dgm:pt modelId="N{i}"><dgm:prSet/><dgm:t><a:bodyPr/><a:p><a:r><a:rPr lang="en-US"/><a:t>{n}</a:t></a:r></a:p></dgm:t></dgm:pt><dgm:pt modelId="P{i}" type="parTrans" cxnId="X{i}"/><dgm:pt modelId="S{i}" type="sibTrans" cxnId="X{i}"/><dgm:pt modelId="V{i}" type="pres"><dgm:prSet presAssocID="N{i}" presName="node"/></dgm:pt>"#
            );
            cxns += &format!(
                r#"<dgm:cxn modelId="X{i}" srcId="D" destId="N{i}" srcOrd="{i}" destOrd="0" parTransId="P{i}" sibTransId="S{i}"/><dgm:cxn modelId="Y{i}" type="presOf" srcId="N{i}" destId="V{i}" srcOrd="0" destOrd="0"/>"#
            );
            for (j, s) in subs.iter().enumerate() {
                pts += &format!(
                    r#"<dgm:pt modelId="N{i}s{j}"><dgm:prSet/><dgm:t><a:bodyPr/><a:p><a:r><a:t>{s}</a:t></a:r></a:p></dgm:t></dgm:pt>"#
                );
                cxns += &format!(
                    r#"<dgm:cxn modelId="X{i}s{j}" srcId="N{i}" destId="N{i}s{j}" srcOrd="{j}" destOrd="0" parTransId="P{i}s{j}" sibTransId="S{i}s{j}"/>"#
                );
            }
            sps += &format!(
                r#"<dsp:sp modelId="V{i}"><dsp:nvSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvSpPr/></dsp:nvSpPr><dsp:spPr><a:xfrm><a:off x="{x}" y="0"/><a:ext cx="100" cy="60"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></dsp:spPr><dsp:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" sz="2400" b="1"/><a:t>{n}</a:t></a:r></a:p></dsp:txBody><dsp:txXfrm><a:off x="{x}" y="0"/><a:ext cx="100" cy="60"/></dsp:txXfrm></dsp:sp>"#,
                x = i * 120
            );
        }
        (
            format!(
                r#"<dsp:drawing xmlns:dsp="{NS_DSP}" xmlns:a="{NS_A}"><dsp:spTree><dsp:nvGrpSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvGrpSpPr/></dsp:nvGrpSpPr><dsp:grpSpPr/>{sps}</dsp:spTree></dsp:drawing>"#
            ),
            format!(
                r#"<dgm:dataModel xmlns:dgm="{DGM}" xmlns:a="{NS_A}"><dgm:ptLst>{pts}</dgm:ptLst><dgm:cxnLst>{cxns}</dgm:cxnLst></dgm:dataModel>"#
            ),
        )
    }

    fn shapes(d: &str) -> Vec<(String, String, Xfrm, String)> {
        let doc = Document::parse(d).unwrap();
        doc.descendants()
            .filter(|n| is(*n, NS_DSP, "sp"))
            .map(|sp| {
                let x = xfrm_of(sp.descendants().find(|n| is(*n, NS_A, "xfrm")).unwrap()).unwrap();
                let prst = sp
                    .descendants()
                    .find(|n| is(*n, NS_A, "prstGeom"))
                    .and_then(|g| g.attribute("prst"))
                    .unwrap_or("custGeom")
                    .to_owned();
                let text: Vec<String> = sp
                    .descendants()
                    .filter(|n| is(*n, NS_A, "p"))
                    .map(|p| {
                        p.descendants()
                            .filter(|n| is(*n, NS_A, "t"))
                            .filter_map(|n| n.text())
                            .collect()
                    })
                    .collect();
                (
                    sp.attribute("modelId").unwrap().to_owned(),
                    prst,
                    x,
                    text.join("|"),
                )
            })
            .collect()
    }

    #[test]
    fn blocks_become_a_process_keeping_text_and_formatting() {
        let (dr, data) = blocks(&[("Plan", &[]), ("Build", &[]), ("Ship", &[])]);
        let (d2, data2, lo) = change_layout(&dr, &data, Family::Process, 14_500, 6_000).unwrap();
        // The data model: new layout id, no old presentation items.
        assert!(data2.contains("layout/process1") && data2.contains("loCatId=\"process\""));
        assert!(!data2.contains("type=\"pres\"") && !data2.contains("presOf"));
        assert!(data2.contains("qsTypeId"));
        assert!(
            lo.contains("uniqueId=\"urn:microsoft.com/office/officeart/2005/8/layout/process1\"")
        );
        assert_eq!(super::super::family(&d2, &data2), Some(Family::Process));
        let s = shapes(&d2);
        let items: Vec<_> = s.iter().filter(|x| x.0.starts_with('N')).collect();
        assert_eq!(items.len(), 3, "{s:?}");
        assert!(items.iter().all(|x| x.1 == "roundRect"));
        assert_eq!(
            items.iter().map(|x| x.3.as_str()).collect::<Vec<_>>(),
            ["Plan", "Build", "Ship"]
        );
        assert!(items[0].2.x < items[1].2.x && items[1].2.x < items[2].2.x);
        // The boxes keep their fill and run formatting.
        assert!(d2.matches("FF0000").count() >= 3 && d2.contains("b=\"1\""));
        // Two arrows between them.
        let arrows: Vec<_> = s.iter().filter(|x| x.1 == "rightArrow").collect();
        assert_eq!(arrows.len(), 2, "{s:?}");
    }

    #[test]
    fn every_layout_draws_the_items() {
        let (dr, data) = blocks(&[("A", &["a1", "a2"]), ("B", &[]), ("C", &["c1"]), ("D", &[])]);
        for (f, ..) in LAYOUTS {
            let (d2, data2, lo) = change_layout(&dr, &data, *f, 12_000, 8_000)
                .unwrap_or_else(|e| panic!("{f:?}: {e}"));
            assert!(Document::parse(&lo).is_ok());
            assert_eq!(super::super::family(&d2, &data2), Some(*f), "{f:?}");
            let s = shapes(&d2);
            for (_, _, x, _) in &s {
                assert!(
                    x.x >= -1 && x.y >= -1 && x.x + x.cx <= 12_001 && x.y + x.cy <= 8_001,
                    "{f:?} {x:?}"
                );
            }
            let texts: Vec<&str> = s.iter().map(|x| x.3.as_str()).collect();
            match f {
                Family::RadialCycle => {
                    // A in the centre, its sub-items round it.
                    assert!(texts.contains(&"A") && texts.contains(&"a1") && !texts.contains(&"B"));
                }
                _ if f.is_tree() => {
                    for t in ["A", "a1", "a2", "B", "C", "c1", "D"] {
                        assert!(texts.contains(&t), "{f:?} {t} {texts:?}");
                    }
                }
                _ if f.sub_boxes() => {
                    assert!(
                        texts.contains(&"A") && texts.contains(&"a1|a2"),
                        "{f:?} {texts:?}"
                    );
                }
                _ => {
                    assert!(
                        texts.contains(&"A|a1|a2") && texts.contains(&"B"),
                        "{f:?} {texts:?}"
                    );
                }
            }
            // Laid out again at another size, and items added.
            relayout(&d2, &data2, 6_000, 6_000).unwrap();
            let sel = s.iter().find(|x| x.3.starts_with('A')).unwrap().0.clone();
            let place = if matches!(*f, Family::RadialCycle | Family::LabeledHierarchy) {
                AddPlace::Below
            } else {
                AddPlace::After
            };
            let (d3, data3, id) = add_item(&d2, &data2, &sel, place, 12_000, 8_000)
                .unwrap_or_else(|e| panic!("{f:?}: {e}"));
            assert!(d3.contains(&id) && data3.contains(&id), "{f:?}");
            assert_eq!(super::super::family(&d3, &data3), Some(*f), "{f:?}");
        }
    }

    #[test]
    fn pyramid_levels_follow_the_triangle() {
        let (dr, data) = blocks(&[("Top", &[]), ("Middle", &[]), ("Base", &[])]);
        let (d2, ..) = change_layout(&dr, &data, Family::Pyramid, 13_000, 9_000).unwrap();
        let s = shapes(&d2);
        assert!(s.iter().all(|x| x.1 == "trapezoid"));
        // Wider going down, each level on the one under it, centred.
        assert!(s[0].2.cx < s[1].2.cx && s[1].2.cx < s[2].2.cx);
        assert!(s[0].2.y < s[1].2.y);
        let mid = |x: &Xfrm| x.x + x.cx / 2;
        assert!((mid(&s[0].2) - mid(&s[2].2)).abs() <= 1);
        // The top is a triangle: its adjust value is the most there is.
        assert!(d2.contains("<a:gd name=\"adj\" fmla=\"val "));
        // Text centred in the levels (the template didn't align it).
        assert_eq!(d2.matches("algn=\"ctr\"").count(), 3, "{d2}");
        assert_eq!(
            centred("<a:p><a:pPr marL=\"1\"/><a:r/></a:p>"),
            "<a:p><a:pPr marL=\"1\" algn=\"ctr\"/><a:r/></a:p>"
        );
        assert_eq!(
            centred("<a:p><a:pPr algn=\"r\"><a:buNone/></a:pPr></a:p>"),
            "<a:p><a:pPr algn=\"r\"><a:buNone/></a:pPr></a:p>"
        );
    }

    #[test]
    fn bullet_lists_get_sub_item_boxes_linked_to_their_items() {
        let (dr, data) = blocks(&[("North", &["Oslo", "Bergen"]), ("South", &[])]);
        let (d2, data2, _) =
            change_layout(&dr, &data, Family::HorizontalBullets, 10_000, 6_000).unwrap();
        // One new presentation item, for North's sub-item box.
        assert_eq!(data2.matches("type=\"pres\"").count(), 1);
        assert!(data2.contains("presAssocID=\"N0\" presName=\"childText\""));
        let s = shapes(&d2);
        let north = s.iter().find(|x| x.3 == "North").unwrap();
        let box_ = s.iter().find(|x| x.3 == "Oslo|Bergen").unwrap();
        assert!(box_.2.y >= north.2.y + north.2.cy - 1 && box_.2.x == north.2.x);
    }
}
