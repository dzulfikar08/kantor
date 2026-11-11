//! A page's cells as plain data: where each row and column goes, fills,
//! borders, gridlines, headings, conditional formatting and the text of
//! every cell with how to place it. Reads the workbook the way the grid
//! does (`sheets_ui`'s cell rendering), with Excel's printing rules where
//! they differ: automatic text is black, gridlines only with "Print
//! gridlines", text overflows into empty neighbours but not past the page.

use std::collections::HashSet;
use std::sync::Arc;

use sheets_engine::{
    AlignDetail, BorderLine, CellRange, CellStyle, HAlign, PrintColumns, TableCellLook, VAlign,
    ValueKind, Workbook,
};

use crate::text::{FontKey, Shaper};
use crate::{
    Align, Bar, CellText, HfLayout, INCH, Icon, Line, LineKind, PageContent, Plan, SheetPlan, hf,
};

/// Space between a cell's edges and its text, points.
pub(crate) const PAD_X: f32 = 2.0;
/// Room above and below text in an automatic-height row, points.
const PAD_Y: f32 = 1.6;

/// A row or column on the page.
#[derive(Debug, Clone, Copy)]
struct Pos {
    index: u32,
    start: f32,
    size: f32,
    /// 0: repeated titles, 1: the page's own rows / columns.
    seg: u8,
}

/// The CSS family list for a cell font: the font itself, its metric
/// compatible substitute, then common fallbacks.
pub(crate) fn family(name: &str) -> Arc<str> {
    let mut list = String::new();
    if !name.is_empty() {
        list.push_str(name);
        list.push_str(", ");
    }
    if let Some(s) = kantor_core::fonts::metric_substitute(name) {
        list.push_str(s);
        list.push_str(", ");
    }
    list.push_str("Carlito, Liberation Sans, Noto Sans, sans-serif");
    Arc::from(list)
}

/// The rows / columns of a page: repeated titles (when the page doesn't
/// already start inside or above them), then its band.
fn positions(
    band: (u32, u32),
    titles: Option<(u32, u32)>,
    start: f32,
    size: &dyn Fn(u32) -> f64,
) -> Vec<Pos> {
    let mut out = Vec::new();
    let mut at = start;
    if let Some((a, b)) = titles.filter(|t| band.0 > t.1) {
        for i in a..=b {
            let s = size(i) as f32;
            out.push(Pos {
                index: i,
                start: at,
                size: s,
                seg: 0,
            });
            at += s;
        }
    }
    for i in band.0..=band.1 {
        let s = size(i) as f32;
        out.push(Pos {
            index: i,
            start: at,
            size: s,
            seg: 1,
        });
        at += s;
    }
    out
}

/// `[start, end)` of each segment of positions.
fn segments(pos: &[Pos]) -> [(f32, f32); 2] {
    let mut seg = [(f32::MAX, f32::MIN); 2];
    for p in pos {
        let s = &mut seg[p.seg as usize];
        s.0 = s.0.min(p.start);
        s.1 = s.1.max(p.start + p.size);
    }
    seg
}

/// Start and size of the span `first..=last` (which may reach off the
/// page), measured from a row / column `anchor` on the page.
fn span(
    pos: &[Pos],
    anchor: usize,
    first: u32,
    last: u32,
    size: &dyn Fn(u32) -> f64,
) -> (f32, f32) {
    let p = pos[anchor];
    let before: f64 = (first..p.index).map(size).sum();
    let total: f64 = (first..=last).map(size).sum();
    (p.start - before as f32, total as f32)
}

fn intersect(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let x0 = a[0].max(b[0]);
    let y0 = a[1].max(b[1]);
    let x1 = (a[0] + a[2]).min(b[0] + b[2]);
    let y1 = (a[1] + a[3]).min(b[1] + b[3]);
    [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)]
}

/// A cell's style with a table style's look under it: the cell's own fill,
/// text colour and borders win, bold adds up (as the grid draws it).
fn table_style(style: &CellStyle, tl: &TableCellLook) -> CellStyle {
    let mut s = style.clone();
    if s.fill.is_none() {
        s.fill = tl.fill;
    }
    if s.font_color.is_none() {
        s.font_color = tl.font_color;
    }
    s.bold |= tl.bold;
    s
}

/// One cell of a page row, read from the workbook.
struct Cell {
    col: usize,
    text: String,
    style: Arc<CellStyle>,
    merge: Option<CellRange>,
    color: Option<u32>,
    repeat: Option<(char, usize)>,
    kind: ValueKind,
    halign: HAlign,
    bar: Option<sheets_engine::DataBar>,
    icon: Option<sheets_engine::CfIcon>,
    borders: [Option<BorderLine>; 4],
}

/// Rows without a stored height grow to fit wrapped text and larger fonts,
/// as Excel fits them when it opens the file.
pub(crate) fn fit_rows(
    book: &Workbook,
    sheet: u32,
    columns: &PrintColumns,
    rows: &mut [f64],
    stored: &[bool],
    areas: &[CellRange],
    shaper: &mut Shaper,
) {
    let default_name = book.default_font_name();
    let font_of = |s: &CellStyle| {
        FontKey::new(
            family(s.font_name.as_deref().unwrap_or(&default_name)),
            s.font_size.max(1) as f32,
            s.bold,
            s.italic,
        )
    };
    // Turned text: its height when turned (Excel fits rows to it too).
    if stored.iter().any(|s| !s) {
        for (r, c) in book.filled_cells(sheet) {
            let ri = r as usize;
            if ri >= rows.len() || stored[ri] {
                continue;
            }
            let s = book.cell_style(sheet, r, c);
            if s.rotation == 0 || book.merge_at(sheet, r, c).is_some() {
                continue;
            }
            let text = book.display(sheet, r, c).text;
            if text.is_empty() {
                continue;
            }
            let font = font_of(&s);
            let need = if s.rotation == 255 {
                let lh = shaper.line("A", &font).height;
                lh * text.chars().filter(|c| !c.is_control()).count() as f32
            } else {
                let line = shaper.line(&text, &font);
                let deg = if s.rotation <= 90 {
                    s.rotation as f32
                } else {
                    (s.rotation.min(180) - 90) as f32
                };
                let (sin, cos) = deg.to_radians().sin_cos();
                line.width * sin.abs() + (line.ascent + line.descent) * cos.abs()
            } + 2.0 * PAD_Y;
            rows[ri] = rows[ri].max(need.min(409.0) as f64);
        }
    }
    for (r, h) in rows.iter_mut().enumerate() {
        if stored[r]
            || !areas
                .iter()
                .any(|a| a.top as usize <= r && r <= a.bottom as usize)
        {
            continue;
        }
        for cell in book.row_fit_cells(sheet, r as u32) {
            let s = &cell.style;
            let font = font_of(s);
            let need = if s.wrap && s.rotation == 0 {
                let w = columns.width_pt(cell.col) as f32 - 2.0 * PAD_X;
                shaper.wrapped(&cell.text, &font, w, false).height
            } else {
                shaper.line(&cell.text, &font).height
            } + PAD_Y;
            *h = h.max(need.min(409.0) as f64);
        }
    }
}

/// Drawings on a page: none yet. This is where charts and pictures anchored
/// to the sheet would be found and placed (see [`crate::Overlay`]).
fn overlays(
    _book: &Workbook,
    _sp: &SheetPlan,
    _cols: &[Pos],
    _rows: &[Pos],
) -> Vec<crate::Overlay> {
    Vec::new()
}

/// The content of page `index` of `plan`.
pub fn extract(book: &mut Workbook, plan: &Plan, index: usize) -> PageContent {
    let pr = plan.pages[index];
    let sp = &plan.sheets[pr.sheet];
    let (_, grid) = &sp.areas[pr.area];
    let sheet = sp.sheet;
    let p = &sp.print;
    let bw = p.setup.black_and_white;
    let (hw, hh) = (sp.heading.0 as f32, sp.heading.1 as f32);
    let col_w = |c: u32| sp.col_width(c);
    let row_h = |r: u32| sp.row_height(r);
    let cols = positions(grid.cols[pr.cols], p.title_cols, hw, &col_w);
    let rows = positions(grid.rows[pr.rows], p.title_rows, hh, &row_h);
    let block_w = cols.last().map_or(hw, |c| c.start + c.size);
    let block_h = rows.last().map_or(hh, |r| r.start + r.size);
    let xseg = segments(&cols);
    let yseg = segments(&rows);

    let scale = grid.scale as f32;
    let m = p.margins;
    let (pw, ph) = (sp.page.0 as f32, sp.page.1 as f32);
    let left = (m.left * INCH) as f32;
    let top = (m.top * INCH) as f32;
    let avail_w = pw - left - (m.right * INCH) as f32;
    let avail_h = ph - top - (m.bottom * INCH) as f32;
    let mut origin = [left, top];
    if p.options.horizontal_centered {
        origin[0] += ((avail_w - block_w * scale) / 2.0).max(0.0);
    }
    if p.options.vertical_centered {
        origin[1] += ((avail_h - block_h * scale) / 2.0).max(0.0);
    }

    let default_name = book.default_font_name();
    let default_size = book.default_font_size() as f32;
    let default_color = book.default_font_color();
    let default_family = family(&default_name);
    let indent_step = (sp.columns.digit_px * 10.0 / 7.0 * 0.75) as f32;

    let mut out = PageContent {
        width: pw,
        height: ph,
        origin,
        scale,
        block: [block_w, block_h],
        ..PageContent::default()
    };
    let mut grid_lines = Vec::new();
    let mut borders = Vec::new();
    let mut merges_done: HashSet<(u32, u32)> = HashSet::new();
    let print_grid = p.options.grid_lines;

    for (ri, rp) in rows.iter().enumerate() {
        if rp.size <= 0.0 {
            continue;
        }
        let r = rp.index;
        let first_row = rows[..ri].iter().all(|q| q.seg != rp.seg || q.size <= 0.0);
        // Read the row's cells.
        let mut cells: Vec<Cell> = Vec::with_capacity(cols.len());
        for (ci, cp) in cols.iter().enumerate() {
            if cp.size <= 0.0 {
                continue;
            }
            let c = cp.index;
            let mut look = book.cell_look(sheet, r, c);
            let (vr, vc) = look.merge.map_or((r, c), |mg| (mg.top, mg.left));
            let shown = book.display(sheet, vr, vc);
            let cond = book.conditional(sheet, vr, vc);
            let shown = cond
                .as_ref()
                .and_then(|x| x.display.clone())
                .unwrap_or(shown);
            let text = if cond.as_ref().is_some_and(|x| !x.show_value) {
                String::new()
            } else {
                shown.text
            };
            let mut style = cond
                .as_ref()
                .and_then(|x| x.style.clone())
                .unwrap_or_else(|| look.style.clone());
            if let Some(tl) = book.table_cell_look(sheet, vr, vc) {
                style = Arc::new(table_style(&style, &tl));
                look.right = look.right.or(tl.right);
                look.bottom = look.bottom.or(tl.bottom);
                look.top = look.top.or(tl.top);
                look.left = look.left.or(tl.left);
            }
            let inside = |rr: u32, cc: u32| look.merge.is_some_and(|mg| mg.contains(rr, cc));
            // Top and left edges: the sheet's first row / column draw their
            // own; elsewhere the page's first row / column shares them with
            // the cell above / to the left.
            let own = if look.merge.is_some() {
                book.cell_style(sheet, r, c).borders
            } else {
                style.borders
            };
            let first_col = cols[..ci].iter().all(|q| q.seg != cp.seg || q.size <= 0.0);
            let top = if look.top.is_some() || r == 0 {
                look.top.or(own.top)
            } else if first_row && !inside(r - 1, c) {
                BorderLine::shared(own.top, book.cell_style(sheet, r - 1, c).borders.bottom)
            } else {
                None
            };
            let left = if look.left.is_some() || c == 0 {
                look.left.or(own.left)
            } else if first_col && !inside(r, c - 1) {
                BorderLine::shared(own.left, book.cell_style(sheet, r, c - 1).borders.right)
            } else {
                None
            };
            let kind = book.value_kind(sheet, vr, vc);
            let halign = book.display_align_of(&style, sheet, vr, vc);
            cells.push(Cell {
                col: ci,
                text,
                color: shown.color,
                repeat: shown.fill,
                merge: look.merge,
                kind,
                halign,
                bar: cond.as_ref().and_then(|x| x.bar),
                icon: cond.as_ref().and_then(|x| x.icon),
                borders: [top, look.right, look.bottom, left],
                style,
            });
        }

        // Fills, run-length merged along the row.
        if !bw {
            let mut run: Option<([f32; 4], u32)> = None;
            for cell in &cells {
                let cp = cols[cell.col];
                let Some(fill) = cell.style.fill else {
                    if let Some(f) = run.take() {
                        out.fills.push(f);
                    }
                    continue;
                };
                let rect = [cp.start, rp.start, cp.size, rp.size];
                match &mut run {
                    Some((rr, color))
                        if *color == fill && (rr[0] + rr[2] - rect[0]).abs() < 0.01 =>
                    {
                        rr[2] += rect[2];
                    }
                    _ => {
                        if let Some(f) = run.take() {
                            out.fills.push(f);
                        }
                        run = Some((rect, fill));
                    }
                }
            }
            if let Some(f) = run.take() {
                out.fills.push(f);
            }
        }

        // Borders and gridlines.
        for cell in &cells {
            let cp = cols[cell.col];
            let (x0, y0) = (cp.start, rp.start);
            let (x1, y1) = (x0 + cp.size, y0 + rp.size);
            let [t, rgt, b, l] = cell.borders;
            let edge =
                |line: Option<BorderLine>, from: [f32; 2], to: [f32; 2], v: &mut Vec<Line>| {
                    if let Some(bl) = line {
                        v.push(Line {
                            from,
                            to,
                            style: LineKind::Border(bl.style),
                            color: if bw { 0 } else { bl.color.unwrap_or(0) },
                        });
                    }
                };
            edge(t, [x0, y0], [x1, y0], &mut borders);
            edge(b, [x0, y1], [x1, y1], &mut borders);
            edge(l, [x0, y0], [x0, y1], &mut borders);
            edge(rgt, [x1, y0], [x1, y1], &mut borders);
            if print_grid {
                let inside = |rr: u32, cc: u32| cell.merge.is_some_and(|mg| mg.contains(rr, cc));
                let mut g = |from, to| {
                    grid_lines.push(Line {
                        from,
                        to,
                        style: LineKind::Grid,
                        color: 0,
                    })
                };
                if !inside(rp.index, cp.index + 1) {
                    g([x1, y0], [x1, y1]);
                }
                if !inside(rp.index + 1, cp.index) {
                    g([x0, y1], [x1, y1]);
                }
                if first_row {
                    g([x0, y0], [x1, y0]);
                }
                let first_col = cols[..cell.col]
                    .iter()
                    .all(|q| q.seg != cp.seg || q.size <= 0.0);
                if first_col {
                    g([x0, y0], [x0, y1]);
                }
            }
        }

        // Data bars and icons (not across merged cells, as on screen).
        if !bw {
            for cell in cells.iter().filter(|c| c.merge.is_none()) {
                let cp = cols[cell.col];
                let rect = [cp.start, rp.start, cp.size, rp.size];
                if let Some(bar) = cell.bar {
                    out.bars.push(Bar { rect, bar });
                }
                if let Some(icon) = cell.icon {
                    out.icons.push(Icon { rect, icon });
                }
            }
        }

        // Text.
        for (k, cell) in cells.iter().enumerate() {
            let cp = cols[cell.col];
            let s = &cell.style;
            let seg_rect = [
                xseg[cp.seg as usize].0,
                yseg[rp.seg as usize].0,
                xseg[cp.seg as usize].1 - xseg[cp.seg as usize].0,
                yseg[rp.seg as usize].1 - yseg[rp.seg as usize].0,
            ];
            let (rect, clip) = match cell.merge {
                Some(mg) => {
                    // A merged block is drawn once, from its first cell on
                    // the page.
                    if !merges_done.insert((mg.top, mg.left)) {
                        continue;
                    }
                    let (x, w) = span(&cols, cell.col, mg.left, mg.right, &col_w);
                    let (y, h) = span(&rows, ri, mg.top, mg.bottom, &row_h);
                    let rect = [x, y, w, h];
                    (rect, intersect(rect, seg_rect))
                }
                None => {
                    let rect = [cp.start, rp.start, cp.size, rp.size];
                    (rect, rect)
                }
            };
            if cell.text.is_empty() {
                continue;
            }
            let number = cell.kind == ValueKind::Number;
            let mut align = match s.align_detail {
                AlignDetail::Fill => Align::Fill,
                AlignDetail::Justify | AlignDetail::Distributed => Align::Justify,
                _ => match cell.halign {
                    HAlign::Center => Align::Center,
                    HAlign::Right => Align::Right,
                    _ => Align::Left,
                },
            };
            let wrap = s.wrap || align == Align::Justify;
            let mut rect = rect;
            let mut clip = clip;
            let empty = |c: &Cell| c.text.is_empty() && c.merge.is_none();
            let same_seg = |c: &Cell| cols[c.col].seg == cp.seg;
            if cell.merge.is_none() && s.align_detail == AlignDetail::CenterAcross {
                // Centre across the empty cells to the right that are
                // centred across too.
                let mut end = rect[0] + rect[2];
                for n in cells[k + 1..].iter().take_while(|n| {
                    same_seg(n) && empty(n) && n.style.align_detail == AlignDetail::CenterAcross
                }) {
                    let q = cols[n.col];
                    end = q.start + q.size;
                }
                rect[2] = end - rect[0];
                clip = rect;
                align = Align::Center;
            } else if cell.merge.is_none()
                && !wrap
                && s.rotation == 0
                && !number
                && align != Align::Fill
            {
                // Text overflows into empty neighbours (not past the page).
                let mut x0 = rect[0];
                let mut x1 = rect[0] + rect[2];
                if matches!(align, Align::Left | Align::Center) {
                    for n in cells[k + 1..]
                        .iter()
                        .take_while(|n| same_seg(n) && empty(n))
                    {
                        let q = cols[n.col];
                        x1 = q.start + q.size;
                    }
                }
                if matches!(align, Align::Right | Align::Center) {
                    for n in cells[..k]
                        .iter()
                        .rev()
                        .take_while(|n| same_seg(n) && empty(n))
                    {
                        x0 = cols[n.col].start;
                    }
                }
                clip = [x0, rect[1], x1 - x0, rect[3]];
            }
            let font = FontKey::new(
                family(s.font_name.as_deref().unwrap_or(&default_name)),
                s.font_size.max(1) as f32,
                s.bold,
                s.italic,
            );
            let color = if bw {
                0
            } else {
                cell.color.or(s.font_color).or(default_color).unwrap_or(0)
            };
            let icon_room = if cell.icon.is_some() && cell.merge.is_none() {
                icon_size(rect[3]) + 2.0
            } else {
                0.0
            };
            out.texts.push(CellText {
                text: cell.text.clone(),
                font,
                underline: s.underline,
                strike: s.strikethrough,
                color,
                cell: rect,
                clip,
                align,
                valign: s.valign,
                wrap,
                indent: s.indent as f32 * indent_step,
                rotation: s.rotation,
                shrink: s.shrink,
                number,
                repeat: cell.repeat,
                icon_room,
            });
        }
    }

    // Headings: column letters along the top, row numbers down the left.
    if hw > 0.0 {
        let head_font = FontKey::new(default_family.clone(), default_size, false, false);
        let head_text = |text: String, rect: [f32; 4]| CellText {
            text,
            font: head_font.clone(),
            underline: false,
            strike: false,
            color: 0,
            cell: rect,
            clip: rect,
            align: Align::Center,
            valign: VAlign::Center,
            wrap: false,
            indent: 0.0,
            rotation: 0,
            shrink: false,
            number: false,
            repeat: None,
            icon_room: 0.0,
        };
        let mut g = |from, to| {
            grid_lines.push(Line {
                from,
                to,
                style: LineKind::Grid,
                color: 0,
            })
        };
        g([0.0, 0.0], [block_w, 0.0]);
        g([0.0, 0.0], [0.0, block_h]);
        g([0.0, hh], [block_w, hh]);
        g([hw, 0.0], [hw, block_h]);
        for cp in cols.iter().filter(|c| c.size > 0.0) {
            let rect = [cp.start, 0.0, cp.size, hh];
            out.texts
                .push(head_text(sheets_engine::column_name(cp.index), rect));
            g([cp.start + cp.size, 0.0], [cp.start + cp.size, hh]);
        }
        for rp in rows.iter().filter(|r| r.size > 0.0) {
            let rect = [0.0, rp.start, hw, rp.size];
            out.texts.push(head_text((rp.index + 1).to_string(), rect));
            g([0.0, rp.start + rp.size], [hw, rp.start + rp.size]);
        }
    }

    out.lines = coalesce(grid_lines);
    out.lines.extend(coalesce(borders));
    out.overlays = overlays(book, sp, &cols, &rows);

    // Header and footer.
    let fields = hf::Fields {
        page: pr.number,
        pages: plan.pages.len() as i64,
        date: plan.options.date.clone(),
        time: plan.options.time.clone(),
        file: plan.options.file_name.clone(),
        folder: plan.options.folder.clone(),
        sheet: sp.name.clone(),
    };
    let (head, foot) = p.header_footer.for_page(pr.nth);
    out.header = hf::parse(head, &fields);
    out.footer = hf::parse(foot, &fields);
    let side = |margin: f64| {
        if p.header_footer.align_with_margins {
            (margin * INCH) as f32
        } else {
            (0.3 * INCH) as f32
        }
    };
    out.hf = HfLayout {
        left: side(m.left),
        right: pw - side(m.right),
        header: (m.header * INCH) as f32,
        footer: (m.footer * INCH) as f32,
        scale: if p.header_footer.scale_with_doc {
            scale
        } else {
            1.0
        },
        family: default_family,
        size: default_size,
    };
    out
}

/// Side of an icon in a cell `height` points tall.
pub(crate) fn icon_size(height: f32) -> f32 {
    (height - 3.0).clamp(4.0, 10.0)
}

/// Join collinear segments that touch and look the same, so dashes run on
/// and the PDF stays small.
fn coalesce(mut lines: Vec<Line>) -> Vec<Line> {
    // Normalise: from is left / top of to.
    for l in &mut lines {
        if l.from[0] > l.to[0] || l.from[1] > l.to[1] {
            std::mem::swap(&mut l.from, &mut l.to);
        }
    }
    let key = |l: &Line| {
        let horizontal = (l.from[1] - l.to[1]).abs() < 0.001;
        let fixed = if horizontal { l.from[1] } else { l.from[0] };
        let start = if horizontal { l.from[0] } else { l.from[1] };
        (horizontal, (fixed * 100.0).round() as i64, start)
    };
    let look = |l: &Line| {
        let style = match l.style {
            LineKind::Grid => 0,
            LineKind::Border(s) => 1 + s as u8,
        };
        (style, l.color)
    };
    lines.sort_by(|a, b| {
        let (ka, kb) = (key(a), key(b));
        (ka.0, ka.1, look(a))
            .cmp(&(kb.0, kb.1, look(b)))
            .then(ka.2.total_cmp(&kb.2))
    });
    let mut out: Vec<Line> = Vec::with_capacity(lines.len());
    for l in lines {
        if let Some(last) = out.last_mut() {
            let (kl, kn) = (key(last), key(&l));
            let end = if kl.0 { last.to[0] } else { last.to[1] };
            if kl.0 == kn.0
                && kl.1 == kn.1
                && last.style == l.style
                && last.color == l.color
                && kn.2 <= end + 0.01
            {
                if kl.0 {
                    last.to[0] = last.to[0].max(l.to[0]);
                } else {
                    last.to[1] = last.to[1].max(l.to[1]);
                }
                continue;
            }
        }
        out.push(l);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(x0: f32, x1: f32, y: f32) -> Line {
        Line {
            from: [x0, y],
            to: [x1, y],
            style: LineKind::Grid,
            color: 0,
        }
    }

    #[test]
    fn segments_join() {
        let lines = vec![
            h(10.0, 20.0, 5.0),
            h(0.0, 10.0, 5.0),
            h(30.0, 40.0, 5.0),
            h(0.0, 5.0, 6.0),
        ];
        let out = coalesce(lines);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].from, [0.0, 5.0]);
        assert_eq!(out[0].to, [20.0, 5.0]);
    }

    #[test]
    fn titles_lead_pages_after_theirs() {
        let size = |_| 10.0;
        let p = positions((5, 7), Some((0, 1)), 0.0, &size);
        assert_eq!(
            p.iter().map(|q| q.index).collect::<Vec<_>>(),
            vec![0, 1, 5, 6, 7]
        );
        assert_eq!(p[2].start, 20.0);
        let p = positions((0, 3), Some((0, 1)), 4.0, &size);
        assert_eq!(p.len(), 4);
        assert_eq!(p[0].start, 4.0);
        // A merge reaching above the page.
        let p = positions((5, 7), None, 0.0, &size);
        assert_eq!(span(&p, 0, 3, 6, &size), (-20.0, 40.0));
    }
}
