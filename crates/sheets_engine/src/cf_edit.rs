//! Making and changing conditional formatting rules in Kantor: Highlight
//! Cells, Top/Bottom, Data Bars, Color Scales and Icon Sets quick picks, New
//! Rule with a formula, and Manage Rules (edit, delete, priority, stop if
//! true).
//!
//! Rules live in IronCalc's model; each change is one IronCalc step
//! (`UserModel::add/update/delete_conditional_formatting`, priority swaps),
//! folded with the history state's [`crate::edits::Edits`] mark into one undo
//! step. A sheet whose rules were changed is saved with IronCalc's own
//! conditional formatting markup (and the dxfs it added appended to the
//! file's); sheets nobody touched keep the file's markup byte for byte (see
//! `preserve`).

use anyhow::{Result, anyhow, bail};
use ironcalc_base::cf_types::{
    CfRule, CfRuleInput, Cfvo, ColorScaleThreshold, IconThreshold, PeriodType, TextOperator,
    ValueOperator, icon_set_icons,
};
use ironcalc_base::types::{Color, Dxf, DxfFont, Fill};

use crate::cond_format::parse_sqref;
use crate::format::{Rgb, parse_rgb};
use crate::{CellRange, Workbook};

/// Value comparisons of "Format only cells with: Cell Value".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfCompare {
    GreaterThan,
    LessThan,
    Between,
    NotBetween,
    EqualTo,
    NotEqualTo,
    GreaterOrEqual,
    LessOrEqual,
}

impl CfCompare {
    pub const ALL: [CfCompare; 8] = [
        CfCompare::GreaterThan,
        CfCompare::LessThan,
        CfCompare::Between,
        CfCompare::NotBetween,
        CfCompare::EqualTo,
        CfCompare::NotEqualTo,
        CfCompare::GreaterOrEqual,
        CfCompare::LessOrEqual,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CfCompare::GreaterThan => "greater than",
            CfCompare::LessThan => "less than",
            CfCompare::Between => "between",
            CfCompare::NotBetween => "not between",
            CfCompare::EqualTo => "equal to",
            CfCompare::NotEqualTo => "not equal to",
            CfCompare::GreaterOrEqual => "greater than or equal to",
            CfCompare::LessOrEqual => "less than or equal to",
        }
    }

    pub fn two_values(self) -> bool {
        matches!(self, CfCompare::Between | CfCompare::NotBetween)
    }

    fn op(self) -> ValueOperator {
        match self {
            CfCompare::GreaterThan => ValueOperator::GreaterThan,
            CfCompare::LessThan => ValueOperator::LessThan,
            CfCompare::Between => ValueOperator::Between,
            CfCompare::NotBetween => ValueOperator::NotBetween,
            CfCompare::EqualTo => ValueOperator::Equal,
            CfCompare::NotEqualTo => ValueOperator::NotEqual,
            CfCompare::GreaterOrEqual => ValueOperator::GreaterThanOrEqual,
            CfCompare::LessOrEqual => ValueOperator::LessThanOrEqual,
        }
    }

    fn from_op(op: &ValueOperator) -> CfCompare {
        match op {
            ValueOperator::GreaterThan => CfCompare::GreaterThan,
            ValueOperator::LessThan => CfCompare::LessThan,
            ValueOperator::Between => CfCompare::Between,
            ValueOperator::NotBetween => CfCompare::NotBetween,
            ValueOperator::Equal => CfCompare::EqualTo,
            ValueOperator::NotEqual => CfCompare::NotEqualTo,
            ValueOperator::GreaterThanOrEqual => CfCompare::GreaterOrEqual,
            ValueOperator::LessThanOrEqual => CfCompare::LessOrEqual,
        }
    }
}

/// Text comparisons ("Text that Contains"...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfTextOp {
    Contains,
    DoesNotContain,
    BeginsWith,
    EndsWith,
}

impl CfTextOp {
    pub const ALL: [CfTextOp; 4] = [
        CfTextOp::Contains,
        CfTextOp::DoesNotContain,
        CfTextOp::BeginsWith,
        CfTextOp::EndsWith,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CfTextOp::Contains => "containing",
            CfTextOp::DoesNotContain => "not containing",
            CfTextOp::BeginsWith => "beginning with",
            CfTextOp::EndsWith => "ending with",
        }
    }

    fn op(self) -> TextOperator {
        match self {
            CfTextOp::Contains => TextOperator::Contains,
            CfTextOp::DoesNotContain => TextOperator::DoesNotContain,
            CfTextOp::BeginsWith => TextOperator::BeginsWith,
            CfTextOp::EndsWith => TextOperator::EndsWith,
        }
    }
}

/// "A Date Occurring".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfPeriod {
    Yesterday,
    Today,
    Tomorrow,
    Last7Days,
    LastWeek,
    ThisWeek,
    NextWeek,
    LastMonth,
    ThisMonth,
    NextMonth,
}

impl CfPeriod {
    pub const ALL: [CfPeriod; 10] = [
        CfPeriod::Yesterday,
        CfPeriod::Today,
        CfPeriod::Tomorrow,
        CfPeriod::Last7Days,
        CfPeriod::LastWeek,
        CfPeriod::ThisWeek,
        CfPeriod::NextWeek,
        CfPeriod::LastMonth,
        CfPeriod::ThisMonth,
        CfPeriod::NextMonth,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CfPeriod::Yesterday => "Yesterday",
            CfPeriod::Today => "Today",
            CfPeriod::Tomorrow => "Tomorrow",
            CfPeriod::Last7Days => "In the last 7 days",
            CfPeriod::LastWeek => "Last week",
            CfPeriod::ThisWeek => "This week",
            CfPeriod::NextWeek => "Next week",
            CfPeriod::LastMonth => "Last month",
            CfPeriod::ThisMonth => "This month",
            CfPeriod::NextMonth => "Next month",
        }
    }

    fn period(self) -> PeriodType {
        match self {
            CfPeriod::Yesterday => PeriodType::Yesterday,
            CfPeriod::Today => PeriodType::Today,
            CfPeriod::Tomorrow => PeriodType::Tomorrow,
            CfPeriod::Last7Days => PeriodType::Last7Days,
            CfPeriod::LastWeek => PeriodType::LastWeek,
            CfPeriod::ThisWeek => PeriodType::ThisWeek,
            CfPeriod::NextWeek => PeriodType::NextWeek,
            CfPeriod::LastMonth => PeriodType::LastMonth,
            CfPeriod::ThisMonth => PeriodType::ThisMonth,
            CfPeriod::NextMonth => PeriodType::NextMonth,
        }
    }

    fn from_period(p: &PeriodType) -> Option<CfPeriod> {
        CfPeriod::ALL.into_iter().find(|c| &c.period() == p)
    }
}

/// What a rule tests, or draws.
#[derive(Debug, Clone, PartialEq)]
pub enum CfSpec {
    /// Cell value compared with one or two values (numbers, `"text"` or
    /// formulas such as `$B$1`).
    CellValue {
        op: CfCompare,
        value: String,
        value2: String,
    },
    Text {
        op: CfTextOp,
        text: String,
    },
    Dates {
        period: CfPeriod,
    },
    /// Duplicate values, or unique ones.
    Duplicates {
        unique: bool,
    },
    TopBottom {
        bottom: bool,
        rank: u32,
        percent: bool,
    },
    Average {
        below: bool,
    },
    Blanks {
        not: bool,
    },
    /// "Use a formula to determine which cells to format" (relative to the
    /// top-left cell of the range).
    Formula {
        formula: String,
    },
    DataBar {
        color: Rgb,
        gradient: bool,
    },
    /// Two colours (lowest, highest) or three (lowest, 50th percentile,
    /// highest).
    ColorScale {
        colors: Vec<Rgb>,
    },
    /// An Excel icon set by name (`3Arrows`, `3TrafficLights1`, `5Rating`...).
    IconSet {
        set: String,
        reverse: bool,
        show_value: bool,
    },
}

impl CfSpec {
    /// Whether it applies a format (fill, font) rather than drawing.
    pub fn has_format(&self) -> bool {
        !matches!(
            self,
            CfSpec::DataBar { .. } | CfSpec::ColorScale { .. } | CfSpec::IconSet { .. }
        )
    }
}

/// The format a rule applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CfFormat {
    pub fill: Option<Rgb>,
    pub font: Option<Rgb>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

impl CfFormat {
    /// Excel's Highlight Cells presets.
    pub const PRESETS: [(&'static str, CfFormat); 6] = [
        (
            "Light Red Fill with Dark Red Text",
            CfFormat::colors(Some(0xFFC7CE), Some(0x9C0006)),
        ),
        (
            "Yellow Fill with Dark Yellow Text",
            CfFormat::colors(Some(0xFFEB9C), Some(0x9C5700)),
        ),
        (
            "Green Fill with Dark Green Text",
            CfFormat::colors(Some(0xC6EFCE), Some(0x006100)),
        ),
        ("Light Red Fill", CfFormat::colors(Some(0xFFC7CE), None)),
        ("Red Text", CfFormat::colors(None, Some(0x9C0006))),
        (
            "Bold",
            CfFormat {
                fill: None,
                font: None,
                bold: true,
                italic: false,
                underline: false,
                strike: false,
            },
        ),
    ];

    pub const fn colors(fill: Option<Rgb>, font: Option<Rgb>) -> CfFormat {
        CfFormat {
            fill,
            font,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
        }
    }

    fn dxf(&self) -> Dxf {
        let font =
            (self.font.is_some() || self.bold || self.italic || self.underline || self.strike)
                .then(|| DxfFont {
                    strike: self.strike.then_some(true),
                    u: self.underline.then_some(true),
                    b: self.bold.then_some(true),
                    i: self.italic.then_some(true),
                    sz: None,
                    color: self.font.map(color).unwrap_or(Color::None),
                });
        Dxf {
            font,
            fill: self.fill.map(|c| Fill { color: color(c) }),
            border: None,
            num_fmt: None,
            alignment: None,
        }
    }

    fn from_dxf(d: &Dxf, wb: &ironcalc_base::Model) -> CfFormat {
        let rgb = |c: &Color| -> Option<Rgb> {
            match c {
                Color::None => None,
                c => parse_rgb(&c.to_rgb(&wb.workbook.theme)),
            }
        };
        CfFormat {
            fill: d.fill.as_ref().and_then(|f| rgb(&f.color)),
            font: d.font.as_ref().and_then(|f| rgb(&f.color)),
            bold: d.font.as_ref().and_then(|f| f.b).unwrap_or(false),
            italic: d.font.as_ref().and_then(|f| f.i).unwrap_or(false),
            underline: d.font.as_ref().and_then(|f| f.u).unwrap_or(false),
            strike: d.font.as_ref().and_then(|f| f.strike).unwrap_or(false),
        }
    }
}

fn color(c: Rgb) -> Color {
    Color::Rgb(format!("#{:06X}", c & 0xFF_FFFF))
}

/// Excel's Data Bar colours.
pub const DATA_BAR_COLORS: [(&str, Rgb); 6] = [
    ("Blue", 0x638EC6),
    ("Green", 0x63C384),
    ("Red", 0xFF555A),
    ("Orange", 0xFFB628),
    ("Light Blue", 0x008AEF),
    ("Purple", 0xD6007B),
];

/// Excel's Color Scales (lowest, [middle,] highest).
pub const COLOR_SCALES: [(&str, &[Rgb]); 12] = [
    ("Green - Yellow - Red", &[0xF8696B, 0xFFEB84, 0x63BE7B]),
    ("Red - Yellow - Green", &[0x63BE7B, 0xFFEB84, 0xF8696B]),
    ("Green - White - Red", &[0xF8696B, 0xFCFCFF, 0x63BE7B]),
    ("Red - White - Green", &[0x63BE7B, 0xFCFCFF, 0xF8696B]),
    ("Blue - White - Red", &[0xF8696B, 0xFCFCFF, 0x5A8AC6]),
    ("Red - White - Blue", &[0x5A8AC6, 0xFCFCFF, 0xF8696B]),
    ("White - Red", &[0xFCFCFF, 0xF8696B]),
    ("Red - White", &[0xF8696B, 0xFCFCFF]),
    ("Green - White", &[0x63BE7B, 0xFCFCFF]),
    ("White - Green", &[0xFCFCFF, 0x63BE7B]),
    ("Green - Yellow", &[0x63BE7B, 0xFFEF9C]),
    ("Yellow - Green", &[0xFFEF9C, 0x63BE7B]),
];

/// Excel's icon sets offered (rating sets are drawn as ratings).
pub const ICON_SETS: [(&str, &str); 12] = [
    ("3 Arrows", "3Arrows"),
    ("3 Arrows (Gray)", "3ArrowsGray"),
    ("3 Triangles", "3Triangles"),
    ("4 Arrows", "4Arrows"),
    ("5 Arrows", "5Arrows"),
    ("3 Traffic Lights", "3TrafficLights1"),
    ("4 Traffic Lights", "4TrafficLights"),
    ("3 Signs", "3Signs"),
    ("3 Symbols (Circled)", "3Symbols"),
    ("3 Symbols", "3Symbols2"),
    ("3 Flags", "3Flags"),
    ("5 Ratings", "5Rating"),
];

fn rating(set: &str) -> Option<(ironcalc_base::cf_types::Icon, usize)> {
    use ironcalc_base::cf_types::Icon;
    match set {
        "3Stars" => Some((Icon::Star, 3)),
        "4Rating" => Some((Icon::FlatRectangle, 4)),
        "5Rating" => Some((Icon::FlatRectangle, 5)),
        "5Quarters" => Some((Icon::Circle, 5)),
        _ => None,
    }
}

/// A rule, as Kantor edits it.
#[derive(Debug, Clone, PartialEq)]
pub struct CfRuleDef {
    pub spec: CfSpec,
    pub format: CfFormat,
    pub stop_if_true: bool,
}

/// A rule of a sheet, as Manage Rules lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct CfRuleInfo {
    /// Where IronCalc stores it (for edits).
    pub index: usize,
    /// `A1:B10 D1:D5`.
    pub range: String,
    /// `None` for kinds Kantor can't edit (shown, deleted, reordered).
    pub def: Option<CfRuleDef>,
    /// "Cell Value > 100", "Data Bar"...
    pub summary: String,
    /// What it draws when it applies, for the list.
    pub format: CfFormat,
    pub stop_if_true: bool,
}

/// A value typed for a comparison: numbers and formulas as they are (a
/// leading `=` dropped), anything else as quoted text.
fn operand(text: &str) -> String {
    let t = text.trim();
    if let Some(f) = t.strip_prefix('=') {
        return f.to_owned();
    }
    if t.parse::<f64>().is_ok() || (t.starts_with('"') && t.ends_with('"') && t.len() >= 2) {
        return t.to_owned();
    }
    format!("\"{}\"", t.replace('"', "\"\""))
}

/// The other way: `"text"` → `text`, a number as it is, a formula with `=`.
fn shown_operand(f: &str) -> String {
    let t = f.trim();
    if t.starts_with('"') && t.ends_with('"') && t.len() >= 2 {
        return t[1..t.len() - 1].replace("\"\"", "\"");
    }
    if t.parse::<f64>().is_ok() {
        return t.to_owned();
    }
    format!("={t}")
}

fn input(def: &CfRuleDef) -> Result<CfRuleInput> {
    let format = def.format.dxf();
    let stop_if_true = def.stop_if_true;
    Ok(match &def.spec {
        CfSpec::CellValue { op, value, value2 } => {
            if value.trim().is_empty() || (op.two_values() && value2.trim().is_empty()) {
                bail!("Type the value to compare with");
            }
            CfRuleInput::CellIs {
                operator: op.op(),
                formula: operand(value),
                formula2: op.two_values().then(|| operand(value2)),
                format,
                stop_if_true,
            }
        }
        CfSpec::Text { op, text } => {
            if text.is_empty() {
                bail!("Type the text to look for");
            }
            CfRuleInput::Text {
                operator: op.op(),
                value: text.clone(),
                format,
                stop_if_true,
            }
        }
        CfSpec::Dates { period } => CfRuleInput::TimePeriod {
            time_period: period.period(),
            date1: None,
            date2: None,
            format,
            stop_if_true,
        },
        CfSpec::Duplicates { unique: false } => CfRuleInput::DuplicateValues {
            format,
            stop_if_true,
        },
        CfSpec::Duplicates { unique: true } => CfRuleInput::UniqueValues {
            format,
            stop_if_true,
        },
        CfSpec::TopBottom {
            bottom,
            rank,
            percent,
        } => {
            if *rank == 0 || (*percent && *rank > 100) || *rank > 1000 {
                bail!("Give a number of items (1-1000) or a percentage (1-100)");
            }
            if *bottom {
                CfRuleInput::Bottom10 {
                    rank: *rank,
                    percent: *percent,
                    format,
                    stop_if_true,
                }
            } else {
                CfRuleInput::Top10 {
                    rank: *rank,
                    percent: *percent,
                    format,
                    stop_if_true,
                }
            }
        }
        CfSpec::Average { below: false } => CfRuleInput::AboveAverage {
            format,
            stop_if_true,
        },
        CfSpec::Average { below: true } => CfRuleInput::BelowAverage {
            format,
            stop_if_true,
        },
        CfSpec::Blanks { not: false } => CfRuleInput::Blanks {
            format,
            stop_if_true,
        },
        CfSpec::Blanks { not: true } => CfRuleInput::NotBlanks {
            format,
            stop_if_true,
        },
        CfSpec::Formula { formula } => {
            let f = formula.trim().trim_start_matches('=').trim();
            if f.is_empty() {
                bail!("Type a formula, such as $B2>100");
            }
            CfRuleInput::Formula {
                formula: f.to_owned(),
                format,
                stop_if_true,
            }
        }
        CfSpec::DataBar { color: c, gradient } => CfRuleInput::DataBar {
            min: None,
            max: None,
            positive_color: color(*c),
            negative_color: color(0xFF0000),
            is_gradient: *gradient,
            show_value: true,
        },
        CfSpec::ColorScale { colors } => {
            if !(2..=3).contains(&colors.len()) {
                bail!("A colour scale has two or three colours");
            }
            let cfvos = if colors.len() == 2 {
                vec![Cfvo::Min, Cfvo::Max]
            } else {
                vec![Cfvo::Min, Cfvo::Percentile(50.0), Cfvo::Max]
            };
            CfRuleInput::ColorScale {
                thresholds: cfvos
                    .into_iter()
                    .zip(colors)
                    .map(|(cfvo, c)| ColorScaleThreshold {
                        cfvo,
                        color: color(*c),
                    })
                    .collect(),
            }
        }
        CfSpec::IconSet {
            set,
            reverse,
            show_value,
        } => {
            if let Some((icon, n)) = rating(set) {
                CfRuleInput::IconRating {
                    icon,
                    color: Color::Rgb("#FFC000".into()),
                    thresholds: (0..n)
                        .map(|k| (Cfvo::Percent((100 * k / n) as f64), true))
                        .collect(),
                    show_value: *show_value,
                }
            } else {
                let mut icons =
                    icon_set_icons(set).ok_or_else(|| anyhow!("unknown icon set {set}"))?;
                if *reverse {
                    icons.reverse();
                }
                let n = icons.len();
                CfRuleInput::IconSet {
                    thresholds: icons
                        .into_iter()
                        .enumerate()
                        .map(|(k, (icon, color))| IconThreshold {
                            icon,
                            cfvo: Cfvo::Percent(((100 * k) as f64 / n as f64).round()),
                            color,
                            is_strict: true,
                        })
                        .collect(),
                    show_value: *show_value,
                }
            }
        }
    })
}

/// The rule as Kantor edits it, and a one-line summary.
fn describe(
    rule: &CfRule,
    dxf: Option<&Dxf>,
    model: &ironcalc_base::Model,
) -> (Option<CfRuleDef>, String) {
    let format = dxf
        .map(|d| CfFormat::from_dxf(d, model))
        .unwrap_or_default();
    let def = |spec: CfSpec, stop: bool| {
        Some(CfRuleDef {
            spec,
            format,
            stop_if_true: stop,
        })
    };
    match rule {
        CfRule::CellIs {
            operator,
            formula,
            formula2,
            stop_if_true,
            ..
        } => {
            let op = CfCompare::from_op(operator);
            let value = shown_operand(formula);
            let value2 = formula2.as_deref().map(shown_operand).unwrap_or_default();
            let summary = if op.two_values() {
                format!("Cell value {} {value} and {value2}", op.label())
            } else {
                format!("Cell value {} {value}", op.label())
            };
            (
                def(CfSpec::CellValue { op, value, value2 }, *stop_if_true),
                summary,
            )
        }
        CfRule::Text {
            operator,
            value,
            stop_if_true,
            ..
        } => {
            let op = match operator {
                TextOperator::Contains => Some(CfTextOp::Contains),
                TextOperator::DoesNotContain => Some(CfTextOp::DoesNotContain),
                TextOperator::BeginsWith => Some(CfTextOp::BeginsWith),
                TextOperator::EndsWith => Some(CfTextOp::EndsWith),
                TextOperator::Equals => None,
            };
            match op {
                Some(op) => (
                    def(
                        CfSpec::Text {
                            op,
                            text: value.clone(),
                        },
                        *stop_if_true,
                    ),
                    format!("Text {} \"{value}\"", op.label()),
                ),
                None => (None, format!("Text equal to \"{value}\"")),
            }
        }
        CfRule::TimePeriod {
            time_period,
            stop_if_true,
            ..
        } => match CfPeriod::from_period(time_period) {
            Some(p) => (
                def(CfSpec::Dates { period: p }, *stop_if_true),
                format!("Date occurring: {}", p.label().to_lowercase()),
            ),
            None => (None, "Date occurring".into()),
        },
        CfRule::DuplicateValues { stop_if_true, .. } => (
            def(CfSpec::Duplicates { unique: false }, *stop_if_true),
            "Duplicate values".into(),
        ),
        CfRule::UniqueValues { stop_if_true, .. } => (
            def(CfSpec::Duplicates { unique: true }, *stop_if_true),
            "Unique values".into(),
        ),
        CfRule::Top10 {
            rank,
            percent,
            stop_if_true,
            ..
        }
        | CfRule::Bottom10 {
            rank,
            percent,
            stop_if_true,
            ..
        } => {
            let bottom = matches!(rule, CfRule::Bottom10 { .. });
            let summary = format!(
                "{} {rank}{}",
                if bottom { "Bottom" } else { "Top" },
                if *percent { "%" } else { "" }
            );
            (
                def(
                    CfSpec::TopBottom {
                        bottom,
                        rank: *rank,
                        percent: *percent,
                    },
                    *stop_if_true,
                ),
                summary,
            )
        }
        CfRule::AboveAverage { stop_if_true, .. } => (
            def(CfSpec::Average { below: false }, *stop_if_true),
            "Above average".into(),
        ),
        CfRule::BelowAverage { stop_if_true, .. } => (
            def(CfSpec::Average { below: true }, *stop_if_true),
            "Below average".into(),
        ),
        CfRule::Blanks { stop_if_true, .. } => (
            def(CfSpec::Blanks { not: false }, *stop_if_true),
            "Blank cells".into(),
        ),
        CfRule::NotBlanks { stop_if_true, .. } => (
            def(CfSpec::Blanks { not: true }, *stop_if_true),
            "Cells with something in them".into(),
        ),
        CfRule::Errors { .. } => (None, "Errors".into()),
        CfRule::NoErrors { .. } => (None, "No errors".into()),
        CfRule::Formula {
            formula,
            stop_if_true,
            ..
        } => (
            def(
                CfSpec::Formula {
                    formula: format!("={}", formula.trim_start_matches('=')),
                },
                *stop_if_true,
            ),
            format!("Formula: ={}", formula.trim_start_matches('=')),
        ),
        CfRule::DataBar {
            positive_color,
            is_gradient,
            ..
        } => {
            let c = parse_rgb(&positive_color.to_rgb(&model.workbook.theme)).unwrap_or(0x638EC6);
            (
                Some(CfRuleDef {
                    spec: CfSpec::DataBar {
                        color: c,
                        gradient: *is_gradient,
                    },
                    format: CfFormat::default(),
                    stop_if_true: false,
                }),
                "Data bar".into(),
            )
        }
        CfRule::ColorScale { thresholds } => {
            let colors: Vec<Rgb> = thresholds
                .iter()
                .filter_map(|t| parse_rgb(&t.color.to_rgb(&model.workbook.theme)))
                .collect();
            let n = thresholds.len();
            (
                (colors.len() == n && (2..=3).contains(&n)).then(|| CfRuleDef {
                    spec: CfSpec::ColorScale { colors },
                    format: CfFormat::default(),
                    stop_if_true: false,
                }),
                format!("{n}-color scale"),
            )
        }
        CfRule::IconSet { thresholds, .. } => {
            (None, format!("Icon set ({} icons)", thresholds.len()))
        }
        CfRule::IconRating { thresholds, .. } => {
            (None, format!("Rating ({} steps)", thresholds.len()))
        }
    }
}

fn stop_of(rule: &CfRule) -> bool {
    match rule {
        CfRule::CellIs { stop_if_true, .. }
        | CfRule::Formula { stop_if_true, .. }
        | CfRule::Text { stop_if_true, .. }
        | CfRule::TimePeriod { stop_if_true, .. }
        | CfRule::DuplicateValues { stop_if_true, .. }
        | CfRule::UniqueValues { stop_if_true, .. }
        | CfRule::Blanks { stop_if_true, .. }
        | CfRule::NotBlanks { stop_if_true, .. }
        | CfRule::Errors { stop_if_true, .. }
        | CfRule::NoErrors { stop_if_true, .. }
        | CfRule::AboveAverage { stop_if_true, .. }
        | CfRule::BelowAverage { stop_if_true, .. }
        | CfRule::Top10 { stop_if_true, .. }
        | CfRule::Bottom10 { stop_if_true, .. } => *stop_if_true,
        _ => false,
    }
}

/// `A1:B5 D2` for a list of ranges.
pub fn sqref_of(ranges: &[CellRange]) -> String {
    ranges
        .iter()
        .map(|r| {
            if r.is_single() {
                crate::cell_name(r.top, r.left)
            } else {
                format!(
                    "{}:{}",
                    crate::cell_name(r.top, r.left),
                    crate::cell_name(r.bottom, r.right)
                )
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl Workbook {
    /// The conditional formatting rules of `sheet` in Excel's priority order
    /// (the first wins), only those touching `within` when given.
    pub fn cf_rules(&self, sheet: u32, within: Option<CellRange>) -> Vec<CfRuleInfo> {
        let Ok(list) = self.model.get_conditional_formatting_list(sheet) else {
            return Vec::new();
        };
        let model = self.model.get_model();
        list.into_iter()
            .filter(|v| match within {
                None => true,
                Some(w) => parse_sqref(&v.range).iter().any(|r| r.intersects(&w)),
            })
            .map(|v| {
                let dxf = self
                    .model
                    .get_dxf_for_conditional_formatting(sheet, v.index as u32)
                    .ok()
                    .flatten();
                let (def, summary) = describe(&v.cf_rule, dxf.as_ref(), model);
                let format = dxf
                    .as_ref()
                    .map(|d| CfFormat::from_dxf(d, model))
                    .unwrap_or_default();
                CfRuleInfo {
                    index: v.index,
                    range: v.range,
                    def,
                    summary,
                    format,
                    stop_if_true: stop_of(&v.cf_rule),
                }
            })
            .collect()
    }

    fn check_sqref(&self, range: &str) -> Result<String> {
        let ranges = parse_sqref(&range.replace(',', " ").replace('$', "").to_uppercase());
        if ranges.is_empty() {
            bail!("Type the cells it applies to, such as A2:A20");
        }
        Ok(sqref_of(&ranges))
    }

    fn mark_cf(&mut self, sheet: u32) {
        let id = self.sheet_id(sheet);
        self.own_edit(|e, _| {
            e.cf_sheets.insert(id);
        });
    }

    /// Add a rule on `range` (`A1:B5 D2`), at the top of the priority list,
    /// as Excel does. One undo step.
    pub fn add_cf_rule(&mut self, sheet: u32, range: &str, rule: &CfRuleDef) -> Result<()> {
        let range = self.check_sqref(range)?;
        let input = input(rule)?;
        self.batch(|wb| {
            wb.model
                .add_conditional_formatting(sheet, &range, input)
                .map_err(|e| anyhow!("{e}"))?;
            wb.history.edited();
            wb.mark_cf(sheet);
            Ok(())
        })
    }

    /// Replace the rule stored at `index` (range and rule; its priority
    /// stays). One undo step.
    pub fn update_cf_rule(
        &mut self,
        sheet: u32,
        index: usize,
        range: &str,
        rule: &CfRuleDef,
    ) -> Result<()> {
        let range = self.check_sqref(range)?;
        let input = input(rule)?;
        self.batch(|wb| {
            wb.model
                .update_conditional_formatting(sheet, index as u32, &range, input)
                .map_err(|e| anyhow!("{e}"))?;
            wb.history.edited();
            wb.mark_cf(sheet);
            Ok(())
        })
    }

    /// Delete the rule stored at `index`. One undo step.
    pub fn delete_cf_rule(&mut self, sheet: u32, index: usize) -> Result<()> {
        self.batch(|wb| {
            wb.model
                .delete_conditional_formatting(sheet, index as u32)
                .map_err(|e| anyhow!("{e}"))?;
            wb.history.edited();
            wb.mark_cf(sheet);
            Ok(())
        })
    }

    /// Move the rule stored at `index` one place up (sooner) or down in the
    /// priority list. One undo step; nothing at either end.
    pub fn move_cf_rule(&mut self, sheet: u32, index: usize, up: bool) -> Result<()> {
        let list = self.cf_rules(sheet, None);
        let at = list
            .iter()
            .position(|r| r.index == index)
            .ok_or_else(|| anyhow!("no rule {index}"))?;
        if (up && at == 0) || (!up && at + 1 == list.len()) {
            return Ok(());
        }
        self.batch(|wb| {
            let r = if up {
                wb.model
                    .raise_conditional_formatting_priority(sheet, index as u32)
            } else {
                wb.model
                    .lower_conditional_formatting_priority(sheet, index as u32)
            };
            r.map_err(|e| anyhow!("{e}"))?;
            wb.history.edited();
            wb.mark_cf(sheet);
            Ok(())
        })
    }

    /// Delete every rule of `sheet` touching `within` (every rule when
    /// `None`): Clear Rules. One undo step; returns how many went.
    pub fn clear_cf_rules(&mut self, sheet: u32, within: Option<CellRange>) -> Result<usize> {
        let mut indexes: Vec<usize> = self
            .cf_rules(sheet, within)
            .iter()
            .map(|r| r.index)
            .collect();
        if indexes.is_empty() {
            return Ok(0);
        }
        // Highest index first, so the others stay where they are.
        indexes.sort_unstable_by(|a, b| b.cmp(a));
        let n = indexes.len();
        self.batch(|wb| {
            for i in indexes {
                wb.model
                    .delete_conditional_formatting(sheet, i as u32)
                    .map_err(|e| anyhow!("{e}"))?;
                wb.history.edited();
            }
            wb.mark_cf(sheet);
            Ok(())
        })?;
        Ok(n)
    }

    /// Sheets whose rules were changed in Kantor (their markup is written
    /// from the model at save).
    pub(crate) fn cf_patches(&self, out: &mut crate::preserve::Patches) {
        out.cf_sheets.extend(self.edits().cf_sheets.iter().copied());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operands() {
        assert_eq!(operand("100"), "100");
        assert_eq!(operand("=$B$1"), "$B$1");
        assert_eq!(operand("Paid"), "\"Paid\"");
        assert_eq!(shown_operand("\"Paid\""), "Paid");
        assert_eq!(shown_operand("$B$1"), "=$B$1");
        assert_eq!(shown_operand("2.5"), "2.5");
        assert_eq!(
            sqref_of(&[
                CellRange::from_corners((0, 0), (4, 1)),
                CellRange::from_corners((2, 3), (2, 3))
            ]),
            "A1:B5 D3"
        );
    }
}
