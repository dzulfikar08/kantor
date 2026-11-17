//! The data of the Office 2016 chart types (`cx:chartSpace`: waterfall,
//! funnel, treemap, sunburst, histogram, Pareto, box & whisker), edited as
//! a classic chart's is (see the parent module).
//!
//! A chartex part keeps its data apart from its series: `cx:chartData`
//! holds one `cx:data` block per series (`cx:series/cx:dataId` names it),
//! each with a category dimension (`cx:strDim type="cat"`, one `cx:lvl` per
//! hierarchy level, innermost first) and a value dimension (`cx:numDim
//! type="val"`). Their references (`cx:f`) name cells of the embedded
//! workbook directly or through a defined name (`_xlchart.v1.0`, as Office
//! writes them). Series without data of their own (a Pareto line) aren't
//! listed.
//!
//! Treemap and sunburst categories are paths, outermost level first,
//! joined with [`SEP`] (`Europe / North / Alpha`); trailing empty levels
//! are left out.
//!
//! Series are added, removed and moved ([`restructure`]) on the types that
//! plot several (box & whisker), as PowerPoint's *Select Data* does: a new
//! series gets its own data block (the categories of the series it copies,
//! values in the workbook's next column or row) and takes its place in the
//! plot order; a removed one takes its data block (and cells) with it.

use std::collections::HashMap;
use std::ops::Range;

use anyhow::{Context as _, bail};
use roxmltree::{Document, Node};

use super::{CellValue, CellWrites, ChartData, ChartSeries, Ref, SeriesChange, fmt_num};
use crate::frame::NS_CX;
use crate::xml::*;

/// Between the levels of a hierarchical category.
pub const SEP: &str = " / ";

/// Whether `xml` is a chartex part.
pub(super) fn is_chartex(xml: &str) -> bool {
    let head = &xml[..xml.len().min(600)];
    head.contains("chartSpace") && head.contains(NS_CX)
}

fn cx<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    child(n, NS_CX, name)
}

/// The series that have data of their own, with their `cx:data` block.
fn data_series<'a, 'i>(doc: &'a Document<'i>) -> Vec<(Node<'a, 'i>, Node<'a, 'i>)> {
    let blocks: Vec<Node> = doc
        .descendants()
        .filter(|n| is(*n, NS_CX, "data"))
        .collect();
    doc.descendants()
        .filter(|n| is(*n, NS_CX, "series"))
        .filter_map(|s| {
            let id = cx(s, "dataId")?.attribute("val")?;
            let d = blocks.iter().find(|d| d.attribute("id") == Some(id))?;
            Some((s, *d))
        })
        .collect()
}

/// A block's category (`cat`) or value (`val`) dimension.
fn dim<'a, 'i>(data: Node<'a, 'i>, kind: &str) -> Option<Node<'a, 'i>> {
    data.children().find(|c| {
        (is(*c, NS_CX, "strDim") || is(*c, NS_CX, "numDim")) && c.attribute("type") == Some(kind)
    })
}

fn levels<'a, 'i>(dim: Node<'a, 'i>) -> Vec<Node<'a, 'i>> {
    dim.children().filter(|c| is(*c, NS_CX, "lvl")).collect()
}

fn points(lvl: Node) -> Vec<Option<String>> {
    let n = attr_i64(lvl, "ptCount").unwrap_or(0).max(0) as usize;
    let mut out = vec![None; n];
    for pt in lvl.children().filter(|c| is(*c, NS_CX, "pt")) {
        let Some(i) = attr_i64(pt, "idx").map(|i| i as usize) else {
            continue;
        };
        if i >= out.len() {
            out.resize(i + 1, None);
        }
        out[i] = Some(pt.text().unwrap_or_default().to_owned());
    }
    out
}

/// A dimension's or series name's reference text (`cx:f`).
fn formula<'a, 'i>(n: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    cx(n, "f")
}

/// A hierarchical category's path (levels innermost first).
fn path(levels: &[Vec<Option<String>>], i: usize) -> String {
    let mut parts: Vec<String> = levels
        .iter()
        .rev()
        .map(|l| l.get(i).cloned().flatten().unwrap_or_default())
        .collect();
    while parts.len() > 1 && parts.last().is_some_and(|p| p.is_empty()) {
        parts.pop();
    }
    parts.join(SEP)
}

/// A path split into `k` levels, innermost first (extra parts stay in the
/// innermost one, missing ones are empty).
fn split_path(s: &str, k: usize) -> Vec<String> {
    if k <= 1 {
        return vec![s.to_owned()];
    }
    let parts: Vec<&str> = s.split(SEP).collect();
    let mut outer: Vec<String> = Vec::with_capacity(k);
    for i in 0..k {
        let p = if i + 1 == k {
            parts.get(i..).map(|r| r.join(SEP)).unwrap_or_default()
        } else {
            parts.get(i).map(|p| p.to_string()).unwrap_or_default()
        };
        outer.push(p);
    }
    outer.reverse();
    outer
}

/// The data a chartex part shows.
pub(super) fn read(xml: &str) -> Option<ChartData> {
    let doc = Document::parse(xml).ok()?;
    let pairs = data_series(&doc);
    let (_, first) = *pairs.first()?;
    let cats: Vec<Vec<Option<String>>> = dim(first, "cat")
        .map(|d| levels(d).into_iter().map(points).collect())
        .unwrap_or_default();
    let ncat = cats.iter().map(Vec::len).max().unwrap_or(0);
    let categories: Vec<String> = (0..ncat).map(|i| path(&cats, i)).collect();
    let series: Vec<ChartSeries> = pairs
        .iter()
        .map(|(s, d)| {
            let name = cx(*s, "tx")
                .and_then(|t| cx(t, "txData"))
                .and_then(|t| cx(t, "v"))
                .and_then(|v| v.text())
                .unwrap_or_default()
                .to_owned();
            let values = dim(*d, "val")
                .and_then(|v| levels(v).into_iter().next())
                .map(|l| {
                    points(l)
                        .into_iter()
                        .map(|v| v.and_then(|v| v.trim().parse().ok()))
                        .collect()
                })
                .unwrap_or_default();
            ChartSeries { name, values }
        })
        .collect();
    let n = series
        .iter()
        .map(|s| s.values.len())
        .chain([categories.len()])
        .max()
        .unwrap_or(0);
    let mut categories = categories;
    categories.resize(n, String::new());
    let series = series
        .into_iter()
        .map(|mut s| {
            s.values.resize(n, None);
            s
        })
        .collect();
    Some(ChartData { categories, series })
}

/// `lvl` with `values` in place of its points (its other attributes kept).
fn new_lvl(src: &str, lvl: Node, values: &[Option<String>]) -> String {
    let p = super::prefix_of(src, lvl);
    let mut out = format!("<{p}lvl ptCount=\"{}\"", values.len());
    write_attrs(&mut out, &attrs_except(lvl, src, &["ptCount"]));
    out.push('>');
    for (i, v) in values.iter().enumerate() {
        if let Some(v) = v {
            out.push_str(&format!("<{p}pt idx=\"{i}\">{}</{p}pt>", esc(v)));
        }
    }
    out.push_str(&format!("</{p}lvl>"));
    out
}

/// A reference as the part writes it: cells, or a defined name of the
/// workbook standing for them.
struct At {
    cells: Ref,
    /// The defined name (`_xlchart.v1.0`) the reference is, if it is one.
    name: Option<String>,
}

fn resolve(text: &str, names: &HashMap<String, String>) -> Option<At> {
    let text = text.trim();
    if let Some(r) = Ref::parse(text) {
        return Some(At {
            cells: r,
            name: None,
        });
    }
    let target = names
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(text))
        .map(|(_, v)| v)?;
    Some(At {
        cells: Ref::parse(target.trim_start_matches('='))?,
        name: Some(text.to_owned()),
    })
}

/// `r` resized to `n` categories: a block of `k` level columns grows down;
/// a single row or column along itself.
fn resized(r: &Ref, n: u32, k: usize) -> Ref {
    if k > 1 && (r.c1 - r.c0 + 1) as usize == k {
        let mut out = r.clone();
        out.r1 = r.r0 + n.max(1) - 1;
        return out;
    }
    r.resized(n)
}

/// The edits of a chartex rewrite: the part's text edits, the workbook
/// cells and the defined names whose range changed (`name`, new range).
pub(super) type ExEdits = (String, Vec<CellWrites>, Vec<(String, String)>);

/// The chartex part with `data` in its caches; the workbook cells it
/// references with their new values; defined names to point at resized
/// ranges.
pub(super) fn rewrite(
    xml: &str,
    data: &ChartData,
    names: &HashMap<String, String>,
) -> anyhow::Result<ExEdits> {
    let doc = Document::parse(xml).context("chart XML")?;
    let pairs = data_series(&doc);
    if pairs.len() != data.series.len() {
        bail!(
            "Kantor can't add, remove or move the series of this chart (it has {})",
            pairs.len()
        );
    }
    let n = data.categories.len();
    if n == 0 {
        bail!("A chart needs at least one category");
    }
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut cells: Vec<CellWrites> = Vec::new();
    let mut stale: Vec<CellWrites> = Vec::new();
    let mut renames: Vec<(String, String)> = Vec::new();
    let mut done: Vec<usize> = Vec::new();
    // A reference resized to `new`: its text (or its defined name)
    // changes, and cells it no longer covers are emptied.
    let mut moved = |f: Node,
                     at: &At,
                     new: &Ref,
                     edits: &mut Vec<(Range<usize>, String)>,
                     stale: &mut Vec<CellWrites>| {
        if *new == at.cells {
            return;
        }
        match &at.name {
            Some(name) => {
                if !renames.iter().any(|(n, _)| n == name) {
                    renames.push((name.clone(), new.write()));
                }
            }
            None => {
                if let Some(t) = f.first_child().filter(|t| t.is_text()) {
                    edits.push((t.range(), esc(&new.write())));
                }
            }
        }
        let keep = new.cells();
        let gone: Vec<(u32, u32)> = at
            .cells
            .cells()
            .into_iter()
            .filter(|c| !keep.contains(c))
            .collect();
        for (c, r) in gone {
            let one = Ref {
                sheet: at.cells.sheet.clone(),
                c0: c,
                r0: r,
                c1: c,
                r1: r,
            };
            stale.push((one, vec![CellValue::Empty]));
        }
    };
    for ((s, d), sd) in pairs.iter().zip(&data.series) {
        // The series' name.
        if let Some(td) = cx(*s, "tx").and_then(|t| cx(t, "txData")) {
            if let Some(v) = cx(td, "v") {
                match v.first_child().filter(|t| t.is_text()) {
                    Some(t) => edits.push((t.range(), esc(&sd.name))),
                    None => {
                        let p = super::prefix_of(xml, v);
                        edits.push((v.range(), format!("<{p}v>{}</{p}v>", esc(&sd.name))));
                    }
                }
            }
            if let Some(at) = formula(td)
                .and_then(|f| f.text())
                .and_then(|t| resolve(t, names))
            {
                cells.push((at.cells, vec![CellValue::Text(sd.name.clone())]));
            }
        }
        // A block two series share is written once.
        if done.contains(&d.range().start) {
            continue;
        }
        done.push(d.range().start);
        if let Some(cd) = dim(*d, "cat") {
            let lv = levels(cd);
            let k = lv.len().max(1);
            let numeric = is(cd, NS_CX, "numDim");
            let split: Vec<Vec<String>> =
                data.categories.iter().map(|c| split_path(c, k)).collect();
            for (l, lvl) in lv.iter().enumerate() {
                let vals: Vec<Option<String>> = split
                    .iter()
                    .map(|parts| {
                        let v = &parts[l];
                        if numeric {
                            v.trim().parse::<f64>().ok().map(fmt_num)
                        } else {
                            Some(v.clone())
                        }
                    })
                    .collect();
                edits.push((lvl.range(), new_lvl(xml, *lvl, &vals)));
            }
            if let Some(f) = formula(cd)
                && let Some(at) = f.text().and_then(|t| resolve(t, names))
            {
                let new = resized(&at.cells, n as u32, k);
                moved(f, &at, &new, &mut edits, &mut stale);
                let by_levels = k > 1 && (new.c1 - new.c0 + 1) as usize == k;
                let mut vals = Vec::new();
                for (c, r) in new.cells() {
                    let (i, l) = if by_levels {
                        ((r - new.r0) as usize, (new.c1 - c) as usize)
                    } else if new.c0 == new.c1 {
                        ((r - new.r0) as usize, 0)
                    } else {
                        ((c - new.c0) as usize, 0)
                    };
                    let v = split
                        .get(i)
                        .and_then(|p| p.get(l))
                        .cloned()
                        .unwrap_or_default();
                    vals.push(match v.trim().parse::<f64>() {
                        Ok(x) if numeric => CellValue::Number(x),
                        _ if v.is_empty() => CellValue::Empty,
                        _ => CellValue::Text(v),
                    });
                }
                cells.push((new, vals));
            }
        }
        if let Some(vd) = dim(*d, "val") {
            let mut values = sd.values.clone();
            values.resize(n, None);
            if let Some(lvl) = levels(vd).first() {
                let vals: Vec<Option<String>> = values.iter().map(|v| v.map(fmt_num)).collect();
                edits.push((lvl.range(), new_lvl(xml, *lvl, &vals)));
            }
            if let Some(f) = formula(vd)
                && let Some(at) = f.text().and_then(|t| resolve(t, names))
            {
                let new = at.cells.resized(n as u32);
                moved(f, &at, &new, &mut edits, &mut stale);
                cells.push((
                    new,
                    values
                        .iter()
                        .map(|v| v.map_or(CellValue::Empty, CellValue::Number))
                        .collect(),
                ));
            }
        }
    }
    stale.append(&mut cells);
    edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
    let mut out = xml.to_owned();
    for (r, t) in edits {
        out.replace_range(r, &t);
    }
    Ok((out, stale, renames))
}

/// Whether a chartex part's series can't be added, removed or moved (its
/// type plots one series).
pub(super) fn series_fixed(xml: &str) -> bool {
    Document::parse(xml).is_ok_and(|d| !many_series(&d))
}

/// Whether a chartex part's type plots several series (box & whisker);
/// the others plot one.
fn many_series(doc: &Document) -> bool {
    doc.descendants()
        .filter(|n| is(*n, NS_CX, "series"))
        .any(|s| s.attribute("layoutId") == Some("boxWhisker"))
}

/// A chartex part with a series added, removed or moved (`change`); the
/// workbook cells to empty. The new series' data and name are written by
/// [`rewrite`] next.
pub(super) fn restructure(
    xml: &str,
    change: SeriesChange,
    names: &HashMap<String, String>,
) -> anyhow::Result<(String, Vec<CellWrites>)> {
    let doc = Document::parse(xml).context("chart XML")?;
    let pairs = data_series(&doc);
    let mut clears: Vec<CellWrites> = Vec::new();
    if change != SeriesChange::Same && !many_series(&doc) {
        bail!("This type of chart shows one series");
    }
    let n = pairs.len();
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    match change {
        SeriesChange::Same => return Ok((xml.to_owned(), clears)),
        SeriesChange::Inserted(k) => {
            if n == 0 {
                bail!("the chart has no series");
            }
            let k = k.min(n);
            let (template, block) = pairs[k.min(n - 1)];
            let refs = |d: Node| {
                dim(d, "val")
                    .and_then(formula)
                    .and_then(|f| f.text())
                    .and_then(|t| resolve(t, names))
            };
            // The new values go beside the last series' (columns move
            // right, rows down).
            let last = pairs
                .iter()
                .filter_map(|(s, d)| Some((*s, refs(*d)?.cells)))
                .max_by_key(|(_, r)| (r.c0, r.r0));
            let (dc, dr) = match &last {
                Some((_, v)) if v.r0 == v.r1 && v.c0 != v.c1 => (0, 1),
                _ => (1, 0),
            };
            let new_val = last.as_ref().map(|(_, v)| v.shifted(dc, dr));
            let new_name = last.as_ref().and_then(|(s, _)| {
                cx(*s, "tx")
                    .and_then(|t| cx(t, "txData"))
                    .and_then(formula)
                    .and_then(|f| f.text())
                    .and_then(|t| resolve(t, names))
                    .map(|a| a.cells.shifted(dc, dr))
            });
            let id = doc
                .descendants()
                .filter(|n| is(*n, NS_CX, "data"))
                .filter_map(|d| d.attribute("id")?.parse::<u32>().ok())
                .max()
                .map_or(0, |m| m + 1);
            // The data block: the template's categories, its own values.
            let mut data = raw(xml, block).to_owned();
            let base = block.range().start;
            let mut local: Vec<(Range<usize>, String)> = Vec::new();
            if let Some(a) = block.attributes().find(|a| a.name() == "id") {
                let r = a.range_value();
                local.push((r.start - base..r.end - base, id.to_string()));
            }
            if let Some(vd) = dim(block, "val") {
                if let (Some(t), Some(v)) = (
                    formula(vd)
                        .and_then(|f| f.first_child())
                        .filter(|t| t.is_text()),
                    &new_val,
                ) {
                    let r = t.range();
                    local.push((r.start - base..r.end - base, esc(&v.write())));
                }
                if let Some(l) = levels(vd).first() {
                    let r = l.range();
                    local.push((r.start - base..r.end - base, new_lvl(xml, *l, &[])));
                }
            }
            local.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
            for (r, t) in local {
                data.replace_range(r, &t);
            }
            let after = doc
                .descendants()
                .rfind(|n| is(*n, NS_CX, "data"))
                .map_or(block.range().end, |d| d.range().end);
            edits.push((after..after, data));
            // The series: the template's look, no ID of its own yet.
            let mut ser = raw(xml, template).to_owned();
            let base = template.range().start;
            let mut local: Vec<(Range<usize>, String)> = Vec::new();
            if let Some(a) = template.attributes().find(|a| a.name() == "uniqueId") {
                // ` uniqueId="{...}"`: from the space before it.
                let r = a.range();
                let start = xml[..r.start].trim_end().len();
                local.push((start - base..r.end - base, String::new()));
            }
            if let Some(a) = template.attributes().find(|a| a.name() == "formatIdx") {
                let next = doc
                    .descendants()
                    .filter(|n| is(*n, NS_CX, "series"))
                    .filter_map(|s| s.attribute("formatIdx")?.parse::<u32>().ok())
                    .max()
                    .map_or(0, |m| m + 1);
                let r = a.range_value();
                local.push((r.start - base..r.end - base, next.to_string()));
            }
            if let Some(d) = cx(template, "dataId")
                && let Some(a) = d.attributes().find(|a| a.name() == "val")
            {
                let r = a.range_value();
                local.push((r.start - base..r.end - base, id.to_string()));
            }
            if let Some(td) = cx(template, "tx").and_then(|t| cx(t, "txData"))
                && let Some(t) = formula(td)
                    .and_then(|f| f.first_child())
                    .filter(|t| t.is_text())
                && let Some(r) = &new_name
            {
                let rr = t.range();
                local.push((rr.start - base..rr.end - base, esc(&r.write())));
            }
            for c in template
                .children()
                .filter(|c| is(*c, NS_CX, "dataPt") || is(*c, NS_CX, "extLst"))
            {
                let r = c.range();
                local.push((r.start - base..r.end - base, String::new()));
            }
            local.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
            for (r, t) in local {
                ser.replace_range(r, &t);
            }
            let at = if k == n {
                pairs[n - 1].0.range().end
            } else {
                template.range().start
            };
            edits.push((at..at, ser));
        }
        SeriesChange::Removed(k) => {
            if n <= 1 {
                bail!("A chart needs at least one series");
            }
            let (gone, block) = *pairs.get(k).context("no such series")?;
            edits.push((gone.range(), String::new()));
            let shared = pairs
                .iter()
                .enumerate()
                .any(|(i, (_, d))| i != k && d.range() == block.range());
            if !shared {
                edits.push((block.range(), String::new()));
                let mut refs: Vec<Ref> = Vec::new();
                if let Some(a) = dim(block, "val")
                    .and_then(formula)
                    .and_then(|f| f.text())
                    .and_then(|t| resolve(t, names))
                {
                    refs.push(a.cells);
                }
                if let Some(a) = cx(gone, "tx")
                    .and_then(|t| cx(t, "txData"))
                    .and_then(formula)
                    .and_then(|f| f.text())
                    .and_then(|t| resolve(t, names))
                {
                    refs.push(a.cells);
                }
                for r in refs {
                    clears.push((r.clone(), vec![CellValue::Empty; r.cells().len()]));
                }
            }
        }
        SeriesChange::Moved { from, to } => {
            let (a, b) = (
                pairs.get(from).context("no such series")?.0,
                pairs.get(to).context("no such series")?.0,
            );
            if from == to {
                return Ok((xml.to_owned(), clears));
            }
            edits.push((a.range(), raw(xml, b).to_owned()));
            edits.push((b.range(), raw(xml, a).to_owned()));
        }
    }
    edits.sort_by_key(|(r, _)| std::cmp::Reverse((r.start, r.end)));
    let mut out = xml.to_owned();
    for (r, t) in edits {
        out.replace_range(r, &t);
    }
    Ok((out, clears))
}

/// The block every reference of a chartex part lies in (one sheet).
pub(super) fn data_box(xml: &str, names: &HashMap<String, String>) -> Option<Ref> {
    let doc = Document::parse(xml).ok()?;
    let mut out: Option<Ref> = None;
    for f in doc.descendants().filter(|n| is(*n, NS_CX, "f")) {
        let Some(at) = f.text().and_then(|t| resolve(t, names)) else {
            continue;
        };
        out = Some(match out {
            None => at.cells,
            Some(b) => b.union(&at.cells)?,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TREE: &str = r#"<cx:chartSpace xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex"><cx:chartData><cx:data id="0"><cx:strDim type="cat"><cx:f>Data!$A$2:$B$4</cx:f><cx:lvl ptCount="3"><cx:pt idx="0">Alpha</cx:pt><cx:pt idx="1">Beta</cx:pt><cx:pt idx="2">Gamma</cx:pt></cx:lvl><cx:lvl ptCount="3"><cx:pt idx="0">North</cx:pt><cx:pt idx="1">North</cx:pt><cx:pt idx="2">South</cx:pt></cx:lvl></cx:strDim><cx:numDim type="val"><cx:f>_xlchart.v1.0</cx:f><cx:lvl ptCount="3" formatCode="General"><cx:pt idx="0">30</cx:pt><cx:pt idx="1">20</cx:pt><cx:pt idx="2">25</cx:pt></cx:lvl></cx:numDim></cx:data></cx:chartData><cx:chart><cx:plotArea><cx:plotAreaRegion><cx:series layoutId="treemap"><cx:tx><cx:txData><cx:f>Data!$C$1</cx:f><cx:v>Sales</cx:v></cx:txData></cx:tx><cx:dataId val="0"/></cx:series></cx:plotAreaRegion></cx:plotArea></cx:chart></cx:chartSpace>"#;

    #[test]
    fn hierarchical_categories_are_paths() {
        assert!(is_chartex(TREE));
        let d = read(TREE).unwrap();
        assert_eq!(
            d.categories,
            ["North / Alpha", "North / Beta", "South / Gamma"]
        );
        assert_eq!(d.series[0].name, "Sales");
        assert_eq!(d.series[0].values, [Some(30.0), Some(20.0), Some(25.0)]);
        assert_eq!(split_path("A / B / C", 2), ["B / C", "A"]);
        assert_eq!(split_path("A", 3), ["", "", "A"]);
        assert_eq!(path(&[vec![None], vec![Some("Asia".into())]], 0), "Asia");
    }

    #[test]
    fn a_rewrite_updates_caches_cells_and_names() {
        let mut d = read(TREE).unwrap();
        d.categories.push("West / Zeta".into());
        d.series[0].values = vec![Some(1.0), Some(2.0), Some(3.0), Some(4.5)];
        d.series[0].name = "Revenue".into();
        let names = HashMap::from([("_xlchart.v1.0".to_owned(), "Data!$C$2:$C$4".to_owned())]);
        let (xml, cells, renames) = rewrite(TREE, &d, &names).unwrap();
        assert_eq!(read(&xml).unwrap(), d);
        assert!(xml.contains("<cx:f>Data!$A$2:$B$5</cx:f>"), "{xml}");
        assert!(xml.contains("<cx:f>_xlchart.v1.0</cx:f>"), "the name stays");
        assert!(
            xml.contains("<cx:lvl ptCount=\"4\" formatCode=\"General\">"),
            "{xml}"
        );
        assert_eq!(
            renames,
            [("_xlchart.v1.0".to_owned(), "Data!$C$2:$C$5".to_owned())]
        );
        // Name, the category block (rows of outer, inner), the values.
        let cat = cells
            .iter()
            .find(|(r, _)| r.write() == "Data!$A$2:$B$5")
            .unwrap();
        assert_eq!(cat.1[6], CellValue::Text("West".into()));
        assert_eq!(cat.1[7], CellValue::Text("Zeta".into()));
        assert!(cells.iter().any(|(r, v)| r.write() == "Data!$C$1"
            && v == &[CellValue::Text("Revenue".into())]));
        let val = cells
            .iter()
            .find(|(r, _)| r.write() == "Data!$C$2:$C$5")
            .unwrap();
        assert_eq!(val.1[3], CellValue::Number(4.5));
        // Fewer categories: the cells left over are emptied.
        let mut short = read(TREE).unwrap();
        short.categories.truncate(2);
        short.series[0].values.truncate(2);
        let (_, cells, _) = rewrite(TREE, &short, &names).unwrap();
        assert!(
            cells
                .iter()
                .any(|(r, v)| r.write() == "Data!$C$4" && v == &[CellValue::Empty])
        );
        // Series can't be added.
        let mut two = read(TREE).unwrap();
        two.series.push(two.series[0].clone());
        assert!(rewrite(TREE, &two, &names).is_err());
    }

    const BOX: &str = r#"<cx:chartSpace xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex"><cx:chartData><cx:data id="0"><cx:strDim type="cat"><cx:f>Data!$A$2:$A$3</cx:f><cx:lvl ptCount="2"><cx:pt idx="0">Mon</cx:pt><cx:pt idx="1">Tue</cx:pt></cx:lvl></cx:strDim><cx:numDim type="val"><cx:f>Data!$B$2:$B$3</cx:f><cx:lvl ptCount="2" formatCode="General"><cx:pt idx="0">1</cx:pt><cx:pt idx="1">2</cx:pt></cx:lvl></cx:numDim></cx:data><cx:data id="1"><cx:strDim type="cat"><cx:f>Data!$A$2:$A$3</cx:f><cx:lvl ptCount="2"><cx:pt idx="0">Mon</cx:pt><cx:pt idx="1">Tue</cx:pt></cx:lvl></cx:strDim><cx:numDim type="val"><cx:f>Data!$C$2:$C$3</cx:f><cx:lvl ptCount="2" formatCode="General"><cx:pt idx="0">3</cx:pt><cx:pt idx="1">4</cx:pt></cx:lvl></cx:numDim></cx:data></cx:chartData><cx:chart><cx:plotArea><cx:plotAreaRegion><cx:series layoutId="boxWhisker" uniqueId="{A}" formatIdx="0"><cx:tx><cx:txData><cx:f>Data!$B$1</cx:f><cx:v>Morning</cx:v></cx:txData></cx:tx><cx:dataId val="0"/><cx:layoutPr><cx:statistics quartileMethod="exclusive"/></cx:layoutPr></cx:series><cx:series layoutId="boxWhisker" uniqueId="{B}" formatIdx="1"><cx:tx><cx:txData><cx:f>Data!$C$1</cx:f><cx:v>Evening</cx:v></cx:txData></cx:tx><cx:dataId val="1"/><cx:layoutPr><cx:statistics quartileMethod="exclusive"/></cx:layoutPr></cx:series></cx:plotAreaRegion></cx:plotArea></cx:chart></cx:chartSpace>"#;

    #[test]
    fn box_whisker_series_added_removed_and_moved() {
        let names = HashMap::new();
        assert!(!series_fixed(BOX) && series_fixed(TREE));
        assert!(restructure(TREE, SeriesChange::Inserted(1), &names).is_err());
        // Added in the middle: its own block, values in column D.
        let (mid, clears) = restructure(BOX, SeriesChange::Inserted(1), &names).unwrap();
        assert!(clears.is_empty());
        Document::parse(&mid).unwrap();
        assert!(mid.contains("<cx:data id=\"2\">"), "{mid}");
        assert!(mid.contains("<cx:f>Data!$D$2:$D$3</cx:f>"), "{mid}");
        assert!(mid.contains("<cx:f>Data!$D$1</cx:f>"), "{mid}");
        assert_eq!(mid.matches("uniqueId").count(), 2, "{mid}");
        let mut d = read(BOX).unwrap();
        d.series.insert(
            1,
            ChartSeries {
                name: "Noon".into(),
                values: vec![Some(5.0), Some(6.0)],
            },
        );
        let (xml, cells, _) = rewrite(&mid, &d, &names).unwrap();
        assert_eq!(read(&xml).unwrap(), d);
        assert!(
            cells
                .iter()
                .any(|(r, v)| r.write() == "Data!$D$1" && v == &[CellValue::Text("Noon".into())])
        );
        // Removed: its block and cells go.
        let (gone, clears) = restructure(&xml, SeriesChange::Removed(0), &names).unwrap();
        assert!(
            !gone.contains("<cx:data id=\"0\">") && !gone.contains("Morning"),
            "{gone}"
        );
        assert!(clears.iter().any(|(r, _)| r.write() == "Data!$B$2:$B$3"));
        assert_eq!(read(&gone).unwrap().series.len(), 2);
        // Moved: the plot order swaps.
        let (m, _) = restructure(BOX, SeriesChange::Moved { from: 0, to: 1 }, &names).unwrap();
        let r = read(&m).unwrap();
        assert_eq!(r.series[0].name, "Evening");
        assert_eq!(r.series[1].name, "Morning");
    }
}
