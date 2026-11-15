//! KANTOR PATCH (functions): FORECAST.ETS, FORECAST.ETS.CONFINT,
//! FORECAST.ETS.SEASONALITY and FORECAST.ETS.STAT.
//!
//! Excel's additive exponential triple smoothing (ETS AAA), step for step:
//! the timeline is read and put on a regular grid (`series`), the season
//! length detected (`detect`) and the model fitted (`fit`). Microsoft
//! documents none of the internals; this is a port of the reverse-engineered
//! implementation in formulon (<https://github.com/libraz/formulon>,
//! `src/eval/{forecast_ets_lazy,holt_winters,ets_series}.cpp`, Apache-2.0,
//! Copyright libraz), checked there against values recorded from Excel 365
//! for Mac and Windows. Floating-point steps follow its order of operations
//! (and its flushing of subnormal results), as results are compared to the
//! last bit.

mod detect;
mod fit;
mod num;
mod quantile;
mod series;

use crate::{
    calc_result::CalcResult,
    expressions::{
        parser::{ArrayNode, Node},
        token::Error,
        types::CellReferenceIndex,
    },
    model::Model,
};

use self::{
    fit::Fit,
    num::whole_count,
    series::{Aggregation, Cell, Series, Timeline},
};

pub(crate) use self::detect::detect_seasonality;

/// Largest seasonality argument; above it is #NUM!.
const MAX_SEASONALITY_ARG: i64 = 8784;
/// A seasonality argument at or above this percentage of the series length
/// means no seasonality.
const SEASONALITY_LENGTH_PERCENT: usize = 70;
/// Largest forecast horizon in steps past the last node.
const MAX_HORIZON_STEPS: f64 = 2147483647.0;

struct Options {
    seasonality: i64,
    completion: i64,
    aggregation: Aggregation,
}

/// A fitted model and the grid that places target dates on it.
struct Fitted {
    fit: Fit,
    timeline: Timeline,
    n: usize,
}

fn cell_of(node: ArrayNode) -> Cell {
    match node {
        ArrayNode::Number(f) => Cell::Number(f),
        ArrayNode::Empty => Cell::Blank,
        ArrayNode::String(_) => Cell::Text,
        ArrayNode::Boolean(_) => Cell::Bool,
        ArrayNode::Error(e) => Cell::Error(e),
    }
}

/// Season length the fit runs with: detected for 1, none for 0 and when the
/// argument covers most of the series.
fn resolve_seasonality(y: &[f64], seasonality: i64) -> usize {
    if seasonality == 1 {
        return detect_seasonality(y);
    }
    if seasonality == 0 || seasonality as usize >= (SEASONALITY_LENGTH_PERCENT * y.len()) / 100 {
        return 1;
    }
    seasonality as usize
}

/// Linear interpolation of the value at `pos` between steps `s` and `s + 1`.
fn interpolate(s: i64, at_s: f64, at_next: f64, pos: f64) -> f64 {
    let span = (s + 1) as f64 - s as f64;
    let diff = at_next - at_s;
    let mut v = pos - s as f64;
    v *= diff;
    v /= span;
    v + at_s
}

/// Where a target date falls: the fractional step count, its whole part and
/// whether it lies between two steps.
fn locate(f: &Fitted, target: f64) -> Result<(f64, i64, bool), Error> {
    let (pos, interpolated) = series::target_position(&f.timeline, target)?;
    match whole_count(pos) {
        Some(s) if pos <= MAX_HORIZON_STEPS => Ok((pos, s, interpolated)),
        _ => Err(Error::NUM),
    }
}

fn finite(v: f64) -> Result<f64, Error> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(Error::NUM)
    }
}

fn forecast_at(f: &Fitted, target: f64) -> Result<f64, Error> {
    let (pos, s, interpolated) = locate(f, target)?;
    // A target at or before the first grid node has no forecast.
    if s <= -(f.n as i64) {
        return Err(Error::NUM);
    }
    let mut v = f.fit.forecast(s);
    if interpolated {
        v = interpolate(s, v, f.fit.forecast(s + 1), pos);
    }
    finite(f.fit.zero_if_negligible(v))
}

fn confint_at(f: &Fitted, target: f64, confidence: f64) -> Result<f64, Error> {
    let (pos, s, interpolated) = locate(f, target)?;
    // The interval is undefined at and before the last observation.
    if s < 0 || (s == 0 && !interpolated) {
        return Err(Error::NUM);
    }
    let variance = if s != 0 {
        let v = f.fit.variance(s);
        if interpolated {
            interpolate(s, v, f.fit.variance(s + 1), pos)
        } else {
            v
        }
    } else {
        interpolate(0, 0.0, f.fit.variance(1), pos)
    };
    let mut p = 1.0 - confidence;
    p /= 2.0;
    p = -p;
    p += 1.0;
    let z = quantile::normal_quantile(p);
    finite(z * if variance > 0.0 { variance.sqrt() } else { 0.0 })
}

impl Model<'_> {
    /// The cells of a range or array argument, row by row, and how many there
    /// are (a range is read only down to its sheet's used area; the rest are
    /// blanks).
    fn ets_cells(
        &mut self,
        node: &Node,
        cell: CellReferenceIndex,
    ) -> Result<Vec<Cell>, CalcResult> {
        let (grid, height) = self.read_grid(node, cell)?;
        let width = grid.first().map_or(0, Vec::len);
        let mut cells: Vec<Cell> = grid.into_iter().flatten().map(cell_of).collect();
        cells.resize(height * width, Cell::Blank);
        Ok(cells)
    }

    /// An optional number: missing or blank is `default`, an error stays one.
    fn ets_number(
        &mut self,
        args: &[Node],
        i: usize,
        default: f64,
        cell: CellReferenceIndex,
    ) -> Result<f64, CalcResult> {
        let Some(node) = args.get(i) else {
            return Ok(default);
        };
        match self.evaluate_node_in_context(node, cell) {
            CalcResult::EmptyArg | CalcResult::EmptyCell => Ok(default),
            result => self.cast_to_number(result, cell),
        }
    }

    /// An optional whole-number option in [lo, hi]; anything else is #NUM!.
    fn ets_option(
        &mut self,
        args: &[Node],
        i: usize,
        default: i64,
        range: (i64, i64),
        cell: CellReferenceIndex,
    ) -> Result<i64, CalcResult> {
        let d = self.ets_number(args, i, default as f64, cell)?;
        match whole_count(d) {
            Some(v) if d == d.floor() && v >= range.0 && v <= range.1 => Ok(v),
            _ => Err(CalcResult::new_error(
                Error::NUM,
                cell,
                "Invalid option".to_string(),
            )),
        }
    }

    /// seasonality, data_completion and aggregation from argument `first`
    /// (`with_seasonality` false: data_completion first, seasonality detected).
    fn ets_options(
        &mut self,
        args: &[Node],
        first: usize,
        with_seasonality: bool,
        cell: CellReferenceIndex,
    ) -> Result<Options, CalcResult> {
        let (seasonality, next) = if with_seasonality {
            (
                self.ets_option(args, first, 1, (0, MAX_SEASONALITY_ARG), cell)?,
                first + 1,
            )
        } else {
            (1, first)
        };
        let completion = self.ets_option(args, next, 1, (0, 1), cell)?;
        let aggregation = self.ets_option(args, next + 1, 1, (1, 7), cell)?;
        Ok(Options {
            seasonality,
            completion,
            aggregation: Aggregation::from_option(aggregation).unwrap_or(Aggregation::Average),
        })
    }

    /// The regular series of `values` and `timeline` and its season length.
    fn ets_series(
        &mut self,
        values: &Node,
        timeline: &Node,
        options: &Options,
        cell: CellReferenceIndex,
    ) -> Result<(Series, usize), CalcResult> {
        let times = self.ets_cells(timeline, cell)?;
        let values = self.ets_cells(values, cell)?;
        let series =
            series::prepare_series(&values, &times, options.completion, options.aggregation)
                .map_err(|e| CalcResult::new_error(e, cell, "Invalid timeline".to_string()))?;
        let m = resolve_seasonality(&series.y, options.seasonality);
        Ok((series, m))
    }

    fn ets_fitted(
        &mut self,
        args: &[Node],
        first_option: usize,
        cell: CellReferenceIndex,
    ) -> Result<Fitted, CalcResult> {
        let options = self.ets_options(args, first_option, true, cell)?;
        let (series, m) = self.ets_series(&args[1], &args[2], &options, cell)?;
        Ok(Fitted {
            fit: fit::fit(&series.y, m),
            n: series.y.len(),
            timeline: series.timeline,
        })
    }

    /// Applies `at` to target_date: once for a single value, per element of
    /// an array keeping its shape. A target error wins over `head`, the
    /// failure of the rest of the call.
    fn ets_each_target(
        &mut self,
        target: CalcResult,
        head: Result<Fitted, CalcResult>,
        at: &dyn Fn(&Fitted, f64) -> Result<f64, Error>,
        cell: CellReferenceIndex,
    ) -> CalcResult {
        let one = |model: &mut Model, value: CalcResult| -> ArrayNode {
            let t = match model.cast_to_number(value, cell) {
                Ok(t) if t.is_finite() => t,
                Ok(_) => return ArrayNode::Error(Error::NUM),
                Err(CalcResult::Error { error, .. }) => return ArrayNode::Error(error),
                Err(_) => return ArrayNode::Error(Error::VALUE),
            };
            match &head {
                Ok(f) => match at(f, t) {
                    Ok(v) => ArrayNode::Number(v),
                    Err(e) => ArrayNode::Error(e),
                },
                Err(CalcResult::Error { error, .. }) => ArrayNode::Error(error.clone()),
                Err(_) => ArrayNode::Error(Error::VALUE),
            }
        };
        let grid = match target {
            CalcResult::Range { left, right } if left != right => self.evaluate_range(left, right),
            CalcResult::Array(a) => a,
            single => {
                return match one(self, single) {
                    ArrayNode::Number(v) => CalcResult::Number(v),
                    ArrayNode::Error(e) => CalcResult::new_error(e, cell, String::new()),
                    _ => CalcResult::new_error(Error::VALUE, cell, String::new()),
                }
            }
        };
        let out: Vec<Vec<ArrayNode>> = grid
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|node| {
                        let value = match node {
                            ArrayNode::Number(f) => CalcResult::Number(f),
                            ArrayNode::String(s) => CalcResult::String(s),
                            ArrayNode::Boolean(b) => CalcResult::Boolean(b),
                            ArrayNode::Error(e) => CalcResult::new_error(e, cell, String::new()),
                            ArrayNode::Empty => CalcResult::EmptyCell,
                        };
                        one(self, value)
                    })
                    .collect()
            })
            .collect();
        if out.len() == 1 && out[0].len() == 1 {
            return match &out[0][0] {
                ArrayNode::Number(v) => CalcResult::Number(*v),
                ArrayNode::Error(e) => CalcResult::new_error(e.clone(), cell, String::new()),
                _ => CalcResult::new_error(Error::VALUE, cell, String::new()),
            };
        }
        CalcResult::Array(out)
    }

    /// The target_date argument: an error is returned at once unless it is
    /// an array (then each element is checked).
    fn ets_target(
        &mut self,
        node: &Node,
        cell: CellReferenceIndex,
    ) -> Result<CalcResult, CalcResult> {
        let target = self.evaluate_node_in_context(node, cell);
        match &target {
            CalcResult::Array(_) => Ok(target),
            CalcResult::Range { left, right } if left != right => Ok(target),
            _ => {
                let t = self.cast_to_number(target, cell)?;
                if !t.is_finite() {
                    return Err(CalcResult::new_error(Error::NUM, cell, String::new()));
                }
                Ok(CalcResult::Number(t))
            }
        }
    }

    /// `FORECAST.ETS(target_date, values, timeline, [seasonality],
    /// [data_completion], [aggregation])`
    pub(crate) fn fn_forecast_ets(
        &mut self,
        args: &[Node],
        cell: CellReferenceIndex,
    ) -> CalcResult {
        if args.len() < 3 || args.len() > 6 {
            return CalcResult::new_args_number_error(cell);
        }
        let target = match self.ets_target(&args[0], cell) {
            Ok(t) => t,
            Err(e) => return e,
        };
        let head = self.ets_fitted(args, 3, cell);
        self.ets_each_target(target, head, &forecast_at, cell)
    }

    /// `FORECAST.ETS.CONFINT(target_date, values, timeline,
    /// [confidence_level], [seasonality], [data_completion], [aggregation])`
    pub(crate) fn fn_forecast_ets_confint(
        &mut self,
        args: &[Node],
        cell: CellReferenceIndex,
    ) -> CalcResult {
        if args.len() < 3 || args.len() > 7 {
            return CalcResult::new_args_number_error(cell);
        }
        let target = match self.ets_target(&args[0], cell) {
            Ok(t) => t,
            Err(e) => return e,
        };
        let confidence = self.ets_number(args, 3, 0.95, cell).and_then(|c| {
            if c.is_finite() && (0.0..1.0).contains(&c) {
                Ok(c)
            } else {
                Err(CalcResult::new_error(
                    Error::NUM,
                    cell,
                    "The confidence level must be from 0 to less than 1".to_string(),
                ))
            }
        });
        let (head, confidence) = match confidence {
            Ok(c) => (self.ets_fitted(args, 4, cell), c),
            Err(e) => (Err(e), 0.0),
        };
        let at = move |f: &Fitted, t: f64| confint_at(f, t, confidence);
        self.ets_each_target(target, head, &at, cell)
    }

    /// `FORECAST.ETS.SEASONALITY(values, timeline, [data_completion],
    /// [aggregation])`: 0 when no season is detected.
    pub(crate) fn fn_forecast_ets_seasonality(
        &mut self,
        args: &[Node],
        cell: CellReferenceIndex,
    ) -> CalcResult {
        if args.len() < 2 || args.len() > 4 {
            return CalcResult::new_args_number_error(cell);
        }
        let result = self
            .ets_options(args, 2, false, cell)
            .and_then(|options| self.ets_series(&args[0], &args[1], &options, cell));
        match result {
            Ok((_, m)) => CalcResult::Number(if m == 1 { 0.0 } else { m as f64 }),
            Err(e) => e,
        }
    }

    /// `FORECAST.ETS.STAT(values, timeline, statistic_type, [seasonality],
    /// [data_completion], [aggregation])`
    pub(crate) fn fn_forecast_ets_stat(
        &mut self,
        args: &[Node],
        cell: CellReferenceIndex,
    ) -> CalcResult {
        if args.len() < 3 || args.len() > 6 {
            return CalcResult::new_args_number_error(cell);
        }
        let num_error = || CalcResult::new_error(Error::NUM, cell, "Invalid statistic".to_string());
        let stat = match self.get_number(&args[2], cell) {
            Ok(f) if f.is_finite() => match whole_count(f) {
                Some(s) if (1..=8).contains(&s) => s,
                _ => return num_error(),
            },
            Ok(_) => return num_error(),
            Err(e) => return e,
        };
        let (series, m) = match self
            .ets_options(args, 3, true, cell)
            .and_then(|options| self.ets_series(&args[0], &args[1], &options, cell))
        {
            Ok(r) => r,
            Err(e) => return e,
        };
        if stat == 8 {
            return CalcResult::Number(series.timeline.step);
        }
        let fit = fit::fit(&series.y, m);
        let v = match stat {
            1 => fit.alpha,
            2 => fit.beta,
            3 => fit.gamma,
            4 => fit.mase,
            5 => fit.smape,
            6 => fit.mae,
            _ => fit.rmse,
        };
        match finite(v) {
            Ok(v) => CalcResult::Number(v),
            Err(e) => CalcResult::new_error(e, cell, String::new()),
        }
    }
}
