//! Running a workbook's macros (after Enable Content): Run in the Macros
//! dialog, buttons and shapes with an assigned macro, and the dialogs a
//! macro shows (`MsgBox`, `InputBox`, the run-time error).
//!
//! **Where a macro runs.** The interpreter (`kantor_script`) is built on
//! `Rc` and isn't `Send`, and it recurses on the native stack, so a run
//! can't be cut into slices on the UI thread. It runs on a worker thread
//! of its own instead, with the workbook (which is `Send`) moved there,
//! as a big recalculation does (`calc`): the project is compiled there
//! from the module sources. Meanwhile the grid holds a stand-in; a run
//! that ends within [`QUICK`] never shows it (the UI thread waits that
//! long), a longer one shows the busy pane with a Stop button (and Esc),
//! which sets the interpreter's cancel flag (checked every 1024
//! statements). A dialog hands the workbook back to the UI thread while
//! it shows, so the grid shows the cells as the macro left them; the
//! answer takes it back to the worker. The whole run is one undo step.
//!
//! `Debug.Print` lines go to the Macros dialog's output list.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{self, Input, InputEvent, InputState};
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_script::{CancelToken, ScriptError};
use kantor_ui::i18n::{t, tf, tk};
use kantor_ui::tokens::Elevation;
use kantor_ui::{metrics::*, tokens};
use sheets_engine::Workbook;
use sheets_script::{
    InputBoxRequest, MacroError, MacroProject, MacroUi, MsgBoxRequest, MsgIcon, RunOptions,
    RunOutcome, Selection, View,
};

use crate::SheetsView;
use crate::calc::{Busy, Job};
use crate::grid_ops::Span;
use crate::vba_ui::MacroTrust;

/// How long the UI thread waits for a run before showing the busy pane.
const QUICK: Duration = Duration::from_millis(150);

/// A message box's button labels, as `sheets_script` names them.
const BUTTONS: [&str; 7] = [
    tk("OK"),
    tk("Cancel"),
    tk("Abort"),
    tk("Retry"),
    tk("Ignore"),
    tk("Yes"),
    tk("No"),
];

/// A button label in the interface language.
fn button_label(en: &str) -> &'static str {
    BUTTONS.iter().copied().find(|b| *b == en).map_or("OK", t)
}

/// `Debug.Print` lines kept.
const OUTPUT_LINES: usize = 500;

/// What the worker asks of the UI thread.
enum ToUi {
    /// Show a dialog; the workbook is lent until the answer.
    Ask {
        book: Workbook,
        view: View,
        ask: Ask,
        reply: mpsc::Sender<(Workbook, Answer)>,
    },
    Print(String),
    Done {
        book: Workbook,
        outcome: RunOutcome,
    },
}

enum Ask {
    Msg(MsgBoxRequest),
    Input(InputBoxRequest),
}

enum Answer {
    Msg(Result<i32, ScriptError>),
    Input(Result<Option<String>, ScriptError>),
}

/// A run in progress.
pub(crate) struct MacroRun {
    pub(crate) name: String,
    cancel: CancelToken,
    rx: mpsc::Receiver<ToUi>,
    /// The dialog showing (the workbook is in the grid meanwhile).
    dialog: Option<RunDialog>,
}

impl Drop for MacroRun {
    fn drop(&mut self) {
        // The pane closed: stop the worker (a dialog it waits for answers
        // as cancelled when its reply channel goes).
        self.cancel.cancel();
    }
}

struct RunDialog {
    ask: Ask,
    reply: mpsc::Sender<(Workbook, Answer)>,
    /// The input box's field.
    input: Option<(Entity<InputState>, Subscription)>,
    focus: FocusHandle,
    /// The button that Enter presses (message boxes).
    choice: usize,
}

/// The worker's side of the UI: requests over a channel, waiting for the
/// answers.
struct ChannelUi {
    tx: mpsc::Sender<ToUi>,
}

impl ChannelUi {
    fn ask(&mut self, book: Workbook, view: View, ask: Ask) -> (Workbook, Option<Answer>) {
        let (reply, answer) = mpsc::channel();
        if let Err(mpsc::SendError(ToUi::Ask { book, .. })) = self.tx.send(ToUi::Ask {
            book,
            view,
            ask,
            reply,
        }) {
            return (book, None);
        }
        match answer.recv() {
            Ok((book, a)) => (book, Some(a)),
            // The pane went away with the workbook: the run stops.
            Err(_) => (
                Workbook::new_empty().unwrap_or_else(|_| unreachable_book()),
                None,
            ),
        }
    }
}

fn unreachable_book() -> Workbook {
    panic!("no workbook to hand back to a stopped macro")
}

fn stopped() -> ScriptError {
    ScriptError::interrupted(18, "User interrupt occurred")
}

impl MacroUi for ChannelUi {
    fn msg_box(
        &mut self,
        book: Workbook,
        view: View,
        req: &MsgBoxRequest,
    ) -> (Workbook, Result<i32, ScriptError>) {
        match self.ask(book, view, Ask::Msg(req.clone())) {
            (book, Some(Answer::Msg(a))) => (book, a),
            (book, _) => (book, Err(stopped())),
        }
    }

    fn input_box(
        &mut self,
        book: Workbook,
        view: View,
        req: &InputBoxRequest,
    ) -> (Workbook, Result<Option<String>, ScriptError>) {
        match self.ask(book, view, Ask::Input(req.clone())) {
            (book, Some(Answer::Input(a))) => (book, a),
            (book, _) => (book, Err(stopped())),
        }
    }

    fn debug_print(&mut self, line: &str) {
        let _ = self.tx.send(ToUi::Print(line.to_owned()));
    }

    /// A file outside the workbook's folder: the user decides (Yes/No).
    fn allow_file(
        &mut self,
        book: Workbook,
        view: View,
        path: &std::path::Path,
        write: bool,
    ) -> (Workbook, bool) {
        let verb = if write { "save" } else { "read" };
        let req = MsgBoxRequest {
            prompt: format!(
                "This macro wants to {verb} a file outside the workbook's folder:\n\n{}\n\nAllow it?",
                path.display()
            ),
            // vbYesNo + vbExclamation + vbDefaultButton2
            buttons: 4 + 48 + 256,
            title: "Kantor Sheets".to_owned(),
        };
        let (book, answer) = self.msg_box(book, view, &req);
        // vbYes
        (book, matches!(answer, Ok(6)))
    }
}

impl SheetsView {
    /// The grid's sheet and selection, for a run to start from.
    fn macro_view(&self, cx: &App) -> View {
        let grid = self.table.read(cx).delegate();
        let range = grid.range();
        View {
            sheet: grid.sheet,
            selection: Selection {
                range,
                active: self.selected,
            },
        }
    }

    /// Show `view` (where a macro left the grid).
    fn apply_macro_view(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.table.read(cx).delegate().book.sheet_names().len() as u32;
        if view.sheet < count && view.sheet != self.table.read(cx).delegate().sheet {
            self.show_sheet(view.sheet, cx);
        }
        let Selection { range, active } = view.selection;
        let end = (
            if active.0 == range.top {
                range.bottom
            } else {
                range.top
            },
            if active.1 == range.left {
                range.right
            } else {
                range.left
            },
        );
        self.select_range(active, end, Span::Cells, true, cx);
        self.load_selected_into_formula(window, cx);
    }

    /// Run macro `name` (`Module1.Macro1`) on the workbook.
    pub(crate) fn run_macro(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.macros.trust != MacroTrust::Enabled {
            self.macros_disabled_notice(cx);
            return;
        }
        if self.busy.is_some() || self.macros.run.is_some() {
            self.status = t("Please wait until the workbook is ready").into();
            cx.notify();
            return;
        }
        if self.editing.is_some() {
            self.commit_edit(None, window, cx);
        }
        let Some(macros) = self.macros.project.clone() else {
            return;
        };
        let Ok(stand_in) = Workbook::new_empty() else {
            return;
        };
        let view = self.macro_view(cx);
        let book = self.table.update(cx, |table, _| {
            std::mem::replace(&mut table.delegate_mut().book, stand_in)
        });
        let (tx, rx) = mpsc::channel();
        let cancel = CancelToken::new();
        let options = RunOptions {
            step_limit: None,
            cancel: cancel.clone(),
        };
        let worker_name = name.clone();
        let spawned = std::thread::Builder::new()
            .name("kantor-macro".into())
            // The interpreter recurses on the native stack.
            .stack_size(32 << 20)
            .spawn(move || run_on_worker(book, macros, worker_name, view, options, tx));
        if let Err(e) = spawned {
            log::error!("starting a macro: {e}");
            self.status = t("The macro couldn't start").into();
            cx.notify();
            return;
        }
        self.macros.last_run = Some(name.clone());
        self.macros.run = Some(MacroRun {
            name,
            cancel,
            rx,
            dialog: None,
        });
        // A quick run ends before the busy pane would show.
        let first = self
            .macros
            .run
            .as_ref()
            .and_then(|r| r.rx.recv_timeout(QUICK).ok());
        if let Some(msg) = first {
            self.macro_message(msg, window, cx);
        }
        self.poll_macro(window, cx);
    }

    /// Handle what the worker sent; keep waiting while it runs.
    fn poll_macro(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        loop {
            let Some(run) = self.macros.run.as_ref() else {
                self.busy = None;
                return;
            };
            if run.dialog.is_some() {
                self.busy = None;
                return;
            }
            match run.rx.try_recv() {
                Ok(msg) => self.macro_message(msg, window, cx),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    // The worker ended without handing the workbook back
                    // (it panicked): the stand-in stays, nothing to save.
                    self.macros.run = None;
                    self.busy = None;
                    self.status = t("The macro stopped unexpectedly").into();
                    cx.notify();
                    return;
                }
            }
        }
        if self.busy.is_none() {
            self.show_macro_busy(window, cx);
        }
    }

    /// The busy pane while a run goes on, with a task watching the worker.
    fn show_macro_busy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = self.macros.run.as_ref() else {
            return;
        };
        let name = run.name.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(15))
                    .await;
                let go_on = this.update_in(cx, |this, window, cx| {
                    this.poll_macro(window, cx);
                    this.macros.run.as_ref().is_some_and(|r| r.dialog.is_none())
                });
                if !matches!(go_on, Ok(true)) {
                    break;
                }
            }
        });
        let had_focus = self
            .table
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx);
        self.busy = Some(Busy {
            what: Job::Macro(name),
            started: Instant::now(),
            name: self.doc_name(cx),
            path: self.path(cx),
            dirty: true,
            _task: task,
        });
        if had_focus || !self.busy_focus.contains_focused(window, cx) {
            window.focus(&self.busy_focus, cx);
        }
        cx.notify();
    }

    /// Stop the run (Stop on the busy pane, Esc).
    pub(crate) fn stop_macro(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = &self.macros.run {
            run.cancel.cancel();
            self.status = t("Stopping the macro…").into();
            cx.notify();
        }
    }

    fn macro_message(&mut self, msg: ToUi, window: &mut Window, cx: &mut Context<Self>) {
        match msg {
            ToUi::Print(line) => {
                let out = &mut self.macros.output;
                out.push(line.into());
                if out.len() > OUTPUT_LINES {
                    out.drain(..out.len() - OUTPUT_LINES);
                }
            }
            ToUi::Ask {
                book,
                view,
                ask,
                reply,
            } => {
                self.busy = None;
                self.put_back(book, cx);
                self.apply_macro_view(view, window, cx);
                let focus = cx.focus_handle();
                let input = match &ask {
                    Ask::Input(req) => {
                        let state = cx.new(|cx| InputState::new(window, cx));
                        let default = req.default.clone();
                        state.update(cx, |i, cx| {
                            i.set_value(default, window, cx);
                            i.focus(window, cx);
                        });
                        let sub = cx.subscribe_in(&state, window, Self::on_macro_input);
                        Some((state, sub))
                    }
                    Ask::Msg(_) => {
                        window.focus(&focus, cx);
                        None
                    }
                };
                let choice = match &ask {
                    Ask::Msg(req) => req.default_button().min(req.choices().len() - 1),
                    Ask::Input(_) => 0,
                };
                if let Some(run) = self.macros.run.as_mut() {
                    run.dialog = Some(RunDialog {
                        ask,
                        reply,
                        input,
                        focus,
                        choice,
                    });
                }
                cx.notify();
            }
            ToUi::Done { book, outcome } => {
                self.macros.run = None;
                self.busy = None;
                self.put_back(book, cx);
                self.apply_macro_view(outcome.view, window, cx);
                self.focus_grid(window, cx);
                match outcome.result {
                    Ok(()) => {
                        self.status = tf("Ran {}", &[&self.macro_title()]).into();
                        if outcome.save_requested {
                            self.save_doc(window, cx);
                        }
                    }
                    Err(e) => {
                        self.status = format!("{e}").into();
                        let focus = cx.focus_handle();
                        window.focus(&focus, cx);
                        self.macros.error_focus = Some(focus);
                        self.macros.error = Some(e);
                    }
                }
                self.update_title(window, cx);
                cx.notify();
            }
        }
    }

    fn macro_title(&self) -> String {
        self.macros.last_run.clone().unwrap_or_default()
    }

    /// The workbook back in the grid.
    fn put_back(&mut self, book: Workbook, cx: &mut Context<Self>) {
        self.drawings.borrow_mut().clear();
        self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            grid.book = book;
            grid.resize();
            table.refresh(cx);
            cx.notify();
        });
    }

    /// Answer the dialog showing: the workbook goes back to the worker.
    fn answer_macro(&mut self, answer: Answer, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.macros.run.as_mut().and_then(|r| r.dialog.take()) else {
            return;
        };
        let Ok(stand_in) = Workbook::new_empty() else {
            return;
        };
        let book = self.table.update(cx, |table, _| {
            std::mem::replace(&mut table.delegate_mut().book, stand_in)
        });
        if let Err(mpsc::SendError((book, _))) = dialog.reply.send((book, answer)) {
            // The worker is gone: keep the workbook.
            self.put_back(book, cx);
            self.macros.run = None;
            return;
        }
        self.poll_macro(window, cx);
    }

    fn press_msg(&mut self, ix: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.macros.run.as_ref().and_then(|r| r.dialog.as_ref()) else {
            return;
        };
        let Ask::Msg(req) = &d.ask else {
            return;
        };
        let choices = req.choices();
        let ix = ix.unwrap_or(d.choice).min(choices.len() - 1);
        let value = choices[ix].1;
        self.answer_macro(Answer::Msg(Ok(value)), window, cx);
    }

    /// Esc on a message box: Cancel if it has one (else nothing, as in
    /// Excel, except OK alone).
    fn escape_msg(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.macros.run.as_ref().and_then(|r| r.dialog.as_ref()) else {
            return;
        };
        let Ask::Msg(req) = &d.ask else {
            self.answer_macro(Answer::Input(Ok(None)), window, cx);
            return;
        };
        let choices = req.choices();
        if let Some(ix) = choices.iter().position(|c| c.1 == 2) {
            self.press_msg(Some(ix), window, cx);
        } else if choices.len() == 1 {
            self.press_msg(Some(0), window, cx);
        }
    }

    fn on_macro_input(
        &mut self,
        _input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let InputEvent::PressEnter { .. } = event {
            self.submit_input(window, cx);
        }
    }

    fn submit_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self
            .macros
            .run
            .as_ref()
            .and_then(|r| r.dialog.as_ref())
            .and_then(|d| d.input.as_ref())
            .map(|(i, _)| i.read(cx).value().to_string());
        if let Some(text) = text {
            self.answer_macro(Answer::Input(Ok(Some(text))), window, cx);
        }
    }

    /// A button or shape with a macro was clicked.
    pub(crate) fn drawing_macro_clicked(
        &mut self,
        key: sheets_engine::drawing::DrawingKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let assigned = self
            .table
            .read(cx)
            .delegate()
            .book
            .drawing_macro_now(key)
            .and_then(|m| sheets_script::assigned_macro(&m));
        let Some(name) = assigned else {
            return;
        };
        if self.macros.trust != MacroTrust::Enabled {
            self.macros_disabled_notice(cx);
            return;
        }
        let known = self.macros.project.as_ref().map(MacroProject::macros);
        let wanted = name.to_ascii_lowercase();
        let found = known.and_then(|list| {
            list.into_iter().find(|m| {
                m.name.to_ascii_lowercase() == wanted
                    || m.full_name().to_ascii_lowercase() == wanted
            })
        });
        match found {
            Some(m) => self.run_macro(m.full_name(), window, cx),
            None => {
                self.status = tf("Cannot run the macro '{}'", &[&name]).into();
                cx.notify();
            }
        }
    }

    /// Clicking a macro button with content disabled: the bar says why.
    pub(crate) fn macros_disabled_notice(&mut self, cx: &mut Context<Self>) {
        self.macros.bar_closed = false;
        self.status = t("Macros are disabled. Enable Content to run them.").into();
        cx.notify();
    }

    /// The dialog a running macro shows, or its run-time error.
    pub(crate) fn render_macro_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(e) = &self.macros.error {
            return Some(self.render_macro_error(e, cx));
        }
        let d = self.macros.run.as_ref()?.dialog.as_ref()?;
        let t_ = tokens(cx);
        let (title, prompt, icon) = match &d.ask {
            Ask::Msg(req) => (req.title.clone(), req.prompt.clone(), req.icon()),
            Ask::Input(req) => (req.title.clone(), req.prompt.clone(), MsgIcon::None),
        };
        let icon = match icon {
            MsgIcon::None => None,
            MsgIcon::Critical => Some((Lucide::CircleX, t_.danger)),
            MsgIcon::Question => Some((Lucide::CircleQuestionMark, t_.info)),
            MsgIcon::Exclamation => Some((Lucide::TriangleAlert, t_.warning)),
            MsgIcon::Information => Some((Lucide::Info, t_.info)),
        };
        let buttons: Vec<AnyElement> = match &d.ask {
            Ask::Msg(req) => req
                .choices()
                .iter()
                .enumerate()
                .map(|(ix, &(label, _))| {
                    dialog_button(
                        ("macro-msg-button", ix),
                        button_label(label),
                        ix == d.choice,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, w, cx| this.press_msg(Some(ix), w, cx)))
                    .into_any_element()
                })
                .collect(),
            Ask::Input(_) => vec![
                dialog_button(("macro-input-button", 0usize), t("Cancel"), false, cx)
                    .on_click(cx.listener(|this, _, w, cx| {
                        this.answer_macro(Answer::Input(Ok(None)), w, cx)
                    }))
                    .into_any_element(),
                dialog_button(("macro-input-button", 1usize), t("OK"), true, cx)
                    .on_click(cx.listener(|this, _, w, cx| this.submit_input(w, cx)))
                    .into_any_element(),
            ],
        };
        let text = v_flex()
            .flex_1()
            .min_w_0()
            .gap(SPACE_3)
            .child(
                div()
                    .text_size(FONT_EMPHASIS)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t_.fg)
                    .child(SharedString::from(title)),
            )
            .child(
                div()
                    .text_color(t_.fg)
                    .whitespace_normal()
                    .child(SharedString::from(prompt)),
            )
            .children(
                d.input
                    .as_ref()
                    .map(|(i, _)| div().pt(SPACE_2).child(Input::new(i).small())),
            );
        let card = v_flex()
            .id("macro-dialog")
            .track_focus(&d.focus)
            .key_context("KantorMacroDialog")
            .on_action(cx.listener(|this, _: &input::Escape, w, cx| this.escape_msg(w, cx)))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                let is_msg = matches!(
                    this.macros
                        .run
                        .as_ref()
                        .and_then(|r| r.dialog.as_ref())
                        .map(|d| &d.ask),
                    Some(Ask::Msg(_))
                );
                match e.keystroke.key.as_str() {
                    "escape" => {
                        cx.stop_propagation();
                        this.escape_msg(window, cx);
                    }
                    "enter" | "space" if is_msg => {
                        cx.stop_propagation();
                        this.press_msg(None, window, cx);
                    }
                    "tab" | "right" | "left" if is_msg => {
                        cx.stop_propagation();
                        let back = e.keystroke.key == "left" || e.keystroke.modifiers.shift;
                        if let Some(d) = this.macros.run.as_mut().and_then(|r| r.dialog.as_mut())
                            && let Ask::Msg(req) = &d.ask
                        {
                            let n = req.choices().len();
                            d.choice = if back {
                                (d.choice + n - 1) % n
                            } else {
                                (d.choice + 1) % n
                            };
                        }
                        cx.notify();
                    }
                    _ => {}
                }
            }))
            .w(px(420.))
            .p(SPACE_7)
            .gap(SPACE_7)
            .bg(t_.elevated)
            .border_1()
            .border_color(t_.border)
            .rounded(RADIUS_LG)
            .shadow(t_.shadow(Elevation::Modal))
            .text_size(FONT_SMALL)
            .child(
                h_flex()
                    .items_start()
                    .gap(SPACE_6)
                    .children(icon.map(|(icon, color)| {
                        div()
                            .flex_none()
                            .text_color(color)
                            .child(Icon::new(icon).with_size(px(22.)))
                    }))
                    .child(text),
            )
            .child(h_flex().justify_end().gap(SPACE_4).children(buttons));
        Some(scrim("macro-dialog-scrim", card, cx))
    }

    fn render_macro_error(&self, e: &MacroError, cx: &mut Context<Self>) -> AnyElement {
        let t_ = tokens(cx);
        let heading = match e.kind {
            kantor_script::ErrorKind::Syntax => t("Compile error:").to_owned(),
            kantor_script::ErrorKind::Interrupted if e.number == 18 => {
                t("Code execution has been interrupted").to_owned()
            }
            _ => tf("Run-time error '{}':", &[&e.number]),
        };
        let place = match (e.module.is_empty(), e.line) {
            (true, _) => String::new(),
            (false, 0) => tf("in {}", &[&error_proc(e)]),
            (false, line) => tf("in {}, line {}", &[&error_proc(e), &line]),
        };
        let card = v_flex()
            .id("macro-error")
            .when_some(self.macros.error_focus.as_ref(), |d, f| d.track_focus(f))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                if matches!(e.keystroke.key.as_str(), "escape" | "enter" | "space") {
                    cx.stop_propagation();
                    this.close_macro_error(window, cx);
                }
            }))
            .w(px(460.))
            .p(SPACE_7)
            .gap(SPACE_6)
            .bg(t_.elevated)
            .border_1()
            .border_color(t_.border)
            .rounded(RADIUS_LG)
            .shadow(t_.shadow(Elevation::Modal))
            .text_size(FONT_SMALL)
            .child(
                div()
                    .text_size(FONT_EMPHASIS)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t("Macro Error")),
            )
            .child(
                h_flex()
                    .items_start()
                    .gap(SPACE_6)
                    .child(
                        div()
                            .flex_none()
                            .text_color(t_.danger)
                            .child(Icon::new(Lucide::CircleX).with_size(px(22.))),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(SPACE_3)
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(t_.fg)
                                    .child(SharedString::from(heading)),
                            )
                            .child(
                                div()
                                    .text_color(t_.fg)
                                    .whitespace_normal()
                                    .child(SharedString::from(e.description.clone())),
                            )
                            .when(!place.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_color(t_.fg_muted)
                                        .child(SharedString::from(place)),
                                )
                            }),
                    ),
            )
            .child(h_flex().justify_end().gap(SPACE_4).child(
                dialog_button("macro-error-end", t("End"), true, cx).on_click(
                    cx.listener(|this, _, window, cx| this.close_macro_error(window, cx)),
                ),
            ));
        scrim("macro-error-scrim", card, cx)
    }

    fn close_macro_error(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.macros.error.take().is_some() {
            self.focus_grid(window, cx);
            cx.notify();
        }
    }

    /// The busy pane's Stop button and Esc.
    pub(crate) fn macro_busy_keys(
        &self,
        pane: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        pane.on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
            if e.keystroke.key == "escape" {
                cx.stop_propagation();
                this.stop_macro(cx);
            }
        }))
    }
}

fn error_proc(e: &MacroError) -> String {
    if e.procedure.is_empty() {
        e.module.clone()
    } else {
        format!("{}.{}", e.module, e.procedure)
    }
}

fn dialog_button(id: impl Into<ElementId>, label: &str, on: bool, cx: &App) -> Stateful<Div> {
    let t_ = tokens(cx);
    div()
        .id(id)
        .h(CONTROL_MD)
        .min_w(px(72.))
        .px(SPACE_6)
        .flex()
        .items_center()
        .justify_center()
        .rounded(RADIUS_MD)
        .text_size(FONT_SMALL)
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .map(|d| {
            if on {
                d.bg(t_.accent).text_color(t_.accent_fg)
            } else {
                d.bg(t_.surface)
                    .border_1()
                    .border_color(t_.border_strong)
                    .text_color(t_.fg)
                    .hover(|s| s.bg(t_.hover))
            }
        })
        .child(SharedString::from(label.to_owned()))
}

fn scrim(id: &'static str, card: Stateful<Div>, cx: &App) -> AnyElement {
    let t_ = tokens(cx);
    div()
        .id(id)
        .absolute()
        .inset_0()
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .bg(t_.scrim)
        .child(card)
        .into_any_element()
}

/// The worker: compile, run, hand the workbook back.
fn run_on_worker(
    book: Workbook,
    macros: MacroProject,
    name: String,
    view: View,
    options: RunOptions,
    tx: mpsc::Sender<ToUi>,
) {
    let (mut project, broken) = macros.compile();
    let (book, outcome) = match sheets_script::broken_for(&broken, &macros, &name) {
        Some(e) => (
            book,
            RunOutcome {
                result: Err(e),
                view,
                save_requested: false,
                changed: false,
            },
        ),
        None => {
            let ui = ChannelUi { tx: tx.clone() };
            sheets_script::run_macro(
                book,
                &macros,
                &mut project,
                &name,
                view,
                Box::new(ui),
                &options,
            )
        }
    };
    let _ = tx.send(ToUi::Done { book, outcome });
}
