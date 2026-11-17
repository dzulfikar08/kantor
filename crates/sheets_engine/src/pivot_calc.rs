//! Calculations a pivot table's layout makes besides summarising: "show
//! values as", label / value / date filters, and calculated field formulas.
//!
//! * **Show Values As** (ECMA-376 Part 1 §18.18.70 `ST_ShowDataAs`, and the
//!   Excel 2010 `x14:dataField/@pivotShowAs` for % of parent, % running
//!   total and ranks): [`ShowAs`].
//! * **Filters** (§18.10.1.33 `filter`, types from §18.18.58
//!   `ST_PivotFilterType`): [`PivotFilter`], read from and written to
//!   `<filters>`; the checks are [`label_matches`], [`value_matches`],
//!   [`date_matches`] and [`top_items`].
//! * **Calculated fields** (`cacheField/@formula`): [`Formula`], a small
//!   formula language over field names, evaluated on each cell's sums as
//!   Excel does.

use roxmltree::Node;

use crate::pivot::{civil_from_days, days_from_civil};

// ------------------------------------------------------------------ show as

/// How a value field's numbers are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum ShowAs {
    #[default]
    Normal,
    PercentOfTotal,
    PercentOfColumn,
    PercentOfRow,
    /// % Of the base item of the base field.
    Percent,
    PercentOfParentRow,
    PercentOfParentColumn,
    /// % of the total of the base field's item the cell is under.
    PercentOfParent,
    Difference,
    PercentDifference,
    RunningTotal,
    PercentRunningTotal,
    RankAscending,
    RankDescending,
    Index,
}

impl ShowAs {
    pub const ALL: [ShowAs; 15] = [
        ShowAs::Normal,
        ShowAs::PercentOfTotal,
        ShowAs::PercentOfColumn,
        ShowAs::PercentOfRow,
        ShowAs::Percent,
        ShowAs::PercentOfParentRow,
        ShowAs::PercentOfParentColumn,
        ShowAs::PercentOfParent,
        ShowAs::Difference,
        ShowAs::PercentDifference,
        ShowAs::RunningTotal,
        ShowAs::PercentRunningTotal,
        ShowAs::RankAscending,
        ShowAs::RankDescending,
        ShowAs::Index,
    ];

    /// What Excel's Value Field Settings calls it.
    pub fn label(self) -> &'static str {
        match self {
            ShowAs::Normal => "No Calculation",
            ShowAs::PercentOfTotal => "% of Grand Total",
            ShowAs::PercentOfColumn => "% of Column Total",
            ShowAs::PercentOfRow => "% of Row Total",
            ShowAs::Percent => "% Of",
            ShowAs::PercentOfParentRow => "% of Parent Row Total",
            ShowAs::PercentOfParentColumn => "% of Parent Column Total",
            ShowAs::PercentOfParent => "% of Parent Total",
            ShowAs::Difference => "Difference From",
            ShowAs::PercentDifference => "% Difference From",
            ShowAs::RunningTotal => "Running Total In",
            ShowAs::PercentRunningTotal => "% Running Total In",
            ShowAs::RankAscending => "Rank Smallest to Largest",
            ShowAs::RankDescending => "Rank Largest to Smallest",
            ShowAs::Index => "Index",
        }
    }

    /// It needs a base field.
    pub fn needs_field(self) -> bool {
        matches!(
            self,
            ShowAs::Percent
                | ShowAs::PercentOfParent
                | ShowAs::Difference
                | ShowAs::PercentDifference
                | ShowAs::RunningTotal
                | ShowAs::PercentRunningTotal
                | ShowAs::RankAscending
                | ShowAs::RankDescending
        )
    }

    /// It needs a base item too.
    pub fn needs_item(self) -> bool {
        matches!(
            self,
            ShowAs::Percent | ShowAs::Difference | ShowAs::PercentDifference
        )
    }

    /// Its numbers are fractions (shown as percentages).
    pub fn is_percent(self) -> bool {
        matches!(
            self,
            ShowAs::PercentOfTotal
                | ShowAs::PercentOfColumn
                | ShowAs::PercentOfRow
                | ShowAs::Percent
                | ShowAs::PercentOfParentRow
                | ShowAs::PercentOfParentColumn
                | ShowAs::PercentOfParent
                | ShowAs::PercentDifference
                | ShowAs::PercentRunningTotal
        )
    }

    /// `(showDataAs, x14 pivotShowAs)` as the file has them.
    pub(crate) fn xml(self) -> (Option<&'static str>, Option<&'static str>) {
        match self {
            ShowAs::Normal => (None, None),
            ShowAs::PercentOfTotal => (Some("percentOfTotal"), None),
            ShowAs::PercentOfColumn => (Some("percentOfCol"), None),
            ShowAs::PercentOfRow => (Some("percentOfRow"), None),
            ShowAs::Percent => (Some("percent"), None),
            ShowAs::Difference => (Some("difference"), None),
            ShowAs::PercentDifference => (Some("percentDiff"), None),
            ShowAs::RunningTotal => (Some("runTotal"), None),
            ShowAs::Index => (Some("index"), None),
            ShowAs::PercentOfParentRow => (None, Some("percentOfParentRow")),
            ShowAs::PercentOfParentColumn => (None, Some("percentOfParentCol")),
            ShowAs::PercentOfParent => (None, Some("percentOfParent")),
            ShowAs::PercentRunningTotal => (None, Some("percentOfRunningTotal")),
            ShowAs::RankAscending => (None, Some("rankAscending")),
            ShowAs::RankDescending => (None, Some("rankDescending")),
        }
    }

    /// From a `<dataField>`; `None` for a kind Kantor doesn't know.
    pub(crate) fn parse(d: Node) -> Option<ShowAs> {
        let x14 = d
            .descendants()
            .find(|n| n.is_element() && n.tag_name().name() == "dataField" && *n != d)
            .and_then(|n| n.attribute("pivotShowAs"));
        if let Some(k) = x14 {
            return ShowAs::ALL.into_iter().find(|s| s.xml().1 == Some(k));
        }
        match d.attribute("showDataAs") {
            None | Some("normal") => Some(ShowAs::Normal),
            Some(k) => ShowAs::ALL.into_iter().find(|s| s.xml().0 == Some(k)),
        }
    }
}

/// `baseItem` values meaning the previous and the next item.
pub(crate) const BASE_PREVIOUS: u32 = 1_048_828;
pub(crate) const BASE_NEXT: u32 = 1_048_829;

/// The x14 namespace and the extension URIs Kantor writes.
pub(crate) const NS_X14: &str = "http://schemas.microsoft.com/office/spreadsheetml/2009/9/main";
pub(crate) const URI_X14_DATA_FIELD: &str = "{E15A36E0-9728-4e99-A89B-3F7291B0FE68}";
pub(crate) const URI_X14_PIVOT_FIELD: &str = "{2946ED86-A175-432a-8AC1-64E0C546D7DE}";

// ------------------------------------------------------------------ filters

/// What a filter tests, by family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterFamily {
    Label,
    Value,
    Top,
    Date,
}

/// The filter types (`ST_PivotFilterType`) Kantor applies, with Excel's
/// menu wording.
pub const FILTER_TYPES: &[(&str, &str, FilterFamily)] = &[
    ("captionEqual", "Equals", FilterFamily::Label),
    ("captionNotEqual", "Does Not Equal", FilterFamily::Label),
    ("captionBeginsWith", "Begins With", FilterFamily::Label),
    (
        "captionNotBeginsWith",
        "Does Not Begin With",
        FilterFamily::Label,
    ),
    ("captionEndsWith", "Ends With", FilterFamily::Label),
    (
        "captionNotEndsWith",
        "Does Not End With",
        FilterFamily::Label,
    ),
    ("captionContains", "Contains", FilterFamily::Label),
    (
        "captionNotContains",
        "Does Not Contain",
        FilterFamily::Label,
    ),
    ("captionGreaterThan", "Greater Than", FilterFamily::Label),
    (
        "captionGreaterThanOrEqual",
        "Greater Than Or Equal To",
        FilterFamily::Label,
    ),
    ("captionLessThan", "Less Than", FilterFamily::Label),
    (
        "captionLessThanOrEqual",
        "Less Than Or Equal To",
        FilterFamily::Label,
    ),
    ("captionBetween", "Between", FilterFamily::Label),
    ("captionNotBetween", "Not Between", FilterFamily::Label),
    ("valueEqual", "Equals", FilterFamily::Value),
    ("valueNotEqual", "Does Not Equal", FilterFamily::Value),
    ("valueGreaterThan", "Greater Than", FilterFamily::Value),
    (
        "valueGreaterThanOrEqual",
        "Greater Than Or Equal To",
        FilterFamily::Value,
    ),
    ("valueLessThan", "Less Than", FilterFamily::Value),
    (
        "valueLessThanOrEqual",
        "Less Than Or Equal To",
        FilterFamily::Value,
    ),
    ("valueBetween", "Between", FilterFamily::Value),
    ("valueNotBetween", "Not Between", FilterFamily::Value),
    ("count", "Top 10 (Items)", FilterFamily::Top),
    ("percent", "Top 10 (Percent)", FilterFamily::Top),
    ("sum", "Top 10 (Sum)", FilterFamily::Top),
    ("dateEqual", "Equals", FilterFamily::Date),
    ("dateNotEqual", "Does Not Equal", FilterFamily::Date),
    ("dateOlderThan", "Before", FilterFamily::Date),
    (
        "dateOlderThanOrEqual",
        "Before Or Equal To",
        FilterFamily::Date,
    ),
    ("dateNewerThan", "After", FilterFamily::Date),
    (
        "dateNewerThanOrEqual",
        "After Or Equal To",
        FilterFamily::Date,
    ),
    ("dateBetween", "Between", FilterFamily::Date),
    ("dateNotBetween", "Not Between", FilterFamily::Date),
    ("tomorrow", "Tomorrow", FilterFamily::Date),
    ("today", "Today", FilterFamily::Date),
    ("yesterday", "Yesterday", FilterFamily::Date),
    ("nextWeek", "Next Week", FilterFamily::Date),
    ("thisWeek", "This Week", FilterFamily::Date),
    ("lastWeek", "Last Week", FilterFamily::Date),
    ("nextMonth", "Next Month", FilterFamily::Date),
    ("thisMonth", "This Month", FilterFamily::Date),
    ("lastMonth", "Last Month", FilterFamily::Date),
    ("nextQuarter", "Next Quarter", FilterFamily::Date),
    ("thisQuarter", "This Quarter", FilterFamily::Date),
    ("lastQuarter", "Last Quarter", FilterFamily::Date),
    ("nextYear", "Next Year", FilterFamily::Date),
    ("thisYear", "This Year", FilterFamily::Date),
    ("lastYear", "Last Year", FilterFamily::Date),
    ("yearToDate", "Year to Date", FilterFamily::Date),
    ("Q1", "Quarter 1", FilterFamily::Date),
    ("Q2", "Quarter 2", FilterFamily::Date),
    ("Q3", "Quarter 3", FilterFamily::Date),
    ("Q4", "Quarter 4", FilterFamily::Date),
    ("M1", "January", FilterFamily::Date),
    ("M2", "February", FilterFamily::Date),
    ("M3", "March", FilterFamily::Date),
    ("M4", "April", FilterFamily::Date),
    ("M5", "May", FilterFamily::Date),
    ("M6", "June", FilterFamily::Date),
    ("M7", "July", FilterFamily::Date),
    ("M8", "August", FilterFamily::Date),
    ("M9", "September", FilterFamily::Date),
    ("M10", "October", FilterFamily::Date),
    ("M11", "November", FilterFamily::Date),
    ("M12", "December", FilterFamily::Date),
];

/// The family of filter type `kind`, if Kantor knows it.
pub fn filter_family(kind: &str) -> Option<FilterFamily> {
    FILTER_TYPES
        .iter()
        .find(|(k, _, _)| *k == kind)
        .map(|(_, _, f)| *f)
}

/// How many values a filter type takes (0, 1 or 2).
pub fn filter_arity(kind: &str) -> usize {
    if kind.ends_with("Between") {
        2
    } else if matches!(filter_family(kind), Some(FilterFamily::Date)) && !kind.starts_with("date") {
        0
    } else {
        1
    }
}

/// A label, value or date filter on a row or column field.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PivotFilter {
    pub fld: usize,
    /// `ST_PivotFilterType`.
    pub kind: String,
    /// The data field a value or top 10 filter tests.
    pub measure: Option<usize>,
    pub v1: String,
    pub v2: String,
    /// Top 10: the top items (else the bottom ones).
    pub top: bool,
    /// The timeline that set it (a date filter named after the timeline's
    /// cache), which also filters a field on no axis.
    pub name: Option<String>,
}

impl PivotFilter {
    /// From a `<filter>` element; `Err` names what Kantor can't apply.
    pub(crate) fn parse(f: Node) -> Result<PivotFilter, String> {
        let kind = f.attribute("type").unwrap_or("unknown").to_owned();
        let family = filter_family(&kind).ok_or_else(|| format!("a {kind} filter"))?;
        let fld = f
            .attribute("fld")
            .and_then(|v| v.parse().ok())
            .ok_or("a filter without a field")?;
        let measure = f.attribute("iMeasureFld").and_then(|v| v.parse().ok());
        let mut v1 = f.attribute("stringValue1").unwrap_or_default().to_owned();
        let mut v2 = f.attribute("stringValue2").unwrap_or_default().to_owned();
        let mut top = true;
        let col = f
            .descendants()
            .find(|n| n.is_element() && n.tag_name().name() == "filterColumn");
        if let Some(t) = col.and_then(|c| {
            c.children()
                .find(|n| n.is_element() && n.tag_name().name() == "top10")
        }) {
            top = !matches!(t.attribute("top"), Some("0" | "false"));
            if v1.is_empty() {
                v1 = t.attribute("val").unwrap_or("10").to_owned();
            }
        }
        if v1.is_empty() && family != FilterFamily::Top {
            let vals: Vec<&str> = col
                .into_iter()
                .flat_map(|c| c.descendants())
                .filter(|n| n.is_element() && n.tag_name().name() == "customFilter")
                .filter_map(|n| n.attribute("val"))
                .collect();
            let unwild = |s: &str| s.trim_matches('*').replace("~*", "*");
            if let Some(a) = vals.first() {
                v1 = unwild(a);
            }
            if let Some(b) = vals.get(1) {
                v2 = unwild(b);
            }
        }
        if matches!(family, FilterFamily::Value | FilterFamily::Top) && measure.is_none() {
            return Err("a value filter without a value field".into());
        }
        Ok(PivotFilter {
            fld,
            kind,
            measure,
            v1,
            v2,
            top,
            name: f.attribute("name").map(str::to_owned),
        })
    }

    /// The `<filter>` element (`id` from 1).
    pub(crate) fn xml(&self, id: usize) -> String {
        let family = filter_family(&self.kind).unwrap_or(FilterFamily::Label);
        let mut out = format!(
            "<filter fld=\"{}\" type=\"{}\" evalOrder=\"-1\" id=\"{id}\"",
            self.fld, self.kind
        );
        if let Some(m) = self.measure {
            out.push_str(&format!(" iMeasureFld=\"{m}\""));
        }
        if let Some(n) = &self.name {
            out.push_str(&format!(" name=\"{}\"", esc(n)));
        }
        let arity = filter_arity(&self.kind);
        if matches!(family, FilterFamily::Label | FilterFamily::Date) && arity > 0 {
            let (a, b) = match family {
                FilterFamily::Date => (iso_date(&self.v1), iso_date(&self.v2)),
                _ => (self.v1.clone(), self.v2.clone()),
            };
            out.push_str(&format!(" stringValue1=\"{}\"", esc(&a)));
            if arity == 2 {
                out.push_str(&format!(" stringValue2=\"{}\"", esc(&b)));
            }
        }
        out.push_str("><autoFilter ref=\"A1\"><filterColumn colId=\"0\">");
        out.push_str(&self.criteria_xml(family));
        out.push_str("</filterColumn></autoFilter>");
        if self.name.is_some() && family == FilterFamily::Date {
            // A timeline's: whole days, as Excel writes it.
            out.push_str("<extLst><ext uri=\"{0605FD5F-26C8-4aeb-8148-2DB25E43C511}\" xmlns:x15=\"http://schemas.microsoft.com/office/spreadsheetml/2010/11/main\"><x15:pivotFilter useWholeDay=\"1\"/></ext></extLst>");
        }
        out.push_str("</filter>");
        out
    }

    /// What goes in the filter's `<filterColumn>`.
    fn criteria_xml(&self, family: FilterFamily) -> String {
        let k = self.kind.as_str();
        if family == FilterFamily::Top {
            let n = self.v1.trim().parse::<f64>().unwrap_or(10.0);
            let n = crate::pivot::format_number(n);
            return format!(
                "<top10{}{} val=\"{n}\" filterVal=\"{n}\"/>",
                if self.top { "" } else { " top=\"0\"" },
                if k == "percent" { " percent=\"1\"" } else { "" },
            );
        }
        if family == FilterFamily::Date && filter_arity(k) == 0 {
            return format!("<dynamicFilter type=\"{k}\"/>");
        }
        let val = |s: &str| match family {
            FilterFamily::Date => date_serial(s)
                .map_or_else(|| s.to_owned(), |d| crate::pivot::format_number(d.floor())),
            FilterFamily::Value => s.trim().to_owned(),
            _ => s.replace('*', "~*"),
        };
        let (a, b) = (val(&self.v1), val(&self.v2));
        let one = |op: Option<&str>, v: &str| match op {
            Some(op) => format!("<customFilter operator=\"{op}\" val=\"{}\"/>", esc(v)),
            None => format!("<customFilter val=\"{}\"/>", esc(v)),
        };
        let rule = k
            .trim_start_matches("caption")
            .trim_start_matches("value")
            .trim_start_matches("date");
        let filters = match rule {
            "Equal" => one(None, &a),
            "NotEqual" => one(Some("notEqual"), &a),
            "BeginsWith" => one(None, &format!("{a}*")),
            "NotBeginsWith" => one(Some("notEqual"), &format!("{a}*")),
            "EndsWith" => one(None, &format!("*{a}")),
            "NotEndsWith" => one(Some("notEqual"), &format!("*{a}")),
            "Contains" => one(None, &format!("*{a}*")),
            "NotContains" => one(Some("notEqual"), &format!("*{a}*")),
            "GreaterThan" | "NewerThan" => one(Some("greaterThan"), &a),
            "GreaterThanOrEqual" | "NewerThanOrEqual" => one(Some("greaterThanOrEqual"), &a),
            "LessThan" | "OlderThan" => one(Some("lessThan"), &a),
            "LessThanOrEqual" | "OlderThanOrEqual" => one(Some("lessThanOrEqual"), &a),
            "Between" => {
                return format!(
                    "<customFilters and=\"1\">{}{}</customFilters>",
                    one(Some("greaterThanOrEqual"), &a),
                    one(Some("lessThanOrEqual"), &b)
                );
            }
            _ => {
                return format!(
                    "<customFilters>{}{}</customFilters>",
                    one(Some("lessThan"), &a),
                    one(Some("greaterThan"), &b)
                );
            }
        };
        format!("<customFilters>{filters}</customFilters>")
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// `*` and `?` wildcards, case-insensitive, `~` escaping them.
fn wild(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    fn go(p: &[char], t: &[char]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some('*') => (0..=t.len()).any(|i| go(&p[1..], &t[i..])),
            Some('?') => !t.is_empty() && go(&p[1..], &t[1..]),
            Some('~') if p.len() > 1 => t.first() == Some(&p[1]) && go(&p[2..], &t[1..]),
            Some(c) => t.first() == Some(c) && go(&p[1..], &t[1..]),
        }
    }
    go(&p, &t)
}

/// Compare two labels: as numbers when both are, else as text ignoring
/// case.
fn cmp_labels(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
        (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => a.to_lowercase().cmp(&b.to_lowercase()),
    }
}

/// Whether item caption `label` passes label filter `kind`.
pub(crate) fn label_matches(kind: &str, label: &str, a: &str, b: &str) -> bool {
    use std::cmp::Ordering::*;
    let lower = label.to_lowercase();
    let la = a.to_lowercase();
    match kind {
        "captionEqual" => wild(a, label),
        "captionNotEqual" => !wild(a, label),
        "captionBeginsWith" => lower.starts_with(&la),
        "captionNotBeginsWith" => !lower.starts_with(&la),
        "captionEndsWith" => lower.ends_with(&la),
        "captionNotEndsWith" => !lower.ends_with(&la),
        "captionContains" => lower.contains(&la),
        "captionNotContains" => !lower.contains(&la),
        "captionGreaterThan" => cmp_labels(label, a) == Greater,
        "captionGreaterThanOrEqual" => cmp_labels(label, a) != Less,
        "captionLessThan" => cmp_labels(label, a) == Less,
        "captionLessThanOrEqual" => cmp_labels(label, a) != Greater,
        "captionBetween" => cmp_labels(label, a) != Less && cmp_labels(label, b) != Greater,
        "captionNotBetween" => cmp_labels(label, a) == Less || cmp_labels(label, b) == Greater,
        _ => true,
    }
}

/// Whether an item's value `x` passes value filter `kind`.
pub(crate) fn value_matches(kind: &str, x: f64, a: &str, b: &str) -> bool {
    let a: f64 = a.trim().parse().unwrap_or(0.0);
    let b: f64 = b.trim().parse().unwrap_or(0.0);
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    match kind {
        "valueEqual" => x == a,
        "valueNotEqual" => x != a,
        "valueGreaterThan" => x > a,
        "valueGreaterThanOrEqual" => x >= a,
        "valueLessThan" => x < a,
        "valueLessThanOrEqual" => x <= a,
        "valueBetween" => x >= lo && x <= hi,
        "valueNotBetween" => x < lo || x > hi,
        _ => true,
    }
}

/// Today as a serial date (UTC).
pub(crate) fn today() -> f64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    (secs / 86_400) as f64 + 25_569.0
}

/// A date typed as `2026-01-05`, `1/5/2026` (month first, as Excel's
/// file strings) or a serial number, as a serial number.
pub(crate) fn date_serial(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(n) = s.parse::<f64>() {
        return Some(n);
    }
    let date = s.split(['T', ' ']).next()?;
    let ymd = |y: i64, m: i64, d: i64| {
        ((1..=12).contains(&m) && (1..=31).contains(&d))
            .then(|| (days_from_civil(y, m, d) + 25_569) as f64)
    };
    if let [y, m, d] = date.split('-').collect::<Vec<_>>()[..] {
        return ymd(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?);
    }
    if let [m, d, y] = date.split('/').collect::<Vec<_>>()[..] {
        return ymd(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?);
    }
    None
}

/// `2026-01-05T00:00:00` for a typed date (kept as typed if not one).
fn iso_date(s: &str) -> String {
    match date_serial(s) {
        Some(d) => crate::pivot::iso_of_serial(d.floor()),
        None => s.to_owned(),
    }
}

/// `(year, month, day)` of a serial date.
pub(crate) fn ymd(serial: f64) -> (i64, i64, i64) {
    civil_from_days(serial.floor() as i64 - 25_569)
}

fn serial(y: i64, m: i64, d: i64) -> f64 {
    // Months past 12 roll into the next year.
    let (y, m) = (y + (m - 1).div_euclid(12), (m - 1).rem_euclid(12) + 1);
    (days_from_civil(y, m, d) + 25_569) as f64
}

/// Whether date `v` (a serial) passes date filter `kind`, `today` being
/// the serial of today. Weeks start on Sunday, as in Excel.
pub(crate) fn date_matches(kind: &str, v: f64, a: &str, b: &str, today: f64) -> bool {
    let day = v.floor();
    let t = today.floor();
    let a = date_serial(a).map(f64::floor);
    let b = date_serial(b).map(f64::floor);
    let (y, m, _) = ymd(t);
    let (vy, vm, _) = ymd(day);
    let within = |lo: f64, hi: f64| day >= lo && day < hi;
    // Sunday of this week (serial 1, 1900-01-01, was a Sunday).
    let week = t - ((t as i64 + 6).rem_euclid(7)) as f64;
    let q = (m - 1) / 3;
    let quarter = |k: i64| {
        let first = serial(y, q * 3 + 1 + k * 3, 1);
        within(first, serial(y, q * 3 + 4 + k * 3, 1))
    };
    match kind {
        "dateEqual" => a == Some(day),
        "dateNotEqual" => a != Some(day),
        "dateOlderThan" => a.is_some_and(|a| day < a),
        "dateOlderThanOrEqual" => a.is_some_and(|a| day <= a),
        "dateNewerThan" => a.is_some_and(|a| day > a),
        "dateNewerThanOrEqual" => a.is_some_and(|a| day >= a),
        "dateBetween" => matches!((a, b), (Some(a), Some(b)) if day >= a.min(b) && day <= a.max(b)),
        "dateNotBetween" => {
            matches!((a, b), (Some(a), Some(b)) if day < a.min(b) || day > a.max(b))
        }
        "tomorrow" => day == t + 1.0,
        "today" => day == t,
        "yesterday" => day == t - 1.0,
        "nextWeek" => within(week + 7.0, week + 14.0),
        "thisWeek" => within(week, week + 7.0),
        "lastWeek" => within(week - 7.0, week),
        "nextMonth" => within(serial(y, m + 1, 1), serial(y, m + 2, 1)),
        "thisMonth" => within(serial(y, m, 1), serial(y, m + 1, 1)),
        "lastMonth" => within(serial(y, m - 1, 1), serial(y, m, 1)),
        "nextQuarter" => quarter(1),
        "thisQuarter" => quarter(0),
        "lastQuarter" => quarter(-1),
        "nextYear" => vy == y + 1,
        "thisYear" => vy == y,
        "lastYear" => vy == y - 1,
        "yearToDate" => within(serial(y, 1, 1), t + 1.0),
        k if k.starts_with('Q') => k[1..].parse::<i64>().is_ok_and(|n| (vm - 1) / 3 + 1 == n),
        k if k.starts_with('M') => k[1..].parse::<i64>().is_ok_and(|n| vm == n),
        _ => true,
    }
}

/// The items a top 10 filter keeps: `values` are `(item, value)`; `kind`
/// is `count` (the n items), `percent` (the items making up n% of the
/// total) or `sum` (the items whose values add up to n).
pub(crate) fn top_items(kind: &str, n: f64, top: bool, values: &[(usize, f64)]) -> Vec<usize> {
    let mut v = values.to_vec();
    v.sort_by(|a, b| {
        let o = b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal);
        if top { o } else { o.reverse() }
    });
    match kind {
        "count" => v
            .iter()
            .take(n.max(0.0).round() as usize)
            .map(|x| x.0)
            .collect(),
        _ => {
            let total: f64 = values.iter().map(|x| x.1).sum();
            let goal = if kind == "percent" {
                total * n / 100.0
            } else {
                n
            };
            let mut out = Vec::new();
            let mut acc = 0.0;
            for (k, x) in v {
                if (top && acc >= goal) || (!top && acc >= goal && !out.is_empty()) {
                    break;
                }
                acc += x;
                out.push(k);
            }
            out
        }
    }
}

// ------------------------------------------------------------------ formulas

/// A calculated field's formula, compiled against the cache's field names.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Formula {
    expr: Expr,
    /// The fields it uses.
    pub fields: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq)]
enum Expr {
    Num(f64),
    Str(String),
    Bool(bool),
    Field(usize),
    Neg(Box<Expr>),
    Pct(Box<Expr>),
    Bin(char, Box<Expr>, Box<Expr>),
    Cmp(&'static str, Box<Expr>, Box<Expr>),
    Call(&'static str, Vec<Expr>),
}

/// Worksheet functions a calculated field may use (those that take no
/// references).
const FUNCTIONS: [&str; 18] = [
    "IF",
    "AND",
    "OR",
    "NOT",
    "ABS",
    "ROUND",
    "ROUNDUP",
    "ROUNDDOWN",
    "INT",
    "MIN",
    "MAX",
    "SUM",
    "AVERAGE",
    "SQRT",
    "MOD",
    "POWER",
    "IFERROR",
    "SIGN",
];

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Name(String),
    Op(&'static str),
    Open,
    Close,
    Comma,
    /// `[East]` after a field name (a calculated item's `Region[East]`).
    Bracket(String),
}

fn tokens(src: &str) -> Result<Vec<Tok>, String> {
    let c: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch.is_ascii_digit()
            || (ch == '.' && c.get(i + 1).is_some_and(char::is_ascii_digit))
        {
            let start = i;
            while i < c.len() && (c[i].is_ascii_digit() || c[i] == '.') {
                i += 1;
            }
            if i < c.len()
                && (c[i] == 'e' || c[i] == 'E')
                && c.get(i + 1)
                    .is_some_and(|d| d.is_ascii_digit() || *d == '-' || *d == '+')
            {
                i += 2;
                while i < c.len() && c[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let s: String = c[start..i].iter().collect();
            out.push(Tok::Num(
                s.parse().map_err(|_| format!("{s} is not a number"))?,
            ));
        } else if ch == '"' {
            let mut s = String::new();
            i += 1;
            loop {
                match c.get(i) {
                    None => return Err("a text in quotes isn't closed".into()),
                    Some('"') if c.get(i + 1) == Some(&'"') => {
                        s.push('"');
                        i += 2;
                    }
                    Some('"') => {
                        i += 1;
                        break;
                    }
                    Some(x) => {
                        s.push(*x);
                        i += 1;
                    }
                }
            }
            out.push(Tok::Str(s));
        } else if ch == '\'' {
            let mut s = String::new();
            i += 1;
            loop {
                match c.get(i) {
                    None => return Err("a field name in quotes isn't closed".into()),
                    Some('\'') if c.get(i + 1) == Some(&'\'') => {
                        s.push('\'');
                        i += 2;
                    }
                    Some('\'') => {
                        i += 1;
                        break;
                    }
                    Some(x) => {
                        s.push(*x);
                        i += 1;
                    }
                }
            }
            out.push(Tok::Name(s));
        } else if ch == '[' {
            let rest: String = c[i + 1..].iter().collect();
            let end = rest
                .find(']')
                .ok_or_else(|| "a [ isn't closed".to_owned())?;
            let inner = rest[..end].trim();
            let inner = match inner.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')) {
                Some(q) => q.replace("''", "'"),
                None => inner.to_owned(),
            };
            out.push(Tok::Bracket(inner));
            i += end + 2;
        } else if ch.is_alphabetic() || ch == '_' {
            let start = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_' || c[i] == '.') {
                i += 1;
            }
            out.push(Tok::Name(c[start..i].iter().collect()));
        } else {
            let two: String = c[i..(i + 2).min(c.len())].iter().collect();
            let op = match two.as_str() {
                "<=" => Some("<="),
                ">=" => Some(">="),
                "<>" => Some("<>"),
                _ => None,
            };
            if let Some(op) = op {
                out.push(Tok::Op(op));
                i += 2;
                continue;
            }
            out.push(match ch {
                '+' => Tok::Op("+"),
                '-' => Tok::Op("-"),
                '*' => Tok::Op("*"),
                '/' => Tok::Op("/"),
                '^' => Tok::Op("^"),
                '&' => Tok::Op("&"),
                '%' => Tok::Op("%"),
                '=' => Tok::Op("="),
                '<' => Tok::Op("<"),
                '>' => Tok::Op(">"),
                '(' => Tok::Open,
                ')' => Tok::Close,
                ',' | ';' => Tok::Comma,
                _ => return Err(format!("{ch:?} can't be used in a formula here")),
            });
            i += 1;
        }
    }
    Ok(out)
}

struct Parser<'a> {
    toks: Vec<Tok>,
    at: usize,
    names: &'a [String],
    used: Vec<usize>,
    /// Compiling a calculated item: the name of its field (`Region` in
    /// `Region[East]`); `names` are then the field's items.
    field: Option<&'a str>,
    /// The calculated item's own place in `names` (for `Region[+1]`).
    me: Option<usize>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }

    fn op(&mut self, ops: &[&'static str]) -> Option<&'static str> {
        match self.peek() {
            Some(Tok::Op(o)) if ops.contains(o) => {
                let o = *o;
                self.at += 1;
                Some(o)
            }
            _ => None,
        }
    }

    /// An item of the calculated item's field, by name.
    fn item(&mut self, name: &str) -> Result<Expr, String> {
        let k = self
            .names
            .iter()
            .position(|x| x.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("there is no item named {name}"))?;
        if !self.used.contains(&k) {
            self.used.push(k);
        }
        Ok(Expr::Field(k))
    }

    /// An item by position (Excel's `Region[2]`: the second item;
    /// `Region[+1]` / `Region[-1]`: the item after / before the calculated
    /// item), counting the field's items in order. `None`: not a position.
    fn position(&self, text: &str) -> Result<Option<usize>, String> {
        let relative = text.starts_with(['+', '-']);
        let Ok(n) = text.replace(' ', "").parse::<i64>() else {
            return Ok(None);
        };
        // The field's real items (blanks are its subtotal rows).
        let items: Vec<usize> = (0..self.names.len())
            .filter(|&k| self.names[k] != "\u{0}")
            .collect();
        let at = if relative {
            let me = match self.me {
                Some(k) => items.iter().position(|&x| x == k),
                // A new item goes after the others.
                None => Some(items.len()),
            }
            .ok_or("the calculated item isn't in its field")? as i64;
            me + n
        } else {
            n - 1
        };
        if n == 0 && relative {
            return Err(format!("[{text}] is the calculated item itself"));
        }
        usize::try_from(at)
            .ok()
            .and_then(|i| items.get(i).copied())
            .map(Some)
            .ok_or_else(|| format!("there is no item at position [{text}]"))
    }

    fn compare(&mut self) -> Result<Expr, String> {
        let mut a = self.concat()?;
        while let Some(o) = self.op(&["=", "<>", "<", ">", "<=", ">="]) {
            let b = self.concat()?;
            a = Expr::Cmp(o, Box::new(a), Box::new(b));
        }
        Ok(a)
    }

    fn concat(&mut self) -> Result<Expr, String> {
        let mut a = self.sum()?;
        while self.op(&["&"]).is_some() {
            let b = self.sum()?;
            a = Expr::Bin('&', Box::new(a), Box::new(b));
        }
        Ok(a)
    }

    fn sum(&mut self) -> Result<Expr, String> {
        let mut a = self.product()?;
        while let Some(o) = self.op(&["+", "-"]) {
            let b = self.product()?;
            a = Expr::Bin(o.chars().next().unwrap_or('+'), Box::new(a), Box::new(b));
        }
        Ok(a)
    }

    fn product(&mut self) -> Result<Expr, String> {
        let mut a = self.power()?;
        while let Some(o) = self.op(&["*", "/"]) {
            let b = self.power()?;
            a = Expr::Bin(o.chars().next().unwrap_or('*'), Box::new(a), Box::new(b));
        }
        Ok(a)
    }

    fn power(&mut self) -> Result<Expr, String> {
        let mut a = self.unary()?;
        while self.op(&["^"]).is_some() {
            let b = self.unary()?;
            a = Expr::Bin('^', Box::new(a), Box::new(b));
        }
        Ok(a)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.op(&["-"]).is_some() {
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        if self.op(&["+"]).is_some() {
            return self.unary();
        }
        let mut a = self.atom()?;
        while self.op(&["%"]).is_some() {
            a = Expr::Pct(Box::new(a));
        }
        Ok(a)
    }

    fn atom(&mut self) -> Result<Expr, String> {
        let t = self
            .toks
            .get(self.at)
            .cloned()
            .ok_or("the formula ends too soon")?;
        self.at += 1;
        match t {
            Tok::Num(n) => Ok(Expr::Num(n)),
            Tok::Str(s) => Ok(Expr::Str(s)),
            Tok::Open => {
                let e = self.compare()?;
                match self.peek() {
                    Some(Tok::Close) => {
                        self.at += 1;
                        Ok(e)
                    }
                    _ => Err("a parenthesis isn't closed".into()),
                }
            }
            Tok::Name(n) => {
                if let Some(Tok::Bracket(item)) = self.peek().cloned() {
                    self.at += 1;
                    let Some(field) = self.field else {
                        return Err(format!("{n}[{item}] can't be used in a calculated field"));
                    };
                    if !field.eq_ignore_ascii_case(&n) {
                        return Err(format!(
                            "a calculated item can only use items of its own field ({field}), not {n}"
                        ));
                    }
                    let item = item.trim();
                    if self.names.iter().any(|x| x.eq_ignore_ascii_case(item)) {
                        return self.item(item);
                    }
                    if let Some(k) = self.position(item)? {
                        if !self.used.contains(&k) {
                            self.used.push(k);
                        }
                        return Ok(Expr::Field(k));
                    }
                    return self.item(item);
                }
                if self.peek() == Some(&Tok::Open) {
                    let upper = n.to_uppercase();
                    let f = FUNCTIONS
                        .iter()
                        .find(|f| **f == upper)
                        .ok_or_else(|| format!("{n} can't be used in a calculated field"))?;
                    self.at += 1;
                    let mut args = Vec::new();
                    if self.peek() == Some(&Tok::Close) {
                        self.at += 1;
                    } else {
                        loop {
                            args.push(self.compare()?);
                            match self.peek() {
                                Some(Tok::Comma) => self.at += 1,
                                Some(Tok::Close) => {
                                    self.at += 1;
                                    break;
                                }
                                _ => return Err(format!("{n}( isn't closed")),
                            }
                        }
                    }
                    return Ok(Expr::Call(f, args));
                }
                if let Some(k) = self.names.iter().position(|x| x.eq_ignore_ascii_case(&n)) {
                    if !self.used.contains(&k) {
                        self.used.push(k);
                    }
                    return Ok(Expr::Field(k));
                }
                match n.to_uppercase().as_str() {
                    "TRUE" => Ok(Expr::Bool(true)),
                    "FALSE" => Ok(Expr::Bool(false)),
                    _ if self.field.is_some() => Err(format!("there is no item named {n}")),
                    _ => Err(format!("there is no field named {n}")),
                }
            }
            Tok::Bracket(b) => Err(format!("[{b}] is out of place")),
            Tok::Op(o) => Err(format!("{o} is out of place")),
            Tok::Close => Err("a closing parenthesis is out of place".into()),
            Tok::Comma => Err("a comma is out of place".into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum V {
    N(f64),
    S(String),
    B(bool),
}

impl V {
    fn num(&self) -> Result<f64, &'static str> {
        match self {
            V::N(n) => Ok(*n),
            V::B(b) => Ok(f64::from(u8::from(*b))),
            V::S(s) if s.is_empty() => Ok(0.0),
            V::S(s) => s.trim().parse().map_err(|_| "#VALUE!"),
        }
    }

    fn truth(&self) -> Result<bool, &'static str> {
        match self {
            V::B(b) => Ok(*b),
            V::N(n) => Ok(*n != 0.0),
            V::S(s) if s.eq_ignore_ascii_case("true") => Ok(true),
            V::S(s) if s.eq_ignore_ascii_case("false") => Ok(false),
            V::S(_) => Err("#VALUE!"),
        }
    }
}

impl Formula {
    /// Compile `src` (with or without a leading `=`) against field `names`.
    pub(crate) fn compile(src: &str, names: &[String]) -> Result<Formula, String> {
        Self::compile_in(src, names, None, None)
    }

    /// Compile a calculated item's formula of field `field` against the
    /// field's item names (`East`, `'North East'`, `Region[East]`, or by
    /// position `Region[2]`, `Region[+1]`); `fields` then index `items`.
    /// `at`: the calculated item's own place in `items` (`None`: a new one,
    /// after them).
    pub(crate) fn compile_items(
        src: &str,
        field: &str,
        items: &[String],
        at: Option<usize>,
    ) -> Result<Formula, String> {
        Self::compile_in(src, items, Some(field), at)
    }

    fn compile_in(
        src: &str,
        names: &[String],
        field: Option<&str>,
        at: Option<usize>,
    ) -> Result<Formula, String> {
        let src = src.trim().trim_start_matches('=');
        let toks = tokens(src)?;
        if toks.is_empty() {
            return Err("the formula is empty".into());
        }
        let mut p = Parser {
            toks,
            at: 0,
            names,
            used: Vec::new(),
            field,
            me: at,
        };
        let expr = p.compare()?;
        if p.at < p.toks.len() {
            return Err("there's more after the end of the formula".into());
        }
        Ok(Formula {
            expr,
            fields: p.used,
        })
    }

    /// The formula's number, `field` giving each field's sum.
    pub(crate) fn eval(&self, field: &dyn Fn(usize) -> f64) -> Result<f64, &'static str> {
        let v = eval(&self.expr, field)?;
        let n = v.num()?;
        if n.is_finite() { Ok(n) } else { Err("#NUM!") }
    }
}

fn eval(e: &Expr, field: &dyn Fn(usize) -> f64) -> Result<V, &'static str> {
    Ok(match e {
        Expr::Num(n) => V::N(*n),
        Expr::Str(s) => V::S(s.clone()),
        Expr::Bool(b) => V::B(*b),
        Expr::Field(k) => V::N(field(*k)),
        Expr::Neg(a) => V::N(-eval(a, field)?.num()?),
        Expr::Pct(a) => V::N(eval(a, field)?.num()? / 100.0),
        Expr::Bin('&', a, b) => {
            let s = |v: V| match v {
                V::S(s) => s,
                V::N(n) => crate::pivot::format_number(n),
                V::B(b) => if b { "TRUE" } else { "FALSE" }.to_owned(),
            };
            V::S(s(eval(a, field)?) + &s(eval(b, field)?))
        }
        Expr::Bin(op, a, b) => {
            let (x, y) = (eval(a, field)?.num()?, eval(b, field)?.num()?);
            V::N(match op {
                '+' => x + y,
                '-' => x - y,
                '*' => x * y,
                '/' if y == 0.0 => return Err("#DIV/0!"),
                '/' => x / y,
                _ => x.powf(y),
            })
        }
        Expr::Cmp(op, a, b) => {
            let (x, y) = (eval(a, field)?, eval(b, field)?);
            let o = match (&x, &y) {
                (V::S(s), V::S(t)) => s.to_lowercase().cmp(&t.to_lowercase()),
                _ => x
                    .num()?
                    .partial_cmp(&y.num()?)
                    .unwrap_or(std::cmp::Ordering::Equal),
            };
            use std::cmp::Ordering::*;
            V::B(match *op {
                "=" => o == Equal,
                "<>" => o != Equal,
                "<" => o == Less,
                ">" => o == Greater,
                "<=" => o != Greater,
                _ => o != Less,
            })
        }
        Expr::Call(f, args) => call(f, args, field)?,
    })
}

fn call(f: &str, args: &[Expr], field: &dyn Fn(usize) -> f64) -> Result<V, &'static str> {
    let arg = |i: usize| -> Result<V, &'static str> {
        args.get(i).map_or(Err("#VALUE!"), |a| eval(a, field))
    };
    let n = |i: usize| arg(i)?.num();
    let all = || -> Result<Vec<f64>, &'static str> {
        args.iter().map(|a| eval(a, field)?.num()).collect()
    };
    let round = |x: f64, d: f64, how: fn(f64) -> f64| {
        let p = 10f64.powi(d as i32);
        how(x * p) / p
    };
    Ok(match f {
        "IF" => {
            if arg(0)?.truth()? {
                arg(1)?
            } else if args.len() > 2 {
                arg(2)?
            } else {
                V::B(false)
            }
        }
        "IFERROR" => match arg(0) {
            Ok(v) => v,
            Err(_) => arg(1)?,
        },
        "AND" => V::B(
            args.iter()
                .map(|a| eval(a, field)?.truth())
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .all(|b| b),
        ),
        "OR" => V::B(
            args.iter()
                .map(|a| eval(a, field)?.truth())
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .any(|b| b),
        ),
        "NOT" => V::B(!arg(0)?.truth()?),
        "ABS" => V::N(n(0)?.abs()),
        "SIGN" => V::N(if n(0)? == 0.0 { 0.0 } else { n(0)?.signum() }),
        "INT" => V::N(n(0)?.floor()),
        "SQRT" if n(0)? < 0.0 => return Err("#NUM!"),
        "SQRT" => V::N(n(0)?.sqrt()),
        "ROUND" => V::N(round(n(0)?, n(1).unwrap_or(0.0), f64::round)),
        "ROUNDUP" => V::N(round(n(0)?, n(1).unwrap_or(0.0), |x| {
            x.signum() * x.abs().ceil()
        })),
        "ROUNDDOWN" => V::N(round(n(0)?, n(1).unwrap_or(0.0), f64::trunc)),
        "MOD" if n(1)? == 0.0 => return Err("#DIV/0!"),
        "MOD" => {
            let (x, y) = (n(0)?, n(1)?);
            V::N(x - y * (x / y).floor())
        }
        "POWER" => V::N(n(0)?.powf(n(1)?)),
        "MIN" => V::N(all()?.into_iter().fold(f64::INFINITY, f64::min)),
        "MAX" => V::N(all()?.into_iter().fold(f64::NEG_INFINITY, f64::max)),
        "SUM" => V::N(all()?.into_iter().sum()),
        "AVERAGE" if args.is_empty() => return Err("#DIV/0!"),
        "AVERAGE" => V::N(all()?.into_iter().sum::<f64>() / args.len() as f64),
        _ => return Err("#NAME?"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formulas() {
        let names: Vec<String> = ["Units", "Revenue", "Unit Price"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let vals = [4.0, 60.0, 15.0];
        let f = |s: &str| Formula::compile(s, &names).map(|x| x.eval(&|k| vals[k]));
        assert_eq!(f("=Revenue/Units"), Ok(Ok(15.0)));
        assert_eq!(f("revenue - units * 2"), Ok(Ok(52.0)));
        assert_eq!(f("'Unit Price'*2"), Ok(Ok(30.0)));
        assert_eq!(f("IF(Units>3, 1, 0)"), Ok(Ok(1.0)));
        assert_eq!(f("-Units^2"), Ok(Ok(16.0)));
        assert_eq!(f("Units/0"), Ok(Err("#DIV/0!")));
        assert_eq!(f("ROUND(Revenue/7, 2)"), Ok(Ok(8.57)));
        assert_eq!(f("50%*Revenue"), Ok(Ok(30.0)));
        assert!(f("Cost*2").is_err());
        assert!(f("VLOOKUP(1)").is_err());
        assert!(f("(Units").is_err());
        assert_eq!(
            Formula::compile("Revenue/Units+Revenue", &names)
                .unwrap()
                .fields,
            [1, 0]
        );
    }

    #[test]
    fn label_and_value_filters() {
        assert!(label_matches("captionEqual", "East", "e*", ""));
        assert!(!label_matches("captionEqual", "West", "e*", ""));
        assert!(label_matches("captionBeginsWith", "North", "no", ""));
        assert!(label_matches("captionNotContains", "North", "x", ""));
        assert!(label_matches("captionBetween", "Pears", "O", "Q"));
        assert!(label_matches("captionGreaterThan", "10", "9", ""));
        assert!(value_matches("valueBetween", 5.0, "10", "1"));
        assert!(value_matches("valueGreaterThan", 401.0, "400", ""));
        assert!(!value_matches("valueNotBetween", 5.0, "1", "10"));
    }

    #[test]
    fn date_filters() {
        // Wednesday 2026-11-04.
        let t = date_serial("2026-11-04").unwrap();
        let d = |s: &str| date_serial(s).unwrap();
        assert!(date_matches("today", t + 0.5, "", "", t));
        assert!(date_matches("thisWeek", d("2026-11-01"), "", "", t));
        assert!(!date_matches("thisWeek", d("2026-10-31"), "", "", t));
        assert!(date_matches("lastMonth", d("2026-10-31"), "", "", t));
        assert!(date_matches("nextMonth", d("2026-12-01"), "", "", t));
        assert!(date_matches("thisQuarter", d("2026-10-01"), "", "", t));
        assert!(date_matches("lastQuarter", d("2026-09-30"), "", "", t));
        assert!(date_matches("nextQuarter", d("2027-01-15"), "", "", t));
        assert!(date_matches("yearToDate", d("2026-01-01"), "", "", t));
        assert!(!date_matches("yearToDate", d("2026-11-05"), "", "", t));
        assert!(date_matches("Q2", d("2019-05-01"), "", "", t));
        assert!(date_matches("M11", d("2019-11-30"), "", "", t));
        assert!(date_matches(
            "dateBetween",
            d("2026-02-01"),
            "1/1/2026",
            "2026-02-01",
            t
        ));
        assert!(date_matches(
            "dateOlderThan",
            d("2025-12-31"),
            "2026-01-01",
            "",
            t
        ));
    }

    #[test]
    fn top_ten() {
        let v = [(0, 10.0), (1, 40.0), (2, 30.0), (3, 20.0)];
        assert_eq!(top_items("count", 2.0, true, &v), [1, 2]);
        assert_eq!(top_items("count", 1.0, false, &v), [0]);
        // 50% of 100: 40 then 30 reaches it.
        assert_eq!(top_items("percent", 50.0, true, &v), [1, 2]);
        assert_eq!(top_items("sum", 25.0, false, &v), [0, 3]);
    }

    #[test]
    fn filters_round_trip() {
        let f = PivotFilter {
            fld: 1,
            kind: "captionBeginsWith".into(),
            measure: None,
            v1: "Ap".into(),
            v2: String::new(),
            top: true,
            name: None,
        };
        let x = f.xml(1);
        let doc = roxmltree::Document::parse(&x).unwrap();
        assert_eq!(PivotFilter::parse(doc.root_element()).unwrap(), f);
        let t = PivotFilter {
            fld: 0,
            kind: "count".into(),
            measure: Some(0),
            v1: "2".into(),
            v2: String::new(),
            top: false,
            name: None,
        };
        let x = t.xml(2);
        assert!(x.contains("<top10 top=\"0\" val=\"2\""), "{x}");
        let doc = roxmltree::Document::parse(&x).unwrap();
        assert_eq!(PivotFilter::parse(doc.root_element()).unwrap(), t);
    }
}
