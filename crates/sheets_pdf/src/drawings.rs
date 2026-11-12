//! Charts and pictures on a printed sheet.
//!
//! Each object is placed once per sheet, in points from the sheet's
//! top-left corner, by its anchor ([`sheets_engine::drawing::Anchor`]) over
//! the printed row heights and column widths (hidden ones are 0, so an
//! object over hidden cells shrinks the way Excel prints it). A page then
//! shows the part of each object that falls on its own band of rows and
//! columns: an object across a page break is drawn on both pages, clipped
//! to each. Repeated title rows and columns don't repeat objects.
//!
//! Charts are parsed here, once; their live cell values are read when a
//! page is extracted, and the encoder lays them out (off the UI thread).

use std::sync::Arc;

use sheets_engine::Workbook;
use sheets_engine::drawing::{Anchor, Drawing, DrawingKey, DrawingKind, SheetGeometry};
use sheets_scene::ChartTheme;
use slides_scene::chart::{Chart, Defaults};

use crate::{Overlay, OverlayKind, SheetPlan};

/// Points per Excel pixel.
const PT_PER_PX: f64 = 0.75;

/// What a printed object is.
#[derive(Debug, Clone)]
pub enum PrintKind {
    Picture {
        part: Arc<str>,
        data: Arc<Vec<u8>>,
        crop: [f32; 4],
    },
    /// A parsed chart, before its live data, and whether its references
    /// are in the current rows and columns (made in Kantor).
    Chart(Arc<Chart>, bool),
}

/// An object to print and where it is on the sheet.
#[derive(Debug, Clone)]
pub struct PrintDrawing {
    pub key: DrawingKey,
    /// `[x, y, w, h]` in points from the sheet's top-left corner.
    pub rect: [f64; 4],
    pub kind: PrintKind,
}

/// The objects of one sheet to print, with the sheet's row and column
/// starts (only built when it has some).
#[derive(Debug, Clone, Default)]
pub struct SheetDrawings {
    pub items: Vec<PrintDrawing>,
    /// Left edge of columns `0..=n`, points (`col_starts[c + 1]` is the
    /// right edge of `c`).
    col_starts: Vec<f64>,
    row_starts: Vec<f64>,
    defaults: Option<Arc<Defaults>>,
}

/// The objects of `sheet` that print: charts and pictures that aren't
/// hidden and print with the sheet.
pub(crate) fn printed(book: &Workbook, sheet: u32) -> Vec<Drawing> {
    if !book.has_drawings() {
        return Vec::new();
    }
    book.drawings(sheet)
        .into_iter()
        .filter(|d| {
            d.prints
                && !d.hidden
                && matches!(
                    *d.kind,
                    DrawingKind::Chart { .. } | DrawingKind::Picture { .. }
                )
        })
        .collect()
}

/// The last row and column whose size moves an anchor.
fn extent(a: &Anchor) -> Option<(u32, u32)> {
    match *a {
        Anchor::TwoCell { from, to, .. } => Some((from.row.max(to.row), from.col.max(to.col))),
        Anchor::OneCell { from, .. } => Some((from.row, from.col)),
        Anchor::Absolute { .. } => None,
    }
}

/// The last row any of `found` depends on.
pub(crate) fn last_row(found: &[Drawing]) -> Option<u32> {
    found
        .iter()
        .filter_map(|d| extent(&d.anchor))
        .map(|e| e.0)
        .max()
}

/// Most rows or columns walked to find where an object ends.
const MAX_WALK: u32 = 1 << 20;

/// The cell under `pos` points from the start of cell `from`, walking
/// sizes (`pos` may reach into later cells).
fn walk(from: u32, mut pos: f64, size: &dyn Fn(u32) -> f64) -> u32 {
    let mut i = from;
    while i < from + MAX_WALK {
        let s = size(i);
        if pos < s || (pos <= s && s > 0.0) {
            return i;
        }
        pos -= s;
        i += 1;
    }
    i
}

/// The cells `found` cover (top-left and bottom-right cell of each object
/// together), from the sheet's column widths and row heights in points.
/// An object whose corner sits exactly on a cell's edge doesn't take that
/// cell in.
pub(crate) fn cell_bounds(
    found: &[Drawing],
    col_w: &dyn Fn(u32) -> f64,
    row_h: &dyn Fn(u32) -> f64,
) -> Option<sheets_engine::CellRange> {
    let px = |emu: i64| emu as f64 / sheets_engine::drawing::EMU_PER_PX * PT_PER_PX;
    let mut out: Option<(u32, u32, u32, u32)> = None;
    for d in found {
        let ((r0, c0), (r1, c1)) = match d.anchor {
            Anchor::TwoCell { from, to, .. } => {
                let back = |cell: u32, off: i64, start: u32| {
                    if off == 0 && cell > start {
                        cell - 1
                    } else {
                        cell
                    }
                };
                (
                    (from.row, from.col),
                    (
                        back(to.row, to.row_off, from.row),
                        back(to.col, to.col_off, from.col),
                    ),
                )
            }
            Anchor::OneCell { from, cx, cy } => (
                (from.row, from.col),
                (
                    walk(from.row, px(from.row_off) + px(cy) - 0.01, row_h),
                    walk(from.col, px(from.col_off) + px(cx) - 0.01, col_w),
                ),
            ),
            Anchor::Absolute { x, y, cx, cy } => {
                let (r0, c0) = (walk(0, px(y), row_h), walk(0, px(x), col_w));
                (
                    (r0, c0),
                    (
                        walk(0, px(y) + px(cy) - 0.01, row_h),
                        walk(0, px(x) + px(cx) - 0.01, col_w),
                    ),
                )
            }
        };
        let (r1, c1) = (
            r1.min(sheets_engine::MAX_ROWS - 1),
            c1.min(sheets_engine::MAX_COLS - 1),
        );
        out = Some(match out {
            None => (r0, c0, r1, c1),
            Some((a, b, c, e)) => (a.min(r0), b.min(c0), c.max(r1), e.max(c1)),
        });
    }
    out.map(|(r0, c0, r1, c1)| sheets_engine::CellRange::from_corners((r0, c0), (r1, c1)))
}

fn starts(n: u32, size: impl Fn(u32) -> f64) -> Vec<f64> {
    let mut out = Vec::with_capacity(n as usize + 2);
    let mut at = 0.0;
    out.push(at);
    for i in 0..=n {
        at += size(i);
        out.push(at);
    }
    out
}

/// The printed sheet's cells, for anchors (points; Excel px × 0.75).
struct Geometry<'a> {
    cols: &'a [f64],
    rows: &'a [f64],
}

fn edge(starts: &[f64], i: u32) -> f64 {
    starts[(i as usize).min(starts.len() - 1)]
}

fn at(starts: &[f64], v: f64) -> u32 {
    (starts.partition_point(|&s| s <= v).saturating_sub(1)).min(starts.len().saturating_sub(2))
        as u32
}

impl SheetGeometry for Geometry<'_> {
    fn col_left(&self, col: u32) -> f64 {
        edge(self.cols, col)
    }
    fn col_width(&self, col: u32) -> f64 {
        edge(self.cols, col + 1) - edge(self.cols, col)
    }
    fn row_top(&self, row: u32) -> f64 {
        edge(self.rows, row)
    }
    fn row_height(&self, row: u32) -> f64 {
        edge(self.rows, row + 1) - edge(self.rows, row)
    }
    fn col_at(&self, x: f64) -> u32 {
        at(self.cols, x)
    }
    fn row_at(&self, y: f64) -> u32 {
        at(self.rows, y)
    }
    fn scale(&self) -> (f64, f64) {
        (PT_PER_PX, PT_PER_PX)
    }
}

impl SheetDrawings {
    /// Place `found` (from [`printed`]) on the sheet of `plan`, whose rows
    /// must cover their anchors.
    pub(crate) fn new(book: &Workbook, found: Vec<Drawing>, plan: &SheetPlan) -> Self {
        if found.is_empty() {
            return Self::default();
        }
        let (mut last_row, mut last_col) = (0u32, 0u32);
        for d in &found {
            if let Some((r, c)) = extent(&d.anchor) {
                last_row = last_row.max(r);
                last_col = last_col.max(c);
            }
        }
        for (a, _) in &plan.areas {
            last_row = last_row.max(a.bottom);
            last_col = last_col.max(a.right);
        }
        let col_starts = starts(last_col, |c| plan.col_width(c));
        let row_starts = starts(last_row, |r| plan.row_height(r));
        let geo = Geometry {
            cols: &col_starts,
            rows: &row_starts,
        };
        let mut theme: Option<ChartTheme> = None;
        let mut items = Vec::new();
        for d in found {
            let rect = d.anchor.rect(&geo);
            if rect[2] <= 0.0 || rect[3] <= 0.0 {
                continue;
            }
            let kind = match &*d.kind {
                DrawingKind::Picture { part, data, crop } => PrintKind::Picture {
                    part: part.clone(),
                    data: data.clone(),
                    crop: *crop,
                },
                DrawingKind::Chart {
                    xml, current_refs, ..
                } => {
                    let t = theme.get_or_insert_with(|| ChartTheme::of(book));
                    match t.parse(xml).filter(Chart::drawable) {
                        Some(c) => PrintKind::Chart(Arc::new(c), *current_refs),
                        None => continue,
                    }
                }
                DrawingKind::Other(_) => continue,
            };
            items.push(PrintDrawing {
                key: d.key,
                rect,
                kind,
            });
        }
        Self {
            items,
            col_starts,
            row_starts,
            defaults: theme.map(|t| Arc::new(t.defaults().clone())),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The objects showing on a page whose own cells are columns
    /// `cols.0..=cols.1` and rows `rows.0..=rows.1`, drawn from `origin`
    /// (content coordinates of that block's top-left corner) and clipped to
    /// `clip`.
    pub(crate) fn overlays(
        &self,
        book: &Workbook,
        cols: (u32, u32),
        rows: (u32, u32),
        origin: [f32; 2],
        clip: [f32; 4],
    ) -> Vec<Overlay> {
        if self.items.is_empty() {
            return Vec::new();
        }
        let x0 = edge(&self.col_starts, cols.0);
        let x1 = edge(&self.col_starts, cols.1 + 1);
        let y0 = edge(&self.row_starts, rows.0);
        let y1 = edge(&self.row_starts, rows.1 + 1);
        let mut out = Vec::new();
        for d in &self.items {
            let [x, y, w, h] = d.rect;
            // Strictly inside the band: an object that only touches its
            // edge belongs to the next page.
            if x >= x1 || x + w <= x0 || y >= y1 || y + h <= y0 {
                continue;
            }
            let kind = match &d.kind {
                PrintKind::Picture { part, data, crop } => OverlayKind::Picture {
                    part: part.clone(),
                    data: data.clone(),
                    crop: *crop,
                },
                PrintKind::Chart(base, current) => {
                    let Some(defaults) = &self.defaults else {
                        continue;
                    };
                    let (chart, hash) = sheets_scene::live_chart(base, book, *current);
                    OverlayKind::Chart {
                        key: d.key,
                        hash,
                        chart: Arc::new(chart),
                        defaults: defaults.clone(),
                    }
                }
            };
            out.push(Overlay {
                rect: [
                    origin[0] + (x - x0) as f32,
                    origin[1] + (y - y0) as f32,
                    w as f32,
                    h as f32,
                ],
                clip,
                kind,
            });
        }
        out
    }
}
