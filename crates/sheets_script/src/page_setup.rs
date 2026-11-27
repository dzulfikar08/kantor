//! `Worksheet.PageSetup` (with `FirstPage` / `EvenPage` headers and
//! footers), `HPageBreaks` / `VPageBreaks`, `ResetAllPageBreaks`,
//! `Range.PageBreak`, `ExportAsFixedFormat` (PDF through `sheets_pdf`, the
//! same printing as File ▸ Export to PDF) and `Application.InchesToPoints` /
//! `CentimetersToPoints`.
//!
//! Page Setup edits go through `Workbook::set_print_settings`, as the app's
//! Page Setup dialog does: one undo step each (folded into the run's step),
//! saved as `<pageSetup>`, `<pageMargins>`, `<printOptions>`,
//! `<headerFooter>` and the `_xlnm.Print_Area` / `_xlnm.Print_Titles` names.
//! `Draft`, `PrintQuality` and `PrintErrors` are accepted and change
//! nothing (Kantor doesn't keep them). `PrintOut` / `PrintPreview` are not
//! supported: unattended printing to a real printer from a macro would
//! surprise the user, so they fail with error 445; export a PDF instead.

use std::any::Any;
use std::cell::Cell;
use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;

use kantor_script::{Arg, Dispatch, ScriptError, Value};
use sheets_engine::{CellRange, Paper, PrintComments, SheetPrint, join_sections, split_sections};

use crate::addr;
use crate::range::{RangeObj, range_of};
use crate::session::Session;
use crate::values::*;

pub const XL_PAPER_LETTER: i64 = 1;
pub const XL_PAPER_LETTER_SMALL: i64 = 2;
pub const XL_PAPER_TABLOID: i64 = 3;
pub const XL_PAPER_LEDGER: i64 = 4;
pub const XL_PAPER_LEGAL: i64 = 5;
pub const XL_PAPER_STATEMENT: i64 = 6;
pub const XL_PAPER_EXECUTIVE: i64 = 7;
pub const XL_PAPER_A3: i64 = 8;
pub const XL_PAPER_A4: i64 = 9;
pub const XL_PAPER_A4_SMALL: i64 = 10;
pub const XL_PAPER_A5: i64 = 11;
pub const XL_PAPER_B4: i64 = 12;
pub const XL_PAPER_B5: i64 = 13;
pub const XL_PAPER_FOLIO: i64 = 14;
pub const XL_PAPER_QUARTO: i64 = 15;
pub const XL_PAPER_10X14: i64 = 16;
pub const XL_PAPER_11X17: i64 = 17;
pub const XL_PAPER_NOTE: i64 = 18;
pub const XL_PAPER_ENVELOPE_10: i64 = 20;
pub const XL_PAPER_USER: i64 = 256;
pub const XL_DOWN_THEN_OVER: i64 = 1;
pub const XL_OVER_THEN_DOWN: i64 = 2;
pub const XL_PRINT_NO_COMMENTS: i64 = -4142;
pub const XL_PRINT_SHEET_END: i64 = 1;
pub const XL_PRINT_IN_PLACE: i64 = 16;
pub const XL_PRINT_ERRORS_DISPLAYED: i64 = 0;
pub const XL_PRINT_ERRORS_BLANK: i64 = 1;
pub const XL_PRINT_ERRORS_DASH: i64 = 2;
pub const XL_PRINT_ERRORS_NA: i64 = 3;
pub const XL_TYPE_PDF: i64 = 0;
pub const XL_TYPE_XPS: i64 = 1;
pub const XL_QUALITY_STANDARD: i64 = 0;
pub const XL_QUALITY_MINIMUM: i64 = 1;
pub const XL_PAGE_BREAK_AUTOMATIC: i64 = -4105;
pub const XL_PAGE_BREAK_MANUAL: i64 = -4135;
pub const XL_PAGE_BREAK_NONE: i64 = -4142;
pub const XL_PAGE_BREAK_FULL: i64 = 1;
pub const XL_PAGE_BREAK_PARTIAL: i64 = 2;

/// `xlAutomatic` (`FirstPageNumber` when the sheet sets none).
const AUTOMATIC: i64 = -4105;

fn long(n: i64) -> Value {
    Value::Long(n as i32)
}

/// Points per inch, per centimetre.
const PT_PER_INCH: f64 = 72.0;
const PT_PER_CM: f64 = 72.0 / 2.54;

/// `Application.InchesToPoints(Inches)`.
pub fn inches_to_points(args: &[Arg]) -> Result<Value, ScriptError> {
    Ok(Value::Double(
        to_f64(need(args, 0, "Inches")?)? * PT_PER_INCH,
    ))
}

/// `Application.CentimetersToPoints(Centimeters)`.
pub fn centimeters_to_points(args: &[Arg]) -> Result<Value, ScriptError> {
    Ok(Value::Double(
        to_f64(need(args, 0, "Centimeters")?)? * PT_PER_CM,
    ))
}

/// `Worksheet.PageSetup`.
pub fn page_setup(s: &Rc<Session>, sheet: u32) -> Value {
    obj(Rc::new(PageSetupObj::new(s, sheet)))
}

/// `Worksheet.HPageBreaks` (manual breaks above rows); with an index
/// (`HPageBreaks(1)`), that break.
pub fn h_page_breaks(s: &Rc<Session>, sheet: u32, args: &[Arg]) -> Result<Value, ScriptError> {
    page_breaks(s, sheet, true, args)
}

/// `Worksheet.VPageBreaks` (manual breaks left of columns); with an index,
/// that break.
pub fn v_page_breaks(s: &Rc<Session>, sheet: u32, args: &[Arg]) -> Result<Value, ScriptError> {
    page_breaks(s, sheet, false, args)
}

fn page_breaks(
    s: &Rc<Session>,
    sheet: u32,
    rows: bool,
    args: &[Arg],
) -> Result<Value, ScriptError> {
    let list = PageBreaksObj {
        s: s.clone(),
        sheet,
        rows,
    };
    if args.is_empty() {
        Ok(obj(Rc::new(list)))
    } else {
        list.get("Item", args)
    }
}

/// `Worksheet.ResetAllPageBreaks`.
pub fn reset_all_page_breaks(s: &Rc<Session>, sheet: u32) -> Result<Value, ScriptError> {
    let idx = s.sheet_index(sheet)?;
    s.write()?.reset_page_breaks(idx).map_err(engine)?;
    Ok(Value::Empty)
}

/// `Range.PageBreak`: `xlPageBreakManual` when a manual break is above the
/// range's first row (or, for whole columns, left of its first column).
pub fn range_page_break(r: &RangeObj) -> Result<Value, ScriptError> {
    let idx = r.idx()?;
    let a = r.first();
    let p = r.s.read()?.print_settings(idx);
    let manual = if a.is_whole_columns() {
        p.col_breaks.contains(&a.left)
    } else {
        p.row_breaks.contains(&a.top)
    };
    Ok(long(if manual {
        XL_PAGE_BREAK_MANUAL
    } else {
        XL_PAGE_BREAK_NONE
    }))
}

/// `Range.PageBreak = xlPageBreakManual | xlPageBreakNone`: a break above
/// the range's first row (left of the first column for whole columns).
pub fn set_range_page_break(r: &RangeObj, value: &Value) -> Result<(), ScriptError> {
    let idx = r.idx()?;
    let a = r.first();
    let kind = to_i64(value)?;
    let (row, col) = if a.is_whole_columns() {
        (None, Some(a.left))
    } else {
        (Some(a.top), None)
    };
    let mut book = r.s.write()?;
    match kind {
        XL_PAGE_BREAK_MANUAL => book.insert_page_break(idx, row, col).map_err(engine),
        XL_PAGE_BREAK_NONE | XL_PAGE_BREAK_AUTOMATIC => {
            book.remove_page_break(idx, row, col).map_err(engine)
        }
        _ => Err(err1004(
            "Unable to set the PageBreak property of the Range class",
        )),
    }
}

/// `PrintOut` / `PrintPreview` on a sheet, workbook or range.
pub fn print_out() -> Result<Value, ScriptError> {
    Err(ScriptError::unsupported("Printing from a macro"))
}

// ── PageSetup ─────────────────────────────────────────────────────────────

pub struct PageSetupObj {
    s: Rc<Session>,
    /// `sheetId`.
    sheet: u32,
    /// `FitToPagesWide` / `FitToPagesTall` set while `Zoom` is a percentage:
    /// Excel keeps them for when `Zoom = False` turns fitting on.
    fit: Cell<(u32, u32)>,
}

impl PageSetupObj {
    pub fn new(s: &Rc<Session>, sheet: u32) -> PageSetupObj {
        let fit = s
            .sheet_index(sheet)
            .ok()
            .and_then(|idx| s.read().ok().map(|b| b.print_settings(idx).setup.fit))
            .flatten()
            .unwrap_or((1, 1));
        PageSetupObj {
            s: s.clone(),
            sheet,
            fit: Cell::new(fit),
        }
    }

    fn settings(&self) -> Result<SheetPrint, ScriptError> {
        settings(&self.s, self.sheet)
    }

    fn edit(
        &self,
        f: impl FnOnce(&mut SheetPrint) -> Result<(), ScriptError>,
    ) -> Result<(), ScriptError> {
        edit(&self.s, self.sheet, f)
    }

    /// A print area or titles argument as areas of this sheet (`""` or
    /// `False`: none).
    fn areas_arg(&self, value: &Value, what: &str) -> Result<Vec<CellRange>, ScriptError> {
        let v = scalar(value)?;
        if matches!(v, Value::Boolean(false) | Value::Empty | Value::Null) {
            return Ok(Vec::new());
        }
        let text = to_str(&v)?;
        let text = text.trim().trim_start_matches('=');
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let bad = || {
            err1004(format!(
                "Unable to set the {what} property of the PageSetup class"
            ))
        };
        let r = crate::sheet::resolve(&self.s, self.sheet, text).map_err(|_| bad())?;
        if r.sheet != self.sheet {
            return Err(bad());
        }
        Ok(r.areas.clone())
    }
}

fn settings(s: &Rc<Session>, sheet: u32) -> Result<SheetPrint, ScriptError> {
    let idx = s.sheet_index(sheet)?;
    let p = s.read()?.print_settings(idx);
    Ok(p)
}

fn edit(
    s: &Rc<Session>,
    sheet: u32,
    f: impl FnOnce(&mut SheetPrint) -> Result<(), ScriptError>,
) -> Result<(), ScriptError> {
    let idx = s.sheet_index(sheet)?;
    let mut book = s.write()?;
    let mut p = book.print_settings(idx);
    f(&mut p)?;
    book.set_print_settings(idx, p).map_err(engine)
}

/// `$A$1:$D$20`, areas joined with commas, as Excel gives `PrintArea`.
fn areas_text(areas: &[CellRange]) -> String {
    areas
        .iter()
        .map(|&a| addr::area_text(a, true, true))
        .collect::<Vec<_>>()
        .join(",")
}

/// Margins are inches in the file, points in VBA.
fn points(inches: f64) -> Value {
    Value::Double((inches * PT_PER_INCH * 100.0).round() / 100.0)
}

fn inches(value: &Value, what: &str) -> Result<f64, ScriptError> {
    let pt = to_f64(value)?;
    if !pt.is_finite() || !(0.0..=4000.0).contains(&pt) {
        return Err(err1004(format!(
            "Unable to set the {what} property of the PageSetup class"
        )));
    }
    Ok(pt / PT_PER_INCH)
}

/// `FitToPagesWide` / `Tall` as VBA gives them: `False` for "automatic".
fn pages_value(n: u32) -> Value {
    if n == 0 {
        Value::Boolean(false)
    } else {
        Value::Long(n as i32)
    }
}

fn pages_arg(value: &Value) -> Result<u32, ScriptError> {
    match scalar(value)? {
        Value::Boolean(false) => Ok(0),
        v => {
            let n = to_i64(&v)?;
            if !(0..=32767).contains(&n) {
                return Err(err1004(
                    "Unable to set the FitToPagesWide property of the PageSetup class",
                ));
            }
            Ok(n as u32)
        }
    }
}

/// Section (0 left, 1 centre, 2 right) and header (or footer) of a
/// `LeftHeader`…`RightFooter` member.
fn hf_member(lower: &str) -> Option<(usize, bool)> {
    Some(match lower {
        "leftheader" => (0, true),
        "centerheader" => (1, true),
        "rightheader" => (2, true),
        "leftfooter" => (0, false),
        "centerfooter" => (1, false),
        "rightfooter" => (2, false),
        _ => return None,
    })
}

/// Which pages a header or footer is for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pages {
    Odd,
    Even,
    First,
}

fn hf_text(p: &mut SheetPrint, pages: Pages, header: bool) -> &mut String {
    let hf = &mut p.header_footer;
    match (pages, header) {
        (Pages::Odd, true) => &mut hf.odd_header,
        (Pages::Odd, false) => &mut hf.odd_footer,
        (Pages::Even, true) => &mut hf.even_header,
        (Pages::Even, false) => &mut hf.even_footer,
        (Pages::First, true) => &mut hf.first_header,
        (Pages::First, false) => &mut hf.first_footer,
    }
}

fn hf_section(
    s: &Rc<Session>,
    sheet: u32,
    pages: Pages,
    section: usize,
    header: bool,
) -> Result<Value, ScriptError> {
    let mut p = settings(s, sheet)?;
    let text = hf_text(&mut p, pages, header);
    Ok(Value::String(split_sections(text)[section.min(2)].clone()))
}

fn set_hf_section(
    s: &Rc<Session>,
    sheet: u32,
    pages: Pages,
    section: usize,
    header: bool,
    value: &Value,
) -> Result<(), ScriptError> {
    let new = to_str(value)?;
    edit(s, sheet, |p| {
        let text = hf_text(p, pages, header);
        let mut parts = split_sections(text);
        parts[section.min(2)] = new;
        *text = join_sections(&parts[0], &parts[1], &parts[2]);
        Ok(())
    })
}

fn paper_code(paper: &Paper) -> i64 {
    paper.code().map_or(XL_PAPER_USER, i64::from)
}

impl Dispatch for PageSetupObj {
    fn type_name(&self) -> String {
        "PageSetup".into()
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        if let Some((section, header)) = hf_member(&lower) {
            return hf_section(&self.s, self.sheet, Pages::Odd, section, header);
        }
        let p = match lower.as_str() {
            "parent" => return Ok(obj(crate::sheet::SheetObj::new(&self.s, self.sheet))),
            "application" => return Ok(obj(Rc::new(crate::app::Application::new(&self.s)))),
            "firstpage" | "evenpage" => {
                return Ok(obj(Rc::new(PageObj {
                    s: self.s.clone(),
                    sheet: self.sheet,
                    pages: if lower == "firstpage" {
                        Pages::First
                    } else {
                        Pages::Even
                    },
                })));
            }
            "draft" => return Ok(Value::Boolean(false)),
            "printquality" => return Ok(Value::Long(600)),
            "printerrors" => return Ok(long(XL_PRINT_ERRORS_DISPLAYED)),
            "leftheaderpicture"
            | "centerheaderpicture"
            | "rightheaderpicture"
            | "leftfooterpicture"
            | "centerfooterpicture"
            | "rightfooterpicture" => {
                return Err(ScriptError::unsupported(
                    "Header and footer pictures from a macro",
                ));
            }
            "pages" => return Err(ScriptError::unsupported("PageSetup.Pages")),
            _ => self.settings()?,
        };
        let setup = &p.setup;
        Ok(match lower.as_str() {
            "orientation" => Value::Long(if setup.landscape { 2 } else { 1 }),
            "papersize" => long(paper_code(&setup.paper)),
            "printarea" => Value::String(areas_text(&p.print_area)),
            "printtitlerows" => Value::String(
                p.title_rows
                    .map(|(a, b)| addr::area_text(CellRange::rows(a, b), true, true))
                    .unwrap_or_default(),
            ),
            "printtitlecolumns" => Value::String(
                p.title_cols
                    .map(|(a, b)| addr::area_text(CellRange::columns(a, b), true, true))
                    .unwrap_or_default(),
            ),
            "zoom" => match setup.fit {
                Some(_) => Value::Boolean(false),
                None => Value::Long(setup.scale as i32),
            },
            "fittopageswide" => pages_value(setup.fit.unwrap_or(self.fit.get()).0),
            "fittopagestall" => pages_value(setup.fit.unwrap_or(self.fit.get()).1),
            "leftmargin" => points(p.margins.left),
            "rightmargin" => points(p.margins.right),
            "topmargin" => points(p.margins.top),
            "bottommargin" => points(p.margins.bottom),
            "headermargin" => points(p.margins.header),
            "footermargin" => points(p.margins.footer),
            "centerhorizontally" => Value::Boolean(p.options.horizontal_centered),
            "centervertically" => Value::Boolean(p.options.vertical_centered),
            "printgridlines" => Value::Boolean(p.options.grid_lines),
            "printheadings" => Value::Boolean(p.options.headings),
            "firstpagenumber" => match setup.first_page_number {
                Some(n) => Value::Long(n),
                None => long(AUTOMATIC),
            },
            "order" => long(if setup.over_then_down {
                XL_OVER_THEN_DOWN
            } else {
                XL_DOWN_THEN_OVER
            }),
            "blackandwhite" => Value::Boolean(setup.black_and_white),
            "printcomments" => long(match setup.comments {
                PrintComments::None => XL_PRINT_NO_COMMENTS,
                PrintComments::AtEnd => XL_PRINT_SHEET_END,
                PrintComments::AsDisplayed => XL_PRINT_IN_PLACE,
            }),
            "differentfirstpageheaderfooter" => Value::Boolean(p.header_footer.different_first),
            "oddandevenpagesheaderfooter" => Value::Boolean(p.header_footer.different_odd_even),
            "scalewithdocheaderfooter" => Value::Boolean(p.header_footer.scale_with_doc),
            "alignmarginsheaderfooter" => Value::Boolean(p.header_footer.align_with_margins),
            _ => return Err(ScriptError::no_member("PageSetup", name)),
        })
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        let lower = name.to_ascii_lowercase();
        if let Some((section, header)) = hf_member(&lower) {
            return set_hf_section(&self.s, self.sheet, Pages::Odd, section, header, &value);
        }
        let fail = |what: &str| {
            err1004(format!(
                "Unable to set the {what} property of the PageSetup class"
            ))
        };
        match lower.as_str() {
            "orientation" => {
                let landscape = match to_i64(&value)? {
                    1 => false,
                    2 => true,
                    _ => return Err(fail("Orientation")),
                };
                self.edit(|p| {
                    p.setup.landscape = landscape;
                    Ok(())
                })
            }
            "papersize" => {
                let code = to_i64(&value)?;
                let paper = u32::try_from(code)
                    .ok()
                    .and_then(Paper::from_code)
                    .ok_or_else(|| fail("PaperSize"))?;
                self.edit(|p| {
                    p.setup.paper = paper;
                    Ok(())
                })
            }
            "printarea" => {
                let areas = self.areas_arg(&value, "PrintArea")?;
                self.edit(|p| {
                    p.print_area = areas;
                    Ok(())
                })
            }
            "printtitlerows" => {
                let areas = self.areas_arg(&value, "PrintTitleRows")?;
                let rows = areas.first().map(|a| (a.top, a.bottom));
                self.edit(|p| {
                    p.title_rows = rows;
                    Ok(())
                })
            }
            "printtitlecolumns" => {
                let areas = self.areas_arg(&value, "PrintTitleColumns")?;
                let cols = areas.first().map(|a| (a.left, a.right));
                self.edit(|p| {
                    p.title_cols = cols;
                    Ok(())
                })
            }
            "zoom" => match scalar(&value)? {
                Value::Boolean(false) => {
                    let fit = self.fit.get();
                    self.edit(|p| {
                        if p.setup.fit.is_none() {
                            p.setup.fit = Some(fit);
                        }
                        Ok(())
                    })
                }
                Value::Boolean(true) => Err(fail("Zoom")),
                v => {
                    let n = to_i64(&v)?;
                    if !(10..=400).contains(&n) {
                        return Err(fail("Zoom"));
                    }
                    self.edit(|p| {
                        p.setup.fit = None;
                        p.setup.scale = n as u32;
                        Ok(())
                    })
                }
            },
            "fittopageswide" | "fittopagestall" => {
                let n = pages_arg(&value)?;
                let wide = lower == "fittopageswide";
                let (w, h) = self.fit.get();
                let both = if wide { (n, h) } else { (w, n) };
                self.fit.set(both);
                // Only used while Zoom is False, as in Excel.
                self.edit(|p| {
                    if let Some(f) = p.setup.fit.as_mut() {
                        if wide {
                            f.0 = n;
                        } else {
                            f.1 = n;
                        }
                    }
                    Ok(())
                })
            }
            "leftmargin" | "rightmargin" | "topmargin" | "bottommargin" | "headermargin"
            | "footermargin" => {
                let v = inches(&value, name)?;
                self.edit(|p| {
                    let m = &mut p.margins;
                    match lower.as_str() {
                        "leftmargin" => m.left = v,
                        "rightmargin" => m.right = v,
                        "topmargin" => m.top = v,
                        "bottommargin" => m.bottom = v,
                        "headermargin" => m.header = v,
                        _ => m.footer = v,
                    }
                    Ok(())
                })
            }
            "centerhorizontally"
            | "centervertically"
            | "printgridlines"
            | "printheadings"
            | "blackandwhite"
            | "differentfirstpageheaderfooter"
            | "oddandevenpagesheaderfooter"
            | "scalewithdocheaderfooter"
            | "alignmarginsheaderfooter" => {
                let on = to_bool(&value)?;
                self.edit(|p| {
                    match lower.as_str() {
                        "centerhorizontally" => p.options.horizontal_centered = on,
                        "centervertically" => p.options.vertical_centered = on,
                        "printgridlines" => p.options.grid_lines = on,
                        "printheadings" => p.options.headings = on,
                        "blackandwhite" => p.setup.black_and_white = on,
                        "differentfirstpageheaderfooter" => p.header_footer.different_first = on,
                        "oddandevenpagesheaderfooter" => p.header_footer.different_odd_even = on,
                        "scalewithdocheaderfooter" => p.header_footer.scale_with_doc = on,
                        _ => p.header_footer.align_with_margins = on,
                    }
                    Ok(())
                })
            }
            "firstpagenumber" => {
                let n = to_i64(&value)?;
                let first = if n == AUTOMATIC {
                    None
                } else {
                    Some(i32::try_from(n).map_err(|_| fail("FirstPageNumber"))?)
                };
                self.edit(|p| {
                    p.setup.first_page_number = first;
                    Ok(())
                })
            }
            "order" => {
                let over = match to_i64(&value)? {
                    XL_DOWN_THEN_OVER => false,
                    XL_OVER_THEN_DOWN => true,
                    _ => return Err(fail("Order")),
                };
                self.edit(|p| {
                    p.setup.over_then_down = over;
                    Ok(())
                })
            }
            "printcomments" => {
                let c = match to_i64(&value)? {
                    XL_PRINT_NO_COMMENTS => PrintComments::None,
                    XL_PRINT_SHEET_END => PrintComments::AtEnd,
                    XL_PRINT_IN_PLACE => PrintComments::AsDisplayed,
                    _ => return Err(fail("PrintComments")),
                };
                self.edit(|p| {
                    p.setup.comments = c;
                    Ok(())
                })
            }
            // Kept by Excel for the printer; Kantor has no use for them.
            "draft" | "printquality" | "printerrors" | "printnotes" => {
                scalar(&value)?;
                Ok(())
            }
            "leftheaderpicture"
            | "centerheaderpicture"
            | "rightheaderpicture"
            | "leftfooterpicture"
            | "centerfooterpicture"
            | "rightfooterpicture" => Err(ScriptError::unsupported(
                "Header and footer pictures from a macro",
            )),
            _ => Err(ScriptError::no_member("PageSetup", name)),
        }
    }
}

/// `PageSetup.FirstPage` / `EvenPage`.
struct PageObj {
    s: Rc<Session>,
    sheet: u32,
    pages: Pages,
}

impl Dispatch for PageObj {
    fn type_name(&self) -> String {
        "Page".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        let (section, header) =
            hf_member(&lower).ok_or_else(|| ScriptError::no_member("Page", name))?;
        Ok(obj(Rc::new(HeaderFooterObj {
            s: self.s.clone(),
            sheet: self.sheet,
            pages: self.pages,
            section,
            header,
        })))
    }
}

/// `Page.LeftHeader`…: one section of a first or even page's header or
/// footer.
struct HeaderFooterObj {
    s: Rc<Session>,
    sheet: u32,
    pages: Pages,
    section: usize,
    header: bool,
}

impl Dispatch for HeaderFooterObj {
    fn type_name(&self) -> String {
        "HeaderFooter".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "text" | "" => hf_section(&self.s, self.sheet, self.pages, self.section, self.header),
            "picture" => Err(ScriptError::unsupported(
                "Header and footer pictures from a macro",
            )),
            _ => Err(ScriptError::no_member("HeaderFooter", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "text" | "" => set_hf_section(
                &self.s,
                self.sheet,
                self.pages,
                self.section,
                self.header,
                &value,
            ),
            _ => Err(ScriptError::no_member("HeaderFooter", name)),
        }
    }
}

// ── Page breaks ───────────────────────────────────────────────────────────

/// `HPageBreaks` / `VPageBreaks`: the sheet's manual page breaks (Kantor
/// works out automatic breaks only when it prints, so they aren't listed).
struct PageBreaksObj {
    s: Rc<Session>,
    sheet: u32,
    /// Horizontal breaks (above rows); else vertical (left of columns).
    rows: bool,
}

impl PageBreaksObj {
    fn list(&self) -> Result<Vec<u32>, ScriptError> {
        let p = settings(&self.s, self.sheet)?;
        Ok(if self.rows {
            p.row_breaks
        } else {
            p.col_breaks
        })
    }

    fn item(&self, at: u32) -> Value {
        obj(Rc::new(PageBreakObj {
            s: self.s.clone(),
            sheet: self.sheet,
            rows: self.rows,
            at: Cell::new(at),
        }))
    }

    fn type_label(&self) -> &'static str {
        if self.rows {
            "HPageBreaks"
        } else {
            "VPageBreaks"
        }
    }
}

impl Dispatch for PageBreaksObj {
    fn type_name(&self) -> String {
        self.type_label().into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "count" => Ok(Value::Long(self.list()?.len() as i32)),
            "item" | "" => {
                let i = to_i64(need(args, 0, "Index")?)?;
                let list = self.list()?;
                let at = usize::try_from(i - 1)
                    .ok()
                    .and_then(|k| list.get(k))
                    .ok_or_else(|| ScriptError::runtime(9))?;
                Ok(self.item(*at))
            }
            "add" => {
                let before = need(args, 0, "Before")?;
                let r = match range_of(before) {
                    Some(r) => r,
                    None => crate::sheet::resolve(&self.s, self.sheet, &to_str(before)?)?,
                };
                if r.sheet != self.sheet {
                    return Err(err1004(format!(
                        "Add method of {} class failed",
                        self.type_label()
                    )));
                }
                let a = r.first();
                let at = if self.rows { a.top } else { a.left };
                if at == 0 {
                    return Err(err1004(format!(
                        "Add method of {} class failed",
                        self.type_label()
                    )));
                }
                let idx = self.s.sheet_index(self.sheet)?;
                let (row, col) = if self.rows {
                    (Some(at), None)
                } else {
                    (None, Some(at))
                };
                self.s
                    .write()?
                    .insert_page_break(idx, row, col)
                    .map_err(engine)?;
                Ok(self.item(at))
            }
            "parent" => Ok(obj(crate::sheet::SheetObj::new(&self.s, self.sheet))),
            _ => Err(ScriptError::no_member(self.type_label(), name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        Ok(self.list()?.into_iter().map(|at| self.item(at)).collect())
    }
}

/// One manual page break (`HPageBreak` / `VPageBreak`).
struct PageBreakObj {
    s: Rc<Session>,
    sheet: u32,
    rows: bool,
    /// The row (column) the next page starts at, 0-based.
    at: Cell<u32>,
}

impl PageBreakObj {
    fn type_label(&self) -> &'static str {
        if self.rows {
            "HPageBreak"
        } else {
            "VPageBreak"
        }
    }

    fn remove(&self) -> Result<(), ScriptError> {
        let idx = self.s.sheet_index(self.sheet)?;
        let at = Some(self.at.get());
        let (row, col) = if self.rows { (at, None) } else { (None, at) };
        self.s
            .write()?
            .remove_page_break(idx, row, col)
            .map_err(engine)
    }
}

impl Dispatch for PageBreakObj {
    fn type_name(&self) -> String {
        self.type_label().into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "location" => {
                let at = self.at.get();
                let cell = if self.rows { (at, 0) } else { (0, at) };
                Ok(obj(RangeObj::new(
                    &self.s,
                    self.sheet,
                    CellRange::from_corners(cell, cell),
                )))
            }
            "type" => Ok(long(XL_PAGE_BREAK_MANUAL)),
            "extent" => Ok(long(XL_PAGE_BREAK_FULL)),
            "delete" | "dragoff" => self.remove().map(|()| Value::Empty),
            "parent" => Ok(obj(crate::sheet::SheetObj::new(&self.s, self.sheet))),
            _ => Err(ScriptError::no_member(self.type_label(), name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "location" => {
                let r = match range_of(&value) {
                    Some(r) => r,
                    None => crate::sheet::resolve(&self.s, self.sheet, &to_str(&value)?)?,
                };
                let a = r.first();
                let to = if self.rows { a.top } else { a.left };
                let idx = self.s.sheet_index(self.sheet)?;
                self.s
                    .write()?
                    .move_page_break(idx, self.rows, Some(self.at.get()), Some(to))
                    .map_err(engine)?;
                self.at.set(to);
                Ok(())
            }
            _ => Err(ScriptError::no_member(self.type_label(), name)),
        }
    }
}

// ── ExportAsFixedFormat ───────────────────────────────────────────────────

/// `Worksheet.ExportAsFixedFormat`: the sheet as Excel prints it.
pub fn export_pdf_sheet(s: &Rc<Session>, sheet: u32, args: &[Arg]) -> Result<Value, ScriptError> {
    let idx = s.sheet_index(sheet)?;
    export(s, args, Some(idx), None)
}

/// `Workbook.ExportAsFixedFormat`: every visible sheet and chart sheet.
pub fn export_pdf_book(s: &Rc<Session>, args: &[Arg]) -> Result<Value, ScriptError> {
    export(s, args, None, None)
}

/// `Range.ExportAsFixedFormat`: only the range (its areas), with its
/// sheet's page setup.
pub fn export_pdf_range(r: &RangeObj, args: &[Arg]) -> Result<Value, ScriptError> {
    let idx = r.idx()?;
    export(&r.s, args, Some(idx), Some(r.areas.clone()))
}

/// The PDF's file: `Filename` (`.pdf` added when it has no extension), or
/// the workbook's name with `.pdf` in its folder.
fn pdf_path(s: &Rc<Session>, args: &[Arg]) -> Result<std::path::PathBuf, ScriptError> {
    let name = match arg_str(args, 1, "Filename")?.filter(|n| !n.trim().is_empty()) {
        Some(n) => {
            let n = n.trim().to_owned();
            if Path::new(&n).extension().is_some() {
                n
            } else {
                format!("{n}.pdf")
            }
        }
        None => {
            let book_name = s.file_name.borrow().clone();
            let stem = Path::new(&book_name)
                .file_stem()
                .map(|x| x.to_string_lossy().into_owned())
                .filter(|x| !x.is_empty())
                .unwrap_or_else(|| "Book1".to_owned());
            format!("{stem}.pdf")
        }
    };
    crate::paths::checked(s, &name, true)
}

fn export(
    s: &Rc<Session>,
    args: &[Arg],
    sheet: Option<u32>,
    range: Option<Vec<CellRange>>,
) -> Result<Value, ScriptError> {
    match to_i64(need(args, 0, "Type")?)? {
        XL_TYPE_PDF => {}
        XL_TYPE_XPS => return Err(ScriptError::unsupported("Exporting XPS files")),
        _ => return Err(err1004("ExportAsFixedFormat: the Type must be xlTypePDF")),
    }
    let ignore_print_area = arg_bool(args, 4, "IgnorePrintAreas")?.unwrap_or(false);
    let from = arg_i64(args, 5, "From")?;
    let to = arg_i64(args, 6, "To")?;
    // `Quality`, `IncludeDocProperties` and `OpenAfterPublish` change
    // nothing: Kantor's PDFs are always full quality, carry the title and
    // are not opened from a macro.
    let path = pdf_path(s, args)?;

    let mut book = s.read()?;
    let print = match (sheet, range) {
        (Some(idx), Some(areas)) => {
            let mut p = book.print_settings(idx);
            p.print_area = areas;
            Some((idx, p))
        }
        _ => None,
    };
    let opts = sheets_pdf::Options {
        sheets: sheet.map(|i| vec![i]),
        ignore_print_area: ignore_print_area && print.is_none(),
        print,
        ..sheets_pdf::Options::default()
    }
    .now()
    .with_path(book.source_path());
    let mut shaper = sheets_pdf::Shaper::new();
    let plan = sheets_pdf::Plan::new(&book, &opts, &mut shaper);
    let n = plan.pages.len();
    let first = from.map_or(0, |f| (f.max(1) - 1) as usize);
    let last = to.map_or(n, |t| (t.max(0) as usize).min(n));
    if n > 0 && first >= last {
        return Err(err1004(
            "Document not saved: no pages in the From and To range",
        ));
    }
    let pages = first.min(n)..last.max(first.min(n));
    let mut skip = HashSet::new();
    let mut shaper = Some(shaper);
    let pdf = loop {
        let sh = shaper.take().unwrap_or_default();
        let mut enc = sheets_pdf::Encoder::new(&plan.title, sh).skip_pictures(&skip);
        for i in pages.clone() {
            let page = sheets_pdf::extract(&mut book, &plan, i);
            enc.page(&page);
        }
        match enc.finish() {
            Ok(pdf) => break pdf,
            // A picture the PDF writer can't decode is left out.
            Err(e) => match e.downcast_ref::<sheets_pdf::BadPicture>() {
                Some(bad) if skip.insert(bad.0) => {}
                _ => return Err(err1004(format!("Document not saved ({e})"))),
            },
        }
    };
    drop(book);
    sheets_pdf::write_file(&path, &pdf)
        .map_err(|e| err1004(format!("Document not saved ({}: {e})", path.display())))?;
    Ok(Value::Empty)
}

/// The constants of this module by their VBA names (registered with the
/// interpreter).
pub const CONSTANTS: &[(&str, i64)] = &[
    ("xlPaperLetter", 1),
    ("xlPaperLetterSmall", 2),
    ("xlPaperTabloid", 3),
    ("xlPaperLedger", 4),
    ("xlPaperLegal", 5),
    ("xlPaperStatement", 6),
    ("xlPaperExecutive", 7),
    ("xlPaperA3", 8),
    ("xlPaperA4", 9),
    ("xlPaperA4Small", 10),
    ("xlPaperA5", 11),
    ("xlPaperB4", 12),
    ("xlPaperB5", 13),
    ("xlPaperFolio", 14),
    ("xlPaperQuarto", 15),
    ("xlPaper10x14", 16),
    ("xlPaper11x17", 17),
    ("xlPaperNote", 18),
    ("xlPaperEnvelope10", 20),
    ("xlPaperUser", 256),
    ("xlDownThenOver", 1),
    ("xlOverThenDown", 2),
    ("xlPrintNoComments", -4142),
    ("xlPrintSheetEnd", 1),
    ("xlPrintInPlace", 16),
    ("xlPrintErrorsDisplayed", 0),
    ("xlPrintErrorsBlank", 1),
    ("xlPrintErrorsDash", 2),
    ("xlPrintErrorsNA", 3),
    ("xlTypePDF", 0),
    ("xlTypeXPS", 1),
    ("xlQualityStandard", 0),
    ("xlQualityMinimum", 1),
    ("xlPageBreakAutomatic", -4105),
    ("xlPageBreakManual", -4135),
    ("xlPageBreakNone", -4142),
    ("xlPageBreakFull", 1),
    ("xlPageBreakPartial", 2),
];
