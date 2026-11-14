//! Notes (Excel's cell comments) on the grid: a small triangle in the top
//! right corner of a cell that has one, and the note (author and text) in a
//! box beside the cell while the pointer is over it, as Excel shows them.
//!
//! The box is drawn deferred and anchored, so the cell's clipping doesn't
//! cut it and it stays inside the window. It holds plain text only (no
//! Input: see the memory note on inputs in the grid).

use gpui::prelude::FluentBuilder as _;
use gpui_kit::*;
use kantor_ui::metrics::*;
use sheets_engine::CellNote;

use crate::SheetsView;

/// Side of the corner triangle, and how far the box sits from the cell.
const MARK: Pixels = px(7.);
const GAP: Pixels = px(6.);
/// Width of the note box (Excel's default note is about this wide).
const BOX_W: Pixels = px(220.);

/// The layers a cell with a note gets: its corner mark, a hover target
/// that shows the note, and the box while `hovered`.
pub(crate) fn layers(
    note: &CellNote,
    (sheet, row, col): (u32, u32, u32),
    hovered: bool,
    view: &WeakEntity<SheetsView>,
    t: &kantor_ui::Tokens,
) -> Vec<AnyElement> {
    let mark_color = t.danger;
    let mark = canvas(
        |_, _, _| {},
        move |b: Bounds<Pixels>, _, window, _| {
            let right = b.origin.x + b.size.width;
            let top = b.origin.y;
            let mut p = PathBuilder::fill();
            p.add_polygon(
                &[
                    point(right - MARK, top),
                    point(right, top),
                    point(right, top + MARK),
                ],
                true,
            );
            if let Ok(path) = p.build() {
                window.paint_path(path, mark_color);
            }
        },
    )
    .absolute()
    .inset_0()
    .into_any_element();
    let view = view.clone();
    // A unique id per cell for the hover state.
    let id = ((row as usize) << 16) | col as usize;
    let target = div()
        .id(("cell-note", id))
        .absolute()
        .inset_0()
        .on_hover(move |hovered, _, cx| {
            let at = hovered.then_some((sheet, row, col));
            let _ = view.update(cx, |v, cx| v.hover_note(at, (sheet, row, col), cx));
        })
        .into_any_element();
    let mut out = vec![mark, target];
    if hovered {
        out.push(note_box(note, t));
    }
    out
}

/// The note beside the cell's top-right corner: author in bold, then the
/// text, like Excel's note box, in Kantor's popover look.
fn note_box(note: &CellNote, t: &kantor_ui::Tokens) -> AnyElement {
    let author = note.author.trim();
    // Excel's notes often start with "Author:" in the text itself.
    let text = note
        .text
        .strip_prefix(&format!("{author}:"))
        .map_or(note.text.as_str(), str::trim_start)
        .trim_end()
        .to_owned();
    let panel = div()
        .id("cell-note-box")
        .ml(GAP)
        .w(BOX_W)
        .p(SPACE_4)
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .bg(t.elevated)
        .border_1()
        .border_color(t.border_strong)
        .rounded(RADIUS_MD)
        .shadow_md()
        .text_size(FONT_SMALL)
        .text_color(t.fg)
        .when(!author.is_empty(), |d| {
            d.child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(SharedString::from(author.to_owned())),
            )
        })
        .child(div().whitespace_normal().child(SharedString::from(text)));
    div()
        .absolute()
        .top_0()
        .right_0()
        .child(
            deferred(anchored().snap_to_window_with_margin(SPACE_4).child(panel)).with_priority(1),
        )
        .into_any_element()
}

impl SheetsView {
    /// The pointer entered (`at`) or left the cell `cell` that has a note.
    pub(crate) fn hover_note(
        &mut self,
        at: Option<(u32, u32, u32)>,
        cell: (u32, u32, u32),
        cx: &mut Context<Self>,
    ) {
        self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let new = match at {
                Some(at) => Some(at),
                // Leaving another cell doesn't hide this one's note.
                None if grid.note_hover == Some(cell) => None,
                None => return,
            };
            if grid.note_hover != new {
                grid.note_hover = new;
                cx.notify();
            }
        });
    }
}
