//! Other workbooks: the `Workbooks` collection, `Workbooks.Open` / `Add`,
//! `Workbook.Save` / `SaveAs` / `Close` / `Activate`, and `ThisWorkbook`
//! (the macro's own) versus `ActiveWorkbook`.
//!
//! Every workbook a run touches has a [`Session`] of its own, so a `Range`
//! of another workbook (`Workbooks("Data.xlsx").Sheets(1).Range("A1")`) is
//! the same object model over a different workbook. The sessions share an
//! [`App`]: the list of workbooks and which one is active; unqualified
//! `Range`, `Cells` and `ActiveSheet` follow the active workbook, as in
//! Excel.
//!
//! - Workbooks open in other tabs of the window are listed by the UI
//!   ([`crate::MacroUi::other_books`]) and lent to the run when a macro
//!   first touches one ([`crate::MacroUi::lend_book`]); they go back when
//!   the run ends ([`BookOut`]).
//! - `Workbooks.Open` reads the file on the run's thread; `Workbooks.Add`
//!   makes a new workbook. Both come out of the run as [`BookOut::New`], for
//!   the UI to show in new tabs (the command line drops them).
//! - **Paths.** A macro must not read or write files the user didn't mean it
//!   to. `Open`, `SaveAs` and `SaveCopyAs` of a path in the workbook's own
//!   folder go ahead on the desktop; any other path is asked about once per
//!   path for the engine's life ([`crate::MacroUi::allow_path`], a Yes/No
//!   message box unless the UI answers itself). Headless (the command line,
//!   tests) only paths given with `--allow-path` (a file, or a folder and
//!   everything in it) work. A refused path fails with error 1004.

use std::cell::{Cell, RefCell};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use kantor_script::{Arg, Dispatch, ScriptError, Value};
use sheets_engine::Workbook;

use crate::run::{MsgBoxRequest, OtherBook};
use crate::session::{Session, View};
use crate::values::*;

/// Where a workbook of the run came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The workbook whose macro runs.
    This,
    /// Lent by the tab with this id.
    Lent(u64),
    /// `Workbooks.Open`.
    Opened,
    /// `Workbooks.Add`.
    Added,
}

pub struct Book {
    pub s: Rc<Session>,
    pub origin: Origin,
    pub closed: bool,
}

/// What a run hands back besides its own workbook.
pub enum BookOut {
    /// A lent workbook, back to its tab.
    Returned { id: u64, book: Workbook },
    /// A lent workbook the macro closed: its tab closes.
    Closed { id: u64, book: Workbook },
    /// A workbook the macro opened or added: a new tab.
    New { book: Workbook },
}

/// The workbooks of a run.
pub struct App {
    pub books: RefCell<Vec<Book>>,
    /// The active workbook (index into `books`).
    pub active: Cell<usize>,
    /// Paths the user allowed (kept for the engine's life).
    pub allowed: RefCell<Vec<PathBuf>>,
    /// Paths in the workbook's own folder need no asking (the desktop).
    pub own_folder: bool,
    /// The other open workbooks, listed by the UI when first needed.
    others: RefCell<Option<Vec<OtherBook>>>,
    /// `Application.OnTime` schedules (kept for the engine's life).
    pub timers: RefCell<Vec<crate::engine::Timer>>,
}

impl App {
    /// The run's app over `this` (the workbook whose macros run).
    pub fn install(this: &Rc<Session>, allowed: Vec<PathBuf>, own_folder: bool) -> Rc<App> {
        let app = Rc::new(App {
            books: RefCell::new(vec![Book {
                s: this.clone(),
                origin: Origin::This,
                closed: false,
            }]),
            active: Cell::new(0),
            allowed: RefCell::new(allowed),
            own_folder,
            others: RefCell::new(None),
            timers: RefCell::new(Vec::new()),
        });
        *this.app.borrow_mut() = Some(app.clone());
        app
    }

    /// Takes every workbook but the macro's own out, for the caller to
    /// hand back or show; breaks the sessions' links to the app.
    pub fn finish(&self) -> Vec<BookOut> {
        let books = std::mem::take(&mut *self.books.borrow_mut());
        let mut out = Vec::new();
        for b in books {
            *b.s.app.borrow_mut() = None;
            if b.origin == Origin::This {
                continue;
            }
            let Some(book) = b.s.take_book() else {
                continue;
            };
            match (b.origin, b.closed) {
                (Origin::Lent(id), false) => out.push(BookOut::Returned { id, book }),
                (Origin::Lent(id), true) => out.push(BookOut::Closed { id, book }),
                (_, false) => out.push(BookOut::New { book }),
                (_, true) => {}
            }
        }
        out
    }

    fn this(&self) -> Option<Rc<Session>> {
        self.books.borrow().first().map(|b| b.s.clone())
    }

    fn position(&self, s: &Rc<Session>) -> Option<usize> {
        self.books.borrow().iter().position(|b| Rc::ptr_eq(&b.s, s))
    }

    fn add(&self, s: Rc<Session>, origin: Origin) -> usize {
        let mut books = self.books.borrow_mut();
        books.push(Book {
            s,
            origin,
            closed: false,
        });
        books.len() - 1
    }
}

/// The app of `s`'s run, if it has one.
fn app_of(s: &Rc<Session>) -> Option<Rc<App>> {
    s.app.borrow().clone()
}

/// The active workbook's session (unqualified `Range`, `ActiveSheet`).
pub fn active(s: &Rc<Session>) -> Rc<Session> {
    let Some(app) = app_of(s) else {
        return s.clone();
    };
    let books = app.books.borrow();
    match books.get(app.active.get()) {
        Some(b) if !b.closed => b.s.clone(),
        _ => books.first().map_or_else(|| s.clone(), |b| b.s.clone()),
    }
}

/// The macro's own workbook (`ThisWorkbook`, code names like `Sheet1`).
pub fn this(s: &Rc<Session>) -> Rc<Session> {
    app_of(s)
        .and_then(|a| a.this())
        .unwrap_or_else(|| s.clone())
}

/// A new session for a workbook of the run (no dialogs of its own, no
/// events: its macros don't run).
fn session_for(book: Workbook, app: &Rc<App>) -> Rc<Session> {
    let sheet = book.active_sheet();
    let view = View {
        sheet,
        ..View::default()
    };
    let s = Rc::new(Session::new(
        book,
        view,
        Box::new(crate::run::HeadlessUi::new(Vec::new(), false)),
    ));
    *s.app.borrow_mut() = Some(app.clone());
    s
}

/// The workbooks in `Workbooks` order: the run's, then the other tabs not
/// lent yet (as `(None, other)`).
fn listing(s: &Rc<Session>) -> Vec<(Option<Rc<Session>>, Option<OtherBook>)> {
    let Some(app) = app_of(s) else {
        return vec![(Some(s.clone()), None)];
    };
    if app.others.borrow().is_none() {
        let this = app.this().unwrap_or_else(|| s.clone());
        let others = this.ui.borrow_mut().other_books();
        *app.others.borrow_mut() = Some(others);
    }
    let mut out: Vec<(Option<Rc<Session>>, Option<OtherBook>)> = app
        .books
        .borrow()
        .iter()
        .filter(|b| !b.closed)
        .map(|b| (Some(b.s.clone()), None))
        .collect();
    let lent: Vec<u64> = app
        .books
        .borrow()
        .iter()
        .filter_map(|b| match b.origin {
            Origin::Lent(id) => Some(id),
            _ => None,
        })
        .collect();
    for o in app.others.borrow().iter().flatten() {
        if !lent.contains(&o.id) {
            out.push((None, Some(o.clone())));
        }
    }
    out
}

/// A workbook of the listing as a session (borrowing it from its tab).
fn materialize(
    s: &Rc<Session>,
    entry: (Option<Rc<Session>>, Option<OtherBook>),
) -> Result<Rc<Session>, ScriptError> {
    match entry {
        (Some(session), _) => Ok(session),
        (None, Some(other)) => {
            let app = app_of(s).ok_or_else(|| ScriptError::runtime(9))?;
            let this = app.this().unwrap_or_else(|| s.clone());
            let book = this.ui.borrow_mut().lend_book(other.id).ok_or_else(|| {
                err1004(format!(
                    "The workbook {} is busy and can't be used by the macro",
                    other.name
                ))
            })?;
            let session = session_for(book, &app);
            *session.file_name.borrow_mut() = other.name.clone();
            if let Some(p) = &other.path {
                session.set_file(p);
            }
            app.add(session.clone(), Origin::Lent(other.id));
            Ok(session)
        }
        (None, None) => Err(ScriptError::runtime(9)),
    }
}

fn same_name(wanted: &str, name: &str) -> bool {
    let stem = |n: &str| {
        Path::new(n)
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    };
    wanted.eq_ignore_ascii_case(name)
        || (Path::new(wanted).extension().is_none() && stem(wanted) == stem(name))
}

/// `Workbooks`.
pub struct WorkbooksObj {
    pub s: Rc<Session>,
}

impl Dispatch for WorkbooksObj {
    fn type_name(&self) -> String {
        "Workbooks".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "count" => Ok(Value::Long(listing(&self.s).len() as i32)),
            "" | "item" => {
                let key = scalar(need(args, 0, "Index")?)?;
                let list = listing(&self.s);
                let entry = match &key {
                    Value::String(n) => list.into_iter().find(|(s, o)| match (s, o) {
                        (Some(s), _) => same_name(n, &s.file_name.borrow()),
                        (None, Some(o)) => same_name(n, &o.name),
                        _ => false,
                    }),
                    v => {
                        let i = to_i64(v)?;
                        usize::try_from(i - 1)
                            .ok()
                            .and_then(|i| list.into_iter().nth(i))
                    }
                };
                let entry = entry.ok_or_else(|| ScriptError::runtime(9))?;
                let session = materialize(&self.s, entry)?;
                Ok(obj(Rc::new(crate::app::WorkbookObj::new(&session))))
            }
            "open" => open(&self.s, args),
            "add" => add(&self.s),
            "close" => {
                if let Some(app) = app_of(&self.s) {
                    for b in app.books.borrow_mut().iter_mut() {
                        if b.origin != Origin::This {
                            b.closed = true;
                        }
                    }
                    app.active.set(0);
                }
                Ok(Value::Empty)
            }
            "opentext" | "openxml" | "opendatabase" => {
                Err(ScriptError::unsupported(&format!("Workbooks.{name}")))
            }
            "application" | "parent" => Ok(obj(Rc::new(crate::app::Application::new(&self.s)))),
            _ => Err(ScriptError::no_member("Workbooks", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        let mut out = Vec::new();
        for entry in listing(&self.s) {
            let session = materialize(&self.s, entry)?;
            out.push(obj(Rc::new(crate::app::WorkbookObj::new(&session))));
        }
        Ok(out)
    }
}

/// A path as given, made absolute against the macro's workbook's folder,
/// with `.` and `..` worked out.
fn absolute(s: &Rc<Session>, path: &str) -> PathBuf {
    let p = Path::new(path);
    let full = if p.is_absolute() {
        p.to_path_buf()
    } else {
        let folder = this(s).folder.borrow().clone();
        if folder.is_empty() {
            std::env::current_dir().unwrap_or_default().join(p)
        } else {
            Path::new(&folder).join(p)
        }
    };
    let mut out = PathBuf::new();
    for c in full.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c.as_os_str()),
        }
    }
    out
}

/// May the macro use `path`? (See the module docs.) Asks the user at most
/// once per path.
pub fn check_path(s: &Rc<Session>, path: &Path, write: bool) -> Result<(), ScriptError> {
    let refused = || {
        err1004(format!(
            "Kantor didn't let the macro {} {}",
            if write { "write" } else { "open" },
            path.display()
        ))
    };
    let Some(app) = app_of(s) else {
        return Err(refused());
    };
    if app.allowed.borrow().iter().any(|a| path.starts_with(a)) {
        return Ok(());
    }
    let this = this(s);
    let folder = this.folder.borrow().clone();
    if app.own_folder && !folder.is_empty() && path.parent() == Some(Path::new(&folder)) {
        return Ok(());
    }
    let view = this.view();
    let Some(book) = this.take_book() else {
        return Err(refused());
    };
    let (book, answer) = this.ui.borrow_mut().allow_path(book, view, path, write);
    this.put_book(book);
    let allowed = match answer {
        Some(a) => a,
        None => {
            let verb = if write { "save" } else { "open" };
            let answer = this.msg_box(&MsgBoxRequest {
                prompt: format!(
                    "The macro wants to {verb} {}, outside the workbook's folder. Allow it?",
                    path.display()
                ),
                // vbYesNo + vbExclamation + vbDefaultButton2
                buttons: 4 + 48 + 256,
                title: "Kantor".to_owned(),
            })?;
            answer == 6
        }
    };
    if !allowed {
        return Err(refused());
    }
    app.allowed.borrow_mut().push(path.to_path_buf());
    Ok(())
}

/// `Workbooks.Open(Filename)`.
fn open(s: &Rc<Session>, args: &[Arg]) -> Result<Value, ScriptError> {
    let file = to_str(need(args, 0, "Filename")?)?;
    let path = absolute(s, &file);
    let app = app_of(s).ok_or_else(|| ScriptError::unsupported("Workbooks.Open here"))?;
    // Already open: that workbook (Excel activates it).
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    for entry in listing(s) {
        let same = match &entry {
            (Some(session), _) => *session.full_name.borrow() == path.display().to_string(),
            (None, Some(o)) => o.path.as_deref() == Some(path.as_path()),
            _ => false,
        };
        if same {
            let session = materialize(s, entry)?;
            if let Some(i) = app.position(&session) {
                app.active.set(i);
            }
            return Ok(obj(Rc::new(crate::app::WorkbookObj::new(&session))));
        }
    }
    check_path(s, &path, false)?;
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if !matches!(
        ext.as_str(),
        "xlsx" | "xlsm" | "xls" | "csv" | "tsv" | "xlsb" | "ods" | "txt"
    ) {
        return Err(err1004(format!(
            "Kantor can't open '{name}' from a macro (not a workbook)"
        )));
    }
    if !path.is_file() {
        return Err(err1004(format!(
            "Sorry, we couldn't find {}. Is it possible it was moved, renamed or deleted?",
            path.display()
        )));
    }
    let book = Workbook::open(&path).map_err(|e| err1004(format!("Opening {name} failed: {e}")))?;
    let session = session_for(book, &app);
    session.set_file(&path);
    let i = app.add(session.clone(), Origin::Opened);
    app.active.set(i);
    Ok(obj(Rc::new(crate::app::WorkbookObj::new(&session))))
}

/// `Workbooks.Add`: a new workbook, active.
fn add(s: &Rc<Session>) -> Result<Value, ScriptError> {
    let app = app_of(s).ok_or_else(|| ScriptError::unsupported("Workbooks.Add here"))?;
    let n = app
        .books
        .borrow()
        .iter()
        .filter(|b| b.origin == Origin::Added)
        .count();
    let book = Workbook::new_empty().map_err(engine)?;
    let session = session_for(book, &app);
    *session.file_name.borrow_mut() = format!("Book{}", n + 2);
    *session.full_name.borrow_mut() = format!("Book{}", n + 2);
    let i = app.add(session.clone(), Origin::Added);
    app.active.set(i);
    Ok(obj(Rc::new(crate::app::WorkbookObj::new(&session))))
}

/// `xlOpenXMLWorkbook` and friends: the extension a `FileFormat` saves as.
fn format_extension(format: i64) -> Result<&'static str, ScriptError> {
    Ok(match format {
        51 => "xlsx",
        52 => "xlsm",
        6 | 62 => "csv",
        -4158 | 42 => "txt",
        _ => {
            return Err(ScriptError::unsupported(&format!(
                "Saving in file format {format} from a macro"
            )));
        }
    })
}

fn save_to(s: &Rc<Session>, path: &Path) -> Result<(), ScriptError> {
    let active = s.active.get();
    let idx = s.sheet_index(active)?;
    let mut book = s.read()?;
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if matches!(ext.as_str(), "csv" | "tsv" | "txt") {
        // CSV holds the active sheet.
        book.set_active_sheet(idx).map_err(engine)?;
    }
    let target = if ext == "txt" {
        path.with_extension("tsv")
    } else {
        path.to_path_buf()
    };
    book.save_as(&target)
        .map_err(|e| err1004(format!("Saving {} failed: {e:#}", path.display())))
}

/// `Workbook` members that concern files and windows: `Save`, `SaveAs`,
/// `SaveCopyAs`, `Close`, `Activate`. `None`: not one of them.
pub fn workbook_member(
    s: &Rc<Session>,
    lower: &str,
    args: &[Arg],
) -> Option<Result<Value, ScriptError>> {
    let is_this = Rc::ptr_eq(&this(s), s);
    Some(match lower {
        "save" if is_this => {
            s.save_requested.set(true);
            Ok(Value::Empty)
        }
        "save" => {
            let path = s.full_name.borrow().clone();
            if Path::new(&path).is_absolute() {
                save_to(s, Path::new(&path)).map(|()| Value::Empty)
            } else {
                Err(err1004(format!(
                    "{} has never been saved: use SaveAs",
                    s.file_name.borrow()
                )))
            }
        }
        "saveas" | "savecopyas" => (|| -> Result<Value, ScriptError> {
            let file = to_str(need(args, 0, "Filename")?)?;
            let mut path = absolute(s, &file);
            if lower == "saveas"
                && let Some(f) = arg_i64(args, 1, "FileFormat")?
            {
                let ext = format_extension(f)?;
                match path
                    .extension()
                    .map(|e| e.to_string_lossy().to_ascii_lowercase())
                {
                    None => {
                        path.set_extension(ext);
                    }
                    Some(e) if e == ext || (ext == "txt" && e == "tsv") => {}
                    Some(e) => {
                        return Err(err1004(format!(
                            "The file format doesn't match the extension .{e}"
                        )));
                    }
                }
            }
            check_path(s, &path, true)?;
            if lower == "savecopyas" {
                // A copy: the workbook keeps its own path and state.
                let book = s.read()?;
                book.prepare_save(&path)
                    .and_then(|job| job.run())
                    .map_err(|e| err1004(format!("Saving {} failed: {e:#}", path.display())))?;
                return Ok(Value::Empty);
            }
            save_to(s, &path)?;
            s.set_file(&path);
            Ok(Value::Empty)
        })(),
        "close" => (|| -> Result<Value, ScriptError> {
            let save = arg_bool(args, 0, "SaveChanges")?;
            if let Some(file) = arg_str(args, 1, "Filename")? {
                let path = absolute(s, &file);
                check_path(s, &path, true)?;
                save_to(s, &path)?;
                s.set_file(&path);
            }
            if is_this {
                s.close_requested.set(Some(save));
                return Ok(Value::Empty);
            }
            if save == Some(true) {
                let path = s.full_name.borrow().clone();
                if Path::new(&path).is_absolute() {
                    save_to(s, Path::new(&path))?;
                }
            }
            if let Some(app) = app_of(s)
                && let Some(i) = app.position(s)
            {
                app.books.borrow_mut()[i].closed = true;
                if app.active.get() == i {
                    app.active.set(0);
                }
            }
            Ok(Value::Empty)
        })(),
        "activate" => {
            if let Some(app) = app_of(s)
                && let Some(i) = app.position(s)
            {
                app.active.set(i);
            }
            Ok(Value::Empty)
        }
        _ => return None,
    })
}
