//! Pictures, charts and shapes of OpenDocument spreadsheets (`.ods`).
//!
//! [`crate::ods`] finds the drawing objects of each table: the ones in a
//! `table:table-cell` are anchored to that cell (their `svg:x`/`svg:y`
//! are offsets into it, `table:end-cell-address` with `table:end-x`/`y`
//! gives the bottom right corner when present), the ones in `table:shapes`
//! to the sheet (`svg:x`/`svg:y` from its top left). This module turns
//! them into the objects of [`crate::xls_drawing`], whose
//! [`package`](crate::xls_drawing::package) writes the hidden `.xlsx`
//! package an import is shown and saved from:
//!
//! * `draw:frame` with `draw:image`: PNG and JPEG pictures from the
//!   package (`Pictures/…`); other formats are left out with an import note.
//! * `draw:frame` with `draw:object`: a chart (`Object N/content.xml` with
//!   `chart:chart`): bar and column (clustered, stacked, 100 %), line,
//!   area, pie, ring, scatter, with series ranges, names, categories,
//!   colours, data labels, title, legend, axis titles, gridlines and axis
//!   scaling. Radar, bubble, stock and other charts and embedded objects
//!   that aren't charts are left out with a note; 3-D charts are drawn flat.
//! * `draw:custom-shape` (LibreOffice's shapes and the `ooxml-*` presets it
//!   keeps from Excel), `draw:rect`, `draw:ellipse`/`draw:circle`,
//!   `draw:line`, `draw:connector` and text frames (`draw:text-box`):
//!   preset geometry, fill (solid, two-colour gradient, none), outline
//!   (colour, width, dashes, arrowheads), rotation and plain text with its
//!   size, weight, slant, colour and alignment. Groups are brought in as
//!   their shapes. Freeform shapes (paths, polygons), form controls and
//!   unknown custom shapes are left out with a note.

use std::collections::HashMap;
use std::io::Read as _;

use roxmltree::{Document, Node};

use crate::drawing::{Anchor, EMU_PER_PX, EditAs, Marker};
use crate::xls_drawing::{
    Blip, Chart, Labels, Object, ObjectKind, Plot, PlotKind, Scale, Series, Skipped, Text, esc,
};

pub(crate) const NS_DRAW: &str = "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0";
const NS_SVG: &str = "urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0";
const NS_TABLE: &str = "urn:oasis:names:tc:opendocument:xmlns:table:1.0";
const NS_XLINK: &str = "http://www.w3.org/1999/xlink";
const NS_STYLE: &str = "urn:oasis:names:tc:opendocument:xmlns:style:1.0";
const NS_FO: &str = "urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0";
const NS_TEXT: &str = "urn:oasis:names:tc:opendocument:xmlns:text:1.0";
const NS_CHART: &str = "urn:oasis:names:tc:opendocument:xmlns:chart:1.0";
const NS_DRAWOOO: &str = "http://openoffice.org/2010/draw";
const NS_LOEXT: &str = "urn:org:documentfoundation:names:experimental:office:xmlns:loext:1.0";

/// A length (`1.85in`, `2.5cm`, `10pt`...) in px (96 per inch).
fn px(s: &str) -> Option<f64> {
    let s = s.trim();
    let split = s.find(|c: char| c.is_ascii_alphabetic() || c == '%')?;
    let (n, unit) = s.split_at(split);
    let n: f64 = n.trim().parse().ok()?;
    let k = match unit {
        "pt" => 96.0 / 72.0,
        "in" => 96.0,
        "cm" => 96.0 / 2.54,
        "mm" => 96.0 / 25.4,
        "pc" => 16.0,
        "px" => 1.0,
        _ => return None,
    };
    let v = n * k;
    v.is_finite().then_some(v)
}

fn emu(px: f64) -> i64 {
    (px * EMU_PER_PX).round() as i64
}

/// A 0-based column and row of an ODF cell address (`Sheet1.B2`,
/// `$'My sheet'.$B$2`), ignoring the sheet.
fn cell(addr: &str) -> Option<(u32, u32)> {
    let cell = match rfind_outside_quotes(addr, '.') {
        Some(dot) => &addr[dot + 1..],
        None => addr,
    };
    let cell = cell.replace('$', "");
    let split = cell.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = cell.split_at(split);
    if letters.is_empty() {
        return None;
    }
    let mut col = 0u32;
    for ch in letters.chars() {
        let ch = ch.to_ascii_uppercase();
        if !ch.is_ascii_uppercase() {
            return None;
        }
        col = col
            .checked_mul(26)?
            .checked_add(ch as u32 - 'A' as u32 + 1)?;
    }
    let row: u32 = digits.parse().ok()?;
    (row >= 1 && col >= 1).then(|| (col - 1, row - 1))
}

fn rfind_outside_quotes(s: &str, needle: char) -> Option<usize> {
    let mut in_quote = false;
    let mut found = None;
    for (i, c) in s.char_indices() {
        if c == '\'' {
            in_quote = !in_quote;
        } else if c == needle && !in_quote {
            found = Some(i);
        }
    }
    found
}

/// A sheet name as written in an ODF address (`'It''s'` → `It's`).
fn unquote_sheet(s: &str) -> String {
    let s = s.trim_start_matches('$');
    match s.strip_prefix('\'').and_then(|t| t.strip_suffix('\'')) {
        Some(inner) => inner.replace("''", "'"),
        None => s.to_owned(),
    }
}

/// `Sheet!` for Excel, quoted when it needs to be.
pub(crate) fn sheet_prefix(name: &str) -> String {
    let plain = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && cell(name).is_none();
    if plain {
        format!("{name}!")
    } else {
        format!("'{}'!", name.replace('\'', "''"))
    }
}

/// An absolute cell (`$B$2`), or a whole row (`$2`) or column (`$B`).
fn absolute(part: &str) -> Option<String> {
    let p = part.replace('$', "");
    let split = p.find(|c: char| c.is_ascii_digit()).unwrap_or(p.len());
    let (letters, digits) = p.split_at(split);
    if !letters.chars().all(|c| c.is_ascii_alphabetic())
        || !digits.chars().all(|c| c.is_ascii_digit())
        || p.is_empty()
    {
        return None;
    }
    let mut out = String::new();
    if !letters.is_empty() {
        out.push('$');
        out.push_str(&letters.to_ascii_uppercase());
    }
    if !digits.is_empty() {
        out.push('$');
        out.push_str(digits);
    }
    Some(out)
}

/// One ODF range address (`Data.B2:Data.B5`, `$'My sheet'.$A$1:.$C$5`,
/// `$Data.$1:.$2`) as an absolute Excel reference (`Data!$B$2:$B$5`),
/// sheet names through `rename`. `None` for several ranges or another
/// file's.
pub(crate) fn excel_ref(addr: &str, rename: &dyn Fn(&str) -> String) -> Option<String> {
    let addr = addr.trim();
    if addr.is_empty() || addr.contains(' ') && !addr.contains('\'') {
        return None;
    }
    let mut parts = Vec::new();
    let (mut start, mut in_quote) = (0, false);
    for (i, c) in addr.char_indices() {
        if c == '\'' {
            in_quote = !in_quote;
        } else if c == ':' && !in_quote {
            parts.push(&addr[start..i]);
            start = i + 1;
        }
    }
    parts.push(&addr[start..]);
    if parts.len() > 2 {
        return None;
    }
    let mut sheet = None;
    let mut cells = Vec::new();
    for part in parts {
        let (s, c) = match rfind_outside_quotes(part, '.') {
            Some(dot) => (&part[..dot], &part[dot + 1..]),
            None => ("", part),
        };
        if s.contains('#') || s.contains('/') && s.starts_with('\'') && s.contains("file:") {
            return None;
        }
        if !s.is_empty() && sheet.is_none() {
            sheet = Some(unquote_sheet(s));
        }
        cells.push(absolute(c)?);
    }
    if cells.len() == 2 && cells[0] == cells[1] {
        cells.pop();
    }
    let prefix = sheet.map(|s| sheet_prefix(&rename(&s))).unwrap_or_default();
    Some(format!("{prefix}{}", cells.join(":")))
}

/// Automatic and named styles of one document (content plus styles), by
/// name, with their parents.
struct StyleSet<'a, 'i> {
    styles: HashMap<(&'a str, &'a str), Node<'a, 'i>>,
    defaults: HashMap<&'a str, Node<'a, 'i>>,
    markers: HashMap<&'a str, Node<'a, 'i>>,
    gradients: HashMap<&'a str, Node<'a, 'i>>,
}

impl<'a, 'i> StyleSet<'a, 'i> {
    fn read(docs: &[&'a Document<'i>]) -> Self {
        let mut set = StyleSet {
            styles: HashMap::new(),
            defaults: HashMap::new(),
            markers: HashMap::new(),
            gradients: HashMap::new(),
        };
        for doc in docs {
            for n in doc.descendants().filter(|n| n.is_element()) {
                let tag = n.tag_name();
                match (tag.namespace(), tag.name()) {
                    (Some(NS_STYLE), "style") => {
                        if let (Some(name), Some(family)) = (
                            n.attribute((NS_STYLE, "name")),
                            n.attribute((NS_STYLE, "family")),
                        ) {
                            set.styles.insert((family, name), n);
                        }
                    }
                    (Some(NS_STYLE), "default-style") => {
                        if let Some(family) = n.attribute((NS_STYLE, "family")) {
                            set.defaults.insert(family, n);
                        }
                    }
                    (Some(NS_DRAW), "marker") => {
                        if let Some(name) = n.attribute((NS_DRAW, "name")) {
                            set.markers.insert(name, n);
                        }
                    }
                    (Some(NS_DRAW), "gradient") => {
                        if let Some(name) = n.attribute((NS_DRAW, "name")) {
                            set.gradients.insert(name, n);
                        }
                    }
                    _ => {}
                }
            }
        }
        set
    }

    /// A property of style `name` (family `family`) in its `props`
    /// element, looked up through its parents and the family's default.
    fn prop(
        &self,
        family: &str,
        name: Option<&str>,
        props: &str,
        ns: &str,
        attr: &str,
    ) -> Option<&'a str> {
        let mut at = name;
        for _ in 0..16 {
            let Some(n) = at.and_then(|n| self.styles.get(&(family, n))) else {
                break;
            };
            if let Some(v) = n
                .children()
                .find(|c| c.has_tag_name((NS_STYLE, props)))
                .and_then(|p| p.attribute((ns, attr)))
            {
                return Some(v);
            }
            at = n.attribute((NS_STYLE, "parent-style-name"));
        }
        self.defaults
            .get(family)?
            .children()
            .find(|c| c.has_tag_name((NS_STYLE, props)))
            .and_then(|p| p.attribute((ns, attr)))
    }
}

/// `#rrggbb` as `0xRRGGBB`.
fn rgb(s: &str) -> Option<u32> {
    let hex = s.trim().strip_prefix('#')?;
    (hex.len() == 6).then(|| u32::from_str_radix(hex, 16).ok())?
}

/// What a sheet's drawing objects became.
#[derive(Default)]
pub(crate) struct OdsDrawings {
    /// Objects by model sheet index.
    pub sheets: Vec<(u32, Vec<Object>)>,
    pub blips: Vec<Option<Blip>>,
    pub skipped: Skipped,
}

/// Reads drawing objects of one `.ods` package.
pub(crate) struct Reader<'a, 'i, R: std::io::Read + std::io::Seek> {
    zip: zip::ZipArchive<R>,
    styles: StyleSet<'a, 'i>,
    rename: &'a dyn Fn(&str) -> String,
    pub out: OdsDrawings,
}

/// Column widths and row heights of a table as the file has them, px:
/// runs of `(first, count, px)` in order.
#[derive(Default)]
pub(crate) struct Spans {
    pub cols: Vec<(u32, u32, f64)>,
    pub rows: Vec<(u32, u32, f64)>,
}

/// LibreOffice's default column width (0.889 in) and row height (0.178 in).
const DEFAULT_COL_PX: f64 = 85.344;
const DEFAULT_ROW_PX: f64 = 17.088;

impl Spans {
    /// Px before line `at` and the line under `pos`, with the offset into it.
    fn start(runs: &[(u32, u32, f64)], at: u32, default: f64) -> f64 {
        let (mut pos, mut next) = (0.0, 0u32);
        for &(first, count, px) in runs {
            if first >= at {
                break;
            }
            pos += f64::from(first.saturating_sub(next)) * default;
            let n = count.min(at - first);
            pos += f64::from(n) * px;
            next = first + count;
        }
        pos + f64::from(at.saturating_sub(next)) * default
    }

    fn find(runs: &[(u32, u32, f64)], pos: f64, default: f64, last: u32) -> (u32, f64) {
        let (mut left, mut next) = (pos.max(0.0), 0u32);
        let skip = |n: u32, px: f64, from: u32, left: &mut f64| -> Option<(u32, f64)> {
            if n == 0 {
                return None;
            }
            if px <= 0.0 {
                return None;
            }
            let whole = ((*left + 1e-6) / px).floor();
            if whole < f64::from(n) {
                return Some((from + whole as u32, (*left - whole * px).max(0.0)));
            }
            *left -= f64::from(n) * px;
            None
        };
        for &(first, count, px) in runs {
            if first > next
                && let Some(hit) = skip(first - next, default, next, &mut left)
            {
                return (hit.0.min(last), hit.1);
            }
            if let Some(hit) = skip(count, px, first, &mut left) {
                return (hit.0.min(last), hit.1);
            }
            next = first + count;
        }
        let whole = ((left + 1e-6) / default).floor();
        let at = u64::from(next) + whole as u64;
        if at > u64::from(last) {
            (last, 0.0)
        } else {
            (at as u32, (left - whole * default).max(0.0))
        }
    }

    pub(crate) fn origin(&self, col: u32, row: u32) -> (f64, f64) {
        (
            Self::start(&self.cols, col, DEFAULT_COL_PX),
            Self::start(&self.rows, row, DEFAULT_ROW_PX),
        )
    }

    /// The cell corner and offset (as Excel px) at (`x`, `y`) px from the
    /// sheet's top left.
    pub(crate) fn marker_at(&self, x: f64, y: f64) -> Marker {
        let (col, dx) = Self::find(&self.cols, x, DEFAULT_COL_PX, crate::MAX_COLS - 1);
        let (row, dy) = Self::find(&self.rows, y, DEFAULT_ROW_PX, crate::MAX_ROWS - 1);
        Marker {
            col,
            col_off: emu(dx),
            row,
            row_off: emu(dy),
        }
    }
}

/// A drawing object found in a table, with the cell it is anchored to.
pub(crate) struct Found<'a, 'i> {
    pub node: Node<'a, 'i>,
    pub cell: Option<(u32, u32)>,
}

impl<'a, 'i, R: std::io::Read + std::io::Seek> Reader<'a, 'i, R> {
    pub(crate) fn new(
        zip: zip::ZipArchive<R>,
        docs: &[&'a Document<'i>],
        rename: &'a dyn Fn(&str) -> String,
    ) -> Self {
        Self {
            zip,
            styles: StyleSet::read(docs),
            rename,
            out: OdsDrawings::default(),
        }
    }

    fn bytes(&mut self, name: &str) -> Option<Vec<u8>> {
        let name = name.trim_start_matches("./");
        let mut entry = self.zip.by_name(name).ok()?;
        if entry.size() > 256 << 20 {
            return None;
        }
        let mut v = Vec::new();
        entry.read_to_end(&mut v).ok()?;
        Some(v)
    }

    /// The objects of the sheet `sheet` (`found` in document order).
    pub(crate) fn sheet(&mut self, sheet: u32, found: &[Found<'a, 'i>], geometry: &Spans) {
        let mut objects = Vec::new();
        for f in found {
            let origin = f.cell.map_or((0.0, 0.0), |(c, r)| geometry.origin(c, r));
            self.object(
                f.node,
                f.cell.is_some(),
                origin,
                geometry,
                &mut objects,
                true,
            );
        }
        if !objects.is_empty() {
            self.out.sheets.push((sheet, objects));
        }
    }

    fn object(
        &mut self,
        n: Node<'a, 'i>,
        in_cell: bool,
        origin: (f64, f64),
        geometry: &Spans,
        out: &mut Vec<Object>,
        top: bool,
    ) {
        let tag = n.tag_name();
        if tag.namespace() != Some(NS_DRAW) {
            return;
        }
        if tag.name() == "g" {
            for c in n.children().filter(|c| c.is_element()) {
                self.object(c, in_cell, origin, geometry, out, false);
            }
            return;
        }
        let name = n.attribute((NS_DRAW, "name")).map(str::to_owned);
        let descr = n
            .children()
            .find(|c| c.has_tag_name((NS_SVG, "desc")) || c.has_tag_name((NS_SVG, "title")))
            .and_then(|c| c.text())
            .map(str::to_owned);
        let hidden = [
            n.attribute((NS_DRAWOOO, "display")),
            n.attribute((NS_DRAW, "display")),
        ]
        .contains(&Some("none"));
        let place = Place::of(n);
        let kind = match tag.name() {
            "frame" => self.frame(n, &place),
            "custom-shape" | "rect" | "ellipse" | "circle" | "line" | "connector" => {
                self.shape(n, &place)
            }
            _ => {
                self.out.skipped.shapes += 1;
                return;
            }
        };
        let Some((kind, size)) = kind else {
            return;
        };
        let (x, y) = (origin.0 + place.x, origin.1 + place.y);
        let (w, h) = size;
        let from = geometry.marker_at(x, y);
        // The end cell is only right for the outermost object.
        let end = top
            .then(|| n.attribute((NS_TABLE, "end-cell-address")))
            .flatten()
            .and_then(cell)
            .filter(|_| place.rot == 0.0);
        let to = match end {
            Some((c, r)) => {
                let ex = n.attribute((NS_TABLE, "end-x")).and_then(px).unwrap_or(0.0);
                let ey = n.attribute((NS_TABLE, "end-y")).and_then(px).unwrap_or(0.0);
                let m = Marker {
                    col: c,
                    col_off: emu(ex),
                    row: r,
                    row_off: emu(ey),
                };
                if (m.row, m.col) < (from.row, from.col) {
                    geometry.marker_at(x + w, y + h)
                } else {
                    m
                }
            }
            None => geometry.marker_at(x + w, y + h),
        };
        out.push(Object {
            anchor: Anchor::TwoCell {
                from,
                to,
                edit_as: if in_cell {
                    EditAs::OneCell
                } else {
                    EditAs::Absolute
                },
            },
            name,
            descr,
            hidden,
            kind,
        });
    }

    /// A `draw:frame`: picture, chart or text box, and its size.
    fn frame(&mut self, n: Node<'a, 'i>, place: &Place) -> Option<(ObjectKind, (f64, f64))> {
        let first = n
            .children()
            .find(|c| c.is_element() && c.tag_name().namespace() == Some(NS_DRAW));
        let Some(first) = first else {
            self.out.skipped.shapes += 1;
            return None;
        };
        match first.tag_name().name() {
            "image" => {
                let href = first.attribute((NS_XLINK, "href")).unwrap_or_default();
                let data = self.bytes(href);
                let ext = data.as_deref().and_then(|d| {
                    if d.starts_with(b"\x89PNG") {
                        Some("png")
                    } else if d.starts_with(&[0xFF, 0xD8]) {
                        Some("jpeg")
                    } else {
                        None
                    }
                });
                let (Some(data), Some(ext)) = (data, ext) else {
                    self.out.skipped.pictures += 1;
                    return None;
                };
                self.out.blips.push(Some(Blip::Image { ext, data }));
                Some((
                    ObjectKind::Picture {
                        blip: self.out.blips.len() - 1,
                        crop: [0.0; 4],
                    },
                    (place.w, place.h),
                ))
            }
            "object" => {
                let href = first.attribute((NS_XLINK, "href")).unwrap_or_default();
                let path = format!("{}/content.xml", href.trim_start_matches("./"));
                let xml = self.bytes(&path).and_then(|b| String::from_utf8(b).ok());
                let parsed = xml.as_deref().and_then(|x| {
                    let doc = Document::parse(x).ok()?;
                    let styles = StyleSet::read(&[&doc]);
                    Some(read_chart(&doc, &styles, self.rename))
                });
                match parsed {
                    Some(Ok((chart, size))) => {
                        self.out.skipped.flat += usize::from(chart.three_d);
                        let size = if place.w > 0.0 && place.h > 0.0 {
                            (place.w, place.h)
                        } else {
                            size
                        };
                        Some((ObjectKind::Chart(Box::new(chart)), size))
                    }
                    _ => {
                        if parsed.is_none() && xml.is_none() {
                            self.out.skipped.shapes += 1;
                        } else {
                            self.out.skipped.charts += 1;
                        }
                        None
                    }
                }
            }
            "text-box" => {
                let mut shape = self.base_shape(n, place, "rect".into(), Vec::new());
                shape.tx_box = true;
                shape.text = self.paragraphs(first);
                Some((ObjectKind::Shape(Box::new(shape)), (place.w, place.h)))
            }
            _ => {
                self.out.skipped.shapes += 1;
                None
            }
        }
    }

    /// The common part of a shape: fill, outline, rotation, text anchor.
    fn base_shape(&self, n: Node<'a, 'i>, place: &Place, prst: String, adj: Vec<i64>) -> Shape {
        let style = n.attribute((NS_DRAW, "style-name"));
        let g = |ns: &str, a: &str| {
            self.styles
                .prop("graphic", style, "graphic-properties", ns, a)
        };
        let fill = match g(NS_DRAW, "fill") {
            Some("none") => Fill::None,
            Some("solid") => g(NS_DRAW, "fill-color")
                .and_then(rgb)
                .map_or(Fill::None, Fill::Solid),
            Some("gradient") => {
                let grad = g(NS_DRAW, "fill-gradient-name")
                    .and_then(|name| self.styles.gradients.get(name));
                match grad {
                    Some(gr) => {
                        let c1 = gr.attribute((NS_DRAW, "start-color")).and_then(rgb);
                        let c2 = gr.attribute((NS_DRAW, "end-color")).and_then(rgb);
                        let angle = gr
                            .attribute((NS_DRAW, "angle"))
                            .and_then(angle_degrees)
                            .unwrap_or(0.0);
                        match (c1, c2) {
                            (Some(a), Some(b)) => Fill::Gradient(a, b, angle),
                            (Some(a), None) | (None, Some(a)) => Fill::Solid(a),
                            _ => Fill::None,
                        }
                    }
                    None => Fill::None,
                }
            }
            // Bitmaps and hatches: their colour, if any.
            Some(_) => g(NS_DRAW, "fill-color")
                .and_then(rgb)
                .map_or(Fill::None, Fill::Solid),
            None => Fill::None,
        };
        let line = match g(NS_DRAW, "stroke") {
            Some("none") => None,
            stroke => Some(Line {
                color: g(NS_SVG, "stroke-color").and_then(rgb).unwrap_or(0),
                width: g(NS_SVG, "stroke-width").and_then(px).map_or(9525, emu),
                dash: stroke == Some("dash"),
                head: g(NS_DRAW, "marker-start").and_then(|m| self.marker(m)),
                tail: g(NS_DRAW, "marker-end").and_then(|m| self.marker(m)),
            }),
        };
        let anchor = match g(NS_DRAW, "textarea-vertical-align") {
            Some("middle") => "ctr",
            Some("bottom") => "b",
            _ => "t",
        };
        Shape {
            prst,
            adj,
            line_shape: false,
            flip: (false, false),
            tx_box: false,
            fill,
            line,
            text: Vec::new(),
            anchor,
            size: (emu(place.w), emu(place.h)),
            rot: place.rot,
        }
    }

    /// An arrowhead of a marker style.
    fn marker(&self, name: &str) -> Option<&'static str> {
        let m = self.styles.markers.get(name);
        let label = m
            .and_then(|m| m.attribute((NS_DRAW, "display-name")))
            .unwrap_or(name)
            .to_ascii_lowercase();
        Some(if label.contains("circle") || label.contains("oval") {
            "oval"
        } else if label.contains("square") || label.contains("diamond") {
            "diamond"
        } else if label.contains("line") && !label.contains("arrow") {
            "arrow"
        } else {
            "triangle"
        })
    }

    /// A shape element, and its size.
    fn shape(&mut self, n: Node<'a, 'i>, place: &Place) -> Option<(ObjectKind, (f64, f64))> {
        let tag = n.tag_name().name();
        if tag == "line" || tag == "connector" {
            let p = |a: &str| n.attribute((NS_SVG, a)).and_then(px);
            let (Some(x1), Some(y1), Some(x2), Some(y2)) = (p("x1"), p("y1"), p("x2"), p("y2"))
            else {
                self.out.skipped.shapes += 1;
                return None;
            };
            let mut shape = self.base_shape(n, place, "line".into(), Vec::new());
            shape.line_shape = true;
            shape.flip = (x2 < x1, y2 < y1);
            let (w, h) = ((x2 - x1).abs(), (y2 - y1).abs());
            shape.size = (emu(w), emu(h));
            if shape.line.is_none() {
                shape.line = Some(Line::default());
            }
            return Some((ObjectKind::Shape(Box::new(shape)), (w, h)));
        }
        let (prst, adj) = match tag {
            "rect" => {
                let round = n
                    .attribute((NS_DRAW, "corner-radius"))
                    .and_then(px)
                    .is_some_and(|r| r > 0.0);
                (
                    if round { "roundRect" } else { "rect" }.to_owned(),
                    Vec::new(),
                )
            }
            "ellipse" | "circle" => ("ellipse".to_owned(), Vec::new()),
            _ => {
                let geom = n
                    .children()
                    .find(|c| c.has_tag_name((NS_DRAW, "enhanced-geometry")));
                let ty = geom
                    .and_then(|g| g.attribute((NS_DRAW, "type")))
                    .unwrap_or("non-primitive");
                match ty.strip_prefix("ooxml-") {
                    Some(p) if preset_name(p) => {
                        let adj = geom
                            .and_then(|g| g.attribute((NS_DRAW, "modifiers")))
                            .map(|m| {
                                m.split_whitespace()
                                    .filter_map(|v| v.parse::<f64>().ok())
                                    .map(|v| v.round() as i64)
                                    .collect()
                            })
                            .unwrap_or_default();
                        (p.to_owned(), adj)
                    }
                    Some(_) => {
                        self.out.skipped.shapes += 1;
                        return None;
                    }
                    None => match lo_preset(ty) {
                        Some(p) => (p.to_owned(), Vec::new()),
                        None => {
                            self.out.skipped.shapes += 1;
                            return None;
                        }
                    },
                }
            }
        };
        let mut shape = self.base_shape(n, place, prst, adj);
        shape.text = self.paragraphs(n);
        if tag == "custom-shape"
            && let Some(geom) = n
                .children()
                .find(|c| c.has_tag_name((NS_DRAW, "enhanced-geometry")))
        {
            let flip = |a: &str| geom.attribute((NS_DRAW, a)) == Some("true");
            shape.flip = (flip("mirror-horizontal"), flip("mirror-vertical"));
        }
        Some((ObjectKind::Shape(Box::new(shape)), (place.w, place.h)))
    }

    /// The paragraphs of a shape's or text box's text.
    fn paragraphs(&self, n: Node<'a, 'i>) -> Vec<Para> {
        let mut out = Vec::new();
        for p in n
            .children()
            .filter(|c| c.has_tag_name((NS_TEXT, "p")) || c.has_tag_name((NS_TEXT, "h")))
        {
            let pstyle = p.attribute((NS_TEXT, "style-name"));
            let align = match self.styles.prop(
                "paragraph",
                pstyle,
                "paragraph-properties",
                NS_FO,
                "text-align",
            ) {
                Some("center") => Some("ctr"),
                Some("end" | "right") => Some("r"),
                Some("justify") => Some("just"),
                Some("start" | "left") => Some("l"),
                _ => None,
            };
            let mut runs = Vec::new();
            self.runs(p, pstyle, &mut runs);
            out.push(Para { align, runs });
        }
        // An empty last paragraph (LibreOffice writes one in every shape).
        if out.iter().all(|p| p.runs.iter().all(|r| r.text.is_empty())) {
            out.clear();
        }
        out
    }

    fn runs(&self, n: Node<'a, 'i>, pstyle: Option<&str>, out: &mut Vec<Run>) {
        for c in n.children() {
            if c.is_text() {
                let text = c.text().unwrap_or_default();
                if !text.is_empty() {
                    let style = n.attribute((NS_TEXT, "style-name"));
                    out.push(self.run(style, pstyle, text.to_owned()));
                }
                continue;
            }
            let tag = c.tag_name();
            if tag.namespace() != Some(NS_TEXT) {
                continue;
            }
            match tag.name() {
                "span" | "a" => self.runs(c, pstyle, out),
                "s" => {
                    let k: usize = c
                        .attribute((NS_TEXT, "c"))
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(1);
                    let style = n.attribute((NS_TEXT, "style-name"));
                    out.push(self.run(style, pstyle, " ".repeat(k.min(100))));
                }
                "tab" => {
                    let style = n.attribute((NS_TEXT, "style-name"));
                    out.push(self.run(style, pstyle, "\t".into()));
                }
                "line-break" => {
                    let style = n.attribute((NS_TEXT, "style-name"));
                    out.push(self.run(style, pstyle, "\n".into()));
                }
                _ => {}
            }
        }
    }

    fn run(&self, style: Option<&str>, pstyle: Option<&str>, text: String) -> Run {
        let t = |a: &str, ns: &str| {
            self.styles
                .prop("text", style, "text-properties", ns, a)
                .or_else(|| {
                    self.styles
                        .prop("paragraph", pstyle, "text-properties", ns, a)
                })
        };
        Run {
            text,
            size: t("font-size", NS_FO)
                .and_then(px)
                .map(|p| (p * 72.0 / 96.0 * 100.0).round() as u32),
            bold: t("font-weight", NS_FO)
                .is_some_and(|w| w == "bold" || w.parse::<u32>().is_ok_and(|n| n >= 600)),
            italic: t("font-style", NS_FO).is_some_and(|s| s == "italic" || s == "oblique"),
            color: t("color", NS_FO).and_then(rgb),
            font: t("font-name", NS_STYLE)
                .or_else(|| t("font-family", NS_FO))
                .map(|f| f.trim_matches('\'').to_owned()),
        }
    }
}

/// A `draw:gradient` angle (`300`, tenths of a degree, or `30deg`).
fn angle_degrees(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Some(d) = s.strip_suffix("deg") {
        return d.parse().ok();
    }
    if let Some(r) = s.strip_suffix("rad") {
        return r.parse::<f64>().ok().map(f64::to_degrees);
    }
    s.parse::<f64>().ok().map(|v| v / 10.0)
}

/// A DrawingML preset name (the `ooxml-*` types LibreOffice keeps).
fn preset_name(p: &str) -> bool {
    !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric())
}

/// The DrawingML preset of a LibreOffice custom shape type.
fn lo_preset(ty: &str) -> Option<&'static str> {
    Some(match ty {
        "rectangle" | "flowchart-process" | "mso-spt1" => "rect",
        "round-rectangle" | "flowchart-alternate-process" => "roundRect",
        "ellipse" | "circle" | "flowchart-connector" => "ellipse",
        "isosceles-triangle" => "triangle",
        "right-triangle" => "rtTriangle",
        "diamond" | "flowchart-decision" => "diamond",
        "parallelogram" | "flowchart-data" => "parallelogram",
        "trapezoid" => "trapezoid",
        "pentagon" => "pentagon",
        "hexagon" | "flowchart-preparation" => "hexagon",
        "octagon" => "octagon",
        "cross" => "plus",
        "star4" => "star4",
        "star5" => "star5",
        "star6" => "star6",
        "star8" => "star8",
        "star12" => "star12",
        "star24" => "star24",
        "can" | "flowchart-magnetic-disk" => "can",
        "cube" => "cube",
        "heart" => "heart",
        "smiley" => "smileyFace",
        "sun" => "sun",
        "moon" => "moon",
        "lightning" => "lightningBolt",
        "ring" => "donut",
        "frame" => "frame",
        "right-arrow" => "rightArrow",
        "left-arrow" => "leftArrow",
        "up-arrow" => "upArrow",
        "down-arrow" => "downArrow",
        "left-right-arrow" => "leftRightArrow",
        "up-down-arrow" => "upDownArrow",
        "chevron" => "chevron",
        "pentagon-right" => "homePlate",
        "rectangular-callout" => "wedgeRectCallout",
        "round-rectangular-callout" => "wedgeRoundRectCallout",
        "round-callout" => "wedgeEllipseCallout",
        "cloud-callout" | "cloud" => "cloudCallout",
        "flowchart-terminator" => "flowChartTerminator",
        "flowchart-document" => "flowChartDocument",
        "flowchart-predefined-process" => "flowChartPredefinedProcess",
        "flowchart-manual-input" => "flowChartManualInput",
        "block-arc" => "blockArc",
        "quad-bevel" => "bevel",
        _ => return None,
    })
}

/// Where an object is: offset (px) from its cell or the sheet, its size and
/// rotation (degrees clockwise).
struct Place {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    rot: f64,
}

impl Place {
    fn of(n: Node) -> Self {
        let p = |a: &str| n.attribute((NS_SVG, a)).and_then(px).unwrap_or(0.0);
        if n.attribute((NS_SVG, "x1")).is_some() {
            let (x1, y1, x2, y2) = (p("x1"), p("y1"), p("x2"), p("y2"));
            return Self {
                x: x1.min(x2),
                y: y1.min(y2),
                w: (x2 - x1).abs(),
                h: (y2 - y1).abs(),
                rot: 0.0,
            };
        }
        let (mut x, mut y, w, h) = (p("x"), p("y"), p("width").max(0.0), p("height").max(0.0));
        let mut rot = 0.0;
        if let Some(t) = n.attribute((NS_DRAW, "transform")) {
            // `rotate (a) translate (x y)`: the box turned about its top left
            // corner by `a` radians anticlockwise, then moved.
            let (mut a, mut tx, mut ty) = (0.0, None, None);
            let mut rest = t;
            while let Some(open) = rest.find('(') {
                let name = rest[..open].trim();
                let Some(close) = rest[open..].find(')') else {
                    break;
                };
                let args: Vec<&str> = rest[open + 1..open + close].split_whitespace().collect();
                match name {
                    "rotate" => a = args.first().and_then(|v| v.parse().ok()).unwrap_or(0.0),
                    "translate" => {
                        tx = args.first().and_then(|v| px(v));
                        ty = args.get(1).and_then(|v| px(v));
                    }
                    _ => {}
                }
                rest = &rest[open + close + 1..];
            }
            if let (Some(tx), Some(ty)) = (tx, ty) {
                let f64_a: f64 = a;
                let (s, c) = f64_a.sin_cos();
                let (hw, hh) = (w / 2.0, h / 2.0);
                // Centre after turning (y down: anticlockwise is
                // (x cos + y sin, -x sin + y cos)).
                let cx = tx + hw * c + hh * s;
                let cy = ty - hw * s + hh * c;
                x = cx - hw;
                y = cy - hh;
                rot = (-f64_a.to_degrees()).rem_euclid(360.0);
                if rot.abs() < 0.01 || (360.0 - rot).abs() < 0.01 {
                    rot = 0.0;
                }
            }
        }
        Self { x, y, w, h, rot }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Fill {
    None,
    Solid(u32),
    /// Start and end colours and the ODF angle (degrees anticlockwise from
    /// a top-to-bottom blend).
    Gradient(u32, u32, f64),
}

#[derive(Debug, Clone, PartialEq)]
struct Line {
    color: u32,
    /// EMU.
    width: i64,
    dash: bool,
    head: Option<&'static str>,
    tail: Option<&'static str>,
}

impl Default for Line {
    fn default() -> Self {
        Self {
            color: 0,
            width: 9525,
            dash: false,
            head: None,
            tail: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Para {
    align: Option<&'static str>,
    runs: Vec<Run>,
}

#[derive(Debug, Clone, PartialEq)]
struct Run {
    text: String,
    /// Hundredths of a point.
    size: Option<u32>,
    bold: bool,
    italic: bool,
    color: Option<u32>,
    font: Option<String>,
}

/// A shape, line or text box to write as `xdr:sp` / `xdr:cxnSp`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Shape {
    prst: String,
    adj: Vec<i64>,
    /// A line (`xdr:cxnSp`).
    line_shape: bool,
    /// Flipped across and down.
    flip: (bool, bool),
    tx_box: bool,
    fill: Fill,
    line: Option<Line>,
    text: Vec<Para>,
    /// `bodyPr anchor`.
    anchor: &'static str,
    /// EMU.
    size: (i64, i64),
    /// Degrees clockwise.
    rot: f64,
}

fn solid(c: u32) -> String {
    format!("<a:solidFill><a:srgbClr val=\"{c:06X}\"/></a:solidFill>")
}

impl Shape {
    /// The `xdr:sp` (or `xdr:cxnSp`) element.
    pub(crate) fn xml(&self, id: usize, name: &str, descr: &str, hidden: &str) -> String {
        let rot = (self.rot * 60_000.0).round() as i64;
        let rot = if rot != 0 {
            format!(" rot=\"{rot}\"")
        } else {
            String::new()
        };
        let flips = format!(
            "{}{}",
            if self.flip.0 { " flipH=\"1\"" } else { "" },
            if self.flip.1 { " flipV=\"1\"" } else { "" }
        );
        let av = match self.adj.len() {
            0 => "<a:avLst/>".to_owned(),
            1 => format!(
                "<a:avLst><a:gd name=\"adj\" fmla=\"val {}\"/></a:avLst>",
                self.adj[0]
            ),
            _ => {
                let mut s = String::from("<a:avLst>");
                for (i, v) in self.adj.iter().enumerate() {
                    s.push_str(&format!("<a:gd name=\"adj{}\" fmla=\"val {v}\"/>", i + 1));
                }
                s + "</a:avLst>"
            }
        };
        let fill = match &self.fill {
            Fill::None => "<a:noFill/>".to_owned(),
            Fill::Solid(c) => solid(*c),
            Fill::Gradient(a, b, angle) => {
                // ODF 0°: start colour at the top; DrawingML `lin ang`
                // turns clockwise from left to right.
                let ang = ((90.0 - angle).rem_euclid(360.0) * 60_000.0).round() as i64;
                format!(
                    "<a:gradFill rotWithShape=\"1\"><a:gsLst><a:gs pos=\"0\"><a:srgbClr val=\"{a:06X}\"/></a:gs><a:gs pos=\"100000\"><a:srgbClr val=\"{b:06X}\"/></a:gs></a:gsLst><a:lin ang=\"{ang}\" scaled=\"0\"/></a:gradFill>"
                )
            }
        };
        let fill = if self.line_shape { String::new() } else { fill };
        let ln = match &self.line {
            None => "<a:ln><a:noFill/></a:ln>".to_owned(),
            Some(l) => format!(
                "<a:ln w=\"{}\">{}{}{}{}</a:ln>",
                l.width.max(0),
                solid(l.color),
                if l.dash {
                    "<a:prstDash val=\"dash\"/>"
                } else {
                    ""
                },
                l.head
                    .map(|t| format!("<a:headEnd type=\"{t}\"/>"))
                    .unwrap_or_default(),
                l.tail
                    .map(|t| format!("<a:tailEnd type=\"{t}\"/>"))
                    .unwrap_or_default(),
            ),
        };
        let sppr = format!(
            "<xdr:spPr><a:xfrm{rot}{flips}><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm><a:prstGeom prst=\"{}\">{av}</a:prstGeom>{fill}{ln}</xdr:spPr>",
            self.size.0.max(0),
            self.size.1.max(0),
            esc(&self.prst)
        );
        if self.line_shape {
            return format!(
                "<xdr:cxnSp macro=\"\"><xdr:nvCxnSpPr><xdr:cNvPr id=\"{id}\" name=\"{}\"{descr}{hidden}/><xdr:cNvCxnSpPr/></xdr:nvCxnSpPr>{sppr}</xdr:cxnSp>",
                esc(name)
            );
        }
        let mut body = String::new();
        if !self.text.is_empty() {
            body.push_str(&format!(
                "<xdr:txBody><a:bodyPr vertOverflow=\"clip\" horzOverflow=\"clip\" wrap=\"square\" rtlCol=\"0\" anchor=\"{}\"/><a:lstStyle/>",
                self.anchor
            ));
            for p in &self.text {
                body.push_str("<a:p>");
                if let Some(a) = p.align {
                    body.push_str(&format!("<a:pPr algn=\"{a}\"/>"));
                }
                for r in &p.runs {
                    let mut rpr = String::from("<a:rPr lang=\"en-US\"");
                    if let Some(sz) = r.size {
                        rpr.push_str(&format!(" sz=\"{}\"", sz.clamp(100, 400_000)));
                    }
                    if r.bold {
                        rpr.push_str(" b=\"1\"");
                    }
                    if r.italic {
                        rpr.push_str(" i=\"1\"");
                    }
                    rpr.push('>');
                    if let Some(c) = r.color {
                        rpr.push_str(&solid(c));
                    }
                    if let Some(f) = &r.font {
                        rpr.push_str(&format!("<a:latin typeface=\"{}\"/>", esc(f)));
                    }
                    rpr.push_str("</a:rPr>");
                    for (i, piece) in r.text.split('\n').enumerate() {
                        if i > 0 {
                            body.push_str("<a:br>");
                            body.push_str(&rpr);
                            body.push_str("</a:br>");
                        }
                        if !piece.is_empty() {
                            body.push_str(&format!("<a:r>{rpr}<a:t>{}</a:t></a:r>", esc(piece)));
                        }
                    }
                }
                body.push_str("</a:p>");
            }
            body.push_str("</xdr:txBody>");
        }
        format!(
            "<xdr:sp macro=\"\" textlink=\"\"><xdr:nvSpPr><xdr:cNvPr id=\"{id}\" name=\"{}\"{descr}{hidden}/><xdr:cNvSpPr{}/></xdr:nvSpPr>{sppr}{body}</xdr:sp>",
            esc(name),
            if self.tx_box { " txBox=\"1\"" } else { "" }
        )
    }
}

/// Why a chart isn't brought in.
#[derive(Debug, PartialEq)]
pub(crate) struct Unsupported;

/// The text of an element's paragraphs, joined by spaces.
fn text_of(n: Node) -> Option<String> {
    let parts: Vec<String> = n
        .children()
        .filter(|c| c.has_tag_name((NS_TEXT, "p")))
        .map(|p| {
            p.descendants()
                .filter(|d| d.is_text())
                .filter_map(|d| d.text())
                .collect::<String>()
        })
        .collect();
    let t = parts.join(" ").trim().to_owned();
    (!t.is_empty()).then_some(t)
}

fn ch<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.has_tag_name((NS_CHART, name)))
}

fn astyle<'a>(a: Option<Node<'a, '_>>) -> Option<&'a str> {
    a.and_then(|a| a.attribute((NS_CHART, "style-name")))
}

/// The chart of an embedded object's `content.xml`, and its own size (px).
fn read_chart(
    doc: &Document,
    styles: &StyleSet,
    rename: &dyn Fn(&str) -> String,
) -> Result<(Chart, (f64, f64)), Unsupported> {
    let chart = doc
        .descendants()
        .find(|n| n.has_tag_name((NS_CHART, "chart")))
        .ok_or(Unsupported)?;
    let size = (
        chart
            .attribute((NS_SVG, "width"))
            .and_then(px)
            .unwrap_or(480.0),
        chart
            .attribute((NS_SVG, "height"))
            .and_then(px)
            .unwrap_or(288.0),
    );
    let class = chart
        .attribute((NS_CHART, "class"))
        .unwrap_or("chart:bar")
        .trim_start_matches("chart:");
    let cp =
        |style: Option<&str>, a: &str| styles.prop("chart", style, "chart-properties", NS_CHART, a);
    let gp = |style: Option<&str>, ns: &str, a: &str| {
        styles.prop("chart", style, "graphic-properties", ns, a)
    };
    let plot = ch(chart, "plot-area").ok_or(Unsupported)?;
    let pstyle = plot.attribute((NS_CHART, "style-name"));
    let on = |v: Option<&str>| v == Some("true");
    let horizontal = on(cp(pstyle, "vertical"));
    let stacked = on(cp(pstyle, "stacked"));
    let percent = on(cp(pstyle, "percentage"));
    let three_d = on(cp(pstyle, "three-dimensional"));
    let grouping = |standard: &'static str| {
        if percent {
            "percentStacked"
        } else if stacked {
            "stacked"
        } else {
            standard
        }
    };
    let axes: Vec<Node> = plot
        .children()
        .filter(|c| c.has_tag_name((NS_CHART, "axis")))
        .collect();
    let axis = |dim: &str| {
        axes.iter()
            .find(|a| {
                a.attribute((NS_CHART, "dimension")) == Some(dim)
                    && a.attribute((NS_CHART, "name"))
                        .is_none_or(|n| n.starts_with("primary"))
            })
            .copied()
    };
    let (x_axis, y_axis) = (axis("x"), axis("y"));
    let cats = x_axis
        .and_then(|a| ch(a, "categories"))
        .and_then(|c| c.attribute((NS_TABLE, "cell-range-address")))
        .and_then(|r| excel_ref(r, rename));
    let gap = cp(astyle(y_axis), "gap-width")
        .or_else(|| cp(pstyle, "gap-width"))
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(150);
    let overlap = cp(astyle(y_axis), "overlap")
        .or_else(|| cp(pstyle, "overlap"))
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(if stacked || percent { 100 } else { 0 });
    let first_angle = cp(pstyle, "angle-offset")
        .and_then(|v| v.parse::<f64>().ok())
        .map_or(0, |a| ((90.0 - a).rem_euclid(360.0)).round() as u16);
    let kind_of = |class: &str| -> Result<PlotKind, Unsupported> {
        Ok(match class {
            "bar" => PlotKind::Bar {
                horizontal,
                grouping: grouping("clustered"),
                gap: gap.clamp(0, 500) as u16,
                overlap: overlap.clamp(-100, 100) as i16,
            },
            "line" => PlotKind::Line {
                grouping: grouping("standard"),
            },
            "area" => PlotKind::Area {
                grouping: grouping("standard"),
            },
            "circle" => PlotKind::Pie {
                hole: 0,
                first_angle,
            },
            "ring" => PlotKind::Pie {
                hole: 50,
                first_angle,
            },
            "scatter" => PlotKind::Scatter,
            _ => return Err(Unsupported),
        })
    };
    let chart_kind = kind_of(class)?;
    let mut plots: Vec<Plot> = Vec::new();
    for s in plot
        .children()
        .filter(|c| c.has_tag_name((NS_CHART, "series")))
    {
        let sclass = s
            .attribute((NS_CHART, "class"))
            .map(|c| c.trim_start_matches("chart:"))
            .unwrap_or(class);
        // A ring's series are written as `circle`.
        let kind = if class == "ring" {
            chart_kind
        } else {
            kind_of(sclass)?
        };
        let Some(values) = s
            .attribute((NS_CHART, "values-cell-range-address"))
            .and_then(|r| excel_ref(r, rename))
        else {
            continue;
        };
        let name = s
            .attribute((NS_CHART, "label-cell-address"))
            .and_then(|r| excel_ref(r, rename))
            .map(Text::Ref)
            .or_else(|| {
                s.attribute((NS_LOEXT, "label-string")).map(|l| {
                    let l = l.trim();
                    let l = l
                        .strip_prefix('"')
                        .and_then(|t| t.strip_suffix('"'))
                        .unwrap_or(l);
                    Text::Literal(l.replace("\"\"", "\""))
                })
            });
        let style = s.attribute((NS_CHART, "style-name"));
        let scats = if matches!(kind, PlotKind::Scatter) {
            ch(s, "domain")
                .and_then(|d| d.attribute((NS_TABLE, "cell-range-address")))
                .and_then(|r| excel_ref(r, rename))
                .or_else(|| cats.clone())
        } else {
            cats.clone()
        };
        let label_number = cp(style, "data-label-number");
        let labels = Labels {
            value: matches!(label_number, Some("value" | "value-and-percentage")),
            percent: matches!(label_number, Some("percentage" | "value-and-percentage")),
            category: on(cp(style, "data-label-text")),
            series: on(cp(style, "data-label-series")),
            pos: cp(style, "label-position").and_then(|p| {
                Some(match p {
                    "outside" => "outEnd",
                    "inside" => "inEnd",
                    "center" => "ctr",
                    "near-origin" => "inBase",
                    "top" => "t",
                    "bottom" => "b",
                    "left" => "l",
                    "right" => "r",
                    "avoid-overlap" => "bestFit",
                    _ => return None,
                })
            }),
        };
        let series = Series {
            name,
            values,
            cats: scats,
            fill: gp(style, NS_DRAW, "fill-color").and_then(rgb),
            line: gp(style, NS_SVG, "stroke-color").and_then(rgb),
            no_line: gp(style, NS_DRAW, "stroke") == Some("none")
                && matches!(kind, PlotKind::Scatter | PlotKind::Line { .. }),
            no_marker: cp(style, "symbol-type").or_else(|| cp(pstyle, "symbol-type"))
                == Some("none"),
            labels: (labels.value || labels.percent || labels.category || labels.series)
                .then_some(labels),
        };
        match plots.iter_mut().find(|p| p.kind == kind) {
            Some(p) => p.series.push(series),
            None => plots.push(Plot {
                kind,
                varied: matches!(kind, PlotKind::Pie { .. }),
                series: vec![series],
            }),
        }
    }
    if plots.is_empty() {
        return Err(Unsupported);
    }
    let scale = |a: Option<Node>| {
        let st = astyle(a);
        let num = |k: &str| cp(st, k).and_then(|v| v.parse::<f64>().ok());
        Scale {
            min: num("minimum"),
            max: num("maximum"),
            major: num("interval-major").filter(|v| *v > 0.0),
            log: on(cp(st, "logarithmic")),
            reverse: on(cp(st, "reverse-direction")),
        }
    };
    let legend = ch(chart, "legend").map(|l| {
        match l.attribute((NS_CHART, "legend-position")).unwrap_or("end") {
            "start" => "l",
            "top" => "t",
            "bottom" => "b",
            "top-end" | "top-start" => "tr",
            _ => "r",
        }
    });
    let grid = |a: Option<Node>| {
        a.is_some_and(|a| {
            a.children().any(|g| {
                g.has_tag_name((NS_CHART, "grid"))
                    && g.attribute((NS_CHART, "class"))
                        .is_none_or(|c| c == "major")
            })
        })
    };
    let title_of = |a: Option<Node>| a.and_then(|a| ch(a, "title")).and_then(text_of);
    Ok((
        Chart {
            plots,
            title: ch(chart, "title").and_then(text_of),
            cat_title: title_of(x_axis),
            val_title: title_of(y_axis),
            legend,
            gridlines: [grid(x_axis), grid(y_axis)],
            three_d,
            scales: [scale(x_axis), scale(y_axis)],
        },
        size,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        let same = |s: &str| s.to_owned();
        assert_eq!(
            excel_ref("Data.B2:Data.B5", &same).as_deref(),
            Some("Data!$B$2:$B$5")
        );
        assert_eq!(
            excel_ref("$'My sheet'.$A$1:.$C$5", &same).as_deref(),
            Some("'My sheet'!$A$1:$C$5")
        );
        assert_eq!(
            excel_ref("Data.B1:Data.B1", &same).as_deref(),
            Some("Data!$B$1")
        );
        assert_eq!(
            excel_ref("$Report.$1:.$2", &same).as_deref(),
            Some("Report!$1:$2")
        );
        assert_eq!(excel_ref("A.B1 A.B3", &same), None);
        assert_eq!(cell("Shapes.O12"), Some((14, 11)));
    }

    #[test]
    fn turned_place() {
        let xml = r#"<x xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" svg:width="2in" svg:height="1in" draw:transform="rotate (-0.5235987755982988) translate (1in 1in)"/>"#;
        let doc = Document::parse(xml).unwrap();
        let p = Place::of(doc.root_element());
        assert!((p.rot - 30.0).abs() < 1e-6, "{}", p.rot);
        assert!((p.w - 192.0).abs() < 1e-6);
    }
}
