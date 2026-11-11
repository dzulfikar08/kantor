//! Shared by the pivot tests: reading a package's parts and the structural
//! checks Excel makes first on pivot parts.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::Path;

use roxmltree::{Document, Node};

pub const MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
pub const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

pub fn parts(path: &Path) -> BTreeMap<String, String> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).unwrap();
        let mut s = String::new();
        if e.read_to_string(&mut s).is_ok() {
            out.insert(e.name().unwrap().into_owned(), s);
        }
    }
    out
}

pub fn el<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

pub fn els<'a, 'i>(n: Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = Node<'a, 'i>> + 'a {
    n.children()
        .filter(move |c| c.is_element() && c.tag_name().name() == name)
}

/// The target of each relationship of `source`, resolved to a part name.
pub fn targets(p: &BTreeMap<String, String>, source: &str) -> BTreeMap<String, (String, String)> {
    let (dir, file) = source.rsplit_once('/').unwrap_or(("", source));
    let rels = format!(
        "{}{}_rels/{file}.rels",
        dir,
        if dir.is_empty() { "" } else { "/" }
    );
    let Some(xml) = p.get(&rels) else {
        return BTreeMap::new();
    };
    let doc = Document::parse(xml).unwrap();
    let mut out = BTreeMap::new();
    for r in doc.root_element().children().filter(|n| n.is_element()) {
        let target = r.attribute("Target").unwrap();
        let mut path: Vec<&str> = if target.starts_with('/') {
            Vec::new()
        } else {
            dir.split('/').filter(|s| !s.is_empty()).collect()
        };
        for seg in target.trim_start_matches('/').split('/') {
            match seg {
                ".." => {
                    path.pop();
                }
                "." => {}
                s => path.push(s),
            }
        }
        let kind = r.attribute("Type").unwrap().rsplit('/').next().unwrap();
        out.insert(
            r.attribute("Id").unwrap().to_owned(),
            (kind.to_owned(), path.join("/")),
        );
    }
    out
}

/// What Excel checks first in pivot parts (ECMA-376 Part 1, 18.10): the
/// parts are well-formed, have their required attributes, `count`s match
/// their children, pivot tables point at a listed cache with as many fields,
/// records have one value per cache field, and content types are declared.
pub fn pivot_problems(p: &BTreeMap<String, String>) -> Vec<String> {
    let mut out = Vec::new();
    let ct = &p["[Content_Types].xml"];
    let wb = Document::parse(&p["xl/workbook.xml"]).unwrap();
    let wb_rels = targets(p, "xl/workbook.xml");
    let mut caches: BTreeMap<String, String> = BTreeMap::new();
    if let Some(list) = el(wb.root_element(), "pivotCaches") {
        for c in els(list, "pivotCache") {
            let id = c.attribute("cacheId").unwrap_or_default().to_owned();
            if caches.contains_key(&id) {
                out.push(format!("pivot cache id {id} is listed twice"));
            }
            let rid = c.attribute((REL, "id")).unwrap_or_default();
            match wb_rels.get(rid) {
                Some((kind, part)) if kind == "pivotCacheDefinition" && p.contains_key(part) => {
                    caches.insert(id, part.clone());
                }
                _ => out.push(format!("pivot cache {id}: relationship {rid} is broken")),
            }
        }
    }
    let count_ok = |n: Node, child: &str, what: &str, out: &mut Vec<String>| {
        if let Some(c) = n.attribute("count") {
            let have = els(n, child).count();
            if c.parse::<usize>().ok() != Some(have) {
                out.push(format!(
                    "{what}: <{}> count={c}, has {have}",
                    n.tag_name().name()
                ));
            }
        }
    };
    let mut cache_fields: BTreeMap<String, usize> = BTreeMap::new();
    let mut shared_items: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for part in caches.values() {
        if !ct.contains(&format!("PartName=\"/{part}\"")) {
            out.push(format!("{part}: no content type"));
        }
        let doc = match Document::parse(&p[part]) {
            Ok(d) => d,
            Err(e) => {
                out.push(format!("{part}: {e}"));
                continue;
            }
        };
        let root = doc.root_element();
        if root.tag_name().namespace() != Some(MAIN)
            || root.tag_name().name() != "pivotCacheDefinition"
        {
            out.push(format!("{part}: not a pivot cache definition"));
        }
        let Some(fields) = el(root, "cacheFields") else {
            out.push(format!("{part}: no cacheFields"));
            continue;
        };
        count_ok(fields, "cacheField", part, &mut out);
        let n = els(fields, "cacheField").count();
        let shared: Vec<usize> = els(fields, "cacheField")
            .map(|f| {
                el(f, "sharedItems").map_or(0, |s| s.children().filter(|c| c.is_element()).count())
            })
            .collect();
        for f in els(fields, "cacheField") {
            if f.attribute("name").is_none() {
                out.push(format!("{part}: cacheField without name"));
            }
            if let Some(items) = el(f, "sharedItems")
                && let Some(c) = items.attribute("count")
            {
                let have = items.children().filter(|c| c.is_element()).count();
                if c.parse::<usize>().ok() != Some(have) {
                    out.push(format!("{part}: sharedItems count={c}, has {have}"));
                }
            }
        }
        cache_fields.insert(part.clone(), n);
        shared_items.insert(part.clone(), shared.clone());
        match el(root, "cacheSource") {
            Some(s) if s.attribute("type").is_some() => {}
            _ => out.push(format!("{part}: cacheSource without type")),
        }
        if let Some(rid) = root.attribute((REL, "id")) {
            let rec = targets(p, part).get(rid).cloned();
            match rec {
                Some((kind, rp)) if kind == "pivotCacheRecords" && p.contains_key(&rp) => {
                    let rdoc = Document::parse(&p[&rp]).unwrap();
                    let rroot = rdoc.root_element();
                    count_ok(rroot, "r", &rp, &mut out);
                    let records = els(rroot, "r").count();
                    if let Some(rc) = root.attribute("recordCount")
                        && rc.parse::<usize>().ok() != Some(records)
                    {
                        out.push(format!("{part}: recordCount={rc}, records has {records}"));
                    }
                    for r in els(rroot, "r") {
                        let k = r.children().filter(|c| c.is_element()).count();
                        if k != n {
                            out.push(format!("{rp}: a record has {k} values for {n} fields"));
                            break;
                        }
                        let bad =
                            r.children()
                                .filter(|c| c.is_element())
                                .enumerate()
                                .find(|(f, v)| {
                                    v.tag_name().name() == "x"
                                        && v.attribute("v")
                                            .unwrap_or("0")
                                            .parse::<usize>()
                                            .unwrap_or(usize::MAX)
                                            >= shared[*f]
                                });
                        if let Some((f, _)) = bad {
                            out.push(format!(
                                "{rp}: a record points past field {f}'s shared items"
                            ));
                            break;
                        }
                    }
                    if !ct.contains(&format!("PartName=\"/{rp}\"")) {
                        out.push(format!("{rp}: no content type"));
                    }
                }
                _ => out.push(format!("{part}: records relationship {rid} is broken")),
            }
        }
    }
    let mut used: BTreeMap<String, usize> = BTreeMap::new();
    for (sheet, _) in p
        .iter()
        .filter(|(n, _)| n.starts_with("xl/worksheets/sheet"))
    {
        for (kind, part) in targets(p, sheet).into_values() {
            if kind == "pivotTable" {
                *used.entry(part).or_default() += 1;
            }
        }
    }
    for (part, n) in &used {
        if *n != 1 || !p.contains_key(part) {
            out.push(format!("{part}: linked from {n} sheets (or missing)"));
        }
    }
    for (name, xml) in p {
        if name.starts_with("xl/pivotTables/") && name.ends_with(".xml") && !used.contains_key(name)
        {
            out.push(format!("{name}: no sheet links to it"));
        }
        if !(name.starts_with("xl/pivotTables/") && name.ends_with(".xml")) {
            continue;
        }
        if !ct.contains(&format!("PartName=\"/{name}\"")) {
            out.push(format!("{name}: no content type"));
        }
        let doc = match Document::parse(xml) {
            Ok(d) => d,
            Err(e) => {
                out.push(format!("{name}: {e}"));
                continue;
            }
        };
        let root = doc.root_element();
        for a in ["name", "cacheId", "dataCaption"] {
            if root.attribute(a).is_none() {
                out.push(format!("{name}: no {a}"));
            }
        }
        let cache_id = root.attribute("cacheId").unwrap_or_default();
        let Some(cache) = caches.get(cache_id) else {
            out.push(format!("{name}: cache {cache_id} is not in the workbook"));
            continue;
        };
        match targets(p, name)
            .values()
            .find(|(k, _)| k == "pivotCacheDefinition")
        {
            Some((_, part)) if part == cache => {}
            other => out.push(format!(
                "{name}: points at {other:?}, workbook lists {cache}"
            )),
        }
        match el(root, "location") {
            Some(l) => {
                for a in ["ref", "firstHeaderRow", "firstDataRow", "firstDataCol"] {
                    if l.attribute(a).is_none() {
                        out.push(format!("{name}: location without {a}"));
                    }
                }
            }
            None => out.push(format!("{name}: no location")),
        }
        if let Some(f) = el(root, "pivotFields") {
            count_ok(f, "pivotField", name, &mut out);
            let n = els(f, "pivotField").count();
            if Some(&n) != cache_fields.get(cache) {
                out.push(format!(
                    "{name}: {n} pivot fields, cache has {:?}",
                    cache_fields.get(cache)
                ));
            }
            let shared = shared_items.get(cache).cloned().unwrap_or_default();
            for (k, pf) in els(f, "pivotField").enumerate() {
                if let Some(items) = el(pf, "items") {
                    count_ok(items, "item", name, &mut out);
                    for it in els(items, "item") {
                        if let Some(x) = it.attribute("x").and_then(|x| x.parse::<usize>().ok())
                            && x >= shared.get(k).copied().unwrap_or(0)
                        {
                            out.push(format!(
                                "{name}: field {k} item x={x} past its shared items"
                            ));
                        }
                    }
                }
            }
            if let Some(d) = el(root, "dataFields") {
                for df in els(d, "dataField") {
                    let fld = df.attribute("fld").and_then(|x| x.parse::<usize>().ok());
                    if fld.is_none_or(|x| x >= n) {
                        out.push(format!("{name}: dataField fld {fld:?} out of range"));
                    }
                }
            }
        }
        for (list, child) in [
            ("rowFields", "field"),
            ("colFields", "field"),
            ("rowItems", "i"),
            ("colItems", "i"),
            ("pageFields", "pageField"),
            ("dataFields", "dataField"),
        ] {
            if let Some(l) = el(root, list) {
                count_ok(l, child, name, &mut out);
            }
        }
    }
    out
}
