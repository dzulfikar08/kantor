//! Data labels and axis options of a chart, as the Chart panel edits them
//! (Excel's Format Data Labels and Format Axis, the common parts).

/// Where data labels sit (`c:dLblPos`), with Excel's names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelPos {
    Center,
    InsideEnd,
    InsideBase,
    OutsideEnd,
    Left,
    Right,
    Above,
    Below,
    BestFit,
}

impl LabelPos {
    pub const ALL: [LabelPos; 9] = [
        LabelPos::Center,
        LabelPos::InsideEnd,
        LabelPos::InsideBase,
        LabelPos::OutsideEnd,
        LabelPos::Left,
        LabelPos::Right,
        LabelPos::Above,
        LabelPos::Below,
        LabelPos::BestFit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LabelPos::Center => "Center",
            LabelPos::InsideEnd => "Inside End",
            LabelPos::InsideBase => "Inside Base",
            LabelPos::OutsideEnd => "Outside End",
            LabelPos::Left => "Left",
            LabelPos::Right => "Right",
            LabelPos::Above => "Above",
            LabelPos::Below => "Below",
            LabelPos::BestFit => "Best Fit",
        }
    }

    pub(crate) fn code(self) -> &'static str {
        match self {
            LabelPos::Center => "ctr",
            LabelPos::InsideEnd => "inEnd",
            LabelPos::InsideBase => "inBase",
            LabelPos::OutsideEnd => "outEnd",
            LabelPos::Left => "l",
            LabelPos::Right => "r",
            LabelPos::Above => "t",
            LabelPos::Below => "b",
            LabelPos::BestFit => "bestFit",
        }
    }

    pub(crate) fn from_code(c: &str) -> Option<Self> {
        LabelPos::ALL.into_iter().find(|p| p.code() == c)
    }

    /// The positions Excel offers for a plot type (`c:barChart`...) and
    /// grouping; empty: labels can't be placed (area, doughnut, 3-D...).
    pub(crate) fn for_group(tag: &str, stacked: bool) -> &'static [LabelPos] {
        use LabelPos::*;
        match tag {
            "barChart" if stacked => &[Center, InsideEnd, InsideBase],
            "barChart" => &[Center, InsideEnd, InsideBase, OutsideEnd],
            "lineChart" | "scatterChart" | "bubbleChart" | "stockChart" => {
                &[Center, Left, Right, Above, Below]
            }
            "pieChart" | "ofPieChart" => &[Center, InsideEnd, OutsideEnd, BestFit],
            _ => &[],
        }
    }
}

/// Data labels of a series (Excel's Label Options).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DataLabels {
    pub value: bool,
    pub category: bool,
    pub series: bool,
    /// Percentage (pie and doughnut charts).
    pub percent: bool,
    /// `None`: the chart type's default place.
    pub pos: Option<LabelPos>,
    /// `None`: linked to the source.
    pub num_fmt: Option<String>,
    /// The labels' text formatting (Excel's Home ▸ Font on selected
    /// labels): unset parts follow the chart.
    pub font: LabelFont,
}

/// Text formatting of data labels (`c:txPr`'s default run properties).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LabelFont {
    /// Points.
    pub size: Option<f32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub color: Option<[u8; 3]>,
}

impl LabelFont {
    pub fn is_default(&self) -> bool {
        *self == LabelFont::default()
    }
}

impl DataLabels {
    pub fn shown(&self) -> bool {
        self.value || self.category || self.series || self.percent
    }
}

/// Excel's display units of a value axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisplayUnits {
    #[default]
    None,
    Hundreds,
    Thousands,
    TenThousands,
    HundredThousands,
    Millions,
    TenMillions,
    HundredMillions,
    Billions,
    Trillions,
    /// A custom divisor from the file (kept unless changed).
    Custom,
}

impl DisplayUnits {
    pub const ALL: [DisplayUnits; 10] = [
        DisplayUnits::None,
        DisplayUnits::Hundreds,
        DisplayUnits::Thousands,
        DisplayUnits::TenThousands,
        DisplayUnits::HundredThousands,
        DisplayUnits::Millions,
        DisplayUnits::TenMillions,
        DisplayUnits::HundredMillions,
        DisplayUnits::Billions,
        DisplayUnits::Trillions,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DisplayUnits::None => "None",
            DisplayUnits::Hundreds => "Hundreds",
            DisplayUnits::Thousands => "Thousands",
            DisplayUnits::TenThousands => "10000",
            DisplayUnits::HundredThousands => "100000",
            DisplayUnits::Millions => "Millions",
            DisplayUnits::TenMillions => "10000000",
            DisplayUnits::HundredMillions => "100000000",
            DisplayUnits::Billions => "Billions",
            DisplayUnits::Trillions => "Trillions",
            DisplayUnits::Custom => "Custom",
        }
    }

    /// `c:builtInUnit` value.
    pub(crate) fn code(self) -> Option<&'static str> {
        Some(match self {
            DisplayUnits::Hundreds => "hundreds",
            DisplayUnits::Thousands => "thousands",
            DisplayUnits::TenThousands => "tenThousands",
            DisplayUnits::HundredThousands => "hundredThousands",
            DisplayUnits::Millions => "millions",
            DisplayUnits::TenMillions => "tenMillions",
            DisplayUnits::HundredMillions => "hundredMillions",
            DisplayUnits::Billions => "billions",
            DisplayUnits::Trillions => "trillions",
            DisplayUnits::None | DisplayUnits::Custom => return None,
        })
    }

    pub(crate) fn from_code(c: &str) -> Self {
        DisplayUnits::ALL
            .into_iter()
            .find(|u| u.code() == Some(c))
            .unwrap_or(DisplayUnits::Custom)
    }
}

/// The time unit of a date axis' base, major and minor units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeUnit {
    Days,
    Months,
    Years,
}

impl TimeUnit {
    pub const ALL: [TimeUnit; 3] = [TimeUnit::Days, TimeUnit::Months, TimeUnit::Years];

    pub fn label(self) -> &'static str {
        match self {
            TimeUnit::Days => "Days",
            TimeUnit::Months => "Months",
            TimeUnit::Years => "Years",
        }
    }

    pub(crate) fn code(self) -> &'static str {
        match self {
            TimeUnit::Days => "days",
            TimeUnit::Months => "months",
            TimeUnit::Years => "years",
        }
    }

    pub(crate) fn from_code(c: &str) -> Option<Self> {
        TimeUnit::ALL.into_iter().find(|u| u.code() == c)
    }
}

/// Excel's Axis Type of a category axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AxisType {
    /// Automatically select based on data.
    #[default]
    Auto,
    Text,
    Date,
}

impl AxisType {
    pub const ALL: [AxisType; 3] = [AxisType::Auto, AxisType::Text, AxisType::Date];

    pub fn label(self) -> &'static str {
        match self {
            AxisType::Auto => "Automatic",
            AxisType::Text => "Text axis",
            AxisType::Date => "Date axis",
        }
    }
}

/// Options of an axis (Excel's Format Axis: Bounds, Units, Logarithmic
/// scale, reverse order, Number, Display units, Axis Type). Bounds, the
/// log scale and display units apply to value axes only; units to value
/// and date axes; the axis type and time units to category axes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AxisOptions {
    /// `None`: automatic.
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub major: Option<f64>,
    /// Minor unit (value and date axes).
    pub minor: Option<f64>,
    /// Logarithmic scale with this base.
    pub log_base: Option<f64>,
    /// Values (categories) in reverse order.
    pub reverse: bool,
    /// `None`: linked to the source.
    pub num_fmt: Option<String>,
    pub units: DisplayUnits,
    /// A category axis' type (Excel's Axis Type).
    pub axis_type: AxisType,
    /// A date axis' base unit (`None`: automatic) and the time units of
    /// its major and minor units.
    pub base_unit: Option<TimeUnit>,
    pub major_time: Option<TimeUnit>,
    pub minor_time: Option<TimeUnit>,
}

/// A number as written in chart XML (`10`, `0.25`).
pub(crate) fn num_text(v: f64) -> String {
    format!("{v}")
}
