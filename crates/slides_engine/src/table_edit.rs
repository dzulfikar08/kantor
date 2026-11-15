//! Editing tables (`a:tbl` in a `p:graphicFrame`): cell text, rows and
//! columns, cell fill.
//!
//! Tables are kept as their XML ([`crate::frame`] reads them for display).
//! An edit splices that XML: a cell's paragraphs are rewritten in the
//! formatting of its first run and paragraph, a new row or column copies
//! its neighbour (formatting, height or width) with empty text, a fill
//! replaces the cell's own fill in `a:tcPr`. The frame grows or shrinks by
//! the row or column, then the shape is read again from the new XML, so
//! everything Kantor doesn't touch stays byte for byte.
//!
//! Merged cells follow PowerPoint: a row or column added inside a merged
//! cell widens it (the new grid cells are covered, `vMerge`/`hMerge`); one
//! added beside it copies its neighbour's merges that start there; removing
//! a row or column shrinks the merged cells crossing it, and one that held
//! a merged cell's top left hands its text to the cell taking its place.
//! Resizing a table scales its columns and rows, as PowerPoint does.

use std::ops::Range;

use anyhow::{Context as _, bail};
use docs_engine::Rgb;
use roxmltree::Node;

use crate::clip::{root_ns, wrap};
use crate::frame::FrameContent;
use crate::history::{Focus, Kind};
use crate::model::*;
use crate::presentation::Presentation;
use crate::shape::{ParseCtx, parse_shape};
use crate::theme::ColorCtx;
use crate::xml::*;

/// One change to a table.
#[derive(Clone, Debug, PartialEq)]
pub enum TableOp {
    /// Replace a cell's text (`\n` separates paragraphs).
    Text {
        row: usize,
        col: usize,
        text: String,
    },
    /// A new empty row above (`below == false`) or below row `row`.
    InsertRow {
        row: usize,
        below: bool,
    },
    DeleteRow(usize),
    /// A new empty column left or right of column `col`.
    InsertCol {
        col: usize,
        right: bool,
    },
    DeleteCol(usize),
    /// A cell's own fill (`None` = no fill of its own: the table style's).
    Fill {
        row: usize,
        col: usize,
        color: Option<Rgb>,
    },
    /// Scale the columns and rows so the table is `cx` × `cy` (EMU).
    Resize {
        cx: Emu,
        cy: Emu,
    },
    /// New column widths (one per column, EMU); the frame follows.
    ColWidths(Vec<Emu>),
    /// A cell's paragraphs, formatting included (typing on the slide).
    Paras {
        row: usize,
        col: usize,
        paras: Vec<TextPara>,
    },
}

/// The XML being edited: the frame wrapped in an element declaring the
/// slide's namespaces.
struct Doc {
    ns: Vec<(String, String)>,
    xml: String,
    start: usize,
}

impl Doc {
    fn new(ns: Vec<(String, String)>, raw: &str) -> Self {
        let (xml, start) = wrap(&ns, raw);
        Self { ns, xml, start }
    }

    fn raw(&self) -> &str {
        &self.xml[self.start..self.xml.len() - "</w>".len()]
    }

    /// Apply edits (absolute ranges in `xml`), back to front.
    fn apply(&mut self, mut edits: Vec<(Range<usize>, String)>) {
        edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
        for (r, t) in edits {
            self.xml.replace_range(r, &t);
        }
        let raw = self.raw().to_owned();
        *self = Doc::new(std::mem::take(&mut self.ns), &raw);
    }
}

fn tbl<'a, 'i>(doc: &'a roxmltree::Document<'i>) -> anyhow::Result<Node<'a, 'i>> {
    doc.descendants()
        .find(|n| is(*n, NS_A, "tbl"))
        .context("not a table")
}

fn rows<'a, 'i>(t: Node<'a, 'i>) -> Vec<Node<'a, 'i>> {
    t.children().filter(|c| is(*c, NS_A, "tr")).collect()
}

fn cells<'a, 'i>(tr: Node<'a, 'i>) -> Vec<Node<'a, 'i>> {
    tr.children().filter(|c| is(*c, NS_A, "tc")).collect()
}

/// The `prefix:` an element is written with.
fn prefix_of(src: &str, n: Node) -> String {
    let tag = &src[n.range().start + 1..];
    let name: String = tag
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '>' && *c != '/')
        .collect();
    match name.split_once(':') {
        Some((p, _)) => format!("{p}:"),
        None => String::new(),
    }
}

/// A cell's text as paragraphs in the formatting of its first paragraph
/// and run (`text` empty: one empty paragraph).
fn cell_paras(src: &str, tc: Node, text: &str, a: &str) -> String {
    let body = child(tc, NS_A, "txBody");
    let first_p = body.and_then(|b| child(b, NS_A, "p"));
    let ppr = first_p
        .and_then(|p| child(p, NS_A, "pPr"))
        .map(|n| raw(src, n).to_owned())
        .unwrap_or_default();
    let rpr = first_p
        .and_then(|p| {
            p.descendants()
                .find(|d| is(*d, NS_A, "rPr") || is(*d, NS_A, "endParaRPr"))
        })
        .map(|n| raw(src, n).to_owned());
    let (run_pr, end_pr) = match &rpr {
        Some(r) if r.contains(&format!("<{a}endParaRPr")) => (
            r.replacen(&format!("<{a}endParaRPr"), &format!("<{a}rPr"), 1)
                .replace(&format!("</{a}endParaRPr>"), &format!("</{a}rPr>")),
            r.clone(),
        ),
        Some(r) => (
            r.clone(),
            r.replacen(&format!("<{a}rPr"), &format!("<{a}endParaRPr"), 1)
                .replace(&format!("</{a}rPr>"), &format!("</{a}endParaRPr>")),
        ),
        None => (String::new(), String::new()),
    };
    let mut out = String::new();
    for line in text.split('\n') {
        out.push_str(&format!("<{a}p>{ppr}"));
        if line.is_empty() {
            out.push_str(&end_pr);
        } else {
            out.push_str(&format!("<{a}r>{run_pr}<{a}t>{}</{a}t></{a}r>", esc(line)));
        }
        out.push_str(&format!("</{a}p>"));
    }
    out
}

/// Edits that put `text` in cell `tc`.
fn set_text(src: &str, tc: Node, text: &str) -> Vec<(Range<usize>, String)> {
    let a = prefix_of(src, tc);
    let paras = cell_paras(src, tc, text, &a);
    match child(tc, NS_A, "txBody") {
        Some(body) => {
            let ps: Vec<Node> = body.children().filter(|c| is(*c, NS_A, "p")).collect();
            match (ps.first(), ps.last()) {
                (Some(f), Some(l)) => vec![(f.range().start..l.range().end, paras)],
                _ => {
                    let at = body.range().end - format!("</{a}txBody>").len();
                    vec![(at..at, paras)]
                }
            }
        }
        None => {
            let at = tc.range().start + tag_end(&src[tc.range()]).unwrap_or(0);
            vec![(
                at..at,
                format!("<{a}txBody><{a}bodyPr/><{a}lstStyle/>{paras}</{a}txBody>"),
            )]
        }
    }
}

/// Edits that put model paragraphs `paras` in cell `tc` (formatting kept
/// as the model has it; written with the `a:` prefix).
fn set_paras(src: &str, tc: Node, paras: &[TextPara]) -> Vec<(Range<usize>, String)> {
    let a = prefix_of(src, tc);
    if a != "a:" {
        let text: Vec<String> = paras.iter().map(|p| p.text.clone()).collect();
        return set_text(src, tc, &text.join("\n"));
    }
    let xml = crate::shape::write_paras(paras);
    match child(tc, NS_A, "txBody") {
        Some(body) => {
            let ps: Vec<Node> = body.children().filter(|c| is(*c, NS_A, "p")).collect();
            match (ps.first(), ps.last()) {
                (Some(f), Some(l)) => vec![(f.range().start..l.range().end, xml)],
                _ => {
                    let at = body.range().end - "</a:txBody>".len();
                    vec![(at..at, xml)]
                }
            }
        }
        None => {
            let at = tc.range().start + tag_end(&src[tc.range()]).unwrap_or(0);
            vec![(
                at..at,
                format!("<a:txBody><a:bodyPr/><a:lstStyle/>{xml}</a:txBody>"),
            )]
        }
    }
}

/// A copy of element `n` with every cell's text emptied and its own
/// `extLst` (unique row/column ids) left out.
fn blank_copy(src: &str, n: Node) -> String {
    let base = n.range().start;
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    for c in n.children().filter(|c| is(*c, NS_A, "extLst")) {
        edits.push((c.range().start - base..c.range().end - base, String::new()));
    }
    let tcs: Vec<Node> = if is(n, NS_A, "tc") {
        vec![n]
    } else {
        n.descendants().filter(|d| is(*d, NS_A, "tc")).collect()
    };
    for tc in tcs {
        for (r, t) in set_text(src, tc, "") {
            edits.push((r.start - base..r.end - base, t));
        }
    }
    let mut out = raw(src, n).to_owned();
    edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
    for (r, t) in edits {
        out.replace_range(r, &t);
    }
    out
}

/// The frame's size (`p:xfrm/a:ext`) changed by `dcx`, `dcy`.
fn grow_frame(src: &str, frame: Node, dcx: i64, dcy: i64) -> Vec<(Range<usize>, String)> {
    let Some(ext) = frame
        .children()
        .find(|c| c.is_element() && c.tag_name().name() == "xfrm")
        .and_then(|x| child(x, NS_A, "ext"))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, d) in [("cx", dcx), ("cy", dcy)] {
        if d == 0 {
            continue;
        }
        if let Some(a) = ext.attributes().find(|a| a.name() == name) {
            let v = a.value().parse::<i64>().unwrap_or(0);
            out.push((a.range_value(), (v + d).max(1).to_string()));
        }
    }
    let _ = src;
    out
}

/// A fill element for a cell (`a:` prefix).
fn fill_xml(a: &str, color: Option<Rgb>) -> String {
    match color {
        Some(c) => format!(
            "<{a}solidFill><{a}srgbClr val=\"{}\"/></{a}solidFill>",
            c.hex()
        ),
        None => String::new(),
    }
}

fn set_fill(src: &str, tc: Node, color: Option<Rgb>) -> Vec<(Range<usize>, String)> {
    let a = prefix_of(src, tc);
    let fill = fill_xml(&a, color);
    const FILLS: [&str; 6] = [
        "noFill",
        "solidFill",
        "gradFill",
        "blipFill",
        "pattFill",
        "grpFill",
    ];
    match child(tc, NS_A, "tcPr") {
        Some(pr) => {
            let mut edits: Vec<(Range<usize>, String)> = pr
                .children()
                .filter(|c| c.is_element() && FILLS.contains(&c.tag_name().name()))
                .map(|c| (c.range(), String::new()))
                .collect();
            if fill.is_empty() {
                return edits;
            }
            let text = raw(src, pr);
            if text.trim_end().ends_with("/>") && !text.contains(&format!("</{a}tcPr>")) {
                // `<a:tcPr …/>`: open it up.
                let open = text.trim_end().trim_end_matches("/>").trim_end().to_owned();
                return vec![(pr.range(), format!("{open}>{fill}</{a}tcPr>"))];
            }
            let at = pr
                .children()
                .find(|c| c.is_element() && matches!(c.tag_name().name(), "headers" | "extLst"))
                .map(|c| c.range().start)
                .unwrap_or(pr.range().end - format!("</{a}tcPr>").len());
            edits.push((at..at, fill));
            edits
        }
        None if fill.is_empty() => Vec::new(),
        None => {
            let at = child(tc, NS_A, "txBody")
                .map(|b| b.range().end)
                .unwrap_or_else(|| tc.range().start + tag_end(&src[tc.range()]).unwrap_or(0));
            vec![(at..at, format!("<{a}tcPr>{fill}</{a}tcPr>"))]
        }
    }
}

/// Which cell shows at each grid position: `owner[r][c]` is the top
/// left `(row, col)` of the (merged) cell covering it.
struct Grid<'a, 'i> {
    trs: Vec<Node<'a, 'i>>,
    tcs: Vec<Vec<Node<'a, 'i>>>,
    owner: Vec<Vec<(usize, usize)>>,
}

fn span(tc: Node, name: &str) -> usize {
    attr_i64(tc, name).unwrap_or(1).max(1) as usize
}

fn flag(tc: Node, name: &str) -> bool {
    tc.attribute(name).is_some_and(|v| v == "1" || v == "true")
}

impl<'a, 'i> Grid<'a, 'i> {
    fn new(t: Node<'a, 'i>, nc: usize) -> anyhow::Result<Self> {
        let trs = rows(t);
        let tcs: Vec<Vec<Node>> = trs.iter().map(|tr| cells(*tr)).collect();
        if tcs.iter().any(|r| r.len() != nc) {
            bail!("This table's rows don't match its columns: it can't be changed");
        }
        let nr = trs.len();
        let mut owner: Vec<Vec<(usize, usize)>> =
            (0..nr).map(|r| (0..nc).map(|c| (r, c)).collect()).collect();
        let mut claimed = vec![vec![false; nc]; nr];
        for r in 0..nr {
            for c in 0..nc {
                let tc = tcs[r][c];
                if flag(tc, "hMerge") || flag(tc, "vMerge") || claimed[r][c] {
                    continue;
                }
                let (rs, cs) = (span(tc, "rowSpan"), span(tc, "gridSpan"));
                for (rr, row) in owner.iter_mut().enumerate().skip(r).take(rs) {
                    for (cc, o) in row.iter_mut().enumerate().skip(c).take(cs) {
                        if (rr, cc) != (r, c) {
                            *o = (r, c);
                            claimed[rr][cc] = true;
                        }
                    }
                }
            }
        }
        Ok(Self { trs, tcs, owner })
    }

    fn rows(&self) -> usize {
        self.trs.len()
    }

    /// The merged cell owning `(r, c)` spans more than one grid cell.
    fn origin(&self, r: usize, c: usize) -> Node<'a, 'i> {
        let (or, oc) = self.owner[r][c];
        self.tcs[or][oc]
    }
}

/// Byte range of an element's start tag.
fn start_range(src: &str, n: Node) -> Range<usize> {
    let s = n.range().start;
    s..s + tag_end(&src[n.range()]).unwrap_or(0)
}

/// `xml` (an element, start tag first) with attributes set (`Some`) or
/// removed (`None`); a span of 1 is written by removing the attribute.
fn retag(xml: &str, set: &[(&str, Option<String>)]) -> String {
    let Some(end) = tag_end(xml) else {
        return xml.to_owned();
    };
    let tag = &xml[..end];
    let empty = tag.ends_with("/>");
    let name = tag_name(tag).to_owned();
    let mut attrs = split_attrs(tag_attrs(tag));
    for (k, v) in set {
        let v = v.clone().filter(|v| v != "1" || !k.ends_with("Span"));
        match (attrs.iter().position(|(a, _)| a == k), v) {
            (Some(i), Some(v)) => attrs[i].1 = v,
            (Some(i), None) => {
                attrs.remove(i);
            }
            (None, Some(v)) => attrs.push(((*k).to_owned(), v)),
            (None, None) => {}
        }
    }
    let mut out = format!("<{name}");
    write_attrs(&mut out, &attrs);
    out.push_str(if empty { "/>" } else { ">" });
    out.push_str(&xml[end..]);
    out
}

/// A start-tag-only edit of `tc`.
fn retag_edit(src: &str, tc: Node, set: &[(&str, Option<String>)]) -> (Range<usize>, String) {
    let r = start_range(src, tc);
    (r.clone(), retag(&src[r], set))
}

const NO_MERGE: [(&str, Option<String>); 4] = [
    ("gridSpan", None),
    ("rowSpan", None),
    ("hMerge", None),
    ("vMerge", None),
];

fn yes(on: bool) -> Option<String> {
    on.then(|| "1".to_owned())
}

/// The frame's size (`p:xfrm/a:ext`) set to `cx`, `cy` (`None` = kept).
fn set_frame_ext(frame: Node, cx: Option<Emu>, cy: Option<Emu>) -> Vec<(Range<usize>, String)> {
    let Some(ext) = frame
        .children()
        .find(|c| c.is_element() && c.tag_name().name() == "xfrm")
        .and_then(|x| child(x, NS_A, "ext"))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, v) in [("cx", cx), ("cy", cy)] {
        if let (Some(v), Some(a)) = (v, ext.attributes().find(|a| a.name() == name)) {
            out.push((a.range_value(), v.max(1).to_string()));
        }
    }
    out
}

fn grid_cols<'a, 'i>(t: Node<'a, 'i>) -> anyhow::Result<Vec<Node<'a, 'i>>> {
    let grid = child(t, NS_A, "tblGrid").context("no column grid")?;
    Ok(grid
        .children()
        .filter(|c| is(*c, NS_A, "gridCol"))
        .collect())
}

/// `total` shared out in proportion to `parts` (rounded, summing exactly).
fn share(parts: &[Emu], total: Emu) -> Vec<Emu> {
    let sum: Emu = parts.iter().sum();
    if sum <= 0 || parts.is_empty() {
        return parts.to_vec();
    }
    let mut out = Vec::with_capacity(parts.len());
    let (mut acc, mut given) = (0i128, 0i64);
    for p in parts {
        acc += *p as i128;
        let upto = (acc * total as i128 / sum as i128) as i64;
        out.push((upto - given).max(1));
        given = upto;
    }
    out
}

fn insert_row(
    src: &str,
    t: Node,
    frame: Node,
    row: usize,
    below: bool,
) -> anyhow::Result<Vec<(Range<usize>, String)>> {
    let nc = grid_cols(t)?.len();
    let g = Grid::new(t, nc)?;
    let tr = *g.trs.get(row).context("no such row")?;
    let k = if below { row + 1 } else { row };
    let a = prefix_of(src, tr);
    let mut e = Vec::new();
    let mut bumped: Vec<(usize, usize)> = Vec::new();
    let mut new_row = start_tag(src, tr).trim_end_matches("/>").to_owned();
    if !new_row.ends_with('>') {
        new_row.push('>');
    }
    for c in 0..nc {
        let crossing = k > 0 && k < g.rows() && g.owner[k - 1][c] == g.owner[k][c];
        if crossing {
            let (or, oc) = g.owner[k][c];
            if !bumped.contains(&(or, oc)) {
                bumped.push((or, oc));
                let o = g.tcs[or][oc];
                let rs = span(o, "rowSpan") + 1;
                e.push(retag_edit(src, o, &[("rowSpan", Some(rs.to_string()))]));
            }
            let copy = blank_copy(src, g.tcs[k][c]);
            new_row.push_str(&retag(
                &copy,
                &[
                    ("gridSpan", None),
                    ("rowSpan", None),
                    ("hMerge", yes(c != oc)),
                    ("vMerge", yes(true)),
                ],
            ));
            continue;
        }
        let tc = g.tcs[row][c];
        let (or, oc) = g.owner[row][c];
        let copy = blank_copy(src, tc);
        let set: Vec<(&str, Option<String>)> = if or == row {
            // A merge across this row's cells is copied (not down the rows).
            let gs = span(g.origin(row, c), "gridSpan");
            vec![
                ("gridSpan", (c == oc && gs > 1).then(|| gs.to_string())),
                ("rowSpan", None),
                ("hMerge", yes(c != oc)),
                ("vMerge", None),
            ]
        } else {
            NO_MERGE.to_vec()
        };
        new_row.push_str(&retag(&copy, &set));
    }
    new_row.push_str(&format!("</{a}tr>"));
    let at = if below {
        tr.range().end
    } else {
        tr.range().start
    };
    // Below a merged cell's last row the new row goes after it, not inside.
    e.push((at..at, new_row));
    let h = attr_i64(tr, "h").unwrap_or(370_840);
    e.extend(grow_frame(src, frame, 0, h));
    Ok(e)
}

fn delete_row(
    src: &str,
    t: Node,
    frame: Node,
    row: usize,
) -> anyhow::Result<Vec<(Range<usize>, String)>> {
    let nc = grid_cols(t)?.len();
    let g = Grid::new(t, nc)?;
    if g.rows() <= 1 {
        bail!("A table needs at least one row");
    }
    let tr = *g.trs.get(row).context("no such row")?;
    let mut e = Vec::new();
    let mut done: Vec<(usize, usize)> = Vec::new();
    for c in 0..nc {
        let (or, oc) = g.owner[row][c];
        let o = g.tcs[or][oc];
        let (rs, gs) = (span(o, "rowSpan"), span(o, "gridSpan"));
        if done.contains(&(or, oc)) {
            continue;
        }
        done.push((or, oc));
        if or < row {
            // Covered from above: the merged cell loses a row.
            e.push(retag_edit(
                src,
                o,
                &[("rowSpan", Some((rs - 1).to_string()))],
            ));
        } else if rs > 1 && row + 1 < g.rows() {
            // Its top left goes: the cell below takes its text and spans.
            let next = g.tcs[row + 1][oc];
            let moved = retag(
                raw(src, o),
                &[("rowSpan", Some((rs - 1).to_string())), ("vMerge", None)],
            );
            e.push((next.range(), moved));
            for tc in g.tcs[row + 1].iter().take((oc + gs).min(nc)).skip(oc + 1) {
                e.push(retag_edit(src, *tc, &[("vMerge", None)]));
            }
        }
    }
    e.push((tr.range(), String::new()));
    let h = attr_i64(tr, "h").unwrap_or(0);
    e.extend(grow_frame(src, frame, 0, -h));
    Ok(e)
}

fn insert_col(
    src: &str,
    t: Node,
    frame: Node,
    col: usize,
    right: bool,
) -> anyhow::Result<Vec<(Range<usize>, String)>> {
    let cols = grid_cols(t)?;
    let nc = cols.len();
    let g = Grid::new(t, nc)?;
    let gc = *cols.get(col).context("no such column")?;
    let k = if right { col + 1 } else { col };
    let w = attr_i64(gc, "w").unwrap_or(914_400);
    let a = prefix_of(src, gc);
    let at = if right {
        gc.range().end
    } else {
        gc.range().start
    };
    let mut e = vec![(at..at, format!("<{a}gridCol w=\"{w}\"/>"))];
    let mut bumped: Vec<(usize, usize)> = Vec::new();
    for r in 0..g.rows() {
        let crossing = k > 0 && k < nc && g.owner[r][k - 1] == g.owner[r][k];
        let cell = if crossing {
            let (or, oc) = g.owner[r][k];
            if !bumped.contains(&(or, oc)) {
                bumped.push((or, oc));
                let o = g.tcs[or][oc];
                let gs = span(o, "gridSpan") + 1;
                e.push(retag_edit(src, o, &[("gridSpan", Some(gs.to_string()))]));
            }
            retag(
                &blank_copy(src, g.tcs[r][k]),
                &[
                    ("gridSpan", None),
                    ("rowSpan", None),
                    ("hMerge", yes(true)),
                    ("vMerge", yes(r != or)),
                ],
            )
        } else {
            let (or, oc) = g.owner[r][col];
            let set: Vec<(&str, Option<String>)> = if oc == col {
                // A merge down this column's cells is copied.
                let rs = span(g.origin(r, col), "rowSpan");
                vec![
                    ("gridSpan", None),
                    ("rowSpan", (r == or && rs > 1).then(|| rs.to_string())),
                    ("hMerge", None),
                    ("vMerge", yes(r != or)),
                ]
            } else {
                NO_MERGE.to_vec()
            };
            retag(&blank_copy(src, g.tcs[r][col]), &set)
        };
        let at = if k < nc {
            g.tcs[r][k].range().start
        } else {
            g.tcs[r][nc - 1].range().end
        };
        e.push((at..at, cell));
    }
    e.extend(grow_frame(src, frame, w, 0));
    Ok(e)
}

fn delete_col(
    src: &str,
    t: Node,
    frame: Node,
    col: usize,
) -> anyhow::Result<Vec<(Range<usize>, String)>> {
    let cols = grid_cols(t)?;
    let nc = cols.len();
    if nc <= 1 {
        bail!("A table needs at least one column");
    }
    let g = Grid::new(t, nc)?;
    let gc = *cols.get(col).context("no such column")?;
    let w = attr_i64(gc, "w").unwrap_or(0);
    let mut e = vec![(gc.range(), String::new())];
    let mut done: Vec<(usize, usize)> = Vec::new();
    for r in 0..g.rows() {
        let (or, oc) = g.owner[r][col];
        if !done.contains(&(or, oc)) {
            done.push((or, oc));
            let o = g.tcs[or][oc];
            let (rs, gs) = (span(o, "rowSpan"), span(o, "gridSpan"));
            if oc < col {
                e.push(retag_edit(
                    src,
                    o,
                    &[("gridSpan", Some((gs - 1).to_string()))],
                ));
            } else if gs > 1 && col + 1 < nc {
                let next = g.tcs[or][col + 1];
                let moved = retag(
                    raw(src, o),
                    &[("gridSpan", Some((gs - 1).to_string())), ("hMerge", None)],
                );
                e.push((next.range(), moved));
                for row in g.tcs.iter().take((or + rs).min(g.rows())).skip(or + 1) {
                    e.push(retag_edit(src, row[col + 1], &[("hMerge", None)]));
                }
            }
        }
        e.push((g.tcs[r][col].range(), String::new()));
    }
    e.extend(grow_frame(src, frame, -w, 0));
    Ok(e)
}

fn apply_op(d: &mut Doc, op: &TableOp) -> anyhow::Result<()> {
    let doc = roxmltree::Document::parse(&d.xml).context("table XML")?;
    let frame = doc
        .root_element()
        .first_element_child()
        .context("no frame")?;
    let t = tbl(&doc)?;
    let trs = rows(t);
    let src = d.xml.as_str();
    let edits = match op {
        TableOp::Text { row, col, text } => {
            let tr = trs.get(*row).context("no such row")?;
            let tc = *cells(*tr).get(*col).context("no such cell")?;
            set_text(src, tc, text)
        }
        TableOp::Paras { row, col, paras } => {
            let tr = trs.get(*row).context("no such row")?;
            let tc = *cells(*tr).get(*col).context("no such cell")?;
            set_paras(src, tc, paras)
        }
        TableOp::Fill { row, col, color } => {
            let tr = trs.get(*row).context("no such row")?;
            let tc = *cells(*tr).get(*col).context("no such cell")?;
            set_fill(src, tc, *color)
        }
        TableOp::InsertRow { row, below } => insert_row(src, t, frame, *row, *below)?,
        TableOp::DeleteRow(row) => delete_row(src, t, frame, *row)?,
        TableOp::InsertCol { col, right } => insert_col(src, t, frame, *col, *right)?,
        TableOp::DeleteCol(col) => delete_col(src, t, frame, *col)?,
        TableOp::Resize { cx, cy } => {
            let cols = grid_cols(t)?;
            let ws: Vec<Emu> = cols
                .iter()
                .map(|c| attr_i64(*c, "w").unwrap_or(0))
                .collect();
            let hs: Vec<Emu> = trs.iter().map(|r| attr_i64(*r, "h").unwrap_or(0)).collect();
            let (nw, nh) = (share(&ws, *cx), share(&hs, *cy));
            let mut e = Vec::new();
            for (n, v) in cols.iter().zip(&nw) {
                if let Some(a) = n.attributes().find(|a| a.name() == "w") {
                    e.push((a.range_value(), v.to_string()));
                }
            }
            for (n, v) in trs.iter().zip(&nh) {
                if let Some(a) = n.attributes().find(|a| a.name() == "h") {
                    e.push((a.range_value(), v.to_string()));
                }
            }
            let sw: Emu = nw.iter().sum();
            let sh: Emu = nh.iter().sum();
            e.extend(set_frame_ext(frame, Some(sw), Some(sh)));
            e
        }
        TableOp::ColWidths(ws) => {
            let cols = grid_cols(t)?;
            if ws.len() != cols.len() {
                bail!("one width per column");
            }
            let mut e = Vec::new();
            for (n, v) in cols.iter().zip(ws) {
                if let Some(a) = n.attributes().find(|a| a.name() == "w") {
                    e.push((a.range_value(), (*v).max(1).to_string()));
                }
            }
            e.extend(set_frame_ext(
                frame,
                Some(ws.iter().map(|w| (*w).max(1)).sum()),
                None,
            ));
            e
        }
    };
    d.apply(edits);
    Ok(())
}

impl Presentation {
    /// The table a shape shows, if it's a table frame.
    pub fn table(&self, slide: usize, shape: usize) -> Option<&crate::frame::Table> {
        let s = self.slides.get(slide)?.shapes.get(shape)?;
        match &s.kind {
            ShapeKind::Other { frame, .. } => match frame.get()? {
                FrameContent::Table(t) => Some(t),
                _ => None,
            },
            _ => None,
        }
    }

    /// Each cell's plain text, row by row (`None` if not a table).
    pub fn table_text(&self, slide: usize, shape: usize) -> Option<Vec<Vec<String>>> {
        let t = self.table(slide, shape)?;
        Some(
            t.rows
                .iter()
                .map(|r| {
                    r.cells
                        .iter()
                        .map(|c| {
                            c.text
                                .as_ref()
                                .map(|b| b.plain_text().replace(docs_engine::LINE_BREAK, "\n"))
                                .unwrap_or_default()
                        })
                        .collect()
                })
                .collect(),
        )
    }

    /// Apply table edits to a table frame: one undo step. The frame's XML
    /// is spliced and read again; nothing else in it changes.
    pub fn edit_table(
        &mut self,
        slide: usize,
        shape: usize,
        ops: &[TableOp],
    ) -> anyhow::Result<()> {
        if ops.is_empty() {
            return Ok(());
        }
        let new = self.edited_table(slide, shape, ops, None)?;
        self.record(Kind::Other, None, Focus::shape(slide, shape), true, |p| {
            let s = p.slide_mut(slide)?;
            s.shapes[shape] = new;
            Ok(((), Focus::shape(slide, shape)))
        })
    }

    /// Typing in table cell `cell` of frame `r`: `f` edits a copy of its
    /// paragraphs, which are written into the table (coalesced as text
    /// edits in a shape are).
    pub(crate) fn edit_cell_text(
        &mut self,
        r: crate::edit::ShapeRef,
        (row, col): (usize, usize),
        kind: Kind,
        before: Focus,
        boundary: bool,
        f: impl FnOnce(&mut Vec<TextPara>) -> docs_engine::Selection,
    ) -> anyhow::Result<docs_engine::Selection> {
        let t = self.table(r.0, r.1).context("not a table")?;
        let cell = t.cell(row, col).context("no such cell")?;
        if cell.covered() {
            bail!("This cell is merged into another one");
        }
        let mut paras = cell
            .text
            .as_ref()
            .map(|b| b.paras.clone())
            .unwrap_or_else(|| vec![TextPara::new("")]);
        let sel = f(&mut paras);
        let new = self.edited_table(r.0, r.1, &[TableOp::Paras { row, col, paras }], None)?;
        self.record(kind, Some(r), before, boundary, |p| {
            let s = p.slide_mut(r.0)?;
            s.shapes[r.1] = new;
            Ok((sel, Focus::text_at(r, sel)))
        })
    }

    /// Resize a table frame to `xfrm` (its columns and rows scaled), as
    /// one undo step (`kind`).
    pub(crate) fn resize_table(
        &mut self,
        slide: usize,
        shape: usize,
        xfrm: Xfrm,
        kind: Kind,
    ) -> anyhow::Result<()> {
        let op = TableOp::Resize {
            cx: xfrm.cx.max(1),
            cy: xfrm.cy.max(1),
        };
        let new = self.edited_table(slide, shape, &[op], Some(xfrm))?;
        let target = Some(crate::edit::ShapeRef::from((slide, shape)));
        self.record(kind, target, Focus::shape(slide, shape), false, |p| {
            let s = p.slide_mut(slide)?;
            s.shapes[shape] = new;
            Ok(((), Focus::shape(slide, shape)))
        })
    }

    /// Table frame `shape` with `ops` applied: its XML as it would be
    /// saved (moves included, placed at `at`'s offset if given), spliced
    /// and read again.
    fn edited_table(
        &self,
        slide: usize,
        shape: usize,
        ops: &[TableOp],
        at: Option<Xfrm>,
    ) -> anyhow::Result<Shape> {
        self.table(slide, shape).context("not a table")?;
        let s = self.slides.get(slide).context("no such slide")?.clone();
        let mut old = s.shapes.get(shape).context("no such shape")?.clone();
        old.src.as_ref().context("a new table")?;
        if let (Some(at), Some(x)) = (at, old.xfrm.as_mut()) {
            x.x = at.x;
            x.y = at.y;
        }
        let mut written = String::new();
        crate::shape::write_shape(&mut written, &old);
        let ns = root_ns(&s.src.prefix);
        let mut d = Doc::new(ns, &written);
        for op in ops {
            apply_op(&mut d, op)?;
        }
        let master = self.master_of(slide);
        let colors = ColorCtx {
            theme: &master.theme,
            map: &master.clr_map,
            ph: None,
        };
        let doc = roxmltree::Document::parse(&d.xml).context("edited table XML")?;
        let el = doc
            .root_element()
            .first_element_child()
            .context("edited table")?;
        let cx = ParseCtx {
            src: &d.xml,
            part: &s.part,
            rels: &s.rels,
            colors,
        };
        Ok(parse_shape(el, &cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;

    fn frame(rows: &str) -> String {
        format!(
            r#"<p:graphicFrame {NS}><p:nvGraphicFramePr><p:cNvPr id="4" name="T"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="0"/><a:ext cx="2000" cy="600"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblGrid><a:gridCol w="1000"/><a:gridCol w="1000"/></a:tblGrid>{rows}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#
        )
    }

    fn cell(t: &str) -> String {
        format!(
            r#"<a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:pPr algn="ctr"/><a:r><a:rPr lang="en-US" b="1"/><a:t>{t}</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc>"#
        )
    }

    fn row(a: &str, b: &str) -> String {
        format!(r#"<a:tr h="300">{}{}</a:tr>"#, cell(a), cell(b))
    }

    fn run(xml: &str, ops: &[TableOp]) -> String {
        let mut d = Doc::new(Vec::new(), xml);
        for op in ops {
            apply_op(&mut d, op).unwrap();
        }
        d.raw().to_owned()
    }

    #[test]
    fn cell_text_keeps_formatting() {
        let x = frame(&(row("a", "b") + &row("c", "d")));
        let out = run(
            &x,
            &[TableOp::Text {
                row: 1,
                col: 0,
                text: "x < y\nz".into(),
            }],
        );
        assert!(out.contains(
            r#"<a:p><a:pPr algn="ctr"/><a:r><a:rPr lang="en-US" b="1"/><a:t>x &lt; y</a:t></a:r></a:p><a:p><a:pPr algn="ctr"/><a:r><a:rPr lang="en-US" b="1"/><a:t>z</a:t></a:r></a:p>"#
        ), "{out}");
        assert!(out.contains("<a:t>d</a:t>"));
    }

    #[test]
    fn rows_and_columns() {
        let x = frame(&(row("a", "b") + &row("c", "d")));
        let out = run(
            &x,
            &[TableOp::InsertRow {
                row: 0,
                below: true,
            }],
        );
        assert_eq!(out.matches("<a:tr ").count(), 3);
        assert!(out.contains(r#"cy="900""#), "frame grew: {out}");
        assert!(out.find("<a:t>a</a:t>") < out.find("<a:endParaRPr"));
        let out = run(&out, &[TableOp::DeleteRow(1)]);
        assert_eq!(out, x);
        let out = run(
            &x,
            &[TableOp::InsertCol {
                col: 1,
                right: true,
            }],
        );
        assert_eq!(out.matches("<a:gridCol").count(), 3);
        assert_eq!(out.matches("<a:tc>").count(), 6);
        assert!(out.contains(r#"cx="3000""#));
        let out = run(&out, &[TableOp::DeleteCol(2)]);
        assert_eq!(out, x);
    }

    #[test]
    fn fills() {
        let x = frame(&row("a", "b"));
        let out = run(
            &x,
            &[TableOp::Fill {
                row: 0,
                col: 1,
                color: Some(Rgb([0xFF, 0, 0])),
            }],
        );
        assert!(
            out.contains(
                r#"<a:tcPr><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:tcPr>"#
            )
        );
        let back = run(
            &out,
            &[TableOp::Fill {
                row: 0,
                col: 1,
                color: None,
            }],
        );
        assert!(back.contains("<a:tcPr></a:tcPr>") || back == x, "{back}");
    }

    /// A merged-table frame: rows of `[(text, attrs)]`, `n` columns of 1000.
    fn mframe(n: usize, rows: &[&[&str]]) -> String {
        let grid: String = (0..n).map(|_| r#"<a:gridCol w="1000"/>"#).collect();
        let body: String = rows
            .iter()
            .map(|r| {
                let cells: String = r
                    .iter()
                    .map(|c| {
                        let (t, attrs) = c.split_once('|').unwrap_or((c, ""));
                        let sp = if attrs.is_empty() { "" } else { " " };
                        format!(r#"<a:tc{sp}{attrs}><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>{t}</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc>"#)
                    })
                    .collect();
                format!(r#"<a:tr h="300">{cells}</a:tr>"#)
            })
            .collect();
        format!(
            r#"<p:graphicFrame {NS}><p:nvGraphicFramePr><p:cNvPr id="4" name="T"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="0"/><a:ext cx="{}" cy="{}"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblGrid>{grid}</a:tblGrid>{body}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#,
            n * 1000,
            rows.len() * 300
        )
    }

    /// Each row's cells as `text|attrs` (merge attributes only, in order).
    fn shape_of(xml: &str) -> Vec<Vec<String>> {
        let (w, _) = wrap(&[], xml);
        let doc = roxmltree::Document::parse(&w).unwrap();
        let t = tbl(&doc).unwrap();
        rows(t)
            .into_iter()
            .map(|tr| {
                cells(tr)
                    .into_iter()
                    .map(|tc| {
                        let text: String = tc
                            .descendants()
                            .filter(|n| is(*n, NS_A, "t"))
                            .filter_map(|n| n.text())
                            .collect();
                        let attrs: Vec<String> = ["gridSpan", "rowSpan", "hMerge", "vMerge"]
                            .iter()
                            .filter_map(|a| tc.attribute(*a).map(|v| format!("{a}={v}")))
                            .collect();
                        if attrs.is_empty() {
                            text
                        } else {
                            format!("{text}|{}", attrs.join(" "))
                        }
                    })
                    .collect()
            })
            .collect()
    }

    fn grid(v: &[&[&str]]) -> Vec<Vec<String>> {
        v.iter()
            .map(|r| r.iter().map(|c| (*c).to_owned()).collect())
            .collect()
    }

    #[test]
    fn merged_rows_grow_and_shrink_as_powerpoint() {
        // A spans two rows in column 0; B spans two columns in row 0.
        let x = mframe(
            3,
            &[
                &[r#"A|rowSpan="2""#, r#"B|gridSpan="2""#, r#"|hMerge="1""#],
                &[r#"|vMerge="1""#, "c", "d"],
                &["e", "f", "g"],
            ],
        );
        // A row inside A's span widens it; the new cell under it is
        // covered; B's merge across the row is copied.
        let out = run(
            &x,
            &[TableOp::InsertRow {
                row: 0,
                below: true,
            }],
        );
        assert_eq!(
            shape_of(&out),
            grid(&[
                &["A|rowSpan=3", "B|gridSpan=2", "|hMerge=1"],
                &["|vMerge=1", "|gridSpan=2", "|hMerge=1"],
                &["|vMerge=1", "c", "d"],
                &["e", "f", "g"],
            ])
        );
        assert!(out.contains(r#"cy="1200""#));
        // Above the first row: B's span across the row is copied.
        let out = run(
            &x,
            &[TableOp::InsertRow {
                row: 0,
                below: false,
            }],
        );
        assert_eq!(shape_of(&out)[0], vec!["", "|gridSpan=2", "|hMerge=1"]);
        // Removing A's first row: the cell below takes A's text and span.
        let out = run(&x, &[TableOp::DeleteRow(0)]);
        assert_eq!(shape_of(&out), grid(&[&["A", "c", "d"], &["e", "f", "g"]]));
        // Removing the row A covers shrinks it.
        let out = run(&x, &[TableOp::DeleteRow(1)]);
        assert_eq!(
            shape_of(&out),
            grid(&[&["A", "B|gridSpan=2", "|hMerge=1"], &["e", "f", "g"],])
        );
    }

    #[test]
    fn merged_columns_grow_and_shrink_as_powerpoint() {
        let x = mframe(
            3,
            &[
                &[r#"A|rowSpan="2""#, r#"B|gridSpan="2""#, r#"|hMerge="1""#],
                &[r#"|vMerge="1""#, "c", "d"],
            ],
        );
        // A column inside B widens it.
        let out = run(
            &x,
            &[TableOp::InsertCol {
                col: 1,
                right: true,
            }],
        );
        assert_eq!(
            shape_of(&out),
            grid(&[
                &["A|rowSpan=2", "B|gridSpan=3", "|hMerge=1", "|hMerge=1"],
                &["|vMerge=1", "c", "", "d"],
            ])
        );
        assert_eq!(out.matches("<a:gridCol").count(), 4);
        // Left of column 0: A's span down the column is copied.
        let out = run(
            &x,
            &[TableOp::InsertCol {
                col: 0,
                right: false,
            }],
        );
        assert_eq!(
            shape_of(&out),
            grid(&[
                &["|rowSpan=2", "A|rowSpan=2", "B|gridSpan=2", "|hMerge=1"],
                &["|vMerge=1", "|vMerge=1", "c", "d"],
            ])
        );
        // Removing B's first column hands B to the next one.
        let out = run(&x, &[TableOp::DeleteCol(1)]);
        assert_eq!(
            shape_of(&out),
            grid(&[&["A|rowSpan=2", "B"], &["|vMerge=1", "d"]])
        );
        // Removing B's covered column shrinks it.
        let out = run(&x, &[TableOp::DeleteCol(2)]);
        assert_eq!(
            shape_of(&out),
            grid(&[&["A|rowSpan=2", "B"], &["|vMerge=1", "c"]])
        );
        // Every result is a table PowerPoint's rules accept: spans
        // cover exactly the merged cells.
        for out in [
            run(
                &x,
                &[TableOp::InsertRow {
                    row: 1,
                    below: true,
                }],
            ),
            run(
                &x,
                &[TableOp::InsertCol {
                    col: 2,
                    right: true,
                }],
            ),
        ] {
            let (w, _) = wrap(&[], &out);
            let doc = roxmltree::Document::parse(&w).unwrap();
            let t = tbl(&doc).unwrap();
            let nc = grid_cols(t).unwrap().len();
            Grid::new(t, nc).unwrap();
        }
    }

    #[test]
    fn resize_scales_columns_and_rows() {
        let x = frame(&(row("a", "b") + &row("c", "d")));
        let out = run(&x, &[TableOp::Resize { cx: 3000, cy: 900 }]);
        assert_eq!(out.matches(r#"<a:gridCol w="1500"/>"#).count(), 2);
        assert_eq!(out.matches(r#"<a:tr h="450">"#).count(), 2);
        assert!(out.contains(r#"<a:ext cx="3000" cy="900"/>"#), "{out}");
        let out = run(&x, &[TableOp::ColWidths(vec![500, 2500])]);
        assert!(out.contains(r#"<a:gridCol w="500"/><a:gridCol w="2500"/>"#));
        assert!(out.contains(r#"cx="3000""#));
        assert_eq!(share(&[1, 1, 1], 10), vec![3, 3, 4]);
    }

    #[test]
    fn cell_paragraphs_keep_their_formatting() {
        let x = frame(&row("a", "b"));
        let mut p = TextPara::new("bold");
        p.runs[0].props.bold = Some(true);
        let out = run(
            &x,
            &[TableOp::Paras {
                row: 0,
                col: 1,
                paras: vec![p, TextPara::new("")],
            }],
        );
        assert!(
            out.contains(r#"b="1""#) && out.contains("<a:t>bold</a:t>"),
            "{out}"
        );
        assert!(out.contains("<a:t>a</a:t>"));
        assert!(!out.contains("<a:t>b</a:t>"));
    }
}
