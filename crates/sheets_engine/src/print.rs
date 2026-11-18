//! Print settings of a sheet, as Excel keeps them: `<pageSetup>`,
//! `<pageMargins>`, `<printOptions>`, `<headerFooter>`, manual page breaks
//! (`<rowBreaks>`, `<colBreaks>`), `<pageSetUpPr fitToPage>`, and the
//! defined names `_xlnm.Print_Area` / `_xlnm.Print_Titles`.
//!
//! IronCalc doesn't model these elements; `preserve` carries them through a
//! save untouched. They are read here from the opened package (the
//! worksheets' skeletons, without their cells) for printing and PDF export,
//! and page breaks follow rows and columns inserted or deleted since.
//!
//! Also here: what Excel prints when a sheet has no print area (the used
//! range: cells with a value or formatting that shows on paper) and column
//! widths in points as Excel prints them.

use std::collections::HashMap;

use roxmltree::{Document, Node};

use crate::shift::{self, Layout};
use crate::{CellRange, MAX_COLS, MAX_ROWS, Workbook};

/// Paper size in millimetres, portrait.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Paper {
    pub width_mm: f64,
    pub height_mm: f64,
}

impl Paper {
    pub const LETTER: Paper = Paper::inches(8.5, 11.0);
    pub const LEGAL: Paper = Paper::inches(8.5, 14.0);
    /// Folio, 8.5 × 13 in: the "F4" paper common in Indonesia.
    pub const FOLIO: Paper = Paper::inches(8.5, 13.0);
    pub const A3: Paper = Paper::mm(297.0, 420.0);
    pub const A4: Paper = Paper::mm(210.0, 297.0);
    pub const A5: Paper = Paper::mm(148.0, 210.0);

    pub const fn mm(width_mm: f64, height_mm: f64) -> Paper {
        Paper {
            width_mm,
            height_mm,
        }
    }

    pub const fn inches(w: f64, h: f64) -> Paper {
        Paper::mm(w * 25.4, h * 25.4)
    }

    /// Excel's `paperSize` code (ECMA-376 18.18.43, the common ones).
    pub fn from_code(code: u32) -> Option<Paper> {
        Some(match code {
            1 | 2 => Paper::LETTER,
            3 => Paper::inches(11.0, 17.0),
            4 => Paper::inches(17.0, 11.0),
            5 => Paper::LEGAL,
            6 => Paper::inches(5.5, 8.5),
            7 => Paper::inches(7.25, 10.5),
            8 => Paper::A3,
            9 | 10 => Paper::A4,
            11 => Paper::A5,
            12 => Paper::mm(257.0, 364.0),
            13 => Paper::mm(182.0, 257.0),
            14 | 41 => Paper::FOLIO,
            15 => Paper::mm(215.0, 275.0),
            16 => Paper::inches(10.0, 14.0),
            17 => Paper::inches(11.0, 17.0),
            18 => Paper::inches(8.5, 11.0),
            20 => Paper::inches(4.5, 10.375),
            25 => Paper::mm(500.0, 707.0),
            27 => Paper::mm(110.0, 220.0),
            28 => Paper::mm(162.0, 229.0),
            34 => Paper::mm(176.0, 250.0),
            70 => Paper::mm(105.0, 148.0),
            _ => return None,
        })
    }

    /// A paper by name, for command lines: `a4`, `letter`, `legal`,
    /// `f4`/`folio`, `a3`, `a5`, or `WxHmm` / `WxHin`.
    pub fn from_name(name: &str) -> Option<Paper> {
        let n = name.trim().to_ascii_lowercase();
        Some(match n.as_str() {
            "a4" => Paper::A4,
            "a3" => Paper::A3,
            "a5" => Paper::A5,
            "letter" => Paper::LETTER,
            "legal" => Paper::LEGAL,
            "f4" | "folio" => Paper::FOLIO,
            _ => {
                let (num, unit) = if let Some(v) = n.strip_suffix("mm") {
                    (v, 1.0)
                } else {
                    (n.strip_suffix("in")?, 25.4)
                };
                let (w, h) = num.split_once('x')?;
                let w: f64 = w.trim().parse().ok()?;
                let h: f64 = h.trim().parse().ok()?;
                if !(w > 0.0 && h > 0.0 && w < 10_000.0 && h < 10_000.0) {
                    return None;
                }
                Paper::mm(w * unit, h * unit)
            }
        })
    }
}

/// `<pageSetup>` and `<pageSetUpPr fitToPage>`.
#[derive(Debug, Clone, PartialEq)]
pub struct PageSetup {
    pub paper: Paper,
    pub landscape: bool,
    /// Percent, 10-400.
    pub scale: u32,
    /// Fit to `(pages wide, pages tall)` (0: as many as needed), when the
    /// sheet asks for it (`fitToPage`).
    pub fit: Option<(u32, u32)>,
    /// `pageOrder="overThenDown"`.
    pub over_then_down: bool,
    /// Number of the first page, when the sheet sets one.
    pub first_page_number: Option<i32>,
    /// `blackAndWhite`: print without colours.
    pub black_and_white: bool,
    /// `cellComments`: whether and where notes are printed.
    pub comments: PrintComments,
}

/// Page Setup ▸ Sheet ▸ Comments: notes left out, printed after the sheet
/// (cell address and text) or printed where they show on the sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrintComments {
    #[default]
    None,
    AtEnd,
    AsDisplayed,
}

impl Default for PageSetup {
    fn default() -> Self {
        Self {
            // The schema's default paperSize is 1 (Letter).
            paper: Paper::LETTER,
            landscape: false,
            scale: 100,
            fit: None,
            over_then_down: false,
            first_page_number: None,
            black_and_white: false,
            comments: PrintComments::None,
        }
    }
}

/// `<pageMargins>`, in inches.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Margins {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
    pub header: f64,
    pub footer: f64,
}

impl Default for Margins {
    /// Excel's "Normal" margins.
    fn default() -> Self {
        Self {
            left: 0.7,
            right: 0.7,
            top: 0.75,
            bottom: 0.75,
            header: 0.3,
            footer: 0.3,
        }
    }
}

/// `<printOptions>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PrintOptions {
    pub grid_lines: bool,
    pub headings: bool,
    pub horizontal_centered: bool,
    pub vertical_centered: bool,
}

/// `<headerFooter>`: the raw header and footer strings (with Excel's `&`
/// codes).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HeaderFooter {
    pub odd_header: String,
    pub odd_footer: String,
    pub even_header: String,
    pub even_footer: String,
    pub first_header: String,
    pub first_footer: String,
    pub different_odd_even: bool,
    pub different_first: bool,
    /// Scale the header and footer with the sheet (default on).
    pub scale_with_doc: bool,
    /// Align the left and right sections with the page margins (default on).
    pub align_with_margins: bool,
}

impl HeaderFooter {
    /// The header and footer of page `n` (1-based, within the sheet).
    pub fn for_page(&self, n: usize) -> (&str, &str) {
        if self.different_first && n == 1 {
            (&self.first_header, &self.first_footer)
        } else if self.different_odd_even && n.is_multiple_of(2) {
            (&self.even_header, &self.even_footer)
        } else {
            (&self.odd_header, &self.odd_footer)
        }
    }

    pub fn is_empty(&self) -> bool {
        [
            &self.odd_header,
            &self.odd_footer,
            &self.even_header,
            &self.even_footer,
            &self.first_header,
            &self.first_footer,
        ]
        .iter()
        .all(|s| s.is_empty())
    }
}

/// Everything about printing one sheet.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SheetPrint {
    pub setup: PageSetup,
    pub margins: Margins,
    pub options: PrintOptions,
    pub header_footer: HeaderFooter,
    /// Manual page breaks: a new page starts at each of these rows /
    /// columns (0-based), sorted.
    pub row_breaks: Vec<u32>,
    pub col_breaks: Vec<u32>,
    /// `_xlnm.Print_Area`: the ranges to print, each on its own pages.
    pub print_area: Vec<CellRange>,
    /// `_xlnm.Print_Titles`: rows and columns repeated on every page,
    /// inclusive.
    pub title_rows: Option<(u32, u32)>,
    pub title_cols: Option<(u32, u32)>,
    /// Pictures of the header and footer (`&G`), from the file.
    pub hf_pictures: Vec<HfPicture>,
}

/// A header or footer picture (`&G`) from the sheet's `legacyDrawingHF`.
#[derive(Debug, Clone, PartialEq)]
pub struct HfPicture {
    /// Where it goes: Excel's shape id, `LH`, `CH`, `RH` (left, centre,
    /// right header), `LF`, `CF`, `RF` (footer), with `FIRST` or `EVEN`
    /// after it for the first and even pages' (`CHFIRST`).
    pub slot: String,
    /// Its media part and bytes.
    pub part: std::sync::Arc<str>,
    pub data: std::sync::Arc<Vec<u8>>,
    /// Size in points.
    pub width_pt: f64,
    pub height_pt: f64,
}

impl SheetPrint {
    /// The picture of a header or footer section on page `n` (1-based):
    /// `section` 0 left, 1 centre, 2 right.
    pub fn hf_picture(&self, n: usize, header: bool, section: usize) -> Option<&HfPicture> {
        let hf = &self.header_footer;
        let suffix = if hf.different_first && n == 1 {
            "FIRST"
        } else if hf.different_odd_even && n.is_multiple_of(2) {
            "EVEN"
        } else {
            ""
        };
        let slot = format!(
            "{}{}{suffix}",
            ["L", "C", "R"][section.min(2)],
            if header { "H" } else { "F" }
        );
        self.hf_pictures.iter().find(|p| p.slot == slot)
    }
}

/// Excel's default column width in characters (Calibri 11, `baseColWidth`
/// 8), as IronCalc reads a `<col>` that gives none.
const EXCEL_DEFAULT_CHARS: f64 = 8.43;

/// IronCalc's own "default width" marker: a column it creates for a style
/// (not resized) gets 90 of its px, i.e. 10 characters, and no
/// `customWidth`.
const IRONCALC_DEFAULT_CHARS: f64 = 10.0;

/// The maximum digit width (px) of a default font of `size` points: 7 for
/// Calibri 11 and Arial 10, about in proportion for other sizes.
pub(crate) fn digit_px(size: i32) -> f64 {
    let size = size.max(1) as f64;
    if (10.0..=11.0).contains(&size) {
        7.0
    } else {
        (size * 7.0 / 11.0).round().max(1.0)
    }
}

/// Excel's default column (px) for `baseColWidth` `base`: the characters,
/// padding and gridline, rounded up to a multiple of 8 px.
pub(crate) fn base_default_px(base: f64, digit_px: f64) -> f64 {
    ((base * digit_px + 5.0) / 8.0).ceil() * 8.0
}

/// How a sheet's column widths (characters, as `<col width>` stores them)
/// map to pixels at 100 %, by Excel's rule: a width of `w` characters is
/// `trunc((256 w + trunc(128 / d)) / 256 × d)` px, `d` being the default
/// font's maximum digit width (7 px for Calibri 11 and Arial 10). The grid,
/// printouts and drawing anchors all use these pixels, so a column is as
/// wide next to its rows as in Excel (64 by 20 px by default).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColumnScale {
    /// Maximum digit width of the default font, px.
    pub digit_px: f64,
    /// Width of a column the sheet says nothing about, px.
    pub default_px: f64,
    /// The width (characters) a `<col>` without one was given on import.
    default_chars: f64,
}

impl ColumnScale {
    /// Pixels of a column `width` characters wide.
    pub fn px(&self, width: f64) -> f64 {
        if !width.is_finite() || width <= 0.0 {
            return 0.0;
        }
        let d = self.digit_px;
        ((256.0 * width + (128.0 / d).trunc()) / 256.0 * d).trunc()
    }

    /// The width in characters to store for a column `px` pixels wide (whole
    /// pixels, as Excel keeps them), written the way Excel writes it:
    /// `px` back through [`Self::px`] gives the same pixels.
    pub fn width_of(&self, px: f64) -> f64 {
        let px = px.round();
        if !px.is_finite() || px <= 0.0 {
            return 0.0;
        }
        let d = self.digit_px;
        // Excel: characters shown = trunc((px - 5) / d × 100 + 0.5) / 100,
        // stored = trunc((chars × d + 5) / d × 256) / 256.
        let chars = ((px - 5.0) / d * 100.0 + 0.5).trunc() / 100.0;
        let excel = ((chars * d + 5.0) / d * 256.0).trunc() / 256.0;
        let mut w = if chars > 0.0 && self.px(excel) == px {
            excel
        } else {
            // The smallest 1/256 step that truncates to `px`.
            ((px * 256.0 / d).ceil() - (128.0 / d).trunc()) / 256.0
        };
        // Exactly IronCalc's default marker would read back as the default.
        if w == IRONCALC_DEFAULT_CHARS {
            let up = w + 1.0 / 256.0;
            w = if self.px(up) == px {
                up
            } else {
                w - 1.0 / 256.0
            };
        }
        w
    }

    /// Pixels of a `<col>` entry (hidden or not). A column IronCalc made for
    /// a style or a hidden flag only, or one the file gave no width, is the
    /// sheet's default width.
    pub(crate) fn col_px(&self, c: &ironcalc_base::types::Col) -> f64 {
        if !c.width.is_finite() || c.width < 0.0 {
            return self.default_px;
        }
        if !c.custom_width && (c.width == IRONCALC_DEFAULT_CHARS || c.width == self.default_chars) {
            return self.default_px;
        }
        self.px(c.width)
    }
}

/// Column widths as Excel prints them.
#[derive(Debug, Clone, PartialEq)]
pub struct PrintColumns {
    /// Width of a column the sheet says nothing about, in points.
    default_pt: f64,
    /// `(first, last, width in points, hidden)`, 0-based, sorted.
    spans: Vec<(u32, u32, f64, bool)>,
    /// Maximum digit width of the default font, px.
    pub digit_px: f64,
}

impl PrintColumns {
    /// Width of `col` in points (0 when hidden).
    pub fn width_pt(&self, col: u32) -> f64 {
        match self.span(col) {
            Some(&(_, _, _, true)) => 0.0,
            Some(&(_, _, w, false)) => w,
            None => self.default_pt,
        }
    }

    pub fn hidden(&self, col: u32) -> bool {
        matches!(self.span(col), Some(&(_, _, _, true)))
    }

    fn span(&self, col: u32) -> Option<&(u32, u32, f64, bool)> {
        let i = self.spans.partition_point(|s| s.1 < col);
        self.spans.get(i).filter(|s| s.0 <= col)
    }
}

/// What a sheet's XML says about printing (before page breaks follow
/// moved rows and columns).
#[derive(Debug, Clone, Default)]
struct Raw {
    print: SheetPrint,
    /// `<sheetFormatPr defaultColWidth / baseColWidth>`.
    default_col_width: Option<f64>,
    base_col_width: Option<f64>,
}

/// Print settings of the opened file, by `sheetId`.
#[derive(Default)]
pub(crate) struct Prints {
    sheets: HashMap<u32, Raw>,
    /// `_xlnm.Print_Area` / `_xlnm.Print_Titles` as the file wrote them,
    /// by `sheetId` (IronCalc keeps only the first part of a union such
    /// as `Sheet1!$A:$A,Sheet1!$1:$2`).
    names: HashMap<u32, (Option<String>, Option<String>)>,
    /// The layout they were read in (the opened file's).
    base: Layout,
}

impl Prints {
    /// Print settings an import read (`.xls`), by `sheetId`, with its
    /// print area and titles formulas.
    pub(crate) fn imported(
        sheets: impl IntoIterator<Item = (u32, SheetPrint)>,
        names: HashMap<u32, (Option<String>, Option<String>)>,
    ) -> Self {
        Self {
            sheets: sheets
                .into_iter()
                .map(|(id, print)| {
                    (
                        id,
                        Raw {
                            print,
                            ..Raw::default()
                        },
                    )
                })
                .collect(),
            names,
            base: Layout::default(),
        }
    }

    /// `skeletons`: the package's [`crate::preserve::worksheet_skeletons`].
    pub(crate) fn read(bytes: &[u8], skeletons: &Result<Vec<(u32, String)>, String>) -> Self {
        let mut sheets = HashMap::new();
        match skeletons {
            Ok(list) => {
                for (sheet_id, xml) in list {
                    match Document::parse(xml) {
                        Ok(doc) => {
                            sheets.insert(*sheet_id, parse_sheet(doc.root_element()));
                        }
                        Err(e) => log::warn!("print settings of sheet {sheet_id} not read: {e}"),
                    }
                }
            }
            Err(e) => log::warn!("print settings not read: {e}"),
        }
        for (id, pictures) in crate::preserve::hf_pictures(bytes) {
            if let Some(raw) = sheets.get_mut(&id) {
                raw.print.hf_pictures = pictures;
            }
        }
        Prints {
            sheets,
            names: print_names(bytes),
            base: Layout::default(),
        }
    }
}

/// The print names of `xl/workbook.xml`, by `sheetId`.
fn print_names(bytes: &[u8]) -> HashMap<u32, (Option<String>, Option<String>)> {
    use std::io::Read as _;
    let mut out: HashMap<u32, (Option<String>, Option<String>)> = HashMap::new();
    let Ok(mut zip) = zip::ZipArchive::new(std::io::Cursor::new(bytes)) else {
        return out;
    };
    let mut xml = String::new();
    match zip.by_name("xl/workbook.xml") {
        Ok(mut f) => {
            if f.read_to_string(&mut xml).is_err() {
                return out;
            }
        }
        Err(_) => return out,
    }
    let Ok(doc) = Document::parse(&xml) else {
        return out;
    };
    let ids: Vec<u32> = doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "sheet")
        .map(|n| num::<u32>(n, "sheetId").unwrap_or(u32::MAX))
        .collect();
    for n in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "definedName")
    {
        let (Some(name), Some(local)) = (n.attribute("name"), num::<usize>(n, "localSheetId"))
        else {
            continue;
        };
        let Some(&id) = ids.get(local) else { continue };
        let text = n.text().unwrap_or_default().to_owned();
        let e = out.entry(id).or_default();
        if name.eq_ignore_ascii_case("_xlnm.Print_Area") {
            e.0 = Some(text);
        } else if name.eq_ignore_ascii_case("_xlnm.Print_Titles") {
            e.1 = Some(text);
        }
    }
    out
}

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

fn flag(n: Node, name: &str, default: bool) -> bool {
    match n.attribute(name) {
        Some("1" | "true") => true,
        Some("0" | "false") => false,
        _ => default,
    }
}

fn num<T: std::str::FromStr>(n: Node, name: &str) -> Option<T> {
    n.attribute(name).and_then(|v| v.trim().parse().ok())
}

/// `"210mm"`, `"8.5in"`, `"21cm"` → millimetres.
fn length_mm(v: &str) -> Option<f64> {
    let v = v.trim();
    let (n, k) = if let Some(n) = v.strip_suffix("mm") {
        (n, 1.0)
    } else if let Some(n) = v.strip_suffix("cm") {
        (n, 10.0)
    } else {
        (v.strip_suffix("in")?, 25.4)
    };
    n.trim()
        .parse::<f64>()
        .ok()
        .filter(|x| *x > 0.0)
        .map(|x| x * k)
}

/// A chart sheet's print settings (`<chartsheet>`: margins, page setup,
/// header and footer). Without an orientation it prints landscape, as
/// Excel does.
pub(crate) fn chart_sheet_print(xml: &str) -> SheetPrint {
    let Ok(doc) = Document::parse(xml) else {
        return SheetPrint {
            setup: PageSetup {
                landscape: true,
                ..PageSetup::default()
            },
            ..SheetPrint::default()
        };
    };
    let root = doc.root_element();
    let mut p = parse_sheet(root).print;
    if child(root, "pageSetup").is_none_or(|n| n.attribute("orientation").is_none()) {
        p.setup.landscape = true;
    }
    p
}

fn parse_sheet(root: Node) -> Raw {
    let mut raw = Raw::default();
    let p = &mut raw.print;
    let fit_to_page = child(root, "sheetPr")
        .and_then(|s| child(s, "pageSetUpPr"))
        .is_some_and(|n| flag(n, "fitToPage", false));
    if let Some(n) = child(root, "sheetFormatPr") {
        raw.default_col_width = num::<f64>(n, "defaultColWidth").filter(|w| *w >= 0.0);
        raw.base_col_width = num::<f64>(n, "baseColWidth").filter(|w| *w >= 0.0);
    }
    if let Some(n) = child(root, "printOptions") {
        p.options = PrintOptions {
            grid_lines: flag(n, "gridLines", false),
            headings: flag(n, "headings", false),
            horizontal_centered: flag(n, "horizontalCentered", false),
            vertical_centered: flag(n, "verticalCentered", false),
        };
    }
    if let Some(n) = child(root, "pageMargins") {
        let d = Margins::default();
        let m = |name: &str, def: f64| {
            num::<f64>(n, name)
                .filter(|v| v.is_finite() && *v >= 0.0 && *v < 50.0)
                .unwrap_or(def)
        };
        p.margins = Margins {
            left: m("left", d.left),
            right: m("right", d.right),
            top: m("top", d.top),
            bottom: m("bottom", d.bottom),
            header: m("header", d.header),
            footer: m("footer", d.footer),
        };
    }
    let mut setup = PageSetup::default();
    let mut fit = (1, 1);
    if let Some(n) = child(root, "pageSetup") {
        let custom = match (n.attribute("paperWidth"), n.attribute("paperHeight")) {
            (Some(w), Some(h)) => length_mm(w).zip(length_mm(h)),
            _ => None,
        };
        if let Some((w, h)) = custom {
            setup.paper = Paper::mm(w, h);
        } else if let Some(paper) = num::<u32>(n, "paperSize").and_then(Paper::from_code) {
            setup.paper = paper;
        }
        setup.landscape = n.attribute("orientation") == Some("landscape");
        setup.scale = num::<u32>(n, "scale").unwrap_or(100).clamp(10, 400);
        fit = (
            num::<u32>(n, "fitToWidth").unwrap_or(1),
            num::<u32>(n, "fitToHeight").unwrap_or(1),
        );
        setup.over_then_down = n.attribute("pageOrder") == Some("overThenDown");
        if flag(n, "useFirstPageNumber", false) {
            setup.first_page_number = num::<i32>(n, "firstPageNumber");
        }
        setup.black_and_white = flag(n, "blackAndWhite", false);
        setup.comments = match n.attribute("cellComments") {
            Some("atEnd") => PrintComments::AtEnd,
            Some("asDisplayed") => PrintComments::AsDisplayed,
            _ => PrintComments::None,
        };
    }
    if fit_to_page {
        setup.fit = Some(fit);
    }
    p.setup = setup;
    if let Some(n) = child(root, "headerFooter") {
        let text = |name: &str| {
            child(n, name)
                .and_then(|c| c.text())
                .unwrap_or_default()
                .to_owned()
        };
        p.header_footer = HeaderFooter {
            odd_header: text("oddHeader"),
            odd_footer: text("oddFooter"),
            even_header: text("evenHeader"),
            even_footer: text("evenFooter"),
            first_header: text("firstHeader"),
            first_footer: text("firstFooter"),
            different_odd_even: flag(n, "differentOddEven", false),
            different_first: flag(n, "differentFirst", false),
            scale_with_doc: flag(n, "scaleWithDoc", true),
            align_with_margins: flag(n, "alignWithMargins", true),
        };
    } else {
        p.header_footer.scale_with_doc = true;
        p.header_footer.align_with_margins = true;
    }
    let breaks = |name: &str, max: u32| -> Vec<u32> {
        let Some(n) = child(root, name) else {
            return Vec::new();
        };
        let mut v: Vec<u32> = n
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "brk")
            .filter(|c| flag(*c, "man", false) || c.attribute("man").is_none())
            .filter_map(|c| num::<u32>(c, "id"))
            .filter(|&id| id > 0 && id < max)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    // `<brk id="10"/>` breaks after the tenth row: the next page starts at
    // 0-based row 10.
    p.row_breaks = breaks("rowBreaks", MAX_ROWS);
    p.col_breaks = breaks("colBreaks", MAX_COLS);
    raw
}

/// Split a defined name's formula into its references, without their sheet
/// names: `'My Sheet'!$A$1:$B$2,Sheet1!$1:$1` → `["$A$1:$B$2", "$1:$1"]`.
fn references(formula: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut quoted = false;
    let mut start = 0;
    let b = formula.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'\'' => quoted = !quoted,
            b',' if !quoted => {
                out.push(&formula[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&formula[start..]);
    out.into_iter()
        .map(|r| {
            let r = r.trim();
            // The sheet name ends at the last `!` (it may be quoted).
            r.rsplit_once('!').map_or(r, |(_, a)| a)
        })
        .filter(|r| !r.is_empty())
        .collect()
}

fn range_of(r: &str) -> Option<CellRange> {
    let a = shift::parse_range(&r.to_ascii_uppercase())?.area();
    Some(CellRange::from_corners(
        (a.top, a.left),
        (a.bottom.min(MAX_ROWS - 1), a.right.min(MAX_COLS - 1)),
    ))
}

/// A defined name's whole-row / whole-column parts: `$1:$2` → rows 0-1,
/// `$A:$B` → columns 0-1.
/// Rows and columns, inclusive.
type Titles = (Option<(u32, u32)>, Option<(u32, u32)>);

fn titles_of(formula: &str) -> Titles {
    let mut rows = None;
    let mut cols = None;
    for r in references(formula) {
        let Some(range) = range_of(r) else { continue };
        if range.is_whole_rows() && rows.is_none() {
            rows = Some((range.top, range.bottom));
        } else if range.is_whole_columns() && cols.is_none() {
            cols = Some((range.left, range.right));
        }
    }
    (rows, cols)
}

impl Workbook {
    /// How `sheet` prints: the file's page setup, margins, print options,
    /// header and footer, page breaks (moved with rows and columns inserted
    /// or deleted since it was opened), print area and print titles.
    /// Defaults (Letter, portrait, 100 %, Excel's normal margins) for a
    /// sheet that says nothing.
    pub fn print_settings(&self, sheet: u32) -> SheetPrint {
        let sheet_id = self.sheet_id(sheet);
        if let Some((p, layout)) = self.edits().prints.get(&sheet_id) {
            return moved_print(
                p.clone(),
                sheet_id,
                &layout.moves_to(&self.history.layout()),
            );
        }
        let mut p = self
            .prints
            .sheets
            .get(&sheet_id)
            .map(|r| r.print.clone())
            .unwrap_or_else(|| SheetPrint {
                header_footer: HeaderFooter {
                    scale_with_doc: true,
                    align_with_margins: true,
                    ..HeaderFooter::default()
                },
                ..SheetPrint::default()
            });
        let moves = self.prints.base.moves_to(&self.history.layout());
        if moves.touches(sheet_id) {
            let row = |r: u32| moves.point(sheet_id, r, 0).0;
            let col = |c: u32| moves.point(sheet_id, 0, c).1;
            p.row_breaks = p.row_breaks.iter().map(|&r| row(r)).collect();
            p.col_breaks = p.col_breaks.iter().map(|&c| col(c)).collect();
            p.row_breaks.retain(|&r| r > 0);
            p.col_breaks.retain(|&c| c > 0);
            p.row_breaks.dedup();
            p.col_breaks.dedup();
        }
        // The file's own names (moved with rows and columns since), else
        // the model's (new workbooks).
        let (mut area, mut titles) = self
            .prints
            .names
            .get(&sheet_id)
            .cloned()
            .unwrap_or_default();
        let from_file = area.is_some() || titles.is_some();
        if !from_file {
            for name in &self.model.get_model().workbook.defined_names {
                if name.sheet_id != Some(sheet_id) {
                    continue;
                }
                if name.name.eq_ignore_ascii_case("_xlnm.Print_Area") {
                    area = Some(name.formula.clone());
                } else if name.name.eq_ignore_ascii_case("_xlnm.Print_Titles") {
                    titles = Some(name.formula.clone());
                }
            }
        }
        let moved = |r: CellRange| -> Option<CellRange> {
            if !from_file || !moves.touches(sheet_id) {
                return Some(r);
            }
            let a = moves.area(
                sheet_id,
                shift::Area {
                    top: r.top,
                    left: r.left,
                    bottom: r.bottom,
                    right: r.right,
                },
            )?;
            Some(CellRange::from_corners(
                (a.top, a.left),
                (a.bottom, a.right),
            ))
        };
        if let Some(f) = area {
            p.print_area = references(&f)
                .into_iter()
                .filter_map(range_of)
                .filter_map(moved)
                .collect();
        }
        if let Some(f) = titles {
            let (rows, cols) = titles_of(&f);
            p.title_rows = rows
                .and_then(|(a, b)| moved(CellRange::rows(a, b)))
                .map(|r| (r.top, r.bottom));
            p.title_cols = cols
                .and_then(|(a, b)| moved(CellRange::columns(a, b)))
                .map(|r| (r.left, r.right));
        }
        p
    }

    /// Whether `sheet` is hidden (Excel leaves hidden sheets out when it
    /// prints the whole workbook).
    pub fn sheet_hidden(&self, sheet: u32) -> bool {
        self.model
            .get_model()
            .workbook
            .worksheet(sheet)
            .is_ok_and(|ws| ws.state != ironcalc_base::types::SheetState::Visible)
    }

    /// How `sheet`'s column widths map to pixels, by Excel's rules (see
    /// [`ColumnScale`]): the default font's maximum digit width and the
    /// sheet's default column width.
    pub fn column_scale(&self, sheet: u32) -> ColumnScale {
        let digit_px = digit_px(self.default_font_size());
        let raw = self.prints.sheets.get(&self.sheet_id(sheet));
        let file_default = raw.and_then(|r| r.default_col_width);
        let scale = ColumnScale {
            digit_px,
            default_px: 0.0,
            default_chars: file_default.unwrap_or(EXCEL_DEFAULT_CHARS),
        };
        let default_px = match file_default {
            Some(w) => scale.px(w),
            // baseColWidth characters, plus padding and the gridline,
            // rounded up to a multiple of 8 px.
            None => base_default_px(raw.and_then(|r| r.base_col_width).unwrap_or(8.0), digit_px),
        };
        ColumnScale {
            default_px,
            ..scale
        }
    }

    /// Column widths of `sheet` as Excel prints them ([`ColumnScale`]; a
    /// pixel is ¾ pt).
    pub fn print_columns(&self, sheet: u32) -> PrintColumns {
        let scale = self.column_scale(sheet);
        let mut spans = Vec::new();
        if let Ok(ws) = self.model.get_model().workbook.worksheet(sheet) {
            for c in &ws.cols {
                if c.min < 1 || c.max < c.min {
                    continue;
                }
                let w = scale.col_px(c) * 0.75;
                let first = c.min as u32 - 1;
                let last = (c.max as u32 - 1).min(MAX_COLS - 1);
                spans.push((first, last, w, c.hidden || c.width == 0.0));
            }
        }
        spans.sort_by_key(|s| s.0);
        PrintColumns {
            default_pt: scale.default_px * 0.75,
            spans,
            digit_px: scale.digit_px,
        }
    }

    /// What Excel prints of `sheet` when it has no print area: the range
    /// from the first to the last cell with a value or with formatting that
    /// shows on paper (a fill or a border), merged cells included. `None`
    /// for a sheet with nothing to print.
    pub fn print_used_range(&self, sheet: u32) -> Option<CellRange> {
        let ws = self.model.get_model().workbook.worksheet(sheet).ok()?;
        let mut bounds: Option<CellRange> = None;
        let mut add = |r: u32, c: u32| {
            bounds = Some(match bounds {
                None => CellRange::from_corners((r, c), (r, c)),
                Some(b) => CellRange::from_corners(
                    (b.top.min(r), b.left.min(c)),
                    (b.bottom.max(r), b.right.max(c)),
                ),
            });
        };
        // Styles that show on paper, by style index.
        let mut shows: HashMap<i32, bool> = HashMap::new();
        for (row, col, cell) in ws.sheet_data.cells() {
            if row < 1 || col < 1 {
                continue;
            }
            let (r, c) = (row as u32 - 1, col as u32 - 1);
            if self.has_input(&cell)
                || matches!(*cell, ironcalc_base::types::Cell::SpillCell { .. })
            {
                add(r, c);
                continue;
            }
            let s = cell.get_style();
            let visible = *shows.entry(s).or_insert_with(|| {
                let st = self.style_by_index(s);
                st.fill.is_some()
                    || st.borders.left.is_some()
                    || st.borders.right.is_some()
                    || st.borders.top.is_some()
                    || st.borders.bottom.is_some()
            });
            if visible {
                add(r, c);
            }
        }
        for m in &ws.merged_cells {
            if m.row >= 1 && m.column >= 1 {
                let (r, c) = (m.row as u32 - 1, m.column as u32 - 1);
                add(r, c);
                add(
                    r + m.height.max(1) as u32 - 1,
                    c + m.width.max(1) as u32 - 1,
                );
            }
        }
        bounds
    }
}

/// `p` (set in the state `moves` start from) with its print area, titles
/// and page breaks following the rows and columns moved since.
fn moved_print(mut p: SheetPrint, sheet_id: u32, moves: &shift::Moves) -> SheetPrint {
    if !moves.touches(sheet_id) {
        return p;
    }
    let row = |r: u32| moves.point(sheet_id, r, 0).0;
    let col = |c: u32| moves.point(sheet_id, 0, c).1;
    p.row_breaks = p
        .row_breaks
        .iter()
        .map(|&r| row(r))
        .filter(|&r| r > 0)
        .collect();
    p.col_breaks = p
        .col_breaks
        .iter()
        .map(|&c| col(c))
        .filter(|&c| c > 0)
        .collect();
    p.row_breaks.dedup();
    p.col_breaks.dedup();
    let moved = |r: CellRange| -> Option<CellRange> {
        let a = moves.area(
            sheet_id,
            shift::Area {
                top: r.top,
                left: r.left,
                bottom: r.bottom,
                right: r.right,
            },
        )?;
        Some(CellRange::from_corners(
            (a.top, a.left),
            (a.bottom, a.right),
        ))
    };
    p.print_area = p.print_area.into_iter().filter_map(moved).collect();
    p.title_rows = p
        .title_rows
        .and_then(|(a, b)| moved(CellRange::rows(a, b)))
        .map(|r| (r.top, r.bottom));
    p.title_cols = p
        .title_cols
        .and_then(|(a, b)| moved(CellRange::columns(a, b)))
        .map(|r| (r.left, r.right));
    p
}

/// The papers Page Setup offers, by name.
pub const PAPERS: [(&str, Paper); 9] = [
    ("A4 (210 × 297 mm)", Paper::A4),
    ("Letter (8.5 × 11 in)", Paper::LETTER),
    ("Legal (8.5 × 14 in)", Paper::LEGAL),
    ("F4 / Folio (8.5 × 13 in)", Paper::FOLIO),
    ("A3 (297 × 420 mm)", Paper::A3),
    ("A5 (148 × 210 mm)", Paper::A5),
    ("B5 (182 × 257 mm)", Paper::mm(182.0, 257.0)),
    ("Executive (7.25 × 10.5 in)", Paper::inches(7.25, 10.5)),
    ("Tabloid (11 × 17 in)", Paper::inches(11.0, 17.0)),
];

impl Paper {
    /// Excel's `paperSize` code for this paper, if it has one.
    pub fn code(&self) -> Option<u32> {
        // The first code of `from_code` that gives this size.
        [
            1, 3, 5, 7, 8, 9, 11, 12, 13, 14, 15, 16, 20, 25, 27, 28, 34, 70,
        ]
        .into_iter()
        .find(|&c| Paper::from_code(c).is_some_and(|p| p.same_as(self)))
    }

    /// Equal within a tenth of a millimetre.
    pub fn same_as(&self, other: &Paper) -> bool {
        (self.width_mm - other.width_mm).abs() < 0.1
            && (self.height_mm - other.height_mm).abs() < 0.1
    }

    /// Its name in [`PAPERS`], or `W × H mm`.
    pub fn name(&self) -> String {
        PAPERS
            .iter()
            .find(|(_, p)| p.same_as(self))
            .map(|(n, _)| (*n).to_owned())
            .unwrap_or_else(|| format!("{:.0} × {:.0} mm", self.width_mm, self.height_mm))
    }
}

impl Margins {
    /// Excel's "Wide" margins.
    pub const WIDE: Margins = Margins {
        left: 1.0,
        right: 1.0,
        top: 1.0,
        bottom: 1.0,
        header: 0.5,
        footer: 0.5,
    };
    /// Excel's "Narrow" margins.
    pub const NARROW: Margins = Margins {
        left: 0.25,
        right: 0.25,
        top: 0.75,
        bottom: 0.75,
        header: 0.3,
        footer: 0.3,
    };
}

/// Header and footer choices offered by Page Setup, as Excel lists them:
/// `(label, text with & codes)`.
pub const HEADER_FOOTER_PRESETS: [(&str, &str); 9] = [
    ("(none)", ""),
    ("Page 1", "&CPage &P"),
    ("Page 1 of ?", "&CPage &P of &N"),
    ("Sheet name", "&C&A"),
    ("File name", "&C&F"),
    ("Date", "&C&D"),
    ("Sheet name · Page 1", "&L&A&RPage &P"),
    ("File name · Page 1 of ?", "&L&F&RPage &P of &N"),
    ("Date and time · Page 1", "&L&D &T&RPage &P"),
];

/// A header or footer string split into its left, centre and right
/// sections (`&L`, `&C`, `&R`; text before any is centred).
pub fn split_sections(text: &str) -> [String; 3] {
    let mut out: [String; 3] = Default::default();
    let mut cur = 1;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            match chars.peek() {
                Some('L' | 'l') => {
                    cur = 0;
                    chars.next();
                    continue;
                }
                Some('C' | 'c') => {
                    cur = 1;
                    chars.next();
                    continue;
                }
                Some('R' | 'r') => {
                    cur = 2;
                    chars.next();
                    continue;
                }
                Some('&') => {
                    out[cur].push_str("&&");
                    chars.next();
                    continue;
                }
                _ => {}
            }
        }
        out[cur].push(c);
    }
    out
}

/// Left, centre and right sections joined into one header or footer
/// string.
pub fn join_sections(left: &str, center: &str, right: &str) -> String {
    let mut out = String::new();
    for (code, text) in [("&L", left), ("&C", center), ("&R", right)] {
        if !text.is_empty() {
            out.push_str(code);
            out.push_str(text);
        }
    }
    out
}

fn fmt_num(v: f64) -> String {
    let s = format!("{:.4}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" {
        "0".to_owned()
    } else {
        s.to_owned()
    }
}

fn esc_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// What a save writes for print settings edited in Kantor: the sheet
/// elements (`None`: remove it), `fitToPage` and the attributes of
/// `<pageSetup>` Kantor sets (the others, such as the printer settings'
/// `r:id`, are kept).
pub(crate) struct PrintXml {
    pub elements: Vec<(&'static str, Option<String>)>,
    pub fit_to_page: bool,
    /// `<pageSetup>` attributes: `(name, value)`; those listed in
    /// [`PAGE_SETUP_ATTRS`] and not here are removed.
    pub page_setup: Vec<(&'static str, String)>,
}

/// The `<pageSetup>` attributes Kantor writes (others are kept as they are).
pub(crate) const PAGE_SETUP_ATTRS: [&str; 13] = [
    "cellComments",
    "paperSize",
    "paperWidth",
    "paperHeight",
    "orientation",
    "scale",
    "fitToWidth",
    "fitToHeight",
    "pageOrder",
    "firstPageNumber",
    "useFirstPageNumber",
    "blackAndWhite",
    "usePrinterDefaults",
];

pub(crate) fn print_xml(p: &SheetPrint) -> PrintXml {
    let o = &p.options;
    let mut opts = String::new();
    for (name, on) in [
        ("horizontalCentered", o.horizontal_centered),
        ("verticalCentered", o.vertical_centered),
        ("headings", o.headings),
        ("gridLines", o.grid_lines),
    ] {
        if on {
            opts.push_str(&format!(" {name}=\"1\""));
        }
    }
    let print_options = (!opts.is_empty()).then(|| format!("<printOptions{opts}/>"));
    let m = &p.margins;
    let margins = format!(
        "<pageMargins left=\"{}\" right=\"{}\" top=\"{}\" bottom=\"{}\" header=\"{}\" footer=\"{}\"/>",
        fmt_num(m.left),
        fmt_num(m.right),
        fmt_num(m.top),
        fmt_num(m.bottom),
        fmt_num(m.header),
        fmt_num(m.footer)
    );
    let s = &p.setup;
    let mut attrs: Vec<(&'static str, String)> = Vec::new();
    match s.paper.code() {
        Some(code) => attrs.push(("paperSize", code.to_string())),
        None => {
            attrs.push(("paperWidth", format!("{}mm", fmt_num(s.paper.width_mm))));
            attrs.push(("paperHeight", format!("{}mm", fmt_num(s.paper.height_mm))));
        }
    }
    if s.scale != 100 {
        attrs.push(("scale", s.scale.clamp(10, 400).to_string()));
    }
    if let Some(n) = s.first_page_number {
        attrs.push(("firstPageNumber", n.to_string()));
    }
    if let Some((w, h)) = s.fit {
        if w != 1 {
            attrs.push(("fitToWidth", w.to_string()));
        }
        if h != 1 {
            attrs.push(("fitToHeight", h.to_string()));
        }
    }
    if s.over_then_down {
        attrs.push(("pageOrder", "overThenDown".into()));
    }
    attrs.push((
        "orientation",
        if s.landscape { "landscape" } else { "portrait" }.into(),
    ));
    if s.black_and_white {
        attrs.push(("blackAndWhite", "1".into()));
    }
    match s.comments {
        PrintComments::None => {}
        PrintComments::AtEnd => attrs.push(("cellComments", "atEnd".into())),
        PrintComments::AsDisplayed => attrs.push(("cellComments", "asDisplayed".into())),
    }
    if s.first_page_number.is_some() {
        attrs.push(("useFirstPageNumber", "1".into()));
    }
    let hf = &p.header_footer;
    let header_footer = if hf.is_empty() && !hf.different_first && !hf.different_odd_even {
        None
    } else {
        let mut a = String::new();
        if hf.different_odd_even {
            a.push_str(" differentOddEven=\"1\"");
        }
        if hf.different_first {
            a.push_str(" differentFirst=\"1\"");
        }
        if !hf.scale_with_doc {
            a.push_str(" scaleWithDoc=\"0\"");
        }
        if !hf.align_with_margins {
            a.push_str(" alignWithMargins=\"0\"");
        }
        let mut body = String::new();
        for (name, text) in [
            ("oddHeader", &hf.odd_header),
            ("oddFooter", &hf.odd_footer),
            ("evenHeader", &hf.even_header),
            ("evenFooter", &hf.even_footer),
            ("firstHeader", &hf.first_header),
            ("firstFooter", &hf.first_footer),
        ] {
            if !text.is_empty() {
                body.push_str(&format!("<{name}>{}</{name}>", esc_xml(text)));
            }
        }
        Some(format!("<headerFooter{a}>{body}</headerFooter>"))
    };
    // Manual page breaks, as Excel writes them.
    let breaks = |name: &str, ids: &[u32], max: u32| {
        (!ids.is_empty()).then(|| {
            let n = ids.len();
            let body: String = ids
                .iter()
                .map(|id| format!("<brk id=\"{id}\" max=\"{max}\" man=\"1\"/>"))
                .collect();
            format!("<{name} count=\"{n}\" manualBreakCount=\"{n}\">{body}</{name}>")
        })
    };
    PrintXml {
        elements: vec![
            ("printOptions", print_options),
            ("pageMargins", Some(margins)),
            ("headerFooter", header_footer),
            (
                "rowBreaks",
                breaks("rowBreaks", &p.row_breaks, MAX_COLS - 1),
            ),
            (
                "colBreaks",
                breaks("colBreaks", &p.col_breaks, MAX_ROWS - 1),
            ),
        ],
        fit_to_page: s.fit.is_some(),
        page_setup: attrs,
    }
}

/// `Sheet1` or `'My Sheet'` for a formula.
pub(crate) fn quote_sheet(name: &str) -> String {
    let plain = name.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !name.chars().next().is_some_and(|c| c.is_ascii_digit())
        && crate::formula::parse_range(name).is_none();
    if plain {
        name.to_owned()
    } else {
        format!("'{}'", name.replace('\'', "''"))
    }
}

/// The `_xlnm.Print_Area` and `_xlnm.Print_Titles` formulas of `p` on the
/// sheet called `sheet` (titles: columns first, as Excel writes them).
pub(crate) fn name_formulas(p: &SheetPrint, sheet: &str) -> (Option<String>, Option<String>) {
    let q = quote_sheet(sheet);
    let abs = |r: &CellRange| {
        let a = format!("${}${}", crate::column_name(r.left), r.top + 1);
        if r.is_single() {
            a
        } else {
            format!("{a}:${}${}", crate::column_name(r.right), r.bottom + 1)
        }
    };
    let area = (!p.print_area.is_empty()).then(|| {
        p.print_area
            .iter()
            .map(|r| format!("{q}!{}", abs(r)))
            .collect::<Vec<_>>()
            .join(",")
    });
    let mut titles = Vec::new();
    if let Some((a, b)) = p.title_cols {
        titles.push(format!(
            "{q}!${}:${}",
            crate::column_name(a),
            crate::column_name(b)
        ));
    }
    if let Some((a, b)) = p.title_rows {
        titles.push(format!("{q}!${}:${}", a + 1, b + 1));
    }
    (area, (!titles.is_empty()).then(|| titles.join(",")))
}

/// Whether a defined name's formula has several references (a union, which
/// IronCalc keeps only the first part of).
pub(crate) fn is_union(formula: &str) -> bool {
    references(formula).len() > 1
}

impl Workbook {
    /// A theme colour of the workbook (`slot` as in OOXML: 0 Background 1,
    /// 1 Text 1, 4-9 accents) with a tint (-1 darker .. +1 lighter), as
    /// `0xRRGGBB`.
    pub fn theme_rgb(&self, slot: i32, tint: f64) -> u32 {
        crate::table::theme(&self.model.get_model().workbook.theme, slot, tint)
    }

    /// Set how `sheet` prints (Page Setup): one undo step. Print area and
    /// titles are in the current rows and columns.
    pub fn set_print_settings(&mut self, sheet: u32, settings: SheetPrint) -> anyhow::Result<()> {
        if sheet as usize >= self.sheet_names().len() {
            anyhow::bail!("no sheet {sheet}");
        }
        if settings == self.print_settings(sheet) {
            return Ok(());
        }
        let sheet_id = self.sheet_id(sheet);
        self.own_edit(|e, layout| {
            e.prints.insert(sheet_id, (settings, layout.clone()));
        });
        Ok(())
    }

    /// Insert manual page breaks (Excel's Insert Page Break): a new page
    /// starts at `row` and/or at `col` (0-based; row or column 0 has none).
    /// One undo step.
    pub fn insert_page_break(
        &mut self,
        sheet: u32,
        row: Option<u32>,
        col: Option<u32>,
    ) -> anyhow::Result<()> {
        let mut p = self.print_settings(sheet);
        let add = |v: &mut Vec<u32>, at: Option<u32>| {
            if let Some(at) = at.filter(|&a| a > 0)
                && let Err(i) = v.binary_search(&at)
            {
                v.insert(i, at);
            }
        };
        add(&mut p.row_breaks, row);
        add(&mut p.col_breaks, col);
        self.set_print_settings(sheet, p)
    }

    /// The manual page breaks at the cell `(row, col)`: the one above its
    /// row and the one left of its column (what Remove Page Break takes).
    pub fn page_breaks_at(&self, sheet: u32, row: u32, col: u32) -> (bool, bool) {
        let p = self.print_settings(sheet);
        (p.row_breaks.contains(&row), p.col_breaks.contains(&col))
    }

    /// Remove the manual page breaks above row `row` and left of column
    /// `col` (Excel's Remove Page Break). One undo step.
    pub fn remove_page_break(
        &mut self,
        sheet: u32,
        row: Option<u32>,
        col: Option<u32>,
    ) -> anyhow::Result<()> {
        let mut p = self.print_settings(sheet);
        p.row_breaks.retain(|&r| Some(r) != row);
        p.col_breaks.retain(|&c| Some(c) != col);
        self.set_print_settings(sheet, p)
    }

    /// Drag a page break in Page Break Preview: the manual break at `from`
    /// (if there is one: an automatic break has none) goes, and a manual
    /// break starts a new page at `to` instead (`None`, or row / column 0:
    /// dragged off the printed range, removed). Rows when `rows`, else
    /// columns. One undo step.
    pub fn move_page_break(
        &mut self,
        sheet: u32,
        rows: bool,
        from: Option<u32>,
        to: Option<u32>,
    ) -> anyhow::Result<()> {
        let mut p = self.print_settings(sheet);
        let list = if rows {
            &mut p.row_breaks
        } else {
            &mut p.col_breaks
        };
        list.retain(|&b| Some(b) != from);
        if let Some(to) = to.filter(|&t| t > 0)
            && let Err(i) = list.binary_search(&to)
        {
            list.insert(i, to);
        }
        self.set_print_settings(sheet, p)
    }

    /// Set the print area of `sheet` (Page Break Preview's dragged edges,
    /// Set Print Area); empty clears it. One undo step.
    pub fn set_print_area(&mut self, sheet: u32, area: Vec<CellRange>) -> anyhow::Result<()> {
        let mut p = self.print_settings(sheet);
        p.print_area = area;
        self.set_print_settings(sheet, p)
    }

    /// Remove every manual page break of `sheet` (Reset All Page Breaks).
    pub fn reset_page_breaks(&mut self, sheet: u32) -> anyhow::Result<()> {
        let mut p = self.print_settings(sheet);
        p.row_breaks.clear();
        p.col_breaks.clear();
        self.set_print_settings(sheet, p)
    }

    /// Patches a save writes for print settings: sheets whose settings were
    /// edited, and sheets whose print names are a union (IronCalc keeps
    /// only its first part).
    pub(crate) fn print_patches(&self, out: &mut crate::preserve::Patches) {
        let edits = self.edits();
        let names = self.sheet_names();
        for (i, name) in names.iter().enumerate() {
            let sheet = i as u32;
            let id = self.sheet_id(sheet);
            if edits.prints.contains_key(&id) || self.touched.prints.contains(&id) {
                let p = self.print_settings(sheet);
                let x = print_xml(&p);
                let patch = out.sheets.entry(id).or_default();
                patch.elements.extend(x.elements);
                patch.fit_to_page = Some(x.fit_to_page);
                patch.page_setup = Some(x.page_setup);
                let (area, titles) = name_formulas(&p, name);
                out.names.push((id, "_xlnm.Print_Area", area));
                out.names.push((id, "_xlnm.Print_Titles", titles));
            } else if let Some((area, titles)) = self.prints.names.get(&id) {
                let union = area.as_deref().is_some_and(is_union)
                    || titles.as_deref().is_some_and(is_union);
                if union {
                    let p = self.print_settings(sheet);
                    let (a, t) = name_formulas(&p, name);
                    if area.is_some() {
                        out.names.push((id, "_xlnm.Print_Area", a));
                    }
                    if titles.is_some() {
                        out.names.push((id, "_xlnm.Print_Titles", t));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {

    /// Excel's column pixels: the default 8.43 characters (stored as
    /// 9.140625) are 64 px with a 7 px digit, and every whole pixel width
    /// is stored as a width that reads back as the same pixels.
    #[std::prelude::v1::test]
    fn column_scale_round_trips_pixels() {
        // Calibri 11, Excel's default font.
        let s = ColumnScale {
            digit_px: 7.0,
            default_px: 64.0,
            default_chars: EXCEL_DEFAULT_CHARS,
        };
        assert_eq!(s.px(9.140625), 64.0);
        assert_eq!(s.width_of(64.0), 9.140625);
        // 100 px: 13.57 characters shown, stored with the padding.
        assert_eq!(s.width_of(100.0), 14.28125);
        for px in 1..=600 {
            let w = s.width_of(f64::from(px));
            assert_eq!(s.px(w), f64::from(px), "{px} px -> {w}");
            assert_ne!(w, 10.0, "IronCalc's default marker");
        }
        // Fractional pixels are rounded; nothing is zero.
        assert_eq!(s.px(s.width_of(101.7)), 102.0);
        // A new workbook's default font is 12 pt (IronCalc's): an 8 px
        // digit, so 8 characters plus padding round up to 72 px.
        let wb = Workbook::new_empty().unwrap();
        let s = wb.column_scale(0);
        assert_eq!((s.digit_px, s.default_px), (8.0, 72.0));
        assert_eq!(wb.column_width(0, 5), 72.0);
    }

    use super::*;

    #[test]
    fn papers() {
        assert_eq!(Paper::from_code(9), Some(Paper::A4));
        assert_eq!(Paper::from_code(14), Some(Paper::FOLIO));
        assert_eq!(Paper::from_name("F4"), Some(Paper::FOLIO));
        assert_eq!(Paper::from_name("100x200mm"), Some(Paper::mm(100.0, 200.0)));
        assert_eq!(Paper::from_name("8.5x14in"), Some(Paper::LEGAL));
        assert_eq!(Paper::from_name("huge"), None);
        assert_eq!(length_mm("21cm"), Some(210.0));
    }

    #[test]
    fn defined_name_references() {
        assert_eq!(
            references("'It''s, here'!$A$1:$B$2,Sheet1!$1:$1"),
            vec!["$A$1:$B$2", "$1:$1"]
        );
        assert_eq!(
            range_of("$B$2:$D$9"),
            Some(CellRange::from_corners((1, 1), (8, 3)))
        );
        assert_eq!(
            titles_of("Sheet1!$A:$B,Sheet1!$1:$2"),
            (Some((0, 1)), Some((0, 1)))
        );
        assert_eq!(titles_of("'S'!$3:$3"), (Some((2, 2)), None));
    }

    #[test]
    fn sheet_xml() {
        let xml = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetPr><pageSetUpPr fitToPage="1"/></sheetPr>
<sheetFormatPr defaultRowHeight="15" defaultColWidth="10"/>
<sheetData/>
<printOptions gridLines="1" horizontalCentered="1"/>
<pageMargins left="0.25" right="0.25" top="1" bottom="1" header="0.5" footer="0.5"/>
<pageSetup paperSize="14" orientation="landscape" fitToWidth="1" fitToHeight="0" pageOrder="overThenDown" firstPageNumber="5" useFirstPageNumber="1"/>
<headerFooter differentFirst="1"><oddHeader>&amp;CReport</oddHeader><firstFooter>&amp;RPage &amp;P</firstFooter></headerFooter>
<rowBreaks count="2" manualBreakCount="2"><brk id="20" max="16383" man="1"/><brk id="10" max="16383" man="1"/></rowBreaks>
<colBreaks count="1"><brk id="3" max="1048575" man="1"/></colBreaks>
</worksheet>"#;
        let doc = Document::parse(xml).unwrap();
        let raw = parse_sheet(doc.root_element());
        let p = &raw.print;
        assert_eq!(p.setup.paper, Paper::FOLIO);
        assert!(p.setup.landscape && p.setup.over_then_down);
        assert_eq!(p.setup.fit, Some((1, 0)));
        assert_eq!(p.setup.first_page_number, Some(5));
        assert_eq!(p.margins.left, 0.25);
        assert_eq!(p.margins.header, 0.5);
        assert!(p.options.grid_lines && p.options.horizontal_centered);
        assert!(!p.options.headings);
        assert_eq!(p.row_breaks, vec![10, 20]);
        assert_eq!(p.col_breaks, vec![3]);
        assert_eq!(p.header_footer.odd_header, "&CReport");
        assert_eq!(p.header_footer.for_page(1).1, "&RPage &P");
        assert_eq!(p.header_footer.for_page(2).0, "&CReport");
        assert_eq!(raw.default_col_width, Some(10.0));
    }
}
