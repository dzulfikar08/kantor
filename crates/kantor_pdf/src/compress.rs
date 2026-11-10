//! Lossless compression: drop unused objects, merge identical streams,
//! Flate-compress uncompressed streams, and pack objects into object streams.
//! Images are never re-encoded.

use std::collections::{HashMap, HashSet};

use lopdf::{Object, ObjectId, SaveOptions, Writer};

use crate::{Pdf, Result};

/// What [`compress`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressReport {
    pub before: usize,
    pub after: usize,
    pub unused_removed: usize,
    pub duplicates_merged: usize,
    /// True when the rewritten file was not smaller, so the original bytes
    /// were returned unchanged.
    pub kept_original: bool,
}

impl CompressReport {
    /// Size saved as a percentage of the original.
    pub fn saved_percent(&self) -> f64 {
        if self.before == 0 {
            return 0.0;
        }
        (self.before as f64 - self.after as f64) * 100.0 / self.before as f64
    }
}

/// Merge streams with identical dictionaries and data. Returns how many
/// were merged away.
fn dedupe_streams(pdf: &mut Pdf) -> usize {
    let doc = &mut pdf.doc;
    let mut merged = 0;
    // Merging can make further streams identical (their dictionaries now name
    // the same objects), so repeat a few times.
    for _ in 0..4 {
        let mut seen: HashMap<(Vec<u8>, &[u8]), ObjectId> = HashMap::new();
        let mut replace: HashMap<ObjectId, ObjectId> = HashMap::new();
        for (&id, obj) in &doc.objects {
            let Object::Stream(s) = obj else { continue };
            if matches!(s.dict.get(b"Type"), Ok(Object::Name(n)) if n == b"XRef" || n == b"ObjStm")
            {
                continue;
            }
            let mut key = Vec::new();
            if Writer::write_object(&mut key, &Object::Dictionary(s.dict.clone())).is_err() {
                continue;
            }
            match seen.entry((key, s.content.as_slice())) {
                std::collections::hash_map::Entry::Occupied(e) => {
                    replace.insert(id, *e.get());
                }
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(id);
                }
            }
        }
        if replace.is_empty() {
            break;
        }
        fn walk(o: &mut Object, replace: &HashMap<ObjectId, ObjectId>) {
            match o {
                Object::Reference(id) => {
                    if let Some(to) = replace.get(id) {
                        *id = *to;
                    }
                }
                Object::Array(a) => a.iter_mut().for_each(|x| walk(x, replace)),
                Object::Dictionary(d) => d.iter_mut().for_each(|(_, x)| walk(x, replace)),
                Object::Stream(s) => s.dict.iter_mut().for_each(|(_, x)| walk(x, replace)),
                _ => {}
            }
        }
        let gone: HashSet<ObjectId> = replace.keys().copied().collect();
        doc.objects.retain(|id, _| !gone.contains(id));
        for o in doc.objects.values_mut() {
            walk(o, &replace);
        }
        for (_, o) in doc.trailer.iter_mut() {
            walk(o, &replace);
        }
        merged += replace.len();
    }
    merged
}

/// Compress a loaded PDF whose original file was `original`. Returns the
/// new bytes, or `original` itself when rewriting would not make it smaller.
pub fn compress(mut pdf: Pdf, original: &[u8]) -> Result<(Vec<u8>, CompressReport)> {
    let unused_removed = pdf.doc.prune_objects().len();
    let duplicates_merged = dedupe_streams(&mut pdf);
    pdf.doc.compress();
    pdf.doc.renumber_objects();
    let mut out = Vec::new();
    let options = SaveOptions::builder()
        .use_object_streams(true)
        .use_xref_streams(true)
        .compression_level(9)
        .build();
    pdf.doc
        .save_with_options(&mut out, options)
        .map_err(|e| crate::Error::Pdf(e.to_string()))?;
    // An encrypted source must not be handed back encrypted as if it were
    // the result; it is always rewritten.
    let kept_original = out.len() >= original.len() && !pdf.was_encrypted();
    let bytes = if kept_original {
        original.to_vec()
    } else {
        out
    };
    let report = CompressReport {
        before: original.len(),
        after: bytes.len(),
        unused_removed,
        duplicates_merged,
        kept_original,
    };
    Ok((bytes, report))
}
