//! Page-tree operations: select (extract, delete, reorder), split, merge, rotate.

use std::collections::{BTreeMap, HashSet};

use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::range::PageRanges;
use crate::{Error, Pdf, Result};

/// Page object ids in page order.
pub(crate) fn page_ids(doc: &Document) -> Vec<ObjectId> {
    doc.get_pages().into_values().collect()
}

/// A page attribute, looking up the page tree for inheritable ones.
pub(crate) fn inherited(doc: &Document, page: ObjectId, key: &[u8]) -> Option<Object> {
    let mut current = page;
    let mut seen = HashSet::new();
    while seen.insert(current) {
        let node = doc.get_dictionary(current).ok()?;
        if let Ok(value) = node.get(key)
            && !matches!(value, Object::Null)
        {
            return Some(value.clone());
        }
        current = node.get(b"Parent").and_then(Object::as_reference).ok()?;
    }
    None
}

/// A number object as f32 (following one reference).
pub(crate) fn number(doc: &Document, obj: &Object) -> Option<f32> {
    match doc.dereference(obj).ok()?.1 {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

/// A rectangle `[x0 y0 x1 y1]`, normalized so x0 <= x1 and y0 <= y1.
pub(crate) fn rect(doc: &Document, obj: &Object) -> Option<[f32; 4]> {
    let arr = doc.dereference(obj).ok()?.1.as_array().ok()?;
    if arr.len() != 4 {
        return None;
    }
    let v: Vec<f32> = arr.iter().map(|o| number(doc, o)).collect::<Option<_>>()?;
    Some([
        v[0].min(v[2]),
        v[1].min(v[3]),
        v[0].max(v[2]),
        v[1].max(v[3]),
    ])
}

/// US Letter, the PDF default when a page has no MediaBox.
const DEFAULT_MEDIA_BOX: [f32; 4] = [0.0, 0.0, 612.0, 792.0];

/// The visible box of a page (CropBox, else MediaBox).
pub(crate) fn visible_box(doc: &Document, page: ObjectId) -> [f32; 4] {
    let media = inherited(doc, page, b"MediaBox")
        .and_then(|o| rect(doc, &o))
        .unwrap_or(DEFAULT_MEDIA_BOX);
    match inherited(doc, page, b"CropBox").and_then(|o| rect(doc, &o)) {
        // The CropBox is clipped to the MediaBox.
        Some(c) => {
            let r = [
                c[0].max(media[0]),
                c[1].max(media[1]),
                c[2].min(media[2]),
                c[3].min(media[3]),
            ];
            if r[0] < r[2] && r[1] < r[3] { r } else { media }
        }
        None => media,
    }
}

/// The page's /Rotate normalized to 0, 90, 180 or 270.
pub(crate) fn rotation(doc: &Document, page: ObjectId) -> i64 {
    let r = inherited(doc, page, b"Rotate")
        .and_then(|o| number(doc, &o))
        .unwrap_or(0.0) as i64;
    // Values that aren't multiples of 90 are invalid; viewers round down.
    (r - r.rem_euclid(90)).rem_euclid(360)
}

/// Copy inherited attributes into the page itself, so it survives being
/// moved to another page tree.
fn materialize(doc: &mut Document, page: ObjectId) -> Result<()> {
    let mut attrs = Vec::new();
    for key in [&b"MediaBox"[..], b"CropBox", b"Resources", b"Rotate"] {
        if let Some(v) = inherited(doc, page, key) {
            attrs.push((key.to_vec(), v));
        }
    }
    let dict = doc.get_dictionary_mut(page)?;
    for (k, v) in attrs {
        dict.set(k, v);
    }
    if !dict.has(b"MediaBox") {
        dict.set(
            "MediaBox",
            DEFAULT_MEDIA_BOX
                .iter()
                .map(|&n| Object::Real(n))
                .collect::<Vec<_>>(),
        );
    }
    if !dict.has(b"Resources") {
        dict.set("Resources", Dictionary::new());
    }
    Ok(())
}

fn pages_root(doc: &Document) -> Result<ObjectId> {
    doc.catalog()?
        .get(b"Pages")
        .and_then(Object::as_reference)
        .map_err(|_| Error::Pdf("the catalog has no page tree reference".into()))
}

/// Replace every reference to `removed` with null, in all objects and the trailer.
fn null_references(doc: &mut Document, removed: &HashSet<ObjectId>) {
    if removed.is_empty() {
        return;
    }
    fn walk(o: &mut Object, removed: &HashSet<ObjectId>) {
        match o {
            Object::Reference(id) if removed.contains(id) => *o = Object::Null,
            Object::Array(a) => a.iter_mut().for_each(|x| walk(x, removed)),
            Object::Dictionary(d) => d.iter_mut().for_each(|(_, x)| walk(x, removed)),
            Object::Stream(s) => s.dict.iter_mut().for_each(|(_, x)| walk(x, removed)),
            _ => {}
        }
    }
    for o in doc.objects.values_mut() {
        walk(o, removed);
    }
    for (_, o) in doc.trailer.iter_mut() {
        walk(o, removed);
    }
}

fn check_pages(pages: &[u32], total: u32) -> Result<()> {
    if pages.is_empty() {
        return Err(Error::Invalid("no pages selected".into()));
    }
    if let Some(p) = pages.iter().find(|&&p| p == 0 || p > total) {
        return Err(Error::Invalid(format!(
            "page {p} is out of range (the document has {total} pages)"
        )));
    }
    Ok(())
}

/// Rebuild the page tree as a flat list of `pages` (1-based, in order; a page
/// may repeat). Pages left out are removed, and links to them become null.
/// The rest of the catalog (outlines, names, metadata) is kept.
pub(crate) fn select(pdf: &mut Pdf, pages: &[u32]) -> Result<()> {
    let doc = &mut pdf.doc;
    let ids = page_ids(doc);
    check_pages(pages, ids.len() as u32)?;
    let root = pages_root(doc)?;
    let keep: HashSet<u32> = pages.iter().copied().collect();
    for &p in &keep {
        materialize(doc, ids[p as usize - 1])?;
    }
    let mut used = HashSet::new();
    let mut kids = Vec::with_capacity(pages.len());
    for &p in pages {
        let id = ids[p as usize - 1];
        let kid = if used.insert(id) {
            id
        } else {
            // A page can only sit in the tree once; repeat it as a copy that
            // shares its contents and resources.
            let copy = doc.get_dictionary(id)?.clone();
            doc.add_object(copy)
        };
        doc.get_dictionary_mut(kid)?.set("Parent", root);
        kids.push(Object::Reference(kid));
    }
    let count = kids.len() as i64;
    let mut tree = Dictionary::new();
    tree.set("Type", "Pages");
    tree.set("Kids", kids);
    tree.set("Count", count);
    doc.objects.insert(root, Object::Dictionary(tree));
    let removed: HashSet<ObjectId> = ids.into_iter().filter(|id| !used.contains(id)).collect();
    null_references(doc, &removed);
    for id in &removed {
        doc.objects.remove(id);
    }
    doc.prune_objects();
    Ok(())
}

/// Keep only `ranges`, in the order written (`3,1-2` puts page 3 first).
pub fn extract(pdf: &mut Pdf, ranges: &PageRanges) -> Result<()> {
    let pages = ranges.resolve(pdf.page_count())?;
    select(pdf, &pages)
}

/// Remove the pages in `ranges`. Returns how many were removed.
pub fn delete(pdf: &mut Pdf, ranges: &PageRanges) -> Result<u32> {
    let total = pdf.page_count();
    let gone: HashSet<u32> = ranges.resolve_set(total)?.into_iter().collect();
    let keep: Vec<u32> = (1..=total).filter(|p| !gone.contains(p)).collect();
    if keep.is_empty() {
        return Err(Error::Invalid(format!(
            "that would delete all {total} pages; a PDF needs at least one"
        )));
    }
    select(pdf, &keep)?;
    Ok(gone.len() as u32)
}

/// Put the pages in `order`, which must list every page exactly once.
pub fn reorder(pdf: &mut Pdf, order: &PageRanges) -> Result<()> {
    let total = pdf.page_count();
    let pages = order.resolve(total)?;
    let mut seen = vec![false; total as usize + 1];
    let mut repeated = Vec::new();
    for &p in &pages {
        if std::mem::replace(&mut seen[p as usize], true) {
            repeated.push(p);
        }
    }
    let missing: Vec<u32> = (1..=total).filter(|&p| !seen[p as usize]).collect();
    if !repeated.is_empty() || !missing.is_empty() {
        let mut msg = format!("the new order must list each of the {total} pages once:");
        if !missing.is_empty() {
            msg += &format!(" missing {}", crate::range::compact(&missing));
        }
        if !repeated.is_empty() {
            repeated.sort_unstable();
            repeated.dedup();
            if !missing.is_empty() {
                msg.push(';');
            }
            msg += &format!(" repeated {}", crate::range::compact(&repeated));
        }
        msg += " (use `extract` to pick or repeat pages)";
        return Err(Error::Invalid(msg));
    }
    select(pdf, &pages)
}

/// Rotate pages clockwise by `degrees` (a multiple of 90; negative turns
/// counter-clockwise). Adds to any rotation the page already has.
/// Returns how many pages were rotated.
pub fn rotate(pdf: &mut Pdf, ranges: &PageRanges, degrees: i64) -> Result<u32> {
    if degrees % 90 != 0 {
        return Err(Error::Invalid(format!(
            "rotation must be a multiple of 90 degrees (90, 180, 270 or -90), not {degrees}"
        )));
    }
    let doc = &mut pdf.doc;
    let ids = page_ids(doc);
    let pages = ranges.resolve_set(ids.len() as u32)?;
    for &p in &pages {
        let id = ids[p as usize - 1];
        let new = (rotation(doc, id) + degrees).rem_euclid(360);
        doc.get_dictionary_mut(id)?.set("Rotate", new);
    }
    Ok(pages.len() as u32)
}

/// Collect every object reachable from `start`, skipping /Parent links and
/// references to pages outside `keep` (which become null in the copy).
fn copy_reachable(
    src: &Document,
    start: &[ObjectId],
    all_pages: &HashSet<ObjectId>,
    keep: &HashSet<ObjectId>,
) -> BTreeMap<ObjectId, Object> {
    fn scrub(
        o: &mut Object,
        all_pages: &HashSet<ObjectId>,
        keep: &HashSet<ObjectId>,
        queue: &mut Vec<ObjectId>,
    ) {
        match o {
            Object::Reference(id) => {
                if all_pages.contains(id) && !keep.contains(id) {
                    *o = Object::Null;
                } else {
                    queue.push(*id);
                }
            }
            Object::Array(a) => a.iter_mut().for_each(|x| scrub(x, all_pages, keep, queue)),
            Object::Dictionary(d) => d
                .iter_mut()
                .for_each(|(_, x)| scrub(x, all_pages, keep, queue)),
            Object::Stream(s) => s
                .dict
                .iter_mut()
                .for_each(|(_, x)| scrub(x, all_pages, keep, queue)),
            _ => {}
        }
    }
    let mut out = BTreeMap::new();
    let mut queue: Vec<ObjectId> = start.to_vec();
    while let Some(id) = queue.pop() {
        if out.contains_key(&id) {
            continue;
        }
        let Ok(obj) = src.get_object(id) else {
            continue;
        };
        let mut obj = obj.clone();
        if keep.contains(&id)
            && let Object::Dictionary(d) = &mut obj
        {
            d.remove(b"Parent");
        }
        scrub(&mut obj, all_pages, keep, &mut queue);
        out.insert(id, obj);
    }
    out
}

/// One part of a split.
pub struct SplitPart {
    /// The pages in the part (1-based, in the source document).
    pub pages: Vec<u32>,
    pub pdf: Pdf,
}

/// Split into one PDF per group of pages, calling `each` as each part is
/// ready (so only one part is in memory at a time). Each part copies only the
/// objects its pages use, plus the document info.
pub fn split(
    pdf: &Pdf,
    groups: &[Vec<u32>],
    mut each: impl FnMut(SplitPart) -> Result<()>,
) -> Result<()> {
    let src = &pdf.doc;
    let ids = page_ids(src);
    let total = ids.len() as u32;
    let all_pages: HashSet<ObjectId> = ids.iter().copied().collect();
    let info = src.trailer.get(b"Info").ok().cloned();
    for group in groups {
        check_pages(group, total)?;
        // Work on materialized copies of the pages so inherited attributes come along.
        let mut work = Document::with_version(src.version.clone());
        work.max_id = src.max_id;
        let wanted: Vec<ObjectId> = group.iter().map(|&p| ids[p as usize - 1]).collect();
        let keep: HashSet<ObjectId> = wanted.iter().copied().collect();
        let mut start: Vec<ObjectId> = wanted.clone();
        if let Some(Object::Reference(id)) = &info {
            start.push(*id);
        }
        let mut objects = copy_reachable(src, &start, &all_pages, &keep);
        for &id in &keep {
            let mut attrs = Vec::new();
            for key in [&b"MediaBox"[..], b"CropBox", b"Resources", b"Rotate"] {
                if let Some(mut v) = inherited(src, id, key) {
                    // Inherited values come from the page tree, which was not copied.
                    let extra = match &mut v {
                        Object::Reference(r) => vec![*r],
                        _ => Vec::new(),
                    };
                    for (eid, eobj) in copy_reachable(src, &extra, &all_pages, &keep) {
                        objects.entry(eid).or_insert(eobj);
                    }
                    attrs.push((key.to_vec(), v));
                }
            }
            if let Some(Object::Dictionary(d)) = objects.get_mut(&id) {
                for (k, v) in attrs {
                    if !d.has(&k) {
                        d.set(k, v);
                    }
                }
                if !d.has(b"MediaBox") {
                    d.set(
                        "MediaBox",
                        DEFAULT_MEDIA_BOX
                            .iter()
                            .map(|&n| Object::Real(n))
                            .collect::<Vec<_>>(),
                    );
                }
            }
        }
        work.objects = objects;
        let root = work.new_object_id();
        let mut used = HashSet::new();
        let mut kids = Vec::new();
        for &id in &wanted {
            let kid = if used.insert(id) {
                id
            } else {
                let copy = work.get_dictionary(id)?.clone();
                work.add_object(copy)
            };
            work.get_dictionary_mut(kid)?.set("Parent", root);
            kids.push(Object::Reference(kid));
        }
        let count = kids.len() as i64;
        let mut tree = Dictionary::new();
        tree.set("Type", "Pages");
        tree.set("Kids", kids);
        tree.set("Count", count);
        work.objects.insert(root, Object::Dictionary(tree));
        let mut catalog = Dictionary::new();
        catalog.set("Type", "Catalog");
        catalog.set("Pages", root);
        let catalog_id = work.add_object(catalog);
        work.trailer.set("Root", catalog_id);
        if let Some(info) = &info {
            work.trailer.set("Info", info.clone());
        }
        work.renumber_objects();
        each(SplitPart {
            pages: group.clone(),
            pdf: Pdf {
                doc: work,
                was_encrypted: pdf.was_encrypted(),
            },
        })?;
    }
    Ok(())
}

/// Append the pages of `others` to `base`, in order. The base keeps its
/// catalog (outlines, metadata); the others contribute their pages only.
pub fn merge(mut base: Pdf, others: Vec<Pdf>) -> Result<Pdf> {
    let root = pages_root(&base.doc)?;
    let mut kids: Vec<Object> = Vec::new();
    for id in page_ids(&base.doc) {
        materialize(&mut base.doc, id)?;
        base.doc.get_dictionary_mut(id)?.set("Parent", root);
        kids.push(Object::Reference(id));
    }
    for mut other in others {
        let doc = &mut other.doc;
        doc.renumber_objects_with(base.doc.max_id + 1);
        let ids = page_ids(doc);
        for &id in &ids {
            materialize(doc, id)?;
            doc.get_dictionary_mut(id)?.set("Parent", root);
            kids.push(Object::Reference(id));
        }
        base.doc.max_id = base.doc.max_id.max(doc.max_id);
        if doc.version > base.doc.version {
            base.doc.version = doc.version.clone();
        }
        base.was_encrypted |= other.was_encrypted;
        base.doc.objects.append(&mut doc.objects);
    }
    let count = kids.len() as i64;
    let mut tree = Dictionary::new();
    tree.set("Type", "Pages");
    tree.set("Kids", kids);
    tree.set("Count", count);
    base.doc.objects.insert(root, Object::Dictionary(tree));
    base.doc.prune_objects();
    Ok(base)
}

/// Pages per part for `--every N`.
pub fn every(total: u32, n: u32) -> Result<Vec<Vec<u32>>> {
    if n == 0 {
        return Err(Error::Invalid(
            "--every needs at least 1 page per part".into(),
        ));
    }
    let pages: Vec<u32> = (1..=total).collect();
    Ok(pages.chunks(n as usize).map(<[u32]>::to_vec).collect())
}
