//! Slicer and timeline parts made to agree with the workbook at a save (see
//! `crate::slicer_edit`): the plan lists every slicer and slicer cache as
//! it should be, and the timelines changed in Kantor. Parts of the package
//! (carried from the original) are changed in place, only where they
//! differ: slicers deleted in Kantor leave their slicer part (and their
//! cache, its workbook extension entry and `#N/A` name, once no slicer uses
//! it), new ones are added with all of those, and settings, timeline levels
//! and timeline selections are written into the existing elements.

use std::collections::HashSet;
use std::ops::Range;

use roxmltree::{Document, Node};

use super::{
    CONTENT_TYPES, NS_MAIN, NS_R, Package, Rel, SheetKind, Skeleton, apply_edits, esc_attr,
    parse_rels, relative, rels_part, resolve, workbook_sheets, write_rels,
};

const NS_X14: &str = "http://schemas.microsoft.com/office/spreadsheetml/2009/9/main";
const NS_X15: &str = "http://schemas.microsoft.com/office/spreadsheetml/2010/11/main";
const NS_MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const MS_REL: &str = "http://schemas.microsoft.com/office/2007/relationships/";
const SLICER_CT: &str = "application/vnd.ms-excel.slicer+xml";
const SLICER_CACHE_CT: &str = "application/vnd.ms-excel.slicerCache+xml";
const HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";

/// Sheet extensions listing slicers: pivot table slicers (Excel 2010) and
/// table slicers (Excel 2013).
const EXT_SHEET_PIVOT: &str = "{A8765BA9-456A-4dab-B4F3-ACF838C121DE}";
const EXT_SHEET_TABLE: &str = "{3A4CF648-6AED-40f4-86FF-DC5316D8AED3}";
/// Workbook extensions listing slicer caches, the same two kinds.
const EXT_WB_PIVOT: &str = "{BBE1A952-AA13-448e-AADC-164F8A28A991}";
const EXT_WB_TABLE: &str = "{46BE6895-7355-4a93-B00E-2C351335B9C9}";
/// A slicer cache's "Hide items with no data".
const EXT_HIDE: &str = "{470722E0-AACD-4C17-9CDC-17EF765DBC7E}";
/// A table slicer cache's table and column.
const EXT_TABLE_CACHE: &str = "{2F2917AC-EB37-4324-AD4E-5DD8C200BD13}";
/// A pivot cache's Excel 2010 id (`x14:pivotCacheDefinition`).
const EXT_PIVOT_CACHE_ID: &str = "{725AE2AE-9491-48be-B2B4-4EB974FC3084}";

/// What a save makes the slicer and timeline parts say.
#[derive(Default)]
pub(crate) struct SlicerPlan {
    /// The workbook has (or had) slicers or timelines: else nothing is done.
    pub active: bool,
    pub slicers: Vec<SlicerOut>,
    pub caches: Vec<CacheOut>,
    /// Timelines whose level or scroll position was set in Kantor.
    pub timelines: Vec<TimelineOut>,
    /// Every timeline cache whose pivot table is present: its selection.
    pub timeline_caches: Vec<TimelineCacheOut>,
}

/// A slicer as it should be saved.
#[derive(Debug, Clone)]
pub(crate) struct SlicerOut {
    pub sheet_id: u32,
    pub name: String,
    pub cache: String,
    pub caption: String,
    pub columns: u32,
    pub row_height: i64,
    pub show_caption: bool,
    pub style: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum CacheKind {
    /// Pivot tables `(sheetId, name)` and, for a cache made in Kantor, its
    /// items (`x`, selected, no data).
    Pivot {
        pivots: Vec<(u32, String)>,
        items: Vec<(usize, bool, bool)>,
    },
    /// A table's column (its `id`).
    Table { table: String, column: u32 },
}

/// A slicer cache as it should be saved.
#[derive(Debug, Clone)]
pub(crate) struct CacheOut {
    pub name: String,
    pub source_name: String,
    pub kind: CacheKind,
    pub descending: bool,
    /// `crossFilter` (`None`: Excel's default).
    pub cross: Option<&'static str>,
    pub hide_no_data: bool,
    /// Made in Kantor: written whole when the package has none, its items
    /// kept up to date.
    pub new: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct TimelineOut {
    pub name: String,
    pub level: u32,
    pub scroll: Option<f64>,
}

#[derive(Debug, Clone)]
pub(crate) struct TimelineCacheOut {
    pub name: String,
    /// First and last day selected (`None`: all periods).
    pub selection: Option<(f64, f64)>,
    pub filter: Option<TimelineFilter>,
}

/// The pivot filter a timeline set (its cache's state names it).
#[derive(Debug, Clone)]
pub(crate) struct TimelineFilter {
    /// Its `id` in the pivot table's `<filters>`.
    pub id: usize,
    /// The pivot table's `sheetId` and name.
    pub tab: u32,
    pub pivot: String,
}

fn local<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

/// The prefix (`x14:`, or empty for the default namespace) `n` binds to
/// `ns`, if any.
fn prefix(n: Node, ns: &str) -> Option<String> {
    n.lookup_prefix(ns).map(|p| {
        if p.is_empty() {
            String::new()
        } else {
            format!("{p}:")
        }
    })
}

/// Edits setting the plain attributes `sets` of element `n` of `xml`
/// (`None` removes one).
fn attr_edits(xml: &str, n: Node, sets: &[(&str, Option<String>)]) -> Vec<(Range<usize>, String)> {
    let mut edits = Vec::new();
    let start = n.range().start;
    let tag_end = start
        + 1
        + xml[start + 1..]
            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .unwrap_or(0);
    for (name, value) in sets {
        let found = n
            .attributes()
            .find(|a| a.namespace().is_none() && a.name() == *name);
        match (found, value) {
            (Some(a), Some(v)) => {
                if a.value() != v {
                    edits.push((a.range_value(), esc_attr(v)));
                }
            }
            (Some(a), None) => {
                let r = a.range();
                let from = if xml[..r.start].ends_with(char::is_whitespace) {
                    r.start - 1
                } else {
                    r.start
                };
                edits.push((from..r.end, String::new()));
            }
            (None, Some(v)) => {
                edits.push((tag_end..tag_end, format!(" {name}=\"{}\"", esc_attr(v))))
            }
            (None, None) => {}
        }
    }
    edits
}

/// Where to insert before element `n`'s end tag (`n` not self-closing).
fn before_close(xml: &str, n: Node) -> Option<usize> {
    let r = n.range();
    if xml[r.clone()].ends_with("/>") {
        return None;
    }
    xml[..r.end].rfind("</")
}

/// Element `n` (`<a/>` or `<a>..</a>`) with `inner` added at its end.
fn append_to(xml: &str, n: Node, inner: &str) -> Vec<(Range<usize>, String)> {
    match before_close(xml, n) {
        Some(at) => vec![(at..at, inner.to_owned())],
        None => {
            let r = n.range();
            let open = &xml[r.start..r.end - 2];
            let name = &xml[r.start + 1..][..xml[r.start + 1..]
                .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
                .unwrap_or(0)];
            vec![(r, format!("{open}>{inner}</{name}>"))]
        }
    }
}

fn add_ct(pkg: &mut Package, part: &str, ct: &str) -> Result<(), String> {
    let mut types = pkg
        .text(CONTENT_TYPES)?
        .ok_or("no content types")?
        .to_owned();
    let o = format!("PartName=\"/{}\"", esc_attr(part));
    if types.contains(&o) {
        return Ok(());
    }
    let at = types.rfind("</Types>").ok_or("content types not closed")?;
    types.insert_str(
        at,
        &format!(
            "<Override PartName=\"/{}\" ContentType=\"{ct}\"/>",
            esc_attr(part)
        ),
    );
    pkg.set(CONTENT_TYPES, types.into_bytes());
    Ok(())
}

fn remove_part(pkg: &mut Package, part: &str) -> Result<(), String> {
    pkg.remove(part);
    pkg.remove(&rels_part(part));
    let mut types = pkg
        .text(CONTENT_TYPES)?
        .ok_or("no content types")?
        .to_owned();
    let o = format!("<Override PartName=\"/{}\"", esc_attr(part));
    if let Some(at) = types.find(&o)
        && let Some(end) = types[at..].find("/>")
    {
        types.replace_range(at..at + end + 2, "");
        pkg.set(CONTENT_TYPES, types.into_bytes());
    }
    Ok(())
}

fn read_rels(pkg: &Package, source: &str) -> Result<Vec<Rel>, String> {
    match pkg.text(&rels_part(source))? {
        Some(x) => parse_rels(x),
        None => Ok(Vec::new()),
    }
}

fn next_rid(rels: &[Rel]) -> String {
    super::IdAlloc::new(rels).next()
}

fn free_name(pkg: &Package, stem: &str) -> String {
    (1..)
        .map(|n| format!("{stem}{n}.xml"))
        .find(|n| !pkg.has(n))
        .expect("a free name")
}

/// How an extension lists its relationships: the list element (its
/// namespace) and the items (always `x14`).
struct ExtSpec {
    uri: &'static str,
    /// The `ext`'s own namespace declaration.
    ext_ns: (&'static str, &'static str),
    list: &'static str,
    list_ns: (&'static str, &'static str),
    item: &'static str,
}

const SHEET_PIVOT: ExtSpec = ExtSpec {
    uri: EXT_SHEET_PIVOT,
    ext_ns: ("x14", NS_X14),
    list: "slicerList",
    list_ns: ("x14", NS_X14),
    item: "slicer",
};
const SHEET_TABLE: ExtSpec = ExtSpec {
    uri: EXT_SHEET_TABLE,
    ext_ns: ("x15", NS_X15),
    list: "slicerList",
    list_ns: ("x14", NS_X14),
    item: "slicer",
};
const WB_PIVOT: ExtSpec = ExtSpec {
    uri: EXT_WB_PIVOT,
    ext_ns: ("x14", NS_X14),
    list: "slicerCaches",
    list_ns: ("x14", NS_X14),
    item: "slicerCache",
};
const WB_TABLE: ExtSpec = ExtSpec {
    uri: EXT_WB_TABLE,
    ext_ns: ("x15", NS_X15),
    list: "slicerCaches",
    list_ns: ("x15", NS_X15),
    item: "slicerCache",
};

fn is_main(n: Node, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some(NS_MAIN)
}

fn r_id<'a>(n: Node<'a, '_>) -> Option<&'a str> {
    n.attributes()
        .find(|a| {
            a.name() == "id"
                && a.namespace()
                    .is_some_and(|ns| ns.ends_with("/relationships"))
        })
        .map(|a| a.value())
}

/// A worksheet or workbook (`full`, cut to its skeleton when it has cells)
/// with the items naming relationships `gone` taken out of the slicer
/// extensions `specs` (lists left empty go, and an empty `extLst`).
fn remove_ext_items(
    full: &str,
    specs: &[&ExtSpec],
    gone: &HashSet<String>,
) -> Result<String, String> {
    if gone.is_empty() {
        return Ok(full.to_owned());
    }
    let sk = Skeleton::new(full);
    let doc = Document::parse(&sk.xml).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    let Some(list) = root.children().find(|c| is_main(*c, "extLst")) else {
        return Ok(full.to_owned());
    };
    let mut edits = Vec::new();
    let exts: Vec<Node> = list.children().filter(|c| is_main(*c, "ext")).collect();
    let mut exts_left = exts.len();
    for ext in &exts {
        let uri = ext.attribute("uri").unwrap_or_default();
        let Some(spec) = specs.iter().find(|s| s.uri == uri) else {
            continue;
        };
        let Some(l) = ext
            .descendants()
            .find(|d| d.is_element() && d.tag_name().name() == spec.list)
        else {
            continue;
        };
        let items: Vec<Node> = l
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == spec.item)
            .collect();
        let drop: Vec<&Node> = items
            .iter()
            .filter(|i| r_id(**i).is_some_and(|id| gone.contains(id)))
            .collect();
        if drop.is_empty() {
            continue;
        }
        if drop.len() == items.len() {
            edits.push((ext.range(), String::new()));
            exts_left -= 1;
        } else {
            for i in drop {
                edits.push((i.range(), String::new()));
            }
        }
    }
    if exts_left == 0 && !exts.is_empty() {
        edits = vec![(list.range(), String::new())];
    }
    let edits = edits
        .into_iter()
        .map(|(r, t)| (sk.full_offset(r.start)..sk.full_offset(r.end), t))
        .collect();
    Ok(apply_edits(full, edits))
}

/// `full` with an item naming relationship `rid` in extension `spec`
/// (made, with an `extLst`, when missing).
fn add_ext_item(full: &str, spec: &ExtSpec, rid: &str) -> Result<String, String> {
    let sk = Skeleton::new(full);
    let doc = Document::parse(&sk.xml).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    let main = prefix(root, NS_MAIN).unwrap_or_default();
    let (r_decl, r) = match prefix(root, NS_R) {
        Some(p) if !p.is_empty() => (String::new(), p),
        _ => (format!(" xmlns:r=\"{NS_R}\""), "r:".to_owned()),
    };
    let (lp, lns) = spec.list_ns;
    let item = format!(
        "<x14:{}{r_decl} xmlns:x14=\"{NS_X14}\" {r}id=\"{}\"/>",
        spec.item,
        esc_attr(rid)
    );
    let list = root.children().find(|c| is_main(*c, "extLst"));
    let ext = list.and_then(|l| {
        l.children()
            .find(|c| is_main(*c, "ext") && c.attribute("uri") == Some(spec.uri))
    });
    let edits = match (list, ext) {
        (_, Some(ext)) => {
            match ext
                .descendants()
                .find(|d| d.is_element() && d.tag_name().name() == spec.list)
            {
                Some(l) => append_to(&sk.xml, l, &item),
                None => append_to(
                    &sk.xml,
                    ext,
                    &format!(
                        "<{lp}:{} xmlns:{lp}=\"{lns}\">{item}</{lp}:{}>",
                        spec.list, spec.list
                    ),
                ),
            }
        }
        (l, None) => {
            let (ep, ens) = spec.ext_ns;
            let new_ext = format!(
                "<{main}ext uri=\"{}\" xmlns:{ep}=\"{ens}\"><{lp}:{} xmlns:{lp}=\"{lns}\">{item}</{lp}:{}></{main}ext>",
                spec.uri, spec.list, spec.list
            );
            match l {
                Some(l) => append_to(&sk.xml, l, &new_ext),
                None => append_to(
                    &sk.xml,
                    root,
                    &format!("<{main}extLst>{new_ext}</{main}extLst>"),
                ),
            }
        }
    };
    let edits = edits
        .into_iter()
        .map(|(r, t)| (sk.full_offset(r.start)..sk.full_offset(r.end), t))
        .collect();
    Ok(apply_edits(full, edits))
}

/// Workbook children after `definedNames`, in schema order.
const AFTER_NAMES: [&str; 10] = [
    "calcPr",
    "oleSize",
    "customWorkbookViews",
    "pivotCaches",
    "smartTagPr",
    "smartTagTypes",
    "webPublishing",
    "fileRecoveryPr",
    "webPublishObjects",
    "extLst",
];

/// The workbook with `#N/A` names `add` and without the `#N/A` names
/// `remove` (lower case; names with a formula are left alone).
fn edit_names(xml: &str, add: &[String], remove: &HashSet<String>) -> Result<String, String> {
    let doc = Document::parse(xml).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    let main = prefix(root, NS_MAIN).unwrap_or_default();
    let names = root.children().find(|c| is_main(*c, "definedNames"));
    let existing: Vec<Node> = names
        .map(|n| {
            n.children()
                .filter(|c| is_main(*c, "definedName"))
                .collect()
        })
        .unwrap_or_default();
    let has = |name: &str| {
        existing.iter().any(|d| {
            d.attribute("name")
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
                && d.attribute("localSheetId").is_none()
        })
    };
    let mut edits = Vec::new();
    let mut left = existing.len();
    for d in &existing {
        let name = d.attribute("name").unwrap_or_default().to_lowercase();
        if remove.contains(&name)
            && d.attribute("localSheetId").is_none()
            && d.text().unwrap_or_default().trim() == "#N/A"
        {
            edits.push((d.range(), String::new()));
            left -= 1;
        }
    }
    let new: String = add
        .iter()
        .filter(|n| !has(n))
        .map(|n| {
            format!(
                "<{main}definedName name=\"{}\">#N/A</{main}definedName>",
                esc_attr(n)
            )
        })
        .collect();
    match names {
        Some(n) if left == 0 && new.is_empty() && !existing.is_empty() => {
            edits = vec![(n.range(), String::new())];
        }
        Some(n) => {
            if !new.is_empty() {
                edits.extend(append_to(xml, n, &new));
            }
        }
        None if !new.is_empty() => {
            let block = format!("<{main}definedNames>{new}</{main}definedNames>");
            let after = root
                .children()
                .find(|c| c.is_element() && AFTER_NAMES.contains(&c.tag_name().name()));
            match after {
                Some(a) => edits.push((a.range().start..a.range().start, block)),
                None => edits.extend(append_to(xml, root, &block)),
            }
        }
        None => {}
    }
    Ok(apply_edits(xml, edits))
}

/// The `id` of table `name`'s part in the package.
fn table_id(pkg: &Package, name: &str) -> Option<u32> {
    let parts: Vec<String> = pkg
        .parts()
        .filter(|p| {
            let l = p.to_lowercase();
            l.starts_with("xl/tables/") && l.ends_with(".xml") && !l.contains("/_rels/")
        })
        .map(str::to_owned)
        .collect();
    for p in parts {
        let Ok(Some(xml)) = pkg.text(&p) else {
            continue;
        };
        let Ok(doc) = Document::parse(xml) else {
            continue;
        };
        let root = doc.root_element();
        if root
            .attribute("name")
            .or(root.attribute("displayName"))
            .is_some_and(|n| n.eq_ignore_ascii_case(name))
        {
            return root.attribute("id")?.parse().ok();
        }
    }
    None
}

/// The Excel 2010 id (`x14:pivotCacheDefinition pivotCacheId`) of the
/// cache of pivot table `name` on the sheet with `sheet_id`; a cache
/// without one gets the next free id.
fn pivot_cache_id(
    pkg: &mut Package,
    sheets: &[super::SheetEntry],
    wb: &str,
    wb_rels: &[Rel],
    (sheet_id, name): (u32, &str),
) -> Result<u32, String> {
    let sheet = sheets
        .iter()
        .find(|s| s.sheet_id == sheet_id && s.kind == SheetKind::Work)
        .ok_or_else(|| format!("sheet {sheet_id} of PivotTable {name} is missing"))?;
    let mut cache_part = None;
    for r in read_rels(pkg, &sheet.part)?
        .iter()
        .filter(|r| r.short_kind() == "pivotTable" && !r.external)
    {
        let Some(p) = resolve(&sheet.part, &r.target) else {
            continue;
        };
        let Some(xml) = pkg.text(&p)? else { continue };
        let Ok(doc) = Document::parse(xml) else {
            continue;
        };
        if doc
            .root_element()
            .attribute("name")
            .is_some_and(|n| n.eq_ignore_ascii_case(name))
        {
            cache_part = read_rels(pkg, &p)?
                .iter()
                .find(|r| r.short_kind() == "pivotCacheDefinition" && !r.external)
                .and_then(|r| resolve(&p, &r.target));
            break;
        }
    }
    let cache_part = cache_part.ok_or_else(|| format!("PivotTable {name} has no cache part"))?;
    let id_of = |xml: &str| -> Option<u32> {
        let doc = Document::parse(xml).ok()?;
        doc.descendants()
            .find(|d| {
                d.tag_name().name() == "pivotCacheDefinition"
                    && d.tag_name().namespace() == Some(NS_X14)
            })?
            .attribute("pivotCacheId")?
            .parse()
            .ok()
    };
    let xml = pkg
        .text(&cache_part)?
        .ok_or("pivot cache part is missing")?
        .to_owned();
    if let Some(id) = id_of(&xml) {
        return Ok(id);
    }
    let mut max = 0;
    for r in wb_rels
        .iter()
        .filter(|r| r.short_kind() == "pivotCacheDefinition" && !r.external)
    {
        if let Some(p) = resolve(wb, &r.target)
            && let Some(x) = pkg.text(&p)?
            && let Some(id) = id_of(x)
        {
            max = max.max(id);
        }
    }
    let id = max + 1;
    let doc = Document::parse(&xml).map_err(|e| format!("{cache_part}: {e}"))?;
    let root = doc.root_element();
    let main = prefix(root, NS_MAIN).unwrap_or_default();
    let ext = format!(
        "<{main}ext uri=\"{EXT_PIVOT_CACHE_ID}\" xmlns:x14=\"{NS_X14}\"><x14:pivotCacheDefinition pivotCacheId=\"{id}\"/></{main}ext>"
    );
    let edits = match root.children().find(|c| is_main(*c, "extLst")) {
        Some(l) => append_to(&xml, l, &ext),
        None => append_to(&xml, root, &format!("<{main}extLst>{ext}</{main}extLst>")),
    };
    pkg.set(&cache_part, apply_edits(&xml, edits).into_bytes());
    Ok(id)
}

fn slicer_element(s: &SlicerOut) -> String {
    let mut out = format!(
        "<slicer name=\"{}\" cache=\"{}\" caption=\"{}\"",
        esc_attr(&s.name),
        esc_attr(&s.cache),
        esc_attr(&s.caption)
    );
    if s.columns != 1 {
        out.push_str(&format!(" columnCount=\"{}\"", s.columns));
    }
    if !s.show_caption {
        out.push_str(" showCaption=\"0\"");
    }
    if let Some(st) = &s.style {
        out.push_str(&format!(" style=\"{}\"", esc_attr(st)));
    }
    out.push_str(&format!(" rowHeight=\"{}\"/>", s.row_height));
    out
}

fn slicer_attrs(s: &SlicerOut) -> Vec<(&'static str, Option<String>)> {
    vec![
        ("caption", Some(s.caption.clone())),
        (
            "columnCount",
            (s.columns != 1).then(|| s.columns.to_string()),
        ),
        ("showCaption", (!s.show_caption).then(|| "0".to_owned())),
        ("style", s.style.clone()),
        ("rowHeight", Some(s.row_height.to_string())),
    ]
}

/// `columnCount`'s and `showCaption`'s value as the package has it, to
/// compare with the plan (absent = default).
fn same_slicer(n: Node, s: &SlicerOut) -> bool {
    let attr = |k: &str| n.attribute(k);
    attr("caption").unwrap_or(attr("name").unwrap_or_default()) == s.caption
        && attr("columnCount")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1u32)
            == s.columns
        && !matches!(attr("showCaption"), Some("0" | "false")) == s.show_caption
        && attr("style") == s.style.as_deref()
        && attr("rowHeight")
            .and_then(|v| v.parse().ok())
            .unwrap_or(241_300i64)
            == s.row_height
}

fn hide_ext(main: &str) -> String {
    format!(
        "<{main}ext uri=\"{EXT_HIDE}\" xmlns:x15=\"{NS_X15}\"><x15:slicerCacheHideItemsWithNoData/></{main}ext>"
    )
}

/// A new slicer cache part.
fn cache_xml(c: &CacheOut, source: &str) -> String {
    let sort = if c.descending {
        " sortOrder=\"descending\""
    } else {
        ""
    };
    let cross = c
        .cross
        .map(|x| format!(" crossFilter=\"{x}\""))
        .unwrap_or_default();
    let hide = if c.hide_no_data {
        hide_ext("x:")
    } else {
        String::new()
    };
    let body = match &c.kind {
        CacheKind::Pivot { pivots, items } => {
            let list: String = pivots
                .iter()
                .map(|(tab, n)| format!("<pivotTable tabId=\"{tab}\" name=\"{}\"/>", esc_attr(n)))
                .collect();
            let items: String = items
                .iter()
                .map(|(x, s, nd)| {
                    format!(
                        "<i x=\"{x}\"{}{}/>",
                        if *s { " s=\"1\"" } else { "" },
                        if *nd { " nd=\"1\"" } else { "" }
                    )
                })
                .collect();
            let ext = if hide.is_empty() {
                String::new()
            } else {
                format!("<extLst>{hide}</extLst>")
            };
            format!(
                "<pivotTables>{list}</pivotTables><data><tabular pivotCacheId=\"{source}\"{sort}{cross}><items count=\"{}\">{items}</items></tabular></data>{ext}",
                c_items(c)
            )
        }
        CacheKind::Table { column, .. } => format!(
            "<extLst><x:ext uri=\"{EXT_TABLE_CACHE}\" xmlns:x15=\"{NS_X15}\"><x15:tableSlicerCache tableId=\"{source}\" column=\"{column}\"{sort}{cross}/></x:ext>{hide}</extLst>"
        ),
    };
    format!(
        "{HEAD}<slicerCacheDefinition xmlns=\"{NS_X14}\" xmlns:mc=\"{NS_MC}\" mc:Ignorable=\"x\" xmlns:x=\"{NS_MAIN}\" name=\"{}\" sourceName=\"{}\">{body}</slicerCacheDefinition>",
        esc_attr(&c.name),
        esc_attr(&c.source_name)
    )
}

fn c_items(c: &CacheOut) -> usize {
    match &c.kind {
        CacheKind::Pivot { items, .. } => items.len(),
        CacheKind::Table { .. } => 0,
    }
}

/// An existing cache part with the plan's sorting, items with no data and
/// (for a cache made in Kantor) items.
fn patch_cache(xml: &str, c: &CacheOut) -> Result<String, String> {
    let mut xml = xml.to_owned();
    if c.new
        && let CacheKind::Pivot { items, .. } = &c.kind
        && let Some(x) = crate::slicer::with_items(&xml, items)
    {
        xml = x;
    }
    let doc = Document::parse(&xml).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    let mut edits = Vec::new();
    let target = root
        .descendants()
        .find(|d| d.is_element() && matches!(d.tag_name().name(), "tabular" | "tableSlicerCache"));
    if let Some(t) = target {
        edits.extend(attr_edits(
            &xml,
            t,
            &[
                ("sortOrder", c.descending.then(|| "descending".to_owned())),
                ("crossFilter", c.cross.map(str::to_owned)),
            ],
        ));
    }
    let hide = root.descendants().find(|d| {
        d.is_element() && d.tag_name().name() == "ext" && d.attribute("uri") == Some(EXT_HIDE)
    });
    match (hide, c.hide_no_data) {
        (Some(h), false) => {
            let list = h.parent().filter(|p| p.tag_name().name() == "extLst");
            let only = list.is_some_and(|l| l.children().filter(|c| c.is_element()).count() == 1);
            match list {
                Some(l) if only => edits.push((l.range(), String::new())),
                _ => edits.push((h.range(), String::new())),
            }
        }
        (None, true) => {
            let main = prefix(root, NS_MAIN).unwrap_or_else(|| "x:".to_owned());
            let decl = if prefix(root, NS_MAIN).is_none() {
                format!(" xmlns:x=\"{NS_MAIN}\"")
            } else {
                String::new()
            };
            let x14 = prefix(root, NS_X14).unwrap_or_default();
            let ext =
                hide_ext(&main).replacen(&format!("<{main}ext "), &format!("<{main}ext{decl} "), 1);
            match local(root, "extLst") {
                Some(l) => edits.extend(append_to(&xml, l, &ext)),
                None => edits.extend(append_to(
                    &xml,
                    root,
                    &format!("<{x14}extLst>{ext}</{x14}extLst>"),
                )),
            }
        }
        _ => {}
    }
    Ok(apply_edits(&xml, edits))
}

/// What a cache part says about the plan's settings: unchanged?
fn same_cache(xml: &str, c: &CacheOut) -> bool {
    let Ok(doc) = Document::parse(xml) else {
        return true;
    };
    let root = doc.root_element();
    let t = root
        .descendants()
        .find(|d| d.is_element() && matches!(d.tag_name().name(), "tabular" | "tableSlicerCache"));
    let sort = t.and_then(|t| t.attribute("sortOrder")) == Some("descending");
    let cross = t.and_then(|t| t.attribute("crossFilter"));
    let cross = match cross {
        Some("showItemsWithDataAtTop") | None => None,
        Some(x) => Some(x),
    };
    let hide = root
        .descendants()
        .any(|d| d.tag_name().name() == "slicerCacheHideItemsWithNoData");
    sort == c.descending && cross == c.cross && hide == c.hide_no_data && !c.new
}

/// Make the package's slicers, slicer caches and timelines agree with
/// `plan`.
pub(crate) fn write(pkg: &mut Package, plan: &SlicerPlan) -> Result<(), String> {
    if !plan.active {
        return Ok(());
    }
    let root_rels = parse_rels(pkg.text("_rels/.rels")?.ok_or("no _rels/.rels")?)?;
    let read = |p: &str| Ok(pkg.text(p)?.map(str::to_owned));
    let (wb, _, _, sheets) = workbook_sheets(&root_rels, &read)?;
    let wanted = |sheet_id: u32, name: &str| {
        plan.slicers
            .iter()
            .find(|s| s.sheet_id == sheet_id && s.name.eq_ignore_ascii_case(name))
    };
    // Slicers: each sheet's slicer parts.
    let mut present: HashSet<(u32, String)> = HashSet::new();
    for sheet in sheets.iter().filter(|s| s.kind == SheetKind::Work) {
        let mut rels = read_rels(pkg, &sheet.part)?;
        let mut gone_rels = HashSet::new();
        for r in rels
            .iter()
            .filter(|r| r.short_kind() == "slicer" && !r.external)
        {
            let Some(part) = resolve(&sheet.part, &r.target) else {
                continue;
            };
            let Some(xml) = pkg.text(&part)?.map(str::to_owned) else {
                continue;
            };
            let doc = Document::parse(&xml).map_err(|e| format!("{part}: {e}"))?;
            let list: Vec<Node> = doc
                .root_element()
                .children()
                .filter(|c| c.is_element() && c.tag_name().name() == "slicer")
                .collect();
            let mut edits = Vec::new();
            let mut left = 0;
            for n in &list {
                let name = n.attribute("name").unwrap_or_default();
                match wanted(sheet.sheet_id, name) {
                    None => edits.push((n.range(), String::new())),
                    Some(s) => {
                        left += 1;
                        present.insert((sheet.sheet_id, name.to_lowercase()));
                        if !same_slicer(*n, s) {
                            edits.extend(attr_edits(&xml, *n, &slicer_attrs(s)));
                        }
                    }
                }
            }
            if left == 0 {
                remove_part(pkg, &part)?;
                gone_rels.insert(r.id.clone());
            } else if !edits.is_empty() {
                pkg.set(&part, apply_edits(&xml, edits).into_bytes());
            }
        }
        if !gone_rels.is_empty() {
            rels.retain(|r| !gone_rels.contains(&r.id));
            pkg.set(&rels_part(&sheet.part), write_rels(&rels).into_bytes());
            let xml = pkg.take_text(&sheet.part)?.ok_or("sheet part is missing")?;
            let new = remove_ext_items(&xml, &[&SHEET_PIVOT, &SHEET_TABLE], &gone_rels)?;
            pkg.set(&sheet.part, new.into_bytes());
        }
    }
    // New slicers: one part per sheet and kind.
    let cache_kind = |name: &str| {
        plan.caches
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
            .map(|c| matches!(c.kind, CacheKind::Table { .. }))
    };
    for sheet in sheets.iter().filter(|s| s.kind == SheetKind::Work) {
        for table in [false, true] {
            let add: Vec<&SlicerOut> = plan
                .slicers
                .iter()
                .filter(|s| s.sheet_id == sheet.sheet_id)
                .filter(|s| !present.contains(&(s.sheet_id, s.name.to_lowercase())))
                .filter(|s| cache_kind(&s.cache) == Some(table))
                .collect();
            if add.is_empty() {
                continue;
            }
            let part = free_name(pkg, "xl/slicers/slicer");
            let body: String = add.iter().map(|s| slicer_element(s)).collect();
            pkg.set(
                &part,
                format!(
                    "{HEAD}<slicers xmlns=\"{NS_X14}\" xmlns:mc=\"{NS_MC}\" mc:Ignorable=\"x\" xmlns:x=\"{NS_MAIN}\">{body}</slicers>"
                )
                .into_bytes(),
            );
            add_ct(pkg, &part, SLICER_CT)?;
            let mut rels = read_rels(pkg, &sheet.part)?;
            let rid = next_rid(&rels);
            rels.push(Rel {
                id: rid.clone(),
                kind: format!("{MS_REL}slicer"),
                target: relative(&sheet.part, &part),
                external: false,
            });
            pkg.set(&rels_part(&sheet.part), write_rels(&rels).into_bytes());
            let xml = pkg.take_text(&sheet.part)?.ok_or("sheet part is missing")?;
            let spec = if table { &SHEET_TABLE } else { &SHEET_PIVOT };
            pkg.set(&sheet.part, add_ext_item(&xml, spec, &rid)?.into_bytes());
        }
    }
    // Caches: the package's, then new ones.
    let used: HashSet<String> = plan
        .slicers
        .iter()
        .map(|s| s.cache.to_lowercase())
        .collect();
    let mut wb_rels = read_rels(pkg, &wb)?;
    let mut gone_rels = HashSet::new();
    let mut gone_names = HashSet::new();
    let mut have = HashSet::new();
    for r in wb_rels
        .iter()
        .filter(|r| r.short_kind() == "slicerCache" && !r.external)
    {
        let Some(part) = resolve(&wb, &r.target) else {
            continue;
        };
        let Some(xml) = pkg.text(&part)?.map(str::to_owned) else {
            continue;
        };
        let doc = Document::parse(&xml).map_err(|e| format!("{part}: {e}"))?;
        let name = doc
            .root_element()
            .attribute("name")
            .unwrap_or_default()
            .to_owned();
        let want = plan
            .caches
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(&name));
        match want {
            Some(c) if used.contains(&name.to_lowercase()) => {
                have.insert(name.to_lowercase());
                if !same_cache(&xml, c) {
                    pkg.set(&part, patch_cache(&xml, c)?.into_bytes());
                }
            }
            _ => {
                remove_part(pkg, &part)?;
                gone_rels.insert(r.id.clone());
                gone_names.insert(name.to_lowercase());
            }
        }
    }
    let mut added: Vec<(String, bool)> = Vec::new();
    for c in plan
        .caches
        .iter()
        .filter(|c| used.contains(&c.name.to_lowercase()) && !have.contains(&c.name.to_lowercase()))
    {
        let source = match &c.kind {
            CacheKind::Pivot { pivots, .. } => {
                let (tab, name) = pivots
                    .first()
                    .ok_or("a slicer cache without a PivotTable")?;
                pivot_cache_id(pkg, &sheets, &wb, &wb_rels, (*tab, name))?
            }
            CacheKind::Table { table, .. } => {
                table_id(pkg, table).ok_or_else(|| format!("table {table} has no part"))?
            }
        };
        let part = free_name(pkg, "xl/slicerCaches/slicerCache");
        pkg.set(&part, cache_xml(c, &source.to_string()).into_bytes());
        add_ct(pkg, &part, SLICER_CACHE_CT)?;
        let rid = next_rid(&wb_rels);
        wb_rels.push(Rel {
            id: rid.clone(),
            kind: format!("{MS_REL}slicerCache"),
            target: relative(&wb, &part),
            external: false,
        });
        added.push((rid, matches!(c.kind, CacheKind::Table { .. })));
    }
    // Every cache in use has its `#N/A` name (IronCalc only writes the
    // names it read from the file).
    let names: Vec<String> = plan
        .caches
        .iter()
        .filter(|c| used.contains(&c.name.to_lowercase()))
        .map(|c| c.name.clone())
        .collect();
    let mut xml = pkg.text(&wb)?.ok_or("workbook part is missing")?.to_owned();
    if !gone_rels.is_empty() || !added.is_empty() {
        wb_rels.retain(|r| !gone_rels.contains(&r.id));
        pkg.set(&rels_part(&wb), write_rels(&wb_rels).into_bytes());
        xml = remove_ext_items(&xml, &[&WB_PIVOT, &WB_TABLE], &gone_rels)?;
        for (rid, table) in &added {
            xml = add_ext_item(&xml, if *table { &WB_TABLE } else { &WB_PIVOT }, rid)?;
        }
    }
    let named = edit_names(&xml, &names, &gone_names)?;
    if named != xml || !gone_rels.is_empty() || !added.is_empty() {
        pkg.set(&wb, named.into_bytes());
    }
    write_timelines(pkg, &sheets, &wb, plan)
}

/// Timelines' levels and their caches' selections.
fn write_timelines(
    pkg: &mut Package,
    sheets: &[super::SheetEntry],
    wb: &str,
    plan: &SlicerPlan,
) -> Result<(), String> {
    if !plan.timelines.is_empty() {
        for sheet in sheets.iter().filter(|s| s.kind == SheetKind::Work) {
            for r in read_rels(pkg, &sheet.part)?
                .iter()
                .filter(|r| r.short_kind() == "timeline" && !r.external)
            {
                let Some(part) = resolve(&sheet.part, &r.target) else {
                    continue;
                };
                let Some(xml) = pkg.text(&part)?.map(str::to_owned) else {
                    continue;
                };
                let doc = Document::parse(&xml).map_err(|e| format!("{part}: {e}"))?;
                let mut edits = Vec::new();
                for n in doc
                    .root_element()
                    .children()
                    .filter(|c| c.is_element() && c.tag_name().name() == "timeline")
                {
                    let name = n.attribute("name").unwrap_or_default();
                    let Some(t) = plan
                        .timelines
                        .iter()
                        .find(|t| t.name.eq_ignore_ascii_case(name))
                    else {
                        continue;
                    };
                    let mut sets = vec![
                        ("level", Some(t.level.to_string())),
                        ("selectionLevel", Some(t.level.to_string())),
                    ];
                    if let Some(s) = t.scroll {
                        sets.push(("scrollPosition", Some(crate::pivot::iso_of_serial(s))));
                    }
                    edits.extend(attr_edits(&xml, n, &sets));
                }
                if !edits.is_empty() {
                    pkg.set(&part, apply_edits(&xml, edits).into_bytes());
                }
            }
        }
    }
    if plan.timeline_caches.is_empty() {
        return Ok(());
    }
    let wb_rels = read_rels(pkg, wb)?;
    for r in wb_rels
        .iter()
        .filter(|r| r.short_kind() == "timelineCache" && !r.external)
    {
        let Some(part) = resolve(wb, &r.target) else {
            continue;
        };
        let Some(xml) = pkg.text(&part)?.map(str::to_owned) else {
            continue;
        };
        if let Some(new) = patch_timeline_cache(&xml, plan)? {
            pkg.set(&part, new.into_bytes());
        }
    }
    Ok(())
}

/// A timeline cache part with the plan's selection, if it differs.
fn patch_timeline_cache(xml: &str, plan: &SlicerPlan) -> Result<Option<String>, String> {
    let doc = Document::parse(xml).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    let name = root.attribute("name").unwrap_or_default();
    let Some(t) = plan
        .timeline_caches
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(name))
    else {
        return Ok(None);
    };
    let Some(state) = local(root, "state") else {
        return Ok(None);
    };
    let day = |n: Node, a: &str| {
        n.attribute(a)
            .and_then(crate::pivot::serial_of_iso)
            .map(f64::floor)
    };
    let sel = local(state, "selection");
    let was = sel.and_then(|s| Some((day(s, "startDate")?, day(s, "endDate")?)));
    if was == t.selection {
        return Ok(None);
    }
    let p = prefix(root, NS_X15).unwrap_or_default();
    let mut edits = Vec::new();
    let filter = t.filter.as_ref().filter(|_| t.selection.is_some());
    edits.extend(attr_edits(
        xml,
        state,
        &[
            (
                "filterType",
                Some(
                    if filter.is_some() {
                        "dateBetween"
                    } else {
                        "unknown"
                    }
                    .to_owned(),
                ),
            ),
            ("filterId", filter.map(|f| f.id.to_string())),
            ("filterTabId", filter.map(|f| f.tab.to_string())),
            ("filterPivotName", filter.map(|f| f.pivot.clone())),
        ],
    ));
    if let Some(s) = sel {
        edits.push((s.range(), String::new()));
    }
    if let Some((a, b)) = t.selection {
        let new = format!(
            "<{p}selection startDate=\"{}\" endDate=\"{}\"/>",
            crate::pivot::iso_of_serial(a),
            crate::pivot::iso_of_serial(b)
        );
        // The first child of the state.
        match state.children().find(|c| c.is_element()) {
            Some(first) => edits.push((first.range().start..first.range().start, new)),
            None => edits.extend(append_to(xml, state, &new)),
        }
    }
    // A copy of the filter Kantor wrote before (not in the schema).
    if let Some(old) = local(root, "pivotFilter") {
        edits.push((old.range(), String::new()));
    }
    Ok(Some(apply_edits(xml, edits)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_items_are_added_and_removed() {
        let wb = format!(
            "<workbook xmlns=\"{NS_MAIN}\" xmlns:r=\"{NS_R}\"><sheets/><calcPr/></workbook>"
        );
        let one = add_ext_item(&wb, &WB_TABLE, "rId9").unwrap();
        assert!(
            one.contains("<extLst><ext uri=\"{46BE6895-7355-4a93-B00E-2C351335B9C9}\""),
            "{one}"
        );
        assert!(Document::parse(&one).is_ok());
        let two = add_ext_item(&one, &WB_TABLE, "rId10").unwrap();
        assert_eq!(two.matches("x14:slicerCache ").count(), 2, "{two}");
        let back =
            remove_ext_items(&two, &[&WB_TABLE], &HashSet::from(["rId9".to_owned()])).unwrap();
        assert_eq!(back.matches("x14:slicerCache ").count(), 1);
        let none =
            remove_ext_items(&back, &[&WB_TABLE], &HashSet::from(["rId10".to_owned()])).unwrap();
        assert!(!none.contains("extLst"), "{none}");
        let named = edit_names(&none, &["Slicer_A".into()], &HashSet::new()).unwrap();
        assert!(named.contains("<definedNames><definedName name=\"Slicer_A\">#N/A</definedName></definedNames><calcPr/>"), "{named}");
        let unnamed = edit_names(&named, &[], &HashSet::from(["slicer_a".to_owned()])).unwrap();
        assert!(!unnamed.contains("definedNames"));
    }
}
