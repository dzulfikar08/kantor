//! Excel tables (ListObjects) as the grid draws them: where they are, their
//! header and total rows, filter buttons, and the look of each cell from the
//! table style (banded rows and columns, first/last column, header and total
//! rows).
//!
//! The tables themselves live in IronCalc's model (`workbook.tables`, with the
//! `<autoFilter>` Kantor keeps; see `vendor/ironcalc_base` KANTOR_PATCHES §13).
//! Styles: the built-in `TableStyleLight1-21`, `Medium1-28` and `Dark1-11`
//! are described by family below, with their colours taken from the
//! workbook's theme as Excel does (accent colours, lighter or darker by a
//! tint); a workbook's own `<tableStyles>` in `styles.xml` are read from its
//! differential formats.

use std::collections::HashMap;
use std::io::Read;

use ironcalc_base::types::{Color, Table, Theme};
use roxmltree::Node;

use crate::format::{BorderLine, LineStyle, Rgb, parse_rgb};
use crate::{CellRange, Workbook};

/// A table, as the grid needs it. Rows and columns are 0-based.
#[derive(Debug, Clone, PartialEq)]
pub struct TableInfo {
    pub name: String,
    /// The whole table: header, data and total rows.
    pub area: CellRange,
    pub header_rows: u32,
    pub totals_rows: u32,
    pub columns: Vec<String>,
    pub style: Option<String>,
    pub first_column: bool,
    pub last_column: bool,
    pub row_stripes: bool,
    pub column_stripes: bool,
    /// Filter buttons in the header row.
    pub filter_buttons: bool,
}

impl TableInfo {
    fn from_ironcalc(t: &Table) -> Option<Self> {
        let (top, left, bottom, right) = ironcalc_base::kantor_tables::table_area(t)?;
        Some(Self {
            name: t.name.clone(),
            area: CellRange {
                top: top as u32 - 1,
                left: left as u32 - 1,
                bottom: bottom as u32 - 1,
                right: right as u32 - 1,
            },
            header_rows: t.header_row_count,
            totals_rows: t.totals_row_count,
            columns: t.columns.iter().map(|c| c.name.clone()).collect(),
            style: t.style_info.name.clone(),
            first_column: t.style_info.show_first_column,
            last_column: t.style_info.show_last_column,
            row_stripes: t.style_info.show_row_stripes,
            column_stripes: t.style_info.show_column_stripes,
            filter_buttons: t.header_row_count > 0 && t.kantor_auto_filter.is_some(),
        })
    }

    /// The header row, if the table shows one.
    pub fn header_row(&self) -> Option<u32> {
        (self.header_rows > 0).then_some(self.area.top)
    }

    /// The total row, if the table shows one.
    pub fn totals_row(&self) -> Option<u32> {
        (self.totals_rows > 0).then_some(self.area.bottom)
    }

    /// The data rows (may be empty: `top > bottom`).
    pub fn data_rows(&self) -> (u32, u32) {
        (
            self.area.top + self.header_rows,
            self.area.bottom.saturating_sub(self.totals_rows),
        )
    }

    /// The data body: what is sorted and filtered.
    pub fn data_range(&self) -> CellRange {
        let (top, bottom) = self.data_rows();
        CellRange {
            top,
            bottom,
            left: self.area.left,
            right: self.area.right,
        }
    }
}

/// How a table style draws one cell (on top of the cell's own format, which
/// wins where it says something: Excel's direct formatting).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TableCellLook {
    pub fill: Option<Rgb>,
    pub font_color: Option<Rgb>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub top: Option<BorderLine>,
    pub bottom: Option<BorderLine>,
    pub left: Option<BorderLine>,
    pub right: Option<BorderLine>,
    /// Indent steps a pivot table gives a compact row label (its level),
    /// unless the cell has its own.
    pub indent: u32,
}

/// One element of a table style (`wholeTable`, `headerRow`...): a
/// differential format, as Excel's Format Cells for a table element sets
/// it (font, border, fill). `None`: the element leaves it as it is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Elem {
    pub fill: Option<Rgb>,
    pub font: Option<Rgb>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    pub left: Option<BorderLine>,
    pub right: Option<BorderLine>,
    pub top: Option<BorderLine>,
    pub bottom: Option<BorderLine>,
    /// Lines between rows / columns inside the element.
    pub horizontal: Option<BorderLine>,
    pub vertical: Option<BorderLine>,
}

impl Elem {
    /// Whether the element formats nothing.
    pub fn is_empty(&self) -> bool {
        *self == Elem::default()
    }
}

/// A table style: its elements, from the lowest priority up (Excel's
/// order: whole table, column stripes, row stripes, last column, first
/// column, header row, total row, first and last header cell, first and
/// last total cell).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct TableStyleDef {
    pub(crate) whole: Elem,
    pub(crate) col_stripe1: Elem,
    pub(crate) col_stripe2: Elem,
    pub(crate) row_stripe1: Elem,
    pub(crate) row_stripe2: Elem,
    /// Rows or columns in each stripe (first, second).
    pub(crate) row_stripe_size: u32,
    pub(crate) row_stripe2_size: u32,
    pub(crate) col_stripe_size: u32,
    pub(crate) col_stripe2_size: u32,
    pub(crate) first_col: Elem,
    pub(crate) last_col: Elem,
    pub(crate) header: Elem,
    pub(crate) total: Elem,
    pub(crate) first_header: Elem,
    pub(crate) last_header: Elem,
    pub(crate) first_total: Elem,
    pub(crate) last_total: Elem,
    /// PivotTable-only elements (`pageFieldLabels`, `firstSubtotalRow`...):
    /// `(type, format, size)`.
    pub(crate) extra: Vec<(String, Elem, u32)>,
}

impl TableStyleDef {
    /// A style with no element formatted and stripes one row or column.
    pub(crate) fn empty() -> Self {
        Self {
            row_stripe_size: 1,
            row_stripe2_size: 1,
            col_stripe_size: 1,
            col_stripe2_size: 1,
            ..Default::default()
        }
    }
}

/// A sheet's tables and their styles.
pub(crate) struct SheetTables {
    tables: Vec<(TableInfo, Option<TableStyleDef>)>,
    /// Each table's own differential formats (same order as `tables`).
    dxfs: Vec<TableDxfs>,
    /// The sheet's hidden rows, sorted: bands count the rows shown.
    hidden: Vec<u32>,
}

/// Custom table styles of a workbook (`<tableStyles>`), by name, with the
/// default style for new tables.
#[derive(Debug, Clone, Default)]
pub(crate) struct CustomStyles {
    pub styles: HashMap<String, TableStyleDef>,
    /// The file's `<fills>` that are patterns (not solid), by fill index
    /// (IronCalc keeps one colour per fill).
    pub patterns: Vec<Option<FillPattern>>,
    /// The file's `<tableStyles>` as it has them, for editing and saving.
    pub file: crate::table_style_edit::FileStyles,
}

/// A cell fill drawn as a pattern (`<patternFill patternType="darkDown">`):
/// pattern dots in `fg` over `bg`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FillPattern {
    /// Excel's name: `gray125`, `lightGray`, `darkDown`...
    pub kind: String,
    pub fg: Rgb,
    pub bg: Option<Rgb>,
}

impl FillPattern {
    /// The 8 × 8 pixel tile Excel draws, a row per byte (top bit left).
    pub fn tile(&self) -> [u8; 8] {
        let rep4 = |r: [u8; 4]| [r[0], r[1], r[2], r[3], r[0], r[1], r[2], r[3]];
        match self.kind.as_str() {
            "darkGray" => rep4([0x77, 0xDD, 0x77, 0xDD]),
            "mediumGray" => rep4([0xAA, 0x55, 0xAA, 0x55]),
            "lightGray" => rep4([0x88, 0x22, 0x88, 0x22]),
            "gray125" => rep4([0x88, 0x00, 0x22, 0x00]),
            "gray0625" => [0x88, 0, 0, 0, 0x22, 0, 0, 0],
            "darkHorizontal" => rep4([0xFF, 0xFF, 0x00, 0x00]),
            "darkVertical" => [0xCC; 8],
            "darkDown" => rep4([0xCC, 0x66, 0x33, 0x99]),
            "darkUp" => rep4([0x33, 0x66, 0xCC, 0x99]),
            "darkGrid" => rep4([0xFF, 0xFF, 0xCC, 0xCC]),
            "darkTrellis" => rep4([0xFF, 0x66, 0xFF, 0x99]),
            "lightHorizontal" => rep4([0xFF, 0x00, 0x00, 0x00]),
            "lightVertical" => [0x88; 8],
            "lightDown" => rep4([0x88, 0x44, 0x22, 0x11]),
            "lightUp" => rep4([0x11, 0x22, 0x44, 0x88]),
            "lightGrid" => rep4([0xFF, 0x88, 0x88, 0x88]),
            "lightTrellis" => rep4([0x88, 0x55, 0x22, 0x55]),
            _ => [0xFF; 8],
        }
    }
}

/// The patterned fills of a `styles.xml`, by fill index.
fn parse_patterns(root: Node, theme_: &Theme) -> Vec<Option<FillPattern>> {
    let Some(fills) = root.children().find(|n| n.tag_name().name() == "fills") else {
        return Vec::new();
    };
    fills
        .children()
        .filter(|n| n.tag_name().name() == "fill")
        .map(|f| {
            let p = f
                .children()
                .find(|n| n.tag_name().name() == "patternFill")?;
            let kind = p.attribute("patternType")?;
            if matches!(kind, "none" | "solid") {
                return None;
            }
            let color = |name: &str| {
                p.children()
                    .find(|n| n.tag_name().name() == name)
                    .and_then(|n| color_of(n, theme_))
            };
            Some(FillPattern {
                kind: kind.to_owned(),
                // Automatic: black dots on white.
                fg: color("fgColor").unwrap_or(0x000000),
                bg: color("bgColor"),
            })
        })
        .collect()
}

pub(crate) fn thin(color: Rgb) -> BorderLine {
    BorderLine {
        style: LineStyle::Thin,
        color: Some(color),
    }
}

pub(crate) fn line(style: LineStyle, color: Rgb) -> BorderLine {
    BorderLine {
        style,
        color: Some(color),
    }
}

/// A theme colour (`slot` as in OOXML: 0 lt1, 1 dk1, 4-9 accents) with a tint.
pub(crate) fn theme(theme_: &Theme, slot: i32, tint: f64) -> Rgb {
    parse_rgb(&Color::Theme(slot, tint).to_rgb(theme_)).unwrap_or(0)
}

/// Whether `name` is one of Excel's built-in table styles (any case).
pub(crate) fn is_builtin(name: &str) -> bool {
    builtin(&canonical_builtin(name), &Theme::default()).is_some()
}

/// `tablestylemedium2` → `TableStyleMedium2`.
fn canonical_builtin(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    for fam in ["Light", "Medium", "Dark"] {
        let p = format!("tablestyle{}", fam.to_ascii_lowercase());
        if let Some(n) = lower.strip_prefix(&p) {
            return format!("TableStyle{fam}{n}");
        }
    }
    name.to_owned()
}

/// The built-in style `name` (`TableStyleMedium2`...), coloured by `t`.
pub(crate) fn builtin(name: &str, t: &Theme) -> Option<TableStyleDef> {
    let (family, n) = if let Some(n) = name.strip_prefix("TableStyleLight") {
        ("light", n)
    } else if let Some(n) = name.strip_prefix("TableStyleMedium") {
        ("medium", n)
    } else {
        let n = name.strip_prefix("TableStyleDark")?;
        ("dark", n)
    };
    let n: u32 = n.parse().ok()?;
    let white = theme(t, 0, 0.0);
    let black = theme(t, 1, 0.0);
    // Colour index within a block of 7: 0 is dark 1 (grey shades), 1-6 the
    // accents.
    let (block, k) = ((n - 1) / 7, (n - 1) % 7);
    let accent = |tint: f64| {
        if k == 0 {
            // Excel's "black" variants use greys: dark 1 lightened.
            theme(
                t,
                1,
                if tint == 0.0 {
                    0.0
                } else {
                    tint.max(0.0) * 0.9 + 0.05
                },
            )
        } else {
            theme(t, 3 + k as i32, tint)
        }
    };
    let base = if k == 0 { black } else { accent(0.0) };
    let bold = Some(true);
    let mut s = TableStyleDef::empty();
    s.first_col.bold = bold;
    s.last_col.bold = bold;
    s.header.bold = bold;
    s.total.bold = bold;
    match (family, block) {
        // Light 1-7: lines above and below, tinted bands, accent text.
        ("light", 0) => {
            s.whole.top = Some(thin(base));
            s.whole.bottom = Some(thin(base));
            if k > 0 {
                s.whole.font = Some(accent(-0.25));
            }
            s.header.bottom = Some(thin(base));
            s.total.top = Some(thin(base));
            s.row_stripe1.fill = Some(accent(0.8));
            s.col_stripe1.fill = Some(accent(0.8));
        }
        // Light 8-14: filled header, outlined, bands as lines.
        ("light", 1) => {
            s.whole.left = Some(thin(base));
            s.whole.right = Some(thin(base));
            s.whole.top = Some(thin(base));
            s.whole.bottom = Some(thin(base));
            s.header.fill = Some(base);
            s.header.font = Some(white);
            s.total.top = Some(line(LineStyle::Double, base));
            s.row_stripe1.top = Some(thin(base));
            s.row_stripe1.bottom = Some(thin(base));
            s.col_stripe1.left = Some(thin(base));
            s.col_stripe1.right = Some(thin(base));
        }
        // Light 15-21: a grid of thin lines, tinted bands.
        ("light", _) => {
            let b = Some(thin(base));
            s.whole.left = b;
            s.whole.right = b;
            s.whole.top = b;
            s.whole.bottom = b;
            s.whole.horizontal = b;
            s.whole.vertical = b;
            s.header.bottom = Some(line(LineStyle::Medium, base));
            s.total.top = Some(line(LineStyle::Double, base));
            s.row_stripe1.fill = Some(accent(0.8));
            s.col_stripe1.fill = Some(accent(0.8));
        }
        // Medium 1-7: filled header, light lines between rows, tinted bands
        // (Medium 2 is Excel's default).
        ("medium", 0) => {
            let l = Some(thin(accent(0.4)));
            s.whole.left = l;
            s.whole.right = l;
            s.whole.top = l;
            s.whole.bottom = l;
            s.whole.horizontal = l;
            s.header.fill = Some(base);
            s.header.font = Some(white);
            s.total.top = Some(line(LineStyle::Double, base));
            s.row_stripe1.fill = Some(accent(0.8));
            s.col_stripe1.fill = Some(accent(0.8));
        }
        // Medium 8-14: tinted body with white lines, filled header and total.
        ("medium", 1) => {
            s.whole.fill = Some(accent(0.8));
            s.whole.horizontal = Some(thin(white));
            s.whole.vertical = Some(thin(white));
            s.header.fill = Some(base);
            s.header.font = Some(white);
            s.header.bottom = Some(line(LineStyle::Thick, white));
            s.total.fill = Some(base);
            s.total.font = Some(white);
            s.total.top = Some(line(LineStyle::Thick, white));
            s.first_col.fill = Some(base);
            s.first_col.font = Some(white);
            s.last_col.fill = Some(base);
            s.last_col.font = Some(white);
            s.row_stripe1.fill = Some(accent(0.6));
            s.col_stripe1.fill = Some(accent(0.6));
        }
        // Medium 15-21: dark lines, filled header, grey bands.
        ("medium", 2) => {
            let d = Some(thin(black));
            s.whole.top = d;
            s.whole.bottom = d;
            s.whole.horizontal = Some(thin(theme(t, 1, 0.65)));
            s.header.fill = Some(base);
            s.header.font = Some(white);
            s.header.bottom = Some(line(LineStyle::Medium, black));
            s.total.top = Some(line(LineStyle::Double, black));
            s.row_stripe1.fill = Some(theme(t, 0, -0.15));
            s.col_stripe1.fill = Some(theme(t, 0, -0.15));
        }
        // Medium 22-28: tinted body with accent lines, bold plain header.
        ("medium", _) => {
            let l = Some(thin(accent(0.4)));
            s.whole.fill = Some(accent(0.8));
            s.whole.left = l;
            s.whole.right = l;
            s.whole.top = l;
            s.whole.bottom = l;
            s.whole.horizontal = l;
            s.whole.vertical = l;
            s.total.top = Some(line(LineStyle::Double, base));
            s.row_stripe1.fill = Some(accent(0.6));
            s.col_stripe1.fill = Some(accent(0.6));
        }
        // Dark 1-7: dark body, black header, darker bands, white text.
        ("dark", 0) => {
            let body = if k == 0 {
                theme(t, 1, 0.35)
            } else {
                accent(-0.25)
            };
            let band = if k == 0 {
                theme(t, 1, 0.15)
            } else {
                accent(-0.5)
            };
            s.whole.fill = Some(body);
            s.whole.font = Some(white);
            s.header.fill = Some(black);
            s.header.font = Some(white);
            s.header.bottom = Some(line(LineStyle::Medium, white));
            s.total.fill = Some(band);
            s.total.font = Some(white);
            s.total.top = Some(line(LineStyle::Double, white));
            s.first_col.fill = Some(band);
            s.first_col.font = Some(white);
            s.last_col.fill = Some(band);
            s.last_col.font = Some(white);
            s.row_stripe1.fill = Some(band);
            s.col_stripe1.fill = Some(band);
        }
        // Dark 8-11: tinted body, black header.
        ("dark", _) => {
            let slot = match n {
                8 => None,
                9 => Some(4),
                10 => Some(6),
                _ => Some(8),
            };
            let tinted =
                |tint: f64| slot.map_or_else(|| theme(t, 0, -tint * 0.3), |s| theme(t, s, tint));
            s.whole.fill = Some(tinted(0.8));
            s.header.fill = Some(black);
            s.header.font = Some(white);
            s.total.top = Some(line(LineStyle::Double, black));
            s.row_stripe1.fill = Some(tinted(0.6));
            s.col_stripe1.fill = Some(tinted(0.6));
        }
        _ => return None,
    }
    Some(s)
}

// ---------------------------------------------------------------- custom styles

/// The custom table styles of a package's `styles.xml`.
pub(crate) fn read_custom_styles(
    package: impl std::io::Read + std::io::Seek,
    theme_: &Theme,
) -> CustomStyles {
    let xml = (|| {
        let mut zip = zip::ZipArchive::new(package).ok()?;
        let mut part = zip.by_name("xl/styles.xml").ok()?;
        if part.size() > 64 * 1024 * 1024 {
            return None;
        }
        let mut s = String::new();
        part.read_to_string(&mut s).ok()?;
        Some(s)
    })();
    xml.map(|x| parse_custom_styles(&x, theme_))
        .unwrap_or_default()
}

pub(crate) fn parse_custom_styles(xml: &str, theme_: &Theme) -> CustomStyles {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return CustomStyles::default();
    };
    let root = doc.root_element();
    let dxf_nodes: Vec<Node> = root
        .children()
        .find(|n| n.tag_name().name() == "dxfs")
        .map(|d| {
            d.children()
                .filter(|n| n.tag_name().name() == "dxf")
                .collect()
        })
        .unwrap_or_default();
    let dxfs: Vec<Elem> = dxf_nodes.iter().map(|n| parse_dxf(*n, theme_)).collect();
    let mut styles = HashMap::new();
    let mut file = crate::table_style_edit::FileStyles::default();
    if let Some(ts) = root
        .children()
        .find(|n| n.tag_name().name() == "tableStyles")
    {
        file.element = crate::preserve::verbatim_pub(ts, xml).ok();
        file.default_table = ts.attribute("defaultTableStyle").map(str::to_owned);
        file.default_pivot = ts.attribute("defaultPivotStyle").map(str::to_owned);
        for style in ts
            .children()
            .filter(|n| n.tag_name().name() == "tableStyle")
        {
            let Some(name) = style.attribute("name") else {
                continue;
            };
            let table = style.attribute("table") != Some("0");
            let mut def = TableStyleDef::empty();
            let mut elements = Vec::new();
            for el in style
                .children()
                .filter(|n| n.tag_name().name() == "tableStyleElement")
            {
                let kind = el.attribute("type").unwrap_or_default();
                let size = el.attribute("size").and_then(|s| s.parse().ok());
                let id = el.attribute("dxfId").and_then(|d| d.parse::<usize>().ok());
                let dxf_xml = id
                    .and_then(|d| dxf_nodes.get(d))
                    .and_then(|n| crate::preserve::verbatim_pub(*n, xml).ok());
                if let Some(dxf) = id.and_then(|d| dxfs.get(d)) {
                    def.set(kind, dxf.clone(), size.unwrap_or(1));
                }
                elements.push(crate::table_style_edit::FileElement { id, dxf_xml });
            }
            if table {
                styles.insert(name.to_owned(), def.clone());
            }
            file.styles.push(crate::table_style_edit::FileStyle {
                name: name.to_owned(),
                xml: crate::preserve::verbatim_pub(style, xml).unwrap_or_default(),
                table,
                pivot: style.attribute("pivot") != Some("0"),
                def,
                elements,
            });
        }
    }
    CustomStyles {
        styles,
        patterns: parse_patterns(root, theme_),
        file,
    }
}

impl TableStyleDef {
    /// Set element `kind` (`headerRow`...) to `e`; `size` is a stripe's.
    pub(crate) fn set(&mut self, kind: &str, e: Elem, size: u32) {
        let size = size.max(1);
        match kind {
            "wholeTable" => self.whole = e,
            "headerRow" => self.header = e,
            "totalRow" => self.total = e,
            "firstColumn" => self.first_col = e,
            "lastColumn" => self.last_col = e,
            "firstRowStripe" => {
                self.row_stripe1 = e;
                self.row_stripe_size = size;
            }
            "secondRowStripe" => {
                self.row_stripe2 = e;
                self.row_stripe2_size = size;
            }
            "firstColumnStripe" => {
                self.col_stripe1 = e;
                self.col_stripe_size = size;
            }
            "secondColumnStripe" => {
                self.col_stripe2 = e;
                self.col_stripe2_size = size;
            }
            "firstHeaderCell" => self.first_header = e,
            "lastHeaderCell" => self.last_header = e,
            "firstTotalCell" => self.first_total = e,
            "lastTotalCell" => self.last_total = e,
            k if crate::table_style_edit::TableElement::of_xml(k).is_some() => {
                self.extra.retain(|(x, _, _)| x != k);
                self.extra.push((k.to_owned(), e, size));
            }
            _ => {}
        }
    }

    /// Element `kind` and its stripe size.
    pub(crate) fn get(&self, kind: &str) -> Option<(&Elem, u32)> {
        Some(match kind {
            "wholeTable" => (&self.whole, 1),
            "headerRow" => (&self.header, 1),
            "totalRow" => (&self.total, 1),
            "firstColumn" => (&self.first_col, 1),
            "lastColumn" => (&self.last_col, 1),
            "firstRowStripe" => (&self.row_stripe1, self.row_stripe_size),
            "secondRowStripe" => (&self.row_stripe2, self.row_stripe2_size),
            "firstColumnStripe" => (&self.col_stripe1, self.col_stripe_size),
            "secondColumnStripe" => (&self.col_stripe2, self.col_stripe2_size),
            "firstHeaderCell" => (&self.first_header, 1),
            "lastHeaderCell" => (&self.last_header, 1),
            "firstTotalCell" => (&self.first_total, 1),
            "lastTotalCell" => (&self.last_total, 1),
            k => {
                let (_, e, size) = self.extra.iter().find(|(x, _, _)| x == k)?;
                (e, *size)
            }
        })
    }
}

fn color_of(n: Node, theme_: &Theme) -> Option<Rgb> {
    if let Some(rgb) = n.attribute("rgb") {
        let hex = rgb.get(rgb.len().saturating_sub(6)..)?;
        return u32::from_str_radix(hex, 16).ok();
    }
    if let Some(slot) = n.attribute("theme").and_then(|s| s.parse::<i32>().ok()) {
        let tint = n
            .attribute("tint")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        return Some(theme(theme_, slot, tint));
    }
    if let Some(i) = n.attribute("indexed").and_then(|s| s.parse::<i32>().ok()) {
        return match i {
            8 | 64 => Some(0x000000),
            9 | 65 => Some(0xFFFFFF),
            _ => None,
        };
    }
    None
}

fn parse_dxf(dxf: Node, theme_: &Theme) -> Elem {
    let mut e = Elem::default();
    for part in dxf.children().filter(|n| n.is_element()) {
        match part.tag_name().name() {
            "font" => {
                for f in part.children().filter(|n| n.is_element()) {
                    match f.tag_name().name() {
                        "b" => {
                            e.bold =
                                Some(f.attribute("val").is_none_or(|v| v != "0" && v != "false"))
                        }
                        "i" => {
                            e.italic =
                                Some(f.attribute("val").is_none_or(|v| v != "0" && v != "false"))
                        }
                        "strike" => {
                            e.strike =
                                Some(f.attribute("val").is_none_or(|v| v != "0" && v != "false"))
                        }
                        "u" => e.underline = Some(f.attribute("val") != Some("none")),
                        "color" => e.font = color_of(f, theme_),
                        _ => {}
                    }
                }
            }
            "fill" => {
                if let Some(p) = part
                    .children()
                    .find(|n| n.tag_name().name() == "patternFill")
                {
                    // A dxf's solid fill colour is its bgColor.
                    let bg = p.children().find(|n| n.tag_name().name() == "bgColor");
                    let fg = p.children().find(|n| n.tag_name().name() == "fgColor");
                    e.fill = bg.or(fg).and_then(|c| color_of(c, theme_));
                }
            }
            "border" => {
                for b in part.children().filter(|n| n.is_element()) {
                    let Some(style) = b.attribute("style") else {
                        continue;
                    };
                    let Some(style) = LineStyle::from_xml(style) else {
                        continue;
                    };
                    let color = b
                        .children()
                        .find(|n| n.tag_name().name() == "color")
                        .and_then(|c| color_of(c, theme_));
                    let l = Some(BorderLine { style, color });
                    match b.tag_name().name() {
                        "left" | "start" => e.left = l,
                        "right" | "end" => e.right = l,
                        "top" => e.top = l,
                        "bottom" => e.bottom = l,
                        "horizontal" => e.horizontal = l,
                        "vertical" => e.vertical = l,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    e
}

// ---------------------------------------------------------------- per-cell look

impl TableCellLook {
    pub(crate) fn apply(&mut self, e: &Elem, edges: Edges) {
        if e.fill.is_some() {
            self.fill = e.fill;
        }
        if e.font.is_some() {
            self.font_color = e.font;
        }
        if let Some(b) = e.bold {
            self.bold = b;
        }
        if let Some(b) = e.italic {
            self.italic = b;
        }
        if let Some(b) = e.underline {
            self.underline = b;
        }
        if let Some(b) = e.strike {
            self.strike = b;
        }
        let pick = |outer: &Option<BorderLine>, inner: &Option<BorderLine>, at_edge: bool| {
            if at_edge { *outer } else { *inner }
        };
        if let Some(l) = pick(&e.top, &e.horizontal, edges.top) {
            self.top = Some(l);
        }
        if let Some(l) = pick(&e.bottom, &e.horizontal, edges.bottom) {
            self.bottom = Some(l);
        }
        if let Some(l) = pick(&e.left, &e.vertical, edges.left) {
            self.left = Some(l);
        }
        if let Some(l) = pick(&e.right, &e.vertical, edges.right) {
            self.right = Some(l);
        }
    }
}

/// A table's own differential formats: its header, data and total rows'
/// (`headerRowDxfId`, `dataDxfId`, `totalsRowDxfId`) and each column's.
/// Excel draws them over the table style and under the cells' own
/// formatting; a column's over the table's.
#[derive(Debug, Clone, Default)]
pub(crate) struct TableDxfs {
    header: Option<Elem>,
    data: Option<Elem>,
    totals: Option<Elem>,
    /// Header, data, total row of each column.
    cols: Vec<[Option<Elem>; 3]>,
}

impl TableDxfs {
    fn of(t: &Table, dxfs: &[ironcalc_base::types::Dxf], theme: &Theme) -> Self {
        let get = |id: Option<u32>| {
            id.and_then(|i| dxfs.get(i as usize))
                .map(|d| elem_of_dxf(d, theme))
                .filter(|e| !e.is_empty())
        };
        let cols: Vec<[Option<Elem>; 3]> = t
            .columns
            .iter()
            .map(|c| {
                [
                    get(c.header_row_dxf_id),
                    get(c.data_dxf_id),
                    get(c.totals_row_dxf_id),
                ]
            })
            .collect();
        Self {
            header: get(t.header_row_dxf_id),
            data: get(t.data_dxf_id),
            totals: get(t.totals_row_dxf_id),
            cols: if cols.iter().all(|c| c.iter().all(Option::is_none)) {
                Vec::new()
            } else {
                cols
            },
        }
    }

    fn is_empty(&self) -> bool {
        self.header.is_none()
            && self.data.is_none()
            && self.totals.is_none()
            && self.cols.is_empty()
    }

    fn apply(&self, t: &TableInfo, row: u32, col: u32, look: &mut TableCellLook) {
        if self.is_empty() {
            return;
        }
        let a = t.area;
        let (dt, db) = t.data_rows();
        let (part, first, last) = if t.header_row() == Some(row) {
            (0, row, row)
        } else if t.totals_row() == Some(row) {
            (2, row, row)
        } else {
            (1, dt, db)
        };
        let table = [&self.header, &self.data, &self.totals][part];
        if let Some(e) = table {
            look.apply(
                e,
                Edges {
                    top: row == first,
                    bottom: row == last,
                    left: col == a.left,
                    right: col == a.right,
                },
            );
        }
        if let Some(Some(e)) = self.cols.get((col - a.left) as usize).map(|c| &c[part]) {
            look.apply(
                e,
                Edges {
                    top: row == first,
                    bottom: row == last,
                    left: true,
                    right: true,
                },
            );
        }
    }
}

/// A differential format as a table element.
fn elem_of_dxf(d: &ironcalc_base::types::Dxf, theme: &Theme) -> Elem {
    use crate::format::resolve;
    let line = |b: &Option<ironcalc_base::types::BorderItem>| {
        b.as_ref().map(|b| BorderLine {
            style: LineStyle::from_ironcalc(&b.style),
            color: resolve(&b.color, theme),
        })
    };
    let font = d.font.as_ref();
    let border = d.border.as_ref();
    Elem {
        fill: d.fill.as_ref().and_then(|f| resolve(&f.color, theme)),
        font: font.and_then(|f| resolve(&f.color, theme)),
        bold: font.and_then(|f| f.b),
        italic: font.and_then(|f| f.i),
        underline: font.and_then(|f| f.u),
        strike: font.and_then(|f| f.strike),
        left: border.and_then(|b| line(&b.left)),
        right: border.and_then(|b| line(&b.right)),
        top: border.and_then(|b| line(&b.top)),
        bottom: border.and_then(|b| line(&b.bottom)),
        horizontal: None,
        vertical: None,
    }
}

/// Whether a cell is on the outer edges of the element it is styled by.
#[derive(Clone, Copy)]
pub(crate) struct Edges {
    pub(crate) top: bool,
    pub(crate) bottom: bool,
    pub(crate) left: bool,
    pub(crate) right: bool,
}

impl TableStyleDef {
    /// [`Self::look`] for a table with no hidden rows.
    pub(crate) fn look_pub(&self, t: &TableInfo, row: u32, col: u32) -> TableCellLook {
        self.look(t, row, col, &[])
    }

    /// The look of the cell at `(row, col)` of table `t`.
    fn look(&self, t: &TableInfo, row: u32, col: u32, hidden: &[u32]) -> TableCellLook {
        let a = t.area;
        let mut look = TableCellLook::default();
        let (dt, db) = t.data_rows();
        let is_header = t.header_row() == Some(row);
        let is_total = t.totals_row() == Some(row);
        let is_data = !is_header && !is_total;
        look.apply(
            &self.whole,
            Edges {
                top: row == a.top,
                bottom: row == a.bottom,
                left: col == a.left,
                right: col == a.right,
            },
        );
        if t.column_stripes && is_data {
            let (i, first, size) =
                stripe(col - a.left, self.col_stripe_size, self.col_stripe2_size);
            let e = if i == 0 {
                &self.col_stripe1
            } else {
                &self.col_stripe2
            };
            let first = a.left + first;
            look.apply(
                e,
                Edges {
                    top: row == dt,
                    bottom: row == db,
                    left: col == first,
                    right: col == (first + size - 1).min(a.right),
                },
            );
        }
        if t.row_stripes && is_data {
            // Bands are counted over the rows shown, as in Excel (a filter
            // keeps them alternating).
            let skipped =
                hidden.partition_point(|&h| h < row) - hidden.partition_point(|&h| h < dt);
            let shown = row - dt - skipped as u32;
            let (i, first, size) = stripe(shown, self.row_stripe_size, self.row_stripe2_size);
            let e = if i == 0 {
                &self.row_stripe1
            } else {
                &self.row_stripe2
            };
            look.apply(
                e,
                Edges {
                    top: shown == first,
                    bottom: shown == first + size - 1 || row == db,
                    left: col == a.left,
                    right: col == a.right,
                },
            );
        }
        let col_edges = Edges {
            top: row == a.top,
            bottom: row == a.bottom,
            left: true,
            right: true,
        };
        if t.last_column && col == a.right {
            look.apply(&self.last_col, col_edges);
        }
        if t.first_column && col == a.left {
            look.apply(&self.first_col, col_edges);
        }
        let row_edges = Edges {
            top: true,
            bottom: true,
            left: col == a.left,
            right: col == a.right,
        };
        let cell = Edges {
            top: true,
            bottom: true,
            left: true,
            right: true,
        };
        if is_header {
            look.apply(&self.header, row_edges);
            if t.first_column && col == a.left {
                look.apply(&self.first_header, cell);
            }
            if t.last_column && col == a.right {
                look.apply(&self.last_header, cell);
            }
        }
        if is_total {
            look.apply(&self.total, row_edges);
            if t.first_column && col == a.left {
                look.apply(&self.first_total, cell);
            }
            if t.last_column && col == a.right {
                look.apply(&self.last_total, cell);
            }
        }
        look
    }
}

/// Which stripe position `n` (0-based) is in, for stripes of `size1` then
/// `size2`: `(0 or 1, where that stripe starts, its size)`.
fn stripe(n: u32, size1: u32, size2: u32) -> (u32, u32, u32) {
    let (s1, s2) = (size1.max(1), size2.max(1));
    let cycle = s1 + s2;
    let base = n / cycle * cycle;
    if n - base < s1 {
        (0, base, s1)
    } else {
        (1, base + s1, s2)
    }
}

impl Workbook {
    /// The tables of `sheet`, with their styles, for the current state.
    fn sheet_tables(&self, sheet: u32) -> std::sync::Arc<SheetTables> {
        if let Some(t) = self.cached(|c| c.tables.get(&sheet).cloned()) {
            return t;
        }
        let model = self.model.get_model();
        let mut hidden: Vec<u32> = Vec::new();
        let theme = &model.workbook.theme;
        let dxf_list = &model.workbook.styles.dxfs;
        let mut v: Vec<((TableInfo, Option<TableStyleDef>), TableDxfs)> =
            match model.workbook.worksheet(sheet) {
                Ok(ws) => model
                    .workbook
                    .tables
                    .values()
                    .filter(|t| t.sheet_name == ws.name)
                    .filter_map(|t| {
                        Some((
                            TableInfo::from_ironcalc(t)?,
                            TableDxfs::of(t, dxf_list, theme),
                        ))
                    })
                    .map(|(t, d)| {
                        let style = t.style.as_deref().and_then(|n| self.table_style(n));
                        ((t, style), d)
                    })
                    .collect(),
                Err(_) => Vec::new(),
            };
        v.sort_by_key(|((t, _), _)| (t.area.top, t.area.left));
        if !v.is_empty() {
            hidden = self.hidden_rows(sheet).into_iter().collect();
            hidden.sort_unstable();
        }
        let (v, dxfs): (Vec<_>, Vec<_>) = v.into_iter().unzip();
        let rc = std::sync::Arc::new(SheetTables {
            tables: v,
            dxfs,
            hidden,
        });
        self.cached(|c| c.tables.insert(sheet, rc.clone()));
        rc
    }

    /// The tables of `sheet`.
    pub fn tables(&self, sheet: u32) -> Vec<TableInfo> {
        self.sheet_tables(sheet)
            .tables
            .iter()
            .map(|(t, _)| t.clone())
            .collect()
    }

    /// The table `(row, col)` of `sheet` is in.
    pub fn table_at(&self, sheet: u32, row: u32, col: u32) -> Option<TableInfo> {
        self.sheet_tables(sheet)
            .tables
            .iter()
            .find(|(t, _)| t.area.contains(row, col))
            .map(|(t, _)| t.clone())
    }

    /// The look the table style gives cell `(row, col)`, if it is in a table
    /// with a style.
    /// The same for a pivot table's cells and its pivot table style.
    pub fn table_cell_look(&self, sheet: u32, row: u32, col: u32) -> Option<TableCellLook> {
        let tables = self.sheet_tables(sheet);
        match tables
            .tables
            .iter()
            .position(|(t, _)| t.area.contains(row, col))
        {
            Some(i) => {
                let (t, style) = &tables.tables[i];
                let dxfs = &tables.dxfs[i];
                if style.is_none() && dxfs.is_empty() {
                    return None;
                }
                let mut look = style
                    .as_ref()
                    .map(|s| s.look(t, row, col, &tables.hidden))
                    .unwrap_or_default();
                // The table's own formats over its style (Excel's table
                // dxfs), under the cell's own formatting.
                dxfs.apply(t, row, col, &mut look);
                Some(look)
            }
            None => self.pivot_cell_look(sheet, row, col),
        }
    }

    /// A table style by name: the workbook's own, else a built-in one.
    pub(crate) fn table_style(&self, name: &str) -> Option<TableStyleDef> {
        if let Some(s) = self.custom_table_style(name) {
            return Some(s);
        }
        builtin(name, &self.model.get_model().workbook.theme)
    }
}

impl Workbook {
    /// Whether row `row` (0-based) of `sheet` is hidden.
    pub fn row_hidden(&self, sheet: u32, row: u32) -> bool {
        self.model
            .get_model()
            .workbook
            .worksheet(sheet)
            .is_ok_and(|ws| ws.rows.iter().any(|r| r.r == row as i32 + 1 && r.hidden))
    }

    /// The table that typing `value` into the empty cell `(row, col)` would
    /// extend: one without a total row whose last row is just above.
    pub(crate) fn table_growth(
        &self,
        sheet: u32,
        row: u32,
        col: u32,
        value: &str,
    ) -> Option<TableInfo> {
        if value.is_empty() || row == 0 || self.is_filled(sheet, row, col) {
            return None;
        }
        let t = self.table_at(sheet, row - 1, col)?;
        (t.totals_rows == 0 && t.area.bottom == row - 1 && self.table_at(sheet, row, col).is_none())
            .then_some(t)
    }

    /// Type `value` into header cell `(row, col)` of table `t`, renaming its
    /// column (a name already taken gets a number, an empty one becomes
    /// `ColumnN`, as in Excel). One undo step.
    pub(crate) fn edit_table_header(
        &mut self,
        sheet: u32,
        t: &TableInfo,
        row: u32,
        col: u32,
        value: &str,
    ) -> anyhow::Result<()> {
        use anyhow::anyhow;
        let index = (col - t.area.left) as usize;
        let old = t.columns[index].clone();
        let taken = |name: &str| {
            t.columns
                .iter()
                .enumerate()
                .any(|(i, c)| i != index && c.to_lowercase() == name.to_lowercase())
        };
        let wanted = value.trim();
        let base = if wanted.is_empty() || wanted.starts_with('=') {
            crate::content_text::trf("Column{}", &[&(index + 1)])
        } else {
            wanted.to_string()
        };
        let mut name = base.clone();
        let mut n = 2;
        while taken(&name) {
            name = format!("{base}{n}");
            n += 1;
        }
        let shown = if wanted == name {
            value.to_string()
        } else {
            name.clone()
        };
        self.model.pause_evaluation();
        let mut steps = 0;
        let r = (|| -> anyhow::Result<()> {
            self.model
                .set_user_input(sheet, row as i32 + 1, col as i32 + 1, &shown)
                .map_err(|e| anyhow!("{e}"))?;
            steps += 1;
            if name != old {
                self.model
                    .kantor_rename_table_column(&t.name, &old, &name)
                    .map_err(|e| anyhow!("{e}"))?;
                steps += 1;
            }
            Ok(())
        })();
        self.end_batch(steps);
        r
    }

    /// Type `value` into `(row, col)` and extend table `t` over the row,
    /// filling its calculated columns (formula cells above) down. One undo
    /// step.
    pub(crate) fn grow_table(
        &mut self,
        sheet: u32,
        t: TableInfo,
        row: u32,
        col: u32,
        value: &str,
    ) -> anyhow::Result<()> {
        use anyhow::anyhow;
        let mut table = self
            .model
            .get_model()
            .workbook
            .tables
            .get(&t.name)
            .cloned()
            .ok_or_else(|| anyhow!("no table {}", t.name))?;
        table.reference = ironcalc_base::kantor_tables::table_reference(
            t.area.top as i32 + 1,
            t.area.left as i32 + 1,
            row as i32 + 1,
            t.area.right as i32 + 1,
        );
        if let Some(xml) = table.kantor_auto_filter.take() {
            let (_, last) = (t.area.top, row);
            table.kantor_auto_filter = Some(ironcalc_base::kantor_tables::with_auto_filter_ref(
                &xml,
                &format!(
                    "{}:{}",
                    crate::cell_name(t.area.top, t.area.left),
                    crate::cell_name(last, t.area.right)
                ),
            ));
        }
        let formulas: Vec<u32> = (t.area.left..=t.area.right)
            .filter(|&c| c != col && !self.is_filled(sheet, row, c))
            .filter(|&c| self.input(sheet, row - 1, c).starts_with('='))
            .collect();
        self.model.pause_evaluation();
        let mut steps = 0;
        let r = (|| -> anyhow::Result<()> {
            self.model
                .set_user_input(sheet, row as i32 + 1, col as i32 + 1, value)
                .map_err(|e| anyhow!("{e}"))?;
            steps += 1;
            self.model
                .kantor_set_table(&t.name, Some(table))
                .map_err(|e| anyhow!("{e}"))?;
            steps += 1;
            for c in formulas {
                let area = CellRange::from_corners((row - 1, c), (row - 1, c)).area(sheet);
                if self.model.auto_fill_rows(&area, row as i32 + 1).is_ok() {
                    steps += 1;
                }
            }
            Ok(())
        })();
        self.end_batch(steps);
        r
    }
}

impl Workbook {
    /// The pattern of the cell's fill, when the file gave it a patterned
    /// fill (Excel prints it as a pattern; IronCalc keeps one colour).
    pub fn fill_pattern(&self, sheet: u32, row: u32, col: u32) -> Option<FillPattern> {
        let index = self.style_index(sheet, row, col);
        let xf = self
            .model
            .get_model()
            .workbook
            .styles
            .cell_xfs
            .get(usize::try_from(index).ok()?)?;
        self.table_styles
            .patterns
            .get(usize::try_from(xf.fill_id).ok()?)?
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn medium2_uses_the_theme_accent() {
        let t = Theme::default();
        let s = builtin("TableStyleMedium2", &t).unwrap();
        // Accent 1 of the Office theme, and its 80% lighter band.
        assert_eq!(s.header.fill, Some(theme(&t, 4, 0.0)));
        assert_eq!(s.header.font, Some(theme(&t, 0, 0.0)));
        assert_eq!(s.row_stripe1.fill, Some(theme(&t, 4, 0.8)));
        assert!(builtin("Nope", &t).is_none());
        for n in 1..=21 {
            assert!(builtin(&format!("TableStyleLight{n}"), &t).is_some());
        }
        for n in 1..=28 {
            assert!(builtin(&format!("TableStyleMedium{n}"), &t).is_some());
        }
        for n in 1..=11 {
            assert!(builtin(&format!("TableStyleDark{n}"), &t).is_some());
        }
    }

    #[test]
    fn custom_styles_read_their_dxfs() {
        let xml = r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<dxfs count="2">
<dxf><font><b/><color rgb="FFFFFFFF"/></font><fill><patternFill><bgColor rgb="FF7030A0"/></patternFill></fill></dxf>
<dxf><fill><patternFill><bgColor theme="4" tint="0.79998"/></patternFill></fill><border><bottom style="thin"><color rgb="FF000000"/></bottom></border></dxf>
</dxfs>
<tableStyles count="1" defaultTableStyle="TableStyleMedium2"><tableStyle name="Purple" pivot="0" count="2">
<tableStyleElement type="headerRow" dxfId="0"/><tableStyleElement type="firstRowStripe" dxfId="1" size="2"/>
</tableStyle></tableStyles></styleSheet>"#;
        let t = Theme::default();
        let c = parse_custom_styles(xml, &t);
        let s = &c.styles["Purple"];
        assert_eq!(s.header.fill, Some(0x7030A0));
        assert_eq!(s.header.font, Some(0xFFFFFF));
        assert_eq!(s.header.bold, Some(true));
        assert_eq!(s.row_stripe_size, 2);
        assert_eq!(s.row_stripe1.fill, Some(theme(&t, 4, 0.79998)));
        assert!(s.row_stripe1.bottom.is_some());
    }

    impl TableStyleDef {
        fn look_all(&self, t: &TableInfo, row: u32, col: u32) -> TableCellLook {
            self.look(t, row, col, &[])
        }
    }

    #[test]
    fn bands_skip_hidden_rows() {
        let t = Theme::default();
        let s = builtin("TableStyleMedium2", &t).unwrap();
        let info = TableInfo {
            name: "T".into(),
            area: CellRange {
                top: 0,
                left: 0,
                bottom: 5,
                right: 1,
            },
            header_rows: 1,
            totals_rows: 0,
            columns: vec!["a".into(), "b".into()],
            style: Some("TableStyleMedium2".into()),
            first_column: false,
            last_column: false,
            row_stripes: true,
            column_stripes: false,
            filter_buttons: true,
        };
        let band = Some(theme(&t, 4, 0.8));
        // Row 2 hidden: rows 1 and 3 are the first two shown.
        assert_eq!(s.look(&info, 1, 0, &[2]).fill, band);
        assert_eq!(s.look(&info, 3, 0, &[2]).fill, None);
        assert_eq!(s.look(&info, 4, 0, &[2]).fill, band);
    }

    #[test]
    fn banding_alternates_and_the_total_row_has_its_line() {
        let t = Theme::default();
        let s = builtin("TableStyleMedium2", &t).unwrap();
        let info = TableInfo {
            name: "T".into(),
            area: CellRange {
                top: 0,
                left: 0,
                bottom: 5,
                right: 2,
            },
            header_rows: 1,
            totals_rows: 1,
            columns: vec!["a".into(), "b".into(), "c".into()],
            style: Some("TableStyleMedium2".into()),
            first_column: false,
            last_column: false,
            row_stripes: true,
            column_stripes: false,
            filter_buttons: true,
        };
        let band = theme(&t, 4, 0.8);
        assert_eq!(s.look_all(&info, 0, 1).fill, Some(theme(&t, 4, 0.0)));
        assert!(s.look_all(&info, 0, 1).bold);
        assert_eq!(s.look_all(&info, 1, 1).fill, Some(band));
        assert_eq!(s.look_all(&info, 2, 1).fill, None);
        assert_eq!(s.look_all(&info, 3, 1).fill, Some(band));
        let total = s.look_all(&info, 5, 1);
        assert!(total.bold);
        assert_eq!(total.top.unwrap().style, LineStyle::Double);
    }
}
