//! How a cell looks in the grid: its style (cached per style index), merged
//! cells, the border edges it shares with its neighbours and its gridlines.
//!
//! The grid asks for every visible cell on every frame, so everything here is
//! cached for the current workbook state (see `Workbook::cached`): resolving
//! an IronCalc style allocates (font name, number format, theme colours),
//! a cell's style index needs the row's style when the cell is empty, and a
//! merge lookup would otherwise scan the sheet's merges.

use std::collections::HashMap;
use std::sync::Arc;

use crate::format::{BorderLine, CellStyle, FontDefaults};
use crate::{CellRange, MAX_COLS, MAX_ROWS, Workbook};

/// Rows per merge-index bucket.
const BUCKET: u32 = 32;

/// Styles and merges of the current workbook state.
#[derive(Default)]
pub(crate) struct LookCache {
    /// Resolved styles by IronCalc style index.
    styles: Vec<Option<Arc<CellStyle>>>,
    /// Compiled number formats by IronCalc style index.
    formats: Vec<Option<kantor_numfmt::NumberFormat>>,
    font: Option<FontDefaults>,
    /// Per sheet: styles of rows with a custom format (1-based row → style).
    row_styles: HashMap<u32, Arc<HashMap<i32, i32>>>,
    merges: HashMap<u32, Arc<MergeIndex>>,
}

/// A sheet's merged cells, bucketed by row for lookups.
#[derive(Default, Debug)]
pub(crate) struct MergeIndex {
    ranges: Vec<CellRange>,
    /// `row / BUCKET` → indices into `ranges` of merges touching those rows.
    buckets: HashMap<u32, Vec<u32>>,
}

impl MergeIndex {
    pub(crate) fn new(ranges: Vec<CellRange>) -> Self {
        let mut buckets: HashMap<u32, Vec<u32>> = HashMap::new();
        for (ix, r) in ranges.iter().enumerate() {
            for b in r.top / BUCKET..=r.bottom / BUCKET {
                buckets.entry(b).or_default().push(ix as u32);
            }
        }
        Self { ranges, buckets }
    }

    pub(crate) fn find(&self, row: u32, col: u32) -> Option<CellRange> {
        if self.ranges.is_empty() {
            return None;
        }
        self.buckets
            .get(&(row / BUCKET))?
            .iter()
            .map(|&ix| self.ranges[ix as usize])
            .find(|r| r.contains(row, col))
    }
}

/// Everything the grid draws for one cell, besides its value.
#[derive(Debug, Clone)]
pub struct CellLook {
    /// The style the content is drawn with: the cell's own, or for a merged
    /// cell its top-left (anchor) cell's.
    pub style: Arc<CellStyle>,
    /// The merged area this cell is part of.
    pub merge: Option<CellRange>,
    /// The edges this cell draws, already shared with the neighbour on the
    /// other side (see [`BorderLine::shared`]). A cell draws its right and
    /// bottom edges; its top and left only in the first row / column, since
    /// otherwise the neighbour draws them. Edges inside a merge are `None`.
    pub right: Option<BorderLine>,
    pub bottom: Option<BorderLine>,
    pub top: Option<BorderLine>,
    pub left: Option<BorderLine>,
    /// Draw a gridline on the right / bottom edge: gridlines are on, there is
    /// no border there, the edge isn't inside a merge and neither side is
    /// filled (fills cover gridlines, as in Excel).
    pub grid_right: bool,
    pub grid_bottom: bool,
}

impl Workbook {
    /// IronCalc's style index for a cell: the cell's own, else its row's
    /// (when the row has a format), else its column's, else 0.
    pub fn style_index(&self, sheet: u32, row: u32, col: u32) -> i32 {
        let Ok(ws) = self.model.get_model().workbook.worksheet(sheet) else {
            return 0;
        };
        let (r, c) = (row as i32 + 1, col as i32 + 1);
        if let Some(cell) = ws.cell(r, c) {
            return cell.get_style();
        }
        let rows = self.row_styles(sheet);
        if let Some(&s) = rows.get(&r) {
            return s;
        }
        ws.cols
            .iter()
            .find(|cl| (cl.min..=cl.max).contains(&c))
            .and_then(|cl| cl.style)
            .unwrap_or(0)
    }

    fn row_styles(&self, sheet: u32) -> Arc<HashMap<i32, i32>> {
        self.cached(|cache| {
            cache
                .look
                .row_styles
                .entry(sheet)
                .or_insert_with(|| {
                    let rows = self
                        .model
                        .get_model()
                        .workbook
                        .worksheet(sheet)
                        .map(|ws| {
                            ws.rows
                                .iter()
                                .filter(|r| r.custom_format)
                                .map(|r| (r.r, r.s))
                                .collect()
                        })
                        .unwrap_or_default();
                    Arc::new(rows)
                })
                .clone()
        })
    }

    /// The resolved style with IronCalc index `index` (default style if the
    /// index is invalid).
    pub fn style_by_index(&self, index: i32) -> Arc<CellStyle> {
        let ix = index.max(0) as usize;
        self.cached(|cache| {
            if let Some(Some(s)) = cache.look.styles.get(ix) {
                return s.clone();
            }
            let model = self.model.get_model();
            let font = cache
                .look
                .font
                .get_or_insert_with(|| font_defaults(model))
                .clone();
            let style = model
                .workbook
                .styles
                .cell_xfs
                .get(ix)
                .and_then(|_| model.workbook.styles.get_style(ix as i32).ok())
                .map(|s| CellStyle::from_ironcalc(&s, &font, &model.workbook.theme))
                .unwrap_or_else(|| CellStyle {
                    font_size: font.size,
                    num_fmt: "general".to_owned(),
                    ..CellStyle::default()
                });
            let style = Arc::new(style);
            if cache.look.styles.len() <= ix {
                cache.look.styles.resize(ix + 1, None);
            }
            cache.look.styles[ix] = Some(style.clone());
            style
        })
    }

    /// The compiled number format of a style index (cached like the style).
    pub(crate) fn number_format_by_index(&self, index: i32) -> kantor_numfmt::NumberFormat {
        let ix = index.max(0) as usize;
        self.cached(|cache| {
            if let Some(Some(f)) = cache.look.formats.get(ix) {
                return f.clone();
            }
            let f = crate::numfmt::compile(self.model.get_model(), index);
            if cache.look.formats.len() <= ix {
                cache.look.formats.resize(ix + 1, None);
            }
            cache.look.formats[ix] = Some(f.clone());
            f
        })
    }

    /// The workbook's default font size in points (Excel's "Normal" style).
    pub fn default_font_size(&self) -> i32 {
        self.cached(|cache| {
            cache
                .look
                .font
                .get_or_insert_with(|| font_defaults(self.model.get_model()))
                .size
        })
    }

    fn merge_index(&self, sheet: u32) -> Arc<MergeIndex> {
        self.cached(|cache| {
            cache
                .look
                .merges
                .entry(sheet)
                .or_insert_with(|| {
                    let ranges = self
                        .model
                        .get_model()
                        .workbook
                        .worksheet(sheet)
                        .map(|ws| ws.merged_cells.iter().filter_map(merge_range).collect())
                        .unwrap_or_default();
                    Arc::new(MergeIndex::new(ranges))
                })
                .clone()
        })
    }

    /// The merged area containing a cell, if any.
    pub fn merge_at(&self, sheet: u32, row: u32, col: u32) -> Option<CellRange> {
        self.merge_index(sheet).find(row, col)
    }

    /// Every merged area of `sheet` that overlaps `range`.
    pub fn merges_in(&self, sheet: u32, range: CellRange) -> Vec<CellRange> {
        self.merge_index(sheet)
            .ranges
            .iter()
            .copied()
            .filter(|m| overlaps(m, &range))
            .collect()
    }

    /// `range` grown until it cuts no merged area in half (Excel's selection
    /// rule).
    pub fn expand_to_merges(&self, sheet: u32, mut range: CellRange) -> CellRange {
        let index = self.merge_index(sheet);
        if index.ranges.is_empty() {
            return range;
        }
        loop {
            let mut grown = range;
            for m in index.ranges.iter().filter(|m| overlaps(m, &range)) {
                grown.top = grown.top.min(m.top);
                grown.left = grown.left.min(m.left);
                grown.bottom = grown.bottom.max(m.bottom);
                grown.right = grown.right.max(m.right);
            }
            if grown == range {
                return range;
            }
            range = grown;
        }
    }

    /// Whether the sheet shows gridlines (Excel's View > Gridlines).
    pub fn show_gridlines(&self, sheet: u32) -> bool {
        self.model
            .get_model()
            .workbook
            .worksheet(sheet)
            .map_or(true, |ws| ws.show_grid_lines)
    }

    /// The style a cell is drawn with and its merge: a merged cell takes its
    /// anchor's style.
    fn drawn_style(
        &self,
        merges: &MergeIndex,
        sheet: u32,
        row: u32,
        col: u32,
    ) -> (Arc<CellStyle>, Option<CellRange>) {
        match merges.find(row, col) {
            Some(m) => (self.cell_style(sheet, m.top, m.left), Some(m)),
            None => (self.cell_style(sheet, row, col), None),
        }
    }

    /// How the grid draws a cell (see [`CellLook`]).
    pub fn cell_look(&self, sheet: u32, row: u32, col: u32) -> CellLook {
        let merges = self.merge_index(sheet);
        let gridlines = self.show_gridlines(sheet);
        let (style, merge) = self.drawn_style(&merges, sheet, row, col);
        // Borders are the cell's own, even inside a merge (Excel stores the
        // outline of a merged cell on its edge cells).
        let own = if merge.is_some() {
            self.cell_style(sheet, row, col).borders
        } else {
            style.borders
        };
        let inside = |r: u32, c: u32| merge.is_some_and(|m| m.contains(r, c));

        let (right, grid_right) = if col + 1 < MAX_COLS {
            let theirs = self.cell_style(sheet, row, col + 1);
            if inside(row, col + 1) {
                (None, false)
            } else {
                let line = BorderLine::shared(own.right, theirs.borders.left);
                let filled = style.fill.is_some()
                    || self
                        .drawn_style(&merges, sheet, row, col + 1)
                        .0
                        .fill
                        .is_some();
                (line, gridlines && line.is_none() && !filled)
            }
        } else {
            (own.right, false)
        };
        let (bottom, grid_bottom) = if row + 1 < MAX_ROWS {
            let theirs = self.cell_style(sheet, row + 1, col);
            if inside(row + 1, col) {
                (None, false)
            } else {
                let line = BorderLine::shared(own.bottom, theirs.borders.top);
                let filled = style.fill.is_some()
                    || self
                        .drawn_style(&merges, sheet, row + 1, col)
                        .0
                        .fill
                        .is_some();
                (line, gridlines && line.is_none() && !filled)
            }
        } else {
            (own.bottom, false)
        };
        CellLook {
            top: if row == 0 { own.top } else { None },
            left: if col == 0 { own.left } else { None },
            style,
            merge,
            right,
            bottom,
            grid_right,
            grid_bottom,
        }
    }
}

fn overlaps(a: &CellRange, b: &CellRange) -> bool {
    a.left <= b.right && b.left <= a.right && a.top <= b.bottom && b.top <= a.bottom
}

/// IronCalc's merged cell (1-based anchor and size) as a range; `None` if
/// it is malformed.
fn merge_range(m: &ironcalc_base::types::MergedCell) -> Option<CellRange> {
    if m.row < 1 || m.column < 1 || m.width < 1 || m.height < 1 {
        return None;
    }
    let (top, left) = (m.row as u32 - 1, m.column as u32 - 1);
    let bottom = (top + m.height as u32 - 1).min(MAX_ROWS - 1);
    let right = (left + m.width as u32 - 1).min(MAX_COLS - 1);
    Some(CellRange::from_corners((top, left), (bottom, right)))
}

fn font_defaults(model: &ironcalc_base::Model) -> FontDefaults {
    let styles = &model.workbook.styles;
    // The "Normal" cell style's font: `cellXfs[0]`'s, which is `fonts[0]`
    // in practically every file.
    let font = styles
        .cell_xfs
        .first()
        .and_then(|xf| styles.fonts.get(xf.font_id as usize))
        .or_else(|| styles.fonts.first());
    match font {
        Some(f) => FontDefaults {
            name: f.name.clone(),
            size: f.sz,
            color: match &f.color {
                ironcalc_base::types::Color::None => None,
                c => crate::format::parse_rgb(&c.to_rgb(&model.workbook.theme)),
            },
        },
        None => FontDefaults {
            name: String::new(),
            size: 11,
            color: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;
    use crate::format::{BorderPreset, LineStyle, StyleChange, VAlign};

    const NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
    const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

    /// A styled workbook, written by hand so the test needs no Excel or
    /// Python. Sheet 1 "Report":
    ///
    /// * A1: fill theme 4 (accent 1) tint 0.6; B1: fill `indexed="44"`;
    ///   C1: red bold 16 pt Arial text, wrapped, top-aligned; D1: the default
    ///   font (its `theme="1"` colour counts as automatic).
    /// * A3: right edge thin; B3: left edge thick (the shared edge is thick).
    ///   A4: bottom edge medium, blue.
    /// * A6:C7 merged (anchor A6 has a fill), and A40:B70 merged (several
    ///   index buckets).
    ///
    /// Sheet 2 "Plain" has gridlines off.
    fn styled_xlsx() -> Vec<u8> {
        let styles = format!(
            "<styleSheet xmlns=\"{NS}\">\
<fonts count=\"2\">\
<font><sz val=\"11\"/><color theme=\"1\"/><name val=\"Calibri\"/><family val=\"2\"/><scheme val=\"minor\"/></font>\
<font><b/><sz val=\"16\"/><color rgb=\"FFFF0000\"/><name val=\"Arial\"/><family val=\"2\"/></font>\
</fonts>\
<fills count=\"5\">\
<fill><patternFill patternType=\"none\"/></fill>\
<fill><patternFill patternType=\"gray125\"/></fill>\
<fill><patternFill patternType=\"solid\"><fgColor theme=\"4\" tint=\"0.59999389629810485\"/><bgColor indexed=\"64\"/></patternFill></fill>\
<fill><patternFill patternType=\"solid\"><fgColor indexed=\"44\"/><bgColor indexed=\"64\"/></patternFill></fill>\
<fill><patternFill patternType=\"solid\"><fgColor rgb=\"FFFFF2CC\"/><bgColor indexed=\"64\"/></patternFill></fill>\
</fills>\
<borders count=\"4\">\
<border><left/><right/><top/><bottom/><diagonal/></border>\
<border><left/><right style=\"thin\"><color indexed=\"64\"/></right><top/><bottom/><diagonal/></border>\
<border><left style=\"thick\"><color rgb=\"FF000000\"/></left><right/><top/><bottom/><diagonal/></border>\
<border><left/><right/><top/><bottom style=\"medium\"><color rgb=\"FF0000FF\"/></bottom><diagonal/></border>\
</borders>\
<cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>\
<cellXfs count=\"8\">\
<xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>\
<xf numFmtId=\"0\" fontId=\"0\" fillId=\"2\" borderId=\"0\" xfId=\"0\" applyFill=\"1\"/>\
<xf numFmtId=\"0\" fontId=\"0\" fillId=\"3\" borderId=\"0\" xfId=\"0\" applyFill=\"1\"/>\
<xf numFmtId=\"0\" fontId=\"1\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyFont=\"1\" applyAlignment=\"1\"><alignment vertical=\"top\" wrapText=\"1\"/></xf>\
<xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"1\" xfId=\"0\" applyBorder=\"1\"/>\
<xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"2\" xfId=\"0\" applyBorder=\"1\"/>\
<xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"3\" xfId=\"0\" applyBorder=\"1\"/>\
<xf numFmtId=\"0\" fontId=\"0\" fillId=\"4\" borderId=\"0\" xfId=\"0\" applyFill=\"1\" applyAlignment=\"1\"><alignment horizontal=\"center\" vertical=\"center\"/></xf>\
</cellXfs>\
<cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles>\
</styleSheet>"
        );
        let c = |r: &str, s: u32, v: Option<&str>| match v {
            Some(v) => format!("<c r=\"{r}\" s=\"{s}\"><v>{v}</v></c>"),
            None => format!("<c r=\"{r}\" s=\"{s}\"/>"),
        };
        let sheet1 = format!(
            "<worksheet xmlns=\"{NS}\"><sheetData>\
<row r=\"1\">{}{}{}{}</row>\
<row r=\"3\">{}{}</row>\
<row r=\"4\">{}</row>\
<row r=\"6\">{}</row>\
</sheetData>\
<mergeCells count=\"2\"><mergeCell ref=\"A6:C7\"/><mergeCell ref=\"A40:B70\"/></mergeCells>\
</worksheet>",
            c("A1", 1, Some("1")),
            c("B1", 2, Some("2")),
            c("C1", 3, Some("3")),
            c("D1", 0, Some("4")),
            c("A3", 4, None),
            c("B3", 5, None),
            c("A4", 6, None),
            c("A6", 7, Some("6")),
        );
        let sheet2 = format!(
            "<worksheet xmlns=\"{NS}\"><sheetViews><sheetView showGridLines=\"0\" workbookViewId=\"0\"/></sheetViews><sheetData/></worksheet>"
        );
        let rels = |items: &[(&str, &str, &str)]| {
            let mut s = String::from(
                "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
            );
            for (id, ty, target) in items {
                s.push_str(&format!(
                    "<Relationship Id=\"{id}\" Type=\"{ty}\" Target=\"{target}\"/>"
                ));
            }
            s + "</Relationships>"
        };
        let ws_type = "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml";
        let parts = [
            (
                "[Content_Types].xml",
                format!(
                    "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
<Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"{ws_type}\"/>\
<Override PartName=\"/xl/worksheets/sheet2.xml\" ContentType=\"{ws_type}\"/>\
<Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>\
<Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>\
<Override PartName=\"/docProps/app.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.extended-properties+xml\"/>\
</Types>"
                ),
            ),
            (
                "_rels/.rels",
                rels(&[
                    ("rId1", &format!("{R}/officeDocument"), "xl/workbook.xml"),
                    (
                        "rId2",
                        "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties",
                        "docProps/core.xml",
                    ),
                    ("rId3", &format!("{R}/extended-properties"), "docProps/app.xml"),
                ]),
            ),
            (
                "docProps/core.xml",
                "<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:creator>t</dc:creator></cp:coreProperties>".into(),
            ),
            (
                "docProps/app.xml",
                "<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\"><Application>t</Application></Properties>".into(),
            ),
            (
                "xl/workbook.xml",
                format!(
                    "<workbook xmlns=\"{NS}\" xmlns:r=\"{R}\"><sheets>\
<sheet name=\"Report\" sheetId=\"1\" r:id=\"rId1\"/><sheet name=\"Plain\" sheetId=\"2\" r:id=\"rId2\"/>\
</sheets></workbook>"
                ),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                rels(&[
                    ("rId1", &format!("{R}/worksheet"), "worksheets/sheet1.xml"),
                    ("rId2", &format!("{R}/worksheet"), "worksheets/sheet2.xml"),
                    ("rId3", &format!("{R}/styles"), "styles.xml"),
                ]),
            ),
            ("xl/styles.xml", styles),
            ("xl/worksheets/sheet1.xml", sheet1),
            ("xl/worksheets/sheet2.xml", sheet2),
        ];
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in parts {
            zip.start_file(name, opts).unwrap();
            zip.write_all(data.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn open_styled() -> (tempfile::TempDir, Workbook) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("styled.xlsx");
        std::fs::write(&path, styled_xlsx()).unwrap();
        let wb = Workbook::open(&path).unwrap();
        (dir, wb)
    }

    fn near(a: Option<u32>, b: u32) -> bool {
        let Some(a) = a else { return false };
        (0..3).all(|i| {
            let (x, y) = ((a >> (i * 8)) & 0xFF, (b >> (i * 8)) & 0xFF);
            x.abs_diff(y) <= 2
        })
    }

    #[test]
    fn resolves_theme_and_indexed_colours() {
        let (_dir, wb) = open_styled();
        // Excel shows accent 1 (#4472C4) at tint 0.6 as #B4C6E7.
        let a1 = wb.cell_style(0, 0, 0);
        assert!(near(a1.fill, 0xB4C6E7), "{:06X?}", a1.fill);
        // The default indexed palette's entry 44.
        assert_eq!(wb.cell_style(0, 0, 1).fill, Some(0x99CCFF));
        let c1 = wb.cell_style(0, 0, 2);
        assert_eq!(c1.font_color, Some(0xFF0000));
        assert!(c1.bold && c1.wrap);
        assert_eq!(c1.valign, VAlign::Top);
        assert_eq!(c1.font_size, 16);
        assert_eq!(c1.font_name.as_deref(), Some("Arial"));
        // The default font: automatic colour, no family of its own.
        let d1 = wb.cell_style(0, 0, 3);
        assert_eq!((d1.font_color, d1.font_name.clone()), (None, None));
        assert_eq!(d1.fill, None);
        assert_eq!(d1.valign, VAlign::Bottom);
        assert_eq!(wb.default_font_size(), 11);
        // Empty cells use the default style.
        assert_eq!(*wb.cell_style(0, 99, 99), *d1);
    }

    #[test]
    fn border_edges_are_shared_with_neighbours() {
        let (_dir, wb) = open_styled();
        // A3's thin right edge meets B3's thick left edge: thick wins.
        let a3 = wb.cell_look(0, 2, 0);
        assert_eq!(a3.right.map(|l| l.style), Some(LineStyle::Thick));
        assert!(!a3.grid_right, "a border replaces the gridline");
        assert!(a3.grid_bottom);
        // B3 doesn't draw its left edge: A3 does.
        assert_eq!(wb.cell_look(0, 2, 1).left, None);
        // A4's own bottom edge, with its colour.
        let a4 = wb.cell_look(0, 3, 0);
        let bottom = a4.bottom.unwrap();
        assert_eq!(
            (bottom.style, bottom.color),
            (LineStyle::Medium, Some(0x0000FF))
        );
        // A2 (above A3) draws no border on its bottom: A3 has no top edge.
        assert_eq!(wb.cell_look(0, 1, 0).bottom, None);
        // Fills cover gridlines on both sides of an edge.
        assert!(!wb.cell_look(0, 0, 0).grid_right, "A1 is filled");
        assert!(!wb.cell_look(0, 0, 0).grid_bottom);
        let e1 = wb.cell_look(0, 0, 4);
        assert!(e1.grid_right && e1.grid_bottom);
        // D1's left neighbour C1 has no fill; C1 (unfilled) next to B1 (filled):
        assert!(wb.cell_look(0, 0, 2).grid_right);
        assert!(!wb.cell_look(0, 0, 1).grid_right);

        // The weights: heavier wins, then a colour, then this cell's line.
        let l = |style, color| Some(BorderLine { style, color });
        assert_eq!(
            BorderLine::shared(l(LineStyle::Medium, None), l(LineStyle::Thin, Some(1))),
            l(LineStyle::Medium, None)
        );
        assert_eq!(
            BorderLine::shared(l(LineStyle::Double, None), l(LineStyle::Thick, None)),
            l(LineStyle::Thick, None)
        );
        assert_eq!(
            BorderLine::shared(l(LineStyle::Thin, None), l(LineStyle::Thin, Some(7))),
            l(LineStyle::Thin, Some(7))
        );
        assert_eq!(
            BorderLine::shared(None, l(LineStyle::Dotted, None)),
            l(LineStyle::Dotted, None)
        );
        assert_eq!(BorderLine::shared(None, None), None);
    }

    #[test]
    fn merged_cells_look_and_hit_testing() {
        let (_dir, wb) = open_styled();
        let m = CellRange::from_corners((5, 0), (6, 2));
        for (r, c) in [(5, 0), (5, 1), (6, 2)] {
            assert_eq!(wb.merge_at(0, r, c), Some(m), "({r}, {c})");
        }
        for (r, c) in [(4, 0), (5, 3), (7, 0)] {
            assert_eq!(wb.merge_at(0, r, c), None, "({r}, {c})");
        }
        // Every cell of the merge is drawn with the anchor's style.
        let b7 = wb.cell_look(0, 6, 1);
        assert_eq!(b7.style.fill, Some(0xFFF2CC));
        assert_eq!(b7.merge, Some(m));
        // No gridlines or borders inside the merge; its outside edge does
        // keep the gridline logic (filled, so none).
        let a6 = wb.cell_look(0, 5, 0);
        assert!(!a6.grid_right && !a6.grid_bottom && a6.right.is_none());
        assert!(!wb.cell_look(0, 6, 2).grid_right, "merged block is filled");
        // A merge over several index buckets.
        let tall = CellRange::from_corners((39, 0), (69, 1));
        for r in [39, 50, 63, 64, 69] {
            assert_eq!(wb.merge_at(0, r, 1), Some(tall), "row {r}");
        }
        assert_eq!(wb.merge_at(0, 70, 0), None);
        // Selections grow to whole merges, transitively.
        let sel = CellRange::from_corners((4, 2), (5, 3));
        assert_eq!(
            wb.expand_to_merges(0, sel),
            CellRange::from_corners((4, 0), (6, 3))
        );
        let one = CellRange::from_corners((0, 5), (0, 5));
        assert_eq!(wb.expand_to_merges(0, one), one);
        assert_eq!(wb.merges_in(0, CellRange::all()).len(), 2);
        assert!(wb.merges_in(1, CellRange::all()).is_empty());
    }

    #[test]
    fn gridlines_per_sheet() {
        let (_dir, wb) = open_styled();
        assert!(wb.show_gridlines(0));
        assert!(!wb.show_gridlines(1));
        let look = wb.cell_look(1, 3, 3);
        assert!(!look.grid_right && !look.grid_bottom);
    }

    #[test]
    fn style_cache_shares_and_refreshes() {
        let mut wb = Workbook::new_empty().unwrap();
        let a = wb.cell_style(0, 0, 0);
        let b = wb.cell_style(0, 5, 5);
        assert!(Arc::ptr_eq(&a, &b), "one resolved style per index");
        assert_eq!(*wb.style_by_index(9999), *a, "unknown index: default");
        let r = CellRange::from_corners((0, 0), (0, 0));
        wb.set_range_style(0, r, StyleChange::Fill(Some(0x123456)))
            .unwrap();
        assert_eq!(wb.cell_style(0, 0, 0).fill, Some(0x123456));
        assert_eq!(wb.cell_style(0, 5, 5).fill, None);
        wb.undo().unwrap();
        assert_eq!(wb.cell_style(0, 0, 0).fill, None);

        // Row styles apply to the row's empty cells.
        let mut wb = Workbook::new_empty().unwrap();
        let row = CellRange::rows(3, 3);
        wb.model
            .update_range_style(&row.area(0), "font.b", "true")
            .unwrap();
        assert!(wb.cell_style(0, 3, 7).bold, "row style");
        assert!(!wb.cell_style(0, 4, 7).bold);
    }

    #[test]
    fn colour_size_wrap_valign_edits_undo_in_one_step() {
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(0, 0, 0, "x").unwrap();
        let r = CellRange::from_corners((0, 0), (1, 1));
        let base = wb.cell_style(0, 0, 0);
        for change in [
            StyleChange::Fill(Some(0xFFF2CC)),
            StyleChange::FontColor(Some(0xC00000)),
            StyleChange::FontSizeDelta(2),
            StyleChange::Wrap(true),
            StyleChange::VAlign(VAlign::Center),
        ] {
            wb.set_range_style(0, r, change).unwrap();
        }
        let s = wb.cell_style(0, 1, 1);
        assert_eq!(s.fill, Some(0xFFF2CC));
        assert_eq!(s.font_color, Some(0xC00000));
        assert_eq!(s.font_size, base.font_size + 2);
        assert!(s.wrap);
        assert_eq!(s.valign, VAlign::Center);
        wb.set_range_style(0, r, StyleChange::Fill(None)).unwrap();
        wb.set_range_style(0, r, StyleChange::FontColor(None))
            .unwrap();
        assert_eq!(
            (
                wb.cell_style(0, 0, 0).fill,
                wb.cell_style(0, 0, 0).font_color
            ),
            (None, None)
        );
        for _ in 0..7 {
            assert!(wb.undo().unwrap());
        }
        assert_eq!(*wb.cell_style(0, 1, 1), *base);
        assert!(wb.undo().unwrap(), "the input");
        assert!(!wb.can_undo());
        assert!(
            wb.set_range_style(0, r, StyleChange::FontSizeDelta(-100))
                .is_err()
        );
    }

    #[test]
    fn border_presets_and_merges_are_one_undo_step() {
        let mut wb = Workbook::new_empty().unwrap();
        let r = CellRange::from_corners((1, 1), (2, 2));
        wb.set_range_borders(0, r, BorderPreset::Outline).unwrap();
        let b2 = wb.cell_look(0, 1, 1);
        assert!(b2.right.is_none(), "inner edge");
        assert_eq!(b2.bottom, None);
        // B2's top edge is drawn by B1 (its bottom).
        assert_eq!(
            wb.cell_look(0, 0, 1).bottom.map(|l| l.style),
            Some(LineStyle::Thin)
        );
        assert!(wb.cell_look(0, 1, 2).right.is_some());
        wb.set_range_borders(0, r, BorderPreset::All).unwrap();
        assert!(wb.cell_look(0, 1, 1).right.is_some());
        assert!(wb.cell_look(0, 1, 1).bottom.is_some());
        wb.set_range_borders(0, r, BorderPreset::None).unwrap();
        assert!(wb.cell_look(0, 1, 1).right.is_none());
        assert!(wb.cell_look(0, 0, 1).bottom.is_none());
        assert!(wb.undo().unwrap());
        assert!(wb.cell_look(0, 1, 1).right.is_some());
        assert!(wb.undo().unwrap());
        assert!(wb.cell_look(0, 1, 1).right.is_none());
        assert!(wb.cell_look(0, 1, 2).right.is_some());

        wb.set_input(0, 1, 1, "Title").unwrap();
        wb.merge_range(0, r).unwrap();
        assert_eq!(wb.merge_at(0, 2, 2), Some(r));
        assert_eq!(wb.cell_style(0, 1, 1).align, crate::HAlign::Center);
        assert!(
            wb.merge_range(0, CellRange::from_corners((5, 5), (5, 5)))
                .is_err()
        );
        assert!(
            wb.unmerge_range(0, CellRange::from_corners((2, 2), (2, 2)))
                .unwrap()
        );
        assert_eq!(wb.merge_at(0, 2, 2), None);
        assert!(!wb.unmerge_range(0, r).unwrap(), "nothing left to unmerge");
        assert!(wb.undo().unwrap());
        assert_eq!(wb.merge_at(0, 2, 2), Some(r));
        assert!(wb.undo().unwrap());
        assert_eq!(wb.merge_at(0, 2, 2), None);
        assert_eq!(wb.cell_style(0, 1, 1).align, crate::HAlign::General);
    }

    /// Opening and saving keeps the formatting: every cell looks the same
    /// after a save and reopen, and a second save writes byte-identical
    /// styles and sheet markup (merges, gridlines) to the first.
    #[test]
    fn styled_workbook_round_trips() {
        let (dir, mut wb) = open_styled();
        let looks = |wb: &Workbook| {
            let mut v = Vec::new();
            for sheet in 0..2 {
                for row in 0..72 {
                    for col in 0..6 {
                        let l = wb.cell_look(sheet, row, col);
                        v.push(format!(
                            "{:?} {:?} {:?} {:?} {:?} {:?} {} {}",
                            l.style,
                            l.merge,
                            l.right,
                            l.bottom,
                            l.top,
                            l.left,
                            l.grid_right,
                            l.grid_bottom
                        ));
                    }
                }
            }
            v
        };
        let before = looks(&wb);
        let first = dir.path().join("first.xlsx");
        wb.save_as(&first).unwrap();
        let mut back = Workbook::open(&first).unwrap();
        assert_eq!(looks(&back), before);
        let second = dir.path().join("second.xlsx");
        back.save_as(&second).unwrap();
        let p1 = crate::preserve::parts_of(&std::fs::read(&first).unwrap());
        let p2 = crate::preserve::parts_of(&std::fs::read(&second).unwrap());
        for part in [
            "xl/styles.xml",
            "xl/worksheets/sheet1.xml",
            "xl/worksheets/sheet2.xml",
        ] {
            assert_eq!(p1[part], p2[part], "{part}");
        }
        let sheet1 = String::from_utf8(p1["xl/worksheets/sheet1.xml"].clone()).unwrap();
        assert!(
            sheet1.contains("A6:C7") && sheet1.contains("A40:B70"),
            "{sheet1}"
        );
        let sheet2 = String::from_utf8(p1["xl/worksheets/sheet2.xml"].clone()).unwrap();
        assert!(sheet2.contains("showGridLines=\"0\""), "{sheet2}");
    }
}
