//! What a provider's answer did to the signatures already in the document.
//!
//! A provider that stamps or signs as an incremental update leaves the
//! original bytes as an exact prefix of its result: earlier signatures stay
//! valid. One that rebuilds the file breaks them. Kantor warns before
//! sending a signed document to a provider known to rebuild files, and
//! checks every result here before saving it.

use pdf_engine::signatures::{self as sigs, Status};

/// Signed signatures in `pdf` (0 when unsigned or unreadable).
pub fn signature_count(pdf: &[u8]) -> usize {
    kantor_pdf::Pdf::from_bytes(pdf, None)
        .ok()
        .and_then(|p| kantor_pdf::sig::signatures(&p))
        .map_or(0, |s| s.count)
}

/// The verdict on a provider's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kept {
    /// The original had no signatures.
    Unsigned,
    /// The original is an exact prefix of the result: every earlier
    /// signature still covers the same bytes.
    Incremental,
    /// The file was rebuilt; these earlier signatures no longer verify
    /// (field names).
    Broken(Vec<String>),
}

/// Compare the document sent (`before`) with the provider's `after`.
pub fn kept(before: &[u8], after: &[u8]) -> Kept {
    let n = signature_count(before);
    if n == 0 {
        return Kept::Unsigned;
    }
    if after.len() >= before.len() && after[..before.len()] == *before {
        return Kept::Incremental;
    }
    // Rebuilt: see which of the original's signatures survived intact.
    let opts = sigs::Options::standard();
    let old = sigs::verify(before, &opts);
    let new = sigs::verify(after, &opts);
    let broken = old
        .signatures
        .iter()
        .filter(|o| {
            !new.signatures.iter().any(|s| {
                s.field == o.field
                    && s.status != Status::Invalid
                    && s.integrity == sigs::Integrity::Intact
            })
        })
        .map(|o| o.field.clone())
        .collect::<Vec<_>>();
    if broken.is_empty() {
        Kept::Incremental
    } else {
        Kept::Broken(broken)
    }
}

/// The result is a PDF at all (providers sometimes answer with an error
/// page and status 200).
pub fn looks_like_pdf(bytes: &[u8]) -> bool {
    bytes.len() > 8
        && bytes[..1024.min(bytes.len())]
            .windows(5)
            .any(|w| w == b"%PDF-")
}
