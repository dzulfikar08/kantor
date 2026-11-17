//! Excel 2007 conditional formats in `.xls` ([MS-XLS] 2.4.40 `CONDFMT12`,
//! 2.4.43 `CF12`, 2.4.44 `CFEX`).
//!
//! Excel 97-2003 readers see `CONDFMT`/`CF` (cell value and formula rules).
//! Excel 2007 writes the rules those can't hold as `CONDFMT12` + `CF12`
//! (colour scales, data bars, icon sets, top/bottom N, and any rule with a
//! template such as "Text that contains"), and adds a `CFEX` after a `CF`
//! whose rule came from a template (text, dates, blanks, errors, duplicate
//! and unique values, above or below average): the `CF` holds the formula
//! Excel 97 evaluates, the `CFEX` the template, its parameters and the
//! rule's priority.
//!
//! A rule with a template Kantor's model has becomes that rule (so it is
//! saved as Excel 2007 writes it in an `.xlsx`); other templates keep the
//! formula of the record. Byte layouts follow the spec, checked against a
//! workbook Excel wrote (Apache POI's public test file
//! `NewStyleConditionalFormattings.xls`: `CFEX` has a full `FrtRefHeader`
//! and its format is a `DXFN12` with its size; a threshold of type min or
//! max has no number). The fixture `tests/corpus/xls/cf12.xls` is written
//! from the same layouts by `scripts/corpus/gen_xls.py --cf12`.

use ironcalc_base::cf_types::{
    CfRuleInput, Cfvo, ColorScaleThreshold, IconThreshold, PeriodType, TextOperator, icon_set_icons,
};
use ironcalc_base::types::Color;

use super::{Bytes, CellRange, Globals, RawCf, XlsDxf, rgb_color, sqref};
use crate::xls_formula::{Context, decompile};

/// What Excel 2007 adds to a rule.
#[derive(Debug, Clone, Default)]
pub(super) struct Ext {
    /// `icfTemplate` (`CFTemp`): which Excel 2007 rule it is.
    pub template: Option<u16>,
    /// `rgbTemplateParms`.
    pub params: [u8; 16],
    pub priority: Option<u16>,
    pub stop: bool,
    /// The rule's own part (`rgbCT` of a `CF12`).
    pub scale: Scale,
}

/// `rgbCT` of a `CF12`.
#[derive(Debug, Clone, Default)]
pub(super) enum Scale {
    #[default]
    None,
    /// `CFGradient`: thresholds, then colours, in order.
    Gradient(Vec<Cfvo12>, Vec<Color>),
    /// `CFDatabar`.
    DataBar {
        color: Color,
        min: Cfvo12,
        max: Cfvo12,
        show_value: bool,
    },
    /// `CFMultistate`.
    Icons {
        set: u8,
        icon_only: bool,
        reverse: bool,
        /// Each threshold and whether it is `>=` (`fEqual`).
        states: Vec<(Cfvo12, bool)>,
    },
}

/// A `CFVO` as read: its type, formula tokens and number.
#[derive(Debug, Clone)]
pub(super) struct Cfvo12 {
    kind: u8,
    rgce: Vec<u8>,
    num: f64,
}

/// `CFTemp` values ([MS-XLS] 2.5.17).
mod temp {
    pub const FILTER: u16 = 0x05;
    pub const UNIQUE: u16 = 0x07;
    pub const TEXT: u16 = 0x08;
    pub const BLANKS: u16 = 0x09;
    pub const NO_BLANKS: u16 = 0x0A;
    pub const ERRORS: u16 = 0x0B;
    pub const NO_ERRORS: u16 = 0x0C;
    pub const TODAY: u16 = 0x0F;
    pub const THIS_MONTH: u16 = 0x18;
    pub const ABOVE_AVERAGE: u16 = 0x19;
    pub const BELOW_AVERAGE: u16 = 0x1A;
    pub const DUPLICATES: u16 = 0x1B;
}

/// The `iIconSet` names, in `KPISets`'s order ([MS-XLS] 2.5.156).
pub(crate) const ICON_SETS: [&str; 17] = [
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
];

/// `CONDFMT12`: `frtRefHeaderU`, then a `CondFmtStructure` (`ccf`, flags,
/// `refBound`, `sqref`). Its ranges and rule count.
pub(super) fn condfmt12(d: &[u8]) -> Option<(Vec<CellRange>, u16)> {
    let mut b = Bytes::new(d);
    b.skip(12)?;
    let ccf = b.u16()?;
    b.skip(2 + 8)?;
    Some((sqref(&mut b)?, ccf))
}

/// A `Ref8U` as a range.
fn ref8(b: &mut Bytes) -> Option<Vec<CellRange>> {
    let (r1, r2, c1, c2) = (b.u16()?, b.u16()?, b.u16()?, b.u16()?);
    if r2 < r1 || c2 < c1 {
        return Some(Vec::new());
    }
    Some(vec![CellRange::from_corners(
        (r1 as u32, c1 as u32),
        (r2 as u32, (c2 as u32).min(crate::MAX_COLS - 1)),
    )])
}

/// A `DXFN12`: `cbDxf`, then a `DXFN` and an `XFExtNoFRT` in that many
/// bytes (or 2 unused bytes when 0).
fn dxfn12(b: &mut Bytes) -> Option<XlsDxf> {
    let cb = b.u32()? as usize;
    if cb == 0 {
        b.skip(2)?;
        return Some(XlsDxf::default());
    }
    let body = b.take(cb)?;
    XlsDxf::parse(&mut Bytes::new(body))
}

/// A `CFVO` ([MS-XLS] 2.5.18): `cfvoType`, a formula (`cce`, `rgce`), and
/// a number when there is no formula and the type isn't min or max.
fn cfvo(b: &mut Bytes) -> Option<Cfvo12> {
    let kind = b.u8()?;
    let cce = b.u16()? as usize;
    let rgce = b.take(cce)?.to_vec();
    let num = if cce == 0 && !matches!(kind, 2 | 3) {
        b.f64()?
    } else {
        0.0
    };
    Some(Cfvo12 { kind, rgce, num })
}

/// A `CFColor` ([MS-XLS] 2.5.13): type, value, tint.
fn cf_color(b: &mut Bytes, g: &Globals) -> Option<Color> {
    let kind = b.u32()?;
    let value = b.take(4)?;
    let tint = b.f64()?;
    let v = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
    Some(match kind {
        // Indexed (a palette colour).
        1 => g.palette.color(v as u16),
        // RGB: red, green, blue, reserved.
        2 => rgb_color(
            (u32::from(value[0]) << 16) | (u32::from(value[1]) << 8) | u32::from(value[2]),
        ),
        // A theme colour, as `.xlsx` numbers them.
        3 => Color::Theme(v as i32, tint),
        _ => Color::None,
    })
}

/// A `CF12` record: its range (`frtRefHeader`'s `ref8`, used when no
/// `CONDFMT12` precedes it) and the rule.
pub(super) fn parse_cf12_with(d: &[u8], g: &Globals) -> Option<(Vec<CellRange>, RawCf)> {
    let mut b = Bytes::new(d);
    b.skip(4)?; // rt, grbitFrt
    let own = ref8(&mut b)?;
    let ct = b.u8()?;
    let cp = b.u8()?;
    let cce1 = b.u16()? as usize;
    let cce2 = b.u16()? as usize;
    let dxf = dxfn12(&mut b)?;
    let rgce1 = b.take(cce1)?.to_vec();
    let rgce2 = b.take(cce2)?.to_vec();
    // fmlaActive (a CFParsedFormula: cce, rgce).
    let cce = b.u16()? as usize;
    b.skip(cce)?;
    let flags = b.u8()?;
    let priority = b.u16()?;
    let template = b.u16()?;
    let cb = b.u8()? as usize;
    let mut params = [0u8; 16];
    let p = b.take(cb.min(16))?;
    params[..p.len()].copy_from_slice(p);
    b.skip(cb.saturating_sub(16))?;
    let scale = match ct {
        3 => {
            // CFGradient: unused, reserved, cInterpCurve, cGradientCurve,
            // flags, then the interpolation items and the colour items.
            b.skip(3)?;
            let n_interp = b.u8()? as usize;
            let n_colors = b.u8()? as usize;
            b.skip(1)?;
            let mut points = Vec::new();
            for _ in 0..n_interp {
                points.push(cfvo(&mut b)?);
                b.f64()?; // numDomain
            }
            let mut colors = Vec::new();
            for _ in 0..n_colors {
                b.f64()?; // numGrange
                colors.push(cf_color(&mut b, g)?);
            }
            Scale::Gradient(points, colors)
        }
        4 => {
            // CFDatabar: unused, reserved, flags (bit 0: the bar only, as
            // Excel writes it and Apache POI reads it), iPercentMin,
            // iPercentMax, colour, two CFVOs.
            b.skip(3)?;
            let flags = b.u8()?;
            b.skip(2)?;
            let color = cf_color(&mut b, g)?;
            let min = cfvo(&mut b)?;
            let max = cfvo(&mut b)?;
            Scale::DataBar {
                color,
                min,
                max,
                show_value: flags & 0x1 == 0,
            }
        }
        6 => {
            // CFMultistate: unused, reserved, cStates, iIconSet, flags,
            // then each state's CFVO, fEqual and 4 unused bytes.
            b.skip(3)?;
            let n = b.u8()? as usize;
            let set = b.u8()?;
            let flags = b.u8()?;
            let mut states = Vec::new();
            for _ in 0..n {
                let v = cfvo(&mut b)?;
                let equal = b.u8()? != 0;
                b.skip(4)?;
                states.push((v, equal));
            }
            Scale::Icons {
                set,
                icon_only: flags & 0x1 != 0,
                reverse: flags & 0x4 != 0,
                states,
            }
        }
        _ => Scale::None,
    };
    Some((
        own,
        RawCf {
            ct,
            cp,
            dxf,
            rgce1,
            rgce2,
            id: None,
            ext: Ext {
                template: Some(template),
                params,
                priority: Some(priority),
                stop: flags & 0x1 != 0,
                scale,
            },
        },
    ))
}

/// A `CFEX` record.
pub(super) enum CfEx {
    /// For a `CF`: `(CONDFMT id, rule index, extras)`.
    Cf(u16, u16, Ext),
    /// The `CF12` that follows belongs to the `CONDFMT` with this id (Excel
    /// puts a rule with a threshold formula there, next to the block's
    /// `CF` rules).
    Cf12(u16),
}

pub(super) fn parse_cfex(d: &[u8]) -> Option<CfEx> {
    let mut b = Bytes::new(d);
    b.skip(12)?; // frtRefHeader: rt, grbitFrt, ref8
    let is_cf12 = b.u32()?;
    let id = b.u16()?;
    if is_cf12 != 0 {
        return Some(CfEx::Cf12(id));
    }
    let icf = b.u16()?;
    let _cp = b.u8()?;
    let template = b.u8()? as u16;
    let priority = b.u16()?;
    let flags = b.u8()?;
    let has_dxf = b.u8()? != 0;
    if has_dxf {
        // A DXFN12 (Excel 2007's format of the rule; the CF has its own).
        dxfn12(&mut b)?;
    }
    let cb = b.u8()? as usize;
    let mut params = [0u8; 16];
    let p = b.take(cb.min(16))?;
    params[..p.len()].copy_from_slice(p);
    Some(CfEx::Cf(
        id,
        icf,
        Ext {
            template: Some(template),
            params,
            priority: Some(priority),
            stop: flags & 0x2 != 0,
            scale: Scale::None,
        },
    ))
}

/// Give each `CF` its `CFEX`.
pub(super) fn apply_cfex(cf: &mut [(Vec<CellRange>, RawCf)], cfex: &[(u16, u16, Ext)]) {
    for (id, icf, ext) in cfex {
        if let Some((_, rule)) = cf
            .iter_mut()
            .find(|(_, r)| r.id == Some((*id, *icf)) && r.ext.template.is_none())
        {
            rule.ext = ext.clone();
        }
    }
}

impl Cfvo12 {
    fn convert(&self, ctx: &Context) -> Option<Cfvo> {
        let formula = || decompile(&self.rgce, ctx);
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

/// The first string literal of a formula (`"ab"` in
/// `NOT(ISERROR(SEARCH("ab",A1)))`).
fn first_string(formula: &str) -> Option<String> {
    let start = formula.find('"')? + 1;
    let mut out = String::new();
    let mut chars = formula[start..].chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' {
            if chars.peek() == Some(&'"') {
                chars.next();
                out.push('"');
            } else {
                return Some(out);
            }
        } else {
            out.push(c);
        }
    }
    None
}

/// A rule of a `CF` (with its `CFEX`) or `CF12` in Kantor's model.
pub(super) fn convert(cf: &RawCf, ctx: &Context, g: &Globals) -> Option<CfRuleInput> {
    let stop_if_true = cf.ext.stop;
    let format = cf.dxf.convert(g);
    let formula = || -> Option<CfRuleInput> {
        Some(CfRuleInput::Formula {
            formula: decompile(&cf.rgce1, ctx)?,
            format: format.clone(),
            stop_if_true,
        })
    };
    let param = |i: usize| u16::from_le_bytes([cf.ext.params[i], cf.ext.params[i + 1]]);
    let top10 = || -> Option<CfRuleInput> {
        // CFExFilterParams: fTop, fPercent, then the rank.
        let flags = cf.ext.params[0];
        let rank = u32::from(param(1));
        let percent = flags & 0x2 != 0;
        Some(if flags & 0x1 != 0 {
            CfRuleInput::Top10 {
                rank,
                percent,
                format: format.clone(),
                stop_if_true,
            }
        } else {
            CfRuleInput::Bottom10 {
                rank,
                percent,
                format: format.clone(),
                stop_if_true,
            }
        })
    };
    match cf.ct {
        1 => cf.cell_is(ctx, format.clone()),
        2 => {
            let Some(t) = cf.ext.template else {
                return formula();
            };
            let f = format.clone();
            Some(match t {
                temp::TEXT => {
                    let operator = match param(0) {
                        0 => TextOperator::Contains,
                        1 => TextOperator::DoesNotContain,
                        2 => TextOperator::BeginsWith,
                        3 => TextOperator::EndsWith,
                        _ => return formula(),
                    };
                    let Some(value) = decompile(&cf.rgce1, ctx).and_then(|s| first_string(&s))
                    else {
                        return formula();
                    };
                    CfRuleInput::Text {
                        operator,
                        value,
                        format: f,
                        stop_if_true,
                    }
                }
                temp::BLANKS => CfRuleInput::Blanks {
                    format: f,
                    stop_if_true,
                },
                temp::NO_BLANKS => CfRuleInput::NotBlanks {
                    format: f,
                    stop_if_true,
                },
                temp::ERRORS => CfRuleInput::Errors {
                    format: f,
                    stop_if_true,
                },
                temp::NO_ERRORS => CfRuleInput::NoErrors {
                    format: f,
                    stop_if_true,
                },
                temp::UNIQUE => CfRuleInput::UniqueValues {
                    format: f,
                    stop_if_true,
                },
                temp::DUPLICATES => CfRuleInput::DuplicateValues {
                    format: f,
                    stop_if_true,
                },
                temp::TODAY..=temp::THIS_MONTH => {
                    let time_period = match t {
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
                    };
                    CfRuleInput::TimePeriod {
                        time_period,
                        date1: None,
                        date2: None,
                        format: f,
                        stop_if_true,
                    }
                }
                // Plain above / below average (no standard deviations):
                // the "or equal" and deviation variants keep the formula.
                temp::ABOVE_AVERAGE if param(0) == 0 => CfRuleInput::AboveAverage {
                    format: f,
                    stop_if_true,
                },
                temp::BELOW_AVERAGE if param(0) == 0 => CfRuleInput::BelowAverage {
                    format: f,
                    stop_if_true,
                },
                temp::FILTER => return top10(),
                _ => return formula(),
            })
        }
        3 => {
            let Scale::Gradient(points, colors) = &cf.ext.scale else {
                return None;
            };
            if points.len() != colors.len() || !(2..=3).contains(&points.len()) {
                return None;
            }
            let thresholds = points
                .iter()
                .zip(colors)
                .map(|(p, color)| {
                    Some(ColorScaleThreshold {
                        cfvo: p.convert(ctx)?,
                        color: color.clone(),
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Some(CfRuleInput::ColorScale { thresholds })
        }
        4 => {
            let Scale::DataBar {
                color,
                min,
                max,
                show_value,
            } = &cf.ext.scale
            else {
                return None;
            };
            Some(CfRuleInput::DataBar {
                min: Some(min.convert(ctx)?),
                max: Some(max.convert(ctx)?),
                positive_color: color.clone(),
                // Excel 2007 bars have no negative colour: Excel 2010's
                // default.
                negative_color: Color::Rgb("#FF0000".into()),
                is_gradient: true,
                show_value: *show_value,
            })
        }
        5 => top10(),
        6 => {
            let Scale::Icons {
                set,
                icon_only,
                reverse,
                states,
            } = &cf.ext.scale
            else {
                return None;
            };
            let name = *ICON_SETS.get(*set as usize)?;
            let cfvos = states
                .iter()
                .map(|(v, equal)| Some((v.convert(ctx)?, *equal)))
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
            if *reverse {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_string_unescapes_quotes() {
        assert_eq!(
            first_string(r#"NOT(ISERROR(SEARCH("a""b",A1)))"#).as_deref(),
            Some("a\"b")
        );
        assert_eq!(first_string("A1>2"), None);
    }

    #[test]
    fn cfvo_reads_a_number_only_without_formula_and_for_numbers() {
        // Min: type 2, no formula, no number.
        let mut b = Bytes::new(&[2, 0, 0, 0xAA]);
        let v = cfvo(&mut b).unwrap();
        assert_eq!((v.kind, b.u8()), (2, Some(0xAA)));
        // Percent 50: type 4, no formula, an Xnum.
        let mut d = vec![4, 0, 0];
        d.extend(50f64.to_le_bytes());
        let v = cfvo(&mut Bytes::new(&d)).unwrap();
        assert_eq!((v.kind, v.num), (4, 50.0));
    }
}
