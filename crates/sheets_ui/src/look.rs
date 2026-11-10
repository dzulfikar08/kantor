//! Drawing a cell's formatting: fill, font, alignment, wrap, borders,
//! gridlines and merged cells. The engine works out what a cell looks like
//! ([`CellLook`]); this turns it into elements.
//!
//! The table gives every row the same height and clips each cell, so:
//!
//! * A merged cell is drawn by *every* cell it covers: each draws the whole
//!   merged block shifted by its own offset inside the merge, and its clip
//!   shows its slice. Together they show one cell, with no per-cell seams.
//! * Larger fonts are capped to what fits the (merged) cell's height, and
//!   wrapped text shows the lines that fit. Row heights from the file are
//!   kept (saved) but not drawn.
//! * Text rotation and indent aren't drawn (IronCalc's model doesn't keep
//!   them), nor is text overflowing into empty neighbours.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::component::Size;
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::{Tokens, metrics::*};
use sheets_engine::{
    BorderLine, CellLook, CellRange, CellStyle, HAlign, LineStyle, VAlign, Workbook,
};

/// Height of every grid row (the table's `small` size).
pub(crate) fn row_height() -> Pixels {
    Size::Small.table_row_height()
}

/// Line height of cell text, relative to its size.
const LINE_HEIGHT: f32 = 1.25;
/// Most room kept above top-aligned text.
const V_PAD: Pixels = px(3.);

/// Font families of the cells, resolved once per name: the family itself
/// when installed, else its metric-compatible substitute, else the UI font.
#[derive(Default)]
pub(crate) struct FontCache {
    families: HashMap<Arc<str>, Option<SharedString>>,
}

impl FontCache {
    fn family(&mut self, name: &Arc<str>, cx: &App) -> Option<SharedString> {
        if let Some(f) = self.families.get(name) {
            return f.clone();
        }
        let installed = kantor_ui::fonts::installed(cx);
        let has = |f: &str| installed.binary_search_by(|i| i.as_str().cmp(f)).is_ok();
        let family = if has(name) {
            Some(SharedString::from(name.to_string()))
        } else {
            kantor_core::fonts::metric_substitute(name)
                .filter(|s| has(s))
                .map(SharedString::new_static)
        };
        self.families.insert(name.clone(), family.clone());
        family
    }
}

pub(crate) fn hsla(c: u32) -> Hsla {
    rgb(c).into()
}

/// WCAG relative luminance of a colour.
fn luminance(c: Hsla) -> f32 {
    let c = Rgba::from(c);
    let lin = |v: f32| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

fn contrast(a: Hsla, b: Hsla) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The colour text is drawn in. Automatic text is black or white on a fill
/// (whichever reads), the theme's text colour otherwise. A colour the file
/// sets is kept, except on no fill when it would vanish into the theme's
/// background (black text in a dark theme).
pub(crate) fn text_color(style: &CellStyle, t: &Tokens) -> Hsla {
    text_color_on(style, t.fg, t.bg)
}

fn text_color_on(style: &CellStyle, fg: Hsla, bg: Hsla) -> Hsla {
    match (style.font_color.map(hsla), style.fill.map(hsla)) {
        (Some(c), Some(_)) => c,
        (None, Some(fill)) => {
            if luminance(fill) > 0.18 {
                hsla(0x000000)
            } else {
                hsla(0xFFFFFF)
            }
        }
        (Some(c), None) if contrast(c, bg) >= 1.8 => c,
        _ => fg,
    }
}

/// An automatic-colour border: the theme's text colour (black in a light
/// theme, readable in a dark one) unless it sits on a fill.
fn border_color(line: &BorderLine, filled: bool, t: &Tokens) -> Hsla {
    match line.color {
        Some(c) => hsla(c),
        None if filled => hsla(0x000000),
        None => t.fg,
    }
}

#[derive(Clone, Copy)]
enum Edge {
    Top,
    Right,
    Bottom,
    Left,
}

/// One border edge along a side of the cell.
fn border_edge(edge: Edge, line: &BorderLine, color: Hsla) -> Div {
    let w = px(line.style.width());
    let d = div().absolute();
    let d = match edge {
        Edge::Top => d.top_0().left_0().right_0().h(w),
        Edge::Bottom => d.bottom_0().left_0().right_0().h(w),
        Edge::Left => d.left_0().top_0().bottom_0().w(w),
        Edge::Right => d.right_0().top_0().bottom_0().w(w),
    };
    let vertical = matches!(edge, Edge::Left | Edge::Right);
    match line.style {
        // Two thin lines with a gap.
        LineStyle::Double => {
            let d = d.border_color(color);
            if vertical {
                d.border_l_1().border_r_1()
            } else {
                d.border_t_1().border_b_1()
            }
        }
        LineStyle::Dotted | LineStyle::MediumDashed => {
            let d = d.border_dashed().border_color(color);
            match edge {
                Edge::Top => d.border_t(w),
                Edge::Bottom => d.border_b(w),
                Edge::Left => d.border_l(w),
                Edge::Right => d.border_r(w),
            }
        }
        _ => d.bg(color),
    }
}

/// The fill, gridlines and borders of a cell (absolute, under the overlays).
pub(crate) fn decorations(look: &CellLook, t: &Tokens) -> impl Iterator<Item = Div> {
    let filled = look.style.fill.is_some();
    let grid = |edge: Edge| {
        let d = div().absolute().bg(t.grid_line);
        match edge {
            Edge::Right => d.top_0().bottom_0().right_0().w(GRID_LINE),
            _ => d.left_0().right_0().bottom_0().h(GRID_LINE),
        }
    };
    let gridlines = [
        look.grid_right.then(|| grid(Edge::Right)),
        look.grid_bottom.then(|| grid(Edge::Bottom)),
    ];
    let borders = [
        (Edge::Right, look.right),
        (Edge::Bottom, look.bottom),
        (Edge::Top, look.top),
        (Edge::Left, look.left),
    ]
    .into_iter()
    .filter_map(move |(edge, line)| {
        line.map(|line| border_edge(edge, &line, border_color(&line, filled, t)))
    });
    gridlines.into_iter().flatten().chain(borders)
}

/// Where the active cell goes when the table selects `to` (coming from
/// `from`) inside merged cell `m`: nowhere else if `to` is the anchor; past
/// the merge when the move started at its anchor (an arrow key or Enter
/// from the merged cell) and went right or down; the anchor otherwise (a
/// click, or arriving from outside). `size` is the grid's `(rows, cols)`.
pub(crate) fn merge_step(
    m: CellRange,
    from: (u32, u32),
    to: (u32, u32),
    clicked: bool,
    size: (u32, u32),
) -> Option<(u32, u32)> {
    let anchor = (m.top, m.left);
    if to == anchor {
        return None;
    }
    if clicked || from != anchor {
        return Some(anchor);
    }
    Some(if to.0 > from.0 {
        ((m.bottom + 1).min(size.0 - 1), to.1)
    } else if to.1 > from.1 {
        (to.0, (m.right + 1).min(size.1 - 1))
    } else {
        anchor
    })
}

/// Where a merged cell's block sits relative to one of its cells, and its
/// size: `(offset_x, offset_y, width, height)`.
pub(crate) fn merge_geometry(
    book: &Workbook,
    sheet: u32,
    merge: CellRange,
    row: u32,
    col: u32,
) -> (Pixels, Pixels, Pixels, Pixels) {
    let width = |from: u32, to: u32| -> f32 {
        (from..to)
            .map(|c| book.column_width(sheet, c) as f32)
            .sum::<f32>()
    };
    let h = row_height();
    (
        px(width(merge.left, col)),
        h * (row - merge.top) as f32,
        px(width(merge.left, merge.right + 1)),
        h * merge.rows_count() as f32,
    )
}

/// The text of a cell (or merged cell) laid out in a box of `height`.
pub(crate) struct Content<'a> {
    pub text: SharedString,
    pub style: &'a CellStyle,
    pub align: HAlign,
    pub color: Hsla,
    /// Text size of the workbook's default font.
    pub base_size: Pixels,
    pub default_font_size: i32,
    pub family: Option<SharedString>,
    pub height: Pixels,
}

impl Content<'_> {
    pub(crate) fn render(self) -> Div {
        let s = self.style;
        let scale = s.font_size.max(1) as f32 / self.default_font_size.max(1) as f32;
        // Larger fonts are capped to what fits one line in the box.
        let fit = (self.height - px(2.)) / LINE_HEIGHT;
        let size = (self.base_size * scale).min(fit).max(px(6.));
        // Free height around one line. Bottom-aligned text (Excel's default)
        // keeps 40% of it below, so a plain cell reads about centred in our
        // tall rows while larger or wrapped text stays anchored to the
        // bottom edge.
        let slack = (self.height - size * LINE_HEIGHT).max(px(0.));
        let text = div()
            .min_w_0()
            .w_full()
            .text_size(size)
            .line_height(relative(LINE_HEIGHT))
            .text_color(self.color)
            .when_some(self.family, |d, f| d.font_family(f))
            .when(s.bold, |d| d.font_weight(FontWeight::BOLD))
            .when(s.italic, |d| d.italic())
            .when(s.underline, |d| d.underline())
            .when(s.strikethrough, |d| d.line_through())
            .when(self.align == HAlign::Center, |d| d.text_center())
            .when(self.align == HAlign::Right, |d| d.text_right())
            .map(|d| {
                if s.wrap {
                    d.whitespace_normal().overflow_hidden()
                } else {
                    d.truncate()
                }
            })
            .child(self.text);
        div()
            .size_full()
            .flex()
            .px(SPACE_3)
            .overflow_hidden()
            .map(|d| match s.valign {
                VAlign::Top => d.items_start().pt(V_PAD.min(slack / 2.)),
                VAlign::Center => d.items_center(),
                VAlign::Bottom => d.items_end().pb(slack * 0.4),
            })
            .child(text)
    }
}

impl FontCache {
    /// The family to draw `style` in (`None`: the UI font).
    pub(crate) fn for_style(&mut self, style: &CellStyle, cx: &App) -> Option<SharedString> {
        style.font_name.as_ref().and_then(|n| self.family(n, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn merge_navigation() {
        // B2:D4 merged; the grid is 100 x 30.
        let m = CellRange::from_corners((1, 1), (3, 3));
        let size = (100, 30);
        // Landing on the anchor needs nothing.
        assert_eq!(merge_step(m, (0, 1), (1, 1), false, size), None);
        // Arriving inside from outside (arrow up from below, left from the
        // right) or clicking inside: the anchor.
        assert_eq!(merge_step(m, (4, 2), (3, 2), false, size), Some((1, 1)));
        assert_eq!(merge_step(m, (2, 4), (2, 3), false, size), Some((1, 1)));
        assert_eq!(merge_step(m, (1, 1), (2, 2), true, size), Some((1, 1)));
        // Moving on from the anchor steps past the merge.
        assert_eq!(merge_step(m, (1, 1), (1, 2), false, size), Some((1, 4)));
        assert_eq!(merge_step(m, (1, 1), (2, 1), false, size), Some((4, 1)));
        // ... but not off the grid.
        let corner = CellRange::from_corners((98, 28), (99, 29));
        assert_eq!(
            merge_step(corner, (98, 28), (98, 29), false, size),
            Some((98, 29))
        );
    }

    #[std::prelude::v1::test]
    fn merge_geometry_offsets() {
        let mut book = Workbook::new_empty().unwrap();
        book.set_column_width(0, 1, 1, 100.0).unwrap();
        book.set_column_width(0, 2, 2, 50.0).unwrap();
        let m = CellRange::from_corners((2, 1), (3, 3));
        let w3 = book.column_width(0, 3) as f32;
        let h = row_height();
        assert_eq!(
            merge_geometry(&book, 0, m, 2, 1),
            (px(0.), px(0.), px(150. + w3), h * 2.)
        );
        assert_eq!(
            merge_geometry(&book, 0, m, 3, 3),
            (px(150.), h, px(150. + w3), h * 2.)
        );
    }

    #[std::prelude::v1::test]
    fn automatic_and_rescued_text_colours() {
        let dark_bg = hsla(0x1E1E2E);
        let light_fg = hsla(0xCDD6F4);
        let style = |font_color, fill| CellStyle {
            font_color,
            fill,
            ..CellStyle::default()
        };
        // Automatic text on no fill: the theme's text colour.
        assert_eq!(
            text_color_on(&style(None, None), light_fg, dark_bg),
            light_fg
        );
        // On a fill: black on light, white on dark.
        assert_eq!(
            text_color_on(&style(None, Some(0xFFF2CC)), light_fg, dark_bg),
            hsla(0x000000)
        );
        assert_eq!(
            text_color_on(&style(None, Some(0x1F4E78)), light_fg, dark_bg),
            hsla(0xFFFFFF)
        );
        // Black text with no fill in a dark theme would vanish: theme colour.
        assert_eq!(
            text_color_on(&style(Some(0x000000), None), light_fg, dark_bg),
            light_fg
        );
        // A readable colour is kept, and any colour on a fill.
        assert_eq!(
            text_color_on(&style(Some(0xFF0000), None), light_fg, dark_bg),
            hsla(0xFF0000)
        );
        assert_eq!(
            text_color_on(&style(Some(0x000000), Some(0x000000)), light_fg, dark_bg),
            hsla(0x000000)
        );
    }
}
