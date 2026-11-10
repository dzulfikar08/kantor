//! What groups and graphic frames hold, for display (never written: their
//! source XML is kept verbatim).
//!
//! - Groups (`p:grpSp`): child shapes in the group's child coordinate space
//!   (`a:chOff`/`a:chExt`); [`compose`] places a child on the slide.
//! - Tables (`a:tbl`): grid, rows, cells with merges, margins, fills and
//!   borders; table styles ([`TableStyle`], from `ppt/tableStyles.xml` or
//!   built in) resolved per cell by [`Table::cell_look`].
//! - SmartArt (`dgm:relIds`): the data part; PowerPoint's pre-drawn shapes
//!   are read on demand ([`crate::Presentation::diagram_drawing`]).
//! - Charts (`c:chart`): the chart part (drawn elsewhere).

use std::collections::HashMap;
use std::sync::Arc;

use docs_engine::Rgb;
use roxmltree::Node;

use crate::fill::{Fill, LineStyle};
use crate::model::*;
use crate::theme::{ColorCtx, LevelStyle};
use crate::xml::*;

pub const NS_DSP: &str = "http://schemas.microsoft.com/office/drawing/2008/diagram";
pub const NS_DGM: &str = "http://schemas.openxmlformats.org/drawingml/2006/diagram";
pub const NS_C: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
const URI_TABLE: &str = "http://schemas.openxmlformats.org/drawingml/2006/table";

/// Display content of a group or graphic frame. Compares by identity (as
/// a shape's source does).
#[derive(Clone, Default)]
pub struct Frame(pub Option<Arc<FrameContent>>);

impl Frame {
    pub fn new(c: FrameContent) -> Self {
        Self(Some(Arc::new(c)))
    }

    pub fn get(&self) -> Option<&FrameContent> {
        self.0.as_deref()
    }
}

impl PartialEq for Frame {
    fn eq(&self, o: &Self) -> bool {
        match (&self.0, &o.0) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }
}

impl Eq for Frame {}

impl std::hash::Hash for Frame {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.0.as_ref().map(Arc::as_ptr).hash(h);
    }
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.get() {
            None => f.write_str("Frame(None)"),
            Some(FrameContent::Group(g)) => write!(f, "Group({} children)", g.children.len()),
            Some(FrameContent::Table(t)) => {
                write!(f, "Table({}×{})", t.rows.len(), t.cols.len())
            }
            Some(FrameContent::Diagram(d)) => write!(f, "Diagram({:?})", d.data),
            Some(FrameContent::Chart(c)) => write!(f, "Chart({:?})", c.part),
        }
    }
}

#[derive(Clone, Debug)]
pub enum FrameContent {
    Group(Group),
    Table(Table),
    Diagram(DiagramRef),
    Chart(ChartRef),
}

/// A group's children (`p:grpSp`), or the shapes of a SmartArt drawing.
#[derive(Clone, Debug, Default)]
pub struct Group {
    /// The child coordinate space (`a:chOff`, `a:chExt`); the group's own
    /// box (`a:off`, `a:ext`) shows it.
    pub child_box: Xfrm,
    pub children: Vec<Shape>,
    /// SmartArt shapes: the text box (`dsp:txXfrm`) of each child, in the
    /// same space (`None` = the shape's own box).
    pub text_boxes: Vec<Option<Xfrm>>,
}

/// A SmartArt frame: the data part (`r:dm`), whose `dsp:dataModelExt`
/// names PowerPoint's cached drawing.
#[derive(Clone, Debug)]
pub struct DiagramRef {
    pub data: Option<String>,
}

/// A chart frame: the chart part (`c:chart r:id`).
#[derive(Clone, Debug)]
pub struct ChartRef {
    pub part: Option<String>,
}

/// Where a child of a group is drawn: `child` (in the group's child
/// space `child_box`) mapped into the group's box `group`, with the group's
/// flips and rotation composed in. Sizes scale with the group; a child
/// turned by about 90° takes the other axis' scale (as PowerPoint).
pub fn compose(group: &Xfrm, child_box: &Xfrm, child: &Xfrm) -> Xfrm {
    let k = |a: Emu, b: Emu| if b == 0 { 1.0 } else { a as f64 / b as f64 };
    let (sx, sy) = (k(group.cx, child_box.cx), k(group.cy, child_box.cy));
    let (ccx, ccy) = child.center();
    let mut px = group.x as f64 + (ccx - child_box.x as f64) * sx;
    let mut py = group.y as f64 + (ccy - child_box.y as f64) * sy;
    let deg = child.rot as f64 / ROT_PER_DEG as f64;
    let sideways = (45.0..135.0).contains(&deg) || (225.0..315.0).contains(&deg);
    let (w, h) = if sideways {
        (child.cx as f64 * sy, child.cy as f64 * sx)
    } else {
        (child.cx as f64 * sx, child.cy as f64 * sy)
    };
    let mut rot = deg;
    let (mut fh, mut fv) = (child.flip_h, child.flip_v);
    let (gx, gy) = group.center();
    // The group flips first, then turns (as a shape does).
    if group.flip_h {
        px = 2.0 * gx - px;
        fh = !fh;
        rot = -rot;
    }
    if group.flip_v {
        py = 2.0 * gy - py;
        fv = !fv;
        rot = -rot;
    }
    if group.rot != 0 {
        let (s, c) = group.angle().sin_cos();
        let (dx, dy) = (px - gx, py - gy);
        px = gx + dx * c - dy * s;
        py = gy + dx * s + dy * c;
        rot += group.rot as f64 / ROT_PER_DEG as f64;
    }
    let out = Xfrm {
        x: (px - w / 2.0).round() as Emu,
        y: (py - h / 2.0).round() as Emu,
        cx: w.round() as Emu,
        cy: h.round() as Emu,
        rot: 0,
        flip_h: fh,
        flip_v: fv,
    };
    out.with_degrees(rot)
}

/// `a:chOff` / `a:chExt` of a group's `a:xfrm` (the box itself if absent).
pub(crate) fn child_box(xfrm: Option<Node>, own: Option<Xfrm>) -> Xfrm {
    let own = own.unwrap_or_default();
    let Some(x) = xfrm else { return own };
    let off = child(x, NS_A, "chOff");
    let ext = child(x, NS_A, "chExt");
    Xfrm::new(
        off.and_then(|o| attr_i64(o, "x")).unwrap_or(own.x),
        off.and_then(|o| attr_i64(o, "y")).unwrap_or(own.y),
        ext.and_then(|e| attr_i64(e, "cx")).unwrap_or(own.cx),
        ext.and_then(|e| attr_i64(e, "cy")).unwrap_or(own.cy),
    )
}

// ── tables ──────────────────────────────────────────────────────────────

/// A cell border (`a:lnL`... or a table style's `a:ln`).
#[derive(Clone, Debug, PartialEq)]
pub struct Border {
    /// `None` = no line (`a:noFill`).
    pub color: Option<(Rgb, u8)>,
    pub width: Emu,
    pub style: LineStyle,
}

/// Cell edges: left, right, top, bottom, then the diagonals (top left →
/// bottom right, bottom left → top right).
pub const EDGE_L: usize = 0;
pub const EDGE_R: usize = 1;
pub const EDGE_T: usize = 2;
pub const EDGE_B: usize = 3;
pub const EDGE_TL_BR: usize = 4;
pub const EDGE_BL_TR: usize = 5;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Vert {
    #[default]
    Horz,
    /// Turned 90° clockwise (`vert`, `eaVert`).
    Down,
    /// Turned 270° (`vert270`): reads upwards.
    Up,
}

impl Vert {
    pub fn parse(v: Option<&str>) -> Option<Self> {
        Some(match v? {
            "vert" | "eaVert" => Self::Down,
            "vert270" => Self::Up,
            _ => Self::Horz,
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct Cell {
    pub text: Option<TextBody>,
    pub grid_span: u32,
    pub row_span: u32,
    /// Covered by the cell to the left / above (merged).
    pub h_merge: bool,
    pub v_merge: bool,
    /// Left, right, top, bottom (EMU).
    pub margins: [Emu; 4],
    pub anchor: Anchor,
    pub vert: Vert,
    /// The cell's own fill (`None` = the table style's).
    pub fill: Option<Fill>,
    /// The cell's own borders (`None` = the table style's), see `EDGE_*`.
    pub borders: [Option<Border>; 6],
}

impl Cell {
    /// Hidden under a merged neighbour.
    pub fn covered(&self) -> bool {
        self.h_merge || self.v_merge
    }
}

#[derive(Clone, Debug, Default)]
pub struct Row {
    /// Minimum height (EMU); rows grow to fit their text.
    pub h: Emu,
    pub cells: Vec<Cell>,
}

/// `a:tblPr` flags: which style parts apply.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TableFlags {
    pub first_row: bool,
    pub last_row: bool,
    pub first_col: bool,
    pub last_col: bool,
    pub band_row: bool,
    pub band_col: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Table {
    /// Column widths (EMU).
    pub cols: Vec<Emu>,
    pub rows: Vec<Row>,
    pub flags: TableFlags,
    /// `a:tableStyleId` (`None` = no style).
    pub style_id: Option<String>,
    /// The table's own background (`a:tblPr` fill).
    pub background: Option<Fill>,
}

/// Parts of a table style, in the order they apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    WholeTbl,
    Band1V,
    Band2V,
    Band1H,
    Band2H,
    LastCol,
    FirstCol,
    LastRow,
    FirstRow,
    SeCell,
    SwCell,
    NeCell,
    NwCell,
}

impl Part {
    const ALL: [Part; 13] = [
        Part::WholeTbl,
        Part::Band1V,
        Part::Band2V,
        Part::Band1H,
        Part::Band2H,
        Part::LastCol,
        Part::FirstCol,
        Part::LastRow,
        Part::FirstRow,
        Part::SeCell,
        Part::SwCell,
        Part::NeCell,
        Part::NwCell,
    ];

    fn name(self) -> &'static str {
        match self {
            Part::WholeTbl => "wholeTbl",
            Part::Band1V => "band1V",
            Part::Band2V => "band2V",
            Part::Band1H => "band1H",
            Part::Band2H => "band2H",
            Part::LastCol => "lastCol",
            Part::FirstCol => "firstCol",
            Part::LastRow => "lastRow",
            Part::FirstRow => "firstRow",
            Part::SeCell => "seCell",
            Part::SwCell => "swCell",
            Part::NeCell => "neCell",
            Part::NwCell => "nwCell",
        }
    }
}

/// Style borders: left, right, top, bottom, insideH, insideV, tl2br, tr2bl.
const S_L: usize = 0;
const S_R: usize = 1;
const S_T: usize = 2;
const S_B: usize = 3;
const S_IH: usize = 4;
const S_IV: usize = 5;
const S_TL: usize = 6;
const S_TR: usize = 7;

/// One part of a table style (`a:wholeTbl`, `a:firstRow`...).
#[derive(Clone, Debug, Default)]
pub struct StylePart {
    /// `a:tcTxStyle`: bold, italic, colour, typeface.
    pub text: LevelStyle,
    pub fill: Option<Fill>,
    pub borders: [Option<Border>; 8],
}

/// A table style (`a:tblStyle`), colours resolved.
#[derive(Clone, Debug, Default)]
pub struct TableStyle {
    pub name: String,
    parts: HashMap<Part, StylePart>,
}

/// The table styles a presentation can use, by id.
#[derive(Clone, Debug, Default)]
pub struct TableStyles {
    pub styles: HashMap<String, TableStyle>,
    /// `def`: the default style.
    pub default: Option<String>,
}

impl TableStyles {
    /// Styles from `ppt/tableStyles.xml` (if any) plus the built-in ones
    /// files usually don't define, colours from `cx`.
    pub fn parse(xml: Option<&str>, cx: &ColorCtx) -> Self {
        let mut out = Self::default();
        for id in BUILTIN {
            let Some(xml) = builtin_style(id) else {
                continue;
            };
            let wrapped = format!("<w xmlns:a=\"{NS_A}\">{xml}</w>");
            if let Ok(doc) = roxmltree::Document::parse(&wrapped)
                && let Some(el) = doc.root_element().first_element_child()
            {
                out.styles.insert(id.to_owned(), TableStyle::parse(el, cx));
            }
        }
        if let Some(doc) = xml.and_then(|x| roxmltree::Document::parse(x).ok()) {
            let root = doc.root_element();
            out.default = root.attribute("def").map(str::to_owned);
            for s in children(root, NS_A, "tblStyle") {
                if let Some(id) = s.attribute("styleId") {
                    out.styles.insert(id.to_owned(), TableStyle::parse(s, cx));
                }
            }
        }
        out
    }

    /// The style `id`: defined in the file, else built in. Unknown ids
    /// fall back to the file's default, then Medium Style 2 – Accent 1
    /// (PowerPoint's default).
    pub fn get(&self, id: &str) -> Option<&TableStyle> {
        self.styles
            .get(id)
            .or_else(|| self.default.as_ref().and_then(|d| self.styles.get(d)))
            .or_else(|| self.styles.get(MEDIUM_2_ACCENT_1))
    }
}

pub const MEDIUM_2_ACCENT_1: &str = "{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}";

/// Built-in style ids (Medium Style 2 and its accents; no style with and
/// without grid).
const BUILTIN: [&str; 9] = [
    "{073A0DAA-6AF3-43AB-8588-CEC1D06C72B9}",
    MEDIUM_2_ACCENT_1,
    "{21E4AEA4-8DFA-4A89-87EB-49C32662AFE8}",
    "{F5AB1C69-6EDB-4FF4-983F-18BD219EF322}",
    "{00A15C55-8517-42AA-B614-E9B94910E393}",
    "{7DF18680-E054-41AD-8BC1-D1AEF772440D}",
    "{93296810-A885-4BE3-A3E7-6D5BEEA58F35}",
    "{2D5ABB26-0587-4C30-8999-92F81FD0307C}",
    "{5940675A-B579-460E-94D1-54222C63F5DA}",
];

/// Built-in styles by id (the ones decks use most), as `a:tblStyle` XML.
fn builtin_style(id: &str) -> Option<String> {
    let medium2 = |accent: &str| {
        let ln = |w: u32| {
            format!(
                "<a:ln w=\"{w}\" cmpd=\"sng\"><a:solidFill><a:schemeClr val=\"lt1\"/></a:solidFill></a:ln>"
            )
        };
        let thin = ln(12_700);
        let txt = |c: &str, b: bool| {
            format!(
                "<a:tcTxStyle{}><a:fontRef idx=\"minor\"><a:prstClr val=\"black\"/></a:fontRef><a:schemeClr val=\"{c}\"/></a:tcTxStyle>",
                if b { " b=\"on\"" } else { "" }
            )
        };
        let fill = |tint: Option<u32>| match tint {
            Some(t) => format!(
                "<a:fill><a:solidFill><a:schemeClr val=\"{accent}\"><a:tint val=\"{t}\"/></a:schemeClr></a:solidFill></a:fill>"
            ),
            None => format!(
                "<a:fill><a:solidFill><a:schemeClr val=\"{accent}\"/></a:solidFill></a:fill>"
            ),
        };
        let edges: String = ["left", "right", "top", "bottom", "insideH", "insideV"]
            .iter()
            .map(|e| format!("<a:{e}>{thin}</a:{e}>"))
            .collect();
        let strong = |edge: &str| {
            format!(
                "{}<a:tcStyle><a:tcBdr><a:{edge}>{}</a:{edge}></a:tcBdr>{}</a:tcStyle>",
                txt("lt1", true),
                ln(38_100),
                fill(None)
            )
        };
        format!(
            "<a:tblStyle styleId=\"{id}\" styleName=\"Medium Style 2\">\
             <a:wholeTbl>{}<a:tcStyle><a:tcBdr>{edges}</a:tcBdr>{}</a:tcStyle></a:wholeTbl>\
             <a:band1H><a:tcStyle><a:tcBdr/>{}</a:tcStyle></a:band1H>\
             <a:band2H><a:tcStyle><a:tcBdr/></a:tcStyle></a:band2H>\
             <a:band1V><a:tcStyle><a:tcBdr/>{}</a:tcStyle></a:band1V>\
             <a:band2V><a:tcStyle><a:tcBdr/></a:tcStyle></a:band2V>\
             <a:lastCol>{}<a:tcStyle><a:tcBdr/>{}</a:tcStyle></a:lastCol>\
             <a:firstCol>{}<a:tcStyle><a:tcBdr/>{}</a:tcStyle></a:firstCol>\
             <a:lastRow>{}</a:lastRow><a:firstRow>{}</a:firstRow></a:tblStyle>",
            txt("dk1", false),
            fill(Some(20_000)),
            fill(Some(40_000)),
            fill(Some(40_000)),
            txt("lt1", true),
            fill(None),
            txt("lt1", true),
            fill(None),
            strong("top"),
            strong("bottom"),
        )
    };
    let accent = match id {
        "{073A0DAA-6AF3-43AB-8588-CEC1D06C72B9}" => "dk1",
        MEDIUM_2_ACCENT_1 => "accent1",
        "{21E4AEA4-8DFA-4A89-87EB-49C32662AFE8}" => "accent2",
        "{F5AB1C69-6EDB-4FF4-983F-18BD219EF322}" => "accent3",
        "{00A15C55-8517-42AA-B614-E9B94910E393}" => "accent4",
        "{7DF18680-E054-41AD-8BC1-D1AEF772440D}" => "accent5",
        "{93296810-A885-4BE3-A3E7-6D5BEEA58F35}" => "accent6",
        // No Style, No Grid.
        "{2D5ABB26-0587-4C30-8999-92F81FD0307C}" => {
            return Some(format!(
                "<a:tblStyle styleId=\"{id}\" styleName=\"No Style, No Grid\"><a:wholeTbl><a:tcTxStyle><a:fontRef idx=\"minor\"><a:scrgbClr r=\"0\" g=\"0\" b=\"0\"/></a:fontRef><a:schemeClr val=\"tx1\"/></a:tcTxStyle><a:tcStyle><a:tcBdr/><a:fill><a:noFill/></a:fill></a:tcStyle></a:wholeTbl></a:tblStyle>"
            ));
        }
        // No Style, Table Grid.
        "{5940675A-B579-460E-94D1-54222C63F5DA}" => {
            let ln = "<a:ln w=\"12700\" cmpd=\"sng\"><a:solidFill><a:schemeClr val=\"tx1\"/></a:solidFill></a:ln>";
            let edges: String = ["left", "right", "top", "bottom", "insideH", "insideV"]
                .iter()
                .map(|e| format!("<a:{e}>{ln}</a:{e}>"))
                .collect();
            return Some(format!(
                "<a:tblStyle styleId=\"{id}\" styleName=\"No Style, Table Grid\"><a:wholeTbl><a:tcTxStyle><a:fontRef idx=\"minor\"><a:scrgbClr r=\"0\" g=\"0\" b=\"0\"/></a:fontRef><a:schemeClr val=\"tx1\"/></a:tcTxStyle><a:tcStyle><a:tcBdr>{edges}</a:tcBdr><a:fill><a:noFill/></a:fill></a:tcStyle></a:wholeTbl></a:tblStyle>"
            ));
        }
        _ => return None,
    };
    Some(medium2(accent))
}

/// A border element's line (`a:ln`, or the `a:lnL`... of a cell).
fn border(ln: Node, cx: &ColorCtx) -> Option<Border> {
    let fill = ln
        .children()
        .find(|c| c.is_element() && matches!(c.tag_name().name(), "noFill" | "solidFill"));
    let color = match fill {
        Some(f) if f.tag_name().name() == "noFill" => None,
        Some(f) => Some(
            f.children()
                .filter(|c| c.is_element())
                .find_map(|c| cx.color_alpha(c))?,
        ),
        // A line without a colour of its own: not set.
        None => return None,
    };
    Some(Border {
        color,
        width: crate::fill::line_width(ln).unwrap_or(12_700),
        style: cx.line_style(ln, &LineStyle::default()),
    })
}

/// The first fill child of `n` (`a:noFill`, `a:solidFill`...).
fn fill_child(n: Node, cx: &ColorCtx) -> Option<Fill> {
    n.children()
        .filter(|c| c.is_element())
        .find_map(|c| cx.full_fill(c, &|_| None))
}

impl TableStyle {
    fn parse(n: Node, cx: &ColorCtx) -> Self {
        let mut parts = HashMap::new();
        for part in Part::ALL {
            let Some(el) = child(n, NS_A, part.name()) else {
                continue;
            };
            let mut sp = StylePart::default();
            if let Some(t) = child(el, NS_A, "tcTxStyle") {
                sp.text.bold = t.attribute("b").map(|v| v == "on");
                sp.text.italic = t.attribute("i").map(|v| v == "on");
                for c in t.children().filter(|c| c.is_element()) {
                    match c.tag_name().name() {
                        "fontRef" => {
                            sp.text.font = match c.attribute("idx") {
                                Some("major") => cx.theme.major_font.clone(),
                                _ => cx.theme.minor_font.clone(),
                            };
                            if sp.text.color.is_none() {
                                sp.text.color = cx.first_color(c);
                            }
                        }
                        "font" => {
                            sp.text.font = child(c, NS_A, "latin")
                                .and_then(|l| l.attribute("typeface"))
                                .and_then(|f| cx.theme.font(f));
                        }
                        _ => {
                            if let Some(col) = cx.color(c) {
                                sp.text.color = Some(col);
                            }
                        }
                    }
                }
            }
            if let Some(st) = child(el, NS_A, "tcStyle") {
                if let Some(f) = child(st, NS_A, "fill") {
                    sp.fill = fill_child(f, cx);
                } else if let Some(r) = child(st, NS_A, "fillRef") {
                    let idx = attr_i64(r, "idx").unwrap_or(0);
                    sp.fill = cx.style_fill(idx, cx.first_color(r));
                }
                if let Some(b) = child(st, NS_A, "tcBdr") {
                    for (i, k) in [
                        "left", "right", "top", "bottom", "insideH", "insideV", "tl2br", "tr2bl",
                    ]
                    .iter()
                    .enumerate()
                    {
                        sp.borders[i] = child(b, NS_A, k)
                            .and_then(|e| child(e, NS_A, "ln"))
                            .and_then(|l| border(l, cx));
                    }
                }
            }
            parts.insert(part, sp);
        }
        Self {
            name: n.attribute("styleName").unwrap_or_default().to_owned(),
            parts,
        }
    }

    pub fn part(&self, p: Part) -> Option<&StylePart> {
        self.parts.get(&p)
    }
}

/// How a cell looks: its own formatting on top of the table style's.
#[derive(Clone, Debug, Default)]
pub struct CellLook {
    pub fill: Option<Fill>,
    pub text: LevelStyle,
    pub borders: [Option<Border>; 6],
}

impl Table {
    /// The anchor cell of every grid position (`(row, col)` of the cell
    /// that shows there).
    pub fn cell(&self, r: usize, c: usize) -> Option<&Cell> {
        self.rows.get(r)?.cells.get(c)
    }

    /// Parts that apply to the cell at `(r, c)` spanning `rs` × `cs`, in
    /// order, each with the region it covers (rows, cols, inclusive).
    fn parts_at(&self, r: usize, c: usize) -> Vec<(Part, [usize; 4])> {
        let (nr, nc) = (self.rows.len(), self.cols.len());
        let (lr, lc) = (nr.saturating_sub(1), nc.saturating_sub(1));
        let f = &self.flags;
        let mut out = vec![(Part::WholeTbl, [0, lr, 0, lc])];
        let header_c = usize::from(f.first_col);
        let header_r = usize::from(f.first_row);
        let in_body_c = !(f.first_col && c == 0) && !(f.last_col && c == lc && nc > 1);
        let in_body_r = !(f.first_row && r == 0) && !(f.last_row && r == lr && nr > 1);
        if f.band_col && in_body_c {
            let p = if (c - header_c.min(c)).is_multiple_of(2) {
                Part::Band1V
            } else {
                Part::Band2V
            };
            out.push((p, [0, lr, c, c]));
        }
        if f.band_row && in_body_r {
            let p = if (r - header_r.min(r)).is_multiple_of(2) {
                Part::Band1H
            } else {
                Part::Band2H
            };
            out.push((p, [r, r, 0, lc]));
        }
        if f.last_col && c == lc {
            out.push((Part::LastCol, [0, lr, lc, lc]));
        }
        if f.first_col && c == 0 {
            out.push((Part::FirstCol, [0, lr, 0, 0]));
        }
        if f.last_row && r == lr {
            out.push((Part::LastRow, [lr, lr, 0, lc]));
        }
        if f.first_row && r == 0 {
            out.push((Part::FirstRow, [0, 0, 0, lc]));
        }
        let corner = [lr, lr, lc, lc];
        if f.last_row && f.last_col && r == lr && c == lc {
            out.push((Part::SeCell, corner));
        }
        if f.last_row && f.first_col && r == lr && c == 0 {
            out.push((Part::SwCell, [lr, lr, 0, 0]));
        }
        if f.first_row && f.last_col && r == 0 && c == lc {
            out.push((Part::NeCell, [0, 0, lc, lc]));
        }
        if f.first_row && f.first_col && r == 0 && c == 0 {
            out.push((Part::NwCell, [0, 0, 0, 0]));
        }
        out
    }

    /// How the cell at `(r, c)` looks with `style` (`None` = no style).
    pub fn cell_look(&self, style: Option<&TableStyle>, r: usize, c: usize) -> CellLook {
        let mut look = CellLook::default();
        let Some(cell) = self.cell(r, c) else {
            return look;
        };
        let r1 = r + cell.row_span.max(1) as usize - 1;
        let c1 = c + cell.grid_span.max(1) as usize - 1;
        if let Some(style) = style {
            for (part, [pr0, pr1, pc0, pc1]) in self.parts_at(r, c) {
                let Some(sp) = style.part(part) else { continue };
                look.text.overlay(&sp.text);
                if sp.fill.is_some() {
                    look.fill = sp.fill.clone();
                }
                let b = &sp.borders;
                let edge = |outer: usize, inner: usize, at_edge: bool| {
                    if at_edge {
                        b[outer].clone()
                    } else {
                        b[inner].clone()
                    }
                };
                let set = [
                    (EDGE_L, edge(S_L, S_IV, c == pc0)),
                    (EDGE_R, edge(S_R, S_IV, c1 >= pc1)),
                    (EDGE_T, edge(S_T, S_IH, r == pr0)),
                    (EDGE_B, edge(S_B, S_IH, r1 >= pr1)),
                    (EDGE_TL_BR, b[S_TL].clone()),
                    (EDGE_BL_TR, b[S_TR].clone()),
                ];
                for (i, v) in set {
                    if v.is_some() {
                        look.borders[i] = v;
                    }
                }
            }
        }
        if cell.fill.is_some() {
            look.fill = cell.fill.clone();
        }
        for (i, own) in cell.borders.iter().enumerate() {
            if own.is_some() {
                look.borders[i] = own.clone();
            }
        }
        look
    }
}

/// `a:tbl` → [`Table`]; text bodies parsed by `text`.
pub(crate) fn parse_table(tbl: Node, cx: &ColorCtx, text: &dyn Fn(Node) -> TextBody) -> Table {
    let mut t = Table::default();
    if let Some(pr) = child(tbl, NS_A, "tblPr") {
        let flag = |k| attr_bool(pr, k).unwrap_or(false);
        t.flags = TableFlags {
            first_row: flag("firstRow"),
            last_row: flag("lastRow"),
            first_col: flag("firstCol"),
            last_col: flag("lastCol"),
            band_row: flag("bandRow"),
            band_col: flag("bandCol"),
        };
        t.style_id = child(pr, NS_A, "tableStyleId")
            .and_then(|s| s.text())
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        t.background = fill_child(pr, cx);
    }
    t.cols = child(tbl, NS_A, "tblGrid")
        .map(|g| {
            children(g, NS_A, "gridCol")
                .map(|c| attr_i64(c, "w").unwrap_or(0).max(0))
                .collect()
        })
        .unwrap_or_default();
    for tr in children(tbl, NS_A, "tr") {
        let mut row = Row {
            h: attr_i64(tr, "h").unwrap_or(0).max(0),
            cells: Vec::new(),
        };
        for tc in children(tr, NS_A, "tc") {
            let mut cell = Cell {
                grid_span: attr_i64(tc, "gridSpan").unwrap_or(1).clamp(1, 1000) as u32,
                row_span: attr_i64(tc, "rowSpan").unwrap_or(1).clamp(1, 1000) as u32,
                h_merge: attr_bool(tc, "hMerge").unwrap_or(false),
                v_merge: attr_bool(tc, "vMerge").unwrap_or(false),
                margins: [91_440, 91_440, 45_720, 45_720],
                text: child(tc, NS_A, "txBody").map(text),
                ..Cell::default()
            };
            if let Some(pr) = child(tc, NS_A, "tcPr") {
                for (i, k) in ["marL", "marR", "marT", "marB"].iter().enumerate() {
                    if let Some(v) = attr_i64(pr, k) {
                        cell.margins[i] = v.max(0);
                    }
                }
                cell.anchor = match pr.attribute("anchor") {
                    Some("ctr") => Anchor::Middle,
                    Some("b") => Anchor::Bottom,
                    _ => Anchor::Top,
                };
                cell.vert = Vert::parse(pr.attribute("vert")).unwrap_or_default();
                for (i, k) in ["lnL", "lnR", "lnT", "lnB", "lnTlToBr", "lnBlToTr"]
                    .iter()
                    .enumerate()
                {
                    cell.borders[i] = child(pr, NS_A, k).and_then(|l| border(l, cx));
                }
                cell.fill = fill_child(pr, cx);
            }
            row.cells.push(cell);
        }
        t.rows.push(row);
    }
    // Rows with fewer cells than the grid: pad (drawn empty).
    let n = t.cols.len();
    for row in &mut t.rows {
        while row.cells.len() < n {
            row.cells.push(Cell {
                margins: [91_440, 91_440, 45_720, 45_720],
                ..Cell::default()
            });
        }
    }
    t
}

/// What a `p:graphicFrame` shows (`None` = something else: OLE, media...).
pub(crate) fn parse_graphic_frame(
    n: Node,
    cx: &ColorCtx,
    rels: &[Rel],
    part: &str,
    text: &dyn Fn(Node) -> TextBody,
) -> Option<FrameContent> {
    let data = n
        .children()
        .find(|c| c.is_element() && c.tag_name().name() == "graphic")
        .and_then(|g| child(g, NS_A, "graphicData"))?;
    let rel_part = |id: Option<&str>| {
        let id = id?;
        rels.iter().find(|r| r.id == id).and_then(|r| r.part(part))
    };
    if let Some(tbl) = child(data, NS_A, "tbl") {
        return Some(FrameContent::Table(parse_table(tbl, cx, text)));
    }
    if data.attribute("uri") == Some(URI_TABLE) {
        return None;
    }
    if let Some(ids) = child(data, NS_DGM, "relIds") {
        return Some(FrameContent::Diagram(DiagramRef {
            data: rel_part(r_attr(ids, "dm")),
        }));
    }
    if let Some(c) = child(data, NS_C, "chart") {
        return Some(FrameContent::Chart(ChartRef {
            part: rel_part(r_attr(c, "id")),
        }));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Xfrm, b: Xfrm) {
        let d = |x: Emu, y: Emu| (x - y).abs() <= 1;
        assert!(
            d(a.x, b.x) && d(a.y, b.y) && d(a.cx, b.cx) && d(a.cy, b.cy),
            "{a:?} != {b:?}"
        );
        assert_eq!((a.rot, a.flip_h, a.flip_v), (b.rot, b.flip_h, b.flip_v));
    }

    #[test]
    fn group_children_scale_into_the_group_box() {
        // Child space 0..100 shown at 1000..1200 × 500..700: twice the size.
        let g = Xfrm::new(1000, 500, 200, 200);
        let ch = Xfrm::new(0, 0, 100, 100);
        close(
            compose(&g, &ch, &Xfrm::new(50, 0, 50, 25)),
            Xfrm::new(1100, 500, 100, 50),
        );
        // A child turned 90° takes the other scale on each side.
        let g2 = Xfrm::new(0, 0, 200, 100);
        let c = Xfrm::new(0, 0, 40, 20).with_degrees(90.0);
        let out = compose(&g2, &ch, &c);
        assert_eq!((out.cx, out.cy), (40, 40));
    }

    #[test]
    fn group_flips_and_turns_compose_into_children() {
        let ch = Xfrm::new(0, 0, 100, 100);
        // Flipped horizontally: a child on the left moves right, flips,
        // and its rotation mirrors.
        let g = Xfrm {
            flip_h: true,
            ..Xfrm::new(0, 0, 100, 100)
        };
        let c = Xfrm::new(0, 0, 20, 10).with_degrees(30.0);
        let out = compose(&g, &ch, &c);
        close(
            out,
            Xfrm {
                flip_h: true,
                ..Xfrm::new(80, 0, 20, 10).with_degrees(-30.0)
            },
        );
        // Turned 90°: the top-left child goes to the top right, turned too.
        let g = Xfrm::new(0, 0, 100, 100).with_degrees(90.0);
        let out = compose(&g, &ch, &Xfrm::new(0, 0, 20, 20));
        close(out, Xfrm::new(80, 0, 20, 20).with_degrees(90.0));
        // Nested: an inner group (child space 0..10) in the turned group.
        let inner = compose(&g, &ch, &Xfrm::new(0, 0, 50, 50));
        let leaf = compose(&inner, &Xfrm::new(0, 0, 10, 10), &Xfrm::new(0, 0, 2, 2));
        close(leaf, Xfrm::new(90, 0, 10, 10).with_degrees(90.0));
    }

    #[test]
    fn table_style_parts_resolve_per_cell() {
        use crate::theme::{ClrMap, Theme};
        let theme = Theme::parse(&format!(
            r#"<a:theme xmlns:a="{NS_A}"><a:themeElements><a:clrScheme name="t"><a:dk1><a:srgbClr val="000000"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:accent1><a:srgbClr val="4472C4"/></a:accent1></a:clrScheme></a:themeElements></a:theme>"#
        ));
        let map = ClrMap::default();
        let cx = ColorCtx {
            theme: &theme,
            map: &map,
            ph: None,
        };
        let styles = TableStyles::parse(None, &cx);
        let style = styles.get(MEDIUM_2_ACCENT_1).unwrap();
        // Unknown ids fall back to the default style.
        assert_eq!(styles.get("{nope}").unwrap().name, style.name);
        let tc = |own: &str| format!("<a:tc><a:tcPr>{own}</a:tcPr></a:tc>");
        let red = r#"<a:lnB w="25400"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:lnB><a:solidFill><a:srgbClr val="00FF00"/></a:solidFill>"#;
        let rows: String = (0..4)
            .map(|r| {
                let cells: String = (0..3)
                    .map(|c| tc(if (r, c) == (2, 2) { red } else { "" }))
                    .collect();
                format!("<a:tr h=\"100\">{cells}</a:tr>")
            })
            .collect();
        let xml = format!(
            r#"<a:tbl xmlns:a="{NS_A}"><a:tblPr firstRow="1" firstCol="1" lastRow="1" bandRow="1"/><a:tblGrid><a:gridCol w="10"/><a:gridCol w="10"/><a:gridCol w="10"/></a:tblGrid>{rows}</a:tbl>"#
        );
        let doc = roxmltree::Document::parse(&xml).unwrap();
        let t = parse_table(doc.root_element(), &cx, &|_| TextBody::new(Vec::new()));
        let fill = |r, c| match t.cell_look(Some(style), r, c).fill {
            Some(Fill::Solid(c, _)) => c.0,
            f => panic!("{f:?}"),
        };
        let accent = [0x44, 0x72, 0xC4];
        // Header row and first column: accent, bold white text.
        assert_eq!(fill(0, 1), accent);
        assert_eq!(fill(1, 0), accent);
        let head = t.cell_look(Some(style), 0, 1);
        assert_eq!(head.text.bold, Some(true));
        assert_eq!(head.text.color, Some(Rgb([255, 255, 255])));
        // Body rows band from the first one after the header.
        assert_eq!(fill(1, 1), [180, 198, 231]);
        assert_eq!(fill(2, 1), [217, 226, 243]);
        assert_eq!(fill(3, 1), accent, "last row");
        // Borders: thin white inside, the header's thick bottom edge.
        let b = |r, c, e: usize| t.cell_look(Some(style), r, c).borders[e].clone().unwrap();
        assert_eq!(b(1, 1, EDGE_L).width, 12_700);
        assert_eq!(b(0, 1, EDGE_B).width, 38_100);
        assert_eq!(b(3, 1, EDGE_T).width, 38_100);
        // The cell's own fill and border win.
        assert_eq!(fill(2, 2), [0, 255, 0]);
        assert_eq!(b(2, 2, EDGE_B).color, Some((Rgb([255, 0, 0]), 255)));
        // Without a style: nothing but the cell's own formatting.
        assert!(t.cell_look(None, 1, 1).fill.is_none());
    }
}
