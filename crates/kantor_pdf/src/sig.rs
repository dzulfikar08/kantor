//! Digitally signed PDFs: finding signatures and their DocMDP permissions,
//! and saving changes as an *incremental update* (ISO 32000-1 §7.5.6) so
//! the signed bytes stay an exact prefix of the file and the signatures
//! stay valid.
//!
//! Kantor doesn't validate signatures; it only keeps them intact.

use lopdf::{Document, IncrementalDocument, Object};

use crate::{Error, Pdf, Result};

/// What a signed document allows (DocMDP, ISO 32000-1 §12.8.2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Allowed {
    /// No changes at all (P = 1).
    Nothing,
    /// Filling in forms and signing (P = 2).
    FormFill,
    /// Also annotations (P = 3, or no certification).
    Annotations,
}

/// The signatures of a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signatures {
    /// Signed signature fields (signature dictionaries with a /ByteRange).
    pub count: usize,
    /// The certification's DocMDP permission (1, 2 or 3), if certified.
    pub docmdp: Option<u8>,
}

impl Signatures {
    pub fn allowed(&self) -> Allowed {
        match self.docmdp {
            Some(1) => Allowed::Nothing,
            Some(2) => Allowed::FormFill,
            _ => Allowed::Annotations,
        }
    }

    pub fn allows_annotations(&self) -> bool {
        self.allowed() == Allowed::Annotations
    }

    pub fn allows_form_fill(&self) -> bool {
        self.allowed() != Allowed::Nothing
    }
}

fn is_signature(doc: &Document, o: &Object) -> bool {
    let d = match o {
        Object::Dictionary(d) => d,
        _ => return false,
    };
    d.has(b"ByteRange")
        && d.has(b"Contents")
        && d.get(b"Type")
            .ok()
            .and_then(|t| doc.dereference(t).ok())
            .and_then(|(_, t)| t.as_name().ok())
            .is_none_or(|t| t == b"Sig" || t == b"DocTimeStamp")
}

/// The document's signatures, or `None` when it has no signed field.
pub fn signatures(pdf: &Pdf) -> Option<Signatures> {
    let doc = &pdf.doc;
    let count = doc
        .objects
        .values()
        .filter(|o| is_signature(doc, o))
        .count();
    if count == 0 {
        return None;
    }
    let docmdp = (|| {
        let perms = doc.catalog().ok()?.get(b"Perms").ok()?;
        let perms = doc.dereference(perms).ok()?.1.as_dict().ok()?;
        let sig = doc
            .dereference(perms.get(b"DocMDP").ok()?)
            .ok()?
            .1
            .as_dict()
            .ok()?;
        let refs = doc
            .dereference(sig.get(b"Reference").ok()?)
            .ok()?
            .1
            .as_array()
            .ok()?;
        for r in refs {
            let r = doc.dereference(r).ok()?.1.as_dict().ok()?;
            if r.get(b"TransformMethod").and_then(Object::as_name).ok() != Some(b"DocMDP") {
                continue;
            }
            let p = r
                .get(b"TransformParams")
                .ok()
                .and_then(|t| doc.dereference(t).ok())
                .and_then(|(_, t)| t.as_dict().ok())
                .and_then(|t| t.get(b"P").ok())
                .and_then(|p| p.as_i64().ok())
                .unwrap_or(2);
            return Some(p.clamp(1, 3) as u8);
        }
        Some(2)
    })();
    Some(Signatures { count, docmdp })
}

/// Apply `edit` to the PDF in `bytes` (opened with `password`) and return
/// the original bytes followed by an incremental update holding only the
/// objects `edit` changed or added. Existing objects keep their numbers;
/// nothing is renumbered, removed or compressed again.
pub fn incremental(
    bytes: &[u8],
    password: Option<&str>,
    edit: impl FnOnce(&mut Pdf) -> Result<()>,
) -> Result<Vec<u8>> {
    let before = match Pdf::from_bytes(bytes, None) {
        Err(Error::Encrypted) => Pdf::from_bytes(bytes, Some(password.unwrap_or(""))),
        r => r,
    }?;
    let mut after = before.clone();
    edit(&mut after)?;
    let mut inc = IncrementalDocument::create_from(bytes.to_vec(), before.doc.clone());
    let new = &mut inc.new_document;
    for (id, obj) in &after.doc.objects {
        if before.doc.objects.get(id) != Some(obj) {
            new.objects.insert(*id, obj.clone());
        }
    }
    new.max_id = new.max_id.max(after.doc.max_id);
    let mut out = Vec::with_capacity(bytes.len() + 4096);
    inc.save_to(&mut out)
        .map_err(|e| Error::Pdf(e.to_string()))?;
    Ok(out)
}
