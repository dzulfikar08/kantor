//! Formatting edits macros make that the app's menus don't (see
//! `crates/sheets_script`): one border edge removed (`Borders(xlEdgeLeft)
//! .LineStyle = xlNone`, the neighbouring cell's facing side too), diagonal
//! borders, a colour scale's points by type and value
//! (`ColorScaleCriteria(i).Type`), and a conditional format's borders read
//! and built without IronCalc's types.
//!
//! Each method is one undo step.

use anyhow::{Result, anyhow, bail};
use ironcalc_base::cf_types::{CfRule, CfRuleInput, Cfvo, ColorScaleThreshold};
use ironcalc_base::types::{Border, BorderItem, Color, Style};

use crate::format::{BorderLine, LineStyle, Rgb, check_style_range, parse_rgb, resolve};
use crate::{BorderEdges, CellRange, CfValue, CfValueKind, MAX_COLS, MAX_ROWS, Workbook};

/// A border side in IronCalc's form (automatic colour: black, as
/// [`Workbook::set_range_border`] draws it).
fn border_item(line: BorderLine) -> Result<BorderItem> {
    let color = line.color.map_or_else(
        || "#000000".to_owned(),
        |c| format!("#{:06X}", c & 0xFF_FFFF),
    );
    Ok(serde_json::from_value(serde_json::json!({
        "style": line.style.xml().to_ascii_lowercase(),
        "color": color,
    }))?)
}

fn number(v: &str, what: &str) -> Result<f64> {
    v.trim()
        .parse::<f64>()
        .map_err(|_| anyhow!("{what}: type a number"))
}

impl Workbook {
    /// Replace the styles of rectangle `top..=bottom`, `left..=right`
    /// (0-based) by each cell's own style (row and column styles included)
    /// changed by `change(row, col, style)`.
    fn restyle_block(
        &mut self,
        sheet: u32,
        (top, left, bottom, right): (u32, u32, u32, u32),
        change: &dyn Fn(u32, u32, &mut Style),
    ) -> Result<()> {
        let block = CellRange {
            top,
            left,
            bottom,
            right,
        };
        check_style_range(block)?;
        let e = |e: String| anyhow!("{e}");
        let mut styles = Vec::with_capacity(block.rows_count() as usize);
        {
            let model = self.model.get_model();
            for r in top..=bottom {
                let mut row = Vec::with_capacity(block.cols_count() as usize);
                for c in left..=right {
                    let mut st = model
                        .get_style_for_cell(sheet, r as i32 + 1, c as i32 + 1)
                        .map_err(e)?;
                    change(r, c, &mut st);
                    row.push(st);
                }
                styles.push(row);
            }
        }
        self.model.set_selected_sheet(sheet).map_err(e)?;
        self.model
            .set_selected_cell(top as i32 + 1, left as i32 + 1)
            .map_err(e)?;
        self.model
            .set_selected_range(
                top as i32 + 1,
                left as i32 + 1,
                bottom as i32 + 1,
                right as i32 + 1,
            )
            .map_err(e)?;
        self.model.on_paste_styles(&styles).map_err(e)?;
        self.history.edited();
        Ok(())
    }

    /// Remove the borders on `edges` of `range` only (`Left`, `Right`,
    /// `Top`, `Bottom`, `Outline`, `InsideVertical`, `InsideHorizontal`,
    /// `Inside`; `All` / `None`: every border, as
    /// [`Workbook::set_range_border`] with `BorderEdges::None`). An outer
    /// edge goes from both cells sharing it. One undo step.
    pub fn remove_range_edges(
        &mut self,
        sheet: u32,
        range: CellRange,
        edges: BorderEdges,
    ) -> Result<()> {
        check_style_range(range)?;
        let CellRange {
            top,
            left,
            bottom,
            right,
        } = range;
        let (left_edge, right_edge, top_edge, bottom_edge, inside_v, inside_h) = match edges {
            BorderEdges::All | BorderEdges::None => {
                return self.set_range_border(
                    sheet,
                    range,
                    BorderEdges::None,
                    LineStyle::Thin,
                    None,
                );
            }
            BorderEdges::Left => (true, false, false, false, false, false),
            BorderEdges::Right => (false, true, false, false, false, false),
            BorderEdges::Top => (false, false, true, false, false, false),
            BorderEdges::Bottom => (false, false, false, true, false, false),
            BorderEdges::Outline => (true, true, true, true, false, false),
            BorderEdges::InsideVertical => (false, false, false, false, true, false),
            BorderEdges::InsideHorizontal => (false, false, false, false, false, true),
            BorderEdges::Inside => (false, false, false, false, true, true),
        };
        self.batch(|wb| {
            if left_edge {
                let from = left.saturating_sub(1);
                wb.restyle_block(sheet, (top, from, bottom, left), &|_, c, st| {
                    if c == left {
                        st.border.left = None;
                    } else {
                        st.border.right = None;
                    }
                })?;
            }
            if right_edge {
                let to = (right + 1).min(MAX_COLS - 1);
                wb.restyle_block(sheet, (top, right, bottom, to), &|_, c, st| {
                    if c == right {
                        st.border.right = None;
                    } else {
                        st.border.left = None;
                    }
                })?;
            }
            if top_edge {
                let from = top.saturating_sub(1);
                wb.restyle_block(sheet, (from, left, top, right), &|r, _, st| {
                    if r == top {
                        st.border.top = None;
                    } else {
                        st.border.bottom = None;
                    }
                })?;
            }
            if bottom_edge {
                let to = (bottom + 1).min(MAX_ROWS - 1);
                wb.restyle_block(sheet, (bottom, left, to, right), &|r, _, st| {
                    if r == bottom {
                        st.border.bottom = None;
                    } else {
                        st.border.top = None;
                    }
                })?;
            }
            let wide = right > left;
            let tall = bottom > top;
            if (inside_v && wide) || (inside_h && tall) {
                wb.restyle_block(sheet, (top, left, bottom, right), &|r, c, st| {
                    if inside_v && wide {
                        if c != left {
                            st.border.left = None;
                        }
                        if c != right {
                            st.border.right = None;
                        }
                    }
                    if inside_h && tall {
                        if r != top {
                            st.border.top = None;
                        }
                        if r != bottom {
                            st.border.bottom = None;
                        }
                    }
                })?;
            }
            Ok(())
        })
    }

    /// Draw (`Some`) or remove (`None`) the diagonal line from bottom left
    /// to top right (`up`) or top left to bottom right of every cell of
    /// `range`. A cell has one diagonal line style for both directions, as
    /// in the file format. One undo step.
    pub fn set_range_diagonal(
        &mut self,
        sheet: u32,
        range: CellRange,
        up: bool,
        line: Option<BorderLine>,
    ) -> Result<()> {
        check_style_range(range)?;
        let item = line.map(border_item).transpose()?;
        self.restyle_block(
            sheet,
            (range.top, range.left, range.bottom, range.right),
            &|_, _, st| {
                let b = &mut st.border;
                if up {
                    b.diagonal_up = item.is_some();
                } else {
                    b.diagonal_down = item.is_some();
                }
                match &item {
                    Some(i) => b.diagonal = Some(i.clone()),
                    None if !b.diagonal_up && !b.diagonal_down => b.diagonal = None,
                    None => {}
                }
            },
        )
    }

    /// A cell's diagonal lines: `(up, down, line)`.
    pub fn cell_diagonal(
        &self,
        sheet: u32,
        row: u32,
        col: u32,
    ) -> (bool, bool, Option<BorderLine>) {
        let model = self.model.get_model();
        let Ok(st) = model.get_style_for_cell(sheet, row as i32 + 1, col as i32 + 1) else {
            return (false, false, None);
        };
        let b = &st.border;
        let line = b.diagonal.as_ref().map(|d| BorderLine {
            style: LineStyle::from_ironcalc(&d.style),
            color: resolve(&d.color, &model.workbook.theme),
        });
        (b.diagonal_up, b.diagonal_down, line)
    }

    /// The points of colour scale rule `index` (as stored) of `sheet`,
    /// lowest first: how each is measured and its colour. `None` when the
    /// rule isn't a colour scale.
    pub fn cf_color_scale(&self, sheet: u32, index: usize) -> Option<Vec<(CfValue, Rgb)>> {
        let list = self.model.get_conditional_formatting_list(sheet).ok()?;
        let view = list.into_iter().find(|v| v.index == index)?;
        let CfRule::ColorScale { thresholds } = view.cf_rule else {
            return None;
        };
        let theme = &self.model.get_model().workbook.theme;
        Some(
            thresholds
                .iter()
                .map(|t| {
                    let n = |v: f64| {
                        if v.fract() == 0.0 && v.abs() < 1e15 {
                            format!("{}", v as i64)
                        } else {
                            format!("{v}")
                        }
                    };
                    let value = match &t.cfvo {
                        Cfvo::Min => CfValue::new(CfValueKind::Lowest, ""),
                        Cfvo::Max => CfValue::new(CfValueKind::Highest, ""),
                        Cfvo::Number(v) => CfValue::new(CfValueKind::Number, n(*v)),
                        Cfvo::Percent(v) => CfValue::new(CfValueKind::Percent, n(*v)),
                        Cfvo::Percentile(v) => CfValue::new(CfValueKind::Percentile, n(*v)),
                        Cfvo::Formula(f) => CfValue::new(
                            CfValueKind::Formula,
                            format!("={}", f.trim_start_matches('=')),
                        ),
                    };
                    let rgb = parse_rgb(&t.color.to_rgb(theme)).unwrap_or(0);
                    (value, rgb)
                })
                .collect(),
        )
    }

    /// Set the points of colour scale rule `index` (two or three; its range
    /// and priority stay). `Automatic` counts as the lowest value for the
    /// first point and the highest for the others. One undo step.
    pub fn set_cf_color_scale(
        &mut self,
        sheet: u32,
        index: usize,
        points: &[(CfValue, Rgb)],
    ) -> Result<()> {
        if !(2..=3).contains(&points.len()) {
            bail!("A colour scale has two or three colours");
        }
        let list = self
            .model
            .get_conditional_formatting_list(sheet)
            .map_err(|e| anyhow!("{e}"))?;
        let Some(view) = list.into_iter().find(|v| v.index == index) else {
            bail!("no rule {index}");
        };
        if !matches!(view.cf_rule, CfRule::ColorScale { .. }) {
            bail!("the rule isn't a colour scale");
        }
        let mut thresholds = Vec::with_capacity(points.len());
        for (k, (v, rgb)) in points.iter().enumerate() {
            let cfvo = match v.kind {
                CfValueKind::Automatic if k == 0 => Cfvo::Min,
                CfValueKind::Automatic => Cfvo::Max,
                CfValueKind::Lowest => Cfvo::Min,
                CfValueKind::Highest => Cfvo::Max,
                CfValueKind::Number => Cfvo::Number(number(&v.value, "Value")?),
                CfValueKind::Percent => Cfvo::Percent(number(&v.value, "Value")?),
                CfValueKind::Percentile => Cfvo::Percentile(number(&v.value, "Value")?),
                CfValueKind::Formula => {
                    let f = v.value.trim().trim_start_matches('=').trim();
                    if f.is_empty() {
                        bail!("Value: type a formula");
                    }
                    Cfvo::Formula(f.to_owned())
                }
            };
            thresholds.push(ColorScaleThreshold {
                cfvo,
                color: Color::Rgb(format!("#{:06X}", rgb & 0xFF_FFFF)),
            });
        }
        let range = view.range;
        self.batch(|wb| {
            wb.model
                .update_conditional_formatting(
                    sheet,
                    index as u32,
                    &range,
                    CfRuleInput::ColorScale { thresholds },
                )
                .map_err(|e| anyhow!("{e}"))?;
            wb.history.edited();
            wb.mark_cf(sheet);
            Ok(())
        })
    }

    /// The sides of a conditional format's border: left, right, top,
    /// bottom.
    pub fn cf_border_sides(&self, border: &Border) -> [Option<BorderLine>; 4] {
        let theme = &self.model.get_model().workbook.theme;
        let line = |b: &Option<BorderItem>| {
            b.as_ref().map(|b| BorderLine {
                style: LineStyle::from_ironcalc(&b.style),
                color: resolve(&b.color, theme),
            })
        };
        [
            line(&border.left),
            line(&border.right),
            line(&border.top),
            line(&border.bottom),
        ]
    }

    /// A conditional format's border with sides left, right, top, bottom
    /// (`None`: no border when every side is `None`).
    pub fn cf_border_of(sides: &[Option<BorderLine>; 4]) -> Result<Option<Border>> {
        if sides.iter().all(Option::is_none) {
            return Ok(None);
        }
        let item = |l: &Option<BorderLine>| l.map(border_item).transpose();
        Ok(Some(Border {
            diagonal_up: false,
            diagonal_down: false,
            left: item(&sides[0])?,
            right: item(&sides[1])?,
            top: item(&sides[2])?,
            bottom: item(&sides[3])?,
            diagonal: None,
        }))
    }
}
