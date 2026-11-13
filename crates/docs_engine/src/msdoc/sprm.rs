//! Property modifiers ([MS-DOC] 2.6 "Sprm"): walking a grpprl and folding
//! the sprms Kantor understands into character, paragraph, table and
//! section properties. Unknown sprms are skipped by their operand size.

use super::rd::{i16_at, i32_at, u16_at, u32_at};

/// The sprms of a grpprl with their operands. Stops at the first sprm whose
/// operand runs past the end.
pub fn sprms(g: &[u8]) -> Vec<(u16, &[u8])> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 2 <= g.len() {
        let id = u16::from_le_bytes([g[i], g[i + 1]]);
        let at = i + 2;
        let (start, len) = match id >> 13 {
            0 | 1 => (at, 1),
            2 | 4 | 5 => (at, 2),
            3 => (at, 4),
            7 => (at, 3),
            _ => match id {
                // sprmTDefTable: a 2-byte size of the rest, plus one.
                0xD608 => match u16_at(g, at) {
                    Some(cb) => (at + 2, (cb as usize).saturating_sub(1)),
                    None => break,
                },
                // sprmPChgTabs: size 255 means "work it out from the content".
                0xC615 if g.get(at) == Some(&255) => {
                    let del = *g.get(at + 1).unwrap_or(&0) as usize;
                    let add_at = at + 2 + del * 4;
                    let add = *g.get(add_at).unwrap_or(&0) as usize;
                    (at, 2 + del * 4 + 1 + add * 3)
                }
                _ => match g.get(at) {
                    // The operand slice keeps its size byte.
                    Some(&cb) => (at, cb as usize + 1),
                    None => break,
                },
            },
        };
        let Some(op) = g.get(start..start + len) else {
            break;
        };
        out.push((id, op));
        i = start + len;
    }
    out
}

/// A colour: `None` is automatic.
pub type Color = Option<[u8; 3]>;

/// The 16 colours of the old `ico` palette.
pub fn ico(i: u8) -> Color {
    const T: [[u8; 3]; 16] = [
        [0, 0, 0],
        [0, 0, 0xFF],
        [0, 0xFF, 0xFF],
        [0, 0xFF, 0],
        [0xFF, 0, 0xFF],
        [0xFF, 0, 0],
        [0xFF, 0xFF, 0],
        [0xFF, 0xFF, 0xFF],
        [0, 0, 0x80],
        [0, 0x80, 0x80],
        [0, 0x80, 0],
        [0x80, 0, 0x80],
        [0x80, 0, 0],
        [0x80, 0x80, 0],
        [0x80, 0x80, 0x80],
        [0xC0, 0xC0, 0xC0],
    ];
    (1..=16).contains(&i).then(|| T[i as usize - 1])
}

/// A COLORREF (`r, g, b, fAuto`).
pub fn colorref(b: &[u8]) -> Color {
    match b {
        [_, _, _, 0xFF, ..] => None,
        [r, g, bl, ..] => Some([*r, *g, *bl]),
        _ => None,
    }
}

/// Character properties. Toggles keep their raw operand (0 off, 1 on,
/// 0x80 as the style, 0x81 the opposite of the style).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chp {
    pub bold: Option<u8>,
    pub italic: Option<u8>,
    pub strike: Option<u8>,
    pub dstrike: Option<u8>,
    pub caps: Option<u8>,
    pub small_caps: Option<u8>,
    pub vanish: Option<u8>,
    /// `kul` underline kind (0 none).
    pub underline: Option<u8>,
    /// Size in half-points.
    pub size: Option<u16>,
    pub color: Option<Color>,
    /// `ico` highlight (0 none).
    pub highlight: Option<u8>,
    /// 0 normal, 1 superscript, 2 subscript.
    pub iss: Option<u8>,
    /// Font indexes: ASCII, East Asian, other (hAnsi), complex script.
    pub fonts: [Option<u16>; 4],
    /// Character style.
    pub istd: Option<u16>,
    pub lang: Option<u16>,
    /// A special character (picture, note reference, field mark…).
    pub special: bool,
    /// Picture location in the Data stream.
    pub pic: Option<u32>,
    /// Form field / object data, not a picture.
    pub data: bool,
    pub ole2: bool,
    pub deleted: bool,
    /// A symbol character (`sprmCSymbol`): font index and character.
    pub symbol: Option<(u16, u16)>,
}

impl Chp {
    pub fn apply(&mut self, g: &[u8]) {
        for (id, op) in sprms(g) {
            let b = op.first().copied().unwrap_or(0);
            let w = u16_at(op, 0);
            match id {
                0x0835 => self.bold = Some(b),
                0x0836 => self.italic = Some(b),
                0x0837 => self.strike = Some(b),
                0x2A53 => self.dstrike = Some(b),
                0x083B => self.caps = Some(b),
                0x083A => self.small_caps = Some(b),
                0x083C => self.vanish = Some(b),
                0x2A3E => self.underline = Some(b),
                0x4A43 => self.size = w,
                0x2A42 => self.color = Some(ico(b)),
                0x6870 => self.color = Some(colorref(op)),
                0x2A0C => self.highlight = Some(b),
                0x2A48 => self.iss = Some(b),
                0x4A4F => self.fonts[0] = w,
                0x4A50 => self.fonts[1] = w,
                0x4A51 => self.fonts[2] = w,
                0x4A5E => self.fonts[3] = w,
                0x4A30 => self.istd = w,
                0x486D | 0x4873 => self.lang = w,
                0x0855 => self.special = b != 0,
                0x6A03 => self.pic = u32_at(op, 0),
                0x0806 => self.data = b != 0,
                0x080A => self.ole2 = b != 0,
                0x0800 => self.deleted = b != 0,
                0x6A09 => {
                    if let (Some(f), Some(c)) = (u16_at(op, 0), u16_at(op, 2)) {
                        self.symbol = Some((f, c));
                    }
                }
                _ => {}
            }
        }
    }

    /// Properties that only matter to the importer, cleared so runs merge.
    pub fn visible(&self) -> Chp {
        Chp {
            special: false,
            pic: None,
            data: false,
            ole2: false,
            symbol: None,
            ..self.clone()
        }
    }
}

/// A tab stop: position, alignment (`jc`) and leader.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tab {
    pub pos: i16,
    pub jc: u8,
    pub leader: u8,
    /// A stop the style has that this paragraph removes.
    pub clear: bool,
}

/// Paragraph properties (plus the table row's, on row-end paragraphs).
#[derive(Clone, Debug, Default)]
pub struct Pap {
    pub istd: u16,
    pub jc: Option<u8>,
    pub left: Option<i32>,
    pub right: Option<i32>,
    pub first: Option<i32>,
    pub before: Option<u16>,
    pub after: Option<u16>,
    /// `dyaLine`, `fMultLinespace`.
    pub line: Option<(i16, i16)>,
    pub keep: Option<bool>,
    pub keep_next: Option<bool>,
    pub page_break_before: Option<bool>,
    pub widow: Option<bool>,
    pub contextual: Option<bool>,
    pub outline: Option<u8>,
    pub in_table: bool,
    pub ttp: bool,
    pub itap: Option<i32>,
    pub inner_cell: bool,
    pub inner_ttp: bool,
    pub ilfo: Option<u16>,
    pub ilvl: Option<u8>,
    pub tabs: Vec<Tab>,
    /// The Data-stream offset of a grpprl too big for the FKP.
    pub huge: Option<u32>,
    pub tap: Tap,
}

impl Pap {
    pub fn apply(&mut self, g: &[u8]) {
        for (id, op) in sprms(g) {
            let b = op.first().copied().unwrap_or(0);
            let w = u16_at(op, 0);
            let s = i16_at(op, 0).map(i32::from);
            match id {
                0x4600 => self.istd = w.unwrap_or(0),
                0x2403 | 0x2461 => self.jc = Some(b),
                0x2405 => self.keep = Some(b != 0),
                0x2406 => self.keep_next = Some(b != 0),
                0x2407 => self.page_break_before = Some(b != 0),
                0x2431 => self.widow = Some(b != 0),
                0x246D => self.contextual = Some(b != 0),
                0x2640 => self.outline = Some(b),
                0x260A => self.ilvl = Some(b),
                0x460B => self.ilfo = w,
                0x840E | 0x845D => self.right = s,
                0x840F | 0x845E => self.left = s,
                0x8411 | 0x8460 => self.first = s,
                0xA413 => self.before = w,
                0xA414 => self.after = w,
                0x6412 => {
                    if let (Some(a), Some(m)) = (i16_at(op, 0), i16_at(op, 2)) {
                        self.line = Some((a, m));
                    }
                }
                0x2416 => self.in_table = b != 0,
                0x2417 => self.ttp = b != 0,
                0x6649 => self.itap = i32_at(op, 0),
                0x244B => self.inner_cell = b != 0,
                0x244C => self.inner_ttp = b != 0,
                0x6646 => self.huge = u32_at(op, 0),
                0xC60D | 0xC615 => self.change_tabs(op, id == 0xC615),
                _ if id >> 10 & 7 == 5 => self.tap.apply_one(id, op),
                _ => {}
            }
        }
    }

    fn change_tabs(&mut self, op: &[u8], with_close: bool) {
        // cb, then deletions (positions, plus close widths in sprmPChgTabs),
        // then additions (positions, then TBDs).
        let mut i = 1;
        let del = *op.get(i).unwrap_or(&0) as usize;
        i += 1;
        for k in 0..del {
            if let Some(pos) = i16_at(op, i + k * 2) {
                self.tabs.retain(|t| t.pos != pos);
                self.tabs.push(Tab {
                    pos,
                    jc: 0,
                    leader: 0,
                    clear: true,
                });
            }
        }
        i += del * if with_close { 4 } else { 2 };
        let add = *op.get(i).unwrap_or(&0) as usize;
        i += 1;
        for k in 0..add {
            let (Some(pos), Some(&tbd)) = (i16_at(op, i + k * 2), op.get(i + add * 2 + k)) else {
                break;
            };
            self.tabs.retain(|t| t.pos != pos);
            self.tabs.push(Tab {
                pos,
                jc: tbd & 7,
                leader: (tbd >> 3) & 7,
                clear: false,
            });
        }
        self.tabs.sort_by_key(|t| t.pos);
    }
}

/// One border: `brcType`, width in eighths of a point, colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Brd {
    pub kind: u8,
    pub size: u8,
    pub color: Color,
}

/// A Brc80 (4 bytes); `None` for "nil" (all 0xFF).
pub fn brc80(b: &[u8]) -> Option<Brd> {
    let b: [u8; 4] = b.get(..4)?.try_into().ok()?;
    if b == [0xFF; 4] {
        return None;
    }
    Some(Brd {
        kind: b[1],
        size: b[0],
        color: ico(b[2]),
    })
}

/// A Brc (8 bytes).
pub fn brc(b: &[u8]) -> Option<Brd> {
    let b: [u8; 8] = b.get(..8)?.try_into().ok()?;
    if b == [0xFF; 8] {
        return None;
    }
    Some(Brd {
        kind: b[5],
        size: b[4],
        color: colorref(&b[..4]),
    })
}

/// One cell of a row definition (TC80).
#[derive(Clone, Debug, Default)]
pub struct Tc {
    pub first_merged: bool,
    pub merged: bool,
    pub vert_merge: bool,
    pub vert_restart: bool,
    /// 0 top, 1 centre, 2 bottom.
    pub valign: u8,
    /// Top, left, bottom, right.
    pub borders: [Option<Brd>; 4],
    pub fill: Color,
}

/// Table row properties (TAP), from the row-end paragraph's sprms.
#[derive(Clone, Debug, Default)]
pub struct Tap {
    /// Cell boundaries in twips (`rgdxaCenter`), one more than cells.
    pub centers: Vec<i16>,
    pub cells: Vec<Tc>,
    pub gap_half: Option<i16>,
    pub jc: Option<u8>,
    pub height: Option<i16>,
    pub header: bool,
    pub cant_split: bool,
    /// Top, left, bottom, right, inside horizontal, inside vertical.
    pub borders: [Option<Brd>; 6],
    /// Default cell padding: top, left, bottom, right.
    pub padding: [Option<u16>; 4],
}

impl Tap {
    fn apply_one(&mut self, id: u16, op: &[u8]) {
        let b = op.first().copied().unwrap_or(0);
        match id {
            0xD608 => self.def_table(op),
            0x9602 => self.gap_half = i16_at(op, 0),
            0x5400 | 0x548A => self.jc = Some(b),
            0x9407 => self.height = i16_at(op, 0),
            0x3404 => self.header = b != 0,
            0x3403 | 0x3644 => self.cant_split = b != 0,
            0xD605 => {
                for (k, side) in self.borders.iter_mut().enumerate() {
                    if let Some(x) = op.get(1 + k * 4..) {
                        *side = brc80(x);
                    }
                }
            }
            0xD613 => {
                // cb, then six Brcs.
                for (k, side) in self.borders.iter_mut().enumerate() {
                    if let Some(x) = op.get(1 + k * 8..) {
                        *side = brc(x);
                    }
                }
            }
            0xD620 | 0xD62F => {
                // cb, itcFirst, itcLim, bordersToApply, border.
                let (Some(&f), Some(&l), Some(&which)) = (op.get(1), op.get(2), op.get(3)) else {
                    return;
                };
                let brd = if id == 0xD620 {
                    op.get(4..).and_then(brc80)
                } else {
                    op.get(4..).and_then(brc)
                };
                for c in self.cells.iter_mut().take(l as usize).skip(f as usize) {
                    for (bit, side) in [(1, 0), (2, 1), (4, 2), (8, 3)] {
                        if which & bit != 0 {
                            c.borders[side] = brd;
                        }
                    }
                }
            }
            0xD62C => {
                let (Some(&f), Some(&l), Some(&v)) = (op.get(1), op.get(2), op.get(3)) else {
                    return;
                };
                for c in self.cells.iter_mut().take(l as usize).skip(f as usize) {
                    c.valign = v;
                }
            }
            0xD609 => {
                // SHD80s, two bytes each: icoFore, icoBack, ipat.
                let n = (b as usize) / 2;
                for k in 0..n.min(self.cells.len()) {
                    if let Some(v) = u16_at(op, 1 + k * 2) {
                        let (fore, back, pat) = (v & 0x1F, (v >> 5) & 0x1F, v >> 10);
                        self.cells[k].fill = shade(
                            ico(fore as u8),
                            ico(back as u8),
                            pat,
                            ico(back as u8).is_none(),
                        );
                    }
                }
            }
            0xD612 | 0xD616 | 0xD60C => {
                let base = match id {
                    0xD612 => 0,
                    0xD616 => 22,
                    _ => 44,
                };
                let n = (b as usize) / 10;
                for k in 0..n {
                    let at = 1 + k * 10;
                    let (Some(fore), Some(back), Some(pat)) = (
                        op.get(at..at + 4),
                        op.get(at + 4..at + 8),
                        u16_at(op, at + 8),
                    ) else {
                        break;
                    };
                    if let Some(c) = self.cells.get_mut(base + k) {
                        c.fill = shade(
                            colorref(fore),
                            colorref(back),
                            pat,
                            colorref(back).is_none(),
                        );
                    }
                }
            }
            0xD634 => {
                // CSSA: itcFirst, itcLim, sides, ftsWidth, width.
                let (Some(&sides), Some(&fts), Some(wd)) = (op.get(3), op.get(4), u16_at(op, 5))
                else {
                    return;
                };
                if fts == 3 {
                    for (bit, k) in [(1, 0), (2, 1), (4, 2), (8, 3)] {
                        if sides & bit != 0 {
                            self.padding[k] = Some(wd);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn def_table(&mut self, op: &[u8]) {
        let n = *op.first().unwrap_or(&0) as usize;
        let centers: Vec<i16> = (0..=n).map_while(|k| i16_at(op, 1 + k * 2)).collect();
        if centers.len() != n + 1 {
            return;
        }
        let tc_at = 1 + (n + 1) * 2;
        let mut cells = Vec::with_capacity(n);
        for k in 0..n {
            let at = tc_at + k * 20;
            let mut tc = Tc::default();
            if let Some(f) = u16_at(op, at) {
                tc.first_merged = f & 1 != 0;
                tc.merged = f & 2 != 0;
                tc.vert_merge = f & 0x20 != 0;
                tc.vert_restart = f & 0x40 != 0;
                tc.valign = ((f >> 7) & 3) as u8;
                // An empty TC80 border means "not set": the table's apply.
                for (s, side) in tc.borders.iter_mut().enumerate() {
                    *side = op
                        .get(at + 4 + s * 4..)
                        .and_then(brc80)
                        .filter(|b| b.kind != 0);
                }
            }
            cells.push(tc);
        }
        self.centers = centers;
        self.cells = cells;
    }
}

/// The fill a shading pattern looks like: `pat` 0 is the background, 1
/// the foreground, 2-13 a percentage of the foreground over the background.
fn shade(fore: Color, back: Color, pat: u16, back_auto: bool) -> Color {
    let pct = match pat {
        0 => return if back_auto { None } else { back },
        1 => 100,
        2 => 5,
        3 => 10,
        4 => 20,
        5 => 25,
        6 => 30,
        7 => 40,
        8 => 50,
        9 => 60,
        10 => 70,
        11 => 75,
        12 => 80,
        13 => 90,
        // Stripes and other patterns: Word's 35-62 are more percentages.
        35..=62 => (pat as u32 - 32) * 25 / 10,
        _ => return back,
    };
    let f = fore.unwrap_or([0, 0, 0]);
    let b = back.unwrap_or([0xFF; 3]);
    let mix = |i: usize| ((f[i] as u32 * pct + b[i] as u32 * (100 - pct)) / 100) as u8;
    Some([mix(0), mix(1), mix(2)])
}

/// Section properties, with Word 97's defaults (US Letter, 1.25" side
/// margins, 1" top and bottom).
#[derive(Clone, Debug)]
pub struct Sep {
    pub width: u32,
    pub height: u32,
    pub left: u32,
    pub right: u32,
    pub top: i32,
    pub bottom: i32,
    pub header: u32,
    pub footer: u32,
    pub gutter: u32,
    pub landscape: bool,
    /// Break kind: 0 continuous, 1 column, 2 new page, 3 even, 4 odd.
    pub bkc: u8,
    pub title_page: bool,
    pub columns: u16,
    pub column_space: u32,
    pub pgn_start: Option<u16>,
    pub pgn_restart: bool,
}

impl Default for Sep {
    fn default() -> Self {
        Self {
            width: 12240,
            height: 15840,
            left: 1800,
            right: 1800,
            top: 1440,
            bottom: 1440,
            header: 720,
            footer: 720,
            gutter: 0,
            landscape: false,
            bkc: 2,
            title_page: false,
            columns: 1,
            column_space: 720,
            pgn_start: None,
            pgn_restart: false,
        }
    }
}

impl Sep {
    pub fn apply(&mut self, g: &[u8]) {
        for (id, op) in sprms(g) {
            let b = op.first().copied().unwrap_or(0);
            let w = u16_at(op, 0).map(u32::from);
            let s = i16_at(op, 0).map(i32::from);
            match id {
                0xB01F => self.width = w.unwrap_or(self.width),
                0xB020 => self.height = w.unwrap_or(self.height),
                0xB021 => self.left = w.unwrap_or(self.left),
                0xB022 => self.right = w.unwrap_or(self.right),
                0x9023 => self.top = s.unwrap_or(self.top),
                0x9024 => self.bottom = s.unwrap_or(self.bottom),
                0xB017 => self.header = w.unwrap_or(self.header),
                0xB018 => self.footer = w.unwrap_or(self.footer),
                0xB025 => self.gutter = w.unwrap_or(0),
                0x301D => self.landscape = b == 2,
                0x3009 => self.bkc = b,
                0x300A => self.title_page = b != 0,
                0x500B => self.columns = u16_at(op, 0).unwrap_or(0).saturating_add(1),
                0x900C => self.column_space = w.unwrap_or(self.column_space),
                0x501C => self.pgn_start = u16_at(op, 0),
                0x3011 => self.pgn_restart = b != 0,
                _ => {}
            }
        }
        // Keep the page sane whatever the file says.
        self.width = self.width.clamp(1440, 31680 * 2);
        self.height = self.height.clamp(1440, 31680 * 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn walks_sprms_of_every_size() {
        // bold on, size 24, colour, a variable-length sprm, a truncated one.
        let g = [
            0x35, 0x08, 0x01, 0x43, 0x4A, 0x18, 0x00, 0x70, 0x68, 1, 2, 3, 0, 0x15, 0xC6, 0x02,
            0xAA, 0xBB, 0x43, 0x4A, 0x01,
        ];
        let s = sprms(&g);
        assert_eq!(s.len(), 4);
        assert_eq!(s[0], (0x0835, &[1u8][..]));
        assert_eq!(s[1], (0x4A43, &[0x18, 0][..]));
        assert_eq!(s[2], (0x6870, &[1, 2, 3, 0][..]));
        assert_eq!(s[3], (0xC615, &[0x02, 0xAA, 0xBB][..]));
        let mut c = Chp::default();
        c.apply(&g);
        assert_eq!(c.bold, Some(1));
        assert_eq!(c.size, Some(24));
        assert_eq!(c.color, Some(Some([1, 2, 3])));
    }

    #[std::prelude::v1::test]
    fn shading_mixes_colours() {
        assert_eq!(shade(None, None, 0, true), None);
        assert_eq!(shade(None, None, 8, true), Some([0x7F; 3]));
        assert_eq!(
            shade(Some([0, 0, 0]), Some([1, 2, 3]), 0, false),
            Some([1, 2, 3])
        );
    }
}
