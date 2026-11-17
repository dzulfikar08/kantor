//! AutoFilter by top 10, average, dates, colour and icon, and sorting by
//! colour, icon or a custom list, as Excel does them.
//!
//! The filters are kept as Excel writes them (`<top10>`, `<dynamicFilter>`,
//! `<colorFilter dxfId>`, `<iconFilter>`); a colour is named by a
//! differential format (`kantor_dxf_index`, vendor/ironcalc_base §30). A
//! column's filter is turned into a [`Test`] once (the cut-off of a top 10,
//! the average, today, the colour) and each cell checked against it.
//!
//! * Top 10 counts the column's numbers (all data rows); ties with the last
//!   one kept are kept too. Percent keeps that share of the numbers.
//! * Colours are the ones the cell shows: its fill or font colour with
//!   conditional formatting applied. No fill and automatic font colour are
//!   colours to filter and sort by too, as in Excel.
//! * Icons are conditional formatting's icon sets, named by set and position
//!   (lowest bucket first).
//! * Sorting by a colour or icon puts the cells that have it on top (or at
//!   the bottom with `descending`); a custom list sorts by position in the
//!   list (values not in it after them).

use ironcalc_base::types::{Color, Dxf, DxfFont, Fill};

use crate::cond_format::CfIcon;
use crate::filter::{FilterKind, Probe, passes};
use crate::format::{Rgb, parse_rgb};
use crate::sort::{CellValue, cell_value};
use crate::{CellRange, Workbook, cell_name};

/// Dynamic filter types Kantor evaluates.
pub(crate) fn dynamic_known(t: &str) -> bool {
    matches!(t, "aboveAverage" | "belowAverage")
        || crate::pivot_calc::filter_family(t) == Some(crate::pivot_calc::FilterFamily::Date)
            && !t.starts_with("date")
}

/// Excel's icon sets, in its own order.
const SETS: [&str; 20] = [
    "3Arrows",
    "3ArrowsGray",
    "3Flags",
    "3TrafficLights1",
    "3TrafficLights2",
    "3Signs",
    "3Symbols",
    "3Symbols2",
    "3Stars",
    "3Triangles",
    "4Arrows",
    "4ArrowsGray",
    "4RedToBlack",
    "4Rating",
    "4TrafficLights",
    "5Arrows",
    "5ArrowsGray",
    "5Rating",
    "5Quarters",
    "5Boxes",
];

/// What a column's filter checks each cell against.
pub(crate) enum Test {
    Plain(FilterKind),
    AtLeast(f64),
    AtMost(f64),
    Above(f64),
    Below(f64),
    Date(String, f64),
    Fill(Option<Rgb>),
    Font(Option<Rgb>),
    Icon(Option<CfIcon>),
    /// Nothing to check (no numbers for a top 10): every row stays hidden
    /// or shown as it is.
    Keep,
    /// No cell passes.
    None,
}

/// What a sort level sorts by.
#[derive(Debug, Clone, PartialEq)]
pub enum SortOn {
    Values,
    /// Cells with the fill colour of a differential format first.
    CellColor {
        dxf: u32,
    },
    FontColor {
        dxf: u32,
    },
    /// Cells showing this icon first.
    Icon {
        set: String,
        id: Option<u32>,
    },
    /// By position in the list (Excel's custom lists: `Low,Medium,High`).
    List(Vec<String>),
}

/// One level of a sort: a column (absolute), its direction (for colours and
/// icons: on the bottom) and what it sorts by.
#[derive(Debug, Clone, PartialEq)]
pub struct SortLevel {
    pub col: u32,
    pub descending: bool,
    pub on: SortOn,
}

impl From<crate::SortKey> for SortLevel {
    fn from(k: crate::SortKey) -> Self {
        SortLevel {
            col: k.col,
            descending: k.descending,
            on: SortOn::Values,
        }
    }
}

/// Colours and icons a column shows, for the filter menu.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ColumnLooks {
    /// Fill colours (`None`: no fill), in order of first appearance.
    pub fills: Vec<Option<Rgb>>,
    /// Font colours (`None`: automatic).
    pub fonts: Vec<Option<Rgb>>,
    /// Icons: set, position and the icon (`id` and icon `None`: no icon).
    pub icons: Vec<(String, Option<u32>, Option<CfIcon>)>,
}

impl Workbook {
    /// The colour of differential format `dxf`: its fill's, or its font's.
    pub fn dxf_color(&self, dxf: u32, font: bool) -> Option<Rgb> {
        let model = self.model.get_model();
        let d = model.workbook.styles.dxfs.get(dxf as usize)?;
        let color = if font {
            &d.font.as_ref()?.color
        } else {
            &d.fill.as_ref()?.color
        };
        match color {
            Color::None => None,
            c => parse_rgb(&c.to_rgb(&model.workbook.theme)),
        }
    }

    /// A differential format for colour `color` (`None`: no fill /
    /// automatic), added if the workbook has none like it.
    pub fn color_dxf(&mut self, color: Option<Rgb>, font: bool) -> u32 {
        let c = color.map_or(Color::None, |c| {
            Color::Rgb(format!("#{:06X}", c & 0xFF_FFFF))
        });
        let dxf = if font {
            Dxf {
                font: Some(DxfFont {
                    strike: None,
                    u: None,
                    b: None,
                    i: None,
                    sz: None,
                    color: c,
                }),
                ..Dxf::default()
            }
        } else {
            Dxf {
                fill: Some(Fill { color: c }),
                ..Dxf::default()
            }
        };
        self.model.kantor_dxf_index(dxf)
    }

    /// The filter by colour `color` (`None`: no fill / automatic).
    pub fn color_filter(&mut self, color: Option<Rgb>, font: bool) -> FilterKind {
        FilterKind::Color {
            font,
            dxf: self.color_dxf(color, font),
        }
    }

    /// The fill and font colours a cell shows and its icon, conditional
    /// formatting applied.
    pub(crate) fn shown_look(
        &mut self,
        sheet: u32,
        row: u32,
        col: u32,
    ) -> (Option<Rgb>, Option<Rgb>, Option<CfIcon>) {
        let cond = self.conditional(sheet, row, col);
        let style = cond
            .as_ref()
            .and_then(|c| c.style.clone())
            .unwrap_or_else(|| self.cell_style(sheet, row, col));
        (
            style.fill,
            style.font_color,
            cond.as_ref().and_then(|c| c.icon),
        )
    }

    /// The icon `id` of set `set` as conditional formatting draws it.
    pub fn set_icon(&self, set: &str, id: u32) -> Option<CfIcon> {
        let icons = ironcalc_base::cf_types::icon_set_icons(set)?;
        let (icon, color) = icons.get(id as usize)?;
        Some(crate::cond_format::icon_of_pub(
            self.model.get_model(),
            icon,
            color,
        ))
    }

    /// The set and position of an icon (the first set that has it).
    fn icon_name(&self, icon: CfIcon) -> Option<(String, u32)> {
        SETS.iter().find_map(|set| {
            let n = ironcalc_base::cf_types::icon_set_icons(set)?.len() as u32;
            (0..n)
                .find(|&i| self.set_icon(set, i) == Some(icon))
                .map(|i| ((*set).to_owned(), i))
        })
    }

    /// The colours and icons column `col` (absolute) of `scope`'s filter
    /// shows, for its Filter by Color and Sort by Color menus.
    pub fn filter_looks(
        &mut self,
        sheet: u32,
        scope: &crate::FilterScope,
        col: u32,
    ) -> ColumnLooks {
        match self.filter(sheet, scope) {
            Some(f) => self.column_looks(sheet, f.area, col),
            None => ColumnLooks::default(),
        }
    }

    /// The colours and icons of column `col` (absolute) of `area`'s data
    /// rows.
    pub fn column_looks(&mut self, sheet: u32, area: CellRange, col: u32) -> ColumnLooks {
        let mut out = ColumnLooks::default();
        let mut no_icon = false;
        for row in area.top + 1..=area.bottom {
            let (fill, font, icon) = self.shown_look(sheet, row, col);
            if !out.fills.contains(&fill) {
                out.fills.push(fill);
            }
            if !out.fonts.contains(&font) {
                out.fonts.push(font);
            }
            match icon {
                Some(i) => {
                    if !out.icons.iter().any(|x| x.2 == Some(i))
                        && let Some((set, id)) = self.icon_name(i)
                    {
                        out.icons.push((set, Some(id), Some(i)));
                    }
                }
                None => no_icon = true,
            }
        }
        if no_icon && !out.icons.is_empty() {
            let set = out.icons[0].0.clone();
            out.icons.push((set, None, None));
        }
        out
    }

    /// [`Self::column_test`] without colours and icons: those columns keep
    /// their rows as shown (they were filtered when applied), for readers
    /// that can't work out conditional formatting (`&self`).
    pub(crate) fn column_test_shown(
        &self,
        sheet: u32,
        area: CellRange,
        col: u32,
        kind: &FilterKind,
    ) -> Test {
        match kind {
            FilterKind::Color { .. } | FilterKind::Icon { .. } => Test::Keep,
            k => self.column_test_values(sheet, area, col, k),
        }
    }

    /// The test for filter `kind` of column `col` (absolute) over `area`.
    pub(crate) fn column_test(
        &mut self,
        sheet: u32,
        area: CellRange,
        col: u32,
        kind: &FilterKind,
    ) -> Test {
        match kind {
            FilterKind::Color { font, dxf } => {
                let c = self.dxf_color(*dxf, *font);
                if *font { Test::Font(c) } else { Test::Fill(c) }
            }
            FilterKind::Icon { set, id } => match id {
                Some(i) => match self.set_icon(set, *i) {
                    Some(icon) => Test::Icon(Some(icon)),
                    None => Test::Keep,
                },
                None => Test::Icon(None),
            },
            k => self.column_test_values(sheet, area, col, k),
        }
    }

    fn column_test_values(&self, sheet: u32, area: CellRange, col: u32, kind: &FilterKind) -> Test {
        let numbers = |wb: &Workbook| -> Vec<f64> {
            let model = wb.model.get_model();
            (area.top + 1..=area.bottom)
                .filter_map(|r| match cell_value(model, sheet, r, col) {
                    CellValue::Number(n) => Some(n),
                    _ => None,
                })
                .collect()
        };
        match kind {
            FilterKind::Top10 {
                top,
                percent,
                value,
            } => {
                let mut v = numbers(self);
                if v.is_empty() {
                    return Test::None;
                }
                v.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
                if !top {
                    v.reverse();
                }
                let n = if *percent {
                    (v.len() as f64 * value / 100.0).floor().max(1.0) as usize
                } else {
                    value.max(0.0).round() as usize
                };
                if n == 0 {
                    return Test::None;
                }
                let cut = v[n.min(v.len()) - 1];
                if *top {
                    Test::AtLeast(cut)
                } else {
                    Test::AtMost(cut)
                }
            }
            FilterKind::Dynamic(t) if t == "aboveAverage" || t == "belowAverage" => {
                let v = numbers(self);
                if v.is_empty() {
                    return Test::None;
                }
                let avg = v.iter().sum::<f64>() / v.len() as f64;
                if t == "aboveAverage" {
                    Test::Above(avg)
                } else {
                    Test::Below(avg)
                }
            }
            FilterKind::Dynamic(t) => Test::Date(t.clone(), crate::pivot_calc::today()),
            FilterKind::Other(_) | FilterKind::Color { .. } | FilterKind::Icon { .. } => Test::Keep,
            other => Test::Plain(other.clone()),
        }
    }

    /// Whether cell `(row, col)` passes `test`; `visible`: shown now.
    pub(crate) fn passes_test(
        &mut self,
        test: &Test,
        sheet: u32,
        row: u32,
        col: u32,
        visible: bool,
    ) -> bool {
        match test {
            Test::Fill(c) => self.shown_look(sheet, row, col).0 == *c,
            Test::Font(c) => self.shown_look(sheet, row, col).1 == *c,
            Test::Icon(i) => self.shown_look(sheet, row, col).2 == *i,
            t => self.passes_shown(t, sheet, row, col, visible),
        }
    }

    /// [`Self::passes_test`] for tests from [`Self::column_test_shown`].
    pub(crate) fn passes_shown(
        &self,
        test: &Test,
        sheet: u32,
        row: u32,
        col: u32,
        visible: bool,
    ) -> bool {
        let number = |wb: &Workbook| match cell_value(wb.model.get_model(), sheet, row, col) {
            CellValue::Number(n) => Some(n),
            _ => None,
        };
        match test {
            Test::Plain(kind) => {
                let p: Probe = self.probe(sheet, row, col);
                passes(kind, &p, visible)
            }
            Test::AtLeast(c) => number(self).is_some_and(|n| n >= *c),
            Test::AtMost(c) => number(self).is_some_and(|n| n <= *c),
            Test::Above(c) => number(self).is_some_and(|n| n > *c),
            Test::Below(c) => number(self).is_some_and(|n| n < *c),
            Test::Date(t, today) => {
                number(self).is_some_and(|n| crate::pivot_calc::date_matches(t, n, "", "", *today))
            }
            Test::Fill(_) | Test::Font(_) | Test::Icon(_) | Test::Keep => visible,
            Test::None => false,
        }
    }

    /// A row's sort key for `level` over cell `(row, col)`: the cell's value,
    /// or its rank (0 first) for colours, icons and lists.
    pub(crate) fn sort_value(&mut self, level: &ResolvedLevel, sheet: u32, row: u32) -> CellValue {
        let col = level.col;
        match &level.on {
            Resolved::Values => cell_value(self.model.get_model(), sheet, row, col),
            Resolved::Fill(c) => {
                CellValue::Number(f64::from(self.shown_look(sheet, row, col).0 != *c))
            }
            Resolved::Font(c) => {
                CellValue::Number(f64::from(self.shown_look(sheet, row, col).1 != *c))
            }
            Resolved::Icon(i) => {
                CellValue::Number(f64::from(self.shown_look(sheet, row, col).2 != *i))
            }
            Resolved::List(items) => {
                let text = self.display_value(sheet, row, col).trim().to_lowercase();
                match items.iter().position(|i| *i == text) {
                    Some(p) => CellValue::Number(p as f64),
                    None if text.is_empty() => CellValue::Blank,
                    None => CellValue::Number(items.len() as f64),
                }
            }
        }
    }

    /// `level` with its colour or icon worked out.
    pub(crate) fn resolve_level(&self, level: &SortLevel) -> ResolvedLevel {
        let on = match &level.on {
            SortOn::Values => Resolved::Values,
            SortOn::CellColor { dxf } => Resolved::Fill(self.dxf_color(*dxf, false)),
            SortOn::FontColor { dxf } => Resolved::Font(self.dxf_color(*dxf, true)),
            SortOn::Icon { set, id } => Resolved::Icon(id.and_then(|i| self.set_icon(set, i))),
            SortOn::List(items) => {
                let mut items: Vec<String> =
                    items.iter().map(|i| i.trim().to_lowercase()).collect();
                if level.descending {
                    items.reverse();
                }
                Resolved::List(items)
            }
        };
        // Ranks already hold the direction, except "on bottom" for colours
        // and icons (rank 0 last).
        let descending = match on {
            Resolved::Values => level.descending,
            Resolved::List(_) => false,
            _ => level.descending,
        };
        ResolvedLevel {
            col: level.col,
            descending,
            on,
        }
    }
}

/// A sort level ready to compare rows with.
pub(crate) struct ResolvedLevel {
    pub col: u32,
    pub descending: bool,
    pub on: Resolved,
}

pub(crate) enum Resolved {
    Values,
    Fill(Option<Rgb>),
    Font(Option<Rgb>),
    Icon(Option<CfIcon>),
    List(Vec<String>),
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A `<sortState>` for `data` (the rows sorted, without the header) sorted
/// by `levels`, as Excel writes it.
pub(crate) fn sort_state_xml(data: CellRange, levels: &[SortLevel]) -> String {
    let mut x = format!(
        "<sortState ref=\"{}:{}\">",
        cell_name(data.top, data.left),
        cell_name(data.bottom, data.right)
    );
    for l in levels {
        x.push_str("<sortCondition");
        if l.descending {
            x.push_str(" descending=\"1\"");
        }
        match &l.on {
            SortOn::Values => {}
            SortOn::CellColor { dxf } => {
                x.push_str(&format!(" sortBy=\"cellColor\" dxfId=\"{dxf}\""));
            }
            SortOn::FontColor { dxf } => {
                x.push_str(&format!(" sortBy=\"fontColor\" dxfId=\"{dxf}\""));
            }
            SortOn::Icon { set, id } => {
                x.push_str(&format!(" sortBy=\"icon\" iconSet=\"{}\"", esc(set)));
                if let Some(i) = id {
                    x.push_str(&format!(" iconId=\"{i}\""));
                }
            }
            SortOn::List(items) => {
                x.push_str(&format!(" customList=\"{}\"", esc(&items.join(","))));
            }
        }
        x.push_str(&format!(
            " ref=\"{}:{}\"/>",
            cell_name(data.top, l.col),
            cell_name(data.bottom, l.col)
        ));
    }
    x.push_str("</sortState>");
    x
}

/// The sort levels of a `<sortCondition>`.
pub(crate) fn parse_level(c: roxmltree::Node) -> Option<SortLevel> {
    let range = crate::shift::parse_range(c.attribute("ref")?)?.area();
    let dxf = || c.attribute("dxfId").and_then(|v| v.parse().ok());
    let on = match (c.attribute("sortBy"), c.attribute("customList")) {
        (Some("cellColor"), _) => SortOn::CellColor { dxf: dxf()? },
        (Some("fontColor"), _) => SortOn::FontColor { dxf: dxf()? },
        (Some("icon"), _) => SortOn::Icon {
            set: c
                .attribute("iconSet")
                .unwrap_or("3TrafficLights1")
                .to_owned(),
            id: c.attribute("iconId").and_then(|v| v.parse().ok()),
        },
        (_, Some(list)) => SortOn::List(list.split(',').map(str::to_owned).collect()),
        _ => SortOn::Values,
    };
    Some(SortLevel {
        col: range.left,
        descending: matches!(c.attribute("descending"), Some("1" | "true")),
        on,
    })
}
