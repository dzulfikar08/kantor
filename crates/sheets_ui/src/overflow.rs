//! Text overflowing into empty neighbours, as Excel and LibreOffice draw it.
//!
//! Text too wide for its cell spills over the empty cells beside it:
//! left-aligned (and General) text to the right, right-aligned text to the
//! left, centred text both ways. It stops at the first cell that shows
//! something (a value, a formula's result, even `""`), at a merged cell and
//! at the edge of frozen panes. Wrapped text, numbers (they show `###`),
//! booleans, errors, merged cells and repeat-fill formats never spill.
//!
//! The table clips every cell, so the text is drawn the way merged cells
//! are ([`crate::look`]): the source cell and every cell it spills over
//! each draw the same text box, shifted by their offset from the source,
//! and each clip shows its slice. Gridlines inside the spill are left out,
//! as in Excel. Spans are worked out once per row and workbook revision
//! ([`OverflowCache`]); an edit anywhere (a neighbour filled in, a column
//! resized) gives a new revision, so they are worked out again.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::{metrics::*, tokens};
use sheets_engine::{CellStyle, HAlign, ValueKind};

use crate::{SheetGrid, cond_paint, look};

/// Furthest a text spills, in columns (keeps a pathological row cheap).
const MAX_SPILL_COLUMNS: u32 = 256;

/// A text cell that may spill: its column, how wide its text is with the
/// cell's padding, and how it is aligned (`General` already resolved).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Candidate {
    pub col: u32,
    pub need: f32,
    pub align: HAlign,
}

/// Where one cell's text is drawn: over columns `first..=last`, in a box
/// at `x` (from the source cell's left edge) `w` wide.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Span {
    pub src: u32,
    pub first: u32,
    pub last: u32,
    pub x: f32,
    pub w: f32,
}

impl Span {
    /// Whether `col` draws (part of) this text.
    pub fn covers(&self, col: u32) -> bool {
        (self.first..=self.last).contains(&col)
    }

    /// The text box's left edge from `col`'s left edge, given the columns'
    /// widths.
    pub fn x_in(&self, col: u32, width: impl Fn(u32) -> f32) -> f32 {
        let offset: f32 = if col >= self.src {
            (self.src..col).map(&width).sum()
        } else {
            -(col..self.src).map(&width).sum::<f32>()
        };
        self.x - offset
    }

    /// Whether `col`'s right gridline is under the text (left out).
    pub fn hides_grid_right(&self, col: u32) -> bool {
        self.first <= col && col < self.last
    }
}

/// Where the text of `c` goes. `None` when it fits its cell. `blocked(col)`
/// is true for cells that stop a spill (they show something, or are
/// merged); `bounds` are the columns the spill may reach (the source's
/// side of frozen panes, the sheet's edge).
pub(crate) fn plan(
    c: Candidate,
    width: impl Fn(u32) -> f32,
    blocked: impl Fn(u32) -> bool,
    bounds: (u32, u32),
) -> Option<Span> {
    let own = width(c.col);
    if c.need <= own {
        return None;
    }
    let extra = c.need - own;
    // Spill one way while room is still needed: the last column reached.
    let reach = |mut room: f32, right: bool| -> u32 {
        let mut at = c.col;
        let mut steps = 0;
        while room > 0.0 && steps < MAX_SPILL_COLUMNS {
            let next = if right {
                match at.checked_add(1).filter(|n| *n <= bounds.1) {
                    Some(n) => n,
                    None => break,
                }
            } else {
                match at.checked_sub(1).filter(|n| *n >= bounds.0) {
                    Some(n) => n,
                    None => break,
                }
            };
            if blocked(next) {
                break;
            }
            room -= width(next);
            at = next;
            steps += 1;
        }
        at
    };
    let (first, last, x) = match c.align {
        HAlign::Right => (reach(extra, false), c.col, own - c.need),
        HAlign::Center => (
            reach(extra / 2.0, false),
            reach(extra / 2.0, true),
            (own - c.need) / 2.0,
        ),
        _ => (c.col, reach(extra, true), 0.0),
    };
    Some(Span {
        src: c.col,
        first,
        last,
        x,
        w: c.need,
    })
}

/// The spans of one row, from its text cells and the columns that show
/// something (sorted). `frozen` is the number of frozen columns: a spill
/// stays on its side of them.
pub(crate) fn plan_row(
    candidates: &[Candidate],
    filled: &[u32],
    width: impl Fn(u32) -> f32,
    merged: impl Fn(u32) -> bool,
    frozen: u32,
    last_col: u32,
) -> Vec<Span> {
    let blocked = |col: u32| filled.binary_search(&col).is_ok() || merged(col);
    candidates
        .iter()
        .filter_map(|&c| {
            let bounds = if frozen > 0 && c.col < frozen {
                (0, frozen - 1)
            } else {
                (frozen, last_col)
            };
            plan(c, &width, blocked, bounds)
        })
        .collect()
}

/// Spans per row for one state of the workbook.
#[derive(Default)]
pub(crate) struct OverflowCache {
    key: Option<OverflowKey>,
    rows: HashMap<u32, Rc<Vec<Span>>>,
}

/// What the spans depend on: the workbook's revision, the sheet and the
/// window's scale (column widths are snapped to it).
type OverflowKey = ((u64, u64), u32, u32);

impl OverflowCache {
    /// The spans of `row`, worked out by `compute` unless known for `key`.
    pub fn row(
        &mut self,
        revision: (u64, u64),
        sheet: u32,
        scale: f32,
        row: u32,
        compute: impl FnOnce() -> Vec<Span>,
    ) -> Rc<Vec<Span>> {
        let key = (revision, sheet, scale.to_bits());
        if self.key != Some(key) {
            self.key = Some(key);
            self.rows.clear();
        }
        self.rows
            .entry(row)
            .or_insert_with(|| Rc::new(compute()))
            .clone()
    }
}

/// A source cell's text as its neighbours draw it.
struct SpillText {
    text: SharedString,
    style: CellStyle,
    align: HAlign,
    color: Hsla,
    family: Option<SharedString>,
}

impl SheetGrid {
    /// A column's width as the grid lays it out, in px.
    pub(crate) fn spill_width(&self, col: u32) -> f32 {
        f32::from(look::column_width(&self.book, self.sheet, col, self.scale))
    }

    /// The text of (`row`, `col`) if it is the kind that may spill: text
    /// (not a number, boolean or error) in a cell that isn't merged,
    /// wrapped, filled with a repeat character, a picture or empty.
    fn spill_text(&mut self, row: u32, col: u32, cx: &App) -> Option<SpillText> {
        let sheet = self.sheet;
        let look = self.book.cell_look(sheet, row, col);
        if look.merge.is_some()
            || self.book.value_kind(sheet, row, col) != ValueKind::Text
            || self.book.cell_image(sheet, row, col).is_some()
        {
            return None;
        }
        let shown = self.book.display(sheet, row, col);
        let cond = self.book.conditional(sheet, row, col);
        if cond.as_ref().is_some_and(|c| !c.show_value) {
            return None;
        }
        let shown = cond
            .as_ref()
            .and_then(|c| c.display.clone())
            .unwrap_or(shown);
        if shown.text.is_empty() || shown.fill.is_some() {
            return None;
        }
        let mut style = cond
            .as_ref()
            .and_then(|c| c.style.as_deref())
            .unwrap_or(&look.style)
            .clone();
        if let Some(tl) = self.book.table_cell_look(sheet, row, col) {
            style = crate::table_style(&style, &tl);
        }
        if style.wrap {
            return None;
        }
        let align = self.book.display_align_of(&style, sheet, row, col);
        if !matches!(align, HAlign::Left | HAlign::Center | HAlign::Right) {
            return None;
        }
        let t = tokens(cx);
        let color = look::value_color(
            &cond_paint::text_backdrop(&style, cond.as_deref()),
            shown.color,
            &t,
        );
        let family = self.fonts.for_style(&style, cx);
        Some(SpillText {
            text: SharedString::from(shown.text),
            style,
            align,
            color,
            family,
        })
    }

    /// `###` filling a cell `width` wide when `text`, a number's display,
    /// doesn't fit in it (Excel never cuts a number short); `None` when it
    /// fits or the cell isn't a number.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn number_hashes(
        &mut self,
        text: &str,
        (sheet, row, col): (u32, u32, u32),
        style: &CellStyle,
        width: f32,
        height: Pixels,
        window: &mut Window,
        cx: &App,
    ) -> Option<String> {
        if self.book.value_kind(sheet, row, col) != ValueKind::Number {
            return None;
        }
        let size = look::text_size(style, self.book.default_font_size(), FONT_BODY, height);
        let room = width - f32::from(SPACE_3) * 2.0;
        // Digits are about half an em wide: short numbers fit unmeasured.
        if text.chars().count() as f32 * f32::from(size) * 0.45 <= room {
            return None;
        }
        let mut run = window.text_style().to_run(text.len());
        if let Some(family) = self.fonts.for_style(style, cx) {
            run.font.family = family;
        }
        if style.bold {
            run.font.weight = FontWeight::BOLD;
        }
        if style.italic {
            run.font.style = FontStyle::Italic;
        }
        let shape = |w: &mut Window, s: String| {
            w.text_system()
                .shape_line(s.into(), size, std::slice::from_ref(&run), None)
                .width
        };
        if f32::from(shape(window, text.to_owned())) <= room {
            return None;
        }
        let hash = f32::from(shape(window, "#".to_owned())).max(1.0);
        Some("#".repeat(((room / hash).floor() as usize).max(1)))
    }

    /// The spans of `row` on the shown sheet (worked out once per state).
    pub(crate) fn overflow_spans(
        &mut self,
        row: u32,
        window: &mut Window,
        cx: &App,
    ) -> Rc<Vec<Span>> {
        let (revision, sheet, scale) = (self.book.revision(), self.sheet, self.scale);
        let mut cache = std::mem::take(&mut self.overflow);
        let spans = cache.row(revision, sheet, scale, row, || {
            self.compute_spans(row, window, cx)
        });
        self.overflow = cache;
        spans
    }

    fn compute_spans(&mut self, row: u32, window: &mut Window, cx: &App) -> Vec<Span> {
        let sheet = self.sheet;
        let filled = self.book.content_columns_in_row(sheet, row);
        if filled.is_empty() {
            return Vec::new();
        }
        let height = px(self.layout.heights.borrow().height(row as usize) as f32);
        let default_font_size = self.book.default_font_size();
        let base = window.text_style();
        let mut candidates = Vec::new();
        for &col in &filled {
            let own = self.spill_width(col);
            let Some(text) = self.spill_text(row, col, cx) else {
                continue;
            };
            let size = look::text_size(&text.style, default_font_size, FONT_BODY, height);
            // Padding, and the indent the cell draws before (or after) it.
            let indent = if text.align == HAlign::Center {
                0.0
            } else {
                f32::from(FONT_BODY) * 1.5 * text.style.indent.min(250) as f32
            };
            let pad = f32::from(SPACE_3) * 2.0 + indent;
            // No glyph is wider than its size: short text fits unmeasured.
            if text.text.chars().count() as f32 * f32::from(size) + pad <= own {
                continue;
            }
            let mut run = base.to_run(text.text.len());
            if let Some(family) = text.family.clone() {
                run.font.family = family;
            }
            if text.style.bold {
                run.font.weight = FontWeight::BOLD;
            }
            if text.style.italic {
                run.font.style = FontStyle::Italic;
            }
            let width = window
                .text_system()
                .shape_line(text.text.clone(), size, &[run], None)
                .width;
            candidates.push(Candidate {
                col,
                // A pixel to spare, so rounding never clips the last glyph.
                need: f32::from(width).ceil() + pad + 1.0,
                align: text.align,
            });
        }
        if candidates.is_empty() {
            return Vec::new();
        }
        let frozen = self.book.frozen(sheet).1;
        let book = &self.book;
        let merged = |col: u32| book.merge_at(sheet, row, col).is_some();
        plan_row(
            &candidates,
            &filled,
            |c| self.spill_width(c),
            merged,
            frozen,
            sheets_engine::MAX_COLS - 1,
        )
    }

    /// The text of span `s`'s source drawn over `col` of `row`.
    pub(crate) fn spill_layer(
        &mut self,
        row: u32,
        s: Span,
        col: u32,
        height: Pixels,
        cx: &App,
    ) -> Option<Div> {
        let text = self.spill_text(row, s.src, cx)?;
        let x = s.x_in(col, |c| self.spill_width(c));
        let content = look::Content {
            text: text.text,
            style: &text.style,
            align: text.align,
            color: text.color,
            base_size: FONT_BODY,
            default_font_size: self.book.default_font_size(),
            family: text.family,
            height,
            fill: None,
            spill: true,
        };
        Some(content.apply(div().absolute().top_0().left(px(x)).w(px(s.w)).h(height)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(_: u32) -> f32 {
        100.0
    }

    fn cand(col: u32, need: f32, align: HAlign) -> Candidate {
        Candidate { col, need, align }
    }

    #[std::prelude::v1::test]
    fn left_text_spills_right_until_a_filled_cell() {
        // A1 holds a 350 px title; B1:D1 are empty, E1 has a value.
        let spans = plan_row(
            &[cand(0, 350.0, HAlign::Left)],
            &[0, 4],
            w,
            |_| false,
            0,
            100,
        );
        assert_eq!(
            spans,
            vec![Span {
                src: 0,
                first: 0,
                last: 3,
                x: 0.0,
                w: 350.0
            }]
        );
        // C1 filled: it stops at B1 (clipped there).
        let spans = plan_row(
            &[cand(0, 350.0, HAlign::Left)],
            &[0, 2],
            w,
            |_| false,
            0,
            100,
        );
        assert_eq!(spans[0].last, 1);
        // B1 filled: nowhere to go, the text is clipped in its own cell.
        let spans = plan_row(
            &[cand(0, 350.0, HAlign::Left)],
            &[0, 1],
            w,
            |_| false,
            0,
            100,
        );
        assert_eq!((spans[0].first, spans[0].last), (0, 0));
        // Fits: no span.
        assert!(plan_row(&[cand(0, 90.0, HAlign::Left)], &[0], w, |_| false, 0, 100).is_empty());
    }

    #[std::prelude::v1::test]
    fn right_and_centred_text() {
        // Right-aligned text in D1 spills left over C1 and B1.
        let s = plan(cand(3, 250.0, HAlign::Right), w, |_| false, (0, 100)).unwrap();
        assert_eq!((s.first, s.last, s.x), (1, 3, -150.0));
        // Right-aligned text in A1 has no room on the left.
        let s = plan(cand(0, 250.0, HAlign::Right), w, |_| false, (0, 100)).unwrap();
        assert_eq!((s.first, s.last), (0, 0));
        // Centred text in C1 spills both ways, half each.
        let s = plan(cand(2, 300.0, HAlign::Center), w, |_| false, (0, 100)).unwrap();
        assert_eq!((s.first, s.last, s.x), (1, 3, -100.0));
        // ... and is cut on a side that is blocked.
        let s = plan(cand(2, 300.0, HAlign::Center), w, |c| c == 3, (0, 100)).unwrap();
        assert_eq!((s.first, s.last), (1, 2));
    }

    #[std::prelude::v1::test]
    fn merged_cells_frozen_panes_and_hidden_columns_stop_or_pass() {
        // A merged cell at C1 stops the spill.
        let spans = plan_row(&[cand(0, 350.0, HAlign::Left)], &[0], w, |c| c == 2, 0, 100);
        assert_eq!(spans[0].last, 1);
        // Two frozen columns: A1's text stays in A:B.
        let spans = plan_row(&[cand(0, 350.0, HAlign::Left)], &[0], w, |_| false, 2, 100);
        assert_eq!(spans[0].last, 1);
        // Right-aligned text in C1 (scrolling side) can't spill into B.
        let spans = plan_row(&[cand(2, 250.0, HAlign::Right)], &[2], w, |_| false, 2, 100);
        assert_eq!(spans[0].first, 2);
        // A hidden (zero-width) column is passed over.
        let hidden = |c: u32| if c == 1 { 0.0 } else { 100.0 };
        let s = plan(cand(0, 250.0, HAlign::Left), hidden, |_| false, (0, 100)).unwrap();
        assert_eq!(s.last, 3);
        // The sheet's last column ends it.
        let s = plan(cand(99, 350.0, HAlign::Left), w, |_| false, (0, 100)).unwrap();
        assert_eq!(s.last, 100);
    }

    #[std::prelude::v1::test]
    fn each_cell_draws_its_slice_and_hides_inner_gridlines() {
        let s = Span {
            src: 2,
            first: 1,
            last: 4,
            x: -50.0,
            w: 400.0,
        };
        assert_eq!(s.x_in(2, w), -50.0);
        assert_eq!(s.x_in(3, w), -150.0);
        assert_eq!(s.x_in(1, w), 50.0);
        assert!(s.covers(1) && s.covers(4) && !s.covers(5) && !s.covers(0));
        assert!(s.hides_grid_right(1) && s.hides_grid_right(3) && !s.hides_grid_right(4));
    }

    #[std::prelude::v1::test]
    fn cache_follows_the_revision() {
        let mut cache = OverflowCache::default();
        let span = Span {
            src: 0,
            first: 0,
            last: 1,
            x: 0.0,
            w: 150.0,
        };
        let a = cache.row((1, 0), 0, 1.0, 0, || vec![span]);
        assert_eq!(a.len(), 1);
        // Same state: not worked out again.
        let b = cache.row((1, 0), 0, 1.0, 0, || unreachable!());
        assert_eq!(b.len(), 1);
        // A neighbour was filled in: a new revision, worked out again.
        let c = cache.row((2, 0), 0, 1.0, 0, Vec::new);
        assert!(c.is_empty());
    }
}
