//! KANTOR PATCH (functions): BAHTTEXT, USDOLLAR, DBCS/JIS and PHONETIC.

use crate::{
    calc_result::CalcResult,
    expressions::{parser::Node, token::Error, types::CellReferenceIndex},
    model::Model,
};

const TH_DIGITS: [&str; 10] = [
    "ศูนย์",
    "หนึ่ง",
    "สอง",
    "สาม",
    "สี่",
    "ห้า",
    "หก",
    "เจ็ด",
    "แปด",
    "เก้า",
];

/// Thai words for `value` (1 to 999,999), the way Excel's BAHTTEXT says them:
/// 1 in the tens is "สิบ", 2 is "ยี่สิบ", a final 1 after a tens digit is
/// "เอ็ด". (The same algorithm as LibreOffice's Excel-compatible BAHTTEXT.)
fn append_block(text: &mut String, mut value: u64) {
    for (power, word) in [
        (100_000, "แสน"),
        (10_000, "หมื่น"),
        (1_000, "พัน"),
        (100, "ร้อย"),
    ] {
        if value >= power {
            text.push_str(TH_DIGITS[(value / power) as usize]);
            text.push_str(word);
            value %= power;
        }
    }
    if value > 0 {
        let tens = value / 10;
        let ones = value % 10;
        if tens >= 1 {
            match tens {
                1 => {}
                2 => text.push_str("ยี่"),
                _ => text.push_str(TH_DIGITS[tens as usize]),
            }
            text.push_str("สิบ");
        }
        if tens > 0 && ones == 1 {
            text.push_str("เอ็ด");
        } else if ones > 0 {
            text.push_str(TH_DIGITS[ones as usize]);
        }
    }
}

/// The amount `value` in Thai words, in baht and satang.
pub(crate) fn baht_text(value: f64) -> String {
    let minus = value < 0.0;
    // Rounded to satang, half away from zero.
    let cents = (value.abs() * 100.0 + 0.5).floor();
    let mut baht = (cents / 100.0).floor();
    let satang = (cents - baht * 100.0) as u64;

    let mut text = String::new();
    if baht == 0.0 {
        if satang == 0 {
            text.push_str(TH_DIGITS[0]);
        }
    } else {
        // Blocks of six digits, lowest first, each but the highest led by "ล้าน".
        let mut blocks: Vec<String> = Vec::new();
        while baht > 0.0 {
            let block = (baht % 1_000_000.0) as u64;
            baht = (baht / 1_000_000.0).floor();
            let mut words = String::new();
            if baht > 0.0 {
                words.push_str("ล้าน");
            }
            append_block(&mut words, block);
            blocks.push(words);
        }
        for block in blocks.iter().rev() {
            text.push_str(block);
        }
    }
    if !text.is_empty() {
        text.push_str("บาท");
    }
    if satang == 0 {
        text.push_str("ถ้วน");
    } else {
        append_block(&mut text, satang);
        text.push_str("สตางค์");
    }
    if minus {
        text.insert_str(0, "ลบ");
    }
    text
}

/// Half-width ASCII and space to full width (the opposite of ASC).
fn dbcs_convert(text: &str) -> String {
    text.chars()
        .map(|c| match c as u32 {
            0x20 => '\u{3000}',
            cp @ 0x21..=0x7E => char::from_u32(cp - 0x21 + 0xFF01).unwrap_or(c),
            _ => c,
        })
        .collect()
}

impl<'a> Model<'a> {
    /// `=BAHTTEXT(number)`: the number as Thai baht in words.
    pub(crate) fn fn_bahttext(&mut self, args: &[Node], cell: CellReferenceIndex) -> CalcResult {
        if args.len() != 1 {
            return CalcResult::new_args_number_error(cell);
        }
        let value = match self.get_number(&args[0], cell) {
            Ok(f) => f,
            Err(e) => return e,
        };
        if value.abs() >= 1e15 {
            return CalcResult::new_error(Error::NUM, cell, "Number too large".to_string());
        }
        CalcResult::String(baht_text(value))
    }

    /// `=USDOLLAR(number, [decimals])`: an old name of DOLLAR.
    pub(crate) fn fn_usdollar(&mut self, args: &[Node], cell: CellReferenceIndex) -> CalcResult {
        self.fn_dollar(args, cell)
    }

    /// `=DBCS(text)` and `=JIS(text)`: half-width (single-byte) letters,
    /// digits, signs and spaces to full width.
    pub(crate) fn fn_dbcs(&mut self, args: &[Node], cell: CellReferenceIndex) -> CalcResult {
        if args.len() != 1 {
            return CalcResult::new_args_number_error(cell);
        }
        match self.get_string(&args[0], cell) {
            Ok(s) => CalcResult::String(dbcs_convert(&s)),
            Err(e) => e,
        }
    }

    /// `=PHONETIC(reference)`: the furigana of the cell. Kantor keeps no
    /// phonetic guides, so this is the cell's text, as Excel gives it for a
    /// cell without them (the top left cell of a range).
    pub(crate) fn fn_phonetic(&mut self, args: &[Node], cell: CellReferenceIndex) -> CalcResult {
        if args.len() != 1 {
            return CalcResult::new_args_number_error(cell);
        }
        match self.evaluate_node_with_reference(&args[0], cell) {
            CalcResult::Range { left, .. } => match self.evaluate_cell(left) {
                CalcResult::String(s) => CalcResult::String(s),
                error @ CalcResult::Error { .. } => error,
                _ => CalcResult::String(String::new()),
            },
            error @ CalcResult::Error { .. } => error,
            _ => CalcResult::new_error(Error::NA, cell, "PHONETIC needs a reference".to_string()),
        }
    }
}
