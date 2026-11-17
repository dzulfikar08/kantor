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
use ironcalc_base::types::{Border, BorderItem, BorderStyle, Color, Dxf, DxfFont, Fill, NumFmt};

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
    /// The whole text (any case).
    Equals,
}

impl CfTextOp {
    pub const ALL: [CfTextOp; 5] = [
        CfTextOp::Contains,
        CfTextOp::DoesNotContain,
        CfTextOp::BeginsWith,
        CfTextOp::EndsWith,
        CfTextOp::Equals,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CfTextOp::Contains => "containing",
            CfTextOp::DoesNotContain => "not containing",
            CfTextOp::BeginsWith => "beginning with",
            CfTextOp::EndsWith => "ending with",
            CfTextOp::Equals => "equal to",
        }
    }

    fn op(self) -> TextOperator {
        match self {
            CfTextOp::Contains => TextOperator::Contains,
            CfTextOp::DoesNotContain => TextOperator::DoesNotContain,
            CfTextOp::BeginsWith => TextOperator::BeginsWith,
            CfTextOp::EndsWith => TextOperator::EndsWith,
            CfTextOp::Equals => TextOperator::Equals,
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
        /// Edit Formatting Rule's bar settings (shortest and longest bar,
        /// border, negative bars, axis, direction, bar only).
        bar: CfBarOptions,
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
        /// Each icon and when it shows, lowest first (as `iconId`); empty:
        /// the set's own icons at 0, 33, 67 % (or quarters, fifths), `>=`.
        /// `steps[0]`'s value isn't used (the lowest icon takes the rest).
        steps: Vec<CfIconStep>,
    },
}

/// How a data bar's shortest or longest bar, or an icon's threshold, is
/// measured (Excel's "Type").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfValueKind {
    /// Data bars only: lowest (or highest) value, or 0 when that is nearer.
    Automatic,
    Lowest,
    Highest,
    Number,
    Percent,
    Percentile,
    Formula,
}

impl CfValueKind {
    /// The types a data bar's minimum offers (its maximum: `Highest` in
    /// place of `Lowest`).
    pub const BAR: [CfValueKind; 6] = [
        CfValueKind::Automatic,
        CfValueKind::Lowest,
        CfValueKind::Number,
        CfValueKind::Percent,
        CfValueKind::Formula,
        CfValueKind::Percentile,
    ];
    /// The types an icon's threshold offers.
    pub const ICON: [CfValueKind; 4] = [
        CfValueKind::Number,
        CfValueKind::Percent,
        CfValueKind::Formula,
        CfValueKind::Percentile,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CfValueKind::Automatic => "Automatic",
            CfValueKind::Lowest => "Lowest Value",
            CfValueKind::Highest => "Highest Value",
            CfValueKind::Number => "Number",
            CfValueKind::Percent => "Percent",
            CfValueKind::Percentile => "Percentile",
            CfValueKind::Formula => "Formula",
        }
    }

    /// Whether a value is typed for it.
    pub fn has_value(self) -> bool {
        !matches!(
            self,
            CfValueKind::Automatic | CfValueKind::Lowest | CfValueKind::Highest
        )
    }
}

/// A data bar end or an icon threshold: its type and the value typed (a
/// number, or a formula with or without `=`).
#[derive(Debug, Clone, PartialEq)]
pub struct CfValue {
    pub kind: CfValueKind,
    pub value: String,
}

impl CfValue {
    pub fn new(kind: CfValueKind, value: impl Into<String>) -> Self {
        Self {
            kind,
            value: value.into(),
        }
    }

    /// IronCalc's form (`None`: automatic).
    fn cfvo(&self, what: &str) -> Result<Option<Cfvo>> {
        let num = || -> Result<f64> {
            self.value
                .trim()
                .parse::<f64>()
                .map_err(|_| anyhow!("{what}: type a number"))
        };
        Ok(Some(match self.kind {
            CfValueKind::Automatic => return Ok(None),
            CfValueKind::Lowest => Cfvo::Min,
            CfValueKind::Highest => Cfvo::Max,
            CfValueKind::Number => Cfvo::Number(num()?),
            CfValueKind::Percent => Cfvo::Percent(num()?),
            CfValueKind::Percentile => Cfvo::Percentile(num()?),
            CfValueKind::Formula => {
                let f = self.value.trim().trim_start_matches('=').trim();
                if f.is_empty() {
                    bail!("{what}: type a formula");
                }
                Cfvo::Formula(f.to_owned())
            }
        }))
    }

    fn of_cfvo(c: Option<&Cfvo>) -> Self {
        let n = |v: &f64| format_number(*v);
        match c {
            None => CfValue::new(CfValueKind::Automatic, ""),
            Some(Cfvo::Min) => CfValue::new(CfValueKind::Lowest, ""),
            Some(Cfvo::Max) => CfValue::new(CfValueKind::Highest, ""),
            Some(Cfvo::Number(v)) => CfValue::new(CfValueKind::Number, n(v)),
            Some(Cfvo::Percent(v)) => CfValue::new(CfValueKind::Percent, n(v)),
            Some(Cfvo::Percentile(v)) => CfValue::new(CfValueKind::Percentile, n(v)),
            Some(Cfvo::Formula(f)) => CfValue::new(CfValueKind::Formula, format!("={f}")),
        }
    }
}

fn format_number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// Where a data bar's axis goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CfBarAxis {
    /// At zero, placed by the values (Excel's "Automatic").
    #[default]
    Automatic,
    /// In the middle of the cell.
    Middle,
    /// None: negative bars run the same way as positive ones.
    None,
}

/// Which way data bars grow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CfBarDirection {
    /// As the sheet reads (left to right).
    #[default]
    Context,
    LeftToRight,
    RightToLeft,
}

/// Edit Formatting Rule's settings of a data bar beyond its colour.
#[derive(Debug, Clone, PartialEq)]
pub struct CfBarOptions {
    pub min: CfValue,
    pub max: CfValue,
    /// Shortest and longest bar, percent of the cell's width.
    pub min_length: u8,
    pub max_length: u8,
    /// A solid border in this colour.
    pub border: Option<Rgb>,
    /// Negative bars' fill (`None`: the positive colour).
    pub negative_fill: Option<Rgb>,
    /// Negative bars' border (`None`: the positive border's).
    pub negative_border: Option<Rgb>,
    pub axis: CfBarAxis,
    pub axis_color: Rgb,
    pub direction: CfBarDirection,
    /// Show the bar only, not the value.
    pub bar_only: bool,
}

impl Default for CfBarOptions {
    /// Excel 2010's defaults (lengths 0 to 100 %, red negative bars).
    fn default() -> Self {
        Self {
            min: CfValue::new(CfValueKind::Automatic, ""),
            max: CfValue::new(CfValueKind::Automatic, ""),
            min_length: 0,
            max_length: 100,
            border: None,
            negative_fill: Some(0xFF0000),
            negative_border: None,
            axis: CfBarAxis::Automatic,
            axis_color: 0x000000,
            direction: CfBarDirection::Context,
            bar_only: false,
        }
    }
}

impl CfBarOptions {
    /// The look drawn and saved for it.
    fn look(&self) -> crate::cf_x14::BarLook {
        use crate::cf_x14::{Axis, BarLook, Direction};
        BarLook {
            min_len: f32::from(self.min_length.min(100)) / 100.0,
            max_len: f32::from(self.max_length.min(100)) / 100.0,
            border: self.border.map(color),
            negative_border: self.negative_border.map(color),
            negative_fill: None,
            axis: match self.axis {
                CfBarAxis::Automatic => Axis::Automatic,
                CfBarAxis::Middle => Axis::Middle,
                CfBarAxis::None => Axis::None,
            },
            axis_color: Some(color(self.axis_color)),
            direction: match self.direction {
                CfBarDirection::Context => Direction::Context,
                CfBarDirection::LeftToRight => Direction::LeftToRight,
                CfBarDirection::RightToLeft => Direction::RightToLeft,
            },
        }
    }
}

/// An icon of Excel's icon sets: the set and its place in it (0: the
/// lowest value's icon), as `<x14:cfIcon>` names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CfIconRef {
    pub set: String,
    pub index: u8,
}

impl CfIconRef {
    /// Excel's "No Cell Icon" (`<x14:cfIcon iconSet="NoIcons" iconId="0"/>`).
    pub fn none() -> Self {
        CfIconRef {
            set: NO_ICONS.to_owned(),
            index: 0,
        }
    }

    pub fn is_none(&self) -> bool {
        self.set == NO_ICONS
    }

    /// The model's icon and colour for it (no colour: no icon).
    fn model_icon(&self) -> Option<(ironcalc_base::cf_types::Icon, Color)> {
        if self.is_none() {
            return Some((ironcalc_base::cf_types::Icon::Circle, Color::None));
        }
        icon_set_icons(&self.set).and_then(|v| v.into_iter().nth(self.index as usize))
    }
}

/// The icon "set" Excel names "No Cell Icon" with.
pub const NO_ICONS: &str = "NoIcons";

/// One icon of an icon set rule and the value from which it shows.
#[derive(Debug, Clone, PartialEq)]
pub struct CfIconStep {
    pub icon: CfIconRef,
    pub value: CfValue,
    /// `>=` (else `>`).
    pub gte: bool,
}

/// Icon sets whose icons can be picked one by one (Excel's icon list).
const ICON_SOURCES: [&str; 16] = [
    "3Arrows",
    "3ArrowsGray",
    "4Arrows",
    "4ArrowsGray",
    "5Arrows",
    "5ArrowsGray",
    "3Triangles",
    "3TrafficLights1",
    "3TrafficLights2",
    "4TrafficLights",
    "3Signs",
    "4RedToBlack",
    "3Symbols",
    "3Symbols2",
    "3Flags",
    "3Stars",
];

/// Every icon offered for a single step, each once (set, index), after
/// "No Cell Icon" ([`CfIconRef::none`]).
pub fn icon_choices() -> Vec<CfIconRef> {
    let mut seen = Vec::new();
    let mut out = vec![CfIconRef::none()];
    for set in ICON_SOURCES {
        let Some(icons) = icon_set_icons(set) else {
            continue;
        };
        for (i, pair) in icons.into_iter().enumerate() {
            if !seen.contains(&pair) {
                seen.push(pair);
                out.push(CfIconRef {
                    set: set.to_owned(),
                    index: i as u8,
                });
            }
        }
    }
    out
}

/// The default steps of icon set `set` (`n` icons): the set's own icons at
/// even percentages, `>=`.
pub fn default_icon_steps(set: &str) -> Vec<CfIconStep> {
    let n = rating(set)
        .map(|(_, n)| n)
        .or_else(|| icon_set_icons(set).map(|v| v.len()))
        .unwrap_or(3);
    (0..n)
        .map(|k| CfIconStep {
            icon: CfIconRef {
                set: set.to_owned(),
                index: k as u8,
            },
            value: CfValue::new(
                CfValueKind::Percent,
                format_number(((100 * k) as f64 / n as f64).round()),
            ),
            gte: true,
        })
        .collect()
}

impl CfSpec {
    /// Whether icon set `set` is a rating (stars, bars, quarters): one icon
    /// filled in steps, not picked icon by icon.
    pub fn is_rating(set: &str) -> bool {
        rating(set).is_some()
    }

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

/// A rule made or changed in Kantor with Excel 2010 markup of its own: a
/// data bar's look (`<x14:dataBar>`), or a custom icon set (the whole
/// `<x14:conditionalFormatting>`). It belongs to the model rule equal to
/// `rule` on its sheet (any range when none matches `range`: rows
/// inserted since).
#[derive(Debug, Clone)]
pub(crate) struct CfLook {
    pub sheet_id: u32,
    pub range: String,
    pub rule: CfRule,
    pub look: Option<crate::cf_x14::BarLook>,
    pub xml: String,
    pub data_bar: bool,
}

/// A rule, as Kantor edits it.
#[derive(Debug, Clone, PartialEq)]
pub struct CfRuleDef {
    pub spec: CfSpec,
    pub format: CfFormat,
    pub stop_if_true: bool,
    /// The number format the rule shows its cells with (`0.00%`).
    pub num_fmt: Option<String>,
    /// The borders it draws, as the file (or Format Cells) had them.
    pub border: Option<Border>,
}

/// A rule's border all round (Format Cells' Outline): thin, `color`.
pub fn outline_border(color: Rgb) -> Border {
    let side = || {
        Some(BorderItem {
            style: BorderStyle::Thin,
            color: Color::Rgb(format!("#{:06X}", color & 0xFF_FFFF)),
        })
    };
    Border {
        diagonal_up: false,
        diagonal_down: false,
        left: side(),
        right: side(),
        top: side(),
        bottom: side(),
        diagonal: None,
    }
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

fn input(def: &CfRuleDef, num_fmt_id: &dyn Fn(&str) -> i32) -> Result<CfRuleInput> {
    let mut format = def.format.dxf();
    format.num_fmt = def
        .num_fmt
        .as_ref()
        .filter(|c| !c.trim().is_empty())
        .map(|code| NumFmt {
            num_fmt_id: num_fmt_id(code),
            format_code: code.clone(),
        });
    format.border = def.border.clone();
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
        CfSpec::DataBar {
            color: c,
            gradient,
            bar,
        } => CfRuleInput::DataBar {
            min: bar.min.cfvo("Minimum")?,
            max: bar.max.cfvo("Maximum")?,
            positive_color: color(*c),
            negative_color: color(bar.negative_fill.unwrap_or(*c)),
            is_gradient: *gradient,
            show_value: !bar.bar_only,
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
            steps,
        } => {
            let steps = if steps.is_empty() {
                default_icon_steps(set)
            } else {
                steps.clone()
            };
            let cfvo = |k: usize, s: &CfIconStep| -> Result<Cfvo> {
                if k == 0 {
                    // The lowest icon takes everything below the next.
                    return Ok(Cfvo::Percent(0.0));
                }
                let what = format!("Icon {}", steps.len() - k);
                s.value
                    .cfvo(&what)?
                    .ok_or_else(|| anyhow!("{what}: pick a type"))
            };
            if let Some((icon, n)) = rating(set) {
                if steps.len() != n {
                    bail!("{set} has {n} steps");
                }
                CfRuleInput::IconRating {
                    icon,
                    color: Color::Rgb("#FFC000".into()),
                    thresholds: steps
                        .iter()
                        .enumerate()
                        .map(|(k, s)| Ok((cfvo(k, s)?, s.gte)))
                        .collect::<Result<_>>()?,
                    show_value: *show_value,
                }
            } else {
                let mut out = Vec::with_capacity(steps.len());
                for (k, s) in steps.iter().enumerate() {
                    // Reverse Icon Order: the icons swap ends, the values stay.
                    let pick = if *reverse {
                        &steps[steps.len() - 1 - k].icon
                    } else {
                        &s.icon
                    };
                    let (icon, color) = pick
                        .model_icon()
                        .ok_or_else(|| anyhow!("unknown icon {}/{}", pick.set, pick.index))?;
                    out.push(IconThreshold {
                        icon,
                        cfvo: cfvo(k, s)?,
                        color,
                        is_strict: s.gte,
                    });
                }
                CfRuleInput::IconSet {
                    thresholds: out,
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
    look: Option<crate::cf_x14::BarLook>,
) -> (Option<CfRuleDef>, String) {
    let format = dxf
        .map(|d| CfFormat::from_dxf(d, model))
        .unwrap_or_default();
    let num_fmt = dxf
        .and_then(|d| d.num_fmt.as_ref())
        .map(|n| n.format_code.clone());
    let border = dxf.and_then(|d| d.border.clone());
    let def = |spec: CfSpec, stop: bool| {
        Some(CfRuleDef {
            spec,
            format,
            stop_if_true: stop,
            num_fmt: num_fmt.clone(),
            border: border.clone(),
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
                TextOperator::Equals => Some(CfTextOp::Equals),
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
            min,
            max,
            positive_color,
            negative_color,
            is_gradient,
            show_value,
        } => {
            use crate::cf_x14::{Axis, Direction};
            let theme = &model.workbook.theme;
            let rgb = |c: &Color| parse_rgb(&c.to_rgb(theme));
            let c = rgb(positive_color).unwrap_or(0x638EC6);
            let look = look.unwrap_or(crate::cf_x14::BarLook::KANTOR);
            let negative = look
                .negative_fill
                .as_ref()
                .and_then(rgb)
                .or_else(|| rgb(negative_color));
            let pct = |f: f32| (f * 100.0).round().clamp(0.0, 100.0) as u8;
            let bar = CfBarOptions {
                min: CfValue::of_cfvo(min.as_ref()),
                max: CfValue::of_cfvo(max.as_ref()),
                min_length: pct(look.min_len),
                max_length: pct(look.max_len),
                border: look.border.as_ref().and_then(rgb),
                negative_fill: negative.filter(|n| *n != c),
                negative_border: look.negative_border.as_ref().and_then(rgb),
                axis: match look.axis {
                    Axis::Automatic => CfBarAxis::Automatic,
                    Axis::Middle => CfBarAxis::Middle,
                    Axis::None => CfBarAxis::None,
                },
                axis_color: look.axis_color.as_ref().and_then(rgb).unwrap_or(0),
                direction: match look.direction {
                    Direction::Context => CfBarDirection::Context,
                    Direction::LeftToRight => CfBarDirection::LeftToRight,
                    Direction::RightToLeft => CfBarDirection::RightToLeft,
                },
                bar_only: !show_value,
            };
            (
                Some(CfRuleDef {
                    num_fmt: None,
                    border: None,
                    spec: CfSpec::DataBar {
                        color: c,
                        gradient: *is_gradient,
                        bar,
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
                    num_fmt: None,
                    border: None,
                    spec: CfSpec::ColorScale { colors },
                    format: CfFormat::default(),
                    stop_if_true: false,
                }),
                format!("{n}-color scale"),
            )
        }
        CfRule::IconSet {
            thresholds,
            show_value,
        } => {
            let summary = format!("Icon set ({} icons)", thresholds.len());
            let pairs: Vec<_> = thresholds
                .iter()
                .map(|t| (t.icon.clone(), t.color.clone()))
                .collect();
            // The set it is (or reversed), else its first icon's set.
            let mut named = None;
            for (_, name) in ICON_SETS.iter().chain(&[
                ("", "4ArrowsGray"),
                ("", "5ArrowsGray"),
                ("", "3ArrowsGray"),
                ("", "3TrafficLights2"),
                ("", "4RedToBlack"),
                ("", "3Stars"),
            ]) {
                let Some(mut set) = icon_set_icons(name) else {
                    continue;
                };
                if set == pairs {
                    named = Some((*name, false));
                    break;
                }
                set.reverse();
                if set == pairs {
                    named = Some((*name, true));
                    break;
                }
            }
            let choices = icon_choices();
            let find = |p: &(ironcalc_base::cf_types::Icon, Color)| {
                choices.iter().find(|c| {
                    if p.1 == Color::None {
                        return c.is_none();
                    }
                    c.model_icon().as_ref() == Some(p)
                })
            };
            let mut steps = Vec::with_capacity(thresholds.len());
            for (k, t) in thresholds.iter().enumerate() {
                let icon = match named {
                    Some((name, rev)) => CfIconRef {
                        set: name.to_owned(),
                        index: if rev {
                            (thresholds.len() - 1 - k) as u8
                        } else {
                            k as u8
                        },
                    },
                    None => match find(&pairs[k]) {
                        Some(c) => c.clone(),
                        None => return (None, summary),
                    },
                };
                steps.push(CfIconStep {
                    icon,
                    value: CfValue::of_cfvo(Some(&t.cfvo)),
                    gte: t.is_strict,
                });
            }
            let set = named.map(|n| n.0.to_owned()).unwrap_or_else(|| {
                steps
                    .first()
                    .map(|s| s.icon.set.clone())
                    .unwrap_or_default()
            });
            // Reverse Icon Order keeps the set's icons in the steps' order.
            let reverse = named.is_some_and(|n| n.1);
            if reverse {
                let icons: Vec<CfIconRef> = steps.iter().rev().map(|s| s.icon.clone()).collect();
                for (s, i) in steps.iter_mut().zip(icons) {
                    s.icon = i;
                }
            }
            (
                Some(CfRuleDef {
                    num_fmt: None,
                    border: None,
                    spec: CfSpec::IconSet {
                        set,
                        reverse,
                        show_value: *show_value,
                        steps,
                    },
                    format: CfFormat::default(),
                    stop_if_true: false,
                }),
                summary,
            )
        }
        CfRule::IconRating {
            icon,
            thresholds,
            show_value,
            ..
        } => {
            let summary = format!("Rating ({} steps)", thresholds.len());
            let Some(set) = ["3Stars", "4Rating", "5Rating", "5Quarters"]
                .into_iter()
                .find(|s| rating(s) == Some((icon.clone(), thresholds.len())))
            else {
                return (None, summary);
            };
            let steps = thresholds
                .iter()
                .enumerate()
                .map(|(k, (cfvo, gte))| CfIconStep {
                    icon: CfIconRef {
                        set: set.to_owned(),
                        index: k as u8,
                    },
                    value: CfValue::of_cfvo(Some(cfvo)),
                    gte: *gte,
                })
                .collect();
            (
                Some(CfRuleDef {
                    num_fmt: None,
                    border: None,
                    spec: CfSpec::IconSet {
                        set: set.to_owned(),
                        reverse: false,
                        show_value: *show_value,
                        steps,
                    },
                    format: CfFormat::default(),
                    stop_if_true: false,
                }),
                summary,
            )
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
                let look = matches!(v.cf_rule, CfRule::DataBar { .. })
                    .then(|| self.bar_look_of(self.sheet_id(sheet), &v.range, &v.cf_rule));
                let (def, summary) = describe(&v.cf_rule, dxf.as_ref(), model, look);
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

    /// The id a rule's number format `code` goes by: the one the workbook
    /// already gives it, else a new one past every id in use.
    fn dxf_num_fmt_id(&self, code: &str) -> i32 {
        let styles = &self.model.get_model().workbook.styles;
        let dxf_fmts = styles.dxfs.iter().filter_map(|d| d.num_fmt.as_ref());
        if let Some(n) = styles
            .num_fmts
            .iter()
            .chain(dxf_fmts.clone())
            .find(|n| n.format_code == code)
        {
            return n.num_fmt_id;
        }
        styles
            .num_fmts
            .iter()
            .chain(dxf_fmts)
            .map(|n| n.num_fmt_id + 1)
            .max()
            .unwrap_or(0)
            .max(164)
    }

    fn check_sqref(&self, range: &str) -> Result<String> {
        let ranges = parse_sqref(&range.replace(',', " ").replace('$', "").to_uppercase());
        if ranges.is_empty() {
            bail!("Type the cells it applies to, such as A2:A20");
        }
        Ok(sqref_of(&ranges))
    }

    pub(crate) fn mark_cf(&mut self, sheet: u32) {
        let id = self.sheet_id(sheet);
        self.own_edit(|e, _| {
            e.cf_sheets.insert(id);
        });
    }

    /// Add a rule on `range` (`A1:B5 D2`), at the top of the priority list,
    /// as Excel does. One undo step.
    pub fn add_cf_rule(&mut self, sheet: u32, range: &str, rule: &CfRuleDef) -> Result<()> {
        let range = self.check_sqref(range)?;
        let input = input(rule, &|code| self.dxf_num_fmt_id(code))?;
        self.batch(|wb| {
            wb.model
                .add_conditional_formatting(sheet, &range, input)
                .map_err(|e| anyhow!("{e}"))?;
            wb.history.edited();
            wb.mark_cf(sheet);
            wb.record_cf_look(sheet, None, rule);
            Ok(())
        })
    }

    /// Remember what Excel 2010's markup must say about the rule just
    /// stored at `index` (`None`: the last one, just added).
    fn record_cf_look(&mut self, sheet: u32, index: Option<usize>, def: &CfRuleDef) {
        let Some(cf) = ({
            let model = self.model.get_model();
            model.workbook.worksheet(sheet).ok().and_then(|ws| {
                let list = &ws.conditional_formatting;
                index.map_or(list.last(), |i| list.get(i)).cloned()
            })
        }) else {
            return;
        };
        let sheet_id = self.sheet_id(sheet);
        let look = match (&def.spec, &cf.cf_rule) {
            (
                CfSpec::DataBar { gradient, bar, .. },
                CfRule::DataBar {
                    min,
                    max,
                    negative_color,
                    ..
                },
            ) => {
                let look = bar.look();
                let xml = look.x14_xml(*gradient, min.as_ref(), max.as_ref(), negative_color);
                Some(CfLook {
                    sheet_id,
                    range: cf.range.clone(),
                    rule: cf.cf_rule.clone(),
                    look: Some(look),
                    xml,
                    data_bar: true,
                })
            }
            (
                CfSpec::IconSet {
                    set,
                    show_value,
                    steps,
                    reverse,
                },
                CfRule::IconSet { thresholds, .. },
            ) if rating(set).is_none() && !steps.is_empty() => {
                // Icons picked one by one: Excel 2010's custom icon set.
                let n = steps.len();
                let own = (0..n).all(|k| {
                    let s = &steps[if *reverse { n - 1 - k } else { k }];
                    let want = if *reverse { n - 1 - k } else { k };
                    s.icon.set == *set && usize::from(s.icon.index) == want
                });
                (!own).then(|| {
                    let list: Vec<_> = thresholds
                        .iter()
                        .enumerate()
                        .map(|(k, t)| {
                            let pick = if *reverse {
                                &steps[n - 1 - k].icon
                            } else {
                                &steps[k].icon
                            };
                            (t.cfvo.clone(), t.is_strict, pick.set.clone(), pick.index)
                        })
                        .collect();
                    CfLook {
                        sheet_id,
                        range: cf.range.clone(),
                        rule: cf.cf_rule.clone(),
                        look: None,
                        xml: crate::cf_x14::custom_icons_xml(&cf.range, set, *show_value, &list),
                        data_bar: false,
                    }
                })
            }
            _ => None,
        };
        if let Some(look) = look {
            self.own_edit(|e, _| e.cf_looks.push(look));
        }
    }

    /// Kantor's own Excel 2010 markup for model rule `rule` over `range` of
    /// sheet `sheet_id` (see [`CfLook`]).
    pub(crate) fn cf_look(&self, sheet_id: u32, range: &str, rule: &CfRule) -> Option<CfLook> {
        let edits = self.edits();
        let mine = edits
            .cf_looks
            .iter()
            .rev()
            .filter(|l| l.sheet_id == sheet_id && l.rule == *rule);
        mine.clone()
            .find(|l| l.range == range)
            .or_else(|| mine.clone().next())
            .cloned()
    }

    /// How data bar `rule` over `range` is drawn: Kantor's look for it,
    /// else the file's while it is as opened, else Excel 2010's defaults.
    pub(crate) fn bar_look_of(
        &self,
        sheet_id: u32,
        range: &str,
        rule: &CfRule,
    ) -> crate::cf_x14::BarLook {
        self.cf_look(sheet_id, range, rule)
            .and_then(|l| l.look)
            .unwrap_or_else(|| self.cf_x14.bar_look(sheet_id, range, rule))
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
        let input = input(rule, &|code| self.dxf_num_fmt_id(code))?;
        self.batch(|wb| {
            wb.model
                .update_conditional_formatting(sheet, index as u32, &range, input)
                .map_err(|e| anyhow!("{e}"))?;
            wb.history.edited();
            wb.mark_cf(sheet);
            wb.record_cf_look(sheet, Some(index), rule);
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
        self.move_cf_rule_within(sheet, index, up, None)
    }

    /// Move the rule stored at `index` past the next rule shown when only
    /// the rules touching `within` are listed (Manage Rules showing the
    /// current selection), as Excel does: rules not shown in between keep
    /// their order and end up on its other side. `None`: every rule is
    /// shown. One undo step; nothing at either end of the shown list.
    pub fn move_cf_rule_within(
        &mut self,
        sheet: u32,
        index: usize,
        up: bool,
        within: Option<CellRange>,
    ) -> Result<()> {
        let all = self.cf_rules(sheet, None);
        let shown: Vec<usize> = self
            .cf_rules(sheet, within)
            .iter()
            .map(|r| r.index)
            .collect();
        let at = all
            .iter()
            .position(|r| r.index == index)
            .ok_or_else(|| anyhow!("no rule {index}"))?;
        let is_shown = |i: usize| shown.contains(&all[i].index);
        // The neighbour in the shown list, and how many places that is.
        let target = if up {
            (0..at).rev().find(|&i| is_shown(i))
        } else {
            (at + 1..all.len()).find(|&i| is_shown(i))
        };
        let Some(target) = target else {
            return Ok(());
        };
        let steps = target.abs_diff(at);
        self.batch(|wb| {
            // Each step swaps priorities with the next rule; stored
            // positions (indexes) don't change.
            for _ in 0..steps {
                let r = if up {
                    wb.model
                        .raise_conditional_formatting_priority(sheet, index as u32)
                } else {
                    wb.model
                        .lower_conditional_formatting_priority(sheet, index as u32)
                };
                r.map_err(|e| anyhow!("{e}"))?;
                wb.history.edited();
            }
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
        out.cf_sheets.extend(self.touched.cf.iter().copied());
        // The file's Excel 2010 extras of rules still as opened.
        let model = self.model.get_model();
        for ws in &model.workbook.worksheets {
            if !out.cf_sheets.contains(&ws.sheet_id) {
                continue;
            }
            let mut kept = self.cf_x14.kept(ws.sheet_id, &ws.conditional_formatting);
            // Kantor's own markup wins over the file's for the same rule.
            for cf in &ws.conditional_formatting {
                let Some(l) = self.cf_look(ws.sheet_id, &cf.range, &cf.cf_rule) else {
                    continue;
                };
                kept.retain(|k| !(k.range == cf.range && k.data_bar == l.data_bar));
                kept.push(crate::cf_x14::KeptX14 {
                    range: cf.range.clone(),
                    old_range: l.range.clone(),
                    data_bar: l.data_bar,
                    bar_xml: l.data_bar.then(|| l.xml.clone()),
                    xml: if l.data_bar {
                        String::new()
                    } else {
                        l.xml.clone()
                    },
                });
            }
            if !kept.is_empty() {
                out.cf_x14.push((ws.sheet_id, kept));
            }
        }
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
