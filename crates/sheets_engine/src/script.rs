//! What macros need from a workbook beyond the app's own edits
//! (`crates/sheets_script`, the Sheets object model for VBA): typed cell
//! values, formulas evaluated without a cell (`WorksheetFunction`), copy and
//! paste with formats (`Range.Copy`, `PasteSpecial`), a run as one undo
//! step, recalculation deferred while a macro writes, the sheets' code names
//! (`Sheet1.Range(...)` in VBA), and user functions (VBA `Public Function`s
//! in cell formulas, see `KANTOR_PATCHES.md` §37).

use std::io::Read;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
pub use ironcalc_base::kantor_udf::{KantorUserFunctions as UserFunctions, KantorValue};

use crate::{CalcMode, CellRange, MAX_COLS, MAX_ROWS, Workbook};

fn check(range: CellRange) -> Result<()> {
    if range.bottom >= MAX_ROWS || range.right >= MAX_COLS || range.top > range.bottom {
        bail!("range out of bounds");
    }
    Ok(())
}

impl Workbook {
    /// A point in the undo history: [`Workbook::fold_undo`] makes everything
    /// after it one step.
    pub fn undo_mark(&self) -> usize {
        self.history.undo.len()
    }

    /// Fold every undo entry recorded since `mark` into one, so a macro run
    /// undoes and redoes as a single step (as [`Workbook::batch`]).
    pub fn fold_undo(&mut self, mark: usize) {
        let h = &mut self.history;
        if h.undo.len() > mark + 1 {
            let steps = h.undo[mark..].iter().map(|e| e.1).sum();
            let first = h.undo[mark].0;
            h.undo.truncate(mark);
            h.undo.push((first, steps));
        }
    }

    /// A macro starts writing: in automatic calculation, edits stop
    /// recalculating one by one; [`Workbook::script_sync`] brings values up
    /// to date before the macro reads, [`Workbook::script_end`] at the end.
    pub fn script_begin(&mut self) {
        if self.calc.mode == CalcMode::Automatic {
            self.model.pause_evaluation();
        }
    }

    /// Recalculate what the macro's edits so far changed (automatic
    /// calculation only).
    pub fn script_sync(&mut self) {
        if self.calc.mode == CalcMode::Automatic {
            self.model.kantor_evaluate_changes();
        }
    }

    /// The macro ended: automatic calculation goes on as before.
    pub fn script_end(&mut self) {
        if self.calc.mode == CalcMode::Automatic {
            self.model.resume_evaluation();
            self.model.kantor_evaluate_changes();
        }
    }

    /// A cell's value as stored (0-based), with its error by name.
    pub fn cell_value(&mut self, sheet: u32, row: u32, col: u32) -> KantorValue {
        if row >= MAX_ROWS || col >= MAX_COLS {
            return KantorValue::Empty;
        }
        self.model
            .kantor_cell_value(sheet, row as i32 + 1, col as i32 + 1)
    }

    /// The formula of a cell (`=SUM(A1:A3)`), `None` for a value.
    pub fn cell_formula(&self, sheet: u32, row: u32, col: u32) -> Option<String> {
        let model = self.model.get_model();
        let cell = model
            .workbook
            .worksheet(sheet)
            .ok()?
            .cell(row as i32 + 1, col as i32 + 1)?;
        cell.get_formula()?;
        self.model
            .get_cell_content(sheet, row as i32 + 1, col as i32 + 1)
            .ok()
            .filter(|f| f.starts_with('='))
    }

    /// Evaluates `formula` (English, `=` optional) as if it were in cell
    /// (`row`, `col`) of `sheet`, without storing it. Reads stored values:
    /// call [`Workbook::script_sync`] first.
    pub fn evaluate_formula(
        &mut self,
        sheet: u32,
        row: u32,
        col: u32,
        formula: &str,
    ) -> Result<KantorValue> {
        self.model
            .kantor_evaluate_formula(sheet, row as i32 + 1, col as i32 + 1, formula)
            .map_err(|e| anyhow!("{e}"))
    }

    /// Sets (or removes) the functions cell formulas may call besides the
    /// built-in ones (a workbook's VBA functions). Doesn't recalculate.
    pub fn set_user_functions(&mut self, functions: Option<Arc<dyn UserFunctions>>) {
        self.model.kantor_set_user_functions(functions);
    }

    /// Copy `src` of `src_sheet` (values, formulas with their references
    /// moved, formats, merges) to `dst_sheet` with its top-left cell at
    /// (`row`, `col`), as Excel's Copy and Paste; with `cut`, the source is
    /// cleared and references to it follow. One undo step. Returns where
    /// it went.
    pub fn copy_cells(
        &mut self,
        src_sheet: u32,
        src: CellRange,
        dst_sheet: u32,
        row: u32,
        col: u32,
        cut: bool,
    ) -> Result<CellRange> {
        check(src)?;
        let dst = CellRange {
            top: row,
            left: col,
            bottom: row + src.rows_count() - 1,
            right: col + src.cols_count() - 1,
        };
        check(dst)?;
        let e = |e: String| anyhow!("{e}");
        self.model.set_selected_sheet(src_sheet).map_err(e)?;
        self.model
            .set_selected_cell(src.top as i32 + 1, src.left as i32 + 1)
            .map_err(e)?;
        self.model
            .set_selected_range(
                src.top as i32 + 1,
                src.left as i32 + 1,
                src.bottom as i32 + 1,
                src.right as i32 + 1,
            )
            .map_err(e)?;
        let clip = self.model.copy_to_clipboard().map_err(e)?;
        self.model.set_selected_sheet(dst_sheet).map_err(e)?;
        self.model
            .set_selected_cell(row as i32 + 1, col as i32 + 1)
            .map_err(e)?;
        let tuple = (
            src.top as i32 + 1,
            src.left as i32 + 1,
            src.bottom as i32 + 1,
            src.right as i32 + 1,
        );
        self.model
            .paste_from_clipboard(src_sheet, tuple, &clip.data, cut)
            .map_err(e)?;
        self.history.edited();
        Ok(dst)
    }

    /// Paste the formats of `src` over `dst` (repeated to fill it, as Paste
    /// Special ▸ Formats does). One undo step.
    pub fn paste_formats(
        &mut self,
        src_sheet: u32,
        src: CellRange,
        dst_sheet: u32,
        dst: CellRange,
    ) -> Result<()> {
        check(src)?;
        check(dst)?;
        let e = |e: String| anyhow!("{e}");
        let mut styles = Vec::with_capacity(src.rows_count() as usize);
        for r in src.top..=src.bottom {
            let mut row = Vec::with_capacity(src.cols_count() as usize);
            for c in src.left..=src.right {
                row.push(
                    self.model
                        .get_cell_style(src_sheet, r as i32 + 1, c as i32 + 1)
                        .map_err(e)?,
                );
            }
            styles.push(row);
        }
        self.model.set_selected_sheet(dst_sheet).map_err(e)?;
        self.model
            .set_selected_cell(dst.top as i32 + 1, dst.left as i32 + 1)
            .map_err(e)?;
        self.model
            .set_selected_range(
                dst.top as i32 + 1,
                dst.left as i32 + 1,
                dst.bottom as i32 + 1,
                dst.right as i32 + 1,
            )
            .map_err(e)?;
        self.model.on_paste_styles(&styles).map_err(e)?;
        self.history.edited();
        Ok(())
    }

    /// Remove the formats of `range` (Clear Formats). One undo step.
    pub fn clear_formats(&mut self, sheet: u32, range: CellRange) -> Result<()> {
        check(range)?;
        self.model
            .range_clear_formatting(&range.area(sheet))
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(())
    }

    /// Remove contents and formats of `range` (Clear All). One undo step.
    pub fn clear_all(&mut self, sheet: u32, range: CellRange) -> Result<()> {
        check(range)?;
        self.model
            .range_clear_all(&range.area(sheet))
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(())
    }

    /// The height of a row in points (Excel's `RowHeight`).
    pub fn row_points(&self, sheet: u32, row: u32) -> f64 {
        let default = self.default_row_points(sheet);
        let Ok(ws) = self.model.get_model().workbook.worksheet(sheet) else {
            return default;
        };
        ws.rows
            .iter()
            .find(|r| r.r == row as i32 + 1)
            .map(|r| {
                if r.hidden {
                    0.0
                } else if r.height.is_finite() && r.height > 0.0 {
                    r.height
                } else {
                    default
                }
            })
            .unwrap_or(default)
    }

    /// The code names of the sheets of the file as it was opened or last
    /// saved (`<sheetPr codeName>`, what VBA's document modules are
    /// called), by sheet name. Empty for a new or imported workbook.
    pub fn sheet_code_names(&self) -> Vec<(String, String)> {
        let Some(bytes) = self.origin_bytes() else {
            return Vec::new();
        };
        code_names(&bytes).unwrap_or_default()
    }
}

/// `(sheet name, code name)` of every worksheet of package `bytes` that has
/// a code name.
fn code_names(bytes: &[u8]) -> Option<Vec<(String, String)>> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    let mut text = |name: &str, limit: u64| -> Option<String> {
        let entry = zip.by_name(name).ok()?;
        let mut s = String::new();
        entry.take(limit).read_to_string(&mut s).ok()?;
        Some(s)
    };
    let workbook = text("xl/workbook.xml", 4 << 20)?;
    let rels = text("xl/_rels/workbook.xml.rels", 4 << 20)?;
    let rels_doc = roxmltree::Document::parse(&rels).ok()?;
    let target = |id: &str| -> Option<String> {
        let r = rels_doc
            .root_element()
            .children()
            .find(|r| r.attribute("Id") == Some(id))?;
        let t = r.attribute("Target")?;
        Some(match t.strip_prefix('/') {
            Some(abs) => abs.to_owned(),
            None => format!("xl/{t}"),
        })
    };
    let doc = roxmltree::Document::parse(&workbook).ok()?;
    let sheets: Vec<(String, String)> = doc
        .descendants()
        .filter(|n| n.tag_name().name() == "sheet")
        .filter_map(|n| {
            let name = n.attribute("name")?;
            let id = n.attributes().find(|a| a.name() == "id")?.value();
            Some((name.to_owned(), target(id)?))
        })
        .collect();
    let mut out = Vec::new();
    for (name, part) in sheets {
        // `sheetPr` is the sheet's first child: the start is enough.
        let Some(head) = text(&part, 8192) else {
            continue;
        };
        let Some(at) = head.find("<sheetPr") else {
            continue;
        };
        let tag = &head[at..head[at..].find('>').map_or(head.len(), |e| at + e)];
        if let Some(v) = attr(tag, "codeName") {
            out.push((name, v));
        }
    }
    Some(out)
}

/// The value of attribute `name` in start tag `tag`.
fn attr(tag: &str, name: &str) -> Option<String> {
    let mut rest = tag;
    while let Some(i) = rest.find(name) {
        let before = rest[..i].chars().last();
        let after = &rest[i + name.len()..];
        if before.is_some_and(char::is_whitespace) {
            let after = after.trim_start();
            if let Some(v) = after.strip_prefix('=') {
                let v = v.trim_start();
                let q = v.chars().next()?;
                if q == '"' || q == '\'' {
                    let end = v[1..].find(q)?;
                    return Some(v[1..1 + end].to_owned());
                }
            }
        }
        rest = &rest[i + name.len()..];
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribute_values() {
        assert_eq!(
            attr(r#"<sheetPr filterMode="1" codeName="Sheet1""#, "codeName").as_deref(),
            Some("Sheet1")
        );
        assert_eq!(attr("<sheetPr xcodeName='x'", "codeName"), None);
        assert_eq!(
            attr("<sheetPr codeName = 'Data'", "codeName").as_deref(),
            Some("Data")
        );
    }

    #[test]
    fn copy_cells_moves_formulas_and_folds_undo() {
        let mut book = Workbook::new_empty().unwrap();
        book.set_input(0, 0, 0, "1").unwrap();
        book.set_input(0, 0, 1, "=A1*2").unwrap();
        let mark = book.undo_mark();
        book.script_begin();
        let dst = book
            .copy_cells(0, CellRange::from_corners((0, 0), (0, 1)), 0, 4, 0, false)
            .unwrap();
        assert_eq!(dst, CellRange::from_corners((4, 0), (4, 1)));
        book.set_input(0, 4, 0, "5").unwrap();
        book.script_sync();
        assert_eq!(book.input(0, 4, 1), "=A5*2");
        assert_eq!(book.cell_value(0, 4, 1), KantorValue::Number(10.0));
        book.script_end();
        book.fold_undo(mark);
        assert!(book.undo().unwrap());
        assert_eq!(book.input(0, 4, 0), "");
        assert_eq!(book.input(0, 4, 1), "");
        assert_eq!(book.input(0, 0, 1), "=A1*2");
    }

    #[test]
    fn formulas_evaluate_in_place() {
        let mut book = Workbook::new_empty().unwrap();
        book.set_input(0, 0, 0, "2").unwrap();
        book.set_input(0, 1, 0, "3").unwrap();
        assert_eq!(
            book.evaluate_formula(0, 0, 0, "SUM(A1:A2)").unwrap(),
            KantorValue::Number(5.0)
        );
        assert_eq!(book.cell_formula(0, 0, 0), None);
    }
}
