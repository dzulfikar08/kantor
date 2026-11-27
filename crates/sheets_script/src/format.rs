//! `Range.Font`, `Range.Interior` and `Range.Borders`.

use std::rc::Rc;

use kantor_script::{Arg, Dispatch, ScriptError, Value};
use sheets_engine::{BorderEdges, CellRange, LineStyle};

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

fn palette(i: i64) -> Result<u32, ScriptError> {
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
            "patterncolorindex" => Value::Long(XL_AUTOMATIC as i32),
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
            "pattern" => {
                if to_i64(&value)? == XL_NONE {
                    set("")
                } else {
                    Ok(())
                }
            }
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
        // Diagonal lines aren't drawn; asking for none of them is fine.
        Some(XL_DIAGONAL_DOWN | XL_DIAGONAL_UP) => BorderEdges::None,
        Some(_) => return Err(ScriptError::runtime(9)),
    })
}

fn line_style(line: i64, weight: i64) -> Option<LineStyle> {
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
    let (rows, cols) = book.used_size(idx);
    for &a in areas {
        let a = if a.is_whole_columns() || a.is_whole_rows() {
            match a.clip(rows.max(1), cols.max(1)) {
                Some(u) => u,
                None => continue,
            }
        } else {
            a
        };
        match line_style(line, weight) {
            Some(style) => book.set_range_border(idx, a, which, style, color),
            // The engine removes all of a range's borders or none: one edge
            // set to `xlNone` is left as it is (recorded macros clear edges
            // that have no border before drawing others).
            None if which == BorderEdges::All => {
                book.set_range_border(idx, a, BorderEdges::None, LineStyle::Thin, None)
            }
            None => Ok(()),
        }
        .map_err(engine)?;
    }
    Ok(())
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
            return Ok(obj(Rc::new(BordersObj {
                s: self.s.clone(),
                sheet: self.sheet,
                areas: self.areas.clone(),
                edge: Some(to_i64(need(args, 0, "Index")?)?),
            })));
        }
        let st = first_style(&self.s, self.sheet, &self.areas)?;
        let b = &st.borders;
        let line = match self.edge {
            Some(XL_EDGE_LEFT) => b.left,
            Some(XL_EDGE_RIGHT) => b.right,
            Some(XL_EDGE_TOP) => b.top,
            Some(XL_EDGE_BOTTOM) => b.bottom,
            _ => b.left.or(b.top).or(b.right).or(b.bottom),
        };
        Ok(match lower.as_str() {
            "linestyle" => Value::Long(match line.map(|l| l.style) {
                None => XL_NONE as i32,
                Some(LineStyle::Double) => XL_DOUBLE as i32,
                Some(LineStyle::Dotted) => XL_DOT as i32,
                Some(LineStyle::Dashed | LineStyle::MediumDashed) => XL_DASH as i32,
                Some(LineStyle::DashDot | LineStyle::MediumDashDot) => XL_DASH_DOT as i32,
                Some(LineStyle::DashDotDot | LineStyle::MediumDashDotDot) => XL_DASH_DOT_DOT as i32,
                Some(LineStyle::SlantDashDot) => XL_SLANT_DASH_DOT as i32,
                Some(_) => XL_CONTINUOUS as i32,
            }),
            "weight" => Value::Long(match line.map(|l| l.style) {
                Some(LineStyle::Hair) => XL_HAIRLINE as i32,
                Some(LineStyle::Medium | LineStyle::MediumDashed | LineStyle::MediumDashDot) => {
                    XL_MEDIUM as i32
                }
                Some(LineStyle::Thick) => XL_THICK as i32,
                _ => XL_THIN as i32,
            }),
            "color" => Value::Long(rgb_to_bgr(line.and_then(|l| l.color).unwrap_or(0))),
            "colorindex" => Value::Long(match line.and_then(|l| l.color) {
                None => XL_COLOR_INDEX_AUTOMATIC as i32,
                Some(c) => color_index(c),
            }),
            "count" => Value::Long(8),
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
        let which = edges(self.edge)?;
        if which == BorderEdges::None {
            return match (lower.as_str(), to_i64(&value)) {
                ("linestyle", Ok(XL_NONE)) => Ok(()),
                _ => Err(ScriptError::unsupported("Diagonal borders")),
            };
        }
        match lower.as_str() {
            "linestyle" => {
                let line = to_i64(&value)?;
                draw_border(&self.s, self.sheet, &self.areas, which, line, XL_THIN, None)
            }
            "weight" => {
                let weight = to_i64(&value)?;
                draw_border(
                    &self.s,
                    self.sheet,
                    &self.areas,
                    which,
                    XL_CONTINUOUS,
                    weight,
                    None,
                )
            }
            "color" => draw_border(
                &self.s,
                self.sheet,
                &self.areas,
                which,
                XL_CONTINUOUS,
                XL_THIN,
                Some(bgr_to_rgb(to_i64(&value)?)),
            ),
            "colorindex" => {
                let i = to_i64(&value)?;
                let color = if i == XL_COLOR_INDEX_AUTOMATIC || i == XL_COLOR_INDEX_NONE {
                    None
                } else {
                    Some(palette(i)?)
                };
                draw_border(
                    &self.s,
                    self.sheet,
                    &self.areas,
                    which,
                    XL_CONTINUOUS,
                    XL_THIN,
                    color,
                )
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
