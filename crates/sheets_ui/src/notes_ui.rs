//! Notes (Excel's cell comments, "Notes" since Excel 365) on the grid: a
//! small triangle in the top right corner of a cell that has one, and the
//! note in its box while the pointer is over the cell, or all the time
//! when the note is shown (Show/Hide Note) or Show All Notes is on.
//!
//! The boxes are drawn by the drawing layer (`drawings`) at the place the
//! file gives them (the VML anchor; Excel's default place otherwise), with
//! a line to the cell's corner as Excel draws it, and their formatted text.
//! A shown note's box is moved and resized like a picture (drag it or its
//! handles; one undo step, saved to the anchor); double-click it to edit.
//!
//! New Note / Edit Note (Shift+F2, the cell's right-click menu, the Notes
//! menu) opens an editor at the note's box. A new note starts with the
//! author's name line (`Author:`, bold in Excel), from Settings' author name
//! or the OS user. The editor's toolbar formats the selected text (bold,
//! italic, underline, font, size, colour); runs keep their formatting
//! through edits (see `rich`). As in Excel, clicking anywhere else or
//! pressing Escape ends the edit and keeps what was typed. Each change is
//! one undo step.
//!
//! The editor is a sibling of the grid, never inside it (the grid's key
//! handler would take its typing, see the memory note on inputs in the
//! grid). It follows its note's box when the sheet scrolls.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{self, Position, Textarea, TextareaState};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;
use kantor_ui::i18n::{t, tf, tk};
use kantor_ui::metrics::*;
use kantor_ui::tokens::Elevation;
use sheets_engine::{CellNote, CellRange, NoteRun};
use slides_engine::{LINE_BREAK, Rgb, RunProps, TextPara};

use crate::SheetsView;
use crate::rich::{self, RichOp, RichState};

actions!(sheets, [EditNote]);

/// The editor's key context (Escape ends the edit).
const NOTE_CONTEXT: &str = "KantorSheetsNote";

/// Side of the corner triangle, and how far the box sits from the cell.
const MARK: Pixels = px(7.);
const GAP: Pixels = px(6.);

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
    /// The text with its formatting, following the box (see `rich`).
    paras: Vec<TextPara>,
    /// As the edit began.
    orig: Vec<TextPara>,
    /// The note had formatted runs of its own (kept as runs on save).
    had_runs: bool,
    _sub: Subscription,
}

/// Where the edited cell is on screen, recorded as it paints.
pub(crate) type NoteAnchor = Rc<Cell<Option<Bounds<Pixels>>>>;

/// The layers a cell with a note gets: its corner mark and a hover target
/// that shows the note (the box itself is drawn by the drawing layer).
/// The mark is `zoom` times its size at 100 %, as Excel's.
pub(crate) fn layers(
    (sheet, row, col): (u32, u32, u32),
    view: &WeakEntity<SheetsView>,
    t: &kantor_ui::Tokens,
    zoom: f32,
) -> Vec<AnyElement> {
    let mark_color = t.danger;
    let mark_side = MARK * zoom;
    let mark = canvas(
        |_, _, _| {},
        move |b: Bounds<Pixels>, _, window, _| {
            let right = b.origin.x + b.size.width;
            let top = b.origin.y;
            let mut p = PathBuilder::fill();
            p.add_polygon(
                &[
                    point(right - mark_side, top),
                    point(right, top),
                    point(right, top + mark_side),
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
    vec![mark, target]
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

/// A note's text as runs: its own, or Excel's default (a bold `Author:`
/// run when the text starts with it, then the rest).
pub(crate) fn note_runs(note: &CellNote) -> Vec<NoteRun> {
    if let Some(r) = &note.runs {
        return r.to_vec();
    }
    let author = note.author.trim();
    let head = format!("{author}:");
    match note.text.strip_prefix(&head) {
        Some(rest) if !author.is_empty() => {
            let mut out = vec![NoteRun {
                text: head,
                bold: true,
                ..NoteRun::default()
            }];
            if !rest.is_empty() {
                out.push(NoteRun {
                    text: rest.to_owned(),
                    ..NoteRun::default()
                });
            }
            out
        }
        _ => vec![NoteRun {
            text: note.text.clone(),
            ..NoteRun::default()
        }],
    }
}

fn props_of(r: &NoteRun) -> RunProps {
    let mut p = RunProps {
        bold: Some(r.bold),
        italic: Some(r.italic),
        underline: Some(r.underline),
        // Excel's note text is 9 pt unless the run says.
        size: r.size.or(Some(900)),
        color: r
            .color
            .map(|c| Rgb([(c >> 16) as u8, (c >> 8) as u8, c as u8])),
        shown_font: r.font.clone(),
        ..RunProps::default()
    };
    p.extra.post = r.other.clone();
    // A colour the file gives another way (theme, indexed) stays until
    // the run gets one in Kantor (or Automatic).
    if r.color.is_none() && r.other.contains("<color") {
        p.shown_color = Some(Rgb([0, 0, 0]));
    }
    p
}

/// A note's runs as paragraphs (lines) for the rich editor.
pub(crate) fn note_paras(runs: &[NoteRun]) -> Vec<TextPara> {
    let mut paras = vec![TextPara::new("")];
    let mut first = true;
    for r in runs {
        let props = props_of(r);
        for (i, line) in r.text.split('\n').enumerate() {
            if i > 0 {
                let mut p = TextPara::new("");
                p.runs[0].props = props.clone();
                paras.push(p);
            }
            let p = paras.last_mut().expect("a paragraph");
            if p.text.is_empty() && (first || i > 0) {
                p.runs[0].props = props.clone();
            }
            let at = p.text.len();
            p.insert(at, line, props.clone());
            first = false;
        }
    }
    paras
}

/// Strip `<tag .../>` (or `<tag ...>...</tag>`) elements named `tag` from
/// a raw run-properties fragment.
fn strip(raw: &str, tag: &str) -> String {
    let mut out = raw.to_owned();
    let open = format!("<{tag}");
    while let Some(i) = out.find(&open) {
        let next = out[i + open.len()..].chars().next();
        if !matches!(next, Some(' ' | '/' | '>')) {
            break;
        }
        let end = match out[i..].find("/>") {
            Some(e) if !out[i..i + e].contains('>') => i + e + 2,
            _ => {
                let close = format!("</{tag}>");
                out[i..]
                    .find(&close)
                    .map_or(out.len(), |e| i + e + close.len())
            }
        };
        out.replace_range(i..end, "");
    }
    out
}

fn run_of(text: String, p: &RunProps) -> NoteRun {
    // `rich` sets a font as DrawingML `<a:latin>`; notes keep it as `font`.
    let mut other = strip(&p.extra.post, "a:latin");
    if p.color.is_some() || p.shown_color.is_none() {
        other = strip(&other, "color");
    }
    NoteRun {
        text,
        bold: p.bold == Some(true),
        italic: p.italic == Some(true),
        underline: p.underline == Some(true),
        // 9 pt is what an unset size is written as.
        size: p.size.filter(|&s| s != 900),
        color: p
            .color
            .map(|Rgb([r, g, b])| (r as u32) << 16 | (g as u32) << 8 | b as u32),
        font: p.shown_font.clone(),
        other,
    }
}

/// The editor's paragraphs back as a note's runs (lines joined with line
/// breaks, neighbours with the same formatting merged).
pub(crate) fn paras_runs(paras: &[TextPara]) -> Vec<NoteRun> {
    let mut out: Vec<NoteRun> = Vec::new();
    let mut push = |r: NoteRun| match out.last_mut() {
        Some(last)
            if NoteRun {
                text: String::new(),
                ..last.clone()
            } == NoteRun {
                text: String::new(),
                ..r.clone()
            } =>
        {
            last.text.push_str(&r.text)
        }
        _ => out.push(r),
    };
    for (i, p) in paras.iter().enumerate() {
        if i > 0 {
            // The line break takes the formatting where the line starts.
            let props = p.runs.first().map(|r| r.props.clone()).unwrap_or_default();
            push(run_of("\n".into(), &props));
        }
        for (range, run) in p.run_ranges() {
            if !range.is_empty() {
                push(run_of(p.text[range].replace(LINE_BREAK, "\n"), &run.props));
            }
        }
    }
    if out.is_empty() {
        out.push(NoteRun::default());
    }
    out
}

/// Kantor's look for note boxes: fill, outline and text colours (`RRGGBB`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NoteColors {
    pub fill: u32,
    pub line: u32,
    pub text: u32,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The note's box as a worksheet text box element, for the drawing layer
/// to draw like a shape: Kantor's colours, the note's runs (9 pt unless
/// they say), top-left text.
pub(crate) fn note_shape_xml(note: &CellNote, c: NoteColors) -> String {
    let mut paras = String::from("<a:p>");
    let runs = note_runs(note);
    // A run's properties as `<{tag} ...>...</{tag}>`.
    let props = |r: &NoteRun, tag: &str| {
        let mut a = format!(" lang=\"en-US\" sz=\"{}\"", r.size.unwrap_or(900));
        if r.bold {
            a.push_str(" b=\"1\"");
        }
        if r.italic {
            a.push_str(" i=\"1\"");
        }
        if r.underline {
            a.push_str(" u=\"sng\"");
        }
        let font = r
            .font
            .as_deref()
            .map(|f| format!("<a:latin typeface=\"{}\"/>", esc(f)))
            .unwrap_or_default();
        format!(
            "<a:{tag}{a}><a:solidFill><a:srgbClr val=\"{:06X}\"/></a:solidFill>{font}</a:{tag}>",
            r.color.unwrap_or(c.text)
        )
    };
    for r in &runs {
        for (i, line) in r.text.split('\n').enumerate() {
            if i > 0 {
                paras.push_str(&props(r, "endParaRPr"));
                paras.push_str("</a:p><a:p>");
            }
            if !line.is_empty() {
                paras.push_str(&format!(
                    "<a:r>{}<a:t>{}</a:t></a:r>",
                    props(r, "rPr"),
                    esc(line)
                ));
            }
        }
    }
    paras.push_str("</a:p>");
    format!(
        "<xdr:sp xmlns:xdr=\"http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><xdr:nvSpPr><xdr:cNvPr id=\"0\" name=\"Note\"/><xdr:cNvSpPr txBox=\"1\"/></xdr:nvSpPr><xdr:spPr><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val=\"{:06X}\"/></a:solidFill><a:ln w=\"9525\"><a:solidFill><a:srgbClr val=\"{:06X}\"/></a:solidFill></a:ln></xdr:spPr><xdr:txBody><a:bodyPr wrap=\"square\" lIns=\"54864\" tIns=\"36576\" rIns=\"54864\" bIns=\"36576\" anchor=\"t\"/><a:lstStyle/>{paras}</xdr:txBody></xdr:sp>",
        c.fill, c.line
    )
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
        let had_runs = note.as_ref().is_some_and(|n| n.runs.is_some());
        let note = note.unwrap_or_else(|| {
            let author = kantor_core::settings::Settings::load().author_name();
            let author = author.trim().to_owned();
            CellNote {
                row,
                col,
                text: if author.is_empty() {
                    String::new()
                } else {
                    format!("{author}:\n")
                },
                author,
                ..CellNote::default()
            }
        });
        let paras = note_paras(&note_runs(&note));
        let text = rich::plain(&paras);
        let input = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx).auto_grow(4, 12);
            s.set_value(text.clone(), window, cx);
            s
        });
        let sub = cx.subscribe_in(&input, window, |this, _, e: &input::InputEvent, _, cx| {
            if let input::InputEvent::Change = e
                && let Some(ed) = this.note_edit.as_mut()
            {
                let text = ed.input.read(cx).value().replace("\r\n", "\n");
                if rich::plain(&ed.paras) != text {
                    slides_engine::text_ops::replace_changed(&mut ed.paras, &text);
                }
                cx.notify();
            }
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
            orig: paras.clone(),
            paras,
            had_runs,
            _sub: sub,
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
        let runs = paras_runs(&ed.paras);
        let res = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            grid.note_editing = None;
            let before = grid.book.revision();
            let exists = grid.book.note_at(ed.sheet, ed.row, ed.col);
            let note_author = exists
                .as_ref()
                .map_or(author.as_str(), |n| n.author.as_str());
            let r = if exists.is_some() && ed.paras == ed.orig {
                Ok(())
            } else if ed.had_runs || !NoteRun::is_default(&runs, note_author) {
                // Formatting of its own: kept as runs.
                grid.book
                    .set_note_runs(ed.sheet, ed.row, ed.col, runs, &author)
            } else {
                grid.book.set_note(ed.sheet, ed.row, ed.col, &text, &author)
            }
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

    /// A toolbar command on the editor's selection (all the text without
    /// one).
    fn note_rich_op(&mut self, op: RichOp, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.note_edit.as_mut() else {
            return;
        };
        let range = ed.input.read(cx).selected_range();
        let sel = rich::selection(&ed.paras, range);
        rich::apply(&mut ed.paras, sel, op);
        let input = ed.input.clone();
        input.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    /// The editor box, at the note's box (it follows the sheet's scroll).
    pub(crate) fn render_note_editor(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ed = self.note_edit.as_ref()?;
        let t = kantor_ui::tokens(cx);
        let anchor = self.table.read(cx).delegate().note_anchor.get();
        let at = match (self.drawings.borrow().note_edit_rect, anchor) {
            (Some(b), _) => b.origin,
            (None, Some(b)) => point(b.origin.x + b.size.width + GAP, b.origin.y),
            (None, None) => point(SPACE_7 * 4., SPACE_7 * 4.),
        };
        let state = RichState::at(
            &ed.paras,
            rich::selection(&ed.paras, ed.input.read(cx).selected_range()),
        );
        let view = cx.entity().downgrade();
        let bar = rich::toolbar(
            "note-rich",
            &state,
            false,
            Rc::new(move |op, w, cx| {
                let _ = view.update(cx, |this, cx| this.note_rich_op(op, w, cx));
            }),
            cx,
        );
        let label = tf("Note on {}", &[&sheets_engine::cell_name(ed.row, ed.col)]);
        // The text types at the sheet's zoom, as the box it fills shows it.
        let zoom = crate::look::zoom(&self.table.read(cx).delegate().book, ed.sheet);
        let panel = div()
            .id("sheets-note-editor")
            .key_context(NOTE_CONTEXT)
            .occlude()
            .w(px(340.) * zoom.max(1.0))
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
            .child(div().text_color(t.fg_muted).child(label))
            .child(bar)
            .child(
                Textarea::new(&ed.input)
                    .small()
                    .text_size(FONT_SMALL * zoom),
            );
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

    fn note(author: &str, text: &str) -> CellNote {
        CellNote {
            author: author.into(),
            text: text.into(),
            ..CellNote::default()
        }
    }

    #[std::prelude::v1::test]
    fn plain_notes_get_a_bold_author_run() {
        let r = note_runs(&note("Dewi", "Dewi:\nCheck it"));
        assert_eq!(r.len(), 2);
        assert!(r[0].bold && r[0].text == "Dewi:");
        assert!(!r[1].bold && r[1].text == "\nCheck it");
        assert_eq!(note_runs(&note("", ":x"))[0].text, ":x");
        assert!(NoteRun::is_default(&r, "Dewi"));
    }

    #[std::prelude::v1::test]
    fn runs_round_trip_through_the_editor() {
        let runs = vec![
            NoteRun {
                text: "Dewi:".into(),
                bold: true,
                other: "<family val=\"2\"/>".into(),
                ..NoteRun::default()
            },
            NoteRun {
                text: "\nRed ".into(),
                color: Some(0xFF0000),
                ..NoteRun::default()
            },
            NoteRun {
                text: "word".into(),
                italic: true,
                color: Some(0xFF0000),
                ..NoteRun::default()
            },
        ];
        let paras = note_paras(&runs);
        assert_eq!(paras.len(), 2);
        assert_eq!(rich::plain(&paras), "Dewi:\nRed word");
        assert_eq!(paras_runs(&paras), runs);
        // Typing keeps the runs; formatting a word adds one.
        let mut paras = paras;
        slides_engine::text_ops::replace_changed(&mut paras, "Dewi:\nRed word!");
        let sel = rich::selection(&paras, 6..9);
        rich::apply(&mut paras, sel, RichOp::Underline(true));
        let sel = rich::selection(&paras, 0..2);
        rich::apply(&mut paras, sel, RichOp::Font("Arial"));
        let back = paras_runs(&paras);
        assert_eq!(
            back.iter().map(|r| r.text.as_str()).collect::<String>(),
            "Dewi:\nRed word!"
        );
        assert!(back.iter().any(|r| r.underline && r.text == "\nRed"));
        assert_eq!(back[0].font.as_deref(), Some("Arial"));
        assert!(!back[0].other.contains("latin"));
        assert!(back.last().unwrap().italic);
        // A theme colour stays until Automatic is picked.
        let themed = vec![NoteRun {
            text: "x".into(),
            other: "<color theme=\"1\"/>".into(),
            ..NoteRun::default()
        }];
        let mut p = note_paras(&themed);
        assert_eq!(paras_runs(&p), themed);
        let sel = rich::selection(&p, 0..0);
        rich::apply(&mut p, sel, RichOp::Color(None));
        assert_eq!(paras_runs(&p)[0].other, "");
    }

    #[std::prelude::v1::test]
    fn the_box_is_a_text_box_shape() {
        let n = note("Dewi", "Dewi:\nTwo\nlines & more");
        let c = NoteColors {
            fill: 0x223344,
            line: 0x556677,
            text: 0xEEEEEE,
        };
        let x = note_shape_xml(&n, c);
        assert_eq!(x.matches("<a:p>").count(), 3);
        assert!(x.contains("b=\"1\"><a:solidFill><a:srgbClr val=\"EEEEEE\"/>"));
        assert!(x.contains("<a:t>lines &amp; more</a:t>"));
        assert!(x.contains("<a:srgbClr val=\"223344\"/>"));
        assert_eq!(slides_engine::sheet_shape::text_paras(&x).unwrap().len(), 3);
    }
}
