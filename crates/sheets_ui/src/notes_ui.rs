//! Notes (Excel's cell comments, "Notes" since Excel 365) on the grid: a
//! small triangle in the top right corner of a cell that has one, and the
//! note (author and text) in a box beside the cell while the pointer is
//! over it, or all the time when the note is shown (Show/Hide Note) or
//! Show All Notes is on.
//!
//! New Note / Edit Note (Shift+F2, the cell's right-click menu, the Notes
//! menu) opens an editor box beside the cell. A new note starts with the
//! author's name line (`Author:`, bold in Excel), from Settings' author name
//! or the OS user. As in Excel, clicking anywhere else or pressing Escape
//! ends the edit and keeps what was typed (Escape doesn't throw it away).
//! Each change is one undo step.
//!
//! The note boxes in the grid are plain text; the editor is a sibling of
//! the grid, never inside it (the grid's key handler would take its typing,
//! see the memory note on inputs in the grid). Its place comes from the
//! edited cell's bounds, which the cell records while it paints.

use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{self, Position, Textarea, TextareaState};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;
use kantor_ui::i18n::{t, tf, tk};
use kantor_ui::metrics::*;
use kantor_ui::tokens::Elevation;
use sheets_engine::{CellNote, CellRange};

use crate::SheetsView;

actions!(sheets, [EditNote]);

/// The editor's key context (Escape ends the edit).
const NOTE_CONTEXT: &str = "KantorSheetsNote";

/// Side of the corner triangle, and how far the box sits from the cell.
const MARK: Pixels = px(7.);
const GAP: Pixels = px(6.);
/// Width of the note box (Excel's default note is about this wide).
const BOX_W: Pixels = px(220.);

pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "shift-f2",
        EditNote,
        Some(crate::KEY_CONTEXT),
    )]);
}

/// The note being written.
pub(crate) struct NoteEditor {
    sheet: u32,
    row: u32,
    col: u32,
    input: Entity<TextareaState>,
}

/// Where the edited cell is on screen, recorded as it paints.
pub(crate) type NoteAnchor = Rc<Cell<Option<Bounds<Pixels>>>>;

/// The layers a cell with a note gets: its corner mark, a hover target
/// that shows the note, and the box while `shown`.
pub(crate) fn layers(
    note: &CellNote,
    (sheet, row, col): (u32, u32, u32),
    shown: bool,
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
    if shown {
        out.push(note_box(note, t));
    }
    out
}

/// An invisible layer over the cell whose note is being edited: records
/// where the cell is, for the editor box.
pub(crate) fn anchor_layer(anchor: &NoteAnchor) -> AnyElement {
    let anchor = anchor.clone();
    canvas(
        move |b: Bounds<Pixels>, window, _| {
            if anchor.get() != Some(b) {
                anchor.set(Some(b));
                window.refresh();
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

/// The note's text without its `Author:` line (the box shows the author
/// in bold above it).
fn body_text(note: &CellNote) -> String {
    let author = note.author.trim();
    note.text
        .strip_prefix(&format!("{author}:"))
        .filter(|_| !author.is_empty())
        .map_or(note.text.as_str(), str::trim_start)
        .trim_end()
        .to_owned()
}

/// The note beside the cell's top-right corner: author in bold, then the
/// text, like Excel's note box, in Kantor's popover look.
fn note_box(note: &CellNote, t: &kantor_ui::Tokens) -> AnyElement {
    let author = note.author.trim();
    let text = body_text(note);
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

/// The caret position at the end of `text` (lines and UTF-16 columns).
fn end_of(text: &str) -> Position {
    let line = text.matches('\n').count() as u32;
    let last = text.rsplit('\n').next().unwrap_or_default();
    Position::new(line, last.encode_utf16().count() as u32)
}

/// What a note command does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteCommand {
    /// New Note, or Edit Note on a cell that has one.
    Edit,
    Delete,
    /// Show/Hide Note.
    ShowHide,
    /// Show All Notes (on or off, for the view; not saved).
    ShowAll,
    Next,
    Previous,
}

/// The note items of a cell's right-click menu (`cell`: sheet row and
/// column), as Excel lists them.
pub(crate) fn cell_menu_items(
    menu: PopupMenu,
    cell: (u32, u32),
    has_note: bool,
    view: &WeakEntity<SheetsView>,
) -> PopupMenu {
    let item = |label: &'static str, command: NoteCommand| {
        let view = view.clone();
        PopupMenuItem::new(t(label)).on_click(move |_, window, cx| {
            let _ = view.update(cx, |v, cx| {
                v.note_command_at(command, Some(cell), window, cx)
            });
        })
    };
    if has_note {
        menu.item(item(tk("Edit Note"), NoteCommand::Edit))
            .item(item(tk("Delete Note"), NoteCommand::Delete))
            .item(item(tk("Show/Hide Note"), NoteCommand::ShowHide))
    } else {
        menu.item(item(tk("New Note"), NoteCommand::Edit))
    }
}

/// The Notes menu (Review > Notes in Excel).
pub(crate) fn notes_menu(
    mut menu: PopupMenu,
    has_note: bool,
    show_all: bool,
    view: &WeakEntity<SheetsView>,
) -> PopupMenu {
    let items: [(&'static str, &str, NoteCommand); 6] = [
        (
            if has_note {
                tk("Edit Note")
            } else {
                tk("New Note")
            },
            "Shift+F2",
            NoteCommand::Edit,
        ),
        (tk("Delete Note"), "", NoteCommand::Delete),
        (tk("Previous Note"), "", NoteCommand::Previous),
        (tk("Next Note"), "", NoteCommand::Next),
        (tk("Show/Hide Note"), "", NoteCommand::ShowHide),
        (tk("Show All Notes"), "", NoteCommand::ShowAll),
    ];
    for (i, (label, keys, command)) in items.into_iter().enumerate() {
        if i == 2 || i == 4 {
            menu = menu.separator();
        }
        let view = view.clone();
        let text = if keys.is_empty() {
            t(label).to_owned()
        } else {
            format!("{}  ({keys})", t(label))
        };
        menu = menu.item(
            PopupMenuItem::new(text)
                .checked(command == NoteCommand::ShowAll && show_all)
                .on_click(move |_, window, cx| {
                    let _ = view.update(cx, |v, cx| v.note_command(command, window, cx));
                }),
        );
    }
    menu
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

    /// A note command on the active cell (the selection for Delete Note),
    /// from the Notes menu, a shortcut or the palette.
    pub fn note_command(
        &mut self,
        command: NoteCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.note_command_at(command, None, window, cx);
    }

    /// A note command; `cell` is the right-clicked cell, if it came from
    /// its menu.
    pub(crate) fn note_command_at(
        &mut self,
        command: NoteCommand,
        cell: Option<(u32, u32)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        self.commit_note(false, window, cx);
        let (sheet, range, show_all) = {
            let grid = self.table.read(cx).delegate();
            (grid.sheet, grid.range(), grid.show_all_notes)
        };
        let (row, col) = cell.unwrap_or(self.selected);
        let has_note =
            |this: &Self, cx: &App| this.table.read(cx).delegate().book.note_at(sheet, row, col);
        match command {
            NoteCommand::Edit => self.open_note_editor(row, col, window, cx),
            NoteCommand::Delete => {
                // The selection's notes (Excel), or the right-clicked cell's
                // when it is outside the selection.
                let target = if range.contains(row, col) {
                    range
                } else {
                    CellRange::from_corners((row, col), (row, col))
                };
                let res = self.change_book(cx, |book| book.delete_notes(sheet, target));
                self.note_status(
                    res,
                    tk("Note deleted"),
                    tk("There is no note to delete"),
                    window,
                    cx,
                );
            }
            NoteCommand::ShowHide => {
                let Some(note) = has_note(self, cx) else {
                    self.status = t("The active cell has no note").into();
                    cx.notify();
                    return;
                };
                let one = CellRange::from_corners((row, col), (row, col));
                let shown = !note.shown;
                let res = self.change_book(cx, |book| book.show_notes(sheet, one, shown));
                let done = if shown {
                    tk("Note shown")
                } else {
                    tk("Note hidden")
                };
                self.note_status(res, done, "", window, cx);
            }
            NoteCommand::ShowAll => {
                self.table.update(cx, |table, cx| {
                    table.delegate_mut().show_all_notes = !show_all;
                    cx.notify();
                });
                self.status = if show_all {
                    t("Notes show when the pointer is over their cell")
                } else {
                    t("All notes shown")
                }
                .into();
                cx.notify();
            }
            NoteCommand::Next | NoteCommand::Previous => {
                let back = command == NoteCommand::Previous;
                let next =
                    self.table
                        .read(cx)
                        .delegate()
                        .book
                        .next_note(sheet, self.selected, back);
                match next {
                    Some((r, c)) => {
                        self.select_cell(r, c, cx);
                        // Show it, as Excel does, until the pointer moves on.
                        self.table.update(cx, |table, cx| {
                            table.delegate_mut().note_hover = Some((sheet, r, c));
                            cx.notify();
                        });
                    }
                    None => {
                        self.status = t("This sheet has no notes").into();
                        cx.notify();
                    }
                }
            }
        }
    }

    /// Run a change on the workbook and repaint the grid.
    fn change_book<R>(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut sheets_engine::Workbook) -> anyhow::Result<R>,
    ) -> anyhow::Result<R> {
        self.table.update(cx, |table, cx| {
            let r = f(&mut table.delegate_mut().book);
            table.refresh(cx);
            r
        })
    }

    fn note_status(
        &mut self,
        res: anyhow::Result<bool>,
        done: &'static str,
        nothing: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.status = match res {
            Ok(true) => {
                self.update_title(window, cx);
                t(done).to_owned()
            }
            Ok(false) if nothing.is_empty() => String::new(),
            Ok(false) => t(nothing).to_owned(),
            Err(e) => format!("{e:#}"),
        }
        .into();
        cx.notify();
    }

    /// New Note / Edit Note on `(row, col)`: the editor box beside it.
    fn open_note_editor(
        &mut self,
        row: u32,
        col: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (sheet, note) = {
            let grid = self.table.read(cx).delegate();
            (grid.sheet, grid.book.note_at(grid.sheet, row, col))
        };
        let text = match note {
            Some(n) => n.text,
            None => {
                let author = kantor_core::settings::Settings::load().author_name();
                if author.trim().is_empty() {
                    String::new()
                } else {
                    format!("{}:\n", author.trim())
                }
            }
        };
        let input = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx).auto_grow(4, 12);
            s.set_value(text.clone(), window, cx);
            s
        });
        if self.selected != (row, col) {
            self.select_cell(row, col, cx);
        }
        self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            grid.note_editing = Some((sheet, row, col));
            grid.note_hover = None;
            cx.notify();
        });
        self.note_edit = Some(NoteEditor {
            sheet,
            row,
            col,
            input: input.clone(),
        });
        // After a menu closes it gives focus back to what had it: focus
        // the editor once that is done.
        window.defer(cx, move |window, cx| {
            input.update(cx, |s, cx| s.set_cursor_position(end_of(&text), window, cx));
        });
        cx.notify();
    }

    /// End the note edit, keeping its text (one undo step if it changed).
    /// `refocus`: give the grid the keyboard again (Escape; a click away
    /// leaves focus to what was clicked).
    pub(crate) fn commit_note(
        &mut self,
        refocus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ed) = self.note_edit.take() else {
            return;
        };
        let text = ed.input.read(cx).value().to_string();
        let author = kantor_core::settings::Settings::load().author_name();
        let author = author.trim().to_owned();
        let res = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            grid.note_editing = None;
            let before = grid.book.revision();
            let r = grid
                .book
                .set_note(ed.sheet, ed.row, ed.col, &text, &author)
                .map(|()| grid.book.revision() != before);
            table.refresh(cx);
            r
        });
        self.note_status(res, tk("Note saved"), "", window, cx);
        if refocus {
            self.focus_grid(window, cx);
        }
    }

    /// Whether the active cell has a note, and whether Show All Notes is
    /// on (for the Notes menu).
    pub(crate) fn note_menu_state(&self, cx: &App) -> (bool, bool) {
        let grid = self.table.read(cx).delegate();
        let (row, col) = self.selected;
        (
            grid.book.note_at(grid.sheet, row, col).is_some(),
            grid.show_all_notes,
        )
    }

    pub(crate) fn note_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(
            cx.listener(|this, _: &EditNote, w, cx| this.note_command(NoteCommand::Edit, w, cx)),
        )
    }

    /// The editor box, beside the edited cell.
    pub(crate) fn render_note_editor(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ed = self.note_edit.as_ref()?;
        let t = kantor_ui::tokens(cx);
        let anchor = self.table.read(cx).delegate().note_anchor.get();
        let at = match anchor {
            Some(b) => point(b.origin.x + b.size.width + GAP, b.origin.y),
            None => point(SPACE_7 * 4., SPACE_7 * 4.),
        };
        let label = tf("Note on {}", &[&sheets_engine::cell_name(ed.row, ed.col)]);
        let panel = div()
            .id("sheets-note-editor")
            .key_context(NOTE_CONTEXT)
            .occlude()
            .w(BOX_W + SPACE_7)
            .p(SPACE_3)
            .flex()
            .flex_col()
            .gap(SPACE_2)
            .bg(t.elevated)
            .border_1()
            .border_color(t.border_strong)
            .rounded(RADIUS_MD)
            .shadow(t.shadow(Elevation::Popover))
            .text_size(FONT_SMALL)
            .text_color(t.fg)
            .on_action(cx.listener(|this, _: &input::Escape, window, cx| {
                this.commit_note(true, window, cx);
            }))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.commit_note(false, window, cx);
            }))
            .child(div().text_color(t.fg_muted).child(label))
            .child(Textarea::new(&ed.input).small());
        Some(
            deferred(
                anchored()
                    .position(at)
                    .snap_to_window_with_margin(SPACE_4)
                    .child(panel),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn caret_goes_to_the_end() {
        assert_eq!(end_of("Dewi:\n"), Position::new(1, 0));
        assert_eq!(end_of("ab\ncdé"), Position::new(1, 3));
        assert_eq!(end_of(""), Position::new(0, 0));
    }

    #[std::prelude::v1::test]
    fn box_text_drops_the_author_line() {
        let note = |author: &str, text: &str| CellNote {
            row: 0,
            col: 0,
            author: author.into(),
            text: text.into(),
            shown: false,
        };
        assert_eq!(body_text(&note("Dewi", "Dewi:\nCheck it")), "Check it");
        assert_eq!(body_text(&note("Dewi", "Plain")), "Plain");
        assert_eq!(body_text(&note("", ":x")), ":x");
    }
}
