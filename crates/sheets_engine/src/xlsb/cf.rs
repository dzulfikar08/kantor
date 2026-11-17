//! Conditional formatting of `.xlsb` sheets ([MS-XLSB] 2.4.34
//! `BrtBeginConditionalFormatting`, 2.4.23 `BrtBeginCFRule`, 2.4.28/43/91
//! colour scales, data bars and icon sets with their `BrtCFVO` and
//! `BrtColor` records) and the differential formats they point at
//! (`BrtDXF` in `styles.bin`: an `XFProps` list).
//!
//! The rule types and templates are the ones `.xls` uses for Excel 2007
//! (`CF12`, see `xls::cf12`); formulas are BIFF12 tokens. Excel 2010's
//! extras (`BrtBeginConditionalFormatting14` inside `BrtFRTBegin`: data
//! bar negative colours and axes, custom icon sets) are not read: those
//! rules keep their Excel 2007 form.

use ironcalc_base::cf_types::{
    CfRuleInput, Cfvo, ColorScaleThreshold, IconThreshold, PeriodType, TextOperator, ValueOperator,
    icon_set_icons,
};
use ironcalc_base::types::{Border, BorderItem, Color, Dxf, DxfFont, Fill, NumFmt};

use super::{color, f64_at, fill_color, u8_at, u16_at, u32_at, wide};
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
    /// Inside a `BrtBeginCFRule`.
    in_rule: bool,
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
        self.blocks.last_mut()?.rules.last_mut()
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

impl Rule {
    pub fn priority(&self) -> i32 {
        self.priority
    }

    /// The rule in Kantor's model (`None`: one it can't express).
    pub fn convert(&self, ctx: &Context, dxfs: &[Dxf], theme: &[u32]) -> Option<CfRuleInput> {
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
                // Plain above / below average; the "or equal" and standard
                // deviation forms have no counterpart (and no formula).
                0x19 if self.param == 0 => CfRuleInput::AboveAverage {
                    format: f,
                    stop_if_true,
                },
                0x1A if self.param == 0 => CfRuleInput::BelowAverage {
                    format: f,
                    stop_if_true,
                },
                0x1B => CfRuleInput::DuplicateValues {
                    format: f,
                    stop_if_true,
                },
                0x19 | 0x1A | 0x1D | 0x1E => return None,
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
                Some(CfRuleInput::DataBar {
                    min: Some(min.convert(ctx)?),
                    max: Some(max.convert(ctx)?),
                    positive_color: self
                        .colors
                        .first()
                        .map_or(Color::Rgb("#638EC6".into()), colour),
                    // Excel 2007 bars have no negative colour: Excel 2010's
                    // default.
                    negative_color: Color::Rgb("#FF0000".into()),
                    is_gradient: true,
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
                let name = *crate::xls::cf12::ICON_SETS.get(set as usize)?;
                let cfvos = self
                    .cfvos
                    .iter()
                    .map(|v| Some((v.convert(ctx)?, v.gte)))
                    .collect::<Option<Vec<_>>>()?;
                if let Some((count, icon, rgb)) = crate::ods::rating(name) {
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
                let mut icons = icon_set_icons(name)?;
                if reverse {
                    icons.reverse();
                }
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
