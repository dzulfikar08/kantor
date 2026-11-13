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
//! Not read: named cell styles (`STYLE`; cells keep the formats they show),
//! comments (Kantor neither shows nor saves them for imported files), data
//! validation and conditional formatting (their formulas are parsed tokens),
//! charts, pictures and shapes. Every read is bounds-checked: a malformed
//! record is skipped, never a panic.

use std::io::Read as _;
use std::path::Path;

use anyhow::{Result, anyhow, bail};
use ironcalc_base::Model;
use ironcalc_base::types::{
    Alignment, Border, BorderItem, BorderStyle, CellProtection, Col, Color, Fill, Font, FontScheme,
    HorizontalAlignment, Link, MergedCell, NumFmt, Row, SheetState, Style, VerticalAlignment,
};

use crate::print::{HeaderFooter, Margins, PageSetup, Paper, PrintOptions, SheetPrint};
use crate::{CellRange, MAX_COLS, MAX_ROWS};

/// Days between the 1900 and 1904 date systems' day zero.
pub(crate) const DAYS_1904: f64 = 1462.0;

/// A record and its `CONTINUE`s, joined.
struct Records<'a> {
    data: &'a [u8],
    pos: usize,
    buf: Vec<u8>,
}

const CONTINUE: u16 = 0x003C;

impl<'a> Records<'a> {
    fn at(data: &'a [u8], pos: usize) -> Self {
        Self {
            data,
            pos,
            buf: Vec::new(),
        }
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
    fn next(&mut self) -> Option<(u16, &[u8])> {
        let (typ, len) = self.header(self.pos)?;
        let start = self.pos + 4;
        let end = start.checked_add(len)?;
        if end > self.data.len() {
            return None;
        }
        self.pos = end;
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
            self.buf.extend_from_slice(part);
            self.pos = e;
        }
        Some((typ, &self.buf))
    }
}

/// Little-endian reads that return `None` past the end.
struct Bytes<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bytes<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.data.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }
    fn skip(&mut self, n: usize) -> Option<()> {
        self.take(n).map(|_| ())
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }
    fn i16(&mut self) -> Option<i16> {
        self.u16().map(|v| v as i16)
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn f64(&mut self) -> Option<f64> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Some(f64::from_le_bytes(a))
    }
    /// The characters of an `XLUnicodeString` after its count: a flags byte
    /// (bit 0: two bytes per character; bit 2: extended data; bit 3: rich
    /// text runs), then the characters.
    fn chars(&mut self, cch: usize) -> Option<String> {
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
    fn string16(&mut self) -> Option<String> {
        let cch = self.u16()? as usize;
        self.chars(cch)
    }
    /// `ShortXLUnicodeString`: an 8-bit count, then [`Self::chars`].
    fn string8(&mut self) -> Option<String> {
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
const DEFAULT_PALETTE: [u32; 56] = [
    0x000000, 0xFFFFFF, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00, 0xFF00FF, 0x00FFFF, 0x800000,
    0x008000, 0x000080, 0x808000, 0x800080, 0x008080, 0xC0C0C0, 0x808080, 0x9999FF, 0x993366,
    0xFFFFCC, 0xCCFFFF, 0x660066, 0xFF8080, 0x0066CC, 0xCCCCFF, 0x000080, 0xFF00FF, 0xFFFF00,
    0x00FFFF, 0x800080, 0x800000, 0x008080, 0x0000FF, 0x00CCFF, 0xCCFFFF, 0xCCFFCC, 0xFFFF99,
    0x99CCFF, 0xFF99CC, 0xCC99FF, 0xFFCC99, 0x3366FF, 0x33CCCC, 0x99CC00, 0xFFCC00, 0xFF9900,
    0xFF6600, 0x666699, 0x969696, 0x003366, 0x339966, 0x003300, 0x333300, 0x993300, 0x993366,
    0x333399, 0x333333,
];

/// The colours of indices 0-7 (fixed, whatever the palette).
const FIXED_COLOURS: [u32; 8] = [
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

fn rgb_color(rgb: u32) -> Color {
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
        }
    }
}

/// The workbook-level part of the stream.
pub(crate) struct Globals {
    pub date1904: bool,
    /// Sheets in tab order: (name, stream offset, visibility, is a
    /// worksheet).
    sheets: Vec<(String, usize, SheetState, bool)>,
    pub labels: Vec<Label>,
    /// The tab Excel shows when the file opens.
    pub active: u32,
    fonts: Vec<XlsFont>,
    formats: Vec<NumFmt>,
    xfs: Vec<Xf>,
    palette: Palette,
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
        file.open_stream("/Workbook")
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
    /// `names` order. Returns each sheet's print settings, by model sheet,
    /// and whether values changed (covered cells of merges were cleared).
    pub(crate) fn apply(&self, model: &mut Model<'_>, names: &[String]) -> (Vec<SheetPrint>, bool) {
        let g = &self.globals;
        let mut styles = StyleMap::new(g, model);
        let mut prints = Vec::with_capacity(names.len());
        let mut cleared = false;
        for (sheet, name) in names.iter().enumerate() {
            let Some(ix) = self.sheet_named(name) else {
                prints.push(SheetPrint::default());
                continue;
            };
            let (_, offset, state, is_worksheet) = &g.sheets[ix];
            if let Ok(ws) = model.workbook.worksheet_mut(sheet as u32) {
                ws.state = state.clone();
            }
            if !is_worksheet {
                prints.push(SheetPrint::default());
                continue;
            }
            let print = read_sheet(
                &self.stream,
                *offset,
                sheet as u32,
                model,
                &mut styles,
                &mut cleared,
            );
            prints.push(print.into_print());
        }
        (prints, cleared)
    }
}

fn read_globals(stream: &[u8]) -> Result<Globals> {
    let mut rec = Records::at(stream, 0);
    match rec.next() {
        Some((0x0809, d)) if d.len() >= 4 => {
            let version = u16::from_le_bytes([d[0], d[1]]);
            if version != 0x0600 {
                bail!("not BIFF8 (version {version:#x})");
            }
        }
        _ => bail!("no BOF record"),
    }
    let mut g = Globals {
        date1904: false,
        sheets: Vec::new(),
        labels: Vec::new(),
        active: 0,
        fonts: Vec::new(),
        formats: Vec::new(),
        xfs: Vec::new(),
        palette: Palette(DEFAULT_PALETTE.to_vec()),
    };
    while let Some((typ, d)) = rec.next() {
        let mut b = Bytes::new(d);
        match typ {
            0x000A => break, // EOF
            0x002F => bail!("the file is encrypted"),
            0x0022 => g.date1904 = b.u16() == Some(1),
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
                if let (Some(id), Some(code)) = (b.u16(), b.string16()) {
                    g.formats.push(NumFmt {
                        num_fmt_id: id as i32,
                        format_code: code,
                    });
                }
            }
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
                    Some((name, pos, state, kind == 0))
                })();
                if let Some(s) = sheet {
                    g.sheets.push(s);
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
                styles
                    .get_style_index(&style)
                    .unwrap_or_else(|| styles.create_new_style(&style))
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

fn border_style(dg: u8) -> Option<BorderStyle> {
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
) -> PrintRaw {
    let mut print = PrintRaw::default();
    let mut rec = Records::at(stream, offset);
    match rec.next() {
        Some((0x0809, _)) => {}
        _ => return print,
    }
    let mut depth = 1;
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
    while let Some((typ, d)) = rec.next() {
        if typ == 0x0809 {
            // An embedded chart's substream.
            depth += 1;
            continue;
        }
        if typ == 0x000A {
            depth -= 1;
            if depth == 0 {
                break;
            }
            continue;
        }
        if depth > 1 {
            continue;
        }
        let mut b = Bytes::new(d);
        match typ {
            // BLANK, NUMBER, LABEL, BOOLERR, RK, LABELSST, FORMULA
            0x0201 | 0x0203 | 0x0204 | 0x0205 | 0x027E | 0x00FD | 0x0006 => {
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
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/corpus/xls/formatted.xls"
        );
        let clean = XlsFormat::read(Path::new(path)).unwrap().stream;
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
        let names: Vec<String> = ["Report", "Plain", "Secret"].map(String::from).into();
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
