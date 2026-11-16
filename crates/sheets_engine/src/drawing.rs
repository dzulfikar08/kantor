//! Charts and pictures drawn over a worksheet (`xl/drawings/drawingN.xml`),
//! for showing them and for moving, resizing and deleting them.
//!
//! The drawing parts themselves are carried through save by `preserve`;
//! this module reads them once when the file is opened:
//!
//! - **Anchors** ([`Anchor`]): `twoCellAnchor` (two cell corners with
//!   offsets, and `editAs`), `oneCellAnchor` (a corner and a size) and
//!   `absoluteAnchor` (a position and a size). [`Anchor::rect`] places one on
//!   any grid through [`SheetGeometry`]: Kantor's grid (whose cells are not
//!   Excel's size), or a printed page.
//! - **Objects** ([`Drawing`]): charts (`graphicFrame` → `c:chart`), with
//!   their part's XML for the renderer, pictures (`pic` → `a:blip`, with
//!   cropping), shapes, text boxes, connectors and groups (`sp`, `cxnSp`,
//!   `grpSp`, also form controls' shapes inside `mc:AlternateContent`) as
//!   their element's XML ([`DrawingKind::Shape`], drawn by `sheets_scene`;
//!   text edited with [`Workbook::set_shape_text`]), and anything else as
//!   [`DrawingKind::Other`], which the grid shows as a box.
//! - **Live data**: [`Workbook::chart_reference`] reads the cells a chart
//!   series points at (`c:f`), with the rows and columns inserted or deleted
//!   since opening applied, so a chart follows edits.
//! - **Edits**: [`Workbook::place_drawing`] and [`Workbook::delete_drawing`]
//!   are undo steps of their own (no IronCalc step). Each history state keeps
//!   the edits made to reach it; anchors placed before rows or columns were
//!   inserted follow them. On save, `preserve` writes the edited objects'
//!   anchors (or drops their elements) into the carried drawing part.
//!
//! Objects are identified by sheet and position in the drawing as opened;
//! saving matches them in the saved package by their `cNvPr` id, so objects
//! whose id isn't unique in their drawing can't be edited.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Result, bail};
use roxmltree::{Document, Node};

use crate::Workbook;
use crate::shift::{Layout, Moves};

pub const NS_XDR: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const NS_C: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
const NS_CX: &str = "http://schemas.microsoft.com/office/drawing/2014/chartex";

/// EMU per pixel at 96 dpi.
pub const EMU_PER_PX: f64 = 9525.0;

/// A cell corner with an offset into the cell (EMU), 0-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Marker {
    pub col: u32,
    pub col_off: i64,
    pub row: u32,
    pub row_off: i64,
}

/// How a two-cell anchored object follows changes to its cells (`editAs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EditAs {
    /// Moves and resizes with the cells.
    #[default]
    TwoCell,
    /// Moves with the cells, keeps its size.
    OneCell,
    /// Stays put.
    Absolute,
}

/// Where an object sits on its sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    TwoCell {
        from: Marker,
        to: Marker,
        edit_as: EditAs,
    },
    /// A corner and a size (EMU).
    OneCell { from: Marker, cx: i64, cy: i64 },
    /// A position from the sheet's top left and a size (EMU).
    Absolute { x: i64, y: i64, cx: i64, cy: i64 },
}

/// Where cells are on some grid, in its own units (logical px for Kantor's
/// grid). Cells needn't be Excel's size: offsets and sizes in the file (in
/// Excel's px) are scaled by [`SheetGeometry::scale`] and offsets are kept
/// inside their cell, so an object covers the same cells as in Excel.
pub trait SheetGeometry {
    fn col_left(&self, col: u32) -> f64;
    fn col_width(&self, col: u32) -> f64;
    fn row_top(&self, row: u32) -> f64;
    fn row_height(&self, row: u32) -> f64;
    /// The column under `x` (the last one for `x` past the end).
    fn col_at(&self, x: f64) -> u32;
    fn row_at(&self, y: f64) -> u32;
    /// Grid units per Excel pixel, across and down.
    fn scale(&self) -> (f64, f64);
    /// How wide and tall the hidden columns and rows of `from..to` (cells)
    /// would be shown: an object that doesn't size with its cells keeps
    /// them (Excel's "Move but don't size with cells").
    fn hidden_extent(&self, _from: (u32, u32), _to: (u32, u32)) -> (f64, f64) {
        (0.0, 0.0)
    }
}

fn emu_px(emu: i64) -> f64 {
    emu as f64 / EMU_PER_PX
}

fn px_emu(px: f64) -> i64 {
    (px * EMU_PER_PX).round() as i64
}

impl Marker {
    /// The corner's position on `g`.
    pub fn point(&self, g: &dyn SheetGeometry) -> (f64, f64) {
        let (sx, sy) = g.scale();
        let x =
            g.col_left(self.col) + (emu_px(self.col_off) * sx).clamp(0.0, g.col_width(self.col));
        let y =
            g.row_top(self.row) + (emu_px(self.row_off) * sy).clamp(0.0, g.row_height(self.row));
        (x, y)
    }

    /// The corner at `(x, y)` on `g`.
    pub fn at(x: f64, y: f64, g: &dyn SheetGeometry) -> Marker {
        let (sx, sy) = g.scale();
        let (x, y) = (x.max(0.0), y.max(0.0));
        let col = g.col_at(x);
        let row = g.row_at(y);
        Marker {
            col,
            col_off: px_emu(((x - g.col_left(col)) / sx).max(0.0)),
            row,
            row_off: px_emu(((y - g.row_top(row)) / sy).max(0.0)),
        }
    }
}

impl Anchor {
    /// `[x, y, w, h]` on `g`.
    pub fn rect(&self, g: &dyn SheetGeometry) -> [f64; 4] {
        let (sx, sy) = g.scale();
        match *self {
            Anchor::TwoCell { from, to, edit_as } => {
                let (x0, y0) = from.point(g);
                let (x1, y1) = to.point(g);
                let (hw, hh) = match edit_as {
                    EditAs::TwoCell => (0.0, 0.0),
                    _ => g.hidden_extent((from.row, from.col), (to.row, to.col)),
                };
                [x0, y0, (x1 - x0).max(0.0) + hw, (y1 - y0).max(0.0) + hh]
            }
            Anchor::OneCell { from, cx, cy } => {
                let (x, y) = from.point(g);
                [x, y, emu_px(cx) * sx, emu_px(cy) * sy]
            }
            Anchor::Absolute { x, y, cx, cy } => [
                emu_px(x) * sx,
                emu_px(y) * sy,
                emu_px(cx) * sx,
                emu_px(cy) * sy,
            ],
        }
    }

    /// The same kind of anchor, placed at `[x, y, w, h]` on `g`.
    pub fn with_rect(&self, [x, y, w, h]: [f64; 4], g: &dyn SheetGeometry) -> Anchor {
        let (sx, sy) = g.scale();
        let (w, h) = (w.max(1.0), h.max(1.0));
        let (x, y) = (x.max(0.0), y.max(0.0));
        match *self {
            Anchor::TwoCell { edit_as, .. } => Anchor::TwoCell {
                from: Marker::at(x, y, g),
                to: Marker::at(x + w, y + h, g),
                edit_as,
            },
            Anchor::OneCell { .. } => Anchor::OneCell {
                from: Marker::at(x, y, g),
                cx: px_emu(w / sx),
                cy: px_emu(h / sy),
            },
            Anchor::Absolute { .. } => Anchor::Absolute {
                x: px_emu(x / sx),
                y: px_emu(y / sy),
                cx: px_emu(w / sx),
                cy: px_emu(h / sy),
            },
        }
    }

    /// The anchor after rows and columns of `sheet` (a `sheetId`) moved:
    /// cell corners follow them, as `preserve` moves them on save.
    pub(crate) fn moved(&self, moves: &Moves, sheet: u32) -> Anchor {
        let m = |k: Marker| {
            let (row, col) = moves.point(sheet, k.row, k.col);
            Marker { row, col, ..k }
        };
        match *self {
            Anchor::TwoCell { from, to, edit_as } => Anchor::TwoCell {
                from: m(from),
                to: m(to),
                edit_as,
            },
            Anchor::OneCell { from, cx, cy } => Anchor::OneCell {
                from: m(from),
                cx,
                cy,
            },
            a @ Anchor::Absolute { .. } => a,
        }
    }

    /// The anchor's position elements, written with prefix `p` (`xdr:`).
    pub(crate) fn position_xml(&self, p: &str) -> String {
        let marker = |tag: &str, k: Marker| {
            format!(
                "<{p}{tag}><{p}col>{}</{p}col><{p}colOff>{}</{p}colOff><{p}row>{}</{p}row><{p}rowOff>{}</{p}rowOff></{p}{tag}>",
                k.col, k.col_off, k.row, k.row_off
            )
        };
        let ext = |cx: i64, cy: i64| format!("<{p}ext cx=\"{cx}\" cy=\"{cy}\"/>");
        match *self {
            Anchor::TwoCell { from, to, .. } => marker("from", from) + &marker("to", to),
            Anchor::OneCell { from, cx, cy } => marker("from", from) + &ext(cx, cy),
            Anchor::Absolute { x, y, cx, cy } => {
                format!("<{p}pos x=\"{x}\" y=\"{y}\"/>") + &ext(cx, cy)
            }
        }
    }
}

/// What an object is.
#[derive(Clone, Debug)]
pub enum DrawingKind {
    /// A chart: its part's name and XML (`c:chartSpace`). `current_refs`:
    /// its references are in the current rows and columns (a chart made in
    /// Kantor; a file's are in the opened file's, see
    /// [`Workbook::chart_reference`]).
    Chart {
        part: Arc<str>,
        xml: Arc<str>,
        current_refs: bool,
    },
    /// A picture: its media part, bytes and cropping (left, top, right,
    /// bottom as fractions of the picture; negative = padding).
    Picture {
        part: Arc<str>,
        data: Arc<Vec<u8>>,
        crop: [f32; 4],
    },
    /// A shape, text box, connector or group: its element's XML, which
    /// declares the namespaces it uses (it parses on its own).
    Shape { xml: Arc<str> },
    /// A slicer (`sle:slicer` in `mc:AlternateContent`), by name: see
    /// [`crate::Workbook::slicer_view`].
    Slicer { name: String },
    /// A timeline (`tsle:timeslicer`), by name: see
    /// [`crate::Workbook::timeline_view`].
    Timeline { name: String },
    /// Something not drawn (a chart whose part is missing, an OLE
    /// object...): the element's name.
    Other(String),
}

/// An object as the file was opened.
#[derive(Debug)]
pub(crate) struct Object {
    /// `cNvPr id`, when unique in its drawing.
    id: Option<u32>,
    name: String,
    descr: String,
    anchor: Anchor,
    kind: Arc<DrawingKind>,
    hidden: bool,
    prints: bool,
    /// The anchor element as written (to put it back after a delete was
    /// saved and then undone), and the namespaces declared around it.
    xml: String,
    namespaces: Arc<str>,
    /// Inside `mc:AlternateContent` (a form control's shape): shown, never
    /// edited.
    alternate: bool,
}

/// An object on a sheet as it is now.
#[derive(Clone, Debug)]
pub struct Drawing {
    pub key: DrawingKey,
    pub name: String,
    /// Alternative text (`descr`).
    pub descr: String,
    pub anchor: Anchor,
    pub kind: Arc<DrawingKind>,
    /// Whether it can be moved, resized and deleted.
    pub editable: bool,
    /// Hidden (`cNvPr hidden="1"`): neither shown nor printed.
    pub hidden: bool,
    /// Printed with the sheet (`clientData fPrintsWithSheet`, on unless
    /// `"0"`).
    pub prints: bool,
}

/// Identifies an object: `sheetId` and position in the opened drawing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DrawingKey {
    pub sheet_id: u32,
    pub index: usize,
}

/// An edit of one object, in the coordinates of the state it was made in.
#[derive(Clone)]
pub(crate) enum Edit {
    Placed { anchor: Anchor, layout: Layout },
    Deleted,
}

/// The edits leading to one history state.
pub(crate) type Edits = HashMap<DrawingKey, Edit>;

/// What an edited object is at save time (see `preserve`).
#[derive(Clone, Debug)]
pub(crate) struct SaveEdit {
    pub id: u32,
    /// `None`: deleted.
    pub anchor: Option<Anchor>,
    /// The anchor element as opened, written back if the saved package lost it.
    pub xml: String,
    /// The namespace declarations it needs (`xmlns:xdr="..." ...`).
    pub namespaces: String,
    /// Its object element as it is now, when its text was edited this
    /// session (written in place of the one in the file).
    pub object: Option<String>,
}

/// The drawings of the opened file.
#[derive(Default)]
pub(crate) struct Drawings {
    sheets: HashMap<u32, Vec<Object>>,
    /// The workbook theme's XML.
    pub theme: Option<Arc<str>>,
    /// Sheet names as opened (lower case) -> `sheetId`.
    pub(crate) names: HashMap<String, u32>,
    /// Objects ever edited this session: their anchors are written on every
    /// save (a saved edit that is undone must be undone in the next file).
    touched: HashSet<DrawingKey>,
    /// The file's chart sheets as opened, in tab order (`preserve` carries
    /// their parts through save; renamed, moved, hidden or deleted ones
    /// follow [`crate::edits::Edits::chart_sheets`]).
    chart_sheets: Vec<ChartSheet>,
    /// How many of `chart_sheets` the file had (the others were made in
    /// Kantor this session: Move Chart, a copied chart sheet).
    opened_chart_sheets: usize,
    /// Chart sheets whose tab colour was changed this session: every save
    /// writes their colour (a change saved, then undone, is undone in the
    /// next file too).
    pub(crate) tab_touched: HashSet<usize>,
}

impl Drawings {
    /// The names of the slicers the opened file has anchors for on the
    /// sheet with `sheet_id`.
    pub(crate) fn opened_slicers(&self, sheet_id: u32) -> Vec<String> {
        self.opened_names(sheet_id, false)
    }

    /// The names of the timelines the opened file has anchors for on the
    /// sheet with `sheet_id`.
    pub(crate) fn opened_timelines(&self, sheet_id: u32) -> Vec<String> {
        self.opened_names(sheet_id, true)
    }

    fn opened_names(&self, sheet_id: u32, timelines: bool) -> Vec<String> {
        self.sheets
            .get(&sheet_id)
            .map(|objects| {
                objects
                    .iter()
                    .filter_map(|o| match (&*o.kind, timelines) {
                        (DrawingKind::Slicer { name }, false)
                        | (DrawingKind::Timeline { name }, true) => Some(name.to_lowercase()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// A chart sheet as opened.
#[derive(Debug)]
struct ChartSheet {
    name: String,
    state: Option<String>,
    /// `sheetId`s of the worksheets before it in the file, nearest first.
    before: Vec<u32>,
    /// Its part (`xl/chartsheets/sheet1.xml`): carrying keeps part names,
    /// so this finds it in every later save of the session.
    part: String,
    rel_kind: String,
    /// Its parts as opened, to put back if a save left it out (deleted,
    /// saved, then the deletion undone).
    bundle: Arc<Vec<crate::preserve::BundlePart>>,
    objects: Vec<Object>,
    print: crate::SheetPrint,
    /// Its tab colour as opened (see [`ChartSheetNow::tab`]).
    tab: Option<String>,
}

/// The attributes of a chart sheet part's `<sheetPr><tabColor>`.
pub(crate) fn chart_sheet_tab(xml: &str) -> Option<String> {
    let doc = Document::parse(xml).ok()?;
    let t = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "sheetPr")?
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "tabColor")?;
    let attrs: Vec<String> = t
        .attributes()
        .map(|a| format!("{}=\"{}\"", a.name(), esc(a.value())))
        .collect();
    (!attrs.is_empty()).then(|| attrs.join(" "))
}

/// See [`Drawings::chart_sheet_parts`].
pub(crate) struct ChartSheetParts {
    pub part: String,
    pub rel_kind: String,
    pub bundle: Arc<Vec<crate::preserve::BundlePart>>,
    /// Its charts: part name and XML as opened.
    pub charts: Vec<(String, Arc<str>)>,
}

/// A chart sheet of the file in a history state: what it is called, whether
/// it is hidden and where its tab is. Chart sheets missing from the state's
/// list were deleted.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ChartSheetNow {
    /// Its index among the chart sheets as opened.
    pub k: usize,
    pub name: String,
    /// `None` (visible), `hidden` or `veryHidden`.
    pub state: Option<String>,
    /// `sheetId`s of worksheets it goes after, nearest first: its tab
    /// follows the first of them still there (none: the first tabs).
    pub after: Vec<u32>,
    /// Its tab colour: `<tabColor>`'s attributes (`rgb="FF4472C4"`).
    pub tab: Option<String>,
}

/// A sheet tab: a worksheet (its index) or a chart sheet (its index among
/// [`Workbook::chart_sheet_name`]'s).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    Sheet(u32),
    Chart(usize),
}

/// The `sheetId` in the [`DrawingKey`]s of chart sheet `k`'s objects: far
/// from any worksheet's, so caches keyed by it never mix the two.
pub fn chart_sheet_key_id(k: usize) -> u32 {
    u32::MAX - 1 - k as u32
}

/// Which chart sheet a [`DrawingKey::sheet_id`] made by
/// [`chart_sheet_key_id`] stands for (`None`: a worksheet's).
pub fn chart_sheet_of_key(sheet_id: u32) -> Option<usize> {
    // Far above any real sheetId (Excel's stay below 65536).
    let k = (u32::MAX - 1).checked_sub(sheet_id)? as usize;
    (k < 1 << 20).then_some(k)
}

fn is(n: Node, ns: &str, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some(ns)
}

fn child<'a, 'i>(n: Node<'a, 'i>, ns: &str, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| is(*c, ns, name))
}

fn num<T: std::str::FromStr>(n: Node, name: &str) -> Option<T> {
    child(n, NS_XDR, name)?.text()?.trim().parse().ok()
}

fn marker(n: Option<Node>) -> Option<Marker> {
    let n = n?;
    Some(Marker {
        col: num(n, "col")?,
        col_off: num(n, "colOff").unwrap_or(0),
        row: num(n, "row")?,
        row_off: num(n, "rowOff").unwrap_or(0),
    })
}

fn ext(n: Option<Node>) -> Option<(i64, i64)> {
    let n = n?;
    Some((
        n.attribute("cx")?.parse().ok()?,
        n.attribute("cy")?.parse().ok()?,
    ))
}

/// An anchor element's position.
pub(crate) fn parse_anchor(a: Node) -> Option<Anchor> {
    match a.tag_name().name() {
        "twoCellAnchor" => Some(Anchor::TwoCell {
            from: marker(child(a, NS_XDR, "from"))?,
            to: marker(child(a, NS_XDR, "to"))?,
            edit_as: match a.attribute("editAs") {
                Some("oneCell") => EditAs::OneCell,
                Some("absolute") => EditAs::Absolute,
                _ => EditAs::TwoCell,
            },
        }),
        "oneCellAnchor" => {
            let (cx, cy) = ext(child(a, NS_XDR, "ext"))?;
            Some(Anchor::OneCell {
                from: marker(child(a, NS_XDR, "from"))?,
                cx,
                cy,
            })
        }
        "absoluteAnchor" => {
            let pos = child(a, NS_XDR, "pos")?;
            let (cx, cy) = ext(child(a, NS_XDR, "ext"))?;
            Some(Anchor::Absolute {
                x: pos.attribute("x")?.parse().ok()?,
                y: pos.attribute("y")?.parse().ok()?,
                cx,
                cy,
            })
        }
        _ => None,
    }
}

/// Whether `n` is an anchor element.
pub(crate) fn is_anchor(n: Node) -> bool {
    n.is_element()
        && n.tag_name().namespace() == Some(NS_XDR)
        && matches!(
            n.tag_name().name(),
            "twoCellAnchor" | "oneCellAnchor" | "absoluteAnchor"
        )
}

/// The object element of an anchor (`pic`, `graphicFrame`, `sp`...).
fn object_element<'a, 'i>(a: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    a.children().find(|c| {
        c.is_element()
            && !matches!(
                c.tag_name().name(),
                "from" | "to" | "ext" | "pos" | "clientData"
            )
    })
}

/// The object's `cNvPr` (its id, name and description).
pub(crate) fn c_nv_pr<'a, 'i>(a: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    let obj = object_element(a)?;
    obj.descendants()
        .find(|d| is(*d, NS_XDR, "cNvPr") || is(*d, NS_A, "cNvPr"))
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

fn r_id<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
    n.attributes()
        .find(|a| {
            a.name() == name
                && a.namespace()
                    .is_some_and(|ns| ns.ends_with("/relationships"))
        })
        .map(|a| a.value())
}

/// `a:srcRect` as fractions (it's in 1/1000 of a percent).
fn crop(blip_fill: Node) -> [f32; 4] {
    let Some(r) = child(blip_fill, NS_A, "srcRect") else {
        return [0.0; 4];
    };
    let f = |k: &str| {
        r.attribute(k)
            .and_then(|v| v.parse::<f32>().ok())
            .map_or(0.0, |v| (v / 100_000.0).clamp(-10.0, 0.99))
    };
    [f("l"), f("t"), f("r"), f("b")]
}

fn kind(
    obj: Node,
    targets: &HashMap<String, (String, String)>,
    parts: &mut HashMap<String, Arc<[u8]>>,
) -> DrawingKind {
    let target = |rid: Option<&str>, want: &str| {
        let (kind, part) = targets.get(rid?)?;
        (kind == want).then_some(part.clone())
    };
    let other = || DrawingKind::Other(obj.tag_name().name().to_owned());
    match obj.tag_name().name() {
        "graphicFrame" => {
            // ChartML, or the Office 2016 types (`cx:chart`, inside an
            // `mc:AlternateContent` choice).
            let chart = obj
                .descendants()
                .find(|d| is(*d, NS_C, "chart"))
                .and_then(|c| r_id(c, "id"));
            let chartex = obj
                .descendants()
                .find(|d| is(*d, NS_CX, "chart"))
                .and_then(|c| r_id(c, "id"));
            let Some(part) = target(chart, "chart").or_else(|| target(chartex, "chartEx")) else {
                return other();
            };
            match parts.get(&part).and_then(|b| {
                std::str::from_utf8(b.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(b)).ok()
            }) {
                Some(xml) => DrawingKind::Chart {
                    xml: xml.into(),
                    part: part.into(),
                    current_refs: false,
                },
                None => other(),
            }
        }
        "pic" => {
            let Some(fill) = child(obj, NS_XDR, "blipFill") else {
                return other();
            };
            let blip = child(fill, NS_A, "blip").and_then(|b| r_id(b, "embed"));
            let Some(part) = target(blip, "image") else {
                return other();
            };
            match parts.get(&part) {
                Some(data) => DrawingKind::Picture {
                    data: Arc::new(data.to_vec()),
                    crop: crop(fill),
                    part: part.into(),
                },
                None => other(),
            }
        }
        // Excel 2010+ objects with a fallback shape: slicers and timelines.
        "AlternateContent" => {
            let choice = obj
                .children()
                .find(|c| c.is_element() && c.tag_name().name() == "Choice");
            let data = choice.and_then(|c| {
                c.descendants()
                    .find(|d| d.is_element() && d.tag_name().name() == "graphicData")
            });
            let found = data.and_then(|d| {
                d.children()
                    .find(|c| {
                        c.is_element() && matches!(c.tag_name().name(), "slicer" | "timeslicer")
                    })
                    .map(|c| {
                        (
                            c.tag_name().name() == "slicer",
                            c.attribute("name").unwrap_or_default().to_owned(),
                        )
                    })
            });
            match found {
                Some((true, name)) => DrawingKind::Slicer { name },
                Some((false, name)) => DrawingKind::Timeline { name },
                None => other(),
            }
        }
        _ => other(),
    }
}

/// The objects of a drawing part.
fn read_objects(
    xml: &str,
    targets: &HashMap<String, (String, String)>,
    parts: HashMap<String, Vec<u8>>,
) -> Vec<Object> {
    let doc = match Document::parse(xml) {
        Ok(d) => d,
        Err(e) => {
            log::warn!("drawing not read: {e}");
            return Vec::new();
        }
    };
    let mut parts: HashMap<String, Arc<[u8]>> =
        parts.into_iter().map(|(k, v)| (k, v.into())).collect();
    let mut out = Vec::new();
    let mut ids: HashMap<u32, usize> = HashMap::new();
    let namespaces: Arc<str> = doc
        .root_element()
        .namespaces()
        .map(|ns| match ns.name() {
            Some(p) => format!("xmlns:{p}=\"{}\"", esc(ns.uri())),
            None => format!("xmlns=\"{}\"", esc(ns.uri())),
        })
        .collect::<Vec<_>>()
        .join(" ")
        .into();
    // Anchors, and those in `mc:AlternateContent` (form controls, charts
    // of the newer types), whose first choice Excel shows.
    let anchors = doc.root_element().children().flat_map(|n| {
        if is_anchor(n) {
            vec![(n, false)]
        } else if n.is_element() && n.tag_name().name() == "AlternateContent" {
            n.children()
                .find(|c| c.is_element() && c.tag_name().name() == "Choice")
                .map(|c| {
                    c.children()
                        .filter(|a| is_anchor(*a))
                        .map(|a| (a, true))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        }
    });
    for (a, alternate) in anchors {
        let Some(anchor) = parse_anchor(a) else {
            continue;
        };
        let nv = c_nv_pr(a);
        let id = nv.and_then(|n| n.attribute("id")?.parse::<u32>().ok());
        if let Some(id) = id {
            *ids.entry(id).or_default() += 1;
        }
        let kind = match object_element(a) {
            Some(obj) if is_shape(obj) => DrawingKind::Shape {
                xml: standalone(&xml[obj.range()], &in_scope(obj)).into(),
            },
            Some(obj) => kind(obj, targets, &mut parts),
            None => DrawingKind::Other("anchor".into()),
        };
        // Shapes of form controls and charts of the newer types (shown,
        // not edited); other alternate content stays unseen.
        if alternate && !matches!(kind, DrawingKind::Shape { .. } | DrawingKind::Chart { .. }) {
            continue;
        }
        out.push(Object {
            id,
            name: nv
                .and_then(|n| n.attribute("name"))
                .unwrap_or_default()
                .to_owned(),
            descr: nv
                .and_then(|n| n.attribute("descr"))
                .unwrap_or_default()
                .to_owned(),
            anchor,
            kind: Arc::new(kind),
            hidden: nv
                .and_then(|n| n.attribute("hidden"))
                .is_some_and(|v| v == "1" || v == "true"),
            prints: child(a, NS_XDR, "clientData")
                .and_then(|c| c.attribute("fPrintsWithSheet"))
                .is_none_or(|v| v != "0" && v != "false"),
            xml: xml[a.range()].to_owned(),
            namespaces: namespaces.clone(),
            alternate,
        });
    }
    for o in &mut out {
        if o.alternate || o.id.is_some_and(|id| ids[&id] > 1) {
            o.id = None;
        }
    }
    out
}

/// The namespace declarations in scope at `n` (`xmlns:p="..." ...`).
fn in_scope(n: Node) -> String {
    n.namespaces()
        .filter(|ns| ns.name() != Some("xml"))
        .map(|ns| match ns.name() {
            Some(p) => format!("xmlns:{p}=\"{}\"", esc(ns.uri())),
            None => format!("xmlns=\"{}\"", esc(ns.uri())),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether `obj` is a shape, text box, connector or group (in the
/// SpreadsheetML drawing namespace).
fn is_shape(obj: Node) -> bool {
    obj.tag_name().namespace() == Some(NS_XDR)
        && matches!(obj.tag_name().name(), "sp" | "cxnSp" | "grpSp")
}

/// An element's XML with the namespace declarations `namespaces`
/// (`xmlns:p="..."`, as declared around it) added to its start tag, so it
/// parses on its own. Declarations it makes itself are kept.
pub(crate) fn standalone(element: &str, namespaces: &str) -> String {
    let Some(end) = element.find(|c: char| c.is_whitespace() || c == '>' || c == '/') else {
        return element.to_owned();
    };
    let tag_end = element.find('>').unwrap_or(element.len());
    let start_tag = &element[..tag_end];
    let mut add = String::new();
    for decl in namespaces.split(' ').filter(|d| !d.is_empty()) {
        let name = decl.split('=').next().unwrap_or_default();
        let own = start_tag
            .split(|c: char| c.is_whitespace())
            .any(|t| t.split('=').next() == Some(name));
        if !own {
            add.push(' ');
            add.push_str(decl);
        }
    }
    format!("{}{add}{}", &element[..end], &element[end..])
}

impl Drawings {
    pub(crate) fn read(bytes: &[u8]) -> Self {
        let src = match crate::preserve::drawing_sources(bytes) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("drawings not read: {e}");
                return Self::default();
            }
        };
        let mut sheets = HashMap::new();
        for d in src.drawings {
            let objects = read_objects(&d.xml, &d.targets, d.parts);
            if !objects.is_empty() {
                sheets.insert(d.sheet_id, objects);
            }
        }
        let chart_sheets: Vec<ChartSheet> = src
            .chart_sheets
            .into_iter()
            .map(|c| ChartSheet {
                name: c.name,
                state: c.state.filter(|v| v != "visible"),
                before: c.before,
                part: c.part,
                rel_kind: c.rel_kind,
                bundle: Arc::new(c.bundle),
                objects: c
                    .drawing
                    .map(|d| read_objects(&d.xml, &d.targets, d.parts))
                    .unwrap_or_default(),
                print: crate::print::chart_sheet_print(c.xml.as_deref().unwrap_or_default()),
                tab: c.xml.as_deref().and_then(chart_sheet_tab),
            })
            .collect();
        Drawings {
            opened_chart_sheets: chart_sheets.len(),
            tab_touched: HashSet::new(),
            sheets,
            theme: src.theme.map(Into::into),
            names: src
                .sheets
                .into_iter()
                .map(|(n, id)| (n.to_lowercase(), id))
                .collect(),
            touched: HashSet::new(),
            chart_sheets,
        }
    }

    pub(crate) fn chart_sheet_count(&self) -> usize {
        self.chart_sheets.len()
    }

    /// Add a chart sheet made in Kantor (it is in no history state until
    /// one lists it): its parts, a drawing with chart `chart_xml`. Returns
    /// its index `k`.
    pub(crate) fn add_chart_sheet(&mut self, new: crate::move_chart::NewChartSheet) -> usize {
        let k = self.chart_sheets.len();
        let mut targets = HashMap::new();
        targets.insert(
            "rId1".to_owned(),
            ("chart".to_owned(), new.chart_part.clone()),
        );
        let mut parts = HashMap::new();
        parts.insert(new.chart_part.clone(), new.chart_xml.clone().into_bytes());
        let objects = read_objects(&new.drawing_xml, &targets, parts);
        self.chart_sheets.push(ChartSheet {
            name: new.name,
            state: None,
            before: Vec::new(),
            part: new.part,
            rel_kind: crate::move_chart::CHARTSHEET_REL.to_owned(),
            print: crate::print::chart_sheet_print(&new.sheet_xml),
            tab: chart_sheet_tab(&new.sheet_xml),
            bundle: Arc::new(new.bundle),
            objects,
        });
        k
    }

    /// Chart sheet `k`'s parts as opened, for a save.
    pub(crate) fn chart_sheet_parts(&self, k: usize) -> Option<ChartSheetParts> {
        let c = self.chart_sheets.get(k)?;
        Some(ChartSheetParts {
            part: c.part.clone(),
            rel_kind: c.rel_kind.clone(),
            bundle: c.bundle.clone(),
            charts: c
                .objects
                .iter()
                .filter_map(|o| match &*o.kind {
                    DrawingKind::Chart { part, xml, .. } => Some((part.to_string(), xml.clone())),
                    _ => None,
                })
                .collect(),
        })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.sheets.is_empty() && self.chart_sheets.is_empty()
    }

    /// The object `key` as opened, on a worksheet or a chart sheet.
    fn object(&self, key: DrawingKey) -> Option<&Object> {
        match chart_sheet_of_key(key.sheet_id) {
            Some(k) => self.chart_sheets.get(k)?.objects.get(key.index),
            None => self.sheets.get(&key.sheet_id)?.get(key.index),
        }
    }

    /// A chart of the file: its frame's `cNvPr id` and its part's XML as
    /// opened (`None`: not a chart, or its id isn't unique).
    pub(crate) fn chart_source(&self, key: DrawingKey) -> Option<(u32, Arc<str>)> {
        let obj = self.object(key)?;
        let DrawingKind::Chart { xml, .. } = &*obj.kind else {
            return None;
        };
        Some((obj.id?, xml.clone()))
    }

    /// Every edited object's state for a save, by `sheetId`: moved,
    /// resized or deleted ones, and those whose text was edited (`texts`
    /// has the current state's, `text_touched` every one edited this
    /// session).
    pub(crate) fn save_edits(
        &self,
        edits: &Edits,
        now: &Layout,
        texts: &HashMap<DrawingKey, Arc<str>>,
        text_touched: &HashSet<DrawingKey>,
    ) -> HashMap<u32, Vec<SaveEdit>> {
        let mut out: HashMap<u32, Vec<SaveEdit>> = HashMap::new();
        let mut keys: Vec<&DrawingKey> = self.touched.iter().collect();
        keys.extend(text_touched.iter().filter(|k| !self.touched.contains(k)));
        for key in keys {
            let Some(obj) = self
                .sheets
                .get(&key.sheet_id)
                .and_then(|v| v.get(key.index))
            else {
                continue;
            };
            let Some(id) = obj.id else { continue };
            let object = text_touched.contains(key).then(|| match texts.get(key) {
                Some(x) => x.to_string(),
                None => match &*obj.kind {
                    DrawingKind::Shape { xml } => xml.to_string(),
                    _ => String::new(),
                },
            });
            out.entry(key.sheet_id).or_default().push(SaveEdit {
                id,
                anchor: current_anchor(obj, *key, edits, now),
                xml: obj.xml.clone(),
                namespaces: obj.namespaces.to_string(),
                object: object.filter(|o| !o.is_empty()),
            });
        }
        out
    }
}

/// Where an object is in the state with `edits` and `now` (`None`: deleted).
fn current_anchor(obj: &Object, key: DrawingKey, edits: &Edits, now: &Layout) -> Option<Anchor> {
    match edits.get(&key) {
        Some(Edit::Deleted) => None,
        Some(Edit::Placed { anchor, layout }) => {
            Some(anchor.moved(&layout.moves_to(now), key.sheet_id))
        }
        None => Some(
            obj.anchor
                .moved(&Layout::default().moves_to(now), key.sheet_id),
        ),
    }
}

/// Where the objects of a chart sheet go in a `w` × `h` pane: absolute
/// anchors (what chart sheets use) scaled so their extent fills it, any
/// other anchor the whole pane.
pub fn chart_sheet_rects(drawings: &[Drawing], w: f64, h: f64) -> Vec<[f64; 4]> {
    let (mut ew, mut eh) = (0i64, 0i64);
    for d in drawings {
        if let Anchor::Absolute { x, y, cx, cy } = d.anchor {
            ew = ew.max(x + cx);
            eh = eh.max(y + cy);
        }
    }
    drawings
        .iter()
        .map(|d| match d.anchor {
            Anchor::Absolute { x, y, cx, cy } if ew > 0 && eh > 0 => {
                let (sx, sy) = (w / ew as f64, h / eh as f64);
                [x as f64 * sx, y as f64 * sy, cx as f64 * sx, cy as f64 * sy]
            }
            _ => [0.0, 0.0, w, h],
        })
        .collect()
}

/// A value in a range a chart refers to.
#[derive(Clone, Debug, PartialEq)]
pub enum RefValue {
    Number(f64),
    Text(String),
    Empty,
}

/// The cells of a chart reference, in order (rows of the first area, then
/// the next area...).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RefData {
    pub values: Vec<RefValue>,
    /// What each cell shows (formatted).
    pub shown: Vec<String>,
    /// The number format of the first number cell.
    pub format: Option<String>,
}

/// Most cells read for one chart reference.
const MAX_REF_CELLS: usize = 100_000;

impl Workbook {
    /// The charts, pictures and other objects drawn over `sheet` (an index),
    /// where they are now, bottom to top.
    pub fn drawings(&self, sheet: u32) -> Vec<Drawing> {
        let sheet_id = self.sheet_id(sheet);
        let mut out = self.file_drawings(sheet_id);
        out.extend(self.new_chart_drawings(sheet_id));
        out
    }

    fn file_drawings(&self, sheet_id: u32) -> Vec<Drawing> {
        let Some(objects) = self.drawings.sheets.get(&sheet_id) else {
            return Vec::new();
        };
        let edits = self.history.drawing_edits();
        let now = self.history.layout();
        let restyled = self.edits();
        objects
            .iter()
            .enumerate()
            .filter_map(|(index, obj)| {
                let key = DrawingKey { sheet_id, index };
                Some(Drawing {
                    key,
                    name: obj.name.clone(),
                    descr: obj.descr.clone(),
                    anchor: current_anchor(obj, key, &edits, &now)?,
                    kind: match (&*obj.kind, restyled.file_charts.contains_key(&key)) {
                        (DrawingKind::Shape { .. }, _)
                            if restyled.shape_texts.contains_key(&key) =>
                        {
                            Arc::new(DrawingKind::Shape {
                                xml: restyled.shape_texts[&key].clone(),
                            })
                        }
                        (DrawingKind::Chart { part, .. }, true) => {
                            let (xml, _) = self.file_chart_now(key)?;
                            Arc::new(DrawingKind::Chart {
                                part: format!(
                                    "{part}#kantor/{:x}",
                                    crate::chart_edit::fxhash(&xml)
                                )
                                .into(),
                                xml: xml.into(),
                                current_refs: true,
                            })
                        }
                        _ => obj.kind.clone(),
                    },
                    editable: obj.id.is_some(),
                    hidden: obj.hidden,
                    prints: obj.prints,
                })
            })
            .collect()
    }

    /// Whether any sheet has drawings.
    pub fn has_drawings(&self) -> bool {
        !self.drawings.is_empty() || !self.edits().charts.is_empty()
    }

    /// The file's chart sheets in the current state, in their order (hidden
    /// ones too; deleted ones are left out).
    pub(crate) fn chart_sheets_now(&self) -> Vec<ChartSheetNow> {
        if let Some(list) = &self.edits().chart_sheets {
            return (**list).clone();
        }
        self.drawings
            .chart_sheets
            .iter()
            .take(self.drawings.opened_chart_sheets)
            .enumerate()
            .map(|(k, c)| ChartSheetNow {
                k,
                name: c.name.clone(),
                state: c.state.clone(),
                after: c.before.clone(),
                tab: c.tab.clone(),
            })
            .collect()
    }

    fn chart_sheet_now(&self, k: usize) -> Option<ChartSheetNow> {
        self.chart_sheets_now().into_iter().find(|c| c.k == k)
    }

    /// Every tab in order, hidden chart sheets included: the worksheets,
    /// with the chart sheets where saving puts them (after the nearest
    /// worksheet before them that is still there, as `preserve` does).
    /// Chart sheets a worksheet took the name of (saving drops them) are
    /// left out.
    pub(crate) fn all_tabs(&self) -> Vec<Tab> {
        let names = self.sheet_names();
        let n = names.len() as u32;
        let ids: Vec<u32> = (0..n).map(|i| self.sheet_id(i)).collect();
        let lower: HashSet<String> = names.iter().map(|s| s.to_lowercase()).collect();
        let mut tabs: Vec<Tab> = (0..n).map(Tab::Sheet).collect();
        for cs in self.chart_sheets_now() {
            if lower.contains(&cs.name.to_lowercase()) {
                continue;
            }
            let mut at = cs
                .after
                .iter()
                .find_map(|id| ids.iter().position(|x| x == id))
                .and_then(|i| tabs.iter().position(|t| *t == Tab::Sheet(i as u32)))
                .map_or(0, |p| p + 1);
            // After the chart sheets put there before (they came first).
            while matches!(tabs.get(at), Some(Tab::Chart(_))) {
                at += 1;
            }
            tabs.insert(at, Tab::Chart(cs.k));
        }
        tabs
    }

    /// The tabs in order: the worksheets, with the file's chart sheets
    /// where saving puts them (after the nearest worksheet before them
    /// that is still there, as `preserve` does). Hidden sheets, and
    /// those a worksheet took the name of (saving drops them), are left
    /// out.
    pub fn tabs(&self) -> Vec<Tab> {
        let hidden: HashSet<usize> = self
            .chart_sheets_now()
            .into_iter()
            .filter(|c| c.state.is_some())
            .map(|c| c.k)
            .collect();
        self.all_tabs()
            .into_iter()
            .filter(|t| match t {
                Tab::Chart(k) => !hidden.contains(k),
                Tab::Sheet(i) => !self.sheet_hidden(*i),
            })
            .collect()
    }

    /// Chart sheet `k`'s name now (`None`: there is no such chart sheet, or
    /// it was deleted).
    pub fn chart_sheet_name(&self, k: usize) -> Option<String> {
        self.chart_sheet_now(k).map(|c| c.name)
    }

    /// Hidden chart sheets (not "very hidden" ones, which Excel doesn't
    /// list either) as `(k, name)`, for Unhide.
    pub fn hidden_chart_sheets(&self) -> Vec<(usize, String)> {
        self.chart_sheets_now()
            .into_iter()
            .filter(|c| c.state.as_deref() == Some("hidden"))
            .map(|c| (c.k, c.name))
            .collect()
    }

    /// Chart sheet `k`'s print settings (landscape unless it says
    /// otherwise).
    pub fn chart_sheet_print(&self, k: usize) -> crate::SheetPrint {
        self.drawings
            .chart_sheets
            .get(k)
            .map(|c| c.print.clone())
            .unwrap_or_default()
    }

    /// The objects of chart sheet `k` (usually one chart), bottom to top,
    /// with their anchors as in the file (chart sheets use absolute ones),
    /// charts restyled in Kantor as they are now. Charts are `editable`
    /// (in the Chart panel; they aren't moved or resized). Empty for a
    /// deleted chart sheet.
    pub fn chart_sheet_drawings(&self, k: usize) -> Vec<Drawing> {
        let Some(cs) = self.drawings.chart_sheets.get(k) else {
            return Vec::new();
        };
        if self.chart_sheet_now(k).is_none() {
            return Vec::new();
        }
        let sheet_id = chart_sheet_key_id(k);
        let restyled = self.edits();
        cs.objects
            .iter()
            .enumerate()
            .map(|(index, obj)| {
                let key = DrawingKey { sheet_id, index };
                let chart = matches!(&*obj.kind, DrawingKind::Chart { .. });
                Drawing {
                    key,
                    name: obj.name.clone(),
                    descr: obj.descr.clone(),
                    anchor: obj.anchor,
                    kind: match &*obj.kind {
                        DrawingKind::Chart { part, .. }
                            if restyled.file_charts.contains_key(&key) =>
                        {
                            match self.file_chart_now(key) {
                                Some((xml, _)) => Arc::new(DrawingKind::Chart {
                                    part: format!(
                                        "{part}#kantor/{:x}",
                                        crate::chart_edit::fxhash(&xml)
                                    )
                                    .into(),
                                    xml: xml.into(),
                                    current_refs: true,
                                }),
                                None => obj.kind.clone(),
                            }
                        }
                        _ => obj.kind.clone(),
                    },
                    editable: chart && obj.id.is_some(),
                    hidden: obj.hidden,
                    prints: obj.prints,
                }
            })
            .collect()
    }

    /// The workbook theme's XML (`xl/theme/theme1.xml`), for charts.
    pub fn theme_xml(&self) -> Option<Arc<str>> {
        self.drawings.theme.clone()
    }

    fn drawing_edit(&mut self, key: DrawingKey, edit: Edit) -> Result<()> {
        let editable = self
            .drawings
            .sheets
            .get(&key.sheet_id)
            .and_then(|v| v.get(key.index))
            .is_some_and(|o| o.id.is_some());
        if !editable {
            bail!("this object can't be edited");
        }
        self.drawings.touched.insert(key);
        let mut edits = (*self.history.drawing_edits()).clone();
        edits.insert(key, edit);
        self.history.edited_steps(0);
        self.history.set_drawing_edits(Arc::new(edits));
        Ok(())
    }

    /// Move or resize an object (one undo step); `anchor` is in the current
    /// state's rows and columns.
    pub fn place_drawing(&mut self, key: DrawingKey, anchor: Anchor) -> Result<()> {
        if key.index >= crate::chart_edit::NEW_CHART_BASE {
            return self.new_chart_edit(key, Some(anchor));
        }
        let layout = self.history.layout();
        self.drawing_edit(key, Edit::Placed { anchor, layout })
    }

    /// Delete an object (one undo step).
    pub fn delete_drawing(&mut self, key: DrawingKey) -> Result<()> {
        if key.index >= crate::chart_edit::NEW_CHART_BASE {
            return self.new_chart_edit(key, None);
        }
        self.drawing_edit(key, Edit::Deleted)
    }

    /// The cells a chart reference (`Sheet1!$B$2:$B$13`, a union in
    /// brackets) points at now, written as the opened file had it: rows and
    /// columns inserted or deleted since then are followed. `None` when it
    /// can't be read (unknown sheet, a name, a deleted range).
    pub fn chart_reference(&self, formula: &str) -> Option<RefData> {
        let moves = Layout::default()
            .moves_to(&self.history.layout())
            .with_renames(self.renamed_since_open());
        let moved = moves.formula(formula, None, &self.drawings.names);
        self.chart_reference_now(&moved)
    }

    /// The cells a chart reference points at, taken as it is (in the
    /// current rows and columns: charts made in Kantor).
    pub fn chart_reference_now(&self, moved: &str) -> Option<RefData> {
        let model = self.model.get_model();
        let mut out = RefData::default();
        let mut found = false;
        for token in crate::shift::references(moved) {
            let name = token.sheet.as_deref()?;
            let sheet = model
                .workbook
                .worksheets
                .iter()
                .position(|w| w.name.eq_ignore_ascii_case(name))? as u32;
            let a = token.range.area();
            let cells = (u64::from(a.bottom - a.top) + 1) * (u64::from(a.right - a.left) + 1);
            if out.values.len() as u64 + cells > MAX_REF_CELLS as u64 {
                return None;
            }
            found = true;
            for row in a.top..=a.bottom {
                for col in a.left..=a.right {
                    let shown = self.display_value(sheet, row, col);
                    let value = match self.raw_number(sheet, row, col) {
                        Some(n) => {
                            if out.format.is_none() {
                                out.format = self.number_format_code(sheet, row, col);
                            }
                            RefValue::Number(n)
                        }
                        None if shown.is_empty() => RefValue::Empty,
                        None => RefValue::Text(shown.clone()),
                    };
                    out.values.push(value);
                    out.shown.push(shown);
                }
            }
        }
        found.then_some(out)
    }

    /// The number a cell holds (formula results included).
    pub(crate) fn raw_number(&self, sheet: u32, row: u32, col: u32) -> Option<f64> {
        let model = self.model.get_model();
        let ws = model.workbook.worksheet(sheet).ok()?;
        let cell = ws.cell(row as i32 + 1, col as i32 + 1)?;
        match cell.value(&model.workbook.shared_strings, crate::numfmt::language()) {
            ironcalc_base::cell::CellValue::Number(n) if n.is_finite() => Some(n),
            _ => None,
        }
    }

    fn number_format_code(&self, sheet: u32, row: u32, col: u32) -> Option<String> {
        let model = self.model.get_model();
        let ws = model.workbook.worksheet(sheet).ok()?;
        let style = ws.cell(row as i32 + 1, col as i32 + 1)?.get_style();
        Some(crate::numfmt::style_code(model, style))
    }
}

#[cfg(test)]
mod tests;
