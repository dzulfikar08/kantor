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

/// A usage-rights signature (Adobe Reader Extensions, `/Perms /UR3`): it
/// unlocks features in Adobe Reader and says nothing about who signed the
/// document, so it isn't counted as a signature.
fn is_usage_rights(doc: &Document, d: &lopdf::Dictionary) -> bool {
    d.get(b"Reference")
        .ok()
        .and_then(|r| doc.dereference(r).ok())
        .and_then(|(_, r)| r.as_array().ok())
        .is_some_and(|refs| {
            refs.iter().any(|r| {
                doc.dereference(r)
                    .ok()
                    .and_then(|(_, r)| r.as_dict().ok())
                    .and_then(|r| r.get(b"TransformMethod").ok())
                    .and_then(|m| m.as_name().ok())
                    .is_some_and(|m| m == b"UR3" || m == b"UR")
            })
        })
}

/// The document has Adobe Reader usage rights (`/Perms /UR3`, as the IRS
/// forms do). They stay valid only through incremental updates, so form
/// entries are saved that way; a rewritten file drops them
/// ([`drop_usage_rights`]).
pub fn usage_rights(pdf: &Pdf) -> bool {
    perms(&pdf.doc).is_some_and(|p| p.has(b"UR3") || p.has(b"UR"))
}

fn perms(doc: &Document) -> Option<&lopdf::Dictionary> {
    let p = doc.catalog().ok()?.get(b"Perms").ok()?;
    doc.dereference(p).ok()?.1.as_dict().ok()
}

/// Remove the usage rights from a document about to be written again from
/// scratch: their signature no longer matches the file, and Adobe Reader
/// would say the document was changed and turn its features off (as
/// `qpdf --remove-restrictions` does). A DocMDP entry is left alone.
pub(crate) fn drop_usage_rights(doc: &mut Document) {
    let Ok(cat) = doc.catalog() else {
        return;
    };
    let target = match cat.get(b"Perms") {
        Ok(Object::Reference(id)) => Some(*id),
        Ok(Object::Dictionary(_)) => None,
        _ => return,
    };
    let emptied = {
        let dict = match target {
            Some(id) => doc.get_object_mut(id).and_then(Object::as_dict_mut),
            None => doc
                .catalog_mut()
                .and_then(|c| c.get_mut(b"Perms"))
                .and_then(Object::as_dict_mut),
        };
        let Ok(dict) = dict else {
            return;
        };
        if !dict.has(b"UR3") && !dict.has(b"UR") {
            return;
        }
        dict.remove(b"UR3");
        dict.remove(b"UR");
        dict.is_empty()
    };
    if emptied && let Ok(c) = doc.catalog_mut() {
        c.remove(b"Perms");
    }
}

fn is_signature(doc: &Document, o: &Object) -> bool {
    let d = match o {
        Object::Dictionary(d) => d,
        _ => return false,
    };
    !is_usage_rights(doc, d)
        && d.has(b"ByteRange")
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
    incremental_diff(bytes, &before, &after)
}

/// The original `bytes` (opened as `before`) followed by an incremental
/// update holding the objects `after` changed or added.
pub fn incremental_diff(bytes: &[u8], before: &Pdf, after: &Pdf) -> Result<Vec<u8>> {
    let mut inc = IncrementalDocument::create_from(bytes.to_vec(), before.doc.clone());
    // A damaged file whose cross-reference data couldn't be read (lopdf
    // rebuilt it by scanning) has no section an update could point back
    // to: the update gets no `/Prev`, so its own table must list every
    // object, or readers that follow it find only the new ones.
    let whole = before.doc.xref_start == 0
        || before.doc.xref_recovery == Some(lopdf::xref::XrefRecovery::Reconstructed);
    let mut spelling = RealSpelling::default();
    let new = &mut inc.new_document;
    for (id, obj) in &after.doc.objects {
        if whole || before.doc.objects.get(id) != Some(obj) {
            let mut obj = obj.clone();
            if before.doc.objects.contains_key(id) {
                spelling.keep(bytes, &before.doc, *id, &mut obj);
            }
            new.objects.insert(*id, obj);
        }
    }
    new.max_id = new.max_id.max(after.doc.max_id);
    let mut out = Vec::with_capacity(bytes.len() + 4096);
    inc.save_to(&mut out)
        .map_err(|e| Error::Pdf(e.to_string()))?;
    spelling.restore(&mut out[bytes.len()..]);
    Ok(out)
}

/// lopdf keeps real numbers as `f32`, so a rewritten object loses digits
/// the file had (`/MediaBox [0 0 595.32001 841.91998]` comes back as
/// `595.32 841.92`), and checkers that compare the signed revision exactly
/// (pyHanko) call that a suspicious change. Reals of a rewritten object
/// that `f32` can't spell are swapped for placeholder names before writing
/// and replaced by the file's own text afterwards, byte for byte, so no
/// offset moves.
#[derive(Default)]
struct RealSpelling {
    /// Placeholder name (without `/`) → the file's spelling.
    names: Vec<(Vec<u8>, Vec<u8>)>,
}

impl RealSpelling {
    fn keep(&mut self, bytes: &[u8], doc: &Document, id: lopdf::ObjectId, obj: &mut Object) {
        let Some(lopdf::xref::XrefEntry::Normal { offset, .. }) = doc.reference_table.get(id.0)
        else {
            return;
        };
        let Some(raw) = bytes.get(*offset as usize..) else {
            return;
        };
        let mut found: Vec<(f32, Option<Vec<u8>>)> = Vec::new();
        for lexeme in object_reals(raw) {
            let Some(v) = std::str::from_utf8(&lexeme)
                .ok()
                .and_then(|s| s.parse::<f64>().ok())
            else {
                continue;
            };
            let f = v as f32;
            // Only spellings f32 loses; equal values spelled alike stay.
            if f.to_string().parse::<f64>().ok() == Some(v) || lexeme.len() < 6 {
                continue;
            }
            match found.iter_mut().find(|(x, _)| *x == f) {
                Some((_, l)) => {
                    if l.as_deref() != Some(&lexeme[..]) {
                        *l = None;
                    }
                }
                None => found.push((f, Some(lexeme))),
            }
        }
        if found.iter().all(|(_, l)| l.is_none()) {
            return;
        }
        self.swap(obj, &found);
    }

    fn swap(&mut self, obj: &mut Object, found: &[(f32, Option<Vec<u8>>)]) {
        match obj {
            Object::Real(v) => {
                let Some((_, Some(lexeme))) = found.iter().find(|(x, _)| x == v) else {
                    return;
                };
                if let Some(name) = self.name_for(lexeme) {
                    *obj = Object::Name(name);
                }
            }
            Object::Array(a) => a.iter_mut().for_each(|o| self.swap(o, found)),
            Object::Dictionary(d) => d.iter_mut().for_each(|(_, o)| self.swap(o, found)),
            Object::Stream(s) => s.dict.iter_mut().for_each(|(_, o)| self.swap(o, found)),
            _ => {}
        }
    }

    /// `Kr` + a zero-padded counter, as long as `lexeme`: written as
    /// `/Kr…`, which becomes a blank (lopdf writes none before a name, as it
    /// would before a number) and the lexeme.
    fn name_for(&mut self, lexeme: &[u8]) -> Option<Vec<u8>> {
        if let Some((n, _)) = self.names.iter().find(|(_, l)| l == lexeme) {
            return Some(n.clone());
        }
        let width = lexeme.len().checked_sub(2)?;
        let digits = format!("{:0width$}", self.names.len());
        if digits.len() > width {
            return None;
        }
        let name = [b"Kr".as_slice(), digits.as_bytes()].concat();
        self.names.push((name.clone(), lexeme.to_vec()));
        Some(name)
    }

    fn restore(&self, update: &mut [u8]) {
        for (name, lexeme) in &self.names {
            let needle = [b"/".as_slice(), name].concat();
            let mut i = 0;
            while let Some(p) = find(&update[i..], &needle) {
                let at = i + p;
                let end = at + needle.len();
                if update.get(end).is_none_or(|&c| !is_regular(c)) {
                    update[at] = b' ';
                    update[at + 1..end].copy_from_slice(lexeme);
                }
                i = end;
            }
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn is_regular(c: u8) -> bool {
    !c.is_ascii_whitespace() && c != 0 && !b"()<>[]{}/%".contains(&c)
}

/// The spellings of the real numbers (with a `.`) of the indirect object
/// at the start of `raw`, up to its `stream` or `endobj` keyword. Strings
/// and comments are skipped.
fn object_reals(raw: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let c = raw[i];
        match c {
            b'%' => {
                while i < raw.len() && raw[i] != b'\n' && raw[i] != b'\r' {
                    i += 1;
                }
            }
            b'(' => {
                let mut depth = 0usize;
                while i < raw.len() {
                    match raw[i] {
                        b'\\' => i += 1,
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                i += 1;
            }
            b'<' if raw.get(i + 1) == Some(&b'<') => i += 2,
            b'<' => {
                while i < raw.len() && raw[i] != b'>' {
                    i += 1;
                }
                i += 1;
            }
            _ if is_regular(c) => {
                let start = i;
                while i < raw.len() && is_regular(raw[i]) {
                    i += 1;
                }
                let tok = &raw[start..i];
                if tok == b"stream" || tok == b"endobj" {
                    break;
                }
                let digits = tok.strip_prefix(b"-").or(tok.strip_prefix(b"+")).unwrap_or(tok);
                if digits.contains(&b'.')
                    && !digits.is_empty()
                    && digits.iter().all(|&d| d.is_ascii_digit() || d == b'.')
                    && digits.iter().filter(|&&d| d == b'.').count() == 1
                {
                    out.push(tok.to_vec());
                }
            }
            _ => i += 1,
        }
    }
    out
}
