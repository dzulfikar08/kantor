//! Kantor Sheets engine.
//!
//! A thin, UI-agnostic wrapper over [IronCalc](https://github.com/ironcalc/IronCalc).
//! All coordinates in this API are **0-based** (`row 0, col 0` is `A1`);
//! IronCalc itself is 1-based, and the conversion happens only here.

use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use ironcalc::export::save_xlsx_to_writer;
use ironcalc::import::load_from_xlsx;
use ironcalc_base::UserModel;
use ironcalc_base::expressions::utils::number_to_column;

const LOCALE: &str = "en";
const TIMEZONE: &str = "UTC";
const LANGUAGE: &str = "en";

/// Excel's limits.
pub const MAX_ROWS: u32 = 1_048_576;
pub const MAX_COLS: u32 = 16_384;

/// An open spreadsheet document.
///
/// Edits go through IronCalc's [`UserModel`], which records each one for
/// undo/redo. Whether there are unsaved changes is tracked as a position in
/// that history: every state gets an id, and the document is dirty when the
/// current id differs from the one last saved. So undoing back to the saved
/// state makes it clean again.
pub struct Workbook {
    model: UserModel<'static>,
    path: Option<PathBuf>,
    history: Revisions,
}

/// Ids of the states reachable by undo and redo, mirroring IronCalc's stacks.
#[derive(Default)]
struct Revisions {
    current: u64,
    saved: u64,
    next: u64,
    undo: Vec<u64>,
    redo: Vec<u64>,
}

impl Revisions {
    fn edited(&mut self) {
        self.next += 1;
        self.undo.push(self.current);
        self.current = self.next;
        self.redo.clear();
    }

    fn undone(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(self.current);
            self.current = prev;
        }
    }

    fn redone(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(self.current);
            self.current = next;
        }
    }

    fn is_dirty(&self) -> bool {
        self.current != self.saved
    }
}

impl Workbook {
    /// A new, empty workbook with one sheet.
    pub fn new_empty() -> Result<Self> {
        let model = UserModel::new_empty("Untitled", LOCALE, TIMEZONE, LANGUAGE)
            .map_err(|e| anyhow!("creating workbook: {e}"))?;
        Ok(Self {
            model,
            path: None,
            history: Revisions::default(),
        })
    }

    /// Open an `.xlsx` file.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        match ext.as_str() {
            "xlsx" | "xlsm" => {}
            other => bail!("unsupported file type: .{other} (only .xlsx for now)"),
        }
        let name = path.to_str().context("path is not valid UTF-8")?;
        let mut model = load_from_xlsx(name, LOCALE, TIMEZONE, LANGUAGE)
            .map_err(|e| anyhow!("opening {}: {e}", path.display()))?;
        model.evaluate();
        Ok(Self {
            model: UserModel::from_model(model),
            path: Some(path.to_path_buf()),
            history: Revisions::default(),
        })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn is_dirty(&self) -> bool {
        self.history.is_dirty()
    }

    pub fn can_undo(&self) -> bool {
        self.model.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.model.can_redo()
    }

    /// Undo the last edit (recalculates). Returns `false` if there was none.
    pub fn undo(&mut self) -> Result<bool> {
        if !self.model.can_undo() {
            return Ok(false);
        }
        self.model.undo().map_err(|e| anyhow!("undo: {e}"))?;
        self.history.undone();
        Ok(true)
    }

    /// Redo the last undone edit (recalculates). Returns `false` if there was none.
    pub fn redo(&mut self) -> Result<bool> {
        if !self.model.can_redo() {
            return Ok(false);
        }
        self.model.redo().map_err(|e| anyhow!("redo: {e}"))?;
        self.history.redone();
        Ok(true)
    }

    /// File name for window titles, e.g. `budget.xlsx` or `Untitled`.
    pub fn title(&self) -> String {
        self.path
            .as_deref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".to_owned())
    }

    /// Save to the current path.
    pub fn save(&mut self) -> Result<()> {
        let path = self
            .path
            .clone()
            .context("workbook has no path yet; use save_as")?;
        self.save_as(path)
    }

    /// Save as `.xlsx` to `path`, atomically (write a temp file, then rename),
    /// so a crash mid-save never destroys the previous file.
    pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let dir = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let file_name = path.file_name().context("save path has no file name")?;
        let tmp = dir.join(format!(".{}.kantor-tmp", file_name.to_string_lossy()));
        {
            let file =
                fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
            let writer = save_xlsx_to_writer(self.model.get_model(), BufWriter::new(file))
                .map_err(|e| anyhow!("writing xlsx: {e}"))?;
            writer
                .into_inner()
                .map_err(|e| anyhow!("flushing xlsx: {e}"))?
                .sync_all()?;
        }
        fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
        self.path = Some(path.to_path_buf());
        self.history.saved = self.history.current;
        Ok(())
    }

    /// Names of all sheets, in order.
    pub fn sheet_names(&self) -> Vec<String> {
        self.model
            .get_worksheets_properties()
            .into_iter()
            .map(|p| p.name)
            .collect()
    }

    /// `(rows, cols)` spanned by data on `sheet`, counted from `A1`.
    pub fn used_size(&self, sheet: u32) -> (u32, u32) {
        match self.model.get_model().workbook.worksheet(sheet) {
            Ok(ws) => {
                let d = ws.dimension();
                (d.max_row.max(0) as u32, d.max_column.max(0) as u32)
            }
            Err(_) => (0, 0),
        }
    }

    /// The value as displayed in the grid (number formats applied).
    pub fn display_value(&self, sheet: u32, row: u32, col: u32) -> String {
        self.model
            .get_formatted_cell_value(sheet, row as i32 + 1, col as i32 + 1)
            .unwrap_or_default()
    }

    /// What the user typed: the formula (`=SUM(A1:A3)`) or the raw value.
    pub fn input(&self, sheet: u32, row: u32, col: u32) -> String {
        self.model
            .get_cell_content(sheet, row as i32 + 1, col as i32 + 1)
            .unwrap_or_default()
    }

    /// Set a cell from user input (value or `=formula`) and recalculate.
    pub fn set_input(&mut self, sheet: u32, row: u32, col: u32, value: &str) -> Result<()> {
        if row >= MAX_ROWS || col >= MAX_COLS {
            bail!("cell out of range");
        }
        self.model
            .set_user_input(sheet, row as i32 + 1, col as i32 + 1, value)
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(())
    }
}

/// `0 -> "A"`, `25 -> "Z"`, `26 -> "AA"`.
pub fn column_name(col: u32) -> String {
    number_to_column(col as i32 + 1).unwrap_or_default()
}

/// `(0, 0) -> "A1"`.
pub fn cell_name(row: u32, col: u32) -> String {
    format!("{}{}", column_name(col), row + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(column_name(0), "A");
        assert_eq!(column_name(25), "Z");
        assert_eq!(column_name(26), "AA");
        assert_eq!(cell_name(9, 1), "B10");
    }

    #[test]
    fn formulas_recalculate() {
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(0, 0, 0, "2").unwrap();
        wb.set_input(0, 0, 1, "=A1*3").unwrap();
        assert_eq!(wb.display_value(0, 0, 1), "6");
        assert_eq!(wb.input(0, 0, 1), "=A1*3");
        wb.set_input(0, 0, 0, "5").unwrap();
        assert_eq!(wb.display_value(0, 0, 1), "15");
        assert!(wb.is_dirty());
        assert_eq!(wb.used_size(0), (1, 2));
    }

    #[test]
    fn xlsx_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rt.xlsx");
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(0, 0, 0, "Item").unwrap();
        wb.set_input(0, 1, 0, "10").unwrap();
        wb.set_input(0, 2, 0, "32").unwrap();
        wb.set_input(0, 3, 0, "=SUM(A2:A3)").unwrap();
        wb.save_as(&path).unwrap();
        assert!(!wb.is_dirty());

        // Saving again over an existing file must work (atomic replace).
        wb.set_input(0, 1, 0, "11").unwrap();
        wb.save().unwrap();

        let back = Workbook::open(&path).unwrap();
        assert_eq!(back.display_value(0, 0, 0), "Item");
        assert_eq!(back.display_value(0, 3, 0), "43");
        assert_eq!(back.input(0, 3, 0), "=SUM(A2:A3)");
        assert_eq!(back.sheet_names(), vec!["Sheet1".to_owned()]);
        assert_eq!(back.title(), "rt.xlsx");
    }

    #[test]
    fn undo_redo() {
        let mut wb = Workbook::new_empty().unwrap();
        assert!(!wb.can_undo());
        wb.set_input(0, 0, 0, "2").unwrap();
        wb.set_input(0, 0, 1, "=A1*3").unwrap();
        wb.set_input(0, 0, 0, "5").unwrap();
        assert_eq!(wb.display_value(0, 0, 1), "15");

        assert!(wb.undo().unwrap());
        assert_eq!(wb.input(0, 0, 0), "2");
        assert_eq!(wb.display_value(0, 0, 1), "6");
        assert!(wb.can_redo());

        assert!(wb.redo().unwrap());
        assert_eq!(wb.display_value(0, 0, 1), "15");
        assert!(!wb.redo().unwrap());

        assert!(wb.undo().unwrap());
        assert!(wb.undo().unwrap());
        assert!(wb.undo().unwrap());
        assert_eq!(wb.input(0, 0, 0), "");
        assert!(!wb.undo().unwrap());
    }

    #[test]
    fn dirty_follows_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.xlsx");
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(0, 0, 0, "1").unwrap();
        assert!(wb.is_dirty());
        wb.save_as(&path).unwrap();
        assert!(!wb.is_dirty());

        wb.set_input(0, 0, 0, "2").unwrap();
        assert!(wb.is_dirty());
        wb.undo().unwrap();
        assert!(!wb.is_dirty(), "undo back to the saved state is clean");
        wb.undo().unwrap();
        assert!(wb.is_dirty(), "undo past the save is dirty");
        wb.redo().unwrap();
        assert!(!wb.is_dirty());

        // A new edit after undo drops the redo branch; the saved state stays reachable only by undo.
        wb.undo().unwrap();
        wb.set_input(0, 0, 0, "3").unwrap();
        assert!(wb.is_dirty());
        assert!(!wb.can_redo());
    }

    #[test]
    fn rejects_unknown_extension() {
        assert!(Workbook::open("notes.txt").is_err());
    }
}
