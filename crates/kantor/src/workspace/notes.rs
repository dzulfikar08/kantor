//! Notes in the workspace: Save as Word, Save as PDF and Make Slides (the
//! builders live in this crate, `agent::notes_convert`), printing, and
//! vaults (a folder of notes opened from the palette; links that open a
//! note at a place).

use std::path::{Path, PathBuf};

use gpui_kit::*;
use kantor_ui::i18n::tf;
use notes_ui::{Export, Jump, NotesView};

use super::Workspace;
use super::doc::DocView;
use crate::agent::notes_convert;

/// What a conversion needs from the note.
struct Source {
    text: String,
    /// The file name without its extension (the fallback title).
    stem: String,
    /// The note's folder (relative pictures), else the home folder.
    dir: PathBuf,
}

fn source(view: &Entity<NotesView>, cx: &App) -> Source {
    let v = view.read(cx);
    let name = v.doc_name(cx);
    let stem = Path::new(&name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or(name);
    let dir = v
        .dir()
        .map(Path::to_path_buf)
        .or_else(kantor_core::dirs::user_home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    Source {
        text: v.editor().text().to_owned(),
        stem,
        dir,
    }
}

fn build(kind: Export, s: &Source) -> anyhow::Result<Vec<u8>> {
    match kind {
        Export::Word => notes_convert::to_docx(&s.text, &s.stem, &s.dir),
        Export::Pdf => notes_convert::to_pdf(&s.text, &s.stem, &s.dir),
        Export::Slides => notes_convert::to_pptx(&s.text, &s.stem, &s.dir),
    }
}

fn extension(kind: Export) -> &'static str {
    match kind {
        Export::Word => "docx",
        Export::Pdf => "pdf",
        Export::Slides => "pptx",
    }
}

impl Workspace {
    /// Ask where, then convert the note there in the background. Slides
    /// open in a tab once made.
    pub(super) fn export_note(
        &mut self,
        view: Entity<NotesView>,
        kind: Export,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let src = source(&view, cx);
        let ext = extension(kind);
        let suggested = format!("{}.{ext}", src.stem);
        let path = cx.prompt_for_new_path(&src.dir, Some(&suggested));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut path))) = path.await else {
                return;
            };
            if path.extension().is_none() {
                path.set_extension(ext);
            }
            let out = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let bytes = build(kind, &src)?;
                    kantor_core::fsx::write_atomic(&out, &bytes)?;
                    anyhow::Ok(())
                })
                .await;
            let _ = this.update_in(cx, |ws, window, cx| match result {
                Ok(()) => {
                    let file = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    ws.notice(tf("Saved {}", &[&file]), cx);
                    if kind == Export::Slides {
                        ws.open_paths(None, vec![path], window, cx);
                    }
                }
                Err(e) => ws.notice(
                    tf("Couldn't convert the note: {}", &[&format!("{e:#}")]),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// Print: the note as a PDF, opened in the system's viewer.
    pub(super) fn print_note(
        &mut self,
        view: Entity<NotesView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let src = source(&view, cx);
        let path = match kantor_core::dirs::print_pdf_path(&src.stem) {
            Ok(p) => p,
            Err(e) => {
                self.notice(tf("Print failed: {}", &[&e]), cx);
                return;
            }
        };
        cx.spawn_in(window, async move |this, cx| {
            let out = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let bytes = build(Export::Pdf, &src)?;
                    kantor_core::fsx::write_atomic(&out, &bytes)?;
                    anyhow::Ok(())
                })
                .await;
            let _ = this.update(cx, |ws, cx| match result {
                Ok(()) => cx.open_with_system(&path),
                Err(e) => ws.notice(tf("Print failed: {}", &[&format!("{e:#}")]), cx),
            });
        })
        .detach();
    }

    /// Open the note at `path` (or show its tab) and go to `jump` in it.
    pub(super) fn open_note_at(
        &mut self,
        path: PathBuf,
        jump: Jump,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_paths(None, vec![path.clone()], window, cx);
        if let Some((_, pane)) = self.find_path(&path, cx)
            && let Some(doc) = self.docs.get(&pane)
            && let DocView::Notes(view) = &doc.view
        {
            view.update(cx, |v, cx| v.jump(&jump, cx));
        }
    }

    /// Ask for a folder and open it as a vault: its newest note opens (a
    /// new one is made in an empty folder) with the files beside it.
    pub(super) fn open_vault(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dirs = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(kantor_ui::i18n::t("Open a folder of notes").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(dirs))) = dirs.await else {
                return;
            };
            let Some(root) = dirs.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |ws, window, cx| {
                notes_ui::vault::open_vault(root.clone(), cx);
                match newest_note(&root) {
                    Some(p) => ws.open_paths(None, vec![p], window, cx),
                    None => {
                        let p = notes_engine::vault::unique_path(
                            &root,
                            &format!("{}.md", kantor_ui::i18n::t("Untitled")),
                        );
                        match std::fs::write(&p, b"") {
                            Ok(()) => ws.open_paths(None, vec![p], window, cx),
                            Err(e) => ws.notice(tf("Couldn't make the note: {}", &[&e]), cx),
                        }
                    }
                }
            });
        })
        .detach();
    }
}

/// The most recently changed note under `root` (hidden folders skipped).
fn newest_note(root: &Path) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let mut stack = vec![root.to_path_buf()];
    let mut seen = 0;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            seen += 1;
            if seen > 50_000 {
                return best.map(|b| b.1);
            }
            let p = e.path();
            let hidden = e.file_name().to_string_lossy().starts_with('.');
            match e.file_type() {
                Ok(ft) if ft.is_dir() => {
                    if !hidden {
                        stack.push(p);
                    }
                }
                Ok(_) if notes_engine::is_note_path(&p) => {
                    let t = e
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::UNIX_EPOCH);
                    if best.as_ref().is_none_or(|b| t > b.0) {
                        best = Some((t, p));
                    }
                }
                _ => {}
            }
        }
    }
    best.map(|b| b.1)
}
