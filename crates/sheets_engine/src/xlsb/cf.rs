//! Conditional formatting of `.xlsb` sheets ([MS-XLSB] 2.4.34
//! `BrtBeginConditionalFormatting`, 2.4.23 `BrtBeginCFRule`, 2.4.28/43/91
//! colour scales, data bars and icon sets with their `BrtCFVO` and
//! `BrtColor` records) and the differential formats they point at
//! (`BrtDXF` in `styles.bin`: an `XFProps` list).
//!
//! The rule types and templates are the ones `.xls` uses for Excel 2007
//! (`CF12`, see `xls::cf12`); formulas are BIFF12 tokens.
//!
//! Excel 2010's rules (`BrtBeginConditionalFormatting14` … in the sheet's
//! `BrtFRTBegin` block, 2.4.24 `BrtBeginCFRule14`, 2.4.44
//! `BrtBeginDatabar14`, 2.4.92 `BrtBeginIconSet14`, 2.4.332 `BrtCFIcon`,
//! 2.4.335 `BrtCFVO14`, 2.4.338 `BrtColor14`) are read too: the extras of a
//! data bar whose Excel 2007 rule names them by GUID (`BrtCFRuleExt`) become
//! its look (lengths, border, negative colours, axis, direction), and icon
//! sets that exist only there (custom icons, 3 Stars …) become rules. Other
//! Excel 2010-only rules (their formats are `BrtDXF14`s) are skipped.

use ironcalc_base::cf_types::{
    CfRuleInput, Cfvo, ColorScaleThreshold, IconThreshold, PeriodType, TextOperator, ValueOperator,
    icon_set_icons,
};
use ironcalc_base::types::{Border, BorderItem, Color, Dxf, DxfFont, Fill, NumFmt};

use super::{color, f64_at, fill_color, u8_at, u16_at, u32_at, wide};
use crate::cf_x14::{Axis, BarLook, Direction};
use crate::xls::{border_style, rgb_color};
use crate::xls_formula::{Context, decompile12};
use crate::{CellRange, MAX_COLS, MAX_ROWS};

/// A `BrtDXF`: `fNewBorder` and unused bits (2 bytes), then `XFProps`
/// (reserved, `cprops`, then each `XFProp`: type, size of the whole
/// property, data).
pub(super) fn dxf(d: &[u8], theme: &[u32], formats: &[NumFmt]) -> Dxf {
    let count = u16_at(d, 4) as usize;
    let mut at = 6;
    let mut font = DxfFont::default();
    let (mut pattern, mut fore, mut back) = (None, None, None);
    let mut border = Border::default();
    let (mut num_id, mut num_code) = (None, None);
    for _ in 0..count {
        let ty = u16_at(d, at);
        let cb = u16_at(d, at + 2) as usize;
        let Some(v) = (cb >= 4).then(|| d.get(at + 4..at + cb)).flatten() else {
            break;
        };
        at += cb;
        let side = || {
            border_style(u8_at(v, 8)).map(|style| BorderItem {
                style,
                color: color(v, 0, theme).map_or(Color::None, rgb_color),
            })
        };
        match ty {
            0x00 => pattern = Some(u8_at(v, 0) as u32),
            0x01 => fore = Some(color(v, 0, theme)),
            0x02 => back = Some(color(v, 0, theme)),
            0x05 => font.color = color(v, 0, theme).map_or(Color::None, rgb_color),
            0x06 => border.top = side(),
            0x07 => border.bottom = side(),
            0x08 => border.left = side(),
            0x09 => border.right = side(),
            0x19 => font.b = Some(u16_at(v, 0) >= 600),
            0x1A => font.u = Some(u8_at(v, 0) != 0),
            0x1C => font.i = Some(u8_at(v, 0) != 0),
            0x1D => font.strike = Some(u8_at(v, 0) != 0),
            0x24 => {
                let twips = if v.len() >= 4 {
                    u32_at(v, 0)
                } else {
                    u16_at(v, 0) as u32
                };
                if twips > 0 {
                    font.sz = Some(((twips as f64 / 20.0).round() as i32).clamp(1, 409));
                }
            }
            0x26 => num_code = Some(format_string(v)),
            0x29 => num_id = Some(u16_at(v, 0) as i32),
            _ => {}
        }
    }
    // A conditional fill shows its background colour when solid, as an
    // `.xlsx` dxf's `bgColor` (the foreground when that is all there is).
    let (fore, back) = (fore.flatten(), back.flatten());
    let fill = match pattern {
        Some(0) => None,
        p if fore.is_some() || back.is_some() => match p.unwrap_or(1) {
            1 => back.or(fore).map(rgb_color),
            p => Some(fill_color(p, fore, back)),
        }
        .filter(|c| *c != Color::None)
        .map(|color| Fill { color }),
        _ => None,
    };
    let num_fmt = match (num_id, num_code) {
        (_, Some(code)) => Some(NumFmt {
            num_fmt_id: num_id.unwrap_or(164),
            format_code: code,
        }),
        (Some(id), None) => Some(NumFmt {
            num_fmt_id: id,
            format_code: ironcalc_base::number_format::get_num_fmt(id, formats),
        }),
        _ => None,
    };
    Dxf {
        font: (font != DxfFont::default()).then_some(font),
        fill,
        border: (border != Border::default()).then_some(border),
        num_fmt,
        alignment: None,
    }
}

/// A number format property: a count of characters (2 bytes), then the
/// characters as UTF-16, after a flags byte when one is there (the
/// `[MS-XLS]` string the spec names).
fn format_string(v: &[u8]) -> String {
    let n = u16_at(v, 0) as usize;
    let start = if v.len() == 3 + 2 * n { 3 } else { 2 };
    let units: Vec<u16> = (0..n.min(v.len().saturating_sub(start) / 2))
        .map(|i| u16_at(v, start + 2 * i))
        .collect();
    String::from_utf16_lossy(&units)
}

/// A sheet's `BrtBeginConditionalFormatting` block: its ranges and rules.
#[derive(Default)]
pub(super) struct Block {
    pub ranges: Vec<CellRange>,
    pub rules: Vec<Rule>,
}

/// A `BrtBeginCFRule` with the records inside it.
#[derive(Default)]
pub(super) struct Rule {
    ty: u32,
    template: u32,
    dxf: i32,
    priority: i32,
    param: u32,
    flags: u16,
    text: Option<String>,
    rgce1: Vec<u8>,
    rgce2: Vec<u8>,
    cfvos: Vec<RawCfvo>,
    /// `BrtColor` records, raw (resolved with the theme later).
    colors: Vec<[u8; 8]>,
    /// `BrtBeginDatabar`'s `fShowValue`.
    show_value: bool,
    /// `BrtBeginIconSet`: `iSet`, `fIcon` (icon only), `fReverse`.
    icons: Option<(u32, bool, bool)>,
    /// `BrtBeginDatabar`'s shortest and longest bar (%).
    lengths: Option<(u8, u8)>,
    /// `BrtCFRuleExt`: the GUID of the Excel 2010 rule with this one's
    /// extras.
    ext: Option<[u8; 16]>,
    /// `BrtBeginCFRule14`'s GUID.
    guid: Option<[u8; 16]>,
    /// `BrtBeginDatabar14`.
    bar14: Option<Bar14>,
    /// `BrtBeginIconSet14`'s `fCustom`, and its `BrtCFIcon`s (icon set,
    /// index; -1: no icon).
    custom_icons: bool,
    cf_icons: Vec<(i32, i32)>,
}

/// `BrtBeginDatabar14`: lengths (%), `bDirection`, `bAxisPosType` and the
/// flags (border, gradient, custom negative fill, custom negative border).
#[derive(Clone, Copy)]
struct Bar14 {
    min: u8,
    max: u8,
    direction: u8,
    axis: u8,
    flags: u16,
}

struct RawCfvo {
    kind: u32,
    num: f64,
    gte: bool,
    rgce: Vec<u8>,
}

/// Reads the conditional formatting records of a sheet part, one at a
/// time, as `apply_sheet` walks them.
#[derive(Default)]
pub(super) struct Reader {
    pub blocks: Vec<Block>,
    /// Excel 2010's (`BrtBeginConditionalFormatting14`).
    pub blocks14: Vec<Block>,
    /// Inside a `BrtBeginCFRule`.
    in_rule: bool,
    /// Inside a `BrtBeginCFRule14`.
    in_rule14: bool,
}

/// An `FRTHeader` (2.5.61): its ranges (`rgSqrefs`), formulas
/// (`rgFormulas`: `FRTParsedFormula` rgce) and where the record goes on.
fn frt_header(d: &[u8]) -> (Vec<CellRange>, Vec<Vec<u8>>, usize) {
    let flags = u32_at(d, 0);
    let mut at = 4usize;
    let mut ranges = Vec::new();
    let mut formulas = Vec::new();
    if flags & 0x1 != 0 {
        // FRTRefs: cref, then FRTRef (flags, UncheckedRfX).
        let n = u32_at(d, at) as usize;
        at = at.saturating_add(4).saturating_add(n.saturating_mul(20));
    }
    if flags & 0x2 != 0 {
        // FRTSqrefs: csqref, then FRTSqref (flags, UncheckedSqRfX).
        let n = u32_at(d, at) as usize;
        at += 4;
        for _ in 0..n.min(d.len() / 8) {
            let count = u32_at(d, at + 4) as usize;
            at += 8;
            for _ in 0..count.min(d.len() / 16) {
                let (r1, r2, c1, c2) = (
                    u32_at(d, at),
                    u32_at(d, at + 4),
                    u32_at(d, at + 8),
                    u32_at(d, at + 12),
                );
                at += 16;
                if r1 <= r2 && c1 <= c2 && r1 < MAX_ROWS && c1 < MAX_COLS {
                    ranges.push(CellRange::from_corners(
                        (r1, c1),
                        (r2.min(MAX_ROWS - 1), c2.min(MAX_COLS - 1)),
                    ));
                }
            }
        }
    }
    if flags & 0x4 != 0 {
        // FRTFormulas: cformula, then FRTFormula (flags, FRTParsedFormula:
        // cce, cb, rgce, rgcb).
        let n = u32_at(d, at) as usize;
        at += 4;
        for _ in 0..n.min(d.len() / 12) {
            let cce = u32_at(d, at + 4) as usize;
            let cb = u32_at(d, at + 8) as usize;
            let start = at + 12;
            formulas.push(
                d.get(start..start.saturating_add(cce))
                    .unwrap_or_default()
                    .to_vec(),
            );
            at = start.saturating_add(cce).saturating_add(cb);
        }
    }
    if flags & 0x8 != 0 {
        // FRTRelID: an LPWideString.
        let cch = u32_at(d, at) as usize;
        at = at.saturating_add(4).saturating_add(cch.saturating_mul(2));
    }
    (ranges, formulas, at)
}

fn guid_at(d: &[u8], o: usize) -> Option<[u8; 16]> {
    d.get(o..o + 16)?.try_into().ok()
}

impl Reader {
    /// Takes record `ty` if it is a conditional formatting one.
    pub fn record(&mut self, ty: u32, d: &[u8]) -> bool {
        match ty {
            // BrtBeginConditionalFormatting: ccf, fPivot, sqrfx.
            461 => {
                let n = u32_at(d, 8) as usize;
                let mut ranges = Vec::new();
                for i in 0..n.min(d.len().saturating_sub(12) / 16) {
                    let o = 12 + i * 16;
                    let (r1, r2, c1, c2) = (
                        u32_at(d, o),
                        u32_at(d, o + 4),
                        u32_at(d, o + 8),
                        u32_at(d, o + 12),
                    );
                    if r1 <= r2 && c1 <= c2 && r1 < MAX_ROWS && c1 < MAX_COLS {
                        ranges.push(CellRange::from_corners(
                            (r1, c1),
                            (r2.min(MAX_ROWS - 1), c2.min(MAX_COLS - 1)),
                        ));
                    }
                }
                self.blocks.push(Block {
                    ranges,
                    rules: Vec::new(),
                });
            }
            462 => self.in_rule = false,
            463 => {
                let rule = parse_rule(d);
                if let Some(b) = self.blocks.last_mut() {
                    b.rules.push(rule);
                    self.in_rule = true;
                }
            }
            464 => self.in_rule = false,
            // BrtCFRuleExt: FRTBlank, the Excel 2010 rule's GUID.
            1146 if self.in_rule => {
                let guid = guid_at(d, 4);
                if let Some(r) = self.rule() {
                    r.ext = guid;
                }
            }
            // BrtBeginConditionalFormatting14: FRTHeader (its ranges), ccf,
            // fPivot.
            1046 => {
                let (ranges, _, _) = frt_header(d);
                self.blocks14.push(Block {
                    ranges,
                    rules: Vec::new(),
                });
            }
            1047 => self.in_rule14 = false,
            // BrtBeginCFRule14: FRTHeader (the formulas), then
            // BrtBeginCFRule's fields, unused1, guid, fGuid, strParam.
            1048 => {
                let (_, formulas, at) = frt_header(d);
                let f = d.get(at..).unwrap_or_default();
                let mut rule = Rule {
                    ty: u32_at(f, 0),
                    template: u32_at(f, 4),
                    dxf: u32_at(f, 8) as i32,
                    priority: u32_at(f, 12) as i32,
                    param: u32_at(f, 16),
                    flags: u16_at(f, 28),
                    guid: (u32_at(f, 62) != 0).then(|| guid_at(f, 46)).flatten(),
                    ..Rule::default()
                };
                let mut formulas = formulas.into_iter();
                rule.rgce1 = formulas.next().unwrap_or_default();
                rule.rgce2 = formulas.next().unwrap_or_default();
                if let Some(b) = self.blocks14.last_mut() {
                    b.rules.push(rule);
                    self.in_rule14 = true;
                }
            }
            1049 => self.in_rule14 = false,
            // BrtBeginDatabar14: FRTBlank, bLenMin, bLenMax, fShowValue,
            // bDirection, bAxisPosType, flags.
            1051 if self.in_rule14 => {
                let bar = Bar14 {
                    min: u8_at(d, 4),
                    max: u8_at(d, 5),
                    direction: u8_at(d, 7),
                    axis: u8_at(d, 8),
                    flags: u16_at(d, 9),
                };
                if let Some(r) = self.rule() {
                    r.show_value = u8_at(d, 6) != 0;
                    r.bar14 = Some(bar);
                }
            }
            // BrtBeginIconSet14: FRTBlank, iSet, fCustom, fIcon, fReverse.
            1052 if self.in_rule14 => {
                let flags = u16_at(d, 8);
                if let Some(r) = self.rule() {
                    r.icons = Some((u32_at(d, 4), flags & 0x2 != 0, flags & 0x4 != 0));
                    r.custom_icons = flags & 0x1 != 0;
                }
            }
            // BrtCFIcon: FRTBlank, iconSet, index.
            1112 if self.in_rule14 => {
                let icon = (u32_at(d, 4) as i32, u32_at(d, 8) as i32);
                if let Some(r) = self.rule() {
                    r.cf_icons.push(icon);
                }
            }
            // BrtCFVO14: FRTHeader (the formula), iType, numParam, fSaveGTE,
            // fGTE, cbFmla.
            1050 if self.in_rule14 => {
                let (_, formulas, at) = frt_header(d);
                let f = d.get(at..).unwrap_or_default();
                let rgce = if u32_at(f, 20) > 0 {
                    formulas.into_iter().next().unwrap_or_default()
                } else {
                    Vec::new()
                };
                let v = RawCfvo {
                    kind: u32_at(f, 0),
                    num: f64_at(f, 4),
                    gte: u32_at(f, 16) != 0,
                    rgce,
                };
                if let Some(r) = self.rule() {
                    r.cfvos.push(v);
                }
            }
            // BrtColor14: FRTBlank, BrtColor.
            1055 if self.in_rule14 => {
                let mut c = [0u8; 8];
                for (i, b) in c.iter_mut().enumerate() {
                    *b = u8_at(d, 4 + i);
                }
                if let Some(r) = self.rule() {
                    r.colors.push(c);
                }
            }
            1050..=1052 | 1055 | 1112 | 1146 | 1155..=1157 => {}
            // BrtBeginIconSet: iSet, then fIcon (bit 1) and fReverse (bit 2).
            465 if self.in_rule => {
                let flags = u16_at(d, 4);
                if let Some(r) = self.rule() {
                    r.icons = Some((u32_at(d, 0), flags & 0x2 != 0, flags & 0x4 != 0));
                }
            }
            // BrtBeginDatabar: bLenMin, bLenMax, fShowValue.
            467 if self.in_rule => {
                if let Some(r) = self.rule() {
                    r.show_value = u8_at(d, 2) != 0;
                    r.lengths = Some((u8_at(d, 0), u8_at(d, 1)));
                }
            }
            // BrtCFVO: iType, numParam, fSaveGTE, fGTE, cbFmla, formula.
            471 if self.in_rule => {
                let cb = u32_at(d, 20);
                let rgce = if cb > 0 {
                    let cce = u32_at(d, 24) as usize;
                    d.get(28..28usize.saturating_add(cce))
                        .unwrap_or_default()
                        .to_vec()
                } else {
                    Vec::new()
                };
                let v = RawCfvo {
                    kind: u32_at(d, 0),
                    num: f64_at(d, 4),
                    gte: u32_at(d, 16) != 0,
                    rgce,
                };
                if let Some(r) = self.rule() {
                    r.cfvos.push(v);
                }
            }
            // BrtColor
            564 if self.in_rule => {
                let mut c = [0u8; 8];
                for (i, b) in c.iter_mut().enumerate() {
                    *b = u8_at(d, i);
                }
                if let Some(r) = self.rule() {
                    r.colors.push(c);
                }
            }
            465..=471 => {}
            _ => return false,
        }
        true
    }

    fn rule(&mut self) -> Option<&mut Rule> {
        let blocks = if self.in_rule14 {
            &mut self.blocks14
        } else {
            &mut self.blocks
        };
        blocks.last_mut()?.rules.last_mut()
    }
}

/// `BrtBeginCFRule`: iType, iTemplate, dxfId, iPri, iParam, two reserved,
/// flags (2 bytes), cbFmla1-3, strParam, then the formulas present
/// (`CFParsedFormula`: cce, rgce, cb, rgcb).
fn parse_rule(d: &[u8]) -> Rule {
    let mut at = 42;
    let text = if u32_at(d, at) == 0xFFFF_FFFF {
        at += 4;
        None
    } else {
        let (s, next) = wide(d, at);
        at = next;
        Some(s)
    };
    let mut formula = |present: bool| -> Vec<u8> {
        if !present {
            return Vec::new();
        }
        let cce = u32_at(d, at) as usize;
        let rgce = d
            .get(at + 4..(at + 4).saturating_add(cce))
            .unwrap_or_default()
            .to_vec();
        let cb = u32_at(d, at + 4 + cce) as usize;
        at = at.saturating_add(8 + cce + cb);
        rgce
    };
    let rgce1 = formula(u32_at(d, 30) != 0);
    let rgce2 = formula(u32_at(d, 34) != 0);
    Rule {
        ty: u32_at(d, 0),
        template: u32_at(d, 4),
        dxf: u32_at(d, 8) as i32,
        priority: u32_at(d, 12) as i32,
        param: u32_at(d, 16),
        flags: u16_at(d, 28),
        text,
        rgce1,
        rgce2,
        ..Rule::default()
    }
}

impl RawCfvo {
    fn convert(&self, ctx: &Context) -> Option<Cfvo> {
        let formula = || decompile12(&self.rgce, ctx);
        let number = || -> Option<f64> {
            if self.rgce.is_empty() {
                Some(self.num)
            } else {
                formula()?.parse().ok()
            }
        };
        Some(match self.kind {
            1 if !self.rgce.is_empty() => match formula()?.parse() {
                Ok(n) => Cfvo::Number(n),
                Err(_) => Cfvo::Formula(formula()?),
            },
            1 => Cfvo::Number(self.num),
            2 => Cfvo::Min,
            3 => Cfvo::Max,
            4 => Cfvo::Percent(number()?),
            5 => Cfvo::Percentile(number()?),
            7 => Cfvo::Formula(formula()?),
            _ => return None,
        })
    }
}

/// `KPISets14` (2.5.87): `KPISets` and Excel 2010's three.
const ICON_SETS14: [&str; 20] = [
    "3Arrows",
    "3ArrowsGray",
    "3Flags",
    "3TrafficLights1",
    "3TrafficLights2",
    "3Signs",
    "3Symbols",
    "3Symbols2",
    "4Arrows",
    "4ArrowsGray",
    "4RedToBlack",
    "4Rating",
    "4TrafficLights",
    "5Arrows",
    "5ArrowsGray",
    "5Rating",
    "5Quarters",
    "3Stars",
    "3Triangles",
    "5Boxes",
];

/// An icon set's name by `KPISets14` value (-1: "No Cell Icon").
fn icon_set_name(set: i32) -> Option<&'static str> {
    if set == -1 {
        return Some(crate::cf_edit::NO_ICONS);
    }
    ICON_SETS14.get(usize::try_from(set).ok()?).copied()
}

/// What a rule has beyond Kantor's model, kept for drawing and saving.
pub(super) enum Look {
    /// A data bar's Excel 2010 look.
    Bar(BarLook, bool),
    /// Icons picked one by one: the set named, whether values show, and per
    /// threshold the icon's set and index.
    CustomIcons(String, bool, Vec<(String, u8)>),
}

impl Rule {
    pub fn priority(&self) -> i32 {
        self.priority
    }

    /// The `BrtBeginCFRule14`'s GUID this rule names (`BrtCFRuleExt`).
    pub fn ext(&self) -> Option<[u8; 16]> {
        self.ext
    }

    pub fn guid(&self) -> Option<[u8; 16]> {
        self.guid
    }

    /// An Excel 2010 rule that stands on its own (no Excel 2007 rule names
    /// it), of a kind Kantor reads: a data bar or an icon set (their
    /// formats are no `BrtDXF14`).
    pub fn standalone_x14(&self) -> bool {
        self.priority != -1 && matches!(self.ty, 4 | 6)
    }

    /// The data bar's Excel 2010 record and colours: `x14`'s (the rule
    /// named by `BrtCFRuleExt`) or this rule's own (an Excel 2010 rule).
    /// Colours as `BrtBeginDatabar14` lists them: border, negative fill,
    /// negative border, axis (the positive fill is the Excel 2007 rule's).
    fn bar14<'a>(&'a self, x14: Option<&'a Rule>) -> Option<(Bar14, Vec<&'a [u8; 8]>)> {
        let src = x14.filter(|r| r.bar14.is_some()).unwrap_or(self);
        let bar = src.bar14?;
        let mut colors = src.colors.iter();
        if src.priority != -1 {
            colors.next(); // its positive fill
        }
        let mut take = |present: bool| if present { colors.next() } else { None };
        let border = take(bar.flags & 0x1 != 0);
        let neg_fill = take(bar.flags & 0x4 != 0);
        let neg_border = take(bar.flags & 0x8 != 0 && bar.flags & 0x1 != 0);
        let axis = take(bar.axis != 2);
        let list = [border, neg_fill, neg_border, axis]
            .into_iter()
            .map(|c| c.unwrap_or(&[0; 8]))
            .collect();
        Some((bar, list))
    }

    /// The look the model can't hold (`None`: Excel's defaults will do).
    pub fn look(&self, x14: Option<&Rule>, theme: &[u32]) -> Option<Look> {
        let colour = |c: &[u8; 8]| color(c, 0, theme).map(rgb_color);
        match self.ty {
            4 => {
                let gradient;
                let look = match self.bar14(x14) {
                    Some((bar, c)) => {
                        gradient = bar.flags & 0x2 != 0;
                        BarLook {
                            min_len: f32::from(bar.min.min(100)) / 100.0,
                            max_len: f32::from(bar.max.min(100)) / 100.0,
                            border: colour(c[0]).filter(|_| bar.flags & 0x1 != 0),
                            negative_fill: colour(c[1]).filter(|_| bar.flags & 0x4 != 0),
                            negative_border: colour(c[2]).filter(|_| bar.flags & 0x9 == 0x9),
                            axis: match bar.axis {
                                1 => Axis::Middle,
                                2 => Axis::None,
                                _ => Axis::Automatic,
                            },
                            axis_color: colour(c[3]).filter(|_| bar.axis != 2),
                            direction: match bar.direction {
                                1 => Direction::LeftToRight,
                                2 => Direction::RightToLeft,
                                _ => Direction::Context,
                            },
                        }
                    }
                    None => {
                        // Excel 2007's bar: its lengths (10 % to 90 %).
                        gradient = true;
                        let (min, max) = self.lengths.unwrap_or((10, 90));
                        BarLook {
                            min_len: f32::from(min.min(100)) / 100.0,
                            max_len: f32::from(max.min(100)) / 100.0,
                            ..BarLook::KANTOR
                        }
                    }
                };
                Some(Look::Bar(look, gradient))
            }
            6 if self.custom_icons && !self.cf_icons.is_empty() => {
                let (set, icon_only, _) = self.icons?;
                let set = icon_set_name(set as i32)?;
                let icons = self
                    .cf_icons
                    .iter()
                    .map(|(s, i)| {
                        let name = icon_set_name(*s)?;
                        let index = if *s == -1 { 0 } else { u8::try_from(*i).ok()? };
                        Some((name.to_owned(), index))
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(Look::CustomIcons(set.to_owned(), !icon_only, icons))
            }
            _ => None,
        }
    }

    /// The rule in Kantor's model (`None`: one it can't express). `x14`:
    /// the Excel 2010 rule named by its `BrtCFRuleExt`.
    pub fn convert(
        &self,
        ctx: &Context,
        dxfs: &[Dxf],
        theme: &[u32],
        x14: Option<&Rule>,
    ) -> Option<CfRuleInput> {
        let stop_if_true = self.flags & 0x2 != 0;
        let format = usize::try_from(self.dxf)
            .ok()
            .and_then(|i| dxfs.get(i))
            .cloned()
            .unwrap_or_default();
        let f = format.clone();
        let formula = || -> Option<CfRuleInput> {
            Some(CfRuleInput::Formula {
                formula: decompile12(&self.rgce1, ctx)?,
                format: format.clone(),
                stop_if_true,
            })
        };
        let colour = |c: &[u8; 8]| color(c, 0, theme).map_or(Color::None, rgb_color);
        match self.ty {
            // Cell value.
            1 => {
                let operator = match self.param {
                    1 => ValueOperator::Between,
                    2 => ValueOperator::NotBetween,
                    3 => ValueOperator::Equal,
                    4 => ValueOperator::NotEqual,
                    5 => ValueOperator::GreaterThan,
                    6 => ValueOperator::LessThan,
                    7 => ValueOperator::GreaterThanOrEqual,
                    8 => ValueOperator::LessThanOrEqual,
                    _ => return None,
                };
                let formula2 =
                    if matches!(operator, ValueOperator::Between | ValueOperator::NotBetween) {
                        Some(decompile12(&self.rgce2, ctx)?)
                    } else {
                        None
                    };
                Some(CfRuleInput::CellIs {
                    operator,
                    formula: decompile12(&self.rgce1, ctx)?,
                    formula2,
                    format: f,
                    stop_if_true,
                })
            }
            // An expression, or one of Excel 2007's templates.
            2 => Some(match self.template {
                0x07 => CfRuleInput::UniqueValues {
                    format: f,
                    stop_if_true,
                },
                0x08 => {
                    let operator = match self.param {
                        0 => TextOperator::Contains,
                        1 => TextOperator::DoesNotContain,
                        2 => TextOperator::BeginsWith,
                        3 => TextOperator::EndsWith,
                        _ => return formula(),
                    };
                    let Some(value) = self.text.clone() else {
                        return formula();
                    };
                    CfRuleInput::Text {
                        operator,
                        value,
                        format: f,
                        stop_if_true,
                    }
                }
                0x09 => CfRuleInput::Blanks {
                    format: f,
                    stop_if_true,
                },
                0x0A => CfRuleInput::NotBlanks {
                    format: f,
                    stop_if_true,
                },
                0x0B => CfRuleInput::Errors {
                    format: f,
                    stop_if_true,
                },
                0x0C => CfRuleInput::NoErrors {
                    format: f,
                    stop_if_true,
                },
                t @ 0x0F..=0x18 => CfRuleInput::TimePeriod {
                    time_period: match t {
                        0x0F => PeriodType::Today,
                        0x10 => PeriodType::Tomorrow,
                        0x11 => PeriodType::Yesterday,
                        0x12 => PeriodType::Last7Days,
                        0x13 => PeriodType::LastMonth,
                        0x14 => PeriodType::NextMonth,
                        0x15 => PeriodType::ThisWeek,
                        0x16 => PeriodType::NextWeek,
                        0x17 => PeriodType::LastWeek,
                        _ => PeriodType::ThisMonth,
                    },
                    date1: None,
                    date2: None,
                    format: f,
                    stop_if_true,
                },
                // Above / below average, "or equal" (0x1D, 0x1E), and
                // iParam standard deviations.
                0x19 | 0x1D => CfRuleInput::AboveAverage {
                    format: f,
                    stop_if_true,
                    equal_average: self.template == 0x1D,
                    std_dev: self.param,
                },
                0x1A | 0x1E => CfRuleInput::BelowAverage {
                    format: f,
                    stop_if_true,
                    equal_average: self.template == 0x1E,
                    std_dev: self.param,
                },
                0x1B => CfRuleInput::DuplicateValues {
                    format: f,
                    stop_if_true,
                },
                _ => return formula(),
            }),
            // Colour scale: the thresholds, then as many colours.
            3 => {
                let n = self.cfvos.len();
                if n != self.colors.len() || !(2..=3).contains(&n) {
                    return None;
                }
                let thresholds = self
                    .cfvos
                    .iter()
                    .zip(&self.colors)
                    .map(|(v, c)| {
                        Some(ColorScaleThreshold {
                            cfvo: v.convert(ctx)?,
                            color: colour(c),
                        })
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(CfRuleInput::ColorScale { thresholds })
            }
            // Data bar: shortest and longest bar's values, the colour.
            4 => {
                let [min, max] = self.cfvos.as_slice() else {
                    return None;
                };
                // Excel 2010's automatic ends (CFVOAUTOMIN/MAX_14).
                let end = |v: &RawCfvo| match v.kind {
                    8 | 9 => Some(None),
                    _ => v.convert(ctx).map(Some),
                };
                let positive_color = self
                    .colors
                    .first()
                    .map_or(Color::Rgb("#638EC6".into()), colour);
                let extras = self.bar14(x14);
                // Excel 2007 bars have no negative colour: Excel 2010's
                // default; an Excel 2010 bar's own, else the positive one.
                let negative_color = match &extras {
                    None => Color::Rgb("#FF0000".into()),
                    Some((bar, c)) if bar.flags & 0x4 != 0 => colour(c[1]),
                    Some(_) => positive_color.clone(),
                };
                Some(CfRuleInput::DataBar {
                    min: end(min)?,
                    max: end(max)?,
                    positive_color,
                    negative_color,
                    is_gradient: extras.is_none_or(|(bar, _)| bar.flags & 0x2 != 0),
                    show_value: self.show_value,
                })
            }
            // Top / bottom N (percent).
            5 => {
                let rank = self.param;
                let percent = self.flags & 0x10 != 0;
                Some(if self.flags & 0x8 != 0 {
                    CfRuleInput::Bottom10 {
                        rank,
                        percent,
                        format: f,
                        stop_if_true,
                    }
                } else {
                    CfRuleInput::Top10 {
                        rank,
                        percent,
                        format: f,
                        stop_if_true,
                    }
                })
            }
            // Icon set.
            6 => {
                let (set, icon_only, reverse) = self.icons?;
                let name = icon_set_name(set as i32)?;
                let cfvos = self
                    .cfvos
                    .iter()
                    .map(|v| Some((v.convert(ctx)?, v.gte)))
                    .collect::<Option<Vec<_>>>()?;
                if let Some((count, icon, rgb)) =
                    crate::ods::rating(name).filter(|_| !self.custom_icons)
                {
                    if cfvos.len() != count {
                        return None;
                    }
                    return Some(CfRuleInput::IconRating {
                        icon,
                        color: Color::Rgb(rgb.to_owned()),
                        thresholds: cfvos,
                        show_value: !icon_only,
                    });
                }
                let icons = if let Some(Look::CustomIcons(_, _, list)) = self.look(None, &[]) {
                    // Excel 2010's icons picked one by one.
                    list.into_iter()
                        .map(|(set, index)| crate::cf_edit::CfIconRef { set, index }.model_icon())
                        .collect::<Option<Vec<_>>>()?
                } else {
                    let mut icons = icon_set_icons(name)?;
                    if reverse {
                        icons.reverse();
                    }
                    icons
                };
                if icons.len() != cfvos.len() {
                    return None;
                }
                Some(CfRuleInput::IconSet {
                    thresholds: icons
                        .into_iter()
                        .zip(cfvos)
                        .map(|((icon, color), (cfvo, is_strict))| IconThreshold {
                            icon,
                            cfvo,
                            color,
                            is_strict,
                        })
                        .collect(),
                    show_value: !icon_only,
                })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prop(ty: u16, data: &[u8]) -> Vec<u8> {
        let mut v = ty.to_le_bytes().to_vec();
        v.extend(((data.len() + 4) as u16).to_le_bytes());
        v.extend(data);
        v
    }

    #[test]
    fn dxf_reads_font_fill_border_and_format() {
        let red = [0x05, 0, 0, 0, 0xC0, 0, 0, 0xFF];
        let yellow = [0x05, 0, 0, 0, 0xFF, 0xFF, 0, 0xFF];
        let mut props = Vec::new();
        props.extend(prop(0x05, &red));
        props.extend(prop(0x19, &700u16.to_le_bytes()));
        props.extend(prop(0x1C, &[1]));
        props.extend(prop(0x00, &[1]));
        props.extend(prop(0x02, &yellow));
        let mut bottom = red.to_vec();
        bottom.extend(1u16.to_le_bytes());
        props.extend(prop(0x07, &bottom));
        let mut code = 4u16.to_le_bytes().to_vec();
        code.extend("0.0%".encode_utf16().flat_map(|u| u.to_le_bytes()));
        props.extend(prop(0x26, &code));
        props.extend(prop(0x29, &170u16.to_le_bytes()));
        let mut d = vec![0, 0, 0, 0];
        d.extend(8u16.to_le_bytes());
        d.extend(props);
        let x = dxf(&d, &[], &[]);
        let font = x.font.unwrap();
        assert_eq!((font.b, font.i), (Some(true), Some(true)));
        assert_eq!(font.color, rgb_color(0xC00000));
        assert_eq!(x.fill.unwrap().color, rgb_color(0xFFFF00));
        assert!(x.border.unwrap().bottom.is_some());
        let n = x.num_fmt.unwrap();
        assert_eq!((n.num_fmt_id, n.format_code.as_str()), (170, "0.0%"));
    }
}
