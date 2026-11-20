//! Organizing pages: selection in the thumbnails and the Organize Pages
//! grid, rotate / delete / duplicate / move, insert from files or blank,
//! copy and paste between open PDFs, drag and drop, Extract, Undo / Redo.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, tf, tk};
use kantor_ui::{metrics::*, tokens};

use crate::PdfView;
use crate::model::{Change, PageRef, Source};

thread_local! {
    /// Pages copied with Copy (or Cut), with their sources, for Paste in
    /// any open PDF.
    static CLIPBOARD: RefCell<Vec<(Arc<Source>, PageRef)>> = const { RefCell::new(Vec::new()) };
}

/// Pages being dragged from a thumbnail list or grid.
#[derive(Clone)]
pub(crate) struct PageDrag {
    /// The view they come from.
    pub from: EntityId,
    /// Their places there.
    pub places: BTreeSet<usize>,
    pub pages: Vec<(Arc<Source>, PageRef)>,
}

/// What a page drag shows under the pointer.
pub(crate) struct DragLabel(pub SharedString);

impl Render for DragLabel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens(cx);
        h_flex()
            .px(SPACE_4)
            .py(SPACE_2)
            .rounded(RADIUS_MD)
            .bg(t.accent)
            .text_color(t.accent_fg)
            .text_size(FONT_SMALL)
            .font_weight(FontWeight::SEMIBOLD)
            .shadow_md()
            .child(self.0.clone())
    }
}

/// Files Insert takes: PDFs and pictures.
pub(crate) fn insertable(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "pdf" | "png" | "jpg" | "jpeg"
        )
    })
}

/// Read a PDF or picture as a source (on a background thread).
pub(crate) fn load_file(path: &Path) -> Result<Source, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let bytes = std::fs::read(path).map_err(|e| tf("Couldn't read {}: {}", &[&name, &e]))?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let bytes = if matches!(ext.as_str(), "png" | "jpg" | "jpeg") {
        let mut pdf = kantor_pdf::images_to_pdf(
            &[(name.clone(), bytes)],
            &kantor_pdf::ImageOptions::default(),
        )
        .map_err(|e| tf("Couldn't read {}: {}", &[&name, &e]))?;
        pdf.to_bytes()
            .map_err(|e| tf("Couldn't read {}: {}", &[&name, &e]))?
    } else {
        bytes
    };
    Source::read(name.clone(), Arc::new(bytes), String::new()).map_err(|e| match e {
        pdf_engine::OpenError::Password { .. } => tf(
            "{} is password-protected: open it, then copy its pages and paste them here",
            &[&name],
        ),
        _ => tf("Kantor can't open {}", &[&name]),
    })
}

impl PdfView {
    // ── selection ─────────────────────────────────────────────────────────

    /// The pages page commands act on: the selected ones, else the
    /// current page.
    pub(crate) fn target_pages(&self) -> BTreeSet<usize> {
        if self.page_sel.is_empty() {
            let n = self.page_count();
            if n == 0 {
                return BTreeSet::new();
            }
            BTreeSet::from([self.current_page().min(n - 1)])
        } else {
            self.page_sel.clone()
        }
    }

    /// A click on thumbnail `i`: select it (Ctrl adds or removes it, Shift
    /// selects the run from the last one clicked) and show it.
    pub(crate) fn click_page(
        &mut self,
        i: usize,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if modifiers.shift {
            let a = self.sel_anchor.unwrap_or(self.current_page());
            let (lo, hi) = if a <= i { (a, i) } else { (i, a) };
            if !modifiers.control && !modifiers.platform {
                self.page_sel.clear();
            }
            self.page_sel.extend(lo..=hi);
        } else if modifiers.control || modifiers.platform {
            if !self.page_sel.remove(&i) {
                self.page_sel.insert(i);
            }
            self.sel_anchor = Some(i);
        } else {
            self.page_sel = BTreeSet::from([i]);
            self.sel_anchor = Some(i);
        }
        if !self.organize {
            self.go_to_page(i, cx);
        }
        window.focus(&self.pages_focus, cx);
        cx.notify();
    }

    /// Right-click on thumbnail `i`: it joins the selection if it isn't in
    /// it (the menu acts on the selection).
    pub(crate) fn context_page(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.page_sel.contains(&i) {
            self.page_sel = BTreeSet::from([i]);
            self.sel_anchor = Some(i);
        }
        window.focus(&self.pages_focus, cx);
        cx.notify();
    }

    pub fn select_all_pages(&mut self, cx: &mut Context<Self>) {
        self.page_sel = (0..self.page_count()).collect();
        cx.notify();
    }

    /// Up / Down (Left / Right in the grid) in the thumbnails: the
    /// selection moves by `step` pages (Shift extends it).
    pub(crate) fn step_page_sel(&mut self, step: isize, extend: bool, cx: &mut Context<Self>) {
        let n = self.page_count();
        if n == 0 {
            return;
        }
        let from = self
            .page_sel
            .iter()
            .next_back()
            .copied()
            .unwrap_or(self.current_page());
        let to = (from as isize + step).clamp(0, n as isize - 1) as usize;
        if extend {
            let a = self.sel_anchor.unwrap_or(from);
            let (lo, hi) = if a <= to { (a, to) } else { (to, a) };
            self.page_sel = (lo..=hi).collect();
        } else {
            self.page_sel = BTreeSet::from([to]);
            self.sel_anchor = Some(to);
        }
        self.thumb_scroll.scroll_to_item(to, ScrollStrategy::Center);
        let cols = self.grid_columns();
        self.grid_scroll
            .scroll_to_item(to / cols.max(1), ScrollStrategy::Center);
        if !self.organize {
            self.go_to_page(to, cx);
        }
        cx.notify();
    }

    // ── after a change ────────────────────────────────────────────────────

    /// The model changed: sizes, text and links follow the pages; the
    /// selection becomes `sel`; a search runs again.
    pub(crate) fn after_change(&mut self, sel: BTreeSet<usize>, cx: &mut Context<Self>) {
        let Some(doc) = self.doc_mut() else {
            return;
        };
        doc.sizes = doc.model.sizes();
        let n = doc.sizes.len();
        // Force the column to be laid out again (sizes may change with
        // the same page count).
        self.column_key.2 = usize::MAX;
        self.texts.clear();
        self.links.clear();
        for i in 0..n {
            self.place_text(i);
        }
        self.sel = None;
        self.press = None;
        self.page_sel = sel.into_iter().filter(|&i| i < n).collect();
        if self.sel_anchor.is_some_and(|a| a >= n) {
            self.sel_anchor = None;
        }
        if let Some(w) = self.worker.clone()
            && let Some(doc) = self.doc()
        {
            for p in doc.model.pages() {
                if let Some(s) = doc.model.source(p.src) {
                    w.add_source(s.clone());
                }
            }
        }
        if self.find.is_searching() {
            self.find_search(cx);
        }
        self.warn_signed_pages();
        cx.notify();
    }

    fn changed(&mut self, sel: Option<BTreeSet<usize>>, status: String, cx: &mut Context<Self>) {
        if let Some(sel) = sel {
            self.after_change(sel, cx);
        }
        self.status = status.into();
        cx.notify();
    }

    // ── page commands ─────────────────────────────────────────────────────

    /// Rotate Left / Right: the selected pages (or the current one) turn a
    /// quarter, in the file (saved), not just on screen.
    pub fn rotate_pages(&mut self, cw: bool, cx: &mut Context<Self>) {
        let sel = self.target_pages();
        let Some(doc) = self.doc_mut() else {
            return;
        };
        if doc.model.rotate(&sel, cw) {
            let n = sel.len();
            let msg = if n == 1 {
                i18n::t("Rotated 1 page").to_owned()
            } else {
                tf("Rotated {} pages", &[&n])
            };
            let keep = if self.page_sel.is_empty() {
                BTreeSet::new()
            } else {
                sel
            };
            self.changed(Some(keep), msg, cx);
        }
    }

    /// Delete the selected pages (never the last one left).
    pub fn delete_pages(&mut self, cx: &mut Context<Self>) {
        let sel = self.target_pages();
        let first = sel.first().copied().unwrap_or(0);
        let Some(doc) = self.doc_mut() else {
            return;
        };
        match doc.model.delete(&sel) {
            Some(n) => {
                let msg = if n == 1 {
                    i18n::t("Deleted 1 page").to_owned()
                } else {
                    tf("Deleted {} pages", &[&n])
                };
                let left = doc.model.len();
                let next = first.min(left.saturating_sub(1));
                self.changed(Some(BTreeSet::from([next])), msg, cx);
                self.sel_anchor = Some(next);
            }
            None => {
                self.status = i18n::t("A PDF needs at least one page").into();
                cx.notify();
            }
        }
    }

    /// Copies of the selected pages, after the last of them.
    pub fn duplicate_pages(&mut self, cx: &mut Context<Self>) {
        let sel = self.target_pages();
        let Some(doc) = self.doc_mut() else {
            return;
        };
        if let Some(new) = doc.model.duplicate(&sel) {
            let n = new.len();
            let msg = if n == 1 {
                i18n::t("Duplicated 1 page").to_owned()
            } else {
                tf("Duplicated {} pages", &[&n])
            };
            self.changed(Some(new), msg, cx);
        }
    }

    /// Move the selected pages to before page `to`.
    pub(crate) fn move_pages(&mut self, sel: &BTreeSet<usize>, to: usize, cx: &mut Context<Self>) {
        let Some(doc) = self.doc_mut() else {
            return;
        };
        if let Some(new) = doc.model.move_pages(sel, to) {
            let n = new.len();
            let first = new.first().copied().unwrap_or(0);
            let msg = if n == 1 {
                tf("Moved 1 page to page {}", &[&(first + 1)])
            } else {
                tf("Moved {} pages to page {}", &[&n, &(first + 1)])
            };
            self.sel_anchor = Some(first);
            self.changed(Some(new), msg, cx);
        }
    }

    /// Where pages inserted before or after the selection go.
    fn insert_place(&self, after: bool) -> usize {
        let sel = self.target_pages();
        if after {
            sel.last().map_or(self.page_count(), |l| l + 1)
        } else {
            sel.first().copied().unwrap_or(0)
        }
    }

    /// A blank page the size of the page next to it, before or after the
    /// selection.
    pub fn insert_blank_page(&mut self, after: bool, cx: &mut Context<Self>) {
        let at = self.insert_place(after);
        let near = if after { at.saturating_sub(1) } else { at };
        let (w, h) = self.doc().map_or((612.0, 792.0), |d| {
            d.model.size(near.min(d.model.len().saturating_sub(1)))
        });
        let mut pdf = kantor_pdf::blank_page(w, h);
        let Ok(bytes) = pdf.to_bytes() else {
            return;
        };
        let Ok(source) = Source::read(
            i18n::t("Blank page").to_owned(),
            Arc::new(bytes),
            String::new(),
        ) else {
            return;
        };
        let source = Arc::new(source);
        let page = PageRef {
            src: source.id,
            page: 0,
            rot: 0,
        };
        let Some(doc) = self.doc_mut() else {
            return;
        };
        doc.model.add_source(source);
        let sel = doc.model.insert(at, vec![page], Change::Insert);
        self.changed(sel, i18n::t("Inserted a blank page").to_owned(), cx);
    }

    /// Insert ▸ Pages from File…: pick PDFs or pictures, inserted before
    /// or after the selection.
    pub fn insert_from_file(&mut self, after: bool, window: &mut Window, cx: &mut Context<Self>) {
        let at = self.insert_place(after);
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(i18n::t("Insert pages from PDFs or pictures").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| this.insert_paths(paths, at, cx));
        })
        .detach();
    }

    /// Read `paths` in the background and insert their pages before page
    /// `at` (one undo step).
    pub(crate) fn insert_paths(&mut self, paths: Vec<PathBuf>, at: usize, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = paths.into_iter().filter(|p| insertable(p)).collect();
        if paths.is_empty() {
            self.status = i18n::t("Only PDFs and pictures (PNG, JPEG) can be inserted").into();
            cx.notify();
            return;
        }
        self.busy = Some(i18n::t("Inserting pages…").into());
        cx.notify();
        let load = cx.background_spawn(async move {
            paths
                .iter()
                .map(|p| load_file(p))
                .collect::<Vec<Result<Source, String>>>()
        });
        cx.spawn(async move |this, cx| {
            let loaded = load.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                let mut errors = Vec::new();
                let mut pages = Vec::new();
                let mut sources = Vec::new();
                for r in loaded {
                    match r {
                        Ok(s) => {
                            let s = Arc::new(s);
                            pages.extend((0..s.sizes.len()).map(|page| PageRef {
                                src: s.id,
                                page,
                                rot: 0,
                            }));
                            sources.push(s);
                        }
                        Err(e) => errors.push(e),
                    }
                }
                let n = pages.len();
                if let Some(doc) = this.doc_mut() {
                    for s in sources {
                        doc.model.add_source(s);
                    }
                    let sel = doc.model.insert(at, pages, Change::Insert);
                    let msg = if n == 1 {
                        i18n::t("Inserted 1 page").to_owned()
                    } else {
                        tf("Inserted {} pages", &[&n])
                    };
                    this.changed(sel, msg, cx);
                }
                if let Some(e) = errors.first() {
                    this.status = e.clone().into();
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Files dropped on the window (the workspace asks): taken when the
    /// pointer is over the thumbnails or the grid.
    pub fn drop_files(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) -> bool {
        if self.batch_drop(paths, cx) {
            self.drop_at = None;
            return true;
        }
        let Some(at) = self.drop_at.take() else {
            // Pictures dropped on a page become stamps there.
            return self.drop_stamps(paths, cx);
        };
        self.insert_paths(paths.to_vec(), at, cx);
        true
    }

    /// Copy the selected pages (Paste puts them into this or another open
    /// PDF).
    pub fn copy_pages(&mut self, cx: &mut Context<Self>) {
        let sel = self.target_pages();
        let Some(doc) = self.doc() else {
            return;
        };
        let pages: Vec<(Arc<Source>, PageRef)> = sel
            .iter()
            .filter_map(|&i| {
                let p = doc.model.page(i)?;
                Some((doc.model.source(p.src)?.clone(), p))
            })
            .collect();
        let n = pages.len();
        CLIPBOARD.with(|c| *c.borrow_mut() = pages);
        self.status = if n == 1 {
            i18n::t("Copied 1 page").into()
        } else {
            tf("Copied {} pages", &[&n]).into()
        };
        cx.notify();
    }

    pub fn cut_pages(&mut self, cx: &mut Context<Self>) {
        self.copy_pages(cx);
        self.delete_pages(cx);
    }

    /// Paste copied pages after the selection (or the current page).
    pub fn paste_pages(&mut self, cx: &mut Context<Self>) {
        let pages = CLIPBOARD.with(|c| c.borrow().clone());
        if pages.is_empty() {
            self.status = i18n::t("No pages copied: select pages and copy them first").into();
            cx.notify();
            return;
        }
        let at = self.insert_place(true);
        self.add_pages(pages, at, Change::Paste, cx);
    }

    /// Insert `pages` (of any sources) before page `at`.
    fn add_pages(
        &mut self,
        pages: Vec<(Arc<Source>, PageRef)>,
        at: usize,
        change: Change,
        cx: &mut Context<Self>,
    ) {
        let n = pages.len();
        let Some(doc) = self.doc_mut() else {
            return;
        };
        let mut refs = Vec::with_capacity(n);
        for (s, p) in pages {
            doc.model.add_source(s);
            refs.push(p);
        }
        let sel = doc.model.insert(at, refs, change);
        let msg = if n == 1 {
            i18n::t("Inserted 1 page").to_owned()
        } else {
            tf("Inserted {} pages", &[&n])
        };
        self.changed(sel, msg, cx);
    }

    /// The drag for thumbnail `i`: the selection if `i` is in it, else `i`.
    pub(crate) fn page_drag(&self, i: usize, cx: &App) -> Option<PageDrag> {
        let doc = self.doc()?;
        let places: BTreeSet<usize> = if self.page_sel.contains(&i) {
            self.page_sel.clone()
        } else {
            BTreeSet::from([i])
        };
        let pages = places
            .iter()
            .filter_map(|&j| {
                let p = doc.model.page(j)?;
                Some((doc.model.source(p.src)?.clone(), p))
            })
            .collect();
        let _ = cx;
        Some(PageDrag {
            from: self.entity_id,
            places,
            pages,
        })
    }

    /// Pages dropped on the thumbnails or the grid, before page `drop_at`:
    /// moved within this document, copied from another one.
    pub(crate) fn drop_pages(&mut self, drag: &PageDrag, cx: &mut Context<Self>) {
        let Some(at) = self.drop_at.take() else {
            return;
        };
        if drag.from == self.entity_id {
            self.move_pages(&drag.places, at, cx);
        } else {
            self.add_pages(drag.pages.clone(), at, Change::Insert, cx);
        }
    }

    /// Extract Pages: the selected pages as a new PDF (the document is
    /// unchanged).
    pub fn extract_pages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sel = self.target_pages();
        let Some(doc) = self.doc() else {
            return;
        };
        let sel_v: Vec<usize> = sel.iter().copied().collect();
        let plan = doc.model.plan().only(&sel_v);
        let first = sel.first().map_or(1, |f| f + 1);
        let last = sel.last().map_or(1, |l| l + 1);
        let stem = self.stem();
        let name = if first == last {
            format!("{stem} (page {first}).pdf")
        } else {
            format!("{stem} (pages {first}-{last}).pdf")
        };
        let dir = self.dir();
        let count = plan.picks.len();
        let target = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut path))) = target.await else {
                return;
            };
            if path.extension().is_none() {
                path.set_extension("pdf");
            }
            let write = cx.background_spawn({
                let path = path.clone();
                async move {
                    let bytes = crate::model::build(&plan, true)?;
                    kantor_pdf::write_atomic(&path, &bytes)
                }
            });
            let result = write.await;
            let _ = this.update(cx, |this, cx| {
                let file = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                this.status = match result {
                    Ok(()) => tf("Extracted {} pages to {}", &[&count, &file]).into(),
                    Err(e) => tf("Couldn't extract the pages: {}", &[&e]).into(),
                };
                cx.notify();
            });
        })
        .detach();
    }

    // ── undo ──────────────────────────────────────────────────────────────

    pub fn undo_doc(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc_mut() else {
            return;
        };
        match doc.model.undo() {
            Some(c) => {
                let msg = tf("Undid: {}", &[&change_name(c)]);
                self.changed(Some(BTreeSet::new()), msg, cx);
            }
            None => {
                self.status = i18n::t("Nothing to undo").into();
                cx.notify();
            }
        }
    }

    pub fn redo_doc(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc_mut() else {
            return;
        };
        match doc.model.redo() {
            Some(c) => {
                let msg = tf("Redid: {}", &[&change_name(c)]);
                self.changed(Some(BTreeSet::new()), msg, cx);
            }
            None => {
                self.status = i18n::t("Nothing to redo").into();
                cx.notify();
            }
        }
    }

    pub(crate) fn stem(&self) -> String {
        Path::new(&self.name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "PDF".into())
    }

    pub(crate) fn dir(&self) -> PathBuf {
        self.path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .or_else(kantor_core::dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

/// A change's name for Undo / Redo's message.
fn change_name(c: Change) -> &'static str {
    match c {
        Change::Move => i18n::t("Move Pages"),
        Change::Delete => i18n::t("Delete Pages"),
        Change::Rotate => i18n::t("Rotate Pages"),
        Change::Insert => i18n::t("Insert Pages"),
        Change::Duplicate => i18n::t("Duplicate Pages"),
        Change::Paste => i18n::t("Paste Pages"),
        Change::Watermark => i18n::t("Watermark"),
        Change::PageNumbers => i18n::t("Page Numbers"),
        Change::Markup(what) => i18n::t(what),
    }
}

/// The right-click menu of a thumbnail (acting on the selection).
pub(crate) fn page_menu(
    menu: PopupMenu,
    view: &WeakEntity<PdfView>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    type Run = fn(&mut PdfView, &mut Window, &mut Context<PdfView>);
    let item = |label: &'static str, run: Run| {
        let v = view.clone();
        PopupMenuItem::new(i18n::t(label)).on_click(move |_, window, cx| {
            let _ = v.update(cx, |v, cx| run(v, window, cx));
        })
    };
    let n = view
        .upgrade()
        .map_or(1, |v| v.read(cx).target_pages().len());
    let has_clip = CLIPBOARD.with(|c| !c.borrow().is_empty());
    let (v1, v2) = (view.clone(), view.clone());
    menu.item(item(tk("Rotate Left"), |v, _, cx| {
        v.rotate_pages(false, cx)
    }))
    .item(item(tk("Rotate Right"), |v, _, cx| {
        v.rotate_pages(true, cx)
    }))
    .separator()
    .item(item(tk("Cut"), |v, _, cx| v.cut_pages(cx)))
    .item(item(tk("Copy"), |v, _, cx| v.copy_pages(cx)))
    .item(item(tk("Paste"), |v, _, cx| v.paste_pages(cx)).disabled(!has_clip))
    .item(item(tk("Duplicate"), |v, _, cx| v.duplicate_pages(cx)))
    .item(item(tk("Delete"), |v, _, cx| v.delete_pages(cx)))
    .separator()
    .submenu(i18n::t("Insert Pages"), window, cx, move |m, _, _| {
        let item = |label: &'static str, run: Run| {
            let v = v1.clone();
            PopupMenuItem::new(i18n::t(label)).on_click(move |_, window, cx| {
                let _ = v.update(cx, |v, cx| run(v, window, cx));
            })
        };
        m.item(item(tk("From File Before…"), |v, w, cx| {
            v.insert_from_file(false, w, cx)
        }))
        .item(item(tk("From File After…"), |v, w, cx| {
            v.insert_from_file(true, w, cx)
        }))
        .separator()
        .item(item(tk("Blank Page Before"), |v, _, cx| {
            v.insert_blank_page(false, cx)
        }))
        .item(item(tk("Blank Page After"), |v, _, cx| {
            v.insert_blank_page(true, cx)
        }))
    })
    .item({
        let v = v2;
        PopupMenuItem::new(if n == 1 {
            i18n::t("Extract Page…").to_owned()
        } else {
            tf("Extract {} Pages…", &[&n])
        })
        .on_click(move |_, window, cx| {
            let _ = v.update(cx, |v, cx| v.extract_pages(window, cx));
        })
    })
    .separator()
    .item(item(tk("Select All Pages"), |v, _, cx| {
        v.select_all_pages(cx)
    }))
}

/// The line that shows where dropped pages go: across a thumbnail row
/// (`vertical` false) or beside a grid cell.
pub(crate) fn drop_line(vertical: bool, cx: &App) -> Div {
    let t = tokens(cx);
    let line = div().absolute().bg(t.accent).rounded(px(2.));
    if vertical {
        line.top(px(4.)).bottom(px(4.)).w(px(4.))
    } else {
        line.left(px(8.)).right(px(8.)).h(px(4.))
    }
}

/// A page box with its number under it, as in the thumbnails and the grid.
pub(crate) fn page_box(
    image: Option<Arc<RenderImage>>,
    (w, h): (f32, f32),
    number: usize,
    selected: bool,
    current: bool,
    cx: &App,
) -> Div {
    let t = tokens(cx);
    let lit = selected || current;
    v_flex()
        .items_center()
        .gap(SPACE_2)
        .child(
            div()
                .w(px(w + 8.0))
                .h(px(h + 8.0))
                .p(px(2.))
                .rounded(RADIUS_SM)
                .border_2()
                .border_color(if selected {
                    t.accent
                } else if current {
                    t.accent.opacity(0.45)
                } else {
                    transparent_black()
                })
                .when(selected, |d| d.bg(t.accent_soft))
                .child(
                    div()
                        .size_full()
                        .bg(t.paper)
                        .border_1()
                        .border_color(t.border_strong)
                        .overflow_hidden()
                        .children(image.map(|i| img(i).size_full())),
                ),
        )
        .child(
            div()
                .text_size(FONT_CAPTION)
                .text_color(if lit { t.accent } else { t.fg_dim })
                .when(lit, |d| d.font_weight(FontWeight::SEMIBOLD))
                .child(format!("{number}")),
        )
}
