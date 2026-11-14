//! Pictures and charts of Excel 97-2003 (`.xls`, BIFF8) files.
//!
//! [`crate::xls`] collects, per sheet, the OfficeArt drawing (`MSODRAWING`
//! records joined) and the `OBJ` records with the chart substream (BOF type
//! `0x0020`) that follows a chart's `OBJ`; the workbook's
//! `MSODRAWINGGROUP` holds the pictures (the BLIP store). This module reads
//! them:
//!
//! * **Shapes**: each shape container with client data in the drawing pairs
//!   with the next `OBJ` (MS-XLS 2.4.181). Its client anchor gives the
//!   corners as cells plus offsets in 1/1024 of the column width and 1/256
//!   of the row height, turned into EMU from the sheet's column widths and
//!   row heights ([`Geometry`]); `fMove`/`fSize` become `editAs`. Pictures
//!   take their BLIP (`pib`), name, alternative text and cropping from the
//!   shape's properties. PNG and JPEG pictures are kept; metafiles (EMF,
//!   WMF, PICT), DIB and TIFF are left out with an import note.
//! * **Charts** ([`Chart`]): `SERIES` with their `AI` references (formula
//!   tokens decompiled by [`crate::xls_formula`]) and names, the chart group
//!   types (`BAR` vertical, horizontal, stacked and 100%, `LINE`, `AREA`,
//!   `PIE` and doughnut, `SCATTER`), the chart and axis titles (`TEXT` with
//!   `OBJECTLINK`), the legend, gridlines and series colours set in the
//!   file. Radar, surface, bubble and bar-of-pie charts are left out with an
//!   import note; 3-D charts are shown flat. Chart sheets (`BOUNDSHEET` type
//!   2) are read the same way.
//!
//! [`package`] writes what was read as a small `.xlsx` package (a worksheet
//! part per sheet with its drawing, chart and media parts, and chart sheet
//! parts) that becomes the import's original: [`crate::drawing`] shows the
//! objects from it and `preserve` carries them into the first `.xlsx` save,
//! as for an `.xlsx` that was opened.

use std::collections::HashMap;
use std::io::Write as _;

use ironcalc_base::types::Worksheet;

use crate::drawing::{Anchor, EMU_PER_PX, EditAs, Marker, RefData, RefValue};
use crate::xls::{Bytes, Records};
use crate::xls_formula::{Context, decompile};

/// An `OBJ` record of a sheet: its type (`ftCmo.ot`; `None` if unreadable)
/// and, for a chart, the records of its substream.
pub(crate) struct RawObj {
    pub ot: Option<u16>,
    pub chart: Option<Vec<(u16, Vec<u8>)>>,
}

/// A sheet's column widths and row heights in Excel's pixels.
#[derive(Default)]
pub(crate) struct Geometry {
    /// By 0-based column; columns past the end have `default_col`.
    cols: Vec<f64>,
    default_col: f64,
    /// Rows that aren't the default height, by 0-based row.
    rows: HashMap<u32, f64>,
    default_row: f64,
}

/// Pixels of a column `width` characters wide as stored (with padding),
/// in the default font (7 px digits): Excel's
/// `trunc((256 * width + trunc(128 / 7)) / 256 * 7)`.
fn width_px(width: f64) -> f64 {
    ((256.0 * width + 18.0) / 256.0 * 7.0).trunc().max(0.0)
}

/// Stored width of Excel's default column (8 digits with padding).
const DEFAULT_WIDTH: f64 = 9.140625;

impl Geometry {
    /// The geometry of `ws` once its columns and rows are set.
    pub(crate) fn of(ws: &Worksheet, default_points: f64) -> Self {
        let mut cols = vec![width_px(DEFAULT_WIDTH); 256];
        for c in &ws.cols {
            let px = if c.hidden { 0.0 } else { width_px(c.width) };
            for col in c.min.max(1)..=c.max.min(256) {
                cols[col as usize - 1] = px;
            }
        }
        let rows = ws
            .rows
            .iter()
            .map(|r| {
                let px = if r.hidden { 0.0 } else { r.height * 4.0 / 3.0 };
                (r.r.max(1) as u32 - 1, px)
            })
            .collect();
        Self {
            cols,
            default_col: width_px(DEFAULT_WIDTH),
            rows,
            default_row: default_points * 4.0 / 3.0,
        }
    }

    fn col_px(&self, col: u32) -> f64 {
        if self.cols.is_empty() {
            return width_px(DEFAULT_WIDTH);
        }
        self.cols
            .get(col as usize)
            .copied()
            .unwrap_or(self.default_col)
    }

    fn row_px(&self, row: u32) -> f64 {
        match self.rows.get(&row) {
            Some(px) => *px,
            None if self.default_row > 0.0 => self.default_row,
            None => 20.0,
        }
    }

    /// An `OfficeArtClientAnchorSheet` (flags, then column, 1/1024 of its
    /// width, row, 1/256 of its height for the top left and bottom right).
    fn anchor(&self, a: &[u16; 9]) -> Anchor {
        let [flags, c1, dx1, r1, dy1, c2, dx2, r2, dy2] = *a;
        let marker = |col: u16, dx: u16, row: u16, dy: u16| {
            let (col, row) = (u32::from(col.min(255)), u32::from(row));
            let x = f64::from(dx.min(1024)) / 1024.0 * self.col_px(col);
            let y = f64::from(dy.min(256)) / 256.0 * self.row_px(row);
            Marker {
                col,
                col_off: (x * EMU_PER_PX).round() as i64,
                row,
                row_off: (y * EMU_PER_PX).round() as i64,
            }
        };
        let from = marker(c1, dx1, r1, dy1);
        let mut to = marker(c2, dx2, r2, dy2);
        if (to.row, to.col) < (from.row, from.col) {
            to = from;
        }
        // fMove (bit 0): stays put when cells move; fSize (bit 1): keeps
        // its size when they are resized.
        let edit_as = match flags & 0x3 {
            0 => EditAs::TwoCell,
            2 => EditAs::OneCell,
            _ => EditAs::Absolute,
        };
        Anchor::TwoCell { from, to, edit_as }
    }
}

/// One OfficeArt record: instance, type and body.
struct Art<'a> {
    inst: u16,
    typ: u16,
    body: &'a [u8],
}

/// The records of one level of an OfficeArt stream. A body running past
/// the end (a drawing split oddly) is cut at the end.
fn art(mut b: &[u8]) -> Vec<Art<'_>> {
    let mut out = Vec::new();
    while b.len() >= 8 {
        let vi = u16::from_le_bytes([b[0], b[1]]);
        let typ = u16::from_le_bytes([b[2], b[3]]);
        let len = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize;
        let end = 8usize.saturating_add(len).min(b.len());
        out.push(Art {
            inst: vi >> 4,
            typ,
            body: &b[8..end],
        });
        b = &b[end..];
    }
    out
}

/// A picture of the BLIP store.
pub(crate) enum Blip {
    /// PNG or JPEG: its extension and bytes.
    Image { ext: &'static str, data: Vec<u8> },
    /// A format Kantor doesn't show (metafiles, DIB, TIFF).
    Unsupported,
}

/// The BLIP store of the drawing group, by `pib - 1`. `None`: an empty
/// slot or a picture kept outside the store.
pub(crate) fn blip_store(group: &[u8]) -> Vec<Option<Blip>> {
    let mut out = Vec::new();
    for dgg in art(group).into_iter().filter(|r| r.typ == 0xF000) {
        for store in art(dgg.body).into_iter().filter(|r| r.typ == 0xF001) {
            for fbse in art(store.body) {
                out.push(if fbse.typ == 0xF007 {
                    fbse_blip(fbse.body)
                } else {
                    None
                });
            }
        }
    }
    out
}

/// The picture embedded in an `OfficeArtFBSE` (36 bytes, a name, then the
/// BLIP record).
fn fbse_blip(body: &[u8]) -> Option<Blip> {
    let name_len = *body.get(33)? as usize;
    let rest = body.get(36 + name_len..)?;
    let blip = art(rest).into_iter().next()?;
    // Bitmaps: one or two 16-byte ids by instance, then a tag byte.
    let (ext, two_ids) = match blip.typ {
        0xF01D | 0xF02A => ("jpeg", matches!(blip.inst, 0x46B | 0x6E3)),
        0xF01E => ("png", blip.inst == 0x6E1),
        0xF01A..=0xF01C | 0xF01F | 0xF029 => return Some(Blip::Unsupported),
        _ => return None,
    };
    let data = blip.body.get(if two_ids { 33 } else { 17 }..)?;
    let ok = match ext {
        "png" => data.starts_with(b"\x89PNG"),
        _ => data.starts_with(&[0xFF, 0xD8]),
    };
    ok.then(|| Blip::Image {
        ext,
        data: data.to_vec(),
    })
}

/// A shape of a sheet's drawing that has client data (an `OBJ`).
#[derive(Default)]
struct Shape {
    anchor: Option<[u16; 9]>,
    pib: Option<u32>,
    name: Option<String>,
    descr: Option<String>,
    /// Left, top, right, bottom, as fractions.
    crop: [f32; 4],
    hidden: bool,
    /// Inside a group (placed relative to it): not shown.
    grouped: bool,
}

/// The shapes with client data in the order of their `OBJ`s.
fn shapes(drawing: &[u8]) -> Vec<Shape> {
    /// `level` 1 is the sheet's group; a group inside it (level 2) is an
    /// object of the sheet through its first shape, its other shapes are
    /// inside it, as is everything deeper.
    fn walk(records: Vec<Art<'_>>, level: u32, out: &mut Vec<Shape>) {
        for (i, r) in records.into_iter().enumerate() {
            match r.typ {
                0xF002 => walk(art(r.body), level, out),
                0xF003 => walk(art(r.body), level + 1, out),
                0xF004 => {
                    let mut s = Shape {
                        grouped: level >= 3 || (level == 2 && i > 0),
                        ..Default::default()
                    };
                    let mut data = false;
                    for c in art(r.body) {
                        match c.typ {
                            0xF00B | 0xF122 => properties(&c, &mut s),
                            0xF010 => {
                                let mut b = Bytes::new(c.body);
                                let mut a = [0u16; 9];
                                if a.iter_mut().all(|v| b.u16().map(|x| *v = x).is_some()) {
                                    s.anchor = Some(a);
                                }
                            }
                            0xF011 => data = true,
                            _ => {}
                        }
                    }
                    if data {
                        out.push(s);
                    }
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(art(drawing), 0, &mut out);
    out
}

/// What `shape` takes from an `OfficeArtFOPT`: the picture, cropping,
/// name, description and whether it is hidden.
fn properties(r: &Art<'_>, shape: &mut Shape) {
    let n = r.inst as usize;
    let Some(table) = r.body.get(..n * 6) else {
        return;
    };
    let mut complex = n * 6;
    for p in table.as_chunks::<6>().0 {
        let opid = u16::from_le_bytes([p[0], p[1]]);
        let op = u32::from_le_bytes([p[2], p[3], p[4], p[5]]);
        let id = opid & 0x3FFF;
        if opid & 0x8000 != 0 {
            // Complex: `op` bytes after the table, in order.
            let data = r.body.get(complex..complex.saturating_add(op as usize));
            complex = complex.saturating_add(op as usize);
            let text = || {
                let units: Vec<u16> = data?
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .take_while(|&u| u != 0)
                    .collect();
                Some(String::from_utf16_lossy(&units)).filter(|s| !s.is_empty())
            };
            match id {
                0x0380 => shape.name = text(),
                0x0381 => shape.descr = text(),
                _ => {}
            }
            continue;
        }
        let frac = || (op as i32) as f32 / 65536.0;
        match id {
            0x0100 => shape.crop[1] = frac(),
            0x0101 => shape.crop[3] = frac(),
            0x0102 => shape.crop[0] = frac(),
            0x0103 => shape.crop[2] = frac(),
            0x0104 => shape.pib = Some(op),
            // Group shape booleans: fHidden with its "use" bit.
            0x03BF => shape.hidden = op & 0x0002_0000 != 0 && op & 0x2 != 0,
            _ => {}
        }
    }
}

/// An object of a sheet, read.
pub(crate) struct Object {
    pub anchor: Anchor,
    pub name: Option<String>,
    pub descr: Option<String>,
    pub hidden: bool,
    pub kind: ObjectKind,
}

pub(crate) enum ObjectKind {
    /// Index into the BLIP store.
    Picture {
        blip: usize,
        crop: [f32; 4],
    },
    Chart(Chart),
}

/// What could not be brought in, for the import notes.
#[derive(Default)]
pub(crate) struct Skipped {
    /// Pictures in a format Kantor doesn't show, or missing.
    pub pictures: usize,
    /// Charts of a type Kantor doesn't draw (or unreadable).
    pub charts: usize,
    /// 3-D charts shown flat.
    pub flat: usize,
    /// Shapes, text boxes, controls, grouped objects.
    pub shapes: usize,
}

impl Skipped {
    pub(crate) fn notes(&self) -> Vec<String> {
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let mut out = Vec::new();
        if self.pictures > 0 {
            out.push(format!(
                "{} picture{} not imported (EMF, WMF, PICT, BMP or TIFF format)",
                self.pictures,
                plural(self.pictures)
            ));
        }
        if self.charts > 0 {
            out.push(format!(
                "{} chart{} not imported (radar, surface, bubble, bar of pie or combination charts)",
                self.charts,
                plural(self.charts)
            ));
        }
        if self.flat > 0 {
            out.push(format!(
                "{} 3-D chart{} shown flat",
                self.flat,
                plural(self.flat)
            ));
        }
        if self.shapes > 0 {
            out.push(format!(
                "{} shape{}, text box{} or control{} not imported",
                self.shapes,
                plural(self.shapes),
                if self.shapes == 1 { "" } else { "es" },
                plural(self.shapes)
            ));
        }
        out
    }
}

/// The objects of a sheet from its drawing and `OBJ`s.
pub(crate) fn sheet_objects(
    drawing: &[u8],
    objs: Vec<RawObj>,
    geometry: &Geometry,
    blips: &[Option<Blip>],
    ctx: &Context,
    skipped: &mut Skipped,
) -> Vec<Object> {
    let shapes = shapes(drawing);
    let mut out = Vec::new();
    for (shape, obj) in shapes.into_iter().zip(objs) {
        let ot = obj.ot.unwrap_or(u16::MAX);
        // Notes are read with their NOTE records.
        if ot == 0x19 {
            continue;
        }
        // Objects inside a group count as the group.
        if shape.grouped {
            continue;
        }
        let Some(anchor) = shape.anchor else {
            skipped.shapes += 1;
            continue;
        };
        let kind = match ot {
            0x08 => {
                let blip = shape.pib.and_then(|p| (p as usize).checked_sub(1));
                match blip.and_then(|b| blips.get(b)?.as_ref().map(|x| (b, x))) {
                    Some((b, Blip::Image { .. })) => ObjectKind::Picture {
                        blip: b,
                        crop: shape.crop,
                    },
                    _ => {
                        skipped.pictures += 1;
                        continue;
                    }
                }
            }
            0x05 => match obj.chart.as_deref().map(|c| parse_chart(c, ctx)) {
                Some(Ok(c)) => {
                    skipped.flat += usize::from(c.three_d);
                    ObjectKind::Chart(c)
                }
                _ => {
                    skipped.charts += 1;
                    continue;
                }
            },
            _ => {
                skipped.shapes += 1;
                continue;
            }
        };
        out.push(Object {
            anchor: geometry.anchor(&anchor),
            name: shape.name,
            descr: shape.descr,
            hidden: shape.hidden,
            kind,
        });
    }
    out
}

/// The records of the substream starting at `offset` (a chart sheet's),
/// from its `BOF` to its `EOF`.
pub(crate) fn substream(stream: &[u8], offset: usize) -> Vec<(u16, Vec<u8>)> {
    let mut rec = Records::at(stream, offset);
    let mut out = Vec::new();
    let mut depth = 0;
    while let Some((typ, d)) = rec.next() {
        out.push((typ, d.to_vec()));
        match typ {
            0x0809 => depth += 1,
            0x000A => {
                depth -= 1;
                if depth <= 0 {
                    break;
                }
            }
            _ if depth == 0 => break,
            _ => {}
        }
    }
    out
}

/// A chart, read from its substream.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Chart {
    /// The chart groups that have series, in drawing order.
    pub plots: Vec<Plot>,
    pub title: Option<String>,
    /// Titles of the category (or X) axis and the value axis.
    pub cat_title: Option<String>,
    pub val_title: Option<String>,
    /// `legendPos`; `None`: no legend.
    pub legend: Option<&'static str>,
    /// Major gridlines on the category and value axes.
    pub gridlines: [bool; 2],
    pub three_d: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Plot {
    pub kind: PlotKind,
    /// Each point its own colour (pie charts).
    pub varied: bool,
    pub series: Vec<Series>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum PlotKind {
    Bar {
        horizontal: bool,
        grouping: &'static str,
        gap: u16,
        overlap: i16,
    },
    Line {
        grouping: &'static str,
    },
    Area {
        grouping: &'static str,
    },
    /// `hole`: a doughnut's hole in percent (0: a pie).
    Pie {
        hole: u16,
        first_angle: u16,
    },
    Scatter,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Series {
    pub name: Option<Text>,
    /// The values (Y values of a scatter chart): a reference.
    pub values: String,
    /// The categories (X values of a scatter chart).
    pub cats: Option<String>,
    pub fill: Option<u32>,
    pub line: Option<u32>,
    pub no_line: bool,
    pub no_marker: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Text {
    Literal(String),
    Ref(String),
}

/// A series while its records are read.
#[derive(Default)]
struct RawSeries {
    s: Series,
    values: Option<String>,
    group: u16,
    /// A trendline or error bars (`SERPARENT`, `SERAUXTREND`...).
    aux: bool,
    /// Its formatting applies to the whole series (`DATAFORMAT` `xi`
    /// 0xFFFF).
    whole: bool,
}

/// A `TEXT` block while its records are read.
#[derive(Default)]
struct TextBlock {
    literal: Option<String>,
    link: u16,
}

/// Why a chart isn't brought in.
#[derive(Debug, PartialEq)]
pub(crate) struct Unsupported;

/// A `LongRGB` as `0xRRGGBB`.
fn rgb(b: &mut Bytes) -> Option<u32> {
    let c = b.take(4)?;
    Some(u32::from(c[0]) << 16 | u32::from(c[1]) << 8 | u32::from(c[2]))
}

/// A reference of a chart (`ChartParsedFormula`), absolute, as `c:f`
/// text.
fn chart_ref(d: &[u8], ctx: &Context) -> Option<String> {
    let mut b = Bytes::new(d);
    let cce = b.u16()? as usize;
    let rgce = absolute(b.take(cce)?);
    let f = decompile(&rgce, ctx)?;
    // A union is written in parentheses in `c:f`.
    Some(if f.contains(',') && !f.starts_with('(') {
        format!("({f})")
    } else {
        f
    })
}

/// Formula tokens with the relative flags of references cleared (a series
/// stores `Sheet1!B2:B5` that means `$B$2:$B$5`). Tokens other than
/// references, unions and parentheses are left as they are.
fn absolute(rgce: &[u8]) -> Vec<u8> {
    let mut out = rgce.to_vec();
    let mut pos = 0;
    let clear = |out: &mut Vec<u8>, at: usize| {
        if let Some(b) = out.get_mut(at + 1) {
            *b &= 0x3F;
        }
    };
    while pos < out.len() {
        let raw = out[pos];
        let ptg = if raw >= 0x20 {
            (raw & 0x1F) | 0x20
        } else {
            raw
        };
        match ptg {
            0x24 => {
                clear(&mut out, pos + 3);
                pos += 5;
            }
            0x25 => {
                clear(&mut out, pos + 5);
                clear(&mut out, pos + 7);
                pos += 9;
            }
            0x3A => {
                clear(&mut out, pos + 5);
                pos += 7;
            }
            0x3B => {
                clear(&mut out, pos + 7);
                clear(&mut out, pos + 9);
                pos += 11;
            }
            0x10 | 0x15 => pos += 1,
            _ => return rgce.to_vec(),
        }
    }
    out
}

const BEGIN: u16 = 0x1033;
const END: u16 = 0x1034;
const SERIES: u16 = 0x1003;
const TEXT: u16 = 0x1025;
const CHARTFORMAT: u16 = 0x1014;
const AXIS: u16 = 0x101D;
const DATAFORMAT: u16 = 0x1006;

/// A chart from the records of its substream.
pub(crate) fn parse_chart(recs: &[(u16, Vec<u8>)], ctx: &Context) -> Result<Chart, Unsupported> {
    let mut stack: Vec<u16> = Vec::new();
    let mut last = 0u16;
    let mut series: Vec<RawSeries> = Vec::new();
    // (icrt, kind, varied); `None` kind: a type Kantor doesn't draw.
    let mut groups: Vec<(u16, Option<PlotKind>, bool)> = Vec::new();
    let mut text: Option<TextBlock> = None;
    let mut axis: Option<u16> = None;
    let mut chart = Chart {
        plots: Vec::new(),
        title: None,
        cat_title: None,
        val_title: None,
        legend: None,
        gridlines: [false; 2],
        three_d: false,
    };
    let mut in_chart = false;
    for (typ, d) in recs {
        let mut b = Bytes::new(d);
        let top = stack.last().copied();
        let in_series = stack.contains(&SERIES);
        match *typ {
            BEGIN => {
                stack.push(last);
                continue;
            }
            END => {
                match stack.pop() {
                    Some(TEXT) => {
                        if let Some(t) = text.take() {
                            let lit = t.literal.filter(|s| !s.trim().is_empty());
                            match t.link {
                                1 => chart.title = lit,
                                2 => chart.val_title = lit,
                                3 => chart.cat_title = lit,
                                _ => {}
                            }
                        }
                    }
                    Some(AXIS) => axis = None,
                    _ => {}
                }
                continue;
            }
            0x1002 => in_chart = true,
            SERIES if stack.len() == 1 => series.push(RawSeries::default()),
            0x104A | 0x104B | 0x105B if in_series => {
                if let Some(s) = series.last_mut() {
                    s.aux = true;
                }
            }
            0x1051 => {
                // AI: id, rt, flags, ifmt, formula.
                let (Some(id), Some(rt)) = (b.u8(), b.u8()) else {
                    continue;
                };
                let formula = b.skip(4).and_then(|_| b.take(d.len().saturating_sub(6)));
                if top == Some(SERIES)
                    && let Some(s) = series.last_mut()
                    && rt == 2
                {
                    // Errors (`#N/A`, `#REF!`) stand for no reference.
                    let f = formula
                        .and_then(|f| chart_ref(f, ctx))
                        .filter(|f| !f.starts_with('#'));
                    match id {
                        0 => {
                            if let Some(f) = f {
                                s.s.name = Some(match f.strip_prefix('"') {
                                    Some(t) => Text::Literal(
                                        t.strip_suffix('"').unwrap_or(t).replace("\"\"", "\""),
                                    ),
                                    None => Text::Ref(f),
                                });
                            }
                        }
                        1 => s.values = f,
                        2 => s.s.cats = f,
                        _ => {}
                    }
                }
            }
            0x100D => {
                // SERIESTEXT: id, then a short string.
                let t = b.skip(2).and_then(|_| b.string8());
                if let Some(tb) = text.as_mut() {
                    tb.literal = t;
                } else if top == Some(SERIES)
                    && let Some(s) = series.last_mut()
                    && let Some(t) = t
                {
                    s.s.name = Some(Text::Literal(t));
                }
            }
            TEXT => text = Some(TextBlock::default()),
            0x1027 => {
                if let (Some(tb), Some(link)) = (text.as_mut(), b.u16()) {
                    tb.link = link;
                }
            }
            0x1045 if in_series => {
                if let (Some(s), Some(g)) = (series.last_mut(), b.u16()) {
                    s.group = g;
                }
            }
            DATAFORMAT if top == Some(SERIES) => {
                if let Some(s) = series.last_mut() {
                    s.whole = b.u16() == Some(0xFFFF);
                }
            }
            // AREAFORMAT, LINEFORMAT, MARKERFORMAT of a series.
            0x100A | 0x1007 | 0x1009 if top == Some(DATAFORMAT) && in_series => {
                let Some(s) = series.last_mut().filter(|s| s.whole) else {
                    continue;
                };
                match *typ {
                    0x100A => {
                        let r = (|| Some((rgb(&mut b)?, b.skip(4)?, b.u16()?, b.u16()?)))();
                        if let Some((fore, _, fls, flags)) = r
                            && flags & 1 == 0
                            && fls == 1
                        {
                            s.s.fill = Some(fore);
                        }
                    }
                    0x1007 => {
                        let r = (|| Some((rgb(&mut b)?, b.u16()?, b.u16()?, b.u16()?)))();
                        if let Some((c, lns, _, flags)) = r
                            && flags & 1 == 0
                        {
                            if lns == 5 {
                                s.s.no_line = true;
                            } else {
                                s.s.line = Some(c);
                            }
                        }
                    }
                    _ => {
                        let r = (|| Some((b.skip(8)?, b.u16()?, b.u16()?)))();
                        if let Some((_, imk, flags)) = r
                            && flags & 1 == 0
                            && imk == 0
                        {
                            s.s.no_marker = true;
                        }
                    }
                }
            }
            CHARTFORMAT => {
                let r = (|| Some((b.skip(16)?, b.u16()?, b.u16()?)))();
                let (flags, icrt) = r.map_or((0, groups.len() as u16), |(_, f, i)| (f, i));
                groups.push((icrt, None, flags & 1 != 0));
            }
            0x1017..=0x101B | 0x103E | 0x103F | 0x1040 | 0x1061 if top == Some(CHARTFORMAT) => {
                let Some(g) = groups.last_mut() else {
                    continue;
                };
                let grouping = |stacked: bool, pct: bool, clustered: &'static str| {
                    if pct {
                        "percentStacked"
                    } else if stacked {
                        "stacked"
                    } else {
                        clustered
                    }
                };
                g.1 = match *typ {
                    0x1017 => (|| {
                        let (overlap, gap, flags) = (b.i16()?, b.u16()?, b.u16()?);
                        Some(PlotKind::Bar {
                            horizontal: flags & 1 != 0,
                            grouping: grouping(flags & 2 != 0, flags & 4 != 0, "clustered"),
                            gap,
                            overlap: -overlap,
                        })
                    })(),
                    0x1018 | 0x101A => b.u16().map(|flags| {
                        let grouping = grouping(flags & 1 != 0, flags & 2 != 0, "standard");
                        if *typ == 0x1018 {
                            PlotKind::Line { grouping }
                        } else {
                            PlotKind::Area { grouping }
                        }
                    }),
                    0x1019 => (|| {
                        let (first_angle, hole) = (b.u16()?, b.u16()?);
                        Some(PlotKind::Pie {
                            hole: hole.min(90),
                            first_angle: first_angle % 360,
                        })
                    })(),
                    0x101B => {
                        let bubbles = b.skip(4).and_then(|_| b.u16()).is_some_and(|f| f & 1 != 0);
                        (!bubbles).then_some(PlotKind::Scatter)
                    }
                    _ => None,
                };
            }
            0x103A => chart.three_d = true,
            0x1015 => {
                chart.legend = Some(match b.skip(16).and_then(|_| b.u8()) {
                    Some(0) => "b",
                    Some(2) => "t",
                    Some(4) => "l",
                    _ => "r",
                });
            }
            AXIS => axis = b.u16(),
            // AXISLINEFORMAT: id 1 is the major gridlines.
            0x1021 if top == Some(AXIS) => {
                if b.u16() == Some(1)
                    && let Some(a @ 0..=1) = axis
                {
                    chart.gridlines[a as usize] = true;
                }
            }
            _ => {}
        }
        last = *typ;
    }
    if !in_chart || groups.is_empty() {
        return Err(Unsupported);
    }
    groups.sort_by_key(|g| g.0);
    for (icrt, kind, varied) in &groups {
        let list: Vec<Series> = series
            .iter()
            .filter(|s| s.group == *icrt && !s.aux)
            .filter_map(|s| {
                Some(Series {
                    values: s.values.clone()?,
                    ..s.s.clone()
                })
            })
            .collect();
        if list.is_empty() {
            continue;
        }
        chart.plots.push(Plot {
            kind: kind.ok_or(Unsupported)?,
            varied: *varied,
            series: list,
        });
    }
    // One plot, or bars, lines and areas on the same axes.
    let combinable = |p: &Plot| {
        matches!(
            p.kind,
            PlotKind::Line { .. }
                | PlotKind::Area { .. }
                | PlotKind::Bar {
                    horizontal: false,
                    ..
                }
        )
    };
    if chart.plots.is_empty() || chart.plots.len() > 1 && !chart.plots.iter().all(combinable) {
        return Err(Unsupported);
    }
    Ok(chart)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// `c:strRef` for `f` with the text its cells show now.
fn str_ref(f: &str, data: &dyn Fn(&str) -> Option<RefData>) -> String {
    let d = data(f).unwrap_or_default();
    let mut x = format!(
        "<c:strRef><c:f>{}</c:f><c:strCache><c:ptCount val=\"{}\"/>",
        esc(f),
        d.shown.len()
    );
    for (i, s) in d.shown.iter().enumerate() {
        x.push_str(&format!("<c:pt idx=\"{i}\"><c:v>{}</c:v></c:pt>", esc(s)));
    }
    x + "</c:strCache></c:strRef>"
}

/// `c:numRef` for `f` with the numbers its cells hold now.
fn num_ref(f: &str, data: &dyn Fn(&str) -> Option<RefData>) -> String {
    let d = data(f).unwrap_or_default();
    let fmt = d.format.clone().unwrap_or_else(|| "General".into());
    let mut x = format!(
        "<c:numRef><c:f>{}</c:f><c:numCache><c:formatCode>{}</c:formatCode><c:ptCount val=\"{}\"/>",
        esc(f),
        esc(&fmt),
        d.values.len()
    );
    for (i, v) in d.values.iter().enumerate() {
        if let RefValue::Number(n) = v {
            x.push_str(&format!("<c:pt idx=\"{i}\"><c:v>{n}</c:v></c:pt>"));
        }
    }
    x + "</c:numCache></c:numRef>"
}

fn rich(text: &str) -> String {
    format!(
        "<c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr/></a:pPr><a:r><a:t>{}</a:t></a:r></a:p></c:rich></c:tx><c:overlay val=\"0\"/>",
        esc(text)
    )
}

fn solid(c: u32) -> String {
    format!("<a:solidFill><a:srgbClr val=\"{c:06X}\"/></a:solidFill>")
}

/// The `c:chartSpace` part of `chart`, its references with the values
/// `data` reads for them.
pub(crate) fn chart_xml(chart: &Chart, data: &dyn Fn(&str) -> Option<RefData>) -> String {
    let mut plots = String::new();
    let mut idx = 0;
    let axes_ids = "<c:axId val=\"500000001\"/><c:axId val=\"500000002\"/>";
    let mut horizontal = false;
    let mut scatter = false;
    let mut pie = false;
    for p in &chart.plots {
        let mut sers = String::new();
        let is_pie = matches!(p.kind, PlotKind::Pie { .. });
        let lines = matches!(p.kind, PlotKind::Line { .. } | PlotKind::Scatter);
        for s in &p.series {
            sers.push_str(&format!(
                "<c:ser><c:idx val=\"{idx}\"/><c:order val=\"{idx}\"/>"
            ));
            idx += 1;
            match &s.name {
                Some(Text::Literal(t)) => {
                    sers.push_str(&format!("<c:tx><c:v>{}</c:v></c:tx>", esc(t)))
                }
                Some(Text::Ref(f)) => sers.push_str(&format!("<c:tx>{}</c:tx>", str_ref(f, data))),
                None => {}
            }
            let line = if s.no_line {
                Some("<a:ln w=\"28575\"><a:noFill/></a:ln>".to_owned())
            } else {
                s.line.filter(|_| lines).map(|c| {
                    format!(
                        "<a:ln w=\"28575\" cap=\"rnd\">{}<a:round/></a:ln>",
                        solid(c)
                    )
                })
            };
            let fill = s
                .fill
                .filter(|_| !lines && !(is_pie && p.varied))
                .map(solid);
            if fill.is_some() || line.is_some() {
                sers.push_str(&format!(
                    "<c:spPr>{}{}</c:spPr>",
                    fill.unwrap_or_default(),
                    line.unwrap_or_default()
                ));
            }
            if lines && s.no_marker {
                sers.push_str("<c:marker><c:symbol val=\"none\"/></c:marker>");
            }
            match p.kind {
                PlotKind::Scatter => {
                    if let Some(x) = &s.cats {
                        sers.push_str(&format!("<c:xVal>{}</c:xVal>", num_ref(x, data)));
                    }
                    sers.push_str(&format!(
                        "<c:yVal>{}</c:yVal><c:smooth val=\"0\"/>",
                        num_ref(&s.values, data)
                    ));
                }
                _ => {
                    if matches!(p.kind, PlotKind::Bar { .. }) {
                        sers.push_str("<c:invertIfNegative val=\"0\"/>");
                    }
                    if let Some(c) = &s.cats {
                        let numeric = data(c).is_some_and(|d| {
                            d.values.iter().any(|v| matches!(v, RefValue::Number(_)))
                                && d.values.iter().all(|v| !matches!(v, RefValue::Text(_)))
                        });
                        let r = if numeric {
                            num_ref(c, data)
                        } else {
                            str_ref(c, data)
                        };
                        sers.push_str(&format!("<c:cat>{r}</c:cat>"));
                    }
                    sers.push_str(&format!("<c:val>{}</c:val>", num_ref(&s.values, data)));
                    if matches!(p.kind, PlotKind::Line { .. }) {
                        sers.push_str("<c:smooth val=\"0\"/>");
                    }
                }
            }
            sers.push_str("</c:ser>");
        }
        let varied = u8::from(p.varied);
        plots.push_str(&match p.kind {
            PlotKind::Bar {
                horizontal: h,
                grouping,
                gap,
                overlap,
            } => {
                horizontal = h;
                let dir = if h { "bar" } else { "col" };
                let overlap = if grouping == "clustered" && overlap == 0 {
                    String::new()
                } else {
                    format!("<c:overlap val=\"{}\"/>", overlap.clamp(-100, 100))
                };
                format!(
                    "<c:barChart><c:barDir val=\"{dir}\"/><c:grouping val=\"{grouping}\"/><c:varyColors val=\"{varied}\"/>{sers}<c:gapWidth val=\"{}\"/>{overlap}{axes_ids}</c:barChart>",
                    gap.min(500)
                )
            }
            PlotKind::Line { grouping } => format!(
                "<c:lineChart><c:grouping val=\"{grouping}\"/><c:varyColors val=\"{varied}\"/>{sers}<c:marker val=\"1\"/>{axes_ids}</c:lineChart>"
            ),
            PlotKind::Area { grouping } => format!(
                "<c:areaChart><c:grouping val=\"{grouping}\"/><c:varyColors val=\"{varied}\"/>{sers}{axes_ids}</c:areaChart>"
            ),
            PlotKind::Pie { hole, first_angle } => {
                pie = true;
                if hole > 0 {
                    format!(
                        "<c:doughnutChart><c:varyColors val=\"{varied}\"/>{sers}<c:firstSliceAng val=\"{first_angle}\"/><c:holeSize val=\"{}\"/></c:doughnutChart>",
                        hole.max(10)
                    )
                } else {
                    format!(
                        "<c:pieChart><c:varyColors val=\"{varied}\"/>{sers}<c:firstSliceAng val=\"{first_angle}\"/></c:pieChart>"
                    )
                }
            }
            PlotKind::Scatter => {
                scatter = true;
                format!(
                    "<c:scatterChart><c:scatterStyle val=\"lineMarker\"/><c:varyColors val=\"{varied}\"/>{sers}{axes_ids}</c:scatterChart>"
                )
            }
        });
    }
    let title = |t: &Option<String>| {
        t.as_deref()
            .map(|t| format!("<c:title>{}</c:title>", rich(t)))
            .unwrap_or_default()
    };
    let grid = |on: bool| if on { "<c:majorGridlines/>" } else { "" };
    let tail = "<c:numFmt formatCode=\"General\" sourceLinked=\"1\"/><c:majorTickMark val=\"out\"/><c:minorTickMark val=\"none\"/><c:tickLblPos val=\"nextTo\"/>";
    let (cat_pos, val_pos) = if horizontal { ("l", "b") } else { ("b", "l") };
    let (cat_title, val_title) = (title(&chart.cat_title), title(&chart.val_title));
    let (cat_grid, val_grid) = (grid(chart.gridlines[0]), grid(chart.gridlines[1]));
    let axes = if pie {
        String::new()
    } else if scatter {
        format!(
            "<c:valAx><c:axId val=\"500000001\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"b\"/>{cat_grid}{cat_title}{tail}<c:crossAx val=\"500000002\"/><c:crosses val=\"autoZero\"/><c:crossBetween val=\"midCat\"/></c:valAx>\
<c:valAx><c:axId val=\"500000002\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"l\"/>{val_grid}{val_title}{tail}<c:crossAx val=\"500000001\"/><c:crosses val=\"autoZero\"/><c:crossBetween val=\"midCat\"/></c:valAx>"
        )
    } else {
        format!(
            "<c:catAx><c:axId val=\"500000001\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"{cat_pos}\"/>{cat_grid}{cat_title}{tail}<c:crossAx val=\"500000002\"/><c:crosses val=\"autoZero\"/><c:auto val=\"1\"/><c:lblAlgn val=\"ctr\"/><c:lblOffset val=\"100\"/><c:noMultiLvlLbl val=\"0\"/></c:catAx>\
<c:valAx><c:axId val=\"500000002\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"{val_pos}\"/>{val_grid}{val_title}{tail}<c:crossAx val=\"500000001\"/><c:crosses val=\"autoZero\"/><c:crossBetween val=\"between\"/></c:valAx>"
        )
    };
    let title = match &chart.title {
        Some(t) => format!(
            "<c:title>{}</c:title><c:autoTitleDeleted val=\"0\"/>",
            rich(t)
        ),
        None => "<c:autoTitleDeleted val=\"1\"/>".to_owned(),
    };
    let legend = chart
        .legend
        .map(|p| format!("<c:legend><c:legendPos val=\"{p}\"/><c:overlay val=\"0\"/></c:legend>"))
        .unwrap_or_default();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
<c:roundedCorners val=\"0\"/><c:chart>{title}<c:plotArea><c:layout/>{plots}{axes}</c:plotArea>{legend}<c:plotVisOnly val=\"1\"/><c:dispBlanksAs val=\"gap\"/></c:chart></c:chartSpace>"
    )
}

/// A chart sheet of the file.
pub(crate) struct ChartSheet {
    pub name: String,
    pub hidden: bool,
    pub chart: Chart,
}

/// Everything drawn in an `.xls`, by sheet name.
#[derive(Default)]
pub(crate) struct XlsDrawings {
    /// Worksheets with objects.
    pub sheets: Vec<(String, Vec<Object>)>,
    pub chart_sheets: Vec<ChartSheet>,
    /// Every sheet's name in tab order, and whether it's a chart sheet.
    pub order: Vec<(String, bool)>,
    /// The BLIP store.
    pub blips: Vec<Option<Blip>>,
}

impl XlsDrawings {
    pub(crate) fn is_empty(&self) -> bool {
        self.sheets.is_empty() && self.chart_sheets.is_empty()
    }
}

const NS_MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const NS_PKG: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const XDR: &str = "xmlns:xdr=\"http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"";

fn rels(list: &[(String, &str, String)]) -> String {
    let mut x = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"{NS_PKG}\">"
    );
    for (id, kind, target) in list {
        x.push_str(&format!(
            "<Relationship Id=\"{id}\" Type=\"{REL}/{kind}\" Target=\"{}\"/>",
            esc(target)
        ));
    }
    x + "</Relationships>"
}

/// A drawing part and what it links to, for `objects`.
struct DrawingPart {
    xml: String,
    rels: Vec<(String, &'static str, String)>,
}

/// The package parts being written.
#[derive(Default)]
struct Parts {
    files: Vec<(String, Vec<u8>)>,
    overrides: Vec<(String, &'static str)>,
    charts: usize,
    /// Media part by BLIP index (each picture written once).
    media: HashMap<usize, String>,
}

impl Parts {
    fn chart(&mut self, chart: &Chart, data: &dyn Fn(&str) -> Option<RefData>) -> String {
        self.charts += 1;
        let part = format!("xl/charts/chart{}.xml", self.charts);
        self.files
            .push((part.clone(), chart_xml(chart, data).into_bytes()));
        self.overrides.push((
            part.clone(),
            "application/vnd.openxmlformats-officedocument.drawingml.chart+xml",
        ));
        part
    }

    fn drawing(
        &mut self,
        objects: &[Object],
        blips: &[Option<Blip>],
        data: &dyn Fn(&str) -> Option<RefData>,
    ) -> DrawingPart {
        let mut xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<xdr:wsDr {XDR}>"
        );
        let mut rels = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            let id = i + 2;
            let rid = format!("rId{}", rels.len() + 1);
            let edit_as = match o.anchor {
                Anchor::TwoCell {
                    edit_as: EditAs::OneCell,
                    ..
                } => " editAs=\"oneCell\"",
                Anchor::TwoCell {
                    edit_as: EditAs::Absolute,
                    ..
                } => " editAs=\"absolute\"",
                _ => "",
            };
            let hidden = if o.hidden { " hidden=\"1\"" } else { "" };
            let descr = o
                .descr
                .as_deref()
                .map(|d| format!(" descr=\"{}\"", esc(d)))
                .unwrap_or_default();
            let body = match &o.kind {
                ObjectKind::Picture { blip, crop } => {
                    let Some(Some(Blip::Image { ext, data: bytes })) = blips.get(*blip) else {
                        continue;
                    };
                    let n = self.media.len() + 1;
                    let media = self
                        .media
                        .entry(*blip)
                        .or_insert_with(|| format!("xl/media/image{n}.{ext}"))
                        .clone();
                    if !self.files.iter().any(|(p, _)| *p == media) {
                        self.files.push((media.clone(), bytes.clone()));
                    }
                    rels.push((
                        rid.clone(),
                        "image",
                        format!("../media/{}", media.rsplit('/').next().unwrap_or_default()),
                    ));
                    let name = o
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("Picture {}", i + 1));
                    let src = if crop.iter().any(|c| *c != 0.0) {
                        let v = |f: f32| (f64::from(f) * 100_000.0).round() as i64;
                        format!(
                            "<a:srcRect l=\"{}\" t=\"{}\" r=\"{}\" b=\"{}\"/>",
                            v(crop[0]),
                            v(crop[1]),
                            v(crop[2]),
                            v(crop[3])
                        )
                    } else {
                        String::new()
                    };
                    format!(
                        "<xdr:pic><xdr:nvPicPr><xdr:cNvPr id=\"{id}\" name=\"{}\"{descr}{hidden}/><xdr:cNvPicPr><a:picLocks noChangeAspect=\"1\"/></xdr:cNvPicPr></xdr:nvPicPr><xdr:blipFill><a:blip xmlns:r=\"{NS_R}\" r:embed=\"{rid}\"/>{src}<a:stretch><a:fillRect/></a:stretch></xdr:blipFill><xdr:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></xdr:spPr></xdr:pic>",
                        esc(&name)
                    )
                }
                ObjectKind::Chart(chart) => {
                    let part = self.chart(chart, data);
                    rels.push((
                        rid.clone(),
                        "chart",
                        format!("../charts/{}", part.rsplit('/').next().unwrap_or_default()),
                    ));
                    let name = o.name.clone().unwrap_or_else(|| format!("Chart {}", i + 1));
                    frame(id, &name, &descr, hidden, &rid)
                }
            };
            xml.push_str(&format!(
                "<xdr:twoCellAnchor{edit_as}>{}{body}<xdr:clientData/></xdr:twoCellAnchor>",
                o.anchor.position_xml("xdr:")
            ));
        }
        xml.push_str("</xdr:wsDr>");
        DrawingPart { xml, rels }
    }

    /// Adds `d` as drawing number `n`; returns its part name.
    fn add_drawing(&mut self, n: usize, d: DrawingPart) -> String {
        let part = format!("xl/drawings/drawing{n}.xml");
        self.files.push((part.clone(), d.xml.into_bytes()));
        if !d.rels.is_empty() {
            self.files.push((
                format!("xl/drawings/_rels/drawing{n}.xml.rels"),
                rels(&d.rels).into_bytes(),
            ));
        }
        self.overrides.push((
            part.clone(),
            "application/vnd.openxmlformats-officedocument.drawing+xml",
        ));
        part
    }
}

fn frame(id: usize, name: &str, descr: &str, hidden: &str, rid: &str) -> String {
    format!(
        "<xdr:graphicFrame macro=\"\"><xdr:nvGraphicFramePr><xdr:cNvPr id=\"{id}\" name=\"{}\"{descr}{hidden}/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/></xdr:xfrm><a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"><c:chart xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" xmlns:r=\"{NS_R}\" r:id=\"{rid}\"/></a:graphicData></a:graphic></xdr:graphicFrame>",
        esc(name)
    )
}

/// The size of a chart sheet's chart (Excel's for a letter page), EMU.
const CHART_SHEET_SIZE: (i64, i64) = (8_670_925, 6_292_850);

/// The `.xlsx` package of `d`: a workbook listing every worksheet (by the
/// `sheetId` `sheet_id` gives its name; sheets it doesn't know are left
/// out) and chart sheet in tab order, each worksheet with its drawing.
/// `data` reads the cells of chart references for their caches.
pub(crate) fn package(
    d: &XlsDrawings,
    sheet_id: &dyn Fn(&str) -> Option<u32>,
    data: &dyn Fn(&str) -> Option<RefData>,
) -> Vec<u8> {
    let mut p = Parts::default();
    let mut sheets_xml = String::new();
    let mut wb_rels: Vec<(String, &str, String)> = Vec::new();
    let mut drawings = 0;
    let mut next_id = d
        .order
        .iter()
        .filter_map(|(n, chart)| (!chart).then(|| sheet_id(n)).flatten())
        .max()
        .unwrap_or(0)
        + 1;
    let (mut worksheets, mut chartsheets) = (0, 0);
    let mut chart_sheets = d.chart_sheets.iter();
    for (name, is_chart) in &d.order {
        let rid = format!("rId{}", wb_rels.len() + 1);
        let (id, part, state) = if *is_chart {
            let Some(cs) = chart_sheets.find(|c| c.name == *name) else {
                continue;
            };
            chartsheets += 1;
            drawings += 1;
            let (cx, cy) = CHART_SHEET_SIZE;
            let chart = p.chart(&cs.chart, data);
            let dxml = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<xdr:wsDr {XDR}><xdr:absoluteAnchor><xdr:pos x=\"0\" y=\"0\"/><xdr:ext cx=\"{cx}\" cy=\"{cy}\"/>{}<xdr:clientData/></xdr:absoluteAnchor></xdr:wsDr>",
                frame(2, "Chart 1", "", "", "rId1")
            );
            let dpart = p.add_drawing(
                drawings,
                DrawingPart {
                    xml: dxml,
                    rels: vec![(
                        "rId1".into(),
                        "chart",
                        format!("../charts/{}", chart.rsplit('/').next().unwrap_or_default()),
                    )],
                },
            );
            let part = format!("xl/chartsheets/sheet{chartsheets}.xml");
            p.files.push((
                part.clone(),
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<chartsheet xmlns=\"{NS_MAIN}\" xmlns:r=\"{NS_R}\"><sheetPr/><sheetViews><sheetView workbookViewId=\"0\"/></sheetViews><pageMargins left=\"0.7\" right=\"0.7\" top=\"0.75\" bottom=\"0.75\" header=\"0.3\" footer=\"0.3\"/><drawing r:id=\"rId1\"/></chartsheet>"
                )
                .into_bytes(),
            ));
            p.files.push((
                format!("xl/chartsheets/_rels/sheet{chartsheets}.xml.rels"),
                rels(&[(
                    "rId1".into(),
                    "drawing",
                    format!(
                        "../drawings/{}",
                        dpart.rsplit('/').next().unwrap_or_default()
                    ),
                )])
                .into_bytes(),
            ));
            p.overrides.push((
                part.clone(),
                "application/vnd.openxmlformats-officedocument.spreadsheetml.chartsheet+xml",
            ));
            wb_rels.push((
                rid.clone(),
                "chartsheet",
                format!("chartsheets/sheet{chartsheets}.xml"),
            ));
            let id = next_id;
            next_id += 1;
            (id, part, if cs.hidden { " state=\"hidden\"" } else { "" })
        } else {
            let Some(id) = sheet_id(name) else {
                continue;
            };
            worksheets += 1;
            let part = format!("xl/worksheets/sheet{worksheets}.xml");
            let objects = d
                .sheets
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, o)| o.as_slice())
                .unwrap_or_default();
            let mut body = String::from("<sheetData/>");
            if !objects.is_empty() {
                drawings += 1;
                let dp = p.drawing(objects, &d.blips, data);
                let dpart = p.add_drawing(drawings, dp);
                body.push_str("<drawing r:id=\"rId1\"/>");
                p.files.push((
                    format!("xl/worksheets/_rels/sheet{worksheets}.xml.rels"),
                    rels(&[(
                        "rId1".into(),
                        "drawing",
                        format!(
                            "../drawings/{}",
                            dpart.rsplit('/').next().unwrap_or_default()
                        ),
                    )])
                    .into_bytes(),
                ));
            }
            p.files.push((
                part.clone(),
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<worksheet xmlns=\"{NS_MAIN}\" xmlns:r=\"{NS_R}\">{body}</worksheet>"
                )
                .into_bytes(),
            ));
            p.overrides.push((
                part.clone(),
                "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml",
            ));
            wb_rels.push((
                rid.clone(),
                "worksheet",
                format!("worksheets/sheet{worksheets}.xml"),
            ));
            (id, part, "")
        };
        let _ = part;
        sheets_xml.push_str(&format!(
            "<sheet name=\"{}\" sheetId=\"{id}\"{state} r:id=\"{rid}\"/>",
            esc(name)
        ));
    }
    p.files.push((
        "xl/workbook.xml".into(),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<workbook xmlns=\"{NS_MAIN}\" xmlns:r=\"{NS_R}\"><sheets>{sheets_xml}</sheets></workbook>"
        )
        .into_bytes(),
    ));
    p.files.push((
        "xl/_rels/workbook.xml.rels".into(),
        rels(&wb_rels).into_bytes(),
    ));
    p.files.push((
        "_rels/.rels".into(),
        rels(&[("rId1".into(), "officeDocument", "xl/workbook.xml".into())]).into_bytes(),
    ));
    let mut types = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Default Extension=\"png\" ContentType=\"image/png\"/><Default Extension=\"jpeg\" ContentType=\"image/jpeg\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>",
    );
    for (part, kind) in &p.overrides {
        types.push_str(&format!(
            "<Override PartName=\"/{part}\" ContentType=\"{kind}\"/>"
        ));
    }
    types.push_str("</Types>");
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    let mut ok = zip.start_file("[Content_Types].xml", opts).is_ok()
        && zip.write_all(types.as_bytes()).is_ok();
    for (name, bytes) in &p.files {
        ok &= zip.start_file(name.as_str(), opts).is_ok() && zip.write_all(bytes).is_ok();
    }
    match zip.finish() {
        Ok(c) if ok => c.into_inner(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_pixels() {
        assert_eq!(width_px(DEFAULT_WIDTH), 64.0);
        assert_eq!(width_px(0.0), 0.0);
    }

    #[test]
    fn relative_flags_cleared() {
        // PtgArea3d Data!B2:B5 with relative rows and columns.
        let rgce = [0x3B, 0, 0, 1, 0, 4, 0, 0x01, 0xC0, 0x01, 0xC0];
        let sheets = ["Data".to_owned()];
        let xti = [Some((0, 0))];
        let ctx = Context {
            base: (0, 0),
            sheets: &sheets,
            xti: &xti,
            names: &[],
        };
        assert_eq!(
            decompile(&absolute(&rgce), &ctx).as_deref(),
            Some("Data!$B$2:$B$5")
        );
        let mut f = vec![11, 0];
        f.extend(rgce);
        assert_eq!(chart_ref(&f, &ctx).as_deref(), Some("Data!$B$2:$B$5"));
    }

    #[test]
    fn anchor_offsets() {
        let g = Geometry::default();
        // From B2 a quarter in, to D5 half way.
        let a = g.anchor(&[2, 1, 256, 1, 64, 3, 512, 4, 128]);
        let Anchor::TwoCell { from, to, edit_as } = a else {
            panic!("{a:?}")
        };
        assert_eq!(edit_as, EditAs::OneCell);
        assert_eq!((from.col, from.row), (1, 1));
        assert_eq!(from.col_off, (16.0 * EMU_PER_PX) as i64);
        assert_eq!(from.row_off, (5.0 * EMU_PER_PX) as i64);
        assert_eq!(to.col_off, (32.0 * EMU_PER_PX) as i64);
    }
}
