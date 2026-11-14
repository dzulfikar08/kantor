//! BIFF8 parsed formulas (`rgce`, [MS-XLS] 2.5.198) back to Excel's A1
//! text, for the formulas of data validation and conditional formatting
//! (cell formulas come from calamine). The common tokens are handled:
//! numbers, strings, booleans, errors, operators, parentheses, references
//! (plain, relative to a base cell, 3-D on this workbook's sheets), areas,
//! defined names and built-in functions. Anything else (array constants,
//! external references, add-in functions, shared formulas) gives `None` and
//! the caller skips the rule.

use crate::xls_ftab::FTAB;

/// What references in a formula resolve against.
pub(crate) struct Context<'a> {
    /// The cell relative references count from (0-based row, column).
    pub base: (u32, u32),
    /// Sheet names by `BOUNDSHEET` index.
    pub sheets: &'a [String],
    /// `EXTERNSHEET` entries: the first and last sheet index of each, or
    /// `None` for another workbook's.
    pub xti: &'a [Option<(u16, u16)>],
    /// Defined names by `LBL` index (1-based in tokens).
    pub names: &'a [String],
}

/// BIFF8's grid: 65,536 rows by 256 columns.
const ROWS: i64 = 65_536;
const COLS: i64 = 256;

fn col_name(mut col: u32) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (col % 26) as u8);
        if col < 26 {
            break;
        }
        col = col / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

/// A cell from a token's row and column fields. `rel` makes relative parts
/// offsets from the base (`PtgRefN`, `PtgAreaN`).
fn cell(rw: u16, col: u16, rel: bool, base: (u32, u32)) -> (u32, u32, bool, bool) {
    let col_rel = col & 0x4000 != 0;
    let row_rel = col & 0x8000 != 0;
    let c = col & 0x3FFF;
    let row = if rel && row_rel {
        (base.0 as i64 + rw as i16 as i64).rem_euclid(ROWS) as u32
    } else {
        rw as u32
    };
    let column = if rel && col_rel {
        (base.1 as i64 + (c & 0xFF) as u8 as i8 as i64).rem_euclid(COLS) as u32
    } else {
        c as u32
    };
    (row, column, row_rel, col_rel)
}

fn a1((row, col, row_rel, col_rel): (u32, u32, bool, bool)) -> String {
    format!(
        "{}{}{}{}",
        if col_rel { "" } else { "$" },
        col_name(col),
        if row_rel { "" } else { "$" },
        row + 1
    )
}

fn area(first: (u32, u32, bool, bool), last: (u32, u32, bool, bool)) -> String {
    // Every row: a whole-column reference; every column: whole rows.
    if first.0 == 0 && last.0 as i64 == ROWS - 1 {
        let c = |(_, c, _, rel): (u32, u32, bool, bool)| {
            format!("{}{}", if rel { "" } else { "$" }, col_name(c))
        };
        return format!("{}:{}", c(first), c(last));
    }
    if first.1 == 0 && last.1 as i64 == COLS - 1 {
        let r = |(r, _, rel, _): (u32, u32, bool, bool)| {
            format!("{}{}", if rel { "" } else { "$" }, r + 1)
        };
        return format!("{}:{}", r(first), r(last));
    }
    format!("{}:{}", a1(first), a1(last))
}

/// A sheet name as a formula prefix, quoted when it needs to be.
fn sheet_prefix(name: &str) -> String {
    let plain = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && crate::shift::parse_range(name).is_none();
    if plain {
        format!("{name}!")
    } else {
        format!("'{}'!", name.replace('\'', "''"))
    }
}

fn number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{v:.0}")
    } else {
        format!("{v}")
    }
}

fn error(code: u8) -> Option<&'static str> {
    Some(match code {
        0x00 => "#NULL!",
        0x07 => "#DIV/0!",
        0x0F => "#VALUE!",
        0x17 => "#REF!",
        0x1D => "#NAME?",
        0x24 => "#NUM!",
        0x2A => "#N/A",
        _ => return None,
    })
}

struct Reader<'a> {
    d: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.d.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// The formula `rgce` spells, without `=`; `None` if a token isn't
/// supported or the tokens don't make one expression.
pub(crate) fn decompile(rgce: &[u8], ctx: &Context) -> Option<String> {
    let mut r = Reader { d: rgce, pos: 0 };
    let mut stack: Vec<String> = Vec::new();
    while r.pos < rgce.len() {
        let raw = r.u8()?;
        // Operand tokens come in three classes (reference, value, array).
        let ptg = if raw >= 0x20 {
            (raw & 0x1F) | 0x20
        } else {
            raw
        };
        match ptg {
            0x03..=0x11 => {
                let op = [
                    "+", "-", "*", "/", "^", "&", "<", "<=", "=", ">=", ">", "<>", " ", ",", ":",
                ][(ptg - 0x03) as usize];
                let b = stack.pop()?;
                let a = stack.pop()?;
                stack.push(format!("{a}{op}{b}"));
            }
            0x12 => {
                let a = stack.pop()?;
                stack.push(format!("+{a}"));
            }
            0x13 => {
                let a = stack.pop()?;
                stack.push(format!("-{a}"));
            }
            0x14 => {
                let a = stack.pop()?;
                stack.push(format!("{a}%"));
            }
            0x15 => {
                let a = stack.pop()?;
                stack.push(format!("({a})"));
            }
            0x16 => stack.push(String::new()),
            0x17 => {
                let cch = r.u8()? as usize;
                let flags = r.u8()?;
                let s = if flags & 0x01 != 0 {
                    let raw = r.take(cch.checked_mul(2)?)?;
                    let units: Vec<u16> = raw
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect();
                    String::from_utf16_lossy(&units)
                } else {
                    r.take(cch)?.iter().map(|&b| b as char).collect()
                };
                stack.push(format!("\"{}\"", s.replace('"', "\"\"")));
            }
            0x19 => {
                let kind = r.u8()?;
                let data = r.u16()?;
                match kind {
                    // Volatile, if, goto, space: no effect on the text.
                    0x01 | 0x02 | 0x08 | 0x20 | 0x40 | 0x41 => {}
                    // Choose: a jump table follows.
                    0x04 => {
                        r.take((data as usize + 1).checked_mul(2)?)?;
                    }
                    // SUM of one argument.
                    0x10 => {
                        let a = stack.pop()?;
                        stack.push(format!("SUM({a})"));
                    }
                    _ => return None,
                }
            }
            0x1C => stack.push(error(r.u8()?)?.to_owned()),
            0x1D => stack.push(if r.u8()? != 0 { "TRUE" } else { "FALSE" }.to_owned()),
            0x1E => stack.push(r.u16()?.to_string()),
            0x1F => {
                let b = r.take(8)?;
                let mut a = [0u8; 8];
                a.copy_from_slice(b);
                let v = f64::from_le_bytes(a);
                if !v.is_finite() {
                    return None;
                }
                stack.push(number(v));
            }
            0x21 | 0x22 => {
                let (argc, iftab) = if ptg == 0x21 {
                    let iftab = r.u16()?;
                    (FTAB.get(iftab as usize)?.1, iftab)
                } else {
                    let argc = r.u8()? & 0x7F;
                    let iftab = r.u16()?;
                    if iftab & 0x8000 != 0 {
                        return None; // a command-equivalent function
                    }
                    (argc, iftab)
                };
                let (name, _) = FTAB.get(iftab as usize)?;
                // 255: an add-in or user-defined function (its name is the
                // first argument, a PtgNameX).
                if argc == 255 || name.is_empty() || iftab == 255 {
                    return None;
                }
                let at = stack.len().checked_sub(argc as usize)?;
                let args = stack.split_off(at);
                stack.push(format!("{name}({})", args.join(",")));
            }
            0x23 => {
                let ix = r.u32()? as usize;
                let name = ctx.names.get(ix.checked_sub(1)?)?;
                if name.is_empty() {
                    return None;
                }
                stack.push(name.clone());
            }
            0x24 | 0x2C => {
                let (rw, col) = (r.u16()?, r.u16()?);
                stack.push(a1(cell(rw, col, ptg == 0x2C, ctx.base)));
            }
            0x25 | 0x2D => {
                let (r1, r2, c1, c2) = (r.u16()?, r.u16()?, r.u16()?, r.u16()?);
                let rel = ptg == 0x2D;
                stack.push(area(
                    cell(r1, c1, rel, ctx.base),
                    cell(r2, c2, rel, ctx.base),
                ));
            }
            // Mem tokens: a subexpression follows; its size is all they add.
            0x26..=0x28 => {
                r.take(6)?;
            }
            0x29 => {
                r.take(2)?;
            }
            0x2A => {
                r.take(4)?;
                stack.push("#REF!".into());
            }
            0x2B => {
                r.take(8)?;
                stack.push("#REF!".into());
            }
            0x3A | 0x3B => {
                let ixti = r.u16()? as usize;
                let (first, last) = (*ctx.xti.get(ixti)?)?;
                let name = |i: u16| ctx.sheets.get(i as usize).cloned();
                let prefix = if first == last {
                    sheet_prefix(&name(first)?)
                } else {
                    let (a, b) = (name(first)?, name(last)?);
                    let joined = format!("{a}:{b}");
                    let plain =
                        sheet_prefix(&a) == format!("{a}!") && sheet_prefix(&b) == format!("{b}!");
                    if plain {
                        format!("{joined}!")
                    } else {
                        format!("'{}'!", joined.replace('\'', "''"))
                    }
                };
                let text = if ptg == 0x3A {
                    let (rw, col) = (r.u16()?, r.u16()?);
                    a1(cell(rw, col, false, ctx.base))
                } else {
                    let (r1, r2, c1, c2) = (r.u16()?, r.u16()?, r.u16()?, r.u16()?);
                    area(cell(r1, c1, false, ctx.base), cell(r2, c2, false, ctx.base))
                };
                stack.push(format!("{prefix}{text}"));
            }
            0x3C | 0x3D => {
                // Ref3dErr / Area3dErr
                r.take(if ptg == 0x3C { 6 } else { 10 })?;
                stack.push("#REF!".into());
            }
            _ => return None,
        }
    }
    if stack.len() == 1 { stack.pop() } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Context<'static> {
        static SHEETS: [String; 0] = [];
        Context {
            base: (1, 0),
            sheets: &SHEETS,
            xti: &[],
            names: &[],
        }
    }

    #[test]
    fn tokens() {
        let c = ctx();
        // ISNUMBER(I1), I1 relative to I1 itself.
        let i1 = Context {
            base: (0, 8),
            ..ctx()
        };
        assert_eq!(
            decompile(&[0x4C, 0, 0, 0, 0xC0, 0x41, 0x80, 0x00], &i1).as_deref(),
            Some("ISNUMBER(I1)")
        );
        // 1 + 2 * 3
        assert_eq!(
            decompile(&[0x1E, 1, 0, 0x1E, 2, 0, 0x1E, 3, 0, 0x05, 0x03], &c).as_deref(),
            Some("1+2*3")
        );
        // "a""b" & TRUE
        assert_eq!(
            decompile(&[0x17, 4, 0, b'a', b'"', b'b', b'c', 0x1D, 1, 0x08], &c).as_deref(),
            Some("\"a\"\"bc\"&TRUE")
        );
        // $A$1:$B$3 absolute area; A:A whole column.
        assert_eq!(
            decompile(&[0x25, 0, 0, 2, 0, 0, 0, 1, 0], &c).as_deref(),
            Some("$A$1:$B$3")
        );
        assert_eq!(
            decompile(&[0x25, 0, 0, 0xFF, 0xFF, 0, 0xC0, 0, 0xC0], &c).as_deref(),
            Some("A:A")
        );
        // A relative reference one row up, one column left of B2... from A2
        // it wraps to the last column.
        assert_eq!(
            decompile(&[0x4C, 0xFF, 0xFF, 0xFF, 0xC0], &c).as_deref(),
            Some("IV1")
        );
        // Unsupported (array constant) and unbalanced.
        assert_eq!(decompile(&[0x20, 0, 0, 0, 0, 0, 0, 0], &c), None);
        assert_eq!(decompile(&[0x1E, 1, 0, 0x1E, 1, 0], &c), None);
        assert_eq!(decompile(&[0x03], &c), None);
    }

    #[test]
    fn garbage_doesnt_panic() {
        let c = ctx();
        for seed in 0..2000u32 {
            let bytes: Vec<u8> = (0..(seed % 23))
                .map(|i| (seed.wrapping_mul(2654435761).wrapping_add(i * 97) >> (i % 8)) as u8)
                .collect();
            let _ = decompile(&bytes, &c);
        }
    }
}
