//! Export as PDF and Print: the workbook printed the way Excel prints it
//! (`sheets_pdf`). Pages are planned here and read from the workbook a few
//! at a time between frames; a thread shapes and writes them as they come,
//! so a big workbook neither freezes the window nor holds every page in
//! memory. Print writes a temporary PDF and opens it in the system's PDF
//! viewer, which prints it (Docs and Slides have no print path of their
//! own to share). Print opens the print view first (`page_setup` with
//! `print_preview`), as Excel's File > Print does.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::SheetsView;

/// Pages read from the workbook per step.
const BATCH: usize = 16;
/// Pages waiting for the writer at most.
const QUEUE: usize = 64;

impl SheetsView {
    /// Ask where, then print the visible sheets to a PDF.
    pub fn export_pdf_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.blocked(cx) {
            return;
        }
        let (dir, name) = {
            let book = &self.table.read(cx).delegate().book;
            let dir = book
                .source_path()
                .and_then(|p| p.parent())
                .map(|p| p.to_path_buf())
                .or_else(kantor_core::dirs::home_dir)
                .unwrap_or_else(|| PathBuf::from("."));
            let name = std::path::Path::new(&self.name(book))
                .with_extension("pdf")
                .to_string_lossy()
                .into_owned();
            (dir, name)
        };
        let path = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut path))) = path.await else {
                return;
            };
            if path.extension().is_none() {
                path.set_extension("pdf");
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.export_to(path, false, window, cx)
            });
        })
        .detach();
    }

    /// Print (Ctrl+P): Excel's print view, the pages previewed beside the
    /// page settings; its Print button prints them.
    pub fn print_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_page_setup(true, window, cx);
    }

    /// Print now: write a temporary PDF and open it in the system's viewer.
    pub(crate) fn print_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.blocked(cx) {
            return;
        }
        let stem = {
            let book = &self.table.read(cx).delegate().book;
            std::path::Path::new(&self.name(book))
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Sheets".into())
        };
        let dir = std::env::temp_dir().join(format!("kantor-print-{}", std::process::id()));
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.status = format!("Print failed: {e}").into();
            cx.notify();
            return;
        }
        self.export_to(dir.join(format!("{stem}.pdf")), true, window, cx);
    }

    fn export_to(
        &mut self,
        path: PathBuf,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (plan, shaper) = {
            let book = &self.table.read(cx).delegate().book;
            let opts = sheets_pdf::Options::default()
                .now()
                .with_path(book.source_path());
            let mut shaper = sheets_pdf::Shaper::new();
            let plan = sheets_pdf::Plan::new(book, &opts, &mut shaper);
            (Arc::new(plan), shaper)
        };
        let total = plan.pages.len();
        let (tx, rx) = sync_channel::<sheets_pdf::PageContent>(QUEUE);
        let title = plan.title.clone();
        let file = path.clone();
        let writer = std::thread::spawn(move || -> anyhow::Result<usize> {
            let mut enc = sheets_pdf::Encoder::new(&title, shaper);
            while let Ok(page) = rx.recv() {
                enc.page(&page);
            }
            let pages = enc.pages().max(1);
            let bytes = enc.finish()?;
            sheets_pdf::write_file(&file, &bytes)?;
            Ok(pages)
        });
        let verb = if open {
            "Preparing to print"
        } else {
            "Exporting"
        };
        self.status = format!("{verb} {}…", display_name(&path)).into();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let mut tx: Option<SyncSender<_>> = Some(tx);
            let mut next = 0;
            while next < total {
                let end = (next + BATCH).min(total);
                let Ok(pages) = this.update(cx, |this, cx| {
                    let plan = plan.clone();
                    let pages = this.table.update(cx, |t, _| {
                        let book = &mut t.delegate_mut().book;
                        (next..end)
                            .map(|i| sheets_pdf::extract(book, &plan, i))
                            .collect::<Vec<_>>()
                    });
                    if total > BATCH * 4 {
                        this.status = format!("{verb}: page {end} of {total}…").into();
                        cx.notify();
                    }
                    pages
                }) else {
                    return; // the view closed; the writer stops with the channel
                };
                for mut page in pages {
                    let Some(sender) = tx.as_ref() else { break };
                    loop {
                        match sender.try_send(page) {
                            Ok(()) => break,
                            Err(TrySendError::Full(p)) => {
                                page = p;
                                cx.background_executor()
                                    .timer(Duration::from_millis(4))
                                    .await;
                            }
                            Err(TrySendError::Disconnected(_)) => {
                                tx = None;
                                break;
                            }
                        }
                    }
                }
                next = end;
                // Let the window paint between steps.
                cx.background_executor()
                    .timer(Duration::from_millis(1))
                    .await;
            }
            drop(tx);
            let result = cx
                .background_spawn(async move {
                    writer
                        .join()
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("the PDF writer stopped")))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let name = display_name(&path);
                this.status = match (&result, open) {
                    (Ok(pages), false) => format!(
                        "Exported {name} ({pages} page{})",
                        if *pages == 1 { "" } else { "s" }
                    )
                    .into(),
                    (Ok(pages), true) => format!(
                        "Opened {pages} page{} in the PDF viewer to print",
                        if *pages == 1 { "" } else { "s" }
                    )
                    .into(),
                    (Err(err), false) => format!("Export to PDF failed: {err:#}").into(),
                    (Err(err), true) => format!("Print failed: {err:#}").into(),
                };
                if open && result.is_ok() {
                    cx.open_with_system(&path);
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn display_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
