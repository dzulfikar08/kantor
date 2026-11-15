//! Grouped pivot fields (Excel's Group / Group Selection / Ungroup), as a
//! cache holds them (ECMA-376 Part 1 §18.10.1.37 `fieldGroup`):
//!
//! * **Ranges** (`rangePr`): numbers in steps (`Starting at`, `Ending at`,
//!   `By`), or dates by seconds, minutes, hours, days (or a number of
//!   days), months, quarters or years. The finest level chosen groups the
//!   field itself; each coarser one is a new cache field (`Years`,
//!   `Quarters`...) grouping the same base field, linked by `par`.
//! * **Discrete** (`discretePr`): items picked by hand (Group Selection)
//!   become `Group1`, `Group2`... of a new field (`Region2`) over the base
//!   field; items not in a group keep their own name there.
//!
//! The group items (`groupItems`) are what pivot fields' items point at;
//! each record's group is worked out from its base field's value.

use std::collections::HashMap;

use roxmltree::Node;

use crate::pivot::{CacheField, Val, format_number, iso_of_serial, serial_of_iso};
use crate::pivot_calc::ymd;

/// A level of date grouping (`ST_GroupBy`), from the finest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DateLevel {
    Seconds,
    Minutes,
    Hours,
    Days,
    Months,
    Quarters,
    Years,
}

impl DateLevel {
    pub const ALL: [DateLevel; 7] = [
        DateLevel::Seconds,
        DateLevel::Minutes,
        DateLevel::Hours,
        DateLevel::Days,
        DateLevel::Months,
        DateLevel::Quarters,
        DateLevel::Years,
    ];

    /// Excel's name for it (also the name of the field it adds).
    pub fn label(self) -> &'static str {
        match self {
            DateLevel::Seconds => "Seconds",
            DateLevel::Minutes => "Minutes",
            DateLevel::Hours => "Hours",
            DateLevel::Days => "Days",
            DateLevel::Months => "Months",
            DateLevel::Quarters => "Quarters",
            DateLevel::Years => "Years",
        }
    }

    fn xml(self) -> &'static str {
        match self {
            DateLevel::Seconds => "seconds",
            DateLevel::Minutes => "minutes",
            DateLevel::Hours => "hours",
            DateLevel::Days => "days",
            DateLevel::Months => "months",
            DateLevel::Quarters => "quarters",
            DateLevel::Years => "years",
        }
    }

    fn parse(s: &str) -> Option<DateLevel> {
        DateLevel::ALL.into_iter().find(|l| l.xml() == s)
    }
}

/// How a range group splits values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum By {
    Numbers,
    Date(DateLevel),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RangeGroup {
    pub by: By,
    pub auto_start: bool,
    pub auto_end: bool,
    pub start: f64,
    pub end: f64,
    /// `By` for numbers, the number of days for days.
    pub interval: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum GroupKind {
    Range(RangeGroup),
    /// The group item of each of the base field's shared items.
    Discrete(Vec<usize>),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FieldGroup {
    /// The field whose values are grouped.
    pub base: Option<usize>,
    /// The next coarser grouping field.
    pub par: Option<usize>,
    pub kind: Option<GroupKind>,
    pub items: Vec<Val>,
}

fn kid<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

impl FieldGroup {
    pub(crate) fn parse(g: Node, own: usize) -> Result<FieldGroup, String> {
        let num = |n: Node, a: &str| n.attribute(a).and_then(|v| v.parse::<f64>().ok());
        let base = g.attribute("base").and_then(|v| v.parse().ok());
        let par = g.attribute("par").and_then(|v| v.parse().ok());
        let items: Vec<Val> = kid(g, "groupItems")
            .map(|gi| {
                gi.children()
                    .filter(|c| c.is_element())
                    .map(|c| match c.tag_name().name() {
                        "n" => Val::Num(num(c, "v").unwrap_or(0.0)),
                        "m" => Val::Missing,
                        _ => Val::Str(c.attribute("v").unwrap_or_default().to_owned()),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let kind = if let Some(r) = kid(g, "rangePr") {
            let by = match r.attribute("groupBy").unwrap_or("range") {
                "range" => By::Numbers,
                s => By::Date(DateLevel::parse(s).ok_or_else(|| format!("grouping by {s}"))?),
            };
            let date = |a: &str| r.attribute(a).and_then(serial_of_iso);
            let (start, end) = match by {
                By::Numbers => (num(r, "startNum"), num(r, "endNum")),
                By::Date(_) => (date("startDate"), date("endDate")),
            };
            let flag = |a: &str| !matches!(r.attribute(a), Some("0" | "false"));
            Some(GroupKind::Range(RangeGroup {
                by,
                auto_start: flag("autoStart"),
                auto_end: flag("autoEnd"),
                start: start.unwrap_or(0.0),
                end: end.unwrap_or(0.0),
                interval: num(r, "groupInterval")
                    .unwrap_or(1.0)
                    .max(f64::MIN_POSITIVE),
            }))
        } else if let Some(d) = kid(g, "discretePr") {
            let map: Vec<usize> = d
                .children()
                .filter(|c| c.is_element() && c.tag_name().name() == "x")
                .map(|x| x.attribute("v").and_then(|v| v.parse().ok()).unwrap_or(0))
                .collect();
            if map.iter().any(|&k| k >= items.len()) {
                return Err("a manual group points past its items".into());
            }
            Some(GroupKind::Discrete(map))
        } else {
            None
        };
        if kind.is_some() && base.is_none() {
            return Err("a grouped field without its base field".into());
        }
        let _ = own;
        Ok(FieldGroup {
            base,
            par,
            kind,
            items,
        })
    }

    /// The `<fieldGroup>` element.
    pub(crate) fn xml(&self, out: &mut String) {
        out.push_str("<fieldGroup");
        if let Some(p) = self.par {
            out.push_str(&format!(" par=\"{p}\""));
        }
        if let Some(b) = self.base {
            out.push_str(&format!(" base=\"{b}\""));
        }
        let Some(kind) = &self.kind else {
            out.push_str("/>");
            return;
        };
        out.push('>');
        match kind {
            GroupKind::Range(r) => {
                out.push_str("<rangePr");
                if !r.auto_start {
                    out.push_str(" autoStart=\"0\"");
                }
                if !r.auto_end {
                    out.push_str(" autoEnd=\"0\"");
                }
                match r.by {
                    By::Numbers => out.push_str(&format!(
                        " startNum=\"{}\" endNum=\"{}\"",
                        format_number(r.start),
                        format_number(r.end)
                    )),
                    By::Date(l) => out.push_str(&format!(
                        " groupBy=\"{}\" startDate=\"{}\" endDate=\"{}\"",
                        l.xml(),
                        iso_of_serial(r.start),
                        iso_of_serial(r.end)
                    )),
                }
                if r.interval != 1.0 {
                    out.push_str(&format!(" groupInterval=\"{}\"", format_number(r.interval)));
                }
                out.push_str("/>");
            }
            GroupKind::Discrete(map) => {
                out.push_str(&format!("<discretePr count=\"{}\">", map.len()));
                // `CT_Index`: `v` is required here.
                for k in map {
                    out.push_str(&format!("<x v=\"{k}\"/>"));
                }
                out.push_str("</discretePr>");
            }
        }
        out.push_str(&format!("<groupItems count=\"{}\">", self.items.len()));
        for v in &self.items {
            match v {
                Val::Str(s) => out.push_str(&format!("<s v=\"{}\"/>", esc(s))),
                Val::Missing => out.push_str("<m/>"),
                other => out.push_str(&format!("<s v=\"{}\"/>", esc(&other.label()))),
            }
        }
        out.push_str("</groupItems></fieldGroup>");
    }

    /// Whether the field's items are group items (a range or manual
    /// grouping; a field with only `par` keeps its own items).
    pub(crate) fn groups_items(&self) -> bool {
        self.kind.is_some()
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// `m/d/yyyy`, as Excel writes the bounds of a date range.
fn us_date(serial: f64) -> String {
    let (y, m, d) = ymd(serial);
    format!("{m}/{d}/{y}")
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Days of each month in the 366-day grid Excel groups days by.
const MONTH_DAYS: [i64; 12] = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

/// The group items of range `r` (first `<start`, last `>end`); `ints`
/// when every value is a whole number (`1-10`, `11-20` rather than `0-10`,
/// `10-20`).
pub(crate) fn range_items(r: &RangeGroup, ints: bool) -> Vec<Val> {
    let s = |x: String| Val::Str(x);
    let mut out = Vec::new();
    match r.by {
        By::Numbers => {
            out.push(s(format!("<{}", format_number(r.start))));
            let mut a = r.start;
            let step = r.interval;
            let whole = ints && step.fract() == 0.0 && r.start.fract() == 0.0;
            let mut n = 0;
            while a <= r.end && n < 10_000 {
                let b = if whole { a + step - 1.0 } else { a + step };
                out.push(s(format!("{}-{}", format_number(a), format_number(b))));
                a += step;
                n += 1;
            }
            out.push(s(format!(">{}", format_number(r.end))));
        }
        By::Date(level) => {
            out.push(s(format!("<{}", us_date(r.start))));
            match level {
                DateLevel::Years => {
                    let (y0, _, _) = ymd(r.start);
                    let (y1, _, _) = ymd(r.end);
                    for y in y0..=y1.max(y0) {
                        out.push(s(y.to_string()));
                    }
                }
                DateLevel::Quarters => (1..=4).for_each(|q| out.push(s(format!("Qtr{q}")))),
                DateLevel::Months => MONTHS.iter().for_each(|m| out.push(s((*m).to_owned()))),
                DateLevel::Days if r.interval > 1.0 => {
                    let step = r.interval.round();
                    let mut a = r.start.floor();
                    let mut n = 0;
                    while a <= r.end && n < 10_000 {
                        out.push(s(format!("{} - {}", us_date(a), us_date(a + step - 1.0))));
                        a += step;
                        n += 1;
                    }
                }
                DateLevel::Days => {
                    for (m, days) in MONTH_DAYS.iter().enumerate() {
                        for d in 1..=*days {
                            out.push(s(format!("{d}-{}", MONTHS[m])));
                        }
                    }
                }
                DateLevel::Hours => {
                    for h in 0..24 {
                        let (n, ap) = match h {
                            0 => (12, "AM"),
                            1..=11 => (h, "AM"),
                            12 => (12, "PM"),
                            _ => (h - 12, "PM"),
                        };
                        out.push(s(format!("{n} {ap}")));
                    }
                }
                DateLevel::Minutes | DateLevel::Seconds => {
                    (0..60).for_each(|m| out.push(s(format!(":{m:02}"))));
                }
            }
            out.push(s(format!(">{}", us_date(r.end))));
        }
    }
    out
}

/// The group item value `v` falls in (an index into [`range_items`]).
pub(crate) fn range_index(r: &RangeGroup, items: usize, v: &Val) -> Option<usize> {
    let x = v.number()?;
    let last = items.checked_sub(1)?;
    match r.by {
        By::Numbers => {
            if x < r.start {
                return Some(0);
            }
            if x > r.end {
                return Some(last);
            }
            let k = ((x - r.start) / r.interval).floor() as usize;
            Some((1 + k).min(last.saturating_sub(1)).max(1))
        }
        By::Date(level) => {
            let day = x.floor();
            if day < r.start.floor() {
                return Some(0);
            }
            if x > r.end + 1.0 - 1e-9 && day > r.end.floor() {
                return Some(last);
            }
            let (y, m, d) = ymd(x);
            let secs = ((x - day) * 86_400.0).round() as i64;
            let k = match level {
                DateLevel::Years => (y - ymd(r.start).0) as usize,
                DateLevel::Quarters => ((m - 1) / 3) as usize,
                DateLevel::Months => (m - 1) as usize,
                DateLevel::Days if r.interval > 1.0 => {
                    ((day - r.start.floor()) / r.interval.round()).floor() as usize
                }
                DateLevel::Days => {
                    (MONTH_DAYS[..(m - 1) as usize].iter().sum::<i64>() + d - 1) as usize
                }
                DateLevel::Hours => (secs / 3600).min(23) as usize,
                DateLevel::Minutes => (secs / 60 % 60) as usize,
                DateLevel::Seconds => (secs % 60) as usize,
            };
            Some((1 + k).min(last.saturating_sub(1)).max(1))
        }
    }
}

/// Set a range group's automatic bounds from `values` (dates: the first
/// day and the day after the last, as Excel does).
pub(crate) fn auto_bounds(r: &mut RangeGroup, values: &[f64]) {
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !lo.is_finite() {
        return;
    }
    match r.by {
        By::Numbers => {
            if r.auto_start {
                r.start = lo;
            }
            if r.auto_end {
                r.end = hi;
            }
        }
        By::Date(_) => {
            if r.auto_start {
                r.start = lo.floor();
            }
            if r.auto_end {
                r.end = hi.floor() + 1.0;
            }
        }
    }
}

/// Group the rebuilt cache `fields` (database fields first, from the
/// source) as `old` was: grouping and calculated fields come back after
/// them, every link (`base`, `par`) by name, range items worked out again
/// from `records`, and manual groups following their items by value.
pub(crate) fn regroup(old: &[CacheField], fields: &mut Vec<CacheField>, records: &[Vec<Val>]) {
    let db = fields.len();
    for f in old.iter().filter(|f| !f.database) {
        if fields.iter().any(|n| n.name.eq_ignore_ascii_case(&f.name)) {
            continue;
        }
        fields.push(CacheField {
            items: Vec::new(),
            ..f.clone()
        });
    }
    let name_of = |i: Option<usize>| i.and_then(|i| old.get(i)).map(|f| f.name.to_lowercase());
    let index_of = |fields: &[CacheField], name: Option<String>| {
        name.and_then(|n| fields.iter().position(|f| f.name.to_lowercase() == n))
    };
    for j in 0..fields.len() {
        let Some(prev) = old
            .iter()
            .find(|o| o.name.eq_ignore_ascii_case(&fields[j].name))
        else {
            continue;
        };
        let Some(g) = &prev.group else {
            fields[j].group = None;
            continue;
        };
        let base = index_of(fields, name_of(g.base));
        let par = index_of(fields, name_of(g.par));
        if g.kind.is_some() && base.is_none() {
            // Its base field is gone: so is the grouping.
            fields[j].group = None;
            continue;
        }
        let kind = match &g.kind {
            None => None,
            Some(GroupKind::Range(r)) => Some(GroupKind::Range(r.clone())),
            Some(GroupKind::Discrete(map)) => {
                // Group of each old base item, by value; new items alone.
                let ob = g.base.and_then(|b| old.get(b));
                let mut by_key: HashMap<String, String> = HashMap::new();
                if let Some(ob) = ob {
                    for (x, gi) in map.iter().enumerate() {
                        if let (Some(v), Some(Val::Str(name))) = (ob.items.get(x), g.items.get(*gi))
                        {
                            by_key.insert(v.key(), name.clone());
                        }
                    }
                }
                let nb = base.map(|b| fields[b].items.clone()).unwrap_or_default();
                let mut names: Vec<String> = Vec::new();
                let mut new_map = Vec::with_capacity(nb.len());
                for v in &nb {
                    let name = by_key.get(&v.key()).cloned().unwrap_or_else(|| v.label());
                    let k = match names.iter().position(|n| *n == name) {
                        Some(k) => k,
                        None => {
                            names.push(name);
                            names.len() - 1
                        }
                    };
                    new_map.push(k);
                }
                // Keep the old order of group items.
                let mut order: Vec<usize> = (0..names.len()).collect();
                let old_pos = |n: &str| {
                    g.items
                        .iter()
                        .position(|v| matches!(v, Val::Str(s) if s == n))
                        .unwrap_or(usize::MAX)
                };
                order.sort_by_key(|&k| (old_pos(&names[k]), k));
                let mut rank = vec![0; names.len()];
                for (r, &k) in order.iter().enumerate() {
                    rank[k] = r;
                }
                fields[j].group = Some(FieldGroup {
                    base,
                    par,
                    kind: Some(GroupKind::Discrete(
                        new_map.iter().map(|k| rank[*k]).collect(),
                    )),
                    items: order.iter().map(|&k| Val::Str(names[k].clone())).collect(),
                });
                continue;
            }
        };
        fields[j].group = Some(FieldGroup {
            base,
            par,
            kind,
            items: g.items.clone(),
        });
    }
    // Range items from the values now.
    for field in fields.iter_mut() {
        let Some(FieldGroup {
            base: Some(b),
            kind: Some(GroupKind::Range(r)),
            ..
        }) = &field.group
        else {
            continue;
        };
        let b = *b;
        if b >= db {
            continue;
        }
        let values: Vec<f64> = records
            .iter()
            .filter_map(|rec| rec.get(b)?.number())
            .collect();
        let ints = values.iter().all(|x| x.fract() == 0.0);
        let mut r = r.clone();
        auto_bounds(&mut r, &values);
        let items = range_items(&r, ints);
        if let Some(g) = field.group.as_mut() {
            g.kind = Some(GroupKind::Range(r));
            g.items = items;
        }
    }
}

/// Each record's group item in field `f` of `fields` (`None` when the
/// field isn't grouped by range or by hand).
pub(crate) fn group_positions(
    fields: &[CacheField],
    f: usize,
    records: &[Vec<Val>],
) -> Option<Vec<Option<usize>>> {
    let g = fields.get(f)?.group.as_ref()?;
    let base = g.base?;
    match g.kind.as_ref()? {
        GroupKind::Range(r) => Some(
            records
                .iter()
                .map(|rec| range_index(r, g.items.len(), rec.get(base)?))
                .collect(),
        ),
        GroupKind::Discrete(map) => {
            let bf = fields.get(base)?;
            let index: HashMap<String, usize> = bf
                .items
                .iter()
                .enumerate()
                .map(|(x, v)| (v.key(), x))
                .collect();
            Some(
                records
                    .iter()
                    .map(|rec| map.get(*index.get(&rec.get(base)?.key())?).copied())
                    .collect(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> f64 {
        serial_of_iso(s).unwrap()
    }

    #[test]
    fn number_ranges() {
        let r = RangeGroup {
            by: By::Numbers,
            auto_start: false,
            auto_end: false,
            start: 1.0,
            end: 30.0,
            interval: 10.0,
        };
        let items = range_items(&r, true);
        let labels: Vec<String> = items.iter().map(Val::label).collect();
        assert_eq!(labels, ["<1", "1-10", "11-20", "21-30", ">30"]);
        let at = |x: f64| range_index(&r, items.len(), &Val::Num(x));
        assert_eq!(at(0.0), Some(0));
        assert_eq!(at(1.0), Some(1));
        assert_eq!(at(10.0), Some(1));
        assert_eq!(at(11.0), Some(2));
        assert_eq!(at(30.0), Some(3));
        assert_eq!(at(31.0), Some(4));
    }

    #[test]
    fn date_levels() {
        let mut r = RangeGroup {
            by: By::Date(DateLevel::Months),
            auto_start: true,
            auto_end: true,
            start: 0.0,
            end: 0.0,
            interval: 1.0,
        };
        auto_bounds(&mut r, &[d("2025-12-31"), d("2026-02-03")]);
        assert_eq!(r.end, d("2026-02-04"));
        let items = range_items(&r, true);
        assert_eq!(items.len(), 14);
        assert_eq!(items[0], Val::Str("<12/31/2025".into()));
        assert_eq!(items[13], Val::Str(">2/4/2026".into()));
        assert_eq!(
            range_index(&r, 14, &Val::Date(d("2026-02-03"))),
            Some(2),
            "Feb"
        );
        r.by = By::Date(DateLevel::Years);
        let items = range_items(&r, true);
        let labels: Vec<String> = items.iter().map(Val::label).collect();
        assert_eq!(labels, ["<12/31/2025", "2025", "2026", ">2/4/2026"]);
        assert_eq!(
            range_index(&r, items.len(), &Val::Date(d("2026-01-01"))),
            Some(2)
        );
        r.by = By::Date(DateLevel::Days);
        let items = range_items(&r, true);
        assert_eq!(items.len(), 368);
        assert_eq!(items[60], Val::Str("29-Feb".into()));
        assert_eq!(range_index(&r, 368, &Val::Date(d("2026-02-01"))), Some(32));
        r.interval = 7.0;
        let items = range_items(&r, true);
        assert_eq!(items[1], Val::Str("12/31/2025 - 1/6/2026".into()));
        assert_eq!(
            range_index(&r, items.len(), &Val::Date(d("2026-01-07"))),
            Some(2)
        );
        r.by = By::Date(DateLevel::Hours);
        r.interval = 1.0;
        let items = range_items(&r, true);
        assert_eq!(items[1], Val::Str("12 AM".into()));
        assert_eq!(
            range_index(&r, items.len(), &Val::Date(d("2026-01-01") + 0.75)),
            Some(19),
            "6 PM"
        );
    }
}
