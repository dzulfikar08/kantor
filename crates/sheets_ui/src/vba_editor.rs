//! The Macro Editor (Excel's Visual Basic Editor, Alt+F11, simpler): a
//! panel beside the grid with the workbook's modules (ThisWorkbook, the
//! sheet modules, standard and class modules, UserForms' code, JavaScript
//! and Python scripts) and a code editor (gpui-base's, with line numbers,
//! search, undo; colours from [`crate::code_highlight`]).
//!
//! Edits go into the workbook a moment after typing stops
//! (`sheets_script::code::apply`: the VBA project is rewritten without its
//! compiled caches, the scripts part replaced), so they are part of the
//! workbook's unsaved state and of the next save (`.xlsm` keeps VBA;
//! saving as `.xlsx` asks, as Excel does). The live macro project is
//! rebuilt with them, so the next run or event uses the new code (module
//! variables start over, as after an edit in Excel).
//!
//! Trust: editing never needs Enable Content. Running does, with one
//! exception: in a workbook that opened without macros, the code is the
//! user's own (typed or pasted in this session), so the first code makes
//! the pane's macros enabled. A workbook that opened with macros keeps them
//! disabled until Enable Content. Only the user runs macros (F5, Run, the
//! Macros dialog): nothing here is reachable from MCP or Paparan AI.

use std::time::Duration;

use gpui_kit::base::input::{Diagnostic as EditorProblem, DiagnosticSeverity};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{
    Editor, EditorState, Input, InputEvent, InputState, Position, TabSize,
};
use gpui_kit::component::{Disableable as _, Sizable as _, h_flex, input, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{t, tf};
use kantor_ui::{metrics::*, tokens};
use sheets_script::code::{self, CodeKind, CodeModule, Diagnostic};

use crate::SheetsView;
use crate::code_highlight::{self, CodeLanguage};
use crate::vba_ui::MacroTrust;

actions!(sheets_vba, [ToggleMacroEditor, RunMacroUnderCaret]);

/// The editor panel's key context.
pub(crate) const CONTEXT: &str = "KantorMacroEditor";

/// How long after the last keystroke the code goes into the workbook.
const COMMIT_DELAY: Duration = Duration::from_millis(400);

pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("alt-f11", ToggleMacroEditor, Some(crate::KEY_CONTEXT)),
        KeyBinding::new("f5", RunMacroUnderCaret, Some(CONTEXT)),
    ]);
}

/// One module open in the editor.
struct EditModule {
    module: CodeModule,
    state: Entity<EditorState>,
    /// The last check's problems (VBA), lines from 1.
    problems: Vec<Diagnostic>,
    _sub: Subscription,
}

/// The Macro Editor's state (one per pane).
pub(crate) struct MacroEditor {
    modules: Vec<EditModule>,
    current: usize,
    /// What to tell the user (an error, why F5 didn't run).
    note: Option<(SharedString, bool)>,
    renaming: Option<(Entity<InputState>, Subscription)>,
    /// Remove was pressed: waiting for the confirmation.
    removing: bool,
    /// Code typed and not yet in the workbook.
    dirty: bool,
    /// The workbook was busy (a macro running): commit when it's back.
    pending: bool,
    commit_task: Option<Task<()>>,
    /// The last run started here: the editor takes the focus back.
    ran: bool,
}

fn language(kind: CodeKind) -> CodeLanguage {
    match kind {
        CodeKind::JavaScript => CodeLanguage::JavaScript,
        CodeKind::Python => CodeLanguage::Plain,
        _ => CodeLanguage::Vba,
    }
}

fn kind_label(kind: CodeKind) -> &'static str {
    match kind {
        CodeKind::Document => t("Document"),
        CodeKind::Standard => t("Module"),
        CodeKind::Class => t("Class Module"),
        CodeKind::Form => t("UserForm"),
        CodeKind::JavaScript => t("JavaScript"),
        CodeKind::Python => t("Python"),
    }
}

/// Tree groups, in order, with their titles.
fn group_of(kind: CodeKind) -> usize {
    match kind {
        CodeKind::Document => 0,
        CodeKind::Standard => 1,
        CodeKind::Class => 2,
        CodeKind::Form => 3,
        CodeKind::JavaScript | CodeKind::Python => 4,
    }
}

fn group_title(group: usize) -> &'static str {
    match group {
        0 => t("Workbook and Sheets"),
        1 => t("Modules"),
        2 => t("Class Modules"),
        3 => t("Forms"),
        _ => t("Scripts"),
    }
}

/// The starter code of a new JavaScript module.
const JS_STARTER: &str = "// Top-level functions without parameters are macros (F5 runs the one\n// the cursor is in).\nfunction main() {\n  Range(\"A1\").value = \"Hello\";\n}\n";

impl SheetsView {
    /// Alt+F11: open the Macro Editor, or move between it and the grid.
    pub(crate) fn toggle_macro_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.macro_editor {
            Some(ed) => {
                let state = ed.modules[ed.current].state.clone();
                if state.read(cx).focus_handle(cx).contains_focused(window, cx) {
                    self.focus_grid(window, cx);
                } else {
                    state.update(cx, |s, cx| s.focus(window, cx));
                }
            }
            None => self.open_macro_editor(None, window, cx),
        }
    }

    /// Open the Macro Editor (on module `module`, else the first module
    /// with code).
    pub fn open_macro_editor(
        &mut self,
        module: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editing.is_some() {
            self.commit_edit(None, window, cx);
        }
        if self.blocked(cx) {
            return;
        }
        if let Some(name) = module
            && let Some(ed) = &self.macro_editor
            && let Some(i) = ed
                .modules
                .iter()
                .position(|m| m.module.name.eq_ignore_ascii_case(name))
        {
            self.macro_editor_select(i, window, cx);
            return;
        }
        let read = code::code_modules(&self.table.read(cx).delegate().book);
        let modules = match read {
            Ok(m) => m,
            Err(e) => {
                self.status = tf("Kantor can't read this workbook's macros: {}", &[&e]).into();
                cx.notify();
                return;
            }
        };
        let mut list = Vec::with_capacity(modules.len());
        for m in modules {
            list.push(self.edit_module(m, window, cx));
        }
        list.sort_by_key(|m| group_of(m.module.kind));
        let current = module
            .and_then(|n| {
                list.iter()
                    .position(|m| m.module.name.eq_ignore_ascii_case(n))
            })
            .or_else(|| list.iter().position(|m| !m.module.code.trim().is_empty()))
            .unwrap_or(0);
        self.macros_dialog = None;
        self.macro_editor = Some(MacroEditor {
            modules: list,
            current,
            note: None,
            renaming: None,
            removing: false,
            dirty: false,
            pending: false,
            commit_task: None,
            ran: false,
        });
        self.macro_editor_check(cx);
        self.focus_macro_editor(window, cx);
        cx.notify();
    }

    fn edit_module(
        &mut self,
        module: CodeModule,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> EditModule {
        let lang = language(module.kind);
        let code = module.code.clone();
        let state = cx.new(|cx| {
            let mut s = EditorState::new(window, cx)
                .language(lang.name())
                .line_number(true)
                .folding(false)
                .soft_wrap(false)
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: false,
                })
                .default_value(code);
            s.set_highlighter_factory(code_highlight::factory(), cx);
            s
        });
        let sub = cx.subscribe_in(&state, window, |this, _, e: &InputEvent, window, cx| {
            if matches!(e, InputEvent::Change) {
                this.macro_code_changed(window, cx);
            }
        });
        EditModule {
            module,
            state,
            problems: Vec::new(),
            _sub: sub,
        }
    }

    fn focus_macro_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ed) = &self.macro_editor
            && let Some(m) = ed.modules.get(ed.current)
        {
            m.state.update(cx, |s, cx| s.focus(window, cx));
        }
    }

    /// Code typed: into the workbook once typing stops.
    fn macro_code_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.macro_editor.as_mut() else {
            return;
        };
        ed.dirty = true;
        ed.commit_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(COMMIT_DELAY).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.commit_macro_code(window, cx);
                this.macro_editor_check(cx);
            });
        }));
    }

    /// The editor's modules as they are now.
    fn editor_modules(&self, cx: &App) -> Vec<CodeModule> {
        let Some(ed) = &self.macro_editor else {
            return Vec::new();
        };
        ed.modules
            .iter()
            .map(|m| {
                let mut c = m.module.clone();
                c.code = m.state.read(cx).value().to_string();
                c
            })
            .collect()
    }

    /// Puts the editor's code into the workbook (and the live project).
    /// `false`: it couldn't (the note says why) or has to wait for a run.
    pub(crate) fn commit_macro_code(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let busy = self.busy.is_some() || self.macros.run.is_some() || self.macros.lent;
        let Some(ed) = self.macro_editor.as_mut() else {
            return true;
        };
        ed.commit_task = None;
        if !ed.dirty {
            return true;
        }
        if busy {
            ed.pending = true;
            return false;
        }
        let modules = self.editor_modules(cx);
        let result = self.table.update(cx, |table, _| {
            code::apply(&mut table.delegate_mut().book, &modules)
        });
        let Some(ed) = self.macro_editor.as_mut() else {
            return true;
        };
        ed.pending = false;
        match result {
            Ok(changed) => {
                ed.dirty = false;
                if ed.note.as_ref().is_some_and(|(_, error)| *error) {
                    ed.note = None;
                }
                for (m, c) in ed.modules.iter_mut().zip(modules) {
                    m.module = c;
                }
                if changed {
                    self.reload_live_macros(cx);
                    self.update_title(window, cx);
                }
                cx.notify();
                true
            }
            Err(e) => {
                ed.note = Some((
                    tf("The code can't be saved in the workbook: {}", &[&e]).into(),
                    true,
                ));
                cx.notify();
                false
            }
        }
    }

    /// The workbook's macros changed: the live project takes the new code
    /// (its module variables start over). Code in a workbook that opened
    /// without macros is the user's own: it enables them (see the module
    /// docs).
    fn reload_live_macros(&mut self, cx: &mut Context<Self>) {
        let (has, read) = {
            let book = &self.table.read(cx).delegate().book;
            (
                book.has_macros() || book.has_scripts(),
                sheets_script::MacroProject::of_book(book),
            )
        };
        match self.macros.trust {
            MacroTrust::Enabled => {
                let project = match read {
                    Some(Ok(p)) => p,
                    Some(Err(e)) => {
                        self.status =
                            tf("Kantor can't read this workbook's macros: {}", &[&e]).into();
                        return;
                    }
                    None => sheets_script::MacroProject::default(),
                };
                self.table.update(cx, |table, _| {
                    sheets_script::enable_functions(&mut table.delegate_mut().book, Some(&project));
                });
                self.macros.thread = crate::macro_thread::MacroThread::start(project.clone());
                self.macros.project = Some(project);
                self.macro_events_seen(cx);
            }
            MacroTrust::NoMacros if has => {
                self.enable_content_now(cx);
                self.status = t("Macros you write here run in this window").into();
            }
            MacroTrust::NoMacros | MacroTrust::Disabled => {}
        }
    }

    /// Checks the current module (VBA) and shows its problems at their
    /// lines.
    fn macro_editor_check(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.macro_editor.as_mut() else {
            return;
        };
        let Some(m) = ed.modules.get_mut(ed.current) else {
            return;
        };
        let mut module = m.module.clone();
        module.code = m.state.read(cx).value().to_string();
        m.problems = code::diagnostics(&module);
        show_problems(&m.state, &m.problems, cx);
        cx.notify();
    }

    fn macro_editor_select(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_macro_code(window, cx);
        let Some(ed) = self.macro_editor.as_mut() else {
            return;
        };
        if ix >= ed.modules.len() {
            return;
        }
        ed.current = ix;
        ed.renaming = None;
        ed.removing = false;
        ed.note = None;
        self.macro_editor_check(cx);
        self.focus_macro_editor(window, cx);
        cx.notify();
    }

    /// Insert ▸ Module / Class Module / JavaScript.
    fn macro_editor_insert(&mut self, kind: CodeKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.macro_editor.is_none() {
            return;
        }
        let modules = self.editor_modules(cx);
        let name = code::free_name(&modules, kind);
        let starter = if kind == CodeKind::JavaScript {
            JS_STARTER
        } else {
            ""
        };
        let m = self.edit_module(CodeModule::new(&name, kind, starter), window, cx);
        let Some(ed) = self.macro_editor.as_mut() else {
            return;
        };
        // After the last module of its group.
        let at = ed
            .modules
            .iter()
            .rposition(|e| group_of(e.module.kind) <= group_of(kind))
            .map_or(0, |i| i + 1);
        ed.modules.insert(at, m);
        ed.dirty = true;
        ed.current = at;
        ed.note = None;
        ed.removing = false;
        self.commit_macro_code(window, cx);
        self.macro_editor_check(cx);
        self.focus_macro_editor(window, cx);
        cx.notify();
    }

    fn macro_editor_start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.macro_editor.as_ref() else {
            return;
        };
        let m = &ed.modules[ed.current].module;
        if !m.kind.can_rename() {
            return;
        }
        let name = m.name.clone();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        let sub = cx.subscribe_in(
            &input,
            window,
            |this, _, e: &InputEvent, window, cx| match e {
                InputEvent::PressEnter { .. } => this.macro_editor_finish_rename(window, cx),
                InputEvent::Blur => {
                    if let Some(ed) = this.macro_editor.as_mut() {
                        ed.renaming = None;
                        cx.notify();
                    }
                }
                _ => {}
            },
        );
        input.update(cx, |s, cx| s.focus(window, cx));
        if let Some(ed) = self.macro_editor.as_mut() {
            ed.renaming = Some((input, sub));
            ed.removing = false;
        }
        cx.notify();
    }

    fn macro_editor_finish_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let modules = self.editor_modules(cx);
        let Some(ed) = self.macro_editor.as_mut() else {
            return;
        };
        let Some((input, _)) = ed.renaming.as_ref() else {
            return;
        };
        let name = input.read(cx).value().trim().to_owned();
        if let Some(problem) = code::name_problem(&modules, &name, Some(ed.current)) {
            let text = if problem.starts_with("Another") {
                t("Another module has that name")
            } else {
                t(
                    "A module name starts with a letter and has only letters, digits and _ (up to 31)",
                )
            };
            ed.note = Some((text.into(), true));
            cx.notify();
            return;
        }
        ed.renaming = None;
        let m = &mut ed.modules[ed.current].module;
        if m.name != name {
            m.name = name;
            ed.dirty = true;
            self.commit_macro_code(window, cx);
        }
        self.focus_macro_editor(window, cx);
        cx.notify();
    }

    fn macro_editor_remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.macro_editor.as_mut() else {
            return;
        };
        if !ed.modules[ed.current].module.kind.can_rename() {
            return;
        }
        ed.modules.remove(ed.current);
        ed.current = ed.current.min(ed.modules.len().saturating_sub(1));
        ed.removing = false;
        ed.dirty = true;
        self.commit_macro_code(window, cx);
        self.macro_editor_check(cx);
        self.focus_macro_editor(window, cx);
        cx.notify();
    }

    /// Close the editor (the code goes into the workbook first).
    pub(crate) fn close_macro_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.macro_editor.is_none() {
            return;
        }
        if !self.commit_macro_code(window, cx)
            && self.macro_editor.as_ref().is_some_and(|ed| !ed.pending)
        {
            // Couldn't go into the workbook: keep the editor so the code
            // isn't lost (the note says why).
            return;
        }
        if self.macro_editor.as_ref().is_some_and(|ed| ed.pending) {
            return;
        }
        self.macro_editor = None;
        self.focus_grid(window, cx);
        cx.notify();
    }

    /// F5 / Run: runs the macro the cursor is in (the module's only macro
    /// when the cursor is outside every procedure).
    pub(crate) fn run_macro_under_caret(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.macro_editor.is_none() {
            return;
        }
        if self.macros.run.is_some() || self.busy.is_some() {
            self.blocked(cx);
            return;
        }
        if !self.commit_macro_code(window, cx) {
            return;
        }
        self.macro_editor_check(cx);
        let Some(ed) = self.macro_editor.as_mut() else {
            return;
        };
        let Some(m) = ed.modules.get(ed.current) else {
            return;
        };
        if let Some(p) = m.problems.iter().find(|p| p.fatal) {
            ed.note = Some((
                tf(
                    "Line {}: {}. Fix it to run this module.",
                    &[&p.line, &p.message],
                )
                .into(),
                true,
            ));
            cx.notify();
            return;
        }
        let module = m.module.clone();
        let line = m.state.read(cx).cursor_position().line as usize;
        let name = match code::macro_at(&module, line) {
            Ok(name) => name,
            Err(name) if !name.is_empty() => {
                ed.note = Some((
                    tf(
                        "{} can't run by itself: only a Sub without arguments (a macro) can.",
                        &[&name],
                    )
                    .into(),
                    true,
                ));
                cx.notify();
                return;
            }
            Err(_) => {
                let macros: Vec<usize> = (0..module.code.lines().count())
                    .filter_map(|l| code::macro_at(&module, l).ok().map(|_| l))
                    .collect();
                let names: Vec<String> = {
                    let mut v: Vec<String> = macros
                        .iter()
                        .filter_map(|l| code::macro_at(&module, *l).ok())
                        .collect();
                    v.dedup();
                    v
                };
                match names.as_slice() {
                    [one] => one.clone(),
                    [] => {
                        ed.note = Some((t("This module has no macro to run.").into(), true));
                        cx.notify();
                        return;
                    }
                    _ => {
                        ed.note = Some((
                            t("Put the cursor inside the macro to run, then press F5.").into(),
                            true,
                        ));
                        cx.notify();
                        return;
                    }
                }
            }
        };
        if self.macros.trust == MacroTrust::Disabled {
            if let Some(ed) = self.macro_editor.as_mut() {
                ed.note = Some((
                    t("Macros are disabled. Enable Content to run them.").into(),
                    true,
                ));
            }
            self.macros_disabled_notice(cx);
            return;
        }
        if self.macros.trust == MacroTrust::NoMacros {
            self.enable_content_now(cx);
        }
        if let Some(ed) = self.macro_editor.as_mut() {
            ed.note = None;
            ed.ran = true;
        }
        self.run_macro(name, window, cx);
    }

    /// A run ended: code typed meanwhile goes in; a run-time error in a
    /// module shown here is marked at its line.
    pub(crate) fn macro_editor_after_run(
        &mut self,
        result: &Result<(), sheets_script::MacroError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ed) = self.macro_editor.as_mut() else {
            return;
        };
        let ran = std::mem::take(&mut ed.ran);
        if ed.pending {
            self.commit_macro_code(window, cx);
        }
        let Some(ed) = self.macro_editor.as_mut() else {
            return;
        };
        match result {
            Err(e) if !e.module.is_empty() => {
                let Some(ix) = ed
                    .modules
                    .iter()
                    .position(|m| m.module.name.eq_ignore_ascii_case(&e.module))
                else {
                    return;
                };
                ed.current = ix;
                let m = &ed.modules[ix];
                let line = e.line.max(1);
                let problem = Diagnostic {
                    line,
                    message: format!("Run-time error '{}': {}", e.number, e.description),
                    fatal: true,
                    unsupported: false,
                };
                let mut problems = m.problems.clone();
                problems.push(problem);
                show_problems(&m.state, &problems, cx);
                m.state.update(cx, |s, cx| {
                    s.set_cursor_position(
                        Position {
                            line: line - 1,
                            character: 0,
                        },
                        window,
                        cx,
                    )
                });
                ed.note = Some((
                    tf("Run-time error '{}': {}", &[&e.number, &e.description]).into(),
                    true,
                ));
                cx.notify();
            }
            Ok(()) if ran => {
                self.focus_macro_editor(window, cx);
            }
            _ => {}
        }
    }

    /// The panel beside the grid.
    pub(crate) fn render_macro_editor(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ed = self.macro_editor.as_ref()?;
        let t_ = tokens(cx);
        let view = cx.entity().downgrade();
        let current = ed.modules.get(ed.current);
        let can_rename = current.is_some_and(|m| m.module.kind.can_rename());
        let button =
            |id: &'static str, label: &'static str| Button::new(id).label(label).xsmall().ghost();

        let toolbar = {
            let v = view.clone();
            let v2 = view.clone();
            let v3 = view.clone();
            let v4 = view.clone();
            let v5 = view.clone();
            let v6 = view.clone();
            let v7 = view.clone();
            let v8 = view.clone();
            h_flex()
                .flex_none()
                .gap(SPACE_1)
                .flex_wrap()
                .child(
                    div()
                        .flex_none()
                        .mr(SPACE_3)
                        .text_size(FONT_EMPHASIS)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t("Macro Editor")),
                )
                .child(
                    button("vba-insert-module", t("Insert Module")).on_click(move |_, w, cx| {
                        let _ = v.update(cx, |this, cx| {
                            this.macro_editor_insert(CodeKind::Standard, w, cx)
                        });
                    }),
                )
                .child(
                    button("vba-insert-class", t("Insert Class Module")).on_click(
                        move |_, w, cx| {
                            let _ = v2.update(cx, |this, cx| {
                                this.macro_editor_insert(CodeKind::Class, w, cx)
                            });
                        },
                    ),
                )
                .child(
                    button("vba-insert-js", t("Insert JavaScript")).on_click(move |_, w, cx| {
                        let _ = v3.update(cx, |this, cx| {
                            this.macro_editor_insert(CodeKind::JavaScript, w, cx)
                        });
                    }),
                )
                .child(
                    button("vba-rename", t("Rename"))
                        .disabled(!can_rename)
                        .on_click(move |_, w, cx| {
                            let _ = v4.update(cx, |this, cx| this.macro_editor_start_rename(w, cx));
                        }),
                )
                .child(
                    button("vba-remove", t("Remove"))
                        .disabled(!can_rename)
                        .on_click(move |_, _, cx| {
                            let _ = v5.update(cx, |this, cx| {
                                if let Some(ed) = this.macro_editor.as_mut() {
                                    ed.removing = true;
                                    cx.notify();
                                }
                            });
                        }),
                )
                .child(div().flex_1())
                .when(self.macros.trust == MacroTrust::Disabled, |row| {
                    row.child(
                        Button::new("vba-enable")
                            .label(t("Enable Content"))
                            .xsmall()
                            .on_click(move |_, w, cx| {
                                let _ = v6.update(cx, |this, cx| this.enable_content(w, cx));
                            }),
                    )
                })
                .child(
                    Button::new("vba-run")
                        .label(t("Run (F5)"))
                        .primary()
                        .xsmall()
                        .on_click(move |_, w, cx| {
                            let _ = v7.update(cx, |this, cx| this.run_macro_under_caret(w, cx));
                        }),
                )
                .child(
                    Button::new("vba-close")
                        .icon(gpui_kit::assets::IconName::X)
                        .ghost()
                        .xsmall()
                        .tooltip(t("Close"))
                        .on_click(move |_, w, cx| {
                            let _ = v8.update(cx, |this, cx| this.close_macro_editor(w, cx));
                        }),
                )
        };

        // The project tree.
        let mut tree = v_flex()
            .id("vba-tree")
            .flex_none()
            .w(px(200.))
            .h_full()
            .overflow_y_scroll()
            .py(SPACE_2)
            .border_r_1()
            .border_color(t_.border)
            .text_size(FONT_SMALL);
        let mut last_group = usize::MAX;
        for (i, m) in ed.modules.iter().enumerate() {
            let group = group_of(m.module.kind);
            if group != last_group {
                last_group = group;
                tree = tree.child(
                    div()
                        .px(SPACE_3)
                        .pt(SPACE_2)
                        .pb(px(2.))
                        .text_color(t_.fg_muted)
                        .font_weight(FontWeight::MEDIUM)
                        .child(group_title(group)),
                );
            }
            let on = i == ed.current;
            let renaming = on && ed.renaming.is_some();
            let v = view.clone();
            let label: SharedString = match &m.module.sheet {
                Some(sheet) if *sheet != m.module.name => {
                    format!("{} ({sheet})", m.module.name).into()
                }
                _ => m.module.name.clone().into(),
            };
            let row = h_flex()
                .id(("vba-module", i))
                .mx(SPACE_1)
                .pl(SPACE_4)
                .pr(SPACE_2)
                .py(px(2.))
                .rounded(RADIUS_SM)
                .when(on, |r| r.bg(t_.selected))
                .when(!on, |r| r.hover(|r| r.bg(t_.hover)));
            let row = if renaming {
                let (input, _) = ed.renaming.as_ref().expect("renaming");
                row.child(div().flex_1().child(Input::new(input).xsmall()))
            } else {
                row.child(div().flex_1().min_w_0().truncate().child(label))
                    .on_click(move |_, w, cx| {
                        let _ = v.update(cx, |this, cx| this.macro_editor_select(i, w, cx));
                    })
            };
            tree = tree.child(row);
        }

        // The code.
        let body = match current {
            Some(m) => {
                let title = format!("{} · {}", m.module.name, kind_label(m.module.kind));
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(
                        div()
                            .flex_none()
                            .px(SPACE_3)
                            .py(SPACE_1)
                            .text_size(FONT_SMALL)
                            .text_color(t_.fg_muted)
                            .child(title),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .child(Editor::new(&m.state).bordered(false).h_full()),
                    )
                    .into_any_element()
            }
            None => div()
                .flex_1()
                .p(SPACE_4)
                .text_color(t_.fg_muted)
                .child(t("Insert a module to write a macro."))
                .into_any_element(),
        };

        // Problems of the current module, each a link to its line.
        let problems = current.filter(|m| !m.problems.is_empty()).map(|m| {
            let state = m.state.clone();
            v_flex()
                .id("vba-problems")
                .flex_none()
                .max_h(px(96.))
                .overflow_y_scroll()
                .px(SPACE_3)
                .py(SPACE_1)
                .border_t_1()
                .border_color(t_.border)
                .text_size(FONT_SMALL)
                .children(m.problems.iter().enumerate().map(|(i, p)| {
                    let state = state.clone();
                    let line = p.line;
                    let text = if p.unsupported {
                        tf(
                            "Line {}: Kantor doesn't run this yet: {}",
                            &[&p.line, &p.message],
                        )
                    } else {
                        tf("Line {}: {}", &[&p.line, &p.message])
                    };
                    div()
                        .id(("vba-problem", i))
                        .cursor_pointer()
                        .text_color(if p.fatal || !p.unsupported {
                            t_.danger
                        } else {
                            t_.warning
                        })
                        .hover(|d| d.underline())
                        .child(text)
                        .on_click(move |_, w, cx| {
                            state.update(cx, |s, cx| {
                                s.set_cursor_position(
                                    Position {
                                        line: line.saturating_sub(1),
                                        character: 0,
                                    },
                                    w,
                                    cx,
                                )
                            });
                        })
                }))
        });

        let note = if ed.removing {
            let name = current.map(|m| m.module.name.clone()).unwrap_or_default();
            let yes = view.clone();
            let no = view.clone();
            Some(
                h_flex()
                    .flex_none()
                    .gap(SPACE_3)
                    .px(SPACE_3)
                    .py(SPACE_2)
                    .border_t_1()
                    .border_color(t_.border)
                    .text_size(FONT_SMALL)
                    .child(
                        div()
                            .flex_1()
                            .child(tf("Remove {} and its code?", &[&name])),
                    )
                    .child(
                        Button::new("vba-remove-yes")
                            .label(t("Remove"))
                            .danger()
                            .xsmall()
                            .on_click(move |_, w, cx| {
                                let _ = yes.update(cx, |this, cx| this.macro_editor_remove(w, cx));
                            }),
                    )
                    .child(
                        Button::new("vba-remove-no")
                            .label(t("Cancel"))
                            .ghost()
                            .xsmall()
                            .on_click(move |_, _, cx| {
                                let _ = no.update(cx, |this, cx| {
                                    if let Some(ed) = this.macro_editor.as_mut() {
                                        ed.removing = false;
                                        cx.notify();
                                    }
                                });
                            }),
                    )
                    .into_any_element(),
            )
        } else {
            ed.note.as_ref().map(|(text, error)| {
                div()
                    .flex_none()
                    .px(SPACE_3)
                    .py(SPACE_2)
                    .border_t_1()
                    .border_color(t_.border)
                    .text_size(FONT_SMALL)
                    .text_color(if *error { t_.danger } else { t_.fg_muted })
                    .child(text.clone())
                    .into_any_element()
            })
        };

        Some(
            v_flex()
                .id("vba-editor")
                .key_context(CONTEXT)
                .flex_none()
                .w(relative(0.6))
                .h_full()
                .bg(t_.chrome)
                .border_l_1()
                .border_color(t_.border)
                .text_color(t_.fg)
                .on_action(cx.listener(|this, _: &RunMacroUnderCaret, w, cx| {
                    this.run_macro_under_caret(w, cx)
                }))
                .on_action(cx.listener(|this, _: &input::Escape, w, cx| {
                    // Escape in the rename box cancels it.
                    if let Some(ed) = this.macro_editor.as_mut()
                        && ed.renaming.take().is_some()
                    {
                        this.focus_macro_editor(w, cx);
                        cx.notify();
                    }
                }))
                .child(
                    div()
                        .flex_none()
                        .px(SPACE_3)
                        .py(SPACE_2)
                        .border_b_1()
                        .border_color(t_.border)
                        .child(toolbar),
                )
                .child(
                    h_flex()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .items_start()
                        .child(tree)
                        .child(body),
                )
                .children(problems)
                .children(note)
                .into_any_element(),
        )
    }
}

/// Shows `problems` (lines from 1) as the editor's diagnostics.
fn show_problems(state: &Entity<EditorState>, problems: &[Diagnostic], cx: &mut App) {
    state.update(cx, |s, cx| {
        let text = s.text().clone();
        let value = s.value().to_string();
        let lines: Vec<&str> = value.split('\n').collect();
        if let Some(set) = s.diagnostics_mut() {
            set.reset(&text);
            for p in problems {
                let line = (p.line.max(1) - 1) as usize;
                let Some(code) = lines.get(line) else {
                    continue;
                };
                let len = code.trim_end_matches('\r').encode_utf16().count() as u32;
                let range = Position {
                    line: line as u32,
                    character: 0,
                }..Position {
                    line: line as u32,
                    character: len.max(1),
                };
                let severity = if p.unsupported {
                    DiagnosticSeverity::Warning
                } else {
                    DiagnosticSeverity::Error
                };
                set.push(EditorProblem::new(range, p.message.clone()).with_severity(severity));
            }
        }
        cx.notify();
    });
}
