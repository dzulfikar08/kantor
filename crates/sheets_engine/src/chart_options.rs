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

/// Options of an axis (Excel's Format Axis: Bounds, Units, Logarithmic
/// scale, reverse order, Number, Display units). Bounds, units, the log
/// scale and display units apply to value axes only.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AxisOptions {
    /// `None`: automatic.
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub major: Option<f64>,
    /// Logarithmic scale with this base.
    pub log_base: Option<f64>,
    /// Values (categories) in reverse order.
    pub reverse: bool,
    /// `None`: linked to the source.
    pub num_fmt: Option<String>,
    pub units: DisplayUnits,
}

/// A number as written in chart XML (`10`, `0.25`).
pub(crate) fn num_text(v: f64) -> String {
    format!("{v}")
}
