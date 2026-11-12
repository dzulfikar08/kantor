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
//!   cropping), and anything else as [`DrawingKind::Other`] (shapes, text
//!   boxes, groups...), which the grid shows as a box.
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
            Anchor::TwoCell { from, to, .. } => {
                let (x0, y0) = from.point(g);
                let (x1, y1) = to.point(g);
                [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)]
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
    /// Something not drawn (a shape, text box, group, connector, a chart
    /// whose part is missing...): the element's name.
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
}

/// The drawings of the opened file.
#[derive(Default)]
pub(crate) struct Drawings {
    sheets: HashMap<u32, Vec<Object>>,
    /// The workbook theme's XML.
    pub theme: Option<Arc<str>>,
    /// Sheet names as opened (lower case) -> `sheetId`.
    names: HashMap<String, u32>,
    /// Objects ever edited this session: their anchors are written on every
    /// save (a saved edit that is undone must be undone in the next file).
    touched: HashSet<DrawingKey>,
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
            let chart = obj
                .descendants()
                .find(|d| is(*d, NS_C, "chart"))
                .and_then(|c| r_id(c, "id"));
            let Some(part) = target(chart, "chart") else {
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
    for a in doc.root_element().children().filter(|n| is_anchor(*n)) {
        let Some(anchor) = parse_anchor(a) else {
            continue;
        };
        let nv = c_nv_pr(a);
        let id = nv.and_then(|n| n.attribute("id")?.parse::<u32>().ok());
        if let Some(id) = id {
            *ids.entry(id).or_default() += 1;
        }
        let kind = match object_element(a) {
            Some(obj) => kind(obj, targets, &mut parts),
            None => DrawingKind::Other("anchor".into()),
        };
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
        });
    }
    for o in &mut out {
        if o.id.is_some_and(|id| ids[&id] > 1) {
            o.id = None;
        }
    }
    out
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
        Drawings {
            sheets,
            theme: src.theme.map(Into::into),
            names: src
                .sheets
                .into_iter()
                .map(|(n, id)| (n.to_lowercase(), id))
                .collect(),
            touched: HashSet::new(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.sheets.is_empty()
    }

    /// Every edited object's state for a save, by `sheetId`.
    pub(crate) fn save_edits(&self, edits: &Edits, now: &Layout) -> HashMap<u32, Vec<SaveEdit>> {
        let mut out: HashMap<u32, Vec<SaveEdit>> = HashMap::new();
        for key in &self.touched {
            let Some(obj) = self
                .sheets
                .get(&key.sheet_id)
                .and_then(|v| v.get(key.index))
            else {
                continue;
            };
            let Some(id) = obj.id else { continue };
            out.entry(key.sheet_id).or_default().push(SaveEdit {
                id,
                anchor: current_anchor(obj, *key, edits, now),
                xml: obj.xml.clone(),
                namespaces: obj.namespaces.to_string(),
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
                    kind: obj.kind.clone(),
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
        let moves = Layout::default().moves_to(&self.history.layout());
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
