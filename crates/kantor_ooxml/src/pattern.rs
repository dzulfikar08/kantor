//! DrawingML pattern fills (`a:pattFill prst="…"`, ECMA-376 §20.1.10.51):
//! the 8 × 8 bitmaps of the presets, as Office draws them (the GDI+ hatch
//! styles they come from), and their names in Office's Pattern gallery.
//!
//! A bitmap is eight rows, top to bottom; in each row's byte bit `i`
//! (least significant first) is column `i` from the left. A set bit is
//! drawn in the foreground colour, a clear one in the background colour.

/// One preset: `prst` value, the name Office shows, its bitmap.
pub struct Preset {
    pub prst: &'static str,
    pub name: &'static str,
    pub bits: [u8; 8],
}

const fn p(prst: &'static str, name: &'static str, bits: [u8; 8]) -> Preset {
    Preset { prst, name, bits }
}

/// Every preset, in the order of Office's Pattern gallery (six rows of
/// eight, then the six Office keeps out of it).
pub const PRESETS: &[Preset] = &[
    p(
        "pct5",
        "5%",
        [0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x80],
    ),
    p(
        "pct10",
        "10%",
        [0x00, 0x08, 0x00, 0x80, 0x00, 0x08, 0x00, 0x80],
    ),
    p(
        "pct20",
        "20%",
        [0x00, 0x22, 0x00, 0x88, 0x00, 0x22, 0x00, 0x88],
    ),
    p(
        "pct25",
        "25%",
        [0x22, 0x88, 0x22, 0x88, 0x22, 0x88, 0x22, 0x88],
    ),
    p(
        "pct30",
        "30%",
        [0x11, 0xaa, 0x44, 0xaa, 0x11, 0xaa, 0x44, 0xaa],
    ),
    p(
        "pct40",
        "40%",
        [0x15, 0xaa, 0x55, 0xaa, 0x51, 0xaa, 0x55, 0xaa],
    ),
    p(
        "pct50",
        "50%",
        [0x55, 0xaa, 0x55, 0xaa, 0x55, 0xaa, 0x55, 0xaa],
    ),
    p(
        "pct60",
        "60%",
        [0x55, 0xbb, 0x55, 0xee, 0x55, 0xbb, 0x55, 0xee],
    ),
    p(
        "pct70",
        "70%",
        [0xdd, 0x77, 0xdd, 0x77, 0xdd, 0x77, 0xdd, 0x77],
    ),
    p(
        "pct75",
        "75%",
        [0xff, 0xdd, 0xff, 0x77, 0xff, 0xdd, 0xff, 0x77],
    ),
    p(
        "pct80",
        "80%",
        [0xff, 0xfe, 0xff, 0xef, 0xff, 0xfe, 0xff, 0xef],
    ),
    p(
        "pct90",
        "90%",
        [0x7f, 0xff, 0xff, 0xff, 0xf7, 0xff, 0xff, 0xff],
    ),
    p(
        "ltDnDiag",
        "Light downward diagonal",
        [0x11, 0x22, 0x44, 0x88, 0x11, 0x22, 0x44, 0x88],
    ),
    p(
        "ltUpDiag",
        "Light upward diagonal",
        [0x88, 0x44, 0x22, 0x11, 0x88, 0x44, 0x22, 0x11],
    ),
    p(
        "dkDnDiag",
        "Dark downward diagonal",
        [0x99, 0x33, 0x66, 0xcc, 0x99, 0x33, 0x66, 0xcc],
    ),
    p(
        "dkUpDiag",
        "Dark upward diagonal",
        [0x99, 0xcc, 0x66, 0x33, 0x99, 0xcc, 0x66, 0x33],
    ),
    p(
        "wdDnDiag",
        "Wide downward diagonal",
        [0x83, 0x07, 0x0e, 0x1c, 0x38, 0x70, 0xe0, 0xc1],
    ),
    p(
        "wdUpDiag",
        "Wide upward diagonal",
        [0xc1, 0xe0, 0x70, 0x38, 0x1c, 0x0e, 0x07, 0x83],
    ),
    p("ltVert", "Light vertical", [0x88; 8]),
    p(
        "ltHorz",
        "Light horizontal",
        [0x00, 0x00, 0x00, 0xff, 0x00, 0x00, 0x00, 0xff],
    ),
    p("narVert", "Narrow vertical", [0x55; 8]),
    p(
        "narHorz",
        "Narrow horizontal",
        [0x00, 0xff, 0x00, 0xff, 0x00, 0xff, 0x00, 0xff],
    ),
    p("dkVert", "Dark vertical", [0xcc; 8]),
    p(
        "dkHorz",
        "Dark horizontal",
        [0x00, 0x00, 0xff, 0xff, 0x00, 0x00, 0xff, 0xff],
    ),
    p(
        "dashDnDiag",
        "Dashed downward diagonal",
        [0x00, 0x00, 0x11, 0x22, 0x44, 0x88, 0x00, 0x00],
    ),
    p(
        "dashUpDiag",
        "Dashed upward diagonal",
        [0x00, 0x00, 0x88, 0x44, 0x22, 0x11, 0x00, 0x00],
    ),
    p(
        "dashHorz",
        "Dashed horizontal",
        [0x00, 0x00, 0x00, 0x0f, 0x00, 0x00, 0x00, 0xf0],
    ),
    p(
        "dashVert",
        "Dashed vertical",
        [0x08, 0x08, 0x08, 0x08, 0x80, 0x80, 0x80, 0x80],
    ),
    p(
        "smConfetti",
        "Small confetti",
        [0x04, 0x20, 0x01, 0x10, 0x02, 0x40, 0x08, 0x80],
    ),
    p(
        "lgConfetti",
        "Large confetti",
        [0x8d, 0x0c, 0xc0, 0xd8, 0x1b, 0x03, 0x30, 0xb1],
    ),
    p(
        "zigZag",
        "Zig zag",
        [0x18, 0x24, 0x42, 0x81, 0x18, 0x24, 0x42, 0x81],
    ),
    p(
        "wave",
        "Wave",
        [0xc0, 0x25, 0x18, 0x00, 0xc0, 0x25, 0x18, 0x00],
    ),
    p(
        "diagBrick",
        "Diagonal brick",
        [0x81, 0x42, 0x24, 0x18, 0x08, 0x04, 0x02, 0x01],
    ),
    p(
        "horzBrick",
        "Horizontal brick",
        [0x08, 0x08, 0x08, 0xff, 0x80, 0x80, 0x80, 0xff],
    ),
    p(
        "weave",
        "Weave",
        [0x51, 0x22, 0x14, 0x88, 0x45, 0x22, 0x54, 0x88],
    ),
    p(
        "plaid",
        "Plaid",
        [0xf0, 0xf0, 0xf0, 0xf0, 0x55, 0xaa, 0x55, 0xaa],
    ),
    p(
        "divot",
        "Divot",
        [0x80, 0x01, 0x80, 0x00, 0x10, 0x08, 0x10, 0x00],
    ),
    p(
        "dotGrid",
        "Dotted grid",
        [0x00, 0x80, 0x00, 0x80, 0x00, 0x80, 0x00, 0xaa],
    ),
    p(
        "dotDmnd",
        "Dotted diamond",
        [0x00, 0x22, 0x00, 0x08, 0x00, 0x22, 0x00, 0x80],
    ),
    p(
        "shingle",
        "Shingle",
        [0x01, 0x01, 0x02, 0x0c, 0x30, 0x48, 0x84, 0x03],
    ),
    p(
        "trellis",
        "Trellis",
        [0x99, 0xff, 0x66, 0xff, 0x99, 0xff, 0x66, 0xff],
    ),
    p(
        "sphere",
        "Sphere",
        [0xf8, 0xf8, 0x98, 0x77, 0x8f, 0x8f, 0x89, 0x77],
    ),
    p(
        "smGrid",
        "Small grid",
        [0x88, 0x88, 0x88, 0xff, 0x88, 0x88, 0x88, 0xff],
    ),
    p(
        "lgGrid",
        "Large grid",
        [0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0xff],
    ),
    p(
        "smCheck",
        "Small checker board",
        [0x99, 0x66, 0x66, 0x99, 0x99, 0x66, 0x66, 0x99],
    ),
    p(
        "lgCheck",
        "Large checker board",
        [0x0f, 0x0f, 0x0f, 0x0f, 0xf0, 0xf0, 0xf0, 0xf0],
    ),
    p(
        "openDmnd",
        "Outlined diamond",
        [0x01, 0x82, 0x44, 0x28, 0x10, 0x28, 0x44, 0x82],
    ),
    p(
        "solidDmnd",
        "Solid diamond",
        [0x00, 0x10, 0x38, 0x7c, 0xfe, 0x7c, 0x38, 0x10],
    ),
    // Not in Office's gallery (kept from older versions).
    p(
        "horz",
        "Horizontal",
        [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff],
    ),
    p("vert", "Vertical", [0x80; 8]),
    p(
        "dnDiag",
        "Downward diagonal",
        [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80],
    ),
    p(
        "upDiag",
        "Upward diagonal",
        [0x80, 0x40, 0x20, 0x10, 0x08, 0x04, 0x02, 0x01],
    ),
    p(
        "cross",
        "Cross",
        [0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0xff],
    ),
    p(
        "diagCross",
        "Diagonal cross",
        [0x81, 0x42, 0x24, 0x18, 0x18, 0x24, 0x42, 0x81],
    ),
];

/// How many presets Office's gallery shows (the first ones of [`PRESETS`]).
pub const GALLERY: usize = 48;

/// The preset called `prst`.
pub fn preset(prst: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.prst == prst)
}

/// The bitmap of `prst`; unknown values draw as `pct5` (the schema's
/// default when `prst` is missing).
pub fn bits(prst: &str) -> [u8; 8] {
    preset(prst).unwrap_or(&PRESETS[0]).bits
}

/// Whether the bit at column `x`, row `y` (both wrap at 8) is foreground.
pub fn on(bits: &[u8; 8], x: usize, y: usize) -> bool {
    bits[y % 8] >> (x % 8) & 1 == 1
}

/// The share of foreground bits, 0..=1 (for a mixed colour where a
/// pattern can't be drawn).
pub fn coverage(bits: &[u8; 8]) -> f32 {
    bits.iter().map(|b| b.count_ones()).sum::<u32>() as f32 / 64.0
}

/// The preset whose bitmap is `bits` (any shift of it, or with foreground
/// and background swapped: `swapped` says so). For imports that store the
/// bitmap itself (`.xls` OfficeArt, `.ods` bitmap fills).
pub fn from_bits(bits: &[u8; 8]) -> Option<(&'static Preset, bool)> {
    let shifted = |b: &[u8; 8], dx: u32, dy: usize| -> [u8; 8] {
        std::array::from_fn(|y| b[(y + dy) % 8].rotate_right(dx))
    };
    for inv in [false, true] {
        let b = if inv { bits.map(|r| !r) } else { *bits };
        for pr in PRESETS {
            for dy in 0..8 {
                for dx in 0..8 {
                    if shifted(&b, dx, dy) == pr.bits {
                        return Some((pr, inv));
                    }
                }
            }
        }
    }
    None
}

/// An 8 × 8 one-bit-per-pixel BMP (an OfficeArt pattern fill's bitmap,
/// as `kantor_metafile` hands DIBs on: a `BM` file) as a bitmap, its set
/// bits the darker of its two palette colours (Office's pattern bitmaps are
/// black on white; the shape's own colours replace them).
pub fn from_bmp(data: &[u8]) -> Option<[u8; 8]> {
    let u32_at = |i: usize| Some(u32::from_le_bytes(data.get(i..i + 4)?.try_into().ok()?));
    let i32_at = |i: usize| u32_at(i).map(|v| v as i32);
    if data.get(..2)? != b"BM" {
        return None;
    }
    let pixels = u32_at(10)? as usize;
    let header = u32_at(14)? as usize;
    let (w, h) = (i32_at(18)?, i32_at(22)?);
    let bpp = u16::from_le_bytes(data.get(28..30)?.try_into().ok()?);
    if w != 8 || h.abs() != 8 || bpp != 1 {
        return None;
    }
    let pal = 14 + header;
    let lum = |k: usize| -> Option<u32> {
        let p = data.get(pal + 4 * k..pal + 4 * k + 3)?;
        Some(p[0] as u32 * 11 + p[1] as u32 * 59 + p[2] as u32 * 30)
    };
    // Set bits: the darker colour's pixels.
    let dark_is_one = lum(1)? < lum(0)?;
    let mut out = [0u8; 8];
    for (row, o) in out.iter_mut().enumerate() {
        // Rows are bottom-up unless the height is negative, 4 bytes each.
        let src = if h > 0 { 7 - row } else { row };
        let b = *data.get(pixels + src * 4)?;
        // Most significant bit = leftmost pixel; ours is the least.
        let b = if dark_is_one { b } else { !b };
        *o = b.reverse_bits();
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_presets_are_distinct_and_named() {
        assert_eq!(PRESETS.len(), 54, "ST_PresetPatternVal has 54 values");
        for (i, a) in PRESETS.iter().enumerate() {
            for b in &PRESETS[i + 1..] {
                assert_ne!(a.prst, b.prst);
            }
        }
        assert_eq!(bits("nope"), bits("pct5"));
        assert!(on(&bits("ltVert"), 3, 5) && !on(&bits("ltVert"), 2, 5));
        assert_eq!(coverage(&bits("pct50")), 0.5);
    }

    #[test]
    fn office_pattern_bitmaps_decode() {
        // A bottom-up 8 × 8 1-bpp BMP, palette black (0) and white (1), of
        // `ltUpDiag` with black pixels set.
        let bits = bits("ltUpDiag");
        let mut bmp = b"BM".to_vec();
        bmp.extend(0u32.to_le_bytes()); // size
        bmp.extend(0u32.to_le_bytes());
        bmp.extend(62u32.to_le_bytes()); // pixels at
        bmp.extend(40u32.to_le_bytes());
        bmp.extend(8i32.to_le_bytes());
        bmp.extend(8i32.to_le_bytes());
        bmp.extend(1u16.to_le_bytes());
        bmp.extend(1u16.to_le_bytes());
        bmp.extend([0u8; 24]);
        bmp.extend([0, 0, 0, 0, 255, 255, 255, 0]);
        for row in (0..8).rev() {
            // Black (index 0) where the bit is set.
            bmp.extend([!bits[row].reverse_bits(), 0, 0, 0]);
        }
        assert_eq!(from_bmp(&bmp), Some(bits));
        assert_eq!(
            from_bits(&from_bmp(&bmp).unwrap()).unwrap().0.prst,
            "ltUpDiag"
        );
        assert_eq!(from_bmp(b"BMnope"), None);
    }

    #[test]
    fn bitmaps_map_back_to_presets() {
        let (pr, inv) = from_bits(&bits("wdUpDiag")).unwrap();
        assert_eq!((pr.prst, inv), ("wdUpDiag", false));
        // Shifted by a row and a column.
        let b = bits("smGrid");
        let s: [u8; 8] = std::array::from_fn(|y| b[(y + 3) % 8].rotate_left(2));
        assert_eq!(from_bits(&s).unwrap().0.prst, "smGrid");
        // 90 % with the colours swapped is 5 % shifted.
        assert_eq!(
            from_bits(&bits("pct90").map(|r| !r)).unwrap().0.prst,
            "pct5"
        );
        let (pr, inv) = from_bits(&bits("trellis").map(|r| !r)).unwrap();
        assert_eq!((pr.prst, inv), ("trellis", true));
    }
}
