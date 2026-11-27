//! One macro run's state: the workbook (lent to the run), the active sheet
//! and selections, the clipboard, Find's last search and the application
//! settings a macro changes (`ScreenUpdating`…).

use std::cell::{Cell, RefCell, RefMut};
use std::collections::HashMap;

use kantor_script::{ScriptError, Value};
use sheets_engine::{CellRange, Workbook};

use crate::values::err1004;

/// What is selected on one sheet: the range and the active cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub range: CellRange,
    pub active: (u32, u32),
}

impl Selection {
    pub fn cell(row: u32, col: u32) -> Selection {
        Selection {
            range: CellRange::from_corners((row, col), (row, col)),
            active: (row, col),
        }
    }
}

/// What the grid shows: the active sheet (index) and its selection. A run
/// starts from the UI's and hands back where the macro left it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View {
    pub sheet: u32,
    pub selection: Selection,
}

impl Default for View {
    fn default() -> View {
        View {
            sheet: 0,
            selection: Selection::cell(0, 0),
        }
    }
}

/// Cells copied or cut, waiting for `Paste` / `PasteSpecial`.
#[derive(Clone, Copy, Debug)]
pub struct Clip {
    pub sheet_id: u32,
    pub range: CellRange,
    pub cut: bool,
}

/// `Find`'s last search, for `FindNext` / `FindPrevious`.
#[derive(Clone, Debug)]
pub struct FindState {
    pub what: String,
    pub formulas: bool,
    pub whole: bool,
    pub match_case: bool,
    pub by_columns: bool,
}

/// Application settings a macro may change (most are accepted and kept,
/// they change nothing in Kantor).
pub struct Settings {
    pub screen_updating: bool,
    pub display_alerts: bool,
    pub enable_events: bool,
    pub status_bar: Value,
    pub display_status_bar: bool,
    pub cursor: i32,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            screen_updating: true,
            display_alerts: true,
            enable_events: true,
            status_bar: Value::Boolean(false),
            display_status_bar: true,
            cursor: -4143,
        }
    }
}

/// A sheet's `Sort` object being set up: `(key column, descending)` keys,
/// the range and the header setting, until `Apply`.
#[derive(Default, Clone)]
pub struct PendingSort {
    pub keys: Vec<(u32, bool)>,
    pub range: Option<CellRange>,
    pub header: i64,
}

pub struct Session {
    book: RefCell<Option<Workbook>>,
    /// Edits since the workbook was last recalculated for reading.
    dirty: Cell<bool>,
    pub active: Cell<u32>,
    pub selections: RefCell<HashMap<u32, Selection>>,
    pub clip: Cell<Option<Clip>>,
    pub find: RefCell<Option<FindState>>,
    pub sorts: RefCell<HashMap<u32, PendingSort>>,
    pub settings: RefCell<Settings>,
    /// Document module (code) names → `sheetId`.
    pub code_names: RefCell<Vec<(String, u32)>>,
    /// `ThisWorkbook.Save` was called: the host saves when the run ends.
    pub save_requested: Cell<bool>,
    /// The workbook's file name and path, for `Name` / `FullName` / `Path`.
    pub file_name: RefCell<String>,
    pub full_name: RefCell<String>,
    pub folder: RefCell<String>,
    /// The user name (`Application.UserName`).
    pub user_name: RefCell<String>,
    /// Where dialogs and `Debug.Print` go.
    pub ui: RefCell<Box<dyn crate::run::MacroUi>>,
}

impl Session {
    pub fn new(book: Workbook, view: View, ui: Box<dyn crate::run::MacroUi>) -> Session {
        let sheet_id = book.sheet_id(view.sheet.min(book.sheet_names().len() as u32 - 1));
        let title = book.title();
        let (full, folder) = match book.path() {
            Some(p) => (
                p.display().to_string(),
                p.parent()
                    .map(|d| d.display().to_string())
                    .unwrap_or_default(),
            ),
            None => (title.clone(), String::new()),
        };
        let mut code_names = Vec::new();
        let names = book.sheet_names();
        for (sheet, code) in book.sheet_code_names() {
            if let Some(i) = names.iter().position(|n| *n == sheet) {
                code_names.push((code, book.sheet_id(i as u32)));
            }
        }
        let mut selections = HashMap::new();
        selections.insert(sheet_id, view.selection);
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_default();
        Session {
            book: RefCell::new(Some(book)),
            dirty: Cell::new(false),
            active: Cell::new(sheet_id),
            selections: RefCell::new(selections),
            clip: Cell::new(None),
            find: RefCell::new(None),
            sorts: RefCell::new(HashMap::new()),
            settings: RefCell::new(Settings::default()),
            code_names: RefCell::new(code_names),
            save_requested: Cell::new(false),
            file_name: RefCell::new(title),
            full_name: RefCell::new(full),
            folder: RefCell::new(folder),
            user_name: RefCell::new(user),
            ui: RefCell::new(ui),
        }
    }

    /// The workbook for an edit (marks values stale until the next read).
    pub fn write(&self) -> Result<RefMut<'_, Workbook>, ScriptError> {
        let book = self.borrow()?;
        self.dirty.set(true);
        Ok(book)
    }

    /// The workbook for reading values: edits so far are recalculated first.
    pub fn read(&self) -> Result<RefMut<'_, Workbook>, ScriptError> {
        let mut book = self.borrow()?;
        if self.dirty.replace(false) {
            book.script_sync();
        }
        Ok(book)
    }

    fn borrow(&self) -> Result<RefMut<'_, Workbook>, ScriptError> {
        let book = self
            .book
            .try_borrow_mut()
            .map_err(|_| err1004("The workbook is busy"))?;
        RefMut::filter_map(book, Option::as_mut)
            .map_err(|_| err1004("The workbook is not available to this macro"))
    }

    /// Takes the workbook out (the run ended, or a dialog shows it).
    pub fn take_book(&self) -> Option<Workbook> {
        if self.dirty.replace(false)
            && let Some(b) = self.book.borrow_mut().as_mut()
        {
            b.script_sync();
        }
        self.book.borrow_mut().take()
    }

    pub fn put_book(&self, book: Workbook) {
        *self.book.borrow_mut() = Some(book);
    }

    /// The index of the sheet with `sheet_id` now (error 424 when it was
    /// deleted).
    pub fn index_of(&self, book: &Workbook, sheet_id: u32) -> Result<u32, ScriptError> {
        let n = book.sheet_names().len() as u32;
        (0..n)
            .find(|&i| book.sheet_id(i) == sheet_id)
            .ok_or_else(|| ScriptError::new(424, "Object required (the sheet was deleted)"))
    }

    /// A sheet's current index by id, borrowing the workbook briefly.
    pub fn sheet_index(&self, sheet_id: u32) -> Result<u32, ScriptError> {
        let book = self.borrow()?;
        self.index_of(&book, sheet_id)
    }

    /// The selection on sheet `sheet_id` (A1 when none was made).
    pub fn selection(&self, sheet_id: u32) -> Selection {
        self.selections
            .borrow()
            .get(&sheet_id)
            .copied()
            .unwrap_or(Selection::cell(0, 0))
    }

    pub fn set_selection(&self, sheet_id: u32, sel: Selection) {
        self.selections.borrow_mut().insert(sheet_id, sel);
    }

    /// Where the run left the grid.
    pub fn view(&self) -> View {
        let id = self.active.get();
        let sheet = self.sheet_index(id).unwrap_or(0);
        View {
            sheet,
            selection: self.selection(id),
        }
    }

    /// The sheet a document module is the code of (`Sheet1` → its `sheetId`).
    pub fn code_name_sheet(&self, name: &str) -> Option<u32> {
        self.code_names
            .borrow()
            .iter()
            .find(|(c, _)| c.eq_ignore_ascii_case(name))
            .map(|(_, id)| *id)
    }
}
