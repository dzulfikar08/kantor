//! Unicode Mathematical Alphanumeric Symbols (U+1D400–U+1D7FF): the
//! italic, bold, script… forms of letters and digits that math fonts
//! draw.

use crate::ast::Variant;

/// Letters the block leaves out because they were encoded earlier
/// (Letterlike Symbols).
fn hole(c: u32) -> u32 {
    match c {
        0x1D455 => 0x210E, // italic h
        0x1D49D => 0x212C, // script B
        0x1D4A0 => 0x2130, // script E
        0x1D4A1 => 0x2131, // script F
        0x1D4A3 => 0x210B, // script H
        0x1D4A4 => 0x2110, // script I
        0x1D4A7 => 0x2112, // script L
        0x1D4A8 => 0x2133, // script M
        0x1D4AD => 0x211B, // script R
        0x1D4BA => 0x212F, // script e
        0x1D4BC => 0x210A, // script g
        0x1D4C4 => 0x2134, // script o
        0x1D506 => 0x212D, // fraktur C
        0x1D50B => 0x210C, // fraktur H
        0x1D50C => 0x2111, // fraktur I
        0x1D515 => 0x211C, // fraktur R
        0x1D51D => 0x2128, // fraktur Z
        0x1D53A => 0x2102, // double-struck C
        0x1D53F => 0x210D, // double-struck H
        0x1D545 => 0x2115, // double-struck N
        0x1D547 => 0x2119, // double-struck P
        0x1D548 => 0x211A, // double-struck Q
        0x1D549 => 0x211D, // double-struck R
        0x1D551 => 0x2124, // double-struck Z
        c => c,
    }
}

/// Start of the Latin capitals for an alphabet, weight and slant.
fn latin_base(v: Variant, bold: bool, italic: bool) -> Option<u32> {
    Some(match (v, bold, italic) {
        (Variant::Roman, true, false) => 0x1D400,
        (Variant::Roman, false, true) => 0x1D434,
        (Variant::Roman, true, true) => 0x1D468,
        (Variant::Script, false, _) => 0x1D49C,
        (Variant::Script, true, _) => 0x1D4D0,
        (Variant::Fraktur, false, _) => 0x1D504,
        (Variant::Fraktur, true, _) => 0x1D56C,
        (Variant::DoubleStruck, _, _) => 0x1D538,
        (Variant::SansSerif, false, false) => 0x1D5A0,
        (Variant::SansSerif, true, false) => 0x1D5D4,
        (Variant::SansSerif, false, true) => 0x1D608,
        (Variant::SansSerif, true, true) => 0x1D63C,
        (Variant::Monospace, _, _) => 0x1D670,
        (Variant::Roman, false, false) => return None,
    })
}

fn greek_base(v: Variant, bold: bool, italic: bool) -> Option<u32> {
    Some(match (v, bold, italic) {
        (Variant::Roman, true, false) => 0x1D6A8,
        (Variant::Roman, false, true) => 0x1D6E2,
        (Variant::Roman, true, true) => 0x1D71C,
        (Variant::SansSerif, true, false) => 0x1D756,
        (Variant::SansSerif, true, true) => 0x1D790,
        _ => return None,
    })
}

fn digit_base(v: Variant, bold: bool) -> Option<u32> {
    Some(match (v, bold) {
        (Variant::Roman, true) => 0x1D7CE,
        (Variant::DoubleStruck, _) => 0x1D7D8,
        (Variant::SansSerif, false) => 0x1D7E2,
        (Variant::SansSerif, true) => 0x1D7EC,
        (Variant::Monospace, _) => 0x1D7F6,
        _ => return None,
    })
}

/// Position of a Greek letter in the block's Greek alphabets.
fn greek_index(c: char) -> Option<u32> {
    let u = c as u32;
    Some(match c {
        'Α'..='Ρ' => u - 0x391,
        'ϴ' => 17,
        'Σ'..='Ω' => u - 0x3A3 + 18,
        '∇' => 25,
        'α'..='ω' => 26 + u - 0x3B1,
        '∂' => 51,
        'ϵ' => 52,
        'ϑ' => 53,
        'ϰ' => 54,
        'ϕ' => 55,
        'ϱ' => 56,
        'ϖ' => 57,
        _ => return None,
    })
}

const GREEK_TAIL: [char; 6] = ['∂', 'ϵ', 'ϑ', 'ϰ', 'ϕ', 'ϱ'];

/// The styled form of `c`, or `c` when there is none.
pub fn styled(c: char, v: Variant, bold: bool, italic: bool) -> char {
    let u = c as u32;
    let mapped = if c.is_ascii_uppercase() || c.is_ascii_lowercase() {
        latin_base(v, bold, italic).map(|b| {
            let off = if c.is_ascii_uppercase() {
                u - 'A' as u32
            } else {
                26 + u - 'a' as u32
            };
            hole(b + off)
        })
    } else if c.is_ascii_digit() {
        digit_base(v, bold).map(|b| b + u - '0' as u32)
    } else if let Some(i) = greek_index(c) {
        greek_base(v, bold, italic).map(|b| b + i)
    } else if italic && !bold && v == Variant::Roman {
        match c {
            'ı' => Some(0x1D6A4),
            'ȷ' => Some(0x1D6A5),
            _ => None,
        }
    } else {
        None
    };
    mapped.and_then(char::from_u32).unwrap_or(c)
}

/// The plain letter a styled one stands for, with its alphabet, weight
/// and slant.
pub fn unstyle(c: char) -> (char, Variant, bool, bool) {
    let u = c as u32;
    // Letterlike holes first.
    if (0x2100..0x2140).contains(&u) {
        for b in 0x1D400u32..0x1D6A4 {
            if hole(b) == u && b != u {
                return unstyle_latin(b).unwrap_or((c, Variant::Roman, false, false));
            }
        }
    }
    if (0x1D400..0x1D6A4).contains(&u) {
        return unstyle_latin(u).unwrap_or((c, Variant::Roman, false, false));
    }
    match u {
        0x1D6A4 => return ('ı', Variant::Roman, false, true),
        0x1D6A5 => return ('ȷ', Variant::Roman, false, true),
        _ => {}
    }
    let greek: [(u32, Variant, bool, bool); 5] = [
        (0x1D6A8, Variant::Roman, true, false),
        (0x1D6E2, Variant::Roman, false, true),
        (0x1D71C, Variant::Roman, true, true),
        (0x1D756, Variant::SansSerif, true, false),
        (0x1D790, Variant::SansSerif, true, true),
    ];
    for (b, v, bold, it) in greek {
        if (b..b + 58).contains(&u) {
            let i = u - b;
            let ch = match i {
                0..=16 => char::from_u32(0x391 + i),
                17 => Some('ϴ'),
                18..=24 => char::from_u32(0x3A3 + i - 18),
                25 => Some('∇'),
                26..=50 => char::from_u32(0x3B1 + i - 26),
                51..=56 => Some(GREEK_TAIL[(i - 51) as usize]),
                _ => Some('ϖ'),
            };
            return (ch.unwrap_or(c), v, bold, it);
        }
    }
    let digits: [(u32, Variant, bool); 5] = [
        (0x1D7CE, Variant::Roman, true),
        (0x1D7D8, Variant::DoubleStruck, false),
        (0x1D7E2, Variant::SansSerif, false),
        (0x1D7EC, Variant::SansSerif, true),
        (0x1D7F6, Variant::Monospace, false),
    ];
    for (b, v, bold) in digits {
        if (b..b + 10).contains(&u) {
            return (
                char::from_u32('0' as u32 + u - b).unwrap_or(c),
                v,
                bold,
                false,
            );
        }
    }
    (c, Variant::Roman, false, false)
}

fn unstyle_latin(u: u32) -> Option<(char, Variant, bool, bool)> {
    let blocks: [(u32, Variant, bool, bool); 13] = [
        (0x1D400, Variant::Roman, true, false),
        (0x1D434, Variant::Roman, false, true),
        (0x1D468, Variant::Roman, true, true),
        (0x1D49C, Variant::Script, false, false),
        (0x1D4D0, Variant::Script, true, false),
        (0x1D504, Variant::Fraktur, false, false),
        (0x1D538, Variant::DoubleStruck, false, false),
        (0x1D56C, Variant::Fraktur, true, false),
        (0x1D5A0, Variant::SansSerif, false, false),
        (0x1D5D4, Variant::SansSerif, true, false),
        (0x1D608, Variant::SansSerif, false, true),
        (0x1D63C, Variant::SansSerif, true, true),
        (0x1D670, Variant::Monospace, false, false),
    ];
    for (b, v, bold, it) in blocks {
        if (b..b + 52).contains(&u) {
            let i = u - b;
            let ch = if i < 26 {
                char::from_u32('A' as u32 + i)
            } else {
                char::from_u32('a' as u32 + i - 26)
            };
            return ch.map(|ch| (ch, v, bold, it));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        assert_eq!(styled('x', Variant::Roman, false, true), '𝑥');
        assert_eq!(styled('h', Variant::Roman, false, true), 'ℎ');
        assert_eq!(styled('R', Variant::DoubleStruck, false, false), 'ℝ');
        assert_eq!(styled('α', Variant::Roman, false, true), '𝛼');
        assert_eq!(styled('A', Variant::Script, false, false), '𝒜');
        assert_eq!(styled('2', Variant::Roman, true, false), '𝟐');
        assert_eq!(unstyle('ℝ'), ('R', Variant::DoubleStruck, false, false));
        assert_eq!(unstyle('𝑥'), ('x', Variant::Roman, false, true));
        assert_eq!(unstyle('ℎ'), ('h', Variant::Roman, false, true));
        assert_eq!(unstyle('𝛼'), ('α', Variant::Roman, false, true));
        assert_eq!(unstyle('𝐀'), ('A', Variant::Roman, true, false));
        assert_eq!(unstyle('𝟐'), ('2', Variant::Roman, true, false));
        for c in "abcXYZ".chars() {
            for v in [Variant::Script, Variant::Fraktur, Variant::DoubleStruck] {
                let s = styled(c, v, false, false);
                assert_eq!(unstyle(s).0, c, "{c} {v:?}");
            }
        }
    }
}
