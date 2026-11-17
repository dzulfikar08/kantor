//! Drawing what conditional formatting adds to a cell besides its format
//! (the engine works it out, see `sheets_engine::Conditional`): data bars
//! under the value and icon-set icons at the cell's left edge. Icons are
//! small vector shapes painted on a canvas, in the rule's colours, so they
//! read the same on light and dark themes.

use gpui_kit::prelude::*;
use gpui_kit::*;
use std::borrow::Cow;

use sheets_engine::{CellStyle, CfIcon, Conditional, DataBar};

use crate::look::hsla;

/// Icon size (square).
const ICON: Pixels = px(13.);
/// Space kept above and below a data bar.
const BAR_INSET: Pixels = px(3.);

/// The layers drawn under the cell's text: the data bar and its axis, then
/// the icon, at the sheet's zoom `zoom`. `None` when the rule only changes
/// the cell's format.
pub(crate) fn layers(c: &Conditional, zoom: f32) -> Vec<AnyElement> {
    let mut out = Vec::new();
    if let Some(bar) = c.bar {
        out.extend(bar_layers(bar, zoom));
    }
    if let Some(icon) = c.icon {
        out.push(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(3.) * zoom)
                .flex()
                .items_center()
                .child(icon_sized(icon, ICON * zoom))
                .into_any_element(),
        );
    }
    out
}

/// The style the value's colour is chosen against: a data bar that reaches
/// under the (right-aligned) value of an unfilled cell counts as a pale
/// fill, so the value reads dark on it, as in Excel, whatever the theme.
pub(crate) fn text_backdrop<'a>(
    style: &'a CellStyle,
    c: Option<&Conditional>,
) -> Cow<'a, CellStyle> {
    match c.and_then(|c| c.bar) {
        Some(bar) if style.fill.is_none() && bar.to >= 0.6 => {
            let mix = |s: u32| {
                let v = f32::from(((bar.color >> s) & 0xff) as u8);
                ((v + (255.0 - v) * 0.85).round() as u32) << s
            };
            Cow::Owned(CellStyle {
                fill: Some(mix(16) | mix(8) | mix(0)),
                ..style.clone()
            })
        }
        _ => Cow::Borrowed(style),
    }
}

fn bar_layers(bar: DataBar, zoom: f32) -> Vec<AnyElement> {
    let color = hsla(bar.color);
    // Excel's gradient fades from the bar colour to near white at the end
    // away from the axis.
    let pale = color.blend(gpui::white().opacity(0.85));
    let fill = if bar.gradient {
        let (from, to) = if bar.negative {
            (pale, color)
        } else {
            (color, pale)
        };
        linear_gradient(90., linear_color_stop(from, 0.), linear_color_stop(to, 1.))
    } else {
        color.into()
    };
    let width = (bar.to - bar.from).max(0.0);
    let mut out = vec![
        div()
            .absolute()
            .top(BAR_INSET * zoom)
            .bottom(BAR_INSET * zoom)
            .left(relative(bar.from))
            .w(relative(width))
            .bg(fill)
            // A solid bar's edge, or the border the file gives it.
            .when(!bar.gradient || bar.border.is_some(), |d| {
                d.border_1().border_color(bar.border.map_or(color, hsla))
            })
            .into_any_element(),
    ];
    if let Some(axis) = bar.axis {
        out.push(
            div()
                .absolute()
                .top(px(1.))
                .bottom(px(1.))
                .left(relative(axis))
                .w(px(1.))
                .bg(bar.axis_color.map_or(gpui::black().opacity(0.6), hsla))
                .into_any_element(),
        );
    }
    out
}

pub(crate) fn icon_element(icon: CfIcon) -> impl IntoElement {
    icon_sized(icon, ICON)
}

/// An icon `size` px square.
fn icon_sized(icon: CfIcon, size: Pixels) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |b, _, window, _| paint_icon(icon, b, window),
    )
    .size(size)
    .flex_none()
}

/// Paint an icon's polygons (`sheets_engine`'s shapes, shared with PDF
/// export) into `b`.
fn paint_icon(icon: CfIcon, b: Bounds<Pixels>, window: &mut Window) {
    let s = f32::from(b.size.width);
    for path in icon.paths() {
        let pts: Vec<Point<Pixels>> = path
            .points
            .iter()
            .map(|&(x, y)| point(b.origin.x + px(x * s), b.origin.y + px(y * s)))
            .collect();
        if pts.len() < 3 {
            continue;
        }
        let mut p = match path.stroke {
            Some(w) => PathBuilder::stroke(px(w * s)),
            None => PathBuilder::fill(),
        };
        p.add_polygon(&pts, true);
        if let Ok(built) = p.build() {
            window.paint_path(built, hsla(path.color));
        }
    }
}

/// A patterned cell fill (`FillPattern`): its 8 × 8 tile's dots in its
/// foreground colour, `unit` px each, over the whole cell (the cell draws
/// the background colour itself).
pub(crate) fn pattern_layer(pattern: sheets_engine::FillPattern, unit: f32) -> AnyElement {
    let color = hsla(pattern.fg);
    let tile = pattern.tile();
    let unit = unit.max(0.25);
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
            let cols = (w / unit).ceil() as usize;
            let rows = (h / unit).ceil() as usize;
            for y in 0..rows.min(4096) {
                let bits = tile[y % 8];
                if bits == 0 {
                    continue;
                }
                let top = b.origin.y + px(y as f32 * unit);
                // Runs of set bits along the row, the tile repeated.
                let mut x = 0;
                while x < cols.min(8192) {
                    if bits & (0x80 >> (x % 8)) == 0 {
                        x += 1;
                        continue;
                    }
                    let start = x;
                    while x < cols && bits & (0x80 >> (x % 8)) != 0 {
                        x += 1;
                    }
                    let left = start as f32 * unit;
                    let right = (x as f32 * unit).min(w);
                    window.paint_quad(fill(
                        Bounds::new(
                            point(b.origin.x + px(left), top),
                            size(px(right - left), px(unit.min(h - y as f32 * unit))),
                        ),
                        color,
                    ));
                }
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}
