//! Workbook, worksheet and application events.
//!
//! An event runs its handlers in Excel's order: the sheet module's
//! (`Sheet1.Worksheet_Change(Target)`, matched by code name), then
//! `ThisWorkbook.Workbook_SheetChange(Sh, Target)`, then every `WithEvents`
//! variable holding the `Application` (`App_SheetChange(Sh, Target)`).
//! Arguments named `Cancel` are passed by reference and carried from one
//! handler to the next; the caller reads the result ([`crate::RunOutcome`]'s
//! `cancel`).
//!
//! Two ways in:
//! - **The host's** (a user edit, double-click, save, close): the UI or the
//!   command line asks [`crate::MacroEngine::fire`] with an [`Event`].
//!   [`EventIndex::wants`] answers first, from the project's procedure names
//!   alone, so an event nobody handles costs nothing.
//! - **A macro's own changes**: between two statements the interpreter asks
//!   the host for calls to run ([`kantor_script::Host::take_calls`]);
//!   [`poll`] compares the workbook with what it saw last (the undo history's
//!   length, the active sheet, its selection, the sheet ids) and turns the
//!   differences into `Change`, `Activate`/`Deactivate`, `SelectionChange`
//!   and `NewSheet` handlers. The changed cells come from the undo entries
//!   the statement added (`Workbook::edited_since`), so every way a macro
//!   edits fires `Worksheet_Change` without each object knowing about events.
//!
//! Nothing fires while `Application.EnableEvents` is `False`. A handler that
//! changes cells fires `Change` again (as in Excel); endless recursion stops
//! at the interpreter's call depth with "Out of stack space" (error 28).

use std::collections::HashSet;
use std::rc::Rc;

use kantor_script::vba::Project;
use kantor_script::{HostCall, Value};
use sheets_engine::CellRange;
use sheets_engine::script::EditedCells;
use sheets_engine::vba::ModuleKind;

use crate::app::WorkbookObj;
use crate::range::RangeObj;
use crate::run::MacroProject;
use crate::session::{Selection, Session};
use crate::sheet::SheetObj;
use crate::values::obj;

/// Something that happened, for the macros' handlers. Sheets are by index.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// After Enable Content (or when a trusted file opens).
    Open,
    BeforeClose,
    BeforeSave {
        save_as_ui: bool,
    },
    AfterSave {
        success: bool,
    },
    Activate {
        sheet: u32,
    },
    Deactivate {
        sheet: u32,
    },
    /// Cells changed by an edit, paste, fill, clear, undo or redo.
    Change {
        sheet: u32,
        areas: Vec<CellRange>,
    },
    SelectionChange {
        sheet: u32,
        range: CellRange,
    },
    NewSheet {
        sheet: u32,
    },
    BeforeDoubleClick {
        sheet: u32,
        row: u32,
        col: u32,
    },
    BeforeRightClick {
        sheet: u32,
        range: CellRange,
    },
    Calculate {
        sheet: u32,
    },
}

/// The kinds of [`Event`], for [`EventIndex::wants`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventKind {
    Open,
    BeforeClose,
    BeforeSave,
    AfterSave,
    Activate,
    Deactivate,
    Change,
    SelectionChange,
    NewSheet,
    BeforeDoubleClick,
    BeforeRightClick,
    Calculate,
}

impl Event {
    pub fn kind(&self) -> EventKind {
        match self {
            Event::Open => EventKind::Open,
            Event::BeforeClose => EventKind::BeforeClose,
            Event::BeforeSave { .. } => EventKind::BeforeSave,
            Event::AfterSave { .. } => EventKind::AfterSave,
            Event::Activate { .. } => EventKind::Activate,
            Event::Deactivate { .. } => EventKind::Deactivate,
            Event::Change { .. } => EventKind::Change,
            Event::SelectionChange { .. } => EventKind::SelectionChange,
            Event::NewSheet { .. } => EventKind::NewSheet,
            Event::BeforeDoubleClick { .. } => EventKind::BeforeDoubleClick,
            Event::BeforeRightClick { .. } => EventKind::BeforeRightClick,
            Event::Calculate { .. } => EventKind::Calculate,
        }
    }
}

const ALL_KINDS: [EventKind; 12] = [
    EventKind::Open,
    EventKind::BeforeClose,
    EventKind::BeforeSave,
    EventKind::AfterSave,
    EventKind::Activate,
    EventKind::Deactivate,
    EventKind::Change,
    EventKind::SelectionChange,
    EventKind::NewSheet,
    EventKind::BeforeDoubleClick,
    EventKind::BeforeRightClick,
    EventKind::Calculate,
];

impl EventKind {
    /// `(sheet module handler, ThisWorkbook handler, Application event)`.
    fn names(self) -> (Option<&'static str>, &'static str, &'static str) {
        match self {
            EventKind::Open => (None, "workbook_open", "workbookopen"),
            EventKind::BeforeClose => (None, "workbook_beforeclose", "workbookbeforeclose"),
            EventKind::BeforeSave => (None, "workbook_beforesave", "workbookbeforesave"),
            EventKind::AfterSave => (None, "workbook_aftersave", "workbookaftersave"),
            EventKind::Activate => (
                Some("worksheet_activate"),
                "workbook_sheetactivate",
                "sheetactivate",
            ),
            EventKind::Deactivate => (
                Some("worksheet_deactivate"),
                "workbook_sheetdeactivate",
                "sheetdeactivate",
            ),
            EventKind::Change => (
                Some("worksheet_change"),
                "workbook_sheetchange",
                "sheetchange",
            ),
            EventKind::SelectionChange => (
                Some("worksheet_selectionchange"),
                "workbook_sheetselectionchange",
                "sheetselectionchange",
            ),
            EventKind::NewSheet => (None, "workbook_newsheet", "workbooknewsheet"),
            EventKind::BeforeDoubleClick => (
                Some("worksheet_beforedoubleclick"),
                "workbook_sheetbeforedoubleclick",
                "sheetbeforedoubleclick",
            ),
            EventKind::BeforeRightClick => (
                Some("worksheet_beforerightclick"),
                "workbook_sheetbeforerightclick",
                "sheetbeforerightclick",
            ),
            EventKind::Calculate => (
                Some("worksheet_calculate"),
                "workbook_sheetcalculate",
                "sheetcalculate",
            ),
        }
    }
}

/// Which events the project handles, from its procedure names: cheap to
/// ask before every keystroke, and `Send` (the UI thread keeps one).
#[derive(Clone, Debug, Default)]
pub struct EventIndex {
    /// `module.procedure` of document modules, lower case.
    procs: HashSet<String>,
    /// The workbook's document module (`thisworkbook`), lower case.
    this: String,
    /// Events some `WithEvents` variable has a handler for (`sheetchange`).
    app: HashSet<String>,
    /// The kinds some handler exists for.
    kinds: HashSet<EventKind>,
    /// Whether a macro's own changes may fire anything.
    inside: bool,
}

impl EventIndex {
    pub fn new(macros: &MacroProject, project: &Project) -> EventIndex {
        let mut procs = HashSet::new();
        let docs: Vec<String> = macros
            .modules
            .iter()
            .filter(|m| m.kind == ModuleKind::Document)
            .map(|m| m.name.to_lowercase())
            .collect();
        for p in project.procedures() {
            let module = p.module.to_lowercase();
            if docs.contains(&module) {
                procs.insert(format!("{module}.{}", p.name.to_lowercase()));
            }
        }
        let this = docs
            .iter()
            .find(|d| *d == "thisworkbook")
            .cloned()
            .unwrap_or_else(|| "thisworkbook".to_owned());
        let app = project
            .with_events_handlers()
            .into_iter()
            .map(|(_, _, event)| event)
            .collect();
        let mut index = EventIndex {
            procs,
            this,
            app,
            kinds: HashSet::new(),
            inside: false,
        };
        for kind in ALL_KINDS {
            let (sheet, book, app) = kind.names();
            if index.app.contains(app)
                || index.procs.contains(&format!("{}.{book}", index.this))
                || sheet.is_some_and(|h| {
                    let suffix = format!(".{h}");
                    index.procs.iter().any(|p| p.ends_with(&suffix))
                })
            {
                index.kinds.insert(kind);
            }
        }
        index.inside = [
            EventKind::Change,
            EventKind::Activate,
            EventKind::Deactivate,
            EventKind::SelectionChange,
            EventKind::NewSheet,
        ]
        .into_iter()
        .any(|k| index.kinds.contains(&k));
        index
    }

    /// Whether any handler exists for `kind`.
    pub fn wants(&self, kind: EventKind) -> bool {
        self.kinds.contains(&kind)
    }

    /// Whether any handler exists for an event a macro's own changes fire.
    pub fn wants_inside(&self) -> bool {
        self.inside
    }

    /// Whether some `WithEvents` variable handles Application event `event`.
    pub fn app_handles(&self, event: &str) -> bool {
        self.app.contains(event)
    }

    /// The workbook's document module name (lower case).
    pub fn this_module(&self) -> &str {
        &self.this
    }

    fn has(&self, module: &str, proc_: &str) -> bool {
        self.procs
            .contains(&format!("{}.{proc_}", module.to_lowercase()))
    }
}

/// One handler to run.
pub struct Handler {
    /// `Sheet1.Worksheet_Change`.
    pub name: String,
    pub args: Vec<Value>,
    /// The position of `Cancel` in `args`.
    pub cancel: Option<usize>,
}

/// An `Application` event for `WithEvents` variables.
pub struct AppEvent {
    pub event: &'static str,
    pub args: Vec<Value>,
    pub cancel: Option<usize>,
}

/// The handlers of `event`, in Excel's order, with their arguments as
/// objects of session `s`. Cancel arguments start `False`.
pub fn handlers(s: &Rc<Session>, index: &EventIndex, event: &Event) -> (Vec<Handler>, AppEvent) {
    let kind = event.kind();
    let (sheet_proc, book_proc, app_event) = kind.names();
    let sheet = match event {
        Event::Activate { sheet }
        | Event::Deactivate { sheet }
        | Event::Change { sheet, .. }
        | Event::SelectionChange { sheet, .. }
        | Event::NewSheet { sheet }
        | Event::BeforeDoubleClick { sheet, .. }
        | Event::BeforeRightClick { sheet, .. }
        | Event::Calculate { sheet } => Some(*sheet),
        _ => None,
    };
    let sheet_id = sheet.and_then(|i| s.peek().ok().map(|b| b.sheet_id(i)));
    let sh = sheet_id.map(|id| obj(SheetObj::new(s, id)));
    let target = match (event, sheet_id) {
        (Event::Change { areas, .. }, Some(id)) => Some(obj(RangeObj::multi(s, id, areas.clone()))),
        (Event::SelectionChange { range, .. }, Some(id))
        | (Event::BeforeRightClick { range, .. }, Some(id)) => {
            Some(obj(RangeObj::new(s, id, *range)))
        }
        (Event::BeforeDoubleClick { row, col, .. }, Some(id)) => Some(obj(RangeObj::new(
            s,
            id,
            CellRange::from_corners((*row, *col), (*row, *col)),
        ))),
        _ => None,
    };
    let f = Value::Boolean(false);
    // The sheet module's arguments, and the workbook's (Sh first).
    let (sheet_args, cancel_at): (Vec<Value>, Option<usize>) = match event {
        Event::BeforeDoubleClick { .. } | Event::BeforeRightClick { .. } => (
            vec![target.clone().unwrap_or(Value::NOTHING), f.clone()],
            Some(1),
        ),
        Event::Change { .. } | Event::SelectionChange { .. } => {
            (vec![target.clone().unwrap_or(Value::NOTHING)], None)
        }
        _ => (Vec::new(), None),
    };
    let mut out = Vec::new();
    if let (Some(h), Some(id)) = (sheet_proc, sheet_id)
        && let Some(code) = s
            .code_names
            .borrow()
            .iter()
            .find(|(_, sid)| *sid == id)
            .map(|(c, _)| c.clone())
        && index.has(&code, h)
    {
        out.push(Handler {
            name: format!("{code}.{h}"),
            args: sheet_args.clone(),
            cancel: cancel_at,
        });
    }
    let (book_args, book_cancel): (Vec<Value>, Option<usize>) = match event {
        Event::Open => (Vec::new(), None),
        Event::BeforeClose => (vec![f.clone()], Some(0)),
        Event::BeforeSave { save_as_ui } => (vec![Value::Boolean(*save_as_ui), f.clone()], Some(1)),
        Event::AfterSave { success } => (vec![Value::Boolean(*success)], None),
        Event::NewSheet { .. }
        | Event::Activate { .. }
        | Event::Deactivate { .. }
        | Event::Calculate { .. } => (vec![sh.clone().unwrap_or(Value::NOTHING)], None),
        _ => {
            let mut a = vec![sh.clone().unwrap_or(Value::NOTHING)];
            a.extend(sheet_args.iter().cloned());
            (a, cancel_at.map(|c| c + 1))
        }
    };
    let this = index.this_module().to_owned();
    if index.has(&this, book_proc) {
        out.push(Handler {
            name: format!("{this}.{book_proc}"),
            args: book_args.clone(),
            cancel: book_cancel,
        });
    }
    // Application events name the workbook first for workbook events.
    let wb = obj(Rc::new(WorkbookObj::new(s)));
    let (app_args, app_cancel) = match event {
        Event::Open => (vec![wb], None),
        Event::BeforeClose | Event::BeforeSave { .. } | Event::AfterSave { .. } => {
            let mut a = vec![wb];
            a.extend(book_args.iter().cloned());
            (a, book_cancel.map(|c| c + 1))
        }
        Event::NewSheet { .. } => (vec![wb, sh.unwrap_or(Value::NOTHING)], None),
        _ => (book_args, book_cancel),
    };
    (
        out,
        AppEvent {
            event: app_event,
            args: app_args,
            cancel: app_cancel,
        },
    )
}

/// What events have seen of a session: the state [`poll`] compares with,
/// and calls queued for the interpreter.
#[derive(Default)]
pub struct Watch {
    /// The project's handlers; `None`: events are off for this workbook
    /// (content disabled, or a workbook whose macros don't run).
    pub index: Option<Rc<EventIndex>>,
    undo_len: usize,
    active: u32,
    selection: Option<Selection>,
    sheet_ids: Vec<u32>,
    /// Events were off since the last look: take the state as seen first.
    stale: bool,
    /// Calls to hand the interpreter at the next statement.
    pub pending: Vec<HostCall>,
}

/// Starts watching `s` for the events a run's changes fire.
pub fn start(s: &Rc<Session>, index: Option<Rc<EventIndex>>) {
    let mut w = s.events.borrow_mut();
    w.index = index;
    w.pending.clear();
    drop(w);
    resync(s);
}

/// Takes the current state as seen (nothing fires for it).
pub fn resync(s: &Rc<Session>) {
    let Ok(book) = s.peek() else {
        return;
    };
    let undo_len = book.history_lens().0;
    let n = book.sheet_names().len() as u32;
    let ids = (0..n).map(|i| book.sheet_id(i)).collect();
    drop(book);
    let active = s.active.get();
    let mut w = s.events.borrow_mut();
    w.stale = false;
    w.undo_len = undo_len;
    w.sheet_ids = ids;
    w.active = active;
    w.selection = Some(s.selection(active));
}

/// The calls a macro's changes since the last poll fire (and what was
/// queued). Called before every statement: without handlers or with
/// events disabled it only looks at a flag.
pub fn poll(s: &Rc<Session>) -> Vec<HostCall> {
    let index = {
        let mut w = s.events.borrow_mut();
        match w.index.clone().filter(|i| i.wants_inside()) {
            Some(i) => i,
            None => return std::mem::take(&mut w.pending),
        }
    };
    if !s.settings.borrow().enable_events {
        let mut w = s.events.borrow_mut();
        w.stale = true;
        return std::mem::take(&mut w.pending);
    }
    if s.events.borrow().stale {
        resync(s);
    }
    let mut events = Vec::new();
    {
        let Ok(book) = s.peek() else {
            return std::mem::take(&mut s.events.borrow_mut().pending);
        };
        let mut w = s.events.borrow_mut();
        let undo_len = book.history_lens().0;
        if undo_len > w.undo_len && index.wants(EventKind::Change) {
            let edited = book.edited_since(w.undo_len);
            events.extend(change_events(&book, &edited));
        }
        w.undo_len = undo_len;
        let n = book.sheet_names().len() as u32;
        let ids: Vec<u32> = (0..n).map(|i| book.sheet_id(i)).collect();
        for (i, id) in ids.iter().enumerate() {
            if !w.sheet_ids.contains(id) {
                events.push(Event::NewSheet { sheet: i as u32 });
            }
        }
        let active = s.active.get();
        let selection = s.selection(active);
        if active != w.active {
            if let Some(old) = ids.iter().position(|id| *id == w.active) {
                events.push(Event::Deactivate { sheet: old as u32 });
            }
            if let Some(new) = ids.iter().position(|id| *id == active) {
                events.push(Event::Activate { sheet: new as u32 });
            }
        } else if w.selection != Some(selection)
            && let Some(i) = ids.iter().position(|id| *id == active)
        {
            events.push(Event::SelectionChange {
                sheet: i as u32,
                range: selection.range,
            });
        }
        w.sheet_ids = ids;
        w.active = active;
        w.selection = Some(selection);
    }
    let mut calls = std::mem::take(&mut s.events.borrow_mut().pending);
    for e in events {
        if !index.wants(e.kind()) {
            continue;
        }
        let (hs, app) = handlers(s, &index, &e);
        for h in hs {
            calls.push(HostCall::Proc {
                name: h.name,
                args: h.args,
            });
        }
        if index.app.contains(app.event) {
            calls.push(HostCall::Event {
                source_type: "Application".into(),
                event: app.event.into(),
                args: app.args,
            });
        }
    }
    calls
}

/// `Change` events for edited cells: one per sheet, with every area. A
/// sheet changed as a whole (rows inserted, a sort) gives its used range.
pub fn change_events(book: &sheets_engine::Workbook, edited: &EditedCells) -> Vec<Event> {
    let mut out: Vec<Event> = Vec::new();
    let n = book.sheet_names().len() as u32;
    let mut add = |sheet: u32, area: CellRange| {
        if sheet >= n {
            return;
        }
        match out
            .iter_mut()
            .find(|e| matches!(e, Event::Change { sheet: s, .. } if *s == sheet))
        {
            Some(Event::Change { areas, .. }) => {
                if !areas.contains(&area) {
                    areas.push(area);
                }
            }
            _ => out.push(Event::Change {
                sheet,
                areas: vec![area],
            }),
        }
    };
    for (sheet, area) in &edited.cells {
        add(*sheet, *area);
    }
    for sheet in &edited.sheets {
        let (rows, cols) = book.used_size(*sheet);
        add(
            *sheet,
            CellRange::from_corners((0, 0), (rows.max(1) - 1, cols.max(1) - 1)),
        );
    }
    out
}
