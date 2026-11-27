//! A workbook's live VBA project: [`MacroEngine`].
//!
//! While content is enabled, a workbook keeps one compiled project, so
//! module-level and global variables keep their values between macro runs
//! and event handlers, as in Excel. The project is reset (variables
//! cleared) by an `End` statement, by a run that ends with an unhandled
//! error (the error dialog's End), and it goes away with the engine (content
//! disabled, the workbook closed).
//!
//! The interpreter is built on `Rc` and isn't `Send`: an engine lives on one
//! thread for its whole life. The desktop keeps a macro thread per workbook
//! (`sheets_ui::macro_run`) that owns its engine and is handed the workbook
//! for each run or event; the command line and the tests use one on their
//! own thread.
//!
//! Besides the project, the engine keeps what Excel's application keeps
//! between runs: `Application.EnableEvents` (a macro that turns it off and
//! ends leaves events off, as in Excel), `Application.OnTime` schedules, and
//! the file paths the user allowed macros to use.

use std::path::PathBuf;
use std::rc::Rc;

use kantor_script::vba::Project;
use kantor_script::{ObjectRef, ScriptError, Value};
use sheets_engine::Workbook;

use crate::app::Globals;
use crate::books::App;
use crate::events::{self, Event, EventIndex};
use crate::run::{
    MacroError, MacroProject, MacroUi, RunOptions, RunOutcome, SheetsHost, broken_for,
    fallback_code_names,
};
use crate::session::{Session, View};

/// An `Application.OnTime` schedule.
#[derive(Clone, Debug, PartialEq)]
pub struct Timer {
    /// When (local time, an OLE date).
    pub at: f64,
    /// The macro (`Module1.Tick`).
    pub name: String,
    /// `LatestTime`: past it the run is skipped.
    pub latest: Option<f64>,
}

/// What lasts between runs besides the project.
#[derive(Default)]
pub(crate) struct State {
    /// The session runs reuse (objects kept in module variables stay
    /// valid; `Application` settings last).
    pub session: Option<Rc<Session>>,
    pub timers: Vec<Timer>,
    pub allowed: Vec<PathBuf>,
    /// Paths in the workbook's folder need no asking.
    pub own_folder_asks: bool,
}

/// A workbook's macros with their live project (see the module docs).
pub struct MacroEngine {
    macros: MacroProject,
    project: Project,
    broken: Vec<MacroError>,
    index: Rc<EventIndex>,
    state: State,
}

impl MacroEngine {
    pub fn new(macros: MacroProject) -> MacroEngine {
        let (project, broken) = macros.compile();
        let index = Rc::new(EventIndex::new(&macros, &project));
        MacroEngine {
            macros,
            project,
            broken,
            index,
            state: State::default(),
        }
    }

    pub fn macros(&self) -> &MacroProject {
        &self.macros
    }

    /// Which events the project handles.
    pub fn index(&self) -> &EventIndex {
        &self.index
    }

    /// Paths macros may use without asking (the command line's
    /// `--allow-path`), and whether the workbook's own folder needs no
    /// asking (the desktop: yes; headless: no).
    pub fn set_paths(&mut self, allowed: Vec<PathBuf>, own_folder: bool) {
        self.state.allowed = allowed;
        self.state.own_folder_asks = !own_folder;
    }

    /// `Application.EnableEvents` as the last run left it.
    pub fn events_enabled(&self) -> bool {
        self.state
            .session
            .as_ref()
            .is_none_or(|s| s.settings.borrow().enable_events)
    }

    /// Clears module-level variables (the editor's Reset button).
    pub fn reset(&mut self) {
        self.project.reset();
    }

    /// The pending `Application.OnTime` schedules, earliest first.
    pub fn timers(&self) -> &[Timer] {
        &self.state.timers
    }

    /// Takes the first schedule due at `now` (local OLE date): the macro to
    /// run. Schedules past their `LatestTime` are dropped.
    pub fn due(&mut self, now: f64) -> Option<String> {
        self.state
            .timers
            .retain(|t| t.latest.is_none_or(|l| now <= l));
        let i = self.state.timers.iter().position(|t| t.at <= now)?;
        Some(self.state.timers.remove(i).name)
    }

    /// Runs macro `name` (`Macro1`, `Module1.Macro1`).
    pub fn run(
        &mut self,
        book: Workbook,
        name: &str,
        view: View,
        ui: Box<dyn MacroUi>,
        options: &RunOptions,
    ) -> (Workbook, RunOutcome) {
        if let Some(e) = broken_for(&self.broken, &self.macros, name) {
            return (book, RunOutcome::failed(e, view));
        }
        let index = Some(self.index.clone());
        run_with(
            &mut self.project,
            &self.macros,
            &mut self.state,
            index,
            book,
            view,
            ui,
            options,
            None,
            |project, host, _| project.run(name, &[], host).map(|_| false),
        )
    }

    /// Fires `event`: its handlers run in Excel's order (sheet module,
    /// `ThisWorkbook`, `Application` `WithEvents` variables) and
    /// `RunOutcome::cancel` says whether one set `Cancel`. With `fold_from`
    /// (an undo mark taken before the user's edit), the edit and what the
    /// handlers change undo as one step. Nothing runs when events are off or
    /// no handler exists.
    pub fn fire(
        &mut self,
        book: Workbook,
        event: &Event,
        view: View,
        ui: Box<dyn MacroUi>,
        options: &RunOptions,
        fold_from: Option<usize>,
    ) -> (Workbook, RunOutcome) {
        if !self.events_enabled() || !self.index.wants(event.kind()) {
            return (book, RunOutcome::nothing(view));
        }
        let index = self.index.clone();
        run_with(
            &mut self.project,
            &self.macros,
            &mut self.state,
            Some(index.clone()),
            book,
            view,
            ui,
            options,
            fold_from,
            |project, host, s| {
                let (handlers, app) = events::handlers(s, &index, event);
                let mut cancel = false;
                for mut h in handlers {
                    if let Some(c) = h.cancel {
                        h.args[c] = Value::Boolean(cancel);
                    }
                    project.run_by_ref(&h.name, &mut h.args, host)?;
                    if let Some(c) = h.cancel {
                        cancel = truthy(&h.args[c]);
                    }
                }
                if index.app_handles(app.event) {
                    let mut args = app.args;
                    if let Some(c) = app.cancel {
                        args[c] = Value::Boolean(cancel);
                    }
                    project.raise_event("Application", app.event, &mut args, host)?;
                    if let Some(c) = app.cancel {
                        cancel = truthy(&args[c]);
                    }
                }
                Ok(cancel)
            },
        )
    }
}

fn truthy(v: &Value) -> bool {
    kantor_script::vba::convert::to_bool(v).unwrap_or(false)
}

/// One run on `book`: the session, the host, the run as one undo step
/// (from `fold_from`, else from the start), and what lasts afterwards.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_with(
    project: &mut Project,
    macros: &MacroProject,
    state: &mut State,
    index: Option<Rc<EventIndex>>,
    book: Workbook,
    view: View,
    ui: Box<dyn MacroUi>,
    options: &RunOptions,
    fold_from: Option<usize>,
    body: impl FnOnce(&mut Project, &mut SheetsHost, &Rc<Session>) -> Result<bool, ScriptError>,
) -> (Workbook, RunOutcome) {
    let mut book = book;
    let start = book.undo_mark();
    let mark = fold_from.unwrap_or(start).min(start);
    book.script_begin();
    let s = match &state.session {
        Some(s) => {
            s.reload(book, view, ui);
            s.clone()
        }
        None => {
            let s = Rc::new(Session::new(book, view, ui));
            state.session = Some(s.clone());
            s
        }
    };
    if s.code_names.borrow().is_empty() {
        fallback_code_names(&s, macros);
    }
    *s.cancel.borrow_mut() = options.cancel.clone();
    s.run_mark.set(start);
    let app = App::install(
        &s,
        std::mem::take(&mut state.allowed),
        !state.own_folder_asks,
    );
    *app.timers.borrow_mut() = std::mem::take(&mut state.timers);
    events::start(&s, index);
    let globals: ObjectRef = Rc::new(Globals { s: s.clone() });
    let mut host = SheetsHost {
        s: s.clone(),
        globals,
        cancel: options.cancel.clone(),
    };
    project.set_step_limit(options.step_limit);
    let result = body(project, &mut host, &s);
    drop(host);
    if result.is_err() {
        // The error dialog's End: module variables start over.
        project.reset();
    }
    let cancel = matches!(result, Ok(true));
    let result = result.map(|_| ()).map_err(|e| macros.error(e));
    state.allowed = std::mem::take(&mut *app.allowed.borrow_mut());
    let mut timers = std::mem::take(&mut *app.timers.borrow_mut());
    timers.sort_by(|a, b| a.at.total_cmp(&b.at));
    state.timers = timers;
    let books = app.finish();
    s.events.borrow_mut().index = None;
    let view = s.view();
    let save_requested = s.save_requested.get();
    let close_requested = s.close_requested.get();
    let mut book = match s.take_book() {
        Some(b) => b,
        None => unreachable!("the workbook is always handed back"),
    };
    book.script_end();
    book.fold_undo(mark);
    let changed = book.undo_mark() > mark || book.undo_mark() != start;
    (
        book,
        RunOutcome {
            result,
            view,
            save_requested,
            changed,
            cancel,
            books,
            close_requested,
        },
    )
}
