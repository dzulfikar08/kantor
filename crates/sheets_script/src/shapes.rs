//! Shapes and pictures: `Worksheet.Shapes`, `Shape`, `ShapeRange`,
//! `TextFrame` / `TextFrame2`, `FillFormat`, `LineFormat` and the legacy
//! `Worksheet.Pictures`.
//!
//! A `Shape` is its sheet's `sheetId` and the engine's key of the drawing
//! object, looked up again on each use. Every change goes through the
//! engine's drawing edits (the ones Insert ▸ Shapes, moving and resizing
//! with the mouse, Format Shape and editing a shape's text use), so a run
//! undoes in one step and saves as those do.
//!
//! Positions are in points from the sheet's top left, as in Excel: cells
//! are as wide and tall as `Range.Left` / `Range.Top` count them (column
//! widths in Excel pixels × 0.75, row heights in points), and anchors are
//! placed on that grid ([`Geo`]).

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use kantor_script::{Arg, Dispatch, ScriptError, Value};
use sheets_engine::drawing::{Anchor, Drawing, DrawingKey, DrawingKind, EditAs, SheetGeometry};
use sheets_engine::script_shapes::{ShapeLook, shape_xml_info};
use sheets_engine::shape_format::{plain_text, set_fill, set_outline, takes_text};
use sheets_engine::{
    CellRange, MAX_COLS, MAX_ROWS, NEW_CHART_BASE, OutlineEdit, RowSpec, ShapeFill, Workbook,
};

use crate::range::RangeObj;
use crate::session::Session;
use crate::sheet::SheetObj;
use crate::values::*;

pub const MSO_TRUE: i64 = -1;
pub const MSO_FALSE: i64 = 0;

pub const MSO_AUTO_SHAPE: i64 = 1;
pub const MSO_CHART: i64 = 3;
pub const MSO_FREEFORM: i64 = 5;
pub const MSO_GROUP: i64 = 6;
pub const MSO_EMBEDDED_OLE_OBJECT: i64 = 7;
pub const MSO_FORM_CONTROL: i64 = 8;
pub const MSO_LINE: i64 = 9;
pub const MSO_PICTURE: i64 = 13;
pub const MSO_TEXT_BOX: i64 = 17;
pub const MSO_SLICER: i64 = 25;

pub const XL_MOVE_AND_SIZE: i64 = 1;
pub const XL_MOVE: i64 = 2;
pub const XL_FREE_FLOATING: i64 = 3;

/// `msoShapeNotPrimitive`: an `AutoShapeType` Kantor has no number for.
pub const MSO_SHAPE_NOT_PRIMITIVE: i64 = 138;
/// `msoShapeMixed`: `AutoShapeType` of what isn't an AutoShape.
pub const MSO_SHAPE_MIXED: i64 = -2;

/// Constants of this module other than [`AUTO_SHAPES`].
pub const CONSTANTS: &[(&str, i64)] = &[
    ("msoTrue", MSO_TRUE),
    ("msoFalse", MSO_FALSE),
    ("msoCTrue", 1),
    ("msoAutoShape", MSO_AUTO_SHAPE),
    ("msoCallout", 2),
    ("msoChart", MSO_CHART),
    ("msoComment", 4),
    ("msoFreeform", MSO_FREEFORM),
    ("msoGroup", MSO_GROUP),
    ("msoEmbeddedOLEObject", MSO_EMBEDDED_OLE_OBJECT),
    ("msoFormControl", MSO_FORM_CONTROL),
    ("msoLine", MSO_LINE),
    ("msoLinkedPicture", 11),
    ("msoOLEControlObject", 12),
    ("msoPicture", MSO_PICTURE),
    ("msoTextBox", MSO_TEXT_BOX),
    ("msoSlicer", MSO_SLICER),
    ("xlMoveAndSize", XL_MOVE_AND_SIZE),
    ("xlMove", XL_MOVE),
    ("xlFreeFloating", XL_FREE_FLOATING),
    ("msoTextOrientationHorizontal", 1),
    ("msoTextOrientationUpward", 2),
    ("msoTextOrientationDownward", 3),
    ("msoTextOrientationVerticalFarEast", 4),
    ("msoTextOrientationVertical", 5),
    ("msoTextOrientationHorizontalRotatedFarEast", 6),
    ("msoBringToFront", 0),
    ("msoSendToBack", 1),
    ("msoBringForward", 2),
    ("msoSendBackward", 3),
    ("msoShapeMixed", MSO_SHAPE_MIXED),
    ("msoShapeNotPrimitive", MSO_SHAPE_NOT_PRIMITIVE),
];

/// `MsoAutoShapeType`: VBA name, value, DrawingML preset geometry.
pub const AUTO_SHAPES: &[(&str, i64, &str)] = &[
    ("msoShapeRectangle", 1, "rect"),
    ("msoShapeParallelogram", 2, "parallelogram"),
    ("msoShapeTrapezoid", 3, "trapezoid"),
    ("msoShapeDiamond", 4, "diamond"),
    ("msoShapeRoundedRectangle", 5, "roundRect"),
    ("msoShapeOctagon", 6, "octagon"),
    ("msoShapeIsoscelesTriangle", 7, "triangle"),
    ("msoShapeRightTriangle", 8, "rtTriangle"),
    ("msoShapeOval", 9, "ellipse"),
    ("msoShapeHexagon", 10, "hexagon"),
    ("msoShapeCross", 11, "plus"),
    ("msoShapeRegularPentagon", 12, "pentagon"),
    ("msoShapeCan", 13, "can"),
    ("msoShapeCube", 14, "cube"),
    ("msoShapeBevel", 15, "bevel"),
    ("msoShapeFoldedCorner", 16, "foldedCorner"),
    ("msoShapeSmileyFace", 17, "smileyFace"),
    ("msoShapeDonut", 18, "donut"),
    ("msoShapeNoSymbol", 19, "noSmoking"),
    ("msoShapeBlockArc", 20, "blockArc"),
    ("msoShapeHeart", 21, "heart"),
    ("msoShapeLightningBolt", 22, "lightningBolt"),
    ("msoShapeSun", 23, "sun"),
    ("msoShapeMoon", 24, "moon"),
    ("msoShapeArc", 25, "arc"),
    ("msoShapeDoubleBracket", 26, "bracketPair"),
    ("msoShapeDoubleBrace", 27, "bracePair"),
    ("msoShapePlaque", 28, "plaque"),
    ("msoShapeLeftBracket", 29, "leftBracket"),
    ("msoShapeRightBracket", 30, "rightBracket"),
    ("msoShapeLeftBrace", 31, "leftBrace"),
    ("msoShapeRightBrace", 32, "rightBrace"),
    ("msoShapeRightArrow", 33, "rightArrow"),
    ("msoShapeLeftArrow", 34, "leftArrow"),
    ("msoShapeUpArrow", 35, "upArrow"),
    ("msoShapeDownArrow", 36, "downArrow"),
    ("msoShapeLeftRightArrow", 37, "leftRightArrow"),
    ("msoShapeUpDownArrow", 38, "upDownArrow"),
    ("msoShapeQuadArrow", 39, "quadArrow"),
    ("msoShapeLeftRightUpArrow", 40, "leftRightUpArrow"),
    ("msoShapeBentArrow", 41, "bentArrow"),
    ("msoShapeUTurnArrow", 42, "uturnArrow"),
    ("msoShapeLeftUpArrow", 43, "leftUpArrow"),
    ("msoShapeBentUpArrow", 44, "bentUpArrow"),
    ("msoShapeCurvedRightArrow", 45, "curvedRightArrow"),
    ("msoShapeCurvedLeftArrow", 46, "curvedLeftArrow"),
    ("msoShapeCurvedUpArrow", 47, "curvedUpArrow"),
    ("msoShapeCurvedDownArrow", 48, "curvedDownArrow"),
    ("msoShapeStripedRightArrow", 49, "stripedRightArrow"),
    ("msoShapeNotchedRightArrow", 50, "notchedRightArrow"),
    ("msoShapePentagon", 51, "homePlate"),
    ("msoShapeChevron", 52, "chevron"),
    ("msoShapeRightArrowCallout", 53, "rightArrowCallout"),
    ("msoShapeLeftArrowCallout", 54, "leftArrowCallout"),
    ("msoShapeUpArrowCallout", 55, "upArrowCallout"),
    ("msoShapeDownArrowCallout", 56, "downArrowCallout"),
    ("msoShapeLeftRightArrowCallout", 57, "leftRightArrowCallout"),
    ("msoShapeUpDownArrowCallout", 58, "upDownArrowCallout"),
    ("msoShapeQuadArrowCallout", 59, "quadArrowCallout"),
    ("msoShapeCircularArrow", 60, "circularArrow"),
    ("msoShapeFlowchartProcess", 61, "flowChartProcess"),
    (
        "msoShapeFlowchartAlternateProcess",
        62,
        "flowChartAlternateProcess",
    ),
    ("msoShapeFlowchartDecision", 63, "flowChartDecision"),
    ("msoShapeFlowchartData", 64, "flowChartInputOutput"),
    (
        "msoShapeFlowchartPredefinedProcess",
        65,
        "flowChartPredefinedProcess",
    ),
    (
        "msoShapeFlowchartInternalStorage",
        66,
        "flowChartInternalStorage",
    ),
    ("msoShapeFlowchartDocument", 67, "flowChartDocument"),
    (
        "msoShapeFlowchartMultidocument",
        68,
        "flowChartMultidocument",
    ),
    ("msoShapeFlowchartTerminator", 69, "flowChartTerminator"),
    ("msoShapeFlowchartPreparation", 70, "flowChartPreparation"),
    ("msoShapeFlowchartManualInput", 71, "flowChartManualInput"),
    (
        "msoShapeFlowchartManualOperation",
        72,
        "flowChartManualOperation",
    ),
    ("msoShapeFlowchartConnector", 73, "flowChartConnector"),
    (
        "msoShapeFlowchartOffpageConnector",
        74,
        "flowChartOffpageConnector",
    ),
    ("msoShapeFlowchartCard", 75, "flowChartPunchedCard"),
    ("msoShapeFlowchartPunchedTape", 76, "flowChartPunchedTape"),
    (
        "msoShapeFlowchartSummingJunction",
        77,
        "flowChartSummingJunction",
    ),
    ("msoShapeFlowchartOr", 78, "flowChartOr"),
    ("msoShapeFlowchartCollate", 79, "flowChartCollate"),
    ("msoShapeFlowchartSort", 80, "flowChartSort"),
    ("msoShapeFlowchartExtract", 81, "flowChartExtract"),
    ("msoShapeFlowchartMerge", 82, "flowChartMerge"),
    ("msoShapeFlowchartStoredData", 83, "flowChartOnlineStorage"),
    ("msoShapeFlowchartDelay", 84, "flowChartDelay"),
    (
        "msoShapeFlowchartSequentialAccessStorage",
        85,
        "flowChartMagneticTape",
    ),
    ("msoShapeFlowchartMagneticDisk", 86, "flowChartMagneticDisk"),
    (
        "msoShapeFlowchartDirectAccessStorage",
        87,
        "flowChartMagneticDrum",
    ),
    ("msoShapeFlowchartDisplay", 88, "flowChartDisplay"),
    ("msoShapeExplosion1", 89, "irregularSeal1"),
    ("msoShapeExplosion2", 90, "irregularSeal2"),
    ("msoShape4pointStar", 91, "star4"),
    ("msoShape5pointStar", 92, "star5"),
    ("msoShape8pointStar", 93, "star8"),
    ("msoShape16pointStar", 94, "star16"),
    ("msoShape24pointStar", 95, "star24"),
    ("msoShape32pointStar", 96, "star32"),
    ("msoShapeUpRibbon", 97, "ribbon2"),
    ("msoShapeDownRibbon", 98, "ribbon"),
    ("msoShapeCurvedUpRibbon", 99, "ellipseRibbon2"),
    ("msoShapeCurvedDownRibbon", 100, "ellipseRibbon"),
    ("msoShapeVerticalScroll", 101, "verticalScroll"),
    ("msoShapeHorizontalScroll", 102, "horizontalScroll"),
    ("msoShapeWave", 103, "wave"),
    ("msoShapeDoubleWave", 104, "doubleWave"),
    ("msoShapeRectangularCallout", 105, "wedgeRectCallout"),
    (
        "msoShapeRoundedRectangularCallout",
        106,
        "wedgeRoundRectCallout",
    ),
    ("msoShapeOvalCallout", 107, "wedgeEllipseCallout"),
    ("msoShapeCloudCallout", 108, "cloudCallout"),
];

/// Every constant of this module, for registering.
pub fn constants() -> impl Iterator<Item = (&'static str, i64)> {
    CONSTANTS
        .iter()
        .copied()
        .chain(AUTO_SHAPES.iter().map(|(n, v, _)| (*n, *v)))
}

/// The name Excel gives a new AutoShape of type `prst` (before its
/// number): `Rectangle`, `Oval`, `Isosceles Triangle`...
fn base_name(vba: &str, prst: &str) -> String {
    let fixed = match prst {
        "rect" => "Rectangle",
        "roundRect" => "Rectangle: Rounded Corners",
        "ellipse" => "Oval",
        "rightArrow" => "Arrow: Right",
        "leftArrow" => "Arrow: Left",
        "upArrow" => "Arrow: Up",
        "downArrow" => "Arrow: Down",
        "wedgeRectCallout" => "Speech Bubble: Rectangle",
        _ => "",
    };
    if !fixed.is_empty() {
        return fixed.to_owned();
    }
    // `msoShapeIsoscelesTriangle` → `Isosceles Triangle`.
    let mut out = String::new();
    for c in vba.trim_start_matches("msoShape").chars() {
        if c.is_ascii_uppercase() && !out.is_empty() && !out.ends_with(' ') {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// Error for a shape that is gone (deleted, or its sheet was).
fn gone() -> ScriptError {
    ScriptError::new(424, "Object required (the shape was deleted)")
}

fn tri(b: bool) -> Value {
    Value::Long(if b { MSO_TRUE as i32 } else { MSO_FALSE as i32 })
}

/// A sheet's rows and columns in points, for placing anchors (see the
/// module). Column edges are added up as far as they are asked for.
struct Geo<'a> {
    book: &'a Workbook,
    sheet: u32,
    /// Left edges of columns `0..len` (one past the last asked for).
    cols: RefCell<Vec<f64>>,
    rows: RowSpec,
}

impl<'a> Geo<'a> {
    fn new(book: &'a Workbook, sheet: u32) -> Geo<'a> {
        Geo {
            book,
            sheet,
            cols: RefCell::new(vec![0.0]),
            rows: book.row_spec(sheet),
        }
    }

    /// Left edges up to column `n` (at most one past the last column).
    fn ensure_cols(&self, n: usize) {
        let mut v = self.cols.borrow_mut();
        while v.len() <= n && v.len() <= MAX_COLS as usize {
            let i = v.len() - 1;
            let next = v[i] + self.book.column_width(self.sheet, i as u32) * 0.75;
            v.push(next);
        }
    }

    fn row_points(&self, row: u32) -> f64 {
        match self.rows.rows.binary_search_by_key(&row, |e| e.row) {
            Ok(i) if self.rows.rows[i].hidden => 0.0,
            Ok(i) => self.rows.rows[i].points,
            Err(_) => self.rows.default_points,
        }
    }
}

impl SheetGeometry for Geo<'_> {
    fn col_left(&self, col: u32) -> f64 {
        let c = col.min(MAX_COLS) as usize;
        self.ensure_cols(c);
        self.cols.borrow()[c]
    }

    fn col_width(&self, col: u32) -> f64 {
        self.book.column_width(self.sheet, col) * 0.75
    }

    fn row_top(&self, row: u32) -> f64 {
        let d = self.rows.default_points;
        let mut top = f64::from(row) * d;
        for e in &self.rows.rows {
            if e.row >= row {
                break;
            }
            top += if e.hidden { 0.0 } else { e.points } - d;
        }
        top
    }

    fn row_height(&self, row: u32) -> f64 {
        self.row_points(row)
    }

    fn col_at(&self, x: f64) -> u32 {
        loop {
            let (len, last) = {
                let v = self.cols.borrow();
                (v.len(), v.last().copied().unwrap_or(0.0))
            };
            if last > x || len > MAX_COLS as usize {
                break;
            }
            self.ensure_cols(len);
        }
        let v = self.cols.borrow();
        let n = v.partition_point(|l| *l <= x);
        (n.saturating_sub(1) as u32).min(MAX_COLS - 1)
    }

    fn row_at(&self, y: f64) -> u32 {
        let d = self.rows.default_points;
        let (mut top, mut next) = (0.0, 0u32);
        let in_gap = |top: f64, from: u32| {
            if d <= 0.0 {
                from
            } else {
                from + ((y - top) / d).floor().max(0.0) as u32
            }
        };
        for e in &self.rows.rows {
            let gap = f64::from(e.row - next) * d;
            if y < top + gap {
                return in_gap(top, next).min(MAX_ROWS - 1);
            }
            top += gap;
            let h = if e.hidden { 0.0 } else { e.points };
            if y < top + h {
                return e.row;
            }
            top += h;
            next = e.row + 1;
        }
        in_gap(top, next).min(MAX_ROWS - 1)
    }

    fn scale(&self) -> (f64, f64) {
        // Points per Excel pixel.
        (0.75, 0.75)
    }
}

/// A new two-cell anchor ("Move and size with cells") over `rect`
/// (points).
fn anchor_at(book: &Workbook, sheet: u32, rect: [f64; 4], edit_as: EditAs) -> Anchor {
    let g = Geo::new(book, sheet);
    Anchor::TwoCell {
        from: Default::default(),
        to: Default::default(),
        edit_as,
    }
    .with_rect(rect, &g)
}

/// The object with `key` on sheet `idx` as it is now.
fn find(book: &Workbook, idx: u32, key: DrawingKey) -> Option<Drawing> {
    book.drawings(idx).into_iter().find(|d| d.key == key)
}

/// An object's name: a shape's as its element has it now (renamed from a
/// macro), else the one it was opened or made with.
fn name_of(d: &Drawing) -> String {
    if let DrawingKind::Shape { xml, .. } = &*d.kind
        && let Some(n) = shape_xml_info(xml).and_then(|i| i.name)
    {
        return n;
    }
    d.name.clone()
}

/// `MsoShapeType` of an object.
fn mso_type(d: &Drawing) -> i64 {
    match &*d.kind {
        DrawingKind::Chart { .. } => MSO_CHART,
        DrawingKind::Picture { .. } => MSO_PICTURE,
        DrawingKind::Slicer { .. } | DrawingKind::Timeline { .. } => MSO_SLICER,
        DrawingKind::Other(_) => MSO_EMBEDDED_OLE_OBJECT,
        DrawingKind::Shape { xml, .. } => match shape_xml_info(xml) {
            Some(i) if i.form_control => MSO_FORM_CONTROL,
            Some(i) if i.element == "cxnSp" || i.prst.as_deref() == Some("line") => MSO_LINE,
            Some(i) if i.element == "grpSp" => MSO_GROUP,
            Some(i) if i.text_box => MSO_TEXT_BOX,
            Some(i) if i.cust_geom => MSO_FREEFORM,
            _ => MSO_AUTO_SHAPE,
        },
    }
}

/// The objects of sheet `sheet` (a `sheetId`), bottom to top; only
/// pictures for `Pictures`.
fn objects(s: &Rc<Session>, sheet: u32, pictures: bool) -> Result<Vec<Drawing>, ScriptError> {
    let idx = s.sheet_index(sheet)?;
    let all = s.read()?.drawings(idx);
    Ok(all
        .into_iter()
        .filter(|d| !pictures || matches!(&*d.kind, DrawingKind::Picture { .. }))
        .collect())
}

/// The object `index` names among `list`: a 1-based number or a name.
fn pick(list: &[Drawing], index: &Value) -> Result<DrawingKey, ScriptError> {
    match scalar(index)? {
        Value::String(name) => list
            .iter()
            .find(|d| name_of(d).eq_ignore_ascii_case(&name))
            .map(|d| d.key)
            .ok_or_else(|| ScriptError::runtime(9)),
        v => {
            let i = to_i64(&v)?;
            if i < 1 || i as usize > list.len() {
                return Err(ScriptError::runtime(9));
            }
            Ok(list[i as usize - 1].key)
        }
    }
}

/// `Worksheet.Shapes` (`Shapes(index)` with an argument).
pub fn shapes(s: &Rc<Session>, sheet: u32, args: &[Arg]) -> Result<Value, ScriptError> {
    let c = Rc::new(ShapesObj {
        s: s.clone(),
        sheet,
    });
    if args.is_empty() {
        Ok(obj(c))
    } else {
        c.get("Item", args)
    }
}

/// `Worksheet.Pictures` (`Pictures(index)` with an argument).
pub fn pictures(s: &Rc<Session>, sheet: u32, args: &[Arg]) -> Result<Value, ScriptError> {
    let c = Rc::new(PicturesObj {
        s: s.clone(),
        sheet,
    });
    if args.is_empty() {
        Ok(obj(c))
    } else {
        c.get("Item", args)
    }
}

/// A picture file for `AddPicture` / `Pictures.Insert`.
struct PictureFile {
    data: Vec<u8>,
    /// The media part's extension (`png`).
    ext: String,
    /// Its own size in points (`None`: unknown).
    size: Option<(f64, f64)>,
}

fn read_picture(s: &Rc<Session>, file: &str) -> Result<PictureFile, ScriptError> {
    let path = crate::paths::checked(s, file, false)?;
    let data = std::fs::read(&path).map_err(|e| {
        err1004(format!(
            "Unable to get the Insert property of the Pictures class ({}: {e})",
            path.display()
        ))
    })?;
    let ext = match sheets_engine::web::picture_format(&data) {
        Some(f) => f.to_owned(),
        None => {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default()
                .to_lowercase();
            if !matches!(ext.as_str(), "emf" | "wmf" | "svg") {
                return Err(err1004(format!(
                    "Kantor can't insert this picture ({}): use a PNG, JPEG, GIF or BMP file",
                    path.display()
                )));
            }
            ext
        }
    };
    let size = sheets_engine::web::picture_size(&data)
        .map(|(w, h)| (f64::from(w) * 0.75, f64::from(h) * 0.75));
    Ok(PictureFile { data, ext, size })
}

/// Adds picture `file` to sheet `sheet` with its top left at `at`
/// (points), `size` points wide and tall (`None` or -1: the picture's own
/// size).
fn add_picture(
    s: &Rc<Session>,
    sheet: u32,
    file: &str,
    at: (f64, f64),
    size: (Option<f64>, Option<f64>),
    legacy: bool,
) -> Result<Value, ScriptError> {
    let pic = read_picture(s, file)?;
    let (own_w, own_h) = pic.size.unwrap_or((96.0, 96.0));
    let w = size.0.filter(|w| *w >= 0.0).unwrap_or(own_w);
    let h = size.1.filter(|h| *h >= 0.0).unwrap_or(own_h);
    let idx = s.sheet_index(sheet)?;
    let mut book = s.write()?;
    let anchor = anchor_at(&book, idx, [at.0, at.1, w, h], EditAs::TwoCell);
    let key = book
        .script_add_picture(idx, pic.data, &pic.ext, anchor)
        .map_err(engine)?;
    drop(book);
    Ok(obj(ShapeObj::new(s, sheet, key, legacy)))
}

/// `Shapes`: the objects drawn over a worksheet.
pub struct ShapesObj {
    pub s: Rc<Session>,
    pub sheet: u32,
}

impl ShapesObj {
    fn shape(&self, key: DrawingKey) -> Value {
        obj(ShapeObj::new(&self.s, self.sheet, key, false))
    }

    /// Adds a shape of geometry `prst` over `rect`.
    fn add(
        &self,
        prst: &str,
        base: &str,
        text_box: bool,
        rect: [f64; 4],
    ) -> Result<Value, ScriptError> {
        let idx = self.s.sheet_index(self.sheet)?;
        let mut book = self.s.write()?;
        let anchor = anchor_at(&book, idx, rect, EditAs::TwoCell);
        let key = book
            .script_add_shape(idx, prst, base, text_box, anchor)
            .map_err(engine)?;
        drop(book);
        Ok(self.shape(key))
    }

    /// `Range(Index)`: shapes by name or 1-based number, one or an array.
    fn range(&self, args: &[Arg]) -> Result<Value, ScriptError> {
        let list = objects(&self.s, self.sheet, false)?;
        let index = need(args, 0, "Index")?;
        let keys = match index {
            Value::Array(a) => a
                .values()
                .iter()
                .map(|v| pick(&list, v))
                .collect::<Result<Vec<_>, _>>()?,
            v => vec![pick(&list, v)?],
        };
        Ok(obj(Rc::new(ShapeRangeObj {
            s: self.s.clone(),
            sheet: self.sheet,
            keys,
        })))
    }
}

/// Four required numbers (points) from `args` starting at `first`.
fn rect_args(args: &[Arg], first: usize, names: [&str; 4]) -> Result<[f64; 4], ScriptError> {
    let mut out = [0.0; 4];
    for (i, n) in names.iter().enumerate() {
        out[i] = to_f64(need(args, first + i, n)?)?;
    }
    Ok(out)
}

impl Dispatch for ShapesObj {
    fn type_name(&self) -> String {
        "Shapes".into()
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "count" => Ok(Value::Long(
                objects(&self.s, self.sheet, false)?.len() as i32
            )),
            "item" | "" => {
                let list = objects(&self.s, self.sheet, false)?;
                let key = pick(&list, need(args, 0, "Index")?)?;
                Ok(self.shape(key))
            }
            "range" => self.range(args),
            "parent" => Ok(obj(SheetObj::new(&self.s, self.sheet))),
            "selectall" => Ok(Value::Empty),
            "addshape" => {
                let t = to_i64(need(args, 0, "Type")?)?;
                let (vba, prst) = AUTO_SHAPES
                    .iter()
                    .find(|(_, v, _)| *v == t)
                    .map(|(n, _, p)| (*n, *p))
                    .ok_or_else(|| {
                        ScriptError::unsupported(&format!("Shapes.AddShape of AutoShape type {t}"))
                    })?;
                let rect = rect_args(args, 1, ["Left", "Top", "Width", "Height"])?;
                self.add(prst, &base_name(vba, prst), false, rect)
            }
            "addtextbox" => {
                let orientation = arg_i64(args, 0, "Orientation")?.unwrap_or(1);
                if orientation != 1 {
                    return Err(ScriptError::unsupported(
                        "Shapes.AddTextbox with vertical text",
                    ));
                }
                let rect = rect_args(args, 1, ["Left", "Top", "Width", "Height"])?;
                self.add("rect", "TextBox", true, rect)
            }
            "addline" => {
                let [x0, y0, x1, y1] = rect_args(args, 0, ["BeginX", "BeginY", "EndX", "EndY"])?;
                let rect = [x0.min(x1), y0.min(y1), (x1 - x0).abs(), (y1 - y0).abs()];
                let idx = self.s.sheet_index(self.sheet)?;
                let mut book = self.s.write()?;
                let anchor = anchor_at(&book, idx, rect, EditAs::TwoCell);
                let key = book
                    .script_add_line(idx, anchor, x1 < x0, y1 < y0)
                    .map_err(engine)?;
                drop(book);
                Ok(self.shape(key))
            }
            "addpicture" => {
                let file = to_str(need(args, 0, "Filename")?)?;
                let num = |i: usize, n: &str| arg(args, i, n).map(to_f64).transpose();
                let at = (
                    num(3, "Left")?.unwrap_or(0.0),
                    num(4, "Top")?.unwrap_or(0.0),
                );
                let size = (num(5, "Width")?, num(6, "Height")?);
                add_picture(&self.s, self.sheet, &file, at, size, false)
            }
            "addchart2" => crate::charts::add_chart2(&self.s, self.sheet, args),
            "addchart" => {
                // AddChart(XlChartType, Left, Top, Width, Height) is
                // AddChart2 without its Style argument first.
                let mut shifted = Vec::with_capacity(args.len() + 1);
                shifted.push(Arg::new(Value::Missing));
                shifted.extend(args.iter().cloned());
                crate::charts::add_chart2(&self.s, self.sheet, &shifted)
            }
            "addform" | "addformcontrol" | "addoleobject" | "addconnector" | "addcurve"
            | "addpolyline" | "buildfreeform" | "addsmartart" | "addlabel" | "group" => Err(
                ScriptError::unsupported(&format!("Shapes.{name} from a macro")),
            ),
            _ => Err(ScriptError::no_member("Shapes", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        Ok(objects(&self.s, self.sheet, false)?
            .into_iter()
            .map(|d| self.shape(d.key))
            .collect())
    }
}

/// `ShapeRange`: some of a sheet's shapes (`Shapes.Range`).
pub struct ShapeRangeObj {
    pub s: Rc<Session>,
    pub sheet: u32,
    pub keys: Vec<DrawingKey>,
}

impl ShapeRangeObj {
    fn single(&self, name: &str) -> Result<Rc<ShapeObj>, ScriptError> {
        match self.keys.as_slice() {
            [k] => Ok(ShapeObj::new(&self.s, self.sheet, *k, false)),
            _ => Err(ScriptError::unsupported(&format!(
                "ShapeRange.{name} on several shapes"
            ))),
        }
    }
}

impl Dispatch for ShapeRangeObj {
    fn type_name(&self) -> String {
        "ShapeRange".into()
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "count" => Ok(Value::Long(self.keys.len() as i32)),
            "item" => {
                let i = to_i64(need(args, 0, "Index")?)?;
                if i < 1 || i as usize > self.keys.len() {
                    return Err(ScriptError::runtime(9));
                }
                Ok(obj(ShapeObj::new(
                    &self.s,
                    self.sheet,
                    self.keys[i as usize - 1],
                    false,
                )))
            }
            "select" => Ok(Value::Empty),
            "delete" => {
                let mut book = self.s.write()?;
                for k in &self.keys {
                    book.delete_drawing(*k).map_err(engine)?;
                }
                Ok(Value::Empty)
            }
            _ => self.single(name)?.get(name, args),
        }
    }

    fn put(&self, name: &str, args: &[Arg], value: Value) -> Result<(), ScriptError> {
        self.single(name)?.put(name, args, value)
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        Ok(self
            .keys
            .iter()
            .map(|k| obj(ShapeObj::new(&self.s, self.sheet, *k, false)))
            .collect())
    }
}

/// `Pictures`: the legacy collection of a sheet's pictures.
pub struct PicturesObj {
    pub s: Rc<Session>,
    pub sheet: u32,
}

impl Dispatch for PicturesObj {
    fn type_name(&self) -> String {
        "Pictures".into()
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "count" => Ok(Value::Long(objects(&self.s, self.sheet, true)?.len() as i32)),
            "item" | "" => {
                let list = objects(&self.s, self.sheet, true)?;
                let key = pick(&list, need(args, 0, "Index")?)?;
                Ok(obj(ShapeObj::new(&self.s, self.sheet, key, true)))
            }
            "insert" => {
                let file = to_str(need(args, 0, "Filename")?)?;
                // At the active cell's top left, at the picture's own size.
                let active = self.s.selection(self.sheet).active;
                let idx = self.s.sheet_index(self.sheet)?;
                let (left, top) = {
                    let book = self.s.read()?;
                    let g = Geo::new(&book, idx);
                    (g.col_left(active.1), g.row_top(active.0))
                };
                add_picture(&self.s, self.sheet, &file, (left, top), (None, None), true)
            }
            "delete" => {
                let list = objects(&self.s, self.sheet, true)?;
                let mut book = self.s.write()?;
                for d in &list {
                    book.delete_drawing(d.key).map_err(engine)?;
                }
                Ok(Value::Empty)
            }
            "select" => Ok(Value::Empty),
            "parent" => Ok(obj(SheetObj::new(&self.s, self.sheet))),
            _ => Err(ScriptError::no_member("Pictures", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        Ok(objects(&self.s, self.sheet, true)?
            .into_iter()
            .map(|d| obj(ShapeObj::new(&self.s, self.sheet, d.key, true)))
            .collect())
    }
}

/// `Shape` (and the legacy `Picture`): one object drawn over a sheet.
pub struct ShapeObj {
    pub s: Rc<Session>,
    /// The sheet's `sheetId`.
    pub sheet: u32,
    /// The engine's key of the object (stays the same while it exists).
    pub key: DrawingKey,
    /// Reached through `Pictures` (`TypeName` is `Picture`).
    pub legacy: bool,
}

impl ShapeObj {
    pub fn new(s: &Rc<Session>, sheet: u32, key: DrawingKey, legacy: bool) -> Rc<ShapeObj> {
        Rc::new(ShapeObj {
            s: s.clone(),
            sheet,
            key,
            legacy,
        })
    }

    fn idx(&self) -> Result<u32, ScriptError> {
        self.s.sheet_index(self.sheet)
    }

    fn drawing(&self) -> Result<Drawing, ScriptError> {
        let idx = self.idx()?;
        let book = self.s.read()?;
        find(&book, idx, self.key).ok_or_else(gone)
    }

    /// `[left, top, width, height]` in points.
    fn rect(&self) -> Result<[f64; 4], ScriptError> {
        let idx = self.idx()?;
        let book = self.s.read()?;
        let d = find(&book, idx, self.key).ok_or_else(gone)?;
        Ok(d.anchor.rect(&Geo::new(&book, idx)))
    }

    /// Moves or resizes the object: `f` turns its rectangle into the new one.
    fn place(&self, f: impl FnOnce([f64; 4]) -> [f64; 4]) -> Result<(), ScriptError> {
        let idx = self.idx()?;
        let mut book = self.s.write()?;
        let d = find(&book, idx, self.key).ok_or_else(gone)?;
        let anchor = {
            let g = Geo::new(&book, idx);
            let r = f(d.anchor.rect(&g));
            d.anchor.with_rect(r, &g)
        };
        book.place_drawing(self.key, anchor).map_err(engine)
    }

    /// The cell under the top left (`false`) or bottom right corner.
    fn corner_cell(&self, bottom_right: bool) -> Result<Value, ScriptError> {
        let idx = self.idx()?;
        let (row, col) = {
            let book = self.s.read()?;
            let d = find(&book, idx, self.key).ok_or_else(gone)?;
            let g = Geo::new(&book, idx);
            let [x, y, w, h] = d.anchor.rect(&g);
            if bottom_right {
                (g.row_at(y + h), g.col_at(x + w))
            } else {
                (g.row_at(y), g.col_at(x))
            }
        };
        Ok(obj(RangeObj::new(
            &self.s,
            self.sheet,
            CellRange::from_corners((row, col), (row, col)),
        )))
    }

    /// The shape's element; error 445 for pictures and charts (`what`
    /// names the member).
    fn shape_xml(&self, what: &str) -> Result<String, ScriptError> {
        match &*self.drawing()?.kind {
            DrawingKind::Shape { xml, .. } => Ok(xml.to_string()),
            _ => Err(ScriptError::unsupported(&format!(
                "{what} of pictures, charts and slicers"
            ))),
        }
    }

    fn look(&self, what: &str) -> Result<ShapeLook, ScriptError> {
        let xml = self.shape_xml(what)?;
        Ok(self.s.read()?.shape_look(&xml))
    }

    /// The shape's text (paragraphs joined by `\n`).
    fn text(&self) -> Result<String, ScriptError> {
        let xml = self.shape_xml("Text")?;
        if !takes_text(&xml) {
            return Err(err1004("This shape has no text"));
        }
        Ok(plain_text(&xml).unwrap_or_default())
    }

    fn set_text(&self, text: &str) -> Result<(), ScriptError> {
        self.shape_xml("Text")?;
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        self.s
            .write()?
            .set_shape_text(self.key, &text)
            .map_err(engine)
    }

    fn set_cnvpr(&self, attr: &str, value: Option<&str>) -> Result<(), ScriptError> {
        self.drawing()?;
        self.s
            .write()?
            .script_set_cnvpr(self.key, attr, value)
            .map_err(engine)
    }

    fn set_fill(&self, fill: ShapeFill) -> Result<(), ScriptError> {
        self.shape_xml("Fill")?;
        self.s
            .write()?
            .edit_shape(self.key, |x| set_fill(x, fill))
            .map_err(engine)
    }

    fn set_outline(&self, edit: OutlineEdit) -> Result<(), ScriptError> {
        self.shape_xml("Line")?;
        self.s
            .write()?
            .edit_shape(self.key, |x| set_outline(x, &edit))
            .map_err(engine)
    }
}

/// `0xRRGGBB` as DrawingML's `[r, g, b]`.
fn rgb_bytes(c: u32) -> [u8; 3] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8]
}

impl Dispatch for ShapeObj {
    fn type_name(&self) -> String {
        if self.legacy { "Picture" } else { "Shape" }.into()
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        let me = || {
            Rc::new(ShapeObj {
                s: self.s.clone(),
                sheet: self.sheet,
                key: self.key,
                legacy: self.legacy,
            })
        };
        match lower.as_str() {
            "name" | "" => Ok(Value::String(name_of(&self.drawing()?))),
            "type" => Ok(Value::Long(mso_type(&self.drawing()?) as i32)),
            "autoshapetype" => {
                let d = self.drawing()?;
                if mso_type(&d) != MSO_AUTO_SHAPE && mso_type(&d) != MSO_TEXT_BOX {
                    return Ok(Value::Long(MSO_SHAPE_MIXED as i32));
                }
                let prst = match &*d.kind {
                    DrawingKind::Shape { xml, .. } => shape_xml_info(xml).and_then(|i| i.prst),
                    _ => None,
                };
                Ok(Value::Long(
                    AUTO_SHAPES
                        .iter()
                        .find(|(_, _, p)| Some(*p) == prst.as_deref())
                        .map_or(MSO_SHAPE_NOT_PRIMITIVE, |(_, v, _)| *v) as i32,
                ))
            }
            "left" | "top" | "width" | "height" => {
                let [x, y, w, h] = self.rect()?;
                Ok(Value::Double(match lower.as_str() {
                    "left" => x,
                    "top" => y,
                    "width" => w,
                    _ => h,
                }))
            }
            "visible" => {
                let d = self.drawing()?;
                let hidden = match &*d.kind {
                    DrawingKind::Shape { xml, .. } => {
                        shape_xml_info(xml).map_or(d.hidden, |i| i.hidden)
                    }
                    _ => d.hidden,
                };
                Ok(tri(!hidden))
            }
            "id" => Ok(Value::Long(if self.key.index >= NEW_CHART_BASE {
                (self.key.index - NEW_CHART_BASE) as i32 + 1_000_000
            } else {
                self.key.index as i32 + 1
            })),
            "alternativetext" => {
                let d = self.drawing()?;
                let descr = match &*d.kind {
                    DrawingKind::Shape { xml, .. } => shape_xml_info(xml).and_then(|i| i.descr),
                    _ => None,
                };
                Ok(Value::String(descr.unwrap_or(d.descr)))
            }
            "onaction" => {
                self.drawing()?;
                let m = self.s.read()?.drawing_macro_now(self.key);
                let m = m.unwrap_or_default();
                Ok(Value::String(match m.strip_prefix("[0]!") {
                    Some(rest) => rest.to_owned(),
                    None => m,
                }))
            }
            "placement" => {
                let d = self.drawing()?;
                Ok(Value::Long(match d.anchor {
                    Anchor::TwoCell {
                        edit_as: EditAs::TwoCell,
                        ..
                    } => XL_MOVE_AND_SIZE,
                    Anchor::TwoCell {
                        edit_as: EditAs::OneCell,
                        ..
                    }
                    | Anchor::OneCell { .. } => XL_MOVE,
                    _ => XL_FREE_FLOATING,
                } as i32))
            }
            "topleftcell" => self.corner_cell(false),
            "bottomrightcell" => self.corner_cell(true),
            "zorderposition" => {
                let list = objects(&self.s, self.sheet, false)?;
                let at = list
                    .iter()
                    .position(|d| d.key == self.key)
                    .ok_or_else(gone)?;
                Ok(Value::Long(at as i32 + 1))
            }
            "haschart" => Ok(Value::Boolean(matches!(
                &*self.drawing()?.kind,
                DrawingKind::Chart { .. }
            ))),
            "chart" => {
                let d = self.drawing()?;
                if !matches!(&*d.kind, DrawingKind::Chart { .. }) {
                    return Err(err1004("This shape has no chart"));
                }
                crate::charts::chart_by_name(&self.s, self.sheet, &name_of(&d))
            }
            "hastextframe" => {
                let d = self.drawing()?;
                Ok(tri(matches!(&*d.kind,
                    DrawingKind::Shape { xml, .. } if takes_text(xml))))
            }
            "textframe" => Ok(obj(Rc::new(TextFrameObj {
                sh: me(),
                two: false,
            }))),
            "textframe2" => Ok(obj(Rc::new(TextFrameObj {
                sh: me(),
                two: true,
            }))),
            "fill" => {
                self.shape_xml("Fill")?;
                Ok(obj(Rc::new(FillObj { sh: me() })))
            }
            "line" => {
                self.shape_xml("Line")?;
                Ok(obj(Rc::new(LineObj { sh: me() })))
            }
            "shaperange" => Ok(obj(Rc::new(ShapeRangeObj {
                s: self.s.clone(),
                sheet: self.sheet,
                keys: vec![self.key],
            }))),
            "parent" => Ok(obj(SheetObj::new(&self.s, self.sheet))),
            "locked" => Ok(Value::Boolean(true)),
            // Methods.
            "select" => Ok(Value::Empty),
            "delete" => {
                self.drawing()?;
                self.s.write()?.delete_drawing(self.key).map_err(engine)?;
                Ok(Value::Empty)
            }
            "incrementleft" | "incrementtop" => {
                let by = to_f64(need(args, 0, "Increment")?)?;
                let left = lower == "incrementleft";
                self.place(|[x, y, w, h]| {
                    if left {
                        [x + by, y, w, h]
                    } else {
                        [x, y + by, w, h]
                    }
                })?;
                Ok(Value::Empty)
            }
            "zorder" | "duplicate" | "copy" | "cut" | "copypicture" | "flip" | "group"
            | "ungroup" | "scalewidth" | "scaleheight" | "incrementrotation" => Err(
                ScriptError::unsupported(&format!("Shape.{name} from a macro")),
            ),
            _ => Err(ScriptError::no_member(&self.type_name(), name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "name" => {
                let n = to_str(&value)?;
                if n.trim().is_empty() {
                    return Err(err1004("A shape needs a name"));
                }
                self.set_cnvpr("name", Some(&n))
            }
            "left" | "top" | "width" | "height" => {
                let v = to_f64(&value)?;
                if !v.is_finite() || (matches!(lower.as_str(), "width" | "height") && v < 0.0) {
                    return Err(err1004(format!("Unable to set the {name} property")));
                }
                self.place(|[x, y, w, h]| match lower.as_str() {
                    "left" => [v, y, w, h],
                    "top" => [x, v, w, h],
                    "width" => [x, y, v, h],
                    _ => [x, y, w, v],
                })
            }
            "visible" => {
                let show = to_bool(&value)?;
                if !matches!(&*self.drawing()?.kind, DrawingKind::Shape { .. }) {
                    return Err(ScriptError::unsupported(
                        "Visible of pictures, charts and slicers",
                    ));
                }
                self.set_cnvpr("hidden", if show { None } else { Some("1") })
            }
            "alternativetext" => {
                let t = to_str(&value)?;
                self.set_cnvpr("descr", if t.is_empty() { None } else { Some(&t) })
            }
            "onaction" => {
                let m = to_str(&value)?;
                let m = m.trim();
                let stored = if m.is_empty() || m.contains('!') {
                    m.to_owned()
                } else {
                    format!("[0]!{m}")
                };
                if !matches!(&*self.drawing()?.kind, DrawingKind::Shape { .. }) {
                    return Err(ScriptError::unsupported(
                        "OnAction of pictures, charts and slicers",
                    ));
                }
                self.s
                    .write()?
                    .script_set_macro(self.key, &stored)
                    .map_err(engine)
            }
            "placement" => {
                let edit_as = match to_i64(&value)? {
                    XL_MOVE_AND_SIZE => EditAs::TwoCell,
                    XL_MOVE => EditAs::OneCell,
                    XL_FREE_FLOATING => EditAs::Absolute,
                    _ => return Err(err1004("Unable to set the Placement property")),
                };
                let idx = self.idx()?;
                let mut book = self.s.write()?;
                let d = find(&book, idx, self.key).ok_or_else(gone)?;
                let rect = d.anchor.rect(&Geo::new(&book, idx));
                let anchor = anchor_at(&book, idx, rect, edit_as);
                book.place_drawing(self.key, anchor).map_err(engine)
            }
            "locked" | "lockaspectratio" => Ok(()),
            _ => Err(ScriptError::no_member(&self.type_name(), name)),
        }
    }
}

/// `TextFrame` (`two`: `TextFrame2`).
pub struct TextFrameObj {
    sh: Rc<ShapeObj>,
    two: bool,
}

impl TextFrameObj {
    fn characters(&self, args: &[Arg]) -> Result<Value, ScriptError> {
        let start = arg_i64(args, 0, "Start")?.unwrap_or(1);
        let length = arg_i64(args, 1, "Length")?;
        if start < 1 || length.is_some_and(|l| l < 0) {
            return Err(ScriptError::runtime(5));
        }
        Ok(obj(Rc::new(CharactersObj {
            sh: self.sh.clone(),
            start,
            length,
            two: self.two,
        })))
    }
}

impl Dispatch for TextFrameObj {
    fn type_name(&self) -> String {
        if self.two { "TextFrame2" } else { "TextFrame" }.into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "characters" if !self.two => self.characters(args),
            "textrange" if self.two => Ok(obj(Rc::new(CharactersObj {
                sh: self.sh.clone(),
                start: 1,
                length: None,
                two: true,
            }))),
            "hastext" => {
                let has = self.sh.text().is_ok_and(|t| !t.is_empty());
                Ok(if self.two {
                    tri(has)
                } else {
                    Value::Boolean(has)
                })
            }
            "parent" => Ok(obj(self.sh.clone())),
            _ => Err(ScriptError::no_member(&self.type_name(), name)),
        }
    }
}

/// `Characters` / `TextRange2`: characters `start..` (1-based; `length`
/// of them, else to the end) of a shape's text. `TextFrame2` separates
/// paragraphs with `vbCr`, `TextFrame` with `vbLf`, as Excel does.
pub struct CharactersObj {
    sh: Rc<ShapeObj>,
    start: i64,
    length: Option<i64>,
    two: bool,
}

impl CharactersObj {
    fn full(&self) -> Result<Vec<char>, ScriptError> {
        let t = self.sh.text()?;
        Ok(if self.two {
            t.replace('\n', "\r").chars().collect()
        } else {
            t.chars().collect()
        })
    }

    /// The character range in `full`.
    fn span(&self, n: usize) -> (usize, usize) {
        let a = (self.start as usize - 1).min(n);
        let b = match self.length {
            Some(l) => (a + l as usize).min(n),
            None => n,
        };
        (a, b)
    }

    fn text(&self) -> Result<String, ScriptError> {
        let full = self.full()?;
        let (a, b) = self.span(full.len());
        Ok(full[a..b].iter().collect())
    }

    fn set(&self, text: &str) -> Result<(), ScriptError> {
        let full = self.full()?;
        let (a, b) = self.span(full.len());
        let mut out: String = full[..a].iter().collect();
        out.push_str(text);
        out.extend(full[b..].iter());
        self.sh.set_text(&out)
    }
}

impl Dispatch for CharactersObj {
    fn type_name(&self) -> String {
        if self.two { "TextRange2" } else { "Characters" }.into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "text" | "caption" | "" => Ok(Value::String(self.text()?)),
            "count" | "length" => Ok(Value::Long(self.text()?.chars().count() as i32)),
            "characters" => {
                let start = arg_i64(args, 0, "Start")?.unwrap_or(1);
                let length = arg_i64(args, 1, "Length")?;
                if start < 1 || length.is_some_and(|l| l < 0) {
                    return Err(ScriptError::runtime(5));
                }
                Ok(obj(Rc::new(CharactersObj {
                    sh: self.sh.clone(),
                    start: self.start + start - 1,
                    length: match (self.length, length) {
                        (Some(outer), Some(l)) => Some(l.min((outer - start + 1).max(0))),
                        (Some(outer), None) => Some((outer - start + 1).max(0)),
                        (None, l) => l,
                    },
                    two: self.two,
                })))
            }
            "insert" => {
                let t = to_str(need(args, 0, "String")?)?;
                self.set(&t)?;
                Ok(Value::Empty)
            }
            "delete" => {
                self.set("")?;
                Ok(Value::Empty)
            }
            "font" => Err(ScriptError::unsupported(
                "the font of part of a shape's text from a macro",
            )),
            _ => Err(ScriptError::no_member(&self.type_name(), name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "text" | "caption" | "" => self.set(&to_str(&value)?),
            _ => Err(ScriptError::no_member(&self.type_name(), name)),
        }
    }
}

/// `FillFormat` of a shape.
pub struct FillObj {
    sh: Rc<ShapeObj>,
}

impl FillObj {
    /// A solid fill of `rgb` keeping the fill's transparency.
    fn solid(&self, rgb: u32, transparency: Option<f64>) -> Result<(), ScriptError> {
        let t = match transparency {
            Some(t) => t,
            None => self.sh.look("Fill")?.transparency,
        };
        self.sh.set_fill(ShapeFill::Solid {
            rgb: rgb_bytes(rgb),
            transparency: (t.clamp(0.0, 1.0) * 100.0).round() as u8,
        })
    }

    /// The colour a fill shown again or made solid gets.
    fn current(&self) -> Result<u32, ScriptError> {
        let look = self.sh.look("Fill")?;
        Ok(match look.fill {
            Some(c) => c,
            None => self.sh.s.read()?.theme_rgb(4, 0.0),
        })
    }
}

impl Dispatch for FillObj {
    fn type_name(&self) -> String {
        "FillFormat".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "forecolor" => Ok(obj(Rc::new(ColorObj {
                sh: self.sh.clone(),
                line: false,
            }))),
            "visible" => Ok(tri(self.sh.look("Fill")?.fill.is_some())),
            "transparency" => Ok(Value::Double(self.sh.look("Fill")?.transparency)),
            "solid" => {
                let c = self.current()?;
                self.solid(c, None)?;
                Ok(Value::Empty)
            }
            "parent" => Ok(obj(self.sh.clone())),
            _ => Err(ScriptError::no_member("FillFormat", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "visible" => {
                if to_bool(&value)? {
                    if self.sh.look("Fill")?.fill.is_none() {
                        let c = self.current()?;
                        self.solid(c, Some(0.0))?;
                    }
                    Ok(())
                } else {
                    self.sh.set_fill(ShapeFill::None)
                }
            }
            "transparency" => {
                let t = to_f64(&value)?;
                if !(0.0..=1.0).contains(&t) {
                    return Err(err1004("The value is out of range"));
                }
                let c = self.current()?;
                self.solid(c, Some(t))
            }
            _ => Err(ScriptError::no_member("FillFormat", name)),
        }
    }
}

/// `LineFormat` of a shape.
pub struct LineObj {
    sh: Rc<ShapeObj>,
}

impl Dispatch for LineObj {
    fn type_name(&self) -> String {
        "LineFormat".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "forecolor" => Ok(obj(Rc::new(ColorObj {
                sh: self.sh.clone(),
                line: true,
            }))),
            "visible" => Ok(tri(self.sh.look("Line")?.line.is_some())),
            "weight" => Ok(Value::Double(self.sh.look("Line")?.line_width)),
            "parent" => Ok(obj(self.sh.clone())),
            _ => Err(ScriptError::no_member("LineFormat", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "visible" => {
                let color = if to_bool(&value)? {
                    let look = self.sh.look("Line")?;
                    if look.line.is_some() {
                        return Ok(());
                    }
                    Some(rgb_bytes(0x000000))
                } else {
                    None
                };
                self.sh.set_outline(OutlineEdit {
                    color: Some(color),
                    ..Default::default()
                })
            }
            "weight" => {
                let pt = to_f64(&value)?;
                if !pt.is_finite() || pt < 0.0 {
                    return Err(err1004("The value is out of range"));
                }
                self.sh.set_outline(OutlineEdit {
                    width: Some((pt * 12_700.0).round() as i64),
                    ..Default::default()
                })
            }
            _ => Err(ScriptError::no_member("LineFormat", name)),
        }
    }
}

/// `ColorFormat`: a fill's or line's colour.
pub struct ColorObj {
    sh: Rc<ShapeObj>,
    line: bool,
}

impl Dispatch for ColorObj {
    fn type_name(&self) -> String {
        "ColorFormat".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "rgb" | "" => {
                let look = self.sh.look(if self.line { "Line" } else { "Fill" })?;
                let c = if self.line { look.line } else { look.fill };
                Ok(Value::Long(rgb_to_bgr(c.unwrap_or(if self.line {
                    0x000000
                } else {
                    0xFFFFFF
                }))))
            }
            _ => Err(ScriptError::no_member("ColorFormat", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "rgb" | "" => {
                let rgb = bgr_to_rgb(to_i64(&value)?);
                if self.line {
                    self.sh.set_outline(OutlineEdit {
                        color: Some(Some(rgb_bytes(rgb))),
                        ..Default::default()
                    })
                } else {
                    FillObj {
                        sh: self.sh.clone(),
                    }
                    .solid(rgb, None)
                }
            }
            "objectthemecolor" | "schemecolor" | "tintandshade" | "brightness" => Err(
                ScriptError::unsupported(&format!("ColorFormat.{name} from a macro")),
            ),
            _ => Err(ScriptError::no_member("ColorFormat", name)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_of_new_autoshapes() {
        assert_eq!(base_name("msoShapeRectangle", "rect"), "Rectangle");
        assert_eq!(
            base_name("msoShapeIsoscelesTriangle", "triangle"),
            "Isosceles Triangle"
        );
        assert_eq!(
            base_name("msoShapeRightArrow", "rightArrow"),
            "Arrow: Right"
        );
        assert!(constants().any(|(n, v)| n == "msoShapeOval" && v == 9));
    }
}
