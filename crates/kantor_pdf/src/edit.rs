//! Building an edited PDF from pages of one or more PDFs: the page tools of
//! Kantor's PDF window (reorder, delete, rotate, insert from other files,
//! duplicate, blank pages) are a list of [`Pick`]s, written by [`assemble`].
//!
//! The first source is the *base*: its catalog (bookmarks, names, forms,
//! metadata, encryption) is kept. Bookmarks and links that pointed at pages
//! left out are dropped ([`prune_dead_targets`]).

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::Path;

use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::pages::{materialize, null_references, page_ids, pages_root, rotation};
use crate::{Error, Pdf, Result};

/// One page of the result: page `page` (0-based) of source `source`,
/// turned `turn` degrees clockwise on top of its own rotation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pick {
    pub source: usize,
    pub page: u32,
    pub turn: i64,
}

/// The pages of `picks`, in order, as one PDF. `sources[0]` is the base
/// (its catalog is kept, and it must be given even when none of its pages
/// is picked); the others contribute the pages picked from them and their
/// form fields.
pub fn assemble(mut sources: Vec<Pdf>, picks: &[Pick]) -> Result<Pdf> {
    if sources.is_empty() {
        return Err(Error::Invalid("no PDF to build from".into()));
    }
    if picks.is_empty() {
        return Err(Error::Invalid("a PDF needs at least one page".into()));
    }
    if let Some(p) = picks.iter().find(|p| p.turn % 90 != 0) {
        return Err(Error::Invalid(format!(
            "rotation must be a multiple of 90 degrees, not {}",
            p.turn
        )));
    }
    let mut base = sources.remove(0);
    let mut ids: Vec<Vec<ObjectId>> = vec![page_ids(&base.doc)];
    let mut fields: Vec<Object> = Vec::new();
    let mut form_extra: Option<Dictionary> = None;
    for mut other in sources {
        let doc = &mut other.doc;
        doc.renumber_objects_with(base.doc.max_id + 1);
        ids.push(page_ids(doc));
        if let Some(form) = acro_form(doc) {
            if let Ok(Object::Array(f)) = form.get(b"Fields") {
                fields.extend(f.iter().cloned());
            }
            if form_extra.is_none() {
                form_extra = Some(form);
            }
        }
        base.doc.max_id = base.doc.max_id.max(doc.max_id);
        if doc.version > base.doc.version {
            base.doc.version = doc.version.clone();
        }
        base.doc.objects.append(&mut doc.objects);
    }
    for p in picks {
        let Some(list) = ids.get(p.source) else {
            return Err(Error::Invalid(format!("no source {}", p.source)));
        };
        if p.page as usize >= list.len() {
            return Err(Error::Invalid(format!(
                "page {} is out of range (the document has {} pages)",
                p.page + 1,
                list.len()
            )));
        }
    }
    let doc = &mut base.doc;
    let root = pages_root(doc)?;
    // Inherited attributes onto the pages while their trees still exist.
    let picked: HashSet<ObjectId> = picks
        .iter()
        .map(|p| ids[p.source][p.page as usize])
        .collect();
    let turns: HashMap<ObjectId, i64> = picked.iter().map(|&id| (id, rotation(doc, id))).collect();
    for &id in &picked {
        materialize(doc, id)?;
    }
    let mut used = HashSet::new();
    let mut kids = Vec::with_capacity(picks.len());
    for p in picks {
        let id = ids[p.source][p.page as usize];
        let kid = if used.insert(id) {
            id
        } else {
            duplicate_page(doc, id)?
        };
        let dict = doc.get_dictionary_mut(kid)?;
        dict.set("Parent", root);
        let r = (turns[&id] + p.turn).rem_euclid(360);
        if r == 0 {
            dict.remove(b"Rotate");
        } else {
            dict.set("Rotate", r);
        }
        kids.push(Object::Reference(kid));
    }
    let mut tree = Dictionary::new();
    tree.set("Type", "Pages");
    tree.set("Count", kids.len() as i64);
    tree.set("Kids", kids);
    doc.objects.insert(root, Object::Dictionary(tree));
    // Form fields of the other sources join the base's form.
    if !fields.is_empty() {
        add_fields(doc, fields, form_extra)?;
    }
    // An XFA form describes the original pages: once pages are added,
    // removed or moved, readers must use the pages' own form fields.
    let same_pages = picks.len() == ids[0].len()
        && picks
            .iter()
            .enumerate()
            .all(|(i, p)| p.source == 0 && p.page as usize == i);
    if !same_pages {
        drop_xfa(doc);
    }
    let removed: HashSet<ObjectId> = ids
        .iter()
        .flatten()
        .copied()
        .filter(|id| !used.contains(id))
        .collect();
    null_references(doc, &removed);
    for id in &removed {
        doc.objects.remove(id);
    }
    prune_dead_targets(doc);
    doc.prune_objects();
    Ok(base)
}

/// Remove the form's XFA part (and the catalog's request to render it),
/// as Acrobat does when pages change: the AcroForm fields on the pages stay.
pub(crate) fn drop_xfa(doc: &mut Document) {
    let form = doc
        .catalog()
        .ok()
        .and_then(|c| c.get(b"AcroForm").ok().cloned());
    match form {
        Some(Object::Reference(id)) => {
            if let Ok(Object::Dictionary(d)) = doc.get_object_mut(id) {
                d.remove(b"XFA");
            }
        }
        Some(Object::Dictionary(mut d)) => {
            if d.remove(b"XFA").is_some()
                && let Ok(c) = doc.catalog_mut()
            {
                c.set("AcroForm", d);
            }
        }
        _ => {}
    }
    if let Ok(c) = doc.catalog_mut() {
        c.remove(b"NeedsRendering");
    }
}

/// The document's interactive form dictionary, if any.
fn acro_form(doc: &Document) -> Option<Dictionary> {
    let form = doc.catalog().ok()?.get(b"AcroForm").ok()?;
    let (_, form) = doc.dereference(form).ok()?;
    let mut form = form.as_dict().ok()?.clone();
    if let Ok(f) = form.get(b"Fields")
        && let Ok((_, Object::Array(a))) = doc.dereference(f)
    {
        form.set("Fields", a.clone());
    }
    Some(form)
}

/// Append `fields` to the base's form (made from `extra` when it has none).
fn add_fields(doc: &mut Document, fields: Vec<Object>, extra: Option<Dictionary>) -> Result<()> {
    let existing = doc.catalog()?.get(b"AcroForm").ok().cloned();
    let form_id = match existing {
        Some(Object::Reference(id)) => Some(id),
        _ => None,
    };
    let mut form = match &existing {
        Some(o) => doc
            .dereference(o)
            .ok()
            .and_then(|(_, o)| o.as_dict().ok().cloned())
            .unwrap_or_default(),
        None => {
            let mut f = extra.unwrap_or_default();
            f.remove(b"Fields");
            f
        }
    };
    let mut all = match form.get(b"Fields") {
        Ok(f) => match doc.dereference(f) {
            Ok((_, Object::Array(a))) => a.clone(),
            _ => Vec::new(),
        },
        Err(_) => Vec::new(),
    };
    all.extend(fields);
    form.set("Fields", all);
    match form_id {
        Some(id) => {
            doc.objects.insert(id, Object::Dictionary(form));
        }
        None => {
            let id = doc.add_object(form);
            doc.catalog_mut()?.set("AcroForm", id);
        }
    }
    Ok(())
}

/// A copy of page `id` sharing its contents and resources. Its comments and
/// other markup annotations are copied too (an annotation belongs to one
/// page); form fields and pop-ups stay on the original page only.
fn duplicate_page(doc: &mut Document, id: ObjectId) -> Result<ObjectId> {
    let mut copy = doc.get_dictionary(id)?.clone();
    let annots = copy
        .get(b"Annots")
        .ok()
        .and_then(|a| doc.dereference(a).ok())
        .and_then(|(_, a)| a.as_array().ok().cloned());
    copy.remove(b"Annots");
    // Structure (tagged PDF) entries name this page's marked content once.
    copy.remove(b"StructParents");
    let new_id = doc.add_object(copy);
    if let Some(annots) = annots {
        let mut out = Vec::new();
        for a in annots {
            let Ok((_, Object::Dictionary(d))) = doc.dereference(&a) else {
                continue;
            };
            let subtype = d.get(b"Subtype").and_then(Object::as_name).unwrap_or(b"");
            if matches!(subtype, b"Widget" | b"Popup") {
                continue;
            }
            let mut d = d.clone();
            d.remove(b"Popup");
            d.remove(b"StructParent");
            d.set("P", new_id);
            out.push(Object::Reference(doc.add_object(d)));
        }
        if !out.is_empty() {
            doc.get_dictionary_mut(new_id)?.set("Annots", out);
        }
    }
    Ok(new_id)
}

/// Whether `dest` (an explicit destination array) points at a page that is
/// gone (made null when its page was removed).
fn dead_array(doc: &Document, dest: &Object) -> bool {
    match doc.dereference(dest) {
        Ok((_, Object::Array(a))) => match a.first() {
            Some(Object::Null) | None => true,
            Some(Object::Reference(id)) => doc.get_object(*id).is_err(),
            _ => false,
        },
        Ok((_, Object::Dictionary(d))) => d.get(b"D").is_ok_and(|d| dead_array(doc, d)),
        _ => false,
    }
}

/// The named destinations: name → destination object.
fn named_dests(doc: &Document) -> HashMap<Vec<u8>, Object> {
    let mut out = HashMap::new();
    let Ok(catalog) = doc.catalog() else {
        return out;
    };
    // PDF 1.1: a dictionary in the catalog.
    if let Ok(d) = catalog.get(b"Dests")
        && let Ok((_, Object::Dictionary(d))) = doc.dereference(d)
    {
        for (k, v) in d.iter() {
            out.insert(k.clone(), v.clone());
        }
    }
    // PDF 1.2: a name tree under /Names /Dests.
    fn walk(doc: &Document, node: &Object, out: &mut HashMap<Vec<u8>, Object>, depth: u32) {
        let Ok((_, Object::Dictionary(n))) = doc.dereference(node) else {
            return;
        };
        if depth > 32 {
            return;
        }
        if let Ok(names) = n.get(b"Names")
            && let Ok((_, Object::Array(a))) = doc.dereference(names)
        {
            for pair in a.chunks(2) {
                if let [Object::String(k, _), v] = pair {
                    out.insert(k.clone(), v.clone());
                }
            }
        }
        if let Ok(kids) = n.get(b"Kids")
            && let Ok((_, Object::Array(a))) = doc.dereference(kids)
        {
            for k in a {
                walk(doc, k, out, depth + 1);
            }
        }
    }
    if let Ok(names) = catalog.get(b"Names")
        && let Ok((_, Object::Dictionary(n))) = doc.dereference(names)
        && let Ok(tree) = n.get(b"Dests")
    {
        walk(doc, tree, &mut out, 0);
    }
    out
}

/// Whether a bookmark or link (`/Dest` or a GoTo `/A`) leads to a page that
/// is gone.
fn dead_target(doc: &Document, item: &Dictionary, names: &HashMap<Vec<u8>, Object>) -> bool {
    let dest = match item.get(b"Dest") {
        Ok(d) => Some(d),
        Err(_) => item
            .get(b"A")
            .ok()
            .and_then(|a| doc.dereference(a).ok())
            .and_then(|(_, a)| a.as_dict().ok())
            .filter(|a| a.get(b"S").and_then(Object::as_name).ok() == Some(b"GoTo"))
            .and_then(|a| a.get(b"D").ok()),
    };
    let Some(dest) = dest else {
        return false;
    };
    let dest = doc.dereference(dest).map_or(dest, |(_, d)| d);
    match dest {
        Object::Name(n) | Object::String(n, _) => names.get(n).is_some_and(|d| dead_array(doc, d)),
        d => dead_array(doc, d),
    }
}

/// Drop bookmarks and links that lead to pages no longer in the document
/// (a bookmark with children keeps its place, without its target), and an
/// open action that does.
pub(crate) fn prune_dead_targets(doc: &mut Document) {
    let names = named_dests(doc);
    // Links on the pages.
    for page in page_ids(doc) {
        let Some(annots) = doc
            .get_dictionary(page)
            .ok()
            .and_then(|p| p.get(b"Annots").ok().cloned())
        else {
            continue;
        };
        let (annots_id, list) = match doc.dereference(&annots) {
            Ok((id, Object::Array(a))) => (id, a.clone()),
            _ => continue,
        };
        let keep: Vec<Object> = list
            .iter()
            .filter(|a| {
                let Ok((_, Object::Dictionary(d))) = doc.dereference(a) else {
                    return true;
                };
                let link = d.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Link");
                !(link && dead_target(doc, d, &names))
            })
            .cloned()
            .collect();
        if keep.len() != list.len() {
            match annots_id {
                Some(id) => {
                    doc.objects.insert(id, Object::Array(keep));
                }
                None => {
                    if let Ok(p) = doc.get_dictionary_mut(page) {
                        p.set("Annots", keep);
                    }
                }
            }
        }
    }
    // The open action.
    if let Ok(cat) = doc.catalog()
        && let Ok(action) = cat.get(b"OpenAction")
    {
        let dead = match doc.dereference(action) {
            Ok((_, Object::Array(_))) => dead_array(doc, action),
            Ok((_, Object::Dictionary(d))) => dead_target(doc, d, &names),
            _ => false,
        };
        if dead && let Ok(cat) = doc.catalog_mut() {
            cat.remove(b"OpenAction");
        }
    }
    // Bookmarks.
    let Some(outlines) = doc
        .catalog()
        .ok()
        .and_then(|c| c.get(b"Outlines").ok())
        .and_then(|o| o.as_reference().ok())
    else {
        return;
    };
    prune_outline(doc, outlines, &names, &mut HashSet::new());
    let visible = outline_visible(doc, outlines, &mut HashSet::new());
    if let Ok(d) = doc.get_dictionary_mut(outlines) {
        if visible == 0 {
            d.remove(b"Count");
        } else {
            d.set("Count", visible);
        }
    }
}

/// The children of an outline node (following /First and /Next).
fn outline_children(doc: &Document, node: ObjectId) -> Vec<ObjectId> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut next = doc
        .get_dictionary(node)
        .ok()
        .and_then(|d| d.get(b"First").ok())
        .and_then(|o| o.as_reference().ok());
    while let Some(id) = next {
        if !seen.insert(id) || doc.get_dictionary(id).is_err() {
            break;
        }
        out.push(id);
        next = doc
            .get_dictionary(id)
            .ok()
            .and_then(|d| d.get(b"Next").ok())
            .and_then(|o| o.as_reference().ok());
    }
    out
}

/// Remove dead items under `node`, relinking the rest.
fn prune_outline(
    doc: &mut Document,
    node: ObjectId,
    names: &HashMap<Vec<u8>, Object>,
    seen: &mut HashSet<ObjectId>,
) {
    if !seen.insert(node) {
        return;
    }
    let kids = outline_children(doc, node);
    let mut keep = Vec::new();
    for id in kids {
        prune_outline(doc, id, names, seen);
        let has_children = !outline_children(doc, id).is_empty();
        let dead = doc
            .get_dictionary(id)
            .is_ok_and(|d| dead_target(doc, d, names));
        if dead {
            if has_children {
                if let Ok(d) = doc.get_dictionary_mut(id) {
                    d.remove(b"Dest");
                    d.remove(b"A");
                }
            } else {
                continue;
            }
        }
        keep.push(id);
    }
    for (i, &id) in keep.iter().enumerate() {
        if let Ok(d) = doc.get_dictionary_mut(id) {
            d.set("Parent", node);
            match i.checked_sub(1).map(|j| keep[j]) {
                Some(prev) => d.set("Prev", prev),
                None => {
                    d.remove(b"Prev");
                }
            }
            match keep.get(i + 1) {
                Some(&next) => d.set("Next", next),
                None => {
                    d.remove(b"Next");
                }
            }
        }
    }
    if let Ok(d) = doc.get_dictionary_mut(node) {
        match (keep.first(), keep.last()) {
            (Some(&f), Some(&l)) => {
                d.set("First", f);
                d.set("Last", l);
            }
            _ => {
                d.remove(b"First");
                d.remove(b"Last");
                d.remove(b"Count");
            }
        }
    }
}

/// Fix /Count under `node` and return how many items show below it when it
/// is open.
fn outline_visible(doc: &mut Document, node: ObjectId, seen: &mut HashSet<ObjectId>) -> i64 {
    if !seen.insert(node) {
        return 0;
    }
    let mut total = 0;
    for id in outline_children(doc, node) {
        let below = outline_visible(doc, id, seen);
        let open = doc
            .get_dictionary(id)
            .ok()
            .and_then(|d| d.get(b"Count").ok())
            .and_then(|c| c.as_i64().ok())
            .is_some_and(|c| c > 0);
        if let Ok(d) = doc.get_dictionary_mut(id) {
            if below == 0 {
                d.remove(b"Count");
            } else {
                d.set("Count", if open { below } else { -below });
            }
        }
        total += 1 + if open { below } else { 0 };
    }
    total
}

/// A PDF with one blank page `width` × `height` points.
pub fn blank_page(width: f32, height: f32) -> Pdf {
    let mut doc = Document::with_version("1.4");
    let pages_id = doc.new_object_id();
    let mut page = Dictionary::new();
    page.set("Type", "Page");
    page.set("Parent", pages_id);
    page.set(
        "MediaBox",
        vec![
            0.into(),
            0.into(),
            Object::Real(width.max(1.0)),
            Object::Real(height.max(1.0)),
        ],
    );
    page.set("Resources", Dictionary::new());
    let page_id = doc.add_object(page);
    let mut tree = Dictionary::new();
    tree.set("Type", "Pages");
    tree.set("Kids", vec![Object::Reference(page_id)]);
    tree.set("Count", 1);
    doc.objects.insert(pages_id, Object::Dictionary(tree));
    let mut catalog = Dictionary::new();
    catalog.set("Type", "Catalog");
    catalog.set("Pages", pages_id);
    let catalog_id = doc.add_object(catalog);
    doc.trailer.set("Root", catalog_id);
    Pdf {
        doc,
        was_encrypted: false,
    }
}

impl Pdf {
    /// Serialize, encrypted again with the source's own password and
    /// method when it was encrypted (`keep_encryption`). Fails with
    /// [`Error::Encryption`] when that isn't possible.
    pub fn to_bytes_encrypted(&mut self, keep_encryption: bool) -> Result<Vec<u8>> {
        crate::sig::drop_usage_rights(&mut self.doc);
        let state = self.doc.encryption_state.take();
        if keep_encryption && let Some(state) = state {
            // Encrypting changes every string and stream: work on a copy so
            // this document stays readable.
            let mut copy = self.clone();
            if let Some(&(max, _)) = copy.doc.objects.keys().next_back() {
                copy.doc.max_id = max;
            }
            let result = copy.doc.encrypt(&state);
            self.doc.encryption_state = Some(state);
            result.map_err(|e| Error::Encryption(e.to_string()))?;
            let mut out = Vec::new();
            copy.doc
                .save_to(&mut out)
                .map_err(|e| Error::Pdf(e.to_string()))?;
            return Ok(out);
        }
        self.doc.encryption_state = state;
        if keep_encryption && self.was_encrypted {
            return Err(Error::Encryption(
                "the encryption of the original can't be reproduced".into(),
            ));
        }
        self.to_bytes_compact()
    }

    /// Serialize with object and cross-reference streams (PDF 1.5), as most
    /// PDF writers do: lopdf's plain writer would store every small object
    /// uncompressed and make the file grow.
    pub fn to_bytes_compact(&mut self) -> Result<Vec<u8>> {
        crate::sig::drop_usage_rights(&mut self.doc);
        if let Some(&(max, _)) = self.doc.objects.keys().next_back() {
            self.doc.max_id = max;
        }
        let mut out = Vec::new();
        self.doc
            .save_modern(&mut out)
            .map_err(|e| Error::Pdf(e.to_string()))?;
        Ok(out)
    }
}

/// Write `bytes` to `path` safely: a temporary file next to it, flushed,
/// then renamed over it (the old file stays whole if anything fails). An
/// existing file's permissions are kept.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let io = |source| Error::Io {
        path: path.to_owned(),
        source,
    };
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let mut tmp = match tempfile::Builder::new()
        .prefix(".kantor-")
        .suffix(".pdf.tmp")
        .tempfile_in(dir)
    {
        Ok(tmp) => tmp,
        // Sandbox: only the file itself may be written (kantor_core::fsx).
        Err(e) if kantor_core::fsx::no_folder_access(&e) => {
            return kantor_core::fsx::write_in_place(path, bytes).map_err(io);
        }
        Err(e) => return Err(io(e)),
    };
    tmp.write_all(bytes).map_err(io)?;
    tmp.as_file().sync_all().map_err(io)?;
    if let Ok(meta) = std::fs::metadata(path) {
        std::fs::set_permissions(tmp.path(), meta.permissions()).map_err(io)?;
    }
    tmp.persist(path).map_err(|e| io(e.error))?;
    Ok(())
}

/// The page's visible size in points as shown (its rotation applied).
pub fn page_size(pdf: &Pdf, page: u32) -> Option<(f32, f32)> {
    let id = *page_ids(&pdf.doc).get(page as usize)?;
    let b = crate::pages::visible_box(&pdf.doc, id);
    let (w, h) = (b[2] - b[0], b[3] - b[1]);
    Some(if rotation(&pdf.doc, id) % 180 == 90 {
        (h, w)
    } else {
        (w, h)
    })
}
