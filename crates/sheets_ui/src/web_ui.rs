//! IMAGE and WEBSERVICE in the app: when formulas may download, the bar
//! that asks first for workbooks opened from a file, and the downloads.
//!
//! Following Excel 365, whose default for linked data and IMAGE pictures is
//! "Prompt user about Linked Data Types" (a security bar; `#BLOCKED!` until
//! the content is enabled):
//!
//! - The setting "Let formulas download from the internet" (`[sheets]
//!   web_formulas`, on by default) turns downloading on or off everywhere.
//!   Off, the formulas give `#BLOCKED!`; IMAGE cells keep the pictures saved
//!   in the file and WEBSERVICE cells their saved values.
//! - A workbook opened from a file whose formulas use IMAGE or WEBSERVICE
//!   starts blocked, with a bar: *This workbook downloads pictures or data
//!   from the internet. [Allow] [Block]*. The answer holds for that file
//!   until Kantor quits (reopening it doesn't ask again). New workbooks and
//!   formulas typed into a workbook that didn't ask download right away.
//! - Downloads run on the background executor ([`web::fetch_cached`]: 30 s
//!   timeout, size caps, the session cache), the cells show `#BUSY!`
//!   meanwhile, and the workbook is recalculated when answers arrive. F9
//!   fetches WEBSERVICE answers (and failed downloads) again; pictures stay
//!   cached for the session.

use std::collections::HashSet;
use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_core::settings::Settings;
use kantor_ui::i18n;
use kantor_ui::{metrics::*, tokens};
use sheets_engine::web::{self, WebKind, WebResult};

use std::sync::Arc;

use crate::SheetsView;
use crate::calc::Job;

/// The setting and the answers given this session, for every pane.
struct WebPolicy {
    on: bool,
    allowed: HashSet<PathBuf>,
    blocked: HashSet<PathBuf>,
    /// Changes when the setting changes (panes apply it at their next
    /// render).
    generation: u64,
}
impl Global for WebPolicy {}

fn policy(cx: &mut App) -> &mut WebPolicy {
    if !cx.has_global::<WebPolicy>() {
        cx.set_global(WebPolicy {
            on: Settings::load().sheets.web_formulas,
            allowed: HashSet::new(),
            blocked: HashSet::new(),
            generation: 0,
        });
    }
    cx.global_mut::<WebPolicy>()
}

/// "Let formulas download from the internet (IMAGE, WEBSERVICE)" is on.
pub fn web_formulas_enabled(cx: &mut App) -> bool {
    policy(cx).on
}

/// Turn downloading by formulas on or off in every workbook, and remember
/// it. Returns the new state.
pub fn toggle_web_formulas(cx: &mut App) -> bool {
    let p = policy(cx);
    p.on = !p.on;
    p.generation += 1;
    let on = p.on;
    cx.background_spawn(async move {
        let mut s = Settings::load();
        if s.sheets.web_formulas != on {
            s.sheets.web_formulas = on;
            if let Err(err) = s.save() {
                log::error!("saving settings: {err:#}");
            }
        }
    })
    .detach();
    cx.refresh_windows();
    on
}

/// What a workbook may do, as decided when it was opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Trust {
    /// Downloads when the setting allows (new workbooks, files without web
    /// formulas, files the user allowed).
    #[default]
    Allowed,
    /// The bar is up: blocked until the user answers.
    Asking,
    Blocked,
}

/// A pane's downloads.
#[derive(Default)]
pub(crate) struct WebState {
    pub(crate) trust: Trust,
    /// The policy generation applied last (`None`: never).
    generation: Option<u64>,
    /// URLs being downloaded.
    in_flight: HashSet<(String, WebKind)>,
    /// Answers waiting for the workbook (it was busy).
    inbox: Vec<(String, WebKind, WebResult)>,
    /// Recalculate when the workbook is free (answers were set).
    recalc_due: bool,
}

impl SheetsView {
    /// After a workbook was opened from `path`: ask before downloading if
    /// its formulas would (see the module docs).
    pub(crate) fn web_after_open(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        let has_web = self.table.read(cx).delegate().book.has_web_formulas();
        let p = policy(cx);
        let trust = match path {
            _ if !has_web => Trust::Allowed,
            Some(path) if p.allowed.contains(&path) => Trust::Allowed,
            Some(path) if p.blocked.contains(&path) => Trust::Blocked,
            _ => Trust::Asking,
        };
        self.web = WebState {
            trust,
            // A file allowed earlier this session: recalculate it when the
            // policy is applied, so its formulas download.
            recalc_due: has_web && trust == Trust::Allowed,
            ..WebState::default()
        };
    }

    /// Allow or block the downloads of the workbook the bar asked about.
    fn answer_web_bar(&mut self, allow: bool, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.path(cx);
        let p = policy(cx);
        if let Some(path) = path {
            if allow {
                p.blocked.remove(&path);
                p.allowed.insert(path);
            } else {
                p.allowed.remove(&path);
                p.blocked.insert(path);
            }
        }
        self.web.trust = if allow {
            Trust::Allowed
        } else {
            Trust::Blocked
        };
        // Applied (and, when allowed, recalculated) below.
        self.web.generation = None;
        self.web.recalc_due = allow;
        self.sync_web(window, cx);
        cx.notify();
    }

    /// On every render: if there is download work ([`Self::sync_web`]),
    /// do it right after this frame.
    pub(crate) fn sync_web_soon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() || self.opening.is_some() {
            return;
        }
        let (on, generation) = {
            let p = policy(cx);
            (p.on, p.generation)
        };
        let work = self.web.generation != Some(generation)
            || !self.web.inbox.is_empty()
            || self.web.recalc_due
            || (on && self.web.trust == Trust::Allowed && {
                let book = &self.table.read(cx).delegate().book;
                book.web_requests()
                    .iter()
                    .any(|r| !self.web.in_flight.contains(r))
            });
        if work {
            cx.defer_in(window, |this, window, cx| this.sync_web(window, cx));
        }
    }

    /// Bring the workbook in line with the setting and this pane's trust,
    /// apply downloaded answers and start the downloads formulas wait for.
    pub(crate) fn sync_web(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() || self.opening.is_some() {
            return;
        }
        let (on, generation) = {
            let p = policy(cx);
            (p.on, p.generation)
        };
        let enabled = on && self.web.trust == Trust::Allowed;
        if self.web.generation != Some(generation) {
            let changed = self.table.update(cx, |table, _| {
                let book = &mut table.delegate_mut().book;
                let was = book.web_enabled();
                book.set_web_enabled(enabled);
                was != enabled && book.has_web_formulas()
            });
            self.web.generation = Some(generation);
            // Turned on: recalculate so formulas download; turned off:
            // so they show #BLOCKED! (or what the file saved).
            self.web.recalc_due |= changed;
        }
        if !self.web.inbox.is_empty() {
            let answers = std::mem::take(&mut self.web.inbox);
            self.table.update(cx, |table, _| {
                let book = &mut table.delegate_mut().book;
                for (url, kind, result) in answers {
                    book.set_web_result(&url, kind, result);
                }
            });
            self.web.recalc_due = true;
        }
        let answered = enabled
            && self
                .table
                .update(cx, |table, _| table.delegate_mut().book.answer_from_cache());
        if answered {
            self.web.recalc_due = true;
        }
        if self.web.recalc_due {
            self.web.recalc_due = false;
            // Manual calculation still shows downloads as they arrive (as
            // Excel does), so this recalculates in either mode.
            self.run_book_job(
                Job::Calculating,
                |book| book.recalculate(),
                |_, (), _, _, _| {},
                window,
                cx,
            );
            return;
        }
        if !enabled {
            return;
        }
        let requests = self.table.read(cx).delegate().book.web_requests();
        for request in requests {
            if !self.web.in_flight.insert(request.clone()) {
                continue;
            }
            let (url, kind) = request;
            let fetch = cx.background_spawn({
                let url = url.clone();
                async move { web::fetch_cached(&url, kind) }
            });
            cx.spawn_in(window, async move |this, cx| {
                let result = fetch.await;
                let _ = this.update_in(cx, |this, window, cx| {
                    this.web.in_flight.remove(&(url.clone(), kind));
                    this.web.inbox.push((url, kind, result));
                    this.sync_web(window, cx);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// F9: WEBSERVICE answers and failed downloads are fetched again.
    pub(crate) fn refresh_web(&mut self, cx: &mut Context<Self>) {
        web::forget_text();
        self.table
            .update(cx, |table, _| table.delegate_mut().book.refresh_web());
    }

    /// The bar asking whether this workbook may download.
    pub(crate) fn render_web_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.web.trust != Trust::Asking || !web_formulas_enabled(cx) {
            return None;
        }
        let t = tokens(cx);
        Some(
            h_flex()
                .id("sheets-web-bar")
                .flex_none()
                .h(CONTROL_LG + SPACE_2)
                .gap(SPACE_4)
                .px(SPACE_4)
                .bg(t.warning.opacity(0.14))
                .border_b_1()
                .border_color(t.border)
                .text_size(FONT_SMALL)
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.fg)
                        .child(i18n::t("Security Warning")),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(t.fg_muted)
                        .child(i18n::t(
                            "This workbook downloads pictures or data from the internet.",
                        )),
                )
                .child(
                    Button::new("sheets-web-allow")
                        .label(i18n::t("Allow"))
                        .small()
                        .on_click(
                            cx.listener(|this, _, window, cx| {
                                this.answer_web_bar(true, window, cx)
                            }),
                        ),
                )
                .child(
                    Button::new("sheets-web-block")
                        .label(i18n::t("Block"))
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.answer_web_bar(false, window, cx)
                        })),
                )
                .into_any_element(),
        )
    }
}

impl crate::SheetGrid {
    /// An IMAGE cell's picture over its `(x, y, w, h)` block (the merged
    /// cell's, `x`/`y` being this slice's offset in it), placed by Excel's
    /// sizing mode (`sheets_scene::cell_image_rect`) and cut to the block.
    pub(crate) fn picture_layer(
        &mut self,
        image: &sheets_engine::web::CellImage,
        (x, y, w, h): (Pixels, Pixels, Pixels, Pixels),
    ) -> Div {
        // Placed at 100 % (a picture at its own size grows with the zoom).
        let z = self.zoom();
        let [ix, iy, iw, ih] =
            sheets_scene::cell_image_rect(image, f32::from(w) / z, f32::from(h) / z).map(|v| v * z);

        let key = Arc::as_ptr(&image.data) as usize;
        let source = match self.pictures.get(&key) {
            Some((_, source)) => source.clone(),
            None => {
                let format = match sheets_engine::web::picture_format(&image.data) {
                    Some("png") => ImageFormat::Png,
                    Some("jpeg") => ImageFormat::Jpeg,
                    Some("gif") => ImageFormat::Gif,
                    Some("bmp") => ImageFormat::Bmp,
                    Some("tiff") => ImageFormat::Tiff,
                    Some("webp") => ImageFormat::Webp,
                    Some("ico") => ImageFormat::Ico,
                    _ => return div(),
                };
                if self.pictures.len() > 256 {
                    self.pictures.clear();
                }
                let source = Arc::new(Image::from_bytes(format, image.data.as_ref().clone()));
                self.pictures
                    .insert(key, (image.data.clone(), source.clone()));
                source
            }
        };
        div()
            .absolute()
            .left(-x)
            .top(-y)
            .w(w)
            .h(h)
            .overflow_hidden()
            .child(
                img(source)
                    .absolute()
                    .left(px(ix))
                    .top(px(iy))
                    .w(px(iw))
                    .h(px(ih))
                    .object_fit(ObjectFit::Fill),
            )
    }
}
