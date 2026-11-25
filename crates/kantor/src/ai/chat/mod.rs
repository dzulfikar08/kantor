//! Paparan AI's chat panel, docked on the right: the user asks for a deck,
//! a document or a workbook (attaching source files), the agent harness
//! makes it in a work folder off the UI thread, and the panel shows what
//! happens as it happens: the model's text, a row per tool call, the
//! pictures `look` took, and at the end the files with Open and Reveal.
//!
//! - [`model`]: the events reduced to rows (tested without a window);
//! - [`job`]: work folders, attachments, the skill and the run;
//! - [`script`]: a scripted model for showing the panel without a provider.

pub mod job;
pub mod model;
pub mod script;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ai::Cancel;
use kantor_ai::agent::{Limits, StopReason};
use kantor_core::settings::{AiSettings, Settings};
use kantor_ui::components::{IconButtonExt as _, icon_button};
use kantor_ui::i18n::{self, tf};
use kantor_ui::metrics::*;
use kantor_ui::tokens;

use super::paparan::{self, FailAction};
use model::{Chat, Item, JobKind, Nudge, ToolRow};

/// What the panel asks of the workspace.
pub enum ChatEvent {
    /// Open a file the job made in a tab.
    Open(PathBuf),
    /// Hide the panel.
    Close,
    /// Open AI Settings (AI is off, or Paparan's sign-in ended).
    OpenSettings,
}

/// The widest a picture shows in the list: one alone, or several in a row.
const THUMB_ONE: f32 = 280.;
const THUMB_MANY: f32 = 150.;

const EXAMPLES: [&str; 3] = [
    i18n::tk("A 6-slide deck on this quarter's sales for the board"),
    i18n::tk("A one-page memo inviting the team to the planning meeting"),
    i18n::tk("A monthly budget workbook with totals and a chart"),
];

enum Worker {
    Msg(job::Msg),
    Done(Box<job::Done>),
}

pub struct ChatPanel {
    chat: Chat,
    kind: JobKind,
    /// The kind follows the request's words until the user picks one.
    kind_auto: bool,
    input: Entity<TextareaState>,
    /// Files for the next request.
    attachments: Vec<PathBuf>,
    /// The folder the user picked for the job (before its first request).
    folder: Option<PathBuf>,
    /// The job's work folder, once made.
    job_dir: Option<PathBuf>,
    /// The job between requests (taken while one runs).
    session: Option<job::Session>,
    cancel: Option<Cancel>,
    pictures: HashMap<usize, job::Picture>,
    /// A picture shown large.
    enlarged: Option<usize>,
    /// Top up / Sign in, offered with the last failure.
    fail_action: Option<FailAction>,
    ai: AiSettings,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ChatEvent> for ChatPanel {}

impl ChatPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let ai = Settings::load().ai;
        paparan::refresh_if_unknown(&ai, cx);
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 10)
                .submit_on_enter(true)
                .placeholder(i18n::t("Ask for a deck, a document or a workbook…"))
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &input,
                window,
                |this, _, ev: &InputEvent, window, cx| match ev {
                    InputEvent::PressEnter { shift: false, .. } => this.send(window, cx),
                    InputEvent::Change => this.guess_kind(cx),
                    _ => {}
                },
            ),
            cx.observe_global::<paparan::PaparanState>(|_, cx| cx.notify()),
            // AI Settings changed (switched on, another provider).
            cx.observe_global::<kantor_ui::ai::AiEnabled>(|this, cx| {
                this.ai = Settings::load().ai;
                cx.notify();
            }),
        ];
        Self {
            chat: Chat::default(),
            kind: JobKind::Deck,
            kind_auto: true,
            input,
            attachments: Vec::new(),
            folder: None,
            job_dir: None,
            session: None,
            cancel: None,
            pictures: HashMap::new(),
            enlarged: None,
            fail_action: None,
            ai,
            scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    pub fn focus_input(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| s.focus(window, cx));
    }

    /// The scripted model for showing the panel without a provider.
    fn script() -> Option<PathBuf> {
        std::env::var_os(script::SCRIPT_ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    }

    fn usable(&self) -> bool {
        super::client_allowed(&self.ai) || Self::script().is_some()
    }

    fn guess_kind(&mut self, cx: &mut Context<Self>) {
        if !self.kind_auto || self.chat.started() {
            return;
        }
        let text = self.input.read(cx).value();
        let files: Vec<&Path> = self.attachments.iter().map(PathBuf::as_path).collect();
        let kind = JobKind::guess(&text, &files);
        if kind != self.kind {
            self.kind = kind;
            cx.notify();
        }
    }

    fn at_bottom(&self) -> bool {
        let max = self.scroll.max_offset().y;
        max - (-self.scroll.offset().y) < px(48.)
    }

    /// Send what's typed: a new job, or a follow-up of the one in the chat.
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().trim().to_owned();
        if text.is_empty() || self.chat.running {
            return;
        }
        self.ai = Settings::load().ai;
        if !self.usable() {
            cx.emit(ChatEvent::OpenSettings);
            return;
        }
        let attachments = std::mem::take(&mut self.attachments);
        let names = attachments
            .iter()
            .filter_map(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .collect();
        self.chat.ask(&text, names);
        self.fail_action = None;
        self.input.update(cx, |s, cx| {
            s.set_value(String::new(), window, cx);
            s.set_placeholder(i18n::t("Ask for changes…"), window, cx);
        });
        let first = self.session.is_none();
        let req = job::Request {
            prompt: text,
            kind: self.kind,
            folder: if first { self.folder.clone() } else { None },
            attachments,
            ai: self.ai.clone(),
            skills: job::local_skills(job::skills_dir(&self.ai)),
            limits: Limits::default(),
            session: self.session.take(),
            script: Self::script(),
        };
        let cancel = Cancel::default();
        self.cancel = Some(cancel.clone());
        let (tx, rx) = async_channel::unbounded::<Worker>();
        let spawned = std::thread::Builder::new()
            .name("kantor-paparan-job".into())
            .spawn(move || {
                let send = |m: job::Msg| {
                    let _ = tx.send_blocking(Worker::Msg(m));
                };
                let done = job::work(req, &cancel, &send);
                let _ = tx.send_blocking(Worker::Done(Box::new(done)));
            });
        if let Err(e) = spawned {
            self.cancel = None;
            self.chat.set_failure(e.to_string());
            self.chat.ended();
            cx.notify();
            return;
        }
        cx.spawn(async move |this, cx| {
            while let Ok(first) = rx.recv().await {
                // Everything waiting goes in one redraw (streamed text comes
                // a word at a time).
                let mut batch = vec![first];
                while let Ok(m) = rx.try_recv() {
                    batch.push(m);
                }
                if this.update(cx, |p, cx| p.take(batch, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn take(&mut self, batch: Vec<Worker>, cx: &mut Context<Self>) {
        let follow = self.at_bottom();
        for w in batch {
            match w {
                Worker::Msg(job::Msg::Event(e)) => self.chat.apply(&e),
                Worker::Msg(job::Msg::Pictures(pics)) => {
                    let labels = pics.iter().map(|p| p.label.clone()).collect();
                    let ids = self.chat.add_pictures(labels);
                    self.pictures.extend(ids.into_iter().zip(pics));
                }
                Worker::Msg(job::Msg::Ready { root, files }) => {
                    self.job_dir = Some(root);
                    // The names the files got in the folder.
                    if let Some(Item::User { files: shown, .. }) = self
                        .chat
                        .items
                        .iter_mut()
                        .rev()
                        .find(|i| matches!(i, Item::User { .. }))
                    {
                        *shown = files;
                    }
                }
                Worker::Done(done) => {
                    let done = *done;
                    self.cancel = None;
                    self.session = done.session;
                    if let Some(e) = &done.error {
                        let (message, action) = paparan::failure(e, cx);
                        if matches!(
                            e,
                            kantor_ai::AiError::SignedOut
                                | kantor_ai::AiError::InsufficientCredits { .. }
                                | kantor_ai::AiError::RateLimited { .. }
                        ) {
                            self.chat.set_failure(message);
                        }
                        self.fail_action = action;
                    }
                    self.chat.ended();
                    paparan::after_call(done.error.as_ref(), cx);
                }
            }
        }
        if follow {
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = &self.cancel {
            c.cancel();
        }
        cx.notify();
    }

    /// Start over: a new job (the running one stops).
    fn new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(c) = self.cancel.take() {
            c.cancel();
        }
        self.chat = Chat::default();
        self.session = None;
        self.job_dir = None;
        self.folder = None;
        self.attachments.clear();
        self.pictures.clear();
        self.enlarged = None;
        self.fail_action = None;
        self.kind_auto = true;
        self.kind = JobKind::Deck;
        self.input.update(cx, |s, cx| {
            s.set_value(String::new(), window, cx);
            s.set_placeholder(
                i18n::t("Ask for a deck, a document or a workbook…"),
                window,
                cx,
            );
        });
        self.focus_input(window, cx);
        cx.notify();
    }

    fn attach(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for p in paths {
            if !self.attachments.contains(&p) {
                self.attachments.push(p);
            }
        }
        self.guess_kind(cx);
        cx.notify();
    }

    fn pick_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(i18n::t("Attach files for Paparan AI").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| this.attach(paths, cx));
        })
        .detach();
    }

    fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dirs = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(i18n::t("Choose the job's work folder").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(dirs))) = dirs.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.folder = dirs.into_iter().next();
                cx.notify();
            });
        })
        .detach();
    }

    /// A file the job made, as a path.
    fn output_path(&self, name: &str) -> Option<PathBuf> {
        let p = Path::new(name);
        if p.is_absolute() {
            return Some(p.to_path_buf());
        }
        Some(self.job_dir.as_ref()?.join(p))
    }

    // ── rendering ──────────────────────────────────────────────────────

    fn render_header(&self, cx: &mut Context<Self>) -> Div {
        let t = tokens(cx);
        let credits = if paparan::chosen(&self.ai) {
            paparan::state(cx).me.map(|m| {
                div()
                    .text_size(FONT_CAPTION)
                    .text_color(if m.credits.low { t.warning } else { t.fg_dim })
                    .child(tf("{} left", &[&m.credits.display()]))
            })
        } else {
            None
        };
        h_flex()
            .flex_none()
            .h(TITLE_BAR_HEIGHT)
            .px(SPACE_5)
            .gap(SPACE_3)
            .border_b_1()
            .border_color(t.border)
            .child(
                Icon::new(IconName::Sparkles)
                    .with_size(ICON_MD)
                    .text_color(t.accent),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(FONT_SMALL)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.fg)
                    .child("Paparan AI"),
            )
            .children(credits)
            .child(
                icon_button("chat-new", IconName::MessageSquarePlus, false, cx)
                    .tip(i18n::t("New chat"), None)
                    .on_click(cx.listener(|this, _, window, cx| this.new_chat(window, cx))),
            )
            .child(
                icon_button("chat-hide", IconName::PanelRightClose, false, cx)
                    .tip(i18n::t("Hide Paparan AI"), None)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ChatEvent::Close))),
            )
    }

    /// AI is off or no provider is chosen.
    fn render_off(&self, cx: &mut Context<Self>) -> Div {
        let t = tokens(cx);
        v_flex()
            .flex_1()
            .p(SPACE_7)
            .gap(SPACE_5)
            .justify_center()
            .items_center()
            .child(
                div()
                    .text_size(FONT_BODY)
                    .text_color(t.fg_muted)
                    .text_center()
                    .child(i18n::t(
                        "Paparan AI makes decks, documents and workbooks from your request. Turn on AI and choose a provider first.",
                    )),
            )
            .child(
                Button::new("chat-settings")
                    .primary()
                    .small()
                    .label(i18n::t("AI Settings…"))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ChatEvent::OpenSettings))),
            )
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> Div {
        let t = tokens(cx);
        let examples = EXAMPLES.iter().enumerate().map(|(i, ex)| {
            let text = i18n::t_dyn(ex).to_owned();
            div()
                .id(("chat-example", i))
                .px(SPACE_5)
                .py(SPACE_4)
                .rounded(RADIUS_LG)
                .border_1()
                .border_color(t.border)
                .text_size(FONT_SMALL)
                .text_color(t.fg_muted)
                .cursor_pointer()
                .hover(|s| s.bg(t.hover).text_color(t.fg))
                .child(text.clone())
                .on_click(cx.listener(move |this, _, window, cx| {
                    let text = text.clone();
                    this.input.update(cx, |s, cx| s.set_value(text, window, cx));
                    this.guess_kind(cx);
                    this.focus_input(window, cx);
                }))
        });
        v_flex()
            .p(SPACE_7)
            .gap(SPACE_4)
            .child(
                div()
                    .text_size(FONT_EMPHASIS)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.fg)
                    .child(i18n::t("What should Paparan AI make?")),
            )
            .child(
                div()
                    .text_size(FONT_SMALL)
                    .text_color(t.fg_dim)
                    .child(i18n::t(
                        "Describe it in English or Indonesian, and drop or attach the source files. The work happens in its own folder; your files are never changed.",
                    )),
            )
            .children(examples)
    }

    fn render_item(&self, ix: usize, item: &Item, cx: &mut Context<Self>) -> AnyElement {
        let t = tokens(cx);
        match item {
            Item::User { text, files } => {
                let chips = files.iter().map(|f| file_chip(f, &t));
                h_flex()
                    .w_full()
                    .justify_end()
                    .child(
                        v_flex()
                            .max_w(relative(0.88))
                            .gap(SPACE_3)
                            .px(SPACE_5)
                            .py(SPACE_4)
                            .rounded(RADIUS_LG)
                            .bg(t.selected)
                            .text_size(FONT_BODY)
                            .text_color(t.fg)
                            .child(text.clone())
                            .when(!files.is_empty(), |d| {
                                d.child(h_flex().flex_wrap().gap(SPACE_2).children(chips))
                            }),
                    )
                    .into_any_element()
            }
            Item::Assistant { text, .. } => div()
                .w_full()
                .text_size(FONT_BODY)
                .text_color(t.fg)
                .child(TextView::markdown(("chat-text", ix), text.clone()).selectable(true))
                .into_any_element(),
            Item::Tool(row) => self.render_tool(ix, row, cx).into_any_element(),
            Item::Nudge(n) => {
                let text = match n {
                    Nudge::Announced => i18n::t("Reminded to do the work it announced"),
                    Nudge::Empty => i18n::t("Reminded to keep going"),
                    Nudge::NoFinish => i18n::t("Reminded to finish with a summary"),
                };
                quiet_line(IconName::RotateCcw, text.to_owned(), t.fg_dim).into_any_element()
            }
            Item::Usage {
                total,
                steps,
                cost,
                estimated,
            } => {
                let mut parts = vec![
                    tf("{} steps", &[steps]),
                    tf(
                        "{} tokens in ({} cached), {} out",
                        &[
                            &count(total.input),
                            &count(total.cached),
                            &count(total.output),
                        ],
                    ),
                ];
                if let Some(c) = cost {
                    parts.push(format!("${c:.4}"));
                }
                if paparan::chosen(&self.ai)
                    && let Some(me) = paparan::state(cx).me
                {
                    parts.push(tf("{} left", &[&me.credits.display()]));
                }
                if *estimated {
                    parts.push(i18n::t("partly estimated").to_owned());
                }
                quiet_line(IconName::Info, parts.join(" · "), t.fg_dim).into_any_element()
            }
            Item::Finished { summary, outputs } => {
                let cards = outputs
                    .iter()
                    .enumerate()
                    .map(|(k, name)| self.render_output(ix, k, name, cx));
                v_flex()
                    .w_full()
                    .gap(SPACE_4)
                    .child(div().text_size(FONT_BODY).text_color(t.fg).child(
                        TextView::markdown(("chat-summary", ix), summary.clone()).selectable(true),
                    ))
                    .children(cards)
                    .into_any_element()
            }
            Item::Stopped { reason, message } => {
                let text = match reason {
                    StopReason::Cancelled => i18n::t("Stopped.").to_owned(),
                    StopReason::Steps => tf("Stopped at the step limit: {}", &[message]),
                    StopReason::Budget => tf("Stopped by the budget: {}", &[message]),
                    StopReason::NoFinish => tf(
                        "Stopped: the model kept ending without finishing. {}",
                        &[message],
                    ),
                };
                quiet_line(IconName::CircleAlert, text, t.warning).into_any_element()
            }
            Item::Failed { message } => {
                let last = ix + 1 == self.chat.items.len();
                let entity = cx.entity().downgrade();
                let action = self.fail_action.as_ref().filter(|_| last).map(|a| {
                    paparan::action_button(a, move |cx| {
                        let _ = entity.update(cx, |_, cx| cx.emit(ChatEvent::OpenSettings));
                    })
                });
                v_flex()
                    .gap(SPACE_3)
                    .child(quiet_line(IconName::CircleX, message.clone(), t.danger))
                    .children(action.map(|b| h_flex().pl(SPACE_7).child(b)))
                    .into_any_element()
            }
        }
    }

    fn render_tool(&self, ix: usize, row: &ToolRow, cx: &mut Context<Self>) -> Div {
        let t = tokens(cx);
        let mono = cx.theme().mono_font_family.clone();
        let (status, color) = match &row.result {
            None => (IconName::LoaderCircle, t.fg_dim),
            Some(r) if r.ok => (IconName::Check, t.success),
            Some(_) => (IconName::CircleX, t.danger),
        };
        let has_result = row.result.as_ref().is_some_and(|r| !r.text.is_empty());
        let header = h_flex()
            .id(("chat-tool", ix))
            .w_full()
            .h(px(24.))
            .px(SPACE_3)
            .gap(SPACE_3)
            .rounded(RADIUS_MD)
            .when(has_result, |d| d.cursor_pointer().hover(|s| s.bg(t.hover)))
            .child(
                Icon::new(if row.expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .with_size(ICON_SM)
                .text_color(if has_result { t.fg_dim } else { t.border }),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(FONT_SMALL)
                    .font_family(mono.clone())
                    .text_color(t.fg_muted)
                    .child(row.name.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(FONT_SMALL)
                    .text_color(t.fg_dim)
                    .child(row.args.clone()),
            )
            .child(Icon::new(status).with_size(ICON_SM).text_color(color))
            .when(has_result, |d| {
                d.on_click(cx.listener(move |this, _, _, cx| {
                    this.chat.toggle(ix);
                    cx.notify();
                }))
            });
        let result = row.result.as_ref().filter(|_| row.expanded).map(|r| {
            div()
                .id(("chat-tool-result", ix))
                .ml(SPACE_7)
                .max_h(px(220.))
                .overflow_y_scroll()
                .p(SPACE_4)
                .rounded(RADIUS_MD)
                .bg(t.surface)
                .border_1()
                .border_color(t.border)
                .font_family(mono.clone())
                .text_size(FONT_CAPTION)
                .text_color(if r.ok { t.fg_muted } else { t.danger })
                .child(r.text.clone())
        });
        let pictures = row.pictures.iter().filter_map(|(id, label)| {
            let pic = self.pictures.get(id)?;
            let size = pic.thumb.size(0);
            let (w, h) = (size.width.0.max(1) as f32, size.height.0.max(1) as f32);
            let max = if row.pictures.len() == 1 {
                THUMB_ONE
            } else {
                THUMB_MANY
            };
            let shown_w = max.min(w / 2.);
            let id = *id;
            Some(
                div()
                    .id(("chat-picture", id))
                    .flex_none()
                    .rounded(RADIUS_MD)
                    .overflow_hidden()
                    .border_1()
                    .border_color(t.border)
                    .cursor_pointer()
                    .hover(|s| s.border_color(t.accent))
                    .tooltip({
                        let label = label.clone();
                        move |window, cx| Tooltip::new(label.clone()).build(window, cx)
                    })
                    .child(
                        img(ImageSource::Render(pic.thumb.clone()))
                            .w(px(shown_w))
                            .h(px(h * shown_w / w)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.enlarged = Some(id);
                        cx.notify();
                    })),
            )
        });
        v_flex()
            .w_full()
            .gap(SPACE_2)
            .child(header)
            .children(result)
            .when(!row.pictures.is_empty(), |d| {
                d.child(
                    h_flex()
                        .flex_wrap()
                        .gap(SPACE_3)
                        .pl(SPACE_7)
                        .py(SPACE_2)
                        .children(pictures),
                )
            })
    }

    fn render_output(&self, ix: usize, k: usize, name: &str, cx: &mut Context<Self>) -> Div {
        let t = tokens(cx);
        let path = self.output_path(name);
        let ext = Path::new(name)
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let (icon, color) = match ext.as_str() {
            "pptx" => (IconName::Presentation, t.slides),
            "docx" => (IconName::FileText, t.docs),
            "xlsx" | "xlsm" | "csv" => (IconName::FileSpreadsheet, t.sheets),
            _ => (IconName::File, t.fg_muted),
        };
        let file_name = Path::new(name)
            .file_name()
            .map_or_else(|| name.to_owned(), |n| n.to_string_lossy().into_owned());
        let open = path.clone().map(|p| {
            Button::new(("chat-open", ix * 100 + k))
                .outline()
                .small()
                .label(i18n::t("Open"))
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(ChatEvent::Open(p.clone()))))
        });
        let reveal = path.map(|p| {
            Button::new(("chat-reveal", ix * 100 + k))
                .ghost()
                .small()
                .label(i18n::t("Reveal"))
                .on_click(move |_, _, cx| cx.reveal_path(&p))
        });
        h_flex()
            .w_full()
            .gap(SPACE_4)
            .px(SPACE_5)
            .py(SPACE_4)
            .rounded(RADIUS_LG)
            .border_1()
            .border_color(t.border)
            .bg(t.surface)
            .child(Icon::new(icon).with_size(ICON_LG).text_color(color))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(FONT_BODY)
                    .text_color(t.fg)
                    .child(file_name),
            )
            .children(open)
            .children(reveal)
    }

    fn render_composer(&self, cx: &mut Context<Self>) -> Div {
        let t = tokens(cx);
        let running = self.chat.running;
        let started = self.chat.started();
        let chips = self.attachments.iter().enumerate().map(|(i, p)| {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            h_flex()
                .gap(SPACE_2)
                .pl(SPACE_4)
                .pr(SPACE_2)
                .h(CONTROL_SM)
                .rounded(RADIUS_MD)
                .bg(t.surface)
                .border_1()
                .border_color(t.border)
                .text_size(FONT_SMALL)
                .text_color(t.fg_muted)
                .child(
                    Icon::new(IconName::Paperclip)
                        .with_size(ICON_SM)
                        .text_color(t.fg_dim),
                )
                .child(div().max_w(px(180.)).truncate().child(name))
                .child(
                    icon_button(("chat-unattach", i), IconName::X, false, cx)
                        .tip(i18n::t("Remove"), None)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if i < this.attachments.len() {
                                this.attachments.remove(i);
                            }
                            this.guess_kind(cx);
                            cx.notify();
                        })),
                )
        });
        let kinds = JobKind::ALL.into_iter().enumerate().map(|(i, kind)| {
            let selected = kind == self.kind;
            let label = match kind {
                JobKind::Deck => i18n::t("Deck"),
                JobKind::Document => i18n::t("Document"),
                JobKind::Workbook => i18n::t("Workbook"),
            };
            div()
                .id(("chat-kind", i))
                .px(SPACE_4)
                .h(CONTROL_SM)
                .flex()
                .items_center()
                .rounded(RADIUS_MD)
                .text_size(FONT_SMALL)
                .when_else(
                    selected,
                    |d| d.bg(t.selected).text_color(t.fg),
                    |d| d.text_color(t.fg_dim),
                )
                .when(!started, |d| {
                    d.cursor_pointer()
                        .when(!selected, |d| d.hover(|s| s.bg(t.hover).text_color(t.fg)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.kind = kind;
                            this.kind_auto = false;
                            cx.notify();
                        }))
                })
                .child(label)
        });
        let action = if running {
            Button::new("chat-stop")
                .outline()
                .small()
                .icon(IconName::Square)
                .tooltip(i18n::t("Stop"))
                .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
        } else {
            Button::new("chat-send")
                .primary()
                .small()
                .icon(IconName::ArrowUp)
                .tooltip(i18n::t("Send (Enter)"))
                .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
        };
        let folder_tip = match &self.folder {
            Some(f) => tf("Work folder: {}", &[&f.display()]),
            None => i18n::t("Choose the work folder (default: a new folder in Kantor's data)")
                .to_owned(),
        };
        let where_ = match (&self.job_dir, &self.folder) {
            (Some(d), _) | (None, Some(d)) => d
                .file_name()
                .map(|n| tf("Folder: {}", &[&n.to_string_lossy()]))
                .unwrap_or_default(),
            (None, None) => String::new(),
        };
        let provider = if Self::script().is_some() {
            i18n::t("Scripted model").to_owned()
        } else {
            super::provider_label(&self.ai).to_owned()
        };
        v_flex()
            .flex_none()
            .p(SPACE_5)
            .gap(SPACE_4)
            .border_t_1()
            .border_color(t.border)
            .when(!self.attachments.is_empty(), |d| {
                d.child(h_flex().flex_wrap().gap(SPACE_3).children(chips))
            })
            .child(Textarea::new(&self.input))
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .children(kinds)
                    .child(div().flex_1())
                    .when(!started, |d| {
                        d.child(
                            icon_button(
                                "chat-folder",
                                IconName::FolderOpen,
                                self.folder.is_some(),
                                cx,
                            )
                            .tip(&folder_tip, None)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_folder(window, cx)),
                            ),
                        )
                    })
                    .child(
                        icon_button("chat-attach", IconName::Paperclip, false, cx)
                            .tip(i18n::t("Attach files"), None)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_files(window, cx)),
                            ),
                    )
                    .child(action),
            )
            .child(
                h_flex()
                    .gap(SPACE_3)
                    .text_size(FONT_CAPTION)
                    .text_color(t.fg_dim)
                    .child(div().flex_none().child(provider))
                    .child(div().flex_1().min_w_0().truncate().child(where_)),
            )
    }

    fn render_enlarged(&self, cx: &mut Context<Self>) -> Option<Div> {
        let id = self.enlarged?;
        let pic = self.pictures.get(&id)?;
        let t = tokens(cx);
        Some(
            div()
                .absolute()
                .inset_0()
                .p(SPACE_5)
                .bg(t.scrim)
                .occlude()
                .flex()
                .flex_col()
                .gap(SPACE_3)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.enlarged = None;
                        cx.notify();
                    }),
                )
                .child(
                    div().flex_1().min_h_0().child(
                        img(ImageSource::Render(pic.full.clone()))
                            .size_full()
                            .object_fit(ObjectFit::Contain),
                    ),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(FONT_SMALL)
                        .text_color(t.fg)
                        .text_center()
                        .child(pic.label.clone()),
                ),
        )
    }
}

impl Render for ChatPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens(cx);
        let (accent, soft) = (t.accent, t.accent_soft);
        let body = if !self.usable() {
            self.render_off(cx).into_any_element()
        } else {
            let items: Vec<AnyElement> = self
                .chat
                .items
                .iter()
                .enumerate()
                .map(|(ix, item)| self.render_item(ix, item, cx))
                .collect();
            let working = self.chat.running.then(|| {
                let text = if self.cancel.as_ref().is_some_and(Cancel::is_cancelled) {
                    i18n::t("Stopping after this step…").to_owned()
                } else if self.chat.step == 0 {
                    i18n::t("Starting…").to_owned()
                } else {
                    tf("Working… step {}", &[&self.chat.step])
                };
                quiet_line(IconName::LoaderCircle, text, t.fg_dim)
            });
            let empty = (self.chat.items.is_empty()).then(|| self.render_empty(cx));
            v_flex()
                .flex_1()
                .min_h_0()
                .child(
                    v_flex()
                        .id("chat-messages")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .child(
                            v_flex()
                                .p(SPACE_6)
                                .gap(SPACE_5)
                                .children(empty)
                                .children(items)
                                .children(working),
                        ),
                )
                .child(self.render_composer(cx))
                .into_any_element()
        };
        v_flex()
            .id("chat-panel")
            .relative()
            .size_full()
            .bg(t.chrome)
            .border_l_1()
            .border_color(t.border)
            .text_color(t.fg)
            .drag_over::<ExternalPaths>(move |s, _, _, _| {
                s.bg(soft.opacity(0.35)).border_2().border_color(accent)
            })
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.attach(paths.paths().to_vec(), cx);
            }))
            .child(self.render_header(cx))
            .child(body)
            .children(self.render_enlarged(cx))
    }
}

/// A small, dim line with an icon: nudges, usage, stops.
fn quiet_line(icon: IconName, text: String, color: Hsla) -> Div {
    h_flex()
        .w_full()
        .gap(SPACE_3)
        .px(SPACE_3)
        .text_size(FONT_SMALL)
        .text_color(color)
        .child(Icon::new(icon).with_size(ICON_SM).flex_none())
        .child(div().flex_1().min_w_0().child(text))
}

fn file_chip(name: &str, t: &kantor_ui::Tokens) -> Div {
    h_flex()
        .gap(SPACE_2)
        .px(SPACE_3)
        .rounded(RADIUS_SM)
        .bg(t.surface)
        .text_size(FONT_CAPTION)
        .text_color(t.fg_muted)
        .child(Icon::new(IconName::Paperclip).with_size(ICON_SM))
        .child(name.to_owned())
}

/// A token count with thousands marks for the interface language.
fn count(n: u64) -> String {
    let digits = n.to_string();
    let mark = match i18n::lang() {
        i18n::Lang::Id => '.',
        i18n::Lang::En => ',',
    };
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(mark);
        }
        out.push(c);
    }
    out
}
