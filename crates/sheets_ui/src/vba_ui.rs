//! Workbooks with macros (a VBA project, or JavaScript and Python scripts
//! stored in the workbook): a bar when one opens, and the Macros dialog
//! (Excel's View ▸ Macros, Alt+F8) listing the project's modules and
//! procedures and the scripts' functions, with a module's code shown
//! read-only (and a UserForm's layout), Run, and the macros' `Debug.Print`
//! / `console.log` / `print` output.
//!
//! Macros start disabled ([`MacroTrust`], per pane), as Excel opens a file
//! from the internet: the bar's Enable Content enables them for this pane
//! until it closes (nothing is remembered). Then Run, buttons and shapes
//! with an assigned macro run them (`macro_run`), and the workbook's VBA
//! functions work in its formulas. [`initial_trust`] is where trusted
//! locations or "always trust this file" would decide otherwise.

use std::rc::Rc;

use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, input, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{t, tf};
use kantor_ui::{metrics::*, tokens};
use sheets_engine::vba::{Module, ModuleKind, Procedure, VbaProject};

use crate::SheetsView;
use crate::dialog;
use crate::macro_run::MacroRun;

/// What the workbook's macros may do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum MacroTrust {
    /// No VBA project.
    #[default]
    NoMacros,
    /// Macros are shown and not run until the user enables them.
    Disabled,
    /// Enable Content was pressed: macros run (for this pane's session).
    Enabled,
}

/// Whether a workbook's macros may run when it opens. Always `Disabled`
/// for now; trusted locations and trusted documents (a remembered Enable
/// Content per file) would return `Enabled` here.
fn initial_trust(_path: Option<&std::path::Path>, has_macros: bool) -> MacroTrust {
    if has_macros {
        MacroTrust::Disabled
    } else {
        MacroTrust::NoMacros
    }
}

/// A pane's macros: the trust, whether the bar was closed, the project to
/// run, a run in progress and the last run's error and output.
#[derive(Default)]
pub(crate) struct MacroState {
    pub(crate) trust: MacroTrust,
    pub(crate) bar_closed: bool,
    /// The modules to run (read when content is enabled).
    pub(crate) project: Option<sheets_script::MacroProject>,
    pub(crate) run: Option<MacroRun>,
    /// The macro run last (`Module1.Macro1`).
    pub(crate) last_run: Option<String>,
    /// The run-time error dialog showing.
    pub(crate) error: Option<sheets_script::MacroError>,
    pub(crate) error_focus: Option<FocusHandle>,
    /// `Debug.Print` lines, oldest first.
    pub(crate) output: Vec<SharedString>,
    /// The macro thread with the live project (content enabled).
    pub(crate) thread: Option<crate::macro_thread::MacroThread>,
    /// The workbook is lent to a macro of another pane.
    pub(crate) lent: bool,
    /// What events last saw: the undo/redo history's lengths, the sheet
    /// and selection (see `macro_events`).
    pub(crate) seen_hist: Option<(usize, usize)>,
    pub(crate) seen_sel: Option<(u32, sheets_engine::CellRange)>,
    /// `Workbook_BeforeClose` / `Workbook_BeforeSave` ran and let it go on.
    pub(crate) close_approved: bool,
    pub(crate) save_approved: bool,
    /// Waits for the next `Application.OnTime` schedule.
    pub(crate) timer: Option<Task<()>>,
    /// Events that wait for the workbook (it was busy when they happened).
    pub(crate) pending: Vec<(sheets_script::Event, Option<usize>)>,
}

/// The Macros dialog.
pub(crate) struct MacrosDialog {
    /// Why the VBA project couldn't be read.
    error: Option<String>,
    /// The VBA project (for a UserForm's layout).
    project: Option<Rc<VbaProject>>,
    /// The VBA modules, then the JavaScript and Python scripts.
    modules: Vec<DialogModule>,
    module: usize,
    procedure: Option<usize>,
    /// The picked module's lines (tabs as spaces).
    lines: Rc<Vec<SharedString>>,
    scroll: UniformListScrollHandle,
    /// The picked module's form layout, when it is a UserForm.
    form: Option<Result<sheets_engine::vba::forms::FormDesign, String>>,
}

/// A module as the dialog lists it: a VBA module or a script.
struct DialogModule {
    name: String,
    /// `Module`, `UserForm`, `JavaScript`…
    kind: &'static str,
    code: String,
    /// A VBA UserForm (its layout is shown).
    is_form: bool,
    procedures: Vec<DialogProcedure>,
}

struct DialogProcedure {
    name: String,
    /// `Sub`, `Private Function`, `function`…
    what: String,
    /// Run can start it.
    is_macro: bool,
    line: usize,
    end_line: usize,
}

impl DialogModule {
    fn vba(m: &Module) -> DialogModule {
        DialogModule {
            name: m.name.clone(),
            kind: kind_label(m.kind),
            code: m.code.clone(),
            is_form: m.kind == ModuleKind::Form,
            procedures: m
                .procedures()
                .iter()
                .map(|p: &Procedure| {
                    let mut what = p.kind.keyword().to_owned();
                    if p.private {
                        what = format!("Private {what}");
                    }
                    DialogProcedure {
                        name: p.name.clone(),
                        what,
                        is_macro: p.is_macro() && m.kind != ModuleKind::Class,
                        line: p.line,
                        end_line: p.end_line,
                    }
                })
                .collect(),
        }
    }

    fn script(s: &sheets_engine::scripts::Script) -> DialogModule {
        let keyword = match s.language {
            sheets_engine::scripts::ScriptLanguage::JavaScript => "function",
            sheets_engine::scripts::ScriptLanguage::Python => "def",
        };
        DialogModule {
            name: s.name.clone(),
            kind: match s.language {
                sheets_engine::scripts::ScriptLanguage::JavaScript => t("JavaScript"),
                sheets_engine::scripts::ScriptLanguage::Python => t("Python"),
            },
            code: s.source.clone(),
            is_form: false,
            procedures: sheets_script::scripts::functions(s)
                .into_iter()
                .map(|f| DialogProcedure {
                    is_macro: f.is_macro() && sheets_script::scripts::can_run(s.language),
                    what: keyword.to_owned(),
                    name: f.name,
                    line: f.line,
                    end_line: f.end_line,
                })
                .collect(),
        }
    }
}

/// Rows of code shown at once.
const CODE_ROWS: f32 = 22.;
const LINE_H: Pixels = px(18.);

/// As the Visual Basic Editor's folders name them.
fn kind_label(kind: ModuleKind) -> &'static str {
    match kind {
        ModuleKind::Standard => t("Module"),
        ModuleKind::Class => t("Class Module"),
        ModuleKind::Document => t("Document"),
        ModuleKind::Form => t("UserForm"),
    }
}

fn code_lines(code: &str) -> Rc<Vec<SharedString>> {
    Rc::new(
        code.lines()
            .map(|l| SharedString::from(l.replace('\t', "    ")))
            .collect(),
    )
}

impl SheetsView {
    /// After a workbook was opened: the bar when it has macros.
    pub(crate) fn macros_after_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (has, path) = {
            let book = &self.table.read(cx).delegate().book;
            (
                book.has_macros() || book.has_scripts(),
                book.path().map(|p| p.to_path_buf()),
            )
        };
        self.macros = MacroState {
            trust: initial_trust(path.as_deref(), has),
            ..Default::default()
        };
        // The editor showed the workbook this pane had before.
        self.macro_editor = None;
        if self.macros.trust == MacroTrust::Enabled {
            self.macros.trust = MacroTrust::Disabled;
            if self.enable_content_now(cx) {
                self.fire_macro_event(sheets_script::Event::Open, None, window, cx);
            }
        }
    }

    /// Enable Content: macros run in this pane until it closes, and the
    /// workbook's VBA functions work in its formulas.
    pub(crate) fn enable_content(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() || !self.enable_content_now(cx) {
            return;
        }
        self.status = t("Macros enabled for this session").into();
        cx.notify();
        // Workbook_Open, as when Excel opens a trusted file; then formulas
        // using the VBA functions get their values.
        let opened = self.fire_event_then(
            sheets_script::Event::Open,
            crate::macro_run::After::Recalculate,
            None,
            window,
            cx,
        );
        if !opened {
            self.calculate_doc(window, cx);
        }
    }

    pub(crate) fn enable_content_now(&mut self, cx: &mut Context<Self>) -> bool {
        let read = sheets_script::MacroProject::of_book(&self.table.read(cx).delegate().book);
        let project = match read {
            Some(Ok(p)) => p,
            Some(Err(e)) => {
                self.status = tf("Kantor can't read this workbook's macros: {}", &[&e]).into();
                cx.notify();
                return false;
            }
            None => return false,
        };
        self.table.update(cx, |table, _| {
            sheets_script::enable_functions(&mut table.delegate_mut().book, Some(&project));
        });
        self.macros.thread = crate::macro_thread::MacroThread::start(project.clone());
        self.macros.project = Some(project);
        self.macros.trust = MacroTrust::Enabled;
        self.macro_events_seen(cx);
        cx.notify();
        true
    }

    /// The bar saying the workbook has macros (until closed).
    pub(crate) fn render_macro_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.macros.trust != MacroTrust::Disabled || self.macros.bar_closed {
            return None;
        }
        let t_ = tokens(cx);
        Some(
            h_flex()
                .id("sheets-macro-bar")
                .flex_none()
                .h(CONTROL_LG + SPACE_2)
                .gap(SPACE_4)
                .px(SPACE_4)
                .bg(t_.warning.opacity(0.14))
                .border_b_1()
                .border_color(t_.border)
                .text_size(FONT_SMALL)
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t_.fg)
                        .child(t("Macros")),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(t_.fg_muted)
                        .child(t(
                            "Macros have been disabled. Enable them only if you trust where this file came from.",
                        )),
                )
                .child(
                    Button::new("sheets-macro-enable")
                        .label(t("Enable Content"))
                        .primary()
                        .small()
                        .on_click(cx.listener(|this, _, window, cx| this.enable_content(window, cx))),
                )
                .child(
                    Button::new("sheets-macro-view")
                        .label(t("View Macros"))
                        .small()
                        .on_click(cx.listener(|this, _, window, cx| this.macros_doc(window, cx))),
                )
                .child(
                    Button::new("sheets-macro-close")
                        .icon(gpui_kit::assets::IconName::X)
                        .ghost()
                        .small()
                        .tooltip(t("Close"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.macros.bar_closed = true;
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }

    /// Open the Macros dialog (View ▸ Macros, Alt+F8).
    pub fn macros_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        let (project, scripts) = {
            let book = &self.table.read(cx).delegate().book;
            (book.vba_project(), book.scripts())
        };
        let (scripts, scripts_error) = match scripts {
            Ok(s) => (s, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        if project.is_none() && scripts.is_empty() && scripts_error.is_none() {
            self.status =
                t("This workbook has no macros. Alt+F11 opens the Macro Editor to write one.")
                    .into();
            cx.notify();
            return;
        }
        let (project, mut error) = match project {
            Some(Ok(p)) => (Some(Rc::new(p)), None),
            Some(Err(e)) => {
                log::warn!("reading the VBA project: {e}");
                (None, Some(e))
            }
            None => (None, None),
        };
        if let Some(e) = scripts_error {
            log::warn!("reading the workbook's scripts: {e}");
            if error.is_none() {
                error = Some(e);
            }
        }
        let mut modules: Vec<DialogModule> = project
            .as_ref()
            .map(|p| p.modules().iter().map(DialogModule::vba).collect())
            .unwrap_or_default();
        modules.extend(scripts.iter().map(DialogModule::script));
        // The first module with a procedure, as Alt+F8 lists macros first.
        let module = modules
            .iter()
            .position(|m| !m.procedures.is_empty())
            .unwrap_or(0);
        let lines = modules
            .get(module)
            .map(|m| code_lines(&m.code))
            .unwrap_or_default();
        let first = modules
            .get(module)
            .and_then(|m| (!m.procedures.is_empty()).then_some(0));
        self.macros_dialog = Some(MacrosDialog {
            error,
            project,
            modules,
            module,
            procedure: first,
            lines,
            scroll: UniformListScrollHandle::new(),
            form: None,
        });
        self.pick_macro(module, first, window, cx);
    }

    fn close_macros(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.macros_dialog.take().is_some() {
            self.focus_grid(window, cx);
            cx.notify();
        }
    }

    /// Show module `module`'s code, scrolled to procedure `procedure`.
    fn pick_macro(
        &mut self,
        module: usize,
        procedure: Option<usize>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(d) = self.macros_dialog.as_mut() else {
            return;
        };
        if let Some(m) = d.modules.get(module) {
            if d.module != module || d.lines.is_empty() {
                d.lines = code_lines(&m.code);
                d.form = match (&d.project, m.is_form) {
                    (Some(p), true) => p.form(&m.name).map(|r| r.map_err(|e| e.to_string())),
                    _ => None,
                };
            }
            d.module = module;
            d.procedure = procedure;
            let line = procedure
                .and_then(|i| d.modules.get(module)?.procedures.get(i))
                .map_or(0, |p| p.line);
            d.scroll.scroll_to_item(line, ScrollStrategy::Top);
        }
        cx.notify();
    }

    pub(crate) fn render_macros_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.macros_dialog.as_ref()?;
        let view = cx.entity().downgrade();
        let t_ = tokens(cx);
        let mut card = dialog::card("macros", t("Macros"), px(860.), cx)
            .key_context("KantorMacros")
            .on_action(cx.listener(|this, _: &input::Escape, w, cx| this.close_macros(w, cx)));
        if let Some(e) = &d.error {
            card = card.child(dialog::note(
                tf("Kantor can't read this workbook's macros: {}", &[e]),
                true,
                cx,
            ));
        }
        {
            if !d.modules.is_empty() {
                let mono = cx.theme().mono_font_family.clone();
                // Modules, each with its procedures.
                let mut list = v_flex()
                    .id("macros-modules")
                    .flex_none()
                    .w(px(260.))
                    .h(LINE_H * CODE_ROWS)
                    .overflow_y_scroll()
                    .border_1()
                    .border_color(t_.border)
                    .rounded(RADIUS_SM)
                    .p(SPACE_1)
                    .text_size(FONT_SMALL);
                for (mi, m) in d.modules.iter().enumerate() {
                    let v = view.clone();
                    let on = d.module == mi && d.procedure.is_none();
                    list = list.child(
                        h_flex()
                            .id(("macro-module", mi))
                            .gap(SPACE_2)
                            .px(SPACE_2)
                            .py(px(2.))
                            .rounded(RADIUS_SM)
                            .when(on, |r| r.bg(t_.selected))
                            .hover(|r| r.bg(t_.hover))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(m.name.clone()),
                            )
                            .child(div().flex_none().text_color(t_.fg_muted).child(m.kind))
                            .on_click(move |_, window, cx| {
                                let _ =
                                    v.update(cx, |this, cx| this.pick_macro(mi, None, window, cx));
                            }),
                    );
                    let procs = &m.procedures;
                    for (pi, p) in procs.iter().enumerate() {
                        let v = view.clone();
                        let on = d.module == mi && d.procedure == Some(pi);
                        let what = p.what.clone();
                        list = list.child(
                            h_flex()
                                .id(("macro-procedure", mi * 10_000 + pi))
                                .gap(SPACE_2)
                                .pl(SPACE_6)
                                .pr(SPACE_2)
                                .py(px(2.))
                                .rounded(RADIUS_SM)
                                .when(on, |r| r.bg(t_.selected))
                                .hover(|r| r.bg(t_.hover))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .when(!p.is_macro, |d| d.text_color(t_.fg_muted))
                                        .child(p.name.clone()),
                                )
                                .child(div().flex_none().text_color(t_.fg_dim).child(what))
                                .on_click(move |_, window, cx| {
                                    let _ = v.update(cx, |this, cx| {
                                        this.pick_macro(mi, Some(pi), window, cx)
                                    });
                                }),
                        );
                    }
                }
                // The code, read-only, with line numbers.
                let lines = d.lines.clone();
                let span = d
                    .procedure
                    .and_then(|i| d.modules.get(d.module)?.procedures.get(i))
                    .map(|p| (p.line, p.end_line));
                let digits = lines.len().max(1).to_string().len();
                let gutter = px(9. * digits as f32 + 12.);
                let code = if lines.is_empty() {
                    div()
                        .flex_1()
                        .h(LINE_H * CODE_ROWS)
                        .p(SPACE_4)
                        .border_1()
                        .border_color(t_.border)
                        .rounded(RADIUS_SM)
                        .text_size(FONT_SMALL)
                        .text_color(t_.fg_muted)
                        .child(t("This module has no code."))
                        .into_any_element()
                } else {
                    uniform_list("macros-code", lines.len(), move |range, _, _| {
                        range
                            .map(|i| {
                                let inside = span.is_some_and(|(a, b)| i >= a && i <= b);
                                h_flex()
                                    .id(("macro-line", i))
                                    .h(LINE_H)
                                    .w_full()
                                    .when(inside, |r| r.bg(t_.selected.opacity(0.5)))
                                    .child(
                                        h_flex()
                                            .flex_none()
                                            .w(gutter)
                                            .pr(SPACE_3)
                                            .justify_end()
                                            .text_color(t_.fg_dim)
                                            .child(SharedString::from((i + 1).to_string())),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .whitespace_nowrap()
                                            .overflow_hidden()
                                            .text_color(t_.fg)
                                            .child(lines[i].clone()),
                                    )
                            })
                            .collect::<Vec<_>>()
                    })
                    .track_scroll(&d.scroll)
                    .flex_1()
                    .h(LINE_H * CODE_ROWS)
                    .py(SPACE_1)
                    .border_1()
                    .border_color(t_.border)
                    .rounded(RADIUS_SM)
                    .bg(t_.surface)
                    .font_family(mono)
                    .text_size(FONT_SMALL)
                    .into_any_element()
                };
                card = card.child(h_flex().gap(SPACE_4).items_start().child(list).child(code));
                // A UserForm: its layout, read-only.
                match &d.form {
                    Some(Ok(design)) => {
                        card = card.child(
                            div()
                                .id("macros-form-preview")
                                .max_h(px(320.))
                                .overflow_y_scroll()
                                .child(crate::macro_form::form_preview(design, 820., cx)),
                        );
                    }
                    Some(Err(e)) => {
                        card = card.child(crate::macro_form::preview_unavailable(e, cx));
                    }
                    None => {}
                }
            }
        }
        // The selected procedure, when it is a macro Run can start.
        let picked = d
            .procedure
            .and_then(|i| d.modules.get(d.module)?.procedures.get(i))
            .filter(|p| p.is_macro)
            .and_then(|p| {
                let m = d.modules.get(d.module)?;
                Some(format!("{}.{}", m.name, p.name))
            });
        let enabled = self.macros.trust == MacroTrust::Enabled;
        let note = if enabled {
            t("Run starts the selected macro. One Undo takes back everything it changed.")
        } else {
            t(
                "Macros are disabled. Enable Content on the bar to run them. Paparan AI can explain a macro or turn it into formulas.",
            )
        };
        // `Debug.Print` output of the runs so far.
        let output = (!self.macros.output.is_empty()).then(|| {
            let lines = &self.macros.output;
            let shown = &lines[lines.len().saturating_sub(8)..];
            v_flex()
                .gap(SPACE_1)
                .child(
                    h_flex()
                        .justify_between()
                        .text_size(FONT_SMALL)
                        .text_color(t_.fg_muted)
                        .child(t("Output (Debug.Print)"))
                        .child(
                            Button::new("macros-output-clear")
                                .label(t("Clear"))
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.macros.output.clear();
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    v_flex()
                        .id("macros-output")
                        .max_h(LINE_H * 8.)
                        .overflow_y_scroll()
                        .p(SPACE_2)
                        .border_1()
                        .border_color(t_.border)
                        .rounded(RADIUS_SM)
                        .bg(t_.surface)
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(FONT_SMALL)
                        .children(shown.iter().map(|l| div().child(l.clone()))),
                )
        });
        let close = view.clone();
        let run_view = view.clone();
        let edit_view = view.clone();
        let edit_module = d.modules.get(d.module).map(|m| m.name.clone());
        let card = card
            .children(output)
            .child(dialog::note(note, false, cx))
            .child(
                h_flex()
                    .justify_end()
                    .gap(SPACE_3)
                    .child(
                        Button::new("macros-close")
                            .label(t("Close"))
                            .ghost()
                            .small()
                            .on_click(move |_, window, cx| {
                                let _ = close.update(cx, |this, cx| this.close_macros(window, cx));
                            }),
                    )
                    .child(
                        Button::new("macros-edit")
                            .label(t("Edit"))
                            .small()
                            .on_click(move |_, window, cx| {
                                let module = edit_module.clone();
                                let _ = edit_view.update(cx, |this, cx| {
                                    this.close_macros(window, cx);
                                    this.open_macro_editor(module.as_deref(), window, cx);
                                });
                            }),
                    )
                    .child(
                        Button::new("macros-run")
                            .label(t("Run"))
                            .primary()
                            .small()
                            .disabled(!enabled || picked.is_none())
                            .on_click(move |_, window, cx| {
                                let Some(name) = picked.clone() else {
                                    return;
                                };
                                let _ = run_view.update(cx, |this, cx| {
                                    this.close_macros(window, cx);
                                    this.run_macro(name, window, cx);
                                });
                            }),
                    ),
            );
        Some(dialog::place(card, self, cx).into_any_element())
    }
}
