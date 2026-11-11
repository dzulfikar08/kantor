//! The shapes of icon-set icons as polygons in a unit box, shared by the
//! grid (painted with GPUI) and PDF export (written with krilla), so both
//! draw the same icon.

use std::f32::consts::PI;

use crate::cond_format::{CfIcon, IconShape};
use crate::format::Rgb;

/// One polygon of an icon: points in a `0..1` box (y down), filled, or
/// stroked as a closed ring `stroke` box-widths wide.
#[derive(Debug, Clone, PartialEq)]
pub struct IconPath {
    pub points: Vec<(f32, f32)>,
    pub color: Rgb,
    pub stroke: Option<f32>,
}

const WHITE: Rgb = 0xFF_FF_FF;
const GREY: Rgb = 0xBF_BF_BF;

/// Points of a circle (or an arc from `a0` to `a1`, radians, clockwise from
/// 12 o'clock) around `c`.
fn arc(c: (f32, f32), r: f32, a0: f32, a1: f32, steps: usize) -> Vec<(f32, f32)> {
    (0..=steps)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / steps as f32;
            (c.0 + r * a.sin(), c.1 - r * a.cos())
        })
        .collect()
}

fn star(c: (f32, f32), r: f32) -> Vec<(f32, f32)> {
    (0..10)
        .map(|i| {
            let rr = if i % 2 == 0 { r } else { r * 0.45 };
            let a = PI * i as f32 / 5.0;
            (c.0 + rr * a.sin(), c.1 - rr * a.cos())
        })
        .collect()
}

impl CfIcon {
    /// The icon's polygons, back to front.
    pub fn paths(&self) -> Vec<IconPath> {
        let color = self.color;
        let fill = |points: Vec<(f32, f32)>, color: Rgb| IconPath {
            points,
            color,
            stroke: None,
        };
        let c = (0.5, 0.5);
        let r = 0.46;
        let circle = || arc(c, r, 0.0, 2.0 * PI, 32);
        match self.shape {
            IconShape::ArrowUp => vec![fill(
                vec![
                    (0.5, 0.05),
                    (0.95, 0.5),
                    (0.66, 0.5),
                    (0.66, 0.95),
                    (0.34, 0.95),
                    (0.34, 0.5),
                    (0.05, 0.5),
                ],
                color,
            )],
            IconShape::ArrowDown => vec![fill(
                vec![
                    (0.5, 0.95),
                    (0.95, 0.5),
                    (0.66, 0.5),
                    (0.66, 0.05),
                    (0.34, 0.05),
                    (0.34, 0.5),
                    (0.05, 0.5),
                ],
                color,
            )],
            IconShape::ArrowRight => vec![fill(
                vec![
                    (0.95, 0.5),
                    (0.5, 0.05),
                    (0.5, 0.34),
                    (0.05, 0.34),
                    (0.05, 0.66),
                    (0.5, 0.66),
                    (0.5, 0.95),
                ],
                color,
            )],
            IconShape::ArrowUpRight => vec![fill(
                vec![
                    (0.92, 0.08),
                    (0.92, 0.62),
                    (0.72, 0.42),
                    (0.28, 0.86),
                    (0.14, 0.72),
                    (0.58, 0.28),
                    (0.38, 0.08),
                ],
                color,
            )],
            IconShape::ArrowDownRight => vec![fill(
                vec![
                    (0.92, 0.92),
                    (0.38, 0.92),
                    (0.58, 0.72),
                    (0.14, 0.28),
                    (0.28, 0.14),
                    (0.72, 0.58),
                    (0.92, 0.38),
                ],
                color,
            )],
            IconShape::Circle => vec![fill(circle(), color)],
            IconShape::TriangleUp => {
                vec![fill(vec![(0.5, 0.12), (0.95, 0.85), (0.05, 0.85)], color)]
            }
            IconShape::TriangleDown => {
                vec![fill(vec![(0.5, 0.88), (0.95, 0.15), (0.05, 0.15)], color)]
            }
            IconShape::Dash => vec![fill(
                vec![(0.1, 0.38), (0.9, 0.38), (0.9, 0.62), (0.1, 0.62)],
                color,
            )],
            IconShape::Diamond => vec![fill(
                vec![(0.5, 0.04), (0.96, 0.5), (0.5, 0.96), (0.04, 0.5)],
                color,
            )],
            IconShape::Flag => vec![
                fill(
                    vec![(0.18, 0.06), (0.28, 0.06), (0.28, 0.96), (0.18, 0.96)],
                    0x59_59_59,
                ),
                fill(vec![(0.28, 0.08), (0.92, 0.3), (0.28, 0.54)], color),
            ],
            IconShape::Check => vec![
                fill(circle(), color),
                fill(
                    vec![
                        (0.22, 0.5),
                        (0.32, 0.4),
                        (0.44, 0.54),
                        (0.7, 0.26),
                        (0.8, 0.36),
                        (0.44, 0.74),
                    ],
                    WHITE,
                ),
            ],
            IconShape::Cross => {
                let bar = |a: f32| {
                    let (dx, dy) = (a.cos(), a.sin());
                    let (w, l) = (0.08, 0.27);
                    [(l, w), (l, -w), (-l, -w), (-l, w)]
                        .map(|(u, v)| (0.5 + u * dx - v * dy, 0.5 + u * dy + v * dx))
                        .to_vec()
                };
                vec![
                    fill(circle(), color),
                    fill(bar(PI / 4.0), WHITE),
                    fill(bar(-PI / 4.0), WHITE),
                ]
            }
            IconShape::Exclamation => vec![
                fill(vec![(0.5, 0.04), (0.97, 0.92), (0.03, 0.92)], color),
                fill(
                    vec![(0.45, 0.32), (0.55, 0.32), (0.53, 0.64), (0.47, 0.64)],
                    WHITE,
                ),
                fill(
                    vec![(0.45, 0.71), (0.55, 0.71), (0.55, 0.8), (0.45, 0.8)],
                    WHITE,
                ),
            ],
            IconShape::Heart => {
                let mut pts = arc((0.3, 0.36), 0.22, -PI / 2.0, PI * 0.55, 12);
                pts.push((0.5, 0.92));
                pts.extend(arc((0.7, 0.36), 0.22, -PI * 0.55, PI / 2.0, 12));
                vec![fill(pts, color)]
            }
            IconShape::ThumbsUp | IconShape::ThumbsDown => {
                let up = self.shape == IconShape::ThumbsUp;
                let y = |v: f32| if up { v } else { 1.0 - v };
                vec![
                    fill(
                        vec![
                            (0.08, y(0.45)),
                            (0.28, y(0.45)),
                            (0.28, y(0.92)),
                            (0.08, y(0.92)),
                        ],
                        color,
                    ),
                    fill(
                        vec![
                            (0.32, y(0.45)),
                            (0.5, y(0.08)),
                            (0.6, y(0.12)),
                            (0.56, y(0.4)),
                            (0.92, y(0.42)),
                            (0.86, y(0.92)),
                            (0.32, y(0.92)),
                        ],
                        color,
                    ),
                ]
            }
            IconShape::Star { filled, of } => {
                let pts = star(c, r);
                let part = if of == 0 {
                    1.0
                } else {
                    f32::from(filled) / f32::from(of)
                };
                let mut out = vec![fill(pts.clone(), GREY)];
                if part > 0.0 {
                    // The filled share from the left: the star's points
                    // clamped to a vertical line.
                    let clipped = pts.iter().map(|q| (q.0.min(part), q.1)).collect();
                    out.push(fill(clipped, color));
                }
                out
            }
            IconShape::Pie { filled, of } => {
                let part = if of == 0 {
                    1.0
                } else {
                    f32::from(filled) / f32::from(of)
                };
                let mut out = vec![fill(circle(), GREY)];
                if part >= 1.0 {
                    out.push(fill(circle(), color));
                } else if part > 0.0 {
                    let mut pts = vec![c];
                    pts.extend(arc(c, r, 0.0, 2.0 * PI * part, 24));
                    out.push(fill(pts, color));
                }
                if part < 1.0 {
                    // The outline Excel draws around the quarters (1 px of
                    // a 13 px icon).
                    out.push(IconPath {
                        points: circle(),
                        color,
                        stroke: Some(1.0 / 13.0),
                    });
                }
                out
            }
            IconShape::Bars { lit, of } => {
                // Four signal bars, lit in proportion to the step (none at
                // the lowest).
                let n: u8 = 4;
                let lit = (f32::from(lit) * 4.0 / f32::from(of.max(1))).round() as u8;
                let gap = 0.08;
                let w = (0.9 - gap * f32::from(n - 1)) / f32::from(n);
                (0..n)
                    .map(|i| {
                        let x = 0.05 + f32::from(i) * (w + gap);
                        let h = 0.3 + 0.65 * f32::from(i + 1) / f32::from(n);
                        fill(
                            vec![(x, 0.95 - h), (x + w, 0.95 - h), (x + w, 0.95), (x, 0.95)],
                            if i < lit { color } else { GREY },
                        )
                    })
                    .collect()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_stays_in_its_box() {
        let shapes = [
            IconShape::ArrowUp,
            IconShape::Check,
            IconShape::Heart,
            IconShape::Star { filled: 1, of: 2 },
            IconShape::Pie { filled: 1, of: 4 },
            IconShape::Bars { lit: 2, of: 4 },
        ];
        for shape in shapes {
            let paths = CfIcon { shape, color: 1 }.paths();
            assert!(!paths.is_empty());
            for p in paths {
                assert!(p.points.len() >= 3);
                assert!(
                    p.points
                        .iter()
                        .all(|&(x, y)| (-0.01..=1.01).contains(&x) && (-0.01..=1.01).contains(&y))
                );
            }
        }
    }
}
