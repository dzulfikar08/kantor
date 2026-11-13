//! Opening and writing formats other than `.xlsx`.
//!
//! * **CSV / TSV** are read and written. One file is one sheet. On import, each
//!   field goes through IronCalc's input parsing (as if typed), so `12`, `3.5`,
//!   `TRUE`, `50%` or `$4` become numbers/booleans. Two exceptions are stored
//!   as plain text: fields starting with `=` (CSV holds data, not formulas, and
//!   evaluating them is the classic "CSV injection" hole) and fields starting
//!   with `'` (kept verbatim). Export writes the **displayed** values (number
//!   formats applied, formulas as their results) of **one** sheet.
//! * **ODS / XLS / XLSB** are read-only imports via [calamine]. Values keep
//!   their types; dates become date-formatted numbers. Formulas are imported
//!   where calamine exposes them: XLS/XLSB formulas are already in Excel syntax,
//!   ODS ones (`of:=SUM([.A1:.B2])`) are converted (references, `;` → `,`).
//!   A formula that fails to evaluate in IronCalc while the file's cached value
//!   was not an error is replaced by that cached value, and counted in
//!   [`Workbook::import_notes`](crate::Workbook::import_notes).
//! * **XLS / XLT** (Excel 97-2003, BIFF8) also keep their look: [`crate::xls`]
//!   reads the formatting calamine skips (cell formats, merges, column widths,
//!   row heights, frozen panes, gridlines, hidden and coloured tabs,
//!   hyperlinks, page setup, sheet-local and print names, the 1904 date
//!   system). Comments, data validation, conditional formatting, charts and
//!   pictures are not imported. Older BIFF5 files (Excel 5/95) get values
//!   only.
//! * ODS and XLSB imports bring values and formulas only: no styles, merged
//!   cells, column widths, comments or charts.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result, anyhow, bail};
use calamine::{Data, Reader, open_workbook_auto};
use ironcalc_base::Model;
use ironcalc_base::cell::CellValue;
use ironcalc_base::types::CellType;

use crate::print::SheetPrint;
use crate::xls::{DAYS_1904, XlsFormat};
use crate::{LANGUAGE, LOCALE, MAX_COLS, MAX_ROWS, TIMEZONE, csv};

/// The kind of file a [`Workbook`](crate::Workbook) was opened from, which
/// decides what Save does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceFormat {
    /// Created in Kantor; Save asks for a path.
    #[default]
    New,
    /// `.xlsx` / `.xlsm`: Save writes the file back.
    Xlsx,
    /// Delimited text; Save writes the active sheet's values back with the
    /// same delimiter. `.tsv` files use tab.
    Csv {
        delimiter: u8,
    },
    /// Read-only imports: [`Workbook::path`](crate::Workbook::path) is `None`,
    /// so Save goes to Save As (suggesting `name.xlsx`); the source is never
    /// overwritten.
    Ods,
    Xls,
    Xlsb,
}

impl SourceFormat {
    /// The format for a file extension (case-insensitive), if Kantor opens it.
    pub fn from_extension(ext: &str) -> Option<Self> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "xlsx" | "xlsm" => Self::Xlsx,
            "csv" => Self::Csv { delimiter: b',' },
            "tsv" | "tab" => Self::Csv { delimiter: b'\t' },
            "ods" => Self::Ods,
            "xls" | "xla" | "xlt" => Self::Xls,
            "xlsb" => Self::Xlsb,
            _ => return None,
        })
    }

    /// Opened read-only from a format Kantor does not write.
    pub fn is_imported(self) -> bool {
        matches!(self, Self::Ods | Self::Xls | Self::Xlsb)
    }

    /// A short name for storing (recovery manifests): `xlsx`, `csv:59`, `ods`.
    pub fn tag(self) -> String {
        match self {
            Self::New => "new".into(),
            Self::Xlsx => "xlsx".into(),
            Self::Csv { delimiter } => format!("csv:{delimiter}"),
            Self::Ods => "ods".into(),
            Self::Xls => "xls".into(),
            Self::Xlsb => "xlsb".into(),
        }
    }

    /// The format [`Self::tag`] names.
    pub fn from_tag(tag: &str) -> Option<Self> {
        Some(match tag {
            "new" => Self::New,
            "xlsx" => Self::Xlsx,
            "ods" => Self::Ods,
            "xls" => Self::Xls,
            "xlsb" => Self::Xlsb,
            _ => Self::Csv {
                delimiter: tag.strip_prefix("csv:")?.parse().ok()?,
            },
        })
    }
}

fn empty_model(name: &'static str) -> Result<Model<'static>> {
    Model::new_empty(name, LOCALE, TIMEZONE, LANGUAGE)
        .map_err(|e| anyhow!("creating workbook: {e}"))
}

#[cfg(test)]
pub(crate) fn empty_model_for_tests() -> Model<'static> {
    let mut m = empty_model("Untitled").unwrap();
    m.rename_sheet("Sheet1", "Report").unwrap();
    m
}

fn sheet_name_from(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Excel's sheet name rules: 1-31 chars, none of []:*?/\
    let clean: String = stem
        .chars()
        .filter(|c| !"[]:*?/\\".contains(*c))
        .take(31)
        .collect();
    let clean = clean.trim_matches('\'').to_owned();
    if clean.is_empty() {
        "Sheet1".to_owned()
    } else {
        clean
    }
}

/// Read a CSV/TSV file into a one-sheet model (not yet evaluated). `delimiter`
/// forces one; `None` detects it. Returns the delimiter used.
pub(crate) fn read_csv(path: &Path, delimiter: Option<u8>) -> Result<(Model<'static>, u8)> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let text = csv::decode(&bytes);
    let delimiter = delimiter.unwrap_or_else(|| csv::detect_delimiter(&text));
    let records = csv::parse(&text, delimiter);
    if records.len() > MAX_ROWS as usize {
        bail!("{} has more than {MAX_ROWS} rows", path.display());
    }
    let mut model = empty_model("Untitled")?;
    let sheet_name = sheet_name_from(path);
    if sheet_name != "Sheet1" {
        model
            .rename_sheet("Sheet1", &sheet_name)
            .map_err(|e| anyhow!("naming sheet: {e}"))?;
    }
    for (r, record) in records.iter().enumerate() {
        if record.len() > MAX_COLS as usize {
            bail!("row {} has more than {MAX_COLS} columns", r + 1);
        }
        for (c, field) in record.iter().enumerate() {
            if field.is_empty() {
                continue;
            }
            let (row, col) = (r as i32 + 1, c as i32 + 1);
            let res = if field.starts_with(['=', '\'']) {
                model.update_cell_with_text(0, row, col, field)
            } else {
                model.set_user_input(0, row, col, field.clone())
            };
            res.map_err(|e| anyhow!("cell {}: {e}", crate::cell_name(r as u32, c as u32)))?;
        }
    }
    Ok((model, delimiter))
}

/// Write `sheet`'s displayed values as delimited text, atomically.
pub(crate) fn write_csv(model: &Model<'_>, sheet: u32, path: &Path, delimiter: u8) -> Result<()> {
    let ws = model
        .workbook
        .worksheet(sheet)
        .map_err(|e| anyhow!("{e}"))?;
    let dim = ws.dimension();
    let (rows, cols) = (dim.max_row.max(0), dim.max_column.max(0));
    let mut out = String::new();
    let mut fields = Vec::with_capacity(cols as usize);
    let mut formats = crate::numfmt::Formats::default();
    for row in 1..=rows {
        fields.clear();
        for col in 1..=cols {
            let style = ws.cell(row, col).map_or(0, |c| c.get_style());
            let fmt = formats.get(model, style);
            fields.push(crate::numfmt::display_cell(model, sheet, row, col, fmt).text);
        }
        csv::write_record(&mut out, &fields, delimiter);
    }
    crate::write_atomic(path, |file| {
        use std::io::Write as _;
        file.write_all(out.as_bytes())?;
        Ok(())
    })
}

/// What an import brings besides the model.
#[derive(Default)]
pub(crate) struct Imported {
    /// What could not be imported faithfully.
    pub notes: Vec<String>,
    /// Each sheet's print settings (`.xls`), by sheet index.
    pub prints: Vec<SheetPrint>,
    /// `Print_Area` and `Print_Titles` formulas, by sheet index.
    pub print_names: HashMap<u32, (Option<String>, Option<String>)>,
    /// The tab the file opens on.
    pub active_sheet: Option<u32>,
}

/// Read an `.ods` / `.xls` / `.xlsb` file via calamine (and `.xls`
/// formatting via [`crate::xls`]). Returns the evaluated model.
pub(crate) fn read_calamine(
    path: &Path,
    format: SourceFormat,
) -> Result<(Model<'static>, Imported)> {
    // The formatting pass reads the file again, meanwhile.
    let reading = (format == SourceFormat::Xls).then(|| {
        let path = path.to_path_buf();
        std::thread::spawn(move || XlsFormat::read(&path))
    });
    let mut book =
        open_workbook_auto(path).map_err(|e| anyhow!("opening {}: {e}", path.display()))?;
    let names = book.sheet_names();
    if names.is_empty() {
        bail!("{} has no sheets", path.display());
    }
    let mut imported = Imported::default();
    let read = reading.map(|h| {
        h.join()
            .unwrap_or_else(|_| Err(anyhow!("the formatting reader failed")))
    });
    let xls = match read {
        Some(read) => match read {
            Ok(x) => Some(x),
            Err(e) => {
                log::warn!("{}: formatting not read: {e}", path.display());
                imported
                    .notes
                    .push("formatting not imported (not an Excel 97-2003 file)".into());
                None
            }
        },
        None => None,
    };
    let date1904 = xls.as_ref().is_some_and(|x| x.globals.date1904);
    // With the file's own formats coming, dates need no format of ours.
    let styled = xls.is_some();
    let mut model = empty_model("Untitled")?;
    // Formula cells and their cached values, for the check below.
    let mut formulas: Vec<(u32, i32, i32, Data, bool)> = Vec::new();
    let mut kept_values = 0;

    for (ix, name) in names.iter().enumerate() {
        if ix == 0 {
            model
                .rename_sheet("Sheet1", name)
                .map_err(|e| anyhow!("sheet {name:?}: {e}"))?;
        } else {
            model
                .add_sheet(name)
                .map_err(|e| anyhow!("sheet {name:?}: {e}"))?;
        }
    }
    // Names before formulas: a formula using an unknown name doesn't parse.
    if let Some(x) = &xls {
        add_xls_names(&mut model, &book, x, &names, &mut imported);
    }
    for (ix, name) in names.iter().enumerate() {
        let sheet = ix as u32;
        let values = book
            .worksheet_range(name)
            .map_err(|e| anyhow!("reading sheet {name:?}: {e}"))?;
        if let Some((r0, c0)) = values.start() {
            for (r, c, value) in values.used_cells() {
                let (row, col) = cell_index(r0 as usize + r, c0 as usize + c)?;
                set_value(&mut model, sheet, row, col, value, date1904, styled).map_err(|e| {
                    anyhow!(
                        "{name}!{}: {e}",
                        crate::cell_name(row as u32 - 1, col as u32 - 1)
                    )
                })?;
            }
        }
        // Not every reader exposes formulas; values alone are still a usable import.
        let Ok(range) = book.worksheet_formula(name) else {
            continue;
        };
        let Some((r0, c0)) = range.start() else {
            continue;
        };
        for (r, c, formula) in range.used_cells() {
            if formula.is_empty() {
                continue;
            }
            let (row, col) = cell_index(r0 as usize + r, c0 as usize + c)?;
            let cached = values
                .get_value((row as u32 - 1, col as u32 - 1))
                .cloned()
                .unwrap_or_default();
            let formula = excel_formula(formula).map(|f| match format {
                SourceFormat::Xls => repair_xls_ref3d(&f, &names),
                _ => f,
            });
            let ok = formula.is_some_and(|f| {
                let volatile = is_volatile(&f);
                let ok = model.set_user_input(sheet, row, col, f).is_ok();
                if ok {
                    formulas.push((sheet, row, col, cached.clone(), volatile));
                }
                ok
            });
            if !ok {
                keep_cached(&mut model, sheet, row, col, &cached, date1904)?;
                kept_values += 1;
            }
        }
    }

    // Trust the file's own results: a formula that errors here although it
    // didn't in the file, or computes something else, is replaced by the value
    // the file stored (unsupported function, syntax we mistranslated, ...).
    model.evaluate();
    let mut replaced = false;
    for (sheet, row, col, cached, volatile) in formulas {
        let errored = matches!(
            model.get_cell_type(sheet, row, col),
            Ok(CellType::ErrorValue)
        );
        let differs = errored && !matches!(cached, Data::Error(_))
            || !volatile && same_result(&model, sheet, row, col, &cached, date1904) == Some(false);
        if differs {
            keep_cached(&mut model, sheet, row, col, &cached, date1904)?;
            kept_values += 1;
            replaced = true;
        }
    }
    // After the check: clearing the covered cells of merges changes values.
    if let Some(x) = &xls {
        let (prints, cleared) = x.apply(&mut model, &names);
        imported.prints = prints;
        imported.active_sheet = Some(x.globals.active).filter(|&a| (a as usize) < names.len());
        replaced |= cleared;
    }
    if replaced {
        model.evaluate();
    }
    if kept_values > 0 {
        imported.notes.push(format!(
            "{kept_values} formula{} kept as values",
            if kept_values == 1 { "" } else { "s" }
        ));
    }
    Ok((model, imported))
}

/// The file's defined names: calamine decodes their formulas, the `LBL`
/// records (same order) say which are sheet-local or built in. Print areas
/// and titles go to the print settings; other built-in names are dropped.
fn add_xls_names<R: std::io::Read + std::io::Seek>(
    model: &mut Model<'_>,
    book: &calamine::Sheets<R>,
    x: &XlsFormat,
    sheets: &[String],
    imported: &mut Imported,
) {
    let decoded = book.defined_names();
    for (label, (_, formula)) in x.globals.labels.iter().zip(decoded) {
        // calamine decodes name formulas without the cell formulas' PtgRef3d
        // slip (see `repair_xls_ref3d`).
        let Some(formula) = excel_formula(formula) else {
            continue;
        };
        let formula = formula.trim_start_matches('=').to_owned();
        // LibreOffice also writes print areas as ordinary `_xlnm.` names.
        if formula.contains("#REF!")
            || label.name.is_empty()
            || label.name.to_ascii_lowercase().starts_with("_xlnm.")
        {
            continue;
        }
        let sheet = label.sheet.filter(|&s| (s as usize) < sheets.len());
        match label.builtin {
            Some(code @ (6 | 7)) => {
                let Some(sheet) = sheet else { continue };
                let formula = whole_rows_and_columns(&formula);
                let entry = imported.print_names.entry(sheet).or_default();
                if code == 6 {
                    entry.0 = Some(formula);
                } else {
                    entry.1 = Some(formula);
                }
            }
            Some(_) => {}
            None => {
                // IronCalc checks the name and parses its formula. Names it
                // can't take (constants, unions) are left out.
                if let Err(e) = model.new_defined_name(&label.name, sheet, &formula) {
                    log::info!("name {} = {formula}: {e}", label.name);
                }
            }
        }
    }
}

/// BIFF8 has no whole-row or whole-column references: print titles are
/// stored as `Sheet!$A$1:$IV$2` (all 256 columns) or `$A$1:$B$65536`. Write
/// them as `Sheet!$1:$2` and `Sheet!$A:$B`, as `.xlsx` does.
fn whole_rows_and_columns(formula: &str) -> String {
    let parts: Vec<String> = split_outside_quotes(formula, ',')
        .into_iter()
        .map(|part| {
            let (prefix, area) = match part.rsplit_once('!') {
                Some((sheet, area)) => (format!("{sheet}!"), area),
                None => (String::new(), part),
            };
            let cell = |c: &str| -> Option<(String, u32)> {
                let c = c.trim_start_matches('$');
                let n = c.bytes().take_while(u8::is_ascii_alphabetic).count();
                let (col, row) = c.split_at(n);
                Some((col.to_owned(), row.trim_start_matches('$').parse().ok()?))
            };
            let whole = area.split_once(':').and_then(|(a, b)| {
                let ((c1, r1), (c2, r2)) = (cell(a)?, cell(b)?);
                if c1 == "A" && c2 == "IV" {
                    Some(format!("${r1}:${r2}"))
                } else if r1 == 1 && r2 == 65536 {
                    Some(format!("${c1}:${c2}"))
                } else {
                    None
                }
            });
            match whole {
                Some(w) => format!("{prefix}{w}"),
                None => part.to_owned(),
            }
        })
        .collect();
    parts.join(",")
}

/// A formula that can't be imported: store the file's cached value instead.
fn keep_cached(
    model: &mut Model<'_>,
    sheet: u32,
    row: i32,
    col: i32,
    cached: &Data,
    date1904: bool,
) -> Result<()> {
    model
        .set_user_input(sheet, row, col, String::new())
        .map_err(|e| anyhow!("{e}"))?;
    set_value(model, sheet, row, col, cached, date1904, false).map_err(|e| anyhow!("{e}"))
}

/// A calamine date as a serial in the 1900 date system (Kantor's).
fn date_serial(dt: &calamine::ExcelDateTime, date1904: bool) -> f64 {
    if date1904 && dt.is_datetime() {
        dt.as_f64() + DAYS_1904
    } else {
        dt.as_f64()
    }
}

/// Results that legitimately change between recalculations.
fn is_volatile(formula: &str) -> bool {
    let upper = formula.to_ascii_uppercase();
    ["NOW(", "TODAY(", "RAND(", "RANDBETWEEN(", "RANDARRAY("]
        .iter()
        .any(|f| upper.contains(f))
}

/// Whether IronCalc's result equals the file's cached one; `None` if the file
/// stored no usable result (some writers leave it empty).
fn same_result(
    model: &Model<'_>,
    sheet: u32,
    row: i32,
    col: i32,
    cached: &Data,
    date1904: bool,
) -> Option<bool> {
    let value = model.get_cell_value_by_index(sheet, row, col).ok()?;
    let number = |n: f64| match value {
        CellValue::Number(v) => (v - n).abs() <= 1e-9 * v.abs().max(n.abs()).max(1.0),
        _ => false,
    };
    Some(match cached {
        Data::Empty => return None,
        Data::String(s) if s.is_empty() => return None,
        Data::String(s) => matches!(&value, CellValue::String(v) if v == s),
        Data::Int(n) => number(*n as f64),
        Data::Float(n) => number(*n),
        Data::Bool(b) => matches!(value, CellValue::Boolean(v) if v == *b),
        Data::DateTime(dt) => number(date_serial(dt, date1904)),
        Data::DateTimeIso(iso) => number(iso_datetime_to_serial(iso)?),
        Data::DurationIso(iso) => number(iso_duration_to_days(iso)?),
        Data::Error(_) => matches!(
            model.get_cell_type(sheet, row, col),
            Ok(CellType::ErrorValue)
        ),
    })
}

/// Work around calamine 0.38's XLS `PtgRef3d` decoding (a reference to a cell
/// on another sheet): it prints the column shifted left by two bits and takes
/// the `$` markers from the column's low bits, so `Data!B4` comes out as
/// `Data!E$4`. The column is recoverable (÷ 4); the absolute markers are not,
/// so the reference becomes relative. Sheet names are also quoted when they
/// need it. Ranges (`PtgArea3d`, always printed `Sheet!$A$1:$B$2`) are left to
/// the cached-value check. If calamine fixes this, the `imports_xls` test fails.
fn repair_xls_ref3d(formula: &str, sheets: &[String]) -> String {
    let mut sheets: Vec<&str> = sheets.iter().map(String::as_str).collect();
    sheets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut out = String::with_capacity(formula.len());
    let mut rest = formula;
    let mut in_string = false;
    let mut prev: Option<char> = None;
    while let Some(c) = rest.chars().next() {
        if c == '"' {
            in_string = !in_string;
        }
        let at_token_start = prev.is_none_or(|p| !(p.is_alphanumeric() || "_.$!'".contains(p)));
        if !in_string && at_token_start {
            let hit = sheets.iter().find_map(|name| {
                let after = rest.strip_prefix(name)?.strip_prefix('!')?;
                Some((*name, after))
            });
            if let Some((name, after)) = hit {
                out.push_str(&quote_sheet(name));
                out.push('!');
                rest = after;
                if let Some((cell, after)) = fix_ref3d_cell(rest) {
                    out.push_str(&cell);
                    rest = after;
                }
                prev = Some('!');
                continue;
            }
        }
        out.push(c);
        prev = Some(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// `$?COL$?ROW` (not followed by `:`) with the column ÷ 4, as `COLROW`.
fn fix_ref3d_cell(s: &str) -> Option<(String, &str)> {
    let t = s.strip_prefix('$').unwrap_or(s);
    let letters = t.bytes().take_while(u8::is_ascii_uppercase).count();
    let (col, t) = t.split_at(letters);
    let t = t.strip_prefix('$').unwrap_or(t);
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    let (row, after) = t.split_at(digits);
    if letters == 0 || digits == 0 || after.starts_with([':', '(']) {
        return None;
    }
    let index = col
        .bytes()
        .fold(0u32, |n, b| n * 26 + u32::from(b - b'A' + 1))
        - 1;
    if index % 4 != 0 {
        return None;
    }
    Some((format!("{}{row}", crate::column_name(index / 4)), after))
}

/// calamine's 0-based `(row, col)` to IronCalc's 1-based, checked.
fn cell_index(row: usize, col: usize) -> Result<(i32, i32)> {
    if row >= MAX_ROWS as usize || col >= MAX_COLS as usize {
        bail!("cell beyond the sheet limits");
    }
    Ok((row as i32 + 1, col as i32 + 1))
}

/// Store a typed calamine value. Text stays text (even if it looks like a
/// number), dates and times become numbers with a date/time format (unless
/// `styled`: the file's own formats are applied afterwards). `date1904`:
/// the file counts days from 1904; Kantor from 1900.
fn set_value(
    model: &mut Model<'_>,
    sheet: u32,
    row: i32,
    col: i32,
    value: &Data,
    date1904: bool,
    styled: bool,
) -> Result<(), String> {
    match value {
        Data::Empty => Ok(()),
        Data::String(s) => model.update_cell_with_text(sheet, row, col, s),
        Data::Int(n) => model.update_cell_with_number(sheet, row, col, *n as f64),
        Data::Float(n) => model.update_cell_with_number(sheet, row, col, *n),
        Data::Bool(b) => model.update_cell_with_bool(sheet, row, col, *b),
        Data::Error(e) => model.set_user_input(sheet, row, col, e.to_string()),
        Data::DateTime(dt) if styled => {
            model.update_cell_with_number(sheet, row, col, date_serial(dt, date1904))
        }
        Data::DateTime(dt) => {
            let serial = date_serial(dt, date1904);
            let format = if dt.is_duration() {
                "[h]:mm:ss"
            } else {
                date_format(serial)
            };
            set_number_with_format(model, sheet, row, col, serial, format)
        }
        Data::DateTimeIso(iso) => match iso_datetime_to_serial(iso) {
            Some(serial) => {
                set_number_with_format(model, sheet, row, col, serial, date_format(serial))
            }
            None => model.update_cell_with_text(sheet, row, col, iso),
        },
        Data::DurationIso(iso) => match iso_duration_to_days(iso) {
            Some(days) => set_number_with_format(model, sheet, row, col, days, "[h]:mm:ss"),
            None => model.update_cell_with_text(sheet, row, col, iso),
        },
    }
}

fn date_format(serial: f64) -> &'static str {
    if serial.fract() == 0.0 {
        "yyyy-mm-dd"
    } else if serial.trunc() == 0.0 {
        "hh:mm:ss"
    } else {
        "yyyy-mm-dd hh:mm:ss"
    }
}

fn set_number_with_format(
    model: &mut Model<'_>,
    sheet: u32,
    row: i32,
    col: i32,
    value: f64,
    format: &str,
) -> Result<(), String> {
    model.update_cell_with_number(sheet, row, col, value)?;
    let mut style = model.get_style_for_cell(sheet, row, col)?;
    style.num_fmt = format.to_owned();
    model.set_cell_style(sheet, row, col, &style)
}

/// `2024-01-15`, `2024-01-15T13:30:00(.5)` → Excel serial (1900 system).
fn iso_datetime_to_serial(iso: &str) -> Option<f64> {
    let (date, time) = match iso.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (iso, None),
    };
    let mut parts = date.splitn(3, '-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Excel day 0 is 1899-12-30 (counting the fictional 1900-02-29).
    let days = days_from_civil(y, m, d) - days_from_civil(1899, 12, 30);
    let mut serial = days as f64;
    if let Some(time) = time {
        let time = time.trim_end_matches('Z');
        let mut hms = time.splitn(3, ':');
        let h: f64 = hms.next()?.parse().ok()?;
        let mi: f64 = hms.next().unwrap_or("0").parse().ok()?;
        let s: f64 = hms.next().unwrap_or("0").parse().ok()?;
        serial += (h * 3600.0 + mi * 60.0 + s) / 86_400.0;
    }
    Some(serial)
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `PT36H30M15S` / `P1DT2H` → days.
fn iso_duration_to_days(iso: &str) -> Option<f64> {
    let (neg, rest) = match iso.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, iso),
    };
    let rest = rest.strip_prefix('P')?;
    let mut seconds = 0.0;
    let mut num = String::new();
    let mut in_time = false;
    for c in rest.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' | '.' | ',' => num.push(if c == ',' { '.' } else { c }),
            unit => {
                let n: f64 = num.parse().ok()?;
                num.clear();
                seconds += n * match (unit, in_time) {
                    ('D', false) => 86_400.0,
                    ('H', true) => 3600.0,
                    ('M', true) => 60.0,
                    ('S', true) => 1.0,
                    _ => return None,
                };
            }
        }
    }
    if !num.is_empty() {
        return None;
    }
    let days = seconds / 86_400.0;
    Some(if neg { -days } else { days })
}

/// A calamine formula as an IronCalc input (`=...`), or `None` if calamine
/// could not decode it. XLS/XLSB formulas come without `=`; ODS ones carry a
/// namespace prefix (`of:=`) and OpenFormula syntax.
fn excel_formula(raw: &str) -> Option<String> {
    if raw.contains("Unsupported ptg") {
        return None;
    }
    if let Some((ns, body)) = raw.split_once(":=")
        && !ns.is_empty()
        && ns.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return Some(format!("={}", open_formula_to_excel(body)));
    }
    Some(match raw.strip_prefix('=') {
        Some(_) => raw.to_owned(),
        None => format!("={raw}"),
    })
}

/// Convert OpenFormula (ODF) syntax to Excel syntax: `[.A1]` → `A1`,
/// `[.A1:.B2]` → `A1:B2`, `[$Sheet2.$A$1]` → `Sheet2!$A$1`, `;` → `,`.
/// String literals are copied unchanged. Other differences (`~` unions,
/// ODF-only functions) are left as they are; such formulas fail to evaluate
/// and fall back to cached values.
pub(crate) fn open_formula_to_excel(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                out.push('"');
                // Copy up to the closing quote; "" is an escaped quote.
                while let Some(c) = chars.next() {
                    out.push(c);
                    if c == '"' {
                        if chars.peek() == Some(&'"') {
                            out.push(chars.next().unwrap_or('"'));
                        } else {
                            break;
                        }
                    }
                }
            }
            '[' => {
                let mut inner = String::new();
                let mut in_quote = false;
                for c in chars.by_ref() {
                    if c == '\'' {
                        in_quote = !in_quote;
                    } else if c == ']' && !in_quote {
                        break;
                    }
                    inner.push(c);
                }
                out.push_str(&convert_reference(&inner));
            }
            ';' => out.push(','),
            c => out.push(c),
        }
    }
    out
}

/// The inside of an ODF `[...]` reference.
fn convert_reference(inner: &str) -> String {
    let mut sheet_of_first: Option<String> = None;
    let mut out = String::new();
    for (i, part) in split_outside_quotes(inner, ':').into_iter().enumerate() {
        let (sheet, cell) = match rfind_outside_quotes(part, '.') {
            Some(dot) => (&part[..dot], &part[dot + 1..]),
            None => ("", part),
        };
        let sheet = sheet.trim_start_matches('$');
        if i > 0 {
            out.push(':');
        }
        if !sheet.is_empty() && (i == 0 || sheet_of_first.as_deref() != Some(sheet)) {
            out.push_str(&quote_sheet(sheet));
            out.push('!');
        }
        if i == 0 {
            sheet_of_first = Some(sheet.to_owned());
        }
        out.push_str(cell);
    }
    out
}

fn split_outside_quotes(s: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_quote = false;
    for (i, c) in s.char_indices() {
        if c == '\'' {
            in_quote = !in_quote;
        } else if c == sep && !in_quote {
            parts.push(&s[start..i]);
            start = i + c.len_utf8();
        }
    }
    parts.push(&s[start..]);
    parts
}

fn rfind_outside_quotes(s: &str, needle: char) -> Option<usize> {
    let mut in_quote = false;
    let mut found = None;
    for (i, c) in s.char_indices() {
        if c == '\'' {
            in_quote = !in_quote;
        } else if c == needle && !in_quote {
            found = Some(i);
        }
    }
    found
}

/// Quote a sheet name for an Excel reference if it needs it.
fn quote_sheet(name: &str) -> String {
    if name.starts_with('\'') {
        return name.to_owned();
    }
    let plain = name.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit());
    if plain {
        name.to_owned()
    } else {
        format!("'{}'", name.replace('\'', "''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_formula_references() {
        let cases = [
            ("SUM([.A1:.B2])", "SUM(A1:B2)"),
            ("[.B4]*2", "B4*2"),
            ("[$Data.B4]+1", "Data!B4+1"),
            ("[$Data.$A$1:.$B$2]", "Data!$A$1:$B$2"),
            ("['My Sheet'.A1]", "'My Sheet'!A1"),
            ("[$'It''s'.C3]", "'It''s'!C3"),
            ("IF([.A1]>0;\"a;[.b]\";\"\")", "IF(A1>0,\"a;[.b]\",\"\")"),
            ("[Sheet 2.A1]", "'Sheet 2'!A1"),
        ];
        for (ods, excel) in cases {
            assert_eq!(open_formula_to_excel(ods), excel, "{ods}");
        }
    }

    #[test]
    fn repairs_calamine_xls_ref3d() {
        let sheets = vec!["Data".to_owned(), "My Data".to_owned()];
        let cases = [
            ("Data!E$4+1", "Data!B4+1"),
            ("Data!$A1", "Data!A1"),
            ("SUM(My Data!I$2,1)", "SUM('My Data'!C2,1)"),
            ("SUM(Data!$A$1:$B$2)", "SUM(Data!$A$1:$B$2)"),
            ("\"Data!E4\"&Data!E4", "\"Data!E4\"&Data!B4"),
            ("MyData!E4", "MyData!E4"),
        ];
        for (calamine, fixed) in cases {
            assert_eq!(repair_xls_ref3d(calamine, &sheets), fixed, "{calamine}");
        }
    }

    #[test]
    fn biff_print_titles_become_whole_rows() {
        assert_eq!(whole_rows_and_columns("Report!$A$1:$IV$2"), "Report!$1:$2");
        assert_eq!(
            whole_rows_and_columns("'My Sheet'!$A$1:$B$65536,'My Sheet'!$A$1:$IV$1"),
            "'My Sheet'!$A:$B,'My Sheet'!$1:$1"
        );
        assert_eq!(
            whole_rows_and_columns("Report!$A$1:$F$16"),
            "Report!$A$1:$F$16"
        );
    }

    #[test]
    fn formula_prefixes() {
        assert_eq!(excel_formula("of:=[.A1]").as_deref(), Some("=A1"));
        assert_eq!(excel_formula("SUM(A1:A3)").as_deref(), Some("=SUM(A1:A3)"));
        assert_eq!(excel_formula("=A1").as_deref(), Some("=A1"));
        assert_eq!(excel_formula("Unsupported ptg: 3c"), None);
    }

    #[test]
    fn iso_dates_and_durations() {
        assert_eq!(iso_datetime_to_serial("2024-01-15"), Some(45306.0));
        assert_eq!(iso_datetime_to_serial("1900-03-01"), Some(61.0));
        assert_eq!(iso_datetime_to_serial("2024-01-15T12:00:00"), Some(45306.5));
        assert_eq!(iso_datetime_to_serial("nope"), None);
        assert_eq!(iso_duration_to_days("PT36H"), Some(1.5));
        assert_eq!(iso_duration_to_days("P1DT12H"), Some(1.5));
        assert_eq!(iso_duration_to_days("PT1X"), None);
    }

    #[test]
    fn extensions() {
        assert_eq!(SourceFormat::from_extension("ODS"), Some(SourceFormat::Ods));
        assert_eq!(
            SourceFormat::from_extension("tsv"),
            Some(SourceFormat::Csv { delimiter: b'\t' })
        );
        assert!(SourceFormat::Xls.is_imported());
        assert_eq!(SourceFormat::from_extension("XLT"), Some(SourceFormat::Xls));
        assert!(!SourceFormat::Csv { delimiter: b',' }.is_imported());
        assert_eq!(SourceFormat::from_extension("txt"), None);
    }
}
