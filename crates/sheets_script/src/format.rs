//! `Range.Font`, `Range.Interior` and `Range.Borders`.

use std::rc::Rc;

use kantor_script::{Arg, Dispatch, ScriptError, Value};
use sheets_engine::{BorderEdges, BorderLine, CellRange, LineStyle};

use crate::consts::*;
use crate::range::set_style;
use crate::session::Session;
use crate::values::*;

/// The first cell's style.
fn first_style(
    s: &Rc<Session>,
    sheet: u32,
    areas: &[CellRange],
) -> Result<std::sync::Arc<sheets_engine::CellStyle>, ScriptError> {
    let idx = s.sheet_index(sheet)?;
    let a = areas[0];
    Ok(s.read()?.cell_style(idx, a.top, a.left))
}

pub struct FontObj {
    pub s: Rc<Session>,
    pub sheet: u32,
    pub areas: Vec<CellRange>,
}

impl Dispatch for FontObj {
    fn type_name(&self) -> String {
        "Font".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        let st = first_style(&self.s, self.sheet, &self.areas)?;
        Ok(match name.to_ascii_lowercase().as_str() {
            "bold" => Value::Boolean(st.bold),
            "italic" => Value::Boolean(st.italic),
            "strikethrough" => Value::Boolean(st.strikethrough),
            "underline" => Value::Long(if st.underline {
                XL_UNDERLINE_STYLE_SINGLE as i32
            } else {
                XL_UNDERLINE_STYLE_NONE as i32
            }),
            "size" => Value::Double(f64::from(st.font_size)),
            "name" => Value::String(match &st.font_name {
                Some(n) => n.to_string(),
                None => self.s.read()?.default_font_name(),
            }),
            "color" => Value::Long(rgb_to_bgr(st.font_color.unwrap_or(0))),
            "colorindex" => Value::Long(match st.font_color {
                None => XL_COLOR_INDEX_AUTOMATIC as i32,
                Some(c) => color_index(c),
            }),
            "fontstyle" => Value::String(
                match (st.bold, st.italic) {
                    (true, true) => "Bold Italic",
                    (true, false) => "Bold",
                    (false, true) => "Italic",
                    (false, false) => "Regular",
                }
                .into(),
            ),
            "superscript" | "subscript" | "shadow" | "outlinefont" => Value::Boolean(false),
            "themecolor" | "tintandshade" => Value::Long(0),
            _ => return Err(ScriptError::no_member("Font", name)),
        })
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        let set = |path: &str, v: &str| set_style(&self.s, self.sheet, &self.areas, path, v);
        let flag = |b: bool| if b { "true" } else { "false" };
        match name.to_ascii_lowercase().as_str() {
            "bold" => set("font.b", flag(to_bool(&value)?)),
            "italic" => set("font.i", flag(to_bool(&value)?)),
            "strikethrough" => set("font.strike", flag(to_bool(&value)?)),
            "underline" => {
                let on = match scalar(&value)? {
                    Value::Boolean(b) => b,
                    v => to_i64(&v)? != XL_UNDERLINE_STYLE_NONE,
                };
                set("font.u", flag(on))
            }
            "size" => {
                let size = to_f64(&value)?.round();
                if !(1.0..=409.0).contains(&size) {
                    return Err(err1004("Unable to set the Size property of the Font class"));
                }
                set("font.size", &(size as i64).to_string())
            }
            "name" => set("font.name", &to_str(&value)?),
            "color" => set(
                "font.color",
                &format!("#{:06X}", bgr_to_rgb(to_i64(&value)?)),
            ),
            "colorindex" => {
                let i = to_i64(&value)?;
                if i == XL_COLOR_INDEX_AUTOMATIC || i == XL_COLOR_INDEX_NONE {
                    return set("font.color", "");
                }
                let rgb = palette(i)?;
                set("font.color", &format!("#{rgb:06X}"))
            }
            "fontstyle" => {
                let v = to_str(&value)?.to_lowercase();
                set("font.b", flag(v.contains("bold")))?;
                set("font.i", flag(v.contains("italic")))
            }
            "superscript" | "subscript" | "shadow" | "outlinefont" | "themecolor"
            | "tintandshade" | "themefont" => Ok(()),
            _ => Err(ScriptError::no_member("Font", name)),
        }
    }
}

/// The VBA name of a fill pattern other than solid and none (`XlPattern`).
fn pattern_name(p: i64) -> Option<&'static str> {
    Some(match p {
        XL_GRAY50 => "xlGray50",
        XL_GRAY75 => "xlGray75",
        XL_GRAY25 => "xlGray25",
        XL_HORIZONTAL => "xlHorizontal",
        XL_VERTICAL => "xlVertical",
        XL_DOWN => "xlDown",
        XL_UP => "xlUp",
        XL_CHECKER => "xlChecker",
        XL_SEMI_GRAY75 => "xlSemiGray75",
        XL_LIGHT_HORIZONTAL => "xlLightHorizontal",
        XL_LIGHT_VERTICAL => "xlLightVertical",
        XL_LIGHT_DOWN => "xlLightDown",
        XL_LIGHT_UP => "xlLightUp",
        XL_GRID => "xlGrid",
        XL_CRISS_CROSS => "xlCrissCross",
        XL_GRAY16 => "xlGray16",
        XL_GRAY8 => "xlGray8",
        XL_PATTERN_LINEAR_GRADIENT => "xlPatternLinearGradient",
        XL_PATTERN_RECTANGULAR_GRADIENT => "xlPatternRectangularGradient",
        _ => return None,
    })
}

// `XlPattern` values (besides xlSolid 1, xlNone / xlPatternNone -4142,
// xlAutomatic -4105, and xlHorizontal / xlVertical / xlDown / xlUp, which
// are registered already).
pub const XL_PATTERN_NONE: i64 = -4142;
pub const XL_PATTERN_SOLID: i64 = 1;
pub const XL_PATTERN_AUTOMATIC: i64 = -4105;
pub const XL_GRAY50: i64 = -4125;
pub const XL_GRAY75: i64 = -4126;
pub const XL_GRAY25: i64 = -4124;
pub const XL_CHECKER: i64 = 9;
pub const XL_SEMI_GRAY75: i64 = 10;
pub const XL_LIGHT_HORIZONTAL: i64 = 11;
pub const XL_LIGHT_VERTICAL: i64 = 12;
pub const XL_LIGHT_DOWN: i64 = 13;
pub const XL_LIGHT_UP: i64 = 14;
pub const XL_GRID: i64 = 15;
pub const XL_CRISS_CROSS: i64 = 16;
pub const XL_GRAY16: i64 = 17;
pub const XL_GRAY8: i64 = 18;
pub const XL_PATTERN_LINEAR_GRADIENT: i64 = 4000;
pub const XL_PATTERN_RECTANGULAR_GRADIENT: i64 = 4001;

pub(crate) fn palette(i: i64) -> Result<u32, ScriptError> {
    usize::try_from(i - 1)
        .ok()
        .and_then(|k| PALETTE.get(k).copied())
        .ok_or_else(|| err1004("Unable to set the ColorIndex property"))
}

pub struct InteriorObj {
    pub s: Rc<Session>,
    pub sheet: u32,
    pub areas: Vec<CellRange>,
}

impl Dispatch for InteriorObj {
    fn type_name(&self) -> String {
        "Interior".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        let st = first_style(&self.s, self.sheet, &self.areas)?;
        Ok(match name.to_ascii_lowercase().as_str() {
            "color" => Value::Long(rgb_to_bgr(st.fill.unwrap_or(0xFF_FFFF))),
            "colorindex" => Value::Long(match st.fill {
                None => XL_COLOR_INDEX_NONE as i32,
                Some(c) => color_index(c),
            }),
            "pattern" => Value::Long(if st.fill.is_some() {
                XL_SOLID as i32
            } else {
                XL_NONE as i32
            }),
            // Only solid fills are kept: the pattern colour is automatic.
            "patterncolorindex" => Value::Long(XL_AUTOMATIC as i32),
            "patterncolor" => Value::Long(0),
            "themecolor" | "tintandshade" | "patterntintandshade" => Value::Long(0),
            _ => return Err(ScriptError::no_member("Interior", name)),
        })
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        let set = |v: &str| set_style(&self.s, self.sheet, &self.areas, "fill.fg_color", v);
        match name.to_ascii_lowercase().as_str() {
            "color" => set(&format!("#{:06X}", bgr_to_rgb(to_i64(&value)?))),
            "colorindex" => {
                let i = to_i64(&value)?;
                if i == XL_COLOR_INDEX_NONE || i == XL_COLOR_INDEX_AUTOMATIC {
                    return set("");
                }
                set(&format!("#{:06X}", palette(i)?))
            }
            "pattern" => match to_i64(&value)? {
                XL_NONE => set(""),
                XL_SOLID => Ok(()),
                p => match pattern_name(p) {
                    // The cell style keeps a solid fill's colour only.
                    Some(name) => Err(ScriptError::unsupported(&format!(
                        "A patterned cell fill ({name}; Kantor keeps solid fills)"
                    ))),
                    None => Err(err1004(
                        "Unable to set the Pattern property of the Interior class",
                    )),
                },
            },
            "patterncolorindex"
            | "patterncolor"
            | "themecolor"
            | "tintandshade"
            | "patterntintandshade" => Ok(()),
            _ => Err(ScriptError::no_member("Interior", name)),
        }
    }
}

/// `Borders` (all edges) or `Borders(xlEdgeBottom)` (one).
pub struct BordersObj {
    pub s: Rc<Session>,
    pub sheet: u32,
    pub areas: Vec<CellRange>,
    pub edge: Option<i64>,
}

fn edges(edge: Option<i64>) -> Result<BorderEdges, ScriptError> {
    Ok(match edge {
        None => BorderEdges::All,
        Some(XL_EDGE_LEFT) => BorderEdges::Left,
        Some(XL_EDGE_TOP) => BorderEdges::Top,
        Some(XL_EDGE_BOTTOM) => BorderEdges::Bottom,
        Some(XL_EDGE_RIGHT) => BorderEdges::Right,
        Some(XL_INSIDE_VERTICAL) => BorderEdges::InsideVertical,
        Some(XL_INSIDE_HORIZONTAL) => BorderEdges::InsideHorizontal,
        Some(_) => return Err(ScriptError::runtime(9)),
    })
}

/// `Some(up)` for `xlDiagonalUp` / `xlDiagonalDown`.
fn diagonal(edge: Option<i64>) -> Option<bool> {
    match edge {
        Some(XL_DIAGONAL_UP) => Some(true),
        Some(XL_DIAGONAL_DOWN) => Some(false),
        _ => None,
    }
}

pub(crate) fn line_style(line: i64, weight: i64) -> Option<LineStyle> {
    Some(match line {
        XL_NONE => return None,
        XL_DOUBLE => LineStyle::Double,
        XL_DOT => LineStyle::Dotted,
        XL_DASH if weight == XL_MEDIUM => LineStyle::MediumDashed,
        XL_DASH => LineStyle::Dashed,
        XL_DASH_DOT if weight == XL_MEDIUM => LineStyle::MediumDashDot,
        XL_DASH_DOT => LineStyle::DashDot,
        XL_DASH_DOT_DOT if weight == XL_MEDIUM => LineStyle::MediumDashDotDot,
        XL_DASH_DOT_DOT => LineStyle::DashDotDot,
        XL_SLANT_DASH_DOT => LineStyle::SlantDashDot,
        _ => match weight {
            XL_HAIRLINE => LineStyle::Hair,
            XL_MEDIUM => LineStyle::Medium,
            XL_THICK => LineStyle::Thick,
            _ => LineStyle::Thin,
        },
    })
}

/// `XlLineStyle` of a line (`xlNone` for none).
pub(crate) fn line_style_of(line: Option<LineStyle>) -> i64 {
    match line {
        None => XL_NONE,
        Some(LineStyle::Double) => XL_DOUBLE,
        Some(LineStyle::Dotted) => XL_DOT,
        Some(LineStyle::Dashed | LineStyle::MediumDashed) => XL_DASH,
        Some(LineStyle::DashDot | LineStyle::MediumDashDot) => XL_DASH_DOT,
        Some(LineStyle::DashDotDot | LineStyle::MediumDashDotDot) => XL_DASH_DOT_DOT,
        Some(LineStyle::SlantDashDot) => XL_SLANT_DASH_DOT,
        Some(_) => XL_CONTINUOUS,
    }
}

/// `XlBorderWeight` of a line.
pub(crate) fn weight_of(line: Option<LineStyle>) -> i64 {
    match line {
        Some(LineStyle::Hair) => XL_HAIRLINE,
        Some(
            LineStyle::Medium
            | LineStyle::MediumDashed
            | LineStyle::MediumDashDot
            | LineStyle::MediumDashDotDot,
        ) => XL_MEDIUM,
        Some(LineStyle::Thick) => XL_THICK,
        _ => XL_THIN,
    }
}

/// The areas to format: whole rows or columns as their used part.
fn clipped(book: &sheets_engine::Workbook, idx: u32, areas: &[CellRange]) -> Vec<CellRange> {
    let (rows, cols) = book.used_size(idx);
    areas
        .iter()
        .filter_map(|&a| {
            if a.is_whole_columns() || a.is_whole_rows() {
                a.clip(rows.max(1), cols.max(1))
            } else {
                Some(a)
            }
        })
        .collect()
}

/// Draw (or with `xlNone`, remove) borders.
pub fn draw_border(
    s: &Rc<Session>,
    sheet: u32,
    areas: &[CellRange],
    which: BorderEdges,
    line: i64,
    weight: i64,
    color: Option<u32>,
) -> Result<(), ScriptError> {
    let idx = s.sheet_index(sheet)?;
    let mut book = s.write()?;
    for a in clipped(&book, idx, areas) {
        match line_style(line, weight) {
            Some(style) => book.set_range_border(idx, a, which, style, color),
            // Only these edges go (and the facing side of the cells next
            // to an outer edge).
            None => book.remove_range_edges(idx, a, which),
        }
        .map_err(engine)?;
    }
    Ok(())
}

/// Draw (`Some`) or remove a diagonal line on every cell of `areas`.
fn draw_diagonal(
    s: &Rc<Session>,
    sheet: u32,
    areas: &[CellRange],
    up: bool,
    line: Option<BorderLine>,
) -> Result<(), ScriptError> {
    let idx = s.sheet_index(sheet)?;
    let mut book = s.write()?;
    for a in clipped(&book, idx, areas) {
        book.set_range_diagonal(idx, a, up, line).map_err(engine)?;
    }
    Ok(())
}

impl BordersObj {
    /// The line of this edge on the first cell (`None`: no line).
    fn line(&self) -> Result<Option<BorderLine>, ScriptError> {
        let idx = self.s.sheet_index(self.sheet)?;
        let a = self.areas[0];
        let book = self.s.read()?;
        if let Some(up) = diagonal(self.edge) {
            let (u, d, line) = book.cell_diagonal(idx, a.top, a.left);
            return Ok(if (up && u) || (!up && d) { line } else { None });
        }
        let st = book.cell_style(idx, a.top, a.left);
        let b = &st.borders;
        Ok(match self.edge {
            Some(XL_EDGE_LEFT) => b.left,
            Some(XL_EDGE_RIGHT) => b.right,
            Some(XL_EDGE_TOP) => b.top,
            Some(XL_EDGE_BOTTOM) => b.bottom,
            Some(XL_INSIDE_VERTICAL) => b.right.filter(|_| a.right > a.left),
            Some(XL_INSIDE_HORIZONTAL) => b.bottom.filter(|_| a.bottom > a.top),
            _ => b.left.or(b.top).or(b.right).or(b.bottom),
        })
    }

    /// Draw this edge as `line` (`None`: remove it).
    fn draw(&self, line: Option<BorderLine>) -> Result<(), ScriptError> {
        if let Some(up) = diagonal(self.edge) {
            return draw_diagonal(&self.s, self.sheet, &self.areas, up, line);
        }
        let which = edges(self.edge)?;
        let idx = self.s.sheet_index(self.sheet)?;
        let mut book = self.s.write()?;
        for a in clipped(&book, idx, &self.areas) {
            match line {
                Some(l) => book.set_range_border(idx, a, which, l.style, l.color),
                None => book.remove_range_edges(idx, a, which),
            }
            .map_err(engine)?;
        }
        Ok(())
    }
}

impl Dispatch for BordersObj {
    fn type_name(&self) -> String {
        if self.edge.is_some() {
            "Border".into()
        } else {
            "Borders".into()
        }
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        if (lower.is_empty() || lower == "item") && self.edge.is_none() {
            let edge = to_i64(need(args, 0, "Index")?)?;
            if diagonal(Some(edge)).is_none() {
                edges(Some(edge))?;
            }
            return Ok(obj(Rc::new(BordersObj {
                s: self.s.clone(),
                sheet: self.sheet,
                areas: self.areas.clone(),
                edge: Some(edge),
            })));
        }
        if lower == "count" {
            return Ok(Value::Long(8));
        }
        let line = self.line()?;
        Ok(match lower.as_str() {
            "linestyle" => Value::Long(line_style_of(line.map(|l| l.style)) as i32),
            "weight" => Value::Long(weight_of(line.map(|l| l.style)) as i32),
            "color" => Value::Long(rgb_to_bgr(line.and_then(|l| l.color).unwrap_or(0))),
            "colorindex" => Value::Long(match line.and_then(|l| l.color) {
                None => XL_COLOR_INDEX_AUTOMATIC as i32,
                Some(c) => color_index(c),
            }),
            "themecolor" | "tintandshade" => Value::Long(0),
            _ => return Err(ScriptError::no_member("Borders", name)),
        })
    }

    fn put(&self, name: &str, args: &[Arg], value: Value) -> Result<(), ScriptError> {
        let lower = name.to_ascii_lowercase();
        if (lower.is_empty() || lower == "item") && !args.is_empty() {
            let edge = self.get(name, args)?;
            return match edge {
                Value::Object(Some(o)) => o.put("LineStyle", &[], value),
                _ => Err(ScriptError::runtime(424)),
            };
        }
        // A colour or weight keeps the edge's other setting (a single
        // edge; `Borders` as a whole starts from thin automatic lines).
        let current = if self.edge.is_some() {
            self.line()?
        } else {
            None
        };
        let style = current.map_or(LineStyle::Thin, |l| l.style);
        let color = current.and_then(|l| l.color);
        match lower.as_str() {
            "linestyle" => {
                let line = to_i64(&value)?;
                self.draw(
                    line_style(line, weight_of(current.map(|l| l.style)))
                        .map(|style| BorderLine { style, color }),
                )
            }
            "weight" => {
                let weight = to_i64(&value)?;
                let keep = line_style_of(current.map(|l| l.style));
                let keep = if keep == XL_NONE { XL_CONTINUOUS } else { keep };
                let style = line_style(keep, weight).unwrap_or(LineStyle::Thin);
                self.draw(Some(BorderLine { style, color }))
            }
            "color" => self.draw(Some(BorderLine {
                style,
                color: Some(bgr_to_rgb(to_i64(&value)?)),
            })),
            "colorindex" => {
                let i = to_i64(&value)?;
                let color = if i == XL_COLOR_INDEX_AUTOMATIC || i == XL_COLOR_INDEX_NONE {
                    None
                } else {
                    Some(palette(i)?)
                };
                self.draw(Some(BorderLine { style, color }))
            }
            "themecolor" | "tintandshade" => Ok(()),
            _ => Err(ScriptError::no_member("Borders", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        Ok([
            XL_EDGE_LEFT,
            XL_EDGE_TOP,
            XL_EDGE_BOTTOM,
            XL_EDGE_RIGHT,
            XL_INSIDE_VERTICAL,
            XL_INSIDE_HORIZONTAL,
        ]
        .into_iter()
        .map(|e| {
            obj(Rc::new(BordersObj {
                s: self.s.clone(),
                sheet: self.sheet,
                areas: self.areas.clone(),
                edge: Some(e),
            }))
        })
        .collect())
    }
}
