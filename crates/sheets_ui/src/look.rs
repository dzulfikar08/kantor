//! Drawing a cell's formatting: fill, font, alignment, wrap, borders,
//! gridlines and merged cells. The engine works out what a cell looks like
//! ([`CellLook`]); this turns it into elements.
//!
//! The table clips each cell, so:
//!
//! * A merged cell is drawn by *every* cell it covers: each draws the whole
//!   merged block shifted by its own offset inside the merge, and its clip
//!   shows its slice. Together they show one cell, with no per-cell seams.
//! * Rows have their own heights (`row_layout`): automatic rows grow to fit
//!   wrapped text and larger fonts; in a row given a smaller height, larger
//!   fonts are capped to what fits and wrapped text shows the lines that fit.
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

use crate::row_heights::RowHeights;

/// Height of a default grid row (the table's `small` size): a row of the
/// sheet's default height in Excel.
pub(crate) fn row_height() -> Pixels {
    Size::Small.table_row_height()
}

/// `len` (logical px) rounded to whole device pixels at `scale`, the way
/// GPUI rounds authored lengths before layout (half toward zero).
///
/// The grid's column widths go to the table snapped: the scrolling columns
/// are placed one after another from their widths as given, while each
/// cell's own width is rounded, so at 125 % a fractional width left a
/// 1 device-pixel gap between neighbouring cells (a seam through fills and
/// merged cells). Merged-cell offsets are summed from the same snapped
/// lengths, so each slice lines up with its cell.
pub(crate) fn snap(len: f32, scale: f32) -> f32 {
    if scale <= 0.0 {
        return len;
    }
    let dp = len * scale;
    (dp.abs() - 0.5).ceil().copysign(dp).max(1.0) / scale
}

/// Width of a column as the grid lays it out (see [`snap`]).
pub(crate) fn column_width(book: &Workbook, sheet: u32, col: u32, scale: f32) -> Pixels {
    px(snap(book.column_width(sheet, col) as f32, scale))
}

/// Line height of cell text, relative to its size.
pub(crate) const LINE_HEIGHT: f32 = 1.25;
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

/// The colour a cell's value is drawn in: its number format's (`[Red]`)
/// when it has one, kept like a set font colour (rescued when it would vanish
/// into the theme's background on no fill), else [`text_color`].
pub(crate) fn value_color(style: &CellStyle, format: Option<u32>, t: &Tokens) -> Hsla {
    match format.map(hsla) {
        Some(c) if style.fill.is_some() || contrast(c, t.bg) >= 1.8 => c,
        Some(_) => t.fg,
        None => text_color(style, t),
    }
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

/// A border's colour: an automatic one is black on a fill, else the theme's
/// text colour (black in a light theme, readable in a dark one); a set one
/// is kept unless, on no fill, it would vanish into the theme's background.
fn border_color(line: &BorderLine, filled: bool, t: &Tokens) -> Hsla {
    match line.color.map(hsla) {
        Some(c) if filled || contrast(c, t.bg) >= 1.8 => c,
        None if filled => hsla(0x000000),
        _ => t.fg,
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

/// The gridlines of a cell, as its own right / bottom border (no extra
/// elements: this runs for every visible cell).
pub(crate) fn gridlines(d: Div, look: &CellLook, t: &Tokens) -> Div {
    d.when(look.grid_right, |d| d.border_r(GRID_LINE))
        .when(look.grid_bottom, |d| d.border_b(GRID_LINE))
        .when(look.grid_right || look.grid_bottom, |d| {
            d.border_color(t.grid_line)
        })
}

/// The border edges of a cell (absolute, under the overlays).
pub(crate) fn borders(look: &CellLook, t: &Tokens) -> impl Iterator<Item = Div> {
    let filled = look.style.fill.is_some();
    [
        (Edge::Right, look.right),
        (Edge::Bottom, look.bottom),
        (Edge::Top, look.top),
        (Edge::Left, look.left),
    ]
    .into_iter()
    .filter_map(move |(edge, line)| {
        line.map(|line| border_edge(edge, &line, border_color(&line, filled, t)))
    })
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
/// size: `(offset_x, offset_y, width, height)`, at window scale `scale`.
/// Lengths are the grid's snapped ones: columns at [`column_width`], rows
/// where [`RowHeights`] puts them (whole device pixels), so the slices meet
/// over rows of different heights.
pub(crate) fn merge_geometry(
    book: &Workbook,
    sheet: u32,
    merge: CellRange,
    row: u32,
    col: u32,
    scale: f32,
    rows: &RowHeights,
) -> (Pixels, Pixels, Pixels, Pixels) {
    let width = |from: u32, to: u32| -> Pixels {
        (from..to)
            .map(|c| column_width(book, sheet, c, scale))
            .fold(px(0.), |a, b| a + b)
    };
    let top = rows.top_dp(merge.top as usize);
    let y = |r: u32| px(rows.to_px(rows.top_dp(r as usize) - top) as f32);
    (
        width(merge.left, col),
        y(row),
        width(merge.left, merge.right + 1),
        y(merge.bottom + 1),
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
    /// Repeat character from the number format's `*x` and its byte position
    /// in `text`: the text splits there and the character fills the gap, so
    /// `_("Rp"* #,##0_)` puts `Rp` at the left edge and the number at the right.
    pub fill: Option<(char, usize)>,
}

/// Repeat characters laid out for a fill gap (the rest is clipped).
const FILL_REPEAT: usize = 256;

impl Content<'_> {
    /// Lay the text out in `d` (which becomes a flex box); an empty cell
    /// gets no text element.
    pub(crate) fn apply(self, d: Div) -> Div {
        if self.text.is_empty() {
            return d;
        }
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
            .when(self.align == HAlign::Right, |d| d.text_right());
        let fill = self
            .fill
            .filter(|&(_, at)| !s.wrap && self.text.is_char_boundary(at));
        let text = match fill {
            // Like Excel, a fill ignores the alignment: the text spans the cell.
            Some((c, at)) => {
                let (left, right) = self.text.split_at(at);
                let gap = div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap();
                let gap = if c == ' ' {
                    gap
                } else {
                    gap.child(SharedString::from(c.to_string().repeat(FILL_REPEAT)))
                };
                text.flex()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(div().flex_none().child(SharedString::from(left.to_owned())))
                    .child(gap)
                    .child(
                        div()
                            .flex_none()
                            .child(SharedString::from(right.to_owned())),
                    )
            }
            None => text
                .map(|d| {
                    if s.wrap {
                        d.whitespace_normal().overflow_hidden()
                    } else {
                        d.truncate()
                    }
                })
                .child(self.text),
        };
        d.flex()
            .px(SPACE_3)
            .map(|d| match s.valign {
                VAlign::Top => d.items_start().pt(V_PAD.min(slack / 2.)),
                VAlign::Center => d.items_center(),
                // Wrapped text may fill the box (its row was fitted to it).
                VAlign::Bottom if s.wrap => d.items_end().pb((slack * 0.4).min(V_PAD)),
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

    /// Rows of the default height at `scale`, as the grid lays them out.
    fn uniform_rows(scale: f32) -> RowHeights {
        let dp = crate::row_heights::device_px(f32::from(row_height()), scale);
        RowHeights::uniform(100, dp, scale)
    }

    #[std::prelude::v1::test]
    fn merge_geometry_offsets() {
        let mut book = Workbook::new_empty().unwrap();
        book.set_column_width(0, 1, 1, 100.0).unwrap();
        book.set_column_width(0, 2, 2, 50.0).unwrap();
        let m = CellRange::from_corners((2, 1), (3, 3));
        let w3 = book.column_width(0, 3) as f32;
        let h = row_height();
        let rows = uniform_rows(1.0);
        assert_eq!(
            merge_geometry(&book, 0, m, 2, 1, 1.0, &rows),
            (px(0.), px(0.), px(150. + w3), h * 2.)
        );
        assert_eq!(
            merge_geometry(&book, 0, m, 3, 3, 1.0, &rows),
            (px(150.), h, px(150. + w3), h * 2.)
        );
    }

    #[std::prelude::v1::test]
    fn merge_slices_line_up_with_snapped_cells_at_125() {
        let mut book = Workbook::new_empty().unwrap();
        // Widths that are not whole device pixels at 125 %.
        book.set_column_width(0, 1, 1, 101.7).unwrap();
        book.set_column_width(0, 2, 2, 69.3).unwrap();
        book.set_column_width(0, 3, 3, 117.9).unwrap();
        let s = 1.25;
        // Rows of different heights under the merge, none of them whole
        // device pixels as given: 30, 47.3, 12.2 and 61 px.
        let dp = |px: f32| crate::row_heights::device_px(px, s);
        let mut heights = vec![dp(30.); 100];
        for (r, px) in [(2, 30.), (3, 47.3), (4, 12.2), (5, 61.)] {
            heights[r] = dp(px);
        }
        let rows = RowHeights::from_heights(heights, s);
        let m = CellRange::from_corners((2, 1), (5, 3));
        let (x, y, w, h) = merge_geometry(&book, 0, m, 4, 3, s, &rows);
        let cols = |a: u32, b: u32| {
            (a..b)
                .map(|c| column_width(&book, 0, c, s))
                .fold(px(0.), |a, b| a + b)
        };
        // Each slice's offset is where its cell sits in the grid, so the
        // pieces of the block meet exactly.
        assert_eq!(x, cols(1, 3));
        assert_eq!(w, cols(1, 4));
        let at = |r: usize| rows.top(r) - rows.top(2);
        assert!((f64::from(f32::from(y)) - at(4)).abs() < 1e-4);
        assert!((f64::from(f32::from(h)) - at(6)).abs() < 1e-4);
        for len in [x, y, w, h] {
            let dp = f32::from(len) * s;
            assert!(
                (dp - dp.round()).abs() < 1e-3,
                "{dp} is not whole device pixels"
            );
        }
        // The slice of each row starts where that row starts.
        for r in 2..=5 {
            let (_, y, _, _) = merge_geometry(&book, 0, m, r, 1, s, &rows);
            assert!((f64::from(f32::from(y)) - at(r as usize)).abs() < 1e-4);
        }
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
