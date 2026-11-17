//! Editing a chart's data: category labels, series names and values.
//!
//! A chart shows the values cached in its part (`c:strCache` /
//! `c:numCache` of each series); PowerPoint also keeps them in an embedded
//! workbook (`ppt/embeddings/*.xlsx`) that its "Edit Data" opens. An edit
//! rewrites both: the caches in the chart part (only the cache elements and
//! the `c:f` ranges when the number of categories changes; the rest of the
//! part stays byte for byte), and the referenced cells of the workbook's
//! sheet (other cells, styles and parts untouched). The edited bytes ride
//! on the frame's [`ShapeSrc`](crate::model) so undo covers them; Save
//! writes them over the original parts.
//!
//! Categories are added or removed at the end. Series are added after the
//! last one (a copy of it without its own colours, its name and values one
//! column or row further) or removed (the later series move into the
//! removed one's cells, so the data stays one block). A table in the
//! embedded sheet that covered the chart's cells is resized with them, so
//! PowerPoint's "Edit Data" shows the same range.

use std::io::{Cursor, Read as _, Write as _};
use std::ops::Range;
use std::sync::Arc;

use anyhow::{Context as _, bail};
use roxmltree::Node;

use crate::frame::NS_C;

mod ex;
use crate::history::{Focus, Kind};
use crate::presentation::Presentation;
use crate::xml::*;
pub use ex::SEP as CATEGORY_LEVEL_SEP;

/// A chart's data as a grid: categories down, series across.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChartData {
    pub categories: Vec<String>,
    pub series: Vec<ChartSeries>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChartSeries {
    pub name: String,
    pub values: Vec<Option<f64>>,
}

/// Edited chart part (and embedded workbook) bytes.
#[derive(Debug, PartialEq)]
pub struct ChartEdit {
    pub xml: Arc<Vec<u8>>,
    /// The embedded workbook's part and its new bytes.
    pub workbook: Option<(String, Arc<Vec<u8>>)>,
}

fn sers<'a, 'i>(doc: &'a roxmltree::Document<'i>) -> Vec<Node<'a, 'i>> {
    doc.descendants().filter(|n| is(*n, NS_C, "ser")).collect()
}

fn kid<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    child(n, NS_C, name)
}

/// The cache under a `c:tx` / `c:cat` / `c:val` element (`strRef/strCache`,
/// `numRef/numCache`, `strLit`, `numLit`).
fn cache<'a, 'i>(n: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    for (r, c) in [("strRef", "strCache"), ("numRef", "numCache")] {
        if let Some(c) = kid(n, r).and_then(|x| kid(x, c)) {
            return Some(c);
        }
    }
    kid(n, "strLit").or_else(|| kid(n, "numLit"))
}

fn formula(n: Node) -> Option<String> {
    ["strRef", "numRef"]
        .iter()
        .find_map(|r| kid(n, r))
        .and_then(|r| kid(r, "f"))
        .and_then(|f| f.text())
        .map(str::to_owned)
}

fn points(c: Node) -> Vec<Option<String>> {
    let n = kid(c, "ptCount")
        .and_then(|p| attr_i64(p, "val"))
        .unwrap_or(0)
        .max(0) as usize;
    let mut out = vec![None; n];
    for pt in c.children().filter(|x| is(*x, NS_C, "pt")) {
        let Some(i) = attr_i64(pt, "idx").map(|i| i as usize) else {
            continue;
        };
        let v = kid(pt, "v").and_then(|v| v.text()).map(str::to_owned);
        if i >= out.len() {
            out.resize(i + 1, None);
        }
        out[i] = v;
    }
    out
}

fn cat_el<'a, 'i>(ser: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    kid(ser, "cat").or_else(|| kid(ser, "xVal"))
}

fn val_el<'a, 'i>(ser: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    kid(ser, "val").or_else(|| kid(ser, "yVal"))
}

/// The data a chart part shows (a classic chart's or one of the Office
/// 2016 types', see [`ex`]).
pub fn read_chart_data(xml: &str) -> Option<ChartData> {
    if ex::is_chartex(xml) {
        return ex::read(xml);
    }
    let doc = roxmltree::Document::parse(xml).ok()?;
    let sers = sers(&doc);
    let first = *sers.first()?;
    let categories: Vec<String> = cat_el(first)
        .and_then(cache)
        .map(|c| {
            points(c)
                .into_iter()
                .map(Option::unwrap_or_default)
                .collect()
        })
        .unwrap_or_default();
    let series = sers
        .iter()
        .map(|s| {
            let name = kid(*s, "tx")
                .and_then(|tx| {
                    cache(tx)
                        .and_then(|c| points(c).into_iter().next().flatten())
                        .or_else(|| kid(tx, "v").and_then(|v| v.text()).map(str::to_owned))
                })
                .unwrap_or_default();
            let values = val_el(*s)
                .and_then(cache)
                .map(|c| {
                    points(c)
                        .into_iter()
                        .map(|v| v.and_then(|v| v.trim().parse().ok()))
                        .collect()
                })
                .unwrap_or_default();
            ChartSeries { name, values }
        })
        .collect::<Vec<_>>();
    // Values padded to the categories.
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

/// The `prefix:` an element is written with.
fn prefix_of(src: &str, n: Node) -> String {
    let name: String = src[n.range().start + 1..]
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '>' && *c != '/')
        .collect();
    match name.split_once(':') {
        Some((p, _)) => format!("{p}:"),
        None => String::new(),
    }
}

/// A new cache element in place of `c` with `values`.
fn new_cache(src: &str, c: Node, values: &[Option<String>]) -> String {
    let p = prefix_of(src, c);
    let name = c.tag_name().name();
    let mut out = format!("<{p}{name}>");
    if let Some(fc) = kid(c, "formatCode") {
        out.push_str(raw(src, fc));
    }
    out.push_str(&format!("<{p}ptCount val=\"{}\"/>", values.len()));
    for (i, v) in values.iter().enumerate() {
        if let Some(v) = v {
            out.push_str(&format!(
                "<{p}pt idx=\"{i}\"><{p}v>{}</{p}v></{p}pt>",
                esc(v)
            ));
        }
    }
    out.push_str(&format!("</{p}{name}>"));
    out
}

/// A cell reference, `Sheet1!$A$2:$A$5` split up.
#[derive(Clone, Debug, PartialEq)]
struct Ref {
    sheet: String,
    c0: u32,
    r0: u32,
    c1: u32,
    r1: u32,
}

fn col_num(s: &str) -> Option<u32> {
    let mut n = 0u32;
    for ch in s.chars() {
        if !ch.is_ascii_alphabetic() {
            return None;
        }
        n = n * 26 + (ch.to_ascii_uppercase() as u32 - 'A' as u32 + 1);
    }
    (n > 0).then_some(n)
}

fn col_name(mut n: u32) -> String {
    let mut s = Vec::new();
    while n > 0 {
        let r = (n - 1) % 26;
        s.push((b'A' + r as u8) as char);
        n = (n - 1) / 26;
    }
    s.iter().rev().collect()
}

fn cell_ref(s: &str) -> Option<(u32, u32)> {
    let s = s.replace('$', "");
    let split = s.find(|c: char| c.is_ascii_digit())?;
    Some((col_num(&s[..split])?, s[split..].parse().ok()?))
}

impl Ref {
    fn parse(f: &str) -> Option<Self> {
        let (sheet, range) = f.rsplit_once('!')?;
        let sheet = sheet.trim_matches('\'').replace("''", "'");
        let (a, b) = range.split_once(':').unwrap_or((range, range));
        let (c0, r0) = cell_ref(a)?;
        let (c1, r1) = cell_ref(b)?;
        Some(Self {
            sheet,
            c0,
            r0,
            c1,
            r1,
        })
    }

    fn cells(&self) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        for r in self.r0..=self.r1 {
            for c in self.c0..=self.c1 {
                out.push((c, r));
            }
        }
        out
    }

    /// Moved by `dc` columns and `dr` rows.
    fn shifted(&self, dc: u32, dr: u32) -> Self {
        let mut r = self.clone();
        r.c0 += dc;
        r.c1 += dc;
        r.r0 += dr;
        r.r1 += dr;
        r
    }

    /// The block covering both (same sheet).
    fn union(&self, o: &Ref) -> Option<Self> {
        (self.sheet.eq_ignore_ascii_case(&o.sheet)).then(|| Ref {
            sheet: self.sheet.clone(),
            c0: self.c0.min(o.c0),
            r0: self.r0.min(o.r0),
            c1: self.c1.max(o.c1),
            r1: self.r1.max(o.r1),
        })
    }

    /// `A1:C5`, without the sheet.
    fn area(&self) -> String {
        format!(
            "{}{}:{}{}",
            col_name(self.c0),
            self.r0,
            col_name(self.c1),
            self.r1
        )
    }

    /// The same start, `n` cells long along its direction.
    fn resized(&self, n: u32) -> Self {
        let n = n.max(1);
        let mut r = self.clone();
        if self.c0 == self.c1 {
            r.r1 = self.r0 + n - 1;
        } else {
            r.c1 = self.c0 + n - 1;
        }
        r
    }

    fn write(&self) -> String {
        let sheet = if self
            .sheet
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            self.sheet.clone()
        } else {
            format!("'{}'", self.sheet.replace('\'', "''"))
        };
        let a = format!("${}${}", col_name(self.c0), self.r0);
        if (self.c0, self.r0) == (self.c1, self.r1) {
            format!("{sheet}!{a}")
        } else {
            format!("{sheet}!{a}:${}${}", col_name(self.c1), self.r1)
        }
    }
}

/// Cells of a range and what goes in them.
type CellWrites = (Ref, Vec<CellValue>);

/// A value to put in a workbook cell.
#[derive(Clone, Debug, PartialEq)]
enum CellValue {
    Text(String),
    Number(f64),
    Empty,
}

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// How the series of a chart change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SeriesChange {
    Same,
    /// A new series at this place (the number of series: after the last).
    Inserted(usize),
    /// This one is gone.
    Removed(usize),
    /// Series `from` and `to` swap places in the plot order.
    Moved {
        from: usize,
        to: usize,
    },
}

/// A series' name and value references.
fn ser_refs(ser: Node) -> (Option<Ref>, Option<Ref>) {
    (
        kid(ser, "tx")
            .and_then(formula)
            .and_then(|f| Ref::parse(&f)),
        val_el(ser).and_then(formula).and_then(|f| Ref::parse(&f)),
    )
}

/// The `c:f` text node under a `c:tx` / `c:val` element.
fn f_text<'a, 'i>(el: Option<Node<'a, 'i>>) -> Option<Node<'a, 'i>> {
    let el = el?;
    ["strRef", "numRef"]
        .iter()
        .find_map(|r| kid(el, r))
        .and_then(|r| kid(r, "f"))
        .and_then(|f| f.first_child())
        .filter(|t| t.is_text())
}

/// The block every reference of the chart lies in (one sheet).
fn data_box(xml: &str) -> Option<Ref> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let mut out: Option<Ref> = None;
    for ser in sers(&doc) {
        for el in [kid(ser, "tx"), cat_el(ser), val_el(ser)]
            .into_iter()
            .flatten()
        {
            let Some(r) = formula(el).and_then(|f| Ref::parse(&f)) else {
                continue;
            };
            out = Some(match out {
                None => r,
                Some(b) => b.union(&r)?,
            });
        }
    }
    out
}

/// The chart part with a series added, removed or moved (caches not yet
/// rewritten), and the cells the change empties.
///
/// Series keep their own cells: a new one takes the cells of the series
/// it goes before (that one and the series after it in the sheet move a
/// column or row on), and when one is removed the series after it in the
/// sheet move back into its cells, so the data stays one block. Moving a
/// series changes the plot order (`c:order`, and its place in the part)
/// as PowerPoint's *Select Data* does; its cells stay where they are.
fn restructure(xml: &str, change: SeriesChange) -> anyhow::Result<(String, Vec<CellWrites>)> {
    let doc = roxmltree::Document::parse(xml).context("chart XML")?;
    let sers = sers(&doc);
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut clears: Vec<CellWrites> = Vec::new();
    let refs: Vec<(Option<Ref>, Option<Ref>)> = sers.iter().map(|s| ser_refs(*s)).collect();
    // Series in the order of their cells in the sheet.
    let mut slots: Vec<usize> = (0..sers.len()).collect();
    slots.sort_by_key(|&i| {
        let (tx, val) = &refs[i];
        val.as_ref()
            .or(tx.as_ref())
            .map_or((u32::MAX, u32::MAX), |r| (r.c0, r.r0))
    });
    let slot_of = |i: usize| slots.iter().position(|&s| s == i).unwrap_or(0);
    let retarget = |j: usize, (tx, val): &(Option<Ref>, Option<Ref>), edits: &mut Vec<_>| {
        for (el, r) in [(kid(sers[j], "tx"), tx), (val_el(sers[j]), val)] {
            if let (Some(t), Some(r)) = (f_text(el), r) {
                edits.push((t.range(), esc(&r.write())));
            }
        }
    };
    match change {
        SeriesChange::Same => return Ok((xml.to_owned(), clears)),
        SeriesChange::Inserted(k) => {
            let n = sers.len();
            if n == 0 {
                bail!("the chart has no series");
            }
            let k = k.min(n);
            let template = sers[k.min(n - 1)];
            let p = prefix_of(xml, template);
            let max = |name: &str| {
                sers.iter()
                    .filter_map(|s| kid(*s, name).and_then(|n| attr_i64(n, "val")))
                    .max()
                    .unwrap_or(-1)
                    + 1
            };
            let order_of = |s: Node| kid(s, "order").and_then(|n| attr_i64(n, "val"));
            let (tx, val) = &refs[template_index(&sers, template)];
            // Series in columns move right, series in rows move down.
            let (dc, dr) = match val {
                Some(v) if v.c0 == v.c1 && v.r0 != v.r1 => (1, 0),
                Some(v) if v.r0 == v.r1 && v.c0 != v.c1 => (0, 1),
                _ => match tx {
                    Some(t) if t.r0 == 1 || val.as_ref().is_some_and(|v| t.r0 < v.r0) => (1, 0),
                    _ => (0, 1),
                },
            };
            let shift = |r: &Option<Ref>| r.as_ref().map(|r| r.shifted(dc, dr));
            // Where the new series' cells are, and the series that move on.
            let (new_refs, new_order) = if k == n {
                let last = slots[n - 1];
                ((shift(&refs[last].0), shift(&refs[last].1)), max("order"))
            } else {
                let from = slot_of(k);
                for &j in &slots[from..] {
                    retarget(j, &(shift(&refs[j].0), shift(&refs[j].1)), &mut edits);
                }
                // Plot order: the new one takes series k's place.
                let ok = order_of(sers[k]).unwrap_or(k as i64);
                for s in &sers {
                    if let (Some(o), Some(n)) = (order_of(*s), kid(*s, "order"))
                        && o >= ok
                        && let Some(a) = n.attributes().find(|a| a.name() == "val")
                    {
                        edits.push((a.range_value(), (o + 1).to_string()));
                    }
                }
                (refs[k].clone(), ok)
            };
            let mut clone = format!("<{p}ser>");
            for c in template.children().filter(|c| c.is_element()) {
                let name = c.tag_name().name();
                match name {
                    "idx" => clone.push_str(&format!("<{p}idx val=\"{}\"/>", max("idx"))),
                    "order" => clone.push_str(&format!("<{p}order val=\"{new_order}\"/>")),
                    // Its own colours, points, labels and IDs stay with
                    // the original.
                    "spPr" | "dPt" | "extLst" => {}
                    "marker" => {
                        let mut m = raw(xml, c).to_owned();
                        if let Some(sp) = kid(c, "spPr") {
                            let r = sp.range();
                            let base = c.range().start;
                            m.replace_range(r.start - base..r.end - base, "");
                        }
                        clone.push_str(&m);
                    }
                    "tx" | "val" | "yVal" => {
                        let mut t = raw(xml, c).to_owned();
                        let r = if name == "tx" {
                            &new_refs.0
                        } else {
                            &new_refs.1
                        };
                        if let Some(f) = f_text(Some(c))
                            && let Some(r) = r
                        {
                            let base = c.range().start;
                            let fr = f.range();
                            t.replace_range(fr.start - base..fr.end - base, &esc(&r.write()));
                        }
                        clone.push_str(&t);
                    }
                    _ => clone.push_str(raw(xml, c)),
                }
            }
            clone.push_str(&format!("</{p}ser>"));
            let at = if k == n {
                template.range().end
            } else {
                template.range().start
            };
            edits.push((at..at, clone));
        }
        SeriesChange::Removed(k) => {
            if sers.len() <= 1 {
                bail!("A chart needs at least one series");
            }
            let gone = *sers.get(k).context("no such series")?;
            edits.push((gone.range(), String::new()));
            // Later series in the sheet take the cells of the one before.
            let from = slot_of(k);
            for w in from..slots.len() - 1 {
                retarget(slots[w + 1], &refs[slots[w]], &mut edits);
            }
            // The last cells used are empty now.
            let (tx, val) = &refs[slots[slots.len() - 1]];
            for r in [tx, val].into_iter().flatten() {
                clears.push((r.clone(), vec![CellValue::Empty; r.cells().len()]));
            }
        }
        SeriesChange::Moved { from, to } => {
            let (a, b) = (
                *sers.get(from).context("no such series")?,
                *sers.get(to).context("no such series")?,
            );
            if from == to {
                return Ok((xml.to_owned(), clears));
            }
            if a.parent() != b.parent() {
                bail!("A series can't be moved into another chart type's series");
            }
            let order = |s: Node| kid(s, "order").and_then(|n| attr_i64(n, "val"));
            let with_order = |s: Node, o: Option<i64>| -> String {
                let mut t = raw(xml, s).to_owned();
                if let (Some(o), Some(n)) = (o, kid(s, "order"))
                    && let Some(at) = n.attributes().find(|x| x.name() == "val")
                {
                    let base = s.range().start;
                    let r = at.range_value();
                    t.replace_range(r.start - base..r.end - base, &o.to_string());
                }
                t
            };
            // Each takes the other's place and plot order.
            edits.push((a.range(), with_order(b, order(a))));
            edits.push((b.range(), with_order(a, order(b))));
        }
    }
    edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
    let mut out = xml.to_owned();
    for (r, t) in edits {
        out.replace_range(r, &t);
    }
    Ok((out, clears))
}

fn template_index(sers: &[Node], n: Node) -> usize {
    sers.iter().position(|s| *s == n).unwrap_or(0)
}

/// The chart part with `data` in its caches; and the workbook cells it
/// references with their new values.
fn rewrite_chart(xml: &str, data: &ChartData) -> anyhow::Result<(String, Vec<CellWrites>)> {
    let doc = roxmltree::Document::parse(xml).context("chart XML")?;
    let sers = sers(&doc);
    if sers.len() != data.series.len() {
        bail!(
            "The chart has {} series, not {}",
            sers.len(),
            data.series.len()
        );
    }
    let n = data.categories.len();
    if n == 0 {
        bail!("A chart needs at least one category");
    }
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut cells: Vec<CellWrites> = Vec::new();
    let fix_ref = |el: Node, edits: &mut Vec<(Range<usize>, String)>| -> Option<(Ref, Ref)> {
        let f = ["strRef", "numRef"]
            .iter()
            .find_map(|r| kid(el, r))
            .and_then(|r| kid(r, "f"))?;
        let r = Ref::parse(f.text()?)?;
        let new = r.resized(n as u32);
        if new != r {
            let text = f.first_child()?;
            edits.push((text.range(), esc(&new.write())));
        }
        Some((new, r))
    };
    // Cells a shorter range no longer covers are emptied (no stale data
    // left below or beside the chart's block).
    let mut stale: Vec<CellWrites> = Vec::new();
    let mut note_stale = |new: &Ref, old: &Ref| {
        let keep = new.cells();
        let gone: Vec<(u32, u32)> = old
            .cells()
            .into_iter()
            .filter(|c| !keep.contains(c))
            .collect();
        if let (Some(&(c0, r0)), Some(&(c1, r1))) = (gone.first(), gone.last()) {
            let r = Ref {
                sheet: old.sheet.clone(),
                c0,
                r0,
                c1,
                r1,
            };
            stale.push((r, vec![CellValue::Empty; gone.len()]));
        }
    };
    for (ser, sd) in sers.iter().zip(&data.series) {
        if let Some(tx) = kid(*ser, "tx") {
            if let Some(c) = cache(tx) {
                edits.push((c.range(), new_cache(xml, c, &[Some(sd.name.clone())])));
                if let Some(f) = formula(tx).and_then(|f| Ref::parse(&f)) {
                    cells.push((f, vec![CellValue::Text(sd.name.clone())]));
                }
            } else if let Some(v) = kid(tx, "v").and_then(|v| v.first_child()) {
                edits.push((v.range(), esc(&sd.name)));
            }
        }
        if let Some(cat) = cat_el(*ser)
            && !kid(cat, "multiLvlStrRef").is_some()
            && let Some(c) = cache(cat)
        {
            let numeric = c.tag_name().name().starts_with("num");
            let vals: Vec<Option<String>> = data
                .categories
                .iter()
                .map(|s| {
                    if numeric {
                        s.trim().parse::<f64>().ok().map(fmt_num)
                    } else {
                        Some(s.clone())
                    }
                })
                .collect();
            edits.push((c.range(), new_cache(xml, c, &vals)));
            if let Some((r, old)) = fix_ref(cat, &mut edits) {
                note_stale(&r, &old);
                let v = data
                    .categories
                    .iter()
                    .map(|s| match s.trim().parse::<f64>() {
                        Ok(x) if numeric => CellValue::Number(x),
                        _ if s.is_empty() => CellValue::Empty,
                        _ => CellValue::Text(s.clone()),
                    })
                    .collect();
                cells.push((r, v));
            }
        }
        if let Some(val) = val_el(*ser)
            && let Some(c) = cache(val)
        {
            let mut values = sd.values.clone();
            values.resize(n, None);
            let vals: Vec<Option<String>> = values.iter().map(|v| v.map(fmt_num)).collect();
            edits.push((c.range(), new_cache(xml, c, &vals)));
            if let Some((r, old)) = fix_ref(val, &mut edits) {
                note_stale(&r, &old);
                cells.push((
                    r,
                    values
                        .iter()
                        .map(|v| v.map_or(CellValue::Empty, CellValue::Number))
                        .collect(),
                ));
            }
        }
    }
    // Emptied first, so a cell another range still uses keeps its value.
    stale.append(&mut cells);
    let cells = stale;
    // Overlapping edits (a formula inside a replaced cache) can't happen:
    // `c:f` is a sibling of the cache.
    edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
    let mut out = xml.to_owned();
    for (r, t) in edits {
        out.replace_range(r, &t);
    }
    Ok((out, cells))
}

/// Find a child element's range by local name directly under the element
/// starting at `at` (simple scanner for SpreadsheetML).
fn find_cell(sheet: &str, addr: &str) -> Option<Range<usize>> {
    let needle = format!("r=\"{addr}\"");
    let mut from = 0;
    while let Some(i) = sheet[from..].find(&needle) {
        let at = from + i;
        let start = sheet[..at].rfind('<')?;
        let tag = &sheet[start..];
        let name_ok = tag.starts_with("<c ") || tag.starts_with("<x:c ");
        if name_ok {
            let open_end = start + tag_end(tag)?;
            if sheet[..open_end].ends_with("/>") {
                return Some(start..open_end);
            }
            let close = if tag.starts_with("<x:") {
                "</x:c>"
            } else {
                "</c>"
            };
            let end = sheet[open_end..].find(close)? + open_end + close.len();
            return Some(start..end);
        }
        from = at + needle.len();
    }
    None
}

fn style_attr(cell: &str) -> String {
    let tag = &cell[..tag_end(cell).unwrap_or(cell.len())];
    split_attrs(tag_attrs(tag))
        .into_iter()
        .find(|(k, _)| k == "s")
        .map(|(_, v)| format!(" s=\"{v}\""))
        .unwrap_or_default()
}

fn new_cell(addr: &str, style: &str, v: &CellValue) -> String {
    match v {
        CellValue::Number(x) => format!("<c r=\"{addr}\"{style}><v>{}</v></c>", fmt_num(*x)),
        CellValue::Text(t) => format!(
            "<c r=\"{addr}\"{style} t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
            esc(t)
        ),
        CellValue::Empty => format!("<c r=\"{addr}\"{style}/>"),
    }
}

/// Put `v` in cell (`col`, `row`) of a worksheet's XML.
fn set_cell(sheet: &mut String, col: u32, row: u32, v: &CellValue) {
    let addr = format!("{}{row}", col_name(col));
    if let Some(r) = find_cell(sheet, &addr) {
        let style = style_attr(&sheet[r.clone()]);
        sheet.replace_range(r, &new_cell(&addr, &style, v));
        return;
    }
    if *v == CellValue::Empty {
        return;
    }
    let cell = new_cell(&addr, "", v);
    // The row, or a new one in order.
    let row_tag = format!("<row r=\"{row}\"");
    if let Some(i) = sheet.find(&row_tag) {
        let open_end = i + tag_end(&sheet[i..]).unwrap_or(0);
        if sheet[..open_end].ends_with("/>") {
            let open = sheet[i..open_end - 2].to_owned();
            sheet.replace_range(i..open_end, &format!("{open}>{cell}</row>"));
            return;
        }
        let close = sheet[open_end..]
            .find("</row>")
            .map_or(open_end, |e| open_end + e);
        // Before the first cell with a later column.
        let mut at = close;
        let mut k = open_end;
        while let Some(j) = sheet[k..close].find("<c r=\"") {
            let s = k + j;
            let a: String = sheet[s + 6..].chars().take_while(|c| *c != '"').collect();
            if cell_ref(&a).is_some_and(|(c, _)| c > col) {
                at = s;
                break;
            }
            k = s + 6;
        }
        sheet.insert_str(at, &cell);
        return;
    }
    let new_row = format!("<row r=\"{row}\">{cell}</row>");
    let Some(data_start) = sheet.find("<sheetData") else {
        return;
    };
    let data_open_end = data_start + tag_end(&sheet[data_start..]).unwrap_or(0);
    if sheet[..data_open_end].ends_with("/>") {
        sheet.replace_range(
            data_start..data_open_end,
            &format!("<sheetData>{new_row}</sheetData>"),
        );
        return;
    }
    let data_end = sheet.find("</sheetData>").unwrap_or(sheet.len());
    let mut at = data_end;
    let mut k = data_open_end;
    while let Some(j) = sheet[k..data_end].find("<row r=\"") {
        let s = k + j;
        let n: String = sheet[s + 8..].chars().take_while(|c| *c != '"').collect();
        if n.parse::<u32>().is_ok_and(|n| n > row) {
            at = s;
            break;
        }
        k = s + 8;
    }
    sheet.insert_str(at, &new_row);
}

/// A table part with its range changed from `old` to `new` (it must cover
/// exactly `old`): `ref`, the filter's `ref` and the columns (named after
/// their header cells in `header`, else as they were).
fn resize_table(
    xml: &str,
    old: &Ref,
    new: &Ref,
    header: &dyn Fn(u32) -> Option<String>,
) -> Option<String> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    let r = root.attribute("ref")?;
    if !r.replace('$', "").eq_ignore_ascii_case(&old.area()) || old.c0 != new.c0 || old.r0 != new.r0
    {
        return None;
    }
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let attr_range = |n: Node, name: &str| {
        n.attributes()
            .find(|a| a.name() == name)
            .map(|a| a.range_value())
    };
    edits.push((attr_range(root, "ref")?, new.area()));
    if let Some(f) = root
        .children()
        .find(|c| c.tag_name().name() == "autoFilter")
        && let Some(rr) = attr_range(f, "ref")
    {
        edits.push((rr, new.area()));
    }
    let cols = root
        .children()
        .find(|c| c.tag_name().name() == "tableColumns")?;
    let list: Vec<Node> = cols
        .children()
        .filter(|c| c.tag_name().name() == "tableColumn")
        .collect();
    let n = (new.c1 - new.c0 + 1) as usize;
    let mut used = std::collections::HashSet::new();
    let mut names = Vec::new();
    for i in 0..n {
        let base = header(new.c0 + i as u32)
            .filter(|h| !h.trim().is_empty())
            .or_else(|| {
                list.get(i)
                    .and_then(|c| c.attribute("name"))
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| format!("Column{}", i + 1));
        let mut name = base.clone();
        let mut k = 2;
        while !used.insert(name.to_lowercase()) {
            name = format!("{base}{k}");
            k += 1;
        }
        names.push(name);
    }
    for (i, c) in list.iter().enumerate() {
        if i < n {
            if let Some(rr) = attr_range(*c, "name") {
                edits.push((rr, esc_attr(&names[i])));
            }
        } else {
            edits.push((c.range(), String::new()));
        }
    }
    if n > list.len() {
        let p = prefix_of(xml, cols);
        let first = list
            .iter()
            .filter_map(|c| attr_i64(*c, "id"))
            .max()
            .unwrap_or(0)
            + 1;
        let mut add = String::new();
        for (next, name) in (first..).zip(&names[list.len()..]) {
            add.push_str(&format!(
                "<{p}tableColumn id=\"{next}\" name=\"{}\"/>",
                esc_attr(name)
            ));
        }
        let end = cols.range().end;
        let close = xml[cols.range()].rfind("</")? + cols.range().start;
        if close < end && !xml[..end].ends_with("/>") {
            edits.push((close..close, add));
        } else {
            return None;
        }
    }
    if let Some(rr) = attr_range(cols, "count") {
        edits.push((rr, n.to_string()));
    }
    edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
    let mut out = xml.to_owned();
    for (r, t) in edits {
        out.replace_range(r, &t);
    }
    Some(out)
}

/// The workbook with the referenced cells set; `resize` (the chart's old
/// and new blocks) resizes a table that covered the old one.
fn patch_workbook(
    bytes: &[u8],
    cells: &[CellWrites],
    resize: Option<(Ref, Ref)>,
    renames: &[(String, String)],
) -> anyhow::Result<Vec<u8>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).context("embedded workbook")?;
    let text = |zip: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str| -> Option<String> {
        let mut f = zip.by_name(name).ok()?;
        let mut s = String::new();
        f.read_to_string(&mut s).ok()?;
        Some(s)
    };
    let wb = text(&mut zip, "xl/workbook.xml").context("no workbook.xml")?;
    let rels = parse_rels(&text(&mut zip, "xl/_rels/workbook.xml.rels").unwrap_or_default());
    let wdoc = roxmltree::Document::parse(&wb).context("workbook.xml")?;
    let sheet_part = |name: &str| -> Option<String> {
        let s = wdoc
            .descendants()
            .find(|n| n.tag_name().name() == "sheet" && n.attribute("name") == Some(name))?;
        let id = s
            .attributes()
            .find(|a| a.name() == "id" && a.namespace() == Some(NS_R))?
            .value();
        rels.iter()
            .find(|r| r.id == id)
            .and_then(|r| r.part("xl/workbook.xml"))
    };
    let mut sheets: Vec<(String, String)> = Vec::new();
    for (r, values) in cells {
        let Some(part) = sheet_part(&r.sheet) else {
            continue;
        };
        if !sheets.iter().any(|(p, _)| *p == part) {
            let Some(src) = text(&mut zip, &part) else {
                continue;
            };
            sheets.push((part.clone(), src));
        }
        let sheet = &mut sheets
            .iter_mut()
            .find(|(p, _)| *p == part)
            .expect("added")
            .1;
        for ((c, row), v) in r.cells().into_iter().zip(values) {
            set_cell(sheet, c, row, v);
        }
    }
    // Tables of the data's sheet covering the old block.
    let mut tables: Vec<(String, String)> = Vec::new();
    if let Some((old, new)) = resize.filter(|(o, n)| o != n)
        && let Some(part) = sheet_part(&old.sheet)
    {
        let header = |c: u32| -> Option<String> {
            cells.iter().rev().find_map(|(r, vals)| {
                r.cells().into_iter().zip(vals).find_map(|((cc, rr), v)| {
                    (cc == c && rr == new.r0).then(|| match v {
                        CellValue::Text(t) => t.clone(),
                        CellValue::Number(x) => fmt_num(*x),
                        CellValue::Empty => String::new(),
                    })
                })
            })
        };
        let srels = parse_rels(&text(&mut zip, &rels_name(&part)).unwrap_or_default());
        for r in srels.iter().filter(|r| r.kind() == "table") {
            let Some(tp) = r.part(&part) else { continue };
            let Some(x) = text(&mut zip, &tp) else {
                continue;
            };
            if let Some(nx) = resize_table(&x, &old, &new, &header) {
                tables.push((tp, nx));
            }
        }
        // The sheet's used range covers the new block.
        if let Some((_, sx)) = sheets.iter_mut().find(|(p, _)| *p == part)
            && let Some(i) = sx.find("<dimension ref=\"")
        {
            let a = i + 16;
            if let Some(len) = sx[a..].find('"') {
                let cur = format!("{}!{}", old.sheet, &sx[a..a + len]);
                if let Some(d) = Ref::parse(&cur).and_then(|d| d.union(&new)) {
                    sx.replace_range(a..a + len, &d.area());
                }
            }
        }
    }
    sheets.extend(tables);
    // Defined names (`_xlchart.v1.0`) whose range grew or shrank.
    if !renames.is_empty() {
        let mut x = wb.clone();
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        for n in wdoc
            .descendants()
            .filter(|n| n.tag_name().name() == "definedName")
        {
            let Some((_, to)) = n
                .attribute("name")
                .and_then(|name| renames.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)))
            else {
                continue;
            };
            if let Some(t) = n.first_child().filter(|t| t.is_text()) {
                edits.push((t.range(), esc(to)));
            }
        }
        edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
        for (r, t) in edits {
            x.replace_range(r, &t);
        }
        sheets.push(("xl/workbook.xml".to_owned(), x));
    }
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for i in 0..zip.len() {
        let file = zip.by_index_raw(i)?;
        let name = file.name()?.into_owned();
        match sheets.iter().find(|(p, _)| *p == name) {
            Some((_, xml)) => {
                drop(file);
                out.start_file(name.as_str(), opts)?;
                out.write_all(xml.as_bytes())?;
            }
            None => out.raw_copy_file(file)?,
        }
    }
    Ok(out.finish()?.into_inner())
}

/// The defined names of a workbook (`xl/workbook.xml`): name -> formula.
fn defined_names(bytes: &[u8]) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let Ok(mut zip) = zip::ZipArchive::new(Cursor::new(bytes)) else {
        return out;
    };
    let mut wb = String::new();
    match zip.by_name("xl/workbook.xml") {
        Ok(mut f) => {
            if f.read_to_string(&mut wb).is_err() {
                return out;
            }
        }
        Err(_) => return out,
    }
    let Ok(doc) = roxmltree::Document::parse(&wb) else {
        return out;
    };
    for n in doc
        .descendants()
        .filter(|n| n.tag_name().name() == "definedName")
    {
        if let (Some(name), Some(f)) = (n.attribute("name"), n.text()) {
            out.insert(name.to_owned(), f.to_owned());
        }
    }
    out
}

impl Presentation {
    /// The chart part a shape shows and its current bytes (edited in
    /// Kantor, or as read).
    fn chart_bytes(&self, slide: usize, shape: usize) -> Option<(String, Arc<Vec<u8>>)> {
        let s = self.slides.get(slide)?.shapes.get(shape)?;
        let src = s.src.as_ref()?;
        let part = src.chart.clone()?;
        let bytes = match &src.chart_edit {
            Some(e) => e.xml.clone(),
            None => self.part_bytes(&part)?,
        };
        Some((part, bytes))
    }

    /// The data a chart frame shows (`None` if it isn't a chart).
    pub fn chart_data(&self, slide: usize, shape: usize) -> Option<ChartData> {
        let (_, bytes) = self.chart_bytes(slide, shape)?;
        read_chart_data(std::str::from_utf8(&bytes).ok()?)
    }

    /// Whether a chart's series can't be added, removed or moved here (the
    /// Office 2016 types that plot one series: waterfall, funnel, treemap,
    /// sunburst, histogram, Pareto).
    pub fn chart_series_fixed(&self, slide: usize, shape: usize) -> bool {
        self.chart_bytes(slide, shape).is_some_and(|(_, b)| {
            std::str::from_utf8(&b).is_ok_and(|x| ex::is_chartex(x) && ex::series_fixed(x))
        })
    }

    /// Bytes of a part as edited in Kantor (a chart or its workbook).
    pub(crate) fn edited_part(&self, part: &str) -> Option<Arc<Vec<u8>>> {
        for s in &self.slides {
            for sh in &s.shapes {
                let Some(e) = sh.src.as_ref().and_then(|src| src.chart_edit.as_ref()) else {
                    continue;
                };
                if sh.src.as_ref().and_then(|x| x.chart.as_deref()) == Some(part) {
                    return Some(e.xml.clone());
                }
                if let Some((p, b)) = &e.workbook
                    && p == part
                {
                    return Some(b.clone());
                }
            }
        }
        None
    }

    /// Every part edited in Kantor (for Save).
    pub(crate) fn edited_parts(&self) -> Vec<(String, Arc<Vec<u8>>)> {
        let mut out: Vec<(String, Arc<Vec<u8>>)> = Vec::new();
        for s in &self.slides {
            for sh in &s.shapes {
                let Some(src) = sh.src.as_ref() else { continue };
                let (Some(e), Some(part)) = (&src.chart_edit, &src.chart) else {
                    continue;
                };
                if !out.iter().any(|(p, _)| p == part) {
                    out.push((part.clone(), e.xml.clone()));
                }
                if let Some((p, b)) = &e.workbook
                    && !out.iter().any(|(x, _)| x == p)
                {
                    out.push((p.clone(), b.clone()));
                }
            }
        }
        out
    }

    /// A number that changes whenever a chart part is edited (for caches).
    pub fn chart_version(&self, part: &str) -> usize {
        for s in &self.slides {
            for sh in &s.shapes {
                if let Some(src) = &sh.src
                    && src.chart.as_deref() == Some(part)
                    && let Some(e) = &src.chart_edit
                {
                    return Arc::as_ptr(e) as usize;
                }
            }
        }
        0
    }

    /// Replace a chart's data: the chart part's caches and the embedded
    /// workbook's cells. One undo step.
    pub fn set_chart_data(
        &mut self,
        slide: usize,
        shape: usize,
        data: &ChartData,
    ) -> anyhow::Result<()> {
        let (part, bytes) = self.chart_bytes(slide, shape).context("not a chart")?;
        let xml = std::str::from_utf8(&bytes).context("chart part isn't text")?;
        self.change_chart(slide, shape, xml, &part, data, SeriesChange::Same)
    }

    /// Add a series after the last one (named `name`, values empty). One
    /// undo step.
    pub fn add_chart_series(
        &mut self,
        slide: usize,
        shape: usize,
        name: &str,
    ) -> anyhow::Result<()> {
        let n = self
            .chart_data(slide, shape)
            .context("not a chart")?
            .series
            .len();
        self.insert_chart_series(slide, shape, n, name)
    }

    /// Insert a series at `index` (before the series there; the number of
    /// series puts it after the last), named `name` with empty values. It
    /// takes that series' cells in the embedded workbook; the series after
    /// it move a column (or row) on, as inserting a column in PowerPoint's
    /// *Edit Data* does. One undo step.
    pub fn insert_chart_series(
        &mut self,
        slide: usize,
        shape: usize,
        index: usize,
        name: &str,
    ) -> anyhow::Result<()> {
        let (part, bytes) = self.chart_bytes(slide, shape).context("not a chart")?;
        let xml = std::str::from_utf8(&bytes).context("chart part isn't text")?;
        let mut data = read_chart_data(xml).context("not a chart")?;
        let index = index.min(data.series.len());
        let n = data.categories.len();
        data.series.insert(
            index,
            ChartSeries {
                name: name.to_owned(),
                values: vec![None; n],
            },
        );
        self.change_chart(
            slide,
            shape,
            xml,
            &part,
            &data,
            SeriesChange::Inserted(index),
        )
    }

    /// Move series `index` one place up (earlier) or down in the plot
    /// order, as PowerPoint's *Select Data* does. One undo step.
    pub fn move_chart_series(
        &mut self,
        slide: usize,
        shape: usize,
        index: usize,
        up: bool,
    ) -> anyhow::Result<()> {
        let (part, bytes) = self.chart_bytes(slide, shape).context("not a chart")?;
        let xml = std::str::from_utf8(&bytes).context("chart part isn't text")?;
        let mut data = read_chart_data(xml).context("not a chart")?;
        let to = if up {
            index.checked_sub(1)
        } else {
            Some(index + 1)
        };
        let Some(to) = to.filter(|&t| t < data.series.len() && index < data.series.len()) else {
            bail!("That series can't move further");
        };
        data.series.swap(index, to);
        self.change_chart(
            slide,
            shape,
            xml,
            &part,
            &data,
            SeriesChange::Moved { from: index, to },
        )
    }

    /// Insert an empty category named `name` at `index` (the number of
    /// categories: after the last). One undo step.
    pub fn insert_chart_category(
        &mut self,
        slide: usize,
        shape: usize,
        index: usize,
        name: &str,
    ) -> anyhow::Result<()> {
        let mut data = self.chart_data(slide, shape).context("not a chart")?;
        let index = index.min(data.categories.len());
        data.categories.insert(index, name.to_owned());
        for s in &mut data.series {
            s.values.insert(index.min(s.values.len()), None);
        }
        self.set_chart_data(slide, shape, &data)
    }

    /// Remove category `index` (its values in every series). One undo step.
    pub fn remove_chart_category(
        &mut self,
        slide: usize,
        shape: usize,
        index: usize,
    ) -> anyhow::Result<()> {
        let mut data = self.chart_data(slide, shape).context("not a chart")?;
        if data.categories.len() <= 1 {
            bail!("A chart needs at least one category");
        }
        if index >= data.categories.len() {
            bail!("no such category");
        }
        data.categories.remove(index);
        for s in &mut data.series {
            if index < s.values.len() {
                s.values.remove(index);
            }
        }
        self.set_chart_data(slide, shape, &data)
    }

    /// Remove series `index`. One undo step.
    pub fn remove_chart_series(
        &mut self,
        slide: usize,
        shape: usize,
        index: usize,
    ) -> anyhow::Result<()> {
        let (part, bytes) = self.chart_bytes(slide, shape).context("not a chart")?;
        let xml = std::str::from_utf8(&bytes).context("chart part isn't text")?;
        let mut data = read_chart_data(xml).context("not a chart")?;
        if data.series.len() <= 1 {
            bail!("A chart needs at least one series");
        }
        if index >= data.series.len() {
            bail!("no such series");
        }
        data.series.remove(index);
        self.change_chart(
            slide,
            shape,
            xml,
            &part,
            &data,
            SeriesChange::Removed(index),
        )
    }

    fn change_chart(
        &mut self,
        slide: usize,
        shape: usize,
        xml: &str,
        part: &str,
        data: &ChartData,
        change: SeriesChange,
    ) -> anyhow::Result<()> {
        let part = part.to_owned();
        // The embedded workbook, from the chart part's relationships.
        let rels = self
            .part_bytes(&rels_name(&part))
            .map(|b| parse_rels(&String::from_utf8_lossy(&b)))
            .unwrap_or_default();
        let wb_part = rels
            .iter()
            .find(|r| r.kind() == "package" && !r.external)
            .and_then(|r| r.part(&part))
            .filter(|p| p.ends_with(".xlsx"));
        let current = match &wb_part {
            Some(p) => Some(
                self.edited_part(p)
                    .or_else(|| self.part_bytes(p))
                    .context("the chart's workbook is missing")?,
            ),
            None => None,
        };
        let (new_xml, cells, resize, renames) = if ex::is_chartex(xml) {
            let names = current
                .as_deref()
                .map(|b| defined_names(b))
                .unwrap_or_default();
            let old_box = ex::data_box(xml, &names);
            let (mid, clears) = ex::restructure(xml, change, &names)?;
            let (new_xml, mut cells, renames) = ex::rewrite(&mid, data, &names)?;
            let mut all = clears;
            all.append(&mut cells);
            let cells = all;
            let mut after = names.clone();
            for (k, v) in &renames {
                after.insert(k.clone(), v.clone());
            }
            let resize = old_box.zip(ex::data_box(&new_xml, &after));
            (new_xml, cells, resize, renames)
        } else {
            let old_box = data_box(xml);
            let (mid, clears) = restructure(xml, change)?;
            let (new_xml, mut cells) = rewrite_chart(&mid, data)?;
            cells.extend(clears);
            let resize = old_box.zip(data_box(&new_xml));
            (new_xml, cells, resize, Vec::new())
        };
        let workbook = match (wb_part, current) {
            (Some(p), Some(current)) => {
                let patched = patch_workbook(&current, &cells, resize, &renames)?;
                Some((p, Arc::new(patched)))
            }
            _ => None,
        };
        let edit = Arc::new(ChartEdit {
            xml: Arc::new(new_xml.into_bytes()),
            workbook,
        });
        self.record(Kind::Other, None, Focus::shape(slide, shape), true, |p| {
            let s = p.slide_mut(slide)?;
            let sh = s.shapes.get_mut(shape).context("no such shape")?;
            let old = sh.src.as_ref().context("not a chart")?;
            sh.src = Some(Arc::new(old.with_chart_edit(edit)));
            Ok(((), Focus::shape(slide, shape)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHART: &str = r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart><c:plotArea><c:barChart><c:ser><c:idx val="0"/><c:tx><c:strRef><c:f>Sheet1!$B$1</c:f><c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>2025</c:v></c:pt></c:strCache></c:strRef></c:tx><c:cat><c:strRef><c:f>Sheet1!$A$2:$A$3</c:f><c:strCache><c:ptCount val="2"/><c:pt idx="0"><c:v>Q1</c:v></c:pt><c:pt idx="1"><c:v>Q2</c:v></c:pt></c:strCache></c:strRef></c:cat><c:val><c:numRef><c:f>Sheet1!$B$2:$B$3</c:f><c:numCache><c:formatCode>General</c:formatCode><c:ptCount val="2"/><c:pt idx="0"><c:v>1.5</c:v></c:pt><c:pt idx="1"><c:v>2</c:v></c:pt></c:numCache></c:numRef></c:val></c:ser></c:barChart></c:plotArea></c:chart></c:chartSpace>"#;

    #[test]
    fn reads_and_rewrites_caches_and_refs() {
        let d = read_chart_data(CHART).unwrap();
        assert_eq!(d.categories, ["Q1", "Q2"]);
        assert_eq!(d.series[0].name, "2025");
        assert_eq!(d.series[0].values, [Some(1.5), Some(2.0)]);
        let mut e = d.clone();
        e.categories.push("Q3".into());
        e.series[0].values = vec![Some(4.0), None, Some(6.25)];
        e.series[0].name = "Sales & co".into();
        let (xml, cells) = rewrite_chart(CHART, &e).unwrap();
        assert_eq!(read_chart_data(&xml).unwrap(), e);
        assert!(xml.contains("<c:f>Sheet1!$A$2:$A$4</c:f>"), "{xml}");
        assert!(xml.contains("<c:f>Sheet1!$B$2:$B$4</c:f>"));
        assert!(xml.contains("<c:formatCode>General</c:formatCode><c:ptCount val=\"3\"/>"));
        assert_eq!(cells.len(), 3);
        assert_eq!(cells[1].0.write(), "Sheet1!$A$2:$A$4");
        assert!(rewrite_chart(CHART, &ChartData::default()).is_err());
    }

    #[test]
    fn worksheet_cells_are_set_in_place_or_added() {
        let mut s = String::from(
            r#"<worksheet><sheetData><row r="1"><c r="B1" t="s"><v>4</v></c></row><row r="2"><c r="A2" s="1" t="s"><v>0</v></c><c r="B2" s="1"><v>12.1</v></c></row></sheetData></worksheet>"#,
        );
        set_cell(&mut s, 2, 2, &CellValue::Number(9.5));
        assert!(s.contains(r#"<c r="B2" s="1"><v>9.5</v></c>"#), "{s}");
        set_cell(&mut s, 1, 2, &CellValue::Text("Q<1>".into()));
        assert!(s.contains(
            r#"<c r="A2" s="1" t="inlineStr"><is><t xml:space="preserve">Q&lt;1&gt;</t></is></c>"#
        ));
        set_cell(&mut s, 3, 1, &CellValue::Number(1.0));
        assert!(s.contains(r#"<c r="B1" t="s"><v>4</v></c><c r="C1"><v>1</v></c></row>"#));
        set_cell(&mut s, 1, 4, &CellValue::Text("Q4".into()));
        assert!(s.ends_with(r#"<row r="4"><c r="A4" t="inlineStr"><is><t xml:space="preserve">Q4</t></is></c></row></sheetData></worksheet>"#), "{s}");
        set_cell(&mut s, 1, 3, &CellValue::Number(2.0));
        assert!(
            s.contains(r#"</row><row r="3"><c r="A3"><v>2</v></c></row><row r="4">"#),
            "{s}"
        );
    }

    #[test]
    fn tables_follow_the_data() {
        let t = r#"<table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" id="1" name="Table1" displayName="Table1" ref="A1:C5" totalsRowShown="0"><autoFilter ref="A1:C5"/><tableColumns count="3"><tableColumn id="1" name=" "/><tableColumn id="2" name="2025"/><tableColumn id="3" name="2026"/></tableColumns><tableStyleInfo name="TableStyleMedium2"/></table>"#;
        let old = Ref::parse("Sheet1!$A$1:$C$5").unwrap();
        let wider = Ref::parse("Sheet1!$A$1:$D$5").unwrap();
        let header = |c: u32| (c == 4).then(|| "2027".to_owned());
        let x = resize_table(t, &old, &wider, &header).unwrap();
        assert!(x.contains(r#"ref="A1:D5""#));
        assert!(x.contains(r#"<autoFilter ref="A1:D5"/>"#));
        assert!(x.contains(r#"<tableColumns count="4">"#));
        assert!(
            x.contains(r#"<tableColumn id="4" name="2027"/></tableColumns>"#),
            "{x}"
        );
        assert!(x.contains("<tableStyleInfo"));
        let narrower = Ref::parse("Sheet1!$A$1:$B$5").unwrap();
        let header = |c: u32| (c == 2).then(|| "2026".to_owned());
        let y = resize_table(t, &old, &narrower, &header).unwrap();
        assert!(y.contains(r#"<tableColumns count="2"><tableColumn id="1" name=" "/><tableColumn id="2" name="2026"/></tableColumns>"#), "{y}");
        // A table that isn't the chart's block stays.
        let other = Ref::parse("Sheet1!$A$1:$C$9").unwrap();
        assert!(resize_table(t, &other, &wider, &header).is_none());
    }

    #[test]
    fn series_added_without_their_colours() {
        let colored = CHART.replace(
            "</c:tx><c:cat>",
            "</c:tx><c:spPr><a:solidFill xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"/></c:spPr><c:cat>",
        );
        let (x, clears) = restructure(&colored, SeriesChange::Inserted(1)).unwrap();
        assert!(clears.is_empty());
        roxmltree::Document::parse(&x).unwrap();
        assert_eq!(x.matches("<c:ser>").count(), 2);
        assert_eq!(x.matches("solidFill").count(), 1, "{x}");
        assert!(x.contains("<c:ser><c:idx val=\"1\"/><c:tx>"), "{x}");
        assert!(x.contains("<c:f>Sheet1!$C$1</c:f>") && x.contains("<c:f>Sheet1!$C$2:$C$3</c:f>"));
        let (y, clears) = restructure(&x, SeriesChange::Removed(0)).unwrap();
        assert_eq!(y.matches("<c:ser>").count(), 1);
        assert!(y.contains("<c:f>Sheet1!$B$2:$B$3</c:f>"), "{y}");
        assert_eq!(clears.len(), 2);
        assert_eq!(clears[1].0.write(), "Sheet1!$C$2:$C$3");
    }

    /// CHART with a second series in column C (orders 0 and 1).
    fn two_series() -> String {
        let one = CHART.replace(
            "<c:idx val=\"0\"/>",
            "<c:idx val=\"0\"/><c:order val=\"0\"/>",
        );
        let start = one.find("<c:ser>").unwrap();
        let end = one.find("</c:ser>").unwrap() + "</c:ser>".len();
        let second = one[start..end]
            .replace(
                "<c:idx val=\"0\"/><c:order val=\"0\"/>",
                "<c:idx val=\"1\"/><c:order val=\"1\"/>",
            )
            .replace("$B$", "$C$")
            .replace("<c:v>2025</c:v>", "<c:v>2026</c:v>");
        format!("{}{second}{}", &one[..end], &one[end..])
    }

    #[test]
    fn series_inserted_moved_and_removed_anywhere() {
        let x = two_series();
        // Before the second: it takes column C, the old second moves to D.
        let (y, clears) = restructure(&x, SeriesChange::Inserted(1)).unwrap();
        assert!(clears.is_empty());
        let doc = roxmltree::Document::parse(&y).unwrap();
        let s = sers(&doc);
        assert_eq!(s.len(), 3);
        let fs: Vec<String> = s
            .iter()
            .map(|n| formula(val_el(*n).unwrap()).unwrap())
            .collect();
        assert_eq!(
            fs,
            ["Sheet1!$B$2:$B$3", "Sheet1!$C$2:$C$3", "Sheet1!$D$2:$D$3"]
        );
        let order = |n: Node| kid(n, "order").and_then(|o| attr_i64(o, "val"));
        assert_eq!(
            s.iter().map(|n| order(*n)).collect::<Vec<_>>(),
            [Some(0), Some(1), Some(2)]
        );
        assert_eq!(kid(s[1], "idx").and_then(|o| attr_i64(o, "val")), Some(2));
        // Removing the first: the others move back a column.
        let (z, clears) = restructure(&y, SeriesChange::Removed(0)).unwrap();
        let doc = roxmltree::Document::parse(&z).unwrap();
        let fs: Vec<String> = sers(&doc)
            .iter()
            .map(|n| formula(val_el(*n).unwrap()).unwrap())
            .collect();
        assert_eq!(fs, ["Sheet1!$B$2:$B$3", "Sheet1!$C$2:$C$3"]);
        assert_eq!(clears[1].0.write(), "Sheet1!$D$2:$D$3");
        // Moving swaps the plot order and the places, not the cells.
        let (m, _) = restructure(&x, SeriesChange::Moved { from: 1, to: 0 }).unwrap();
        let doc = roxmltree::Document::parse(&m).unwrap();
        let s = sers(&doc);
        assert_eq!(formula(val_el(s[0]).unwrap()).unwrap(), "Sheet1!$C$2:$C$3");
        assert_eq!((order(s[0]), order(s[1])), (Some(0), Some(1)));
        assert_eq!(read_chart_data(&m).unwrap().series[0].name, "2026");
    }

    #[test]
    fn a_removed_category_leaves_no_stale_cells() {
        let mut d = read_chart_data(CHART).unwrap();
        d.categories.remove(0);
        d.series[0].values.remove(0);
        let (xml, cells) = rewrite_chart(CHART, &d).unwrap();
        assert!(
            xml.contains("<c:f>Sheet1!$A$2:$A$2</c:f>") || xml.contains("<c:f>Sheet1!$A$2</c:f>"),
            "{xml}"
        );
        // A3 and B3 emptied before the kept cells are written.
        let empties: Vec<String> = cells
            .iter()
            .filter(|(_, v)| v.iter().all(|c| *c == CellValue::Empty))
            .map(|(r, _)| r.write())
            .collect();
        assert_eq!(empties, ["Sheet1!$A$3", "Sheet1!$B$3"]);
        assert_eq!(read_chart_data(&xml).unwrap().categories, ["Q2"]);
    }

    #[test]
    fn refs() {
        let r = Ref::parse("'My Data'!$A$2:$A$5").unwrap();
        assert_eq!((r.c0, r.r0, r.c1, r.r1), (1, 2, 1, 5));
        assert_eq!(r.resized(2).write(), "'My Data'!$A$2:$A$3");
        let row = Ref::parse("Sheet1!$B$1:$D$1").unwrap();
        assert_eq!(row.resized(4).write(), "Sheet1!$B$1:$E$1");
        assert_eq!(col_name(28), "AB");
    }
}
