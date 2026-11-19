//! A small, lenient ASN.1 reader (DER, plus the BER indefinite lengths some
//! older PDF signers still write) and the few encoders the checker needs.
//!
//! Signatures are read with this rather than strict DER decoders: a signer
//! that wrote a slightly off certificate should still get a readable report.

use std::fmt;

pub const BOOLEAN: u8 = 0x01;
pub const INTEGER: u8 = 0x02;
pub const BIT_STRING: u8 = 0x03;
pub const OCTET_STRING: u8 = 0x04;
pub const NULL: u8 = 0x05;
pub const OID: u8 = 0x06;
pub const ENUMERATED: u8 = 0x0a;
pub const UTC_TIME: u8 = 0x17;
pub const GENERALIZED_TIME: u8 = 0x18;
pub const SEQUENCE: u8 = 0x30;
pub const SET: u8 = 0x31;

/// Context-specific tag `[n]`, constructed.
pub const fn ctx(n: u8) -> u8 {
    0xa0 | n
}

/// Context-specific tag `[n]`, primitive.
pub const fn ctx_prim(n: u8) -> u8 {
    0x80 | n
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub &'static str);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "malformed ASN.1: {}", self.0)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// One element: its tag byte, the whole encoding and the contents.
#[derive(Clone, Copy, Debug)]
pub struct Tlv<'a> {
    pub tag: u8,
    pub raw: &'a [u8],
    pub body: &'a [u8],
}

impl<'a> Tlv<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Tlv<'a>> {
        Reader::new(data).read()
    }

    /// The children of a constructed element.
    pub fn reader(&self) -> Reader<'a> {
        Reader::new(self.body)
    }

    pub fn expect(self, tag: u8) -> Result<Self> {
        if self.tag == tag {
            Ok(self)
        } else {
            Err(Error("unexpected tag"))
        }
    }

    pub fn oid(&self) -> Result<String> {
        if self.tag != OID {
            return Err(Error("expected an OID"));
        }
        oid_string(self.body)
    }

    /// The bytes of an OCTET STRING, joining the pieces of a BER
    /// constructed one. Works on implicitly tagged ones too.
    pub fn octets(&self) -> Result<Vec<u8>> {
        if self.tag & 0x20 == 0 {
            return Ok(self.body.to_vec());
        }
        let mut out = Vec::new();
        let mut r = self.reader();
        while !r.is_empty() {
            out.extend(r.read()?.octets()?);
        }
        Ok(out)
    }

    /// A BIT STRING's bytes (the unused-bits count dropped).
    pub fn bits(&self) -> Result<&'a [u8]> {
        match self.body.split_first() {
            Some((_, rest)) => Ok(rest),
            None => Err(Error("empty bit string")),
        }
    }

    /// A small non-negative INTEGER or ENUMERATED.
    pub fn small_int(&self) -> Result<u64> {
        let b = self.body;
        if b.is_empty() || b.len() > 9 || b[0] & 0x80 != 0 {
            return Err(Error("integer out of range"));
        }
        Ok(b.iter().fold(0u64, |a, &x| (a << 8) | u64::from(x)))
    }

    /// An INTEGER's magnitude bytes with leading zeros removed (serial
    /// numbers compare this way).
    pub fn uint_bytes(&self) -> &'a [u8] {
        let mut b = self.body;
        while b.len() > 1 && b[0] == 0 {
            b = &b[1..];
        }
        b
    }

    /// UTCTime or GeneralizedTime as Unix seconds.
    pub fn time(&self) -> Result<i64> {
        let s = std::str::from_utf8(self.body).map_err(|_| Error("bad time"))?;
        parse_time(s, self.tag == UTC_TIME).ok_or(Error("bad time"))
    }

    /// A directory string or similar as text.
    pub fn text(&self) -> String {
        decode_string(self.tag, self.body)
    }
}

/// Reads elements one after another.
#[derive(Clone, Debug)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.pos >= self.data.len()
    }

    pub fn peek_tag(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    pub fn read(&mut self) -> Result<Tlv<'a>> {
        let (tlv, used) = parse_one(&self.data[self.pos..])?;
        self.pos += used;
        Ok(tlv)
    }

    pub fn read_tag(&mut self, tag: u8) -> Result<Tlv<'a>> {
        self.read()?.expect(tag)
    }

    /// The next element if it has `tag`.
    pub fn read_opt(&mut self, tag: u8) -> Result<Option<Tlv<'a>>> {
        if self.peek_tag() == Some(tag) {
            self.read().map(Some)
        } else {
            Ok(None)
        }
    }

    /// All remaining elements.
    pub fn all(mut self) -> Result<Vec<Tlv<'a>>> {
        let mut out = Vec::new();
        while !self.is_empty() {
            out.push(self.read()?);
        }
        Ok(out)
    }
}

fn parse_one(data: &[u8]) -> Result<(Tlv<'_>, usize)> {
    let tag = *data.first().ok_or(Error("truncated"))?;
    if tag & 0x1f == 0x1f {
        return Err(Error("high tag numbers are not supported"));
    }
    let len0 = *data.get(1).ok_or(Error("truncated"))?;
    let mut pos = 2;
    if len0 == 0x80 {
        // Indefinite length: children until an end-of-contents marker.
        if tag & 0x20 == 0 {
            return Err(Error("indefinite primitive"));
        }
        let start = pos;
        loop {
            if data.get(pos..pos + 2) == Some(&[0, 0]) {
                let tlv = Tlv {
                    tag,
                    raw: &data[..pos + 2],
                    body: &data[start..pos],
                };
                return Ok((tlv, pos + 2));
            }
            let (_, used) = parse_one(data.get(pos..).ok_or(Error("truncated"))?)?;
            pos += used;
        }
    }
    let len = if len0 & 0x80 == 0 {
        usize::from(len0)
    } else {
        let n = usize::from(len0 & 0x7f);
        if n > 4 {
            return Err(Error("length too long"));
        }
        let bytes = data.get(2..2 + n).ok_or(Error("truncated"))?;
        pos += n;
        bytes.iter().fold(0usize, |a, &b| (a << 8) | usize::from(b))
    };
    let end = pos.checked_add(len).ok_or(Error("length overflow"))?;
    let body = data.get(pos..end).ok_or(Error("truncated"))?;
    Ok((
        Tlv {
            tag,
            raw: &data[..end],
            body,
        },
        end,
    ))
}

pub fn oid_string(body: &[u8]) -> Result<String> {
    let mut parts: Vec<u64> = Vec::new();
    let mut v: u64 = 0;
    for &b in body {
        v = (v << 7) | u64::from(b & 0x7f);
        if b & 0x80 == 0 {
            if parts.is_empty() {
                let first = (v / 40).min(2);
                parts.push(first);
                parts.push(v - first * 40);
            } else {
                parts.push(v);
            }
            v = 0;
        }
    }
    if parts.is_empty() {
        return Err(Error("empty OID"));
    }
    Ok(parts
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join("."))
}

/// Days from 1970-01-01 to a civil date (proleptic Gregorian).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date and time of Unix seconds (UTC).
pub fn civil(t: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (
        y,
        m,
        d,
        (secs / 3600) as u32,
        (secs / 60 % 60) as u32,
        (secs % 60) as u32,
    )
}

fn parse_time(s: &str, utc_time: bool) -> Option<i64> {
    let digits = |s: &str, from: usize, n: usize| -> Option<u32> {
        let p = s.get(from..from + n)?;
        if p.bytes().all(|b| b.is_ascii_digit()) {
            p.parse().ok()
        } else {
            None
        }
    };
    let (year, rest) = if utc_time {
        let y = i64::from(digits(s, 0, 2)?);
        (if y >= 50 { 1900 + y } else { 2000 + y }, 2)
    } else {
        (i64::from(digits(s, 0, 4)?), 4)
    };
    let mon = digits(s, rest, 2)?;
    let day = digits(s, rest + 2, 2)?;
    let hour = digits(s, rest + 4, 2)?;
    let min = digits(s, rest + 6, 2)?;
    let mut i = rest + 8;
    let sec = match digits(s, i, 2) {
        Some(v) => {
            i += 2;
            v
        }
        None => 0,
    };
    // Fractions of a second are dropped.
    if s.as_bytes().get(i) == Some(&b'.') || s.as_bytes().get(i) == Some(&b',') {
        i += 1;
        while s.as_bytes().get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
    }
    let mut offset = 0i64;
    match s.as_bytes().get(i) {
        Some(b'Z') | None => {}
        Some(&c @ (b'+' | b'-')) => {
            let oh = i64::from(digits(s, i + 1, 2)?);
            let om = i64::from(digits(s, i + 3, 2).unwrap_or(0));
            offset = (oh * 3600 + om * 60) * if c == b'+' { 1 } else { -1 };
        }
        _ => return None,
    }
    if !(1..=12).contains(&mon) || !(1..=31).contains(&day) || hour > 23 || min > 59 || sec > 60 {
        return None;
    }
    let days = days_from_civil(year, mon, day);
    Some(
        days * 86_400 + i64::from(hour) * 3600 + i64::from(min) * 60 + i64::from(sec.min(59))
            - offset,
    )
}

fn decode_string(tag: u8, body: &[u8]) -> String {
    match tag {
        // BMPString: UTF-16BE.
        0x1e => {
            let units: Vec<u16> = body
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes(*c))
                .collect();
            String::from_utf16_lossy(&units)
        }
        // UniversalString: UTF-32BE.
        0x1c => body
            .as_chunks::<4>()
            .0
            .iter()
            .filter_map(|c| char::from_u32(u32::from_be_bytes(*c)))
            .collect(),
        // T61String: usually Latin-1 in practice.
        0x14 if std::str::from_utf8(body).is_err() => body.iter().map(|&b| b as char).collect(),
        _ => String::from_utf8_lossy(body).into_owned(),
    }
}

// --- Encoding (OCSP requests) ---

/// One DER element.
pub fn encode(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let n = body.len();
    if n < 0x80 {
        out.push(n as u8);
    } else {
        let bytes = n.to_be_bytes();
        let skip = bytes.iter().take_while(|&&b| b == 0).count();
        out.push(0x80 | (bytes.len() - skip) as u8);
        out.extend_from_slice(&bytes[skip..]);
    }
    out.extend_from_slice(body);
    out
}

/// A DER OID from its dotted form.
pub fn encode_oid(dotted: &str) -> Vec<u8> {
    let parts: Vec<u64> = dotted.split('.').filter_map(|p| p.parse().ok()).collect();
    let mut body = Vec::new();
    let mut push = |mut v: u64| {
        let mut tmp = vec![(v & 0x7f) as u8];
        v >>= 7;
        while v > 0 {
            tmp.push(0x80 | (v & 0x7f) as u8);
            v >>= 7;
        }
        tmp.reverse();
        body.extend(tmp);
    };
    if parts.len() >= 2 {
        push(parts[0] * 40 + parts[1]);
        for &p in &parts[2..] {
            push(p);
        }
    }
    encode(OID, &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        assert_eq!(
            parse_time("260101000000Z", true),
            Some(days_from_civil(2026, 1, 1) * 86_400)
        );
        assert_eq!(
            parse_time("20260101000000Z", false),
            parse_time("260101000000Z", true)
        );
        assert_eq!(
            parse_time("20260101070000+0700", false),
            parse_time("260101000000Z", true)
        );
        assert_eq!(
            parse_time("20260101000000.123Z", false),
            parse_time("260101000000Z", true)
        );
        assert_eq!(
            civil(days_from_civil(2026, 11, 19) * 86_400 + 3661),
            (2026, 11, 19, 1, 1, 1)
        );
    }

    #[test]
    fn oids_round_trip() {
        let enc = encode_oid("1.2.840.113549.1.7.2");
        let t = Tlv::parse(&enc).unwrap();
        assert_eq!(t.oid().unwrap(), "1.2.840.113549.1.7.2");
    }

    #[test]
    fn indefinite_lengths() {
        // SEQUENCE (indefinite) { INTEGER 5 } EOC
        let data = [0x30, 0x80, 0x02, 0x01, 0x05, 0x00, 0x00];
        let t = Tlv::parse(&data).unwrap();
        assert_eq!(t.raw.len(), 7);
        let kids = t.reader().all().unwrap();
        assert_eq!(kids[0].small_int().unwrap(), 5);
    }
}
