//! Little-endian reads that give 0 past the end (a damaged record draws
//! wrongly instead of failing the picture).

pub(crate) fn u8_at(b: &[u8], o: usize) -> u8 {
    b.get(o).copied().unwrap_or(0)
}

pub(crate) fn u16_at(b: &[u8], o: usize) -> u16 {
    match b.get(o..o.wrapping_add(2)) {
        Some(s) => u16::from_le_bytes([s[0], s[1]]),
        None => 0,
    }
}

pub(crate) fn i16_at(b: &[u8], o: usize) -> i16 {
    u16_at(b, o) as i16
}

pub(crate) fn u32_at(b: &[u8], o: usize) -> u32 {
    match b.get(o..o.wrapping_add(4)) {
        Some(s) => u32::from_le_bytes([s[0], s[1], s[2], s[3]]),
        None => 0,
    }
}

pub(crate) fn i32_at(b: &[u8], o: usize) -> i32 {
    u32_at(b, o) as i32
}

pub(crate) fn f32_at(b: &[u8], o: usize) -> f64 {
    let v = f32::from_bits(u32_at(b, o)) as f64;
    if v.is_finite() { v } else { 0.0 }
}

pub(crate) fn be16(b: &[u8], o: usize) -> u16 {
    match b.get(o..o.wrapping_add(2)) {
        Some(s) => u16::from_be_bytes([s[0], s[1]]),
        None => 0,
    }
}

pub(crate) fn be32(b: &[u8], o: usize) -> u32 {
    match b.get(o..o.wrapping_add(4)) {
        Some(s) => u32::from_be_bytes([s[0], s[1], s[2], s[3]]),
        None => 0,
    }
}

/// A COLORREF (`0x00BBGGRR`).
pub(crate) fn colorref(b: &[u8], o: usize) -> [u8; 3] {
    [u8_at(b, o), u8_at(b, o + 1), u8_at(b, o + 2)]
}

/// UTF-16LE text up to the first NUL.
pub(crate) fn utf16(b: &[u8], o: usize, units: usize) -> String {
    let v: Vec<u16> = (0..units)
        .map(|i| u16_at(b, o + 2 * i))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&v)
}

/// Windows-1252 text (what 8-bit metafile strings almost always are);
/// controls become spaces.
pub(crate) fn ansi(b: &[u8]) -> String {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}',
        'ž', 'Ÿ',
    ];
    b.iter()
        .map(|&c| match c {
            0x80..=0x9F => HIGH[(c - 0x80) as usize],
            c => c as char,
        })
        .collect()
}
