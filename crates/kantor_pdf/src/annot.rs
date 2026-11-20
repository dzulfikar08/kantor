//! Annotations (ISO 32000-1 §12.5): comments and markup the PDF window
//! adds, moves and edits, written as standard annotation dictionaries with
//! appearance streams so every viewer shows them the same.
//!
//! [`read_page`] lists a page's annotations: the kinds Kantor edits become
//! [`Kind`]s, everything else ([`Kind::Other`]: links, form widgets, 3-D,
//! sounds…) is listed so it is kept untouched. [`apply_page`] writes an
//! edited list back: annotations not changed stay byte-for-byte the same
//! objects; changed ones keep their dictionary (and keys Kantor doesn't
//! know) with the edited keys and a new appearance. [`layer`] builds a
//! one-page PDF with only a page's annotations, for the viewer to draw
//! over the page while it is being edited. [`flatten`] draws stamps and
//! signatures into the page content.
//!
//! Coordinates are PDF user space (points, y up), as written in the file.

use std::collections::HashMap;
use std::sync::Arc;

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use crate::form::{self, FieldValue};
use crate::pages::{inherited, page_ids, rect};
use crate::pdfstr::{copy_object, get_text, num, text};
use crate::{Error, Pdf, Result};

/// A standard font for text boxes (no embedding needed).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Font {
    #[default]
    Helvetica,
    Courier,
}

impl Font {
    pub fn base_font(self) -> &'static str {
        match self {
            Font::Helvetica => "Helvetica",
            Font::Courier => "Courier",
        }
    }

    fn resource(self) -> &'static str {
        match self {
            Font::Helvetica => "Helv",
            Font::Courier => "Cour",
        }
    }

    /// Width of `s` at `size` points.
    pub fn width(self, s: &str, size: f32) -> f32 {
        match self {
            Font::Helvetica => crate::overlay::helvetica_width(s) * size / 1000.0,
            Font::Courier => s.chars().count() as f32 * 0.6 * size,
        }
    }
}

/// Drawing commands of a filled vector picture (a typed signature): each
/// path is a list of segments in the stamp's box, 0..1 on both axes, y up.
#[derive(Clone, Debug, PartialEq)]
pub enum Seg {
    Move(f32, f32),
    Line(f32, f32),
    Curve(f32, f32, f32, f32, f32, f32),
    Close,
}

/// What a stamp shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Art {
    /// A PNG or JPEG file (PNG transparency kept as a soft mask).
    Image(Arc<Vec<u8>>),
    /// Filled outlines, 0..1 in the box (a typed signature).
    Paths(Arc<Vec<Seg>>),
    /// The appearance the file already had (a stamp from another program,
    /// or one read back): kept, scaled to the box.
    Keep,
}

/// The kinds of annotation Kantor edits.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// Text markup: quadrilaterals as Acrobat writes them (upper left,
    /// upper right, lower left, lower right).
    Highlight(Vec<[f32; 8]>),
    Underline(Vec<[f32; 8]>),
    StrikeOut(Vec<[f32; 8]>),
    /// A sticky note (Text annotation; its pop-up is written along).
    /// Replies are notes with [`Annot::reply_to`] set.
    Note,
    /// A review state (Acrobat's "Completed" check) on another annotation:
    /// `"Completed"`, `"Accepted"`, `"Rejected"`, `"Cancelled"`, `"None"`.
    State(String),
    /// A text box; `color` is the text and border colour, `fill` the
    /// background.
    FreeText {
        font: Font,
        size: f32,
    },
    /// Freehand strokes.
    Ink(Vec<Vec<[f32; 2]>>),
    Square,
    Circle,
    /// A line; `arrow` puts an open arrowhead at the end `b`.
    Line {
        a: [f32; 2],
        b: [f32; 2],
        arrow: bool,
    },
    Polygon(Vec<[f32; 2]>),
    /// A stamp: an image, a typed signature or a kept appearance, turned
    /// `rotate` degrees counterclockwise around the centre of `rect` (the
    /// unturned box). `signature` marks Fill & Sign signatures.
    Stamp {
        art: Art,
        rotate: f32,
        signature: bool,
    },
    /// Any other annotation: kept as it is.
    Other(String),
}

impl Kind {
    pub fn subtype(&self) -> &str {
        match self {
            Kind::Highlight(_) => "Highlight",
            Kind::Underline(_) => "Underline",
            Kind::StrikeOut(_) => "StrikeOut",
            Kind::Note | Kind::State(_) => "Text",
            Kind::FreeText { .. } => "FreeText",
            Kind::Ink(_) => "Ink",
            Kind::Square => "Square",
            Kind::Circle => "Circle",
            Kind::Line { .. } => "Line",
            Kind::Polygon(_) => "Polygon",
            Kind::Stamp { .. } => "Stamp",
            Kind::Other(s) => s,
        }
    }

    /// Kantor can move and change it.
    pub fn editable(&self) -> bool {
        !matches!(self, Kind::Other(_) | Kind::State(_))
    }
}

/// One annotation of a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Annot {
    /// Its name (`/NM`); made up for annotations that have none.
    pub id: String,
    pub kind: Kind,
    /// User space `[x0 y0 x1 y1]`. For a turned stamp, the unturned box.
    pub rect: [f32; 4],
    /// RGB 0..1: markup and outline colour (text colour for text boxes).
    pub color: [f32; 3],
    /// Interior / background colour.
    pub fill: Option<[f32; 3]>,
    /// 0..1.
    pub opacity: f32,
    /// Line width, points (0: no border on a text box).
    pub width: f32,
    /// The comment (or a text box's text).
    pub contents: String,
    /// Who wrote it (`/T`).
    pub author: String,
    /// Its subject (`/Subj`): `Signature` for Fill & Sign signatures.
    pub subject: String,
    /// When it was last changed, as a PDF date.
    pub modified: String,
    /// The annotation this one replies to (its id).
    pub reply_to: Option<String>,
    /// Hidden from view (`/F` Hidden or NoView).
    pub hidden: bool,
    /// Its place in the page's original `/Annots` (an annotation read from
    /// the file).
    pub orig: Option<usize>,
    /// Changed since it was read: written again.
    pub dirty: bool,
}

impl Annot {
    /// A new annotation (not from the file yet).
    pub fn new(kind: Kind, rect: [f32; 4], color: [f32; 3]) -> Self {
        Self {
            id: new_id(),
            kind,
            rect: norm(rect),
            color,
            fill: None,
            opacity: 1.0,
            width: 1.0,
            contents: String::new(),
            author: String::new(),
            subject: String::new(),
            modified: crate::pdfstr::pdf_date_now(),
            reply_to: None,
            hidden: false,
            orig: None,
            dirty: true,
        }
    }

    /// The box it covers on the page (for a turned stamp, the turned box).
    pub fn bounds(&self) -> [f32; 4] {
        match &self.kind {
            Kind::Stamp { rotate, .. } if rotate.rem_euclid(360.0) != 0.0 => {
                turned_bounds(self.rect, *rotate)
            }
            _ => self.rect,
        }
    }

    /// Move by `(dx, dy)` points.
    pub fn translate(&mut self, dx: f32, dy: f32) {
        let r = self.rect;
        self.transform(
            |x, y| (x + dx, y + dy),
            [r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy],
        );
    }

    /// Fit into the box `to` (geometry scaled from the current box).
    pub fn resize(&mut self, to: [f32; 4]) {
        let to = norm(to);
        let r = self.rect;
        let sx = (to[2] - to[0]) / (r[2] - r[0]).max(1e-3);
        let sy = (to[3] - to[1]) / (r[3] - r[1]).max(1e-3);
        self.transform(
            |x, y| (to[0] + (x - r[0]) * sx, to[1] + (y - r[1]) * sy),
            to,
        );
    }

    fn transform(&mut self, f: impl Fn(f32, f32) -> (f32, f32), rect: [f32; 4]) {
        let quad = |q: &mut [f32; 8]| {
            for i in 0..4 {
                let (x, y) = f(q[2 * i], q[2 * i + 1]);
                q[2 * i] = x;
                q[2 * i + 1] = y;
            }
        };
        let pt = |p: &mut [f32; 2]| {
            let (x, y) = f(p[0], p[1]);
            *p = [x, y];
        };
        match &mut self.kind {
            Kind::Highlight(q) | Kind::Underline(q) | Kind::StrikeOut(q) => {
                q.iter_mut().for_each(quad)
            }
            Kind::Ink(s) => s.iter_mut().flatten().for_each(pt),
            Kind::Line { a, b, .. } => {
                pt(a);
                pt(b);
            }
            Kind::Polygon(v) => v.iter_mut().for_each(pt),
            _ => {}
        }
        self.rect = rect;
        self.dirty = true;
    }

    /// Mark as changed now.
    pub fn touch(&mut self) {
        self.dirty = true;
        self.modified = crate::pdfstr::pdf_date_now();
    }
}

/// A fresh annotation name.
pub fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    format!(
        "kantor-{:x}-{:x}-{:x}",
        t,
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

pub(crate) fn norm(r: [f32; 4]) -> [f32; 4] {
    [
        r[0].min(r[2]),
        r[1].min(r[3]),
        r[0].max(r[2]),
        r[1].max(r[3]),
    ]
}

fn turned_bounds(r: [f32; 4], deg: f32) -> [f32; 4] {
    let (cx, cy) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    let (hw, hh) = ((r[2] - r[0]) / 2.0, (r[3] - r[1]) / 2.0);
    let (s, c) = deg.to_radians().sin_cos();
    let ex = (hw * c).abs() + (hh * s).abs();
    let ey = (hw * s).abs() + (hh * c).abs();
    [cx - ex, cy - ey, cx + ex, cy + ey]
}

// ── reading ────────────────────────────────────────────────────────────────

const F_HIDDEN: i64 = 1 << 1;
const F_PRINT: i64 = 1 << 2;
const F_NOZOOM: i64 = 1 << 3;
const F_NOROTATE: i64 = 1 << 4;
const F_NOVIEW: i64 = 1 << 5;

fn num_of(doc: &Document, o: &Object) -> Option<f32> {
    crate::pages::number(doc, o)
}

fn nums(doc: &Document, d: &Dictionary, key: &[u8]) -> Vec<f32> {
    d.get(key)
        .ok()
        .and_then(|o| doc.dereference(o).ok())
        .and_then(|(_, o)| o.as_array().ok())
        .map(|a| a.iter().filter_map(|o| num_of(doc, o)).collect())
        .unwrap_or_default()
}

fn color(doc: &Document, d: &Dictionary, key: &[u8]) -> Option<[f32; 3]> {
    let v = nums(doc, d, key);
    match v.len() {
        1 => Some([v[0]; 3]),
        3 => Some([v[0], v[1], v[2]]),
        4 => {
            let k = 1.0 - v[3];
            Some([(1.0 - v[0]) * k, (1.0 - v[1]) * k, (1.0 - v[2]) * k])
        }
        _ => None,
    }
}

fn name<'a>(d: &'a Dictionary, key: &[u8]) -> Option<&'a [u8]> {
    d.get(key).and_then(Object::as_name).ok()
}

fn points(v: &[f32]) -> Vec<[f32; 2]> {
    v.as_chunks::<2>().0.to_vec()
}

fn quads(v: &[f32]) -> Vec<[f32; 8]> {
    v.as_chunks::<8>().0.to_vec()
}

/// The annotation dictionaries of page `page` (0-based) with their object
/// ids (None for direct ones).
pub(crate) fn page_annots(doc: &Document, page: ObjectId) -> Vec<(Option<ObjectId>, Dictionary)> {
    let Some(arr) = doc
        .get_dictionary(page)
        .ok()
        .and_then(|p| p.get(b"Annots").ok())
        .and_then(|a| doc.dereference(a).ok())
        .and_then(|(_, a)| a.as_array().ok().cloned())
    else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|o| match o {
            Object::Reference(id) => doc.get_dictionary(*id).ok().map(|d| (Some(*id), d.clone())),
            Object::Dictionary(d) => Some((None, d.clone())),
            _ => None,
        })
        .collect()
}

/// The annotations of page `page` (0-based), in the file's order. Pop-ups
/// are left out (they belong to their parent and are kept with it).
pub fn read_page(pdf: &Pdf, page: u32) -> Vec<Annot> {
    let doc = &pdf.doc;
    let Some(&pid) = page_ids(doc).get(page as usize) else {
        return Vec::new();
    };
    let list = page_annots(doc, pid);
    // Object id → name, for replies.
    let ids: HashMap<ObjectId, String> = list
        .iter()
        .enumerate()
        .filter_map(|(i, (id, d))| Some(((*id)?, annot_name(doc, d, page, i))))
        .collect();
    let mut out = Vec::new();
    for (i, (_, d)) in list.iter().enumerate() {
        if name(d, b"Subtype") == Some(b"Popup") {
            continue;
        }
        out.push(read_one(doc, d, page, i, &ids));
    }
    out
}

fn annot_name(doc: &Document, d: &Dictionary, page: u32, i: usize) -> String {
    get_text(doc, d, b"NM")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("p{page}-a{i}"))
}

fn read_one(
    doc: &Document,
    d: &Dictionary,
    page: u32,
    i: usize,
    ids: &HashMap<ObjectId, String>,
) -> Annot {
    let subtype = name(d, b"Subtype").unwrap_or(b"");
    let r = d
        .get(b"Rect")
        .ok()
        .and_then(|o| rect(doc, o))
        .unwrap_or([0.0; 4]);
    let flags = d.get(b"F").ok().and_then(|o| num_of(doc, o)).unwrap_or(0.0) as i64;
    let width = d
        .get(b"BS")
        .ok()
        .and_then(|o| doc.dereference(o).ok())
        .and_then(|(_, o)| o.as_dict().ok())
        .and_then(|bs| bs.get(b"W").ok())
        .and_then(|w| num_of(doc, w))
        .or_else(|| nums(doc, d, b"Border").get(2).copied())
        .unwrap_or(1.0);
    let mut rect_out = r;
    let kind = match subtype {
        b"Highlight" => Kind::Highlight(quads(&nums(doc, d, b"QuadPoints"))),
        b"Underline" => Kind::Underline(quads(&nums(doc, d, b"QuadPoints"))),
        b"StrikeOut" => Kind::StrikeOut(quads(&nums(doc, d, b"QuadPoints"))),
        b"Text" => match get_text(doc, d, b"State") {
            Some(s) if d.has(b"IRT") => Kind::State(s),
            _ => Kind::Note,
        },
        b"FreeText" => {
            let (font, size, _) = parse_da(&get_text(doc, d, b"DA").unwrap_or_default());
            Kind::FreeText { font, size }
        }
        b"Ink" => {
            let strokes = d
                .get(b"InkList")
                .ok()
                .and_then(|o| doc.dereference(o).ok())
                .and_then(|(_, o)| o.as_array().ok())
                .map(|a| {
                    a.iter()
                        .filter_map(|s| {
                            let s = doc.dereference(s).ok()?.1.as_array().ok()?;
                            let v: Vec<f32> = s.iter().filter_map(|o| num_of(doc, o)).collect();
                            Some(points(&v))
                        })
                        .collect()
                })
                .unwrap_or_default();
            Kind::Ink(strokes)
        }
        b"Square" => Kind::Square,
        b"Circle" => Kind::Circle,
        b"Line" => {
            let l = nums(doc, d, b"L");
            let arrow = d
                .get(b"LE")
                .ok()
                .and_then(|o| o.as_array().ok())
                .and_then(|a| a.get(1))
                .and_then(|o| o.as_name().ok())
                .is_some_and(|n| n != b"None");
            if l.len() == 4 {
                Kind::Line {
                    a: [l[0], l[1]],
                    b: [l[2], l[3]],
                    arrow,
                }
            } else {
                Kind::Other("Line".into())
            }
        }
        b"Polygon" => Kind::Polygon(points(&nums(doc, d, b"Vertices"))),
        b"Stamp" => {
            let (rotate, unturned) = stamp_frame(doc, d, r);
            rect_out = unturned;
            Kind::Stamp {
                art: Art::Keep,
                rotate,
                signature: get_text(doc, d, b"Name").is_some_and(|n| n.contains("Signature"))
                    || name(d, b"Name").is_some_and(|n| n.ends_with(b"Signature")),
            }
        }
        other => Kind::Other(String::from_utf8_lossy(other).into_owned()),
    };
    let base_color = match &kind {
        Kind::FreeText { .. } => parse_da(&get_text(doc, d, b"DA").unwrap_or_default()).2,
        _ => color(doc, d, b"C").unwrap_or([1.0, 0.82, 0.0]),
    };
    let fill = match &kind {
        Kind::FreeText { .. } => color(doc, d, b"C"),
        _ => color(doc, d, b"IC"),
    };
    let reply_to = d
        .get(b"IRT")
        .ok()
        .and_then(|o| o.as_reference().ok())
        .and_then(|id| ids.get(&id).cloned());
    Annot {
        id: annot_name(doc, d, page, i),
        kind,
        rect: rect_out,
        color: base_color,
        fill,
        opacity: d
            .get(b"CA")
            .ok()
            .and_then(|o| num_of(doc, o))
            .unwrap_or(1.0),
        width,
        contents: get_text(doc, d, b"Contents").unwrap_or_default(),
        author: get_text(doc, d, b"T").unwrap_or_default(),
        subject: get_text(doc, d, b"Subj").unwrap_or_default(),
        modified: get_text(doc, d, b"M").unwrap_or_default(),
        reply_to,
        hidden: flags & (F_HIDDEN | F_NOVIEW) != 0,
        orig: Some(i),
        dirty: false,
    }
}

/// A stamp's turn (from its appearance's matrix) and unturned box.
fn stamp_frame(doc: &Document, d: &Dictionary, r: [f32; 4]) -> (f32, [f32; 4]) {
    let Some(ap) = normal_appearance(doc, d) else {
        return (0.0, r);
    };
    let m = nums(doc, &ap.dict, b"Matrix");
    let bbox = ap
        .dict
        .get(b"BBox")
        .ok()
        .and_then(|o| rect(doc, o))
        .unwrap_or(r);
    if m.len() != 6 || (m[1].abs() < 1e-4 && m[2].abs() < 1e-4) {
        return (0.0, r);
    }
    let deg = m[1].atan2(m[0]).to_degrees();
    let (bw, bh) = (bbox[2] - bbox[0], bbox[3] - bbox[1]);
    let tb = turned_bounds([0.0, 0.0, bw, bh], deg);
    let k = (r[2] - r[0]) / (tb[2] - tb[0]).max(1e-3);
    let (cx, cy) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    let (w, h) = (bw * k / 2.0, bh * k / 2.0);
    (deg, [cx - w, cy - h, cx + w, cy + h])
}

fn normal_appearance(doc: &Document, d: &Dictionary) -> Option<Stream> {
    let ap = doc.dereference(d.get(b"AP").ok()?).ok()?.1.as_dict().ok()?;
    match doc.dereference(ap.get(b"N").ok()?).ok()?.1 {
        Object::Stream(s) => Some(s.clone()),
        Object::Dictionary(states) => {
            let st = name(d, b"AS")?;
            match doc.dereference(states.get(st).ok()?).ok()?.1 {
                Object::Stream(s) => Some(s.clone()),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Font, size and colour of a default appearance string such as
/// `/Helv 12 Tf 0 0 1 rg`.
pub(crate) fn parse_da(da: &str) -> (Font, f32, [f32; 3]) {
    let toks: Vec<&str> = da.split_whitespace().collect();
    let mut font = Font::Helvetica;
    let mut size = 12.0;
    let mut color = [0.0; 3];
    for (i, t) in toks.iter().enumerate() {
        let n = |k: usize| -> f32 {
            toks.get(i.wrapping_sub(k))
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0)
        };
        match *t {
            "Tf" => {
                size = n(1);
                let f = toks.get(i.wrapping_sub(2)).copied().unwrap_or("");
                if f.to_ascii_lowercase().contains("cour") {
                    font = Font::Courier;
                }
            }
            "rg" => color = [n(3), n(2), n(1)],
            "g" => color = [n(1); 3],
            "k" => {
                let k = 1.0 - n(1);
                color = [(1.0 - n(4)) * k, (1.0 - n(3)) * k, (1.0 - n(2)) * k];
            }
            _ => {}
        }
    }
    (font, size, color)
}

// ── writing ────────────────────────────────────────────────────────────────

fn color_obj(c: [f32; 3]) -> Object {
    Object::Array(c.iter().map(|&v| Object::Real(v)).collect())
}

fn reals(v: impl IntoIterator<Item = f32>) -> Object {
    Object::Array(v.into_iter().map(Object::Real).collect())
}

/// Writes annotations into one document (the file, or a layer).
pub(crate) struct Writer {
    /// Font resources shared by the appearances written.
    fonts: HashMap<Font, ObjectId>,
}

impl Writer {
    pub(crate) fn new() -> Self {
        Self {
            fonts: HashMap::new(),
        }
    }

    fn font(&mut self, doc: &mut Document, f: Font) -> ObjectId {
        *self.fonts.entry(f).or_insert_with(|| {
            let mut d = Dictionary::new();
            d.set("Type", "Font");
            d.set("Subtype", "Type1");
            d.set("BaseFont", f.base_font());
            d.set("Encoding", "WinAnsiEncoding");
            doc.add_object(d)
        })
    }

    /// The dictionary for `a`: `base` (its old dictionary, kept keys and
    /// all) with Kantor's keys and a new appearance. `parent_ref`: the
    /// object of the annotation it replies to.
    pub(crate) fn write(
        &mut self,
        doc: &mut Document,
        page: ObjectId,
        base: Option<Dictionary>,
        a: &Annot,
        parent_ref: Option<ObjectId>,
    ) -> Result<Dictionary> {
        let mut d = base.unwrap_or_default();
        let old_ap = normal_appearance(doc, &d);
        d.set("Type", "Annot");
        d.set("Subtype", a.kind.subtype());
        d.set("P", page);
        d.set("NM", text(&a.id));
        d.set("Contents", text(&a.contents));
        if !a.author.is_empty() {
            d.set("T", text(&a.author));
        }
        if !a.subject.is_empty() {
            d.set("Subj", text(&a.subject));
        }
        if !a.modified.is_empty() {
            d.set("M", text(&a.modified));
        }
        if !d.has(b"CreationDate") && !a.modified.is_empty() {
            d.set("CreationDate", text(&a.modified));
        }
        let mut flags = F_PRINT;
        if matches!(a.kind, Kind::Note | Kind::State(_)) {
            flags |= F_NOZOOM | F_NOROTATE;
        }
        if a.hidden {
            flags |= F_HIDDEN;
        }
        d.set("F", flags);
        match parent_ref {
            Some(p) => d.set("IRT", p),
            None => {
                d.remove(b"IRT");
            }
        }
        if a.opacity < 1.0 {
            d.set("CA", a.opacity);
        } else {
            d.remove(b"CA");
        }
        let bounds = a.bounds();
        d.set("Rect", reals(bounds));
        d.set("C", color_obj(a.color));
        let mut bs = Dictionary::new();
        bs.set("W", a.width);
        bs.set("S", "S");
        let ap: Option<Stream> = match &a.kind {
            Kind::Highlight(q) | Kind::Underline(q) | Kind::StrikeOut(q) => {
                d.set("C", color_obj(a.color));
                d.set("QuadPoints", reals(q.iter().flatten().copied()));
                Some(self.markup(doc, a, q))
            }
            Kind::Note => {
                d.set("C", color_obj(a.color));
                d.set("Name", "Comment");
                if a.reply_to.is_some() {
                    // Replies show in the comment list, not on the page.
                    d.remove(b"AP");
                    None
                } else {
                    Some(note_ap(a))
                }
            }
            Kind::State(s) => {
                d.set("State", text(s));
                d.set("StateModel", text("Review"));
                d.set("F", F_HIDDEN | F_NOVIEW | F_PRINT);
                d.remove(b"AP");
                None
            }
            Kind::FreeText { font, size } => {
                let f = self.font(doc, *font);
                d.set(
                    "DA",
                    text(&format!(
                        "/{} {} Tf {} {} {} rg",
                        font.resource(),
                        num(*size),
                        num(a.color[0]),
                        num(a.color[1]),
                        num(a.color[2])
                    )),
                );
                match a.fill {
                    Some(c) => d.set("C", color_obj(c)),
                    None => {
                        d.remove(b"C");
                    }
                }
                d.set("BS", bs.clone());
                d.remove(b"Border");
                d.remove(b"RC");
                d.remove(b"DS");
                Some(freetext_ap(a, *font, *size, f))
            }
            Kind::Ink(strokes) => {
                d.set("C", color_obj(a.color));
                d.set("BS", bs.clone());
                d.set(
                    "InkList",
                    Object::Array(
                        strokes
                            .iter()
                            .map(|s| reals(s.iter().flatten().copied()))
                            .collect(),
                    ),
                );
                Some(shape_ap(a))
            }
            Kind::Square | Kind::Circle | Kind::Polygon(_) | Kind::Line { .. } => {
                d.set("C", color_obj(a.color));
                d.set("BS", bs.clone());
                match a.fill {
                    Some(c) => d.set("IC", color_obj(c)),
                    None => {
                        d.remove(b"IC");
                    }
                }
                match &a.kind {
                    Kind::Line { a: p, b: q, arrow } => {
                        d.set("L", reals([p[0], p[1], q[0], q[1]]));
                        d.set(
                            "LE",
                            vec![
                                Object::Name(b"None".to_vec()),
                                Object::Name(if *arrow {
                                    b"OpenArrow".to_vec()
                                } else {
                                    b"None".to_vec()
                                }),
                            ],
                        );
                    }
                    Kind::Polygon(v) => d.set("Vertices", reals(v.iter().flatten().copied())),
                    _ => {}
                }
                Some(shape_ap(a))
            }
            Kind::Stamp {
                art,
                rotate,
                signature,
            } => {
                d.set(
                    "Name",
                    Object::Name(if *signature {
                        b"KantorSignature".to_vec()
                    } else {
                        b"KantorImage".to_vec()
                    }),
                );
                Some(stamp_ap(doc, a, art, *rotate, old_ap)?)
            }
            Kind::Other(_) => return Ok(d),
        };
        if let Some(ap) = ap {
            let id = doc.add_object(ap);
            let mut apd = Dictionary::new();
            apd.set("N", id);
            d.set("AP", apd);
            d.remove(b"AS");
        }
        Ok(d)
    }

    fn markup(&mut self, _: &mut Document, a: &Annot, q: &[[f32; 8]]) -> Stream {
        let c = a.color;
        let mut s = String::new();
        let mut res = Dictionary::new();
        match a.kind {
            Kind::Highlight(_) => {
                let mut gs = Dictionary::new();
                gs.set("Type", "ExtGState");
                gs.set("BM", "Multiply");
                gs.set("ca", a.opacity);
                gs.set("CA", a.opacity);
                let mut gsd = Dictionary::new();
                gsd.set("GS0", gs);
                res.set("ExtGState", gsd);
                s.push_str(&format!(
                    "/GS0 gs {} {} {} rg\n",
                    num(c[0]),
                    num(c[1]),
                    num(c[2])
                ));
                for q in q {
                    s.push_str(&format!(
                        "{} {} m {} {} l {} {} l {} {} l h f\n",
                        num(q[4]),
                        num(q[5]),
                        num(q[6]),
                        num(q[7]),
                        num(q[2]),
                        num(q[3]),
                        num(q[0]),
                        num(q[1])
                    ));
                }
            }
            _ => {
                s.push_str(&format!("{} {} {} RG\n", num(c[0]), num(c[1]), num(c[2])));
                let strike = matches!(a.kind, Kind::StrikeOut(_));
                for q in q {
                    // Height of the quad (upper left to lower left).
                    let h = ((q[0] - q[4]).powi(2) + (q[1] - q[5]).powi(2)).sqrt();
                    let w = (h / 14.0).max(0.5);
                    let t = if strike {
                        0.5
                    } else {
                        w / h.max(1e-3) / 2.0 + 0.04
                    };
                    let lerp = |a: f32, b: f32| a + (b - a) * t;
                    // From the lower edge towards the upper.
                    let (x0, y0) = (lerp(q[4], q[0]), lerp(q[5], q[1]));
                    let (x1, y1) = (lerp(q[6], q[2]), lerp(q[7], q[3]));
                    s.push_str(&format!(
                        "{} w {} {} m {} {} l S\n",
                        num(w),
                        num(x0),
                        num(y0),
                        num(x1),
                        num(y1)
                    ));
                }
            }
        }
        form_xobject(a.bounds(), s, res)
    }
}

fn form_xobject(bbox: [f32; 4], content: String, resources: Dictionary) -> Stream {
    let mut d = Dictionary::new();
    d.set("Type", "XObject");
    d.set("Subtype", "Form");
    d.set("BBox", reals(bbox));
    d.set("Resources", resources);
    let mut s = Stream::new(d, content.into_bytes());
    let _ = s.compress();
    s
}

fn opacity_gs(res: &mut Dictionary, opacity: f32) -> &'static str {
    if opacity >= 1.0 {
        return "";
    }
    let mut gs = Dictionary::new();
    gs.set("Type", "ExtGState");
    gs.set("ca", opacity);
    gs.set("CA", opacity);
    let mut gsd = Dictionary::new();
    gsd.set("GS0", gs);
    res.set("ExtGState", gsd);
    "/GS0 gs\n"
}

/// A speech bubble filling the note's box.
fn note_ap(a: &Annot) -> Stream {
    let r = a.rect;
    let (x0, y0, x1, y1) = (r[0] + 1.0, r[1] + 1.0, r[2] - 1.0, r[3] - 1.0);
    let h = y1 - y0;
    let tail = y0 + h * 0.25;
    let c = a.color;
    let mut res = Dictionary::new();
    let gs = opacity_gs(&mut res, a.opacity);
    let s = format!(
        "{gs}{} {} {} rg 0.25 0.25 0.25 RG 1 w 1 j\n\
         {} {} m {} {} l {} {} l {} {} l {} {} l {} {} l {} {} l h B\n\
         0.25 0.25 0.25 RG 1 w\n\
         {} {} m {} {} l S {} {} m {} {} l S {} {} m {} {} l S\n",
        num(c[0]),
        num(c[1]),
        num(c[2]),
        num(x0),
        num(y1),
        num(x1),
        num(y1),
        num(x1),
        num(tail),
        num(x0 + (x1 - x0) * 0.45),
        num(tail),
        num(x0 + (x1 - x0) * 0.25),
        num(y0),
        num(x0 + (x1 - x0) * 0.28),
        num(tail),
        num(x0),
        num(tail),
        // Three lines of "text".
        num(x0 + 3.0),
        num(y1 - h * 0.22),
        num(x1 - 3.0),
        num(y1 - h * 0.22),
        num(x0 + 3.0),
        num(y1 - h * 0.4),
        num(x1 - 3.0),
        num(y1 - h * 0.4),
        num(x0 + 3.0),
        num(y1 - h * 0.58),
        num(x1 - 6.0),
        num(y1 - h * 0.58),
    );
    form_xobject(r, s, res)
}

/// Break `text` into lines that fit `width` at `size`.
pub fn wrap(text: &str, font: Font, size: f32, width: f32) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let candidate = if line.is_empty() {
                word.to_owned()
            } else {
                format!("{line} {word}")
            };
            if font.width(&candidate, size) <= width || line.is_empty() {
                line = candidate;
                // A single word longer than the box: break it by characters.
                while font.width(&line, size) > width && line.chars().count() > 1 {
                    let mut cut = String::new();
                    let mut rest = String::new();
                    for ch in line.chars() {
                        if rest.is_empty() && font.width(&format!("{cut}{ch}"), size) <= width {
                            cut.push(ch);
                        } else {
                            rest.push(ch);
                        }
                    }
                    if cut.is_empty() {
                        break;
                    }
                    out.push(cut);
                    line = rest;
                }
            } else {
                out.push(std::mem::take(&mut line));
                line = word.to_owned();
            }
        }
        out.push(line);
    }
    out
}

/// A string for a Tj operator in WinAnsi (characters it lacks become `?`).
pub(crate) fn tj_string(s: &str) -> String {
    let mut out = String::from("(");
    for c in s.chars() {
        let b = crate::overlay::winansi_code(c).unwrap_or(b'?');
        match b {
            b'(' | b')' | b'\\' => {
                out.push('\\');
                out.push(b as char);
            }
            32..=126 => out.push(b as char),
            _ => out.push_str(&format!("\\{:03o}", b)),
        }
    }
    out.push(')');
    out
}

fn freetext_ap(a: &Annot, font: Font, size: f32, font_id: ObjectId) -> Stream {
    let r = a.rect;
    let mut res = Dictionary::new();
    let gs = opacity_gs(&mut res, a.opacity);
    let mut fonts = Dictionary::new();
    fonts.set(font.resource(), font_id);
    res.set("Font", fonts);
    let mut s = String::from(gs);
    if let Some(f) = a.fill {
        s.push_str(&format!(
            "{} {} {} rg {} {} {} {} re f\n",
            num(f[0]),
            num(f[1]),
            num(f[2]),
            num(r[0]),
            num(r[1]),
            num(r[2] - r[0]),
            num(r[3] - r[1])
        ));
    }
    let c = a.color;
    if a.width > 0.0 {
        let w = a.width;
        s.push_str(&format!(
            "{} {} {} RG {} w {} {} {} {} re S\n",
            num(c[0]),
            num(c[1]),
            num(c[2]),
            num(w),
            num(r[0] + w / 2.0),
            num(r[1] + w / 2.0),
            num(r[2] - r[0] - w),
            num(r[3] - r[1] - w)
        ));
    }
    let pad = 2.0 + a.width;
    let lines = wrap(&a.contents, font, size, (r[2] - r[0] - 2.0 * pad).max(1.0));
    let lead = size * 1.17;
    s.push_str(&format!(
        "{} {} {} {} re W n\nBT /{} {} Tf {} {} {} rg {} TL\n",
        num(r[0]),
        num(r[1]),
        num(r[2] - r[0]),
        num(r[3] - r[1]),
        font.resource(),
        num(size),
        num(c[0]),
        num(c[1]),
        num(c[2]),
        num(lead)
    ));
    s.push_str(&format!(
        "{} {} Td\n",
        num(r[0] + pad),
        num(r[3] - pad - size * 0.9)
    ));
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            s.push_str("T* ");
        }
        s.push_str(&tj_string(l));
        s.push_str(" Tj\n");
    }
    s.push_str("ET\n");
    form_xobject(r, s, res)
}

/// Bézier circle constant.
const KAPPA: f32 = 0.552_284_8;

fn ellipse_path(r: [f32; 4]) -> String {
    let (cx, cy) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    let (rx, ry) = ((r[2] - r[0]) / 2.0, (r[3] - r[1]) / 2.0);
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    format!(
        "{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c h\n",
        num(cx + rx),
        num(cy),
        num(cx + rx),
        num(cy + ky),
        num(cx + kx),
        num(cy + ry),
        num(cx),
        num(cy + ry),
        num(cx - kx),
        num(cy + ry),
        num(cx - rx),
        num(cy + ky),
        num(cx - rx),
        num(cy),
        num(cx - rx),
        num(cy - ky),
        num(cx - kx),
        num(cy - ry),
        num(cx),
        num(cy - ry),
        num(cx + kx),
        num(cy - ry),
        num(cx + rx),
        num(cy - ky),
        num(cx + rx),
        num(cy),
    )
}

/// Ink, rectangles, ellipses, lines and polygons.
fn shape_ap(a: &Annot) -> Stream {
    let c = a.color;
    let w = a.width.max(0.0);
    let mut res = Dictionary::new();
    let mut s = String::from(opacity_gs(&mut res, a.opacity));
    s.push_str(&format!(
        "{} {} {} RG {} w 1 J 1 j\n",
        num(c[0]),
        num(c[1]),
        num(c[2]),
        num(w)
    ));
    if let Some(f) = a.fill {
        s.push_str(&format!("{} {} {} rg\n", num(f[0]), num(f[1]), num(f[2])));
    }
    let paint = if a.fill.is_some() { "B" } else { "S" };
    let r = a.rect;
    let h = w / 2.0;
    match &a.kind {
        Kind::Ink(strokes) => {
            for st in strokes {
                let Some(first) = st.first() else { continue };
                s.push_str(&format!("{} {} m\n", num(first[0]), num(first[1])));
                if st.len() == 1 {
                    s.push_str(&format!("{} {} l\n", num(first[0] + 0.01), num(first[1])));
                }
                // Smooth: quadratic curves through midpoints.
                for i in 1..st.len() {
                    let p = st[i];
                    if i + 1 < st.len() {
                        let n = st[i + 1];
                        let (mx, my) = ((p[0] + n[0]) / 2.0, (p[1] + n[1]) / 2.0);
                        let prev = st[i - 1];
                        let start = if i == 1 {
                            prev
                        } else {
                            [(prev[0] + p[0]) / 2.0, (prev[1] + p[1]) / 2.0]
                        };
                        // Quadratic (start, p, mid) as cubic.
                        let c1 = [
                            start[0] + 2.0 / 3.0 * (p[0] - start[0]),
                            start[1] + 2.0 / 3.0 * (p[1] - start[1]),
                        ];
                        let c2 = [mx + 2.0 / 3.0 * (p[0] - mx), my + 2.0 / 3.0 * (p[1] - my)];
                        s.push_str(&format!(
                            "{} {} {} {} {} {} c\n",
                            num(c1[0]),
                            num(c1[1]),
                            num(c2[0]),
                            num(c2[1]),
                            num(mx),
                            num(my)
                        ));
                    } else {
                        s.push_str(&format!("{} {} l\n", num(p[0]), num(p[1])));
                    }
                }
                s.push_str("S\n");
            }
        }
        Kind::Square => s.push_str(&format!(
            "{} {} {} {} re {paint}\n",
            num(r[0] + h),
            num(r[1] + h),
            num(r[2] - r[0] - w),
            num(r[3] - r[1] - w)
        )),
        Kind::Circle => {
            s.push_str(&ellipse_path([r[0] + h, r[1] + h, r[2] - h, r[3] - h]));
            s.push_str(paint);
            s.push('\n');
        }
        Kind::Polygon(v) => {
            for (i, p) in v.iter().enumerate() {
                s.push_str(&format!(
                    "{} {} {}\n",
                    num(p[0]),
                    num(p[1]),
                    if i == 0 { "m" } else { "l" }
                ));
            }
            s.push_str(&format!("h {paint}\n"));
        }
        Kind::Line { a: p, b: q, arrow } => {
            s.push_str(&format!(
                "{} {} m {} {} l S\n",
                num(p[0]),
                num(p[1]),
                num(q[0]),
                num(q[1])
            ));
            if *arrow {
                let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
                let len = (dx * dx + dy * dy).sqrt().max(1e-3);
                let (ux, uy) = (dx / len, dy / len);
                let size = (w * 4.0).max(8.0);
                let (bx, by) = (q[0] - ux * size, q[1] - uy * size);
                let (nx, ny) = (-uy * size * 0.5, ux * size * 0.5);
                s.push_str(&format!(
                    "{} {} m {} {} l {} {} l S\n",
                    num(bx + nx),
                    num(by + ny),
                    num(q[0]),
                    num(q[1]),
                    num(bx - nx),
                    num(by - ny)
                ));
            }
        }
        _ => {}
    }
    form_xobject(a.rect, s, res)
}

/// The box a line, polygon or ink needs (with room for the pen and an
/// arrowhead).
pub fn fit_rect(kind: &Kind, width: f32) -> Option<[f32; 4]> {
    let pts: Vec<[f32; 2]> = match kind {
        Kind::Ink(s) => s.iter().flatten().copied().collect(),
        Kind::Line { a, b, .. } => vec![*a, *b],
        Kind::Polygon(v) => v.clone(),
        _ => return None,
    };
    let first = pts.first()?;
    let mut r = [first[0], first[1], first[0], first[1]];
    for p in &pts {
        r = [
            r[0].min(p[0]),
            r[1].min(p[1]),
            r[2].max(p[0]),
            r[3].max(p[1]),
        ];
    }
    let m = match kind {
        Kind::Line { arrow: true, .. } => (width * 4.0).max(8.0),
        _ => width / 2.0 + 1.0,
    };
    Some([r[0] - m, r[1] - m, r[2] + m, r[3] + m])
}

fn stamp_ap(
    doc: &mut Document,
    a: &Annot,
    art: &Art,
    rotate: f32,
    old: Option<Stream>,
) -> Result<Stream> {
    let r = a.rect;
    let (w, h) = ((r[2] - r[0]).max(0.1), (r[3] - r[1]).max(0.1));
    let mut res = Dictionary::new();
    let mut s = String::from(opacity_gs(&mut res, a.opacity));
    match art {
        Art::Image(bytes) => {
            let img = crate::images::embed(bytes, "stamp")?;
            let mut xo = img.xobject;
            if let Some(mask) = img.smask {
                let id = doc.add_object(mask);
                xo.dict.set("SMask", id);
            }
            let id = doc.add_object(xo);
            let mut xd = Dictionary::new();
            xd.set("Im0", id);
            res.set("XObject", xd);
            let m = crate::images::placement(img.exif, 0.0, 0.0, w, h);
            s.push_str(&format!(
                "q {} {} {} {} {} {} cm /Im0 Do Q\n",
                num(m[0]),
                num(m[1]),
                num(m[2]),
                num(m[3]),
                num(m[4]),
                num(m[5])
            ));
        }
        Art::Paths(segs) => {
            s.push_str(&format!(
                "{} {} {} rg\n",
                num(a.color[0]),
                num(a.color[1]),
                num(a.color[2])
            ));
            for seg in segs.iter() {
                match *seg {
                    Seg::Move(x, y) => s.push_str(&format!("{} {} m\n", num(x * w), num(y * h))),
                    Seg::Line(x, y) => s.push_str(&format!("{} {} l\n", num(x * w), num(y * h))),
                    Seg::Curve(a1, b1, a2, b2, x, y) => s.push_str(&format!(
                        "{} {} {} {} {} {} c\n",
                        num(a1 * w),
                        num(b1 * h),
                        num(a2 * w),
                        num(b2 * h),
                        num(x * w),
                        num(y * h)
                    )),
                    Seg::Close => s.push_str("h\n"),
                }
            }
            s.push_str("f\n");
        }
        Art::Keep => {
            let Some(old) = old else {
                return Err(Error::Invalid("the stamp has no appearance to keep".into()));
            };
            // The old appearance as a form, fitted into the box.
            let bbox = old
                .dict
                .get(b"BBox")
                .ok()
                .and_then(|o| rect(doc, o))
                .unwrap_or([0.0, 0.0, w, h]);
            let mut inner = old.clone();
            // Its own turn is replaced by ours.
            inner.dict.remove(b"Matrix");
            let id = doc.add_object(inner);
            let mut xd = Dictionary::new();
            xd.set("Fm0", id);
            res.set("XObject", xd);
            let (bw, bh) = ((bbox[2] - bbox[0]).max(0.1), (bbox[3] - bbox[1]).max(0.1));
            s.push_str(&format!(
                "q {} 0 0 {} {} {} cm /Fm0 Do Q\n",
                num(w / bw),
                num(h / bh),
                num(-bbox[0] * w / bw),
                num(-bbox[1] * h / bh)
            ));
        }
    }
    let mut d = Dictionary::new();
    d.set("Type", "XObject");
    d.set("Subtype", "Form");
    d.set("BBox", reals([0.0, 0.0, w, h]));
    if rotate.rem_euclid(360.0) != 0.0 {
        let (sn, cs) = rotate.to_radians().sin_cos();
        d.set("Matrix", reals([cs, sn, -sn, cs, 0.0, 0.0]));
    }
    d.set("Resources", res);
    let mut st = Stream::new(d, s.into_bytes());
    let _ = st.compress();
    Ok(st)
}

// ── applying edits ─────────────────────────────────────────────────────────

/// Replace the annotations of page `page` (0-based) with `marks`: entries
/// from the file not changed keep their object; changed ones are written
/// over their dictionary; new ones are added (notes with a pop-up).
/// Annotations of the page missing from `marks` are removed, with their
/// pop-ups.
pub fn apply_page(pdf: &mut Pdf, page: u32, marks: &[Annot]) -> Result<()> {
    let doc = &mut pdf.doc;
    let pid = *page_ids(doc)
        .get(page as usize)
        .ok_or_else(|| Error::Invalid(format!("no page {}", page + 1)))?;
    let originals = page_annots(doc, pid);
    // Popups by their parent's place.
    let mut popups: HashMap<usize, (Option<ObjectId>, Dictionary)> = HashMap::new();
    for (id, d) in &originals {
        if name(d, b"Subtype") == Some(b"Popup")
            && let Ok(Object::Reference(parent)) = d.get(b"Parent")
            && let Some(i) = originals.iter().position(|(pid, _)| *pid == Some(*parent))
        {
            popups.insert(i, (*id, d.clone()));
        }
    }
    // Object ids for every mark first (replies refer to them).
    let mut ids: Vec<ObjectId> = Vec::with_capacity(marks.len());
    for m in marks {
        let id = match m.orig.and_then(|i| originals.get(i)).and_then(|o| o.0) {
            Some(id) => id,
            None => doc.new_object_id(),
        };
        ids.push(id);
    }
    let by_name: HashMap<&str, ObjectId> = marks
        .iter()
        .zip(&ids)
        .map(|(m, id)| (m.id.as_str(), *id))
        .collect();
    let mut w = Writer::new();
    let mut out: Vec<Object> = Vec::new();
    for (m, &id) in marks.iter().zip(&ids) {
        let original = m.orig.and_then(|i| originals.get(i));
        match original {
            Some((Some(_), _)) if !m.dirty => {}
            Some((None, d)) if !m.dirty => {
                doc.objects.insert(id, Object::Dictionary(d.clone()));
            }
            _ => {
                let parent = m.reply_to.as_deref().and_then(|n| by_name.get(n).copied());
                let d = w.write(doc, pid, original.map(|o| o.1.clone()), m, parent)?;
                doc.objects.insert(id, Object::Dictionary(d));
            }
        }
        out.push(Object::Reference(id));
        // Its pop-up.
        if let Some((pop_id, pop)) = m.orig.and_then(|i| popups.get(&i)) {
            let pop_id = match pop_id {
                Some(p) => *p,
                None => doc.add_object(pop.clone()),
            };
            out.push(Object::Reference(pop_id));
        } else if m.orig.is_none() && matches!(m.kind, Kind::Note) && m.reply_to.is_none() {
            let r = m.rect;
            let mut pop = Dictionary::new();
            pop.set("Type", "Annot");
            pop.set("Subtype", "Popup");
            pop.set("Parent", id);
            pop.set("P", pid);
            pop.set("Open", false);
            pop.set("F", F_PRINT | F_NOZOOM | F_NOROTATE);
            pop.set(
                "Rect",
                reals([r[2] + 10.0, r[3] - 120.0, r[2] + 210.0, r[3]]),
            );
            let pop_id = doc.add_object(pop);
            if let Ok(d) = doc.get_dictionary_mut(id) {
                d.set("Popup", pop_id);
            }
            out.push(Object::Reference(pop_id));
        }
    }
    let page = doc.get_dictionary_mut(pid)?;
    if out.is_empty() {
        page.remove(b"Annots");
    } else {
        page.set("Annots", out);
    }
    Ok(())
}

/// A one-page PDF the size of page `page` with only its annotations as
/// `marks` say (form widgets with `fields`' values), for drawing them over
/// the page while it is edited.
pub fn layer(
    pdf: &Pdf,
    page: u32,
    marks: &[Annot],
    fields: &HashMap<String, FieldValue>,
) -> Result<Vec<u8>> {
    let src = &pdf.doc;
    let pid = *page_ids(src)
        .get(page as usize)
        .ok_or_else(|| Error::Invalid(format!("no page {}", page + 1)))?;
    let originals = page_annots(src, pid);
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let new_page = doc.new_object_id();
    let mut map = HashMap::new();
    let mut annots = Vec::new();
    let mut w = Writer::new();
    let widgets = form::page_widgets(pdf, page);
    for m in marks {
        let original = m.orig.and_then(|i| originals.get(i));
        let base = original.map(|(_, d)| {
            match copy_object(src, &mut doc, &Object::Dictionary(d.clone()), &mut map) {
                Object::Dictionary(d) => d,
                _ => Dictionary::new(),
            }
        });
        let mut d = if m.dirty || original.is_none() {
            w.write(&mut doc, new_page, base, m, None)?
        } else {
            base.unwrap_or_default()
        };
        if let Some(i) = m.orig
            && let Some(wd) = widgets.iter().find(|w| w.annot_index == i)
            && let Some(v) = fields.get(&wd.field)
        {
            form::widget_look(&mut doc, wd, v, &mut d)?;
        }
        annots.push(Object::Reference(doc.add_object(d)));
    }
    let mut p = Dictionary::new();
    p.set("Type", "Page");
    p.set("Parent", pages_id);
    for key in [&b"MediaBox"[..], b"CropBox", b"Rotate"] {
        if let Some(v) = inherited(src, pid, key)
            && let Ok((_, v)) = src.dereference(&v)
        {
            p.set(key.to_vec(), v.clone());
        }
    }
    if !p.has(b"MediaBox") {
        p.set("MediaBox", reals([0.0, 0.0, 612.0, 792.0]));
    }
    p.set("Resources", Dictionary::new());
    p.set("Annots", annots);
    doc.objects.insert(new_page, Object::Dictionary(p));
    let mut tree = Dictionary::new();
    tree.set("Type", "Pages");
    tree.set("Kids", vec![Object::Reference(new_page)]);
    tree.set("Count", 1);
    doc.objects.insert(pages_id, Object::Dictionary(tree));
    let mut cat = Dictionary::new();
    cat.set("Type", "Catalog");
    cat.set("Pages", pages_id);
    let cat_id = doc.add_object(cat);
    doc.trailer.set("Root", cat_id);
    let mut out = Vec::new();
    doc.save_to(&mut out)
        .map_err(|e| Error::Pdf(e.to_string()))?;
    Ok(out)
}

/// Which annotations [`flatten`] draws into the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlattenWhat {
    /// Image stamps and Fill & Sign signatures.
    StampsAndSignatures,
    /// Every annotation with an appearance (comments, markup, drawings,
    /// stamps, signatures), with their pop-ups; links, form fields and
    /// media (attachments, sounds, movies, 3-D) stay.
    Annotations,
}

/// Draw stamps and signatures (or every annotation, as `what` says) into
/// their pages' content (they can't be moved or deleted afterwards) and
/// remove them as annotations. Returns how many were flattened.
pub fn flatten(pdf: &mut Pdf, what: FlattenWhat) -> Result<u32> {
    let mut count = 0;
    let pages = page_ids(&pdf.doc);
    for pid in pages {
        let list = page_annots(&pdf.doc, pid);
        let mut keep = Vec::new();
        let mut draw = String::new();
        let mut xobjects = Dictionary::new();
        let mut flat: Vec<ObjectId> = Vec::new();
        for (id, d) in &list {
            let sub = name(d, b"Subtype").unwrap_or(b"");
            let ink_sig =
                sub == b"Ink" && get_text(&pdf.doc, d, b"Subj").is_some_and(|s| s == "Signature");
            let is_target = match what {
                FlattenWhat::StampsAndSignatures => sub == b"Stamp" || ink_sig,
                FlattenWhat::Annotations => !matches!(
                    sub,
                    b"Link"
                        | b"Popup"
                        | b"Widget"
                        | b"FileAttachment"
                        | b"Sound"
                        | b"Movie"
                        | b"Screen"
                        | b"3D"
                        | b"RichMedia"
                        | b"PrinterMark"
                        | b"TrapNet"
                ),
            };
            let ap_id = d
                .get(b"AP")
                .ok()
                .and_then(|o| pdf.doc.dereference(o).ok())
                .and_then(|(_, o)| o.as_dict().ok())
                .and_then(|ap| ap.get(b"N").ok())
                .and_then(|n| n.as_reference().ok());
            let hidden = d.get(b"F").ok().and_then(|f| f.as_i64().ok()).unwrap_or(0)
                & (F_HIDDEN | F_NOVIEW)
                != 0;
            let (true, Some(ap_id), false) = (is_target, ap_id, hidden) else {
                if let Some(id) = id {
                    keep.push(Object::Reference(*id));
                } else {
                    keep.push(Object::Dictionary(d.clone()));
                }
                continue;
            };
            let Some(r) = d.get(b"Rect").ok().and_then(|o| rect(&pdf.doc, o)) else {
                continue;
            };
            let Ok(Object::Stream(ap)) = pdf.doc.get_object(ap_id) else {
                continue;
            };
            let bbox = ap
                .dict
                .get(b"BBox")
                .ok()
                .and_then(|o| rect(&pdf.doc, o))
                .unwrap_or(r);
            let m = nums(&pdf.doc, &ap.dict, b"Matrix");
            let m = if m.len() == 6 {
                [m[0], m[1], m[2], m[3], m[4], m[5]]
            } else {
                [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
            };
            // The appearance's box after its matrix, then fitted to Rect (§12.5.5).
            let corners = [
                (bbox[0], bbox[1]),
                (bbox[2], bbox[1]),
                (bbox[0], bbox[3]),
                (bbox[2], bbox[3]),
            ]
            .map(|(x, y)| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]));
            let tx0 = corners.iter().map(|c| c.0).fold(f32::MAX, f32::min);
            let ty0 = corners.iter().map(|c| c.1).fold(f32::MAX, f32::min);
            let tx1 = corners.iter().map(|c| c.0).fold(f32::MIN, f32::max);
            let ty1 = corners.iter().map(|c| c.1).fold(f32::MIN, f32::max);
            let sx = (r[2] - r[0]) / (tx1 - tx0).max(1e-3);
            let sy = (r[3] - r[1]) / (ty1 - ty0).max(1e-3);
            let a = [sx, 0.0, 0.0, sy, r[0] - sx * tx0, r[1] - sy * ty0];
            if let Some(id) = id {
                flat.push(*id);
            }
            let key = format!("KFl{count}");
            xobjects.set(key.clone(), ap_id);
            draw.push_str(&format!(
                "q {} {} {} {} {} {} cm /{key} Do Q\n",
                num(a[0]),
                num(a[1]),
                num(a[2]),
                num(a[3]),
                num(a[4]),
                num(a[5])
            ));
            count += 1;
        }
        if draw.is_empty() {
            continue;
        }
        // Pop-ups of flattened annotations go with them.
        keep.retain(|o| {
            let d = match o {
                Object::Reference(r) => pdf.doc.get_dictionary(*r).ok(),
                Object::Dictionary(d) => Some(d),
                _ => None,
            };
            !d.is_some_and(|d| {
                name(d, b"Subtype") == Some(b"Popup")
                    && d.get(b"Parent")
                        .and_then(Object::as_reference)
                        .is_ok_and(|p| flat.contains(&p))
            })
        });
        crate::pages::materialize(&mut pdf.doc, pid)?;
        // The form keeps its own matrix; Do applies it inside our cm.
        add_to_content(&mut pdf.doc, pid, draw, xobjects)?;
        let page = pdf.doc.get_dictionary_mut(pid)?;
        if keep.is_empty() {
            page.remove(b"Annots");
        } else {
            page.set("Annots", keep);
        }
    }
    Ok(count)
}

/// Append drawing `ops` (using `xobjects`) after the page's own content,
/// which is wrapped in q/Q so its state can't leak.
fn add_to_content(
    doc: &mut Document,
    page: ObjectId,
    ops: String,
    xobjects: Dictionary,
) -> Result<()> {
    let res_obj = doc
        .get_dictionary(page)?
        .get(b"Resources")
        .cloned()
        .unwrap_or(Object::Dictionary(Dictionary::new()));
    let mut res = match doc.dereference(&res_obj) {
        Ok((_, Object::Dictionary(d))) => d.clone(),
        _ => Dictionary::new(),
    };
    let mut xo = match res.get(b"XObject").ok().map(|o| doc.dereference(o)) {
        Some(Ok((_, Object::Dictionary(d)))) => d.clone(),
        _ => Dictionary::new(),
    };
    for (k, v) in xobjects.iter() {
        xo.set(k.clone(), v.clone());
    }
    res.set("XObject", xo);
    let save = doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
    let tail = doc.add_object(Stream::new(
        Dictionary::new(),
        format!("Q\n{ops}").into_bytes(),
    ));
    let contents = doc.get_dictionary(page)?.get(b"Contents").cloned().ok();
    let mut parts = vec![Object::Reference(save)];
    match contents.map(|c| doc.dereference(&c).map(|(_, o)| o.clone())) {
        Some(Ok(Object::Array(a))) => parts.extend(a),
        Some(Ok(Object::Stream(_))) => {
            if let Ok(Object::Reference(id)) = doc.get_dictionary(page)?.get(b"Contents") {
                parts.push(Object::Reference(*id));
            }
        }
        _ => {}
    }
    parts.push(Object::Reference(tail));
    let p = doc.get_dictionary_mut(page)?;
    p.set("Resources", res);
    p.set("Contents", parts);
    Ok(())
}
