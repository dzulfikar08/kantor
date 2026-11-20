//! The PDF side of digital signing (ISO 32000-1 §12.8, PAdES baseline,
//! ETSI EN 319 142-1): the signature field and its appearance, the
//! signature dictionary with a reserved `/Contents`, DocMDP certification,
//! document time stamps and the Document Security Store (DSS).
//!
//! Every step is an incremental update, so earlier signatures stay intact.
//! [`prepare`] writes the update with a zero-filled `/Contents` and the
//! final `/ByteRange`; the caller digests [`Prepared::signed_parts`], makes
//! the CMS (or RFC 3161 token) and puts it in with [`Prepared::finish`].
//! The cryptography lives in `pdf_engine::signing`.

use std::collections::HashSet;

use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat};

use crate::annot::{Art, Font, Seg, tj_string};
use crate::pages::{page_ids, rect};
use crate::pdfstr::{get_text, num, pdf_date, text};
use crate::{Error, Pdf, Result};

/// A signature field of the document.
#[derive(Clone, Debug, PartialEq)]
pub struct SigField {
    /// Full field name.
    pub name: String,
    /// 0-based page of its widget, if it has one on a page.
    pub page: Option<u32>,
    /// Its box on the page (user space), `[0 0 0 0]` when invisible.
    pub rect: [f32; 4],
    /// Already signed (has a `/V`).
    pub signed: bool,
}

/// Where the signature goes.
#[derive(Clone, Debug, PartialEq)]
pub enum Place {
    /// A new field with no box (shown only in the signature panel).
    Invisible,
    /// An existing empty signature field, by full name.
    Field(String),
    /// A new field drawn on a page (0-based) at `rect` (user space).
    New { page: u32, rect: [f32; 4] },
}

/// What a visible signature shows: text lines (name, date, reason…) and
/// an optional picture on their left.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Look {
    pub lines: Vec<String>,
    pub art: Option<Art>,
}

/// What [`prepare`] adds.
#[derive(Clone, Debug, PartialEq)]
pub enum SigType {
    /// A signature (`/SubFilter /ETSI.CAdES.detached`); `certify` makes it
    /// a certification signature with DocMDP permission 1, 2 or 3.
    Signature { certify: Option<u8> },
    /// A document time stamp (`/Type /DocTimeStamp`, `/SubFilter
    /// /ETSI.RFC3161`, PAdES B-LTA).
    DocTimeStamp,
}

/// Everything [`prepare`] needs.
#[derive(Clone, Debug)]
pub struct Request {
    pub kind: SigType,
    pub place: Place,
    /// Name for a new field (default `Signature1`, `Signature2`…).
    pub field_name: Option<String>,
    pub look: Option<Look>,
    pub name: Option<String>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub contact: Option<String>,
    /// Signing time for `/M` (Unix seconds, UTC).
    pub time: i64,
    /// Bytes reserved for the CMS in `/Contents`.
    pub reserve: usize,
    /// Program name and version for `/Prop_Build`.
    pub app: Option<(String, String)>,
}

impl Request {
    pub fn new(kind: SigType, place: Place, time: i64, reserve: usize) -> Request {
        Request {
            kind,
            place,
            field_name: None,
            look: None,
            name: None,
            reason: None,
            location: None,
            contact: None,
            time,
            reserve,
            app: None,
        }
    }
}

/// A document with a signature waiting for its CMS.
#[derive(Clone, Debug)]
pub struct Prepared {
    bytes: Vec<u8>,
    /// `/ByteRange`: offset and length of the two signed parts.
    pub byte_range: [usize; 4],
}

impl Prepared {
    /// The bytes the signature covers: everything but the `/Contents` hex
    /// string.
    pub fn signed_parts(&self) -> [&[u8]; 2] {
        let [a, b, c, d] = self.byte_range;
        [&self.bytes[a..a + b], &self.bytes[c..c + d]]
    }

    /// How many bytes of CMS fit.
    pub fn capacity(&self) -> usize {
        (self.byte_range[2] - self.byte_range[1] - 2) / 2
    }

    /// Put `cms` (DER) into `/Contents`. Fails when it doesn't fit.
    pub fn finish(mut self, cms: &[u8]) -> Result<Vec<u8>> {
        if cms.len() > self.capacity() {
            return Err(Error::Invalid(format!(
                "the signature ({} bytes) is larger than the space reserved for it ({} bytes)",
                cms.len(),
                self.capacity()
            )));
        }
        let start = self.byte_range[1] + 1;
        for (i, b) in cms.iter().enumerate() {
            let hex = format!("{b:02X}");
            self.bytes[start + 2 * i..start + 2 * i + 2].copy_from_slice(hex.as_bytes());
        }
        Ok(self.bytes)
    }
}

/// The document's signature fields.
pub fn signature_fields(pdf: &Pdf) -> Vec<SigField> {
    let doc = &pdf.doc;
    let pages: Vec<ObjectId> = page_ids(doc);
    sig_fields(doc)
        .into_iter()
        .map(|f| {
            let w = f.widget.and_then(|w| doc.get_dictionary(w).ok());
            let page = f.widget.and_then(|w| widget_page(doc, &pages, w));
            SigField {
                name: f.name,
                page,
                rect: w
                    .and_then(|w| w.get(b"Rect").ok())
                    .and_then(|r| rect(doc, r))
                    .unwrap_or([0.0; 4]),
                signed: f.signed,
            }
        })
        .collect()
}

struct Found {
    name: String,
    /// The field dictionary (holds /V).
    field: ObjectId,
    /// Its widget (often the same object).
    widget: Option<ObjectId>,
    signed: bool,
}

fn acro_form_dict(doc: &Document) -> Option<Dictionary> {
    let af = doc.catalog().ok()?.get(b"AcroForm").ok()?;
    doc.dereference(af).ok()?.1.as_dict().ok().cloned()
}

fn sig_fields(doc: &Document) -> Vec<Found> {
    let Some(af) = acro_form_dict(doc) else {
        return Vec::new();
    };
    let roots = af
        .get(b"Fields")
        .ok()
        .and_then(|o| doc.dereference(o).ok())
        .and_then(|(_, o)| o.as_array().ok().cloned())
        .unwrap_or_default();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for r in roots {
        if let Ok(id) = r.as_reference() {
            walk(doc, id, "", None, &mut out, &mut seen, 0);
        }
    }
    out
}

fn walk(
    doc: &Document,
    id: ObjectId,
    parent: &str,
    ft: Option<Vec<u8>>,
    out: &mut Vec<Found>,
    seen: &mut HashSet<ObjectId>,
    depth: u32,
) {
    if depth > 32 || !seen.insert(id) {
        return;
    }
    let Ok(d) = doc.get_dictionary(id) else {
        return;
    };
    let name = match get_text(doc, d, b"T") {
        Some(t) if parent.is_empty() => t,
        Some(t) => format!("{parent}.{t}"),
        None => parent.to_string(),
    };
    let ft = d
        .get(b"FT")
        .and_then(Object::as_name)
        .ok()
        .map(<[u8]>::to_vec)
        .or(ft);
    let kids: Vec<ObjectId> = d
        .get(b"Kids")
        .ok()
        .and_then(|o| doc.dereference(o).ok())
        .and_then(|(_, o)| o.as_array().ok())
        .map(|a| a.iter().filter_map(|k| k.as_reference().ok()).collect())
        .unwrap_or_default();
    // Kids with /T are fields; kids without are this field's widgets.
    let (fields, widgets): (Vec<ObjectId>, Vec<ObjectId>) = kids
        .into_iter()
        .partition(|k| doc.get_dictionary(*k).is_ok_and(|k| k.has(b"T")));
    for k in fields {
        walk(doc, k, &name, ft.clone(), out, seen, depth + 1);
    }
    if ft.as_deref() != Some(b"Sig") {
        return;
    }
    let is_widget = d
        .get(b"Subtype")
        .and_then(Object::as_name)
        .is_ok_and(|s| s == b"Widget");
    let widget = if is_widget {
        Some(id)
    } else {
        widgets.first().copied()
    };
    if d.has(b"T") && (is_widget || widget.is_some() || !d.has(b"Kids")) {
        out.push(Found {
            name,
            field: id,
            widget,
            signed: d.get(b"V").is_ok_and(|v| !matches!(v, Object::Null)),
        });
    }
}

fn widget_page(doc: &Document, pages: &[ObjectId], widget: ObjectId) -> Option<u32> {
    if let Some(p) = doc
        .get_dictionary(widget)
        .ok()
        .and_then(|w| w.get(b"P").ok())
        .and_then(|p| p.as_reference().ok())
        && let Some(i) = pages.iter().position(|x| *x == p)
    {
        return Some(i as u32);
    }
    pages
        .iter()
        .position(|p| {
            crate::annot::page_annots(doc, *p)
                .iter()
                .any(|(id, _)| *id == Some(widget))
        })
        .map(|i| i as u32)
}

const PLACEHOLDER: &str = "[0 1111111111 2222222222 3333333333]";

/// Add a signature (or document time stamp) with an empty `/Contents` of
/// `req.reserve` bytes as an incremental update of `bytes`.
pub fn prepare(bytes: &[u8], req: &Request) -> Result<Prepared> {
    let pdf = Pdf::from_bytes(bytes, None)?;
    if pdf.was_encrypted() {
        return Err(Error::Invalid(
            "signing password-protected PDFs isn't supported yet".into(),
        ));
    }
    let existing = crate::sig::signatures(&pdf);
    if existing.is_some_and(|s| s.docmdp == Some(1)) {
        return Err(Error::Invalid(
            "the document was certified with no changes allowed; it can't be signed again".into(),
        ));
    }
    if let SigType::Signature { certify: Some(p) } = req.kind {
        if !(1..=3).contains(&p) {
            return Err(Error::Invalid(format!(
                "the certification level must be 1, 2 or 3 (not {p})"
            )));
        }
        if existing.is_some() {
            return Err(Error::Invalid(
                "only the first signature of a document can certify it; this one is already signed"
                    .into(),
            ));
        }
    }
    // After a certification, a new visible field changes the page: only
    // existing empty fields or invisible signatures are allowed changes
    // (as Acrobat and pyHanko judge DocMDP).
    if existing.is_some_and(|s| s.docmdp.is_some()) && matches!(req.place, Place::New { .. }) {
        return Err(Error::Invalid(
            "the document is certified: sign one of its empty signature fields, or sign invisibly"
                .into(),
        ));
    }
    if req.reserve == 0 || req.reserve > 1 << 20 {
        return Err(Error::Invalid("bad signature size".into()));
    }
    let out = crate::sig::incremental(bytes, None, |pdf| add_signature(&mut pdf.doc, req))?;
    locate(out, bytes.len(), req.reserve)
}

/// Find the placeholders in the update and fill in `/ByteRange`.
fn locate(mut out: Vec<u8>, from: usize, reserve: usize) -> Result<Prepared> {
    let missing = || Error::Pdf("the signature placeholder wasn't written".into());
    let tail = &out[from..];
    let br = find(tail, PLACEHOLDER.as_bytes()).ok_or_else(missing)? + from;
    let mut hex = Vec::with_capacity(reserve * 2 + 2);
    hex.push(b'<');
    hex.resize(reserve * 2 + 1, b'0');
    hex.push(b'>');
    let c0 = find(&out[from..], &hex).ok_or_else(missing)? + from;
    let c1 = c0 + hex.len();
    let range = [0, c0, c1, out.len() - c1];
    let mut text = format!("[0 {} {} {}", range[1], range[2], range[3]);
    if text.len() + 1 > PLACEHOLDER.len() {
        return Err(Error::Pdf("the file is too large to sign".into()));
    }
    while text.len() + 1 < PLACEHOLDER.len() {
        text.push(' ');
    }
    text.push(']');
    out[br..br + PLACEHOLDER.len()].copy_from_slice(text.as_bytes());
    Ok(Prepared {
        bytes: out,
        byte_range: range,
    })
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn name_obj(s: &str) -> Object {
    Object::Name(s.as_bytes().to_vec())
}

fn add_signature(doc: &mut Document, req: &Request) -> Result<()> {
    // The signature dictionary.
    let mut sig = Dictionary::new();
    match req.kind {
        SigType::Signature { .. } => {
            sig.set("Type", name_obj("Sig"));
            sig.set("Filter", name_obj("Adobe.PPKLite"));
            sig.set("SubFilter", name_obj("ETSI.CAdES.detached"));
        }
        SigType::DocTimeStamp => {
            sig.set("Type", name_obj("DocTimeStamp"));
            sig.set("Filter", name_obj("Adobe.PPKLite"));
            sig.set("SubFilter", name_obj("ETSI.RFC3161"));
        }
    }
    sig.set(
        "ByteRange",
        Object::Array(
            [0i64, 1111111111, 2222222222, 3333333333]
                .into_iter()
                .map(Object::Integer)
                .collect(),
        ),
    );
    sig.set(
        "Contents",
        Object::String(vec![0; req.reserve], StringFormat::Hexadecimal),
    );
    if matches!(req.kind, SigType::Signature { .. }) {
        sig.set(
            "M",
            Object::String(pdf_date(req.time).into_bytes(), StringFormat::Literal),
        );
        for (key, v) in [
            ("Name", &req.name),
            ("Reason", &req.reason),
            ("Location", &req.location),
            ("ContactInfo", &req.contact),
        ] {
            if let Some(v) = v.as_deref().filter(|v| !v.trim().is_empty()) {
                sig.set(key, text(v));
            }
        }
        if let Some((app, version)) = &req.app {
            let mut a = Dictionary::new();
            a.set("Name", name_obj(app));
            a.set(
                "REx",
                Object::String(version.as_bytes().to_vec(), StringFormat::Literal),
            );
            let mut pb = Dictionary::new();
            pb.set("App", a);
            sig.set("Prop_Build", pb);
        }
    }
    let certify = match req.kind {
        SigType::Signature { certify } => certify,
        SigType::DocTimeStamp => None,
    };
    if let Some(p) = certify {
        let mut params = Dictionary::new();
        params.set("Type", name_obj("TransformParams"));
        params.set("P", p as i64);
        params.set("V", name_obj("1.2"));
        let mut r = Dictionary::new();
        r.set("Type", name_obj("SigRef"));
        r.set("TransformMethod", name_obj("DocMDP"));
        r.set("TransformParams", params);
        sig.set("Reference", Object::Array(vec![Object::Dictionary(r)]));
    }
    let sig_id = doc.add_object(sig);
    if certify.is_some() {
        let cat = doc.catalog_mut()?;
        let mut perms = match cat.get(b"Perms") {
            Ok(Object::Dictionary(d)) => d.clone(),
            _ => Dictionary::new(),
        };
        perms.set("DocMDP", sig_id);
        cat.set("Perms", perms);
    }

    // The field.
    let pages = page_ids(doc);
    match &req.place {
        Place::Field(name) => {
            let found = sig_fields(doc)
                .into_iter()
                .find(|f| &f.name == name)
                .ok_or_else(|| Error::Invalid(format!("no signature field named \"{name}\"")))?;
            if found.signed {
                return Err(Error::Invalid(format!(
                    "the signature field \"{name}\" is already signed"
                )));
            }
            doc.get_dictionary_mut(found.field)?.set("V", sig_id);
            if let Some(w) = found.widget {
                let r = doc
                    .get_dictionary(w)
                    .ok()
                    .and_then(|d| d.get(b"Rect").ok())
                    .and_then(|r| rect(doc, r))
                    .unwrap_or([0.0; 4]);
                let ap = appearance(doc, req.look.as_ref(), r)?;
                let ap_id = doc.add_object(ap);
                let mut apd = Dictionary::new();
                apd.set("N", ap_id);
                let wd = doc.get_dictionary_mut(w)?;
                wd.set("AP", apd);
                // Signed widgets are printed and locked in place.
                let f = wd.get(b"F").and_then(Object::as_i64).unwrap_or(0);
                wd.set("F", f | 4 | 128);
            }
        }
        Place::Invisible | Place::New { .. } => {
            let (page_ix, r) = match &req.place {
                Place::New { page, rect } => (*page as usize, crate::annot::norm(*rect)),
                _ => (0, [0.0; 4]),
            };
            let page = *pages.get(page_ix).ok_or_else(|| {
                Error::Invalid(format!(
                    "page {} is outside the document ({} pages)",
                    page_ix + 1,
                    pages.len()
                ))
            })?;
            let existing: HashSet<String> = sig_fields(doc).into_iter().map(|f| f.name).collect();
            let name = match &req.field_name {
                Some(n) if !n.is_empty() => {
                    if existing.contains(n) {
                        return Err(Error::Invalid(format!(
                            "a field named \"{n}\" already exists; sign it with --field"
                        )));
                    }
                    n.clone()
                }
                _ => {
                    let base = match req.kind {
                        SigType::DocTimeStamp => "Timestamp",
                        _ => "Signature",
                    };
                    (1..)
                        .map(|i| format!("{base}{i}"))
                        .find(|n| !existing.contains(n) && !any_field_named(doc, n))
                        .unwrap_or_default()
                }
            };
            let mut w = Dictionary::new();
            w.set("Type", name_obj("Annot"));
            w.set("Subtype", name_obj("Widget"));
            w.set("FT", name_obj("Sig"));
            w.set("T", text(&name));
            w.set("V", sig_id);
            w.set(
                "Rect",
                Object::Array(r.iter().map(|v| Object::Real(*v)).collect()),
            );
            w.set("F", 4 | 128);
            w.set("P", page);
            if r[2] > r[0] && r[3] > r[1] {
                let ap = appearance(doc, req.look.as_ref(), r)?;
                let ap_id = doc.add_object(ap);
                let mut apd = Dictionary::new();
                apd.set("N", ap_id);
                w.set("AP", apd);
            }
            let wid = doc.add_object(w);
            add_to_annots(doc, page, wid)?;
            add_to_fields(doc, wid)?;
        }
    }
    set_sig_flags(doc)?;
    Ok(())
}

fn any_field_named(doc: &Document, name: &str) -> bool {
    let Some(af) = acro_form_dict(doc) else {
        return false;
    };
    af.get(b"Fields")
        .ok()
        .and_then(|o| doc.dereference(o).ok())
        .and_then(|(_, o)| o.as_array().ok())
        .is_some_and(|a| {
            a.iter().any(|f| {
                doc.dereference(f)
                    .ok()
                    .and_then(|(_, f)| f.as_dict().ok())
                    .and_then(|f| get_text(doc, f, b"T"))
                    .is_some_and(|t| t == name)
            })
        })
}

fn add_to_annots(doc: &mut Document, page: ObjectId, wid: ObjectId) -> Result<()> {
    let annots = doc.get_dictionary(page)?.get(b"Annots").ok().cloned();
    match annots {
        Some(Object::Reference(aid)) => {
            if let Ok(Object::Array(a)) = doc.get_object_mut(aid) {
                a.push(wid.into());
                return Ok(());
            }
            doc.get_dictionary_mut(page)?
                .set("Annots", Object::Array(vec![wid.into()]));
        }
        Some(Object::Array(mut a)) => {
            a.push(wid.into());
            doc.get_dictionary_mut(page)?.set("Annots", a);
        }
        _ => {
            doc.get_dictionary_mut(page)?
                .set("Annots", Object::Array(vec![wid.into()]));
        }
    }
    Ok(())
}

/// The AcroForm dictionary's object id, made indirect (and created) if
/// needed so later updates touch only it, not the catalog.
fn acro_form_id(doc: &mut Document) -> Result<ObjectId> {
    match doc.catalog()?.get(b"AcroForm") {
        Ok(Object::Reference(id)) => return Ok(*id),
        Ok(Object::Dictionary(d)) => {
            let d = d.clone();
            let id = doc.add_object(d);
            doc.catalog_mut()?.set("AcroForm", id);
            return Ok(id);
        }
        _ => {}
    }
    let mut d = Dictionary::new();
    d.set("Fields", Object::Array(Vec::new()));
    let id = doc.add_object(d);
    doc.catalog_mut()?.set("AcroForm", id);
    Ok(id)
}

fn add_to_fields(doc: &mut Document, wid: ObjectId) -> Result<()> {
    let af = acro_form_id(doc)?;
    let fields = doc.get_dictionary(af)?.get(b"Fields").ok().cloned();
    match fields {
        Some(Object::Reference(fid)) => {
            if let Ok(Object::Array(a)) = doc.get_object_mut(fid) {
                a.push(wid.into());
                return Ok(());
            }
            doc.get_dictionary_mut(af)?
                .set("Fields", Object::Array(vec![wid.into()]));
        }
        Some(Object::Array(mut a)) => {
            a.push(wid.into());
            doc.get_dictionary_mut(af)?.set("Fields", a);
        }
        _ => {
            doc.get_dictionary_mut(af)?
                .set("Fields", Object::Array(vec![wid.into()]));
        }
    }
    Ok(())
}

fn set_sig_flags(doc: &mut Document) -> Result<()> {
    let af = acro_form_id(doc)?;
    let d = doc.get_dictionary_mut(af)?;
    let flags = d.get(b"SigFlags").and_then(Object::as_i64).unwrap_or(0);
    if flags & 3 != 3 {
        d.set("SigFlags", flags | 3);
    }
    Ok(())
}

/// The widget's normal appearance: the picture on the left (or alone),
/// the lines of text on the right, sized to fit the box.
fn appearance(doc: &mut Document, look: Option<&Look>, r: [f32; 4]) -> Result<Stream> {
    let (w, h) = ((r[2] - r[0]).abs().max(1.0), (r[3] - r[1]).abs().max(1.0));
    let mut res = Dictionary::new();
    let mut s = String::new();
    let empty = Look::default();
    let look = look.unwrap_or(&empty);
    let lines: Vec<&str> = look
        .lines
        .iter()
        .map(String::as_str)
        .filter(|l| !l.trim().is_empty())
        .collect();
    let pad = (h * 0.06).clamp(1.0, 4.0);
    let mut text_x = pad;
    if let Some(art) = &look.art {
        let art_w = if lines.is_empty() { w } else { w * 0.42 };
        let (bx, by, bw, bh) = (pad, pad, art_w - 2.0 * pad, h - 2.0 * pad);
        match art {
            Art::Image(bytes) => {
                let img = crate::images::embed(bytes, "signature picture")?;
                let mut xo = img.xobject;
                if let Some(mask) = img.smask {
                    let id = doc.add_object(mask);
                    xo.dict.set("SMask", id);
                }
                let (iw, ih) = (
                    xo.dict.get(b"Width").and_then(Object::as_i64).unwrap_or(1) as f32,
                    xo.dict.get(b"Height").and_then(Object::as_i64).unwrap_or(1) as f32,
                );
                let id = doc.add_object(xo);
                let mut xd = Dictionary::new();
                xd.set("Im0", id);
                res.set("XObject", xd);
                // Keep the picture's proportions, centred in its part.
                let turned = img.exif >= 5;
                let (iw, ih) = if turned { (ih, iw) } else { (iw, ih) };
                let k = (bw / iw).min(bh / ih).max(0.0);
                let (dw, dh) = (iw * k, ih * k);
                let (dx, dy) = (bx + (bw - dw) / 2.0, by + (bh - dh) / 2.0);
                let m = crate::images::placement(img.exif, dx, dy, dw, dh);
                s.push_str(&format!(
                    "q {} {} {} {} {} {} cm /Im0 Do Q\n",
                    num(m[0]),
                    num(m[1]),
                    num(m[2]),
                    num(m[3]),
                    num(m[4]),
                    num(m[5])
                ));
            }
            Art::Paths(segs) => {
                s.push_str("q 0 0 0.5 rg\n");
                let pt = |x: f32, y: f32| (bx + x * bw, by + y * bh);
                for seg in segs.iter() {
                    match *seg {
                        Seg::Move(x, y) => {
                            let (x, y) = pt(x, y);
                            s.push_str(&format!("{} {} m\n", num(x), num(y)));
                        }
                        Seg::Line(x, y) => {
                            let (x, y) = pt(x, y);
                            s.push_str(&format!("{} {} l\n", num(x), num(y)));
                        }
                        Seg::Curve(a1, b1, a2, b2, x, y) => {
                            let (a1, b1) = pt(a1, b1);
                            let (a2, b2) = pt(a2, b2);
                            let (x, y) = pt(x, y);
                            s.push_str(&format!(
                                "{} {} {} {} {} {} c\n",
                                num(a1),
                                num(b1),
                                num(a2),
                                num(b2),
                                num(x),
                                num(y)
                            ));
                        }
                        Seg::Close => s.push_str("h\n"),
                    }
                }
                s.push_str("f Q\n");
            }
            Art::Keep => {}
        }
        text_x = art_w + pad;
    }
    if !lines.is_empty() {
        let tw = (w - text_x - pad).max(1.0);
        let th = (h - 2.0 * pad).max(1.0);
        // Largest size (up to 14 pt) at which every line fits.
        let mut size = (th / (lines.len() as f32 * 1.2)).min(14.0);
        for l in &lines {
            let lw = Font::Helvetica.width(l, size);
            if lw > tw {
                size *= tw / lw;
            }
        }
        let size = size.max(1.0);
        let lead = size * 1.2;
        let block = lead * lines.len() as f32;
        let mut y = pad + (th + block) / 2.0 - size;
        s.push_str(&format!("BT /Helv {} Tf 0 g\n", num(size)));
        for l in &lines {
            s.push_str(&format!(
                "1 0 0 1 {} {} Tm {} Tj\n",
                num(text_x),
                num(y),
                tj_string(l)
            ));
            y -= lead;
        }
        s.push_str("ET\n");
        let mut f = Dictionary::new();
        f.set("Type", name_obj("Font"));
        f.set("Subtype", name_obj("Type1"));
        f.set("BaseFont", name_obj("Helvetica"));
        f.set("Encoding", name_obj("WinAnsiEncoding"));
        let mut fd = Dictionary::new();
        fd.set("Helv", f);
        res.set("Font", fd);
    }
    let mut d = Dictionary::new();
    d.set("Type", name_obj("XObject"));
    d.set("Subtype", name_obj("Form"));
    d.set(
        "BBox",
        Object::Array(vec![0.into(), 0.into(), Object::Real(w), Object::Real(h)]),
    );
    d.set("Resources", res);
    let mut st = Stream::new(d, s.into_bytes());
    let _ = st.compress();
    Ok(st)
}

/// Validation data for the Document Security Store (PAdES B-LT).
#[derive(Clone, Debug, Default)]
pub struct Dss {
    pub certs: Vec<Vec<u8>>,
    pub ocsps: Vec<Vec<u8>>,
    pub crls: Vec<Vec<u8>>,
    /// Per-signature entries: (upper-case hex SHA-1 of the signature's
    /// `/Contents` bytes, cert indexes, OCSP indexes, CRL indexes) into
    /// the lists above.
    pub vri: Vec<Vri>,
}

#[derive(Clone, Debug, Default)]
pub struct Vri {
    pub key: String,
    pub certs: Vec<usize>,
    pub ocsps: Vec<usize>,
    pub crls: Vec<usize>,
}

/// Add (or extend) the document's DSS as an incremental update. Data
/// already in the DSS isn't added twice.
pub fn add_dss(bytes: &[u8], dss: &Dss) -> Result<Vec<u8>> {
    let pdf = Pdf::from_bytes(bytes, None)?;
    if pdf.was_encrypted() {
        return Err(Error::Invalid(
            "signing password-protected PDFs isn't supported yet".into(),
        ));
    }
    crate::sig::incremental(bytes, None, |pdf| write_dss(&mut pdf.doc, dss))
}

fn stream_bytes(doc: &Document, o: &Object) -> Option<Vec<u8>> {
    let s = doc.dereference(o).ok()?.1.as_stream().ok()?;
    s.decompressed_content()
        .ok()
        .or_else(|| Some(s.content.clone()))
}

fn write_dss(doc: &mut Document, dss: &Dss) -> Result<()> {
    let (dss_id, mut d) = match doc.catalog()?.get(b"DSS") {
        Ok(Object::Reference(id)) => (Some(*id), doc.get_dictionary(*id)?.clone()),
        Ok(Object::Dictionary(d)) => (None, d.clone()),
        _ => (None, Dictionary::new()),
    };
    // Each list: existing entries (by content) and the new refs.
    let add_list = |doc: &mut Document, d: &mut Dictionary, key: &str, items: &[Vec<u8>]| {
        let mut arr: Vec<Object> = d
            .get(key.as_bytes())
            .ok()
            .and_then(|o| doc.dereference(o).ok())
            .and_then(|(_, o)| o.as_array().ok().cloned())
            .unwrap_or_default();
        let mut have: Vec<(Vec<u8>, Object)> = arr
            .iter()
            .filter_map(|o| Some((stream_bytes(doc, o)?, o.clone())))
            .collect();
        let mut refs = Vec::new();
        for it in items {
            if let Some((_, o)) = have.iter().find(|(b, _)| b == it) {
                refs.push(o.clone());
                continue;
            }
            let mut st = Stream::new(Dictionary::new(), it.clone());
            let _ = st.compress();
            let id = doc.add_object(st);
            arr.push(id.into());
            have.push((it.clone(), id.into()));
            refs.push(id.into());
        }
        if !arr.is_empty() {
            d.set(key, Object::Array(arr));
        }
        refs
    };
    let certs = add_list(doc, &mut d, "Certs", &dss.certs);
    let ocsps = add_list(doc, &mut d, "OCSPs", &dss.ocsps);
    let crls = add_list(doc, &mut d, "CRLs", &dss.crls);
    if !dss.vri.is_empty() {
        let mut vri = d
            .get(b"VRI")
            .ok()
            .and_then(|o| doc.dereference(o).ok())
            .and_then(|(_, o)| o.as_dict().ok().cloned())
            .unwrap_or_default();
        for v in &dss.vri {
            let mut e = Dictionary::new();
            let pick = |list: &[Object], ix: &[usize]| -> Vec<Object> {
                ix.iter().filter_map(|&i| list.get(i).cloned()).collect()
            };
            for (key, list, ix) in [
                ("Cert", &certs, &v.certs),
                ("OCSP", &ocsps, &v.ocsps),
                ("CRL", &crls, &v.crls),
            ] {
                let a = pick(list, ix);
                if !a.is_empty() {
                    e.set(key, Object::Array(a));
                }
            }
            vri.set(v.key.to_uppercase().as_bytes().to_vec(), e);
        }
        d.set("VRI", vri);
    }
    d.set("Type", name_obj("DSS"));
    match dss_id {
        Some(id) => {
            *doc.get_object_mut(id)? = Object::Dictionary(d);
        }
        None => {
            let id = doc.add_object(d);
            doc.catalog_mut()?.set("DSS", id);
        }
    }
    // PAdES: declare the ESIC extension level (ISO 32000-1 extensions).
    let cat = doc.catalog_mut()?;
    let mut ext = match cat.get(b"Extensions") {
        Ok(Object::Dictionary(e)) => e.clone(),
        _ => Dictionary::new(),
    };
    if !ext.has(b"ESIC") {
        let mut esic = Dictionary::new();
        esic.set("BaseVersion", name_obj("1.7"));
        esic.set("ExtensionLevel", 5);
        ext.set("ESIC", esic);
        cat.set("Extensions", ext);
    }
    Ok(())
}

/// The `/Contents` bytes (the CMS, trailing zeros included) of the
/// signature in the last incremental update holding `field`'s value, for
/// VRI keys: the signature dictionary whose `/ByteRange` covers `len`.
pub fn signature_contents(bytes: &[u8], byte_range: [usize; 4]) -> Option<Vec<u8>> {
    let hex = bytes.get(byte_range[1] + 1..byte_range[2] - 1)?;
    hex.chunks(2)
        .map(|c| u8::from_str_radix(std::str::from_utf8(c).ok()?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_page() -> Vec<u8> {
        crate::testing::sample_pdf(1)
    }

    #[test]
    fn prepare_reserves_contents_and_byte_range() {
        let src = one_page();
        let mut req = Request::new(
            SigType::Signature { certify: Some(2) },
            Place::New {
                page: 0,
                rect: [50.0, 50.0, 250.0, 110.0],
            },
            1_790_000_000,
            1000,
        );
        req.look = Some(Look {
            lines: vec!["Digitally signed by Test".into(), "Date: 2026".into()],
            art: None,
        });
        let p = prepare(&src, &req).unwrap();
        assert!(p.bytes.starts_with(&src));
        assert_eq!(p.capacity(), 1000);
        let [a, b] = p.signed_parts();
        assert_eq!(a.len() + b.len() + 2002, p.bytes.len());
        let out = p.finish(&[0x30, 0x03, 0x02, 0x01, 0x05]).unwrap();
        let pdf = Pdf::from_bytes(&out, None).unwrap();
        let s = crate::sig::signatures(&pdf).unwrap();
        assert_eq!(s.count, 1);
        assert_eq!(s.docmdp, Some(2));
        let f = signature_fields(&pdf);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].name, "Signature1");
        assert_eq!(f[0].page, Some(0));
        assert!(f[0].signed);
        // A second certification is refused.
        assert!(prepare(&out, &req).is_err());
    }

    #[test]
    fn dss_is_added_once() {
        let src = one_page();
        let dss = Dss {
            certs: vec![b"cert".to_vec()],
            ocsps: vec![b"ocsp".to_vec()],
            crls: vec![],
            vri: vec![Vri {
                key: "ab12".into(),
                certs: vec![0],
                ocsps: vec![0],
                crls: vec![],
            }],
        };
        let once = add_dss(&src, &dss).unwrap();
        assert!(once.starts_with(&src));
        let twice = add_dss(&once, &dss).unwrap();
        let pdf = Pdf::from_bytes(&twice, None).unwrap();
        let doc = &pdf.doc;
        let d = doc
            .dereference(doc.catalog().unwrap().get(b"DSS").unwrap())
            .unwrap()
            .1
            .as_dict()
            .unwrap();
        let certs = doc
            .dereference(d.get(b"Certs").unwrap())
            .unwrap()
            .1
            .as_array()
            .unwrap();
        assert_eq!(certs.len(), 1);
        let vri = d.get(b"VRI").unwrap().as_dict().unwrap();
        assert!(vri.has(b"AB12"));
    }
}
