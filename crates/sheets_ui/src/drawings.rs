//! Charts and pictures over the grid, as in Excel.
//!
//! Each object of the sheet ([`sheets_engine::drawing`]) is placed from its
//! anchor on the grid as laid out now ([`GridGeometry`]: column widths and
//! the per-row heights' prefix sums), so it moves with scrolling, row and
//! column resizing and inserted rows, and follows the window's scale. With
//! frozen columns it is drawn in each pane it reaches, clipped to that pane
//! (the frozen pane doesn't scroll sideways).
//!
//! Charts and pictures are built by `sheets_scene` (the Slides chart
//! renderer with the chart's cells read live, cached per object, size and
//! data) and painted by Slides' scene painter, also cached per built scene:
//! a frame of scrolling only positions and paints what is already built.
//! Objects off screen are skipped.
//!
//! Interaction: clicking an object selects it (accent outline and eight
//! handles), dragging moves it, a handle resizes it, Delete or Backspace
//! removes it and Escape lets go. Each move, resize or delete is one undo
//! step, written back to the anchor on save. Objects Kantor can't draw
//! (shapes, text boxes, unusual chart types) show as a placeholder box and
//! can be moved too. While a cell is being edited, objects don't take
//! clicks (so pointing at cells under them works).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::*;
use sheets_engine::drawing::{Drawing, DrawingKey, SheetGeometry};
use sheets_engine::{MAX_COLS, Workbook};
use sheets_scene::DrawingScenes;
use slides_scene::Item;
use slides_ui::{Painted, Painter, SceneView, paint_items};

use crate::row_heights::RowHeights;
use crate::table::TableState;
use crate::{SheetGrid, SheetsView, look};
use kantor_ui::i18n;
use kantor_ui::metrics::ROW_GUTTER_WIDTH;

/// Handle squares, logical px.
const HANDLE: f32 = 8.0;
/// Smallest object a resize makes.
const MIN_SIZE: f64 = 8.0;
/// Excel's pixels per point.
const PX_PER_PT: f64 = 4.0 / 3.0;

/// The grid as laid out now, for placing anchors (logical px from the top
/// left of cell A1).
pub(crate) struct GridGeometry<'a> {
    /// Left edge of columns `0..lefts.len()`; the last entry is the right
    /// edge of the last column measured.
    lefts: Vec<f64>,
    widths: Vec<f64>,
    default_col: f64,
    rows: &'a RowHeights,
    default_row: f64,
    scale: (f64, f64),
}

impl<'a> GridGeometry<'a> {
    /// Columns up to `max_col` measured from the table (live widths while
    /// one is being dragged), then from the workbook.
    pub(crate) fn new(
        grid: &SheetGrid,
        table: Option<&TableState<SheetGrid>>,
        rows: &'a RowHeights,
        max_col: u32,
    ) -> Self {
        let scale = grid.scale;
        let book = &grid.book;
        let n = max_col.min(MAX_COLS - 1) + 1;
        let mut lefts = Vec::with_capacity(n as usize + 1);
        let mut widths = Vec::with_capacity(n as usize);
        let mut x = 0.0;
        for col in 0..n {
            let w = table
                .and_then(|t| t.col_width(col as usize + 1))
                .unwrap_or_else(|| look::column_width(book, grid.sheet, col, scale));
            lefts.push(x);
            let w = f64::from(f32::from(w));
            widths.push(w);
            x += w;
        }
        lefts.push(x);
        // Excel's default column is `width * 7 + 5` px wide for a width in
        // characters; IronCalc's px are `width * 9`.
        let iron_default = book.column_width(grid.sheet, MAX_COLS - 1);
        let excel_default = iron_default / 9.0 * 7.0 + 5.0;
        let default_col = f64::from(f32::from(look::column_width(
            book,
            grid.sheet,
            MAX_COLS - 1,
            scale,
        )));
        let default_row = f64::from(f32::from(look::row_height()));
        let excel_row = book.default_row_points(grid.sheet) * PX_PER_PT;
        Self {
            lefts,
            widths,
            default_col,
            rows,
            default_row,
            scale: (
                default_col / excel_default.max(1.0),
                default_row / excel_row.max(1.0),
            ),
        }
    }

    fn measured(&self) -> u32 {
        self.widths.len() as u32
    }
}

impl SheetGeometry for GridGeometry<'_> {
    fn col_left(&self, col: u32) -> f64 {
        match self.lefts.get(col as usize) {
            Some(x) => *x,
            None => {
                let end = *self.lefts.last().unwrap_or(&0.0);
                end + f64::from(col - self.measured()) * self.default_col
            }
        }
    }

    fn col_width(&self, col: u32) -> f64 {
        self.widths
            .get(col as usize)
            .copied()
            .unwrap_or(self.default_col)
    }

    fn row_top(&self, row: u32) -> f64 {
        let len = self.rows.len() as u32;
        if row <= len {
            self.rows.top(row as usize)
        } else {
            self.rows.top(len as usize) + f64::from(row - len) * self.default_row
        }
    }

    fn row_height(&self, row: u32) -> f64 {
        if (row as usize) < self.rows.len() {
            self.rows.height(row as usize)
        } else {
            self.default_row
        }
    }

    fn col_at(&self, x: f64) -> u32 {
        let end = *self.lefts.last().unwrap_or(&0.0);
        if x >= end {
            let past = ((x - end) / self.default_col.max(1.0)).floor() as u32;
            return (self.measured() + past).min(MAX_COLS - 1);
        }
        // The last left edge at or before x.
        let i = self.lefts.partition_point(|l| *l <= x);
        i.saturating_sub(1) as u32
    }

    fn row_at(&self, y: f64) -> u32 {
        let total = self.rows.top(self.rows.len());
        if y >= total {
            let past = ((y - total) / self.default_row.max(1.0)).floor() as u32;
            return (self.rows.len() as u32 + past).min(sheets_engine::MAX_ROWS - 1);
        }
        self.rows.row_at(y).unwrap_or(0) as u32
    }

    fn scale(&self) -> (f64, f64) {
        self.scale
    }
}

/// The last column any of `drawings` reaches (for measuring columns).
fn max_col(drawings: &[Drawing]) -> u32 {
    use sheets_engine::drawing::Anchor;
    drawings
        .iter()
        .map(|d| match d.anchor {
            Anchor::TwoCell { to, .. } => to.col + 1,
            Anchor::OneCell { from, .. } => from.col + 40,
            Anchor::Absolute { .. } => 200,
        })
        .max()
        .unwrap_or(0)
}

/// What a drag changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Grip {
    Move,
    /// A handle: `(dx, dy)` in -1, 0, 1 (which edges it moves).
    Edge(i8, i8),
}

const GRIPS: [(i8, i8); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
];

/// A move or resize in progress.
#[derive(Clone, Copy, Debug)]
struct Drag {
    key: DrawingKey,
    grip: Grip,
    start: Point<Pixels>,
    from: [f64; 4],
    rect: [f64; 4],
}

impl Drag {
    fn update(&mut self, at: Point<Pixels>) {
        let dx = f64::from(f32::from(at.x - self.start.x));
        let dy = f64::from(f32::from(at.y - self.start.y));
        let [x, y, w, h] = self.from;
        self.rect = match self.grip {
            Grip::Move => [(x + dx).max(0.0), (y + dy).max(0.0), w, h],
            Grip::Edge(ex, ey) => {
                let (mut x0, mut y0, mut x1, mut y1) = (x, y, x + w, y + h);
                match ex {
                    -1 => x0 = (x0 + dx).min(x1 - MIN_SIZE).max(0.0),
                    1 => x1 = (x1 + dx).max(x0 + MIN_SIZE),
                    _ => {}
                }
                match ey {
                    -1 => y0 = (y0 + dy).min(y1 - MIN_SIZE).max(0.0),
                    1 => y1 = (y1 + dy).max(y0 + MIN_SIZE),
                    _ => {}
                }
                [x0, y0, x1 - x0, y1 - y0]
            }
        };
    }

    fn changed(&self) -> bool {
        self.rect
            .iter()
            .zip(self.from)
            .any(|(a, b)| (a - b).abs() > 0.25)
    }
}

/// `(workbook revision, sheet)`.
type ListKey = ((u64, u64), u32);

/// Built drawings and what the user is doing with them.
#[derive(Default)]
pub(crate) struct DrawLayer {
    painter: Option<Rc<Painter>>,
    scenes: DrawingScenes,
    /// Painted forms of built scenes, by the scene's address (the `Arc` is
    /// kept so the address stays its own).
    prepared: HashMap<usize, (Arc<Vec<Item>>, Rc<Painted>)>,
    /// The sheet's objects for `(revision, sheet)`.
    list: Option<(ListKey, Rc<Vec<Drawing>>)>,
    pub(crate) selected: Option<DrawingKey>,
    drag: Option<Drag>,
}

impl DrawLayer {
    /// Forget what was built (the workbook was replaced or reloaded).
    pub(crate) fn clear(&mut self) {
        self.scenes.clear();
        self.prepared.clear();
        self.list = None;
        self.selected = None;
        self.drag = None;
    }

    fn drawings(&mut self, book: &Workbook, sheet: u32) -> Rc<Vec<Drawing>> {
        let key = (book.revision(), sheet);
        if let Some((k, list)) = &self.list
            && *k == key
        {
            return list.clone();
        }
        let list = Rc::new(book.drawings(sheet));
        if self
            .selected
            .is_some_and(|s| !list.iter().any(|d| d.key == s))
        {
            self.selected = None;
        }
        self.list = Some((key, list.clone()));
        list
    }

    fn painted(&mut self, items: Arc<Vec<Item>>, cx: &App) -> Option<Rc<Painted>> {
        let painter = self.painter.clone()?;
        let addr = Arc::as_ptr(&items) as usize;
        if let Some((_, p)) = self.prepared.get(&addr) {
            return Some(p.clone());
        }
        let p = Rc::new(painter.prepare_items(&items, slides_engine::Rgb([255, 255, 255]), cx));
        if self.prepared.len() > 512 {
            self.prepared.clear();
        }
        self.prepared.insert(addr, (items, p.clone()));
        Some(p)
    }
}

/// One horizontal pane of the grid: where sheet x = 0 is on screen and
/// what it shows.
#[derive(Clone, Copy, Debug)]
struct Pane {
    clip: Bounds<Pixels>,
    origin_x: Pixels,
    /// Where sheet y = 0 would be on screen in this pane.
    origin_y: Pixels,
}

/// An object placed on screen this frame.
struct Placed {
    drawing: Drawing,
    /// Sheet px.
    rect: [f64; 4],
    /// Screen rectangle (full, unclipped) in each pane it shows in, with
    /// that pane's clip.
    shown: Vec<(Bounds<Pixels>, Bounds<Pixels>)>,
}

/// What prepaint hands paint.
pub(crate) struct Frame {
    placed: Vec<Placed>,
    hitboxes: Vec<(usize, Grip, Hitbox)>,
    body: Bounds<Pixels>,
    interactive: bool,
}

fn screen_rect(pane: &Pane, [x, y, w, h]: [f64; 4]) -> Bounds<Pixels> {
    Bounds::new(
        point(pane.origin_x + px(x as f32), pane.origin_y + px(y as f32)),
        size(px(w as f32), px(h as f32)),
    )
}

fn handle_bounds(r: Bounds<Pixels>, (gx, gy): (i8, i8)) -> Bounds<Pixels> {
    let cx = match gx {
        -1 => r.left(),
        0 => r.left() + r.size.width / 2.,
        _ => r.right(),
    };
    let cy = match gy {
        -1 => r.top(),
        0 => r.top() + r.size.height / 2.,
        _ => r.bottom(),
    };
    Bounds::new(
        point(cx - px(HANDLE / 2.), cy - px(HANDLE / 2.)),
        size(px(HANDLE), px(HANDLE)),
    )
}

fn cursor(grip: Grip) -> CursorStyle {
    match grip {
        Grip::Move => CursorStyle::OpenHand,
        Grip::Edge(0, _) => CursorStyle::ResizeUpDown,
        Grip::Edge(_, 0) => CursorStyle::ResizeLeftRight,
        Grip::Edge(x, y) if x == y => CursorStyle::ResizeUpLeftDownRight,
        Grip::Edge(_, _) => CursorStyle::ResizeUpRightDownLeft,
    }
}

impl SheetsView {
    /// The layer drawing the sheet's charts and pictures over the grid
    /// (an overlay filling the grid area).
    pub(crate) fn render_drawings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let layer = self.drawings.clone();
        let table = self.table.clone();
        let view = cx.entity().downgrade();
        let interactive = self.editing.is_none();
        let paint_layer = layer.clone();
        let t = kantor_ui::tokens(cx);
        canvas(
            move |_, window, cx| prepaint(&layer, &table, interactive, window, cx),
            move |_, frame, window, cx| {
                let Some(frame) = frame else { return };
                paint(&paint_layer, frame, &t, view.clone(), window, cx);
            },
        )
        .absolute()
        .inset_0()
    }

    /// Delete / Backspace / Escape with an object selected. Returns whether
    /// the key was used.
    pub(crate) fn drawing_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(selected) = self.drawings.borrow().selected else {
            return false;
        };
        match key {
            "delete" | "backspace" => {
                let res = self.table.update(cx, |table, _| {
                    table.delegate_mut().book.delete_drawing(selected)
                });
                self.drawings.borrow_mut().selected = None;
                self.status = match res {
                    Ok(()) => i18n::t("Deleted").into(),
                    Err(e) => format!("{e:#}").into(),
                };
                self.update_title(window, cx);
            }
            "escape" => self.drawings.borrow_mut().selected = None,
            _ => {
                // Anything else goes to the cells.
                self.drawings.borrow_mut().selected = None;
                cx.notify();
                return false;
            }
        }
        cx.notify();
        true
    }

    /// The end of a move or resize: write the new anchor (one undo step).
    fn drop_drawing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drawings.borrow_mut().drag.take() else {
            return;
        };
        if !drag.changed() {
            cx.notify();
            return;
        }
        let res = self.table.update(cx, |table, cx| {
            let anchor = {
                let grid = table.delegate();
                let list = grid.book.drawings(grid.sheet);
                let Some(d) = list.iter().find(|d| d.key == drag.key) else {
                    return Ok(());
                };
                let heights = grid.layout.heights.clone();
                let rows = heights.borrow();
                let max = max_col(&list).max(
                    GridGeometry::new(grid, Some(table), &rows, 0)
                        .col_at(drag.rect[0] + drag.rect[2])
                        + 2,
                );
                let geom = GridGeometry::new(grid, Some(table), &rows, max);
                d.anchor.with_rect(drag.rect, &geom)
            };
            let grid = table.delegate_mut();
            let res = grid.book.place_drawing(drag.key, anchor);
            grid.resize();
            table.refresh(cx);
            res
        });
        self.status = match (res, drag.grip) {
            (Ok(()), Grip::Move) => i18n::t("Moved").into(),
            (Ok(()), _) => i18n::t("Resized").into(),
            (Err(e), _) => format!("{e:#}").into(),
        };
        self.update_title(window, cx);
        cx.notify();
    }
}

/// Where the grid's panes are on screen, for [`layout_panes`].
#[derive(Debug, Clone, Copy)]
struct PaneInput {
    /// Left edge of column A (right of the row numbers).
    sheet_left: Pixels,
    /// Top of row 1 (under the column letters).
    body_top: Pixels,
    /// Top of the scrolling rows (under the frozen ones).
    rows_top: Pixels,
    bottom: Pixels,
    /// Left edge of the scrolling columns.
    cols_left: Pixels,
    right: Pixels,
    /// Scroll offsets (`scroll_x` ≤ 0, as the handle has it; `scroll_y` ≥ 0).
    scroll_x: Pixels,
    scroll_y: Pixels,
    /// Size of the frozen columns and rows in sheet px.
    frozen_w: Pixels,
    frozen_h: Pixels,
}

/// The panes objects are drawn in, each clipped to its part of the grid:
/// frozen rows and columns stay put, the rest follows the scroll offsets,
/// as Excel's split panes do. An object crossing the split shows in each
/// pane it reaches.
fn layout_panes(p: PaneInput, frozen_cols: bool, frozen_rows: bool) -> Vec<Pane> {
    let mut cols = Vec::with_capacity(2);
    if frozen_cols {
        cols.push((p.sheet_left, p.cols_left.max(p.sheet_left), p.sheet_left));
    }
    cols.push((
        p.cols_left,
        p.right.max(p.cols_left),
        p.cols_left + p.scroll_x - p.frozen_w,
    ));
    let mut rows = Vec::with_capacity(2);
    if frozen_rows {
        rows.push((p.body_top, p.rows_top.max(p.body_top), p.body_top));
    }
    rows.push((
        p.rows_top,
        p.bottom.max(p.rows_top),
        p.rows_top - p.scroll_y - p.frozen_h,
    ));
    let mut panes = Vec::with_capacity(4);
    for &(top, bottom, origin_y) in &rows {
        for &(left, right, origin_x) in &cols {
            panes.push(Pane {
                clip: Bounds::from_corners(point(left, top), point(right, bottom)),
                origin_x,
                origin_y,
            });
        }
    }
    panes
}

/// Lay the objects out on screen and register their hitboxes.
fn prepaint(
    layer: &Rc<RefCell<DrawLayer>>,
    table: &Entity<TableState<SheetGrid>>,
    interactive: bool,
    window: &mut Window,
    cx: &mut App,
) -> Option<Frame> {
    let _span = kantor_ui::perf::span("sheet drawings");
    if layer.borrow().painter.is_none() {
        let (engine, bridge) = docs_ui::shared_text(cx);
        layer.borrow_mut().painter = Some(Rc::new(Painter::new(engine, bridge)));
    }
    let table = table.read(cx);
    let grid = table.delegate();
    let drawings = layer.borrow_mut().drawings(&grid.book, grid.sheet);
    if drawings.is_empty() {
        return None;
    }
    // Where the body is: the rows' viewport and the scrolling columns.
    let (rows_vp, top) = {
        let h = table.vertical_scroll_handle.0.borrow();
        let b = h.base_handle.bounds();
        (b, -f32::from(h.base_handle.offset().y))
    };
    let cols_vp = table.horizontal_scroll_handle.bounds();
    let ox = table.horizontal_scroll_handle.offset().x;
    if rows_vp.size.width <= px(0.) || rows_vp.size.height <= px(0.) {
        return None;
    }
    let heights = grid.layout.heights.clone();
    let rows = heights.borrow();
    let geom = GridGeometry::new(grid, Some(table), &rows, max_col(&drawings));
    let (frozen_rows, frozen) = grid.book.frozen(grid.sheet);
    let frozen_w = px(geom.col_left(frozen) as f32);
    // The list scrolls the rows below the frozen ones, which the table pins
    // between the header and the list.
    let frozen_h = px(geom.row_top(frozen_rows) as f32);
    let body_top = rows_vp.top() - table.pinned_rows_height;
    let sheet_left = rows_vp.left() + ROW_GUTTER_WIDTH;
    let panes = layout_panes(
        PaneInput {
            sheet_left,
            body_top,
            rows_top: rows_vp.top(),
            bottom: rows_vp.bottom(),
            cols_left: cols_vp.left(),
            right: cols_vp.right(),
            scroll_x: ox,
            scroll_y: px(top),
            frozen_w,
            frozen_h,
        },
        frozen > 0,
        frozen_rows > 0,
    );
    let body = Bounds::from_corners(
        point(sheet_left, body_top),
        point(cols_vp.right(), rows_vp.bottom()),
    );
    let drag = layer.borrow().drag;
    let selected = layer.borrow().selected;
    let mut placed = Vec::new();
    for d in drawings.iter() {
        let rect = match drag {
            Some(g) if g.key == d.key => g.rect,
            _ => d.anchor.rect(&geom),
        };
        if rect[2] < 1.0 || rect[3] < 1.0 {
            continue;
        }
        let shown: Vec<_> = panes
            .iter()
            .map(|p| (screen_rect(p, rect), p.clip))
            .filter(|(r, clip)| r.intersects(clip))
            .collect();
        if shown.is_empty() {
            continue;
        }
        placed.push(Placed {
            drawing: d.clone(),
            rect,
            shown,
        });
    }
    drop(rows);
    let mut hitboxes = Vec::new();
    if interactive {
        for (i, p) in placed.iter().enumerate() {
            if !p.drawing.editable {
                continue;
            }
            for (r, clip) in &p.shown {
                let b = r.intersect(clip);
                hitboxes.push((
                    i,
                    Grip::Move,
                    window.insert_hitbox(b, HitboxBehavior::BlockMouseExceptScroll),
                ));
            }
        }
        // Handles of the selected object, over everything.
        if let Some(i) = placed.iter().position(|p| Some(p.drawing.key) == selected)
            && let Some((r, _)) = placed[i].shown.last()
        {
            for g in GRIPS {
                let b = handle_bounds(*r, g).intersect(&body);
                if b.size.width > px(0.) && b.size.height > px(0.) {
                    hitboxes.push((
                        i,
                        Grip::Edge(g.0, g.1),
                        window.insert_hitbox(b, HitboxBehavior::BlockMouseExceptScroll),
                    ));
                }
            }
        }
    }
    Some(Frame {
        placed,
        hitboxes,
        body,
        interactive,
    })
}

/// Paint the objects, the selection, and register the mouse handlers.
fn paint(
    layer: &Rc<RefCell<DrawLayer>>,
    frame: Frame,
    t: &kantor_ui::Tokens,
    view: WeakEntity<SheetsView>,
    window: &mut Window,
    cx: &mut App,
) {
    let book_items: Vec<Option<Rc<Painted>>> = {
        let Some(table) = view.upgrade().map(|v| v.read(cx).table.clone()) else {
            return;
        };
        let mut layer_ref = layer.borrow_mut();
        let layer_mut = &mut *layer_ref;
        let Some(painter) = layer_mut.painter.clone() else {
            return;
        };
        let grid = table.read(cx).delegate();
        let mut engine = painter.engine.borrow_mut();
        let started = std::time::Instant::now();
        let built: Vec<Arc<Vec<Item>>> = frame
            .placed
            .iter()
            .map(|p| {
                layer_mut.scenes.items(
                    &mut engine,
                    &grid.book,
                    &p.drawing,
                    p.rect[2] as f32,
                    p.rect[3] as f32,
                )
            })
            .collect();
        drop(engine);
        let builds = layer_mut.scenes.take_builds();
        if builds > 0 {
            log::debug!(
                "sheets: built {builds} chart(s) in {:.1} ms",
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
        built
            .into_iter()
            .map(|items| layer_mut.painted(items, cx))
            .collect()
    };
    let (painter, selected) = {
        let l = layer.borrow();
        (l.painter.clone(), l.selected)
    };
    let Some(painter) = painter else { return };
    let other = t.fg_muted;
    for (p, painted) in frame.placed.iter().zip(&book_items) {
        let Some(painted) = painted else { continue };
        for (r, clip) in &p.shown {
            window.with_content_mask(Some(ContentMask { bounds: *clip }), |window| {
                paint_items(
                    painted,
                    SceneView::px(r.origin, 1.0),
                    other,
                    &painter.bridge,
                    window,
                );
            });
        }
    }
    // The selection: an accent outline and handles.
    if let Some(p) = frame
        .placed
        .iter()
        .find(|p| Some(p.drawing.key) == selected)
        && let Some((r, _)) = p.shown.last()
    {
        let r = *r;
        window.with_content_mask(Some(ContentMask { bounds: frame.body }), |window| {
            window.paint_quad(quad(
                r,
                Corners::default(),
                transparent_black(),
                Edges::all(px(1.5)),
                t.accent,
                BorderStyle::Solid,
            ));
            for g in GRIPS {
                window.paint_quad(quad(
                    handle_bounds(r, g),
                    Corners::all(px(1.)),
                    t.bg,
                    Edges::all(px(1.5)),
                    t.accent,
                    BorderStyle::Solid,
                ));
            }
        });
    }
    if !frame.interactive {
        return;
    }
    for (_, grip, hb) in &frame.hitboxes {
        window.set_cursor_style(cursor(*grip), hb);
    }
    // Mouse: press on an object selects it and starts a drag; a press
    // elsewhere lets go of the selection.
    let keys: Vec<(DrawingKey, [f64; 4])> = frame
        .placed
        .iter()
        .map(|p| (p.drawing.key, p.rect))
        .collect();
    let hits: Vec<(usize, Grip, Hitbox)> = frame.hitboxes;
    {
        let layer = layer.clone();
        let view = view.clone();
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || e.button != MouseButton::Left {
                return;
            }
            // Handles first (registered last), then the topmost object.
            let hit = hits
                .iter()
                .rev()
                .find(|(_, _, hb)| hb.is_hovered(window))
                .map(|(i, g, _)| (*i, *g));
            let Some((i, grip)) = hit else {
                if layer.borrow().selected.is_some() {
                    layer.borrow_mut().selected = None;
                    let _ = view.update(cx, |_, cx| cx.notify());
                }
                return;
            };
            let (key, rect) = keys[i];
            {
                let mut l = layer.borrow_mut();
                l.selected = Some(key);
                l.drag = Some(Drag {
                    key,
                    grip,
                    start: e.position,
                    from: rect,
                    rect,
                });
            }
            cx.stop_propagation();
            let _ = view.update(cx, |this, cx| {
                this.focus_grid(window, cx);
                cx.notify();
            });
        });
    }
    if layer.borrow().drag.is_some() {
        let layer2 = layer.clone();
        let view2 = view.clone();
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
            if phase != DispatchPhase::Capture {
                return;
            }
            if let Some(d) = layer2.borrow_mut().drag.as_mut() {
                d.update(e.position);
            }
            cx.stop_propagation();
            let _ = view2.update(cx, |_, cx| cx.notify());
        });
    }
    // Always listening: a quick click's button comes up before the frame
    // that would add this listener for the drag, and the object would then
    // stick to the pointer.
    let layer3 = layer.clone();
    window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
        if phase != DispatchPhase::Capture || e.button != MouseButton::Left {
            return;
        }
        let dragging = match layer3.borrow_mut().drag.as_mut() {
            Some(d) => {
                d.update(e.position);
                true
            }
            None => false,
        };
        if !dragging {
            return;
        }
        let _ = view.update(cx, |this, cx| this.drop_drawing(window, cx));
    });
}

impl SheetsView {
    /// A chart sheet in place of the toolbar, formula bar and grid: a bar
    /// naming it, and its chart filling the rest.
    pub(crate) fn render_chart_sheet_pane(
        &self,
        k: usize,
        name: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use kantor_ui::metrics::*;
        let t = kantor_ui::tokens(cx);
        gpui_kit::component::v_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .track_focus(&self.chart_focus)
            .child(
                gpui_kit::component::h_flex()
                    .flex_none()
                    .h(CONTROL_LG)
                    .px(SPACE_4)
                    .gap(SPACE_2)
                    .bg(t.chrome)
                    .border_b_1()
                    .border_color(t.border)
                    .text_size(FONT_SMALL)
                    .text_color(t.fg_muted)
                    .child(kantor_ui::i18n::tf(
                        "Chart sheet “{}” · view only: its data is on the worksheets",
                        &[&name],
                    )),
            )
            .child(
                div()
                    .id("chart-sheet")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .p(SPACE_5)
                    .bg(t.surface)
                    .child(self.render_chart_sheet(k, cx)),
            )
    }

    /// Chart sheet `k` filling the pane (read-only: no selection).
    pub(crate) fn render_chart_sheet(&self, k: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let layer = self.drawings.clone();
        let table = self.table.clone();
        let other = kantor_ui::tokens(cx).fg_muted;
        canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                paint_chart_sheet(&layer, &table, k, bounds, other, window, cx)
            },
        )
        .size_full()
    }
}

fn paint_chart_sheet(
    layer: &Rc<RefCell<DrawLayer>>,
    table: &Entity<TableState<SheetGrid>>,
    k: usize,
    bounds: Bounds<Pixels>,
    other: Hsla,
    window: &mut Window,
    cx: &mut App,
) {
    if layer.borrow().painter.is_none() {
        let (engine, bridge) = docs_ui::shared_text(cx);
        layer.borrow_mut().painter = Some(Rc::new(Painter::new(engine, bridge)));
    }
    let (w, h) = (
        f64::from(f32::from(bounds.size.width)),
        f64::from(f32::from(bounds.size.height)),
    );
    if w < 1.0 || h < 1.0 {
        return;
    }
    let book = &table.read(cx).delegate().book;
    let drawings: Vec<Drawing> = book
        .chart_sheet_drawings(k)
        .into_iter()
        .filter(|d| !d.hidden)
        .collect();
    let rects = sheets_engine::drawing::chart_sheet_rects(&drawings, w, h);
    let mut l = layer.borrow_mut();
    let Some(painter) = l.painter.clone() else {
        return;
    };
    let built: Vec<Arc<Vec<Item>>> = {
        let mut engine = painter.engine.borrow_mut();
        drawings
            .iter()
            .zip(&rects)
            .map(|(d, r)| {
                l.scenes
                    .items(&mut engine, book, d, r[2] as f32, r[3] as f32)
            })
            .collect()
    };
    let painted: Vec<Option<Rc<Painted>>> = built
        .into_iter()
        .map(|items| l.painted(items, cx))
        .collect();
    drop(l);
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for (r, p) in rects.iter().zip(painted) {
            let Some(p) = p else { continue };
            let at = point(
                bounds.origin.x + px(r[0] as f32),
                bounds.origin.y + px(r[1] as f32),
            );
            paint_items(&p, SceneView::px(at, 1.0), other, &painter.bridge, window);
        }
    });
}

#[cfg(test)]
mod tests {

    fn pane_input() -> PaneInput {
        PaneInput {
            sheet_left: px(40.),
            body_top: px(30.),
            rows_top: px(90.),
            bottom: px(500.),
            cols_left: px(140.),
            right: px(800.),
            scroll_x: px(-300.),
            scroll_y: px(1000.),
            frozen_w: px(100.),
            frozen_h: px(60.),
        }
    }

    #[std::prelude::v1::test]
    fn panes_pin_frozen_rows_and_columns() {
        let panes = layout_panes(pane_input(), true, true);
        assert_eq!(panes.len(), 4);
        // Top-left: frozen both ways, at the sheet's origin.
        let tl = &panes[0];
        assert_eq!((tl.origin_x, tl.origin_y), (px(40.), px(30.)));
        assert_eq!(
            tl.clip,
            Bounds::from_corners(point(px(40.), px(30.)), point(px(140.), px(90.)))
        );
        // Top-right: frozen rows, columns scrolled; sheet x 100 (the first
        // scrolling column) at the scrolling columns' left minus the scroll.
        let tr = &panes[1];
        assert_eq!(tr.origin_y, px(30.));
        assert_eq!(tr.origin_x + px(100.), px(140.) - px(300.));
        // Bottom-left: frozen columns, rows scrolled; sheet y 60 (row below
        // the frozen ones) at the list's top minus the scroll.
        let bl = &panes[2];
        assert_eq!(bl.origin_x, px(40.));
        assert_eq!(bl.origin_y + px(60.), px(90.) - px(1000.));
        assert_eq!(bl.clip.top(), px(90.));
        // Bottom-right scrolls both ways.
        let br = &panes[3];
        assert_eq!((br.origin_x, br.origin_y), (tr.origin_x, bl.origin_y));
        assert_eq!(
            br.clip,
            Bounds::from_corners(point(px(140.), px(90.)), point(px(800.), px(500.)))
        );
    }

    #[std::prelude::v1::test]
    fn an_object_crossing_the_split_shows_in_each_pane() {
        let panes = layout_panes(pane_input(), true, true);
        // From sheet (80, 40) to (680, 1300): crosses both splits and is
        // still on screen past the scrolled-away part.
        let rect = [80.0, 40.0, 600.0, 1260.0];
        let shown: Vec<usize> = panes
            .iter()
            .enumerate()
            .filter(|(_, p)| screen_rect(p, rect).intersects(&p.clip))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(shown, vec![0, 1, 2, 3]);
        // A picture inside the frozen rows (y 0..50) stays in the top panes
        // whatever the vertical scroll.
        let top_only = [500.0, 0.0, 200.0, 50.0];
        let shown: Vec<usize> = panes
            .iter()
            .enumerate()
            .filter(|(_, p)| screen_rect(p, top_only).intersects(&p.clip))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(shown, vec![1]);
        // No frozen panes: one pane, as before.
        let one = layout_panes(
            PaneInput {
                frozen_w: px(0.),
                frozen_h: px(0.),
                rows_top: px(30.),
                cols_left: px(40.),
                ..pane_input()
            },
            false,
            false,
        );
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].origin_y, px(30.) - px(1000.));
    }
    use super::*;

    #[std::prelude::v1::test]
    fn drags_move_and_resize_within_limits() {
        let mut d = Drag {
            key: DrawingKey {
                sheet_id: 1,
                index: 0,
            },
            grip: Grip::Move,
            start: point(px(100.), px(100.)),
            from: [50.0, 60.0, 200.0, 100.0],
            rect: [50.0, 60.0, 200.0, 100.0],
        };
        d.update(point(px(130.), px(90.)));
        assert_eq!(d.rect, [80.0, 50.0, 200.0, 100.0]);
        d.update(point(px(-500.), px(100.)));
        assert_eq!(d.rect[0], 0.0, "never left of column A");
        d.grip = Grip::Edge(-1, 1);
        d.update(point(px(110.), px(120.)));
        assert_eq!(d.rect, [60.0, 60.0, 190.0, 120.0]);
        // Dragging the left edge past the right one stops at the minimum.
        d.update(point(px(1000.), px(100.)));
        assert_eq!(d.rect[2], MIN_SIZE);
        assert!(d.changed());
        d.update(point(px(100.), px(100.)));
        assert!(!d.changed());
    }

    #[std::prelude::v1::test]
    fn grid_geometry_places_columns_and_rows() {
        let rows = RowHeights::from_heights(vec![20, 40, 0, 30], 1.0);
        let geom = GridGeometry {
            lefts: vec![0.0, 90.0, 140.0, 300.0],
            widths: vec![90.0, 50.0, 160.0],
            default_col: 90.0,
            rows: &rows,
            default_row: 25.0,
            scale: (1.2, 1.25),
        };
        assert_eq!(geom.col_left(2), 140.0);
        assert_eq!(
            geom.col_left(5),
            300.0 + 2.0 * 90.0,
            "past the measured ones"
        );
        assert_eq!(geom.col_at(139.9), 1);
        assert_eq!(geom.col_at(140.0), 2);
        assert_eq!(geom.col_at(400.0), 4);
        assert_eq!(geom.row_top(3), 60.0);
        assert_eq!(geom.row_at(59.0), 1);
        assert_eq!(geom.row_at(60.0), 3, "the hidden row has no pixels");
        assert_eq!(geom.row_top(6), 90.0 + 2.0 * 25.0, "past the grid's rows");
        assert_eq!(geom.row_at(91.0), 4);
        use sheets_engine::drawing::{Anchor, EditAs, Marker};
        let a = Anchor::TwoCell {
            from: Marker {
                col: 1,
                col_off: 9525 * 10,
                row: 1,
                row_off: 9525 * 8,
            },
            to: Marker {
                col: 3,
                col_off: 0,
                row: 4,
                row_off: 0,
            },
            edit_as: EditAs::TwoCell,
        };
        assert_eq!(a.rect(&geom), [102.0, 30.0, 198.0, 60.0]);
        let back = a.with_rect(a.rect(&geom), &geom);
        assert_eq!(back.rect(&geom), a.rect(&geom));
    }
}
