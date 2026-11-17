//! Formatting of Excel 97-2003 (`.xls`, `.xlt`) files: a BIFF8 reader for
//! what calamine leaves out. calamine brings the values and formulas (it has
//! the formula decompiler); this module walks the same `Workbook` stream again
//! for the look of the sheets and applies it to the IronCalc model:
//!
//! * cell formats (`XF`, with `FONT`, `FORMAT` and `PALETTE`): font, number
//!   format, alignment (wrap, indent, rotation, shrink), borders with their
//!   styles and colours, fill, protection. Fill patterns other than solid
//!   become a solid colour mixed from the pattern's two colours by its
//!   density (IronCalc fills have one colour);
//! * merged cells, column widths and hidden columns (`COLINFO`,
//!   `DEFCOLWIDTH`), row heights and hidden rows (`ROW`, `DEFAULTROWHEIGHT`);
//! * frozen panes, gridlines and the active sheet (`PANE`, `WINDOW2`,
//!   `WINDOW1`), hidden sheets (`BOUNDSHEET`), tab colours (`SHEETEXT`);
//! * hyperlinks with their tooltips (`HLINK`, `HLINKTOOLTIP`);
//! * page setup (`SETUP`, margins, centring, `WSBOOL` fit to page,
//!   header/footer, gridlines and headings, manual page breaks);
//! * the 1904 date system (`DATEMODE`) and which defined names are
//!   sheet-local or built in (`LBL`), for pairing with calamine's names.
//!
//! * notes (`NOTE` with the text of its `OBJ`/`TXO` box) into the model's
//!   comments, which an `.xlsx` save writes (`notes`);
//! * data validation (`DV`) and conditional formats (`CONDFMT`/`CF`: cell
//!   value and formula rules with font, border, fill and number formats),
//!   their formulas decompiled from tokens by [`crate::xls_formula`]; Excel
//!   2007 rules (`CONDFMT12`/`CF12`: colour scales, data bars, icon sets,
//!   top/bottom, and the rule templates also kept in `CFEX` for a `CF`:
//!   text, dates, blanks, errors, duplicates, averages), see [`cf12`]. A
//!   rule whose formula has an unsupported token is left out (with an
//!   import note).
//!
//! * pictures and charts (`MSODRAWINGGROUP`, `MSODRAWING`, `OBJ` and chart
//!   substreams), read by [`crate::xls_drawing`].
//!
//! Excel 5/95 files (BIFF5, the stream called `Book`) are read the same
//! way for what they share with BIFF8: strings are a count and bytes in the
//! file's code page (`CODEPAGE`, see [`BIFF5`]), `FORMAT` counts are 8-bit,
//! and `XF`s use the older packing ([`Xf::parse5`]). Their notes, objects
//! and links (other records in BIFF5) are left out.
//!
//! Not read: named cell styles (`STYLE`; cells keep the formats they show),
//! shapes and text boxes. Every read is bounds-checked: a malformed
//! record is skipped, never a panic.

use std::collections::HashMap;
use std::io::Read as _;
use std::path::Path;

use anyhow::{Result, anyhow, bail};
use ironcalc_base::Model;
use ironcalc_base::types::{
    Alignment, Border, BorderItem, BorderStyle, CellProtection, Col, Color, Comment, Fill, Font,
    FontScheme, HorizontalAlignment, Link, MergedCell, NumFmt, Row, SheetState, Style,
    VerticalAlignment,
};

use ironcalc_base::cf_types::{CfRuleInput, ValueOperator};
use ironcalc_base::types::{Dxf, DxfFont};

use crate::print::{HeaderFooter, Margins, PageSetup, Paper, PrintOptions, SheetPrint};
use crate::validation::{ErrorStyle, Message, Validation, ValidationOperator, ValidationType};
use crate::{CellRange, MAX_COLS, MAX_ROWS};

mod cf12;

/// Days between the 1900 and 1904 date systems' day zero.
pub(crate) const DAYS_1904: f64 = 1462.0;

/// A record and its `CONTINUE`s, joined.
pub(crate) struct Records<'a> {
    data: &'a [u8],
    pos: usize,
    buf: Vec<u8>,
    /// Where each `CONTINUE` payload starts in the last joined record (empty
    /// for a record without continuations).
    bounds: Vec<usize>,
}

const CONTINUE: u16 = 0x003C;

impl<'a> Records<'a> {
    pub(crate) fn at(data: &'a [u8], pos: usize) -> Self {
        Self {
            data,
            pos,
            buf: Vec::new(),
            bounds: Vec::new(),
        }
    }

    /// See [`Self::bounds`].
    fn bounds(&self) -> &[usize] {
        &self.bounds
    }

    fn header(&self, pos: usize) -> Option<(u16, usize)> {
        let h = self.data.get(pos..pos + 4)?;
        Some((
            u16::from_le_bytes([h[0], h[1]]),
            u16::from_le_bytes([h[2], h[3]]) as usize,
        ))
    }

    /// The next record: its type and payload (with any `CONTINUE` payloads
    /// appended). `None` at the end of the stream or on a truncated record.
    pub(crate) fn next(&mut self) -> Option<(u16, &[u8])> {
        let (typ, len) = self.header(self.pos)?;
        let start = self.pos + 4;
        let end = start.checked_add(len)?;
        if end > self.data.len() {
            return None;
        }
        self.pos = end;
        self.bounds.clear();
        // Most records have no continuation: borrow them.
        if !matches!(self.header(self.pos), Some((CONTINUE, _))) {
            return Some((typ, &self.data[start..end]));
        }
        self.buf.clear();
        self.buf.extend_from_slice(&self.data[start..end]);
        while let Some((CONTINUE, len)) = self.header(self.pos) {
            let s = self.pos + 4;
            let e = s + len;
            let Some(part) = self.data.get(s..e) else {
                break;
            };
            self.bounds.push(self.buf.len());
            self.buf.extend_from_slice(part);
            self.pos = e;
        }
        Some((typ, &self.buf))
    }
}

thread_local! {
    /// While an Excel 5/95 (BIFF5) file is read: the code page its strings
    /// are in. Its strings are a count and bytes in that code page, without
    /// BIFF8's flags byte; [`Bytes::chars`] reads them so.
    static BIFF5: std::cell::Cell<Option<&'static encoding_rs::Encoding>> =
        const { std::cell::Cell::new(None) };
}

/// Sets [`BIFF5`] until dropped.
struct Biff5Guard;

impl Biff5Guard {
    fn set(enc: Option<&'static encoding_rs::Encoding>) -> Self {
        BIFF5.with(|b| b.set(enc));
        Biff5Guard
    }
}

impl Drop for Biff5Guard {
    fn drop(&mut self) {
        BIFF5.with(|b| b.set(None));
    }
}

/// Whether a BIFF5 file is being read.
fn biff5() -> bool {
    BIFF5.with(|b| b.get().is_some())
}

/// The encoding of a `CODEPAGE` value (Windows code pages; others as
/// Windows-1252).
fn code_page(cp: u16) -> &'static encoding_rs::Encoding {
    use encoding_rs::*;
    match cp {
        874 => WINDOWS_874,
        932 => SHIFT_JIS,
        936 => GBK,
        949 => EUC_KR,
        950 => BIG5,
        1200 => UTF_16LE,
        1250 => WINDOWS_1250,
        1251 => WINDOWS_1251,
        1253 => WINDOWS_1253,
        1254 => WINDOWS_1254,
        1255 => WINDOWS_1255,
        1256 => WINDOWS_1256,
        1257 => WINDOWS_1257,
        1258 => WINDOWS_1258,
        10000 | 32768 => MACINTOSH,
        _ => WINDOWS_1252,
    }
}

/// Little-endian reads that return `None` past the end.
pub(crate) struct Bytes<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bytes<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    pub(crate) fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.data.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }
    pub(crate) fn skip(&mut self, n: usize) -> Option<()> {
        self.take(n).map(|_| ())
    }
    pub(crate) fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    pub(crate) fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }
    pub(crate) fn i16(&mut self) -> Option<i16> {
        self.u16().map(|v| v as i16)
    }
    pub(crate) fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub(crate) fn f64(&mut self) -> Option<f64> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Some(f64::from_le_bytes(a))
    }
    /// The characters of an `XLUnicodeString` after its count: a flags byte
    /// (bit 0: two bytes per character; bit 2: extended data; bit 3: rich
    /// text runs), then the characters.
    fn chars(&mut self, cch: usize) -> Option<String> {
        if let Some(enc) = BIFF5.with(|b| b.get()) {
            let raw = self.take(cch)?;
            return Some(enc.decode_without_bom_handling(raw).0.into_owned());
        }
        let flags = self.u8()?;
        let runs = if flags & 0x08 != 0 { self.u16()? } else { 0 };
        let ext = if flags & 0x04 != 0 { self.u32()? } else { 0 };
        let s = if flags & 0x01 != 0 {
            let raw = self.take(cch.checked_mul(2)?)?;
            let units: Vec<u16> = raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            // Compressed: Latin-1 (the high byte of each UTF-16 unit is 0).
            self.take(cch)?.iter().map(|&b| b as char).collect()
        };
        // Rich runs and extended data follow the characters; skip what fits.
        let _ = self.skip(runs as usize * 4);
        let _ = self.skip(ext as usize);
        Some(s)
    }
    /// `XLUnicodeString`: a 16-bit count, then [`Self::chars`].
    pub(crate) fn string16(&mut self) -> Option<String> {
        let cch = self.u16()? as usize;
        self.chars(cch)
    }
    /// `ShortXLUnicodeString`: an 8-bit count, then [`Self::chars`].
    pub(crate) fn string8(&mut self) -> Option<String> {
        let cch = self.u8()? as usize;
        self.chars(cch)
    }
    /// A `HyperlinkString`: a 32-bit count of UTF-16 units including the
    /// terminating null.
    fn hyperlink_string(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        let raw = self.take(n.checked_mul(2)?)?;
        let units: Vec<u16> = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .collect();
        Some(String::from_utf16_lossy(&units))
    }
}

/// The 56 colours of BIFF8's default palette (indices 8-63).
pub(crate) const DEFAULT_PALETTE: [u32; 56] = [
    0x000000, 0xFFFFFF, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00, 0xFF00FF, 0x00FFFF, 0x800000,
    0x008000, 0x000080, 0x808000, 0x800080, 0x008080, 0xC0C0C0, 0x808080, 0x9999FF, 0x993366,
    0xFFFFCC, 0xCCFFFF, 0x660066, 0xFF8080, 0x0066CC, 0xCCCCFF, 0x000080, 0xFF00FF, 0xFFFF00,
    0x00FFFF, 0x800080, 0x800000, 0x008080, 0x0000FF, 0x00CCFF, 0xCCFFFF, 0xCCFFCC, 0xFFFF99,
    0x99CCFF, 0xFF99CC, 0xCC99FF, 0xFFCC99, 0x3366FF, 0x33CCCC, 0x99CC00, 0xFFCC00, 0xFF9900,
    0xFF6600, 0x666699, 0x969696, 0x003366, 0x339966, 0x003300, 0x333300, 0x993300, 0x993366,
    0x333399, 0x333333,
];

/// The colours of indices 0-7 (fixed, whatever the palette).
pub(crate) const FIXED_COLOURS: [u32; 8] = [
    0x000000, 0xFFFFFF, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00, 0xFF00FF, 0x00FFFF,
];

struct Palette(Vec<u32>);

impl Palette {
    /// The RGB of colour index `icv`; `None` for the system colours
    /// (64 window text, 65 window background, 0x7FFF automatic).
    fn rgb(&self, icv: u16) -> Option<u32> {
        match icv {
            0..=7 => Some(FIXED_COLOURS[icv as usize]),
            8..=63 => self.0.get(icv as usize - 8).copied(),
            _ => None,
        }
    }
    fn color(&self, icv: u16) -> Color {
        self.rgb(icv).map_or(Color::None, rgb_color)
    }
}

pub(crate) fn rgb_color(rgb: u32) -> Color {
    Color::Rgb(format!("#{rgb:06X}"))
}

#[derive(Clone)]
struct XlsFont {
    height_twips: u16,
    italic: bool,
    strike: bool,
    icv: u16,
    bold: bool,
    underline: bool,
    family: u8,
    name: String,
}

/// The raw fields of a cell `XF` record.
#[derive(Clone, Copy, Default)]
struct Xf {
    ifnt: u16,
    ifmt: u16,
    locked: bool,
    hidden: bool,
    quote_prefix: bool,
    halign: u8,
    wrap: bool,
    valign: u8,
    rotation: u8,
    indent: u8,
    shrink: bool,
    border: [(u8, u16); 4], // left, right, top, bottom: (style, colour)
    diag: (u8, u16),
    diag_down: bool,
    diag_up: bool,
    pattern: u8,
    fore: u16,
    back: u16,
}

impl Xf {
    fn parse(d: &[u8]) -> Option<Self> {
        let mut b = Bytes::new(d);
        let ifnt = b.u16()?;
        let ifmt = b.u16()?;
        let flags = b.u16()?;
        let align = b.u8()?;
        let rotation = b.u8()?;
        let indent = b.u8()?;
        let _used = b.u8()?;
        let b1 = b.u32()?;
        let b2 = b.u32()?;
        let fill = b.u16()?;
        Some(Self {
            ifnt,
            ifmt,
            locked: flags & 0x1 != 0,
            hidden: flags & 0x2 != 0,
            quote_prefix: flags & 0x8 != 0,
            halign: align & 0x7,
            wrap: align & 0x8 != 0,
            valign: (align >> 4) & 0x7,
            rotation,
            indent: indent & 0xF,
            shrink: indent & 0x10 != 0,
            border: [
                ((b1 & 0xF) as u8, ((b1 >> 16) & 0x7F) as u16),
                (((b1 >> 4) & 0xF) as u8, ((b1 >> 23) & 0x7F) as u16),
                (((b1 >> 8) & 0xF) as u8, (b2 & 0x7F) as u16),
                (((b1 >> 12) & 0xF) as u8, ((b2 >> 7) & 0x7F) as u16),
            ],
            diag: (((b2 >> 21) & 0xF) as u8, ((b2 >> 14) & 0x7F) as u16),
            diag_down: b1 & (1 << 30) != 0,
            diag_up: b1 & (1 << 31) != 0,
            pattern: ((b2 >> 26) & 0x3F) as u8,
            fore: fill & 0x7F,
            back: (fill >> 7) & 0x7F,
        })
    }
}

impl Xf {
    /// An Excel 5/95 `XF` (16 bytes): fonts, formats and alignment as in
    /// BIFF8, an orientation instead of a rotation, and the fill and the
    /// borders packed in two words (3-bit line styles).
    fn parse5(d: &[u8]) -> Option<Self> {
        let mut b = Bytes::new(d);
        let ifnt = b.u16()?;
        let ifmt = b.u16()?;
        let flags = b.u16()?;
        let align = b.u8()?;
        let ori = b.u8()?;
        let b1 = b.u32()?;
        let b2 = b.u32()?;
        Some(Self {
            ifnt,
            ifmt,
            locked: flags & 0x1 != 0,
            hidden: flags & 0x2 != 0,
            quote_prefix: false,
            halign: align & 0x7,
            wrap: align & 0x8 != 0,
            valign: (align >> 4) & 0x7,
            // Stacked, 90° up, 90° down.
            rotation: match ori & 0x3 {
                1 => 255,
                2 => 90,
                3 => 180,
                _ => 0,
            },
            indent: 0,
            shrink: false,
            // Left, right, top, bottom: (style, colour).
            border: [
                (((b2 >> 3) & 0x7) as u8, ((b2 >> 16) & 0x7F) as u16),
                (((b2 >> 6) & 0x7) as u8, ((b2 >> 23) & 0x7F) as u16),
                ((b2 & 0x7) as u8, ((b2 >> 9) & 0x7F) as u16),
                (((b1 >> 22) & 0x7) as u8, ((b1 >> 25) & 0x7F) as u16),
            ],
            diag: (0, 0),
            diag_down: false,
            diag_up: false,
            pattern: ((b1 >> 16) & 0x3F) as u8,
            fore: (b1 & 0x7F) as u16,
            back: ((b1 >> 7) & 0x7F) as u16,
        })
    }
}

/// A defined name as the `LBL` record has it (calamine decodes the formula).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Label {
    /// The name, or for built-in names Excel's name (`Print_Area`).
    pub name: String,
    pub builtin: Option<u8>,
    /// 0-based sheet the name is local to.
    pub sheet: Option<u32>,
    pub hidden: bool,
}

/// What a sheet substream says about printing, before it becomes a
/// [`SheetPrint`].
#[derive(Default)]
struct PrintRaw {
    setup: Option<(u16, u16, i16, u16, u16, u16)>,
    margins: [Option<f64>; 4],
    hcenter: bool,
    vcenter: bool,
    grid: bool,
    headings: bool,
    fit_to_page: bool,
    header: String,
    footer: String,
    row_breaks: Vec<u32>,
    col_breaks: Vec<u32>,
}

impl PrintRaw {
    fn into_print(self) -> SheetPrint {
        let mut setup = PageSetup::default();
        if let Some((paper, scale, first, fit_w, fit_h, flags)) = self.setup {
            // fNoPls: paper, scale and orientation were never set.
            if flags & 0x4 == 0 {
                if let Some(p) = Paper::from_code(paper as u32) {
                    setup.paper = p;
                }
                if (10..=400).contains(&scale) {
                    setup.scale = scale as u32;
                }
                // fNoOrient leaves the default (portrait).
                setup.landscape = flags & 0x40 == 0 && flags & 0x2 == 0;
            }
            setup.over_then_down = flags & 0x1 != 0;
            setup.black_and_white = flags & 0x8 != 0;
            if flags & 0x80 != 0 {
                setup.first_page_number = Some(first as i32);
            }
            if self.fit_to_page {
                setup.fit = Some((fit_w as u32, fit_h as u32));
            }
        } else if self.fit_to_page {
            setup.fit = Some((1, 1));
        }
        let d = Margins::default();
        let m = self.margins;
        // BIFF's defaults are Excel 97's (0.75" left/right, 1" top/bottom),
        // used when a sheet has no margin records.
        let margins = Margins {
            left: m[0].unwrap_or(0.75),
            right: m[1].unwrap_or(0.75),
            top: m[2].unwrap_or(1.0),
            bottom: m[3].unwrap_or(1.0),
            header: self.setup.map_or(d.header, |_| d.header),
            footer: d.footer,
        };
        SheetPrint {
            setup,
            margins,
            options: PrintOptions {
                grid_lines: self.grid,
                headings: self.headings,
                horizontal_centered: self.hcenter,
                vertical_centered: self.vcenter,
            },
            header_footer: HeaderFooter {
                odd_header: self.header,
                odd_footer: self.footer,
                scale_with_doc: true,
                align_with_margins: true,
                ..HeaderFooter::default()
            },
            row_breaks: self.row_breaks,
            col_breaks: self.col_breaks,
            print_area: Vec::new(),
            title_rows: None,
            title_cols: None,
            hf_pictures: Vec::new(),
        }
    }
}

/// The workbook-level part of the stream.
pub(crate) struct Globals {
    pub date1904: bool,
    /// An Excel 5/95 file (BIFF5): the code page of its strings.
    biff5: Option<&'static encoding_rs::Encoding>,
    /// Sheets in tab order: (name, stream offset, visibility, type: 0
    /// worksheet, 1 macro sheet, 2 chart sheet...).
    pub(crate) sheets: Vec<(String, usize, SheetState, u8)>,
    pub labels: Vec<Label>,
    /// The tab Excel shows when the file opens.
    pub active: u32,
    fonts: Vec<XlsFont>,
    formats: Vec<NumFmt>,
    xfs: Vec<Xf>,
    palette: Palette,
    /// `EXTERNSHEET` entries resolved to this workbook's sheet indices
    /// (`None`: another workbook's), for 3-D references in formulas.
    pub(crate) xti: Vec<Option<(u16, u16)>>,
    /// The OfficeArt drawing group (`MSODRAWINGGROUP`): the pictures every
    /// sheet's drawing shares.
    drawing_group: Vec<u8>,
}

/// The formatting of an `.xls` file, read before calamine's values.
pub(crate) struct XlsFormat {
    stream: Vec<u8>,
    pub globals: Globals,
}

impl XlsFormat {
    /// Read the `Workbook` stream and its globals. Fails on files that
    /// aren't BIFF8 (Excel 97 and later); the caller then imports values only.
    pub(crate) fn read(path: &Path) -> Result<Self> {
        let mut file = cfb::open(path).map_err(|e| anyhow!("{e}"))?;
        let mut stream = Vec::new();
        // Excel 5/95 files call the stream "Book".
        let mut found = file.open_stream("/Workbook");
        if found.is_err() {
            found = file.open_stream("/Book");
        }
        found
            .map_err(|e| anyhow!("no Workbook stream: {e}"))?
            .read_to_end(&mut stream)?;
        let globals = read_globals(&stream)?;
        Ok(Self { stream, globals })
    }

    /// The 0-based sheet index in calamine's list (every `BOUNDSHEET`) of the
    /// sheet called `name`.
    fn sheet_named(&self, name: &str) -> Option<usize> {
        self.globals.sheets.iter().position(|s| s.0 == name)
    }

    /// Apply the formatting to `model`, whose sheets calamine filled in
    /// `names` order: cell formats, layout, notes and conditional formats
    /// go into the model; print settings and data validation come back.
    pub(crate) fn apply(&self, model: &mut Model<'_>, names: &[String]) -> Applied {
        let g = &self.globals;
        let _biff5 = Biff5Guard::set(g.biff5);
        let mut styles = StyleMap::new(g, model);
        let mut prints = Vec::with_capacity(names.len());
        let mut cleared = false;
        let mut validations = Vec::new();
        let mut skipped = Skipped::default();
        let mut drawings = crate::xls_drawing::XlsDrawings::default();
        let mut art_skipped = crate::xls_drawing::Skipped::default();
        if !g.drawing_group.is_empty() {
            drawings.blips = crate::xls_drawing::blip_store(&g.drawing_group);
        }
        let sheet_names: Vec<String> = g.sheets.iter().map(|s| s.0.clone()).collect();
        let label_names: Vec<String> = g
            .labels
            .iter()
            .map(|l| {
                if l.builtin.is_some() {
                    String::new()
                } else {
                    l.name.clone()
                }
            })
            .collect();
        for (sheet, name) in names.iter().enumerate() {
            let Some(ix) = self.sheet_named(name) else {
                prints.push(SheetPrint::default());
                continue;
            };
            let (_, offset, state, kind) = &g.sheets[ix];
            if let Ok(ws) = model.workbook.worksheet_mut(sheet as u32) {
                ws.state = state.clone();
            }
            if *kind != 0 {
                prints.push(SheetPrint::default());
                continue;
            }
            let ctx = |base: (u32, u32)| crate::xls_formula::Context {
                base,
                sheets: &sheet_names,
                xti: &g.xti,
                names: &label_names,
                formats: &g.formats,
            };
            let mut rules = SheetRules::default();
            let print = read_sheet(
                &self.stream,
                *offset,
                sheet as u32,
                model,
                &mut styles,
                &mut cleared,
                &mut rules,
            );
            prints.push(print.into_print());
            if !rules.objects.is_empty() {
                let objects = crate::xls_drawing::sheet_objects(
                    &rules.drawing,
                    std::mem::take(&mut rules.objects),
                    &rules.geometry,
                    &drawings.blips,
                    &ctx((0, 0)),
                    &mut art_skipped,
                );
                if !objects.is_empty() {
                    drawings.sheets.push((name.clone(), objects));
                }
            }
            let list: Vec<Validation> = rules
                .validations
                .into_iter()
                .filter_map(|dv| {
                    let v = dv.convert(&ctx);
                    if v.is_none() {
                        skipped.validations += 1;
                    }
                    v
                })
                .collect();
            if !list.is_empty() {
                validations.push((sheet as u32, list));
            }
            cf12::apply_cfex(&mut rules.cf, &rules.cfex);
            for (ranges, d) in std::mem::take(&mut rules.cf12_raw) {
                match cf12::parse_cf12_with(&d, g) {
                    Some((own, cf)) => rules.cf.push((ranges.unwrap_or(own), cf)),
                    None => rules.cf12 += 1,
                }
            }
            skipped.cf += rules.cf12;
            // Excel 2007 rules carry their priority; older ones keep file
            // order.
            rules.cf.sort_by_key(|(_, cf)| cf.ext.priority.unwrap_or(0));
            let mut inputs = Vec::new();
            for (ranges, cf) in rules.cf {
                let Some(input) = cf.convert(&ranges, &ctx, g, model) else {
                    skipped.cf += 1;
                    continue;
                };
                inputs.push((crate::cf_edit::sqref_of(&ranges), input));
            }
            // The first rule in the file comes first in priority (Excel's
            // priority 1); each rule added goes to the top: add the last
            // one first.
            for (sqref, input) in inputs.into_iter().rev() {
                if model
                    .add_conditional_formatting(sheet as u32, &sqref, input)
                    .is_err()
                {
                    skipped.cf += 1;
                }
            }
        }
        // Chart sheets (the model has no sheet for them).
        let ctx = crate::xls_formula::Context {
            base: (0, 0),
            sheets: &sheet_names,
            xti: &g.xti,
            names: &label_names,
            formats: &g.formats,
        };
        for (name, offset, state, kind) in &g.sheets {
            drawings.order.push((name.clone(), *kind == 2));
            if *kind != 2 {
                continue;
            }
            let recs = crate::xls_drawing::substream(&self.stream, *offset);
            match crate::xls_drawing::parse_chart(&recs, &ctx) {
                Ok(chart) => {
                    art_skipped.flat += usize::from(chart.three_d);
                    drawings.chart_sheets.push(crate::xls_drawing::ChartSheet {
                        name: name.clone(),
                        hidden: *state != SheetState::Visible,
                        chart,
                    });
                }
                Err(_) => art_skipped.charts += 1,
            }
        }
        if g.date1904 {
            drawings.shift_dates(DAYS_1904);
        }
        let mut notes = Vec::new();
        if skipped.validations > 0 {
            notes.push(crate::ImportNote::ValidationsSkipped(skipped.validations));
        }
        if skipped.cf > 0 {
            notes.push(crate::ImportNote::ConditionalFormatsSkipped(skipped.cf));
        }
        notes.extend(art_skipped.notes());
        Applied {
            patterns: std::mem::take(&mut styles.patterns),
            prints,
            cleared,
            validations,
            notes,
            drawings,
        }
    }
}

/// What [`XlsFormat::apply`] brings back.
pub(crate) struct Applied {
    /// Patterned fills by the model's fill index (see [`crate::FillPattern`]).
    pub patterns: Vec<Option<crate::FillPattern>>,
    /// Each sheet's print settings, by model sheet.
    pub prints: Vec<SheetPrint>,
    /// Values changed (covered cells of merges were cleared).
    pub cleared: bool,
    /// Data validation, by model sheet.
    pub validations: Vec<(u32, Vec<Validation>)>,
    /// What was left out, for the import notes.
    pub notes: Vec<crate::ImportNote>,
    /// Pictures and charts, to show and save (`xls_drawing`).
    pub drawings: crate::xls_drawing::XlsDrawings,
}

#[derive(Default)]
struct Skipped {
    validations: usize,
    cf: usize,
}

/// A sheet's data validation and conditional formatting as read, their
/// formulas still tokens.
#[derive(Default)]
struct SheetRules {
    validations: Vec<RawDv>,
    /// Each `CONDFMT`'s ranges with one of its `CF` rules.
    cf: Vec<(Vec<CellRange>, RawCf)>,
    /// `CF12` rules that could not be read.
    cf12: usize,
    /// `CFEX` records: Excel 2007 templates of `CF` rules, by `CONDFMT` id
    /// and rule index.
    cfex: Vec<(u16, u16, cf12::Ext)>,
    /// `CF12` records with their `CONDFMT12`'s ranges, read once the
    /// palette is known.
    cf12_raw: Vec<(Option<Vec<CellRange>>, Vec<u8>)>,
    /// The sheet's OfficeArt drawing (its `MSODRAWING` records joined).
    drawing: Vec<u8>,
    /// Its `OBJ`s in order, each with the chart substream after it.
    objects: Vec<crate::xls_drawing::RawObj>,
    /// Column widths and row heights in Excel's pixels, for anchors.
    geometry: crate::xls_drawing::Geometry,
}

/// A `DV` record (MS-XLS 2.4.107), or an `.xlsb` `BrtDVal` (`b12`: its
/// formulas are BIFF12 tokens).
pub(crate) struct RawDv {
    pub flags: u32,
    pub prompt_title: String,
    pub error_title: String,
    pub prompt: String,
    pub error: String,
    pub formula1: Vec<u8>,
    pub formula2: Vec<u8>,
    pub ranges: Vec<CellRange>,
    pub b12: bool,
}

/// A `DV` string: a lone NUL means none.
fn dv_string(b: &mut Bytes) -> Option<String> {
    let s = b.string16()?;
    Some(if s == "\0" { String::new() } else { s })
}

/// A `SqRefU`: a count, then `Ref8U` ranges (first row, last row, first
/// column, last column).
fn sqref(b: &mut Bytes) -> Option<Vec<CellRange>> {
    let n = b.u16()?;
    let mut out = Vec::new();
    for _ in 0..n {
        let (r1, r2, c1, c2) = (b.u16()?, b.u16()?, b.u16()?, b.u16()?);
        if r2 < r1 || c2 < c1 || u32::from(c1) >= MAX_COLS {
            continue;
        }
        out.push(CellRange::from_corners(
            (r1 as u32, c1 as u32),
            (r2 as u32, (c2 as u32).min(MAX_COLS - 1)),
        ));
    }
    Some(out)
}

impl RawDv {
    fn parse(d: &[u8]) -> Option<Self> {
        let mut b = Bytes::new(d);
        let flags = b.u32()?;
        let prompt_title = dv_string(&mut b)?;
        let error_title = dv_string(&mut b)?;
        let prompt = dv_string(&mut b)?;
        let error = dv_string(&mut b)?;
        let cce = b.u16()? as usize;
        b.skip(2)?;
        let formula1 = b.take(cce)?.to_vec();
        let cce = b.u16()? as usize;
        b.skip(2)?;
        let formula2 = b.take(cce)?.to_vec();
        let ranges = sqref(&mut b)?;
        Some(Self {
            flags,
            prompt_title,
            error_title,
            prompt,
            error,
            formula1,
            formula2,
            ranges,
            b12: false,
        })
    }

    pub(crate) fn convert<'c>(
        self,
        ctx: &dyn Fn((u32, u32)) -> crate::xls_formula::Context<'c>,
    ) -> Option<Validation> {
        let first = *self.ranges.first()?;
        let ctx = ctx((first.top, first.left));
        let f = self.flags;
        let kind = match f & 0xF {
            0 => ValidationType::Any,
            1 => ValidationType::Whole,
            2 => ValidationType::Decimal,
            3 => ValidationType::List,
            4 => ValidationType::Date,
            5 => ValidationType::Time,
            6 => ValidationType::TextLength,
            7 => ValidationType::Custom,
            _ => return None,
        };
        let style = match (f >> 4) & 0x7 {
            1 => ErrorStyle::Warning,
            2 => ErrorStyle::Information,
            _ => ErrorStyle::Stop,
        };
        let operator = match (f >> 20) & 0xF {
            1 => ValidationOperator::NotBetween,
            2 => ValidationOperator::Equal,
            3 => ValidationOperator::NotEqual,
            4 => ValidationOperator::GreaterThan,
            5 => ValidationOperator::LessThan,
            6 => ValidationOperator::GreaterThanOrEqual,
            7 => ValidationOperator::LessThanOrEqual,
            _ => ValidationOperator::Between,
        };
        let decode = |rgce: &[u8]| -> Option<Option<String>> {
            if rgce.is_empty() {
                return Some(None);
            }
            if self.b12 {
                crate::xls_formula::decompile12(rgce, &ctx).map(Some)
            } else {
                crate::xls_formula::decompile(rgce, &ctx).map(Some)
            }
        };
        let formula1 = if kind == ValidationType::List && f & 0x80 != 0 {
            // An explicit list: one string token, items split by NULs.
            decode(&self.formula1)?.map(|s| s.replace('\0', ","))
        } else {
            decode(&self.formula1)?
        };
        let two = matches!(
            operator,
            ValidationOperator::Between | ValidationOperator::NotBetween
        ) && !matches!(
            kind,
            ValidationType::Any | ValidationType::List | ValidationType::Custom
        );
        let formula2 = if two { decode(&self.formula2)? } else { None };
        if kind != ValidationType::Any && formula1.is_none() {
            return None;
        }
        let prompt = Message {
            title: self.prompt_title,
            text: self.prompt,
        };
        Some(Validation {
            ime_mode: None,
            kind,
            operator,
            allow_blank: f & 0x100 != 0,
            dropdown: f & 0x200 == 0,
            prompt: (f & 0x40000 != 0 && !(prompt.title.is_empty() && prompt.text.is_empty()))
                .then_some(prompt),
            error: (f & 0x80000 != 0).then_some((
                style,
                Message {
                    title: self.error_title,
                    text: self.error,
                },
            )),
            formula1,
            formula2,
            ranges: self.ranges,
        })
    }
}

/// A `CF` record (MS-XLS 2.4.42): its type, operator, the differential
/// format and the formulas.
struct RawCf {
    ct: u8,
    cp: u8,
    dxf: XlsDxf,
    rgce1: Vec<u8>,
    rgce2: Vec<u8>,
    /// `(CONDFMT id, index in it)`, for its `CFEX`.
    id: Option<(u16, u16)>,
    /// What Excel 2007 adds (`CF12`, `CFEX`).
    ext: cf12::Ext,
}

/// A `DXFN` as read: palette indices still to resolve.
#[derive(Default)]
struct XlsDxf {
    num: Option<(u16, Option<String>)>,
    bold: Option<bool>,
    italic: Option<bool>,
    strike: Option<bool>,
    underline: Option<bool>,
    height: Option<u32>,
    font_icv: Option<u16>,
    /// left, right, top, bottom: (style, colour) when set.
    border: [Option<(u8, u16)>; 4],
    /// (pattern, fore, back), each when set.
    pattern: (Option<u8>, Option<u16>, Option<u16>),
}

impl XlsDxf {
    /// A `DXFN` (MS-XLS 2.5.39), consuming exactly its bytes.
    fn parse(b: &mut Bytes) -> Option<Self> {
        let flags = b.u32()?;
        let flags2 = b.u16()?;
        let bit = |n: u32| flags & (1 << n) != 0;
        let mut x = XlsDxf::default();
        if bit(25) {
            if flags2 & 0x1 != 0 {
                let cb = b.u16()? as usize;
                let body = b.take(cb.checked_sub(2)?)?;
                let code = Bytes::new(body).string16()?;
                x.num = Some((0, Some(code)));
            } else {
                b.skip(1)?;
                x.num = Some((b.u8()? as u16, None));
            }
            if bit(19) {
                x.num = None;
            }
        }
        if bit(26) {
            let f = b.take(118)?;
            let mut fb = Bytes::new(&f[64..]);
            let height = fb.u32()?;
            let ts = fb.u32()?;
            let bls = fb.u16()?;
            let _sss = fb.u16()?;
            let uls = fb.u8()?;
            fb.skip(3)?;
            let icv = fb.u32()?;
            fb.skip(4)?;
            let ts_ninch = fb.u32()?;
            let _sss_ninch = fb.u32()?;
            let uls_ninch = fb.u32()?;
            let bls_ninch = fb.u32()?;
            if height != 0xFFFF_FFFF && height > 0 && height < 8200 {
                x.height = Some(height);
            }
            if ts_ninch & 0x2 == 0 {
                x.italic = Some(ts & 0x2 != 0);
            }
            if ts_ninch & 0x80 == 0 {
                x.strike = Some(ts & 0x80 != 0);
            }
            if bls_ninch == 0 && bls != 0xFFFF {
                x.bold = Some(bls >= 700);
            }
            if uls_ninch == 0 && uls != 0xFF {
                x.underline = Some(uls != 0);
            }
            if icv != 0xFFFF_FFFF && icv < 0x7FFF {
                x.font_icv = Some(icv as u16);
            }
        }
        if bit(27) {
            b.skip(8)?;
        }
        if bit(28) {
            let b1 = b.u32()?;
            let b2 = b.u32()?;
            let sides = [
                ((b1 & 0xF) as u8, ((b1 >> 16) & 0x7F) as u16),
                (((b1 >> 4) & 0xF) as u8, ((b1 >> 23) & 0x7F) as u16),
                (((b1 >> 8) & 0xF) as u8, (b2 & 0x7F) as u16),
                (((b1 >> 12) & 0xF) as u8, ((b2 >> 7) & 0x7F) as u16),
            ];
            for (i, side) in sides.into_iter().enumerate() {
                if !bit(10 + i as u32) {
                    x.border[i] = Some(side);
                }
            }
        }
        if bit(29) {
            let p = b.u16()?;
            let c = b.u16()?;
            x.pattern = (
                (!bit(16)).then_some((p >> 10) as u8 & 0x3F),
                (!bit(17)).then_some(c & 0x7F),
                (!bit(18)).then_some((c >> 7) & 0x7F),
            );
        }
        if bit(30) {
            b.skip(2)?;
        }
        Some(x)
    }

    fn convert(&self, g: &Globals) -> Dxf {
        let p = &g.palette;
        let font = DxfFont {
            strike: self.strike,
            u: self.underline,
            b: self.bold,
            i: self.italic,
            sz: self
                .height
                .map(|h| ((h as f64 / 20.0).round() as i32).clamp(1, 409)),
            color: self.font_icv.map_or(Color::None, |icv| p.color(icv)),
        };
        let font = (font != DxfFont::default()).then_some(font);
        // A conditional fill shows its background colour when solid (as an
        // `.xlsx` dxf's `bgColor`).
        let fill = match self.pattern {
            (Some(0), _, _) => None,
            (pat, fore, back) if fore.is_some() || back.is_some() => {
                let color = match pat.unwrap_or(1) {
                    1 => back.or(fore).map(|icv| p.color(icv)),
                    pat => Some(fill_color(pat, fore.unwrap_or(64), back.unwrap_or(65), p)),
                };
                color
                    .filter(|c| *c != Color::None)
                    .map(|color| Fill { color })
            }
            _ => None,
        };
        let item = |side: Option<(u8, u16)>| {
            let (style, icv) = side?;
            border_style(style).map(|style| BorderItem {
                style,
                color: p.color(icv),
            })
        };
        let border = Border {
            left: item(self.border[0]),
            right: item(self.border[1]),
            top: item(self.border[2]),
            bottom: item(self.border[3]),
            diagonal: None,
            diagonal_down: false,
            diagonal_up: false,
        };
        let border = (border != Border::default()).then_some(border);
        let num_fmt = self.num.as_ref().map(|(id, code)| NumFmt {
            num_fmt_id: *id as i32,
            format_code: code.clone().unwrap_or_else(|| {
                ironcalc_base::number_format::get_num_fmt(*id as i32, &g.formats)
            }),
        });
        Dxf {
            font,
            fill,
            border,
            num_fmt,
            alignment: None,
        }
    }
}

impl RawCf {
    fn parse(d: &[u8]) -> Option<Self> {
        let mut b = Bytes::new(d);
        let ct = b.u8()?;
        let cp = b.u8()?;
        let cce1 = b.u16()? as usize;
        let cce2 = b.u16()? as usize;
        let dxf = XlsDxf::parse(&mut b)?;
        let rgce1 = b.take(cce1)?.to_vec();
        let rgce2 = b.take(cce2)?.to_vec();
        Some(Self {
            ct,
            cp,
            dxf,
            rgce1,
            rgce2,
            id: None,
            ext: cf12::Ext::default(),
        })
    }

    fn convert<'c>(
        &self,
        ranges: &[CellRange],
        ctx: &dyn Fn((u32, u32)) -> crate::xls_formula::Context<'c>,
        g: &Globals,
        _model: &Model<'_>,
    ) -> Option<CfRuleInput> {
        let first = *ranges.first()?;
        let ctx = ctx((first.top, first.left));
        cf12::convert(self, &ctx, g)
    }

    /// A cell value rule (`ct` 1).
    fn cell_is(&self, ctx: &crate::xls_formula::Context, format: Dxf) -> Option<CfRuleInput> {
        let f1 = crate::xls_formula::decompile(&self.rgce1, ctx)?;
        let operator = match self.cp {
            1 => ValueOperator::Between,
            2 => ValueOperator::NotBetween,
            3 => ValueOperator::Equal,
            4 => ValueOperator::NotEqual,
            5 => ValueOperator::GreaterThan,
            6 => ValueOperator::LessThan,
            7 => ValueOperator::GreaterThanOrEqual,
            8 => ValueOperator::LessThanOrEqual,
            _ => return None,
        };
        let formula2 = if matches!(operator, ValueOperator::Between | ValueOperator::NotBetween) {
            Some(crate::xls_formula::decompile(&self.rgce2, ctx)?)
        } else {
            None
        };
        Some(CfRuleInput::CellIs {
            operator,
            formula: f1,
            formula2,
            format,
            stop_if_true: self.ext.stop,
        })
    }
}

fn read_globals(stream: &[u8]) -> Result<Globals> {
    let mut rec = Records::at(stream, 0);
    let five = match rec.next() {
        Some((0x0809, d)) if d.len() >= 4 => match u16::from_le_bytes([d[0], d[1]]) {
            0x0600 => false,
            // Excel 5 and 95.
            0x0500 => true,
            version => bail!("not BIFF5 or BIFF8 (version {version:#x})"),
        },
        _ => bail!("no BOF record"),
    };
    let guard = Biff5Guard::set(five.then_some(encoding_rs::WINDOWS_1252));
    let mut g = Globals {
        date1904: false,
        biff5: None,
        sheets: Vec::new(),
        labels: Vec::new(),
        active: 0,
        fonts: Vec::new(),
        formats: Vec::new(),
        xfs: Vec::new(),
        palette: Palette(DEFAULT_PALETTE.to_vec()),
        xti: Vec::new(),
        drawing_group: Vec::new(),
    };
    // Which SUPBOOK is this workbook (its marker is 0x0401).
    let mut supbooks: Vec<bool> = Vec::new();
    let mut raw_xti: Vec<(u16, u16, u16)> = Vec::new();
    while let Some((typ, d)) = rec.next() {
        let mut b = Bytes::new(d);
        match typ {
            0x000A => break, // EOF
            0x002F => bail!("the file is encrypted"),
            0x0022 => g.date1904 = b.u16() == Some(1),
            0x0042 if five => {
                if let Some(cp) = b.u16() {
                    BIFF5.with(|b| b.set(Some(code_page(cp))));
                }
            }
            0x003D => {
                // WINDOW1: itabCur at offset 10.
                if b.skip(10).is_some()
                    && let Some(tab) = b.u16()
                {
                    g.active = tab as u32;
                }
            }
            0x0031 => {
                let font = (|| {
                    let height_twips = b.u16()?;
                    let flags = b.u16()?;
                    let icv = b.u16()?;
                    let weight = b.u16()?;
                    let _script = b.u16()?;
                    let underline = b.u8()?;
                    let family = b.u8()?;
                    let _charset = b.u8()?;
                    b.skip(1)?;
                    let name = b.string8()?;
                    Some(XlsFont {
                        height_twips,
                        italic: flags & 0x2 != 0,
                        strike: flags & 0x8 != 0,
                        icv,
                        bold: weight >= 700,
                        underline: underline != 0,
                        family,
                        name,
                    })
                })();
                // Keep indices aligned even when a record is malformed.
                g.fonts.push(font.unwrap_or_else(|| XlsFont {
                    height_twips: 200,
                    italic: false,
                    strike: false,
                    icv: 0x7FFF,
                    bold: false,
                    underline: false,
                    family: 2,
                    name: "Arial".into(),
                }));
            }
            0x041E => {
                let id = b.u16();
                // BIFF5: an 8-bit count.
                let code = if five { b.string8() } else { b.string16() };
                if let (Some(id), Some(code)) = (id, code) {
                    g.formats.push(NumFmt {
                        num_fmt_id: id as i32,
                        format_code: code,
                    });
                }
            }
            0x00E0 if five => g.xfs.push(Xf::parse5(d).unwrap_or_default()),
            0x00E0 => g.xfs.push(Xf::parse(d).unwrap_or_default()),
            0x0092 => {
                if let Some(n) = b.u16() {
                    for slot in g.palette.0.iter_mut().take(n as usize) {
                        let Some(c) = b.take(4) else { break };
                        *slot = (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32;
                    }
                }
            }
            0x0085 => {
                let sheet = (|| {
                    let pos = b.u32()? as usize;
                    let state = match b.u8()? & 0x3 {
                        0 => SheetState::Visible,
                        1 => SheetState::Hidden,
                        _ => SheetState::VeryHidden,
                    };
                    let kind = b.u8()?;
                    let name = b.string8()?;
                    Some((name, pos, state, kind))
                })();
                if let Some(s) = sheet {
                    g.sheets.push(s);
                }
            }
            0x00EB => g.drawing_group.extend_from_slice(d),
            0x01AE => supbooks.push(d.len() == 4 && d[2..4] == [0x01, 0x04]),
            0x0017 => {
                let n = b.u16().unwrap_or(0);
                for _ in 0..n {
                    let (Some(sb), Some(first), Some(last)) = (b.u16(), b.u16(), b.u16()) else {
                        break;
                    };
                    raw_xti.push((sb, first, last));
                }
            }
            0x0018 => {
                g.labels.push(parse_label(d).unwrap_or(Label {
                    name: String::new(),
                    builtin: None,
                    sheet: None,
                    hidden: true,
                }));
            }
            _ => {}
        }
    }
    if g.sheets.is_empty() {
        bail!("no sheets");
    }
    g.biff5 = BIFF5.with(|b| b.get());
    drop(guard);
    g.xti = raw_xti
        .into_iter()
        .map(|(sb, first, last)| {
            let internal = supbooks.get(sb as usize).copied().unwrap_or(false);
            (internal && first <= last && (last as usize) < g.sheets.len()).then_some((first, last))
        })
        .collect();
    Ok(g)
}

/// `LBL` (MS-XLS 2.4.150) up to its name.
fn parse_label(d: &[u8]) -> Option<Label> {
    let mut b = Bytes::new(d);
    let flags = b.u16()?;
    let _key = b.u8()?;
    let cch = b.u8()? as usize;
    let _cce = b.u16()?;
    b.skip(2)?;
    let itab = b.u16()?;
    b.skip(4)?;
    let name = b.chars(cch)?;
    let builtin = (flags & 0x20 != 0).then(|| name.chars().next().map_or(0, |c| c as u8));
    let name = match builtin {
        Some(code) => match code {
            0x00 => "Consolidate_Area",
            0x01 => "Auto_Open",
            0x02 => "Auto_Close",
            0x03 => "Extract",
            0x04 => "Database",
            0x05 => "Criteria",
            0x06 => "Print_Area",
            0x07 => "Print_Titles",
            0x08 => "Recorder",
            0x09 => "Data_Form",
            0x0A => "Auto_Activate",
            0x0B => "Auto_Deactivate",
            0x0C => "Sheet_Title",
            0x0D => "_FilterDatabase",
            _ => "",
        }
        .to_owned(),
        None => name,
    };
    Some(Label {
        name,
        builtin,
        sheet: itab.checked_sub(1).map(u32::from),
        hidden: flags & 0x1 != 0,
    })
}

/// IronCalc style indices for the file's XFs, made when first used.
struct StyleMap<'g> {
    g: &'g Globals,
    /// By XF index; -1 until first used.
    map: Vec<i32>,
    /// Patterned fills, by the model's fill index (the model keeps one
    /// colour per fill: the pattern's colours mixed).
    patterns: Vec<Option<crate::FillPattern>>,
}

impl<'g> StyleMap<'g> {
    /// Also makes the file's default font (font 0, the Normal style's)
    /// the workbook's, so unformatted cells look as in Excel.
    fn new(g: &'g Globals, model: &mut Model<'_>) -> Self {
        if let Some(f) = g.fonts.first() {
            let font = convert_font(f, &g.palette);
            if let Some(slot) = model.workbook.styles.fonts.first_mut() {
                *slot = font;
            }
        }
        Self {
            g,
            map: vec![-1; g.xfs.len()],
            patterns: Vec::new(),
        }
    }

    /// The model style for XF `ixfe`; 0 (the default) for unknown XFs.
    fn get(&mut self, model: &mut Model<'_>, ixfe: u16) -> i32 {
        match self.map.get(ixfe as usize) {
            Some(&s) if s >= 0 => return s,
            Some(_) => {}
            // Not an XF of the file: the default.
            None => return 0,
        }
        let s = match self.g.xfs.get(ixfe as usize) {
            Some(xf) => {
                let style = self.convert(xf);
                let styles = &mut model.workbook.styles;
                match pattern_of(xf, &self.g.palette) {
                    // A pattern: a fill of its own, so that the pattern
                    // can be found from the cell's fill.
                    Some(p) => {
                        let s = styles.create_new_style(&style);
                        styles.fills.push(style.fill.clone());
                        let fill = styles.fills.len() - 1;
                        if let Some(x) = styles.cell_xfs.get_mut(s as usize) {
                            x.fill_id = fill as i32;
                        }
                        if self.patterns.len() <= fill {
                            self.patterns.resize(fill + 1, None);
                        }
                        self.patterns[fill] = Some(p);
                        s
                    }
                    None => styles
                        .get_style_index(&style)
                        .unwrap_or_else(|| styles.create_new_style(&style)),
                }
            }
            None => 0,
        };
        self.map[ixfe as usize] = s;
        s
    }

    fn convert(&self, xf: &Xf) -> Style {
        let g = self.g;
        let p = &g.palette;
        // Font index 4 doesn't exist in BIFF: 5 is the fifth record.
        let fi = if xf.ifnt >= 4 { xf.ifnt - 1 } else { xf.ifnt } as usize;
        let font = g
            .fonts
            .get(fi)
            .or(g.fonts.first())
            .map(|f| convert_font(f, p))
            .unwrap_or_default();
        let num_fmt = ironcalc_base::number_format::get_num_fmt(xf.ifmt as i32, &g.formats);
        let horizontal = match xf.halign {
            1 => HorizontalAlignment::Left,
            2 => HorizontalAlignment::Center,
            3 => HorizontalAlignment::Right,
            4 => HorizontalAlignment::Fill,
            5 => HorizontalAlignment::Justify,
            6 => HorizontalAlignment::CenterContinuous,
            7 => HorizontalAlignment::Distributed,
            _ => HorizontalAlignment::General,
        };
        let vertical = match xf.valign {
            0 => VerticalAlignment::Top,
            1 => VerticalAlignment::Center,
            3 => VerticalAlignment::Justify,
            4 => VerticalAlignment::Distributed,
            _ => VerticalAlignment::Bottom,
        };
        let rotation = match xf.rotation {
            r @ 0..=180 => r as u32,
            255 => 255,
            _ => 0,
        };
        let alignment = Alignment {
            horizontal,
            vertical,
            wrap_text: xf.wrap,
            indent: xf.indent as u32,
            text_rotation: rotation,
            shrink_to_fit: xf.shrink,
        };
        let alignment = (alignment != Alignment::default()).then_some(alignment);
        let item = |(style, icv): (u8, u16)| {
            border_style(style).map(|style| BorderItem {
                style,
                color: p.color(icv),
            })
        };
        let border = Border {
            left: item(xf.border[0]),
            right: item(xf.border[1]),
            top: item(xf.border[2]),
            bottom: item(xf.border[3]),
            diagonal: if xf.diag_down || xf.diag_up {
                item(xf.diag)
            } else {
                None
            },
            diagonal_down: xf.diag_down,
            diagonal_up: xf.diag_up,
        };
        let protection = (!xf.locked || xf.hidden).then_some(CellProtection {
            locked: xf.locked,
            hidden: xf.hidden,
        });
        Style {
            alignment,
            num_fmt,
            fill: Fill {
                color: fill_color(xf.pattern, xf.fore, xf.back, p),
            },
            font,
            border,
            quote_prefix: xf.quote_prefix,
            protection,
        }
    }
}

fn convert_font(f: &XlsFont, p: &Palette) -> Font {
    Font {
        strike: f.strike,
        u: f.underline,
        b: f.bold,
        i: f.italic,
        sz: ((f.height_twips as f64 / 20.0).round() as i32).clamp(1, 409),
        color: match f.icv {
            0x7FFF => Color::None,
            icv => p.color(icv),
        },
        name: f.name.clone(),
        family: if f.family == 0 { 2 } else { f.family as i32 },
        scheme: FontScheme::None,
    }
}

pub(crate) fn border_style(dg: u8) -> Option<BorderStyle> {
    Some(match dg {
        1 => BorderStyle::Thin,
        2 => BorderStyle::Medium,
        3 => BorderStyle::Dashed,
        4 => BorderStyle::Dotted,
        5 => BorderStyle::Thick,
        6 => BorderStyle::Double,
        7 => BorderStyle::Hair,
        8 => BorderStyle::MediumDashed,
        9 => BorderStyle::DashDot,
        10 => BorderStyle::MediumDashDot,
        11 => BorderStyle::DashDotDot,
        12 => BorderStyle::MediumDashDotDot,
        13 => BorderStyle::SlantDashDot,
        _ => return None,
    })
}

/// An XF's fill pattern (`fls` 2 to 18) as Excel names it, with its
/// colours; `None` for no fill and solid fills.
fn pattern_of(xf: &Xf, p: &Palette) -> Option<crate::FillPattern> {
    let kind = pattern_name(u32::from(xf.pattern))?;
    // System colours: 64 is window text (black), 65 the background (white).
    Some(crate::FillPattern {
        kind: kind.to_owned(),
        fg: p.rgb(xf.fore).unwrap_or(0x000000),
        bg: Some(p.rgb(xf.back).unwrap_or(0xFFFFFF)),
    })
}

/// Excel's name of fill pattern `fls` (BIFF and `.xlsb`), for 2 to 18;
/// `None` for no fill (0), solid (1) and unknown codes.
pub(crate) fn pattern_name(fls: u32) -> Option<&'static str> {
    Some(match fls {
        2 => "mediumGray",
        3 => "darkGray",
        4 => "lightGray",
        5 => "darkHorizontal",
        6 => "darkVertical",
        7 => "darkDown",
        8 => "darkUp",
        9 => "darkGrid",
        10 => "darkTrellis",
        11 => "lightHorizontal",
        12 => "lightVertical",
        13 => "lightDown",
        14 => "lightUp",
        15 => "lightGrid",
        16 => "lightTrellis",
        17 => "gray125",
        18 => "gray0625",
        _ => return None,
    })
}

/// A fill as one colour: solid fills keep the foreground colour; patterns
/// mix it into the background by how much of the cell the pattern covers.
fn fill_color(pattern: u8, fore: u16, back: u16, p: &Palette) -> Color {
    let density = match pattern {
        0 => return Color::None,
        1 => return p.color(fore),
        2 => 0.5,
        3 => 0.75,
        4 => 0.25,
        5..=10 => 0.5,
        11..=16 => 0.25,
        17 => 0.125,
        18 => 0.0625,
        _ => 0.5,
    };
    // System colours: 64 is window text (black), 65 the background (white).
    let rgb = |icv: u16, system: u32| p.rgb(icv).unwrap_or(system);
    let (f, b) = (rgb(fore, 0x000000), rgb(back, 0xFFFFFF));
    let mix = |shift: u32| {
        let (f, b) = (((f >> shift) & 0xFF) as f64, ((b >> shift) & 0xFF) as f64);
        ((f * density + b * (1.0 - density)).round() as u32).min(255) << shift
    };
    rgb_color(mix(16) | mix(8) | mix(0))
}

/// The stored width of a default column `cch` digits wide (`DEFCOLWIDTH`),
/// for a default font whose digits are 7 pixels (Arial 10, Calibri 11):
/// the digits plus padding and gridline, rounded up to 8 pixels, as Excel
/// sizes default columns (8 digits: 64 px, width 9.140625).
fn chars_to_width(cch: u16) -> f64 {
    let px = ((cch as f64 * 7.0 + 5.0) / 8.0).ceil() * 8.0;
    (px / 7.0 * 256.0).trunc() / 256.0
}

/// One sheet substream: cell formats go straight into `model`; returns the
/// print settings.
fn read_sheet(
    stream: &[u8],
    offset: usize,
    sheet: u32,
    model: &mut Model<'_>,
    styles: &mut StyleMap<'_>,
    cleared: &mut bool,
    rules: &mut SheetRules,
) -> PrintRaw {
    let mut print = PrintRaw::default();
    let mut rec = Records::at(stream, offset);
    match rec.next() {
        Some((0x0809, _)) => {}
        _ => return print,
    }
    let mut depth = 1;
    let five = biff5();
    let mut cols: Vec<Col> = Vec::new();
    let mut rows: Vec<Row> = Vec::new();
    let mut merges: Vec<MergedCell> = Vec::new();
    let mut links: Vec<((i32, i32), Link)> = Vec::new();
    let mut default_cch: Option<u16> = None;
    let mut default_height: Option<f64> = None;
    let mut window2: Option<u16> = None;
    let mut pane: Option<(u16, u16)> = None;
    let mut tab: Option<u16> = None;
    let mut set = |model: &mut Model<'_>, row: u16, col: u16, ixfe: u16| {
        let s = styles.get(model, ixfe);
        if s == 0 || u32::from(row) >= MAX_ROWS || u32::from(col) >= MAX_COLS {
            return;
        }
        if let Ok(ws) = model.workbook.worksheet_mut(sheet) {
            let _ = ws.set_cell_style(row as i32 + 1, col as i32 + 1, s);
        }
    };
    let mut col_styles: Vec<(usize, u16)> = Vec::new();
    let mut row_styles: Vec<(usize, u16)> = Vec::new();
    // Notes: the text box of the last note `OBJ`, the texts by object id,
    // then `NOTE` records pointing at them.
    let mut note_obj: Option<u16> = None;
    let mut texts: HashMap<u16, String> = HashMap::new();
    let mut notes: Vec<(u16, u16, u16, String)> = Vec::new();
    // The ranges of the last CONDFMT, how many CF records it has left, its
    // id and the index of the next one; the same for CONDFMT12 and CF12.
    let mut condfmt: Option<(Vec<CellRange>, u16, u16, u16)> = None;
    let mut condfmt12: Option<(Vec<CellRange>, u16)> = None;
    // Each CONDFMT's ranges by id, and those of the one a CFEX gave the
    // next CF12 to.
    let mut condfmt_ranges: HashMap<u16, Vec<CellRange>> = HashMap::new();
    let mut cf12_owner: Option<Vec<CellRange>> = None;
    // The records of the embedded chart being read.
    let mut chart: Option<Vec<(u16, Vec<u8>)>> = None;
    while let Some((typ, d)) = rec.next() {
        if typ == 0x0809 {
            // An embedded chart's substream.
            depth += 1;
            if depth == 2 {
                chart = Some(Vec::new());
            }
        } else if typ == 0x000A {
            depth -= 1;
            if depth == 0 {
                break;
            }
        }
        if depth > 1 || typ == 0x000A {
            if let Some(c) = &mut chart {
                c.push((typ, d.to_vec()));
            }
            if depth == 1
                && let Some(c) = chart.take()
                && let Some(obj) = rules.objects.last_mut()
                && obj.chart.is_none()
            {
                obj.chart = Some(c);
            }
            continue;
        }
        let mut b = Bytes::new(d);
        match typ {
            // BLANK, NUMBER, LABEL, BOOLERR, RK, LABELSST, FORMULA
            // BIFF8's objects, notes, links, validation and conditional
            // formats; BIFF5 has other records under some of these ids.
            0x001C | 0x005D | 0x00EC | 0x01B6 | 0x01B8 | 0x0800 | 0x01BE | 0x01B2 | 0x01B0
            | 0x01B1 | 0x0879 | 0x087A | 0x087B
                if five =>
            {
                continue;
            }
            // BLANK, NUMBER, LABEL, BOOLERR, RK, LABELSST, FORMULA, and
            // BIFF5's RSTRING.
            0x0201 | 0x0203 | 0x0204 | 0x0205 | 0x027E | 0x00FD | 0x0006 | 0x00D6 => {
                if let (Some(r), Some(c), Some(x)) = (b.u16(), b.u16(), b.u16()) {
                    set(model, r, c, x);
                }
            }
            // MULBLANK: row, first column, an XF per cell, last column.
            0x00BE => {
                if let (Some(r), Some(c)) = (b.u16(), b.u16()) {
                    let n = d.len().saturating_sub(6) / 2;
                    for i in 0..n {
                        let Some(x) = b.u16() else { break };
                        set(model, r, c.saturating_add(i as u16), x);
                    }
                }
            }
            // MULRK: row, first column, (XF, RK) per cell, last column.
            0x00BD => {
                if let (Some(r), Some(c)) = (b.u16(), b.u16()) {
                    let n = d.len().saturating_sub(6) / 6;
                    for i in 0..n {
                        let Some(x) = b.u16() else { break };
                        let _ = b.skip(4);
                        set(model, r, c.saturating_add(i as u16), x);
                    }
                }
            }
            0x00E5 => {
                let n = b.u16().unwrap_or(0);
                for _ in 0..n {
                    let (Some(r1), Some(r2), Some(c1), Some(c2)) =
                        (b.u16(), b.u16(), b.u16(), b.u16())
                    else {
                        break;
                    };
                    if r2 < r1 || c2 < c1 || (r1, c1) == (r2, c2) {
                        continue;
                    }
                    let (r2, c2) = (
                        (r2 as u32).min(MAX_ROWS - 1) as i32,
                        (c2 as u32).min(MAX_COLS - 1) as i32,
                    );
                    let m = MergedCell {
                        row: r1 as i32 + 1,
                        column: c1 as i32 + 1,
                        width: c2 - c1 as i32 + 1,
                        height: r2 - r1 as i32 + 1,
                    };
                    if m.width >= 1
                        && m.height >= 1
                        && !merges
                            .iter()
                            .any(|o| o.intersects(m.row, m.column, m.width, m.height))
                    {
                        merges.push(m);
                    }
                }
            }
            0x007D => {
                let c = (|| {
                    let first = b.u16()?;
                    let last = b.u16()?;
                    let width = b.u16()?;
                    let ixfe = b.u16()?;
                    let flags = b.u16()?;
                    Some((first, last, width, ixfe, flags))
                })();
                if let Some((first, last, width, ixfe, flags)) = c {
                    let last = (last as u32).min(MAX_COLS - 1);
                    if (first as u32) <= last {
                        cols.push(Col {
                            min: first as i32 + 1,
                            max: last as i32 + 1,
                            width: width as f64 / 256.0,
                            custom_width: true,
                            hidden: flags & 0x1 != 0,
                            style: None,
                            outline_level: ((flags >> 8) & 0x7) as u8,
                            collapsed: flags & 0x1000 != 0,
                        });
                        col_styles.push((cols.len() - 1, ixfe));
                    }
                }
            }
            0x0055 => default_cch = b.u16(),
            0x0225 => {
                if let (Some(_), Some(h)) = (b.u16(), b.u16()) {
                    default_height = Some((h & 0x7FFF) as f64 / 20.0);
                }
            }
            0x0208 => {
                let r = (|| {
                    let row = b.u16()?;
                    b.skip(4)?;
                    let height = b.u16()?;
                    b.skip(4)?;
                    let flags = b.u16()?;
                    let ixfe = b.u16()? & 0xFFF;
                    Some((row, height, flags, ixfe))
                })();
                if let Some((row, height, flags, ixfe)) = r {
                    rows.push(Row {
                        r: row as i32 + 1,
                        height: (height & 0x7FFF) as f64 / 20.0,
                        custom_format: flags & 0x80 != 0,
                        custom_height: flags & 0x40 != 0,
                        s: 0,
                        hidden: flags & 0x20 != 0,
                        outline_level: (flags & 0x7) as u8,
                        collapsed: flags & 0x10 != 0,
                    });
                    if flags & 0x80 != 0 {
                        row_styles.push((rows.len() - 1, ixfe));
                    }
                }
            }
            0x023E => window2 = b.u16(),
            0x0041 => {
                if let (Some(x), Some(y)) = (b.u16(), b.u16()) {
                    pane = Some((x, y));
                }
            }
            0x0862 => {
                // SHEETEXT: FrtHeader (12 bytes), cb, then icvPlain.
                if b.skip(16).is_some()
                    && let Some(v) = b.u32()
                {
                    tab = Some((v & 0x7F) as u16);
                }
            }
            0x01B8 => {
                if let Some((area, link)) = parse_hlink(d) {
                    for r in area.top..=area.bottom.min(area.top + 1000) {
                        for c in area.left..=area.right.min(area.left + 100) {
                            links.push(((r as i32 + 1, c as i32 + 1), link.clone()));
                        }
                    }
                }
            }
            0x0800 => {
                // HLINKTOOLTIP: rt, a Ref8U, then a null-terminated UTF-16 tip.
                if b.skip(2).is_some()
                    && let (Some(r1), Some(_), Some(c1), Some(_)) =
                        (b.u16(), b.u16(), b.u16(), b.u16())
                {
                    let units: Vec<u16> = d[b.pos..]
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .take_while(|&u| u != 0)
                        .collect();
                    let tip = String::from_utf16_lossy(&units);
                    let at = (r1 as i32 + 1, c1 as i32 + 1);
                    for (cell, link) in links.iter_mut().rev() {
                        if *cell == at {
                            match link {
                                Link::External { tooltip, .. } | Link::Internal { tooltip, .. } => {
                                    *tooltip = Some(tip.clone());
                                }
                            }
                            break;
                        }
                    }
                }
            }
            0x005D => {
                // OBJ: its first subrecord is ftCmo (ft, cb, ot, id).
                let cmo = match (b.u16(), b.u16(), b.u16(), b.u16()) {
                    (Some(0x15), Some(_), Some(ot), Some(id)) => Some((ot, id)),
                    _ => None,
                };
                note_obj = cmo.filter(|c| c.0 == 0x19).map(|c| c.1);
                rules.objects.push(crate::xls_drawing::RawObj {
                    ot: cmo.map(|c| c.0),
                    chart: None,
                    text: None,
                });
            }
            0x00EC => rules.drawing.extend_from_slice(d),
            0x01B6 => {
                let data = d.to_vec();
                let bounds = rec.bounds().to_vec();
                if let Some(id) = note_obj.take() {
                    if let Some(text) = txo_text(&data, &bounds) {
                        texts.insert(id, text);
                    }
                } else if let Some(obj) = rules.objects.last_mut()
                    && obj.text.is_none()
                {
                    // A text box's or shape's text, and its alignment
                    // (grbit: horizontal in bits 1-3, vertical in 4-6).
                    let grbit = u16::from_le_bytes([
                        data.first().copied().unwrap_or(0),
                        data.get(1).copied().unwrap_or(0),
                    ]);
                    obj.text = txo_text(&data, &bounds).map(|t| crate::xls_drawing::ObjText {
                        text: t,
                        h_align: ((grbit >> 1) & 7) as u8,
                        v_align: ((grbit >> 4) & 7) as u8,
                    });
                }
            }
            0x001C => {
                let n = (|| {
                    let (r, c, _flags, id) = (b.u16()?, b.u16()?, b.u16()?, b.u16()?);
                    Some((r, c, id, b.string16().unwrap_or_default()))
                })();
                if let Some(n) = n {
                    notes.push(n);
                }
            }
            0x01BE => {
                if let Some(dv) = RawDv::parse(d)
                    && !dv.ranges.is_empty()
                {
                    rules.validations.push(dv);
                }
            }
            0x01B0 => {
                // CONDFMT: ccf, fToughRecalc and nID, refBound, sqref.
                condfmt = (|| {
                    let ccf = b.u16()?;
                    let id = b.u16()? >> 1;
                    b.skip(8)?;
                    Some((sqref(&mut b)?, ccf, id, 0))
                })()
                .filter(|(r, ..)| !r.is_empty());
                if let Some((ranges, _, id, _)) = &condfmt {
                    condfmt_ranges.insert(*id, ranges.clone());
                }
            }
            0x01B1 => {
                if let Some((ranges, left, id, next)) = &mut condfmt
                    && *left > 0
                {
                    *left -= 1;
                    match RawCf::parse(d) {
                        Some(mut cf) => {
                            cf.id = Some((*id, *next));
                            rules.cf.push((ranges.clone(), cf));
                        }
                        None => rules.cf12 += 1,
                    }
                    *next += 1;
                }
            }
            // CONDFMT12 and its CF12 rules (Excel 2007), CFEX (Excel 2007
            // templates of CF rules).
            0x0879 => condfmt12 = cf12::condfmt12(d).filter(|(r, _)| !r.is_empty()),
            0x087A => {
                let ranges = match &mut condfmt12 {
                    Some((ranges, left)) if *left > 0 => {
                        *left -= 1;
                        Some(ranges.clone())
                    }
                    _ => cf12_owner.take(),
                };
                rules.cf12_raw.push((ranges, d.to_vec()));
            }
            0x087B => match cf12::parse_cfex(d) {
                Some(cf12::CfEx::Cf(id, icf, ext)) => rules.cfex.push((id, icf, ext)),
                Some(cf12::CfEx::Cf12(id)) => {
                    condfmt12 = None;
                    cf12_owner = condfmt_ranges.get(&id).cloned();
                }
                None => {}
            },
            0x00A1 => {
                print.setup =
                    (|| Some((b.u16()?, b.u16()?, b.i16()?, b.u16()?, b.u16()?, b.u16()?)))();
            }
            0x0026..=0x0029 => {
                if let Some(v) = b
                    .f64()
                    .filter(|v| v.is_finite() && (0.0..=49.0).contains(v))
                {
                    print.margins[(typ - 0x0026) as usize] = Some(v);
                }
            }
            0x0083 => print.hcenter = b.u16() == Some(1),
            0x0084 => print.vcenter = b.u16() == Some(1),
            0x002B => print.grid = b.u16() == Some(1),
            0x002A => print.headings = b.u16() == Some(1),
            0x0081 => print.fit_to_page = b.u16().is_some_and(|f| f & 0x100 != 0),
            // BIFF5: an 8-bit count.
            0x0014 if five => print.header = b.string8().unwrap_or_default(),
            0x0015 if five => print.footer = b.string8().unwrap_or_default(),
            0x0014 => print.header = b.string16().unwrap_or_default(),
            0x0015 => print.footer = b.string16().unwrap_or_default(),
            0x001B | 0x001A => {
                let n = b.u16().unwrap_or(0);
                let mut breaks = Vec::new();
                for _ in 0..n {
                    let Some(at) = b.u16() else { break };
                    let _ = b.skip(4);
                    if at > 0 {
                        breaks.push(at as u32);
                    }
                }
                breaks.sort_unstable();
                breaks.dedup();
                if typ == 0x001B {
                    print.row_breaks = breaks;
                } else {
                    print.col_breaks = breaks;
                }
            }
            _ => {}
        }
    }

    for (i, ixfe) in col_styles {
        let s = styles.get(model, ixfe);
        if s != 0 {
            cols[i].style = Some(s);
        }
    }
    for (i, ixfe) in row_styles {
        rows[i].s = styles.get(model, ixfe);
    }
    let Ok(ws) = model.workbook.worksheet_mut(sheet) else {
        return print;
    };

    // Columns: sorted, without overlaps; the gaps get the sheet's default
    // width when it isn't Excel's usual 8 digits.
    cols.sort_by_key(|c| c.min);
    let mut kept: Vec<Col> = Vec::with_capacity(cols.len());
    for c in cols {
        if kept.last().is_some_and(|k| k.max >= c.min) {
            continue;
        }
        kept.push(c);
    }
    if let Some(cch) = default_cch.filter(|&c| c != 8 && c <= 255) {
        let width = chars_to_width(cch);
        let mut gaps = Vec::new();
        let mut next = 1;
        for c in &kept {
            if c.min > next {
                gaps.push((next, c.min - 1));
            }
            next = c.max + 1;
        }
        if next <= 256 {
            gaps.push((next, 256));
        }
        for (min, max) in gaps {
            kept.push(Col {
                min,
                max,
                width,
                custom_width: false,
                hidden: false,
                style: None,
                outline_level: 0,
                collapsed: false,
            });
        }
        kept.sort_by_key(|c| c.min);
    }
    ws.cols = kept;

    if let Some(h) = default_height.filter(|h| *h > 0.0) {
        ws.default_row_height = Some(h);
    }
    let default_points = ws
        .default_row_height
        .unwrap_or(crate::rows::DEFAULT_ROW_POINTS);
    rows.retain(|r| {
        r.custom_height
            || r.hidden
            || r.custom_format
            || r.outline_level > 0
            || (r.height - default_points).abs() > 0.001
    });
    rows.sort_by_key(|r| r.r);
    rows.dedup_by_key(|r| r.r);
    ws.rows = rows;
    if !rules.objects.is_empty() {
        rules.geometry = crate::xls_drawing::Geometry::of(ws, default_points);
    }

    // Covered cells of a merge hold no content (IronCalc's rule).
    for m in &merges {
        for r in m.row..=m.last_row() {
            for c in m.column..=m.last_column() {
                if (r, c) != (m.row, m.column)
                    && ws.cell(r, c).is_some_and(|cell| {
                        !matches!(*cell, ironcalc_base::types::Cell::EmptyCell { .. })
                    })
                {
                    *cleared |= ws.cell_clear_contents(r, c).is_ok();
                }
            }
        }
    }
    ws.merged_cells = merges;

    for (cell, link) in links {
        ws.links.insert(cell, link);
    }

    for (r, c, id, author) in notes {
        let Some(text) = texts.remove(&id) else {
            continue;
        };
        if u32::from(r) >= MAX_ROWS || u32::from(c) >= MAX_COLS {
            continue;
        }
        ws.comments.push(Comment {
            text,
            author_name: author,
            author_id: None,
            cell_ref: crate::cell_name(r as u32, c as u32),
        });
    }

    if let Some(flags) = window2 {
        ws.show_grid_lines = flags & 0x2 != 0;
        if flags & 0x8 != 0
            && let Some((x, y)) = pane
        {
            ws.frozen_columns = (x as u32).min(MAX_COLS - 1) as i32;
            ws.frozen_rows = (y as u32).min(MAX_ROWS - 1) as i32;
        }
    }
    if let Some(icv) = tab {
        ws.color = styles.g.palette.color(icv);
    }
    print
}

/// The text of a `TXO` record (MS-XLS 2.4.329) whose `CONTINUE`s start at
/// `bounds`: `cchText` at offset 10; the characters fill the continuations,
/// each starting with its own flags byte (compressed or UTF-16).
fn txo_text(d: &[u8], bounds: &[usize]) -> Option<String> {
    let cch = u16::from_le_bytes([*d.get(10)?, *d.get(11)?]) as usize;
    if cch == 0 {
        return Some(String::new());
    }
    let mut units: Vec<u16> = Vec::with_capacity(cch);
    for (i, &start) in bounds.iter().enumerate() {
        let end = bounds.get(i + 1).copied().unwrap_or(d.len());
        let seg = d.get(start..end)?;
        let (&flags, chars) = seg.split_first()?;
        let left = cch - units.len();
        if flags & 0x01 != 0 {
            units.extend(
                chars
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .take(left)
                    .map(|c| u16::from_le_bytes([c[0], c[1]])),
            );
        } else {
            units.extend(chars.iter().take(left).map(|&b| b as u16));
        }
        if units.len() >= cch {
            break;
        }
    }
    // Excel stores line breaks in notes as CR LF or CR.
    let text = String::from_utf16_lossy(&units)
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    Some(text)
}

/// `HLINK` (MS-XLS 2.4.140): the cells and the link. URLs (including
/// `mailto:`), file paths and places in the workbook; other monikers are
/// skipped.
fn parse_hlink(d: &[u8]) -> Option<(CellRange, Link)> {
    const URL: [u8; 16] = [
        0xE0, 0xC9, 0xEA, 0x79, 0xF9, 0xBA, 0xCE, 0x11, 0x8C, 0x82, 0x00, 0xAA, 0x00, 0x4B, 0xA9,
        0x0B,
    ];
    const FILE: [u8; 16] = [
        0x03, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x46,
    ];
    let mut b = Bytes::new(d);
    let (r1, r2, c1, c2) = (b.u16()?, b.u16()?, b.u16()?, b.u16()?);
    b.skip(16)?; // hlinkClsid
    let _version = b.u32()?;
    let flags = b.u32()?;
    let has_moniker = flags & 0x1 != 0;
    let has_location = flags & 0x8 != 0;
    let has_display = flags & 0x14 != 0;
    let has_frame = flags & 0x80 != 0;
    let moniker_as_string = flags & 0x100 != 0;
    if has_display {
        b.hyperlink_string()?;
    }
    if has_frame {
        b.hyperlink_string()?;
    }
    let mut target = None;
    if has_moniker {
        if moniker_as_string {
            target = Some(b.hyperlink_string()?);
        } else {
            let clsid = b.take(16)?;
            if clsid == URL {
                let len = b.u32()? as usize;
                let raw = b.take(len)?;
                let units: Vec<u16> = raw
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .take_while(|&u| u != 0)
                    .collect();
                target = Some(String::from_utf16_lossy(&units));
            } else if clsid == FILE {
                let up = b.u16()?;
                let len = b.u32()? as usize;
                let ansi = b.take(len)?;
                let ansi: String = ansi
                    .iter()
                    .take_while(|&&c| c != 0)
                    .map(|&c| c as char)
                    .collect();
                b.skip(2 + 2 + 20)?;
                let size = b.u32()?;
                let path = if size > 0 {
                    let bytes = b.u32()? as usize;
                    b.skip(2)?;
                    let raw = b.take(bytes)?;
                    let units: Vec<u16> = raw
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect();
                    String::from_utf16_lossy(&units)
                } else {
                    ansi
                };
                target = Some(format!(
                    "{}{}",
                    "../".repeat(up as usize),
                    path.replace('\\', "/")
                ));
            } else {
                return None;
            }
        }
    }
    let location = if has_location {
        Some(b.hyperlink_string()?)
    } else {
        None
    };
    let link = match (target, location) {
        (Some(t), Some(l)) if !l.is_empty() => Link::External {
            target: format!("{t}#{l}"),
            tooltip: None,
        },
        (Some(t), _) if !t.is_empty() => Link::External {
            target: t,
            tooltip: None,
        },
        (_, Some(l)) if !l.is_empty() => Link::Internal {
            location: l,
            tooltip: None,
        },
        _ => return None,
    };
    if r2 < r1 || c2 < c1 || u32::from(r1) >= MAX_ROWS || u32::from(c1) >= MAX_COLS {
        return None;
    }
    Some((
        CellRange::from_corners(
            (r1 as u32, c1 as u32),
            ((r2 as u32).min(MAX_ROWS - 1), (c2 as u32).min(MAX_COLS - 1)),
        ),
        link,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncated_and_garbage_records_dont_panic() {
        // A BOF that claims more bytes than there are.
        let stream = [0x09, 0x08, 0xFF, 0x00, 0x00, 0x06];
        assert!(read_globals(&stream).is_err());
        // Every prefix of a small valid globals stream.
        let mut s = Vec::new();
        let mut push = |typ: u16, data: &[u8]| {
            s.extend_from_slice(&typ.to_le_bytes());
            s.extend_from_slice(&(data.len() as u16).to_le_bytes());
            s.extend_from_slice(data);
        };
        push(0x0809, &[0x00, 0x06, 0x05, 0x00, 0, 0, 0, 0]);
        push(0x0031, &[0xC8, 0x00, 0x00]);
        push(0x00E0, &[1, 2, 3]);
        push(0x0092, &[0xFF, 0x00, 1, 2]);
        push(0x0085, &[0, 0, 0, 0, 0, 0, 5, 0, b'a']);
        push(0x0018, &[0x20]);
        push(0x000A, &[]);
        for n in 0..s.len() {
            let _ = read_globals(&s[..n]);
        }
        for d in [&[][..], &[1, 2, 3], &[0; 30], &[0xFF; 40]] {
            let _ = parse_hlink(d);
            let _ = parse_label(d);
            let _ = Xf::parse(d);
        }
    }

    /// The fixture's stream with the payloads of its records damaged (the
    /// record headers kept, so every parser meets garbage counts, lengths
    /// and offsets), and with records cut short.
    #[test]
    fn damaged_streams_dont_panic() {
        damage("formatted.xls", &["Report", "Plain", "Secret"]);
        // Notes, data validation and conditional formats.
        damage("features.xls", &["Checks", "Other"]);
    }

    fn damage(file: &str, names: &[&str]) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/xls")
            .join(file);
        let clean = XlsFormat::read(&path).unwrap().stream;
        // Payload ranges, past the first BOF.
        let mut payloads = Vec::new();
        let mut pos = 0;
        while pos + 4 <= clean.len() {
            let len = u16::from_le_bytes([clean[pos + 2], clean[pos + 3]]) as usize;
            if pos > 0 {
                payloads.push((pos + 4, len));
            }
            pos += 4 + len;
        }
        let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        let mut applied = 0;
        for seed in 0..60usize {
            let mut stream = clean.clone();
            for (i, &(start, len)) in payloads.iter().enumerate() {
                if len == 0 || (i + seed) % 3 != 0 {
                    continue;
                }
                let at = start + (seed * 7 + i) % len;
                stream[at] = stream[at].wrapping_add(0x3B ^ (seed as u8));
                if seed % 5 == 0 {
                    stream[at] = 0xFF;
                }
            }
            if seed % 4 == 0 {
                // Cut the stream inside a record.
                stream.truncate(clean.len() * (seed + 10) / 80);
            }
            let Ok(globals) = read_globals(&stream) else {
                continue;
            };
            let x = XlsFormat { stream, globals };
            let mut model = crate::import::empty_model_for_tests();
            for n in &names[1..] {
                let _ = model.add_sheet(n);
            }
            let _ = x.apply(&mut model, &names);
            applied += 1;
        }
        assert!(applied > 30, "{applied}");
    }

    #[test]
    fn note_text_across_continues() {
        // cchText 5 at offset 10; "ab" compressed, then "cd\r" as UTF-16.
        let mut d = vec![0u8; 18];
        d[10] = 5;
        d.extend_from_slice(&[0, b'a', b'b']);
        d.extend_from_slice(&[1, b'c', 0, b'd', 0, b'\r', 0]);
        assert_eq!(txo_text(&d, &[18, 21]).as_deref(), Some("abcd\n"));
        assert_eq!(txo_text(&d, &[18]).as_deref(), Some("ab\u{1}c\0"));
        assert_eq!(txo_text(&d[..5], &[]), None);
    }

    #[test]
    fn patterns_mix_colours() {
        let p = Palette(DEFAULT_PALETTE.to_vec());
        assert_eq!(fill_color(0, 10, 9, &p), Color::None);
        assert_eq!(fill_color(1, 10, 9, &p), Color::Rgb("#FF0000".into()));
        // 50 % grey: black dots on white.
        assert_eq!(fill_color(2, 8, 9, &p), Color::Rgb("#808080".into()));
        assert_eq!(fill_color(2, 64, 65, &p), Color::Rgb("#808080".into()));
    }

    #[test]
    fn default_widths() {
        assert_eq!(chars_to_width(8), 9.140625);
        // 70 + 5 px, rounded up to 80.
        assert_eq!(chars_to_width(10), (80.0f64 / 7.0 * 256.0).trunc() / 256.0);
    }
}
