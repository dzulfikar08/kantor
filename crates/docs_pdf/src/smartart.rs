//! SmartArt in floating graphic frames (`dgm:relIds`), as Word shows it:
//! from the pre-drawn shapes it saves in the drawing part (`dsp:drawing`),
//! read with Slides' SmartArt code (`slides_engine::smartart`). Shapes are
//! drawn like a group's ([`Painter`]); their text is laid out with the
//! Docs engine from Slides' resolved text. Editing a shape's text writes
//! the drawing and the data model, as Slides does, into the object's
//! edited parts ([`docs_engine::Float::parts`]).

use std::sync::Arc;

use docs_engine::object::EMU_PER_PX;
use docs_engine::{Document, Float, FloatKind};
use docs_layout::{PageSetup, PagedDocument};
use slides_engine::frame::DiagramDrawing;
use slides_engine::theme::ListStyle;
use slides_engine::{Anchor, Xfrm};

use crate::floats::{BoxText, FloatPrim, Painter};
use crate::pages::{LayoutFn, StoryLayout};

/// A SmartArt's data model part (name, XML) and drawing.
pub struct SmartArt {
    pub data: String,
    pub data_xml: String,
    /// The drawing part's XML (as edited, or as read).
    pub drawing_xml: String,
    pub drawing: DiagramDrawing,
}

/// A part's XML as `doc` has it now (edited with `f`, or as read).
fn current(doc: &Document, f: &Float, name: &str) -> Option<String> {
    f.parts
        .iter()
        .find(|p| p.0 == name)
        .map(|p| p.1.to_string())
        .or_else(|| doc.current_part_xml(name))
}

/// The SmartArt object `f` shows (`doc` is the main document), if its
/// file has a drawing for it.
pub fn read(doc: &Document, f: &Float, painter: &Painter) -> Option<SmartArt> {
    let FloatKind::SmartArt { data, drawings } = &f.kind else {
        return None;
    };
    let data_xml = current(doc, f, data)?;
    let by_rel = slides_engine::smartart::drawing_rel_id(&data_xml)
        .and_then(|id| drawings.iter().find(|d| d.0 == id));
    let part = match by_rel {
        Some(d) => &d.1,
        None if drawings.len() == 1 => &drawings[0].1,
        None => return None,
    };
    let xml = current(doc, f, part)?;
    let drawing = slides_engine::smartart::read_drawing(
        part,
        &xml,
        &[],
        data,
        &data_xml,
        &painter.ctx(),
        design_size(f, &xml, &data_xml),
    )?;
    Some(SmartArt {
        data: data.clone(),
        data_xml,
        drawing_xml: xml,
        drawing,
    })
}

/// The frame size (EMU) a SmartArt's drawing is drawn for: the object's
/// size for layouts Kantor lays out (they're laid out again when resized),
/// else the size it was read at (it scales).
fn design_size(f: &Float, drawing: &str, data: &str) -> (i64, i64) {
    if slides_engine::smartart_layout::family(drawing, data).is_some() {
        (f.at.cx.max(1), f.at.cy.max(1))
    } else {
        (f.read.cx.max(1), f.read.cy.max(1))
    }
}

/// A SmartArt's drawing at `rect`: its shapes, and their text laid out.
pub struct SmartArtDraw {
    pub prims: Vec<FloatPrim>,
    pub texts: Vec<BoxText>,
}

/// What SmartArt object `f` at `rect` draws (`anchor`: its anchor's
/// story, paragraph and byte index).
pub fn draw(
    doc: &Document,
    f: &Float,
    anchor: (Option<usize>, usize, usize),
    rect: [f32; 4],
    painter: &Painter,
    layout: &mut LayoutFn,
) -> Option<SmartArtDraw> {
    let sa = read(doc, f, painter)?;
    let dd = &sa.drawing;
    let (w, h) = design_size(f, &sa.drawing_xml, &sa.data_xml);
    let k = (rect[2] / w as f32, rect[3] / h as f32);
    let prims = painter.smartart_prims(&dd.drawing_with_edits(&[]), rect, k);
    let base = ListStyle::default();
    let minor = painter.minor_font();
    let mut texts = Vec::new();
    for (i, shape) in dd.group.children.iter().enumerate() {
        let Some(xfrm) = shape.xfrm else { continue };
        let own_box = dd.group.text_boxes.get(i).copied().flatten();
        let rt = match own_box {
            Some(_) => slides_engine::render::free_shape_text_boxed(shape, &base, minor),
            None => slides_engine::render::free_shape_text(shape, &base, minor, &xfrm),
        };
        // An empty item still takes a place (double-click to type in it).
        let rt = match rt {
            Some(rt) => rt,
            None if shape.text.is_some() => slides_engine::RenderText {
                paras: Vec::new(),
                insets: [0; 4],
                anchor: Anchor::Middle,
                wrap: true,
                prompt: false,
                vert: Default::default(),
            },
            None => continue,
        };
        let bx: Xfrm = own_box.unwrap_or(xfrm);
        let b = [
            rect[0] + bx.x as f32 * k.0,
            rect[1] + bx.y as f32 * k.1,
            bx.cx as f32 * k.0,
            bx.cy as f32 * k.1,
        ];
        let [l, t, r, bt] = rt.insets.map(|v| v.max(0) as f32 / EMU_PER_PX);
        let width = (b[2] - l - r).max(1.0);
        let room = (b[3] - t - bt).max(0.0);
        let mut paras = slides_scene::text::layout_paragraphs(&rt);
        if paras.is_empty() {
            paras.push(docs_layout::Paragraph::new(String::new()));
        }
        let layouts = paras.iter().map(|p| layout(p, width)).collect();
        let paged = PagedDocument::paginate(&paras, layouts, setup(width));
        let height = paged
            .pages
            .first()
            .and_then(|p| p.lines.last())
            .map_or(0.0, |l| l.top + l.height);
        let dy = match rt.anchor {
            Anchor::Top => 0.0,
            Anchor::Middle => (room - height) / 2.0,
            Anchor::Bottom => room - height,
        };
        let story = StoryLayout {
            story: 0,
            note: None,
            text_box: Some(anchor),
            group_box: Some(i),
            paged,
            paras: paras
                .iter()
                .map(|p| docs_engine::Paragraph::new(p.text.clone()))
                .collect(),
            height,
            strikes: Arc::default(),
        };
        texts.push(BoxText {
            which: i,
            layout: Arc::new(story),
            x: b[0] + l,
            y: b[1] + t + dy,
            rect: b,
            turn: bx.angle() as f32,
        });
    }
    Some(SmartArtDraw { prims, texts })
}

fn setup(width: f32) -> PageSetup {
    PageSetup {
        width: width.max(1.0),
        height: 1.0e7,
        margin_top: 0.0,
        margin_bottom: 0.0,
        margin_left: 0.0,
        margin_right: 0.0,
    }
}

/// The text of SmartArt shape `k` (`\n` between paragraphs) and, when it
/// can't be edited, why.
pub fn shape_text(doc: &Document, f: &Float, k: usize) -> Option<(String, Option<&'static str>)> {
    let sa = read(doc, f, &Painter::new(doc))?;
    let shape = sa.drawing.group.children.get(k)?;
    let text = shape
        .text
        .as_ref()
        .map(|t| {
            t.paras
                .iter()
                .map(|p| p.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let locked = sa.drawing.fixed_paras(k).err();
    Some((text, locked))
}

/// Give SmartArt shape `k` of `f` the text `text`: the edited drawing and
/// data model parts to store with the object ([`Document::set_float_parts`]),
/// `None` when nothing changed.
#[allow(clippy::type_complexity)]
pub fn set_shape_text(
    doc: &Document,
    f: &Float,
    k: usize,
    text: &str,
) -> Result<Option<Vec<(String, Arc<str>)>>, &'static str> {
    let painter = Painter::new(doc);
    let Some(mut sa) = read(doc, f, &painter) else {
        return Err("Kantor can't read this SmartArt's drawing");
    };
    if !sa.drawing.set_plain_text(k, text)? {
        return Ok(None);
    }
    let (drawing, data) = sa
        .drawing
        .saved_parts(&painter.ctx(), &sa.data, &sa.data_xml);
    let mut parts = vec![(sa.drawing.part.clone(), Arc::from(drawing))];
    if let Some(d) = data {
        parts.push((sa.data.clone(), Arc::from(d)));
    }
    Ok(Some(parts))
}

/// Edited parts: the drawing and data model.
pub type Parts = Vec<(String, Arc<str>)>;

/// The SmartArt `f`'s drawing part name and XML, data model XML, and the
/// model id of the item shape `k` shows (when `k` is given and shows one),
/// for a layout Kantor lays out.
fn laid_out(
    doc: &Document,
    f: &Float,
    k: Option<usize>,
) -> Result<(SmartArt, Option<String>), &'static str> {
    let Some(sa) = read(doc, f, &Painter::new(doc)) else {
        return Err("Kantor can't read this SmartArt's drawing");
    };
    if slides_engine::smartart_layout::family(&sa.drawing_xml, &sa.data_xml).is_none() {
        return Err(slides_engine::smartart_layout::LAYOUT_REFUSAL);
    }
    let id = k.and_then(|k| sa.drawing.group.model_ids.get(k).cloned().flatten());
    Ok((sa, id))
}

/// Kantor's layout of `f` (drawing XML for `data`, a `size` EMU frame) with
/// its text shrunk to fit the shapes, as Word's autofit does
/// ([`slides_engine::smartart_fit`]).
fn fitted(doc: &Document, sa: &SmartArt, drawing: String, data: &str, size: (i64, i64)) -> String {
    slides_engine::smartart_fit::set_text_measure(slides_scene::text::text_height);
    let painter = Painter::new(doc);
    let Some(dd) = slides_engine::smartart::read_drawing(
        &sa.drawing.part,
        &drawing,
        &[],
        &sa.data,
        data,
        &painter.ctx(),
        size,
    ) else {
        return drawing;
    };
    let base = ListStyle::default();
    let minor = painter.minor_font();
    let text = |shape: &slides_engine::Shape, x: &Xfrm, own: bool| match own {
        true => slides_engine::render::free_shape_text_boxed(shape, &base, minor),
        false => slides_engine::render::free_shape_text(shape, &base, minor, x),
    };
    slides_engine::smartart_fit::fit_text(&dd, &text).unwrap_or(drawing)
}

fn parts(sa: &SmartArt, drawing: String, data: Option<String>) -> Parts {
    let mut out = vec![(sa.drawing.part.clone(), Arc::from(drawing))];
    if let Some(d) = data {
        out.push((sa.data.clone(), Arc::from(d)));
    }
    out
}

/// Whether Kantor lays SmartArt `f` out (items can be added and removed,
/// and it's laid out again when resized).
pub fn can_relayout(doc: &Document, f: &Float) -> bool {
    laid_out(doc, f, None).is_ok()
}

/// Word's *Add Shape After* / *Before* on SmartArt shape `k`: a new empty
/// item next to the one it shows, the others laid out with it. The edited
/// parts to store with the object ([`Document::set_float_parts`]).
pub fn add_item(doc: &Document, f: &Float, k: usize, after: bool) -> Result<Parts, &'static str> {
    let (sa, id) = laid_out(doc, f, Some(k))?;
    let id = id.ok_or(SELECT_ITEM)?;
    let place = if after {
        slides_engine::AddPlace::After
    } else {
        slides_engine::AddPlace::Before
    };
    let (w, h) = (f.at.cx.max(1), f.at.cy.max(1));
    let (drawing, data, _) =
        slides_engine::smartart_layout::add_item(&sa.drawing_xml, &sa.data_xml, &id, place, w, h)
            .map_err(|e| known(&e).unwrap_or(SELECT_ITEM))?;
    let drawing = fitted(doc, &sa, drawing, &data, (w, h));
    Ok(parts(&sa, drawing, Some(data)))
}

/// Delete the item SmartArt shape `k` shows (with its sub-items); the
/// others are laid out again.
pub fn remove_item(doc: &Document, f: &Float, k: usize) -> Result<Parts, &'static str> {
    let (sa, id) = laid_out(doc, f, Some(k))?;
    let id = id.ok_or(SELECT_ITEM)?;
    let (w, h) = (f.at.cx.max(1), f.at.cy.max(1));
    let (drawing, data) =
        slides_engine::smartart_layout::remove_item(&sa.drawing_xml, &sa.data_xml, &id, w, h)
            .map_err(|e| {
                if e.to_string().contains("at least one") {
                    LAST_ITEM
                } else {
                    known(&e).unwrap_or(SELECT_ITEM)
                }
            })?;
    let drawing = fitted(doc, &sa, drawing, &data, (w, h));
    Ok(parts(&sa, drawing, Some(data)))
}

/// SmartArt `f` laid out again for a `size` (EMU) frame, when Kantor lays
/// its layout out: the edited parts (`None`: it scales).
pub fn relaid(doc: &Document, f: &Float, size: (i64, i64)) -> Option<Parts> {
    let (sa, _) = laid_out(doc, f, None).ok()?;
    let drawing =
        slides_engine::smartart_layout::relayout(&sa.drawing_xml, &sa.data_xml, size.0, size.1)
            .ok()?;
    let drawing = fitted(doc, &sa, drawing, &sa.data_xml, size);
    Some(parts(&sa, drawing, None))
}

/// A refusal Slides' layout explains itself (a Labeled Hierarchy's).
fn known(e: &anyhow::Error) -> Option<&'static str> {
    use slides_engine::smartart_layout::{LABEL_LEVELS, TOP_ALONE, TOP_KEEP};
    let msg = e.to_string();
    [LABEL_LEVELS, TOP_ALONE, TOP_KEEP]
        .into_iter()
        .find(|m| *m == msg)
}

pub const SELECT_ITEM: &str = "This SmartArt shape doesn't show an item";
pub const LAST_ITEM: &str = "A SmartArt keeps at least one item";
