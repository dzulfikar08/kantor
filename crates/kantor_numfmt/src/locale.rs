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

use std::cell::Cell;
use std::sync::atomic::{AtomicU8, Ordering};

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

/// The display locale (KANTOR PATCH): what `.` and `,` in an untagged format code show when a
/// cell is displayed ([`crate::NumberFormat::display_number`]), as Excel takes them from the
/// system's regional settings. 0 is en-US (`1,234.5`), 1 Indonesian (`1.234,5`).
static DISPLAY: AtomicU8 = AtomicU8::new(0);

/// Which separators (and month and day names, for codes without a locale tag) cells are
/// displayed with. Set once by the app from the interface language or the user's override.
/// Only display changes: `TEXT()` results, saved files and exports stay en-US.
pub fn set_display_locale(locale: DisplayLocale) {
    DISPLAY.store(locale as u8, Ordering::Relaxed);
}

/// The locale set with [`set_display_locale`].
pub fn display_locale() -> DisplayLocale {
    match DISPLAY.load(Ordering::Relaxed) {
        1 => DisplayLocale::Indonesian,
        _ => DisplayLocale::EnUs,
    }
}

/// The locales cells can be displayed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum DisplayLocale {
    /// `1,234.5`, English names.
    #[default]
    EnUs = 0,
    /// `1.234,5`, Indonesian names.
    Indonesian = 1,
}

impl DisplayLocale {
    /// Its separators and names (`None`: en-US).
    pub fn locale(self) -> Option<&'static Locale> {
        match self {
            Self::EnUs => None,
            Self::Indonesian => Some(&INDONESIAN),
        }
    }

    /// The decimal separator (`.` or `,`).
    pub fn decimal(self) -> char {
        self.locale().map_or('.', |l| l.decimal)
    }

    /// The thousands separator (`,` or `.`).
    pub fn group(self) -> char {
        self.locale().map_or(',', |l| l.group)
    }
}

thread_local! {
    /// The locale of the display call running on this thread (see [`displaying`]).
    static ACTIVE: Cell<Option<&'static Locale>> = const { Cell::new(None) };
    /// Set inside [`invariant`]: display calls use en-US whatever the display locale.
    static INVARIANT: Cell<bool> = const { Cell::new(false) };
}

/// The locale untagged sections render with right now: the display locale inside a display
/// call, otherwise none (en-US), so `TEXT()` and other formatting stay invariant.
pub(crate) fn active() -> Option<&'static Locale> {
    ACTIVE.try_with(Cell::get).ok().flatten()
}

/// Run `f` (a display call) with `display` active (en-US inside [`invariant`]).
pub(crate) fn displaying<R>(display: DisplayLocale, f: impl FnOnce() -> R) -> R {
    let invariant = INVARIANT.try_with(Cell::get).unwrap_or(false);
    let locale = if invariant { None } else { display.locale() };
    let before = ACTIVE.try_with(|a| a.replace(locale)).ok().flatten();
    let r = f();
    let _ = ACTIVE.try_with(|a| a.set(before));
    r
}

/// Run `f` with displays in en-US on this thread whatever the display locale: for output that
/// is file content (CSV export) rather than what the user reads.
pub fn invariant<R>(f: impl FnOnce() -> R) -> R {
    let before = INVARIANT.try_with(|i| i.replace(true)).unwrap_or(false);
    let r = f();
    let _ = INVARIANT.try_with(|i| i.set(before));
    r
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
