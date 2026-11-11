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
/// the icon. `None` when the rule only changes the cell's format.
pub(crate) fn layers(c: &Conditional) -> Vec<AnyElement> {
    let mut out = Vec::new();
    if let Some(bar) = c.bar {
        out.extend(bar_layers(bar));
    }
    if let Some(icon) = c.icon {
        out.push(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(3.))
                .flex()
                .items_center()
                .child(icon_element(icon))
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

fn bar_layers(bar: DataBar) -> Vec<AnyElement> {
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
            .top(BAR_INSET)
            .bottom(BAR_INSET)
            .left(relative(bar.from))
            .w(relative(width))
            .bg(fill)
            .when(!bar.gradient, |d| d.border_1().border_color(color))
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
                .bg(gpui::black().opacity(0.6))
                .into_any_element(),
        );
    }
    out
}

fn icon_element(icon: CfIcon) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |b, _, window, _| paint_icon(icon, b, window),
    )
    .size(ICON)
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
