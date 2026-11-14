//! What the workspace's recovery copies and autosave need from a workbook
//! pane (see `kantor_core::recovery`, `sheets_engine::XlsxSnapshot`): cheap
//! snapshots on the UI thread, everything slow in the background.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Result, anyhow};
use gpui_kit::*;
use kantor_ui::disk_guard::{DiskGuard, Overwrite, confirm_overwrite};
use kantor_ui::i18n::{t, tf};
use sheets_engine::{Workbook, XlsxSnapshot};

use crate::calc::{Busy, Job};
use crate::{SaveAs, SheetsView};

impl SheetsView {
    /// Identifies the workbook's state (`None` while it's opening or
    /// recalculating in the background).
    pub fn revision(&self, cx: &App) -> Option<u64> {
        if self.busy.is_some() {
            return None;
        }
        let (state, epoch) = self.table.read(cx).delegate().book.revision();
        Some(state.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ epoch)
    }

    /// A copy of the workbook to write as a recovery copy elsewhere, with
    /// the file it came from and that file's format tag.
    pub fn recovery_snapshot(&self, cx: &App) -> Option<(XlsxSnapshot, Option<PathBuf>, String)> {
        if self.busy.is_some() {
            return None;
        }
        let book = &self.table.read(cx).delegate().book;
        let (original, format) = book.recovery_origin();
        Some((book.xlsx_snapshot(), original, format))
    }

    /// Whether autosave may save this workbook (an `.xlsx` with a file, not
    /// changed on disk by someone else, not busy or saving).
    pub fn can_autosave(&self, cx: &App) -> bool {
        if self.busy.is_some() || self.saving.is_some() || self.pending_save.is_some() {
            return false;
        }
        let book = &self.table.read(cx).delegate().book;
        book.can_autosave()
            && !book
                .path()
                .is_some_and(|p| self.disk_guard.as_ref().is_some_and(|g| g.changed(p)))
    }

    /// Save to the workbook's own file without stopping typing (see
    /// [`crate::saving`]).
    pub fn autosave(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<Result<()>> {
        if !self.can_autosave(cx) {
            return Task::ready(Err(anyhow!("not autosaved")));
        }
        let Some(path) = self
            .table
            .read(cx)
            .delegate()
            .book
            .path()
            .map(|p| p.to_path_buf())
        else {
            return Task::ready(Err(anyhow!("no file")));
        };
        let Some(done) = self.start_save(path, true, window, cx) else {
            return Task::ready(Err(anyhow!("not autosaved")));
        };
        cx.background_spawn(async move {
            match done.await {
                Ok(Ok(_)) => Ok(()),
                Ok(Err(e)) => Err(anyhow!(e)),
                Err(_) => Err(anyhow!("autosave cancelled")),
            }
        })
    }

    /// A pane showing the recovery copy `copy` of `original` (opened in the
    /// background), unsaved. `format`: the original's format tag.
    pub fn new_recovered(
        copy: PathBuf,
        original: Option<PathBuf>,
        format: Option<String>,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::new(None, window, cx);
        let bytes = std::fs::metadata(&copy).map_or(0, |m| m.len());
        let parse = cx.background_spawn({
            let original = original.clone();
            async move {
                let book = Workbook::open_recovered(&copy, original.as_deref(), format.as_deref());
                kantor_mem::release_free_memory();
                book
            }
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = parse.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_open(result, window, cx);
                if this.path(cx).is_some() {
                    this.status =
                        tf("Restored unsaved changes to {}", &[&this.doc_name(cx)]).into();
                } else {
                    this.status = t("Restored unsaved changes").into();
                }
                cx.notify();
            });
        });
        this.opening = original.clone();
        this.status = tf("Restoring {}…", &[&name]).into();
        this.busy = Some(Busy {
            what: Job::Opening {
                name: name.clone(),
                bytes,
            },
            started: Instant::now(),
            name,
            path: original,
            dirty: true,
            _task: task,
        });
        window.focus(&this.busy_focus, cx);
        this
    }

    /// Ask before Save overwrites `guard`'s file if it changed on disk.
    pub fn set_disk_guard(&mut self, guard: Option<DiskGuard>) {
        self.disk_guard = guard;
    }

    /// Save is about to write `target`: if it changed on disk since Kantor
    /// last saw it, ask first (and return `true`: don't save now).
    pub(crate) fn ask_before_overwrite(
        &mut self,
        target: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.disk_guard.as_ref().is_some_and(|g| g.changed(target)) {
            return false;
        }
        let view = cx.entity().downgrade();
        confirm_overwrite(&self.doc_name(cx), window, cx, move |choice, window, cx| {
            let _ = view.update(cx, |this, cx| match choice {
                Overwrite::Replace => {
                    this.disk_guard = None;
                    this.save_doc(window, cx);
                }
                Overwrite::SaveAs => this.save_as(&SaveAs, window, cx),
            });
        });
        true
    }
}
