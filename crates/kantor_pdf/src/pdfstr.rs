//! Small helpers shared by annotations and forms: PDF text strings, dates,
//! numbers in content streams, and copying objects between documents.

use std::collections::HashMap;

use lopdf::{Dictionary, Document, Object, ObjectId, StringFormat};

/// A PDF text string: PDFDocEncoding (Latin-1 here) when every character
/// fits, else UTF-16BE with a byte order mark.
pub(crate) fn text(s: &str) -> Object {
    if s.chars().all(|c| (c as u32) < 0x80) {
        Object::String(s.as_bytes().to_vec(), StringFormat::Literal)
    } else {
        let mut b = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            b.extend_from_slice(&u.to_be_bytes());
        }
        Object::String(b, StringFormat::Hexadecimal)
    }
}

/// Decode a PDF text string (UTF-16BE with BOM, UTF-8 with BOM, or
/// PDFDocEncoding read as Latin-1).
pub(crate) fn decode(b: &[u8]) -> String {
    if b.starts_with(&[0xFE, 0xFF]) {
        let u: Vec<u16> = b[2..]
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&u);
    }
    if let Some(rest) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    b.iter().map(|&c| c as char).collect()
}

/// A string entry of a dictionary, decoded (following a reference).
pub(crate) fn get_text(doc: &Document, d: &Dictionary, key: &[u8]) -> Option<String> {
    let o = d.get(key).ok()?;
    match doc.dereference(o).ok()?.1 {
        Object::String(b, _) => Some(decode(b)),
        Object::Stream(s) => Some(decode(&s.decompressed_content().ok()?)),
        _ => None,
    }
}

/// A number for a content stream: no exponent, few decimals.
pub(crate) fn num(v: f32) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let s = format!("{:.3}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_owned() }
}

/// The time now as a PDF date (UTC): `D:20261119103000Z`.
pub fn pdf_date_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    pdf_date(secs)
}

/// Seconds since 1970 as a PDF date (UTC).
pub fn pdf_date(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (y, m, d) = civil(days);
    format!(
        "D:{y:04}{m:02}{d:02}{:02}{:02}{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// Year, month, day of a day count since 1970-01-01 (Howard Hinnant's
/// algorithm).
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A PDF date shown as `2026-11-19 10:30` (its own time zone, as written).
pub fn show_date(date: &str) -> String {
    let d: String = date
        .trim_start_matches("D:")
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if d.len() < 8 {
        return String::new();
    }
    let mut out = format!("{}-{}-{}", &d[0..4], &d[4..6], &d[6..8]);
    if d.len() >= 12 {
        out.push_str(&format!(" {}:{}", &d[8..10], &d[10..12]));
    }
    out
}

/// Keys never followed when copying an annotation into another document:
/// they lead back into the page tree, the form or other annotations.
const NOT_FOLLOWED: &[&[u8]] = &[
    b"P",
    b"Parent",
    b"Popup",
    b"IRT",
    b"A",
    b"AA",
    b"StructParent",
    b"StructParents",
    b"Kids",
    b"Dest",
];

/// Copy `obj` from `src` into `dst` with everything it refers to (except
/// [`NOT_FOLLOWED`] keys); `map` remembers objects already copied.
pub(crate) fn copy_object(
    src: &Document,
    dst: &mut Document,
    obj: &Object,
    map: &mut HashMap<ObjectId, ObjectId>,
) -> Object {
    match obj {
        Object::Reference(id) => {
            if let Some(n) = map.get(id) {
                return Object::Reference(*n);
            }
            let Ok(target) = src.get_object(*id) else {
                return Object::Null;
            };
            let new_id = dst.new_object_id();
            map.insert(*id, new_id);
            let copied = copy_object(src, dst, &target.clone(), map);
            dst.objects.insert(new_id, copied);
            Object::Reference(new_id)
        }
        Object::Array(a) => {
            Object::Array(a.iter().map(|o| copy_object(src, dst, o, map)).collect())
        }
        Object::Dictionary(d) => Object::Dictionary(copy_dict(src, dst, d, map)),
        Object::Stream(s) => {
            let mut s = s.clone();
            s.dict = copy_dict(src, dst, &s.dict, map);
            Object::Stream(s)
        }
        o => o.clone(),
    }
}

fn copy_dict(
    src: &Document,
    dst: &mut Document,
    d: &Dictionary,
    map: &mut HashMap<ObjectId, ObjectId>,
) -> Dictionary {
    let mut out = Dictionary::new();
    for (k, v) in d.iter() {
        if NOT_FOLLOWED.contains(&k.as_slice()) {
            continue;
        }
        out.set(k.clone(), copy_object(src, dst, v, map));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn strings_dates_numbers() {
        let o = text("Résumé ✓");
        let Object::String(b, _) = &o else { panic!() };
        assert_eq!(decode(b), "Résumé ✓");
        assert_eq!(decode(b"plain"), "plain");
        assert_eq!(num(1.0), "1");
        assert_eq!(num(-0.0001), "0");
        assert_eq!(num(2.5), "2.5");
        assert_eq!(pdf_date(0), "D:19700101000000Z");
        assert_eq!(pdf_date(1_795_084_200), "D:20261119103000Z");
        assert_eq!(show_date("D:20261119103000Z"), "2026-11-19 10:30");
    }
}
