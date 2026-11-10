//! Locale tags in format codes: `[$-421]`, `[$Rp-421]`, `[$€-407]` (Kantor addition; GridCraft
//! ignores them).
//!
//! The hex number after the `-` is a Windows LCID; its low 16 bits are the language id. Excel
//! uses it for month and day names, so `[$-421]d mmmm yyyy` shows `17 Agustus 2025` in any
//! Excel. Kantor also takes the decimal and thousands separators from it, so a file made in an
//! Indonesian Excel (`[$Rp-421]#,##0.00`) shows `Rp1.234,50` as its author saw it. Excel itself
//! takes separators from the system settings instead; without a tag Kantor shows en-US
//! separators (`1,234.50`), which is what Excel shows on an en-US system.
//!
//! Names are only known for Indonesian; other languages keep English names but get their
//! separators.

/// How a language writes numbers and dates.
#[derive(Debug, PartialEq, Eq)]
pub struct Locale {
    /// Decimal separator (what `.` in a format code shows).
    pub decimal: char,
    /// Thousands separator (what `,` between digits shows).
    pub group: char,
    /// Month names (`mmmm`) and abbreviations (`mmm`), January first; `None` keeps English.
    pub months: Option<&'static [&'static str; 12]>,
    pub months_short: Option<&'static [&'static str; 12]>,
    /// Weekday names (`dddd`) and abbreviations (`ddd`), Sunday first; `None` keeps English.
    pub days: Option<&'static [&'static str; 7]>,
    pub days_short: Option<&'static [&'static str; 7]>,
}

/// Indonesian (id-ID, LCID 0x0421): the names Windows and Excel use.
pub static INDONESIAN: Locale = Locale {
    decimal: ',',
    group: '.',
    months: Some(&["Januari", "Februari", "Maret", "April", "Mei", "Juni", "Juli", "Agustus", "September", "Oktober", "November", "Desember"]),
    months_short: Some(&["Jan", "Feb", "Mar", "Apr", "Mei", "Jun", "Jul", "Agu", "Sep", "Okt", "Nov", "Des"]),
    days: Some(&["Minggu", "Senin", "Selasa", "Rabu", "Kamis", "Jumat", "Sabtu"]),
    days_short: Some(&["Min", "Sen", "Sel", "Rab", "Kam", "Jum", "Sab"]),
};

const fn seps(decimal: char, group: char) -> Locale {
    Locale { decimal, group, months: None, months_short: None, days: None, days_short: None }
}

/// Decimal comma, thousands dot (German, Dutch, Spanish, Italian, Portuguese, Turkish…).
static COMMA_DOT: Locale = seps(',', '.');
/// Decimal comma, no-break space between thousands (French, Russian, Polish, Swedish…).
static COMMA_SPACE: Locale = seps(',', '\u{a0}');
/// Decimal point, apostrophe between thousands (Swiss German and Italian).
static POINT_APOSTROPHE: Locale = seps('.', '\u{2019}');

/// The locale of an LCID, or `None` for English-style numbers and names (en-US, en-GB, ja-JP,
/// zh-CN, a system tag like `F800`, any language not listed).
pub fn for_lcid(lcid: u32) -> Option<&'static Locale> {
    let lang = lcid & 0xFFFF;
    // Below 0x400 there is no sublanguage: Excel's own `[$€-1]` / `[$€-2]` currency markers
    // (`€ 123` / `123 €`), not a language.
    if lang < 0x400 {
        return None;
    }
    // Variants that write numbers differently from their primary language.
    match lang {
        0x0807 | 0x1407 | 0x0810 => return Some(&POINT_APOSTROPHE), // de-CH, de-LI, it-CH
        0x100C => return Some(&COMMA_SPACE),                        // fr-CH
        0x080A | 0x540A | 0x500A | 0x1C0A => return None,           // es-MX, es-US, es-PR, es-DO
        _ => {}
    }
    match lang & 0x3FF {
        0x21 => Some(&INDONESIAN),
        // de, nl, es, it, pt, da, tr, el, ro, hr/sr/bs, sl, vi.
        0x07 | 0x13 | 0x0A | 0x10 | 0x16 | 0x06 | 0x1F | 0x08 | 0x18 | 0x1A | 0x24 | 0x2A => Some(&COMMA_DOT),
        // fr, ru, pl, cs, sv, nb/nn, fi, uk, hu, sk, bg, lt, lv, et.
        0x0C | 0x19 | 0x15 | 0x05 | 0x1D | 0x14 | 0x0B | 0x22 | 0x0E | 0x1B | 0x02 | 0x27 | 0x26 | 0x25 => Some(&COMMA_SPACE),
        _ => None,
    }
}

/// The LCID in the body of a `[$…]` tag (what follows the `$`): `Rp-421` → 0x421. `None` when
/// there is no `-` or what follows is not hex (`€-x-euro2`, `-x-sysdate`).
pub(crate) fn parse_lcid(body: &str) -> Option<u32> {
    let (_, hex) = body.split_once('-')?;
    if hex.is_empty() || hex.len() > 8 {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lcids() {
        assert_eq!(parse_lcid("Rp-421"), Some(0x421));
        assert_eq!(parse_lcid("-421"), Some(0x421));
        assert_eq!(parse_lcid("-F800"), Some(0xF800));
        assert_eq!(parse_lcid("€-x-euro2"), None);
        assert_eq!(parse_lcid("$"), None);
        assert_eq!(for_lcid(0x421), Some(&INDONESIAN));
        assert_eq!(for_lcid(0x409), None);
        assert_eq!(for_lcid(0xF800), None);
        assert_eq!(for_lcid(0x407).map(|l| (l.decimal, l.group)), Some((',', '.')));
        assert_eq!(for_lcid(0x807).map(|l| (l.decimal, l.group)), Some(('.', '\u{2019}')));
        assert_eq!(for_lcid(0x40C).map(|l| (l.decimal, l.group)), Some((',', '\u{a0}')));
        assert_eq!(for_lcid(0x80A), None);
        assert_eq!(for_lcid(0x2), None);
    }
}
