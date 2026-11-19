//! Reading signature fields, signature dictionaries and the DSS from the
//! PDF's object structure (lopdf).

use lopdf::{Dictionary, Document, Object, ObjectId};

/// A signature field found in the AcroForm.
#[derive(Clone, Debug)]
pub struct Field {
    /// Fully qualified field name ("Signature1", "Form.Sig").
    pub name: String,
    /// 1-based page of the field's first widget.
    pub page: Option<usize>,
    /// The signature dictionary (the field's /V), when signed.
    pub sig: Option<SigDict>,
}

/// A signature dictionary's entries.
#[derive(Clone, Debug, Default)]
pub struct SigDict {
    pub filter: String,
    pub sub_filter: String,
    pub is_doc_timestamp: bool,
    pub contents: Vec<u8>,
    pub byte_range: Vec<i64>,
    pub name: Option<String>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub contact: Option<String>,
    /// /M as Unix seconds.
    pub time: Option<i64>,
    /// The DocMDP permission level, for a certification signature.
    pub docmdp: Option<u8>,
    /// FieldMDP locks: (action, field names).
    pub field_locks: Vec<(String, Vec<String>)>,
}

pub fn deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    doc.dereference(o).map(|(_, o)| o).unwrap_or(o)
}

pub fn dict_get<'a>(doc: &'a Document, d: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    d.get(key).ok().map(|o| deref(doc, o))
}

pub fn as_dict<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Dictionary> {
    match deref(doc, o) {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&s.dict),
        _ => None,
    }
}

pub fn name_of(o: Option<&Object>) -> Option<String> {
    match o {
        Some(Object::Name(n)) => Some(String::from_utf8_lossy(n).into_owned()),
        _ => None,
    }
}

/// A PDF text string: UTF-16 with a BOM, UTF-8 with a BOM, or
/// PDFDocEncoding (close enough to Latin-1 for names and reasons).
pub fn text_string(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xfe, 0xff]) {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes(*c))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xff, 0xfe]) {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    bytes.iter().map(|&b| pdfdoc_char(b)).collect()
}

fn pdfdoc_char(b: u8) -> char {
    match b {
        0x80 => '•',
        0x81 => '†',
        0x82 => '‡',
        0x83 => '…',
        0x84 => '—',
        0x85 => '–',
        0x8d => '“',
        0x8e => '”',
        0x8f => '‘',
        0x90 => '’',
        0x91 => '‚',
        0x92 => '™',
        0xa0 => '€',
        _ => b as char,
    }
}

fn text_of(doc: &Document, d: &Dictionary, key: &[u8]) -> Option<String> {
    match dict_get(doc, d, key)? {
        Object::String(s, _) => Some(text_string(s)).filter(|s| !s.trim().is_empty()),
        _ => None,
    }
}

/// A PDF date (`D:20261119102300+07'00'`) as Unix seconds.
pub fn pdf_date(s: &str) -> Option<i64> {
    let s = s.strip_prefix("D:").unwrap_or(s);
    let num = |a: usize, n: usize, def: u32| -> Option<u32> {
        match s.get(a..a + n) {
            Some(p) if p.bytes().all(|b| b.is_ascii_digit()) => p.parse().ok(),
            Some(_) => None,
            None => Some(def),
        }
    };
    let year = i64::from(num(0, 4, 0)?);
    if year == 0 {
        return None;
    }
    let (mon, day) = (num(4, 2, 1)?, num(6, 2, 1)?);
    let (h, mi, sec) = (num(8, 2, 0)?, num(10, 2, 0)?, num(12, 2, 0)?);
    let mut offset = 0i64;
    if let Some(sign) = s.as_bytes().get(14) {
        let rest: String = s[15..].chars().filter(char::is_ascii_digit).collect();
        let oh: i64 = rest.get(0..2).and_then(|p| p.parse().ok()).unwrap_or(0);
        let om: i64 = rest.get(2..4).and_then(|p| p.parse().ok()).unwrap_or(0);
        offset = match sign {
            b'+' => oh * 3600 + om * 60,
            b'-' => -(oh * 3600 + om * 60),
            _ => 0,
        };
    }
    if !(1..=12).contains(&mon) || !(1..=31).contains(&day) {
        return None;
    }
    Some(
        super::der::days_from_civil(year, mon, day) * 86_400
            + i64::from(h) * 3600
            + i64::from(mi) * 60
            + i64::from(sec)
            - offset,
    )
}

/// Every signature field, in AcroForm order.
pub fn fields(doc: &Document) -> Vec<Field> {
    let mut out = Vec::new();
    let Ok(catalog) = doc.catalog() else {
        return out;
    };
    let Some(acro) = dict_get(doc, catalog, b"AcroForm").and_then(|o| as_dict(doc, o)) else {
        return out;
    };
    let pages = page_index(doc);
    let Some(Object::Array(list)) = dict_get(doc, acro, b"Fields") else {
        return out;
    };
    let mut seen = std::collections::HashSet::new();
    for f in list {
        walk_field(doc, f, None, None, &pages, &mut seen, &mut out);
    }
    out
}

/// Page object ids → 1-based page numbers.
pub fn page_index(doc: &Document) -> std::collections::HashMap<ObjectId, usize> {
    doc.get_pages()
        .into_iter()
        .map(|(n, id)| (id, n as usize))
        .collect()
}

fn walk_field(
    doc: &Document,
    obj: &Object,
    parent_name: Option<&str>,
    inherited_ft: Option<&str>,
    pages: &std::collections::HashMap<ObjectId, usize>,
    seen: &mut std::collections::HashSet<ObjectId>,
    out: &mut Vec<Field>,
) {
    if let Object::Reference(id) = obj
        && !seen.insert(*id)
    {
        return;
    }
    let Some(d) = as_dict(doc, obj) else { return };
    let partial = text_of(doc, d, b"T");
    let name = match (parent_name, partial) {
        (Some(p), Some(t)) => Some(format!("{p}.{t}")),
        (None, Some(t)) => Some(t),
        (p, None) => p.map(str::to_string),
    };
    let ft = name_of(dict_get(doc, d, b"FT")).or(inherited_ft.map(str::to_string));
    let kids = match dict_get(doc, d, b"Kids") {
        Some(Object::Array(k)) => k.clone(),
        _ => Vec::new(),
    };
    // Kids that are fields (have /T) make this a non-terminal field.
    let field_kids: Vec<&Object> = kids
        .iter()
        .filter(|k| as_dict(doc, k).is_some_and(|kd| kd.has(b"T")))
        .collect();
    if !field_kids.is_empty() {
        for k in field_kids {
            walk_field(doc, k, name.as_deref(), ft.as_deref(), pages, seen, out);
        }
        return;
    }
    if ft.as_deref() != Some("Sig") {
        return;
    }
    // The page: the widget's /P, or the page whose /Annots holds it.
    let widgets: Vec<&Object> = if kids.is_empty() {
        vec![obj]
    } else {
        kids.iter().collect()
    };
    let mut page = None;
    for w in &widgets {
        if let Some(wd) = as_dict(doc, w)
            && let Ok(Object::Reference(p)) = wd.get(b"P")
            && let Some(n) = pages.get(p)
        {
            page = Some(*n);
            break;
        }
        if let Object::Reference(wid) = w {
            page = page_holding(doc, *wid, pages);
            if page.is_some() {
                break;
            }
        }
    }
    let sig = dict_get(doc, d, b"V")
        .and_then(|v| as_dict(doc, v))
        .map(|v| sig_dict(doc, v));
    out.push(Field {
        name: name.unwrap_or_else(|| "Signature".into()),
        page,
        sig,
    });
}

fn page_holding(
    doc: &Document,
    annot: ObjectId,
    pages: &std::collections::HashMap<ObjectId, usize>,
) -> Option<usize> {
    for (pid, n) in pages {
        let Ok(Object::Dictionary(pd)) = doc.get_object(*pid) else {
            continue;
        };
        if let Some(Object::Array(a)) = dict_get(doc, pd, b"Annots")
            && a.iter().any(|o| o.as_reference().ok() == Some(annot))
        {
            return Some(*n);
        }
    }
    None
}

pub fn sig_dict(doc: &Document, d: &Dictionary) -> SigDict {
    let contents = match dict_get(doc, d, b"Contents") {
        Some(Object::String(s, _)) => s.clone(),
        _ => Vec::new(),
    };
    let byte_range = match dict_get(doc, d, b"ByteRange") {
        Some(Object::Array(a)) => a
            .iter()
            .filter_map(|o| match deref(doc, o) {
                Object::Integer(i) => Some(*i),
                Object::Real(r) => Some(*r as i64),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let mut docmdp = None;
    let mut field_locks = Vec::new();
    if let Some(Object::Array(refs)) = dict_get(doc, d, b"Reference") {
        for r in refs {
            let Some(rd) = as_dict(doc, r) else { continue };
            let method = name_of(dict_get(doc, rd, b"TransformMethod"));
            let params = dict_get(doc, rd, b"TransformParams").and_then(|p| as_dict(doc, p));
            match method.as_deref() {
                Some("DocMDP") => {
                    let p = params
                        .and_then(|p| dict_get(doc, p, b"P"))
                        .and_then(|o| o.as_i64().ok())
                        .unwrap_or(2);
                    docmdp = Some(p.clamp(1, 3) as u8);
                }
                Some("FieldMDP") => {
                    if let Some(p) = params {
                        let action = name_of(dict_get(doc, p, b"Action")).unwrap_or_default();
                        let names = match dict_get(doc, p, b"Fields") {
                            Some(Object::Array(a)) => a
                                .iter()
                                .filter_map(|o| match deref(doc, o) {
                                    Object::String(s, _) => Some(text_string(s)),
                                    _ => None,
                                })
                                .collect(),
                            _ => Vec::new(),
                        };
                        field_locks.push((action, names));
                    }
                }
                _ => {}
            }
        }
    }
    let type_name = name_of(dict_get(doc, d, b"Type"));
    let sub_filter = name_of(dict_get(doc, d, b"SubFilter")).unwrap_or_default();
    SigDict {
        filter: name_of(dict_get(doc, d, b"Filter")).unwrap_or_default(),
        is_doc_timestamp: type_name.as_deref() == Some("DocTimeStamp")
            || sub_filter == "ETSI.RFC3161",
        sub_filter,
        contents,
        byte_range,
        name: text_of(doc, d, b"Name"),
        reason: text_of(doc, d, b"Reason"),
        location: text_of(doc, d, b"Location"),
        contact: text_of(doc, d, b"ContactInfo"),
        time: text_of(doc, d, b"M").and_then(|m| pdf_date(&m)),
        docmdp,
        field_locks,
    }
}

/// The DocMDP level in the catalog's /Perms (for files whose certification
/// signature dictionary lacks /Reference).
pub fn catalog_docmdp(doc: &Document) -> Option<u8> {
    let catalog = doc.catalog().ok()?;
    let perms = dict_get(doc, catalog, b"Perms").and_then(|p| as_dict(doc, p))?;
    let sig = dict_get(doc, perms, b"DocMDP").and_then(|s| as_dict(doc, s))?;
    sig_dict(doc, sig).docmdp.or(Some(2))
}

/// The Document Security Store: certificates, OCSP responses and CRLs
/// added for long-term validation.
#[derive(Clone, Debug, Default)]
pub struct Dss {
    pub certs: Vec<Vec<u8>>,
    pub ocsps: Vec<Vec<u8>>,
    pub crls: Vec<Vec<u8>>,
}

pub fn dss(doc: &Document) -> Dss {
    let mut out = Dss::default();
    let Ok(catalog) = doc.catalog() else {
        return out;
    };
    let Some(dss) = dict_get(doc, catalog, b"DSS").and_then(|o| as_dict(doc, o)) else {
        return out;
    };
    let streams = |key: &[u8]| -> Vec<Vec<u8>> {
        let mut v = Vec::new();
        let mut push = |o: &Object| {
            if let Object::Stream(s) = deref(doc, o) {
                v.push(
                    s.decompressed_content()
                        .unwrap_or_else(|_| s.content.clone()),
                );
            }
        };
        if let Some(Object::Array(a)) = dict_get(doc, dss, key) {
            a.iter().for_each(&mut push);
        }
        // Per-signature VRI entries can hold their own.
        if let Some(vri) = dict_get(doc, dss, b"VRI").and_then(|o| as_dict(doc, o)) {
            for (_, e) in vri.iter() {
                let Some(ed) = as_dict(doc, e) else { continue };
                let k = match key {
                    b"Certs" => &b"Cert"[..],
                    b"OCSPs" => b"OCSP",
                    _ => b"CRL",
                };
                if let Some(Object::Array(a)) = dict_get(doc, ed, k) {
                    a.iter().for_each(&mut push);
                }
            }
        }
        v
    };
    out.certs = streams(b"Certs");
    out.ocsps = streams(b"OCSPs");
    out.crls = streams(b"CRLs");
    out
}

/// Where each revision of the file ends (after `%%EOF` and its line end).
pub fn revision_ends(bytes: &[u8]) -> Vec<usize> {
    let mut ends = Vec::new();
    let pat = b"%%EOF";
    let mut i = 0;
    while i + pat.len() <= bytes.len() {
        if &bytes[i..i + pat.len()] == pat {
            let mut e = i + pat.len();
            if bytes.get(e) == Some(&b'\r') {
                e += 1;
            }
            if bytes.get(e) == Some(&b'\n') {
                e += 1;
            }
            ends.push(e);
            i = e;
        } else {
            i += 1;
        }
    }
    ends
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_dates() {
        let utc = pdf_date("D:20261119030000Z").unwrap();
        assert_eq!(pdf_date("D:20261119100000+07'00'"), Some(utc));
        assert_eq!(pdf_date("D:20261119100000+07'00"), Some(utc));
        assert_eq!(pdf_date("D:2026"), pdf_date("D:20260101000000Z"));
    }

    #[test]
    fn text_strings() {
        assert_eq!(text_string(b"\xfe\xff\x00B\x00u"), "Bu");
        assert_eq!(text_string(b"Jakarta"), "Jakarta");
    }
}
