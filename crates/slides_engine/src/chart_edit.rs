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
//! Series can't be added or removed yet; categories can (at the end).

use std::io::{Cursor, Read as _, Write as _};
use std::ops::Range;
use std::sync::Arc;

use anyhow::{Context as _, bail};
use roxmltree::Node;

use crate::frame::NS_C;
use crate::history::{Focus, Kind};
use crate::presentation::Presentation;
use crate::xml::*;

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

/// The data a chart part shows.
pub fn read_chart_data(xml: &str) -> Option<ChartData> {
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

/// The chart part with `data` in its caches; and the workbook cells it
/// references with their new values.
fn rewrite_chart(xml: &str, data: &ChartData) -> anyhow::Result<(String, Vec<CellWrites>)> {
    let doc = roxmltree::Document::parse(xml).context("chart XML")?;
    let sers = sers(&doc);
    if sers.len() != data.series.len() {
        bail!("Series can't be added or removed yet");
    }
    let n = data.categories.len();
    if n == 0 {
        bail!("A chart needs at least one category");
    }
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut cells: Vec<CellWrites> = Vec::new();
    let fix_ref = |el: Node, edits: &mut Vec<(Range<usize>, String)>| -> Option<Ref> {
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
        Some(new)
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
            if let Some(r) = fix_ref(cat, &mut edits) {
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
            if let Some(r) = fix_ref(val, &mut edits) {
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

/// The workbook with the referenced cells set.
fn patch_workbook(bytes: &[u8], cells: &[CellWrites]) -> anyhow::Result<Vec<u8>> {
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
        let (new_xml, cells) = rewrite_chart(xml, data)?;
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
        let workbook = match wb_part {
            Some(p) => {
                let current = self
                    .edited_part(&p)
                    .or_else(|| self.part_bytes(&p))
                    .context("the chart's workbook is missing")?;
                let patched = patch_workbook(&current, &cells)?;
                Some((p, Arc::new(patched)))
            }
            None => None,
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
    fn refs() {
        let r = Ref::parse("'My Data'!$A$2:$A$5").unwrap();
        assert_eq!((r.c0, r.r0, r.c1, r.r1), (1, 2, 1, 5));
        assert_eq!(r.resized(2).write(), "'My Data'!$A$2:$A$3");
        let row = Ref::parse("Sheet1!$B$1:$D$1").unwrap();
        assert_eq!(row.resized(4).write(), "Sheet1!$B$1:$E$1");
        assert_eq!(col_name(28), "AB");
    }
}
