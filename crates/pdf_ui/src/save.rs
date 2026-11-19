//! Saving an edited PDF: Save writes the file in place safely (a temporary
//! file next to it, then renamed over it; no backup, as in Docs and
//! Slides), Save As switches the document to the new file. A
//! password-protected file stays protected; when that isn't possible Kantor
//! asks before saving it without the password. Also the close prompt's
//! save, autosave and recovery copies.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogDescription, DialogFooter};
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, tf};

use crate::PdfView;
use crate::model::build;

impl PdfView {
    /// Whether the document has changes not saved.
    pub fn is_dirty(&self, _: &App) -> bool {
        self.doc().is_some_and(|d| d.model.is_dirty())
    }

    /// Identifies the document's state (changes with every edit, undo and
    /// redo).
    pub fn revision(&self, _: &App) -> Option<u64> {
        Some(self.doc().map_or(0, |d| d.model.revision()))
    }

    /// Autosave may save it: it has changes and a file, and isn't saving.
    pub fn can_autosave(&self, cx: &App) -> bool {
        self.is_dirty(cx) && !self.path.as_os_str().is_empty() && !self.saving
    }

    /// PDFs always have a file.
    pub fn set_untitled_name(&mut self, _: String, _: &mut Context<Self>) {}

    /// Ask before Save overwrites the file if it changed on disk since.
    pub fn set_disk_guard(&mut self, guard: Option<kantor_ui::disk_guard::DiskGuard>) {
        self.disk_guard = guard;
    }

    pub fn save_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_task(window, cx).detach();
    }

    pub fn save_doc_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_as_task(window, cx).detach();
    }

    /// Save before the document closes; resolves to whether it is saved.
    pub fn save_for_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        if !self.is_dirty(cx) {
            return Task::ready(true);
        }
        self.save_task(window, cx)
    }

    /// Autosave: Save, resolving once written.
    pub fn autosave(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        let task = self.save_task(window, cx);
        cx.background_spawn(async move {
            if task.await {
                Ok(())
            } else {
                Err(anyhow::anyhow!("not saved"))
            }
        })
    }

    /// Save to the document's file; resolves to whether it is saved.
    pub(crate) fn save_task(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        if self.doc().is_none() {
            return Task::ready(false);
        }
        if !self.is_dirty(cx) {
            self.status = i18n::t("No changes to save").into();
            cx.notify();
            return Task::ready(true);
        }
        let target = self.path.clone();
        if self.disk_guard.as_ref().is_some_and(|g| g.changed(&target)) {
            let view = cx.entity().downgrade();
            kantor_ui::disk_guard::confirm_overwrite(
                &self.name,
                window,
                cx,
                move |choice, window, cx| {
                    let _ = view.update(cx, |this, cx| match choice {
                        kantor_ui::disk_guard::Overwrite::Replace => {
                            this.disk_guard = None;
                            this.save_doc(window, cx);
                        }
                        kantor_ui::disk_guard::Overwrite::SaveAs => this.save_doc_as(window, cx),
                    });
                },
            );
            return Task::ready(false);
        }
        self.write_to(target, true, window, cx)
    }

    /// Ask for a file and save there; the document then is that file.
    fn save_as_task(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        if self.doc().is_none() {
            return Task::ready(false);
        }
        let target = cx.prompt_for_new_path(&self.dir(), Some(&self.name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut path))) = target.await else {
                return false;
            };
            if path.extension().is_none() {
                path.set_extension("pdf");
            }
            let Ok(task) =
                this.update_in(cx, |this, window, cx| this.write_to(path, true, window, cx))
            else {
                return false;
            };
            task.await
        })
    }

    /// Build the document and write it to `path` in the background.
    fn write_to(
        &mut self,
        path: PathBuf,
        keep_encryption: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        if self.saving {
            self.status = i18n::t("Still saving…").into();
            cx.notify();
            return Task::ready(false);
        }
        let Some(doc) = self.doc() else {
            return Task::ready(false);
        };
        let rev = doc.model.revision();
        let (sources, picks) = doc.model.picks();
        // Unchanged pages of the file as it is: its own bytes, untouched.
        let as_is = doc
            .model
            .is_base_as_is()
            .then(|| doc.model.base().bytes.clone());
        self.saving = true;
        self.busy = Some(i18n::t("Saving…").into());
        cx.notify();
        let started = Instant::now();
        let out = path.clone();
        let work = cx.background_spawn(async move {
            let bytes = match as_is {
                Some(b) => Arc::unwrap_or_clone(b),
                None => build(&sources, &picks, keep_encryption)?,
            };
            kantor_pdf::write_atomic(&out, &bytes)?;
            Ok::<usize, kantor_pdf::Error>(bytes.len())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            this.update_in(cx, |this, window, cx| {
                this.saving = false;
                this.busy = None;
                match result {
                    Ok(len) => {
                        if let Some(d) = this.doc_mut() {
                            d.model.mark_saved(rev);
                        }
                        this.disk_guard = None;
                        if path != this.path {
                            this.set_path(path);
                        }
                        log::info!(
                            "saved {} ({} KB) in {:.0} ms",
                            this.path.display(),
                            len / 1024,
                            started.elapsed().as_secs_f64() * 1000.0
                        );
                        this.status = tf("Saved {}", &[&this.name]).into();
                        cx.notify();
                        true
                    }
                    Err(kantor_pdf::Error::Encryption(e)) => {
                        log::warn!("keeping the encryption of {}: {e}", this.name);
                        this.ask_save_unprotected(path, window, cx);
                        false
                    }
                    Err(e) => {
                        this.status = tf("Save failed: {}", &[&e]).into();
                        cx.notify();
                        false
                    }
                }
            })
            .unwrap_or(false)
        })
    }

    /// The protection can't be kept: ask before saving without it.
    fn ask_save_unprotected(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.status = i18n::t("Not saved: the password protection can't be kept").into();
        cx.notify();
        if window.has_active_dialog(cx) {
            return;
        }
        let view = cx.entity().downgrade();
        let title: SharedString = i18n::t("Save without the password?").into();
        let detail: SharedString = tf(
            "Kantor can't protect “{}” with its password again. Saved now, it opens without a password. Use Save As to keep the protected original.",
            &[&self.name],
        )
        .into();
        window.open_dialog(cx, move |dialog, _, _| {
            let (view, path) = (view.clone(), path.clone());
            dialog
                .w(px(460.))
                .title(title.clone())
                .child(v_flex().child(DialogDescription::new().child(detail.clone())))
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("pdf-unprotected-cancel")
                                .label(i18n::t("Cancel"))
                                .ghost()
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("pdf-unprotected-save")
                                .label(i18n::t("Save Without Password"))
                                .primary()
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let path = path.clone();
                                    let _ = view.update(cx, |this, cx| {
                                        this.write_to(path, false, window, cx).detach();
                                    });
                                }),
                        ),
                )
        });
    }

    /// The document is the file at `path` now.
    pub(crate) fn set_path(&mut self, path: PathBuf) {
        self.name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "PDF".into());
        self.path = path;
    }

    /// A recovery copy of the document: built (and kept password-protected
    /// like the original) by the returned function, on any thread. `None`
    /// before the file is open.
    pub fn recovery_snapshot(
        &self,
        _: &App,
    ) -> Option<Box<dyn FnOnce() -> anyhow::Result<Vec<u8>> + Send>> {
        let doc = self.doc()?;
        let (sources, picks) = doc.model.picks();
        Some(Box::new(move || {
            build(&sources, &picks, true).map_err(|e| anyhow::anyhow!("{e}"))
        }))
    }

    /// Print: the document opened in the system's PDF viewer (with unsaved
    /// changes, a temporary copy of it).
    pub fn print_doc(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.path.as_os_str().is_empty() {
            return;
        }
        let Some(doc) = self.doc() else {
            return;
        };
        if !doc.model.is_dirty() {
            cx.open_with_system(&self.path);
            self.status = tf("Opened {} in the PDF viewer to print", &[&self.name]).into();
            cx.notify();
            return;
        }
        let (sources, picks) = doc.model.picks();
        let name = self.name.clone();
        let work = cx.background_spawn(async move {
            let bytes = build(&sources, &picks, true)?;
            let dir = std::env::temp_dir().join("kantor-print");
            std::fs::create_dir_all(&dir).map_err(|source| kantor_pdf::Error::Io {
                path: dir.clone(),
                source,
            })?;
            let path = dir.join(&name);
            kantor_pdf::write_atomic(&path, &bytes)?;
            Ok::<PathBuf, kantor_pdf::Error>(path)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(p) => {
                        cx.open_with_system(&p);
                        this.status =
                            tf("Opened {} in the PDF viewer to print", &[&this.name]).into();
                    }
                    Err(e) => this.status = tf("Couldn't print: {}", &[&e]).into(),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
