//! Kantor Sheets → PDF (no GPUI): a workbook printed the way Excel prints
//! it.
//!
//! Each sheet's print settings (`sheets_engine::SheetPrint`: paper,
//! orientation, scale or fit to pages, margins, gridlines and headings,
//! centring, print area and titles, manual page breaks, page order,
//! headers and footers) decide its pages; row heights and column widths
//! are the file's, in points as Excel prints them; hidden rows and columns
//! are left out. Cells are drawn with their number formats, fonts, fills,
//! borders, alignment (wrap, indent, rotation, shrink to fit, centre across
//! selection), merged cells, text overflowing into empty neighbours,
//! conditional formatting (formats, data bars, icons) and table styles.
//! Text is shaped by parley and written with subset embedded fonts and
//! ToUnicode, so the PDF can be searched and copied.
//!
//! Three steps, so the slow ones can leave the UI thread:
//! 1. [`Plan::new`]: how every sheet splits into pages (needs the
//!    workbook; quick).
//! 2. [`extract`]: one page's cells, as plain data ([`PageContent`] is
//!    `Send`; needs the workbook).
//! 3. [`Encoder`]: shapes and writes pages, then [`Encoder::finish`]
//!    returns the PDF. Pages are written as they come, so memory stays
//!    bounded by one page plus the PDF itself.
//!
//! [`export`] does all three for headless callers (`kantor convert`).
//!
//! Charts and pictures on a sheet print over its cells where they sit on
//! the grid ([`drawings`]): an object reaching over a page break is split,
//! each page showing its part; objects in repeated title rows or columns
//! aren't repeated (only the cells are, as in Excel). Charts are drawn by
//! the renderer Slides uses, with live cell data and vector, searchable
//! text; pictures are embedded at their own resolution. Hidden objects and
//! those set not to print are left out, and so are shapes and text boxes
//! (Kantor can't draw them yet).

pub mod drawings;
mod encode;
mod extract;
pub mod hf;
pub mod paginate;
mod text;

use std::sync::Arc;

use anyhow::Result;
use sheets_engine::{CellRange, Paper, PrintColumns, SheetPrint, Workbook};

pub use docs_pdf::write_file;
pub use encode::{BadPicture, Encoder};
pub use extract::extract;
pub use text::Shaper;

/// Points per inch.
const INCH: f64 = 72.0;
const MM: f64 = 72.0 / 25.4;

/// What to print and how, beyond the file's own settings.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Sheets to print (0-based); `None`: every visible sheet.
    pub sheets: Option<Vec<u32>>,
    /// Paper, overriding the sheets'.
    pub paper: Option<Paper>,
    /// Orientation, overriding the sheets'.
    pub landscape: Option<bool>,
    /// Fit to `(pages wide, pages tall)` (0: no limit), overriding the
    /// sheets' scale.
    pub fit: Option<(u32, u32)>,
    /// Scale in percent, overriding the sheets' (and their fit to pages).
    pub scale: Option<u32>,
    /// Print each sheet's used range even when it has a print area.
    pub ignore_print_area: bool,
    /// For `&F` / `&Z` in headers and footers.
    pub file_name: String,
    pub folder: String,
    /// For `&D` / `&T`; [`Options::now`] fills them with the local time.
    pub date: String,
    pub time: String,
}

impl Options {
    /// Fill `date` and `time` with the current local date and time
    /// (`11/11/2026`, `2:05 PM`).
    pub fn now(mut self) -> Self {
        use chrono::{Datelike as _, Timelike as _};
        let now = chrono::Local::now();
        self.date = format!("{}/{}/{}", now.month(), now.day(), now.year());
        let (pm, h) = now.hour12();
        self.time = format!("{}:{:02} {}", h, now.minute(), if pm { "PM" } else { "AM" });
        self
    }

    /// Set `file_name` and `folder` from a path.
    pub fn with_path(mut self, path: Option<&std::path::Path>) -> Self {
        if let Some(p) = path {
            self.file_name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.folder = p
                .parent()
                .map(|d| {
                    let mut s = d.to_string_lossy().into_owned();
                    if !s.ends_with(std::path::MAIN_SEPARATOR) {
                        s.push(std::path::MAIN_SEPARATOR);
                    }
                    s
                })
                .unwrap_or_default();
        }
        self
    }
}

/// How one sheet prints.
#[derive(Debug, Clone)]
pub struct SheetPlan {
    pub sheet: u32,
    pub name: String,
    pub print: SheetPrint,
    pub columns: PrintColumns,
    /// Height of rows `0..rows.len()` in points (0 when hidden); rows past
    /// the end are `default_row`.
    pub rows: Vec<f64>,
    pub default_row: f64,
    /// Page size (oriented), points.
    pub page: (f64, f64),
    /// Width of the row headings and height of the column headings
    /// (0 without headings), points.
    pub heading: (f64, f64),
    /// The ranges printed and how each splits.
    pub areas: Vec<(CellRange, paginate::Grid)>,
    /// Charts and pictures to print, and where cells are on the sheet.
    pub drawings: drawings::SheetDrawings,
}

impl SheetPlan {
    pub fn row_height(&self, row: u32) -> f64 {
        self.rows
            .get(row as usize)
            .copied()
            .unwrap_or(self.default_row)
    }

    pub fn col_width(&self, col: u32) -> f64 {
        self.columns.width_pt(col)
    }
}

/// One page to print.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageRef {
    /// Index into [`Plan::sheets`].
    pub sheet: usize,
    pub area: usize,
    /// Column and row band of the area's grid.
    pub cols: usize,
    pub rows: usize,
    /// The page's number for `&P`.
    pub number: i64,
    /// 1-based position within its sheet (for "different first page").
    pub nth: usize,
}

/// Every page of a print job.
#[derive(Debug, Clone)]
pub struct Plan {
    pub title: String,
    pub sheets: Vec<SheetPlan>,
    pub pages: Vec<PageRef>,
    pub options: Options,
}

impl Plan {
    /// Work out the pages of the sheets `opts` asks for. `shaper` measures
    /// wrapped text for rows that fit their content (rows without a stored
    /// height).
    pub fn new(book: &Workbook, opts: &Options, shaper: &mut Shaper) -> Plan {
        let names = book.sheet_names();
        let sheets: Vec<u32> = match &opts.sheets {
            Some(s) => s
                .iter()
                .copied()
                .filter(|&s| (s as usize) < names.len())
                .collect(),
            None => (0..names.len() as u32)
                .filter(|&s| !book.sheet_hidden(s))
                .collect(),
        };
        let mut plans = Vec::new();
        let mut pages = Vec::new();
        let mut number = 0i64;
        for sheet in sheets {
            let plan = sheet_plan(book, sheet, &names[sheet as usize], opts, shaper);
            let ix = plans.len();
            let mut nth = 0;
            for (area, (_, grid)) in plan.areas.iter().enumerate() {
                for (c, r) in paginate::order(grid, plan.print.setup.over_then_down) {
                    nth += 1;
                    number = match plan.print.setup.first_page_number {
                        Some(first) if nth == 1 => i64::from(first),
                        _ => number + 1,
                    };
                    pages.push(PageRef {
                        sheet: ix,
                        area,
                        cols: c,
                        rows: r,
                        number,
                        nth,
                    });
                }
            }
            plans.push(plan);
        }
        Plan {
            title: book.title(),
            sheets: plans,
            pages,
            options: opts.clone(),
        }
    }
}

fn sheet_plan(
    book: &Workbook,
    sheet: u32,
    name: &str,
    opts: &Options,
    shaper: &mut Shaper,
) -> SheetPlan {
    let mut print = book.print_settings(sheet);
    if let Some(p) = opts.paper {
        print.setup.paper = p;
    }
    if let Some(l) = opts.landscape {
        print.setup.landscape = l;
    }
    if let Some(s) = opts.scale {
        print.setup.scale = s.clamp(10, 400);
        print.setup.fit = None;
    }
    if let Some(f) = opts.fit {
        print.setup.fit = Some(f);
    }
    let columns = book.print_columns(sheet);
    let spec = book.row_spec(sheet);
    let found = drawings::printed(book, sheet);
    let areas: Vec<CellRange> = if print.print_area.is_empty() || opts.ignore_print_area {
        // Excel prints the cells in use and the objects over the sheet.
        let used = book.print_used_range(sheet);
        let stored: std::collections::HashMap<u32, f64> = spec
            .rows
            .iter()
            .map(|e| (e.row, if e.hidden { 0.0 } else { e.points }))
            .collect();
        let row_h = |r: u32| stored.get(&r).copied().unwrap_or(spec.default_points);
        let drawn = drawings::cell_bounds(&found, &|c| columns.width_pt(c), &row_h);
        match (used, drawn) {
            (Some(a), Some(b)) => vec![CellRange::from_corners(
                (a.top.min(b.top), a.left.min(b.left)),
                (a.bottom.max(b.bottom), a.right.max(b.right)),
            )],
            (a, b) => a.or(b).into_iter().collect(),
        }
    } else {
        print.print_area.clone()
    };
    // Row heights up to the last row printed.
    let last_row = areas
        .iter()
        .map(|a| a.bottom)
        .chain(print.title_rows.map(|t| t.1))
        .max();
    let default_row = spec.default_points;
    let mut rows = Vec::new();
    // Rows under drawings too, so their sizes follow the rows'.
    let last_row_drawn = last_row.max(drawings::last_row(&found));
    if let Some(last) = last_row_drawn {
        rows = vec![default_row; last as usize + 1];
        let mut stored = vec![false; rows.len()];
        for e in spec.rows.iter().filter(|e| e.row <= last) {
            rows[e.row as usize] = if e.hidden { 0.0 } else { e.points };
            stored[e.row as usize] = true;
        }
        extract::fit_rows(book, sheet, &columns, &mut rows, &stored, &areas, shaper);
    }
    let (pw, ph) = (
        print.setup.paper.width_mm * MM,
        print.setup.paper.height_mm * MM,
    );
    let page = if print.setup.landscape {
        (pw.max(ph), pw.min(ph))
    } else {
        (pw.min(ph), pw.max(ph))
    };
    let heading = if print.options.headings {
        let digits = last_row.map_or(1, |r| (r + 1).to_string().len()) as f64;
        (
            (digits * columns.digit_px + 8.0).max(3.0 * columns.digit_px) * 0.75,
            default_row,
        )
    } else {
        (0.0, 0.0)
    };
    let m = print.margins;
    let avail_w = (page.0 - (m.left + m.right) * INCH).max(36.0);
    let avail_h = (page.1 - (m.top + m.bottom) * INCH).max(36.0);
    let mut plan = SheetPlan {
        sheet,
        name: name.to_owned(),
        print,
        columns,
        rows,
        default_row,
        page,
        heading,
        areas: Vec::new(),
        drawings: Default::default(),
    };
    let mut grids = Vec::new();
    for area in areas {
        let col_sizes: Vec<f64> = (area.left..=area.right)
            .map(|c| plan.col_width(c))
            .collect();
        let row_sizes: Vec<f64> = (area.top..=area.bottom)
            .map(|r| plan.row_height(r))
            .collect();
        let p = &plan.print;
        let titles = |t: Option<(u32, u32)>, size: &dyn Fn(u32) -> f64| {
            t.map(|(a, b)| ((a, b), (a..=b).map(size).sum::<f64>()))
        };
        let col_titles = titles(p.title_cols, &|c| plan.col_width(c));
        let row_titles = titles(p.title_rows, &|r| plan.row_height(r));
        let cols = paginate::Axis {
            first: area.left,
            sizes: &col_sizes,
            titles: col_titles,
            breaks: &p.col_breaks,
            fixed: plan.heading.0,
        };
        let rows = paginate::Axis {
            first: area.top,
            sizes: &row_sizes,
            titles: row_titles,
            breaks: &p.row_breaks,
            fixed: plan.heading.1,
        };
        let grid = paginate::grid(&cols, &rows, avail_w, avail_h, p.setup.scale, p.setup.fit);
        grids.push((area, grid));
    }
    plan.areas = grids;
    plan.drawings = drawings::SheetDrawings::new(book, found, &plan);
    plan
}

/// A chart or picture on a page, in the page's content coordinates
/// (points before the print scale, from the top-left of the printed
/// block). `rect` is the whole object, which may reach off the page;
/// `clip` is the part of the page it may show in (the page's own cells,
/// not the repeated titles).
#[derive(Debug, Clone)]
pub struct Overlay {
    pub rect: [f32; 4],
    pub clip: [f32; 4],
    pub kind: OverlayKind,
}

#[derive(Debug, Clone)]
pub enum OverlayKind {
    /// A picture's media part, bytes and cropping (left, top, right,
    /// bottom fractions).
    Picture {
        part: Arc<str>,
        data: Arc<Vec<u8>>,
        crop: [f32; 4],
    },
    /// A chart with the cells' values read when the page was extracted;
    /// laid out by the encoder. `hash` identifies the data, so a chart split
    /// over pages is laid out once.
    Chart {
        key: sheets_engine::drawing::DrawingKey,
        hash: u64,
        chart: Arc<slides_scene::chart::Chart>,
        defaults: Arc<slides_scene::chart::Defaults>,
    },
}

/// One page's content, ready to encode (plain data, `Send`). Content
/// coordinates are points before the print scale, from the top-left of the
/// printed block (headings included).
#[derive(Debug, Clone, Default)]
pub struct PageContent {
    /// Page size, points.
    pub width: f32,
    pub height: f32,
    /// Where the block's top-left corner is on the page, points.
    pub origin: [f32; 2],
    /// Print scale (1.0 = 100 %).
    pub scale: f32,
    /// Size of the block (unscaled).
    pub block: [f32; 2],
    pub fills: Vec<([f32; 4], u32)>,
    /// Data bars and icons, under the text.
    pub bars: Vec<Bar>,
    pub icons: Vec<Icon>,
    pub texts: Vec<CellText>,
    /// Gridlines and borders, over the text.
    pub lines: Vec<Line>,
    pub overlays: Vec<Overlay>,
    pub header: hf::Parts,
    pub footer: hf::Parts,
    /// Header and footer placement and their default font.
    pub hf: HfLayout,
}

/// Where the header and footer go.
#[derive(Debug, Clone, Default)]
pub struct HfLayout {
    /// Left and right edges for the side sections, points.
    pub left: f32,
    pub right: f32,
    /// Top of the header and bottom of the footer, points from the page's
    /// top and bottom edges.
    pub header: f32,
    pub footer: f32,
    /// Font size factor (the print scale with `scaleWithDoc`).
    pub scale: f32,
    pub family: Arc<str>,
    pub size: f32,
}

/// A border or gridline segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub style: LineKind,
    pub color: u32,
}

/// How a line is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Grid,
    Border(sheets_engine::LineStyle),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bar {
    /// The cell.
    pub rect: [f32; 4],
    pub bar: sheets_engine::DataBar,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Icon {
    pub rect: [f32; 4],
    pub icon: sheets_engine::CfIcon,
}

/// Horizontal placement of cell text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
    /// Repeat the text across the cell.
    Fill,
    /// Wrapped and justified (Excel's justify and distributed).
    Justify,
}

/// A cell's text and how to place it.
#[derive(Debug, Clone, PartialEq)]
pub struct CellText {
    pub text: String,
    pub font: text::FontKey,
    pub underline: bool,
    pub strike: bool,
    pub color: u32,
    /// The cell (or merged block) the text is aligned in.
    pub cell: [f32; 4],
    /// What may show: the cell, or the empty cells it may overflow into,
    /// within the page.
    pub clip: [f32; 4],
    pub align: Align,
    pub valign: sheets_engine::VAlign,
    pub wrap: bool,
    /// Indent, points.
    pub indent: f32,
    pub rotation: u32,
    pub shrink: bool,
    /// A number: shown as `####` when it doesn't fit.
    pub number: bool,
    /// A repeat character from the number format and where it goes.
    pub repeat: Option<(char, usize)>,
    /// Room kept at the left for an icon, points.
    pub icon_room: f32,
}

pub use text::FontKey;

/// Print `opts.sheets` of a workbook to PDF. Returns the PDF and its page
/// count.
pub fn export(book: &mut Workbook, opts: &Options) -> Result<(Vec<u8>, usize)> {
    let mut shaper = Shaper::new();
    let plan = Plan::new(book, opts, &mut shaper);
    let pages = plan.pages.len().max(1);
    let mut skip = std::collections::HashSet::new();
    let mut shaper = Some(shaper);
    loop {
        let sh = shaper.take().unwrap_or_default();
        let mut enc = Encoder::new(&plan.title, sh).skip_pictures(&skip);
        for i in 0..plan.pages.len() {
            let page = extract(book, &plan, i);
            enc.page(&page);
        }
        match enc.finish() {
            Ok(pdf) => return Ok((pdf, pages)),
            // Leave out a picture krilla can't decode and write it again.
            Err(e) => match e.downcast_ref::<BadPicture>() {
                Some(bad) if skip.insert(bad.0) => {}
                _ => return Err(e),
            },
        }
    }
}
