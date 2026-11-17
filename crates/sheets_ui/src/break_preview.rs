//! Page Break Preview (View ▸ Page Break Preview), as in Excel: the pages a
//! sheet prints on are drawn over its cells, each with a large "Page N"
//! watermark, cells that don't print are greyed, the printed range has a
//! blue outline, automatic page breaks are dashed blue lines and manual ones
//! solid. Dragging a break moves it (an automatic break becomes manual,
//! one dragged off the range goes); dragging the outline sets the print
//! area. The pages come from the same plan as printing
//! ([`sheets_pdf::break_preview`]), worked out again when the workbook
//! changes.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::*;
use kantor_ui::i18n::{self, tf, tk};
use sheets_engine::CellRange;
use sheets_engine::drawing::SheetGeometry as _;
use sheets_pdf::Shaper;
use sheets_pdf::break_preview::BreakMap;

use crate::drawings::{GridGeometry, Pane, grid_panes, screen_rect};
use crate::table::TableState;
use crate::{SheetGrid, SheetsView};

/// Excel's page break blue.
const BLUE: u32 = 0x2F5BEA;
/// Width of the lines and of the strip around them that takes a drag, px.
const LINE: f32 = 2.5;
const GRAB: f32 = 4.0;

/// What a drag moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Grab {
    /// The break starting a page at `at` in area `area`, along rows
    /// (`rows`: a horizontal line) or columns.
    Break {
        area: usize,
        rows: bool,
        at: u32,
        manual: bool,
    },
    /// An edge of area `area`: 0 top, 1 bottom, 2 left, 3 right.
    Edge { area: usize, side: u8 },
}

impl Grab {
    fn horizontal(self) -> bool {
        match self {
            Grab::Break { rows, .. } => rows,
            Grab::Edge { side, .. } => side < 2,
        }
    }
}

/// A drag in progress: what, and the row or column boundary it is over
/// now (a page would start there).
#[derive(Clone, Copy, Debug)]
pub(crate) struct BreakDrag {
    grab: Grab,
    to: u32,
}

/// `(workbook revision, sheet)`.
type MapKey = ((u64, u64), u32);

/// The view's state.
#[derive(Default)]
pub(crate) struct BreakLayer {
    /// Built on first use: it scans the system's fonts.
    shaper: Option<Shaper>,
    map: Option<(MapKey, Rc<BreakMap>)>,
    drag: Option<BreakDrag>,
}

impl BreakLayer {
    fn map(&mut self, grid: &SheetGrid) -> Rc<BreakMap> {
        let key = (grid.book.revision(), grid.sheet);
        if let Some((k, m)) = &self.map
            && *k == key
        {
            return m.clone();
        }
        let shaper = self.shaper.get_or_insert_with(Shaper::new);
        let map = Rc::new(BreakMap::new(&grid.book, grid.sheet, shaper));
        self.map = Some((key, map.clone()));
        map
    }
}

/// The boundary nearest sheet position `v` along rows (`rows`) or
/// columns: the index of the row or column starting there.
fn boundary(geom: &GridGeometry, rows: bool, v: f64) -> u32 {
    if rows {
        let r = geom.row_at(v.max(0.0));
        if v - geom.row_top(r) > geom.row_height(r) / 2.0 {
            r + 1
        } else {
            r
        }
    } else {
        let c = geom.col_at(v.max(0.0));
        if v - geom.col_left(c) > geom.col_width(c) / 2.0 {
            c + 1
        } else {
            c
        }
    }
}

/// Sheet px of the edges of `range`: left, top, right, bottom.
fn range_px(geom: &GridGeometry, r: &CellRange) -> [f64; 4] {
    [
        geom.col_left(r.left),
        geom.row_top(r.top),
        geom.col_left(r.right + 1),
        geom.row_top(r.bottom + 1),
    ]
}

/// One line to draw: from, to (screen), dashed, in which pane.
struct Seg {
    a: Point<Pixels>,
    b: Point<Pixels>,
    dashed: bool,
    clip: Bounds<Pixels>,
}

/// What prepaint hands paint.
pub(crate) struct Frame {
    greys: Vec<Bounds<Pixels>>,
    marks: Vec<(Bounds<Pixels>, Bounds<Pixels>, usize)>,
    segs: Vec<Seg>,
    hits: Vec<(Grab, Hitbox)>,
    /// Pane origins (to turn the pointer into sheet px while dragging).
    panes: Vec<Pane>,
}

/// The new print area after edge `side` of `r` is dragged to boundary `to`.
pub(crate) fn dragged_area(r: CellRange, side: u8, to: u32) -> CellRange {
    let mut r = r;
    match side {
        0 => r.top = to.min(r.bottom),
        1 => r.bottom = to.saturating_sub(1).max(r.top),
        2 => r.left = to.min(r.right),
        _ => r.right = to.saturating_sub(1).max(r.left),
    }
    r
}

/// What dropping a break at `to` does: `(from, to)` for
/// [`sheets_engine::Workbook::move_page_break`] (`from` only for a manual
/// break), or `None` when nothing changes.
pub(crate) fn dropped_break(
    range: CellRange,
    rows: bool,
    at: u32,
    manual: bool,
    to: u32,
) -> Option<(Option<u32>, Option<u32>)> {
    let (first, last) = if rows {
        (range.top, range.bottom)
    } else {
        (range.left, range.right)
    };
    let from = manual.then_some(at);
    if to <= first || to > last {
        // Off the printed range: a manual break goes, an automatic one
        // stays where the page size puts it.
        return from.map(|f| (Some(f), None));
    }
    if to == at && manual {
        return None;
    }
    Some((from, Some(to)))
}

impl SheetsView {
    /// Whether the sheet on screen is in Page Break Preview.
    pub(crate) fn in_break_preview(&self, cx: &App) -> bool {
        let grid = self.table.read(cx).delegate();
        grid.book.break_preview(grid.sheet)
    }

    /// View ▸ Page Break Preview (`on`) or Normal, for the sheet on screen.
    pub fn set_break_preview(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
            grid.book.set_break_preview(sheet, on);
            table.refresh(cx);
        });
        self.break_layer.borrow_mut().drag = None;
        self.status = if on {
            i18n::t("Page Break Preview").into()
        } else {
            i18n::t("Normal view").into()
        };
        self.update_title(window, cx);
        cx.notify();
    }

    /// Normal and Page Break Preview buttons at the right of the sheet
    /// tabs, as Excel has them in its status bar.
    pub(crate) fn render_view_buttons(&self, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::assets::IconName as Lucide;
        use gpui_kit::component::{Sizable as _, h_flex};
        let on = self.in_break_preview(cx);
        let button = |id: &'static str, icon: Lucide, label: &'static str, pbp: bool| {
            kantor_ui::components::icon_button(id, icon, on == pbp, cx)
                .xsmall()
                .tooltip(i18n::t(label))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.set_break_preview(pbp, window, cx)),
                )
        };
        h_flex()
            .flex_none()
            .gap_1()
            .child(button("view-normal", Lucide::Grid3x3, tk("Normal"), false))
            .child(button(
                "view-page-break",
                Lucide::SquareDashed,
                tk("Page Break Preview"),
                true,
            ))
    }

    /// The layer over the grid in Page Break Preview.
    pub(crate) fn render_break_preview(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.in_break_preview(cx) {
            return None;
        }
        let layer = self.break_layer.clone();
        let table = self.table.clone();
        let view = cx.entity().downgrade();
        let interactive = self.editing.is_none();
        let paint_layer = layer.clone();
        let t = kantor_ui::tokens(cx);
        Some(
            canvas(
                move |_, window, cx| prepaint(&layer, &table, interactive, window, cx),
                move |_, frame, window, cx| {
                    let Some(frame) = frame else { return };
                    paint(&paint_layer, frame, &t, view.clone(), window, cx);
                },
            )
            .absolute()
            .inset_0()
            .into_any_element(),
        )
    }

    /// The end of a drag: move the break or set the print area (one undo
    /// step).
    fn drop_break(&mut self, drag: BreakDrag, window: &mut Window, cx: &mut Context<Self>) {
        let map = self.break_layer.borrow().map.as_ref().map(|m| m.1.clone());
        let Some(map) = map else { return };
        let res = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
            let r = match drag.grab {
                Grab::Break {
                    area,
                    rows,
                    at,
                    manual,
                } => {
                    let Some(a) = map.areas.get(area) else {
                        return Ok(None);
                    };
                    match dropped_break(a.range, rows, at, manual, drag.to) {
                        Some((from, to)) => {
                            grid.book.move_page_break(sheet, rows, from, to).map(|()| {
                                Some(if to.is_some() {
                                    tk("Page break moved")
                                } else {
                                    tk("Page break removed")
                                })
                            })
                        }
                        None => Ok(None),
                    }
                }
                Grab::Edge { area, side } => {
                    let mut areas: Vec<CellRange> = map.areas.iter().map(|a| a.range).collect();
                    let Some(a) = areas.get_mut(area) else {
                        return Ok(None);
                    };
                    let new = dragged_area(*a, side, drag.to);
                    if new == *a {
                        return Ok(None);
                    }
                    *a = new;
                    grid.book
                        .set_print_area(sheet, areas)
                        .map(|()| Some(tk("Print area changed")))
                }
            };
            table.refresh(cx);
            r
        });
        match res {
            Ok(Some(msg)) => self.status = i18n::t(msg).into(),
            Ok(None) => {}
            Err(e) => self.status = format!("{e:#}").into(),
        }
        self.update_title(window, cx);
        cx.notify();
    }
}

fn prepaint(
    layer: &Rc<RefCell<BreakLayer>>,
    table: &Entity<TableState<SheetGrid>>,
    interactive: bool,
    window: &mut Window,
    cx: &mut App,
) -> Option<Frame> {
    let table = table.read(cx);
    let grid = table.delegate();
    let map = layer.borrow_mut().map(grid);
    let drag = layer.borrow().drag;
    let heights = grid.layout.heights.clone();
    let rows = heights.borrow();
    let max_col = map
        .areas
        .iter()
        .map(|a| a.range.right + 2)
        .max()
        .unwrap_or(0);
    let geom = GridGeometry::new(grid, Some(table), &rows, max_col);
    let (panes, body) = grid_panes(table, &geom)?;
    let mut frame = Frame {
        greys: Vec::new(),
        marks: Vec::new(),
        segs: Vec::new(),
        hits: Vec::new(),
        panes: panes.clone(),
    };
    let areas: Vec<[f64; 4]> = map
        .areas
        .iter()
        .map(|a| range_px(&geom, &a.range))
        .collect();
    for pane in &panes {
        let clip = pane.clip.intersect(&body);
        if clip.size.width <= px(0.) || clip.size.height <= px(0.) {
            continue;
        }
        // The pane's part of the sheet, in sheet px.
        let vis = [
            f64::from(f32::from(clip.left() - pane.origin_x)),
            f64::from(f32::from(clip.top() - pane.origin_y)),
            f64::from(f32::from(clip.right() - pane.origin_x)),
            f64::from(f32::from(clip.bottom() - pane.origin_y)),
        ];
        // Grey outside the printed ranges: cut the pane at every range edge
        // and fill the pieces no range covers.
        let mut xs = vec![vis[0], vis[2]];
        let mut ys = vec![vis[1], vis[3]];
        for a in &areas {
            xs.extend([a[0], a[2]].iter().filter(|&&x| x > vis[0] && x < vis[2]));
            ys.extend([a[1], a[3]].iter().filter(|&&y| y > vis[1] && y < vis[3]));
        }
        xs.sort_by(f64::total_cmp);
        xs.dedup();
        ys.sort_by(f64::total_cmp);
        ys.dedup();
        for y in ys.windows(2) {
            for x in xs.windows(2) {
                let (cx_, cy_) = ((x[0] + x[1]) / 2.0, (y[0] + y[1]) / 2.0);
                let inside = areas
                    .iter()
                    .any(|a| cx_ >= a[0] && cx_ < a[2] && cy_ >= a[1] && cy_ < a[3]);
                if !inside {
                    frame.greys.push(
                        screen_rect(pane, [x[0], y[0], x[1] - x[0], y[1] - y[0]]).intersect(&clip),
                    );
                }
            }
        }
        // Each page's watermark, and the lines.
        let line = |a: (f64, f64), b: (f64, f64)| {
            (
                point(
                    pane.origin_x + px(a.0 as f32),
                    pane.origin_y + px(a.1 as f32),
                ),
                point(
                    pane.origin_x + px(b.0 as f32),
                    pane.origin_y + px(b.1 as f32),
                ),
            )
        };
        for (ai, a) in map.areas.iter().enumerate() {
            let [l, t, r, b] = areas[ai];
            for &(c, rw, n) in &a.pages {
                let (c0, c1) = a.cols[c];
                let (r0, r1) = a.rows[rw];
                let rect = [
                    geom.col_left(c0),
                    geom.row_top(r0),
                    geom.col_left(c1 + 1) - geom.col_left(c0),
                    geom.row_top(r1 + 1) - geom.row_top(r0),
                ];
                let s = screen_rect(pane, rect);
                if s.intersects(&clip) {
                    frame.marks.push((s, clip, n));
                }
            }
            let dragged = |g: Grab| drag.is_some_and(|d| d.grab == g);
            // The breaks.
            for rows in [true, false] {
                for br in map.breaks(ai, rows) {
                    let grab = Grab::Break {
                        area: ai,
                        rows,
                        at: br.at,
                        manual: br.manual,
                    };
                    if dragged(grab) {
                        continue;
                    }
                    let (p, q) = if rows {
                        let y = geom.row_top(br.at);
                        line((l, y), (r, y))
                    } else {
                        let x = geom.col_left(br.at);
                        line((x, t), (x, b))
                    };
                    frame.segs.push(Seg {
                        a: p,
                        b: q,
                        dashed: !br.manual,
                        clip,
                    });
                    hit(&mut frame.hits, grab, p, q, clip, interactive, window);
                }
            }
            // The outline.
            let sides = [
                ((l, t), (r, t)),
                ((l, b), (r, b)),
                ((l, t), (l, b)),
                ((r, t), (r, b)),
            ];
            for (side, (p, q)) in sides.into_iter().enumerate() {
                let grab = Grab::Edge {
                    area: ai,
                    side: side as u8,
                };
                if dragged(grab) {
                    continue;
                }
                let (p, q) = line(p, q);
                frame.segs.push(Seg {
                    a: p,
                    b: q,
                    dashed: false,
                    clip,
                });
                hit(&mut frame.hits, grab, p, q, clip, interactive, window);
            }
        }
        // The line being dragged, where it would go.
        if let Some(d) = drag {
            let ai = match d.grab {
                Grab::Break { area, .. } | Grab::Edge { area, .. } => area,
            };
            if let Some(&[l, t, r, b]) = areas.get(ai) {
                let (p, q) = if d.grab.horizontal() {
                    let y = geom.row_top(d.to);
                    line((l, y), (r, y))
                } else {
                    let x = geom.col_left(d.to);
                    line((x, t), (x, b))
                };
                frame.segs.push(Seg {
                    a: p,
                    b: q,
                    dashed: false,
                    clip,
                });
            }
        }
    }
    Some(frame)
}

/// A hitbox along a line, for dragging it.
fn hit(
    hits: &mut Vec<(Grab, Hitbox)>,
    grab: Grab,
    a: Point<Pixels>,
    b: Point<Pixels>,
    clip: Bounds<Pixels>,
    interactive: bool,
    window: &mut Window,
) {
    if !interactive {
        return;
    }
    let r = Bounds::from_corners(
        point(a.x.min(b.x) - px(GRAB), a.y.min(b.y) - px(GRAB)),
        point(a.x.max(b.x) + px(GRAB), a.y.max(b.y) + px(GRAB)),
    )
    .intersect(&clip);
    if r.size.width > px(0.) && r.size.height > px(0.) {
        hits.push((
            grab,
            window.insert_hitbox(r, HitboxBehavior::BlockMouseExceptScroll),
        ));
    }
}

fn paint(
    layer: &Rc<RefCell<BreakLayer>>,
    frame: Frame,
    t: &kantor_ui::Tokens,
    view: WeakEntity<SheetsView>,
    window: &mut Window,
    cx: &mut App,
) {
    let grey = if t.dark {
        hsla(0., 0., 0., 0.45)
    } else {
        hsla(0., 0., 0.55, 0.35)
    };
    for g in &frame.greys {
        window.paint_quad(fill(*g, grey));
    }
    // "Page N", large and pale, in the middle of each page.
    let mark = if t.dark {
        hsla(0., 0., 1., 0.16)
    } else {
        hsla(0., 0., 0.35, 0.22)
    };
    for (r, clip, n) in &frame.marks {
        let text: SharedString = tf("Page {}", &[n]).into();
        let size = (f32::from(r.size.height) / 4.0)
            .min(f32::from(r.size.width) / 5.0)
            .clamp(10.0, 120.0);
        let mut style = window.text_style();
        style.font_weight = FontWeight::BOLD;
        let run = TextRun {
            color: mark,
            ..style.to_run(text.len())
        };
        let line = window
            .text_system()
            .shape_line(text, px(size), &[run], None);
        let at = point(
            r.origin.x + (r.size.width - line.width) / 2.,
            r.origin.y + (r.size.height - px(size * 1.2)) / 2.,
        );
        window.with_content_mask(Some(ContentMask { bounds: *clip }), |window| {
            let _ = line.paint(at, px(size * 1.2), TextAlign::Left, None, window, cx);
        });
    }
    let blue = rgb(BLUE);
    for s in &frame.segs {
        window.with_content_mask(Some(ContentMask { bounds: s.clip }), |window| {
            let horizontal = s.a.y == s.b.y;
            let len = if horizontal {
                f32::from(s.b.x - s.a.x)
            } else {
                f32::from(s.b.y - s.a.y)
            };
            let (dash, gap) = if s.dashed { (6.0, 4.0) } else { (len, 0.0) };
            let mut at = 0.0;
            while at < len {
                let d = dash.min(len - at);
                let b = if horizontal {
                    Bounds::new(
                        point(s.a.x + px(at), s.a.y - px(LINE / 2.)),
                        size(px(d), px(LINE)),
                    )
                } else {
                    Bounds::new(
                        point(s.a.x - px(LINE / 2.), s.a.y + px(at)),
                        size(px(LINE), px(d)),
                    )
                };
                window.paint_quad(fill(b, blue));
                at += dash + gap;
                if dash <= 0.0 {
                    break;
                }
            }
        });
    }
    for (g, hb) in &frame.hits {
        let style = if g.horizontal() {
            CursorStyle::ResizeUpDown
        } else {
            CursorStyle::ResizeLeftRight
        };
        window.set_cursor_style(style, hb);
    }
    // Press on a line: start dragging it.
    let hits: Vec<(Grab, Hitbox)> = frame.hits;
    let panes = frame.panes;
    {
        let layer = layer.clone();
        let view = view.clone();
        let panes = panes.clone();
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || e.button != MouseButton::Left {
                return;
            }
            let Some((grab, _)) = hits.iter().rev().find(|(_, hb)| hb.is_hovered(window)) else {
                return;
            };
            let to = match *grab {
                Grab::Break { at, .. } => at,
                Grab::Edge { .. } => 0,
            };
            layer.borrow_mut().drag = Some(BreakDrag { grab: *grab, to });
            cx.stop_propagation();
            let _ = view.update(cx, |this, cx| {
                this.break_drag_panes = panes.clone();
                this.move_break_drag(e.position, cx);
            });
        });
    }
    if layer.borrow().drag.is_some() {
        let view = view.clone();
        let panes = panes.clone();
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
            if phase != DispatchPhase::Capture {
                return;
            }
            cx.stop_propagation();
            let _ = view.update(cx, |this, cx| {
                this.break_drag_panes = panes.clone();
                this.move_break_drag(e.position, cx);
            });
        });
    }
    let layer = layer.clone();
    window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
        if phase != DispatchPhase::Capture || e.button != MouseButton::Left {
            return;
        }
        if layer.borrow().drag.is_none() {
            return;
        }
        let _ = view.update(cx, |this, cx| {
            this.break_drag_panes = panes.clone();
            this.move_break_drag(e.position, cx);
            let drag = this.break_layer.borrow_mut().drag.take();
            if let Some(d) = drag {
                this.drop_break(d, window, cx);
            }
        });
    });
}

impl SheetsView {
    /// The pointer moved while dragging a line: snap it to the nearest row
    /// or column boundary.
    fn move_break_drag(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = self.break_layer.borrow().drag else {
            return;
        };
        let table = self.table.read(cx);
        let grid = table.delegate();
        let heights = grid.layout.heights.clone();
        let rows = heights.borrow();
        let probe = GridGeometry::new(grid, Some(table), &rows, 0);
        let pane = self
            .break_drag_panes
            .iter()
            .find(|p| p.clip.contains(&at))
            .or(self.break_drag_panes.last())
            .copied();
        let Some(pane) = pane else { return };
        let (x, y) = (
            f64::from(f32::from(at.x - pane.origin_x)),
            f64::from(f32::from(at.y - pane.origin_y)),
        );
        let horizontal = drag.grab.horizontal();
        let to = if horizontal {
            boundary(&probe, true, y)
        } else {
            let geom = GridGeometry::new(grid, Some(table), &rows, probe.col_at(x.max(0.0)) + 2);
            boundary(&geom, false, x)
        };
        drop(rows);
        if to != drag.to {
            if let Some(d) = self.break_layer.borrow_mut().drag.as_mut() {
                d.to = to;
            }
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn drops() {
        let r = CellRange::from_corners((0, 0), (99, 5));
        // A manual break moved.
        assert_eq!(
            dropped_break(r, true, 40, true, 30),
            Some((Some(40), Some(30)))
        );
        // An automatic one made manual where dropped.
        assert_eq!(
            dropped_break(r, true, 47, false, 30),
            Some((None, Some(30)))
        );
        // Dropped where it was: nothing for a manual break.
        assert_eq!(dropped_break(r, true, 40, true, 40), None);
        // Off the range: a manual break goes, an automatic one stays.
        assert_eq!(dropped_break(r, true, 40, true, 0), Some((Some(40), None)));
        assert_eq!(
            dropped_break(r, true, 40, true, 100),
            Some((Some(40), None))
        );
        assert_eq!(dropped_break(r, false, 3, false, 6), None);
    }

    #[std::prelude::v1::test]
    fn edges() {
        let r = CellRange::from_corners((2, 1), (20, 4));
        assert_eq!(
            dragged_area(r, 1, 31),
            CellRange::from_corners((2, 1), (30, 4))
        );
        assert_eq!(
            dragged_area(r, 0, 0),
            CellRange::from_corners((0, 1), (20, 4))
        );
        assert_eq!(
            dragged_area(r, 3, 3),
            CellRange::from_corners((2, 1), (20, 2))
        );
        // Never past the other edge.
        assert_eq!(
            dragged_area(r, 2, 9),
            CellRange::from_corners((2, 4), (20, 4))
        );
        assert_eq!(
            dragged_area(r, 1, 0),
            CellRange::from_corners((2, 1), (2, 4))
        );
    }
}
