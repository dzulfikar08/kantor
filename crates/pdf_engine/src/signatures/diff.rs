//! What changed after a signature: the signed revision against the whole
//! file, object by object, each changed object classified by where it sits
//! in the document (page content, an annotation, a form field, the DSS…),
//! the way Acrobat sorts "changes allowed" from "document modified".

use std::collections::{HashMap, HashSet};

use lopdf::{Document, Object, ObjectId};

use super::pdf::{as_dict, deref, dict_get, name_of, page_index, text_string};

/// One change made after a signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// Long-term validation data (the DSS) was added or updated.
    LtvAdded,
    /// Another signature was added (or an empty signature field).
    SignatureAdded {
        field: String,
    },
    /// A document time stamp was added.
    TimestampAdded,
    /// A form field's value changed.
    FormFilled {
        field: String,
    },
    AnnotationAdded {
        kind: String,
        page: Option<usize>,
    },
    AnnotationChanged {
        kind: String,
        page: Option<usize>,
    },
    AnnotationRemoved {
        page: Option<usize>,
    },
    /// What a page shows changed (content stream, resources, size).
    PageContentChanged {
        page: Option<usize>,
    },
    /// Pages added, removed or reordered.
    PagesChanged,
    /// Document properties (title, author…) or XMP metadata.
    MetadataChanged,
    /// Anything else, named by the catalog entry it hangs from.
    Other {
        what: String,
    },
}

/// How a change counts under DocMDP.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// DSS and document time stamps: always allowed.
    Ltv,
    /// Signing (allowed unless P = 1).
    Signing,
    /// Form filling (allowed unless P = 1).
    FormFill,
    /// Annotations (allowed only at P = 3, or with no certification).
    Annotation,
    /// Metadata: allowed unless P = 1.
    Metadata,
    /// Never allowed after signing.
    Modification,
}

impl Change {
    pub fn class(&self) -> Class {
        match self {
            Change::LtvAdded | Change::TimestampAdded => Class::Ltv,
            Change::SignatureAdded { .. } => Class::Signing,
            Change::FormFilled { .. } => Class::FormFill,
            Change::AnnotationAdded { .. }
            | Change::AnnotationChanged { .. }
            | Change::AnnotationRemoved { .. } => Class::Annotation,
            Change::MetadataChanged => Class::Metadata,
            Change::PageContentChanged { .. } | Change::PagesChanged | Change::Other { .. } => {
                Class::Modification
            }
        }
    }

    /// Whether DocMDP level `p` (None: no certification) allows it.
    pub fn allowed(&self, p: Option<u8>) -> bool {
        match (self.class(), p) {
            (Class::Ltv, _) => true,
            (Class::Modification, _) => false,
            (_, Some(1)) => false,
            (Class::Annotation, Some(2)) => false,
            _ => true,
        }
    }
}

/// Where an object sits.
#[derive(Clone, Debug)]
enum Role {
    Catalog,
    Dss,
    Metadata,
    PagesNode,
    Page(usize),
    AnnotList,
    PageContent(Option<usize>),
    Annot {
        owner: ObjectId,
        kind: String,
        page: Option<usize>,
    },
    Field {
        name: String,
        sig: bool,
    },
    SigValue {
        field: String,
        timestamp: bool,
    },
    FormRoot,
    /// The catalog's `/Extensions` (developer extensions such as ESIC or
    /// ADBE, declared by LTV updates), and the appearances in the name
    /// dictionary's `/AP` tree: not a change to the document.
    Extensions,
    /// The name dictionary (compared without its `/AP` tree).
    NamesRoot,
    Other(String),
}

struct Labeler<'a> {
    doc: &'a Document,
    labels: HashMap<ObjectId, Role>,
}

impl Labeler<'_> {
    /// Label `obj` (if a reference) and everything it reaches that has no
    /// label yet. /P and /Parent (back links) aren't followed.
    fn mark(&mut self, obj: &Object, role: &Role) {
        let mut stack = vec![obj.clone()];
        while let Some(o) = stack.pop() {
            let target = match &o {
                Object::Reference(id) => {
                    if self.labels.contains_key(id) {
                        continue;
                    }
                    self.labels.insert(*id, role.clone());
                    match self.doc.get_object(*id) {
                        Ok(t) => t.clone(),
                        Err(_) => continue,
                    }
                }
                other => other.clone(),
            };
            match target {
                Object::Array(a) => stack.extend(a),
                Object::Dictionary(d) => push_dict(&d, &mut stack),
                Object::Stream(s) => push_dict(&s.dict, &mut stack),
                _ => {}
            }
        }
    }

    fn label(&mut self, obj: &Object, role: Role) -> bool {
        match obj {
            Object::Reference(id) if !self.labels.contains_key(id) => {
                self.labels.insert(*id, role);
                true
            }
            Object::Reference(_) => false,
            _ => true,
        }
    }
}

fn push_dict(d: &lopdf::Dictionary, stack: &mut Vec<Object>) {
    for (k, v) in d.iter() {
        if k.as_slice() != b"P" && k.as_slice() != b"Parent" {
            stack.push(v.clone());
        }
    }
}

fn label_document(doc: &Document) -> HashMap<ObjectId, Role> {
    let mut l = Labeler {
        doc,
        labels: HashMap::new(),
    };
    let Ok(root) = doc.trailer.get(b"Root").cloned() else {
        return l.labels;
    };
    l.label(&root, Role::Catalog);
    let Some(catalog) = as_dict(doc, &root).cloned() else {
        return l.labels;
    };
    if let Ok(info) = doc.trailer.get(b"Info") {
        l.mark(info, &Role::Metadata);
    }
    if let Ok(dss) = catalog.get(b"DSS") {
        l.mark(dss, &Role::Dss);
    }
    if let Ok(m) = catalog.get(b"Metadata") {
        l.mark(m, &Role::Metadata);
    }
    if let Ok(e) = catalog.get(b"Extensions") {
        l.mark(e, &Role::Extensions);
    }
    // The name dictionary: its /AP tree holds annotation appearances (the
    // annotations themselves are reported); every other tree is a change.
    if let Ok(n) = catalog.get(b"Names")
        && let Some(names) = as_dict(doc, n).cloned()
    {
        if let Ok(ap) = names.get(b"AP") {
            l.mark(ap, &Role::Extensions);
        }
        if let Object::Reference(id) = n {
            l.labels.entry(*id).or_insert(Role::NamesRoot);
        }
        for (k, v) in names.iter() {
            if k != b"AP" {
                l.mark(v, &Role::Other("Names".into()));
            }
        }
    }
    // The page tree.
    let pages = page_index(doc);
    if let Ok(p) = catalog.get(b"Pages") {
        let mut stack = vec![p.clone()];
        let mut seen = HashSet::new();
        while let Some(node) = stack.pop() {
            let Object::Reference(id) = node else {
                continue;
            };
            if !seen.insert(id) {
                continue;
            }
            let Some(d) = as_dict(doc, &node).cloned() else {
                continue;
            };
            let is_page = name_of(d.get(b"Type").ok()) == Some("Page".into())
                || (pages.contains_key(&id) && !d.has(b"Kids"));
            if is_page {
                let n = pages.get(&id).copied().unwrap_or(0);
                l.label(&node, Role::Page(n));
                for (k, v) in d.iter() {
                    match k.as_slice() {
                        b"Parent" | b"Annots" | b"B" => {}
                        _ => l.mark(v, &Role::PageContent(Some(n))),
                    }
                }
            } else {
                l.label(&node, Role::PagesNode);
                if let Ok(Object::Array(kids)) = d.get(b"Kids").map(|k| deref(doc, k)) {
                    stack.extend(kids.iter().cloned());
                }
                for (k, v) in d.iter() {
                    if matches!(k.as_slice(), b"Resources" | b"MediaBox" | b"CropBox") {
                        l.mark(v, &Role::PageContent(None));
                    }
                }
            }
        }
    }
    // Annotations, page by page.
    let mut by_page: Vec<(usize, ObjectId)> = pages.iter().map(|(id, n)| (*n, *id)).collect();
    by_page.sort();
    for (n, pid) in &by_page {
        let Ok(Object::Dictionary(pd)) = doc.get_object(*pid) else {
            continue;
        };
        let Ok(annots) = pd.get(b"Annots") else {
            continue;
        };
        l.label(annots, Role::AnnotList);
        let Object::Array(list) = deref(doc, annots) else {
            continue;
        };
        for a in list.clone() {
            let Some(ad) = as_dict(doc, &a).cloned() else {
                continue;
            };
            let subtype = name_of(ad.get(b"Subtype").ok()).unwrap_or_default();
            if subtype == "Widget" {
                continue; // labelled with the form below
            }
            let owner = a.as_reference().unwrap_or((0, 0));
            l.mark(
                &a,
                &Role::Annot {
                    owner,
                    kind: subtype,
                    page: Some(*n),
                },
            );
        }
    }
    // The form: fields, widgets, signature values.
    if let Ok(acro) = catalog.get(b"AcroForm") {
        l.label(acro, Role::FormRoot);
        if let Some(ad) = as_dict(doc, acro).cloned() {
            for (k, v) in ad.iter() {
                match k.as_slice() {
                    b"Fields" => {
                        l.label(v, Role::FormRoot);
                        if let Object::Array(list) = deref(doc, v) {
                            for f in list.clone() {
                                label_field(&mut l, &f, None, false);
                            }
                        }
                    }
                    _ => l.mark(v, &Role::FormRoot),
                }
            }
        }
    }
    // Widgets not in the field tree.
    for (_, pid) in &by_page {
        let Ok(Object::Dictionary(pd)) = doc.get_object(*pid) else {
            continue;
        };
        if let Some(Object::Array(list)) = dict_get(doc, pd, b"Annots") {
            for a in list.clone() {
                l.mark(
                    &a,
                    &Role::Field {
                        name: String::new(),
                        sig: false,
                    },
                );
            }
        }
    }
    // Everything else the catalog reaches.
    for (k, v) in catalog.iter() {
        let key = String::from_utf8_lossy(k).into_owned();
        l.mark(v, &Role::Other(key));
    }
    l.labels
}

fn label_field(l: &mut Labeler<'_>, f: &Object, parent: Option<&str>, parent_sig: bool) {
    let Some(d) = as_dict(l.doc, f).cloned() else {
        return;
    };
    let partial = match dict_get(l.doc, &d, b"T") {
        Some(Object::String(s, _)) => Some(text_string(s)),
        _ => None,
    };
    let name = match (parent, partial) {
        (Some(p), Some(t)) => format!("{p}.{t}"),
        (None, Some(t)) => t,
        (Some(p), None) => p.to_string(),
        (None, None) => String::new(),
    };
    let sig = name_of(dict_get(l.doc, &d, b"FT")).as_deref() == Some("Sig") || parent_sig;
    if !l.label(
        f,
        Role::Field {
            name: name.clone(),
            sig,
        },
    ) {
        return;
    }
    for (k, v) in d.iter() {
        match k.as_slice() {
            b"Kids" => {
                l.label(
                    v,
                    Role::Field {
                        name: name.clone(),
                        sig,
                    },
                );
                if let Object::Array(kids) = deref(l.doc, v) {
                    for kid in kids.clone() {
                        label_field(l, &kid, Some(&name), sig);
                    }
                }
            }
            b"V" if sig => {
                let ts = as_dict(l.doc, v)
                    .map(|vd| {
                        name_of(vd.get(b"Type").ok()).as_deref() == Some("DocTimeStamp")
                            || name_of(vd.get(b"SubFilter").ok()).as_deref() == Some("ETSI.RFC3161")
                    })
                    .unwrap_or(false);
                l.mark(
                    v,
                    &Role::SigValue {
                        field: name.clone(),
                        timestamp: ts,
                    },
                );
            }
            b"P" | b"Parent" => {}
            _ => l.mark(
                v,
                &Role::Field {
                    name: name.clone(),
                    sig,
                },
            ),
        }
    }
}

/// The same object, however its strings are written (a literal and a hex
/// string with the same bytes are equal: writers re-encode them).
pub(crate) fn same(a: &Object, b: &Object) -> bool {
    match (a, b) {
        (Object::String(x, _), Object::String(y, _)) => x == y,
        (Object::Array(x), Object::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b))
        }
        (Object::Dictionary(x), Object::Dictionary(y)) => same_dict(x, y),
        (Object::Stream(x), Object::Stream(y)) => {
            same_dict(&x.dict, &y.dict) && x.content == y.content
        }
        _ => a == b,
    }
}

fn same_dict(x: &lopdf::Dictionary, y: &lopdf::Dictionary) -> bool {
    x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_ok_and(|w| same(v, w)))
}

fn same_opt(a: Option<&Object>, b: Option<&Object>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => same(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// The catalog's name dictionary without its `/AP` tree (the appearances
/// Acrobat names when stamps are added: part of the annotations).
fn names_without_ap(doc: &Document, o: Option<&Object>) -> Option<lopdf::Dictionary> {
    let mut d = as_dict(doc, o?)?.clone();
    d.remove(b"AP");
    Some(d)
}

fn annot_refs(doc: &Document, page: ObjectId) -> HashSet<ObjectId> {
    let Ok(Object::Dictionary(pd)) = doc.get_object(page) else {
        return HashSet::new();
    };
    match dict_get(doc, pd, b"Annots") {
        Some(Object::Array(a)) => a.iter().filter_map(|o| o.as_reference().ok()).collect(),
        _ => HashSet::new(),
    }
}

fn without_annots(o: &Object) -> Option<lopdf::Dictionary> {
    match o {
        Object::Dictionary(d) => {
            let mut d = d.clone();
            d.remove(b"Annots");
            Some(d)
        }
        _ => None,
    }
}

/// The changes in `full` that aren't in `signed` (an earlier revision of
/// the same file).
pub fn changes(signed: &Document, full: &Document) -> Vec<Change> {
    let labels = label_document(full);
    let mut out: Vec<Change> = Vec::new();
    let mut push = |c: Change| {
        if !out.contains(&c) {
            out.push(c);
        }
    };
    let mut ids: Vec<&ObjectId> = full.objects.keys().collect();
    ids.sort();
    let mut seen_owners = HashSet::new();
    for id in ids {
        let obj = &full.objects[id];
        let before = signed.objects.get(id);
        if before.is_some_and(|b| same(b, obj)) {
            continue;
        }
        let Some(role) = labels.get(id) else {
            continue; // unreachable: not part of the document
        };
        match role {
            Role::Catalog => {
                let (Some(Object::Dictionary(new)), Some(Object::Dictionary(old))) =
                    (Some(obj), before)
                else {
                    push(Change::Other {
                        what: "Catalog".into(),
                    });
                    continue;
                };
                let keys: HashSet<&Vec<u8>> = new
                    .iter()
                    .map(|(k, _)| k)
                    .chain(old.iter().map(|(k, _)| k))
                    .collect();
                for k in keys {
                    if same_opt(new.get(k).ok(), old.get(k).ok()) {
                        continue;
                    }
                    match k.as_slice() {
                        b"DSS" => push(Change::LtvAdded),
                        b"Names" => {
                            let (n, o) = (
                                names_without_ap(full, new.get(k).ok()),
                                names_without_ap(signed, old.get(k).ok()),
                            );
                            let empty = |d: &Option<lopdf::Dictionary>| {
                                d.as_ref().is_none_or(|d| d.is_empty())
                            };
                            let differ = match (&n, &o) {
                                (Some(n), Some(o)) => !same_dict(n, o),
                                _ => !(empty(&n) && empty(&o)),
                            };
                            if differ {
                                push(Change::Other {
                                    what: "Names".into(),
                                });
                            }
                        }
                        b"AcroForm" | b"Extensions" | b"Version" => {}
                        b"Metadata" => push(Change::MetadataChanged),
                        b"Pages" => push(Change::PagesChanged),
                        other => push(Change::Other {
                            what: String::from_utf8_lossy(other).into_owned(),
                        }),
                    }
                }
            }
            Role::Dss => push(Change::LtvAdded),
            Role::Metadata => push(Change::MetadataChanged),
            Role::PagesNode => {
                let kids = |o: Option<&Object>| {
                    o.and_then(|o| as_dict(full, o))
                        .and_then(|d| d.get(b"Kids").ok().cloned())
                };
                // Only attributes the pages inherit change what they show;
                // a producer's private key (iText's /ITXT) doesn't.
                let shown = |o: Option<&Object>| {
                    o.and_then(|o| as_dict(full, o)).map(|d| {
                        [&b"Resources"[..], b"MediaBox", b"CropBox", b"Rotate"]
                            .map(|k| d.get(k).ok().cloned())
                    })
                };
                if kids(Some(obj)) != kids(before) {
                    push(Change::PagesChanged);
                } else if shown(Some(obj)) != shown(before) {
                    push(Change::PageContentChanged { page: None });
                }
            }
            Role::Page(n) => {
                if before.and_then(without_annots) != without_annots(obj) {
                    push(Change::PageContentChanged { page: Some(*n) });
                }
            }
            Role::AnnotList => {}
            Role::PageContent(n) => push(Change::PageContentChanged { page: *n }),
            Role::Annot { owner, kind, page } => {
                if !seen_owners.insert(*owner) {
                    continue;
                }
                if signed.objects.contains_key(owner) {
                    push(Change::AnnotationChanged {
                        kind: kind.clone(),
                        page: *page,
                    });
                } else {
                    push(Change::AnnotationAdded {
                        kind: kind.clone(),
                        page: *page,
                    });
                }
            }
            Role::Field { name, sig: true } => push(Change::SignatureAdded {
                field: name.clone(),
            }),
            Role::Field { name, sig: false } => push(Change::FormFilled {
                field: name.clone(),
            }),
            Role::SigValue {
                timestamp: true, ..
            } => push(Change::TimestampAdded),
            Role::SigValue { field, .. } => push(Change::SignatureAdded {
                field: field.clone(),
            }),
            Role::FormRoot | Role::Extensions => {}
            Role::NamesRoot => {
                let differ = match (
                    names_without_ap(full, Some(obj)),
                    before.and_then(|b| names_without_ap(signed, Some(b))),
                ) {
                    (Some(n), Some(o)) => !same_dict(&n, &o),
                    (Some(n), None) => !n.is_empty(),
                    _ => true,
                };
                if differ {
                    push(Change::Other {
                        what: "Names".into(),
                    });
                }
            }
            Role::Other(what) => push(Change::Other { what: what.clone() }),
        }
    }
    // Annotations removed, and pages added or removed.
    let (old_pages, new_pages) = (signed.get_pages(), full.get_pages());
    if old_pages.len() != new_pages.len() {
        push(Change::PagesChanged);
    }
    for (n, pid) in &new_pages {
        if old_pages.get(n) != Some(pid) {
            continue;
        }
        let (old, new) = (annot_refs(signed, *pid), annot_refs(full, *pid));
        if old.iter().any(|a| !new.contains(a)) {
            push(Change::AnnotationRemoved {
                page: Some(*n as usize),
            });
        }
    }
    // A timestamp field's widget reads as a signature field: keep one entry.
    if out.contains(&Change::TimestampAdded) {
        let ts_fields: Vec<String> = labels
            .values()
            .filter_map(|r| match r {
                Role::SigValue {
                    field,
                    timestamp: true,
                } => Some(field.clone()),
                _ => None,
            })
            .collect();
        out.retain(|c| !matches!(c, Change::SignatureAdded { field } if ts_fields.contains(field)));
    }
    out
}
