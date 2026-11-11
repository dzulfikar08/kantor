//! Slow work kept off the UI thread: opening a file and recalculating.
//!
//! A big `.xlsx` takes seconds to parse and can take minutes to recalculate,
//! so both run on the background executor. Meanwhile the pane shows what is
//! going on (file name, size, time so far) instead of the grid, and nothing
//! can edit the workbook. Opening can be cancelled (the pane closes);
//! a recalculation can't be interrupted, but closing the pane drops it.
//!
//! Calculation follows Excel: *Automatic* recalculates after every edit,
//! *Manual* only on F9 (Shift+F9 too: IronCalc can only recalculate the whole
//! workbook). Workbooks with many formulas open in Manual (see
//! [`sheets_engine::MANUAL_CALC_FORMULAS`]).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Sizable as _, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::{metrics::*, tokens};
use sheets_engine::{CalcMode, Workbook};

use crate::{GRID_CONTEXT, KEY_CONTEXT, SheetGrid, SheetsView, opened_status};

actions!(sheets, [Recalculate, RecalculateSheet]);

/// Workbooks with up to this many formula cells recalculate on the UI thread
/// (in well under a frame); bigger ones in the background.
const INLINE_CALC_FORMULAS: usize = 5_000;

pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("f9", Recalculate, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-f9", RecalculateSheet, Some(KEY_CONTEXT)),
    ]);
}

/// Something running in the background; the pane waits for it.
pub(crate) struct Busy {
    what: Job,
    started: Instant,
    /// The document as it was when the job started (the grid holds a
    /// stand-in meanwhile): its name, file and whether it had unsaved edits.
    pub(crate) name: String,
    pub(crate) path: Option<PathBuf>,
    pub(crate) dirty: bool,
    _task: Task<()>,
}

pub(crate) enum Job {
    Opening {
        name: String,
        bytes: u64,
    },
    Calculating,
    /// An edit IronCalc follows with a recalculation in any mode (fill).
    Editing(&'static str),
}

impl Job {
    fn title(&self, doc: &str) -> String {
        match self {
            Job::Opening { name, .. } => format!("Opening {name}…"),
            Job::Calculating => format!("Calculating {doc}…"),
            Job::Editing(what) => format!("{what} and recalculating {doc}…"),
        }
    }
}

/// What the view asks of the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetsEvent {
    /// Close this pane (opening was cancelled).
    Close,
}

impl EventEmitter<SheetsEvent> for SheetsView {}

/// "28 MB", "640 KB".
fn size_text(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    if bytes >= MB {
        format!("{} MB", (bytes + MB / 2) / MB)
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

fn manual_note(book: &Workbook) -> Option<String> {
    (book.calc_mode() == CalcMode::Manual).then(|| {
        format!(
            "Manual calculation ({} formulas) — press F9 to recalculate",
            book.formula_count()
        )
    })
}

/// A workbook that goes away (its pane closed, or a stand-in replaced) frees
/// its memory, which the allocator would otherwise keep: a closed 180k-row
/// workbook left 180 MB resident. Give it back shortly after, off the UI
/// thread.
impl Drop for SheetGrid {
    fn drop(&mut self) {
        kantor_mem::release_free_memory_soon(Duration::from_millis(500));
    }
}

impl SheetsView {
    /// Whether the workbook is being opened or recalculated (no edits).
    pub fn is_busy(&self) -> bool {
        self.busy.is_some()
    }

    /// Parse `path` on the background executor, then show it.
    pub(crate) fn start_open(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let parse = cx.background_spawn({
            let path = path.clone();
            async move {
                let book = Workbook::open(&path);
                // Parsing freed what it read the file with.
                kantor_mem::release_free_memory();
                book
            }
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = parse.await;
            let _ = this.update_in(cx, |this, window, cx| this.finish_open(result, window, cx));
        });
        self.set_editing(None, cx);
        self.status = format!("Opening {name}…").into();
        self.opening = Some(path.clone());
        let what = Job::Opening {
            name: name.clone(),
            bytes,
        };
        self.busy = Some(Busy {
            what,
            started: Instant::now(),
            name,
            path: Some(path),
            dirty: false,
            _task: task,
        });
        cx.notify();
    }

    fn finish_open(
        &mut self,
        result: anyhow::Result<Workbook>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let had_focus = self.busy_focus.is_focused(window);
        let took = self.busy.take().map(|b| b.started.elapsed());
        match result {
            Ok(book) => {
                log::info!(
                    "opened {} in {:.2?}: {} formula cells, {:?} calculation",
                    book.title(),
                    took.unwrap_or_default(),
                    book.formula_count(),
                    book.calc_mode()
                );
                self.opening = None;
                let mut status = opened_status(&book);
                if let Some(took) = took.filter(|t| *t >= Duration::from_secs(1)) {
                    status.push_str(&format!(" in {:.1} s", took.as_secs_f32()));
                }
                if let Some(note) = manual_note(&book) {
                    status.push_str(". ");
                    status.push_str(&note);
                }
                self.status = status.into();
                let view = cx.entity().downgrade();
                self.table.update(cx, |table, cx| {
                    *table.delegate_mut() = SheetGrid::new(book, view);
                    table.refresh(cx);
                    cx.notify();
                });
                self.select_cell(0, 0, cx);
                self.load_selected_into_formula(window, cx);
            }
            Err(err) => {
                log::error!("{err:#}");
                self.status = format!("{err:#}").into();
            }
        }
        if had_focus {
            self.focus_grid(window, cx);
        }
        self.update_title(window, cx);
        cx.notify();
    }

    /// Cancel an open in progress: the pane closes.
    fn cancel_open(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.busy,
            Some(Busy {
                what: Job::Opening { .. },
                ..
            })
        ) {
            // Dropping the task discards the parse's result.
            self.busy = None;
            cx.emit(SheetsEvent::Close);
        }
    }

    /// F9: recalculate every formula. Big workbooks recalculate in the
    /// background while the pane waits.
    pub fn calculate_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        if self.editing.is_some() {
            self.commit_edit(None, window, cx);
        }
        self.run_book_job(
            Job::Calculating,
            |book| book.recalculate(),
            |this, (), took, _, _| {
                this.status = match took {
                    Some(t) => format!("Calculated in {:.1} s", t.as_secs_f32()).into(),
                    None => "Calculated".into(),
                };
            },
            window,
            cx,
        );
    }

    /// Run `work` on the workbook, then `done` with its result and how long
    /// it took (`None` when it ran inline). Workbooks with few formulas run it
    /// right here; others move to the background executor (a [`Workbook`] is
    /// `Send`) while the pane shows `what`, and come back when it is done.
    pub(crate) fn run_book_job<R: Send + 'static>(
        &mut self,
        what: Job,
        work: impl FnOnce(&mut Workbook) -> R + Send + 'static,
        done: impl FnOnce(&mut Self, R, Option<Duration>, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let small = self.table.read(cx).delegate().book.formula_count() <= INLINE_CALC_FORMULAS;
        if small {
            let result = self.table.update(cx, |table, cx| {
                let grid = table.delegate_mut();
                let res = work(&mut grid.book);
                grid.resize();
                table.refresh(cx);
                cx.notify();
                res
            });
            done(self, result, None, window, cx);
            self.load_selected_into_formula(window, cx);
            self.update_title(window, cx);
            cx.notify();
            return;
        }
        let Ok(stand_in) = Workbook::new_empty() else {
            return;
        };
        let (name, path, dirty) = (self.doc_name(cx), self.path(cx), self.is_dirty(cx));
        let mut book = self.table.update(cx, |table, _| {
            std::mem::replace(&mut table.delegate_mut().book, stand_in)
        });
        let had_focus = self.grid_focused(window, cx);
        let run = cx.background_spawn(async move {
            let result = work(&mut book);
            // A recalculation frees its bookkeeping when it ends.
            kantor_mem::release_free_memory();
            (book, result)
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            let (book, result) = run.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let took = this.busy.take().map(|b| b.started.elapsed());
                this.drawings.borrow_mut().clear();
                this.table.update(cx, |table, cx| {
                    let grid = table.delegate_mut();
                    grid.book = book;
                    grid.resize();
                    table.refresh(cx);
                    cx.notify();
                });
                done(this, result, took, window, cx);
                if this.busy_focus.is_focused(window) {
                    this.focus_grid(window, cx);
                }
                this.load_selected_into_formula(window, cx);
                this.update_title(window, cx);
                cx.notify();
            });
        });
        self.status = what.title(&name).into();
        self.busy = Some(Busy {
            what,
            started: Instant::now(),
            name,
            path,
            dirty,
            _task: task,
        });
        if had_focus {
            window.focus(&self.busy_focus, cx);
        }
        cx.notify();
    }

    /// Switch between automatic and manual calculation. Going back to
    /// automatic recalculates if values are out of date.
    pub fn set_calc_mode_doc(
        &mut self,
        mode: CalcMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy.is_some() {
            return;
        }
        let stale = self.table.update(cx, |table, _| {
            let book = &mut table.delegate_mut().book;
            book.set_calc_mode(mode);
            book.needs_recalc()
        });
        self.status = match mode {
            CalcMode::Automatic => "Automatic calculation".into(),
            CalcMode::Manual => "Manual calculation — press F9 to recalculate".into(),
        };
        cx.notify();
        if mode == CalcMode::Automatic && stale {
            self.calculate_doc(window, cx);
        }
    }

    /// The calculation mode, when it is worth showing in the status bar:
    /// manual, or being recalculated.
    pub fn calc_status(&self, cx: &App) -> Option<String> {
        match self.busy.as_ref().map(|b| &b.what) {
            Some(Job::Opening { .. }) => return None,
            Some(_) => return Some("Calculating…".into()),
            None => {}
        }
        let book = &self.table.read(cx).delegate().book;
        match book.calc_mode() {
            CalcMode::Automatic => None,
            CalcMode::Manual if book.needs_recalc() => Some("Manual · Calculate (F9)".into()),
            CalcMode::Manual => Some("Manual calculation".into()),
        }
    }

    fn grid_focused(&self, window: &Window, cx: &App) -> bool {
        self.table
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
    }

    pub(crate) fn calc_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &Recalculate, w, cx| this.calculate_doc(w, cx)))
            .on_action(cx.listener(|this, _: &RecalculateSheet, w, cx| this.calculate_doc(w, cx)))
    }

    /// The pane while the workbook is being opened or recalculated.
    pub(crate) fn render_busy(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let busy = self.busy.as_ref()?;
        let t = tokens(cx);
        let secs = busy.started.elapsed().as_secs();
        let title = busy.what.title(&busy.name);
        let detail = match &busy.what {
            Job::Opening { bytes, .. } => format!("{} · {secs} s", size_text(*bytes)),
            Job::Calculating | Job::Editing(_) => {
                format!("{secs} s · editing resumes when it is done")
            }
        };
        let cancel = matches!(busy.what, Job::Opening { .. }).then(|| {
            Button::new("sheets-cancel-open")
                .label("Cancel")
                .ghost()
                .small()
                .on_click(cx.listener(|this, _, _, cx| this.cancel_open(cx)))
        });
        // The spinner's animation redraws the view every frame, which also
        // moves the time on.
        Some(
            v_flex()
                .id("sheets-busy")
                .track_focus(&self.busy_focus)
                .key_context(GRID_CONTEXT)
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .gap(SPACE_4)
                .bg(t.bg)
                .child(Spinner::new().large().color(t.fg_muted))
                .child(
                    div()
                        .text_size(FONT_EMPHASIS)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.fg)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(FONT_SMALL)
                        .text_color(t.fg_muted)
                        .child(detail),
                )
                .children(cancel)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn sizes() {
        assert_eq!(size_text(29_483_596), "28 MB");
        assert_eq!(size_text(1000), "1 KB");
        assert_eq!(size_text(5 * 1024 * 1024 + 1), "5 MB");
    }
}
