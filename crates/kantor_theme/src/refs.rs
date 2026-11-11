//! Colours for the references of a formula being typed (cell outline and
//! formula text), picked from a theme's palette.
//!
//! Many themes make two palette colours the same: every Ghostty scheme's
//! accent is its blue, and so is Catppuccin's, so taking `accent, blue, …`
//! as they are gave the first two references one colour. Here each candidate
//! is first made readable on the background (WCAG contrast ≥ [`MIN_CONTRAST`]),
//! must not be grey, and is kept only if it differs in hue and in OKLab from
//! those already taken.
//! When a palette runs out (monochrome themes), hues from a colour wheel fill
//! the remaining slots.

use crate::{Palette, Rgb, mix};

/// How many reference colours there are; slot `n` uses colour `n % REF_COLORS`.
pub const REF_COLORS: usize = 6;

/// Minimum WCAG contrast of a reference colour on the background.
pub const MIN_CONTRAST: f32 = 3.0;

/// Minimum OKLab distance between two reference colours.
pub const MIN_DISTANCE: f32 = 0.09;

/// Hue differences (degrees, OKLab) tried between two reference colours: the
/// first that gives every slot a colour wins. Crowded palettes on coloured
/// backgrounds (Scabos Sand) need the smaller one.
pub const HUE_GAPS: [f32; 2] = [30.0, 25.0];

/// Minimum OKLab chroma of a reference colour (below it reads as grey).
pub const MIN_CHROMA: f32 = 0.05;

fn channel(c: Rgb, shift: u32) -> f32 {
    ((c >> shift) & 0xff) as f32 / 255.0
}

fn to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn from_linear(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn linear(c: Rgb) -> [f32; 3] {
    [
        to_linear(channel(c, 16)),
        to_linear(channel(c, 8)),
        to_linear(channel(c, 0)),
    ]
}

/// WCAG relative luminance.
pub fn relative_luminance(c: Rgb) -> f32 {
    let [r, g, b] = linear(c);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// WCAG contrast ratio of two colours (1..=21).
pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The colour in OKLab (`L`, `a`, `b`).
pub fn oklab(c: Rgb) -> [f32; 3] {
    let [r, g, b] = linear(c);
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

fn from_oklab([l, a, b]: [f32; 3]) -> Rgb {
    let l_ = (l + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m_ = (l - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s_ = (l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    let r = 4.076_741_7 * l_ - 3.307_711_6 * m_ + 0.230_969_94 * s_;
    let g = -1.268_438 * l_ + 2.609_757_4 * m_ - 0.341_319_38 * s_;
    let bl = -0.004_196_086_3 * l_ - 0.703_418_6 * m_ + 1.707_614_7 * s_;
    let q = |v: f32| (from_linear(v) * 255.0).round() as u32;
    (q(r) << 16) | (q(g) << 8) | q(bl)
}

/// OKLab distance between two colours.
pub fn distance(a: Rgb, b: Rgb) -> f32 {
    let (a, b) = (oklab(a), oklab(b));
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// `c` made lighter or darker (whichever the background allows more
/// contrast with) until it reaches [`MIN_CONTRAST`] on `bg`. Lightness moves
/// in OKLab with the hue and chroma kept, so the colour stays a colour.
fn readable(c: Rgb, bg: Rgb) -> Rgb {
    if contrast(c, bg) >= MIN_CONTRAST {
        return c;
    }
    let lighter = contrast(0xffffff, bg) >= contrast(0x000000, bg);
    let [l, a, b] = oklab(c);
    let step = if lighter { 0.02 } else { -0.02 };
    (1..=50)
        .map(|i| from_oklab([(l + step * i as f32).clamp(0.0, 1.0), a, b]))
        .find(|&m| contrast(m, bg) >= MIN_CONTRAST)
        .unwrap_or_else(|| {
            let target = if lighter { 0xffffff } else { 0x000000 };
            (1..=20)
                .map(|i| mix(c, target, i as f32 * 0.05))
                .find(|&m| contrast(m, bg) >= MIN_CONTRAST)
                .unwrap_or(target)
        })
}

/// A hue wheel of saturated colours, light on dark backgrounds and dark on
/// light ones, for palettes without enough distinct colours. Strongly
/// coloured backgrounds may need the other lightness levels too.
fn wheel(bg: Rgb) -> impl Iterator<Item = Rgb> {
    let dark = relative_luminance(bg) < 0.18;
    let levels: [f32; 3] = if dark {
        [0.76, 0.9, 0.62]
    } else {
        [0.55, 0.42, 0.32]
    };
    // Hues (degrees) far apart first, then the steps between them.
    const HUES: [f32; 16] = [
        255.0, 55.0, 145.0, 330.0, 195.0, 95.0, 295.0, 15.0, 225.0, 120.0, 170.0, 75.0, 35.0,
        350.0, 275.0, 310.0,
    ];
    levels.into_iter().flat_map(|lightness| {
        HUES.into_iter().map(move |h: f32| {
            let (s, c) = h.to_radians().sin_cos();
            from_oklab([lightness, 0.13 * c, 0.13 * s])
        })
    })
}

/// OKLab chroma and hue (degrees).
fn chroma_hue(c: Rgb) -> (f32, f32) {
    let [_, a, b] = oklab(c);
    (a.hypot(b), b.atan2(a).to_degrees().rem_euclid(360.0))
}

/// Whether two colours tell apart as references: far enough in OKLab and,
/// both being colourful, in hue (two reds of different lightness look like
/// one colour next to text).
fn distinct(a: Rgb, b: Rgb, hue_gap: f32) -> bool {
    let (ha, hb) = (chroma_hue(a).1, chroma_hue(b).1);
    let dh = (ha - hb).abs();
    distance(a, b) >= MIN_DISTANCE && dh.min(360.0 - dh) >= hue_gap
}

/// The candidates that pass, in order (at most [`REF_COLORS`]).
fn pick(p: &Palette, hue_gap: f32) -> Vec<Rgb> {
    let bg = p.background;
    let palette = [
        p.accent, p.blue, p.green, p.yellow, 0xf47a3a, p.magenta, p.cyan, p.red,
    ];
    let mut chosen: Vec<Rgb> = Vec::with_capacity(REF_COLORS);
    for c in palette.into_iter().chain(wheel(bg)) {
        if chosen.len() == REF_COLORS {
            break;
        }
        let c = readable(c, bg);
        // Greys don't read as a colour; monochrome themes get the wheel.
        if chroma_hue(c).0 >= MIN_CHROMA && chosen.iter().all(|&o| distinct(o, c, hue_gap)) {
            chosen.push(c);
        }
    }
    chosen
}

/// The reference colours for a palette: readable on its background and
/// distinct from each other. The first ones are the palette's accent, blue,
/// green and yellow when those differ, as before.
pub fn reference_colors(p: &Palette) -> [Rgb; REF_COLORS] {
    let mut chosen = HUE_GAPS
        .iter()
        .map(|&gap| pick(p, gap))
        .find(|c| c.len() == REF_COLORS)
        .unwrap_or_else(|| pick(p, HUE_GAPS[HUE_GAPS.len() - 1]));
    // The wheel always fills the slots (tested over every bundled theme), but
    // never panic over a theme.
    chosen.resize(REF_COLORS, p.foreground);
    let mut out = [0; REF_COLORS];
    out.copy_from_slice(&chosen);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{ghostty_themes, omarchy_themes};

    const CATPPUCCIN: &str = r##"
mode = "dark"
accent = "#89b4fa"
selection = "#45475a"
muted = "#585b70"
background = "#1e1e2e"
dark_background = "#161622"
lighter_background = "#313244"
foreground = "#cdd6f4"
dark_foreground = "#6c7086"
red = "#f38ba8"
yellow = "#f9e2af"
green = "#a6e3a1"
cyan = "#94e2d5"
blue = "#89b4fa"
magenta = "#f5c2e7"
"##;

    fn check(name: &str, p: &Palette) {
        let gap = HUE_GAPS[HUE_GAPS.len() - 1];
        assert_eq!(pick(p, gap).len(), REF_COLORS, "{name}: ran out of colours");
        let refs = reference_colors(p);
        for (i, &a) in refs.iter().enumerate() {
            let ratio = contrast(a, p.background);
            assert!(
                ratio >= MIN_CONTRAST - 0.01,
                "{name}: slot {i} {a:06x} contrast {ratio} on {:06x}",
                p.background
            );
            assert!(
                chroma_hue(a).0 >= MIN_CHROMA,
                "{name}: slot {i} {a:06x} is grey"
            );
            for (j, &b) in refs.iter().enumerate().skip(i + 1) {
                assert!(
                    distinct(a, b, gap),
                    "{name}: slots {i} {a:06x} and {j} {b:06x} too close"
                );
            }
        }
    }

    #[test]
    fn oklab_round_trips() {
        for c in [0x000000, 0xffffff, 0x89b4fa, 0xf47a3a, 0x1e1e2e, 0x40a02b] {
            let back = from_oklab(oklab(c));
            assert!(distance(c, back) < 0.005, "{c:06x} -> {back:06x}");
        }
    }

    #[test]
    fn catppuccin_first_two_differ() {
        let p = Palette::from_colors_toml(CATPPUCCIN).unwrap();
        let refs = reference_colors(&p);
        assert_eq!(refs[0], 0x89b4fa, "the accent stays first");
        assert_ne!(refs[1], refs[0]);
        check("catppuccin", &p);
    }

    #[test]
    fn every_bundled_theme_has_distinct_readable_refs() {
        let ghostty = ghostty_themes();
        assert!(ghostty.len() > 900);
        // Omarchy themes are only there on Omarchy machines; check them too.
        for theme in ghostty.iter().chain(omarchy_themes().iter()) {
            check(&theme.name, &theme.palette);
        }
        check("default", &Palette::default());
    }
}
