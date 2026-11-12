//! Editing tables (`a:tbl` in a `p:graphicFrame`): cell text, rows and
//! columns, cell fill.
//!
//! Tables are kept as their XML ([`crate::frame`] reads them for display).
//! An edit splices that XML: a cell's paragraphs are rewritten in the
//! formatting of its first run and paragraph, a new row or column copies
//! its neighbour (formatting, height or width) with empty text, a fill
//! replaces the cell's own fill in `a:tcPr`. The frame grows or shrinks by
//! the row or column, then the shape is read again from the new XML, so
//! everything Kantor doesn't touch stays byte for byte. Rows and columns
//! can't be added or removed in tables with merged cells (Kantor says so).

use std::ops::Range;
use std::sync::Arc;

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

/// Whether any cell spans or is covered (rows and columns can't be added
/// or removed around merges).
fn has_merges(t: Node) -> bool {
    t.descendants().filter(|n| is(*n, NS_A, "tc")).any(|tc| {
        ["gridSpan", "rowSpan"]
            .iter()
            .any(|a| tc.attribute(*a).is_some_and(|v| v != "1"))
            || ["hMerge", "vMerge"]
                .iter()
                .any(|a| tc.attribute(*a).is_some_and(|v| v == "1" || v == "true"))
    })
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
        TableOp::Fill { row, col, color } => {
            let tr = trs.get(*row).context("no such row")?;
            let tc = *cells(*tr).get(*col).context("no such cell")?;
            set_fill(src, tc, *color)
        }
        TableOp::InsertRow { row, below } => {
            if has_merges(t) {
                bail!("Rows can't be added to a table with merged cells yet");
            }
            let tr = *trs.get(*row).context("no such row")?;
            let copy = blank_copy(src, tr);
            let at = if *below {
                tr.range().end
            } else {
                tr.range().start
            };
            let h = attr_i64(tr, "h").unwrap_or(370_840);
            let mut e = vec![(at..at, copy)];
            e.extend(grow_frame(src, frame, 0, h));
            e
        }
        TableOp::DeleteRow(row) => {
            if has_merges(t) {
                bail!("Rows can't be removed from a table with merged cells yet");
            }
            if trs.len() <= 1 {
                bail!("A table needs at least one row");
            }
            let tr = *trs.get(*row).context("no such row")?;
            let h = attr_i64(tr, "h").unwrap_or(0);
            let mut e = vec![(tr.range(), String::new())];
            e.extend(grow_frame(src, frame, 0, -h));
            e
        }
        TableOp::InsertCol { col, right } => {
            if has_merges(t) {
                bail!("Columns can't be added to a table with merged cells yet");
            }
            let grid = child(t, NS_A, "tblGrid").context("no column grid")?;
            let cols: Vec<Node> = grid
                .children()
                .filter(|c| is(*c, NS_A, "gridCol"))
                .collect();
            let gc = *cols.get(*col).context("no such column")?;
            let w = attr_i64(gc, "w").unwrap_or(914_400);
            let a = prefix_of(src, gc);
            let place = |n: Node| {
                if *right {
                    n.range().end
                } else {
                    n.range().start
                }
            };
            let at = place(gc);
            let mut e = vec![(at..at, format!("<{a}gridCol w=\"{w}\"/>"))];
            for tr in &trs {
                let tc = *cells(*tr).get(*col).context("a row is missing a cell")?;
                let at = place(tc);
                e.push((at..at, blank_copy(src, tc)));
            }
            e.extend(grow_frame(src, frame, w, 0));
            e
        }
        TableOp::DeleteCol(col) => {
            if has_merges(t) {
                bail!("Columns can't be removed from a table with merged cells yet");
            }
            let grid = child(t, NS_A, "tblGrid").context("no column grid")?;
            let cols: Vec<Node> = grid
                .children()
                .filter(|c| is(*c, NS_A, "gridCol"))
                .collect();
            if cols.len() <= 1 {
                bail!("A table needs at least one column");
            }
            let gc = *cols.get(*col).context("no such column")?;
            let w = attr_i64(gc, "w").unwrap_or(0);
            let mut e = vec![(gc.range(), String::new())];
            for tr in &trs {
                if let Some(tc) = cells(*tr).get(*col) {
                    e.push((tc.range(), String::new()));
                }
            }
            e.extend(grow_frame(src, frame, -w, 0));
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
        self.table(slide, shape).context("not a table")?;
        let s = self.slides.get(slide).context("no such slide")?.clone();
        let old = s.shapes.get(shape).context("no such shape")?.clone();
        let src = old.src.as_ref().context("a new table")?;
        let ns = root_ns(&s.src.prefix);
        let mut d = Doc::new(ns, &src.raw);
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
        let mut new = parse_shape(el, &cx);
        // Moves made to the frame in Kantor stay.
        if old.xfrm != src.xfrm {
            new.xfrm = old.xfrm;
        }
        let new = Arc::new(new);
        self.record(Kind::Other, None, Focus::shape(slide, shape), true, |p| {
            let s = p.slide_mut(slide)?;
            s.shapes[shape] = (*new).clone();
            Ok(((), Focus::shape(slide, shape)))
        })
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

    #[test]
    fn merged_tables_refuse_structure_edits() {
        let merged = r#"<a:tr h="300"><a:tc gridSpan="2"><a:txBody><a:bodyPr/><a:p/></a:txBody></a:tc><a:tc hMerge="1"><a:txBody><a:bodyPr/><a:p/></a:txBody></a:tc></a:tr>"#;
        let x = frame(merged);
        let mut d = Doc::new(Vec::new(), &x);
        assert!(
            apply_op(
                &mut d,
                &TableOp::InsertRow {
                    row: 0,
                    below: true
                }
            )
            .is_err()
        );
        assert!(
            apply_op(
                &mut d,
                &TableOp::Text {
                    row: 0,
                    col: 0,
                    text: "ok".into()
                }
            )
            .is_ok()
        );
    }
}
