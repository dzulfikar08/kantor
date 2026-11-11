//! Pivot tables: carried through saves with their caches, moved with
//! inserted and deleted rows and columns (file 21 of the corpus, written by
//! `scripts/corpus/gen_pivot_xlsx.py`).

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use roxmltree::{Document, Node};
use sheets_engine::Workbook;

const MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/xlsx/21_pivot_tables.xlsx")
}

fn parts(path: &Path) -> BTreeMap<String, String> {
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

fn save(wb: &mut Workbook, dir: &Path, name: &str) -> BTreeMap<String, String> {
    let path = dir.join(name);
    wb.save_as(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        sheets_engine::package_problems(&bytes),
        Vec::<String>::new()
    );
    Workbook::open(&path).unwrap();
    let p = parts(&path);
    assert_eq!(pivot_problems(&p), Vec::<String>::new());
    p
}

fn sheet_index(wb: &Workbook, name: &str) -> u32 {
    wb.sheet_names().iter().position(|n| n == name).unwrap() as u32
}

fn el<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

fn els<'a, 'i>(n: Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = Node<'a, 'i>> + 'a {
    n.children()
        .filter(move |c| c.is_element() && c.tag_name().name() == name)
}

/// The target of each relationship of `source`, resolved to a part name.
fn targets(p: &BTreeMap<String, String>, source: &str) -> BTreeMap<String, (String, String)> {
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
fn pivot_problems(p: &BTreeMap<String, String>) -> Vec<String> {
    let mut out = Vec::new();
    let ct = &p["[Content_Types].xml"];
    let wb = Document::parse(&p["xl/workbook.xml"]).unwrap();
    let wb_rels = targets(p, "xl/workbook.xml");
    let mut caches: BTreeMap<String, String> = BTreeMap::new();
    if let Some(list) = el(wb.root_element(), "pivotCaches") {
        for c in els(list, "pivotCache") {
            let id = c.attribute("cacheId").unwrap_or_default().to_owned();
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
        for f in els(fields, "cacheField") {
            if f.attribute("name").is_none() {
                out.push(format!("{part}: cacheField without name"));
            }
            if let Some(items) = el(f, "sharedItems") {
                if let Some(c) = items.attribute("count") {
                    let have = items.children().filter(|c| c.is_element()).count();
                    if c.parse::<usize>().ok() != Some(have) {
                        out.push(format!("{part}: sharedItems count={c}, has {have}"));
                    }
                }
            }
        }
        cache_fields.insert(part.clone(), n);
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
                    }
                    if !ct.contains(&format!("PartName=\"/{rp}\"")) {
                        out.push(format!("{rp}: no content type"));
                    }
                }
                _ => out.push(format!("{part}: records relationship {rid} is broken")),
            }
        }
    }
    for (name, xml) in p {
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
            for pf in els(f, "pivotField") {
                if let Some(items) = el(pf, "items") {
                    count_ok(items, "item", name, &mut out);
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

fn location(p: &BTreeMap<String, String>, part: &str) -> String {
    let doc = Document::parse(&p[part]).unwrap();
    el(doc.root_element(), "location")
        .unwrap()
        .attribute("ref")
        .unwrap()
        .to_owned()
}

fn source(p: &BTreeMap<String, String>, part: &str) -> (Option<String>, Option<String>) {
    let doc = Document::parse(&p[part]).unwrap();
    let s = doc
        .descendants()
        .find(|n| n.tag_name().name() == "worksheetSource")
        .unwrap();
    (
        s.attribute("ref").map(str::to_owned),
        s.attribute("name").map(str::to_owned),
    )
}

#[test]
fn the_fixture_is_consistent() {
    let p = parts(&fixture());
    assert_eq!(pivot_problems(&p), Vec::<String>::new());
}

#[test]
fn pivot_tables_survive_a_save() {
    let dir = tempfile::tempdir().unwrap();
    let orig = parts(&fixture());
    let mut wb = Workbook::open(fixture()).unwrap();
    let p = save(&mut wb, dir.path(), "a.xlsx");
    assert!(
        wb.save_notes()
            .iter()
            .all(|n| !n.to_lowercase().contains("pivot")),
        "{:?}",
        wb.save_notes()
    );
    for k in 1..=3 {
        let part = format!("xl/pivotTables/pivotTable{k}.xml");
        assert_eq!(p[&part], orig[&part], "{part} is carried as it was");
    }
    for k in 1..=2 {
        for kind in ["pivotCacheDefinition", "pivotCacheRecords"] {
            let part = format!("xl/pivotCache/{kind}{k}.xml");
            assert_eq!(p[&part], orig[&part]);
        }
    }
    let wbx = &p["xl/workbook.xml"];
    assert!(
        wbx.contains(r#"<pivotCache cacheId="1""#) && wbx.contains(r#"<pivotCache cacheId="2""#)
    );
    // The list comes after calcPr, as CT_Workbook orders it.
    assert!(wbx.find("<calcPr").unwrap() < wbx.find("<pivotCaches>").unwrap());
    assert_eq!(location(&p, "xl/pivotTables/pivotTable1.xml"), "A3:E8");
    // The pivot's cells are ordinary cells.
    let s = sheet_index(&wb, "Pivot");
    assert_eq!(wb.display_value(s, 4, 0), "East");
    assert_eq!(wb.display_value(s, 7, 4), "1068");

    // Saved again from the saved file: still there, once.
    let mut wb = Workbook::open(dir.path().join("a.xlsx")).unwrap();
    let p2 = save(&mut wb, dir.path(), "b.xlsx");
    assert_eq!(p2["xl/workbook.xml"].matches("<pivotCache ").count(), 2);
    assert_eq!(
        p2["xl/pivotTables/pivotTable3.xml"],
        orig["xl/pivotTables/pivotTable3.xml"]
    );
}

#[test]
fn pivots_follow_inserted_and_deleted_rows_and_columns() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(fixture()).unwrap();
    let pivot = sheet_index(&wb, "Pivot");
    let data = sheet_index(&wb, "Data");
    wb.insert_rows(pivot, 0, 2).unwrap();
    wb.insert_columns(pivot, 0, 1).unwrap();
    wb.insert_rows(data, 5, 3).unwrap(); // inside the source
    let p = save(&mut wb, dir.path(), "a.xlsx");
    assert_eq!(location(&p, "xl/pivotTables/pivotTable1.xml"), "B5:F10");
    assert_eq!(location(&p, "xl/pivotTables/pivotTable2.xml"), "I5:K8");
    assert_eq!(
        location(&p, "xl/pivotTables/pivotTable3.xml"),
        "A1:C14",
        "other sheet"
    );
    assert_eq!(
        source(&p, "xl/pivotCache/pivotCacheDefinition1.xml")
            .0
            .as_deref(),
        Some("A1:E16")
    );
    // A source by name follows the table.
    assert_eq!(
        source(&p, "xl/pivotCache/pivotCacheDefinition2.xml")
            .1
            .as_deref(),
        Some("Sales")
    );
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());

    wb.delete_rows(data, 5, 7).unwrap();
    wb.delete_columns(pivot, 0, 0).unwrap();
    let p = save(&mut wb, dir.path(), "b.xlsx");
    assert_eq!(location(&p, "xl/pivotTables/pivotTable1.xml"), "A5:E10");
    assert_eq!(
        source(&p, "xl/pivotCache/pivotCacheDefinition1.xml")
            .0
            .as_deref(),
        Some("A1:E13")
    );
}
