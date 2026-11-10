//! Shapes ↔ XML: reading `p:spTree` children, and writing them back:
//! verbatim when unchanged, spliced (only `a:xfrm` and/or the paragraphs
//! replaced) when edited, generated when new.

use std::fmt::Write as _;
use std::ops::Range;
use std::sync::Arc;

use docs_engine::{LINE_BREAK, Rgb};
use roxmltree::Node;

use crate::fill::Fill;
use crate::frame::{Frame, FrameContent, Group, NS_DSP};
use crate::model::*;
use crate::theme::{ColorCtx, ListStyle, bullet_of, is_bullet_el, para_style};
use crate::xml::*;

/// What shape parsing needs from the part being read.
pub(crate) struct ParseCtx<'a> {
    pub src: &'a str,
    pub part: &'a str,
    pub rels: &'a [Rel],
    pub colors: ColorCtx<'a>,
}

/// Shape children of an `p:spTree`, and the byte range they occupy (the
/// rest of the tree, `nvGrpSpPr`/`grpSpPr`/`extLst`, stays verbatim).
pub(crate) fn parse_tree(tree: Node, cx: &ParseCtx) -> (Vec<Shape>, Range<usize>) {
    let is_frame = |n: &Node| {
        n.is_element()
            && !(is(*n, NS_P, "nvGrpSpPr") || is(*n, NS_P, "grpSpPr") || is(*n, NS_P, "extLst"))
    };
    let shapes: Vec<Node> = tree.children().filter(is_frame).collect();
    let range = match (shapes.first(), shapes.last()) {
        (Some(a), Some(b)) => a.range().start..b.range().end,
        _ => {
            // Insert after grpSpPr (or nvGrpSpPr, or the start tag).
            let at = tree
                .children()
                .filter(|c| is(*c, NS_P, "grpSpPr") || is(*c, NS_P, "nvGrpSpPr"))
                .map(|c| c.range().end)
                .max()
                .unwrap_or_else(|| {
                    tree.range().start + tag_end(&cx.src[tree.range()]).unwrap_or(0)
                });
            at..at
        }
    };
    let shapes = shapes.into_iter().map(|n| parse_shape(n, cx)).collect();
    (shapes, range)
}

fn xfrm_of(x: Node) -> Option<Xfrm> {
    let off = child(x, NS_A, "off")?;
    let ext = child(x, NS_A, "ext")?;
    let full = 360 * ROT_PER_DEG as i64;
    Some(Xfrm {
        x: attr_i64(off, "x")?,
        y: attr_i64(off, "y")?,
        cx: attr_i64(ext, "cx")?,
        cy: attr_i64(ext, "cy")?,
        rot: attr_i64(x, "rot").unwrap_or(0).rem_euclid(full) as i32,
        flip_h: attr_bool(x, "flipH").unwrap_or(false),
        flip_v: attr_bool(x, "flipV").unwrap_or(false),
    })
}

pub(crate) fn parse_shape(n: Node, cx: &ParseCtx) -> Shape {
    parse_shape_in(n, cx, None)
}

/// A shape inside a group whose fill is `grp_fill` (for `a:grpFill`).
/// SmartArt drawings (`dsp:`) use the same elements in their own namespace.
pub(crate) fn parse_shape_in(n: Node, cx: &ParseCtx, grp_fill: Option<&Fill>) -> Shape {
    let src = cx.src;
    let base = n.range().start;
    let local = n.tag_name().name();
    let ns: &'static str = if n.tag_name().namespace() == Some(NS_DSP) {
        NS_DSP
    } else {
        NS_P
    };
    let in_p = n.tag_name().namespace() == Some(ns);
    let nv = n
        .children()
        .find(|c| c.is_element() && c.tag_name().name().starts_with("nv"));
    let cnv = nv.and_then(|nv| child(nv, ns, "cNvPr"));
    let id = cnv.and_then(|c| attr_i64(c, "id")).unwrap_or(0).max(0) as u32;
    let name = cnv
        .and_then(|c| c.attribute("name"))
        .unwrap_or_default()
        .to_owned();
    let ph = nv
        .and_then(|nv| path(nv, &[(ns, "nvPr"), (ns, "ph")]))
        .map(|p| Placeholder {
            kind: p.attribute("type").map(str::to_owned),
            idx: attr_i64(p, "idx").map(|i| i.max(0) as u32),
        });
    let max_id = n
        .descendants()
        .filter(|d| is(*d, ns, "cNvPr"))
        .filter_map(|d| attr_i64(d, "id"))
        .max()
        .unwrap_or(0)
        .max(0) as u32;

    let sp_pr = child(n, ns, "spPr").or_else(|| child(n, ns, "grpSpPr"));
    let xfrm_el = sp_pr.and_then(|s| child(s, NS_A, "xfrm"));
    let mut xfrm = xfrm_el.and_then(xfrm_of);
    let xfrm_at = sp_pr.map(|sp| match xfrm_el {
        Some(x) => {
            let r = x.range();
            XfrmAt::Replace(
                r.start - base..r.end - base,
                tag_attrs(start_tag(src, x)).to_owned(),
            )
        }
        None => {
            let tag = start_tag(src, sp);
            if tag.ends_with("/>") {
                let r = sp.range();
                XfrmAt::Expand(r.start - base..r.end - base, tag_attrs(tag).to_owned())
            } else {
                XfrmAt::Insert(sp.range().start - base + tag.len())
            }
        }
    });

    let sppr = sp_pr.map(|sp| {
        let tag = start_tag(src, sp);
        let rel = |r: Range<usize>| r.start - base..r.end - base;
        let fill = sp
            .children()
            .find(|c| is_in(*c, NS_A, &FILLS))
            .map(|f| rel(f.range()));
        let fill_at = sp
            .children()
            .filter(|c| is_in(*c, NS_A, &["xfrm", "custGeom", "prstGeom"]))
            .map(|c| c.range().end)
            .max()
            .unwrap_or(sp.range().start + tag.len())
            - base;
        let ln = child(sp, NS_A, "ln").map(|l| {
            let lt = start_tag(src, l);
            LnAt {
                range: rel(l.range()),
                attrs: tag_attrs(lt).to_owned(),
                inner_at: (!lt.ends_with("/>")).then(|| l.range().start - base + lt.len()),
                fill: l
                    .children()
                    .find(|c| is_in(*c, NS_A, &["noFill", "solidFill", "gradFill", "pattFill"]))
                    .map(|f| rel(f.range())),
            }
        });
        SpPrAt {
            range: rel(sp.range()),
            attrs: tag_attrs(tag).to_owned(),
            empty: tag.ends_with("/>"),
            fill,
            fill_at,
            ln,
        }
    });

    let style = child(n, ns, "style");
    let style_color = |name: &str| {
        style
            .and_then(|s| child(s, NS_A, name))
            .filter(|r| attr_i64(*r, "idx").unwrap_or(0) > 0 || name == "fontRef")
            .and_then(|r| cx.colors.first_color(r))
    };
    let mut look = Look {
        text_color: style_color("fontRef"),
        ..Look::default()
    };
    if let Some(sp) = sp_pr {
        let fill = sp.children().find(|c| {
            c.is_element()
                && matches!(
                    c.tag_name().name(),
                    "noFill" | "solidFill" | "gradFill" | "pattFill" | "blipFill" | "grpFill"
                )
        });
        look.fill = match fill {
            Some(f) => cx.colors.fill(f),
            None => style_color("fillRef"),
        };
        let resolve = |id: &str| {
            cx.rels
                .iter()
                .find(|r| r.id == id)
                .and_then(|r| r.part(cx.part))
        };
        let style_ref = |name: &str| {
            let r = style.and_then(|s| child(s, NS_A, name))?;
            Some((attr_i64(r, "idx").unwrap_or(0), cx.colors.first_color(r)))
        };
        look.fill_as_read = match fill {
            Some(f) if f.tag_name().name() == "grpFill" => {
                look.fill = grp_fill.and_then(Fill::main_color);
                grp_fill.cloned()
            }
            Some(f) => cx.colors.full_fill(f, &resolve),
            None => style_ref("fillRef").and_then(|(idx, ph)| cx.colors.style_fill(idx, ph)),
        };
        let ln = child(sp, NS_A, "ln");
        let themed = style_ref("lnRef")
            .filter(|(idx, _)| *idx > 0)
            .and_then(|(idx, ph)| cx.colors.style_line(idx, ph));
        let width = ln
            .and_then(crate::fill::line_width)
            .or(themed.as_ref().and_then(|t| t.0))
            .unwrap_or(12700);
        let ln_fill = ln.and_then(|l| {
            l.children().find(|c| {
                c.is_element() && matches!(c.tag_name().name(), "noFill" | "solidFill" | "gradFill")
            })
        });
        let ln_color = match ln_fill {
            Some(f) => cx.colors.fill(f),
            None => style_color("lnRef"),
        };
        look.line = ln_color.map(|c| (c, width));
        let base = themed.map(|t| t.2).unwrap_or_default();
        look.line_style = match ln {
            Some(l) => cx.colors.line_style(l, &base),
            None => base,
        };
    }

    let mut text = None;
    let mut paras_at = None;
    let kind = match (in_p, local) {
        // Connectors are drawn like shapes (lines with arrowheads).
        (true, "sp" | "cxnSp") => {
            let geometry = sp_pr.and_then(|s| {
                s.children()
                    .find(|c| is_in(*c, NS_A, &["prstGeom", "custGeom"]))
                    .and_then(Geometry::from_xml)
            });
            let geom = match &geometry {
                Some(Geometry::Preset { name, .. }) => Geom::from_prst(name),
                Some(Geometry::Custom(_)) => Geom::Other("custom".into()),
                None => Geom::Rect,
            };
            // Plain presets the display kind already says aren't stored.
            let geometry = geometry.filter(|g| *g != geom.geometry());
            let text_box = nv
                .and_then(|nv| child(nv, ns, "cNvSpPr"))
                .and_then(|c| attr_bool(c, "txBox"))
                .unwrap_or(false);
            if let Some(tb) = child(n, ns, "txBody") {
                let (body, at) = parse_text_body(tb, cx);
                text = Some(body);
                paras_at = Some(at.start - base..at.end - base);
            }
            ShapeKind::Shape {
                geom,
                text_box,
                geometry,
            }
        }
        (true, "pic") => {
            let rel = child(n, ns, "blipFill")
                .and_then(|b| child(b, NS_A, "blip"))
                .and_then(|b| r_attr(b, "embed"))
                .unwrap_or_default()
                .to_owned();
            let part = cx
                .rels
                .iter()
                .find(|r| r.id == rel)
                .and_then(|r| r.part(cx.part));
            look.picture = crate::fill::picture_look(child(n, ns, "blipFill"));
            ShapeKind::Picture { rel, part }
        }
        _ => {
            let xfrm_el = n
                .descendants()
                .find(|d| d.is_element() && d.tag_name().name() == "xfrm");
            xfrm = xfrm_el.and_then(xfrm_of);
            let frame = match (in_p, local) {
                (true, "grpSp") => {
                    // The group's own fill is what `a:grpFill` children show.
                    // (A group's own `a:grpFill` was resolved above.)
                    let own = look
                        .fill_as_read
                        .clone()
                        .or_else(|| look.fill.map(|c| Fill::Solid(c, 255)));
                    Frame::new(FrameContent::Group(parse_group(
                        n,
                        cx,
                        xfrm_el,
                        xfrm,
                        own.as_ref(),
                    )))
                }
                (true, "graphicFrame") => {
                    let text = |tb: Node| parse_text_body(tb, cx).0;
                    crate::frame::parse_graphic_frame(n, &cx.colors, cx.rels, cx.part, &text)
                        .map(Frame::new)
                        .unwrap_or_default()
                }
                _ => Frame::default(),
            };
            ShapeKind::Other {
                what: describe_element(local),
                frame,
            }
        }
    };
    let src_rec = ShapeSrc {
        raw: raw(src, n).to_owned(),
        xfrm,
        text: text.as_ref().map(|t: &TextBody| t.paras.clone()),
        xfrm_at: if matches!(kind, ShapeKind::Other { .. }) {
            None
        } else {
            xfrm_at
        },
        paras_at,
        sppr: if matches!(kind, ShapeKind::Other { .. }) {
            None
        } else {
            sppr
        },
        max_id,
    };
    Shape {
        id,
        name,
        kind,
        xfrm,
        ph,
        look,
        text,
        paint: PaintEdit::default(),
        src: Some(Arc::new(src_rec)),
    }
}

/// The children of a group (or SmartArt drawing tree) `n`.
pub(crate) fn parse_group(
    n: Node,
    cx: &ParseCtx,
    xfrm_el: Option<Node>,
    xfrm: Option<Xfrm>,
    fill: Option<&Fill>,
) -> Group {
    let mut g = Group {
        child_box: crate::frame::child_box(xfrm_el, xfrm),
        ..Group::default()
    };
    for c in n.children().filter(|c| {
        c.is_element()
            && matches!(
                c.tag_name().name(),
                "sp" | "cxnSp" | "pic" | "grpSp" | "graphicFrame"
            )
    }) {
        g.children.push(parse_shape_in(c, cx, fill));
        let tx = c
            .children()
            .find(|k| is(*k, NS_DSP, "txXfrm"))
            .and_then(xfrm_of);
        g.text_boxes.push(tx);
    }
    g
}

/// Whether `n` is an element in `ns` named one of `names`.
fn is_in(n: Node, ns: &str, names: &[&str]) -> bool {
    n.is_element() && n.tag_name().namespace() == Some(ns) && names.contains(&n.tag_name().name())
}

/// Body properties (`a:bodyPr`).
pub(crate) fn body_props(n: Node) -> BodyProps {
    let mut b = BodyProps::default();
    for (i, k) in ["lIns", "tIns", "rIns", "bIns"].iter().enumerate() {
        b.insets[i] = attr_i64(n, k);
    }
    b.anchor = n.attribute("anchor").map(|a| match a {
        "ctr" => Anchor::Middle,
        "b" => Anchor::Bottom,
        _ => Anchor::Top,
    });
    b.no_wrap = n.attribute("wrap").map(|w| w == "none");
    b.vert = crate::frame::Vert::parse(n.attribute("vert"));
    let fit = child(n, NS_A, "normAutofit");
    b.font_scale = fit
        .and_then(|a| attr_i64(a, "fontScale"))
        .map(|v| v.clamp(1000, 100_000) as u32);
    b.line_reduction = fit
        .and_then(|a| attr_i64(a, "lnSpcReduction"))
        .map(|v| v.clamp(0, 90_000) as u32);
    b
}

/// A text body and the byte range of its paragraphs in the source.
pub(crate) fn parse_text_body(tb: Node, cx: &ParseCtx) -> (TextBody, Range<usize>) {
    let src = cx.src;
    let mut body = TextBody::new(Vec::new());
    if let Some(b) = child(tb, NS_A, "bodyPr") {
        body.body = body_props(b);
    }
    if let Some(l) = child(tb, NS_A, "lstStyle") {
        body.list = ListStyle::parse(l, &cx.colors);
    }
    let ps: Vec<Node> = children(tb, NS_A, "p").collect();
    let range = match (ps.first(), ps.last()) {
        (Some(a), Some(b)) => a.range().start..b.range().end,
        _ => {
            let at = tb
                .children()
                .filter(|c| c.is_element())
                .map(|c| c.range().end)
                .max()
                .unwrap_or_else(|| tb.range().start + start_tag(src, tb).len());
            at..at
        }
    };
    if tb.children().any(|c| {
        c.is_element() && !(is(c, NS_A, "bodyPr") || is(c, NS_A, "lstStyle") || is(c, NS_A, "p"))
    }) {
        body.locked = Some("unusual text body content".into());
    }
    for p in ps {
        let para = parse_para(p, cx, &mut body.locked);
        body.paras.push(para);
    }
    if body.paras.is_empty() {
        body.paras.push(TextPara::default());
    }
    (body, range)
}

fn parse_para(p: Node, cx: &ParseCtx, locked: &mut Option<String>) -> TextPara {
    let src = cx.src;
    let mut para = TextPara::default();
    para.runs.clear();
    for c in p.children().filter(|c| c.is_element()) {
        let in_a = c.tag_name().namespace() == Some(NS_A);
        match (in_a, c.tag_name().name()) {
            (true, "pPr") => para.props = para_props(c, cx),
            (true, "r" | "fld") => {
                if c.tag_name().name() == "fld" && locked.is_none() {
                    *locked = Some("fields (slide numbers, dates)".into());
                }
                let props = child(c, NS_A, "rPr")
                    .map(|r| run_props(r, cx))
                    .unwrap_or_default();
                let t: String = child(c, NS_A, "t")
                    .map(|t| {
                        t.descendants()
                            .filter(|d| d.is_text())
                            .filter_map(|d| d.text())
                            .collect::<String>()
                    })
                    .unwrap_or_default()
                    .replace(['\n', '\r', LINE_BREAK], " ");
                push_run(&mut para, &t, props);
            }
            (true, "br") => {
                let props = child(c, NS_A, "rPr")
                    .map(|r| run_props(r, cx))
                    .unwrap_or_default();
                push_run(&mut para, &LINE_BREAK.to_string(), props);
            }
            (true, "endParaRPr") => {
                para.end = Some(Arc::from(raw(src, c)));
            }
            (_, other) => {
                if locked.is_none() {
                    *locked = Some(match other {
                        "m" => "equations".into(),
                        o => format!("<{o}> in text"),
                    });
                }
            }
        }
    }
    if para.runs.is_empty() {
        // Typing into an empty paragraph uses its end-of-paragraph formatting.
        let props = para
            .end
            .as_deref()
            .map(|e| end_props(e, cx))
            .unwrap_or_default();
        para.runs.push(Run { len: 0, props });
    }
    para.normalize();
    para
}

/// Children of `a:pPr` that come before the bullet element (schema order).
const PPR_PRE: [&str; 10] = [
    "lnSpc", "spcBef", "spcAft", "buClrTx", "buClr", "buSzTx", "buSzPct", "buSzPts", "buFontTx",
    "buFont",
];

pub(crate) fn para_props(c: Node, cx: &ParseCtx) -> ParaProps {
    let src = cx.src;
    let mut style = para_style(c, &cx.colors);
    let mut pre = String::new();
    let mut post = String::new();
    for k in c.children().filter(|k| k.is_element()) {
        let name = k.tag_name().name();
        let in_a = k.tag_name().namespace() == Some(NS_A);
        if in_a && is_bullet_el(name) {
            style.bullet = bullet_of(k, raw(src, k));
        } else if in_a && PPR_PRE.contains(&name) && post.is_empty() {
            pre.push_str(raw(src, k));
        } else {
            post.push_str(raw(src, k));
        }
    }
    ParaProps {
        align: c.attribute("algn").and_then(Align::from_attr),
        level: attr_i64(c, "lvl").unwrap_or(0).clamp(0, 8) as u8,
        style,
        extra: Extra {
            attrs: attrs_except(c, src, &["algn", "lvl", "marL", "indent"]),
            pre,
            post,
        },
    }
}

fn push_run(para: &mut TextPara, text: &str, props: RunProps) {
    para.text.push_str(text);
    para.runs.push(Run {
        len: text.len(),
        props,
    });
}

/// Wrap a raw `a:` fragment so it parses on its own.
fn wrap_ns(fragment: &str) -> String {
    format!("<w xmlns:a=\"{NS_A}\" xmlns:r=\"{NS_R}\">{fragment}</w>")
}

/// A raw fragment of `a:` elements without the elements named `drop`.
pub(crate) fn strip_elements(fragment: &str, drop: &[&str]) -> String {
    let wrapped = wrap_ns(fragment);
    let Ok(d) = roxmltree::Document::parse(&wrapped) else {
        return fragment.to_owned();
    };
    let mut out = String::new();
    for c in d.root_element().children().filter(|c| c.is_element()) {
        if !drop.contains(&c.tag_name().name()) {
            out.push_str(raw(&wrapped, c));
        }
    }
    out
}

/// Run properties of a raw `a:endParaRPr`.
fn end_props(fragment: &str, cx: &ParseCtx) -> RunProps {
    let wrapped = wrap_ns(fragment);
    let Ok(d) = roxmltree::Document::parse(&wrapped) else {
        return RunProps::default();
    };
    let Some(el) = d.root_element().first_element_child() else {
        return RunProps::default();
    };
    let inner = ParseCtx {
        src: &wrapped,
        part: cx.part,
        rels: cx.rels,
        colors: cx.colors,
    };
    run_props(el, &inner)
}

const FILLS: [&str; 6] = [
    "noFill",
    "solidFill",
    "gradFill",
    "blipFill",
    "pattFill",
    "grpFill",
];

fn run_props(r: Node, cx: &ParseCtx) -> RunProps {
    let src = cx.src;
    let mut p = RunProps {
        bold: attr_bool(r, "b"),
        italic: attr_bool(r, "i"),
        size: attr_i64(r, "sz").map(|s| s.max(1) as u32),
        ..RunProps::default()
    };
    let mut skip = vec!["b", "i", "sz"];
    match r.attribute("u") {
        Some("sng") => {
            p.underline = Some(true);
            skip.push("u");
        }
        Some("none") => {
            p.underline = Some(false);
            skip.push("u");
        }
        Some(_) => p.shown_underline = true,
        None => {}
    }
    p.extra.attrs = attrs_except(r, src, &skip);
    let mut seen_fill = false;
    for c in r.children().filter(|c| c.is_element()) {
        let name = c.tag_name().name();
        let in_a = c.tag_name().namespace() == Some(NS_A);
        if in_a && FILLS.contains(&name) && !seen_fill {
            seen_fill = true;
            let srgb = (name == "solidFill")
                .then(|| {
                    let kids: Vec<Node> = c.children().filter(|k| k.is_element()).collect();
                    match kids.as_slice() {
                        [k] if is(*k, NS_A, "srgbClr")
                            && !k.children().any(|g| g.is_element())
                            && k.attributes().count() == 1 =>
                        {
                            k.attribute("val").and_then(Rgb::parse)
                        }
                        _ => None,
                    }
                })
                .flatten();
            match srgb {
                Some(c) => p.color = Some(c),
                None => {
                    p.shown_color = cx.colors.fill(c);
                    p.extra.pre.push_str(raw(src, c));
                }
            }
        } else if in_a && name == "ln" && !seen_fill {
            p.extra.pre.push_str(raw(src, c));
        } else {
            if in_a && name == "latin" {
                p.shown_font = c
                    .attribute("typeface")
                    .and_then(|f| cx.colors.theme.font(f));
            }
            p.extra.post.push_str(raw(src, c));
        }
    }
    p
}

// ── writing ────────────────────────────────────────────────────────────────

pub(crate) fn write_rpr(out: &mut String, tag: &str, p: &RunProps) {
    let mut attrs = p.extra.attrs.clone();
    if let Some(b) = p.bold {
        attrs.push(("b".into(), if b { "1" } else { "0" }.into()));
    }
    if let Some(i) = p.italic {
        attrs.push(("i".into(), if i { "1" } else { "0" }.into()));
    }
    if let Some(u) = p.underline {
        attrs.retain(|(k, _)| k != "u");
        attrs.push(("u".into(), if u { "sng" } else { "none" }.into()));
    }
    if let Some(sz) = p.size {
        attrs.push(("sz".into(), sz.to_string()));
    }
    let has_children = !p.extra.pre.is_empty() || p.color.is_some() || !p.extra.post.is_empty();
    if attrs.is_empty() && !has_children && tag == "a:rPr" {
        return;
    }
    out.push('<');
    out.push_str(tag);
    write_attrs(out, &attrs);
    if !has_children {
        out.push_str("/>");
        return;
    }
    out.push('>');
    out.push_str(&p.extra.pre);
    if let Some(c) = p.color {
        let _ = write!(
            out,
            "<a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill>",
            c.hex()
        );
    }
    out.push_str(&p.extra.post);
    let _ = write!(out, "</{tag}>");
}

pub(crate) fn write_para(out: &mut String, p: &TextPara) {
    out.push_str("<a:p>");
    write_ppr(out, &p.props);
    for (range, run) in p.run_ranges() {
        let text = &p.text[range];
        for (i, seg) in text.split(LINE_BREAK).enumerate() {
            if i > 0 {
                out.push_str("<a:br>");
                write_rpr(out, "a:rPr", &run.props);
                out.push_str("</a:br>");
            }
            if !seg.is_empty() {
                out.push_str("<a:r>");
                write_rpr(out, "a:rPr", &run.props);
                let _ = write!(out, "<a:t>{}</a:t></a:r>", esc(seg));
            }
        }
    }
    match &p.end {
        Some(e) => out.push_str(e),
        None if p.text.is_empty() && p.runs[0].props != RunProps::default() => {
            write_rpr(out, "a:endParaRPr", &p.runs[0].props);
        }
        None => {}
    }
    out.push_str("</a:p>");
}

/// The bullet element of a paragraph.
pub(crate) fn write_bullet(out: &mut String, b: &Bullet) {
    match b {
        Bullet::None => out.push_str("<a:buNone/>"),
        Bullet::Char(c) => {
            let _ = write!(out, "<a:buChar char=\"{}\"/>", esc_attr(c));
        }
        Bullet::AutoNum { scheme, start } => {
            let _ = write!(out, "<a:buAutoNum type=\"{}\"", esc_attr(scheme));
            if *start != 1 {
                let _ = write!(out, " startAt=\"{start}\"");
            }
            out.push_str("/>");
        }
        Bullet::Picture(raw) => out.push_str(raw),
    }
}

fn write_ppr(out: &mut String, pp: &ParaProps) {
    let st = &pp.style;
    let has_attrs = pp.align.is_some()
        || pp.level > 0
        || st.mar_l.is_some()
        || st.indent.is_some()
        || !pp.extra.attrs.is_empty();
    let has_children = st.bullet.is_some() || !pp.extra.pre.is_empty() || !pp.extra.post.is_empty();
    if !has_attrs && !has_children {
        return;
    }
    out.push_str("<a:pPr");
    if let Some(m) = st.mar_l {
        let _ = write!(out, " marL=\"{m}\"");
    }
    if pp.level > 0 {
        let _ = write!(out, " lvl=\"{}\"", pp.level);
    }
    if let Some(i) = st.indent {
        let _ = write!(out, " indent=\"{i}\"");
    }
    if let Some(a) = pp.align {
        let _ = write!(out, " algn=\"{}\"", a.attr());
    }
    write_attrs(out, &pp.extra.attrs);
    if !has_children {
        out.push_str("/>");
        return;
    }
    out.push('>');
    out.push_str(&pp.extra.pre);
    if let Some(b) = &st.bullet {
        write_bullet(out, b);
    }
    out.push_str(&pp.extra.post);
    out.push_str("</a:pPr>");
}

fn write_xfrm(out: &mut String, attrs: &str, x: &Xfrm) {
    out.push_str("<a:xfrm");
    let mut attrs = split_attrs(attrs);
    attrs.retain(|(k, _)| !matches!(k.as_str(), "rot" | "flipH" | "flipV"));
    if x.rot != 0 {
        attrs.push(("rot".into(), x.rot.to_string()));
    }
    if x.flip_h {
        attrs.push(("flipH".into(), "1".into()));
    }
    if x.flip_v {
        attrs.push(("flipV".into(), "1".into()));
    }
    write_attrs(out, &attrs);
    let _ = write!(
        out,
        "><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm>",
        x.x, x.y, x.cx, x.cy
    );
}

fn write_paras(paras: &[TextPara]) -> String {
    let mut s = String::new();
    for p in paras {
        write_para(&mut s, p);
    }
    s
}

pub(crate) fn write_paint(out: &mut String, p: &Paint) {
    match p {
        Paint::None => out.push_str("<a:noFill/>"),
        Paint::Scheme(s) => {
            let _ = write!(
                out,
                "<a:solidFill><a:schemeClr val=\"{}\"/></a:solidFill>",
                esc_attr(s)
            );
        }
        Paint::Rgb(c) => out.push_str(&solid(*c)),
    }
}

/// `a:ln` attributes with the width set.
fn ln_attrs(attrs: &str, w: Option<Emu>) -> Vec<(String, String)> {
    let mut a = split_attrs(attrs);
    if let Some(w) = w {
        a.retain(|(k, _)| k != "w");
        a.insert(0, ("w".into(), w.to_string()));
    }
    a
}

/// A whole new `a:ln` for a shape that has none (or `<a:ln/>`).
fn new_ln(attrs: &str, p: &PaintEdit) -> String {
    let mut s = String::from("<a:ln");
    write_attrs(&mut s, &ln_attrs(attrs, p.line_w));
    match &p.line {
        Some(c) => {
            s.push('>');
            write_paint(&mut s, c);
            s.push_str("</a:ln>");
        }
        None => s.push_str("/>"),
    }
    s
}

/// Edits (range, text, order at equal positions) for fill and outline
/// changes inside `p:spPr`; `None` when `p:spPr` must be rewritten whole.
fn paint_edits(at: &SpPrAt, p: &PaintEdit) -> Vec<(Range<usize>, String, u8)> {
    let mut edits = Vec::new();
    if let Some(f) = &p.fill {
        let mut buf = String::new();
        write_paint(&mut buf, f);
        match &at.fill {
            Some(r) => edits.push((r.clone(), buf, 1)),
            None => edits.push((at.fill_at..at.fill_at, buf, 1)),
        }
    }
    if p.line.is_none() && p.line_w.is_none() {
        return edits;
    }
    match &at.ln {
        Some(ln) => match ln.inner_at {
            Some(inner) => {
                let mut tag = String::from("<a:ln");
                write_attrs(&mut tag, &ln_attrs(&ln.attrs, p.line_w));
                tag.push('>');
                edits.push((ln.range.start..inner, tag, 2));
                if let Some(c) = &p.line {
                    let mut buf = String::new();
                    write_paint(&mut buf, c);
                    match &ln.fill {
                        Some(r) => edits.push((r.clone(), buf, 2)),
                        None => edits.push((inner..inner, buf, 3)),
                    }
                }
            }
            None => edits.push((ln.range.clone(), new_ln(&ln.attrs, p), 2)),
        },
        None => {
            let pos = at.fill.as_ref().map_or(at.fill_at, |r| r.end);
            edits.push((pos..pos, new_ln("", p), 2));
        }
    }
    edits
}

/// Write one shape: raw when unchanged, spliced when edited, generated when new.
pub(crate) fn write_shape(out: &mut String, s: &Shape) {
    let Some(src) = &s.src else {
        generate(out, s);
        return;
    };
    if s.unchanged() {
        out.push_str(&src.raw);
        return;
    }
    let mut edits: Vec<(Range<usize>, String, u8)> = Vec::new();
    // `<p:spPr/>` with a new fill or outline: written whole (with the xfrm).
    let whole_sppr = !s.paint.is_empty() && src.sppr.as_ref().is_some_and(|a| a.empty);
    if whole_sppr && let Some(at) = &src.sppr {
        let mut buf = String::from("<p:spPr");
        if !at.attrs.is_empty() {
            buf.push(' ');
            buf.push_str(&at.attrs);
        }
        buf.push('>');
        if let Some(x) = &s.xfrm {
            write_xfrm(&mut buf, "", x);
        }
        if let Some(f) = &s.paint.fill {
            write_paint(&mut buf, f);
        }
        if s.paint.line.is_some() || s.paint.line_w.is_some() {
            buf.push_str(&new_ln("", &s.paint));
        }
        buf.push_str("</p:spPr>");
        edits.push((at.range.clone(), buf, 0));
    } else if !s.paint.is_empty()
        && let Some(at) = &src.sppr
    {
        edits.extend(paint_edits(at, &s.paint));
    }
    if s.xfrm != src.xfrm
        && !whole_sppr
        && let (Some(x), Some(at)) = (&s.xfrm, &src.xfrm_at)
    {
        let mut buf = String::new();
        match at {
            XfrmAt::Replace(r, attrs) => {
                write_xfrm(&mut buf, attrs, x);
                edits.push((r.clone(), buf, 0));
            }
            XfrmAt::Insert(i) => {
                write_xfrm(&mut buf, "", x);
                edits.push((*i..*i, buf, 0));
            }
            XfrmAt::Expand(r, attrs) => {
                buf.push_str("<p:spPr");
                if !attrs.is_empty() {
                    buf.push(' ');
                    buf.push_str(attrs);
                }
                buf.push('>');
                write_xfrm(&mut buf, "", x);
                buf.push_str("</p:spPr>");
                edits.push((r.clone(), buf, 0));
            }
        }
    }
    if let (Some(t), Some(at)) = (&s.text, &src.paras_at)
        && src.text.as_ref() != Some(&t.paras)
    {
        edits.push((at.clone(), write_paras(&t.paras), 0));
    }
    let mut raw = src.raw.clone();
    // From the end; at one position the later child (higher order) first,
    // so it ends up after the earlier one.
    edits.sort_by_key(|(r, _, o)| (std::cmp::Reverse(r.start), std::cmp::Reverse(*o)));
    for (r, text, _) in edits {
        raw.replace_range(r, &text);
    }
    out.push_str(&raw);
}

fn solid(c: Rgb) -> String {
    format!(
        "<a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill>",
        c.hex()
    )
}

fn generate(out: &mut String, s: &Shape) {
    let name = esc_attr(&s.name);
    match &s.kind {
        ShapeKind::Picture { rel, .. } => {
            let _ = write!(
                out,
                "<p:pic><p:nvPicPr><p:cNvPr id=\"{}\" name=\"{name}\"/><p:cNvPicPr><a:picLocks noChangeAspect=\"1\"/></p:cNvPicPr><p:nvPr/></p:nvPicPr>\
                 <p:blipFill><a:blip r:embed=\"{}\"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr>",
                s.id,
                esc_attr(rel)
            );
            if let Some(x) = &s.xfrm {
                write_xfrm(out, "", x);
            }
            out.push_str("<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom>");
            if let Some(f) = &s.paint.fill {
                write_paint(out, f);
            }
            if s.paint.line.is_some() || s.paint.line_w.is_some() {
                out.push_str(&new_ln("", &s.paint));
            }
            out.push_str("</p:spPr></p:pic>");
        }
        ShapeKind::Shape {
            geom,
            text_box,
            geometry,
        } => {
            let _ = write!(
                out,
                "<p:sp><p:nvSpPr><p:cNvPr id=\"{}\" name=\"{name}\"/>",
                s.id
            );
            match (&s.ph, text_box) {
                (Some(_), _) => out.push_str("<p:cNvSpPr><a:spLocks noGrp=\"1\"/></p:cNvSpPr>"),
                (None, true) => out.push_str("<p:cNvSpPr txBox=\"1\"/>"),
                (None, false) => out.push_str("<p:cNvSpPr/>"),
            }
            match &s.ph {
                Some(ph) => {
                    out.push_str("<p:nvPr><p:ph");
                    if let Some(k) = &ph.kind {
                        let _ = write!(out, " type=\"{}\"", esc_attr(k));
                    }
                    if let Some(i) = ph.idx {
                        let _ = write!(out, " idx=\"{i}\"");
                    }
                    out.push_str("/></p:nvPr></p:nvSpPr>");
                }
                None => out.push_str("<p:nvPr/></p:nvSpPr>"),
            }
            out.push_str("<p:spPr>");
            if let Some(x) = &s.xfrm {
                write_xfrm(out, "", x);
            }
            if s.ph.is_none() {
                // A preset read from a file keeps its name and adjustments;
                // custom geometry isn't generated (shapes read from a file
                // are written from their own XML).
                match geometry {
                    Some(Geometry::Preset { name, adjust })
                        if crate::geom::preset(name).is_some() =>
                    {
                        let _ = write!(out, "<a:prstGeom prst=\"{}\"><a:avLst", esc_attr(name));
                        if adjust.is_empty() {
                            out.push_str("/>");
                        } else {
                            out.push('>');
                            for g in adjust {
                                let _ = write!(
                                    out,
                                    "<a:gd name=\"{}\" fmla=\"{}\"/>",
                                    esc_attr(&g.name),
                                    esc_attr(&g.fmla.to_string())
                                );
                            }
                            out.push_str("</a:avLst>");
                        }
                        out.push_str("</a:prstGeom>");
                    }
                    _ => {
                        let prst = match geom {
                            Geom::Ellipse => "ellipse",
                            Geom::RoundRect => "roundRect",
                            Geom::Line => "line",
                            Geom::Rect | Geom::Other(_) => "rect",
                        };
                        let _ = write!(out, "<a:prstGeom prst=\"{prst}\"><a:avLst/></a:prstGeom>");
                    }
                }
                match (&s.paint.fill, s.look.fill) {
                    (Some(p), _) => write_paint(out, p),
                    (None, Some(c)) => out.push_str(&solid(c)),
                    (None, None) => out.push_str("<a:noFill/>"),
                }
                let line = match (&s.paint.line, s.look.line) {
                    (Some(Paint::None), _) => Some(Paint::None),
                    (Some(p), _) => Some(p.clone()),
                    (None, Some((c, _))) => Some(Paint::Rgb(c)),
                    (None, None) => None,
                };
                if let Some(p) = line {
                    let w = s
                        .paint
                        .line_w
                        .or(s.look.line.map(|l| l.1))
                        .unwrap_or(12_700);
                    let _ = write!(out, "<a:ln w=\"{w}\">");
                    write_paint(out, &p);
                    out.push_str("</a:ln>");
                }
            }
            out.push_str("</p:spPr>");
            if let Some(c) = s.look.text_color.filter(|_| s.ph.is_none()) {
                let z = "<a:scrgbClr r=\"0\" g=\"0\" b=\"0\"/>";
                let _ = write!(
                    out,
                    "<p:style><a:lnRef idx=\"0\">{z}</a:lnRef><a:fillRef idx=\"0\">{z}</a:fillRef>\
                     <a:effectRef idx=\"0\">{z}</a:effectRef><a:fontRef idx=\"minor\"><a:srgbClr val=\"{}\"/></a:fontRef></p:style>",
                    c.hex()
                );
            }
            let empty = TextBody::new(vec![TextPara::default()]);
            let t = s.text.as_ref().unwrap_or(&empty);
            out.push_str("<p:txBody><a:bodyPr");
            if s.ph.is_none() {
                out.push_str(if t.body.no_wrap == Some(true) {
                    " wrap=\"none\""
                } else {
                    " wrap=\"square\""
                });
                out.push_str(" rtlCol=\"0\"");
                match t.body.anchor {
                    Some(Anchor::Middle) => out.push_str(" anchor=\"ctr\""),
                    Some(Anchor::Bottom) => out.push_str(" anchor=\"b\""),
                    _ => {}
                }
            }
            out.push_str("/><a:lstStyle/>");
            out.push_str(&write_paras(&t.paras));
            out.push_str("</p:txBody></p:sp>");
        }
        // Never created by Kantor; copied shapes keep their source.
        ShapeKind::Other { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ClrMap, Theme};

    const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;

    fn with_shapes(xml: &str, f: impl FnOnce(Vec<Shape>, &str)) {
        let src = format!("<p:spTree {NS}><p:nvGrpSpPr/><p:grpSpPr/>{xml}</p:spTree>");
        let doc = roxmltree::Document::parse(&src).unwrap();
        let theme = Theme::default();
        let map = ClrMap::default();
        let cx = ParseCtx {
            src: &src,
            part: "ppt/slides/slide1.xml",
            rels: &[],
            colors: ColorCtx {
                theme: &theme,
                map: &map,
                ph: None,
            },
        };
        let (shapes, _) = parse_tree(doc.root_element(), &cx);
        f(shapes, &src);
    }

    const SP: &str = r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="T"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm rot="60000"><a:off x="1" y="2"/><a:ext cx="3" cy="4"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr wrap="square"/><a:lstStyle/><a:p><a:pPr algn="ctr" marL="10"><a:buNone/></a:pPr><a:r><a:rPr lang="en-US" b="1" dirty="0"><a:solidFill><a:schemeClr val="accent1"/></a:solidFill><a:latin typeface="Arial"/></a:rPr><a:t>Hi &amp; bye</a:t></a:r><a:br><a:rPr lang="en-US"/></a:br><a:r><a:rPr lang="en-US" sz="2400"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:rPr><a:t>two</a:t></a:r><a:endParaRPr lang="en-US"/></a:p></p:txBody></p:sp>"#;

    #[test]
    fn unchanged_shape_is_written_verbatim() {
        with_shapes(SP, |shapes, _| {
            let s = &shapes[0];
            let rotated = Xfrm {
                rot: 60_000,
                ..Xfrm::new(1, 2, 3, 4)
            };
            assert_eq!(s.xfrm, Some(rotated));
            let t = s.text.as_ref().unwrap();
            assert_eq!(t.paras[0].text, format!("Hi & bye{LINE_BREAK}two"));
            assert_eq!(t.paras[0].props.align, Some(Align::Center));
            let first = &t.paras[0].runs[0].props;
            assert_eq!(first.bold, Some(true));
            assert!(first.extra.pre.contains("schemeClr"));
            assert_eq!(first.shown_font.as_deref(), Some("Arial"));
            let mut out = String::new();
            write_shape(&mut out, s);
            assert_eq!(out, SP);
        });
    }

    #[test]
    fn regenerated_paragraphs_keep_unmodelled_xml() {
        with_shapes(SP, |shapes, _| {
            let mut s = shapes[0].clone();
            // Regenerate without a change in content: same XML semantics.
            let t = s.text.as_mut().unwrap();
            t.paras[0].insert(0, "", RunProps::default());
            t.paras[0].props.level = 0;
            let mut forced = String::new();
            write_paras_into(&mut forced, &t.paras);
            assert!(forced.contains(r#"marL="10""#));
            assert!(forced.contains("<a:buNone/>"));
            assert!(forced.contains(r#"<a:schemeClr val="accent1"/>"#));
            assert!(forced.contains(r#"<a:srgbClr val="FF0000"/>"#));
            assert!(forced.contains("<a:br><a:rPr lang=\"en-US\"/></a:br>"));
            assert!(forced.contains("Hi &amp; bye"));
            // Move: only the xfrm changes, rotation kept.
            let x = s.xfrm.unwrap();
            s.xfrm = Some(Xfrm {
                x: 10,
                y: 20,
                cx: 30,
                cy: 40,
                ..x
            });
            let mut out = String::new();
            write_shape(&mut out, &s);
            assert!(out.contains(
                r#"<a:xfrm rot="60000"><a:off x="10" y="20"/><a:ext cx="30" cy="40"/></a:xfrm>"#
            ));
            assert_eq!(
                out.replace(
                    r#"x="10" y="20"/><a:ext cx="30" cy="40""#,
                    r#"x="1" y="2"/><a:ext cx="3" cy="4""#
                ),
                SP
            );
        });
    }

    fn write_paras_into(out: &mut String, paras: &[TextPara]) {
        out.push_str(&write_paras(paras));
    }

    #[test]
    fn placeholder_without_xfrm_gets_one_inserted() {
        let sp = r#"<p:sp><p:nvSpPr><p:cNvPr id="3" name="Title 2"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:endParaRPr lang="en-US"/></a:p></p:txBody></p:sp>"#;
        with_shapes(sp, |shapes, _| {
            let mut s = shapes[0].clone();
            assert!(s.ph.as_ref().unwrap().is_title());
            assert_eq!(s.xfrm, None);
            s.xfrm = Some(Xfrm::new(5, 6, 7, 8));
            s.text.as_mut().unwrap().paras[0].insert(0, "Hello", RunProps::default());
            let mut out = String::new();
            write_shape(&mut out, &s);
            assert!(out.contains(r#"<p:spPr><a:xfrm><a:off x="5" y="6"/><a:ext cx="7" cy="8"/></a:xfrm></p:spPr>"#), "{out}");
            assert!(
                out.contains(
                    r#"<a:p><a:r><a:t>Hello</a:t></a:r><a:endParaRPr lang="en-US"/></a:p>"#
                ),
                "{out}"
            );
        });
    }

    #[test]
    fn fields_lock_text_and_groups_are_opaque() {
        let xml = r#"<p:sp><p:nvSpPr><p:cNvPr id="4" name="N"/><p:cNvSpPr/><p:nvPr><p:ph type="sldNum" idx="12"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:p><a:fld id="{1}" type="slidenum"><a:t>3</a:t></a:fld></a:p></p:txBody></p:sp><p:grpSp><p:nvGrpSpPr><p:cNvPr id="9" name="G"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="1" y="1"/><a:ext cx="2" cy="2"/><a:chOff x="0" y="0"/><a:chExt cx="2" cy="2"/></a:xfrm></p:grpSpPr><p:sp><p:nvSpPr><p:cNvPr id="11" name="in"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/></p:sp></p:grpSp>"#;
        with_shapes(xml, |shapes, _| {
            assert!(shapes[0].text.as_ref().unwrap().locked.is_some());
            assert!(!shapes[0].text_editable());
            assert!(matches!(shapes[1].kind, ShapeKind::Other { .. }));
            assert_eq!(shapes[1].xfrm, Some(Xfrm::new(1, 1, 2, 2)));
            assert_eq!(shapes[1].max_id(), 11);
        });
    }

    #[test]
    fn generated_shapes_parse_back() {
        let s = Shape {
            id: 7,
            name: "Oval 6".into(),
            kind: ShapeKind::Shape {
                geom: Geom::Ellipse,
                text_box: false,
                geometry: None,
            },
            xfrm: Some(Xfrm::new(1, 2, 3, 4)),
            ph: None,
            look: Look {
                fill: Some(Rgb([1, 2, 3])),
                line: Some((Rgb([4, 5, 6]), 12700)),
                text_color: Some(Rgb([255, 255, 255])),
                ..Look::default()
            },
            text: Some(TextBody::new(vec![TextPara::new("A <b>")])),
            paint: PaintEdit::default(),
            src: None,
        };
        let mut out = String::new();
        write_shape(&mut out, &s);
        with_shapes(&out, |shapes, _| {
            let b = &shapes[0];
            assert_eq!(b.kind, s.kind);
            assert_eq!(b.xfrm, s.xfrm);
            assert_eq!(
                (b.look.fill, b.look.line, b.look.text_color),
                (s.look.fill, s.look.line, s.look.text_color)
            );
            assert_eq!(
                b.look.fill_as_read,
                Some(crate::fill::Fill::Solid(Rgb([1, 2, 3]), 255))
            );
            assert_eq!(b.text.as_ref().unwrap().paras[0].text, "A <b>");
        });
        // A preset with adjustments is written with them.
        let mut arrow = s.clone();
        arrow.kind = ShapeKind::Shape {
            geom: Geom::Other("rightArrow".into()),
            text_box: false,
            geometry: Some(Geometry::Preset {
                name: "rightArrow".into(),
                adjust: vec![crate::geom::Guide::new("adj1", "val 80000").unwrap()],
            }),
        };
        let mut out = String::new();
        write_shape(&mut out, &arrow);
        assert!(out.contains(r#"<a:prstGeom prst="rightArrow"><a:avLst><a:gd name="adj1" fmla="val 80000"/></a:avLst></a:prstGeom>"#), "{out}");
        with_shapes(&out, |shapes, _| assert_eq!(shapes[0].kind, arrow.kind));
    }
}
