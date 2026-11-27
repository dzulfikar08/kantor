//! Macro events from what the user does (see `sheets_script::events` for
//! the handlers and their order).
//!
//! Every hook asks the project's [`sheets_script::EventIndex`] first (kept
//! on the UI thread): without a handler for the event nothing happens, so
//! typing, clicking and saving cost what they cost before. With one, the
//! event runs on the macro thread like a macro (`macro_run`: the 150 ms
//! wait before the busy pane, Stop, dialogs).
//!
//! - **Change**: edits, pastes, fills, clears, undo and redo all end in
//!   `update_title`, which looks at the undo history (`Workbook::history_lens`)
//!   and fires `Change` with the cells the step changed. The edit and what
//!   its handlers change fold into one undo step; after an undo the handlers'
//!   own changes are a step of their own.
//! - **SelectionChange** when the active cell or range moves, **Deactivate**
//!   and **Activate** when another sheet is picked, **NewSheet** for an
//!   inserted sheet, **Calculate** after an edit recalculates (automatic
//!   calculation) and after F9.
//! - **BeforeDoubleClick**, **BeforeSave**, **BeforeClose** wait for their
//!   handlers: the edit, save or close goes on only if none set `Cancel`.
//!   **BeforeRightClick** runs while the menu is about to open: when its
//!   handlers end within the 150 ms wait, `Cancel` stops the menu; a slower
//!   handler goes on with the busy pane and the menu doesn't open.
//! - **Open** after Enable Content, **AfterSave** after a save.
//! - `Application.OnTime` schedules wait in a timer task while the pane is
//!   open.

use std::time::Duration;

use gpui_kit::*;
use kantor_ui::i18n::t;
use sheets_engine::{CalcMode, Workbook};
use sheets_script::{Event, EventKind};

use crate::SheetsView;
use crate::calc::{Busy, Job, SheetsEvent};
use crate::macro_run::After;
use crate::macro_thread::MacroTask;
use crate::vba_ui::{MacroState, MacroTrust};

impl MacroState {
    /// Whether a handler exists for `kind` (content enabled).
    pub(crate) fn wants(&self, kind: EventKind) -> bool {
        self.trust == MacroTrust::Enabled
            && self.thread.as_ref().is_some_and(|t| t.index.wants(kind))
    }

    /// Whether `update_title` should look for events afterwards.
    pub(crate) fn wants_after_edit(&self) -> bool {
        !self.pending.is_empty()
            || self.wants(EventKind::Change)
            || self.wants(EventKind::Calculate)
    }
}

fn label(kind: EventKind) -> String {
    format!("{kind:?}")
}

impl SheetsView {
    /// Fires `event` (handlers run on the macro thread) with nothing to do
    /// afterwards. `false`: no handler, or the workbook is busy.
    pub(crate) fn fire_macro_event(
        &mut self,
        event: Event,
        fold_from: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.fire_event_then(event, After::Nothing, fold_from, window, cx)
    }

    pub(crate) fn fire_event_then(
        &mut self,
        event: Event,
        after: After,
        fold_from: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let kind = event.kind();
        if !self.macros.wants(kind) {
            return false;
        }
        if self.busy.is_some() || self.macros.run.is_some() {
            // Later, when the workbook is back (events that wait for an
            // answer don't queue: what they guard goes on).
            if matches!(after, After::Nothing) {
                self.macros.pending.push((event, fold_from));
            }
            return false;
        }
        self.start_work(
            MacroTask::Fire { event, fold_from },
            label(kind),
            after,
            window,
            cx,
        )
    }

    /// Events wait their turn: fire `events` one after another.
    pub(crate) fn queue_macro_events(
        &mut self,
        events: Vec<Event>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for e in events {
            if self.macros.wants(e.kind()) {
                self.macros.pending.push((e, None));
            }
        }
        self.macro_after_edit(window, cx);
    }

    /// What the run left is what events have seen.
    pub(crate) fn macro_events_seen(&mut self, cx: &App) {
        let grid = self.table.read(cx).delegate();
        self.macros.seen_hist = Some(grid.book.history_lens());
        self.macros.seen_sel = Some((grid.sheet, grid.range()));
    }

    /// After an edit, undo or redo (from `update_title`): `Change` for the
    /// cells the step changed (then `Calculate`), ahead of the events
    /// waiting; then the next of those.
    pub(crate) fn macro_after_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() || self.macros.run.is_some() || self.macros.lent {
            return;
        }
        let edit = self.unseen_edit_events(cx);
        self.macros.pending.splice(0..0, edit);
        if self.macros.pending.is_empty() {
            return;
        }
        let (event, fold_from) = self.macros.pending.remove(0);
        if !self.fire_macro_event(event, fold_from, window, cx) {
            // Nobody handles it after all: go on with the next.
            cx.defer_in(window, |this, window, cx| this.macro_after_edit(window, cx));
        }
    }

    /// The events of the edits, undos and redos events haven't seen yet
    /// (`Change` per sheet, the first folding into the edit's undo step,
    /// then `Calculate`), those with handlers; they count as seen now.
    pub(crate) fn unseen_edit_events(&mut self, cx: &App) -> Vec<(Event, Option<usize>)> {
        let (now, changes, sheet, automatic) = {
            let grid = self.table.read(cx).delegate();
            let book = &grid.book;
            let now = book.history_lens();
            let edited = match self.macros.seen_hist {
                Some(prev) if now.0 > prev.0 => Some((book.edited_since(prev.0), Some(prev.0))),
                Some(prev) if now.0 < prev.0 => Some((book.undone_last(prev.0 - now.0), None)),
                _ => None,
            };
            let changes = edited
                .map(|(cells, fold)| (sheets_script::events::change_events(book, &cells), fold));
            (
                now,
                changes,
                grid.sheet,
                book.calc_mode() == CalcMode::Automatic,
            )
        };
        self.macros.seen_hist = Some(now);
        let Some((events, fold)) = changes else {
            return Vec::new();
        };
        if events.is_empty() {
            return Vec::new();
        }
        // An edit after a close was let through: ask again next time.
        self.macros.close_approved = false;
        let mut out = Vec::new();
        if self.macros.wants(EventKind::Change) {
            for (i, e) in events.into_iter().enumerate() {
                out.push((e, if i == 0 { fold } else { None }));
            }
        }
        if automatic && self.macros.wants(EventKind::Calculate) {
            out.push((Event::Calculate { sheet }, None));
        }
        out
    }

    /// The active cell or range moved.
    pub(crate) fn macro_selection_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.macros.wants(EventKind::SelectionChange) || self.editing.is_some() {
            return;
        }
        let (sheet, range) = {
            let grid = self.table.read(cx).delegate();
            (grid.sheet, grid.range())
        };
        if self.macros.seen_sel == Some((sheet, range)) {
            return;
        }
        self.macros.seen_sel = Some((sheet, range));
        // Behind the `Change` of an edit Enter or Tab just ended (its check
        // is deferred), as in Excel: the edit's event isn't lost to this run.
        self.macros
            .pending
            .push((Event::SelectionChange { sheet, range }, None));
        self.macro_after_edit(window, cx);
    }

    /// Another sheet was picked: `Deactivate`, then `Activate`.
    pub(crate) fn macro_sheet_switched(
        &mut self,
        old: u32,
        new: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if old == new {
            return;
        }
        let range = self.table.read(cx).delegate().range();
        self.macros.seen_sel = Some((new, range));
        self.queue_macro_events(
            vec![
                Event::Deactivate { sheet: old },
                Event::Activate { sheet: new },
            ],
            window,
            cx,
        );
    }

    /// A double-click on a cell: `true` when `BeforeDoubleClick` runs first
    /// (the edit starts afterwards unless cancelled).
    pub(crate) fn macro_double_click(
        &mut self,
        cell: (u32, u32),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.macros.wants(EventKind::BeforeDoubleClick) {
            return false;
        }
        let sheet = self.table.read(cx).delegate().sheet;
        self.fire_event_then(
            Event::BeforeDoubleClick {
                sheet,
                row: cell.0,
                col: cell.1,
            },
            After::Edit(cell),
            None,
            window,
            cx,
        )
    }

    /// Save or Save As: `true` when `BeforeSave` runs first (the save goes
    /// on afterwards unless cancelled).
    pub(crate) fn macro_before_save(
        &mut self,
        as_ui: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if std::mem::take(&mut self.macros.save_approved)
            || !self.macros.wants(EventKind::BeforeSave)
        {
            return false;
        }
        self.fire_event_then(
            Event::BeforeSave { save_as_ui: as_ui },
            After::Save { as_ui },
            None,
            window,
            cx,
        )
    }

    /// A save (not an autosave) ended.
    pub(crate) fn macro_after_save(
        &mut self,
        success: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.macros.wants(EventKind::AfterSave) {
            self.queue_macro_events(vec![Event::AfterSave { success }], window, cx);
        }
    }

    /// Whether closing this pane should run `Workbook_BeforeClose` first.
    pub fn needs_before_close(&self) -> bool {
        self.macros.wants(EventKind::BeforeClose) && !self.macros.close_approved
    }

    /// Runs `Workbook_BeforeClose`; afterwards the view emits
    /// [`SheetsEvent::RetryClose`] or [`SheetsEvent::CloseCancelled`].
    /// `false`: it couldn't run (busy): close as usual.
    pub fn fire_before_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.fire_event_then(Event::BeforeClose, After::Close, None, window, cx)
    }

    /// What follows an event no handler cancelled.
    pub(crate) fn continue_after(
        &mut self,
        after: After,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match after {
            After::Nothing => {}
            After::Recalculate => self.calculate_doc(window, cx),
            After::Edit(cell) => {
                self.selected = cell;
                self.select_cell(cell.0, cell.1, cx);
                self.start_edit(None, window, cx);
            }
            After::Save { as_ui } => {
                self.macros.save_approved = true;
                if as_ui {
                    self.save_doc_as(window, cx);
                } else {
                    self.save_doc(window, cx);
                }
            }
            After::Close => {
                self.macros.close_approved = true;
                cx.emit(SheetsEvent::RetryClose);
            }
        }
    }

    /// `ThisWorkbook.Close [SaveChanges]`.
    pub(crate) fn macro_closes_workbook(
        &mut self,
        save: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dirty = self.is_dirty(cx);
        match save {
            Some(false) => cx.emit(SheetsEvent::Close),
            _ if !dirty => cx.emit(SheetsEvent::Close),
            Some(true) => {
                let saved = self.save_for_close(window, cx);
                cx.spawn(async move |this, cx| {
                    if saved.await {
                        let _ = this.update(cx, |_, cx| cx.emit(SheetsEvent::Close));
                    }
                })
                .detach();
            }
            None => {
                self.status =
                    t("The macro closed the workbook; it has unsaved changes, so it stays open")
                        .into();
                cx.notify();
            }
        }
    }

    /// Waits for the next `Application.OnTime` schedule (local OLE date).
    pub(crate) fn schedule_macro_timer(
        &mut self,
        at: Option<f64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = at else {
            self.macros.timer = None;
            return;
        };
        let task = cx.spawn_in(window, async move |this, cx| {
            loop {
                let left = (at - sheets_script::local_now()) * 86_400.0;
                if left <= 0.0 {
                    break;
                }
                let wait = Duration::from_secs_f64(left.clamp(0.05, 30.0));
                cx.background_executor().timer(wait).await;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                let now = sheets_script::local_now();
                if !this.start_work(
                    MacroTask::Due { now },
                    "OnTime".to_owned(),
                    After::Nothing,
                    window,
                    cx,
                ) {
                    // Busy: try again in a moment.
                    this.schedule_macro_timer(Some(now + 1.0 / 86_400.0), window, cx);
                }
            });
        });
        self.macros.timer = Some(task);
    }

    /// Lends the workbook to a macro of another pane (`Workbooks(...)`);
    /// the pane shows it's busy until [`Self::macro_returned`].
    pub fn lend_to_macro(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Workbook> {
        if self.busy.is_some()
            || self.macros.run.is_some()
            || self.macros.lent
            || self.editing.is_some()
            || self.saving.is_some()
        {
            return None;
        }
        let stand_in = Workbook::new_empty().ok()?;
        let (name, path, dirty) = (self.doc_name(cx), self.path(cx), self.is_dirty(cx));
        let book = self.table.update(cx, |table, _| {
            std::mem::replace(&mut table.delegate_mut().book, stand_in)
        });
        self.macros.lent = true;
        self.busy = Some(Busy {
            what: Job::Macro(t("a macro of another workbook").to_owned()),
            started: std::time::Instant::now(),
            name,
            path,
            dirty,
            _task: Task::ready(()),
        });
        window.focus(&self.busy_focus, cx);
        cx.notify();
        Some(book)
    }

    /// A lent workbook comes back.
    pub fn macro_returned(&mut self, book: Workbook, window: &mut Window, cx: &mut Context<Self>) {
        if !self.macros.lent {
            return;
        }
        self.macros.lent = false;
        self.busy = None;
        self.drawings.borrow_mut().clear();
        self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            grid.book = book;
            grid.resize();
            table.refresh(cx);
            cx.notify();
        });
        self.macro_events_seen(cx);
        self.load_selected_into_formula(window, cx);
        self.focus_grid(window, cx);
        self.update_title(window, cx);
        cx.notify();
    }

    /// A pane showing `book` (a workbook a macro opened or added).
    pub fn from_book(book: Workbook, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self::new(None, window, cx);
        this.finish_open(Ok(book), window, cx);
        this
    }

    /// Right-click on a cell, from the grid as its menu is about to open
    /// (the grid holds the workbook): runs `BeforeRightClick` with it,
    /// waiting up to the quick-run time. `true`: show the menu.
    pub(crate) fn macro_right_click(
        &mut self,
        book: &mut Workbook,
        sheet: u32,
        range: sheets_engine::CellRange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.macros.wants(EventKind::BeforeRightClick)
            || self.busy.is_some()
            || self.macros.run.is_some()
        {
            return true;
        }
        match self.fire_in_place(book, Event::BeforeRightClick { sheet, range }, window, cx) {
            Some(cancel) => !cancel,
            None => false,
        }
    }
}
