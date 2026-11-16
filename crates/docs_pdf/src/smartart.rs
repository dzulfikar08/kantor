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
        (f.read.cx.max(1), f.read.cy.max(1)),
    )?;
    Some(SmartArt {
        data: data.clone(),
        data_xml,
        drawing,
    })
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
    let k = (
        rect[2] / f.read.cx.max(1) as f32,
        rect[3] / f.read.cy.max(1) as f32,
    );
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
