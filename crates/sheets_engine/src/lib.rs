//! Kantor Sheets engine.
//!
//! A thin, UI-agnostic wrapper over [IronCalc](https://github.com/ironcalc/IronCalc).
//! All coordinates in this API are **0-based** (`row 0, col 0` is `A1`);
//! IronCalc itself is 1-based, and the conversion happens only here.
//!
//! Files: `.xlsx` is the native format (open and save). CSV/TSV open and save
//! (one sheet, values only); `.ods` / `.xls` / `.xlsb` open read-only and save
//! as `.xlsx`. See [`SourceFormat`] and the `import` module docs.

mod csv;
mod import;
mod preserve;

pub use import::SourceFormat;
pub use preserve::package_problems;

use std::borrow::Cow;
use std::fs;
use std::io::{Cursor, Write as _};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context as _, Result, anyhow, bail};
use ironcalc::export::save_xlsx_to_writer;
use ironcalc::import::{load_from_xlsx, load_from_xlsx_bytes};
use ironcalc_base::expressions::types::Area;
use ironcalc_base::expressions::utils::number_to_column;
use ironcalc_base::types::Color;
use ironcalc_base::{Model, UserModel};

pub mod format;
pub mod tsv;

pub use format::{CellStyle, HAlign, NumberFormat, StyleChange, ValueKind};

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
    /// Where Save writes; `None` for new and imported workbooks.
    path: Option<PathBuf>,
    history: Revisions,
    origin: Origin,
    save_notes: Vec<String>,
}

/// The file a workbook shows and how it got there.
#[derive(Default)]
struct Origin {
    format: SourceFormat,
    /// The file last opened or saved (for imports, the `.ods`/`.xls` itself).
    path: Option<PathBuf>,
    notes: Vec<String>,
    /// The last `.xlsx` opened or saved, whose parts IronCalc doesn't model
    /// are carried into the next `.xlsx` save (see the `preserve` module).
    xlsx: Option<XlsxSource>,
}

/// Files up to this size are kept in memory for part preservation; larger
/// ones are read again at save time.
const KEEP_IN_MEMORY: u64 = 64 * 1024 * 1024;

/// Where the original package for part preservation comes from.
enum XlsxSource {
    Bytes(Vec<u8>),
    /// A large file, read again at save. `len` and `modified` are checked so a
    /// file changed by someone else is not mixed into ours.
    File {
        path: PathBuf,
        len: u64,
        modified: Option<SystemTime>,
    },
}

impl XlsxSource {
    fn file(path: &Path, meta: &fs::Metadata) -> Self {
        XlsxSource::File {
            path: path.to_path_buf(),
            len: meta.len(),
            modified: meta.modified().ok(),
        }
    }

    fn bytes(&self) -> Result<Cow<'_, [u8]>, String> {
        match self {
            XlsxSource::Bytes(b) => Ok(Cow::Borrowed(b)),
            XlsxSource::File {
                path,
                len,
                modified,
            } => {
                let meta = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
                if meta.len() != *len || meta.modified().ok() != *modified {
                    return Err(format!(
                        "{} changed on disk since it was opened",
                        path.display()
                    ));
                }
                fs::read(path)
                    .map(Cow::Owned)
                    .map_err(|e| format!("{}: {e}", path.display()))
            }
        }
    }
}

/// Ids of the states reachable by undo and redo, mirroring IronCalc's stacks.
/// Each revision also records how many IronCalc steps it took (a replace-all
/// sets many cells, one IronCalc step each, but undoes as one).
#[derive(Default)]
struct Revisions {
    current: u64,
    saved: u64,
    next: u64,
    undo: Vec<(u64, u32)>,
    redo: Vec<(u64, u32)>,
    /// IronCalc steps of the current revision.
    steps: u32,
}

impl Revisions {
    fn edited(&mut self) {
        self.edited_steps(1);
    }

    fn edited_steps(&mut self, steps: u32) {
        self.next += 1;
        self.undo.push((self.current, self.steps));
        self.current = self.next;
        self.steps = steps;
        self.redo.clear();
    }

    /// IronCalc steps to undo for the current revision.
    fn undo_steps(&self) -> u32 {
        if self.undo.is_empty() {
            1
        } else {
            self.steps.max(1)
        }
    }

    /// IronCalc steps to redo for the next revision.
    fn redo_steps(&self) -> u32 {
        self.redo.last().map_or(1, |r| r.1.max(1))
    }

    fn undone(&mut self) {
        if let Some((prev, prev_steps)) = self.undo.pop() {
            self.redo.push((self.current, self.steps));
            self.current = prev;
            self.steps = prev_steps;
        }
    }

    fn redone(&mut self) {
        if let Some((next, steps)) = self.redo.pop() {
            self.undo.push((self.current, self.steps));
            self.current = next;
            self.steps = steps;
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
            origin: Origin::default(),
            save_notes: Vec::new(),
        })
    }

    /// Open a spreadsheet, by extension: `.xlsx`/`.xlsm`, `.csv`/`.tsv`
    /// (delimiter detected for `.csv`), or a read-only import of
    /// `.ods`/`.xls`/`.xlsb`. The workbook starts clean with no undo history.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        let Some(format) = SourceFormat::from_extension(ext) else {
            bail!("unsupported file type: .{ext}");
        };
        let mut notes = Vec::new();
        let mut xlsx = None;
        let (mut model, format) = match format {
            SourceFormat::Xlsx => {
                let (model, source) = open_xlsx(path)?;
                xlsx = Some(source);
                (model, format)
            }
            SourceFormat::Csv { delimiter } => {
                let forced = (delimiter == b'\t').then_some(delimiter);
                let (model, delimiter) = import::read_csv(path, forced)?;
                (model, SourceFormat::Csv { delimiter })
            }
            _ => {
                let (model, n) = import::read_calamine(path, format)?;
                notes = n;
                (model, format)
            }
        };
        model.evaluate();
        Ok(Self {
            model: UserModel::from_model(model),
            path: (!format.is_imported()).then(|| path.to_path_buf()),
            history: Revisions::default(),
            origin: Origin {
                format,
                path: Some(path.to_path_buf()),
                notes,
                xlsx,
            },
            save_notes: Vec::new(),
        })
    }

    /// Where Save writes. `None` for new workbooks and for imports
    /// (`.ods`/`.xls`/`.xlsb`), so Save turns into Save As and an imported file
    /// is never overwritten with xlsx bytes.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The file this workbook was opened from or last saved to, including
    /// imports (unlike [`Workbook::path`]). Use it to identify the document.
    pub fn source_path(&self) -> Option<&Path> {
        self.origin.path.as_deref()
    }

    /// The format of [`Workbook::source_path`].
    pub fn source_format(&self) -> SourceFormat {
        self.origin.format
    }

    /// What an import could not bring over faithfully, e.g.
    /// `"2 formulas kept as values"`. Empty for xlsx and CSV.
    pub fn import_notes(&self) -> &[String] {
        &self.origin.notes
    }

    /// What the last save could not carry over from the original `.xlsx`
    /// (e.g. `sheet "Data": tableParts not kept`), or error values it had to
    /// rewrite. Empty when everything was kept.
    pub fn save_notes(&self) -> &[String] {
        &self.save_notes
    }

    /// File name to suggest in a Save As prompt: the current name, or for
    /// imports the same name with `.xlsx` (`budget.ods` → `budget.xlsx`).
    pub fn suggested_file_name(&self) -> String {
        match (&self.path, &self.origin.path) {
            (Some(_), _) => self.title(),
            (None, Some(src)) => {
                let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned());
                format!("{}.xlsx", stem.as_deref().unwrap_or("Untitled"))
            }
            (None, None) => "Untitled.xlsx".to_owned(),
        }
    }

    /// The sheet CSV export writes (IronCalc's selected sheet). Not an edit:
    /// it doesn't touch undo history or the dirty state.
    pub fn active_sheet(&self) -> u32 {
        self.model.get_selected_sheet()
    }

    pub fn set_active_sheet(&mut self, sheet: u32) -> Result<()> {
        self.model
            .set_selected_sheet(sheet)
            .map_err(|e| anyhow!("{e}"))
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
        for _ in 0..self.history.undo_steps() {
            if !self.model.can_undo() {
                break;
            }
            self.model.undo().map_err(|e| anyhow!("undo: {e}"))?;
        }
        self.history.undone();
        Ok(true)
    }

    /// Redo the last undone edit (recalculates). Returns `false` if there was none.
    pub fn redo(&mut self) -> Result<bool> {
        if !self.model.can_redo() {
            return Ok(false);
        }
        for _ in 0..self.history.redo_steps() {
            if !self.model.can_redo() {
                break;
            }
            self.model.redo().map_err(|e| anyhow!("redo: {e}"))?;
        }
        self.history.redone();
        Ok(true)
    }

    /// File name for window titles, e.g. `budget.xlsx`, `data.ods` or `Untitled.xlsx`.
    pub fn title(&self) -> String {
        self.origin
            .path
            .as_deref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled.xlsx".to_owned())
    }

    /// Save to the current path.
    pub fn save(&mut self) -> Result<()> {
        let path = self
            .path
            .clone()
            .context("workbook has no path yet; use save_as")?;
        self.save_as(path)
    }

    /// Save to `path` atomically (write a temp file, then rename), so a crash
    /// mid-save never destroys the previous file. The format follows the
    /// extension:
    /// * `.xlsx` / `.xlsm`: the whole workbook.
    /// * `.csv` / `.tsv`: **only the active sheet's displayed values**
    ///   ([`Workbook::active_sheet`]); formulas, formats and other sheets are not
    ///   written. `.tsv` uses tab; `.csv` keeps the delimiter the file was opened
    ///   with (else `,`). Use [`Workbook::sheets_lost_in_csv`] to warn first.
    /// * anything else (including `.ods` / `.xls`) is refused.
    ///
    /// For `.xlsx`, parts of the original `.xlsx` that IronCalc doesn't write
    /// (comments, custom properties, drawings, print setup...) are carried
    /// over; see [`Workbook::save_notes`] for what could not be.
    pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        let mut xlsx = self.origin.xlsx.take();
        let format = match SourceFormat::from_extension(ext) {
            Some(SourceFormat::Xlsx) => {
                let written = save_xlsx_to_writer(self.model.get_model(), Cursor::new(Vec::new()))
                    .map_err(|e| anyhow!("writing xlsx: {e}"));
                let written = match written {
                    Ok(w) => w.into_inner(),
                    Err(e) => {
                        self.origin.xlsx = xlsx;
                        return Err(e);
                    }
                };
                let mut notes = Vec::new();
                let original = xlsx.as_ref().and_then(|s| match s.bytes() {
                    Ok(b) => Some(b),
                    Err(e) => {
                        notes.push(format!("extra parts of the original not kept: {e}"));
                        None
                    }
                });
                let finished = preserve::finish(written, original.as_deref(), &self.tab_colors());
                drop(original);
                notes.extend(finished.notes);
                for note in &notes {
                    log::warn!("saving {}: {note}", path.display());
                }
                let bytes = finished.bytes;
                if let Err(e) = write_atomic(path, |file| Ok(file.write_all(&bytes)?)) {
                    self.origin.xlsx = xlsx;
                    return Err(e);
                }
                self.save_notes = notes;
                xlsx = Some(if bytes.len() as u64 <= KEEP_IN_MEMORY {
                    XlsxSource::Bytes(bytes)
                } else {
                    match fs::metadata(path) {
                        Ok(meta) => XlsxSource::file(path, &meta),
                        Err(_) => XlsxSource::Bytes(bytes),
                    }
                });
                SourceFormat::Xlsx
            }
            Some(SourceFormat::Csv { delimiter }) => {
                let delimiter = match self.origin.format {
                    SourceFormat::Csv { delimiter: d } if delimiter != b'\t' && d != b'\t' => d,
                    _ => delimiter,
                };
                let written =
                    import::write_csv(self.model.get_model(), self.active_sheet(), path, delimiter);
                if let Err(e) = written {
                    self.origin.xlsx = xlsx;
                    return Err(e);
                }
                self.save_notes = Vec::new();
                SourceFormat::Csv { delimiter }
            }
            _ => {
                self.origin.xlsx = xlsx;
                bail!("Kantor saves .xlsx or .csv, not .{ext}")
            }
        };
        self.path = Some(path.to_path_buf());
        // A CSV save keeps the xlsx source: a later .xlsx save still carries
        // its parts, since the model still has every sheet.
        self.origin = Origin {
            format,
            path: Some(path.to_path_buf()),
            notes: Vec::new(),
            xlsx,
        };
        self.history.saved = self.history.current;
        Ok(())
    }

    /// How many sheets a save to CSV would leave out (all but the active one).
    pub fn sheets_lost_in_csv(&self) -> usize {
        self.model
            .get_worksheets_properties()
            .len()
            .saturating_sub(1)
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

    /// Cells of `sheet` that have content, `(row, col)` in reading order
    /// (row by row). Cells that only carry a style are left out.
    pub fn filled_cells(&self, sheet: u32) -> Vec<(u32, u32)> {
        let Ok(ws) = self.model.get_model().workbook.worksheet(sheet) else {
            return Vec::new();
        };
        let mut cells: Vec<(u32, u32)> = ws
            .sheet_data
            .cells()
            .filter(|(row, col, _)| *row >= 1 && *col >= 1)
            .map(|(row, col, _)| (row as u32 - 1, col as u32 - 1))
            .filter(|&(row, col)| !self.input(sheet, row, col).is_empty())
            .collect();
        cells.sort_unstable();
        cells
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

    /// Set many cells from user input, as one undo step (Replace All).
    /// Recalculates once at the end. Stops at the first error; the cells
    /// set until then still undo as one step.
    pub fn set_inputs(&mut self, sheet: u32, cells: &[(u32, u32, String)]) -> Result<()> {
        if cells.is_empty() {
            return Ok(());
        }
        self.model.pause_evaluation();
        let mut done = 0;
        let mut result = Ok(());
        for (row, col, value) in cells {
            if *row >= MAX_ROWS || *col >= MAX_COLS {
                result = Err(anyhow!("cell out of range"));
                break;
            }
            if let Err(e) =
                self.model
                    .set_user_input(sheet, *row as i32 + 1, *col as i32 + 1, value)
            {
                result = Err(anyhow!("{e}"));
                break;
            }
            done += 1;
        }
        self.model.resume_evaluation();
        self.model.evaluate();
        if done > 0 {
            self.history.edited_steps(done);
        }
        result
    }

    /// The inputs of `range` (formulas as typed) as tab-separated text, the
    /// way spreadsheets copy cells to the clipboard.
    pub fn range_tsv(&self, sheet: u32, range: CellRange) -> String {
        let rows: Vec<Vec<String>> = (range.top..=range.bottom)
            .map(|row| {
                (range.left..=range.right)
                    .map(|col| self.input(sheet, row, col))
                    .collect()
            })
            .collect();
        tsv::format(&rows)
    }

    /// Clear the contents of `range` (styles stay), as one undo step.
    pub fn clear_range(&mut self, sheet: u32, range: CellRange) -> Result<()> {
        if range.bottom >= MAX_ROWS || range.right >= MAX_COLS {
            bail!("range out of bounds");
        }
        self.model
            .range_clear_contents(&range.area(sheet))
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(())
    }

    /// Paste tab-separated text with its top-left cell at `(row, col)`, as one
    /// undo step. Each cell is taken as user input, so `=formulas` stay
    /// formulas (their references are not shifted). Short rows are padded with
    /// empty cells, so the whole pasted rectangle is replaced. Returns the
    /// pasted range, or `None` if the text had no cells.
    pub fn paste_tsv(
        &mut self,
        sheet: u32,
        row: u32,
        col: u32,
        text: &str,
    ) -> Result<Option<CellRange>> {
        let rows = tsv::parse(text);
        let width = rows.iter().map(Vec::len).max().unwrap_or(0);
        if rows.is_empty() || width == 0 {
            return Ok(None);
        }
        let range = CellRange {
            top: row,
            left: col,
            bottom: row + rows.len() as u32 - 1,
            right: col + width as u32 - 1,
        };
        if range.bottom >= MAX_ROWS || range.right >= MAX_COLS {
            bail!(
                "pasting {} × {} cells at {} goes past the sheet's edge",
                rows.len(),
                width,
                cell_name(row, col)
            );
        }
        // IronCalc parses this back with a strict reader; see `format_rect_quoted`.
        let csv = tsv::format_rect_quoted(&rows, width);
        // `paste_csv_string` ends by selecting the pasted range in IronCalc's
        // own view state, which fails (after recording the history entry) if
        // that view's sheet and cell are elsewhere. Point the view at the
        // target first.
        self.model
            .set_selected_sheet(sheet)
            .and_then(|()| self.model.set_selected_cell(row as i32 + 1, col as i32 + 1))
            .map_err(|e| anyhow!("{e}"))?;
        self.model
            .paste_csv_string(&range.area(sheet), &csv)
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(Some(range))
    }
}

/// A rectangle of cells, 0-based and inclusive (`top..=bottom`, `left..=right`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRange {
    pub top: u32,
    pub left: u32,
    pub bottom: u32,
    pub right: u32,
}

impl CellRange {
    /// The range spanned by two corner cells `(row, col)`, in either order.
    pub fn from_corners(a: (u32, u32), b: (u32, u32)) -> Self {
        Self {
            top: a.0.min(b.0),
            left: a.1.min(b.1),
            bottom: a.0.max(b.0),
            right: a.1.max(b.1),
        }
    }

    pub fn contains(&self, row: u32, col: u32) -> bool {
        (self.top..=self.bottom).contains(&row) && (self.left..=self.right).contains(&col)
    }

    pub fn is_single(&self) -> bool {
        self.top == self.bottom && self.left == self.right
    }

    /// `A1` for one cell, `A1:C4` for a range.
    pub fn name(&self) -> String {
        let start = cell_name(self.top, self.left);
        if self.is_single() {
            start
        } else {
            format!("{start}:{}", cell_name(self.bottom, self.right))
        }
    }

    fn area(&self, sheet: u32) -> Area {
        Area {
            sheet,
            row: self.top as i32 + 1,
            column: self.left as i32 + 1,
            width: (self.right - self.left) as i32 + 1,
            height: (self.bottom - self.top) as i32 + 1,
        }
    }
}

impl Workbook {
    /// Each sheet's tab colour as `<tabColor>` attributes, by `sheetId`.
    fn tab_colors(&self) -> preserve::TabColors {
        self.model
            .get_model()
            .workbook
            .worksheets
            .iter()
            .map(|ws| {
                let attrs = match &ws.color {
                    Color::Rgb(hex) => hex
                        .strip_prefix('#')
                        .filter(|h| h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()))
                        .map(|h| format!("rgb=\"FF{}\"", h.to_ascii_uppercase())),
                    Color::Theme(slot, tint) if *tint == 0.0 => Some(format!("theme=\"{slot}\"")),
                    Color::Theme(slot, tint) => Some(format!("theme=\"{slot}\" tint=\"{tint}\"")),
                    Color::None => None,
                };
                (ws.sheet_id, attrs)
            })
            .collect()
    }
}

/// Load an `.xlsx`, keeping what part preservation needs: the bytes for small
/// files (read once, so the model and the kept bytes are the same file), the
/// path and its size/mtime for large ones.
fn open_xlsx(path: &Path) -> Result<(Model<'static>, XlsxSource)> {
    let meta = fs::metadata(path).with_context(|| format!("opening {}", path.display()))?;
    if meta.len() <= KEEP_IN_MEMORY {
        let bytes = fs::read(path).with_context(|| format!("opening {}", path.display()))?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let workbook = load_from_xlsx_bytes(&bytes, &name, LOCALE, TIMEZONE)
            .map_err(|e| anyhow!("opening {}: {e}", path.display()))?;
        let model = Model::from_workbook(workbook, LANGUAGE)
            .map_err(|e| anyhow!("opening {}: {e}", path.display()))?;
        Ok((model, XlsxSource::Bytes(bytes)))
    } else {
        let name = path.to_str().context("path is not valid UTF-8")?;
        let model = load_from_xlsx(name, LOCALE, TIMEZONE, LANGUAGE)
            .map_err(|e| anyhow!("opening {}: {e}", path.display()))?;
        Ok((model, XlsxSource::file(path, &meta)))
    }
}

/// Write a file via a temp file in the same directory and a rename.
fn write_atomic(path: &Path, write: impl FnOnce(&mut fs::File) -> Result<()>) -> Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().context("save path has no file name")?;
    let tmp = dir.join(format!(".{}.kantor-tmp", file_name.to_string_lossy()));
    let result = (|| {
        let mut file =
            fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        write(&mut file)?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(err) = result {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
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
    fn set_inputs_undoes_as_one_step() {
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(0, 0, 0, "cat").unwrap();
        wb.set_input(0, 2, 1, "a cat").unwrap();
        assert_eq!(wb.filled_cells(0), vec![(0, 0), (2, 1)]);
        let cells = vec![(0, 0, "dog".to_owned()), (2, 1, "a dog".to_owned())];
        wb.set_inputs(0, &cells).unwrap();
        assert_eq!(wb.input(0, 2, 1), "a dog");
        assert!(wb.undo().unwrap());
        assert_eq!(wb.input(0, 0, 0), "cat");
        assert_eq!(wb.input(0, 2, 1), "a cat");
        assert!(wb.redo().unwrap());
        assert_eq!(wb.input(0, 0, 0), "dog");
        assert_eq!(wb.input(0, 2, 1), "a dog");
        // Undo all the way back to the first edit, one step at a time.
        assert!(wb.undo().unwrap());
        assert!(wb.undo().unwrap());
        assert_eq!(wb.input(0, 2, 1), "");
        assert_eq!(wb.input(0, 0, 0), "cat");
        assert!(wb.undo().unwrap());
        assert!(!wb.is_dirty());
    }

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

    fn corpus(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/xlsx")
            .join(name)
    }

    fn zip_names(path: &Path) -> Vec<String> {
        let zip = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
        let mut names: Vec<String> = zip
            .file_names()
            .map(|n| n.unwrap().into_owned())
            .filter(|n| !n.ends_with('/'))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn xlsx_extras_survive_edit_and_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rich.xlsx");
        fs::write(&path, preserve::rich_fixture("")).unwrap();
        let mut wb = Workbook::open(&path).unwrap();
        wb.set_input(0, 2, 0, "new").unwrap();
        wb.save().unwrap();
        assert_eq!(wb.save_notes(), &[] as &[String]);
        let bytes = fs::read(&path).unwrap();
        assert_eq!(package_problems(&bytes), Vec::<String>::new());
        let names = zip_names(&path);
        for p in [
            "docProps/custom.xml",
            "xl/comments1.xml",
            "xl/charts/chart1.xml",
        ] {
            assert!(names.iter().any(|n| n == p), "{p} dropped: {names:?}");
        }
        let sheet1 = preserve::parts_of(&bytes)["xl/worksheets/sheet1.xml"].clone();
        let sheet1 = String::from_utf8(sheet1).unwrap();
        // The tab colour comes from the model (IronCalc itself never writes it).
        assert!(
            sheet1.contains(r#"<sheetPr codeName="Sheet1"><tabColor rgb="FF00FF00"/><pageSetUpPr fitToPage="1"/></sheetPr>"#),
            "{sheet1}"
        );

        // Save As elsewhere, then save again: still everything.
        let other = dir.path().join("copy.xlsx");
        wb.save_as(&other).unwrap();
        wb.set_input(0, 3, 0, "again").unwrap();
        wb.save().unwrap();
        assert_eq!(zip_names(&other), names);
        let back = Workbook::open(&other).unwrap();
        assert_eq!(back.display_value(0, 3, 0), "again");
    }

    #[test]
    fn unknown_error_codes_are_not_written() {
        // Q1 Summary!B6 is a 3-D reference IronCalc evaluates to #ERROR!.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("multi.xlsx");
        let mut wb = Workbook::open(corpus("02_multi_sheet.xlsx")).unwrap();
        wb.save_as(&path).unwrap();
        let parts = preserve::parts_of(&fs::read(&path).unwrap());
        assert!(
            parts
                .values()
                .all(|p| !String::from_utf8_lossy(p).contains("#ERROR!"))
        );
        let workbook = String::from_utf8(parts["xl/workbook.xml"].clone()).unwrap();
        assert!(workbook.contains(r#"<calcPr fullCalcOnLoad="1"/>"#));
        assert!(wb.save_notes().iter().any(|n| n.contains("error value")));
        // Every sheet is readable by a strict reader.
        let mut xlsx: calamine::Xlsx<_> = calamine::open_workbook(&path).unwrap();
        use calamine::Reader as _;
        for name in xlsx.sheet_names() {
            xlsx.worksheet_range(&name)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        // And the formula is still there.
        let back = Workbook::open(&path).unwrap();
        let q1 = back
            .sheet_names()
            .iter()
            .position(|n| n == "Q1 Summary")
            .unwrap();
        assert_eq!(back.input(q1 as u32, 5, 1), wb.input(q1 as u32, 5, 1));
    }

    #[test]
    fn changed_source_is_not_mixed_in() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.xlsx");
        fs::write(&path, preserve::rich_fixture("")).unwrap();
        let meta = fs::metadata(&path).unwrap();
        let source = XlsxSource::file(&path, &meta);
        assert!(source.bytes().is_ok());
        fs::write(&path, b"something else entirely").unwrap();
        assert!(source.bytes().unwrap_err().contains("changed on disk"));
        fs::remove_file(&path).unwrap();
        assert!(source.bytes().is_err());

        // A workbook whose large source vanished still saves, without extras.
        let mut wb = Workbook::open(corpus("12_comments_links_props.xlsx")).unwrap();
        wb.origin.xlsx = Some(source);
        let out = dir.path().join("out.xlsx");
        wb.save_as(&out).unwrap();
        assert!(wb.save_notes()[0].contains("not kept"));
        assert!(!zip_names(&out).iter().any(|n| n == "xl/comments1.xml"));
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
    fn ranges() {
        let r = CellRange::from_corners((3, 2), (0, 0));
        assert_eq!(r.name(), "A1:C4");
        assert!(r.contains(3, 2) && r.contains(0, 0) && !r.contains(4, 0));
        assert_eq!(CellRange::from_corners((1, 1), (1, 1)).name(), "B2");
    }

    #[test]
    fn copy_range_as_tsv() {
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(0, 0, 0, "Item").unwrap();
        wb.set_input(0, 0, 1, "2").unwrap();
        wb.set_input(0, 1, 1, "=B1*2").unwrap();
        wb.set_input(0, 1, 0, "two\nlines").unwrap();
        let r = CellRange::from_corners((0, 0), (1, 2));
        assert_eq!(wb.range_tsv(0, r), "Item\t2\t\n\"two\nlines\"\t=B1*2\t");
    }

    #[test]
    fn paste_is_one_undo_step() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.xlsx");
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(0, 5, 5, "old").unwrap();
        wb.set_input(0, 6, 6, "keep?").unwrap();
        wb.save_as(&path).unwrap();

        // Ragged rows, CRLF, a quoted multi-line cell and a formula.
        let text = "1\t2\r\n\"a\nb\"\r\n=F6+F7\t\t\"x\"\"y\"\r\n";
        let pasted = wb.paste_tsv(0, 5, 5, text).unwrap().unwrap();
        assert_eq!(pasted, CellRange::from_corners((5, 5), (7, 7)));
        assert_eq!(wb.input(0, 5, 5), "1");
        assert_eq!(wb.input(0, 5, 6), "2");
        assert_eq!(wb.input(0, 6, 5), "a\nb");
        assert_eq!(
            wb.input(0, 6, 6),
            "",
            "short rows clear the rest of the rectangle"
        );
        assert_eq!(wb.input(0, 7, 5), "=F6+F7");
        assert_eq!(wb.input(0, 7, 7), "x\"y");
        assert!(wb.is_dirty());

        assert!(wb.undo().unwrap());
        assert_eq!(wb.input(0, 5, 5), "old");
        assert_eq!(wb.input(0, 6, 6), "keep?");
        assert_eq!(wb.input(0, 7, 5), "");
        assert!(!wb.is_dirty(), "one undo returns to the saved state");
        assert!(wb.redo().unwrap());
        assert_eq!(wb.input(0, 6, 5), "a\nb");

        assert_eq!(wb.paste_tsv(0, 0, 0, "").unwrap(), None);
        assert!(wb.paste_tsv(0, MAX_ROWS - 1, 0, "a\nb").is_err());
    }

    #[test]
    fn paste_formula_recalculates() {
        let mut wb = Workbook::new_empty().unwrap();
        wb.paste_tsv(0, 0, 0, "2\t3\n=A1*B1").unwrap();
        assert_eq!(wb.display_value(0, 1, 0), "6");
    }

    #[test]
    fn clear_range_is_one_undo_step() {
        let mut wb = Workbook::new_empty().unwrap();
        wb.paste_tsv(0, 0, 0, "1\t2\n3\t4").unwrap();
        wb.set_input(0, 0, 2, "=A1+B2").unwrap();
        wb.clear_range(0, CellRange::from_corners((0, 0), (1, 1)))
            .unwrap();
        assert_eq!(wb.input(0, 1, 1), "");
        assert_eq!(wb.display_value(0, 0, 2), "0");
        assert!(wb.undo().unwrap());
        assert_eq!(wb.input(0, 1, 1), "4");
        assert_eq!(wb.display_value(0, 0, 2), "5");
        assert!(wb.undo().unwrap()); // the formula
        assert!(wb.undo().unwrap()); // the paste
        assert!(!wb.can_undo());
        assert!(!wb.is_dirty());
    }

    #[test]
    fn rejects_unknown_extension() {
        assert!(Workbook::open("notes.txt").is_err());
    }
}
