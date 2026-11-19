//! The PDF tools' dialogs, on `kantor_pdf`: Combine Files, Split, Add Page
//! Numbers, Watermark and Reduce File Size. Each is a card over the pane
//! (modal, Escape cancels); the work runs in the background.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    Disableable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_pdf::{PageNumberOptions, PageRanges, Position, WatermarkOptions};
use kantor_ui::i18n::{self, tf, tk};
use kantor_ui::{metrics::*, tokens};

use crate::PdfView;
use crate::model::{Change, Source};

/// How Split cuts the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SplitBy {
    Every,
    Bookmarks,
    Ranges,
}

/// Page number styles (Acrobat's Header & Footer offers similar ones).
const NUMBER_FORMATS: &[(&str, &str)] = &[
    ("{n}", "1"),
    ("Page {n}", tk("Page 1")),
    ("Page {n} of {total}", tk("Page 1 of N")),
    ("{n} / {total}", "1 / N"),
];

const POSITIONS: &[(Position, &str)] = &[
    (Position::TopLeft, tk("Top left")),
    (Position::TopCenter, tk("Top center")),
    (Position::TopRight, tk("Top right")),
    (Position::BottomLeft, tk("Bottom left")),
    (Position::BottomCenter, tk("Bottom center")),
    (Position::BottomRight, tk("Bottom right")),
];

pub(crate) enum Dialog {
    Combine {
        files: Vec<PathBuf>,
    },
    Split {
        by: SplitBy,
        every: Entity<InputState>,
        ranges: Entity<InputState>,
        error: Option<String>,
    },
    PageNumbers {
        format: usize,
        position: Position,
        start: Entity<InputState>,
        pages: Entity<InputState>,
        error: Option<String>,
    },
    Watermark {
        text: Entity<InputState>,
        size: Entity<InputState>,
        opacity: u8,
        diagonal: bool,
        red: bool,
        pages: Entity<InputState>,
        error: Option<String>,
    },
    Reduce,
}

fn input(
    window: &mut Window,
    cx: &mut Context<PdfView>,
    value: &str,
    hint: &str,
) -> Entity<InputState> {
    let i = cx.new(|cx| InputState::new(window, cx).placeholder(hint.to_owned()));
    i.update(cx, |i, cx| i.set_value(value.to_owned(), window, cx));
    i
}

/// The card of a dialog: title, body, buttons.
fn card(title: &str, width: f32, cx: &App) -> Stateful<Div> {
    let t = tokens(cx);
    v_flex()
        .id("pdf-dialog")
        .occlude()
        .w(px(width))
        .max_h(px(620.))
        .gap(SPACE_5)
        .p(SPACE_6)
        .rounded(RADIUS_MD)
        .bg(t.surface)
        .border_1()
        .border_color(t.border_strong)
        .shadow_md()
        .text_size(FONT_BODY)
        .text_color(t.fg)
        .child(
            div()
                .flex_none()
                .text_size(FONT_EMPHASIS)
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.to_owned()),
        )
}

/// `label` on the left, the control filling the rest.
fn row(label: &str, control: impl IntoElement, cx: &App) -> Div {
    let t = tokens(cx);
    h_flex()
        .gap(SPACE_4)
        .min_h(CONTROL_MD)
        .child(
            div()
                .flex_none()
                .w(px(112.))
                .text_size(FONT_SMALL)
                .text_color(t.fg_muted)
                .child(label.to_owned()),
        )
        .child(div().flex_1().min_w_0().child(control))
}

fn note(text: impl Into<SharedString>, error: bool, cx: &App) -> Div {
    let t = tokens(cx);
    div()
        .text_size(FONT_SMALL)
        .text_color(if error { t.danger } else { t.fg_muted })
        .child(text.into())
}

type Set<T> = fn(&mut Dialog, T);

/// A row of buttons, one per option, the current one filled.
fn choices<T: Copy + PartialEq + 'static>(
    id: &'static str,
    options: &[(T, String)],
    current: T,
    view: &WeakEntity<PdfView>,
    set: Set<T>,
) -> Div {
    let mut row = h_flex().gap(SPACE_2).flex_wrap();
    for (i, (value, label)) in options.iter().enumerate() {
        let v = view.clone();
        let value = *value;
        row = row.child(
            Button::new((id, i))
                .small()
                .label(label.clone())
                .when_else(value == current, |b| b.primary(), |b| b.outline())
                .on_click(move |_, _, cx| {
                    let _ = v.update(cx, |this, cx| {
                        if let Some(d) = this.dialog.as_mut() {
                            set(d, value);
                        }
                        cx.notify();
                    });
                }),
        );
    }
    row
}

type Run = fn(&mut PdfView, &mut Window, &mut Context<PdfView>);

fn buttons(ok_label: &str, ok_enabled: bool, view: &WeakEntity<PdfView>, ok: Run) -> Div {
    let (ok_view, cancel_view) = (view.clone(), view.clone());
    h_flex()
        .flex_none()
        .justify_end()
        .gap(SPACE_3)
        .child(
            Button::new("pdf-dialog-cancel")
                .small()
                .ghost()
                .label(i18n::t("Cancel"))
                .on_click(move |_, w, cx| {
                    let _ = cancel_view.update(cx, |this, cx| this.close_dialog(w, cx));
                }),
        )
        .child(
            Button::new("pdf-dialog-ok")
                .small()
                .primary()
                .label(ok_label.to_owned())
                .disabled(!ok_enabled)
                .on_click(move |_, w, cx| {
                    let _ = ok_view.update(cx, |this, cx| ok(this, w, cx));
                }),
        )
}

/// A page range typed in a dialog; empty means every page.
fn parse_pages(text: &str) -> Result<PageRanges, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(PageRanges::all());
    }
    PageRanges::parse(text).map_err(|_| {
        i18n::t("Type pages like 1-3, 5, 8- (or leave it empty for all pages)").to_owned()
    })
}

/// A whole number typed in a dialog.
fn parse_int(text: &str) -> Option<i64> {
    text.trim().parse().ok()
}

/// `name` made safe as a file name (bookmark titles in Split).
pub(crate) fn file_safe(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let s = s.trim().trim_matches('.').trim();
    let s: String = s.chars().take(80).collect();
    if s.is_empty() { "part".into() } else { s }
}

/// `dir/name.pdf`, or `dir/name (2).pdf`… when that file exists.
pub(crate) fn free_path(dir: &Path, name: &str, taken: &mut BTreeSet<PathBuf>) -> PathBuf {
    let mut path = dir.join(format!("{name}.pdf"));
    let mut k = 2;
    while path.exists() || taken.contains(&path) {
        path = dir.join(format!("{name} ({k}).pdf"));
        k += 1;
    }
    taken.insert(path.clone());
    path
}

/// Split groups (1-based pages) at the top-level bookmarks' pages: one part
/// per bookmark, plus the pages before the first one. Each part is named
/// after its bookmark.
pub(crate) fn bookmark_groups(
    starts: &[(usize, String)],
    n: usize,
) -> Vec<(Vec<u32>, Option<String>)> {
    let mut starts: Vec<(usize, String)> = starts.iter().filter(|(p, _)| *p < n).cloned().collect();
    starts.sort_by_key(|(p, _)| *p);
    starts.dedup_by_key(|(p, _)| *p);
    let mut out = Vec::new();
    if starts.first().is_none_or(|(p, _)| *p > 0) {
        let end = starts.first().map_or(n, |(p, _)| *p);
        out.push(((1..=end as u32).collect(), None));
    }
    for (k, (p, title)) in starts.iter().enumerate() {
        let end = starts.get(k + 1).map_or(n, |(q, _)| *q);
        out.push((
            ((*p + 1) as u32..=end as u32).collect(),
            Some(title.clone()),
        ));
    }
    out
}

impl PdfView {
    pub(crate) fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog = None;
        self.focus(window, cx);
        cx.notify();
    }

    fn open_dialog(
        &mut self,
        d: Dialog,
        focus: Option<Entity<InputState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.page_count() == 0 {
            return;
        }
        self.dialog = Some(d);
        match focus {
            Some(i) => i.update(cx, |i, cx| {
                i.focus(window, cx);
                i.select_all(window, cx);
            }),
            None => window.focus(&self.dialog_focus, cx),
        }
        cx.notify();
    }

    /// Enter in a dialog's field: OK.
    fn submit_on_enter(
        &mut self,
        i: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sub = cx.subscribe_in(i, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.dialog_ok(window, cx);
            }
        });
        self.dialog_subs.push(sub);
    }

    fn dialog_ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.dialog {
            Some(Dialog::Combine { .. }) => self.combine_ok(window, cx),
            Some(Dialog::Split { .. }) => self.split_ok(window, cx),
            Some(Dialog::PageNumbers { .. }) => self.page_numbers_ok(window, cx),
            Some(Dialog::Watermark { .. }) => self.watermark_ok(window, cx),
            Some(Dialog::Reduce) => self.reduce_ok(window, cx),
            None => {}
        }
    }

    // ── Combine Files ─────────────────────────────────────────────────────

    /// Combine Files…: PDFs and pictures, in an order you choose, into a
    /// new PDF (this one first; its saved file).
    pub fn combine_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let files = if self.path.as_os_str().is_empty() {
            Vec::new()
        } else {
            vec![self.path.clone()]
        };
        self.open_dialog(Dialog::Combine { files }, None, window, cx);
        self.add_combine_files(window, cx);
    }

    fn add_combine_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(i18n::t("Add PDFs or pictures to combine").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(Dialog::Combine { files }) = &mut this.dialog {
                    files.extend(paths.into_iter().filter(|p| crate::organize::insertable(p)));
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn combine_ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Dialog::Combine { files }) = &self.dialog else {
            return;
        };
        let files = files.clone();
        if files.is_empty() {
            return;
        }
        self.close_dialog(window, cx);
        let target =
            cx.prompt_for_new_path(&self.dir(), Some(&format!("{}.pdf", i18n::t("Combined"))));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut path))) = target.await else {
                return;
            };
            if path.extension().is_none() {
                path.set_extension("pdf");
            }
            let _ = this.update(cx, |this, cx| {
                this.busy = Some(i18n::t("Combining files…").into());
                cx.notify();
            });
            let out = path.clone();
            let work = cx.background_spawn(async move { combine(&files, &out) });
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                let name = file_name(&path);
                match result {
                    Ok(n) => {
                        this.status = tf("Combined {} files into {}", &[&n, &name]).into();
                        cx.emit(PdfEvent::Open(path));
                    }
                    Err(e) => this.status = e.into(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ── Split ─────────────────────────────────────────────────────────────

    pub fn split_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let every = input(window, cx, "1", "");
        let ranges = input(window, cx, "", &i18n::t("e.g. 1-3, 4-10, 11-"));
        self.dialog_subs.clear();
        self.submit_on_enter(&every, window, cx);
        self.submit_on_enter(&ranges, window, cx);
        let by = if self.outline().is_empty() {
            SplitBy::Every
        } else {
            SplitBy::Bookmarks
        };
        let focus = (by == SplitBy::Every).then(|| every.clone());
        self.open_dialog(
            Dialog::Split {
                by,
                every,
                ranges,
                error: None,
            },
            focus,
            window,
            cx,
        );
    }

    /// The parts Split makes now: (pages, name) in document order.
    fn split_groups(&self, cx: &App) -> Result<Vec<(Vec<u32>, Option<String>)>, String> {
        let Some(Dialog::Split {
            by, every, ranges, ..
        }) = &self.dialog
        else {
            return Err(String::new());
        };
        let n = self.page_count();
        match by {
            SplitBy::Every => {
                let k = parse_int(&every.read(cx).value())
                    .filter(|k| *k >= 1)
                    .ok_or_else(|| {
                        i18n::t("Type how many pages go in each file (1 or more)").to_owned()
                    })?;
                let groups = kantor_pdf::every(n as u32, k as u32).map_err(|e| e.to_string())?;
                Ok(groups.into_iter().map(|g| (g, None)).collect())
            }
            SplitBy::Bookmarks => {
                let base = self.doc().map(|d| d.model.base().id);
                let starts: Vec<(usize, String)> = self
                    .outline()
                    .iter()
                    .filter_map(|b| {
                        let t = self.map_target(base?, b.target.clone()?)?;
                        match t {
                            pdf_engine::LinkTarget::Dest(d) => Some((d.page, b.title.clone())),
                            _ => None,
                        }
                    })
                    .collect();
                if starts.is_empty() {
                    return Err(i18n::t("This document has no bookmarks to split at").to_owned());
                }
                Ok(bookmark_groups(&starts, n))
            }
            SplitBy::Ranges => {
                let text = ranges.read(cx).value().to_string();
                if text.trim().is_empty() {
                    return Err(
                        i18n::t("Type the pages of each file, like 1-3, 4-10, 11-").to_owned()
                    );
                }
                let r = parse_pages(&text)?;
                let groups = r.resolve_groups(n as u32).map_err(|e| e.to_string())?;
                let labels = r.item_labels();
                Ok(groups
                    .into_iter()
                    .zip(labels)
                    .map(|(g, l)| (g, Some(tf("pages {}", &[&l]))))
                    .collect())
            }
        }
    }

    fn split_ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let groups = match self.split_groups(cx) {
            Ok(g) => g,
            Err(e) => {
                if let Some(Dialog::Split { error, .. }) = &mut self.dialog {
                    *error = Some(e);
                }
                cx.notify();
                return;
            }
        };
        self.close_dialog(window, cx);
        let Some(doc) = self.doc() else {
            return;
        };
        let (sources, picks) = doc.model.picks();
        let stem = self.stem();
        let folder = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(i18n::t("Choose a folder for the parts").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(dirs))) = folder.await else {
                return;
            };
            let Some(dir) = dirs.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.busy = Some(i18n::t("Splitting…").into());
                cx.notify();
            });
            let work =
                cx.background_spawn(async move { split(&sources, &picks, &groups, &stem, &dir) });
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                this.status = match result {
                    Ok((n, dir)) => tf("Split into {} files in {}", &[&n, &dir.display()]).into(),
                    Err(e) => tf("Split failed: {}", &[&e]).into(),
                };
                cx.notify();
            });
        })
        .detach();
    }

    // ── Page numbers and watermark (they change the pages) ───────────────

    pub fn add_page_numbers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let start = input(window, cx, "1", "");
        let pages = input(window, cx, "", &i18n::t("All pages"));
        self.dialog_subs.clear();
        self.submit_on_enter(&start, window, cx);
        self.submit_on_enter(&pages, window, cx);
        self.open_dialog(
            Dialog::PageNumbers {
                format: 0,
                position: Position::BottomCenter,
                start,
                pages,
                error: None,
            },
            None,
            window,
            cx,
        );
    }

    fn page_numbers_ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Dialog::PageNumbers {
            format,
            position,
            start,
            pages,
            error,
        }) = &mut self.dialog
        else {
            return;
        };
        let first = match parse_int(&start.read(cx).value()) {
            Some(s) if s >= 0 => s,
            _ => {
                *error = Some(i18n::t("Start at: type a whole number").into());
                cx.notify();
                return;
            }
        };
        let ranges = match parse_pages(&pages.read(cx).value()) {
            Ok(r) => r,
            Err(e) => {
                *error = Some(e);
                cx.notify();
                return;
            }
        };
        let opts = PageNumberOptions {
            format: NUMBER_FORMATS[*format].0.to_owned(),
            position: *position,
            start: first,
            ..PageNumberOptions::default()
        };
        self.close_dialog(window, cx);
        self.bake(
            Change::PageNumbers,
            i18n::t("Adding page numbers…"),
            cx,
            move |pdf| {
                let n = kantor_pdf::add_page_numbers(pdf, &opts, &ranges)?;
                Ok(tf("Numbered {} pages", &[&n]))
            },
        );
    }

    pub fn add_watermark(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = input(window, cx, "", &i18n::t("e.g. DRAFT or CONFIDENTIAL"));
        let size = input(window, cx, "60", "");
        let pages = input(window, cx, "", &i18n::t("All pages"));
        self.dialog_subs.clear();
        for i in [&text, &size, &pages] {
            self.submit_on_enter(i, window, cx);
        }
        let focus = Some(text.clone());
        self.open_dialog(
            Dialog::Watermark {
                text,
                size,
                opacity: 30,
                diagonal: true,
                red: false,
                pages,
                error: None,
            },
            focus,
            window,
            cx,
        );
    }

    fn watermark_ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Dialog::Watermark {
            text,
            size,
            opacity,
            diagonal,
            red,
            pages,
            error,
        }) = &mut self.dialog
        else {
            return;
        };
        let words = text.read(cx).value().trim().to_owned();
        if words.is_empty() {
            *error = Some(i18n::t("Type the watermark's text").into());
            cx.notify();
            return;
        }
        if !words.chars().all(|c| (' '..='\u{ff}').contains(&c)) {
            *error = Some(
                i18n::t("The watermark can use Latin letters, digits and punctuation only").into(),
            );
            cx.notify();
            return;
        }
        let font_size = match parse_int(&size.read(cx).value()) {
            Some(s) if (6..=400).contains(&s) => s as f32,
            _ => {
                *error = Some(i18n::t("Font size: type a number from 6 to 400").into());
                cx.notify();
                return;
            }
        };
        let ranges = match parse_pages(&pages.read(cx).value()) {
            Ok(r) => r,
            Err(e) => {
                *error = Some(e);
                cx.notify();
                return;
            }
        };
        let opts = WatermarkOptions {
            text: words,
            font_size,
            opacity: f32::from(*opacity) / 100.0,
            rotation: if *diagonal { 45.0 } else { 0.0 },
            color: if *red {
                [0.8, 0.1, 0.1]
            } else {
                [0.5, 0.5, 0.5]
            },
            ..WatermarkOptions::default()
        };
        self.close_dialog(window, cx);
        self.bake(
            Change::Watermark,
            i18n::t("Adding the watermark…"),
            cx,
            move |pdf| {
                let n = kantor_pdf::add_watermark(pdf, &opts, &ranges)?;
                Ok(tf("Watermarked {} pages", &[&n]))
            },
        );
    }

    /// Run `op` on the document as built now (in the background); its
    /// result replaces the document (one undo step). A password-protected
    /// document stays protected.
    fn bake(
        &mut self,
        change: Change,
        busy: &str,
        cx: &mut Context<Self>,
        op: impl FnOnce(&mut kantor_pdf::Pdf) -> kantor_pdf::Result<String> + Send + 'static,
    ) {
        let Some(doc) = self.doc() else {
            return;
        };
        let (sources, picks) = doc.model.picks();
        let name = self.name.clone();
        self.busy = Some(busy.to_owned().into());
        cx.notify();
        let work = cx.background_spawn(async move {
            let pdfs = sources
                .iter()
                .map(|s| s.lopdf())
                .collect::<kantor_pdf::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?;
            let mut pdf = kantor_pdf::assemble(pdfs, &picks).map_err(|e| e.to_string())?;
            let msg = op(&mut pdf).map_err(|e| e.to_string())?;
            let password = sources[0].password.clone();
            let (bytes, password) = match pdf.to_bytes_encrypted(true) {
                Ok(b) => (b, password),
                Err(_) => (
                    pdf.to_bytes_compact().map_err(|e| e.to_string())?,
                    String::new(),
                ),
            };
            let source =
                Source::read(name, Arc::new(bytes), password).map_err(|e| e.to_string())?;
            Ok::<_, String>((source, msg))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Ok((source, msg)) => {
                        let source = Arc::new(source);
                        if let Some(w) = &this.worker {
                            w.add_source(source.clone());
                        }
                        if let Some(doc) = this.doc_mut() {
                            doc.model.replace(source, change);
                        }
                        this.after_change(BTreeSet::new(), cx);
                        this.status = msg.into();
                    }
                    Err(e) => this.status = tf("Couldn't change the pages: {}", &[&e]).into(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ── Reduce File Size ──────────────────────────────────────────────────

    pub fn reduce_file_size(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_dialog(Dialog::Reduce, None, window, cx);
    }

    fn reduce_ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_dialog(window, cx);
        let Some(doc) = self.doc() else {
            return;
        };
        let (sources, picks) = doc.model.picks();
        let name = format!("{} ({}).pdf", self.stem(), i18n::t("smaller"));
        let target = cx.prompt_for_new_path(&self.dir(), Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut path))) = target.await else {
                return;
            };
            if path.extension().is_none() {
                path.set_extension("pdf");
            }
            let _ = this.update(cx, |this, cx| {
                this.busy = Some(i18n::t("Reducing the file size…").into());
                cx.notify();
            });
            let out = path.clone();
            let work = cx.background_spawn(async move {
                let pdfs = sources
                    .iter()
                    .map(|s| s.lopdf())
                    .collect::<kantor_pdf::Result<Vec<_>>>()?;
                let mut pdf = kantor_pdf::assemble(pdfs, &picks)?;
                let before = sources[0].bytes.len();
                let plain = pdf.to_bytes_compact()?;
                let (bytes, _) = kantor_pdf::compress(pdf, &plain)?;
                kantor_pdf::write_atomic(&out, &bytes)?;
                Ok::<_, kantor_pdf::Error>((before, bytes.len()))
            });
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                let name = file_name(&path);
                this.status = match result {
                    Ok((before, after)) => tf(
                        "Saved {}: {} (was {})",
                        &[
                            &name,
                            &crate::size_text(after as u64),
                            &crate::size_text(before as u64),
                        ],
                    )
                    .into(),
                    Err(e) => tf("Couldn't reduce the file size: {}", &[&e]).into(),
                };
                cx.notify();
            });
        })
        .detach();
    }

    // ── drawing ───────────────────────────────────────────────────────────

    pub(crate) fn render_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.dialog.as_ref()?;
        let view = cx.entity().downgrade();
        let t = tokens(cx);
        let card = match d {
            Dialog::Combine { files } => {
                let mut list = v_flex().gap(SPACE_1).max_h(px(320.)).overflow_y_hidden();
                for (i, f) in files.iter().enumerate() {
                    let (up, down, remove) = (view.clone(), view.clone(), view.clone());
                    let n = files.len();
                    let mv = |v: &WeakEntity<PdfView>, delta: isize| {
                        let v = v.clone();
                        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                            let _ = v.update(cx, |this, cx| {
                                if let Some(Dialog::Combine { files }) = &mut this.dialog {
                                    let j = i as isize + delta;
                                    if j >= 0 && (j as usize) < files.len() {
                                        files.swap(i, j as usize);
                                    }
                                }
                                cx.notify();
                            });
                        }
                    };
                    list = list.child(
                        h_flex()
                            .gap(SPACE_2)
                            .px(SPACE_3)
                            .h(CONTROL_MD)
                            .rounded(RADIUS_SM)
                            .bg(t.chrome)
                            .child(
                                div()
                                    .w(px(20.))
                                    .text_size(FONT_SMALL)
                                    .text_color(t.fg_dim)
                                    .child(format!("{}", i + 1)),
                            )
                            .child(div().flex_1().min_w_0().truncate().child(file_name(f)))
                            .child(
                                Button::new(("pdf-combine-up", i))
                                    .ghost()
                                    .xsmall()
                                    .icon(Lucide::ChevronUp)
                                    .tooltip(i18n::t("Move up"))
                                    .disabled(i == 0)
                                    .on_click(mv(&up, -1)),
                            )
                            .child(
                                Button::new(("pdf-combine-down", i))
                                    .ghost()
                                    .xsmall()
                                    .icon(Lucide::ChevronDown)
                                    .tooltip(i18n::t("Move down"))
                                    .disabled(i + 1 == n)
                                    .on_click(mv(&down, 1)),
                            )
                            .child(
                                Button::new(("pdf-combine-remove", i))
                                    .ghost()
                                    .xsmall()
                                    .icon(Lucide::X)
                                    .tooltip(i18n::t("Remove"))
                                    .on_click(move |_, _, cx| {
                                        let _ = remove.update(cx, |this, cx| {
                                            if let Some(Dialog::Combine { files }) = &mut this.dialog {
                                                files.remove(i);
                                            }
                                            cx.notify();
                                        });
                                    }),
                            ),
                    );
                }
                let add = view.clone();
                card(i18n::t("Combine Files"), 480.0, cx)
                    .child(note(
                        i18n::t("PDFs and pictures are combined in this order into a new PDF, which opens in Kantor."),
                        false,
                        cx,
                    ))
                    .child(list)
                    .child(
                        h_flex().child(
                            Button::new("pdf-combine-add")
                                .small()
                                .outline()
                                .icon(Lucide::Plus)
                                .label(i18n::t("Add Files…"))
                                .on_click(move |_, w, cx| {
                                    let _ = add.update(cx, |this, cx| this.add_combine_files(w, cx));
                                }),
                        ),
                    )
                    .when(files.len() < 2, |c| {
                        c.child(note(i18n::t("Add at least two files to combine."), false, cx))
                    })
                    .child(buttons(i18n::t("Combine…"), files.len() >= 2, &view, Self::combine_ok))
            }
            Dialog::Split {
                by,
                every,
                ranges,
                error,
            } => {
                let options = [
                    (SplitBy::Every, i18n::t("Every N pages").to_owned()),
                    (SplitBy::Bookmarks, i18n::t("At top-level bookmarks").to_owned()),
                    (SplitBy::Ranges, i18n::t("By page ranges").to_owned()),
                ];
                let preview = match self.split_groups(cx) {
                    Ok(g) if g.len() == 1 => i18n::t("Makes 1 file").to_owned(),
                    Ok(g) => tf("Makes {} files", &[&g.len()]),
                    Err(_) => String::new(),
                };
                let mut c = card(i18n::t("Split Document"), 460.0, cx)
                    .child(row(
                        i18n::t("Split"),
                        choices("pdf-split-by", &options, *by, &view, |d, v| {
                            if let Dialog::Split { by, error, .. } = d {
                                *by = v;
                                *error = None;
                            }
                        }),
                        cx,
                    ));
                c = match by {
                    SplitBy::Every => c.child(row(
                        i18n::t("Pages per file"),
                        div().w(px(90.)).child(Input::new(every).small()),
                        cx,
                    )),
                    SplitBy::Ranges => c.child(row(i18n::t("Pages of each file"), Input::new(ranges).small(), cx)),
                    SplitBy::Bookmarks => c.child(note(
                        i18n::t("Each top-level bookmark starts a file named after it."),
                        false,
                        cx,
                    )),
                };
                c.child(note(
                    i18n::t("The files are saved in a folder you choose next. This document doesn't change."),
                    false,
                    cx,
                ))
                .when(!preview.is_empty(), |c| c.child(note(preview, false, cx)))
                .when_some(error.clone(), |c, e| c.child(note(e, true, cx)))
                .child(buttons(i18n::t("Split…"), true, &view, Self::split_ok))
            }
            Dialog::PageNumbers {
                format,
                position,
                start,
                pages,
                error,
            } => {
                let formats: Vec<(usize, String)> = NUMBER_FORMATS
                    .iter()
                    .enumerate()
                    .map(|(i, (_, label))| (i, i18n::t(label).to_owned()))
                    .collect();
                let positions: Vec<(Position, String)> =
                    POSITIONS.iter().map(|(p, l)| (*p, i18n::t(l).to_owned())).collect();
                card(i18n::t("Add Page Numbers"), 480.0, cx)
                    .child(row(
                        i18n::t("Style"),
                        choices("pdf-num-format", &formats, *format, &view, |d, v| {
                            if let Dialog::PageNumbers { format, .. } = d {
                                *format = v;
                            }
                        }),
                        cx,
                    ))
                    .child(row(
                        i18n::t("Position"),
                        choices("pdf-num-pos", &positions, *position, &view, |d, v| {
                            if let Dialog::PageNumbers { position, .. } = d {
                                *position = v;
                            }
                        }),
                        cx,
                    ))
                    .child(row(
                        i18n::t("Start at"),
                        div().w(px(90.)).child(Input::new(start).small()),
                        cx,
                    ))
                    .child(row(i18n::t("Pages"), Input::new(pages).small(), cx))
                    .when_some(error.clone(), |c, e| c.child(note(e, true, cx)))
                    .child(buttons(i18n::t("Add"), true, &view, Self::page_numbers_ok))
            }
            Dialog::Watermark {
                text,
                size,
                opacity,
                diagonal,
                red,
                pages,
                error,
            } => {
                let opacities: Vec<(u8, String)> =
                    [15u8, 30, 50, 80].iter().map(|o| (*o, format!("{o}%"))).collect();
                let angles = [
                    (true, i18n::t("Diagonal").to_owned()),
                    (false, i18n::t("Horizontal").to_owned()),
                ];
                let colors = [
                    (false, i18n::t("Gray").to_owned()),
                    (true, i18n::t("Red").to_owned()),
                ];
                card(i18n::t("Watermark"), 460.0, cx)
                    .child(row(i18n::t("Text"), Input::new(text).small(), cx))
                    .child(row(
                        i18n::t("Font size"),
                        div().w(px(90.)).child(Input::new(size).small()),
                        cx,
                    ))
                    .child(row(
                        i18n::t("Opacity"),
                        choices("pdf-wm-opacity", &opacities, *opacity, &view, |d, v| {
                            if let Dialog::Watermark { opacity, .. } = d {
                                *opacity = v;
                            }
                        }),
                        cx,
                    ))
                    .child(row(
                        i18n::t("Angle"),
                        choices("pdf-wm-angle", &angles, *diagonal, &view, |d, v| {
                            if let Dialog::Watermark { diagonal, .. } = d {
                                *diagonal = v;
                            }
                        }),
                        cx,
                    ))
                    .child(row(
                        i18n::t("Color"),
                        choices("pdf-wm-color", &colors, *red, &view, |d, v| {
                            if let Dialog::Watermark { red, .. } = d {
                                *red = v;
                            }
                        }),
                        cx,
                    ))
                    .child(row(i18n::t("Pages"), Input::new(pages).small(), cx))
                    .when_some(error.clone(), |c, e| c.child(note(e, true, cx)))
                    .child(buttons(i18n::t("Add"), true, &view, Self::watermark_ok))
            }
            Dialog::Reduce => card(i18n::t("Reduce File Size"), 440.0, cx)
                .child(note(
                    i18n::t("Kantor writes a smaller copy of this document: unused objects are removed, repeated pictures and fonts are stored once and everything is compressed. Pictures keep their quality. This document doesn't change."),
                    false,
                    cx,
                ))
                .child(note(
                    i18n::t("The copy has no password, even if this document has one."),
                    false,
                    cx,
                ))
                .child(buttons(i18n::t("Save Smaller Copy…"), true, &view, Self::reduce_ok)),
        };
        let close = view.clone();
        let focus = self.dialog_focus.clone();
        let card = card.track_focus(&focus).on_key_down(move |e, window, cx| {
            if e.keystroke.key == "escape" {
                cx.stop_propagation();
                let _ = close.update(cx, |this, cx| this.close_dialog(window, cx));
            }
        });
        Some(
            kantor_ui::overlay::backdrop()
                .child(
                    h_flex()
                        .absolute()
                        .top(CONTROL_LG * 2. + SPACE_5)
                        .left_0()
                        .right_0()
                        .justify_center()
                        .child(card),
                )
                .into_any_element(),
        )
    }
}

/// Events for the workspace.
#[derive(Clone, Debug, PartialEq)]
pub enum PdfEvent {
    /// Open this file in a new tab (a combined PDF).
    Open(PathBuf),
}

impl EventEmitter<PdfEvent> for PdfView {}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Combine `files` (PDFs and pictures, in order) into `out`. Returns how
/// many files went in.
fn combine(files: &[PathBuf], out: &Path) -> Result<usize, String> {
    let mut pdfs = Vec::new();
    for f in files {
        let s = crate::organize::load_file(f)?;
        let pdf = s
            .lopdf()
            .map_err(|e| tf("Couldn't read {}: {}", &[&file_name(f), &e]))?;
        pdfs.push(pdf);
    }
    let mut it = pdfs.into_iter();
    let Some(first) = it.next() else {
        return Err(i18n::t("No files to combine").into());
    };
    let mut pdf = kantor_pdf::merge(first, it.collect()).map_err(|e| e.to_string())?;
    let bytes = pdf.to_bytes_compact().map_err(|e| e.to_string())?;
    kantor_pdf::write_atomic(out, &bytes).map_err(|e| e.to_string())?;
    Ok(files.len())
}

/// Write each group of the document built from `sources` / `picks` as
/// its own PDF in `dir`. Returns how many files and the folder.
fn split(
    sources: &[Arc<Source>],
    picks: &[kantor_pdf::Pick],
    groups: &[(Vec<u32>, Option<String>)],
    stem: &str,
    dir: &Path,
) -> Result<(usize, PathBuf), String> {
    let pdfs = sources
        .iter()
        .map(|s| s.lopdf())
        .collect::<kantor_pdf::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    let pdf = kantor_pdf::assemble(pdfs, picks).map_err(|e| e.to_string())?;
    let mut taken = BTreeSet::new();
    let names: Vec<PathBuf> = groups
        .iter()
        .enumerate()
        .map(|(i, (_, title))| {
            let name = match title {
                Some(t) => format!("{stem} - {}", file_safe(t)),
                None => format!("{stem} - {}", tf("part {}", &[&(i + 1)])),
            };
            free_path(dir, &name, &mut taken)
        })
        .collect();
    let pages: Vec<Vec<u32>> = groups.iter().map(|(g, _)| g.clone()).collect();
    let mut k = 0;
    kantor_pdf::split(&pdf, &pages, |mut part| {
        let bytes = part.pdf.to_bytes_compact()?;
        kantor_pdf::write_atomic(&names[k], &bytes)?;
        k += 1;
        Ok(())
    })
    .map_err(|e| e.to_string())?;
    Ok((k, dir.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn bookmark_parts_start_at_each_bookmark() {
        let starts = [
            (3, "Two".to_owned()),
            (0, "One".to_owned()),
            (3, "Dup".to_owned()),
            (7, "Three".to_owned()),
        ];
        let g = bookmark_groups(&starts, 9);
        let pages: Vec<Vec<u32>> = g.iter().map(|(p, _)| p.clone()).collect();
        assert_eq!(pages, [vec![1, 2, 3], vec![4, 5, 6, 7], vec![8, 9]]);
        assert_eq!(g[0].1.as_deref(), Some("One"));
        // Pages before the first bookmark make their own part.
        let g = bookmark_groups(&[(2, "Body".into())], 4);
        assert_eq!(g[0], (vec![1, 2], None));
        assert_eq!(g[1], (vec![3, 4], Some("Body".into())));
    }

    #[std::prelude::v1::test]
    fn safe_names_and_free_paths() {
        assert_eq!(file_safe("Ch. 1: Intro/Start?"), "Ch. 1- Intro-Start-");
        assert_eq!(file_safe("..."), "part");
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.pdf"), b"x").unwrap();
        let mut taken = BTreeSet::new();
        assert_eq!(
            free_path(dir.path(), "a", &mut taken),
            dir.path().join("a (2).pdf")
        );
        assert_eq!(
            free_path(dir.path(), "a", &mut taken),
            dir.path().join("a (3).pdf")
        );
        assert_eq!(
            free_path(dir.path(), "b", &mut taken),
            dir.path().join("b.pdf")
        );
    }

    #[std::prelude::v1::test]
    fn page_range_text() {
        assert!(parse_pages("").is_ok());
        assert!(parse_pages("1-3, 5").is_ok());
        assert!(parse_pages("abc").is_err());
    }
}
