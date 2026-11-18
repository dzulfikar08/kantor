//! Sparklines (Excel 2010's `x14:sparklineGroups` in a worksheet's
//! `extLst`): small line, column and win/loss charts drawn in a cell.
//!
//! IronCalc doesn't model them. They are read from the opened file and
//! carried into saves verbatim by `preserve` (which moves their locations
//! and data with inserted and deleted rows and columns); here the same moves
//! ([`crate::shift`]) map each sparkline to the current workbook, its values
//! are read from the cells now, and [`Sparkline::geometry`] lays it out the
//! way Excel draws it, for the grid and for printing.

use std::collections::HashMap;
use std::sync::Arc;

use ironcalc_base::types::Color;
use roxmltree::{Document, Node};

use crate::cond_format::{Value, cell_value, parse_sqref};
use crate::format::{Rgb, parse_rgb};
use crate::shift::{Layout, references};
use crate::{CellRange, Workbook};

/// The sparkline extension of `extLst`.
const EXTENSION: &str = "{05C60535-1F16-4fd2-B633-F4F36F0B64E0}";
/// Values read for one sparkline at most (a whole-row source).
const MAX_POINTS: usize = 2_000;

/// What a sparkline draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SparkKind {
    #[default]
    Line,
    Column,
    /// Win/Loss (`stacked`): an up bar for each positive value, a down bar
    /// for each negative one.
    WinLoss,
}

/// How empty cells show (Excel's Hidden and Empty Cell Settings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmptyCells {
    #[default]
    Gap,
    Zero,
    /// A line joins the points on either side.
    Span,
}

/// Where an axis end comes from.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum AxisEnd {
    /// The sparkline's own values.
    #[default]
    Individual,
    /// Every sparkline of its group.
    Group,
    Custom(f64),
}

/// A group's settings, colours unresolved.
#[derive(Debug, Clone, Default)]
struct GroupLook {
    kind: SparkKind,
    /// Line width, points.
    weight: f32,
    markers: bool,
    high: bool,
    low: bool,
    first: bool,
    last: bool,
    negative: bool,
    axis: bool,
    empty: EmptyCells,
    hidden: bool,
    min: AxisEnd,
    max: AxisEnd,
    right_to_left: bool,
    colors: [Option<Color>; 8],
}

/// Colour slots of [`GroupLook::colors`].
const SERIES: usize = 0;
const NEGATIVE: usize = 1;
const AXIS: usize = 2;
const MARKERS: usize = 3;
const FIRST: usize = 4;
const LAST: usize = 5;
const HIGH: usize = 6;
const LOW: usize = 7;

/// A group as read from the file, on sheet `sheet_id`: each sparkline's
/// location (`sqref`) and data (`f`, a reference with its sheet), and the
/// group's dates (a date axis), if any.
#[derive(Debug, Clone)]
struct Raw {
    sheet_id: u32,
    look: GroupLook,
    dates: Option<String>,
    items: Vec<(String, String)>,
}

/// The file's sparklines and the layout they were read in.
#[derive(Default)]
pub(crate) struct Sparklines {
    raw: Vec<Raw>,
    base: Layout,
    state: State,
}

/// Sparklines of one workbook state, by sheet, made on first use.
#[derive(Default)]
struct State {
    key: (u64, u64),
    sheets: HashMap<u32, Arc<SheetSparklines>>,
}

/// A sheet's sparklines by cell `(row, col)`.
type SheetSparklines = HashMap<(u32, u32), Arc<Sparkline>>;

/// A sparkline being made: its cell, values and date-axis positions.
type Made = ((u32, u32), Vec<Option<f64>>, Option<Vec<f64>>);

/// A sparkline ready to draw: its values now and its look.
#[derive(Debug, Clone, PartialEq)]
pub struct Sparkline {
    pub kind: SparkKind,
    /// The values in drawing order (left to right); `None` for an empty
    /// cell shown as a gap (or spanned).
    pub values: Vec<Option<f64>>,
    /// Where each value goes along the cell, `0.0..=1.0`, on a date axis;
    /// `None`: evenly spaced.
    pub positions: Option<Vec<f64>>,
    /// The vertical axis's ends.
    pub min: f64,
    pub max: f64,
    /// Line width, points.
    pub weight: f32,
    pub span_gaps: bool,
    pub series: Rgb,
    /// Colours of the points marked (`None`: not marked).
    pub negative: Option<Rgb>,
    pub markers: Option<Rgb>,
    pub first: Option<Rgb>,
    pub last: Option<Rgb>,
    pub high: Option<Rgb>,
    pub low: Option<Rgb>,
    /// The horizontal axis (at zero), when shown.
    pub axis: Option<Rgb>,
}

/// A sparkline laid out in a `w` × `h` box (any unit, y down).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SparkGeometry {
    /// Line segments of a line sparkline, in the series colour.
    pub lines: Vec<Vec<[f32; 2]>>,
    /// Bars: `[x, y, w, h]` and colour.
    pub bars: Vec<([f32; 4], Rgb)>,
    /// Markers: centre and colour.
    pub markers: Vec<([f32; 2], Rgb)>,
    /// The horizontal axis: its `y`.
    pub axis: Option<(f32, Rgb)>,
    /// Line width and marker diameter.
    pub line_width: f32,
    pub marker_size: f32,
}

impl Sparkline {
    /// Lay the sparkline out in `w` × `h`; `pt` is the size of a point in
    /// the same unit (for the line width).
    pub fn geometry(&self, w: f32, h: f32, pt: f32) -> SparkGeometry {
        let mut g = SparkGeometry {
            line_width: (self.weight * pt).max(pt * 0.25),
            ..Default::default()
        };
        let n = self.values.len();
        if n == 0 || w <= 0.0 || h <= 0.0 {
            return g;
        }
        // Excel keeps the drawing a little inside the cell.
        let (mx, my) = ((w * 0.04).min(3.0 * pt), (h * 0.12).min(3.0 * pt));
        let (x0, y0, iw, ih) = (mx, my, (w - 2.0 * mx).max(0.0), (h - 2.0 * my).max(0.0));
        let (lo, hi) = (self.min, self.max);
        let span = if hi > lo { hi - lo } else { 1.0 };
        let y_of = |v: f64| -> f32 {
            let t = if hi > lo {
                ((v - lo) / span).clamp(0.0, 1.0)
            } else {
                0.5
            };
            y0 + ih * (1.0 - t as f32)
        };
        let zero_inside = lo <= 0.0 && hi >= 0.0;
        let colour = |i: usize, v: f64| self.point_colour(i, v);
        match self.kind {
            SparkKind::Line => {
                g.marker_size = (g.line_width * 2.5).max(3.0 * pt).min(ih.max(pt));
                let x_of = |i: usize| -> f32 {
                    match &self.positions {
                        Some(p) => x0 + iw * p[i] as f32,
                        None if n == 1 => x0 + iw / 2.0,
                        None => x0 + iw * i as f32 / (n - 1) as f32,
                    }
                };
                let mut run: Vec<[f32; 2]> = Vec::new();
                for (i, v) in self.values.iter().enumerate() {
                    match v {
                        Some(v) => run.push([x_of(i), y_of(*v)]),
                        None if self.span_gaps => {}
                        None => {
                            if !run.is_empty() {
                                g.lines.push(std::mem::take(&mut run));
                            }
                        }
                    }
                }
                if !run.is_empty() {
                    g.lines.push(run);
                }
                for (i, v) in self.values.iter().enumerate() {
                    if let Some(v) = v
                        && let Some(c) = colour(i, *v).or(self.markers)
                    {
                        g.markers.push(([x_of(i), y_of(*v)], c));
                    }
                }
                if let Some(c) = self.axis
                    && zero_inside
                {
                    g.axis = Some((y_of(0.0), c));
                }
            }
            SparkKind::Column | SparkKind::WinLoss => {
                let slot = iw / n as f32;
                let bar_w = (slot * 0.8).max(pt * 0.5).min(slot);
                let x_of = |i: usize| -> f32 {
                    let centre = match &self.positions {
                        Some(p) => x0 + bar_w / 2.0 + (iw - bar_w) * p[i] as f32,
                        None => x0 + slot * (i as f32 + 0.5),
                    };
                    centre - bar_w / 2.0
                };
                let win_loss = self.kind == SparkKind::WinLoss;
                let base = if win_loss {
                    y0 + ih / 2.0
                } else if zero_inside {
                    y_of(0.0)
                } else if hi < 0.0 {
                    y0
                } else {
                    y0 + ih
                };
                for (i, v) in self.values.iter().enumerate() {
                    let Some(v) = *v else { continue };
                    let (top, bottom) = if win_loss {
                        match v.partial_cmp(&0.0) {
                            Some(std::cmp::Ordering::Greater) => (y0, base),
                            Some(std::cmp::Ordering::Less) => (base, y0 + ih),
                            _ => continue,
                        }
                    } else {
                        let y = y_of(v);
                        let (a, b) = if y < base { (y, base) } else { (base, y) };
                        // The smallest value still shows a sliver, as in
                        // Excel.
                        if b - a < pt * 0.75 {
                            if y <= base {
                                (base - pt * 0.75, base)
                            } else {
                                (base, base + pt * 0.75)
                            }
                        } else {
                            (a, b)
                        }
                    };
                    let c = colour(i, v).unwrap_or(self.series);
                    g.bars.push(([x_of(i), top, bar_w, bottom - top], c));
                }
                if let Some(c) = self.axis
                    && (win_loss || zero_inside)
                {
                    g.axis = Some((base, c));
                }
            }
        }
        g
    }

    /// The colour a highlighted point gets (high and low first, then
    /// first, last and negative points), if any.
    fn point_colour(&self, i: usize, v: f64) -> Option<Rgb> {
        let shown = || self.values.iter().filter_map(|v| *v);
        let first = self.values.iter().position(Option::is_some);
        let last = self.values.iter().rposition(Option::is_some);
        if let Some(c) = self.high
            && shown().fold(f64::MIN, f64::max) == v
        {
            return Some(c);
        }
        if let Some(c) = self.low
            && shown().fold(f64::MAX, f64::min) == v
        {
            return Some(c);
        }
        if let Some(c) = self.first
            && first == Some(i)
        {
            return Some(c);
        }
        if let Some(c) = self.last
            && last == Some(i)
        {
            return Some(c);
        }
        if let Some(c) = self.negative
            && v < 0.0
        {
            return Some(c);
        }
        None
    }
}

impl Sparklines {
    /// Every sparkline group of the package's worksheets, from
    /// [`crate::preserve::worksheet_skeletons`].
    pub(crate) fn read(skeletons: &Result<Vec<(u32, String)>, String>) -> Self {
        let mut raw = Vec::new();
        if let Ok(sheets) = skeletons {
            for (sheet_id, xml) in sheets {
                if xml.contains("sparklineGroup") {
                    raw.extend(parse_sheet(*sheet_id, xml));
                }
            }
        }
        Sparklines {
            raw,
            base: Layout::default(),
            state: State::default(),
        }
    }
}

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

fn flag(n: Node, name: &str) -> bool {
    matches!(n.attribute(name), Some("1" | "true"))
}

fn color_of(n: Node) -> Option<Color> {
    if let Some(rgb) = n.attribute("rgb") {
        return Some(Color::Rgb(format!(
            "#{}",
            rgb.get(rgb.len().saturating_sub(6)..)?
        )));
    }
    let slot = n.attribute("theme")?.parse().ok()?;
    let tint = n
        .attribute("tint")
        .and_then(|t| t.parse().ok())
        .unwrap_or(0.0);
    Some(Color::Theme(slot, tint))
}

fn axis_end(g: Node, kind: &str, manual: &str) -> AxisEnd {
    match g.attribute(kind) {
        Some("group") => AxisEnd::Group,
        Some("custom") => g
            .attribute(manual)
            .and_then(|v| v.parse().ok())
            .map_or(AxisEnd::Individual, AxisEnd::Custom),
        _ => AxisEnd::Individual,
    }
}

/// The sparkline groups of a worksheet (skeleton) XML.
fn parse_sheet(sheet_id: u32, xml: &str) -> Vec<Raw> {
    let doc = match Document::parse(xml) {
        Ok(d) => d,
        Err(e) => {
            log::warn!("sparklines of sheet {sheet_id} not read: {e}");
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    let exts = doc
        .root_element()
        .children()
        .filter(|n| n.tag_name().name() == "extLst")
        .flat_map(|l| l.children())
        .filter(|n| n.tag_name().name() == "ext" && n.attribute("uri") == Some(EXTENSION));
    for ext in exts {
        for g in ext
            .descendants()
            .filter(|n| n.tag_name().name() == "sparklineGroup")
        {
            let mut look = GroupLook {
                kind: match g.attribute("type") {
                    Some("column") => SparkKind::Column,
                    Some("stacked") => SparkKind::WinLoss,
                    _ => SparkKind::Line,
                },
                weight: g
                    .attribute("lineWeight")
                    .and_then(|w| w.parse().ok())
                    .unwrap_or(0.75),
                markers: flag(g, "markers"),
                high: flag(g, "high"),
                low: flag(g, "low"),
                first: flag(g, "first"),
                last: flag(g, "last"),
                negative: flag(g, "negative"),
                axis: flag(g, "displayXAxis"),
                empty: match g.attribute("displayEmptyCellsAs") {
                    Some("zero") => EmptyCells::Zero,
                    Some("span") => EmptyCells::Span,
                    _ => EmptyCells::Gap,
                },
                hidden: flag(g, "displayHidden"),
                min: axis_end(g, "minAxisType", "manualMin"),
                max: axis_end(g, "maxAxisType", "manualMax"),
                right_to_left: flag(g, "rightToLeft"),
                colors: Default::default(),
            };
            for (slot, name) in [
                (SERIES, "colorSeries"),
                (NEGATIVE, "colorNegative"),
                (AXIS, "colorAxis"),
                (MARKERS, "colorMarkers"),
                (FIRST, "colorFirst"),
                (LAST, "colorLast"),
                (HIGH, "colorHigh"),
                (LOW, "colorLow"),
            ] {
                look.colors[slot] = child(g, name).and_then(color_of);
            }
            // A date axis: the group's own `<xm:f>`.
            let dates = flag(g, "dateAxis")
                .then(|| child(g, "f").and_then(|f| f.text()).map(str::to_owned))
                .flatten();
            let items = child(g, "sparklines")
                .into_iter()
                .flat_map(|s| s.children())
                .filter(|s| s.tag_name().name() == "sparkline")
                .filter_map(|s| {
                    let f = child(s, "f")?.text()?.trim().to_owned();
                    let sqref = child(s, "sqref")?.text()?.trim().to_owned();
                    Some((sqref, f))
                })
                .collect::<Vec<_>>();
            if !items.is_empty() {
                out.push(Raw {
                    sheet_id,
                    look,
                    dates,
                    items,
                });
            }
        }
    }
    out
}

/// A sparkline's source cells, in reading order, before empty cells and
/// hidden rows or columns are dealt with.
struct Source {
    sheet: u32,
    range: CellRange,
}

impl Workbook {
    /// Whether `sheet` shows sparklines.
    pub fn has_sparklines(&mut self, sheet: u32) -> bool {
        !self.sparklines_of(sheet).is_empty()
    }

    /// The sparkline in a cell (0-based), with its values now.
    pub fn sparkline(&mut self, sheet: u32, row: u32, col: u32) -> Option<Arc<Sparkline>> {
        if self.sparklines.raw.is_empty() {
            return None;
        }
        self.sparklines_of(sheet).get(&(row, col)).cloned()
    }

    fn sparklines_of(&mut self, sheet: u32) -> Arc<SheetSparklines> {
        if self.sparklines.raw.is_empty() {
            return Arc::default();
        }
        let key = self.revision();
        if self.sparklines.state.key != key {
            self.sparklines.state = State {
                key,
                ..State::default()
            };
        }
        if let Some(s) = self.sparklines.state.sheets.get(&sheet) {
            return s.clone();
        }
        let made = Arc::new(self.make_sparklines(sheet));
        self.sparklines.state.sheets.insert(sheet, made.clone());
        made
    }

    /// The sparklines of `sheet` moved to the current state, with their
    /// values read now.
    fn make_sparklines(&self, sheet: u32) -> SheetSparklines {
        let sheet_id = self.sheet_id(sheet);
        let raws: Vec<&Raw> = self
            .sparklines
            .raw
            .iter()
            .filter(|r| r.sheet_id == sheet_id)
            .collect();
        let mut out = HashMap::new();
        if raws.is_empty() {
            return out;
        }
        let moves = self.sparklines.base.moves_to(&self.history.layout());
        let model = self.model.get_model();
        let names: HashMap<String, u32> = model
            .workbook
            .worksheets
            .iter()
            .map(|w| (w.get_name().to_lowercase(), w.sheet_id))
            .collect();
        let index_of = |id: u32| {
            model
                .workbook
                .worksheets
                .iter()
                .position(|w| w.sheet_id == id)
                .map(|i| i as u32)
        };
        // A reference (with its sheet, or on this one) as a sheet index and
        // a range, after the moves.
        let source = |f: &str| -> Option<Source> {
            let f = moves.formula(f, Some(sheet_id), &names);
            let token = references(&f).into_iter().next()?;
            let id = match &token.sheet {
                Some(name) => *names.get(&name.to_lowercase())?,
                None => sheet_id,
            };
            let a = token.range.area();
            Some(Source {
                sheet: index_of(id)?,
                range: CellRange {
                    top: a.top,
                    left: a.left,
                    bottom: a.bottom,
                    right: a.right,
                },
            })
        };
        let theme = &model.workbook.theme;
        let rgb = |c: &Option<Color>| -> Option<Rgb> {
            match c.as_ref()? {
                Color::None => None,
                c => parse_rgb(&c.to_rgb(theme).to_ascii_uppercase()),
            }
        };
        for raw in raws {
            let look = &raw.look;
            let dates = raw
                .dates
                .as_deref()
                .and_then(&source)
                .map(|s| self.source_values(&s, true));
            // Each sparkline's values, then the group's extremes.
            let mut made: Vec<Made> = Vec::new();
            for (sqref, f) in &raw.items {
                let Some(at) = moves.sqref(sheet_id, sqref) else {
                    continue;
                };
                let Some(cell) = parse_sqref(&at).first().copied() else {
                    continue;
                };
                let Some(src) = source(f) else { continue };
                let mut values = self.source_values(&src, look.hidden);
                let mut positions = dates.as_ref().and_then(|d| date_positions(d, values.len()));
                if look.empty == EmptyCells::Zero {
                    for v in values.iter_mut().filter(|v| v.is_none()) {
                        *v = Some(0.0);
                    }
                }
                if look.right_to_left {
                    values.reverse();
                    if let Some(p) = positions.as_mut() {
                        p.reverse();
                        for x in p.iter_mut() {
                            *x = 1.0 - *x;
                        }
                    }
                }
                made.push(((cell.top, cell.left), values, positions));
            }
            let extreme = |pick: fn(f64, f64) -> f64, start: f64| {
                made.iter()
                    .flat_map(|(_, v, _)| v.iter().flatten())
                    .fold(start, |a, &b| pick(a, b))
            };
            let (group_min, group_max) = (extreme(f64::min, f64::MAX), extreme(f64::max, f64::MIN));
            for (cell, values, positions) in made {
                let own = values.iter().flatten();
                let own_min = own.clone().fold(f64::MAX, |a, &b| a.min(b));
                let own_max = own.fold(f64::MIN, |a, &b| a.max(b));
                let end = |e: AxisEnd, own: f64, group: f64| match e {
                    AxisEnd::Individual => own,
                    AxisEnd::Group => group,
                    AxisEnd::Custom(v) => v,
                };
                let (mut min, mut max) = (
                    end(look.min, own_min, group_min),
                    end(look.max, own_max, group_max),
                );
                if min > max {
                    // No values (or custom ends crossed).
                    (min, max) = (0.0, 0.0);
                }
                let c = &look.colors;
                let on = |b: bool, slot: usize| b.then(|| rgb(&c[slot])).flatten();
                out.insert(
                    cell,
                    Arc::new(Sparkline {
                        kind: look.kind,
                        values,
                        positions,
                        min,
                        max,
                        weight: look.weight,
                        span_gaps: look.empty == EmptyCells::Span,
                        series: rgb(&c[SERIES]).unwrap_or(0x376092),
                        negative: on(look.negative, NEGATIVE),
                        markers: on(look.markers && look.kind == SparkKind::Line, MARKERS),
                        first: on(look.first, FIRST),
                        last: on(look.last, LAST),
                        high: on(look.high, HIGH),
                        low: on(look.low, LOW),
                        axis: on(look.axis, AXIS),
                    }),
                );
            }
        }
        out
    }

    /// The numbers of a source range in reading order (`None` for a cell
    /// that isn't a number), leaving out hidden rows and columns unless
    /// `hidden` (Excel's "Show data in hidden rows and columns").
    fn source_values(&self, src: &Source, hidden: bool) -> Vec<Option<f64>> {
        let model = self.model.get_model();
        let r = src.range;
        // A whole column or row: up to the sheet's used size.
        let (rows, cols) = self.used_size(src.sheet);
        let bottom = r.bottom.min(rows.saturating_sub(1).max(r.top));
        let right = r.right.min(cols.saturating_sub(1).max(r.left));
        let mut out = Vec::new();
        'rows: for row in r.top..=bottom {
            if !hidden && self.is_row_hidden(src.sheet, row) {
                continue;
            }
            for col in r.left..=right {
                if !hidden && self.is_column_hidden(src.sheet, col) {
                    continue;
                }
                let v = cell_value(model, src.sheet, row as i32 + 1, col as i32 + 1);
                out.push(match v {
                    Value::Number(n) => Some(n),
                    _ => None,
                });
                if out.len() >= MAX_POINTS {
                    break 'rows;
                }
            }
        }
        out
    }
}

/// Where `n` values go along a date axis, from the group's dates (`None`
/// when they don't give one date per value, or all the same day).
fn date_positions(dates: &[Option<f64>], n: usize) -> Option<Vec<f64>> {
    if dates.len() < n || n == 0 {
        return None;
    }
    let d: Vec<f64> = dates[..n].iter().map(|d| d.unwrap_or(f64::NAN)).collect();
    if d.iter().any(|x| x.is_nan()) {
        return None;
    }
    let lo = d.iter().copied().fold(f64::MAX, f64::min);
    let hi = d.iter().copied().fold(f64::MIN, f64::max);
    (hi > lo).then(|| d.iter().map(|x| (x - lo) / (hi - lo)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(values: &[Option<f64>]) -> Sparkline {
        let shown = values.iter().flatten();
        Sparkline {
            kind: SparkKind::Line,
            values: values.to_vec(),
            positions: None,
            min: shown.clone().fold(f64::MAX, |a, &b| a.min(b)),
            max: shown.fold(f64::MIN, |a, &b| a.max(b)),
            weight: 0.75,
            span_gaps: false,
            series: 0x376092,
            negative: None,
            markers: None,
            first: None,
            last: None,
            high: Some(0xD00000),
            low: None,
            axis: None,
        }
    }

    #[test]
    fn line_points_span_the_cell_and_gaps_break_it() {
        let s = line(&[Some(1.0), Some(3.0), None, Some(2.0)]);
        let g = s.geometry(100.0, 20.0, 1.0);
        assert_eq!(g.lines.len(), 2);
        let (a, b) = (&g.lines[0], &g.lines[1]);
        // The highest value is at the top, the lowest at the bottom, inside
        // the margins.
        assert!(a[1][1] < a[0][1]);
        assert!(a[0][0] < a[1][0] && a[1][0] < b[0][0]);
        assert!(a[0][0] > 0.0 && b[0][0] < 100.0);
        // The high point gets its marker.
        assert_eq!(g.markers.len(), 1);
        assert_eq!(g.markers[0].1, 0xD00000);
        // Spanned gaps join up.
        let g = Sparkline {
            span_gaps: true,
            ..s
        }
        .geometry(100.0, 20.0, 1.0);
        assert_eq!(g.lines.len(), 1);
        assert_eq!(g.lines[0].len(), 3);
    }

    #[test]
    fn columns_rise_from_zero_and_win_loss_from_the_middle() {
        let s = Sparkline {
            kind: SparkKind::Column,
            min: -2.0,
            max: 4.0,
            negative: Some(0xFF0000),
            high: None,
            axis: Some(0),
            ..line(&[Some(4.0), Some(-2.0), Some(1.0)])
        };
        let g = s.geometry(90.0, 30.0, 1.0);
        assert_eq!(g.bars.len(), 3);
        let axis = g.axis.unwrap().0;
        let [_, y, _, h] = g.bars[0].0;
        assert!((y + h - axis).abs() < 1e-3);
        let [_, y, _, _] = g.bars[1].0;
        assert!((y - axis).abs() < 1e-3);
        assert_eq!(g.bars[1].1, 0xFF0000);
        assert_eq!(g.bars[0].1, s.series);

        let w = Sparkline {
            kind: SparkKind::WinLoss,
            ..s
        }
        .geometry(90.0, 30.0, 1.0);
        let mid = w.axis.unwrap().0;
        assert!((w.bars[0].0[1] + w.bars[0].0[3] - mid).abs() < 1e-3);
        assert!((w.bars[1].0[1] - mid).abs() < 1e-3);
        // Up and down bars are the same height.
        assert!((w.bars[0].0[3] - w.bars[1].0[3]).abs() < 1e-3);
    }

    #[test]
    fn date_axis_spaces_points_by_date() {
        let p = date_positions(&[Some(1.0), Some(2.0), Some(5.0)], 3).unwrap();
        assert_eq!(p, vec![0.0, 0.25, 1.0]);
        assert!(date_positions(&[Some(1.0), None], 2).is_none());
    }
}
