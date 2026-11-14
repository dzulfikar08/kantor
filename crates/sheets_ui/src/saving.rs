//! Save and Save As without freezing the window: the workbook is copied
//! here (tens of milliseconds even for big workbooks, see
//! `sheets_engine::SaveJob`), the file is built and written in the
//! background, and the save is recorded when it's on disk. Editing goes on
//! meanwhile; edits made after the copy keep the workbook unsaved.
//!
//! Only one save runs at a time (autosave included). Save pressed during a
//! save saves again once it's done; closing waits for it.

use std::path::{Path, PathBuf};
use std::time::Instant;

use futures::channel::oneshot;
use gpui_kit::*;
use kantor_ui::i18n::{self, tf, tk};
use sheets_engine::{Saved, SourceFormat};

use crate::{SheetsEvent, SheetsView};

/// How a save ended, for whoever waits on it: `Ok(clean)` once written
/// (`clean`: nothing changed since), or the error.
pub(crate) type Outcome = Result<bool, String>;

/// A save being written in the background.
pub(crate) struct Saving {
    path: PathBuf,
    /// The workbook state being written.
    revision: (u64, u64),
    started: Instant,
    autosave: bool,
    /// Save pressed again meanwhile: save the latest state when done.
    again: bool,
    waiters: Vec<oneshot::Sender<Outcome>>,
    _task: Task<()>,
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl SheetsView {
    /// Whether a save is being written (closing must wait for it).
    pub fn is_saving(&self) -> bool {
        self.saving.is_some()
    }

    /// "Saving…" while a save the user asked for is being written.
    pub(crate) fn saving_status(&self) -> Option<String> {
        self.saving
            .as_ref()
            .filter(|s| !s.autosave)
            .map(|_| i18n::t("Saving…").to_owned())
    }

    /// A save is running and nothing changed since its snapshot (closing
    /// only has to wait for it).
    pub fn save_holds_all_changes(&self, cx: &App) -> bool {
        self.busy.is_none()
            && self.saving.as_ref().is_some_and(|s| {
                !s.again && s.revision == self.table.read(cx).delegate().book.revision()
            })
    }

    /// Resolves once no save is running (at once if none is).
    pub fn saved_signal(&mut self) -> Option<oneshot::Receiver<Outcome>> {
        let saving = self.saving.as_mut()?;
        let (tx, rx) = oneshot::channel();
        saving.waiters.push(tx);
        Some(rx)
    }

    /// Start saving to `path` in the background. Returns a receiver for the
    /// outcome, or `None` if the save couldn't start (the status says why).
    pub(crate) fn start_save(
        &mut self,
        path: PathBuf,
        autosave: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<oneshot::Receiver<Outcome>> {
        if let Some(saving) = &mut self.saving {
            if !autosave && saving.path == path {
                saving.again = true;
                let msg = i18n::t("Saving {}… (then again with your latest changes)");
                self.status = i18n::fill(msg, &[&file_name(&path)]).into();
                cx.notify();
                return self.saved_signal();
            }
            if !autosave {
                self.status = i18n::t("Please wait until the save finishes").into();
                cx.notify();
            }
            return None;
        }
        let t = Instant::now();
        let job = self.table.read(cx).delegate().book.prepare_save(&path);
        let job = match job {
            Ok(job) => job,
            Err(err) => {
                self.save_failed(&path, autosave, &format!("{err:#}"), cx);
                return None;
            }
        };
        log::info!(
            "save snapshot of {} took {:.1?} on the UI thread",
            path.display(),
            t.elapsed()
        );
        let write = cx.background_spawn(async move {
            let t = Instant::now();
            let result = job.run();
            (result, t.elapsed())
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            let (result, took) = write.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.save_done(result, took, window, cx)
            });
        });
        let (tx, rx) = oneshot::channel();
        let revision = self.table.read(cx).delegate().book.revision();
        self.saving = Some(Saving {
            path: path.clone(),
            revision,
            started: t,
            autosave,
            again: false,
            waiters: vec![tx],
            _task: task,
        });
        if !autosave {
            self.status = tf("Saving {}…", &[&file_name(&path)]).into();
        }
        cx.notify();
        Some(rx)
    }

    fn save_done(
        &mut self,
        result: anyhow::Result<Saved>,
        took: std::time::Duration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(saving) = self.saving.take() else {
            return;
        };
        let outcome = match result {
            Ok(saved) => {
                log::info!(
                    "saved {} in {took:.1?} in the background ({:.1?} in all)",
                    saved.path().display(),
                    saving.started.elapsed()
                );
                self.disk_guard = None;
                if self.busy.is_some() {
                    // The workbook is in the background (recalculating);
                    // the save is recorded when it's back (see `calc`).
                    self.pending_save = Some(saved);
                    // Closing waits for the recalculation anyway.
                    Ok(false)
                } else {
                    self.table
                        .update(cx, |table, _| table.delegate_mut().book.finish_save(saved));
                    self.status = self.saved_status(saving.autosave, cx).into();
                    Ok(!self.is_dirty(cx))
                }
            }
            Err(err) => {
                let msg = format!("{err:#}");
                self.save_failed(&saving.path, saving.autosave, &msg, cx);
                Err(msg)
            }
        };
        self.update_title(window, cx);
        cx.notify();
        let again = saving.again && matches!(outcome, Ok(false)) && self.busy.is_none();
        if again && let Some(rx) = self.start_save(saving.path.clone(), false, window, cx) {
            // The waiters get the second save's outcome.
            let waiters = saving.waiters;
            cx.background_spawn(async move {
                let outcome = rx.await.unwrap_or(Err("cancelled".into()));
                for w in waiters {
                    let _ = w.send(outcome.clone());
                }
            })
            .detach();
            return;
        }
        for w in saving.waiters {
            let _ = w.send(outcome.clone());
        }
    }

    /// The status line after a save, in the interface language.
    pub(crate) fn saved_status(&self, autosave: bool, cx: &App) -> String {
        self.saved_message(autosave, i18n::t, cx)
    }

    /// The same in English, for MCP agents.
    pub(crate) fn saved_status_english(&self, cx: &App) -> String {
        self.saved_message(false, |en| en, cx)
    }

    fn saved_message(
        &self,
        autosave: bool,
        tr: fn(&'static str) -> &'static str,
        cx: &App,
    ) -> String {
        let book = &self.table.read(cx).delegate().book;
        if autosave {
            return i18n::fill(tr(tk("Autosaved {}")), &[&book.title()]);
        }
        let lost = book.sheets_lost_in_csv();
        if matches!(book.source_format(), SourceFormat::Csv { .. }) && lost > 0 {
            let sheet = book.active_sheet() as usize;
            let name = book.sheet_names().get(sheet).cloned().unwrap_or_default();
            let msg = tk("Saved {} with values of sheet \"{}\" only; CSV can't hold the other {}");
            i18n::fill(tr(msg), &[&book.title(), &name, &lost])
        } else {
            i18n::fill(tr(tk("Saved {}")), &[&book.title()])
        }
    }

    /// Nothing was replaced on disk and the workbook keeps its changes: say
    /// so (status bar, and a notice for a Save the user asked for).
    fn save_failed(&mut self, path: &Path, autosave: bool, err: &str, cx: &mut Context<Self>) {
        log::warn!("saving {}: {err}", path.display());
        if autosave {
            return;
        }
        self.status = tf("Save failed: {}", &[&err]).into();
        let msg = i18n::t("Couldn't save “{}”: {}. Your changes are still open; try Save As.");
        cx.emit(SheetsEvent::Notice(i18n::fill(
            msg,
            &[&file_name(path), &err],
        )));
        cx.notify();
    }
}
