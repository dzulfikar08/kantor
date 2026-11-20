//! Tools ▸ Batch…: the same steps on many PDFs (like Acrobat's Action
//! Wizard, kept simple), with `kantor_pdf::batch`. Pick files or a folder
//! (or drop them on the dialog), add steps in order, choose where the
//! results go, run: the files are processed in the background with
//! progress, Cancel, and a result per file (done, skipped and why, failed
//! and why). A batch can be saved and loaded as a JSON file, the same one
//! `kantor pdf batch` runs.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::dialog::{DialogDescription, DialogFooter};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{
    Disableable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_pdf::Position;
use kantor_pdf::batch::{
    self, Anchor, Batch, Example, Existing, FileResult, Fill, Output, PageNumbers, Place, Plugin,
    Plugins, Prepared, Reason, Stamp, Status, Step, Watermark,
};
use kantor_ui::i18n::{self, tf, tk};
use kantor_ui::{metrics::*, tokens};

use crate::PdfView;
use crate::dialog::{Dialog, NUMBER_FORMATS, POSITIONS, card, choices, note, row};

// ── plugins ────────────────────────────────────────────────────────────────

fn registry() -> &'static Mutex<Plugins> {
    static R: OnceLock<Mutex<Plugins>> = OnceLock::new();
    R.get_or_init(Mutex::default)
}

/// Make a step from outside `kantor_pdf` (signing with a certificate…)
/// available to batches: its `type` in batch files is `plugin.kind()`.
pub fn register_plugin(plugin: Arc<dyn Plugin>) {
    if let Ok(mut r) = registry().lock() {
        r.add(plugin);
    }
}

fn plugins() -> Plugins {
    registry().lock().map(|r| r.clone()).unwrap_or_default()
}

// ── the dialog's state ─────────────────────────────────────────────────────

/// Which pages a step works on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PagesSel {
    All,
    First,
    Last,
    Range,
}

/// A picture for a stamp step: (what the batch file says, shown name).
type Picture = (String, String);

pub(crate) enum StepUi {
    Stamp {
        signature: bool,
        picture: Option<Picture>,
        pages: PagesSel,
        range: Entity<InputState>,
        anchor: Anchor,
        /// The same spot as the stamp selected in the document.
        example: Option<Example>,
        width: Entity<InputState>,
        opacity: u8,
    },
    Fill {
        table: Option<PathBuf>,
        /// `name=value; name=value` for every file.
        values: Entity<InputState>,
        rows: Option<usize>,
    },
    PageNumbers {
        format: usize,
        position: Position,
    },
    Watermark {
        text: Entity<InputState>,
        opacity: u8,
    },
    Flatten,
    /// A step from a loaded batch file that this window doesn't edit.
    Other(Step),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Where {
    NextTo,
    Folder,
}

pub(crate) enum Stage {
    Edit,
    Running {
        done: usize,
        cancel: Arc<AtomicBool>,
    },
    Finished {
        cancelled: bool,
    },
}

pub(crate) struct BatchState {
    files: Vec<PathBuf>,
    steps: Vec<StepUi>,
    place: Where,
    folder: Option<PathBuf>,
    suffix: Entity<InputState>,
    stage: Stage,
    results: Vec<FileResult>,
    error: Option<String>,
    /// The step list the results belong to.
    ran: Vec<Step>,
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

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// My Stamps pictures (and drawn signatures for signature steps).
fn my_stamps(signature: bool) -> Vec<(String, PathBuf)> {
    let Some(dir) = crate::sign::stamps_dir() else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    v.sort();
    v.into_iter()
        .filter_map(|p| {
            let name = file_name(&p);
            let is_sig = name.contains(".sig.");
            let ok = name.ends_with(".png")
                || name.ends_with(".jpg")
                || (signature && name.ends_with(".ink"));
            (ok && is_sig == signature).then_some((name, p))
        })
        .collect()
}

fn pages_spec(sel: PagesSel, range: &str) -> String {
    match sel {
        PagesSel::All => "all".into(),
        PagesSel::First => "first".into(),
        PagesSel::Last => "last".into(),
        PagesSel::Range => range.trim().to_owned(),
    }
}

fn pages_sel(spec: &str) -> (PagesSel, String) {
    match spec.trim().to_ascii_lowercase().as_str() {
        "" | "all" => (PagesSel::All, String::new()),
        "first" | "1" => (PagesSel::First, String::new()),
        "last" => (PagesSel::Last, String::new()),
        _ => (PagesSel::Range, spec.trim().to_owned()),
    }
}

/// `a=1; b=2` as values.
fn parse_values(text: &str) -> batch::Fill {
    let mut f = Fill::default();
    for part in text.split([';', '\n']) {
        if let Some((k, v)) = part.split_once('=')
            && !k.trim().is_empty()
        {
            f.values.insert(k.trim().to_owned(), v.trim().to_owned());
        }
    }
    f
}

fn step_title(s: &StepUi) -> String {
    match s {
        StepUi::Stamp {
            signature: false, ..
        } => i18n::t("Place a stamp").into(),
        StepUi::Stamp {
            signature: true, ..
        } => i18n::t("Place a signature picture").into(),
        StepUi::Fill { .. } => i18n::t("Fill in form fields").into(),
        StepUi::PageNumbers { .. } => i18n::t("Add page numbers").into(),
        StepUi::Watermark { .. } => i18n::t("Add a watermark").into(),
        StepUi::Flatten => i18n::t("Flatten annotations").into(),
        StepUi::Other(s) => tf("Step “{}” (from the batch file)", &[&s.kind()]),
    }
}

/// A reason in the user's language.
pub(crate) fn reason_text(r: &Reason) -> String {
    match r {
        Reason::Password => {
            i18n::t("Password-protected: open it in Kantor with its password").into()
        }
        Reason::Signed => {
            i18n::t("Skipped: digitally signed, and this would invalidate the signature").into()
        }
        Reason::NoChangesAllowed => i18n::t("Skipped: the certification allows no changes").into(),
        Reason::FormFillOnly => {
            i18n::t("Skipped: the certification allows only form filling and signing").into()
        }
        Reason::NoForm => i18n::t("Skipped: no form fields to fill").into(),
        Reason::NoRow => i18n::t("Skipped: no row for this file in the table").into(),
        Reason::UnknownFields(v) => tf("Fields not in this file: {}", &[&v.join(", ")]),
        Reason::BadValues(v) => tf("Values not allowed: {}", &[&v.join(", ")]),
        Reason::Pages(e) => tf("Skipped: {}", &[e]),
        Reason::NothingToFlatten => i18n::t("Nothing to flatten").into(),
        Reason::Exists => i18n::t("Skipped: the result file already exists").into(),
        Reason::Plugin(s) => s.clone(),
    }
}

impl BatchState {
    /// Stop a run (after the file it is on).
    pub(crate) fn stop(&self) {
        if let Stage::Running { cancel, .. } = &self.stage {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    /// The batch as the dialog shows it (for running or saving).
    fn to_batch(&self, cx: &App) -> Result<Batch, String> {
        let mut steps = Vec::new();
        for (i, s) in self.steps.iter().enumerate() {
            let n = i + 1;
            steps.push(match s {
                StepUi::Stamp {
                    signature,
                    picture,
                    pages,
                    range,
                    anchor,
                    example,
                    width,
                    opacity,
                } => {
                    let Some((image, _)) = picture else {
                        return Err(tf("Step {}: choose a picture", &[&n]));
                    };
                    let w_text = width.read(cx).value().trim().to_owned();
                    let width = if w_text.is_empty() {
                        0.0
                    } else {
                        match w_text.parse::<f32>() {
                            Ok(w) if (4.0..=2000.0).contains(&w) => w,
                            _ => {
                                return Err(tf(
                                    "Step {}: width: type points from 4 to 2000, or leave it empty",
                                    &[&n],
                                ));
                            }
                        }
                    };
                    let range = range.read(cx).value().to_string();
                    if *pages == PagesSel::Range
                        && kantor_pdf::PageRanges::parse(range.trim()).is_err()
                    {
                        return Err(tf("Step {}: type pages like 1-3, 5", &[&n]));
                    }
                    let st = Stamp {
                        image: image.clone(),
                        pages: pages_spec(*pages, &range),
                        place: match example {
                            Some(e) => Place::Example { example: *e },
                            None => Place::Anchor {
                                anchor: *anchor,
                                dx: 36.0,
                                dy: 36.0,
                            },
                        },
                        width,
                        opacity: f32::from(*opacity) / 100.0,
                    };
                    if *signature {
                        Step::Signature(st)
                    } else {
                        Step::Stamp(st)
                    }
                }
                StepUi::Fill { table, values, .. } => {
                    let mut f = parse_values(&values.read(cx).value());
                    f.data = table.as_ref().map(|t| t.to_string_lossy().into_owned());
                    if f.data.is_none() && f.values.is_empty() {
                        return Err(tf(
                            "Step {}: choose a table of values or type name=value",
                            &[&n],
                        ));
                    }
                    Step::Fill(f)
                }
                StepUi::PageNumbers { format, position } => Step::PageNumbers(PageNumbers {
                    format: NUMBER_FORMATS[*format].0.to_owned(),
                    position: position.name().into(),
                    ..PageNumbers::default()
                }),
                StepUi::Watermark { text, opacity } => {
                    let t = text.read(cx).value().trim().to_owned();
                    if t.is_empty() {
                        return Err(tf("Step {}: type the watermark's text", &[&n]));
                    }
                    if !t.chars().all(|c| (' '..='\u{ff}').contains(&c)) {
                        return Err(i18n::t(
                            "The watermark can use Latin letters, digits and punctuation only",
                        )
                        .into());
                    }
                    Step::Watermark(Watermark {
                        text: t,
                        size: 60.0,
                        opacity: f32::from(*opacity) / 100.0,
                        ..Watermark::default()
                    })
                }
                StepUi::Flatten => Step::Flatten,
                StepUi::Other(s) => s.clone(),
            });
        }
        if steps.is_empty() {
            return Err(i18n::t("Add a step").into());
        }
        let suffix = self.suffix.read(cx).value().trim().to_owned();
        let folder = match self.place {
            Where::Folder => match &self.folder {
                Some(f) => Some(f.clone()),
                None => return Err(i18n::t("Choose a folder for the results").into()),
            },
            Where::NextTo => None,
        };
        if folder.is_none() && suffix.is_empty() {
            return Err(i18n::t(
                "Type a suffix for the results' names, or choose a folder: the originals are never replaced",
            )
            .into());
        }
        Ok(Batch {
            steps,
            output: Output { suffix, folder },
            ..Batch::default()
        })
    }
}

impl PdfView {
    /// Tools ▸ Batch…
    pub fn open_batch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let files = if self.path.as_os_str().is_empty() {
            Vec::new()
        } else {
            vec![self.path.clone()]
        };
        let suffix = input(window, cx, "-batch", "");
        let state = BatchState {
            files,
            steps: Vec::new(),
            place: Where::NextTo,
            folder: None,
            suffix,
            stage: Stage::Edit,
            results: Vec::new(),
            error: None,
            ran: Vec::new(),
        };
        self.dialog = Some(Dialog::Batch(Box::new(state)));
        window.focus(&self.dialog_focus, cx);
        cx.notify();
    }

    fn batch(&mut self) -> Option<&mut BatchState> {
        match self.dialog.as_mut() {
            Some(Dialog::Batch(b)) => Some(b),
            _ => None,
        }
    }

    /// PDFs (and folders of PDFs) dropped while the Batch dialog is open
    /// join its files. True when it took them.
    pub(crate) fn batch_drop(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) -> bool {
        let Some(b) = self.batch() else {
            return false;
        };
        if !matches!(b.stage, Stage::Edit) {
            return true;
        }
        add_paths(b, paths);
        cx.notify();
        true
    }

    fn batch_add_files(&mut self, folders: bool, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: !folders,
            directories: folders,
            multiple: true,
            prompt: Some(if folders {
                i18n::t("Add folders of PDFs").into()
            } else {
                i18n::t("Add PDFs").into()
            }),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(b) = this.batch() {
                    add_paths(b, &paths);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn batch_add_step(&mut self, kind: &str, window: &mut Window, cx: &mut Context<Self>) {
        let step = match kind {
            "stamp" | "signature" => {
                let signature = kind == "signature";
                let picture = my_stamps(signature)
                    .into_iter()
                    .next_back()
                    .map(|(name, _)| (format!("{}{name}", batch::MY_STAMPS), name));
                StepUi::Stamp {
                    signature,
                    picture,
                    pages: if signature {
                        PagesSel::Last
                    } else {
                        PagesSel::All
                    },
                    range: input(window, cx, "", i18n::t("e.g. 1-3, 5")),
                    anchor: Anchor::BottomRight,
                    example: None,
                    width: input(
                        window,
                        cx,
                        if signature { "150" } else { "120" },
                        i18n::t("Auto"),
                    ),
                    opacity: 100,
                }
            }
            "fill" => StepUi::Fill {
                table: None,
                values: input(
                    window,
                    cx,
                    "",
                    i18n::t("e.g. Company=PT Maju; City=Surabaya"),
                ),
                rows: None,
            },
            "page-numbers" => StepUi::PageNumbers {
                format: 2,
                position: Position::BottomCenter,
            },
            "watermark" => StepUi::Watermark {
                text: input(window, cx, "", i18n::t("e.g. DRAFT or CONFIDENTIAL")),
                opacity: 30,
            },
            _ => StepUi::Flatten,
        };
        if let Some(b) = self.batch() {
            b.steps.push(step);
            b.error = None;
        }
        cx.notify();
    }

    /// The stamp selected in the document, as an example place.
    fn selected_example(&self) -> Option<Example> {
        let sel = self.markup.sel.clone()?;
        let a = self.find_mark(&sel)?;
        if !matches!(
            a.kind,
            kantor_pdf::annot::Kind::Stamp { .. } | kantor_pdf::annot::Kind::Ink(_)
        ) {
            return None;
        }
        let p = self.page_ref(sel.place)?;
        let doc = self.doc()?;
        let src = doc.model.source(p.src)?;
        let r = src.rect_to_page(p.page, a.bounds());
        let (pw, ph) = doc.model.unturned_size(p);
        Some(Example {
            x: r[0],
            y: r[1],
            width: r[2] - r[0],
            height: r[3] - r[1],
            page_width: pw,
            page_height: ph,
        })
    }

    fn batch_pick_picture(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(i18n::t("Choose a PNG or JPEG picture").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(p) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(b) = this.batch()
                    && let Some(StepUi::Stamp { picture, .. }) = b.steps.get_mut(i)
                {
                    *picture = Some((p.to_string_lossy().into_owned(), file_name(&p)));
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn batch_pick_table(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(i18n::t("Choose a table of values (CSV, Excel or JSON)").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(p) = paths.into_iter().next() else {
                return;
            };
            let read = p.clone();
            let table = cx
                .background_spawn(async move { kantor_pdf::table::Table::read(&read) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(b) = this.batch() {
                    match table {
                        Ok(t) => {
                            if let Some(StepUi::Fill { table, rows, .. }) = b.steps.get_mut(i) {
                                *table = Some(p);
                                *rows = Some(t.rows.len());
                            }
                            b.error = None;
                        }
                        Err(e) => b.error = Some(tf("Couldn't read {}: {}", &[&file_name(&p), &e])),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn batch_pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(i18n::t("Choose a folder for the results").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(b) = this.batch() {
                    b.folder = paths.into_iter().next();
                    b.place = Where::Folder;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Save the batch as a JSON file (`kantor pdf batch` runs it).
    fn batch_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.batch() else {
            return;
        };
        let batch = match b.to_batch(cx) {
            Ok(x) => x,
            Err(e) => {
                b.error = Some(e);
                cx.notify();
                return;
            }
        };
        let target =
            cx.prompt_for_new_path(&self.dir(), Some(&format!("{}.json", i18n::t("Batch"))));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut path))) = target.await else {
                return;
            };
            if path.extension().is_none() {
                path.set_extension("json");
            }
            let result = std::fs::write(&path, batch.to_json());
            let _ = this.update(cx, |this, cx| {
                this.status = match result {
                    Ok(()) => tf("Saved the batch as {}", &[&file_name(&path)]).into(),
                    Err(e) => tf("Couldn't save the batch: {}", &[&e]).into(),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn batch_load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(i18n::t("Open a saved batch").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(p) = paths.into_iter().next() else {
                return;
            };
            let text = std::fs::read_to_string(&p).map_err(|e| e.to_string());
            let _ = this.update_in(cx, |this, window, cx| {
                let loaded = text.and_then(|t| Batch::from_json(&t).map_err(|e| e.to_string()));
                match loaded {
                    Ok(batch) => this.batch_apply(batch, &p, window, cx),
                    Err(e) => {
                        if let Some(b) = this.batch() {
                            b.error = Some(tf("Couldn't open {}: {}", &[&file_name(&p), &e]));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Show a loaded batch (`file`: where it was, for relative paths).
    fn batch_apply(
        &mut self,
        batch: Batch,
        file: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let base = file.parent().map(Path::to_path_buf).unwrap_or_default();
        let abs = |s: &str| -> String {
            if s.starts_with(batch::MY_STAMPS) || Path::new(s).is_absolute() {
                s.to_owned()
            } else {
                base.join(s).to_string_lossy().into_owned()
            }
        };
        let mut steps = Vec::new();
        for s in batch.steps {
            steps.push(match s {
                Step::Stamp(ref st) | Step::Signature(ref st) => {
                    let signature = matches!(s, Step::Signature(_));
                    let (pages, range) = pages_sel(&st.pages);
                    let image = abs(&st.image);
                    let shown = file_name(Path::new(&image));
                    let (anchor, example) = match st.place {
                        Place::Anchor { anchor, .. } => (anchor, None),
                        Place::Example { example } => (Anchor::BottomRight, Some(example)),
                        Place::Point { .. } => {
                            steps.push(StepUi::Other(s.clone()));
                            continue;
                        }
                    };
                    let width = if st.width > 0.0 {
                        format!("{}", st.width)
                    } else {
                        String::new()
                    };
                    StepUi::Stamp {
                        signature,
                        picture: Some((image, shown)),
                        pages,
                        range: input(window, cx, &range, i18n::t("e.g. 1-3, 5")),
                        anchor,
                        example,
                        width: input(window, cx, &width, i18n::t("Auto")),
                        opacity: (st.opacity * 100.0).round().clamp(5.0, 100.0) as u8,
                    }
                }
                Step::Fill(ref f) => {
                    let values: Vec<String> =
                        f.values.iter().map(|(k, v)| format!("{k}={v}")).collect();
                    if f.key.is_some() {
                        StepUi::Other(s.clone())
                    } else {
                        StepUi::Fill {
                            table: f.data.as_deref().map(|d| PathBuf::from(abs(d))),
                            values: input(window, cx, &values.join("; "), ""),
                            rows: None,
                        }
                    }
                }
                Step::PageNumbers(ref p) => {
                    let format = NUMBER_FORMATS.iter().position(|(f, _)| *f == p.format);
                    let position = p.position.parse::<Position>().ok();
                    match (format, position, pages_sel(&p.pages).0) {
                        (Some(format), Some(position), PagesSel::All) => {
                            StepUi::PageNumbers { format, position }
                        }
                        _ => StepUi::Other(s.clone()),
                    }
                }
                Step::Watermark(ref w) => StepUi::Watermark {
                    text: input(window, cx, &w.text, ""),
                    opacity: (w.opacity * 100.0).round().clamp(0.0, 100.0) as u8,
                },
                Step::Flatten => StepUi::Flatten,
                other => StepUi::Other(other),
            });
        }
        let suffix = batch.output.suffix.clone();
        if let Some(b) = self.batch() {
            b.steps = steps;
            b.place = if batch.output.folder.is_some() {
                Where::Folder
            } else {
                Where::NextTo
            };
            b.folder = batch
                .output
                .folder
                .map(|f| if f.is_absolute() { f } else { base.join(f) });
            b.suffix.update(cx, |i, cx| i.set_value(suffix, window, cx));
            b.error = None;
        }
        self.status = tf("Opened the batch {}", &[&file_name(file)]).into();
    }

    /// Run: ask first when results would replace files.
    fn batch_run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.batch() else {
            return;
        };
        if b.files.is_empty() {
            b.error = Some(i18n::t("Add the PDFs to process").into());
            cx.notify();
            return;
        }
        let batch = match b.to_batch(cx) {
            Ok(x) => x,
            Err(e) => {
                b.error = Some(e);
                cx.notify();
                return;
            }
        };
        let exist = batch::existing_outputs(&batch.output, &b.files);
        if exist.is_empty() {
            self.batch_start(batch, Existing::Skip, window, cx);
            return;
        }
        if window.has_active_dialog(cx) {
            return;
        }
        let view = cx.entity().downgrade();
        let detail: SharedString = if exist.len() == 1 {
            tf("{} already exists.", &[&file_name(&exist[0])]).into()
        } else {
            tf(
                "{} result files already exist, like {}.",
                &[&exist.len(), &file_name(&exist[0])],
            )
            .into()
        };
        let batch = Arc::new(batch);
        window.open_dialog(cx, move |dialog, _, _| {
            let (skip, replace) = (view.clone(), view.clone());
            let (b1, b2) = (batch.clone(), batch.clone());
            dialog
                .w(px(460.))
                .title(i18n::t("Replace the existing files?"))
                .child(v_flex().child(DialogDescription::new().child(detail.clone())))
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("pdf-batch-exist-cancel")
                                .label(i18n::t("Cancel"))
                                .ghost()
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("pdf-batch-exist-skip")
                                .label(i18n::t("Skip Them"))
                                .outline()
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let b = (*b1).clone();
                                    let _ = skip.update(cx, |this, cx| {
                                        this.batch_start(b, Existing::Skip, window, cx)
                                    });
                                }),
                        )
                        .child(
                            Button::new("pdf-batch-exist-replace")
                                .label(i18n::t("Replace"))
                                .primary()
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let b = (*b2).clone();
                                    let _ = replace.update(cx, |this, cx| {
                                        this.batch_start(b, Existing::Replace, window, cx)
                                    });
                                }),
                        ),
                )
        });
    }

    fn batch_start(
        &mut self,
        batch: Batch,
        existing: Existing,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let base = self.dir();
        let prepared = match Prepared::new(
            &batch,
            &base,
            crate::sign::stamps_dir().as_deref(),
            &plugins(),
        ) {
            Ok(p) => p,
            Err(e) => {
                if let Some(b) = self.batch() {
                    b.error = Some(e.to_string());
                }
                cx.notify();
                return;
            }
        };
        let Some(b) = self.batch() else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        b.stage = Stage::Running {
            done: 0,
            cancel: cancel.clone(),
        };
        b.results.clear();
        b.error = None;
        b.ran = batch.steps.clone();
        let files = b.files.clone();
        let output = batch.output.clone();
        let (tx, rx) = async_channel::unbounded::<FileResult>();
        let stop = cancel.clone();
        let work = cx.background_spawn(async move {
            batch::run(&prepared, &files, &output, existing, None, &stop, |_, r| {
                let _ = tx.send_blocking(r.clone());
            });
        });
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(r) = rx.recv().await {
                let ok = this
                    .update(cx, |this, cx| {
                        if let Some(b) = this.batch() {
                            b.results.push(r);
                            if let Stage::Running { done, .. } = &mut b.stage {
                                *done += 1;
                            }
                        }
                        cx.notify();
                    })
                    .is_ok();
                if !ok {
                    break;
                }
            }
            work.await;
            let _ = this.update(cx, |this, cx| {
                let cancelled = cancel.load(Ordering::Relaxed);
                let mut summary = None;
                if let Some(b) = this.batch() {
                    b.stage = Stage::Finished { cancelled };
                    let done = b
                        .results
                        .iter()
                        .filter(|r| matches!(r.status, Status::Done { .. }))
                        .count();
                    summary = Some(tf("Batch: {} of {} files done", &[&done, &b.files.len()]));
                }
                if let Some(s) = summary {
                    this.status = s.into();
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn batch_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.batch().map(|b| &b.stage) {
            Some(Stage::Running { cancel, .. }) => cancel.store(true, Ordering::Relaxed),
            _ => self.close_dialog(window, cx),
        }
        cx.notify();
    }

    /// Back from the results to the steps (to run again).
    fn batch_back(&mut self, cx: &mut Context<Self>) {
        if let Some(b) = self.batch() {
            b.stage = Stage::Edit;
            b.results.clear();
        }
        cx.notify();
    }

    pub(crate) fn batch_ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.batch().map(|b| &b.stage) {
            Some(Stage::Edit) => self.batch_run(window, cx),
            Some(Stage::Finished { .. }) => self.close_dialog(window, cx),
            _ => {}
        }
    }

    // ── drawing ───────────────────────────────────────────────────────────

    pub(crate) fn render_batch(&self, b: &BatchState, cx: &mut Context<Self>) -> Stateful<Div> {
        let view = cx.entity().downgrade();
        let t = tokens(cx);
        let card = card(i18n::t("Batch"), 620.0, cx).max_h(px(700.));
        match &b.stage {
            Stage::Edit => {}
            Stage::Running { done, .. } => {
                let total = b.files.len().max(1);
                let frac = *done as f32 / total as f32;
                return card
                    .child(note(
                        tf(
                            "Processing {} of {} files…",
                            &[&(*done + 1).min(total), &total],
                        ),
                        false,
                        cx,
                    ))
                    .child(
                        div()
                            .h(px(6.))
                            .w_full()
                            .rounded(RADIUS_SM)
                            .bg(t.hover)
                            .child(
                                div()
                                    .h_full()
                                    .rounded(RADIUS_SM)
                                    .bg(t.accent)
                                    .w(relative(frac)),
                            ),
                    )
                    .child(self.batch_results(b, cx))
                    .child(
                        h_flex().justify_end().child(
                            Button::new("pdf-batch-stop")
                                .small()
                                .outline()
                                .label(i18n::t("Cancel"))
                                .on_click({
                                    let v = view.clone();
                                    move |_, w, cx| {
                                        let _ = v.update(cx, |this, cx| this.batch_cancel(w, cx));
                                    }
                                }),
                        ),
                    );
            }
            Stage::Finished { cancelled } => {
                let count =
                    |f: fn(&Status) -> bool| b.results.iter().filter(|r| f(&r.status)).count();
                let done = count(|s| matches!(s, Status::Done { .. }));
                let skipped = count(|s| matches!(s, Status::Skipped));
                let failed = count(|s| matches!(s, Status::Failed(_)));
                let mut summary = tf(
                    "{} done, {} skipped, {} failed",
                    &[&done, &skipped, &failed],
                );
                if *cancelled {
                    summary = format!("{summary} · {}", i18n::t("Cancelled"));
                }
                let (back, close) = (view.clone(), view.clone());
                return card
                    .child(note(summary, false, cx))
                    .child(self.batch_results(b, cx))
                    .child(
                        h_flex()
                            .justify_end()
                            .gap(SPACE_3)
                            .child(
                                Button::new("pdf-batch-back")
                                    .small()
                                    .ghost()
                                    .label(i18n::t("Back to Steps"))
                                    .on_click(move |_, _, cx| {
                                        let _ = back.update(cx, |this, cx| this.batch_back(cx));
                                    }),
                            )
                            .child(
                                Button::new("pdf-batch-close")
                                    .small()
                                    .primary()
                                    .label(i18n::t("Close"))
                                    .on_click(move |_, w, cx| {
                                        let _ =
                                            close.update(cx, |this, cx| this.close_dialog(w, cx));
                                    }),
                            ),
                    );
            }
        }

        // Files.
        let mut files = v_flex().gap(SPACE_1);
        for (i, f) in b.files.iter().enumerate().take(200) {
            let v = view.clone();
            files = files.child(
                h_flex()
                    .gap(SPACE_2)
                    .text_size(FONT_SMALL)
                    .child(div().flex_1().min_w_0().truncate().child(file_name(f)))
                    .child(
                        Button::new(("pdf-batch-file-x", i))
                            .xsmall()
                            .ghost()
                            .icon(Lucide::X)
                            .tooltip(i18n::t("Remove"))
                            .on_click(move |_, _, cx| {
                                let _ = v.update(cx, |this, cx| {
                                    if let Some(b) = this.batch()
                                        && i < b.files.len()
                                    {
                                        b.files.remove(i);
                                    }
                                    cx.notify();
                                });
                            }),
                    ),
            );
        }
        if b.files.len() > 200 {
            files = files.child(note(
                tf("… and {} more", &[&(b.files.len() - 200)]),
                false,
                cx,
            ));
        }
        if b.files.is_empty() {
            files = files.child(note(
                i18n::t("No files yet: add PDFs or a folder, or drop them here."),
                false,
                cx,
            ));
        }
        let (add_files, add_folder) = (view.clone(), view.clone());
        let files_box = v_flex()
            .gap(SPACE_2)
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(tf("Files ({})", &[&b.files.len()])),
                    )
                    .child(
                        Button::new("pdf-batch-add-files")
                            .xsmall()
                            .outline()
                            .icon(Lucide::Plus)
                            .label(i18n::t("Add Files…"))
                            .on_click(move |_, w, cx| {
                                let _ = add_files
                                    .update(cx, |this, cx| this.batch_add_files(false, w, cx));
                            }),
                    )
                    .child(
                        Button::new("pdf-batch-add-folder")
                            .xsmall()
                            .outline()
                            .icon(Lucide::Plus)
                            .label(i18n::t("Add Folder…"))
                            .on_click(move |_, w, cx| {
                                let _ = add_folder
                                    .update(cx, |this, cx| this.batch_add_files(true, w, cx));
                            }),
                    ),
            )
            .child(
                div()
                    .id("pdf-batch-files")
                    .max_h(px(110.))
                    .overflow_y_scroll()
                    .p(SPACE_2)
                    .rounded(RADIUS_SM)
                    .border_1()
                    .border_color(t.border)
                    .child(files),
            );

        // Steps.
        let mut steps = v_flex().gap(SPACE_3);
        let n = b.steps.len();
        for (i, s) in b.steps.iter().enumerate() {
            steps = steps.child(self.batch_step(i, n, s, cx));
        }
        let weak = view.clone();
        let add = Button::new("pdf-batch-add-step")
            .xsmall()
            .outline()
            .icon(Lucide::Plus)
            .label(i18n::t("Add Step"))
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let item = |label: &'static str, kind: &'static str| {
                    let v = weak.clone();
                    PopupMenuItem::new(i18n::t(label)).on_click(move |_, window, cx| {
                        let _ = v.update(cx, |this, cx| this.batch_add_step(kind, window, cx));
                    })
                };
                menu.item(item(tk("Place a stamp"), "stamp"))
                    .item(item(tk("Place a signature picture"), "signature"))
                    .item(item(tk("Fill in form fields"), "fill"))
                    .item(item(tk("Add page numbers"), "page-numbers"))
                    .item(item(tk("Add a watermark"), "watermark"))
                    .item(item(tk("Flatten annotations"), "flatten"))
            });
        let steps_box = v_flex()
            .gap(SPACE_2)
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(i18n::t("Steps, in order")),
                    )
                    .child(add),
            )
            .child(steps)
            .when(n == 0, |c| {
                c.child(note(
                    i18n::t("Add the steps to run on every file."),
                    false,
                    cx,
                ))
            });

        // Output.
        let places = [
            (Where::NextTo, i18n::t("Next to the originals").to_owned()),
            (Where::Folder, i18n::t("In a folder").to_owned()),
        ];
        let pick = view.clone();
        let output_box = v_flex()
            .gap(SPACE_2)
            .child(div().font_weight(FontWeight::SEMIBOLD).child(i18n::t("Results")))
            .child(row(
                i18n::t("Save"),
                h_flex()
                    .gap(SPACE_2)
                    .child(choices("pdf-batch-where", &places, b.place, &view, |d, v| {
                        if let Dialog::Batch(b) = d {
                            b.place = v;
                        }
                    }))
                    .when(b.place == Where::Folder, |r| {
                        r.child(
                            Button::new("pdf-batch-folder")
                                .xsmall()
                                .ghost()
                                .label(match &b.folder {
                                    Some(f) => file_name(f),
                                    None => i18n::t("Choose…").into(),
                                })
                                .on_click(move |_, w, cx| {
                                    let _ = pick.update(cx, |this, cx| this.batch_pick_folder(w, cx));
                                }),
                        )
                    }),
                cx,
            ))
            .child(row(
                i18n::t("Name suffix"),
                Input::new(&b.suffix).small(),
                cx,
            ))
            .child(note(
                i18n::t("Originals are never replaced. Digitally signed files keep their signatures valid: steps that would break them are skipped."),
                false,
                cx,
            ));

        let (load, save, cancel, run) = (view.clone(), view.clone(), view.clone(), view.clone());
        card.child(
            div()
                .id("pdf-batch-body")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(
                    v_flex()
                        .gap(SPACE_5)
                        .pr(SPACE_2)
                        .child(files_box)
                        .child(steps_box)
                        .child(output_box),
                ),
        )
        .when_some(b.error.clone(), |c, e| c.child(note(e, true, cx)))
        .child(
            h_flex()
                .flex_none()
                .gap(SPACE_3)
                .child(
                    Button::new("pdf-batch-load")
                        .small()
                        .ghost()
                        .label(i18n::t("Open Batch…"))
                        .on_click(move |_, w, cx| {
                            let _ = load.update(cx, |this, cx| this.batch_load(w, cx));
                        }),
                )
                .child(
                    Button::new("pdf-batch-save")
                        .small()
                        .ghost()
                        .label(i18n::t("Save Batch…"))
                        .disabled(b.steps.is_empty())
                        .on_click(move |_, w, cx| {
                            let _ = save.update(cx, |this, cx| this.batch_save(w, cx));
                        }),
                )
                .child(div().flex_1())
                .child(
                    Button::new("pdf-batch-cancel")
                        .small()
                        .ghost()
                        .label(i18n::t("Cancel"))
                        .on_click(move |_, w, cx| {
                            let _ = cancel.update(cx, |this, cx| this.close_dialog(w, cx));
                        }),
                )
                .child(
                    Button::new("pdf-batch-run")
                        .small()
                        .primary()
                        .label(i18n::t("Run"))
                        .disabled(b.files.is_empty() || b.steps.is_empty())
                        .on_click(move |_, w, cx| {
                            let _ = run.update(cx, |this, cx| this.batch_run(w, cx));
                        }),
                ),
        )
    }

    fn batch_results(&self, b: &BatchState, cx: &App) -> impl IntoElement {
        let t = tokens(cx);
        let mut list = v_flex().gap(SPACE_2);
        for r in &b.results {
            let (icon, color, status): (Lucide, Hsla, String) = match &r.status {
                Status::Done { incremental } => (
                    Lucide::CircleCheck,
                    t.success,
                    match (&r.output, incremental) {
                        (Some(o), true) => {
                            tf("Saved as {} (signatures kept valid)", &[&file_name(o)])
                        }
                        (Some(o), false) => tf("Saved as {}", &[&file_name(o)]),
                        (None, _) => String::new(),
                    },
                ),
                Status::Skipped => (Lucide::Minus, t.fg_muted, i18n::t("Skipped").into()),
                Status::Failed(e) => (Lucide::TriangleAlert, t.danger, tf("Failed: {}", &[e])),
            };
            let mut lines = v_flex()
                .flex_1()
                .min_w_0()
                .child(div().truncate().child(file_name(&r.input)))
                .child(div().text_color(t.fg_muted).child(status));
            for n in &r.notes {
                let what = match n.step.and_then(|i| b.ran.get(i)) {
                    Some(s) => {
                        format!("{} {}: ", i18n::t("Step"), n.step.unwrap_or(0) + 1)
                            + &step_kind_name(s)
                    }
                    None => String::new(),
                };
                let sep = if what.is_empty() { "" } else { " — " };
                lines = lines.child(
                    div()
                        .text_color(t.fg_muted)
                        .child(format!("{what}{sep}{}", reason_text(&n.reason))),
                );
            }
            list = list.child(
                h_flex()
                    .items_start()
                    .gap(SPACE_2)
                    .text_size(FONT_SMALL)
                    .child(
                        gpui_kit::component::Icon::new(icon)
                            .with_size(ICON_SM)
                            .text_color(color)
                            .flex_none(),
                    )
                    .child(lines),
            );
        }
        div()
            .id("pdf-batch-results")
            .max_h(px(380.))
            .overflow_y_scroll()
            .child(list)
    }

    fn batch_step(&self, i: usize, n: usize, s: &StepUi, cx: &mut Context<Self>) -> Stateful<Div> {
        let view = cx.entity().downgrade();
        let t = tokens(cx);
        let mv = |delta: isize| {
            let v = view.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                let _ = v.update(cx, |this, cx| {
                    if let Some(b) = this.batch() {
                        let j = i as isize + delta;
                        if j >= 0 && (j as usize) < b.steps.len() {
                            b.steps.swap(i, j as usize);
                        }
                    }
                    cx.notify();
                });
            }
        };
        let remove = view.clone();
        let header = h_flex()
            .gap(SPACE_1)
            .child(
                div()
                    .flex_1()
                    .font_weight(FontWeight::MEDIUM)
                    .child(format!("{}. {}", i + 1, step_title(s))),
            )
            .child(
                Button::new(("pdf-batch-up", i))
                    .xsmall()
                    .ghost()
                    .icon(Lucide::ChevronUp)
                    .tooltip(i18n::t("Move Up"))
                    .disabled(i == 0)
                    .on_click(mv(-1)),
            )
            .child(
                Button::new(("pdf-batch-down", i))
                    .xsmall()
                    .ghost()
                    .icon(Lucide::ChevronDown)
                    .tooltip(i18n::t("Move Down"))
                    .disabled(i + 1 == n)
                    .on_click(mv(1)),
            )
            .child(
                Button::new(("pdf-batch-remove", i))
                    .xsmall()
                    .ghost()
                    .icon(Lucide::Trash)
                    .tooltip(i18n::t("Remove Step"))
                    .on_click(move |_, _, cx| {
                        let _ = remove.update(cx, |this, cx| {
                            if let Some(b) = this.batch()
                                && i < b.steps.len()
                            {
                                b.steps.remove(i);
                            }
                            cx.notify();
                        });
                    }),
            );
        let body = match s {
            StepUi::Stamp {
                signature,
                picture,
                pages,
                range,
                anchor,
                example,
                width,
                opacity,
            } => {
                let weak = view.clone();
                let sig = *signature;
                let pic_label = picture
                    .as_ref()
                    .map_or_else(|| i18n::t("Choose…").to_owned(), |p| p.1.clone());
                let pic = Button::new(("pdf-batch-pic", i))
                    .xsmall()
                    .outline()
                    .label(pic_label)
                    .dropdown_caret(true)
                    .dropdown_menu(move |mut menu, _, _| {
                        for (name, _) in my_stamps(sig) {
                            let v = weak.clone();
                            let spec = format!("{}{name}", batch::MY_STAMPS);
                            menu = menu.item(PopupMenuItem::new(name.clone()).on_click(move |_, _, cx| {
                                let (spec, name) = (spec.clone(), name.clone());
                                let _ = v.update(cx, |this, cx| {
                                    if let Some(b) = this.batch()
                                        && let Some(StepUi::Stamp { picture, .. }) = b.steps.get_mut(i)
                                    {
                                        *picture = Some((spec, name));
                                    }
                                    cx.notify();
                                });
                            }));
                        }
                        let v = weak.clone();
                        menu.separator().item(
                            PopupMenuItem::new(i18n::t("Choose a Picture File…")).on_click(move |_, w, cx| {
                                let _ = v.update(cx, |this, cx| this.batch_pick_picture(i, w, cx));
                            }),
                        )
                    });
                let page_opts = [
                    (PagesSel::All, i18n::t("All").to_owned()),
                    (PagesSel::First, i18n::t("First").to_owned()),
                    (PagesSel::Last, i18n::t("Last").to_owned()),
                    (PagesSel::Range, i18n::t("Pages").to_owned()),
                ];
                let set_pages: fn(&mut Dialog, (usize, PagesSel)) = |d, (i, v)| {
                    if let Dialog::Batch(b) = d
                        && let Some(StepUi::Stamp { pages, .. }) = b.steps.get_mut(i)
                    {
                        *pages = v;
                    }
                };
                let page_opts: Vec<((usize, PagesSel), String)> =
                    page_opts.into_iter().map(|(v, l)| ((i, v), l)).collect();
                let pages_row = h_flex()
                    .gap(SPACE_2)
                    .child(choices("pdf-batch-pages", &page_opts, (i, *pages), &view, set_pages))
                    .when(*pages == PagesSel::Range, |r| {
                        r.child(div().w(px(110.)).child(Input::new(range).small()))
                    });
                // The 3 × 3 place grid, or the example's spot.
                let mut grid = v_flex().gap(px(2.));
                for rw in 0..3 {
                    let mut line = h_flex().gap(px(2.));
                    for col in 0..3 {
                        let a = Anchor::ALL[rw * 3 + col];
                        let v = view.clone();
                        let on = example.is_none() && a == *anchor;
                        line = line.child(
                            div()
                                .id(("pdf-batch-anchor", i * 9 + rw * 3 + col))
                                .size(px(14.))
                                .rounded(px(2.))
                                .border_1()
                                .border_color(t.border_strong)
                                .when(on, |d| d.bg(t.accent))
                                .hover(|d| d.bg(t.hover))
                                .cursor_pointer()
                                .on_click(move |_, _, cx| {
                                    let _ = v.update(cx, |this, cx| {
                                        if let Some(b) = this.batch()
                                            && let Some(StepUi::Stamp { anchor, example, .. }) =
                                                b.steps.get_mut(i)
                                        {
                                            *anchor = a;
                                            *example = None;
                                        }
                                        cx.notify();
                                    });
                                }),
                        );
                    }
                    grid = grid.child(line);
                }
                let current = self.selected_example();
                let ex = view.clone();
                let place_row = h_flex()
                    .gap(SPACE_3)
                    .child(grid)
                    .child(
                        Button::new(("pdf-batch-example", i))
                            .xsmall()
                            .when_else(example.is_some(), |b| b.primary(), |b| b.ghost())
                            .label(i18n::t("Same Spot as the Selected Stamp"))
                            .tooltip(i18n::t("Select a stamp or signature placed in this document first"))
                            .disabled(current.is_none() && example.is_none())
                            .on_click(move |_, _, cx| {
                                let _ = ex.update(cx, |this, cx| {
                                    let now = this.selected_example();
                                    if let Some(b) = this.batch()
                                        && let Some(StepUi::Stamp { example, .. }) = b.steps.get_mut(i)
                                        && now.is_some()
                                    {
                                        *example = now;
                                    }
                                    cx.notify();
                                });
                            }),
                    );
                let opac: Vec<((usize, u8), String)> = [100u8, 80, 60, 40]
                    .into_iter()
                    .map(|o| ((i, o), format!("{o}%")))
                    .collect();
                let set_opacity: fn(&mut Dialog, (usize, u8)) = |d, (i, v)| {
                    if let Dialog::Batch(b) = d
                        && let Some(StepUi::Stamp { opacity, .. }) = b.steps.get_mut(i)
                    {
                        *opacity = v;
                    }
                };
                v_flex()
                    .gap(SPACE_2)
                    .child(row(i18n::t("Picture"), pic, cx))
                    .child(row(i18n::t("Pages"), pages_row, cx))
                    .child(row(i18n::t("Position"), place_row, cx))
                    .child(row(
                        i18n::t("Width (pt)"),
                        div().w(px(110.)).child(Input::new(width).small()),
                        cx,
                    ))
                    .child(row(
                        i18n::t("Opacity"),
                        choices("pdf-batch-opacity", &opac, (i, *opacity), &view, set_opacity),
                        cx,
                    ))
            }
            StepUi::Fill {
                table, values, rows, ..
            } => {
                let pick = view.clone();
                let label = match (table, rows) {
                    (Some(t), Some(n)) => tf("{} ({} rows)", &[&file_name(t), n]),
                    (Some(t), None) => file_name(t),
                    (None, _) => i18n::t("Choose…").into(),
                };
                v_flex()
                    .gap(SPACE_2)
                    .child(row(
                        i18n::t("Table"),
                        Button::new(("pdf-batch-table", i))
                            .xsmall()
                            .outline()
                            .label(label)
                            .on_click(move |_, w, cx| {
                                let _ = pick.update(cx, |this, cx| this.batch_pick_table(i, w, cx));
                            }),
                        cx,
                    ))
                    .child(row(i18n::t("For every file"), Input::new(values).small(), cx))
                    .child(note(
                        i18n::t("The table (CSV, Excel or JSON) has a “file” column with each PDF's name and a column per form field, headed with the field's name."),
                        false,
                        cx,
                    ))
            }
            StepUi::PageNumbers { format, position } => {
                let formats: Vec<((usize, usize), String)> = NUMBER_FORMATS
                    .iter()
                    .enumerate()
                    .map(|(k, (_, l))| ((i, k), i18n::t(l).to_owned()))
                    .collect();
                let set_format: fn(&mut Dialog, (usize, usize)) = |d, (i, v)| {
                    if let Dialog::Batch(b) = d
                        && let Some(StepUi::PageNumbers { format, .. }) = b.steps.get_mut(i)
                    {
                        *format = v;
                    }
                };
                let positions: Vec<((usize, Position), String)> = POSITIONS
                    .iter()
                    .map(|(p, l)| ((i, *p), i18n::t(l).to_owned()))
                    .collect();
                let set_position: fn(&mut Dialog, (usize, Position)) = |d, (i, v)| {
                    if let Dialog::Batch(b) = d
                        && let Some(StepUi::PageNumbers { position, .. }) = b.steps.get_mut(i)
                    {
                        *position = v;
                    }
                };
                v_flex()
                    .gap(SPACE_2)
                    .child(row(
                        i18n::t("Format"),
                        choices("pdf-batch-format", &formats, (i, *format), &view, set_format),
                        cx,
                    ))
                    .child(row(
                        i18n::t("Position"),
                        choices("pdf-batch-pos", &positions, (i, *position), &view, set_position),
                        cx,
                    ))
            }
            StepUi::Watermark { text, opacity } => {
                let opac: Vec<((usize, u8), String)> = [15u8, 30, 50]
                    .into_iter()
                    .map(|o| ((i, o), format!("{o}%")))
                    .collect();
                let set_opacity: fn(&mut Dialog, (usize, u8)) = |d, (i, v)| {
                    if let Dialog::Batch(b) = d
                        && let Some(StepUi::Watermark { opacity, .. }) = b.steps.get_mut(i)
                    {
                        *opacity = v;
                    }
                };
                v_flex()
                    .gap(SPACE_2)
                    .child(row(i18n::t("Text"), Input::new(text).small(), cx))
                    .child(row(
                        i18n::t("Opacity"),
                        choices("pdf-batch-wm-opacity", &opac, (i, *opacity), &view, set_opacity),
                        cx,
                    ))
            }
            StepUi::Flatten => v_flex().child(note(
                i18n::t("Comments, stamps, signature pictures and drawings become part of the pages. Skipped on digitally signed files."),
                false,
                cx,
            )),
            StepUi::Other(_) => v_flex().child(note(
                i18n::t("Kept as it is in the batch file."),
                false,
                cx,
            )),
        };
        v_flex()
            .id(("pdf-batch-step", i))
            .gap(SPACE_2)
            .p(SPACE_3)
            .rounded(RADIUS_SM)
            .border_1()
            .border_color(t.border)
            .child(header)
            .child(body)
    }
}

/// The step's name for the results list.
fn step_kind_name(s: &Step) -> String {
    match s {
        Step::Stamp(_) => i18n::t("Stamp").into(),
        Step::Signature(_) => i18n::t("Signature").into(),
        Step::Fill(_) => i18n::t("Fill").into(),
        Step::PageNumbers(_) => i18n::t("Page numbers").into(),
        Step::Watermark(_) => i18n::t("Watermark").into(),
        Step::Flatten => i18n::t("Flatten").into(),
        Step::Plugin { kind, .. } => kind.clone(),
    }
}

/// Add PDFs, and the PDFs in folders, once each.
fn add_paths(b: &mut BatchState, paths: &[PathBuf]) {
    for p in paths {
        let found = if p.is_dir() {
            batch::pdfs_in(p)
        } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
            vec![p.clone()]
        } else {
            Vec::new()
        };
        for f in found {
            if !b.files.contains(&f) {
                b.files.push(f);
            }
        }
    }
    b.error = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn page_choices_round_trip() {
        for (sel, range) in [
            (PagesSel::All, ""),
            (PagesSel::First, ""),
            (PagesSel::Last, ""),
            (PagesSel::Range, "2-4"),
        ] {
            assert_eq!(pages_sel(&pages_spec(sel, range)), (sel, range.to_owned()));
        }
        let f = parse_values("Company = PT Maju; City=Surabaya\nEmpty=");
        assert_eq!(f.values.get("Company").map(String::as_str), Some("PT Maju"));
        assert_eq!(f.values.get("Empty").map(String::as_str), Some(""));
        assert_eq!(f.values.len(), 3);
    }
}
