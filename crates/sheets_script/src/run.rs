//! Running a macro: a workbook's modules compiled into a VBA project, the
//! host (globals, constants, message boxes through the UI, `Debug.Print`,
//! cancellation), the run as one undo step, and errors as Excel words them.

use std::rc::Rc;

use kantor_script::forms::{Answer, FormDesign, FormInput, FormView};
use kantor_script::vba::{ModuleKind as ScriptKind, ProcInfo, ProcKind, Project};
use kantor_script::{CancelToken, ErrorKind, Host, ObjectRef, ScriptError, Value};
use sheets_engine::Workbook;
use sheets_engine::vba::{ModuleKind, VbaProject};

use crate::session::{Session, View};

/// One module of a workbook's VBA project, as the interpreter takes it.
#[derive(Clone, Debug)]
pub struct ModuleSource {
    pub name: String,
    pub kind: ModuleKind,
    /// The source with its `Attribute` lines (they carry default members).
    pub source: String,
    /// For each source line (0-based), the line the Visual Basic Editor
    /// shows it on (attribute lines hidden), 1-based.
    shown_line: Vec<u32>,
}

/// A workbook's macros, ready to compile. `Send`: it goes to whatever
/// thread runs the macro.
#[derive(Clone, Debug, Default)]
pub struct MacroProject {
    pub modules: Vec<ModuleSource>,
    /// The UserForms' designers (forms whose designer can't be read show
    /// empty).
    pub forms: Vec<FormDesign>,
}

impl MacroProject {
    /// From a workbook's VBA project (modules that couldn't be read are
    /// left out).
    pub fn from_vba(p: &VbaProject) -> MacroProject {
        let modules = p
            .modules()
            .iter()
            .filter(|m| m.error.is_none())
            .map(|m| ModuleSource::new(&m.name, m.kind, &m.source))
            .collect();
        MacroProject {
            modules,
            forms: p.forms(),
        }
    }

    /// The workbook's macros, `None` when it has none.
    pub fn of_book(book: &Workbook) -> Option<Result<MacroProject, String>> {
        Some(book.vba_project()?.map(|p| MacroProject::from_vba(&p)))
    }

    /// Compiles every module. A module whose structure doesn't parse is
    /// left out and its error returned (running a macro of it reports the
    /// error; the other modules still run).
    pub fn compile(&self) -> (Project, Vec<MacroError>) {
        let mut project = Project::new();
        let mut broken = Vec::new();
        for m in &self.modules {
            let kind = match m.kind {
                ModuleKind::Standard => ScriptKind::Standard,
                ModuleKind::Class => ScriptKind::Class,
                ModuleKind::Document => ScriptKind::Document,
                ModuleKind::Form => ScriptKind::Form,
            };
            if let Err(e) = project.add_module(&m.name, kind, &m.source) {
                broken.push(self.error(e));
            }
        }
        for (name, value) in crate::consts::ALL
            .iter()
            .chain(crate::charts::CHART_CONSTANTS)
            .chain(crate::cond_formats::CONSTANTS)
            .chain(crate::hyperlinks::CONSTANTS)
            .chain(crate::page_setup::CONSTANTS)
            .chain(crate::pivots::CONSTANTS)
            .chain(crate::tables::CONSTANTS)
            .chain(crate::validation::CONSTANTS.iter())
            .copied()
            .chain(crate::shapes::constants())
        {
            project.register_constant(name, Value::Long(value as i32));
        }
        for f in &self.forms {
            project.add_form(f.clone());
        }
        (project, broken)
    }

    /// Which events the macros handle (compiles the project).
    pub fn event_index(&self) -> crate::events::EventIndex {
        let (project, _) = self.compile();
        crate::events::EventIndex::new(self, &project)
    }

    /// What Excel lists under Macros (Alt+F8): public Subs without
    /// parameters, in standard and document modules.
    pub fn macros(&self) -> Vec<MacroInfo> {
        macro_list(&self.compile().0)
    }

    /// The error with its line as the editor shows it.
    pub fn error(&self, e: ScriptError) -> MacroError {
        let (module, procedure, line) = match &e.location {
            Some(loc) => {
                let shown = self
                    .modules
                    .iter()
                    .find(|m| m.name.eq_ignore_ascii_case(&loc.module))
                    .and_then(|m| {
                        m.shown_line
                            .get(loc.line.saturating_sub(1) as usize)
                            .copied()
                    })
                    .unwrap_or(loc.line);
                (loc.module.clone(), loc.procedure.clone(), shown)
            }
            None => (String::new(), String::new(), 0),
        };
        MacroError {
            number: e.number,
            description: e.description,
            kind: e.kind,
            module,
            procedure,
            line,
        }
    }
}

impl ModuleSource {
    pub fn new(name: &str, kind: ModuleKind, source: &str) -> ModuleSource {
        let mut shown_line = Vec::new();
        let mut shown = 0u32;
        for line in source.lines() {
            if !line.trim_start().starts_with("Attribute ") {
                shown += 1;
            }
            shown_line.push(shown.max(1));
        }
        ModuleSource {
            name: name.to_owned(),
            kind,
            source: source.to_owned(),
            shown_line,
        }
    }
}

/// A macro as the Macros dialog lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MacroInfo {
    pub module: String,
    pub name: String,
}

impl MacroInfo {
    /// `Module1.Macro1`.
    pub fn full_name(&self) -> String {
        format!("{}.{}", self.module, self.name)
    }
}

pub fn macro_list(p: &Project) -> Vec<MacroInfo> {
    p.procedures()
        .into_iter()
        .filter(|i: &ProcInfo| i.is_macro)
        .map(|i| MacroInfo {
            module: i.module,
            name: i.name,
        })
        .collect()
}

/// Whether `name` is a public Function of a standard module (a UDF).
pub fn is_udf(p: &Project, name: &str) -> bool {
    p.procedures()
        .iter()
        .any(|i| i.kind == ProcKind::Function && i.public && i.name.eq_ignore_ascii_case(name))
}

/// A macro's error, as Excel's run-time error dialog shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct MacroError {
    pub number: i32,
    pub description: String,
    pub kind: ErrorKind,
    pub module: String,
    pub procedure: String,
    /// The line in the module as the editor shows it (1-based; 0: unknown).
    pub line: u32,
}

impl MacroError {
    /// "Run-time error '1004':" or "Compile error:".
    pub fn title(&self) -> String {
        match self.kind {
            ErrorKind::Syntax => "Compile error:".to_owned(),
            ErrorKind::Interrupted if self.number == 18 => {
                "Code execution has been interrupted".to_owned()
            }
            _ => format!("Run-time error '{}':", self.number),
        }
    }

    /// "in Module1.Macro1, line 12".
    pub fn place(&self) -> String {
        match (self.module.is_empty(), self.procedure.is_empty(), self.line) {
            (true, _, _) => String::new(),
            (false, true, 0) => self.module.clone(),
            (false, true, l) => format!("{}, line {l}", self.module),
            (false, false, 0) => format!("{}.{}", self.module, self.procedure),
            (false, false, l) => format!("{}.{}, line {l}", self.module, self.procedure),
        }
    }
}

impl std::fmt::Display for MacroError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.title(), self.description)?;
        let place = self.place();
        if !place.is_empty() {
            write!(f, " (in {place})")?;
        }
        Ok(())
    }
}

/// A message box a macro shows.
#[derive(Clone, Debug, PartialEq)]
pub struct MsgBoxRequest {
    pub prompt: String,
    /// VBA's `Buttons` argument (`vbYesNo + vbQuestion`…).
    pub buttons: i32,
    pub title: String,
}

/// What `MsgBox` shows, from its `Buttons` argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MsgButtons {
    Ok,
    OkCancel,
    AbortRetryIgnore,
    YesNoCancel,
    YesNo,
    RetryCancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MsgIcon {
    None,
    Critical,
    Question,
    Exclamation,
    Information,
}

impl MsgBoxRequest {
    pub fn kind(&self) -> MsgButtons {
        match self.buttons & 0x7 {
            1 => MsgButtons::OkCancel,
            2 => MsgButtons::AbortRetryIgnore,
            3 => MsgButtons::YesNoCancel,
            4 => MsgButtons::YesNo,
            5 => MsgButtons::RetryCancel,
            _ => MsgButtons::Ok,
        }
    }

    pub fn icon(&self) -> MsgIcon {
        match self.buttons & 0x70 {
            0x10 => MsgIcon::Critical,
            0x20 => MsgIcon::Question,
            0x30 => MsgIcon::Exclamation,
            0x40 => MsgIcon::Information,
            _ => MsgIcon::None,
        }
    }

    /// The default button (0-based): `vbDefaultButton2` is 1.
    pub fn default_button(&self) -> usize {
        ((self.buttons & 0x300) >> 8) as usize
    }

    /// The buttons and what each returns (`vbOK` 1, `vbCancel` 2, `vbAbort`
    /// 3, `vbRetry` 4, `vbIgnore` 5, `vbYes` 6, `vbNo` 7).
    pub fn choices(&self) -> &'static [(&'static str, i32)] {
        match self.kind() {
            MsgButtons::Ok => &[("OK", 1)],
            MsgButtons::OkCancel => &[("OK", 1), ("Cancel", 2)],
            MsgButtons::AbortRetryIgnore => &[("Abort", 3), ("Retry", 4), ("Ignore", 5)],
            MsgButtons::YesNoCancel => &[("Yes", 6), ("No", 7), ("Cancel", 2)],
            MsgButtons::YesNo => &[("Yes", 6), ("No", 7)],
            MsgButtons::RetryCancel => &[("Retry", 4), ("Cancel", 2)],
        }
    }

    /// The answer when nobody can be asked (headless): OK, or Yes.
    pub fn headless_answer(&self) -> i32 {
        self.choices()[0].1
    }
}

/// What an input box asks for (`Application.InputBox`'s `Type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    /// VBA's `InputBox` function, or `Type:=2`.
    Text,
    /// `Type:=1`.
    Number,
    /// `Type:=4`.
    Boolean,
    /// `Type:=8`: a cell reference.
    Range,
    /// `Type:=0`: a formula.
    Formula,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InputBoxRequest {
    pub prompt: String,
    pub title: String,
    pub default: String,
    pub kind: InputKind,
}

/// Where a macro's dialogs and output go. The workbook is lent to the UI
/// while a dialog shows, so it can show the cells as the macro left them
/// (and must hand the same workbook back).
pub trait MacroUi {
    fn msg_box(
        &mut self,
        book: Workbook,
        view: View,
        req: &MsgBoxRequest,
    ) -> (Workbook, Result<i32, ScriptError>);

    /// `Ok(None)`: cancelled.
    fn input_box(
        &mut self,
        book: Workbook,
        view: View,
        req: &InputBoxRequest,
    ) -> (Workbook, Result<Option<String>, ScriptError>);

    /// One line of `Debug.Print` output.
    fn debug_print(&mut self, _line: &str) {}

    /// The other workbooks open in the window (for `Workbooks`).
    fn other_books(&mut self) -> Vec<OtherBook> {
        Vec::new()
    }

    /// Lends the workbook of tab `id` to the run (it comes back in
    /// [`RunOutcome::books`]); `None` when it can't be (busy, closed).
    fn lend_book(&mut self, _id: u64) -> Option<Workbook> {
        None
    }

    /// Whether the macro may open (`write`: save to) `path`, outside the
    /// workbook's folder. `None`: ask with a Yes/No message box. The
    /// workbook is lent while a dialog shows.
    fn allow_path(
        &mut self,
        book: Workbook,
        _view: View,
        _path: &std::path::Path,
        _write: bool,
    ) -> (Workbook, Option<bool>) {
        (book, None)
    }

    /// Shows (or updates) UserForm `form` and waits for the user's next
    /// step (see [`kantor_script::forms`]); lent the workbook meanwhile, as
    /// for a message box. The default has no window to show it in.
    fn form_event(
        &mut self,
        book: Workbook,
        _view: View,
        form: &FormView,
    ) -> (Workbook, Result<FormInput, ScriptError>) {
        let name = &form.design.name;
        (
            book,
            Err(ScriptError::new(
                445,
                format!(
                    "The macro shows the UserForm {name}, and there is no window to show it in"
                ),
            )),
        )
    }

    /// The form [`MacroUi::form_event`] was showing is closed.
    fn form_closed(&mut self, _name: &str) {}
}

/// Another workbook open in the window.
#[derive(Clone, Debug, PartialEq)]
pub struct OtherBook {
    /// The UI's id for its tab.
    pub id: u64,
    /// `Data.xlsx`.
    pub name: String,
    pub path: Option<std::path::PathBuf>,
}

impl Session {
    /// Shows a message box through the UI, lending it the workbook.
    pub fn msg_box(&self, req: &MsgBoxRequest) -> Result<i32, ScriptError> {
        let view = self.view();
        let book = self
            .take_book()
            .ok_or_else(|| crate::values::err1004("The workbook is not available"))?;
        let (book, answer) = self.ui.borrow_mut().msg_box(book, view, req);
        self.put_book(book);
        answer
    }

    /// Shows a UserForm through the UI, lending it the workbook.
    pub fn form_event(&self, form: &FormView) -> Result<FormInput, ScriptError> {
        let view = self.view();
        let book = self
            .take_book()
            .ok_or_else(|| crate::values::err1004("The workbook is not available"))?;
        let (book, answer) = self.ui.borrow_mut().form_event(book, view, form);
        self.put_book(book);
        answer
    }

    pub fn input_box(&self, req: &InputBoxRequest) -> Result<Option<String>, ScriptError> {
        let view = self.view();
        let book = self
            .take_book()
            .ok_or_else(|| crate::values::err1004("The workbook is not available"))?;
        let (book, answer) = self.ui.borrow_mut().input_box(book, view, req);
        self.put_book(book);
        answer
    }
}

/// The interpreter's host for a run.
pub(crate) struct SheetsHost {
    pub(crate) s: Rc<Session>,
    pub(crate) globals: ObjectRef,
    pub(crate) cancel: CancelToken,
}

impl Host for SheetsHost {
    fn globals(&mut self) -> Option<ObjectRef> {
        Some(self.globals.clone())
    }

    fn msg_box(&mut self, prompt: &str, buttons: i32, title: &str) -> Result<i32, ScriptError> {
        self.s.msg_box(&MsgBoxRequest {
            prompt: prompt.to_owned(),
            buttons,
            title: if title.is_empty() {
                "Microsoft Excel".to_owned()
            } else {
                title.to_owned()
            },
        })
    }

    fn input_box(
        &mut self,
        prompt: &str,
        title: &str,
        default: &str,
    ) -> Result<Option<String>, ScriptError> {
        self.s.input_box(&InputBoxRequest {
            prompt: prompt.to_owned(),
            title: if title.is_empty() {
                "Microsoft Excel".to_owned()
            } else {
                title.to_owned()
            },
            default: default.to_owned(),
            kind: InputKind::Text,
        })
    }

    fn debug_print(&mut self, line: &str) {
        self.s.ui.borrow_mut().debug_print(line);
    }

    fn form_event(&mut self, view: &FormView) -> Result<FormInput, ScriptError> {
        self.s.form_event(view)
    }

    fn form_closed(&mut self, name: &str) {
        self.s.ui.borrow_mut().form_closed(name);
    }

    fn now(&mut self) -> f64 {
        local_now()
    }

    fn cancelled(&mut self) -> bool {
        self.cancel.is_cancelled()
    }

    fn take_calls(&mut self) -> Vec<kantor_script::HostCall> {
        crate::events::poll(&self.s)
    }
}

/// The local time as an OLE date (the UTC offset from the `TZ`-aware clock
/// isn't available without a time zone library: UTC plus the offset the
/// host set with [`set_utc_offset_minutes`]).
pub fn local_now() -> f64 {
    let offset = UTC_OFFSET.load(std::sync::atomic::Ordering::Relaxed);
    kantor_script::date::now_utc() + f64::from(offset) / 1440.0
}

static UTC_OFFSET: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// The local time zone's offset from UTC, for `Now`, `Date` and `Time`.
pub fn set_utc_offset_minutes(minutes: i32) {
    UTC_OFFSET.store(minutes, std::sync::atomic::Ordering::Relaxed);
}

/// How a run goes.
#[derive(Clone)]
pub struct RunOptions {
    /// Stop after this many statements (error 18). `None`: no limit (the
    /// cancel token stops it).
    pub step_limit: Option<u64>,
    pub cancel: CancelToken,
}

impl Default for RunOptions {
    fn default() -> RunOptions {
        RunOptions {
            step_limit: None,
            cancel: CancelToken::new(),
        }
    }
}

/// How a run ended.
#[derive(Debug)]
pub struct RunOutcome {
    pub result: Result<(), MacroError>,
    /// Where the macro left the grid.
    pub view: View,
    /// `ThisWorkbook.Save` was called: the caller saves.
    pub save_requested: bool,
    /// The macro changed the workbook (an undo step was made).
    pub changed: bool,
    /// An event handler set `Cancel` (the double-click, save or close
    /// doesn't happen).
    pub cancel: bool,
    /// Other workbooks: lent ones going back, new ones to show.
    pub books: Vec<crate::books::BookOut>,
    /// `ThisWorkbook.Close` was called: `Some(SaveChanges)`.
    pub close_requested: Option<Option<bool>>,
}

impl RunOutcome {
    /// A run that failed before it started.
    pub fn failed(e: MacroError, view: View) -> RunOutcome {
        RunOutcome {
            result: Err(e),
            ..RunOutcome::nothing(view)
        }
    }

    /// Nothing ran.
    pub fn nothing(view: View) -> RunOutcome {
        RunOutcome {
            result: Ok(()),
            view,
            save_requested: false,
            changed: false,
            cancel: false,
            books: Vec::new(),
            close_requested: None,
        }
    }
}

impl std::fmt::Debug for crate::books::BookOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            crate::books::BookOut::Returned { id, .. } => write!(f, "Returned({id})"),
            crate::books::BookOut::Closed { id, .. } => write!(f, "Closed({id})"),
            crate::books::BookOut::New { book } => write!(f, "New({})", book.title()),
        }
    }
}

/// Runs macro `name` (`Macro1` or `Module1.Macro1`) of `macros` on `book`,
/// starting from `view`. Every change is one undo step; recalculation is
/// deferred while the macro writes and brought up to date before it reads.
/// The workbook comes back whatever happens.
pub fn run_macro(
    book: Workbook,
    macros: &MacroProject,
    project: &mut Project,
    name: &str,
    view: View,
    ui: Box<dyn MacroUi>,
    options: &RunOptions,
) -> (Workbook, RunOutcome) {
    let mut state = crate::engine::State::default();
    crate::engine::run_with(
        project,
        macros,
        &mut state,
        None,
        book,
        view,
        ui,
        options,
        None,
        |project, host, _| project.run(name, &[], host).map(|_| false),
    )
}

/// When the file has no code names (a new workbook): `Sheet1`… document
/// modules map to the sheets in order.
pub(crate) fn fallback_code_names(s: &Rc<Session>, macros: &MacroProject) {
    let Ok(book) = s.read() else {
        return;
    };
    let ids: Vec<u32> = (0..book.sheet_names().len() as u32)
        .map(|i| book.sheet_id(i))
        .collect();
    drop(book);
    let docs = macros
        .modules
        .iter()
        .filter(|m| m.kind == ModuleKind::Document && !m.name.eq_ignore_ascii_case("ThisWorkbook"));
    let mut codes = s.code_names.borrow_mut();
    for (m, id) in docs.zip(ids) {
        codes.push((m.name.clone(), id));
    }
}

/// One step on a UserForm when nobody can fill it in: values to put in
/// controls (by name), then a control to click (`"close"`: the close box).
#[derive(Clone, Debug, PartialEq)]
pub struct FormAnswer {
    pub set: Vec<(String, Answer)>,
    pub click: Option<String>,
}

/// A UI with nobody to ask (the command line, tests): message boxes print
/// to stderr and answer OK or Yes; input boxes take the given answers in
/// order and fail when there are none left; UserForms take the steps in
/// [`HeadlessUi::forms`] and fail when there are none left.
#[derive(Clone)]
pub struct HeadlessUi {
    /// Answers for input boxes, shared by clones (one per run).
    pub inputs: Rc<std::cell::RefCell<std::collections::VecDeque<String>>>,
    /// Steps for each form (by name, lowercase), in order; shared by clones.
    pub forms: Rc<
        std::cell::RefCell<
            std::collections::HashMap<String, std::collections::VecDeque<FormAnswer>>,
        >,
    >,
    /// The forms shown (each step's view), in order.
    pub form_views: Rc<std::cell::RefCell<Vec<FormView>>>,
    /// Message boxes shown, in order.
    pub shown: Rc<std::cell::RefCell<Vec<String>>>,
    /// `Debug.Print` lines.
    pub printed: Rc<std::cell::RefCell<Vec<String>>>,
    /// Print message boxes and `Debug.Print` to stderr.
    pub echo: bool,
    /// Files and folders macros may open and save (`--allow-path`); any
    /// other path is refused.
    pub allowed_paths: Vec<std::path::PathBuf>,
}

impl HeadlessUi {
    pub fn new(inputs: Vec<String>, echo: bool) -> HeadlessUi {
        HeadlessUi {
            inputs: Rc::new(std::cell::RefCell::new(inputs.into())),
            forms: Rc::default(),
            form_views: Rc::default(),
            shown: Rc::default(),
            printed: Rc::default(),
            echo,
            allowed_paths: Vec::new(),
        }
    }

    /// Adds steps for form `name`.
    pub fn with_form_answers(mut self, name: &str, steps: Vec<FormAnswer>) -> HeadlessUi {
        self.forms
            .borrow_mut()
            .entry(name.to_lowercase())
            .or_default()
            .extend(steps);
        self
    }
}

impl MacroUi for HeadlessUi {
    fn msg_box(
        &mut self,
        book: Workbook,
        _view: View,
        req: &MsgBoxRequest,
    ) -> (Workbook, Result<i32, ScriptError>) {
        if self.echo {
            eprintln!("[MsgBox] {}: {}", req.title, req.prompt);
        }
        self.shown.borrow_mut().push(req.prompt.clone());
        (book, Ok(req.headless_answer()))
    }

    fn input_box(
        &mut self,
        book: Workbook,
        _view: View,
        req: &InputBoxRequest,
    ) -> (Workbook, Result<Option<String>, ScriptError>) {
        let next = self.inputs.borrow_mut().pop_front();
        let answer = match next {
            Some(a) => {
                if self.echo {
                    eprintln!("[InputBox] {}: {a}", req.prompt);
                }
                Ok(Some(a))
            }
            None => Err(ScriptError::new(
                5,
                format!(
                    "The macro asked for input (\"{}\") and no --input value was left",
                    req.prompt
                ),
            )),
        };
        (book, answer)
    }

    fn debug_print(&mut self, line: &str) {
        if self.echo {
            eprintln!("{line}");
        }
        self.printed.borrow_mut().push(line.to_owned());
    }

    fn allow_path(
        &mut self,
        book: Workbook,
        _view: View,
        path: &std::path::Path,
        _write: bool,
    ) -> (Workbook, Option<bool>) {
        let ok = self.allowed_paths.iter().any(|a| path.starts_with(a));
        (book, Some(ok))
    }

    fn form_event(
        &mut self,
        book: Workbook,
        _view: View,
        form: &FormView,
    ) -> (Workbook, Result<FormInput, ScriptError>) {
        self.form_views.borrow_mut().push(form.clone());
        let name = &form.design.name;
        let step = self
            .forms
            .borrow_mut()
            .get_mut(&name.to_lowercase())
            .and_then(|q| q.pop_front());
        let answer = match step {
            Some(step) => {
                if self.echo {
                    eprintln!("[UserForm] {name}: {step:?}");
                }
                kantor_script::forms::scripted_input(form, &step.set, step.click.as_deref())
                    .map_err(|e| ScriptError::new(5, e))
            }
            None => Err(ScriptError::new(
                445,
                format!(
                    "The macro shows the UserForm {name} and there is no window to show it in \
                     (give its answers with --form-answers)"
                ),
            )),
        };
        (book, answer)
    }
}

/// Runs `name` headless: compiles `macros`, runs, returns the workbook.
pub fn run_headless(
    book: Workbook,
    macros: &MacroProject,
    name: &str,
    view: View,
    ui: HeadlessUi,
    options: &RunOptions,
) -> (Workbook, RunOutcome) {
    let mut engine = crate::engine::MacroEngine::new(macros.clone());
    engine.set_paths(ui.allowed_paths.clone(), false);
    engine.run(book, name, view, Box::new(ui), options)
}

/// The compile error of the module macro `name` is in, if it didn't
/// compile.
pub fn broken_for(broken: &[MacroError], macros: &MacroProject, name: &str) -> Option<MacroError> {
    let module = match name.rsplit_once('.') {
        Some((m, _)) => Some(m.to_owned()),
        // A bare name: the broken module whose text defines it.
        None => macros
            .modules
            .iter()
            .find(|m| {
                m.source.lines().any(|l| {
                    let l = l.trim_start().to_ascii_lowercase();
                    let l = l.strip_prefix("public ").unwrap_or(&l);
                    l.strip_prefix("sub ")
                        .is_some_and(|r| r.trim_start().starts_with(&name.to_ascii_lowercase()))
                })
            })
            .map(|m| m.name.clone()),
    }?;
    broken
        .iter()
        .find(|e| e.module.eq_ignore_ascii_case(&module))
        .cloned()
}
