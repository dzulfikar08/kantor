//! Drawing what conditional formatting adds to a cell besides its format
//! (the engine works it out, see `sheets_engine::Conditional`): data bars
//! under the value and icon-set icons at the cell's left edge. Icons are
//! small vector shapes painted on a canvas, in the rule's colours, so they
//! read the same on light and dark themes.

use std::f32::consts::PI;

use gpui_kit::prelude::*;
use gpui_kit::*;
use sheets_engine::{CfIcon, Conditional, DataBar, IconShape};

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

fn polygon(window: &mut Window, pts: &[Point<Pixels>], color: Hsla) {
    if pts.len() < 3 {
        return;
    }
    let mut p = PathBuilder::fill();
    p.add_polygon(pts, true);
    if let Ok(path) = p.build() {
        window.paint_path(path, color);
    }
}

/// Points of a circle (or an arc from `a0` to `a1`, radians, clockwise from
/// 12 o'clock) around `c`.
fn arc(c: Point<Pixels>, r: f32, a0: f32, a1: f32, steps: usize) -> Vec<Point<Pixels>> {
    (0..=steps)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / steps as f32;
            point(c.x + px(r * a.sin()), c.y - px(r * a.cos()))
        })
        .collect()
}

fn star(c: Point<Pixels>, r: f32) -> Vec<Point<Pixels>> {
    (0..10)
        .map(|i| {
            let rr = if i % 2 == 0 { r } else { r * 0.45 };
            let a = PI * i as f32 / 5.0;
            point(c.x + px(rr * a.sin()), c.y - px(rr * a.cos()))
        })
        .collect()
}

fn paint_icon(icon: CfIcon, b: Bounds<Pixels>, window: &mut Window) {
    let color = hsla(icon.color);
    let grey = hsla(0xBF_BF_BF);
    let s = f32::from(b.size.width);
    // Points in a 0..1 box.
    let p = |x: f32, y: f32| point(b.origin.x + px(x * s), b.origin.y + px(y * s));
    let c = p(0.5, 0.5);
    let r = s * 0.46;
    match icon.shape {
        IconShape::ArrowUp => polygon(
            window,
            &[
                p(0.5, 0.05),
                p(0.95, 0.5),
                p(0.66, 0.5),
                p(0.66, 0.95),
                p(0.34, 0.95),
                p(0.34, 0.5),
                p(0.05, 0.5),
            ],
            color,
        ),
        IconShape::ArrowDown => polygon(
            window,
            &[
                p(0.5, 0.95),
                p(0.95, 0.5),
                p(0.66, 0.5),
                p(0.66, 0.05),
                p(0.34, 0.05),
                p(0.34, 0.5),
                p(0.05, 0.5),
            ],
            color,
        ),
        IconShape::ArrowRight => polygon(
            window,
            &[
                p(0.95, 0.5),
                p(0.5, 0.05),
                p(0.5, 0.34),
                p(0.05, 0.34),
                p(0.05, 0.66),
                p(0.5, 0.66),
                p(0.5, 0.95),
            ],
            color,
        ),
        IconShape::ArrowUpRight => polygon(
            window,
            &[
                p(0.92, 0.08),
                p(0.92, 0.62),
                p(0.72, 0.42),
                p(0.28, 0.86),
                p(0.14, 0.72),
                p(0.58, 0.28),
                p(0.38, 0.08),
            ],
            color,
        ),
        IconShape::ArrowDownRight => polygon(
            window,
            &[
                p(0.92, 0.92),
                p(0.38, 0.92),
                p(0.58, 0.72),
                p(0.14, 0.28),
                p(0.28, 0.14),
                p(0.72, 0.58),
                p(0.92, 0.38),
            ],
            color,
        ),
        IconShape::Circle => polygon(window, &arc(c, r, 0.0, 2.0 * PI, 32), color),
        IconShape::TriangleUp => {
            polygon(window, &[p(0.5, 0.12), p(0.95, 0.85), p(0.05, 0.85)], color)
        }
        IconShape::TriangleDown => {
            polygon(window, &[p(0.5, 0.88), p(0.95, 0.15), p(0.05, 0.15)], color)
        }
        IconShape::Dash => polygon(
            window,
            &[p(0.1, 0.38), p(0.9, 0.38), p(0.9, 0.62), p(0.1, 0.62)],
            color,
        ),
        IconShape::Diamond => polygon(
            window,
            &[p(0.5, 0.04), p(0.96, 0.5), p(0.5, 0.96), p(0.04, 0.5)],
            color,
        ),
        IconShape::Flag => {
            polygon(
                window,
                &[p(0.18, 0.06), p(0.28, 0.06), p(0.28, 0.96), p(0.18, 0.96)],
                hsla(0x59_59_59),
            );
            polygon(window, &[p(0.28, 0.08), p(0.92, 0.3), p(0.28, 0.54)], color);
        }
        IconShape::Check => {
            polygon(window, &arc(c, r, 0.0, 2.0 * PI, 32), color);
            polygon(
                window,
                &[
                    p(0.22, 0.5),
                    p(0.32, 0.4),
                    p(0.44, 0.54),
                    p(0.7, 0.26),
                    p(0.8, 0.36),
                    p(0.44, 0.74),
                ],
                gpui::white(),
            );
        }
        IconShape::Cross => {
            polygon(window, &arc(c, r, 0.0, 2.0 * PI, 32), color);
            let bar = |a: f32| {
                let (dx, dy) = (a.cos(), a.sin());
                let (w, l) = (0.08, 0.27);
                [(l, w), (l, -w), (-l, -w), (-l, w)]
                    .map(|(u, v)| p(0.5 + u * dx - v * dy, 0.5 + u * dy + v * dx))
            };
            polygon(window, &bar(PI / 4.0), gpui::white());
            polygon(window, &bar(-PI / 4.0), gpui::white());
        }
        IconShape::Exclamation => {
            polygon(window, &[p(0.5, 0.04), p(0.97, 0.92), p(0.03, 0.92)], color);
            polygon(
                window,
                &[p(0.45, 0.32), p(0.55, 0.32), p(0.53, 0.64), p(0.47, 0.64)],
                gpui::white(),
            );
            polygon(
                window,
                &[p(0.45, 0.71), p(0.55, 0.71), p(0.55, 0.8), p(0.45, 0.8)],
                gpui::white(),
            );
        }
        IconShape::Heart => {
            let mut pts = arc(p(0.3, 0.36), s * 0.22, -PI / 2.0, PI * 0.55, 12);
            pts.push(p(0.5, 0.92));
            pts.extend(arc(p(0.7, 0.36), s * 0.22, -PI * 0.55, PI / 2.0, 12));
            polygon(window, &pts, color);
        }
        IconShape::ThumbsUp | IconShape::ThumbsDown => {
            let up = icon.shape == IconShape::ThumbsUp;
            let y = |v: f32| if up { v } else { 1.0 - v };
            polygon(
                window,
                &[
                    p(0.08, y(0.45)),
                    p(0.28, y(0.45)),
                    p(0.28, y(0.92)),
                    p(0.08, y(0.92)),
                ],
                color,
            );
            polygon(
                window,
                &[
                    p(0.32, y(0.45)),
                    p(0.5, y(0.08)),
                    p(0.6, y(0.12)),
                    p(0.56, y(0.4)),
                    p(0.92, y(0.42)),
                    p(0.86, y(0.92)),
                    p(0.32, y(0.92)),
                ],
                color,
            );
        }
        IconShape::Star { filled, of } => {
            let pts = star(c, r);
            polygon(window, &pts, grey);
            let part = if of == 0 {
                1.0
            } else {
                f32::from(filled) / f32::from(of)
            };
            if part > 0.0 {
                // The filled share from the left: the star's points clamped
                // to a vertical line.
                let edge = b.origin.x + px(s * part);
                let clipped: Vec<_> = pts.iter().map(|q| point(q.x.min(edge), q.y)).collect();
                polygon(window, &clipped, color);
            }
        }
        IconShape::Pie { filled, of } => {
            polygon(window, &arc(c, r, 0.0, 2.0 * PI, 32), grey);
            let part = if of == 0 {
                1.0
            } else {
                f32::from(filled) / f32::from(of)
            };
            if part >= 1.0 {
                polygon(window, &arc(c, r, 0.0, 2.0 * PI, 32), color);
            } else if part > 0.0 {
                let mut pts = vec![c];
                pts.extend(arc(c, r, 0.0, 2.0 * PI * part, 24));
                polygon(window, &pts, color);
            }
            if part < 1.0 {
                // The outline Excel draws around the quarters.
                let mut ring = PathBuilder::stroke(px(1.));
                ring.add_polygon(&arc(c, r, 0.0, 2.0 * PI, 32), true);
                if let Ok(path) = ring.build() {
                    window.paint_path(path, color);
                }
            }
        }
        IconShape::Bars { lit, of } => {
            // Four signal bars, lit in proportion to the step (none at the
            // lowest).
            let n: u8 = 4;
            let lit = (f32::from(lit) * 4.0 / f32::from(of.max(1))).round() as u8;
            let gap = 0.08;
            let w = (0.9 - gap * f32::from(n - 1)) / f32::from(n);
            for i in 0..n {
                let x = 0.05 + f32::from(i) * (w + gap);
                let h = 0.3 + 0.65 * f32::from(i + 1) / f32::from(n);
                let col = if i < lit { color } else { grey };
                polygon(
                    window,
                    &[
                        p(x, 0.95 - h),
                        p(x + w, 0.95 - h),
                        p(x + w, 0.95),
                        p(x, 0.95),
                    ],
                    col,
                );
            }
        }
    }
}
