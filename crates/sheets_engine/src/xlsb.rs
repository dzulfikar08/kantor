//! The formatting of Excel binary workbooks (`.xlsb`, [MS-XLSB]) that
//! calamine leaves out: cell formats (number formats, fonts, fills,
//! borders, alignment, protection) from `xl/styles.bin`, and each sheet's
//! column widths, row heights, merged cells and default sizes from its
//! `.bin` part. Values and formulas come from calamine, as for `.ods`.
//!
//! Records are a variable-length type (7 bits a byte), a variable-length
//! size, then the data.

use std::collections::HashMap;
use std::io::Read as _;
use std::path::Path;

use anyhow::{Result, anyhow};
use ironcalc_base::Model;
use ironcalc_base::types::{
    Alignment, Border, BorderItem, CellProtection, Col, Color, Fill, Font, FontScheme,
    HorizontalAlignment, MergedCell, NumFmt, Row, Style, VerticalAlignment,
};

use crate::xls::{DEFAULT_PALETTE, FIXED_COLOURS, border_style, rgb_color};
use crate::{MAX_COLS, MAX_ROWS};

/// Records of a part.
struct Records<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Iterator for Records<'a> {
    type Item = (u32, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let mut var = |max: usize| -> Option<u32> {
            let mut v = 0u32;
            for i in 0..max {
                let byte = *self.b.get(self.at)?;
                self.at += 1;
                v |= ((byte & 0x7F) as u32) << (7 * i);
                if byte & 0x80 == 0 {
                    break;
                }
            }
            Some(v)
        };
        let ty = var(2)?;
        let len = var(4)? as usize;
        let start = self.at;
        let end = start.checked_add(len)?;
        let data = self.b.get(start..end)?;
        self.at = end;
        Some((ty, data))
    }
}

fn records(b: &[u8]) -> Records<'_> {
    Records { b, at: 0 }
}

fn u8_at(b: &[u8], o: usize) -> u8 {
    b.get(o).copied().unwrap_or(0)
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    b.get(o..o + 2)
        .map_or(0, |s| u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4)
        .map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// An `XLWideString` at `o`: its text and the offset after it.
fn wide(b: &[u8], o: usize) -> (String, usize) {
    let n = u32_at(b, o) as usize;
    let n = n.min(b.len().saturating_sub(o + 4) / 2);
    let units: Vec<u16> = (0..n).map(|i| u16_at(b, o + 4 + i * 2)).collect();
    (String::from_utf16_lossy(&units), o + 4 + n * 2)
}

/// A `BrtColor` (8 bytes) as RGB, `None` for automatic.
fn color(b: &[u8], o: usize, theme: &[u32]) -> Option<u32> {
    let kind = u8_at(b, o) >> 1;
    let index = u8_at(b, o + 1);
    let tint = (u16_at(b, o + 2) as i16) as f64 / 32767.0;
    let rgb = match kind {
        0 => return None,
        1 => match index {
            0..=7 => FIXED_COLOURS[index as usize],
            8..=63 => DEFAULT_PALETTE[index as usize - 8],
            _ => return None,
        },
        2 => (u8_at(b, o + 4) as u32) << 16 | (u8_at(b, o + 5) as u32) << 8 | u8_at(b, o + 6) as u32,
        3 => *theme.get(index as usize)?,
        _ => return None,
    };
    Some(if tint.abs() > 1e-6 { tinted(rgb, tint) } else { rgb })
}

/// Excel's tint: the colour's HLS lightness moved towards black or white.
fn tinted(rgb: u32, tint: f64) -> u32 {
    let (r, g, b) = (
        ((rgb >> 16) & 0xFF) as f64 / 255.0,
        ((rgb >> 8) & 0xFF) as f64 / 255.0,
        (rgb & 0xFF) as f64 / 255.0,
    );
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let (h, s) = if max == min {
        (0.0, 0.0)
    } else {
        let d = max - min;
        let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
        let h = if max == r {
            (g - b) / d + if g < b { 6.0 } else { 0.0 }
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        (h / 6.0, s)
    };
    let l = if tint < 0.0 { l * (1.0 + tint) } else { l * (1.0 - tint) + tint };
    let hue = |p: f64, q: f64, t: f64| {
        let t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    let (r, g, b) = if s == 0.0 {
        (l, l, l)
    } else {
        let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
        let p = 2.0 * l - q;
        (hue(p, q, h + 1.0 / 3.0), hue(p, q, h), hue(p, q, h - 1.0 / 3.0))
    };
    let c = |v: f64| ((v * 255.0).round().clamp(0.0, 255.0)) as u32;
    c(r) << 16 | c(g) << 8 | c(b)
}

/// The theme's colours in Excel's index order (lt1, dk1, lt2, dk2,
/// accents, links); Office's defaults when the part is missing.
fn theme_colors(xml: Option<&str>) -> Vec<u32> {
    let mut out = vec![
        0xFFFFFF, 0x000000, 0xE7E6E6, 0x44546A, 0x4472C4, 0xED7D31, 0xA5A5A5, 0xFFC000,
        0x5B9BD5, 0x70AD47, 0x0563C1, 0x954F72,
    ];
    let Some(doc) = xml.and_then(|x| roxmltree::Document::parse(x).ok()) else {
        return out;
    };
    let Some(scheme) = doc.descendants().find(|n| n.has_tag_name("clrScheme")) else {
        return out;
    };
    let order = [
        "lt1", "dk1", "lt2", "dk2", "accent1", "accent2", "accent3", "accent4", "accent5",
        "accent6", "hlink", "folHlink",
    ];
    for (i, name) in order.iter().enumerate() {
        let Some(el) = scheme.children().find(|n| n.tag_name().name() == *name) else {
            continue;
        };
        let value = el.children().find(|n| n.is_element()).and_then(|c| {
            c.attribute("val")
                .filter(|_| c.tag_name().name() == "srgbClr")
                .or_else(|| c.attribute("lastClr"))
        });
        if let Some(v) = value.and_then(|v| u32::from_str_radix(v, 16).ok()) {
            out[i] = v;
        }
    }
    out
}

struct Xf {
    fmt: u16,
    font: u16,
    fill: u16,
    border: u16,
    rotation: u8,
    indent: u8,
    flags: u16,
}

/// What `styles.bin` holds, and the model styles made from it so far.
struct Styles {
    formats: Vec<NumFmt>,
    fonts: Vec<Font>,
    fills: Vec<Color>,
    borders: Vec<Border>,
    xfs: Vec<Xf>,
    made: HashMap<u32, i32>,
}

impl Styles {
    fn read(bin: &[u8], theme: &[u32]) -> Self {
        let mut s = Styles {
            formats: Vec::new(),
            fonts: Vec::new(),
            fills: Vec::new(),
            borders: Vec::new(),
            xfs: Vec::new(),
            made: HashMap::new(),
        };
        let mut in_cell_xfs = false;
        for (ty, d) in records(bin) {
            match ty {
                // BrtFmt
                44 => s.formats.push(NumFmt {
                    num_fmt_id: u16_at(d, 0) as i32,
                    format_code: wide(d, 2).0,
                }),
                // BrtFont
                43 => {
                    let grbit = u16_at(d, 2);
                    s.fonts.push(Font {
                        strike: grbit & 0x8 != 0,
                        u: u8_at(d, 8) != 0,
                        b: u16_at(d, 4) >= 600,
                        i: grbit & 0x2 != 0,
                        sz: ((u16_at(d, 0) as f64 / 20.0).round() as i32).clamp(1, 409),
                        color: color(d, 12, theme).map_or(Color::None, rgb_color),
                        name: wide(d, 21).0,
                        family: (u8_at(d, 9) as i32).max(1),
                        scheme: FontScheme::None,
                    })
                }
                // BrtFill: a solid pattern keeps its foreground colour,
                // other patterns are mixed as `.xls` does.
                45 => {
                    let pattern = u32_at(d, 0);
                    let fore = color(d, 4, theme);
                    let back = color(d, 12, theme);
                    s.fills.push(fill_color(pattern, fore, back));
                }
                // BrtBorder: flags, then top, bottom, left, right, diagonal.
                46 => {
                    let flags = u8_at(d, 0);
                    let item = |o: usize| {
                        border_style(u8_at(d, o)).map(|style| BorderItem {
                            style,
                            color: color(d, o + 2, theme).map_or(Color::None, rgb_color),
                        })
                    };
                    let (down, up) = (flags & 1 != 0, flags & 2 != 0);
                    s.borders.push(Border {
                        top: item(1),
                        bottom: item(11),
                        left: item(21),
                        right: item(31),
                        diagonal: if down || up { item(41) } else { None },
                        diagonal_down: down,
                        diagonal_up: up,
                    });
                }
                617 => in_cell_xfs = true,
                618 => in_cell_xfs = false,
                47 if in_cell_xfs => s.xfs.push(Xf {
                    fmt: u16_at(d, 2),
                    font: u16_at(d, 4),
                    fill: u16_at(d, 6),
                    border: u16_at(d, 8),
                    rotation: u8_at(d, 10),
                    indent: u8_at(d, 11),
                    flags: u16_at(d, 12),
                }),
                _ => {}
            }
        }
        s
    }

    /// The model style of XF `ix` (0, the default, for XF 0 and unknown
    /// ones).
    fn get(&mut self, model: &mut Model<'_>, ix: u32) -> i32 {
        if ix == 0 {
            return 0;
        }
        if let Some(&s) = self.made.get(&ix) {
            return s;
        }
        let Some(xf) = self.xfs.get(ix as usize) else {
            return 0;
        };
        let style = self.convert(xf);
        let styles = &mut model.workbook.styles;
        let s = styles
            .get_style_index(&style)
            .unwrap_or_else(|| styles.create_new_style(&style));
        self.made.insert(ix, s);
        s
    }

    fn convert(&self, xf: &Xf) -> Style {
        let f = xf.flags;
        let horizontal = match f & 7 {
            1 => HorizontalAlignment::Left,
            2 => HorizontalAlignment::Center,
            3 => HorizontalAlignment::Right,
            4 => HorizontalAlignment::Fill,
            5 => HorizontalAlignment::Justify,
            6 => HorizontalAlignment::CenterContinuous,
            7 => HorizontalAlignment::Distributed,
            _ => HorizontalAlignment::General,
        };
        let vertical = match (f >> 3) & 7 {
            0 => VerticalAlignment::Top,
            1 => VerticalAlignment::Center,
            3 => VerticalAlignment::Justify,
            4 => VerticalAlignment::Distributed,
            _ => VerticalAlignment::Bottom,
        };
        let alignment = Alignment {
            horizontal,
            vertical,
            wrap_text: f & 0x40 != 0,
            indent: xf.indent as u32,
            text_rotation: match xf.rotation {
                r @ 0..=180 => r as u32,
                255 => 255,
                _ => 0,
            },
            shrink_to_fit: f & 0x100 != 0,
        };
        let (locked, hidden) = (f & 0x1000 != 0, f & 0x2000 != 0);
        Style {
            alignment: (alignment != Alignment::default()).then_some(alignment),
            num_fmt: ironcalc_base::number_format::get_num_fmt(xf.fmt as i32, &self.formats),
            fill: Fill {
                color: self.fills.get(xf.fill as usize).cloned().unwrap_or(Color::None),
            },
            font: self
                .fonts
                .get(xf.font as usize)
                .or(self.fonts.first())
                .cloned()
                .unwrap_or_default(),
            border: self.borders.get(xf.border as usize).cloned().unwrap_or_default(),
            quote_prefix: f & 0x8000 != 0,
            protection: (!locked || hidden).then_some(CellProtection { locked, hidden }),
        }
    }
}

/// A fill as one colour (see `xls::fill_color`): patterns other than solid
/// mix the foreground into the background by how much they cover.
fn fill_color(pattern: u32, fore: Option<u32>, back: Option<u32>) -> Color {
    let density = match pattern {
        0 => return Color::None,
        1 => return fore.map_or(Color::None, rgb_color),
        2 => 0.5,
        3 => 0.75,
        4 => 0.25,
        5..=10 => 0.5,
        11..=16 => 0.25,
        17 => 0.125,
        18 => 0.0625,
        _ => 0.5,
    };
    let (f, b) = (fore.unwrap_or(0), back.unwrap_or(0xFFFFFF));
    let mix = |shift: u32| {
        let (f, b) = (((f >> shift) & 0xFF) as f64, ((b >> shift) & 0xFF) as f64);
        ((f * density + b * (1.0 - density)).round() as u32).min(255) << shift
    };
    rgb_color(mix(16) | mix(8) | mix(0))
}

fn part(zip: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Option<Vec<u8>> {
    let mut f = zip.by_name(name).ok()?;
    let mut out = Vec::new();
    f.read_to_end(&mut out).ok()?;
    Some(out)
}

/// The workbook's sheets in order: name and part name.
fn sheet_parts(zip: &mut zip::ZipArchive<std::fs::File>) -> Vec<(String, String)> {
    let rels = part(zip, "xl/_rels/workbook.bin.rels")
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default();
    let mut targets = HashMap::new();
    if let Ok(doc) = roxmltree::Document::parse(&rels) {
        for r in doc.descendants().filter(|n| n.has_tag_name("Relationship")) {
            if let (Some(id), Some(t)) = (r.attribute("Id"), r.attribute("Target")) {
                let t = t.trim_start_matches('/');
                let t = if t.starts_with("xl/") { t.to_owned() } else { format!("xl/{t}") };
                targets.insert(id.to_owned(), t);
            }
        }
    }
    let Some(wb) = part(zip, "xl/workbook.bin") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (ty, d) in records(&wb) {
        // BrtBundleSh: hsState, iTabID, strRelID, strName.
        if ty == 156 {
            let (rel, at) = wide(d, 8);
            let (name, _) = wide(d, at);
            if let Some(t) = targets.get(&rel) {
                out.push((name, t.clone()));
            }
        }
    }
    out
}

/// Apply an `.xlsb`'s formatting to `model`, whose sheets calamine filled
/// in `names` order. Returns whether covered cells of merges were cleared
/// (the model needs evaluating again).
pub(crate) fn apply(path: &Path, model: &mut Model<'_>, names: &[String]) -> Result<bool> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| anyhow!("{e}"))?;
    let theme = theme_colors(
        part(&mut zip, "xl/theme/theme1.xml")
            .and_then(|b| String::from_utf8(b).ok())
            .as_deref(),
    );
    let styles_bin = part(&mut zip, "xl/styles.bin").unwrap_or_default();
    let mut styles = Styles::read(&styles_bin, &theme);
    // The Normal style's font is the workbook's default.
    if let Some(f) = styles.fonts.first().cloned()
        && let Some(slot) = model.workbook.styles.fonts.first_mut()
    {
        *slot = f;
    }
    let mut cleared = false;
    for (name, part_name) in sheet_parts(&mut zip) {
        let Some(sheet) = names.iter().position(|n| *n == name) else {
            continue;
        };
        let Some(bin) = part(&mut zip, &part_name) else {
            continue;
        };
        cleared |= apply_sheet(&bin, sheet as u32, model, &mut styles);
    }
    Ok(cleared)
}

fn apply_sheet(bin: &[u8], sheet: u32, model: &mut Model<'_>, styles: &mut Styles) -> bool {
    let mut cols: Vec<Col> = Vec::new();
    let mut rows: Vec<Row> = Vec::new();
    let mut merges: Vec<MergedCell> = Vec::new();
    let mut cells: Vec<(u32, u32, u32)> = Vec::new();
    let mut default_height = None;
    let mut row = 0u32;
    for (ty, d) in records(bin) {
        match ty {
            // BrtRowHdr: rw, ixfe, miyRw, flags.
            0 => {
                row = u32_at(d, 0);
                let ixfe = u32_at(d, 4);
                let height = u16_at(d, 8) as f64 / 20.0;
                let flags = u8_at(d, 11);
                let custom_format = flags & 0x40 != 0;
                if row < MAX_ROWS {
                    rows.push(Row {
                        r: row as i32 + 1,
                        height,
                        custom_format,
                        custom_height: flags & 0x20 != 0,
                        s: if custom_format { ixfe as i32 } else { 0 },
                        hidden: flags & 0x10 != 0,
                        outline_level: flags & 7,
                        collapsed: flags & 0x08 != 0,
                    });
                }
            }
            // Cells: a `Cell` (column, then the XF in 24 bits) first.
            1..=11 | 62 => {
                let col = u32_at(d, 0);
                let ixfe = u32_at(d, 4) & 0x00FF_FFFF;
                if ixfe != 0 && row < MAX_ROWS && col < MAX_COLS {
                    cells.push((row, col, ixfe));
                }
            }
            // BrtColInfo: first, last, width (1/256 char), ixfe, flags.
            60 => {
                let (first, last) = (u32_at(d, 0), u32_at(d, 4).min(MAX_COLS - 1));
                let flags = u16_at(d, 16);
                if first <= last {
                    let ixfe = u32_at(d, 12);
                    cols.push(Col {
                        min: first as i32 + 1,
                        max: last as i32 + 1,
                        width: u32_at(d, 8) as f64 / 256.0,
                        custom_width: true,
                        hidden: flags & 0x1 != 0,
                        style: (ixfe != 0).then_some(ixfe as i32),
                        outline_level: ((flags >> 8) & 7) as u8,
                        collapsed: flags & 0x1000 != 0,
                    });
                }
            }
            // BrtMergeCell: rwFirst, rwLast, colFirst, colLast.
            176 => {
                let (r1, r2, c1, c2) = (u32_at(d, 0), u32_at(d, 4), u32_at(d, 8), u32_at(d, 12));
                if r2 >= r1 && c2 >= c1 && (r1, c1) != (r2, c2) && r2 < MAX_ROWS && c2 < MAX_COLS {
                    let m = MergedCell {
                        row: r1 as i32 + 1,
                        column: c1 as i32 + 1,
                        width: (c2 - c1) as i32 + 1,
                        height: (r2 - r1) as i32 + 1,
                    };
                    if !merges
                        .iter()
                        .any(|o| o.intersects(m.row, m.column, m.width, m.height))
                    {
                        merges.push(m);
                    }
                }
            }
            // BrtWsFmtInfo: dxGCol, cchDefColWidth, miyDefRwHeight.
            485 => {
                let h = u16_at(d, 6) as f64 / 20.0;
                if h > 0.0 {
                    default_height = Some(h);
                }
            }
            _ => {}
        }
    }
    for (r, c, ixfe) in cells {
        let s = styles.get(model, ixfe);
        if s != 0
            && let Ok(ws) = model.workbook.worksheet_mut(sheet)
        {
            let _ = ws.set_cell_style(r as i32 + 1, c as i32 + 1, s);
        }
    }
    for c in &mut cols {
        c.style = c.style.map(|ix| styles.get(model, ix as u32)).filter(|&s| s != 0);
    }
    for r in &mut rows {
        if r.s != 0 {
            r.s = styles.get(model, r.s as u32);
        }
    }
    let Ok(ws) = model.workbook.worksheet_mut(sheet) else {
        return false;
    };
    cols.sort_by_key(|c| c.min);
    let mut kept: Vec<Col> = Vec::with_capacity(cols.len());
    for c in cols {
        if kept.last().is_some_and(|k| k.max >= c.min) {
            continue;
        }
        kept.push(c);
    }
    ws.cols = kept;
    if let Some(h) = default_height {
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
    let mut cleared = false;
    for m in &merges {
        for r in m.row..=m.last_row() {
            for c in m.column..=m.last_column() {
                if (r, c) != (m.row, m.column)
                    && ws.cell(r, c).is_some_and(|cell| {
                        !matches!(*cell, ironcalc_base::types::Cell::EmptyCell { .. })
                    })
                {
                    cleared |= ws.cell_clear_contents(r, c).is_ok();
                }
            }
        }
    }
    ws.merged_cells = merges;
    cleared
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_tints() {
        // Type 0x8C 0x01 = 140, size 2.
        let b = [0x8C, 0x01, 0x02, 0xAA, 0xBB, 0x00, 0x00];
        let v: Vec<(u32, Vec<u8>)> = records(&b).map(|(t, d)| (t, d.to_vec())).collect();
        assert_eq!(v, [(140, vec![0xAA, 0xBB]), (0, vec![])]);
        assert_eq!(tinted(0x4472C4, 0.0), 0x4472C4);
        assert_eq!(tinted(0x000000, 0.5), 0x808080);
        assert_eq!(tinted(0xFFFFFF, -0.5), 0x808080);
        assert_eq!(theme_colors(None)[4], 0x4472C4);
    }
}
