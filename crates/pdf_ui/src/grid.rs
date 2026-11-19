//! Page lists you can organize: the thumbnails in the sidebar (one column)
//! and the Organize Pages grid (big thumbnails filling the pane). Both
//! select with click / Shift / Ctrl, drag pages to a new place (or to
//! another open PDF), take dropped files, and have the page menu.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;

use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, tf};
use kantor_ui::{metrics::*, tokens};
use pdf_engine::Turn;

use crate::layout::ImageCache;
use crate::model::PageKey;
use crate::organize::{DragLabel, PageDrag, drop_line, page_box, page_menu};
use crate::worker::{PageJob, Worker};
use crate::{PdfView, THUMB_H, THUMB_W};

/// Key context of the thumbnails and the grid.
pub const PAGES_CONTEXT: &str = "PdfPages";

/// One thumbnail row in the sidebar: the box plus its page number, px.
pub(crate) const THUMB_ROW: f32 = THUMB_H + 38.0;
/// The grid's page box and cell, px.
const GRID_W: f32 = 168.0;
const GRID_H: f32 = 216.0;
const CELL_W: f32 = GRID_W + 40.0;
const CELL_H: f32 = GRID_H + 46.0;
const GRID_PAD: f32 = 16.0;

/// Which list a page cell is in.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListKind {
    Sidebar,
    Grid,
}

type CellPage = (PageKey, Turn, (f32, f32));

/// What the cells of one frame need (they're built inside the lists'
/// closures, away from the view).
#[derive(Clone)]
pub(crate) struct Cells {
    kind: ListKind,
    /// Each page's source page, turn and size as shown.
    keys: Rc<Vec<CellPage>>,
    selected: Rc<BTreeSet<usize>>,
    current: Option<usize>,
    drop_at: Option<usize>,
    thumbs: Rc<RefCell<ImageCache>>,
    worker: Option<Rc<Worker>>,
    weak: WeakEntity<PdfView>,
    sf: f32,
}

impl Cells {
    pub fn new(view: &PdfView, kind: ListKind, window: &Window, cx: &Context<PdfView>) -> Self {
        let keys = view
            .doc()
            .map(|d| {
                (0..d.model.len())
                    .filter_map(|i| {
                        let p = d.model.page(i)?;
                        let (w, h) = d.model.unturned_size(p);
                        let turn = view.page_turn(i);
                        Some((p.key(), turn, turn.size(w, h)))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            kind,
            keys: Rc::new(keys),
            selected: Rc::new(view.page_sel.clone()),
            current: (kind == ListKind::Sidebar).then(|| view.current_page()),
            drop_at: view.drop_at,
            thumbs: view.thumbs.clone(),
            worker: view.worker.clone(),
            weak: cx.entity().downgrade(),
            sf: window.scale_factor(),
        }
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    fn box_size(&self) -> (f32, f32) {
        match self.kind {
            ListKind::Sidebar => (THUMB_W, THUMB_H),
            ListKind::Grid => (GRID_W, GRID_H),
        }
    }

    /// Ask for the thumbnails of `pages` that aren't drawn yet.
    pub fn request(&self, pages: impl Iterator<Item = usize>) {
        let (bw, bh) = self.box_size();
        let mut jobs = Vec::new();
        for i in pages {
            let Some(&(key, turn, (w, h))) = self.keys.get(i) else {
                continue;
            };
            let k = (bw / w).min(bh / h);
            let scale = k * self.sf;
            let have = match self.kind {
                // Any size will do in the sidebar.
                ListKind::Sidebar => self.thumbs.borrow().has(key, turn),
                ListKind::Grid => self
                    .thumbs
                    .borrow_mut()
                    .get(key, turn, scale)
                    .is_some_and(|(_, exact)| exact),
            };
            if !have {
                jobs.push(PageJob { key, scale, turn });
            }
        }
        if let Some(w) = &self.worker {
            match self.kind {
                ListKind::Sidebar => w.want_thumbs(jobs),
                ListKind::Grid => w.want_grid(jobs),
            }
        }
    }

    /// The cell of page `i`.
    pub fn cell(&self, i: usize, cx: &App) -> AnyElement {
        let (bw, bh) = self.box_size();
        let (key, turn, (w, h)) = self.keys[i];
        let k = (bw / w).min(bh / h);
        let image = self
            .thumbs
            .borrow_mut()
            .get(key, turn, k * self.sf)
            .map(|(i, _)| i);
        let selected = self.selected.contains(&i);
        let current = self.current == Some(i) && self.selected.is_empty();
        let t = tokens(cx);
        let (click, menu, right) = (self.weak.clone(), self.weak.clone(), self.weak.clone());
        let drag = self
            .weak
            .upgrade()
            .and_then(|v| v.read(cx).page_drag(i, cx));
        let id = match self.kind {
            ListKind::Sidebar => ElementId::NamedInteger("pdf-thumb".into(), i as u64),
            ListKind::Grid => ElementId::NamedInteger("pdf-grid-cell".into(), i as u64),
        };
        let (cw, ch) = match self.kind {
            ListKind::Sidebar => (None, THUMB_ROW),
            ListKind::Grid => (Some(CELL_W), CELL_H),
        };
        let mut cell = div()
            .id(id)
            .relative()
            .h(px(ch))
            .flex()
            .items_center()
            .justify_center()
            .rounded(RADIUS_MD)
            .when_some(cw, |d, w| d.w(px(w)))
            .when(cw.is_none(), |d| d.w_full())
            .when(!selected, |d| d.hover(|d| d.bg(t.hover)))
            .child(page_box(
                image,
                (w * k, h * k),
                i + 1,
                selected,
                current,
                cx,
            ))
            .on_click(move |ev, window, cx| {
                let m = ev.modifiers();
                let _ = click.update(cx, |v, cx| v.click_page(i, m, window, cx));
            })
            .on_mouse_down(MouseButton::Right, move |_, window, cx| {
                let _ = right.update(cx, |v, cx| v.context_page(i, window, cx));
            });
        if let Some(drag) = drag {
            cell = cell.on_drag(drag, |d: &PageDrag, _, _, cx| {
                let n = d.places.len();
                let label = if n == 1 {
                    i18n::t("1 page").to_owned()
                } else {
                    tf("{} pages", &[&n])
                };
                cx.new(|_| DragLabel(label.into()))
            });
        }
        // Where dropped pages go: a line before this page, or after the
        // last one.
        let n = self.len();
        let before = self.drop_at == Some(i);
        let after = self.drop_at == Some(n) && i + 1 == n;
        cell = match self.kind {
            ListKind::Sidebar => cell
                .when(before, |d| d.child(drop_line(false, cx).top(px(-2.))))
                .when(after, |d| d.child(drop_line(false, cx).bottom(px(-2.)))),
            ListKind::Grid => cell
                .when(before, |d| d.child(drop_line(true, cx).left(px(-2.))))
                .when(after, |d| d.child(drop_line(true, cx).right(px(-2.)))),
        };
        cell.context_menu(move |m, window, cx| page_menu(m, &menu, window, cx))
            .into_any_element()
    }
}

/// The gap a pointer at `pos` points at in a list in `bounds` scrolled by
/// `offset` (negative, px), for `n` pages in `cols` columns of `cell`
/// size. `None` outside the list.
pub(crate) fn drop_index(
    pos: Point<Pixels>,
    bounds: Bounds<Pixels>,
    offset: Point<Pixels>,
    n: usize,
    cols: usize,
    cell: (f32, f32),
    pad: f32,
) -> Option<usize> {
    if !bounds.contains(&pos) || n == 0 {
        return None;
    }
    let cols = cols.max(1);
    let y = f32::from(pos.y - bounds.origin.y - offset.y) - pad;
    let row = (y / cell.1).floor().max(0.0) as usize;
    let at = if cols == 1 {
        let frac = y / cell.1 - row as f32;
        row + usize::from(frac > 0.5)
    } else {
        let x = f32::from(pos.x - bounds.origin.x) - pad;
        let col = (x / cell.0).floor().clamp(0.0, cols as f32 - 1.0) as usize;
        let frac = x / cell.0 - col as f32;
        row * cols + col + usize::from(frac > 0.5)
    };
    Some(at.min(n))
}

impl PdfView {
    /// Columns of the grid at its width now.
    pub(crate) fn grid_columns(&self) -> usize {
        let w = f32::from(self.grid_bounds.get().size.width);
        (((w - 2.0 * GRID_PAD) / CELL_W).floor() as usize).max(1)
    }

    /// Where a drag at `pos` would drop in the sidebar or the grid.
    fn drop_target(&self, pos: Point<Pixels>, kind: ListKind) -> Option<usize> {
        let n = self.page_count();
        match kind {
            ListKind::Sidebar => drop_index(
                pos,
                self.thumbs_bounds.get(),
                self.thumb_scroll.0.borrow().base_handle.offset(),
                n,
                1,
                (THUMB_W, THUMB_ROW),
                0.0,
            ),
            ListKind::Grid => drop_index(
                pos,
                self.grid_bounds.get(),
                self.grid_scroll.0.borrow().base_handle.offset(),
                n,
                self.grid_columns(),
                (CELL_W, CELL_H),
                GRID_PAD,
            ),
        }
    }

    fn set_drop(&mut self, at: Option<usize>, cx: &mut Context<Self>) {
        if self.drop_at != at {
            self.drop_at = at;
            cx.notify();
        }
    }
}

/// Wrap a page list: focus and keys for page commands, drag and drop of
/// pages and files, its bounds for drop places.
pub(crate) fn page_list(
    list: AnyElement,
    kind: ListKind,
    view: &PdfView,
    cx: &mut Context<PdfView>,
) -> Stateful<Div> {
    let bounds: Rc<Cell<Bounds<Pixels>>> = match kind {
        ListKind::Sidebar => view.thumbs_bounds.clone(),
        ListKind::Grid => view.grid_bounds.clone(),
    };
    let weak = cx.entity().downgrade();
    let (w1, w2, w3) = (weak.clone(), weak.clone(), weak);
    div()
        .id(match kind {
            ListKind::Sidebar => "pdf-thumb-area",
            ListKind::Grid => "pdf-grid-area",
        })
        .key_context(PAGES_CONTEXT)
        .track_focus(&view.pages_focus)
        .size_full()
        .relative()
        .child(
            canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                .absolute()
                .size_full(),
        )
        .child(list)
        .on_drag_move::<PageDrag>(move |ev, _, cx| {
            let pos = ev.event.position;
            let _ = w1.update(cx, |v, cx| {
                let at = v.drop_target(pos, kind);
                v.set_drop(at, cx);
            });
        })
        .on_drag_move::<ExternalPaths>(move |ev, _, cx| {
            let pos = ev.event.position;
            let _ = w2.update(cx, |v, cx| {
                let at = v.drop_target(pos, kind);
                v.set_drop(at, cx);
            });
        })
        .on_drop::<PageDrag>(move |drag, _, cx| {
            let _ = w3.update(cx, |v, cx| v.drop_pages(drag, cx));
        })
}

/// The Organize Pages view: every page as a big thumbnail in a grid.
pub(crate) fn render(
    view: &mut PdfView,
    window: &mut Window,
    cx: &mut Context<PdfView>,
) -> AnyElement {
    let t = tokens(cx);
    let cells = Cells::new(view, ListKind::Grid, window, cx);
    let n = cells.len();
    let cols = view.grid_columns();
    let rows = n.div_ceil(cols);
    let list = uniform_list("pdf-grid", rows, move |range, _, cx| {
        let first = range.start * cols;
        let last = (range.end * cols).min(n);
        cells.request(first..last);
        range
            .map(|r| {
                let mut row = h_flex().h(px(CELL_H)).px(px(GRID_PAD));
                for i in r * cols..((r + 1) * cols).min(n) {
                    row = row.child(cells.cell(i, cx));
                }
                row
            })
            .collect()
    })
    .track_scroll(&view.grid_scroll)
    .py(px(GRID_PAD))
    .size_full()
    .into_any_element();
    let hint = div()
        .flex_none()
        .px(SPACE_6)
        .py(SPACE_3)
        .text_size(FONT_SMALL)
        .text_color(t.fg_muted)
        .border_b_1()
        .border_color(t.border)
        .bg(t.chrome)
        .child(i18n::t(
            "Drag pages to reorder them, or onto another open PDF. Right-click for more. Drop PDFs or pictures here to insert them.",
        ));
    v_flex()
        .flex_1()
        .min_w_0()
        .h_full()
        .bg(t.desk)
        .child(hint)
        .child(
            div()
                .flex_1()
                .min_h_0()
                .child(page_list(list, ListKind::Grid, view, cx)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b() -> Bounds<Pixels> {
        Bounds::new(point(px(100.), px(50.)), size(px(400.), px(600.)))
    }

    #[std::prelude::v1::test]
    fn drop_places_in_a_column() {
        let at = |y: f32, off: f32| {
            drop_index(
                point(px(150.), px(y)),
                b(),
                point(px(0.), px(off)),
                5,
                1,
                (100.0, 100.0),
                0.0,
            )
        };
        assert_eq!(at(60.0, 0.0), Some(0));
        assert_eq!(at(140.0, 0.0), Some(1), "lower half: after the page");
        assert_eq!(at(260.0, 0.0), Some(2));
        // Scrolled down two rows.
        assert_eq!(at(60.0, -200.0), Some(2));
        // Below the last page: the end.
        assert_eq!(at(640.0, 0.0), Some(5));
        assert_eq!(at(20.0, 0.0), None, "outside");
    }

    #[std::prelude::v1::test]
    fn drop_places_in_a_grid() {
        let at = |x: f32, y: f32| {
            drop_index(
                point(px(x), px(y)),
                b(),
                point(px(0.), px(0.)),
                7,
                3,
                (100.0, 100.0),
                10.0,
            )
        };
        // Row 0: cells at x 110..210, 210..310, 310..410.
        assert_eq!(at(120.0, 70.0), Some(0));
        assert_eq!(at(190.0, 70.0), Some(1), "right half: after it");
        assert_eq!(at(330.0, 70.0), Some(2));
        // Row 1 starts at page 3.
        assert_eq!(at(120.0, 170.0), Some(3));
        // Row 2 has page 6 only; past it: the end.
        assert_eq!(at(390.0, 270.0), Some(7));
        assert_eq!(at(495.0, 70.0), Some(3), "past the last column: after it");
    }
}
