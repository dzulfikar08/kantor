//! Painting a laid-out note: backgrounds, selection, text, decorations
//! (bullets, numbers, check boxes, quote bars, rules), tables, pictures and
//! the caret. Only the lines on screen are painted.

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::*;
use kantor_ui::tokens;

use crate::layout::{Body, Laid, Look, MarkerDeco, Mode, TextBox};
use crate::{NotesView, PAD_TOP, column};

/// Per visible line: display ranges to highlight.
#[derive(Default)]
struct LineMarks {
    selection: Vec<Range<usize>>,
    found: Vec<(Range<usize>, bool)>,
    caret: Option<usize>,
}

pub(crate) struct Frame {
    laid: Rc<Laid>,
    look: Look,
    visible: Range<usize>,
    marks: HashMap<usize, LineMarks>,
    images: HashMap<PathBuf, Arc<RenderImage>>,
    selection: Hsla,
    caret: Hsla,
    quote_bar: Hsla,
    check: Hsla,
    check_fg: Hsla,
    find: Hsla,
    find_current: Hsla,
    header_bg: Hsla,
    scroll_y: Pixels,
    zoom: f32,
    focus: FocusHandle,
    view: Entity<NotesView>,
}

impl Frame {
    pub(crate) fn new(
        v: &NotesView,
        visible: Range<usize>,
        images: HashMap<PathBuf, Arc<RenderImage>>,
        window: &Window,
        cx: &mut Context<NotesView>,
    ) -> Self {
        let t = tokens(cx);
        let look = v.look(cx);
        let buf = v.editor.buffer();
        let sel = v.editor.selection().range();
        let head = v.editor.selection().head;
        let focused = v.focus.is_focused(window);
        let mut marks: HashMap<usize, LineMarks> = HashMap::new();
        if v.laid.lines.len() == buf.line_count() {
            for i in visible.clone() {
                let Body::Text { text, .. } = &v.laid.lines[i].body else {
                    continue;
                };
                let start = buf.line_start(i);
                let full = buf.line_full_range(i);
                let m = marks.entry(i).or_default();
                // Selection: the part on this line (its line break too).
                if !sel.is_empty() && sel.start <= full.end && sel.end > full.start {
                    let a = text.map.to_display(sel.start.max(start) - start);
                    let b = text
                        .map
                        .to_display(sel.end.min(full.end).max(start) - start);
                    m.selection.push(a..b.max(a));
                    if sel.end > buf.line_range(i).end && b == a {
                        // An empty line inside the selection.
                        m.selection.push(a..a);
                    }
                }
                for (k, f) in v.found.iter().enumerate() {
                    if f.end < start || f.start > full.end {
                        continue;
                    }
                    let a = text.map.to_display(f.start.max(start) - start);
                    let b = text.map.to_display(f.end.min(full.end).max(start) - start);
                    m.found.push((a..b, v.found_current == Some(k)));
                }
                if focused && v.caret_shown && v.mode != Mode::Reading && buf.line_of(head) == i {
                    m.caret = Some(text.map.to_display(head - start));
                }
            }
        }
        Frame {
            laid: v.laid.clone(),
            look,
            visible,
            marks,
            images,
            selection: if focused { t.accent_soft } else { t.selected },
            caret: t.fg,
            quote_bar: t.border_strong,
            check: t.accent,
            check_fg: t.accent_fg,
            find: t.find_match,
            find_current: t.find_match_current,
            header_bg: t.surface,
            scroll_y: v.scroll_y,
            zoom: v.zoom,
            focus: v.focus.clone(),
            view: cx.entity(),
        }
    }
}

pub(crate) fn paint(f: &Frame, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let (left, width) = column(bounds.size.width, f.zoom);
    let origin = point(
        bounds.origin.x + left,
        bounds.origin.y + px(PAD_TOP) - f.scroll_y,
    );
    let look = &f.look;
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for i in f.visible.clone() {
            let Some(b) = f.laid.lines.get(i) else {
                continue;
            };
            let top = origin.y + f.laid.tops[i];
            let y = top + b.pad_top;
            match &b.body {
                Body::Hidden => {}
                Body::Rule => {
                    let mid = y + look.line_height() / 2.;
                    window.paint_quad(fill(
                        Bounds::new(point(origin.x, mid), size(width, px(1.))),
                        look.border,
                    ));
                }
                Body::Fence { label } => {
                    window.paint_quad(fill(
                        Bounds::new(point(origin.x, top), size(width, b.height)),
                        look.code_bg,
                    ));
                    if let Some(l) = label {
                        paint_text(l, point(origin.x, y), window, cx);
                    }
                }
                Body::Image(img) => {
                    if let Some(data) = f.images.get(&img.path) {
                        let r = Bounds::new(point(origin.x, y), img.size);
                        let _ =
                            window.paint_image(r, r, Corners::all(px(4.)), data.clone(), 0, false);
                    }
                }
                Body::Table(tb) => paint_table(f, tb, point(origin.x, y), window, cx),
                Body::Text {
                    text,
                    deco,
                    code,
                    image,
                } => {
                    if *code {
                        window.paint_quad(fill(
                            Bounds::new(point(origin.x, top), size(width, b.height)),
                            look.code_bg,
                        ));
                    }
                    // Quote bars span the whole box (no gaps between lines).
                    for q in 0..deco.quote {
                        let x = origin.x + look.quote_slot() * f32::from(q) + look.size * 0.15;
                        window.paint_quad(fill(
                            Bounds::new(point(x, top), size(px(3.), b.height)),
                            f.quote_bar,
                        ));
                    }
                    if let Some((m, slot)) = &deco.marker {
                        paint_marker(f, m, origin.x + *slot, y, text.line_height, window, cx);
                    }
                    let at = point(origin.x, y);
                    if let Some(m) = f.marks.get(&i) {
                        for (r, current) in &m.found {
                            for rect in text.rects(r.clone()) {
                                let c = if *current { f.find_current } else { f.find };
                                window.paint_quad(fill(offset(rect, at), c));
                            }
                        }
                        for r in &m.selection {
                            let rects = if r.is_empty() {
                                vec![Bounds::new(
                                    text.caret(r.start),
                                    size(px(6.), text.line_height),
                                )]
                            } else {
                                text.rects(r.clone())
                            };
                            for rect in rects {
                                window.paint_quad(fill(offset(rect, at), f.selection));
                            }
                        }
                    }
                    paint_text(text, at, window, cx);
                    if let Some(img) = image
                        && let Some(data) = f.images.get(&img.path)
                    {
                        let r = Bounds::new(
                            point(origin.x, y + text.height() + look.size * 0.25),
                            img.size,
                        );
                        let _ =
                            window.paint_image(r, r, Corners::all(px(4.)), data.clone(), 0, false);
                    }
                    if let Some(c) = f.marks.get(&i).and_then(|m| m.caret) {
                        let p = text.caret(c);
                        window.paint_quad(fill(
                            Bounds::new(
                                point(at.x + p.x, at.y + p.y),
                                size(kantor_ui::metrics::CARET_WIDTH, text.line_height),
                            ),
                            f.caret,
                        ));
                    }
                }
            }
        }
    });
    window.handle_input(
        &f.focus,
        ElementInputHandler::new(bounds, f.view.clone()),
        cx,
    );
}

fn offset(b: Bounds<Pixels>, by: Point<Pixels>) -> Bounds<Pixels> {
    Bounds::new(point(b.origin.x + by.x, b.origin.y + by.y), b.size)
}

fn paint_text(t: &TextBox, at: Point<Pixels>, window: &mut Window, cx: &mut App) {
    if let Some(w) = &t.wrapped {
        let _ = w.paint(
            point(at.x + t.x, at.y),
            t.line_height,
            TextAlign::Left,
            None,
            window,
            cx,
        );
    }
}

fn paint_marker(
    f: &Frame,
    m: &MarkerDeco,
    slot_x: Pixels,
    y: Pixels,
    line_height: Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    let look = &f.look;
    let slot = look.list_slot();
    let mid_y = y + line_height / 2.;
    match m {
        MarkerDeco::Bullet { depth } => {
            let r = look.size * 0.17;
            let c = point(slot_x + slot * 0.45, mid_y);
            let b = Bounds::new(point(c.x - r, c.y - r), size(r * 2., r * 2.));
            // Deeper levels: hollow, like most Markdown themes.
            let quad = if depth % 2 == 0 {
                quad(
                    b,
                    r,
                    transparent_black(),
                    px(1.2),
                    look.muted,
                    BorderStyle::default(),
                )
            } else {
                quad(
                    b,
                    r,
                    look.muted,
                    px(0.),
                    transparent_black(),
                    BorderStyle::default(),
                )
            };
            window.paint_quad(quad);
        }
        MarkerDeco::Number(n) => {
            let run = TextRun {
                len: n.len(),
                font: font(look.font.clone()),
                color: look.muted,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let line = window
                .text_system()
                .shape_line(n.clone(), look.size, &[run], None);
            let x = slot_x + slot - line.width() - look.size * 0.4;
            let _ = line.paint(
                point(x.max(slot_x - slot), y),
                line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }
        MarkerDeco::Task { checked } => {
            let s = look.size * 0.95;
            let b = Bounds::new(
                point(slot_x + (slot - s) / 2. - look.size * 0.1, mid_y - s / 2.),
                size(s, s),
            );
            if *checked {
                window.paint_quad(quad(
                    b,
                    px(3.),
                    f.check,
                    px(0.),
                    transparent_black(),
                    BorderStyle::default(),
                ));
                let mut p = PathBuilder::stroke(px(1.8));
                p.move_to(point(b.origin.x + s * 0.24, b.origin.y + s * 0.52));
                p.line_to(point(b.origin.x + s * 0.43, b.origin.y + s * 0.70));
                p.line_to(point(b.origin.x + s * 0.76, b.origin.y + s * 0.32));
                if let Ok(path) = p.build() {
                    window.paint_path(path, f.check_fg);
                }
            } else {
                window.paint_quad(quad(
                    b,
                    px(3.),
                    transparent_black(),
                    px(1.5),
                    look.muted,
                    BorderStyle::default(),
                ));
            }
        }
    }
}

fn paint_table(
    f: &Frame,
    t: &crate::layout::TableBox,
    at: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let look = &f.look;
    let x0 = at.x + t.x;
    let height = t.rows.last().map_or(px(0.), |r| r.top + r.height);
    for r in &t.rows {
        if r.header {
            window.paint_quad(fill(
                Bounds::new(point(x0, at.y + r.top), size(t.width, r.height)),
                f.header_bg,
            ));
        }
        for c in &r.cells {
            let pad_y = (r.height - c.text.height()) / 2.;
            if let Some(w) = &c.text.wrapped {
                let align = match c.align {
                    notes_engine::parse::Align::Center => TextAlign::Center,
                    notes_engine::parse::Align::Right => TextAlign::Right,
                    _ => TextAlign::Left,
                };
                let o = point(x0 + c.x, at.y + r.top + pad_y);
                let _ = w.paint(
                    o,
                    c.text.line_height,
                    align,
                    Some(Bounds::new(o, size(c.width, c.text.height()))),
                    window,
                    cx,
                );
            }
        }
        // Rule under each row.
        window.paint_quad(fill(
            Bounds::new(
                point(x0, at.y + r.top + r.height - px(1.)),
                size(t.width, px(1.)),
            ),
            look.border,
        ));
    }
    // Frame and column lines.
    window.paint_quad(outline(
        Bounds::new(at, size(t.width, height)),
        look.border,
        BorderStyle::default(),
    ));
    let mut x = x0;
    for w in t.cols.iter().take(t.cols.len().saturating_sub(1)) {
        x += *w;
        window.paint_quad(fill(
            Bounds::new(point(x, at.y), size(px(1.), height)),
            look.border,
        ));
    }
}
